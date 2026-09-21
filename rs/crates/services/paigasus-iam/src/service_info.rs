// SPDX-License-Identifier: Apache-2.0

//! IAM's capability projection: which registered keys this build currently has ENABLED.
//!
//! `Capabilities` is a small value type projected out of `IamConfig` ONCE, at wiring time, and
//! then carried on `AppState`. Deliberately not `&IamConfig` on request state: `IamConfig`
//! transitively carries `RawPepper` and every `RedactedUrl`, so storing it would clone the
//! API-key pepper into every HTTP and gRPC worker.
//!
//! `enabled()` is a pure function of three booleans, plus two unconditional keys
//! (`iam.deadletters`, SMA-629; `iam.authn.grants`, SMA-633). That is what makes AC 3's central
//! assertion ("flip the flag, the key disappears, the siblings remain") an ordinary unit test
//! with no `AppState`, no Postgres and no Docker.

use paigasus_proto::paigasus::common::v1::{Capability, ServiceInfo};

use crate::config::IamConfig;

/// The bare service slug, matching the prefix of this service's own capability keys.
/// Advisory per the proto — a client must never use it as a cache key.
pub const SERVICE: &str = "iam";

/// This build's version. `env!` is evaluated in THIS crate, so it is `paigasus-iam`'s own
/// `Cargo.toml` version and nothing else's (AC 4). Every crate in the workspace is currently
/// `0.0.0` and release-plz is dormant, so this reports `0.0.0` until releases are cut.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The three capability toggles, projected out of `IamConfig` at wiring time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// `authz.admin_enabled` -> `iam.authz.cedar`.
    pub authz_admin: bool,
    /// `api_keys.management_enabled` -> `iam.apikeys`.
    pub apikeys_management: bool,
    /// `audit.query_enabled` -> `iam.audit`.
    pub audit_query: bool,
}

impl Capabilities {
    #[must_use]
    pub fn from_config(cfg: &IamConfig) -> Self {
        Capabilities {
            authz_admin: cfg.authz.admin_enabled,
            apikeys_management: cfg.api_keys.management_enabled,
            audit_query: cfg.audit.query_enabled,
        }
    }

    /// The registered capabilities this build currently has enabled. Pure — the unit under
    /// test for AC 3.
    ///
    /// `iam.deadletters` and `iam.authn.grants` are UNCONDITIONAL. `iam.deadletters` (SMA-629 D2):
    /// `OutboxService` is registered on gRPC and HTTP with no config switch, because a break-glass
    /// surface must not be disable-able (`adapters/grpc/dead_letters.rs:13-18`). The key therefore
    /// means "this build serves `OutboxService`". `iam.authn.grants` (SMA-633 D9): Introspect
    /// populates `role_grants`, and the population has no config switch either. Neither key has a
    /// field on `Capabilities`: a field that is always `true` would be a false degree of freedom.
    #[must_use]
    pub fn enabled(&self) -> Vec<Capability> {
        let mut caps = Vec::new();
        if self.authz_admin {
            caps.push(Capability::IamAuthzCedar);
        }
        if self.apikeys_management {
            caps.push(Capability::IamApikeys);
        }
        if self.audit_query {
            caps.push(Capability::IamAudit);
        }
        caps.push(Capability::IamDeadletters);
        // SMA-633 D9: unconditional, for the same reason as `iam.deadletters`. The population
        // has no config switch, and a `Capabilities` field that is always `true` would be a
        // false degree of freedom. The key means "this build populates `role_grants` in
        // `Introspect`".
        caps.push(Capability::IamAuthnGrants);
        caps
    }

    /// The descriptor both transports serve. Shared so the HTTP route and the gRPC RPC cannot
    /// drift (spec § 6.5 pins that they agree).
    #[must_use]
    pub fn descriptor(&self) -> ServiceInfo {
        paigasus_service_info::descriptor(SERVICE, VERSION, &self.enabled())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiKeyConfig, AuditConfig, AuthnConfig, AuthzConfig, IamConfig, JwksCacheBackend, JwksCacheConfig, MetricsConfig, MigrationConfig, OutboxConfig};
    use paigasus_proto::paigasus::common::v1::Capability;
    use std::collections::HashSet;

    fn caps(authz_admin: bool, apikeys_management: bool, audit_query: bool) -> HashSet<Capability> {
        Capabilities {
            authz_admin,
            apikeys_management,
            audit_query,
        }
        .enabled()
        .into_iter()
        .collect()
    }

    #[test]
    fn all_enabled_advertises_every_iam_capability() {
        assert_eq!(
            caps(true, true, true),
            HashSet::from([
                Capability::IamAuthzCedar,
                Capability::IamApikeys,
                Capability::IamAudit,
                Capability::IamDeadletters,
                Capability::IamAuthnGrants
            ])
        );
    }

    /// SMA-629 D2 / SMA-633 D9: neither OutboxService nor Introspect's `role_grants` population
    /// has a config switch, so every flag off still advertises both unconditional keys.
    #[test]
    fn all_flags_off_advertises_only_the_unconditional_keys() {
        assert_eq!(caps(false, false, false), HashSet::from([Capability::IamDeadletters, Capability::IamAuthnGrants]));
    }

    /// AC 3's central assertion. Asserting only "the key is absent" would pass against an
    /// implementation returning an empty list unconditionally, so every case ALSO asserts the
    /// siblings survive.
    #[test]
    fn disabling_one_flag_removes_exactly_its_key() {
        assert_eq!(
            caps(false, true, true),
            HashSet::from([Capability::IamApikeys, Capability::IamAudit, Capability::IamDeadletters, Capability::IamAuthnGrants])
        );
        assert_eq!(
            caps(true, false, true),
            HashSet::from([Capability::IamAuthzCedar, Capability::IamAudit, Capability::IamDeadletters, Capability::IamAuthnGrants])
        );
        assert_eq!(
            caps(true, true, false),
            HashSet::from([Capability::IamAuthzCedar, Capability::IamApikeys, Capability::IamDeadletters, Capability::IamAuthnGrants])
        );
    }

    /// R3: the real risk surface is combinations, not single flags. All 8 are cheap here
    /// because this is a pure function. Both unconditional keys are in EVERY combination.
    #[test]
    fn every_combination_advertises_exactly_its_enabled_keys() {
        for authz in [false, true] {
            for apikeys in [false, true] {
                for audit in [false, true] {
                    let got = caps(authz, apikeys, audit);
                    assert_eq!(got.contains(&Capability::IamAuthzCedar), authz);
                    assert_eq!(got.contains(&Capability::IamApikeys), apikeys);
                    assert_eq!(got.contains(&Capability::IamAudit), audit);
                    assert!(got.contains(&Capability::IamDeadletters), "iam.deadletters must be advertised for ({authz}, {apikeys}, {audit})");
                    assert!(got.contains(&Capability::IamAuthnGrants), "iam.authn.grants must be advertised for ({authz}, {apikeys}, {audit})");
                }
            }
        }
    }

    /// `IamConfig` has no `Default` (`database_url`/`authn.issuers` have no sensible default),
    /// so this builds the config the same way `tests/support/mod.rs::test_config_with` does —
    /// by hand, with the four sub-configs that DO have `Default` delegated to it and `authn`
    /// filled in with an empty issuer list (irrelevant here; only the three flags matter).
    fn iam_config_with_empty_authn(authz: AuthzConfig, api_keys: ApiKeyConfig, audit: AuditConfig) -> IamConfig {
        IamConfig {
            http_addr: "127.0.0.1:0".parse().expect("valid addr"),
            grpc_addr: "127.0.0.1:0".parse().expect("valid addr"),
            database_url: "unused-in-tests".into(),
            log_level: "info".to_string(),
            authn: AuthnConfig {
                leeway_secs: 60,
                http_timeout_secs: 5,
                jwks_ttl_secs: 3600,
                jwks_refresh_cooldown_secs: 30,
                max_token_bytes: 16384,
                accept_invalid_tls: true,
                extra_ca_bundle_path: None,
                jwks_cache: JwksCacheConfig {
                    backend: JwksCacheBackend::Memory,
                    redis_url: None,
                },
                issuers: Vec::new(),
            },
            authz,
            api_keys,
            audit,
            outbox: OutboxConfig::default(),
            metrics: MetricsConfig::default(),
            migration: MigrationConfig::default(),
        }
    }

    #[test]
    fn the_projection_reads_the_three_config_flags() {
        let cfg = iam_config_with_empty_authn(
            AuthzConfig {
                admin_enabled: false,
                ..AuthzConfig::default()
            },
            ApiKeyConfig {
                management_enabled: true,
                ..ApiKeyConfig::default()
            },
            AuditConfig {
                query_enabled: false,
                ..AuditConfig::default()
            },
        );
        assert_eq!(
            Capabilities::from_config(&cfg),
            Capabilities {
                authz_admin: false,
                apikeys_management: true,
                audit_query: false
            }
        );
    }

    #[test]
    fn the_descriptor_names_this_service_and_this_crates_build_version() {
        let info = Capabilities {
            authz_admin: true,
            apikeys_management: true,
            audit_query: true,
        }
        .descriptor();
        assert_eq!(info.service, "iam");
        // `env!` is expanded HERE, in the test, NOT read back from the module's `VERSION` const.
        // That is the whole point: replacing the const with a literal (`"1.0.0"`) makes the
        // served value diverge from this crate's real `Cargo.toml` version and fails the
        // assertion, where comparing against the const itself would pass trivially.
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        // SemVer-shaped: `major.minor.patch`, all numeric, ignoring any pre-release/build
        // suffix. An empty or malformed version fails loudly instead of being served.
        let core = info.version.split(['-', '+']).next().expect("a version always has a core");
        let parts: Vec<&str> = core.split('.').collect();
        assert_eq!(parts.len(), 3, "version core must be major.minor.patch: {}", info.version);
        assert!(
            parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
            "version core must be numeric: {}",
            info.version
        );
        // Still NOT proven while every crate is "0.0.0": that this is the SERVICE's version
        // rather than the shared library's. Both strings are identical today (spec § 6.4).
    }

    #[test]
    fn every_advertised_string_is_a_registered_capability_key() {
        let info = Capabilities {
            authz_admin: true,
            apikeys_management: true,
            audit_query: true,
        }
        .descriptor();
        for key in &info.capabilities {
            assert!(Capability::from_wire_key(key).is_some(), "{key} is not in the registry");
        }
    }
}
