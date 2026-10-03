// SPDX-License-Identifier: Apache-2.0

//! The two `LimitStore` adapters (SMA-677 spec § 4.3) and the boot wiring that picks one.

pub mod memory;
pub mod redis;

pub use self::redis::RedisLimitStore;
pub use memory::MemoryLimitStore;

use std::sync::Arc;

use secrecy::ExposeSecret;
use tokio_util::task::TaskTracker;

use crate::application::limits::{Limits, StoreBackend, prime_metrics};
use crate::config::{LimitsBackend, LimitsConfig};
use crate::domain::limits::SystemClock;

/// What `main.rs` puts into `AppState` and `runtime::supervise`.
#[derive(Default)]
pub struct LimitsWiring {
    pub limits: Option<Arc<Limits>>,
    /// The Redis store's spawned charges, drained at shutdown (D16). `None` for memory.
    pub charge_tasks: Option<TaskTracker>,
}

/// The boot error for a Redis that did not connect. It names the error kind only: redis error
/// text can carry the URL, a host or a password, and the boot error reaches the logs.
fn connect_failure_message(err: &::redis::RedisError) -> String {
    format!("the limits Redis store did not connect at boot (redis error kind={:?})", err.kind())
}

/// Build the limits from `[limits]` (spec § 4.4). Call it after `paigasus_observability::init`
/// (D17 "Order"): `prime_metrics` and the breaker's constructor need the installed recorder.
///
/// - No table: nothing at all (A6).
/// - A table that can only give empty policies: no store, no Redis connection, one `info` line (D11).
/// - `memory`: a `MemoryLimitStore`.
/// - `redis`: `RedisLimitStore::connect`, eager. A Redis that is down fails the boot (D17, Q14).
pub async fn build_limits(config: Option<&LimitsConfig>) -> anyhow::Result<LimitsWiring> {
    let Some(config) = config else {
        return Ok(LimitsWiring::default());
    };
    let backend = match config.backend {
        LimitsBackend::Memory => StoreBackend::Memory,
        LimitsBackend::Redis => StoreBackend::Redis,
    };
    prime_metrics(backend);
    let rules = config.rules();
    if rules.every_policy_is_empty() {
        tracing::info!(backend = ?config.backend, "[limits] is set, but no limit is configured: no store is built and no limit applies");
        return Ok(LimitsWiring::default());
    }
    let clock = Arc::new(SystemClock);
    match config.backend {
        LimitsBackend::Memory => Ok(LimitsWiring {
            limits: Some(Arc::new(Limits::new(rules, Arc::new(MemoryLimitStore::new()), clock))),
            charge_tasks: None,
        }),
        LimitsBackend::Redis => {
            let url = config.redis_url.as_ref().ok_or_else(|| anyhow::anyhow!("limits.backend = \"redis\" requires limits.redis_url"))?;
            let store = RedisLimitStore::connect(url.expose_secret()).await.map_err(|err| anyhow::anyhow!(connect_failure_message(&err)))?;
            let charge_tasks = store.tracker();
            Ok(LimitsWiring {
                limits: Some(Arc::new(Limits::new(rules, Arc::new(store), clock))),
                charge_tasks: Some(charge_tasks),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LimitsBackend;
    use secrecy::SecretString;

    #[tokio::test]
    async fn no_limits_table_builds_nothing() {
        let wiring = build_limits(None).await.expect("no table is fine");
        assert!(wiring.limits.is_none() && wiring.charge_tasks.is_none());
    }

    #[tokio::test]
    async fn a_table_with_only_empty_policies_builds_no_store() {
        let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::INFO);
        let wiring = build_limits(Some(&LimitsConfig::default())).await.expect("an empty table is fine");
        assert!(wiring.limits.is_none(), "D11: no store and no Redis connection");
        assert!(logs.text().contains("no limit is configured"), "{}", logs.text());
    }

    #[tokio::test]
    async fn the_memory_backend_builds_limits_without_a_tracker() {
        let config = LimitsConfig {
            principal_requests_per_minute: Some(5),
            ..LimitsConfig::default()
        };
        let wiring = build_limits(Some(&config)).await.expect("memory never fails");
        assert!(wiring.limits.is_some() && wiring.charge_tasks.is_none());
    }

    /// D17/Q14: a Redis that is down at boot fails the boot, and the error never shows the URL.
    #[tokio::test]
    async fn a_redis_that_is_down_at_boot_fails_the_boot_without_the_url() {
        let config = LimitsConfig {
            backend: LimitsBackend::Redis,
            redis_url: Some(SecretString::from("redis://:hunter2@127.0.0.1:1/2".to_owned())),
            principal_requests_per_minute: Some(5),
            ..LimitsConfig::default()
        };
        let err = build_limits(Some(&config)).await.err().expect("an unreachable Redis fails the boot");
        let text = format!("{err:#}");
        assert!(text.contains("did not connect at boot"), "{text}");
        assert!(!text.contains("hunter2"), "the password never reaches the error: {text}");
    }

    /// Carry (a): redis error text can hold the URL and the password. The message names the
    /// error kind only, whatever the error text says.
    #[test]
    fn the_connect_failure_message_never_holds_the_error_text() {
        let err = ::redis::RedisError::from((::redis::ErrorKind::Io, "bad url", "redis://:hunter2@10.0.0.9:6379/2".to_owned()));
        let text = connect_failure_message(&err);
        assert!(text.contains("did not connect at boot") && text.contains("kind=Io"), "{text}");
        assert!(!text.contains("hunter2") && !text.contains("10.0.0.9"), "{text}");
    }
}
