# SMA-757 No Partial Calls at a Client Raise: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The oracle accepts no strict-prefix call at an excused client raise, and the OpenAI stream interceptor rejects every measured record shape that lets a client hold such a call or misread a call.

**Architecture:** Four new canonical-form or malformed-record rules in `paigasus-gateway-core`'s OpenAI classifier, stream step and assembly. One model fix in the anthropic-python oracle model. The oracle prefix branch is deleted. Real OpenAI recordings become a no-deviation regression set.

**Tech Stack:** Rust (edition 2024, rust 1.95), serde_json, proptest, cargo nextest. Python 3 standard library for the fixture generator and the capture script.

**Spec:** `docs/superpowers/specs/2026-11-14-sma-757-oracle-argument-prefix-design.md`

## As built (differences from the tasks below)

The tasks below stay as the record of the original plan. The code and the spec hold the final rules.

- Fixture canaries are in the deviating record or in the allowed call's late fragment, not in a separate later `blocked_*` call. Observe mode forwards later records, so a separate canary leaks under `allow` (Tasks 3-6; spec § 7.3).
- `fragment_after_bodyless_entry`: the boundary is renewed at every bodyless record, because the rewrite can remove an earlier blocked bodyless call (spec § 5.1). A bodyless continuation fragment of an existing custom or Other call does not renew it.
- `chunk_without_choices`, `choice_without_delta` and `array_outside_tool_calls` apply in an open call of choice 0 and in the record that opens one, through one shared predicate (`in_call`, spec § 5.2, § 5.7). The finish entry of choice 0 may omit `delta`. The spec § 5.2 text does not state this exemption. The module doc of `src/openai/stream.rs` does.
- `"delta": null` stays `Malformed("delta")` (spec § 5.2).
- New rules from the final review: `array_outside_tool_calls` at any depth (spec § 5.7) and `Malformed("function_call_key")` on stream and body (spec § 5.8). Each has a generator quirk, a fixture and a row.
- A body reads `"type": null` as absent (spec D4: bodies have no fragments, so § 5.3 does not apply to them).
- The oracle names a bodyless kind-Other call `unknown`, as the core does (spec § 7.3, fixture `bodyless_type_null`).
- The recorded test runs every split point under allow-all and one push under each decider (spec § 7.3).
- The full `moon ci` graph ran after the final-review fix wave (Task 8 Step 5).

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python).
- Every canonical check runs on the copy (`Check`, phase 1) before anything changes. A deviation leaves no half-applied state.
- New deviation names, exactly: `fragment_after_bodyless_entry`, `chunk_without_choices`, `choice_without_delta`. New malformed names, exactly: `chunk_key`, `content_delta_type`. `"type": null` reuses `repeated_type` and `type_shape_mismatch`.
- Fixture names under `tests/fixtures/openai/malformed/`, exactly: `fragment_after_bodyless_entry`, `chunk_without_choices`, `choice_without_delta`, `type_null`, `chunk_key`, `content_delta_type`.
- Workspace lints deny warnings, test targets included. A mutation for a proof must compile.
- Revert a mutation by deleting the marked edit. Never use `git checkout --` on a file that holds the fix.
- Do not run `brew install` or install host software. Do not use `--no-verify`.
- Commit subjects are lower case after the scope: `fix(rs): … (SMA-757)`. Never begin a body line with `#NNN`.
- Run cargo from `rs/` with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Always pass `--locked`.
- Work in the worktree `/Users/smaschek/dev/paigasus/paigasus-core-sma-757`, branch `feature/sma-757-oracle-argument-prefix`. Run every command in the foreground.

Commands used by several tasks (from `rs/`):

- `TEST`: `cargo nextest run --locked -p paigasus-gateway-core`
- `LINT`: `cargo clippy --locked -p paigasus-gateway-core --all-targets -- -D warnings && cargo fmt --check`
- `GEN`: from `py/`: `uv run python -I ../rs/crates/libs/paigasus-gateway-core/tests/fixtures/make_fixtures.py ../rs/crates/libs/paigasus-gateway-core/tests/fixtures`, then `git status --short` shows only the intended fixture files.

## Review Focus

1. A choice entry with no `delta` on choice 0's **finish** record (`{"index":0,"finish_reason":"tool_calls"}`). Some upstreams omit `delta` there. No fragment can follow it, so it must stay conforming. Task 4 pins it.
2. A `custom` call before any function call, and a function call that starts in the same record as a `custom` entry. Neither is a prefix risk, so both must stay conforming. Task 3 pins both.
3. A text-only chunk without `choices`, or a choice without `delta`, while **no** call is open (for example before the first fragment, or the final usage chunk with `"choices":[]`). It must stay conforming. Task 4 pins it.
4. The non-stream body path calls `Check::apply` once. The bodyless rule must be a no-op there (a body with a function call and a custom call). Task 3 pins it.
5. `"type": null` on the **first** fragment with no body (a kind Other call). Clients run nothing, so it must stay kind Other, not a deviation. Task 5 pins it.

---

### Task 1: Real OpenAI recordings as a no-deviation regression set

**Files:**
- Add (already in the worktree, uncommitted): `rs/crates/libs/paigasus-gateway-core/tests/fixtures/openai/recorded/capture.py` and the 12 `*.sse` files beside it
- Create: `rs/crates/libs/paigasus-gateway-core/tests/fixtures/openai/recorded/README.md`
- Modify: `rs/crates/libs/paigasus-gateway-core/tests/fixtures.rs`

**Interfaces:**
- Produces: the fixture set `(Format::OpenAiChat, "openai/recorded")`, read by `files(rel)`.

- [ ] **Step 1: Write the README.** State: these are live recordings of 2026-11-14 (models, scenarios, `reasoning_effort: "none"` for `gpt-6.1-terra`), not hand-built. How to record again (the two `capture.py` command lines from its docstring, the key file outside the repo, never printed). They are not run in CI. Copy the measured results R1–R5 from spec § 2.4 as one short list.

- [ ] **Step 2: Write the test `recorded_streams_pass_unchanged`** in `tests/fixtures.rs`. For each file in `openai/recorded`, under `allow_all` and `limits(OnFailure::Block)`, for one push and for every single split point `i` in `0..=len`:
  - `o.finished`;
  - `o.metrics` holds no `Metric::RecordDropped(_)` and no `Metric::OnFailure(_)`;
  - `check_i1(...)` is `Ok`;
  - for every runner in `runners(Format::OpenAiChat)`, `run(&o.out).error.is_none()`, and the derived `(kind, name)` list equals the decided list mapped by `as_model_call` (the same order). The legacy recordings derive kind `legacy`. Arguments are compared by `check_i1` (LangChain's are parsed values, so a string comparison would be wrong).
  The recordings are over 4 KiB. Run every split point anyway: there are 12 files.
  Do NOT add `openai/recorded` to `SETS`: the `malformed_shapes_follow_section_6` row count reads `openai/malformed` only, and the canary tests assume hand-built names.

- [ ] **Step 3: Run `TEST`.** Expected: PASS, the new test included. If it fails, stop and report: the current interceptor rejects real traffic, which changes the spec.

- [ ] **Step 4: Commit.**
```bash
git add rs/crates/libs/paigasus-gateway-core/tests/fixtures/openai/recorded rs/crates/libs/paigasus-gateway-core/tests/fixtures.rs
git commit -m "test(rs): add real openai recordings as a no-deviation regression set (SMA-757)"
```

### Task 2: anthropic-python model drops an unfinished number, as jiter does

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/tests/support/oracle.rs` (`Partial::value`, the `_ =>` arm, about lines 1122-1133)
- Create: `rs/crates/libs/paigasus-gateway-core/tests/oracle.rs`

**Interfaces:**
- Consumes: `support::oracle::partial_json(&str) -> Result<Value, String>` (exists, `pub`).
- Produces: the test binary `tests/oracle.rs` (`mod support;`), which Task 7 extends.

- [ ] **Step 1: Write the failing test `partial_json_drops_an_unfinished_number`** in the new `tests/oracle.rs`:
```rust
assert_eq!(partial_json(r#"{"n":10."#).unwrap(), json!({}));
assert_eq!(partial_json(r#"{"n":1e"#).unwrap(), json!({}));
assert_eq!(partial_json(r#"{"n":-"#).unwrap(), json!({}));
assert_eq!(partial_json(r#"{"a":"x","n":10."#).unwrap(), json!({"a":"x"}));
assert_eq!(partial_json(r#"{"n":10"#).unwrap(), json!({"n":10}));
assert!(partial_json(r#"{"n":1x}"#).is_err()); // invalid, not at the end of input
```

- [ ] **Step 2: Run** `cargo nextest run --locked -p paigasus-gateway-core --test oracle`. Expected: FAIL on the first assertion (`ValueError: invalid token`). This is red 5's state; record the output.

- [ ] **Step 3: Implement.** In the `_ =>` arm, when the token does not parse, `self.eof()` holds, and every byte of the token is in `-+.0123456789eE`, return `Ok(None)` (the caller drops the member). Update the `Partial` doc comment: cite jiter 0.19.0 in partial mode, measured on 2026-11-14 (anthropic 2.3.0 requires `jiter<1,>=0.4.0`).

- [ ] **Step 4: Widen the Anthropic generator** (`tests/support/streams.rs`, spec § 7.1): tool inputs can hold a decimal member (for example `"n":10.5`), and the `partial_json` split points can fall inside that number. Keep it to the existing input builder; do not add a quirk (the input is conforming).

- [ ] **Step 5: Run `TEST`.** Expected: PASS, `anthropic_p1_and_p2_hold` included.

- [ ] **Step 6: Commit.** `test(rs): make the anthropic-python model drop an unfinished number like jiter (SMA-757)`

### Task 3: `fragment_after_bodyless_entry` (spec § 5.1)

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/src/openai/assembly.rs` (`Check`, `Check::apply`, module doc, `#[cfg(test)] mod tests`)
- Modify: `rs/crates/libs/paigasus-gateway-core/tests/fixtures/make_fixtures.py`, `tests/fixtures.rs` (one row)
- Generated: `tests/fixtures/openai/malformed/fragment_after_bodyless_entry.sse`

**Interfaces:**
- Produces: `Check.bodyless_at: Option<usize>`, the number of calls that existed before the first record with a bodyless entry was applied.

- [ ] **Step 1: Write the failing unit tests** in `assembly.rs`'s test module, each driving `ChoiceAssembly::check`/`commit` record by record:
  - `a_fragment_of_an_earlier_call_after_a_custom_entry_deviates`: record 1 function call 0 `{"v":"ab`; record 2 custom call 1; record 3 call 0 `cdefgh"}`. Record 3's `check` is `Err(Deviation("fragment_after_bodyless_entry"))`.
  - `an_earlier_call_in_the_bodyless_record_deviates`: record 1 call 0 start; record 2 holds call 0 `"cd"}` AND custom call 1. Record 2 is the deviation.
  - `a_bodyless_entry_with_only_an_index_counts`: record 2 is `{"index":1}` only. Then a fragment of call 0 deviates.
  - `custom_first_then_function_is_conforming` (Review Focus 2): custom call 0, then function call 1 in later records, then more fragments of call 1. All `Ok`.
  - `a_function_call_that_starts_in_the_bodyless_record_is_conforming` (Review Focus 2): record 1 holds custom call 0 and function call 1 start; record 2 continues call 1 only. All `Ok`. (Call 1 did not exist before record 1.)
  - `contiguous_function_then_custom_is_conforming`: call 0 complete in records 1-2, custom call 1 in records 3-4. All `Ok`.
  - `the_bodyless_rule_is_a_no_op_for_a_body` (Review Focus 4): one `apply` with a function call 0 and a custom call 1 is `Ok`. Also add a body fixture-free test in `src/openai/body.rs` tests if that module has the body parse helper: a `chat.completion` body with a function and a custom call parses with two calls.

- [ ] **Step 2: Run `TEST`.** Expected: the three deviation tests FAIL, the conforming ones PASS.

- [ ] **Step 3: Implement** the rule in `Check::apply`, before the per-fragment loop. A fragment is bodyless when `f.function.is_none()`. Let `before = self.calls.len()`. If `bodyless_at` is `Some(n)`, any fragment with `index < n` is the deviation. Else, if any fragment in `frags` is bodyless: any fragment with `index < before` is the deviation; then set `bodyless_at = Some(before)`. Add the rule and its reason to the module doc: LangChain raises at the first record with a bodyless entry (`rtc["function"].get`) and drops that record, so a later fragment of an earlier call makes its snapshot a strict prefix (spec § 2.1 M3). Say that a fragment that adds nothing is also a deviation, on purpose.

- [ ] **Step 4: Add the fixture** in `make_fixtures.py` (`OPENAI_MALFORMED`): an allowed `read_file` call 0 with `{"v":"abcdefgh"}` split as `{"v":"ab` and `cdefgh"}`, a custom call 1 `note` between them, and a separate `blocked_rm` call 2 with `CANARY-bodyless` after them. Run `GEN`. Add the row `row("fragment_after_bodyless_entry", &["(malformed)"], &[], &[PASS_THROUGH], &[], true)`.

- [ ] **Step 5: Run `TEST` and `LINT`.** Expected: PASS.

- [ ] **Step 6: Commit.** `fix(rs): reject a fragment of an earlier call after a bodyless tool-call entry (SMA-757)`

### Task 4: `chunk_without_choices` and `choice_without_delta` (spec § 5.2)

**Files:**
- Modify: `src/openai/classify.rs` (`ChoiceView` gains `pub has_delta: bool`; `Chunk::Choices` carries whether the `choices` key was present: change the variant to `Chunk::Choices { choices: Vec<ChoiceView>, keyed: bool }` and update every match)
- Modify: `src/openai/stream.rs` (`step`, phase 1)
- Modify: `tests/support/streams.rs` (two quirks), `make_fixtures.py`, `tests/fixtures.rs` (two rows)

**Interfaces:**
- Produces: `ChoiceView.has_delta`, `Chunk::Choices { choices, keyed }`.

"Open call" = `self.choices.get(&0)` is `Some(s)` with `s.hold_open && !s.finished`.

- [ ] **Step 1: Write the failing unit tests** in `stream.rs`'s test module (use its `run`/`chunk`/`rec` helpers):
  - `a_chunk_without_choices_inside_an_open_call_deviates`: call 0 start, then `{"object":"chat.completion.chunk","id":"c"}`. Assert `Deviation("chunk_without_choices")` (through the step result).
  - `a_choice_without_delta_inside_an_open_call_deviates`: call 0 start, then `choices:[{"index":0}]`. Assert `Deviation("choice_without_delta")`.
  - `a_choice_without_delta_at_position_1_also_deviates`: `choices:[{"index":0,"delta":{}},{"index":1}]` inside an open call.
  - `a_finish_record_without_delta_is_conforming` (Review Focus 1): call 0 complete, then `{"index":0,"finish_reason":"tool_calls"}`. `Ok`, and the call completes.
  - `no_choices_or_no_delta_with_no_open_call_is_conforming` (Review Focus 3): before any fragment, a chunk without `choices` and a choice without `delta`; after finish, `"choices":[]` with `usage`. All `Ok`.

- [ ] **Step 2: Run `TEST`.** Expected: the three deviation tests FAIL.

- [ ] **Step 3: Implement.** In phase 1 of `step`, when choice 0 has an open call: a chunk with `keyed == false` is `Deviation("chunk_without_choices")`; a choice entry with `has_delta == false` is `Deviation("choice_without_delta")`, except the entry for choice 0 that carries a `finish_reason`. Explain both in the `stream.rs` module doc with the raising model (openai-python `chunk.choices`/`choice.delta`, openai-node `chunk.choices`, LangChain `KeyError: 'delta'`).

- [ ] **Step 4: Add the generator quirks** `Quirk::NoChoicesKey` and `Quirk::NoDelta` to `streams.rs` (enum with doc lines, the strategy list, and the insertion `match`). Insert at `pos`: `{"object":"chat.completion.chunk","id":"q"}` and `chunk(json!([{"index":0}]))`. Their insertion point is random, so some cases land inside an open call.

- [ ] **Step 5: Add the two fixtures** (same layout as Task 3: allowed `read_file` split around the rule's record, a separate `blocked_rm` canary). Run `GEN`. Add two rows like Task 3's.

- [ ] **Step 6: Run `TEST` and `LINT`.** Expected: PASS.

- [ ] **Step 7: Commit.** `fix(rs): reject a chunk without choices or a choice without delta inside an open call (SMA-757)`

### Task 5: a present `type` key counts as the type fragment (spec § 5.3)

**Files:**
- Modify: `src/openai/classify.rs` (`Fragment` gains `pub ty_key: bool`, true when the `type` key is present, `null` included)
- Modify: `src/openai/assembly.rs` (`Check::apply`: the `type` branch reads `ty_key`; `null` maps to `TyClass::Other`)
- Modify: `tests/support/streams.rs` (one quirk), `make_fixtures.py`, `tests/fixtures.rs` (one row)

- [ ] **Step 1: Write the failing unit tests** in `assembly.rs`:
  - `a_later_null_type_is_a_repeated_type`: fragment 1 `type:"function"` with a body, fragment 2 `{"index":0,"type":null}`. Fragment 2 is `Err(Deviation("repeated_type"))`.
  - `a_first_null_type_with_a_function_body_mismatches`: `type:null` plus a `function` body; `finish()` is `Err(Deviation("type_shape_mismatch"))`.
  - `a_null_type_with_no_body_stays_kind_other` (Review Focus 5): one fragment `{"index":0,"id":"x","type":null}`; `finish()` is `Ok(vec![ToolCallKind::Other])`.

- [ ] **Step 2: Run `TEST`.** Expected: the first two FAIL.

- [ ] **Step 3: Implement.** Set `ty_key` in `fragment_at`. In `Check::apply`, take the `type` branch when `f.ty_key`. Then run `TEST` over the whole crate: existing fixtures and generators must stay green. If a conforming fixture or generator emits `"type": null`, stop and report it.

- [ ] **Step 4: Add `Quirk::NullType`** (insert `chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"type":null}]}}]))`), and the fixture `type_null` (allowed `read_file` split around the null-type fragment, a separate canary call). Run `GEN`. Add the row.

- [ ] **Step 5: Run `TEST` and `LINT`.** Expected: PASS.

- [ ] **Step 6: Commit.** `fix(rs): count a null tool-call type as the type fragment (SMA-757)`

### Task 6: H1 `chunk_key` and H2 `content_delta_type` (spec § 5.4, § 5.5)

**Files:**
- Modify: `src/openai/classify.rs` (`chunk`), `src/json.rs` (`CLASSIFIER_KEYS` gains `"chunk"`, in sorted order)
- Modify: `tests/support/streams.rs` (two quirks), `make_fixtures.py`, `tests/fixtures.rs` (two rows)

- [ ] **Step 1: Write the failing unit tests** in `classify.rs`'s test module:
  - `a_top_level_chunk_key_is_malformed`: `{"choices":[],"chunk":{"choices":[…]}}` gives `Err(Malformed("chunk_key"))`. Also with `"chunk":{}`.
  - `a_case_folded_chunk_key_is_rejected`: `"Chunk"` gives `case_folded_key` through `parse_strict`.
  - `content_delta_type_with_tool_data_is_malformed`: top-level `"type":"content.delta"` with a `tool_calls` fragment gives `Err(Malformed("content_delta_type"))`; with only `finish_reason`, the same.
  - `content_delta_type_with_text_only_is_not_checked`: the same `type` with only `content` text is `Ok`.

- [ ] **Step 2: Run `TEST`.** Expected: the malformed tests FAIL.

- [ ] **Step 3: Implement** both checks at the start of `chunk` (after the `error` branch), using the existing `carries` value for H2. Module doc: LangChain reads `chunk.choices` when `choices` is empty, and skips a record whose top-level `type` is `content.delta` (`_convert_chunk_to_generation_chunk`). Cite spec § 2.2 H1, H2.

- [ ] **Step 4: Add `Quirk::ChunkKey`** (a `blocked_chunk` call with `CANARY-chunk` inside `chunk.choices`, top-level `"choices":[]`) and `Quirk::ContentDeltaType` (a fragment `{"index":0,"function":{"arguments":"zz"}}` in a record with `"type":"content.delta"`). Add the fixtures `chunk_key` (the `blocked_*` call inside `chunk`, plus an allowed split call) and `content_delta_type`. Run `GEN`. Add both rows. Check with the existing canary test that `CANARY-chunk` never reaches the output under block.

- [ ] **Step 5: Run `TEST` and `LINT`.** Expected: PASS.

- [ ] **Step 6: Commit.** `fix(rs): reject the chunk key and content.delta records that langchain reads differently (SMA-757)`

### Task 7: the oracle accepts no strict partial (spec § 4, § 7.2)

**Files:**
- Modify: `tests/support/oracle.rs` (`json_prefix` deleted; `arguments_match` loses `partial`; doc comments of the module header, `check_i1`, `arguments_match`)
- Modify: `tests/oracle.rs` (Task 2's file)

**Interfaces:**
- Consumes: `check_i1(format, input, out, decided, core_failed)` (signature unchanged); `paigasus_gateway_core::parse_body(Format, &[u8], usize) -> Result<BodyCalls, Malformed>` with `pub calls: Vec<ToolCall>`.

- [ ] **Step 1: Write the property test `a_prefix_at_an_excused_raise_is_rejected`** in `tests/oracle.rs` (proptest, 1 000 cases). Strategy: a lower-case string `s` of 4 to 16 characters, a cut `k` in `1..len(args)` where `args = {"v":"<s>"}`, and a raise shape in {P-M3, P-a, P-b, P-c} (spec § 2.2). Build the stream: role chunk; call 0 `read_file` with `args[..k]`; the raise record; call 0 with `args[k..]`; a finish chunk; `[DONE]`. For P-M3 the raise record is a custom call 1 `note` with input `hi`. Build `decided` from `parse_body(Format::OpenAiChat, <a chat.completion body with the same calls>, 1 << 20)`, all `Decision::Allow`. Call `check_i1(Format::OpenAiChat, &bytes, &bytes, &decided, false)`. Assert `Err(e)` where `e` contains `"undecided or blocked call"` and the model that raises first in `runners` order: `langchain` for P-M3, `openai-python` for P-a, P-b and P-c.
  Second test `a_raise_after_the_last_fragment_is_excused`: the same stream with the raise record after the second fragment. Assert `Ok(())`.

- [ ] **Step 2: Run** `cargo nextest run --locked -p paigasus-gateway-core --test oracle`. Expected: the first test FAILS (the current oracle returns `Ok`). This is red 1. Record the output.

- [ ] **Step 3: Implement § 4.** Delete `json_prefix` and the `partial` parameter of `arguments_match`. `check_i1` keeps `partial` only to excuse the raise. Doc comments: a call in a snapshot at an excused raise must match an allowed call exactly; a prefix is never accepted, because a prefix of allowed arguments can be a call that an argument-reading policy (SMA-752) denies, and the gateway never asks the decider about it.

- [ ] **Step 4: Run `TEST`** three times (three random seeds for the 10 000-case properties). Expected: PASS each time. A red here is an unclosed raise path: stop and report the shrunk case. Do not loosen the oracle.

- [ ] **Step 5: Run `LINT`.** Expected: PASS.

- [ ] **Step 6: Commit.** `test(rs): accept no strict-prefix call at an excused client raise (SMA-757)`

### Task 8: proof that each half bites, and the full gate

**Files:** none committed, except the record in the PR body.

Record, for each mutation: the edit, the command, and the failing test names from the output. Each mutation must compile. Revert each by deleting the marked edit, then run `TEST` green before the next one.

- [ ] **Step 1: Red 2 and red 4.** Delete each § 5 rule in turn (Tasks 3, 4 (two rules, one at a time), 5, 6 (two rules, one at a time)). For each, run `cargo nextest run --locked -p paigasus-gateway-core --test fixtures`. Expected: that rule's fixture reds in `every_fixture_holds_i1_for_every_client` under `allow-all` and `block-by-name` (by fixture name), and in `malformed_shapes_follow_section_6` (the `stream_failure` column). Under `block-all` the allowed call is blocked and rewritten out, so I1 cannot red there; that is expected. Then delete all six rules at once and run the same command: every new fixture reds (this is red 2: the new oracle catches the gap without the interceptor rules).
- [ ] **Step 2: Red 3.** Restore the prefix acceptance in `arguments_match` (the old `starts_with` and `json_prefix` branches). Run `--test oracle`. Expected: `a_prefix_at_an_excused_raise_is_rejected` reds.
- [ ] **Step 3: Red 5.** Restore the raising `_ =>` arm in `Partial::value`. Run `--test oracle`. Expected: `partial_json_drops_an_unfinished_number` reds.
- [ ] **Step 4: Full crate gates.** Run `TEST`, `LINT`, then `GEN` and confirm `git status --short` is empty.
- [ ] **Step 5: Full graph** (CLAUDE.md "Before you push"). Run the root CLAUDE.md `moon ci … --base origin/main --include-relations` command with the shims on `PATH`. Read any red with the CLAUDE.md diagnosis procedure before you re-run it. A gate red that comes from this Mac's bash split (`mapfile`, `declare -A`, `local -n`, the 512-byte pipe) is not a finding: re-run that gate with the right bash as CLAUDE.md says and record both results.
