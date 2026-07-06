# SMA-443: M2 Authentication (BYO-IdP / OIDC) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provider-agnostic OIDC authentication for `paigasus-iam`: `Authenticator` port, OIDC validator (discovery + JWKS cache/rotation), `ExternalIdentity` mapping + JIT provisioning, `AuthnService.Introspect` (HTTP + gRPC), and bearer-token enforcement on the whole existing API surface.

**Architecture:** Pure domain types + ports in `paigasus-iam-core`; the OIDC validator, JWKS caches, Postgres identity repo, error funnels, and middleware are adapters in `services/paigasus-iam` (hexagonal, generic-by-value DI — no `Arc<dyn>`). Wire surface is proto-first (`contracts/` → regenerated `paigasus-proto`). Spec: `docs/superpowers/specs/2026-07-06-sma-443-m2-authentication-design.md` — read it before starting any task; decisions D1–D15 are binding.

**Tech Stack:** Rust edition 2024 / 1.95, axum 0.8, tonic 0.14, SeaORM 1, figment, `jsonwebtoken` (new), `reqwest` (first consumer), `redis` (new), testcontainers 0.27, `p256` (dev-only, mock IdP keys).

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Run all commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` prefixed.
- Test command: `cargo nextest run -p <crate> [filter]` from `rs/` (`--no-tests=pass` if a target has no tests).
- Lint gate: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` must stay green after every task.
- Commits: conventional, e.g. `feat(rs): …`, body must not contain `#NNN` refs; commit from the worktree root.
- Never name a file with a Windows reserved device basename (`con`, `prn`, `aux`, `nul`, `com1..9`, `lpt1..9`).
- No token material, claim values, or upstream error text in logs or wire error messages. Log defect kinds / `kid` / issuer only.
- µs-truncated timestamps come from the `Clock` port (never `Utc::now()` in domain/application code).
- New deps may require `rs/deny.toml` updates; run `moon run repo:deny` after adding any.

---

### Task 1: Core authn domain types (`paigasus-iam-core::authn`)

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/src/authn.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/src/lib.rs` (add `pub mod authn;` + re-exports, mirroring how `tenancy` is re-exported)
- Modify: `rs/crates/libs/paigasus-iam-core/src/value.rs` (add `DomainError::InvalidIssuer(String)` variant)

**Interfaces:**
- Produces: `Issuer` (`parse(&str) -> Result<Issuer, DomainError>`, `as_str()`, `Display`, `Clone/PartialEq/Eq/Hash`), `ValidatedClaims`, `AuthnPrincipal`, `PrincipalContext`, `ExternalIdentity`, `TokenDefect`, `ProvisioningDefect`, `AuthnError` — all re-exported from the crate root like the existing types.

- [ ] **Step 1: Write failing unit tests** in `authn.rs` `#[cfg(test)]`:

```rust
#[test]
fn issuer_accepts_https_urls_verbatim() {
    let i = Issuer::parse("https://idp.example.com/realms/acme").unwrap();
    assert_eq!(i.as_str(), "https://idp.example.com/realms/acme");
    // No normalization: trailing slash is a DIFFERENT issuer (exact-match rule, spec §3.1).
    let j = Issuer::parse("https://idp.example.com/realms/acme/").unwrap();
    assert_ne!(i, j);
}

#[test]
fn issuer_rejects_non_https_fragments_and_garbage() {
    for bad in ["", "http://idp.example.com", "idp.example.com", "https://", "https://idp.example.com/#frag", "not a url"] {
        assert!(Issuer::parse(bad).is_err(), "expected {bad:?} rejected");
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo nextest run -p paigasus-iam-core issuer_` → FAIL (module missing).

- [ ] **Step 3: Implement.** `Issuer(String)` newtype. `parse`: trim; must start with `https://`; parse-lite validation without a URL crate: after the scheme there must be a non-empty host segment (at least one char before any `/`), and the string must contain no `#`, no whitespace. Store the trimmed string verbatim. Then the data types (all `Debug + Clone + PartialEq`):

```rust
pub struct ValidatedClaims {
    pub issuer: Issuer, pub subject: String, pub audiences: Vec<String>,
    pub expires_at: DateTime<Utc>,
    pub email: Option<String>, pub name: Option<String>,
    pub locale: Option<String>, pub zoneinfo: Option<String>,
}
pub struct AuthnPrincipal {
    pub principal_id: PrincipalId, pub kind: PrincipalKind, pub status: PrincipalStatus,
    pub issuer: Issuer, pub subject: String, pub expires_at: DateTime<Utc>,
}
pub struct PrincipalContext {
    pub principal: AuthnPrincipal,
    pub memberships: Vec<MembershipRecord>,
    pub role_groups: Vec<Prn>, // empty until M3
}
pub struct ExternalIdentity {
    pub id: Uuid, pub principal_id: PrincipalId, pub issuer: Issuer, pub subject: String,
    pub created_at: DateTime<Utc>, pub updated_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenDefect { Malformed, UnsupportedAlg, UnknownKid, BadSignature, Expired, IssuerNotConfigured, AudienceMismatch, Oversized }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisioningDefect { MissingEmail, EmailConflict }
#[derive(Debug, thiserror::Error)]
pub enum AuthnError {
    #[error("invalid token: {0:?}")] InvalidToken(TokenDefect),
    #[error("identity not provisioned")] IdentityNotProvisioned,
    #[error("provisioning failed: {0:?}")] ProvisioningFailed(ProvisioningDefect),
    #[error("principal inactive")] PrincipalInactive,
    #[error("authentication backend unavailable")] Unavailable,
    #[error("backend error")] Backend(#[from] Box<dyn std::error::Error + Send + Sync>),
}
```

(`TokenDefect` detail is for logs/tests only — spec §3.3.)

- [ ] **Step 4: Run** `cargo nextest run -p paigasus-iam-core` → PASS; clippy + fmt clean.
- [ ] **Step 5: Commit** `feat(rs): add core authn domain types for m2 oidc (SMA-443)`

---

### Task 2: Core ports + id generator extension

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/ports.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/id.rs` (`KernelIdGenerator`)
- Modify: `rs/crates/services/paigasus-iam/src/application/create_user.rs` (test `FixedIdGenerator`)
- Modify: `rs/crates/services/paigasus-iam/src/application/fakes.rs` (any `IdGenerator` fake there)

**Interfaces:**
- Consumes: Task 1 types.
- Produces (in `ports.rs`, re-exported at crate root):

```rust
#[async_trait]
pub trait Authenticator: Send + Sync {
    /// The pluggable port (ADR-0015). OIDC validator is the v1 impl.
    async fn authenticate(&self, token: &str) -> Result<ValidatedClaims, AuthnError>;
}
#[async_trait]
pub trait ExternalIdentityRepository: Send + Sync {
    async fn find_by_issuer_subject(&self, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError>;
    /// One transaction spanning principal + user + external_identity (D9).
    async fn provision(&self, principal: &Principal, user: &User, identity: &ExternalIdentity) -> Result<(), RepositoryError>;
}
```

  plus: `PrincipalRepository::find_principal(&self, id: &PrincipalId) -> Result<Option<Principal>, RepositoryError>` (new method on the existing trait), `IdGenerator::new_external_identity_id(&self) -> Uuid`, and `ConflictKind::ExternalIdentityExists` variant.

- [ ] **Step 1: Failing compile/tests.** Add the traits/methods; extend the object-safety test in `ports.rs` to include `&dyn ExternalIdentityRepository` and `&dyn Authenticator`. Build fails until every `IdGenerator`/`PrincipalRepository` impl is updated.
- [ ] **Step 2:** `cargo build -p paigasus-iam-core -p paigasus-iam` → expected compile errors listing all impl sites.
- [ ] **Step 3: Implement.** `KernelIdGenerator::new_external_identity_id` mirrors `new_membership_id` (SystemTime ms + 10 random bytes → `mint_uuid7`). Add `new_external_identity_id` to every test fake (`FixedIdGenerator` in `create_user.rs` returns `self.0`; same for any fake in `fakes.rs`). Implement `find_principal` on all `PrincipalRepository` impls found via `grep -rn "impl PrincipalRepository" rs/` (Pg impl comes in Task 3 — for now in `pg_repository.rs` implement it as `principal::Entity::find_by_id(...)` mapping only the principal row; in-memory fakes map from their stored `(Principal, User)` tuple).
- [ ] **Step 4:** `cargo nextest run -p paigasus-iam-core -p paigasus-iam` → PASS; clippy/fmt clean.
- [ ] **Step 5: Commit** `feat(rs): add authenticator + external identity ports (SMA-443)`

---

### Task 3: Migration m0003 + Pg external-identity repository

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0003_create_external_identity.rs`
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/entities/external_identity.rs`
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_external_identities.rs`
- Modify: `migration/mod.rs` (register m0003), `entities/mod.rs`, `persistence/mod.rs` (export + `conflict_kind` mapping)
- Test: `rs/crates/services/paigasus-iam/tests/authn_identities.rs`

**Interfaces:**
- Consumes: Task 2 ports; existing `map_err`/`conflict_kind` (`persistence/mod.rs`), `start_migrated_postgres()` (tests/support).
- Produces: `PgExternalIdentityRepository::new(db: DatabaseConnection)` implementing `ExternalIdentityRepository`.

- [ ] **Step 1: Write failing integration tests** (`tests/authn_identities.rs`, `mod support;` + `start_migrated_postgres`):
  - `provision_creates_principal_user_and_identity_atomically`: provision → `find_by_issuer_subject` returns identity; `find_user` returns the user.
  - `duplicate_identity_conflicts`: second provision with same `(issuer, subject)` but fresh principal/email → `RepositoryError::Conflict(ConflictKind::ExternalIdentityExists)`; **assert no orphan**: the second principal id does not exist (`find_principal` → `None`).
  - `email_conflict_rolls_back_everything`: provision A ok; provision B (new identity, same email) → `Conflict(EmailTaken)`; assert B's identity absent AND B's principal absent (atomicity, D9).
  - `constraint_names_are_stable`: like `tenancy_schema.rs`, query `pg_constraint`/`pg_indexes` for `uq_external_identity_issuer_subject`, `fk_external_identity_principal`, `ix_external_identity_principal`.
- [ ] **Step 2:** `cargo nextest run -p paigasus-iam authn_identities` → FAIL (types missing).
- [ ] **Step 3: Implement.**
  - Migration table `external_identity` per spec §5.1: `id uuid PK`, `principal_id uuid NOT NULL` + `ForeignKey::create().name("fk_external_identity_principal")` (no cascade), `issuer text NOT NULL`, `subject text NOT NULL`, `created_at/updated_at timestamptz NOT NULL`, `Index::create().name("uq_external_identity_issuer_subject").unique()` on (issuer, subject), `Index::create().name("ix_external_identity_principal")` on principal_id. Follow `m0002_create_tenancy.rs` style (named everything).
  - `conflict_kind`: add `else if msg.contains("uq_external_identity_issuer_subject") { ConflictKind::ExternalIdentityExists }` + extend its unit test.
  - `PgExternalIdentityRepository::provision`: one `self.db.begin()`; insert principal, user, external_identity (reuse the ActiveModel mapping style from `pg_repository.rs::create_user`); commit. `find_by_issuer_subject`: filter on both columns, map row → `ExternalIdentity` (parse stored principal prn via `principal` join? No — store only the uuid; reconstruct `PrincipalId` by loading the principal row's `prn` column with a second query inside the same call, or store nothing PRN-shaped: fetch `principal.prn` via `principal::Entity::find_by_id`. Keep it two queries, no join needed.)
- [ ] **Step 4:** `cargo nextest run -p paigasus-iam authn_identities` → PASS (Docker needed; skip-if-absent locally).
- [ ] **Step 5: Commit** `feat(rs): persist external identities with atomic jit provisioning (SMA-443)`

---

### Task 4: Authn configuration

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs`
- Modify: `rs/crates/services/paigasus-iam/iam.toml.example`

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AuthnConfig {
    pub leeway_secs: u64,                 // default 60
    pub http_timeout_secs: u64,           // default 10
    pub jwks_ttl_secs: u64,               // default 3600
    pub jwks_refresh_cooldown_secs: u64,  // default 30
    pub max_token_bytes: usize,           // default 16384
    pub jwks_cache: JwksCacheConfig,
    pub issuers: Vec<IssuerConfig>,
}
#[derive(...)] pub struct JwksCacheConfig { pub backend: JwksCacheBackend, pub redis_url: Option<String> } // backend default Memory
#[derive(...)] #[serde(rename_all = "lowercase")] pub enum JwksCacheBackend { Memory, Redis }
#[derive(...)] pub struct IssuerConfig { pub issuer: String, pub audiences: Vec<String>, pub jit_provisioning: bool /* serde default = true */ }
```

  `IamConfig` gains `pub authn: AuthnConfig`; `IamConfig::validate(&self) -> Result<(), String>` enforcing spec §6.4: ≥1 issuer, unique issuers, non-empty audiences per issuer, each `issuer` passes `Issuer::parse`, `Redis` backend ⇒ `redis_url` present. `main.rs` calls `validate()` after `load()` and exits with the message on `Err`.

- [ ] **Step 1: Failing tests** (figment `Jail`, existing module style): defaults land (`leeway_secs == 60` etc. with a minimal `[[authn.issuers]]` block written via `jail.create_file("iam.toml", ...)`); validation rejects: empty issuers, duplicate issuer, empty audiences, `http://` issuer, redis-without-url. `jit_provisioning` defaults to `true`.
- [ ] **Step 2:** run → FAIL. **Step 3:** implement (defaults via nested `Serialized::defaults` or `#[serde(default = ...)]` fns — issuers has NO default, like `database_url`). Update `iam.toml.example` with a commented `[authn]` block including the D15 Redis trust note ("dedicated instance, TLS + AUTH — a writable JWKS cache is an auth bypass").
- [ ] **Step 4:** `cargo nextest run -p paigasus-iam config` → PASS. **Step 5: Commit** `feat(rs): add authn issuer + jwks cache configuration (SMA-443)`

---

### Task 5: `AuthenticateToken` use case (JIT + introspection, fakes only)

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs`
- Modify: `rs/crates/services/paigasus-iam/src/application/mod.rs`

**Interfaces:**
- Consumes: core ports (Task 2), `ValidatedClaims`/`AuthnError`/etc. (Task 1).
- Produces:

```rust
#[derive(Clone, Copy, PartialEq, Eq)] pub enum Provisioning { Enabled, Disabled }
#[derive(Clone)] pub struct JitPolicy { /* map issuer string -> jit flag, built from &[IssuerConfig] */ }
impl JitPolicy { pub fn from_issuers(issuers: &[(Issuer, bool)]) -> Self; pub fn allows(&self, issuer: &Issuer) -> bool; }
#[derive(Clone)]
pub struct AuthenticateToken<A, E, P, M, I, C> { /* authenticator, identities, principals, memberships, id_gen, clock, jit: JitPolicy */ }
impl<A: Authenticator, E: ExternalIdentityRepository, P: PrincipalRepository, M: MembershipRepository, I: IdGenerator, C: Clock> AuthenticateToken<A, E, P, M, I, C> {
    pub fn new(authenticator: A, identities: E, principals: P, memberships: M, id_gen: I, clock: C, jit: JitPolicy) -> Self;
    pub async fn resolve(&self, token: &str, provisioning: Provisioning) -> Result<AuthnPrincipal, AuthnError>;
    pub async fn introspect(&self, token: &str) -> Result<PrincipalContext, AuthnError>;
}
```

  Logic per spec §6.1/§6.2 exactly. JIT profile mapping: `email` required (`Email::parse` failure or absence → `ProvisioningFailed(MissingEmail)`), `display_name = name.unwrap_or(email local part)`, locale/zoneinfo passthrough. Race: on `Conflict(ExternalIdentityExists)` from `provision`, re-read `find_by_issuer_subject` and continue with the winner (if the re-read is `None`, return `Backend`). `Conflict(EmailTaken)` → `ProvisioningFailed(EmailConflict)`. Other `RepositoryError` → `Backend`. `introspect` = `resolve(token, Provisioning::Disabled)` + memberships via `list_by_principal` paged loop (limit 200, offset += 200 until short page).

- [ ] **Step 1: Failing unit tests** with local fakes (follow `create_user.rs` test-module style; write a `FakeAuthenticator { result: Result<ValidatedClaims, AuthnError> }`, in-memory identity/principal/membership fakes):
  - `known_identity_resolves_without_provisioning`
  - `unknown_identity_jit_provisions_user` (asserts created user email/display_name from claims; asserts identity row exists)
  - `jit_disabled_issuer_returns_identity_not_provisioned` (JitPolicy flag false)
  - `introspect_never_provisions` (unknown identity + jit-enabled issuer → `IdentityNotProvisioned`, D10)
  - `missing_email_fails_provisioning`, `email_conflict_maps_to_provisioning_failed` (fake returns `Conflict(EmailTaken)`)
  - `provision_race_loser_reuses_winner_row` (fake `provision` returns `Conflict(ExternalIdentityExists)` once, `find_by_issuer_subject` then returns the winner)
  - `introspect_pages_through_memberships` (fake with 450 records; assert all 450 returned)
  - `invalid_token_short_circuits` (FakeAuthenticator err → same err, no repo calls)
- [ ] **Step 2:** run → FAIL. **Step 3:** implement. **Step 4:** `cargo nextest run -p paigasus-iam authenticate_token` → PASS.
- [ ] **Step 5: Commit** `feat(rs): add authenticate-token use case with jit provisioning (SMA-443)`

---

### Task 6: JWKS provider (fetch + cache + rotation)

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/oidc/mod.rs` (`pub mod jwks; pub mod validator;` — validator lands Task 7, keep a stub `pub mod jwks;` only for now)
- Create: `rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/mod.rs`, `rs/Cargo.toml` + service `Cargo.toml` (deps: `jsonwebtoken`, `serde_json` already present; consume workspace `reqwest`)

**Interfaces:**
- Produces:

```rust
pub struct CachedJwks { pub jwks: jsonwebtoken::jwk::JwkSet, pub jwks_uri: String, pub fetched_at: DateTime<Utc> } // + serde
#[async_trait] pub trait JwksCache: Send + Sync {
    async fn get(&self, issuer: &Issuer) -> Result<Option<CachedJwks>, AuthnError>;
    async fn put(&self, issuer: &Issuer, jwks: CachedJwks) -> Result<(), AuthnError>;
}
pub struct InMemoryJwksCache(/* tokio::sync::RwLock<HashMap<Issuer, CachedJwks>> */);
#[async_trait] pub trait JwksFetcher: Send + Sync { // seam for unit tests
    async fn fetch(&self, issuer: &Issuer) -> Result<CachedJwks, AuthnError>; // discovery + JWKS
}
pub struct HttpJwksFetcher { /* reqwest::Client (rustls, timeout), clock */ }
pub struct JwksProvider<F: JwksFetcher, K: JwksCache, C: Clock> { /* + per-issuer tokio::sync::Mutex map, cooldown state, ttl, cooldown */ }
impl<...> JwksProvider<F, K, C> {
    pub async fn key_for(&self, issuer: &Issuer, kid: &str) -> Result<jsonwebtoken::jwk::Jwk, AuthnError>;
}
```

  `key_for` algorithm (spec §4.3): cached + fresh (age < ttl) + kid present → return. kid missing or entry stale → if cooldown for this issuer has not elapsed and entry existed → `InvalidToken(UnknownKid)`; else single-flight refetch (per-issuer async Mutex; double-check cache after acquiring), `put`, then kid lookup → hit or `InvalidToken(UnknownKid)`. Fetch failure with NO usable cached entry → `Unavailable`. `HttpJwksFetcher::fetch`: GET `{issuer}/.well-known/openid-configuration` → verify body `issuer` field exact-equals → require `https` `jwks_uri` → GET it; cap BOTH bodies at 1 MiB by streaming `chunk()`s into a limited buffer (never unbounded `.text()`); any HTTP/parse failure → `Unavailable` (log kind, no bodies).
  Workspace `Cargo.toml`: add `jsonwebtoken = "10"` (fall back to `"9"` if 10 is not on crates.io; both expose `jwk::JwkSet` + `DecodingKey::from_jwk`) and `base64 = "0.22"` (needed in Task 7); service consumes `reqwest` (workspace, rustls) for the first time.

- [ ] **Step 1: Failing unit tests** in `jwks.rs` (fake fetcher counting calls + `FixedClock`-style adjustable clock; build a tiny `JwkSet` from JSON literals with a known `kid`):
  - `fresh_cache_hit_does_not_fetch`
  - `ttl_expiry_triggers_refetch`
  - `kid_miss_triggers_one_refetch_then_unknown_kid`
  - `cooldown_suppresses_repeated_kid_miss_refetch` (2nd miss inside cooldown → no fetch call, `UnknownKid`)
  - `fetch_failure_without_cache_is_unavailable`
  - `single_flight_coalesces_concurrent_refetches` (spawn N `key_for` on cold cache with a slow fake fetcher; assert exactly 1 fetch)
- [ ] **Step 2:** run → FAIL. **Step 3:** implement (deps first; `moon run repo:deny` — add `rs/deny.toml` entries only if it reds). **Step 4:** `cargo nextest run -p paigasus-iam jwks` → PASS.
- [ ] **Step 5: Commit** `feat(rs): add jwks provider with ttl + kid-miss rotation and caches (SMA-443)`

---

### Task 7: OIDC validator (`Authenticator` v1 impl)

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/mod.rs`
- Modify: service `Cargo.toml` dev-deps: `p256 = { version = "0.13", features = ["pkcs8", "pem"] }`

**Interfaces:**
- Consumes: `JwksProvider::key_for` (Task 6), `IssuerConfig` (Task 4), core `Authenticator` port.
- Produces: `OidcAuthenticator<F, K, C>` (`new(issuers: Vec<IssuerConfig>, provider: JwksProvider<F, K, C>, leeway_secs: u64, max_token_bytes: usize)`) implementing `Authenticator` per spec §4.1: length cap → `decode_header` (alg ∈ {RS256, ES256} else `UnsupportedAlg`; malformed → `Malformed`) → unverified `iss` read (split token on `.`, base64url-decode payload with the `base64` crate, `serde_json` read of `iss` only) → exact match against configured issuers else `IssuerNotConfigured` → `key_for(issuer, kid)` (header without `kid` → `UnknownKid`) → assert JWK kty matches alg family → `jsonwebtoken::decode::<WireClaims>` with `Validation` pinned to exactly the header alg, `validation.set_audience(&issuer.audiences)`, `validation.set_issuer(&[issuer])`, `validation.leeway = leeway_secs` → map to `ValidatedClaims` (`aud` may be string or array — serde untagged helper). jsonwebtoken error kinds map: `ExpiredSignature → Expired`, `InvalidSignature → BadSignature`, `InvalidAudience → AudienceMismatch`, else `Malformed`.
- Also produces (in `validator.rs` under `#[cfg(test)]`, reused later by tests/support via copy — keep it simple, duplicate is fine): `fn es256_keypair() -> (jsonwebtoken::EncodingKey, jsonwebtoken::jwk::Jwk, String /*kid*/)` using `p256::SecretKey::random` → `to_pkcs8_pem` → `EncodingKey::from_ec_pem`, JWK built from the public key's uncompressed affine `x`/`y` (base64url, no padding), fixed `kid` string.

  **Note (spec §8 refinement):** the mock-IdP strategy uses **runtime-generated EC P-256 keys** (dev-only `p256`) instead of committed PEM/JWK fixtures — same intent (no `rsa` crate, no committed private keys, ES256 covered), and RS256's accept path is covered end-to-end by the Keycloak test (Keycloak signs RS256 by default).

- [ ] **Step 1: Failing unit tests** (sign tokens locally with `es256_keypair` + a stub `JwksFetcher` serving the matching `JwkSet`):
  - `valid_es256_token_yields_claims` (iss/sub/aud/email/name/locale/zoneinfo round-trip)
  - `alg_none_and_hs256_rejected_before_key_lookup` (craft header manually; assert fetcher never called)
  - `unconfigured_issuer_rejected`, `audience_mismatch_rejected`, `expired_token_rejected_and_leeway_honored` (exp = now-30 with leeway 60 → OK; exp = now-120 → Expired)
  - `oversized_token_rejected` (token > max_token_bytes → `Oversized`)
  - `missing_kid_is_unknown_kid`
- [ ] **Step 2:** run → FAIL. **Step 3:** implement. **Step 4:** `cargo nextest run -p paigasus-iam validator` → PASS; clippy/fmt.
- [ ] **Step 5: Commit** `feat(rs): add provider-agnostic oidc token validator (SMA-443)`

---

### Task 8: Redis JWKS cache

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/oidc/redis_cache.rs`
- Modify: `oidc/mod.rs`, `rs/Cargo.toml` (workspace dep `redis`, features `tokio-comp`, `connection-manager` — use the current stable version line), service `Cargo.toml` (+ dev-dep `testcontainers-modules` gains the `redis` feature)
- Test: `rs/crates/services/paigasus-iam/tests/redis_jwks_cache.rs`

**Interfaces:**
- Consumes: `JwksCache` trait + `CachedJwks` (Task 6).
- Produces: `RedisJwksCache::connect(redis_url: &str, ttl_secs: u64) -> Result<Self, AuthnError>` (ConnectionManager) implementing `JwksCache`; key `iam:jwks:<issuer>`, value `serde_json`-serialized `CachedJwks`, `SET ... EX ttl_secs`; any Redis error → `Unavailable` (fail closed, spec §4.3/D15).

- [ ] **Step 1: Failing integration test** (`tests/redis_jwks_cache.rs`, testcontainers `redis` module, same CI-hard-fail/local-skip pattern as `start_migrated_postgres`): put → get round-trips; get of unknown issuer → `None`; after stopping the container, `get` → `Err(Unavailable)`.
- [ ] **Step 2:** run → FAIL. **Step 3:** implement + `moon run repo:deny`. **Step 4:** `cargo nextest run -p paigasus-iam redis_jwks` → PASS (Docker).
- [ ] **Step 5: Commit** `feat(rs): add redis jwks cache adapter (SMA-443)`

---

### Task 9: Proto `AuthnService` + regeneration

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto`
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/**` via `moon run contracts:generate` (commit the diff)

**Interfaces:**
- Produces the wire types per spec §7.1 (message/service names exactly): `AuthnService { rpc Introspect(IntrospectRequest) returns (IntrospectResponse) }`, `IntrospectRequest { string token = 1 }`, `IntrospectResponse { string principal_prn = 1; string status = 2; string issuer = 3; string subject = 4; google.protobuf.Timestamp expires_at = 5; repeated Membership memberships = 6; repeated string role_group_prns = 7; }`. Update the file header: drop the stale "SCAFFOLD ONLY (SMA-441)" text, note `TenancyService` (M1), `AuthnService` (M2), and keep `AuthorizationService { IsAuthorized }` reserved for M4/M5 (its `Introspect` folds into M2's — spec D4).

- [ ] **Step 1:** edit proto; run `moon run contracts:lint` (or the repo's buf lint task) → clean; `moon run contracts:generate`.
- [ ] **Step 2:** `cargo build -p paigasus-proto` → PASS; confirm `authn_service_server` module exists in the generated code.
- [ ] **Step 3:** run `moon ci :breaking --base origin/main` → additive, green.
- [ ] **Step 4: Commit** `feat(contracts): add iam authn service introspect rpc (SMA-443)` (include generated Rust/Py/TS diffs).

---

### Task 10: AppState wiring + HTTP Introspect + authn error funnel

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs` (handler + `AuthnApiError`)
- Modify: `http/mod.rs` (AppState, router), `http/dto.rs`, `main.rs`
- Modify: `tests/support/mod.rs` (mock IdP + new `app` signature) — **all existing test files keep compiling via the helper**
- Test: `rs/crates/services/paigasus-iam/tests/http_authn.rs`

**Interfaces:**
- Consumes: Tasks 5–7 types; Task 9 proto (gRPC side is Task 12).
- Produces:
  - Type aliases in `http/mod.rs`:

```rust
pub type Oidc = OidcAuthenticator<HttpJwksFetcher, InMemoryJwksCache, SystemClock>; // memory backend
pub type OidcRedis = OidcAuthenticator<HttpJwksFetcher, RedisJwksCache, SystemClock>;
// AppState holds an enum or generic? Keep it simple: a small enum wrapper implementing Authenticator
pub enum WiredAuthenticator { Memory(Oidc), Redis(OidcRedis) } // #[async_trait] impl Authenticator by delegation
pub type AuthnSvc = AuthenticateToken<WiredAuthenticator, PgExternalIdentityRepository, PgPrincipalRepository, PgMembershipRepository, KernelIdGenerator, SystemClock>;
```

  - `AppState::new(db: DatabaseConnection, cfg: &IamConfig) -> Result<AppState, AuthnError>` (Redis connect can fail) — gains `pub authn: AuthnSvc`. `main.rs` updated (`AppState::new(db, &cfg)?`).
  - `AuthnApiError(pub AuthnError)` `IntoResponse` per spec §6.3 exactly (401 + `WWW-Authenticate: Bearer error="invalid_token"`, 403 codes `identity_not_provisioned`/`provisioning_failed`/`principal_inactive`, 503, 500 opaque; body `{"error":{code,message}}`, messages static per code).
  - `POST /v1/authn/introspect` handler → `state.authn.introspect(&body.token)` → `IntrospectResponse`-shaped DTO (snake_case JSON per spec §7.2); route registered in `router()` OUTSIDE the auth-protected group (Task 11).
  - `tests/support`: `pub struct MockIdp { pub issuer: String, sign: EncodingKey, kid: String, handle: JoinHandle }` — `start_mock_idp() -> MockIdp` binds an ephemeral-port axum server serving the discovery doc + JWKS (ES256 runtime keys, Task 7 note); `MockIdp::bearer(&self, sub: &str, email: Option<&str>, aud: &str, exp_offset_secs: i64) -> String`; `pub fn test_config(idp: &MockIdp) -> IamConfig` (test defaults + the mock issuer, audience `"paigasus"`); `pub async fn app(db) -> (Router, MockIdp)` now builds `router(AppState::new(db, &test_config(&idp)).unwrap())`. Update `send` to accept `token: Option<&str>` and set the `authorization: Bearer …` header. **Mechanically update every existing caller** in `http_tenancy.rs`, `http_memberships.rs`, `health.rs`, `roundtrip.rs`, `tenancy_*.rs`, `grpc_*.rs` for the new signatures (pass a token everywhere; enforcement itself arrives Task 11 so they still pass without it — the point is the harness is ready).

- [ ] **Step 1: Failing tests** (`tests/http_authn.rs`): introspect happy path (JIT-provision a user first via an authenticated `POST /v1/organizations`? No — enforcement is Task 11; provision via `state.authn.resolve(token, Enabled)` directly or via the use case; simplest: call introspect on an unknown identity → 403 `identity_not_provisioned` (proves D10), then provision through `AuthnSvc::resolve(…, Enabled)` and assert introspect 200 with correct `principal_prn`, empty `role_group_prns`, memberships list); invalid token → 401 + `WWW-Authenticate` header; oversized token → 401.
- [ ] **Step 2:** run → FAIL. **Step 3:** implement; whole-workspace build must pass including all test files. **Step 4:** `cargo nextest run -p paigasus-iam` → PASS.
- [ ] **Step 5: Commit** `feat(rs): wire authn into app state and add http introspect (SMA-443)`

---

### Task 11: HTTP auth middleware + enforcement (test sweep)

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/http/auth_middleware.rs`
- Modify: `http/mod.rs` (`router()` splits protected/unprotected), existing HTTP test files
- Test: extend `tests/http_authn.rs`

**Interfaces:**
- Consumes: `AuthnSvc::resolve(token, Provisioning::Enabled)`, `AuthnApiError` (Task 10).
- Produces: `AuthContext { pub principal_id: PrincipalId, pub issuer: Issuer, pub subject: String }` (Clone) inserted as a request extension; `pub async fn require_bearer(State<AppState>, Request, Next) -> Response` extracting `Authorization: Bearer` (missing/malformed header → 401 `invalid_token`). Applied in `router()` (D14 — NOT `serve_http`) via `.route_layer(axum::middleware::from_fn_with_state(state.clone(), require_bearer))` on the sub-router containing organizations/teams/projects/memberships/users only; `/readyz`, `/healthz`, `/v1/authn/introspect` stay outside it.

- [ ] **Step 1: Failing tests:** `protected_route_without_token_is_401`; `protected_route_with_invalid_token_is_401_with_www_authenticate`; `protected_route_with_valid_token_succeeds_and_jit_provisions` (fresh sub → `POST /v1/organizations` 2xx; then introspect same token → 200 with the JIT'd principal, proving AC 2); `readyz_and_introspect_do_not_require_bearer`; `jit_disabled_unknown_identity_is_403` (second mock issuer with `jit_provisioning=false` in `test_config`).
- [ ] **Step 2:** run → new tests FAIL, and (after wiring the layer) every pre-existing HTTP test that sends no token now fails with 401 — that is the expected sweep signal.
- [ ] **Step 3:** implement middleware; sweep all existing HTTP tests to pass a valid `idp.bearer(...)` token via the updated `send`.
- [ ] **Step 4:** `cargo nextest run -p paigasus-iam` → PASS (full suite).
- [ ] **Step 5: Commit** `feat(rs): enforce bearer auth on the http api surface (SMA-443)`

---

### Task 12: gRPC `AuthnService` + gRPC auth middleware (test sweep)

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs` (service impl + tower auth layer)
- Modify: `grpc/mod.rs` (mount service + layer), `grpc/convert.rs` (`PrincipalContext` → `IntrospectResponse`, `authn_status` mapping per spec §6.3: `UNAUTHENTICATED`/`PERMISSION_DENIED`/`UNAVAILABLE`/`INTERNAL`), existing gRPC test files
- Test: `rs/crates/services/paigasus-iam/tests/grpc_authn.rs`

**Interfaces:**
- Consumes: `AuthnSvc`, Task 9 generated `authn_service_server::{AuthnService, AuthnServiceServer}`.
- Produces: `AuthnGrpc::new(state)` implementing the generated trait (`Introspect` → use case → convert; errors via `authn_status`); `AuthLayer` — a tower `Layer`/`Service` pair generic over the request body, applied via `Server::builder().layer(...)` in `grpc::router()`. Exemptions by `:path` prefix: `/grpc.health.v1.Health/`, `/paigasus.iam.v1.AuthnService/Introspect`. Rejections render a **trailers-only gRPC response** (HTTP 200, `content-type: application/grpc`, `grpc-status` = 16 UNAUTHENTICATED / 7 PERMISSION_DENIED / 14 UNAVAILABLE + ASCII-safe `grpc-message`), never a bare HTTP 401 — use `tonic::Status::…::into_http()` if available in tonic 0.14, otherwise construct the `http::Response` manually with those headers. On success insert the same `AuthContext` extension.

- [ ] **Step 1: Failing tests** (`tests/grpc_authn.rs`, ephemeral-port pattern from `grpc_tenancy.rs`): introspect over gRPC round-trips a JIT'd principal; tenancy RPC without token → `Code::Unauthenticated`; with valid bearer metadata (`authorization` MetadataValue) → OK; health service works without a token.
- [ ] **Step 2:** run → FAIL; pre-existing gRPC tenancy tests fail once the layer lands (expected sweep).
- [ ] **Step 3:** implement; sweep `grpc_tenancy.rs`/`grpc_health.rs` to attach bearer metadata (helper in support: `fn grpc_bearer(req: &mut tonic::Request<T>, token: &str)` or build requests via a small fn).
- [ ] **Step 4:** `cargo nextest run -p paigasus-iam` → PASS (full suite). **Step 5: Commit** `feat(rs): add grpc authn service and bearer enforcement (SMA-443)`

---

### Task 13: Keycloak end-to-end test (AC 1)

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs`
- Create: `rs/crates/services/paigasus-iam/tests/fixtures/keycloak-realm.json`

**Interfaces:**
- Consumes: everything; no new production code. Config-only proof (AC 1).

- [ ] **Step 1: Realm fixture.** Realm `paigasus-test` with: a confidential-less (public) client `paigasus-cli` with `directAccessGrantsEnabled: true`; test user `alice` / password `alice-password` with email `alice@example.com` + firstName/lastName; **client scope with an `oidc-audience-mapper` adding `paigasus` to the access token `aud`** and **`oidc-usermodel-property-mapper` (or the builtin email mapper) with `access.token.claim: true`** so `email` lands in the ACCESS token (spec D11 — vanilla Keycloak omits both).
- [ ] **Step 2: Failing test.** `GenericImage::new("quay.io/keycloak/keycloak", "<current stable tag — check quay at implementation>")` with env `KEYCLOAK_ADMIN`/`KEYCLOAK_ADMIN_PASSWORD`, cmd `start-dev --import-realm`, the fixture copied to `/opt/keycloak/data/import/` (`with_copy_to`), wait strategy on the mgmt/health readiness (or stdout "started" message; budget ~120 s). Same CI-hard-fail/local-skip Docker gating as `start_migrated_postgres`. Then: build `IamConfig` whose single issuer is `http://…` — **note:** Keycloak in the container is plain HTTP; `Issuer::parse` requires https (spec §3.1). Resolution (part of this task): allow `http://` issuers **only** under a `#[cfg(test)]`-unavailable escape? NO — keep the domain rule; instead run Keycloak with `--https-certificate…`? Simplest compliant path: add a config-level constant-free test override is NOT allowed by the spec, so use `start-dev` behind HTTPS via Keycloak's self-signed dev cert (`--https-port`, `KC_HTTPS_CERTIFICATE_*` with a testcontainers-generated self-signed cert) AND build the `HttpJwksFetcher`'s reqwest client with `danger_accept_invalid_certs(true)` **only when** `IamConfig` gets a new `authn.accept_invalid_tls: bool` (default `false`, documented test-only, validated to be false unless compiled with `cfg(test)`… keep it a plain config flag with a loud doc comment — it is still "config only"). Obtain a token: `POST {issuer}/protocol/openid-connect/token` (password grant, reqwest, `danger_accept_invalid_certs` in the test client). Assert: (1) `POST /v1/organizations` with the Keycloak bearer → 2xx (JIT fired); (2) introspect → 200, principal PRN stable across calls, email-derived user exists via `GET`ing `find_user` through the DB or the introspect response fields; (3) token is RS256 (assert header alg) — closing the RS256 coverage from Task 7's note.
- [ ] **Step 3:** implement until green: `cargo nextest run -p paigasus-iam keycloak_e2e` (Docker required).
- [ ] **Step 4: Commit** `test(rs): add keycloak end-to-end oidc acceptance test (SMA-443)`

---

### Task 14: Full CI gate + docs polish

**Files:**
- Modify (as needed): `rs/deny.toml`, `iam.toml.example`, `rs/crates/services/paigasus-iam/README.md` (if present — else the crate-level doc comment) for the D15 Redis trust note.

- [ ] **Step 1:** `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations` — fix anything red (machete: `reqwest` is now consumed; deny: new `jsonwebtoken`/`redis`/`base64`/`p256` licenses; affected-smoke should be untouched — no new crates, no kernel edits).
- [ ] **Step 2:** `moon run ts:fmt` (proto TS codegen from Task 9 may touch `ts/` — Prettier is a separate whole-tree gate).
- [ ] **Step 3:** Re-read the spec end-to-end vs. the diff (`git diff origin/main --stat`); confirm every §2 decision D1–D15 is implemented; no stray debug code (`grep -rn "dbg!\|println!" rs/crates` on changed files).
- [ ] **Step 4: Commit** any residue: `chore(rs): satisfy repo gates for m2 authn (SMA-443)`

---

## Self-review notes (already applied)

- Spec coverage: §3→T1/T2, §4→T6/T7 (+T8 Redis), §5→T3, §6→T4/T5 (+funnel T10), §7→T9/T10/T11/T12, §8→tests throughout +T13, §9→T14. `role_groups` empty everywhere (M3).
- Deviation from spec §8 flagged in T7: runtime EC keys instead of committed fixtures (rationale inline); RS256 accept-path covered by T13 step 2 assertion. T13 also surfaces the container-HTTPS wrinkle the spec missed and resolves it inside the "config only" constraint (`authn.accept_invalid_tls`, default false, loudly documented).
- Type consistency: `AuthnSvc` = `AuthenticateToken<WiredAuthenticator, PgExternalIdentityRepository, PgPrincipalRepository, PgMembershipRepository, KernelIdGenerator, SystemClock>` used in T10/T11/T12; `resolve(token, Provisioning)` / `introspect(token)` names fixed across T5/T10/T11/T12; `MembershipRecord` (not `Membership`) in `PrincipalContext`.
