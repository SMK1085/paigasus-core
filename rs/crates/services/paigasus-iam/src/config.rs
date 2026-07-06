// SPDX-License-Identifier: Apache-2.0

//! Service configuration via figment: built-in defaults < `iam.toml` < `IAM_*` env.

use figment::providers::{Env, Format, Serialized, Toml};
use figment::{Figment, error::Error as FigmentError};
use paigasus_iam_core::Issuer;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::SocketAddr;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct IamConfig {
    pub http_addr: SocketAddr,
    pub grpc_addr: SocketAddr,
    pub database_url: String,
    pub log_level: String,
    pub authn: AuthnConfig,
}

/// BYO-IdP OIDC authentication config (spec §6.4). `issuers` is intentionally left
/// without a default (see `AuthnDefaults` below) — same "hard error when missing"
/// treatment as `IamConfig::database_url`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AuthnConfig {
    pub leeway_secs: u64,
    pub http_timeout_secs: u64,
    pub jwks_ttl_secs: u64,
    pub jwks_refresh_cooldown_secs: u64,
    pub max_token_bytes: usize,
    /// TEST-ONLY escape hatch for self-signed IdP TLS: when `true`, certificate
    /// verification is DISABLED for the discovery/JWKS fetches (`reqwest`'s
    /// `danger_accept_invalid_certs`). NEVER enable this in production — it lets any
    /// on-path attacker serve a forged JWKS, which is a full authentication bypass.
    /// Exists solely so the integration suites (in-process mock IdP, Keycloak-in-Docker,
    /// SMA-443 Tasks 10/13) can fetch from HTTPS endpoints with self-signed dev certs.
    #[serde(default)]
    pub accept_invalid_tls: bool,
    pub jwks_cache: JwksCacheConfig,
    pub issuers: Vec<IssuerConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct JwksCacheConfig {
    pub backend: JwksCacheBackend,
    pub redis_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JwksCacheBackend {
    Memory,
    Redis,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssuerConfig {
    pub issuer: String,
    pub audiences: Vec<String>,
    #[serde(default = "default_jit_provisioning")]
    pub jit_provisioning: bool,
}

fn default_jit_provisioning() -> bool {
    true
}

// Only the fields that HAVE a default. `database_url` and `authn.issuers` are
// intentionally absent so a missing value is a hard error at load time.
#[derive(Serialize)]
struct Defaults {
    http_addr: SocketAddr,
    grpc_addr: SocketAddr,
    log_level: String,
    authn: AuthnDefaults,
}

// Mirrors `AuthnConfig` minus `issuers` — deliberately absent, see `AuthnConfig` doc.
#[derive(Serialize)]
struct AuthnDefaults {
    leeway_secs: u64,
    http_timeout_secs: u64,
    jwks_ttl_secs: u64,
    jwks_refresh_cooldown_secs: u64,
    max_token_bytes: usize,
    accept_invalid_tls: bool,
    jwks_cache: JwksCacheConfig,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults {
            http_addr: "0.0.0.0:8080".parse().expect("valid addr"),
            grpc_addr: "0.0.0.0:9090".parse().expect("valid addr"),
            log_level: "info".to_string(),
            authn: AuthnDefaults::default(),
        }
    }
}

impl Default for AuthnDefaults {
    fn default() -> Self {
        AuthnDefaults {
            leeway_secs: 60,
            http_timeout_secs: 10,
            jwks_ttl_secs: 3600,
            jwks_refresh_cooldown_secs: 30,
            max_token_bytes: 16384,
            accept_invalid_tls: false,
            jwks_cache: JwksCacheConfig {
                backend: JwksCacheBackend::Memory,
                redis_url: None,
            },
        }
    }
}

impl IamConfig {
    #[must_use]
    pub fn figment() -> Figment {
        Figment::from(Serialized::defaults(Defaults::default())).merge(Toml::file("iam.toml")).merge(Env::prefixed("IAM_"))
    }

    // `figment::Error` is a large enum (~208B); allow the size lint narrowly rather than
    // reshape the public signature the brief specifies (`main` calls this directly).
    #[allow(clippy::result_large_err)]
    pub fn load() -> Result<Self, FigmentError> {
        Self::figment().extract()
    }

    /// Boot-time validation beyond what serde/figment structurally enforce (spec §6.4):
    /// at least one issuer, issuers unique, each issuer's `audiences` non-empty, each
    /// issuer string a valid absolute `https` URL (`Issuer::parse`), and a `redis` JWKS
    /// cache backend has `redis_url` configured.
    pub fn validate(&self) -> Result<(), String> {
        if self.authn.issuers.is_empty() {
            return Err("authn.issuers must contain at least one issuer".to_string());
        }

        let mut seen = HashSet::with_capacity(self.authn.issuers.len());
        for issuer_cfg in &self.authn.issuers {
            if !seen.insert(issuer_cfg.issuer.as_str()) {
                return Err(format!("authn.issuers contains a duplicate issuer: {}", issuer_cfg.issuer));
            }
            if issuer_cfg.audiences.is_empty() {
                return Err(format!("authn.issuers[{}].audiences must not be empty", issuer_cfg.issuer));
            }
            if let Err(e) = Issuer::parse(&issuer_cfg.issuer) {
                return Err(format!("authn.issuers[{}] is not a valid issuer: {e}", issuer_cfg.issuer));
            }
        }

        if self.authn.jwks_cache.backend == JwksCacheBackend::Redis && self.authn.jwks_cache.redis_url.is_none() {
            return Err("authn.jwks_cache.backend = \"redis\" requires authn.jwks_cache.redis_url".to_string());
        }

        Ok(())
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
            jail.create_file("iam.toml", minimal_issuer_toml())?;
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

    fn minimal_issuer_toml() -> &'static str {
        r#"
            [[authn.issuers]]
            issuer = "https://idp.example.com/realms/acme"
            audiences = ["paigasus"]
        "#
    }

    #[test]
    fn authn_defaults_land_with_a_minimal_issuer() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("iam.toml", minimal_issuer_toml())?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert_eq!(cfg.authn.leeway_secs, 60);
            assert_eq!(cfg.authn.http_timeout_secs, 10);
            assert_eq!(cfg.authn.jwks_ttl_secs, 3600);
            assert_eq!(cfg.authn.jwks_refresh_cooldown_secs, 30);
            assert_eq!(cfg.authn.max_token_bytes, 16384);
            assert!(!cfg.authn.accept_invalid_tls, "accept_invalid_tls must default to false (test-only escape)");
            assert_eq!(cfg.authn.jwks_cache.backend, JwksCacheBackend::Memory);
            assert_eq!(cfg.authn.jwks_cache.redis_url, None);
            assert_eq!(cfg.authn.issuers.len(), 1);
            assert_eq!(cfg.authn.issuers[0].issuer, "https://idp.example.com/realms/acme");
            assert_eq!(cfg.authn.issuers[0].audiences, vec!["paigasus".to_string()]);
            assert!(cfg.authn.issuers[0].jit_provisioning, "jit_provisioning should default to true");
            assert!(cfg.validate().is_ok(), "a single valid issuer should pass validation");
            Ok(())
        });
    }

    #[test]
    fn missing_issuers_is_a_load_error() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            let result = IamConfig::figment().extract::<IamConfig>();
            assert!(result.is_err(), "expected missing authn.issuers to error");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_an_empty_issuer_list() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [authn]
                    issuers = []
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected an empty issuer list to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_duplicate_issuers() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = ["paigasus"]

                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = ["other"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected a duplicate issuer to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_empty_audiences() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = []
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected empty audiences to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_a_non_https_issuer() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [[authn.issuers]]
                    issuer = "http://idp.example.com/realms/acme"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected a non-https issuer to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_redis_backend_without_a_url() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [authn.jwks_cache]
                    backend = "redis"

                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected a redis backend without redis_url to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_accepts_redis_backend_with_a_url() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [authn.jwks_cache]
                    backend = "redis"
                    redis_url = "redis://localhost:6379"

                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert_eq!(cfg.authn.jwks_cache.backend, JwksCacheBackend::Redis);
            assert_eq!(cfg.authn.jwks_cache.redis_url.as_deref(), Some("redis://localhost:6379"));
            assert!(cfg.validate().is_ok(), "expected a redis backend with redis_url to pass validation");
            Ok(())
        });
    }
}
