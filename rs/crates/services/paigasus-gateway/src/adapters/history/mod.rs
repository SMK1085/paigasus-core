// SPDX-License-Identifier: Apache-2.0

//! Session history store adapters (SMA-753).

pub mod memory;
pub mod redis;

pub use self::redis::RedisSessionStore;
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
            match store.read_eviction_policy().await {
                EvictionCheck::Safe => {}
                EvictionCheck::Evicts(policy) => {
                    anyhow::bail!("the tool history Redis has maxmemory-policy {policy:?}, which can evict a history key: set maxmemory-policy to \"noeviction\" or use another Redis");
                }
                EvictionCheck::Unknown => {
                    tracing::warn!("could not read maxmemory-policy of the tool history Redis (CONFIG GET is not allowed?): make sure it is \"noeviction\"");
                }
            }
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
