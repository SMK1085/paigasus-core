# SMA-690 Refuse a Sender-Constrained Token — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** IAM refuses a verified access token that has a non-null `cnf` claim or a payload `typ` of `DPoP`, with a new `TokenDefect::SenderConstrained`, its own rate-limited log line, a real Keycloak DPoP test, and runbook and CHANGELOG text.

**Architecture:** One new check (step 7) in `OidcAuthenticator::authenticate`, after the SMA-686 token-type check. Every IAM path and the gateway reach this one validator, so no other code changes. The wire response stays 401 `invalid-token`.

**Tech Stack:** Rust 2024 (rust-version 1.95), `jsonwebtoken` 11.1.0, `serde_json`, `tracing`, `p256` 0.14 (dev), `sha2` 0.11, `rand` 0.10, testcontainers with Keycloak 26.4, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-09-26-sma-690-refuse-sender-constrained-token-design.md` (approved at GATE 1). Evidence: `docs/superpowers/specs/2026-09-26-sma-690-measurements.md`.

## Global Constraints

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-690`, branch `feature/sma-690-iam-sender-constrained-token`. Run `git branch --show-current` before the first edit; stop if it is not this branch.
- Before any cargo or moon command: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`. (This plan creates no new source file.)
- `warnings = "deny"` applies to the workspace: dead code, an unused import or an unused variable is a compile error.
- Conventional commits with a workspace scope: `feat(rs): …`, `test(rs): …`, `docs(rs): …`. End each commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `#NNN` and no `token: value` line in the commit body.
- Never `git commit --amend`, never `git reset`, never `git stash`, never `--no-verify`. Make a new commit for each fix.
- Run every command in the foreground. Do not start a background job and then wait for it.
- Install nothing on the host (no `brew`, no `pip`, no `npm -g`).
- The marker strings are exact: `"cnf"` and `"typ DPoP"`. The new log message is exact: `refused a bearer token: it is bound to a key, and IAM cannot check the binding`.
- The SMA-686 log message `refused a bearer token: a verified marker shows it is not an access token` must stay byte-identical.
- Write all prose (doc comments, runbook, CHANGELOG) in ASD-STE100 Simplified Technical English: short sentences, active voice, no idiom.

## Review Focus

1. A `cnf` with JSON `null` must be accepted, and a `cnf` of any other JSON type (string, number, array, empty object) must be refused as `SenderConstrained`, never `Malformed`. Owned by Task 1 (tests 4, 5, 9 plus the array case added below).
2. A token with both an SMA-686 marker and `cnf` must report `NotAnAccessToken`; an expired bound token must report `Expired`. Owned by Task 1 (tests 11, 12).
3. The log line must never contain the `jkt` value or the token's own `typ` spelling. Owned by Task 1 (tests 14, 15).
4. A real Keycloak DPoP proof over HTTPS on the host-mapped port must be accepted by Keycloak (the `htu` and `iat` risks). Owned by Task 2; on failure, the implementer records the error body and stops.
5. A plain Keycloak Bearer access token must still be accepted end to end (201 on the org create, 200 on introspect). Owned by Task 2 (the existing assertions plus the new no-`cnf` assertion).

---

### Task 1: The validator check, the defect, and the unit and mapping tests

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authn.rs:171-175` (the `TokenDefect` enum)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` (lines 3-10, 34-38, 51-54, 91-98, 151-172, 225-246, 340-344, and the test module)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs:140-146` (the defect list of `invalid_token_is_401_with_bearer_challenge`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:841-843` (the cases of `every_authn_status_carries_a_registered_reason_and_its_original_message`)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `paigasus_iam_core::TokenDefect::SenderConstrained` (a unit variant). Task 2 matches on `AuthnError::InvalidToken(TokenDefect::SenderConstrained)`.

- [ ] **Step 1: Add the defect variant**

In `rs/crates/libs/paigasus-iam-core/src/authn.rs`, after the `NotAnAccessToken` variant (line 174), add:

```rust
    /// The token is bound to a key: it has a `cnf` claim (RFC 7800), or a Keycloak payload
    /// `typ` of `DPoP`. IAM cannot check the binding, so it does not accept the token as a
    /// bearer token (SMA-690).
    SenderConstrained,
```

- [ ] **Step 2: Write the failing unit tests**

In `validator.rs`, in the test module, after `expired_id_token_reports_expired` (line 807), add:

```rust
    // ---- SMA-690: the sender-constraint check ---------------------------------------------

    #[tokio::test]
    async fn refuses_sender_constrained_tokens() {
        // Spec § 5.1 tests 1-8, plus an array `cnf` (Review Focus 1).
        let cases = [
            ("cnf.jkt (Keycloak DPoP, M2)", serde_json::json!({ "cnf": { "jkt": "abc" } })),
            ("cnf.x5t#S256 (RFC 8705)", serde_json::json!({ "cnf": { "x5t#S256": "abc" } })),
            ("cnf.jwk (RFC 7800)", serde_json::json!({ "cnf": { "jwk": { "kty": "EC" } } })),
            ("cnf empty object", serde_json::json!({ "cnf": {} })),
            ("cnf string", serde_json::json!({ "cnf": "x" })),
            ("cnf array", serde_json::json!({ "cnf": ["x"] })),
            ("typ DPoP", serde_json::json!({ "typ": "DPoP" })),
            ("typ dpop", serde_json::json!({ "typ": "dpop" })),
            ("full Keycloak shape", serde_json::json!({ "typ": "DPoP", "cnf": { "jkt": "abc" } })),
        ];
        for (name, extra) in cases {
            let err = authenticate_json(&claims_with(extra)).await.unwrap_err();
            assert!(
                matches!(err, AuthnError::InvalidToken(TokenDefect::SenderConstrained)),
                "{name}: must be refused as SenderConstrained, got {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn accepts_null_cnf() {
        // Spec D2 / § 5.1 test 9: a `null` `cnf` confirms no key.
        authenticate_json(&claims_with(serde_json::json!({ "cnf": null })))
            .await
            .expect("cnf: null must be accepted");
    }

    #[tokio::test]
    async fn id_token_with_cnf_reports_not_an_access_token() {
        // Spec D5 / § 5.1 test 11: the SMA-686 check runs first.
        let err = authenticate_json(&claims_with(serde_json::json!({ "typ": "ID", "cnf": { "jkt": "abc" } }))).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)), "got {err:?}");
    }

    #[tokio::test]
    async fn expired_bound_token_reports_expired() {
        // Spec D5 / § 5.1 test 12: the check runs after `decode`.
        let claims = claims_with(serde_json::json!({ "cnf": { "jkt": "abc" }, "exp": Utc::now().timestamp() - 120 }));
        let err = authenticate_json(&claims).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Expired)), "got {err:?}");
    }

    /// `WireClaims` from a JSON payload, for the direct marker test.
    fn wire_claims(extra: serde_json::Value) -> WireClaims {
        serde_json::from_value(claims_with(extra)).expect("test claims deserialize")
    }

    #[test]
    fn sender_constraint_marker_names_the_marker() {
        // Spec § 5.1 test 13: the marker text and the D3 order.
        assert_eq!(sender_constraint_marker(&wire_claims(serde_json::json!({ "cnf": { "jkt": "abc" } }))), Some("cnf"));
        assert_eq!(sender_constraint_marker(&wire_claims(serde_json::json!({ "typ": "DPoP", "cnf": { "jkt": "abc" } }))), Some("cnf"));
        assert_eq!(sender_constraint_marker(&wire_claims(serde_json::json!({ "typ": "dpop" }))), Some("typ DPoP"));
        assert_eq!(sender_constraint_marker(&wire_claims(serde_json::json!({ "cnf": null }))), None);
        assert_eq!(sender_constraint_marker(&wire_claims(serde_json::json!({ "typ": "Bearer" }))), None);
    }
```

In `accepts_non_marker_typ_values` (line 784), replace the comment on line 785 and the `DPoP` case on line 788:

```rust
        // Spec § 5.1 tests 5-10 (SMA-686), and SMA-690 test 10: `DPoP` moved to
        // `refuses_sender_constrained_tokens`. Every other shape here passes.
        let cases = [
            ("Bearer (Keycloak access token)", serde_json::json!({ "typ": "Bearer" })),
            // Every Dex access token: no `typ`, but `at_hash` and `nonce` (measured). `c_hash`
            // added too: none of the three is a marker (spec D3).
            ("Dex shape", serde_json::json!({ "at_hash": "x", "c_hash": "y", "nonce": "abc123" })),
            ("typ null", serde_json::json!({ "typ": null })),
            ("typ number", serde_json::json!({ "typ": 1 })),
            ("typ with leading space", serde_json::json!({ "typ": " ID" })),
            ("typ DPoP with leading space", serde_json::json!({ "typ": " DPoP" })),
        ];
```

After `repeated_refusals_log_once_per_issuer_and_defect` (line 988), add the log tests:

```rust
    /// The SMA-690 log message (spec D8). Filter on this text, not on the SMA-686 text.
    const BINDING_REFUSAL: &str = "it is bound to a key, and IAM cannot check the binding";

    #[tokio::test]
    async fn sender_constrained_refusal_logs_issuer_and_cnf_marker_only() {
        // Spec § 5.1 test 14.
        let (logs, _guard) = capture_logs();
        let err = authenticate_json(&claims_with(serde_json::json!({ "cnf": { "jkt": "jkt-secret-value" } }))).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::SenderConstrained)), "got {err:?}");

        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(BINDING_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "exactly one binding refusal line expected, got:\n{text}");
        let line = lines[0];
        assert!(line.contains("INFO"), "the refusal logs at info: {line}");
        assert!(line.contains(ISSUER), "the refusal names the issuer: {line}");
        assert!(line.contains("\"cnf\"") || line.contains("=cnf"), "the refusal names the marker cnf: {line}");
        assert!(!text.contains("not an access token"), "the SMA-686 line must not appear:\n{text}");
        for secret in ["jkt-secret-value", "sub-1", "alice@example.com"] {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }

    #[tokio::test]
    async fn dpop_typ_refusal_logs_canonical_marker() {
        // Spec § 5.1 test 15: the canonical marker, not the token's own spelling.
        let (logs, _guard) = capture_logs();
        let err = authenticate_json(&claims_with(serde_json::json!({ "typ": "dpop" }))).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::SenderConstrained)), "got {err:?}");

        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(BINDING_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "exactly one binding refusal line expected, got:\n{text}");
        assert!(lines[0].contains("typ DPoP"), "the refusal names the marker typ DPoP: {}", lines[0]);
        assert!(!text.contains("\"dpop\""), "the log must not contain the token's own spelling:\n{text}");
    }

    #[tokio::test]
    async fn repeated_sender_constrained_refusals_log_once() {
        // Spec § 5.1 test 16: the SMA-686 D14 rate limit covers the new defect.
        let (logs, _guard) = capture_logs();
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        for _ in 0..3 {
            let token = sign(&encoding_key, Some(&kid), &claims_with(serde_json::json!({ "cnf": { "jkt": "abc" } })));
            let err = authenticator.authenticate(&token).await.unwrap_err();
            assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::SenderConstrained)));
        }
        let text = logs.text();
        assert_eq!(text.lines().filter(|line| line.contains(BINDING_REFUSAL)).count(), 1, "binding lines:\n{text}");
    }
```

- [ ] **Step 3: Add the wire-mapping cases**

In `adapters/http/authn.rs`, in `invalid_token_is_401_with_bearer_challenge`, add `TokenDefect::SenderConstrained,` after `TokenDefect::NotAnAccessToken,` (line 145).

In `adapters/grpc/convert.rs`, in `every_authn_status_carries_a_registered_reason_and_its_original_message`, add this row after the `Malformed` row (line 842):

```rust
            (AuthnError::InvalidToken(TokenDefect::SenderConstrained), Code::Unauthenticated, "invalid-token", "invalid bearer token"),
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(/sender_constrain|null_cnf|cnf_reports|bound_token|dpop_typ|non_marker_typ/)' --no-fail-fast`
Expected: a compile error, `cannot find function sender_constraint_marker` (and no field `cnf` in `WireClaims` is not reached yet). This is the expected red for a new function.

- [ ] **Step 5: Implement the check**

In `validator.rs`:

(a) After `NON_ACCESS_TOKEN_TYPES` (line 38), add:

```rust
/// Payload `typ` values that mark a sender-constrained token (SMA-690 D3). Keycloak sets `DPoP` on
/// a DPoP-bound access token (measured, SMA-690 measurements M2). Compared ASCII
/// case-insensitively.
const SENDER_CONSTRAINED_TYPES: [&str; 1] = ["DPoP"];
```

(b) Change the `NON_ACCESS_TOKEN_TYPES` doc (lines 34-37) so that its second sentence reads:

```rust
/// Payload `typ` values that mark a token as NOT an access token (SMA-686 spec D2). Keycloak sets
/// `ID` on its ID token and `Logout` on its back-channel logout token; its access token carries
/// `Bearer`. A Keycloak DPoP-bound access token carries `DPoP`, which step 7 refuses (SMA-690).
/// Compared ASCII case-insensitively. A denylist, not an allowlist: an IdP that sets no `typ`
/// (Dex, measured) must keep working (spec D1).
```

(c) In the `RefusalLog` doc (line 53), change `issuers × 2` to `issuers × 3`.

(d) In `RefusalDetail` (lines 91-98), add a variant after `Accepted`:

```rust
    /// The static marker that shows a verified token is bound to a key (SMA-690 D8).
    Binding(&'static str),
```

(e) In `log_refusal`: change its doc (lines 151-154) to:

```rust
    /// The one place that decides which refusals are logged (SMA-686 D8, D11, D14, D15; SMA-690
    /// D8): only `NotAnAccessToken`, `AudienceMismatch` and `SenderConstrained`, each reachable
    /// only for a correctly signed token from a configured issuer, and each rate-limited per
    /// (issuer, defect). Logs the issuer and a static or configured detail — never a token claim.
```

and add a match arm after the `RefusalDetail::Accepted` arm:

```rust
            RefusalDetail::Binding(marker) => {
                tracing::info!(
                    issuer = issuer.as_str(),
                    marker,
                    suppressed,
                    "refused a bearer token: it is bound to a key, and IAM cannot check the binding"
                );
            }
```

(f) In `WireClaims` (line 245), add a field after `events`:

```rust
    cnf: Option<serde_json::Value>,
```

and change the last sentence of its doc (lines 232-234) to:

```rust
/// The profile claims are optional since an IdP may omit any of them. `typ`, `events` and `cnf`
/// are untyped `Value`s on purpose (SMA-686 D6, D12; SMA-690 D6): a non-string `typ`, a
/// non-object `events` or any shape of `cnf` must not become `Malformed`. A JSON `null` `cnf`
/// deserializes as `None`.
```

(g) After `non_access_token_marker` (line 296), add:

```rust
/// The static marker that shows a signature-verified token is bound to a key, or `None`
/// (SMA-690 D2, D3). Marker 1: a `cnf` claim with any value except `null`. Marker 2: a payload
/// `typ` of `DPoP`. Marker 1 is checked first.
fn sender_constraint_marker(claims: &WireClaims) -> Option<&'static str> {
    if claims.cnf.is_some() {
        return Some("cnf");
    }
    let Some(serde_json::Value::String(typ)) = &claims.typ else {
        return None;
    };
    SENDER_CONSTRAINED_TYPES.iter().any(|marker| typ.eq_ignore_ascii_case(marker)).then_some("typ DPoP")
}
```

(h) In `authenticate`, after the step-6 block (line 344), add:

```rust
        // 7. Sender-constraint check on the verified token (SMA-690): IAM cannot check a binding.
        if let Some(marker) = sender_constraint_marker(&token_data.claims) {
            self.log_refusal(&issuer, TokenDefect::SenderConstrained, RefusalDetail::Binding(marker));
            return Err(invalid(TokenDefect::SenderConstrained));
        }
```

(i) Change the module doc (lines 3-10) to:

```rust
//! The `Authenticator` v1 implementation (spec §4.1): a provider-agnostic OIDC access token
//! validator. Pipeline: length cap -> header decode + alg allowlist + `kid` presence ->
//! unverified `iss` read -> exact issuer match -> JWKS `kid` lookup -> JWK/alg family
//! consistency -> signature + claims validation (issuer/audience/expiry) -> payload `typ`
//! check -> sender-constraint check -> `ValidatedClaims`. The token-type check (SMA-686) refuses
//! an ID token or a logout token: a Keycloak payload `typ` (`ID`, `Logout`) or a standard
//! back-channel logout marker (header `typ: logout+jwt`, the `events` member). The
//! sender-constraint check (SMA-690) refuses a token bound to a key (a `cnf` claim, or a Keycloak
//! payload `typ: DPoP`), because IAM cannot check the binding. Three refusals are logged,
//! rate-limited (`log_refusal`). Never logs token or claim material (`TokenDefect` itself carries
//! no payload).
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast`
Expected: PASS, including the two SMA-686 log tests (`refusal_logs_issuer_and_marker_only`, `repeated_refusals_log_once_per_issuer_and_defect`), unchanged.

Run: `cd rs && cargo nextest run -p paigasus-iam-core --no-tests=pass && cargo clippy -p paigasus-iam -p paigasus-iam-core --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS, no warning, no format drift. If `cargo fmt --check` fails, run `cargo fmt` and include the change.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src/authn.rs rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs rs/crates/services/paigasus-iam/src/adapters/http/authn.rs rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs
git commit -m "feat(rs): refuse a sender-constrained access token in IAM (SMA-690)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The Keycloak DPoP integration test

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs` (imports at lines 32-46; new code after line 161 and after line 187; new helpers after `jwt_payload`, line 288)

**Interfaces:**
- Consumes: `paigasus_iam_core::TokenDefect::SenderConstrained` from Task 1.
- Produces: nothing for later tasks.

Do NOT add a helper to `tests/support/mod.rs`. Keep all new helpers in `keycloak_e2e.rs` (a `pub` helper in `support` needs `#[allow(dead_code)]` for every other test binary; this plan avoids that).

- [ ] **Step 1: Add the imports**

Add to the `use` block of `keycloak_e2e.rs`:

```rust
use jsonwebtoken::jwk::Jwk;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use p256::elliptic_curve::Generate;
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::pkcs8::{EncodePrivateKey, LineEnding};
use sha2::Digest;
```

(These are the same `p256` imports that `validator.rs`'s test module uses at lines 373-375.)

- [ ] **Step 2: Add the helpers**

After `jwt_payload` (line 288), add:

```rust
/// An EC P-256 key for a DPoP proof (SMA-690 spec § 5.3): the signing key and the public key's
/// base64url `x` and `y` coordinates.
fn dpop_keypair() -> (EncodingKey, String, String) {
    let secret_key = p256::SecretKey::generate();
    let pem = secret_key.to_pkcs8_pem(LineEnding::LF).expect("valid pkcs8 pem");
    let encoding_key = EncodingKey::from_ec_pem(pem.as_bytes()).expect("valid ec pem");
    let point = secret_key.public_key().to_sec1_point(false);
    let x = URL_SAFE_NO_PAD.encode(point.x().expect("uncompressed point has x"));
    let y = URL_SAFE_NO_PAD.encode(point.y().expect("uncompressed point has y"));
    (encoding_key, x, y)
}

/// A DPoP proof (RFC 9449 § 4.2) for one request: header `typ: dpop+jwt`, `alg: ES256` and the
/// public `jwk`; payload `jti`, `htm`, `htu` and `iat`.
fn dpop_proof(key: &EncodingKey, x: &str, y: &str, htm: &str, htu: &str) -> String {
    let jwk: Jwk = serde_json::from_value(json!({ "kty": "EC", "crv": "P-256", "x": x, "y": y })).expect("public EC jwk");
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some("dpop+jwt".to_string());
    header.jwk = Some(jwk);
    let jti = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 16]>());
    let claims = json!({ "jti": jti, "htm": htm, "htu": htu, "iat": chrono::Utc::now().timestamp() });
    jsonwebtoken::encode(&header, &claims, key).expect("sign the DPoP proof")
}

/// The RFC 7638 SHA-256 thumbprint of a P-256 public key, from the literal canonical JSON.
fn jwk_thumbprint(x: &str, y: &str) -> String {
    let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#);
    URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(canonical.as_bytes()))
}
```

If `chrono` is not in scope as a crate path in this test binary, use `std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_secs()` for `iat`. If `sha2::Sha256::digest` returns a type that `encode` does not accept, pass `.as_slice()`.

- [ ] **Step 3: Request the DPoP-bound token and pin its shape**

After the RS256 header assertion (line 161), add:

```rust
    // SMA-690 AC 2: Keycloak's plain Bearer access token has no `cnf`.
    assert!(access_claims.get("cnf").is_none(), "keycloak Bearer access token must carry no cnf: {access_claims}");

    // SMA-690 AC 1: a DPoP-bound token from the SAME client. Keycloak binds a token when the
    // client sends a DPoP proof, with no client setting (measurement M2).
    let (dpop_key, dpop_x, dpop_y) = dpop_keypair();
    let dpop_response = http
        .post(&token_url)
        .header("DPoP", dpop_proof(&dpop_key, &dpop_x, &dpop_y, "POST", &token_url))
        .form(&[
            ("grant_type", "password"),
            ("client_id", "paigasus-cli"),
            ("username", "alice"),
            ("password", "alice-password"),
            ("scope", "openid"),
        ])
        .send()
        .await
        .expect("DPoP token request");
    let dpop_status = dpop_response.status();
    let dpop_body: Value = dpop_response.json().await.expect("DPoP token response json");
    assert!(dpop_status.is_success(), "DPoP password grant failed ({dpop_status}): {dpop_body}\n{}", dump_logs(&keycloak).await);
    assert_eq!(dpop_body["token_type"], "DPoP", "keycloak must answer a DPoP proof with token_type DPoP: {dpop_body}");
    let dpop_token = dpop_body["access_token"].as_str().expect("access_token in DPoP token response").to_string();
    let dpop_claims = jwt_payload(&dpop_token);
    assert_eq!(dpop_claims["typ"], "DPoP", "keycloak DPoP-bound access token must carry typ=DPoP: {dpop_claims}");
    assert_eq!(
        dpop_claims["cnf"]["jkt"],
        jwk_thumbprint(&dpop_x, &dpop_y),
        "cnf.jkt must be the RFC 7638 thumbprint of the proof key: {dpop_claims}"
    );
```

- [ ] **Step 4: Assert the refusal**

After the ID-token wire assertions (line 187), add:

```rust
    // SMA-690 AC 1: the bound token is refused as SenderConstrained (the defect proves the cause;
    // every defect renders the same 401).
    let err = state.authn.resolve(&dpop_token, Provisioning::Enabled).await.expect_err("a DPoP-bound token must not authenticate");
    assert!(
        matches!(err, AuthnError::InvalidToken(TokenDefect::SenderConstrained)),
        "DPoP-bound token must be refused as SenderConstrained, got {err:?}"
    );
    let (status, body) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "dpop", "name": "DPoP token" })), Some(&dpop_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid-token", "{body}");
    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": dpop_token })), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid-token", "{body}");
```

Also add one sentence to the module doc (after line 12): `SMA-690: the test also gets a DPoP-bound token from the same client and asserts that IAM refuses it as SenderConstrained.`

- [ ] **Step 5: Run the integration test**

Docker must be running. Run:
`cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test keycloak_e2e --no-fail-fast`
Expected: PASS.

If the DPoP grant fails, do NOT change `htu`, the realm fixture or a Keycloak setting to make it pass. Record the full `dpop_body` error and stop, and report it (spec § 5.3 risks). An `iat` error on macOS Docker Desktop is a known local risk; report it with the exact body.

Run: `cd rs && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs
git commit -m "test(rs): refuse a real Keycloak DPoP-bound token end to end (SMA-690)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The runbook and the CHANGELOG

**Files:**
- Modify: `docs/ops/RUNBOOK-chart.md` (insert after line 134, before the line `The console requests the scopes …`)
- Modify: `rs/crates/services/paigasus-iam/CHANGELOG.md` (under `## [Unreleased]` / `### Security`)

**Interfaces:**
- Consumes: the marker strings and the log message from Task 1 (exact text in Global Constraints).
- Produces: nothing.

- [ ] **Step 1: Add the runbook paragraph**

Insert after line 134 of `docs/ops/RUNBOOK-chart.md` (after the paragraph that ends "See "Dex" below."), with one blank line before and after:

```markdown
**IAM refuses a sender-constrained token (SMA-690).** IAM refuses an access token that has one
of these markers:

- A `cnf` claim with any value except `null`. A DPoP-bound token (RFC 9449) and an mTLS-bound
  token (RFC 8705) have this claim.
- A `typ` claim of `DPoP`, in any letter case. Keycloak sets this value on a DPoP-bound access
  token.

IAM cannot check the binding of such a token. So it does not accept the token as a bearer token.

Keycloak binds an access token when the client sends a `DPoP` header to the token endpoint. The
client needs no DPoP setting for this. So a client that calls Paigasus must not send a `DPoP`
header to the token endpoint. A bound login stays bound when the client refreshes the token. A
client that got a bound token must log in again without a `DPoP` header.

Do not set the Keycloak client attribute `dpop.bound.access.tokens` on a client that calls
Paigasus. A Keycloak client policy can also require DPoP. Do not use such a policy for this
client (not measured).

The console does not send a `DPoP` header. The SDKs do not get tokens. They send the token that
you give them. If you use an SDK, do not turn on DPoP in your own OIDC library.

The IAM log shows the refusal at `info`: "it is bound to a key, and IAM cannot check the
binding". The line gives the issuer and the marker `cnf` or `typ DPoP`. The same rate limit
applies as for the refusal of a token that is not an access token.
```

- [ ] **Step 2: Add the CHANGELOG entry**

In `rs/crates/services/paigasus-iam/CHANGELOG.md`, add as the last bullet of the `### Security` list under `## [Unreleased]`:

```markdown
- IAM refuses a sender-constrained access token: a token with a `cnf` claim, or with a Keycloak
  `typ` of `DPoP`. Before, IAM accepted a DPoP-bound or mTLS-bound token as a plain bearer
  token, so the sender constraint did not protect it. IAM logs the refusal at `info` with the
  issuer and the marker (SMA-690).
```

- [ ] **Step 3: Check the text**

Run: `grep -n "SMA-690" docs/ops/RUNBOOK-chart.md rs/crates/services/paigasus-iam/CHANGELOG.md`
Expected: one hit in each file.
Run: `grep -c "<!-- ci-targets:begin -->" CLAUDE.md` — Expected: `1` (this task must not touch `CLAUDE.md`).

- [ ] **Step 4: Commit**

```bash
git add docs/ops/RUNBOOK-chart.md rs/crates/services/paigasus-iam/CHANGELOG.md
git commit -m "docs(rs): state that IAM refuses a sender-constrained token (SMA-690)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Prove that the tests bite (mutations A and B)

**Files:**
- Modify (scratch only, restored before the task ends): `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` (the step-7 block from Task 1)
- Create: `docs/superpowers/plans/2026-09-26-sma-690-mutation-results.md` (the recorded results)

**Interfaces:**
- Consumes: the step-7 block and the tests from Tasks 1 and 2.
- Produces: a results file that the PR body cites.

Restore each mutation with an edit, NOT with `git checkout` or `git restore`.

- [ ] **Step 1: Mutation A (the check)**

Change the step-7 line
`if let Some(marker) = sender_constraint_marker(&token_data.claims) {`
to
`if let Some(marker) = sender_constraint_marker(&token_data.claims).filter(|_| false) {`

Run: `cd rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast 2>&1 | tail -40`
Expected: it compiles. RED: `refuses_sender_constrained_tokens`, `sender_constrained_refusal_logs_issuer_and_cnf_marker_only`, `dpop_typ_refusal_logs_canonical_marker`, `repeated_sender_constrained_refusals_log_once`. GREEN: `accepts_null_cnf`, `accepts_non_marker_typ_values`, `id_token_with_cnf_reports_not_an_access_token`, `expired_bound_token_reports_expired`, `sender_constraint_marker_names_the_marker`, `invalid_token_is_401_with_bearer_challenge`, `every_authn_status_carries_a_registered_reason_and_its_original_message`, and every SMA-686 test.

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test keycloak_e2e --no-fail-fast 2>&1 | tail -40`
Expected: RED at the `SenderConstrained` `resolve` assertion ("a DPoP-bound token must not authenticate" or "must be refused as SenderConstrained"). Record the panic message and line. The two 401 assertions after it do not run; record that this run does not prove them.

Restore the line with an edit.

- [ ] **Step 2: Mutation B (the log)**

In the step-7 block, replace
`self.log_refusal(&issuer, TokenDefect::SenderConstrained, RefusalDetail::Binding(marker));`
with
`let _ = RefusalDetail::Binding(marker);`

This keeps the `Binding` variant constructed, so it compiles under `warnings = "deny"`. (`let _ = marker;` alone does NOT compile: the variant is then never constructed, and that is a dead-code error.)

Run: `cd rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast 2>&1 | tail -40`
Expected: it compiles. RED: `sender_constrained_refusal_logs_issuer_and_cnf_marker_only`, `dpop_typ_refusal_logs_canonical_marker`, `repeated_sender_constrained_refusals_log_once`. GREEN: `refuses_sender_constrained_tokens` and every other test.

Restore the line with an edit.

- [ ] **Step 3: Verify the restore**

Run: `git diff --stat -- rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs`
Expected: no output (the file equals the Task 1 commit).
Run: `cd rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test keycloak_e2e`
Expected: PASS.

- [ ] **Step 4: Record and commit the results**

Write `docs/superpowers/plans/2026-09-26-sma-690-mutation-results.md` with: the two mutations as exact diffs, the RED and GREEN test names for each run, the e2e panic message and line, and the restore check output. Write it in Simplified Technical English. Record only what you observed.

```bash
git add docs/superpowers/plans/2026-09-26-sma-690-mutation-results.md
git commit -m "docs(rs): record the SMA-690 mutation results

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## After the tasks (controller)

1. Run the full gate graph from the root `CLAUDE.md` (`moon ci … --base origin/main --include-relations`). Read gate failures that come from the wrong local bash as the root `CLAUDE.md` describes, not as findings.
2. Create the follow-up Linear issue (spec § 4.5) and record its key in the spec § 8 R1.
3. Stage 5 (local review), Stage 6 (PR), Stage 7 (CodeRabbit loop).
