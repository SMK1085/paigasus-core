// SPDX-License-Identifier: Apache-2.0

//! Layered configuration: defaults, then `agent-host.toml`, then `AGENT_HOST_*` variables.

use std::net::SocketAddr;
use std::time::Duration;

use figment::providers::{Env, Format, Serialized, Toml};
use figment::{Figment, error::Error as FigmentError};
use serde::{Deserialize, Serialize};

/// A connection URL that can hold a password. `Debug` and `Serialize` print a fixed placeholder.
/// Read the real value with [`RedactedUrl::as_str`].
#[derive(Clone, PartialEq, Eq)]
pub struct RedactedUrl(String);

impl RedactedUrl {
    /// The real URL, for a connection constructor. Never log the result.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for RedactedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RedactedUrl").field(&"<redacted>").finish()
    }
}

impl<'de> Deserialize<'de> for RedactedUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(RedactedUrl)
    }
}

impl Serialize for RedactedUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str("<redacted>")
    }
}

/// The service configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub http_addr: SocketAddr,
    /// The Postgres connection string. The agent host needs its own database.
    pub database_url: RedactedUrl,
    pub log_level: String,
    pub metrics: MetricsConfig,
    pub migration: MigrationConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MetricsConfig {
    pub enabled: bool,
    /// A separate metrics listener. Its port must differ from the port of `http_addr`.
    pub addr: Option<SocketAddr>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MigrationConfig {
    /// Seconds to wait for the migration advisory lock. Range 1 to 3600.
    pub lock_wait_secs: u64,
}

impl MigrationConfig {
    /// The configured wait as a `Duration`.
    #[must_use]
    pub fn lock_wait(&self) -> Duration {
        Duration::from_secs(self.lock_wait_secs)
    }
}

/// A configuration value that is not valid.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("metrics.addr must use a different port than http_addr")]
    MetricsPortClash,
    #[error("migration.lock_wait_secs must be between 1 and 3600 (got {0})")]
    LockWaitOutOfRange(u64),
}

/// The defaults. `database_url` has none: it is required.
#[derive(Serialize)]
struct Defaults {
    http_addr: &'static str,
    log_level: &'static str,
    metrics: MetricsDefaults,
    migration: MigrationConfigDefaults,
}

#[derive(Serialize)]
struct MetricsDefaults {
    enabled: bool,
}

#[derive(Serialize)]
struct MigrationConfigDefaults {
    lock_wait_secs: u64,
}

impl Config {
    /// The figment chain: defaults, `agent-host.toml`, then `AGENT_HOST_*` (nested split `__`).
    #[must_use]
    pub fn figment() -> Figment {
        Figment::from(Serialized::defaults(Defaults {
            http_addr: "0.0.0.0:8086",
            log_level: "info",
            metrics: MetricsDefaults { enabled: true },
            migration: MigrationConfigDefaults { lock_wait_secs: 120 },
        }))
        .merge(Toml::file("agent-host.toml"))
        .merge(Env::prefixed("AGENT_HOST_").split("__"))
    }

    // `figment::Error` is large. The signature stays as the brief specifies it.
    #[allow(clippy::result_large_err)]
    pub fn load() -> Result<Self, FigmentError> {
        Self::figment().extract()
    }

    /// Boot-time checks that serde does not make.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Compare the port only. Two addresses with one port still fail at bind time.
        if let Some(addr) = self.metrics.addr
            && addr.port() == self.http_addr.port()
        {
            return Err(ConfigError::MetricsPortClash);
        }
        if !(1..=3600).contains(&self.migration.lock_wait_secs) {
            return Err(ConfigError::LockWaitOutOfRange(self.migration.lock_wait_secs));
        }
        Ok(())
    }
}

// `figment::Jail::expect_with` fixes its closure error type to `figment::Error`.
#[cfg(test)]
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

    fn with_url() -> Config {
        Config::figment().extract().expect("config")
    }

    #[test]
    fn defaults_bind_the_container_port() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let cfg = with_url();
            assert_eq!(cfg.http_addr.to_string(), "0.0.0.0:8086");
            assert_eq!(cfg.log_level, "info");
            assert!(cfg.metrics.enabled);
            assert_eq!(cfg.metrics.addr, None);
            assert_eq!(cfg.migration.lock_wait_secs, 120);
            assert_eq!(cfg.database_url.as_str(), "postgres://u:p@localhost/db");
            Ok(())
        });
    }

    #[test]
    fn database_url_is_required() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            assert!(Config::load().is_err());
            Ok(())
        });
    }

    #[test]
    fn debug_redacts_the_database_url() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:s3cret@localhost/db");
            let cfg = with_url();
            assert!(!format!("{cfg:?}").contains("s3cret"));
            Ok(())
        });
    }

    #[test]
    fn the_toml_file_and_nested_env_keys_are_read() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.set_env("AGENT_HOST_METRICS__ADDR", "127.0.0.1:9100");
            jail.create_file("agent-host.toml", "log_level = \"debug\"\n")?;
            let cfg = Config::load()?;
            assert_eq!(cfg.log_level, "debug");
            assert_eq!(cfg.metrics.addr, Some("127.0.0.1:9100".parse().unwrap()));
            Ok(())
        });
    }

    #[test]
    fn metrics_addr_on_the_http_port_is_rejected() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.set_env("AGENT_HOST_METRICS__ADDR", "127.0.0.1:8086");
            let cfg = with_url();
            assert_eq!(cfg.validate(), Err(ConfigError::MetricsPortClash));
            Ok(())
        });
    }

    #[test]
    fn lock_wait_outside_1_to_3600_is_rejected() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let mut cfg = with_url();
            for (secs, ok) in [(0, false), (1, true), (3600, true), (3601, false)] {
                cfg.migration.lock_wait_secs = secs;
                assert_eq!(cfg.validate().is_ok(), ok, "lock_wait_secs = {secs}");
            }
            Ok(())
        });
    }
}
