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
    /// The main SeaORM connection string. A [`RedactedUrl`] because a Postgres DSN routinely
    /// carries a password (`postgres://user:pass@host/db`) and this struct derives
    /// `Debug`/`Serialize` (see [`RedactedUrl`]); read the real value with
    /// [`RedactedUrl::as_str`].
    pub database_url: RedactedUrl,
    pub log_level: String,
    pub authn: AuthnConfig,
    pub authz: AuthzConfig,
    pub api_keys: ApiKeyConfig,
    pub audit: AuditConfig,
    pub outbox: OutboxConfig,
    pub metrics: MetricsConfig,
}

/// A connection URL that may embed credentials (`postgres://user:pass@host/db`,
/// `redis://user:pass@host:6379/0`, `nats://user:pass@host`) — the redacting newtype worn by
/// [`IamConfig::database_url`], [`OutboxConfig::listen_database_url`], all three cache
/// `redis_url`s ([`JwksCacheConfig`], [`AuthzCacheConfig`], [`ApiKeyCacheConfig`]) and
/// [`PublisherConfig::url`].
///
/// `IamConfig` derives `Debug`/`Serialize`, so a credential must never round-trip through
/// either: both outbound directions emit a fixed `<redacted>` placeholder, while `Deserialize`
/// is hand-rolled to delegate straight to `String` so figment still populates the REAL value
/// from whichever layer (default/toml/env) supplied it. Exactly [`RawPepper`]'s idiom.
///
/// **Nothing dumps `IamConfig` today** (SMA-496) — `readyz` returns a bare status object, the one
/// config-bearing log line prints two socket addresses, and `Serialize` is exercised only by this
/// module's tests. The redaction is deliberate defense-in-depth: it makes the dump somebody
/// eventually adds — a boot-time config log, a debug endpoint, a stray `{config:?}` in an error
/// path — safe by construction, rather than a leak found in review. Choosing the type IS the
/// mechanism; there is no runtime guard behind it.
///
/// A newtype rather than per-container manual impls **because redaction then travels with the
/// type**: a future credential-bearing URL field is protected by choosing this type, not by
/// remembering to extend two hand-written impls that spell out every sibling field. `RawPepper`
/// can skip `Serialize` entirely — `ApiKeyConfig` marks its field `#[serde(skip_serializing)]` —
/// but these fields are genuinely serialized, so the impl has to exist and redact in place.
/// `PublisherConfig` carried exactly those hand-written impls until SMA-496 and now simply wears
/// this type instead.
///
/// Deliberately implements neither `Display` nor `AsRef<str>`: the only way out is
/// [`as_str`](RedactedUrl::as_str), which is greppable and cannot be reached by accident through a
/// `{}` in a format string.
#[derive(Clone, PartialEq, Eq)]
pub struct RedactedUrl(String);

impl RedactedUrl {
    /// The REAL url, for handing to a connection constructor. Never log the result.
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

impl From<String> for RedactedUrl {
    fn from(url: String) -> Self {
        RedactedUrl(url)
    }
}

impl From<&str> for RedactedUrl {
    fn from(url: &str) -> Self {
        RedactedUrl(url.to_string())
    }
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
    /// Required when `backend = "redis"`. A [`RedactedUrl`] because a Redis connection string
    /// carries credentials exactly as a Postgres DSN does (`redis://user:pass@host:6379/0`);
    /// read the real value with [`RedactedUrl::as_str`].
    pub redis_url: Option<RedactedUrl>,
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
    /// Required when `backend = "redis"`. A [`RedactedUrl`], same reason as
    /// [`JwksCacheConfig::redis_url`]; read the real value with [`RedactedUrl::as_str`].
    pub redis_url: Option<RedactedUrl>,
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
    /// derived `Serialize` omits it entirely; `RawPepper`'s
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
/// challenge M6). `IamConfig` derives `Debug`/`Serialize` — see [`RedactedUrl`]'s doc for why
/// that alone is reason enough — so the configured secret must never round-trip through
/// either: `Debug` is
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
    /// Required when `backend = "redis"`. A [`RedactedUrl`], same reason as
    /// [`JwksCacheConfig::redis_url`]; read the real value with [`RedactedUrl::as_str`].
    pub redis_url: Option<RedactedUrl>,
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
    ///
    /// Defaults to **60** (SMA-471 D9, raised from `5`): a row survives
    /// `max_attempts × poll_interval_secs` of consecutive publish failures before parking, which
    /// at the default `poll_interval_secs = 5` is ≈5 minutes — enough that a routine broker
    /// restart does not dead-letter the whole in-flight backlog into the SMA-469 dead-letter
    /// surface (the old default of `5` gave only ~25 seconds).
    ///
    /// **The cost, paid on every deployment regardless of `[outbox.publisher].backend`:** the
    /// relay drains strictly `ORDER BY id` (FIFO) one attempt per row per tick, so a
    /// *permanently* failing row — not transient, believed impossible today since all eight
    /// payload shapes are small fixed objects (spec §5) — now head-of-line blocks every healthy
    /// row behind it (up to `batch_size` of them) for ~5 minutes instead of ~25 seconds before it
    /// parks and the relay moves on. Accepted; `PublishError::Permanent`, parking a deterministic
    /// failure immediately instead of retrying it to exhaustion, is the follow-up that removes
    /// this cost (spec §7).
    pub max_attempts: u32,
    /// SMA-489 D11. When `true` (the default) each `PgOutbox::enqueue` emits
    /// `pg_notify('iam_outbox_event','')` on the mutation's own transaction, and `main.rs`
    /// spawns the `PgOutboxListener` that turns those notifications into relay wakeups.
    ///
    /// Gates **both halves on purpose.** The writer is not free to leave on: a listener that
    /// wedges while still holding its `LISTEN` fills Postgres's async notification queue, and a
    /// full queue makes every transaction that calls `NOTIFY` **fail at commit** — i.e. every
    /// IAM mutation. An escape hatch that could not switch the writer off would not be one.
    ///
    /// `false` restores today's *wakeup* behaviour exactly (poll-only, no notify statement). It
    /// does NOT restore today's *drain* behaviour: the relay's backlog continuation (D9) is
    /// independent of this flag and stays active.
    pub wake_on_commit: bool,
    /// SMA-489 D14. Minimum gap between two nudge-driven ticks, in milliseconds (± up to 25%
    /// jitter so replicas do not converge). Validated non-zero.
    ///
    /// `Notify::notify_one` stores a permit, so under sustained write traffic there is always
    /// one pending and the relay would otherwise tick back-to-back with zero idle. NOTIFY is
    /// broadcast to every listening session, so R commits/s × N replicas produces R×N wakeups
    /// and `SKIP LOCKED` makes N-1 of those ticks do wasted work. At the design point
    /// (<10 mutations/s, 2-3 replicas) this is never reached; it bounds the worst case.
    ///
    /// Does NOT apply to the poll arm — that is already bounded by `poll_interval_secs`.
    pub wake_debounce_ms: u64,
    /// SMA-489 D6/§1.5. Connection string for the listener only; falls back to `database_url`.
    ///
    /// **`LISTEN` requires a direct connection or a SESSION-mode pooler.** PgBouncer's
    /// transaction and statement modes do not support it, and the failure is silent and total:
    /// `pg_notify` still succeeds on the writer side while the listener receives nothing
    /// forever. This field exists so a deployment that fronts Postgres with a transaction-mode
    /// pooler can point the listener at a direct endpoint without moving the main connection.
    /// `IamOutboxNotificationsAbsent` is the alert that detects the misconfiguration.
    ///
    /// A [`RedactedUrl`] for the same reason `database_url` is: it is a full DSN, credentials
    /// included, and this struct derives `Debug`/`Serialize`.
    pub listen_database_url: Option<RedactedUrl>,
    /// Retention for the table the relay drains — see [`OutboxRetentionConfig`].
    #[serde(default)]
    pub retention: OutboxRetentionConfig,
    /// The delivery sink the relay drains into — see [`PublisherConfig`].
    #[serde(default)]
    pub publisher: PublisherConfig,
}

/// `event_outbox` retention (SMA-469) — the knobs for
/// [`PgOutboxMaintainer`](crate::adapters::persistence::PgOutboxMaintainer), the background
/// sweep that bounds the outbox's growth. Nests under `[outbox]` exactly as
/// [`RetentionConfig`] nests under `[audit]`; every field has a default, so an absent
/// `[outbox.retention]` block is valid config.
///
/// **`0` means "never" for BOTH day windows** — one meaning for the sentinel across the whole
/// block, deliberately: two different readings of `0` inside one table would be a trap.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct OutboxRetentionConfig {
    /// When `false`, the maintainer performs NO deletions — but it is still spawned and still
    /// ticks, because the tick is what refreshes `iam_outbox_parked_rows`. Gating the SPAWN on
    /// this would mean an operator who sets `enabled = false` (a plausible "stop deleting
    /// things" reaction during an incident) silently loses the dead-letter backlog signal
    /// while the relay keeps parking rows.
    pub enabled: bool,
    /// Seconds between sweep ticks. Validated non-zero.
    pub interval_secs: u64,
    /// Delete published rows older than this many days. `0` = never delete published rows.
    pub published_days: u32,
    /// Delete parked rows whose `parked_at` is older than this many days. `0` = never (the
    /// default) — auto-deleting the very thing an operator is alerted to inspect must be a
    /// deliberate choice, mirroring `audit.retention.committed_months`. A non-zero value
    /// triggers a startup `warn!`.
    pub parked_days: u32,
    /// Rows deleted per pass. Validated non-zero.
    pub batch_size: u64,
    /// Passes per tick, so one tick retires at most `batch_size * this` rows and a huge first
    /// sweep resumes next tick instead of holding one tick open. Config rather than a constant
    /// because it is exactly as much an operational knob as `batch_size`: at the defaults a
    /// deployment draining a 10M-row backlog needs ~8 days, and the operator doing that
    /// drain must be able to raise it. Validated non-zero.
    pub max_batches_per_tick: u32,
}

impl Default for OutboxRetentionConfig {
    fn default() -> Self {
        OutboxRetentionConfig {
            enabled: true,
            interval_secs: 3_600,
            published_days: 7,
            parked_days: 0,
            batch_size: 1_000,
            max_batches_per_tick: 50,
        }
    }
}

/// The outbox relay's delivery sink (SMA-471). Mirrors `[authn.jwks_cache]` /
/// `[authz.cache]` field-for-field: a `backend` enum plus the connection fields the non-default
/// backend needs, all `Option` with NO default so `validate` can require them meaningfully.
///
/// Defaults to `tracing`, so an absent `[outbox.publisher]` block — and every existing config
/// file — keeps working with no broker available (SMA-471 D12).
///
/// `url` wears [`RedactedUrl`], so `Debug`/`Serialize` are ordinary derives: redaction travels
/// with the field's type rather than with two hand-written impls that had to spell out every
/// sibling field (and hand-maintain their own field count) to protect one of them.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PublisherConfig {
    pub backend: PublisherBackend,
    /// Required when `backend = "nats"`. A [`RedactedUrl`] because it may carry credentials
    /// (`nats://user:pass@host`); read the real value with [`RedactedUrl::as_str`].
    ///
    /// The sibling `credentials_file`, `root_ca_bundle` and `inbox_prefix` stay plain `String`s
    /// deliberately: the first two are filesystem paths and the third a subject prefix — none is
    /// a secret, so none needs the newtype.
    pub url: Option<RedactedUrl>,
    pub stream: String,
    /// CloudEvents `source`, copied verbatim into every published envelope. MUST stay stable for
    /// a stream's lifetime: consumers dedup on `id` alone while CloudEvents scopes identity to
    /// `(source, id)` (SMA-471 D6), so changing this on a live stream is a breaking operational
    /// act.
    ///
    /// Validated as an **absolute** URI via `url::Url::parse` — deliberately narrower than
    /// CloudEvents, which permits any RFC 3986 URI-reference and merely RECOMMENDs the absolute
    /// form. Non-special schemes (`urn:`, `tag:`, `mailto:`, a bare custom scheme) all parse; only
    /// relative references and malformed values are rejected. Validation parses but never
    /// rewrites: the raw string is what ships, so WHATWG normalization (which would lowercase the
    /// scheme and host) never changes what consumers see.
    pub source: String,
    pub publish_timeout_secs: u64,
    /// JetStream's per-stream dedup window. A COVERAGE window, not a guarantee — see
    /// `IamConfig::validate` and SMA-471 D3/D10 for what it does and does not cover.
    pub duplicate_window_secs: u64,
    /// Stream `max_age`. `0` = unlimited (warns at startup when this service creates the
    /// stream): an unbounded `File` stream grows until the broker's disk fills.
    pub max_age_secs: u64,
    /// Path to a NATS `.creds` (JWT + nkey seed). A path, not a secret — no redaction needed.
    pub credentials_file: Option<String>,
    /// Path to a PEM bundle of root CAs used to verify the broker's certificate (SMA-493 D7).
    ///
    /// **This REPLACES the system trust store, it does not extend it.**
    /// `ConnectOptions::add_root_certificates` assigns rather than appends (`options.rs:543`),
    /// and `config_tls` skips `load_native_certs()` entirely once any certificate is named
    /// (`tls.rs:61`). Concatenate every CA the client needs into one file — naming only a private
    /// CA and later moving the broker behind a public one is a total outage that presents as a
    /// bare TLS error. Omitted, the system trust store is used, which is the pre-SMA-493
    /// behaviour.
    ///
    /// Re-read on every connection attempt (`connector.rs:544`), so a rotated bundle needs no
    /// restart.
    pub root_ca_bundle: Option<String>,
    /// The client's `_INBOX` prefix (SMA-493 D4). MUST match the `subscribe` grant in the NATS
    /// account, or every publish times out waiting for an ack it is not allowed to receive.
    ///
    /// Not cosmetic: JetStream acks and pull-consumer deliveries both land on the client's inbox,
    /// so inside a shared account a client holding `sub _INBOX.>` can read another client's
    /// deliveries. Per-user prefixes are the only way to close that, because inbox replies are
    /// the one subject space every client must be able to read. `None` keeps async-nats' default
    /// `_INBOX`, so a deployment that has not adopted `ops/nats/` is unaffected.
    pub inbox_prefix: Option<String>,
    /// Escape hatch for a dev or CI broker (SMA-493 D6). Relaxes BOTH the `tls://` requirement
    /// and the `credentials_file` requirement — it legalises an unauthenticated broker as well as
    /// an unencrypted one, which is why it is not called `allow_plaintext`. Never relaxes the ban
    /// on url-embedded credentials, which async-nats ignores outright.
    pub allow_insecure_broker: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PublisherBackend {
    Tracing,
    Nats,
}

impl Default for PublisherConfig {
    fn default() -> Self {
        PublisherConfig {
            backend: PublisherBackend::Tracing,
            url: None,
            stream: "IAM_EVENTS".to_string(),
            source: "urn:paigasus:iam".to_string(),
            publish_timeout_secs: 2,
            duplicate_window_secs: 3_600,
            max_age_secs: 604_800,
            credentials_file: None,
            root_ca_bundle: None,
            inbox_prefix: None,
            allow_insecure_broker: false,
        }
    }
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

// Mirrors `OutboxConfig` field-for-field. `poll_interval_secs = 5` / `batch_size = 100` are the
// spec-example steady-state values (frequent enough to keep `event_outbox` drained close to real
// time, generous enough to absorb a burst without an oversized single transaction);
// `relay_enabled = true` because the relay is the intended steady-state behavior (see
// `OutboxConfig`'s doc for the disabled path). `max_attempts = 60` (SMA-471 D9, raised from 5) so
// a routine broker restart does not dead-letter the in-flight backlog before it recovers.
// `publisher` defaults to the `tracing` backend (see `PublisherConfig`'s doc) so an absent
// `[outbox.publisher]` block — and every pre-SMA-471 config file — keeps working with no broker
// available.
#[derive(Serialize)]
struct OutboxDefaults {
    relay_enabled: bool,
    poll_interval_secs: u64,
    batch_size: u64,
    max_attempts: u32,
    wake_on_commit: bool,
    wake_debounce_ms: u64,
    // Plain `String`, not `RedactedUrl` — same exception `ApiKeyDefaults::pepper` takes: this
    // struct only ever feeds figment's default LAYER and is never itself logged or dumped, and a
    // `RedactedUrl` here would serialize the literal `"<redacted>"` INTO that layer. The default
    // is `None` either way; figment still deserializes the merged value into
    // `OutboxConfig::listen_database_url: Option<RedactedUrl>` whichever layer supplied it.
    listen_database_url: Option<String>,
    retention: OutboxRetentionConfig,
    publisher: PublisherConfig,
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
            max_attempts: 60,
            wake_on_commit: true,
            wake_debounce_ms: 200,
            listen_database_url: None,
            retention: OutboxRetentionConfig::default(),
            publisher: PublisherConfig::default(),
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
            wake_on_commit: d.wake_on_commit,
            wake_debounce_ms: d.wake_debounce_ms,
            listen_database_url: d.listen_database_url.map(RedactedUrl::from),
            retention: d.retention,
            publisher: d.publisher,
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

        // Both windows feed `to - chrono::Duration::days(window)` on the audit-query hot
        // path (`pg_audit_log.rs`) — an absurdly large value PANICS there (out-of-range
        // `DateTime`), not merely returns an error. Cap both at 36_600 days (~100 years),
        // comfortably beyond any legitimate retention/query need, and reject at boot instead
        // of on the first oversized query. Also require the default lookback to not exceed
        // the max clamp: a `query_default_window_days` wider than `query_max_window_days`
        // is incoherent (the "default" would always get clamped down to the max, making the
        // configured default value meaningless).
        const MAX_QUERY_WINDOW_DAYS: u32 = 36_600;
        if self.audit.query_default_window_days > MAX_QUERY_WINDOW_DAYS || self.audit.query_max_window_days > MAX_QUERY_WINDOW_DAYS {
            return Err(format!(
                "audit.query_default_window_days and audit.query_max_window_days must be at most {MAX_QUERY_WINDOW_DAYS} (~100 years; larger values overflow the `to - Duration::days(window)` computation on the audit-query hot path)"
            ));
        }
        if self.audit.query_default_window_days > self.audit.query_max_window_days {
            return Err(format!(
                "audit.query_default_window_days ({}) must be <= audit.query_max_window_days ({}): a default lookback wider than the max clamp is incoherent",
                self.audit.query_default_window_days, self.audit.query_max_window_days
            ));
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
        // SMA-489 D14: a zero debounce removes the tick-rate floor entirely, which is the
        // busy-loop the whole design exists to avoid.
        if self.outbox.wake_debounce_ms == 0 {
            return Err("outbox.wake_debounce_ms must be at least 1 (0 would remove the nudge tick-rate floor)".to_string());
        }

        // --- SMA-471: `[outbox.publisher]` config ----------------------------------------------
        // Every rule below except (6) is gated on the `nats` backend — a `tracing` deployment
        // must never fail boot over a broker it does not run.
        if self.outbox.publisher.backend == PublisherBackend::Nats {
            let p = &self.outbox.publisher;
            if p.url.is_none() {
                return Err("outbox.publisher.backend = \"nats\" requires outbox.publisher.url".to_string());
            }
            // --- SMA-493 D6: transport + credential posture --------------------------------
            // `url::Url` is already this function's dependency (the `source` check below). A
            // url that does not parse at all is left to `connect` to report, exactly as before
            // — this block tightens posture, it does not add a syntax gate.
            if let Some(raw) = p.url.as_ref().map(RedactedUrl::as_str) {
                // Unconditional, no escape hatch: async-nats never reads url userinfo
                // (`ServerAddr::username`/`password` have no caller in the connect path), so a
                // config carrying `nats://user:pass@host` connects ANONYMOUSLY while looking
                // authenticated. Rejecting it is the only way that misconception surfaces. Only
                // checked when `raw` actually parses with a userinfo component — a url that
                // does not parse at all is left to `connect` to report.
                if let Ok(parsed) = url::Url::parse(raw)
                    && (!parsed.username().is_empty() || parsed.password().is_some())
                {
                    return Err(
                        "outbox.publisher.url must not embed credentials — async-nats ignores them entirely, so the connection would be anonymous; use outbox.publisher.credentials_file".to_string(),
                    );
                }
                // Checked on the RAW string, not `url::Url::parse`'s scheme: a schemeless form
                // like `tls:user@host:4222` (missing `//`) parses via `url::Url` as a
                // cannot-be-a-base URL with scheme "tls" and EMPTY userinfo — passing both the
                // check above and a naive `parsed.scheme() == "tls"` check — while async-nats'
                // own parser (`ServerAddr::from_str`) requires `://` to recognize a scheme at
                // all and prepends `nats://` to anything lacking it, so this would actually be
                // dialled as `nats://tls:user@host:4222`: plaintext, with `tls`/`user` silently
                // discarded. Requiring the literal `://` on the raw string closes that gap.
                // The scheme itself is compared case-INsensitively (`eq_ignore_ascii_case`) to
                // match both `url::Url::parse` (lowercases the scheme per the WHATWG URL
                // Standard) and async-nats' `ServerAddr::from_url`, which compares against that
                // same lowercased scheme — so `TLS://host:4222` is accepted here exactly as it
                // is by async-nats, instead of forcing an operator to reach for
                // `allow_insecure_broker` over a mere casing mismatch.
                let scheme_is_tls = raw.split_once("://").is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("tls"));
                if !scheme_is_tls && !p.allow_insecure_broker {
                    // Report only the scheme-shaped prefix (substring before the first `:`),
                    // never the full raw url: a malformed url with no `://` but baked-in
                    // credentials (e.g. `user:hunter2@host:4222`) parses via `url::Url` as an
                    // opaque scheme "user" with EMPTY username/password, so the credentials
                    // check above does not catch it — and `url` wears `RedactedUrl` specifically
                    // so it never reaches a log line. THIS error string DOES reach the logs, so
                    // interpolating `raw` would bypass that redaction entirely. Note `as_str()`
                    // is called once above to obtain `raw` for PARSING, and deliberately does not
                    // appear in the message: emit the scheme alone, never the url. Guarded by
                    // `a_rejected_url_with_a_password_does_not_leak_it_into_the_error` below.
                    let scheme_hint = raw.split(':').next().unwrap_or(raw);
                    return Err(format!(
                        "outbox.publisher.url must use tls:// for the nats backend (got {scheme_hint:?}) — set outbox.publisher.allow_insecure_broker = true for a dev or CI broker, which also waives the credentials_file requirement"
                    ));
                }
            }
            if p.credentials_file.is_none() && !p.allow_insecure_broker {
                return Err(
                    "outbox.publisher.backend = \"nats\" requires outbox.publisher.credentials_file (a NATS .creds) — set outbox.publisher.allow_insecure_broker = true for a dev or CI broker, which also waives the tls:// requirement".to_string()
                );
            }
            if p.publish_timeout_secs == 0 {
                return Err("outbox.publisher.publish_timeout_secs must be at least 1".to_string());
            }
            if p.duplicate_window_secs == 0 {
                return Err("outbox.publisher.duplicate_window_secs must be at least 1".to_string());
            }
            if p.stream.is_empty() {
                return Err("outbox.publisher.stream must not be empty".to_string());
            }
            // CloudEvents requires `source` to be a non-empty URI-reference and RECOMMENDS an
            // absolute URI. We require the absolute form (`url::Url::parse` rejects relative
            // references): the value is copied verbatim into every published envelope, it must
            // stay stable for a stream's lifetime (D6), and a stricter check here costs an
            // operator nothing while catching typos that would otherwise ship malformed events
            // to external consumers.
            //
            // A hand-rolled "non-empty and no whitespace" check was the first attempt and was
            // too weak — it accepted `%` and `http://[`, both of which are malformed
            // URI-references (CodeRabbit, PR 112). `url` is already in this crate's dependency
            // tree via `async-nats` and `redis`, so parsing properly costs no extra build.
            if let Err(e) = url::Url::parse(&p.source) {
                return Err(format!(
                    "outbox.publisher.source must be an absolute URI (CloudEvents RECOMMENDs one, and it is copied verbatim into every event): {e}"
                ));
            }
            // SMA-471 D10. A FLOOR, not a guarantee: it catches the one republish gap fully
            // determined by config (an operator raising `max_attempts` past the window). It does
            // NOT cover a tick rollback, a crash-restart, or an operator dead-letter replay —
            // see the spec's D3. `saturating_mul` because `max_attempts` is u32 and the product
            // overflows a naive multiply.
            let retry_span = u64::from(self.outbox.max_attempts).saturating_mul(self.outbox.poll_interval_secs);
            if p.duplicate_window_secs <= retry_span {
                return Err(format!(
                    "outbox.publisher.duplicate_window_secs ({}) must exceed outbox.max_attempts × outbox.poll_interval_secs ({} × {} = {}) — otherwise a row's last retry falls outside JetStream's dedup window and double-delivers",
                    p.duplicate_window_secs, self.outbox.max_attempts, self.outbox.poll_interval_secs, retry_span
                ));
            }
            // SMA-471 D8: JetStream itself requires duplicate_window <= max_age when max_age > 0.
            if p.max_age_secs != 0 && p.max_age_secs <= p.duplicate_window_secs {
                return Err(format!(
                    "outbox.publisher.max_age_secs ({}) must exceed outbox.publisher.duplicate_window_secs ({}), or be 0 for unlimited",
                    p.max_age_secs, p.duplicate_window_secs
                ));
            }
        }
        // (6) Not gated: a config that names a broker but never spawns the relay publishes
        // nothing while looking correct.
        if !self.outbox.relay_enabled && self.outbox.publisher.backend == PublisherBackend::Nats {
            return Err("outbox.relay_enabled = false with outbox.publisher.backend = \"nats\" would publish nothing — set backend = \"tracing\" or enable the relay".to_string());
        }
        // SMA-489 §3.4: `relay_enabled = false` with `wake_on_commit = true` is inert, not
        // invalid, so it stays an `Ok` here and is NOT diagnosed here either — `main.rs` calls
        // `validate()` BEFORE `paigasus_logging::init`, so a `warn!` emitted from this function
        // is written before the service logger exists and is silently lost. The diagnostic lives
        // in `main.rs` immediately after logging init instead (CodeRabbit round 1). Keep
        // `validate` free of logging side effects generally: it is a pure predicate over config.

        // --- SMA-469: `[outbox.retention]` config ----------------------------------------------
        // Same posture as the `[outbox]` checks directly above: each of these three is a divisor
        // of the sweep's own behavior. Both `*_days` sentinels are deliberately NOT validated
        // here — `0` is a legitimate "never delete" value for both `published_days` and
        // `parked_days` (see `OutboxRetentionConfig`'s doc), so any `u32` value is accepted.
        if self.outbox.retention.interval_secs == 0 {
            return Err("outbox.retention.interval_secs must be at least 1 (0 would busy-loop the sweep)".to_string());
        }
        if self.outbox.retention.batch_size == 0 {
            return Err("outbox.retention.batch_size must be at least 1 (0 would make every sweep pass delete nothing)".to_string());
        }
        if self.outbox.retention.max_batches_per_tick == 0 {
            return Err("outbox.retention.max_batches_per_tick must be at least 1 (0 would make every sweep tick do no passes)".to_string());
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
            assert_eq!(cfg.database_url.as_str(), "postgres://u:p@localhost/db");
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
            assert_eq!(cfg.authn.jwks_cache.redis_url.as_ref().map(RedactedUrl::as_str), Some("redis://localhost:6379"));
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
            assert_eq!(cfg.authz.cache.redis_url.as_ref().map(RedactedUrl::as_str), Some("redis://localhost:6379"));
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
            assert_eq!(cfg.api_keys.introspect_cache.redis_url.as_ref().map(RedactedUrl::as_str), Some("redis://localhost:6379"));
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
            assert_eq!(
                cfg.database_url.as_str(),
                "postgres://u:p@localhost/db",
                "the flat IAM_DATABASE_URL env var must still map to database_url"
            );
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

    /// Companion to `pepper_never_appears_in_debug_or_serialized_config`, for the two connection
    /// DSNs (SMA-489 CodeRabbit round 1). Both routinely carry a password, and `IamConfig`
    /// derives both `Debug` and `Serialize` — so `RedactedUrl` has to cover BOTH outbound
    /// directions for BOTH fields. `database_url` is in here alongside `listen_database_url` on purpose:
    /// redacting one while its identically-sensitive neighbour two fields away leaked would be
    /// incoherent.
    #[test]
    fn connection_urls_never_appear_in_debug_or_serialized_config() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://main_user:main_pw_secret@db.example.com/iam");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [api_keys]
                        pepper = "{}"

                        [outbox]
                        listen_database_url = "postgres://listen_user:listen_pw_secret@direct.example.com/iam"

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]
                    "#,
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;

            // Sanity: both REAL values round-tripped through figment, so the "must not contain"
            // assertions below cannot pass vacuously — and `as_str` still yields something usable
            // at the `Database::connect`/`PgOutboxListener::new` call sites.
            assert_eq!(cfg.database_url.as_str(), "postgres://main_user:main_pw_secret@db.example.com/iam");
            assert_eq!(
                cfg.outbox.listen_database_url.as_ref().map(RedactedUrl::as_str),
                Some("postgres://listen_user:listen_pw_secret@direct.example.com/iam")
            );

            let debugged = format!("{cfg:?}");
            assert!(!debugged.contains("main_pw_secret"), "database_url leaked into IamConfig's Debug output: {debugged}");
            assert!(!debugged.contains("listen_pw_secret"), "listen_database_url leaked into IamConfig's Debug output: {debugged}");
            assert!(!debugged.contains("db.example.com"), "database_url's host leaked into IamConfig's Debug output: {debugged}");
            assert!(!debugged.contains("direct.example.com"), "listen_database_url's host leaked into IamConfig's Debug output: {debugged}");

            let serialized = serde_json::to_string(&cfg).expect("IamConfig serializes");
            assert!(!serialized.contains("main_pw_secret"), "database_url leaked into IamConfig's serialized form: {serialized}");
            assert!(!serialized.contains("listen_pw_secret"), "listen_database_url leaked into IamConfig's serialized form: {serialized}");
            // The placeholder must actually be emitted IN PLACE — a field silently dropped from
            // the dump would also satisfy the two assertions above.
            assert!(serialized.contains(r#""database_url":"<redacted>""#), "{serialized}");
            assert!(serialized.contains(r#""listen_database_url":"<redacted>""#), "{serialized}");

            Ok(())
        });
    }

    /// The newtype itself, without figment in the way: `Debug` and `Serialize` both render the
    /// placeholder, and `as_str` is the one door out.
    #[test]
    fn redacted_url_renders_a_placeholder_in_both_outbound_directions() {
        let url = RedactedUrl::from("postgres://u:p@localhost/db");
        assert_eq!(format!("{url:?}"), r#"RedactedUrl("<redacted>")"#);
        assert_eq!(serde_json::to_string(&url).expect("RedactedUrl serializes"), r#""<redacted>""#);
        assert_eq!(url.as_str(), "postgres://u:p@localhost/db", "as_str must still yield the REAL url");
    }

    /// SMA-496. Companion to `connection_urls_never_appear_in_debug_or_serialized_config`
    /// above: a Redis connection string carries credentials exactly as a Postgres DSN does
    /// (`redis://user:pass@host:6379/0`), and so does the NATS broker url
    /// (`nats://user:pass@host`) — while `IamConfig` derives `Debug`/`Serialize`. So
    /// `RedactedUrl` has to cover BOTH outbound directions for all four of them.
    ///
    /// Each URL gets its own password and host so a leak names its own source.
    ///
    /// `[outbox.publisher]` deliberately leaves `backend` at its `tracing` default: `url` is
    /// redacted regardless of backend, and selecting `nats` would drag SMA-493's TLS and
    /// credentials-file validation rules into a test that is about redaction. The broker
    /// assertions here also passed BEFORE `url` became a `RedactedUrl` — they pinned the
    /// behaviour the hand-rolled `Debug`/`Serialize` impls provided, so that deleting those
    /// impls in favour of the derive had to preserve it exactly.
    #[test]
    fn cache_and_broker_urls_never_appear_in_debug_or_serialized_config() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://db_user:db_pw_secret@db.example.com/iam");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [api_keys]
                        pepper = "{}"

                        [authn.jwks_cache]
                        backend = "redis"
                        redis_url = "redis://jwks_user:jwks_pw_secret@jwks.example.com:6379/0"

                        [authz.cache]
                        backend = "redis"
                        redis_url = "redis://authz_user:authz_pw_secret@authz.example.com:6379/1"

                        [api_keys.introspect_cache]
                        backend = "redis"
                        redis_url = "redis://apikey_user:apikey_pw_secret@apikey.example.com:6379/2"

                        [outbox.publisher]
                        url = "tls://nats_user:nats_pw_secret@nats.example.com:4222"

                        [[authn.issuers]]
                        issuer = "https://idp.example.com/realms/acme"
                        audiences = ["paigasus"]
                    "#,
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;

            // Sanity: all three REAL values round-tripped through figment, so the "must not
            // contain" assertions below cannot pass merely because figment populated nothing —
            // and `as_str` still yields something usable at the `connect_redis` call sites.
            assert_eq!(
                cfg.authn.jwks_cache.redis_url.as_ref().map(RedactedUrl::as_str),
                Some("redis://jwks_user:jwks_pw_secret@jwks.example.com:6379/0")
            );
            assert_eq!(
                cfg.authz.cache.redis_url.as_ref().map(RedactedUrl::as_str),
                Some("redis://authz_user:authz_pw_secret@authz.example.com:6379/1")
            );
            assert_eq!(
                cfg.api_keys.introspect_cache.redis_url.as_ref().map(RedactedUrl::as_str),
                Some("redis://apikey_user:apikey_pw_secret@apikey.example.com:6379/2")
            );

            let debugged = format!("{cfg:?}");
            let serialized = serde_json::to_string(&cfg).expect("IamConfig serializes");

            // Hosts are asserted as EXACT names, never as a bare "example.com": the mandatory
            // issuer is `https://idp.example.com/...` and is deliberately NOT redacted, so a
            // blanket substring check would fail on it.
            for secret in [
                "jwks_pw_secret",
                "authz_pw_secret",
                "apikey_pw_secret",
                "nats_pw_secret",
                "jwks.example.com",
                "authz.example.com",
                "apikey.example.com",
                "nats.example.com",
            ] {
                assert!(!debugged.contains(secret), "{secret} leaked into IamConfig's Debug output: {debugged}");
                assert!(!serialized.contains(secret), "{secret} leaked into IamConfig's serialized form: {serialized}");
            }

            // The placeholder must land IN PLACE, and in the right NUMBER. A field silently
            // dropped from the dump satisfies the "must not contain" assertions above just as
            // well as a redacted one does, which is why this is a count and not a `contains`.
            assert_eq!(serialized.matches(r#""redis_url":"<redacted>""#).count(), 3, "{serialized}");
            assert_eq!(debugged.matches(r#"redis_url: Some(RedactedUrl("<redacted>"))"#).count(), 3, "{debugged}");
            // Cannot collide with the `redis_url` pattern above: matching `"url"` needs a quote
            // immediately before `u`, and in `"redis_url"` the preceding character is `_`.
            assert_eq!(serialized.matches(r#""url":"<redacted>""#).count(), 1, "{serialized}");

            Ok(())
        });
    }

    /// SMA-496 D6. The `*Defaults` structs feed figment's default LAYER (`Serialized::defaults`,
    /// see `IamConfig::figment`), and they mirror only their TOP-LEVEL struct — the nested ones
    /// are the REAL config types (`AuthzDefaults.cache` is an `AuthzCacheConfig`,
    /// `OutboxDefaults.publisher` a `PublisherConfig`). So a [`RedactedUrl`] whose default were
    /// `Some(_)` would serialize the literal `"<redacted>"` INTO that layer, and figment would
    /// then deserialize that string straight back out as the value: every deployment that did
    /// not override it would boot pointed at a host named `<redacted>`.
    ///
    /// `OutboxDefaults::listen_database_url` dodges this by being a plain `String` (its own
    /// comment says why); the four nested URLs are safe only because every default is `None`.
    /// This test is what keeps it that way.
    ///
    /// Asserting over `serde_json` rather than figment's own `Value` tree is valid because
    /// `RedactedUrl::serialize` is serializer-agnostic — it calls
    /// `serializer.serialize_str("<redacted>")` unconditionally — so it emits the placeholder
    /// into figment's tree exactly as it does into JSON. If that ever stops being true, this
    /// guard silently decouples from the hazard it guards.
    #[test]
    fn defaults_never_serialize_a_redaction_placeholder() {
        let layer = serde_json::to_string(&Defaults::default()).expect("Defaults serializes");
        assert!(
            !layer.contains("<redacted>"),
            "a RedactedUrl with a non-None default leaked the placeholder INTO figment's default layer, \
             which figment would then deserialize back out as the real value: {layer}"
        );
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

    #[test]
    fn validate_rejects_a_query_max_window_days_over_the_cap() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!("{}\n[api_keys]\npepper = \"{}\"\n[audit]\nquery_max_window_days = 36601", minimal_issuer_toml(), valid_pepper_b64()),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "query_max_window_days = 36601 must fail validation (over the ~100y cap)");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_a_query_default_window_wider_than_the_max() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!(
                    "{}\n[api_keys]\npepper = \"{}\"\n[audit]\nquery_default_window_days = 400\nquery_max_window_days = 100",
                    minimal_issuer_toml(),
                    valid_pepper_b64()
                ),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "query_default_window_days (400) > query_max_window_days (100) must fail validation");
            Ok(())
        });
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
            // SMA-471 D9: raised from 5 so a routine broker restart does not dead-letter the
            // in-flight backlog — see `outbox_max_attempts_defaults_to_sixty` for the dedicated test.
            assert_eq!(cfg.outbox.max_attempts, 60);
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

    // --- SMA-469: `[outbox.retention]` config -----------------------------------------------

    #[test]
    fn outbox_retention_defaults() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()))?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.outbox.retention.enabled, "retention must default to enabled");
            assert_eq!(cfg.outbox.retention.interval_secs, 3600);
            assert_eq!(cfg.outbox.retention.published_days, 7);
            assert_eq!(cfg.outbox.retention.parked_days, 0, "parked rows must NOT age out by default");
            assert_eq!(cfg.outbox.retention.batch_size, 1000);
            assert_eq!(cfg.outbox.retention.max_batches_per_tick, 50);
            assert!(cfg.validate().is_ok(), "outbox retention defaults alone should pass validation");
            Ok(())
        });
    }

    #[test]
    fn outbox_retention_rejects_zero_interval_batch_and_max_batches() {
        for mutate in [
            (|c: &mut IamConfig| c.outbox.retention.interval_secs = 0) as fn(&mut IamConfig),
            |c: &mut IamConfig| c.outbox.retention.batch_size = 0,
            |c: &mut IamConfig| c.outbox.retention.max_batches_per_tick = 0,
        ] {
            figment::Jail::expect_with(|jail| {
                jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
                jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()))?;
                let mut cfg: IamConfig = IamConfig::figment().extract()?;
                mutate(&mut cfg);
                assert!(cfg.validate().is_err(), "expected a zero retention knob to fail validation");
                Ok(())
            });
        }
    }

    #[test]
    fn outbox_retention_allows_zero_day_windows_meaning_never() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()))?;
            let mut cfg: IamConfig = IamConfig::figment().extract()?;
            cfg.outbox.retention.published_days = 0;
            cfg.outbox.retention.parked_days = 0;
            assert!(cfg.validate().is_ok(), "0 days must be valid — it is the 'never delete' sentinel");
            Ok(())
        });
    }

    #[test]
    fn outbox_retention_partial_toml_override_merges_with_defaults() {
        // Real figment-merge exercise (not an in-memory struct mutation): a `[outbox.retention]`
        // block that specifies only SOME keys must still land the REST on their documented
        // defaults via figment's merge — the actual production config-loading path, and the
        // single most common real-world config mistake if the defaults layer regresses.
        //
        // Also covers a nesting question: `[outbox]` carries its OWN `batch_size` (relay drain
        // batch) and `[outbox.retention]` carries a DIFFERENT `batch_size` (sweep delete batch).
        // Setting both in the same file proves TOML's table nesting keeps them distinct — no
        // collision, no cross-contamination in either direction.
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!(
                    r#"
                        [outbox]
                        batch_size = 250

                        [outbox.retention]
                        published_days = 30
                        max_batches_per_tick = 200

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

            // The two explicitly-configured retention keys took their configured values.
            assert_eq!(cfg.outbox.retention.published_days, 30, "published_days must take the configured override");
            assert_eq!(cfg.outbox.retention.max_batches_per_tick, 200, "max_batches_per_tick must take the configured override");

            // Every UNSPECIFIED retention key must still land on its documented default,
            // enumerated individually — asserting against `OutboxRetentionConfig::default()`
            // wholesale would still pass even if the defaults layer were bypassed entirely
            // (serde's own `#[derive(Default)]`-shaped fallback could paper over that), so each
            // field is checked against its literal documented value instead.
            assert!(cfg.outbox.retention.enabled, "enabled must still default to true when not overridden");
            assert_eq!(cfg.outbox.retention.interval_secs, 3600, "interval_secs must still default to 3600 when not overridden");
            assert_eq!(cfg.outbox.retention.parked_days, 0, "parked_days must still default to 0 when not overridden");
            assert_eq!(cfg.outbox.retention.batch_size, 1000, "retention.batch_size must still default to 1000 when not overridden");

            // The outbox-level `batch_size` (relay drain) and the retention-level `batch_size`
            // (sweep delete) are distinct TOML tables and must not collide in either direction.
            assert_eq!(cfg.outbox.batch_size, 250, "outbox.batch_size (relay) must take its own override, unaffected by [outbox.retention]");

            assert!(
                cfg.validate().is_ok(),
                "a partial [outbox.retention] override merged with the rest of the defaults should pass validation"
            );
            Ok(())
        });
    }

    // --- SMA-471: `[outbox.publisher]` config -----------------------------------------------

    /// SMA-471 test helper: loads an `IamConfig` from `extra_toml` layered on top of the same
    /// minimal valid base (one issuer + a valid `[api_keys]` pepper) every `[outbox]`/
    /// `[outbox.retention]`/`[metrics]` test above already builds by hand via
    /// `format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64())` —
    /// same `Jail`/`IamConfig::figment()` construction idiom, just factored out because every
    /// `[outbox.publisher]` test below needs it.
    fn load_config_with(extra_toml: &str) -> IamConfig {
        let mut loaded = None;
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"\n{extra_toml}", minimal_issuer_toml(), valid_pepper_b64()))?;
            loaded = Some(IamConfig::figment().extract()?);
            Ok(())
        });
        loaded.expect("figment extraction to succeed for a well-formed test fixture")
    }

    /// The minimal valid config with no `[outbox.publisher]` override — every publisher field on
    /// its documented default.
    fn load_minimal_config() -> IamConfig {
        load_config_with("")
    }

    /// Loads `extra_toml` on top of the minimal base and returns `validate()`'s result directly.
    fn validate_result(extra_toml: &str) -> Result<(), String> {
        load_config_with(extra_toml).validate()
    }

    /// Loads `extra_toml`, asserts validation failed, and returns the error string.
    fn validate_err(extra_toml: &str) -> String {
        validate_result(extra_toml).expect_err("expected this fixture to fail validation")
    }

    #[test]
    fn outbox_publisher_defaults_are_the_tracing_backend() {
        let cfg = load_minimal_config();
        assert_eq!(cfg.outbox.publisher.backend, PublisherBackend::Tracing);
        assert_eq!(cfg.outbox.publisher.url, None);
        assert_eq!(cfg.outbox.publisher.credentials_file, None);
        assert_eq!(cfg.outbox.publisher.stream, "IAM_EVENTS");
        assert_eq!(cfg.outbox.publisher.source, "urn:paigasus:iam");
        assert_eq!(cfg.outbox.publisher.publish_timeout_secs, 2);
        assert_eq!(cfg.outbox.publisher.duplicate_window_secs, 3_600);
        assert_eq!(cfg.outbox.publisher.max_age_secs, 604_800);
    }

    /// D9: raised from 5 so a routine broker restart does not dead-letter the in-flight backlog.
    #[test]
    fn outbox_max_attempts_defaults_to_sixty() {
        assert_eq!(load_minimal_config().outbox.max_attempts, 60);
    }

    // --- SMA-489: outbox wake-on-commit config ---------------------------------------------

    #[test]
    fn outbox_wake_defaults_are_on_with_a_200ms_debounce() {
        let cfg = load_minimal_config();
        assert!(cfg.outbox.wake_on_commit, "the nudge is on by default (SMA-489 D11)");
        assert_eq!(cfg.outbox.wake_debounce_ms, 200, "SMA-489 D14 default");
        assert_eq!(cfg.outbox.listen_database_url, None, "falls back to database_url");
    }

    #[test]
    fn zero_wake_debounce_is_rejected() {
        let err = validate_err(
            r#"
            [outbox]
            wake_debounce_ms = 0
        "#,
        );
        assert!(err.contains("wake_debounce_ms"), "{err}");
    }

    #[test]
    fn nats_backend_requires_a_url() {
        let err = validate_err(
            r#"
            [outbox.publisher]
            backend = "nats"
        "#,
        );
        assert!(err.contains("outbox.publisher.url"), "{err}");
    }

    /// D10: the floor is necessary-not-sufficient, and the message must name all three fields.
    #[test]
    fn duplicate_window_must_exceed_the_retry_span() {
        let err = validate_err(
            r#"
            [outbox]
            max_attempts = 60
            poll_interval_secs = 5
            [outbox.publisher]
            backend = "nats"
            url = "tls://localhost:4222"
            credentials_file = "/etc/paigasus/iam.creds"
            duplicate_window_secs = 100
        "#,
        );
        assert!(err.contains("duplicate_window_secs"), "{err}");
        assert!(err.contains("max_attempts"), "{err}");
        assert!(err.contains("poll_interval_secs"), "{err}");
    }

    /// Strict `>`: equality is REJECTED, one second more is accepted.
    #[test]
    fn duplicate_window_boundary_is_exclusive() {
        let at = r#"
            [outbox]
            max_attempts = 10
            poll_interval_secs = 5
            [outbox.publisher]
            backend = "nats"
            url = "tls://localhost:4222"
            credentials_file = "/etc/paigasus/iam.creds"
            duplicate_window_secs = 50
            max_age_secs = 0
        "#;
        assert!(validate_result(at).is_err(), "equality must be rejected");
        assert!(validate_result(&at.replace("duplicate_window_secs = 50", "duplicate_window_secs = 51")).is_ok());
    }

    /// A `u32::MAX` max_attempts must be rejected, not overflow-panic in the product.
    #[test]
    fn a_huge_max_attempts_is_rejected_not_panicking() {
        let err = validate_err(
            r#"
            [outbox]
            max_attempts = 4294967295
            poll_interval_secs = 3600
            [outbox.publisher]
            backend = "nats"
            url = "tls://localhost:4222"
            credentials_file = "/etc/paigasus/iam.creds"
            duplicate_window_secs = 3600
        "#,
        );
        assert!(err.contains("duplicate_window_secs"), "{err}");
    }

    /// D10: the floor is gated on the backend — a tracing deployment must not fail boot over NATS.
    #[test]
    fn the_window_floor_does_not_apply_to_the_tracing_backend() {
        assert!(
            validate_result(
                r#"
            [outbox]
            max_attempts = 60
            poll_interval_secs = 5
            [outbox.publisher]
            backend = "tracing"
            duplicate_window_secs = 1
        "#
            )
            .is_ok()
        );
    }

    /// D8: JetStream requires duplicate_window <= max_age when max_age > 0. 0 means unlimited.
    #[test]
    fn max_age_must_exceed_the_duplicate_window_unless_unlimited() {
        let base = r#"
            [outbox.publisher]
            backend = "nats"
            url = "tls://localhost:4222"
            credentials_file = "/etc/paigasus/iam.creds"
            duplicate_window_secs = 3600
        "#;
        assert!(validate_result(&format!("{base}\nmax_age_secs = 1800")).is_err());
        assert!(validate_result(&format!("{base}\nmax_age_secs = 0")).is_ok(), "0 = unlimited");
        assert!(validate_result(&format!("{base}\nmax_age_secs = 7200")).is_ok());
    }

    /// Strict `>`: equality is REJECTED, one second more is accepted — pins the D8 boundary the
    /// same way `duplicate_window_boundary_is_exclusive` pins the retry-span rule's boundary
    /// above. The base test's 1800/0/7200 fixtures never exercise exactly
    /// `max_age_secs == duplicate_window_secs`, so a `<=` weakened to `<` would still pass all
    /// three.
    #[test]
    fn max_age_boundary_is_exclusive() {
        let at = r#"
            [outbox.publisher]
            backend = "nats"
            url = "tls://localhost:4222"
            credentials_file = "/etc/paigasus/iam.creds"
            duplicate_window_secs = 3600
            max_age_secs = 3600
        "#;
        assert!(validate_result(at).is_err(), "max_age_secs == duplicate_window_secs must be rejected");
        assert!(validate_result(&at.replace("max_age_secs = 3600", "max_age_secs = 3601")).is_ok());
    }

    /// `source` is copied verbatim into every published envelope, so a malformed value ships
    /// spec-violating CloudEvents to external consumers. The original check was "non-empty and
    /// no whitespace", which accepted `%` and `http://[` — both malformed URI-references
    /// (CodeRabbit, PR 112). These cases are the ones that discriminate a real parse from that
    /// hand-rolled approximation: whitespace alone would not.
    #[test]
    fn source_must_parse_as_an_absolute_uri() {
        for bad in ["my prod cluster", "%", "http://[", "", "paigasus/iam"] {
            let err = validate_err(&format!(
                r#"
                [outbox.publisher]
                backend = "nats"
                url = "tls://localhost:4222"
                credentials_file = "/etc/paigasus/iam.creds"
                source = "{bad}"
            "#
            ));
            assert!(err.contains("outbox.publisher.source"), "{bad:?} should have been rejected, got: {err}");
        }
    }

    /// Scheme coverage for the accepted side (CodeRabbit, PR 112). `url::Url::parse` implements
    /// the WHATWG URL Standard, which treats six "special" schemes (http/https/ws/wss/ftp/file)
    /// differently from everything else — so the cases that matter here are the NON-special ones,
    /// which a naive "must contain `://`" check would reject outright. `urn:` is the shipped
    /// default; `tag:`/`mailto:`/a bare custom scheme are the other realistic shapes an operator
    /// might reach for.
    #[test]
    fn every_realistic_absolute_uri_scheme_is_accepted() {
        for good in [
            "urn:paigasus:iam",                              // the shipped default
            "urn:uuid:6e8bc430-9c3a-11d9-9669-0800200c9a66", // non-special, opaque path
            "tag:paigasus.dev,2026:iam",                     // non-special, commas
            "mailto:ops@paigasus.dev",                       // non-special, '@' in path
            "paigasus:iam",                                  // bare custom scheme
            "https://paigasus.dev/iam",                      // special scheme
            "https://iam.eu-central-1.paigasus.dev",
            "nats://host:4222",
            "file:///x",
        ] {
            assert!(
                validate_result(&format!(
                    r#"
                    [outbox.publisher]
                    backend = "nats"
                    url = "tls://localhost:4222"
                    credentials_file = "/etc/paigasus/iam.creds"
                    source = "{good}"
                "#
                ))
                .is_ok(),
                "{good:?} is a valid absolute URI and must be accepted"
            );
        }
    }

    /// Validation must PARSE `source` without rewriting it. WHATWG normalization lowercases the
    /// scheme and host (`HTTPS://Paigasus.DEV/IAM` parses to `https://paigasus.dev/IAM`), and
    /// every published envelope carries this field verbatim — so if validation ever swapped in
    /// the normalized form, the `source` external consumers see would silently change on upgrade.
    /// D6 requires it to stay stable for the lifetime of a stream, which makes this load-bearing.
    #[test]
    fn validation_does_not_normalize_the_source_it_accepts() {
        let raw = "HTTPS://Paigasus.DEV/IAM";
        let cfg = load_config_with(&format!(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "tls://localhost:4222"
            credentials_file = "/etc/paigasus/iam.creds"
            source = "{raw}"
        "#
        ));
        assert!(cfg.validate().is_ok(), "the value must be accepted in the first place");
        assert_eq!(cfg.outbox.publisher.source, raw, "validation must not rewrite `source` into its WHATWG-normalized form");
    }

    /// A config that publishes nothing while claiming a broker must not boot silently.
    #[test]
    fn a_disabled_relay_with_the_nats_backend_is_rejected() {
        let err = validate_err(
            r#"
            [outbox]
            relay_enabled = false
            [outbox.publisher]
            backend = "nats"
            url = "tls://localhost:4222"
            credentials_file = "/etc/paigasus/iam.creds"
        "#,
        );
        assert!(err.contains("relay_enabled"), "{err}");
        assert!(err.contains("outbox.publisher.backend"), "{err}");
    }

    #[test]
    fn zero_timeout_and_zero_window_are_rejected() {
        for field in ["publish_timeout_secs", "duplicate_window_secs"] {
            let err = validate_err(&format!(
                r#"
                [outbox.publisher]
                backend = "nats"
                url = "tls://localhost:4222"
                credentials_file = "/etc/paigasus/iam.creds"
                {field} = 0
            "#
            ));
            assert!(err.contains(field), "{field}: {err}");
        }
    }

    #[test]
    fn the_publisher_url_is_redacted_in_debug() {
        let cfg = PublisherConfig {
            url: Some("nats://user:hunter2@host:4222".into()),
            ..PublisherConfig::default()
        };
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("hunter2"), "credentials leaked into Debug: {rendered}");
        // In place, not merely present: a `url` dropped from the output entirely would satisfy
        // a bare `contains("redacted")` just as well as a redacted one does.
        assert!(rendered.contains(r#"url: Some(RedactedUrl("<redacted>"))"#), "{rendered}");
    }

    /// Companion to `the_publisher_url_is_redacted_in_debug`, for the other outbound direction:
    /// a credentialed `url` must not leak into a serialized config dump either. Both directions
    /// are ordinary derives since SMA-496 — the redaction rides on `url`'s [`RedactedUrl`] type
    /// rather than on the hand-rolled impls this struct used to carry (`serde_json` is already a
    /// regular, non-dev dependency of this crate; see `pepper_never_appears_in_debug_or_
    /// serialized_config` above for the identical pattern applied to `api_keys.pepper`).
    #[test]
    fn the_publisher_url_is_redacted_in_serialize() {
        let cfg = PublisherConfig {
            url: Some("nats://user:hunter2@host:4222".into()),
            ..PublisherConfig::default()
        };
        let serialized = serde_json::to_string(&cfg).expect("PublisherConfig serializes");
        assert!(!serialized.contains("hunter2"), "credentials leaked into Serialize: {serialized}");
        assert!(serialized.contains(r#""url":"<redacted>""#), "{serialized}");
    }

    // --- SMA-493: transport + credential posture ---------------------------------------------

    /// D6 rule 1. The default posture: a `nats` backend must speak TLS.
    #[test]
    fn a_plaintext_url_is_rejected() {
        let err = validate_err(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "nats://localhost:4222"
            credentials_file = "/etc/paigasus/iam.creds"
        "#,
        );
        assert!(err.contains("tls://"), "{err}");
        assert!(err.contains("allow_insecure_broker"), "the message must name the escape hatch: {err}");
    }

    #[test]
    fn a_plaintext_url_is_accepted_with_the_insecure_flag() {
        validate_result(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "nats://localhost:4222"
            allow_insecure_broker = true
        "#,
        )
        .expect("the explicit dev/CI escape hatch must be honoured");
    }

    /// D6 rule 2, unconditional. async-nats never reads url userinfo (`lib.rs:1682` has no
    /// caller), so accepting it would let a config that LOOKS authenticated connect anonymously.
    #[test]
    fn url_embedded_credentials_are_rejected_even_with_the_insecure_flag() {
        let err = validate_err(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "nats://user:pass@localhost:4222"
            allow_insecure_broker = true
        "#,
        );
        assert!(err.contains("credentials_file"), "{err}");
    }

    /// D6 rules 1+2, hardened: `url::Url::parse` treats a schemeless form like
    /// `tls:user@host:4222` (missing `//`) as a cannot-be-a-base URL with scheme "tls" and
    /// EMPTY userinfo — passing a naive `parsed.scheme() == "tls"` check and the embedded-
    /// credentials check above it, even though async-nats' own parser requires `://` to
    /// recognize a scheme at all and would actually dial this as `nats://tls:user@host:4222`:
    /// plaintext, with `tls`/`user` silently discarded. The raw-string check must catch it.
    #[test]
    fn a_schemeless_url_masquerading_as_tls_is_rejected() {
        let err = validate_err(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "tls:user@host:4222"
            credentials_file = "/etc/paigasus/iam.creds"
        "#,
        );
        assert!(err.contains("tls://"), "{err}");
        assert!(err.contains("allow_insecure_broker"), "the message must name the escape hatch: {err}");
    }

    /// D6 rule 3.
    #[test]
    fn the_nats_backend_requires_a_credentials_file() {
        let err = validate_err(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "tls://localhost:4222"
        "#,
        );
        assert!(err.contains("credentials_file"), "{err}");
    }

    #[test]
    fn a_tls_url_with_credentials_passes() {
        validate_result(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "tls://nats.internal:4222"
            credentials_file = "/etc/paigasus/iam.creds"
            root_ca_bundle = "/etc/paigasus/nats-ca.pem"
            inbox_prefix = "_INBOX_IAM_PUB"
        "#,
        )
        .expect("the documented production shape must validate");
    }

    /// Regression: `url::Url::parse` lowercases the scheme per the WHATWG URL Standard, and so
    /// does async-nats' own `ServerAddr::from_url`, which compares against that same lowercased
    /// `Url::scheme()` — so async-nats itself dials `TLS://...` over TLS. The raw-string check
    /// (added to close the schemeless-masquerade gap covered by
    /// `a_schemeless_url_masquerading_as_tls_is_rejected` above) must stay case-insensitive on
    /// the scheme portion, or an operator would be forced to reach for `allow_insecure_broker`
    /// — the dev/CI escape hatch — over a mere casing mismatch.
    #[test]
    fn an_uppercase_tls_scheme_is_accepted() {
        validate_result(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "TLS://localhost:4222"
            credentials_file = "/etc/paigasus/iam.creds"
        "#,
        )
        .expect("the scheme check must be case-insensitive, matching async-nats' own comparison");
    }

    /// Regression: the rejection message must never echo the raw url. A malformed url with no
    /// `://` but baked-in credentials (`user:hunter2@host:4222`) parses via `url::Url` as an
    /// opaque scheme "user" with EMPTY username/password, so the unconditional embedded-
    /// credentials check above does NOT catch it — the raw string, password included, must not
    /// then leak into the returned (and potentially logged) validation error. `url` wears
    /// `RedactedUrl` for the same reason (see `the_publisher_url_is_redacted_in_serialize`
    /// above) — but a newtype cannot protect a string that validation interpolates by hand,
    /// which is exactly what this test exists to catch.
    #[test]
    fn a_rejected_url_with_a_password_does_not_leak_it_into_the_error() {
        let err = validate_err(
            r#"
            [outbox.publisher]
            backend = "nats"
            url = "user:hunter2@host:4222"
            credentials_file = "/etc/paigasus/iam.creds"
        "#,
        );
        assert!(!err.contains("hunter2"), "password leaked into validation error: {err}");
        assert!(err.contains("tls://"), "{err}");
        assert!(err.contains("allow_insecure_broker"), "the message must name the escape hatch: {err}");
    }

    /// Every SMA-493 rule is gated on the `nats` backend: a `tracing` deployment must never fail
    /// boot over a broker it does not run.
    #[test]
    fn the_tracing_backend_is_unaffected_by_the_transport_rules() {
        validate_result(
            r#"
            [outbox.publisher]
            backend = "tracing"
            url = "nats://user:pass@localhost:4222"
        "#,
        )
        .expect("the tracing backend ignores publisher transport posture entirely");
    }

    #[test]
    fn the_new_publisher_fields_default_to_absent() {
        let cfg = load_minimal_config();
        assert_eq!(cfg.outbox.publisher.root_ca_bundle, None);
        assert_eq!(cfg.outbox.publisher.inbox_prefix, None);
        assert!(!cfg.outbox.publisher.allow_insecure_broker);
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
