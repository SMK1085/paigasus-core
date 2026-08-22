# SMA-501 — proto services for the HTTP-only IAM ops endpoints

**Status:** revised after adversarial challenge — awaiting approval
**Date:** 2026-08-22
**Linear:** [SMA-501](https://linear.app/smaschek/issue/SMA-501/contracts-proto-services-for-the-http-only-iam-ops-endpoints)
**ADRs:** [ADR-0004](https://app.notion.com/p/368830e8fbaa81a99777ceb7421b64d7) (proto is the
single source of truth for wire contracts), [ADR-0018](https://app.notion.com/p/3bb830e8fbaa81579b5cc146e505173a)
(Connect-ES over gRPC; no OpenAPI surface)

## Problem

Three IAM surfaces are reachable only over HTTP and have no proto service, so a generated
client has a hole exactly where the ops console needs one:

| Surface | HTTP module | Application service | Authorization today |
|---|---|---|---|
| `POST /v1/users` | `adapters/http/users.rs` | `application::create_user` | **none beyond the bearer** (see D0) |
| `GET /v1/outbox/dead-letters`, `POST .../replay`, `POST .../{id}/replay`, `POST .../{id}/discard` | `adapters/http/dead_letters.rs` | `application::dead_letters` | Root-only, inside the service |
| `POST /v1/authz/system-policies/{id}/retire` | `adapters/http/system_retirement.rs` | `application::system_retirement` | Root-only, inside the service |

Per ADR-0004 these belong in the contract regardless of the console.

For the dead-letter and retirement surfaces the application service authorizes internally, so
the gRPC adapter is thin — the same posture `grpc/audit.rs` has toward `AuditQueryService`.
**`create_user` is not like that**, and D0 deals with it.

## Scope

**In:** the proto definitions, the gRPC handlers, their tests, and the doc corrections the
change makes necessary.

**Out:** the `@paigasus/sdk` client and the TypeScript export surface — see *Acceptance
criteria* and D13. Adding per-action authorization to `create_user` — see D0.

**HTTP handlers undergo no behavior change.** Edits to them are limited to (a) the module-doc
corrections listed under *Documentation corrections* and (b) additions to their `#[cfg(test)]`
test modules, which D9.1 requires because the function under test is private (challenge finding).

## Acceptance criteria

1. All three surfaces are reachable over gRPC with behavior matching their HTTP counterparts,
   including error mapping — **except for the divergences enumerated in D3, D11 and D12**, each
   of which is a deliberate decision recorded there with its reason.
2. ~~`@paigasus/sdk`'s hand-written HTTP client shrinks to cover only the gateway's
   OpenAI-compatible chat surface.~~ **Dropped; moved to [SMA-575](https://linear.app/smaschek/issue/SMA-575).**

### Why AC-1 needed amending

The original AC says "including error mapping" without qualification. D3 deliberately breaks it
for the two retirement refusals, and D11/D12 record two smaller divergences. Leaving AC-1
unqualified would have claimed coverage the design does not deliver.

### Why AC-2 was dropped

The AC presupposes a client that does not exist. `ts/packages/paigasus-sdk/src/index.ts` is
`export {}` — a stub. ADR-0018 §5 says a small hand-written fetch client *will* live in
`@paigasus/sdk/http`; it was never built, so there is nothing to shrink.

It moved to SMA-575, which must carry ADR-0018's warning: the moment `@paigasus/sdk` depends on
`@paigasus/proto`, `ci/affected-graph/run.sh` fails, because its `contracts->proto` case asserts
the affected set *equals* (verified at `ci/affected-graph/run.sh:249-250` — **seven** ids, not
the six ADR-0018 quotes):

```
contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs,paigasus-iam-rs,paigasus-service-info-rs
```

ADR-0018 §5 is **not** amended by this issue.

## Design

### D0 — `create_user` has no authorization, and this issue does not add any

**The premise this spec originally rested on was false.** `create_user` does *not* authorize
internally:

- `application/create_user.rs:103` is `pub async fn execute(&self, cmd: NewUser)` — no `actor`
  parameter. Its own module doc says so: "`CreateUser::execute` has no `actor: &Prn` parameter".
- `adapters/http/users.rs` references `AuthContext` **zero** times.
- There is **no `Action::CreateUser`** in `paigasus-iam-core/src/authz/action.rs`.

Meanwhile all 21 existing `TenancyService` RPCs authorize **in the adapter**:
`if self.state.enforce_tenancy { self.state.authorize.check(actor, Action::X, &root_prn()).await? }`
(`grpc/tenancy.rs:103-109`).

So `/v1/users` is bearer-gated but otherwise unauthorized: **any authenticated principal can mint
user principals** — including an identity the `AuthLayer` JIT-provisions under
`Provisioning::Enabled` (`grpc/authn.rs:165`).

**Decision: mirror HTTP exactly — `CreateUser` over gRPC is bearer-only, with no Cedar check.**
AC-1 is parity, and tightening authorization on an existing endpoint is a behavior change that
belongs in its own issue with its own risk assessment. **[SMA-584](https://linear.app/smaschek/issue/SMA-584)**
proposes `Action::CreateUser` applied to both transports together.

Two independent reviewers flagged this unprompted — the adversarial spec-challenger as a BLOCKER
during design, and CodeRabbit again on the finished branch. Neither disputes the parity
reasoning; both want the hole closed. That convergence is why SMA-584 is filed at High rather
than left as a note here.

**Consequence, and a reversal of an earlier decision: `CreateUser` moves to a dedicated
`UserService` rather than joining `TenancyService`.** Placing the one unauthorized RPC among 21
authorized ones camouflages exactly the property a reviewer most needs to see. A separate service
makes it legible in the contract. This overrides the earlier "fold into `TenancyService`" choice;
`RetireSystemPolicy` still joins `AuthorizationService`, which is uniformly capability-gated.

The gRPC handler must carry a comment stating the absence of a check is deliberate and mirrors
HTTP, and a test must pin it, so a future reader does not "fix" it into an inconsistency between
the transports.

### D1 — Contract shape

Everything lands in the existing `contracts/proto/paigasus/iam/v1/iam.proto` (497 → ~700 lines).

A separate `outbox.proto` was considered. The generators differ — prost/tonic and betterproto2
emit per **package**, protobuf-es per **file** — so a second file would split only the TypeScript
output. But `paigasus.common.v1` is already a four-file package, so per-file TS output is a cost
the repo accepts elsewhere: **this is a "no real benefit either way, keep it simple" call, not a
decisive technical argument** (challenge finding). `AuditService`, the closest analogue, already
lives in `iam.proto`, and `contracts:generate` globs `proto/**/*`, so neither option needs
registration.

Conventions mirror `ListAuditEntriesRequest`: absent `google.protobuf.Timestamp` means
unfiltered, empty string means unfiltered, `uint32 limit` with `0` meaning server default,
`string cursor`/`next_cursor`. `kind` is a `string` because `Policy.kind` is one and
`policy_kind_str` yields `"static"`/`"template"`.

```proto
// ─── Users ────────────────────────────────────────────────────
message CreateUserRequest {
  string email = 1;
  string display_name = 2;
  string locale = 3;         // empty => unset (see D11 — diverges from HTTP)
  string timezone = 4;       // empty => unset (see D11)
}
message CreateUserResponse { string principal_prn = 1; }

// ─── System-policy retirement ─────────────────────────────────
message RetireSystemPolicyRequest {
  string policy_id = 1;
  bool acknowledge_decision_change = 2;   // absent == false == "not acknowledged" (D4)
}
message SurvivingGrant { string id = 1; string principal_prn = 2; string scope_prn = 3; }
message RetiredPolicy { string policy_id = 1; string kind = 2; bool role_deleted = 3; }
message RetirementBlocked {
  string role_key = 1;                    // gRPC-only field; HTTP carries it in prose (D6)
  repeated SurvivingGrant grants = 2;
  uint64 total_surviving = 3;
  bool truncated = 4;
}
message RetirementNeedsAcknowledgement {
  string policy_id = 1;                   // gRPC-only field; HTTP carries it in prose (D6)
  string kind = 2; string source = 3; string description = 4;
}
message RetireSystemPolicyResponse {
  // An UNSET oneof is a protocol error: clients must treat it as failure, never as success (D3).
  oneof outcome {
    RetiredPolicy retired = 1;
    RetirementBlocked blocked = 2;
    RetirementNeedsAcknowledgement needs_acknowledgement = 3;
  }
}

// ─── Outbox dead letters ──────────────────────────────────────
message DeadLetterEntry {
  string id = 1;
  google.protobuf.Timestamp occurred_at = 2;
  string event_type = 3;
  int32 schema_version = 4;
  string aggregate_prn = 5;
  string actor_prn = 6;                       // empty => none
  string payload = 7;                         // JSON string; mirrors AuditEntry.detail_json
  string correlation_id = 8;                  // empty => none
  uint32 attempts = 9;
  google.protobuf.Timestamp parked_at = 10;   // absent => not parked
  string last_error = 11;                     // empty => none
}
message ListDeadLettersRequest {
  string event_type = 1;
  google.protobuf.Timestamp parked_from = 2;  // absent => unfiltered; PRESENT-BUT-INVALID => error (D10)
  google.protobuf.Timestamp parked_to = 3;    // ditto
  string cursor = 4;
  uint32 limit = 5;                           // 0 => server default; clamped to MAX_LIMIT (200)
}
message ListDeadLettersResponse { repeated DeadLetterEntry entries = 1; string next_cursor = 2; }
message ReplayDeadLetterRequest   { string id = 1; }
message ReplayDeadLetterResponse  { DeadLetterEntry entry = 1; }
message DiscardDeadLetterRequest  { string id = 1; }
message DiscardDeadLetterResponse { DeadLetterEntry entry = 1; }
message BulkReplayDeadLettersRequest {
  string event_type = 1;
  google.protobuf.Timestamp parked_from = 2;  // PRESENT-BUT-INVALID => error, never unfiltered (D10)
  google.protobuf.Timestamp parked_to = 3;    // ditto
  // 0 (which an absent field collapses to) => invalid-bulk-replay, exactly as on HTTP. This is
  // DELIBERATE, not an oversight — see D5. Do NOT "fix" it into an `optional`.
  // Silently clamped to MAX_BULK_REPLAY (10_000) by capped_max_rows(), with no signal to the
  // caller — same on both transports.
  uint64 max_rows = 4;
}
message BulkReplayDeadLettersResponse { uint64 replayed = 1; }
```

### D2 — Service grouping

```proto
service UserService {             // new — see D0 for why this is not on TenancyService
  rpc CreateUser(CreateUserRequest) returns (CreateUserResponse);
}
service AuthorizationService {    // + 1 rpc
  rpc RetireSystemPolicy(RetireSystemPolicyRequest) returns (RetireSystemPolicyResponse);
}
service OutboxService {           // new
  rpc ListDeadLetters(ListDeadLettersRequest) returns (ListDeadLettersResponse);
  rpc ReplayDeadLetter(ReplayDeadLetterRequest) returns (ReplayDeadLetterResponse);
  rpc BulkReplayDeadLetters(BulkReplayDeadLettersRequest) returns (BulkReplayDeadLettersResponse);
  rpc DiscardDeadLetter(DiscardDeadLetterRequest) returns (DiscardDeadLetterResponse);
}
```

**The bulk RPC is `BulkReplayDeadLetters`, not `ReplayDeadLetters`** (challenge finding). The
original pair differed by one character while doing very different things — one row versus up to
`BulkReplayRequest::MAX_BULK_REPLAY = 10_000` — on a destructive operator surface, and
`buf.yaml`'s `STANDARD` category has no rule that would catch the typo: `replay_dead_letter` and
`replay_dead_letters` both compile. `BulkReplay*` also matches `BulkReplayRequest`/
`BulkReplayBody`, the names the codebase already uses for this operation.

### D3 — The `RetireOutcome` mapping is a `oneof`, not a `Status`

**All three outcomes return gRPC `OK`, discriminated by the `oneof`.** This encodes the argument
`system_retirement.rs`'s module doc already makes — that the two refusals "[are] not an error at
all, just an outcome that isn't `Retired`" — and mirrors the Rust enum 1:1.

The `FAILED_PRECONDITION` + `ErrorInfo`-metadata alternative was rejected as lossy (the grant list
becomes JSON-in-a-string) and because a typed encoding would want `google.rpc.*`, which per
SMA-498 makes `buf generate` exit `0` while emitting Rust/TS pointing at ungenerated modules.

**Accepted divergences, wider than first stated (challenge finding) — all to be documented in the
proto:**

1. A gRPC client sees a `oneof` variant where HTTP sends `409` plus the registry codes
   `grants-survive` / `decision-change-unacknowledged`.
2. HTTP's `conflict()` sets `paigasus-retryable: false` (`http/system_retirement.rs:150-153`); the
   gRPC `OK` path cannot carry it.
3. `record_grpc` will label a refusal `grpc_status="ok"`, so refusals are invisible to
   `IamHighGrpcErrorRate` (`ops/observability/prometheus/rules/iam.rules.yml:208`). This is
   arguably *correct* — a refusal is not a server error — but it must be stated, not discovered.
4. prost renders `oneof outcome` as `Option<..>`, so an **unset** oneof is representable. The
   proto must state it is a protocol error clients treat as failure; otherwise a naive generated
   client's happy path reads a `Blocked` refusal as a successful retirement.

**Open question carried to the review gate:** whether to additionally put the registry code in
**response metadata** (`Response::metadata_mut`), which closes divergence (1) for the cost of one
`ci/error-registry/check.py` MANIFEST row plus a membership test. See *Open questions*.

### D4 — The refusal messages carry no `error.code` **field**

The `oneof` variant is the discriminator. A redundant `code` field would reintroduce
hand-maintained duplication and make the adapter an error **emission** site. (This rejects a proto
*field*; it does not settle the response-metadata question in D3.)

### D5 — `max_rows`'s sentinel collapse is safe, by luck of the semantics

**Verified end to end.** `BulkReplayRequest::is_valid` is exactly `self.max_rows > 0`
(`paigasus-iam-core/src/dead_letter.rs:79-81`); HTTP's `into_request` already collapses
`Option<u64>` via `unwrap_or(0)` (`http/dead_letters.rs:102`); and `replay_matching` rejects
before any store access. There is no path where absent and explicit `0` differ.

Proto3 cannot express the distinction, and here it does not need to. This must be pinned in a
proto comment so nobody "fixes" it into an `optional`.

### D6 — `SurvivingGrant` is a new message; the parity claim is a **subset** claim

`RoleGrant{id, principal_prn, role_key, scope_prn}` is a superset of `GrantRef{id, principal_prn,
scope_prn}`, so reuse was possible. A dedicated message is used because HTTP's `grants_json`
emits exactly three fields.

**Correction (challenge finding):** the original rationale — "reuse would make gRPC carry a field
HTTP does not" — is contradicted by this spec's own proto. `RetirementBlocked.role_key` and
`RetirementNeedsAcknowledgement.policy_id` are exactly such fields: HTTP emits neither as a field,
only interpolated into `error.message` prose (`http/system_retirement.rs:109-124`). HTTP also
emits `error.code`/`error.message`, which the proto omits per D4.

So the two projections are **not** field-for-field equal, and D9.1's assertion is a **subset check
with a named allowlist**:

- gRPC-only fields: `RetirementBlocked.role_key`, `RetirementNeedsAcknowledgement.policy_id`
- HTTP-only fields: `error.code`, `error.message`
- Everything else must agree exactly.

`SurvivingGrant` is kept anyway: it is the honest three-field mirror, and `role_key` is carried
once at the top level rather than repeated per grant.

### D7 — Adapter placement

| Change | File | Approx. size |
|---|---|---|
| `UserGrpc` (`CreateUser`) | **new** `grpc/users.rs` | ~70 |
| `RetireSystemPolicy` | `grpc/authz.rs` (existing `AuthzGrpc`) | 244 → ~290 |
| `OutboxGrpc` (4 RPCs) | **new** `grpc/dead_letters.rs` | ~250 |
| `to_proto_dead_letter_entry`, `to_proto_retire_response`, timestamp parsing (D10) | `grpc/convert.rs` | 615 → ~710 |
| register `UserServiceServer` + `OutboxServiceServer` | `grpc/mod.rs` | +8 |

`RetireSystemPolicy`'s first statement is `require_authz_admin(&self.state)?`, matching HTTP,
where `system_retirement::router()` merges only under `caps.authz_admin`.

**Every new handler wraps its result in `paigasus_observability::record_grpc(service, method,
started, &result)`** (challenge finding — the original spec omitted this entirely). All 21
`TenancyService` RPCs, all 7 `ServiceAccountService` RPCs, and `audit.rs:144` do. The `service`
label drops the `Service` suffix — the existing labels are `"Tenancy"`, `"Authorization"`,
`"Authentication"`, `"ServiceAccount"`, `"Audit"`, `"ServiceInfo"` — so the new ones are
**`"User"`** and **`"Outbox"`**.

**Type-alias convention** (challenge finding): `paigasus_proto::..::DeadLetterEntry` and
`paigasus_iam_core::DeadLetterEntry` are both in scope in `grpc/dead_letters.rs`. Follow
`audit.rs:21`'s precedent — `use ..::{DeadLetterEntry as ProtoDeadLetterEntry, ..}`.

### D8 — Three shapes that are deliberate, not incidental

**The outcome → response mapping is a pure function over an owned `RetireOutcome`**, in
`convert.rs`, not inlined. Same reasoning that produced HTTP's `response_for`: an earlier revision
changed `Retired`'s status to `204` and the whole suite stayed green
(`http/system_retirement.rs:41-48`). `RetireOutcome` is `Clone`, so one value can feed both
projections.

**`OutboxService` is registered unconditionally**, matching HTTP's ungated
`dead_letters::router()` — and this is *argued*, not merely commented (challenge finding). The
asymmetry is real: `iam.audit`, a **read-only** surface, can be switched off, while the
dead-letter surface, which permanently **discards** events and bulk-replays up to 10 000, cannot
be switched off on either transport. The argument for keeping it ungated is that a break-glass
surface must not be disable-able — the moment you need it is the moment a config flag is hardest
to change. Both are Root-only, so this is an operator-control question, not an authz hole. Adding
`CAPABILITY_IAM_OUTBOX` to `service_info.proto` would be append-only and cheap; see *Open
questions*.

**`UserService` has no authorization check**, deliberately, per D0 — commented at the call site
and pinned by a test.

### D9 — Testing

**1. Twin pairs (Docker-free).** The drift risk this issue creates is two hand-written adapters
over one service. `dto.rs`'s `introspect_api_key_dto_carries_scope_prn` is the existing antidote.

- one `DeadLetterEntry` domain value into both `DeadLetterEntryDto::from` and
  `convert::to_proto_dead_letter_entry`, asserting field-for-field agreement — including each
  `Option` → empty-string / absent-timestamp mapping.
- all three `RetireOutcome` variants into both `http::system_retirement::response_for` and
  `convert::to_proto_retire_response`, asserting the **subset** agreement defined in D6.
- `CreateUserRequest` → `NewUser` alongside `CreateUserBody` → `NewUser`, covering D11's
  `locale`/`timezone` divergence explicitly.

**Where the retire twin test lives** (challenge finding): `mod system_retirement` is private
(`http/mod.rs:25`) and `fn response_for` is private to it, so `convert.rs`'s test module cannot
name it. The test lives in **`http/system_retirement.rs`'s own `#[cfg(test)] mod tests`**, which
can reach `crate::adapters::grpc::convert`. This is why Scope permits test-module additions to
HTTP files. (`DeadLetterEntryDto` needs no such accommodation — `pub mod dto` at `http/mod.rs:18`.)

**2. Unit tests in `grpc/dead_letters.rs`**, mirroring `http/dead_letters.rs`'s battery, including
its two mutation-catching tests: an explicit non-zero `limit` is passed through; and every present
filter field lands on `BulkReplayRequest` with its **exact** value. The second guards the one
security-relevant mutation — silently dropping filters turns a scoped bulk replay into "replay
everything up to `max_rows`".

Plus the D10 negative tests, which the original spec lacked entirely: a **present but
unrepresentable** timestamp (`nanos: -1`, and `seconds: i64::MAX`) must yield `INVALID_ARGUMENT`,
never an unfiltered query — for all four timestamp fields, on both `ListDeadLetters` and
`BulkReplayDeadLetters`.

The adapter must also reproduce two behaviors HTTP implements in the *adapter*, not the service:

- `limit` absent or `0` maps to `DEFAULT_LIMIT` **in the adapter** — passing a bare `0` through
  would hit `capped_limit`'s floor of `1`, returning a single row.
- `next_cursor` is the last entry's id **only when the page came back full**.

**3. Integration (Docker-backed).** All suites use the shared `tests/support/docker.rs` policy;
hand-rolling a skip fails `repo:iam-docker-policy-single-site`.

- **new** `tests/grpc_dead_letters.rs`, mirroring `http_dead_letters.rs`: non-Root denied; list;
  replay-one plus its second-call `NOT_FOUND`; discard; bulk replay rejected without `max_rows`;
  bulk replay happy path. Plus `outbox_rpcs_not_exempt`.
- **new** `tests/grpc_system_retirement.rs`, covering all three outcome variants over the wire.
  **Corrected rationale (challenge finding):** the original reason — "Root-scoped, and
  `grpc_authz.rs` seeds platform_admin" — is a false dichotomy; `DeadLetterService` also
  authorizes at `root_prn()` and `http_dead_letters.rs:82` satisfies it with
  `support::provision_platform_admin`, while `grpc_authz.rs:194` uses `seed_platform_admin` at
  Root. The real reason is **seeding complexity**: beyond re-declaring `seed_orphan_chain` /
  `seed_grants` / `seed_system_policy_with_revision`, the suite must drive `converge_starter_set`
  through the real boot path to clear the D11 `min_starter_revision` fleet-convergence guard.
- **new** `tests/grpc_users.rs`: happy path, duplicate email → conflict, invalid email →
  `INVALID_ARGUMENT`, plus a test pinning D0 (a non-admin bearer succeeds — the absence of a check
  is deliberate).
- Each of the three new suites asserts its own RPCs are bearer-enforced, not just `OutboxService`
  (challenge finding): `management_rpcs_not_exempt` covers only `ServiceAccountService`.

### D10 — Timestamp parsing must reject, never silently unfilter

**The original spec's D10 claim was false** (challenge finding). It asserted an
`InvalidPrn`-as-sentinel idiom "both `http/dead_letters.rs` and `grpc/audit.rs` already use" for
timestamps. `grpc/audit.rs` uses it for `cursor` and `outcome` only. For timestamps it does
`req.from.and_then(convert::from_ts)` (`grpc/audit.rs:91-92`), and `convert::from_ts` returns
`None` for an unrepresentable value (`convert.rs:191-194`: `u32::try_from(t.nanos).ok()?`).

**`None` means unfiltered.** So a client sending `parked_from { nanos: -1 }` would have its time
bound silently dropped — turning a scoped bulk replay into "replay everything up to `max_rows`",
precisely the mutation D9.2 exists to prevent. HTTP rejects the equivalent with a `400`
(`http/dead_letters.rs:61-68`).

**Rule for the new adapter: a present-but-unrepresentable `google.protobuf.Timestamp` is
`INVALID_ARGUMENT`. The `and_then(from_ts)` shape is forbidden here.** Absent stays unfiltered.
A helper in `convert.rs` distinguishing the three cases (absent / valid / invalid) is preferred
over repeating the match at eight call sites.

`from_ts`'s own doc already says "callers map that to a client error themselves", so `audit.rs`
carries the same latent bug. **Fixing `audit.rs` is out of scope**; a follow-up should be filed.

### D11 — `locale`/`timezone` empty-string divergence

`CreateUserBody.locale`/`timezone` are `Option<String>` (`http/dto.rs:189-194`), so an HTTP client
sending `{"locale": ""}` persists `Some("")`. The proto's `empty => unset` maps the same wire
value to `None`.

**The proto sentinel is normative for gRPC; HTTP is unchanged.** Divergence accepted: persisting
an empty string is not a behavior worth mirroring. Covered by a D9.1 twin test.

### D12 — Two smaller divergences, stated rather than discovered

- **Path-extractor errors.** HTTP's `replay_one`/`discard_one` use `Path<Uuid>`, so a malformed
  uuid produces an axum `PathRejection` — *not* the crate's `{"error":{code,message}}` envelope.
  The gRPC twin produces `INVALID_ARGUMENT` + `ErrorInfo`. gRPC is normative; HTTP is not changed.
- **`limit` type.** `uint32` on the wire vs `Option<u64>` on HTTP. Both clamp to
  `DeadLetterFilter::MAX_LIMIT = 200`, so behaviorally equivalent, but not literally identical.

### D13 — The TypeScript export surface stays closed, deliberately

`ts/packages/paigasus-proto/src/index.ts` exports only `audit`, `capability` and `service_info`
symbols, and `package.json`'s `exports` map is `{".": "./src/index.ts"}` with no subpath — so
`generated/paigasus/iam/v1/iam_pb.ts` is unreachable from the package's public API today, and
remains so after this change (challenge finding).

**This is SMA-575's job, not this issue's.** Stated here so it is a decision rather than an
oversight: this issue closes the *contract* half of the hole; the console still cannot import the
result until SMA-575 opens the export surface.

### D14 — Error mapping

| Condition | Mapping |
|---|---|
| `TenancyError` | `convert::status_to_grpc` — Validation→`INVALID_ARGUMENT`, NotFound→`NOT_FOUND`, Conflict→`ALREADY_EXISTS`, Precondition→`FAILED_PRECONDITION`, Forbidden→`PERMISSION_DENIED`, Internal→`INTERNAL` (source never leaked) |
| malformed cursor | `TenancyError::InvalidPrn`-as-sentinel, as `grpc/audit.rs` does → `INVALID_ARGUMENT` |
| present-but-invalid timestamp | `INVALID_ARGUMENT` — **never** `None`/unfiltered (D10) |
| non-Root caller (dead letters, retire) | enforced inside the application service → `PERMISSION_DENIED` |
| any bearer-authenticated caller (`CreateUser`) | **allowed** — no check, mirroring HTTP (D0) |
| `authz.admin_enabled = false` | `convert::capability_disabled("iam.authz.cedar")` |
| no bearer | `AuthLayer`/`AuthEnforce`; `is_exempt` (`grpc/authn.rs:126-128`) is a three-entry allowlist, so all six new RPCs are enforced by construction |

## CI and gate impact

| Gate | Effect |
|---|---|
| `repo:breaking` | **Passes** — `contracts/buf.yaml:15-33` uses category `FILE`; additive RPCs and new services are non-breaking. |
| `contracts:fmt` | Reds unless `buf format -w` is run before commit. |
| codegen drift | `contracts:generate` declares no `outputs:` and can serve a stale cache — run `buf generate` directly. The embedded `FILE_DESCRIPTOR_SET` shifts, so bindings must be regenerated even for whitespace-only edits. |
| `ci/affected-graph/run.sh` | **Unchanged** — no new crate, no new Moon project. Its `contracts->proto` case keys on `health.proto`. |
| `repo:error-code-single-site` | **No MANIFEST entry needed** *provided* no new or rewritten file spells a registry code in **double quotes**. `scan()` matches `"<code>"` anywhere in a file with no production/test split; `invalid-bulk-replay`, `grants-survive` and `decision-change-unacknowledged` are all declared. `http/dead_letters.rs:134` writes `400 invalid-bulk-replay` unquoted today, which is why it is off the MANIFEST — the D5 proto comment and the rewritten docs must keep that discipline. *(If the D3 open question resolves toward response metadata, this changes: one MANIFEST row plus a membership test.)* |
| `repo:observability-drift` | Untouched — `record_grpc`'s labels are compile-time literals and dashboards aggregate by label. |
| `repo:iam-docker-policy-single-site` | New suites must use `tests/support/docker.rs`; `docker_preflight.rs` remains the canary. |
| `repo:input-liveness` | Untouched. |

`tests/http_authn.rs`'s route table needs no update: no HTTP route is added.

## Documentation corrections

Each records *why* the old shape was chosen, so each must be rewritten, not deleted:

1. **`adapters/http/dead_letters.rs`** module doc — "deliberately HTTP-only … keeps `contracts/`
   untouched. That is a scope decision, not an API-boundary principle."
2. **`docs/ops/RUNBOOK-observability.md:2398-2401`** — "**A gRPC mirror of the
   `/v1/outbox/dead-letters` surface**, if a non-HTTP operator client ever needs one — untracked,
   no follow-up issue filed." The operator-facing doc, and the one on-call actually reads.
3. **`tests/grpc_tenancy.rs:134-136`** — "`TenancyService` has no `CreateUser` RPC (users stay
   HTTP-only per Task 15)". Still true of `TenancyService` after D0, but the parenthetical is not.
4. **`contracts/proto/paigasus/iam/v1/iam.proto:9-16`** — the file header enumerating services,
   already stale for `ServiceAccountService`/`AuditService`.
5. **`adapters/grpc/authn.rs:120-128`** — `is_exempt`'s doc, which enumerates what is deliberately
   *not* exempt.
6. **`adapters/grpc/mod.rs`** module doc and `router()` doc.
7. **`adapters/http/mod.rs`** header.

## Open questions for the review gate

1. **Response metadata for the retirement refusal codes (D3).** Adding the registry code to
   `Response::metadata_mut` closes the "a gRPC client cannot observe the codes" divergence for one
   `ci/error-registry/check.py` MANIFEST row plus a membership test. Recommendation: **defer** —
   the `oneof` variant already discriminates, and no known consumer needs the string. Cheap to add
   later; the contract does not change.
2. **Capability-gating `OutboxService` (D8).** `CAPABILITY_IAM_OUTBOX` in `service_info.proto` is
   append-only and cheap. Recommendation: **do not gate** — a break-glass surface must not be
   disable-able, and HTTP is ungated today, so gating gRPC alone would itself be a divergence.
3. **`CreateUser` on a dedicated `UserService` (D0).** This reverses an earlier decision to fold
   it into `TenancyService`. Recommendation: **keep the reversal**.
4. **A `total`/count on `ListDeadLetters`.** HTTP has none. Recommendation: **reject** — AC-1 is
   parity, a count means a second query per list on a Root-only break-glass surface, and adding
   the field later is non-breaking.

## Risks

- **`BulkReplayDeadLetters`' partial-failure contract is undefined** (challenge finding). It is a bulk
  mutation under tonic's `Server::timeout`, whose `Status` is produced *outside* `CorrelationLayer`
  and carries no ids or `ErrorInfo` — a gap `grpc/mod.rs` already documents as accepted. A client
  receiving `DEADLINE_EXCEEDED` mid-replay cannot tell how many rows were replayed. This is
  **pre-existing on HTTP** and not made worse here. **Resolved as far as the contract can:**
  `BulkReplayDeadLettersRequest` now states the operation is not atomic and that re-issuing is
  safe, because every replay statement carries `AND parked = true` — an already-replayed row no
  longer matches. The underlying "how many succeeded" question remains unanswerable on both
  transports.
- **Parity is asserted, not structurally guaranteed.** Nothing statically forces a future `/v1`
  route to acquire an RPC. A `repo:*` route↔RPC gate was rejected as disproportionate: a new
  `repo:*` task must join `ci.yml`'s `T=(…)`, the CLAUDE.md marker block, `ci_targets.py`, and
  `input-liveness`.
- **Three new Docker-backed IAM suites**, the flaky ones under parallel load. They inherit
  `rs/.config/nextest.toml`'s retry budget automatically; no `--retries` is added anywhere.
- **`/v1/users` remains effectively unauthenticated-beyond-bearer on both transports** (D0), and
  this issue widens its reachable surface without widening the hole. The follow-up matters.
