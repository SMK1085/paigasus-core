# SMA-445: M4 API keys & service accounts — Design

- **Issue:** [SMA-445](https://linear.app/smaschek/issue/SMA-445) · Epic M4 of 6 · IAM v1 vertical slice
- **Date:** 2026-07-10
- **Status:** Approved (GATE 1 passed 2026-07-10; Stage 2 challenge folded in — see §18; D15 trust model confirmed)
- **Governing ADRs:** the **API-key & secret handling ADR** (opened by this epic; number
  assigned in Notion), ADR-0013 (Cedar authorization — the policy path an SA is authorized
  through), ADR-0014 (tenancy hierarchy & PRN scheme — SA/key scope nodes), ADR-0015
  (Authenticator port — API-key auth is a sibling authenticator), ADR-0005 (kernel-first;
  PRN + `to_cedar_uid`), ADR-0004 (proto contracts), ADR-0006 (open-core boundary).
- **Precedents:** `2026-07-07-sma-444-m3-authorization-cedar-design.md` (M3 authz — reused
  wholesale), `2026-07-06-sma-443-m2-authentication-design.md` (M2 authn — sibling credential
  path), `2026-07-05-sma-442-m1-tenancy-design.md` (M1 tenancy — `Principal`↔`User` template),
  `2026-06-30-sma-448-kernel-prn-design.md` (PRN + `to_cedar_uid`).

## 1. Context & goal

M1 shipped the tenancy spine and the `Principal`↔`User` two-table pattern; M2 added OIDC-bearer
authentication; M3 added the embedded Cedar authorizer, roles-as-grants, and the decision/entity
caches. Every one of those was built kind-agnostic on a **single `Pgs::Iam::Principal` Cedar
type** whose `kind` attribute was deliberately left forward-compatible with `service_account`
(M3 spec D3/D5; `m0004` header records "ServiceAccount was cut from M3 (GATE 1 decision)").

M4 adds **machine identity**: a `ServiceAccount` principal owned by a tenancy node, and
issuable/revocable `ApiKey`s scoped to a tenancy node. Keys are hashed at rest, validated in
constant time on a cached hot path, and shown exactly once at creation. A ServiceAccount
authenticated by a valid key can act, authorized by its M3 role grants.

Because M3 was built kind-agnostic, the authorization surface barely moves: role grants, the
grant store, both caches, and grant-change invalidation all already accept any principal PRN.
The **new** work is the credential itself (hashing, token format, constant-time validation), the
service-account/api-key domain + persistence, the API-key bearer authenticator, the cached
introspection path, and the management wire surface.

**Acceptance criteria (from the issue):**

1. Keys issue and revoke; validation is **constant-time**; **revoked/expired keys are denied**.
2. A **service account can act, authorized by policy (M3)**.

Scope posture: **full scope in one PR** (all five issue bullets, including cached introspection
and property-based validation tests), matching the M1/M3 single-large-PR rhythm. §17 decomposes
it into ordered commits.

## 2. Decisions (settled during brainstorm; refined by the Stage 2 challenge — see §18)

| # | Decision | Choice |
|---|----------|--------|
| D1 | ServiceAccount authz modeling | **Reuse the unified `Pgs::Iam::Principal` type** (M3 D3). An SA is a `principal` PRN (`prn:pgs:iam:::principal/<uuidv7>`) with `kind = "service_account"`. The **only** authz-path change is un-hardcoding `kind = "user"` in the entity-slice loader (§8). No Cedar schema entity change, no engine change, no grant-store change, no cache change. Grants, `GrantRoleRequest.principal_prn`, and the `roles.rs` `resource_type == "principal"` gate all keep working unchanged. |
| D2 | Key hashing at rest | **HMAC-SHA-256 with a server-side pepper** (a config secret), not argon2 or bare SHA-256. API keys are 256-bit random tokens, not human passwords: a memory-hard KDF buys nothing against brute force and is far too slow for a per-request hot path; the pepper means a DB dump alone cannot validate keys. Deterministic → enables an indexed lookup. Verify is constant-time (D3). |
| D3 | Token format & validation | Shown-once token: `pgs_sk_<keyid_hex>_<secret_b64url>`. `keyid` = the ApiKey's UUIDv7 (kernel-minted) rendered as its **fixed 32-char simple hex** (`Uuid::as_simple`) — embedded so introspection is an **O(1) primary-key lookup**, never a scan. `secret` = 32 random bytes (256-bit), base64url-nopad. **Parsing is by fixed width, not by delimiter** (base64url's alphabet contains `_`/`-`, so splitting on `_` would be ambiguous): strip the `pgs_sk_` prefix, read exactly 32 hex chars as `keyid`, require one `_`, the remainder is the secret. Stored: `key_hash = HMAC-SHA-256(pepper, secret)` (unique), a display `prefix`, and scope/status/expiry. **The plaintext token is never persisted.** Validate: length-cap → parse → hex-decode `keyid` → PK lookup → `hmac::Mac::verify_slice(stored_hash)` (RustCrypto HMAC verify is inherently constant-time — no `subtle` dep) → assert `status = active` and `expires_at` unset-or-future. Malformed input → an `ApiKeyDefect` whose detail appears in `Debug` only (never `Display`/logs), mirroring `TokenDefect`. |
| D4 | Key authorization model | The key **authenticates as the SA**; authorization comes entirely from the SA's M3 role grants (AC "authorized by policy (M3)"). The key stores a scope PRN + optional action/role-scope metadata, but v1 does **not** enforce key-level downscoping — the metadata is persisted and returned for a later milestone (§15). Because a key confers the SA's *whole* grant set as a bearer credential, issuance is itself a privilege-escalation vector and is gated by the anti-escalation check in **D15**. |
| D5 | Introspection caching vs revocation | A **positive-validation cache** keyed by `keyid`, short TTL (default **30 s**). The cache stores the peppered `key_hash`, and **every cache hit still constant-time-verifies the presented secret** against it (keying by the non-secret `keyid` alone must never authenticate — §9). Revoke **and** SA-archive **evict** the affected key entries (challenge M5 — archive must enumerate & evict the SA's keys, not just revoke). Postgres is the source of truth; a cache **miss/outage fails open to a real DB validation** (never an accept). Expiry is re-checked on every cache read from the cached `expires_at`. **Honest guarantee (challenge M5):** revocation is *immediate on the evicting replica*; on other replicas, and in a put-after-evict race, a stale positive is bounded by the TTL. Therefore **the memory backend is single-replica/dev only; multi-replica (HA) deployments must use the Redis backend** so eviction is global. A revocation-generation for zero-window denial is noted as future hardening (§15). Backend chosen by config (mirrors M2's `jwks_cache` / M3's authz cache). |
| D6 | `last_used_at` | **Throttled best-effort.** Updated at most once per `last_used_throttle_secs` (default 60) per key, off the request's critical path, errors swallowed. Coarse `last_used` is acceptable; write amplification on the hot path is not. |
| D7 | ApiKey identity & addressing | Plain **UUIDv7 PK** (kernel-minted), addressed by UUID in the management API (`/v1/service-accounts/{sa}/api-keys/{id}`), mirroring role-grants. The key has **no PRN of its own**; its `keyid` in the token is this UUID. Scope is a tenancy-node PRN. |
| D8 | Non-OIDC principal representation | Generalize the authenticated-principal type. `AuthnPrincipal` and the transport `AuthContext` carry a `Credential` enum — `Oidc { issuer, subject, expires_at }` \| `ApiKey { key_id, expires_at }` — replacing the flat OIDC `issuer`/`subject`/`expires_at`. **Honest blast radius (challenge B1):** this is **not** a two-file change. `AuthContext` (`adapters/auth.rs`) *carries* `issuer`+`subject` and is populated at both seams (`auth_middleware.rs`, `grpc/authn.rs`); the token-introspect projections read them (`http/dto.rs::IntrospectResponseDto::from`, `grpc/convert.rs::to_introspect_response`); and `authenticate_token.rs` builds `AuthnPrincipal`. All change (~6 files). Token introspection only ever validates JWTs, so its projections match the `Oidc` variant and treat `ApiKey` as unreachable (`debug_assert`/error). §17 commit 1 sizing updated accordingly. |
| D9 | Management actions | Seven **new fine-grained Cedar actions** (matching the existing 27-action convention): `CreateServiceAccount`, `GetServiceAccount`, `ListServiceAccounts`, `ArchiveServiceAccount`, `IssueApiKey`, `RevokeApiKey`, `ListApiKeys` — added to `SCHEMA_SRC`, the `Action` enum (`as_wire`/`is_write`/`ALL`), and **all four admin templates** — `platform_admin`, `org_admin`, `team_admin`, `project_admin` (challenge M4: ownership can be any tenancy node per D10, so a team/project admin must be able to manage SAs owned in its own subtree — the `resource in ?resource` scope already contains it). Authorized against the SA's **owner node**. A test asserts `Action::ALL` covers every enum variant (challenge: `ALL` is hand-maintained). |
| D10 | SA ownership | An SA is **owned by exactly one tenancy node** (org/team/project) via `owner_*` columns + a `ck_service_account_owner` exactly-one-of discriminator (mirroring `role_grant` scope). Ownership is the SA's home for listing + management authorization; it is distinct from role grants (which give the SA its powers, possibly at other nodes). Name is unique per owner (`uq_service_account_owner_name`). |
| D11 | Wire surface | A **new `ServiceAccountService`** (proto): SA CRUD + `IssueApiKey`/`RevokeApiKey`/`ListApiKeys`. `IssueApiKeyResponse` carries the one-time plaintext token; `ListApiKeys` returns records **without** secrets (prefix + status + `last_used` + expiry). A key-introspection RPC/endpoint mirrors token introspect. **No new grant RPCs** — existing `GrantRole`/`RevokeRole` accept any `principal_prn`. |
| D12 | Configuration | New `[api_keys]` config block: `pepper` (**required** secret via env/figment), `key_prefix` (default `pgs_sk_`), `max_token_bytes`, `default_expiry_days` (optional), `last_used_throttle_secs`, and `introspect_cache { backend, redis_url, ttl_secs }`. **The `pepper` is a redacting newtype** (challenge M6): custom `Debug` prints `"<redacted>"` and it is `#[serde(skip_serializing)]`, so it never appears in a config dump/log/`readyz` — otherwise the peppered-hash guarantee dies. `validate()`: pepper present + ≥32 decoded bytes; `key_prefix` **non-empty & not `Bearer`-colliding** (an empty prefix would route every JWT to the API-key path); non-zero TTL; `redis_url` iff redis; sane `max_token_bytes`; **redis backend required if the deployment is multi-replica** (documented, not enforced). |
| D13 | ADR | **Opens the "API-key & secret handling" ADR** (per the issue), recording: HMAC-SHA-256+pepper over argon2/SHA-256, token structure + entropy, shown-once, constant-time verify, cache-evict-on-revoke, no plaintext at rest. Set Accepted at GATE 1. |
| D14 | Crypto deps & entropy | New workspace dep **`hmac`** (RustCrypto; `sha2`, `base64`, `rand`, `getrandom` already present). Entropy (32 random bytes, pepper handling) lives in the **service adapter** via `OsRng`, behind a `KeyMaterial`/`SecretHasher` port — the pure `paigasus-kernel`/`paigasus-iam-core` stay `getrandom`-free (preserves the `wasm-getrandom-free` gate). `deny.toml` license/advisory review for `hmac` (RustCrypto is broadly already vendored). |
| D15 | Issuance anti-escalation (**challenge BLOCKER**) | Issuing a key = handing out a bearer credential for the SA's **entire** current grant set, so it must not let an actor obtain authority it couldn't grant directly. `IssueApiKey` therefore requires **both**: (a) `authorize.check(actor, IssueApiKey, sa.owner_node)`, and (b) for **every** `RoleGrant` the SA currently holds, `authorize.check(actor, GrantRole, grant.scope)` — the exact anti-escalation invariant `RoleService::grant` enforces (`roles.rs`). An actor who cannot grant a role to the SA cannot mint a key that wields it. Trust-model note for GATE 1: this makes "can I issue a key for this SA" strictly stronger than "can I manage this SA." A dedicated **escalation test** proves the cross-node case is denied. |
| D16 | SA lifecycle status is authoritative on `principal.status` (**challenge B3**) | An SA has one lifecycle status, stored on the **`principal` row** (the column the entity-slice loader and OIDC resolve already read) — there is **no separate `service_account.status` column**. `PrincipalStatus` gains a `Disabled` variant (shared with `User`; carries its `as_str`/`parse`/round-trip test, and updates the resolve guard). `ArchiveServiceAccount` flips `principal.status = Disabled`; `AuthenticateApiKey` rejects a non-active **SA** (not only a non-active key) — closing the "disabled SA still authenticates" bypass. |

## 3. Token model & validation (the crux of AC-1)

### 3.1 Anatomy

```
pgs_sk_<keyid_hex>_<secret_b64url>
└──┬──┘ └───┬────┘  └─────┬──────┘
 prefix   ApiKey UUIDv7  32 random bytes (256-bit), OsRng
        (32 fixed hex chars)
```

- `prefix` (`api_keys.key_prefix`, default `pgs_sk_`) makes keys greppable in secret scanners and
  lets the credential router (§7) cheaply distinguish an API key from an OIDC JWT.
- `keyid_hex` = `Uuid::as_simple` of the 16-byte UUIDv7 → **fixed 32 hex chars** (alphabet
  `0-9a-f`, no `_`), the DB primary key. Not secret. Fixed width makes the `_` after it an
  unambiguous separator even though the trailing secret's base64url alphabet contains `_`.
- `secret_b64url` = base64url(no-pad) of 32 CSPRNG bytes.

### 3.2 Persisted columns (never the plaintext)

`api_key { id (=keyid), service_account_id, scope node ref, prefix (display, e.g. pgs_sk_AbC3dEf…),
key_hash = HMAC_SHA256(pepper, secret) [unique], status, expires_at?, last_used_at?, created_at,
revoked_at?, scope_actions? (json), scope_roles? (json) }`.

### 3.3 Validation path (pure `verify` in the domain; I/O in the adapter)

1. Length-cap the presented string (`max_token_bytes`) — reject early (DoS guard, mirrors M2's
   `max_token_bytes`).
2. Strip `prefix`; read the fixed 32 hex chars as `keyid`, require one `_`, the remainder is the
   secret; **strictly** decode (hex for `keyid`, base64url-nopad for the secret using a
   `NO_PAD`+canonical engine that **rejects non-canonical trailing bits/overlong forms**, so one
   secret has one token string — challenge MINOR). Any structural failure → `ApiKeyDefect::Malformed`
   (no secret material in the error).
3. `keyid` → `Uuid` → **primary-key lookup**. Missing row → reject (unknown key). `keyid` is
   caller-known and useless without the secret, so short-circuiting here leaks nothing about the
   *secret*; it does leave a keyid-existence timing signal, which we **accept** (UUIDv7, non-secret).
4. `Hmac::<Sha256>::new_from_slice(pepper).chain_update(secret).verify_slice(&row.key_hash)` —
   **constant-time** comparison of the full tag. Mismatch → `ApiKeyDefect::BadSecret`.
5. Assert the **key** `status = Active` and `expires_at` null-or-future (→ `Revoked`/`Expired`),
   **and** assert the owning **SA** `principal.status = Active` (→ `PrincipalInactive`) — a disabled
   SA denies auth even with a live key (challenge B3 / D16).
6. On success → build `AuthnPrincipal { principal_id, kind: ServiceAccount, status,
   credential: ApiKey { key_id, expires_at } }`.

The pepper is the HMAC key (any length; `validate()` requires ≥32 bytes of decoded key material).

## 4. Domain model (`rs/crates/libs/paigasus-iam-core/src/`, pure — no serde/sqlx)

### 4.1 `principal.rs`
Add `PrincipalKind::ServiceAccount` (`as_str` → `"service_account"`, `parse`, round-trip test).
`principal.kind` has no CHECK constraint, so no migration ALTER is required.

### 4.2 New `service_account.rs`
```rust
pub struct ServiceAccount {
    pub principal_id: PrincipalId,   // 1:1 with a Principal (kind = ServiceAccount)
    pub owner: TenancyNodeRef,       // org | team | project (D10)
    pub name: String,                // validated via validate_name (reuse M1)
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    // NOTE: no `status` here — lifecycle status is authoritative on the `principal` row (D16).
}
impl ServiceAccount { pub fn new(id, owner, name, now) -> Result<Self, DomainError> }
```
Follows the `User` template (references `Principal` by id). **Lifecycle status lives on the
`Principal`, not the SA** (D16): `PrincipalStatus` gains `Disabled` (as_str `"disabled"`, parse,
round-trip test), the OIDC resolve guard (`authenticate_token.rs`) and `pg_repository` mapping pick
up the new arm, `ArchiveServiceAccount` flips `principal.status`, and both `AuthenticateApiKey` and
the entity-slice loader read that one status. This avoids the "disabled SA still authenticates"
bypass a separate `service_account.status` would create.

### 4.3 New `api_key.rs`
```rust
pub struct ApiKeyId(Uuid);                       // plain UUID (D7)
pub enum   ApiKeyStatus { Active, Revoked }      // as_str/parse + test
pub struct ApiKey {
    pub id: ApiKeyId,
    pub service_account_id: PrincipalId,
    pub scope: TenancyNodeRef,                   // node the key is scoped to (D4 metadata)
    pub prefix: String,                          // display only
    pub status: ApiKeyStatus,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub scope_actions: Vec<Action>,              // stored, unenforced in v1 (D4)
    pub scope_roles: Vec<String>,                // stored, unenforced in v1 (D4)
}
pub enum ApiKeyDefect { Malformed, BadSecret, Revoked, Expired }  // detail in Debug only
```
Plus a pure `NewApiKey` value carrying the freshly minted plaintext (returned once) separate from
the persisted `ApiKey`. Pure `parse_token`/`format_token` helpers live here; the CSPRNG + HMAC
are behind ports (§4.5), so the domain stays deterministic and testable.

### 4.4 `authn.rs`
Introduce `Credential`, which also carries credential-imposed expiry (OIDC always expires; an API
key may be non-expiring), so `AuthnPrincipal` no longer needs a required `expires_at`:
```rust
pub enum Credential {
    Oidc  { issuer: Issuer, subject: String, expires_at: DateTime<Utc> },
    ApiKey { key_id: ApiKeyId, expires_at: Option<DateTime<Utc>> },  // None = no credential expiry
}
```
`AuthnPrincipal` swaps its flat `issuer`/`subject`/`expires_at` for `credential: Credential` (+
keeps `principal_id`, `kind`, `status`), with an `expires_at()` accessor for the transports that
surface it. **Real change surface (challenge B1 — the earlier "downstream reads only `principal_id`"
was wrong):** (1) `authenticate_token.rs` builds the `Oidc` variant; (2) `AuthContext`
(`adapters/auth.rs`) drops its `issuer`/`subject` fields in favour of the credential (or an
`actor_prn()`-only shape) and both seams' insertions update; (3) the two **token**-introspect
projections — `http/dto.rs::IntrospectResponseDto::from` and `grpc/convert.rs::to_introspect_response`
— now match on `Credential`, reading the `Oidc` variant (token introspection only ever validates a
JWT, so `ApiKey` is unreachable there → `debug_assert!`/error). API-key introspection has its **own**
DTO/projection (§10). `PrincipalContext` is otherwise unchanged.

### 4.5 `ports.rs`
- `IdGenerator`: add `new_service_account_id() -> PrincipalId`, `new_api_key_id() -> ApiKeyId`.
- New `SecretHasher` port: `hash(&self, secret: &[u8]) -> Vec<u8>` and
  `verify(&self, secret: &[u8], expected: &[u8]) -> bool` (constant-time) — pepper injected into
  the adapter, so the domain never sees raw pepper.
- New `KeyEntropy` port: `fn new_secret(&self) -> [u8; 32]` (adapter uses `OsRng`).
- New `ServiceAccountRepository` (`create` [principal+SA in one txn], `find`, `list_by_owner`,
  `set_status`) and `ApiKeyRepository` (`issue`, `find_by_id`, `revoke`, `list_by_service_account`,
  `touch_last_used`) — `#[async_trait]`, `: Send + Sync`, return `RepositoryError`, added to the
  object-safety assertion. New `ConflictKind` variants (`ServiceAccountNameTaken`,
  `ApiKeyHashCollision`).

### 4.6 `value.rs`
Add any needed `DomainError` variants (`InvalidServiceAccountName` reuses `InvalidName`; add
`InvalidApiKeyToken` for structural token errors surfaced at the domain boundary).

### 4.7 `lib.rs`
`pub mod service_account; pub mod api_key;` + extend the re-export block.

## 5. Persistence (`services/paigasus-iam`, migration `m0005_create_service_accounts_and_api_keys`)

### 5.1 Tables
- **`service_account`** — shared PK `principal_id UUID` FK→`principal(id)` `ON DELETE CASCADE`
  (mirrors `user`); `owner_org_id`/`owner_team_id`/`owner_project_id` nullable UUIDs +
  `ck_service_account_owner CHECK (num_nonnulls(owner_org_id, owner_team_id, owner_project_id) = 1)`;
  `name TEXT`, `created_at`/`updated_at`. **No `status` column** — SA lifecycle status is on the
  `principal` row (D16). Name-unique-per-owner via **three partial unique indexes** (the m0002
  NULL-aware precedent): `uq_service_account_org_name ON (owner_org_id, name) WHERE owner_org_id IS
  NOT NULL`, and likewise for `team`/`project`. FKs to `organization`/`team`/`project` by string
  alias (per the m0002+ pattern).
- **`api_key`** — `id UUID` PK; `service_account_id UUID` FK→`service_account(principal_id)`
  `ON DELETE CASCADE`; `scope_org_id`/`scope_team_id`/`scope_project_id` + `ck_api_key_scope`
  disjunction; `prefix TEXT`, `key_hash TEXT` (the peppered HMAC stored as **lowercase hex** — the `ApiKeyRepository` hex-encodes on write and decodes back to `Vec<u8>` on read; the raw HMAC bytes are not valid TEXT) with `uq_api_key_hash UNIQUE`, `status TEXT`,
  `expires_at TIMESTAMPTZ NULL`, `last_used_at TIMESTAMPTZ NULL`, `created_at`, `revoked_at NULL`,
  `scope_actions TEXT NULL` (JSON), `scope_roles TEXT NULL` (JSON); `ix_api_key_service_account`
  on the FK. All uniques/checks via raw `ALTER TABLE … ADD CONSTRAINT <name>` (names are
  load-bearing for `conflict_kind`). Enum columns are TEXT (no native PG enum).

Registered in `migration/mod.rs` (5th `Box::new`), `entities/mod.rs`, `persistence/mod.rs`
(`pub use` + `conflict_kind` branches for the two new constraint names).

### 5.2 Entities & adapters
- `entities/service_account.rs` (shared-PK `belongs_to` principal, mirrors `user.rs`),
  `entities/api_key.rs` (own UUID PK, empty `Relation`, adapters query directly).
- `pg_service_accounts.rs`, `pg_api_keys.rs` — `#[derive(Clone)]` over `DatabaseConnection`,
  `*_to_model`/`model_to_*` hand-mapping (a parse failure on stored data → `Backend`, never a
  silent default), pagination via `Page` + `ORDER BY created_at, id`.
- **Creation mirrors `create_user`:** `principal` row + `service_account` row in one transaction,
  principal first. Issuing a key is a single insert. `touch_last_used` is a throttled `UPDATE`
  (adapter-side guard: skip if `last_used_at > now - throttle`).

## 6. Application layer (`services/paigasus-iam/src/application/`)

- `service_accounts.rs`: `CreateServiceAccount`, `GetServiceAccount`, `ListServiceAccounts`,
  `ArchiveServiceAccount` — each `authorize.check(actor, Action::X, owner_node)` before acting;
  create mints via `IdGenerator` + `Clock`, then `repo.create`. **`ArchiveServiceAccount`** flips
  `principal.status = Disabled` (D16), **evicts every one of the SA's cached keys** (enumerate
  keyids by SA → `cache.evict`, challenge M5), and bumps `entity_gen` (a future policy may read
  `principal.status`; cheap, reuses `Generations`).
- `api_keys.rs`: `IssueApiKey` — **two-part authorization (D15, challenge BLOCKER):**
  (a) `authorize.check(actor, IssueApiKey, sa.owner_node)`, then (b) for **every** `RoleGrant` the
  SA currently holds, `authorize.check(actor, GrantRole, grant.scope)` (mirrors `RoleService::grant`
  anti-escalation — the actor cannot mint a credential wielding authority it could not itself grant).
  Then mint id + secret via ports → hash → persist → return `NewApiKey` with the plaintext **once**.
  Non-idempotent: a lost response strands an active, secret-unknown key — the client's remedy is
  revoke-and-reissue (documented; an idempotency key is deferred, §15). `RevokeApiKey` (authorize
  → set status Revoked + `revoked_at` → **`cache.evict(keyid)`**), `ListApiKeys` (authorize →
  paginated, no secrets).
- `authenticate_api_key.rs`: `AuthenticateApiKey` (mirrors `AuthenticateToken`): parse+validate
  (cache-first, §9) → `AuthnPrincipal` for the SA; `introspect_api_key` → `PrincipalContext`
  (principal + role grants) reusing M3's membership/grant paging.
- Register in `application/mod.rs`; wire in `AppState::new` (repos + `KernelIdGenerator` +
  `SystemClock` + the new hasher/entropy/cache adapters).

## 7. Authentication wiring (API-key bearer — the sibling authenticator)

The token→`AuthContext` seam is centralized in `adapters/auth.rs` + two enforcement call-sites
(`http/auth_middleware.rs::require_bearer`, `grpc/authn.rs::AuthEnforce::call`), both of which
call `state.authn.resolve(&token, Enabled)` today. Add **one credential-router branch** at each:
if `bearer_from_headers(...)` starts with `api_keys.key_prefix` → `state.api_key_auth.resolve(token)`,
else the existing OIDC `resolve`. Both yield an `AuthnPrincipal` → the **same** `AuthContext`
insertion (handlers and the authz middleware read only the principal, unchanged). **One thing is
*not* shared (challenge B2):** the seams currently call
`bootstrap_seeder.ensure_platform_admin(id, issuer, subject)` right after `resolve`, which is
**OIDC-only** (an SA has no issuer/subject and is never a bootstrap platform_admin). That call moves
**inside the OIDC arm**, before `AuthContext` insertion; the API-key arm skips it. The
`WiredAuthenticator` memory/redis enum is the precedent for building `api_key_auth` in
`AppState::new`.

API-key auth errors funnel through the existing `AuthnApiError`/`authn_status` (401
`invalid_token`, 403 `principal_inactive`, 503 `unavailable`) — an `AuthnError` mapping for the new
`ApiKeyDefect`s (no secret material in any response).

## 8. Authorization (SA as a first-class principal)

- **Entity-slice loader** (`pg_entity_slice.rs`): the one hardcode to change — `principal_entity()`
  reads `kind` from the `principal` row (already fetched by PK) instead of the literal `"user"`.
  With SAs minted as `principal` PRNs, `to_cedar_uid` yields `Pgs::Iam::Principal` and the slice
  is correct with no schema change.
- **New management actions** (D9) added to `authz/schema.rs` `SCHEMA_SRC` (`appliesTo { principal:
  [Principal], resource: [Root, Organization, Team, Project] }`), `authz/action.rs` (`Action`
  enum + `ALL`/`as_wire`/`is_write`), and the `platform_admin`/`org_admin` templates in
  `authz/roles.rs`. Adding actions to the schema re-runs write-time validation of every system
  template (existing test).
- **Cache invalidation for SA grant changes is already automatic**: `role_grant.principal_id` is
  kind-agnostic and `PgRoleGrantStore::grant/revoke` already bump `policy_gen`. No new mechanism.
- **Runtime authorization of an acting SA** flows through the unchanged `CedarAuthorizer` path:
  authenticated SA principal + its role grants → `is_authorized`.

## 9. Introspection & caching (`adapters/api_keys/` or `adapters/authz/`-style)

- Port `ApiKeyValidationCache { get(keyid) -> Option<CachedValidation>; put(keyid, &v);
  evict(keyid) }`. `CachedValidation` = `{ principal_id, sa_status, expires_at, key_hash }`. It stores
  the **peppered `key_hash`** (the same HMAC already in Postgres — safe to cache: useless without the
  in-process pepper) so the hot path can **re-verify the presented secret on every hit** without a DB
  round-trip. It **never** stores the plaintext secret. **The cache is keyed by `keyid` (a non-secret
  identifier embedded in the token), so a cache hit MUST NOT authenticate on `keyid` alone** — every
  hit runs the constant-time `hasher.verify(secret, key_hash)` before returning; a mismatch is
  `InvalidToken`, never a success. (Skipping the secret check on a hit would be an auth bypass: an
  attacker who learned a cached `keyid` could present any secret. Corrected during implementation.)
- `MemoryApiKeyCache` + `RedisApiKeyCache` (reusing M3's shared `ConnectionManager`), key
  `iam:apikey:<keyid>`, short TTL `introspect_cache.ttl_secs` (default 30 s). **Fail-open**: `get`
  errors → `None` (fall through to DB); `put` errors → logged+swallowed. Eviction triggers:
  `RevokeApiKey`, `ArchiveServiceAccount` (all the SA's keyids), and any read that observes expiry.
  Backend chosen by config (memory/redis enum), matching M2/M3.
- **Revocation-vs-cache honesty (challenge M5):** `MemoryApiKeyCache` evicts only the local replica
  → it is **single-replica/dev only**; HA must use Redis so eviction is global. Even on Redis a
  **put-after-evict race** (a concurrent `resolve` that DB-validated *before* the revoke and
  `put`s *after* the evict) can re-seed a positive entry for up to one TTL; this is why the TTL is
  short, and a per-key revocation-generation is the future zero-window fix (§15). Expiry never has
  this problem — it is recomputed from the cached `expires_at` on every read.
- Hot path: `AuthenticateApiKey.resolve` → parse token → `cache.get(keyid)` hit → **verify the secret
  against the cached `key_hash` (constant-time)** → re-check expiry + cached `sa_status` → return; on a
  miss → full DB validate (§3.3) → `cache.put`. A hit skips the two DB round-trips (`find_by_id` +
  `find_principal`) but never the secret check. Introspection endpoint uses the same path.

## 10. Wire surface

### 10.1 Proto (`contracts/proto/paigasus/iam/v1/iam.proto`)
New `ServiceAccount` and `ApiKey` messages (string `prn`/ids, `google.protobuf.Timestamp`,
embedded `AuditMetadata`, UPPER_SNAKE status enums, `_UNSPECIFIED = 0`). New `ServiceAccountService`:
`CreateServiceAccount`, `GetServiceAccount`, `ListServiceAccounts` (uint32 limit/uint64 offset),
`ArchiveServiceAccount`, `IssueApiKey` (→ `IssueApiKeyResponse { ApiKey api_key; string token; }` —
token shown once), `RevokeApiKey`, `ListApiKeys` (records w/o secrets). **`IntrospectApiKey` lives on
`AuthnService`** next to the token `Introspect` (challenge MINOR), not on the otherwise-bearer-gated
`ServiceAccountService` — so the single unauthenticated method sits with its peer and the exempt-set
match stays obvious. Registered in the tonic router (`grpc/mod.rs`, wrapped by the existing
`AuthLayer`; `IntrospectApiKey` added to the `is_exempt` set like token `Introspect`, with a test
asserting the `ServiceAccountService` management RPCs are **not** exempt). Buf codegen via the
committed prost/tonic pipeline (`moon run contracts:generate`); breaking-change gate respected
(reserve-and-add).

### 10.2 HTTP (axum)
- Management (bearer-gated, merged into `protected`): `POST/GET /v1/service-accounts`,
  `GET/DELETE /v1/service-accounts/{sa}`, `POST/GET /v1/service-accounts/{sa}/api-keys`,
  `DELETE /v1/service-accounts/{sa}/api-keys/{id}`. Handlers mirror `authz.rs` role-grant CRUD
  (State + `Extension<AuthContext>` + `Json` DTOs; `IssueApiKey` returns `201` with the one-time
  token; management errors via `ApiError`/`TenancyError`).
- Introspection (unauthenticated, body-limited, merged outside the bearer layer like token
  introspect): `POST /v1/authn/api-keys/introspect`.
- New DTOs in `http/dto.rs` (`ServiceAccountDto`, `ApiKeyDto` [no secret], `IssueApiKeyResponseDto`
  [with token], `IntrospectApiKeyResponseDto`) with `From<Domain>` projections.

### 10.3 gRPC
`ServiceAccountGrpc` mirrors `TenancyGrpc` (actor extraction, optional enforce, `convert::to_proto_*`).
`convert.rs` gains `to_proto_service_account`/`to_proto_api_key`/`to_introspect_api_key_response`.

## 11. Configuration (`config.rs`, figment) — new `[api_keys]` block

```toml
[api_keys]
# pepper = "<base64/hex secret>"   # REQUIRED (env IAM_API_KEYS__PEPPER); >=32 bytes decoded.
#                                   # Redacting newtype: never logged/serialized (D12).
key_prefix              = "pgs_sk_" # must be non-empty & not collide with "Bearer"
max_token_bytes         = 512
# default_expiry_days   = 365       # optional; unset = non-expiring until revoked
last_used_throttle_secs = 60

[api_keys.introspect_cache]
backend  = "memory"                 # memory (single-replica/dev) | redis (required for HA)
# redis_url = "redis://..."         # required iff backend = "redis"
ttl_secs = 30                       # short: bounds the cross-replica/put-after-evict staleness (D5)
```
`pepper` is a redacting newtype (custom `Debug` → `"<redacted>"`, `#[serde(skip_serializing)]`) with
a test asserting it never appears in `{:?}` (D12/challenge M6). `validate()`: pepper present + ≥32
decoded bytes; `key_prefix` non-empty & not `Bearer`-colliding; non-zero TTL; `redis_url` iff redis;
sane `max_token_bytes`. Mirrors the M3 `[authz]` defaults/validation precedent; documented in
`iam.toml.example`.

## 12. ADR

Opens the **API-key & secret handling ADR** (Notion), recording D2/D3/D5/D13: HMAC-SHA-256+pepper
(rationale vs argon2/SHA-256), token structure + 256-bit entropy + prefix, shown-once, constant-time
verify, no plaintext at rest, cache-evict-on-revoke, pepper as an operational secret (rotation
noted as a follow-up). Set Accepted at GATE 1; add to the crate's ADR cross-references.

## 13. Testing

- **Domain unit** (`paigasus-iam-core`): `PrincipalKind`/`ApiKeyStatus` round-trips; `format_token`
  ↔ `parse_token`; SA/ApiKey constructors + validation; `ApiKeyDefect` `Display` scrubs detail.
- **Property-based** (`proptest`, new dev-dep — the issue explicitly requires this): for arbitrary
  peppers/secrets/ids — (a) round-trip `issue → verify` always accepts; (b) any bit-flip /
  truncation / wrong-pepper / wrong-keyid → reject; (c) revoked or expired → reject; (d) arbitrary
  bytes into `parse_token`/`verify` **never panic** and never accept without the exact secret.
- **Integration** (`tests/`, testcontainers, mirroring `authz_*`/`tenancy_*`): issue → introspect →
  **SA acts and is authorized by a role grant** (AC-2); revoke → denied (incl. cache eviction);
  expiry → denied; wrong secret → denied; HTTP + gRPC parity; shown-once (token only in the issue
  response, absent from list/get); list omits secrets. A focused **constant-time** assertion on the
  verify port (AC-1) — structural (uses `verify_slice`) plus a coarse timing sanity check, not a
  flaky microbenchmark.
- **Security regression tests (from the challenge):**
  (a) **Escalation** (D15/BLOCKER): an actor with `IssueApiKey` at the SA's owner node but lacking
  authority over a role the SA holds at *another* node is **denied** issuance.
  (b) **Disabled-SA bypass** (D16): archiving the SA denies both a fresh introspection and a
  *cached* key (eviction) even though the key row is still `Active`.
  (c) **Pepper redaction** (M6): the pepper never appears in `format!("{:?}", config)` nor in the
  serialized config.
  (d) **`Action::ALL` coverage**: a test fails if a new `Action` variant is missing from `ALL`.
  (e) **Exempt-set**: `IntrospectApiKey` is exempt; `ServiceAccountService` management RPCs are not.

## 14. Build / CI wiring

- New workspace dep `hmac` → `deny.toml` license check (RustCrypto is Apache/MIT) + `proptest`
  dev-dep. Run the full graph before pushing (per CLAUDE.md): `moon ci :build :test :lint :fmt :deny
  :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base
  origin/main --include-relations`.
- Kernel/domain stay `getrandom`-free (entropy is service-side) → `:wasm-getrandom-free` stays green.
- Proto change → `:breaking` (reserve-and-add only) + regenerate committed bindings.
- No new crate `dependsOn paigasus-kernel-rs`, so the `:affected-smoke` strict-equality set is
  untouched.

## 15. Out of scope (deferred, stated deliberately)

- **Key-level action/role downscope enforcement** — metadata stored (D4), not enforced in v1.
- Pepper **rotation** / multi-pepper validation: v1 has a single pepper, so a rotation is a **hard,
  fleet-wide cutover that invalidates all keys at once** — documented for operators (challenge Q).
  A dual-pepper verify-then-migrate is the follow-up.
- A per-key **revocation-generation** for zero-window cross-replica denial (v1 bounds staleness by
  the short cache TTL instead, D5).
- Key **rotation** endpoints; automatic expiry **sweeps** (expired keys are denied lazily on read,
  not reaped); `IssueApiKey` **idempotency keys** (v1 remedy is revoke-and-reissue).
- Per-key **rate limiting** / usage quotas on the unauthenticated introspection oracle (it already
  requires presenting a valid key, matching the token-introspect posture); per-request `last_used`
  precision (coarse only, D6).
- A distinct Cedar `ServiceAccount` **entity type** (unified `Principal` is deliberate, D1).
- SA **memberships** UI beyond ownership; custom (non-system) roles for SAs.

## 16. AC traceability

| AC | Satisfied by |
|----|--------------|
| Keys issue & revoke | §6 `IssueApiKey`/`RevokeApiKey` (+ D15 anti-escalation on issue); §10 endpoints; §13 integration |
| Validation is constant-time | D2/D3 `hmac::verify_slice`; §3.3; §13 property + constant-time test |
| Revoked/expired keys denied | Postgres is authoritative; D5 evict-on-revoke/archive + expiry-on-read (§3.3 step 5). Immediate on the evicting replica; on other replicas/put-after-evict, bounded by the short cache TTL (Redis required for HA). §13 (a)(b) |
| SA can act, authorized by policy (M3) | D1 unified principal; §8 entity-slice fix + grants; §13 AC-2 test |

## 17. Decomposition (keeps "one PR" coherent — ordered commits)

1. Domain: `PrincipalKind::ServiceAccount`, `PrincipalStatus::Disabled` (D16), `service_account.rs`,
   `api_key.rs`, the `Credential` enum + its ~6-file OIDC-side surface (D8), ports
   (`SecretHasher`/`KeyEntropy`/`IdGenerator`/repos) + unit + property tests.
2. Persistence: `m0005` (no `service_account.status` col), entities, `pg_service_accounts`,
   `pg_api_keys`, `conflict_kind`.
3. Authz: entity-slice `kind` fix; new actions in schema/action + all four admin templates (D9)
   + schema-validation & `Action::ALL`-coverage tests.
4. Application: `CreateServiceAccount`/…, `ArchiveServiceAccount` (flip `principal.status` + evict
   keys), `IssueApiKey` (**D15 anti-escalation**), `AuthenticateApiKey`/introspect (SA-status check).
5. Auth wiring + cache: credential router at both seams (OIDC-only bootstrap seeding, D8/B2);
   redacting pepper newtype; hasher/entropy adapters; introspection cache (memory+redis);
   `AppState`/config + `validate()`.
6. Wire surface: proto + regen (`IntrospectApiKey` on `AuthnService`); HTTP routes/DTOs; gRPC
   service + convert + exempt-set test.
7. Integration + security-regression tests (HTTP+gRPC), `iam.toml.example`, ADR, CI graph green.

## 18. Change log — Stage 2 adversarial challenge

The spec-challenger (Opus) verdict was **APPROVE WITH CHANGES** (it verified the D1 "reuse the
unified `Principal`" claim against real code and confirmed the crypto reasoning). All findings were
justified; none were rejected. Folded in:

| Severity | Finding | Fold-in |
|----------|---------|---------|
| **BLOCKER** | Key issuance is a confused-deputy escalation — gated only by owner-node management, but an SA may hold grants at *other* nodes | **D15** + §6: issuance now also requires the actor to pass `GrantRole` authz for **every** grant the SA holds (mirrors `RoleService::grant`); §13(a) escalation test |
| MAJOR | D8 blast-radius claim false — `AuthContext`/two introspect projections/`authenticate_token` also read `issuer`/`subject`/`expires_at` | D8 + §4.4 rewritten with the honest ~6-file surface; §17 commit 1 resized |
| MAJOR | API-key seam breaks the OIDC-only `ensure_platform_admin` call | §7: bootstrap seeding moved inside the OIDC arm; API-key arm skips it |
| MAJOR | Three overlapping status fields; disabled-SA could still authenticate | **D16** + §3.3 step 5 + §4.2 + §5.1: `principal.status` authoritative (`+Disabled`), no `service_account.status`, `AuthenticateApiKey` checks the SA; §13(b) test |
| MAJOR | SA ownership (any node) vs management authz (org/platform only) asymmetry | D9: management actions added to **all four** admin templates |
| MAJOR | Cache revocation not "immediate" — memory multi-replica, put-after-evict race, archive doesn't evict | D5/§9 rewritten: memory = single-replica/dev, Redis for HA, archive evicts, short TTL bound stated; AC-1 wording made honest (§16) |
| MAJOR | `pepper` leaks via config `Debug`/`Serialize` | D12/§11: redacting newtype (`Debug`→`<redacted>`, `skip_serializing`) + §13(c) test |
| MINOR | base64url non-canonical → malleable token | §3.3 step 2: strict/canonical decoding |
| MINOR | `key_prefix=""` routes all JWTs to the API-key path | §11 `validate()`: non-empty & non-`Bearer` prefix |
| MINOR | `Action::ALL` hand-maintained, no coverage test | D9 + §13(d) coverage test |
| MINOR | Unauthenticated `IntrospectApiKey` on a bearer-gated service is an exempt-set footgun | §10.1: moved to `AuthnService`; §13(e) not-exempt test |
| MINOR | `IssueApiKey` non-idempotent (lost response strands a key) | §6 + §15: documented revoke-and-reissue remedy |
| MINOR | keyid-existence timing oracle | §3.3 step 3: explicitly accepted (keyid non-secret) |
| QUESTION | Pepper rotation = fleet-wide cutover; entity_gen on SA status change | §15: documented; `ArchiveServiceAccount` bumps `entity_gen` (§6) |

**Trust-model note carried to GATE 1:** D15 makes "can I issue a key for this SA" *strictly
stronger* than "can I manage this SA" (the issuer must dominate everything the SA can do). This is
the safe default and matches M3's grant invariant; flagged for Sven's confirmation.
