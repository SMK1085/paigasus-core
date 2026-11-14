# SMA-757: no partial calls at a client raise, in the oracle and in the interceptor

Linear: SMA-757 (parent SMA-747). Related: SMA-749 (the oracle and the interceptor), SMA-752
(argument-reading policies). This change must land before SMA-752 or as part of it.

## 1. Intent

The differential oracle of SMA-749 (`rs/crates/libs/paigasus-gateway-core/tests/support/oracle.rs`)
checks invariant I1: every call that a client model derives from the gateway's output was decided
`Allow`, with the same choice, kind, name and arguments that the decider saw (SMA-749 spec § 2).
When a client model raises partway through a stream, and the raise is excused (`core_failed`, or
the client also raises on the upstream bytes), the oracle accepts a call whose arguments are only a
**prefix** of the decided arguments (`json_prefix` for parsed values, `starts_with` for raw
strings).

That tolerance is sound only while deciders read the tool name alone. SMA-752 adds policies that
read arguments. A prefix of allowed arguments can then be a call that the policy denies. An
example: a policy allows `read_file` only for a `path` under `/workspace/`. The decider sees
`{"path":"/workspace/a"}` and allows it. A client that holds the prefix `{"path":"/wor"}` holds a
call that the policy would block. The oracle must catch this, and the gateway must not produce it.

## 2. Measured current state (2026-11-14, `origin/main` 2d58afd6)

### 2.1 The generated suite

The oracle was instrumented temporarily, and `tests/properties.rs` and `tests/fixtures.rs` were run
once (10 000 cases per format for P1).

- M1. The strict-prefix branch (a partial result that is a prefix but not equal) fired **9 times**.
  All 9 were the LangChain model, on OpenAI streams, through the parsed (`json_prefix`) branch.
  Examples: `{"v":"pihn"}` of `{"v":"pihnarwopuv"}`, `{}` of `{"v":"vgfpua"}`.
- M2. The raw `starts_with` branch and the anthropic-python model fired **0 times** in the
  generated suite. § 2.2 shows that this comes from the generators, not from the interceptor.
- M3. All 9 streams have one shape. The interceptor forwards the upstream records verbatim after
  the decision. A function call's argument fragments are interleaved with a later tool-call entry
  that has no `function` body (a `custom` call). LangChain reads `rtc["function"].get(...)` and
  raises `AttributeError` at that record. The upstream bytes raise the same way, so the raise is
  excused. `core_failed` was `false` in all 9.

### 2.2 Hand-built probes (one allowed call, `{"v":"ab` + one record + `cdefgh"}`, `allow-all`)

The adversarial review found more raise paths. Each probe was driven through the interceptor and
every OpenAI model. In each case `core_failed` was `false` and the upstream bytes raise too.

| Id | Record between the two fragments | Models that hold a strict prefix |
|----|----------------------------------|----------------------------------|
| P-M3 | a `custom` tool-call entry | LangChain (`{"v":"ab"}`) |
| P-a | a chunk with no `choices` key | openai-python, openai-node (raw `{"v":"ab`) |
| P-b | a choice entry with no `delta` key | openai-python (raw), LangChain (parsed) |
| P-c | a fragment `{"index":0,"type":null}` of the same call | openai-python (raw), LangChain (parsed) |
| P-e | two records: an unknown delta key with an indexed list, then a scalar list (`{"foo":[{"index":0}]}`, then `{"foo":[1]}`). The same under a tool-call entry key (`"x":[…]`) or a `function` key. | openai-python (raw `{"v":"ab`) |
| P-e′ | as P-e, but the indexed list is in a text record before the call, and the scalar list is in the record that opens the call | openai-python (raw `{"v":"ab`) |
| P-f | an earlier text record, then one record with the fragment that opens the call at position 0 and a choice entry `{"index":1}` with no `delta` at position 1 | openai-python (raw `{"v":"ab`) |
| P-d | a legacy `function_call` whose extra key changes JSON type | LangChain raises. It merges `arguments` before the extra key, so it holds the full call **only when the type change is in the last fragment**. When the change is in a middle fragment and more arguments follow, LangChain holds a strict prefix (raw `{"v":"abcd`, final review probe G2). |

P-d was first recorded with the type change in the last fragment only, so its first result
("none") was wrong for the general case. P-e, P-e′ and P-f were measured by the final branch review (2026-11-14, at 29784b6a), after the
rules of § 5.1 to § 5.5 were in place. openai-python raises in `accumulate_delta` (P-e) or at
`choice.delta` (P-f) and keeps what it merged before the raise. P-f was missed at first because
the "open call" state was read before the record that opens the call (§ 5.2).

Two related I1 holes have the same root cause (a record that the interceptor reads differently
from LangChain). They are not prefixes:

| Id | Record | Effect |
|----|--------|--------|
| H1 | `"choices":[]` and a top-level `chunk` object that holds `choices` with a tool call | LangChain reads `chunk.choices` when `choices` is empty. The interceptor does not read `chunk`. LangChain runs a call that **no decider saw**, under every decider. |
| H2 | a record with top-level `"type":"content.delta"` that carries an argument fragment | LangChain skips the record. The interceptor applies it. LangChain runs `{"v":"abcdefgh"}`, the decider saw `{"v":"abzzcdefgh"}`. |

### 2.3 Anthropic

The anthropic-python model raises on a buffer that ends inside a number (`{"n":10.`, `1e`, `-`).
Measured with jiter 0.19.0 (anthropic 2.3.0 requires `jiter<1,>=0.4.0`) in partial mode: jiter does
**not** raise. It drops the unfinished member (`{"n":10.` gives `{}`, `{"a":"x","n":10.` gives
`{"a":"x"}`). So there is no Anthropic gap, but the model is too strict. A false raise can make the
oracle excuse a snapshot that the real SDK never produces.

### 2.4 Real OpenAI streams (recorded 2026-11-14)

`tests/fixtures/openai/recorded/capture.py` recorded live Chat Completions streams with
`stream_options.include_usage`. It used three model families, five runs for each scenario, so
60 streams in total:

- `gpt-4.1`: parallel function calls and legacy `functions`. `custom` tools are rejected
  (`Invalid value: 'custom'`).
- `gpt-5.5`: parallel function calls, `custom` tools with function tools, a forced `custom` tool,
  and legacy `functions`.
- `gpt-6.1-terra`, with `reasoning_effort: "none"`: the same scenarios. On `/v1/chat/completions`,
  a `gpt-6` model accepts function tools only with `reasoning_effort: "none"`. Without it the API
  answers "Function tools with reasoning_effort are not supported … use /v1/responses". The models
  `gpt-6.1-sol` and `gpt-6-astra` do not accept `none`, so they cannot stream tool calls on Chat
  Completions at all. `gpt-6-sol` and `gpt-6-luna` accept `none`.

Results:

- R1. **No stream interleaves calls.** In all 60 streams, the fragments of each call are in
  contiguous records, and no other record is between two fragments of one call.
- R2. No stream holds a chunk without `choices`, a choice entry without `delta`, a
  `"type": null`, a top-level `chunk` or `type` key, or a choice other than 0.
- R3. The only chunk with empty `choices` is the final usage chunk (60 of 60).
- R4. A `custom` call streams as `{"index","id","type":"custom","custom":{…}}` and then
  `{"index","custom":{"input":…}}` entries. These are bodyless entries in the sense of § 5.1. They
  were never next to a function call.
- R5. Not measured: a `custom` call and a function call in one response. In 10 tries the models
  never did it, even when the prompt asked for both.
- Every chunk has an `obfuscation` string field, which pads the chunks to a uniform length. So
  repeated runs often have the same byte size. The completion and call ids differ in every run.

Not covered: Azure OpenAI (for example its content-filter chunks), and self-hosted upstreams
(SMA-749 T2).

One recording for each model and scenario (12 files, 88 KiB) is kept as a fixture next to the
script.

## 3. Decisions

| Id | Decision |
|----|----------|
| D1 | Scope: the oracle **and** the interceptor (Sven, 2026-11-14). An oracle-only change cannot pass honestly (§ 2). |
| D2 | Oracle rule: **reject every strict partial.** At an excused raise, a call in the client's snapshot must match an allowed call exactly (the non-partial rules, including `""` against `{}`). A re-decide of the prefix was the first draft. It was dropped: it proves I1 only for the test deciders, and the gateway never asks the production decider about a prefix. This rule is decider-free, and it is what I1 says. Changed after the review. Confirmed by Sven at Gate 1 (2026-11-14). |
| D3 | Interceptor rule: each record shape in § 5 becomes a canonical-form deviation (a stream-level `on_failure`, as every § 4.3 deviation of SMA-749 is), or a malformed record. The rules name shapes, not one SDK. |
| D4 | Anthropic: no interceptor change (§ 2.3). The anthropic-python model's partial parser is fixed to match jiter (§ 6.3). Bodies: no change (a body has no fragments), with one exception: § 5.8 applies to a body `function_call` too, because the stream and the body share one parser for it and real OpenAI sends no such key in either. |
| D5 | The generators render every shape of § 2.2, so P1 covers each one with random cuts and deciders (§ 7.1). |
| D6 | H1 and H2 are fixed in this change, not in a later sweep. Confirmed by Sven at Gate 1 (2026-11-14). |
| D7 | Real OpenAI streams are recorded and kept as fixtures (§ 2.4), at Sven's request. Every recording must pass with no deviation (§ 7.3). |

## 4. The oracle change

In `arguments_match`, the `partial` flag no longer allows a prefix. A snapshot at an excused raise
is checked with the same rules as a complete run. The raise only excuses the fact that the client
raised, and that it may hold fewer calls. Concretely:

- `json_prefix` is deleted.
- The `partial` branch of `arguments_match` (`starts_with`, and `json_prefix` for parsed values) is
  deleted, so `arguments_match` loses the `partial` parameter.
- `check_i1` keeps the `partial` state only to excuse the raise. The doc comments of `check_i1`,
  `arguments_match` and the module header must say that a partial call is never accepted, and why
  (SMA-752).
- Matching stays first-fit over candidates, and it is exact. Two allowed calls with the same choice,
  kind and name but other arguments cannot be confused, because only an exact match is accepted.

Kind `other` keeps its current rule: arguments are not compared, because the decided form is the
raw entry. This is a known limit for SMA-752 policies that read the arguments of a kind Other call
(§ 9).

## 5. The interceptor changes (OpenAI stream)

"Open call" means a call of choice 0 whose first fragment has arrived, before the choice's
`finish_reason`. All checks run on the copy before anything is changed, as every canonical check
does, so a deviation leaves no half-applied state.

### 5.1 `fragment_after_bodyless_entry` (P-M3)

A tool-call entry is **bodyless** when it has no `function` object (`Fragment.function` is `None`):
a `custom` entry, an entry with only `index`, `id` or `type`, or `"function": null`. LangChain
raises at the first record that holds a bodyless entry in its choice, and it drops that whole
record. So, once a record holds a bodyless entry, that record and every later record must hold no
fragment for a call that existed **before that record was applied**. Such a fragment is
`Deviation("fragment_after_bodyless_entry")`.

- A call that starts in the bodyless record or later is never seen by LangChain. LangChain then
  runs fewer calls, which is fail-closed.
- `Check` stores the number of calls that existed before the first bodyless record was applied
  (`Option<usize>`, `None` until then).
- The rule is slightly too broad, on purpose: a later fragment that adds nothing
  (`"arguments":""`, `null`, `"function":{}`) is also a deviation. Exempting it adds a case for no
  practical gain.

### 5.2 `choice_without_delta` and `chunk_without_choices` (P-a, P-b, P-f)

While choice 0 has an open call, **and in the record that opens a call of choice 0** (the record
carries tool data for choice 0):

- a chunk with no `choices` key is `Deviation("chunk_without_choices")`;
- a choice entry with no `delta` key, in any position, is `Deviation("choice_without_delta")`.

`"delta": null` stays `Malformed("delta")` in every position (SMA-749 classifier); inside an open
call the queue is non-empty, so it is already a stream-level failure.

Outside an open call these records stay as they are today (classified as tool-free). This limits
the rule to the place where a raise can cut a call.

The record that opens a call counts (P-f): a raise later in that record cuts the call after its
first fragment. One predicate in `stream.rs` (`in_call`: an open call, or this record carries tool
data for choice 0) decides this for § 5.2 and § 5.7, so the two rules cannot drift.

### 5.3 A present `type` key counts as the type fragment (P-c)

`"type": null` is read as "absent" today (`opt_str`, `json.rs`), so a later `"type": null` passes
`repeated_type`. In a tool-call fragment, a present `type` key, `null` included, counts as the type
fragment. A second one is `repeated_type`. A first `"type": null` with a `function` body is
`type_shape_mismatch` at completion. The builder must check that the existing `type`-absent
streams in the fixtures and generators stay conforming.

### 5.4 H1: a top-level `chunk` key

A chunk that has a top-level `chunk` key is `Malformed("chunk_key")` (the record is dropped, with
the same § 6 behaviour of SMA-749 as other malformed records). No real OpenAI-compatible upstream
sends it. The key is added to the classifier key list in `json.rs`, so case-folded forms are
rejected the same way as other classifier keys.

### 5.5 H2: a top-level `type` of `content.delta`

A chunk whose top-level `type` is the string `content.delta` and that carries tool data
(`tool_calls`, `function_call`) or a `finish_reason` is `Malformed("content_delta_type")`.
LangChain skips such a record, so the decider and LangChain would see different calls.

### 5.6 Common consequences

- Real OpenAI did not send any of these shapes in 60 recorded streams (§ 2.4, R1 and R2). So no
  rule fires on that traffic. The recorded fixtures pin this (§ 7.3). A self-hosted or Azure
  upstream that sends one of these shapes gets `on_failure`. The upstream bytes already make at
  least one supported client raise or misread in every case.
- **`"type": null` on every continuation fragment.** An upstream that sends `"type": null` on
  each later fragment of a call (for example a server that dumps pydantic models without
  `exclude_none`) now gets `repeated_type`, so `on_failure` runs on every tool stream from it. The
  rule is necessary: openai-python replaces `type` on such a fragment and its
  `new_tool.type == "function"` assert raises (§ 5.3, P-c). How common this upstream shape is was
  not measured. Real OpenAI does not send it (§ 2.4, R2).
- **Observe mode trade-off.** Under `on_failure = allow`, the deviating record is not emitted and
  later records are forwarded unclassified (SMA-749 § 5.3). A stream that was enforced before (for
  example an interleaved function and custom call) now reaches openai-python and openai-node in
  pass-through, without the deviating record. For P-M3 that record holds the `custom` entry and any
  fragments in it. Observe mode is not a security mode, and every SMA-749 deviation behaves this
  way. This spec does not change that behaviour.
- Each deviation is reported as `RecordDropped("malformed")` and `OnFailure("malformed")`, as the
  existing deviations are. An operator cannot tell the new deviations apart. This is unchanged.
- The rules and their reasons go into the module docs: `src/openai/assembly.rs` for § 5.1 and
  § 5.3, `src/openai/stream.rs` for § 5.2 and § 5.7, and `src/openai/classify.rs` for § 5.4,
  § 5.5 and § 5.8. The SMA-749 spec is not edited; this spec
  supersedes its § 4.3 list for these shapes.

### 5.7 `array_outside_tool_calls` (P-e)

Inside a call of choice 0 (the `in_call` predicate of § 5.2), a delta that holds an array outside
the `tool_calls` array itself is `Deviation("array_outside_tool_calls")`. The array can be the
value of any delta key other than `tool_calls`, or of any key of a tool-call entry, its `function`
and `custom` objects included, or of any key of a legacy `function_call`. It counts at any depth
below those keys. The rule applies to the choice entries in every position of the record, as
§ 5.2 does: a raise in any choice stops openai-python for the whole stream.

- openai-python's `accumulate_delta` merges a list by `index` when the earlier list holds
  non-scalar entries. A later list whose entry is not a dict raises `TypeError`, so the client
  keeps a strict prefix of the call. The rule names the shape (an array where no real upstream
  sends one), not the exact raise condition.
- The rule also covers an array nested in an object (`{"foo":{"bar":[…]}}`), because
  `accumulate_delta` merges dicts recursively. The review named only the first level. A kind
  Other entry with an array in its raw body is a deviation too: that is fail-closed.
- Choice-level keys (for example `logprobs`) are not merged as a delta. They are not checked.
- The recorded streams (§ 2.4) and the conforming fixtures hold no array outside `tool_calls`.
  The recorded delta keys are `content`, `function_call`, `refusal`, `role` and `tool_calls`.
- Outside a call the shape stays conforming: a raise there cuts no call.

### 5.8 `function_call_key` (P-d)

A legacy `function_call` object with a key other than `name` and `arguments` is
`Malformed("function_call_key")`. This applies in every stream record and in a non-stream body:
`classify::legacy_call` reads the object for both.

- LangChain merges the extra key into `additional_kwargs["function_call"]`. When a later fragment
  gives it another JSON type, `merge_dicts` raises `TypeError`. Its snapshot then holds the
  arguments merged so far, which is a strict prefix when more arguments follow.
- The rule is malformed, not a deviation, and it does not depend on an open call: no real OpenAI
  stream or body sends another key (§ 2.4: the recorded `function_call` keys are `name` and
  `arguments`). A body with such a key is `Malformed`, which changes the body path (D4).
- In a stream, the record is dropped. Inside an open hold this is a stream-level failure, as for
  every malformed record (SMA-749 § 6).

## 6. Model changes in the oracle

### 6.1 None for the raise paths

The OpenAI models already raise as the real SDKs do on P-a, P-b, P-c and P-M3 (cited sources in
`oracle.rs`). They are not changed.

### 6.2 LangChain H1 and H2

The LangChain model already reads `chunk.choices` and skips `content.delta` records. It is not
changed.

### 6.3 anthropic-python partial parse

`Partial::value` in `oracle.rs` must drop an unfinished number or literal at the end of the input,
with its key, and not raise, as jiter does (§ 2.3). The doc comment must cite the jiter version
measured. A unit test pins the three measured inputs: `{"n":10.` gives `{}`, `{"n":1e` gives `{}`,
`{"a":"x","n":10.` gives `{"a":"x"}`.

## 7. Tests

### 7.1 Generator quirks

`tests/support/streams.rs` gets one OpenAI `Quirk` for each shape of § 2.2 and § 5: a bodyless
entry interleaved inside an open call (it exists), a chunk without `choices`, a choice entry
without `delta`, a `"type": null` fragment, a top-level `chunk` key with a tool call, a
`content.delta` record with a fragment, two records with an array outside `tool_calls`
(§ 5.7), and two legacy fragments with an extra key that changes type (§ 5.8). The quirk for a choice entry without `delta` also adds `{"index":1}` behind the
record that opens a call (P-f). Each quirk must be able to land between two fragments of
an allowed call. The generators must also emit an Anthropic input with a decimal number, split
inside the number.

### 7.2 A property test of the oracle (the issue's "fails on the current tolerance")

A new proptest drives `check_i1` directly, with no interceptor, on a generated OpenAI stream. It has
an allowed function call 0 whose arguments are split into two fragments, and a raise record from
§ 2.2 (P-M3, P-a, P-b or P-c, chosen by the strategy) between them. `decided` holds the calls, all
`Allow`. The output is the upstream bytes, so the raise is excused and the partial path runs. The
test asserts that `check_i1` returns `Err`, and that the message names the model and call 0.

- On the current oracle it fails: the prefix is accepted.
- After § 4 it passes.

A second case puts the raise record after the last fragment of call 0. The test asserts `Ok`, so
the oracle does not simply reject every excused raise.

`ToolCall` is `#[non_exhaustive]`, so the test builds the decided calls from `parse_body`
(`BodyCalls.calls`) and changes fields on a clone, or with a test-support constructor if one is
needed. This test lives in one test file (for example `tests/oracle.rs`), not in `support/`,
because `support/` is compiled into every test binary.

### 7.3 Fixtures

One fixture per new rule under `tests/fixtures/openai/malformed/`, generated by
`tests/fixtures/make_fixtures.py`, each with a row in `malformed_shapes_follow_section_6`:
`fragment_after_bodyless_entry`, `chunk_without_choices`, `choice_without_delta`, `type_null`,
`chunk_key`, `content_delta_type`, `array_outside_tool_calls` (§ 5.7),
`choice_without_delta_in_opening_record` (§ 5.2, P-f) and `function_call_key` (§ 5.8). The
conforming fixture `bodyless_type_null` pins a bare `{"index":0,"type":null}` call: kind Other,
named `unknown` by the core and by the oracle.

Each fixture has an **allowed** call (`read_file` with `{"v":"abcdefgh"}`, a function call or a
legacy call for § 5.8) whose fragments are split around the rule's record. The canary is **in
the deviating record itself** (for example a `"note":"CANARY-…"` key), not in a separate
`blocked_*` call: a separate later call is forwarded in observe mode after the deviation, so its
canary would leak there. Then `every_fixture_holds_i1_for_every_client` reds for that fixture
under every decider when its rule is removed (§ 7.4). The H1 fixture puts a `blocked_*` call
inside `chunk`.

The recorded streams of § 2.4 are kept in `openai/recorded`. They are **not** in the fixture sets
of `tests/fixtures.rs` (`SETS`), because the canary and row tests assume the hand-built names.
`recorded_streams_pass_unchanged` replays them with `on_failure = block`:

- One push per file under each of the three deciders (`allow-all`, `block-all`,
  `block-by-name`): the stream ends, there is no `RecordDropped` or `OnFailure` metric, and I1
  holds (`check_i1`). Under `allow-all` the output is also payload-identical to the input, and
  every OpenAI model derives the decided calls from it (the same kinds and names, in order; I1
  compares the arguments).
- Every single split point of every file (one cut), under `allow-all`: the stream ends, the
  metrics are clean, and the output is byte-identical to the one-push output. So it is
  payload-identical to the input, and the client runs of the one-push pass hold for it. The
  files run in parallel threads (about 74 000 drives).

So a future rule that rejects real OpenAI traffic reds at once. `capture.py` stays next to the
recordings, so they can be recorded again. It reads the API key from a file and never prints it.
It is not run in CI.

### 7.4 Proof that each half bites

The plan must record each of these as a measured red. Each mutation must compile (`-D warnings`
applies to test targets) and is reverted by deleting the marked edit, not by `git checkout`.

1. The § 7.2 test, written first, against the current oracle. (Red 1 is the same state as
   mutation 3 below: the prefix branch exists. Measure it once, before § 4.)
2. `every_fixture_holds_i1_for_every_client` with § 4 and the § 7.3 fixtures, before § 5. Every
   new fixture must red, by name. This is deterministic, unlike a random P1 run.
3. After all changes: restore the prefix acceptance in `arguments_match`. The § 7.2 test must red.
4. After all changes: delete each § 5 rule in turn. Its fixture must red in
   `every_fixture_holds_i1_for_every_client`, not only in the `stream_failure` column.
5. After all changes: restore the raising `Partial` behaviour. The § 6.3 unit test must red.

`openai_p1_and_p2_hold` and `anthropic_p1_and_p2_hold` must pass at 10 000 cases after all
changes. A random red in them before § 5 is expected but is not part of the proof.

## 8. Changes to the test deciders

None are needed for the proof: D2 is decider-free. The existing three deciders stay.

## 9. Non-goals

- `json.rs` `DATA_KEYS` matches `input`, `input_schema` and `parameters` by key name only. That is
  safe while no classifier reads below those keys. SMA-750 or SMA-752 must re-check it when one
  does. This change does not touch it.
- Kind `other` arguments are not compared (§ 4). An SMA-752 policy that reads them needs its own
  rule.
- A BOM-prefixed first record that openai-python ignores on the upstream bytes reaches it after the
  SMA-749 D2 BOM strip. With a chunk that has no `choices`, the client then raises. This is
  availability only: no tool call is affected (a hidden first record with a call makes the client
  run fewer calls). It is SMA-749 behaviour, and this change does not fix it. The generator keeps
  such records off the first line when it emits a BOM.
- No change to the Anthropic assembly, to bodies (except § 5.8), or to the decider port
  (`ToolCallDecider`).
- The OpenAI Responses API (`/v1/responses`) is SMA-769, which starts after this change. It needs
  its own ingress, format and oracle models. It takes the D2 rule and the recording method of this
  spec from the start (§ 2.4).
- Support for an upstream that sends `"type": null` on every continuation fragment (§ 5.6). It
  gets `on_failure`; this change does not measure or relax that.
- The LangChain model is `<not pinned: modelled only>` (`oracle.rs`). This change does not pin it.
  If the real SDK does not raise where the model raises, the § 5 rules still hold: they only reject
  streams.

## 10. Left to the builder

- The exact doc-comment text, the proptest strategies, and the fixture contents within § 7.3.
- Whether a test-support constructor for `ToolCall` is needed (§ 7.2).
- The exact place of each § 5 check in `Check::apply` or the classifier, within the rule that
  every check runs on the copy before anything changes.
