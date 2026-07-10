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
    pub authz: AuthzConfig,
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

/// Cedar authorization config (SMA-444 Task 21, spec §7/§11) — mirrors `AuthnConfig`'s
/// shape/style: the in-process [`PolicySnapshot`](crate::adapters::authz::PolicySnapshot)
/// staleness bound (`policy_cache_ttl_secs`), the entity-slice cache TTL
/// (`slice_cache_ttl_secs`), the decision-cache TTL (`decision_cache_ttl_secs`), the
/// background-reload poll cadence (`refresh_interval_secs`), the cache backend (`cache`,
/// mirroring `authn.jwks_cache`), the Task 20 tenancy-enforcement toggle
/// (`enforce_tenancy`), and the cold-start platform-admin seed list (`bootstrap_admins` —
/// this task only defines + validates it; Task 21b consumes it for JIT seeding). Unlike
/// `AuthnConfig.issuers`, every field here HAS a sensible default (see `AuthzDefaults`
/// below), so an absent `[authz]` block entirely is valid config.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AuthzConfig {
    pub enforce_tenancy: bool,
    pub policy_cache_ttl_secs: u64,
    pub slice_cache_ttl_secs: u64,
    pub decision_cache_ttl_secs: u64,
    pub refresh_interval_secs: u64,
    pub cache: AuthzCacheConfig,
    pub bootstrap_admins: Vec<BootstrapAdmin>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AuthzCacheConfig {
    pub backend: AuthzCacheBackend,
    pub redis_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthzCacheBackend {
    Memory,
    Redis,
}

/// A cold-start platform-administrator seed entry (spec §11): the `(issuer, subject)` pair
/// identifying an external identity that should be JIT-granted `platform_admin`@`Root` the
/// first time it authenticates (Task 21b). `issuer` is validated the same way as
/// `authn.issuers[].issuer` (`Issuer::parse`, a valid absolute `https` URL); `subject` must be
/// non-empty (it is the IdP's opaque `sub` claim, never validated beyond that here).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct BootstrapAdmin {
    pub issuer: String,
    pub subject: String,
}

// Only the fields that HAVE a default. `database_url` and `authn.issuers` are
// intentionally absent so a missing value is a hard error at load time.
#[derive(Serialize)]
struct Defaults {
    http_addr: SocketAddr,
    grpc_addr: SocketAddr,
    log_level: String,
    authn: AuthnDefaults,
    authz: AuthzDefaults,
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

// Mirrors `AuthzConfig` field-for-field — unlike `AuthnDefaults`, nothing is omitted: every
// `AuthzConfig` field has a sensible default (see `AuthzConfig`'s doc).
#[derive(Serialize)]
struct AuthzDefaults {
    enforce_tenancy: bool,
    policy_cache_ttl_secs: u64,
    slice_cache_ttl_secs: u64,
    decision_cache_ttl_secs: u64,
    refresh_interval_secs: u64,
    cache: AuthzCacheConfig,
    bootstrap_admins: Vec<BootstrapAdmin>,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults {
            http_addr: "0.0.0.0:8080".parse().expect("valid addr"),
            grpc_addr: "0.0.0.0:9090".parse().expect("valid addr"),
            log_level: "info".to_string(),
            authn: AuthnDefaults::default(),
            authz: AuthzDefaults::default(),
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

impl Default for AuthzDefaults {
    fn default() -> Self {
        AuthzDefaults {
            enforce_tenancy: true,
            policy_cache_ttl_secs: 30,
            slice_cache_ttl_secs: 60,
            decision_cache_ttl_secs: 30,
            refresh_interval_secs: 1,
            cache: AuthzCacheConfig {
                backend: AuthzCacheBackend::Memory,
                redis_url: None,
            },
            bootstrap_admins: Vec::new(),
        }
    }
}

// `AuthzConfig` gets a `Default` too (unlike `AuthnConfig`, which can't sensibly have one —
// `issuers` has no default) so callers that build an `IamConfig` by hand (test support, not
// through `figment`) can write `authz: AuthzConfig::default()` rather than repeat every field.
// Delegates to `AuthzDefaults` so the two never drift apart.
impl Default for AuthzConfig {
    fn default() -> Self {
        let d = AuthzDefaults::default();
        AuthzConfig {
            enforce_tenancy: d.enforce_tenancy,
            policy_cache_ttl_secs: d.policy_cache_ttl_secs,
            slice_cache_ttl_secs: d.slice_cache_ttl_secs,
            decision_cache_ttl_secs: d.decision_cache_ttl_secs,
            refresh_interval_secs: d.refresh_interval_secs,
            cache: d.cache,
            bootstrap_admins: d.bootstrap_admins,
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

    /// Boot-time validation beyond what serde/figment structurally enforce (spec §6.4/§11):
    /// at least one issuer, issuers unique (compared on the TRIMMED string, so a padded
    /// duplicate is still caught), each issuer string carries no leading/trailing
    /// whitespace of its own (padding is never valid config, even when unique), each
    /// issuer's `audiences` non-empty, each issuer string a valid absolute `https` URL
    /// (`Issuer::parse`), a `redis` JWKS cache backend has `redis_url` configured, and
    /// `jwks_ttl_secs` is non-zero (a zero TTL breaks both cache backends). Mirrors that same
    /// posture for `[authz]`: a `redis` cache backend has `redis_url` configured, every one of
    /// the four `*_secs` TTL/interval fields is non-zero, `refresh_interval_secs` is at most
    /// `policy_cache_ttl_secs` (else the snapshot's background reload poll could fire less
    /// often than its own staleness bound expects, letting evaluated-policy freshness
    /// overshoot the TTL the rest of the system assumes it's bounded by), and every
    /// `bootstrap_admins` entry (if any — the list itself is allowed to be empty) has a valid
    /// `https` issuer and a non-empty subject.
    pub fn validate(&self) -> Result<(), String> {
        if self.authn.issuers.is_empty() {
            return Err("authn.issuers must contain at least one issuer".to_string());
        }

        let mut seen = HashSet::with_capacity(self.authn.issuers.len());
        for issuer_cfg in &self.authn.issuers {
            let raw = issuer_cfg.issuer.as_str();
            let trimmed = raw.trim();
            if !seen.insert(trimmed) {
                return Err(format!("authn.issuers contains a duplicate issuer (after trimming whitespace): {trimmed}"));
            }
            if raw != trimmed {
                return Err(format!("authn.issuers[{trimmed}] has leading/trailing whitespace, which is never valid config: {raw:?}"));
            }
            if issuer_cfg.audiences.is_empty() {
                return Err(format!("authn.issuers[{trimmed}].audiences must not be empty"));
            }
            if let Err(e) = Issuer::parse(&issuer_cfg.issuer) {
                return Err(format!("authn.issuers[{trimmed}] is not a valid issuer: {e}"));
            }
        }

        if self.authn.jwks_cache.backend == JwksCacheBackend::Redis && self.authn.jwks_cache.redis_url.is_none() {
            return Err("authn.jwks_cache.backend = \"redis\" requires authn.jwks_cache.redis_url".to_string());
        }

        // `jwks_ttl_secs = 0` is broken with EITHER backend: redis `SET EX 0` is a command
        // error (every JWKS put fails -> permanent Unavailable), and the memory cache
        // treats every entry as already expired (never fresh + refresh cooldown -> requests
        // inside the cooldown window fail Unavailable). Reject at boot instead.
        if self.authn.jwks_ttl_secs == 0 {
            return Err("authn.jwks_ttl_secs must be at least 1 (0 disables JWKS caching and breaks both cache backends)".to_string());
        }

        if self.authz.cache.backend == AuthzCacheBackend::Redis && self.authz.cache.redis_url.is_none() {
            return Err("authz.cache.backend = \"redis\" requires authz.cache.redis_url".to_string());
        }

        // Every authz TTL/interval is a divisor of the system's staleness/availability
        // behavior (spec §7/§11) — a zero value breaks the corresponding cache/reload loop
        // the same way `authn.jwks_ttl_secs = 0` does above. Reject all four at boot.
        for (name, secs) in [
            ("authz.policy_cache_ttl_secs", self.authz.policy_cache_ttl_secs),
            ("authz.slice_cache_ttl_secs", self.authz.slice_cache_ttl_secs),
            ("authz.decision_cache_ttl_secs", self.authz.decision_cache_ttl_secs),
            ("authz.refresh_interval_secs", self.authz.refresh_interval_secs),
        ] {
            if secs == 0 {
                return Err(format!("{name} must be at least 1 (0 breaks the corresponding authz cache/reload loop)"));
            }
        }

        // The snapshot's background reload poll must fire at least as often as its own
        // staleness bound: a `refresh_interval_secs` GREATER than `policy_cache_ttl_secs`
        // would let the in-process compiled-policy snapshot go stale for longer than the TTL
        // the rest of the system (decision cache keys, freshness guarantees) assumes it's
        // bounded by. Equal is fine (the poll fires exactly at the TTL boundary).
        if self.authz.refresh_interval_secs > self.authz.policy_cache_ttl_secs {
            return Err(format!(
                "authz.refresh_interval_secs ({}) must be <= authz.policy_cache_ttl_secs ({}): a slower reload poll than the staleness TTL lets policy freshness overshoot the TTL bound",
                self.authz.refresh_interval_secs, self.authz.policy_cache_ttl_secs
            ));
        }

        // `bootstrap_admins` is allowed to be EMPTY (a fresh deployment with no platform
        // administrator boots fine — `main` logs a warning instead, spec §11) — only
        // non-empty entries are validated: a valid absolute `https` issuer (mirrors
        // `authn.issuers[].issuer`) and a non-empty (non-whitespace-only) subject.
        for admin in &self.authz.bootstrap_admins {
            if let Err(e) = Issuer::parse(&admin.issuer) {
                return Err(format!("authz.bootstrap_admins[issuer={:?}] is not a valid issuer: {e}", admin.issuer));
            }
            if admin.subject.trim().is_empty() {
                return Err(format!("authz.bootstrap_admins[issuer={:?}] has an empty subject", admin.issuer));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
// `figment::Jail::expect_with` fixes its closure's `Err` type to `figment::Error`
// (~208B) — not something callers control, so the size lint is allowed here, scoped
// to this test module's Jail-based tests, rather than reshaped away.
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
    fn validate_rejects_a_padded_issuer() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [[authn.issuers]]
                    issuer = " https://idp.example.com/realms/acme"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected a padded issuer to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_duplicate_issuers_differing_only_by_padding() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = ["paigasus"]

                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme "
                    audiences = ["other"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected two issuers differing only by padding to fail validation as duplicates");
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

    #[test]
    fn validate_rejects_zero_jwks_ttl() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [authn]
                    jwks_ttl_secs = 0

                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected jwks_ttl_secs = 0 to fail validation");
            Ok(())
        });
    }

    // --- SMA-444 Task 21: `[authz]` config -------------------------------------------------

    #[test]
    fn authz_defaults_land_with_no_authz_block_at_all() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("iam.toml", minimal_issuer_toml())?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.authz.enforce_tenancy, "enforce_tenancy must default to true");
            assert_eq!(cfg.authz.policy_cache_ttl_secs, 30);
            assert_eq!(cfg.authz.slice_cache_ttl_secs, 60);
            assert_eq!(cfg.authz.decision_cache_ttl_secs, 30);
            assert_eq!(cfg.authz.refresh_interval_secs, 1);
            assert_eq!(cfg.authz.cache.backend, AuthzCacheBackend::Memory);
            assert_eq!(cfg.authz.cache.redis_url, None);
            assert!(cfg.authz.bootstrap_admins.is_empty(), "bootstrap_admins should default to empty");
            assert!(cfg.validate().is_ok(), "authz defaults alone should pass validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_authz_redis_backend_without_a_url() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [authz.cache]
                    backend = "redis"

                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected an authz redis backend without redis_url to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_a_zero_authz_ttl() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [authz]
                    policy_cache_ttl_secs = 0

                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected authz.policy_cache_ttl_secs = 0 to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_refresh_interval_exceeding_policy_cache_ttl() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [authz]
                    policy_cache_ttl_secs = 10
                    refresh_interval_secs = 20

                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected authz.refresh_interval_secs > authz.policy_cache_ttl_secs to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_accepts_refresh_interval_equal_to_policy_cache_ttl() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [authz]
                    policy_cache_ttl_secs = 10
                    refresh_interval_secs = 10

                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_ok(), "expected authz.refresh_interval_secs == authz.policy_cache_ttl_secs to pass validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_a_non_https_bootstrap_admin_issuer() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [[authz.bootstrap_admins]]
                    issuer = "http://idp.example.com/realms/acme"
                    subject = "admin-sub"

                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected a non-https bootstrap_admins issuer to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_an_empty_bootstrap_admin_subject() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [[authz.bootstrap_admins]]
                    issuer = "https://idp.example.com/realms/acme"
                    subject = ""

                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected an empty bootstrap_admins subject to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_accepts_a_full_valid_authz_block() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                r#"
                    [authz]
                    enforce_tenancy = false
                    policy_cache_ttl_secs = 15
                    slice_cache_ttl_secs = 45
                    decision_cache_ttl_secs = 20
                    refresh_interval_secs = 2

                    [authz.cache]
                    backend = "redis"
                    redis_url = "redis://localhost:6379"

                    [[authz.bootstrap_admins]]
                    issuer = "https://idp.example.com/realms/acme"
                    subject = "platform-admin-sub"

                    [[authn.issuers]]
                    issuer = "https://idp.example.com/realms/acme"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(!cfg.authz.enforce_tenancy);
            assert_eq!(cfg.authz.policy_cache_ttl_secs, 15);
            assert_eq!(cfg.authz.slice_cache_ttl_secs, 45);
            assert_eq!(cfg.authz.decision_cache_ttl_secs, 20);
            assert_eq!(cfg.authz.refresh_interval_secs, 2);
            assert_eq!(cfg.authz.cache.backend, AuthzCacheBackend::Redis);
            assert_eq!(cfg.authz.cache.redis_url.as_deref(), Some("redis://localhost:6379"));
            assert_eq!(cfg.authz.bootstrap_admins.len(), 1);
            assert_eq!(cfg.authz.bootstrap_admins[0].issuer, "https://idp.example.com/realms/acme");
            assert_eq!(cfg.authz.bootstrap_admins[0].subject, "platform-admin-sub");
            assert!(cfg.validate().is_ok(), "expected a fully-populated, valid [authz] block to pass validation");
            Ok(())
        });
    }
}
