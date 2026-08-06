// SPDX-License-Identifier: Apache-2.0

//! The single place this service constructs a Redis [`ConnectionManager`] (SMA-473).
//!
//! **Why this module exists.** `ConnectionManager::new` applies a stock
//! `ConnectionManagerConfig::default()`, whose reconnect budget is 6 retries on a
//! `100+200+400+800+1600+3200 ms` schedule. `backon` adds jitter (`delay × [1,2]`), so a
//! dead backend costs ~6.3–12.6 s per cycle — and a `ConnectionManager` burns a full cycle
//! per failed command, because the failing command triggers a background reconnect and the
//! NEXT command awaits a brand-new cycle. A single authz decision makes 2–3 such reads, for
//! a measured 19–28 s; a revoke, 28.4 s.
//!
//! **What the budget actually buys.** Only tolerance while ESTABLISHING a connection.
//! `send_packed_command` never retries a *command* — it surfaces the error to the caller and
//! reconnects in the background. So the case one retry covers is narrow and specific: a
//! first connect attempt landing in a failover gap (old primary gone, new one not yet
//! accepting), which `min_delay` (100–200 ms jittered) is well matched to.
//!
//! **What is deliberately left alone** (SMA-473 D1) — `min_delay`, `exponent_base`,
//! `connection_timeout` (1 s) and `response_timeout` (500 ms). The last two are already
//! bounded by redis-rs and are NOT what costs the time; tightening them was considered and
//! declined. Note the consequence: this bounds a *stopped* Redis (instant `ECONNREFUSED`),
//! not a *blackholed* one, where `connection_timeout` dominates at ~2.1 s per command.

use metrics::{counter, gauge};
use paigasus_observability::names;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Retries AFTER the first attempt (redis-rs defaults to 6 — see the module doc for what
/// that costs). One retry covers a first connect attempt landing in a failover gap;
/// anything more just adds latency to a genuine outage.
const CONNECT_RETRIES: usize = 1;

/// Guard only — **inert** at `CONNECT_RETRIES = 1`. `backon` applies `max_delay` to the
/// pre-jitter base delay and never to the first step (the first delay is always
/// `min_delay`), so with a single retry this is never reached. It exists so that raising
/// `CONNECT_RETRIES` later caps each step here rather than at `backon`'s own 60 s default.
const RETRY_MAX_DELAY: Duration = Duration::from_millis(500);

/// The tuned config every Redis connection in this service is opened with.
///
/// `pub(crate)` and exposed separately from [`connect`] so the config tests can assert on it
/// directly, and so the two `#[cfg(test)]` lazy managers elsewhere in this crate can build
/// from the exact production config rather than a hand-rolled copy.
pub(crate) fn connection_manager_config() -> ConnectionManagerConfig {
    ConnectionManagerConfig::new().set_number_of_retries(CONNECT_RETRIES).set_max_delay(RETRY_MAX_DELAY)
}

/// Opens `redis_url` and wraps it in a [`ConnectionManager`] built with
/// [`connection_manager_config`] — the ONLY way this crate constructs one (enforced by the
/// `repo:redis-connect-single-site` CI gate).
///
/// **Eager**: `new_with_config` awaits the initial connection, so a Redis that is down at
/// boot still fails `AppState::new` rather than yielding a manager that fails later. That
/// preserves the pre-SMA-473 contract — but note the tolerance window shrinks from ~6–12 s
/// to ~200 ms, so a Redis slow to start now costs one crash-restart (SMA-473 D10).
///
/// Returns a bare [`redis::RedisResult`] rather than a domain error because the callers map
/// it differently on purpose: `http::connect_redis` to `AuthnError::Backend`,
/// `RedisJwksCache::connect` to the fail-closed `AuthnError::Unavailable`.
pub(crate) async fn connect(redis_url: &str) -> redis::RedisResult<ConnectionManager> {
    let client = redis::Client::open(redis_url)?;
    ConnectionManager::new_with_config(client, connection_manager_config()).await
}

// ---- SMA-476: circuit breaker ------------------------------------------------------------
//
// Task 3's `RedisHandle` wires this state machine into the production command call sites;
// until then, everything below runs only from `#[cfg(test)]`, and this workspace denies
// `dead_code` in-source (`[workspace.lints.rust] warnings = "deny"`). Items unreachable from
// the plain (non-test) `lib` target below carry `#[allow(dead_code)]`; each is removed once
// Task 3 lands and calls it from production code.

/// Consecutive counted failures that trip the breaker (SMA-476 D6). Three rather than one so a
/// first connect attempt landing in a failover gap — the case SMA-473's single retry exists for —
/// does not disable the cache. Under real concurrency a failover WILL still trip this; see D6.
pub(crate) const FAILURE_THRESHOLD: u32 = 3;

/// How long an open breaker short-circuits before admitting one probe (SMA-476 D7).
///
/// Two seconds, not five, because recovery costs TWO windows: `ConnectionManager` holds an
/// `ArcSwap<Shared<..>>` connect future (`redis-1.3.0/src/aio/connection_manager.rs:335,387`) and
/// `Shared` MEMOIZES, so the first probe after a quiet window consumes an already-resolved `Err`
/// in microseconds without touching the network, and only the SECOND probe sees a fresh dial.
/// The upside of the same fact is that probes are essentially free, which is what makes a short
/// window affordable.
pub(crate) const OPEN_DURATION: Duration = Duration::from_secs(2);

/// A half-open older than this admits another probe regardless (SMA-476 D8) — the second of two
/// defences against a probe whose future was dropped before it could report.
pub(crate) const HALF_OPEN_DEADLINE: Duration = Duration::from_secs(5);

/// The short-circuit error's message. Pinned by test and deliberately free of any URL, host or
/// credential: `cedar_authorizer.rs` and `generation.rs` log the wrapping `AuthzError` with
/// `error = %err`, so this literal reaches the logs (SMA-476 D4).
pub(crate) const BREAKER_OPEN_MESSAGE: &str = "redis circuit breaker open (SMA-476)";

/// Which connection a breaker guards. A CLOSED set, so the `role` metric label is bounded by the
/// type system and cannot mint cardinality (SMA-476 D10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum RedisRole {
    Authz,
    ApiKeys,
    Jwks,
}

impl RedisRole {
    fn as_label(self) -> &'static str {
        match self {
            RedisRole::Authz => "authz",
            RedisRole::ApiKeys => "api_keys",
            RedisRole::Jwks => "jwks",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum BreakerState {
    Closed,
    HalfOpen,
    Open,
}

impl BreakerState {
    fn gauge_value(self) -> f64 {
        match self {
            BreakerState::Closed => 0.0,
            BreakerState::HalfOpen => 1.0,
            BreakerState::Open => 2.0,
        }
    }

    #[allow(dead_code)]
    fn as_label(self) -> &'static str {
        match self {
            BreakerState::Closed => "closed",
            BreakerState::HalfOpen => "half_open",
            BreakerState::Open => "open",
        }
    }
}

#[allow(dead_code)]
struct Inner {
    state: BreakerState,
    consecutive_failures: u32,
    changed_at: Instant,
}

/// Per-connection circuit breaker (SMA-476 D1). `Arc`-shared, so every clone of a
/// [`RedisHandle`] observes one breaker — load-bearing, because all eleven command call sites do
/// `self.conn.clone()` per command.
#[allow(dead_code)]
pub(crate) struct Breaker {
    role: RedisRole,
    open_duration: Duration,
    half_open_deadline: Duration,
    inner: Mutex<Inner>,
}

/// What [`Breaker::admit`] decided. `Pass` carries an RAII permit whose `Drop` records a failure
/// if no outcome was reported (SMA-476 D8).
#[allow(dead_code)]
pub(crate) enum Admission {
    Pass(ProbePermit),
    ShortCircuit,
}

/// Reports one command's outcome back to the breaker. Consumed by [`ProbePermit::record`]; if it
/// is instead DROPPED without recording — an axum handler future cancelled by a client
/// disconnect — `Drop` records a failure, so a half-open breaker can never wedge (SMA-476 D8).
#[allow(dead_code)]
pub(crate) struct ProbePermit {
    breaker: Arc<Breaker>,
    reported: bool,
}

impl ProbePermit {
    #[allow(dead_code)]
    pub(crate) fn record<T>(mut self, result: &redis::RedisResult<T>) {
        self.reported = true;
        let healthy = match result {
            Ok(_) => true,
            // An uncounted error means the backend ANSWERED — the connection is healthy, and the
            // fault is ours or the data's (SMA-476 D5).
            Err(err) => !counts_as_failure(err),
        };
        if healthy {
            self.breaker.on_success();
        } else {
            self.breaker.on_failure();
        }
    }
}

impl Drop for ProbePermit {
    fn drop(&mut self) {
        if !self.reported {
            self.breaker.on_failure();
        }
    }
}

#[allow(dead_code)]
impl Breaker {
    pub(crate) fn new(role: RedisRole) -> Arc<Breaker> {
        Breaker::with_durations(role, OPEN_DURATION, HALF_OPEN_DEADLINE)
    }

    pub(crate) fn with_durations(role: RedisRole, open_duration: Duration, half_open_deadline: Duration) -> Arc<Breaker> {
        // Publish the healthy state up front (SMA-476 D10): an unset gauge renders as "No data",
        // which an operator cannot distinguish from a broken scrape or an unregistered metric.
        gauge!(names::IAM_REDIS_BREAKER_STATE, "role" => role.as_label()).set(BreakerState::Closed.gauge_value());
        Arc::new(Breaker {
            role,
            open_duration,
            half_open_deadline,
            inner: Mutex::new(Inner {
                state: BreakerState::Closed,
                consecutive_failures: 0,
                changed_at: Instant::now(),
            }),
        })
    }

    /// Decide whether this command may reach the backend.
    ///
    /// The mutex guard is dropped before returning and is NEVER held across an `.await` — holding
    /// it would make the returned future `!Send` and break `AsyncCommands`' blanket impl, which
    /// requires `Send + Sync` (`redis-1.3.0/src/commands/mod.rs:3288`).
    pub(crate) fn admit(self: &Arc<Self>) -> Admission {
        {
            let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match inner.state {
                BreakerState::Closed => {}
                BreakerState::Open => {
                    if inner.changed_at.elapsed() < self.open_duration {
                        return Admission::ShortCircuit;
                    }
                    // Exactly one caller wins this transition; everyone else arrives to find
                    // HalfOpen and short-circuits below rather than queueing (SMA-476 D8).
                    self.transition(&mut inner, BreakerState::HalfOpen);
                }
                BreakerState::HalfOpen => {
                    if inner.changed_at.elapsed() < self.half_open_deadline {
                        return Admission::ShortCircuit;
                    }
                    // Stale: the admitted probe never reported and never dropped. Re-arm.
                    self.transition(&mut inner, BreakerState::HalfOpen);
                }
            }
        }
        Admission::Pass(ProbePermit {
            breaker: Arc::clone(self),
            reported: false,
        })
    }

    fn on_success(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.consecutive_failures = 0;
        if inner.state != BreakerState::Closed {
            self.transition(&mut inner, BreakerState::Closed);
        }
    }

    fn on_failure(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match inner.state {
            BreakerState::Closed => {
                inner.consecutive_failures = inner.consecutive_failures.saturating_add(1);
                if inner.consecutive_failures >= FAILURE_THRESHOLD {
                    self.transition(&mut inner, BreakerState::Open);
                }
            }
            BreakerState::HalfOpen => self.transition(&mut inner, BreakerState::Open),
            BreakerState::Open => {}
        }
    }

    fn transition(&self, inner: &mut Inner, next: BreakerState) {
        inner.state = next;
        inner.changed_at = Instant::now();
        if next == BreakerState::Closed {
            inner.consecutive_failures = 0;
        }
        gauge!(names::IAM_REDIS_BREAKER_STATE, "role" => self.role.as_label()).set(next.gauge_value());
        counter!(names::IAM_REDIS_BREAKER_TRANSITIONS_TOTAL, "role" => self.role.as_label(), "to" => next.as_label()).increment(1);
    }

    // Task 3 removes this `#[allow(dead_code)]`: `force_open_for_tests` is unused until
    // Task 3's connection-handle tests call it.
    #[allow(dead_code)]
    #[cfg(test)]
    pub(crate) fn force_open_for_tests(self: &Arc<Self>) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.transition(&mut inner, BreakerState::Open);
    }
}

/// Which redis errors mean "the connection is bad" (SMA-476 D5). NEITHER half is sufficient
/// alone:
///
/// - `retry_method()` alone misses the blackhole case. A connect timeout becomes
///   `io::ErrorKind::TimedOut` (`redis-1.3.0/src/aio/runtime.rs:189-193`), i.e. `ErrorKind::Io`
///   that is not `is_connection_dropped()`, and `retry_method` maps that to `RetryImmediately`,
///   NOT `Reconnect` (`errors/redis_error.rs:451-464`).
/// - `is_io_error()` alone misses `Parse` and `AuthenticationFailed`, which redis-rs itself
///   treats as connection-fatal (`errors/redis_error.rs:447-448`) and which would otherwise drive
///   an endless reconnect loop the breaker never opens on.
///
/// Everything else — `UnexpectedReturnType`, `Client`, `Extension`, `InvalidClientConfig`,
/// `RESP3NotSupported`, every `Server(..)` — means the backend answered and is healthy.
/// `ErrorKind` is `#[non_exhaustive]`, so this must not be written as an exhaustive match.
#[allow(dead_code)]
fn counts_as_failure(err: &redis::RedisError) -> bool {
    err.is_io_error() || matches!(err.retry_method(), redis::RetryMethod::Reconnect | redis::RetryMethod::ReconnectFromInitialConnections)
}

/// The error an open breaker returns instead of dialling. `ErrorKind::Io` so `is_io_error()`
/// holds and all five adapters' error arms fire exactly as they do against a genuinely dead
/// socket (SMA-476 D4) — they all read `err.kind()` and nothing else.
#[allow(dead_code)]
fn breaker_open_error() -> redis::RedisError {
    redis::RedisError::from((redis::ErrorKind::Io, BREAKER_OPEN_MESSAGE))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The change itself. If this fails, a Redis outage costs seconds per authz decision
    /// again (SMA-473) — do not "fix" it by relaxing the assertion.
    #[test]
    fn the_tuned_config_caps_the_reconnect_retry_budget() {
        let cfg = connection_manager_config();
        assert_eq!(
            cfg.number_of_retries(),
            1,
            "SMA-473: the reconnect retry count must stay capped at 1 — redis-rs defaults to 6, \
             which costs ~6.3-12.6s per failed command and a measured 19-28s per authz decision"
        );
        assert_eq!(
            cfg.max_delay(),
            Some(Duration::from_millis(500)),
            "SMA-473: the max_delay guard must stay set — it is inert at 1 retry, but it is what \
             caps each step if the retry count is ever raised (backon's own default is 60s/step)"
        );
    }

    /// Pins the OTHER half of D1 — the knobs deliberately NOT touched. Asserted twice on
    /// purpose: against a stock config (catches us tightening one) AND against absolute
    /// values (catches a redis-rs bump moving a default under us).
    #[test]
    fn the_tuned_config_leaves_every_other_knob_at_the_redis_rs_default() {
        let cfg = connection_manager_config();
        let stock = ConnectionManagerConfig::new();

        assert_eq!(cfg.min_delay(), stock.min_delay(), "SMA-473 D1: min_delay must stay at the redis-rs default");
        assert_eq!(
            cfg.connection_timeout(),
            stock.connection_timeout(),
            "SMA-473 D1: connection_timeout must stay at the redis-rs default — it is already bounded \
             and is NOT what costs the time during an outage"
        );
        assert_eq!(cfg.response_timeout(), stock.response_timeout(), "SMA-473 D1: response_timeout must stay at the redis-rs default");
        assert!(
            (cfg.exponent_base() - stock.exponent_base()).abs() < f32::EPSILON,
            "SMA-473 D1: exponent_base must stay at the redis-rs default"
        );

        assert_eq!(
            cfg.min_delay(),
            Duration::from_millis(100),
            "redis-rs 1.3.0's documented min_delay default moved — re-check the SMA-473 arithmetic"
        );
        assert_eq!(
            cfg.connection_timeout(),
            Some(Duration::from_secs(1)),
            "redis-rs 1.3.0's documented connection_timeout default moved — re-check the SMA-473 arithmetic"
        );
        assert_eq!(
            cfg.response_timeout(),
            Some(Duration::from_millis(500)),
            "redis-rs 1.3.0's documented response_timeout default moved — re-check the SMA-473 arithmetic"
        );
    }

    /// Proves the config is actually APPLIED to a real manager rather than built and
    /// dropped. Deliberately loose (2 s vs a ~100-200 ms expectation) — the two tests above
    /// own exactness; this one only has to fail if the config never reaches the manager.
    ///
    /// `#[tokio::test]` is REQUIRED: `new_lazy_with_config` calls `runtime.spawn`, which
    /// panics outside a Tokio runtime. `127.0.0.1:1` is a closed port, so the connect is
    /// refused instantly rather than timing out (same pattern as
    /// `entity_cache`/`api_keys::cache`'s unreachable-backend tests).
    #[tokio::test]
    async fn a_command_against_an_unreachable_backend_fails_fast() {
        use redis::AsyncCommands;

        let client = redis::Client::open("redis://127.0.0.1:1").expect("well-formed redis URL, never actually reachable");
        let mut conn = ConnectionManager::new_lazy_with_config(client, connection_manager_config()).expect("lazy ConnectionManager construction never connects");

        let started = std::time::Instant::now();
        let result: redis::RedisResult<Option<Vec<u8>>> = conn.get("sma473:probe").await;
        let elapsed = started.elapsed();

        // Control: without this the deadline could pass for the WRONG reason — a malformed
        // URL or an invalid config erroring instantly looks identical to a fast, correct
        // failure. (It cannot separate a fast refuse from a slow timeout; the deadline does.)
        let err = result.expect_err("an unreachable backend must error, not return a value");
        assert!(err.is_io_error(), "expected an IO/connection error, got {err:?} — the probe never actually dialed");

        assert!(
            elapsed < Duration::from_secs(2),
            "SMA-473: a command against a dead Redis took {elapsed:?}; the tuned config must bound it \
             well under 2s (stock redis-rs is ~6.3-12.6s and cost a measured 19-28s per authz decision)"
        );
    }

    /// Guards the eager contract directly (SMA-473 D10). The three tests above all exercise
    /// `connection_manager_config()` or a hand-built `new_lazy_with_config` manager — none of
    /// them call [`connect`] itself, so a later edit swapping its `new_with_config` for
    /// `new_lazy_with_config` would pass every one of them. Against a dead port an eager
    /// `connect` returns `Err` (it awaits the initial connection); a lazy one would return
    /// `Ok` (the manager builds fine — the error only surfaces on the first command), so this
    /// assertion actually distinguishes the two implementations rather than just exercising
    /// the happy path.
    #[tokio::test]
    async fn connect_is_eager_so_a_dead_backend_fails_at_construction() {
        let started = std::time::Instant::now();
        let result = connect("redis://127.0.0.1:1").await;
        let elapsed = started.elapsed();

        let err = result.expect_err(
            "connect() returned Ok against an unreachable backend — that means connect went \
             lazy, and AppState::new would no longer fail fast at boot (SMA-473 D10)",
        );
        assert!(err.is_io_error(), "expected an IO/connection error, got {err:?} — the probe never actually dialed");

        assert!(
            elapsed < Duration::from_secs(2),
            "SMA-473: connect() against a dead Redis took {elapsed:?}; the tuned config must bound it \
             well under 2s (stock redis-rs is ~6.3-12.6s and cost a measured 19-28s per authz decision)"
        );
    }

    // ---- SMA-476: circuit breaker ------------------------------------------------------

    /// Test breakers use a 50ms window so the whole suite stays fast; production values are
    /// pinned separately by `the_breaker_constants_are_pinned`.
    fn test_breaker() -> std::sync::Arc<Breaker> {
        Breaker::with_durations(RedisRole::Authz, Duration::from_millis(50), Duration::from_millis(200))
    }

    fn io_err() -> redis::RedisError {
        redis::RedisError::from((redis::ErrorKind::Io, "test io failure"))
    }

    fn pass(b: &std::sync::Arc<Breaker>) -> ProbePermit {
        match b.admit() {
            Admission::Pass(permit) => permit,
            Admission::ShortCircuit => panic!("expected the breaker to admit, but it short-circuited"),
        }
    }

    fn fail_once(b: &std::sync::Arc<Breaker>) {
        pass(b).record::<()>(&Err(io_err()));
    }

    #[test]
    fn a_closed_breaker_admits_every_command() {
        let b = test_breaker();
        for _ in 0..10 {
            pass(&b).record(&Ok(()));
        }
    }

    #[test]
    fn a_success_resets_the_consecutive_failure_count() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        pass(&b).record(&Ok(()));
        // Without the reset this third failure would be the threshold-tripping one.
        fail_once(&b);
        fail_once(&b);
        assert!(matches!(b.admit(), Admission::Pass(_)), "two failures after a reset must not open the breaker");
    }

    #[test]
    fn three_consecutive_failures_open_the_breaker() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        fail_once(&b);
        assert!(matches!(b.admit(), Admission::ShortCircuit), "SMA-476 D6: three consecutive failures must open the breaker");
    }

    /// Named for what it actually asserts: a long run of short-circuits must not disturb
    /// recovery. (It cannot separately prove short-circuits are "not counted" — `on_failure` is
    /// a no-op in the Open state either way — so it does not claim to.)
    #[test]
    fn an_open_breaker_still_recovers_after_many_short_circuits() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        fail_once(&b);
        for _ in 0..100 {
            assert!(matches!(b.admit(), Admission::ShortCircuit));
        }
        std::thread::sleep(Duration::from_millis(60));
        pass(&b).record(&Ok(()));
        assert!(matches!(b.admit(), Admission::Pass(_)), "a successful probe must close the breaker");
    }

    #[test]
    fn exactly_one_probe_is_admitted_after_the_open_window() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        fail_once(&b);
        std::thread::sleep(Duration::from_millis(60));

        let mut admitted = 0;
        let mut permits = Vec::new();
        for _ in 0..50 {
            if let Admission::Pass(p) = b.admit() {
                admitted += 1;
                permits.push(p);
            }
        }
        assert_eq!(admitted, 1, "SMA-476 D8: half-open must admit exactly one probe, not {admitted}");
        std::mem::forget(permits); // keep Drop from re-opening before the assertion above is read
    }

    #[test]
    fn a_failed_probe_reopens_the_breaker_for_another_full_window() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        fail_once(&b);
        std::thread::sleep(Duration::from_millis(60));
        pass(&b).record::<()>(&Err(io_err()));
        assert!(matches!(b.admit(), Admission::ShortCircuit), "a failed probe must re-open immediately");
    }

    /// SMA-476 D8, defence 1. A probe future dropped mid-await (axum drops handler futures on
    /// client disconnect) must NOT leave the breaker half-open forever.
    #[test]
    fn a_dropped_probe_permit_reopens_the_breaker() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        fail_once(&b);
        std::thread::sleep(Duration::from_millis(60));

        drop(pass(&b)); // never records an outcome

        assert!(
            matches!(b.admit(), Admission::ShortCircuit),
            "SMA-476 D8: a dropped ProbePermit must record a failure and re-open — otherwise the \
             breaker wedges half-open for the process lifetime, silently bypassing the cache forever"
        );
    }

    /// SMA-476 D8, defence 2 — belt and braces, in case defence 1 is ever refactored away.
    #[test]
    fn a_stale_half_open_admits_another_probe() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        fail_once(&b);
        std::thread::sleep(Duration::from_millis(60));

        let permit = pass(&b);
        std::mem::forget(permit); // simulate a probe that never reports AND never drops
        assert!(matches!(b.admit(), Admission::ShortCircuit), "still within the half-open deadline");

        std::thread::sleep(Duration::from_millis(210));
        assert!(
            matches!(b.admit(), Admission::Pass(_)),
            "SMA-476 D8: a half-open older than HALF_OPEN_DEADLINE must admit another probe"
        );
    }

    /// SMA-476 D5. Neither half of the classifier is sufficient alone — see the decision.
    #[test]
    fn the_failure_classifier_counts_connection_errors_only() {
        // An IO error: the ordinary dead-backend case.
        assert!(counts_as_failure(&redis::RedisError::from((redis::ErrorKind::Io, "io"))));
        // A connect TIMEOUT — the blackhole case this whole issue is about. redis-rs maps it to
        // io::ErrorKind::TimedOut (aio/runtime.rs:189-193), i.e. ErrorKind::Io, whose
        // retry_method() is RetryImmediately and NOT Reconnect — so a Reconnect-only classifier
        // would never open the breaker on a blackhole.
        assert!(counts_as_failure(&redis::RedisError::from(std::io::Error::from(std::io::ErrorKind::TimedOut))));
        // Connection-fatal to redis-rs but NOT io errors (redis_error.rs:447-448).
        assert!(counts_as_failure(&redis::RedisError::from((redis::ErrorKind::Parse, "parse"))));
        assert!(counts_as_failure(&redis::RedisError::from((redis::ErrorKind::AuthenticationFailed, "auth"))));
        // The backend ANSWERED — it is healthy, the fault is ours or the data's. Counting these
        // would let a data bug disable caching fleet-wide. (Note: `TypeError` does not exist in
        // redis 1.3.0 — the variant is `UnexpectedReturnType`.)
        assert!(!counts_as_failure(&redis::RedisError::from((redis::ErrorKind::UnexpectedReturnType, "type"))));
        assert!(!counts_as_failure(&redis::RedisError::from((redis::ErrorKind::Client, "client"))));
    }

    /// An error the classifier does NOT count must behave like a success: the backend answered,
    /// so the connection is healthy.
    #[test]
    fn an_uncounted_error_resets_the_failure_count() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        pass(&b).record::<()>(&Err(redis::RedisError::from((redis::ErrorKind::UnexpectedReturnType, "type"))));
        fail_once(&b);
        fail_once(&b);
        assert!(matches!(b.admit(), Admission::Pass(_)), "an uncounted error must reset the consecutive-failure count");
    }

    /// SMA-476 D4. The literal reaches the logs: unlike the five adapters (which log
    /// `err.kind()` only), `cedar_authorizer.rs:167` and `generation.rs:141` log the wrapping
    /// AuthzError with `error = %err`, i.e. this Display.
    #[test]
    fn the_short_circuit_error_is_an_io_error_carrying_no_connection_details() {
        let err = breaker_open_error();
        assert!(
            err.is_io_error(),
            "SMA-476 D4: the synthetic error must be indistinguishable from a real connection failure to all five adapters"
        );
        let rendered = err.to_string();
        assert!(rendered.contains(BREAKER_OPEN_MESSAGE), "expected the pinned literal in {rendered:?}");
        assert!(
            !rendered.contains("redis://") && !rendered.contains("127.0.0.1"),
            "the short-circuit error must never echo connection details: {rendered:?}"
        );
    }

    #[test]
    fn the_breaker_constants_are_pinned() {
        assert_eq!(
            FAILURE_THRESHOLD, 3,
            "SMA-476 D6: three consecutive failures — one would trip on every failover gap, which is exactly what SMA-473's single retry exists to absorb"
        );
        assert_eq!(
            OPEN_DURATION,
            Duration::from_secs(2),
            "SMA-476 D7: recovery costs TWO windows (a half-open probe consumes ConnectionManager's memoized connect future), so this bounds recovery at ~2x2s + one dial. Do not raise it without re-reading D7."
        );
        assert_eq!(
            HALF_OPEN_DEADLINE,
            Duration::from_secs(5),
            "SMA-476 D8: must comfortably exceed a worst-case ~2.1s dial so it never pre-empts a merely-slow probe"
        );
    }
}
