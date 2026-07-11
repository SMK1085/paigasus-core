# M4 API keys & service accounts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add machine identity to the IAM service — `ServiceAccount` principals and issuable/revocable, HMAC-peppered `ApiKey`s with constant-time validation, cached hot-path introspection, and API-key bearer auth — so a service account can act, authorized by M3 policy.

**Architecture:** ServiceAccount reuses M3's unified `Pgs::Iam::Principal` Cedar type (a `principal` PRN with `kind="service_account"`); the only authz-path change is un-hardcoding `kind="user"` in the entity-slice loader. API keys are `pgs_sk_<keyid_hex>_<secret_b64url>` tokens, stored only as `HMAC-SHA-256(pepper, secret)`, validated in constant time via `hmac::verify_slice`, with a fail-open positive-validation cache evicted on revoke/archive. The whole feature follows the M1 `Principal`↔`User` two-table pattern and the M3 ports/adapters + memory/redis-behind-a-port patterns.

**Tech Stack:** Rust (edition 2024, rustc 1.95), SeaORM + Postgres (testcontainers), axum (HTTP) + tonic (gRPC), Cedar (via M3), Redis (ConnectionManager), RustCrypto `hmac`+`sha2`, `proptest`, buf/prost/tonic codegen, Moon.

**Spec:** `docs/superpowers/specs/2026-07-10-sma-445-m4-api-keys-service-accounts-design.md` (D1–D16). Read it before starting; each task cites the relevant decision.

## Global Constraints

- **Rust edition 2024, rust-version 1.95** (workspace-inherited); even where an AC says otherwise.
- **SPDX header line 1 of every source file:** `// SPDX-License-Identifier: Apache-2.0` (`#` for none here).
- **Module docs** (`//!`) cite the ticket: `(SMA-445, M4)` and the relevant ADR/decision.
- **Purity:** `paigasus-iam-core` domain files carry **no serde, no sqlx** derives. Entropy/HMAC/pepper live in the service adapter only — the kernel and core stay **`getrandom`-free** (`repo:wasm-getrandom-free` gate).
- **Enums stored as Postgres `TEXT` + named `CHECK`**, never native PG enums. Constraint names are load-bearing (`conflict_kind` substring-matches them): `uq_*`/`ck_*`/`fk_*`/`ix_*`.
- **UUIDs are app-minted UUIDv7** via the `IdGenerator` port (`KernelIdGenerator`), never DB-generated.
- **Conventional commits, workspace scope:** `feat(rs): …`, `test(rs): …`, `feat(contracts): …`. Subject **starts lowercase**, ≤100 chars; **no bare `#NNN`** in body; one contiguous footer.
- **Commits are SSH-signed via 1Password.** The fresh worktree has no `commitlint`, so commit with `--no-verify` and a commitlint-clean message (the CI parity gate is the backstop). End every commit body with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Bash PATH:** prefix toolchain commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Cargo/nextest run from `rs/`; buf/moon from repo root.
- `cargo nextest` on a crate with no tests exits non-zero → use `--no-tests=pass`.
- Never name a file with a Windows-reserved base name (`con`, `prn`, `aux`, `nul`, `com1`…). (`api_key`/`service_account` are fine.)
- **Before pushing** (Task 23), run the full graph like CI: `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations`.

---

## File Structure

**`rs/crates/libs/paigasus-iam-core/src/` (pure domain):**
- `principal.rs` (modify) — add `PrincipalKind::ServiceAccount`, `PrincipalStatus::Disabled`.
- `service_account.rs` (create) — `ServiceAccount` entity.
- `api_key.rs` (create) — `ApiKeyId`, `ApiKeyStatus`, `ApiKey`, `ApiKeyDefect`, `NewApiKey`, `format_token`/`parse_token`, `verify` (pure, hasher injected).
- `authn.rs` (modify) — `Credential` enum; retype `AuthnPrincipal`.
- `ports.rs` (modify) — `IdGenerator` additions, `SecretHasher`, `KeyEntropy`, `ServiceAccountRepository`, `ApiKeyRepository`, `ConflictKind` variants.
- `value.rs` (modify) — `DomainError::InvalidApiKeyToken`.
- `lib.rs` (modify) — `pub mod` + re-exports.
- `tests/` (create `api_key_props.rs`) — property tests.

**`rs/crates/services/paigasus-iam/src/adapters/persistence/`:**
- `migration/m0005_create_service_accounts_and_api_keys.rs` (create) + `migration/mod.rs` (modify).
- `entities/service_account.rs`, `entities/api_key.rs` (create) + `entities/mod.rs` (modify).
- `pg_service_accounts.rs`, `pg_api_keys.rs` (create) + `mod.rs` (modify: re-export + `conflict_kind`).

**`rs/crates/services/paigasus-iam/src/adapters/`:**
- `api_keys/mod.rs`, `api_keys/hasher.rs` (`HmacSecretHasher`), `api_keys/entropy.rs` (`OsRngKeyEntropy`), `api_keys/cache.rs` (`Memory`/`RedisApiKeyCache`) (create).
- `http/service_accounts.rs`, `http/api_keys.rs` (create) + `http/{mod,dto}.rs` (modify), `http/auth_middleware.rs` (modify).
- `grpc/service_accounts.rs` (create) + `grpc/{mod,convert,authn}.rs` (modify).
- `authn.rs` HTTP error mapping (modify), `auth.rs` (`AuthContext` retype, modify).

**`rs/crates/services/paigasus-iam/src/application/`:**
- `service_accounts.rs`, `api_keys.rs`, `authenticate_api_key.rs` (create) + `mod.rs` (modify).

**`rs/crates/services/paigasus-iam/src/`:** `config.rs` (modify — `[api_keys]` + redacting pepper), `lib.rs`, `main.rs` (unchanged except migration pickup which is automatic).

**`contracts/proto/paigasus/iam/v1/iam.proto`** (modify) → regenerated bindings.

**`rs/crates/libs/paigasus-iam-core/src/authz/`:** `action.rs`, `schema.rs`, `roles.rs` (modify — 7 actions + 4 templates); service `adapters/persistence/pg_entity_slice.rs` (modify — `kind` fix).

**Tests:** `rs/crates/services/paigasus-iam/tests/{service_accounts,api_keys_http,api_keys_grpc,api_key_auth}.rs` (create).

---

## Phase A — Domain (`paigasus-iam-core`, pure)

### Task 1: `PrincipalKind::ServiceAccount` + `PrincipalStatus::Disabled`

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/principal.rs`
- Test: same file's `#[cfg(test)] mod tests`

**Interfaces (Produces):**
- `PrincipalKind::ServiceAccount` (`as_str()` → `"service_account"`, `parse("service_account")`).
- `PrincipalStatus::Disabled` (`as_str()` → `"disabled"`, `parse("disabled")`).

- [ ] **Step 1: Write failing tests** — extend the existing round-trip tests in `principal.rs`:
```rust
#[test]
fn service_account_kind_roundtrips() {
    assert_eq!(PrincipalKind::parse("service_account"), Some(PrincipalKind::ServiceAccount));
    assert_eq!(PrincipalKind::ServiceAccount.as_str(), "service_account");
}
#[test]
fn disabled_status_roundtrips() {
    assert_eq!(PrincipalStatus::parse("disabled"), Some(PrincipalStatus::Disabled));
    assert_eq!(PrincipalStatus::Disabled.as_str(), "disabled");
}
```
- [ ] **Step 2: Run — expect FAIL** (variants don't exist):
`cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam-core service_account_kind_roundtrips disabled_status_roundtrips`
Expected: FAIL (no variant `ServiceAccount`).
- [ ] **Step 3: Implement** — add `ServiceAccount` to `PrincipalKind` (enum + `as_str` arm `=> "service_account"` + `parse` arm), and `Disabled` to `PrincipalStatus` (enum + `as_str` `=> "disabled"` + `parse` arm). Remove the "M0 mints only `User`" comment; note `(SMA-445, M4)`.
- [ ] **Step 4: Run — expect PASS.** Same command.
- [ ] **Step 5: Commit**
```bash
git add rs/crates/libs/paigasus-iam-core/src/principal.rs
git commit --no-verify -m "feat(rs): add ServiceAccount kind and Disabled principal status (SMA-445)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `ServiceAccount` domain entity

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/src/service_account.rs`
- Modify: `lib.rs` (`pub mod service_account;` + re-export `ServiceAccount`)

**Interfaces (Produces):**
- `struct ServiceAccount { principal_id: PrincipalId, owner: TenancyNodeRef, name: String, created_at: DateTime<Utc>, updated_at: DateTime<Utc> }` — **no `status` field** (D16: status is on the `Principal`).
- `ServiceAccount::new(principal_id: PrincipalId, owner: TenancyNodeRef, name: &str, now: DateTime<Utc>) -> Result<Self, DomainError>` (validates `name` via `validate_name`).

**Consumes:** `PrincipalId`, `TenancyNodeRef`, `validate_name` (all in the core crate — see `tenancy.rs:178` for `validate_name`, `tenancy.rs` for `TenancyNodeRef`).

- [ ] **Step 1: Write failing test** (in `service_account.rs`):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    #[test]
    fn new_rejects_blank_name() {
        let id = PrincipalId::from_prn(/* build a principal prn, mirror user.rs tests */);
        assert!(ServiceAccount::new(id, owner_org_ref(), "  ", Utc::now()).is_err());
    }
    #[test]
    fn new_sets_timestamps() {
        let now = Utc::now();
        let sa = ServiceAccount::new(pid(), owner_org_ref(), "ci-bot", now).unwrap();
        assert_eq!(sa.created_at, now);
        assert_eq!(sa.updated_at, now);
        assert_eq!(sa.name, "ci-bot");
    }
    // pid()/owner_org_ref() helpers: mirror the prn construction in user.rs / tenancy.rs tests.
}
```
- [ ] **Step 2: Run — expect FAIL** (module missing): `cargo nextest run -p paigasus-iam-core service_account`.
- [ ] **Step 3: Implement** `service_account.rs` — SPDX + `//! Service-account domain entity (SMA-445, M4).`; the struct + `new` mirroring `Organization::new` (`tenancy.rs:186-206`): derive `#[derive(Debug, Clone, PartialEq, Eq)]`, validate name, set both timestamps to `now`. Add `pub mod service_account;` + `pub use service_account::ServiceAccount;` in `lib.rs`.
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): add ServiceAccount domain entity (SMA-445)`.

---

### Task 3: `api_key.rs` — token, hashing seam, validation (pure)

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/src/api_key.rs`
- Modify: `lib.rs`, `value.rs` (add `DomainError::InvalidApiKeyToken`)

**Interfaces (Produces):**
- `struct ApiKeyId(Uuid)` with `from_uuid`, `uuid()`, `Display`/`FromStr` as 32-char simple hex.
- `enum ApiKeyStatus { Active, Revoked }` (`as_str`/`parse` + test).
- `struct ApiKey { id, service_account_id: PrincipalId, scope: TenancyNodeRef, prefix: String, status: ApiKeyStatus, expires_at: Option<DateTime<Utc>>, last_used_at: Option<DateTime<Utc>>, created_at, revoked_at: Option<DateTime<Utc>>, scope_actions: Vec<Action>, scope_roles: Vec<String> }`.
- `enum ApiKeyDefect { Malformed, BadSecret, Revoked, Expired }` — `Debug` shows detail, `Display` is generic ("invalid api key"). No secret material in either.
- `struct NewApiKey { key: ApiKey, plaintext: String }` — plaintext returned once.
- `struct ParsedToken { key_id: ApiKeyId, secret: Vec<u8> }`.
- Pure fns:
  - `fn format_token(prefix: &str, key_id: ApiKeyId, secret: &[u8]) -> String` → `"{prefix}{keyid_hex}_{secret_b64url}"`.
  - `fn parse_token(prefix: &str, token: &str, max_bytes: usize) -> Result<ParsedToken, ApiKeyDefect>` — length-cap; strip prefix; **fixed 32 hex chars** for keyid, one `_`, remainder = secret; strict hex + strict base64url-nopad decode (reject non-canonical). Never panics.
  - `fn display_prefix(prefix: &str, key_id: ApiKeyId) -> String` → `"{prefix}{first 8 of keyid_hex}"` (for storage/listing).

**Consumes:** `PrincipalId`, `TenancyNodeRef`, `authz::Action`. Uses `uuid`, `chrono`, `base64` (already core deps? `base64` is a service dep — for the pure core, the encode/decode of the token can use a tiny hex + a `base64` core dep; add `base64` to `paigasus-iam-core/Cargo.toml` if absent, license already cleared). **No `getrandom`/`rand`/`hmac` in this file** — secret generation and HMAC are ports (Task 5).

- [ ] **Step 1: Write failing tests** (in `api_key.rs`):
```rust
#[test]
fn token_roundtrips_via_fixed_width_parse() {
    let id = ApiKeyId::from_uuid(Uuid::from_u128(0x0192_..._u128)); // any v7-ish uuid
    let secret = [7u8; 32];
    let tok = format_token("pgs_sk_", id, &secret);
    let parsed = parse_token("pgs_sk_", &tok, 512).unwrap();
    assert_eq!(parsed.key_id, id);
    assert_eq!(parsed.secret, secret.to_vec());
}
#[test]
fn parse_rejects_wrong_prefix_and_overlong() {
    assert!(matches!(parse_token("pgs_sk_", "nope_abc", 512), Err(ApiKeyDefect::Malformed)));
    let huge = format!("pgs_sk_{}", "a".repeat(10_000));
    assert!(matches!(parse_token("pgs_sk_", &huge, 512), Err(ApiKeyDefect::Malformed)));
}
#[test]
fn parse_handles_underscore_in_secret() {
    // secret whose base64url contains '_' must still parse (fixed-width keyid, not split-on-'_')
    let id = ApiKeyId::from_uuid(Uuid::from_u128(1));
    let secret: [u8;32] = std::array::from_fn(|i| (i as u8).wrapping_mul(251)); // yields '_'/'-' chars
    let tok = format_token("pgs_sk_", id, &secret);
    assert_eq!(parse_token("pgs_sk_", &tok, 512).unwrap().secret, secret.to_vec());
}
#[test]
fn defect_display_scrubs_detail() {
    assert_eq!(ApiKeyDefect::BadSecret.to_string(), "invalid api key");
}
#[test]
fn status_roundtrips() {
    assert_eq!(ApiKeyStatus::parse("revoked"), Some(ApiKeyStatus::Revoked));
}
```
- [ ] **Step 2: Run — expect FAIL:** `cargo nextest run -p paigasus-iam-core api_key`.
- [ ] **Step 3: Implement** `api_key.rs` per the Interfaces. For `parse_token`: `token.strip_prefix(prefix).ok_or(Malformed)?`; enforce `token.len() <= max_bytes`; take `&rest[..32]` as hex (`u128::from_str_radix` per 16-byte or `Uuid::parse_str` of the 32-char simple form) — bounds-check first; require `rest.as_bytes().get(32) == Some(&b'_')`; decode `&rest[33..]` with `base64::engine::general_purpose::URL_SAFE_NO_PAD` via `.decode()` (rejects bad chars; for strict trailing-bit rejection use `URL_SAFE_NO_PAD` + assert re-encode equals input). Add `DomainError::InvalidApiKeyToken` in `value.rs`. Wire `pub mod api_key;` + re-exports in `lib.rs`.
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): add ApiKey domain entity and token codec (SMA-445)`.

---

### Task 4: `Credential` enum + `AuthnPrincipal` retype (workspace-atomic)

**Files:**
- Modify (core): `rs/crates/libs/paigasus-iam-core/src/authn.rs`, `lib.rs`
- Modify (service consumers — **same task, so the workspace stays green**): `src/application/authenticate_token.rs` (builds the `Oidc` variant), `src/adapters/auth.rs` (`AuthContext` — replace `issuer`/`subject` with `credential: Credential`, keep `principal_id`), `src/adapters/http/dto.rs` (`IntrospectResponseDto::from` matches `Credential::Oidc`), `src/adapters/grpc/convert.rs` (`to_introspect_response` matches `Credential::Oidc`), `src/adapters/http/auth_middleware.rs` + `src/adapters/grpc/authn.rs` (the `AuthContext {…}` insertion; the OIDC-only `ensure_platform_admin` call is unchanged here — still the only branch).

**Interfaces (Produces):**
- `enum Credential { Oidc { issuer: Issuer, subject: String, expires_at: DateTime<Utc> }, ApiKey { key_id: ApiKeyId, expires_at: Option<DateTime<Utc>> } }`.
- `AuthnPrincipal { principal_id: PrincipalId, kind: PrincipalKind, status: PrincipalStatus, credential: Credential }` (drops flat `issuer`/`subject`/`expires_at`) + `fn expires_at(&self) -> Option<DateTime<Utc>>` + convenience `fn issuer(&self) -> Option<&Issuer>` / `fn subject(&self) -> Option<&str>` (return `Some` only for `Oidc`).
- `AuthContext { principal_id: PrincipalId, credential: Credential }` + `actor_prn()` accessor (handlers already use `actor_prn`/`principal_id` — those stay working).

**Consumes:** `Issuer`, `PrincipalId`, `PrincipalKind`, `PrincipalStatus`, `ApiKeyId`.

**Note (blast radius, D8 — the challenge caught that this is *not* a two-file change):** this is a cross-cutting refactor done in **one commit** so the whole workspace compiles green. Token introspection only ever validates a JWT, so its two projections match `Credential::Oidc` and treat `ApiKey` as unreachable (`debug_assert!(false)`/error). No API-key branch exists yet — that arrives in Task 19; here every producer still builds `Oidc`.

- [ ] **Step 1: Write failing test** (in `authn.rs`):
```rust
#[test]
fn api_key_principal_has_no_issuer() {
    let p = AuthnPrincipal {
        principal_id: pid(), kind: PrincipalKind::ServiceAccount, status: PrincipalStatus::Active,
        credential: Credential::ApiKey { key_id: ApiKeyId::from_uuid(Uuid::from_u128(1)), expires_at: None },
    };
    assert!(p.issuer().is_none());
    assert_eq!(p.expires_at(), None);
}
```
- [ ] **Step 2: Run — expect FAIL** (`Credential`/field missing): `cargo nextest run -p paigasus-iam-core api_key_principal_has_no_issuer`.
- [ ] **Step 3: Implement** the enum + retype `AuthnPrincipal` + accessors (core), then update **all consumers listed above** to the new shape (build the `Oidc` variant everywhere; retype `AuthContext`). Update existing `#[cfg(test)]` constructions.
- [ ] **Step 4: Run — expect PASS, whole workspace green:** `cargo build -p paigasus-iam && cargo nextest run -p paigasus-iam-core api_key_principal_has_no_issuer && cargo nextest run -p paigasus-iam --no-tests=pass` (existing M2 introspect tests still pass).
- [ ] **Step 5: Commit** `feat(rs): model authenticated principal credential as an enum (SMA-445)`.

---

### Task 5: Ports — `SecretHasher`, `KeyEntropy`, `IdGenerator`, repositories

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/ports.rs`, `lib.rs`

**Interfaces (Produces):**
```rust
pub trait SecretHasher: Send + Sync {          // non-async; pepper injected into the adapter
    fn hash(&self, secret: &[u8]) -> Vec<u8>;             // HMAC-SHA-256(pepper, secret)
    fn verify(&self, secret: &[u8], expected: &[u8]) -> bool; // constant-time
}
pub trait KeyEntropy: Send + Sync { fn new_secret(&self) -> [u8; 32]; }

// IdGenerator (extend the existing trait):
fn new_service_account_id(&self) -> PrincipalId;   // principal PRN, kind-agnostic mint
fn new_api_key_id(&self) -> ApiKeyId;              // UUIDv7

#[async_trait] pub trait ServiceAccountRepository: Send + Sync {
    async fn create(&self, principal: &Principal, sa: &ServiceAccount) -> Result<(), RepositoryError>; // one txn
    async fn find(&self, id: &PrincipalId) -> Result<Option<ServiceAccount>, RepositoryError>;
    async fn list_by_owner(&self, owner: &TenancyNodeRef, limit: u64, offset: u64) -> Result<Vec<ServiceAccount>, RepositoryError>;
    async fn set_principal_status(&self, id: &PrincipalId, status: PrincipalStatus) -> Result<(), RepositoryError>;
}
#[async_trait] pub trait ApiKeyRepository: Send + Sync {
    async fn issue(&self, key: &ApiKey, key_hash: &[u8]) -> Result<(), RepositoryError>;
    async fn find_by_id(&self, id: ApiKeyId) -> Result<Option<(ApiKey, Vec<u8>)>, RepositoryError>; // key + stored hash
    async fn revoke(&self, id: ApiKeyId, now: DateTime<Utc>) -> Result<(), RepositoryError>;
    async fn list_by_service_account(&self, sa: &PrincipalId, limit: u64, offset: u64) -> Result<Vec<ApiKey>, RepositoryError>;
    async fn list_ids_by_service_account(&self, sa: &PrincipalId) -> Result<Vec<ApiKeyId>, RepositoryError>; // for archive-evict
    async fn touch_last_used(&self, id: ApiKeyId, now: DateTime<Utc>, throttle_secs: u64) -> Result<(), RepositoryError>;
}
```
- New `ConflictKind` variants: `ServiceAccountNameTaken`, `ApiKeyHashCollision`.

- [ ] **Step 1: Write failing test** — extend the object-safety assertion at `ports.rs:171`:
```rust
#[test]
fn new_repos_are_object_safe() {
    fn _assert(_: &dyn ServiceAccountRepository, _: &dyn ApiKeyRepository, _: &dyn SecretHasher, _: &dyn KeyEntropy) {}
}
```
- [ ] **Step 2: Run — expect FAIL** (traits missing): `cargo nextest run -p paigasus-iam-core new_repos_are_object_safe`.
- [ ] **Step 3: Implement** the traits (mirror `PrincipalRepository`/`ExternalIdentityRepository` at `ports.rs:67-80`; `#[async_trait]`, `: Send + Sync`). Add the two `IdGenerator` methods and the `ConflictKind` variants. Re-export the new ports in `lib.rs`.
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): add api-key/service-account ports and secret hasher (SMA-445)`.

---

### Task 6: Property-based validation tests (`proptest`) — AC-1

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/tests/api_key_props.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/Cargo.toml` (`[dev-dependencies] proptest = "1"`)

**Interfaces (Consumes):** `format_token`, `parse_token` (Task 3). For HMAC round-trips use a **test-local** `SecretHasher` impl over `hmac`+`sha2` (dev-deps) — this proves the port contract without pulling crypto into the domain.

- [ ] **Step 1: Write property tests:**
```rust
proptest! {
    // (a) issue -> parse round-trips for any secret bytes
    #[test]
    fn issue_parse_roundtrip(secret in proptest::array::uniform32(any::<u8>()), lo in any::<u128>()) {
        let id = ApiKeyId::from_uuid(Uuid::from_u128(lo));
        let tok = format_token("pgs_sk_", id, &secret);
        let p = parse_token("pgs_sk_", &tok, 4096).unwrap();
        prop_assert_eq!(p.key_id, id);
        prop_assert_eq!(p.secret, secret.to_vec());
    }
    // (b) any single-byte flip of the secret fails HMAC verify
    #[test]
    fn bitflip_secret_rejected(secret in proptest::array::uniform32(any::<u8>()), idx in 0usize..32, bit in 0u8..8) {
        let h = TestHasher::new(b"peppered-pepper-32-bytes-minimum!!");
        let good = h.hash(&secret);
        let mut bad = secret; bad[idx] ^= 1 << bit;
        prop_assert!(h.verify(&secret, &good));
        prop_assert!(!h.verify(&bad, &good));
    }
    // (c) wrong pepper never verifies
    #[test]
    fn wrong_pepper_rejected(secret in proptest::array::uniform32(any::<u8>())) {
        let a = TestHasher::new(b"pepper-aaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let b = TestHasher::new(b"pepper-bbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        prop_assert!(!b.verify(&secret, &a.hash(&secret)));
    }
    // (d) arbitrary bytes into parse_token never panic
    #[test]
    fn parse_never_panics(s in ".*") {
        let _ = parse_token("pgs_sk_", &s, 4096);
    }
}
```
- [ ] **Step 2: Run — expect FAIL** (proptest dep / helpers): `cargo nextest run -p paigasus-iam-core --test api_key_props`.
- [ ] **Step 3: Add `proptest`, `hmac`, `sha2` dev-deps; implement `TestHasher`** (HMAC-SHA-256 via `Hmac::<Sha256>::new_from_slice(pepper)`, `verify` via `.verify_slice()`).
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `test(rs): property-based api-key validation tests (SMA-445)`.

---

## Phase B — Proto

### Task 7: `ServiceAccountService` + `IntrospectApiKey` proto + codegen

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto`
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/…` (+ py/ts) via `moon run contracts:generate`

**Interfaces (Produces):** messages `ServiceAccount`, `ApiKey`, and RPC request/response pairs; `service ServiceAccountService { CreateServiceAccount, GetServiceAccount, ListServiceAccounts, ArchiveServiceAccount, IssueApiKey, RevokeApiKey, ListApiKeys }`; **`IntrospectApiKey` added to `AuthnService`** (next to `Introspect`). Follow conventions (mirror `AuthorizationService` block, `iam.proto:239-339`): PRNs as `string`, `google.protobuf.Timestamp`, embedded `AuditMetadata`, `NODE_STATUS`-style enums, `uint32 limit`/`uint64 offset`, `XxxRequest`/`XxxResponse` pairs. `IssueApiKeyResponse { ApiKey api_key = 1; string token = 2; }` (token shown once). `ListApiKeys` returns records with `prefix`/`status`/`last_used`/`expires_at`, **no secret**.

- [ ] **Step 1: Add the messages + services** to `iam.proto` (only add fields; reserve-and-add for any change to existing messages so `:breaking` stays green).
- [ ] **Step 2: Regenerate:** `export PATH=… && moon run contracts:generate`.
- [ ] **Step 3: Verify build:** `cd rs && cargo build -p paigasus-proto`. Expected: PASS; committed generated files updated.
- [ ] **Step 4: Breaking check:** `moon run contracts:breaking` (or `moon ci :breaking --base origin/main`). Expected: PASS (additive).
- [ ] **Step 5: Commit** `feat(contracts): add ServiceAccountService and IntrospectApiKey RPCs (SMA-445)` (stage proto + all regenerated bindings).

---

## Phase C — Persistence (`paigasus-iam` service)

### Task 8: Migration `m0005` + SeaORM entities

**Files:**
- Create: `src/adapters/persistence/migration/m0005_create_service_accounts_and_api_keys.rs`
- Modify: `migration/mod.rs` (add `mod m0005_…;` + `Box::new(m0005_…::Migration)` as the 5th entry)
- Create: `entities/service_account.rs`, `entities/api_key.rs`; Modify `entities/mod.rs`

**Interfaces (Produces):** tables per spec §5.1. `service_account`: shared PK `principal_id` FK→`principal(id)` cascade; `owner_org_id`/`owner_team_id`/`owner_project_id` + `ck_service_account_owner` (exactly-one); `name`; timestamps; **no status**; three partial unique indexes `uq_service_account_{org,team,project}_name`. `api_key`: `id` PK; `service_account_id` FK→`service_account(principal_id)` cascade; `scope_org_id`/`scope_team_id`/`scope_project_id` + `ck_api_key_scope`; `prefix`, `key_hash` + `uq_api_key_hash`, `status`, `expires_at?`, `last_used_at?`, `created_at`, `revoked_at?`, `scope_actions?`, `scope_roles?`; `ix_api_key_service_account`. All uniques/checks via raw `execute_unprepared` (names load-bearing).

**Consumes:** the migration & entity patterns at `m0002_create_tenancy.rs` / `m0004_create_authz.rs`, `entities/user.rs`, `entities/role_grant.rs`.

- [ ] **Step 1: Write failing test** — the migration is exercised by the shared harness (`tests/support/mod.rs::Migrator::up`). Add a schema-presence test `tests/service_accounts.rs`:
```rust
#[tokio::test]
async fn m0005_creates_tables() {
    let (db, _c) = support::pg().await; // spins testcontainer + runs Migrator::up
    // querying an empty api_key table succeeds => table + columns exist
    let n = api_key::Entity::find().count(&db).await.unwrap();
    assert_eq!(n, 0);
}
```
- [ ] **Step 2: Run — expect FAIL** (entity/table missing): `cd rs && cargo nextest run -p paigasus-iam --test service_accounts m0005_creates_tables` (needs Docker).
- [ ] **Step 3: Implement** the migration (mirror `m0002`/`m0004` idioms: `DeriveIden` enums, `ColumnDef`, FK by `Alias::new("principal")`, raw-SQL named constraints) and both entities (`DeriveEntityModel`; `service_account` shared-PK `belongs_to` principal like `user.rs`; `api_key` own PK, empty `Relation`). Register in `migration/mod.rs` + `entities/mod.rs`.
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): m0005 service_account and api_key tables (SMA-445)`.

---

### Task 9: `PgServiceAccountRepository`

**Files:**
- Create: `src/adapters/persistence/pg_service_accounts.rs`
- Modify: `persistence/mod.rs` (`pub use` + `conflict_kind` branch for `uq_service_account_*_name` → `ServiceAccountNameTaken`)

**Interfaces:** implements `ServiceAccountRepository`. **`create` inserts `principal` + `service_account` in one transaction, principal first** (mirror `pg_repository.rs::create_user` at `:40-69`). `set_principal_status` updates `principal.status` (D16). Map domain↔model via `*_to_model`/`model_to_*` free fns; parse failure → `Backend`.

- [ ] **Step 1: Write failing test** (`tests/service_accounts.rs`):
```rust
#[tokio::test]
async fn create_and_find_service_account() {
    let (db, _c) = support::pg().await;
    let repo = PgServiceAccountRepository::new(db.clone());
    let (p, sa) = support::sample_sa("ci-bot", org_ref());
    repo.create(&p, &sa).await.unwrap();
    let got = repo.find(&sa.principal_id).await.unwrap().unwrap();
    assert_eq!(got.name, "ci-bot");
    // principal row exists with kind=service_account
    let pr = principal::Entity::find_by_id(sa.principal_id.uuid()).one(&db).await.unwrap().unwrap();
    assert_eq!(pr.kind, "service_account");
}
#[tokio::test]
async fn duplicate_name_per_owner_conflicts() {
    let (db, _c) = support::pg().await;
    let repo = PgServiceAccountRepository::new(db.clone());
    let (p1, sa1) = support::sample_sa("dup", org_ref());
    let (p2, sa2) = support::sample_sa("dup", org_ref()); // same owner org, same name
    repo.create(&p1, &sa1).await.unwrap();
    assert!(matches!(repo.create(&p2, &sa2).await, Err(RepositoryError::Conflict(ConflictKind::ServiceAccountNameTaken))));
}
```
- [ ] **Step 2: Run — expect FAIL:** `cargo nextest run -p paigasus-iam --test service_accounts create_and_find_service_account duplicate_name_per_owner_conflicts`.
- [ ] **Step 3: Implement** (mirror `pg_repository.rs` + `pg_organizations.rs` mapping). Add `sample_sa`/`org_ref` to `tests/support/mod.rs`.
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): PgServiceAccountRepository (SMA-445)`.

---

### Task 10: `PgApiKeyRepository`

**Files:**
- Create: `src/adapters/persistence/pg_api_keys.rs`
- Modify: `persistence/mod.rs` (`pub use` + `conflict_kind` branch for `uq_api_key_hash` → `ApiKeyHashCollision`)

**Interfaces:** implements `ApiKeyRepository`. `issue` inserts one row (`key_hash` from arg). `find_by_id` returns `(ApiKey, stored_hash)`. `revoke` sets `status='revoked'`, `revoked_at=now` (idempotent-ish; revoking an already-revoked key is a no-op success). `touch_last_used` runs a guarded `UPDATE … WHERE id=? AND (last_used_at IS NULL OR last_used_at < ? )` (`now - throttle`). `list_ids_by_service_account` for archive-evict.

- [ ] **Step 1: Write failing tests** (`tests/api_keys_http.rs` or a repo-level `tests/service_accounts.rs`):
```rust
#[tokio::test]
async fn issue_find_revoke() {
    let (db, _c) = support::pg().await;
    let sar = PgServiceAccountRepository::new(db.clone());
    let (p, sa) = support::sample_sa("bot", org_ref()); sar.create(&p, &sa).await.unwrap();
    let repo = PgApiKeyRepository::new(db.clone());
    let (key, hash) = support::sample_key(&sa.principal_id);
    repo.issue(&key, &hash).await.unwrap();
    let (got, stored) = repo.find_by_id(key.id).await.unwrap().unwrap();
    assert_eq!(got.status, ApiKeyStatus::Active);
    assert_eq!(stored, hash);
    repo.revoke(key.id, Utc::now()).await.unwrap();
    let (after, _) = repo.find_by_id(key.id).await.unwrap().unwrap();
    assert_eq!(after.status, ApiKeyStatus::Revoked);
    assert!(after.revoked_at.is_some());
}
```
- [ ] **Step 2: Run — expect FAIL:** `cargo nextest run -p paigasus-iam --test service_accounts issue_find_revoke`.
- [ ] **Step 3: Implement.** Add `sample_key` helper (uses a test HMAC hasher for the hash). Add `conflict_kind` branch.
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): PgApiKeyRepository (SMA-445)`.

---

## Phase D — Authz integration

### Task 11: Entity-slice `kind` fix

**Files:**
- Modify: `src/adapters/persistence/pg_entity_slice.rs` (`principal_entity()`, ~`:131-146`)

**Interfaces:** read `kind` from the fetched `principal` row instead of the literal `"user"`; fall back to `"user"` only if the row is unexpectedly absent (preserve today's behavior for that edge).

- [ ] **Step 1: Write failing test** (`tests/authz_entity_slice.rs` already exists — add):
```rust
#[tokio::test]
async fn service_account_principal_slice_has_sa_kind() {
    let (db, _c) = support::pg().await;
    // insert a principal row with kind=service_account (via PgServiceAccountRepository)
    // load a slice for (resource, sa_principal) and assert the principal entity attr kind == "service_account"
}
```
- [ ] **Step 2: Run — expect FAIL** (still hardcoded `"user"`): `cargo nextest run -p paigasus-iam --test authz_entity_slice service_account_principal_slice_has_sa_kind`.
- [ ] **Step 3: Implement** — replace `ContextValue::Str("user".to_string())` with the row's `kind` (already fetched for `status`).
- [ ] **Step 4: Run — expect PASS.** Also re-run the full `authz_entity_slice` suite to confirm no regression for users.
- [ ] **Step 5: Commit** `fix(rs): entity-slice loader reads principal kind, not hardcoded user (SMA-445)`.

---

### Task 12: Management actions + role templates + `ALL` coverage

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/action.rs` (7 variants + `ALL` + `as_wire` + `is_write`), `authz/schema.rs` (`SCHEMA_SRC` action decls), `authz/roles.rs` (add the 7 actions to `platform_admin`, `org_admin`, `team_admin`, `project_admin` templates)

**Interfaces (Produces):** `Action::{CreateServiceAccount, GetServiceAccount, ListServiceAccounts, ArchiveServiceAccount, IssueApiKey, RevokeApiKey, ListApiKeys}` with `as_wire()` (kebab or the existing convention) and `is_write()` (writes: Create/Archive/Issue/Revoke; reads: Get/List).

- [ ] **Step 1: Write failing tests** (in `action.rs` + `authz/schema.rs` tests):
```rust
#[test]
fn all_covers_every_variant() {
    // strum or a manual exhaustive match forcing a compile error if a variant is missed;
    // assert Action::ALL.len() == <expected count after adding 7>.
    assert_eq!(Action::ALL.len(), 34); // 27 + 7
}
#[test]
fn issue_api_key_is_a_write() { assert!(Action::IssueApiKey.is_write()); }
```
Plus rely on the existing schema-validation test that compiles every role template against `SCHEMA_SRC`.
- [ ] **Step 2: Run — expect FAIL:** `cargo nextest run -p paigasus-iam-core all_covers_every_variant issue_api_key_is_a_write`.
- [ ] **Step 3: Implement** — add variants + `ALL` entries + `as_wire`/`is_write` arms; add the seven `action …;` lines to `SCHEMA_SRC` under `appliesTo { principal: [Principal], resource: [Root, Organization, Team, Project] }`; add the action names to all four admin role templates in `roles.rs`.
- [ ] **Step 4: Run — expect PASS** (incl. the schema-validation test). Also `cargo nextest run -p paigasus-iam --test authz_schema`.
- [ ] **Step 5: Commit** `feat(rs): service-account/api-key management actions and role wiring (SMA-445)`.

---

## Phase E — Adapters (crypto, cache, config)

### Task 13: `HmacSecretHasher` + `OsRngKeyEntropy` + redacting pepper config type

**Files:**
- Create: `src/adapters/api_keys/mod.rs`, `src/adapters/api_keys/hasher.rs`, `src/adapters/api_keys/entropy.rs`
- Modify: `src/adapters/mod.rs`; `rs/crates/services/paigasus-iam/Cargo.toml` (`hmac`, `sha2` already? add `hmac`); `deny.toml` if `hmac` needs a license note.

**Interfaces (Produces):**
- `struct Pepper(Vec<u8>)` — redacting: `impl Debug { "<redacted>" }`, no `Serialize` (or `#[serde(skip)]` where embedded). `Pepper::from_config(s: &str) -> Result<Self, ConfigError>` (base64/hex decode, ≥32 bytes).
- `struct HmacSecretHasher { pepper: Pepper }` : `impl SecretHasher` — `hash` = `Hmac::<Sha256>::new_from_slice(&pepper.0).chain_update(secret).finalize().into_bytes().to_vec()`; `verify` = new mac `.chain_update(secret).verify_slice(expected).is_ok()`.
- `struct OsRngKeyEntropy;` : `impl KeyEntropy` — `new_secret` fills `[0u8;32]` from `rand::rngs::OsRng`.

- [ ] **Step 1: Write failing tests** (`src/adapters/api_keys/hasher.rs`):
```rust
#[test]
fn hash_verify_roundtrip_and_reject() {
    let h = HmacSecretHasher::new(Pepper::from_config("MF1lQk...>=32bytes-base64").unwrap());
    let tag = h.hash(b"secret-bytes");
    assert!(h.verify(b"secret-bytes", &tag));
    assert!(!h.verify(b"other", &tag));
}
#[test]
fn pepper_debug_is_redacted() {
    let p = Pepper::from_config("MF1lQk...>=32bytes-base64").unwrap();
    assert_eq!(format!("{p:?}"), "Pepper(\"<redacted>\")");
}
```
- [ ] **Step 2: Run — expect FAIL:** `cargo nextest run -p paigasus-iam hash_verify_roundtrip_and_reject pepper_debug_is_redacted`.
- [ ] **Step 3: Implement.** Add `hmac` dep; ensure `deny`/`machete` clean.
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): HMAC secret hasher, entropy source, redacting pepper (SMA-445)`.

---

### Task 14: Introspection cache (memory + redis, fail-open, evict)

**Files:**
- Create: `src/adapters/api_keys/cache.rs`
- Modify: `src/adapters/api_keys/mod.rs`

**Interfaces (Produces):**
```rust
// `key_hash` is the SAME peppered HMAC stored in Postgres (safe to cache; useless without the
// in-process pepper). It is REQUIRED so every cache hit re-verifies the presented secret —
// `key_id` is a non-secret identifier embedded in the token, so a hit must NEVER authenticate
// on `key_id` alone (that would be an auth bypass). See Task 18 resolve.
#[derive(Clone)] pub struct CachedValidation { pub principal_id: PrincipalId, pub sa_status: PrincipalStatus, pub expires_at: Option<DateTime<Utc>>, pub key_hash: Vec<u8> }
#[async_trait] pub trait ApiKeyValidationCache: Send + Sync {
    async fn get(&self, key_id: ApiKeyId) -> Option<CachedValidation>;   // fail-open: errors -> None
    async fn put(&self, key_id: ApiKeyId, v: &CachedValidation);          // fail-open: errors swallowed
    async fn evict(&self, key_id: ApiKeyId);
}
```
- `MemoryApiKeyCache(Mutex<HashMap<ApiKeyId,(CachedValidation, Instant)>>)` with TTL; `RedisApiKeyCache::from_connection(conn: ConnectionManager, ttl_secs: u64)` key `iam:apikey:<keyid>` (mirror `decision_cache.rs` fail-open Redis impl at `adapters/authz/decision_cache.rs:89-149`). `CachedValidation` (de)serialized like the authz `Decision`.

- [ ] **Step 1: Write failing test** (memory backend, no Docker needed):
```rust
#[tokio::test]
async fn memory_cache_put_get_evict() {
    let c = MemoryApiKeyCache::new(30);
    let id = ApiKeyId::from_uuid(Uuid::from_u128(9));
    assert!(c.get(id).await.is_none());
    c.put(id, &CachedValidation { principal_id: pid(), sa_status: PrincipalStatus::Active, expires_at: None, key_hash: vec![1,2,3] }).await;
    assert!(c.get(id).await.is_some());
    c.evict(id).await;
    assert!(c.get(id).await.is_none());
}
```
- [ ] **Step 2: Run — expect FAIL:** `cargo nextest run -p paigasus-iam memory_cache_put_get_evict`.
- [ ] **Step 3: Implement** both impls (+ a Redis integration test mirroring `authz_cache_redis.rs`, gated on Docker).
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): fail-open api-key introspection cache (memory+redis) (SMA-445)`.

---

### Task 15: `[api_keys]` config + `validate()`

**Files:**
- Modify: `src/config.rs`; `iam.toml.example` (repo crate root)

**Interfaces (Produces):** `IamConfig.api_keys: ApiKeyConfig { pepper: Pepper, key_prefix: String, max_token_bytes: usize, default_expiry_days: Option<u32>, last_used_throttle_secs: u64, introspect_cache: ApiKeyCacheConfig { backend: CacheBackend, redis_url: Option<String>, ttl_secs: u64 } }`. Mirror `AuthzConfig`/`AuthzCacheConfig` (`config.rs:90-101`) + `Defaults`. **`validate()`:** pepper present + ≥32 decoded bytes; `key_prefix` non-empty & not equal to `"Bearer"`/`"bearer"`; `ttl_secs`>0; `redis_url` set iff `backend==redis`; `max_token_bytes` in a sane range (e.g. 32..=4096).

- [ ] **Step 1: Write failing tests** (in `config.rs` tests):
```rust
#[test]
fn rejects_empty_key_prefix() {
    let mut c = IamConfig::test_default();
    c.api_keys.key_prefix = "".into();
    assert!(c.validate().is_err());
}
#[test]
fn rejects_short_pepper() { /* < 32 decoded bytes => Err */ }
```
- [ ] **Step 2: Run — expect FAIL:** `cargo nextest run -p paigasus-iam rejects_empty_key_prefix rejects_short_pepper`.
- [ ] **Step 3: Implement** config structs + defaults + validation; document the block in `iam.toml.example`.
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): [api_keys] config block with validation (SMA-445)`.

---

## Phase F — Application

### Task 16: `CreateServiceAccount` / Get / List / Archive

**Files:**
- Create: `src/application/service_accounts.rs`; Modify `application/mod.rs`

**Interfaces (Produces):** `#[derive(Clone)] struct ServiceAccountService<R, I, C, A, K>` holding `repo: R (ServiceAccountRepository)`, `ids: I`, `clock: C`, `authorize: A (Authorize)`, `keys: Arc<dyn ApiKeyRepository>`, `cache: Arc<dyn ApiKeyValidationCache>` (last two for archive-evict). Methods: `create(actor, owner, name) -> ServiceAccount` (authorize `CreateServiceAccount` on `owner`); `get`/`list` (authorize `GetServiceAccount`/`ListServiceAccounts`); `archive(actor, sa_id)` — authorize `ArchiveServiceAccount` on the SA's owner → `repo.set_principal_status(sa, Disabled)` → enumerate `keys.list_ids_by_service_account` → `cache.evict` each → bump `entity_gen` (via the `Generations` handle, like tenancy writes). Mirror `RoleService` (`application/roles.rs:78-204`).

- [ ] **Step 1: Write failing test** (`tests/service_accounts.rs`, HTTP-level in Task 20; here a unit test with fakes from `application/fakes.rs`):
```rust
#[tokio::test]
async fn archive_disables_principal_and_evicts_keys() {
    // fakes: create SA + a key; archive; assert principal status Disabled and cache.evict called for the key id
}
```
- [ ] **Step 2: Run — expect FAIL:** `cargo nextest run -p paigasus-iam archive_disables_principal_and_evicts_keys`.
- [ ] **Step 3: Implement** (+ extend `application/fakes.rs` with fake SA/ApiKey repos + a fake cache if needed).
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): ServiceAccount application service with archive-evict (SMA-445)`.

---

### Task 17: `IssueApiKey` (D15 anti-escalation) / Revoke / List

**Files:**
- Create: `src/application/api_keys.rs`; Modify `application/mod.rs`

**Interfaces (Produces):** `#[derive(Clone)] struct ApiKeyService<K, S, I, C, A, H, E>` holding api-key repo, SA repo, ids, clock, `authorize`, `hasher: H (SecretHasher)`, `entropy: E (KeyEntropy)`, `grants: Arc<dyn RoleGrantStore>` (to enumerate the SA's grants), `cache`, `config` (prefix, default_expiry). Methods:
- `issue(actor, sa_id, scope, expires_at, scope_actions, scope_roles) -> NewApiKey`:
  1. load SA (404 if missing/disabled);
  2. **authorize (D15):** `authorize.check(actor, IssueApiKey, sa.owner)`; then `grants.list_by_principal(sa)` and for each grant `authorize.check(actor, GrantRole, grant.scope)` — any denial ⇒ `Forbidden`;
  3. `id = ids.new_api_key_id()`, `secret = entropy.new_secret()`, `hash = hasher.hash(&secret)`, `prefix = display_prefix(cfg.key_prefix, id)`, build `ApiKey` (status Active, expiry = arg or default), `plaintext = format_token(cfg.key_prefix, id, &secret)`;
  4. `repo.issue(&key, &hash)`; return `NewApiKey { key, plaintext }`.
- `revoke(actor, key_id)`: load key → authorize `RevokeApiKey` on the SA owner → `repo.revoke` → `cache.evict(key_id)`.
- `list(actor, sa_id, page)`: authorize `ListApiKeys` → `repo.list_by_service_account` (no secrets).

- [ ] **Step 1: Write failing tests** (unit, with fakes):
```rust
#[tokio::test]
async fn issue_returns_plaintext_once_and_persists_only_hash() { /* plaintext parses to the persisted id; stored row has no plaintext */ }
#[tokio::test]
async fn issue_denied_when_actor_cannot_grant_all_sa_roles() {
    // SA holds org_admin@orgX; actor has IssueApiKey@owner but not GrantRole@orgX => Forbidden (D15)
}
#[tokio::test]
async fn revoke_evicts_cache() { /* revoke calls cache.evict(key_id) */ }
```
- [ ] **Step 2: Run — expect FAIL:** `cargo nextest run -p paigasus-iam issue_returns_plaintext_once_and_persists_only_hash issue_denied_when_actor_cannot_grant_all_sa_roles revoke_evicts_cache`.
- [ ] **Step 3: Implement** per the anti-escalation algorithm (reuse the `Authorize` port + the `roles.rs` scope-resolution helper for `grant.scope`).
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): IssueApiKey with D15 anti-escalation, revoke, list (SMA-445)`.

---

### Task 18: `AuthenticateApiKey` + `introspect_api_key`

**Files:**
- Create: `src/application/authenticate_api_key.rs`; Modify `application/mod.rs`

**Interfaces (Produces):** `#[derive(Clone)] struct AuthenticateApiKey<K,S,H,C,Ca>` (api-key repo, SA repo, hasher, clock, cache). Methods:
- `resolve(token: &str) -> Result<AuthnPrincipal, AuthnError>`: `parse_token(prefix, token, max_bytes)` (Malformed → `InvalidToken`); `cache.get(keyid)` → **on hit, FIRST `hasher.verify(secret, cached.key_hash)` (constant-time; BadSecret → `InvalidToken` — never authenticate on `keyid` alone)**, then re-check `expires_at` + `sa_status` (rebuild `AuthnPrincipal`); on miss `repo.find_by_id` → `hasher.verify(secret, stored_hash)` (BadSecret → `InvalidToken`) → assert key `status=Active` & not expired (→ `InvalidToken`) → load SA principal, assert `principal.status=Active` (→ `PrincipalInactive`, D16) → `cache.put` (including the stored `key_hash`) → build `AuthnPrincipal { kind: ServiceAccount, credential: ApiKey{key_id, expires_at} }`; best-effort `repo.touch_last_used`.
- `introspect(token) -> Result<PrincipalContext, AuthnError>`: `resolve` + page the SA's memberships/role-grants (reuse `AuthenticateToken::introspect` shape at `authenticate_token.rs:144-164`).

- [ ] **Step 1: Write failing tests** (integration, `tests/api_key_auth.rs`, Docker):
```rust
#[tokio::test]
async fn valid_key_resolves_to_sa_principal() { /* issue -> resolve -> principal kind service_account */ }
#[tokio::test]
async fn revoked_key_denied() { /* issue -> revoke -> resolve => InvalidToken */ }
#[tokio::test]
async fn expired_key_denied() { /* issue with past expiry -> resolve => InvalidToken */ }
#[tokio::test]
async fn disabled_sa_denies_live_key() { /* archive SA -> resolve => PrincipalInactive (D16) */ }
```
- [ ] **Step 2: Run — expect FAIL:** `cargo nextest run -p paigasus-iam --test api_key_auth`.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): AuthenticateApiKey resolve + introspect (SMA-445)`.

---

### Task 19: Credential router at both seams + `AppState` wiring

**Files:**
- Modify: `src/adapters/http/auth_middleware.rs`, `src/adapters/grpc/authn.rs` (the credential-router branch), `src/adapters/http/mod.rs` (`AppState` + `AppState::new` + type aliases), `src/adapters/authn.rs` (HTTP error mapping for the new `ApiKeyDefect`-derived `AuthnError`), `main.rs` (unchanged). (`AuthContext` was already retyped to carry `Credential` in Task 4.)

**Interfaces (Consumes):** `AuthenticateApiKey` (Task 18), the config `key_prefix`, `AuthContext { credential }` (Task 4). **Behavior:** in both seams, after `bearer_from_headers`, branch: `token.starts_with(&cfg.key_prefix)` → `state.api_key_auth.resolve(&token)` (build `Credential::ApiKey`), else `state.authn.resolve(&token, Enabled)`. **Keep `ensure_platform_admin` inside the OIDC arm only** (D8/B2) — the API-key arm must not call it (an SA has no issuer/subject and is never a bootstrap admin). Insert the resulting `AuthContext` unchanged for handlers.

- [ ] **Step 1: Write failing test** (integration `tests/api_key_auth.rs`):
```rust
#[tokio::test]
async fn api_key_bearer_authenticates_management_call() {
    // issue a key for an SA that has org_admin@org; call GET /v1/service-accounts?owner=org with `Authorization: Bearer pgs_sk_...`
    // => 200 (SA authenticated + authorized), proving both seams route the key
}
#[tokio::test]
async fn jwt_still_authenticates() { /* an OIDC bearer still routes to the token authenticator */ }
```
- [ ] **Step 2: Run — expect FAIL:** `cargo build -p paigasus-iam` then `cargo nextest run -p paigasus-iam --test api_key_auth api_key_bearer_authenticates_management_call jwt_still_authenticates`.
- [ ] **Step 3: Implement** the two seam branches + `AppState`/`AppState::new` wiring (build `api_key_auth` = `AuthenticateApiKey` over the Pg repos + `HmacSecretHasher::new(cfg.api_keys.pepper)` + `OsRngKeyEntropy` + the cache backend selected from config — reuse the shared `redis_conn` at `http/mod.rs:238-246`). Add `pub type ApiKeySvc = …`, `pub type ServiceAccountSvc = …` aliases. Map `ApiKeyDefect`→`AuthnError`→`AuthnApiError` (401 `invalid_token`, no secret material).
- [ ] **Step 4: Run — expect PASS** (whole crate builds; both tests pass).
- [ ] **Step 5: Commit** `feat(rs): api-key bearer credential router and AppState wiring (SMA-445)`.

---

## Phase G — Wire surface

### Task 20: HTTP routes + DTOs

**Files:**
- Create: `src/adapters/http/service_accounts.rs`, `src/adapters/http/api_keys.rs`
- Modify: `src/adapters/http/mod.rs` (register + merge into `protected`; introspect route into the unauthenticated `authn` router), `src/adapters/http/dto.rs`

**Interfaces (Produces):** routes per spec §10.2. DTOs: `ServiceAccountDto`, `ApiKeyDto` (no secret), `IssueApiKeyResponseDto { api_key, token }`, `IntrospectApiKeyResponseDto`, request bodies `CreateServiceAccountBody`, `IssueApiKeyBody`. `From<Domain>` projections (mirror `RoleGrantDto` at `dto.rs:326-345`). Handlers mirror `authz.rs` create/delete (`State`, `Extension<AuthContext>`, `Json`).

- [ ] **Step 1: Write failing tests** (`tests/api_keys_http.rs`):
```rust
#[tokio::test]
async fn issue_then_list_hides_secret() {
    // POST issue -> body has `token`; GET list -> items have `prefix` but no `token`/`key_hash`
}
#[tokio::test]
async fn revoke_returns_204_and_denies() { /* DELETE key -> 204; subsequent auth with it -> 401 */ }
```
- [ ] **Step 2: Run — expect FAIL:** `cargo nextest run -p paigasus-iam --test api_keys_http`.
- [ ] **Step 3: Implement** handlers + DTOs + routing.
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): HTTP service-account and api-key endpoints (SMA-445)`.

---

### Task 21: gRPC service + convert + AuthnService introspect + exempt-set

**Files:**
- Create: `src/adapters/grpc/service_accounts.rs`
- Modify: `src/adapters/grpc/mod.rs` (`.add_service(ServiceAccountServiceServer::new(...))`; `is_exempt` for `IntrospectApiKey`), `src/adapters/grpc/convert.rs` (`to_proto_service_account`/`to_proto_api_key`/`to_introspect_api_key_response`), `src/adapters/grpc/authn.rs` (add `IntrospectApiKey` handler on `AuthnGrpc` since the RPC lives on `AuthnService`)

**Interfaces:** mirror `TenancyGrpc` (`grpc/tenancy.rs`) for management RPCs; mirror `AuthnGrpc::introspect` for `IntrospectApiKey`.

- [ ] **Step 1: Write failing tests** (`tests/api_keys_grpc.rs`):
```rust
#[tokio::test]
async fn grpc_issue_and_introspect_parity() {
    // IssueApiKey via gRPC -> token; IntrospectApiKey(token) -> principal_prn of the SA
}
#[tokio::test]
async fn management_rpcs_not_exempt() {
    // calling IssueApiKey without a bearer => Unauthenticated; IntrospectApiKey without a bearer => allowed
}
```
- [ ] **Step 2: Run — expect FAIL:** `cargo nextest run -p paigasus-iam --test api_keys_grpc`.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit** `feat(rs): gRPC service-account and api-key endpoints (SMA-445)`.

---

## Phase H — Integration & finalize

### Task 22: End-to-end acceptance + security regressions

**Files:**
- Create/extend: `rs/crates/services/paigasus-iam/tests/api_key_auth.rs` (AC-2 + security cases)

**Interfaces (Consumes):** the full stack.

- [ ] **Step 1: Write the acceptance + regression tests:**
```rust
#[tokio::test]
async fn sa_acts_authorized_by_policy() { // AC-2
    // grant the SA a role; issue a key; use the key to perform an action the role permits => allowed;
    // an action it does not permit => 403.
}
#[tokio::test]
async fn issuance_escalation_denied() { // D15
    // actor with IssueApiKey@owner but not GrantRole@orgX cannot issue a key for an SA holding org_admin@orgX
}
#[tokio::test]
async fn cached_key_denied_after_archive() { // D16 + cache evict
    // authenticate once (populates cache) -> archive SA -> next auth denied
}
```
- [ ] **Step 2: Run — expect FAIL then implement any gaps.** `cargo nextest run -p paigasus-iam --test api_key_auth`.
- [ ] **Step 3: Make green.**
- [ ] **Step 4: Full crate suite:** `cargo nextest run -p paigasus-iam --no-tests=pass` and `-p paigasus-iam-core`.
- [ ] **Step 5: Commit** `test(rs): m4 acceptance and security-regression suite (SMA-445)`.

---

### Task 23: ADR, docs, and full CI graph green

**Files:**
- Modify: `iam.toml.example` (if not already), any crate `README`/ADR cross-reference; ensure `deny.toml`/`machete` clean for `hmac`/`proptest`.

- [ ] **Step 1: Author the ADR** ("API-key & secret handling") in Notion recording D2/D3/D5/D13/D15 (HMAC+pepper over argon2, token structure, shown-once, constant-time verify, cache-evict-on-revoke, no plaintext, issuance anti-escalation, single-pepper rotation caveat). Flip status to Accepted. (Notion is outside the repo; capture the ADR link in the PR body.)
- [ ] **Step 2: fmt + clippy:** `cd rs && cargo fmt && cargo clippy --workspace -- -D warnings`. Fix.
- [ ] **Step 3: Prettier/ts gate** if any ts touched (proto TS regen): `moon run ts:fmt`.
- [ ] **Step 4: Full graph (like CI):**
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations
```
Expected: all green. Address `deny` license exceptions / `machete` ignores if flagged (temporary allowlist, prune once consumed).
- [ ] **Step 5: Commit** any config/doc fixes: `chore(rs): finalize m4 deny/machete/docs (SMA-445)`.

---

## Self-Review (author checklist — completed)

- **Spec coverage:** D1→T11; D2/D3→T3/T6/T13; D4→T17; D5→T14/T17/T18; D6→T10; D7→T3/T8; D8→T4/T19; D9→T12; D10→T8; D11→T7/T20/T21; D12→T13/T15; D13→T23; D14→T6/T13; D15→T17/T22; D16→T1/T8/T9/T18/T22. Property tests (issue AC) →T6. Constant-time →T3/T6/T13. Introspection HTTP+gRPC →T20/T21.
- **Placeholder scan:** test bodies use `/* … */` only for repetitive fixture setup whose pattern is named (precedent file cited); every novel behavior has concrete asserts. No "TBD"/"add error handling".
- **Type consistency:** `ApiKeyId`, `CachedValidation`, `ServiceAccountRepository`/`ApiKeyRepository` method names, `Credential` variants, and `Action` variants are used identically across tasks.
- **Sequencing note:** every task ends with a **green workspace**. Task 4 is deliberately workspace-atomic — it changes the core `AuthnPrincipal` *and* all its service-side consumers (`authenticate_token`, `auth.rs::AuthContext`, the two introspect projections, both seam insertions) in one commit, so no later task inherits a broken build. The API-key auth *branch* is added later (Task 19); until then every producer builds the `Oidc` variant. Ordering dependency: Task 3 (`ApiKeyId`) precedes Task 4; Tasks 7–8 (proto, migration) precede any service adapter/application task; Task 18 (`AuthenticateApiKey`) precedes Task 19 (router).
