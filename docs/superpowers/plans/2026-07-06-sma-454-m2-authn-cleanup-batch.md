# SMA-454 M2-Authn Cleanup Batch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land all 14 deferred items from the SMA-443 final review (5 test adds, 6 cleanups, 3 hardening items) in one PR, per the approved spec `docs/superpowers/specs/2026-07-06-sma-454-m2-authn-cleanup-batch-design.md`.

**Architecture:** All changes live in `rs/` — the `paigasus-iam` service crate (hexagonal: adapters/application), one dead-code fix in the `paigasus-iam-core` lib, and `rs/deny.toml`. No new dependencies, no proto/py/ts changes, no new `AuthnError`/`TokenDefect` variants.

**Tech Stack:** Rust (edition 2024, 1.95), axum 0.8, tonic, jsonwebtoken 10.4, sea-orm, figment, cargo-nextest, testcontainers (Docker required for integration tests).

## Global Constraints

- **Worktree:** ALL work happens in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch` on branch `feature/sma-454-m2-authn-cleanup-batch`. If you are a subagent, your cwd starts pinned to the MAIN checkout — your FIRST action must be `EnterWorktree {path: "/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch"}`, then verify `git branch --show-current` prints `feature/sma-454-m2-authn-cleanup-batch` before touching anything.
- **PATH:** prefix every shell that runs moon/cargo-nextest/uv/buf with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- **Cargo cwd:** the Rust workspace root is `rs/` — run cargo from `<worktree>/rs`.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- rustfmt `max_width = 200` — long single-line signatures are house style; run `cargo fmt` before every commit.
- Conventional commits with workspace scope (`test(rs): …`, `refactor(rs): …`, `feat(rs): …`, `chore(rs): …`). commitlint runs on commit-msg (ts/node_modules already installed in the worktree). NEVER put a bare `#NNN` reference in a commit body.
- Docker is available and required for the `paigasus-iam` integration tests (they hard-fail only in CI; locally they'd silently skip — Docker IS running here, so a skip means something is wrong).
- Tasks are ordered; later tasks assume earlier ones landed (Task 5/10/11 use Task 5's `send_raw_parts`; Task 7 uses Task 6's changed `make_authenticator`).

---

### Task 1: C5 + T4 — `Issuer::parse` dead fallback + interior-whitespace tests

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authn.rs:32` (host split) and its `mod tests`

**Interfaces:**
- Produces: no signature changes; `Issuer::parse` behavior identical.

- [ ] **Step 1: Add the interior-whitespace coverage test (passes immediately — this is a coverage add for an existing branch)**

In `rs/crates/libs/paigasus-iam-core/src/authn.rs`, add to the existing `mod tests` (after `issuer_rejects_non_https_fragments_and_garbage`):

```rust
    #[test]
    fn issuer_rejects_interior_whitespace() {
        // The whitespace check must catch spaces/tabs INSIDE the trimmed string — both in
        // the host and in the path — not just the surrounding padding `parse` trims away.
        for bad in ["https://idp.example .com", "https://idp.example.com/realms acme", "https://idp.example.com/realms\tacme"] {
            assert!(Issuer::parse(bad).is_err(), "expected {bad:?} rejected");
        }
    }
```

- [ ] **Step 2: Run it — expect PASS (branch already exists; this pins it)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch/rs
cargo nextest run -p paigasus-iam-core issuer
```
Expected: PASS, including `issuer_rejects_interior_whitespace`.

- [ ] **Step 3: Replace the dead fallback**

In `authn.rs`, `Issuer::parse`, replace:

```rust
        let host = rest.split('/').next().unwrap_or("");
```

with:

```rust
        let host = rest.split_once('/').map_or(rest, |(host, _)| host);
```

(`split('/').next()` can never return `None`, so `unwrap_or("")` was dead; `split_once` has both arms live: no `/` → the whole `rest` is the host.)

- [ ] **Step 4: Re-run the crate's tests**

```bash
cargo nextest run -p paigasus-iam-core
```
Expected: all PASS (both accept and reject issuer tests).

- [ ] **Step 5: Format + commit**

```bash
cargo fmt
cd .. && git add rs/crates/libs/paigasus-iam-core/src/authn.rs
git commit -m "refactor(rs): drop dead unwrap_or in Issuer::parse, cover interior-whitespace branch (SMA-454)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: C2 — `map_principal_row` helper in the Pg principal repository

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_repository.rs`

**Interfaces:**
- Produces: private `fn map_principal_row(pm: principal::Model) -> Result<Principal, RepositoryError>` — file-local only.

- [ ] **Step 1: Extract the helper and rewrite both finders**

In `pg_repository.rs`, add above the `#[async_trait] impl PrincipalRepository` block:

```rust
/// Maps a `principal` row to the domain `Principal` (PRN/kind/status parsing, each
/// failure wrapped as a backend error) — the shared half of `find_user`/`find_principal`.
fn map_principal_row(pm: principal::Model) -> Result<Principal, RepositoryError> {
    let prn = Prn::parse(&pm.prn).map_err(|e| RepositoryError::Backend(Box::new(std::io::Error::other(e.to_string()))))?;
    let pid = PrincipalId::from_prn(prn);
    let kind = PrincipalKind::parse(&pm.kind).ok_or_else(|| RepositoryError::Backend(Box::new(std::io::Error::other("bad kind"))))?;
    let status = PrincipalStatus::parse(&pm.status).ok_or_else(|| RepositoryError::Backend(Box::new(std::io::Error::other("bad status"))))?;
    Ok(Principal::new(pid, kind, status, pm.created_at, pm.updated_at))
}
```

Replace the bodies of `find_user` and `find_principal`:

```rust
    async fn find_user(&self, id: &PrincipalId) -> Result<Option<(Principal, User)>, RepositoryError> {
        let uuid = id.uuid();
        let Some(pm) = principal::Entity::find_by_id(uuid).one(&self.db).await.map_err(map_err)? else {
            return Ok(None);
        };
        let Some(um) = user::Entity::find_by_id(uuid).one(&self.db).await.map_err(map_err)? else {
            return Ok(None);
        };

        let principal = map_principal_row(pm)?;
        let email = Email::parse(&um.email).map_err(|e| RepositoryError::Backend(Box::new(std::io::Error::other(format!("{e}")))))?;
        let user = User::new(principal.id.clone(), email, um.display_name, um.locale, um.timezone, um.created_at, um.updated_at);
        Ok(Some((principal, user)))
    }

    async fn find_principal(&self, id: &PrincipalId) -> Result<Option<Principal>, RepositoryError> {
        let Some(pm) = principal::Entity::find_by_id(id.uuid()).one(&self.db).await.map_err(map_err)? else {
            return Ok(None);
        };
        map_principal_row(pm).map(Some)
    }
```

(`find_user` now takes the `PrincipalId` for `User::new` from `principal.id` — same value it previously rebuilt from the row.)

- [ ] **Step 2: Build + clippy the crate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch/rs
cargo clippy -p paigasus-iam --all-targets -- -D warnings
```
Expected: clean (in particular, no unused-import warning — all imports were already used).

- [ ] **Step 3: Run the integration tests that exercise both finders (Docker)**

```bash
cargo nextest run -p paigasus-iam -E 'binary(authn_identities) or binary(roundtrip) or binary(http_authn)'
```
Expected: all PASS (these suites drive `find_user`/`find_principal` through JIT provisioning and user round-trips).

- [ ] **Step 4: Format + commit**

```bash
cargo fmt
cd .. && git add rs/crates/services/paigasus-iam/src/adapters/persistence/pg_repository.rs
git commit -m "refactor(rs): extract map_principal_row in pg principal repository (SMA-454)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: C3 + H2 — config comment fix + boot-reject `jwks_ttl_secs = 0`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs` (comment at ~line 171, `validate()` at ~line 137, new Jail test)

**Interfaces:**
- Produces: `IamConfig::validate` gains one rejection case; signature unchanged.

- [ ] **Step 1: Write the failing Jail test**

In `config.rs` `mod tests`, add after `validate_accepts_redis_backend_with_a_url`:

```rust
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
```

- [ ] **Step 2: Run it — expect FAIL**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch/rs
cargo nextest run -p paigasus-iam validate_rejects_zero_jwks_ttl
```
Expected: FAIL — `validate()` currently accepts `0`.

- [ ] **Step 3: Add the validation + update the doc comment**

In `IamConfig::validate`, add immediately before the final `Ok(())`:

```rust
        // `jwks_ttl_secs = 0` is broken with EITHER backend: redis `SET EX 0` is a command
        // error (every JWKS put fails -> permanent Unavailable), and the memory cache
        // treats every entry as already expired (never fresh + refresh cooldown -> requests
        // inside the cooldown window fail Unavailable). Reject at boot instead.
        if self.authn.jwks_ttl_secs == 0 {
            return Err("authn.jwks_ttl_secs must be at least 1 (0 disables JWKS caching and breaks both cache backends)".to_string());
        }
```

Extend the `validate` doc comment's checklist sentence: after "and a `redis` JWKS cache backend has `redis_url` configured", append ", and `jwks_ttl_secs` is non-zero (a zero TTL breaks both cache backends)".

- [ ] **Step 4: Fix the stale `#[allow]` comment (C3)**

Replace (immediately above `mod tests`):

```rust
// `figment::Jail::expect_with` fixes its closure's `Err` type to `figment::Error`
// (~208B) — not something callers control, so the size lint is allowed here, scoped
// to this test module's two Jail tests, rather than reshaped away.
```

with:

```rust
// `figment::Jail::expect_with` fixes its closure's `Err` type to `figment::Error`
// (~208B) — not something callers control, so the size lint is allowed here, scoped
// to this test module's Jail-based tests, rather than reshaped away.
```

- [ ] **Step 5: Run the config tests — expect PASS**

```bash
cargo nextest run -p paigasus-iam config
```
Expected: all PASS, including `validate_rejects_zero_jwks_ttl`.

- [ ] **Step 6: Format + commit**

```bash
cargo fmt
cd .. && git add rs/crates/services/paigasus-iam/src/config.rs
git commit -m "feat(rs): boot-reject jwks_ttl_secs=0, fix stale allow comment (SMA-454)

A zero TTL breaks both JWKS cache backends (redis SET EX 0 errors; the
memory cache is never fresh, so cooldown windows return Unavailable) -
fail at boot instead of intermittently at runtime.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: C1 + T3(unit) — shared `bearer_from_headers` + `adapters::auth` home for `AuthContext`

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/auth.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/mod.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/auth_middleware.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs`

**Interfaces:**
- Produces: `crate::adapters::auth::AuthContext` (struct, fields `principal_id: PrincipalId`, `issuer: Issuer`, `subject: String`, `#[derive(Clone)]`) and `crate::adapters::auth::bearer_from_headers(headers: &axum::http::HeaderMap) -> Option<String>`. Tasks 5 and 11 rely on the extraction behavior being byte-identical to today's.

- [ ] **Step 1: Create `adapters/auth.rs` with the shared function, the moved `AuthContext`, and unit tests**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Transport-agnostic authentication plumbing shared by the HTTP bearer middleware and the
//! gRPC enforcement layer: bearer extraction from request headers, and the [`AuthContext`]
//! both surfaces attach for downstream handlers. Lives outside `adapters::http` so the gRPC
//! layer no longer reaches into a sibling transport adapter for it (SMA-454 C1); M3
//! (authorization) and M5 (audit) will consume both from here.

use axum::http::{HeaderMap, header};
use paigasus_iam_core::{Issuer, PrincipalId};

/// The authenticated request context the enforcement layers attach on success (D13: the
/// hot path resolves the principal only — no membership fetch; that stays in `Introspect`).
/// M2 handlers don't read it yet; M3 (authorization) and M5 (audit) will. The HTTP
/// middleware and the gRPC layer attach this exact same shape, so the field set is
/// deliberately fixed here.
#[derive(Clone)]
pub struct AuthContext {
    pub principal_id: PrincipalId,
    pub issuer: Issuer,
    pub subject: String,
}

/// Extracts the bearer token from the `Authorization` header — the sole accepted
/// credential source on both surfaces (no cookies, no query parameters). Returns `None`
/// for an absent header, a non-UTF-8 value, a fused or non-`Bearer` scheme, or an empty
/// credential. The scheme match is ASCII-case-insensitive per RFC 7235 §2.1.
pub fn bearer_from_headers(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(value: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = value {
            headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn accepts_bearer_scheme_case_insensitively() {
        // RFC 7235 §2.1: the auth-scheme token is case-insensitive.
        assert_eq!(bearer_from_headers(&headers(Some("Bearer abc"))).as_deref(), Some("abc"));
        assert_eq!(bearer_from_headers(&headers(Some("bearer abc"))).as_deref(), Some("abc"));
        assert_eq!(bearer_from_headers(&headers(Some("BEARER abc"))).as_deref(), Some("abc"));
    }

    #[test]
    fn rejects_absent_fused_foreign_and_empty() {
        assert_eq!(bearer_from_headers(&headers(None)), None, "absent header");
        assert_eq!(bearer_from_headers(&headers(Some("Bearertoken"))), None, "scheme fused with credential (no space)");
        assert_eq!(bearer_from_headers(&headers(Some("Basic dXNlcjpwdw=="))), None, "non-Bearer scheme");
        assert_eq!(bearer_from_headers(&headers(Some("Bearer "))), None, "empty credential");
        assert_eq!(bearer_from_headers(&headers(Some("Bearer \t "))), None, "whitespace-only credential");
    }
}
```

- [ ] **Step 2: Register the module**

In `rs/crates/services/paigasus-iam/src/adapters/mod.rs`, add `pub mod auth;` as the first module line (alphabetical):

```rust
pub mod auth;
pub mod clock;
pub mod grpc;
pub mod http;
pub mod id;
pub mod oidc;
pub mod persistence;
```

- [ ] **Step 3: Run the new unit tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch/rs
cargo nextest run -p paigasus-iam adapters::auth
```
Expected: 2 PASS.

- [ ] **Step 4: Rewire the HTTP middleware**

In `http/auth_middleware.rs`:
1. Delete the whole `AuthContext` struct + its doc comment (lines 23–32) and the whole `bearer_token` function + its doc comment (lines 58–72).
2. Replace the import block's `use super::AppState;` region so imports read:

```rust
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use paigasus_iam_core::{AuthnError, TokenDefect};

use super::AppState;
use super::authn::AuthnApiError;
use crate::adapters::auth::{AuthContext, bearer_from_headers};
use crate::application::authenticate_token::Provisioning;
```

(`axum::http::header`, `Issuer`, and `PrincipalId` are no longer used here — remove them.)
3. In `require_bearer`, replace `let Some(token) = bearer_token(&request) else {` with `let Some(token) = bearer_from_headers(request.headers()) else {`.
4. In the module doc comment (line 6), the phrase "inserts an [`AuthContext`] request extension" keeps working as a doc-link only if the item is in scope — it is (imported). Leave the comment text as is.

- [ ] **Step 5: Rewire the gRPC layer**

In `grpc/authn.rs`:
1. Delete the whole local `bearer_token` function + its doc comment (lines 148–163).
2. Replace `use crate::adapters::http::auth_middleware::AuthContext;` with `use crate::adapters::auth::{AuthContext, bearer_from_headers};`.
3. The call site (`let Some(token) = bearer_token(req.headers()) else {`) becomes `let Some(token) = bearer_from_headers(req.headers()) else {` — `tonic::codegen::http` re-exports the same `http 1.4.2` crate axum wraps, so the `&HeaderMap` type unifies.

- [ ] **Step 6: Verify nothing else imported the old paths**

```bash
grep -rn "auth_middleware::AuthContext\|fn bearer_token" src/ ../services 2>/dev/null; grep -rn "auth_middleware::AuthContext" /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch/rs/crates
```
Expected: no matches.

- [ ] **Step 7: Clippy + the enforcement integration suites (Docker)**

```bash
cargo clippy -p paigasus-iam --all-targets -- -D warnings
cargo nextest run -p paigasus-iam -E 'binary(http_authn) or binary(grpc_authn)'
```
Expected: clippy clean; all integration tests PASS (extraction behavior unchanged).

- [ ] **Step 8: Format + commit**

```bash
cargo fmt
cd .. && git add rs/crates/services/paigasus-iam/src/adapters/
git commit -m "refactor(rs): shared bearer_from_headers and adapters::auth home for AuthContext (SMA-454)

The HTTP middleware and gRPC layer held byte-identical 8-line bearer
extractors, and gRPC imported AuthContext from the HTTP adapter. Both
now live in a transport-agnostic adapters::auth module (M3 consumes
AuthContext from here).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: T3(integration) — raw-request harness helper + bearer negative cases

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs` (add `send_raw_parts`, reimplement `send_raw` on it)
- Modify: `rs/crates/services/paigasus-iam/tests/http_authn.rs` (2 new tests)

**Interfaces:**
- Produces: `pub async fn send_raw_parts(app: &Router, method: &str, uri: &str, authorization: Option<&str>, content_type: Option<&str>, body: Option<Vec<u8>>) -> Response` in `tests/support/mod.rs`. Tasks 10 and 11 call it with exactly this signature.
- Consumes: Task 4's extraction (behavior-identical, so a valid lowercase `bearer` header already authenticates).

- [ ] **Step 1: Add `send_raw_parts` and rebase `send_raw` on it**

In `tests/support/mod.rs`, replace the existing `send_raw` function with:

```rust
/// Lowest-level request driver: full control over the `Authorization` value, the
/// `content-type`, and the raw body bytes — for tests that need a non-`Bearer {token}`
/// credential shape (scheme casing, fused scheme, foreign scheme) or a deliberately
/// broken/oversized body. Everything else goes through `send_raw`/`send`.
#[allow(dead_code)]
pub async fn send_raw_parts(app: &Router, method: &str, uri: &str, authorization: Option<&str>, content_type: Option<&str>, body: Option<Vec<u8>>) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(authorization) = authorization {
        builder = builder.header("authorization", authorization);
    }
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    let request = builder.body(body.map_or_else(Body::empty, Body::from)).unwrap();
    app.clone().oneshot(request).await.unwrap()
}

/// Drives one request through the router and returns the raw response — for tests that
/// assert on headers (e.g. `WWW-Authenticate`). `token` sets `Authorization: Bearer …`.
#[allow(dead_code)]
pub async fn send_raw(app: &Router, method: &str, uri: &str, body: Option<Value>, token: Option<&str>) -> Response {
    let authorization = token.map(|token| format!("Bearer {token}"));
    let (content_type, body) = match body {
        Some(b) => (Some("application/json"), Some(serde_json::to_vec(&b).unwrap())),
        None => (None, None),
    };
    send_raw_parts(app, method, uri, authorization.as_deref(), content_type, body).await
}
```

- [ ] **Step 2: Add the two integration cases**

In `tests/http_authn.rs`, first extend the support import to include the new helper:

```rust
use support::{send, send_raw, send_raw_parts, start_mock_idp, test_config, test_config_with};
```

Then add after `protected_route_with_invalid_token_is_401_with_www_authenticate`:

```rust
#[tokio::test]
async fn fused_bearer_scheme_is_401() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, idp) = support::app(db).await;

    // A perfectly VALID token, but the scheme is fused with the credential ("Bearer<jwt>",
    // no space): header parsing requires `<scheme> <credential>`, so this must 401 without
    // the token ever reaching the validator.
    let token = idp.bearer("fused-scheme", Some("fused@example.com"), "paigasus", 3600);
    let response = send_raw_parts(&app, "GET", "/v1/organizations", Some(&format!("Bearer{token}")), None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "invalid_token");
}

#[tokio::test]
async fn lowercase_bearer_scheme_is_accepted() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, idp) = support::app(db).await;

    // RFC 7235 §2.1: the auth-scheme is case-insensitive, so `bearer <jwt>` must
    // authenticate exactly like `Bearer <jwt>` (and JIT-provision on the way in).
    let token = idp.bearer("lowercase-bearer", Some("lower@example.com"), "paigasus", 3600);
    let response = send_raw_parts(&app, "GET", "/v1/organizations", Some(&format!("bearer {token}")), None, None).await;
    assert_eq!(response.status(), StatusCode::OK, "a lowercase bearer scheme must be accepted");
}
```

- [ ] **Step 3: Run the suite (Docker)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch/rs
cargo nextest run -p paigasus-iam -E 'binary(http_authn)'
```
Expected: all PASS, including the 2 new tests (they pin EXISTING behavior — both should pass on first run; if `lowercase_bearer_scheme_is_accepted` fails, STOP: the extraction changed behavior, which Task 4 must not do).

- [ ] **Step 4: Verify the other harness consumers still compile and pass (send_raw rebase touches them)**

```bash
cargo nextest run -p paigasus-iam -E 'binary(http_tenancy) or binary(http_memberships)'
```
Expected: all PASS.

- [ ] **Step 5: Format + commit**

```bash
cargo fmt
cd .. && git add rs/crates/services/paigasus-iam/tests/
git commit -m "test(rs): bearer-parsing negative integration tests + raw-request harness (SMA-454)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: C4 — parse configured issuers once in `OidcAuthenticator::new`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` (both constructor call sites)

**Interfaces:**
- Produces: `OidcAuthenticator::new(...) -> Result<Self, AuthnError>` (was infallible). Task 7's tests use the updated `make_authenticator` unchanged from the outside.

- [ ] **Step 1: Write the failing constructor test**

In `validator.rs` `mod tests`, add after `make_authenticator`/`issuer_config` helpers:

```rust
    #[test]
    fn unparseable_configured_issuer_fails_construction() {
        // `IamConfig::validate` rejects this at boot, so an Err here is a wiring-defect
        // guard — but the constructor must still refuse rather than defer to per-request
        // parse failures (which this change removes).
        let (_encoding_key, jwk, _kid) = es256_keypair();
        let provider = JwksProvider::new(StubFetcher::new(jwk), InMemoryJwksCache::new(), SystemClock, Duration::from_secs(3600), Duration::from_secs(30));
        let err = OidcAuthenticator::new(vec![issuer_config("http://not-https.example.com", &["aud"])], provider, 60, 16_384).unwrap_err();
        assert!(matches!(err, AuthnError::Backend(_)));
    }
```

- [ ] **Step 2: Run it — expect COMPILE FAIL (`new` returns `Self`, `.unwrap_err()` doesn't exist)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch/rs
cargo nextest run -p paigasus-iam unparseable_configured_issuer
```
Expected: compile error on `unwrap_err`.

- [ ] **Step 3: Make the constructor parse-once and fallible**

In `validator.rs`:

1. Add below the `ALLOWED_ALGORITHMS` const:

```rust
/// One configured issuer, parsed once at construction — replacing the per-request
/// `Issuer::parse` the request path used to run after every issuer match.
/// `jit_provisioning` is deliberately absent: the validator never reads it.
struct ConfiguredIssuer {
    issuer: Issuer,
    audiences: Vec<String>,
}
```

2. Change the struct field: `issuers: Vec<IssuerConfig>,` → `issuers: Vec<ConfiguredIssuer>,`.

3. Replace `new` and `find_issuer_config`:

```rust
    /// Parses every configured issuer once, up front. Fails when one doesn't parse —
    /// `IamConfig::validate` already rejects that at boot, so an `Err` here is a wiring
    /// defect, mirroring the `redis_url` guard in `AppState::new`.
    pub fn new(issuers: Vec<IssuerConfig>, provider: JwksProvider<F, K, C>, leeway_secs: u64, max_token_bytes: usize) -> Result<Self, AuthnError> {
        let issuers = issuers
            .into_iter()
            .map(|cfg| {
                let issuer = Issuer::parse(&cfg.issuer).map_err(|e| AuthnError::Backend(e.to_string().into()))?;
                Ok(ConfiguredIssuer { issuer, audiences: cfg.audiences })
            })
            .collect::<Result<Vec<_>, AuthnError>>()?;
        Ok(Self {
            issuers,
            provider,
            leeway_secs,
            max_token_bytes,
        })
    }

    /// Exact string match against the configured issuer list (spec §3.1's "no
    /// normalization" rule applies here too — compared byte-for-byte, same as
    /// `Issuer::parse`'s own equality semantics). Matching against the PARSED (trimmed)
    /// form is equivalent to the raw config string because `IamConfig::validate` rejects
    /// padded issuers before any authenticator is constructed.
    fn find_issuer_config(&self, iss: &str) -> Option<&ConfiguredIssuer> {
        self.issuers.iter().find(|cfg| cfg.issuer.as_str() == iss)
    }
```

4. In `authenticate`, replace the step-3 pair of lines:

```rust
        let issuer_config = self.find_issuer_config(&unverified_iss).ok_or_else(|| invalid(TokenDefect::IssuerNotConfigured))?;
        let issuer = Issuer::parse(&issuer_config.issuer).map_err(|_| invalid(TokenDefect::IssuerNotConfigured))?;
```

with:

```rust
        let issuer_config = self.find_issuer_config(&unverified_iss).ok_or_else(|| invalid(TokenDefect::IssuerNotConfigured))?;
        let issuer = issuer_config.issuer.clone();
```

(everything downstream — `key_for(&issuer, ..)`, `set_issuer(&[issuer.as_str()])`, `set_audience(&issuer_config.audiences)`, `ValidatedClaims { issuer, .. }` — compiles unchanged.)

5. In `mod tests`, `make_authenticator` gains `.expect(..)`:

```rust
    fn make_authenticator(fetcher: StubFetcher, issuers: Vec<IssuerConfig>, leeway_secs: u64, max_token_bytes: usize) -> OidcAuthenticator<StubFetcher, InMemoryJwksCache, SystemClock> {
        let provider = JwksProvider::new(fetcher, InMemoryJwksCache::new(), SystemClock, Duration::from_secs(3600), Duration::from_secs(30));
        OidcAuthenticator::new(issuers, provider, leeway_secs, max_token_bytes).expect("test issuers parse")
    }
```

- [ ] **Step 4: Update both composition-root call sites**

In `http/mod.rs` `AppState::new`, wrap both constructor calls with `?`:

```rust
            JwksCacheBackend::Memory => WiredAuthenticator::Memory(Arc::new(OidcAuthenticator::new(
                authn_cfg.issuers.clone(),
                JwksProvider::new(fetcher, InMemoryJwksCache::new(), SystemClock, ttl, cooldown),
                authn_cfg.leeway_secs,
                authn_cfg.max_token_bytes,
            )?)),
```

and in the `Redis` arm identically:

```rust
                WiredAuthenticator::Redis(Arc::new(OidcAuthenticator::new(
                    authn_cfg.issuers.clone(),
                    JwksProvider::new(fetcher, cache, SystemClock, ttl, cooldown),
                    authn_cfg.leeway_secs,
                    authn_cfg.max_token_bytes,
                )?))
```

- [ ] **Step 5: Full crate check + tests**

```bash
cargo clippy -p paigasus-iam --all-targets -- -D warnings
cargo nextest run -p paigasus-iam validator
cargo nextest run -p paigasus-iam -E 'binary(http_authn)'
```
Expected: clippy clean; all validator unit tests PASS (including the new constructor test); http_authn PASS.

- [ ] **Step 6: Format + commit**

```bash
cargo fmt
cd .. && git add rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
git commit -m "refactor(rs): parse configured issuers once in OidcAuthenticator::new (SMA-454)

Drops the per-request Issuer::parse of the matched config entry; the
constructor is now fallible (an unparseable issuer is a wiring defect -
IamConfig::validate already rejects it at boot).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: T1 — validator kty/alg-mismatch + empty-`aud` branch tests

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` (tests only)

**Interfaces:**
- Consumes: Task 6's `make_authenticator` (unchanged call shape).

- [ ] **Step 1: Generalize the `sign` test helper to arbitrary claims**

In `validator.rs` `mod tests`, change the signature of `sign` (body unchanged):

```rust
    fn sign<C: serde::Serialize>(encoding_key: &EncodingKey, kid: Option<&str>, claims: &C) -> String {
        let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
        header.kid = kid.map(str::to_string);
        jsonwebtoken::encode(&header, claims, encoding_key).expect("signing a test token")
    }
```

- [ ] **Step 2: Add the two branch tests (both must pass immediately — they pin existing behavior on uncovered branches)**

Add after `wrong_signing_key_under_the_same_kid_is_bad_signature`:

```rust
    #[tokio::test]
    async fn jwk_kty_mismatching_header_alg_is_unsupported_alg() {
        // The stub JWKS serves an EC P-256 key under `kid`, but the crafted header claims
        // RS256 under that SAME kid: allowlist passes (RS256 is allowed), kid lookup
        // succeeds, and the kty/alg consistency check must reject — an EC key can never
        // have produced an RS256 signature — BEFORE signature verification (the
        // placeholder signature is never inspected).
        let (_encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config("https://idp.example.com", &["aud"])], 60, 16_384);

        let token = manual_token(&format!(r#"{{"alg":"RS256","typ":"JWT","kid":"{kid}"}}"#));
        let err = authenticator.authenticate(&token).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::UnsupportedAlg)));
    }

    #[tokio::test]
    async fn empty_audience_array_is_audience_mismatch() {
        // RFC 7519 §4.1.3 allows `aud` as an array; an EMPTY array can never intersect the
        // configured audience set, so validation must reject it as an audience mismatch
        // (verified against jsonwebtoken 10.4's is_subset: empty intersection -> InvalidAudience).
        #[derive(Serialize)]
        struct EmptyAudClaims {
            iss: String,
            sub: String,
            aud: Vec<String>,
            exp: i64,
        }
        let (encoding_key, jwk, kid) = es256_keypair();
        let issuer = "https://idp.example.com";
        let now = Utc::now().timestamp();
        let claims = EmptyAudClaims {
            iss: issuer.to_string(),
            sub: "sub-1".to_string(),
            aud: vec![],
            exp: now + 3600,
        };
        let token = sign(&encoding_key, Some(&kid), &claims);

        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(issuer, &["expected-aud"])], 60, 16_384);
        let err = authenticator.authenticate(&token).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)));
    }
```

- [ ] **Step 3: Run the validator tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch/rs
cargo nextest run -p paigasus-iam validator
```
Expected: all PASS including the 2 new tests. If `empty_audience_array_is_audience_mismatch` fails with `Malformed` instead, STOP and report — the defect mapping differs from the spec's verified expectation.

- [ ] **Step 4: Format + commit**

```bash
cargo fmt
cd .. && git add rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs
git commit -m "test(rs): validator kty/alg-mismatch and empty-aud branch tests (SMA-454)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: T2 — use-case JIT tests (display-name fallback, bad email, profile passthrough)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` (tests only)

**Interfaces:**
- Produces: test-local `claims_with_profile(issuer, subject, email, name, locale, zoneinfo) -> ValidatedClaims`; `claims(..)` reimplemented on top of it.

- [ ] **Step 1: Generalize the claims helper**

In `authenticate_token.rs` `mod tests`, replace the existing `claims` function with:

```rust
    fn claims_with_profile(issuer: &str, subject: &str, email: Option<&str>, name: Option<&str>, locale: Option<&str>, zoneinfo: Option<&str>) -> ValidatedClaims {
        ValidatedClaims {
            issuer: Issuer::parse(issuer).unwrap(),
            subject: subject.to_string(),
            audiences: vec!["aud".to_string()],
            expires_at: Utc.timestamp_opt(2_000_000_000, 0).unwrap(),
            email: email.map(str::to_string),
            name: name.map(str::to_string),
            locale: locale.map(str::to_string),
            zoneinfo: zoneinfo.map(str::to_string),
        }
    }

    fn claims(issuer: &str, subject: &str, email: Option<&str>, name: Option<&str>) -> ValidatedClaims {
        claims_with_profile(issuer, subject, email, name, None, None)
    }
```

- [ ] **Step 2: Add the three tests**

Add after `missing_email_fails_provisioning`:

```rust
    #[tokio::test]
    async fn missing_name_falls_back_to_email_local_part() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-lp", Some("carol.smith@example.com"), None)),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer, true)]),
        );

        let resolved = uc.resolve("token", Provisioning::Enabled).await.unwrap();

        let (_, user) = store.principals.lock().unwrap().get(&resolved.principal_id.uuid()).cloned().unwrap();
        assert_eq!(user.display_name, "carol.smith", "display_name must fall back to the email local part when the name claim is absent");
    }

    #[tokio::test]
    async fn unparseable_email_fails_provisioning_as_missing_email() {
        // An email claim that is PRESENT but unparseable (no '@') is the same defect as an
        // absent one: MissingEmail, and nothing is provisioned.
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-bad-email", Some("not-an-email"), Some("Broken"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer, true)]),
        );

        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)));
        assert!(store.principals.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn locale_and_zoneinfo_pass_through_to_the_provisioned_user() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims_with_profile("https://idp.example.com", "sub-loc", Some("dora@example.com"), Some("Dora"), Some("de-DE"), Some("Europe/Berlin"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer, true)]),
        );

        let resolved = uc.resolve("token", Provisioning::Enabled).await.unwrap();

        let (_, user) = store.principals.lock().unwrap().get(&resolved.principal_id.uuid()).cloned().unwrap();
        assert_eq!(user.locale.as_deref(), Some("de-DE"));
        assert_eq!(user.timezone.as_deref(), Some("Europe/Berlin"), "the zoneinfo claim lands on User.timezone untouched");
    }
```

- [ ] **Step 3: Run the use-case tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch/rs
cargo nextest run -p paigasus-iam authenticate_token
```
Expected: all PASS including the 3 new tests (coverage adds on existing behavior).

- [ ] **Step 4: Format + commit**

```bash
cargo fmt
cd .. && git add rs/crates/services/paigasus-iam/src/application/authenticate_token.rs
git commit -m "test(rs): jit display-name fallback, unparseable-email, profile passthrough (SMA-454)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 9: T5 — Redis round-trip asserts full `JwkSet` equality

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/redis_jwks_cache.rs:71-74`

- [ ] **Step 1: Replace the piecemeal asserts**

In `put_then_get_round_trips`, replace:

```rust
    assert_eq!(got.jwks_uri, jwks.jwks_uri);
    assert_eq!(got.fetched_at, jwks.fetched_at);
    assert_eq!(got.jwks.keys.len(), jwks.jwks.keys.len());
    assert_eq!(got.jwks.keys[0].common.key_id, jwks.jwks.keys[0].common.key_id);
```

with:

```rust
    assert_eq!(got.jwks, jwks.jwks, "the full JwkSet must survive the round-trip byte-for-byte");
    assert_eq!(got.jwks_uri, jwks.jwks_uri);
    assert_eq!(got.fetched_at, jwks.fetched_at);
```

(`JwkSet` derives `PartialEq, Eq` in the locked jsonwebtoken 10.4.0.)

- [ ] **Step 2: Run the redis suite (Docker)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch/rs
cargo nextest run -p paigasus-iam -E 'binary(redis_jwks_cache)'
```
Expected: 3 PASS.

- [ ] **Step 3: Commit**

```bash
cargo fmt
cd .. && git add rs/crates/services/paigasus-iam/tests/redis_jwks_cache.rs
git commit -m "test(rs): assert full JwkSet equality in redis jwks round-trip (SMA-454)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 10: H1 — introspect body limit + enveloped JSON rejections

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` (AppState field + router plumbing)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs` (EnvelopeJson extractor, router signature)
- Modify: `rs/crates/services/paigasus-iam/tests/http_authn.rs` (3 new tests)

**Interfaces:**
- Consumes: `send_raw_parts` from Task 5 (exact signature `(app, method, uri, authorization, content_type, body)`).
- Produces: `AppState.introspect_body_limit: usize` (pub field); `authn::router(body_limit: usize) -> Router<AppState>` (was zero-arg). New public error codes on introspect: `request_too_large` (413), `invalid_request` (other body rejections).

- [ ] **Step 1: Write the three failing integration tests**

In `tests/http_authn.rs`, add after `introspect_oversized_token_is_401_invalid_token`:

```rust
#[tokio::test]
async fn introspect_oversized_body_is_413_request_too_large() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;

    // Past max_token_bytes (16384) + the 1024-byte envelope headroom: rejected by the
    // route-level body limit BEFORE JSON parsing, in the standard envelope (H1). The
    // 401 band just above max_token_bytes stays covered by
    // introspect_oversized_token_is_401_invalid_token — the two-tier behavior is by design.
    let huge = format!(r#"{{"token":"{}"}}"#, "a".repeat(20_000));
    let response = send_raw_parts(&app, "POST", "/v1/authn/introspect", None, Some("application/json"), Some(huge.into_bytes())).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "request_too_large");
    assert_eq!(body["error"]["message"], "request body too large");
}

#[tokio::test]
async fn introspect_malformed_json_is_enveloped() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;

    // Broken JSON must render the same {"error":{code,message}} envelope as every other
    // authn error — not axum's default plain-text rejection (H1).
    let response = send_raw_parts(&app, "POST", "/v1/authn/introspect", None, Some("application/json"), Some(b"{not json".to_vec())).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
    assert_eq!(body["error"]["message"], "invalid request body");
}

#[tokio::test]
async fn introspect_wrong_content_type_is_enveloped() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;

    let response = send_raw_parts(&app, "POST", "/v1/authn/introspect", None, Some("text/plain"), Some(br#"{"token":"x"}"#.to_vec())).await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
}
```

- [ ] **Step 2: Run them — expect FAIL**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch/rs
cargo nextest run -p paigasus-iam -E 'binary(http_authn)' introspect_
```
Expected: the 3 new tests FAIL (today: 413/400/415 with axum's plain-text bodies — the `serde_json::from_slice` unwrap panics or the code assert fails). Pre-existing introspect tests still PASS.

- [ ] **Step 3: Implement the extractor + route-level limit**

In `http/authn.rs`:

1. Extend imports:

```rust
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, FromRequest, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
```

(replace the current `use axum::{Json, Router, extract::State};` and `use axum::http::…` lines with the above set.)

2. Add below the `AuthnApiError` impls:

```rust
/// `Json<T>` with the authn error envelope on rejection (spec H1): axum's default
/// plain-text rejections (malformed JSON, wrong content-type, oversized body) become the
/// same `{"error":{code,message}}` shape every other authn response uses. The status is
/// the rejection's own; messages are static — nothing ever echoes the request body.
struct EnvelopeJson<T>(T);

impl<S, T> FromRequest<S> for EnvelopeJson<T>
where
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(EnvelopeJson(value)),
            Err(rejection) => {
                let (code, message) = if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    ("request_too_large", "request body too large")
                } else {
                    ("invalid_request", "invalid request body")
                };
                Err((rejection.status(), Json(json!({ "error": { "code": code, "message": message } }))).into_response())
            }
        }
    }
}
```

3. Change `router` to take and apply the limit:

```rust
/// The introspect sub-router. Merged alongside (NOT inside) the tenancy `/v1` sub-router
/// in `super::router` so Task 11's bearer-enforcement layer, which wraps the tenancy
/// sub-router only, never covers this route (middleware-exempt, spec §7.4). `body_limit`
/// (from `AppState::introspect_body_limit`) caps the request body at
/// `max_token_bytes` + envelope headroom — the only legitimate payload is
/// `{"token":"<= max_token_bytes>"}`, so anything larger is rejected before JSON parsing
/// (H1; deliberately far below axum's 2 MB default).
pub fn router(body_limit: usize) -> Router<AppState> {
    Router::new().route("/v1/authn/introspect", post(introspect)).route_layer(DefaultBodyLimit::max(body_limit))
}
```

4. Change the handler's extractor:

```rust
async fn introspect(State(state): State<AppState>, EnvelopeJson(body): EnvelopeJson<IntrospectBody>) -> Result<Json<IntrospectResponseDto>, AuthnApiError> {
    let ctx = state.authn.introspect(&body.token).await?;
    Ok(Json(ctx.into()))
}
```

In `http/mod.rs`:

5. Add above `AppState`:

```rust
/// Headroom over `max_token_bytes` for the introspect JSON envelope (`{"token":"…"}` —
/// braces, quotes, key, and any insignificant whitespace): a request larger than
/// `max_token_bytes` + this can never carry a valid token, so the route body limit
/// rejects it before JSON parsing (spec H1).
const INTROSPECT_BODY_OVERHEAD_BYTES: usize = 1024;
```

6. Add the field to `AppState`:

```rust
#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub orgs: OrgSvc,
    pub teams: TeamSvc,
    pub projects: ProjectSvc,
    pub memberships: MembershipSvc,
    pub users: UserSvc,
    pub authn: AuthnSvc,
    /// Route-level body cap for `POST /v1/authn/introspect` (H1): `max_token_bytes` +
    /// [`INTROSPECT_BODY_OVERHEAD_BYTES`], computed once at wiring time.
    pub introspect_body_limit: usize,
}
```

7. In `AppState::new`, set it in the final struct literal:

```rust
        Ok(AppState {
            db,
            orgs,
            teams,
            projects,
            memberships,
            users,
            authn,
            introspect_body_limit: cfg.authn.max_token_bytes + INTROSPECT_BODY_OVERHEAD_BYTES,
        })
```

8. In `router(state)`, thread the limit (the field is read before `state` moves):

```rust
    let authn_api = authn::router(state.introspect_body_limit).with_state(state);
```

- [ ] **Step 4: Run the suite — expect PASS**

```bash
cargo clippy -p paigasus-iam --all-targets -- -D warnings
cargo nextest run -p paigasus-iam -E 'binary(http_authn)'
```
Expected: clippy clean; ALL http_authn tests PASS — the 3 new ones AND `introspect_oversized_token_is_401_invalid_token` (16 385-byte token ≈ 16 397-byte body < 17 408 limit → still reaches the validator's 401 band).

- [ ] **Step 5: Format + commit**

```bash
cargo fmt
cd .. && git add rs/crates/services/paigasus-iam/src/adapters/http/ rs/crates/services/paigasus-iam/tests/http_authn.rs
git commit -m "feat(rs): introspect route body limit + enveloped JSON rejections (SMA-454)

POST /v1/authn/introspect now caps its body at max_token_bytes + 1KiB
envelope headroom (route-level DefaultBodyLimit), and a failed JSON
extraction renders the standard error envelope (request_too_large /
invalid_request) instead of axum's plain-text rejection.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 11: H3 — bare `WWW-Authenticate: Bearer` on absent credentials

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/auth_middleware.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/http_authn.rs` (update 1 test, add 1 test)

**Interfaces:**
- Consumes: `bearer_from_headers` (Task 4), `send_raw_parts` (Task 5).
- Produces: HTTP-only behavior change — 401 challenge is bare `Bearer` iff the `Authorization` header is entirely absent. Body and status unchanged; gRPC unchanged.

- [ ] **Step 1: Update the existing no-token test to the new contract (failing first)**

In `tests/http_authn.rs`, replace the body of `protected_route_without_token_is_401`:

```rust
#[tokio::test]
async fn protected_route_without_token_is_401() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;

    // No `Authorization` header at all on a protected tenancy route: 401 with the same
    // `invalid_token` body as any rejected credential, but a BARE `Bearer` challenge —
    // RFC 6750 §3.1 says a request with no authentication information gets a challenge
    // without an error attribute (H3). Only the header distinguishes the cases.
    let response = send_raw(&app, "POST", "/v1/organizations", Some(json!({ "slug": "acme", "name": "Acme" })), None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let challenge = response.headers().get("www-authenticate").expect("WWW-Authenticate header").to_str().unwrap();
    assert_eq!(challenge, "Bearer");
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "invalid_token");
}
```

And add a companion test directly after it:

```rust
#[tokio::test]
async fn present_but_malformed_authorization_keeps_error_challenge() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;

    // A PRESENT-but-unusable header (foreign scheme) is NOT "missing credentials": the
    // client did attempt authentication, so the challenge keeps the error attribute
    // (H3 differentiates only the fully-absent case).
    let response = send_raw_parts(&app, "GET", "/v1/organizations", Some("Basic dXNlcjpwdw=="), None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let challenge = response.headers().get("www-authenticate").expect("WWW-Authenticate header").to_str().unwrap();
    assert_eq!(challenge, "Bearer error=\"invalid_token\"");
}
```

- [ ] **Step 2: Run — expect exactly one FAIL**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch/rs
cargo nextest run -p paigasus-iam -E 'binary(http_authn)' protected_route_without_token
cargo nextest run -p paigasus-iam -E 'binary(http_authn)' present_but_malformed
```
Expected: `protected_route_without_token_is_401` FAILS (challenge still carries `error="invalid_token"`); `present_but_malformed_authorization_keeps_error_challenge` PASSES (pins current behavior).

- [ ] **Step 3: Implement the middleware branch**

In `http/auth_middleware.rs`:

1. The COMPLETE import block becomes (re-adding `axum::http` for the header check/insert; the `super::`/`crate::` lines are unchanged from Task 4 and must stay):

```rust
use axum::extract::{Request, State};
use axum::http::{HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use paigasus_iam_core::{AuthnError, TokenDefect};

use super::AppState;
use super::authn::AuthnApiError;
use crate::adapters::auth::{AuthContext, bearer_from_headers};
use crate::application::authenticate_token::Provisioning;
```

2. Replace the rejection branch of `require_bearer`:

```rust
pub async fn require_bearer(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let header_present = request.headers().contains_key(header::AUTHORIZATION);
    let Some(token) = bearer_from_headers(request.headers()) else {
        // Absent or unusable credentials get the SAME 401 status and `invalid_token` body
        // (the error contract stays uniform, D12); only the challenge header distinguishes
        // a client that sent NO credentials at all (bare `Bearer`, RFC 6750 §3.1 — no error
        // attribute when the request lacks authentication information) from one whose
        // header or token was present but rejected (`Bearer error="invalid_token"`).
        let mut response = AuthnApiError(AuthnError::InvalidToken(TokenDefect::Malformed)).into_response();
        if !header_present {
            response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        return response;
    };

    match state.authn.resolve(&token, Provisioning::Enabled).await {
        Ok(principal) => {
            request.extensions_mut().insert(AuthContext {
                principal_id: principal.principal_id,
                issuer: principal.issuer,
                subject: principal.subject,
            });
            next.run(request).await
        }
        Err(err) => AuthnApiError(err).into_response(),
    }
}
```

3. Reword the module doc comment (the D12 sentence, lines 7–11). Replace:

```rust
//! JIT-provisions an unknown identity, AC 2), and, on success, inserts an [`AuthContext`]
//! request extension for downstream handlers. Every rejection short-circuits through the
//! shared `AuthnApiError` funnel (D12): a missing or malformed header, like any other
//! `InvalidToken`, is a 401 `invalid_token` with a `WWW-Authenticate` challenge; the token
//! itself is never logged (nothing here logs it, and `AuthnError`'s own contract keeps
//! claim/token material out of its `Display`).
```

with:

```rust
//! JIT-provisions an unknown identity, AC 2), and, on success, inserts an [`AuthContext`]
//! request extension for downstream handlers. Every rejection short-circuits through the
//! shared `AuthnApiError` funnel (D12): status and body are always 401 `invalid_token`;
//! only the `WWW-Authenticate` challenge distinguishes a fully-absent `Authorization`
//! header (bare `Bearer`, RFC 6750 §3.1) from a present-but-rejected credential
//! (`Bearer error="invalid_token"`). The token itself is never logged (nothing here logs
//! it, and `AuthnError`'s own contract keeps claim/token material out of its `Display`).
```

- [ ] **Step 4: Run the full http suite — expect PASS**

```bash
cargo clippy -p paigasus-iam --all-targets -- -D warnings
cargo nextest run -p paigasus-iam -E 'binary(http_authn)'
```
Expected: clippy clean; ALL tests PASS — including `every_protected_v1_route_requires_bearer` (it asserts status + body code only, both unchanged) and the two Step-1 tests.

- [ ] **Step 5: Format + commit**

```bash
cargo fmt
cd .. && git add rs/crates/services/paigasus-iam/src/adapters/http/auth_middleware.rs rs/crates/services/paigasus-iam/tests/http_authn.rs
git commit -m "feat(rs): bare WWW-Authenticate challenge on absent credentials (SMA-454)

RFC 6750 3.1: a request with no authentication information gets a
challenge without an error attribute. Present-but-unusable headers and
rejected tokens keep error=invalid_token; status and body are identical
in all cases, and gRPC is unchanged.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 12: C6 — deny.toml hygiene + full CI gate graph

**Files:**
- Modify: `rs/deny.toml`

- [ ] **Step 1: Scope BSL-1.0 and drop the stale advisory ignore**

In `rs/deny.toml`:

1. In `[advisories] ignore`, delete the whole `RUSTSEC-2025-0111` entry INCLUDING its comment block (the 8 comment lines starting `# RUSTSEC-2025-0111 ("Tarmageddon")` and the `"RUSTSEC-2025-0111",` line). Keep `RUSTSEC-2023-0071` untouched.
2. In `[licenses] allow`, delete the `BSL-1.0` entry and its 3 comment lines (`"BSL-1.0",` through the `# RedisJwksCache)` line).
3. In `[licenses] exceptions`, add after the `webpki-roots` entry:

```toml
  # xxhash-rust is BSL-1.0-only (Boost Software License 1.0: permissive, OSI-approved),
  # pulled in unconditionally by redis (SMA-443 Task 8's RedisJwksCache). Scoped here
  # rather than globally allowed so a future BSL-1.0 dependency is a conscious decision.
  { name = "xxhash-rust", allow = ["BSL-1.0"] },
```

- [ ] **Step 2: Run the deny gate first (fast feedback)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch
moon run repo:deny
```
Expected: PASS. If licenses fails naming another BSL-1.0-only crate, add a scoped exception for that crate too (same shape) and re-run — do NOT restore the global allow. If advisories fails on RUSTSEC-2025-0111 still being reachable, STOP and report (the lockfile claim was wrong).

- [ ] **Step 3: Commit**

```bash
git add rs/deny.toml
git commit -m "chore(rs): scope BSL-1.0 to xxhash-rust, drop stale tokio-tar advisory ignore (SMA-454)

testcontainers 0.27 (SMA-453) no longer pulls tokio-tar, so the
RUSTSEC-2025-0111 ignore is dead; BSL-1.0 moves from the global license
allowlist to a crate-scoped exception.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

- [ ] **Step 4: Run the FULL repo gate graph exactly like CI**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-454-cleanup-batch
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations
```
Expected: ALL gates green. This is the pre-push verification for the whole branch — if anything reds, fix it (or report if it's outside this batch's scope) before Stage 5.

---

## Verification checklist (after all tasks)

- [ ] `cargo nextest run --workspace --no-tests=pass` from `rs/`: everything green (was 181 at baseline; 16 new tests → expect 197).
- [ ] Full `moon ci` gate graph green (Task 12 Step 4).
- [ ] `git log --oneline origin/main..HEAD` shows the 2 spec commits + 1 plan commit + 12 implementation commits, all conventional, all mentioning SMA-454.
- [ ] Diff review: no stray debug code, no `#[allow]` additions beyond what the spec names, every touched file still opens with the SPDX header.
