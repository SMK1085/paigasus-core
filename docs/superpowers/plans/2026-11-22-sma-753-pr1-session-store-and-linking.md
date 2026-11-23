# SMA-753 PR 1: Session Store and Session Linking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the gateway a trusted, append-only tool history with session linking, decision references in block notices, and the three history findings, with no effect on any policy decision (spec D15, PR 1).

**Architecture:** A `SessionStore` port in the gateway domain, with a memory adapter and a Redis adapter (Lua scripts on `paigasus-redis`), both proven by one contract suite. A `ToolHistory` application service links each request to a session on the request side (read only), writes after the limits admission, reserves pending call events inside the decide future, and settles them at the report site on a `TaskTracker`. The decider makes one decision id per call. The same id is the event id, the `ref=` in the notice and the decision record id.

**Tech Stack:** Rust edition 2024 (rust-version 1.95), tokio, `async-trait`, `redis` + `paigasus-redis`, `tokio-util` `TaskTracker`, `metrics`, `testcontainers-modules` Redis, `paigasus-test-docker`, cargo-nextest.

**Spec:** `docs/superpowers/specs/2026-11-22-sma-753-session-history-design.md` (revision 2, approved). Read it with this plan. This plan does not change a decision of the spec.

## Global Constraints

- Every new source file starts with `// SPDX-License-Identifier: Apache-2.0`.
- Commits: conventional, with a scope (`feat(rs): …`, `test(rs): …`, `docs(ops): …`). No line in a commit body starts with `#` and a number. End each commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- `cargo clippy --workspace -- -D warnings` must pass after every task. Dead code is a hard error: a task adds no `pub(crate)` or private item that only a later task uses. Public items of a library crate are not dead code.
- `cargo fmt --check` must pass after every task.
- Redis clients come only from `paigasus_redis::connect` (`repo:redis-connect-single-site`). Tests read Redis only with `redis-cli` through `container.exec`.
- Every new gateway Docker test binary goes into the gateway override of `rs/.config/nextest.toml` (`test-group = 'docker-containers'`).
- No `Bytes` parameter in a function of `adapters/http/` (`repo:http-extractor-envelope`).
- Bash tool PATH: prefix commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Do not install software on the host. Do not bump crate versions by hand: release-plz versions `paigasus-kernel` (`.github/CLAUDE.md`).
- Constants (spec D4, D11, D12, § 7): link id cap **128** per list; **10,000** events per `(history, tool)`; longest window **24 h** (`86_400_000` ms); retention **24 h + 1 h** (`90_000_000` ms); key TTL **90,000 s** from the last write; Redis key prefix **`paigasus:gateway:history:v1:`**; `[tool_history] deadline` default **`"1s"`**, strictly less than `tool_interception.decide_timeout_secs`; `BreakerMetrics` role **`tool_history`**.
- Finding names: **`mixed_sessions`**, **`forged_history`**, **`history_truncated`**.
- Notice form: **`[paigasus] tool call "<name>" was blocked (<code>). ref=<uuid>`**. A block without a decision id keeps the old form with no suffix.
- Metric names (D13): **`gateway_tool_history_store_errors_total{op, kind}`** (`op` ∈ `lookup`, `write`, `reserve`, `settle`, `append`; `kind` ∈ `io`, `server`, `decode`, `deadline`), **`gateway_tool_history_ids_capped_total{list}`** (`list` ∈ `calls`, `results`, `refs`), **`gateway_tool_history_events_trimmed_total{reason}`** (`reason` ∈ `retention`, `cap`).
- PR 1 evaluates no temporal history conditions. The history findings and the session reach response-side Cedar. A store error is counted and the request continues. The gateway loads a policy set with history conditions and leaves its slots empty, per decision D16 (Sven, 2026-11-22). The SMA-775 R5 refusal in `adapters/policy_source/iam.rs` is removed. IAM still refuses saves with conditions, and `HISTORY_CONDITIONS_ENFORCED_BY_GATEWAY` stays unchanged.
- PR 1 events store no argument `fields` and no `bad` flag (spec D5 to D7). Both need the condition paths and the kernel input mapping of PR 2. The event encoding has a version and readers ignore unknown fields, so PR 2 adds them without a migration.

## Review Focus

1. **A decide timeout under `on_failure = allow` after the reserve ran.** The reserved event stays `pending` and the report site appends a second, allowed event. The call counts twice, never zero (spec D8: a lost settle never lowers a count). Test: Task 12 `a_timed_out_hold_under_allow_appends_and_keeps_the_pending_event`.
2. **A notice inside other assistant text, or a changed notice.** Chat puts the notice after the model text (`"<text>\n\n<notice>"`). Extra text on the line gives no reference. `notice_ref` checks only the syntax: a malformed name, an unknown block code or a malformed UUID gives none. A changed tool name or code still gives the UUID. The session lookup then rejects it: a reference counts only when the link table has it for the same principal in the same org. Test: Task 4 `a_ref_is_read_only_from_an_exact_notice_line`.
3. **A slow or blackholed Redis on the request side.** One lookup can spend the whole budget. The request continues with no session, and every later store call of that request is skipped. Test: Task 10 `a_slow_lookup_spends_the_budget_and_skips_the_rest`.
4. **The same tool result sent twice**, in one request or in two requests. Only one result event is written. Tests: Task 6 `a_result_sent_twice_in_one_request_is_linked_once`, Task 7 contract case `a_result_event_is_written_once_per_call_id`.
5. **Hostile call ids** (very long, non-ASCII, with `:` or `*`). Key names use a SHA-256 hex digest, so the length and the characters are fixed. Test: Task 6 `link_digest_is_64_lower_hex_for_any_id`.

---

### Task 1: The three finding constants and their metric labels

**Files:**
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/types.rs` (next to `NAME_COLLISION`, line 12)
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/mod.rs:19` (re-export)
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/decider.rs:42-62` (`FINDING_KINDS`, `finding_label`)
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs:77-79` (doc comment of `GATEWAY_TOOL_POLICY_FINDINGS_TOTAL`)
- Modify: `docs/ops/RUNBOOK-observability.md:150` (the `gateway_tool_policy_findings_total` row)

**Interfaces:**
- Produces: `paigasus_kernel::tool_policy::{MIXED_SESSIONS, FORGED_HISTORY, HISTORY_TRUNCATED}: &str` with the values `"mixed_sessions"`, `"forged_history"`, `"history_truncated"`.
- Produces: `FINDING_KINDS: [&str; 6] = ["name_collision", "unparsed_arguments", "mixed_sessions", "forged_history", "history_truncated", "other"]`. `"other"` stays last.

- [ ] **Step 1: Write the failing tests**

In `decider.rs` tests:

```rust
#[test]
fn each_history_finding_has_its_own_label() {
    use paigasus_kernel::tool_policy::{FORGED_HISTORY, HISTORY_TRUNCATED, MIXED_SESSIONS};
    assert_eq!(finding_label(MIXED_SESSIONS), "mixed_sessions");
    assert_eq!(finding_label(FORGED_HISTORY), "forged_history");
    assert_eq!(finding_label(HISTORY_TRUNCATED), "history_truncated");
    assert_eq!(finding_label("something_new"), "other");
    assert_eq!(FINDING_KINDS.len(), 6);
    assert_eq!(FINDING_KINDS[5], "other");
}
```

Extend the existing priming test (or add `priming_covers_twelve_pairs`): after `prime_tool_policy_metrics()`, every `(side, kind)` of `["request","response"] × FINDING_KINDS` reads 0.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib finding`
Expected: FAIL to compile (`MIXED_SESSIONS` not found).

- [ ] **Step 3: Add the constants and the labels**

Add the three `pub const`s with one-line docs in the style of `NAME_COLLISION`, and re-export them in `tool_policy/mod.rs`. Extend `FINDING_KINDS` and the `match` in `finding_label` with the kernel constants. Update the doc comment in `names.rs` and the runbook row: the six kinds, and "primed at 0 for all twelve pairs".

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib finding && cargo nextest run -p paigasus-kernel`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy -p paigasus-kernel -p paigasus-gateway --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/libs/paigasus-kernel/src/tool_policy rs/crates/services/paigasus-gateway/src/application/tool_policy/decider.rs rs/crates/libs/paigasus-observability/src/names.rs docs/ops/RUNBOOK-observability.md
git commit -m "feat(rs): add the three history finding kinds and their metric labels (SMA-753)"
```

The kernel is `publish = true`. Do not change its version: release-plz cuts the release. `repo:version-lockstep` stays green because no version string changes.

---

### Task 2: A decision reference in the block notice (gateway-core)

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/src/report.rs` (`notice_line`, new `Blocked`, new `notice_ref`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/protocol.rs` (`Verdict::PerCall`, new `Verdict::blocked`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/decider.rs` (`DecidedCall.decision_id`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/decision_log/record.rs:196-211` (`ReportedCall.decision_id`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/interceptor.rs:361-412` (`apply_decision`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/body.rs:78-160` (`code`, `apply`, `reported`)
- Modify: the six notice sites: `openai/body.rs:85-125`, `openai/stream.rs:105-121`, `anthropic/body.rs:88-100`, `anthropic/stream.rs:370-425`, `responses/body.rs:92-100`, `responses/stream.rs:60-110, 218`
- Modify: `rs/crates/libs/paigasus-gateway-core/tests/golden.rs:96,148` (new `notice_line` arity)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/interception.rs:153-156` (`calls.apply(&decisions)`)

**Interfaces:**
- Produces: `pub fn notice_line(name: &str, code: BlockCode, reference: Option<Uuid>) -> String`. With `Some(id)`: the old text plus `" ref="` and the hyphenated lower-case uuid.
- Produces: `pub(crate) fn notice_ref(line: &str) -> Option<Uuid>`. It returns `Some` only for one whole line in the exact form `[paigasus] tool call "<n>" was blocked (<code>). ref=<uuid>`, where `<n>` is 1 to 128 bytes of `[A-Za-z0-9_.:-]`, `<code>` is one of `BlockCode::ALL` in kebab case, and `<uuid>` is 36 characters that `Uuid::try_parse` accepts. Task 4 uses it.
- Produces: `pub(crate) struct Blocked { pub code: BlockCode, pub reference: Option<Uuid> }`.
- Produces: `Verdict::PerCall(Vec<(Decision, Option<Uuid>)>)` and `pub(crate) fn blocked(&self, slot: usize) -> Option<Blocked>`. `block_code(slot)` stays as `self.blocked(slot).map(|b| b.code)`. `BlockAll` and a missing slot give `reference: None`.
- Produces: `DecidedCall { pub decision, pub detail, pub decision_id: Option<Uuid> }`, `DecidedCall::new` sets `None`, and `pub fn with_decision_id(self, id: Uuid) -> Self`.
- Produces: `ReportedCall { pub entry, pub call, pub detail, pub decision_id: Option<Uuid> }`, `ReportedCall::new` sets `None`, and `pub fn with_decision_id(self, id: Option<Uuid>) -> Self`.
- Produces: `pub fn apply(self, decided: &[DecidedCall]) -> Option<Vec<u8>>` on `BodyCalls` (was `&[Decision]`). The three `rewrite` functions take `codes: &[Option<Blocked>]`.

Rules:
- The ref goes on a notice only for a block that the decider made (`Decision::Block` with a decision id). A forced `Truncated` of a Responses body item, `BlockAll`, `hold-limit`, `truncated`, `malformed` and `decider-error` blocks get no ref (spec D1).
- `ReportedCall.decision_id` is the decider's id whenever the decision vector is aligned with the calls, also for a forced `Truncated` call. The settle of Task 12 needs it to mark that event `deny`.

- [ ] **Step 1: Write the failing tests**

In `report.rs` tests:

```rust
const ID: &str = "0193f6c2-8a4e-7b10-9c2d-5e8f1a2b3c4d";

#[test]
fn the_notice_carries_the_reference() {
    let id = Uuid::try_parse(ID).unwrap();
    assert_eq!(notice_line("send", BlockCode::Policy, Some(id)), format!("[paigasus] tool call \"send\" was blocked (policy). ref={ID}"));
    assert_eq!(notice_line("send mail", BlockCode::HoldLimit, None), "[paigasus] tool call \"send_mail\" was blocked (hold-limit).");
}

#[test]
fn notice_ref_reads_only_the_exact_form() {
    let id = Uuid::try_parse(ID).unwrap();
    for code in BlockCode::ALL {
        assert_eq!(notice_ref(&notice_line("ns.tool", code, Some(id))), Some(id));
    }
    for bad in [
        format!("x [paigasus] tool call \"send\" was blocked (policy). ref={ID}"),
        format!("[paigasus] tool call \"send\" was blocked (policy). ref={ID} "),
        format!("[paigasus] tool call \"se nd\" was blocked (policy). ref={ID}"),
        format!("[paigasus] tool call \"send\" was blocked (nope). ref={ID}"),
        format!("[paigasus] tool call \"\" was blocked (policy). ref={ID}"),
        "[paigasus] tool call \"send\" was blocked (policy). ref=not-a-uuid".to_owned(),
        "[paigasus] tool call \"send\" was blocked (policy).".to_owned(),
    ] {
        assert_eq!(notice_ref(&bad), None, "{bad}");
    }
}
```

At each of the six sites, add one test that a decider block with `Some(id)` gives a notice ending in `ref=<id>`, and that a block without an id gives the old text. Name them `<format>_<path>_notice_has_the_ref`, for example `openai_body_notice_has_the_ref` and `responses_stream_notice_has_the_ref`. In `interceptor.rs` tests: `the_decision_id_reaches_the_reported_action` (each `ReportedCall.decision_id` equals the id of its `DecidedCall`), and `a_decider_error_reports_no_decision_id`. In `body.rs` tests: `a_forced_truncated_call_keeps_its_decision_id_but_gets_no_ref`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core`
Expected: FAIL to compile (`notice_line` takes 2 arguments, `notice_ref` not found).

- [ ] **Step 3: Implement**

Add the fields, builders and `Blocked`. In `apply_decision`, build `Verdict::PerCall` from `(d.decision, d.decision_id)` and pass `d.decision_id` to `ReportedCall::with_decision_id`. `BodyCalls::code` returns `Option<Blocked>`: a forced code has `reference: None`. Change the six sites to pass `Blocked.reference`. Give the tests of `protocol.rs` users one helper `fn per_call(d: Vec<Decision>) -> Verdict` so the existing tests stay short. Update `golden.rs` to pass `None` (the golden driver decides without ids, so the golden files do not change). Update the one service call site to `calls.apply(&decisions)`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core && cargo nextest run -p paigasus-gateway --lib`
Expected: PASS. The golden `.sse` files are unchanged (`git status` shows no change under `tests/golden/`).

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy -p paigasus-gateway-core -p paigasus-gateway --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/libs/paigasus-gateway-core rs/crates/services/paigasus-gateway/src/adapters/http/interception.rs
git commit -m "feat(rs): put the decision reference into the block notice (SMA-753)"
```

---

### Task 3: One decision id from the decider to the decision record

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/application/decision_log.rs:112-140` (`header`, `emit`, `record_response`, new `mint_decision_id`)
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/decider.rs:76-122` (`decide_now`, `decide`)
- Modify (notice assertions that now carry `ref=`): `rs/crates/services/paigasus-gateway/tests/tool_policy.rs`, `tests/tool_interception.rs`, `tests/responses_proxy.rs`
- Test: `rs/crates/services/paigasus-gateway/src/adapters/http/interception.rs` tests

**Interfaces:**
- Consumes: `DecidedCall::with_decision_id`, `ReportedCall.decision_id` (Task 2).
- Produces: `pub fn mint_decision_id(&self) -> Uuid` on `DecisionRecorder`: `mint_uuid7(unix_ms(self.clock.now()), self.entropy.bytes10())`.
- Produces: `record_response` uses `r.decision_id` as `RecordHeader.decision_id` when it is `Some`, else it mints one as before. `record_request` and `record_refusal` do not change.

- [ ] **Step 1: Write the failing tests**

In `decision_log.rs` tests: `a_reported_decision_id_is_the_record_id` (a `ReportedCall` with `with_decision_id(Some(id))` gives a record with `decision_id == id`; one without gives a fresh uuid7 as before).

In `decider.rs` tests: `the_decider_gives_every_call_its_own_decision_id` (four calls give four distinct `Some` ids, each a UUIDv7: `id.get_version_num() == 7`).

In `interception.rs` tests: `the_notice_ref_is_the_record_id_on_the_stream_path` and `the_notice_ref_is_the_record_id_on_the_body_path`. Use the `CedarDecider` over a memory recorder (the existing `cedar_ctx` and `memory_recorder` helpers), let the floor block `rm`, parse the `ref=` from the client bytes, and assert that it equals the `decision_id` of the one block record in `MemoryDecisionLog::records()`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib decision_id notice_ref`
Expected: FAIL (`mint_decision_id` not found).

- [ ] **Step 3: Implement**

`header(&self, org, id: Option<Uuid>)`. In `decide_now`, mint one id per call and attach it with `with_decision_id`. Then update the exact notice strings in the three integration test files: a Cedar block now ends in ` ref=<uuid>`. Match the prefix and parse the uuid; do not hard-code an id.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway`
Expected: PASS (Docker suites skip without Docker; that is fine here).

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy -p paigasus-gateway --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): use the decider's decision id for the record and the notice (SMA-753)"
```

---

### Task 4: The newest part of the conversation in the parsed turn

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/src/turn.rs` (`TurnFacts`, `openai`, `anthropic`, `responses`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/lib.rs:33` (exports)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/types.rs` (`ToolCall::item_id`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/decision_log/record.rs:177-194` (`CallFacts.item_id`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/decision_log/build.rs:258` (the test helper `call()` sets `item_id: None`)

**Interfaces:**
- Consumes: `notice_ref` (Task 2).
- Produces:

```rust
pub const MAX_LINK_IDS: usize = 128;

#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LatestTurn {
    /// The ids of the tool calls in the last assistant message, at most MAX_LINK_IDS.
    pub call_ids: Vec<String>,
    /// The call ids of the tool results after that message, at most MAX_LINK_IDS.
    pub result_call_ids: Vec<String>,
    /// The `ref=` ids in the text of that message, at most MAX_LINK_IDS.
    pub refs: Vec<Uuid>,
    pub dropped_calls: usize,
    pub dropped_results: usize,
    pub dropped_refs: usize,
}
```

and `TurnFacts.latest: LatestTurn` (default empty).

- Produces (local shell, spec D4): `pub fn item_id(&self) -> Option<String>` on `ToolCall`. For `ToolCallKind::LocalShell` it is the string `id` of the item JSON in `arguments` (`responses/assembly.rs` `tool_call` keeps the whole item, without `status`, in `arguments`; `id` holds the `call_id`). For every other kind, and for a missing or non-string `id`, it is `None`. `ToolCall` gets no new field, so the 22 struct literals do not change.
- Produces: `CallFacts { pub kind, pub id, pub namespace, pub name, pub item_id: Option<String> }`; `CallFacts::of` sets `item_id: call.item_id()`. Task 10 uses it. Why: a later `local_shell_call_output` names its call by its own `id` (`ToolResultRef.item_id`), and that value can be the call's `call_id` or the call's item `id` (`policy/plan.rs:28-29`). A `c:` link under the `call_id` alone would give `forged_history` for the second form.

Rules (spec D4, D1):
- Chat: the last message with `role: "assistant"`. Call ids: its `tool_calls[].id`. A legacy `function_call` has no id and is skipped. Results: `role: "tool"` messages after it, by `tool_call_id`. A `role: "function"` result is ignored. Text: `content` as a string, or the `text` of each `{"type":"text"}` part.
- Anthropic: the last message with `role: "assistant"`. Call ids: its `tool_use` block ids. Results: `tool_result` blocks of later messages, by `tool_use_id`. Text: `content` as a string, or `text` blocks.
- Responses: the last model turn is the longest run of contiguous input items that ends at the last item which is an assistant `message` or a client-run call item. The run holds assistant messages, `reasoning` items and client-run call items. Call ids: the `call_id` of each call item (`id` for an `mcp_approval_request`, as `assembly::tool_call` gives it). Results: result items after the first item of the run, by `call_id`, by `approval_request_id`, or by `id` for a `local_shell_call_output` that has no `call_id`. Text: `output_text` parts of the assistant messages, and a string `content`.
- Refs: split each text on `\n` and keep the lines where `notice_ref` returns `Some`.
- Keep the first 128 entries of each list. Count the rest in `dropped_*`.
- A request with no assistant message gives an empty `LatestTurn`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn chat_latest_turn_reads_the_last_assistant_message_and_later_results() { /* two assistant messages; only the second one's ids, and only results after it */ }
#[test]
fn anthropic_latest_turn_reads_tool_use_and_later_tool_result_blocks() {}
#[test]
fn responses_latest_turn_reads_the_last_run_of_model_items() { /* reasoning + function_call + output; an older run is not read */ }
#[test]
fn a_local_shell_output_without_call_id_uses_its_item_id() {}
#[test]
fn a_legacy_function_result_and_a_legacy_function_call_are_ignored() {}
#[test]
fn refs_are_read_from_every_text_form() { /* chat string, chat parts, anthropic string, anthropic text blocks, responses output_text, responses string content */ }
#[test]
fn a_ref_is_read_only_from_an_exact_notice_line() {
    // "model text\n\n<notice with ref>" gives the ref; the same notice with a bad uuid, an unknown
    // code, a malformed name or trailing text gives none; a notice in a user message gives none.
    // A changed (but well-formed) name or code still gives the uuid: the session lookup rejects it, not notice_ref.
}
#[test]
fn each_list_is_capped_at_128_and_counts_the_rest() {
    // 130 tool calls, 129 results, 131 notices: lengths 128/128/128, dropped 2/1/3.
}
#[test]
fn a_request_without_an_assistant_message_has_an_empty_latest_turn() {}
#[test]
fn a_local_shell_call_gives_its_item_id() {
    // A Responses local_shell_call {"id":"lsh_1","call_id":"call_1",…}: id() == "call_1", item_id() == "lsh_1",
    // CallFacts::of(..).item_id == Some("lsh_1"). A function call: item_id() == None.
}
```

Assert that `format!("{:?}", facts)` never holds result content (as the existing `an_openai_request_yields_definitions_calls_and_results` test does).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core turn::`
Expected: FAIL to compile (`latest` not a field).

- [ ] **Step 3: Implement the three parsers**

One private helper `fn push_capped<T>(list: &mut Vec<T>, dropped: &mut usize, v: T)` and one `fn refs_in(text: &str, out: &mut LatestTurn)`. Track the index of the last assistant message while you walk `messages` or `input`, then read results from the later entries. Export `LatestTurn` and `MAX_LINK_IDS` from `lib.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy -p paigasus-gateway-core --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/libs/paigasus-gateway-core/src
git commit -m "feat(rs): read the newest call ids, result ids, notice refs and local shell item ids (SMA-753)"
```

---

### Task 5: The session in the decision context

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/session.rs`
- Modify: `rs/crates/libs/paigasus-gateway-core/src/decider.rs` (`DecisionContext.session`, `with_session`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/policy/calls.rs:98-135` (`decide_calls_with`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/decision_log/build.rs:48-53` (`session_of`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/lib.rs`, `src/policy/mod.rs` (exports)

**Interfaces:**
- Produces:

```rust
/// The store time budget of one request (spec D10 "Deadline"). No clock: callers pass durations.
#[derive(Debug)]
pub struct StoreBudget { /* limit: Duration, spent_nanos: AtomicU64, failed: AtomicBool */ }
impl StoreBudget {
    pub fn new(limit: Duration) -> Self;
    /// The time left, or None when the budget is spent or a store call failed.
    pub fn remaining(&self) -> Option<Duration>;
    pub fn charge(&self, spent: Duration);
    pub fn fail(&self);
    pub fn is_failed(&self) -> bool;
}

/// The session of one request (spec D4).
#[non_exhaustive]
#[derive(Debug)]
pub struct SessionContext {
    pub org: Uuid,
    pub session_id: Uuid,
    /// `SessionInfo::new(session_id.to_string(), turn)`.
    pub info: SessionInfo,
    /// Request-level findings: `mixed_sessions`, `forged_history`.
    pub findings: Findings,
    pub budget: StoreBudget,
}
impl SessionContext {
    pub fn new(org: Uuid, session_id: Uuid, turn: i64, findings: Findings, budget: StoreBudget) -> Self;
}
```

- Produces: `DecisionContext.session: Option<Arc<SessionContext>>` (default `None`) and `pub fn with_session(self, s: Arc<SessionContext>) -> Self`.
- Produces: `pub fn decide_calls_with(snapshot: &PolicySnapshot, turn: &ToolTurn, calls: &[ToolCall], extra: &Findings, now: &dyn Fn() -> Instant) -> Vec<CallOutcome>`. Each call's findings are `call_findings(c, &collisions)` plus every kind in `extra`. `decide_calls` calls it with `&Findings::new()`.
- Produces: `session_of(ctx)` returns `ctx.session.info` when `ctx.session` is `Some`, else the current behaviour.

- [ ] **Step 1: Write the failing tests**

```rust
// session.rs
#[test] fn the_budget_shrinks_with_each_charge_and_ends_at_zero() {}
#[test] fn a_failed_budget_has_no_time_left() {}
// calls.rs
#[test] fn extra_findings_reach_every_call_of_the_hold() {
    // A tenant policy `forbid ... when { context.findings.contains("forged_history") }`:
    // decide_calls_with(.., extra = {forged_history}) blocks both calls; decide_calls allows them.
}
// build.rs
#[test] fn the_record_takes_the_session_of_the_context() { /* session_id and turn from ctx.session */ }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core session extra_findings the_record_takes`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

Add the module and the fields. `Findings` has no merge method; insert each kind of `extra` with `iter()` and `insert()`. Export `SessionContext`, `StoreBudget` and `decide_calls_with`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy -p paigasus-gateway-core -p paigasus-gateway --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/libs/paigasus-gateway-core
git commit -m "feat(rs): carry the request session and its store budget in the decision context (SMA-753)"
```

---

### Task 6: The `SessionStore` port and the linking rule

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/domain/tool_history.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/domain/mod.rs` (`pub mod tool_history;`)

**Interfaces:**
- Produces the constants: `MAX_EVENTS_PER_TOOL: usize = 10_000`, `MAX_WINDOW_MS: u64 = 86_400_000`, `RETENTION_MS: u64 = MAX_WINDOW_MS + 3_600_000`, `KEY_TTL_SECS: u64 = RETENTION_MS / 1000`.
- Produces the types (all `#[derive(Debug, Clone, PartialEq, Eq)]`):

```rust
pub enum Scope { Session, Principal, Key }                // as_str: "s", "p", "k"
pub enum EventKind { Call, Result }                       // as_str: "request", "response" (Dogwood names)
pub enum EventVerdict { Pending, Allow, Deny }            // as_str: "pending", "allow", "deny"
pub struct Histories { pub org: Uuid, pub principal: String, pub key: Option<String>, pub session: Uuid }
impl Histories {
    /// (Session, session id), (Principal, principal), and (Key, key id) only when `key` is Some (D5:
    /// with no API key the key history is the principal history, written once).
    pub fn scopes(&self) -> Vec<(Scope, String)>;
}
pub struct StoredEvent { pub id: Uuid, pub ts_ms: u64, pub kind: EventKind, pub tool: String, pub verdict: EventVerdict }

pub struct LookupRequest { pub org: Uuid, pub principal: String, pub call_ids: Vec<String>, pub result_call_ids: Vec<String>, pub refs: Vec<Uuid> }
pub struct CallLink { pub session: Uuid, pub tool: String, pub event_id: Uuid, pub result_written: bool }
pub struct SessionState { pub id: Uuid, pub last_ms: u64, pub turn: i64 }
pub struct LookupReply { pub calls: Vec<Option<CallLink>>, pub results: Vec<Option<CallLink>>, pub refs: Vec<Option<Uuid>>, pub sessions: Vec<SessionState> }

pub struct ResultEvent { pub id: Uuid, pub call_id: String, pub tool: String }
pub struct WriteRequest { pub histories: Histories, pub turn: Option<i64>, pub results: Vec<ResultEvent>, pub now_ms: u64 }
pub struct CallEvent { pub id: Uuid, pub tool: String }
pub struct ReserveRequest { pub histories: Histories, pub turn: i64, pub calls: Vec<CallEvent>, pub now_ms: u64, pub window_from_ms: Option<u64> }
pub struct ForwardedCall { pub id: Uuid, pub tool: String, pub call_id: Option<String>, pub item_id: Option<String> }
pub struct AppendRequest { pub histories: Histories, pub turn: i64, pub calls: Vec<ForwardedCall>, pub now_ms: u64 }
pub struct AppendReply { pub written: usize, pub trimmed_retention: u64, pub trimmed_cap: u64, pub truncated: Vec<(Scope, String)> }
/// `item_id`: the item id of a forwarded local shell call. The `c:` link is written under `call_id`
/// and also under `item_id`, with the same session, tool and event id (spec D4).
pub struct SettleItem { pub event_id: Uuid, pub verdict: EventVerdict, pub tool: String, pub call_id: Option<String>, pub item_id: Option<String> }
pub struct SettleRequest { pub org: Uuid, pub principal: String, pub session: Uuid, pub items: Vec<SettleItem>, pub now_ms: u64 }

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HistoryStoreError {
    #[error("history store unavailable ({kind:?}): {detail}")]
    Unavailable { kind: UnavailableKind, detail: String },   // UnavailableKind from domain::limits
}

#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Read only. One answer per id, in order. `sessions` holds the state of every session that a link names.
    async fn lookup(&self, req: &LookupRequest) -> Result<LookupReply, HistoryStoreError>;
    /// After the admission: touch the session when `turn` is Some, and write each result event once per call event.
    async fn write(&self, req: &WriteRequest) -> Result<AppendReply, HistoryStoreError>;
    /// Inside the decide future: one pending call event per call in every history, and its `d:` link.
    async fn reserve(&self, req: &ReserveRequest) -> Result<AppendReply, HistoryStoreError>;
    /// At the report site: set each verdict; write the `c:` link of each forwarded call. Idempotent.
    async fn settle(&self, req: &SettleRequest) -> Result<(), HistoryStoreError>;
    /// At the report site, for a call forwarded with no decision: an allowed event and its `c:` link.
    async fn append(&self, req: &AppendRequest) -> Result<AppendReply, HistoryStoreError>;
}
```

- Produces the pure functions:

```rust
/// SHA-256 of the id, as 64 lower-case hex digits (D4 "Key names").
pub fn link_digest(id: &str) -> String;
/// The history tool of a call: `normalise(&call.display_name())`. The catalog stores normalised names.
pub fn history_tool(call: &ToolCall) -> String;

pub struct LinkedResult { pub call_id: String, pub tool: String }
pub struct SessionLink { pub session: Uuid, pub turn: i64, pub joined: bool, pub mixed_sessions: bool, pub forged_results: usize, pub results: Vec<LinkedResult> }
/// The linking rule of D4. `new_session` is used when no id matches.
pub fn link_session(req: &LookupRequest, reply: &LookupReply, new_session: Uuid) -> SessionLink;
```

`link_session` rules:
- The candidate sessions are the sessions of every `Some` entry of `calls`, `results` and `refs`.
- No candidate: `session = new_session`, `turn = 1`, `joined = false`.
- One or more: the candidate whose `SessionState.last_ms` is largest. A candidate with no `SessionState` has `last_ms = 0`. A tie goes to the larger uuid (a UUIDv7 that is later). `turn` = that state's `turn + 1`, or 1 when it has no state. `mixed_sessions` = more than one distinct candidate.
- `forged_results` = the number of `results` entries that are `None`.
- `results` = the linked results with `result_written == false`, one per call event (from the first of its linked call ids), with the tool of the link. The tool name of the result message is never used (D5).

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn no_match_starts_a_new_session_at_turn_1() {}
#[test] fn all_matches_in_one_session_join_it_with_the_next_turn() {}
#[test] fn matches_in_two_sessions_join_the_most_recent_and_flag_mixed_sessions() {}
#[test] fn a_session_without_state_still_joins_with_last_ms_zero() {}
#[test] fn an_unlinked_result_counts_as_forged_and_writes_nothing() {}
#[test] fn a_ref_alone_joins_its_session() {}
#[test] fn a_result_sent_twice_in_one_request_is_linked_once() {}
#[test] fn an_already_written_result_is_not_written_again() {}
#[test] fn the_result_tool_comes_from_the_link() {}
#[test] fn link_digest_is_64_lower_hex_for_any_id() {
    for id in ["c1", &"x".repeat(10_000), "ü:*?\n", ""] {
        let d = link_digest(id);
        assert_eq!(d.len(), 64);
        assert!(d.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }
}
#[test] fn a_keyless_caller_has_two_histories_and_a_key_caller_three() {}
#[test] fn retention_is_25_hours_and_the_ttl_follows_it() {
    assert_eq!(RETENTION_MS, 90_000_000);
    assert_eq!(KEY_TTL_SECS, 90_000);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib tool_history`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the module**

Types, constants, the trait and the three functions. `sha2` and `paigasus_kernel::tool_policy::normalise` are already dependencies.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib tool_history`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy -p paigasus-gateway --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway/src/domain
git commit -m "feat(rs): add the SessionStore port and the session linking rule (SMA-753)"
```

---

### Task 7: The memory adapter and the contract suite

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/history/mod.rs` (`pub mod memory;` only in this task)
- Create: `rs/crates/services/paigasus-gateway/src/adapters/history/memory.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/mod.rs` (`pub mod history;`)
- Create: `rs/crates/services/paigasus-gateway/tests/support/history_contract.rs`
- Modify: `rs/crates/services/paigasus-gateway/tests/support/mod.rs` (`pub mod history_contract;`)
- Create: `rs/crates/services/paigasus-gateway/tests/history_store_memory.rs`

**Interfaces:**
- Consumes: the port and types of Task 6.
- Produces: `pub struct MemorySessionStore`, `pub fn new() -> Self`, `impl SessionStore`. Inspection, for the harness: `pub fn events(&self, org: Uuid, scope: Scope, scope_id: &str, tool: &str) -> Vec<StoredEvent>` (ordered by `ts_ms`), and `pub fn ttls_ms(&self, org: Uuid, now_ms: u64) -> Vec<Option<u64>>` (one entry per stored key of the org).
- Produces (test support):

```rust
#[async_trait::async_trait]
pub trait Harness: Send + Sync {
    /// Redis: a new store per call (one more replica). Memory: the one shared store.
    async fn store(&self) -> Arc<dyn SessionStore>;
    async fn events(&self, org: Uuid, scope: Scope, scope_id: &str, tool: &str) -> Vec<StoredEvent>;
    async fn ttls_ms(&self, org: Uuid, now_ms: u64) -> Vec<Option<u64>>;
}
pub async fn run_all(h: &impl Harness);
```

The memory store keeps the same model as the Redis layout of Task 8: one sorted list per `(org, scope, scope id, tool)`, a tool index per history, one event body per id with its verdict, the `c:` and `d:` links per `(org, principal)`, and a session state per `(org, session)`. Each entry stores its expiry in ms (`now_ms + KEY_TTL_SECS * 1000`, reset on each write). A read ignores an entry whose expiry is before the `now_ms` of the request. The behaviour is the reserve algorithm of Task 8, step for step.

Contract cases (each takes fresh ids, as `limits_contract::fresh_ids` does; fixed `now_ms`, no sleeps):

| Case | Assertion |
|---|---|
| `a_reserve_appends_one_pending_event_per_history` | 3 histories for a key caller, each holds the event with `verdict == Pending`, `kind == Call`, `ts_ms == now_ms` |
| `a_caller_without_a_key_writes_the_principal_history_once` | the principal history holds 1 event, and no key history exists |
| `the_split_by_tool_keeps_other_tools` | 10,001 `send` events leave the one `read` event in place |
| `retention_trims_only_events_older_than_25_hours` | an event at `now - RETENTION_MS` stays; one at `now - RETENTION_MS - 1` goes; `trimmed_retention == 1` |
| `the_cap_keeps_the_newest_10_000_events_per_tool` | after 10,001 events: 10,000 remain, the oldest went, `trimmed_cap == 1` |
| `the_truncated_flag_needs_a_lost_event_inside_the_window` | flag set for `(Scope, tool)` when the popped event is at or after `window_from_ms`; empty when it is before, and empty with `window_from_ms: None` |
| `settle_sets_the_verdict_and_writes_the_call_link` | `Allow` with `call_id` gives a `lookup` hit with the session, tool and event id |
| `settle_twice_is_idempotent` | the second settle changes nothing |
| `a_deny_settle_writes_no_call_link` | `lookup` of the call id gives `None` |
| `a_lost_settle_leaves_the_event_pending_and_no_call_link` | no settle: event `Pending`, `lookup` `None` |
| `a_settle_for_an_unknown_event_writes_nothing` | no event body and no link appear |
| `the_reserve_writes_decision_links` | `lookup` with `refs = [id]` gives the session |
| `links_are_isolated_per_principal_and_org` | principal B, or org B, gets `None` for links of A |
| `a_result_event_is_written_once_per_call_id` | two `write`s with one call id: one `Result` event, `written` 1 then 0, the link has `result_written == true` |
| `a_write_without_results_and_without_turn_writes_nothing` | `ttls_ms` of the org is empty |
| `touch_keeps_the_latest_activity_and_the_highest_turn` | a later write with a smaller `now_ms` (clock skew of 50 ms) and a smaller turn lowers neither `last_ms` nor `turn` |
| `a_future_event_survives_a_reserve_with_an_older_clock` | an event at `now + 50` stays after a reserve at `now` |
| `every_written_key_expires_after_the_retention` | every TTL is in `(RETENTION_MS - 60_000, RETENTION_MS]` |
| `append_writes_an_allowed_event_and_its_call_link` | event `Allow`, `lookup` of the call id is a hit |
| `a_local_shell_settle_links_the_item_id_too` | a settle `Allow` with `call_id` and `item_id`: `lookup` with `result_call_ids = [item_id]` and with `[call_id]` both give the session, tool and event id; the same holds for `append` |
| `concurrent_reserves_all_land` | 50 concurrent reserves through two stores give 50 events (multi-thread runtime, 4 workers) |

- [ ] **Step 1: Write the contract suite and the memory test binary**

`tests/history_store_memory.rs` has one `#[tokio::test]` per case that calls `contract::<case>(&harness())`, as `tests/limits_store_memory.rs` does. `run_all` calls every case in table order; the Redis binary of Task 8 uses it once per image. Add a unit test in `memory.rs`: `an_expired_link_reads_as_absent`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --test history_store_memory`
Expected: FAIL to compile (`MemorySessionStore` not found).

- [ ] **Step 3: Implement `MemorySessionStore`**

One `std::sync::Mutex` over the state; no lock across an `.await`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway --test history_store_memory && cargo nextest run -p paigasus-gateway --lib history`
Expected: PASS, every case.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy -p paigasus-gateway --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway/src/adapters rs/crates/services/paigasus-gateway/tests
git commit -m "feat(rs): add the memory session store and the history contract suite (SMA-753)"
```

---

### Task 8: The Redis adapter

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/history/redis.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/history/mod.rs` (`pub mod redis;`)
- Create: `rs/crates/services/paigasus-gateway/tests/history_store_redis.rs`
- Modify: `rs/.config/nextest.toml:134` (add `binary(history_store_redis)` to the gateway Docker override)

**Interfaces:**
- Consumes: the port of Task 6, the contract suite of Task 7.
- Produces: `pub const KEY_PREFIX: &str = "paigasus:gateway:history:v1:";` and `pub const HISTORY_BREAKER_ROLE: &str = "tool_history";`.
- Produces: `pub struct RedisSessionStore` (`Clone`), `pub async fn connect(redis_url: &str) -> redis::RedisResult<Self>` (through `paigasus_redis::connect(url, breaker_metrics())`), `pub fn from_handle(handle: RedisHandle) -> Self`, `impl SessionStore`.
- Produces: `pub fn breaker_metrics() -> BreakerMetrics` with the gateway breaker names (`GATEWAY_REDIS_BREAKER_STATE`, `GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL`) and role `HISTORY_BREAKER_ROLE`.
- Produces the key functions, for the tests: `pub fn history_key(org: Uuid, scope: Scope, scope_id: &str, tool: &str) -> String`, `pub fn event_key(org: Uuid, id: Uuid) -> String`, `pub fn call_link_key(org: Uuid, principal: &str, call_id: &str) -> String`, `pub fn decision_link_key(org: Uuid, principal: &str, id: Uuid) -> String`, `pub fn session_key(org: Uuid, session: Uuid) -> String`, and `pub fn decode_event(fields: &[(String, String)]) -> Result<StoredEvent, HistoryStoreError>`.
- Produces: `pub enum EvictionCheck { Safe, Evicts(String), Unknown }` and `pub fn eviction_check(reply: &redis::RedisResult<Vec<String>>) -> EvictionCheck`, plus `pub async fn read_eviction_policy(&self) -> EvictionCheck` (`CONFIG GET maxmemory-policy`). `Safe` only for `noeviction`. Every `allkeys-*` and `volatile-*` value is `Evicts` (every history key has a TTL). A `NOPERM`, an unknown command or another error is `Unknown`.

Layout (spec § 7). A client string in a key name is a digest: `h:<org>:<s|p|k>:<sha256(scope id)>:<sha256(tool)>` (sorted set: score `ts_ms`, member event id), `t:<org>:<s|p|k>:<sha256(scope id)>` (set of tool digests), `e:<org>:<event id>` (hash: `b` = the body, `v` = the verdict), `l:<org>:<sha256(principal)>:c:<sha256(call id)>` (hash: `s`, `t`, `e`, and `r` once the result event exists), `l:<org>:<sha256(principal)>:d:<decision id>` (string: the session), `s:<org>:<session>` (hash: `last_ms`, `turn`). The body is JSON with short keys, `{"v":1,"id":…,"ts":…,"k":"request"|"response","t":"<tool>"}`. `decode_event` ignores unknown keys (rolling upgrade). Map errors with the `unavailable_kind` and `unavailable` rules of `adapters/limits/redis.rs` (never `err.to_string()` in a detail).

Scripts. The caller passes `now_ms`; no script calls `TIME`. Rust builds every key name and passes it in `KEYS`.

`APPEND_SCRIPT`, shared by `write`, `reserve` and `append` (Redis 6.2 commands only):

```lua
-- ARGV: now_ms, retention_ms, cap, ttl_secs, window_from ('' = none), turn ('' = no touch), H, N, then
--       per event: id, body, verdict, tool_digest, guard ('0'|'1'), link ('-'|'d'|'c'), link value
-- KEYS: s-key, H t-keys, then per event: e-key, H h-keys, guard-or-link key
-- for each event j:
--   if guard == '1' and redis.call('HSETNX', guardkey, 'r', id) == 0 then skip j   -- result already written
--   HSET e_j b body v verdict; EXPIRE e_j ttl
--   for each history i:
--     trimmed_retention += ZREMRANGEBYSCORE h_ij -inf ('(' .. (now - retention))   -- strictly older
--     ZADD h_ij now id; SADD t_i tool_digest
--     over = ZCARD h_ij - cap
--     if over > 0 then popped = ZPOPMIN h_ij over; trimmed_cap += over
--       if window_from ~= '' and some popped score >= window_from then flag (i, j)
--     EXPIRE h_ij ttl; EXPIRE t_i ttl
--   link 'd': SET key session EX ttl;  link 'c': HSET key s session t tool e id; EXPIRE key ttl
--   written += 1
-- if turn ~= '' : last = HGET s last_ms; if not last or tonumber(last) < now then HSET s last_ms now
--                 t = HGET s turn;     if not t or tonumber(t) < turn then HSET s turn turn
--                 EXPIRE s ttl
-- return {written, trimmed_retention, trimmed_cap, flagged pairs...}
```

`LOOKUP_SCRIPT` (read only): `HMGET` `s t e r` of each `c:` key, `GET` each `d:` key, then `HMGET last_ms turn` of each distinct session found. The `s:` key names are built in the script from a prefix in `ARGV`; this is allowed because Redis Cluster is not supported (N4). `SETTLE_SCRIPT`: per item, only if `EXISTS e-key`: `HSET e-key v verdict`, and for an `allow` with a call id, `HSET` the `c:` link and `EXPIRE`; with an `item_id` too, the same for the `c:` key of the item id. `APPEND_SCRIPT` writes the second `c:` link in the same way. The guard sits on the call event, not on the link, so a client that sends one result by the call id and one by the item id gets one result event: the count does not go up. Both scripts are idempotent.

- [ ] **Step 1: Write the failing tests**

`tests/history_store_redis.rs`: `the_contract_suite_passes_on_redis_6_2` and `the_contract_suite_passes_on_redis_7_4` (start the image with `paigasus_test_docker::start_redis_image_or_skip`; `RedisHarness::store()` connects a new `RedisSessionStore`; `events()` reads with `redis-cli ZRANGE … WITHSCORES` and `HGETALL`, then `decode_event`; `ttls_ms()` uses `redis-cli --scan --pattern 'paigasus:gateway:history:v1:*<org>*'` and `PTTL`). Add `an_evicting_policy_is_reported` (`redis-cli CONFIG SET maxmemory-policy allkeys-lru`, then `read_eviction_policy()` is `Evicts("allkeys-lru")`) and `noeviction_is_safe`. Unit tests in `redis.rs`: `eviction_check_classifies_each_reply` (`noeviction` → `Safe`; `volatile-ttl`, `allkeys-random` → `Evicts`; a `NOPERM` error → `Unknown`), `key_names_hold_no_client_string` (a call id with `:` and `*` does not appear in any key), `a_body_with_an_unknown_field_still_decodes`, and `errors_map_to_their_kinds` (as in the limits adapter).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib history::redis`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the adapter and register the binary**

Add `or binary(history_store_redis)` to the filter at `rs/.config/nextest.toml:134`, and update the comment above it (SMA-753 adds the history store binary).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib history && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-gateway --test history_store_redis`
Expected: PASS on both images (Docker must be up; `PAIGASUS_REQUIRE_DOCKER=1` turns a skip into a failure).

- [ ] **Step 5: Mutation check, lint, format, commit**

Make each mutation, run the Redis binary, see it red, then delete the marked change (do not use `git checkout --` on the file): delete the `ZREMRANGEBYSCORE` line; change `'(' .. (now - retention)` to an inclusive bound; delete the `HSETNX` guard; delete the `last_ms` comparison. Each must fail a named contract case.

Run: `cd rs && cargo clippy -p paigasus-gateway --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway rs/.config/nextest.toml
git commit -m "feat(rs): add the Redis session store with append, lookup and settle scripts (SMA-753)"
```

---

### Task 9: The `[tool_history]` configuration

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/config.rs` (new table, `GatewayConfig.tool_history`, `validate`)

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolHistoryConfig {
    pub backend: HistoryBackend,                 // "memory" (default) | "redis"
    #[serde(skip_serializing)]
    pub redis_url: Option<SecretString>,         // or GATEWAY_TOOL_HISTORY__REDIS_URL
    pub deadline: String,                        // default "1s"
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryBackend { #[default] Memory, Redis }

impl ToolHistoryConfig {
    pub fn validate(&self, ti: &ToolInterceptionConfig) -> Result<(), String>;
    /// Only after `validate`.
    pub fn deadline(&self) -> Duration;
}
/// "<n>ms" or "<n>s", n a positive integer.
pub fn parse_deadline(s: &str) -> Result<Duration, String>;
```

- Produces: `GatewayConfig.tool_history: Option<ToolHistoryConfig>` with `#[serde(default)]`. No table means no history (spec D12).

Rules (each message names the key, as the other tables do):
- `backend = "redis"` needs a non-blank `redis_url`. `memory` refuses a `redis_url` (the `limits` rule).
- `deadline` parses, is at least 1 ms, and is strictly less than `tool_interception.decide_timeout_secs` in ms (spec D10, as SMA-752 D31).

- [ ] **Step 1: Write the failing tests**

`parse_deadline_accepts_ms_and_s` (`"1s"` → 1000 ms, `"250ms"` → 250 ms); `parse_deadline_refuses_other_forms` (`""`, `"0s"`, `"1.5s"`, `"1m"`, `" 1s"`, `"-1s"`); `no_tool_history_table_is_none`; `the_default_deadline_is_one_second`; `validate_rejects_each_bad_tool_history_value_and_names_the_key` (table-driven, as `validate_rejects_each_bad_limits_value_and_names_the_key`, including `deadline = "5s"` with `decide_timeout_secs = 5`); `the_redis_url_comes_from_the_environment` (figment `Jail` with `GATEWAY_TOOL_HISTORY__REDIS_URL`); `the_redis_url_is_not_serialized`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib config::`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

Call `tool_history.validate(&self.tool_interception)` from `GatewayConfig::validate` when the table is present.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib config::`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy -p paigasus-gateway --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway/src/config.rs
git commit -m "feat(rs): add the [tool_history] configuration and its deadline check (SMA-753)"
```

---

### Task 10: The `ToolHistory` service and its metrics

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/application/tool_history.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/application/mod.rs` (`pub mod tool_history;`)
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (three constants, `ALL`, a registration test)
- Modify: `rs/crates/services/paigasus-gateway/src/main.rs:245` (`describe_gateway_metrics`)
- Modify: `docs/ops/RUNBOOK-observability.md` (three rows in the gateway metric table)

**Interfaces:**
- Consumes: Tasks 4 to 7.
- Produces: `names::{GATEWAY_TOOL_HISTORY_STORE_ERRORS_TOTAL, GATEWAY_TOOL_HISTORY_IDS_CAPPED_TOTAL, GATEWAY_TOOL_HISTORY_EVENTS_TRIMMED_TOTAL}` with the values of Global Constraints.
- Produces:

```rust
pub struct ToolHistory { /* store, clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>, deadline: Duration, tasks: TaskTracker */ }

/// What `link` gives the request side.
pub struct LinkedRequest { pub session: Arc<SessionContext>, pub pending: Option<PendingWrite> }
/// The write that runs after the admission (spec D4). `None` when there is nothing to write.
pub struct PendingWrite { /* session: Arc<SessionContext>, request: WriteRequest */ }

impl ToolHistory {
    pub fn new(store: Arc<dyn SessionStore>, clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>, deadline: Duration) -> Self;
    /// The settle and append tasks, for the shutdown drain.
    pub fn tasks(&self) -> TaskTracker;
    /// Read only. `None` after a lookup error (counted, op `lookup`): the request has no session.
    pub async fn link(&self, principal: &str, org: Uuid, api_key_id: Option<&str>, turn: &ToolTurn) -> Option<LinkedRequest>;
    /// After the admission. A store error is counted (op `write`); the request continues.
    pub async fn commit(&self, pending: PendingWrite);
    /// Inside the decide future. Returns the findings for the hold: `history_truncated` when the
    /// reply flags a set. A store error is counted (op `reserve`) and gives no finding.
    pub async fn reserve(&self, ctx: &DecisionContext, session: &SessionContext, calls: &[ToolCall], ids: &[Uuid]) -> Findings;
    /// At the report site. Spawns one task on the tracker for the settle and, when needed, the append.
    pub fn settle(&self, ctx: &DecisionContext, reported: &[ReportedCall]);
}

pub struct SettlePlan { pub settle: Vec<SettleItem>, pub append: Vec<ForwardedCall> }
/// Pure. A reported call with a decision id: `Allow` when `entry.decision` is `ReportDecision::Allow`,
/// else `Deny`, with the call id only for `Allow`. A reported call with no decision id, `call: Some`
/// and `ReportDecision::Allow` (forwarded with no decision, spec D8): an append with a new event id.
/// Everything else: nothing. `SettleItem.item_id` and `ForwardedCall.item_id` come from
/// `CallFacts.item_id` (Task 4) and are set only for `Allow`.
pub fn settle_plan(reported: &[ReportedCall], mint: &mut dyn FnMut() -> Uuid) -> SettlePlan;

/// Prime every series of the three history metrics at 0.
pub fn prime_tool_history_metrics();
```

Rules:
- `link` builds a `StoreBudget::new(deadline)`, reads `turn`'s `LatestTurn` (an `Unavailable` turn has no ids), counts `ids_capped{list}` by the `dropped_*` values, calls `lookup` through the budget, runs `link_session` with a new UUIDv7 (clock + entropy, as the recorder mints ids), and counts `gateway_tool_policy_findings_total{side="request", kind}` once for `mixed_sessions` and once per forged result. The `SessionContext` holds those two findings. `pending` holds a `WriteRequest` with `turn: Some(link.turn)` when `joined`, `None` otherwise, and one `ResultEvent` per `LinkedResult` (new ids); it is `None` when there is no touch and no result.
- Every store call on the latency path (`lookup`, `write`, `reserve`) goes through one private helper. It returns `None` without a call when `budget.remaining()` is `None`. Otherwise it runs `tokio::time::timeout(remaining, call)`, charges the elapsed time, and on an error or a timeout calls `budget.fail()` and counts `store_errors{op, kind}` (`kind = "deadline"` for a timeout).
- A failed budget skips the later latency-path calls only. `settle` and `append` still run (rulings T10-a, T10-b). Each gets `timeout(deadline)` of its own and does not charge the budget (it is off the latency path). `append` also writes an allowed call that has no reserved event (final-review fix). The tracker being closed still skips both.
- Count `events_trimmed{reason}` from every `AppendReply`.
- PR 1 passes `window_from_ms: None` to `reserve` (no window read, spec D15).
- `reserve` uses `Histories { org: session.org, principal: ctx.principal, key: ctx.api_key_id, session: session.session_id }`, `turn: session.info.turn`, and one `CallEvent { id: ids[i], tool: history_tool(&calls[i]) }` per call.

- [ ] **Step 1: Write the failing tests**

Unit tests with `MemorySessionStore` and fakes (a store that fails every call, and a store that sleeps 2 s under `#[tokio::test(start_paused = true)]`):

```rust
#[tokio::test] async fn a_first_request_gets_a_new_session_and_writes_nothing() {}
#[tokio::test] async fn a_follow_up_with_a_linked_result_joins_and_writes_one_result_event() {}
#[tokio::test] async fn a_forged_result_counts_a_request_finding_and_writes_no_event() {}
#[tokio::test] async fn a_lookup_error_gives_no_session_and_counts_lookup_io() {}
#[tokio::test(start_paused = true)] async fn a_slow_lookup_spends_the_budget_and_skips_the_rest() {
    // lookup sleeps past the 1 s deadline: store_errors{op="lookup",kind="deadline"} == 1, link() is None;
    // with a SessionContext whose budget failed, reserve() makes no store call (the fake counts calls).
}
#[tokio::test] async fn a_reserve_writes_one_pending_event_per_call_in_every_history() {}
#[tokio::test] async fn a_truncated_reply_gives_the_history_truncated_finding() { /* fake store returns a flagged pair */ }
#[tokio::test] async fn a_reserve_error_is_counted_and_gives_no_finding() {}
#[test] fn settle_plan_marks_allow_and_deny_and_links_only_forwarded_calls() {}
#[test] fn settle_plan_appends_a_forwarded_call_without_a_decision() {}
#[test] fn settle_plan_carries_the_item_id_of_a_forwarded_local_shell_call() { /* Some for allow, None for deny */ }
#[test] fn settle_plan_ignores_a_block_without_a_decision_and_an_unenforceable_entry() {}
#[tokio::test] async fn settle_runs_on_the_tracker_and_sets_the_verdict() { /* tasks().close(); tasks().wait().await; then read the memory store */ }
#[tokio::test] async fn ids_past_the_cap_are_counted_per_list() {}
#[test] fn priming_sets_every_history_series_to_zero() { /* 5 ops x 4 kinds, 3 lists, 2 reasons */ }
```

In `names.rs`: `the_gateway_tool_history_families_are_registered`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib tool_history && cargo nextest run -p paigasus-observability`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the service, the names, the descriptions and the runbook rows**

Describe each family in `describe_gateway_metrics` in one sentence. In `RUNBOOK-observability.md`, add one row per family with its labels and what an operator does: a rise of `store_errors` means the history misses events (PR 1 evaluates no temporal history conditions yet; the history findings and the session reach response-side Cedar); `ids_capped` means a client sends more than 128 ids in one turn; `events_trimmed{reason="cap"}` means a flood of one tool.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib tool_history && cargo nextest run -p paigasus-observability`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy -p paigasus-gateway -p paigasus-observability --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway rs/crates/libs/paigasus-observability docs/ops/RUNBOOK-observability.md
git commit -m "feat(rs): add the tool history service with its budget and metrics (SMA-753)"
```

---

### Task 11: Linking on the request side and the write after the admission

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/request.rs` (`RequestToolPolicy`, `RequestOutcome`, `apply_inner`, new `commit_history`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/pipeline.rs:110-166` (session into the context, commit after the admission)

**Interfaces:**
- Consumes: `ToolHistory::link`, `ToolHistory::commit`, `LinkedRequest`, `PendingWrite` (Task 10); `DecisionContext::with_session` (Task 5).
- Produces: `pub fn with_history(self, history: Arc<ToolHistory>) -> Self` on `RequestToolPolicy`.
- Produces: `RequestOutcome { pub body, pub entries, pub snapshot, pub session: Option<Arc<SessionContext>>, pub history: Option<PendingWrite> }`.
- Produces: `pub async fn commit_history(&self, pending: PendingWrite)` on `RequestToolPolicy` (delegates to `ToolHistory::commit`).

Rules (spec D4, D5):
- Link only when a history is set, the org is `Some`, and the final tenant mode is `log_only` or `enforce`. Link after the policy set and the mode are known, before `plan_request`.
- Put `SessionInfo::new(session_id.to_string(), turn)` into `facts.session` of a `PolicySet::Available` before `PolicySnapshot::new`.
- Use `ctx.clone().with_session(Arc::clone(&s))` for the request records of `apply_inner` (`record_entries`, `record_refusal`), so they carry `session_id` and `turn`.
- A refusal returns `Err` and drops the pending write.
- No request-side Cedar decision gets `mixed_sessions` or `forged_history` in PR 1. The reason is the current design. Every request-side decision (definition removal, result replacement, tool-choice reset) goes through `Judge::denied` in `gateway-core/src/policy/plan.rs:127-141`. `Judge::denied` calls `decide_tool`, and `decide_tool` evaluates only the optimistic set (`paigasus-kernel/src/tool_policy/optimistic.rs:1-10, 45-53`, SMA-752 § 6.1.6). That set drops every `forbid` that reads `context.findings`, so no finding can change a request-side verdict. Adding the two findings to `Judge` would change nothing that a test can see. To make a request-side policy read findings, the request side needs a full per-result `decide` with the result's call. That is a design change to SMA-752 D5, not a PR 1 task. The two findings are counted on the request side (`side="request"`, Task 10) and reach every response-side decision (Task 12).
- In `pipeline.rs`: `ctx = ctx.with_policy(outcome.snapshot)` and, when `outcome.session` is `Some`, `.with_session(..)`. Keep `outcome.history` in a local. After the limits admission returns `Ok` (or when there is no `[limits]`), call `tp.commit_history(pending).await`. A limit refusal returns before this line, so it writes nothing.

- [ ] **Step 1: Write the failing tests**

In `request.rs` tests (memory store, `testing::FakeSource`):

```rust
#[tokio::test] async fn apply_links_the_request_and_fills_facts_session() {}
#[tokio::test] async fn mode_off_links_nothing() {}
#[tokio::test] async fn an_org_less_scope_links_nothing() {}
#[tokio::test] async fn a_refused_request_returns_no_pending_write() { /* the unreplaceable-result refusal */ }
#[tokio::test] async fn request_records_carry_the_session() { /* MemoryDecisionLog: session_id and turn of a request entry */ }
#[tokio::test] async fn without_a_history_the_session_stays_the_placeholder() {}
```

In `tests/tool_policy.rs` (HTTP, memory store), add `a_rate_limited_request_writes_no_history`: a follow-up request with a linked result is refused by a principal rate limit of 0 admissions (a `limits` with `ScriptedStore`-style refusal from `tests/support/limits.rs`), and the memory store then has no result event.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib request:: && cargo nextest run -p paigasus-gateway --test tool_policy history`
Expected: FAIL to compile (`with_history` not found).

- [ ] **Step 3: Implement**

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib && cargo nextest run -p paigasus-gateway --test tool_policy`
Expected: PASS.

- [ ] **Step 5: Mutation check, lint, format, commit**

Move the `commit_history` call before the limits admission: `a_rate_limited_request_writes_no_history` must fail. Undo the move. Task 14 owns the proof that deleting the call reds the end-to-end tests.

Run: `cd rs && cargo clippy -p paigasus-gateway --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): link each request to a session and write its history after the admission (SMA-753)"
```

---

### Task 12: The reserve in the decider and the settle at the report sites

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/decider.rs` (`CedarDecider::with_history`, `decide`, `decide_now`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/interception.rs` (`ToolInterception.history`, `intercept_body`, `Relay::apply`)
- Modify (add `history: None` to each `ToolInterception { .. }` literal): `src/runtime.rs`, `tests/perf.rs`, `tests/responses_proxy.rs`, `tests/tool_interception.rs`, `tests/tool_policy.rs`, `tests/tool_policy_nats.rs`, and the tests in `interception.rs`

**Interfaces:**
- Consumes: `ToolHistory::reserve`, `ToolHistory::settle` (Task 10); `decide_calls_with` (Task 5); decision ids (Task 3).
- Produces: `pub fn with_history(self, history: Arc<ToolHistory>) -> Self` on `CedarDecider`.
- Produces: `ToolInterception { pub limits, pub decider, pub recorder, pub history: Option<Arc<ToolHistory>> }`.

Rules (spec D1, D6 step 1 and 2, D8, C3):
- `decide` mints the ids first (synchronously). The future then runs `reserve` when the decider has a history and `ctx.session` is `Some`, and then `decide_now` with `extra` = the session findings plus the reserve findings. The decider spawns nothing. A dropped future drops the reserve.
- `decide_now` passes `extra` to `decide_calls_with` and keeps the ids from the mint.
- The stream path: `Relay::apply` collects the `Reported` actions of one `apply` call, records each one, then calls `history.settle(&ctx, &batch)` once when the batch is not empty and `ctx.session` is `Some`.
- The body path: after the records, `history.settle(&ctx, &reported)` with the same condition.

- [ ] **Step 1: Write the failing tests**

In `decider.rs` tests: `the_reserve_runs_before_the_decision_and_uses_the_decision_ids` (the memory store holds one pending event per call, with the event id equal to the `DecidedCall.decision_id`); `a_session_finding_reaches_the_cedar_context` (a tenant `forbid … when { context.findings.contains("mixed_sessions") }` blocks when the session has that finding); `a_reserve_error_still_decides` (failing store: the decisions equal those with no history).

In `interception.rs` tests: `a_forwarded_call_is_settled_allow_with_its_call_link`; `a_blocked_call_is_settled_deny_without_a_call_link`; `the_body_path_settles_like_the_stream_path`; `a_timed_out_hold_under_allow_appends_and_keeps_the_pending_event` (a test decider that awaits the `CedarDecider` future, so the reserve runs, and then sleeps past `decide_timeout` (the existing `SlowCedar` helper sleeps first; this one sleeps after); `on_failure = allow`: the memory store holds the `pending` event and one `allow` event for the call); `without_a_session_nothing_is_settled`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib decider:: interception::`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway`
Expected: PASS.

- [ ] **Step 5: Mutation check, lint, format, commit**

Delete the reserve call: `the_reserve_runs_before_the_decision_and_uses_the_decision_ids` fails. Delete the settle call in `Relay::apply`: `a_forwarded_call_is_settled_allow_with_its_call_link` fails. Delete the marked change after each run.

Run: `cd rs && cargo clippy -p paigasus-gateway --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): reserve call events in the decider and settle them at the report site (SMA-753)"
```

---

### Task 13: Boot wiring, the Redis requirement check and the runbook

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/history/mod.rs` (`build_tool_history`)
- Modify: `rs/crates/services/paigasus-gateway/src/runtime.rs` (`build_tool_policy`, `tool_interception`, `supervise`, `ToolPolicyRuntime`)
- Modify: `rs/crates/services/paigasus-gateway/src/main.rs:106-140, 239`
- Modify: `docs/ops/RUNBOOK-tool-policies.md` (a new section "Tool history (SMA-753)")

**Interfaces:**
- Consumes: Tasks 8 to 12.
- Produces:

```rust
#[derive(Default)]
pub struct HistoryWiring { pub history: Option<Arc<ToolHistory>>, pub tasks: Option<TaskTracker> }
/// No table: nothing. `memory`: a MemorySessionStore. `redis`: RedisSessionStore::connect (eager:
/// a Redis that is down fails the boot, spec D10), then the eviction check (spec D11):
/// `Evicts(p)` fails the boot and names `p`; `Unknown` logs one warning; `Safe` passes.
/// Primes the history metrics. Call after `paigasus_observability::init`.
pub async fn build_tool_history(config: Option<&ToolHistoryConfig>, clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>) -> anyhow::Result<HistoryWiring>;
```

- Produces: `build_tool_policy(config: &GatewayConfig, recorder: Arc<DecisionRecorder>, history: Option<Arc<ToolHistory>>) -> anyhow::Result<ToolPolicyRuntime>`. It calls `with_history` on the `RequestToolPolicy` and the `CedarDecider`. `ToolPolicyRuntime` gets `pub history: Option<Arc<ToolHistory>>`, and `tool_interception` passes it into `ToolInterception.history`. With the tool policy off, the history is not used, and one `info` line says so.
- Produces: `supervise(servers, shutdown, tx, charges, history: Option<TaskTracker>, decision_log)`. After the charges drain, it closes the history tracker and waits at most `CHARGE_DRAIN_BUDGET`. Settles still running then are lost and logged at `warn` (a lost settle leaves `pending`, which still counts).
- The boot error for a failed connect names the error kind only, as `connect_failure_message` in `adapters/limits/mod.rs` does.

Runbook section (Simplified Technical English): the `[tool_history]` keys and the deadline rule; the Redis requirement (a dedicated instance or logical database with `maxmemory-policy noeviction`; the limits Redis of SMA-677 can be shared, the IAM Redis cannot); the sizing formula: memory ≈ events per day × (event body ≈ 60 B + 3 sorted-set entries ≈ 3 × 60 B) + links, with at most 10,000 events per `(history, tool)` and a retention of 25 h; the boot checks; that PR 1 records history and links but no policy reads them yet; that `forged_history` also fires for a blocked call and for a link older than 25 h (R2).

- [ ] **Step 1: Write the failing tests**

In `adapters/history/mod.rs`: `no_table_builds_nothing`; `the_memory_backend_builds_a_history_without_a_tracker_wait` (a history and a tracker); `a_redis_that_is_down_fails_the_boot_without_the_url` (an unused local port; the error text does not contain the URL). In `runtime.rs` tests: `the_history_reaches_the_request_side_the_decider_and_the_report_site` (`Arc::ptr_eq` on the three holders); `supervise_drains_the_history_tasks`. Update the existing callers of `build_tool_policy` and `supervise` in `runtime.rs` tests.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib runtime:: adapters::history`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the wiring and the runbook section**

In `main.rs`: `let history = build_tool_history(config.tool_history.as_ref(), Arc::new(SystemClock), Arc::new(OsEntropy)).await?;` after `build_limits`, then pass `history.history` to `build_tool_policy` and `history.tasks` to `supervise`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-gateway --test history_store_redis`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway docs/ops/RUNBOOK-tool-policies.md
git commit -m "feat(rs): wire the tool history at boot with the Redis eviction check (SMA-753)"
```

---

### Task 14: End-to-end linking through HTTP

**Files:**
- Create: `rs/crates/services/paigasus-gateway/tests/tool_history.rs` (memory store, no Docker)

**Interfaces:**
- Consumes: the whole stack. Build the app as `tests/tool_policy.rs` does (`router(AppState { .. })` with a `RequestToolPolicy::with_history`, a `ToolInterception` with the same `ToolHistory`, a `CedarDecider::with_history`, and a `MemoryDecisionLog` recorder), and the mock upstream of `tests/support/mod.rs`.

Each test sends request 1, reads the answer, builds request 2 from the answer as a client would, and asserts on the memory store and on the decision records. Wait for the settle with `history.tasks().close(); history.tasks().wait().await` before request 2.

| Test | Assertion |
|---|---|
| `chat_a_forwarded_call_and_its_result_stay_in_one_session` | request 2 records `session_id` of request 1 and `turn == 2`; one `Result` event with the tool of the call |
| `anthropic_a_forwarded_call_and_its_result_stay_in_one_session` | same, Anthropic Messages |
| `responses_a_forwarded_call_and_its_result_stay_in_one_session` | same, Responses |
| `a_blocked_call_links_the_next_request_by_its_ref` | request 1 blocks `rm` (floor deny), request 2 holds the notice text: same session |
| `a_stream_answer_links_like_a_body_answer` | `stream: true` for request 1 |
| `a_result_for_an_unknown_call_id_is_forged_and_counted` | `gateway_tool_policy_findings_total{side="request",kind="forged_history"} == 1`, no `Result` event |
| `a_ref_of_another_principal_starts_a_new_session` | another API key principal sends the notice: a new session |
| `a_mixed_conversation_joins_the_newest_session_and_flags_it` | ids of two sessions: the newer one, `mixed_sessions` counted |
| `the_notice_ref_is_the_event_id_and_the_record_id` | one id in the notice, the store event and the record |
| `responses_a_local_shell_result_by_item_id_stays_in_the_session` | request 1 forwards a `local_shell_call` (`id` ≠ `call_id`); request 2 sends a `local_shell_call_output` whose `id` is the call's item `id` and that has no `call_id`: same session, one `Result` event, `forged_history` not counted |

- [ ] **Step 1: Write the tests**

- [ ] **Step 2: Run the tests**

Run: `cd rs && cargo nextest run -p paigasus-gateway --test tool_history`
Expected: PASS.

- [ ] **Step 3: Prove that the tests bite**

Delete the `commit_history` call in `pipeline.rs`: the three `*_stay_in_one_session` tests must fail (no `Result` event). Delete the `with_session` call in `pipeline.rs`: they must fail (no reserve, no `d:` link). Delete the `ref` argument at the chat body notice site: `a_blocked_call_links_the_next_request_by_its_ref` must fail. Delete each marked change after its run.

- [ ] **Step 4: Lint, format, commit**

Run: `cd rs && cargo clippy -p paigasus-gateway --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway/tests/tool_history.rs
git commit -m "test(rs): prove session linking end to end for the three formats (SMA-753)"
```

---

### Task 15: The gate graph

**Files:** none (fix only what a gate reds, in the task that owns the file, as a new commit).

- [ ] **Step 1: Run the workspace checks**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; cd rs && cargo build --workspace && cargo fmt --check && cargo clippy --workspace -- -D warnings`
Expected: all exit 0.

- [ ] **Step 2: Run the affected graph and the gates that this PR touches**

Run (from the repository root, Docker up):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :observability-drift :redis-connect-single-site \
  :http-extractor-envelope :error-code-single-site :version-lockstep :affected-smoke \
  --base origin/main --include-relations
```

Expected green: `paigasus-kernel-rs:*` and its affected bindings (`paigasus-kernel-py:test`, `paigasus-kernel-ts:test`), `paigasus-gateway-core-rs:*`, `paigasus-gateway-rs:*` (with `history_store_redis` in the `docker-containers` group), `paigasus-observability-rs:test`, `repo:observability-drift`, `repo:redis-connect-single-site`, `repo:http-extractor-envelope`, `repo:version-lockstep`, `repo:affected-smoke`.

- [ ] **Step 3: Run the full CI target list before the push**

Run the exact command between the `ci-targets` markers in the root `CLAUDE.md`. Expected: green. A failure with `mapfile`, `declare -A`, `local -n` or a "512 bytes" pipe message is the local bash split of the root `CLAUDE.md`, not a finding: re-run that gate with the other bash as the root `CLAUDE.md` says. For an unattributed failure, follow the moon diagnosis procedure of the root `CLAUDE.md` (Step 0 first).

- [ ] **Step 4: Record the result**

List each red gate, its cause and its fix commit in the PR description. Do not commit a report file.

---

## Self-review notes

- **Spec coverage (PR 1 scope):** D1 → Tasks 2, 3, 4. D4 → Tasks 4, 6, 10, 11. D5 (layout, scopes, results once, tool from the link) → Tasks 6, 7, 8, 10. D7 → not in PR 1 (no fields, Global Constraints). D8 (append-only reserve, settle at the report site, pass-through append, caller time) → Tasks 7, 8, 10, 12. D10 (deadline, skip after the first error, boot fail on a down Redis; the store-error table rows for conditions are PR 2) → Tasks 9, 10, 13. D11 (constants, TTL, cap, truncated flag, eviction check) → Tasks 6, 7, 8, 13. D12 → Tasks 9, 13. D13 → Tasks 1, 10. D14 decision log `session_id`/`turn` → Tasks 5, 11. § 9 contract suite and linking tests → Tasks 7, 8, 14; the evaluator, fail-path rows with conditions, parity, cap and p99 tests are PR 2.
- **Local shell results (D4):** `ToolCall::item_id` and `CallFacts.item_id` (Task 4) → `SettleItem`/`ForwardedCall.item_id` (Task 6) → a second `c:` link (Task 8, contract case in Task 7) → `settle_plan` (Task 10) → end-to-end test (Task 14).
- **Request-side findings:** not planned; Task 11 states why (the optimistic set drops forbids that read `context.findings`).
- **Not in PR 1, by the spec:** the kernel evaluator and slots, the window read, condition parsing and load validation, the R5 removal, the IAM setting, the corpus, parity, the cap test and the p99 test.
- **Type names used across tasks:** `SessionContext`, `StoreBudget`, `LatestTurn`, `SessionStore`, `Histories`, `ReserveRequest`, `WriteRequest`, `AppendRequest`, `SettleRequest`, `AppendReply`, `ToolHistory`, `LinkedRequest`, `PendingWrite`, `settle_plan`, `with_history`, `commit_history`, `build_tool_history`. Each is defined once, in the task that produces it.
