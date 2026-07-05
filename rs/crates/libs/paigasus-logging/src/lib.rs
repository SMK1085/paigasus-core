// SPDX-License-Identifier: Apache-2.0

//! Structured JSON logging conventions shared by every Paigasus service.
//!
//! `init` installs a global JSON `tracing-subscriber` honoring `RUST_LOG`
//! (falling back to the caller-supplied default level, e.g. a service's own
//! config). Kept tiny and dependency-light so every service shares one log
//! shape (ADR-0005-adjacent; the first consumer is `paigasus-iam`).

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// The env-filter for logging: `RUST_LOG` if set, else `default_directive`. Pure so it is
/// unit-testable without touching the process-global subscriber.
#[must_use]
pub fn env_filter(default_directive: &str) -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_directive))
}

/// Install the global JSON tracing subscriber for `service`, defaulting the filter to
/// `default_level` unless `RUST_LOG` is set. Call once at process start; a second call is
/// a no-op-with-error (the global subscriber is already set).
pub fn init(service: &str, default_level: &str) {
    let _ = tracing_subscriber::registry()
        .with(env_filter(default_level))
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
        assert_eq!(env_filter("info").to_string(), "info");
    }

    #[test]
    fn env_filter_defaults_to_given_level_without_rust_log() {
        // SAFETY: single-threaded test; we remove RUST_LOG so the default branch runs.
        unsafe { std::env::remove_var("RUST_LOG") };
        assert_eq!(env_filter("debug").to_string(), "debug");
    }

    #[test]
    fn env_filter_rust_log_overrides_default() {
        // SAFETY: single-threaded test; we set then remove RUST_LOG to stay hermetic.
        unsafe { std::env::set_var("RUST_LOG", "warn") };
        assert_eq!(env_filter("info").to_string(), "warn");
        unsafe { std::env::remove_var("RUST_LOG") };
    }
}
