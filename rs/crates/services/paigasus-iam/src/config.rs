// SPDX-License-Identifier: Apache-2.0

//! Service configuration via figment: built-in defaults < `iam.toml` < `IAM_*` env.

use figment::providers::{Env, Format, Serialized, Toml};
use figment::{Figment, error::Error as FigmentError};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct IamConfig {
    pub http_addr: SocketAddr,
    pub grpc_addr: SocketAddr,
    pub database_url: String,
    pub log_level: String,
}

// Only the fields that HAVE a default. `database_url` is intentionally absent so a
// missing value is a hard error at load time.
#[derive(Serialize)]
struct Defaults {
    http_addr: SocketAddr,
    grpc_addr: SocketAddr,
    log_level: String,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults {
            http_addr: "0.0.0.0:8080".parse().expect("valid addr"),
            grpc_addr: "0.0.0.0:9090".parse().expect("valid addr"),
            log_level: "info".to_string(),
        }
    }
}

impl IamConfig {
    #[must_use]
    pub fn figment() -> Figment {
        Figment::from(Serialized::defaults(Defaults::default())).merge(Toml::file("iam.toml")).merge(Env::prefixed("IAM_"))
    }

    // `load` isn't called yet — the temporary `main.rs` stub only builds the `Figment`
    // (Task 11 wires the composition root through `load`), so it's dead code in this
    // binary today. `figment::Error` is also a large enum (~208B); allow both narrowly
    // rather than reshape the public signature the brief specifies.
    #[allow(dead_code, clippy::result_large_err)]
    pub fn load() -> Result<Self, FigmentError> {
        Self::figment().extract()
    }
}

#[cfg(test)]
// `figment::Jail::expect_with` fixes its closure's `Err` type to `figment::Error`
// (~208B) — not something callers control, so the size lint is allowed here, scoped
// to this test module's two Jail tests, rather than reshaped away.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

    #[test]
    fn database_url_from_env_with_defaults() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert_eq!(cfg.database_url, "postgres://u:p@localhost/db");
            assert_eq!(cfg.http_addr.to_string(), "0.0.0.0:8080");
            assert_eq!(cfg.log_level, "info");
            Ok(())
        });
    }

    #[test]
    fn missing_database_url_is_an_error() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            let result = IamConfig::figment().extract::<IamConfig>();
            assert!(result.is_err(), "expected missing database_url to error");
            Ok(())
        });
    }
}
