// SPDX-License-Identifier: Apache-2.0

//! Service configuration via figment: built-in defaults < `iam.toml` < `IAM_*` env.

use crate::adapters::api_keys::{Pepper, PepperConfigError};
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
    pub api_keys: ApiKeyConfig,
    pub audit: AuditConfig,
    pub outbox: OutboxConfig,
    pub metrics: MetricsConfig,
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

/// API-key issuance/validation config (SMA-445 Task 15, spec D12/§11) — mirrors `AuthzConfig`'s
/// shape/style directly above: every field HAS a sensible default (see `ApiKeyDefaults` below)
/// EXCEPT `pepper`, whose default (the empty string) is deliberately invalid — an operator MUST
/// configure a real one. Unlike `AuthnConfig.issuers` (which has no default at all and fails to
/// even extract), an absent/short `pepper` loads fine and is caught by `IamConfig::validate`
/// instead, because that's where the rest of this block's checks already live (`key_prefix`,
/// `introspect_cache`, `max_token_bytes`) and figment's defaults layer needs SOME string value
/// to seed a well-typed field.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ApiKeyConfig {
    /// The raw, still-`base64`-encoded pepper as configured (`[api_keys] pepper` /
    /// `IAM_API_KEYS__PEPPER`) — see [`RawPepper`]'s doc for why this isn't the decoded
    /// `adapters::api_keys::Pepper` directly. `#[serde(skip_serializing)]` so `IamConfig`'s
    /// derived `Serialize` (used by log/`readyz` config dumps) omits it entirely; `RawPepper`'s
    /// own hand-rolled `Debug` additionally redacts it for the derived `Debug` path. Call
    /// [`ApiKeyConfig::pepper`] to decode + validate it into the real key material.
    #[serde(skip_serializing)]
    pub pepper: RawPepper,
    pub key_prefix: String,
    pub max_token_bytes: usize,
    /// Unset = non-expiring until revoked (spec §11 example: `default_expiry_days = 365`).
    pub default_expiry_days: Option<u32>,
    pub last_used_throttle_secs: u64,
    pub introspect_cache: ApiKeyCacheConfig,
}

impl ApiKeyConfig {
    /// Decodes + validates the configured pepper into the actual key material
    /// `adapters::api_keys::HmacSecretHasher` is keyed by. Lazy — called at composition time
    /// (and by [`IamConfig::validate`] below) rather than eagerly at load, because figment's
    /// `Deserialize` only ever has the raw configured string to work with (see [`RawPepper`]).
    pub fn pepper(&self) -> Result<Pepper, PepperConfigError> {
        Pepper::from_config(&self.pepper.0)
    }

    /// Test/dev-only convenience: an [`ApiKeyConfig::default`] with `pepper` overridden to
    /// `pepper_b64` (raw, still-base64-encoded, exactly as `[api_keys] pepper` /
    /// `IAM_API_KEYS__PEPPER` would supply it) — for callers that build an `IamConfig` by hand
    /// (integration-test support, not through `figment`) and need [`ApiKeyConfig::pepper`] to
    /// actually succeed. SMA-445 Task 19: `AppState::new` now calls `pepper()` unconditionally,
    /// and `ApiKeyConfig::default()`'s pepper is deliberately the invalid empty string (see its
    /// own doc) — mirrors `Default for ApiKeyConfig`'s identical "hand-built test `IamConfig`"
    /// rationale. `RawPepper`'s inner field is private (redaction, its own doc), so this is the
    /// sole way for code outside this module to construct one carrying a real value.
    #[must_use]
    pub fn with_test_pepper(pepper_b64: impl Into<String>) -> Self {
        ApiKeyConfig {
            pepper: RawPepper(pepper_b64.into()),
            ..ApiKeyConfig::default()
        }
    }
}

/// The raw (still-`base64`-encoded, undecoded) HMAC pepper exactly as figment read it from
/// `iam.toml`/`IAM_API_KEYS__PEPPER` — a redacting newtype around a `String` (spec D12,
/// challenge M6). `IamConfig` derives `Debug`/`Serialize` because it's dumped in logs/`readyz`
/// (`main.rs`), so the configured secret must never round-trip through either: `Debug` is
/// hand-rolled to print a fixed placeholder (mirrors `adapters::api_keys::Pepper`'s own
/// redacted `Debug`, which this decodes into via [`ApiKeyConfig::pepper`]), and `Deserialize`
/// is hand-rolled to delegate straight to `String` so figment can still populate the REAL
/// value — only the outbound directions (`Debug`, and `Serialize` on the containing
/// `ApiKeyConfig::pepper` field, via `#[serde(skip_serializing)]`) are redacted.
/// Deliberately does NOT derive/implement `Serialize` itself: `ApiKeyConfig::pepper` never
/// serializes this type, so there's nothing to redact-in-place, and not having the impl at all
/// makes a future accidental removal of `#[serde(skip_serializing)]` a compile error instead of
/// a silent leak.
#[derive(Clone, PartialEq, Eq)]
pub struct RawPepper(String);

impl std::fmt::Debug for RawPepper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RawPepper").field(&"<redacted>").finish()
    }
}

impl<'de> Deserialize<'de> for RawPepper {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(RawPepper)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ApiKeyCacheConfig {
    pub backend: ApiKeyCacheBackend,
    pub redis_url: Option<String>,
    pub ttl_secs: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyCacheBackend {
    Memory,
    Redis,
}

/// Persistent denial-audit config (SMA-446 Slice A, Task A12) — the single knob wiring the
/// bounded, non-blocking denial-audit buffer (`adapters::authz::DenialAuditBuffer`) that
/// `AppState::new` composes into the `CedarAuthorizer`'s `AuditSink` and drains to Postgres.
/// Like `[authz]`/`[api_keys]`, every field HAS a sensible default (see `AuditDefaults`), so an
/// absent `[audit]` block entirely is valid config.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AuditConfig {
    /// Ring-buffer capacity for denial [`AuditEntry`](paigasus_iam_core::AuditEntry)s awaiting
    /// out-of-band persistence. When full, the OLDEST queued entry is dropped (favoring
    /// recency) and `DenialAuditBuffer::dropped()` is bumped — see that type's doc. Validated
    /// non-zero in [`IamConfig::validate`] (a `0` would make every push immediately evict
    /// itself); `DenialAuditBuffer::new` additionally clamps to `>= 1` as a belt-and-braces
    /// defense, but a misconfigured `0` is an operator error caught at boot, not silently
    /// corrected.
    pub denial_buffer_capacity: usize,
    /// Partition-maintenance + outcome-aware retention (SMA-467). Absent block → all defaults.
    #[serde(default)]
    pub retention: RetentionConfig,
    /// When an audit `query` supplies neither `from` nor `to`, apply this lookback so the read
    /// prunes to recent partitions instead of MergeAppend-scanning every leaf (SMA-467 §3.6).
    pub query_default_window_days: u32,
    /// Hard cap on any `from`/`to` span; an over-wide range is clamped to this.
    pub query_max_window_days: u32,
}

/// `[audit.retention]` (SMA-467) — the in-app partition-maintenance task's knobs. Like the rest of
/// `[audit]`, every field has a default so an absent block is valid config.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RetentionConfig {
    /// `false` → the maintenance task is NOT spawned at all (no create-ahead, no pruning). To pause
    /// only DELETIONS while keeping create-ahead healthy, leave this `true` and set the two
    /// `*_months` to 0 — see `main.rs`'s startup `warn` for the disabled path's default-pollution
    /// consequence.
    pub enabled: bool,
    /// Seconds between maintenance ticks (create-ahead + prune). Validated non-zero.
    pub interval_secs: u64,
    /// How many months ahead to pre-create leaf partitions. Validated `1..=24`.
    pub ahead_months: u32,
    /// Drop denied monthly leaves older than this. `0` = never drop denied.
    pub denied_months: u32,
    /// Drop committed monthly leaves older than this. `0` = never auto-drop committed (default;
    /// a non-zero value auto-deletes compliance rows and triggers a startup `warn`).
    pub committed_months: u32,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        RetentionConfig {
            enabled: true,
            interval_secs: 86_400,
            ahead_months: 1,
            denied_months: 3,
            committed_months: 0,
        }
    }
}

/// Outbox-relay config (SMA-446 Slice B, Task B9) — the knobs for
/// [`OutboxRelay`](crate::adapters::events::OutboxRelay), the background drain that turns
/// committed `event_outbox` rows (written by `PgOutbox::enqueue`, B2) into calls on the
/// injected `EventPublisher`. Like `[audit]`, every field HAS a sensible default (see
/// `OutboxDefaults`), so an absent `[outbox]` block entirely is valid config. Unlike `[audit]`,
/// this block ALSO carries an enable/disable toggle (`relay_enabled`): the relay is wired
/// directly in `main.rs` off a cloned `db` handle (not through `AppState`), so disabling it is a
/// pure boot-time no-spawn rather than a runtime knob on an already-composed service — see
/// `main.rs` for the `warn!` it emits instead when disabled (rows still accrue, undrained).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct OutboxConfig {
    /// When `false`, `main.rs` does not spawn the relay at all — `event_outbox` rows still
    /// accrue (written inside each triggering mutation's own transaction, unconditionally),
    /// they are just never drained until an operator re-enables this and restarts. Defaults to
    /// `true`: the relay is the intended steady-state behavior.
    pub relay_enabled: bool,
    /// How long the relay sleeps between drain ticks. Validated non-zero in
    /// [`IamConfig::validate`] (a zero interval would busy-loop the poll).
    pub poll_interval_secs: u64,
    /// Max rows locked (`FOR UPDATE SKIP LOCKED`) and processed per tick. Validated non-zero in
    /// [`IamConfig::validate`] (a zero batch size would make every tick drain nothing forever).
    pub batch_size: u64,
    /// Attempts (publish failures) before a row is parked (`parked = true`, excluded from
    /// future ticks). Validated non-zero in [`IamConfig::validate`] (a zero limit would park
    /// every row on its very first failed attempt, before ever getting a retry).
    pub max_attempts: u32,
}

/// `GET /metrics` (SMA-446 Unit 3, mirrors `paigasus-gateway::config::MetricsConfig` field-for-
/// field): whether the Prometheus recorder is installed at all, and where the route is served.
/// `addr: None` (the default) merges `/metrics` onto `http_addr` (the same port as the
/// tenancy/authn/authz HTTP surface); `Some(addr)` serves it on its OWN listener instead —
/// keeping operational metrics off a port that also serves application traffic. `enabled =
/// false` skips installing the recorder entirely (no route, no global `metrics`-facade
/// recorder in this process). Unlike the gateway's `MetricsConfig`, this derives
/// `PartialEq`/`Eq` — `IamConfig` itself derives both (no `SecretString`-carrying field blocks
/// it, unlike `GatewayConfig`), so every nested config type must too.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MetricsConfig {
    pub enabled: bool,
    pub addr: Option<SocketAddr>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        MetricsConfig { enabled: true, addr: None }
    }
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
    api_keys: ApiKeyDefaults,
    audit: AuditDefaults,
    outbox: OutboxDefaults,
    metrics: MetricsConfig,
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

// Mirrors `ApiKeyConfig` field-for-field, EXCEPT `pepper`'s TYPE: this struct only ever feeds
// figment's default LAYER (`Serialized::defaults`), never gets logged/dumped itself, so a
// plain (unredacted) `String` is fine here — figment still deserializes the eventual merged
// value into `ApiKeyConfig.pepper: RawPepper` regardless of which layer (default/toml/env)
// supplied it. The default is the empty string, which is deliberately INVALID
// (`Pepper::from_config("")` fails with `TooShort`) — see `ApiKeyConfig`'s doc for why that's
// caught at `validate()` rather than at figment extraction.
#[derive(Serialize)]
struct ApiKeyDefaults {
    pepper: String,
    key_prefix: String,
    max_token_bytes: usize,
    default_expiry_days: Option<u32>,
    last_used_throttle_secs: u64,
    introspect_cache: ApiKeyCacheConfig,
}

// Mirrors `AuditConfig` field-for-field. A 4096-entry denial buffer comfortably absorbs a
// denial burst between drain wakes without back-pressure while staying a modest bounded
// footprint (each queued entry is a handful of small strings). The SMA-467 `retention`/
// query-window defaults mirror the spec's steady-state values (see `RetentionConfig::default`
// and this struct's own `Default` below for the rationale of each).
#[derive(Serialize)]
struct AuditDefaults {
    denial_buffer_capacity: usize,
    retention: RetentionConfig,
    query_default_window_days: u32,
    query_max_window_days: u32,
}

// Mirrors `OutboxConfig` field-for-field. `poll_interval_secs = 5` / `batch_size = 100` /
// `max_attempts = 5` are the spec-example steady-state values (frequent enough to keep
// `event_outbox` drained close to real time, generous enough to absorb a burst without an
// oversized single transaction); `relay_enabled = true` because the relay is the intended
// steady-state behavior (see `OutboxConfig`'s doc for the disabled path).
#[derive(Serialize)]
struct OutboxDefaults {
    relay_enabled: bool,
    poll_interval_secs: u64,
    batch_size: u64,
    max_attempts: u32,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults {
            http_addr: "0.0.0.0:8080".parse().expect("valid addr"),
            grpc_addr: "0.0.0.0:9090".parse().expect("valid addr"),
            log_level: "info".to_string(),
            authn: AuthnDefaults::default(),
            authz: AuthzDefaults::default(),
            api_keys: ApiKeyDefaults::default(),
            audit: AuditDefaults::default(),
            outbox: OutboxDefaults::default(),
            metrics: MetricsConfig::default(),
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

// Defaults mirror the spec §11 example `iam.toml` block (`key_prefix = "pgs_sk_"`,
// `max_token_bytes = 512`, `last_used_throttle_secs = 60`, `introspect_cache` memory-backed
// with a 30s TTL — the same short TTL D5 gives as the introspection cache's rationale/default).
// `pepper` defaults to the empty string (see `ApiKeyDefaults`'s doc).
impl Default for ApiKeyDefaults {
    fn default() -> Self {
        ApiKeyDefaults {
            pepper: String::new(),
            key_prefix: "pgs_sk_".to_string(),
            max_token_bytes: 512,
            default_expiry_days: None,
            last_used_throttle_secs: 60,
            introspect_cache: ApiKeyCacheConfig {
                backend: ApiKeyCacheBackend::Memory,
                redis_url: None,
                ttl_secs: 30,
            },
        }
    }
}

impl Default for AuditDefaults {
    fn default() -> Self {
        AuditDefaults {
            denial_buffer_capacity: 4096,
            retention: RetentionConfig::default(),
            query_default_window_days: 90,
            query_max_window_days: 366,
        }
    }
}

impl Default for OutboxDefaults {
    fn default() -> Self {
        OutboxDefaults {
            relay_enabled: true,
            poll_interval_secs: 5,
            batch_size: 100,
            max_attempts: 5,
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

// `ApiKeyConfig` gets a `Default` too, same rationale as `AuthzConfig` above (hand-built test
// `IamConfig`s can write `api_keys: ApiKeyConfig::default()`). NOTE: the resulting `pepper` is
// the empty-string default — fine for tests that never call `validate()`/`pepper()`, but a
// test that DOES needs to override it explicitly (see `config::tests::valid_pepper_b64`).
impl Default for ApiKeyConfig {
    fn default() -> Self {
        let d = ApiKeyDefaults::default();
        ApiKeyConfig {
            pepper: RawPepper(d.pepper),
            key_prefix: d.key_prefix,
            max_token_bytes: d.max_token_bytes,
            default_expiry_days: d.default_expiry_days,
            last_used_throttle_secs: d.last_used_throttle_secs,
            introspect_cache: d.introspect_cache,
        }
    }
}

// `AuditConfig` gets a `Default` too, same rationale as `AuthzConfig`/`ApiKeyConfig` above
// (hand-built test `IamConfig`s can write `audit: AuditConfig::default()`). Delegates to
// `AuditDefaults` so the two never drift apart.
impl Default for AuditConfig {
    fn default() -> Self {
        let d = AuditDefaults::default();
        AuditConfig {
            denial_buffer_capacity: d.denial_buffer_capacity,
            retention: d.retention,
            query_default_window_days: d.query_default_window_days,
            query_max_window_days: d.query_max_window_days,
        }
    }
}

// `OutboxConfig` gets a `Default` too, same rationale as `AuditConfig` above (hand-built test
// `IamConfig`s can write `outbox: OutboxConfig::default()`). Delegates to `OutboxDefaults` so
// the two never drift apart.
impl Default for OutboxConfig {
    fn default() -> Self {
        let d = OutboxDefaults::default();
        OutboxConfig {
            relay_enabled: d.relay_enabled,
            poll_interval_secs: d.poll_interval_secs,
            batch_size: d.batch_size,
            max_attempts: d.max_attempts,
        }
    }
}

impl IamConfig {
    #[must_use]
    pub fn figment() -> Figment {
        // `.split("__")` maps a DOUBLE-underscore in an `IAM_*` env var to struct nesting, so a
        // SECRET like the API-key pepper can be injected without a config file:
        // `IAM_API_KEYS__PEPPER` -> `api_keys.pepper` (and, as a latent bonus, nested authn/authz
        // env overrides like `IAM_AUTHZ__CACHE__BACKEND` now work too). Splitting on the DOUBLE
        // underscore is what preserves flat single-underscore fields — `IAM_DATABASE_URL` stays
        // `database_url`, `IAM_HTTP_ADDR` stays `http_addr` — so no existing env var changes
        // meaning (the only config env var used anywhere today is `IAM_DATABASE_URL`).
        Figment::from(Serialized::defaults(Defaults::default()))
            .merge(Toml::file("iam.toml"))
            .merge(Env::prefixed("IAM_").split("__"))
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
    /// `https` issuer and a non-empty subject. Mirrors that same posture again for `[api_keys]`
    /// (SMA-445 Task 15, spec D12/§11): the configured pepper decodes to at least 32 bytes
    /// (`ApiKeyConfig::pepper`, surfacing `Pepper::from_config`'s own error), `key_prefix` is
    /// non-empty and doesn't collide (case-insensitively) with the `Bearer` scheme (else it
    /// would misroute every JWT to the API-key auth path), the introspection cache's `redis`
    /// backend has `redis_url` configured and its `ttl_secs` is non-zero (same posture as
    /// `authn.jwks_cache`/`authz.cache` above), and `max_token_bytes` falls within a sane range
    /// whose floor scales with `key_prefix.len()` — a `max_token_bytes` below the shortest
    /// token this config can ever emit would make `api_key::parse_token` reject every issued
    /// key.
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

        // --- SMA-445 Task 15: `[api_keys]` config ------------------------------------------
        // Pepper decodability (>= 32 decoded bytes) is delegated to `Pepper::from_config`
        // (spec D12) rather than re-implemented here — `ApiKeyConfig::pepper` surfaces that
        // same error, covering both an unset (empty-string default) and a too-short pepper.
        if let Err(e) = self.api_keys.pepper() {
            return Err(format!("api_keys.pepper is invalid: {e}"));
        }

        // An empty OR `Bearer`-colliding prefix would misroute every JWT to the API-key
        // resolution path (spec §11 MINOR finding) — checked case-insensitively since the
        // `Bearer` HTTP auth scheme itself is case-insensitive (RFC 9110 §11.1).
        if self.api_keys.key_prefix.is_empty() {
            return Err("api_keys.key_prefix must not be empty (an empty prefix would misroute every JWT to the API-key auth path)".to_string());
        }
        if self.api_keys.key_prefix.eq_ignore_ascii_case("bearer") {
            return Err(format!("api_keys.key_prefix must not collide with the \"Bearer\" scheme (got {:?})", self.api_keys.key_prefix));
        }

        if self.api_keys.introspect_cache.backend == ApiKeyCacheBackend::Redis && self.api_keys.introspect_cache.redis_url.is_none() {
            return Err("api_keys.introspect_cache.backend = \"redis\" requires api_keys.introspect_cache.redis_url".to_string());
        }

        // A zero TTL is broken the same way `authn.jwks_ttl_secs = 0` is above: every put
        // either fails outright (redis `SET EX 0`) or is immediately-expired (memory).
        if self.api_keys.introspect_cache.ttl_secs == 0 {
            return Err("api_keys.introspect_cache.ttl_secs must be at least 1 (0 breaks the introspection cache)".to_string());
        }

        // A sane range. The FLOOR isn't a fixed constant — `api_key::format_token` emits
        // `"{key_prefix}{32 hex chars}_{secret_b64url}"`, and the secret is always the fixed
        // 32-byte `[u8; 32]` `entropy.rs::new_secret` generates, whose base64url-nopad encoding
        // is a fixed `43` chars (`ceil(32 * 8 / 6)`) — so the SHORTEST token this config can
        // ever emit is `key_prefix.len() + FIXED_TOKEN_BYTES` bytes long, where
        // `FIXED_TOKEN_BYTES = 32 (keyid hex) + 1 (separator) + 43 (secret b64url) = 76`. A
        // `max_token_bytes` below that floor would pass this check but then make
        // `api_key::parse_token`'s length-cap check (`token.len() > max_bytes`) reject EVERY
        // key this same config ever issues as `ApiKeyDefect::Malformed` (CodeRabbit SMA-445
        // review fix) — so the floor must scale with the configured `key_prefix`, not be a
        // constant. 8192 as the ceiling leaves generous headroom for a longer future prefix
        // while still rejecting clearly-misconfigured values before they're used to size a
        // length-cap check on the request hot path.
        const FIXED_TOKEN_BYTES: usize = 32 + 1 + 43;
        const MAX_MAX_TOKEN_BYTES: usize = 8192;
        let min_max_token_bytes = self.api_keys.key_prefix.len() + FIXED_TOKEN_BYTES;
        if !(min_max_token_bytes..=MAX_MAX_TOKEN_BYTES).contains(&self.api_keys.max_token_bytes) {
            return Err(format!(
                "api_keys.max_token_bytes ({}) must be between {min_max_token_bytes} (api_keys.key_prefix.len() [{}] + {FIXED_TOKEN_BYTES}, the shortest token this config can ever emit) and {MAX_MAX_TOKEN_BYTES}",
                self.api_keys.max_token_bytes,
                self.api_keys.key_prefix.len()
            ));
        }

        // --- SMA-446 Task A12: `[audit]` config ------------------------------------------------
        // A zero capacity is broken the same way a zero TTL is above: `DenialAuditBuffer::push`
        // would evict the entry it just enqueued on every call, silently discarding every denial
        // audit past the very first `dropped()` bump. `DenialAuditBuffer::new` clamps to `>= 1`
        // defensively, but a misconfigured `0` is an operator error caught at boot here, not
        // silently corrected.
        if self.audit.denial_buffer_capacity == 0 {
            return Err("audit.denial_buffer_capacity must be at least 1 (0 makes the denial-audit buffer evict every entry it enqueues)".to_string());
        }

        // --- SMA-467: `[audit.retention]` config ------------------------------------------------
        // The tick interval divides the maintenance loop's cadence (zero would busy-loop), and
        // ahead_months is capped — each ahead month is a parent-locking CREATE, so a
        // fat-fingered large value would hammer the parent every tick.
        if self.audit.retention.interval_secs == 0 {
            return Err("audit.retention.interval_secs must be at least 1 (0 would busy-loop the maintenance task)".to_string());
        }
        if !(1..=24).contains(&self.audit.retention.ahead_months) {
            return Err(format!("audit.retention.ahead_months ({}) must be between 1 and 24", self.audit.retention.ahead_months));
        }
        if self.audit.query_default_window_days == 0 || self.audit.query_max_window_days == 0 {
            return Err("audit.query_default_window_days and audit.query_max_window_days must be at least 1".to_string());
        }

        // --- SMA-446 Task B9: `[outbox]` config ------------------------------------------------
        // Each of these three is a divisor of the relay loop's own behavior, the same posture as
        // the `authz`/`authn`/`api_keys` `*_secs` checks above: a zero `poll_interval_secs` would
        // busy-loop the poll, a zero `batch_size` would make every tick drain nothing forever, and
        // a zero `max_attempts` would park every row on its very first failed publish, before it
        // ever gets a retry.
        if self.outbox.poll_interval_secs == 0 {
            return Err("outbox.poll_interval_secs must be at least 1 (0 would busy-loop the relay's poll)".to_string());
        }
        if self.outbox.batch_size == 0 {
            return Err("outbox.batch_size must be at least 1 (0 would make every relay tick drain nothing)".to_string());
        }
        if self.outbox.max_attempts == 0 {
            return Err("outbox.max_attempts must be at least 1 (0 would park every outbox row on its first failed publish attempt)".to_string());
        }

        // --- SMA-446 Unit 3: `[metrics]` config ------------------------------------------------
        // Mirrors `GatewayConfig::validate`'s identical check: a separate metrics listener must
        // not collide with the main HTTP port (`enabled = true` with `addr = None` merges
        // `/metrics` onto `http_addr` instead — that's the intended same-port case, not a
        // collision). A same-PORT check, not exact-address-equality: an unequal-but-same-port
        // pair (e.g. `0.0.0.0:8080` metrics vs `127.0.0.1:8080` http) passes exact equality yet
        // both listeners still try to claim the same port and fail at bind time with
        // `AddrInUse`. IAM also has a gRPC listener (`grpc_addr`), so metrics must differ from
        // BOTH.
        if let Some(addr) = self.metrics.addr {
            if addr.port() == self.http_addr.port() {
                return Err("metrics.addr must use a different port than http_addr".to_string());
            }
            if addr.port() == self.grpc_addr.port() {
                return Err("metrics.addr must use a different port than grpc_addr".to_string());
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
            // SMA-445: an overall `validate().is_ok()` now also needs a valid `[api_keys]`
            // pepper — see `authz_defaults_land_with_no_authz_block_at_all`'s identical note.
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()))?;
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
            // SMA-445: an overall `validate().is_ok()` now also needs a valid `[api_keys]`
            // pepper (see `config::tests::valid_pepper_b64`'s doc) — the `[authn.jwks_cache]`
            // block under test here is otherwise unaffected.
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [authn.jwks_cache]
                        backend = "redis"
                        redis_url = "redis://localhost:6379"

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]

                        [api_keys]
                        pepper = "{}"
                    "#,
                    valid_pepper_b64()
                ),
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
            // SMA-445: an overall `validate().is_ok()` now also needs a valid `[api_keys]`
            // pepper (`minimal_issuer_toml()` deliberately carries none — see
            // `config::tests::api_keys_defaults_land_with_no_api_keys_block_at_all`, which
            // exercises exactly that absence — so it's added here at the call site instead).
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()))?;
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
            // SMA-445: an overall `validate().is_ok()` now also needs a valid `[api_keys]`
            // pepper — see `authz_defaults_land_with_no_authz_block_at_all`'s identical note.
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [authz]
                        policy_cache_ttl_secs = 10
                        refresh_interval_secs = 10

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]

                        [api_keys]
                        pepper = "{}"
                    "#,
                    valid_pepper_b64()
                ),
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
            // SMA-445: an overall `validate().is_ok()` now also needs a valid `[api_keys]`
            // pepper — see `authz_defaults_land_with_no_authz_block_at_all`'s identical note.
            jail.create_file(
                "iam.toml",
                &format!(
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

                        [api_keys]
                        pepper = "{}"
                    "#,
                    valid_pepper_b64()
                ),
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

    // --- SMA-445 Task 15: `[api_keys]` config ----------------------------------------------

    /// A valid, base64-encoded 32-byte pepper for `[api_keys]` test fixtures — re-derives
    /// `adapters::api_keys::hasher`'s own test pepper (`[0x5A; 32]`) rather than reaching into
    /// a sibling module's private test helpers.
    fn valid_pepper_b64() -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode([0x5Au8; 32])
    }

    #[test]
    fn api_keys_defaults_land_with_no_api_keys_block_at_all() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("iam.toml", minimal_issuer_toml())?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert_eq!(cfg.api_keys.key_prefix, "pgs_sk_");
            assert_eq!(cfg.api_keys.max_token_bytes, 512);
            assert_eq!(cfg.api_keys.default_expiry_days, None);
            assert_eq!(cfg.api_keys.last_used_throttle_secs, 60);
            assert_eq!(cfg.api_keys.introspect_cache.backend, ApiKeyCacheBackend::Memory);
            assert_eq!(cfg.api_keys.introspect_cache.redis_url, None);
            assert_eq!(cfg.api_keys.introspect_cache.ttl_secs, 30);
            // No pepper configured -> the empty-string default, which `validate()` (not
            // figment extraction) rejects — see `ApiKeyDefaults`'s doc.
            assert!(cfg.validate().is_err(), "an unset api_keys.pepper must fail validate(), not figment extraction");
            Ok(())
        });
    }

    #[test]
    fn rejects_empty_key_prefix() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [api_keys]
                        pepper = "{}"
                        key_prefix = ""

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]
                    "#,
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected an empty api_keys.key_prefix to fail validation");
            Ok(())
        });
    }

    #[test]
    fn rejects_bearer_key_prefix() {
        for prefix in ["Bearer", "bearer", "BEARER"] {
            figment::Jail::expect_with(|jail| {
                jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
                jail.create_file(
                    "iam.toml",
                    &format!(
                        r#"
                            [api_keys]
                            pepper = "{}"
                            key_prefix = "{prefix}"

                            [[authn.issuers]]
                            issuer = "https://idp.example.com/realms/acme"
                            audiences = ["paigasus"]
                        "#,
                        valid_pepper_b64()
                    ),
                )?;
                let cfg: IamConfig = IamConfig::figment().extract()?;
                assert!(cfg.validate().is_err(), "expected api_keys.key_prefix = {prefix:?} to fail validation (Bearer-colliding)");
                Ok(())
            });
        }
    }

    #[test]
    fn rejects_short_pepper() {
        figment::Jail::expect_with(|jail| {
            use base64::Engine;
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            let short_pepper = base64::engine::general_purpose::STANDARD.encode([0x5Au8; 16]); // 16 decoded bytes < the 32 minimum
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [api_keys]
                        pepper = "{short_pepper}"

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]
                    "#
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected a <32-byte decoded pepper to fail validation");
            Ok(())
        });
    }

    #[test]
    fn rejects_zero_ttl() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [api_keys]
                        pepper = "{}"

                        [api_keys.introspect_cache]
                        ttl_secs = 0

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]
                    "#,
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected api_keys.introspect_cache.ttl_secs = 0 to fail validation");
            Ok(())
        });
    }

    #[test]
    fn rejects_redis_backend_without_url() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [api_keys]
                        pepper = "{}"

                        [api_keys.introspect_cache]
                        backend = "redis"

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]
                    "#,
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected an api_keys redis backend without redis_url to fail validation");
            Ok(())
        });
    }

    #[test]
    fn rejects_max_token_bytes_out_of_range() {
        for bad in [0usize, 8, 16384] {
            figment::Jail::expect_with(|jail| {
                jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
                jail.create_file(
                    "iam.toml",
                    &format!(
                        r#"
                            [api_keys]
                            pepper = "{}"
                            max_token_bytes = {bad}

                            [[authn.issuers]]
                            issuer = "https://idp.example.com/realms/acme"
                            audiences = ["paigasus"]
                        "#,
                        valid_pepper_b64()
                    ),
                )?;
                let cfg: IamConfig = IamConfig::figment().extract()?;
                assert!(cfg.validate().is_err(), "expected api_keys.max_token_bytes = {bad} to fail validation");
                Ok(())
            });
        }
    }

    #[test]
    fn accepts_max_token_bytes_at_the_range_boundaries() {
        // The inclusive range: both endpoints must PASS (the rejection test above covers
        // just-outside/way-outside values). Guards against an off-by-one that would flip the
        // bound to exclusive. The lower bound (83) is the default `key_prefix = "pgs_sk_"`
        // (7 chars) + the fixed 76-byte token structure (32 hex + 1 separator + 43-char
        // base64url secret) — the shortest token this default config can ever emit.
        for ok in [83usize, 8192] {
            figment::Jail::expect_with(|jail| {
                jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
                jail.create_file(
                    "iam.toml",
                    &format!(
                        r#"
                            [api_keys]
                            pepper = "{}"
                            max_token_bytes = {ok}

                            [[authn.issuers]]
                            issuer = "https://idp.example.com/realms/acme"
                            audiences = ["paigasus"]
                        "#,
                        valid_pepper_b64()
                    ),
                )?;
                let cfg: IamConfig = IamConfig::figment().extract()?;
                assert_eq!(cfg.api_keys.max_token_bytes, ok);
                assert!(cfg.validate().is_ok(), "expected api_keys.max_token_bytes = {ok} (a range boundary) to pass validation");
                Ok(())
            });
        }
    }

    /// CodeRabbit SMA-445 review fix: the `max_token_bytes` floor must scale with the
    /// configured `key_prefix`, not sit at a fixed constant — a `max_token_bytes` below
    /// `key_prefix.len() + 76` (32 hex + 1 separator + 43-char base64url secret) passes a
    /// fixed-floor check but then makes `api_key::parse_token`'s length cap reject EVERY key
    /// this same config ever issues as `Malformed`. Uses a longer-than-default `key_prefix` to
    /// prove the floor is derived, not hardcoded to the default `"pgs_sk_"`'s 83.
    #[test]
    fn rejects_max_token_bytes_below_the_floor_derived_from_key_prefix_len() {
        let prefix = "a_much_longer_key_prefix_"; // 25 chars -> floor = 25 + 76 = 101
        let floor = prefix.len() + 76;

        for (max_token_bytes, should_pass) in [(floor - 1, false), (floor, true)] {
            figment::Jail::expect_with(|jail| {
                jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
                jail.create_file(
                    "iam.toml",
                    &format!(
                        r#"
                            [api_keys]
                            pepper = "{}"
                            key_prefix = "{prefix}"
                            max_token_bytes = {max_token_bytes}

                            [[authn.issuers]]
                            issuer = "https://idp.example.com/realms/acme"
                            audiences = ["paigasus"]
                        "#,
                        valid_pepper_b64()
                    ),
                )?;
                let cfg: IamConfig = IamConfig::figment().extract()?;
                assert_eq!(
                    cfg.validate().is_ok(),
                    should_pass,
                    "expected api_keys.max_token_bytes = {max_token_bytes} (floor = {floor}) validation to {}",
                    if should_pass { "pass" } else { "fail" }
                );
                Ok(())
            });
        }
    }

    #[test]
    fn valid_config_passes() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [api_keys]
                        pepper = "{}"
                        key_prefix = "pgs_sk_"
                        max_token_bytes = 512
                        default_expiry_days = 365
                        last_used_throttle_secs = 60

                        [api_keys.introspect_cache]
                        backend = "redis"
                        redis_url = "redis://localhost:6379"
                        ttl_secs = 30

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]
                    "#,
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert_eq!(cfg.api_keys.key_prefix, "pgs_sk_");
            assert_eq!(cfg.api_keys.max_token_bytes, 512);
            assert_eq!(cfg.api_keys.default_expiry_days, Some(365));
            assert_eq!(cfg.api_keys.last_used_throttle_secs, 60);
            assert_eq!(cfg.api_keys.introspect_cache.backend, ApiKeyCacheBackend::Redis);
            assert_eq!(cfg.api_keys.introspect_cache.redis_url.as_deref(), Some("redis://localhost:6379"));
            assert_eq!(cfg.api_keys.introspect_cache.ttl_secs, 30);
            assert!(cfg.api_keys.pepper().is_ok(), "a valid pepper must decode via ApiKeyConfig::pepper");
            assert!(cfg.validate().is_ok(), "expected a fully-populated, valid [api_keys] block to pass validation");
            Ok(())
        });
    }

    #[test]
    fn pepper_is_injectable_via_the_double_underscore_env_var() {
        // The pepper is a SECRET, so operators must be able to inject it purely via env — no
        // config file. `IAM_API_KEYS__PEPPER` -> `api_keys.pepper` relies on `figment()`'s
        // `.split("__")`; this proves it actually lands (and that a flat `IAM_DATABASE_URL`
        // still works alongside it, i.e. the split didn't break single-underscore fields).
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            let pepper = valid_pepper_b64();
            jail.set_env("IAM_API_KEYS__PEPPER", &pepper);
            // No `[api_keys]` in the file at all — the pepper comes purely from the env var.
            jail.create_file("iam.toml", minimal_issuer_toml())?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert_eq!(cfg.database_url, "postgres://u:p@localhost/db", "the flat IAM_DATABASE_URL env var must still map to database_url");
            assert_eq!(cfg.api_keys.pepper.0, pepper, "IAM_API_KEYS__PEPPER must populate api_keys.pepper");
            assert!(cfg.api_keys.pepper().is_ok(), "the env-injected pepper must decode via ApiKeyConfig::pepper");
            assert!(cfg.validate().is_ok(), "a config whose only pepper source is IAM_API_KEYS__PEPPER must pass validation");
            Ok(())
        });
    }

    #[test]
    fn pepper_never_appears_in_debug_or_serialized_config() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            let pepper = valid_pepper_b64();
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [api_keys]
                        pepper = "{pepper}"

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]
                    "#
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            // Sanity: the raw pepper string actually round-tripped through figment — otherwise
            // the "never appears" assertions below would pass vacuously.
            assert_eq!(cfg.api_keys.pepper.0, pepper);

            let debugged = format!("{cfg:?}");
            assert!(!debugged.contains(&pepper), "the configured pepper leaked into IamConfig's Debug output: {debugged}");
            assert!(debugged.contains("redacted"), "expected a redaction marker in IamConfig's Debug output: {debugged}");

            let serialized = serde_json::to_string(&cfg).expect("IamConfig serializes");
            assert!(!serialized.contains(&pepper), "the configured pepper leaked into IamConfig's serialized form: {serialized}");

            Ok(())
        });
    }

    // --- SMA-446 Task A12: `[audit]` config ------------------------------------------------

    #[test]
    fn audit_defaults_land_with_no_audit_block_at_all() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            // A valid `[api_keys]` pepper is still needed for the overall `validate().is_ok()` —
            // see `authz_defaults_land_with_no_authz_block_at_all`'s identical note.
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()))?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert_eq!(cfg.audit.denial_buffer_capacity, 4096, "denial_buffer_capacity must default to 4096");
            assert!(cfg.validate().is_ok(), "audit defaults alone should pass validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_a_zero_denial_buffer_capacity() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [audit]
                        denial_buffer_capacity = 0

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]

                        [api_keys]
                        pepper = "{}"
                    "#,
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected audit.denial_buffer_capacity = 0 to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_accepts_an_explicit_denial_buffer_capacity() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [audit]
                        denial_buffer_capacity = 128

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]

                        [api_keys]
                        pepper = "{}"
                    "#,
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert_eq!(cfg.audit.denial_buffer_capacity, 128);
            assert!(cfg.validate().is_ok(), "expected an explicit non-zero denial_buffer_capacity to pass validation");
            Ok(())
        });
    }

    // --- SMA-467: `[audit.retention]` + query-window config --------------------------------

    #[test]
    fn retention_defaults_land_with_no_block() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()))?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.audit.retention.enabled);
            assert_eq!(cfg.audit.retention.interval_secs, 86_400);
            assert_eq!(cfg.audit.retention.ahead_months, 1);
            assert_eq!(cfg.audit.retention.denied_months, 3);
            assert_eq!(cfg.audit.retention.committed_months, 0);
            assert_eq!(cfg.audit.query_default_window_days, 90);
            assert_eq!(cfg.audit.query_max_window_days, 366);
            assert!(cfg.validate().is_ok(), "retention defaults must validate");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_zero_retention_interval() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!("{}\n[api_keys]\npepper = \"{}\"\n[audit.retention]\ninterval_secs = 0", minimal_issuer_toml(), valid_pepper_b64()),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "interval_secs = 0 must fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_out_of_range_ahead_months() {
        for bad in ["ahead_months = 0", "ahead_months = 25"] {
            figment::Jail::expect_with(|jail| {
                jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
                jail.create_file(
                    "iam.toml",
                    &format!("{}\n[api_keys]\npepper = \"{}\"\n[audit.retention]\n{bad}", minimal_issuer_toml(), valid_pepper_b64()),
                )?;
                let cfg: IamConfig = IamConfig::figment().extract()?;
                assert!(cfg.validate().is_err(), "{bad} must fail validation");
                Ok(())
            });
        }
    }

    // --- SMA-446 Task B9: `[outbox]` config -------------------------------------------------

    #[test]
    fn outbox_defaults_land_with_no_outbox_block_at_all() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            // A valid `[api_keys]` pepper is still needed for the overall `validate().is_ok()` —
            // see `authz_defaults_land_with_no_authz_block_at_all`'s identical note.
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()))?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.outbox.relay_enabled, "relay_enabled must default to true");
            assert_eq!(cfg.outbox.poll_interval_secs, 5);
            assert_eq!(cfg.outbox.batch_size, 100);
            assert_eq!(cfg.outbox.max_attempts, 5);
            assert!(cfg.validate().is_ok(), "outbox defaults alone should pass validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_a_zero_outbox_poll_interval() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [outbox]
                        poll_interval_secs = 0

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]

                        [api_keys]
                        pepper = "{}"
                    "#,
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected outbox.poll_interval_secs = 0 to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_a_zero_outbox_batch_size() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [outbox]
                        batch_size = 0

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]

                        [api_keys]
                        pepper = "{}"
                    "#,
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected outbox.batch_size = 0 to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_a_zero_outbox_max_attempts() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [outbox]
                        max_attempts = 0

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]

                        [api_keys]
                        pepper = "{}"
                    "#,
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected outbox.max_attempts = 0 to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_accepts_an_explicit_outbox_block_including_relay_disabled() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [outbox]
                        relay_enabled = false
                        poll_interval_secs = 10
                        batch_size = 250
                        max_attempts = 8

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]

                        [api_keys]
                        pepper = "{}"
                    "#,
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(!cfg.outbox.relay_enabled, "relay_enabled = false must be honored (not just a default)");
            assert_eq!(cfg.outbox.poll_interval_secs, 10);
            assert_eq!(cfg.outbox.batch_size, 250);
            assert_eq!(cfg.outbox.max_attempts, 8);
            assert!(cfg.validate().is_ok(), "expected a fully-populated, valid [outbox] block (relay disabled) to pass validation");
            Ok(())
        });
    }

    // --- SMA-446 Unit 3: `[metrics]` config -------------------------------------------------

    #[test]
    fn metrics_defaults_land_with_no_metrics_block_at_all() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()))?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.metrics.enabled, "metrics.enabled must default to true");
            assert_eq!(cfg.metrics.addr, None, "metrics.addr must default to unset (same-port merge)");
            assert!(cfg.validate().is_ok(), "metrics defaults alone should pass validation");
            Ok(())
        });
    }

    #[test]
    fn metrics_addr_must_differ_from_http_addr() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()))?;
            let mut cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_ok(), "sanity: the base config must validate before the collision is introduced");
            cfg.metrics.addr = Some(cfg.http_addr); // exact collision
            assert!(cfg.validate().is_err(), "metrics.addr == http_addr is a config error");
            Ok(())
        });
    }

    #[test]
    fn metrics_addr_same_port_different_host_as_http_addr_is_rejected() {
        figment::Jail::expect_with(|jail| {
            // A wildcard-vs-loopback pair on the SAME port is NOT caught by exact-address
            // equality but both listeners still fail at bind with `AddrInUse` — validate() must
            // reject this too, not just an exact-address match.
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!("http_addr = \"127.0.0.1:8080\"\n{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()),
            )?;
            let mut cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_ok(), "sanity: the base config must validate before the collision is introduced");
            cfg.metrics.addr = Some("0.0.0.0:8080".parse().expect("valid addr"));
            assert!(cfg.validate().is_err(), "metrics.addr and http_addr sharing a port on different hosts is a config error");
            Ok(())
        });
    }

    #[test]
    fn metrics_addr_same_port_as_grpc_addr_is_rejected() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()))?;
            let mut cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_ok(), "sanity: the base config must validate before the collision is introduced");
            cfg.metrics.addr = Some(cfg.grpc_addr); // collision with grpc_addr, not http_addr
            assert!(cfg.validate().is_err(), "metrics.addr == grpc_addr is a config error");
            Ok(())
        });
    }

    #[test]
    fn metrics_addr_distinct_ports_from_http_and_grpc_is_ok() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()))?;
            let mut cfg: IamConfig = IamConfig::figment().extract()?;
            cfg.metrics.addr = Some("0.0.0.0:9999".parse().expect("valid addr"));
            assert!(cfg.validate().is_ok(), "metrics.addr on a distinct port from both http_addr and grpc_addr should validate");
            Ok(())
        });
    }
}
