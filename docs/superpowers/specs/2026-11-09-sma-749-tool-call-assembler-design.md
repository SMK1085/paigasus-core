# SMA-749 — gateway: assemble, hold and rewrite tool calls in the response path

- Linear: SMA-749 (spec 1 of the SMA-747 umbrella, "Gateway tool-call interception").
- ADR: ADR-0027 (Accepted 2026-11-08), decisions (a) one check point, (b) hold only tool
  calls, (c) replace a blocked call with a notice, (e) evaluation in the gateway via kernel
  types.
- Status: draft, before the spec challenge.
- Scope: a new lib crate `paigasus-gateway-core`, a few I/O-free types in `paigasus-kernel`,
  and the wiring into `paigasus-gateway`'s `/v1/chat/completions` handler, off by default.
- Out of scope: the policy decision itself (SMA-752), session history (SMA-753), detectors
  (SMA-754), the decision log (SMA-755), the Anthropic ingress route (SMA-750), MCP (SMA-756),
  and any rewrite of the request body.

## 1. Problem

The gateway forwards model responses byte for byte. A response can contain tool calls: the
model asks the client's agent to run a function with some arguments. Today nothing between the
model and the client can look at a tool call, decide on it, or remove it.

ADR-0027 puts that check in the gateway. Every later spec (decisions, history, detectors, the
decision log) needs the same foundation first:

1. find every tool call in a response, streaming or not, for both OpenAI Chat Completions and
   Anthropic Messages;
2. hold a streaming tool call until its arguments are complete, while text keeps streaming;
3. ask a decider about the complete calls;
4. forward allowed calls unchanged and replace blocked ones with a notice, in a response the
   client's SDK still parses.

This spec builds that foundation with a decider that allows everything. It ships wired in and
off by default, so the hot path is exercised in tests and can be turned on per deployment.

## 2. Decisions

| Id | Question | Decision |
|----|----------|----------|
| D1 | Where the code lives | Kernel: `ToolCall`, `ToolCallKind`, `Decision`, `DecisionId`. New crate `paigasus-gateway-core`: the format converters, `ToolTurn`, `ToolCallAssembler`, the rewriters, the `ToolCallDecider` trait and `AllowAll`. |
| D2 | What counts as a tool call (OpenAI) | `choices[].delta.tool_calls[]` / `message.tool_calls[]` of type `function` or `custom`, AND the legacy `delta.function_call` / `message.function_call`. Unknown `type` values are tool calls of kind `Other`. |
| D3 | What counts as a tool call (Anthropic) | `content_block` of type `tool_use`. `server_tool_use` (and its `*_tool_result` blocks) pass through untouched and are reported as `unenforceable`. |
| D4 | Which records are parsed | Every SSE `data:` record, as JSON, when interception is on. No substring prefilter. |
| D5 | A record that does not parse | Dropped (not forwarded) and counted. The literal `[DONE]` is not JSON and is handled as the end marker. |
| D6 | A record over `max_record_bytes` | Never skipped. Triggers `on_failure` for the choice it belongs to, or for the stream when the choice is unknown. |
| D7 | When an OpenAI call is complete | At the `finish_reason` of its choice, or at `[DONE]` / end of stream. Never at "the next index started". |
| D8 | When an Anthropic call is complete | At `content_block_stop` for its block index. |
| D9 | What is held | From the first tool-call frame of a choice (OpenAI) or the `content_block_start` of a `tool_use` block (Anthropic), every later frame of the stream, in order, until the decision. Text before that point is forwarded at once. |
| D10 | Hold limits | `max_held_bytes` (default 1 MiB) and `max_hold_secs` (default 60) per hold. Over a limit → `on_failure`. |
| D11 | Keep-alive while holding | A `: keep-alive\n\n` SSE comment every `keepalive_secs` (default 10) while any hold is open and nothing else was sent. |
| D12 | `on_failure` | `"block"` (default) or `"allow"`. Applies on hold overflow, hold timeout, a decider error, an oversized record, and a 2xx non-stream body that does not parse. |
| D13 | Upstream error mid-hold | Drop the held frames, emit the existing `TERMINAL_SSE_ERROR`. No partial tool call is ever forwarded. |
| D14 | A blocked call, streaming OpenAI | Removed from the held frames. A synthesized `delta.content` chunk with the notice text is inserted. Allowed calls in the same choice are renumbered from 0. `finish_reason: "tool_calls"` becomes `"stop"` when no call remains. |
| D15 | A blocked call, non-stream OpenAI | Removed from `message.tool_calls` (or `function_call` removed). The notice is appended to `message.content` (set when it is null). `finish_reason` fixed as in D14. |
| D16 | A blocked call, Anthropic | The `tool_use` block becomes a `text` block at the same index (`content_block_start` / `text_delta` / `content_block_stop`, each with its `event:` line). `stop_reason: "tool_use"` becomes `"end_turn"` when no `tool_use` block remains. |
| D17 | Reporting decisions | Streaming: an SSE comment `: paigasus-decision {json}` per decision, before the released frames. Non-stream: a `paigasus-tool-decisions` response header (JSON array). Tool names are sanitized (D25). |
| D18 | Byte identity | With `AllowAll`, the concatenated bytes to the client equal the upstream bytes, minus nothing, plus only `:`-comment lines. Chunk boundaries may differ. |
| D19 | Billing | `UsageScanner` and `ChargeGuard` keep reading the upstream bytes before the assembler. A dropped or rewritten frame does not change the charge. |
| D20 | Cancellation | The assembler is a plain stream adapter inside the existing relay. Nothing is spawned. Client disconnect drops it, as today. |
| D21 | Request side | Parse the request into `ToolTurn` (definitions, earlier calls, tool results) and hand it to the decider. No request rewrite. |
| D22 | Config | `[tool_interception]` table, `enabled = false`. Only `/v1/chat/completions` is wired. The Anthropic converters are library code until SMA-750. |
| D23 | Interception off | The relay is exactly today's relay. No parsing, no allocation, no extra frames. |
| D24 | Fuzzing | `proptest` properties (random chunking, random mutation) instead of a `cargo-fuzz` target: the workspace is pinned to stable 1.95 and `cargo-fuzz` needs nightly. |
| D25 | Sanitizing names in reports | Tool names in decision reports are truncated to 128 bytes and restricted to `[A-Za-z0-9_.:-]`; any other byte becomes `_`. |

### Reasons

- **D1, kernel.** ADR-0005 keeps cross-language behaviour in the kernel. `ToolCall` and
  `Decision` are the shapes the IAM side, the SDKs and the decision log (SMA-755) will share.
  They are plain data with no I/O, so they fit the kernel's `uuid` + `thiserror` dependency
  set. They are Rust-only for now; the PyO3/napi/wasm bindings do not export them until a
  binding needs them. `Finding` (SMA-754) is not added before it has a user.
- **D1, new crate.** The SSE and JSON handling is gateway-specific, too large for the
  kernel, and must be testable without axum, reqwest or Redis. A lib crate keeps the service
  crate's hexagonal shape: `paigasus-gateway` is the adapter, `paigasus-gateway-core` the
  domain.
- **D2, legacy `function_call`.** Clients that still send `functions` receive
  `function_call`, not `tool_calls`. Missing it would let a whole client class bypass every
  check.
- **D3, server tools.** Anthropic runs server tools (web search, code execution) itself before
  the gateway sees the event. Blocking them in the response is theatre: the call already ran.
  They are reported so an operator can see them, and SMA-752 may refuse them on the request
  side (by stripping the server-tool definition) instead.
- **D4, no prefilter.** JSON allows `"tool_calls"`. A `contains("tool_calls")` shortcut
  would miss it while every client JSON parser decodes it. Parsing every record costs one
  `serde_json::from_slice` into a borrowed `RawValue` skeleton per record; the measurement is
  part of the plan (§ 8, T-perf).
- **D5, drop unparseable.** Python's `json` accepts `NaN` and `Infinity`; `serde_json` does
  not. A record the gateway cannot parse but the client can is a channel for an unchecked
  tool call. Dropping is safe because a well-formed upstream never sends such records. A
  counter (`gateway_tool_interception_dropped_records_total{reason}`) makes it visible.
- **D6, no skip.** `UsageScanner` skips records over 64 KiB because a missed usage record only
  costs an estimate. Here a skipped record is a forwarded, unchecked record. The assembler
  has its own `max_record_bytes` (default 1 MiB, equal to `max_held_bytes`).
- **D7, no early release.** OpenAI itself sends a call's argument fragments in order, but
  OpenAI-compatible upstreams (vLLM, LiteLLM, some routers) are known to interleave indices.
  Releasing call 0 when index 1 starts would decide on partial arguments, and fragments of
  call 0 arriving later would then reach the client unchecked. Deciding per choice at
  `finish_reason` costs latency only for the first call of a parallel batch, which the client
  cannot run before the turn ends anyway.
- **D7, `n > 1`.** Holds are keyed by choice index. One choice holding does not hold another
  choice's text.
- **D9, hold everything after.** Frames after a tool-call frame can include text of the same
  choice (some upstreams emit text after calls) and frames of other choices. Re-ordering
  them would change what the client sees. Holding the tail in order keeps the stream's order
  intact; the cost is that other choices' text pauses during a hold, which is accepted for
  `n > 1` streams with calls (rare).
- **D10, limits.** A hold is memory the client controls indirectly (by prompting a model into
  huge arguments). The caps bound it. 60 s matches the default upstream read timeout.
- **D11, keep-alive.** Holding a large call can take longer than a proxy or client idle
  timeout. SSE comments are ignored by every conforming client.
- **D12, default block.** Fail closed is the point of the feature. `"allow"` exists for an
  operator who turns interception on first in observe mode.
- **D14-D16, notice instead of error.** ADR-0027 (c): the agent loop keeps running, and the
  model sees on its next turn that the call was refused. The rewrite keeps the response
  valid for the official OpenAI and Anthropic SDKs: copied `id`, `created`, `model`,
  `system_fingerprint`; contiguous tool-call indices; a `finish_reason` that matches the
  content.
- **D17, comments and header.** Neither changes the body schema. The header is sanitized
  because tool names come from the model.
- **D18, byte identity.** It is the strongest regression check available: if allow-all
  changes a single byte of payload, something parses and re-serializes where it must not.
  Only frames that a block rewrites are re-serialized.
- **D19, billing.** The upstream produced and billed the tokens of a blocked call. The
  charge follows the upstream, as SMA-677 and SMA-730 already decided.
- **D21, request side.** The decider needs the tool definitions (to know a tool's declared
  schema) and the earlier calls and results (for sequence rules in SMA-753). Rewriting the
  request is SMA-752's job.
- **D22, wired but off.** User decision 2026-11-09. Wiring now proves the integration and the
  performance claim in D23; off by default means no deployment changes behaviour.

## 3. Components

### 3.1 Kernel (`rs/crates/libs/paigasus-kernel/src/tool_call.rs`)

```rust
pub enum ToolCallKind { Function, Custom, LegacyFunctionCall, AnthropicToolUse, Other(String) }

pub struct ToolCall {
    pub id: Option<String>,      // absent for legacy function_call
    pub name: String,
    pub kind: ToolCallKind,
    pub arguments: String,       // raw, exactly as assembled; not parsed here
}

pub enum Decision {
    Allow,
    Block { code: BlockCode, reason: String },
}

pub enum BlockCode { Policy, Detector, HoldLimit, Unparseable, DeciderError }

pub struct DecisionId(/* dcs_<uuid7> */);
```

`DecisionId` reuses the kernel's `uuid7` module and the resource-name prefix rules. No new
dependency.

### 3.2 `paigasus-gateway-core`

| Module | Holds |
|---|---|
| `sse` | A byte-level SSE record splitter (CR, LF, CRLF; multi-line `data:` joined with `\n` per WHATWG; `event:` kept; comments and unknown fields kept verbatim for forwarding). Unlike `UsageScanner`, it has no skip path. |
| `openai` | Request → `ToolTurn`; stream frame → frame events; non-stream body → calls; rewriters (D14, D15). |
| `anthropic` | The same for Messages (D16). Library only. |
| `turn` | `ToolTurn { definitions, prior_calls, tool_results }`. |
| `assembler` | `ToolCallAssembler`: per-choice / per-block state machine, holds, limits, release. |
| `decider` | Object-safe trait (held as `Arc<dyn ToolCallDecider>`): `fn decide<'a>(&'a self, turn: &'a ToolTurn, calls: &'a [ToolCall]) -> BoxFuture<'a, Result<Vec<Decision>, DeciderError>>`, one `Decision` per call, in order. `AllowAll`. |
| `notice` | The notice text: `[paigasus] tool call "<sanitized name>" was blocked: <reason>`. |
| `metrics` | Counter names only; the gateway registers them. |

Dependencies: `serde`, `serde_json` (with `raw_value`), `bytes`, `futures`, `thiserror`,
`paigasus-kernel`. No tokio, axum or reqwest. Timers are injected (a `Clock` / sleep factory
argument) so the crate stays runtime-free and tests are deterministic.

### 3.3 Gateway wiring

- `config.rs`: `ToolInterceptionConfig { enabled, max_held_bytes, max_record_bytes,
  max_hold_secs, keepalive_secs, on_failure }` with validation (all non-zero; `keepalive_secs
  < max_hold_secs`).
- `chat.rs`: when enabled, parse the request copy into `ToolTurn` (a parse failure here is not
  a new error: the request is forwarded as today and the turn is empty). Streaming: the
  relay's output stream goes through the assembler after `UsageScanner` has seen the upstream
  bytes. Non-stream: the full body goes through the non-stream path before the response is
  built.
- `service_info.rs`: `Capabilities { chat_stream, tool_interception }`.
- The decider is `Arc<dyn …>` in `AppState`, `AllowAll` for now.

## 4. Data flow (streaming, OpenAI)

```
upstream bytes ──► UsageScanner (billing, unchanged)
              └──► sse splitter ──► record
                                     │ parse fails ─► drop + count
                                     ▼
                         frame events per choice
          text-only, no hold open ─► forward original bytes
          tool-call frame / hold open ─► append to hold (bytes + parsed)
          finish_reason / [DONE] ─► hold complete ─► decider ─► release
                                                               │
                         all allow ─► original held bytes      │
                         any block ─► rewritten frames + notice│
                         + `: paigasus-decision` comments ◄────┘
```

Non-stream: parse body → calls → decider → allow-all returns the original bytes unchanged;
any block re-serializes the body once.

## 5. Error handling

| Case | Behaviour |
|---|---|
| Unparseable record (stream) | Drop, count (D5). |
| Record over `max_record_bytes` | `on_failure` (D6). With `block`: every open hold is blocked with `HoldLimit`; a record outside any hold is dropped. |
| Hold over `max_held_bytes` / `max_hold_secs` | `on_failure` for that hold. With `block`, the calls are blocked with `HoldLimit` and later fragments of them are dropped (D7 late-fragment rule). |
| Decider error | `on_failure`, code `DeciderError`. |
| Upstream error mid-hold | D13. |
| Non-stream 2xx body that does not parse | `on_failure`: `block` returns a 502 with a new registry error code `gateway.tool_interception.unparseable_response`; `allow` forwards it. |
| Non-2xx upstream | Forwarded as today. No interception of error bodies. |
| Late tool-call fragment after a decision | Dropped, counted (`reason="late_fragment"`). |

## 6. Repo gates the new crate touches

- `rs/Cargo.toml` `members`: one literal line (SMA-663 A11).
- `ci/affected-graph/run.sh`: the `lockfile->all-lint` set.
- the crate's `moon.yml`: `fileGroups.upstreams` and `dependsOn` per the existing lib crates.
- `contracts/` error registry `MANIFEST` for the new error code (ADR-0019).
- Kernel: the new module follows the kernel's `[lints]` (Check 1c) and its publish metadata.
- `paigasus-gateway-core` is `publish = false`.

## 7. Acceptance criteria

1. With `enabled = false`, every existing gateway test passes unchanged and the relay code
   path does not call into `paigasus-gateway-core`.
2. With `enabled = true` and `AllowAll`, D18 holds for every fixture in § 8 and for every
   byte-boundary split of each.
3. A blocked call never reaches the client: no byte of its arguments appears in the output,
   for every fixture and split.
4. Rewritten responses parse with the official `openai` (Python and Node) and `anthropic`
   SDKs' stream parsers, and the SDK sees the notice as text and no tool call.
5. Interleaved indices, `n > 1`, legacy `function_call`, text after calls, unicode-escaped
   keys, `NaN` records, oversized records and upstream errors mid-hold each have a fixture.
6. `proptest`: for arbitrary chunking and single-byte mutations of the fixtures, the
   assembler never panics, never forwards an unparsed record, and never forwards a byte of a
   blocked call.
7. Hold limits and keep-alives are tested with an injected clock.
8. The full gate graph from `CLAUDE.md` passes.

## 8. Testing

- Unit tests per module in `paigasus-gateway-core`, with recorded fixtures under
  `tests/fixtures/{openai,anthropic}/` (real recordings where available; hand-built for
  interleaving and malformed cases, labelled as such).
- A split harness that feeds each fixture at every single split point and at random
  multi-split points.
- SDK conformance (AC 4): a small script under the crate's `tests/sdk/` run by a moon task in
  the py and ts workspaces against the rewritten fixture bytes. No network.
- Gateway integration tests through the real router against the existing mock upstream:
  enabled/disabled, streaming/non-stream, allow-all identity, a test-only `BlockNamed`
  decider.
- T-perf: a criterion-free micro-benchmark (a `#[test] #[ignore]` timing loop) for a 1 MiB
  text-only stream with interception on vs off, recorded in the PR. No CI threshold.
