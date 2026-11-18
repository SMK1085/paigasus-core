// SPDX-License-Identifier: Apache-2.0

//! The Redis connection of the decision log (SMA-755 § 5.6, A2). A task connects in a loop, so a Redis that
//! is down at boot does not block the boot. The writer and the relay read the connection from a `RedisSlot`.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use paigasus_observability::names;
use paigasus_redis::{BreakerMetrics, RedisHandle};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::watch;

/// The `role` label of the decision-log breaker series.
pub const DECISION_LOG_BREAKER_ROLE: &str = "decision_log";

const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// The breaker series of the decision log carry the gateway's names and `role="decision_log"`.
#[must_use]
pub fn breaker_metrics() -> BreakerMetrics {
    BreakerMetrics {
        state: names::GATEWAY_REDIS_BREAKER_STATE,
        transitions: names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL,
        role: DECISION_LOG_BREAKER_ROLE,
    }
}

/// A cell that holds the Redis handle once the connect loop has made it. It is empty until then.
#[derive(Debug, Clone, Default)]
pub struct RedisSlot(Arc<OnceLock<RedisHandle>>);

impl RedisSlot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The handle, or `None` while Redis is not connected.
    #[must_use]
    pub fn get(&self) -> Option<&RedisHandle> {
        self.0.get()
    }

    /// Put the handle in. The first handle stays; a second call has no effect.
    pub fn set(&self, handle: RedisHandle) {
        let _ = self.0.set(handle);
    }
}

/// Connect to Redis with a backoff from 1 s that doubles to 30 s, and put the handle into `slot`. It returns
/// when it has connected or when the shutdown signal comes. The log line holds the error kind and never the
/// error text, because that text can hold the URL.
pub async fn connect_loop(url: SecretString, slot: RedisSlot, mut shutdown: watch::Receiver<()>) {
    let mut backoff = BACKOFF_START;
    loop {
        let attempt = tokio::select! {
            r = paigasus_redis::connect(url.expose_secret(), breaker_metrics()) => r,
            _ = shutdown.changed() => return,
        };
        match attempt {
            Ok(handle) => {
                slot.set(handle);
                tracing::info!("the decision log connected to Redis");
                return;
            }
            Err(err) => {
                tracing::warn!(kind = ?err.kind(), retry_in_secs = backoff.as_secs(), "the decision log cannot connect to Redis");
            }
        }
        tokio::select! {
            () = tokio::time::sleep(backoff) => {}
            _ = shutdown.changed() => return,
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}
