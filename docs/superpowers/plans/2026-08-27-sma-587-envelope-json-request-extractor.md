# SMA-587 — `EnvelopeJson` at every IAM request extractor: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make all fourteen IAM HTTP routes that take a plain `axum::Json<T>` answer a refused request body inside the stable `{"error":{code,message}}` envelope with a registered, kind-specific `reason`.

**Architecture:** The existing `EnvelopeJson<T>` extractor moves from `adapters::http::authn` to a neutral `adapters::http::json`, its two-code rejection taxonomy grows to four (adding `unsupported-content-type` and `invalid-request-schema` to the canonical registry), and the fourteen handler signatures swap extractor. A new `repo:http-extractor-envelope` CI gate stops a fifteenth site landing. Prose and assertions the change invalidates are retired in the same branch.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), axum 0.8, prost/tonic 0.14, protobuf via buf, Moon 2.3.2 task graph, Python 3 for CI gates, `cargo nextest` with Docker-backed integration suites.

**Spec:** `docs/superpowers/specs/2026-08-27-sma-587-envelope-json-request-extractor-design.md`

## Global Constraints

- **Worktree:** all work happens in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-587` on branch `feature/sma-587-iam-envelope-json-malformed-body`. Never `git checkout` in the main checkout — a peer session is live there.
- **PATH:** every shell command must be prefixed with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` or moon/uv/buf/nextest resolve to the wrong (or no) binary. Shims first.
- **SPDX header:** every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python).
- **Rust edition:** 2024, rust-version 1.95. Workspace lints are `warnings = deny`, so dead code is a hard compile error on a lib target — never stage an unused item "to wire up later".
- **Commits:** conventional, workspace-scoped, subject **starts lowercase** and is **≤100 chars**. No `#NNN` issue refs in the body (breaks `footer-leading-blank`); write "owner/repo PR NNN". Never `--no-verify`.
- **Registry vocabulary — the two new wire codes, verbatim:** `unsupported-content-type` (415), `invalid-request-schema` (422). Existing, unchanged: `invalid-request-body` (400), `request-too-large` (413).
- **Registry field numbers:** `ERROR_REASON_UNSUPPORTED_CONTENT_TYPE = 905;`, `ERROR_REASON_INVALID_REQUEST_SCHEMA = 906;`
- **Registry count anchor:** `rs/crates/libs/paigasus-proto/src/error.rs` currently asserts `52`; it becomes `54`.
- **Docker:** the integration suites need a reachable daemon. Run them with `env -u CI` so the presence-based `CI` check does not turn skips into panics unexpectedly, and never set `PAIGASUS_SKIP_DOCKER=1` in a shell profile. A `moon run` greened under that flag leaves a cached PASS — follow with `--force`.
- **Codegen is by hand.** `contracts:generate` has no `outputs:` and can serve stale cached output. After any `.proto` edit run `(cd contracts && buf format -w && buf generate)` directly and commit the regenerated bindings.

---

## File Structure

**Created**
- `rs/crates/services/paigasus-iam/src/adapters/http/json.rs` — the `EnvelopeJson` extractor, the four-arm `RejectionKind`, the pure `classify`, `envelope_rejection`, and their unit tests. One responsibility: turn an axum body rejection into the house envelope.
- `ci/http-extractor/check.py` — the banned-extractor scan (signature parsing + verdict + self-tests).
- `ci/http-extractor/README.md` — what is gated, what is not, and the residuals.

**Modified**
- `contracts/proto/paigasus/common/v1/error.proto` — two additive values; 901's comment reworded.
- `rs/crates/libs/paigasus-proto/src/error.rs` — `EXPECTED_REASONS` +2, count anchor 52→54.
- `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs` — loses the extractor; keeps `AuthnApiError`; its membership test narrows.
- `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` — declares `json`.
- The fourteen handler files (§ Task 4).
- `rs/crates/services/paigasus-iam/src/adapters/http/api_keys.rs`, `system_retirement.rs` — repoint `EnvelopeJson` imports.
- `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` — retire the stale comment, assert the HTTP half, add divergence rows.
- `rs/crates/services/paigasus-iam/tests/support/mod.rs` — one new raw-body helper.
- Six `tests/http_*.rs` suites — the coverage tables.
- `rs/crates/services/paigasus-iam/tests/http_authn.rs` — the 415 expectation is reassigned.
- `ci/error-registry/check.py` — `MANIFEST` gains a `json.rs` row; `authn.rs`'s `why` narrows.
- `moon.yml`, `.github/workflows/ci.yml`, `CLAUDE.md` — the new gate's plumbing.

---

## Task 1: Add the two registry reasons

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/error.proto:205-215`
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs` (`EXPECTED_REASONS` tail, and the count anchor at `:224`)
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/**`, `py/**/generated/**`, `ts/**/generated/**`

**Interfaces:**
- Consumes: nothing.
- Produces: `ErrorReason::UnsupportedContentType` and `ErrorReason::InvalidRequestSchema`, resolvable via `ErrorReason::from_wire_reason("unsupported-content-type")` / `("invalid-request-schema")`. Task 2 depends on both.

- [ ] **Step 1: Update the count anchor first, so the test fails for the right reason**

In `rs/crates/libs/paigasus-proto/src/error.rs`, change the anchor:

```rust
        assert_eq!(actual.len(), 54, "the registry should hold 54 reasons");
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-proto -E 'test(the_registry_contains_exactly_the_expected_reasons)'
```

Expected: FAIL — `assertion left == right failed: the registry should hold 54 reasons`, left `52`, right `54`.

- [ ] **Step 3: Add the two values to the proto registry**

In `contracts/proto/paigasus/common/v1/error.proto`, immediately after the
`ERROR_REASON_CAPABILITY_DISABLED = 904;` block and before the closing `}` of `enum ErrorReason`:

```proto
  // "unsupported-content-type" — the request declared a Content-Type the endpoint does not
  // accept, so the body was never read. HTTP-only, structurally: tonic negotiates
  // `application/grpc` at the transport layer, so a gRPC client cannot present a wrong
  // content type to a handler.
  ERROR_REASON_UNSUPPORTED_CONTENT_TYPE = 905;
  // "invalid-request-schema" — the body was syntactically valid JSON but did not match the
  // target type. HTTP-only, structurally: proto3 decoding has no "syntactically valid but
  // schema-invalid" state, since unknown fields are skipped by design.
  ERROR_REASON_INVALID_REQUEST_SCHEMA = 906;
```

- [ ] **Step 4: Reword 901's comment, which this narrowing makes untrue**

Replace the existing two comment lines above `ERROR_REASON_INVALID_REQUEST_BODY = 901;`:

```proto
  // "invalid-request-body" — the request body could not be read or deserialized. NOTE the two
  // services scope this differently since SMA-587: IAM's HTTP extractor emits it only for a
  // MALFORMED body (a JSON syntax error, or a body that failed to buffer), having split the
  // wrong-content-type and schema-mismatch cases out to 905 and 906; the gateway's own funnel
  // (`adapters/http/error.rs`) still emits it for ANY deserialization failure. Reconverging the
  // two is a follow-up, not an accident.
```

- [ ] **Step 5: Add both codes to the hand-transcribed mirror**

In `rs/crates/libs/paigasus-proto/src/error.rs`, at the end of the `// Shared` group in
`EXPECTED_REASONS`, after `"capability-disabled",`:

```rust
        "unsupported-content-type",
        "invalid-request-schema",
```

- [ ] **Step 6: Format and regenerate the bindings by hand**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf format -w && buf generate
```

Do not rely on `moon run contracts:generate` — it has no `outputs:` and can serve stale cached output.

- [ ] **Step 7: Run the registry tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-proto
```

Expected: PASS, including `the_registry_contains_exactly_the_expected_reasons` and the
proto/mirror cross-check.

- [ ] **Step 8: Verify the new codes resolve**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-proto -E 'test(from_wire_reason)'
```

Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add contracts/ rs/crates/libs/paigasus-proto/ py/ ts/
git commit -m "feat(contracts): add unsupported-content-type and invalid-request-schema reasons (SMA-587)"
```

---

## Task 2: Move the extractor to `json.rs` and grow its taxonomy

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/http/json.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs` (remove the extractor; narrow the membership test)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:34` area (declare the module)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/api_keys.rs:132`, `system_retirement.rs:96,97,195,199,215,233,257` (repoint imports)
- Modify: `rs/crates/services/paigasus-iam/tests/http_authn.rs:162-173` (the 415 expectation is reassigned)
- Modify: `ci/error-registry/check.py` (`MANIFEST`)

**Interfaces:**
- Consumes: `ErrorReason::UnsupportedContentType`, `ErrorReason::InvalidRequestSchema` (Task 1).
- Produces:
  - `pub(crate) struct EnvelopeJson<T>(pub(crate) T)` at `crate::adapters::http::json::EnvelopeJson`, with `FromRequest` and `OptionalFromRequest` impls — Task 4 uses it in fourteen signatures.
  - `fn classify(status: StatusCode) -> Option<RejectionKind>` — private, unit-tested in-module.
  - Test `every_request_extractor_code_is_in_the_registry` — named in `check.py`'s MANIFEST.

- [ ] **Step 1: Write the failing test for the four-arm taxonomy**

Create `rs/crates/services/paigasus-iam/src/adapters/http/json.rs` containing **only** the SPDX
header and this test module, so the test names the API before it exists:

```rust
// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    /// D1.1a: `classify` is the status-only half of the rule, extracted so the fallback is
    /// reachable at all. The `match` in `envelope_rejection` is NOT — axum's rejections have
    /// `pub(crate)` constructors and `#[non_exhaustive]` enums, so no `BytesRejection` can be
    /// built outside axum. Without this function the fallback would be untestable.
    #[test]
    fn classify_maps_each_client_error_and_refuses_server_errors() {
        assert_eq!(classify(StatusCode::BAD_REQUEST), Some(RejectionKind::Invalid));
        assert_eq!(classify(StatusCode::PAYLOAD_TOO_LARGE), Some(RejectionKind::TooLarge));
        assert_eq!(classify(StatusCode::UNSUPPORTED_MEDIA_TYPE), Some(RejectionKind::UnsupportedContentType));
        assert_eq!(classify(StatusCode::UNPROCESSABLE_ENTITY), Some(RejectionKind::InvalidSchema));
        // Any other CLIENT error is still the caller's problem, so it stays in the envelope.
        assert_eq!(classify(StatusCode::CONFLICT), Some(RejectionKind::Invalid));
        // A SERVER error is OUR mistake. Answering it with a 4xx-flavoured code would report it
        // as the caller's — the exact inversion `path.rs:11-17` refuses. `None` means "hand
        // axum's own response back untouched".
        assert_eq!(classify(StatusCode::INTERNAL_SERVER_ERROR), None);
        assert_eq!(classify(StatusCode::BAD_GATEWAY), None);
    }

    /// Every code this module can put on the wire is in the canonical registry. Driven off
    /// `strum::EnumIter` rather than restated literals — the SMA-507 E3 lesson: a hand-copied
    /// list lets a new arm escape both this test and `repo:error-code-single-site`.
    #[test]
    fn every_request_extractor_code_is_in_the_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        use strum::IntoEnumIterator;

        let codes: Vec<&'static str> = RejectionKind::iter().map(|kind| kind.parts().0).collect();
        assert_eq!(codes.len(), 4, "all four kinds must be enumerated, or this asserts less than it claims");
        for code in codes {
            assert!(ErrorReason::from_wire_reason(code).is_some(), "{code} is not declared in common/v1/error.proto");
        }
    }
}
```

- [ ] **Step 2: Declare the module so the test compiles into the crate**

In `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`, add alongside the other extractor
module declaration (keep alphabetical order among the `mod` lines; `path` is at `:34`):

```rust
/// `pub(crate)` for the same reason `path` is: the extractor is used by handler modules across
/// this adapter, and `grpc::convert`'s transport parity guard drives it directly (SMA-587 D6).
pub(crate) mod json;
```

- [ ] **Step 3: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(json::tests)'
```

Expected: FAIL to compile — `cannot find function classify in this scope`, `cannot find type RejectionKind in this scope`.

- [ ] **Step 4: Write the module body**

Insert above the `#[cfg(test)] mod tests` block in `json.rs`:

```rust
//! The house JSON request extractor: `EnvelopeJson<T>` answers a refused body inside IAM's
//! stable `{"error":{code,message}}` envelope with a registered reason, instead of letting
//! axum's plain-text rejection escape the error contract (SMA-587).
//!
//! Sibling of `path.rs` by design — one extractor module per input kind, neither owned by a
//! handler module. It differs from `path.rs` on one axis deliberately (spec D2.1): `path.rs`
//! renders through `ApiError(TenancyError::…)`, while this module builds the envelope by hand
//! from literals. It must, because `EnvelopeJson` also serves `api_keys::introspect`, whose
//! every other failure is an `AuthnApiError` — a funnel deliberately separate from
//! `ApiError`/`TenancyError` (`authn.rs` module docs). An extractor emitting a `TenancyError`
//! there would make a route's error type depend on WHERE in the request it failed. The cost is
//! that this file carries code literals and therefore sits on
//! `ci/error-registry/check.py`'s MANIFEST; the mitigation is that the membership test
//! enumerates `RejectionKind` via `strum::EnumIter` rather than restating them.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, OptionalFromRequest, Request};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Every `(code, message)` pair this module can put on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(strum::EnumIter))]
pub(crate) enum RejectionKind {
    /// The body exceeded the configured byte limit.
    TooLarge,
    /// The body could not be read, or was not syntactically valid JSON.
    Invalid,
    /// The request declared a `Content-Type` this endpoint does not accept.
    UnsupportedContentType,
    /// Syntactically valid JSON that did not match the target type.
    InvalidSchema,
}

impl RejectionKind {
    /// This kind's canonical registry code and its static, caller-safe message.
    fn parts(self) -> (&'static str, &'static str) {
        match self {
            // Narrowed by SMA-587 to MALFORMED-or-unreadable only; the wrong-content-type and
            // schema-mismatch cases used to share this code and now have their own.
            RejectionKind::Invalid => ("invalid-request-body", "invalid request body"),
            RejectionKind::TooLarge => ("request-too-large", "request body too large"),
            RejectionKind::UnsupportedContentType => ("unsupported-content-type", "unsupported content type"),
            RejectionKind::InvalidSchema => ("invalid-request-schema", "request body did not match the expected schema"),
        }
    }
}

/// The status-only half of the classification rule (spec D1.1).
///
/// `None` means "this is not the caller's mistake" — the caller gets axum's own response rather
/// than a 4xx-flavoured code on a 5xx status. `path.rs:87-92` makes the identical choice for
/// `PathRejection`'s server-bug family, and two extractors answering server bugs differently
/// would be worse than one plain-text 500.
fn classify(status: StatusCode) -> Option<RejectionKind> {
    match status {
        StatusCode::PAYLOAD_TOO_LARGE => Some(RejectionKind::TooLarge),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => Some(RejectionKind::UnsupportedContentType),
        StatusCode::UNPROCESSABLE_ENTITY => Some(RejectionKind::InvalidSchema),
        s if s.is_client_error() => Some(RejectionKind::Invalid),
        _ => None,
    }
}

/// Renders one kind into the envelope, preserving the rejection's OWN status — no route's
/// status changes anywhere in SMA-587.
fn envelope(kind: RejectionKind, status: StatusCode) -> Response {
    let (code, message) = kind.parts();
    let mut response = (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response();
    response.headers_mut().insert(
        paigasus_observability::correlation::RETRYABLE_HEADER,
        HeaderValue::from_static(paigasus_observability::Retryable::No.as_wire()),
    );
    response
}

/// Maps a `JsonRejection` into the stable envelope — shared by both extraction paths below so
/// the two can never drift on status, code or shape.
///
/// The rule is hybrid on purpose (spec D1.1): match the VARIANT where the variant determines the
/// status, dispatch on STATUS everywhere else. A variant-only match would be wrong, because
/// `JsonRejection::BytesRejection` wraps `FailedToBufferBody`, itself
/// `{LengthLimitError (413), UnknownBodyError (400)}` — so mapping that variant straight to
/// `request-too-large` would render a 413 code on a 400 response. And `JsonRejection` is
/// `#[non_exhaustive]`, so the fallback arm is mandatory rather than optional.
fn envelope_rejection(rejection: JsonRejection) -> Response {
    let status = rejection.status();
    match &rejection {
        JsonRejection::JsonSyntaxError(_) => envelope(RejectionKind::Invalid, status),
        JsonRejection::MissingJsonContentType(_) => envelope(RejectionKind::UnsupportedContentType, status),
        JsonRejection::JsonDataError(_) => envelope(RejectionKind::InvalidSchema, status),
        _ => match classify(status) {
            Some(kind) => envelope(kind, status),
            None => rejection.into_response(),
        },
    }
}

/// `Json<T>` with the IAM error envelope on rejection: axum's default plain-text rejections
/// (malformed JSON, wrong content-type, schema mismatch, oversized body) become the same
/// `{"error":{code,message}}` shape every other IAM response uses. The status is the
/// rejection's own; messages are static — nothing ever echoes the request body.
///
/// This is the house extractor for EVERY request body on this adapter (SMA-587). A handler
/// taking a bare `axum::Json` in request position is a bug, and `repo:http-extractor-envelope`
/// fails the build on one.
#[derive(Debug)]
pub(crate) struct EnvelopeJson<T>(pub(crate) T);

impl<S, T> FromRequest<S> for EnvelopeJson<T>
where
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(EnvelopeJson(value)),
            Err(rejection) => Err(envelope_rejection(rejection)),
        }
    }
}

/// `Option<EnvelopeJson<T>>` support (SMA-481): mirrors axum's own `Json<T>:
/// OptionalFromRequest` impl exactly for the "is there a body at all" question — no
/// `Content-Type` header means `Ok(None)`, never an attempt to parse zero bytes as JSON — but a
/// body that DOES declare `Content-Type: application/json` and fails to parse still gets the
/// same envelope the required impl above produces.
impl<S, T> OptionalFromRequest<S> for EnvelopeJson<T>
where
    Json<T>: OptionalFromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Option<Self>, Self::Rejection> {
        match <Json<T> as OptionalFromRequest<S>>::from_request(req, state).await {
            Ok(Some(Json(value))) => Ok(Some(EnvelopeJson(value))),
            Ok(None) => Ok(None),
            Err(rejection) => Err(envelope_rejection(rejection)),
        }
    }
}
```

- [ ] **Step 5: Run the test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(json::tests)'
```

Expected: PASS, 2 tests.

- [ ] **Step 6: Move the extractor's behavioural tests over from `authn.rs`**

Cut these three tests out of `authn.rs`'s `mod tests` and paste them into `json.rs`'s `mod tests`,
renaming `Probe` along with them (they exercise the extractor, which now lives here):
`optional_envelope_json_yields_none_when_no_content_type_is_present`,
`optional_envelope_json_maps_a_malformed_body_to_the_stable_envelope`,
`optional_envelope_json_extracts_some_for_a_well_formed_body`.

They need these additional imports at the top of `json.rs`'s test module:

```rust
    use axum::body::{Body, to_bytes};
    use axum::extract::Request;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, Deserialize)]
    struct Probe {
        x: i32,
    }
```

- [ ] **Step 7: Add the two new-arm behavioural tests and the 413 round-trip**

Append to `json.rs`'s `mod tests`:

```rust
    /// The 415 arm, end to end through the extractor: a declared content type the endpoint does
    /// not accept is refused before the body is read, and answers in the envelope.
    #[tokio::test]
    async fn a_wrong_content_type_is_unsupported_content_type() {
        let req = Request::builder().method("POST").uri("/").header("content-type", "text/plain").body(Body::from("{}")).unwrap();
        let rejection = <EnvelopeJson<Probe> as FromRequest<()>>::from_request(req, &()).await.expect_err("a wrong content type must be rejected");
        assert_eq!(rejection.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let bytes = to_bytes(rejection.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], json!("unsupported-content-type"));
    }

    /// The 422 arm: syntactically valid JSON that does not match the target type. Distinct from
    /// the 400 syntax case above — before SMA-587 both answered `invalid-request-body`.
    #[tokio::test]
    async fn a_schema_mismatch_is_invalid_request_schema() {
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"x": "not an integer"}"#))
            .unwrap();
        let rejection = <EnvelopeJson<Probe> as FromRequest<()>>::from_request(req, &()).await.expect_err("a schema mismatch must be rejected");
        assert_eq!(rejection.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = to_bytes(rejection.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], json!("invalid-request-schema"));
    }

    /// The 413 arm, which `classify` alone cannot prove reachable: a genuine `LengthLimitError`
    /// only exists behind a real body limit, and `BytesRejection` cannot be constructed outside
    /// axum. Driving a router with `DefaultBodyLimit` is the only way to produce one.
    #[tokio::test]
    async fn an_oversized_body_is_request_too_large() {
        use axum::Router;
        use axum::extract::DefaultBodyLimit;
        use axum::routing::post;
        use tower::ServiceExt;

        async fn probe(EnvelopeJson(_): EnvelopeJson<Probe>) -> StatusCode {
            StatusCode::OK
        }
        let app = Router::new().route("/", post(probe)).layer(DefaultBodyLimit::max(8));

        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"x": 123456789}"#))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], json!("request-too-large"));
    }
```

- [ ] **Step 8: Delete the extractor from `authn.rs` and narrow its membership test**

In `authn.rs`: delete `EnvelopeJson`, `RejectionKind`, `envelope_rejection`, both impls, and the
three extractor tests moved in Step 6. Then in `every_authn_http_code_is_in_the_registry`, delete
the `RejectionKind::iter()` seeding and its `assert!(!codes.is_empty(), …)` line, so the test
drives `all_authn_errors()` only, and start `codes` empty:

```rust
        let mut codes: Vec<String> = Vec::new();
```

Update its doc comment: the paragraph explaining the `RejectionKind` half now belongs to
`json.rs`; what remains is the `AuthnError` half. Keep the `use` list minimal — the workspace
denies warnings, so a now-unused import is a hard error. `authn.rs` still needs `JsonRejection`
removed from its imports and `OptionalFromRequest`/`FromRequest`/`Request` dropped if unused.

- [ ] **Step 9: Repoint the two existing importers**

In `api_keys.rs:132` and `system_retirement.rs` (7 sites), change
`use super::authn::EnvelopeJson;` / `super::authn::EnvelopeJson` references to
`super::json::EnvelopeJson`. Compile errors will name each one.

- [ ] **Step 10: Reassign the superseded 415 assertion**

`tests/http_authn.rs:162-173`'s `introspect_wrong_content_type_is_enveloped` asserts
`invalid-request-body` on a 415. That expectation is superseded, not deleted — it is the
assertion that pins the 415 path:

```rust
    assert_eq!(body["error"]["code"], "unsupported-content-type");
```

Add a line above it noting the reassignment:

```rust
    // SMA-587 split this out of `invalid-request-body`: a wrong content type means the body was
    // never read, which is not the same failure as a body that could not be parsed.
```

Leave the 400 case immediately above unchanged — `invalid-request-body` still means malformed
syntax — and leave `system_retirement.rs:241,262` (both 400) untouched.

- [ ] **Step 11: Register `json.rs` on the error-registry manifest**

In `ci/error-registry/check.py`'s `MANIFEST`, add after the `authn.rs` row:

```python
    ("rs/crates/services/paigasus-iam/src/adapters/http/json.rs", "emits",
     "every_request_extractor_code_is_in_the_registry", "RejectionKind::parts() — the request-body extractor's four codes"),
```

and narrow the now-stale `authn.rs` row's `why`, since `envelope_rejection` no longer lives there:

```python
    ("rs/crates/services/paigasus-iam/src/adapters/http/authn.rs", "emits",
     "every_authn_http_code_is_in_the_registry", "the authn funnel"),
```

- [ ] **Step 12: Run the full unit suite and the registry gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib
cd .. && python3 ci/error-registry/check.py --self-test && python3 ci/error-registry/check.py --single-site
```

Expected: all PASS.

- [ ] **Step 13: Run the affected integration suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && env -u CI cargo nextest run -p paigasus-iam --test http_authn
```

Expected: PASS (needs Docker; a skip here proves nothing — check the daemon).

- [ ] **Step 14: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/http/ rs/crates/services/paigasus-iam/tests/http_authn.rs ci/error-registry/check.py
git commit -m "refactor(rs): move EnvelopeJson to http::json and split its rejection taxonomy (SMA-587)"
```

---

## Task 3: A test that proves the fourteen routes are still broken

This task exists so Task 4 has a red test to turn green — the plan's only "write the failing test"
step that spans a whole feature rather than one function.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/http_tenancy.rs`

**Interfaces:**
- Consumes: `EnvelopeJson` (Task 2), `send_raw_parts` (existing, `support/mod.rs:525`).
- Produces: `support::send_bytes(app, method, uri, content_type, body, token) -> (StatusCode, Value)` — Task 5 uses it in all six suites.

- [ ] **Step 1: Add the raw-body helper**

`support::send` takes a `serde_json::Value`, so it *cannot* send malformed JSON. Add to
`rs/crates/services/paigasus-iam/tests/support/mod.rs`, next to `send`:

```rust
/// Drives one request with a RAW body through the router and returns `(status, json body)` —
/// for tests that must send bytes `serde_json::Value` cannot represent (malformed JSON), or a
/// deliberate wrong `Content-Type`. `send` cannot: it serializes a `Value`, which is always
/// valid JSON. An empty response body yields `Value::Null`.
#[allow(dead_code)]
pub async fn send_bytes(app: &Router, method: &str, uri: &str, content_type: Option<&str>, body: &[u8], token: Option<&str>) -> (StatusCode, Value) {
    let authorization = token.map(|token| format!("Bearer {token}"));
    let response = send_raw_parts(app, method, uri, authorization.as_deref(), content_type, Some(body.to_vec())).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, value)
}
```

- [ ] **Step 2: Write the failing test on the real router**

Append to `rs/crates/services/paigasus-iam/tests/http_tenancy.rs`. Six routes, each × two kinds:

```rust
/// SMA-587, end-to-end on REAL routes: a refused request body answers inside the
/// `{"error":{code,message}}` envelope with a kind-specific reason.
///
/// Driven against the merged `router(...)` rather than a synthetic one for the reason SMA-586
/// learned expensively: a synthetic route proves the EXTRACTOR, never the handler wiring, and
/// that is exactly how a mis-named `{sa}` path segment survived its whole suite. Each row here
/// pins one live route's extractor choice.
#[tokio::test]
async fn a_refused_body_answers_in_the_error_envelope() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("body-user", Some("body@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    // Seed a real org/team/project so the rename routes reach their extractor rather than 404.
    let (_, created) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "envelope", "name": "Envelope"})), Some(token.as_str())).await;
    let org_id = created["organization"]["prn"].as_str().expect("organization.prn").rsplit('/').next().unwrap().to_string();
    let team_id = created["default_team"]["prn"].as_str().expect("default_team.prn").rsplit('/').next().unwrap().to_string();
    let (_, project) = send(&app, "POST", &format!("/v1/teams/{team_id}/projects"), Some(json!({"slug": "p1", "name": "P1"})), Some(token.as_str())).await;
    let project_id = project["prn"].as_str().expect("project.prn").rsplit('/').next().unwrap().to_string();

    // (method, uri) for every tenancy route that takes a body.
    let routes: Vec<(&str, String)> = vec![
        ("POST", "/v1/organizations".to_string()),
        ("PATCH", format!("/v1/organizations/{org_id}")),
        ("POST", format!("/v1/organizations/{org_id}/teams")),
        ("PATCH", format!("/v1/teams/{team_id}")),
        ("POST", format!("/v1/teams/{team_id}/projects")),
        ("PATCH", format!("/v1/projects/{project_id}")),
    ];

    for (method, uri) in &routes {
        // 400: not JSON at all.
        let (status, err) = support::send_bytes(&app, method, uri, Some("application/json"), b"{not json", Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}: {err}");
        assert_eq!(err["error"]["code"], "invalid-request-body", "{method} {uri}: {err}");

        // 422: valid JSON, wrong shape. `RenameBody`'s fields are all `Option<String>` with no
        // `deny_unknown_fields`, so `{}` would DESERIALIZE and reach the handler — a type
        // mismatch is what actually reaches `JsonDataError` on the PATCH routes.
        let (status, err) = support::send_bytes(&app, method, uri, Some("application/json"), br#"{"slug": 1, "name": 2}"#, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{method} {uri}: {err}");
        assert_eq!(err["error"]["code"], "invalid-request-schema", "{method} {uri}: {err}");
    }

    // 415 is an extractor-level fact, refused before any handler-specific code runs, so it is
    // asserted ONCE here rather than per route (the `json.rs` unit test covers the extractor;
    // this proves it is reachable on a real route at all).
    let (status, err) = support::send_bytes(&app, "POST", "/v1/organizations", Some("text/plain"), b"{}", Some(token.as_str())).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{err}");
    assert_eq!(err["error"]["code"], "unsupported-content-type", "{err}");

    // A well-formed body on the same route still reaches the handler — so every row above is an
    // assertion about the BODY's shape, not about the route being broken.
    let (status, _) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "still-works", "name": "Still Works"})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED);
}
```

- [ ] **Step 3: Run it to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && env -u CI cargo nextest run -p paigasus-iam --test http_tenancy -E 'test(a_refused_body_answers_in_the_error_envelope)'
```

Expected: FAIL. The body will be axum's plain text, so `serde_json::from_slice` in `send_bytes`
panics (`expected value at line 1`) before any assertion runs. That panic **is** the failure —
it is the defect stated as a test: the route's answer is not JSON at all.

- [ ] **Step 4: Commit the red test**

```bash
git add rs/crates/services/paigasus-iam/tests/
git commit -m "test(rs): pin the refused-body envelope contract on the tenancy routes (SMA-587)"
```

---

## Task 4: Swap the fourteen extractors

**Files:**
- Modify: `api_keys.rs:82`, `authz.rs:85,107,137`, `dead_letters.rs:140`, `memberships.rs:68`, `organizations.rs:52,80,114`, `projects.rs:41`, `service_accounts.rs:63`, `teams.rs:48,78`, `users.rs:59` — all under `rs/crates/services/paigasus-iam/src/adapters/http/`

**Interfaces:**
- Consumes: `EnvelopeJson` (Task 2), the red test (Task 3).
- Produces: fourteen routes whose refused bodies are enveloped. Task 5 extends coverage; Task 6 gates it.

- [ ] **Step 1: Swap every request-position `Json` in the fourteen signatures**

For each site, replace the binding and its type, leaving everything else — including every
return-position `Json<Dto>` — untouched. `Json` is `FromRequest` and is already the LAST parameter
in all fourteen, so no reordering is needed. Example (`organizations.rs:52`):

```rust
-async fn create_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Json(b): Json<CreateNodeBody>) -> Result<(StatusCode, Json<CreateOrgResponse>), ApiError> {
+async fn create_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, EnvelopeJson(b): EnvelopeJson<CreateNodeBody>) -> Result<(StatusCode, Json<CreateOrgResponse>), ApiError> {
```

Note the two binding styles in the tree — `Json(body):` and `Json(b):` — and the multi-line
signatures at `api_keys.rs:82` and `organizations.rs:114`. Add
`use super::json::EnvelopeJson;` to each of the ten files that does not already import it, and
**remove `Json` from a file's imports only if no return type still uses it** (most still do; the
workspace denies warnings, so an unused import is a hard error either way).

- [ ] **Step 2: Verify the swap is complete by counting**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs/crates/services/paigasus-iam/src/adapters/http
grep -rn "Json(\w*): Json<" *.rs | wc -l
```

Expected: `0`.

- [ ] **Step 3: Build**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam
```

Expected: clean, no warnings (they are denied).

- [ ] **Step 4: Run the red test to verify it now passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && env -u CI cargo nextest run -p paigasus-iam --test http_tenancy
```

Expected: PASS, including `a_refused_body_answers_in_the_error_envelope`.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/http/
git commit -m "fix(rs): answer a refused request body in the error envelope on fourteen routes (SMA-587)"
```

---

## Task 5: Extend coverage to the remaining five suites

**Files:**
- Modify: `tests/http_users.rs`, `tests/http_memberships.rs`, `tests/http_authz.rs`, `tests/http_dead_letters.rs`, `tests/http_service_accounts.rs`

**Interfaces:**
- Consumes: `support::send_bytes` (Task 3), the swapped routes (Task 4).
- Produces: nothing downstream.

- [ ] **Step 1: Add the table to each suite**

In each of the five files, append a test following the Task 3 shape exactly — same name
(`a_refused_body_answers_in_the_error_envelope`, one per suite, they are separate binaries), same
two assertions per route, same trailing well-formed-body control. The per-suite route tables:

```
http_users.rs             POST   /v1/users
http_memberships.rs       POST   /v1/memberships
http_authz.rs             POST   /v1/authz/is-authorized
                          POST   /v1/authz/policies
                          POST   /v1/authz/role-grants
http_dead_letters.rs      POST   /v1/outbox/dead-letters/replay
http_service_accounts.rs  POST   /v1/service-accounts
                          POST   /v1/service-accounts/{sa}/api-keys
```

Confirm each URI against that module's `router()` before writing the row — do not copy the paths
above on trust.

Omit the 415 assertion in these five: it is asserted once in `http_tenancy.rs` and once as a
`json.rs` unit test, and it cannot vary per route.

- [ ] **Step 2: Enable the two capability-gated route groups**

`put_policy` and `create_role_grant` live in `authz::admin_router()`, mounted only when
`caps.authz_admin` (`mod.rs:871-873`); the api-key `issue` route lives in `api_keys::router()`,
mounted only when `caps.apikeys_management` (`mod.rs:875-877`). A disabled capability **404s the
route**, which would make the row pass vacuously against the wrong failure. Check how the
existing tests in `http_authz.rs` and `http_service_accounts.rs` build their app and reuse that
configuration; if they use a helper that leaves a capability off, enable it explicitly for these
rows.

- [ ] **Step 3: Run each suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && env -u CI cargo nextest run -p paigasus-iam --test http_users --test http_memberships --test http_authz --test http_dead_letters --test http_service_accounts
```

Expected: PASS. A `0 skipped` line is **not** proof these ran — confirm the daemon is up, or set
`PAIGASUS_REQUIRE_DOCKER=1` to turn a skip into a panic (needed for any filtered run, since the
`docker_preflight` canary is not in the filter).

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/
git commit -m "test(rs): cover the refused-body envelope on the remaining five http suites (SMA-587)"
```

---

## Task 6: Retire the prose this change invalidates

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:1171-1199`

**Interfaces:**
- Consumes: the swapped routes (Task 4).
- Produces: nothing downstream.

- [ ] **Step 1: Read the current comment and its surrounding test**

```bash
sed -n '1160,1205p' rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs
```

The twelve-line comment inside `the_recorded_transport_divergences_still_hold` says `issue`'s
`Json<IssueApiKeyBody>` "is axum's plain `Json`, not `http::authn::EnvelopeJson`, so a malformed
body never reaches the IAM error envelope or the registry at all… Flagged for a follow-up ticket
rather than fixed here." **This ticket is that follow-up.** After Task 4 the claim is false three
ways: the extractor changed, the module path changed, and the HTTP half is now assertable.

- [ ] **Step 2: Rewrite the comment and assert the HTTP half**

Replace the twelve-line paragraph with a statement of the resolved position, and add the HTTP-half
assertion the old comment said was impossible — driving `EnvelopeJson<IssueApiKeyBody>` with
`{"expires_at":"not-a-timestamp"}`, which is valid JSON that fails to deserialize into
`DateTime<Utc>`, i.e. a `JsonDataError` → 422 `invalid-request-schema`. Follow the file's existing
assertion style for the gRPC half directly above it.

Note the reasons genuinely differ across transports here and that is correct, not a defect: gRPC
yields `invalid-timestamp` (a `prost_types::Timestamp` that fails to CONVERT), HTTP yields
`invalid-request-schema` (a string that fails to DESERIALIZE). They are different failures at
different layers. State that in the comment rather than leaving a reader to assume drift.

- [ ] **Step 3: Add the two HTTP-only divergence rows**

In the same test, add rows recording that `unsupported-content-type` and
`invalid-request-schema` are HTTP-only **structurally** — tonic negotiates `application/grpc` at
the transport, and proto3 decoding has no schema-invalid state. Mirror how the existing
`MutuallyExclusiveFields` row (`convert.rs:1193-1198`) states its own structural asymmetry.

- [ ] **Step 4: Run the test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam -E 'test(the_recorded_transport_divergences_still_hold)'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs
git commit -m "test(rs): close the http half of the api-key body divergence (SMA-587)"
```

---

## Task 7: The `repo:http-extractor-envelope` gate

**Files:**
- Create: `ci/http-extractor/check.py`, `ci/http-extractor/README.md`
- Modify: `moon.yml` (a new task on the root `repo` project)
- Modify: `.github/workflows/ci.yml:214` (the `T=(…)` array)
- Modify: `CLAUDE.md` (the marker-delimited command)

**Interfaces:**
- Consumes: a tree with zero bare request-position `Json` (Task 4).
- Produces: `python3 ci/http-extractor/check.py [--self-test | --check]`, exit 0 clean / 1 violation / 2 broken harness.

- [ ] **Step 1: Write the checker with its self-tests**

Create `ci/http-extractor/check.py`. Model it on `ci/error-registry/check.py`: a `# SPDX` header,
an `InfraError` mapped to rc 2 so a broken checker aborts loudly rather than folding into a green,
a `--self-test` mode driven by fixture tables, and a `--check` mode that scans the tree.

The scan, per spec D4.1 — a naive line grep fails in four directions, all present in this tree:

1. Extract each `fn` **signature** by paren-balancing from `fn <name>(` to its matching `)`.
   Multi-line signatures (`api_keys.rs:82`, `organizations.rs:114`) fall out of this for free.
2. Cut at the top-level `->`. This is what separates `organizations.rs:80`'s
   `… Json(b): Json<RenameBody>) -> Result<Json<OrgDto>, ApiError>` — one banned binding and one
   legal return type on one line — which no line-oriented scan can do.
3. Scan **only the parameter span** for each banned type name. The rule is *the type appears in
   the parameter span*, not *a `Json(x):` binding pattern appears*: the house style already uses
   the non-destructuring form for extractors (`system_retirement.rs:96`
   `body: Option<EnvelopeJson<RetireBody>>`, `organizations.rs:71` `path: UuidPath<…>`), so
   `body: Json<CreateNodeBody>` must be caught too.
4. `where` clauses are a third context, neither parameter list nor return type. Cutting at the
   top-level `->` already excludes a normal `fn`'s `where` clause; the extractor's own impl-block
   bodies are handled by the ALLOW row in Step 2.

Carry a **banned-extractor table**, one row per extractor type with an explicit on/off flag, so
closing the `Query`/`Path<String>` instances later flips a flag rather than designing a second
gate:

```python
# (type name, enabled, required replacement)
BANNED = (
    ("Json", True, "EnvelopeJson"),
    # Reserved rows — the same class of hole, different extractor, deliberately not closed by
    # SMA-587 (see its spec's Out of scope). Ten `Query<…>` bindings and two `Path<String>` still
    # answer outside the envelope. Their replacement is the follow-up's call, not this gate's.
    ("Query", False, None),
    ("Path", False, None),
)

SCAN_GLOB = "rs/crates/services/*/src/adapters/http/**/*.rs"
```

Self-test fixtures must include, at minimum: the single-line both-contexts case, a multi-line
signature, the non-destructuring `body: Json<T>` form, a legal return-only `-> Result<Json<Dto>, _>`,
and a `where Json<T>: FromRequest<…>` clause. A **planted-violation** case is what proves the gate
reds — the self-test is where it lives, since this gate is not script-pinned and carries no
separate `--negative-control` (`repo:error-code-single-site`'s shape, `moon.yml:647-650`).

- [ ] **Step 2: Add the ALLOW table — it is NOT empty at merge**

```python
# Files permitted to name a banned extractor in a parameter span. Each row states why.
ALLOW = (
    ("rs/crates/services/paigasus-iam/src/adapters/http/json.rs",
     "the extractor's own definition site — it wraps `axum::Json` by construction"),
)
```

- [ ] **Step 3: Run the self-test to verify it fails before the checker is right**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/http-extractor/check.py --self-test
```

Iterate until every fixture case passes, including the planted violation.

- [ ] **Step 4: Run the real check against the tree**

```bash
python3 ci/http-extractor/check.py --check
```

Expected: exit 0, zero violations (Task 4 removed them all).

- [ ] **Step 5: Prove it reds on a real regression**

Temporarily revert one signature — `users.rs:59` back to `Json(b): Json<CreateUserBody>` — and
re-run `--check`. Expected: exit 1 naming that file and line. Then restore it.

**Restore by `git checkout -- <file>`, not by re-editing**, and be aware of the cargo mtime trap:
restoring a file with an older mtime lets cargo reuse the binary built from the temporary edit.
`touch` the file after restoring if you then run any cargo command.

- [ ] **Step 6: Write the README's Limitations section**

Create `ci/http-extractor/README.md` stating what is gated, and — per `ci/actionlint/README.md`'s
precedent — the residuals it cannot see: an aliased import (`use axum::Json as J`), a re-export
renamed on the way through, and bodies taken as `Bytes`/`String`. Naming them is the point;
a gate that fails open is worse than no gate.

*(This step originally predicted a fourth residual — a fully-qualified `axum::Json<T>`. It is not
one: the shipped scan matches on an identifier boundary, so `axum::Json<T>` and any other
`…::Json<T>` are caught. Corrected here so the plan, the design doc and the shipped README state
one limitation contract rather than three.)*

- [ ] **Step 7: Add the Moon task**

In `moon.yml`, on the root `repo` project, following `error-code-single-site`'s shape
(`moon.yml:626-655`) — **not** the `release-parity*` family, which is script-pinned for different
reasons:

```yaml
  http-extractor-envelope:
    description: 'Assert no HTTP handler takes a bare `axum::Json` in request position, so a refused body cannot answer outside the IAM error envelope (SMA-587).'
    # `--self-test` runs FIRST and in the SAME script block: a rotted checker must red rather
    # than ship green. `set -euo pipefail` is REQUIRED — Moon does not enable errexit for
    # `script:` blocks, so without it a failing --self-test would be masked by a passing --check.
    script: |
      set -euo pipefail
      python3 ci/http-extractor/check.py --self-test
      python3 ci/http-extractor/check.py --check
    toolchain: 'system'
    inputs:
      - 'rs/crates/services/*/src/adapters/http/**/*.rs'
      - 'ci/http-extractor/**/*'
```

Every declared input must match at least one **tracked** file or `repo:input-liveness` reds.

- [ ] **Step 8: Add the task to BOTH coverage lists — this pair is mandatory**

`ci/affected-graph/ci_targets.py` asserts the two agree, and `moon ci` exits **0** on a target
that resolves to nothing, so a typo is otherwise a silent no-op on every PR.

In `.github/workflows/ci.yml:214`, add `:http-extractor-envelope` to the `T=(…)` array. It must
stay a **single-line bash array**.

In `CLAUDE.md`, add the same target inside the `<!-- ci-targets:begin -->` /
`<!-- ci-targets:end -->` markers. **Do not remove or quote the markers, and do not create a
second copy of either marker anywhere in the file** — even inside backticks in prose, a second
copy makes the count 2 and reds `repo:affected-smoke`.

- [ ] **Step 9: Verify the coverage gates agree**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py
python3 ci/affected-graph/task_inputs.py
```

Expected: both PASS.

- [ ] **Step 10: Settle the open question the spec flagged — by running, not reasoning**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/affected-graph/run.sh
```

The spec's Verification section records this as unsettled: no `repo:*single-site` task appears in
`run.sh`'s expected action sets today, which *suggests* a new one keyed on
`rs/crates/services/*/src/adapters/http/**` changes nothing — but that has not been demonstrated.
If any case reds, re-baseline the expected set it names and say so in the commit message.

- [ ] **Step 11: Commit**

```bash
git add ci/http-extractor/ moon.yml .github/workflows/ci.yml CLAUDE.md ci/affected-graph/
git commit -m "ci: gate bare axum::Json in http request position (SMA-587)"
```

---

## Task 8: Whole-branch verification

**Files:** none — this task changes nothing unless it finds something.

- [ ] **Step 1: Run the full CI graph the way CI runs it**

A new `repo:*` gate and a `contracts/` change both reach well beyond per-project tasks, so
per-project `build/test/lint/fmt` is not sufficient evidence.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions \
  :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep \
  --base origin/main --include-relations
```

Expected: all green. Moon reports an unattributed "N failed" — diagnose with:

```bash
jq '.actions[]|select(.status=="failed")' .moon/cache/ciReport.json
```

- [ ] **Step 2: Confirm the Docker-backed suites actually ran**

A passing task proves nothing if the daemon was down — nextest discards a passing test's stderr
and Moon discards a passing task's output, so the skip is silent by design.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && env -u CI PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam
```

Expected: PASS with a real test count. `PAIGASUS_REQUIRE_DOCKER=1` turns every suite's skip into a
panic, so a green here cannot be a Docker-less false pass.

- [ ] **Step 3: Confirm no bare request-position `Json` survives**

```bash
cd rs/crates/services/paigasus-iam/src/adapters/http && grep -rn "Json(\w*): Json<" *.rs | wc -l
```

Expected: `0`.

- [ ] **Step 4: Check the diff against the plan**

```bash
git diff origin/main --stat
```

Confirm: no stray debug code, no `dbg!`/`println!`, no commented-out blocks, every file in the
§ File Structure list and nothing outside it.

- [ ] **Step 5: File the follow-up issue for the twelve remaining escapes**

The spec's Out of scope section promises it, and D4's reserved table rows reference it. Create a
Linear issue covering the ten `Query<…>` bindings (`audit.rs`, `authz.rs` ×2, `dead_letters.rs`,
`memberships.rs`, `organizations.rs` ×2, `service_accounts.rs`, `teams.rs`, `api_keys.rs`) and the
two `Path<String>` (`authz.rs`, `system_retirement.rs:96`), noting that
`ci/http-extractor/check.py`'s `BANNED` table already reserves rows for both and that the gateway's
own `invalid-request-body` scoping (spec D1.2) is the same issue's second half.

---

## Self-Review

**Spec coverage.** Every spec section maps to a task: D1 + Registry mechanics → Task 1; D1.1,
D1.1a, D1.3, D2, D2.1 → Task 2; D3 → Tasks 3–4; D4, D4.1, D4.2 → Task 7; D5 → Tasks 3, 5 and
Task 2 Step 7; D6 + Cross-transport divergence → Task 6; Tests that must change → Task 2 Step 10
(`http_authn.rs`), Task 1 Step 1 (count anchor), Task 6 (`convert.rs`); Out of scope → Task 8
Step 5; Verification → Task 8.

**Placeholders.** None. Every code step carries the actual code; the three steps that deliberately
do not (Task 5 Step 1's route table, Task 6 Step 2's rewrite, Task 7 Step 1's parser) each state
the exact inputs, the constraint to satisfy, and the file:line precedent to follow — and each is
guarded by a runnable expected-output check in the next step.

**Type consistency.** `RejectionKind`'s four variants (`TooLarge`, `Invalid`,
`UnsupportedContentType`, `InvalidSchema`) are spelled identically in Task 2 Steps 1 and 4.
`classify` returns `Option<RejectionKind>` in both its test and its definition.
`support::send_bytes`'s signature in Task 3 Step 1 matches its call sites in Task 3 Step 2 and
Task 5. `every_request_extractor_code_is_in_the_registry` is the same name in Task 2 Step 1 and
Task 2 Step 11's MANIFEST row. Wire codes match the Global Constraints block verbatim throughout.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
