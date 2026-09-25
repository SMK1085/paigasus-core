# SMA-686: Refuse a Keycloak ID Token or Logout Token as a Bearer Token — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** IAM's OIDC validator refuses a correctly signed token whose payload `typ` claim is `ID` or `Logout` (ASCII case-insensitive), with a new `TokenDefect::NotAnAccessToken` and one `info` log line.

**Architecture:** One new private check in `validator.rs`, called right after `jsonwebtoken::decode` (so it reads only signature-verified claims). `WireClaims` gets `typ: Option<serde_json::Value>`. The HTTP and gRPC funnels already map every `InvalidToken(_)` to 401 / `Unauthenticated` `invalid-token`, so no wire code changes. A real-Keycloak integration test proves the refusal of a real ID token.

**Tech Stack:** Rust 2024 (rust-version 1.95), `jsonwebtoken` 11.1.0, `serde_json`, `tracing`, `tracing-subscriber` (test only), `testcontainers` + Keycloak 26.4, `cargo nextest`, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-25-sma-686-refuse-id-token-bearer-design.md` (evidence: `docs/superpowers/specs/2026-09-25-sma-686-measurements.md`). Read the spec before you start.

## Global Constraints

- Branch: `feature/sma-686-iam-refuse-id-token-bearer`, in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-686-id-token-bearer`. Before your first command, run `git branch --show-current` and check that it prints this branch. Never commit on `main`.
- Every command runs with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` first.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`. (This plan creates no new source file.)
- Conventional commits with a workspace scope, lower-case subject: `feat(rs): …`, `test(rs): …`, `docs(rs): …`. End every commit message with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never put `#NNN` or a `token: value` line in the commit BODY (commitlint `footer-leading-blank`).
- Never use `git commit --amend`, `git reset`, `git stash` or `--no-verify`. Make a new commit for every fix.
- If the commit fails with `1Password: failed to fill whole buffer`, 1Password is locked. Stop and report; do not bypass signing.
- The marker values are exactly `"ID"` and `"Logout"`, compared with `str::eq_ignore_ascii_case`. No trimming. No other claim is a marker: `at_hash`, `c_hash` and `nonce` are NOT markers (spec D3).
- `typ` is read as `Option<serde_json::Value>`. A JSON `null` or a non-string value is not a marker and must not cause `Malformed` (spec D6).
- The log line has the issuer and the matched marker as a static string only. It never contains `sub`, `email`, any other claim value, or token material (spec D8).
- Run all foreground. Do not use `run_in_background`, and do not wait on a background job.
- `rs/Cargo.toml` sets `warnings = "deny"`: dead code and unused imports are compile errors.

## Review Focus

1. A token with no `typ` claim at all (every Dex token, spec § 2) — must be accepted. Pinned by Task 1 test `accepts_non_marker_typ_values` (case "Dex shape").
2. A Keycloak access token with `typ: "DPoP"` or `"Bearer"` — must be accepted. Pinned by Task 1 `accepts_non_marker_typ_values`.
3. An expired ID token — must report `Expired`, not `NotAnAccessToken` (the check runs after `decode`). Pinned by Task 1 `expired_id_token_reports_expired`.
4. A `typ` with odd JSON shape (`null`, a number) — must be accepted, never `Malformed`. Pinned by Task 1 `accepts_non_marker_typ_values`.
5. An operator reading logs after a 401 — must find one `info` line with the issuer and the marker, and no personal data. Pinned by Task 2 `refusal_logs_issuer_and_marker_only`.

---

## File map

| File | Change | Task |
|---|---|---|
| `rs/crates/libs/paigasus-iam-core/src/authn.rs` | new `TokenDefect::NotAnAccessToken` | 1 |
| `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` | `WireClaims.typ`, `check_access_token_type`, step 6, module doc, unit tests | 1, 2 |
| `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs` | add the new variant to one test's defect list | 1 |
| `rs/crates/services/paigasus-iam/Cargo.toml` | dev-dependency `tracing-subscriber` | 2 |
| `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs` | real ID-token refusal, marker assertions, audiences | 3 |
| `docs/ops/RUNBOOK-chart.md` § 6 | item 1 addition, new paragraph | 5 |
| `rs/crates/services/paigasus-iam/CHANGELOG.md` | `### Security` entry | 5 |

---

### Task 1: The `typ` check in the validator

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authn.rs:162-172` (the `TokenDefect` enum)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` (module doc lines 3-7, `WireClaims` lines 128-141, `authenticate` lines 202-207, tests module)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs:140` (test defect list)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces:
  - `paigasus_iam_core::TokenDefect::NotAnAccessToken` (public enum variant).
  - In `validator.rs` (private): `const NON_ACCESS_TOKEN_TYPES: [&str; 2]`, `fn check_access_token_type(_issuer: &Issuer, claims: &WireClaims) -> Result<(), AuthnError>` (Task 2 renames `_issuer` to `issuer`), and test helpers `const ISSUER: &str`, `fn claims_with(extra: serde_json::Value) -> serde_json::Value`, `async fn authenticate_json(claims: &serde_json::Value) -> Result<ValidatedClaims, AuthnError>`. Task 2 uses the function and the helpers.

- [ ] **Step 1: Add the variant (needed so the tests compile)**

In `rs/crates/libs/paigasus-iam-core/src/authn.rs`, change the enum to:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenDefect {
    Malformed,
    UnsupportedAlg,
    UnknownKid,
    BadSignature,
    Expired,
    NotYetValid,
    IssuerNotConfigured,
    AudienceMismatch,
    Oversized,
    /// The payload `typ` claim marks the token as a Keycloak ID token or back-channel logout
    /// token, not an access token (SMA-686).
    NotAnAccessToken,
}
```

- [ ] **Step 2: Write the failing unit tests**

Append inside `mod tests` of `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` (after the last test, before the closing `}` of the module):

```rust
    // ---- SMA-686: the payload `typ` check -------------------------------------------------

    const ISSUER: &str = "https://idp.example.com";

    /// A token that passes every other check (issuer, audience `aud`, one hour of life),
    /// plus the claims in `extra`. So only the extra claims differ from an accepted token.
    fn claims_with(extra: serde_json::Value) -> serde_json::Value {
        let mut claims = serde_json::json!({
            "iss": ISSUER,
            "sub": "sub-1",
            "aud": "aud",
            "exp": Utc::now().timestamp() + 3600,
            "email": "alice@example.com",
        });
        let extra = extra.as_object().expect("extra claims are a JSON object").clone();
        claims.as_object_mut().expect("claims are a JSON object").extend(extra);
        claims
    }

    /// Signs `claims` with a fresh ES256 key and runs the full validator pipeline on it.
    async fn authenticate_json(claims: &serde_json::Value) -> Result<ValidatedClaims, AuthnError> {
        let (encoding_key, jwk, kid) = es256_keypair();
        let token = sign(&encoding_key, Some(&kid), claims);
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        authenticator.authenticate(&token).await
    }

    #[tokio::test]
    async fn refuses_keycloak_id_and_logout_typ() {
        // Spec § 5.1 tests 1-4: Keycloak sets `typ: ID` on the ID token and `typ: Logout` on the
        // back-channel logout token (measured, SMA-686 measurements). Case-insensitive.
        for typ in ["ID", "id", "Logout", "LOGOUT"] {
            let err = authenticate_json(&claims_with(serde_json::json!({ "typ": typ }))).await.unwrap_err();
            assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)), "typ {typ:?} must be refused as NotAnAccessToken, got {err:?}");
        }
    }

    #[tokio::test]
    async fn accepts_non_marker_typ_values() {
        // Spec § 5.1 tests 5-10. The check is a denylist of two values: every other shape passes.
        let cases = [
            ("Bearer (Keycloak access token)", serde_json::json!({ "typ": "Bearer" })),
            ("DPoP (Keycloak DPoP-bound access token)", serde_json::json!({ "typ": "DPoP" })),
            // Every Dex access token: no `typ`, but `at_hash` and `nonce` (measured). `c_hash`
            // added too: none of the three is a marker (spec D3).
            ("Dex shape", serde_json::json!({ "at_hash": "x", "c_hash": "y", "nonce": "abc123" })),
            ("typ null", serde_json::json!({ "typ": null })),
            ("typ number", serde_json::json!({ "typ": 1 })),
            ("typ with leading space", serde_json::json!({ "typ": " ID" })),
        ];
        for (name, extra) in cases {
            authenticate_json(&claims_with(extra)).await.unwrap_or_else(|err| panic!("{name}: must be accepted, got {err:?}"));
        }
    }

    #[tokio::test]
    async fn expired_id_token_reports_expired() {
        // Spec D5: the check runs after `decode`, so the expiry defect wins.
        let claims = claims_with(serde_json::json!({ "typ": "ID", "exp": Utc::now().timestamp() - 120 }));
        let err = authenticate_json(&claims).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Expired)), "got {err:?}");
    }
```

Note: `claims_with`'s `extend` overwrites `exp` in the last test. That is intentional.

In `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs` line 140, change the defect list to:

```rust
        for defect in [TokenDefect::Malformed, TokenDefect::Expired, TokenDefect::Oversized, TokenDefect::BadSignature, TokenDefect::NotAnAccessToken] {
```

- [ ] **Step 3: Run the tests and check that they fail**

Run (from `rs/`): `cargo nextest run -p paigasus-iam --lib -E 'test(/typ|expired_id_token|invalid_token_is_401/)' --no-fail-fast`
Expected: `refuses_keycloak_id_and_logout_typ` FAILS (the token is accepted, so `unwrap_err` panics). `accepts_non_marker_typ_values`, `expired_id_token_reports_expired` and `invalid_token_is_401_with_bearer_challenge` PASS already (that is correct: they pin behavior that must not change).

- [ ] **Step 4: Implement the check**

In `validator.rs`:

(a) Replace the module doc comment (lines 3-7) with:

```rust
//! The `Authenticator` v1 implementation (spec §4.1): a provider-agnostic OIDC access token
//! validator. Pipeline: length cap -> header decode + alg allowlist + `kid` presence ->
//! unverified `iss` read -> exact issuer match -> JWKS `kid` lookup -> JWK/alg family
//! consistency -> signature + claims validation (issuer/audience/expiry) -> payload `typ`
//! check -> `ValidatedClaims`. The `typ` check is the one Keycloak-specific rule (SMA-686): it
//! refuses a Keycloak ID token or logout token, and passes every token without such a `typ`.
//! Never logs token or claim material (`TokenDefect` itself carries no payload).
```

(b) After `const ALLOWED_ALGORITHMS …;` (line 26), add:

```rust

/// Payload `typ` values that mark a token as NOT an access token (SMA-686 spec D2). Keycloak
/// sets `ID` on its ID token and `Logout` on its back-channel logout token; its access token
/// carries `Bearer` (or `DPoP`). Compared ASCII case-insensitively. A denylist, not an
/// allowlist: an IdP that sets no `typ` (Dex, measured) must keep working (spec D1).
const NON_ACCESS_TOKEN_TYPES: [&str; 2] = ["ID", "Logout"];
```

(c) In `WireClaims`, add the field after `zoneinfo` and extend the doc comment's last sentence:

```rust
/// The claims this validator reads off a token, deserialized only AFTER `jsonwebtoken` has
/// verified the signature (spec §4.1). `sub`/`exp`/`aud` are required — their absence (or a
/// wrong-shaped value) is a serde failure, which `map_jwt_error` collapses to `Malformed`;
/// the profile claims are optional since an IdP may omit any of them. `typ` is an untyped
/// `Value` on purpose (SMA-686 spec D6): a non-string `typ` must not become `Malformed`.
#[derive(Deserialize)]
struct WireClaims {
    sub: String,
    exp: u64,
    aud: WireAudience,
    email: Option<String>,
    name: Option<String>,
    locale: Option<String>,
    zoneinfo: Option<String>,
    typ: Option<serde_json::Value>,
}
```

(d) After `fn check_kty_matches_alg … }` (ends line 167), add:

```rust

/// Refuses a token whose payload `typ` is one of `NON_ACCESS_TOKEN_TYPES` (SMA-686). Runs on
/// signature-verified claims only (spec D5). A missing, `null` or non-string `typ` passes.
fn check_access_token_type(_issuer: &Issuer, claims: &WireClaims) -> Result<(), AuthnError> {
    let Some(serde_json::Value::String(typ)) = &claims.typ else {
        return Ok(());
    };
    match NON_ACCESS_TOKEN_TYPES.iter().find(|marker| typ.eq_ignore_ascii_case(marker)) {
        Some(_marker) => Err(invalid(TokenDefect::NotAnAccessToken)),
        None => Ok(()),
    }
}
```

(The parameter is `_issuer` and the binding is `_marker` in this task, because an unused variable is a compile error under `warnings = "deny"`. Task 2 renames both when it adds the log line.)

(e) In `authenticate`, directly after the `let token_data = decode::<WireClaims>(…)…?;` line, insert:

```rust

        // 6. Token-type check on the verified claims (SMA-686): a Keycloak ID or logout token.
        check_access_token_type(&issuer, &token_data.claims)?;
```

- [ ] **Step 5: Run the tests and check that they pass**

Run (from `rs/`): `cargo nextest run -p paigasus-iam --lib --no-fail-fast`
Expected: all PASS, including the four tests from Step 3 and every older validator test.

Run: `cargo nextest run -p paigasus-iam-core --no-tests=pass`
Expected: PASS.

- [ ] **Step 6: Lint and format**

Run (from `rs/`): `cargo fmt --check && cargo clippy -p paigasus-iam -p paigasus-iam-core --all-targets --locked -- -D warnings`
Expected: no output from fmt, clippy clean. If fmt reports a diff, run `cargo fmt` and re-run.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src/authn.rs rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs rs/crates/services/paigasus-iam/src/adapters/http/authn.rs
git commit -m "feat(rs): iam refuses a keycloak id or logout token as a bearer token (SMA-686)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Log the refusal

**Files:**
- Modify: `rs/crates/services/paigasus-iam/Cargo.toml` (`[dev-dependencies]`, line 130 onward)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` (`check_access_token_type`, tests module)

**Interfaces:**
- Consumes: from Task 1 — `check_access_token_type(issuer: &Issuer, claims: &WireClaims)`, `NON_ACCESS_TOKEN_TYPES`, test helpers `ISSUER`, `claims_with`, `authenticate_json`.
- Produces: nothing new for later tasks.

- [ ] **Step 1: Add the dev-dependency**

In `rs/crates/services/paigasus-iam/Cargo.toml`, under `[dev-dependencies]`, add (keep the section's existing order style; the workspace already declares it at `rs/Cargo.toml:63`, so the lockfile gets no new package):

```toml
# SMA-686: captures the validator's refusal log line in a unit test.
tracing-subscriber = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Append inside `mod tests` of `validator.rs`, after the Task 1 tests:

```rust
    // ---- log capture (copy of the `LogBuffer` helper in paigasus-gateway's
    // `adapters/http/auth.rs` tests; a crate cannot share a `#[cfg(test)]` helper) ----------

    #[derive(Clone, Default)]
    struct LogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
        type Writer = LogBuffer;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    impl LogBuffer {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    /// TRACE, not INFO: the "no personal data" assertion below must see every level.
    fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt().with_writer(buffer.clone()).with_ansi(false).with_max_level(tracing::Level::TRACE).finish();
        (buffer, tracing::subscriber::set_default(subscriber))
    }

    #[tokio::test]
    async fn refusal_logs_issuer_and_marker_only() {
        // Spec D8 / § 5.1 test 12. `#[tokio::test]` is current-thread, so the thread-local
        // subscriber from `set_default` sees the validator's log line.
        let (logs, _guard) = capture_logs();
        let err = authenticate_json(&claims_with(serde_json::json!({ "typ": "id" }))).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)));

        let text = logs.text();
        let refusal_lines: Vec<&str> = text.lines().filter(|line| line.contains("not an access token")).collect();
        assert_eq!(refusal_lines.len(), 1, "exactly one refusal line expected, got:\n{text}");
        let line = refusal_lines[0];
        assert!(line.contains("INFO"), "the refusal logs at info: {line}");
        assert!(line.contains(ISSUER), "the refusal names the issuer: {line}");
        // The canonical marker, not the token's own spelling ("id").
        assert!(line.contains("\"ID\"") || line.contains("=ID"), "the refusal names the canonical marker: {line}");
        for secret in ["sub-1", "alice@example.com", "\"id\""] {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }
```

- [ ] **Step 3: Run the test and check that it fails**

Run (from `rs/`): `cargo nextest run -p paigasus-iam --lib -E 'test(refusal_logs_issuer_and_marker_only)'`
Expected: FAIL with "exactly one refusal line expected, got:" (nothing is logged yet).

- [ ] **Step 4: Add the log line**

Replace the body of `check_access_token_type` (keep its doc comment, add one sentence) with:

```rust
/// Refuses a token whose payload `typ` is one of `NON_ACCESS_TOKEN_TYPES` (SMA-686). Runs on
/// signature-verified claims only (spec D5). A missing, `null` or non-string `typ` passes.
/// Logs the refusal at `info` with the issuer and the CANONICAL marker only — never the
/// token's own `typ` spelling or any other claim (spec D8).
fn check_access_token_type(issuer: &Issuer, claims: &WireClaims) -> Result<(), AuthnError> {
    let Some(serde_json::Value::String(typ)) = &claims.typ else {
        return Ok(());
    };
    match NON_ACCESS_TOKEN_TYPES.iter().find(|marker| typ.eq_ignore_ascii_case(marker)) {
        Some(marker) => {
            tracing::info!(issuer = issuer.as_str(), typ = *marker, "refused a bearer token: its typ claim marks it as not an access token");
            Err(invalid(TokenDefect::NotAnAccessToken))
        }
        None => Ok(()),
    }
}
```

This replaces Task 1's `_issuer` and `_marker` with `issuer` and `marker`.

- [ ] **Step 5: Run the tests and check that they pass**

Run (from `rs/`): `cargo nextest run -p paigasus-iam --lib --no-fail-fast`
Expected: all PASS. If the marker assertion fails, print the line: the `fmt` formatter renders a `&str` field as `typ="ID"`; adjust only the assertion's accepted spellings, never the log call.

- [ ] **Step 6: Lint, format, machete**

Run (from `rs/`): `cargo fmt --check && cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings`
Expected: clean.
Run (from repo root): `moon run repo:machete`
Expected: PASS (`tracing-subscriber` is used by the test).

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/Cargo.toml rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs rs/Cargo.lock
git commit -m "feat(rs): log the refusal of a non-access bearer token at info (SMA-686)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

(`rs/Cargo.lock` must be unchanged; `git add` of an unchanged file is a no-op. If `git diff --cached --stat` shows a lockfile change, stop and report.)

---

### Task 3: Real Keycloak ID token in the integration test

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs` (imports lines 32-44, token section lines 118-148, the assertions before `provision_platform_admin` at line 156, comments at lines 150-152 and 190-191, `keycloak_config` audiences at line 213)

**Interfaces:**
- Consumes: `paigasus_iam_core::{AuthnError, TokenDefect}` (Task 1 variant `NotAnAccessToken`); `paigasus_iam::application::authenticate_token::Provisioning`; `state.authn.resolve(&str, Provisioning) -> Result<AuthnPrincipal, AuthnError>`; `support::send(&Router, &str, &str, Option<Value>, Option<&str>) -> (StatusCode, Value)`.
- Produces: nothing for later tasks.

- [ ] **Step 1: Add imports**

Add to the `use` block:

```rust
use paigasus_iam::application::authenticate_token::Provisioning;
use paigasus_iam_core::{AuthnError, TokenDefect};
```

Add this helper function near `dump_logs` (outside the test function):

```rust
/// Decodes a JWT's payload segment WITHOUT verifying it — test inspection only.
fn jwt_payload(token: &str) -> Value {
    let segment = token.split('.').nth(1).expect("jwt has a payload segment");
    let bytes = URL_SAFE_NO_PAD.decode(segment).expect("base64url-decodable payload");
    serde_json::from_slice(&bytes).expect("json payload")
}
```

- [ ] **Step 2: Read the ID token and assert the markers**

Directly after the line `let access_token = token_body["access_token"]…;` add:

```rust
    // SMA-686: `scope=openid` also returns an ID token. Pin the Keycloak markers this change
    // relies on, so a Keycloak upgrade that drops them reds here rather than silently
    // re-opening the gap (spec § 5.2).
    let id_token = token_body["id_token"].as_str().expect("id_token in token response (scope=openid)").to_string();
    let id_claims = jwt_payload(&id_token);
    assert_eq!(id_claims["typ"], "ID", "keycloak ID token must carry typ=ID: {id_claims}");
    assert_eq!(id_claims["aud"], "paigasus-cli", "keycloak ID token aud is the client id: {id_claims}");
    let access_claims = jwt_payload(&access_token);
    assert_eq!(access_claims["typ"], "Bearer", "keycloak access token must carry typ=Bearer: {access_claims}");
    let access_aud_has_paigasus = match &access_claims["aud"] {
        Value::String(aud) => aud == "paigasus",
        Value::Array(auds) => auds.iter().any(|aud| aud == "paigasus"),
        _ => false,
    };
    assert!(access_aud_has_paigasus, "keycloak access token aud must contain paigasus: {access_claims}");
```

- [ ] **Step 3: Assert the refusal (before any access-token call)**

Directly after `let app = router(state.clone());` and BEFORE the comment block that precedes `provision_platform_admin(&state, &access_token).await;`, insert:

```rust
    // SMA-686 AC1: the configured audiences include the client id (`paigasus-cli`, see
    // `keycloak_config`), exactly like the chart default — so the ID token passes the issuer,
    // signature, audience and expiry checks and reaches the `typ` check. Assert the DEFECT
    // through the use case: every defect renders the same 401, so a 401 alone cannot prove
    // which check refused the token.
    let err = state.authn.resolve(&id_token, Provisioning::Enabled).await.expect_err("an ID token must not authenticate");
    assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)), "ID token must be refused as NotAnAccessToken, got {err:?}");

    // The wire contract on both HTTP paths: the bearer middleware and the exempt introspect.
    let (status, body) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "idtok", "name": "ID token" })), Some(&id_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid-token", "{body}");
    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": id_token })), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid-token", "{body}");
```

- [ ] **Step 4: Change the audiences and the stale comments**

In `keycloak_config`, change `audiences: vec!["paigasus".to_string()],` to:

```rust
                // `paigasus` is the access token's aud (audience mapper); `paigasus-cli` is the
                // client id, which the chart accepts by default — and which is the ID token's
                // aud. Both are accepted so the ID token reaches the SMA-686 `typ` check.
                audiences: vec!["paigasus".to_string(), "paigasus-cli".to_string()],
```

Update the doc comment of `keycloak_config` (line ~190-192): replace `(audience \`paigasus\`, JIT on)` with `(audiences \`paigasus\` and the client id \`paigasus-cli\`, JIT on)`.

Update the comment at lines 150-152: replace `so a 201 here is itself proof the audience + email mappers landed both claims in the ACCESS token.` with `so a 201 here is itself proof the email mapper landed its claim in the ACCESS token (the audience is asserted on the decoded payload above).`

- [ ] **Step 5: Run the test**

Docker must be running. Run (from `rs/`):
`PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test keycloak_e2e --no-fail-fast`
Expected: PASS (Keycloak start takes up to ~4 minutes). A skip is impossible with `PAIGASUS_REQUIRE_DOCKER=1`; a Docker failure panics.

Because Task 1 is already in place, this test cannot go red first. Task 4 proves that it bites.

- [ ] **Step 6: Lint and format**

Run (from `rs/`): `cargo fmt --check && cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs
git commit -m "test(rs): refuse a real keycloak id token in the e2e suite (SMA-686)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Prove that the tests bite (mutation run, no commit)

**Files:**
- Temporarily modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` (one line), then restore it.

**Interfaces:**
- Consumes: the step-6 call from Task 1.
- Produces: a result table in the task report. No commit.

- [ ] **Step 1: Apply the mutation**

With the Edit tool, replace exactly:

```rust
        check_access_token_type(&issuer, &token_data.claims)?;
```

with:

```rust
        let _ = check_access_token_type(&issuer, &token_data.claims); // SMA-686 MUTATION
```

This compiles under `warnings = "deny"`: the function and `typ` stay used.

- [ ] **Step 2: Run the suites**

Run (from `rs/`):
`cargo nextest run -p paigasus-iam --lib --no-fail-fast 2>&1 | tail -30`
Expected: FAIL for `refuses_keycloak_id_and_logout_typ` and `refusal_logs_issuer_and_marker_only` (the log line is still written, but the test's `matches!(err, …)` on `unwrap_err` panics because the token is accepted). PASS for `accepts_non_marker_typ_values`, `expired_id_token_reports_expired`, `invalid_token_is_401_with_bearer_challenge`, and every older test.

Run: `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test keycloak_e2e --no-fail-fast 2>&1 | tail -30`
Expected: FAIL at "an ID token must not authenticate" (`expect_err` on an `Ok`).

If a compile error appears instead of test failures, the mutation is wrong: stop and report. A compile error proves nothing.

- [ ] **Step 3: Restore by an edit**

With the Edit tool, replace the mutated line back to exactly:

```rust
        check_access_token_type(&issuer, &token_data.claims)?;
```

Do NOT use `git checkout` or `git restore`. Then run `git diff --stat`; expected: no output (the tree equals the last commit).

- [ ] **Step 4: Re-run to confirm green**

Run (from `rs/`): `cargo nextest run -p paigasus-iam --lib --no-fail-fast`
Expected: all PASS.

- [ ] **Step 5: Report**

Report a table: test name, result under the mutation, expected result. No commit in this task.

---

### Task 5: Runbook and CHANGELOG

**Files:**
- Modify: `docs/ops/RUNBOOK-chart.md` § 6 (item 1 at lines 97-105; new paragraph after item 4 at line ~110)
- Modify: `rs/crates/services/paigasus-iam/CHANGELOG.md:11` (`## [Unreleased]`)

**Interfaces:**
- Consumes: the behavior of Tasks 1-2 (the values `ID` and `Logout`, the `info` log line).
- Produces: nothing for later tasks.

Write in ASD-STE100 Simplified Technical English, like the rest of the runbook: short sentences, active voice, no idiom.

- [ ] **Step 1: Item 1 (Audience)**

In § 6 item 1, after the sentence "The value replaces the client id. It does not add another value next to the client id.", insert:

```markdown
   Also set `oidc.audience` when the IdP's ID token has the same `aud` as the access token and
   no Keycloak `typ` claim. The paragraph after this list tells you why.
```

- [ ] **Step 2: The new paragraph**

After item 4 (Discovery) and before the line "The console requests the scopes `openid profile email offline_access`.", insert:

```markdown
**IAM refuses a token that is not an access token (SMA-686).** IAM refuses a bearer token whose
`typ` claim is `ID` or `Logout`, in any letter case. Keycloak sets these values on its ID token
and on its back-channel logout token. Its access token has `typ: Bearer`. The IAM log shows each
refusal at `info`, with the issuer and the `typ` value.

This adds no requirement on the IdP. No Keycloak or Dex access token measured for SMA-686 has
one of these values. Do not add a mapper that sets `typ` on the access token.

The check does not protect an IdP whose ID token has no `typ` claim. Dex is an example. Decode a
real ID token and a real access token from your IdP. If the ID token has no `typ: ID`, choose an
audience that is in the access token's `aud` and not in the ID token's `aud`. Then set
`oidc.audience` to that audience. If your IdP cannot do this, IAM accepts its ID token as a
bearer token. For Dex, both tokens have the same `aud`, so this remedy does not work.
```

- [ ] **Step 3: CHANGELOG**

In `rs/crates/services/paigasus-iam/CHANGELOG.md`, change

```markdown
## [Unreleased]

## [0.1.0] - 2026-09-20
```

to

```markdown
## [Unreleased]

### Security

- IAM refuses a bearer token whose `typ` claim is `ID` or `Logout`. Keycloak sets these values
  on its ID token and its back-channel logout token. Before, IAM accepted such a token when its
  `aud` held the accepted audience, which is the client id in the default chart configuration.
  The refusal is logged at `info` (SMA-686).

## [0.1.0] - 2026-09-20
```

- [ ] **Step 4: Check the gated marker blocks and formatting**

Run (from repo root): `git diff --stat`
Expected: only the two files. The root `CLAUDE.md` is not touched.

- [ ] **Step 5: Commit**

```bash
git add docs/ops/RUNBOOK-chart.md rs/crates/services/paigasus-iam/CHANGELOG.md
git commit -m "docs(rs): runbook and changelog for the id token refusal (SMA-686)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Final verification

**Files:** none changed (fix-forward commits only if a gate reds).

- [ ] **Step 1: The crate targets**

Run (from repo root): `moon run paigasus-iam-core-rs:test paigasus-iam-core-rs:lint paigasus-iam-core-rs:fmt paigasus-iam-rs:lint paigasus-iam-rs:fmt`
Expected: all PASS.

Run (from repo root): `moon run paigasus-iam-rs:test`
Expected: PASS. This runs the Docker-backed suites; Docker must be up. If `authz_policy_store.rs` or another Docker suite flakes, re-run that binary alone with `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test <name>` and report both results. Never read a failure as a flake without that re-run.

- [ ] **Step 2: The affected graph like CI**

Run (from repo root) the command between the `ci-targets` markers in the root `CLAUDE.md`, with `--base main` in place of `--base origin/main` if `git fetch` is not possible. Read the "This development Mac only" section of `CLAUDE.md` first: a `mapfile`/`declare -A` failure under bash 3.2, or an actionlint rc 2 "pipe holds only 512 bytes", is a host artifact, not a finding. Re-run such a gate directly with the right bash (`/opt/homebrew/bin/bash ci/<gate>/run.sh` or `/bin/bash ci/<gate>/run.sh`) and report that result.
Expected: every gate the change selects is green, or has a documented host-artifact explanation with a direct green re-run.

- [ ] **Step 3: The diff matches the plan**

Run: `git diff main --stat` and `git log --oneline main..HEAD`.
Expected: the spec, the measurements, this plan, and the files of Tasks 1-3 and 5 only. No debug code (`grep -n "dbg!\|MUTATION" -r rs/crates/services/paigasus-iam/src` prints nothing).

- [ ] **Step 4: After the push (Stage 6, not in this plan's commits)**

Start the kind job on the branch, because `chart.yml` does not run on an `rs/`-only PR (spec § 7):
`gh workflow run chart.yml --ref feature/sma-686-iam-refuse-id-token-bearer`
Record the run URL in the PR body. That run is the AC2 proof for the kind realm.
