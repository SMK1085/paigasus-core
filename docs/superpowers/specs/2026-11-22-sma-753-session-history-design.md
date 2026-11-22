# SMA-753: gateway session history and history conditions

- **Issue:** [SMA-753](https://linear.app/smaschek/issue/SMA-753) (parent SMA-747)
- **ADR:** ADR-0027, decisions (e), (f) and (g), as amended 2026-11-09 and 2026-11-16
- **Builds on:** SMA-749 (tool call assembler), SMA-751 (IAM tool policy sets), SMA-752 (InvokeTool
  decisions), SMA-755 (decision log)
- **Status:** approved by Sven, revision 2, 2026-11-22. Revision 2 adds the results of the builder check and
  of the adversarial challenge (§ 12).

## 1. Intent

The gateway decides each tool call with Cedar (SMA-752). Some rules need the history of earlier
calls, not one request. Examples: "no outbound email after a secret was read in this session", or
"the sum of transfers in 24 hours must be at most 5,000".

IAM already compiles such rules. Each Dogwood temporal block becomes a `HistoryCondition` with a
stable slot name (SMA-751). Today the gateway refuses every set with a condition, and IAM refuses
to save one. This change gives the gateway a trusted history and an evaluator, so these rules
work.

The history must be trusted. The client sends the conversation, so the client can edit it. The
gateway therefore keeps its own record. Only the gateway clock sets times. Each event is bound to
the authenticated principal. A client must not escape a rule when it starts a new conversation,
so a policy can also read the history of the principal or of the API key.

## 2. Goals, non-goals, anti-goals

**Goals**

- G1. A `SessionStore` port with a Redis adapter (on `paigasus-redis`) and an in-memory adapter.
- G2. Two event kinds, `call` and `result`, with gateway timestamps.
- G3. Session linking by forwarded tool call ids and by decision references in notices.
- G4. Three history scopes: session, principal and key.
- G5. An evaluator for the three accepted shapes (`formerly`, `count_within`, `sum_within`) that
  fills the Cedar slots and matches Dogwood's semantics.
- G6. Concurrent requests see each other's calls, across replicas.
- G7. The findings `mixed_sessions`, `forged_history` and `history_truncated`.
- G8. The gateway accepts sets with conditions. IAM accepts saves with conditions, behind a setting.
- G9. Parity with an extended decision corpus. A real-Redis cap test. A p99 test.

**Non-goals**

- N1. `count_distinct_within`. The compiler rejects it (`translate.rs:289-292`).
- N2. A window longer than 24 hours. ADR-0027 (g), amended 2026-11-16, removed the 7-day maximum,
  because Dogwood limits every window to 24 hours. The issue text still names 7 days. This spec
  follows the ADR.
- N3. Running counters per slot (§ 5, approach 3).
- N4. Redis Cluster. ADR-0025 does not support it, and the scripts use several keys.
- N5. Tool result content. It is never stored.
- N6. History for the MCP gateway (SMA-756).

**Anti-goals** (the change fails if any of these is true, even when the tests pass)

- A1. A client can lower a count or a sum when it edits, drops, splits or floods its conversation.
- A2. Two concurrent requests, on one replica or on two, can both pass a cap that only one may pass.
- A3. A missing slot, unreadable history, a malformed condition or a mistyped value allows a call
  that a correct history would deny.
- A4. The gateway stores argument values that no condition reads, a plain hash of arguments, or any
  result content.
- A5. The gateway evaluator and Dogwood disagree on a corpus case that the corpus does not mark as
  a deliberate difference.
- A6. A Redis outage blocks tool calls of a tenant that has no history condition.

## 3. Constraints

- C1. The evaluation rules in `contracts/proto/paigasus/iam/v1/iam.proto` (comments on
  `CompiledToolPolicySet` and `HistoryCondition`) are the contract. This change edits two of those
  rules (D6 and D10). In short:
  - The window is closed: `[now - window_ms, now]`.
  - The decided event is recorded before evaluation. It counts toward `count_within` and
    `sum_within`. A denied call is still recorded, so it counts too.
  - `formerly` sees the decided event.
  - Tool matching uses the catalog `name_key`.
  - `sum_within`: a missing value or a negative value adds 0. The sum saturates at the i64 limits.
  - Scope KEY with no API key uses the principal.
  - A missing slot, an unknown shape or an unknown enum value fails closed.
- C2. `ToolCallDecider` keeps its signature (SMA-752 D19). `DecisionContext` and `DecidedCall`
  can get new fields.
- C3. The decider spawns nothing (SMA-749 D25). The decide future can be dropped on a hold
  timeout or a client disconnect. A decide timeout follows `[tool_interception] on_failure`
  (SMA-752 D26).
- C4. The kernel holds `decide()`. A kernel change is a lockstep release behind the `tool-policy`
  feature (SMA-752 D2, D27). ADR-0005 puts behaviour that other languages may reuse (Helikon) in
  the kernel.
- C5. Redis clients come from `paigasus_redis::connect` only (`repo:redis-connect-single-site`).
  Tests use `redis-cli` through `container.exec`. A blackholed Redis costs about 2.1 s per command
  before the breaker opens (`paigasus-redis` `lib.rs:26-29`).
- C6. Every gateway Docker test binary is listed in the gateway override of
  `rs/.config/nextest.toml`, in the `docker-containers` group.
- C7. `/v1/responses` stays stateless: it refuses `previous_response_id` (SMA-752). So each
  request carries the full conversation.
- C8. SMA-755 bans a plain hash of arguments (`application/decision_log.rs:47`). It uses a keyed
  digest.

## 4. Who decided what

- Sven decided D1 (option A) and D2 (approach 1) on 2026-11-22.
- On the same day, Sven approved a summary that covered the first forms of D3, D4, D7, D11 and
  D15. Revision 2 changes some of them after the challenge (§ 12).
- Every other decision is a proposal in this spec. The decisions that change the IAM contract or an
  issue rule are marked **[Sven, 2026-11-22]**. Sven approved them, and the whole spec, at Gate 1.

## 5. Approaches

The three approaches share one atomic Lua step that appends the new events and reads the window.

1. **Redis stores, Rust evaluates (chosen by Sven).** The script returns the events of the window.
   One evaluator serves both adapters. Good: one set of semantics, exact closed windows, no
   migration on a policy change. Cost: each hold moves the window. D9 bounds the cost.
2. **Lua evaluates.** The script computes the slots. Not chosen: a second evaluator, in Lua, with
   predicates up to 32 levels deep, must agree with the Rust one and with Dogwood.
3. **Running counters per slot.** Not chosen: buckets do not give an exact closed window, and an
   IAM publish resets the counters of a changed policy. It stays a fallback if approach 1 fails
   the p99 test. That fallback needs a new decision.

## 6. Decisions

### D1. A blocked call's notice carries a decision reference (Sven, option A)

```text
[paigasus] tool call "send" was blocked (policy). ref=0193f6c2-8a4e-7b10-9c2d-5e8f1a2b3c4d
```

- This changes the closed notice text of SMA-749 D21. Only the `ref=` suffix is new.
- The decider makes the decision id (a UUIDv7) for each call, before the reserve (D8). The id is
  also the event id of the call. It goes in a new field of `DecidedCall`. The decision recorder
  uses this id and does not make its own. So the notice, the event and the decision record have
  one id.
- All six notice sites (three formats, each with a body path and a stream path) get the suffix.
- A block that the interceptor makes without the decider (`decider-error`, `hold-limit`,
  `truncated`, `malformed`) has no decision id and gets no `ref=`. Its session link is lost for
  that turn (R1).
- In the next request, the gateway reads `ref=` tokens only from assistant messages, and only in
  this exact notice form. It reads every text form: Chat `content` as a string or as an array of
  parts, Anthropic `content` as a string or as `text` blocks, and Responses `output_text`.
- A reference counts only when the link table has it for the same principal in the same org. An
  unknown reference is ignored. It is not `forged_history`.

### D2. History store: Redis stores, Rust evaluates (Sven, approach 1)

### D3. Where the evaluator lives

The evaluator is pure and synchronous, so it goes in `paigasus-kernel` `tool_policy`, next to
`decide()` (C4, ADR-0027 (e)). Helikon can then reuse it. The event types and the condition types
that it reads go there too. `paigasus-gateway-core` maps the IAM proto into those types.

### D4. Session linking

**Which ids the gateway reads.** Only the newest part of the conversation:

- the ids of the tool calls in the last assistant message,
- the call ids of the tool results after that message,
- the `ref=` ids in the last assistant message (D1).

Each list is capped at 128 entries. Entries past the cap are ignored and counted in a metric. Older
ids are not read: they were linked when they were new. A result with no call id (a legacy
`role: "function"` message) is ignored. A Responses `local_shell_call_output` uses its `item_id`
as the call id.

The gateway looks up each id in the link table of **this principal in this org**.

- No id matches: a new session starts. The session id is a new UUIDv7.
- All matches give one session: the request joins that session.
- The matches give more than one session: the request joins the session with the most recent
  activity, and the request gets the finding `mixed_sessions`.
- A result whose call id has no forwarded-call link gets the finding `forged_history`. Only the
  results after the last assistant message are checked, so an old result does not fire the finding
  again on every later request. The finding also fires for a call that the gateway blocked, and for
  a link that expired (D11). The gateway cannot tell those cases apart from a forgery.
- `SessionInfo.turn` is the number of requests in the session so far, starting at 1.

Findings never block alone. Only a Cedar policy that reads `context.findings` can block.

**When the gateway writes.** The lookup is read-only, and it runs in `RequestToolPolicy::apply`, so
the session is in `Facts.session` and in a new `DecisionContext.session` field. The writes (the
session update and the result events) run after the limits admission (`pipeline.rs:146`) and after
the request-side refusal checks. So a refused or rate-limited request writes nothing.

**Key names.** Call ids and refs are client strings. In key names they are a SHA-256 hex digest of
the id, so their length and characters are fixed.

**The link table.**

- `c:<call id>`, written when the gateway forwards the call (D8 settle): the session, the tool
  `name_key`, and the event id of the call.
- `d:<decision id>`, written in the reserve (D8): the session.

### D5. Events and scopes

Each event goes into all three histories of the request: the session, the principal and the key.
So a tenant who adds a `@scope("principal")` rule later has a full history from the start. A caller
with no API key uses the principal as the key (C1). For that caller, the key history *is* the
principal history. The event is written once.

**History layout.** A history is split by tool. Each `(org, scope, scope id, tool)` has its own
sorted set of events. So the events of one tool cannot push out the events of another tool (D11).
Each history also has a small index of the tools it holds, for predicates that need every tool.

**`call` event** (a tool call the model proposed):

| Field | Content |
|---|---|
| `id` | the decision id (D1) |
| `ts_ms` | gateway time in milliseconds |
| `kind` | `request` (Dogwood's name for a call) |
| `tool` | the catalog `name_key`, or the normalised name for a tool that is not in the catalog |
| `verdict` | `pending`, then `allow` (forwarded) or `deny` (blocked) |
| `fields` | the argument values at the referenced paths (D7) |
| `bad` | true when the arguments were unparsed (D6) |

The event body is stored once. The three histories refer to it by id.

**`result` event** (a tool result the client sent back):

| Field | Content |
|---|---|
| `id`, `ts_ms` | a new UUIDv7, the gateway time |
| `kind` | `response` |
| `tool` | the `name_key` from the `c:` link of its call id |
| `fields` | the `fields` of the call event that the link names |

- A result event is written only for a call id that has a `c:` link. An unlinked result gives only
  the `forged_history` finding. So a client cannot add events with forged results (§ 12, B1).
- A result event is written once for each call id. A repeated result writes nothing.
- The tool name in the result message is never used. The client could change it.
- A result that the request-side plan replaces with a notice still makes a result event. The
  client did send it.
- Dogwood patterns such as `read_file::response{ input.path: "/secret" }` read the call's
  arguments. The result event carries the call's fields, so these patterns work.

**When the gateway writes events.** Only when the tool policy is on, the org is known, and the
tenant mode is `log_only` or `enforce`, or the floor has a condition. A request with no org writes
nothing.

### D6. Evaluation

**Inputs.** For each request, the gateway takes the conditions of the floor and of the tenant set.
`W_max` is the largest `window_ms` among them.

**Steps for one hold** (the calls that the decider gets together):

1. The decider makes a decision id for each call.
2. One reserve (D8) appends a pending event for each call and returns the window
   `[now - W_max, +inf)` of every needed history.
3. For call *i*, the evaluator uses the returned events plus the new events of calls 1 to *i*, and
   not the new events of calls *i+1* to *N*. This is Dogwood's order: each event is recorded before
   it is decided.
4. The evaluator computes each slot. For `count_within` and `sum_within`, it filters by the
   condition window, then by the predicate, then compares with `op` and `threshold`. For
   `formerly`, a slot is true when any event in the window matches.
5. An event with `ts_ms > now` counts as inside every window. It comes from a replica with a faster
   clock. This keeps A2 true under clock skew (§ 12, B2). No corpus case has such an event, so
   parity does not change.
6. `decide()` gets the slots of call *i* as a new argument. The slots differ per call, so they are
   not in `Facts`. The kernel puts each slot as a Bool attribute at the top of the Cedar context
   (`context.<slot>`), as the corpus shows.

**Typed matching.**

- A `FieldComparison` is false when the stored value and the literal have different types. `lt`,
  `le`, `ge` and `gt` on strings or bools are false. Decimal compares with decimal.
- `any_catalog_tool` is checked against the catalog of the current snapshot, at evaluation time.

**[Sven, 2026-11-22] Mistyped values on the decided call.** The kernel maps arguments with a duplicate
key, or that are not one JSON object, to an empty `input` (`input.rs:262-288`). A string, a decimal
or a big integer is not a Long. Today such a value would add 0 to a sum. So
`{"amount":1,"amount":4000}` or `{"amount":"4000"}` escapes a sum cap. The proposed rule:

- If the decided call has `bad = true`, or a value at a `sum_within` `field_path` that is present
  but is not a Long, every `sum_within` slot of that call is missing (fail closed).
- On earlier events, such a value adds 0. Those calls were decided under the same rule.

This changes the `HistoryCondition` comment in `iam.proto` and adds a `paigasus_differs` corpus
case.

**Fail-closed rules for the conditions.** At load, a set is malformed when any of these is true. A
malformed tenant set makes the tenant part unavailable, and the tenant's fail mode decides. A
malformed floor follows the global fail mode, as today.

- `shape`, `node` or `LiteralValue` is not set.
- A `PredicatePair` has a missing side.
- An enum value is unknown, or is `UNSPECIFIED` (scope, event kind, op).
- A `ToolMatch` has both fields or neither.
- A `path` or a `field_path` is empty.
- `window_ms` is 0, or more than `max_window_ms`.
- A slot name is not a Cedar identifier, or is a reserved context name (`input`, `input_text`,
  `findings`, `call`, `request`, `session`). Otherwise `Context::from_pairs(...).expect(...)`
  panics (`decide.rs:103`).
- Two conditions in one set have the same slot name.
- A tenant slot name is the same as a floor slot name. Then the tenant set is malformed, never the
  floor.

Each form has a test that asserts a fail-closed result.

**Optimistic set.** `Reads::attr` already treats an unknown context attribute as a read of all
parts (`optimistic.rs:73`). So a forbid that reads a slot is never in the optimistic set. No new
`ContextAttr` variant is needed. A test proves it.

### D7. Stored argument fields

At policy load, the gateway walks every `FieldComparison.path` and every `SumWithinShape.field_path`
of the floor and the tenant set.

- A path under a positive `ToolMatch { name_key }` applies to that tool.
- A path under `any_catalog_tool`, or under `negation` or `any_of`, applies to every tool.
- When the tenant part is unavailable, the gateway uses the last known paths of that org.

A `call` event stores only the values at those paths, for its tool:

- A Long is stored as a number. `sum_within` and the ordered comparisons need the value.
- A string, a bool or a decimal is stored as a keyed digest (the SMA-755 key, C8). The builder must
  check in `translate.rs:434-456` which ops the compiler allows on these types. If an ordered op on
  a decimal is allowed, decimals are stored as numbers.
- No hash of the full arguments is stored (C8, A4).

A policy change can add a path. Events written before the change do not have that field, so it
counts as "no value" (R3).

### D8. Reserve, then settle

ADR-0027 (g) says "as in ADR-0025". ADR-0025 does not reserve: it checks first and charges after,
and it rejected a reservation. So this is a new primitive. It shares the patterns of the SMA-677
adapter: one Lua script, an injected `now`, versioned keys and the `Harness` contract suite.

**Reserve** is one script call for each hold, on the response side, inside the decide future. It
runs atomically:

1. For each history that it writes, remove events older than `now - retention`.
2. Append the pending event of each call to the session, principal and key histories, in the
   tool's sorted set. Write the `d:` link of each call.
3. If a `(history, tool)` set holds more than 10,000 events, remove the oldest. Return a
   `truncated` flag for each `(history, tool)` that lost an event inside `[now - W_max, +inf)`.
4. Return the events of the needed histories in `[now - W_max, +inf)` (D9).
5. Set the expiry of every key it wrote (D11).

Two concurrent holds therefore see each other. The second reserve always returns the first
reserve's events.

**Settle** runs at the report site, in `adapters/http/interception.rs`, where the gateway knows
what it did. The decider does not know that: an observed hold or a timeout can override it
(`interceptor.rs:194-200, 387-410`). Settle sets the verdict of each event, and writes the `c:`
link of each forwarded call. It is idempotent, so one retry is safe. It runs on a `TaskTracker`,
as `charge()` does in SMA-677, so it is not on the latency path.

- A lost settle leaves the event `pending`. A pending event still counts, so a lost settle never
  lowers a count or a sum. A pending event has no `c:` link, so a later result for it gets
  `forged_history`.
- A complete call that the interceptor forwards with no decision (a decider error or a decide
  timeout under `on_failure = allow`) gets an append-only event and a `c:` link at the report site,
  so it still counts.
- A truncated or oversized hold under `on_failure = allow` (`Verdict::PassThrough`,
  `interceptor.rs:495-498`) has no complete call and makes no report entry. It gets no event, so it
  does not count (R10).

**Time.** The caller passes `now` in milliseconds from the gateway `Clock`. The scripts never call
Redis `TIME`. The contract suite uses a fixed clock for window edges, and two store clients with
different clocks for skew.

**Concurrency proof (done-when 1).** Three concurrent `send` calls of 2,000 each, against
`sum_within(24h) <= 5,000`, with an empty history, on two replicas. The three reserves run in some
order. The first sees 2,000, the second 4,000, the third 6,000. The third is denied. With Dogwood's
rule, the denied 2,000 also counts, so a fourth call of 100 is also denied (6,100). The test proves
both, also with a clock skew of 50 ms between the two store clients.

### D9. Read size

The application computes the read request from the conditions:

- the scopes that some condition uses,
- the tools that the conditions name in a positive `ToolMatch`, or "every tool" when a predicate
  needs it (`any_catalog_tool`, `negation`, `any_of`, or `event_kind` alone).

The script reads only those `(history, tool)` sets. This is key selection, not a second evaluator.
When the request has no condition, the reserve reads nothing and only appends.

### D10. Failures

**[Sven, 2026-11-22] Retention shorter than the window.** The contract says that the set "fails closed"
when the retention is shorter than `max_window_ms`. IAM writes `max_window_ms` into every set, also
into a set with no condition (`sets.rs:83`). Proposed rule: the check applies only to a set with at
least one condition, and only per condition (`window_ms <= retention`). When it fails, only the
slots of those conditions are missing. The other policies of the set still work. This affects fewer
calls than a failed set, and it makes a rollback safe (§ 10). The `iam.proto` comment changes to
say this.

**Store errors.** The rule depends on what the request needs:

| The request has conditions | Part | Outcome of a store error |
|---|---|---|
| no | any | Count the error and continue. No effect on the decision (A6). |
| yes, tenant conditions | tenant | The tenant part is unavailable. The tenant's `OnUnavailable` decides (`open`: observe, `closed`: block). |
| yes, floor conditions | floor | Only the floor slots are missing. A floor policy that reads a missing slot errors, and an errored forbid denies (SMA-752 D26). |

**[Sven, 2026-11-22]** A floor history rule is a security rule for every tenant. It therefore fails closed in every mode.

**Deadline.** The history has one deadline per request, `[tool_history] deadline` (default 1 s). It
must be shorter than `decide_timeout`. The gateway checks this at boot, as SMA-752 D31 does. A
store call past the deadline is a store error. After the first store error in a request, the
remaining store calls of that request are skipped.

**`on_failure = allow`.** It is supported with history conditions. A decide timeout then allows the
call (C3), and the pass-through call still gets an event (D8). The runbook states that
`on_failure = allow` weakens A2 and A3 for timeouts.

**Boot.** With `backend = "redis"`, a Redis that is down at boot fails the boot, as in SMA-677 D17.

### D11. Limits, expiry and Redis requirements

- The longest window is 24 hours.
- The retention is 24 hours plus a margin of 1 hour. Events older than the retention are removed on
  reserve.
- At most 10,000 events for each `(history, tool)`.
- **[Sven, 2026-11-22] Truncation fails closed.** When the reserve removes an event inside the window of
  a condition that reads that tool, the slot of that condition is missing, and the request gets the
  finding `history_truncated`. The split by tool (D5) means that only a flood of the same tool can
  do this, and then a cap on that tool should deny anyway.
- Each key expires after the retention, measured from its last write. A session is made only when
  its first event or link is written.
- These values are constants in the domain, as `MAX_TOKENS_PER_PERIOD` is in `domain/limits.rs`.
- **Redis requirements.** A dedicated instance or logical database with
  `maxmemory-policy noeviction`. An evicted key would reset a count with no error. A full instance
  refuses writes, which is a store error (D10). At boot, the gateway reads `maxmemory-policy`. It
  fails the boot on a policy that can evict a history key. If `CONFIG GET` is not allowed, it logs a
  warning. The limits Redis of SMA-677 meets this rule and can be shared. The IAM Redis uses
  `volatile-*` and must not be shared. The runbook gets a sizing formula.

### D12. Configuration

```toml
[tool_history]
backend = "redis"           # "memory" | "redis"
redis_url = "..."           # or GATEWAY_TOOL_HISTORY__REDIS_URL; required for "redis"
deadline = "1s"             # must be shorter than [tool_interception] decide_timeout
```

- With no `[tool_history]` table, there is no history. A condition then has a missing slot (D10).
- The memory backend is for development and for one replica. The chart does not deploy the
  gateway (`charts/paigasus/values.yaml`), so no chart can check the replica count. The runbook
  states the rule, and the gateway logs a warning at boot with `backend = "memory"`.
- The Redis backend uses its own `BreakerMetrics` role, `tool_history`.

**IAM setting.** The constant `HISTORY_CONDITIONS_ENFORCED_BY_GATEWAY` becomes the IAM config
setting `tool_policies.history_conditions`, default `false`. The chart exposes it as an IAM value.
The chart does not deploy the gateway, so it cannot tie the setting to `[tool_history]`. The
operator turns on history in the gateway first and in IAM second. This is the same pattern as the
DPoP setting (SMA-700, `values.yaml`). The runbook states the order.

### D13. Metrics

Following the `gateway_*` pattern, with the runbook and `repo:observability-drift` updated:

- `gateway_tool_history_store_errors_total{op, kind}`, with `op` in `lookup`, `write`, `reserve`,
  `settle`.
- `gateway_tool_history_ids_capped_total{list}`.
- `gateway_tool_history_events_trimmed_total{reason}`, with `reason` in `retention`, `cap`.
- The new findings in `finding_label` and its primed list: `mixed_sessions`, `forged_history`,
  `history_truncated`.

The builder can add metrics. These names are the minimum.

### D14. Code changes

| Area | Change |
|---|---|
| `paigasus-kernel` `tool_policy` | event and condition types; the evaluator; the slots argument of `decide()`; finding constants `MIXED_SESSIONS`, `FORGED_HISTORY`, `HISTORY_TRUNCATED` |
| `paigasus-gateway-core` | proto mapping and load validation of conditions; the path walk (D7); `ref=` in `notice_line` and the six notice sites; `ref=` parsing in `turn.rs`; `DecisionContext.session`; the decision id in `DecidedCall` |
| `paigasus-gateway` domain | `SessionStore` port (lookup, write, reserve, settle, append); `PolicyText`, `TenantPolicies` and `PolicySet::Available` carry the conditions |
| `paigasus-gateway` adapters | `adapters/history/{memory,redis}.rs`; wiring in `runtime.rs`; settle in `interception.rs` |
| `paigasus-gateway` application | linking in `RequestToolPolicy::apply` and the writes after admission; reserve and evaluation in `CedarDecider`; the recorder takes the decision id |
| `policy_source/iam.rs` and `file.rs` | remove the R5 refusal; parse the conditions. The retention check is in the application layer, so both sources get it. |
| decision log | fill `session_id` and `turn` |
| IAM | the setting `tool_policies.history_conditions` (D12); tests for both values; `docs/ops/RUNBOOK-tool-policies.md` |
| `charts/` | the IAM value `tool_policies.history_conditions` |
| `rs/.config/nextest.toml` | the new Docker test binaries; the cap test with `retries = 0` |
| policy compiler corpus | new cases (§ 9) |

### D15. Delivery as two stacked PRs

- **PR 1, no policy effect.** The `SessionStore` port, both adapters, the contract suite, session
  linking with its writes, an append-only reserve, settle and the links, the `ref=` notice, the
  findings, `session_id` and `turn` in the decision log, the Redis requirement check. So PR 1 has
  working links and real findings. A store error in PR 1 is counted, and the request continues.
  The R5 refusal and the IAM setting stay as they are.
- **PR 2, the policy effect.** The kernel evaluator and slots, the window read in the reserve, the
  load validation, the R5 removal, the IAM setting, the corpus cases, the parity test, the cap test
  and the p99 test.

Both PRs use a `feature/sma-753-…` branch, because the pre-push hook allows only `feature/*`.
Linear closes the issue when PR 1 merges. After that merge, SMA-753 is moved back to In Progress by
hand.

## 7. Redis layout (proposal, the builder can change it)

Prefix: `paigasus:gateway:history:v1:`.

- `h:<org>:<scope>:<scope id>:<tool>` — a sorted set. The score is `ts_ms`. The member is the event
  id.
- `t:<org>:<scope>:<scope id>` — a set of the tools that the history holds.
- `e:<org>:<event id>` — the encoded event body, stored once.
- `l:<org>:<principal>:c:<sha256(call id)>` and `l:<org>:<principal>:d:<decision id>` — links.
- `s:<org>:<session>` — a hash with `last_ms` and `turn`.

The event encoding has a version byte, and a reader ignores unknown fields, so a rolling upgrade
does not fail. A builder target: about 60 bytes for an event with one stored field.

## 8. Rollback

- **Gateway rollback to a build before PR 2**, or removal of `[tool_history]`. A build before PR 2
  refuses a set with a condition (`iam.rs:157-166`), so a `closed` tenant loses its tool calls.
  Before such a rollback, the operator sets `tool_policies.history_conditions = false` and tells the
  tenants that have conditions. After PR 2, the removal of `[tool_history]` only makes the history
  slots missing (D10).
- **IAM setting back to `false`.** IAM then refuses every save with a condition. Existing compiled
  sets stay in force. A later catalog write that recompiles such a set is refused
  (`sets.rs:21-24`). The runbook states this.
- The chart deploys IAM only, so nothing enforces the order between IAM and the gateway. A
  history-off gateway can meet a history-on IAM when an operator gets the order wrong. A tenant set
  with a condition then has missing slots (D10), so only its history policies fail closed. The
  runbook states the order for turning history on (gateway first) and off (IAM first).

## 9. Tests

- **Contract suite.** One `Harness` trait for both adapters, as in
  `tests/support/limits_contract.rs`. It covers append and read, the window edges, retention trim,
  the 10,000 trim per tool and the `truncated` flag, the per-tool split, settle, a lost settle,
  expiry, links, principal isolation of links, and two clients with clock skew. Redis runs on
  `6.2-alpine` and `7.4-alpine`.
- **Evaluator unit tests.** Every shape, every op, every predicate node, the window edges, a future
  event, saturation, negative, missing and mistyped values, `bad = true`, `any_catalog_tool`, the
  per-call order inside a hold, and each malformed form of D6.
- **Linking tests.** No match, one match, `mixed_sessions`, `forged_history` (new results only), a
  `ref=` match, a `ref=` from another principal, a deleted `ref=`, the 128 cap, each text form of
  D1, each of the three formats, and no writes for a refused or rate-limited request.
- **Fail-path tests.** A store error and a deadline during a hold, for each row of the D10 table,
  and for `open` and `closed`. A decide timeout during a hold. A pass-through call under
  `on_failure = allow` still counts. A store error on the request side. Modes `off`, `log_only` and
  `enforce`. A forged-result flood adds no events.
- **Parity test.** Extend `rs/tools/paigasus-policy-compiler/tests/corpus.rs`, which runs Dogwood's
  `Authorizer` in the test, and regenerate the files with `PAIGASUS_WRITE_FIXTURES=1`. Add cases for
  every predicate node and every op, `response` events, `any_catalog_tool`, missing and mistyped
  fields, and the rule of D6. `to_dogwood` needs decimal support. The gateway test replays every
  corpus file through the kernel evaluator and `decide()`, and requires the corpus `decisions`,
  except where `paigasus_differs` marks a difference. No spike trace files and no `dogwood replay`
  CLI are in the repo, so this corpus is the parity source.
- **Cap test (done-when 1).** Real Redis, two store clients, D8. It runs with `retries = 0` in its
  own nextest override, because a retry can hide a race.
- **p99 test (done-when 2).** In `tests/perf.rs` (ignored, release, so CI does not run it; the PR
  records the measured result). A 1,000-event history in real Redis, a hold of three calls, one
  `sum_within` and one `formerly` condition. Assert `p99 < 15 ms` of
  `GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS`. A second, non-gating case measures the design maximum:
  10,000 events in each needed `(history, tool)`, every tool read.
- **Mutation checks.** Delete the reserve's read. Delete the append. Make the window half-open.
  Restore the `now` upper bound. Change a store error to allow. Each mutation must make a test fail.

## 10. Left to the builder

- The event encoding and the exact Redis layout (§ 7), within the size target.
- How `RequestToolPolicy`, `CedarDecider` and the interception adapter get the `SessionStore`
  (constructor injection).
- Metric labels past the minimum of D13.

## 11. Risks and known limits

- R1. A client can delete `ref=` from a notice, and an interceptor block has no `ref=`. Then a fully
  blocked turn starts a new session. Principal-scope and key-scope rules still apply.
- R2. `forged_history` also fires for an expired link and for a blocked call. A tenant who forbids
  on it can block a client that resumes a conversation after more than 25 hours.
- R3. A new field path starts with no values for older events (D7).
- R4. Each hold now needs one reserve round trip, plus one settle off the latency path, when history
  is on.
- R5. A denied call counts toward a sum (C1). A user may not expect that a refused transfer uses
  budget. The runbook must say so.
- R6. `mixed_sessions` joins only the most recent session (the issue's rule). A client that mixes
  ids from two sessions can lower a session-scope count. Principal-scope and key-scope rules are not
  affected, and a policy can forbid on `mixed_sessions`.
- R7. A retried call counts again. A client that retries a blocked transfer uses budget each time.
- R8. Chat `n > 1`: the gateway decides the calls of every choice, but the client continues only one
  choice. Counts then grow n times. The runbook states this.
- R9. A `response` event depends on the client. A client that leaves out a result keeps
  `formerly(...::response)` false.
- R10. `on_failure = allow` weakens A2 and A3 for decide timeouts (D10). A truncated or oversized
  hold that passes through under `on_failure = allow` is not counted at all (D8).

## 13. Changes after Gate 1

The PR 1 plan found three facts in the code. They correct the spec, and they do not change a
decision of Sven.

- The chart does not deploy the gateway. D12 and § 8 no longer say that the chart ties the IAM
  setting to the gateway or refuses the memory backend.
- A truncated pass-through hold has no complete call, so it gets no event (D8, R10).
- `RequestToolPolicy::apply` takes `&DecisionContext`, so it returns the session in its outcome, and
  the pipeline puts it into the context (an implementation detail of D4).
- R11 (new). A request-side decision (for example, the replacement of a tool result) cannot act on
  `forged_history` or `mixed_sessions`. Request-side decisions use only the optimistic set
  (SMA-752 D5, `optimistic.rs`), and that set drops every forbid that reads `context.findings`. The
  findings still reach the response-side decisions of the same request. A change needs a change of
  SMA-752 D5, which is out of scope.

## 12. Changelog of revision 2

The builder check (a fresh subagent) and the adversarial challenge (`spec-challenger`, verdict
"needs rework") gave these changes.

**Folded in**

- B1, a forged-result flood can reset a principal sum: results only for linked ids (D5), capped
  lookups of the newest ids only (D4), a split by tool (D5), truncation fails closed (D11).
- B2, clock skew hides concurrent requests: read `[now - W_max, +inf)`, and a future event counts
  (D6, D8).
- B3, a duplicate key or a non-Long value escapes a sum: the rule of D6, approved by Sven.
- The decider does not know the applied outcome: settle moved to the report site, the decision id
  made before the reserve, and pass-through calls append (D1, D8).
- `on_failure` decides a slow store: a history deadline shorter than `decide_timeout` (D10).
- The store-error rule contradicted itself: the D10 table, and A6.
- `max_window_ms` would fail every set: the check applies per condition, and only the slots go
  missing (D10), approved by Sven.
- An incomplete malformed list, and a slot name that panics: the full list in D6.
- A `response` pattern with arguments never matched: result events carry the call's fields (D5).
- Unbounded request-side writes: caps, digests in key names, writes after admission (D4).
- Unbounded read size: one reserve per hold, key selection by tool (D8, D9), a design-maximum
  measurement (§ 9).
- Key eviction: the Redis requirements and the boot check (D11).
- `args_hash` was a plain hash: removed. String and bool values are keyed digests (D7).
- No path for the decision id: D1.
- PR 1 had no working links: PR 1 now has the append-only reserve, settle and links (D15).
- No rollback plan: § 8.
- No fail-path tests and a narrow corpus: § 9.
- Builder check: the result tool comes from the link; the key scope aliases the principal; the IAM
  rollout order; caller-injected time; the port read request; retries count; mixed sessions (R6).
- Minor: slots as an argument of `decide()`; no new `ContextAttr`; results with no call id;
  replaced results; `n > 1`; the memory backend with replicas; the path walk under `negation` and
  `any_of`; last known paths; all `ref=` text forms; an encoding version; one event body;
  `forged_history` only for new results; the domain types; the metrics.
- The evaluator moved to the kernel (D3), after the challenger's question about reuse.

**Not folded in**

- None of the findings was rejected. The question "can the history Redis be the IAM Redis" has the
  answer "no" in D11.
