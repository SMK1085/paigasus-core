# SMA-749 — gateway: assemble, hold and rewrite tool calls in the response path

- Linear: SMA-749 (spec 1 of the SMA-747 umbrella, "Gateway tool-call interception").
- ADR: ADR-0027 (Accepted 2026-11-08), decisions (a) one check point, (b) hold only tool
  calls, (c) replace a blocked call with a notice, (e) evaluation in the gateway.
- Status: approved on 2026-11-09, after the spec challenge (§ 10).
- Amended 2026-11-11: the implementation rulings are recorded in § 11, and the sentences they
  changed are edited in place.
- Scope: a new lib crate `paigasus-gateway-core` (a synchronous, I/O-free state machine), and
  its wiring into `paigasus-gateway`'s `/v1/chat/completions` handler, off by default.
- Out of scope: the policy decision itself (SMA-752), session history (SMA-753), detectors
  (SMA-754), the decision log (SMA-755), the Anthropic ingress route (SMA-750), MCP (SMA-756),
  any rewrite of the request body, and the non-goals in § 2.

## 1. Problem

The gateway forwards model responses byte for byte. A response can contain tool calls: the
model asks the client's agent to run a function with some arguments. Today nothing between the
model and the client can look at a tool call, decide on it, or remove it.

ADR-0027 puts that check in the gateway. Every later spec needs the same foundation first:

1. find every tool call in a response, streaming or not, for OpenAI Chat Completions and
   Anthropic Messages;
2. hold a streaming tool call until it is complete, while text keeps streaming;
3. ask a decider about the complete calls;
4. forward allowed calls and replace blocked ones with a notice, in a response the client's
   SDK still parses.

This spec builds that foundation with a decider that allows everything. It ships wired in and
off by default.

## 2. Threat model and invariant

**Adversaries.**

- T1. **Model output is hostile**, always. Prompt injection can make the model emit any tool
  call, any arguments, any text.
- T2. **The upstream may be non-conforming or compromised.** `upstream.openai.base_url` and
  `extra_ca_bundle_path` exist for self-hosted vLLM / LiteLLM (SMA-558). Such upstreams
  interleave indices, emit odd framing, or can be made to emit arbitrary bytes. Nothing about
  the upstream's framing or JSON is trusted.
- T3. **The caller is not an adversary of its own agent** in this spec. Request-side
  differentials (for example duplicate `tools` keys) are SMA-752's concern.

**Invariant (I1).** For every supported client parser P (§ 8.2), every tool call that P
accumulates from the bytes the gateway emits was decided `Allow`, with the same choice, kind,
name and arguments that the decider saw. Anything the gateway cannot classify with certainty
is not forwarded (or triggers `on_failure`).

**Non-goals.** Tool calls encoded inside text content (ReAct / XML conventions, or a vLLM tool
parser that fails and falls back to `content`); tools executed by the provider (Anthropic
`server_tool_use`, `mcp_tool_use`); the OpenAI Responses API; request rewriting.

## 3. Decisions

| Id | Question | Decision |
|----|----------|----------|
| D1 | Where the code lives | New crate `paigasus-gateway-core` (`publish = false`): types, SSE framing, JSON classification, assembly, rewrite, as a sans-IO state machine. `paigasus-gateway` drives it with tokio. No kernel change in this spec. |
| D2 | SSE input framing | WHATWG line splitting (CR, LF, CRLF), with a CR held until the next byte. Strip one leading UTF-8 BOM. Multi-line `data:` joined with `\n`. |
| D3 | SSE output framing | **Re-framed, not forwarded.** For every classified record, emit `data: <original payload bytes>\n\n` (Anthropic: preceded by `event: <validated name>\n`). Upstream comments, `id:`, `retry:` and unknown fields are dropped. |
| D4 | End of stream | An unterminated last record is parsed and classified (as `UsageScanner::finish` does). Records after `[DONE]` are dropped and counted. |
| D5 | JSON parsing | One strict parse for stream records and non-stream bodies. A record is **malformed** if it is not valid JSON, has a duplicate key at any depth, has a `__proto__` / `constructor` / `prototype` key, or has a key that case-folds to a classifier key without being byte-equal to it. |
| D6 | Frame classification | Total, by a positive allowlist (§ 4.2): **tool-free** (forward), **tool-bearing** (hold), **malformed** (D7). Nothing else. |
| D7 | A malformed record | Queue empty: dropped and counted (never forwarded). Queue non-empty: `on_failure` for the whole stream. |
| D8 | Records over `max_record_bytes` | Never skipped. Stream-level `on_failure` (the choice cannot be known without parsing). |
| D9 | Canonical assembly | Strict rules (§ 4.3). Any deviation is `on_failure` for the stream. Where client SDK accumulators disagree, the strict form makes them agree. |
| D10 | When a hold completes (OpenAI) | At its choice's `finish_reason`. `[DONE]` or EOF with an open hold and no `finish_reason` = truncated → `on_failure`, never "complete". |
| D11 | When a hold completes (Anthropic) | At `content_block_stop` of the block. `message_stop` / EOF with an open block = truncated → `on_failure`. |
| D12 | Ordering and release | **One FIFO per stream.** Once any hold is open, every later frame is queued in order. Each queued frame carries the set of (choice, call) or (block) it touches. A frame is releasable when every member of its set is decided. Release pops from the head and stops at the first frame that is not releasable. Text before the first hold is forwarded at once. |
| D13 | Held memory | `max_held_bytes` (default 4 MiB) is a **per-stream total** of the queue. Overflow → `on_failure` for every open hold. |
| D14 | Time limits | `max_hold_secs` (default 300, equal to `stream_idle_timeout_secs`) from a hold's first frame to its completion; `decide_timeout_secs` (default 5) for the decider. Either → `on_failure` for that hold. |
| D15 | Keep-alive | `: keep-alive\n\n` every `keepalive_secs` (default 10) while the queue is non-empty and nothing else was emitted. |
| D16 | `on_failure` | `"block"` (default) or `"allow"` (observe mode, § 5.3). |
| D17 | Upstream transport error | The state machine consumes `Result<Bytes, _>`. On `Err` it is aborted: the queue is dropped, then the existing `TERMINAL_SSE_ERROR` is emitted. An upstream `{"error":…}` record while a hold is open is treated the same. |
| D18 | Decider contract | `decide(ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> BoxFuture<'static, Result<Vec<Decision>, DeciderError>>`. Called once per completed hold. A result whose length differs from `calls.len()` is a `DeciderError`. Release looks up each call's decision; a missing one is `Block`. |
| D19 | Request side | Parse the request into `ToolTurn::Parsed { definitions, prior_calls, tool_results }`, including legacy `functions` and `role:"function"` messages. A parse failure gives `ToolTurn::Unavailable(reason)`, never an empty `Parsed`. No request rewrite. |
| D20 | Rewrite rules | § 4.4. Blocked calls are removed per fragment; `role` is always kept; `logprobs` are stripped from rewritten frames; `finish_reason` / `stop_reason` are rewritten wherever they arrive, including after a hold closed by `on_failure`. |
| D21 | Notice | Fixed text built from `BlockCode` and a sanitized name; never from argument or detector content (§ 4.5). |
| D22 | Decision reporting | Streaming: `: paigasus-decision <ASCII-escaped JSON>` per decision, emitted just before the frames that hold released. Non-stream: `paigasus-tool-decisions` header, ASCII-escaped JSON, capped at 2 KiB with `"truncated":true`. Schema in § 4.6. |
| D23 | Identity | With `AllowAll`, the sequence of emitted `data` payloads is byte-equal to the upstream's classified payloads; only gateway comments are added. Framing may differ (D3). |
| D24 | Billing | `UsageScanner` and `ChargeGuard` keep reading the raw upstream bytes before the state machine; `non_stream_usage` reads the original body. A drop, a rewrite or the new 502 does not change the charge. |
| D25 | Cancellation | Nothing is spawned. The decider future is owned by the relay's stream state. Client disconnect drops everything, as today. |
| D26 | Config | `[tool_interception]`, `#[serde(default, deny_unknown_fields)]`, `enabled = false`. Only `/v1/chat/completions` is wired. Anthropic is library code until SMA-750. |
| D27 | Interception off | The relay is exactly today's relay. The state machine is not constructed. |
| D28 | Capability exposure | Not advertised in `service_info` in this spec. It reveals security posture and has no consumer yet. |
| D29 | Property testing | `proptest` with grammar-based generators against a differential oracle (§ 8.1), not `cargo-fuzz` (stable 1.95 pin; `cargo-fuzz` needs nightly). |

### Reasons

- **D1, no kernel change.** The challenge found that `paigasus-kernel` is `publish = true` and
  tied by `repo:version-lockstep` to the py and npm binding releases. Types no binding exports
  would still force a kernel release. ADR-0027 (e) places evaluation in the gateway via the
  kernel's `decide()`, which arrives with Cedar in SMA-752. The shared types move to the kernel
  then, when they have a binding user. Until then they live in gateway-core with
  `#[non_exhaustive]`. `DecisionId` is deferred to SMA-755, its only user.
- **D1, sans-IO.** A runtime-free `Stream` that polls the upstream, a decider future and two
  timers at once is the hardest shape to get right and to test. A synchronous machine with
  explicit inputs and actions makes the properties in § 8 plain synchronous tests, and it
  removes the self-referential-future problem of a borrowed decider call.
- **D3, re-framing.** Any byte the gateway forwards without reading is a channel to a client
  that reads it differently: a BOM-prefixed first line (`TextDecoder` strips the BOM, this
  repo's own `chat-stream.ts` and `paigasus-sdk/src/chat.ts` use it), lax line-trimming
  clients (`paigasus-sdk/src/chat.ts:108` trims), records after `[DONE]`, a JSON body
  returned on a `stream:true` request. Re-framing closes the whole class. Payload bytes stay
  untouched, so D23 still catches any accidental re-serialization.
- **D5, strict JSON.** Records that parse everywhere but mean different things are worse than
  records that fail. Duplicate keys: serde and JS keep the last, Go's `encoding/json` merges
  struct-typed duplicates. Case-insensitive matching in Go and Newtonsoft makes
  `"Tool_Calls"` a tool call to them. `__proto__` can change the accumulated object in
  JavaScript. Rejecting all of these costs nothing on a conforming upstream.
- **D6, positive allowlist.** "Has a `tool_calls` key" is wrong both ways. It misses
  `tool_calls` as an object or string and `message.tool_calls` inside a chunk. It also holds
  all text on upstreams that send `"tool_calls": []` on every delta (some vLLM versions).
- **D9, canonical assembly.** The SDKs disagree. openai-python concatenates every string field
  except `index` and `type`, so a second `name` fragment becomes `safe_` + `rm`. openai-node
  replaces it. anthropic-python appends a block on start but applies deltas by index. The
  decider must see what every client will run, and that is only knowable for the canonical
  form.
- **D10/D11, truncation.** The relay turns a transport error into a data record and a clean
  end. An assembler placed after that conversion would see EOF and release a call cut off
  mid-arguments. D17 moves the state machine in front of the conversion.
- **D12, one FIFO.** Per-choice holds with a shared tail leak: releasing hold A would ship
  frames of hold B sitting in A's tail. A frame can also carry several choices. Keeping the
  stream order and releasing only fully decided frames is the simplest rule that never leaks.
  The cost: other choices pause during a hold. Interception supports tool calls in choice 0
  only (§ 11), so an `n > 1` request whose later choices call tools is not supported while
  interception is on.
- **D13, per-stream total.** `n` is caller-controlled (up to 128 on OpenAI). A per-hold cap
  multiplies by `n`.
- **D14, 300 s.** A file-writing call with 40 KB of arguments takes about 200 s at 50 tok/s. A
  60 s cap would block it. The decider timeout is separate because SMA-752 may add a remote
  call.
- **D18, owned arguments and length check.** A future decider that returns too few decisions
  must not silently allow the rest. `DecisionContext` carries the caller (principal, scope,
  org) and the request ids, captured in the handler. Inside the stream, `current_ids()` is
  `None` (SMA-504 §4.3).
- **D19, `Unavailable`.** An empty `Parsed` would make SMA-753's sequence rules fail open on a
  request that did not parse.
- **D21, closed notice text.** The model reads the notice on its next turn. Echoing detector
  or argument content would reflect attacker text into the prompt.
- **D22, header cap.** Many parallel calls can exceed nginx's default 4–8 KiB
  `proxy_buffer_size`, and the proxy would turn the response into a 502. `HeaderValue`
  rejects raw UTF-8.
- **D24, billing.** The upstream produced and billed the tokens of a blocked call. The charge
  follows the upstream, as SMA-677 and SMA-730 decided.

## 4. Semantics

### 4.1 Types (`paigasus-gateway-core::types`, all `#[non_exhaustive]`)

```rust
pub enum ToolCallKind { Function, Custom, LegacyFunctionCall, AnthropicToolUse, Other }

pub struct ToolCall {
    pub choice: u32,              // OpenAI choice index; 0 for Anthropic
    pub position: u32,            // tool-call index (OpenAI) or block index (Anthropic)
    pub id: Option<String>,       // absent for legacy function_call
    pub name: String,
    pub kind: ToolCallKind,
    pub arguments: String,        // see below
}

pub enum Decision { Allow, Block { code: BlockCode } }
pub enum BlockCode { Policy, Detector, HoldLimit, Malformed, Truncated, DeciderError }
pub enum Basis { Decider, OnFailure }   // how a decision was reached (§ 5.3)
```

`arguments` holds: the concatenated `function.arguments` string (Function, LegacyFunctionCall);
`custom.input` (Custom); the concatenated `partial_json` (AnthropicToolUse); the raw JSON of the
whole tool-call entry (Other).

### 4.2 Classification

**OpenAI stream chunk.** Tool-free if all of these hold:

- the payload is a JSON object, and `choices` is absent or an array of objects;
- every `choices[i].index` is a JSON integer literal in `0..=127`;
- every `delta` is absent or an object, and no choice has a `message` key;
- every `delta.tool_calls` is absent, `null` or `[]`, and every `delta.function_call` is
  absent or `null`.

Tool-bearing if the above holds except that some `delta.tool_calls` is a non-empty array of
objects, or some `delta.function_call` is an object. Anything else is malformed.

`{"error":…}` at the top level is an upstream error record (D17 when a hold is open;
forwarded otherwise, as today).

**OpenAI non-stream body.** The same rules applied to `choices[].message`.

**Anthropic.** Dispatch on `data.type`; a present `event:` name that differs is malformed.
These types are known: `message_start`, `message_delta`, `message_stop`, `ping`, `error`,
`content_block_start`, `content_block_delta`, `content_block_stop`. Any other type is
malformed. Block types:

- `text`, `thinking`, `redacted_thinking`: tool-free content.
- `server_tool_use`, `*_tool_result`, `mcp_tool_use`: tool-free, reported `unenforceable`.
- `tool_use`: tool-bearing.
- any other block type: tool-bearing, kind `Other` (fail closed).

### 4.3 Canonical assembly (any deviation → stream-level `on_failure`, code `Malformed`)

OpenAI, per choice:

- tool-call `index` values first appear in order 0, 1, 2, …; interleaving after that is fine;
- `id`, `type` and `function.name` (or `custom.name`) each appear in exactly one fragment;
- `type` never changes, and `function` and `custom` never appear in the same entry;
- `function.arguments` and `custom.input` are strings or `null` in every fragment (`null` adds
  nothing, as in every SDK);
- at the call's completion, `function.arguments` (Function and legacy calls) is `""` or one
  complete JSON value (§ 11, `incomplete_arguments`); `custom.input` is free text;
- a legacy `function_call` and `tool_calls` never appear in the same choice;
- no fragment arrives for a call after its choice's `finish_reason`.

Anthropic:

- a block's `index` equals the number of earlier `content_block_start` events;
- `tool_use.input` at `content_block_start` is `{}`;
- `message_start.message.content` is `[]`;
- no delta arrives for an unknown or stopped index;
- `partial_json` is a string.

### 4.4 Rewrite

Only frames that contain a blocked call are re-serialized.

**OpenAI streaming.** In every queued frame of a blocked call's choice:

- Remove the blocked entries from `delta.tool_calls`. Renumber the remaining entries'
  `index` from 0, in first-appearance order. Drop the key when it becomes empty.
- Remove `delta.function_call` when the legacy call is blocked.
- Keep `role`. Strip `logprobs`.
- Never drop a frame or a choice entry (§ 11, P-10): a choice entry whose blocked data is
  removed stays, with its `index` and an emptied `delta`.

Insert the notice as one chunk, `{"delta":{"content":"<notice>"}}` for that choice, right
before the frame that carries the choice's `finish_reason`. Copy `id`, `created`, `model` and
`system_fingerprint` from that frame when they are strings or numbers, and set `object` to
`chat.completion.chunk`. A notice owed at the end of the stream copies them from the last
accepted record. When text already streamed for the choice,
the notice starts with `\n\n`. `finish_reason` `"tool_calls"` or `"function_call"` becomes
`"stop"` when no call of the choice remains. Per-choice state survives the decision, so a
`finish_reason` that arrives after an `on_failure` close is rewritten too.

**OpenAI non-stream.** The same per-choice rules applied to `message`. The notice is appended
to `message.content` when it is a string, or set when it is `null`. When `content` is an
array, the notice is appended as a text part.

**Anthropic.** A blocked `tool_use` block becomes a `text` block at the same index:
`content_block_start {type:text, text:""}`, one `text_delta` with the notice, and
`content_block_stop`, each with its `event:` line. Its `input_json_delta` frames are dropped.
`message_delta.stop_reason` `"tool_use"` becomes `"end_turn"` when no `tool_use` block remains.
This applies wherever that frame arrives.

### 4.5 Notice and names

The notice reads `[paigasus] tool call "<name>" was blocked (<code>).`. `<code>` is the
kebab-case `BlockCode`. `<name>` is truncated to 128 bytes, and any byte outside
`[A-Za-z0-9_.:-]` becomes `_`.

### 4.6 Decision report schema

```json
{"v":1,"choice":0,"position":1,"name":"<sanitized>","name_sha256_8":"<8 hex>",
 "decision":"allow|block|unenforceable","basis":"decider|on_failure","code":"<kebab or null>"}
```

`name_sha256_8` disambiguates names that sanitize alike.

## 5. Components and data flow

### 5.1 `paigasus-gateway-core` (sans-IO)

```rust
pub struct Interceptor { /* per-stream state */ }
impl Interceptor {
    pub fn new(format: Format, limits: Limits, now: Instant) -> Self;
    pub fn push(&mut self, chunk: &[u8], now: Instant) -> Vec<Action>;
    pub fn decided(&mut self, hold: HoldId, r: Result<Vec<Decision>, DeciderError>, now: Instant) -> Vec<Action>;
    pub fn tick(&mut self, now: Instant) -> Vec<Action>;
    pub fn finish(&mut self, now: Instant) -> Vec<Action>;   // clean upstream end
    pub fn abort(&mut self) -> Vec<Action>;                  // upstream error, D17
}
pub enum Action { Emit(Bytes), Decide { hold: HoldId, calls: Vec<ToolCall> }, Wake(Instant), Count(Metric) }
pub fn intercept_body(format: Format, body: &[u8]) -> Result<BodyPlan, Malformed>;  // non-stream
```

The terminal error frame is a constructor argument, so the Anthropic path can pass its own.
Modules: `sse`, `json` (strict parse, D5; skeletons use `Cow<str>` / `&RawValue`, never
`&str`, because arguments always contain escapes), `openai`, `anthropic`, `turn`, `assembly`,
`rewrite`, `report`, `types`. Dependencies: `serde`, `serde_json` (`raw_value`), `bytes`,
`thiserror`, `sha2`. No tokio, futures, axum or reqwest.

### 5.2 Gateway wiring

- `config.rs`: `ToolInterceptionConfig { enabled, max_record_bytes (1 MiB), max_held_bytes
  (4 MiB), max_hold_secs (300), decide_timeout_secs (5), keepalive_secs (10), on_failure }`.
  Validation: all non-zero, `max_record_bytes <= max_held_bytes`, `keepalive_secs <
  max_hold_secs`.
- `chat.rs`, streaming: the relay's upstream reader feeds raw bytes to `UsageScanner` first,
  then to `Interceptor::push`. On `Err` it calls `abort()`, emits the actions, and then emits
  `TERMINAL_SSE_ERROR` (owned by the interceptor when enabled). The relay state owns at most
  one pending decider future and one `tokio::time::Sleep`. It polls them with the upstream
  in `select!` (biased: decider, timer, upstream).
- `chat.rs`, non-stream: `non_stream_usage` reads the original body. Then `intercept_body`
  runs: allow-all returns the original bytes; a block re-serializes once. For a 2xx body that
  is malformed, `block` returns 502 `tool-interception-malformed-response` (the charge is
  settled as for the forwarded body); `allow` forwards it. Bodies over `max_held_bytes` are
  malformed.
- `AppState` gets `Arc<dyn ToolCallDecider>`, which is `AllowAll` for now.
  `DecisionContext` is built in the handler from `CallerContext` and the request ids.
- Metrics, in `paigasus-observability/src/names.rs` plus `ALL`:
  - `gateway_tool_interception_records_dropped_total{reason}`: `malformed`, `after_done`,
    `late_fragment`, `oversized`;
  - `gateway_tool_interception_decisions_total{decision,basis}`;
  - `gateway_tool_interception_on_failure_total{code}`;
  - `gateway_tool_interception_hold_seconds` (histogram).

### 5.3 Observe mode (`on_failure = "allow"`)

On overflow or hold timeout, the affected holds are released as they are. Later fragments of
those calls are forwarded unheld. When such a call completes, the decider is still consulted
(unless it outgrew `max_held_bytes`, § 11),
and the decision is reported with `basis: "on_failure"` and is not enforced. On a malformed
record while the queue is non-empty, or a canonical-form deviation, the stream switches to
pass-through: the rest is forwarded as received, and the switch
is reported once. A decider error or timeout means allow, with `basis: "on_failure"`.

## 6. Error handling

| Case | `block` | `allow` |
|---|---|---|
| Malformed record while the queue is non-empty, or a canonical-form deviation | Every open and later hold is blocked (`Malformed`). Later tool-bearing frames are dropped. | Pass-through (§ 5.3). |
| Record over `max_record_bytes` | As malformed. | As malformed. |
| Queue over `max_held_bytes` | Open holds blocked (`HoldLimit`); their later fragments dropped. | § 5.3 |
| Hold over `max_hold_secs` | That hold blocked (`HoldLimit`). | § 5.3 |
| Decider error, timeout, wrong length | That hold blocked (`DeciderError`). | Allow, `basis: "on_failure"`. |
| Truncated hold (D10/D11) | Blocked (`Truncated`), then the stream ends as received. | Released as is. |
| Upstream transport error / error record mid-hold | D17. | D17. |
| Non-2xx upstream | Forwarded as today, no interception. | Same. |

## 7. Repo gates the new crate touches

- `rs/Cargo.toml` `members`: one literal line (SMA-663 A11).
- `ci/affected-graph/run.sh`: the `lockfile->all-lint` set, plus a new
  `gateway-core->consumer-tasks` case (a gateway-core source edit selects the gateway's
  build, test and lint, and `gateway-console-ts:test-e2e`).
- The crate's `moon.yml`: `fileGroups.upstreams`, `dependsOn` as for the other lib crates. The
  gateway's `moon.yml` gains the dependency.
- Error code: `ERROR_REASON_TOOL_INTERCEPTION_MALFORMED_RESPONSE = 313` in
  `contracts/proto/paigasus/common/v1/error.proto` (gateway range). This also needs:
  - codegen drift for rs, py and ts;
  - a `GatewayError` variant;
  - the `ci/error-registry/check.py` MANIFEST row and its membership test;
  - the `mapError` mapping in `ts/packages/paigasus-sdk/src/chat.ts`.
- Metric names: `paigasus-observability` `names.rs` and `ALL` (observability drift).
- `sha2` is already in `rs/Cargo.lock`; the plan picks the existing version so `:deny` and
  `:osv` see no new crate.

## 8. Testing

### 8.1 Differential oracle (the security proof)

`tests/oracle/` holds Rust models of these client accumulators:

- openai-python `accumulate_delta` / `ChatCompletionStreamState`;
- openai-node `ChatCompletionStream`;
- LangChain's chunk merge;
- anthropic-python `MessageStream`;
- this repo's `chat-stream.ts` and `paigasus-sdk/src/chat.ts` parsers.

Each model takes the emitted bytes and returns the tool calls that client would run. The
models cover their framing and JSON quirks: BOM, trim, CR handling, dispatch after `[DONE]`,
duplicate-key policy, and index handling. Each model cites the pinned SDK version and source
location it reproduces.

**Property P1 (I1).** For every generated upstream stream and every model M, every call M
derives from the output equals a decision-`Allow` call by (choice, kind, name, arguments).

**Property P2 (no panic, bounded memory).** The queue never exceeds `max_held_bytes` plus one
record.

**Generators** are grammar-based. They produce conforming streams and every shape named in
§ 4.2 and § 4.3: interleaving, `n > 1`, multi-choice chunks, duplicate keys, case-folded keys,
`__proto__`, float, bool and negative indices, a repeated `name`, BOM, trimmed and unknown
fields, records after `[DONE]`, a missing final blank line, oversized records, `NaN`, and a
JSON body on a stream. Each is run under allow-all, block-all and block-by-name deciders,
with random chunking.

### 8.2 SDK conformance

The Rust models are checked against the real SDKs once, through golden files:

- `paigasus-gateway-core` writes a small set of rewritten golden streams to
  `tests/golden/*.sse`, gated by a drift test.
- A pytest module in `py/` runs the pinned `openai` and `anthropic` packages (dev
  dependencies) and asserts the calls and the notice text. Its moon task's `inputs` include
  the golden directory, so a change to the golden files selects it.
- The two first-party TS parsers are tested against the same golden files in their existing
  vitest suites.
- openai-node is covered by its Rust model only. It is not added as a dependency.

### 8.3 Other tests

- Unit tests per module. Fixtures go under `tests/fixtures/{openai,anthropic}/`: real
  recordings where available, hand-built ones labelled as such.
- Exhaustive single-split runs on fixtures of 4 KiB or less. Random multi-split runs on
  larger ones.
- Canary arguments: blocked calls use unique random argument strings. AC 3 checks that the
  canary is absent from the output.
- Gateway integration tests through the real router against the mock upstream:
  enabled/disabled, streaming/non-stream, allow-all identity, the block-by-name decider,
  upstream error mid-hold, the new 502, and billing unchanged on a block.
- Time-based behaviour (limits, keep-alive) is tested synchronously through `tick(now)`.
- T-perf: an `#[ignore]` timing test for 1 MiB of text-only stream, on versus off. The result
  is recorded in the PR. No CI threshold.

## 9. Acceptance criteria

1. With `enabled = false`, every existing gateway test passes unchanged and the interceptor is
   never constructed.
2. With `AllowAll`, D23 holds for every conforming fixture and every split of it. The
   malformed fixtures are excluded and covered by AC 5.
3. A blocked call's canary arguments never appear in the output, for every fixture, split and
   generated stream.
4. P1 and P2 hold over at least 10 000 generated cases per format in CI.
5. Each malformed and non-canonical shape in § 4.2 and § 4.3 has a fixture and the § 6
   behaviour under both `on_failure` values.
6. The golden files pass the pinned openai and anthropic Python SDKs and both first-party TS
   parsers (§ 8.2).
7. A decider returning the wrong number of decisions blocks every call of the hold.
8. The full gate graph from `CLAUDE.md` passes.

## 10. Spec challenge (2026-11-09) — changelog

The verdict was **NEEDS REWORK**: 7 blockers, 9 majors, 9 minors and 5 questions. Everything
was folded in except where noted.

- **Blockers folded in:**
  - the threat model and invariant (§ 2);
  - the single FIFO with touch sets (D12);
  - SSE re-framing (D2–D4, D23 redefined as payload identity);
  - strict JSON (D5);
  - total classification by a positive allowlist (D6, § 4.2);
  - canonical assembly (D9, § 4.3);
  - the decider length check (D18).
- **Majors folded in:**
  - the state machine sits before the error conversion, and truncation is not completion
    (D10, D11, D17);
  - per-fragment rewrite rules (D20, § 4.4);
  - the per-stream memory cap (D13);
  - 300 s plus a separate decider timeout (D14);
  - the owned decider arguments, `DecisionContext` and `ToolTurn::Unavailable` (D18, D19);
  - the sans-IO core (D1, § 5.1);
  - the closed Anthropic allowlist and dispatch on `data.type` (§ 4.2);
  - the differential oracle and canary-based ACs (§ 8.1, § 9);
  - the SDK conformance mechanism (§ 8.2);
  - the capped ASCII header and closed-set notice text (D21, D22).
- **Kernel placement, changed.** The types stay in gateway-core with `#[non_exhaustive]`, and
  `DecisionId` waits for SMA-755. This avoids a kernel release now (D1).
- **Partly adopted: SDK conformance.** The real Python SDKs and the first-party TS parsers are
  tested. openai-node is modelled in Rust and not added as a dependency, to keep a large npm
  dependency out of the ts workspace for one test.
- **Minors folded in:**
  - the full gate list (§ 7);
  - `deny_unknown_fields` (D26);
  - stream-level handling of oversized records (D8);
  - `Cow` / `RawValue` skeletons (§ 5.1);
  - the non-stream order and billing (§ 5.2);
  - the held trailing CR (D2);
  - the `Other` payload (§ 4.1);
  - `name_sha256_8` (§ 4.6);
  - the SDK `mapError` mapping (§ 7).
- **Questions, answered:**
  - Observe mode is specified in § 5.3.
  - The decider is called once per completed hold. SMA-753 orders by completion.
  - The SDK targets are pinned in § 8.2. Canonical form makes the SDKs agree.
  - The capability is not exposed (D28).
  - The caller is not an adversary (T3), which leaves request-side differentials to SMA-752.

## 11. Implementation rulings (2026-11-11)

The controller made these rulings during implementation. Each one changes or settles
behaviour that §§ 3–6 left open, and each fails closed. The sentences they made stale are
edited in place above (D12's reason, § 4.3, § 4.4, § 5.2).

**OpenAI classification and canonical form.**

- **`choice_order` (P-10 (a)).** A choice index seen for the first time must equal the number
  of choices seen so far, or the record is `Deviation("choice_order")`. openai-python finds a
  stream choice by list position, so choice 1 before choice 0 would merge two choices' calls.
- **Choice-0-only tool data.** A tool-call fragment (`tool_calls` or `function_call`) in a
  choice whose `index` is not 0 is `Deviation("multi_choice_tool_calls")`. Tool data in a
  choice entry at array position other than 0 of its chunk is `Deviation("choice_position")`.
  LangChain reads `choices[0]` of each chunk and ignores `choice.index`. **Limitation:** an
  `n > 1` request whose later choices call tools is not supported while interception is on. It
  gets `on_failure` (block, or pass-through in observe mode). The non-stream body applies the
  same rules, as `Malformed`.
- **Duplicate choice index.** A chunk that names one choice index twice is
  `Malformed("duplicate_choice_index")`.
- **The `object` rule.** Stream: a chunk that carries tool data or a `finish_reason` must have
  `object` exactly `"chat.completion.chunk"`, or it is `Malformed("object")`; openai-python
  skips any other chunk. Body: `object` must be `"chat.completion"` only when the body carries
  tool calls.
- **`type_shape_mismatch`.** A `type` that disagrees with the entry's body shape (`function` or
  `custom`) is a deviation, never kind `Other`: clients ignore `type` and run the body. Kind
  `Other` applies only when no `function` or `custom` body arrived.
- **`incomplete_arguments`.** At completion, OpenAI function and legacy-function arguments
  (stream and body) must be `""` or one complete JSON value; Anthropic `tool_use` input at
  `content_block_stop` the same. Otherwise the call is `Deviation("incomplete_arguments")`
  (`Malformed` in a body). `""` is presented to the decider as `"{}"`. Custom-tool input is
  free text and is not checked. Reason: LangChain and anthropic-python partial-parse truncated
  JSON, so the decider would see different arguments than the client runs.
- **`second_finish`.** A second `finish_reason` for one choice is a deviation, for tool-free
  choices too.
- **Arguments may be `null`** in a fragment (§ 4.3). It adds nothing, as in every SDK.
- **P-10 (b), never drop.** The rewrite never removes a choice entry and never drops a frame
  (§ 4.4), so the gateway's own rewrite cannot shift openai-python's positions.
- **Notice meta.** A notice chunk copies `id`, `created`, `model` and `system_fingerprint` only
  when they are strings or numbers, and always says `"object":"chat.completion.chunk"`. Only
  accepted records update the meta that the closing notice uses, so a dropped record cannot
  name it.
- **Prior OpenAI calls.** A request's prior assistant `tool_calls` entry reads `function.name`
  and `function.arguments`, or `custom.name` and `custom.input`. An entry with neither body,
  with both, or with a body that disagrees with its `type` makes the turn
  `Unavailable("malformed")`, as a `function` without a `name` does (D19).

**Anthropic.**

- **`block_interleave`.** A `content_block_start` while an earlier block is still open is a
  deviation. LangChain routes `input_json_delta` to the most recent start, not to
  `event.index`.
- **Synthesized stop frame.** A blocked block's `content_block_stop` is re-emitted as the
  synthesized `{"type":"content_block_stop","index":i}`, never the upstream payload.

**Bodies and numbers.**

- **`lossy_number`.** A body (either format) holding a number outside `u64`/`i64` that is an
  integral `f64` with magnitude of 2^53 or more is `Malformed("lossy_number")`. serde_json would
  round it, and the decider would see a different number than the client. The rule applies to
  stream kind-Other calls too, in both protocols: their arguments are the parsed raw entries, so
  a raw entry holding such a number makes the call's completion `Deviation("lossy_number")`.
- **`preserve_order`.** `paigasus-gateway-core` enables serde_json's `preserve_order`, so a
  re-serialized frame or body keeps the upstream key order in every build.

**Strict JSON scope (I-1).** The D5 envelope rules (case-folded classifier keys, `constructor`,
`prototype`) apply to envelope keys only. Tool argument and schema data gets JSON validity,
duplicate-key rejection at every depth, non-finite rejection and `__proto__` rejection, but not
the case-fold or `constructor`/`prototype` checks: the classifiers never read inside it, and a
tool may take an argument named `Id` or `constructor`. That data is OpenAI function and legacy
arguments, Anthropic `tool_use` and `server_tool_use` `input`, a request's
`tools[].function.parameters` and `functions[].parameters`, an Anthropic request's
`tools[].input_schema`, and prior-call inputs in the request. In a whole-record, body or
request parse, the value of an `input`, `parameters` or `input_schema` key gets these rules.

**Interceptor and limits.**

- **Wrong-length decision vectors.** A decision vector whose length differs from the calls'
  blocks every call of the hold with `DeciderError` (stream and body), not only the missing ones.
- **`after_done` before `oversized`.** After `[DONE]` (or `message_stop`) every record is
  dropped and counted as `after_done`, an oversized one included. It never becomes a stream
  failure.
- **Oversized records** are always dropped and counted with the new `records_dropped` reason
  `oversized` (§ 5.2), in both `on_failure` modes (P-3). With `allow` the stream then switches
  to pass-through.
- **Observed holds are bounded (D13).** A hold under observe mode (§ 5.3) keeps assembling for
  the report, but once it retains more than `max_held_bytes` of arguments and raw entries it
  drops them and is never decided: no decision and no report entry, as for a pass-through
  call. Its frames still flow. A completed hold keeps no arguments in either protocol.
- **86 400 s caps.** Every `[tool_interception]` `*_secs` value is capped at 86 400 by
  validation; `Instant + Duration` would otherwise panic on overflow.
