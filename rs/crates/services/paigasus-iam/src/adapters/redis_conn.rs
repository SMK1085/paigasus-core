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

/// A `ConnectionManager` behind a per-connection circuit breaker (SMA-476).
///
/// Implements [`redis::aio::ConnectionLike`], which is what makes the breaker transparent:
/// redis-rs's `AsyncCommands` is a blanket impl over that trait
/// (`redis-1.3.0/src/commands/mod.rs:3288` — note it requires `Send + Sync + Sized`, which the
/// trait's own declaration does not), so every `conn.get(..)` / `conn.set_ex(..)` call site keeps
/// compiling and behaving identically while gaining the breaker.
///
/// `req_packed_command` is the correct seam rather than merely a convenient one: it is where
/// `ConnectionManager` awaits its shared connect future, i.e. where the ~2.1 s against a
/// blackholed backend is actually spent.
///
/// **Coupling to watch on a redis-rs upgrade** (SMA-476 §6 risk 5): the `AsyncCommands` blanket
/// impl over `ConnectionLike`, and the `ArcSwap<Shared<..>>` memoization that makes recovery cost
/// two open windows rather than one.
#[derive(Clone, Debug)]
pub struct RedisHandle {
    conn: ConnectionManager,
    breaker: Arc<Breaker>,
}

impl redis::aio::ConnectionLike for RedisHandle {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a redis::Cmd) -> redis::RedisFuture<'a, redis::Value> {
        Box::pin(async move {
            let permit = match self.breaker.admit() {
                Admission::Pass(permit) => permit,
                Admission::ShortCircuit => return Err(breaker_open_error()),
            };
            let result = self.conn.req_packed_command(cmd).await;
            permit.record(&result);
            result
        })
    }

    fn req_packed_commands<'a>(&'a mut self, cmd: &'a redis::Pipeline, offset: usize, count: usize) -> redis::RedisFuture<'a, Vec<redis::Value>> {
        // Implemented for real even though no call site pipelines today: `unimplemented!()`
        // behind a trait method redis-rs may call is a live panic, not a placeholder.
        Box::pin(async move {
            let permit = match self.breaker.admit() {
                Admission::Pass(permit) => permit,
                Admission::ShortCircuit => return Err(breaker_open_error()),
            };
            let result = self.conn.req_packed_commands(cmd, offset, count).await;
            permit.record(&result);
            result
        })
    }

    fn get_db(&self) -> i64 {
        self.conn.get_db()
    }
}

/// Opens `redis_url` and wraps it in a [`RedisHandle`] — a [`ConnectionManager`] built with
/// [`connection_manager_config`] behind a fresh circuit breaker. The ONLY way this crate obtains
/// a Redis connection (enforced by the `repo:redis-connect-single-site` CI gate, which since
/// SMA-476 also bans naming the `ConnectionManager` type outside this module).
///
/// **Eager**: `new_with_config` awaits the initial connection, so a Redis that is down at boot
/// still fails `AppState::new` rather than yielding a manager that fails later. That preserves
/// the pre-SMA-473 contract — but note the tolerance window shrinks from ~6-12 s to ~200 ms, so a
/// Redis slow to start now costs one crash-restart (SMA-473 D10).
///
/// The boot dial is deliberately NOT breaker-mediated (SMA-476 D11): the breaker starts Closed
/// and wraps commands only. A single boot dial has nothing to break on.
///
/// Returns a bare [`redis::RedisResult`] rather than a domain error because the callers map it
/// differently on purpose: `http::connect_redis` to `AuthnError::Backend`,
/// `RedisJwksCache::connect` to the fail-closed `AuthnError::Unavailable`.
pub(crate) async fn connect(redis_url: &str, role: RedisRole) -> redis::RedisResult<RedisHandle> {
    let client = redis::Client::open(redis_url)?;
    let conn = ConnectionManager::new_with_config(client, connection_manager_config()).await?;
    Ok(RedisHandle { conn, breaker: Breaker::new(role) })
}

/// A lazily-connecting handle with a CLOSED breaker and short test durations.
///
/// Required wherever a test must actually dial: the production [`connect`] is eager, so against a
/// dead or blackholed backend it fails before any command can be issued.
#[cfg(test)]
pub(crate) fn new_lazy_for_tests(redis_url: &str, role: RedisRole) -> redis::RedisResult<RedisHandle> {
    let client = redis::Client::open(redis_url)?;
    let conn = ConnectionManager::new_lazy_with_config(client, connection_manager_config())?;
    Ok(RedisHandle { conn, breaker: Breaker::new(role) })
}

/// A lazily-connecting handle whose breaker is forced OPEN, for proving that a call site
/// short-circuits rather than dials. NOT interchangeable with [`new_lazy_for_tests`].
#[cfg(test)]
pub(crate) fn with_open_breaker_for_tests(redis_url: &str, role: RedisRole) -> redis::RedisResult<RedisHandle> {
    let handle = new_lazy_for_tests(redis_url, role)?;
    handle.breaker.force_open_for_tests();
    Ok(handle)
}

// ---- SMA-476: circuit breaker ------------------------------------------------------------
//
// `RedisHandle` (defined above `connect`) wires this state machine into the production
// command call sites via `ConnectionLike::req_packed_command`/`req_packed_commands`.

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

    fn as_label(self) -> &'static str {
        match self {
            BreakerState::Closed => "closed",
            BreakerState::HalfOpen => "half_open",
            BreakerState::Open => "open",
        }
    }
}

#[derive(Debug)]
struct Inner {
    state: BreakerState,
    consecutive_failures: u32,
    changed_at: Instant,
}

/// Per-connection circuit breaker (SMA-476 D1). `Arc`-shared, so every clone of a
/// [`RedisHandle`] observes one breaker — load-bearing, because all eleven command call sites do
/// `self.conn.clone()` per command.
///
/// `Debug` is derived (not part of the original design, added because [`connect`]'s Ok type
/// replaced [`ConnectionManager`] — which already implemented `Debug` — with [`RedisHandle`], and
/// `Result::expect_err` requires `T: Debug`; see `connect_is_eager_so_a_dead_backend_fails_at_construction`).
#[derive(Debug)]
pub(crate) struct Breaker {
    role: RedisRole,
    open_duration: Duration,
    half_open_deadline: Duration,
    inner: Mutex<Inner>,
}

/// What [`Breaker::admit`] decided. `Pass` carries an RAII permit whose `Drop` re-opens the
/// breaker if it was `HalfOpen` and no outcome was reported (SMA-476 D8) — in every other state,
/// a dropped permit is a no-op.
pub(crate) enum Admission {
    Pass(ProbePermit),
    ShortCircuit,
}

/// Reports one command's outcome back to the breaker. Consumed by [`ProbePermit::record`]; if it
/// is instead DROPPED without recording — an axum handler future cancelled by a client
/// disconnect (also `serve_http`'s `TimeoutLayer`) — `Drop` records a failure ONLY when the
/// breaker is `HalfOpen`, so a half-open breaker can never wedge (SMA-476 D8). In `Closed` (and
/// `Open`) it is a no-op: a dropped permit means no result was ever observed, so it is not
/// evidence about the backend — treating "no information" as "failure" is a category error, and
/// in `Closed` it would let cancelled client requests trip the breaker against a healthy Redis.
pub(crate) struct ProbePermit {
    breaker: Arc<Breaker>,
    reported: bool,
}

impl ProbePermit {
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
            self.breaker.on_probe_abandoned();
        }
    }
}

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

    /// Called from [`ProbePermit`]'s `Drop` when a permit was abandoned without recording an
    /// outcome. Transitions to `Open` ONLY if the breaker is currently `HalfOpen` — that is the
    /// one state where an abandoned probe is a wedge hazard (the single half-open slot is spent
    /// and nothing else will ever re-arm it before [`HALF_OPEN_DEADLINE`]). In `Closed` and
    /// `Open` this is a no-op: a dropped permit means no result was ever observed, so it carries
    /// no evidence about the backend's health — treating "no information" as "failure" is a
    /// category error, and in `Closed` it would let cancelled client requests (axum drops
    /// handler futures on client disconnect; `serve_http`'s `TimeoutLayer` too) open the breaker
    /// against a perfectly healthy Redis (SMA-476 D8).
    fn on_probe_abandoned(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if inner.state == BreakerState::HalfOpen {
            self.transition(&mut inner, BreakerState::Open);
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
fn counts_as_failure(err: &redis::RedisError) -> bool {
    err.is_io_error() || matches!(err.retry_method(), redis::RetryMethod::Reconnect | redis::RetryMethod::ReconnectFromInitialConnections)
}

/// The error an open breaker returns instead of dialling. `ErrorKind::Io` so `is_io_error()`
/// holds and all five adapters' error arms fire exactly as they do against a genuinely dead
/// socket (SMA-476 D4) — they all read `err.kind()` and nothing else.
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

        // Exactly ONE command: with more, SMA-476's breaker would open at the third and the
        // later ones would short-circuit rather than measuring a real dial.
        let mut conn = new_lazy_for_tests("redis://127.0.0.1:1", RedisRole::Authz).expect("well-formed redis URL, never actually reachable");

        let started = std::time::Instant::now();
        let result: redis::RedisResult<Option<Vec<u8>>> = conn.get("sma473:probe").await;
        let elapsed = started.elapsed();

        let err = result.expect_err("an unreachable backend must error, not return a value");
        assert!(err.is_io_error(), "expected an IO/connection error, got {err:?} — the probe never actually dialed");
        assert!(!err.to_string().contains(BREAKER_OPEN_MESSAGE), "this must measure a real dial, not a short-circuit");

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
        let result = connect("redis://127.0.0.1:1", RedisRole::Authz).await;
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

        // Discriminates Open from a still-HalfOpen breaker: the open window is 50ms and the
        // half-open deadline is 200ms, so only a breaker that actually transitioned to Open
        // re-admits after 60ms. If Drop::drop were a no-op, the breaker would still be
        // HalfOpen here (60ms < 200ms) and this would short-circuit instead.
        std::thread::sleep(Duration::from_millis(60));
        assert!(
            matches!(b.admit(), Admission::Pass(_)),
            "SMA-476 D8: a dropped ProbePermit must record a FAILURE, not merely leave the \
             breaker half-open — after the 50ms open window elapses the breaker must have \
             re-opened and be admitting a fresh probe; if it is still short-circuiting here, \
             Drop::drop is not recording a failure and the breaker will wedge half-open forever"
        );
    }

    /// SMA-476 D8 correction: a dropped `ProbePermit` while `Closed` must NOT be treated as a
    /// failure. Axum drops handler futures on client disconnect and `serve_http` wraps the
    /// router in a `TimeoutLayer`, so without this the breaker would open against a perfectly
    /// healthy Redis purely because clients hung up — a dropped permit means no result was ever
    /// observed, so it is not evidence about the backend.
    #[test]
    fn a_dropped_probe_permit_while_closed_does_not_open_the_breaker() {
        let b = test_breaker();

        drop(pass(&b)); // never records an outcome
        drop(pass(&b));
        drop(pass(&b)); // FAILURE_THRESHOLD (3) abandoned permits

        assert!(
            matches!(b.admit(), Admission::Pass(_)),
            "SMA-476 D8: a dropped ProbePermit while Closed must NOT count as a failure — a \
             dropped permit means no result was ever observed, so treating it as a failure lets \
             cancelled client requests (axum drops handler futures on disconnect; serve_http's \
             TimeoutLayer) trip the breaker against a perfectly healthy Redis"
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

    /// SMA-476 D1. Every one of the eleven call sites does `self.conn.clone()` per command, so a
    /// `#[derive(Clone)]` over a non-`Arc` breaker field would compile and silently give every
    /// call its own breaker — which would never open. This is that guard.
    #[tokio::test]
    async fn cloning_a_handle_shares_one_breaker() {
        use redis::AsyncCommands;

        let handle = with_open_breaker_for_tests("redis://127.0.0.1:1", RedisRole::Authz).expect("well-formed url");
        let mut clone = handle.clone();

        let started = std::time::Instant::now();
        let result: redis::RedisResult<Option<Vec<u8>>> = clone.get("sma476:probe").await;
        let elapsed = started.elapsed();

        let err = result.expect_err("an open breaker must short-circuit with an error");
        assert!(
            err.to_string().contains(BREAKER_OPEN_MESSAGE),
            "a CLONE dialled instead of short-circuiting — the breaker is not Arc-shared: {err:?}"
        );
        assert!(elapsed < Duration::from_millis(100), "short-circuit took {elapsed:?}");
    }

    /// The interception itself: an `AsyncCommands` call on a `RedisHandle` must route through the
    /// breaker. If `ConnectionLike` is ever implemented by delegating verbatim to the inner
    /// manager, this fails.
    #[tokio::test]
    async fn an_open_breaker_short_circuits_asynccommands_without_dialling() {
        use redis::AsyncCommands;

        let mut handle = with_open_breaker_for_tests("redis://127.0.0.1:1", RedisRole::Jwks).expect("well-formed url");
        let result: redis::RedisResult<Option<Vec<u8>>> = handle.get("sma476:probe").await;
        let err = result.expect_err("an open breaker must error");
        assert!(err.is_io_error(), "SMA-476 D4: the short-circuit error must be an IO error");
        assert!(err.to_string().contains(BREAKER_OPEN_MESSAGE));
    }

    /// SMA-476 AC1: the blackholed shape, MEASURED rather than calculated. Before this test the
    /// ~2.1 s figure in the issue and the RUNBOOK was arithmetic — nothing had ever been run
    /// against a backend that swallows SYNs.
    ///
    /// One test, both numbers: command #1 is the pre-breaker per-command cost (the breaker is
    /// still Closed), commands #4+ are the post-breaker cost.
    ///
    /// Bounds are deliberately loose — same posture as
    /// `a_command_against_an_unreachable_backend_fails_fast`'s 2 s. A contended CI runner adds
    /// scheduler jitter, and these assertions only have to discriminate between "dialled"
    /// (~2.1 s) and "short-circuited" (~0), not to pin exact timings.
    #[tokio::test]
    async fn a_blackholed_backend_costs_seconds_per_command_until_the_breaker_opens() {
        use redis::AsyncCommands;

        let blackhole = test_support::start().await;
        let mut conn = new_lazy_for_tests(&blackhole.url, RedisRole::Authz).expect("well-formed redis URL");

        let overall = std::time::Instant::now();

        // --- Command #1: breaker Closed, so this is a real dial. THE measured number. ---
        let started = std::time::Instant::now();
        let first: redis::RedisResult<Option<Vec<u8>>> = conn.get("sma476:probe").await;
        let first_elapsed = started.elapsed();
        eprintln!("SMA-476 AC1: command #1 (blackholed, breaker Closed) took {first_elapsed:?}");

        let err = first.expect_err("a blackholed backend must error");
        assert!(err.is_io_error(), "expected an IO/timeout error, got {err:?}");
        assert!(!err.to_string().contains(BREAKER_OPEN_MESSAGE), "command #1 must be a real dial, not a short-circuit");
        assert!(
            first_elapsed >= Duration::from_millis(1900),
            "a blackholed command took only {first_elapsed:?} — the listener REFUSED or reset instead of \
             blackholing (check that test_support retains the accepted TcpStream), so this test is \
             measuring the wrong thing"
        );
        assert!(
            first_elapsed < Duration::from_millis(3500),
            "a blackholed command took {first_elapsed:?}, well past the expected ~2.1s (2 x connection_timeout + one jittered min_delay)"
        );

        // --- Commands #2, #3: still Closed, still real dials. These trip the breaker. ---
        let _: redis::RedisResult<Option<Vec<u8>>> = conn.get("sma476:probe").await;
        let _: redis::RedisResult<Option<Vec<u8>>> = conn.get("sma476:probe").await;

        // --- Command #4 onwards: breaker Open, short-circuited. ---
        for i in 4..=10 {
            let started = std::time::Instant::now();
            let result: redis::RedisResult<Option<Vec<u8>>> = conn.get("sma476:probe").await;
            let elapsed = started.elapsed();
            let err = result.expect_err("an open breaker must error");
            assert!(
                err.to_string().contains(BREAKER_OPEN_MESSAGE),
                "command #{i} dialled instead of short-circuiting — the breaker never opened"
            );
            assert!(
                elapsed < Duration::from_millis(100),
                "command #{i} took {elapsed:?}; an open breaker must return without touching the network"
            );
        }

        // --- The aggregate, which is what makes the fix legible. ---
        // Ten un-broken commands cost ~21s. Three real dials plus seven short-circuits cost
        // ~6.3s. This bound sits between, with margin on both sides: it fails if the breaker
        // never opens, and passes only if it did.
        let total = overall.elapsed();
        eprintln!("SMA-476 AC1: all ten commands (breaker Closed then Open) took {total:?} in aggregate");
        assert!(
            total < Duration::from_secs(14),
            "ten commands against a blackholed backend took {total:?}; without the breaker this is ~21s, with it ~6.3s"
        );
    }
}

/// Shared test fixtures for the SMA-476 breaker tests. Lives here rather than in each adapter so
/// the five posture tests (Task 6) and the blackhole measurement (Task 4) use one implementation.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex as AsyncMutex;

    /// A listener that reproduces a BLACKHOLED Redis: it accepts the TCP connection and then
    /// never replies, so redis-rs's dial runs to `connection_timeout`.
    ///
    /// Why this is the right shape: the dial ALWAYS awaits a server response —
    /// `connection_setup_pipeline` always appends `CLIENT SETINFO LIB-NAME`/`LIB-VER`
    /// (`redis-1.3.0/src/connection.rs:1380-1400`) — and the whole dial, resolver included, sits
    /// inside `rt.timeout(connection_timeout)` (`client.rs:495-520`). So the connect burns the
    /// full 1 s, twice (one retry = two attempts), exactly as a dropped SYN would, with no root,
    /// no iptables and no Docker.
    ///
    /// **The accepted streams MUST be retained.** Dropping a `TcpStream` — the natural way to
    /// write "ignore the socket" — makes the kernel send FIN/RST, redis-rs's setup-pipeline read
    /// returns EOF immediately, and a command costs microseconds instead of ~2.1 s. The accept
    /// task's `JoinHandle` is retained for the same reason.
    ///
    /// **Precondition for the ~1 s-per-attempt bound:** the setup pipeline must be non-empty. It
    /// is guarded by `if !connection_info.skip_set_lib_name`, and an empty pipeline short-circuits
    /// to `Ok` without I/O (`aio/mod.rs:110-112`), which would move the hang to `response_timeout`
    /// (500 ms) instead. A plain `redis://host:port` URL (RESP2, no auth, db 0) keeps it non-empty
    /// — do not add credentials or a db index to these tests' URLs.
    pub(crate) struct Blackhole {
        pub(crate) url: String,
        // Read only by `start_responding`, below — dead until Task 7 (the recovery test) calls it.
        #[allow(dead_code)]
        responding: Arc<AtomicBool>,
        _held: Arc<AsyncMutex<Vec<tokio::net::TcpStream>>>,
        _accept: tokio::task::JoinHandle<()>,
    }

    impl Blackhole {
        /// Switch the listener from blackholing to answering as a minimal RESP server. Only
        /// affects connections opened AFTER this call. Used by the recovery test (Task 7); dead
        /// until it lands.
        #[allow(dead_code)]
        pub(crate) fn start_responding(&self) {
            self.responding.store(true, Ordering::SeqCst);
        }
    }

    pub(crate) async fn start() -> Blackhole {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binding an ephemeral loopback port never fails in tests");
        let port = listener.local_addr().expect("a bound listener always has a local address").port();
        let responding = Arc::new(AtomicBool::new(false));
        let held: Arc<AsyncMutex<Vec<tokio::net::TcpStream>>> = Arc::new(AsyncMutex::new(Vec::new()));

        let accept_responding = Arc::clone(&responding);
        let accept_held = Arc::clone(&held);
        let accept = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                if accept_responding.load(Ordering::SeqCst) {
                    tokio::spawn(serve_minimal_resp(stream));
                } else {
                    // Hold it open forever — see the struct doc for why dropping breaks the test.
                    accept_held.lock().await.push(stream);
                }
            }
        });

        Blackhole {
            url: format!("redis://127.0.0.1:{port}"),
            responding,
            _held: held,
            _accept: accept,
        }
    }

    /// Answers just enough RESP for redis-rs to complete a dial and one `GET`: `+OK` for each
    /// setup-pipeline command, `$-1` (null bulk string) for anything else. Deliberately dumb —
    /// it only has to let the half-open probe succeed.
    async fn serve_minimal_resp(mut stream: tokio::net::TcpStream) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut buf = vec![0_u8; 4096];
        loop {
            let Ok(n) = stream.read(&mut buf).await else { return };
            if n == 0 {
                return;
            }
            let request = String::from_utf8_lossy(&buf[..n]).to_ascii_uppercase();
            // One reply per command in the batch. The setup pipeline sends CLIENT SETINFO twice;
            // a GET arrives on its own.
            let commands = request.matches("\r\n$").count().max(1);
            let mut reply = String::new();
            for _ in 0..commands {
                if request.contains("GET") && !request.contains("SETINFO") {
                    reply.push_str("$-1\r\n");
                } else {
                    reply.push_str("+OK\r\n");
                }
            }
            if stream.write_all(reply.as_bytes()).await.is_err() {
                return;
            }
        }
    }
}
