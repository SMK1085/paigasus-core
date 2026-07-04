// SPDX-License-Identifier: Apache-2.0

//! Structured JSON logging conventions shared by every Paigasus service.
//!
//! `init` installs a global JSON `tracing-subscriber` honoring `RUST_LOG`
//! (defaulting to `info`). Kept tiny and dependency-light so every service
//! shares one log shape (ADR-0005-adjacent; the first consumer is `paigasus-iam`).

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// The env-filter for logging: `RUST_LOG` if set, else `info`. Pure so it is unit-testable
/// without touching the process-global subscriber.
#[must_use]
pub fn env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Install the global JSON tracing subscriber for `service`. Call once at process start;
/// a second call is a no-op-with-error (the global subscriber is already set).
pub fn init(service: &str) {
    let _ = tracing_subscriber::registry()
        .with(env_filter())
        .with(fmt::layer().json().with_current_span(true).with_span_list(true))
        .try_init();
    tracing::info!(service, "logging initialized");
}

#[cfg(test)]
mod tests {
    use super::env_filter;

    #[test]
    fn env_filter_defaults_to_info_without_rust_log() {
        // SAFETY: single-threaded test; we remove RUST_LOG so the default branch runs.
        unsafe { std::env::remove_var("RUST_LOG") };
        assert_eq!(env_filter().to_string(), "info");
    }
}
