# SMA-443: M2 Authentication (BYO-IdP / OIDC) — Design

- **Issue:** [SMA-443](https://linear.app/smaschek/issue/SMA-443) · Epic M2 of 6 · IAM v1 vertical slice
- **Date:** 2026-07-06
- **Status:** Drafted (Stage 1; adversarial challenge pending)
- **Governing ADRs:** new **Authenticator port & OIDC strategy** ADR (opened by this epic,
  drafted at GATE 1; next free number), ADR-0005 (kernel-first), ADR-0004 (proto contracts),
  ADR-0014 (tenancy hierarchy & PRN scheme)
- **Precedents:** `docs/superpowers/specs/2026-07-05-sma-442-m1-tenancy-design.md` (M1 tenancy),
  `docs/superpowers/specs/2026-07-04-sma-441-paigasus-iam-walking-skeleton-design.md` (M0 skeleton)

## 1. Context & goal

M1 shipped the tenancy spine (org → team → project + memberships) over HTTP and gRPC, with
every endpoint deliberately unauthenticated and actor context deferred to M2. M2 closes that
gap with **provider-agnostic OIDC authentication (BYO-IdP)**: a pluggable `Authenticator`
port whose v1 implementation validates external JWTs (discovery, JWKS fetch + cache +
rotation, signature, `exp`/`aud`/`iss`), an `ExternalIdentity` `(issuer, subject) → User`
mapping with multi-issuer config, configurable just-in-time provisioning, an `Introspect`
call returning a principal context, and auth enforcement on the existing API surface.
No password storage in the core — authentication is fully delegated to the customer's IdP.

Acceptance criteria (from the issue):

1. A real OIDC IdP authenticates a user end-to-end with **config only** (no code change).
2. An unknown `(issuer, subject)` JIT-provisions a `User`.
3. `Introspect` returns the correct principal context.

## 2. Decisions (settled during brainstorm)

| # | Decision | Choice |
|---|----------|--------|
| D1 | OIDC implementation | **`jsonwebtoken` + `reqwest`** (already a workspace dep): hand-rolled discovery + JWKS fetch behind adapter-internal abstractions. Not `openidconnect` (heavy RP library aimed at auth-code flows IAM never performs; its internal JWKS handling fights D2) and not `josekit` (smaller ecosystem; extra algorithms speculative for v1). Generalizes the old `paigasus-iam-cognito` validator. |
| D2 | JWKS caching | `JwksCache` trait **in the OIDC adapter module** (an infra detail of one adapter, not a domain port). Two impls behind config: `InMemoryJwksCache` (default) and `RedisJwksCache` (new `redis` workspace dep). |
| D3 | Enforcement scope | **Everywhere**: all `/v1` HTTP routes and the `TenancyService` gRPC surface require a valid bearer token. Exempt: `/readyz`, `POST /v1/authn/introspect`, `grpc.health.v1.Health`, `AuthnService/Introspect`. |
| D4 | Introspect placement | New **`AuthnService { Introspect }`** in `iam/v1` (HTTP + gRPC) now; the reserved `AuthorizationService { IsAuthorized, Introspect }` stays M4/M5 (header comment updated). `role_group_prns` ships empty until M3 Cedar. |
| D5 | JIT semantics | Per-issuer `jit_provisioning` flag, **default `true`** (trusting an issuer implies trusting its user base). `email` claim **required** for JIT. **Never auto-link by email** — an email-uniqueness conflict fails provisioning (auto-linking is an account-takeover vector). |
| D6 | Algorithms | **RS256 + ES256 only**, asymmetric by construction; every other `alg` (incl. `none`, HS*) is rejected before key lookup. |
| D7 | gRPC auth mechanism | A shared **tower middleware** wraps both the axum router and the tonic server — tonic interceptors are synchronous and token validation is async (JWKS fetch). One implementation, two transports. |
| D8 | ADR | This epic opens the **Authenticator port & OIDC strategy** ADR (Notion, next free number). Drafted from this spec's decisions at GATE 1 approval, Proposed → Accepted before implementation. |

## 3. Domain model (`rs/crates/libs/paigasus-iam-core`, pure)

New module `authn.rs`; extensions to `value.rs`, `ports.rs`.

### 3.1 Value objects

- `Issuer` — `parse(&str) -> Result<Issuer, DomainError>`: absolute `https` URL, no fragment,
  stored **exactly as configured** (OIDC requires exact string match on `iss`; no trailing-slash
  or case normalization). `as_str()`, `Display`.
- `ValidatedClaims` — IdP-agnostic output of the `Authenticator` port:

```text
ValidatedClaims { issuer: Issuer, subject: String, audiences: Vec<String>,
                  expires_at: DateTime<Utc>,
                  email: Option<String>, name: Option<String>,
                  locale: Option<String>, zoneinfo: Option<String> }
```

- `PrincipalContext` — what introspection and the auth middleware produce:

```text
PrincipalContext { principal_id: PrincipalId, kind: PrincipalKind, status: PrincipalStatus,
                   issuer: Issuer, subject: String, expires_at: DateTime<Utc>,
                   memberships: Vec<Membership>, role_groups: Vec<Prn> /* empty until M3 */ }
```

### 3.2 Entities

```text
ExternalIdentity { id: Uuid, principal_id: PrincipalId, issuer: Issuer, subject: String,
                   created_at, updated_at }
```

Plain **UUIDv7 primary key, no PRN** (same reasoning as M1 `Membership`, D5 there): an
external identity is a mapping, never referenced by policies/keys/budgets. Timestamps are
µs-truncated `DateTime<Utc>` from the `Clock` port.

### 3.3 Error taxonomy

`AuthnError` (in core, transport-agnostic):

- `InvalidToken(TokenDefect)` — malformed JWT, unsupported/forbidden `alg`, unknown `kid`
  (after one refresh), bad signature, expired/not-yet-valid, `iss` not configured,
  `aud` mismatch. The defect enum is for **logs and tests only**; wire responses carry a
  coarse code (§6.4) — no token material or claim detail leaves the service.
- `IdentityNotProvisioned` — valid token, unknown `(issuer, subject)`, JIT disabled.
- `ProvisioningFailed(ProvisioningDefect)` — JIT attempted but impossible:
  `MissingEmail` | `EmailConflict`.
- `PrincipalInactive` — mapped principal exists but `status != Active`.
- `Unavailable` — JWKS/discovery unreachable or cache backend down; retryable, distinct
  from token invalidity.
- `Backend(String)` — persistence failure during lookup/provisioning.

### 3.4 Ports (`ports.rs`, `#[async_trait]`, M0/M1 style)

- `Authenticator` — **the pluggable port this epic exists to open**:
  `authenticate(&self, token: &str) -> Result<ValidatedClaims, AuthnError>`.
  The OIDC validator (§4) is the v1 implementation; `paigasus-cloud` adds managed-IdP /
  opaque-token implementations later with zero core change.
- `ExternalIdentityRepository` — `find_by_issuer_subject(&Issuer, &str) ->
  Result<Option<ExternalIdentity>, RepositoryError>`, `create(&ExternalIdentity)`.
  JIT's create-or-race is handled in the use case via the unique constraint (§5.2 rule 4).
- `PrincipalRepository` — grows `find_principal(&PrincipalId) -> Result<Option<Principal>,
  RepositoryError>` (principal + status without the user profile; introspection and the
  middleware don't need `User`). `create_user` gains no new surface — JIT reuses it plus
  `ExternalIdentityRepository::create` in one transaction (§5.2).
- `IdGenerator` — grows `new_external_identity_id() -> Uuid` (UUIDv7, kernel-minted like
  membership ids). No kernel changes.
- `MembershipRepository` — reused as-is (`list_by_principal`) for context assembly.

## 4. OIDC validation adapter (`services/paigasus-iam/src/adapters/oidc/`)

`OidcAuthenticator` implements `Authenticator` for the configured issuer set.

### 4.1 Validation pipeline (per token)

1. Decode the JOSE header only → `alg`, `kid`. Reject `alg ∉ {RS256, ES256}` (D6).
2. Decode the unverified `iss` claim → must equal a configured `Issuer` exactly, else
   `InvalidToken`. (The unverified read only *selects* the trust anchor; nothing is trusted
   until the signature verifies against that issuer's keys.)
3. JWKS lookup via `JwksCache` (§4.3); select the key by `kid`.
4. `jsonwebtoken::decode` with that key: signature, `exp` (leeway `authn.leeway_secs`,
   default 60), `iss` (must round-trip to the same issuer), `aud` (must intersect the
   issuer's configured `audiences`).
5. Map standard claims (`sub`, `email`, `name`, `locale`, `zoneinfo`) → `ValidatedClaims`.

### 4.2 Discovery

`GET {issuer}/.well-known/openid-configuration` via `reqwest` (rustls, timeout
`authn.http_timeout_secs`, default 10). The document's `issuer` field must match exactly
(RFC 8414 / OIDC Discovery §4.3); `jwks_uri` must be `https`. The discovery doc is cached
alongside the JWKS with the same TTL. Issuer URLs come from operator config (trusted), so
SSRF exposure is limited to operator error; the `https` requirement still holds.

### 4.3 `JwksCache` + rotation

```text
trait JwksCache { get(&Issuer) -> Option<CachedJwks>; put(&Issuer, CachedJwks); }
CachedJwks { jwks: JwkSet, jwks_uri: String, fetched_at: DateTime<Utc> }
```

- **TTL refresh**: entries older than `authn.jwks_ttl_secs` (default 3600) are refetched
  on next use.
- **Rotation (kid-miss)**: an unknown `kid` triggers **one** forced refetch, rate-limited by
  a per-issuer cooldown (`authn.jwks_refresh_cooldown_secs`, default 30) so unknown-kid spam
  cannot DoS the IdP. Still unknown after refresh → `InvalidToken`.
- **Single-flight**: one in-flight fetch per issuer (per-issuer `tokio::sync::Mutex`);
  concurrent validations await the same fetch. Cooldown + single-flight state is per-process
  even with Redis (only the JWKS payload is shared).
- `InMemoryJwksCache` — `RwLock<HashMap<Issuer, CachedJwks>>`.
- `RedisJwksCache` — key `iam:jwks:<issuer>`, value = serialized `CachedJwks`, Redis TTL =
  `jwks_ttl_secs`; `redis` crate with tokio support + `ConnectionManager` (auto-reconnect).
  Cache backend down → `Unavailable` (fail closed, but distinguishable from a bad token).

## 5. Persistence

### 5.1 Migration `m0003_create_external_identity`

Table `external_identity` (named constraints, M1 D7 convention):

```text
id            uuid PK            (pk_external_identity)
principal_id  uuid NOT NULL      fk_external_identity_principal → principal(id)
issuer        text NOT NULL
subject       text NOT NULL
created_at    timestamptz NOT NULL
updated_at    timestamptz NOT NULL
UNIQUE (issuer, subject)         (uq_external_identity_issuer_subject)
INDEX (principal_id)             (ix_external_identity_principal)
```

No FK cascade — principals are never hard-deleted in v1 (status-based lifecycle).

### 5.2 Adapter

`PgExternalIdentityRepository` (SeaORM, `adapters/persistence/pg_external_identities.rs` +
`entities/external_identity.rs`). Error mapping by **constraint name** via the shared
`map_err`/`conflict_kind` (M1 D7): `uq_external_identity_issuer_subject` →
`RepositoryError::Conflict(ConflictKind::ExternalIdentityExists)` (new variant). JIT's
transaction spans principal + user + external_identity inserts (§6.2 use case).

## 6. Application layer (`services/paigasus-iam/src/application/`)

### 6.1 `AuthenticateToken` use case (new `authenticate_token.rs`)

Generic-by-value like M1 use cases:
`AuthenticateToken<A: Authenticator, E: ExternalIdentityRepository, P: PrincipalRepository, M: MembershipRepository, I: IdGenerator, C: Clock>`.

```text
execute(token) -> Result<PrincipalContext, AuthnError>:
  claims  = authenticator.authenticate(token)?              // §4
  ident   = external_identities.find_by_issuer_subject(...)?
  match ident:
    Some -> principal = principals.find_principal(...)?      // must exist (FK)
    None -> jit_provision(claims)?                           // §6.2, or IdentityNotProvisioned
  guard principal.status == Active else PrincipalInactive
  memberships = memberships.list_by_principal(...)           // unpaginated internal fetch
  -> PrincipalContext { ..., role_groups: vec![] }
```

### 6.2 JIT provisioning rules

1. Runs only when the token's issuer has `jit_provisioning = true` (config, default true).
2. `email` claim required → else `ProvisioningFailed(MissingEmail)`. Profile mapping:
   `email` → `Email::parse`, `name` → display_name (fallback: email local part),
   `locale`/`zoneinfo` → `locale`/`timezone`. Standard OIDC claims only; per-issuer claim
   mapping is deferred (§10).
3. One transaction: insert principal (kind `User`, status `Active`), user, external_identity
   — reusing the M0 `CreateUser` construction path with a kernel-minted principal PRN.
4. Races resolve via the DB: a concurrent JIT for the same `(issuer, subject)` loses on
   `uq_external_identity_issuer_subject`, then **re-reads and proceeds with the winner's row**
   (idempotent outcome). An email conflict (existing `user_email_key` constraint →
   `ConflictKind::EmailTaken`) → `ProvisioningFailed(EmailConflict)` — never auto-link (D5).

### 6.3 Error → transport mapping

| `AuthnError` | HTTP | gRPC |
|---|---|---|
| `InvalidToken(_)` | 401 + `WWW-Authenticate: Bearer error="invalid_token"` | `UNAUTHENTICATED` |
| `IdentityNotProvisioned` | 403 code `identity_not_provisioned` | `PERMISSION_DENIED` |
| `ProvisioningFailed(_)` | 403 code `provisioning_failed` | `PERMISSION_DENIED` |
| `PrincipalInactive` | 403 code `principal_inactive` | `PERMISSION_DENIED` |
| `Unavailable` | 503 | `UNAVAILABLE` |
| `Backend(_)` | 500 (opaque, M1 rule) | `INTERNAL` |

Bodies use the existing `{"error":{code,message}}` envelope. Messages are static per code —
no claim values, token fragments, or upstream error text (M1 PII rule; NFR "never logged"
applies to token material in logs too: log defect kinds, `kid`, issuer — never the token).

### 6.4 Configuration (`config.rs`, figment)

```toml
[authn]
leeway_secs = 60                    # default
http_timeout_secs = 10              # default
jwks_ttl_secs = 3600                # default
jwks_refresh_cooldown_secs = 30     # default

[authn.jwks_cache]
backend = "memory"                  # "memory" (default) | "redis"
# redis_url = "redis://..."         # required iff backend = "redis"

[[authn.issuers]]
issuer = "https://idp.example.com/realms/acme"
audiences = ["paigasus"]
jit_provisioning = true             # default
```

Boot validation (fail fast): ≥ 1 issuer; issuers unique; each `audiences` non-empty;
issuer URLs valid per §3.1; `backend = "redis"` ⇒ `redis_url` present. Issuer list is
file-config; scalar env overrides (`IAM_*`) keep working as today. Startup does **not**
fetch JWKS (lazy on first use) — `/readyz` stays independent of IdP availability.

## 7. Wire surface

### 7.1 Proto (`contracts/proto/paigasus/iam/v1/iam.proto`)

```proto
service AuthnService {
  rpc Introspect(IntrospectRequest) returns (IntrospectResponse);
}
message IntrospectRequest  { string token = 1; }
message IntrospectResponse {
  string principal_prn = 1;
  string status = 2;                            // principal status
  string issuer = 3;
  string subject = 4;
  google.protobuf.Timestamp expires_at = 5;
  repeated Membership memberships = 6;          // reuse tenancy message
  repeated string role_group_prns = 7;          // empty until M3
}
```

Header comment updated: `AuthnService` is M2; `AuthorizationService { IsAuthorized }`
remains reserved for M4/M5 (its future `Introspect` folds into what M2 ships, or extends it
— decided in M4/M5). Regenerate via `moon run contracts:generate`; `:breaking` gate is
additive-safe.

### 7.2 HTTP

`POST /v1/authn/introspect` `{"token": "..."}` → 200 `IntrospectResponse`-shaped JSON
(dto.rs projections, snake_case, PRN strings). Errors per §6.3. The introspected token is
itself the credential — the endpoint is middleware-exempt; caller-level service auth (API
keys) is M4.

### 7.3 gRPC

`AuthnGrpc` (`adapters/grpc/authn.rs`) on the existing tonic server, sharing
`AuthenticateToken` via `AppState`. `convert.rs` grows domain → proto for
`PrincipalContext`.

### 7.4 Auth middleware & enforcement

One async tower middleware (`adapters/http/auth.rs` or `adapters/middleware.rs`), applied to
**both** transports (D3, D7):

- Extract `Authorization: Bearer <token>` (only source; no cookies/query).
- Run `AuthenticateToken` → insert `AuthContext { principal_id, issuer, subject }` as a
  request extension (consumed by M3 authz / M5 audit; M2 handlers don't read it yet).
- Failures short-circuit per §6.3.
- HTTP exemptions: `/readyz`, `POST /v1/authn/introspect`. gRPC exemptions (by `:path`
  prefix): `/grpc.health.v1.Health/`, `/paigasus.iam.v1.AuthnService/Introspect`.
- Layer order: `TraceLayer` → `TimeoutLayer` → auth (401s are traced and time-bounded).

`AppState` grows the wired `AuthnSvc` type alias (OIDC authenticator + Pg repos + kernel id
gen + system clock), constructed in `AppState::new` from `IamConfig`.

## 8. Testing

- **Unit (inline `#[cfg(test)]`, fakes per M0/M1 convention):** claim-pipeline policy
  (alg rejection, iss/aud/exp/leeway matrix) against locally-signed tokens; JIT rules incl.
  missing-email, email-conflict, race-loser re-read (fake repos); cache behavior (TTL expiry,
  kid-miss refetch, cooldown suppression, single-flight) with a fake clock + counting fake
  fetcher; middleware mapping table (§6.3); config boot validation.
- **Mock IdP integration (`tests/support`):** an in-process axum server serving
  `/.well-known/openid-configuration` + JWKS, signing tokens with a **committed test-only RSA
  PEM** (avoids the `rsa` crate and RUSTSEC-2023-0071; keys are fixtures, clearly marked, no
  secret value). `support::mod` gains `start_mock_idp()` and `bearer(claims)`; `app(db)` wires
  the test issuer. All existing HTTP/gRPC tenancy tests updated to send bearer tokens.
  New tests: introspect round-trip, JIT end-to-end, 401/403 surfaces, key-rotation
  (mock IdP swaps kid), multi-issuer.
- **Redis integration:** testcontainers `redis` module exercising `RedisJwksCache`
  (hit/miss/TTL + fail-closed `Unavailable` when the container stops).
- **Keycloak end-to-end (AC 1):** testcontainers `GenericImage`
  (`quay.io/keycloak/keycloak`, `start-dev`, realm-import JSON fixture: realm + client with
  direct-access grants + test user). Test builds `IamConfig` pointing at the container realm
  (**config only**), obtains a real token via password grant (reqwest), calls a tenancy
  endpoint + `Introspect`, asserts JIT provisioning and correct principal context.
  Docker-gated exactly like `start_migrated_postgres()` (hard-fail in CI, skip locally
  without Docker).

## 9. Build / CI wiring

- New workspace deps: `jsonwebtoken` (MIT), `redis` (BSD-3-Clause) — `rs/deny.toml` license
  review (both in the common allowlist; add exceptions only if `:deny` says so). `reqwest`
  becomes consumed — drop any machete allowlist entry for it if present.
- No new crates, no kernel changes → no `ci/affected-graph/run.sh` expected-set edits, no
  binding glue churn.
- `contracts:generate` regen committed (ADR-0004); `:breaking` additive.
- Moon: no new projects; `paigasus-iam-rs` / `paigasus-iam-core-rs` task graph unchanged.
- Full pre-push gate per CLAUDE.md (`moon ci :build :test :lint :fmt :deny :machete
  :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free
  --base origin/main --include-relations`).

## 10. Out of scope (deferred)

- **M3:** Cedar authorization, role groups (`role_groups` stays empty), consuming
  `AuthContext` for authz decisions.
- **M4:** ServiceAccounts, API keys, caller authentication on `Introspect`,
  `AuthorizationService { IsAuthorized }`.
- **M5:** audit-log population from `AuthContext`, domain events (identity provisioned…).
- **Cloud:** managed-IdP / opaque-token `Authenticator` impls.
- Per-issuer claim-name mapping; SCIM; token revocation / back-channel logout;
  refresh tokens (IAM never sees them — clients talk to the IdP); rate limiting;
  JWKS warm-up at boot; PS256/EdDSA.
