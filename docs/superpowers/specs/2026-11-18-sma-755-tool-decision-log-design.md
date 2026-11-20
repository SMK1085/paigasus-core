# SMA-755: the gateway tool decision log

- **Issue:** SMA-755. Parent: SMA-747. Blocked by SMA-752 (done, PR 475).
- **ADR:** ADR-0027 (i), with (c), (d), (f), (g) and (h). ADR-0012 for `AuditMetadata`.
- **Related:** SMA-748 (the Dogwood spike), SMA-753 (session history), SMA-754 (detectors),
  SMA-760 (arguments out of `Debug`), SMA-769 (Responses ingress), SMA-802 (the console export,
  the follow-up of this issue).
- **Status:** revision 2, after the builder check and the adversarial challenge (§10).

## 1. Intent

The gateway decides every tool call with Cedar (SMA-752). Today the decision goes only to a
`tracing` line, a counter and the decision report for the client. Nothing keeps it.

This issue makes a durable record of each decision **as the gateway applied it**. The record has
three uses:

1. **An audit trail.** An operator can see which calls the gateway allowed, observed or blocked,
   why, and which policies decided.
2. **A replay before enforcement.** A tenant writes a new policy in Dogwood. An operator exports
   the real traffic of the tenant as a Dogwood trace. The tenant runs `dogwood replay` on it and
   sees what the policy would do before the tenant sets the mode to `enforce`.
3. **Metrics** for the decisions and for the log itself.

## 2. Goals, non-goals and anti-goals

### Goals

- G1. A `ToolDecisionEvent` proto, in `contracts/proto/paigasus/gateway/v1/tool_decision.proto`,
  that embeds `paigasus.common.v1.AuditMetadata`.
- G2. A `DecisionLog` port in the gateway. Each entry of the decision report, on the request side
  and on the response side, becomes one event. The event holds the **applied** effect.
- G3. A Redis stream is the first stop for each event. A background relay publishes the events to
  NATS JetStream, in a NATS account of its own.
- G4. Metrics count the events that the gateway drops, with the reason, and the Redis losses.
- G5. An operator CLI, `paigasus-gateway decisions export`, writes the events of one org and one
  time range as a Dogwood trace, with a summary of what the replay cannot reproduce.
- G6. One new metric for findings, and the metrics and alerts of the log pipeline.

### Non-goals

- N1. **A tenant self-service export** (an API and a console page). That is SMA-802.
- N2. **Raw arguments in any form.** The trace carries `input: {}` (Sven's decision, option A).
  A replay of a policy that reads arguments is not correct, and the CLI says so.
- N3. **Tool results.** The gateway does not log tool results, so the trace has no `response` or
  `error` lines. A replay of a policy with `formerly … ::response` is not correct.
- N4. **Real session ids and turns.** These are placeholders (`""`, 0) until SMA-753. The event
  carries the placeholder values, and SMA-753 fills them.
- N5. **Finding severities and detector names.** The kernel gives only finding kinds today. The
  detectors come in SMA-754. Field 2 of `Finding` stays free for the severity.
- N6. **A database or a query API** for the log. The JetStream stream is the store.
- N7. **New metrics for verdicts or hold time.** Both exist (§5.9).
- N8. **Helm values.** The chart does not project `[tool_policy]` today (SMA-752 §6.8), so a
  `[decision_log]` projection has no effect yet. It comes with the `[tool_policy]` projection.

### Anti-goals

These make the work a failure, even if the code works:

- A1. A raw argument, a tool result, or any content in the event, in a log line, in a metric
  label, or in the trace. A digest that lets a reader confirm a guessed argument offline also
  counts as a leak.
- A2. A tool call that fails, or changes its verdict, because of the decision log. A Redis or NATS
  outage, or a bug in the log tasks, must never stop the data plane. The only cost on the hot
  path is building the record and the argument digest, and that cost is measured (§5.3).
- A3. The events of one org in the trace of another org.
- A4. A trace that `dogwood replay` cannot parse.
- A5. A record that says "prevented" for a call that the client received, or that the provider
  can still run (SMA-769).
- A6. A decision-log stream that can fill the storage of `IAM_EVENTS`.

## 3. Constraints and facts

These facts come from the code and from the Dogwood source at the pinned revision
`ff158506cfd9c7871385ed3a87d7435505fd24b1` (dogwood-language 1.1.1).

### The gateway

- F1. `decide_now` in `rs/crates/services/paigasus-gateway/src/application/tool_policy/decider.rs`
  calls `paigasus_gateway_core::decide_calls`, which returns one `CallOutcome` per call.
  `CedarDecider` is a unit struct today, built at `runtime.rs` as `Arc::new(CedarDecider)`.
- F2. **The decider's verdict is not the applied outcome.** The interceptor can change it:
  - an observe hold reports a decider `Block` as `Allow` and releases the call
    (`paigasus-gateway-core/src/interceptor.rs:380-391`)
  - hold-limit, malformed and truncated blocks never reach the decider (`interceptor.rs:406-437`,
    `:454-498`)
  - on the body path a forced `Truncated` wins over the decision
    (`adapters/http/interception.rs:111-122`)

  The decision report (`ReportEntry`, v2) is the one place that holds the applied outcome of
  each call.
- F3. On the request side, `plan_request` makes a `RequestEntry` only for a removal, a
  replacement or a `tool_choice` reset, not one per definition (`policy/plan.rs:240-244`,
  `:326-328`). A `RequestEntry` holds `tool`, `action`, `enforced` and `reason` (`report.rs:149-155`).
- F4. `DecisionContext` holds `principal`, `scope`, `request_id` and `policy`. It does not hold the
  API key id, the model or the ingress (`paigasus-gateway-core/src/decider.rs:23-29`).
- F5. The org comes from `org_of(&ctx.scope)` (`request.rs:230`). `org_of` returns `None` for a
  root grant, another service, another resource type, or a PRN that does not parse
  (`domain/mod.rs:108-118`). Then the policy set is `Unavailable`, and `PolicySet::Unavailable`
  holds no `Facts` (`policy/snapshot.rs:67-69`).
- F6. **Tenant mode `off` still enforces floor blocks** (D33, `policy/calls.rs:61-72`, test
  `tenant_off_still_evaluates_the_floor`). Only `[tool_policy] enabled = false` makes the
  `AllowAll` decider (`runtime.rs:129-133`).
- F7. Findings are kinds only (`Findings(BTreeSet<String>)`). The kernel adds `unparsed_arguments`
  inside `decide`, and `Verdict` does not expose the final set.
- F8. No code hashes tool arguments today. `sha2 = "0.11"` is a gateway dependency. Responses
  arguments can be up to `max_held_bytes` = 32 MiB (`config.rs:277`).
- F9. `supervise` stops the gateway when any task on the `JoinSet` ends or panics
  (`runtime.rs:53-76`). It sends shutdown to all tasks at the same time (`runtime.rs:79`). Only
  the limit charges drain after the servers stop (`runtime.rs:92-94`).
- F10. The kernel mints UUIDv7 values from injected bytes (`mint_uuid7`). `Clock` is a port
  (`domain/limits.rs:529`).
- F11. `paigasus-proto` is only a dev-dependency of `paigasus-gateway-core` today, and it pulls in
  `tonic`. The existing pattern maps proto types to domain types in the adapters.
- F12. `ToolCallKind::Shell` and `ToolCallKind::ToolSearch` are the provider-run kinds of
  SMA-769. No helper names this rule yet.

### Redis

- F13. The only allowed constructor is `paigasus_redis::connect`, which is eager and returns a
  `RedisHandle`. `repo:redis-connect-single-site` bans every other way to connect.
- F14. `RedisHandle` has a fixed 500 ms response timeout (SMA-473, pinned by a test at
  `paigasus-redis/src/lib.rs:560-580`). The breaker opens after three failures.
- F15. The workspace `redis` crate does not enable the `streams` feature. No code in `rs/` uses a
  Redis stream today.

### NATS

- F16. `provision.sh` makes one account, `PAIGASUS_IAM`, with 10 GiB of disk, 4 streams and
  32 consumers (`ops/nats/provision.sh:46-54`). `IAM_EVENTS` has `--max-bytes=-1`. When an
  account is full, JetStream refuses new messages for every stream in it.
- F17. The gateway's `gateway-consumer` identity reads `IAM_EVENTS` and has no publish right. It
  must not get one.
- F18. IAM's `NatsEventPublisher` is the model for a JetStream publisher. Its contract is
  at-least-once, so consumers must be idempotent (`nats_publisher.rs:11-17`).
- F19. `ops/nats/subjects.env` is the single source for grants. `repo:nats-permissions` checks
  that `ops/nats/test/accounts.conf.tmpl` agrees with it, and it runs only `-p paigasus-iam`
  tests (`moon.yml:380-387`). `provision.sh` runs once per environment and is not safe to run
  again (`:4-5`, `:118-146`).

### The Cedar request

- F20. The kernel builds the request as follows:
  - principal: `Facts.principal` (`entity_type`, `entity_id`)
  - action: `Pgs::Gateway::Action::"<normalise(name)>"`, with the parent `InvokeTool` (a catalog
    name) or `Unregistered`
  - resource: `Pgs::Iam::Organization::"<org uuid>"`
  - context: `input`, `findings`, `call{declared}`, `request{scope, ingress, model}`,
    `session{id, turn}`, and `input_text` for a text input
- F21. `normalise` keeps every character except case and Unicode form, so `(`, `)`, quotes and
  control characters can be in an action id (`kernel/src/tool_policy/normalise.rs:6-10`).

### The Dogwood trace

- F22. A trace is a text file with one event per line:
  `@<ts> scope(principal: <uid>, resource: <uid>) request_context(<k>: <v>, …) <Ns>::Action::"<name>"::request(<field>: <value>, …)`.
- F23. `<ts>` is an `i64` in **Unix seconds**. Dogwood converts `within 1h` to 3600 and compares
  `0 <= ts(i) - ts(j) <= W`.
- F24. Strictly increasing timestamps are Dogwood's contract, but nothing enforces it. Equal
  timestamps give `delta == 0`, which is inside every window.
- F25. A trace has **no comment syntax**. A non-blank line that does not start with `@` is a parse
  error. `parse_trace` stops at the first bad line.
- F26. Strings use Cedar's `escape_debug` form. A raw newline splits the event.
- F27. **The event head is found with an unescaped `find('(')`** (`log_parse.rs:85-87`). A `(`
  inside the quoted action id cuts the head, and the line does not parse. The `scope(` group uses
  a naive `find(')')`, which is safe for UUID ids only.
- F28. A decision event without `scope(…)` fails closed to DENY. Dogwood builds the Cedar context
  only from `request_context(…)`. A policy that reads `context.input.x` on `input: {}` gets an
  evaluation error for that policy.
- F29. Dogwood replay takes the action entities from the tenant's schema only (`api.rs:1345-1355`).
  An action that the live kernel puts under `Unregistered` is not under it in replay.
- F30. Dogwood 1.2.0 checks each replayed event against the signature of its action
  (`dogwood-language/src/conformance.rs:769`, deny at `api.rs:1766`). The trace writes `input: {}`,
  so a tool with a required input argument replays as DENY ("nonconforming decision event"). No
  `dogwood replay` flag relaxes this, and replay exits 0.

## 4. Approaches

ADR-0027 (i) fixes the shape: a Redis stream first, then NATS JetStream. These were the
alternatives:

| Approach | What it is | Why not |
|---|---|---|
| **Redis stream outbox, then a relay to JetStream (chosen)** | Each event is an `XADD`. A relay in each instance reads with a consumer group and publishes. | — |
| Direct publish from a bounded in-memory channel | No Redis. | ADR-0027 rejects it. A NATS outage would lose the events of every instance. |
| Direct publish, with Redis as a fallback | Two write paths. | The order of events is hard to keep, and it is more code for the same guarantee. |

Where the record is made:

| Option | Why |
|---|---|
| In `decide_now` (revision 1) | Rejected. It records the Cedar verdict, not what the gateway did (F2). Every on-failure block is absent. |
| **At the decision report entry (chosen)** | The report entry is the one place with the applied outcome, on both the stream and the body path. The decider attaches its Cedar details to the outcome, and the report site builds the record. |

For the trace content, Sven chose **option A**: `input: {}` plus a digest of the arguments. For
the export, Sven chose an **operator CLI**. The tenant console page is SMA-802.

## 5. Design

### 5.1 The domain record

The core and the encoder use a domain type, not the wire type (F11). In `paigasus-gateway-core`,
module `decision_log`:

```rust
#[non_exhaustive]
pub struct DecisionRecord {
    pub decision_id: Uuid,              // UUIDv7
    pub decided_at: SystemTime,         // the gateway clock
    pub instance_id: Uuid,
    pub sequence: u64,                  // per (instance, org), starts at 1 (§5.8)
    pub org: Option<Uuid>,              // None for a scope with no org (F5)
    pub principal_prn: String,
    pub principal: Option<CedarUid>,    // to_cedar_uid(principal_prn)
    pub api_key_id: Option<String>,
    pub scope_prn: String,
    pub request_id: Option<String>,
    pub session: SessionInfo,           // placeholder until SMA-753
    pub side: Side,                     // Request | Response
    pub tool: ToolRef,
    pub ingress: String,
    pub model: String,
    pub mode: Option<TenantMode>,       // None when the policy set is unavailable
    pub applied: AppliedEffect,         // Allow | Observe | Block, as the report says
    pub block_code: Option<BlockCode>,  // applied or would-be code
    pub basis: RecordBasis,             // Policy | ErroredForbid | Unavailable | OnFailure
    pub prevention: Prevention,         // §5.4
    pub request_action: Option<RequestAction>, // request side only
    pub cedar: Option<CedarDetail>,     // None when no Cedar evaluation ran
    pub arguments: Option<ArgumentDigest>,     // response side only
}

#[non_exhaustive]
pub struct CedarDetail {
    pub effect: Effect,                 // the kernel verdict, before the interceptor
    pub determining: Vec<PolicyRef>,
    pub errored: Vec<PolicyRef>,
    pub floor_denied: bool,
    pub findings: BTreeSet<String>,     // the final set, from the kernel
    pub declared: Declared,
    pub action_group: ActionGroup,      // Catalog | Unregistered (F20)
    pub decide_duration: Duration,
}
```

The Redis adapter maps `DecisionRecord` to `ToolDecisionEvent`. The CLI maps the proto back to a
`DecisionRecord` and gives it to the encoder. `paigasus-gateway-core` stays free of a runtime
dependency on `paigasus-proto`.

### 5.2 The proto

New file `contracts/proto/paigasus/gateway/v1/tool_decision.proto`, package
`paigasus.gateway.v1`. The fields follow `DecisionRecord`:

```proto
// SPDX-License-Identifier: Apache-2.0
syntax = "proto3";

package paigasus.gateway.v1;

import "google/protobuf/duration.proto";
import "paigasus/common/v1/audit.proto";

// One applied tool decision of the gateway (ADR-0027 (i)). It never holds raw arguments,
// tool results or any other content.
message ToolDecisionEvent {
  string decision_id = 1;                 // UUIDv7, hyphenated, lower case
  paigasus.common.v1.AuditMetadata audit = 2;
  string org_id = 3;                      // the org uuid; empty for a scope with no org
  string principal_prn = 4;
  CedarUid principal = 5;
  string api_key_id = 6;                  // empty for an OIDC caller
  string scope_prn = 7;
  string request_id = 8;
  string session_id = 9;                  // "" until SMA-753
  int64 turn = 10;                        // 0 until SMA-753
  DecisionSide side = 11;
  ToolRef tool = 12;
  string ingress = 13;
  string model = 14;
  DecisionMode mode = 15;                 // UNSPECIFIED when the policy set is unavailable
  DecisionEffect applied = 16;            // what the gateway did (F2)
  string block_code = 17;                 // BlockCode::as_kebab(), applied or would-be
  DecisionBasis basis = 18;
  Prevention prevention = 19;
  RequestAction request_action = 20;      // request side only
  CedarDetail cedar = 21;                 // unset when no Cedar evaluation ran
  ArgumentDigest arguments = 22;          // response side only
  string instance_id = 23;
  uint64 sequence = 24;                   // per (instance_id, org_id), starts at 1
}

message CedarUid { string entity_type = 1; string entity_id = 2; }

message ToolRef {
  string name = 1;                        // sanitize_name(name), at most 128 bytes
  string namespace = 2;                   // at most 256 bytes
  string action_id = 3;                   // normalise(name), at most 256 bytes
  bool truncated = 4;                     // true when any field of ToolRef was cut
  string kind = 5;                        // ToolCallKind::as_str()
  string call_id = 6;                     // at most 256 bytes
}

message CedarDetail {
  DecisionEffect effect = 1;              // the kernel verdict, before the interceptor
  repeated PolicyRef determining = 2;
  repeated PolicyRef errored = 3;
  bool floor_denied = 4;
  repeated Finding findings = 5;
  string declared = 6;                    // yes | no | unknown
  ActionGroup action_group = 7;
  google.protobuf.Duration decide_duration = 8;
}

message PolicyRef { PolicyOrigin origin = 1; string id = 2; }

message Finding {
  string kind = 1;
  // Field 2 is kept for the severity (SMA-754). Do not use it for anything else.
}

message ArgumentDigest {
  bytes hmac_sha256 = 1;                  // HMAC-SHA256 of the raw argument bytes (§5.3)
  string key_id = 2;                      // which digest key made it
}

enum DecisionSide { DECISION_SIDE_UNSPECIFIED = 0; DECISION_SIDE_REQUEST = 1; DECISION_SIDE_RESPONSE = 2; }
enum DecisionMode { DECISION_MODE_UNSPECIFIED = 0; DECISION_MODE_OFF = 1; DECISION_MODE_LOG_ONLY = 2; DECISION_MODE_ENFORCE = 3; }
enum DecisionEffect { DECISION_EFFECT_UNSPECIFIED = 0; DECISION_EFFECT_ALLOW = 1; DECISION_EFFECT_OBSERVE = 2; DECISION_EFFECT_BLOCK = 3; }
enum DecisionBasis { DECISION_BASIS_UNSPECIFIED = 0; DECISION_BASIS_POLICY = 1; DECISION_BASIS_ERRORED_FORBID = 2; DECISION_BASIS_UNAVAILABLE = 3; DECISION_BASIS_ON_FAILURE = 4; }
enum Prevention { PREVENTION_UNSPECIFIED = 0; PREVENTION_NOT_APPLICABLE = 1; PREVENTION_PREVENTED = 2; PREVENTION_PROVIDER_MAY_RUN = 3; }
enum RequestAction { REQUEST_ACTION_UNSPECIFIED = 0; REQUEST_ACTION_REMOVE_DEFINITION = 1; REQUEST_ACTION_REPLACE_RESULT = 2; REQUEST_ACTION_RESET_TOOL_CHOICE = 3; REQUEST_ACTION_REFUSE = 4; }
enum PolicyOrigin { POLICY_ORIGIN_UNSPECIFIED = 0; POLICY_ORIGIN_FLOOR = 1; POLICY_ORIGIN_TENANT = 2; }
enum ActionGroup { ACTION_GROUP_UNSPECIFIED = 0; ACTION_GROUP_CATALOG = 1; ACTION_GROUP_UNREGISTERED = 2; }
```

Rules:

- `audit.created_at` is `decided_at`. `audit.creator` is the principal as an `Actor`.
  `modified_at` and `modifier` stay unset, because an event never changes.
- The builder matches the `RequestAction` values to the real `RequestEntry.action` values and to
  the refusal path. Add a value only when a real action needs it.
- `Finding` has **no `reserved` line**. `contracts/buf.yaml` uses the `FILE` category, so a
  reservation would be permanent, and SMA-754 could never use field 2 (challenge finding).
- `buf format -w`, then `moon run contracts:generate`. Add the `include!` line in
  `rs/crates/libs/paigasus-proto/src/lib.rs` if the generated file needs one. The Python and TS
  bindings regenerate too. `:breaking` passes, because the file is new.
- `Auditable` is not automatic. Add a `message_attribute` line for `ToolDecisionEvent` in
  `contracts/buf.gen.yaml`, or `auditable_derive_drift.rs` fails.
- No new error codes. No error of this issue reaches a client. The CLI errors are local exit
  codes (§5.11).

### 5.3 The argument digest

- **What it is for.** An operator who holds a copy of the arguments (for example from the
  client's own log, during an incident) can confirm that a logged call had exactly these
  arguments. Two calls with the same digest had the same arguments. The digest is not in the
  trace.
- **Why a keyed digest.** A plain SHA-256 of a short argument such as `{"approve":true}` or a
  one-time code is easy to guess and confirm offline (A1). So the digest is
  **HMAC-SHA256** with a deployment key, `[decision_log] digest_key` (a `SecretString`, at least
  32 bytes), and a `digest_key_id` that goes in each event. The length of the arguments is not
  logged.
- **Key rotation.** A new key gets a new `digest_key_id`. Old events keep the old id. To check
  an old event, the operator needs the old key. The runbook says how long to keep old keys
  (30 days, the stream retention).
- **Cost on the hot path (A2).** The digest is computed only when the log is enabled
  (`DecisionLog::wants_digest()`), once per call, in `decide_now`. Its time is part of
  `gateway_tool_policy_overhead_seconds`. A benchmark at 1 KiB, 1 MiB and 32 MiB is part of the
  plan. If the 32 MiB case is above 50 ms on the CI runner, the plan moves the digest of
  arguments above 1 MiB to `spawn_blocking` before the release of the hold.
- Nothing else from the arguments leaves `decide_now`.

### 5.4 Building the record

The record is made **at the decision report entry** (§4). One site makes the records for the
stream path and the body path. The builder finds where `ReportEntry` values are final in
`adapters/http/interception.rs` and the interceptor's outputs, and records there.

- **The decider attaches its Cedar details.** `CallOutcome` (`#[non_exhaustive]`) gets a
  `CedarDetail`, with the final finding set and `declared` from the kernel (no second parse, no
  second copy of the arguments). `Verdict` exposes the final finding set for this. The decider
  measures `decide_duration` per call around each kernel `decide` in `decide_calls`.
- **The interceptor carries the detail to the report site.** The decisions that the decider
  returns carry their `CedarDetail` and their `ArgumentDigest`. A hold-limit, malformed,
  truncated or decider-error block has `cedar = None` and `basis = OnFailure`.
- **`applied`** is the effect in the report entry: an observed hold is `Allow` (or `Observe`
  with a would-be code), not `Block` (F2).
- **`prevention`:**
  - `PREVENTED` when `applied = Block` and the call kind is not provider-run
  - `PROVIDER_MAY_RUN` when `applied = Block` and `ToolCallKind::provider_may_run()` is true
    (`Shell`, `ToolSearch`), and for every Responses item that the report marks as
    unenforceable (for example `web_search_call`, `mcp_call`)
  - `NOT_APPLICABLE` for `Allow` and `Observe`

  Add `ToolCallKind::provider_may_run(self) -> bool` in gateway-core, with a test for each kind
  and no `_` arm in its `match`, so that a new kind must be classified.
- **`DecisionContext` gets three fields:** `api_key_id`, `ingress` and `model`, set in
  `pipeline.rs`. They do not depend on the snapshot, so an `Unavailable` record has them too.
  `DecisionContext` is `#[non_exhaustive]`, so this is a minor change.
- **The decision id** is a UUIDv7 from `paigasus_kernel::mint_uuid7`, with bytes from an injected
  entropy source (`Arc<dyn Entropy>`). The clock is the injected `Clock` port.
- **The request side** makes one record per `RequestEntry` (removals, replacements, `tool_choice`
  resets) and one record for a request that the tool policy refuses. It has `side = Request`,
  `request_action`, `cedar = None` and `arguments = None`. A later refusal by the limits (429)
  does not remove the records: they say what the tool policy did.
- **Mode:** tenant mode `off` **writes records**, because floor blocks are real blocks (F6). Only
  `[tool_policy] enabled = false` writes none.
- **Caps (A1, memory).** `ToolRef.name` is `sanitize_name` (128 bytes). `namespace`,
  `action_id`, `call_id`, `model` and `ingress` are cut at 256 bytes on a UTF-8 boundary. A cut
  sets `ToolRef.truncated`. A test with a 1 MiB call name proves the cap.

#### Where each field comes from

| Field | Response, policy available | Response, policy unavailable or on-failure | Request side |
|---|---|---|---|
| `org` | `Facts.org` | `org_of(scope)`, may be `None` | `org_of(scope)`, may be `None` |
| `principal` | `Facts.principal` | `to_cedar_uid(principal_prn)` | `to_cedar_uid(principal_prn)` |
| `api_key_id`, `ingress`, `model` | `DecisionContext` | `DecisionContext` | `DecisionContext` |
| `session` | `Facts.session` | `SessionInfo::placeholder()` | `Facts.session` or the placeholder |
| `mode` | `snapshot.mode` | `None` | `snapshot.mode` or `None` |
| `applied`, `block_code` | the report entry | the report entry | the `RequestEntry` |
| `basis` | `CallOutcome.basis` | `Unavailable` or `OnFailure` | from `RequestEntry.reason` |
| `cedar` | the decider's `CedarDetail` | `None` | `None` |
| `arguments` | the digest | the digest when the decider ran, else `None` | `None` |
| `tool.kind`, `tool.call_id` | the `ToolCall` | the `ToolCall` | the definition kind, no call id |

### 5.5 The `DecisionLog` port

In `rs/crates/services/paigasus-gateway/src/domain/decision_log.rs`:

```rust
/// The durable record of applied tool decisions (ADR-0027 (i)).
pub trait DecisionLog: Send + Sync {
    /// Whether the caller must compute an argument digest.
    fn wants_digest(&self) -> bool;
    /// Record one decision. It must not block, must not fail, and must not panic.
    /// A failure is counted in a metric, never returned.
    fn record(&self, record: DecisionRecord);
}
```

- One `Arc<dyn DecisionLog>` goes to the decider, the request-side policy and the report site.
  The composition root builds it once.
- **`TracingDecisionLog`** is the default when `[decision_log]` is absent or `enabled = false`.
  `wants_digest()` is false. It writes the `tracing` line that exists today in `decide_now`. Move
  that line into it, so that there is one place for it.
- **`RedisDecisionLog`** is the adapter when `enabled = true`. It also writes the same `tracing`
  line, so the log lines do not change when the log is turned on.
- No `{:?}` of a record or an event in a log line. `action_id` is not sanitised.
- A test adapter, `MemoryDecisionLog`, keeps the records in a `Vec` behind a mutex. It is behind
  the `test-support` feature.

### 5.6 The Redis stream

- `record` maps the record to the proto, encodes it with prost, and sends it into a bounded
  `tokio::sync::mpsc` channel (default capacity 4096) with `try_send`.
  - A full channel counts as `dropped{reason="queue_full"}`.
  - A closed channel (after shutdown) counts as `dropped{reason="closed"}`.
- **The Redis connection does not block the boot (A2).** A task calls `paigasus_redis::connect`
  in a loop with a backoff from 1 s to 30 s until it succeeds. Until then, the writer counts each
  event as `dropped{reason="redis_unavailable"}`. This uses the existing eager constructor, so the
  single-site gate does not change.
- One **writer task** reads the channel and runs
  `XADD <prefix>:decisions MAXLEN ~ <2 × max_len> * e <bytes>` through the `RedisHandle`. It sends
  up to 64 `XADD`s in one pipeline.
  - A failed write counts each event of the batch as `dropped{reason="write_error"}`. This is an
    uncertain write outcome, not an exact count of lost events: Redis can accept an `XADD` whose
    reply times out, and the relay then publishes that event.
  - An open breaker counts each event as `dropped{reason="breaker_open"}`.
  - The writer does not retry. The event is already late, and a retry would hold the channel.
- **The backlog cap.** `max_len` defaults to 100 000. The relay deletes each entry after its
  publish, so `XLEN` is the backlog. The writer samples `XLEN` once per second. While the sample
  is at or above `max_len`, the writer drops each new event as `dropped{reason="backlog_full"}`.
  The old events stay. The `MAXLEN ~ 2 × max_len` is only a guard for Redis memory.
- `<prefix>` is `[decision_log] key_prefix`, default `paigasus:gateway`, so that two environments
  can share a Redis instance.
- Add the `streams` feature to the workspace `redis` crate in `rs/Cargo.toml`, with a comment. It
  adds no crate, so `deny.toml` does not change. The builder confirms with
  `cargo tree -e features -p redis` before and after.
- The writer and the relay share one `RedisHandle`. Neither sends a blocking command (§5.7). The
  connection is not shared with the limits store, because the two features can be on
  separately. The breaker metrics use the role label `decision_log`.
- Redis 7.0 or later (for `XAUTOCLAIM` with deleted ids). The runbook requires `noeviction`, and
  it recommends a Redis instance apart from the limits store and `appendonly yes`.
- **Shutdown.** The writer drains **after the HTTP server stops**, as `drain_charges` does, with a
  2 s budget. Events that are still in the channel then count as `dropped{reason="shutdown"}`.
  `supervise` gets a second drain step for this.
- **The tasks never stop the gateway (A2).** The writer and the relay loops catch panics
  (`catch_unwind` around one iteration, as the policy cache does), log them, count
  `gateway_decision_log_task_restarts_total{task}`, and go on. They return only after shutdown.

### 5.7 The relay to JetStream

- One **relay task** per gateway instance.
- At start it runs `XGROUP CREATE <prefix>:decisions relay 0 MKSTREAM`, and it ignores
  `BUSYGROUP`. The start id is `0`, so that no entry written before the group exists is skipped.
  The consumer name is the instance id.
- The loop:
  1. `XREADGROUP GROUP relay <instance> COUNT 256 STREAMS <prefix>:decisions >`, **with no
     `BLOCK`** (F14). A `BLOCK` of 500 ms or more fails on the client side and opens the breaker,
     and it would stall the writer on the shared connection. When the read returns no entries,
     the relay sleeps for 200 ms.
  2. For each entry: check that `org_id` is empty or a canonical lower-case UUID. Publish the
     **original stream bytes** (no decode and re-encode, so a field from a newer gateway is kept
     during a rolling upgrade) to `gateway.decision.<org_id>`, or to `gateway.decision._none`
     when `org_id` is empty. The header is `Nats-Msg-Id: <decision_id>`. Up to 32 publishes are
     in flight at a time.
  3. After the PubAcks: `XACK` and `XDEL` of the published ids in one `MULTI`.
  4. Every 30 s: `XAUTOCLAIM <prefix>:decisions relay <instance> 60000 0 COUNT 256`. The ids that
     `XAUTOCLAIM` reports as deleted count as `dropped{reason="redis_deleted"}`.
  5. Every 10 min: `XINFO CONSUMERS`, then `XGROUP DELCONSUMER` for each consumer with 0 pending
     entries and an idle time above 1 h.
- **Publish errors are classified.**
  - A transport error or a timeout: the relay stops reading, keeps the entries in Redis, and
    tries again with a backoff from 1 s to 30 s.
    No stream for the subject is retried too, because async-nats reports a JetStream that is not
    ready the same way (R9). The relay makes the stream again before the next publish.
  - A PubAck error with a resource or availability code is transient too. async-nats 0.53
    reports each PubAck error code as `PublishErrorKind::Other` (except 10070 and 10071), so the
    relay reads the server code from the error source. The transient codes are 10002 (account
    resources exceeded), 10023 (insufficient resources), 10028 and 10047 (no memory or disk),
    10077 (the store failed), 10120 (no limits), and 10008, 10009, 10039, 10063, 10076 and 10118
    (JetStream not available). Newer servers add 10158 (a message with the same id is in
    process), 10167 (too many requests) and 10194 (the stream is offline); they are transient
    too. A status 503 with no code is transient too. So a full disk does not delete valid events:
    on the 2.10.14 publish path a full disk gives 10023 (server limit) or 10002 (account limit).
    The meanings come from the nats-server v2.10.14 error table and the async-nats 0.53
    `ErrorCode` table. An `Other` with no JetStream code (a closed client, a closed ack
    semaphore, a PubAck that does not parse) stays permanent.
  - A permanent error (the payload is above `max_payload`, an invalid subject, a permission
    error, a PubAck code that rejects the message, for example 10054 for a message above
    `max-msg-size`): the relay acks and deletes that entry, counts
    `dropped{reason="publish_rejected"}`, logs the error class, and goes on. One poison entry
    must not stop the relay.
  - An entry whose `org_id` fails the check is a permanent error too. Anybody with Redis write
    access can forge an event, and the check keeps a forged org out of the subject. The runbook
    states this trust boundary: Redis write access is equal to write access to the decision log.
- `NOGROUP` from `XREADGROUP`, `XACK` or `XAUTOCLAIM` starts the loss handling of §5.8.
- JetStream drops a duplicate inside the dupe window. Later duplicates (after a Redis restore,
  for example) are removed by the export (§5.11). The contract is at-least-once, as for IAM (F18).
- **Shutdown.** The relay finishes the batch that it has, with a 2 s budget, and stops. The
  entries that are not published stay in Redis for the next instance.

### 5.8 Loss accounting

There are three kinds of loss:

1. **Drops that the gateway sees**, counted in `gateway_decision_log_dropped_total{reason}`.
   The reasons are: `queue_full`, `closed`, `redis_unavailable`, `write_error`, `breaker_open`,
   `backlog_full`, `publish_rejected`, `redis_deleted`, `shutdown`. Each count is exact as a count of
   the events that the gateway gave up. For `write_error` it is not an exact count of lost events:
   the batch was in flight, and Redis can have stored it (a reply timeout). `shutdown` has two parts.
   Events that were still in the queue at the shutdown deadline never went to Redis, so they are lost.
   A batch that was in flight at the deadline has an unknown result, and Redis can have stored it.
2. **A Redis loss that the relay detects.** When Redis loses the stream, it loses the consumer
   group too, and the next command returns `NOGROUP`. The relay runs `XGROUP CREATE … 0
   MKSTREAM`. Only one relay gets `OK`. That relay:
   - counts 1 in `gateway_decision_log_redis_resets_total` (a best-effort count of the incidents,
     not an exact one; see the known limits below)
   - adds its last `XLEN` sample to `gateway_decision_log_lost_estimate_total`, and logs it

   The estimate is **not a bound**: it is the last sample. Entries that the writer added after
   the sample make it undercount. Entries that the relays published after the sample make it
   overcount. With a 15 s sample, the error is about 15 s of writes or publishes. The
   dashboard says "estimate". If no relay has a sample, it adds 0 and logs that the loss is
   unknown. The sampler keeps a sample only while the stream has the consumer group. After a loss
   that no relay has found yet (for example while each relay holds a batch during a NATS outage),
   the stream has no group, so its length does not replace the sample from before the loss.
3. **Exact gaps in the export.** Each record has a `sequence` per `(instance_id, org)`, which
   starts at 1 when the instance starts. The CLI reports, per instance, the sequence numbers that
   are missing inside the exported range. This is exact for the events of the exported org, also
   after a restore of Redis from an older RDB snapshot (which keeps an old group, so `NOGROUP`
   does not detect it).

   The gateway keeps one `u64` counter per org in memory, for the life of the instance, with no
   eviction. The cost is about 24 bytes per org that the instance saw. A new instance run has a
   new `instance_id`, so a sequence never starts again inside one `(instance_id, org)` pair.
   Records with an empty org use the pair `(instance_id, "")`.
   - The counter increases when `record` accepts the record, before the `try_send`. So a
     `dropped` event also makes a gap. Inside the exported range, the gap count is the number of
     sequence numbers that are missing between observed events of one `(instance_id, org)` pair.
   - The export cannot see a gap at the start or at the end of a range. It compares only the
     observed sequence numbers. If sequence 1 is lost and 2 and 3 are exported, it reports no gap.
     An event after the last exported one is outside the range. So the gap count is not the true
     total of missing events. The summary says "gaps inside the range only".

Known limits, which the runbook states:

- A Redis failover that keeps the group but loses the last writes is not seen live. The export
  shows the gaps.
- The live metrics are per process. A dashboard and the alerts use `sum()` over the instances.
- The reset count is best-effort. Three cases are not counted. (a) A new relay that makes the group
  at its start does not count a reset, because it cannot tell a first start from a start after a
  loss. (b) A relay that holds a batch settles it with `XACK`, which returns 0 and no `NOGROUP` when
  the stream or the group is missing. If a new relay makes the group before the next read, no relay
  counts the reset. (c) If Redis makes the group but the reply to `XGROUP CREATE` is lost, the retry
  gets `BUSYGROUP` and does not count. A durable record of the recovery would close these cases. It
  is not built. The export shows the gaps.
- If only the consumer group is removed (`XGROUP DESTROY`), `NOGROUP` counts a reset, but the stream
  still holds its events. The relay reads them again from ID 0, so the lost estimate overcounts.
- A cross-account import of `gateway.decision.>` into an account with a stream that captures it gives
  two PubAck responders. The foreign stream answers 10060. If that answer comes first, the relay retries
  without end, although `GATEWAY_DECISIONS` stored the event. Nothing in `ops/nats` exports the subject
  today.

### 5.9 Metrics and alerts

What exists and stays:

| Name | Use |
|---|---|
| `gateway_tool_policy_decisions_total{side, mode, effect, basis}` | decisions per verdict |
| `gateway_tool_interception_hold_seconds` | how long tool calls are held |

New, in `paigasus_observability::names` and in `names::ALL`, each with a registry test, a
`describe_*!` line in `main.rs`, a prime at zero for **every** label value, and a row in
`RUNBOOK-observability.md` §2.3:

| Name | Type | Labels | Meaning |
|---|---|---|---|
| `gateway_tool_policy_findings_total` | counter | `side`, `kind` | findings per kind. `kind` is a fixed set of constants; an unknown kind becomes `other`. |
| `gateway_decision_log_recorded_total` | counter | — | records that `record` accepted |
| `gateway_decision_log_written_total` | counter | — | events in the Redis stream |
| `gateway_decision_log_published_total` | counter | — | events with a PubAck |
| `gateway_decision_log_dropped_total` | counter | `reason` (§5.8) | events that the gateway dropped |
| `gateway_decision_log_redis_resets_total` | counter | — | Redis losses that the relay detected |
| `gateway_decision_log_lost_estimate_total` | counter | — | an estimate of the events in those losses (it can undercount or overcount) |
| `gateway_decision_log_task_restarts_total` | counter | `task` = `writer`, `relay` | caught panics |
| `gateway_decision_log_publish_seconds` | histogram | — | publish to PubAck |
| `gateway_decision_log_backlog` | gauge | — | `XLEN`, sampled every 15 s |

"Findings per detector" in the issue becomes "per kind" until SMA-754 adds a `detector` label.

Alerts in `ops/observability/prometheus/rules/gateway.rules.yml`, each with a `promtool` test that
primes the series at zero:

- **Drops:** `sum(increase(gateway_decision_log_dropped_total{reason!="shutdown"}[15m])) > 0`.
  `shutdown` is left out, because every deploy can have it.
- **Redis reset:** `sum(increase(gateway_decision_log_redis_resets_total[15m])) > 0`.
- **Relay stalled:** `written` increases while `published` does not, for 10 minutes. Or the
  backlog is above 50 % of `max_len`.
- **Breaker:** a decision-log breaker alert with `{role="decision_log"}`. Add `{role="limits"}`
  to the existing `GatewayLimitsRedisBreakerOpen`, because its text says that the limits fail
  open, and a decision-log breaker must not fire it.
- **Task restarts:** `sum(increase(gateway_decision_log_task_restarts_total[15m])) > 0`.

### 5.10 NATS: the account, the stream and the identities

**A NATS account of its own (A6).** The stream goes in a new account, `PAIGASUS_GATEWAY`, with its
own JetStream limits. A full decision stream then cannot stop `IAM_EVENTS` (F16).

| Account setting | Value |
|---|---|
| `--js-mem-storage` | 0 |
| `--js-disk-storage` | an operator setting, default 20 GiB |
| `--js-streams` | 2 |
| `--js-consumer` | 16 |

**The stream `GATEWAY_DECISIONS`:**

| Setting | Value |
|---|---|
| Subjects | `gateway.decision.>` |
| Storage | file |
| Retention | limits |
| `max-age` | **30 days** (Sven's decision) |
| `max-bytes` | **required**, default 90 % of the account disk |
| `discard` | old |
| `dupe-window` | 2 minutes |
| `max-msg-size` | 64 KiB |

`provision.sh` and the relay use **one** stream config. The relay calls `get_or_create_stream`
with the same values, so a stream that the relay makes is never unbounded. On drift, the relay
logs a warning and does not change the stream.

The relay does not know `DECISIONS_ACCOUNT_DISK_BYTES`. So before `get_or_create_stream` it reads
the account storage limit with `$JS.API.INFO` (`query_account`). When the limit is above 0, the
relay uses `min(19 327 352 832, limit × 9 / 10)` as `max-bytes`; with no limit it uses the
default. It never goes above the per-stream limit of the account (`storage_max_stream_bytes`,
untiered or in the tier `R1`), because the broker refuses a stream above that limit too. The drift check uses the same value. A failed query gives the default and a warning; it
does not stop the relay. Without this, a smaller account disk makes the broker refuse the create,
and the relay stalls (CodeRabbit review).

The runbook gets a sizing formula: `disk = peak decisions per second × average event bytes ×
86 400 × 30 / 0.9`, because the stream gets 90 % of the account disk. At 600 bytes per event, 20 GiB
gives a stream of 19 327 352 832 bytes, which holds about 12.4 decisions per second, sustained, for
30 days. An operator with more traffic raises the account disk, or the stream keeps fewer days
(`discard old`).

**Two new identities** in the new account, in `ops/nats/subjects.env`:

- `gateway-decision-publisher`, for the relay. Publish: `gateway.decision.>`, `$JS.API.INFO`
  (read-only, for the account limit),
  `$JS.API.STREAM.INFO.GATEWAY_DECISIONS`, `$JS.API.STREAM.CREATE.GATEWAY_DECISIONS`. Subscribe:
  its inbox prefix `_INBOX_GW_DEC.>`, for the PubAck. No subscribe on `gateway.decision.>`.
- `gateway-decision-reader`, for the CLI. Publish: the JetStream API subjects that an
  `async-nats` 0.52 ordered consumer on `GATEWAY_DECISIONS` needs (the builder confirms the exact
  list). Subscribe: its inbox prefix `_INBOX_GW_RD.>`. No publish to `gateway.decision.>`. The
  runbook says that the reader creds must never be deployed with the gateway.

The `gateway-consumer` identity does not change.

**Files that change:**

- `ops/nats/subjects.env`: the account, `DECISIONS_STREAM_NAME`, the two identities, the two
  inbox prefixes
- `ops/nats/test/accounts.conf.tmpl`: the second account and its two users
- `ops/nats/check-subjects.sh`: two `check_identity` calls, account-aware if needed
- **a new `ops/nats/provision-decisions.sh`**, safe to run again, for an environment that already
  exists. `provision.sh` runs once (F19), so it calls the new script for a new environment.
- `ops/nats/permissions.md`: a new section with the reasons
- `docs/ops/RUNBOOK-nats.md`: the account, the stream, the sizing, the identities, and the order
  of a rollout: provision, deploy the creds, then set `enabled = true`
- the permission tests: positive and negative cases for both identities, and a test that a
  `PAIGASUS_GATEWAY` user cannot publish to `iam.>`. If they live in a gateway test target,
  extend the `repo:nats-permissions` command and inputs in `moon.yml`.

### 5.11 The trace export

**The encoder** is pure, in `paigasus-gateway-core`, `decision_log::trace`:

```rust
pub enum Encoded { Line(TraceLine), Skipped(SkipReason) }
pub enum SkipReason { RequestSide, NoOrg, NoPrincipal, Truncated, UnsafeActionId }
pub fn encode(record: &DecisionRecord) -> Encoded;
impl Display for TraceLine { /* one line, no trailing newline */ }
```

One response-side record gives one line:

```
@<ts> scope(principal: <P>, resource: Pgs::Iam::Organization::"<org>") request_context(input: {}, findings: [<kinds>], call: { declared: "<declared>" }, request: { scope: "<scope>", ingress: "<ingress>", model: "<model>" }, session: { id: "<session>", turn: <turn> }) Pgs::Gateway::Action::"<action_id>"::request(input: {}, callerPrincipal: <P>, callerResource: Pgs::Iam::Organization::"<org>", requestId: "<request_id>", sessionId: "<session>")
```

- `<P>` is `principal.entity_type::"principal.entity_id"`. `<ts>` is `decided_at` in **Unix
  seconds**, rounded down (F23).
- `request_context` uses the kernel context keys (F20) except `input` and `input_text`. When
  `cedar` is `None`, `findings` is `[]` and `declared` is `"unknown"`.
- Every string goes through one escape function in the `escape_debug` form (F26).
- **Skips.** `encode` returns `Skipped` for:
  - a request-side record (Dogwood decides only `request` events)
  - a record with no org or no principal
  - a record with `tool.truncated`, because its action id is not exact
  - an action id that Dogwood's head parser cannot carry (F27): any `(`, `)`, `"` or control
    character, at minimum. The builder makes the set exact from `log_parse.rs`.
- **Order.** Lines are ordered by `(ts, decided_at, instance_id, sequence)`. Equal seconds stay
  equal; the exporter does not shift events, because a shift changes the window arithmetic (F24).
- **Clock skew between instances (accepted).** A skew of a few seconds can put two calls in the
  wrong order. Dogwood accepts this without an error. The runbook says that the gateway hosts
  need NTP.

**The CLI:**

```
paigasus-gateway decisions export --org <uuid> --since <rfc3339> --until <rfc3339>
    [--principal <prn>] [--api-key <id>] [--max-lag <duration>] [--max-events <n>]
    [--out <file>] [--report <file>] [--nats-url-file <file>] [--creds <file>] [--ca <file>]
```

- The dispatch is hand-written, as `healthcheck` is. The workspace does not get `clap`.
- `--org` must pass `parse_org_uuid`, and the CLI converts it to lower case. `--until` is
  exclusive. The NATS URL comes from a file or from `PAIGASUS_NATS_URL`, never from argv, because
  a URL can hold credentials.
- **Reading.** An ordered, ephemeral pull consumer on `GATEWAY_DECISIONS` with the filter subject
  `gateway.decision.<org>`. The start time is `since - max_lag` (default 1 h), because the store
  time can be later than `decided_at` (backlog, outage, `XAUTOCLAIM`). It keeps the records with
  `decided_at` in `[since, until)`. The read stops at the first limit it reaches: the end of the
  stream, the `until + max_lag` storage-time cutoff, or the raw-event cap. The summary warns when
  the cutoff stops the read (R5, R13).
- **Duplicates.** It removes duplicates by `decision_id`.
- **The org check.** It checks `org_id` a second time after the decode. An event of a different
  org stops the export with exit code 3 (A3).
- **Memory.** The CLI holds the records of the range in memory to sort them. `--max-events`
  (default 5 000 000) stops the export with exit code 4 and asks for a smaller range.
- **Filters.** `--principal` and `--api-key` select one history, for a policy with
  `@scope("principal")` or `@scope("key")`.
- **The trace file** holds only `@` lines (F25). The summary and the warnings go to stderr, and
  to `--report <file>`:

  ```
  exported 1234 decisions for org <uuid> from <since> to <until> (until exclusive)
  skipped 12: 9 request side, 2 unsafe action id, 1 truncated
  removed 3 duplicates; 5 decisions arrived later than 10 min
  47 lines share a second with another line
  sequence gaps: instance 0190…: 2 missing (41, 42)
  WARNING: the trace holds no tool arguments and no tool results.
    A policy that reads context.input or context.input_text gets an evaluation error on replay.
    A policy that uses a `response` or `error` event in a temporal block never matches.
  WARNING: Dogwood 1.2 checks each replayed call against its action schema. The trace has no tool
    arguments, so a tool with a required argument replays as DENY ("nonconforming decision event").
    Replay against a copy of your schema in which every context.input attribute is optional (`name?: Type`).
  WARNING: 8 lines are tools that the live gateway put under Unregistered. Replay takes the
    action groups from your schema, so `action in Unregistered` can decide differently.
  WARNING: the trace is one history for the org. A policy with @scope("session") or
    @scope("key") replays against the wrong history unless you filter with --principal or
    --api-key. Session ids are empty until SMA-753.
  ```

- The export includes the decisions of every mode. The replay computes its own verdict.
- Exit codes: 0 = done; 1 = bad arguments; 2 = NATS or stream error; 3 = an event of a wrong
  org; 4 = more than `--max-events`.

**Dogwood 1.2.0 (added after approval).** Dogwood 1.2.0 checks each replayed event against the
signature of its action (F30). The trace has no tool arguments, so a tool with a required input
argument replays as DENY ("nonconforming decision event"). The owner chose to ship with an exact
warning in the export summary and a runbook step. Workaround: replay against a copy of the tenant
schema in which every attribute under `context.input` is optional (`name?: Type`). SMA-815 plans
the generator for that schema.

**Proof that Dogwood accepts the trace (A4).** Dogwood cannot join the `rs/` workspace (SMA-748),
but `rs/tools/paigasus-policy-compiler` already pins it in its own workspace.

- The fixture path is `rs/tools/paigasus-policy-compiler/tests/fixtures/decision-trace.log`, with
  its policy set next to it. A test in gateway-core generates the expected content from known
  records. A drift test in gateway-core fails when the encoder output and the fixture differ.
- A test in the compiler workspace parses the fixture with `parse_trace` and replays it against a
  `count_within` policy. It asserts **an exact verdict sequence** (for example ALLOW, ALLOW,
  DENY at line 3), so the test is not vacuous.
- A property test in the compiler workspace sends many generated action ids through the escape
  function and the skip rule, then through `parse_trace`. The ids include `(`, `)`, `::"`, `\r`,
  NUL, bidi characters and a leading combining mark. Every id that the encoder does not skip
  must parse and must give back the same action id.
- The fixture path is an input of both Moon test tasks. The compiler task uses
  `mergeInputs: 'replace'`, so the path must be added there explicitly.

### 5.12 Configuration

A new optional section in `config.rs` and in `gateway.toml.example`:

```toml
[decision_log]
enabled = false
redis_url = "redis://…"           # SecretString; required when enabled
key_prefix = "paigasus:gateway"
stream_max_len = 100000
queue_capacity = 4096
digest_key = "…"                  # SecretString, at least 32 bytes; required when enabled
digest_key_id = "2026-11"

[decision_log.nats]
url = "nats://…"                  # SecretString
creds_file = "/etc/paigasus/nats/gateway-decision-publisher.creds"   # the `[tool_policy.nats]` key name
ca_file = "/etc/paigasus/nats/ca.pem"   # optional
```

- `deny_unknown_fields`, as the other sections.
- `enabled = true` with `[tool_policy]` off is a validation error. The gateway must not connect to
  Redis and NATS for nothing.
- Neither a Redis outage nor a NATS outage at boot stops the boot (§5.6, `retry_on_initial_connect`).

### 5.13 Hygiene

- The record, the event, the trace and the logs never hold `ToolCall.arguments`, `ToolInput`, a
  tool definition schema, or a tool result.
- **The canary test.** Build records from a call with the arguments
  `{"secret":"s3cr3t-canary"}`, on every path (allowed, blocked, observed hold, truncated,
  unavailable). Assert that the canary is not in the encoded proto bytes, in the trace line, in
  the captured `tracing` output, or in any metric label. Assert that the digest differs from
  `sha256(arguments)`, so a plain hash cannot come back by mistake.

## 6. Testing

- **Unit, gateway-core:** the builder for each row of the table in §5.4; `prevention` per kind;
  `provider_may_run` per kind; the caps; the escape function; the encoder against golden lines
  and each skip reason; the canary test.
- **Unit, gateway:** the report site records once per report entry, with `MemoryDecisionLog`, on
  the stream path and the body path. One test each for an observe hold (applied `Allow`, Cedar
  `Block`), a truncation under `block`, a hold-limit block (`OnFailure`, no Cedar detail), tenant
  mode `off` with a floor block, and an org-less scope (`gateway.decision._none`).
- **The production wiring.** A composition-root test builds the runtime with `[decision_log]
  enabled = true` and checks that the decider, the request side and the report site share one
  log. One end-to-end test through the HTTP router checks an event on `gateway.decision.<org>`.
  Mutations to run and see fail: remove the `record` call at the report site; wire
  `TracingDecisionLog` in the runtime; give the request side a different log instance.
- **Integration with Docker (testcontainers Redis and NATS):**
  - N records give N messages with the right subjects and the original bytes
  - NATS down: the events wait in Redis and publish after NATS comes back
  - Redis down at boot: the gateway boots, counts `redis_unavailable`, and writes after Redis
    comes back
  - a poison entry above `max-msg-size` gives `publish_rejected`, and the next entries publish
  - `FLUSHALL` with two relays gives `redis_resets_total` = 1, once
  - a backlog at `max_len` gives `backlog_full` for each new event
  - two relays and one dead instance: `XAUTOCLAIM` publishes its pending entries once
  - a record after shutdown starts gives `closed`, and a long stream that decides during the
    drain is written
- **The digest benchmark** at 1 KiB, 1 MiB and 32 MiB (§5.3).
- **CLI:** the lines, the org filter and the wrong-org exit, late events inside `max_lag`,
  duplicates, sequence gaps, `--max-events`, and the summary.
- **Dogwood:** the fixture replay and the property test (§5.11).
- **NATS permissions:** positive and negative cases for both identities and the account
  isolation.
- **Gates before the push:** the full `ci-targets` graph from the root `CLAUDE.md`, plus
  `repo:nats-permissions` and `repo:actionlint` with the right bash on this Mac.

## 7. Decisions

| # | Decision | Who |
|---|---|---|
| D1 | The trace carries `input: {}` (option A). | Sven |
| D2 | The export is an operator CLI. The console page is SMA-802. | Sven |
| D3 | `GATEWAY_DECISIONS` keeps events for 30 days. | Sven |
| D4 | A Redis stream outbox and a relay in each instance, with a consumer group. | ADR-0027 (i) |
| D5 | The record holds the **applied** effect, made at the decision report entry. The Cedar verdict is a detail. | my call, after challenge B3 |
| D6 | The payload is the proto in binary. The relay publishes the original bytes. | my call |
| D7 | The subject is `gateway.decision.<org>`; a scope with no org goes to `gateway.decision._none`, which the export never reads. | my call, after challenge B4 |
| D8 | The request side is logged, one record per `RequestEntry` and per refusal. | my call: ADR-0027 (i) says every decision |
| D9 | A domain `DecisionRecord` in gateway-core; the proto mapping is in the adapters. | my call, after challenge (hexagonal) |
| D10 | The CLI is a subcommand of `paigasus-gateway`, with no `clap`. | my call |
| D11 | The argument digest is HMAC-SHA256 with a deployment key, and no length is logged. | my call, after challenge (A1) |
| D12 | A NATS account of its own, `PAIGASUS_GATEWAY`, with a required `max-bytes`. | my call, after challenge B1 |
| D13 | Loss: exact drop counts (`write_error` and an in-flight `shutdown` batch are uncertain write outcomes), a best-effort Redis-reset count, an estimate (not a bound), and exact gaps between the observed events per `(instance, org)` sequence in the export. | my call, after challenge |
| D14 | Neither Redis nor NATS can stop the boot or the data plane. | my call, after challenge (A2) |
| D15 | Tenant mode `off` writes records, because floor blocks are real. | my call, after challenge |
| D16 | The encoder skips records that Dogwood cannot parse, and the CLI reports them. | my call, after challenge B2 |
| D17 | No Helm change in this issue. | my call (the chart does not project `[tool_policy]`) |

## 8. Left to the builder

- The exact JetStream API subjects of the reader identity (§5.10).
- The exact report site in `interception.rs` and the interceptor outputs (§5.4).
- The exact set of characters that the encoder must skip, from `log_parse.rs` (§5.11).
- Where the permission tests for the new identities live (§5.10).
- The exact `RequestAction` values (§5.2).

## 9. Follow-ups

- SMA-815: write a relaxed replay schema for the decision trace.
- SMA-802: the tenant console export.
- SMA-753: fill the real session id and turn.
- SMA-754: the finding severity (field 2 of `Finding`) and a `detector` label.
- SMA-760: arguments out of `Debug`. This issue does not depend on it.
- The Helm projection of `[tool_policy]` and `[decision_log]`.

## 10. Review log

### Builder check (revision 1)

All eight questions were technical. Answers are in the spec: no `BLOCK` on `XREADGROUP` (§5.7),
`NOGROUP` during a batch (§5.8), per-process metrics with `sum()` (§5.8), clock skew (§5.11), dead
consumers (§5.7), `decide_duration` and `prevention` (§5.1, §5.4), the `streams` feature (§5.6),
and org-less scopes (§5.4, D7).

### Adversarial challenge (revision 1, verdict NEEDS REWORK)

Folded in:

| Finding | Change |
|---|---|
| B1 shared account storage | D12, §5.10 |
| B2 `(` in an action id breaks the trace | D16, §5.11 skips and property test |
| B3 Cedar verdict instead of the applied outcome | D5, §4, §5.4 |
| B4 "the org is always known" is false | D7, the field table in §5.4 |
| Unsalted hash is a guessing oracle | D11, §5.3 |
| Digest latency | §5.3 (only when enabled, measured, benchmark) |
| No caps on strings | §5.4 caps, `ToolRef.truncated` |
| A poison message stops the relay | §5.7 error classes |
| The log tasks can stop the gateway | D14, §5.6 |
| Shutdown loses events without a count | §5.6 drain after the server, `closed` |
| The export selects by store time, no dedup | §5.11 `max_lag`, dedup |
| The loss metric is not a lower bound | D13, §5.8 |
| Mode `off` is ambiguous | D15, §5.4 |
| Builder inputs missing on two paths; F2 wrong | the field table in §5.4, `RequestAction`, F3 corrected |
| The CLI warning misses two divergences | `ActionGroup`, `--principal`, `--api-key`, warnings (§5.11) |
| `reserved` blocks SMA-754 | §5.2, no `reserved` line |
| The wire crate in the core | D9, §5.1 |
| No test of the production wiring | §6 |
| The fixture crosses workspaces | §5.11 fixture path and Moon inputs |
| Alerts | §5.9 |
| No upgrade path for `provision.sh` | §5.10 `provision-decisions.sh` |
| All MINOR findings | applied in place (reason list, original bytes, `MULTI`, in-flight publishes, canonical `--org`, order, `Prevention` enum, `buf.gen.yaml`, Helm out of scope, injection, no second parse, per-call timing, tracing behaviour, gate command, memory bound, runbook, org check, validation) |

Questions answered in the spec: what reads the digest (§5.3); provider-run Responses items are
logged as `PROVIDER_MAY_RUN` (§5.4); refused requests (§5.4); the account (D12); `--until` is
exclusive (§5.11). Out of scope: whether a blocked call counts in the SMA-753 history.

Not folded in: none of the findings was rejected. One question stays open for Sven: the expected
peak decisions per second, which sets the default account disk (§5.10).
