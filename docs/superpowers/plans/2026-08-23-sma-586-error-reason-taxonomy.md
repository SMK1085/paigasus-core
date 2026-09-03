# SMA-586 Error-Reason Taxonomy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `TenancyError::InvalidPrn`'s catch-all duty with six distinct, registered error reasons, so a malformed timestamp, uuid, cursor, audit outcome, missing field and mutually-exclusive field pair each get their own machine-readable `reason` on both HTTP and gRPC.

**Architecture:** Six new `TenancyError` variants, each carrying a `&'static str` field name (making caller input structurally unrepresentable in the payload). Both transports already derive `reason` from `TenancyError::code()`, so the enum is the single migration point. Two parity repairs close AC-1 gaps where one transport had no emitter at all: a custom axum path extractor for uuids, and empty-string checks on three gRPC required-field sites.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), `thiserror`, `strum::EnumIter`, `axum` 0.8 `FromRequestParts`, `tonic`, `prost`, `buf` for codegen, Moon for the task graph.

**Spec:** `docs/superpowers/specs/2026-08-23-sma-586-error-reason-taxonomy-design.md`

> **This plan is a historical record, not live instructions.** It was executed on
> 2026-08-23/24 and the branch's commits are the authority where the two differ. Review found
> three defects in the plan's own prescribed code, all fixed in the shipped implementation —
> do not copy these from here:
> - **Task 7's `UuidPathPair<F>`** carried ONE marker and reported that field for either
>   segment, so a malformed service-account uuid answered `"api_key_id must be a uuid"`. Shipped
>   as `UuidPathPair<F1, F2>`, parsing each segment independently and attributing positionally.
> - **Task 7's extractor** swallowed every `PathRejection`, turning 500-class router bugs into
>   `400 invalid-uuid`. Shipped branching on `FailedToDeserializePathParams::status()`, which
>   preserves a 500 for an arity/unsupported-type bug. (Note `PathRejection` has only two
>   variants; `WrongNumberOfParameters`/`UnsupportedType` are kinds nested inside the first.)
> - **Task 8's "dead-letter id" parity row** constructed `TenancyError::InvalidUuid` inline, so
>   its HTTP half was tautological — mutation testing showed it stayed green with the real
>   extractor broken. Shipped as a real `Router`/`oneshot` round trip.
>
> Task 1's `git add py ts` staging is also broader than that task's own outputs, and its Step 7
> drift check runs before the intended changes are committed. Both were harmless in execution;
> neither is a pattern to reuse.

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Rust crates are **edition 2024 + rust-version 1.95**.
- Conventional commits with a workspace scope: `feat(rs):`, `fix(contracts):`. Subject must **start lowercase** and be **≤100 chars**. Never put a `#NNN` issue ref or a `token: value` line in the commit **body** — it breaks commitlint's `footer-leading-blank`.
- Branch is already created: `feature/sma-586-error-reason-taxonomy`. Do not create another.
- The Bash PATH lacks proto-managed CLIs. **Every** command below assumes you first ran:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`
- Rust workspace lints are `warnings = deny`, and dead code is a hard **compile** error on the lib target. Do not add a variant in one task and wire it up in a later one — each task must leave the crate compiling.
- `cargo nextest` exits non-zero on a workspace with no tests: use `--no-tests=pass` if you ever scope it that narrowly.
- Integration tests under `tests/` are Docker-backed. Run them with Docker up. For a **filtered** run you must set `PAIGASUS_REQUIRE_DOCKER=1`, because the `docker_preflight` canary is not in your filter and the suites otherwise skip silently.
- Never bypass the commit hook with `--no-verify`.

**Working directory for all Rust commands:** `rs/`
**IAM crate root:** `rs/crates/services/paigasus-iam/`

---

### Task 1: Register the six reasons in the canonical registry

This is the contract change. Nothing else compiles against it yet, so it stands alone and is independently verifiable by the registry's own tests.

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/error.proto` (append after line 138, the current IAM max)
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs:154-217` (`EXPECTED_REASONS` and the count assertion)
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/**`, `py/**/generated/**`, `ts/**/generated/**`

**Interfaces:**
- Consumes: nothing.
- Produces: `ErrorReason::InvalidTimestamp`, `InvalidUuid`, `InvalidCursor`, `InvalidAuditOutcome`, `MissingRequiredField`, `MutuallyExclusiveFields` — prost-generated variants on `paigasus_proto::paigasus::common::v1::ErrorReason`. Their wire strings are `"invalid-timestamp"`, `"invalid-uuid"`, `"invalid-cursor"`, `"invalid-audit-outcome"`, `"missing-required-field"`, `"mutually-exclusive-fields"`.

- [ ] **Step 1: Write the failing test**

In `rs/crates/libs/paigasus-proto/src/error.rs`, inside the existing `#[cfg(test)] mod tests`, add:

```rust
/// SMA-586: the six request-validation reasons that replace `invalid-prn`'s catch-all duty.
/// Asserted by wire string rather than by enum variant so a renumbering or a rename that
/// silently changes the kebab spelling fails here too.
#[test]
fn the_request_validation_reasons_resolve_both_ways() {
    for (variant, wire) in [
        (ErrorReason::InvalidTimestamp, "invalid-timestamp"),
        (ErrorReason::InvalidUuid, "invalid-uuid"),
        (ErrorReason::InvalidCursor, "invalid-cursor"),
        (ErrorReason::InvalidAuditOutcome, "invalid-audit-outcome"),
        (ErrorReason::MissingRequiredField, "missing-required-field"),
        (ErrorReason::MutuallyExclusiveFields, "mutually-exclusive-fields"),
    ] {
        assert_eq!(variant.as_wire_reason().as_deref(), Some(wire));
        assert_eq!(ErrorReason::from_wire_reason(wire), Some(variant));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd rs && cargo test -p paigasus-proto the_request_validation_reasons_resolve_both_ways`
Expected: FAIL to **compile** — `no variant named InvalidTimestamp found for enum ErrorReason`.

- [ ] **Step 3: Add the six values to the proto registry**

In `contracts/proto/paigasus/common/v1/error.proto`, immediately after
`ERROR_REASON_DECISION_CHANGE_UNACKNOWLEDGED = 32;` (line 138) and before the
`// ---- Gateway (300-599)` banner, insert:

```proto
  // ---- IAM: request validation (1-299) -------------------------------------
  // SMA-586. These six replace `invalid-prn`'s use as a catch-all sentinel for
  // any validation failure lacking a dedicated code; `invalid-prn` now means a
  // genuine PRN parse or shape failure and nothing else.

  // "invalid-timestamp" — a timestamp field could not be parsed or represented.
  ERROR_REASON_INVALID_TIMESTAMP = 33;
  // "invalid-uuid" — a field that must be a uuid was not one.
  ERROR_REASON_INVALID_UUID = 34;
  // "invalid-cursor" — an opaque pagination cursor was not a well-formed token.
  // Distinct from invalid-uuid on purpose: a cursor is server-issued, so a
  // client can recover by restarting pagination without any user action.
  ERROR_REASON_INVALID_CURSOR = 35;
  // "invalid-audit-outcome" — the audit `outcome` filter did not name a known
  // outcome. Named for its surface rather than the bare word "outcome", which
  // is already an overloaded token (a metric label, and RetireOutcome).
  ERROR_REASON_INVALID_AUDIT_OUTCOME = 36;
  // "missing-required-field" — a semantically required field was absent or empty.
  ERROR_REASON_MISSING_REQUIRED_FIELD = 37;
  // "mutually-exclusive-fields" — two fields were set that may not both be.
  // HTTP-only, structurally: the gRPC surface expresses the same choice as a
  // proto3 `oneof`, which cannot carry two values, so its only failure mode is
  // "neither set" — which is missing-required-field, not this.
  // Deliberately NOT named "*-conflict": every other conflict-named reason in
  // this registry is a 409, and this is a 400.
  ERROR_REASON_MUTUALLY_EXCLUSIVE_FIELDS = 38;
```

- [ ] **Step 4: Extend the Rust mirror and its count**

In `rs/crates/libs/paigasus-proto/src/error.rs`, after `"decision-change-unacknowledged",` in
`EXPECTED_REASONS` (line 189), insert:

```rust
        // IAM: request validation (SMA-586)
        "invalid-timestamp",
        "invalid-uuid",
        "invalid-cursor",
        "invalid-audit-outcome",
        "missing-required-field",
        "mutually-exclusive-fields",
```

Then change line 217 from `46` to `52`:

```rust
        assert_eq!(actual.len(), 52, "the registry should hold 52 reasons");
```

- [ ] **Step 5: Format and regenerate the bindings**

Run from **`contracts/`**, not the repo root — there is no root-level `buf.gen.yaml`; all buf
config (`buf.yaml`, `buf.gen.yaml`, `buf.lock`) lives in `contracts/`, which is also the cwd
Moon's `contracts:generate` task uses:

```bash
cd contracts && buf format -w && buf generate
```

Do **not** use `moon run contracts:generate` here — that task declares no `outputs:`, so it can
serve stale cached output.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cd rs && cargo test -p paigasus-proto
```

Expected: PASS, including `the_request_validation_reasons_resolve_both_ways`,
`the_registry_contains_exactly_the_expected_reasons` (now asserting 52) and
`the_adr_examples_are_spelled_as_documented`.

- [ ] **Step 7: Verify no codegen drift remains**

```bash
git add --intent-to-add . && git diff --exit-code
```

Expected: exit 0 after you stage the regenerated files in the next step. If it reports changes
in a `generated/` tree you have not staged, stage them — they are part of this task.

- [ ] **Step 8: Commit**

```bash
git add contracts/proto/paigasus/common/v1/error.proto \
        rs/crates/libs/paigasus-proto/src/error.rs \
        rs/crates/libs/paigasus-proto/src/generated \
        py ts
git commit -m "feat(contracts): register six request-validation error reasons (SMA-586)"
```

---

### Task 2: Add the six `TenancyError` variants and the `field()` accessor

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/error.rs`

**Interfaces:**
- Consumes: Task 1's `ErrorReason` variants (via the existing membership test only).
- Produces:
  - `TenancyError::InvalidTimestamp(&'static str)`, `InvalidUuid(&'static str)`, `InvalidCursor(&'static str)`, `InvalidAuditOutcome(&'static str)`, `MissingRequiredField(&'static str)`, `MutuallyExclusiveFields(&'static str)`
  - `TenancyError::field(&self) -> Option<&'static str>` — the payload for those six, `None` for every other variant.
  - `code()` returns the six wire strings from Task 1; `class()` returns `ErrorClass::Validation` for all six.

**Note on staging:** adding variants alone does not trip `warnings = deny` (an unused enum
variant on a `pub` enum is not dead code), so this task compiles and tests clean on its own.

- [ ] **Step 1: Write the failing tests**

In `rs/crates/services/paigasus-iam/src/application/error.rs`, inside `#[cfg(test)] mod tests`, add:

```rust
/// SMA-586: the six reasons that replace `invalid-prn`'s catch-all duty. All are
/// `Validation` — the sites they migrate are 400/InvalidArgument today and must stay so.
#[test]
fn the_request_validation_codes_are_stable_and_all_validation() {
    for (err, code) in [
        (TenancyError::InvalidTimestamp("from"), "invalid-timestamp"),
        (TenancyError::InvalidUuid("membership_id"), "invalid-uuid"),
        (TenancyError::InvalidCursor("cursor"), "invalid-cursor"),
        (TenancyError::InvalidAuditOutcome("outcome"), "invalid-audit-outcome"),
        (TenancyError::MissingRequiredField("owner_prn"), "missing-required-field"),
        (TenancyError::MutuallyExclusiveFields("principal|node"), "mutually-exclusive-fields"),
    ] {
        assert_eq!(err.code(), code);
        assert_eq!(err.class(), ErrorClass::Validation, "{code} must stay a 400");
    }
}

/// The field name reaches `Display` — the inverse of the pre-SMA-586 behaviour, where every
/// call site passed a detail and `InvalidPrn`'s static Display threw it away.
#[test]
fn the_request_validation_displays_carry_their_field_name() {
    assert_eq!(TenancyError::InvalidTimestamp("parked_to").to_string(), "invalid timestamp for parked_to");
    assert_eq!(TenancyError::InvalidUuid("api_key_id").to_string(), "api_key_id must be a uuid");
    assert_eq!(TenancyError::InvalidCursor("cursor").to_string(), "cursor is not a valid pagination cursor");
    assert_eq!(TenancyError::InvalidAuditOutcome("outcome").to_string(), "outcome is not a known audit outcome");
    assert_eq!(TenancyError::MissingRequiredField("scope_prn").to_string(), "scope_prn is required");
    assert_eq!(TenancyError::MutuallyExclusiveFields("principal|node").to_string(), "provide exactly one of principal|node");
}

/// `field()` is what `status_to_grpc` uses to populate `ErrorInfo.metadata["field"]` without
/// matching on variants at the transport layer. It is `None` for everything else — including
/// `InvalidPrn`, whose `String` payload is deliberately NOT a field name.
#[test]
fn field_is_some_only_for_the_request_validation_variants() {
    assert_eq!(TenancyError::InvalidTimestamp("from").field(), Some("from"));
    assert_eq!(TenancyError::MutuallyExclusiveFields("a|b").field(), Some("a|b"));
    assert_eq!(TenancyError::InvalidPrn("iam:bad".to_string()).field(), None);
    assert_eq!(TenancyError::NotFound.field(), None);
    assert_eq!(TenancyError::Internal.field(), None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd rs && cargo test -p paigasus-iam --lib application::error`
Expected: FAIL to compile — `no variant or associated item named InvalidTimestamp`.

- [ ] **Step 3: Add the variants**

In the `TenancyError` enum, immediately after the `InvalidPrn(String)` / `PrnMismatch` pair
(around line 42-44), insert:

```rust
    /// SMA-586. The six variants below replace `InvalidPrn`'s former use as a catch-all
    /// sentinel for any validation failure without a dedicated code. Each carries a
    /// `&'static str` naming the offending wire field, interpolated into `Display` and
    /// emitted as `ErrorInfo.metadata["field"]` on gRPC.
    ///
    /// The payload type is load-bearing: a `&'static str` cannot hold caller-supplied input,
    /// so "never reflect untrusted input into an error body" is enforced by the type rather
    /// than remembered by each call site. The pre-SMA-586 sites passed a `format!` carrying
    /// the caller's raw value; that is now unrepresentable.
    #[error("invalid timestamp for {0}")]
    InvalidTimestamp(&'static str),
    #[error("{0} must be a uuid")]
    InvalidUuid(&'static str),
    /// Distinct from [`TenancyError::InvalidUuid`] on purpose: a cursor is an opaque,
    /// server-issued token, so a client can recover by restarting pagination without
    /// involving the user. Collapsing the two would make that indistinguishable from
    /// "your input is wrong".
    #[error("{0} is not a valid pagination cursor")]
    InvalidCursor(&'static str),
    #[error("{0} is not a known audit outcome")]
    InvalidAuditOutcome(&'static str),
    #[error("{0} is required")]
    MissingRequiredField(&'static str),
    /// HTTP-only, structurally — see the registry comment on
    /// `ERROR_REASON_MUTUALLY_EXCLUSIVE_FIELDS`: the gRPC surface models the same choice as a
    /// proto3 `oneof`, which cannot carry two values.
    #[error("provide exactly one of {0}")]
    MutuallyExclusiveFields(&'static str),
```

- [ ] **Step 4: Add the six `code()` arms**

In `code()`, after `Self::InvalidPrn(_) => "invalid-prn",`:

```rust
            Self::InvalidTimestamp(_) => "invalid-timestamp",
            Self::InvalidUuid(_) => "invalid-uuid",
            Self::InvalidCursor(_) => "invalid-cursor",
            Self::InvalidAuditOutcome(_) => "invalid-audit-outcome",
            Self::MissingRequiredField(_) => "missing-required-field",
            Self::MutuallyExclusiveFields(_) => "mutually-exclusive-fields",
```

- [ ] **Step 5: Add the six `class()` arms**

In `class()`, extend the `ErrorClass::Validation` arm — insert these six after
`| Self::InvalidPrn(_)`:

```rust
            | Self::InvalidTimestamp(_)
            | Self::InvalidUuid(_)
            | Self::InvalidCursor(_)
            | Self::InvalidAuditOutcome(_)
            | Self::MissingRequiredField(_)
            | Self::MutuallyExclusiveFields(_)
```

- [ ] **Step 6: Add the `field()` accessor**

In `impl TenancyError`, after `class()`:

```rust
    /// The wire field name this error names, for `ErrorInfo.metadata["field"]` (SMA-586).
    ///
    /// `None` for every variant that does not carry one — including `InvalidPrn`, whose
    /// `String` payload is a PRN error-kind token or a canonical PRN, not a field name.
    /// Returning it here rather than matching on variants inside `status_to_grpc` keeps the
    /// transport layer free of variant knowledge.
    pub fn field(&self) -> Option<&'static str> {
        match self {
            Self::InvalidTimestamp(f)
            | Self::InvalidUuid(f)
            | Self::InvalidCursor(f)
            | Self::InvalidAuditOutcome(f)
            | Self::MissingRequiredField(f)
            | Self::MutuallyExclusiveFields(f) => Some(f),
            _ => None,
        }
    }
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cd rs && cargo test -p paigasus-iam --lib`
Expected: PASS. In particular `every_tenancy_code_is_declared_in_the_canonical_registry` (in
`adapters::grpc::convert`) must pass **without modification** — it iterates `strum::EnumIter`,
so it picks the six new variants up automatically. If it fails, Task 1 did not land correctly.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/application/error.rs
git commit -m "feat(rs): add six request-validation TenancyError variants (SMA-586)"
```

---

### Task 3: Emit the field name in gRPC `ErrorInfo.metadata`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:111-126` (`status_to_grpc`)

**Interfaces:**
- Consumes: `TenancyError::field()` from Task 2; the existing `iam_status(code, reason, message, retryable, extra)`.
- Produces: every `TenancyError` carrying a field now yields a `Status` whose `ErrorInfo.metadata` contains `"field"`.

**Why this is safe:** `error_metadata` inserts `extra` **first** and lets the canonical
`retryable` / `correlation_id` / `request_id` keys win a collision, so adding `"field"` cannot
displace them.

- [ ] **Step 1: Write the failing test**

In `grpc/convert.rs`'s `#[cfg(test)] mod tests`, add:

```rust
/// SMA-586: the field name is also machine-readable on gRPC. `Display` alone is not enough —
/// SMA-508 AC2 forbids branching on message text, so a field reachable only through the
/// message is reachable only by humans. Uses the same open metadata map as `capability`.
#[test]
fn status_to_grpc_puts_the_field_name_in_error_info_metadata() {
    let status = status_to_grpc(TenancyError::InvalidTimestamp("parked_to"));
    let details = status.get_error_details();
    let info = details.error_info().expect("every IAM status carries ErrorInfo");
    assert_eq!(info.metadata.get("field").map(String::as_str), Some("parked_to"));
    assert_eq!(info.reason, "invalid-timestamp");
    // The canonical keys are untouched by the new one.
    assert_eq!(info.metadata.get("retryable").map(String::as_str), Some("false"));
}

/// A variant with no field name adds no key at all — an absent key, never an empty string,
/// so a consumer can distinguish "no field" from "a field named nothing".
#[test]
fn status_to_grpc_omits_the_field_key_when_there_is_no_field() {
    let status = status_to_grpc(TenancyError::NotFound);
    let details = status.get_error_details();
    let info = details.error_info().expect("every IAM status carries ErrorInfo");
    assert!(!info.metadata.contains_key("field"), "metadata: {:?}", info.metadata);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd rs && cargo test -p paigasus-iam --lib adapters::grpc::convert::tests::status_to_grpc`
Expected: FAIL — `assertion failed: left: None, right: Some("parked_to")`.

- [ ] **Step 3: Thread the field into `iam_status`**

Replace the body of `status_to_grpc` (keep the existing `code` match and the `tracing::error!`
arm exactly as they are) so the final call becomes:

```rust
    // `e.code()` IS the canonical wire string — the registry is the validation (see the
    // `every_tenancy_code_is_declared_in_the_canonical_registry` test), not the transform.
    // The field name (SMA-586) rides in metadata alongside it, so a client can act on WHICH
    // field failed without parsing the message — which SMA-508 AC2 forbids.
    let field = e.field();
    let extra: &[(&str, &str)] = match &field {
        Some(f) => &[("field", f)],
        None => &[],
    };
    iam_status(code, e.code(), e.to_string(), tenancy_retryable(e.class()), extra)
```

If the borrow checker objects to the temporary slice, bind it first:

```rust
    let field = e.field();
    let extra_owned: Vec<(&str, &str)> = field.map(|f| ("field", f)).into_iter().collect();
    iam_status(code, e.code(), e.to_string(), tenancy_retryable(e.class()), &extra_owned)
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd rs && cargo test -p paigasus-iam --lib adapters::grpc::convert`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs
git commit -m "feat(rs): carry the failing field name in grpc ErrorInfo metadata (SMA-586)"
```

---

### Task 4: Migrate the gRPC timestamp, cursor and outcome sites

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:216-224` (`parse_opt_ts`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/audit.rs:56-79` (`parse_outcome`, `parse_cursor`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/dead_letters.rs:73-80` (`parse_cursor`)
- Modify (tests): `grpc/convert.rs:730-762`, `grpc/audit.rs:210,220,252,265`, `grpc/dead_letters.rs:250,258,259,310,318`

**Interfaces:**
- Consumes: Task 2's variants.
- Produces: `pub(crate) fn parse_opt_ts(t: Option<prost_types::Timestamp>, field: &'static str) -> Result<Option<DateTime<Utc>>, TenancyError>` (note the narrowed lifetime — it stays `pub`), `pub(crate) fn parse_outcome(raw: &str) -> Result<Option<AuditOutcome>, TenancyError>` in `grpc::audit`, `pub(crate) fn parse_cursor(raw: &str) -> Result<Option<Uuid>, TenancyError>` in both `grpc::audit` and `grpc::dead_letters`. Task 8's parity guard calls all of these.

- [ ] **Step 1: Replace the swallowing test with its inverse**

In `grpc/convert.rs`, **delete** `parse_opt_ts_detail_is_swallowed_by_invalid_prns_static_display`
(lines 743-762, including its doc comment) and put this in its place:

```rust
/// The inverse of the pre-SMA-586 behaviour this replaces. `parse_opt_ts` took a field name
/// and then threw it away, because `InvalidPrn`'s Display is static — pinned by the test that
/// used to live here. `InvalidTimestamp` interpolates it, so the caller learns WHICH bound
/// failed, and `status_to_grpc` also puts it in `ErrorInfo.metadata["field"]`.
#[test]
fn parse_opt_ts_surfaces_the_field_name_in_its_display() {
    let err = parse_opt_ts(Some(prost_types::Timestamp { seconds: 0, nanos: -1 }), "parked_to").unwrap_err();
    assert_eq!(err, TenancyError::InvalidTimestamp("parked_to"));
    assert!(err.to_string().contains("parked_to"), "got {err}");
    assert_eq!(err.code(), "invalid-timestamp");
}
```

- [ ] **Step 2: Retarget the remaining `InvalidPrn` assertions in this task's files**

In `grpc/convert.rs:736`, inside
`parse_opt_ts_rejects_a_present_but_unrepresentable_timestamp_instead_of_unfiltering`, change:

```rust
        assert!(matches!(err, TenancyError::InvalidPrn(_)), "{label} must be a client error, not None");
```
to
```rust
        assert!(matches!(err, TenancyError::InvalidTimestamp(_)), "{label} must be a client error, not None");
```

In `grpc/audit.rs`, change the four assertions at lines 210, 220, 252, 265 from
`TenancyError::InvalidPrn(_)` to the variant that site now produces — read each test's name to
tell which: an outcome test asserts `InvalidAuditOutcome(_)`, a cursor test asserts
`InvalidCursor(_)`, a timestamp test asserts `InvalidTimestamp(_)`.

In `grpc/dead_letters.rs`, change line 250 to `Err(TenancyError::InvalidCursor(_))`, lines 258
and 259 to `Err(TenancyError::InvalidTimestamp(_))`, and lines 310 and 318 to
`Err(TenancyError::InvalidTimestamp(_))`.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cd rs && cargo test -p paigasus-iam --lib adapters::grpc`
Expected: FAIL — the assertions now expect the new variants but the production code still
constructs `InvalidPrn`.

- [ ] **Step 4: Migrate `parse_opt_ts`**

In `grpc/convert.rs`, change the signature and the error, and fix the doc comment's now-false
last paragraph:

```rust
/// `InvalidTimestamp` carries the field name, which reaches both the message and
/// `ErrorInfo.metadata["field"]` (SMA-586 — this used to be `InvalidPrn`-as-sentinel, whose
/// static Display threw the field name away).
pub(crate) fn parse_opt_ts(t: Option<prost_types::Timestamp>, field: &'static str) -> Result<Option<DateTime<Utc>>, TenancyError> {
    match t {
        None => Ok(None),
        Some(raw) => from_ts(raw).map(Some).ok_or(TenancyError::InvalidTimestamp(field)),
    }
}
```

Note `ok_or` replaces `ok_or_else` — there is no allocation to defer any more.

- [ ] **Step 5: Migrate the audit helpers**

In `grpc/audit.rs`, replace both helpers (and their doc comments, which claim "there is no
dedicated error code" and are now false):

```rust
/// Parses the wire `outcome` filter: empty means unfiltered; a non-empty value must name a
/// known [`AuditOutcome`]. The caller's raw value is deliberately NOT echoed back — the
/// `&'static str` payload cannot carry it (SMA-586).
pub(crate) fn parse_outcome(raw: &str) -> Result<Option<AuditOutcome>, TenancyError> {
    if raw.is_empty() {
        return Ok(None);
    }
    AuditOutcome::parse(raw).map(Some).ok_or(TenancyError::InvalidAuditOutcome("outcome"))
}

/// Parses the wire `cursor`: empty means "first page" (`None`); a non-empty value must be a
/// valid uuid. `InvalidCursor` rather than `InvalidUuid`: a cursor is server-issued, so a
/// client recovers by restarting pagination rather than by asking the user to fix input
/// (SMA-586).
pub(crate) fn parse_cursor(raw: &str) -> Result<Option<Uuid>, TenancyError> {
    if raw.is_empty() {
        return Ok(None);
    }
    Uuid::parse_str(raw).map(Some).map_err(|_| TenancyError::InvalidCursor("cursor"))
}
```

- [ ] **Step 6: Migrate the dead-letter cursor helper**

In `grpc/dead_letters.rs`:

```rust
/// Empty means unfiltered; a non-empty value must parse as a uuid. Mirrors
/// `grpc::audit::parse_cursor`, including its `InvalidCursor`-not-`InvalidUuid` choice.
pub(crate) fn parse_cursor(raw: &str) -> Result<Option<Uuid>, TenancyError> {
    if raw.is_empty() {
        return Ok(None);
    }
    Uuid::parse_str(raw).map(Some).map_err(|_| TenancyError::InvalidCursor("cursor"))
}
```

- [ ] **Step 7: Fix the stale doc comment on `grpc/service_accounts.rs:178-183`**

That comment says `expires_at`'s failure is "`InvalidPrn`-as-sentinel … there is no dedicated
error code for 'not a valid timestamp' either". Replace those two sentences with:

```rust
            // `expires_at` unset means non-expiring (or the configured `default_expiry_days`
            // fallback, `ApiKeyService::issue`) — mirrors `IssueApiKeyBody::expires_at`'s HTTP
            // counterpart. A present-but-out-of-range timestamp is `InvalidTimestamp`
            // (SMA-586). `parse_opt_ts` is that exact absent/valid/unrepresentable split,
            // shared with the filter call sites (SMA-583). NOTE the HTTP twin diverges here
            // and that is deliberate: its `expires_at` is a typed `DateTime<Utc>` in the body,
            // so a malformed value fails inside serde and yields `invalid-request-body`, which
            // is the correct reason for a body that would not deserialize.
```

- [ ] **Step 8: Widen both `to_filter` entry points**

Task 8's parity guard drives the request-conversion entry points rather than the raw helpers,
so that it also proves each helper is still WIRED IN — an unwired helper is the failure SMA-583
actually hit, and a test calling the helper directly cannot catch it.

Change `fn to_filter` to `pub(crate) fn to_filter` in **both**
`grpc/audit.rs:89` and `grpc/dead_letters.rs:88`. Leave `to_bulk_request` private — Task 8 does
not drive it.

- [ ] **Step 9: Run tests to verify they pass**

Run: `cd rs && cargo test -p paigasus-iam --lib adapters::grpc`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/
git commit -m "feat(rs): migrate grpc timestamp, cursor and outcome sites off invalid-prn (SMA-586)"
```

---

### Task 5: Migrate the gRPC uuid sites and add the required-field checks

This is the gRPC half of D5.2 plus the D6 reclassification. It changes behaviour on three
sites that previously fell through to the PRN parser.

**Files:**
- Modify: `grpc/tenancy.rs:592` (uuid), `:611-615` (the oneof, D6)
- Modify: `grpc/authz.rs:217` (uuid), `:236` (required field, D5.2)
- Modify: `grpc/service_accounts.rs:213` (uuid), `:142` and `:177` (required fields, D5.2)
- Modify: `grpc/dead_letters.rs:113-115` (`parse_id`)

**Interfaces:**
- Consumes: Task 2's variants.
- Produces: `pub(crate) fn parse_id(raw: &str) -> Result<Uuid, TenancyError>` in `grpc::dead_letters` (widened for Task 8).

- [ ] **Step 1: Write the failing tests**

Add to `grpc/service_accounts.rs`'s test module:

```rust
/// SMA-586 D5.2: an empty required PRN field is `missing-required-field`, not a PRN parse
/// failure. Before this, an empty `owner_prn` fell through to `parse_node_prn` and answered
/// `invalid-prn` — while the HTTP twin answered `missing-required-field`, so the two
/// transports disagreed on the same logical failure.
#[test]
fn an_empty_owner_prn_is_a_missing_required_field() {
    assert_eq!(require_present("", "owner_prn").unwrap_err(), TenancyError::MissingRequiredField("owner_prn"));
    assert_eq!(require_present("   ", "owner_prn").unwrap_err(), TenancyError::MissingRequiredField("owner_prn"));
    assert_eq!(require_present("iam::org/x", "owner_prn").unwrap(), "iam::org/x");
}
```

Add to `grpc/tenancy.rs`'s test module:

```rust
/// SMA-586 D6: the `ListMembershipsRequest.filter` oneof cannot carry two values, so its
/// `None` arm means NEITHER field is set — which is `missing-required-field`. The old message
/// ("provide exactly one of …") described a failure the wire format makes impossible.
#[test]
fn an_absent_membership_filter_oneof_is_a_missing_required_field() {
    let err = membership_filter(None).unwrap_err();
    assert_eq!(err, TenancyError::MissingRequiredField("principal_prn|node_prn"));
    assert_eq!(err.code(), "missing-required-field");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd rs && cargo test -p paigasus-iam --lib adapters::grpc`
Expected: FAIL to compile — `cannot find function require_present` / `membership_filter`.

- [ ] **Step 3: Add the shared `require_present` helper**

In `grpc/convert.rs`, next to `parse_opt_ts`:

```rust
/// Rejects an absent required wire field before it reaches a parser that would mis-describe
/// the failure (SMA-586 D5.2).
///
/// proto3 has no absence for a plain `string`, so an unset field arrives as `""` — which
/// `Prn::parse` would report as a malformed PRN rather than as a missing one, diverging from
/// the HTTP twin where the param is genuinely `Option`. Whitespace counts as empty: a
/// `?owner_prn=%20` is not a PRN anyone meant to send.
pub(crate) fn require_present<'a>(raw: &'a str, field: &'static str) -> Result<&'a str, TenancyError> {
    if raw.trim().is_empty() {
        return Err(TenancyError::MissingRequiredField(field));
    }
    Ok(raw)
}
```

In `grpc/service_accounts.rs` and `grpc/authz.rs`, import it: `use super::convert::require_present;`
(or `use crate::adapters::grpc::convert::require_present;` to match each file's existing import style).

- [ ] **Step 4: Add the `membership_filter` helper in `grpc/tenancy.rs`**

Extract the oneof match out of the handler so it is unit-testable:

```rust
/// Maps the `ListMembershipsRequest.filter` oneof to a `MembershipFilter`.
///
/// A proto3 `oneof` cannot carry two values, so `None` means NEITHER field is set — which is
/// `missing-required-field`, not a conflict (SMA-586 D6). The HTTP twin, whose two query
/// params CAN both be present, is the only surface that can produce
/// `MutuallyExclusiveFields`.
pub(crate) fn membership_filter(filter: Option<list_memberships_request::Filter>) -> Result<MembershipFilter, TenancyError> {
    match filter {
        Some(list_memberships_request::Filter::PrincipalPrn(prn)) => Ok(MembershipFilter::Principal(prn)),
        Some(list_memberships_request::Filter::NodePrn(prn)) => Ok(MembershipFilter::Node(prn)),
        None => Err(TenancyError::MissingRequiredField("principal_prn|node_prn")),
    }
}
```

Then replace the inline `match req.filter { … }` at `:611-615` with:

```rust
            let filter = membership_filter(req.filter).map_err(convert::status_to_grpc)?;
```

- [ ] **Step 5: Migrate the four uuid sites**

- `grpc/tenancy.rs:592` → `TenancyError::InvalidUuid("membership_id")`
- `grpc/authz.rs:217` → `TenancyError::InvalidUuid("role_grant_id")`
- `grpc/service_accounts.rs:213` → `TenancyError::InvalidUuid("api_key_id")`
- `grpc/dead_letters.rs:114` → `TenancyError::InvalidUuid("dead_letter_id")`, and widen the fn
  to `pub(crate) fn parse_id`.

Each keeps its surrounding `.map_err(convert::status_to_grpc)` exactly as it is. Example, for
`grpc/tenancy.rs:592`:

```rust
            let id = Uuid::parse_str(&req.id).map_err(|_| convert::status_to_grpc(TenancyError::InvalidUuid("membership_id")))?;
```

Also fix the stale comment above it (`:584-587`), which explains the `InvalidPrn`-as-sentinel
reuse — replace it with:

```rust
            // `DetachMembershipRequest.id` is a bare uuid, not a PRN, so a malformed value is
            // `InvalidUuid` naming the segment (SMA-586). The field name reaches the client in
            // both the message and `ErrorInfo.metadata["field"]`.
```

Do the same for `grpc/authz.rs:214-216`. Do **not** touch `grpc/audit.rs` or
`grpc/service_accounts.rs:178-183` — Task 4 already fixed the stale prose there, and editing it
again risks reverting that work.

- [ ] **Step 6: Add the three required-field checks**

- `grpc/authz.rs:236` — before `self.state.roles.list(&actor, &req.principal_prn)`:
  ```rust
            let principal_prn = require_present(&req.principal_prn, "principal_prn").map_err(convert::status_to_grpc)?;
            let grants = self.state.roles.list(&actor, principal_prn).await.map_err(convert::status_to_grpc)?;
  ```
- `grpc/service_accounts.rs:142` — before `parse_node_prn(&req.owner_prn)`:
  ```rust
            let owner_prn = require_present(&req.owner_prn, "owner_prn").map_err(convert::status_to_grpc)?;
            let owner = parse_node_prn(owner_prn).map_err(convert::status_to_grpc)?;
  ```
- `grpc/service_accounts.rs:177` — before `parse_node_prn(&req.scope_prn)`:
  ```rust
            let scope_prn = require_present(&req.scope_prn, "scope_prn").map_err(convert::status_to_grpc)?;
            let scope = parse_node_prn(scope_prn).map_err(convert::status_to_grpc)?;
  ```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cd rs && cargo test -p paigasus-iam --lib adapters::grpc`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/
git commit -m "feat(rs): grpc uuid and required-field reasons, oneof reclassified (SMA-586)"
```

---

### Task 6: Migrate the HTTP query-param sites

**Files:**
- Modify: `http/audit.rs:47-81` (`parse_outcome`, `parse_cursor`, `parse_ts`) and `:100-103`
- Modify: `http/dead_letters.rs:62-77` (`parse_ts`, `parse_cursor`) and `:86-88`, `:103-104`
- Modify: `http/service_accounts.rs:71`, `http/api_keys.rs:85`, `http/authz.rs:145`
- Modify: `http/memberships.rs:93-97` (the D6 split)
- Modify: `http/dto.rs:181-186` (`MembershipQuery`, D7 normalisation)
- Modify (tests): `http/audit.rs:198,208,218`, `http/dead_letters.rs:220,227`

**Interfaces:**
- Consumes: Task 2's variants.
- Produces: `pub(crate) fn parse_ts(raw: Option<String>, field: &'static str)` in both `http::audit` and `http::dead_letters`; `pub(crate) fn parse_cursor(raw: Option<String>)` and `pub(crate) fn parse_outcome(raw: Option<String>)` in `http::audit`; `pub(crate) fn parse_cursor(raw: Option<String>)` in `http::dead_letters`; `pub(crate) fn membership_filter(principal: Option<String>, node: Option<String>) -> Result<MembershipFilter, TenancyError>` in `http::memberships`. Task 8 calls all of these.

- [ ] **Step 1: Write the failing tests**

Add to `http/memberships.rs`'s test module (create `#[cfg(test)] mod tests` at the end of the
file if none exists, with `use super::*;`):

```rust
/// SMA-586 D6: the two halves of the old `_ =>` catch-all are different client mistakes and
/// now get different reasons. Both were `invalid-prn` before, which is the same catch-all this
/// ticket removes, in miniature.
#[test]
fn the_membership_filter_distinguishes_neither_set_from_both_set() {
    // Reasons are pinned as ErrorReason values compared via `as_wire_reason()`, NEVER as bare
    // kebab literals. Two reasons: it routes the assertion through the registry, so an
    // unregistered rename fails here too; and a literal in a `src/` file would put this
    // production module on `ci/error-registry/check.py`'s MANIFEST, which would blind that gate
    // to a future *production* code literal anywhere in this file.
    use paigasus_proto::paigasus::common::v1::ErrorReason;
    let wire = |r: ErrorReason| r.as_wire_reason().expect("not the Unspecified sentinel");

    let neither = membership_filter(None, None).unwrap_err();
    assert_eq!(neither, TenancyError::MissingRequiredField("principal|node"));
    assert_eq!(neither.code(), wire(ErrorReason::MissingRequiredField));

    let both = membership_filter(Some("a".into()), Some("b".into())).unwrap_err();
    assert_eq!(both, TenancyError::MutuallyExclusiveFields("principal|node"));
    assert_eq!(both.code(), wire(ErrorReason::MutuallyExclusiveFields));

    assert!(matches!(membership_filter(Some("a".into()), None).unwrap(), MembershipFilter::Principal(_)));
    assert!(matches!(membership_filter(None, Some("b".into())).unwrap(), MembershipFilter::Node(_)));
}

/// SMA-586 D7: `?principal=` (present but empty) means the same thing as an absent param.
/// Without this, D5.2's gRPC repair — where proto3's empty string IS the unset sentinel —
/// would make the two transports disagree again.
#[test]
fn an_empty_membership_param_is_treated_as_absent() {
    assert_eq!(membership_filter(Some(String::new()), None).unwrap_err(), TenancyError::MissingRequiredField("principal|node"));
}
```

Add to `http/audit.rs`'s test module:

```rust
/// SMA-586: each audit query param now names itself, and the two timestamp bounds are
/// distinguishable — `from` and `to` used to yield the identical response.
#[test]
fn the_audit_query_params_each_get_their_own_reason() {
    assert_eq!(parse_ts(Some("nope".into()), "from").unwrap_err(), TenancyError::InvalidTimestamp("from"));
    assert_eq!(parse_ts(Some("nope".into()), "to").unwrap_err(), TenancyError::InvalidTimestamp("to"));
    assert_eq!(parse_cursor(Some("nope".into())).unwrap_err(), TenancyError::InvalidCursor("cursor"));
    assert_eq!(parse_outcome(Some("maybe".into())).unwrap_err(), TenancyError::InvalidAuditOutcome("outcome"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd rs && cargo test -p paigasus-iam --lib adapters::http`
Expected: FAIL to compile — `cannot find function membership_filter`, and `parse_ts` takes one
argument.

- [ ] **Step 3: Migrate `http/audit.rs`**

Replace the three helpers (dropping the now-false "no dedicated error code" prose):

```rust
/// Parses the `outcome` query param: absent/empty means unfiltered; a present value must name
/// a known [`AuditOutcome`]. The caller's raw value is not echoed back — `InvalidAuditOutcome`
/// carries a `&'static str`, which cannot hold it (SMA-586).
pub(crate) fn parse_outcome(raw: Option<String>) -> Result<Option<AuditOutcome>, TenancyError> {
    match opt_non_empty(raw) {
        None => Ok(None),
        Some(s) => AuditOutcome::parse(&s).map(Some).ok_or(TenancyError::InvalidAuditOutcome("outcome")),
    }
}

/// Parses the `cursor` query param: absent/empty means "first page" (`None`); a present value
/// must be a valid uuid. `InvalidCursor` rather than `InvalidUuid` — a cursor is server-issued,
/// so a client recovers by restarting pagination (SMA-586). Mirrors `grpc::audit::parse_cursor`.
pub(crate) fn parse_cursor(raw: Option<String>) -> Result<Option<Uuid>, TenancyError> {
    match opt_non_empty(raw) {
        None => Ok(None),
        Some(s) => Uuid::parse_str(&s).map(Some).map_err(|_| TenancyError::InvalidCursor("cursor")),
    }
}

/// Parses an RFC3339 `from`/`to` query param: absent/empty means unfiltered; a present value
/// must parse as RFC3339. `field` names which bound failed and reaches the client (SMA-586) —
/// before, both bounds produced the identical static message.
///
/// The gRPC twin has the SAME three-case split: a `prost_types::Timestamp` cannot fail to
/// *parse*, but it can fail to *convert*, and `grpc::audit::to_filter` rejects that via
/// `convert::parse_opt_ts` exactly as this does (SMA-583).
pub(crate) fn parse_ts(raw: Option<String>, field: &'static str) -> Result<Option<DateTime<Utc>>, TenancyError> {
    match opt_non_empty(raw) {
        None => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(&s)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|_| TenancyError::InvalidTimestamp(field)),
    }
}
```

Update the two call sites at `:101-102`:

```rust
        from: parse_ts(q.from, "from")?,
        to: parse_ts(q.to, "to")?,
```

Change `fn to_filter` to `pub(crate) fn to_filter` (`http/audit.rs:95`) — Task 8 drives it
rather than the raw helpers, so that the guard also proves the helper is still wired in.

- [ ] **Step 4: Migrate `http/dead_letters.rs`**

Same shape:

```rust
/// Absent/empty means unfiltered; a present value must parse as RFC3339. `field` names which
/// bound failed (SMA-586). Mirrors `http::audit::parse_ts` exactly.
pub(crate) fn parse_ts(raw: Option<String>, field: &'static str) -> Result<Option<DateTime<Utc>>, TenancyError> {
    match opt_non_empty(raw) {
        None => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(&s)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|_| TenancyError::InvalidTimestamp(field)),
    }
}

pub(crate) fn parse_cursor(raw: Option<String>) -> Result<Option<Uuid>, TenancyError> {
    match opt_non_empty(raw) {
        None => Ok(None),
        Some(s) => Uuid::parse_str(&s).map(Some).map_err(|_| TenancyError::InvalidCursor("cursor")),
    }
}
```

Update all four call sites (`:86,87` and `:103,104`) to pass `"parked_from"` / `"parked_to"`,
and change `fn to_filter` to `pub(crate) fn to_filter` (`http/dead_letters.rs:83`).

- [ ] **Step 5: Migrate the three required-field sites**

- `http/service_accounts.rs:71` → `.ok_or(TenancyError::MissingRequiredField("owner_prn"))?`
- `http/api_keys.rs:85` → `.ok_or(TenancyError::MissingRequiredField("scope_prn"))?`
- `http/authz.rs:145` → `.ok_or(TenancyError::MissingRequiredField("principal_prn"))?`

Each was `.ok_or_else(|| TenancyError::InvalidPrn(…))`; `ok_or` replaces it since there is no
allocation left to defer. Apply D7 to each by filtering empties first, e.g. for
`http/service_accounts.rs:71`:

```rust
    let owner_prn = q.owner_prn.filter(|s| !s.trim().is_empty()).ok_or(TenancyError::MissingRequiredField("owner_prn"))?;
```

- [ ] **Step 6: Split the membership filter (D6)**

In `http/memberships.rs`, add the helper and call it from the handler:

```rust
/// Maps the two mutually-exclusive query params to a `MembershipFilter`.
///
/// The old single `_ =>` arm folded "neither set" and "both set" into one reason, which is the
/// catch-all this ticket removes, in miniature (SMA-586 D6). An empty string counts as absent
/// (D7), matching the gRPC surface where proto3's empty string IS the unset sentinel.
///
/// Unlike gRPC, this surface CAN receive both — its two query params are independent, where
/// the wire models the same choice as a `oneof`. So `MutuallyExclusiveFields` is emitted here
/// and nowhere else in the service.
pub(crate) fn membership_filter(principal: Option<String>, node: Option<String>) -> Result<MembershipFilter, TenancyError> {
    let principal = principal.filter(|s| !s.trim().is_empty());
    let node = node.filter(|s| !s.trim().is_empty());
    match (principal, node) {
        (Some(principal), None) => Ok(MembershipFilter::Principal(principal)),
        (None, Some(node)) => Ok(MembershipFilter::Node(node)),
        (None, None) => Err(TenancyError::MissingRequiredField("principal|node")),
        (Some(_), Some(_)) => Err(TenancyError::MutuallyExclusiveFields("principal|node")),
    }
}
```

Replace the handler's inline match at `:93-97` with:

```rust
    let filter = membership_filter(q.principal, q.node)?;
```

Update the module doc at `:6-7`, which currently names
`TenancyError::InvalidPrn("provide exactly one of principal|node")`.

- [ ] **Step 7: Retarget the HTTP test assertions**

`http/audit.rs:198,208,218` and `http/dead_letters.rs:220,227` assert
`TenancyError::InvalidPrn(_)`. Change each to the variant its test now produces
(`InvalidAuditOutcome`, `InvalidCursor` or `InvalidTimestamp` — read the test name).

- [ ] **Step 8: Fix the four stale `http/dto.rs` doc comments**

Lines 179, 364, 414 and 457 each explain that a field is `Option` so a missing value "funnels
through `TenancyError::InvalidPrn`". Replace `InvalidPrn` with `MissingRequiredField` in each,
and at `:179` (`MembershipQuery`) note that both-set now yields `MutuallyExclusiveFields`.

- [ ] **Step 9: Run tests to verify they pass**

Run: `cd rs && cargo test -p paigasus-iam --lib adapters::http`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/http/
git commit -m "feat(rs): migrate http query-param sites off the invalid-prn sentinel (SMA-586)"
```

---

### Task 7: The `UuidPath` extractor (D5.1, all 26 path-param sites)

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/http/path.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` (declare the module)
- Modify: `organizations.rs` (6), `teams.rs` (6), `projects.rs` (4), `service_accounts.rs` (2), `api_keys.rs` (3), `memberships.rs` (1), `authz.rs` (1), `dead_letters.rs` (2)

**Interfaces:**
- Consumes: `TenancyError::InvalidUuid` from Task 2; `ApiError` from `http::error`.
- Produces:
  - `pub(crate) trait PathField { const NAME: &'static str; }`
  - Marker types `OrganizationId`, `TeamId`, `ProjectId`, `ServiceAccountId`, `ApiKeyId`, `MembershipId`, `RoleGrantId`, `DeadLetterId`, each implementing it.
  - `pub(crate) struct UuidPath<F: PathField> { pub id: Uuid, .. }` and
    `pub(crate) struct UuidPathPair<F: PathField> { pub first: Uuid, pub second: Uuid, .. }`, both `FromRequestParts`.

**Why a type marker rather than a runtime extension.** `&'static str` is not a stable
const-generic parameter, so the field name has to travel some other way. A request extension set
by a route-level layer works, but a route that forgets the layer silently falls back to a wrong
name — a runtime failure in the exact code that exists to make failures legible. Binding the name
to a marker type puts it in the handler signature instead, so a route cannot compile without one.
It also needs no new dependency: `tower-http` here is pinned to `["trace", "timeout"]` and has no
`add-extension`.

- [ ] **Step 1: Write the failing test**

Create `rs/crates/services/paigasus-iam/src/adapters/http/path.rs` with only its test module
first:

```rust
// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::routing::get;
    use tower::ServiceExt;

    async fn ok(path: UuidPath<MembershipId>) -> String {
        path.id.to_string()
    }

    /// SMA-586 D5.1: a malformed uuid path segment answers inside the error envelope, with
    /// `invalid-uuid`. Before this, axum's own `Path<Uuid>` rejection produced a plain-text
    /// 400 that was not the `{"error":{code,message}}` contract at all — on all 26 routes.
    #[tokio::test]
    async fn a_malformed_uuid_segment_answers_in_the_error_envelope() {
        let app = Router::new().route("/x/{id}", get(ok));
        let resp = app
            .oneshot(axum::http::Request::builder().uri("/x/not-a-uuid").body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "invalid-uuid");
        assert_eq!(body["error"]["message"], "membership_id must be a uuid");
    }

    /// A well-formed uuid still reaches the handler unchanged.
    #[tokio::test]
    async fn a_well_formed_uuid_segment_extracts() {
        let id = uuid::Uuid::new_v4();
        let app = Router::new().route("/x/{id}", get(ok));
        let resp = app
            .oneshot(axum::http::Request::builder().uri(format!("/x/{id}")).body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), id.to_string());
    }

    /// Every marker's NAME is the literal the route's error should carry. Pinned so a rename
    /// that drifts from the URL segment it describes fails here.
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
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd rs && cargo test -p paigasus-iam --lib adapters::http::path`
Expected: FAIL to compile — the module is not declared and `UuidPath` does not exist.

- [ ] **Step 3: Implement the extractor**

Write the module body above the test:

```rust
// SPDX-License-Identifier: Apache-2.0

//! A uuid path-segment extractor that answers inside IAM's error envelope (SMA-586 D5.1).
//!
//! axum's own `Path<Uuid>` rejects a malformed segment with a plain-text 400 that is not the
//! `{"error":{code,message}}` contract every other IAM failure uses — so 26 routes answered
//! outside their own error contract, and `invalid-uuid` had no HTTP emitter at all while its
//! gRPC twins did (AC-1). This closes both.
//!
//! Mirrors `authn::EnvelopeJson`, which already does exactly this for `Json<T>` rejections.
//!
//! The field name is carried by a MARKER TYPE rather than a request extension: `&'static str`
//! is not a stable const-generic parameter, and an extension set by a route-level layer would
//! let a route that forgets the layer report a wrong name at runtime. A marker puts the name
//! in the handler signature, so a route cannot compile without choosing one.

use std::marker::PhantomData;

use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::adapters::http::error::ApiError;
use crate::application::error::TenancyError;

/// Names the wire field a uuid path segment stands for, for `TenancyError::InvalidUuid`.
pub(crate) trait PathField {
    const NAME: &'static str;
}

/// Declares a marker type and its wire field name.
macro_rules! path_field {
    ($(#[$m:meta])* $name:ident => $wire:literal) => {
        $(#[$m])*
        pub(crate) struct $name;
        impl PathField for $name {
            const NAME: &'static str = $wire;
        }
    };
}

path_field!(/// `{id}` on an organization route.
    OrganizationId => "organization_id");
path_field!(/// `{id}` or `{team_id}` on a team route.
    TeamId => "team_id");
path_field!(/// `{id}` on a project route.
    ProjectId => "project_id");
path_field!(/// `{sa}` — a service account's bare uuid.
    ServiceAccountId => "service_account_id");
path_field!(/// `{id}` on an api-key route.
    ApiKeyId => "api_key_id");
path_field!(/// `{id}` on a membership route.
    MembershipId => "membership_id");
path_field!(/// `{id}` on a role-grant route.
    RoleGrantId => "role_grant_id");
path_field!(/// `{id}` on a dead-letter route.
    DeadLetterId => "dead_letter_id");

/// A single uuid path segment, reported as `F::NAME` when it is malformed.
pub(crate) struct UuidPath<F: PathField> {
    pub id: Uuid,
    _marker: PhantomData<F>,
}

impl<S: Send + Sync, F: PathField> FromRequestParts<S> for UuidPath<F> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<Uuid>::from_request_parts(parts, state).await {
            Ok(Path(id)) => Ok(UuidPath { id, _marker: PhantomData }),
            Err(_) => Err(ApiError(TenancyError::InvalidUuid(F::NAME)).into_response()),
        }
    }
}

/// Two uuid path segments, for `/{sa}/api-keys/{id}`.
///
/// Both are reported under the SAME field name: axum's `PathRejection` does not say WHICH
/// segment failed, and inventing one would be a guess presented as fact.
pub(crate) struct UuidPathPair<F: PathField> {
    pub first: Uuid,
    pub second: Uuid,
    _marker: PhantomData<F>,
}

impl<S: Send + Sync, F: PathField> FromRequestParts<S> for UuidPathPair<F> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<(Uuid, Uuid)>::from_request_parts(parts, state).await {
            Ok(Path((first, second))) => Ok(UuidPathPair { first, second, _marker: PhantomData }),
            Err(_) => Err(ApiError(TenancyError::InvalidUuid(F::NAME)).into_response()),
        }
    }
}
```

Declare it in `http/mod.rs` alongside the other modules: `pub(crate) mod path;`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd rs && cargo test -p paigasus-iam --lib adapters::http::path`
Expected: PASS.

- [ ] **Step 5: Swap all 26 call sites**

For each handler, replace `Path(x): Path<Uuid>` with `path: UuidPath<Marker>` and use `path.id`
in the body; replace `Path((a, b)): Path<(Uuid, Uuid)>` with `path: UuidPathPair<ApiKeyId>` and
use `path.first` / `path.second`. No route-builder change is needed — the marker lives entirely
in the handler signature.

| File and lines | Marker |
|---|---|
| `organizations.rs:71,79,88,97,106,115` | `OrganizationId` |
| `teams.rs:39,47,56,65,77,88` | `TeamId` |
| `projects.rs:32,40,49,58` | `ProjectId` |
| `service_accounts.rs:78,89` | `ServiceAccountId` |
| `api_keys.rs:80,98` | `ServiceAccountId` |
| `api_keys.rs:113` | `ApiKeyId` (the pair; `first` is the sa, `second` the key) |
| `memberships.rs:82` | `MembershipId` |
| `authz.rs:150` | `RoleGrantId` |
| `dead_letters.rs:126,132` | `DeadLetterId` |

Worked example — `memberships.rs:82`:

```rust
async fn delete_membership(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<MembershipId>) -> Result<StatusCode, ApiError> {
    let id = path.id;
    if s.enforce_tenancy {
        let record = s.memberships.get(id).await?;
```

- [ ] **Step 6: Run the full lib test suite**

Run: `cd rs && cargo test -p paigasus-iam --lib`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/http/
git commit -m "feat(rs): answer a malformed uuid path segment inside the error envelope (SMA-586)"
```

---

### Task 8: The AC-3 transport parity guard

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` (test module)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` (widen three modules)

**Interfaces:**
- Consumes: every `pub(crate)` helper Tasks 4-7 produced.
- Produces: nothing consumed later.

- [ ] **Step 1: Widen the three private HTTP modules**

In `http/mod.rs`, change `mod audit;` → `pub(crate) mod audit;`, `mod dead_letters;` →
`pub(crate) mod dead_letters;`, `mod memberships;` → `pub(crate) mod memberships;`. Add a
comment above them:

```rust
// `pub(crate)` rather than private so `grpc::convert`'s transport parity guard can drive these
// modules' request-conversion helpers directly (SMA-586 AC-3). `dto` is already `pub` for the
// same reason — the existing dead-letter projection drift guard.
```

- [ ] **Step 2: Write the failing test**

Add to `grpc/convert.rs`'s test module:

```rust
/// AC-3: HTTP and gRPC agree on the reason for the same logical failure.
///
/// Both transports derive `reason` from the SAME function (`TenancyError::code()`), so for a
/// GIVEN variant they cannot disagree. Parity can only break where the two sides CONSTRUCT
/// DIFFERENT VARIANTS — so this drives each transport's request-conversion entry point
/// (`to_filter`) rather than comparing two calls to `code()`, which would be tautological.
/// Driving `to_filter` also proves each helper is still WIRED IN, which is the failure SMA-583
/// actually hit: a helper that exists and is correct but no longer reached.
///
/// Two rows call a helper directly, because those surfaces have no filter-shaped entry point:
/// the dead-letter id parser, and both `membership_filter`s.
#[test]
fn http_and_grpc_agree_on_the_reason_for_the_same_failure() {
    use paigasus_proto::paigasus::common::v1::ErrorReason;

    use crate::adapters::grpc::{audit as gaudit, dead_letters as gdl, tenancy as gtenancy};
    use crate::adapters::http::{audit as haudit, dead_letters as hdl, memberships as hmem};
    use crate::adapters::http::dto::{AuditQuery, DeadLetterQuery};
    use paigasus_proto::paigasus::iam::v1::{ListAuditEntriesRequest, ListDeadLettersRequest};

    // A `nanos` of -1 is unrepresentable in chrono, which is how a gRPC timestamp fails — it
    // cannot fail to PARSE (it is already a struct), only to CONVERT.
    let bad_ts = prost_types::Timestamp { seconds: 0, nanos: -1 };

    fn audit_req() -> ListAuditEntriesRequest {
        ListAuditEntriesRequest {
            actor_prn: String::new(),
            resource_prn: String::new(),
            action: String::new(),
            outcome: String::new(),
            from: None,
            to: None,
            cursor: String::new(),
            limit: 0,
        }
    }
    fn audit_query() -> AuditQuery {
        AuditQuery { actor: None, resource: None, action: None, outcome: None, from: None, to: None, cursor: None, limit: None }
    }
    fn dl_req() -> ListDeadLettersRequest {
        ListDeadLettersRequest { event_type: String::new(), parked_from: None, parked_to: None, cursor: String::new(), limit: 0 }
    }
    fn dl_query() -> DeadLetterQuery {
        DeadLetterQuery { event_type: None, parked_from: None, parked_to: None, cursor: None, limit: None }
    }

    let cases: Vec<(&str, TenancyError, TenancyError, ErrorReason)> = vec![
        (
            "audit from-bound",
            haudit::to_filter(AuditQuery { from: Some("not-a-timestamp".into()), ..audit_query() }).unwrap_err(),
            gaudit::to_filter(ListAuditEntriesRequest { from: Some(bad_ts), ..audit_req() }).unwrap_err(),
            ErrorReason::InvalidTimestamp,
        ),
        (
            "audit cursor",
            haudit::to_filter(AuditQuery { cursor: Some("not-a-uuid".into()), ..audit_query() }).unwrap_err(),
            gaudit::to_filter(ListAuditEntriesRequest { cursor: "not-a-uuid".into(), ..audit_req() }).unwrap_err(),
            ErrorReason::InvalidCursor,
        ),
        (
            "audit outcome",
            haudit::to_filter(AuditQuery { outcome: Some("not-a-real-outcome".into()), ..audit_query() }).unwrap_err(),
            gaudit::to_filter(ListAuditEntriesRequest { outcome: "not-a-real-outcome".into(), ..audit_req() }).unwrap_err(),
            ErrorReason::InvalidAuditOutcome,
        ),
        (
            "dead-letter parked_from bound",
            hdl::to_filter(DeadLetterQuery { parked_from: Some("not-a-timestamp".into()), ..dl_query() }).unwrap_err(),
            gdl::to_filter(ListDeadLettersRequest { parked_from: Some(bad_ts), ..dl_req() }).unwrap_err(),
            ErrorReason::InvalidTimestamp,
        ),
        (
            "dead-letter cursor",
            hdl::to_filter(DeadLetterQuery { cursor: Some("not-a-uuid".into()), ..dl_query() }).unwrap_err(),
            gdl::to_filter(ListDeadLettersRequest { cursor: "not-a-uuid".into(), ..dl_req() }).unwrap_err(),
            ErrorReason::InvalidCursor,
        ),
        (
            // No filter-shaped entry point on either side: HTTP takes this segment through the
            // `UuidPath<DeadLetterId>` extractor, gRPC through `parse_id`.
            "dead-letter id",
            TenancyError::InvalidUuid("dead_letter_id"),
            gdl::parse_id("not-a-uuid").unwrap_err(),
            ErrorReason::InvalidUuid,
        ),
        (
            "membership filter, neither set",
            hmem::membership_filter(None, None).unwrap_err(),
            gtenancy::membership_filter(None).unwrap_err(),
            ErrorReason::MissingRequiredField,
        ),
    ];

    for (label, http_err, grpc_err, expected) in cases {
        let wire = expected.as_wire_reason().expect("not the Unspecified sentinel");
        assert_eq!(http_err.code(), wire, "{label}: HTTP reason");
        assert_eq!(grpc_err.code(), wire, "{label}: gRPC reason");
    }
}
```

```rust
/// The counterpart to the agreement table: the two places the transports DELIBERATELY differ.
/// Recorded as assertions so a change breaks this test rather than slipping through as an
/// omission — the failure mode the SMA-586 spec review caught in its own first draft.
#[test]
fn the_accepted_transport_divergences_are_exactly_these_two() {
    use paigasus_proto::paigasus::common::v1::ErrorReason;

    // 1. `IssueApiKey.expires_at`. gRPC takes a `prost_types::Timestamp` and classifies a bad
    //    one itself; HTTP takes a typed `DateTime<Utc>` in the body, so a bad value fails
    //    inside serde and never reaches our code — yielding `invalid-request-body`, which is
    //    the registry's correct reason for a body that would not deserialize. Making it
    //    `invalid-timestamp` would need a custom deserializer for no contract gain.
    let wire = |r: ErrorReason| r.as_wire_reason().expect("not the Unspecified sentinel");
    assert_eq!(
        parse_opt_ts(Some(prost_types::Timestamp { seconds: 0, nanos: -1 }), "expires_at").unwrap_err().code(),
        wire(ErrorReason::InvalidTimestamp),
    );

    // 2. `mutually-exclusive-fields` is HTTP-only and STRUCTURALLY so: the gRPC surface models
    //    the same choice as a proto3 `oneof`, which cannot carry two values. Its only failure
    //    is "neither set", asserted as `missing-required-field` in the table above.
    use crate::adapters::http::memberships::membership_filter;
    assert_eq!(membership_filter(Some("a".into()), Some("b".into())).unwrap_err().code(), wire(ErrorReason::MutuallyExclusiveFields));
    // There is no gRPC expression of "both set" to compare against — that is the point.
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cd rs && cargo test -p paigasus-iam --lib http_and_grpc_agree`
Expected: FAIL to compile if any helper from Tasks 4-7 was left private. Fix the visibility of
whichever one the compiler names — do **not** work around it by duplicating the logic.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd rs && cargo test -p paigasus-iam --lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/
git commit -m "test(rs): assert http and grpc agree on each validation reason (SMA-586)"
```

---

### Task 9: Update the Docker-backed integration tests

These assert on runtime JSON, so they compile fine and fail only when run. They are the
failure the spec review caught.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/http_memberships.rs:144-156`
- Modify: `rs/crates/services/paigasus-iam/tests/http_audit.rs:98-110`

**Interfaces:** none produced.

- [ ] **Step 1: Update `tests/http_memberships.rs`**

The two cases now assert different codes. Change line 150's assertion to:

```rust
    assert_eq!(err["error"]["code"], "missing-required-field");
```

and line 156's to:

```rust
    assert_eq!(err["error"]["code"], "mutually-exclusive-fields");
```

Update the comments at `:144` and `:152` — `:144` currently says "Neither `principal` nor
`node` set: 400 `invalid-prn`. (`TenancyError::InvalidPrn`'s …" and `:152` says "Both set: 400
`invalid-prn`." Replace with:

```rust
    // Neither `principal` nor `node` set: 400 `missing-required-field`. These two cases used
    // to share one reason (`invalid-prn`); SMA-586 D6 split them, because "you omitted a
    // filter" and "you sent two" are different mistakes with different fixes.
```
```rust
    // Both set: 400 `mutually-exclusive-fields`. HTTP is the only surface that can produce
    // this — the gRPC twin models the choice as a `oneof`, which cannot carry two values.
```

- [ ] **Step 2: Update `tests/http_audit.rs`**

Line 109 asserts `invalid-prn` for `GET /v1/audit?cursor=not-a-uuid`. Change to:

```rust
    assert_eq!(body["error"]["code"], "invalid-cursor");
```

Add a line above it:

```rust
    // `invalid-cursor`, not `invalid-uuid`: a cursor is server-issued, so a client can recover
    // by restarting pagination rather than asking the user to fix input (SMA-586 D1).
```

- [ ] **Step 3: Confirm `tests/http_authz.rs:222` is untouched**

Read it. Its request body is `{"principal_prn": "not-a-prn", …}`, which reaches the genuine PRN
parser at `http/authz.rs:72` — **not** the required-field check at `:145`. It must keep
asserting `invalid-prn`. Do not change it. This is AC-2's evidence in test form.

- [ ] **Step 4: Run the integration tests**

Docker must be running.

```bash
cd rs && env -u CI cargo nextest run -p paigasus-iam --test http_memberships --test http_audit --test http_authz
```

Note: because this is a **filtered** run, the `docker_preflight` canary is not in the filter, so
set `PAIGASUS_REQUIRE_DOCKER=1` to turn a silent skip into a panic:

```bash
cd rs && PAIGASUS_REQUIRE_DOCKER=1 env -u CI cargo nextest run -p paigasus-iam --test http_memberships --test http_audit --test http_authz
```

Expected: PASS. A `postgres did not accept connections within 60s` failure is flakiness under
parallel load, not your change — re-run before investigating.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/
git commit -m "test(rs): retarget the integration assertions at the new reasons (SMA-586)"
```

---

### Task 10: Full-graph verification

No code changes expected. If a gate reds, fix it here.

**Files:** possibly `ci/error-registry/check.py` (only if the gate demands a MANIFEST row).

- [ ] **Step 1: Run the whole IAM suite with Docker**

```bash
cd rs && env -u CI cargo nextest run -p paigasus-iam
```

Expected: PASS, with `docker_preflight` green (proving Docker was actually reachable and the
64 container-backed binaries really ran).

- [ ] **Step 2: Run the full CI graph**

From the repo root, exactly as CI does:

```bash
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity \
  :release-parity-py :release-parity-ts :publish-metadata :version-lockstep \
  --base origin/main --include-relations
```

Expected: all green. If Moon reports an unattributed "N failed", find the culprit with:

```bash
jq '.actions[] | select(.status=="failed") | .label' .moon/cache/ciReport.json
```

Gate notes:
- `:error-code-single-site` should pass with no MANIFEST change: the six codes appear as
  literals only in `application/error.rs` (already an `emits` row) and in `tests/` (not
  scanned). If it names a new offender, add a MANIFEST row with a stated reason — do not
  work around it.
- `:breaking` must be clean; six added enum values are additive.
- `:fmt` covers `cargo fmt`. Run `cargo fmt` in `rs/` if it complains.

- [ ] **Step 3: Verify no codegen drift**

```bash
(cd contracts && buf format -w && buf generate)
git add --intent-to-add . && git diff --exit-code
```

Expected: exit 0. Note the subshell: buf must run from `contracts/` (no root-level
`buf.gen.yaml`), while the drift check runs from the repo root. This is a **standalone**
`ci.yml` step, not part of the Moon command above.

- [ ] **Step 4: Verify the migration is complete**

```bash
grep -rn 'InvalidPrn' rs/crates/services/paigasus-iam/src/ | grep -v 'Prn::parse\|canonical()\|InvalidNodePrn'
```

Expected: only genuine PRN sites remain — `application/error.rs` (the variant, its `code()`,
`class()` and `From<DomainError>` arms) and the `Prn::parse` helpers listed in the spec's
"Sites that keep `InvalidPrn`" section. Any survivor in `audit.rs`, `dead_letters.rs` or a
required-field position is a missed migration.

- [ ] **Step 5: Commit any fixes**

```bash
git add -A
git commit -m "fix(rs): green the full ci graph for the error-reason taxonomy (SMA-586)"
```

Skip this step if nothing changed.

---

## Self-review notes

**Spec coverage.** D1 → Task 1. D2 → Tasks 2, 3. D3 → verified in Task 10 Step 4. D4 → Task 8.
D5.1 → Task 7. D5.2 → Task 5 Steps 3, 6. D6 → Tasks 5 (gRPC), 6 (HTTP), 9. D7 → Tasks 5, 6.
Registry mechanics → Task 1. Module visibility → Tasks 4-8. "Comments that must change" →
distributed across Tasks 4-6 at the site being touched, which is where the author can see
whether the prose is still true. AC-1 → Tasks 4-7. AC-2 → Task 10 Step 4. AC-3 → Task 8.
AC-4 → free, verified in Task 2 Step 7. AC-5 → Task 10 Step 2.

**Task 7 dependency check, resolved.** `tower-http` is pinned to `["trace", "timeout"]` in
`rs/Cargo.toml:26`, so `AddExtensionLayer` is not available. The marker-type design needs no new
dependency and is the better shape anyway: a route cannot compile without choosing a field name,
where a layer-based one could silently report a wrong name at runtime.

**Task ordering.** Tasks 1-3 are prerequisites for everything. Tasks 4-7 are independent of
each other and could run in parallel if their files do not overlap — but 4 and 5 both touch
`grpc/`, and 6 and 7 both touch `http/`, so run them in order to avoid conflicts. Task 8
requires 4-7. Task 9 requires 5-7. Task 10 requires all.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
