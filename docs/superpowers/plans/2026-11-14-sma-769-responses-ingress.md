<!-- moon-diagnosis:ok -->
# SMA-769 Responses API Ingress and Tool-Call Interception: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The gateway serves `POST /v1/responses`, streamed and non-streamed, with the authentication, limits, budget charge and tool-call interception of `/v1/chat/completions`.

**Architecture:** A new sans-IO format module `paigasus-gateway-core::responses` (classify, assembly, rewrite, body, stream) implements the existing `Protocol` trait with one hold per response. The service moves the chat handler into a generic pipeline with one endpoint descriptor per route, and adds the Responses route behind `[responses] enabled`. Four Rust oracle models, a generator with one quirk per rule, fixtures, goldens and an openai-python conformance test prove invariant I1.

**Tech Stack:** Rust (edition 2024, rust 1.95), axum, reqwest, serde_json, proptest, cargo nextest. Python 3 standard library for `capture.py`, `measure.py` and `make_fixtures.py`. openai-python 3.48.0 and pytest for conformance. buf codegen for the error code.

**Spec:** `docs/superpowers/specs/2026-11-14-sma-769-responses-ingress-design.md` (it extends `2026-11-09-sma-749-tool-call-assembler-design.md` and `2026-11-14-sma-757-oracle-argument-prefix-design.md`; read the parts it cites).

## Global Constraints

- Work in the worktree `/Users/smaschek/dev/paigasus/paigasus-core-sma-769`, branch `feature/sma-769-gateway-openai-responses-api-ingress-and`. Run every command in the foreground.
- Prefix every shell with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Run cargo from `rs/` and always pass `--locked`.
- Rust crates use edition 2024 and rust-version 1.95. Do not change either.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python and TOML).
- Commits: conventional, with a workspace scope, lower case after the scope, and the key at the end: `feat(rs): … (SMA-769)`, `test(rs): …`, `feat(contracts): …`, `test(py): …`, `docs: …`. End each message with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never begin a body line with `#NNN`. Never use `--no-verify`.
- Do not install host software (no `brew install`, no global `pip`/`npm`). Do not edit `.github/CODEOWNERS` (Moon generates it).
- Never name a file with a Windows reserved base name (`con`, `prn`, `aux`, `nul`, `com1`–`com9`, `lpt1`–`lpt9`). Never put a new file under a directory named `build/`.
- Workspace lints deny warnings, test targets included. Dead code is a hard error. Tasks 3–5 add code that Task 6 wires. Each of those tasks puts exactly one `#![cfg_attr(not(test), allow(dead_code))]` line at the top of each new file it adds, and on `turn::responses`. Task 6 deletes every one of them.
- The API key file is `~/.config/paigasus/openai.key`. Never read, print, copy or `cat` it. Only pass its path to `capture.py --key-file`. Run Python scripts with `python3 -I`.
- Exact values (copy them, do not retype them from memory):
  - Error code `responses-stateful-not-supported`, proto `ERROR_REASON_RESPONSES_STATEFUL_NOT_SUPPORTED = 314;`, HTTP 400, OpenAI type `invalid_request_error`, message `Stateful Responses API requests are not supported by this gateway.`
  - D13 frame, as a Rust literal: `"\n\nevent: error\ndata: {\"type\":\"error\",\"code\":\"upstream-error\",\"message\":\"upstream stream error\",\"param\":null}\n\n"`.
  - Rewrite id: `msg_paigasus_<i>`, with `<i>` the blocked `output_index` in decimal.
  - Responses limits: `max_record_bytes = 16_777_216` (16 MiB), `max_held_bytes = 33_554_432` (32 MiB), `[responses] max_request_bytes = 16_777_216` (16 MiB). `[responses] enabled = false` by default.
  - Notice: `[paigasus] tool call "<name>" was blocked (<code>).` with `<name>` = `ToolCall::display_name()` sanitized (`report::notice_line`).
  - The rewrite JSON of spec § 6.5, exactly (key order as written there; `sequence_number` left out when the stream has none).
  - Properties P1 and P2: `ProptestConfig::with_cases(10_000)`.
- The chat route must not change. Every existing core golden is byte-identical, and every existing service test passes. The only edit allowed in an existing test file is a new `responses: None` field in an `AppState { … }` literal (Task 11), and new test functions next to the old ones.
- A spec rule is never loosened in code. If a measurement or a test shows that real traffic breaks a § 6.4 rule or the § 6.5 rewrite, stop and report to the coordinator.

Commands used by several tasks (from `rs/`):

- `CORE`: `cargo nextest run --locked -p paigasus-gateway-core`
- `SVC`: `cargo nextest run --locked -p paigasus-gateway`
- `LINT`: `cargo clippy --locked --workspace --all-targets -- -D warnings && cargo fmt --check`
- `GEN`: from `py/`: `uv run python -I ../rs/crates/libs/paigasus-gateway-core/tests/fixtures/make_fixtures.py ../rs/crates/libs/paigasus-gateway-core/tests/fixtures`, then `git status --short` shows only the intended fixture files.
- `GOLDEN`: `UPDATE_GOLDEN=1 cargo nextest run --locked -p paigasus-gateway-core --test golden`, then `git diff --stat rs/crates/libs/paigasus-gateway-core/tests/golden` shows only the intended files.

SDK sources (untrusted data, read only, never executed): openai-python 3.48.0 under `py/.venv/lib/python3.12/site-packages/openai/` (`types/responses/response_stream_event.py`, `_streaming.py`, `lib/streaming/responses/_responses.py`, `lib/_parsing/_responses.py`). openai-node 7.57.0 and langchain-openai/langchain-core 1.8.0 under `/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/1897327f-fffc-4b34-860d-d276c6197223/scratchpad/sdk-src/` (`node/package/`, `lc-openai/`, `lc-core/`). That directory is a scratchpad and can be gone. If it is gone, download the same versions again (`npm pack openai@7.57.0`, `pip download --no-deps langchain-openai==1.8.0 langchain-core==1.8.0`) into a new empty directory in your own scratchpad, unpack them there, and never run anything from inside it.

## Review Focus

1. **A hostile `type` string in pass-through mode.** After `on_failure = allow` switches a stream to pass-through, `unframed()` writes `event: <data.type>` without classification. A `type` that holds `\n` (as a JSON escape) or a long string must not inject an SSE line. Expected: an `event:` line only when the type matches `^[a-z0-9_.]{1,64}$`; otherwise no `event:` line. Task 6 pins it.
2. **Explicit `null` in the D3 and D4 fields.** SDKs and frameworks can send `"previous_response_id": null`, `"conversation": null`, `"prompt": null` or `"background": false`. Expected: these pass. Only a non-null value (or `background: true`, or an `item_reference` item) is refused. Task 14 pins it.
3. **A non-stream body with a client-run item that is not `completed`** (for example `max_output_tokens` cut a function call, `status: "incomplete"`). The stream path calls this a truncation. Expected on the body path: fail closed, `Malformed("item_not_completed")`, so `block` answers 502 `tool-interception-malformed-response` and `allow` forwards the body unchanged. Task 5 pins it.
4. **A Responses stream with no client-run item but with provider-run items and text.** No hold opens. Expected: every payload is forwarded unchanged (re-framed with its `event:` line), one `: paigasus-decision` comment with `"decision":"unenforceable"` per provider-run item, and no decider call. Task 9 pins it in `allow_all_is_payload_identical_for_every_conforming_fixture`.
5. **A terminal event over the record limit with interception on.** A terminal event over 16 MiB is a stream failure (D8). Expected: the client gets the D13 path or the `(malformed)` rewrite as 749 § 6 says, and the charge is still the reported `response.usage.total_tokens`, because the usage scanner has no record limit. Task 13 pins the scanner half and Task 14 the HTTP half.

---

### Task 1: Responses recordings and the M1–M7 measurements (spec § 3.3, § 10.5)

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/tests/fixtures/openai/recorded/capture.py`
- Create: `rs/crates/libs/paigasus-gateway-core/tests/fixtures/openai/recorded/measure.py`
- Create: `rs/crates/libs/paigasus-gateway-core/tests/fixtures/responses/recorded/README.md` and the kept `*.sse` files
- Modify: `docs/superpowers/specs/2026-11-14-sma-769-responses-ingress-design.md` (§ 3.3, and the D15 row with the measured sizes)

**Interfaces:**
- Consumes: nothing.
- Produces: recorded streams `tests/fixtures/responses/recorded/<model>__<scenario>.sse` (one kept file per model and scenario) for Task 9; the § 3.3 results that Tasks 4, 6 and 9 read (M2 wire `output_text`, M4 shapes, M5 cut shape).

- [ ] **Step 1: Extend `capture.py`.** Add `--api {chat,responses}` (default `chat`, so the old command lines do not change) and `--dry-run` (writes each request body to `<out>/<stem>.request.json` and sends nothing; `--key-file` is then not read). For `responses`, POST to `/responses` with `"stream": true` and the body keys of the Responses API (`input`, `tools` with flat `{"type":"function","name",…}` entries, `tool_choice`, `parallel_tool_calls`, `max_output_tokens`, `text`, `include`, `store`). Add `RESPONSES_SCENARIOS` with these names, exactly: `parallel_functions`, `custom_and_function`, `custom_forced`, `reasoning_then_call`, `text_only`, `structured_output`, `web_search_then_function`, `max_output_tokens_in_call`, `long_instructions_30_tools` (about 50 KB of `instructions` and 30 function tools), `apply_patch_forced`, `shell_forced`, `local_shell_forced`, `computer_forced`, `m1_unanswered_continuation`, `m6_rewrite_replay`. The two `m*` scenarios run two requests: the first non-stream call makes a function call; the second sends the continuation and writes `<stem>.measure.json` with the HTTP status and the error body, never a header. M1: `previous_response_id` of the first response, with no `function_call_output`. M6: replay the first output as `input` with the blocked call replaced by the § 6.5 completed message (`id` `msg_paigasus_<i>`), in three variants: `store: true`, `store: false`, and `store: false` with `include: ["reasoning.encrypted_content"]`. Keep the key handling as it is: read from the file, never printed.
- [ ] **Step 2: Write `measure.py`.** Standard library only. `python3 -I measure.py <dir>` reads every `*.sse` in `<dir>` and prints one JSON object per file with: `event_lines` (present, and equal to `data.type` or not), `done_marker` (present, position), `sequence_number` (all, none or mixed; strictly increasing or not), `obfuscation` (present or not), `wire_output_text` (the terminal `response` has an `output_text` key or not), `keepalive` (count and positions), `contiguous` (R5 holds or not, with the first breaking index), `index_stable` (each item's `output_index` is equal from `added` to `done`), `added_shapes` (per client-run item type: `arguments`/`input` at `added`, and whether `added` equals `done` without `status`), `done_status` (per client-run item), `max_record_bytes` (per event type). `python3 -I measure.py --self-test` runs two built-in streams (one conforming, one with an item event between another item's `added` and `done`) and exits 1 when a field is wrong.
- [ ] **Step 3: Offline checks.** Run from `rs/crates/libs/paigasus-gateway-core/tests/fixtures/openai/recorded/`: `python3 -I measure.py --self-test` (expected: exit 0), and `python3 -I capture.py --api responses --dry-run --model gpt-6.1-sol --runs 1 --out "$SCRATCH/dry"` with `$SCRATCH` your scratchpad (expected: 15 `*.request.json` files, and every body parses as JSON). Run `python3 -I capture.py --key-file ~/.config/paigasus/openai.key --list-models` only if a model below is not known to exist.
- [ ] **Step 4: Commit the scripts.** `git add capture.py measure.py` and commit `test(rs): record and measure openai responses streams (SMA-769)`.
- [ ] **Step 5: Live run (costs money; keep it small).** For each model `gpt-6.1-sol` (no reasoning-effort override), `gpt-6.1-terra` and `gpt-4.1`: `python3 -I capture.py --api responses --key-file ~/.config/paigasus/openai.key --model <m> --runs 3 --out "$SCRATCH/rec"`. Run the four `*_forced` built-in scenarios with `--runs 1 --only …` on `gpt-6.1-sol` only. A rejected request (for example `gpt-4.1` with `custom` tools) is a result, not a failure: keep its `.error.json` in the scratchpad and record it. Do not re-run a scenario more than three times unless a measurement needs it.
- [ ] **Step 6: Measure.** `python3 -I measure.py "$SCRATCH/rec"`. Fill spec § 3.3: after each of M1–M7 add one line `Measured 2026-11-NN: …` with the numbers (files, counts, sizes). Add the M7 sizes of `response.created` and `response.completed` to the D15 row, as "measured: … bytes".
- [ ] **Step 7: STOP check.** Compare each result with R1–R16 and § 6.5. If any recording breaks a rule (for example `contiguous` false, `index_stable` false, `added.arguments` not `""`, a client-run `done` missing at a clean end), or M6 shows that OpenAI rejects the rewritten message, stop here. Report to the coordinator with the file names and the measure output. Do not start Task 2.
- [ ] **Step 8: Keep the recordings.** Copy one stream per model and scenario that passed (the first conforming run) to `tests/fixtures/responses/recorded/<model>__<scenario>.sse`, unchanged. Write `README.md` there in the shape of `openai/recorded/README.md`: date, models, scenarios, what was rejected, the M1–M7 results, and the "Record again" commands with `--api responses`. Note which M4 client-run kinds could not be recorded (Task 9 builds those from the openai-python types).
- [ ] **Step 9: Commit.** `git add` the README, the kept `*.sse` files and the spec. Commit `test(rs): add recorded responses streams and the m1-m7 results (SMA-769)`.

### Task 2: Core types (spec § 4 D2, § 5, § 7.2)

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/src/types.rs`, `src/turn.rs`, `src/report.rs`, `src/interceptor.rs`, `src/body.rs`, `src/openai/assembly.rs`, `src/openai/body.rs`, `src/openai/stream.rs`, `src/anthropic/stream.rs`, `src/anthropic/body.rs`, `src/decider.rs` (every `ToolCall {…}` and `ToolDefinition {…}` literal)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `ToolCallKind::{Computer, LocalShell, Shell, ApplyPatch, ToolSearch, McpApprovalRequest}`; `as_str()` gives `"computer"`, `"local_shell"`, `"shell"`, `"apply_patch"`, `"tool_search"`, `"mcp_approval_request"`.
  - `HoldKey::Response` (last variant; it sorts after every `Block`).
  - `ToolCall.namespace: Option<String>` and `impl ToolCall { #[must_use] pub fn display_name(&self) -> String }` (`"<namespace>.<name>"`, or `name` when `namespace` is `None`).
  - `ToolDefinition.namespace: Option<String>`.
  - `TurnFacts.definitions_complete: bool`. `TurnFacts::default()` gives `true` (a manual `impl Default`, not the derive).
  - `Format` does not change in this task (Task 6 adds the variant).

- [ ] **Step 1: Write the failing tests** in `types.rs` `mod tests`:
  - `responses_kinds_have_labels`: the six `as_str()` values above.
  - `a_namespace_prefixes_the_display_name`: a call with `namespace: Some("ns")`, `name: "f"` gives `"ns.f"`; with `None` gives `"f"`.
  - `the_response_hold_sorts_last`: `[HoldKey::Response, HoldKey::Block(0), HoldKey::Choice(0)]` sorts to `[Choice(0), Block(0), Response]`.
  - In `turn.rs`: `chat_and_anthropic_definitions_are_complete`: `parse_request` of the existing OpenAI and Anthropic test requests gives `definitions_complete == true` and `namespace == None` on every definition.
  - In `interceptor.rs`: `a_namespaced_call_reports_the_qualified_name` — build a `ToolCall` with `namespace: Some("ns")`, `name: "f"` in the test module, pass it to the new private helper `fn report_entry(c: &ToolCall, decision: ReportDecision, basis: Basis, code: Option<BlockCode>) -> ReportEntry`, and check `report_comment(&entry)` holds `"name":"ns.f"`. `apply_decision` and `BodyCalls::report` both build their entries through this helper (move it to `report.rs` as `pub(crate)` so `body.rs` can use it).
- [ ] **Step 2: Run** `CORE`. Expected: FAIL (no such variants or fields).
- [ ] **Step 3: Implement.** Add the variants and fields. Set `namespace: None` in every existing literal. `choice_of(HoldKey::Response)` returns `0`. Every `match` on `HoldKey` in `openai/stream.rs` and `anthropic/stream.rs` gets a `HoldKey::Response => Vec::new()` arm. The interceptor (`apply_decision`) and `BodyCalls::report` take `name: c.display_name()`, not `c.name.clone()`. The notice code paths that build a name from a `ToolCall` use `display_name()` too.
- [ ] **Step 4: Run** `CORE` and `LINT`. Expected: PASS. Then `cargo nextest run --locked -p paigasus-gateway-core --test golden` with no `UPDATE_GOLDEN`. Expected: PASS (the chat and Anthropic goldens are byte-identical). Run `SVC`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): add the responses tool-call kinds, the response hold and call namespaces (SMA-769)`.

### Task 3: Strict JSON per format and record classification (spec § 6.1–§ 6.3, § 7.1)

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/src/json.rs`, `src/lib.rs` (`mod responses;`)
- Create: `src/responses/mod.rs`, `src/responses/classify.rs`

**Interfaces:**
- Consumes: `sse::Record`, `json::Malformed`, `ToolCallKind` (Task 2).
- Produces (all `pub(crate)`):
  - `json::ResponsesDoc { Request, Event, Body }`.
  - `json::parse_strict_responses(bytes: &[u8], doc: ResponsesDoc) -> Result<Value, Malformed>`.
  - `json::check_numbers_responses(v: &Value, doc: ResponsesDoc) -> Result<(), Malformed>` (skips the data subtrees).
  - `classify::STREAM_EVENT_TYPES: [&str; 60]` (the 59 members of openai-python 3.48.0 `ResponseStreamEvent` plus `"keepalive"`).
  - `classify::ItemClass { Content, ProviderRun, ClientRun(ToolCallKind) }` and `classify::item_class(item_type: &str) -> ItemClass`.
  - `classify::Scope { Lifecycle, Terminal, ItemAdded, ItemDone, Item(&'static str), ResponseLevel, Keepalive, Error }` and `classify::scope_of(ty: &'static str) -> Scope` (`Item(t)` names the one item type of § 6.1).
  - `classify::Event { Done, UpstreamError(Value), Known { ty: &'static str, value: Value } }` and `classify::classify(rec: &Record) -> Result<Event, Malformed>`.

- [ ] **Step 1: Write the failing tests** in `json.rs` and `classify.rs`:
  - `responses_data_subtrees_relax_the_key_rules`: under `ResponsesDoc::Request`, `{"text":{"format":{"schema":{"properties":{"Name":{},"ID":{},"Type":{},"constructor":{}}}}},"metadata":{"Id":"x"}}` parses; under `Event`, the same keys below `response.text` and `response.metadata` parse; under `Body`, below top-level `text` and `metadata`.
  - `responses_envelope_keys_follow_the_responses_list`: `{"Output_Index":0}`, `{"Item_Id":"x"}`, `{"Call_ıd":"x"}` and `{"Namespace":"x"}` are `Err` under `Event`; `{"Choices":[]}` is `Ok` under `Event` (not a Responses classifier key); `{"constructor":{}}` at the envelope is `Err`; `{"__proto__":1}` is `Err` inside a data subtree too.
  - `the_chat_lists_do_not_change`: `parse_strict(br#"{"Output_Index":0}"#)` is `Ok` (AC 8), and the existing `json.rs` tests still pass.
  - `lossy_numbers_are_checked_outside_data_only`: `check_numbers_responses` of `{"text":{"format":{"schema":{"maximum":1e300}}}}` under `Body` is `Ok`; of `{"output":[{"type":"function_call","x":1e300}]}` is `Err`.
  - `the_event_list_has_sixty_types` (`STREAM_EVENT_TYPES.len() == 60`, no duplicates, contains `"keepalive"` and `"response.output_text.delta"`).
  - `every_item_scoped_event_has_one_item_type`: a table test of § 6.1, one row per family, for example `scope_of("response.function_call_arguments.delta") == Scope::Item("function_call")`, `scope_of("response.audio.delta") == Scope::ResponseLevel`, `scope_of("response.compaction.compacting") == Scope::Item("compaction")`.
  - `item_classes_follow_section_6_2`: every type in § 6.2 maps to its class; `"function_call_output"`, `"mcp_approval_response"` and `"never_seen"` map to `ClientRun(ToolCallKind::Other)`.
  - `classify_dispatches_on_data_type`: an `event:` name that differs from `data.type` is `Err`; `[DONE]` is `Event::Done`; a top-level truthy `error` and a `"type":"error"` record are `UpstreamError`; `{"error":null,"type":"response.created",…}` is not an error; an unknown `type` is `Err(Malformed("unknown_type"))`.
- [ ] **Step 2: Run** `CORE`. Expected: FAIL.
- [ ] **Step 3: Implement.** Generalize the strict visitor with a profile: the chat and Anthropic profile keeps `CLASSIFIER_KEYS` and the key-name `DATA_KEYS` rule unchanged. The Responses profile uses `RESPONSES_CLASSIFIER_KEYS = ["type","output_index","item_id","call_id","item","output","arguments","input","name","namespace","id","delta","response","error"]` and path rules: `Request` makes the top-level keys `tools`, `text`, `metadata`, `instructions`, `prompt`, `reasoning`, `input` data; `Event` makes those keys except `input` data one level below a top-level `response` key; `Body` makes them data at the top level. Copy the 59 event types from `openai/types/responses/response_stream_event.py`, and check the item map against Node's `expectedOutputItemTypes` (`response-accumulator.mjs:111-153`); write both source paths in a doc comment. `responses/mod.rs` declares `pub(crate) mod classify;` and the later modules.
- [ ] **Step 4: Run** `CORE` and `LINT`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): classify responses records with per-format strict json (SMA-769)`.

### Task 4: Item assembly and the canonical rules (spec § 5, § 6.2, § 6.4 R1–R16)

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/responses/assembly.rs`
- Modify: `src/responses/mod.rs`

**Interfaces:**
- Consumes: Task 3 (`classify::*`, `ResponsesDoc`), `protocol::{Deviation, MAX_IDENTIFIER_BYTES}`, `report::ReportEntry`.
- Produces (all `pub(crate)`):
  - `assembly::tool_call(position: u32, item: &Map<String, Value>, kind: ToolCallKind) -> Result<ToolCall, Malformed>` — the § 5 mapping (shared with the body path, Task 5).
  - `assembly::ItemFrame { Other, Added, Middle, Done }` and `assembly::Effect { opened: bool, item: Option<u32>, frame: ItemFrame, report: Option<ReportEntry>, terminal: bool }` (`item` is the `output_index` of the client-run item the frame belongs to).
  - `assembly::Assembly` with `with_cap(cap: usize) -> Self`, `accept(&mut self, ty: &'static str, v: &Value) -> Result<Effect, Deviation>`, `hold_open(&self) -> bool`, `truncated(&self) -> bool`, `hold_calls(&mut self) -> Vec<ToolCall>` (output_index order), `slot_of(&self, output_index: u32) -> Option<usize>`, `known(&self) -> Vec<(u32, String)>` (position, `display_name`), `sequence_numbers(&self) -> bool` (R7: every event has one), and `#[cfg(test)] retained(&self) -> usize`.

- [ ] **Step 1: Write the failing tests** in `assembly.rs`, each feeding parsed events to `accept`:
  - `a_function_call_yields_its_call`: `added` (arguments `""`), two deltas, `function_call_arguments.done`, `output_item.done`, `response.completed` gives one call `{choice:0, position:<i>, id:Some(call_id), namespace, name, kind:Function, arguments}`; empty arguments give `"{}"`.
  - `a_custom_call_uses_input`, `a_namespaced_function_keeps_its_namespace`, `an_mcp_approval_request_uses_the_item_id_and_server_label`.
  - `built_in_calls_use_the_done_item_without_status`: `computer_call`, `local_shell_call`, `apply_patch_call`, `tool_search_call` and kind `Other` give `arguments` = compact JSON of the `done` item with `status` removed, and the fixed names `computer`, `local_shell`, `apply_patch`, `tool_search`, and the item `type` for `Other`.
  - `shell_commands_must_agree` (R13): per command index, `added` + deltas equal `shell_call_command.done` and `done.item.action.commands[i]`; a mismatch is `Deviation("shell_command_mismatch")`.
  - One test per rule with the deviation name, exactly: R1 `first_not_created`, `response_id_changed`; R2 `early_output`; R3 `terminal_output_missing`, `second_terminal`; R4 `index_order`, `index_reused`; R5 `item_interleaved`; R6 `wrong_item_index`, `item_id_mismatch`, `event_item_type`; R7 `sequence_number`; R8 `item_shape`, `duplicate_id`, `duplicate_call_id`, `reserved_id`; R9 `item_identity_changed`; R10 `terminal_output_mismatch`; R11 `added_arguments`, `arguments_mismatch`, `incomplete_arguments`; R12 `input_mismatch`; R14 `added_done_differ`; R16 `event_after_done`; and `long_identifier` for an id or name over `MAX_IDENTIFIER_BYTES`.
  - `keepalive_and_errors_are_exempt` (R1, R2, R7): a `keepalive` with no `sequence_number` before `response.created` is accepted.
  - `truncation_is_reported_not_a_deviation` (R15): a client-run `done` with `status: "incomplete"`, and a terminal event while a client-run item has no `done`, give `truncated() == true` and no `Err`.
  - `provider_run_items_report_unenforceable`: a `web_search_call` item opens no hold, gives one `ReportEntry { choice:0, position:<i>, name:"web_search_call", decision: Unenforceable, basis: None, code: None }` at its `added`, and its frames give `item: None`.
  - `the_hold_opens_at_the_first_client_run_added` (D10): `opened` is `true` once, at the first client-run `added`; text and reasoning items before it do not open it.
  - `retained_bytes_are_bounded`: `retained()` never exceeds `cap` plus one record over a long function delta stream (749 D13 accounting).
- [ ] **Step 2: Run** `CORE`. Expected: FAIL.
- [ ] **Step 3: Implement** `Assembly` as one item-at-a-time state machine (R5 makes one open item enough), plus the per-response sets of ids and call ids, and the stored `done` items for R9 and R10. Strip `status` with a copy of the item map. Use the § 6.4 names above as the `Deviation` labels. M4 results from Task 1 decide nothing here: R14 is the rule; if Task 1 recorded a shape that breaks it, Task 1 stopped the work.
- [ ] **Step 4: Run** `CORE` and `LINT`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): assemble responses items under the canonical rules (SMA-769)`.

### Task 5: Rewrite builders, non-stream body and the request side (spec § 6.5 non-stream, § 7.2)

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/responses/rewrite.rs`, `src/responses/body.rs`
- Modify: `src/responses/mod.rs`, `src/turn.rs`

**Interfaces:**
- Consumes: Task 3 (`parse_strict_responses`, `check_numbers_responses`, `item_class`), Task 4 (`assembly::tool_call`), `report::notice_line`.
- Produces (all `pub(crate)`):
  - `rewrite::message_id(i: u32) -> String` (`"msg_paigasus_<i>"`).
  - `rewrite::completed_message(i: u32, notice: &str) -> Value`.
  - `rewrite::added_events(i: u32, notice: &str, seq: Option<&Value>) -> [Value; 3]` and `rewrite::done_event(i: u32, notice: &str, seq: Option<&Value>) -> Value` — the exact § 6.5 JSON.
  - `body::calls(v: &Value) -> Result<(Vec<ToolCall>, Vec<ReportEntry>), Malformed>` and `body::rewrite(v: &mut Value, calls: &[ToolCall], codes: &[Option<BlockCode>])`.
  - `turn::responses(v: &Value) -> Result<TurnFacts, Malformed>` (private to `turn.rs`; Task 6 calls it from `parse_request`).

- [ ] **Step 1: Write the failing tests.**
  - `rewrite.rs`: `the_rewrite_json_is_exact`: `serde_json::to_string` of the four values for `i = 3`, notice `N`, `seq = Some(&json!(7))` equals the four § 6.5 lines with `<i>`=3, `<n>`=N, `<s>`=7, compact; with `seq = None` the `sequence_number` key is absent.
  - `body.rs`: `a_body_yields_its_client_run_calls` (function, custom, shell, mcp_approval_request in one `output[]`); `provider_items_are_unenforceable`; `a_blocked_item_becomes_the_completed_message` (only `output[i]` changes; `status`, `usage`, `incomplete_details`, `error` stay); `an_item_not_completed_is_malformed` (`status: "incomplete"` or `"in_progress"` on a client-run item gives `Malformed("item_not_completed")`; Review Focus 3); `body_rules_follow_r8_and_r11` (duplicate `id`, duplicate `call_id`, a `msg_paigasus_` id, function arguments that are not `""` or one JSON value, each `Malformed`).
  - `turn.rs`: `a_responses_request_yields_definitions_calls_and_results`: `tools[]` with a function, a custom, a `namespace` tool holding functions (definitions get `namespace`), and a `web_search` tool (a definition with the tool `type` as name and kind `Other`); `input` with a prior `function_call`, `custom_tool_call`, `shell_call` and their outputs gives `prior_calls` (id = `call_id`, name, arguments) and `tool_results` (`call_id`) for every output type of § 7.2.
  - `turn.rs`: `a_string_input_has_no_calls` (`"input":"hi"`, no `tools`: empty facts, `definitions_complete == true`).
  - `turn.rs`: `run_time_tools_make_definitions_incomplete`: a `tool_search` tool, an `additional_tools` input item, or a `tool_search_output` input item each give `definitions_complete == false`.
- [ ] **Step 2: Run** `CORE`. Expected: FAIL.
- [ ] **Step 3: Implement.** `body::calls` reads `output[]` of a body parsed with `parse_strict_responses(.., Body)` and checked with `check_numbers_responses(.., Body)` (Task 6 wires that parse into `parse_body`). It applies R8, R11, R12 and R14 per item where a body can (no `added` frame: only the item itself), and `tool_call` for each client-run item.
- [ ] **Step 4: Run** `CORE` and `LINT`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): rewrite and read responses bodies and requests (SMA-769)`.

### Task 6: The Responses stream protocol, and the format goes live (spec D10–D13, § 6.5)

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/responses/stream.rs`
- Modify: `src/types.rs` (`Format::OpenAiResponses`), `src/protocol.rs`, `src/interceptor.rs`, `src/body.rs`, `src/turn.rs`, `src/responses/*.rs` (delete every `allow(dead_code)` line)

**Interfaces:**
- Consumes: Tasks 3, 4, 5.
- Produces:
  - `pub enum Format { OpenAiChat, AnthropicMessages, OpenAiResponses }`.
  - `protocol::Step::Terminal { frame: Frame, completed: Vec<(HoldKey, Vec<ToolCall>)> }` — the interceptor completes each listed hold, then truncates every hold still in `incomplete()`, then enqueues the frame.
  - `protocol::StepError::AfterEnd` — the interceptor counts `Metric::RecordDropped("after_done")` and drops the record, with no stream failure.
  - `Protocol::closing_on_abort(&mut self, verdicts: &Verdicts) -> Vec<Bytes>` with a default body that returns `Vec::new()`. `Interceptor::abort` emits it before the terminal frame. Chat and Anthropic keep the default, so their bytes do not change.
  - `responses::stream::ResponsesStream::with_cap(cap: usize) -> Self`, `impl Protocol`.
  - `Interceptor::new(Format::OpenAiResponses, …)`, `parse_body(Format::OpenAiResponses, …)` and `parse_request(Format::OpenAiResponses, …)` work.

- [ ] **Step 1: Write the failing tests** in `stream.rs` (drive `ResponsesStream` directly, as `openai/stream.rs` tests do) and in `interceptor.rs`:
  - `allow_all_reemits_every_payload_with_its_event_line` (D11): each output record is `event: <type>\ndata: <original payload>\n\n`; `[DONE]` has no `event:` line.
  - `a_blocked_function_call_becomes_a_message_at_its_index`: the `added` frame becomes exactly the three § 6.5 events, every delta frame is dropped, the `done` frame becomes the `output_item.done` message, and the terminal `output[i]` is the completed message; no byte of the blocked arguments is in the output.
  - `a_blocked_item_without_done_gets_a_synthesized_done`: before the terminal frame (release), before EOF (`closing`), and before the D13 frame (`closing_on_abort`).
  - `the_rewrite_repeats_the_sequence_number`: the three events from one `added` carry its `sequence_number`; a stream without `sequence_number` gives events without the key.
  - `a_truncated_item_blocks_the_whole_hold` (D10, R15): two client-run items, the second `done` has `status: "incomplete"`; at the terminal event both calls are blocked with `Truncated`, and the decider is not asked.
  - `frames_after_the_terminal_are_dropped_and_done_is_emitted` (R3): a text event after `response.completed` gives `StepError::AfterEnd`; `[DONE]` after it gives `Step::End`.
  - `provider_frames_wait_behind_an_open_hold` (749 D12): a `web_search_call` after an open hold is released after the decision, in stream order, with its report comment.
  - `unframed_writes_a_safe_event_line` (Review Focus 1): `unframed` of `{"type":"response.created"}` writes `event: response.created`; of `{"type":"a\nb"}`, of a 65-byte type and of a non-JSON record writes no `event:` line.
  - `known_calls_name_the_qualified_names`.
  - In `interceptor.rs`: `a_responses_stream_runs_end_to_end` (one hold, one `Decide` with every client-run call of the response, the rewrite on block), and `the_chat_abort_bytes_do_not_change` (`abort` on an OpenAI chat interceptor with a blocked, truncated choice emits only `TERMINAL`).
  - In `body.rs`: `a_responses_body_parses_and_rewrites_through_parse_body`, and the § 7.1 case `structured_output_keys_pass_on_the_body` (schema properties `Name`, `ID`, `Type`, `constructor`, metadata key `Id`, `maximum: 1e300`: `parse_body` is `Ok` and `apply` with allow returns `None`).
  - In `turn.rs`: `parse_request_reads_a_responses_request`.
- [ ] **Step 2: Run** `CORE`. Expected: FAIL.
- [ ] **Step 3: Implement.** `step` calls `classify`, then `Assembly::accept` for `Known` events; `Terminal` comes from a terminal event (with `completed = [(HoldKey::Response, hold_calls())]` only when the hold is open and not truncated), `End` from `[DONE]`, `UpstreamError` from an error record. Every frame of a client-run item and the terminal frame touch `HoldKey::Response`; no other frame touches anything. `release` rewrites only frames that hold a blocked item (`slot_of` + `Verdict::block_code`); every other frame keeps its payload bytes. The notice for slot `s` is `notice_line(&call.display_name(), code)`. If Task 1 recorded a wire `output_text` field in the terminal `response` (M2), add the notice to it and write that in the spec § 6.5 line that names it; otherwise do nothing. `Interceptor::new` uses `ResponsesStream::with_cap(limits.max_held_bytes)`. `parse_body` uses the Task 3 strict parse and number check for `OpenAiResponses` and the old path for the other two formats. Delete every `allow(dead_code)` line that Tasks 3–5 added.
- [ ] **Step 4: Run** `CORE`, `LINT`, and `cargo nextest run --locked -p paigasus-gateway-core --test golden` (no update). Expected: PASS, goldens unchanged. Run `grep -rn "allow(dead_code)" rs/crates/libs/paigasus-gateway-core/src`. Expected: no line from this branch (`git diff origin/main -- rs/crates/libs/paigasus-gateway-core/src | grep -c "allow(dead_code)"` prints `0`).
- [ ] **Step 5: Commit** `feat(rs): intercept responses streams with one hold per response (SMA-769)`.

### Task 7: The four Responses oracle models (spec § 10.1, AC 4, AC 7)

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/tests/support/responses_oracle.rs`
- Modify: `tests/support/mod.rs`, `tests/support/oracle.rs`, `tests/oracle.rs`

**Interfaces:**
- Consumes: `Format::OpenAiResponses`, `parse_body` (Task 6); `oracle::{Run, Runner, ModelCall, py_events, node_events}`.
- Produces:
  - `ModelCall` gets `pub position: Option<u32>`, `pub id: Option<String>`, `pub namespace: Option<String>`. The chat and Anthropic models set all three to `None`.
  - `responses_oracle::{run_python_stream, run_python_raw, run_node, run_langchain}: fn(&[u8]) -> Run`.
  - `responses_oracle::RESPONSES_RUNNERS: &[(&str, Runner)]` with the names `"openai-python-responsestream"`, `"openai-python-raw"`, `"openai-node-responses"`, `"langchain-responses"`.
  - `oracle::runners(Format)` returns `RESPONSES_RUNNERS` for `OpenAiResponses`, and its last arm is `f => panic!("no oracle runners for {f:?}")`.
  - `oracle::responses_kind_of(k: ToolCallKind) -> &'static str` (`function`, `custom`, and `item` for every other kind, `Other` included).

- [ ] **Step 1: Write the failing tests** in `tests/oracle.rs`:
  - `each_model_reads_a_conforming_stream`: a hand-built stream with a function call, a custom call, an `apply_patch_call` and a `web_search_call` gives, in each of the four models, exactly the three client-run calls with the right `position`, `id` and `namespace` (LangChain gives the built-in calls from its content blocks, `base.py:5596-5614`).
  - `each_raise_path_raises`: one row per raise of spec § 3.2 and § 10.1, per model: openai-node on `event: error`, on an unknown type, on an `item_id` that differs from the item `id`; every model on a top-level truthy `error`; LangChain on `item: null` (`AttributeError`); openai-python `ResponseStream` on `response.failed` at `get_final_response()`; the raw model raises only at the SSE layer.
  - `models_see_every_surface` (R-a, R-b): a stream whose `output_item.done` item differs from the terminal `output` item gives two different calls in openai-python and openai-node; a delta on a `custom_tool_call` index becomes call data in LangChain (R-e).
  - `a_raise_between_two_argument_deltas_is_an_err` (proptest, as 757 § 7.2, 1_000 cases): an allowed function call, a random cut of its arguments into two deltas, an `error` event between them; `check_i1(OpenAiResponses, …, core_failed = true)` is `Err` for at least LangChain and openai-node.
  - `a_changed_shell_command_fails_p1` and `a_changed_patch_fails_p1` (the § 10.1 mutation proof): drive a conforming `shell_call` (and `apply_patch_call`) stream with `allow_all`, change `action.commands[0]` (and `operation.diff`) in the output `done` item and terminal item, and assert `check_i1` is `Err`.
  - No test for the `runners()` panic arm: every `Format` variant has runners, so no test can reach it. Step 4 checks it by reading.
- [ ] **Step 2: Run** `cargo nextest run --locked -p paigasus-gateway-core --test oracle`. Expected: FAIL.
- [ ] **Step 3: Implement** the four models, each with a doc comment that names its version and source files (spec § 10.1 list). Each reproduces its decoder (`py_events` for the two Python models, `node_events` for Node: BOM strip, last-event flush, `[DONE]` exact match), its keying (§ 3.2 table), and its raise paths; at a raise it returns `Run { calls: <snapshot>, error: Some(..) }`. `check_i1` matches by `choice`, `kind`, `name`, arguments and, when the model call has them, `position`, `id` and `namespace`. For model kind `item`, arguments match by JSON equality of the item with `status` removed (LangChain: after a `model_dump(exclude_none=True)` normalization, that is, keys with `null` removed). The chat `"other"` rule does not change.
- [ ] **Step 4: Run** `cargo nextest run --locked -p paigasus-gateway-core --test oracle --test properties --test fixtures`, and `LINT`. Expected: PASS, and the chat and Anthropic results do not change. Check by reading that `runners()` ends with the panic arm.
- [ ] **Step 5: Commit** `test(rs): add the four responses client models to the oracle (SMA-769)`.

### Task 8: The Responses generator, quirks and properties (spec § 10.3, AC 4, AC 5)

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/tests/support/streams.rs`, `tests/properties.rs`

**Interfaces:**
- Consumes: Task 7 (`check_i1`, `RESPONSES_RUNNERS`), `support::driver::{drive, cut, DECIDERS, limits}`.
- Produces: `streams::RQuirk` (one variant per quirk below), `streams::ResponsesScenario { items, quirks, cuts, .. }`, `streams::responses_scenario() -> impl Strategy<Value = ResponsesScenario>`, `streams::render_responses(&ResponsesScenario) -> Vec<u8>`.

- [ ] **Step 1: Write the failing properties** in `properties.rs`:
  - `responses_p1_and_p2_hold(s in responses_scenario())` under `ProptestConfig::with_cases(10_000)`, through `check_all_deciders(Format::OpenAiResponses, …)` (P1 with `check_p1`, P2 with the existing bound, and `check_canary`).
  - `responses_observe_mode_is_bounded_and_parseable` and `responses_allow_all_models_agree_with_the_decider`, in the existing 2_000-case blocks, as the chat variants do.
- [ ] **Step 2: Run** `cargo nextest run --locked -p paigasus-gateway-core --test properties`. Expected: FAIL (no generator).
- [ ] **Step 3: Implement** the generator. Items of every class in § 6.2, every client-run kind, text and reasoning items, allowed names `read_file`/`send_mail` and blocked names `blocked_*` with `CANARY` in their arguments, optional `sequence_number` on all events, optional `[DONE]`, and random chunk cuts. `RQuirk` has one variant per quirk, exactly: `EventErrorName`, `TopLevelError`, `DoneTrailing` (`[DONE]x`), `UnknownType`, `RepeatedIndex`, `DeltaBeforeAdded`, `ItemIdMismatch`, `WrongItemTypeEvent` (a function delta on a custom or a message item), `TypeChangesAtDone`, `TypeChangesInTerminal`, `TerminalItemNeverStreamed`, `ItemNull`, `DuplicateId`, `DuplicateCallId`, `ReservedId`, `EarlyOutput`, `TerminalNewName`, `TerminalNewNamespace`, `AddedArgumentsNotEmpty`, `TerminalOutputAbsent`, `TerminalOutputNull`, `FailedWithOpenItem`, `ErrorEventMidItem`, `DoneIncomplete`, `InvalidJsonArguments`, `ResponseIdChanged`, `Bom`, `NoFinalBlankLine`, `KeepaliveWithoutSequence`, `ItemInterleaved`, `SequenceNotIncreasing`, `EventAfterDone`, `ShellCommandMismatch`. Write one doc line per variant that names its § 6.4 rule or its § 3.2 raise path.
- [ ] **Step 4: Run** the properties. Expected: PASS. Then remove the R5 check in `Assembly::accept` (mark the edit), run again, and see `responses_p1_and_p2_hold` fail; delete the marked edit and run green again. Record the failing case in the commit body.
- [ ] **Step 5: Commit** `test(rs): generate hostile responses streams with one quirk per rule (SMA-769)`.

### Task 9: Fixtures and goldens (spec § 10.4, AC 6)

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/tests/fixtures/make_fixtures.py`, `tests/fixtures.rs`, `tests/golden.rs`, `tests/golden/manifest.json`, `tests/golden/README.md`
- Create: `tests/fixtures/responses/conforming/*.sse`, `tests/fixtures/responses/malformed/*.sse` (written by `GEN`), `tests/golden/responses/*.sse` (written by `GOLDEN`)

**Interfaces:**
- Consumes: Tasks 6–8; Task 1 recordings in `tests/fixtures/responses/recorded/`.
- Produces: the golden manifest entries for Task 16. A Responses entry has the keys `file`, `format` (`"responses"`), `allowed` (`[{name, arguments, choice, position, id, namespace}]`), `notices`, `notice_choices` (all `0`), `notice_positions`, and `finish_reasons` (`{"0": <terminal response.status>}`).

- [ ] **Step 1: Extend `make_fixtures.py`.** Conforming set, names exactly: `text_only`, `reasoning_then_function`, `function_allowed`, `function_blocked`, `parallel_mixed`, `custom_blocked`, `namespace_function`, `computer_call`, `local_shell_call`, `shell_call_commands`, `apply_patch_call`, `tool_search_call`, `mcp_approval_request`, `web_search_then_function`, `provider_only`, `keepalive_events`, `no_sequence_numbers`, `done_marker`, `structured_output_keys` (the § 7.1 schema). Build each client-run kind that Task 1 could not record from the openai-python 3.48.0 types (`openai/types/responses/`). Malformed set: one fixture per `RQuirk` of Task 8, named in snake case (`event_error_name`, `top_level_error`, …), with the bad record after the first client-run `added`, and a `blocked_*` canary in the bad record or after it.
- [ ] **Step 2: Write the failing tests** in `fixtures.rs`: add the Responses sets to `allow_all_is_payload_identical_for_every_conforming_fixture` (Review Focus 4: compare payloads, the `event:` lines and the unenforceable comments), `blocked_canaries_never_appear`, `every_fixture_holds_i1_for_every_client`, `every_split_point_gives_the_same_output`; add one `row(..)` per malformed fixture to `malformed_shapes_follow_section_6` (under `block`: `(malformed)` notice and `stream_failure: true`; under `allow`: `PASS_THROUGH`; a malformed record before any hold is dropped and is `stream_failure: false`); make `recorded_streams_pass_unchanged` also replay `responses/recorded` (no dropped record, no `on_failure` path, every split point under allow-all, one push per decider); add `a_large_lifecycle_event_passes` (a `response.created` and a `response.completed` of 2 MiB each, under the Responses limits: no failure); add `structured_output_keys_pass_on_the_stream` (§ 7.1).
- [ ] **Step 3: Run** `GEN`, then `cargo nextest run --locked -p paigasus-gateway-core --test fixtures`. Expected: PASS for the generated sets. If a recorded stream from Task 1 fails, stop and report (spec § 6.4 last paragraph).
- [ ] **Step 4: Goldens.** Add to `golden.rs` `SET`: `("responses", "function_blocked_deltas")`, `("responses", "builtin_blocked_added_done")`, `("responses", "truncated_item")`, `("responses", "blocked_after_reasoning")`, with matching conforming fixtures. For `"responses"` use `Format::OpenAiResponses`, read `finish_reasons` from the terminal `response.status`, and write the extra manifest keys above. Run `GOLDEN`. Expected: four new files and the manifest change; the six old files are byte-identical. Update `tests/golden/README.md` with the new set.
- [ ] **Step 5: Run** `CORE` and `LINT`. Expected: PASS.
- [ ] **Step 6: Commit** `test(rs): add responses fixtures, recorded replays and goldens (SMA-769)`.

### Task 10: The generic pipeline, with no change in chat behaviour (spec D6, D7, § 8.1)

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/http/pipeline.rs`
- Modify: `src/adapters/http/chat.rs`, `src/adapters/http/interception.rs`, `src/adapters/http/usage.rs`, `src/adapters/http/mod.rs`, `src/adapters/openai/client.rs`, `src/adapters/openai/mod.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `openai::UpstreamRoute { ChatCompletions, Responses }` with `fn path(self) -> &'static str` (`"/v1/chat/completions"`, `"/v1/responses"`), and `OpenAiClient::send(&self, route: UpstreamRoute, body: Bytes, stream: bool) -> Result<ChatResponse, OpenAiError>`. `chat_completion(body, stream)` stays as a one-line call of `send(UpstreamRoute::ChatCompletions, …)`, so `tests/openai_egress.rs` does not change.
  - `usage::StreamUsage { Chat(UsageScanner), Responses(ResponsesUsageScanner) }` with `feed`, `finish`, `completion_records`, `reported_total`. In this task only `Chat` exists; Task 13 adds `Responses`.
  - `chat::feed_scan(scan: &mut StreamUsage, …)` and `chat::finish_scan(scan: &mut StreamUsage, …)`.
  - `pipeline::RequestFacts { model: String, stream: bool, request_estimate: u64 }`.
  - `pipeline::Endpoint { name: &'static str, route: UpstreamRoute, format: Format, parse: fn(&[u8], bool) -> Result<RequestFacts, GatewayError>, non_stream_usage: fn(&[u8]) -> BodyUsage, stream_usage: fn() -> StreamUsage, terminal: &'static str, limits: fn(&ToolInterception, &AppState) -> Limits }`. `name` is `"chat_completions"` for chat (Task 15 uses it as the label).
  - `pipeline::proxy(endpoint: &'static Endpoint, state: AppState, caller: CallerContext, body: Bytes) -> Response`.
  - `interception::intercept_body(ti: &ToolInterception, limits: Limits, format: Format, ctx, turn, body: &[u8]) -> BodyOutcome` and `interception::intercepted_stream(format: Format, limits: Limits, inner, guard, usage: StreamUsage, ti, ctx, turn, terminal: &'static [u8])`.
  - `chat::CHAT: Endpoint` and `chat::chat_completions` (unchanged signature) that calls `pipeline::proxy(&CHAT, …)`.

- [ ] **Step 1: Pin the behaviour first.** Run `SVC` and `cargo nextest run --locked -p paigasus-gateway-core`. Save the list of passing test names: `cargo nextest list --locked -p paigasus-gateway > "$SCRATCH/svc-before.txt"`. Write one new test in `tests/chat_proxy.rs`, `the_chat_route_forwards_to_the_chat_path`: the mock records the path `"/v1/chat/completions"` (add `pub path: String` to `RecordedRequest` in `tests/support/mod.rs`). Run it. Expected: FAIL (no `path`).
- [ ] **Step 2: Refactor.** Move the handler body of `chat_completions` into `pipeline::proxy` step for step, with each chat-specific step taken from the descriptor: the DTO parse (`ChatCompletionRequest`, the 400 split of SMA-588), the request estimate, `send(route, …)`, `non_stream_usage`, the scanner, `intercept_body`/`intercepted_stream` with `format`, `limits` and `terminal`, and the non-intercepted relay `terminal_sse_error_stream(inner, guard, usage, terminal)`. For chat, `limits` returns `ti.limits` unchanged. Keep every log line, every comment that explains a load-bearing rule, and the extractor order.
- [ ] **Step 3: Run** `SVC`, `CORE` and `LINT`. Expected: PASS, the same test names as `svc-before.txt` plus the new test, and the chat-only unit tests in `chat.rs` and `interception.rs` pass with only the new call signatures. `git diff origin/main -- rs/crates/services/paigasus-gateway/tests` shows only `RecordedRequest.path` and the new test.
- [ ] **Step 4: Commit** `refactor(rs): move the chat handler into an endpoint pipeline (SMA-769)`.

### Task 11: The `[responses]` config and the route switch (spec D8, D15)

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/config.rs`, `src/adapters/http/mod.rs` (`AppState`, `router`), `src/main.rs`, `gateway.toml.example`, every `AppState { … }` literal (`src/adapters/http/mod.rs:261`, `tests/chat_proxy.rs:164` and `:674`, `tests/metrics.rs:103`, `:158`, `:233`, `tests/service_info.rs:156`, `tests/support/limits.rs:218`)

**Interfaces:**
- Consumes: Task 10.
- Produces:
  - `config::ResponsesConfig { enabled: bool, max_request_bytes: usize, max_record_bytes: usize, max_held_bytes: usize }`, `#[serde(default, deny_unknown_fields)]`, defaults `false`, `16_777_216`, `16_777_216`, `33_554_432`; `GatewayConfig.responses: ResponsesConfig`; `ResponsesConfig::validate(&self) -> Result<(), String>` (every value at least 1, `max_record_bytes <= max_held_bytes`; each message names the key, for example `responses.max_record_bytes`).
  - `http::ResponsesSettings { max_request_bytes: usize, max_record_bytes: usize, max_held_bytes: usize }` and `AppState.responses: Option<Arc<ResponsesSettings>>` (`None` when `enabled = false`).
  - The router mounts `POST /v1/responses` only when `state.responses` is `Some`, in its own group with the auth `route_layer` and `DefaultBodyLimit::max(settings.max_request_bytes)`. Task 14 supplies the handler and mounts the route. In this task `state.responses` is only built and stored.

- [ ] **Step 1: Write the failing tests** in `config.rs`: `responses_defaults_match_the_spec` (the four defaults), `responses_keys_load_from_toml_and_env` (`[responses] enabled = true`, and `GATEWAY_RESPONSES__ENABLED=true`), `responses_validation_names_the_key` (a zero value, and record over held), `an_unknown_responses_key_fails` (`deny_unknown_fields`).
- [ ] **Step 2: Run** `SVC`. Expected: FAIL.
- [ ] **Step 3: Implement.** Add `responses: None` to every `AppState` literal listed above; this is the only edit to those test files. `main.rs` builds `ResponsesSettings` from the config when `enabled` is `true`, with one info log line `"the responses route is on (SMA-769)"`. Add a commented `[responses]` section to `gateway.toml.example` next to `[tool_interception]`, with each key, its default, its env name (`GATEWAY_RESPONSES__…`), and the three operator notes of spec § 9 (Task 17 writes the final text).
- [ ] **Step 4: Run** `SVC` and `LINT`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): add the [responses] config and route switch (SMA-769)`.

### Task 12: The error code `responses-stateful-not-supported` (spec § 11, 749 § 7 chain)

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/error.proto`, the generated files (`rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs`, `py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/__init__.py`, `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts`), `rs/crates/libs/paigasus-proto/src/error.rs` (`EXPECTED_REASONS`), `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs`, `ts/packages/paigasus-sdk/src/errors/presentation.ts`, `ts/packages/paigasus-sdk/tests/map-error.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `GatewayError::ResponsesStatefulNotSupported { param: &'static str }` → 400, `invalid_request_error`, code `responses-stateful-not-supported`, `param` = the refused field (`"previous_response_id"`, `"conversation"`, `"prompt"`, `"input"` for an `item_reference` item, or `"background"`), the message of the Global Constraints, `Retryable::No`.

- [ ] **Step 1: Proto.** Add after `ERROR_REASON_TOOL_INTERCEPTION_MALFORMED_RESPONSE = 313;`, with a comment in the style of its neighbours: `// "responses-stateful-not-supported" — a /v1/responses request asked for server-side state (previous_response_id, conversation, prompt, an item_reference input item) with tool interception on, or asked for background mode (400, OpenAI type "invalid_request_error"). The gateway cannot see a stored turn (SMA-769).` then `ERROR_REASON_RESPONSES_STATEFUL_NOT_SUPPORTED = 314;`. Run `buf format -w` on the file (from `contracts/`), then `moon run contracts:generate`.
- [ ] **Step 2: Write the failing tests.** In `paigasus-proto/src/error.rs`, add `"responses-stateful-not-supported"` to `EXPECTED_REASONS` after `"tool-interception-malformed-response"`. In `error.rs` (gateway), add the variant to the exhaustive test lists (the status table near line 486, the retryable match near line 591) and a test `the_stateful_refusal_names_its_field` (status 400, `code`, `param` `"previous_response_id"`). In `map-error.test.ts`, add `maps responses-stateful-not-supported to from-transport, not retryable` (status 400, presentation from the transport table, `retryable` `false`).
- [ ] **Step 3: Run** `cargo nextest run --locked -p paigasus-proto -p paigasus-gateway` and `moon run paigasus-sdk-ts:test paigasus-sdk-ts:typecheck` (use the project id that `moon query projects` shows for `ts/packages/paigasus-sdk`). Expected: FAIL.
- [ ] **Step 4: Implement** the `GatewayError` variant and its `parts()`/`retryable()` arms, and `[ErrorReason.RESPONSES_STATEFUL_NOT_SUPPORTED]: 'from-transport',` in `presentation.ts` with a `// SMA-769.` comment line.
- [ ] **Step 5: Run** the Step 3 commands, `python3 ci/error-registry/check.py --self-test`, `python3 ci/error-registry/check.py --single-site`, and `git diff --exit-code` after a second `moon run contracts:generate` (codegen drift). Expected: PASS and no drift.
- [ ] **Step 6: Commit** `feat(contracts): add the responses-stateful-not-supported error code (SMA-769)`.

### Task 13: The Responses usage reader (spec D14, § 8.2, AC 10)

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/http/responses_usage.rs`
- Modify: `src/adapters/http/usage.rs` (`StreamUsage::Responses`), `src/adapters/http/mod.rs`

**Interfaces:**
- Consumes: Task 10 (`StreamUsage`, `BodyUsage`).
- Produces:
  - `responses_usage::ResponsesUsageScanner` with `new()`, `feed(&mut self, chunk: &[u8])`, `finish(&mut self)`, `completion_records(&self) -> u64`, `reported_total(&self) -> Option<u64>`.
  - `responses_usage::non_stream_usage(body: &[u8]) -> BodyUsage`.
  - `responses_usage::request_tokens_estimate(v: &Value, body_len: usize) -> u64`.

- [ ] **Step 1: Write the failing tests** in `responses_usage.rs`:
  - `a_completed_event_reports_its_total`: one `response.completed` with `response.usage.total_tokens: 42` gives `Some(42)`; `response.incomplete` and `response.failed` too; the last value wins.
  - `a_terminal_event_over_64_kib_is_read` and `a_terminal_event_over_1_mib_is_read` (AC 10): `instructions` padding of 70 KiB and of 2 MiB before `usage`, fed in 1000-byte chunks; the scanner holds no more than its fixed state (assert a `#[cfg(test)] fn held_bytes(&self) -> usize` stays under 1024).
  - `usage_keys_elsewhere_do_not_count`: `usage.total_tokens` inside `response.metadata`, inside an output item, or at the top level of a `response.output_text.delta` record is ignored; a `type` key after the `response` key still decides (the type is read when the record ends).
  - `strings_with_braces_and_escapes_do_not_confuse_the_scan`: `"}"`, `"\""` and `"\\"` inside strings before `usage`.
  - `other_records_over_64_kib_are_skipped` and `one_token_per_data_record`.
  - `the_body_reports_usage_or_estimates`: `usage.total_tokens` wins; without it, the estimate counts `output[].content[].text`, `output[].arguments` and `output[].input` bytes by the chat rule.
  - `the_request_estimate_follows_the_chat_rule`: `instructions`, a string `input`, and per input item `content` (string or `input_text` parts), `arguments`, `input` and `output`; a non-object item gives the fallback.
- [ ] **Step 2: Run** `SVC`. Expected: FAIL.
- [ ] **Step 3: Implement** the scanner as a byte-level JSON key scan per data record: track depth, the key path for depth 1 to 3, string and escape state, and capture a number at exactly `/response/usage/total_tokens` and a short string (at most 64 bytes) at `/type`. At the end of a record, keep the number only when the type is `response.completed`, `response.incomplete` or `response.failed`. Keep the SSE line and record handling of `UsageScanner` (CR, LF, CRLF, `data:` prefix, blank line), but never buffer a record.
- [ ] **Step 4: Run** `SVC` and `LINT`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): read responses usage with no record size limit (SMA-769)`.

### Task 14: The `/v1/responses` route (spec D3, D4, D5, D9, § 7.2, § 8, § 9, § 10.6, AC 1–3)

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/http/responses.rs`, `tests/responses_proxy.rs`
- Modify: `src/adapters/http/dto.rs`, `src/adapters/http/mod.rs` (mount the route), `tests/support/mod.rs` (the mock serves `POST /v1/responses` with the same handler), `ci/error-registry/check.py` (`MANIFEST`)

**Interfaces:**
- Consumes: Tasks 6, 10, 11, 12, 13.
- Produces:
  - `dto::ResponsesRequest { model: Option<String>, input: Option<Value>, stream: Option<bool>, background: Option<bool>, previous_response_id: Option<Value>, conversation: Option<Value>, prompt: Option<Value>, #[serde(flatten)] extra: Map<String, Value> }`.
  - `responses::RESPONSES_TERMINAL_SSE_ERROR: &str` = the D13 literal of the Global Constraints.
  - `responses::RESPONSES: Endpoint` (`name: "responses"`, `route: UpstreamRoute::Responses`, `format: Format::OpenAiResponses`, `terminal: RESPONSES_TERMINAL_SSE_ERROR`, `limits` = `ti.limits` with `max_record_bytes` and `max_held_bytes` from `state.responses`) and `responses::create_response(State, Option<Extension<CallerContext>>, EnvelopeBytes) -> Response`.

- [ ] **Step 1: Write the failing unit tests** in `responses.rs`: `the_responses_terminal_frame_carries_a_registered_code` (strip the leading blank lines and the `event: error` line, parse the `data:` JSON, `ErrorReason::from_wire_reason(code)` is `Some`, `type` is `"error"`); `the_terminal_frame_is_its_own_record_after_a_partial_one`; `stateful_fields_are_refused_only_when_non_null` (Review Focus 2: `null` and `false` pass; with interception on each of the four D3 fields refuses with its `param`; `background: true` refuses with interception on and off; with interception off the D3 fields pass); `input_must_be_a_string_or_an_array` (`"input": 1` is `InvalidRequestSchema`; a body that is not a JSON object is a 400 as in chat).
- [ ] **Step 2: Write the failing service tests** in `tests/responses_proxy.rs` (use `MockOpenAi` and the helpers of `tool_interception.rs` and `limits_http.rs`): `the_route_is_404_when_disabled`; `the_route_needs_a_credential`; `the_request_goes_to_the_responses_path` (recorded `path == "/v1/responses"`, body bytes unchanged, no caller header upstream); `the_body_limit_is_the_responses_limit` (a 2 MiB body passes, a body over `max_request_bytes` is a 413 in the envelope); `rate_limit_and_budget_refuse_with_429`; `stateful_requests_are_refused_before_admission` (the refusal uses no quota: a following request is still admitted with a limit of one); `background_is_refused_with_interception_off`; `streaming_obeys_stream_enabled`; `a_non_stream_answer_is_charged_its_usage`; `a_stream_is_charged_its_usage` and `a_terminal_event_over_1_mib_is_charged_with_interception_on_and_off` (Review Focus 5 and AC 10; with interception on, the terminal over 16 MiB case asserts the charge only); `a_non_stream_blocked_call_is_rewritten_with_the_header` (`paigasus-tool-decisions` present, `output[i]` is the completed message); `a_malformed_body_is_a_502_under_block`; `a_streamed_blocked_call_never_reaches_the_client` (no `CANARY` byte, the § 6.5 events present); `an_upstream_error_mid_hold_ends_with_the_d13_frame`; `a_non_2xx_answer_is_forwarded_with_no_charge`; `client_abort_cancels_upstream` (as the chat test).
- [ ] **Step 3: Run** `SVC`. Expected: FAIL.
- [ ] **Step 4: Implement.** `parse` reads the DTO from a copy of the body, refuses D4 always and D3 when `interception` is `true`, in the order `background`, `previous_response_id`, `conversation`, `prompt`, `input` (`item_reference`), and computes the request estimate with Task 13's function; `model` is `"-"` when absent. D3 and D4 run inside `parse`, which the pipeline calls before `Limits::admit`. The request body goes upstream unchanged (D5: no `store` rewrite). Add the manifest row to `ci/error-registry/check.py`: `("rs/crates/services/paigasus-gateway/src/adapters/http/responses.rs", "emits", "the_responses_terminal_frame_carries_a_registered_code", "the D13 terminal SSE error frame of /v1/responses")`.
- [ ] **Step 5: Run** `SVC`, `LINT`, `python3 ci/error-registry/check.py --self-test` and `--single-site`. Expected: PASS. Run every existing service test file unchanged (`SVC` covers it). Expected: PASS.
- [ ] **Step 6: Commit** `feat(rs): serve /v1/responses with tool-call interception (SMA-769)`.

### Task 15: The `endpoint` log field and metric label (spec § 8.1)

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/pipeline.rs`, `src/application/charge_guard.rs`, `tests/metrics.rs` (a new test only), `docs/ops/RUNBOOK-observability.md`, and the drift files that need it under `ops/observability/` (`prometheus/rules/gateway.rules.yml`, `prometheus/rules/tests/gateway.test.yml`, `grafana/dashboards/gateway.json`)

**Interfaces:**
- Consumes: `Endpoint.name` (Task 10, Task 14).
- Produces: `gateway_upstream_requests_total{status_class, endpoint}` with `endpoint` in `{"chat_completions", "responses"}`; the `"chat completion proxied"` and `"chat completion metered"` lines carry `endpoint = <name>` (the message strings do not change, so existing log queries keep working); `ChargeGuard::set_endpoint(&mut self, endpoint: &'static str)` (default `"chat_completions"`).

- [ ] **Step 1: Write the failing tests:** in `tests/metrics.rs`, `the_upstream_counter_carries_the_endpoint` (one chat and one responses call give `endpoint="chat_completions"` and `endpoint="responses"`); in `charge_guard.rs`, extend a new test `the_metered_line_names_the_endpoint`.
- [ ] **Step 2: Run** `SVC`. Expected: FAIL.
- [ ] **Step 3: Implement** `record_upstream_call(status, started, endpoint)`. Update the `gateway_upstream_requests_total` row of the RUNBOOK labels table (`status_class`, `endpoint`). The rules and the dashboard sum over every label today; change them only if `repo:observability-drift` or `repo:promtool` asks for it, and add an `endpoint` label to the promtool test input series so the test reads like production.
- [ ] **Step 4: Run** `SVC`, `LINT`, `moon run repo:observability-drift repo:promtool`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): label upstream calls and request logs with the endpoint (SMA-769)`.

### Task 16: Real-SDK conformance for the Responses goldens (spec § 10.2)

**Files:**
- Modify: `py/packages/paigasus-gateway-conformance/tests/test_golden_streams.py`

**Interfaces:**
- Consumes: the Task 9 manifest entries and golden files.
- Produces: nothing for later tasks.

- [ ] **Step 1: Write the failing tests.** Make `Case` hold the new keys with `NotRequired`. Rename `test_manifest_lists_six_goldens` to `test_manifest_lists_ten_goldens` (`len(MANIFEST) == 10`; for `"responses"` entries also `len(notices) == len(notice_positions)`). Add `test_responses_stream_helper_sees_only_allowed_calls(case)` (`client.responses.stream(model="m", input="hi")`, `get_final_response()`: client-run items of the output equal the manifest `allowed` by `(position, id, name, arguments)`; each notice is the text of the message item at its `notice_position`, with id `msg_paigasus_<position>`; `final.status == finish_reasons["0"]`; no `CANARY` in the body) and `test_responses_raw_stream_sees_only_allowed_calls(case)` (`client.responses.create(model="m", input="hi", stream=True)`: every `response.output_item.done` item and the terminal `output` give only allowed calls; the repeated `sequence_number` of § 6.5 raises nothing).
- [ ] **Step 2: Run** from the repo root: `moon run paigasus-gateway-conformance-py:test`. Expected: PASS (the Task 9 goldens exist).
- [ ] **Step 3: Prove it bites.** Change one character of one Responses `notices` string in `tests/golden/manifest.json` (mark the edit), run the command again, and see the responses test FAIL. Delete the marked edit and run green again.
- [ ] **Step 4: Commit** `test(py): check the responses goldens through openai-python 3.48.0 (SMA-769)`.

### Task 17: Operator notes and the full gate run (spec § 9, § 11, AC 11)

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/gateway.toml.example` (the `[responses]` section text), `rs/crates/libs/paigasus-gateway-core/README.md` (the Responses format in its format list), `docs/superpowers/specs/2026-11-14-sma-769-responses-ingress-design.md` (an "As built" list only if the build differs from the spec)

**Interfaces:**
- Consumes: every earlier task.
- Produces: the branch, ready for review.

- [ ] **Step 1: Write the operator notes** in the `[responses]` section of `gateway.toml.example` (the gateway service has no README; see the report to the coordinator): D5 retention (with `store` true, OpenAI keeps the output, a blocked call included, for its retention period); the hosted-execution limit of § 6.2 (a hosted `shell_call` or a server `tool_search_call` runs on the provider even when the report says `block`; the decision log must not count it as a prevented action); the four `[responses]` keys and the 404 rollback. Write them in Simplified Technical English.
- [ ] **Step 2: Chart check.** The Helm chart does not deploy the gateway backend (`charts/paigasus/values.yaml`, the `zones.gateway.backend` comment) and holds no gateway config key today. Confirm with `grep -rn "tool_interception\|stream_enabled" charts/`. Expected: no output, so no chart change. If the grep finds a key, add `[responses]` beside it and follow `charts/CLAUDE.md`.
- [ ] **Step 3: Crate gates.** Run `CORE`, `SVC`, `LINT`, `GEN` (then `git status --short` is empty), `cargo nextest run --locked -p paigasus-gateway-core --test golden` (no update).
- [ ] **Step 4: Full graph.** Run the `moon ci … --base origin/main --include-relations` command between the `ci-targets` markers of the root `CLAUDE.md`, with the shims on `PATH`. Read any red with the root `CLAUDE.md` diagnosis procedure (Step 0: copy `.moon/cache/ciReport.json` and the task state first). A red that comes from this Mac's bash split (`mapfile`, `declare -A`, `local -n`, a 512-byte pipe) is not a finding: re-run that gate with the right bash as `CLAUDE.md` says, and record both results.
- [ ] **Step 5: Commit** `docs: add the responses operator notes (SMA-769)`, with the gate results in the commit body (one line per gate that needed a second bash).

---

## Self-Review

1. **Spec coverage.** § 1–2 and AC 1: Tasks 11, 14. § 3.3, § 10.5, AC 6, AC 9: Task 1 (recordings and results), Task 9 (replay). D1, D16, § 6.1–§ 6.3, § 7.1: Task 3. D2, § 5: Tasks 2, 4. § 6.4 R1–R16: Task 4 (rules), Task 8 (quirks), Task 9 (malformed fixtures). D10–D13, § 6.5, § 6.6: Tasks 5, 6. § 7.2, D17: Tasks 5, 6. D3, D4, D5, D9, § 9 table: Task 14. D6, D7, § 8.1: Task 10; `endpoint`: Task 15. D8, D15: Task 11 (config), Task 14 (limits in the descriptor). D14, § 8.2, AC 10: Tasks 13, 14. D18, AC 3: Tasks 4, 6, 9. § 10.1, AC 4, AC 7: Task 7. § 10.2: Task 16. § 10.3, AC 5: Task 8. § 10.4: Task 9. § 10.6: Task 14. § 11: Task 12 (error chain), Task 15 (metric label), Tasks 11 and 17 (config, chart). AC 8: Global Constraints, the golden runs in Tasks 2, 6, 9, and Task 10 Step 3. AC 11: Task 17.
2. **Step scan.** Each test step names its tests and the values they check. Each code step names its signatures and the spec values. Task 7 originally had a test that cannot be written (a panic on an unknown variant of a `non_exhaustive` enum from outside the crate). It is replaced by a code-review check in Step 4, and the panic arm is stated in the Interfaces block.
3. **Type consistency.** `display_name()` (Task 2) is used in Tasks 4, 6. `ResponsesDoc`, `parse_strict_responses`, `check_numbers_responses` (Task 3) are used in Tasks 5, 6. `assembly::tool_call` (Task 4) is used in Task 5. `rewrite::*` (Task 5) is used in Task 6. `Step::Terminal`, `StepError::AfterEnd`, `closing_on_abort` are defined and used in Task 6. `StreamUsage` (Task 10) gets its `Responses` arm in Task 13. `Endpoint.name` (Task 10) is the label in Task 15. `AppState.responses` (Task 11) is read by `RESPONSES.limits` (Task 14). `GatewayError::ResponsesStatefulNotSupported { param }` (Task 12) is used in Task 14.
4. **Review Focus.** Five lines, each with its test: 1 in Task 6 (`unframed_writes_a_safe_event_line`), 2 in Task 14 (`stateful_fields_are_refused_only_when_non_null`), 3 in Task 5 (`an_item_not_completed_is_malformed`), 4 in Task 9 (`allow_all_is_payload_identical_for_every_conforming_fixture` with `provider_only`), 5 in Tasks 13 and 14 (the over-1-MiB charge tests).
5. **Proportion.** The plan holds no function bodies. The only literal blocks are spec values (the D13 frame, the error comment, the event and quirk names).

Decisions this plan makes where the spec is silent (the coordinator can change them before execution):

- `[responses] max_record_bytes` and `max_held_bytes` are config keys, next to `max_request_bytes` (D15 names only `max_request_bytes` as a key).
- A non-stream client-run item that is not `completed` is `Malformed("item_not_completed")` (§ 6.5 non-stream does not say).
- The stateful refusal carries `param` = the refused field; the SDK presentation is `from-transport`.
- The log messages keep their chat wording and get an `endpoint` field.
- The provider-run report `name` is the item `type`.
- The operator notes go into `gateway.toml.example`, because the gateway service has no README.
