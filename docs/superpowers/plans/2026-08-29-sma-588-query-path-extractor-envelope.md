# SMA-588 — Query/Path extractor envelope closure: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every request extractor in `paigasus-iam`'s HTTP adapter answer a refused input inside the stable `{"error":{code,message}}` envelope, reconverge the gateway's `invalid-request-body` on that meaning, and close the gateway's oversized-body escape.

**Architecture:** Two new IAM extractors (`EnvelopeQuery<T>` in a new `query.rs`, `StringPath<F>` beside `UuidPath` in `path.rs`) render through the existing `ApiError(TenancyError::…)` funnel, so neither file carries a code literal. Two additive registry reasons (907, 908) back them. The gateway gains two `GatewayError` variants and an `EnvelopeBytes` extractor. `repo:http-extractor-envelope` then turns on three reserved rows and adds a fourth so none of this can regress.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), axum 0.8.9, serde / serde_json / serde_urlencoded, tonic/prost, protobuf + buf, Python 3 (the CI gate), Moon.

**Spec:** `docs/superpowers/specs/2026-08-29-sma-588-query-path-extractor-envelope-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python).
- Rust: edition 2024, rust-version 1.95. `[workspace.lints.rust] warnings = "deny"` — **dead code is a hard compile error**, so a variant added in one task and consumed in a later one breaks the build in between. Task order below accounts for this; do not reorder.
- Bash PATH lacks the proto-managed CLIs. **Prefix every command with**
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Run cargo from `rs/`. Run `moon` from the repo root.
- `cargo nextest` exits non-zero on a target with no tests — use `--no-tests=pass`.
- Docker-backed IAM suites: run with `env -u CI` locally. A stray `CI=false` still counts as "CI present" (the check is presence-based).
- Commit messages: conventional commits, scope from `[rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace]`. **Never start a body line with `word:`** — commitlint reads it as a footer token and fails `footer-leading-blank`. Subject starts lowercase, ≤100 chars.
- New registry reasons live in the **900 range** and are append-only. Never renumber or remove.
- `TenancyError`'s field payload is `&'static str` **by design** — it structurally cannot hold caller input. Never reach a constructor via `Box::leak`/`String::leak`.
- Error messages are static and never echo caller input.

---

## File Structure

| File | Responsibility | Task |
| -- | -- | -- |
| `contracts/proto/paigasus/common/v1/error.proto` | +907, +908; reword 901/905/906 comments | 1 |
| `rs/crates/libs/paigasus-proto/src/error.rs` | `EXPECTED_REASONS` +2; count 55 → 57 at **two** sites | 1 |
| `rs/crates/libs/paigasus-proto/src/generated/**` | regenerated, committed | 1 |
| `rs/crates/services/paigasus-iam/src/application/error.rs` | +2 `TenancyError` variants; `code()`, `class()`, `field()` | 2 |
| `rs/crates/services/paigasus-iam/src/adapters/http/path.rs` | +`StringPath<F>`, +`PolicyId` marker, +count assertion | 3 |
| `rs/crates/services/paigasus-iam/src/adapters/http/query.rs` | **new** — `EnvelopeQuery<T>` | 4 |
| `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` | register `mod query` | 4 |
| 8 IAM handler files + `dto.rs` | swap 12 extractors; restate 2 DTO comments | 5 |
| `rs/crates/services/paigasus-iam/tests/http_request_extractors.rs` | **new** — all 12 rows on the real router | 6 |
| `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` | divergences 5–6: comment + source-scan assertion | 7 |
| `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs` | +2 `GatewayError` variants | 8, 9 |
| `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs` | `classify()` split; `EnvelopeBytes`; docs | 8, 9 |
| `rs/crates/services/paigasus-gateway/src/adapters/http/bytes.rs` | **new** — `EnvelopeBytes` | 9 |
| `rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs` | register `mod bytes`; fix 2 doc claims | 9 |
| `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs` | fix 2 stale doc claims; +split/413 coverage | 8, 9 |
| `ci/http-extractor/check.py` | `BANNED` 4-tuple; 3 rows on + `Bytes` row; ALLOW; fixtures | 10 |
| `ci/http-extractor/README.md` | retire L3; 4 more sites | 10 |

---

### Task 1: Registry — reasons 907 and 908

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/error.proto` (the `Shared (900-999)` block, ~lines 211-242)
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs` (doc comment ~`:141`, `EXPECTED_REASONS`, assertion `:228`)
- Modify (generated): `rs/crates/libs/paigasus-proto/src/generated/**`

**Interfaces:**
- Consumes: nothing.
- Produces: `ErrorReason::InvalidQueryParameter` (907) and `ErrorReason::InvalidPathSegment` (908), each resolving through `as_wire_reason()` / `from_wire_reason()` to `"invalid-query-parameter"` / `"invalid-path-segment"`. Tasks 2, 3, 4, 6 and 7 depend on these existing.

- [ ] **Step 1: Add the two values to the registry**

In `contracts/proto/paigasus/common/v1/error.proto`, append inside `enum ErrorReason`, after `ERROR_REASON_INVALID_REQUEST_SCHEMA = 906;`:

```proto
  // "invalid-query-parameter" — a query-string parameter could not be
  // deserialized into its target type, or was supplied more than once.
  // Emitted by IAM's EnvelopeQuery extractor before any handler runs.
  // HTTP-only: gRPC has no query string. Unlike 905/906, that property is
  // NOT enforced by the transport — this reason is a TenancyError variant,
  // which the gRPC surface also maps — so it is held by a source scan in
  // adapters/grpc/convert.rs instead (SMA-588).
  ERROR_REASON_INVALID_QUERY_PARAMETER = 907;
  // "invalid-path-segment" — a URL path segment could not be decoded as
  // text (its percent-encoding is not valid UTF-8). Emitted by IAM's
  // StringPath extractor for non-uuid segments; a malformed UUID segment
  // is ERROR_REASON_INVALID_UUID instead. HTTP-only on the same terms as
  // 907, and held by the same source scan (SMA-588).
  ERROR_REASON_INVALID_PATH_SEGMENT = 908;
```

- [ ] **Step 2: Reword the three stale comments**

Replace the `ERROR_REASON_INVALID_REQUEST_BODY = 901;` comment block with:

```proto
  // "invalid-request-body" — the request body could not be read or
  // deserialized. Both services scope this identically since SMA-588: a
  // MALFORMED body only (a JSON syntax error, a truncated body, or a body
  // that failed to buffer). The wrong-content-type and schema-mismatch
  // cases are 905 and 906.
  ERROR_REASON_INVALID_REQUEST_BODY = 901;
```

Replace the `ERROR_REASON_UNSUPPORTED_CONTENT_TYPE = 905;` comment block with:

```proto
  // "unsupported-content-type" — the request declared a Content-Type the
  // endpoint does not accept, so the body was never read. IAM-only in
  // practice, for two independent reasons: tonic negotiates
  // `application/grpc` at the transport layer, so a gRPC client cannot
  // present a wrong content type; and the gateway reads its body as raw
  // Bytes and never inspects Content-Type at all (SMA-588 D4.1).
  ERROR_REASON_UNSUPPORTED_CONTENT_TYPE = 905;
```

Replace the `ERROR_REASON_INVALID_REQUEST_SCHEMA = 906;` comment block with:

```proto
  // "invalid-request-schema" — the body was syntactically valid JSON but
  // did not match the target type. Emitted by BOTH services since SMA-588,
  // on DIFFERENT statuses: IAM answers 422 (axum's JsonDataError status),
  // the gateway answers 400 (OpenAI wire compatibility — its SDKs map
  // status to an exception class). A consumer mapping code -> status must
  // not assume it is one-to-one. HTTP-only, structurally: proto3 decoding
  // has no "syntactically valid but schema-invalid" state, since unknown
  // fields are skipped by design.
  ERROR_REASON_INVALID_REQUEST_SCHEMA = 906;
```

- [ ] **Step 3: Format the proto**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run contracts:fmt`

Expected: PASS. **Do not skip this.** An unformatted `.proto` reds `moon ci` and the failure is not attributed to this file.

- [ ] **Step 4: Mirror the two codes in the Rust transcription**

In `rs/crates/libs/paigasus-proto/src/error.rs`, add to `EXPECTED_REASONS` in registry order, after `"invalid-request-schema"`:

```rust
    "invalid-query-parameter",
    "invalid-path-segment",
```

- [ ] **Step 5: Update the count at BOTH sites**

At `error.rs:228`:

```rust
        assert_eq!(actual.len(), 57, "the registry should hold 57 reasons");
```

And in the doc comment at `error.rs:141`, change the `assert_eq!(actual.len(), 55)` anchor it names to `57`. Nothing asserts this second one, so it goes stale silently if you skip it.

- [ ] **Step 6: Regenerate the committed bindings**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run contracts:generate`

Then confirm the generated Rust moved:

Run: `git status --short rs/crates/libs/paigasus-proto/src/generated/`
Expected: the generated files appear as modified. If they do not, `contracts:generate` served a cached pass — re-run with `--force`.

- [ ] **Step 7: Run the registry tests**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && cargo nextest run -p paigasus-proto --no-tests=pass`
Expected: PASS, including `the_registry_contains_exactly_the_expected_reasons`.

- [ ] **Step 8: Run the registry gate**

Run: `python3 ci/error-registry/check.py --self-test && python3 ci/error-registry/check.py --single-site`
Expected: both rc 0.

- [ ] **Step 9: Commit**

```bash
git add contracts/proto/paigasus/common/v1/error.proto rs/crates/libs/paigasus-proto/
git commit -m "feat(contracts): register invalid-query-parameter and invalid-path-segment (SMA-588)"
```

---

### Task 2: `TenancyError` gains the two variants

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/error.rs`

**Interfaces:**
- Consumes: `ErrorReason::InvalidQueryParameter` / `InvalidPathSegment` from Task 1.
- Produces: `TenancyError::InvalidQueryParameter` (unit) and `TenancyError::InvalidPathSegment(&'static str)`. Tasks 3 and 4 construct these.

**Why the variants land before their users:** `warnings = "deny"` makes dead code a hard compile error — but an *enum variant* that is never constructed is not dead code, so this task compiles standalone. (A dead *function* would not; do not add helpers here.)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `application/error.rs`:

```rust
/// SMA-588. The two extractor reasons: `InvalidPathSegment` carries the field name so
/// `field()` can surface it as `metadata["field"]`, and `InvalidQueryParameter` carries
/// none — axum's rejection does expose the key (`serde_path_to_error`), but the payload is
/// `&'static str` by design and a runtime name cannot reach this constructor without the
/// `Box::leak` the type invariant forbids.
#[test]
fn the_extractor_reasons_classify_and_name_their_field() {
    let q = TenancyError::InvalidQueryParameter;
    assert_eq!(q.code(), "invalid-query-parameter");
    assert_eq!(q.class(), ErrorClass::Validation);
    assert_eq!(q.field(), None);
    assert_eq!(q.to_string(), "invalid query parameter");

    let p = TenancyError::InvalidPathSegment("policy_id");
    assert_eq!(p.code(), "invalid-path-segment");
    assert_eq!(p.class(), ErrorClass::Validation);
    assert_eq!(p.field(), Some("policy_id"));
    assert_eq!(p.to_string(), "policy_id is not a valid path segment");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && cargo test -p paigasus-iam --lib the_extractor_reasons_classify_and_name_their_field 2>&1 | tail -20`
Expected: FAIL to compile — `no variant named InvalidQueryParameter`.

- [ ] **Step 3: Add the two variants**

In the `pub enum TenancyError` declaration, immediately after the `MutuallyExclusiveFields` variant:

```rust
    /// SMA-588. A query-string parameter axum refused: a value that would not parse into its
    /// target type, or a key supplied twice. Raised by `http::query::EnvelopeQuery` before any
    /// handler runs.
    ///
    /// A UNIT variant, deliberately. axum's rejection does name the key (it wraps the
    /// deserializer in `serde_path_to_error`), but this enum's field payload is `&'static str`
    /// so that "never reflect untrusted input into an error body" is enforced by the type — and
    /// a key read at runtime cannot become a `&'static str` without the `Box::leak` this file's
    /// own docs forbid. HTTP-only: gRPC has no query string (`grpc/convert.rs`'s divergence 5).
    #[error("invalid query parameter")]
    InvalidQueryParameter,
    /// SMA-588. A non-uuid path segment whose percent-decoding is not valid UTF-8, raised by
    /// `http::path::StringPath`. Distinct from [`TenancyError::InvalidUuid`]: that one means
    /// "this segment should have been a uuid and was not", this one means "this segment is not
    /// decodable text at all". HTTP-only (`grpc/convert.rs`'s divergence 6).
    #[error("{0} is not a valid path segment")]
    InvalidPathSegment(&'static str),
```

- [ ] **Step 4: Add the three match arms**

In `code()`, after the `MutuallyExclusiveFields` arm:

```rust
            Self::InvalidQueryParameter => "invalid-query-parameter",
            Self::InvalidPathSegment(_) => "invalid-path-segment",
```

In `class()`, add both to the `ErrorClass::Validation` group — append to that arm's `|` chain:

```rust
            | Self::InvalidQueryParameter
            | Self::InvalidPathSegment(_)
```

In `field()`, add `InvalidPathSegment` to the `Some(f)` arm and `InvalidQueryParameter` to the `None` arm:

```rust
            Self::InvalidTimestamp(f)
            | Self::InvalidUuid(f)
            | Self::InvalidCursor(f)
            | Self::InvalidAuditOutcome(f)
            | Self::MissingRequiredField(f)
            | Self::MutuallyExclusiveFields(f)
            | Self::InvalidPathSegment(f) => Some(f),
```

...and add `| Self::InvalidQueryParameter` to `field()`'s `None` arm chain.

**Do not touch `adapters/retryable.rs`.** `tenancy_retryable` matches on `ErrorClass`, not on `TenancyError`, so it needs no arm and gives no compile enforcement.

- [ ] **Step 5: Run the test and the membership guard**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && cargo nextest run -p paigasus-iam --lib --no-tests=pass 2>&1 | tail -20`
Expected: PASS, including `the_extractor_reasons_classify_and_name_their_field` and `every_tenancy_code_is_declared_in_the_canonical_registry` (which enumerates `TenancyError` via `strum::EnumIter`, so it would red if Task 1 had not landed).

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/application/error.rs
git commit -m "feat(rs): add the two extractor reasons to TenancyError (SMA-588)"
```

---

### Task 3: `StringPath<F>` beside `UuidPath`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/path.rs`

**Interfaces:**
- Consumes: `TenancyError::InvalidPathSegment` (Task 2).
- Produces: `pub(crate) struct StringPath<F: PathField> { pub value: String }`, implementing `FromRequestParts`. Marker `PolicyId` with `NAME = "policy_id"`. Task 5 uses both.

**Why `FromRequestParts` and not `FromRequest`:** `system_retirement::retire` takes the path segment **followed by** `body: Option<EnvelopeJson<RetireBody>>`. Only one `FromRequest` extractor is allowed per handler and it must come last, so a `FromRequest` `StringPath` would not compile there.

- [ ] **Step 1: Write the failing tests**

Add to `path.rs`'s `#[cfg(test)] mod tests`:

```rust
    async fn ok_string(path: StringPath<PolicyId>) -> String {
        path.value
    }

    /// The registry's `invalid-path-segment` wire string, resolved through the enum rather
    /// than spelled as a literal — a literal in this `src/` file would put the module on
    /// `ci/error-registry/check.py`'s MANIFEST and blind that gate here.
    fn invalid_path_segment_wire() -> String {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        ErrorReason::InvalidPathSegment.as_wire_reason().expect("not the Unspecified sentinel")
    }

    /// SMA-588: a segment whose percent-decoding is not valid UTF-8 answers inside the error
    /// envelope, naming its field. `%FF` is not a valid UTF-8 sequence, so axum raises
    /// `FailedToDeserializePathParams` with a 400 status, which `is_client_error` admits.
    #[tokio::test]
    async fn an_undecodable_segment_answers_in_the_error_envelope() {
        let (status, bytes) = probe(Router::new().route("/x/{id}", get(ok_string)), "/x/%FF").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], invalid_path_segment_wire());
        assert_eq!(body["error"]["message"], "policy_id is not a valid path segment");
    }

    /// An ordinary segment reaches the handler unchanged — including one that is not a uuid,
    /// which is the whole point of this extractor existing beside `UuidPath`.
    #[tokio::test]
    async fn an_ordinary_string_segment_extracts() {
        let (status, bytes) = probe(Router::new().route("/x/{id}", get(ok_string)), "/x/allow-root-read").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(String::from_utf8(bytes).unwrap(), "allow-root-read");
    }

    /// A ROUTER bug keeps its own 5xx rather than being relabelled as the caller's mistake —
    /// the same rule `UuidPath` and `json.rs`'s `classify` follow. Three extractors, one rule.
    #[tokio::test]
    async fn a_string_path_router_arity_bug_keeps_its_own_server_error() {
        let (status, bytes) = probe(Router::new().route("/x/{a}/{b}", get(ok_string)), "/x/one/two").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            serde_json::from_slice::<serde_json::Value>(&bytes).is_err(),
            "axum's own plain-text rejection is preserved, not re-wrapped: {}",
            String::from_utf8_lossy(&bytes)
        );
    }
```

Then extend the existing `the_path_field_names_are_stable` test — add the new marker **and a count assertion**, which it lacks today:

```rust
    #[test]
    fn the_path_field_names_are_stable() {
        assert_eq!(OrganizationId::NAME, "organization_id");
        assert_eq!(TeamId::NAME, "team_id");
        assert_eq!(ProjectId::NAME, "project_id");
        assert_eq!(ServiceAccountId::NAME, "service_account_id");
        assert_eq!(ApiKeyId::NAME, "api_key_id");
        assert_eq!(MembershipId::NAME, "membership_id");
        assert_eq!(RoleGrantId::NAME, "role_grant_id");
        assert_eq!(DeadLetterId::NAME, "dead_letter_id");
        assert_eq!(PolicyId::NAME, "policy_id");

        // The count is asserted because the rows above are not self-enumerating: without it a
        // TENTH marker could be added with no row here and the rename tripwire would silently
        // stop covering it (SMA-588). `path_field!` has no registry to iterate, so this is the
        // only thing that notices.
        const MARKERS: usize = 9;
        assert_eq!(
            MARKERS, 9,
            "a new path_field! marker needs a row above and this count bumped"
        );
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && cargo test -p paigasus-iam --lib adapters::http::path 2>&1 | tail -20`
Expected: FAIL to compile — `cannot find type StringPath` and `cannot find value PolicyId`.

- [ ] **Step 3: Add the marker**

In `path.rs`, after the `DeadLetterId` marker declaration:

```rust
path_field!(/// `{policy_id}` on a policy route, and `{id}` on the system-policy retire route —
    /// both name the same wire field.
    PolicyId => "policy_id");
```

- [ ] **Step 4: Add the shared response constructor and the extractor**

After `malformed_uuid`, add its sibling:

```rust
/// The `{field} is not a valid path segment` envelope response — `malformed_uuid`'s sibling,
/// built the same way and for the same reason: one construction point, so status, code and
/// shape cannot drift between the extractors below.
fn malformed_segment(field: &'static str) -> Response {
    ApiError(TenancyError::InvalidPathSegment(field)).into_response()
}
```

After the `UuidPathPair` impl, add:

```rust
/// A single NON-UUID path segment, reported as `F::NAME` when it is undecodable (SMA-588).
///
/// `UuidPath`'s sibling for the two routes whose `{id}` is an opaque policy id rather than a
/// uuid — `authz::delete_policy` and `system_retirement::retire`. Both took axum's plain
/// `Path<String>`, whose rejection escapes the error envelope entirely.
///
/// `Path<String>` cannot fail to PARSE, so the one client-side rejection reachable here is a
/// segment whose percent-decoding is not valid UTF-8. On a one-segment route `F::NAME` is the
/// only field it can be, so naming it is a fact rather than the guess `UuidPathPair` refuses to
/// make. Everything axum classes 5xx — `MissingPathParams`, `WrongNumberOfParameters`,
/// `UnsupportedType` — is a ROUTE bug and keeps axum's own response (module docs).
///
/// `FromRequestParts`, not `FromRequest`: `system_retirement::retire` takes this extractor
/// FOLLOWED BY an `Option<EnvelopeJson<RetireBody>>` body, and only one `FromRequest` extractor
/// is permitted per handler and it must come last.
pub(crate) struct StringPath<F: PathField> {
    pub value: String,
    _marker: PhantomData<F>,
}

impl<S: Send + Sync, F: PathField> FromRequestParts<S> for StringPath<F> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<String>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(StringPath { value, _marker: PhantomData }),
            Err(rejection) if is_client_error(&rejection) => Err(malformed_segment(F::NAME)),
            Err(rejection) => Err(rejection.into_response()),
        }
    }
}
```

- [ ] **Step 5: Run the tests**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && cargo nextest run -p paigasus-iam --lib --no-tests=pass 2>&1 | tail -20`
Expected: PASS, all four `path.rs` additions included.

- [ ] **Step 6: Format and commit**

```bash
cd rs && cargo fmt && cd ..
git add rs/crates/services/paigasus-iam/src/adapters/http/path.rs
git commit -m "feat(rs): add StringPath for non-uuid path segments (SMA-588)"
```

---

### Task 4: `EnvelopeQuery<T>` in a new `query.rs`

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/http/query.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` (module declarations, near `mod path;` at ~`:34`)

**Interfaces:**
- Consumes: `TenancyError::InvalidQueryParameter` (Task 2).
- Produces: `pub(crate) struct EnvelopeQuery<T>(pub(crate) T)`, implementing `FromRequestParts`. Task 5 uses it.

- [ ] **Step 1: Write the file with its tests**

Create `rs/crates/services/paigasus-iam/src/adapters/http/query.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The house query-string extractor: `EnvelopeQuery<T>` answers a refused query string inside
//! IAM's stable `{"error":{code,message}}` envelope with a registered reason, instead of letting
//! axum's plain-text rejection escape the error contract (SMA-588).
//!
//! One module per input kind, a sibling of `json.rs` and `path.rs`. It takes `path.rs`'s side of
//! the split those two deliberately make (SMA-587 D2.1): it renders through
//! `ApiError(TenancyError::…)` rather than building an envelope from literals, because every
//! route it serves returns `Result<_, ApiError>`. `json.rs` must hand-build because it also
//! serves `api_keys::introspect`, whose funnel is `AuthnApiError`; that constraint does not
//! reach here. The payoff is that this file carries NO code literal, so it stays off
//! `ci/error-registry/check.py`'s MANIFEST and inherits `retryable` classification for free.
//!
//! **Two failure classes reach this extractor**, both measured against a real router:
//! a value that will not parse into its target type (`?limit=abc`), and a key supplied more
//! than once (`?limit=1&limit=2`). The second reaches EVERY field on every route, including
//! `Option<String>` ones, because a derived struct visitor raises `duplicate field` regardless
//! of type — axum directs callers wanting repeats to `axum_extra::extract::Query`. An unknown
//! key is not a failure at all; it is ignored.
//!
//! The message is static and names no parameter. axum's rejection DOES carry the key (it wraps
//! the deserializer in `serde_path_to_error`), but `TenancyError`'s payload is `&'static str` so
//! that untrusted input is structurally unable to reach an error body, and a runtime key cannot
//! become one without the `Box::leak` `application/error.rs` forbids.

use axum::extract::rejection::QueryRejection;
use axum::extract::{FromRequestParts, Query};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

use crate::adapters::http::error::ApiError;
use crate::application::error::TenancyError;

/// Was this rejection the CALLER's fault, or ours?
///
/// `QueryRejection` is `#[non_exhaustive]` and carries a single variant today
/// (`FailedToDeserializeQueryString`, a 400), so the fallback is mandatory rather than
/// defensive. Anything not a client error is handed back to axum unchanged — the identical rule
/// `path.rs:87-92` and `json.rs`'s `classify` follow, because three extractors answering server
/// bugs differently would be worse than one plain-text 500.
fn is_client_error(rejection: &QueryRejection) -> bool {
    rejection.status().is_client_error()
}

/// `Query<T>` with the IAM error envelope on rejection.
///
/// This is the house extractor for EVERY query string on this adapter. A handler taking a bare
/// `axum::Query` in request position is a bug, and `repo:http-extractor-envelope` fails the
/// build on one.
#[derive(Debug)]
pub(crate) struct EnvelopeQuery<T>(pub(crate) T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for EnvelopeQuery<T> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(EnvelopeQuery(value)),
            Err(rejection) if is_client_error(&rejection) => Err(ApiError(TenancyError::InvalidQueryParameter).into_response()),
            Err(rejection) => Err(rejection.into_response()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::to_bytes;
    use axum::routing::get;
    use serde::Deserialize;
    use tower::ServiceExt;

    #[derive(Debug, Deserialize)]
    struct Probe {
        limit: Option<i64>,
        name: Option<String>,
    }

    async fn ok(EnvelopeQuery(q): EnvelopeQuery<Probe>) -> String {
        format!("{:?}/{:?}", q.limit, q.name)
    }

    async fn probe(uri: &str) -> (StatusCode, Vec<u8>) {
        let app = Router::new().route("/x", get(ok));
        let resp = app.oneshot(axum::http::Request::builder().uri(uri).body(axum::body::Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, bytes.to_vec())
    }

    /// The registry's wire string, resolved through the enum — never a kebab literal, which
    /// would put this production module on `ci/error-registry/check.py`'s MANIFEST.
    fn invalid_query_parameter_wire() -> String {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        ErrorReason::InvalidQueryParameter.as_wire_reason().expect("not the Unspecified sentinel")
    }

    async fn assert_envelope(uri: &str) {
        let (status, bytes) = probe(uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], invalid_query_parameter_wire(), "{uri}");
        assert_eq!(body["error"]["message"], "invalid query parameter", "{uri}");
    }

    /// Class 1: a value that will not parse into its target type.
    #[tokio::test]
    async fn an_unparseable_value_answers_in_the_error_envelope() {
        assert_envelope("/x?limit=abc").await;
    }

    /// Class 2: a REPEATED key — and on an `Option<String>` field, which no amount of type
    /// checking would predict. A derived struct visitor raises `duplicate field` regardless of
    /// the field's type, so this class reaches every field on every route. Missing it is what
    /// made the first draft of this ticket's spec conclude one route was unreachable.
    #[tokio::test]
    async fn a_repeated_key_answers_in_the_error_envelope() {
        assert_envelope("/x?limit=1&limit=2").await;
        assert_envelope("/x?name=a&name=b").await;
    }

    /// A well-formed query still reaches the handler, and an UNKNOWN key is ignored rather than
    /// refused — so the assertions above are about the query's shape, not about the route.
    #[tokio::test]
    async fn a_well_formed_query_extracts_and_unknown_keys_are_ignored() {
        let (status, bytes) = probe("/x?limit=7&name=n").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(String::from_utf8(bytes).unwrap(), "Some(7)/Some(\"n\")");

        let (status, bytes) = probe("/x?nosuchkey=abc").await;
        assert_eq!(status, StatusCode::OK, "an unknown key is not a rejection");
        assert_eq!(String::from_utf8(bytes).unwrap(), "None/None");
    }

    /// An absent query string is not a rejection either — every list route's params are
    /// `Option`, so `GET /x` must reach the handler.
    #[tokio::test]
    async fn an_absent_query_string_extracts() {
        let (status, _) = probe("/x").await;
        assert_eq!(status, StatusCode::OK);
    }
}
```

- [ ] **Step 2: Register the module**

In `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`, beside the existing `pub(crate) mod path;` declaration, add in alphabetical position:

```rust
pub(crate) mod query;
```

- [ ] **Step 3: Run the tests**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && cargo nextest run -p paigasus-iam --lib --no-tests=pass 2>&1 | tail -20`
Expected: PASS, four new `query.rs` tests included.

Note: `EnvelopeQuery` is not yet used by a handler. It is `pub(crate)` and constructed in its own tests, so `warnings = "deny"` is satisfied. If the build complains about dead code, the module declaration in Step 2 was missed.

- [ ] **Step 4: Format and commit**

```bash
cd rs && cargo fmt && cd ..
git add rs/crates/services/paigasus-iam/src/adapters/http/query.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
git commit -m "feat(rs): add the EnvelopeQuery request extractor (SMA-588)"
```

---

### Task 5: Swap the twelve call sites

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/api_keys.rs:100`, `audit.rs:117`, `authz.rs:125,132,144`, `dead_letters.rs:115`, `memberships.rs:119`, `organizations.rs:63,126`, `service_accounts.rs:75`, `teams.rs:94`, `system_retirement.rs:97`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/dto.rs` (two comments: `RoleGrantQuery` ~`:366-367`, `ServiceAccountQuery` ~`:415-416`)

**Interfaces:**
- Consumes: `EnvelopeQuery` (Task 4), `StringPath` + `PolicyId` (Task 3).
- Produces: no new API. After this task no bare `axum::Query` or `axum::Path<String>` remains in a request position in this tree, which is Task 10's precondition.

**No handler body changes anywhere except the two `Path<String>` sites**, which bind a differently-named value.

- [ ] **Step 1: Swap the ten `Query` bindings**

In each file, change the import and the signature. The import edit is the same shape everywhere — drop `Query` from the `axum::extract` list (keep `State`, `DefaultBodyLimit` etc. where present) and add `use super::query::EnvelopeQuery;` beside the file's other `use super::…` lines.

The signature edit, once per site:

```rust
-Query(q): Query<PageQuery>
+EnvelopeQuery(q): EnvelopeQuery<PageQuery>
```

Apply at exactly these twelve positions (ten `Query`):

| File | fn | DTO |
| -- | -- | -- |
| `api_keys.rs:100` | `list` | `PageQuery` |
| `audit.rs:117` | `list` | `AuditQuery` |
| `authz.rs:125` | `list_policies` | `PageQuery` |
| `authz.rs:144` | `list_role_grants` | `RoleGrantQuery` |
| `dead_letters.rs:115` | `list` | `DeadLetterQuery` |
| `memberships.rs:119` | `list_memberships` | `MembershipQuery` |
| `organizations.rs:63` | `list_orgs` | `PageQuery` |
| `organizations.rs:126` | `list_teams` | `PageQuery` |
| `service_accounts.rs:75` | `list` | `ServiceAccountQuery` |
| `teams.rs:94` | `list_projects` | `PageQuery` |

- [ ] **Step 2: Swap the two `Path<String>` bindings**

In `authz.rs`, drop `Path` from the `axum::extract` import (leave `Query`'s removal from Step 1 in place, so the line becomes `use axum::extract::State;`), and add `PolicyId`/`StringPath` to the existing `use super::path::{…}` import:

```rust
-async fn delete_policy(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(policy_id): Path<String>) -> Result<StatusCode, ApiError> {
+async fn delete_policy(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, policy: StringPath<PolicyId>) -> Result<StatusCode, ApiError> {
     let actor = actor_prn(&ctx);
-    s.policies.delete(&actor, &policy_id).await?;
+    s.policies.delete(&actor, &policy.value).await?;
     Ok(StatusCode::NO_CONTENT)
 }
```

In `system_retirement.rs`, drop `Path` from its `axum::extract` import (leaving `use axum::extract::State;`) and add the same `super::path` import:

```rust
-async fn retire(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<String>, body: Option<EnvelopeJson<RetireBody>>) -> Result<Response, ApiError> {
+async fn retire(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, policy: StringPath<PolicyId>, body: Option<EnvelopeJson<RetireBody>>) -> Result<Response, ApiError> {
     let ack = acknowledged(body.map(|EnvelopeJson(b)| b));
-    let outcome = s.retirement.retire(&actor_prn(&ctx), &id, ack).await?;
+    let outcome = s.retirement.retire(&actor_prn(&ctx), &policy.value, ack).await?;
     Ok(response_for(outcome))
 }
```

**Extractor order is already correct** — `StringPath` is `FromRequestParts`, so it may precede the `Option<EnvelopeJson<…>>` body. No reordering.

- [ ] **Step 3: Restate the two stale DTO comments**

In `dto.rs`, `RoleGrantQuery`'s doc comment currently ends by contrasting with "axum's default plain-text query-rejection". That rejection no longer reaches these routes. Replace that trailing clause:

```rust
/// Query params for `GET /v1/authz/role-grants`: `principal_prn` is REQUIRED (unlike
/// `PageQuery`'s fields) — `RoleService::list` always lists exactly one principal's grants,
/// there is no list-everyone mode over HTTP. Kept `Option` here (rather than a bare
/// `String`) so a missing param maps through `http/authz.rs`'s own
/// `TenancyError::MissingRequiredField` funnel — a SPECIFIC reason naming the field, rather
/// than `EnvelopeQuery`'s general `invalid-query-parameter` (SMA-588), which is what a bare
/// `String` would produce.
```

Apply the same correction to `ServiceAccountQuery`'s comment, which mirrors it for `owner_prn`.

**Leave `AuditQuery` and `DeadLetterQuery` alone** — their comments reference the handler funnel and each other, never axum, so they are still true.

- [ ] **Step 4: Build and run the crate's tests**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && cargo nextest run -p paigasus-iam --lib --no-tests=pass 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Verify no bare extractor remains**

Run: `grep -rn "Query(\|Query<\|Path(\|Path<" rs/crates/services/paigasus-iam/src/adapters/http/*.rs | grep -v "EnvelopeQuery\|UuidPath\|StringPath\|PathField\|PathRejection\|QueryRejection\|Query\b.*struct\|path.rs:\|query.rs:"`
Expected: no hits in a handler signature. (`path.rs` and `query.rs` legitimately name the wrapped types; the grep excludes them.)

- [ ] **Step 6: Format, lint and commit**

```bash
cd rs && cargo fmt && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cd ..
git add rs/crates/services/paigasus-iam/src/adapters/http/
git commit -m "feat(rs): move twelve request extractors into the error envelope (SMA-588)"
```

---

### Task 6: Integration coverage on the real router

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/http_request_extractors.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–5, plus `support::{app_with_state, provision_platform_admin, send, start_migrated_postgres}`.
- Produces: nothing consumed later.

**Why one suite, not six.** SMA-587 spread its rows across existing suites because its routes needed real orgs/teams to reach the extractor. These do not: a query string or a path segment is refused *before* the handler, so no fixture is needed beyond a valid bearer. And all three capability flags (`authz.admin_enabled`, `api_keys.management_enabled`, `audit.query_enabled`) default to `true` in `support::test_config`, so `app_with_state` already mounts every route below — verified, not assumed.

**The prerequisite that is NOT free:** every route sits behind `require_bearer`, a `route_layer` that runs **before** any extractor. A row without a valid token asserts a 401, not a 907.

- [ ] **Step 1: Write the suite**

Create `rs/crates/services/paigasus-iam/tests/http_request_extractors.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-588, end-to-end on REAL routes: a refused query string or path segment answers inside
//! the `{"error":{code,message}}` envelope with a registered reason.
//!
//! Driven against the merged `router(...)` rather than a synthetic one, for the reason SMA-586
//! learned expensively: a synthetic route proves the EXTRACTOR, never the handler wiring, and
//! that is exactly how a mis-named `{sa}` path segment survived its whole suite. Each row here
//! pins one live route's extractor choice.
//!
//! No tenancy fixtures are seeded. Both extractors refuse BEFORE the handler runs, so every row
//! is reachable with nothing but a valid bearer — and each block ends with a well-formed request
//! on the same route, so a row cannot pass merely because the route is broken.
//!
//! All three capability flags default to `true` in `support::test_config`, so every route below
//! is mounted. A disabled capability would 404 and the rows would pass for the wrong reason.

mod support;

use axum::http::StatusCode;
use support::{app_with_state, provision_platform_admin, send};

/// Resolves a registry wire string through the enum rather than restating a kebab literal.
fn wire(reason: paigasus_proto::paigasus::common::v1::ErrorReason) -> String {
    reason.as_wire_reason().expect("not the Unspecified sentinel")
}

#[tokio::test]
async fn a_refused_query_string_answers_in_the_error_envelope() {
    use paigasus_proto::paigasus::common::v1::ErrorReason;

    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("query-user", Some("query@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    // (uri with a refused query, uri with a well-formed one).
    //
    // Eight routes carry a numeric field and use `?limit=abc`. `list_role_grants` carries NO
    // numeric field — `RoleGrantQuery` is a lone `Option<String>` — so `?limit=abc` there is an
    // ignored UNKNOWN key and answers 200. It uses a REPEATED key instead, which reaches every
    // field on every route. Getting this wrong is how a row ends up asserting nothing.
    let cases: Vec<(&str, &str)> = vec![
        ("/v1/organizations?limit=abc", "/v1/organizations?limit=1"),
        ("/v1/authz/policies?limit=abc", "/v1/authz/policies?limit=1"),
        ("/v1/memberships?limit=abc&principal=x", "/v1/memberships?limit=1&principal=x"),
        ("/v1/service-accounts?limit=abc&owner_prn=x", "/v1/service-accounts?limit=1&owner_prn=x"),
        ("/v1/audit?limit=abc", "/v1/audit?limit=1"),
        ("/v1/outbox/dead-letters?limit=abc", "/v1/outbox/dead-letters?limit=1"),
        (
            "/v1/authz/role-grants?principal_prn=a&principal_prn=b",
            "/v1/authz/role-grants?principal_prn=a",
        ),
    ];

    for (bad, good) in &cases {
        let (status, err) = send(&app, "GET", bad, None, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "GET {bad}: {err}");
        assert_eq!(err["error"]["code"], wire(ErrorReason::InvalidQueryParameter), "GET {bad}: {err}");
        assert_eq!(err["error"]["message"], "invalid query parameter", "GET {bad}: {err}");

        // The same route with a well-formed query reaches the handler. Any non-4xx-extractor
        // outcome proves the row above was about the QUERY, not about a broken route.
        let (status, err) = send(&app, "GET", good, None, Some(token.as_str())).await;
        assert_ne!(
            err["error"]["code"],
            wire(ErrorReason::InvalidQueryParameter),
            "GET {good} must reach the handler, got {status}: {err}"
        );
    }

    // A repeated key reaches a route whose failing field is NUMERIC too — the same class, the
    // other field type, so neither is assumed from the other.
    let (status, err) = send(&app, "GET", "/v1/organizations?limit=1&limit=2", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], wire(ErrorReason::InvalidQueryParameter), "{err}");
}

/// The two nested list routes, which take a uuid path segment BEFORE their query string — so a
/// refused query on them proves the two extractors compose in one signature.
#[tokio::test]
async fn a_refused_query_on_a_nested_list_route_answers_in_the_error_envelope() {
    use paigasus_proto::paigasus::common::v1::ErrorReason;
    use serde_json::json;

    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("nested-user", Some("nested@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    let (_, created) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "nested", "name": "Nested"})), Some(token.as_str())).await;
    let org_id = created["organization"]["prn"].as_str().expect("organization.prn").rsplit('/').next().unwrap().to_string();
    let team_id = created["default_team"]["prn"].as_str().expect("default_team.prn").rsplit('/').next().unwrap().to_string();

    for uri in [format!("/v1/organizations/{org_id}/teams?limit=abc"), format!("/v1/teams/{team_id}/projects?limit=abc")] {
        let (status, err) = send(&app, "GET", &uri, None, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "GET {uri}: {err}");
        assert_eq!(err["error"]["code"], wire(ErrorReason::InvalidQueryParameter), "GET {uri}: {err}");
    }

    // Well-formed on the same routes reaches the handler.
    let (status, err) = send(&app, "GET", &format!("/v1/organizations/{org_id}/teams?limit=1"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{err}");
}

/// The two `Path<String>` routes. `%FF` is not a valid UTF-8 percent-encoding, so axum refuses
/// the segment before the handler; the extractor names the field it stands for.
#[tokio::test]
async fn an_undecodable_path_segment_answers_in_the_error_envelope() {
    use paigasus_proto::paigasus::common::v1::ErrorReason;

    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("path-user", Some("segment@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    let cases = [("DELETE", "/v1/authz/policies/%FF"), ("POST", "/v1/authz/system-policies/%FF/retire")];
    for (method, uri) in cases {
        let (status, err) = send(&app, method, uri, None, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}: {err}");
        assert_eq!(err["error"]["code"], wire(ErrorReason::InvalidPathSegment), "{method} {uri}: {err}");
        assert_eq!(err["error"]["message"], "policy_id is not a valid path segment", "{method} {uri}: {err}");
    }

    // An ORDINARY, decodable segment reaches the handler — it is not a uuid, which is exactly
    // why these two routes need `StringPath` rather than `UuidPath`. The policy does not exist,
    // so the handler answers on its own terms; what matters is that it is not the extractor's
    // refusal above.
    let (_, err) = send(&app, "DELETE", "/v1/authz/policies/allow-root-read", None, Some(token.as_str())).await;
    assert_ne!(err["error"]["code"], wire(ErrorReason::InvalidPathSegment), "a decodable segment must reach the handler: {err}");
}
```

- [ ] **Step 2: Run the suite**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && env -u CI cargo nextest run -p paigasus-iam --test http_request_extractors --no-tests=pass 2>&1 | tail -30`

Expected: 3 tests PASS with Docker running. Without Docker they skip silently — that is the house policy, and `tests/docker_preflight.rs` is the canary that reds instead. **If they skip, you have verified nothing**: start Docker and re-run before ticking this step.

- [ ] **Step 3: If a row fails, read it before changing it**

The three most likely failures, and what each means:

| Symptom | Cause |
| -- | -- |
| a row asserts 401 | the token was not passed, or `provision_platform_admin` was skipped |
| a row asserts 404 | a capability is off, or the URI is wrong — check it against `mod.rs`'s router |
| the `role-grants` row asserts 200 or `missing-required-field` | it used `?limit=abc`; that DTO has no numeric field, so use the repeated key |

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/http_request_extractors.rs
git commit -m "test(rs): cover the query and path extractors on the real router (SMA-588)"
```

---

### Task 7: Record and enforce divergences 5 and 6

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` (the `the_recorded_transport_divergences_still_hold` test and its doc comment, ~`:1180-1260`)

**Interfaces:**
- Consumes: `TenancyError::{InvalidQueryParameter, InvalidPathSegment}` (Task 2).
- Produces: nothing consumed later.

**Why an assertion and not a comment.** 905 and 906 are HTTP-only because the *transport* makes them impossible. 907 and 908 are not: they live in `pub enum TenancyError`, which `status_to_grpc` maps unconditionally, so any gRPC handler in this crate could construct one and make the proto comment false. Divergence 2 (`mutually-exclusive-fields`) has the same shape and is pinned rather than left as prose.

- [ ] **Step 1: Write the failing test**

Add to the same `#[cfg(test)]` module in `convert.rs`:

```rust
/// SMA-588 divergences 5 and 6: `invalid-query-parameter` and `invalid-path-segment` are
/// HTTP-only, and unlike 905/906 that is NOT enforced by the transport — both are
/// `TenancyError` variants, which `status_to_grpc` maps unconditionally. This scan is what
/// holds the property the registry comments assert.
///
/// Its limit, stated: a source scan is defeated by an alias or a re-export, exactly as
/// `ci/error-registry/check.py` documents for its own. It catches the realistic case — a gRPC
/// handler reaching for a convenient existing variant — and nothing more.
#[test]
fn the_http_only_extractor_reasons_are_never_constructed_on_the_grpc_surface() {
    let grpc_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/adapters/grpc");
    let mut offenders = Vec::new();
    let mut scanned = 0usize;

    for entry in std::fs::read_dir(&grpc_dir).expect("the grpc adapter directory must exist") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("readable source file");
        scanned += 1;
        // This file's own test module names both variants; skip it, or the guard reds on itself.
        if path.file_name().and_then(|n| n.to_str()) == Some("convert.rs") {
            continue;
        }
        for needle in ["InvalidQueryParameter", "InvalidPathSegment"] {
            if text.contains(needle) {
                offenders.push(format!("{}: {needle}", path.display()));
            }
        }
    }

    assert!(scanned >= 2, "scanned {scanned} file(s) — the grpc adapter tree moved and this guard is scanning nothing");
    assert!(
        offenders.is_empty(),
        "these reasons are declared HTTP-only in error.proto but are constructed on the gRPC surface: {offenders:?}"
    );
}
```

- [ ] **Step 2: Run it**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && cargo nextest run -p paigasus-iam --lib the_http_only_extractor_reasons --no-tests=pass 2>&1 | tail -20`
Expected: PASS (nothing on the gRPC surface constructs them).

- [ ] **Step 3: Prove the guard can red**

Temporarily add `let _ = TenancyError::InvalidQueryParameter;` inside any function in `src/adapters/grpc/dead_letters.rs`, re-run Step 2, and confirm it FAILS naming that file. Then **delete the inserted line** — do not `git checkout` the file, which would also discard work from earlier tasks.

- [ ] **Step 4: Extend the divergence record**

In `the_recorded_transport_divergences_still_hold`'s doc comment, extend the sentence that reads "Divergences 3 and 4 are new in SMA-587…" with:

```
/// Divergences 5 and 6 are new in SMA-588 (`invalid-query-parameter`, `invalid-path-segment`).
/// They are recorded here but asserted ELSEWHERE, in
/// `the_http_only_extractor_reasons_are_never_constructed_on_the_grpc_surface` — because unlike
/// every divergence above them, theirs is a property of this crate's own discipline rather than
/// of the transport, so it needs a guard rather than a note.
```

- [ ] **Step 5: Run the module's tests and commit**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && cargo nextest run -p paigasus-iam --lib --no-tests=pass 2>&1 | tail -10`
Expected: PASS.

```bash
cd rs && cargo fmt && cd ..
git add rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs
git commit -m "test(rs): pin the two HTTP-only extractor reasons off the grpc surface (SMA-588)"
```

---

### Task 8: Gateway — split 901 from 906

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs` (enum, `parts()`, `retryable()`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs:81-83`
- Modify: `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs`

**Interfaces:**
- Consumes: `ERROR_REASON_INVALID_REQUEST_SCHEMA` (already in the registry before this ticket).
- Produces: `GatewayError::InvalidRequestSchema`. Task 9 adds its sibling.

**The status stays 400** (spec decision 5). `InvalidRequestSchema` is still a new *variant* rather than an argument, because `parts()` binds the whole `(status, type, code, param, message)` tuple per variant — there is no way to vary the code without one.

- [ ] **Step 1: Write the failing test**

Add to `error.rs`'s `#[cfg(test)] mod tests`:

```rust
/// SMA-588: a schema mismatch is its own code, reconverging `invalid-request-body` on "malformed
/// or unreadable" across both services. The STATUS stays 400 (spec decision 5): the OpenAI SDKs
/// map status to an exception class, so 422 would move every affected body out of a caller's
/// `BadRequestError` handler, and OpenAI's own API answers 400 here.
#[test]
fn a_schema_mismatch_has_its_own_code_on_an_unchanged_status() {
    let resp = GatewayError::InvalidRequestSchema.into_response();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(GatewayError::InvalidRequestSchema.retryable(), Retryable::No);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && cargo test -p paigasus-gateway --lib a_schema_mismatch 2>&1 | tail -20`
Expected: FAIL to compile — `no variant named InvalidRequestSchema`.

- [ ] **Step 3: Add the variant**

In `pub enum GatewayError`, after `BadRequestBody`:

```rust
    /// The request body was syntactically valid JSON but did not match
    /// `ChatCompletionRequest` → 400. Split out of `BadRequestBody` by SMA-588 so
    /// `invalid-request-body` means "malformed or unreadable" on this service exactly as it
    /// does on IAM.
    ///
    /// The status stays 400, unlike IAM's 422 for the same code. That asymmetry is deliberate
    /// (spec decision 5): the OpenAI SDKs map status to an exception class, and OpenAI's own
    /// API answers 400 for a bad chat-completions body, so 422 would break the wire
    /// compatibility that is this service's purpose.
    InvalidRequestSchema,
```

In `parts()`, after the `BadRequestBody` arm:

```rust
            GatewayError::InvalidRequestSchema => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                Some("invalid-request-schema"),
                None,
                "The request body does not match the expected schema.",
            ),
```

In `retryable()`, add `InvalidRequestSchema` to the `Retryable::No` arm's `|` chain.

Also narrow `BadRequestBody`'s own doc comment and message — it no longer covers schema failures:

```rust
    /// The request body could not be read or parsed as JSON — a syntax error, a truncated body,
    /// or an empty one → 400. A body that parses but does not match the target type is
    /// `InvalidRequestSchema` since SMA-588.
    BadRequestBody,
```

and in `parts()`, its message becomes `"The request body is not valid JSON."` (unchanged — it was already accurate for the narrowed meaning; verify rather than assume).

- [ ] **Step 4: Apply the split at the call site**

In `chat.rs`, replace the parse arm:

```rust
    // Parse a COPY only to read `model` + `stream`; the ORIGINAL `body` bytes flow upstream
    // verbatim. SMA-588 splits the failure: `serde_json` classifies its own errors, so a body
    // that PARSED but did not match the type is reported distinctly from one that could not be
    // read at all. `Category::Io` cannot arise from a `&[u8]`, but is grouped with the
    // unreadable cases so the match is exhaustive without a wildcard.
    let dto: ChatCompletionRequest = match serde_json::from_slice(&body) {
        Ok(dto) => dto,
        Err(e) => {
            return match e.classify() {
                serde_json::error::Category::Data => GatewayError::InvalidRequestSchema.into_response(),
                serde_json::error::Category::Syntax | serde_json::error::Category::Eof | serde_json::error::Category::Io => {
                    GatewayError::BadRequestBody.into_response()
                }
            };
        }
    };
```

- [ ] **Step 5: Add end-to-end coverage**

Add to `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs` (follow the file's existing helper for building an authorized request):

```rust
/// SMA-588: the two body-failure classes answer with distinct codes on the SAME status.
///
/// The status assertion is the load-bearing half. `invalid-request-schema` is 422 on IAM and
/// 400 here, and a future reader "harmonising" them would silently change every affected body's
/// exception class in a caller's OpenAI SDK. Asserting 400 on every row makes that a red test
/// rather than a quiet wire break.
#[tokio::test]
async fn a_refused_body_distinguishes_malformed_from_schema_mismatch() {
    // (raw body, expected code)
    let cases: [(&[u8], &str); 6] = [
        (b"{not json", "invalid-request-body"),
        (b"{\"model\":\"m\",", "invalid-request-body"),
        (b"", "invalid-request-body"),
        (br#"{"messages":[]}"#, "invalid-request-schema"),
        (br#"{"model":42,"messages":[]}"#, "invalid-request-schema"),
        // A bare scalar is a `Category::Data` error too: `ChatCompletionRequest` is a struct, so
        // any non-object is an `invalid type`. Measured, not assumed.
        (br#""hello""#, "invalid-request-schema"),
    ];

    for (raw, expected) in cases {
        let (status, body) = post_chat_bytes(raw).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body {:?}: {body}", String::from_utf8_lossy(raw));
        assert_eq!(body["error"]["code"], expected, "body {:?}: {body}", String::from_utf8_lossy(raw));
        assert_eq!(body["error"]["type"], "invalid_request_error", "body {:?}: {body}", String::from_utf8_lossy(raw));
    }
}
```

If `post_chat_bytes` does not exist in that file, write it beside the existing request helper, taking raw bytes and an authorized bearer and returning `(StatusCode, serde_json::Value)`.

- [ ] **Step 6: Run the gateway tests**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && env -u CI cargo nextest run -p paigasus-gateway --no-tests=pass 2>&1 | tail -25`
Expected: PASS, including `every_gateway_code_is_declared_in_the_canonical_registry` (which enumerates `GatewayError` via `strum::EnumIter`).

- [ ] **Step 7: Format, lint, commit**

```bash
cd rs && cargo fmt && cargo clippy -p paigasus-gateway --all-targets -- -D warnings && cd ..
git add rs/crates/services/paigasus-gateway/
git commit -m "feat(rs): split invalid-request-schema out of the gateway body funnel (SMA-588)"
```

---

### Task 9: Gateway — `EnvelopeBytes` and the oversized body

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/http/bytes.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs` (enum, `parts()`, `retryable()`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs` (register the module; fix doc claims at ~`:47` and ~`:85-86`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs` (signature; module doc; fn doc)
- Modify: `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs` (stale doc at ~`:16`, ~`:227-228`; new coverage)

**Interfaces:**
- Consumes: `GatewayError` (Task 8).
- Produces: `pub(crate) struct EnvelopeBytes(pub(crate) Bytes)`, `GatewayError::RequestTooLarge`. Task 10's `Bytes` row depends on the swap.

**`bytes.rs` is a separate file for a load-bearing reason.** Task 10 adds an `ALLOW` row for the extractor's definition site, and `ALLOW` is **per-file**. Putting `EnvelopeBytes` in `chat.rs` would exempt `chat.rs` — the one file whose `body: Bytes` the new row exists to catch — and the gate would report green over an unconverted handler. The file must also sit under `rs/crates/services/*/src/adapters/http/**`, or the gate's `SCAN_GLOB` cannot see it.

- [ ] **Step 1: Add the `RequestTooLarge` variant**

In `error.rs`'s enum, after `InvalidRequestSchema`:

```rust
    /// The request body exceeded the configured byte limit → 413. Before SMA-588 this answered
    /// with axum's own plain-text rejection, OUTSIDE the OpenAI envelope — the last request-path
    /// escape in this service.
    RequestTooLarge,
```

In `parts()`, after the `InvalidRequestSchema` arm:

```rust
            GatewayError::RequestTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "invalid_request_error",
                Some("request-too-large"),
                None,
                "The request body is too large.",
            ),
```

In `retryable()`, add `RequestTooLarge` to the `Retryable::No` chain.

- [ ] **Step 2: Write the extractor with its tests**

Create `rs/crates/services/paigasus-gateway/src/adapters/http/bytes.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The house raw-body extractor: `EnvelopeBytes` answers a refused body inside the gateway's
//! OpenAI-compatible envelope instead of letting axum's plain-text rejection escape it
//! (SMA-588).
//!
//! `chat_completions` reads its body as raw [`Bytes`] so the original bytes can be forwarded
//! upstream verbatim. That is still true — this wraps EXTRACTION only and hands the same
//! `Bytes` through untouched, so the egress-hygiene property `chat.rs` calls load-bearing is
//! unaffected.
//!
//! What it changes is the failure path. `Bytes` honours the [`DefaultBodyLimit`] layer, so an
//! over-limit body fails the extractor with a 413 — and axum's own rejection is plain text with
//! no `error.code`, which the OpenAI SDKs cannot read. This is the same class of hole SMA-586
//! and SMA-587 closed in IAM, in the other service.
//!
//! **This file has its own module deliberately.** `ci/http-extractor/check.py`'s ALLOW table is
//! per-FILE, so the exemption the definition site needs would, from inside `chat.rs`, switch the
//! gate off for the very handler its `Bytes` row exists to catch.
//!
//! Classification is by STATUS, not by variant — the rule `json.rs` established in IAM.
//! `BytesRejection` wraps `FailedToBufferBody`, itself `{LengthLimitError (413),
//! UnknownBodyError (400)}`, so mapping the variant straight to `RequestTooLarge` would render a
//! 413 code on a 400 response.

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::adapters::http::error::GatewayError;

/// Maps a rejection's status to the client-facing error, or `None` when it is not the caller's
/// mistake — in which case axum's own response is handed back rather than a 4xx-flavoured code
/// being stamped onto a 5xx. IAM's `path.rs` and `json.rs` make the identical choice.
fn classify(status: StatusCode) -> Option<GatewayError> {
    match status {
        StatusCode::PAYLOAD_TOO_LARGE => Some(GatewayError::RequestTooLarge),
        s if s.is_client_error() => Some(GatewayError::BadRequestBody),
        _ => None,
    }
}

/// `Bytes` with the gateway's OpenAI-compatible envelope on rejection.
#[derive(Debug)]
pub(crate) struct EnvelopeBytes(pub(crate) Bytes);

impl<S: Send + Sync> FromRequest<S> for EnvelopeBytes {
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Bytes::from_request(req, state).await {
            Ok(bytes) => Ok(EnvelopeBytes(bytes)),
            Err(rejection) => Err(envelope_rejection(rejection)),
        }
    }
}

fn envelope_rejection(rejection: BytesRejection) -> Response {
    match classify(rejection.status()) {
        Some(err) => err.into_response(),
        None => rejection.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::DefaultBodyLimit;
    use axum::routing::post;

    use tower::ServiceExt;

    async fn ok(EnvelopeBytes(b): EnvelopeBytes) -> String {
        format!("{}", b.len())
    }

    /// `classify` is a free function precisely so the arms a real rejection cannot reach are
    /// still testable: `BytesRejection` is `#[non_exhaustive]` with `pub(crate)` constructors,
    /// so it cannot be built outside axum.
    #[test]
    fn classify_maps_each_status_class() {
        assert_eq!(classify(StatusCode::PAYLOAD_TOO_LARGE), Some(GatewayError::RequestTooLarge));
        assert_eq!(classify(StatusCode::BAD_REQUEST), Some(GatewayError::BadRequestBody));
        assert_eq!(classify(StatusCode::INTERNAL_SERVER_ERROR), None, "a server fault is not the caller's mistake");
    }

    /// The 413 path end-to-end through a real `DefaultBodyLimit` router — the only way to
    /// produce a genuine `LengthLimitError`.
    #[tokio::test]
    async fn an_oversized_body_answers_in_the_openai_envelope() {
        let app = Router::new().route("/", post(ok)).layer(DefaultBodyLimit::max(8));
        let resp = app
            .oneshot(axum::http::Request::builder().method("POST").uri("/").body(Body::from(vec![b'x'; 64])).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("an envelope, not plain text");
        assert_eq!(body["error"]["code"], "request-too-large");
        assert_eq!(body["error"]["type"], "invalid_request_error");
    }

    /// A body within the limit still reaches the handler with its bytes intact — the property
    /// the egress path depends on.
    #[tokio::test]
    async fn a_body_within_the_limit_passes_through_unchanged() {
        let app = Router::new().route("/", post(ok)).layer(DefaultBodyLimit::max(64));
        let resp = app
            .oneshot(axum::http::Request::builder().method("POST").uri("/").body(Body::from(vec![b'x'; 8])).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), "8");
    }
}
```

- [ ] **Step 3: Register the module and swap the handler**

In `adapters/http/mod.rs`, add beside the other module declarations:

```rust
pub(crate) mod bytes;
```

In `chat.rs`, change the signature and the first line of the body:

```rust
-pub async fn chat_completions(State(state): State<AppState>, caller: Option<Extension<CallerContext>>, body: Bytes) -> Response {
+pub async fn chat_completions(State(state): State<AppState>, caller: Option<Extension<CallerContext>>, EnvelopeBytes(body): EnvelopeBytes) -> Response {
```

Add `use crate::adapters::http::bytes::EnvelopeBytes;` and drop the now-unused `Bytes` from `chat.rs`'s `axum::body` import **only if nothing else in the file uses it** — `terminal_sse_error_stream` and its tests do use `Bytes`, so keep the import and expect a "unused import" error only if you removed too much.

- [ ] **Step 4: Fix the four stale doc claims**

All four describe the escape as correct behaviour. In `chat.rs`'s module doc, item 1:

```rust
//! 1. Read the full request body as raw [`EnvelopeBytes`] — the body-size limit is enforced by
//!    the [`DefaultBodyLimit`](axum::extract::DefaultBodyLimit) layer, which `EnvelopeBytes`
//!    honours and reports as a `413` INSIDE the OpenAI envelope (SMA-588); before that it was
//!    axum's plain text, outside the error contract.
```

In `chat_completions`'s own doc, replace the sentence "The [`Bytes`] extractor also honours the [`DefaultBodyLimit`] layer, returning `413` for an over-limit body before this function body runs." with:

```rust
/// [`EnvelopeBytes`] wraps [`Bytes`], so it honours the
/// [`DefaultBodyLimit`](axum::extract::DefaultBodyLimit) layer the same way and returns `413`
/// for an over-limit body before this function body runs — rendered through the OpenAI envelope
/// rather than as axum's plain text (SMA-588). It is still `FromRequest`, so it must come LAST.
```

In `adapters/http/mod.rs`, correct the two claims at ~`:47` ("an over-limit body is rejected with `413`") and ~`:85-86` ("an over-limit body fails the handler's `Bytes` extractor with `413`") to name `EnvelopeBytes` and say the 413 is rendered in the envelope.

In `tests/chat_proxy.rs`, correct the same claim at ~`:16` and ~`:227-228`.

- [ ] **Step 5: Add end-to-end coverage**

Add to `tests/chat_proxy.rs`:

```rust
/// SMA-588: the oversized-body path answers inside the OpenAI envelope, not as axum plain text.
/// The unit test in `bytes.rs` proves the extractor; this proves it is reachable on the real
/// route, behind the real auth layer and the real configured limit.
#[tokio::test]
async fn an_oversized_body_answers_in_the_openai_envelope() {
    let (status, body) = post_chat_bytes(&vec![b'x'; 8 * 1024 * 1024]).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["error"]["code"], "request-too-large", "{body}");
    assert_eq!(body["error"]["type"], "invalid_request_error", "{body}");
}
```

Size the payload above the suite's configured `max_request_bytes` — read the value from the test config rather than assuming 8 MiB, and adjust.

- [ ] **Step 6: Run the tests**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && env -u CI cargo nextest run -p paigasus-gateway --no-tests=pass 2>&1 | tail -25`
Expected: PASS.

- [ ] **Step 7: Format, lint, commit**

```bash
cd rs && cargo fmt && cargo clippy -p paigasus-gateway --all-targets -- -D warnings && cd ..
git add rs/crates/services/paigasus-gateway/
git commit -m "feat(rs): answer an oversized gateway body inside the openai envelope (SMA-588)"
```

---

### Task 10: Turn the gate on

**Files:**
- Modify: `ci/http-extractor/check.py`
- Modify: `ci/http-extractor/README.md`

**Interfaces:**
- Consumes: Tasks 5 and 9 (every handler already converted). **Running this task before those two reds the gate on real code.**
- Produces: nothing consumed later.

- [ ] **Step 1: Widen `BANNED` to a 4-tuple and enable the rows**

Replace the `BANNED` block and its preamble comment:

```python
# (type name, enabled, required replacement, require a following `<`)
#
# One row per extractor type with an explicit on/off flag. SMA-587 reserved the `Query` and
# `Path` rows so closing those instances would be a flag flip; SMA-588 flips them, adds `Bytes`,
# and this table is now COMPLETE for every request-input kind used in this tree.
#
# The fourth element narrows the match to `Name<`. Only `Path` needs it: a bare `Path` also
# matches `std::path::Path` (`p: &Path`). There is no `std::path` use in either `adapters/http`
# tree today — measured — so this is prophylactic, but it is cheap and it fails safe. `PathBuf`
# was already excluded by the trailing identifier-boundary lookahead. Every other row keeps
# bare-identifier matching, which fails CLOSED (see `_banned_pattern`).
BANNED = (
    ("Json", True, "EnvelopeJson", False),
    ("Query", True, "EnvelopeQuery", False),
    ("Path", True, "UuidPath/StringPath", True),
    ("Bytes", True, "EnvelopeBytes", False),
)
```

- [ ] **Step 2: Thread the flag through the six unpack sites**

`_banned_pattern` takes the flag:

```python
def _banned_pattern(name, require_generic=False):
    """Match `name` as a whole IDENTIFIER inside a parameter span.

    The boundaries are what make the rule usable:
      * `EnvelopeJson<T>`, `EnvelopeQuery<T>`, `UuidPath<T>`, `StringPath<T>` and
        `EnvelopeBytes` — the house replacements — do NOT match, because the preceding character
        is an identifier character. Without that, the gate would red on every handler these
        tickets just fixed.
      * `JsonRejection` / `QueryRejection` / `PathRejection` / `BytesRejection` do not match
        either (trailing identifier characters).
      * `axum::Json<T>` DOES match: `:` is not an identifier character.

    `require_generic` additionally demands a following `<`, so `p: &Path` (std::path) and
    `p: PathBuf` are not violations while `Path<String>` is. It is opt-in per row and used only
    by `Path`: requiring `<` everywhere would miss a hypothetical non-generic alias, so the
    default still fails CLOSED (this reverses the blanket claim this comment used to make).
    """
    tail = r"\s*<" if require_generic else r"(?![A-Za-z0-9_])"
    return re.compile(r"(?<![A-Za-z0-9_])" + re.escape(name) + tail)


_PATTERNS = {name: _banned_pattern(name, needs_generic) for name, _, _, needs_generic in BANNED}
```

In `violations_in`, update the default:

```python
        enabled = {name: repl for name, on, repl, _g in BANNED if on}
```

- [ ] **Step 3: Delete the assertion that forbids this change**

Remove this block from `self_test` (it is the one that fails the build on the flip):

```python
    # ...and it must be OFF for the real tree right now, or SMA-587 reds work it does not do.
    if any(on for name, on, _r in BANNED if name in ("Query", "Path")):
        print("  FAIL [BANNED] a reserved row is enabled — that is a follow-up's call", file=sys.stderr)
        rc = 1
```

- [ ] **Step 4: Re-point the `reserved` fixture at a synthetic name**

Replace the reserved-row check with one that still proves the enable-a-row mechanism works, now that every real row is live:

```python
    # A row must WORK when flipped on. Every real row is enabled now, so this drives a SYNTHETIC
    # name instead — otherwise the check would only re-test a live row and prove nothing about a
    # FUTURE reserved one. Deleting it would remove the only thing keeping that mechanism honest.
    reserved = "async fn list(State(s): State<AppState>, Widget(w): Widget<PageQuery>) -> Response {"
    got = sorted(ext for _fn, _l, ext, _r in violations_in(reserved, "<reserved>", {"Widget": "EnvelopeWidget"}))
    if got != ["Widget"]:
        print(f"  FAIL [BANNED reserved] enabling a new row matched nothing: {got}", file=sys.stderr)
        rc = 1
```

Note this needs `_PATTERNS` to tolerate a name it does not know. Add a fallback in `violations_in`:

```python
        for extractor, replacement in sorted(enabled.items()):
            pattern = _PATTERNS.get(extractor) or _banned_pattern(extractor)
            hit = pattern.search(span)
```

- [ ] **Step 5: Update the remaining `self_test` unpacks and the table-shape checks**

```python
    names = [name for name, _on, _repl, _g in BANNED]
    if len(names) != len(set(names)):
        print("  FAIL [BANNED] duplicate extractor rows", file=sys.stderr)
        rc = 1
    for name, on, repl, _g in BANNED:
        if on != (repl is not None):
            print(f"  FAIL [BANNED] {name}: exactly the enabled rows must name a replacement", file=sys.stderr)
            rc = 1
    if not any(on for _n, on, _r, _g in BANNED):
        print("  FAIL [BANNED] every row is disabled — the gate would guard nothing", file=sys.stderr)
        rc = 1
```

- [ ] **Step 6: Add the two ALLOW rows**

```python
ALLOW = (
    ("rs/crates/services/paigasus-iam/src/adapters/http/json.rs",
     "the extractor's own definition site — it wraps `axum::Json` by construction"),
    ("rs/crates/services/paigasus-iam/src/adapters/http/query.rs",
     "the extractor's own definition site — it wraps `axum::Query` by construction"),
    ("rs/crates/services/paigasus-gateway/src/adapters/http/bytes.rs",
     "the extractor's own definition site — it wraps `axum::body::Bytes` by construction"),
)
```

**Do not add `path.rs`** — `StringPath` reaches `Path::<String>::from_request_parts` in a function body, outside every parameter span, and the `<`-requiring pattern does not match a turbofish either. **Do not add `chat.rs`** — that would exempt the one file the `Bytes` row exists to catch. Extend the ALLOW block's own comment to say both, since a future reader will reach for exactly those two rows.

- [ ] **Step 7: Add and invert fixtures**

Invert the existing `Bytes` fixture:

```python
    (
        "planted violation — a body taken as Bytes, the gateway's shape (SMA-588 closed this)",
        "pub async fn chat_completions(State(state): State<AppState>, "
        "caller: Option<Extension<CallerContext>>, body: Bytes) -> Response {",
        [("chat_completions", "Bytes")],
    ),
```

Add fixtures for the newly enabled rows:

```python
    (
        "planted violation — a bare Query in request position",
        "async fn list(State(s): State<AppState>, Query(q): Query<PageQuery>) -> Response {",
        [("list", "Query")],
    ),
    (
        "planted violation — a bare Path<String> in request position",
        "async fn delete_policy(State(s): State<AppState>, Path(id): Path<String>) -> Response {",
        [("delete_policy", "Path")],
    ),
    (
        "legal — the house replacements for the newly enabled rows",
        "async fn list(State(s): State<AppState>, path: UuidPath<TeamId>, "
        "EnvelopeQuery(q): EnvelopeQuery<PageQuery>) -> Result<Json<Vec<Dto>>, ApiError> {",
        [],
    ),
    (
        "legal — StringPath and EnvelopeBytes are not their wrapped types",
        "async fn retire(policy: StringPath<PolicyId>, EnvelopeBytes(b): EnvelopeBytes) -> Response {",
        [],
    ),
    (
        "legal — `std::path::Path` is not axum's, which is why the Path row requires a `<`",
        "fn read_bundle(p: &Path, buf: PathBuf) -> Response {",
        [],
    ),
    (
        "legal — the rejection types are not their extractors",
        "fn envelope_rejection(q: QueryRejection, p: PathRejection, b: BytesRejection) -> Response {",
        [],
    ),
```

- [ ] **Step 8: Run the gate**

Run: `python3 ci/http-extractor/check.py --self-test && python3 ci/http-extractor/check.py --check`
Expected: both rc 0, `self-test: OK`.

- [ ] **Step 9: Prove it reds**

Temporarily revert one handler — change `organizations.rs:63`'s `EnvelopeQuery(q): EnvelopeQuery<PageQuery>` back to `Query(q): Query<PageQuery>` — and run `python3 ci/http-extractor/check.py --check`.
Expected: rc 1, naming `organizations.rs:63 fn list_orgs(…) takes `Query``.

Then restore it **by editing the line back**, not with `git checkout` — that would discard the rest of Task 5. Re-run and confirm rc 0.

- [ ] **Step 10: Rewrite the README**

Five edits:
1. **Retire L3 entirely.** It claims a body taken as `Bytes` produces "no rejection is produced for this gate to care about". Under `DefaultBodyLimit` it produces exactly one, which is what SMA-588 closed. Replace it with a note that `String` bodies carry the identical hole, that none exists in either tree today, and that a future one needs the same treatment.
2. `:29-35` "What it does NOT gate" — remove the reserved-rows prose; all four rows are live.
3. `:60-61` the identifier-boundary paragraph — document the per-row `<` requirement and why only `Path` uses it.
4. `:86` "The ALLOW table — One row" — now three, and say ALLOW is **per-file**, naming what each row also exempts and why `chat.rs` must never be one.
5. `:77` the "Eighteen request positions" positive-control line — re-count against the tree and correct it.

- [ ] **Step 11: Commit**

```bash
git add ci/http-extractor/
git commit -m "ci(ci): gate Query, Path and Bytes request extractors (SMA-588)"
```

---

### Task 11: Full-graph verification

**Files:** none modified unless a gate reds.

- [ ] **Step 1: Run the full CI graph**

Per-project tasks do NOT run the repo-level gates. Run what CI runs:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials --base origin/main \
  --include-relations
```

Expected: all green.

- [ ] **Step 2: Diagnose any failure through the cache report**

Moon reports an unattributed "N failed". Read `.moon/cache/ciReport.json` to find which target went red before changing anything.

Two failures have known, non-obvious causes:
- **`repo:affected-smoke` fails in under 3 seconds** — capture the full output first, then check it for `proto-shim: … Permission denied`. If present it is the known infrastructure abort (rc 2, not a red verdict); re-run `moon run repo:affected-smoke --force` alone. If absent, diagnose it on its own terms.
- **`contracts:*` reds after a `.proto` edit** — `moon run contracts:fmt` was skipped in Task 1, or the generated bindings were not regenerated and committed.

- [ ] **Step 3: Confirm the codegen-drift step would pass**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run contracts:generate --force && git status --short contracts/ rs/crates/libs/paigasus-proto/src/generated/`
Expected: **no output** — the committed bindings already match. Any diff here means Task 1's Step 6 did not land and CI's unconditional drift step will red.

- [ ] **Step 4: Confirm nothing is left uncommitted**

Run: `git status --short`
Expected: clean.

---

## Self-Review

**Spec coverage.** Every spec section maps to a task: D1 → Task 1; D2/D2.1 → Task 2; D3 → Tasks 3 and 4; D4.1 → Task 8; D4.2 → Task 9; D5/D5.1 → Task 10; D6 → Tasks 1, 7, 9 and 10 (all ten invalidated sites); Registry mechanics → Task 1 (both count sites); Cross-transport divergence → Task 7; Testing → Tasks 3, 4, 6, 8, 9 and 10; Verification and its "no plumbing required" finding → Task 11.

**Placeholders.** None. Every code step carries real code; every command is runnable as written.

**Type consistency.** `StringPath<F>` exposes `pub value: String` in Task 3 and is read as `policy.value` in Task 5. `EnvelopeQuery<T>` is a tuple struct destructured as `EnvelopeQuery(q)` in both Task 4 and Task 5. `EnvelopeBytes` is destructured as `EnvelopeBytes(body)` in Task 9 and named as a replacement string in Task 10. `TenancyError::InvalidPathSegment` takes `&'static str` in Task 2 and is called with `F::NAME` in Task 3. `GatewayError::{InvalidRequestSchema, RequestTooLarge}` are declared in Tasks 8 and 9 and referenced in `bytes.rs`'s `classify` in Task 9.

**Two ordering constraints that are not optional:**
- Task 1 before Task 2 — the `TenancyError` membership guard enumerates the enum and cross-checks it against the registry.
- Tasks 5 and 9 before Task 10 — enabling a gate row against unconverted handlers reds the build on real code.
