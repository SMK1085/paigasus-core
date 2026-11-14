# SMA-769: OpenAI Responses API ingress and tool-call interception (`/v1/responses`)

Linear: SMA-769 (parent SMA-747). Blocked by SMA-757 (merged in `ed5409fd`). Related: SMA-749 (the
tool-call model, the interceptor and the oracle), SMA-750 (Anthropic ingress), SMA-752 (policy
decisions), SMA-753 (sequence rules), SMA-755 (decision log).

This spec extends SMA-749 (`2026-11-09-sma-749-tool-call-assembler-design.md`) and SMA-757
(`2026-11-14-sma-757-oracle-argument-prefix-design.md`). Every rule of those two specs applies to
the new format unless this spec says otherwise. A reference in the form "749 § 4.3" points to
them. The spec challenge of 2026-11-14 is in § 14.

## 1. Intent

The gateway serves only `POST /v1/chat/completions`. A client that sends `POST /v1/responses`
cannot use the gateway, so its tool calls are not intercepted.

The newest OpenAI models need the Responses API for tools. On `/v1/chat/completions`, a `gpt-6`
model accepts function tools only with `reasoning_effort: "none"`. `gpt-6.1-sol` and `gpt-6-astra`
do not accept `none`, so they cannot stream tool calls on Chat Completions at all (757 § 2.4).

This change adds `POST /v1/responses`. It has the same authentication, rate limits, budget charge
and tool-call interception as `/v1/chat/completions`, for streamed and non-streamed responses.
SMA-749 lists the Responses API as a non-goal. This spec removes that non-goal.

## 2. Threat model and invariant

The threat model of 749 § 2 applies unchanged: T1 hostile model output, T2 a non-conforming
upstream, T3 the caller is not an adversary of its own agent.

**Invariant (I1, Responses form).** For every supported client parser P (§ 10.1), every tool call
that P derives from the bytes the gateway emits was decided `Allow`, with the same output index,
call id, kind, namespace, name and arguments that the decider saw. "Tool call" means every output
item that the **client** runs (§ 6.2), on every surface where P exposes it. Anything the gateway
cannot classify with certainty is not forwarded (or triggers `on_failure`).

The SMA-757 rule D2 applies from the start: at an excused client raise, every call in the client's
snapshot must match an allowed call exactly. A prefix of allowed arguments is a failure.

## 3. Read state (2026-11-14, `origin/main` `ed5409fd`)

### 3.1 The gateway today

- Only `/v1/chat/completions` has a route (`adapters/http/mod.rs:107`). `/v1/messages` (SMA-750)
  is not wired. The Anthropic format exists only as library code.
- The service code is chat-specific in five places: `interception::intercept_body` (fixed to
  `Format::OpenAiChat`), `interception::intercepted_stream` (fixed to OpenAI and
  `TERMINAL_SSE_ERROR`), `OpenAiClient::chat_completion` (fixed upstream path),
  `usage::request_tokens_estimate` (reads `messages`), and `dto::ChatCompletionRequest` (requires
  `messages`).
- `UsageScanner` reads `/usage/total_tokens` per record, and it skips any record over
  `MAX_RECORD_BYTES = 64 KiB` without a parse (`usage.rs:11`, `:122-126`). `non_stream_usage` reads
  only `choices[]` (`usage.rs:65`).
- The interceptor treats a record over `max_record_bytes` (1 MiB) as a stream failure, even with an
  empty queue (`interceptor.rs:234-237`). `max_request_bytes` defaults to 1 MiB
  (`config.rs:438`).
- `json.rs` `DATA_KEYS` is `["input", "input_schema", "parameters"]`. Below a `DATA_KEYS` key,
  `parse_strict` relaxes the envelope key rules (`case_folded_key`, `constructor`, `prototype`).
  The body path runs `check_numbers` over the whole body (`body.rs:36`).
- The stream capability is `stream_enabled` in config, advertised as
  `Capability::GatewayChatStream`.
- The oracle maps every kind that it does not list to `"other"` (`oracle.rs:89-97`), and
  `arguments_match` returns `true` for `"other"` (`:118-121`). `runners()` falls back to the chat
  runners with a `_ =>` arm (`:81-85`). `check_i1` matches calls by choice, kind, name and
  arguments (`:158`).

### 3.2 Client SDKs (read from source; nothing was executed)

Versions: **openai-python 3.48.0**, **openai-node 7.57.0**, **langchain-openai 1.8.0 with
langchain-core 1.8.0**. In openai-python, `_streaming.py` and
`lib/streaming/responses/_responses.py` are byte-identical in 3.40.0 (the version that 757 cites)
and 3.48.0.

| Aspect | openai-python 3.48.0 | openai-node 7.57.0 | LangChain 1.8.0 |
|---|---|---|---|
| Dispatch key | `data.type`; `event:` ignored | `data.type`; `event: error` raises | `chunk.type` (python raw stream) |
| Top-level truthy `error` key | raises | raises | raises (python) |
| Leading BOM | not stripped | stripped | as python |
| Last event with no blank line at EOF | dropped | flushed | as python |
| `[DONE]` | prefix match, stop | exact match, stop | as python |
| `error` event (`type: error`) | passed through | raises | raises |
| `response.failed` | passed through; `get_final_response()` raises | accumulated | raises |
| Unknown event type | passed through | raises (11 types ignored) | ignored |
| `item: null` on `added` | skipped | skipped | `AttributeError` |
| Keying | `output_index` | `output_index`; `item_id` must equal the item `id` | `item_id` first, then `output_index` |
| `*.done` events | stored apart; the live snapshot is unchanged | replace arguments, input or the whole item | ignored for `function_call`; used for `custom_tool_call` |
| Terminal `response.output` | the final response | replaces the whole snapshot | ignored for tool calls |
| Final call source | terminal `output` | the snapshot (partial at a clean EOF) | `added.arguments` + deltas (+ custom `done`); built-ins as content blocks |
| Item type check | none at terminal for a done index | skips an index it cannot find (`ResponseStream.mjs:244-250`) | none (`base.py:5615-5631`) |

The `ResponseStreamEvent` union of openai-python 3.48.0 has 59 members. With `keepalive` it has 60,
which equals the Node set (`response-accumulator.mjs:242-303`).

Consequences:

- R-a. The SDKs read a call from **different events**. Python reads the terminal `output`. Node
  reads the last of `arguments.done`, `output_item.done` and the terminal event. LangChain reads
  `added.arguments` plus the deltas. All of these views must agree, or a client runs a call that
  the decider did not see (the Responses form of 749 D9).
- R-b. No SDK checks that an index keeps its item type across views. An index that is added as
  `message` and arrives as `custom_tool_call` at `done`, or as `function_call` in the terminal
  `output`, becomes a call in all three SDKs.
- R-c. LangChain's `tool_calls` hold a partial call at every prefix (`parse_partial_json`). A call
  must never be released before it is decided.
- R-d. Python fills a missing `response.output` on `response.failed` and `response.incomplete`
  from its partial items. A terminal event without an explicit `output` array exposes partial
  calls.
- R-e. LangChain appends a `function_call_arguments.delta` to the tool call chunk at its index,
  whatever the item type. A delta event on a `custom_tool_call` or `message` item becomes call
  data.
- R-f. LangChain's source says that real traffic can report a later `output_index` on an item's
  `done` events than on its `added` event, after several built-in tool calls (`base.py:5419-5421`).
  This is not measured (M3).
- R-g. Every lifecycle event carries the whole `response` object. It echoes `instructions`,
  `tools`, `text.format.schema` and `metadata`, and the terminal event repeats every output item.
  These events can be large.

### 3.3 To measure in the implementation (§ 10.5, the first task)

- M1. A continuation with `previous_response_id` after a function call that the client did not
  answer. Expected: a 400 error. This is a reason for D3. D3 stays whatever the result, because
  the gateway still cannot see the earlier turns.
  Measured 2026-11-14: 3 of 3 models (`gpt-6.1-sol`, `gpt-6.1-terra`, `gpt-4.1`) refused the
  continuation with HTTP 400, type `invalid_request_error`, `param` `"input"`, `code` `null` and
  the message `No tool output found for function call <call_id>.`. The first call was a non-stream
  call with `store: true` and a forced function call.
- M2. Whether OpenAI sends `event:` lines, `[DONE]`, `sequence_number`, `obfuscation`, a wire
  `output_text`, and `keepalive` events on Responses streams, and the position of each.
  Measured 2026-11-14 on 78 streams of the 3 models (§ 10.5). Every record has an `event:` line,
  and it is equal to `data.type` (78 of 78). No stream has `[DONE]` (0 of 78): the stream ends
  after the terminal event. Every event has `sequence_number`. It starts at 0 and increases by 1,
  with no gap (78 of 78). `obfuscation` is on every `*.delta` event and on no other event type
  (77 of 78 streams; the one `computer_call` stream has no delta event). No terminal `response`
  has a wire `output_text` key (0 of 78). So the § 6.5 sentence about a wire `output_text` does
  not apply. No stream has a `keepalive` event (0 of 78).
- M3. Whether the events of one output item are contiguous, and whether `output_index` stays the
  same from `added` to `done` (R-f). Include a stream with several provider-run tool calls.
  Measured 2026-11-14: R5 holds in 78 of 78 streams, and `output_index` is the same at `added`
  and `done` for every item (78 of 78). 6 streams have two provider-run `web_search_call` items
  and then a `function_call` (`gpt-6.1-sol` and `gpt-6.1-terra`, `web_search_then_function`).
  `gpt-4.1` did not search in 3 of 3 runs. An extra check of R1 to R4, R6, R8, R9 and R16 found
  no deviation in 78 of 78. **R10 does not hold for `reasoning` items:** in 26 of 26 `reasoning`
  items (one in each of 26 streams of `gpt-6.1-sol` and `gpt-6.1-terra`), the
  `encrypted_content` in the terminal `output` is different from the one in
  `response.output_item.done`. All other fields are equal. The streams did not ask for
  `include: ["reasoning.encrypted_content"]`.
- M4. The `added`, delta and `done` shapes of each client-run kind that can be recorded: whether
  `function_call` `added` carries `arguments: ""`, and whether `computer_call`,
  `apply_patch_call`, `shell_call` and `local_shell_call` are complete at `added`.
  Measured 2026-11-14. `function_call`: `added.item.arguments` is `""`, and the deltas equal
  `function_call_arguments.done` and `done.item.arguments` (78 items in 51 streams).
  `custom_tool_call`: the same with `input` (6 items in 6 `custom_forced` streams of
  `gpt-6.1-sol` and `gpt-6.1-terra`). No model made a custom call and a function call in one
  response (`custom_and_function`: 6 of 6 runs made only function calls). `gpt-4.1` refuses
  `custom` tools: HTTP 400, `Invalid value: 'custom'`, code `unknown_parameter` (6 of 6). The
  built-in kinds were recorded once each on `gpt-6.1-sol`:
  `computer_call` is **not** complete at `added`. `added` has no `actions`, and `done` has
  `actions: [{"type":"screenshot"}]`. No event comes between them. This breaks R14.
  `apply_patch_call` is **not** complete at `added`. `added` has `operation.diff: ""`, the diff
  streams in `response.apply_patch_call_operation_diff.delta` events and one `.done` event (each
  with `item_id`), and `done` has the full diff. This breaks R14. These two event types are not in
  the openai-python 3.48.0 `ResponseStreamEvent` union and not in the § 6.1 map, so § 6.3 makes
  them malformed.
  `shell_call`: `added.item.action.commands` is `[]`. The `shell_call_command.added` command
  (`""`) plus its deltas equals the `.done` command and `done.item.action.commands[0]`. The
  `shell_call_command.*` events have no `item_id`. `added` and `done` also differ in
  `action.max_output_length` (`null` at `added`, `20000` at `done`). This breaks R13.
  `local_shell_call` could not be recorded: HTTP 400, `The local_shell tool is no longer
  supported.` (`param` `"tools"`). `tool_search_call` and `mcp_approval_request` were not tried.
- M5. What OpenAI emits when `max_output_tokens` cuts a function call: an `output_item.done` with
  `status: "incomplete"`, or no `done`.
  Measured 2026-11-14 in 9 of 9 cut streams (3 per model, `max_output_tokens: 48`): OpenAI sends
  one `function_call_arguments.done` and one `response.output_item.done` with
  `status: "incomplete"`, then `response.incomplete` with
  `incomplete_details.reason: "max_output_tokens"`. The arguments at `done` are not complete JSON
  (9 of 9), and they equal the deltas. So R15 sees a truncation. R11 also applies to the same
  item ("`""` or one complete JSON value"), and this spec does not say which rule wins.
- M6. A next turn after a rewrite: replay a rewritten output (a `reasoning` item followed by the
  `msg_paigasus_…` message) as `input`, with and without `store`, with
  `include: ["reasoning.encrypted_content"]`. Expected unknown. If OpenAI rejects it, the rewrite
  (§ 6.5) changes before implementation.
  Measured 2026-11-14: OpenAI accepted the rewritten output in 9 of 9 requests (3 models,
  3 variants each, HTTP 200, `status: "completed"`). Only `gpt-6.1-sol` put a `reasoning` item
  before the call. Its replay was `reasoning`, then `msg_paigasus_1`, then a user message, in all
  3 variants. `gpt-6.1-terra` and `gpt-4.1` made only a `function_call`, so their replay had
  `msg_paigasus_0` and no `reasoning` item. Limit: the one first call per model used
  `store: true` and `include: ["reasoning.encrypted_content"]`. So the `store: false` variant
  replayed a `reasoning` item that had `encrypted_content`. A `reasoning` item without
  `encrypted_content` with `store: false` was not measured.
- M7. The byte size of `response.created` and `response.completed` with a long `instructions`
  (about 50 KB) and 30 tool schemas.
  Measured 2026-11-14 (9 streams, `long_instructions_30_tools`, 50,000 bytes of `instructions`,
  30 function tools): `response.created` is 60,472 to 60,507 bytes, `response.in_progress` is
  60,480 to 60,515 bytes, and `response.completed` is 60,933 to 60,976 bytes (the whole record,
  with the `event:` line). The largest other record in the 78 streams is 4,885 bytes
  (`response.incomplete`). All are below 64 KiB and far below the D15 limits.

## 4. Decisions

| Id | Question | Decision |
|----|----------|----------|
| D1 | Where the format lives | A new module `paigasus-gateway-core::responses` (`classify`, `assembly`, `stream`, `body`), with its own `Protocol` implementation. It shares only the generic parts: `json`, `sse`, `report`, `interceptor`, `body`, `turn`. `ChoiceAssembly` does not change. |
| D2 | New core types | `Format::OpenAiResponses`. `HoldKey::Response` (one hold per response, D10). New `ToolCallKind` variants: `Computer`, `LocalShell`, `Shell`, `ApplyPatch`, `ToolSearch`, `McpApprovalRequest`. `function_call` uses `Function`, `custom_tool_call` uses `Custom`. `ToolCall` gets `namespace: Option<String>`. |
| D3 | Server-side state | **Stateless only.** With interception enabled (the global `[tool_interception] enabled`, that is `AppState.tool_interception` is `Some`, known before admission), a request is refused with 400 `responses-stateful-not-supported` when it has a non-null `previous_response_id`, `conversation` or `prompt`, or an `item_reference` input item. |
| D4 | Background mode | `background: true` is refused with the same 400, whether interception is enabled or not. The gateway has no route for `GET /v1/responses/{id}`, so a background result cannot be fetched through it. |
| D5 | `store` | The request body is forwarded unchanged. The gateway does not set `store: false`. D3 already stops every later turn from reading the stored output. A rewrite of `store` would be the first request rewrite (749 non-goal). Operator note: with `store` true, OpenAI keeps the output, including a blocked call, for its retention period (§ 9). |
| D6 | Service shape | One generic handler pipeline with an endpoint descriptor per route (§ 8). `/v1/chat/completions` moves to it with no change in behaviour. The descriptor is for OpenAI-shaped routes. SMA-750 decides whether it extends it. |
| D7 | Upstream | `OpenAiClient` gets one method that takes a closed `UpstreamRoute` enum (`ChatCompletions`, `Responses`), never a free path. `Responses` forwards to `{base_url}/v1/responses` with the same headers, timeouts and cancellation as chat. |
| D8 | Route switch | A new `[responses] enabled` config key, default `false`. When it is `false`, `/v1/responses` returns 404. Rollback is this key, with no deploy. |
| D9 | Streaming switch | Streaming obeys the existing `stream_enabled`. No new capability is advertised (749 D28). |
| D10 | The hold | **One hold per response.** It opens at the first client-run item's `added` frame and completes at the terminal event. The decider gets every client-run call of the response in one `decide` call, as it gets every call of choice 0 in chat. A truncation (§ 6.4, R15) blocks every call of the hold with `Truncated`. |
| D11 | Output framing | Re-framed as in 749 D3, with an `event: <data.type>` line before each `data:` line. `unframed()` uses the same form. `[DONE]` has no `event:` line. |
| D12 | Rewrite | A blocked item becomes a `message` item at the same `output_index`, frame by frame (§ 6.5). No other index changes. |
| D13 | Abort frame | A new constant for this format: `"\n\nevent: error\ndata: {\"type\":\"error\",\"code\":\"upstream-error\",\"message\":\"upstream stream error\",\"param\":null}\n\n"`. The leading blank line ends a partly sent record, as in `TERMINAL_SSE_ERROR`. |
| D14 | Billing | A Responses usage reader (§ 8.2) with no 64 KiB record limit. 749 D24 applies: a rewrite does not change the charge. |
| D15 | Size limits | Per-format limits. For Responses: `max_record_bytes` 16 MiB, `max_held_bytes` 32 MiB, and a separate `[responses] max_request_bytes` of 16 MiB. These are first values. M7 checks them, and the plan records the measured sizes. Measured: `response.created` at most 60,507 bytes and `response.completed` at most 60,976 bytes, with 50 KB of `instructions` and 30 tools (M7, 2026-11-14). |
| D16 | Strict JSON | Per-format data subtrees and classifier keys (§ 7.1). The chat and Anthropic lists do not change (AC 8). |
| D17 | Request side | `parse_request(Format::OpenAiResponses, …)` builds `TurnFacts` (§ 7.2). |
| D18 | Client-run built-in items | Enforced like function calls (§ 6.2). They are held, decided and rewritten. |

### Reasons

- **D1.** Responses streams are item-based: each event has an `output_index` and an `item_id`, and
  `*.done` events repeat the full value. Chat Completions streams index-delta fragments. A shared
  assembly would put Responses cases into the code path that SMA-757 proved.
- **D3.** The gateway cannot see a turn that OpenAI stores. A decision then has no `prior_calls` or
  `tool_results`, and SMA-753 sequence rules fail open. A blocked call also stays in OpenAI's
  stored output, which the client never saw (M1). A `prompt` template can add tools that the
  gateway cannot see. With interception off, the gateway is a plain proxy, and these fields do no
  harm through it.
- **D8.** Billing and availability depend on shapes that are first measured in this change (M2,
  M7). An operator must be able to turn the route off without a deploy.
- **D10.** SMA-752 and SMA-753 policies that limit the calls per turn need every call of the turn
  in one decision. Chat gives them that per choice. A hold per item would give them one call at a
  time. The cost: frames after the first client-run item wait until the terminal event. Chat has
  the same cost until `finish_reason`.
- **D12.** A replacement at the same index keeps every later `output_index` valid. It needs no
  renumbering, which is the main cost of the chat rewrite.
- **D14, D15.** R-g. A lifecycle event over 64 KiB would charge only the estimate. A lifecycle
  event over 1 MiB would fail the stream, and every SDK raises without `response.created` or a
  terminal event. D3 also makes a client resend the whole conversation on every turn.
- **D18.** A shell command, a file patch or a mouse action has at least the risk of a function
  call. A report of "unenforceable" would let a client run an action that was never decided.

## 5. Types

```rust
pub enum Format { OpenAiChat, AnthropicMessages, OpenAiResponses }
pub enum HoldKey { Choice(u32), Block(u32), Response }
pub enum ToolCallKind {
    Function, Custom, LegacyFunctionCall, AnthropicToolUse, Other,
    Computer, LocalShell, Shell, ApplyPatch, ToolSearch, McpApprovalRequest,
}
pub struct ToolCall { /* 749 fields */ pub namespace: Option<String> }
```

All stay `#[non_exhaustive]`. For a Responses call:

| `ToolCall` field | Value |
|---|---|
| `choice` | `0` |
| `position` | the item's `output_index` |
| `id` | `call_id` (function, custom, computer, local shell, shell, apply patch, tool search); the item `id` for `mcp_approval_request` |
| `namespace` | `namespace` (function, when present); `server_label` (`mcp_approval_request`); `None` otherwise |
| `name` | `name` (function, custom, `mcp_approval_request`); a fixed label for the others: `computer`, `local_shell`, `shell`, `apply_patch`, `tool_search` |
| `arguments` | `arguments` (function; `""` becomes `"{}"`, as chat, 749 § 11); `input` (custom); for every other kind, including `Other`: the `done` item as compact JSON, with `status` removed |

The chat and Anthropic formats set `namespace` to `None`. The notice and the report show
`<namespace>.<name>` when a namespace is present.

## 6. The response stream

### 6.1 Item-scoped event map

Every item-scoped event type belongs to exactly one item type. The map is closed:

| Event types | Item type |
|---|---|
| `response.content_part.*`, `response.output_text.*`, `response.refusal.*`, `response.output_text.annotation.added` | `message` |
| `response.reasoning_text.*`, `response.reasoning_summary_part.*`, `response.reasoning_summary_text.*` | `reasoning` |
| `response.function_call_arguments.*` | `function_call` |
| `response.custom_tool_call_input.*` | `custom_tool_call` |
| `response.shell_call_command.*` | `shell_call` |
| `response.mcp_call_arguments.*`, `response.mcp_call.*` | `mcp_call` |
| `response.mcp_list_tools.*` | `mcp_list_tools` |
| `response.web_search_call.*` | `web_search_call` |
| `response.file_search_call.*` | `file_search_call` |
| `response.code_interpreter_call.*`, `response.code_interpreter_call_code.*` | `code_interpreter_call` |
| `response.image_generation_call.*` | `image_generation_call` |
| `response.audio.*`, `response.audio.transcript.*` | no item (response-level) |
| `response.compaction.compacting` | `compaction` |

The builder takes the exact type list from openai-python 3.48.0 and checks it against Node's
`expectedOutputItemTypes` (`response-accumulator.mjs:111-153`). An item-scoped event whose item
has another type is malformed.

### 6.2 Item classes

- **Tool-free content:** `message`, `reasoning`, `compaction`.
- **Provider-run, reported `unenforceable`, never decided:** `web_search_call`, `file_search_call`,
  `code_interpreter_call`, `image_generation_call`, `mcp_call`, `mcp_list_tools`, `program`,
  `program_output`, `additional_tools`, `tool_search_output`, `shell_call_output`,
  `local_shell_call_output`, `computer_call_output`, `apply_patch_call_output`. This is the
  treatment of Anthropic `server_tool_use` (749 § 4.2): the item opens no hold, its frames are
  forwarded in stream order (behind an open hold, 749 D12), and one report entry with
  `decision: "unenforceable"` is emitted. `on_failure` never applies to it.
- **Client-run (held and decided):** `function_call` (Function), `custom_tool_call` (Custom),
  `computer_call` (Computer), `local_shell_call` (LocalShell), `shell_call` (Shell),
  `apply_patch_call` (ApplyPatch), `tool_search_call` (ToolSearch), `mcp_approval_request`
  (McpApprovalRequest).
- **Any other item type**, including `function_call_output`, `custom_tool_call_output` and
  `mcp_approval_response` in the output: client-run, kind `Other` (fail closed).

`shell_call` and `tool_search_call` are client-run for every value of `environment` and
`execution`. The class must be fixed at `added`, and these fields can be absent there. A hosted
shell or a server search then runs on the provider even when the decision is `block`. The report
says `block` for it, and the notice replaces it for the client. This is a known limit: the
SMA-755 log must not treat such a block as a prevented action (spec author's call).

A `function_call` with a `caller` of type `program` is still a client-run `function_call`.

### 6.3 Record classification

Dispatch on `data.type`. A present `event:` name that differs from `data.type` is malformed. A
record with a top-level `error` key whose value is truthy, and a `type: "error"` record, are
upstream error records (749 D17 when a hold is open; forwarded otherwise). `[DONE]` ends the
stream. The known types are the 59 members of openai-python 3.48.0's `ResponseStreamEvent` union
plus `keepalive`. Any other type is malformed.

### 6.4 Canonical rules (any deviation: stream-level `on_failure`, code `Malformed`)

`keepalive` and the upstream error record are exempt from R1, R2 and R7.

Stream order:

- R1. The first record is `response.created`. Every lifecycle event has the same `response.id`.
- R2. `response.created`, `response.queued` and `response.in_progress` carry `response.output: []`.
- R3. Exactly one terminal event (`response.completed`, `response.incomplete` or
  `response.failed`). It carries `response.output` as an array, never absent or `null` (R-d).
  After it, only `[DONE]` may follow; `[DONE]` is then emitted. Any other record after it is
  dropped and counted (749 D4).
- R4. `output_index` values first appear in order 0, 1, 2, …, each on one
  `response.output_item.added`. Each index has exactly one `added` and at most one `done`.
- R5. The events of an item are contiguous: no event of another item arrives between its `added`
  and its `done` (M3).
- R6. Every item-scoped event has the `output_index` of the open item, and its `item_id` (where
  the type has one) equals that item's `id`. The event type matches the item type (§ 6.1).
- R7. `sequence_number`, when present, is an integer that strictly increases. Either every event
  has it or none does.
- R8. `added.item` is an object with a string `type` and a non-empty string `id`. Item `id`s are
  unique in the response, and `call_id`s are unique in the response. No upstream `id` starts with
  `msg_paigasus_`.

Every item, of every class (R-b):

- R9. `type`, `id`, `call_id`, `namespace` and `name` (each where the type has it) are equal in
  `added`, `done` and the item at the same index in the terminal `output`.
- R10. The terminal `output` has exactly one item per streamed `output_index`, in order, and no
  other item. An item with a `done` frame equals its `done` item as a JSON value in the terminal
  `output`.

Per client-run item (R-a, R-e):

- R11. `function_call`: `added.item.arguments` is `""`. The concatenated
  `function_call_arguments.delta` values equal `function_call_arguments.done.arguments`, which
  equals `done.item.arguments`. At `done`, the arguments are `""` or one complete JSON value
  (749 § 11 `incomplete_arguments`).
- R12. `custom_tool_call`: the same rule with `input` and `custom_tool_call_input.*`. `input` is
  free text.
- R13. `shell_call`: per command index i, the `shell_call_command.added` command plus its deltas
  equal its `shell_call_command.done` command, which equals `done.item.action.commands[i]`.
  Indexes appear in order. `added` and `done` are equal as JSON values except for `status` and
  `action.commands`. `added.item.action.commands` is `[]` or equals the `done` commands
  (preflight ruling F2).
- R14. The other client-run kinds: `added` and `done` are equal as JSON values, except for
  `status` (M4).
- R15. A client-run item whose `done` has a `status` other than `"completed"` is a truncation (M5).
  So are a terminal event, `[DONE]` or EOF while a client-run item has no `done`.
- R16. No event for an item arrives after its `done`.

If a measurement (§ 3.3) shows that real traffic breaks a rule, the work stops and the spec
changes first. A rule is not loosened in code.

### 6.5 Rewrite

Only frames that contain a blocked item are re-serialized. A blocked item's frames are rewritten
one by one, so the rule works for a blocked hold, a truncated item, a hold limit and a stream that
fails while the item assembles (`Mode::Failed`):

- The item's `added` frame becomes three events: `response.output_item.added` with the message
  item, `response.content_part.added` with an empty `output_text` part, and
  `response.output_text.delta` with the notice (§ 6.6).
- Every middle frame of the item is dropped.
- The item's `done` frame becomes `response.output_item.done` with the completed message. When the
  item has no `done` frame, this event is synthesized before the next frame of the stream (the
  terminal event, `[DONE]`, EOF or the D13 frame), as chat's `closing()` does.
- In the terminal event, the item at a blocked index is always replaced with the completed message.
  This replacement is exempt from R10.

The exact JSON, with `<i>` the blocked `output_index`, `<n>` the notice, and `<s>` the
`sequence_number` of the replaced frame (the key is left out when R7 says none):

```json
{"type":"response.output_item.added","output_index":<i>,"sequence_number":<s>,
 "item":{"type":"message","id":"msg_paigasus_<i>","role":"assistant","status":"in_progress","content":[]}}
{"type":"response.content_part.added","item_id":"msg_paigasus_<i>","output_index":<i>,"content_index":0,"sequence_number":<s>,
 "part":{"type":"output_text","text":"","annotations":[],"logprobs":[]}}
{"type":"response.output_text.delta","item_id":"msg_paigasus_<i>","output_index":<i>,"content_index":0,"sequence_number":<s>,
 "delta":"<n>","logprobs":[]}
{"type":"response.output_item.done","output_index":<i>,"sequence_number":<s>,
 "item":{"type":"message","id":"msg_paigasus_<i>","role":"assistant","status":"completed",
         "content":[{"type":"output_text","text":"<n>","annotations":[],"logprobs":[]}]}}
```

The three events from one `added` frame repeat its `sequence_number`. The emitted numbers then do
not strictly increase. No SDK of § 3.2 reads `sequence_number` on a `create` stream (spec author's
call; the Python conformance test of § 10.2 confirms it for openai-python).

No other field of the terminal event changes: `status`, `usage`, `incomplete_details` and `error`
stay as the upstream sent them. If M2 shows a wire `output_text` field, the builder adds the
notice to it and records this here. The Responses API has no `finish_reason`, so nothing else is
rewritten.

**Non-stream.** The same replacement in the body's `output[]` (the completed message). 749 § 4.4
applies otherwise: one re-serialization, and the original bytes when nothing is blocked. A
client-run item with a `status` other than `"completed"` is blocked with code `Truncated` and
replaced, as in the stream (R15; preflight ruling F7).

### 6.6 Notice, names and report

749 § 4.5 and § 4.6 apply. The notice is `[paigasus] tool call "<name>" was blocked (<code>).`.
`<name>` is `ToolCall.name`, prefixed with `<namespace>.` when a namespace is present, then
sanitized. In a report, `choice` is `0` and `position` is the `output_index`.

## 7. Request side and strict JSON

### 7.1 Strict JSON per format

- The Responses data subtrees are: `tools[]` (whole entries), `text`, `metadata`, `instructions`,
  `prompt`, `reasoning`, and `input` (request); `response.tools`, `response.text`,
  `response.metadata`, `response.instructions`, `response.prompt` and `response.reasoning`
  (lifecycle events and the non-stream body). Below a data subtree, the `parse_value_strict` rules
  apply.
- `check_numbers` runs on the envelope and on each client-run item only, not on data subtrees.
- The Responses classifier keys (`type`, `output_index`, `item_id`, `call_id`, `item`, `output`,
  `arguments`, `input`, `name`, `namespace`, `id`, `delta`, `response`, `error`) are a per-format
  list. The chat and Anthropic lists do not change.
- A test shows that a structured-output schema with properties `Name`, `ID`, `Type` and
  `constructor`, a metadata key `Id` and a schema `maximum: 1e300` pass unchanged on stream and
  body.

### 7.2 `TurnFacts`

- The handler parses a copy of the body into a `ResponsesRequest` DTO: `model`, `input`, `stream`,
  `background`, `previous_response_id`, `conversation`, `prompt`, all `Option`, and the rest as
  `extra`. `input` is a string or an array. A body that is not a JSON object is a 400, as in chat.
- D3 and D4 are checked before `Limits::admit`, so a refused request is not counted or charged.
- `definitions` from `tools[]`: function and custom names (with `namespace` from `namespace`
  tools), and the built-in tool types.
- `prior_calls` from the client-run item types of § 6.2 in `input`. `tool_results` from
  `function_call_output`, `custom_tool_call_output`, `computer_call_output`,
  `local_shell_call_output`, `shell_call_output`, `apply_patch_call_output` and
  `mcp_approval_response`.
- `TurnFacts` gets a `definitions_complete: bool`. It is `false` when `tools[]` has a `tool_search`
  tool, or when `input` has an `additional_tools` or `tool_search_output` item: tools can then be
  added at run time. SMA-752 decides how a policy treats an incomplete set. The chat and Anthropic
  formats set it to `true`.

## 8. Service

### 8.1 The generic pipeline

An endpoint descriptor per OpenAI-shaped route:

```rust
struct Endpoint {
    route: UpstreamRoute,          // D7
    format: Format,
    parse: fn(&[u8], bool /* interception */) -> Result<RequestFacts, ApiError>, // DTO, D3, D4
    estimate: fn(&RequestFacts, usize) -> u64,
    usage: UsageReader,            // § 8.2
    terminal: &'static str,        // TERMINAL_SSE_ERROR or D13
    max_request_bytes: usize,      // D15
}
```

The flow is the current chat flow (auth, body limit, parse, stream switch, `Limits::admit`,
`ChargeGuard`, `parse_request`, upstream, usage, interception, non-2xx pass-through), with each
chat-specific step taken from the descriptor. `relay`, `intercepted_stream` and `intercept_body`
take the `Format` and the terminal frame. The names are a shape, not a fixed API.

`/v1/chat/completions` moves to the pipeline. Its behaviour must not change: every existing service
test and every core golden passes byte-identical.

Log events and the `"chat completion metered"` event get an `endpoint` field.
`gateway_upstream_requests_total` gets an `endpoint` label, and the observability drift files are
updated.

### 8.2 Usage and the charge

- **Stream.** A Responses scanner reads a copy of the upstream bytes, as `UsageScanner` does
  (1 token per data record). For `response.completed`, `response.incomplete` and `response.failed`
  records, it reads `response.usage.total_tokens` with a streaming key scan that has no record size
  limit and holds no more than the scan state. The last value wins. Other records over 64 KiB are
  skipped, as today.
- **Non-stream.** `usage.total_tokens` from the original body. Fallback: the completion estimate
  from `output[].content[].text`, `output[].arguments` and `output[].input`, by the chat rule
  (`usage.rs`).
- **Request estimate.** By the chat rule, over `instructions`, the `input` string, and per `input`
  item: `content` text (string or `input_text` parts), `arguments`, `input` and `output`.
- Tests: a terminal event over 64 KiB and one over 1 MiB are charged their reported usage, with
  interception on and off.

## 9. Error handling and operator notes

| Case | Result |
|---|---|
| `[responses] enabled = false` | 404 |
| D3 or D4 field present | 400 `responses-stateful-not-supported`, before admission |
| Body is not a JSON object | 400, as chat |
| Body over `[responses] max_request_bytes` | 413, as chat |
| Rate limit or budget | 429, as chat |
| `stream: true` and `stream_enabled` off | the chat refusal, with the same code |
| Upstream non-2xx | forwarded verbatim, no charge, as chat |
| Malformed record, deviation, truncation, hold limits | 749 § 6, with the D13 frame |
| Malformed non-stream body under `block` | 502 `tool-interception-malformed-response`, as chat |

Operator notes (in the gateway README): D5 retention; the hosted-execution limit of § 6.2; the
`[responses]` keys.

## 10. Testing

### 10.1 Oracle models

Four models, each with its cited version and source paths in a doc comment. Each derives calls
from every surface its SDK exposes:

- **openai-python 3.48.0 `ResponseStream`.** `_streaming.py`, `lib/streaming/responses/_responses.py`
  and `lib/_parsing/_responses.py`. Calls: the items of every yielded `output_item.added` (for
  client-run kinds other than function and custom) and `output_item.done` event, the `output` of a
  yielded `response.incomplete` or `response.failed`, and the `get_final_response()` output.
- **openai-python 3.48.0 raw stream** (`client.responses.create(stream=True)`). The same SSE
  decoder, no accumulation. Calls: the items of every yielded `output_item.done` and the terminal
  `output`. It raises only at the SSE layer, so it is stronger than the `ResponseStream` model at
  an excused raise.
- **openai-node 7.57.0.** `core/streaming.ts`, `lib/responses/ResponseStream.ts` and
  `internal/responses/response-accumulator.ts`. Calls: the final snapshot (partial at a clean
  EOF), plus every emitted `output_item.done` item.
- **LangChain (langchain-openai 1.8.0, langchain-core 1.8.0)** on the raw stream.
  `_convert_responses_chunk_to_generation_chunk`, `merge_lists`, `merge_dicts`, and
  `init_tool_calls` with `parse_partial_json`. Calls: `tool_calls` of the summed chunk, plus the
  content blocks for `computer_call`, `apply_patch_call`, `mcp_approval_request` and
  `tool_search_call` (`base.py:5596-5614`). `invalid_tool_calls` are not calls.

Each model reproduces every raise path that § 3.2 and the source list, and stops there with the
calls of its snapshot (`Run{calls, error}`). The D2 rule applies at each raise.

Oracle changes:

- `check_i1` gets a `RESPONSES_RUNNERS` table. The `_ =>` fallback in `runners()` becomes a panic,
  so a missing format cannot pass with zero calls.
- Each new kind gets its own model kind. Its comparison is JSON equality of the item with `status`
  removed, after the normalisation of each SDK (LangChain: `model_dump(exclude_none=True)`). A
  model call also matches on `position`, `id` (call id) and `namespace`.
- A mutation proof: a changed `action.commands` in a `shell_call` `done` item, and a changed patch
  in an `apply_patch_call`, must make P1 fail.

### 10.2 Real-SDK conformance

The Python conformance package (`py/packages/paigasus-gateway-conformance`) gets golden tests for
the Responses goldens, run through the real openai-python 3.48.0 `ResponseStream` and raw stream.
The package already pins `openai==3.48.0` (`pyproject.toml:13`), so no dependency change is
needed. openai-node and LangChain are covered by the Rust models only, because neither is a
dependency of the repo today.

### 10.3 Generators and properties

- A `responses_scenario()` generator in `tests/support/streams.rs`: items of every class in § 6.2,
  every client-run kind, text and reasoning items, and chunk cuts.
- **Every client raise path and every rule of § 6.4 gets a generator quirk** (757 lesson). The
  list includes: an `event: error` name; a top-level `error` key; `[DONE]x`; an unknown event
  type; a repeated `output_index`; a delta before `added`; an `item_id` mismatch; an event on the
  wrong item type (a function delta on a custom or message item); an item type that changes at
  `done` or in the terminal `output`; a terminal item that never streamed; `item: null`; a
  duplicate `id` or `call_id`; an `msg_paigasus_` id; a non-empty `in_progress.output`; a terminal
  item with a new `name` or `namespace`; `added.arguments` not empty; a terminal `output` absent or
  `null`; a `response.failed` with an open item; an `error` event mid-item; a `done` with
  `status: "incomplete"`; invalid JSON arguments at `done`; a lifecycle `response.id` change; a
  BOM; a last event with no blank line; a `keepalive` without `sequence_number`.
- P1 and P2 hold over at least 10 000 generated cases (`ProptestConfig::with_cases(10_000)`).
- The observe-mode and allow-all agreement properties of 749 § 8.1 get a Responses variant.
- A property test of the oracle itself, as 757 § 7.2: a raise between two argument deltas must be
  an `Err`.

### 10.4 Fixtures and goldens

- Hand-built `responses/conforming` and `responses/malformed` sets, written by `make_fixtures.py`,
  with a row per malformed fixture in `malformed_shapes_follow_section_6`. A client-run kind that
  cannot be recorded (M4) gets a conforming fixture built from the openai-python 3.48.0 types.
- Goldens under `tests/golden/responses/`, with the manifest, under `block_named` and
  `on_failure=block`: a blocked function call with deltas, a blocked built-in with only `added`
  and `done`, a truncated item, and a blocked call after a reasoning item.

### 10.5 Recordings and measurements (the first task)

- `capture.py` gets `--api responses`. It keeps its key handling (key from a file, never printed).
- Scenarios: parallel functions, custom and function, forced custom, a reasoning item before a
  call, text only, structured output, several provider-run tools (web search) then a function,
  `max_output_tokens` inside a call, long `instructions` with 30 tools, and the M1 and M6
  continuations.
- Models: `gpt-6.1-sol` (no reasoning-effort override), `gpt-6.1-terra`, and one older model
  (`gpt-4.1`).
- The results of M1 to M7 go into § 3.3. Each kept recording is deterministic in the tests and
  passes with no deviation (`recorded_streams_pass_unchanged`).
- This task comes first, because R5, R11 and the rewrite (M3, M4, M6) are design rules.

### 10.6 Service tests

A `tests/responses_proxy.rs`: the route switch; authentication; rate limit and budget; the charge
from `response.usage` (streamed and non-streamed, including terminal events over 64 KiB and
1 MiB); the D3 and D4 refusals (with interception on and off); the upstream route; the
non-stream rewrite and the `paigasus-tool-decisions` header; the streamed interception; the D13
frame on an upstream error. The existing chat tests pass unchanged.

## 11. Repo gates this change touches

- The new error code `responses-stateful-not-supported` follows the 749 § 7 chain: the proto enum,
  codegen drift, `GatewayError`, `ci/error-registry/check.py` and the SDK `mapError`.
- The `endpoint` metric label: the observability drift files and `repo:promtool`.
- New config keys: the config tests and the Helm chart values (`charts/CLAUDE.md`).
- The full gate graph of the root `CLAUDE.md`, before the push.

## 12. Acceptance criteria

1. `POST /v1/responses` works streamed and non-streamed, with the authentication, limits and budget
   of chat, behind `[responses] enabled`.
2. The D3 and D4 refusals hold, before admission.
3. Every client-run item kind of § 6.2 is held, decided and rewritten. Every provider-run item is
   reported `unenforceable`.
4. The four oracle models exist, with cited versions. Every raise path and every rule of § 6.4 has
   a generator quirk. The mutation proof of § 10.1 fails P1 as required.
5. P1 and P2 hold over at least 10 000 generated cases.
6. Recorded Responses streams from at least one `gpt-6` model are fixtures and pass with no
   deviation.
7. The D2 rule (no partial call at a client raise) holds in the oracle.
8. `/v1/chat/completions` behaviour is unchanged: all existing tests and goldens pass
   byte-identical.
9. M1 to M7 are recorded in § 3.3, and the spec agrees with them.
10. A terminal event over 64 KiB is charged its reported usage.
11. The full gate graph (root `CLAUDE.md`) passes.

## 13. Non-goals and left to the builder

Non-goals:

- Server-side state: `previous_response_id`, `conversation`, `prompt`, `background`,
  `item_reference` (D3, D4).
- Other Responses routes: `GET`/`DELETE /v1/responses/{id}`, `/v1/responses/{id}/input_items`,
  `/v1/responses/{id}/cancel`, `/v1/responses/compact`, `/v1/responses/input_tokens`.
- The Realtime and WebSocket modes.
- Request rewriting, including `store` (D5).
- Policy deciders (SMA-752), detectors (SMA-754), the decision log (SMA-755), MCP (SMA-756), the
  Anthropic ingress (SMA-750).
- Tool calls encoded inside text content (749 non-goal).
- openai-node and LangChain as real-SDK conformance dependencies (§ 10.2).

Left to the builder:

- Module, type and function names, and the shape of the endpoint descriptor (§ 8.1).
- Whether the Responses oracle models live in `oracle.rs` or in a new file.
- The exact key allowlists per event type, read from openai-python 3.48.0's types.
- The streaming key scan of § 8.2.

## 14. Spec challenge (2026-11-14) — changelog

Verdict: NEEDS REWORK (3 BLOCKER, 9 MAJOR, 11 MINOR, 3 QUESTION). Folded in:

- BLOCKER, item class not fixed across views: R9 and R10 now apply to every item; the closed event
  map of § 6.1; R8 (`item` an object, unique ids); quirks.
- BLOCKER, usage from a large terminal event: § 8.2 scanner with no record size limit; the
  estimate and body fallback; tests over 64 KiB and 1 MiB (AC 10).
- BLOCKER, the oracle does not compare the new kinds: per-kind comparison, the `_ =>` panic, the
  mutation proof (§ 10.1).
- MAJOR: per-format data subtrees and classifier keys (§ 7.1); per-format size limits (D15, M7);
  the frame-by-frame rewrite with exact JSON and the truncated case (§ 6.5); the `shell_call`
  rule per command index (R13, M4); more recording scenarios and the `keepalive`/`error`
  exemption and `[DONE]` after the terminal event (R3, § 10.5); the next-turn measurement (M6);
  `namespace` and `server_label` in the identity (D2, § 5); `definitions_complete` (§ 7.2); four
  oracle models that cover every surface, with match on position and call id (§ 10.1); R11 keeps
  the chat rule `""` → `"{}"` (§ 5); the route switch (D8).
- MINOR: the event counts; `Option` DTO fields; the error-code gate chain (§ 11); the exact D13
  bytes; `unframed()` framing (D11); unique ids and the `msg_paigasus_` prefix (R8); the
  hosted-execution limit (§ 6.2); the closed `UpstreamRoute` (D7); the SMA-750 reuse claim removed
  (D6); the `endpoint` metric label (§ 8.1); a wire `output_text` (M2, § 6.5).
- QUESTION, one call per decision: answered by D10 (one hold per response, spec author's call,
  flagged at the gate). QUESTION, `store` retention: the operator note (D5, § 9). QUESTION, a cut
  function call: M5 and R15.

Rejected: none.
