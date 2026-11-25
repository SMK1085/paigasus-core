# SMA-753 PR 2: history conditions

- **Issue:** [SMA-753](https://linear.app/smaschek/issue/SMA-753), part 2 of 2
- **Parent design:** `docs/superpowers/specs/2026-11-22-sma-753-session-history-design.md` (called
  "the design" below). Sven approved it at Gate 1, revision 2, on 2026-11-22.
- **PR 1:** #527, squash commit `7f1b1f58`
- **Status:** approved by Sven at Gate 1, revision 2, 2026-11-25. Revision 2 adds the results of the
  builder check and of the adversarial challenge (§ 8).

## 1. Intent

PR 1 gave the gateway a trusted history, but no rule reads it. Each history slot stays empty, so a
policy that reads a slot errors: a forbid denies, and a permit does not allow (design D16). PR 2
makes the rules work. The gateway evaluates the history conditions that IAM compiles, fills the
Cedar slots, and IAM accepts saves with conditions behind a setting.

This document does not repeat the design. It lists what PR 2 builds, the facts that the PR 1 code
added, and the decisions that those facts need. A decision of the design stays in force unless this
document names it and changes it. The anti-goals A1 to A6 of the design apply without change.

## 2. Scope

| # | Part | From |
|---|---|---|
| S1 | Kernel condition types, the evaluator, and `decide_with_slots` | design D3, D6; P1, P2 |
| S2 | Proto mapping and load validation of conditions | design D6, D14; P5, P6 |
| S3 | Stored argument fields, the history key, the digests | design D7; P3 |
| S4 | The window read in the reserve, with a persistent truncation mark | design D8, D9, D11; P4 |
| S5 | The failure rules for the slots, and the paths with no session | design D10; P7 |
| S6 | The call link in the reserve | Sven's PR 1 comment (F13); P8 |
| S7 | A verdict on the append port, and the hold intent | Sven's PR 1 comment; P9 |
| S8 | A missing event body counts as no event | Sven's PR 1 comment; P10 |
| S9 | The `forged_history` anchor on the last assistant message with tool calls | design D4, Q1 = A; P11 |
| S10 | Remove `TenantPolicies.refused` and `PolicyCache::store_refused` | Sven's PR 1 comment; P12 |
| S11 | The IAM setting `tool_policies.history_conditions` and its chart value | design D12; P13 |
| S12 | The `iam.proto` comment changes | design D6, D10; P14 |
| S13 | Corpus cases, the parity replay, the cap test, the p99 test, the mutation checks | design § 9; P15 |
| S14 | Runbook and metric catalog | design D13 |

**Not in PR 2.** The follow-up sweep items in Sven's PR 1 comment: the memory-adapter edge cases,
the dropped-after-close counter, the request-side finding counters before admission, the warning
throttle, and the stale key layout in the PR 1 plan. The growth of the `t:` tool index moves into
PR 2 only as far as P4 needs a limit on it.

**Not in PR 2 either.** The decision record does not get the slot values. The metric of P7 gives the
count of missing slots. A record change needs a contract change of SMA-755.

## 3. Facts from the PR 1 code

- F1. `StoredEvent` has `id`, `ts_ms`, `kind`, `tool` and `verdict`. It has no argument values. So
  design D7 is not built (`domain/tool_history.rs:101`).
- F2. `reserve` returns only an `AppendReply` with counts and `truncated` flags. No port method
  reads events back. `ReserveRequest.window_from_ms` is `None` (`application/tool_history.rs:339`).
- F3. The IAM adapter drops `conditions`, `max_window_ms`, `cedar_schema` and `schema_revision`
  (`adapters/policy_source/iam.rs:137-169`).
- F4. `decide()` has two production callers: `policy/calls.rs:226` (response side) and
  `policy/plan.rs:628, 630` (the request-side result differential, SMA-855).
  `Context::from_pairs(...).expect(...)` panics on a duplicate key (`decide.rs:161`). No binding
  calls `decide()`.
- F5. The `c:` link is written only by `settle` (allow) and by `append`. Both run on a task after
  the decision. So a fast next request can see no `c:` link yet (F13).
- F6. `AppendRequest` has no verdict. A denied call with no reserved event writes nothing.
- F7. After an interceptor block (a decide timeout or a decider error), the interceptor has no
  decision id: the ids are made inside the decide future that it drops (`decider.rs:365`,
  `interception.rs:152-160`). So the pending event is never settled.
- F8. `MappedInput.input` is an opaque Cedar `RestrictedExpression` (`input.rs:14-18`). Cedar has no
  public way to read it back. `classify_number` (`detect/input.rs:32`) makes every integral number a
  Long (`5.0` gives `Long(5)`), and a big integer the type Text.
- F9. The compiler emits only `EQ` for a string, bool or decimal literal. All six ops exist only for
  a Long (`translate.rs:429-455`). A decimal literal is the raw text of `decimal("…")`.
- F10. `decision_log.digest_key` is optional. `[tool_history]` uses `deny_unknown_fields`, and the
  environment alone can create the table (`config.rs:244-247`).
- F11. `TenantPart::Unavailable` is set only at policy load. `FloorEntry::floor_only` already holds
  a compiled floor-only set (`cache.rs:70-76, 567`).
- F12. The cap trim sets `truncated` only in the reserve that removes events, and only for the sets
  that this reserve writes (`adapters/history/redis.rs:157-199`).
- F13. The gateway links a request only when the tenant mode is not `off`
  (`request.rs:491-494`). A lookup error gives no session (`application/tool_history.rs:260-265`).
- F14. `Histories::scopes()` leaves out `Key` when the caller has no API key
  (`domain/tool_history.rs:91-97`).
- F15. The compiler is a separate Cargo workspace. It pins `cedar-policy = "=4.14.0"`, and the
  production lock resolves 4.15.0. Its `moon.yml` says that it has no in-tree cargo dependency.
- F16. `ReportedCall.call` (`decision_log/record.rs:272-305`) holds no arguments.
- F17. The perf test sets `history: None` (`tests/perf.rs:162, 502`).

## 4. Decisions

"Sven" marks a decision that Sven made. Every other decision is a proposal of this document.

### P1. The kernel evaluator (S1)

The kernel module `tool_policy::history` holds the types. Each public type is `#[non_exhaustive]`.

- `Condition { slot, part, scope, window_ms, shape }`, with `part` in `Floor`, `Tenant`, and
  `Shape::{Formerly, CountWithin{op, threshold}, SumWithin{path, op, threshold}}`.
- `Predicate::{EventKind, Tool(ToolMatch), Field(FieldTest), AllOf, AnyOf, Not}`.
- `ToolMatch::{NameKey(String), AnyCatalogTool}`.
- `FieldTest { path, op, literal: Literal }`. `Literal::{Long(i64), Digests(Vec<(KeyId, Digest)>)}`.
- `Event { id, ts_ms, kind, tool, fields: Fields, key: Option<KeyId> }`.
- `Field` (one stored value): `Long(i64)`, `Digest(Digest)`, `Absent`, `Mistyped`.
- `Slots`: the Bool value of each slot that has a value. A missing slot is absent.

**Three-valued matching [Sven, 2026-11-25].** A predicate gives true, false or unknown on an event:

| Stored value at the path | `FieldTest` result |
|---|---|
| `Long`, literal `Long` | the comparison |
| `Digest`, literal digests | true when the digest under the event's key id is the same, false otherwise |
| `Digest` and a `Long` literal, or the reverse | unknown (different types: the value cannot be read as the rule expects) |
| `Absent` (the path was captured, and the arguments have no value there) | false |
| the path is not in the event (the event is older than the path, design R3) | false |
| `Mistyped` (an object, an array, or a Text number), or unparsed arguments | unknown |
| `Digest` with a key id that the gateway does not know | unknown |

`AllOf`, `AnyOf` and `Not` use Kleene logic. The shapes then use these rules:

- `formerly`: true when one event in the window is true. Else unknown when one event is unknown.
  Else false.
- `count_within`: count the true events and the unknown events apart. When the comparison gives the
  same result for every count from `true` to `true + unknown`, that is the slot value. Else the slot
  is missing.
- `sum_within`: only events whose predicate is true or unknown take part. An event whose predicate
  is unknown, or whose value at `path` is `Mistyped`, makes the slot missing. A true event with a
  `Long` adds it (a negative value adds 0, the sum saturates). A true event with `Absent` adds 0.

Sven decided on 2026-11-25 that an unknown value fails closed. The rule of design D6 for the decided
call is a special case of it: a decided call with unparsed arguments, or with a non-Long value at a
`sum_within` path, makes that slot missing. The rule now applies only when the decided call's
predicate is true or unknown. So a free-text call of one tool does not remove a sum slot that only
another tool can match (challenger question 1).

**Signature.** The evaluator is one pure function with no clock and no I/O:

```rust
pub fn evaluate(conditions: &[Condition], window: &Window, hold: &[Event], index: usize,
                now_ms: u64, catalog: &Catalog) -> Evaluation
```

- `window` holds the events of each requested set from the reserve reply, without the events of
  this hold, and the truncation mark of each set (P4).
- Call `index` sees `hold[0..=index]` and not the later calls (design D6 step 3). The slots do not
  depend on a verdict, because a pending, allowed or denied event counts the same (C1). So the
  gateway computes the slots of every call of the hold once, before any decision.
- `catalog` is the tool catalog of the tenant set (design D6). P7 says what happens when it is not
  known.
- `Evaluation` holds the `Slots` and the reason for each missing slot.

A condition gives a missing slot when one of these is true:

- a set that it reads is unavailable, or its truncation mark is at or after the window start (P4),
- an event that it depends on is unknown (above),
- its `window_ms` is more than `RETENTION_MS` (design D10),
- its literal digests have no digest (no history key, P3),
- it reads `any_catalog_tool` and the catalog is not known (P7).

Every other rule is C1 of the design: the closed window, the decided event counts, an event with
`ts_ms > now` is inside every window, and `Scope::Key` with no API key reads the principal sets.

### P2. Slots in `decide` (S1)

- The kernel gets a new function `decide_with_slots(call, facts, findings, slots, policies)`.
  `decide()` stays and calls it with no slots. So the change is additive (a minor version bump of
  the lockstep release, design C4).
- The kernel adds each slot as a Bool pair at the top of the Cedar context.
- The kernel holds one constant list of the reserved context names (`input`, `input_text`,
  `findings`, `max_severity`, `call`, `request`, `session`). Load validation (P5) refuses a slot
  with such a name. `decide_with_slots` skips such a slot and does not panic. It does not use
  `debug_assert` on caller data, because Helikon can call it.
- **Response side.** `decide_calls_with` gets one `Slots` for each call.
- **Request side.** The result differential (`plan.rs:628, 630`) passes no slots to both decisions.
  A forbid that reads a slot then errors in both, so it is not a difference. A forbid such as
  `findings.contains(x) && context.slot` errors only in the decision with findings, so the result
  is replaced. So empty slots on the request side can only add replacements, never remove one. A
  test covers both shapes.

### P3. The history key and the stored fields (S3) [Sven, 2026-11-25]

**Key.** The history has its own key, as Sven chose on 2026-11-25:

```toml
[tool_history]
digest_key = "..."               # at least 32 bytes; optional
previous_digest_key = "..."      # optional; read only
```

- The key id is derived from the key bytes: the first 8 bytes of
  `HMAC-SHA-256(key, "paigasus-history-kid")`, as hex. So two replicas can never use one id for two
  keys. Sven's choice showed a `digest_key_id` field. This revision removes it for this reason. The
  config refuses a previous key with the same id as the current key.
- **No key.** The key is optional, so a PR 1 deployment still boots. With no key, the gateway logs a
  warning at boot, stores no string, bool or decimal value (only `Long` and `Absent`), and every
  condition with a digest literal has a missing slot (reason `no_key`). Long conditions still work.
- **Rotation, in two phases.** Phase 1: deploy the new key as `previous_digest_key` on every
  replica. Phase 2: swap the two keys on every replica. Keep the old key as the previous key for 25
  hours (`RETENTION_MS`). An event with an unknown key id is unknown (P1), so a wrong order fails
  closed, not open.
- `GATEWAY_TOOL_HISTORY__DIGEST_KEY` and `GATEWAY_TOOL_HISTORY__PREVIOUS_DIGEST_KEY` are the
  environment routes. `Debug` never prints a key.

**Digest.** `HMAC-SHA-256(key, org || 0x00 || path || 0x00 || tag || value)`, cut to 16 bytes. The
org and the path keep a value from being linked across tenants or paths. The tag and the canonical
value:

- a string: `s` plus the UTF-8 bytes, with no change,
- a bool: `b` plus `1` or `0`,
- a decimal: `d` plus the scaled value (the decimal times 10,000) as an 8-byte big-endian i64.

At load, a decimal literal is parsed with the Cedar grammar: at most four fractional digits and
inside the i64 range. Any other literal makes the set malformed. A decimal literal with an integral
value (`decimal("5.0")`) and a Long argument `5`: the kernel classifies `5.0` in the arguments as
`Long(5)` (F8). The corpus gets the cases `5.0`, `-0.0`, `1.5000`, a big integer and `1e19`, and the
gateway follows Dogwood's decision for each. A deliberate difference is marked `paigasus_differs`.

**One walker.** The kernel gets one typed walk of the arguments. `map_input` builds the Cedar value
from it, and a new public function `extract_fields(input: &ToolInput, plan: &PathSet) ->
ExtractedFields` reads the values from it. So the stored fields and `context.input` cannot differ
(depth limit, dropped nulls, duplicate keys). The kernel does not hold the key: a
`HistoryDigester` trait is injected, and the gateway gives the HMAC implementation. The literals
and the call values use the same digester.

**Path walk.** At load, the gateway walks the conditions of the floor and of the tenant set:

- `AllOf` takes the intersection of the tool sets of its sides. `AnyOf` takes the union. A
  `Tool(NameKey)` gives one tool. `AnyCatalogTool` gives the catalog.
- Only `Not`, and a predicate with no tool node, give every tool.
- The result is a `FieldPlan`: the paths for each tool, and the paths for every tool. When the
  tenant part is unavailable, the gateway uses the last known plan of that org (design D7).

**Encoding.** The event body gets `f` (a JSON object from path to value) and `kid`. The path is the
segments joined with `.`. Load validation refuses a segment that is empty or that holds a `.`. A
`Long` is a JSON number, a digest is a base64url string, `Absent` is `null`, and `Mistyped` is
`{"x":1}`. Unparsed arguments give `"b":1`, and no `f`. The body keeps `v: 1`, because a reader
ignores unknown fields (design § 7).

**Result events.** The write script already reads the `e:` body of the call for the result guard
(`redis.rs:113-125`). It copies `f`, `kid` and `b` from that body into the result event (design
D5). A contract test covers it.

**Appended calls.** The report site computes the fields where `CallFacts::of(call)` runs, because
the `ToolCall` is there. `ReportedCall` carries the fields. The decision record does not get them
(ADR-0027 (i)).

### P4. The window read (S4)

- The cache computes a `ReadPlan` for each set at load: the scopes, the tool sets, and `W_max`.
  Each request computes `from_ms = now - W_max`.
- `ReserveRequest` gets `read: Option<ReadPlan>`. With no condition, it is `None`, and the reserve
  only appends (PR 1 behaviour).
- `Scope::Key` with no API key reads the principal sets (F14).
- **Tool sets.** A `NameKey` reads that tool's set. `AnyCatalogTool` reads the set of each catalog
  tool: Rust computes the keys, and the script does not read `t:`. Only "every tool" (P3) reads the
  `t:` index.
- **Limits.** One reserve reads at most `MAX_READ_SETS = 64` sets and at most
  `MAX_READ_EVENTS = 20_000` events. Over a limit, the script stops reading and returns the set as
  truncated. The slot is then missing, and it is not a store error. The `t:` index of a history
  holds at most 256 tools. Past that limit, a new tool is still written to its own set, but not to
  the index, and the history gets a truncation mark for "every tool".
- **Truncation mark.** When the cap trim removes events from a set, the script writes
  `x:<set key>` with the largest removed score. It keeps the larger value, and it has the TTL of the
  set. The window read returns the mark of each set that it reads. The evaluator makes a slot
  missing when a mark of its sets is at or after the window start. So a flood of one tool also
  fails closed for a rule on another tool that reads it (challenger blocker 1).
- **Reply.** `AppendReply` gets `window: Vec<SetWindow { scope, tool, events, mark, truncated }>`,
  with one entry for each requested set, in the order of the request. The adapter checks the length
  and the order, as `check_len` does for the lookup. A missing entry is a decode error, not an
  empty set. The events are sorted by `(ts_ms, id)`. They include future events and the pending
  events of this hold. The application removes the hold's own events by id.

### P5. Load validation (S2)

Each malformed form of design D6 makes the set malformed. These rules are added:

- A slot name is a reserved context name (the kernel list of P2).
- A `name_key` is not equal to `normalise(name_key)`.
- A predicate is deeper than 32 levels (IAM `MAX_PREDICATE_DEPTH`).
- A path segment is empty or holds a `.`.
- A decimal literal that does not parse (P3).
- A comparison op other than `EQ` on a string, bool or decimal literal (F9). The compiler never
  emits one, so this is a guard.

A malformed tenant set makes the tenant part unavailable. A malformed floor makes the floor
unavailable. Each form has a test.

### P6. Where the conditions travel (S2)

- `PolicyText` and `TenantPolicies` get `conditions` and `max_window_ms`. The IAM source copies
  them from the proto. The file source has no conditions.
- The **cache entry** (the compiled set in `PolicyCache`) holds the kernel conditions, the
  `FieldPlan`, the `ReadPlan`, the digests of the literals, and the catalog. They are made once at
  load.
- The **request snapshot** (`PolicySnapshot`) refers to the cache entry with an `Arc`.
  `PolicySet::Available` gets a `floor_only: Arc<ToolPolicies>` field, which is the `Arc` of
  `FloorEntry::floor_only` (F11). No Cedar is compiled for each request.

### P7. Failure rules for the slots (S5)

The rules are the table of design D10, with the rows that the PR 1 code needs.

| Case | Floor slots | Tenant part |
|---|---|---|
| No `[tool_history]` table | missing (`disabled`) | its slots missing (`disabled`); the set is enforced |
| Reserve error, deadline, or budget skip | missing (`store`) | unavailable when it has a condition; the tenant's `OnUnavailable` decides |
| No session because of a lookup error | missing (`no_session`) | the same as a reserve error |
| A set is truncated or over a read limit | the slots that read it are missing (`truncated`) | the same |
| An unknown event (P1) | missing (`unknown`) | the same |
| `any_catalog_tool` with no known catalog | missing (`catalog`) | not possible: the tenant set has its catalog |
| No condition in either set | no effect on the decision (design A6) | no effect |

- When the tenant part is unavailable for one hold, the decider decides that hold with
  `floor_only` and `TenantPart::Unavailable`. So under `open`, every tenant policy is observed only
  for that hold, also a forbid that reads no slot. Under `closed`, every call is blocked. This is the
  approved rule of design D10, and the runbook states it.
- **The catalog of a floor condition.** In mode `off`, or when the tenant set is unavailable, the
  gateway uses the last known catalog of the org. With no known catalog, the slot is missing.
- **Mode `off`.** The gateway links a request (lookup, write, reserve) when the tenant mode is not
  `off`, **or** when the floor has a condition (design D5). So a floor history rule also works for
  a tenant in mode `off`.
- A new metric counts each missing slot: `gateway_tool_history_slots_missing_total{part, reason}`,
  with `part` in `floor`, `tenant` and `reason` in `disabled`, `store`, `no_session`, `truncated`,
  `unknown`, `retention`, `no_key`, `catalog`. All 16 pairs are primed.

### P8. The call link in the reserve (S6, F13)

- The reserve writes the `c:` link of each call next to its `d:` link: one for the call id, and one
  for the item id when the call has one. The link is a hash with `s`, `t` and `e`, as in PR 1. So a
  PR 1 replica can read it after a rollback. The link holds no state field.
- The reserve does not replace a `c:` link that already exists for another event. A provider can
  reuse a call id (`call_0`) in a later turn, and then the older link stays until settle.
- `settle` keeps the link of a forwarded call. For a blocked call, it deletes the link only when
  `e` still names this event.
- A lost settle keeps the link. A result for that call then counts as linked, also when the call
  was blocked (R-new-1).

### P9. The append verdict and the hold intent (S7)

- `ForwardedCall` becomes `AppendCall { id, tool, verdict, call_id, item_id, fields }`. A denied
  call with no reserved event is appended with the verdict `deny` and no `c:` link. So it counts.
- **Hold intent.** Before the reserve runs, the decider records the decision ids of the hold, by
  position, in `SessionContext`. The interceptor can then find them when it drops the decide future
  (F7). At the report site:
  - for a call that the interceptor blocks, the gateway settles the reserved event with `deny`,
  - for a call that it forwards under `on_failure = allow`, it settles the reserved event with
    `allow`, and it does not append. This removes the double count of design R10.
- A settle for an event that the reserve never wrote is a no-op, as today (`redis.rs:284`).

### P10. A missing event body (S8)

The window read skips a member whose `e:` body does not exist. It does not fail the reserve, and
the event does not count. A new metric counts it:
`gateway_tool_history_events_missing_total`. A contract test covers both adapters. A missing body
comes from an expiry race or an eviction, and the runbook says that every history key has a TTL, so
an evicting `maxmemory-policy` can remove any of them (design D11).

### P11. The `forged_history` anchor (S9) [Sven, Q1 = A]

- The gateway checks the results after the last assistant message **that has tool calls**, not
  after the last assistant message (design D4).
- The call ids and the `ref=` ids stay as in PR 1: they come from the last assistant message.
- For Responses, the anchor is the run of assistant, reasoning and call items that holds at least
  one call item. The results are the result items after the start of that run.
- The 128 cap applies to the result list.
- This closes the trailing-text case of design R2. It does not close every case: a client that
  repeats a linked tool turn after the forged results still hides them (R2, revised in § 6).

### P12. Remove the refusal fields (S10)

No refusal rule comes back. `TenantPolicies.refused`, the `refused` branch in
`PolicyCache::tenant_set`, and `PolicyCache::store_refused` are removed with their tests.

### P13. The IAM setting (S11)

- `ToolPoliciesConfig` gets `history_conditions: bool`, default `false`, in the defaults struct and
  in `Default`. The environment route is `IAM_TOOL_POLICIES__HISTORY_CONDITIONS`.
- `HISTORY_CONDITIONS_ENFORCED_BY_GATEWAY` is removed. The compile helper reads the setting. With
  `false`, IAM refuses each save with a condition (reason 62). With `true`, it accepts it. The IAM
  tests run with both values.
- The chart gets `zones.iam.backend.toolPolicies.historyConditions`, default `false`, in the DPoP
  pattern: `dig` with a default, the env entry only when it is `true`, the name in
  `iamReservedEnv`, env-test rows with their counter, a refusal row for an `extraEnv` collision, a
  golden render, and the chart README.
- The runbook states the order: turn on history in the gateway first, then in IAM. Turn it off in
  IAM first (design § 8).

### P14. The `iam.proto` comments (S12)

The contract comments change, with no field change:

- `HistoryCondition`: an event with `ts_ms > now` is inside every window. The three-valued rule of
  P1, and the decided-call rule of design D6 as P1 states it. A string, bool or decimal is compared
  as a keyed digest. Only `EQ` exists for these types.
- `CompiledToolPolicySet`: the retention check applies per condition, and only the slots of that
  condition are missing (design D10).

The generated code holds the comments, so `contracts:generate` runs, and the drift gate must pass.

### P15. Tests (S13)

The test list of design § 9 stays. PR 2 adds or makes these exact:

- **Corpus.** The compiler corpus gets new files: every predicate node, every op, a `response`
  event with copied fields, `any_catalog_tool`, a principal scope, a key scope, nested paths, a
  string, a bool and a decimal field, the decimal edge cases of P3, a big integer, a missing field,
  a negative value, and the unknown and decided-call rules with `paigasus_differs`. `to_dogwood`
  gets decimal and nested-path support. The Dogwood regeneration stays in the compiler crate.
- **Parity replay.** The replay lives in the `rs/` workspace, in `paigasus-gateway-core` tests. It
  reads `corpus/*.json` as data, and its Moon task lists that directory as an input. Its chain is
  the production chain: the corpus JSON, then the `paigasus_iam_core` protocol `HistoryCondition`,
  then `condition_to_pb` (moved from `paigasus-iam/.../sets.rs:115-144` to `paigasus-iam-core`),
  then the gateway-core proto mapping, then the evaluator and `decide_with_slots`. So it runs on the
  production Cedar version (F15). It requires the corpus `decisions`, except where
  `paigasus_differs` marks a difference. It uses a fixed test key.
- **Proto mapping.** One case for each node, each `LiteralValue` kind, and each malformed form.
- **Contract suite.** New cases: the window read, the truncation mark across tools, the read
  limits, a missing body, the reserve link, the settle delete guard, the append verdict, copied
  result fields, and a reply length check.
- **Cap test.** Real Redis, two store clients, the case of design D8, also with a clock skew of
  50 ms. Its own binary with `retries = 0` in `rs/.config/nextest.toml`, in `docker-containers`.
- **p99 test.** `tests/perf.rs` gets a case with the Redis store: a 1,000-event history, a hold of
  three calls, one `sum_within` and one `formerly` condition. It asserts `p99 < 15 ms` and records
  the Redis time per hold. A second, non-gating case measures the read limits. The test stays
  `#[ignore]` (design § 9), and the PR records both results.
- **Mutation checks.** The five of design § 9, plus: remove the reserve link (P8), ignore the append
  verdict, fail the read on a missing body, remove the `Key` to principal alias, remove the
  future-event rule, remove the removal of the hold's own events, remove the truncation mark, and
  make unknown count as false. Each must make a test fail.

## 5. What is left to the builder

- The exact Lua for P4 and P8, within the PR 1 key layout plus the `x:` mark.
- The internal shape of `Window`, `ReadPlan`, `FieldPlan`, `PathSet` and `ExtractedFields`.
- How the hold intent is stored in `SessionContext`.
- Extra metric labels past P7 and P10.

## 6. Risks and known limits

The limits of the design (R1 to R11) stay, except where this list changes them.

- R2, revised. After PR 2, a trailing assistant message with text only no longer hides
  `forged_history`. A client that repeats a linked tool turn after the forged results still hides
  them. After the link expiry, a result of the last tool turn can give `forged_history` on each
  request until the next tool turn.
- R10, revised. The double count of a decide timeout under `on_failure = allow` is gone (P9). A
  truncated pass-through hold still makes no event.
- R-new-1. A lost settle keeps the `c:` link of a blocked call. A forged result for it then counts
  as linked, and it adds a result event. This can make a `formerly(x::response)` or a
  `count_within(...) >= N` permit allow a call. The risk is low: the client never sees the call id
  of a blocked call.
- R-new-2. One mistyped or unparsed call makes every condition that depends on it missing, for up to
  24 hours, in that call's histories (Sven, 2026-11-25). Only the client's own session, principal or
  key is affected.
- R-new-3. With no history key, every condition with a string, bool or decimal literal has a missing
  slot (P3).
- R-new-4. Each hold now reads its window. The read limits of P4 bound the cost. The p99 test
  measures 1,000 events, and the design maximum is measured but does not gate. The history Redis
  can still be shared with the limits Redis (design D11). The runbook gives the sizing and tells the
  operator to split them when the hold read time grows.
- R-new-5. Under a tenant `fail_mode = open`, a store error makes every tenant policy only observed
  for that hold, caps included (design D10). The runbook states it.
- R-new-6. The diff is large: kernel, gateway-core, gateway, IAM, iam-core, chart, proto comments,
  compiler corpus. The plan must order the tasks so that each one builds and passes on its own.

## 7. Who decided what

- Sven decided on 2026-11-25: the history has its own key with a previous key (P3), and an unknown
  value makes the slot missing (P1).
- Sven's PR 1 comment of 2026-11-23 set the scope items S6 to S10 and the anchor Q1 = A.
- Every other decision is a proposal of this document, for Gate 1.

## 8. Changelog of revision 2

The builder check (a fresh Sonnet subagent) and the adversarial challenge (`spec-challenger`,
verdict "needs rework") gave these changes.

**Folded in**

- Blocker 1, a cross-tool flood removes events with no truncation flag: the persistent `x:` mark (P4).
- Blocker 2, mode `off` and a lookup error give no session: link when the floor has a condition, and
  the new rows of P7.
- Blocker 3, the value model fails open on unknown values: three-valued matching, `Absent` and
  `Mistyped` markers, unknown key id is unknown (P1, P3). Sven decided the cross-tool part.
- `extract_fields` cannot read a Cedar value: one typed walker and an injected digester (P3).
- F4 was wrong: the request-side caller passes no slots, and the reason (P2).
- Result events copy the call's fields (P3). Appended calls carry fields through `ReportedCall` (P3).
- The interceptor cannot find its ids: the hold intent, which also removes the R10 double count (P9).
- The unbounded "every tool" read: catalog keys from Rust, read limits, a limit on `t:` (P4).
- A keyless caller and the key scope: the principal alias (P1, P4).
- A missing reply entry is a decode error (P4).
- `any_catalog_tool` with the empty floor-only catalog: the last known catalog, else missing (P7).
- The decimal rules: Cedar-grammar parse at load, a scaled i64 digest, corpus edge cases (P3).
- Rotation order and key-id collision: two phases, and a derived key id (P3).
- A required key stops PR 1 deployments: the key is optional, and missing slots without it (P3).
- The parity placement: the replay in the `rs/` workspace on the production chain (P15).
- The builder check: the catalog source, the tool name function, the order of slots and verdicts,
  the floor-only fallback semantics, the clock, the link rules, how the tests run, and the binding
  callers. Each has its answer in P1, P4, P7, P8, P15 or F4.
- Minor: `max_severity` in the reserved list, the existing `floor_only`, one term per "snapshot",
  `from_ms` per request, better P5 rules, domain separation and a 16-byte digest, the path encoding,
  the settle delete guard, R-new-1 and R2 wording, narrower path collection, more metric reasons,
  more proto comments, an additive kernel API, a missing-body metric, nested corpus paths, and more
  mutation checks.

**Correction after Gate 1 (plan review, 2026-11-25)**

- The P1 table said that a digest compared with a Long literal is false. That let
  `formerly(transfer{amount > 1000})` miss `{"amount":"2000"}`, which is the fail-open case of Sven's
  decision of 2026-11-25. A different type is now unknown. The plan decisions D-B and D-C apply the
  same rule to a missing key and to a `sum_within` path.

**Not folded in**

- Record the slot values in the decision record: the metric of P7 gives the counts, and a record
  change needs an SMA-755 contract change. Out of scope.
- A separate history Redis as a rule: the read limits bound the cost, and design D11 allows the
  sharing. The runbook gives the sizing instead (R-new-4).
- A `pending` state on the link: the challenger showed that no reader needs it, so P8 keeps no state.
