# SMA-454 — M2 authn follow-up: deferred cleanup batch (design)

**Issue:** [SMA-454](https://linear.app/smaschek/issue/SMA-454) · **Source:** final
whole-branch review of SMA-443 (see
`docs/superpowers/specs/2026-07-06-sma-443-m2-authentication-design.md`)
**Status:** draft for GATE 1 review

## 1. Problem & scope

The SMA-443 final review triaged 14 items as OK-TO-DEFER — none are correctness or
security defects. This batch lands all of them in one PR: 5 test-coverage adds
(T1–T5), 6 cleanups (C1–C6), and 3 hardening nice-to-haves (H1–H3). Everything is in
`rs/` — the `paigasus-iam` service crate, one item in `paigasus-iam-core`, one in
`rs/deny.toml`. No proto, py, or ts changes. No behavior change except H1–H3 (each
deliberately small and additive).

**Scope decision:** the full batch ships as one PR (the issue batches it
deliberately; every item is small). Alternatives considered: tests+cleanups only
(defers hardening a second time for no gain); dropping the `AuthContext` move
(see D1). Rejected — a second follow-up issue for ~3 small items is more overhead
than value.

## 2. Test-coverage adds

- **T1 — validator branch tests** (`adapters/oidc/validator.rs` tests):
  - *kty/alg mismatch:* a manually crafted token whose header says `alg: RS256`
    with the stub JWKS serving the EC P-256 key under the same `kid` (the
    `manual_token` pattern — the check fires after the JWKS lookup but before any
    signature verification). Expect `InvalidToken(UnsupportedAlg)`.
  - *empty `aud`:* a signed token whose `aud` is `[]` (test-local claims struct
    with `aud: Vec<String>`). Expect `InvalidToken(AudienceMismatch)`.
- **T2 — use-case tests** (`application/authenticate_token.rs` tests):
  - `name` claim absent + valid email → provisioned `display_name` equals the
    email local part.
  - `email` claim present but unparseable (e.g. `"not-an-email"`) →
    `ProvisioningFailed(MissingEmail)` (same as absent).
  - `locale`/`zoneinfo` claims pass through untouched onto the provisioned `User`.
- **T3 — bearer-parsing negatives:** unit tests on the new shared
  `bearer_from_headers` (C1): `Bearertoken abc` (no space) → `None`; lowercase
  `bearer abc` → `Some("abc")`; `Basic abc` → `None`; `Bearer ` (empty credential)
  → `None`. Plus two integration cases in `tests/http_authn.rs`: `Bearertoken <valid>`
  → 401; lowercase `bearer <valid token>` → 200 (piggybacks the existing
  valid-token harness).
- **T4 — `Issuer::parse` interior whitespace** (`paigasus-iam-core` authn tests):
  `https://idp.example .com` and a tab variant → rejected (the branch at
  `authn.rs:29` is currently uncovered).
- **T5 — Redis JWKS round-trip full equality** (`tests/redis_jwks_cache.rs`):
  replace the piecemeal field asserts with `assert_eq!(got.jwks, jwks.jwks)`
  (jsonwebtoken's `JwkSet` derives `PartialEq`; if that assumption fails at build
  time, fall back to comparing `serde_json::to_value` of both).

## 3. Cleanups

- **C1 — shared bearer extraction + `adapters::auth` home.** New module
  `adapters/auth.rs` holding:
  - `pub fn bearer_from_headers(headers: &http::HeaderMap) -> Option<String>` —
    the single copy of the 8-line extractor (RFC 7235 case-insensitive scheme,
    trim, reject empty). The HTTP middleware calls it with `request.headers()`;
    the gRPC layer calls it directly. Both local copies deleted.
  - `AuthContext` moves here from `http/auth_middleware.rs` (**D1**, see §5).
    Import sites updated (middleware, gRPC layer); no re-export shim — the crate
    is private, and M3 will consume it from the new home.
- **C2 — `map_principal_row` helper** (`persistence/pg_repository.rs`): extract
  `fn map_principal_row(pm: principal::Model) -> Result<Principal, RepositoryError>`
  (Prn/kind/status parsing); `find_user` and `find_principal` both call it.
  `find_user` derives the `PrincipalId` it needs for `User::new` from the returned
  `Principal`'s own id.
- **C3 — stale comment** (`config.rs:171`): the `#[allow(clippy::result_large_err)]`
  justification says "two Jail tests"; the module now has ~14. Reword to drop the
  count (e.g. "scoped to this module's Jail-based tests").
- **C4 — parse issuers once** (`adapters/oidc/validator.rs`):
  `OidcAuthenticator::new` parses each configured issuer to an `Issuer` at
  construction and stores a validator-local `ConfiguredIssuer { issuer: Issuer,
  audiences: Vec<String> }` (the validator never reads `jit_provisioning`). `new`
  becomes `Result<Self, AuthnError>` (an unparseable issuer is
  `AuthnError::Backend` — `IamConfig::validate` already guarantees this can't
  happen at boot, so this is a wiring-defect guard, mirroring the existing
  `redis_url` treatment in `AppState::new`). The per-request
  `Issuer::parse(&issuer_config.issuer)` at `validator.rs:158` disappears; the
  request path matches `iss` against `configured.issuer.as_str()`. Call sites:
  two arms in `AppState::new` (already `Result`-returning) and the test helper.
- **C5 — dead `unwrap_or("")`** (`paigasus-iam-core/src/authn.rs:33`):
  `rest.split('/').next()` can never be `None` (split yields ≥1 element), so the
  fallback is dead. Replace with
  `let host = rest.split_once('/').map_or(rest, |(host, _)| host);` — both arms
  live, same semantics. (The issue text says "authn.rs `split('/')`" — this core
  file is the actual site; the service's authn adapters have no such code.)
- **C6 — deny.toml hygiene** (`rs/deny.toml`):
  - Move `BSL-1.0` from the global `[licenses].allow` list to a crate-scoped
    exception: `{ name = "xxhash-rust", allow = ["BSL-1.0"] }`.
  - Drop the `RUSTSEC-2025-0111` (tokio-tar) ignore — stale since SMA-453 bumped
    testcontainers to 0.27, which no longer pulls tokio-tar.
  - Verified by the `repo:deny` gate in the full CI graph run.

## 4. Hardening

- **H1 — introspect body limit + JSON-rejection envelope** (`http/authn.rs`,
  `http/mod.rs`):
  - Route-specific `DefaultBodyLimit::max(max_token_bytes + 1024)` on
    `POST /v1/authn/introspect` (1024 bytes of envelope headroom over the token
    itself). The limit is plumbed as a new `AppState` field set from
    `cfg.authn.max_token_bytes` in `AppState::new` (the router currently sees no
    config). Note this *lowers* the effective limit from axum's 2 MB default —
    that is the point: the only legitimate payload is
    `{"token":"<≤max_token_bytes>"}`.
  - A failed `Json<IntrospectBody>` extraction currently renders axum's plain-text
    rejection. Replace with a small crate-local extractor (wrapping
    `Json::from_request` and mapping `JsonRejection`) that renders the standard
    `{"error":{code,message}}` envelope: status 413 → `request_too_large` /
    "request body too large"; every other rejection keeps its axum status
    (400/415/422) with `invalid_request` / "invalid request body". Messages are
    static — nothing echoes the body.
  - Tests: oversized body → 413 with envelope; malformed JSON → envelope (not
    plain text); wrong content-type → envelope.
- **H2 — boot-reject `jwks_ttl_secs = 0` with the redis backend** (`config.rs`):
  `IamConfig::validate` errors when `jwks_cache.backend == Redis &&
  jwks_ttl_secs == 0` (Redis `SET EX 0` is a command error → every JWKS fetch
  becomes `Unavailable` at runtime; failing at boot is strictly better). The
  memory backend keeps allowing `0` (it just means "always refetch", throttled by
  the cooldown). Jail test added.
- **H3 — bare `WWW-Authenticate: Bearer` on missing credentials** (RFC 6750 §3.1;
  `http/auth_middleware.rs`): when the `Authorization` header is entirely absent,
  the 401 carries a bare `Bearer` challenge (no `error` attribute — the client
  simply hasn't authenticated). A header that is present but unusable (wrong
  scheme, no space, empty credential) or a rejected token keeps today's
  `Bearer error="invalid_token"`. The JSON body stays the identical
  `invalid_token` envelope in both cases — only the challenge header varies, so
  the API error contract is untouched. Implemented as a middleware-local branch
  (absent-header case builds its response directly); the `AuthnApiError` funnel
  and `TokenDefect` are NOT extended — the funnel's "every defect renders
  identically" invariant stays intact. gRPC is unchanged (no
  `WWW-Authenticate` concept in trailers-only responses; missing and invalid are
  both `Unauthenticated`, as today). Tests: no-header case asserts the bare
  challenge; existing invalid-token cases keep asserting the error challenge.

## 5. Decisions (judgment calls, flagged for GATE 1)

- **D1 — move `AuthContext` to `adapters::auth` now, not in M3.** The issue says
  "natural to do when M3 consumes `AuthContext`". We move it now anyway: C1
  creates the module regardless, both import sites are already being edited for
  `bearer_from_headers`, and the move is ~10 lines — doing it in M3 would just
  re-churn the same files. *Alternative (rejected): leave `AuthContext` in the
  HTTP adapter until M3 — saves nothing now, costs a second edit later.*
- **D2 — implement H3 rather than "consider and skip".** The distinction is
  observable, spec-correct per RFC 6750, and costs one branch in one function
  plus one test. Skipping would re-defer a third time.
- **D3 — `OidcAuthenticator::new` becomes fallible** (C4) rather than
  pre-validated-by-contract with a panic or silent skip. Honest signature, both
  call sites already return `Result`, and it mirrors the existing wiring-defect
  guard pattern in `AppState::new`.

## 6. Error handling

No new error variants anywhere. C4 reuses `AuthnError::Backend` for the
can't-happen wiring guard. H1 maps axum's own rejection statuses into the existing
envelope shape. H2 is a new `Err(String)` case in the existing `validate`. H3
reuses the existing 401 envelope.

## 7. Testing & verification

- Every behavioral item (T1–T5, H1–H3) carries its own tests, listed above.
- Cleanups C1/C2/C4/C5 are covered by the existing suites (bearer enforcement,
  tenancy round-trips, validator pipeline, `Issuer` tests) plus T3/T4.
- Full verification is the repo gate graph, not per-project tasks:
  `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking
  :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main
  --include-relations` (C6 specifically needs `:deny`).
- Baseline before any change: 181/181 tests green in the worktree.

## 8. Non-goals

- No `TokenDefect`/`AuthnError` surface changes; no proto changes; no new config
  keys (H1 reuses `max_token_bytes`; H2 validates existing keys).
- No introspect rate limiting, no metrics, no gRPC challenge semantics — not in
  the issue.
- No dedup of the `Issuer::parse` calls in `AppState::new`'s `jit_flags` wiring
  (correct as-is; C4 is scoped to the validator's per-request parse).
