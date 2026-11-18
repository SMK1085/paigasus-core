# SMA-776: Keep the schema of blocked `computer_call_output` and `shell_call_output` results

Status: design approved by Sven on 2026-11-16 (design conversation). Revised after the
adversarial challenge on the same day. Follow-up from SMA-752 (PR 475).

## 1. Intent

When the tool policy blocks an earlier tool call, the gateway replaces the result of that call
with a fixed notice string (SMA-752 spec § 6.5.2). On the OpenAI Responses API, two result types
do not accept a string in `output`:

- `computer_call_output.output` must be a screenshot object.
- `shell_call_output.output` must be an array of result chunks.

The upstream then answers 400, and the whole turn stops (measured, § 4). This change makes the
gateway send a schema-valid body, or refuse the request itself with a clear error. It never sends
a body that it knows the upstream rejects.

## 2. Goals, non-goals, anti-goals

Goals:

- G1. A blocked `shell_call_output` becomes a schema-valid result that carries the notice. The
  turn continues.
- G2. When a block of a `computer_call_output` is enforced, the gateway refuses the request with a
  400 and a dedicated error code. No upstream call occurs.
- G3. Tests assert the exact output shape of each Responses result type, not only that the body
  parses as JSON. This applies in gateway-core and at the HTTP level.
- G4. SMA-752 spec § 6.5.2, D11, § 6.5.0, § 6.6 and the `request_rewrite` fixture README match the
  new behaviour. The README records the real upstream measurement of § 4.
- G5. Each refusal cause that the plan finds but does not enforce gives a tracing event and a
  refusal metric sample with the label `refused="false"`. This includes the unknown `tool_choice`
  case of SMA-752 § 6.5.1 step 4 with only tenant log-only removals. That case has no event today.

Non-goals:

- The G8 p99 measurement of SMA-752 (`perf.rs` stays `#[ignore]`). It needs an idle Linux host.
- Chat and Anthropic results. Their result fields accept a string, so they do not change.
- A change to the four Responses output types that accept a string: `function_call_output`,
  `custom_tool_call_output`, `local_shell_call_output`, `apply_patch_call_output`.
- The provider-run `mcp_call` item that a client replays with its `output`. The parser does not
  read it (`classify.rs:107`). This gap exists before this change.

Anti-goals:

- A placeholder screenshot for a blocked computer result. It is schema-valid (§ 4), but the model
  gets no notice, and a computer agent can then act on a screen that it did not see.
- Any path that sends the original result bytes of an enforced block upstream (ADR-0027 (c)).
- Notice text or error text that contains result content (SMA-749 D21).

## 3. Constraints

- The notice text stays as `result_notice` builds it: fixed, from the reason and the cleaned tool
  name only.
- The rewrite edits the body through `RawValue` slices. Every untouched member keeps its bytes.
- A tenant deny in log-only mode never changes the body and never refuses (D25). It only reports.
- The floor is always enforced (D33), in every tenant mode and also when the tenant part is
  unavailable. So a floor deny of `computer` refuses the request in log-only mode too. Today, a
  floor removal already changes the body in log-only mode
  (`a_floor_removal_is_enforced_in_log_only`).
- A refused request makes no upstream call and uses no quota: the tool policy runs before the
  limits (SMA-752 D17, D19, `pipeline.rs:107-151`).
- A new error code needs all of these changes:
  - `contracts/proto/paigasus/common/v1/error.proto`, then `buf format -w` and
    `contracts:generate`.
  - `EXPECTED_REASONS` in `rs/crates/libs/paigasus-proto/src/error.rs`.
  - `PRESENTATION` in `ts/packages/paigasus-sdk/src/errors/presentation.ts`. It is a total
    `Record`, so `tsc` fails without the entry.
  - The count in `ts/packages/paigasus-sdk/tests/presentation.test.ts` (74 to 75).
  - A `MANIFEST` row in `ci/error-registry/check.py` only for a new `src/` file that spells the
    code. `adapters/http/error.rs` already has an `emits` row, and its guard iterates
    `GatewayError::iter()`. A row for a file that does not spell the code fails the gate.

## 4. Measurement (real upstream, 2026-11-16)

Synthetic bodies, `POST https://api.openai.com/v1/responses`, model `gpt-6-sol`,
`store: false`, `max_output_tokens: 16`. Each body has a user message, one call, and one output
for that call. "no tool" means that `tools` is absent. This is the body that the rewrite sends
when the denied tool was the last definition.

| case | `tools` | `output` | answer |
|---|---|---|---|
| shell control | `shell` | `[{"stdout":"127.0.0.1 localhost","stderr":"","outcome":{"type":"exit","exit_code":0}}]` | 200 |
| computer control | `computer` | `{"type":"computer_screenshot","image_url":"data:image/png;base64,<1x1 PNG>"}` | 200 |
| shell, the SMA-752 rewrite | `shell` | the notice string | 400 `invalid_type`, `input[2].output`: "expected an array of objects, but got a string instead." |
| computer, the SMA-752 rewrite | `computer` | the notice string | 400 `invalid_type`, `input[2].output`: "expected a computer call output, but got a string instead." |
| shell, notice in `stderr`, exit 1 | `shell` | `[{"stdout":"","stderr":"<notice>","outcome":{"type":"exit","exit_code":1}}]` | 200 |
| shell, notice in `stdout`, exit 0 | `shell` | `[{"stdout":"<notice>","stderr":"","outcome":{"type":"exit","exit_code":0}}]` | 200 |
| shell, notice chunk | no tool | the `stderr` chunk | 200 |
| shell, two commands, one notice chunk | `shell` | the `stderr` chunk | 200 |
| shell, two commands, one notice chunk | no tool | the `stderr` chunk | 200 |
| shell, `max_output_length: 10` on the output item | no tool | the `stderr` chunk (the notice is longer than 10) | 200 |
| `function_call_output` | no tool | the notice string | 200 |
| `custom_tool_call_output` | no tool | the notice string | 200 |
| `apply_patch_call_output` | no tool | the notice string | 200 |
| `apply_patch_call_output` | `apply_patch` | the notice string | 200 |
| `local_shell_call_output` | `local_shell`, and no tool | the notice string | not measurable: 400 `invalid_value` at the `local_shell_call` item (`input[1]`) in both bodies |

More facts from the same session:

- With the GA `computer` tool, the upstream rejects a `computer_call` that has `action`:
  "Computer call input must include exactly one of `action` or `actions`." The body with
  `actions: [{"type":"screenshot"}]` passes. The gateway does not read either member: the plan
  matches a result to its call by `call_id` only.
- `mcp_approval_response` is not measured. It needs an `mcp_approval_request` from a real MCP
  server.
- The schema source is the openai-python SDK types (`response_input_item_param.py`). A
  `shell_call_output` chunk has three required members: `stdout`, `stderr`, `outcome`. `outcome`
  is `{"type":"exit","exit_code":<int>}` or `{"type":"timeout"}`. The screenshot object has no
  text member.

## 5. Approaches

1. **Schema-valid results for both types.** A shell notice chunk, and a placeholder screenshot for
   the computer result. The turn always continues. Rejected for the computer half: see the
   anti-goal in § 2.
2. **A local refusal for both types.** It is simple, but it stops a shell turn that a valid shape
   can keep alive.
3. **A mix (chosen by Sven):** approach 1 for `shell_call_output`, approach 2 for
   `computer_call_output`.

### 5.1 An exception to SMA-752 D11 and ADR-0027 (c)

SMA-752 D11 says: "A blocked call or result does not fail the request (ADR-0027 (c))", and "the
only new HTTP error is the refusal of D17". G2 is a new case where a blocked result fails the
request. Sven chose this on 2026-11-16, because the Responses API has no field that can carry the
notice for a computer result. The exception is narrow: only a `computer_call_output` whose block
is enforced. This spec records it, and § 6.6 updates D11. Whether ADR-0027 in Notion gets an
amendment is Sven's decision at Gate 1.

## 6. Design

### 6.1 One table of result types (`paigasus-gateway-core`)

A new `#[non_exhaustive]` enum `ResultShape` says how each result type is replaced:

| `ResultShape` | item types | replacement |
|---|---|---|
| `Text` | Chat `role:"tool"`, `role:"function"`; Anthropic `tool_result`; Responses `function_call_output`, `custom_tool_call_output`, `local_shell_call_output`, `apply_patch_call_output` | the notice string (Anthropic also sets `is_error: true`) |
| `ShellChunks` | Responses `shell_call_output` | one notice chunk (§ 6.3) |
| `Screenshot` | Responses `computer_call_output` | none: the request is refused (§ 6.2) |
| `Approval` | Responses `mcp_approval_response` | `approve: false`, `reason` = the notice |

One function maps a Responses item type to its `ResultShape`. `turn.rs` uses it to decide which
items are results, and `rewrite.rs` uses it to choose the replacement. The constant
`RESPONSES_OUTPUTS` and the type list at `turn.rs:294` go away. `ToolResultRef` gets a member
`shape: ResultShape`.

### 6.2 Plan (`paigasus-gateway-core/src/policy/plan.rs`)

`RequestPlan::refuse: bool` becomes `refusals: BTreeMap<RefusalCause, bool>`. The value is
`enforced`. `RefusalCause` is a new `#[non_exhaustive]` enum, ordered as the plan steps:
`UnreplaceableResult` (step 3), then `UnknownToolChoice` (step 4).

- To add a cause, the plan ORs `enforced` into the value. So each cause has one entry, and it is
  enforced when any of its findings is enforced.
- `RequestPlan::refusal()` gives the first enforced cause in map order, or `None`. `is_noop()`
  uses it in place of the old flag.
- Step 3 (results): when `ResultJudge::decide` gives `Some(enforced)` for a result reference with
  shape `Screenshot`, the plan adds `UnreplaceableResult` with that `enforced`. The check is per
  result reference, not per target, because the `seen` cache gives the cached outcome for a
  repeated call id. Both orders of a `Text` and a `Screenshot` result for one call id must give
  the cause.
- Step 4 (`tool_choice`): an unknown `tool_choice` with an enforced removal adds an enforced
  `UnknownToolChoice`, as the old flag did. An unknown `tool_choice` with no enforced removal but
  at least one removal adds a cause with `enforced = false` (G5).
- `enforced` follows the existing rules: a floor deny is always enforced (D33), and a tenant deny
  is enforced only in `enforce` mode.
- The report entries do not change. A blocked computer result still gives its `replace-result`
  entry, so the caller sees which tool caused the refusal. On a refused request, an entry with
  `enforced: true` means that the gateway would apply it. The gateway refused the request in
  place of the change. § 6.5.3 of SMA-752 gets this sentence.

### 6.3 Rewrite (`paigasus-gateway-core/src/policy/rewrite.rs`)

The Responses input loop chooses the replacement by `ResultShape`:

- `Text`: `e.set("output", <notice string>)`, as today.
- `ShellChunks`: `output` becomes
  `[{"stdout":"","stderr":<notice>,"outcome":{"type":"exit","exit_code":1}}]`. It is one chunk,
  for any number of commands in the call (measured, § 4). The notice goes in `stderr` with exit
  code 1, so the model reads the result as a failed command, not as the command's output. Every
  other member of the item keeps its bytes: `call_id`, `id`, `status`, `max_output_length`,
  `caller`.
- `Screenshot`: the rewrite does not edit it. The service refuses such a plan before it calls the
  rewrite. If the rewrite still gets an enforced target for a `computer_call_output`, it returns
  `None`. The service then refuses with the existing `rewrite_failed` cause, so the body never
  goes upstream (fail closed).
- `Approval`: as today.

### 6.4 Service (`paigasus-gateway/src/application/tool_policy/request.rs`)

- `Refusal` gets the variant `UnreplaceableResult`, metric label `unreplaceable_result`.
- `RefusalCause` is `#[non_exhaustive]` and belongs to another crate. So the service match has a
  `_` arm, and that arm refuses with `UnreadableTurn` (fail closed).
- `apply_inner` reports the causes of the plan as follows:
  - If `plan.refusal()` gives a cause, the service calls `refusal_event` once for it, with
    `refused = true`, and returns its refusal. The service does not report the other enforced
    causes. So one refused request adds exactly 1 to the counter with `refused="true"`.
  - If no cause is enforced, the service calls `refusal_event` once for each cause, with
    `refused = false`. Then it continues with the rewrite.
- The refusal metric `gateway_tool_policy_request_refusals_total` gets a third label,
  `refused` (`"true"` or `"false"`). The existing calls of `refusal_event` set it from their
  `refused` argument. Update the metric text in `paigasus_observability::names` and in
  `main.rs:291`: it counts refusals and the refusals that the mode did not enforce, and it lists
  the three reasons.
- Example: the floor forbids `computer`, and the tenant mode is `log_only`. A request with a
  `computer_call_output` for a `computer_call` is refused with `tool-result-unreplaceable`.

### 6.5 HTTP (`paigasus-gateway/src/adapters/http/`)

- A new error reason, `ERROR_REASON_TOOL_RESULT_UNREPLACEABLE = 318`, wire code
  `tool-result-unreplaceable`, HTTP 400, OpenAI type `invalid_request_error`, `param: "input"`,
  `Retryable::No`. It goes under the "Tool decisions (SMA-752)" banner. Its comment says that it
  is the only code of that banner that is an HTTP error (SMA-776). The `PRESENTATION` entry in
  the SDK gets its own comment for the same reason.
- A new `GatewayError` variant maps to it. Add `From<Refusal> for GatewayError` next to
  `From<LimitRefusal>`. `UnreadableTurn` and `UnknownToolChoice` keep `invalid-request-body`.
- The error message is fixed. It names no tool and no result content: "A blocked
  computer_call_output cannot be replaced. Remove the computer call and its output from the
  input." The decision header names the tool.
- A refused request now carries the `paigasus-tool-decisions` header when the plan has entries.
  This is a deliberate extension of SMA-752 § 6.5.3, which names "a non-2xx upstream answer". The
  limit refusal already does this (`add_request_report`, `pipeline.rs:145-148`). It also applies
  to `UnknownToolChoice` and `rewrite_failed`. So the `Err` of `apply` carries the plan entries
  with the `Refusal`.
- A refusal of a `stream: true` request is a JSON 400 with the header, not an SSE stream, as for
  the other refusals.

### 6.6 Documentation

- SMA-752 spec:
  - § 6.5.2: replace "The notice replaces the whole content" with the table of § 6.1 and a
    reference to this spec.
  - D11, § 6.5.0 ("`Refusal` is the D17 400") and § 6.6: add the exception of § 5.1 and the new
    code.
  - § 6.5.3: the sentence about `enforced` on a refused request (§ 6.2), and the header on a
    refusal (§ 6.5).
- `tests/fixtures/request_rewrite/README.md`: the new case list, and the measurement table of
  § 4. Each row that § 4 measured gets its result. The other rows say "not measured", with the
  reason (`mcp_approval_response`: no MCP server; `local_shell_call_output`: `gpt-6-sol` rejects
  the `local_shell_call` item). The paragraph "Nobody has measured …" goes.
- Comments that become false:
  - `gateway.toml.example:191-194` (the "upstream 400" text).
  - `paigasus-gateway/src/lib.rs:28-33`.
  - The `Refusal` doc at `request.rs:24` ("Both are a 400 `BadRequestBody`").
  - The `refuse` doc at `plan.rs:63`.
  - The error.proto comments of codes 315-317 stay true ("never as an HTTP error (D11)"): they
    describe those three codes only. The banner gets one line that names 318 as the exception.

## 7. Tests

gateway-core:

- Fixture `responses_outputs_replaced`: the `computer_call` and `computer_call_output` pair moves
  out, and `"computer"` leaves that `CASES` row. The `shell_call_output` expectation becomes the
  chunk array of § 6.3. The fixture harness needs an `.out.json` file for each case, so the
  computer refusal is an inline test, not a fixture.
- A new test in `tests/request_rewrite.rs` parses the rewritten body and checks each Responses
  output type: a string for the four `Text` types, and for `shell_call_output` an array of one
  object with exactly the members `stdout` (`""`), `stderr` (the notice) and `outcome`
  (`{"type":"exit","exit_code":1}`).
- A rewrite unit test in `src/policy/rewrite.rs` (a hand-made plan needs crate access): an
  enforced target for a `computer_call_output` gives `None`.
- A `turn.rs` unit test: each Responses result type gives its `ResultShape`.
- Plan unit tests:
  - An enforced tenant block of a `Screenshot` result gives an enforced `UnreplaceableResult`.
  - A log-only tenant block gives the cause with `enforced = false`.
  - A floor deny in log-only mode gives an enforced cause (D33).
  - A floor deny with the tenant part unavailable gives an enforced cause.
  - Two results for one call id, in both orders (`Screenshot` then `Text`, `Text` then
    `Screenshot`), give the cause.
  - An unknown `tool_choice` with only tenant log-only removals gives `UnknownToolChoice` with
    `enforced = false`.

gateway service (`request.rs` tests):

- Enforce mode: the refusal, the metric label `reason="unreplaceable_result"`, `refused="true"`.
- Tenant log-only mode: the event and the metric with `refused="false"`, no refusal, and the body
  unchanged.
- Floor deny in log-only mode: the refusal.
- An unknown `tool_choice` with a tenant log-only removal (not a floor tool): the event with
  `refused="false"`, and no refusal.
- Update the existing tests that assert the exact label set of the refusal metric
  (`request.rs:233` and the others).

gateway HTTP (`tests/tool_policy.rs`, which has the policy harness):

- Change `tool_result_replacement_for_each_result_type` (lines 1111-1188): the computer pair goes
  out, and the test asserts the shell chunk array of § 6.3 at the `shell_call_output` item.
- A new test for `Ep::RespJson` and `Ep::RespSse`: a blocked `computer_call_output` gets 400 with
  `error.code = "tool-result-unreplaceable"` and the decision header, and the mock upstream gets
  no request.
- The same test with the floor forbidding `computer` in log-only mode.
- Add a header assertion to the unknown `tool_choice` refusal case (lines 1017-1020).

Registry and SDK:

- The `paigasus-proto` test `the_registry_contains_exactly_the_expected_reasons`.
- The SDK `presentation.test.ts` count, and one `map-error.test.ts` case for the new code, as for
  the SMA-769 code.

## 8. Left to the builder

- The exact type that carries the plan entries with a `Refusal` (§ 6.5).
- The names of private helpers, and where the `ResultShape` mapping lives in gateway-core.
- Whether `rewrite_request` keeps `Option` or returns a `Result` that names the cause. If it
  changes, the `rewrite_failed` log cause must stay greppable.
