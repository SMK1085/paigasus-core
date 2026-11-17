# SMA-776 Blocked Structured Results Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A blocked Responses `shell_call_output` becomes one schema-valid notice chunk. An enforced block of a `computer_call_output` refuses the request with a 400 `tool-result-unreplaceable` (318) and the decision header. No upstream call occurs.
**Architecture:** One `ResultShape` table in `paigasus-gateway-core` tells `turn.rs` which items are results and tells `rewrite.rs` how to replace each one. The plan records refusal causes in a `refusals` map with an `enforced` flag. The service refuses on the first enforced cause, reports unenforced causes with `refused="false"`, and the HTTP adapter maps the new refusal to code 318 and adds the `paigasus-tool-decisions` header.
**Tech Stack:** Rust 1.95 (edition 2024), `serde_json` `RawValue` slices, `metrics`, axum; protobuf + buf (`contracts:generate`); TypeScript (vitest) for the SDK.
**Spec:** docs/superpowers/specs/2026-11-16-sma-776-blocked-structured-results-design.md

## Global Constraints

- PATH prefix for every shell: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python). This plan adds no new source file.
- Rust crates stay on edition 2024, rust-version 1.95.
- Commits: conventional, scope from `rs`, `contracts`, `ts`, `docs` (commitlint `scope-enum`), header at most 100 characters, body lines at most 100 characters, no `#NNN` line in the body.
- Every commit ends with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never use `--no-verify`. If `commitlint not found`, provision the worktree (root `CLAUDE.md`).
- Before each commit, in `rs/`: `cargo fmt --check` and `cargo clippy --locked --all-targets -p <crate> -- -D warnings`. Dead code is a hard error: do not add an item that a later task uses.
- New reason: `ERROR_REASON_TOOL_RESULT_UNREPLACEABLE = 318`, wire code `tool-result-unreplaceable`, HTTP 400, OpenAI type `invalid_request_error`, `param: "input"`, `Retryable::No`.
- Exact error message: `A blocked computer_call_output cannot be replaced. Remove the computer call and its output from the input.`
- Exact shell chunk (compact, member order fixed, `<notice>` is the JSON string of `result_notice`): `[{"stdout":"","stderr":<notice>,"outcome":{"type":"exit","exit_code":1}}]`.
- Refusal metric: `gateway_tool_policy_request_refusals_total{mode, reason, refused}`. `reason` = `unreadable_turn` | `unknown_tool_choice` | `unreplaceable_result`. `refused` = `"true"` | `"false"`.
- The notice text and the error text never hold result content (SMA-749 D21). Logs and metrics hold no body content and no tool name (ADR-0027 (i)).
- `contracts:generate` runs remote buf plugins (BSR). Run it only as `moon run contracts:generate --force`, never a bare `buf generate`. If it fails, restore `error_details_pb.ts` and do not commit a deletion (`contracts/moon.yml` RECOVERY note).
- Workspace user text (docs, comments, commit bodies) uses ASD-STE100 Simplified Technical English.

## Review Focus

1. **The OR of `enforced` per cause.** A log-only block and a floor block both give `UnreplaceableResult`. The order must not matter: the value is `true`. Spec § 7 tests only one finding per cause. Test: Task 3, `two_findings_of_one_cause_are_enforced_when_any_is`.
2. **A shell notice must keep every other member byte-exact and give one chunk for many commands.** A wrong `set` can drop `max_output_length` or `caller`. The fixture has neither member. Test: Task 4, `a_shell_notice_is_one_chunk_and_keeps_every_other_member`.
3. **An unenforced computer block must not fail the rewrite of other enforced items.** Log-only with a floor removal and a tenant-blocked computer result must rewrite the definitions and leave the screenshot as it is. Test: Task 4, `an_unenforced_computer_block_leaves_the_output_and_the_rest_applies`.
4. **One refused request gives exactly one `refused="true"` sample**, also when two causes are enforced. Test: Task 5, `a_request_with_two_enforced_causes_counts_once`.
5. **A refused stream request is a JSON 400 with the header, not SSE. An unreadable turn has no header.** Test: Task 6, the content-type assertion in `a_blocked_computer_result_is_refused_before_the_upstream`, and the no-header assertion in `an_unavailable_turn_is_refused_in_enforce_and_counted_in_log_only`.

Order of tasks: the error code first (no Rust dependent), then the `ResultShape` refactor (no behaviour change), then the plan (the first behaviour change, it moves the computer pair out of the HTTP test), then the rewrite, then the metric label, then the HTTP mapping, then the SMA-752 spec. The spec suggests one service task. This plan splits it into Task 5 (metric, service only) and Task 6 (HTTP), because a reviewer can reject each one separately.

---

## Task 1: The `tool-result-unreplaceable` error reason (318)

**Files**
- Modify: `contracts/proto/paigasus/common/v1/error.proto:267-273` (banner line, new value after 317).
- Regenerate (commit the output): `rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs`, `py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/__init__.py`, `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts`.
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs:227-230` (`EXPECTED_REASONS`), `:253` (count 74 to 75).
- Modify: `ts/packages/paigasus-proto/src/error.test.ts:57-60` (count and its comment).
- Modify: `ts/packages/paigasus-sdk/src/errors/presentation.ts:106-110` (new entry after `TOOL_POLICY_UNAVAILABLE`).
- Test: `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`, `ts/packages/paigasus-sdk/tests/map-error.test.ts` (after the SMA-769 case, about line 208).
- No `ci/error-registry/check.py` row: `adapters/http/error.rs` already has an `emits` row (spec § 3).

**Interfaces**
- Produces: `ErrorReason::ToolResultUnreplaceable` (Rust, wire `tool-result-unreplaceable`), `ErrorReason.TOOL_RESULT_UNREPLACEABLE` (TS).

- [ ] **Step 1: Write the failing tests.** In `rs/crates/libs/paigasus-proto/src/error.rs`, add after `"tool-policy-unavailable",`:

```rust
        // Gateway tool decisions: the one HTTP error of the banner (SMA-776)
        "tool-result-unreplaceable",
```

and change the count line to `assert_eq!(actual.len(), 75, "the registry should hold 75 reasons");`. In `presentation.test.ts` set `expect(reasons).toHaveLength(75);`. In `ts/packages/paigasus-proto/src/error.test.ts` set `expect(values).toHaveLength(75);` and change the comment to "The Rust mirror asserts 75 at rs/crates/libs/paigasus-proto/src/error.rs:254". In `map-error.test.ts` add:

```ts
  // SMA-776. A blocked computer_call_output cannot be replaced: a 400, never retryable.
  it('maps tool-result-unreplaceable to from-transport, not retryable', () => {
    const result = mapError({
      kind: 'http',
      status: 400,
      headers: new Headers({ 'paigasus-retryable': 'false' }),
      body: {
        error: {
          message:
            'A blocked computer_call_output cannot be replaced. Remove the computer call and its output from the input.',
          type: 'invalid_request_error',
          param: 'input',
          code: 'tool-result-unreplaceable',
        },
      },
    });
    expect(result.reason).toBe(ErrorReason.TOOL_RESULT_UNREPLACEABLE);
    // The transport table presents a 400 as invalid input.
    expect(result.presentation).toBe('invalid-input');
    expect(result.retryable).toBe(false);
  });
```

- [ ] **Step 2: Run them and see them fail.** `cd rs && cargo nextest run --locked -p paigasus-proto the_registry_contains_exactly_the_expected_reasons`. Expected: FAIL, "declared in the test but not in the registry: [\"tool-result-unreplaceable\"]".

- [ ] **Step 3: Implement.** In `error.proto`, add this line directly under `// ---- Gateway: tool decisions (SMA-752) ----`:

```proto
  // 315-317 never reach the caller as an HTTP error (D11). 318 is the one exception (SMA-776).
```

and after `ERROR_REASON_TOOL_POLICY_UNAVAILABLE = 317;`:

```proto
  // "tool-result-unreplaceable" — the tool policy blocked a Responses computer_call_output, and that item has no field that can carry the notice, so the gateway refused the request (400, OpenAI type "invalid_request_error", param "input"). It is the only code of this banner that is an HTTP error (SMA-776).
  ERROR_REASON_TOOL_RESULT_UNREPLACEABLE = 318;
```

The comments of 315-317 do not change. Then `cd contracts && buf format -w && cd .. && moon run contracts:generate --force`. In `presentation.ts` add after the SMA-752 block:

```ts
  // SMA-776. The only tool-decision reason that is an HTTP error: the gateway refuses a request
  // with a blocked computer_call_output. A 400, so the transport table's answer applies.
  [ErrorReason.TOOL_RESULT_UNREPLACEABLE]: 'from-transport',
```

- [ ] **Step 4: Run and see them pass.**
  - `moon run contracts:fmt contracts:lint contracts:breaking` passes.
  - `git status --short` shows the three generated files modified, and no deleted file.
  - `cd rs && cargo nextest run --locked -p paigasus-proto` passes. Then run `cargo nextest run --locked -p paigasus-gateway --lib every_gateway_code_is_declared_in_the_canonical_registry`, which passes unchanged.
  - `moon run paigasus-proto-ts:test paigasus-proto-ts:typecheck paigasus-sdk-ts:test paigasus-sdk-ts:typecheck ts:fmt ts:lint repo:error-code-single-site` passes.

- [ ] **Step 5: Commit.**

```bash
git add contracts/proto/paigasus/common/v1/error.proto \
  rs/crates/libs/paigasus-proto/src/generated py/packages/paigasus-proto/src/paigasus_proto/generated \
  ts/packages/paigasus-proto/src/generated rs/crates/libs/paigasus-proto/src/error.rs \
  ts/packages/paigasus-proto/src/error.test.ts ts/packages/paigasus-sdk/src/errors/presentation.ts \
  ts/packages/paigasus-sdk/tests/presentation.test.ts ts/packages/paigasus-sdk/tests/map-error.test.ts
git commit -m "feat(contracts): add the tool-result-unreplaceable error reason (SMA-776)" \
  -m "Add reason 318 under the tool-decision banner. It is the one code of that banner that is an HTTP error. Regenerate the bindings and present the code in the SDK." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

## Task 2: One `ResultShape` table for the Responses result types (no behaviour change)

**Files**
- Modify: `rs/crates/libs/paigasus-gateway-core/src/turn.rs:103-112` (`ToolResultRef`), `:207-216` (chat results), `:294-307` (Responses results), `:495-499` (Anthropic results); tests at `:537-570`, `:642-684`.
- Modify: `rs/crates/libs/paigasus-gateway-core/src/policy/rewrite.rs:18-26` (delete `RESPONSES_OUTPUTS`), `:322-337` (input loop).
- Modify: `rs/crates/libs/paigasus-gateway-core/src/lib.rs:32` (export `ResultShape`).
- Modify: `rs/crates/libs/paigasus-gateway-core/src/policy/plan.rs` tests: every `ToolResultRef { .. }` literal (lines 374, 554, 580, 604, 647, 657) gets `shape: ResultShape::Text`.

**Interfaces**
- Produces (planner's call: the table lives in `turn.rs`, beside the parse that reads it):

```rust
/// How a blocked tool result is replaced (SMA-776 § 6.1).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultShape { Text, ShellChunks, Screenshot, Approval }

/// The shape of a Responses input item of type `ty`, or `None` when the item is not a tool result.
pub(crate) fn responses_result_shape(ty: &str) -> Option<ResultShape>;

pub struct ToolResultRef { pub call_id: Option<String>, pub name: Option<String>, pub item_id: Option<String>, pub shape: ResultShape }
```

- Pinned table: `function_call_output`, `custom_tool_call_output`, `local_shell_call_output`, `apply_patch_call_output` give `Text`; `shell_call_output` gives `ShellChunks`; `computer_call_output` gives `Screenshot`; `mcp_approval_response` gives `Approval`; every other type gives `None`. Chat `role:"tool"`, `role:"function"` and Anthropic `tool_result` give `Text`.

- [ ] **Step 1: Write the failing test** in the `turn.rs` test module:

```rust
    #[test]
    fn each_responses_result_type_has_its_shape() {
        use ResultShape::{Approval, Screenshot, ShellChunks, Text};
        let table = [
            ("function_call_output", Some(Text)),
            ("custom_tool_call_output", Some(Text)),
            ("local_shell_call_output", Some(Text)),
            ("apply_patch_call_output", Some(Text)),
            ("shell_call_output", Some(ShellChunks)),
            ("computer_call_output", Some(Screenshot)),
            ("mcp_approval_response", Some(Approval)),
            ("function_call", None),
            ("computer_call", None),
            ("mcp_call", None),
            ("message", None),
        ];
        for (ty, shape) in table {
            assert_eq!(responses_result_shape(ty), shape, "{ty}");
        }
    }
```

In `a_responses_request_yields_definitions_calls_and_results`, after the `ids` assert, add:

```rust
        let shapes: Vec<_> = f.tool_results.iter().map(|r| r.shape).collect();
        use ResultShape::{Approval, Screenshot, ShellChunks, Text};
        assert_eq!(shapes, [Text, Text, ShellChunks, Screenshot, Text, Text, Approval]);
```

Add `shape: ResultShape::Text` to the two `ToolResultRef` literals of `an_openai_request_yields_definitions_calls_and_results`. In `an_anthropic_request_yields_definitions_calls_and_results` add `assert!(f.tool_results.iter().all(|r| r.shape == ResultShape::Text));`.

- [ ] **Step 2: Run and see it fail.** `cd rs && cargo nextest run --locked -p paigasus-gateway-core turn::`. Expected: compile error, `cannot find type ResultShape` and `cannot find function responses_result_shape`.

- [ ] **Step 3: Implement.** Add the enum, the field and the function with the pinned table. In `responses()` of `turn.rs`, replace the two result arms with one lookup of `responses_result_shape(&ty)`. For `Approval`, the call id comes from `approval_request_id`. For the other shapes it comes from `call_id`. `item_id` is set only for `local_shell_call_output`. Keep the `_` arm logic for prior calls unchanged. Set `shape: ResultShape::Text` in the chat and Anthropic pushes. In `rewrite.rs`, delete `RESPONSES_OUTPUTS` and change the input loop to:

```rust
        match responses_result_shape(&ty) {
            Some(ResultShape::Text | ResultShape::ShellChunks | ResultShape::Screenshot) => { /* the current `output` notice branch */ }
            Some(ResultShape::Approval) => { /* the current mcp_approval_response branch */ }
            None => continue,
        }
```

Export `ResultShape` from `lib.rs` (`pub use turn::{.., ResultShape, ..}`). Update the doc of `turn.rs` at the old type list to say that `responses_result_shape` is the one list.

- [ ] **Step 4: Run and see it pass.** `cd rs && cargo nextest run --locked -p paigasus-gateway-core` passes, and every fixture of `request_rewrite.rs` passes unchanged. `cargo clippy --locked --all-targets -p paigasus-gateway-core -- -D warnings` is clean.

- [ ] **Step 5: Commit.**

```bash
git add rs/crates/libs/paigasus-gateway-core/src/turn.rs rs/crates/libs/paigasus-gateway-core/src/policy/rewrite.rs \
  rs/crates/libs/paigasus-gateway-core/src/policy/plan.rs rs/crates/libs/paigasus-gateway-core/src/lib.rs
git commit -m "refactor(rs): map each Responses result type to its ResultShape in gateway-core (SMA-776)" \
  -m "One table says how each result type is replaced. turn.rs and rewrite.rs use it. ToolResultRef gets a shape member. The behaviour does not change." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

## Task 3: The plan's refusal causes, and the service refuses an enforced computer block

**Files**
- Modify: `rs/crates/libs/paigasus-gateway-core/src/policy/plan.rs:53-85` (struct, `empty`, `is_noop`), `:159-177` (`choice_after` stays), `:262-298` (steps 3 and 4); tests `:301-851`.
- Modify: `rs/crates/libs/paigasus-gateway-core/src/policy/mod.rs:15`, `src/lib.rs:28-31` (export `RefusalCause`).
- Modify: `rs/crates/libs/paigasus-gateway-core/src/policy/rewrite.rs:90-93` (doc: `plan.refuse` becomes `plan.refusal()`).
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/request.rs:24-43` (`Refusal`), `:167-172` (`apply_inner`); tests.
- Modify: `rs/crates/services/paigasus-gateway/tests/tool_policy.rs:1111-1188` (the computer pair goes out).

**Interfaces**
- Produces (gateway-core):

```rust
/// Why the plan refuses the request (SMA-776 § 6.2). The order is the order of the plan steps.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefusalCause { UnreplaceableResult, UnknownToolChoice }

pub struct RequestPlan { /* .. */ pub refusals: BTreeMap<RefusalCause, bool>, /* `refuse: bool` is gone */ }
impl RequestPlan {
    /// The first enforced cause in map order, or `None`.
    #[must_use] pub fn refusal(&self) -> Option<RefusalCause>;
    /// Add a cause; `enforced` is ORed into the value (private).
    fn add_refusal(&mut self, cause: RefusalCause, enforced: bool);
}
```

- `is_noop()` uses `self.refusal().is_none()` in place of `!self.refuse`.
- Step 3: for each `r` in `turn.tool_results` with `r.shape == ResultShape::Screenshot`, when the `call_id` decide gives `Some(enforced)`, add `UnreplaceableResult` with `enforced`. Collect the value while `ResultJudge` holds `&mut plan`, and add it after the loop.
- Step 4 (planner's call): compute `let (_, refuse_all, reset_all) = choice_after(&turn.tool_choice, &all);` once. Add `UnknownToolChoice` with `true` when the enforced `refuse` is true. Else add it with `false` when `refuse_all` is true. `all` leaves out the kept Anthropic definition, because that definition stays in the request.
- Produces (service): `Refusal::UnreplaceableResult` (label `unreplaceable_result`) and

```rust
impl Refusal {
    /// The refusal for a plan cause. A cause that this service does not know fails closed (§ 6.4).
    fn from_cause(cause: RefusalCause) -> Self; // UnreplaceableResult, UnknownToolChoice, `_` => UnreadableTurn
}
```

- `apply_inner`: `if let Some(cause) = plan.refusal()`, then `refusal_event(ctx, mode, &refusal, refusal.label(), true)` and `return Err(refusal)`. The `cause` argument is the label (planner's call). The `rewrite_failed` branch does not change. The HTTP answer stays `invalid-request-body` until Task 6.

- [ ] **Step 1: Write the failing tests.** In `plan.rs` tests, add these helpers and tests:

```rust
    const COMPUTER_FLOOR: &str = r#"@id("f") forbid(principal, action == Pgs::Gateway::Action::"computer", resource);"#;

    fn shot(id: &str) -> ToolResultRef {
        ToolResultRef { call_id: Some(id.to_owned()), name: None, item_id: None, shape: ResultShape::Screenshot }
    }

    fn computer_turn() -> TurnFacts {
        let mut t = turn(vec![]);
        t.prior_calls = vec![prior("c1", "computer")];
        t.tool_results = vec![shot("c1")];
        t
    }

    fn causes(p: &RequestPlan) -> Vec<(RefusalCause, bool)> {
        p.refusals.iter().map(|(c, e)| (*c, *e)).collect()
    }

    #[test]
    fn an_enforced_block_of_a_screenshot_result_is_an_enforced_refusal() {
        let p = plan_request(&forbid("computer"), Format::OpenAiResponses, &computer_turn());
        assert_eq!(causes(&p), [(RefusalCause::UnreplaceableResult, true)]);
        assert_eq!(p.refusal(), Some(RefusalCause::UnreplaceableResult));
        assert!(!p.is_noop());
        // The report entry stays, so the caller sees which tool caused the refusal.
        assert_eq!(actions(&p), [(RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked)]);
        // An allowed computer call gives no cause.
        let p = plan_request(&forbid("other"), Format::OpenAiResponses, &computer_turn());
        assert!(p.refusals.is_empty() && p.is_noop());
    }

    #[test]
    fn a_log_only_block_of_a_screenshot_result_is_an_unenforced_cause() {
        let p = plan_request(&snap(NO_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly), Format::OpenAiResponses, &computer_turn());
        assert_eq!(causes(&p), [(RefusalCause::UnreplaceableResult, false)]);
        assert_eq!(p.refusal(), None);
        assert!(p.is_noop());
    }

    #[test]
    fn a_floor_block_of_a_screenshot_result_is_enforced_in_log_only_and_without_a_tenant_part() {
        for s in [
            snap(COMPUTER_FLOOR, PERMIT_ALL, TenantPart::Evaluated, TenantMode::LogOnly),
            snap(COMPUTER_FLOOR, "", TenantPart::Unavailable, TenantMode::LogOnly),
            snap(COMPUTER_FLOOR, "", TenantPart::Unavailable, TenantMode::Enforce),
        ] {
            let p = plan_request(&s, Format::OpenAiResponses, &computer_turn());
            assert_eq!(causes(&p), [(RefusalCause::UnreplaceableResult, true)]);
        }
    }

    #[test]
    fn a_screenshot_result_gives_the_cause_in_both_orders_of_one_call_id() {
        for results in [vec![shot("c1"), by_id("c1")], vec![by_id("c1"), shot("c1")]] {
            let mut t = computer_turn();
            t.tool_results = results;
            let p = plan_request(&forbid("computer"), Format::OpenAiResponses, &t);
            assert_eq!(p.replaced.len(), 1, "one target");
            assert_eq!(causes(&p), [(RefusalCause::UnreplaceableResult, true)]);
        }
    }

    /// Review Focus 1: the value is enforced when any finding of the cause is enforced.
    #[test]
    fn two_findings_of_one_cause_are_enforced_when_any_is() {
        let s = snap(RM_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly);
        for results in [vec![shot("c1"), shot("c2")], vec![shot("c2"), shot("c1")]] {
            let mut t = turn(vec![]);
            // "computer" is a log-only tenant deny; "rm" is a floor deny.
            t.prior_calls = vec![prior("c1", "computer"), prior("c2", "rm")];
            t.tool_results = results;
            let p = plan_request(&s, Format::OpenAiResponses, &t);
            assert_eq!(causes(&p), [(RefusalCause::UnreplaceableResult, true)]);
        }
    }

    #[test]
    fn an_unknown_tool_choice_with_only_log_only_removals_is_an_unenforced_cause() {
        let log_only = snap(NO_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly);
        let p = plan_request(&log_only, Format::OpenAiChat, &with_choice(&["a"], ToolChoice::Unknown));
        assert_eq!(causes(&p), [(RefusalCause::UnknownToolChoice, false)]);
        assert_eq!(p.refusal(), None);
        assert!(p.is_noop());
        // No removal at all: no cause.
        let p = plan_request(&log_only, Format::OpenAiChat, &with_choice(&["ok"], ToolChoice::Unknown));
        assert!(p.refusals.is_empty());
    }

    #[test]
    fn the_refusal_is_the_first_enforced_cause_in_plan_order() {
        let mut t = with_choice(&["computer"], ToolChoice::Unknown);
        t.definitions = vec![other("computer")];
        t.prior_calls = vec![prior("c1", "computer")];
        t.tool_results = vec![shot("c1")];
        let p = plan_request(&forbid("computer"), Format::OpenAiResponses, &t);
        assert_eq!(causes(&p), [(RefusalCause::UnreplaceableResult, true), (RefusalCause::UnknownToolChoice, true)]);
        assert_eq!(p.refusal(), Some(RefusalCause::UnreplaceableResult));
    }
```

Change the existing `refuse` asserts: `assert!(p.refuse)` becomes `assert_eq!(p.refusal(), Some(RefusalCause::UnknownToolChoice))`. `assert!(!p.refuse)` becomes `assert_eq!(p.refusal(), None)`. A tuple `(p.choice.clone(), p.refuse)` becomes `(p.choice.clone(), p.refusal())` with `None`. In the service `request.rs` tests, add:

```rust
    const FORBID_COMPUTER: &str = r#"@id("p") permit(principal, action, resource); @id("t") forbid(principal, action == Pgs::Gateway::Action::"computer", resource);"#;
    const FLOOR_COMPUTER: &str = r#"@id("floor-computer") forbid(principal, action == Pgs::Gateway::Action::"computer", resource);"#;

    /// A Responses request with one computer call and its screenshot output.
    fn computer_request(extra: &str) -> Bytes {
        Bytes::from(format!(
            r#"{{"model":"m"{extra},"input":[{{"type":"computer_call","id":"cu_1","call_id":"c3","actions":[{{"type":"screenshot"}}],"status":"completed"}},{{"type":"computer_call_output","call_id":"c3","output":{{"type":"computer_screenshot","image_url":"data:image/png;base64,SECRET"}}}}]}}"#
        ))
    }

    async fn run_responses(policy: &RequestToolPolicy, body: Bytes) -> Result<RequestOutcome, Refusal> {
        let turn = parse_request(Format::OpenAiResponses, &body);
        policy.apply(&ctx(), "m", Format::OpenAiResponses, &turn, body).await
    }

    #[tokio::test]
    async fn a_blocked_computer_result_is_refused_in_enforce_and_by_the_floor_in_log_only() {
        let enforce = testing::policy(&FakeSource::new(testing::FLOOR, FORBID_COMPUTER), settings(TenantMode::Enforce));
        assert_eq!(run_responses(&enforce, computer_request("")).await.err(), Some(Refusal::UnreplaceableResult));
        let floor = testing::policy(&FakeSource::new(FLOOR_COMPUTER, testing::PERMIT_ALL), settings(TenantMode::LogOnly));
        assert_eq!(run_responses(&floor, computer_request("")).await.err(), Some(Refusal::UnreplaceableResult));
    }
```

In `tests/tool_policy.rs` `tool_result_replacement_for_each_result_type`: delete the `computer_call` and `computer_call_output` items, change the notice loop to `for i in [4, 6, 8, 10, 12]`, change `items[16]["approve"]` to `items[14]["approve"]`, and delete `"computer"` from the `want` list.

- [ ] **Step 2: Run and see them fail.** `cd rs && cargo nextest run --locked -p paigasus-gateway-core policy::plan`. Expected: compile error, `no field refusals`, `cannot find type RefusalCause`.

- [ ] **Step 3: Implement** the interfaces above. Replace the doc of the old `refuse` field with a doc of `refusals` ("Each cause that the plan found, and whether it is enforced (SMA-776 § 6.2)"). Change the `Refusal` doc at `request.rs:24` to "Why the request side refuses a request. Each refusal is a 400 (P-10, SMA-776)."

- [ ] **Step 4: Run and see them pass.** `cd rs && cargo nextest run --locked -p paigasus-gateway-core` passes. `cargo nextest run --locked -p paigasus-gateway --lib --test tool_policy` passes; this includes the edited HTTP test. Clippy is clean for both crates.

- [ ] **Step 5: Commit.**

```bash
git add rs/crates/libs/paigasus-gateway-core/src rs/crates/services/paigasus-gateway/src/application/tool_policy/request.rs \
  rs/crates/services/paigasus-gateway/tests/tool_policy.rs
git commit -m "feat(rs): refuse an enforced block of a computer result in the request plan (SMA-776)" \
  -m "The plan records each refusal cause with an enforced flag. An enforced block of a computer_call_output refuses the request, also by the floor in log-only mode." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

## Task 4: The shell notice chunk, the screenshot fail-closed, and the fixtures

**Files**
- Modify: `rs/crates/libs/paigasus-gateway-core/src/policy/rewrite.rs:88-114` (`rewrite_request` and its doc), `:253-341` (`responses`); add a `#[cfg(test)] mod tests` at the end.
- Modify: `rs/crates/libs/paigasus-gateway-core/tests/fixtures/request_rewrite/responses_outputs_replaced.in.json` and `.out.json` (delete lines 7-8, the computer pair; change the `shell_call_output` line of `.out.json`).
- Modify: `rs/crates/libs/paigasus-gateway-core/tests/request_rewrite.rs:1-4` (module doc), `:84-88` (`CASES` row); add tests.
- Modify: `rs/crates/libs/paigasus-gateway-core/tests/fixtures/request_rewrite/README.md`.
- Modify: `rs/crates/services/paigasus-gateway/tests/tool_policy.rs` (`tool_result_replacement_for_each_result_type`, the shell item).

**Interfaces**
- Consumes: `responses_result_shape`, `ResultShape` (Task 2); `RequestPlan::refusals` (Task 3).
- Produces (planner's call: `rewrite_request` keeps `Option<Bytes>`, so the service log cause `rewrite_failed` stays as it is):

```rust
/// `None` also when an enforced target is a `computer_call_output`: it has no replacement
/// (SMA-776 § 6.3), so the caller must not send the body.
pub fn rewrite_request(format: Format, body: &[u8], plan: &RequestPlan) -> Option<Bytes>;
fn responses(top: &RawObject, en: &Enforced, choice: &ChoiceRewrite, edit: &mut ObjectEdit) -> Option<()>;
/// The one notice chunk of a blocked `shell_call_output`. `notice` is a JSON string literal.
fn shell_chunk(notice: &str) -> String;
```

- `shell_chunk` returns exactly `format!(r#"[{{"stdout":"","stderr":{notice},"outcome":{{"type":"exit","exit_code":1}}}}]"#)`.
- Loop: `Text` sets `output` to the notice (as now). `ShellChunks` sets `output` to `shell_chunk(notice)`. `Screenshot` returns `None` when `en.id_notice(&item, "call_id")` is `Some`, else it continues. `Approval` is as now. `rewrite_request` uses `responses(..)?`.
- New `.out.json` line for `c5` (the `"output": ` spacing of the input stays):

```json
    {"type": "shell_call_output", "call_id": "c5", "output": [{"stdout":"","stderr":"[paigasus] tool result for \"shell\" was replaced (tool-result-blocked).","outcome":{"type":"exit","exit_code":1}}]},
```

- `CASES` row: `("responses_outputs_replaced", Format::OpenAiResponses, &["delete_file", "run_sql", "local_shell", "shell", "apply_patch"])`.

- [ ] **Step 1: Write the failing tests.** In `rewrite.rs` add:

```rust
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::policy::plan::PlanItem;
    use crate::report::RequestEntry;

    /// A hand-made plan: the service refuses this plan before the rewrite, so only a direct call
    /// can give the rewrite an enforced computer target (SMA-776 § 6.3, fail closed).
    #[test]
    fn an_enforced_computer_output_target_fails_closed() {
        let body = br#"{"model":"m","input":[{"type":"computer_call","call_id":"c3","actions":[{"type":"screenshot"}]},{"type":"computer_call_output","call_id":"c3","output":{"type":"computer_screenshot","image_url":"SECRET"}},{"type":"function_call_output","call_id":"c3","output":"x"}]}"#;
        let mut plan = RequestPlan {
            removed: Vec::new(),
            replaced: vec![PlanItem { target: ResultTarget::CallId("c3".to_owned()), enforced: true }],
            choice: ChoiceRewrite::Keep,
            refusals: BTreeMap::new(),
            entries: vec![RequestEntry::new("computer".to_owned(), RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked)],
        };
        assert!(!plan.is_noop());
        assert_eq!(rewrite_request(Format::OpenAiResponses, body, &plan), None);
        plan.replaced[0].enforced = false;
        assert_eq!(rewrite_request(Format::OpenAiResponses, body, &plan), None, "a no-op plan");
    }
}
```

In `request_rewrite.rs` add (import `RefusalCause`):

```rust
/// G3: each Responses output type keeps its schema after the rewrite.
#[test]
fn each_responses_output_type_keeps_its_schema() {
    let input = fixture("responses_outputs_replaced", "in");
    let tools = ["delete_file", "run_sql", "local_shell", "shell", "apply_patch"];
    let p = plan(&enforce(Format::OpenAiResponses, &tools), Format::OpenAiResponses, input.as_bytes());
    let out: Value = serde_json::from_str(&rewrite(Format::OpenAiResponses, input.as_bytes(), &p)).expect("json");
    let notice = |t: &str| Value::from(result_notice(t, RequestReason::ToolResultBlocked));
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for item in out["input"].as_array().expect("input") {
        let ty = item["type"].as_str().unwrap_or("message");
        match ty {
            "function_call_output" => assert_eq!(item["output"], notice("delete_file")),
            "custom_tool_call_output" => assert_eq!(item["output"], notice("run_sql")),
            "apply_patch_call_output" => assert_eq!(item["output"], notice("apply_patch")),
            "local_shell_call_output" => assert!(item["output"].is_string(), "{item}"),
            "shell_call_output" => {
                assert_eq!(item["output"], json!([{"stdout": "", "stderr": notice("shell"), "outcome": {"type": "exit", "exit_code": 1}}]));
                assert_eq!(item["output"][0].as_object().map(|o| o.len()), Some(3), "exactly stdout, stderr, outcome");
            }
            _ => continue,
        }
        *seen.entry(ty).or_default() += 1;
    }
    let want = [("apply_patch_call_output", 1), ("custom_tool_call_output", 1), ("function_call_output", 1), ("local_shell_call_output", 4), ("shell_call_output", 1)];
    assert_eq!(seen, BTreeMap::from(want));
}

/// Review Focus 2: one chunk for two commands; every other member keeps its bytes.
#[test]
fn a_shell_notice_is_one_chunk_and_keeps_every_other_member() {
    let body = concat!(
        r#"{"model":"m","input":[{"type":"shell_call","id":"sh_1","call_id":"c5","action":{"commands":["ls","cat /etc/hosts"]},"status":"completed"},"#,
        r#"{"type":"shell_call_output", "id" : "sho_1","call_id":"c5","status":"completed","max_output_length" : 10,"caller":{"type":"x"},"#,
        r#""output":[{"stdout":"SECRET-1","stderr":"","outcome":{"type":"exit","exit_code":0}},{"stdout":"SECRET-2","stderr":"","outcome":{"type":"timeout"}}]}]}"#
    );
    let p = plan(&enforce(Format::OpenAiResponses, &["shell"]), Format::OpenAiResponses, body.as_bytes());
    let out = rewrite(Format::OpenAiResponses, body.as_bytes(), &p);
    assert!(!out.contains("SECRET"));
    let item = |s: &str| -> String {
        let top: BTreeMap<String, &RawValue> = serde_json::from_str(s).expect("object");
        let items: Vec<&RawValue> = serde_json::from_str(top["input"].get()).expect("input");
        items[1].get().to_owned()
    };
    let members = |s: String| serde_json::from_str::<BTreeMap<String, &RawValue>>(&s).expect("item").into_iter().map(|(k, v)| (k, v.get().to_owned())).collect::<BTreeMap<_, _>>();
    let (mut a, mut b) = (members(item(body)), members(item(&out)));
    let chunk = b.remove("output").expect("output");
    a.remove("output");
    assert_eq!(a, b, "every member except output keeps its bytes");
    let notice = serde_json::to_string(&result_notice("shell", RequestReason::ToolResultBlocked)).expect("json");
    assert_eq!(chunk, format!(r#"[{{"stdout":"","stderr":{notice},"outcome":{{"type":"exit","exit_code":1}}}}]"#));
}

/// Review Focus 3: a log-only computer block is a cause, not a failure of the other items.
#[test]
fn an_unenforced_computer_block_leaves_the_output_and_the_rest_applies() {
    let body = json!({"model":"m","tools":[{"type":"function","name":"rm"},{"type":"function","name":"ok"}],"input":[
        {"type":"computer_call","id":"cu_1","call_id":"c3","actions":[{"type":"screenshot"}],"status":"completed"},
        {"type":"computer_call_output","call_id":"c3","output":{"type":"computer_screenshot","image_url":"data:image/png;base64,AAAA"}}]})
    .to_string();
    let floor = r#"@id("f") forbid(principal, action == Pgs::Gateway::Action::"rm", resource);"#;
    let snap = snapshot(Format::OpenAiResponses, floor, &forbid_text(&["computer"]), TenantMode::LogOnly);
    let p = plan(&snap, Format::OpenAiResponses, body.as_bytes());
    assert_eq!(p.refusals.iter().map(|(c, e)| (*c, *e)).collect::<Vec<_>>(), [(RefusalCause::UnreplaceableResult, false)]);
    assert_eq!(p.refusal(), None);
    let before: Value = serde_json::from_str(&body).expect("json");
    let out: Value = serde_json::from_str(&rewrite(Format::OpenAiResponses, body.as_bytes(), &p)).expect("json");
    assert_eq!(out["tools"], json!([{"type":"function","name":"ok"}]));
    assert_eq!(out["input"], before["input"], "the screenshot is not changed");
}
```

In `tests/tool_policy.rs` `tool_result_replacement_for_each_result_type`, change the notice loop to `for i in [4, 6, 8, 12]` and add:

```rust
            let notice = items[10]["output"][0]["stderr"].clone();
            assert!(is_notice(&notice), "{ep:?}: {}", items[10]);
            assert_eq!(items[10]["output"], json!([{"stdout": "", "stderr": notice, "outcome": {"type": "exit", "exit_code": 1}}]), "{ep:?}");
```

- [ ] **Step 2: Run and see them fail.** `cd rs && cargo nextest run --locked -p paigasus-gateway-core -E 'test(/fails_closed|keeps_its_schema|one_chunk|unenforced_computer/)'`. Expected: `an_enforced_computer_output_target_fails_closed` fails (a body comes back), `each_responses_output_type_keeps_its_schema` fails (`output` is a string), `a_shell_notice_is_one_chunk...` fails. The Review Focus 3 test passes already; it is a guard for this task.

- [ ] **Step 3: Implement** the interfaces above. Edit the fixture pair and the `CASES` row. Change the `request_rewrite.rs` module doc to: "Each fixture pair is built from the provider API reference. The README there says which cases SMA-776 measured against the real upstream." Rewrite the README:
  - The case table: the `responses_outputs_replaced` row says "every Text output type gets the notice string; `shell_call_output` gets one notice chunk; the `computer_call_output` case is not a fixture, because the gateway refuses it (inline tests in `src/policy/rewrite.rs` and `src/policy/plan.rs`)". Its last cell says "measured (SMA-776, see below)". The `responses_mcp_approval_refused` row says "not measured: it needs an `mcp_approval_request` from a real MCP server". Every other row says "not measured (SMA-776 measured only the Responses result types)" (planner's call: § 6.6 gives no reason for these rows).
  - A new section `## Responses result types, real upstream (2026-11-16, SMA-776)`: copy the table of spec § 4 and its three bullets verbatim. Add one line: "`local_shell_call_output`: not measured, because `gpt-6-sol` rejects the `local_shell_call` item."
  - Delete the paragraph that starts "Two shapes need the real upstream answer most" and ends "in these two places."

- [ ] **Step 4: Run and see them pass.** `cd rs && cargo nextest run --locked -p paigasus-gateway-core` passes. `cargo nextest run --locked -p paigasus-gateway --test tool_policy tool_result_replacement_for_each_result_type` passes. Clippy is clean for both crates.

- [ ] **Step 5: Commit.**

```bash
git add rs/crates/libs/paigasus-gateway-core/src/policy/rewrite.rs rs/crates/libs/paigasus-gateway-core/tests \
  rs/crates/services/paigasus-gateway/tests/tool_policy.rs
git commit -m "fix(rs): send a schema-valid notice chunk for a blocked shell_call_output (SMA-776)" \
  -m "A blocked shell result becomes one chunk with the notice in stderr and exit code 1. An enforced computer target makes the rewrite fail closed. The fixture README records the real upstream measurement." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

## Task 5: The `refused` label, and an event for each unenforced cause (G5)

**Files**
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/request.rs:60-75` (`refusal_event`), `:167-172` (`apply_inner`); tests `:225-252` and new tests.
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs:62-65` (doc), `rs/crates/services/paigasus-gateway/src/main.rs:289-292` (describe text).
- Modify: `rs/crates/services/paigasus-gateway/tests/tool_policy.rs:551` (the `refusals` closure).

**Interfaces**
- Consumes: `RequestPlan::refusal`, `RequestPlan::refusals`, `Refusal::from_cause` (Task 3).
- `refusal_event` adds the label `"refused" => if refused { "true" } else { "false" }`. Its signature does not change.
- `apply_inner`: on `Some(cause)`, as in Task 3 (one event, `refused = true`, no event for the other causes). On `None`, call `refusal_event(ctx, mode, &r, r.label(), false)` once for each key of `plan.refusals`, with `r = Refusal::from_cause(cause)`, then continue to the rewrite.
- `names.rs` doc: "Requests that the tool policy refused, and refusals that the mode did not enforce (D17, P-10, SMA-776 G5). `mode` = `off` | `log_only` | `enforce`; `reason` = `unreadable_turn` | `unknown_tool_choice` | `unreplaceable_result`; `refused` = `true` (a 400) | `false` (forwarded; enforce mode would refuse it)."
- `main.rs` text: "Requests the tool policy refused, and refusals that the mode did not enforce, labeled by mode, reason (unreadable_turn, unknown_tool_choice, unreplaceable_result) and refused (true, false)."

- [ ] **Step 1: Write the failing tests** in `request.rs` tests (`refusals` is a helper for the three labels):

```rust
    fn refusals(m: &Snap, mode: &str, reason: &str, refused: &str) -> u64 {
        m.counter(names::GATEWAY_TOOL_POLICY_REQUEST_REFUSALS_TOTAL, &[("mode", mode), ("reason", reason), ("refused", refused)])
    }

    #[tokio::test]
    async fn a_computer_block_is_counted_as_refused_or_as_not_enforced() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let source = FakeSource::new(testing::FLOOR, FORBID_COMPUTER);
        let enforce = testing::policy(&source, settings(TenantMode::Enforce));
        assert_eq!(run_responses(&enforce, computer_request("")).await.err(), Some(Refusal::UnreplaceableResult));
        let m = Snap::take(&snapshotter);
        assert_eq!((refusals(&m, "enforce", "unreplaceable_result", "true"), refusals(&m, "enforce", "unreplaceable_result", "false")), (1, 0));

        let log_only = testing::policy(&source, settings(TenantMode::LogOnly));
        let body = computer_request("");
        let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::INFO);
        let out = run_responses(&log_only, body.clone()).await.expect("log_only forwards");
        assert_eq!(out.body.as_ptr(), body.as_ptr(), "the original bytes");
        let m = Snap::take(&snapshotter);
        assert_eq!((refusals(&m, "log_only", "unreplaceable_result", "true"), refusals(&m, "log_only", "unreplaceable_result", "false")), (0, 1));
        let line = one_line(&logs.text(), "tool-policy request refusal");
        for want in ["req-1", "log_only", "unreplaceable_result", "refused=false"] {
            assert!(line.contains(want), "{want:?} in {line}");
        }
        assert!(!line.contains("SECRET") && !line.contains("computer\""), "{line}");
    }

    #[tokio::test]
    async fn an_unknown_tool_choice_with_a_log_only_removal_is_counted_not_refused() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let log_only = testing::policy(&FakeSource::new(testing::FLOOR, FORBID_X), settings(TenantMode::LogOnly));
        let body = chat_request(&["x", "y"], r#","tool_choice":"weird""#);
        let out = run(&log_only, body.clone()).await.expect("log_only forwards");
        assert_eq!(out.body, body);
        assert_eq!(refusals(&Snap::take(&snapshotter), "log_only", "unknown_tool_choice", "false"), 1);
    }

    /// Review Focus 4: two enforced causes, one refused sample.
    #[tokio::test]
    async fn a_request_with_two_enforced_causes_counts_once() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let tenant = r#"@id("p") permit(principal, action, resource); @id("t") forbid(principal, action in [Pgs::Gateway::Action::"x", Pgs::Gateway::Action::"computer"], resource);"#;
        let enforce = testing::policy(&FakeSource::new(testing::FLOOR, tenant), settings(TenantMode::Enforce));
        let body = computer_request(r#","tools":[{"type":"function","name":"x","parameters":{}}],"tool_choice":{"type":"a_future_kind"}"#);
        assert_eq!(run_responses(&enforce, body).await.err(), Some(Refusal::UnreplaceableResult));
        let m = Snap::take(&snapshotter);
        assert_eq!(refusals(&m, "enforce", "unreplaceable_result", "true"), 1);
        assert_eq!(refusals(&m, "enforce", "unknown_tool_choice", "true") + refusals(&m, "enforce", "unknown_tool_choice", "false"), 0);
    }
```

In `an_unavailable_turn_is_refused_only_in_enforce`, replace the `refusals` closure: enforce asserts `refusals(&m, "enforce", "unreadable_turn", "true") == 1`, log-only asserts `refusals(&m, "log_only", "unreadable_turn", "false") == 1`, off asserts 0 for both values. In `tests/tool_policy.rs:551`, the closure gets the label `("refused", if mode == "enforce" { "true" } else { "false" })`.

- [ ] **Step 2: Run and see them fail.** `cd rs && cargo nextest run --locked -p paigasus-gateway --lib refus counted counts_once`. Expected: FAIL, the counter with the `refused` label reads 0.

- [ ] **Step 3: Implement** the interfaces above.

- [ ] **Step 4: Run and see them pass.** `cd rs && cargo nextest run --locked -p paigasus-gateway --lib --test tool_policy` passes. `cargo nextest run --locked -p paigasus-observability` passes. `moon run repo:observability-drift` passes. Clippy is clean.

- [ ] **Step 5: Commit.**

```bash
git add rs/crates/services/paigasus-gateway/src/application/tool_policy/request.rs rs/crates/services/paigasus-gateway/src/main.rs \
  rs/crates/libs/paigasus-observability/src/names.rs rs/crates/services/paigasus-gateway/tests/tool_policy.rs
git commit -m "feat(rs): report each refusal cause with the refused label (SMA-776)" \
  -m "The refusal metric gets the label refused. A refused request adds one sample with refused true. Each cause that the mode did not enforce adds an event and a sample with refused false." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

## Task 6: The HTTP answer `tool-result-unreplaceable` and the header on a refusal

**Files**
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/request.rs:24-52` (`Refused`), `:135-194` (`apply`, `apply_inner`); every `.err()` in its tests.
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/mod.rs:15` (export `Refused`).
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs:127-136` (variant), `:139-147` (new `From`), `:264-277` (`parts`), `:302-318` (`retryable`); test after `:636`.
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/pipeline.rs:120-124`.
- Modify: `rs/crates/services/paigasus-gateway/src/lib.rs:28-33`, `rs/crates/services/paigasus-gateway/gateway.toml.example:190-194`.
- Test: `rs/crates/services/paigasus-gateway/tests/tool_policy.rs` (new test; `:546-598`; `:1017-1020`).

**Interfaces**
- Produces (planner's call for spec § 8 "the exact type"):

```rust
/// A refused request: why, and the plan entries for the decision header (SMA-776 § 6.5).
/// `entries` is empty when no plan exists (an unreadable turn).
#[derive(Debug, PartialEq, Eq)]
pub struct Refused { pub refusal: Refusal, pub entries: Vec<RequestEntry> }

pub async fn apply(&self, ctx: &DecisionContext, model: &str, format: Format, turn: &ToolTurn, body: Bytes) -> Result<RequestOutcome, Refused>;

// `Refusal` derives Debug, Clone, Copy, PartialEq, Eq.
pub enum GatewayError { /* .. */ ToolResultUnreplaceable }
impl From<Refusal> for GatewayError; // UnreadableTurn | UnknownToolChoice => BadRequestBody; UnreplaceableResult => ToolResultUnreplaceable
```

- `parts()` row: `(StatusCode::BAD_REQUEST, "invalid_request_error", Some("tool-result-unreplaceable"), Some("input"), "A blocked computer_call_output cannot be replaced. Remove the computer call and its output from the input.")`. `retryable()`: `Retryable::No`.
- `apply_inner`: the plan-cause refusal and the `rewrite_failed` refusal return `Refused { refusal, entries: plan.entries }`. The unreadable-turn refusal returns empty entries.
- Pipeline `Err(refused)`: debug line with `reason = ?refused.refusal`, then `let mut resp = GatewayError::from(refused.refusal).into_response(); add_request_report(&mut resp, &refused.entries); return resp;`.
- New `Refusal` doc: "`UnreadableTurn` and `UnknownToolChoice` are a 400 `invalid-request-body`. `UnreplaceableResult` is a 400 `tool-result-unreplaceable` (SMA-776)."

- [ ] **Step 1: Write the failing tests.** In `error.rs` tests:

```rust
    /// SMA-776: the computer refusal names `input`, and its message names no tool and no content.
    #[tokio::test]
    async fn the_unreplaceable_result_refusal_names_the_input() {
        use paigasus_observability::Retryable;

        use crate::application::tool_policy::Refusal;
        assert_eq!(GatewayError::from(Refusal::UnreadableTurn), GatewayError::BadRequestBody);
        assert_eq!(GatewayError::from(Refusal::UnknownToolChoice), GatewayError::BadRequestBody);
        let err = GatewayError::from(Refusal::UnreplaceableResult);
        assert_eq!(err, GatewayError::ToolResultUnreplaceable);
        assert_eq!(err.retryable(), Retryable::No);
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(resp.headers()["paigasus-retryable"], "false");
        let body = body_json(resp).await;
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert_eq!(body["error"]["code"], "tool-result-unreplaceable");
        assert_eq!(body["error"]["param"], "input");
        assert_eq!(
            body["error"]["message"],
            "A blocked computer_call_output cannot be replaced. Remove the computer call and its output from the input."
        );
    }
```

In `tests/tool_policy.rs` add:

```rust
const FLOOR_COMPUTER: &str = "// SPDX-License-Identifier: Apache-2.0\n@id(\"floor-computer\") forbid(principal, action == Pgs::Gateway::Action::\"computer\", resource);\n";
const TENANT_COMPUTER: &str = r#"@id("p") permit(principal is Pgs::Iam::Principal, action, resource); @id("t") forbid(principal, action == Pgs::Gateway::Action::"computer", resource);"#;

fn computer_body(ep: Ep) -> String {
    ep.body(json!({"model":"gpt-6","tools":responses_tools(&["read_file"]),"input":[
        {"role":"user","content":"hi"},
        {"type":"computer_call","id":"cc_1","call_id":"cc1","actions":[{"type":"screenshot"}],"status":"completed"},
        {"type":"computer_call_output","call_id":"cc1","output":{"type":"computer_screenshot","image_url":"SECRET-SCREEN"}}]}))
}

/// SMA-776 G2, § 6.5: an enforced computer block is a 400 before the upstream, with the header.
/// The floor enforces it in log-only too (D33). A log-only tenant block is forwarded unchanged.
#[tokio::test]
async fn a_blocked_computer_result_is_refused_before_the_upstream() {
    for ep in [Ep::RespJson, Ep::RespSse] {
        for (floor, tenant, mode) in [(FLOOR, TENANT_COMPUTER, TenantMode::Enforce), (FLOOR_COMPUTER, TENANT, TenantMode::LogOnly)] {
            let dir = policy_dir(floor, tenant);
            let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, mode), &computer_body(ep), &[]).await;
            let case = format!("{ep:?} {mode:?}");
            assert_eq!(ex.status, StatusCode::BAD_REQUEST, "{case}: {}", ex.text);
            // Review Focus 5: a refused stream request is a JSON answer, not SSE.
            assert!(ex.sent.header("content-type").starts_with("application/json"), "{case}");
            let err = &ex.sent.json()["error"];
            assert_eq!((err["code"].as_str(), err["param"].as_str(), err["type"].as_str()), (Some("tool-result-unreplaceable"), Some("input"), Some("invalid_request_error")), "{case}");
            assert_eq!(request_entries(&ex.entries), [req("computer", "replace-result", true)], "{case}");
            assert!(ex.upstream.is_none(), "{case}: nothing goes upstream");
            assert!(!ex.text.contains("SECRET"), "{case}");
        }
        // A log-only tenant block is not enforced: the original bytes go upstream.
        let dir = policy_dir(FLOOR, TENANT_COMPUTER);
        let body = computer_body(ep);
        let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::LogOnly), &body, &[]).await;
        assert_eq!(ex.status, StatusCode::OK, "{ep:?}: {}", ex.text);
        assert_eq!(ex.upstream.as_deref(), Some(body.as_bytes()), "{ep:?}");
        assert_eq!(request_entries(&ex.entries), [req("computer", "replace-result", false)], "{ep:?}");
    }
}
```

After `assert!(ex.upstream.is_none(), "{ep:?}");` at `:1020` add `assert_eq!(request_entries(&ex.entries), [req("blocked_rm", "remove-definition", true)], "{ep:?}");`. In `an_unavailable_turn_is_refused_in_enforce_and_counted_in_log_only`, after the `invalid-request-body` assert add `assert!(sent.headers.get("paigasus-tool-decisions").is_none(), "no plan, no header ({ep:?})");`. In `request.rs` tests, change every `.err()` comparison to `.err().map(|r| r.refusal)`. In `a_blocked_computer_result_is_refused_in_enforce_and_by_the_floor_in_log_only`, also assert that `entries` has one entry `("computer", RequestAction::ReplaceResult, true)`.

- [ ] **Step 2: Run and see them fail.** `cd rs && cargo nextest run --locked -p paigasus-gateway --lib --test tool_policy`. Expected: compile error (`no variant ToolResultUnreplaceable`, `From<Refusal>` missing). After a stub, the HTTP test fails with `invalid-request-body` and an empty header.

- [ ] **Step 3: Implement** the interfaces above. Update the `apply` `# Errors` doc: "A [`Refused`] when the request must be refused with a 400 (D17, § 6.5.1 step 4, SMA-776). It carries the plan entries for the header." Change `lib.rs:31-32` to: "It refuses a request whose tools it cannot read in `enforce` with a 400 `invalid-request-body`. It refuses a request with an enforced block of a `computer_call_output` with a 400 `tool-result-unreplaceable`, because that item has no field for the notice (SMA-776)." Replace `gateway.toml.example:191-194` (from "Some request shapes" to "the turn stops.") with:

```toml
# A blocked shell_call_output becomes one notice chunk; the real upstream accepts it (SMA-776).
# A blocked computer_call_output has no field for the notice. An enforced block refuses the
# request with a 400 tool-result-unreplaceable. A floor block of "computer" does this in every
# mode. See rs/crates/libs/paigasus-gateway-core/tests/fixtures/request_rewrite/README.md.
```

- [ ] **Step 4: Run and see them pass.** `cd rs && cargo nextest run --locked -p paigasus-gateway --lib --test tool_policy` passes, then `moon run paigasus-gateway-rs:test` (Docker-backed suites included). `moon run repo:error-code-single-site paigasus-gateway-rs:lint` passes. `cargo fmt --check` is clean.

- [ ] **Step 5: Commit.**

```bash
git add rs/crates/services/paigasus-gateway/src rs/crates/services/paigasus-gateway/gateway.toml.example \
  rs/crates/services/paigasus-gateway/tests/tool_policy.rs
git commit -m "feat(rs): map the computer refusal to tool-result-unreplaceable with the header (SMA-776)" \
  -m "The refusal of apply carries the plan entries, so a refused request gets the decision header. A blocked computer_call_output gives a 400 with code tool-result-unreplaceable and param input." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

## Task 7: The SMA-752 spec

**Files**
- Modify: `docs/superpowers/specs/2026-11-15-sma-752-tool-invoke-decisions-design.md`: D11 row (`:204`), § 6.5.0 (`:545-553`), § 6.5.2 (`:609-610`), § 6.5.3 (`:612-628`), § 6.6 (`:630-640`).

**Interfaces:** none (documentation only).

- [ ] **Step 1: Edit.** Use this text (STE):
  - D11 row, append: "Exception (SMA-776 § 5.1): an enforced block of a Responses `computer_call_output` refuses the request with a 400 `tool-result-unreplaceable` (318). No field of that item can carry the notice."
  - § 6.5.0: the signature becomes `-> Result<RequestOutcome, Refused>`. Replace "`Refusal` is the D17 400." with "`Refusal` is the D17 400. Since SMA-776 it is also the 400 `tool-result-unreplaceable` for an enforced block of a `computer_call_output`. `Refused` carries the `Refusal` and the plan entries, so the refusal carries the decision header."
  - § 6.5.2: replace "The notice replaces the whole content (ADR-0027 (c))." with the table of SMA-776 spec § 6.1 (four rows, three columns), and the line "See the SMA-776 spec (`2026-11-16-sma-776-blocked-structured-results-design.md`)." The sentence about the fixed text stays.
  - § 6.5.3: in the "Non-stream" bullet, after "on a non-2xx upstream answer", add "on a request-side refusal (SMA-776)". Add a bullet: "On a refused request, the header is sent when the plan has entries, stream or not. An entry with `enforced: true` then means that the gateway would apply it. The gateway refused the request in place of the change (SMA-776 § 6.2)."
  - § 6.6: add "SMA-776 adds `ERROR_REASON_TOOL_RESULT_UNREPLACEABLE = 318` (`tool-result-unreplaceable`) under the same banner. It is the only code of that banner that is an HTTP error. `adapters/http/error.rs` already has an `emits` row, so it needs no new `MANIFEST` row."

- [ ] **Step 2: Check.** `grep -n "tool-result-unreplaceable" docs/superpowers/specs/2026-11-15-sma-752-tool-invoke-decisions-design.md` gives 4 or more lines. `grep -c "The notice replaces the whole content" <same file>` gives 0.

- [ ] **Step 3: Full graph before push.** Run the `moon ci` command between the `ci-targets` markers of the root `CLAUDE.md`, with the bash rules of "This development Mac only". Every task passes, or each red task is a documented bash-version artifact that passes under the other bash.

- [ ] **Step 4: Commit.**

```bash
git add docs/superpowers/specs/2026-11-15-sma-752-tool-invoke-decisions-design.md
git commit -m "docs(rs): update the SMA-752 spec for blocked structured results (SMA-776)" \
  -m "Record the D11 exception, the ResultShape table, the header on a refusal and reason 318." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Spec coverage

| Spec section | Task |
|---|---|
| § 1, § 2 G1 (shell chunk) | 4 |
| § 2 G2 (computer refusal), § 5.1 | 3, 6 |
| § 2 G3 (exact shapes, core and HTTP) | 4 (`each_responses_output_type_keeps_its_schema`, HTTP shell assert) |
| § 2 G4, § 6.6 (docs, README, comments) | 3 (plan.rs and request.rs docs), 4 (README, rewrite doc), 6 (lib.rs, toml example), 7 (SMA-752 spec) |
| § 2 G5 (unenforced causes) | 3 (plan), 5 (events, metric) |
| § 2 non-goals (Chat, Anthropic, four Text types, `mcp_call`) | 2 (table keeps them `Text`), 4 (schema test) |
| § 3 constraints (raw bytes, D25, D33, no quota, error-code steps) | 1, 3, 4, 6 |
| § 4 measurement into the README | 4 |
| § 6.1 `ResultShape` table, `ToolResultRef.shape` | 2 |
| § 6.2 plan `refusals`, `refusal()`, steps 3 and 4 | 3 |
| § 6.3 rewrite (shell chunk, screenshot fail closed) | 4 |
| § 6.4 service `Refusal`, `_` arm, reporting, metric label and text | 3, 5 |
| § 6.5 HTTP code 318, `From<Refusal>`, message, header, stream refusal | 1, 6 |
| § 7 tests: core, service, HTTP, registry, SDK | 1-6 |
| § 8 choices left to the builder | 2, 4, 6 (planner's calls below) |

Planner's calls: `ResultShape` and `responses_result_shape` live in `turn.rs`. `rewrite_request` keeps `Option<Bytes>`. The error type is `Refused { refusal, entries }`. `Refusal::from_cause` maps the `_` arm to `UnreadableTurn`, and the event `cause` is the label. The unenforced `UnknownToolChoice` uses the `all` removal set, which leaves out the kept Anthropic definition. README rows outside § 4 say "not measured (SMA-776 measured only the Responses result types)".
