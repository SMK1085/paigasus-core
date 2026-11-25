// SPDX-License-Identifier: Apache-2.0

//! Session history store adapters (SMA-753).

pub mod digest;
pub mod memory;
pub mod redis;

pub use self::redis::RedisSessionStore;
pub use digest::{HistoryKey, HmacHistoryDigester};
pub use memory::MemorySessionStore;

use std::sync::Arc;

use secrecy::ExposeSecret;
use tokio_util::task::TaskTracker;

use crate::application::tool_history::{ToolHistory, prime_tool_history_metrics};
use crate::config::{HistoryBackend, ToolHistoryConfig};
use crate::domain::decision_log::Entropy;
use crate::domain::limits::Clock;
use crate::domain::tool_history::SessionStore;

use self::redis::EvictionCheck;

/// What `main.rs` puts into `build_tool_policy` and `runtime::supervise`.
#[derive(Default)]
pub struct HistoryWiring {
    pub history: Option<Arc<ToolHistory>>,
    /// The settle and append tasks of the history, drained at shutdown.
    pub tasks: Option<TaskTracker>,
}

/// The boot error for a Redis that did not connect. It names the error kind only: redis error
/// text can carry the URL, a host or a password, and the boot error reaches the logs.
fn connect_failure_message(err: &::redis::RedisError) -> String {
    format!("the tool history Redis store did not connect at boot (redis error kind={:?})", err.kind())
}

/// The boot rule for `maxmemory-policy` (spec D11): `Evicts` fails the boot and names the policy,
/// `Unknown` logs one warning, `Safe` passes.
///
/// # Errors
///
/// A policy that can evict a history key.
fn apply_eviction_check(check: EvictionCheck) -> anyhow::Result<()> {
    match check {
        EvictionCheck::Safe => Ok(()),
        EvictionCheck::Evicts(policy) => {
            anyhow::bail!("the tool history Redis has maxmemory-policy {policy:?}, which can evict a history key: set maxmemory-policy to \"noeviction\" or use another Redis")
        }
        EvictionCheck::Unknown => {
            tracing::warn!("could not read maxmemory-policy of the tool history Redis (CONFIG GET is not allowed?): make sure it is \"noeviction\"");
            Ok(())
        }
    }
}

/// `[tool_history]` only counts with the tool policy on. With the tool policy off, nothing reads the
/// history, so nothing is built: no warning, no Redis connection, no eviction check (ruling T13-a).
#[must_use]
pub fn history_config_in_use(tool_policy_enabled: bool, config: Option<&ToolHistoryConfig>) -> Option<&ToolHistoryConfig> {
    if config.is_some() && !tool_policy_enabled {
        tracing::info!("[tool_history] is set, but the tool policy is off: the history is not built and not used");
        return None;
    }
    config
}

/// Build the tool history from `[tool_history]` (spec D10, D11, D12). Call it after
/// `paigasus_observability::init`: the metrics and the breaker need the installed recorder.
///
/// - No table: nothing.
/// - `memory`: a `MemorySessionStore`, with a warning (one replica only, for development).
/// - `redis`: `RedisSessionStore::connect`, eager. A Redis that is down fails the boot. Then the
///   eviction check: `Evicts(p)` fails the boot and names `p`; `Unknown` logs one warning.
///
/// # Errors
///
/// A Redis that does not connect, or a `maxmemory-policy` that can evict a history key.
pub async fn build_tool_history(config: Option<&ToolHistoryConfig>, clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>) -> anyhow::Result<HistoryWiring> {
    let Some(config) = config else {
        return Ok(HistoryWiring::default());
    };
    prime_tool_history_metrics();
    let store: Arc<dyn SessionStore> = match config.backend {
        HistoryBackend::Memory => {
            tracing::warn!("tool_history.backend = \"memory\": the memory backend is for development and one replica only; a second replica does not see this history (use backend = \"redis\")");
            Arc::new(MemorySessionStore::new())
        }
        HistoryBackend::Redis => {
            let url = config
                .redis_url
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("tool_history.backend = \"redis\" requires tool_history.redis_url"))?;
            let store = RedisSessionStore::connect(url.expose_secret()).await.map_err(|err| anyhow::anyhow!(connect_failure_message(&err)))?;
            apply_eviction_check(store.read_eviction_policy().await)?;
            Arc::new(store)
        }
    };
    let history = Arc::new(ToolHistory::new(store, clock, entropy, config.deadline()));
    let tasks = history.tasks();
    tracing::info!(backend = ?config.backend, deadline_ms = u64::try_from(config.deadline().as_millis()).unwrap_or(u64::MAX), "tool history enabled");
    Ok(HistoryWiring {
        history: Some(history),
        tasks: Some(tasks),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::decision_log::OsEntropy;
    use crate::domain::limits::SystemClock;
    use secrecy::SecretString;

    async fn build(config: Option<&ToolHistoryConfig>) -> anyhow::Result<HistoryWiring> {
        build_tool_history(config, Arc::new(SystemClock), Arc::new(OsEntropy)).await
    }

    #[tokio::test]
    async fn no_table_builds_nothing() {
        let wiring = build(None).await.expect("no table is fine");
        assert!(wiring.history.is_none() && wiring.tasks.is_none());
    }

    #[tokio::test]
    async fn the_memory_backend_builds_a_history_without_a_tracker_wait() {
        let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::INFO);
        let wiring = build(Some(&ToolHistoryConfig::default())).await.expect("memory never fails");
        assert!(wiring.history.is_some() && wiring.tasks.is_some());
        assert!(wiring.tasks.expect("tracker").is_empty(), "no task runs at boot");
        assert!(logs.text().contains("development and one replica only"), "{}", logs.text());
    }

    #[test]
    fn an_evicting_policy_fails_the_boot_and_names_it() {
        let err = apply_eviction_check(EvictionCheck::Evicts("allkeys-lru".to_owned())).expect_err("must fail");
        assert!(format!("{err:#}").contains("allkeys-lru"), "{err:#}");
    }

    #[test]
    fn an_unknown_policy_warns_once_and_passes() {
        let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
        apply_eviction_check(EvictionCheck::Unknown).expect("passes");
        assert_eq!(logs.text().matches("could not read maxmemory-policy").count(), 1, "{}", logs.text());
    }

    #[test]
    fn a_safe_policy_passes_silently() {
        let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
        apply_eviction_check(EvictionCheck::Safe).expect("passes");
        assert!(logs.text().is_empty(), "{}", logs.text());
    }

    /// T13-a: with the tool policy off, a down Redis cannot fail the boot: the config is not used.
    #[tokio::test]
    async fn the_tool_policy_off_builds_no_history_even_for_a_down_redis() {
        let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::INFO);
        let config = ToolHistoryConfig {
            backend: HistoryBackend::Redis,
            redis_url: Some(SecretString::from("redis://127.0.0.1:1/2".to_owned())),
            ..ToolHistoryConfig::default()
        };
        let used = history_config_in_use(false, Some(&config));
        assert!(used.is_none());
        let wiring = build(used).await.expect("nothing is built");
        assert!(wiring.history.is_none());
        assert!(logs.text().contains("the tool policy is off"), "{}", logs.text());
        assert!(history_config_in_use(true, Some(&config)).is_some());
        assert!(history_config_in_use(false, None).is_none());
    }

    #[tokio::test]
    async fn a_redis_that_is_down_fails_the_boot_without_the_url() {
        let config = ToolHistoryConfig {
            backend: HistoryBackend::Redis,
            redis_url: Some(SecretString::from("redis://:hunter2@127.0.0.1:1/2".to_owned())),
            ..ToolHistoryConfig::default()
        };
        let err = build(Some(&config)).await.err().expect("an unreachable Redis fails the boot");
        let text = format!("{err:#}");
        assert!(text.contains("did not connect at boot"), "{text}");
        assert!(!text.contains("hunter2") && !text.contains("127.0.0.1"), "{text}");
    }
}
