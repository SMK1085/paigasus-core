<!-- moon-diagnosis:ok -->
# SMA-750 Anthropic Messages Ingress: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The gateway serves `POST /v1/messages` (stream and non-stream), `POST /v1/messages/count_tokens` and `GET /v1/models` against a direct Anthropic upstream, with the authentication, limits, budget charge and tool-call interception of the OpenAI routes.

**Architecture:** A new `AnthropicClient` sits beside `OpenAiClient`. Both build their `reqwest::Client` through one shared helper with no redirects. `/v1/messages` runs through the existing `pipeline::proxy` with a new `Upstream` field on `Endpoint` and a `Forward` value (the `anthropic-*` headers and the query). The two pass-through routes use one small handler each. A response layer on the Anthropic route group re-renders every gateway error into the Anthropic envelope. CORE gets the D12 `ping` keep-alive, the D17 upstream-error terminal, the D18 data keys and the wider SMA-758 generator.

**Tech Stack:** Rust (edition 2024, rust 1.95), axum, reqwest 0.13, serde_json, proptest, cargo nextest. Python 3 standard library for `capture.py` and `measure.py`.

**Spec:** `docs/superpowers/specs/2026-11-25-sma-750-anthropic-messages-ingress-design.md`. It extends the SMA-749 and SMA-769 specs. Read the parts that it cites. The spec is the source of truth. Do not re-open its decisions.

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-750-anthropic-ingress`, branch `feature/sma-750-gateway-anthropic-messages-ingress`. Run every command in the foreground.
- Put `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` on its own shell line before a cargo, moon or python command. Never put it on the same line as a git command. Run cargo from `rs/` and always pass `--locked`.
- Rust crates use edition 2024 and rust-version 1.95. Do not change either.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python and TOML).
- Commits: conventional, with a workspace scope, lower case after the scope, and the key at the end: `feat(rs): … (SMA-750)`, `test(rs): …`, `docs(rs): …`, `docs: …`. Body lines are at most 100 characters. No body line starts with `#NNN`. End each message with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never use `--no-verify`.
- `cargo clippy --locked --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass at the end of every task. Dead code is a hard error. A new item that only a later task uses is `pub` in a `pub mod` (the crate is a library, so a `pub` item is not dead code). Never add `allow(dead_code)`.
- A src file that spells a registry code gets a `ci/error-registry/check.py` MANIFEST row and a membership test in the same task. A new HTTP extractor definition file gets a `ci/http-extractor/check.py` ALLOW row. The new handlers use `EnvelopeBytes`, `HeaderMap`, `OriginalUri`, `State` and `Extension` only (no `Bytes`, `Json`, `Query` or `Path` in a parameter list).
- The operator key is in `~/.config/paigasus/anthropic-api-key`. A live step reads it only as `--key-file ~/.config/paigasus/anthropic-api-key` (the script reads the file) or as `GATEWAY_UPSTREAM__ANTHROPIC__API_KEY="$(cat ~/.config/paigasus/anthropic-api-key)"` on the command that needs it. Never print it, log it, echo it, or write it to a file or a fixture. Never read any `.env*` file. Run Python with `python3 -I`.
- Do not install host software. Do not edit `.github/CODEOWNERS`. No new crate (no `members` change). Never name a file with a Windows reserved base name. Never put a new file under a directory named `build/`.
- The OpenAI routes do not change. Every existing core golden is byte-identical. Every existing service test passes. Edits to existing test files are limited to: new test functions, a new field in an `AppState { … }` or `AuthState { … }` literal, and the Task 4 type renames.
- A spec rule is never loosened in code. If a recording or a test shows that real traffic breaks a rule, stop and report to the coordinator.
- Exact values (copy them, do not retype them):
  - D13 frame, as a Rust literal: `"\n\nevent: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"upstream stream error\",\"code\":\"upstream-error\"}}\n\n"`.
  - D12 keep-alive frame: `b"event: ping\ndata: {\"type\":\"ping\"}\n\n"`.
  - D7 envelope: `{"type":"error","error":{"type":<anthropic type>,"message":<text>,"code":<registry code>},"request_id":<paigasus-request-id>}`. The 404/405 fallback has no `code` key.
  - § 6.1 table: 400 `invalid_request_error`, 401 `authentication_error`, 403 `permission_error`, 404 `not_found_error`, 405 `invalid_request_error`, 413 `request_too_large`, 429 `RateLimited` `rate_limit_error`, 429 `BudgetExhausted` `billing_error`, 500/502/503 `api_error`, 504 `timeout_error`.
  - `MissingBearer` message on the Anthropic routes: `Missing credentials. Send the x-api-key header or an Authorization: Bearer header.`
  - `[upstream.anthropic]` defaults: `base_url = "https://api.anthropic.com"`, `default_version = "2023-06-01"`, `non_stream_timeout_secs = 600`; the key only from `GATEWAY_UPSTREAM__ANTHROPIC__API_KEY`.
  - `[messages]` defaults: `enabled = false`, `max_request_bytes = 33_554_432`, `max_record_bytes = 8_388_608`, `max_held_bytes = 33_554_432`, `max_hold_secs = 600`. Task 1 can change the last three (D15); the spec § 15 then holds the values to use.
  - Endpoint labels: `messages`, `messages_count_tokens`, `models`. The decision-log ingress label stays `anthropic_messages`.
  - Budget `retry-after` on the Anthropic routes: the seconds until `resets_at_unix`, at least `61`.
  - Fixture scrub (D21): kept headers `content-type`, `request-id`, `retry-after`, `x-should-retry`; the fixed `request-id` value `req_paigasus_fixture`.
  - Request estimate floor: `min(ceil(body_len / 4), 32_768)`; a non-text block counts `85`.
  - Properties P1 and P2: `ProptestConfig::with_cases(10_000)`.

Commands used by several tasks (from `rs/`):

- `CORE`: `cargo nextest run --locked -p paigasus-gateway-core`
- `SVC`: `cargo nextest run --locked -p paigasus-gateway`
- `LINT`: `cargo clippy --locked --workspace --all-targets -- -D warnings && cargo fmt --check`
- `GOLDEN`: `cargo nextest run --locked -p paigasus-gateway-core --test golden` (no `UPDATE_GOLDEN`; the goldens must not change).
- `REG`: from the repo root, `python3 -I ci/error-registry/check.py --self-test && python3 -I ci/error-registry/check.py --single-site && python3 -I ci/http-extractor/check.py`.

Shared paths: `GW` = `rs/crates/services/paigasus-gateway`, `CORE_DIR` = `rs/crates/libs/paigasus-gateway-core`, `REC` = `CORE_DIR/tests/fixtures/anthropic/recorded`, `GWFIX` = `GW/tests/fixtures/anthropic`. `$SCRATCH` is your scratchpad directory.

## Review Focus

1. **Several `anthropic-*` headers, mixed case and repeated values.** A caller can send `Anthropic-Beta` twice with different values, or `anthropic-version` together with a second copy. Expected: every value of every `anthropic-*` name reaches the upstream in the order received; `default_version` is added only when no `anthropic-version` value is present. Task 6 pins it in `forward_keeps_every_anthropic_value_in_order`.
2. **An empty `x-api-key` next to a valid `Authorization: Bearer`.** Expected: the presence of `x-api-key` decides; an empty value is `MissingBearer` (401) and the bearer is not used, so a stale or broken key never silently falls back to another identity. Task 7 pins it in `an_empty_x_api_key_is_missing_even_with_a_bearer`.
3. **A query string that looks like a URL.** `?beta=true&next=http://evil.example/@x/../v1/models` must not change the upstream host or path (I3). Expected: the mock sees path `/v1/messages`, the same host, and the raw query unchanged. Task 6 pins it in `the_query_cannot_change_the_host_or_the_path`.
4. **Usage fields that are `null`, absent, or objects.** A real `message_delta.usage` can carry `server_tool_use: {…}` and `null` cache fields. Expected: only the four D10 number fields count; `null` is absent; an object field is ignored; the record still counts as one data record. Task 9 pins it in `null_and_object_usage_fields_are_ignored`.
5. **A non-2xx answer on a pass-through route.** `count_tokens` with an unknown model gives a 404 from Anthropic; a bad operator key gives a 401. Expected: the body, the status and the upstream `content-type` arrive unchanged, the Anthropic re-render layer does not touch them, and the charge is 0. Task 11 pins it in `a_pass_through_error_is_forwarded_unchanged`.

---

### Task 1: Real Anthropic recordings, the D21 scrub, and the measurements (spec § 10.1, D15, D18, D21)

**Files:**
- Create: `REC/capture.py`, `REC/measure.py`, `REC/README.md`, the kept `REC/*.sse` streams
- Create: the kept `GWFIX/*` files (non-stream bodies, error bodies, `count_tokens`, `models`, and one `<stem>.response.json` per recording), `GW/tests/anthropic_fixtures.rs`
- Modify: `CORE_DIR/tests/fixtures.rs` (`recorded_streams_pass_unchanged`, one new test), `docs/superpowers/specs/2026-11-25-sma-750-anthropic-messages-ingress-design.md` (append § 15)

Decision: the `.sse` streams live in CORE (the CORE replay reads them). Everything a service test reads lives in `GWFIX`. A service test never reads a CORE file, because the gateway's Moon `fileGroups.upstreams` covers only CORE `src/**` (A6 asserts it), so a cross-crate fixture would not make the gateway test affected.

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `REC/<scenario>.sse` streams and `GWFIX/<scenario>.body.json` / `GWFIX/<scenario>.response.json` files. A `.response.json` is `{"status": <int>, "headers": {<kept header>: <value>}}`.
  - Spec § 15 lines that Tasks 3, 5 and 12 read: the D15 sizes and times, the D18 data subtrees, and the event, block and delta type list.
  - `GW/tests/anthropic_fixtures.rs` with `fn scrub_problems(name: &str, text: &str) -> Vec<String>` (test-local).

- [ ] **Step 1: Write `capture.py`.** Standard library only, in the shape of `CORE_DIR/tests/fixtures/openai/recorded/capture.py`. Options: `--key-file PATH` (required for every live mode; read with `.expanduser().read_text().strip()`; never printed), `--dry-run` (writes each request body to `<out>/<scenario>.request.json`, sends nothing, does not read the key), `--list-models`, `--model`, `--thinking-model`, `--only NAME[,NAME]`, `--out DIR`. It sends `x-api-key`, `anthropic-version: 2023-06-01`, and `content-type: application/json`. Scenarios, names exactly: `text_stream`, `parallel_tool_use` (two tools, `tool_choice` any), `thinking_then_tool_use` (extended thinking, then a tool call), `large_tool_input` (a tool whose input is over 40 KB, for example a file `write` tool asked to write a long text), `server_tool_web_fetch` (the `web_fetch` server tool on a large public PDF; if the account cannot use it, `web_search`), `max_tokens_in_tool_use` (a small `max_tokens` that cuts a `tool_use` block), `non_stream_tool_use`, `prompt_too_long` (a body over the model context; expected 400), `rate_limited` (a burst of `count_tokens` calls, at most 200, that stops at the first 429), `count_tokens`, `models` (`GET /v1/models?limit=1000`), `structured_output` (two requests: the `output_format` form and the `output_config.format` form, each with a schema that has a property `Name`; record the status of each), `tool_input_examples` (a tool with `input_examples` that hold a key `Name`). For a feature that needs a beta header, use the beta value that Anthropic's current docs name, and write it in the README. Each stream records the arrival time of each chunk in `<scenario>.timing.json` (seconds from the request start, per chunk index). Any `event: error` record in any stream is kept as `mid_stream_error` evidence.
- [ ] **Step 2: Put the D21 scrub inside `capture.py`.** Before it writes a file: keep only the headers `content-type`, `request-id`, `retry-after`, `x-should-retry` (lower-case names); replace the `request-id` header value and every top-level `request_id` body value with `req_paigasus_fixture`; refuse to write (exit 1) when the output text contains `sk-ant-`. The decision to also replace the body `request_id` is this plan's (the spec names the header only).
- [ ] **Step 3: Write `measure.py`.** Standard library only. `python3 -I measure.py <dir>` prints one JSON object per `.sse` file: `event_types`, `block_types`, `delta_types` (sets), `max_record_bytes` per event type, `max_content_block_start_bytes`, and per `tool_use` block its input bytes and its seconds from `content_block_start` to `content_block_stop` (from `timing.json`). `python3 -I measure.py --self-test` runs one built-in stream and exits 1 when a field is wrong.
- [ ] **Step 4: Offline checks.** From `REC`: `python3 -I measure.py --self-test` (expected exit 0); `python3 -I capture.py --dry-run --model m --thinking-model m --out "$SCRATCH/dry"` (expected: one `.request.json` per scenario, each parses as JSON).
- [ ] **Step 5: Commit the scripts.** `git add REC/capture.py REC/measure.py`, commit `test(rs): add the anthropic capture and measure scripts (SMA-750)`.
- [ ] **Step 6: Live run (costs money; one run per scenario).** `python3 -I capture.py --key-file ~/.config/paigasus/anthropic-api-key --list-models` gives the model ids. Use the newest Sonnet model for `--model` and a model with extended thinking for `--thinking-model`. Then `python3 -I capture.py --key-file ~/.config/paigasus/anthropic-api-key --model <m> --thinking-model <t> --out "$SCRATCH/rec"`. A rejected request is a result: keep its body and status. Run a scenario again at most twice.
- [ ] **Step 7: Measure and amend the spec.** `python3 -I measure.py "$SCRATCH/rec"`. Append `## 15. Amendments after the measurements` to the spec. Do not edit any decision row. Add one line per item, `Measured 2026-11-NN: …`: (a) D15: the largest `content_block_start` bytes (server tool result) and the largest `tool_use` input bytes and hold seconds; if a value is over 50 % of the default `max_record_bytes`, `max_held_bytes` or `max_hold_secs`, state the new default (the next power of two in bytes, or the measured seconds times 2, rounded up to 60 s); (b) D18: which structured-output form Anthropic accepted (status of each), and the full list of user-data subtrees (at least `tools[].input_examples` and the accepted output-schema path); (c) the union of event, block and delta types; (d) each § 10.1 case that could not be recorded, as "not measured" with the reason. D9 and the Claude Code session are Task 14.
- [ ] **Step 8: STOP check.** If a recorded stream holds an event, block or delta type that CORE `anthropic/stream.rs` does not know (see spec § 3 last bullet), or a conforming stream that the replay of Step 10 fails, stop. Report the file names and the `measure.py` output to the coordinator. Do not start Task 3.
- [ ] **Step 9: Keep the files.** Copy each kept stream to `REC/<scenario>.sse` unchanged. Copy each `.body.json`, `.response.json` and the `count_tokens` and `models` answers to `GWFIX/`. Write `REC/README.md` in the shape of `openai/recorded/README.md`: date, models, scenarios, beta values used, what was rejected, the § 15 results, and the "Record again" commands.
- [ ] **Step 10: Write the tests.**
  - `GW/tests/anthropic_fixtures.rs`: `every_anthropic_fixture_is_scrubbed` (for every file in `GWFIX`: no `sk-ant-` substring; for every `.response.json`, the header keys are a subset of the four kept names and `request-id`, when present, is `req_paigasus_fixture`); `the_scrub_check_refuses_an_extra_header_and_a_key` (`scrub_problems` of an in-memory `.response.json` with `anthropic-ratelimit-requests-limit`, and of a text with `sk-ant-x`, each give a non-empty list).
  - `CORE_DIR/tests/fixtures.rs`: `recorded_streams_pass_unchanged` also replays `anthropic/recorded` with `Format::AnthropicMessages` and limits `max_record_bytes = 8_388_608`, `max_held_bytes = 33_554_432` (or the § 15 values); add `anthropic_recordings_hold_no_key` (no `sk-ant-` in any file of `anthropic/recorded`).
- [ ] **Step 11: Run** `CORE`, `SVC`, `LINT`. Expected: PASS.
- [ ] **Step 12: Commit.** `git add` the README, the kept files, the two test files and the spec. Commit `test(rs): add recorded anthropic answers and the d15 and d18 measurements (SMA-750)`.

### Task 2: SMA-758, the wider Anthropic generator (spec § 10.3, D14)

Independent of every other task. It may run in parallel with Task 1.

**Files:**
- Modify: `CORE_DIR/tests/support/streams.rs`, `CORE_DIR/tests/properties.rs`
- Modify only if the generator finds a defect of about one task: `CORE_DIR/src/anthropic/stream.rs` or `src/interceptor.rs`

**Interfaces:**
- Consumes: `support::driver::{drive, cut, DECIDERS, limits, block_named, allow_all, Outcome}`, `support::oracle::{check_i1, runners}`, `check_p1`, `check_all_deciders` (in `properties.rs`).
- Produces: `streams::Block::Thinking(String)` and the `AQuirk` variants `SplitPartialJson`, `PingInHeldBlock`, `MessageDeltaBeforeLastStop`. Nothing outside the tests.

- [ ] **Step 1: Widen the generator** in `streams.rs`. Add `Block::Thinking(String)`: `render_anthropic` writes `content_block_start` with `{"type":"thinking","thinking":""}`, one `thinking_delta`, one `signature_delta` with a fixed base64 string, and `content_block_stop`. Add it to the block strategy, so a scenario mixes parallel `Tool` blocks with `Text`, `Thinking` and `Server` blocks. Raise the block count from `0..5` to `0..7`. Add three `AQuirk` variants, each with one doc line that names the SMA-758 gap: `SplitPartialJson` (each `input_json_delta` of every tool is cut at odd byte offsets, including inside a JSON string escape such as `\"` and `é`; the joined input is unchanged), `PingInHeldBlock` (a `ping` event between two deltas of the first `Tool` block), `MessageDeltaBeforeLastStop` (the `message_delta` frame moves to just before the last `content_block_stop`). Add doc lines to the existing undocumented variants too (one line each, naming the deviation it produces).
- [ ] **Step 2: Write the failing properties** in `properties.rs`:
  - `anthropic_p1_and_p2_hold` stays in the `with_cases(10_000)` block and now runs over the wider generator (no code change in the test body).
  - New in the 2 000-case block: `anthropic_observe_mode_holds_for_many_quirks(a in anthropic_scenario_with_quirks(2..4))`. Under `limits(OnFailure::Allow)` and `block_named`: (1) `o.finished`; (2) `o.max_queued <= l.max_held_bytes + l.max_record_bytes`; (3) for each `(call, Decision::Block { .. })` in `o.decided`, the call's arguments do not occur in `o.out`; (4) the output data payloads that occur in the input occur in the same relative order as in the input (map each to its first unused input index; the indices increase). Add `streams::anthropic_scenario_with_quirks(n: std::ops::Range<usize>) -> impl Strategy<Value = AnthropicScenario>`.
  - New, outside proptest: `a_pass_through_interceptor_fails_anthropic_p1`. Render a fixed scenario with one `blocked_rm` tool; build `Outcome { out: input.clone(), decided: vec![], max_queued: 0, finished: true, metrics: vec![], aborted: false }`; assert `check_p1(Format::AnthropicMessages, &input, &o).is_err()`.
- [ ] **Step 3: Run** `cargo nextest run --locked -p paigasus-gateway-core --test properties`. Expected: the new tests fail to compile, then FAIL or PASS on the first real run. A real P1 or P2 failure on the conforming scenarios is a defect: if the fix is about one task, fix it in CORE and add the shrunk case as a unit test in the file you fix; if it is larger, stop and report to the coordinator.
- [ ] **Step 4: Prove the mutation still bites.** Temporarily change `check_all_deciders` to pass the input as the output for Anthropic (mark the edit `// MUTATION`), run `anthropic_p1_and_p2_hold`, see it FAIL, delete the marked edit, run green. Record the shrunk failing case in the commit body.
- [ ] **Step 5: Run** `CORE`, `GOLDEN`, `LINT`. Expected: PASS.
- [ ] **Step 6: Commit** `test(rs): widen the anthropic stream generator (SMA-758)`. The subject carries SMA-758, the body says "Part of SMA-750."

### Task 3: CORE changes D12, D17, D18 (spec D12, D17, D18, § 8)

**Files:**
- Modify: `CORE_DIR/src/protocol.rs`, `src/interceptor.rs`, `src/anthropic/stream.rs`, `src/sse.rs`, `src/json.rs`, `src/turn.rs`, `src/body.rs` (only if a body parse needs the D18 profile; it does not by default)

**Interfaces:**
- Consumes: the § 15 D18 list (Task 1).
- Produces:
  - `Protocol::keep_alive(&self) -> &'static [u8]` with a default body that returns `sse::KEEP_ALIVE`. `AnthropicStream` returns `sse::ANTHROPIC_PING`.
  - `pub(crate) const ANTHROPIC_PING: &[u8] = b"event: ping\ndata: {\"type\":\"ping\"}\n\n";` in `sse.rs`.
  - `Protocol::upstream_error_is_terminal(&self) -> bool` with default `false`; `AnthropicStream` returns `true`.
  - `Interceptor::abort_with(&mut self, terminal: Bytes) -> Vec<Action>` (private); `abort()` calls it with `self.terminal.clone()`.
  - `json::parse_strict_anthropic_request(bytes: &[u8]) -> Result<Value, Malformed>`; `turn::parse_value` uses it for `Format::AnthropicMessages`.

- [ ] **Step 1: Write the failing tests.**
  - `interceptor.rs`: `an_anthropic_hold_keeps_alive_with_a_ping_event` (an `AnthropicMessages` interceptor with an open `tool_use` block; `tick(t0 + keepalive)` emits exactly `ANTHROPIC_PING`; one per interval); `the_chat_keep_alive_is_still_a_comment` (the existing OpenAI test values, unchanged).
  - `interceptor.rs`: `an_anthropic_upstream_error_mid_hold_is_the_terminal_frame` (open `tool_use` block, then `event: error\ndata: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}\n\n`; the last action is `Emit` of `event: error\ndata: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}\n\n` with the payload bytes unchanged; `self.terminal` is not emitted; the closing frames of `closing_on_abort` come before it); `a_transport_failure_still_emits_the_terminal_frame` (`abort()` on the same state emits `self.terminal`); `the_chat_upstream_error_still_aborts` (an `OpenAiChat` error record mid-hold emits `self.terminal`, as before).
  - `json.rs`: `anthropic_request_data_subtrees_relax_the_key_rules`: `parse_strict_anthropic_request` accepts `{"tools":[{"name":"t","input_schema":{},"input_examples":[{"Name":1,"constructor":2}]}]}` and each § 15 output-schema path with `{"properties":{"Name":{},"ID":{}}}` under it; it refuses `{"Type":"x"}` at the top level and `{"__proto__":1}` inside a data subtree. `the_chat_key_rules_do_not_change`: `parse_strict` of `{"tools":[{"input_examples":[{"Name":1}]}]}` keeps today's result.
  - `turn.rs`: `an_anthropic_request_with_user_data_keys_parses`: `parse_request(Format::AnthropicMessages, …)` of a request with those subtrees gives `ToolTurn::Parsed`.
- [ ] **Step 2: Run** `CORE`. Expected: FAIL.
- [ ] **Step 3: Implement.** D12: `tick` emits `self.proto.keep_alive()`. D17: in `Interceptor::item`, the `Ok(Step::UpstreamError(frame))` arm calls `abort_with(reframed)` when `self.proto.upstream_error_is_terminal()` and the queue is not empty or a hold is open; `reframed` is `event: error\n`, then one `data: <line>\n` per line of `frame.payload`, then `\n`. D18: add `Mode::AnthropicTop` and the modes it needs to the strict visitor. The top-level keys of the § 15 list become `Mode::Data` (or the nested path, for example `output_config` → `format`); a key `input_examples` below `tools` becomes `Mode::Data`; every other key keeps the `Mode::Envelope` rules. Add a doc comment that names the § 15 line as the source of the list. Response records and bodies keep `parse_strict`.
- [ ] **Step 4: Run** `CORE`, `GOLDEN`, `LINT`. Expected: PASS, goldens unchanged.
- [ ] **Step 5: Commit** `feat(rs): add the anthropic ping keep-alive, the upstream error terminal and the data keys (SMA-750)`.

### Task 4: The shared upstream helper (spec D2, D16, § 5)

**Files:**
- Create: `GW/src/adapters/upstream.rs`
- Modify: `GW/src/adapters/mod.rs` (`pub mod upstream;`), `src/adapters/openai/client.rs`, `src/adapters/openai/mod.rs`, `src/adapters/http/{pipeline,relay,interception,chat,error}.rs`, `tests/openai_egress.rs` (renames only)

**Interfaces:**
- Consumes: nothing.
- Produces (all `pub` in `adapters::upstream`):
  - `enum Provider { OpenAi, Anthropic }` with `Display` (`OpenAI`, `Anthropic`) and `fn ca_key(self) -> &'static str` (`upstream.openai.extra_ca_bundle_path`, `upstream.anthropic.extra_ca_bundle_path`).
  - `type UpstreamByteStream = BoxStream<'static, Result<Bytes, reqwest::Error>>` (was `OpenAiByteStream`).
  - `enum UpstreamResponse { Full { status, body }, Stream { status, stream } }` (was `ChatResponse`; Task 6 adds `head`).
  - `enum UpstreamError { Build { provider, source }, Connect { provider, source }, Timeout { provider, source }, Transport { provider, source }, Redirect { provider, status: StatusCode }, CaBundle { provider, path: String, source } }` (was `OpenAiError`). The OpenAI texts are byte-identical to today's: `failed to build the OpenAI HTTP client — this can also mean the platform trust store contains no parseable certificates`, `failed to connect to the OpenAI upstream`, `the OpenAI upstream request timed out`, `transport error talking to the OpenAI upstream`, `failed to load upstream.openai.extra_ca_bundle_path "<path>"`. `Redirect` renders `the {provider} upstream answered with a redirect ({status}), which the gateway does not follow`.
  - `fn build_client(provider: Provider, extra_ca_bundle_path: Option<&str>, connect_timeout: Duration, read_timeout: Duration) -> Result<reqwest::Client, UpstreamError>` and `pub(crate) fn build_client_with_control(…, control_build_ok: impl FnOnce() -> bool)`. The base builder sets `redirect(reqwest::redirect::Policy::none())`.
  - `fn classify(provider: Provider, e: reqwest::Error) -> UpstreamError` (timeout, connect, transport).
  - `GatewayError::from(UpstreamError)`: `Timeout` → `UpstreamTimeout`; every other variant → `UpstreamUnavailable`.

- [ ] **Step 1: Write the failing tests** in `upstream.rs`: `the_openai_error_texts_are_byte_unchanged` (the five texts above, each built from a real or minted error); `the_anthropic_ca_errors_name_the_anthropic_key` (`CaBundle` and the DER-invalid path with `Provider::Anthropic` name `upstream.anthropic.extra_ca_bundle_path` and never `upstream.openai`); `a_redirect_is_not_followed` (a local axum server answers `307` with `Location` to a second local server; a client from `build_client` gets status 307 and the second server counts 0 requests). Move the CA-bundle and attribution tests of `client.rs` to `upstream.rs` unchanged in their assertions. In `error.rs`: `a_redirect_maps_to_502`. In `client.rs`: `an_openai_redirect_is_a_502_error` (`OpenAiClient::send` against a 307 mock returns `UpstreamError::Redirect`).
- [ ] **Step 2: Run** `SVC`. Expected: FAIL.
- [ ] **Step 3: Implement.** Move `base_builder`, `attribute_build_failure`, `describe_error`, the two typed sources and the CA load into `upstream.rs`, with the config key and the client name taken from `Provider` in every error and log text (the two `tracing` texts keep their OpenAI wording for `Provider::OpenAi`). `OpenAiClient::new` calls `build_client(Provider::OpenAi, …)`. `OpenAiClient::send` returns `Err(UpstreamError::Redirect { .. })` for a `3xx` status. Rename the three types at every use site (`grep -rn "ChatResponse\|OpenAiByteStream\|OpenAiError" rs/crates/services/paigasus-gateway`). `openai/mod.rs` re-exports `OpenAiClient` and `UpstreamRoute` only.
- [ ] **Step 4: Run** `SVC`, `LINT`. Expected: PASS. `tests/openai_egress.rs` changes only by the renames (`git diff --stat` shows it).
- [ ] **Step 5: Commit** `refactor(rs): share the upstream client builder and stop following redirects (SMA-750)`.

### Task 5: The `[upstream.anthropic]` and `[messages]` config (spec D3, D15)

**Files:**
- Modify: `GW/src/config.rs`, `GW/src/runtime.rs` (the three `detector_settings` calls), `GW/gateway.toml.example`

**Interfaces:**
- Consumes: the § 15 D15 values (Task 1).
- Produces:
  - `config::AnthropicConfig { base_url: String, #[serde(skip_serializing)] api_key: SecretString, extra_ca_bundle_path: Option<String>, default_version: String, non_stream_timeout_secs: u64 }` with `#[serde(deny_unknown_fields)]` and serde defaults for every field except `api_key`. `UpstreamConfig.anthropic: Option<AnthropicConfig>` (`#[serde(default)]`).
  - `config::MessagesConfig { enabled: bool, max_request_bytes: usize, max_record_bytes: usize, max_held_bytes: usize, max_hold_secs: u64 }`, `#[serde(default, deny_unknown_fields)]`, the Global Constraints defaults; `MessagesConfig::validate(&self) -> Result<(), String>`; `GatewayConfig.messages: MessagesConfig`.
  - `DetectorsConfig.max_scan_bytes: Option<usize>` (`None` is the default) and `GatewayConfig::default_detector_scan_bytes(&self) -> usize`; `ToolPolicyConfig::detector_settings(&self, default_scan_bytes: usize) -> DetectorSettings`.

- [ ] **Step 1: Write the failing tests** in `config.rs`: `anthropic_and_messages_defaults_match_the_spec`; `the_anthropic_key_loads_from_the_environment_only` (`GATEWAY_UPSTREAM__ANTHROPIC__API_KEY` loads; the serialized config has no `api_key` key); `an_empty_anthropic_key_fails` (message names `upstream.anthropic.api_key`); `an_invalid_default_version_fails` (`"2023\n06"` fails, naming `upstream.anthropic.default_version`; the check is `HeaderValue::from_str`); `messages_without_the_anthropic_upstream_fails` (`[messages] enabled = true` and no `[upstream.anthropic]`); `messages_validation_names_the_key` (each zero value; record over held; `max_hold_secs` over 86 400; `non_stream_timeout_secs = 0`); `the_anthropic_ca_path_follows_the_openai_rule` (empty and padded paths fail, naming `upstream.anthropic.extra_ca_bundle_path`); `an_unknown_messages_key_fails`; `the_detector_default_covers_every_route_cap` (default config: 16 777 216, as today; with `[messages] enabled = true`: 33 554 432; an explicit `max_scan_bytes` wins).
- [ ] **Step 2: Run** `SVC`. Expected: FAIL.
- [ ] **Step 3: Implement.** `default_detector_scan_bytes` is the maximum of `max_request_bytes`, `responses.max_request_bytes` (counted whether `[responses]` is on or off, so today's default does not drop) and `messages.max_request_bytes` when `[messages]` is on. Update the doc comment on `max_scan_bytes` and the existing `detectors_defaults_pass` assertion source (the value stays 16 777 216). Validation keeps `[upstream.openai]` required. Add commented `[upstream.anthropic]` and `[messages]` sections to `gateway.toml.example`, after `[upstream.openai]` and `[responses]`, each key with its default, its env name and one line of meaning; the CA key says it ADDS to the trust store. Task 13 adds the operator notes.
- [ ] **Step 4: Run** `SVC`, `LINT`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): add the [upstream.anthropic] and [messages] config (SMA-750)`.

### Task 6: `AnthropicClient` (spec § 5, D5, D8, D16, I2, I3)

**Files:**
- Create: `GW/src/adapters/anthropic/mod.rs`, `GW/src/adapters/anthropic/client.rs`
- Modify: `GW/src/adapters/mod.rs`, `src/adapters/upstream.rs` (`ResponseHead`, `head` field), `src/adapters/openai/client.rs` (returns `ResponseHead::default()`), `src/adapters/http/pipeline.rs` (patterns gain `..`), `GW/tests/support/mod.rs`

**Interfaces:**
- Consumes: Task 4 (`build_client`, `classify`, `UpstreamResponse`, `UpstreamError`), Task 5 (`AnthropicConfig`).
- Produces:
  - `upstream::ResponseHead { request_id, retry_after, should_retry, content_type: Option<HeaderValue> }` (`Default`, `Clone`, `Debug`) and `fn apply(&self, headers: &mut HeaderMap)` (inserts the first three when `Some`; never `content-type`). `UpstreamResponse::Full { status, head, body }` and `Stream { status, head, stream }`.
  - `anthropic::AnthropicRoute { Messages, CountTokens, Models }` with `fn path(self) -> &'static str` (`/v1/messages`, `/v1/messages/count_tokens`, `/v1/models`).
  - `anthropic::Forward { pub query: Option<String>, pub headers: Vec<(HeaderName, HeaderValue)> }` (`Default`, `Clone`, `Debug`), `fn beta_values(&self) -> Option<String>` (the `anthropic-beta` values joined with `,`).
  - `AnthropicClient::new(cfg: &AnthropicConfig, connect_timeout: Duration, stream_idle_timeout: Duration) -> Result<Self, UpstreamError>`, `fn forward(&self, headers: &HeaderMap, query: Option<&str>) -> Forward`, `async fn send(&self, route: AnthropicRoute, method: Method, body: Bytes, stream: bool, fwd: &Forward) -> Result<UpstreamResponse, UpstreamError>`.
  - Test support: `RecordedRequest` gains `pub method: String` and `pub query: Option<String>`. The mock serves `POST /v1/messages`, `POST /v1/messages/count_tokens` and `GET /v1/models` with the same handler. New `MockResponse` arms and constructors: `spawn_json_with_headers(status, content_type: &str, headers: Vec<(String, String)>, body)` and `spawn_redirect(status, location: String)`.

- [ ] **Step 1: Write the failing tests** in `client.rs` (against `support`-style local axum servers in the test module, or in a new `GW/tests/anthropic_egress.rs` that uses `support::MockOpenAi`; pick the second):
  - `the_operator_key_and_only_anthropic_headers_go_upstream` (I2): caller headers `x-api-key: caller`, `authorization: Bearer caller`, `paigasus-org: …`, `dpop: x`, `x-claude-code-session-id: s`, `anthropic-beta: a`, `user-agent: u` give upstream `x-api-key` = the operator key, `anthropic-beta: a`, `anthropic-version: 2023-06-01`, `content-type: application/json`, `accept: application/json`, and none of the other caller headers.
  - `forward_keeps_every_anthropic_value_in_order` (Review Focus 1): `Anthropic-Beta: b1`, `anthropic-beta: b2`, `anthropic-version: 2023-06-01` give the upstream `anthropic-beta` values `[b1, b2]` in order and one `anthropic-version`; with no `anthropic-version`, `default_version` is sent once.
  - `the_query_cannot_change_the_host_or_the_path` (Review Focus 3, I3): query `beta=true&next=http://evil.example/@x/../v1/models` gives path `/v1/messages`, the same host, and the raw query unchanged.
  - `a_get_has_no_content_type_and_a_stream_accepts_sse`.
  - `the_response_head_keeps_only_the_d8_headers`: the mock answers with `request-id`, `retry-after`, `x-should-retry`, `anthropic-ratelimit-requests-limit`, `location`; the `ResponseHead` holds the first three and the `content-type`.
  - `a_redirect_is_a_502_and_the_second_host_sees_nothing` (D16): mock A answers 307 to mock B; `send` is `Err(UpstreamError::Redirect { .. })`; `B.request_count() == 0`.
  - `a_slow_non_stream_head_survives_the_idle_timeout` (§ 5): `stream_idle_timeout = 1 s`, `non_stream_timeout_secs = 3`, mock delay 2 s: `Ok` with status 200.
  - `a_non_2xx_is_a_full_response` (a 529 body arrives unchanged in `Full`).
  - `debug_never_leaks_the_anthropic_key`.
- [ ] **Step 2: Run** `SVC`. Expected: FAIL.
- [ ] **Step 3: Implement.** `new` builds with `build_client(Provider::Anthropic, …, read_timeout = max(stream_idle_timeout, non_stream_timeout))` and stores `default_version` as a `HeaderValue`. `forward` keeps every header whose name starts with `anthropic-` (names are lower case in `http`), all values via `get_all`, in order, then adds `default_version` when no `anthropic-version` exists. `send` builds the URL as `format!("{base}{path}")` plus `?{query}` when the query is `Some` and not empty; sets `x-api-key`, `content-type` (not on `GET`), `accept`, then the `fwd.headers`; applies `.timeout(non_stream_timeout)` only when `!stream`; maps a `3xx` status to `Redirect`; reads the `ResponseHead`; maps errors with `classify(Provider::Anthropic, …)`. `Debug` is derived over a `SecretString`.
- [ ] **Step 4: Run** `SVC`, `LINT`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): add the anthropic upstream client (SMA-750)`.

### Task 7: `x-api-key` authentication on the Anthropic route group (spec D6)

**Files:**
- Modify: `GW/src/adapters/http/auth.rs`, `src/adapters/http/mod.rs` (`AuthState` literal)

**Interfaces:**
- Consumes: nothing new.
- Produces: `AuthState { iam, dpop_enabled, accept_x_api_key: bool }`; `credentials(headers: &HeaderMap, dpop_enabled: bool, accept_x_api_key: bool) -> Option<Credentials>`. With the flag on: when any `x-api-key` header exists, exactly one non-empty value gives `Credentials::Bearer(value)` and anything else gives `None`; `Authorization` is then not read. With no `x-api-key`, today's rules apply. Both middlewares pass `auth.accept_x_api_key`.

- [ ] **Step 1: Write the failing tests** in `auth.rs`: `x_api_key_is_read_as_bearer_only_when_the_flag_is_on`; `x_api_key_wins_over_authorization`; `an_empty_x_api_key_is_missing_even_with_a_bearer` (Review Focus 2); `two_x_api_key_headers_are_missing`; `the_openai_routes_ignore_x_api_key` (flag off: `x-api-key` alone gives `None`); `a_dpop_bound_token_in_x_api_key_goes_the_bearer_leg` (with DPoP on and a `DPoP` header present, a fake IAM sees `introspect_token(token, None)`, answers `Unauthenticated`, and the middleware answers 401).
- [ ] **Step 2: Run** `SVC`. Expected: FAIL.
- [ ] **Step 3: Implement** the flag and the parse. Set `accept_x_api_key: false` in the existing `AuthState` literals (`mod.rs:118`, the two in the `auth.rs` tests).
- [ ] **Step 4: Run** `SVC`, `LINT`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): accept x-api-key on the anthropic route group (SMA-750)`.

### Task 8: The Anthropic error envelope (spec D7, § 6)

**Files:**
- Create: `GW/src/adapters/http/anthropic_error.rs`
- Modify: `GW/src/adapters/http/error.rs`, `src/adapters/http/mod.rs` (`pub mod anthropic_error;`), `src/domain/limits.rs`, `src/application/limits.rs`, `ci/error-registry/check.py` (an `asserts` row for `anthropic_error.rs` if its tests spell a code)

**Interfaces:**
- Consumes: `GatewayError::parts()`.
- Produces:
  - `LimitRefusal::BudgetExhausted { period, resets_at_unix, retry_after_secs: u32 }` and `GatewayError::BudgetExhausted { resets_at_unix, period, retry_after_secs: u32 }`. `domain::limits::choose_refusal(failed: &[FailedCheck], now: SystemTime) -> Option<LimitRefusal>` sets `retry_after_secs` to the seconds from `now` to `resets_at_unix`, at least 1, saturating at `u32::MAX`. `Limits::admit_for` passes `self.clock.now()`. The OpenAI rendering does not change.
  - `GatewayError::into_response` inserts `RenderedError(self)` into the response extensions (`#[derive(Clone, Copy, Debug)] pub struct RenderedError(pub GatewayError)` in `error.rs`).
  - In `anthropic_error`: `pub fn anthropic_type(err: GatewayError) -> &'static str`; `pub fn render(err: GatewayError, request_id: Option<&str>) -> Vec<u8>`; `pub async fn rerender(resp: Response) -> Response` (for `axum::middleware::map_response`); `pub async fn not_found() -> Response`; `pub async fn method_not_allowed() -> Response`.

- [ ] **Step 1: Write the failing tests** in `anthropic_error.rs`:
  - `each_case_maps_to_its_anthropic_type`: one row per `GatewayError` variant (strum `EnumIter`), checked against the § 6.1 table by status, with `BudgetExhausted` → `billing_error`, `RateLimited` → `rate_limit_error`, `UpstreamTimeout` → `timeout_error`.
  - `the_body_is_the_anthropic_envelope`: keys exactly `type`, `error`, `request_id`; `error` keys exactly `type`, `message`, `code`; `code` is the registry code of `parts()`; `message` equals the OpenAI message, except `MissingBearer`, which is the Global Constraints text.
  - `the_rerender_keeps_status_and_headers`: a toy router whose handler returns `GatewayError::MissingBearer` with an appended `WWW-Authenticate` and a `paigasus-tool-decisions` header, wrapped in `map_response(rerender)`: status 401, both headers present, `content-length` equals the new body length, body is the envelope.
  - `an_upstream_body_is_never_rerendered` (R5): a handler that returns a 400 with an Anthropic error body and no `RenderedError` comes out byte-identical.
  - `a_budget_refusal_sends_at_least_61_seconds`: `retry_after_secs` 5 gives `retry-after: 61`; 3600 gives `3600`; `x-should-retry: false` stays.
  - `a_rate_refusal_keeps_its_retry_after` (`retry-after` of the OpenAI rendering, at least 1).
  - `the_malformed_502_says_do_not_retry`: `ToolInterceptionMalformedResponse` gives `x-should-retry: false`.
  - `the_fallbacks_have_no_code`: `not_found()` gives 404 `not_found_error`; `method_not_allowed()` gives 405 `invalid_request_error`; neither has an `error.code` key.
  - In `domain/limits.rs`: `the_budget_refusal_carries_the_seconds_to_reset` (a fixed `now` 100 s before the reset gives 100); in `application/limits.rs`, the existing `a_budget_refusal_wins_and_is_counted_as_org_budget` asserts `retry_after_secs` from the test clock.
- [ ] **Step 2: Run** `SVC`. Expected: FAIL.
- [ ] **Step 3: Implement.** `rerender` reads `RenderedError` from the response extensions; when absent it returns the response unchanged. Otherwise it replaces the body with `render(err, request_id)` (the id from `paigasus_observability::current_ids()`), sets `content-length` and `content-type: application/json`, sets `retry-after` for `BudgetExhausted` (`max(retry_after_secs, 61)`) and `x-should-retry: false` for `ToolInterceptionMalformedResponse`, and keeps every other header and the status. Update every `BudgetExhausted` literal (`grep -rn "BudgetExhausted" rs/crates/services/paigasus-gateway`).
- [ ] **Step 4: Run** `SVC`, `LINT`, `REG`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): render gateway errors in the anthropic envelope (SMA-750)`.

### Task 9: Anthropic usage readers and the request estimate (spec D10, § 7)

**Files:**
- Create: `GW/src/adapters/http/anthropic_usage.rs`
- Modify: `GW/src/adapters/http/usage.rs` (`StreamUsage::Anthropic`), `src/adapters/http/mod.rs` (`pub mod anthropic_usage;`)

**Interfaces:**
- Consumes: `usage::{BodyUsage, bytes_to_tokens, FALLBACK_REQUEST_CAP, NON_TEXT_PART_TOKENS}`.
- Produces: `anthropic_usage::AnthropicUsageScanner` with `new()`, `feed(&mut self, chunk: &[u8])`, `finish(&mut self)`, `completion_records(&self) -> u64`, `reported_total(&self) -> Option<u64>`; `anthropic_usage::non_stream_usage(body: &[u8]) -> BodyUsage`; `anthropic_usage::request_tokens_estimate(v: &Value, body_len: usize) -> u64`; `StreamUsage::Anthropic(AnthropicUsageScanner)` in every `StreamUsage` match.

- [ ] **Step 1: Write the failing tests** in `anthropic_usage.rs`:
  - `a_full_stream_reports_the_d10_sum` (`message_start` usage 10/2/3/1 and `message_delta` `output_tokens: 7` give `10+2+3+7 = 22`).
  - `a_delta_replaces_each_present_field` (a `message_delta` with `input_tokens: 12` replaces 10).
  - `a_stream_cut_after_message_start_estimates_the_output` (`message_start` with input sum 15 and `output_tokens: 1`, then 4 content records: `reported_total() == Some(15 + 4)`; with `output_tokens: 9`: `Some(15 + 9)`).
  - `no_message_start_reports_nothing` (`None`).
  - `null_and_object_usage_fields_are_ignored` (Review Focus 4: `cache_read_input_tokens: null` and `server_tool_use: {"web_search_requests": 1}` change nothing; the record counts once).
  - `the_type_decides_not_a_substring` (a `content_block_delta` whose text holds `"message_delta"` and `"usage"` changes nothing).
  - `line_endings_cr_lf_and_crlf_split_records` (the same totals for each).
  - `the_body_reports_usage_or_estimates` (with `usage`: the D10 sum, missing fields 0; without `usage` or with a non-object `usage`: `Completion(ceil(len/4))` of the `text` of `text` blocks plus the compact JSON of `tool_use.input`).
  - `the_request_estimate_follows_section_7_3` (`system` as a string and as blocks, string and block `content`, `tool_use.input`, `tool_result.content` as a string and as blocks, `tools[]` as compact JSON, one `image` block counts 85, the floor `min(ceil(body_len/4), 32_768)`, and a non-object message gives the floor).
- [ ] **Step 2: Run** `SVC`. Expected: FAIL.
- [ ] **Step 3: Implement** the scanner with the record splitting of `UsageScanner` (CR, LF, CRLF, `data:` prefix, blank line, bounded record buffer). A record over the buffer bound still counts as one data record and is not parsed. Parse a complete record, read the top-level `type`, and read `message.usage` (for `message_start`) or `usage` (for `message_delta`) as numbers only.
- [ ] **Step 4: Run** `SVC`, `LINT`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): read anthropic usage for the budget charge (SMA-750)`.

### Task 10: The `/v1/messages` route (spec D4, D7, D8, D13, D15, D19, § 5, § 8, § 9, § 10.2)

**Files:**
- Create: `GW/src/adapters/http/messages.rs`, `GW/src/adapters/http/duplicates.rs`, `GW/tests/messages_proxy.rs`
- Modify: `src/adapters/http/pipeline.rs`, `chat.rs`, `responses.rs` (use `duplicates.rs`; pass `Forward::default()`), `dto.rs`, `mod.rs` (`AppState`, `MessagesSettings`, `router`, the `/readyz` doc), `src/main.rs`, every `AppState { … }` literal (see the `grep -rn "responses: None\|responses: Some" rs/crates/services/paigasus-gateway` list), `GW/tests/support/limits.rs`, `ci/error-registry/check.py`

**Interfaces:**
- Consumes: Tasks 4–9.
- Produces:
  - `pipeline::Upstream { OpenAi(UpstreamRoute), Anthropic(AnthropicRoute) }`; `Endpoint.upstream: Upstream` replaces `Endpoint.route`. `pipeline::proxy(endpoint: &'static Endpoint, state: AppState, caller: CallerContext, fwd: Forward, body: EnvelopeBytes) -> Response`. `pub(crate) fn record_upstream_call(status, started, endpoint)` (Task 11 uses it).
  - `AppState.anthropic: Option<Arc<AnthropicClient>>` and `AppState.messages: Option<Arc<MessagesSettings>>`; `http::MessagesSettings { max_request_bytes: usize, max_record_bytes: usize, max_held_bytes: usize, max_hold: Duration }`.
  - `duplicates::Duplicates { pub any: bool, pub top: HashSet<String> }` and `pub(crate) fn duplicates(body: &[u8]) -> Option<Duplicates>` (moved from `responses.rs`; `responses.rs` checks `top.contains("background")`).
  - `dto::MessagesRequest { model: String, messages: Vec<Value>, stream: Option<bool>, #[serde(flatten)] extra: Map<String, Value> }`.
  - `messages::MESSAGES_TERMINAL_SSE_ERROR: &str` (the D13 literal), `messages::MESSAGES: Endpoint` (`name: "messages"`, `upstream: Upstream::Anthropic(AnthropicRoute::Messages)`, `format: Format::AnthropicMessages`, the Task 9 readers, `limits` = `ti.limits` with `max_record_bytes`, `max_held_bytes` and `max_hold` from `state.messages`), `messages::create_message(State, Option<Extension<CallerContext>>, HeaderMap, OriginalUri, EnvelopeBytes) -> Response`.
  - The router: with `state.messages` `Some`, one group with `/v1/messages` (`post(create_message).fallback(anthropic_error::method_not_allowed)`), `route_layer(auth with accept_x_api_key: true)`, `route_layer(DefaultBodyLimit::max(settings.max_request_bytes))`, and outermost `route_layer(map_response(anthropic_error::rerender))`. With `None`, `/v1/messages` and `/v1/messages/count_tokens` answer `anthropic_error::not_found` (`any(..)`). `/v1/models` is not mounted when off.

- [ ] **Step 1: Write the failing unit tests** in `messages.rs`: `the_messages_terminal_frame_carries_a_registered_code` (strip the leading blank lines and `event: error`, parse `data:`, `ErrorReason::from_wire_reason(error.code)` is `Some`, `type` is `"error"`, `error.type` is `"api_error"`); `the_terminal_frame_is_its_own_record_after_a_partial_one` (as in `responses.rs`); `the_parse_splits_syntax_and_type_failures` (bad JSON → `BadRequestBody`; no `model`, `messages` not an array, `stream` a string → `InvalidRequestSchema`); `a_duplicate_top_level_key_is_refused_in_both_modes` (D19: `{"stream":false,"stream":true,…}` → `BadRequestBody` with interception on and off); `a_nested_duplicate_is_refused_only_with_interception` (→ `InvalidRequestSchema` on, `Ok` off).
- [ ] **Step 2: Write the failing service tests** in `GW/tests/messages_proxy.rs` (model it on `chat_proxy.rs` and `responses_proxy.rs`; use `MockOpenAi` with the Task 6 routes and the Task 1 `GWFIX` bodies):
  - Auth: `no_credential_is_401_in_the_anthropic_envelope`, `x_api_key_is_accepted`, `a_bearer_is_accepted`, `x_api_key_wins_over_a_bearer` (the fake IAM sees the `x-api-key` value).
  - Pass-through: `the_body_goes_upstream_byte_identical` (path `/v1/messages`, query `beta=true` unchanged); `a_stream_arrives_in_order_with_event_lines`; `a_mid_stream_transport_failure_ends_with_the_d13_frame` (`spawn_truncated_sse`); `streaming_obeys_stream_enabled`; `the_route_is_404_when_disabled` (Anthropic envelope, `not_found_error`); `client_abort_cancels_upstream`.
  - Egress (I2): `only_the_operator_key_and_anthropic_headers_reach_the_upstream` (caller `x-api-key`, `authorization`, `paigasus-org` never arrive; every `anthropic-*` value arrives; the default version when the caller sent none).
  - Errors: `gateway_errors_are_in_the_anthropic_envelope` (401, 413 over `max_request_bytes`, 400 schema, 429 rate with `retry-after`, 429 budget with `retry-after` > 60, each with `type: "error"` and the § 6.1 type); `an_upstream_error_body_arrives_byte_identical` (the Task 1 `prompt_too_long` body and its `content-type`, status 400, no charge); `the_d8_headers_pass_and_ratelimit_headers_do_not` (stream and body); `get_on_messages_is_a_405_in_the_anthropic_envelope` (with a valid credential); `the_rerender_keeps_www_authenticate_and_tool_decisions` (DPoP on, a 401 keeps `WWW-Authenticate`; a policy refusal keeps `paigasus-tool-decisions`).
  - Limits: `a_non_stream_answer_is_charged_the_d10_sum`, `a_full_stream_is_charged_the_d10_sum`, `a_stream_cut_after_message_start_is_charged_input_plus_estimate`.
  - Hardening: `an_upstream_redirect_is_a_502_and_the_target_sees_nothing` (D16); `a_slow_non_stream_head_is_not_cut_by_the_idle_timeout` (§ 5); `a_duplicate_stream_key_is_refused_before_admission` (D19; the upstream sees no request).
  - Logs: `the_request_log_carries_the_endpoint_and_the_beta_values` (`paigasus_logging::test_support::capture_logs`: `endpoint = "messages"` and `anthropic_beta = "a,b"`; the chat line has no `anthropic_beta` field).
- [ ] **Step 3: Run** `SVC`. Expected: FAIL.
- [ ] **Step 4: Implement.**
  - `pipeline::proxy`: after the parse and the streaming check, resolve the upstream. `Upstream::Anthropic` with `state.anthropic` `None` returns `GatewayError::Internal` before the limits. The send is one `match` on `endpoint.upstream` that calls `state.openai.send(route, body, stream)` or `anthropic.send(route, Method::POST, body, stream, &fwd)`; nothing else in `proxy` reads the provider. On every answer from the upstream, call `head.apply(resp.headers_mut())`. A non-2xx answer uses `head.content_type`, or `application/json` when it is `None` (the OpenAI client always returns `None`, so the OpenAI routes do not change). The request log line adds `anthropic_beta` only when `fwd.beta_values()` is `Some`.
  - `create_message` builds the `Forward` with `state.anthropic`'s `forward(&headers, uri.query())` (an empty `Forward` when the client is `None`; `proxy` then answers 500) and calls `proxy(&MESSAGES, …)`. `chat_completions` and `create_response` pass `Forward::default()`.
  - `parse_messages` (the `Endpoint.parse`): SMA-588 split; `duplicates`: any top-level duplicate → `BadRequestBody`; with interception on, any duplicate → `InvalidRequestSchema`; then the DTO; `request_estimate` from Task 9; `model` from the DTO.
  - `main.rs` builds `AnthropicClient` and `MessagesSettings` when `[messages] enabled = true`, with one info line `"the anthropic messages route is on (SMA-750)"`. Every other `AppState` literal gets `anthropic: None, messages: None`.
  - `ci/error-registry/check.py`: add `("rs/crates/services/paigasus-gateway/src/adapters/http/messages.rs", "emits", "the_messages_terminal_frame_carries_a_registered_code", "the D13 terminal SSE error frame of /v1/messages")`.
  - The `/readyz` doc comment names both upstreams (config presence only).
- [ ] **Step 5: Run** `SVC`, `LINT`, `REG`. Expected: PASS. `chat_proxy.rs` and `responses_proxy.rs` pass with only the `AppState` field edits.
- [ ] **Step 6: Commit** `feat(rs): serve /v1/messages through the anthropic upstream (SMA-750)`.

### Task 11: `count_tokens` and `models` (spec D11, D20)

**Files:**
- Create: `GW/src/adapters/http/anthropic_passthrough.rs`, `GW/tests/anthropic_passthrough.rs`
- Modify: `src/adapters/http/mod.rs` (mount the two routes in the Task 10 group), `src/application/charge_guard.rs` (only if `mark_no_charge` needs a call site change; it should not)

**Interfaces:**
- Consumes: Tasks 6, 8, 10 (`AnthropicClient::send`, `Forward`, `ResponseHead::apply`, `record_upstream_call`, `Limits::admit_for`).
- Produces: `anthropic_passthrough::count_tokens(State<AppState>, Option<Extension<CallerContext>>, HeaderMap, OriginalUri, EnvelopeBytes) -> Response` and `anthropic_passthrough::list_models(State<AppState>, Option<Extension<CallerContext>>, HeaderMap, OriginalUri) -> Response`.

- [ ] **Step 1: Write the failing service tests** in `GW/tests/anthropic_passthrough.rs`: `count_tokens_passes_through_with_zero_charge` (path, body bytes and query unchanged; the Task 1 answer arrives byte-identical; the recorded charge is 0; one request-rate unit is used); `models_passes_through_with_zero_charge` (`GET /v1/models?limit=1000` with `anthropic-version`; the answer is byte-identical; method `GET`); `models_without_anthropic_version_is_a_404_in_the_openai_envelope` (D20: status 404, `error.type` `invalid_request_error`, no upstream request); `the_pass_through_routes_need_a_credential` (401 in the Anthropic envelope, `x-api-key` accepted); `a_pass_through_error_is_forwarded_unchanged` (Review Focus 5: upstream 404 and 401 bodies with their `content-type` arrive unchanged; charge 0); `a_rate_refusal_on_count_tokens_is_429_in_the_anthropic_envelope`; `the_pass_through_routes_are_404_when_disabled` (`count_tokens`: Anthropic `not_found_error`; `models`: the plain 404 of today); `the_pass_through_routes_forward_the_d8_headers`; `the_pass_through_routes_label_their_endpoint` (`gateway_upstream_requests_total{endpoint="messages_count_tokens"}` and `{endpoint="models"}`).
- [ ] **Step 2: Run** `SVC`. Expected: FAIL.
- [ ] **Step 3: Implement.** Each handler: `admit_for(&caller, "<endpoint>")` when `state.limits` is `Some` (a refusal renders `GatewayError::from(refusal)`); `guard.mark_no_charge()`; `send` with the D5 `Forward`; on `Full` or `Stream`, the upstream status, `head.content_type` (or `application/json`), the body as received, and `head.apply`; on `Err`, `GatewayError::from(err)`. `record_upstream_call` with the endpoint label. One info log line per request with `endpoint`, `status`, `latency_ms`, `principal`, `scope`, `auth`, as the pipeline's line, and the message `"anthropic request proxied"`. `list_models` checks `anthropic-version` before admission; without it, it returns 404 with `{"error":{"message":"Not found.","type":"invalid_request_error","param":null,"code":null}}` and no `RenderedError` extension. `count_tokens` never parses the body.
- [ ] **Step 4: Run** `SVC`, `LINT`, `REG`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): pass count_tokens and models through to anthropic (SMA-750)`.

### Task 12: Interception through `/v1/messages` (spec D9, D12, D17, D18, § 3, § 8, § 10.2 last bullet)

**Files:**
- Create: `GW/tests/messages_interception.rs`
- Modify: `GW/tests/tool_policy.rs` (header note → real test), `GW/tests/tool_history.rs` (header note → real test), `GW/src/adapters/http/interception.rs` (the note near line 1026), `GW/src/lib.rs` (the two "not wired until SMA-750" notes)

**Interfaces:**
- Consumes: Tasks 3, 10. The § 15 type list (Task 1).
- Produces: nothing for later tasks except the test table that Task 14 can extend.

- [ ] **Step 1: Write the failing service tests** in `GW/tests/messages_interception.rs` (interception on, a `BlockNamed` decider as in `responses_proxy.rs`):
  - `a_streamed_blocked_tool_use_never_reaches_the_client` (no `CANARY` byte; the notice text block is present; `: paigasus-decision` present).
  - `a_non_stream_blocked_tool_use_is_rewritten_with_the_header` (`paigasus-tool-decisions` present).
  - `a_malformed_body_is_a_502_with_x_should_retry_false` (under `block`).
  - `an_upstream_error_event_during_a_hold_reaches_the_client_unchanged` (D17: the upstream `overloaded_error` record is the last record, byte-identical payload; no D13 frame).
  - `a_transport_failure_during_a_hold_gives_the_d13_frame`.
  - `a_held_call_keeps_alive_with_ping_events` (D12: a mock that sends `content_block_start` of a `tool_use`, then waits past `keepalive`; the client sees `event: ping\ndata: {"type":"ping"}\n\n` and no `: keep-alive` comment; use `tokio::time::pause` only if the relay timer allows it, else a 1 s keepalive in the test limits).
  - `every_recorded_type_passes_with_interception_on`: for each `REC`-derived stream kept in `GWFIX` (copy the streams this test needs into `GWFIX` in this task), allow-all gives payload-identical output (only `: paigasus-decision` comments added), and no `RecordDropped` metric. One row per event, block and delta type of the § 15 list.
  - `user_data_keys_pass_an_enforced_policy` (D18: a request with `Name` under each § 15 subtree passes `[tool_policy]` in `enforce` with a deny rule for another tool; status 200).
  - `a_denied_tool_definition_is_removed_from_the_upstream_body` (D9: enforce mode; the mock body has no denied tool; other fields byte-identical).
- [ ] **Step 2: Turn the notes into tests.** `tests/tool_policy.rs`: add `the_anthropic_route_applies_the_request_policy` and delete the note at lines 5–7. `tests/tool_history.rs`: add `the_anthropic_route_records_history` and delete the note at lines 6–8. `interception.rs`: change the note to say the relay test stays a unit test because it pins the relay timing; it no longer claims there is no route. `lib.rs`: delete "Only `/v1/chat/completions` (OpenAI) is wired; … wait for SMA-750" and the "(Anthropic, not wired until SMA-750)" qualifier; add one line that names the three Anthropic routes.
- [ ] **Step 3: Run** `SVC`. Expected: the new tests FAIL only where a defect exists; fix it in the owning task's file. Then PASS.
- [ ] **Step 4: Run** `SVC`, `CORE`, `LINT`. Expected: PASS.
- [ ] **Step 5: Commit** `test(rs): intercept tool calls through /v1/messages (SMA-750)`.

### Task 13: Operator docs, the alert, and the full gate run (spec § 9, § 11)

**Files:**
- Create: `docs/ops/RUNBOOK-anthropic-messages.md`
- Modify: `GW/gateway.toml.example`, `rs/CLAUDE.md` (the CA-knob line), `GW/CHANGELOG.md` (`[Unreleased]`), `docs/ops/RUNBOOK-observability.md`, `ops/observability/prometheus/rules/gateway.rules.yml`, `ops/observability/prometheus/rules/tests/gateway.test.yml`

**Interfaces:**
- Consumes: every earlier task.
- Produces: the branch, ready for the coordinator's Task 14.

- [ ] **Step 1: Write the runbook** `docs/ops/RUNBOOK-anthropic-messages.md`, in Simplified Technical English: the two config tables and the 404 rollback; the Claude Code setup (`ANTHROPIC_BASE_URL`; `ANTHROPIC_API_KEY` sends `x-api-key` and wins over `ANTHROPIC_AUTH_TOKEN`, so a stale key wins; `ANTHROPIC_CUSTOM_HEADERS` for `paigasus-org`; the `apiKeyHelper` TTL for OIDC tokens; `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` is not needed with a direct Anthropic upstream; `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1` for `/model`); the § 3 gaps (server tools and MCP not enforceable, caller-chosen cost features, the shared prompt cache, fail-closed on unknown types); the endpoint labels and the `anthropic_messages` ingress label; the malformed-record alert and what to do when it fires; that `GET /v1/models` lists every model of the operator account.
- [ ] **Step 2: Add the alert.** `gateway.rules.yml`: `GatewayToolInterceptionMalformedRecords`, `expr: sum(increase(gateway_tool_interception_records_dropped_total{reason="malformed"}[15m])) > 0`, `for: 0m`, severity `warning`, a `runbook_url` annotation to the new runbook section. The metric has no `endpoint` label, so the alert covers every route; the annotation says so. Add a promtool test case to `gateway.test.yml` with a control series (a `reason="oversized"` series that must not fire), per the `promtool-gate-semantics` memory.
- [ ] **Step 3: Update the other docs.** `RUNBOOK-observability.md`: the `endpoint` label values (`chat_completions`, `responses`, `messages`, `messages_count_tokens`, `models`) and the alert row. `rs/CLAUDE.md`: "**five**" becomes "**six**", and `upstream.anthropic.extra_ca_bundle_path` (SMA-750) joins the ADD list. `gateway.toml.example`: the operator notes of § 11 next to `[messages]`. `CHANGELOG.md` `[Unreleased]` → `### Added`: one entry for the three routes and the two tables. Dashboards: `ops/observability/grafana/dashboards/gateway.json` has no `endpoint` filter today, so it does not change (confirm with `grep -c endpoint`; expected `0`).
- [ ] **Step 4: Crate gates.** Run `CORE`, `SVC`, `LINT`, `GOLDEN`, `REG`.
- [ ] **Step 5: Full graph.** From the repo root, with the shims on `PATH`, run the `moon ci … --base origin/main --include-relations` command between the `ci-targets` markers of the root `CLAUDE.md`. At least these targets must pass: `:build :test :lint :fmt :deny :machete :observability-drift :error-code-single-site :http-extractor-envelope :promtool`. Read any red with the root `CLAUDE.md` diagnosis procedure (Step 0: copy `.moon/cache/ciReport.json` and the task state first). A red that comes from this Mac's bash split (`mapfile`, `declare -A`, `local -n`, a 512-byte pipe) is not a finding: run that gate again with the right bash, as the root `CLAUDE.md` says, and record both results.
- [ ] **Step 6: Commit** `docs: add the anthropic messages runbook and the malformed-record alert (SMA-750)`, with one body line per gate that needed a second bash.

### Task 14: Claude Code conformance and the D9 measurement (spec § 10.1 last items, § 10.4) — run by the coordinator

The coordinator runs this task, because it needs the operator key and a real `claude` binary. A subagent does not run it.

**Files:**
- Modify: `docs/superpowers/specs/2026-11-25-sma-750-anthropic-messages-ingress-design.md` (§ 15), the PR body
- Modify only in a fix step: the file of the owning task (for example `CORE_DIR/src/json.rs`, `GW/src/config.rs`, `GW/tests/messages_interception.rs`)

**Interfaces:**
- Consumes: the whole branch.
- Produces: the § 15 lines and the PR body section "Claude Code conformance".

- [ ] **Step 1: Start a local gateway.** `[messages] enabled = true`, `[tool_interception] enabled = true`, `[tool_policy]` in `enforce` with a policy that blocks one tool (for example `Bash` with a command that starts with `rm`), `[limits]` with a large budget, a local IAM or the test IAM setup, and `GATEWAY_UPSTREAM__ANTHROPIC__API_KEY="$(cat ~/.config/paigasus/anthropic-api-key)"` on the start command only. Turn on a debug log of the request event, block and delta types and the `anthropic-beta` values (the per-request log line has the beta values; the types come from a `measure.py` run over a copy of the stream that the session writes to `$SCRATCH`).
- [ ] **Step 2: Run Claude Code twice,** once with `ANTHROPIC_API_KEY=<paigasus key>` and once with `ANTHROPIC_AUTH_TOKEN=<paigasus key>`, with `ANTHROPIC_BASE_URL` set to the gateway. In each session: a task with tool calls; `/context` (count_tokens); `/model` with `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`; two turns with a long prefix (`cache_read_input_tokens` above 0 on the second turn, from the gateway's charge log); the blocked tool (the session continues after the notice). Count the `count_tokens` calls per session from the request log.
- [ ] **Step 3: Measure D9.** Through the gateway, send a follow-up turn after a blocked `tool_use` that came after a `thinking` block: once with all calls blocked, once with one blocked and one allowed. Record whether Anthropic answers 200 or 400, and whether Claude Code recovers from a 400.
- [ ] **Step 4: Record.** Append to spec § 15: the event, block and delta types of the session, the `anthropic-beta` values, the `count_tokens` calls per session, the D9 results, and each check of Step 2 as pass or fail. Copy the same into the PR body.
- [ ] **Step 5: Fix step (only if needed).** If the session shows an event, block or delta type that is not in the Task 1 list, add its row to `every_recorded_type_passes_with_interception_on` (Task 12) with a scrubbed stream in `GWFIX`, run it, and fix CORE if it fails (one task of work at most; a larger fix stops here and goes to Sven). If a measured size or hold time breaks the D15 defaults, change them in `GW/src/config.rs` and `gateway.toml.example`. If a user-data subtree is not in the D18 list, add it in `CORE_DIR/src/json.rs` with a test. Run `CORE`, `SVC`, `LINT`.
- [ ] **Step 6: Commit** (only when Step 5 changed a file) `fix(rs): <what changed> after the claude code session (SMA-750)`, and commit the spec change `docs(rs): record the claude code conformance results (SMA-750)`.

---

## Self-Review

1. **Spec coverage.** § 1, AC 1: Tasks 10, 11. § 2.2 R1 (path match, query): Task 6, Task 10. R2, R3, D5: Task 6, Task 10 egress. R4, D12: Task 3, Task 12. R5: Task 8 (`an_upstream_body_is_never_rerendered`), Task 10, Task 11, D17 in Tasks 3 and 12. R6, D8: Tasks 6, 8, 10. § 3 I2, I3, AC 2: Tasks 6, 10. § 3 gaps and the alert: Task 13. D1, D2: Tasks 4, 6. D3: Task 5 (config, detector cap), Task 10 (`/readyz` doc). D4: Tasks 10, 11. D6: Task 7, Task 10 auth tests. D7, § 6, AC 3: Task 8, Task 10 error tests, Task 11. D9: Task 12 (rewrite on), Task 14 (measurement). D10, § 7, AC 4: Task 9, Task 10 limits tests. D11, D20, AC 6: Task 11. D13: Task 10. D14, § 10.3, AC 7: Task 2. D15: Task 1 (measure), Task 5 (config), Task 10 (`limits` fn). D16: Tasks 4, 6, 10. D17: Tasks 3, 12. D18: Tasks 1, 3, 12. D19: Task 10. D21: Task 1. § 8 content-type and `max_tokens` cut: Task 10 (`head.content_type`), Task 1 recording `max_tokens_in_tool_use` replayed in Task 12. § 9: Task 10 (log fields), Tasks 10–11 (labels), Task 13 (runbook mapping, dashboards). § 10.1, AC 8: Task 1, Task 14. § 10.4, AC 9: Task 14. § 11: Tasks 10, 11 (MANIFEST, extractors), Task 13 (docs, gates).
2. **Step scan.** Each test step names its tests and the spec values they check. Each code step names its signatures and files. Task 12 Step 3 allows a fix in the owning task's file, because the end-to-end tests are the first place where Tasks 3 and 10 meet.
3. **Type consistency.** `UpstreamError`, `UpstreamResponse`, `Provider`, `build_client` (Task 4) are used in Tasks 6, 10, 11. `ResponseHead` and the `head` field (Task 6) are used in Tasks 10, 11. `Forward` and `AnthropicRoute` (Task 6) are used in Tasks 10, 11. `accept_x_api_key` (Task 7) is set in Task 10's router. `RenderedError`, `rerender`, `not_found`, `method_not_allowed` (Task 8) are wired in Task 10. `retry_after_secs` on `BudgetExhausted` (Task 8) is read by `rerender`. `StreamUsage::Anthropic` (Task 9) is used by `MESSAGES` (Task 10). `record_upstream_call` (Task 10) is used in Task 11. `Protocol::keep_alive`, `upstream_error_is_terminal`, `parse_strict_anthropic_request` (Task 3) are exercised in Task 12.
4. **Review Focus.** Five lines, each with its test: 1 and 3 in Task 6, 2 in Task 7, 4 in Task 9, 5 in Task 11.
5. **Proportion.** The plan holds no function bodies. The only literal blocks are spec values and test names.

Decisions this plan makes where the spec is silent or leaves a choice (the coordinator can change them before execution):

- Fixture layout: `.sse` streams in CORE `tests/fixtures/anthropic/recorded/`, every file a service test reads in `GW/tests/fixtures/anthropic/`. The recording scripts sit with the CORE streams, as the OpenAI scripts do.
- The D21 scrub also replaces a top-level `request_id` in a recorded body.
- The shared helper is `adapters/upstream.rs`. `ChatResponse`, `OpenAiError` and `OpenAiByteStream` become `UpstreamResponse`, `UpstreamError` and `UpstreamByteStream`, with a `Provider` field that keeps every OpenAI text byte-identical.
- An upstream `3xx` is also a 502 on the OpenAI routes (D16 says "both clients"). This is a behaviour change for the OpenAI routes, which today follow a redirect.
- The detector default counts `responses.max_request_bytes` whether `[responses]` is on or off (as today), plus `messages.max_request_bytes` when `[messages]` is on. A literal "enabled routes only" reading would lower today's default from 16 MiB to 1 MiB when `[responses]` is off.
- `MissingBearer` on the Anthropic routes has its own message that names both headers (D6); every other message is the OpenAI text (§ 6.1).
- With `[messages] enabled = false`, `/v1/messages` and `/v1/messages/count_tokens` answer an Anthropic `not_found_error`, and `/v1/models` stays the plain 404 of today (D20).
- The budget wait is computed in `choose_refusal` from the `now` that `Limits::admit_for` passes from its clock.
- The D20 404 body is `{"error":{"message":"Not found.","type":"invalid_request_error","param":null,"code":null}}`, with no registry code.
- The malformed-record alert covers every route, because the metric has no `endpoint` label.
- The D17 terminal frame keeps the upstream payload bytes and re-adds the `event: error` line.
