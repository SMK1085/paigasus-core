# SMA-501 — IAM ops proto services Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the three HTTP-only IAM operator surfaces (`POST /v1/users`, the four
`/v1/outbox/dead-letters` routes, and `POST /v1/authz/system-policies/{id}/retire`) a proto
contract and gRPC handlers that reuse the existing application layer.

**Architecture:** Additive contract change in `contracts/proto/paigasus/iam/v1/iam.proto` — two
new services (`UserService`, `OutboxService`) plus one RPC on the existing
`AuthorizationService`. Each gRPC handler is a thin adapter over an existing application service
(`AppState.users` / `.dead_letters` / `.retirement`); no business logic moves into the adapter
layer, and the HTTP handlers keep their current behavior unchanged.

**Tech Stack:** protobuf + buf (prost/tonic for Rust, betterproto2 for Python, protobuf-es for
TypeScript), Rust edition 2024 / rust-version 1.95, tonic, axum, SeaORM, cargo-nextest, Moon.

**Spec:** `docs/superpowers/specs/2026-08-22-sma-501-iam-ops-proto-services-design.md` — read it
before starting. Every decision reference below (D0, D3, D5, D6, D10 …) points into that file.

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Rust crates are **edition 2024 + rust-version 1.95**.
- The Rust workspace is `warnings = deny`. **Dead code is a hard compile error on the lib
  target**, so never add a private helper in one task and wire it up in a later one. Every task
  below is drawn so its new code is reachable when the task ends.
- Conventional commits with a workspace scope: `feat(contracts):`, `feat(rs):`, `docs(rs):`.
  Subject must **start lowercase** and the header must be **≤100 chars**. Never put a bare
  `#NNN` or a `token: value` line in the commit **body** — commitlint fails `footer-leading-blank`.
- Prefix every shell command with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `moon`/`buf`/`cargo-nextest`
  resolve to the repo-pinned versions.
- **Never spell a registry error code inside double quotes** in any file added or edited by this
  plan. `ci/error-registry/check.py` scans whole files with no production/test split, so writing
  `"invalid-bulk-replay"` (or `"grants-survive"` / `"decision-change-unacknowledged"`) in a doc
  comment or a proto comment drags that file onto the MANIFEST and reds
  `repo:error-code-single-site`. Write them unquoted, as `http/dead_letters.rs:134` already does.
- Docker-backed tests use the shared policy in `rs/crates/services/paigasus-iam/tests/support/docker.rs`.
  Never hand-roll a skip — `repo:iam-docker-policy-single-site` fails on a second copy.
- Run tests **in the foreground**. Do not background a build or test and end your turn waiting.

---

### Task 1: The contract

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto` (append after the `AuditService` block at the end; edit the file header at lines 9-16)
- Regenerate (commit the output, do not hand-edit): `rs/crates/libs/paigasus-proto/src/generated/paigasus/iam/v1/*`, `py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/iam/v1/__init__.py`, `ts/packages/paigasus-proto/src/generated/paigasus/iam/v1/iam_pb.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: the generated Rust types every later task imports —
  `paigasus_proto::paigasus::iam::v1::{CreateUserRequest, CreateUserResponse, user_service_server::{UserService, UserServiceServer}, RetireSystemPolicyRequest, RetireSystemPolicyResponse, RetiredPolicy, RetirementBlocked, RetirementNeedsAcknowledgement, SurvivingGrant, retire_system_policy_response::Outcome, DeadLetterEntry, ListDeadLettersRequest, ListDeadLettersResponse, ReplayDeadLetterRequest, ReplayDeadLetterResponse, DiscardDeadLetterRequest, DiscardDeadLetterResponse, BulkReplayDeadLettersRequest, BulkReplayDeadLettersResponse, outbox_service_server::{OutboxService, OutboxServiceServer}}`.

- [ ] **Step 1: Append the new messages and services to `iam.proto`**

Append this verbatim at the end of the file. Note the comment wording: registry codes appear
**unquoted** on purpose (Global Constraints).

```proto
// ─────────────────────────────────────────────────────────────────────────
// Users (SMA-501). CreateUser has NO per-action authorization — it requires
// a bearer and nothing more, mirroring `POST /v1/users` exactly. That is a
// deliberate parity decision, not an oversight: see the design doc's D0.
// It lives on its own service rather than on TenancyService precisely so
// that property is visible in the contract instead of hidden among 21
// authorized RPCs.
// ─────────────────────────────────────────────────────────────────────────
message CreateUserRequest {
  string email = 1;
  string display_name = 2;
  // Empty means unset. This DIVERGES from HTTP, where `{"locale": ""}`
  // persists an empty string rather than NULL (design doc D11). The proto
  // sentinel is normative for gRPC; HTTP is unchanged.
  string locale = 3;
  string timezone = 4;
}
message CreateUserResponse {
  string principal_prn = 1;
}

service UserService {
  rpc CreateUser(CreateUserRequest) returns (CreateUserResponse);
}

// ─────────────────────────────────────────────────────────────────────────
// System-policy retirement (SMA-501), mirroring
// POST /v1/authz/system-policies/{id}/retire.
// ─────────────────────────────────────────────────────────────────────────
message RetireSystemPolicyRequest {
  string policy_id = 1;
  // Absent == false == "not acknowledged". The safe reading of an
  // unspecified acknowledgement is "no", so the flag must be set
  // deliberately to take effect.
  bool acknowledge_decision_change = 2;
}

// One grant blocking a role retirement. Deliberately NOT the existing
// RoleGrant: this mirrors the HTTP body's three fields exactly, and
// role_key is carried once on RetirementBlocked rather than repeated.
message SurvivingGrant {
  string id = 1;
  string principal_prn = 2;
  string scope_prn = 3;
}

message RetiredPolicy {
  string policy_id = 1;
  string kind = 2; // "static" or "template"
  bool role_deleted = 3;
}

// A refusal, not an error: nothing was written because grants of this role
// survive and must be revoked first. `role_key` is a gRPC-only field — the
// HTTP twin carries it only inside its error message prose (design D6).
message RetirementBlocked {
  string role_key = 1;
  repeated SurvivingGrant grants = 2;
  uint64 total_surviving = 3;
  bool truncated = 4;
}

// A refusal, not an error: this is a STATIC policy, so removing it changes
// decisions fleet-wide and the caller has not acknowledged that. Carries
// what would be destroyed, so the refusal doubles as the operator's
// preview. `policy_id` is a gRPC-only field (design D6).
message RetirementNeedsAcknowledgement {
  string policy_id = 1;
  string kind = 2;
  string source = 3;
  string description = 4;
}

// All three outcomes return gRPC OK: the two refusals are outcomes that
// are not Retired, never server errors (design D3). Two consequences a
// client MUST handle:
//   1. An UNSET `outcome` is a protocol error. Treat it as failure, never
//      as a successful retirement.
//   2. The HTTP twin answers the two refusals with 409 plus a registry
//      error code; this response carries neither. The payload fields are
//      the same, the status is not.
message RetireSystemPolicyResponse {
  oneof outcome {
    RetiredPolicy retired = 1;
    RetirementBlocked blocked = 2;
    RetirementNeedsAcknowledgement needs_acknowledgement = 3;
  }
}

// ─────────────────────────────────────────────────────────────────────────
// Outbox dead letters (SMA-501), mirroring /v1/outbox/dead-letters.
// Every RPC here is Root-only, enforced inside DeadLetterService itself.
//
// A caveat for the time filters, not a bug: a row with parked_at unset can
// never satisfy parked_from/parked_to, because Postgres never evaluates a
// NULL comparison as true. Such a row is invisible to ListDeadLetters
// whenever either bound is set. It stays reachable via an unfiltered list.
// ─────────────────────────────────────────────────────────────────────────
message DeadLetterEntry {
  string id = 1;
  google.protobuf.Timestamp occurred_at = 2;
  string event_type = 3;
  int32 schema_version = 4;
  string aggregate_prn = 5;
  string actor_prn = 6; // empty => none
  string payload = 7;   // JSON-serialized string, like AuditEntry.detail_json
  string correlation_id = 8; // empty => none
  uint32 attempts = 9;
  google.protobuf.Timestamp parked_at = 10; // absent => not parked
  string last_error = 11; // empty => none
}

// Optional filters use absent/empty/zero sentinels, mirroring
// ListAuditEntriesRequest. An ABSENT timestamp means unfiltered; a PRESENT
// but unrepresentable one is INVALID_ARGUMENT and never silently
// unfiltered (design D10).
message ListDeadLettersRequest {
  string event_type = 1;
  google.protobuf.Timestamp parked_from = 2;
  google.protobuf.Timestamp parked_to = 3;
  string cursor = 4;
  uint32 limit = 5; // 0 => server default; clamped to 200
}
message ListDeadLettersResponse {
  repeated DeadLetterEntry entries = 1;
  string next_cursor = 2; // set only when the page came back FULL
}

message ReplayDeadLetterRequest {
  string id = 1;
}
message ReplayDeadLetterResponse {
  DeadLetterEntry entry = 1;
}
message DiscardDeadLetterRequest {
  string id = 1;
}
message DiscardDeadLetterResponse {
  DeadLetterEntry entry = 1;
}

// Named BulkReplayDeadLetters, not ReplayDeadLetters: one character from
// ReplayDeadLetter while replaying up to 10000 rows instead of one, on a
// destructive operator surface, with no lint rule that would catch the
// typo. Matches the BulkReplayRequest type it maps onto.
// NOTE: what shipped also carries the partial-failure contract this block
// omits — bulk replay is NOT atomic, so a DEADLINE_EXCEEDED or cancelled RPC
// may leave an unknown number of rows already replayed, and re-issuing is
// safe because every replay statement carries `AND parked = true`, so an
// already-replayed row no longer matches. Added in the final fix wave; see
// the shipped `contracts/proto/paigasus/iam/v1/iam.proto`.
message BulkReplayDeadLettersRequest {
  string event_type = 1;
  google.protobuf.Timestamp parked_from = 2;
  google.protobuf.Timestamp parked_to = 3;
  // 0 is rejected as invalid-bulk-replay before any store access, and an
  // absent field collapses to 0, so "didn't say" and "said zero" are
  // rejected identically — exactly HTTP's behavior. This is DELIBERATE.
  // Do NOT "fix" it into an optional: the explicit row budget is the guard
  // on blast radius and must never default to anything usable.
  // Silently clamped to 10000, with no signal to the caller, on both
  // transports.
  uint64 max_rows = 4;
}
message BulkReplayDeadLettersResponse {
  uint64 replayed = 1;
}

service OutboxService {
  rpc ListDeadLetters(ListDeadLettersRequest) returns (ListDeadLettersResponse);
  rpc ReplayDeadLetter(ReplayDeadLetterRequest) returns (ReplayDeadLetterResponse);
  rpc BulkReplayDeadLetters(BulkReplayDeadLettersRequest) returns (BulkReplayDeadLettersResponse);
  rpc DiscardDeadLetter(DiscardDeadLetterRequest) returns (DiscardDeadLetterResponse);
}
```

- [ ] **Step 2: Do NOT add the retire RPC here — it belongs to Task 7**

The retire **messages** above stay in this task: Task 4 needs the generated
`RetireSystemPolicyResponse`/`RetiredPolicy`/… types and runs before Task 7. Unreferenced
messages are valid proto and `buf lint` does not object.

The `rpc RetireSystemPolicy(...)` line itself must **not** be added here. `AuthorizationService`
already has a concrete implementor — `AuthzGrpc` in `grpc/authz.rs:92` — and Rust requires every
trait method to be implemented, so adding the RPC without the handler is a hard `error[E0046]`
that would break `cargo build --workspace` for Tasks 2 through 6. This differs from
`UserService`/`OutboxService`, which are brand-new traits with zero implementors and are
therefore safe to declare here.

Task 7 adds the RPC line, regenerates, and implements the handler in one commit, so the contract
change and its implementor land atomically.

- [ ] **Step 3: Refresh the stale file header**

The header at lines 9-16 enumerates services and is already stale (it omits
`ServiceAccountService` and `AuditService`). Rewrite that list to name all eight services now
in the file: `TenancyService`, `AuthnService`, `AuthorizationService`, `ServiceAccountService`,
`AuditService`, `UserService`, `OutboxService` — plus `ServiceInfoService` living in
`common/v1`. Keep the existing comment style.

- [ ] **Step 4: Format, lint, and check for breakage**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf format -w && buf lint && buf breaking --against '../.git#branch=main,subdir=contracts'
```
Expected: all three exit 0. `buf breaking` passes because every change is additive.

- [ ] **Step 5: Regenerate the bindings**

Call `buf generate` **directly**, not via Moon: `contracts:generate` declares no `outputs:` and
can serve a stale cached result.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf generate
```

- [ ] **Step 6: Verify the generated surface exists and the workspace still builds**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
grep -c "OutboxServiceServer\|UserServiceServer" rs/crates/libs/paigasus-proto/src/generated/paigasus/iam/v1/paigasus.iam.v1.tonic.rs
cd rs && cargo build --workspace
```
Expected: the grep prints a non-zero count; the build succeeds. Nothing implements the new
server traits yet, which is fine — an unimplemented generated trait is not an error.

- [ ] **Step 7: Commit**

```bash
git add contracts/ rs/crates/libs/paigasus-proto/src/generated/ py/packages/paigasus-proto/ ts/packages/paigasus-proto/
git commit -m "feat(contracts): add UserService, OutboxService and RetireSystemPolicy (SMA-501)"
```

---

### Task 2: Strict timestamp conversion in `convert.rs`

The single most important correctness fix in this plan. `convert::from_ts` returns `None` for an
unrepresentable timestamp, and `grpc/audit.rs` feeds that straight into a filter via
`and_then(from_ts)` — where `None` means **unfiltered**. Copying that shape into the dead-letter
adapter would let `parked_from { nanos: -1 }` silently widen a bulk replay into "replay
everything up to `max_rows`". This task builds the helper that makes the mistake impossible.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` (add after `from_ts`, around line 195)

**Interfaces:**
- Consumes: `convert::from_ts` (existing), `TenancyError::InvalidPrn`.
- Produces: `pub fn parse_opt_ts(t: Option<prost_types::Timestamp>, field: &str) -> Result<Option<DateTime<Utc>>, TenancyError>` — used by Task 6.

- [ ] **Step 1: Write the failing tests**

Add to `convert.rs`'s existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn parse_opt_ts_treats_an_absent_timestamp_as_unfiltered() {
    assert_eq!(parse_opt_ts(None, "parked_from").unwrap(), None);
}

#[test]
fn parse_opt_ts_returns_the_exact_instant_for_a_valid_timestamp() {
    let expected = DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z").unwrap().with_timezone(&Utc);
    let got = parse_opt_ts(Some(prost_types::Timestamp { seconds: expected.timestamp(), nanos: 0 }), "parked_from").unwrap();
    assert_eq!(got, Some(expected));
}

/// The whole reason this helper exists. `from_ts` alone returns `None` here, and `None` means
/// UNFILTERED — so an `and_then(from_ts)` call site would silently drop the caller's time
/// bound and widen a bulk replay to every parked row. A present field must never be
/// reinterpreted as an absent one.
#[test]
fn parse_opt_ts_rejects_a_present_but_unrepresentable_timestamp_instead_of_unfiltering() {
    for (label, t) in [
        ("negative nanos", prost_types::Timestamp { seconds: 0, nanos: -1 }),
        ("out-of-range seconds", prost_types::Timestamp { seconds: i64::MAX, nanos: 0 }),
    ] {
        let err = parse_opt_ts(Some(t), "parked_from").expect_err(label);
        assert!(matches!(err, TenancyError::InvalidPrn(_)), "{label} must be a client error, not None");
    }
    // Sanity: the underlying primitive really does collapse these to None, which is what makes
    // this helper load-bearing rather than decorative.
    assert_eq!(from_ts(prost_types::Timestamp { seconds: 0, nanos: -1 }), None);
}

/// The message must name the offending field: a request carries four timestamps, and "invalid
/// timestamp" alone would leave an operator guessing which one.
#[test]
fn parse_opt_ts_names_the_field_in_its_error() {
    let err = parse_opt_ts(Some(prost_types::Timestamp { seconds: 0, nanos: -1 }), "parked_to").unwrap_err();
    assert!(err.to_string().contains("parked_to"), "got: {err}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(parse_opt_ts)' --no-tests=pass
```
Expected: compile error — `cannot find function parse_opt_ts`.

- [ ] **Step 3: Implement the helper**

```rust
/// Parses an optional wire timestamp with the three cases kept DISTINCT: absent means
/// unfiltered, a valid value converts, and a **present but unrepresentable** value is a client
/// error.
///
/// That third case is why this exists. [`from_ts`] returns `None` for a negative `nanos` or an
/// out-of-`chrono`-range `seconds`, and on a filter field `None` means UNFILTERED — so the
/// `req.field.and_then(convert::from_ts)` shape used in `grpc::audit` silently DROPS a
/// malformed bound instead of rejecting it. On `BulkReplayDeadLetters` that turns a
/// narrowly-scoped replay into "replay everything up to `max_rows`". The HTTP twin rejects the
/// equivalent with a 400 (`http::dead_letters::parse_ts`), so this also restores parity.
///
/// `InvalidPrn`-as-sentinel, mirroring `http::dead_letters::parse_ts` and
/// `grpc::audit::parse_cursor` — there is no dedicated error code for "not a valid timestamp".
pub fn parse_opt_ts(t: Option<prost_types::Timestamp>, field: &str) -> Result<Option<DateTime<Utc>>, TenancyError> {
    match t {
        None => Ok(None),
        Some(raw) => from_ts(raw)
            .map(Some)
            .ok_or_else(|| TenancyError::InvalidPrn(format!("invalid timestamp for {field}"))),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(parse_opt_ts)'
```
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs
git commit -m "feat(rs): reject a present-but-invalid wire timestamp instead of unfiltering (SMA-501)"
```

---

### Task 3: `to_proto_dead_letter_entry` and its twin test

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs`

**Interfaces:**
- Consumes: `convert::ts` (existing), `paigasus_iam_core::DeadLetterEntry`, `paigasus_proto::..::DeadLetterEntry`.
- Produces: `pub fn to_proto_dead_letter_entry(e: &paigasus_iam_core::DeadLetterEntry) -> ProtoDeadLetterEntry` — used by Task 6.

**Alias convention:** both `DeadLetterEntry` types are in scope. Follow `audit.rs:21` and import
the generated one as `DeadLetterEntry as ProtoDeadLetterEntry`.

- [ ] **Step 1: Write the failing twin test**

Add to `convert.rs`'s test module. This is the drift guard: one domain value into both
projections, asserting they agree.

```rust
/// The HTTP/gRPC drift guard for the dead-letter surface (design D9.1), paired with
/// `http::dto`'s own projection. Both transports project the SAME domain value, so feeding one
/// `DeadLetterEntry` through both and comparing field-for-field is the only cheap, deterministic
/// way to catch one of them drifting. Deliberately exercises the `None` half of every optional
/// field, since the empty-string / absent-timestamp sentinel mapping is where the two shapes
/// differ in TYPE and so is where they are most likely to diverge in MEANING.
#[test]
fn dead_letter_entry_projects_identically_for_http_and_grpc() {
    use crate::adapters::http::dto::DeadLetterEntryDto;

    let occurred = DateTime::parse_from_rfc3339("2026-08-01T10:00:00Z").unwrap().with_timezone(&Utc);
    let parked = DateTime::parse_from_rfc3339("2026-08-01T11:00:00Z").unwrap().with_timezone(&Utc);
    let domain = paigasus_iam_core::DeadLetterEntry {
        id: Uuid::from_u128(7),
        occurred_at: occurred,
        event_type: "iam.principal.created".to_string(),
        schema_version: 3,
        aggregate_prn: "prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000042".to_string(),
        actor_prn: None,
        payload: r#"{"principal_id":"x"}"#.to_string(),
        correlation_id: None,
        attempts: 5,
        parked_at: Some(parked),
        last_error: None,
    };

    let http = DeadLetterEntryDto::from(domain.clone());
    let grpc = to_proto_dead_letter_entry(&domain);

    assert_eq!(grpc.id, http.id.to_string());
    assert_eq!(grpc.occurred_at, Some(ts(http.occurred_at)));
    assert_eq!(grpc.event_type, http.event_type);
    assert_eq!(grpc.schema_version, http.schema_version);
    assert_eq!(grpc.aggregate_prn, http.aggregate_prn);
    assert_eq!(grpc.attempts, http.attempts);
    assert_eq!(grpc.parked_at, http.parked_at.map(ts));
    // The sentinel half: HTTP keeps `None`, the wire uses "".
    assert_eq!(http.actor_prn, None);
    assert_eq!(grpc.actor_prn, "");
    assert_eq!(http.correlation_id, None);
    assert_eq!(grpc.correlation_id, "");
    assert_eq!(http.last_error, None);
    assert_eq!(grpc.last_error, "");
    assert_eq!(grpc.payload, http.payload);
}

/// The `Some` half, asserted separately so a projection that hardcoded the empty-string
/// sentinel — passing the test above — still fails here.
#[test]
fn dead_letter_entry_forwards_present_optional_fields_verbatim() {
    let occurred = DateTime::parse_from_rfc3339("2026-08-01T10:00:00Z").unwrap().with_timezone(&Utc);
    let correlation = Uuid::from_u128(99);
    let domain = paigasus_iam_core::DeadLetterEntry {
        id: Uuid::from_u128(7),
        occurred_at: occurred,
        event_type: "iam.principal.created".to_string(),
        schema_version: 3,
        aggregate_prn: "prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000042".to_string(),
        actor_prn: Some("prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000001".to_string()),
        payload: "{}".to_string(),
        correlation_id: Some(correlation),
        attempts: 1,
        parked_at: None,
        last_error: Some("connection refused".to_string()),
    };

    let grpc = to_proto_dead_letter_entry(&domain);
    assert_eq!(grpc.actor_prn, domain.actor_prn.clone().unwrap());
    assert_eq!(grpc.correlation_id, correlation.to_string());
    assert_eq!(grpc.last_error, "connection refused");
    assert_eq!(grpc.parked_at, None, "an unparked row must carry an ABSENT timestamp, not epoch");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(dead_letter_entry)' --no-tests=pass
```
Expected: compile error — `cannot find function to_proto_dead_letter_entry`.

- [ ] **Step 3: Implement the projection**

Add the import alias at the top of `convert.rs` alongside the other generated-type imports, then:

```rust
/// Projects a domain [`DeadLetterEntry`] into its wire message. `id`/`correlation_id` become
/// canonical uuid strings, `actor_prn`/`correlation_id`/`last_error` collapse `None` to the
/// empty-string sentinel the proto documents, and an unparked row carries an ABSENT
/// `parked_at` rather than an epoch timestamp. Mirrors `http::dto::DeadLetterEntryDto`'s
/// `From` impl field-for-field — the two are pinned together by
/// `dead_letter_entry_projects_identically_for_http_and_grpc`.
pub fn to_proto_dead_letter_entry(e: &paigasus_iam_core::DeadLetterEntry) -> ProtoDeadLetterEntry {
    ProtoDeadLetterEntry {
        id: e.id.to_string(),
        occurred_at: Some(ts(e.occurred_at)),
        event_type: e.event_type.clone(),
        schema_version: e.schema_version,
        aggregate_prn: e.aggregate_prn.clone(),
        actor_prn: e.actor_prn.clone().unwrap_or_default(),
        payload: e.payload.clone(),
        correlation_id: e.correlation_id.map(|id| id.to_string()).unwrap_or_default(),
        attempts: e.attempts,
        parked_at: e.parked_at.map(ts),
        last_error: e.last_error.clone().unwrap_or_default(),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(dead_letter_entry)'
```
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs
git commit -m "feat(rs): project a dead-letter entry onto the wire, pinned to its http twin (SMA-501)"
```

---

### Task 4: `to_proto_retire_response` and its twin test

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/system_retirement.rs` (its `#[cfg(test)] mod tests` **only** — no production change)

**Interfaces:**
- Consumes: `paigasus_iam_core::{RetireOutcome, GrantRef}`, `paigasus_iam_core::authz::reconcile::policy_kind_str`.
- Produces: `pub fn to_proto_retire_response(outcome: RetireOutcome) -> RetireSystemPolicyResponse` — used by Task 7.

**Why the twin test lives in the HTTP file:** `mod system_retirement` is private
(`http/mod.rs:25`) and `fn response_for` is private to it, so `convert.rs`'s test module cannot
name it. The comparison must run from inside `system_retirement.rs`, which *can* reach
`crate::adapters::grpc::convert`.

- [ ] **Step 1: Write the failing conversion tests in `convert.rs`**

```rust
/// Every variant constructed directly — no `AppState`, database, or request needed. That is the
/// whole point of this being a free function over an owned `RetireOutcome` (design D8): an
/// earlier revision of the HTTP twin changed `Retired`'s status code and the entire crate's
/// suite stayed green, because nothing exercised the mapping against a real outcome value.
#[test]
fn retire_response_maps_each_outcome_to_its_own_variant() {
    use paigasus_iam_core::authz::model::PolicyKind;
    use paigasus_proto::paigasus::iam::v1::retire_system_policy_response::Outcome;

    let retired = to_proto_retire_response(RetireOutcome::Retired {
        policy_id: "legacy_auditor".to_string(),
        kind: PolicyKind::Template,
        role_deleted: true,
    });
    match retired.outcome.expect("outcome must be set") {
        Outcome::Retired(r) => {
            assert_eq!(r.policy_id, "legacy_auditor");
            assert_eq!(r.kind, "template");
            assert!(r.role_deleted);
        }
        other => panic!("expected Retired, got {other:?}"),
    }

    let blocked = to_proto_retire_response(RetireOutcome::Blocked {
        role_key: "legacy_auditor".to_string(),
        grants: vec![GrantRef {
            id: "0192f1c0-0000-7000-8000-000000000001".to_string(),
            principal_prn: "prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000002".to_string(),
            scope_prn: "prn:pgs:iam:::organization/0192f1c0-0000-7000-8000-000000000003".to_string(),
        }],
        total: 42,
        truncated: true,
    });
    match blocked.outcome.expect("outcome must be set") {
        Outcome::Blocked(b) => {
            assert_eq!(b.role_key, "legacy_auditor");
            assert_eq!(b.total_surviving, 42, "the TRUE total, not the truncated page length");
            assert!(b.truncated);
            assert_eq!(b.grants.len(), 1);
            assert_eq!(b.grants[0].id, "0192f1c0-0000-7000-8000-000000000001");
            assert_eq!(b.grants[0].principal_prn, "prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000002");
            assert_eq!(b.grants[0].scope_prn, "prn:pgs:iam:::organization/0192f1c0-0000-7000-8000-000000000003");
        }
        other => panic!("expected Blocked, got {other:?}"),
    }

    let needs = to_proto_retire_response(RetireOutcome::NeedsAcknowledgement {
        policy_id: "legacy_forbid".to_string(),
        kind: PolicyKind::Static,
        source: "permit(principal, action, resource);".to_string(),
        description: "an orphaned starter policy".to_string(),
    });
    match needs.outcome.expect("outcome must be set") {
        Outcome::NeedsAcknowledgement(n) => {
            assert_eq!(n.policy_id, "legacy_forbid");
            assert_eq!(n.kind, "static");
            assert_eq!(n.source, "permit(principal, action, resource);");
            assert_eq!(n.description, "an orphaned starter policy");
        }
        other => panic!("expected NeedsAcknowledgement, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(retire_response)' --no-tests=pass
```
Expected: compile error — `cannot find function to_proto_retire_response`.

- [ ] **Step 3: Implement the projection**

```rust
/// Maps a [`RetireOutcome`] onto its wire response. All three variants are gRPC `OK`: the two
/// refusals are outcomes that are not `Retired`, never server errors (design D3, and the same
/// argument `http::system_retirement`'s module doc makes for routing them around `ApiError`).
///
/// A free function over an OWNED outcome, deliberately — that is what lets every variant be
/// constructed in a test with no `AppState`, database, or request, which is exactly the gap
/// that let an earlier `200`->`204` regression in the HTTP twin pass a green suite.
pub fn to_proto_retire_response(outcome: RetireOutcome) -> RetireSystemPolicyResponse {
    use paigasus_proto::paigasus::iam::v1::retire_system_policy_response::Outcome;

    let variant = match outcome {
        RetireOutcome::Retired { policy_id, kind, role_deleted } => Outcome::Retired(RetiredPolicy {
            policy_id,
            kind: policy_kind_str(kind).to_string(),
            role_deleted,
        }),
        RetireOutcome::Blocked { role_key, grants, total, truncated } => Outcome::Blocked(RetirementBlocked {
            role_key,
            grants: grants
                .iter()
                .map(|g| SurvivingGrant {
                    id: g.id.clone(),
                    principal_prn: g.principal_prn.clone(),
                    scope_prn: g.scope_prn.clone(),
                })
                .collect(),
            total_surviving: total,
            truncated,
        }),
        RetireOutcome::NeedsAcknowledgement { policy_id, kind, source, description } => {
            Outcome::NeedsAcknowledgement(RetirementNeedsAcknowledgement {
                policy_id,
                kind: policy_kind_str(kind).to_string(),
                source,
                description,
            })
        }
    };
    RetireSystemPolicyResponse { outcome: Some(variant) }
}
```

- [ ] **Step 4: Run to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(retire_response)'
```
Expected: 1 passed.

- [ ] **Step 5: Write the cross-transport twin test in `system_retirement.rs`**

Add to that file's existing `#[cfg(test)] mod tests`. Per design D6 this is a **subset** check:
`role_key` and `policy_id` are gRPC-only, `error.code`/`error.message` are HTTP-only.

```rust
/// The HTTP/gRPC drift guard for the retirement surface (design D9.1). Both transports project
/// the same `RetireOutcome`; feeding one value through both is the only deterministic way to
/// catch one drifting. Lives HERE, not in `grpc::convert`'s test module, because
/// `mod system_retirement` and `fn response_for` are both private — this file can reach
/// `grpc::convert`, but not the reverse.
///
/// A SUBSET check with a named allowlist, not field-for-field (design D6): the wire messages
/// carry `role_key`/`policy_id` as real fields where HTTP has them only inside `error.message`
/// prose, and HTTP carries `error.code`/`error.message` which the proto deliberately omits.
/// Everything else must agree exactly.
#[tokio::test]
async fn retire_outcomes_project_consistently_across_http_and_grpc() {
    use crate::adapters::grpc::convert;
    use paigasus_proto::paigasus::iam::v1::retire_system_policy_response::Outcome;

    let outcome = RetireOutcome::Blocked {
        role_key: "legacy_auditor".to_string(),
        grants: vec![GrantRef {
            id: "0192f1c0-0000-7000-8000-000000000001".to_string(),
            principal_prn: "prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000002".to_string(),
            scope_prn: "prn:pgs:iam:::organization/0192f1c0-0000-7000-8000-000000000003".to_string(),
        }],
        total: 42,
        truncated: true,
    };

    let http_response = response_for(outcome.clone());
    assert_eq!(http_response.status(), StatusCode::CONFLICT, "the HTTP twin refuses with 409");
    let bytes = to_bytes(http_response.into_body(), usize::MAX).await.unwrap();
    let http: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    let Some(Outcome::Blocked(grpc)) = convert::to_proto_retire_response(outcome).outcome else {
        panic!("gRPC must map a Blocked outcome to the Blocked variant");
    };

    assert_eq!(grpc.total_surviving, http["total_surviving"].as_u64().unwrap());
    assert_eq!(grpc.truncated, http["truncated"].as_bool().unwrap());
    let http_grants = http["grants"].as_array().unwrap();
    assert_eq!(grpc.grants.len(), http_grants.len());
    assert_eq!(grpc.grants[0].id, http_grants[0]["id"].as_str().unwrap());
    assert_eq!(grpc.grants[0].principal_prn, http_grants[0]["principal_prn"].as_str().unwrap());
    assert_eq!(grpc.grants[0].scope_prn, http_grants[0]["scope_prn"].as_str().unwrap());

    // The allowlisted divergences, asserted so they stay DELIBERATE rather than becoming
    // accidents nobody notices.
    assert_eq!(grpc.role_key, "legacy_auditor", "gRPC-only field");
    assert!(http.get("role_key").is_none(), "HTTP carries role_key only in the message prose");
    assert!(http["error"]["code"].is_string(), "HTTP-only field");
}

/// The `Retired` half of the same guard: the one variant whose HTTP status a past regression
/// actually changed.
#[tokio::test]
async fn a_retired_outcome_projects_consistently_across_http_and_grpc() {
    use crate::adapters::grpc::convert;
    use paigasus_proto::paigasus::iam::v1::retire_system_policy_response::Outcome;

    let outcome = RetireOutcome::Retired {
        policy_id: "legacy_auditor".to_string(),
        kind: PolicyKind::Template,
        role_deleted: true,
    };

    let http_response = response_for(outcome.clone());
    assert_eq!(http_response.status(), StatusCode::OK, "200, never 204 — the body is the record");
    let bytes = to_bytes(http_response.into_body(), usize::MAX).await.unwrap();
    let http: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    let Some(Outcome::Retired(grpc)) = convert::to_proto_retire_response(outcome).outcome else {
        panic!("gRPC must map a Retired outcome to the Retired variant");
    };
    assert_eq!(grpc.policy_id, http["policy_id"].as_str().unwrap());
    assert_eq!(grpc.kind, http["kind"].as_str().unwrap());
    assert_eq!(grpc.role_deleted, http["role_deleted"].as_bool().unwrap());
}
```

- [ ] **Step 6: Run the twin tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(project_consistently) or test(projects_consistently)'
```
Expected: 2 passed. If `RetireOutcome` or `GrantRef` does not derive `Clone`, add the derive in
`paigasus-iam-core/src/authz/retirement.rs` — `RetireOutcome` already does; check `GrantRef`.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs rs/crates/services/paigasus-iam/src/adapters/http/system_retirement.rs
git commit -m "feat(rs): map a retire outcome onto the response oneof, pinned to its http twin (SMA-501)"
```

---

### Task 5: `UserService` gRPC handler

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/grpc/users.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs:132-136` (stale comment)

**Interfaces:**
- Consumes: Task 1's generated `UserService`/`UserServiceServer`, `CreateUserRequest`, `CreateUserResponse`.
- Produces: `pub struct UserGrpc` with `pub fn new(state: AppState) -> Self`.

**Read D0 before writing this.** `CreateUser` has **no** authorization check. That is
deliberate parity with HTTP, and the handler must say so, because every neighbouring gRPC
handler in this crate does check.

- [ ] **Step 1: Write the handler**

```rust
// SPDX-License-Identifier: Apache-2.0

//! `UserGrpc`: the `UserService` gRPC server (SMA-501) — a thin adapter over `AppState.users`:
//! parse the wire request -> `CreateUser::execute` -> project the id, no business logic in this
//! layer (mirrors `grpc::audit`'s posture).
//!
//! **This RPC performs NO authorization check, deliberately, and that is why the service
//! exists.** `CreateUser::execute` takes no `actor` parameter, `http::users` extracts no
//! `AuthContext`, and there is no `Action::CreateUser` in the Cedar action catalog — so
//! `POST /v1/users` is bearer-gated and otherwise unauthorized. This adapter mirrors that
//! exactly, because parity with the HTTP surface is the acceptance criterion and tightening
//! authorization on an existing endpoint is a behavior change belonging to its own issue.
//!
//! It sits on `UserService` rather than `TenancyService` for exactly this reason: all 21
//! `TenancyService` RPCs authorize in the adapter (`if self.state.enforce_tenancy { … }`), so
//! parking the one unchecked RPC among them would camouflage the single property a reviewer
//! most needs to see. On its own service, the absence is legible in the contract.
//!
//! **Bearer enforcement still applies:** `UserService` is NOT on `AuthLayer`'s `:path`
//! exemption list (`grpc::authn::is_exempt`), so an unauthenticated call never reaches here.

use std::time::Instant;

use paigasus_observability::record_grpc;
use paigasus_proto::paigasus::iam::v1::user_service_server::UserService;
use paigasus_proto::paigasus::iam::v1::{CreateUserRequest, CreateUserResponse};
use tonic::{Request, Response, Status};

use super::convert;
use crate::adapters::http::AppState;
use crate::application::create_user::NewUser;
use crate::application::error::TenancyError;

/// The `UserService` gRPC server — a thin adapter over `AppState.users` (module docs).
pub struct UserGrpc {
    state: AppState,
}

impl UserGrpc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// An empty wire string means "unset" on an optional scalar (the proto's own doc). NOTE this
/// DIVERGES from HTTP, where `CreateUserBody.locale` is an `Option<String>` and `{"locale": ""}`
/// therefore persists `Some("")` rather than `None` (design D11). The proto sentinel is
/// normative for gRPC; HTTP is deliberately left unchanged.
fn opt_string(raw: String) -> Option<String> {
    if raw.is_empty() { None } else { Some(raw) }
}

#[tonic::async_trait]
impl UserService for UserGrpc {
    /// `CreateUser`: bearer-required, otherwise UNAUTHORIZED BY DESIGN — see this module's doc.
    /// An invalid email is rejected before an id is minted or a transaction opened
    /// (`CreateUser::execute`), and a duplicate email rolls the whole unit of work back before
    /// the `iam.principal.created` event is ever enqueued.
    async fn create_user(&self, request: Request<CreateUserRequest>) -> Result<Response<CreateUserResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<CreateUserResponse>, Status> = async {
            let req = request.into_inner();
            let cmd = NewUser {
                email: req.email,
                display_name: req.display_name,
                locale: opt_string(req.locale),
                timezone: opt_string(req.timezone),
            };
            // `TenancyError::from` spelled out rather than a bare `.into()`: `status_to_grpc`
            // takes a `TenancyError`, and inference through two conversions is fragile here.
            let id = self
                .state
                .users
                .execute(cmd)
                .await
                .map_err(|e| convert::status_to_grpc(TenancyError::from(e)))?;
            Ok(Response::new(CreateUserResponse {
                principal_prn: id.canonical(),
            }))
        }
        .await;
        record_grpc("User", "CreateUser", started, &result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_optional_scalar_becomes_none() {
        assert_eq!(opt_string(String::new()), None);
        assert_eq!(opt_string("de-DE".to_string()), Some("de-DE".to_string()));
    }

    // ⚠️ SUPERSEDED DURING IMPLEMENTATION — do not copy the test below.
    //
    // As written here it is VACUOUS: it builds the "HTTP side" as a hand-written struct literal
    // rather than running the real projection, so every value it compares is one the test just
    // wrote, and one assertion compares a value to itself. Mutating the real HTTP mapping to
    // `locale: None` leaves it green. The final whole-branch review caught this.
    //
    // What shipped instead: `http::users::to_command` was extracted as a pure function so BOTH
    // sides of the twin run production code, and the test lives in `http/users.rs` (it cannot
    // live in `grpc/users.rs` — `mod users` is private inside `adapters::http`, so naming
    // `http::users::to_command` from there is `E0603`). See `adapters/http/users.rs`.

    /// The HTTP/gRPC twin test for this surface (design D9.1), covering the ONE field where the
    /// two transports deliberately disagree (D11). Both build the same `NewUser` command, so
    /// feeding an equivalent wire payload through both projections is what pins the divergence
    /// as intentional rather than letting it drift further.
    #[test]
    fn create_user_projects_onto_the_same_command_except_for_the_empty_string_sentinel() {
        use crate::adapters::http::dto::CreateUserBody;

        // Both present: the two transports must agree exactly.
        let grpc = NewUser {
            email: "a@example.com".to_string(),
            display_name: "A".to_string(),
            locale: opt_string("de-DE".to_string()),
            timezone: opt_string("Europe/Berlin".to_string()),
        };
        let body = CreateUserBody {
            email: "a@example.com".to_string(),
            display_name: "A".to_string(),
            locale: Some("de-DE".to_string()),
            timezone: Some("Europe/Berlin".to_string()),
        };
        assert_eq!(grpc.email, body.email);
        assert_eq!(grpc.display_name, body.display_name);
        assert_eq!(grpc.locale, body.locale);
        assert_eq!(grpc.timezone, body.timezone);

        // The allowlisted divergence, asserted so it stays deliberate: the same "empty" wire
        // value means `None` on gRPC and `Some("")` on HTTP, which persists an empty string
        // rather than NULL.
        assert_eq!(opt_string(String::new()), None);
        let http_empty = CreateUserBody {
            email: "b@example.com".to_string(),
            display_name: "B".to_string(),
            locale: Some(String::new()),
            timezone: None,
        };
        assert_eq!(http_empty.locale, Some(String::new()), "HTTP keeps the empty string — gRPC does not");
    }
}
```

If `CreateUserBody`'s fields are not constructible from this module, make the struct's fields
`pub` (they already are, `http/dto.rs:189-194`) — no other change is needed.

- [ ] **Step 2: Register the service in `grpc/mod.rs`**

Add `pub mod users;` to the module list, `use users::UserGrpc;` to the imports, the server
import alongside the others, and this line to the unconditional `add_service` chain (next to
`TenancyServiceServer`):

```rust
        .add_service(UserServiceServer::new(UserGrpc::new(state.clone())))
```

Also extend `mod.rs`'s module doc and `router()`'s doc to name `UserService` — both enumerate
the mounted services.

- [ ] **Step 3: Fix the now-false comment in `grpc_tenancy.rs`**

Lines 132-136 currently say `TenancyService` has no `CreateUser` RPC "(users stay HTTP-only per
Task 15)". The first clause is still true; the parenthetical is not. Rewrite to note that user
creation now has a gRPC surface on the separate `UserService` (SMA-501), and that this test
still calls the application service directly because it only needs a principal, not coverage of
that RPC.

- [ ] **Step 4: Build and run the unit test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --workspace && cargo nextest run -p paigasus-iam --lib -E 'test(an_empty_optional_scalar)'
```
Expected: build succeeds, 1 passed.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/ rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs
git commit -m "feat(rs): serve CreateUser over grpc on a dedicated UserService (SMA-501)"
```

---

### Task 6: `OutboxService` gRPC handler

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/grpc/dead_letters.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs`

**Interfaces:**
- Consumes: `convert::parse_opt_ts` (Task 2), `convert::to_proto_dead_letter_entry` (Task 3), Task 1's generated types.
- Produces: `pub struct OutboxGrpc` with `pub fn new(state: AppState) -> Self`.

- [ ] **Step 1: Write the failing unit tests**

Write the handler file with only its `to_filter` / `to_bulk_request` helpers plus this test
module first, so the tests fail to compile for the right reason.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> ListDeadLettersRequest {
        ListDeadLettersRequest {
            event_type: String::new(),
            parked_from: None,
            parked_to: None,
            cursor: String::new(),
            limit: 0,
        }
    }

    #[test]
    fn to_filter_treats_empty_wire_fields_as_unfiltered() {
        let f = to_filter(req()).unwrap();
        assert_eq!(f.event_type, None);
        assert_eq!(f.parked_from, None);
        assert_eq!(f.parked_to, None);
        assert_eq!(f.cursor, None);
    }

    /// Mirrors `http::dead_letters`'s identical test. `limit` is mapped HERE, not left to
    /// `capped_limit` — whose floor for a literal 0 is 1, so a default request would otherwise
    /// return a single row.
    #[test]
    fn to_filter_maps_an_absent_limit_to_the_server_default() {
        assert_eq!(to_filter(req()).unwrap().limit, DEFAULT_LIMIT);
        assert_ne!(DEFAULT_LIMIT, 1);
    }

    /// A hardcoded `limit: DEFAULT_LIMIT` inside `to_filter` would pass every other test here.
    #[test]
    fn to_filter_passes_through_an_explicit_nonzero_limit() {
        assert_eq!(to_filter(ListDeadLettersRequest { limit: 5, ..req() }).unwrap().limit, 5);
    }

    #[test]
    fn to_filter_forwards_present_filters_with_their_exact_values() {
        let from = DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z").unwrap().with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339("2026-08-02T00:00:00Z").unwrap().with_timezone(&Utc);
        let f = to_filter(ListDeadLettersRequest {
            event_type: "iam.principal.created".to_string(),
            parked_from: Some(convert::ts(from)),
            parked_to: Some(convert::ts(to)),
            cursor: Uuid::from_u128(7).to_string(),
            limit: 5,
        })
        .unwrap();
        assert_eq!(f.event_type, Some("iam.principal.created".to_string()));
        assert_eq!(f.parked_from, Some(from));
        assert_eq!(f.parked_to, Some(to));
        assert_eq!(f.cursor, Some(Uuid::from_u128(7)));
    }

    #[test]
    fn to_filter_rejects_a_malformed_cursor() {
        assert!(matches!(
            to_filter(ListDeadLettersRequest { cursor: "nope".to_string(), ..req() }),
            Err(TenancyError::InvalidPrn(_))
        ));
    }

    /// The security-relevant case (design D10). A present-but-unrepresentable bound must be a
    /// client error — mapping it to `None` would mean UNFILTERED, silently widening the query.
    #[test]
    fn to_filter_rejects_a_present_but_invalid_timestamp_rather_than_unfiltering() {
        for t in [
            prost_types::Timestamp { seconds: 0, nanos: -1 },
            prost_types::Timestamp { seconds: i64::MAX, nanos: 0 },
        ] {
            assert!(matches!(
                to_filter(ListDeadLettersRequest { parked_from: Some(t), ..req() }),
                Err(TenancyError::InvalidPrn(_))
            ));
            assert!(matches!(
                to_filter(ListDeadLettersRequest { parked_to: Some(t), ..req() }),
                Err(TenancyError::InvalidPrn(_))
            ));
        }
    }

    fn bulk() -> BulkReplayDeadLettersRequest {
        BulkReplayDeadLettersRequest {
            event_type: String::new(),
            parked_from: None,
            parked_to: None,
            max_rows: 0,
        }
    }

    /// Design D5: proto3 cannot tell an absent `max_rows` from an explicit 0, and does not need
    /// to — both are rejected identically, before any store access. The explicit row budget is
    /// the guard on blast radius and must never default to anything usable.
    #[test]
    fn a_zero_max_rows_produces_an_invalid_bulk_replay_request() {
        assert!(!to_bulk_request(bulk()).unwrap().is_valid());
    }

    /// The one security-relevant mutation on this surface: silently dropping the filters turns
    /// a narrowly-scoped bulk replay into "replay everything up to max_rows". Asserts EXACT
    /// values — `is_some()` would pass even with `event_type` dropped or the instants swapped.
    #[test]
    fn to_bulk_request_forwards_every_filter_and_max_rows() {
        let from = DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z").unwrap().with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339("2026-08-02T00:00:00Z").unwrap().with_timezone(&Utc);
        let r = to_bulk_request(BulkReplayDeadLettersRequest {
            event_type: "iam.principal.created".to_string(),
            parked_from: Some(convert::ts(from)),
            parked_to: Some(convert::ts(to)),
            max_rows: 500,
        })
        .unwrap();
        assert_eq!(r.event_type, Some("iam.principal.created".to_string()));
        assert_eq!(r.parked_from, Some(from));
        assert_eq!(r.parked_to, Some(to));
        assert_eq!(r.max_rows, 500);
    }

    /// The same D10 guard on the BULK path, where dropping a bound is worst.
    #[test]
    fn to_bulk_request_rejects_a_present_but_invalid_timestamp() {
        let bad = prost_types::Timestamp { seconds: 0, nanos: -1 };
        assert!(matches!(
            to_bulk_request(BulkReplayDeadLettersRequest { parked_from: Some(bad), max_rows: 500, ..bulk() }),
            Err(TenancyError::InvalidPrn(_))
        ));
        assert!(matches!(
            to_bulk_request(BulkReplayDeadLettersRequest { parked_to: Some(bad), max_rows: 500, ..bulk() }),
            Err(TenancyError::InvalidPrn(_))
        ));
    }
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(to_filter) or test(to_bulk_request) or test(max_rows)' --no-tests=pass
```
Expected: compile errors naming the missing helpers.

- [ ] **Step 3: Write the full handler**

```rust
// SPDX-License-Identifier: Apache-2.0

//! `OutboxGrpc`: the `OutboxService` gRPC server (SMA-501) — a thin adapter over
//! `AppState.dead_letters`: parse -> `DeadLetterService` -> project, no business logic here
//! (mirrors `grpc::audit`, and `http::dead_letters` on the other transport).
//!
//! All four RPCs are Root-only, enforced INSIDE `DeadLetterService` itself, so a non-Root
//! caller gets `PermissionDenied` with nothing about the dead-letter contents reaching the
//! response. Every RPC is bearer-enforced by `AuthLayer` — `OutboxService` is not on
//! `grpc::authn::is_exempt`'s allowlist — so the caller's PRN comes from the resolved
//! `AuthContext`, never a client-supplied value.
//!
//! **Registered unconditionally**, unlike the neighbouring `AuditService`, which is dropped
//! entirely when `iam.audit` is off. The asymmetry is deliberate: `iam.audit` gates a READ-ONLY
//! surface, while this one permanently discards events and bulk-replays up to 10 000 — a
//! break-glass surface must not be disable-able, because the moment you need it is the moment a
//! config flag is hardest to change. HTTP mounts `dead_letters::router()` ungated too, so
//! gating gRPC alone would itself be a divergence.
//!
//! **A caveat for time filters, not a bug** (mirrors `PgDeadLetters` and `http::dead_letters`):
//! a row whose `parked_at` is unset can never satisfy `parked_from`/`parked_to` — Postgres
//! never evaluates a NULL comparison as true — so it is invisible to `ListDeadLetters` whenever
//! either bound is set. It stays reachable via an unfiltered list.

use std::time::Instant;

use chrono::{DateTime, Utc};
use paigasus_iam_core::{BulkReplayRequest, DeadLetterFilter};
use paigasus_kernel::Prn;
use paigasus_observability::record_grpc;
use paigasus_proto::paigasus::iam::v1::outbox_service_server::OutboxService;
use paigasus_proto::paigasus::iam::v1::{
    BulkReplayDeadLettersRequest, BulkReplayDeadLettersResponse, DiscardDeadLetterRequest, DiscardDeadLetterResponse, ListDeadLettersRequest,
    ListDeadLettersResponse, ReplayDeadLetterRequest, ReplayDeadLetterResponse,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use super::convert;
use crate::adapters::auth::AuthContext;
use crate::adapters::http::AppState;
use crate::application::error::TenancyError;
use crate::application::pagination::DEFAULT_LIMIT;

/// The `OutboxService` gRPC server — a thin adapter over `AppState.dead_letters`.
pub struct OutboxGrpc {
    state: AppState,
}

impl OutboxGrpc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Extracts the bearer-resolved [`AuthContext`] — mirrors `grpc::audit::actor_context`
/// exactly (duplicated rather than shared across a transport-internal boundary, the same
/// posture as every sibling).
fn actor_context<T>(request: &Request<T>) -> Result<AuthContext, Status> {
    request.extensions().get::<AuthContext>().cloned().ok_or_else(convert::missing_auth_context)
}

fn actor_prn(ctx: &AuthContext) -> Prn {
    ctx.principal_id.prn().clone()
}

/// An empty wire string means "unfiltered" on that field (the proto's own doc).
fn opt_string(raw: String) -> Option<String> {
    if raw.is_empty() { None } else { Some(raw) }
}

/// Empty means unfiltered; a non-empty value must parse as a uuid. `InvalidPrn`-as-sentinel,
/// mirroring `grpc::audit::parse_cursor`.
fn parse_cursor(raw: &str) -> Result<Option<Uuid>, TenancyError> {
    if raw.is_empty() {
        return Ok(None);
    }
    Uuid::parse_str(raw).map(Some).map_err(|_| TenancyError::InvalidPrn("cursor must be a uuid".to_string()))
}

/// `limit` `0` maps to [`DEFAULT_LIMIT`] HERE — passing a bare `0` through would hit
/// `DeadLetterFilter::capped_limit`'s own floor of 1, so a default request would return a
/// single row (the trap `http::dead_letters::to_filter` documents).
///
/// Timestamps go through [`convert::parse_opt_ts`], NOT `and_then(convert::from_ts)`: the
/// latter maps an unrepresentable value to `None`, which on a filter field means UNFILTERED.
fn to_filter(req: ListDeadLettersRequest) -> Result<DeadLetterFilter, TenancyError> {
    Ok(DeadLetterFilter {
        event_type: opt_string(req.event_type),
        parked_from: convert::parse_opt_ts(req.parked_from, "parked_from")?,
        parked_to: convert::parse_opt_ts(req.parked_to, "parked_to")?,
        cursor: parse_cursor(&req.cursor)?,
        limit: if req.limit == 0 { DEFAULT_LIMIT } else { u64::from(req.limit) },
    })
}

/// A `max_rows` of 0 — which an absent field collapses to — produces an INVALID request that
/// `DeadLetterService::replay_matching` rejects before any store access (design D5). It is
/// deliberately NOT defaulted to anything usable: the explicit row budget is the guard.
///
/// Same strict timestamp handling as [`to_filter`], and it matters more here: a silently
/// dropped bound turns a narrowly-scoped bulk replay into "replay everything up to `max_rows`".
fn to_bulk_request(req: BulkReplayDeadLettersRequest) -> Result<BulkReplayRequest, TenancyError> {
    Ok(BulkReplayRequest {
        event_type: opt_string(req.event_type),
        parked_from: convert::parse_opt_ts(req.parked_from, "parked_from")?,
        parked_to: convert::parse_opt_ts(req.parked_to, "parked_to")?,
        max_rows: req.max_rows,
    })
}

fn parse_id(raw: &str) -> Result<Uuid, TenancyError> {
    Uuid::parse_str(raw).map_err(|_| TenancyError::InvalidPrn("id must be a uuid".to_string()))
}

#[tonic::async_trait]
impl OutboxService for OutboxGrpc {
    /// Root-only. `next_cursor` is the last returned entry's id when the page came back FULL,
    /// else empty — the same keyset convention `grpc::audit::list_audit_entries` uses.
    async fn list_dead_letters(&self, request: Request<ListDeadLettersRequest>) -> Result<Response<ListDeadLettersResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ListDeadLettersResponse>, Status> = async {
            let actor = actor_prn(&actor_context(&request)?);
            let filter = to_filter(request.into_inner()).map_err(convert::status_to_grpc)?;
            let limit = filter.capped_limit();
            let entries = self.state.dead_letters.list(&actor, filter).await.map_err(convert::status_to_grpc)?;
            let next_cursor = if entries.len() as u64 == limit {
                entries.last().map_or_else(String::new, |e| e.id.to_string())
            } else {
                String::new()
            };
            Ok(Response::new(ListDeadLettersResponse {
                entries: entries.iter().map(convert::to_proto_dead_letter_entry).collect(),
                next_cursor,
            }))
        }
        .await;
        record_grpc("Outbox", "ListDeadLetters", started, &result);
        result
    }

    /// Root-only. `NotFound` covers an absent id, a live row, and a row another actor already
    /// replayed or discarded.
    async fn replay_dead_letter(&self, request: Request<ReplayDeadLetterRequest>) -> Result<Response<ReplayDeadLetterResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ReplayDeadLetterResponse>, Status> = async {
            let actor = actor_prn(&actor_context(&request)?);
            let id = parse_id(&request.into_inner().id).map_err(convert::status_to_grpc)?;
            let entry = self.state.dead_letters.replay(&actor, id).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ReplayDeadLetterResponse {
                entry: Some(convert::to_proto_dead_letter_entry(&entry)),
            }))
        }
        .await;
        record_grpc("Outbox", "ReplayDeadLetter", started, &result);
        result
    }

    /// Root-only. A discarded row is gone forever — its audit entry is its only remaining trace.
    async fn discard_dead_letter(&self, request: Request<DiscardDeadLetterRequest>) -> Result<Response<DiscardDeadLetterResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<DiscardDeadLetterResponse>, Status> = async {
            let actor = actor_prn(&actor_context(&request)?);
            let id = parse_id(&request.into_inner().id).map_err(convert::status_to_grpc)?;
            let entry = self.state.dead_letters.discard(&actor, id).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(DiscardDeadLetterResponse {
                entry: Some(convert::to_proto_dead_letter_entry(&entry)),
            }))
        }
        .await;
        record_grpc("Outbox", "DiscardDeadLetter", started, &result);
        result
    }

    /// Root-only. A missing or zero `max_rows` is rejected before any store access — the
    /// explicit row budget is the guard on blast radius, never defaulted to anything usable.
    async fn bulk_replay_dead_letters(
        &self,
        request: Request<BulkReplayDeadLettersRequest>,
    ) -> Result<Response<BulkReplayDeadLettersResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<BulkReplayDeadLettersResponse>, Status> = async {
            let actor = actor_prn(&actor_context(&request)?);
            let req = to_bulk_request(request.into_inner()).map_err(convert::status_to_grpc)?;
            let replayed = self.state.dead_letters.replay_matching(&actor, req).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(BulkReplayDeadLettersResponse { replayed }))
        }
        .await;
        record_grpc("Outbox", "BulkReplayDeadLetters", started, &result);
        result
    }
}
```

- [ ] **Step 4: Register the service in `grpc/mod.rs`**

Add `pub mod dead_letters;`, `use dead_letters::OutboxGrpc;`, the `OutboxServiceServer` import,
and — in the **unconditional** chain, not inside the `if audit_enabled` branch:

```rust
        .add_service(OutboxServiceServer::new(OutboxGrpc::new(state.clone())))
```

Extend `mod.rs`'s module doc and `router()`'s doc to name `OutboxService`, and state there why
it is unconditional while `AuditService` is not.

- [ ] **Step 5: Run the unit tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --workspace && cargo nextest run -p paigasus-iam --lib -E 'test(to_filter) or test(to_bulk_request) or test(max_rows)'
```
Expected: 9 passed.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/
git commit -m "feat(rs): serve the outbox dead-letter surface over grpc (SMA-501)"
```

---

### Task 7: `RetireSystemPolicy` on `AuthorizationService`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/authz.rs`

**Files:** additionally modify `contracts/proto/paigasus/iam/v1/iam.proto` and commit the
regenerated bindings — see Step 0.

**Interfaces:**
- Consumes: `convert::to_proto_retire_response` (Task 4), `require_authz_admin` (existing, `authz.rs:83`), Task 1's generated message types.
- Produces: nothing further.

- [ ] **Step 0: Declare the RPC in the contract and regenerate**

Task 1 deliberately left this line out: `AuthorizationService` already has a concrete
implementor (`AuthzGrpc`), so declaring an RPC without its handler is `error[E0046]` and would
have broken the build for every task in between. Declaring it and implementing it in this one
commit keeps the workspace green throughout.

Find the `service AuthorizationService { … }` block and add one line after `ListRoleGrants`:

```proto
  // Root-only, and additionally gated on the iam.authz.cedar capability,
  // mirroring the HTTP route's placement behind caps.authz_admin.
  rpc RetireSystemPolicy(RetireSystemPolicyRequest) returns (RetireSystemPolicyResponse);
```

Then, from the repo root:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf format -w && buf lint && buf breaking --against '../.git#branch=main,subdir=contracts' && buf generate
```
Expected: all exit 0. Adding an RPC to an existing service is additive, so `buf breaking` passes.

Also update the file header's service list, which Task 1 left saying `AuthorizationService` had
not yet gained this RPC.

- [ ] **Step 1: Add the RPC to the existing `impl AuthorizationService for AuthzGrpc`**

Follow the surrounding RPCs' shape exactly — `require_authz_admin` first, then the actor, then
the service call, then `record_grpc`.

```rust
    /// `RetireSystemPolicy`: Root-only (enforced inside `SystemRetirementService::retire`), and
    /// additionally gated on `iam.authz.cedar` — mirroring HTTP, where
    /// `system_retirement::router()` is merged only under `caps.authz_admin`.
    ///
    /// **All three outcomes return `OK`**, discriminated by the response `oneof`: the two
    /// refusals are outcomes that are not `Retired`, not server errors (design D3, and the same
    /// argument `http::system_retirement`'s module doc makes). This DIVERGES from HTTP, which
    /// answers both refusals with a 409 carrying a registry error code — the payload fields are
    /// identical, the status is not. A consequence worth knowing: `record_grpc` labels a
    /// refusal `grpc_status="ok"`, so refusals do not feed the gRPC error-rate alert.
    ///
    /// The outcome -> response mapping lives in `convert::to_proto_retire_response`, a free
    /// function over an owned `RetireOutcome`, so every variant stays testable without an
    /// `AppState` — see its doc for the regression that made that necessary.
    async fn retire_system_policy(
        &self,
        request: Request<RetireSystemPolicyRequest>,
    ) -> Result<Response<RetireSystemPolicyResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<RetireSystemPolicyResponse>, Status> = async {
            require_authz_admin(&self.state)?;
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let outcome = self
                .state
                .retirement
                .retire(&actor, &req.policy_id, req.acknowledge_decision_change)
                .await
                .map_err(convert::status_to_grpc)?;
            Ok(Response::new(convert::to_proto_retire_response(outcome)))
        }
        .await;
        record_grpc("Authorization", "RetireSystemPolicy", started, &result);
        result
    }
```

Add `RetireSystemPolicyRequest, RetireSystemPolicyResponse` to the file's existing
`paigasus_proto::paigasus::iam::v1::{…}` import list.

- [ ] **Step 2: Build**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --workspace
```
Expected: success. A missing-method error here means the RPC name in Task 1's proto and the
`snake_case` method name disagree.

- [ ] **Step 3: Run the whole lib test suite as a regression check**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib
```
Expected: all pass.

- [ ] **Step 4: Commit**

Stage the proto, the regenerated bindings, AND the handler — the whole point of Step 0 is that
the contract change and its implementor land in ONE commit, so the workspace is never left with
an RPC declared on a trait that `AuthzGrpc` does not implement (`error[E0046]`).

```bash
git add contracts/proto/paigasus/iam/v1/iam.proto \
        rs/crates/libs/paigasus-proto/src/generated/ \
        py/packages/paigasus-proto/ \
        ts/packages/paigasus-proto/ \
        rs/crates/services/paigasus-iam/src/adapters/grpc/authz.rs
git commit -m "feat(rs): serve system-policy retirement over grpc (SMA-501)"
```

---

### Task 8: Integration suite — `grpc_users.rs`

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/grpc_users.rs`

**Interfaces:**
- Consumes: `mod support` (`support::start_migrated_postgres`, `support::start_mock_idp`, `support::test_config`, `support::provision`), Task 5's `UserService`.

**Model this file on `tests/grpc_tenancy.rs`** — copy its harness verbatim: ephemeral Postgres
via the shared Docker policy, the HTTPS mock IdP, `AppState::new`, `grpc::router` over an
ephemeral `TcpListener`, and the `authed` bearer wrapper.

- [ ] **Step 1: Write the suite**

Required tests, each with a doc comment saying what a mutation would have to break to pass:

1. `create_user_over_grpc_mints_a_principal` — a bearer-authenticated call returns a
   `principal_prn` parsing as a valid `Prn`.
2. `a_duplicate_email_is_already_exists` — creating the same email twice; second call is
   `Code::AlreadyExists`.
3. `a_malformed_email_is_invalid_argument` — `"not-an-email"` yields `Code::InvalidArgument`
   **and no principal is created**.
4. `an_empty_locale_becomes_unset` — asserts D11's sentinel over the wire.
5. `create_user_requires_a_bearer_but_no_authorization` — **the D0 pin.** Two assertions in one
   test: an unauthenticated call is `Code::Unauthenticated` (proving `UserService` is not on
   `is_exempt`'s allowlist), and a call by an **ordinary, non-admin** principal **succeeds**.
   Doc comment must state this asserts a deliberate design decision (parity with HTTP, which
   has no `Action::CreateUser`), so a future reader who tightens authorization on one transport
   sees this test fail and is forced to consider the other.

- [ ] **Step 2: Run the suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && env -u CI PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_users
```
`PAIGASUS_REQUIRE_DOCKER=1` is required: this is a FILTERED run, so `docker_preflight.rs` is not
in the filter and a silently skipped suite would look like a pass. `env -u CI` clears a stray
`CI` var, which is presence-based.
Expected: 5 passed.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/grpc_users.rs
git commit -m "test(rs): cover CreateUser over grpc including its deliberate lack of authz (SMA-501)"
```

---

### Task 9: Integration suite — `grpc_dead_letters.rs`

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/grpc_dead_letters.rs`

**Model this on `tests/http_dead_letters.rs`** for the scenarios and on `tests/grpc_authz.rs`
for the harness. Copy `seed_parked_with_details` from `http_dead_letters.rs:31` — it is private
to that file, and duplicating a private seeder across suites is this crate's established posture
(`relay_nudge_pg.rs` documents doing exactly that).

Root authorization is satisfied with `support::provision_platform_admin`, exactly as
`http_dead_letters.rs:82` does — `DeadLetterService` authorizes at `root_prn()` and that helper
seeds a grant there.

- [ ] **Step 1: Write the suite**

Required tests:

1. `a_non_root_caller_is_permission_denied` — for **all four** RPCs, and asserts the error
   carries nothing about the dead-letter contents.
2. `list_returns_seeded_parked_rows` — asserts field values, not just a count, so a broken
   projection fails here.
3. `replay_is_not_idempotent_and_the_second_call_is_not_found`.
4. `discard_removes_the_row_from_a_subsequent_list`.
5. `bulk_replay_without_max_rows_is_invalid_argument` — the D5 pin, over the wire.
6. `bulk_replay_with_max_rows_replays_matching_rows`.
7. `a_present_but_invalid_parked_from_is_rejected_not_ignored` — **the D10 pin, over the wire.**
   Send `prost_types::Timestamp { seconds: 0, nanos: -1 }` on `BulkReplayDeadLetters` with a
   valid `max_rows`, seed rows that an unfiltered replay WOULD match, assert
   `Code::InvalidArgument`, and then assert those rows are **still parked** — proving the
   request was rejected rather than silently widened. A unit test cannot prove this last part.
8. `outbox_rpcs_not_exempt` — modelled on `api_keys_grpc.rs::management_rpcs_not_exempt`: every
   one of the four RPCs is `Code::Unauthenticated` without a bearer.

- [ ] **Step 2: Run the suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && env -u CI PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_dead_letters
```
Expected: 8 passed.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/grpc_dead_letters.rs
git commit -m "test(rs): cover the outbox dead-letter grpc surface end to end (SMA-501)"
```

---

### Task 10: Integration suite — `grpc_system_retirement.rs`

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/grpc_system_retirement.rs`

**Read `tests/authz_system_retirement_pg.rs` first.** Beyond re-declaring its private seeders
(`seed_orphan_chain`, `seed_grants`, `seed_system_policy_with_revision`), this suite must drive
`converge_starter_set` through the real boot path — the retirement service refuses on a
`min_starter_revision` fleet-convergence guard until the starter set has converged. That
seeding requirement, not any authorization difference, is why this is a separate suite.

- [ ] **Step 1: Write the suite**

Required tests:

1. `retiring_an_orphan_template_returns_the_retired_variant` — asserts `policy_id`, `kind`,
   `role_deleted`, and that the row is actually gone.
2. `surviving_grants_return_the_blocked_variant_with_the_true_total` — seeds grants, asserts the
   `Blocked` variant carries the grant list, the **true** `total_surviving`, and `truncated`;
   and asserts **nothing was written** (the policy still exists).
3. `a_static_policy_without_acknowledgement_returns_the_needs_acknowledgement_variant` — asserts
   the preview fields (`kind`, `source`, `description`) and that nothing was written.
4. `an_acknowledged_static_policy_retires` — the same request with
   `acknowledge_decision_change: true` returns `Retired`.
5. `a_non_root_caller_is_permission_denied`.
6. `retire_requires_a_bearer` — `Code::Unauthenticated` without one.
7. `retire_is_unimplemented_when_authz_admin_is_disabled` — build the state with
   `authz.admin_enabled = false` and assert `require_authz_admin`'s capability status, matching
   HTTP's 404-by-non-registration.

Every refusal test must assert the response is **`OK` with the right `oneof` variant**, never an
error status — that is the D3 contract, and asserting only "not Retired" would let an
accidental error mapping pass.

- [ ] **Step 2: Run the suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && env -u CI PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_system_retirement
```
Expected: 7 passed.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/grpc_system_retirement.rs
git commit -m "test(rs): cover system-policy retirement over grpc for all three outcomes (SMA-501)"
```

---

### Task 11: Documentation corrections and the full gate run

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/dead_letters.rs` (module doc)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` (header)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs` (`is_exempt`'s doc, lines 120-128)
- Modify: `docs/ops/RUNBOOK-observability.md:2398-2401`

- [ ] **Step 1: Rewrite `http/dead_letters.rs`'s module doc**

The final paragraph currently reads: "This is an operator-only break-glass surface and is
deliberately HTTP-only: unlike the audit read API it has no gRPC mirror, which keeps
`contracts/` untouched. That is a scope decision, not an API-boundary principle."

Rewrite it — do not delete it. It must record that SMA-501 reversed that scope decision, that
the surface now has a gRPC mirror in `grpc::dead_letters`, and that the two adapters are
independently hand-written over one application service with the twin tests as the drift guard.

- [ ] **Step 2: Rewrite the RUNBOOK entry**

`docs/ops/RUNBOOK-observability.md:2398-2401` lists "A gRPC mirror of the
`/v1/outbox/dead-letters` surface … untracked, no follow-up issue filed" under known gaps. This
is the doc an on-call operator reads. Remove it from the gaps list and state that the mirror
shipped in SMA-501, naming the RPCs so an operator can find them.

- [ ] **Step 3: Update `http/mod.rs`'s header and `is_exempt`'s doc**

`http/mod.rs`'s header enumerates the surfaces — note the gRPC mirrors. `grpc/authn.rs`'s
`is_exempt` doc enumerates what is deliberately NOT exempt; add `UserService` and
`OutboxService` to that list, and note `UserService.CreateUser` is bearer-enforced despite
performing no authorization check of its own.

- [ ] **Step 4: Verify no registry code got quoted**

Scope the search to what the gate actually reads. `ci/error-registry/check.py:77` sets
`SCAN_GLOB = "**/src/**/*.rs"`, so only Rust files under a `src/` directory can trip it —
searching `docs/` too would just match this plan's own quoted examples and cry wolf.

```bash
grep -rn '"invalid-bulk-replay"\|"grants-survive"\|"decision-change-unacknowledged"' \
  rs/crates/services/paigasus-iam/src
```
Expected: **no output.** Any hit drags that file onto `ci/error-registry/check.py`'s MANIFEST
and reds `repo:error-code-single-site`. Remove the quotes.

Pre-existing hits in files already on the MANIFEST (`application/error.rs`,
`http/system_retirement.rs`) are expected and fine — the check is that no file this branch adds
or edits newly acquires one.

- [ ] **Step 5: Run the full CI graph**

Per-project tasks do not run the repo-level gates. Run what CI runs:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity \
  :release-parity-py :release-parity-ts :publish-metadata --base origin/main --include-relations
```

Expected: green. If Moon reports an unattributed failure, diagnose it with
`jq '.actions[]|select(.status=="failed")' .moon/cache/ciReport.json`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "docs(rs): record that the dead-letter surface now has a grpc mirror (SMA-501)"
```

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
