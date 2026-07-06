# SMA-443: M2 Authentication (BYO-IdP / OIDC) — Design

- **Issue:** [SMA-443](https://linear.app/smaschek/issue/SMA-443) · Epic M2 of 6 · IAM v1 vertical slice
- **Date:** 2026-07-06
- **Status:** Approved (GATE 1 passed 2026-07-06; challenge folded in)
- **Governing ADRs:** ADR-0015 (Authenticator port & OIDC strategy — opened by this epic,
  Proposed), ADR-0005 (kernel-first), ADR-0004 (proto contracts), ADR-0014 (tenancy
  hierarchy & PRN scheme)
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
| D8 | ADR | This epic opens **ADR-0015: Authenticator port & OIDC strategy** (Notion, drafted at GATE 1 approval, currently Proposed → flipped Accepted by Sven). |

Decisions added by the adversarial challenge (Stage 2):

| # | Decision | Choice |
|---|----------|--------|
| D9 | JIT atomicity | Principal + user + external_identity are written by **one port method owning one transaction** — `ExternalIdentityRepository::provision(&Principal, &User, &ExternalIdentity)` (M1 precedent: `OrganizationRepository::create` writes org + default team in one txn). Two separate port calls cannot share a transaction in this codebase's port design. |
| D10 | Introspect is non-provisioning | `Introspect` is **read-only**: an unknown `(issuer, subject)` returns `identity_not_provisioned` even when JIT is enabled. JIT runs only on the **authenticated API path** (middleware). Rationale: Introspect is middleware-exempt and caller auth is M4 — an unauthenticated endpoint must not have a user-creation side effect. |
| D11 | Token type | IAM validates **access tokens** (resource-server semantics), never ID tokens. Consequence: the BYO IdP must emit `aud` and (for JIT) `email` in the access token — for Keycloak that is two protocol mappers in the realm (§8); "config only" (AC 1) holds for IdPs that can be configured to do so. |
| D12 | Authn error funnel | `AuthnError` gets a **dedicated response path** — its own `IntoResponse` (401/403/503 + `WWW-Authenticate`) and its own gRPC status mapping. It does **not** reuse `TenancyError`/`ErrorClass`/`status_to_grpc` (they can only express 400/404/409/500). The JIT use case intercepts `Conflict(EmailTaken)` into `ProvisioningFailed(EmailConflict)` so it never reaches the tenancy funnel as a 409. |
| D13 | Hot-path shape | The middleware **resolves the principal only** (token verify + identity lookup + JIT; no membership fetch). Full `PrincipalContext` assembly (memberships via internal pagination) happens **only in `Introspect`**. |
| D14 | Layer placement | Auth attaches **inside `router()` / the gRPC service construction**, not in `serve_http` — the existing `oneshot`/ephemeral-port test harnesses must exercise enforcement (else every existing test is a false green). One validator core, two thin transport adapters (axum JSON + `WWW-Authenticate`; tonic `grpc-status` trailers). |
| D15 | Redis trust boundary | The JWKS cache holds the **trust anchors' public keys**: whoever can write to it can forge tokens. `backend = "redis"` requires a dedicated/isolated instance with TLS + AUTH on `redis_url`; this is documented as an operator requirement, and memory remains the default. |

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

- `AuthnPrincipal` — what the middleware produces (D13):

```text
AuthnPrincipal { principal_id: PrincipalId, kind: PrincipalKind, status: PrincipalStatus,
                 issuer: Issuer, subject: String, expires_at: DateTime<Utc> }
```

- `PrincipalContext` — what introspection produces:

```text
PrincipalContext { principal: AuthnPrincipal,
                   memberships: Vec<MembershipRecord>,   // the type list_by_principal returns
                   role_groups: Vec<Prn> /* empty until M3 */ }
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
- `PrincipalInactive` — mapped principal exists but `status != Active`. **Forward-looking
  and unreachable in M2**: `PrincipalStatus` currently has only `Active`; there is no local
  disable path, so account revocation in M2 is delegated entirely to the IdP. The guard is
  specified now so suspend/delete (later milestone) needs no authn change; its failing
  branch is unit-tested via a fake repository, not the Pg adapter.
- `Unavailable` — JWKS/discovery unreachable or cache backend down; retryable, distinct
  from token invalidity.
- `Backend(String)` — persistence failure during lookup/provisioning.

### 3.4 Ports (`ports.rs`, `#[async_trait]`, M0/M1 style)

- `Authenticator` — **the pluggable port this epic exists to open**:
  `authenticate(&self, token: &str) -> Result<ValidatedClaims, AuthnError>`.
  The OIDC validator (§4) is the v1 implementation; `paigasus-cloud` adds managed-IdP /
  opaque-token implementations later with zero core change.
- `ExternalIdentityRepository` — `find_by_issuer_subject(&Issuer, &str) ->
  Result<Option<ExternalIdentity>, RepositoryError>` and
  `provision(&Principal, &User, &ExternalIdentity) -> Result<(), RepositoryError>` (D9):
  one method, one transaction spanning all three inserts, so a lost JIT race or an email
  conflict rolls the whole provisioning back — no orphan principals/users. (Two separate
  port calls could not be atomic: each Pg adapter method owns its own `db.begin()`.)
- `PrincipalRepository` — grows `find_principal(&PrincipalId) -> Result<Option<Principal>,
  RepositoryError>` (principal + status without the user profile; introspection and the
  middleware don't need `User`). `create_user` is untouched; JIT goes through
  `ExternalIdentityRepository::provision` (D9).
- `IdGenerator` — grows `new_external_identity_id() -> Uuid` (UUIDv7, kernel-minted like
  membership ids). No kernel changes.
- `MembershipRepository` — reused as-is (`list_by_principal`) for context assembly.

## 4. OIDC validation adapter (`services/paigasus-iam/src/adapters/oidc/`)

`OidcAuthenticator` implements `Authenticator` for the configured issuer set.

### 4.1 Validation pipeline (per token)

The input is the **access token** presented as a bearer credential (D11); IAM never
validates ID tokens. Token length is capped (`authn.max_token_bytes`, default 16 KiB)
before any decoding.

1. Decode the JOSE header only → `alg`, `kid`. Reject `alg ∉ {RS256, ES256}` (D6).
2. Decode the unverified `iss` claim → must equal a configured `Issuer` exactly, else
   `InvalidToken`. (The unverified read only *selects* the trust anchor; nothing is trusted
   until the signature verifies against that issuer's keys.)
3. JWKS lookup via `JwksCache` (§4.3); select the key by `kid` and assert the JWK's
   `kty`/`alg` (when present) are consistent with the header `alg`.
4. `jsonwebtoken::decode` with that key and `Validation.algorithms` pinned to **exactly the
   header `alg`** (not the full allowed set): signature, `exp` (leeway `authn.leeway_secs`,
   default 60), `iss` (must round-trip to the same issuer), `aud` (must intersect the
   issuer's configured `audiences`; `azp` is not evaluated in v1 — `aud` intersection is
   the authorization-party check, §10).
5. Map standard claims (`sub`, `email`, `name`, `locale`, `zoneinfo`) → `ValidatedClaims`.

### 4.2 Discovery

`GET {issuer}/.well-known/openid-configuration` via `reqwest` (rustls, timeout
`authn.http_timeout_secs`, default 10). The document's `issuer` field must match exactly
(RFC 8414 / OIDC Discovery §4.3); `jwks_uri` must be `https`. Discovery and JWKS response
bodies are capped at 1 MiB (read via bounded streaming, not `.text()` on an unbounded
body). The discovery doc is cached alongside the JWKS with the same TTL. Issuer URLs come
from operator config (trusted), so SSRF exposure is limited to operator error; the `https`
requirement still holds. (A jwks_uri-host-must-match-issuer rule was considered and
rejected: real IdPs serve JWKS cross-host, e.g. Google's `accounts.google.com` issuer vs
`googleapis.com` JWKS.)

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
- **Trust boundary (D15):** the cache stores the issuers' *public signing keys* — a
  writable cache is an auth bypass (attacker-substituted keys verify forged tokens).
  `backend = "redis"` therefore requires a dedicated, network-isolated instance with TLS +
  AUTH in `redis_url`; this is a documented operator requirement (config example + README
  note), and `memory` stays the default. The cached entry is only ever a copy of an
  authenticated `https` fetch; nothing else writes the key.

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
Two entry points (D10, D13):

```text
resolve(token, provisioning) -> Result<AuthnPrincipal, AuthnError>:
  claims  = authenticator.authenticate(token)?              // §4
  ident   = external_identities.find_by_issuer_subject(...)?
  match ident:
    Some -> principal = principals.find_principal(...)?      // must exist (FK)
    None -> match provisioning:
      Enabled  -> jit_provision(claims)?                     // §6.2 (issuer flag still applies)
      Disabled -> IdentityNotProvisioned                     // Introspect path, D10
  guard principal.status == Active else PrincipalInactive
  -> AuthnPrincipal { ... }                                  // no membership fetch

introspect(token) -> Result<PrincipalContext, AuthnError>:
  principal   = resolve(token, Disabled)                     // read-only, never provisions
  memberships = memberships.list_by_principal(...)           // internal pagination (page 200)
                                                             // until exhaustion; counts are
                                                             // small in v1 (bounded by tenancy)
  -> PrincipalContext { principal, memberships, role_groups: vec![] }
```

The middleware calls `resolve(token, Enabled)` — JIT happens on any *authenticated API
call* (AC 2); `Introspect` never provisions (D10).

### 6.2 JIT provisioning rules

1. Runs only when the token's issuer has `jit_provisioning = true` (config, default true).
2. `email` claim required → else `ProvisioningFailed(MissingEmail)`. Profile mapping:
   `email` → `Email::parse`, `name` → display_name (fallback: email local part),
   `locale`/`zoneinfo` → `locale`/`timezone`. Standard OIDC claims only; per-issuer claim
   mapping is deferred (§10).
3. One call to `ExternalIdentityRepository::provision(principal, user, external_identity)`
   (D9): a single transaction spans all three inserts, entities constructed in the use case
   with a kernel-minted principal PRN (M0 `CreateUser` construction conventions).
4. Races resolve via the DB inside that transaction: a concurrent JIT for the same
   `(issuer, subject)` loses on `uq_external_identity_issuer_subject`, the transaction rolls
   back atomically (no orphan principal/user), and the loser **re-reads and proceeds with
   the winner's row** (idempotent outcome). An email conflict (existing `user_email_key`
   constraint → `ConflictKind::EmailTaken`) also rolls back fully and is intercepted by the
   use case as `ProvisioningFailed(EmailConflict)` (D12) — never auto-link (D5).
5. Operational consequence of D5 (documented, accepted): a user provisioned via issuer A
   whose email also arrives in a valid token from issuer B gets a deterministic 403
   (`provisioning_failed`) on the B path — there is no account-linking remediation inside
   M2 (an admin linking API is deferred, §10); remediation is at the IdP (change the email)
   or in config (drop the second issuer).

### 6.3 Error → transport mapping

Dedicated funnel (D12): a new `AuthnApiError(AuthnError)` with its own `IntoResponse`
(status + `WWW-Authenticate` header support) and a new `authn_status_to_grpc` in
`grpc/convert.rs`. The existing `ApiError`/`ErrorClass`/`status_to_grpc` machinery is for
tenancy errors only and is not touched. The three 403 subcodes were reviewed for
enumeration risk and kept: they are only reachable with a *valid token from a trusted
issuer*, and they materially aid operators debugging JIT failures.

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
max_token_bytes = 16384             # default

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
— decided in M4/M5). The stale "SCAFFOLD ONLY (SMA-441) … M0 defines no RPCs" header text
is refreshed at the same time (it predates M1's `TenancyService`). Regenerate via
`moon run contracts:generate`; `:breaking` gate is additive-safe.

### 7.2 HTTP

`POST /v1/authn/introspect` `{"token": "..."}` → 200 `IntrospectResponse`-shaped JSON
(dto.rs projections, snake_case, PRN strings). Errors per §6.3; a `token` exceeding
`max_token_bytes` is a 401 `invalid_token`, and the request body is size-limited for this
route. The introspected token is itself the credential — the endpoint is middleware-exempt
— but it is **read-only** (D10): it never JIT-provisions, so the unauthenticated surface
has no write side effect. Caller-level service auth (API keys) is M4. Request bodies for
this route are never logged.

### 7.3 gRPC

`AuthnGrpc` (`adapters/grpc/authn.rs`) on the existing tonic server, sharing
`AuthenticateToken` via `AppState`. `convert.rs` grows domain → proto for
`PrincipalContext`.

### 7.4 Auth middleware & enforcement

One **validator core** (extract `Authorization: Bearer <token>` — only source, no
cookies/query — then `AuthenticateToken::resolve(token, Enabled)`), wrapped by **two thin
transport adapters** (D7, D14) — a naive single layer cannot serve both transports because
the reject rendering differs:

- HTTP adapter (`axum::middleware::from_fn_with_state` on the `/v1` router): success
  inserts `AuthContext { principal_id, issuer, subject }` as a request extension (consumed
  by M3 authz / M5 audit; M2 handlers don't read it yet); failure renders §6.3 JSON +
  `WWW-Authenticate`.
- gRPC adapter (tower layer around the tonic services): failure renders a proper
  `grpc-status`/`grpc-message` response, never a bare HTTP 401.
- Exemptions — HTTP: `/readyz`, `POST /v1/authn/introspect`; gRPC (by `:path` prefix):
  `/grpc.health.v1.Health/`, `/paigasus.iam.v1.AuthnService/Introspect`.
- **Placement (D14):** the layers attach where the integration tests exercise the service —
  inside `router()` and the gRPC service construction — NOT in `serve_http`/`grpc::serve`
  (today's `TraceLayer`/`TimeoutLayer` live in `serve_http`; auth there would be invisible
  to every `oneshot` test — a false green). Effective order stays `TraceLayer` →
  `TimeoutLayer` → auth (401s traced and time-bounded).

`AppState::new(db)` becomes `AppState::new(db, &IamConfig)` (signature change rippling
through `main.rs` and every test constructor) and grows the wired `AuthnSvc` type alias
(OIDC authenticator + Pg repos + kernel id gen + system clock).

## 8. Testing

- **Unit (inline `#[cfg(test)]`, fakes per M0/M1 convention):** claim-pipeline policy
  (alg rejection, iss/aud/exp/leeway matrix) against locally-signed tokens; JIT rules incl.
  missing-email, email-conflict, race-loser re-read (fake repos); cache behavior (TTL expiry,
  kid-miss refetch, cooldown suppression, single-flight) with a fake clock + counting fake
  fetcher; middleware mapping table (§6.3); config boot validation.
- **Mock IdP integration (`tests/support`):** an in-process axum server serving
  `/.well-known/openid-configuration` + JWKS, signing tokens with **committed test-only key
  fixtures**: an RSA private PEM **plus its precomputed public JWK** (`n`/`e` — jsonwebtoken
  has no PEM→JWK conversion, and this avoids the `rsa` crate / RUSTSEC-2023-0071), and an
  EC P-256 pair likewise so the **ES256 path is exercised end-to-end** (D6), all clearly
  marked as fixtures with no secret value. `support::mod` gains `start_mock_idp()` and
  `bearer(claims)`; `app(...)` wires the test issuer. **Suite-wide refactor, stated
  plainly:** every existing HTTP/gRPC integration test constructs `AppState` via the new
  `AppState::new(db, &IamConfig)` and sends bearer tokens — all ~10 tenancy/health test
  files are touched. New tests: introspect round-trip (incl. non-provisioning, D10), JIT
  end-to-end via an authenticated API call, 401/403 surfaces, key-rotation (mock IdP swaps
  kid; cooldown honored), multi-issuer, ES256.
- **Redis integration:** testcontainers `redis` module exercising `RedisJwksCache`
  (hit/miss/TTL + fail-closed `Unavailable` when the container stops).
- **Keycloak end-to-end (AC 1):** testcontainers `GenericImage`
  (`quay.io/keycloak/keycloak`, `start-dev --import-realm` with the realm JSON mounted).
  The realm fixture must make the **access token** (D11) satisfy §4: a client with
  direct-access grants, a test user, an **audience protocol mapper** (adds `paigasus` to
  `aud`) and an **email-claim mapper on the access token** — vanilla Keycloak access tokens
  carry neither, and without the mappers AC 1/AC 2 cannot pass. Readiness: wait on the
  mgmt health endpoint (or the "started" log line) — `start-dev` takes ~30 s. The test
  builds `IamConfig` pointing at the container realm (**config only**), obtains a real
  token via password grant (reqwest), calls a tenancy endpoint (JIT fires) + `Introspect`,
  asserts provisioning and correct principal context. Docker-gated exactly like
  `start_migrated_postgres()` (hard-fail in CI, skip locally without Docker).

## 9. Build / CI wiring

- New workspace deps: `jsonwebtoken` (MIT; the `jwk` parsing surface — `jwk::JwkSet`,
  `DecodingKey::from_jwk` — plus serde on `JwkSet` for the Redis serialization path; exact
  feature flags confirmed against the crate at implementation) and `redis` (BSD-3-Clause;
  features `tokio-comp` + `connection-manager`) — `rs/deny.toml` license review (add
  exceptions only if `:deny` says so). `reqwest` (already declared workspace-level) becomes
  consumed for the first time.
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
  refresh tokens (IAM never sees them — clients talk to the IdP); rate limiting /
  provisioning caps; JWKS warm-up at boot; PS256/EdDSA; `azp` validation (v1 relies on
  `aud` intersection, §4.1); an admin account-linking API (the multi-issuer same-email
  remediation, §6.2 rule 5); local principal suspend/disable (revocation is IdP-side in
  M2, §3.3).
