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

use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use std::time::Duration;

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
#[allow(dead_code, reason = "Task 2 (SMA-473) wires up the 8 call sites; unused until then")]
pub(crate) async fn connect(redis_url: &str) -> redis::RedisResult<ConnectionManager> {
    let client = redis::Client::open(redis_url)?;
    ConnectionManager::new_with_config(client, connection_manager_config()).await
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
}
