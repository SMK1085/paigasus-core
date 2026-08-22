# SMA-501 — proto services for the HTTP-only IAM ops endpoints

**Status:** approved (design)
**Date:** 2026-08-22
**Linear:** [SMA-501](https://linear.app/smaschek/issue/SMA-501/contracts-proto-services-for-the-http-only-iam-ops-endpoints)
**ADRs:** [ADR-0004](https://app.notion.com/p/368830e8fbaa81a99777ceb7421b64d7) (proto is the
single source of truth for wire contracts), [ADR-0018](https://app.notion.com/p/3bb830e8fbaa81579b5cc146e505173a)
(Connect-ES over gRPC; no OpenAPI surface)

## Problem

Three IAM surfaces are reachable only over HTTP and have no proto service, so a generated
client has a hole exactly where the ops console needs one:

| Surface | HTTP module | Application service |
|---|---|---|
| `POST /v1/users` | `adapters/http/users.rs` | `application::create_user` |
| `GET /v1/outbox/dead-letters`, `POST .../replay`, `POST .../{id}/replay`, `POST .../{id}/discard` | `adapters/http/dead_letters.rs` | `application::dead_letters` |
| `POST /v1/authz/system-policies/{id}/retire` | `adapters/http/system_retirement.rs` | `application::system_retirement` |

Per ADR-0004 these belong in the contract regardless of the console. All three application
services already exist and already enforce their own authorization internally, so the gRPC
adapters are thin — the same posture `grpc/audit.rs` has toward `AuditQueryService`.

## Scope

**In:** the proto definitions, the gRPC handlers, their tests, and the doc corrections the
change makes necessary.

**Out:** the `@paigasus/sdk` client (see *Acceptance criteria* below). The HTTP handlers undergo
**no behavior change** and no shared-parse refactor between the two transports — the only edits
to them are the module-doc corrections listed under *Documentation corrections*.

## Acceptance criteria

1. All three surfaces are reachable over gRPC with behavior matching their HTTP counterparts,
   including error mapping. *(Unchanged from the Linear issue.)*
2. ~~`@paigasus/sdk`'s hand-written HTTP client shrinks to cover only the gateway's
   OpenAI-compatible chat surface.~~ **Dropped from this issue; moved to a follow-up.**

### Why AC-2 was dropped

The AC presupposes a client that does not exist. `ts/packages/paigasus-sdk/src/index.ts` is
`export {}` — a stub. ADR-0018 §5 says a small hand-written fetch client *will* live in
`@paigasus/sdk/http` covering these three IAM surfaces plus the gateway's chat endpoint; it was
never built. There is therefore nothing to shrink, and AC-2 cannot be demonstrated as written.

It moves to a follow-up issue tied to the ops-console screens, which must carry ADR-0018's own
warning: the moment `@paigasus/sdk` depends on `@paigasus/proto`, `ci/affected-graph/run.sh`'s
`contracts->proto` case fails, because it asserts the affected set *equals*
`contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs,paigasus-iam-rs`
and strict equality rejects any extra project. That expected set must gain `paigasus-sdk-ts` and
everything downstream of it in the same change.

ADR-0018 §5 is **not** amended by this issue. It stays accurate until the follow-up decides
whether the shrunken client is built at all.

## Design

### D1 — Contract shape

Everything lands in the existing `contracts/proto/paigasus/iam/v1/iam.proto` (497 → ~700
lines).

A separate `outbox.proto` was considered and rejected. prost and tonic generate **per package**,
so a second file in `package paigasus.iam.v1` would append to the same
`generated/paigasus/iam/v1/paigasus.iam.v1.rs` and need no `lib.rs` change; betterproto2 is also
per-package. But protobuf-es generates **per file**, so it would split the TypeScript surface
into a second `outbox_pb.ts` for no benefit. `AuditService` — the closest analogue, also an ops
read surface — already lives in `iam.proto`. `contracts:generate` globs `proto/**/*`, so neither
option needs registration anywhere.

Conventions are mirrored from `ListAuditEntriesRequest`, the near-exact analogue: absent
`google.protobuf.Timestamp` means unfiltered, empty string means unfiltered, `uint32 limit` with
`0` meaning server default, `string cursor` / `string next_cursor`. `kind` is a `string` because
the existing `Policy.kind` is one and `policy_kind_str` already yields `"static"` / `"template"`.

```proto
// ─── Users ────────────────────────────────────────────────────
message CreateUserRequest {
  string email = 1;
  string display_name = 2;
  string locale = 3;         // empty => unset
  string timezone = 4;       // empty => unset
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
  string role_key = 1;
  repeated SurvivingGrant grants = 2;
  uint64 total_surviving = 3;
  bool truncated = 4;
}
message RetirementNeedsAcknowledgement {
  string policy_id = 1; string kind = 2; string source = 3; string description = 4;
}
message RetireSystemPolicyResponse {
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
  google.protobuf.Timestamp parked_from = 2;
  google.protobuf.Timestamp parked_to = 3;
  string cursor = 4;
  uint32 limit = 5;                           // 0 => server default
}
message ListDeadLettersResponse { repeated DeadLetterEntry entries = 1; string next_cursor = 2; }
message ReplayDeadLetterRequest   { string id = 1; }
message ReplayDeadLetterResponse  { DeadLetterEntry entry = 1; }
message DiscardDeadLetterRequest  { string id = 1; }
message DiscardDeadLetterResponse { DeadLetterEntry entry = 1; }
message ReplayDeadLettersRequest {
  string event_type = 1;
  google.protobuf.Timestamp parked_from = 2;
  google.protobuf.Timestamp parked_to = 3;
  uint64 max_rows = 4;   // 0 (which absent collapses to) => invalid-bulk-replay, as on HTTP
}
message ReplayDeadLettersResponse { uint64 replayed = 1; }
```

### D2 — Service grouping

Each RPC lands where its HTTP route already lives, so no existing service's meaning is
stretched:

```proto
service TenancyService {          // + 1 rpc
  rpc CreateUser(CreateUserRequest) returns (CreateUserResponse);
}
service AuthorizationService {    // + 1 rpc
  rpc RetireSystemPolicy(RetireSystemPolicyRequest) returns (RetireSystemPolicyResponse);
}
service OutboxService {           // new
  rpc ListDeadLetters(ListDeadLettersRequest) returns (ListDeadLettersResponse);
  rpc ReplayDeadLetter(ReplayDeadLetterRequest) returns (ReplayDeadLetterResponse);
  rpc ReplayDeadLetters(ReplayDeadLettersRequest) returns (ReplayDeadLettersResponse);
  rpc DiscardDeadLetter(DiscardDeadLetterRequest) returns (DiscardDeadLetterResponse);
}
```

`CreateUser` joins `TenancyService` because it mints a user principal and `http/users.rs`
already describes itself as mirroring `organizations.rs`. `RetireSystemPolicy` joins
`AuthorizationService` because its route is `/v1/authz/*` and `config.rs` groups it with
`/v1/authz/policies*`. The four dead-letter RPCs are a genuinely new domain and get a new
service.

### D3 — The `RetireOutcome` mapping is a `oneof`, not a `Status`

`SystemRetirementService::retire` returns three outcomes. On HTTP, `Retired` is a `200`, and
`Blocked` / `NeedsAcknowledgement` are hand-built `409`s carrying structured payloads *next to*
the standard `error` envelope, deliberately routed around `ApiError`. `system_retirement.rs`'s
module doc argues these "[are] not an error at all, just an outcome that isn't `Retired`".

gRPC has no equivalent of "a `409` with a rich body", so:

**All three outcomes return gRPC `OK`, discriminated by the `oneof`.** This encodes exactly the
argument the HTTP module already makes, and mirrors the Rust enum 1:1.

The alternative — `FAILED_PRECONDITION` with the payload flattened into `ErrorInfo` string
metadata — was rejected on two grounds. It is lossy: the surviving-grant list would become
JSON-in-a-string, reintroducing precisely the untyped hand-maintained shape this issue exists to
remove. And a typed encoding would want `google.rpc.*`, which per SMA-498 makes `buf generate`
exit `0` while emitting Rust/TS that points at ungenerated modules — only well-known types are
special-cased.

**Accepted divergence, to be documented in the proto:** a gRPC client sees a `oneof` variant
where an HTTP client sees a `409` and the registry-declared codes `grants-survive` /
`decision-change-unacknowledged`. The payload fields are identical; the status code and the code
string are not.

### D4 — The refusal messages carry no `error.code` string

The `oneof` variant *is* the discriminator. Adding a redundant `code` field would reintroduce
hand-maintained duplication and — more concretely — would make the gRPC adapter an error
**emission** site, requiring a new `ci/error-registry/check.py` `MANIFEST` entry plus a
membership test. Both costs buy nothing a client cannot get from the variant.

### D5 — `max_rows`'s sentinel collapse is safe, by luck of the semantics

HTTP models `max_rows` as `Option<u64>` specifically so an omitted field is distinguishable from
an explicit `0`. Proto3 scalars cannot represent that distinction.

It does not matter here: `BulkReplayRequest::is_valid` rejects **both** identically, and
`DeadLetterService::replay_matching` turns that into `TenancyError::InvalidBulkReplay` before any
store access. So absent → `0` → `invalid-bulk-replay` is exactly HTTP's behavior. The explicit
row budget remains the guard on blast radius; it is still never defaulted to anything usable.

This is the one field where the house sentinel style would normally be wrong, and it happens not
to be. It must be stated in the proto comment so a future reader does not "fix" it into an
`optional`.

### D6 — `SurvivingGrant` is a new message rather than a reused `RoleGrant`

The existing `RoleGrant{id, principal_prn, role_key, scope_prn}` is a superset of Rust's
`GrantRef{id, principal_prn, scope_prn}`, and `role_key` is known at the call site, so reuse
would invent no types and tell no lies.

A dedicated message is chosen anyway because AC-1 is *parity*: HTTP's `grants_json` emits exactly
three fields, and reusing `RoleGrant` would make the gRPC surface carry a field its HTTP twin
does not — which also weakens the D9 twin test from a field-for-field match to a subset check.

### D7 — Adapter placement

tonic requires one impl per service trait, so placement follows from D2:

| Change | File | Approx. size |
|---|---|---|
| `CreateUser` | `grpc/tenancy.rs` (existing `TenancyGrpc`) | 636 → ~670 |
| `RetireSystemPolicy` | `grpc/authz.rs` (existing `AuthzGrpc`) | 244 → ~290 |
| `OutboxGrpc` (4 RPCs) | **new** `grpc/dead_letters.rs` | ~230 |
| `to_proto_dead_letter_entry`, `to_proto_retire_response` | `grpc/convert.rs` | 615 → ~700 |
| register `OutboxServiceServer` | `grpc/mod.rs` | +4 |

`RetireSystemPolicy`'s first statement is `require_authz_admin(&self.state)?`, matching HTTP,
where `system_retirement::router()` is merged only under `caps.authz_admin`. The gRPC precedent
is `grpc/authz.rs`'s existing six call sites, which return
`convert::capability_disabled("iam.authz.cedar")` rather than declining to register the service.

### D8 — Two shapes that are deliberate, not incidental

**The outcome → response mapping is a pure function over an owned `RetireOutcome`**, living in
`convert.rs`, not inlined in the handler. This is the same reasoning that produced HTTP's
`response_for`: an earlier revision of `system_retirement.rs` changed `Retired`'s status to `204`
and the whole crate's suite stayed green, because nothing exercised the mapping against a real
outcome value. A free function taking an owned enum lets every variant be constructed directly in
a test with no `AppState`, database, or request.

**`OutboxService` is registered unconditionally**, matching HTTP's ungated
`dead_letters::router()`. This needs an explicit comment in `grpc/mod.rs`, because the
neighbouring `AuditService` *is* capability-gated (`if audit_enabled`) and the asymmetry
otherwise reads as an oversight.

### D9 — Testing

Three layers.

**1. Twin pairs (Docker-free).** The drift risk this issue creates is that HTTP and gRPC are now
two independently hand-written adapters over one application service. The codebase already has
the antidote: `dto.rs`'s `introspect_api_key_dto_carries_scope_prn` notes it is "paired with
`convert.rs`'s twin gRPC test — deterministically guarantees the two wire shapes agree without
PG/Redis". Mirror it:

- one `DeadLetterEntry` domain value into both `DeadLetterEntryDto::from` and
  `convert::to_proto_dead_letter_entry`, asserting field-for-field agreement — including each
  `Option` → empty-string / absent-timestamp sentinel mapping;
- all three `RetireOutcome` variants into both `http::system_retirement::response_for` and
  `convert::to_proto_retire_response`, asserting the payload fields agree.

**2. Unit tests in `grpc/dead_letters.rs`**, mirroring `http/dead_letters.rs`'s `to_filter` /
`into_request` battery — explicitly including its two mutation-catching tests:

- an explicit non-zero `limit` is passed through (a hardcoded `DEFAULT_LIMIT` otherwise passes
  every other test in the module);
- every present filter field lands on `BulkReplayRequest` with its **exact** expected value
  (`is_some()` would pass even if `event_type` were dropped or the two instants swapped).

The second is the one security-relevant mutation on this surface: silently dropping the filters
turns a narrowly-scoped bulk replay into "replay everything up to `max_rows`".

`grpc/dead_letters.rs` must also reproduce two behaviors HTTP implements in the adapter, not the
service, or parity breaks silently:

- `limit` absent or `0` maps to `DEFAULT_LIMIT` **in the adapter**. Passing a bare `0` through
  would hit `DeadLetterFilter::capped_limit`'s own floor of `1`, so a default request would
  return a single row.
- `next_cursor` is the last entry's id **only when the page came back full** (`entries.len() ==
  filter.capped_limit()`), else absent.

**3. Integration (Docker-backed).** All suites use the shared policy in `tests/support/docker.rs`
— hand-rolling a skip fails `repo:iam-docker-policy-single-site`.

- **new** `tests/grpc_dead_letters.rs`, mirroring `http_dead_letters.rs`: non-Root denied; list;
  replay-one plus its second-call `NOT_FOUND`; discard; bulk replay rejected without `max_rows`;
  bulk replay happy path. Plus `outbox_rpcs_not_exempt`, modelled on
  `api_keys_grpc.rs::management_rpcs_not_exempt`, asserting all four RPCs require a bearer.
- **new** `tests/grpc_system_retirement.rs`. New rather than folded into `grpc_authz.rs` because
  retirement is **Root**-scoped (`Action::RetireSystemPolicy` at `root_prn()`) while
  `grpc_authz.rs`'s harness seeds `platform_admin`. It re-declares `seed_orphan_chain` /
  `seed_grants` / `seed_system_policy_with_revision`, which are private to
  `authz_system_retirement_pg.rs`; duplicating a private seeder across suites is the established
  posture (`relay_nudge_pg.rs` documents doing exactly this for `dead_letters_pg.rs::seed_parked`).
  Covers all three outcome variants over the wire.
- `tests/grpc_tenancy.rs` gains `CreateUser`: happy path, duplicate email → conflict, invalid
  email → `INVALID_ARGUMENT`.

### D10 — Error mapping needs no new machinery

Every path already exists and is reused unchanged:

| Condition | Mapping |
|---|---|
| `TenancyError` | `convert::status_to_grpc` — Validation→`INVALID_ARGUMENT`, NotFound→`NOT_FOUND`, Conflict→`ALREADY_EXISTS`, Precondition→`FAILED_PRECONDITION`, Forbidden→`PERMISSION_DENIED`, Internal→`INTERNAL` (source never leaked) |
| malformed RFC3339 / cursor | `TenancyError::InvalidPrn`-as-sentinel, the idiom both `http/dead_letters.rs` and `grpc/audit.rs` already use → `INVALID_ARGUMENT` |
| non-Root caller | enforced inside the application service → `Forbidden` → `PERMISSION_DENIED`; the adapter forwards `AuthContext` and does nothing |
| `authz.admin_enabled = false` | `convert::capability_disabled("iam.authz.cedar")` |
| no bearer | `AuthLayer` / `AuthEnforce`; `is_exempt` is a three-entry allowlist, so new RPCs are enforced by construction |

## CI and gate impact

| Gate | Effect |
|---|---|
| `repo:breaking` | **Passes** — purely additive (new RPCs on existing services, one new service). |
| `contracts:fmt` | Reds unless `buf format -w` is run before commit. |
| codegen drift | `contracts:generate` declares no `outputs:` and can serve a stale cache — run `buf generate` directly and commit the regenerated Rust/Py/TS. The embedded `FILE_DESCRIPTOR_SET` shifts, so bindings must be regenerated even for whitespace-only proto edits. |
| `ci/affected-graph/run.sh` | **Unchanged** — no new crate and no new Moon project, so the strict-equality `contracts->proto` expected set stays as-is. (The case ADR-0018 warns about bites on the AC-2 follow-up, not here.) |
| `repo:error-code-single-site` | **No MANIFEST entry needed** — a direct consequence of D4. |
| `repo:observability-drift` | Untouched — no new metrics; the dead-letter counters fire inside the application service. |
| `repo:iam-docker-policy-single-site` | New suites must use `tests/support/docker.rs`; `docker_preflight.rs` remains the canary. |
| `repo:input-liveness` | Untouched — no task `inputs` change. |

`tests/http_authn.rs`'s every-`/v1`-route table needs no update: no HTTP route is added.

## Documentation corrections

These become false when this change lands and must be rewritten, not deleted — each records
*why* the old shape was chosen, and that reasoning is what the correction has to answer:

- **`adapters/http/dead_letters.rs` module doc.** Currently: "This is an operator-only
  break-glass surface and is deliberately HTTP-only: unlike the audit read API it has no gRPC
  mirror, which keeps `contracts/` untouched. That is a scope decision, not an API-boundary
  principle." SMA-501 reverses that scope decision; the paragraph must say so and say why.
- **`adapters/grpc/mod.rs`** module doc and `router()` doc — both enumerate the mounted services.
- **`adapters/http/mod.rs`** header, which enumerates the surfaces.

## Risks

- **Parity is asserted, not structurally guaranteed.** The twin tests (D9.1) cover the two
  projections that carry structured payloads, and the mirrored suites cover behavior, but nothing
  statically forces a future `/v1` route to acquire an RPC. A `repo:*` route↔RPC parity gate was
  considered and rejected as disproportionate for a Low-priority issue: a new `repo:*` task must
  join `ci.yml`'s `T=(…)` array, the CLAUDE.md marker block, `ci_targets.py`, and
  `input-liveness`.
- **Two suites more of Docker-backed IAM tests**, which are the flaky ones under parallel load.
  They inherit `rs/.config/nextest.toml`'s retry budget and container-concurrency cap
  automatically; no `--retries` is added anywhere.
- **The D3 divergence is permanent** unless a later issue revisits it. A gRPC client cannot
  observe the `grants-survive` / `decision-change-unacknowledged` codes.
