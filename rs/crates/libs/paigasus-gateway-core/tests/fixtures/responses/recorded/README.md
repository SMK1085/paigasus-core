# Recorded OpenAI Responses streams

These files are live recordings of OpenAI Responses API (`/v1/responses`) streams. They were
recorded on 2026-11-14 for SMA-769 (spec § 3.3, § 10.5). They are not hand-built. Each file holds
the raw response bytes, unchanged.

- Models: `gpt-6.1-sol` (no reasoning-effort override), `gpt-6.1-terra` and `gpt-4.1`.
- Scenarios, 3 runs per model: `parallel_functions`, `custom_and_function`, `custom_forced`,
  `reasoning_then_call`, `text_only`, `structured_output`, `web_search_then_function`,
  `max_output_tokens_in_call` and `long_instructions_30_tools`.
- Scenarios, 1 run on `gpt-6.1-sol` only: `apply_patch_forced`, `shell_forced`,
  `local_shell_forced` and `computer_forced`.
- Continuations, 1 run per model: `m1_unanswered_continuation` and `m6_rewrite_replay`. They
  write `.measure.json` files, not streams, so this directory keeps none of them.

The run made 78 streams. This directory keeps 28 of them: the first run of each model and
scenario that the model accepts (see "What was rejected"). A file is named `<model>__<scenario>.sse`.
Each file passes the canonical rules of spec § 6.4 as § 15 amended them (A1, A2, A4), with no
deviation. Task 1 first kept only 17 files, because the rules of the day (R10, R13, R14) rejected
the 11 others. Task 9 checked the 11 again against the amended rules, with no new API call, and
they all pass. Their raw `.sse` bytes are unchanged. The `.error.json` and `.measure.json` files
of the run are not kept.

Two tests in `tests/fixtures.rs` replay this directory.

- `recorded_responses_streams_pass_unchanged` replays every file except the cut streams. Under
  every decider: the stream ends, no record is dropped, no `on_failure` path runs, and I1 holds for
  every client model. Under allow-all: the output is payload-identical to the input, and every
  split point gives the one-push output. A file over 12 KiB gets the split points near each record
  boundary and every 499th point, because the cost of all points grows with the square of the size.
- `recorded_responses_cut_streams_are_rewritten` replays the three `max_output_tokens_in_call`
  files. Each one ends with a `function_call` that has `status: "incomplete"`. The expected outcome
  is a `Truncated` block with the exact § 6.5 rewrite (spec § 15, A3), not a pass with no change.

## What was rejected

- `gpt-4.1` refuses `custom` tools: HTTP 400, `Invalid value: 'custom'`, code
  `unknown_parameter`. So `gpt-4.1` has no `custom_and_function` and no `custom_forced` file.
- `gpt-6.1-sol` refuses the `local_shell` tool: HTTP 400, `The local_shell tool is no longer
  supported.`. So no file has a `local_shell_call`.

## What the amendments changed for these files

Four rules of spec § 6.4 did not match these streams on the day of the run. Spec § 15 amended
them, and the files above follow the amended rules.

- A `reasoning` item has a different `encrypted_content` in the terminal `output` than in
  `response.output_item.done` (26 streams of `gpt-6.1-sol` and `gpt-6.1-terra`). A1: R10 full
  equality applies to client-run items only.
- `gpt-6.1-sol__computer_forced`: `added` has no `actions`, and `done` has them. A2: the subset
  rule.
- `gpt-6.1-sol__apply_patch_forced`: `added` has `operation.diff: ""`, and the diff streams in
  `response.apply_patch_call_operation_diff.delta` and `.done` events. A2 and A4: the subset rule,
  and the two event types are known now.
- `gpt-6.1-sol__shell_forced`: `action.max_output_length` is `null` at `added` and `20000` at
  `done`. A2: the subset rule.

## Client-run kinds with no recording here

`make_fixtures.py` builds these from the openai-python 3.48.0 types: `local_shell_call`,
`tool_search_call` and `mcp_approval_request`. No model made a `custom_tool_call` and a
`function_call` in one response.

## The M1 to M7 results

The full results with the counts are in spec § 3.3.

- M1. A `previous_response_id` continuation with no `function_call_output` gets HTTP 400,
  `No tool output found for function call <call_id>.` (3 of 3 models).
- M2. Every record has an `event:` line equal to `data.type`. No stream has `[DONE]`. Every
  event has `sequence_number`, from 0 and with no gap. Every `*.delta` event has `obfuscation`, and no other event type has it.
  No terminal `response` has `output_text`. No stream has a `keepalive` event.
- M3. The events of each item are contiguous, and `output_index` does not change from `added` to
  `done` (78 of 78). R10 does not hold for `reasoning` items (see above).
- M4. `function_call` and `custom_tool_call` start with `""` at `added`. `computer_call`,
  `apply_patch_call` and `shell_call` are not complete at `added` (see above).
- M5. A cut function call gets `output_item.done` with `status: "incomplete"` and arguments that
  are not complete JSON, then `response.incomplete` (9 of 9).
- M6. OpenAI accepts the § 6.5 rewritten output as `input` with `store: true`, with
  `store: false`, and with `store: false` and `include: ["reasoning.encrypted_content"]` (9 of 9).
- M7. With 50,000 bytes of `instructions` and 30 tools, `response.created` is at most 60,507
  bytes and `response.completed` is at most 60,976 bytes.

## Record again

`capture.py` and `measure.py` are in `../../openai/recorded/`. They use only the Python standard
library. Keep the API key in a file outside the repository. The script reads the key from that
file and never prints it.

    python3 -I capture.py --api responses --dry-run --model gpt-6.1-sol --runs 1 --out <dir>
    python3 -I capture.py --api responses --key-file ~/.config/paigasus/openai.key \
        --model <id> --runs 3 --out <dir> --only parallel_functions custom_and_function \
        custom_forced reasoning_then_call text_only structured_output web_search_then_function \
        max_output_tokens_in_call long_instructions_30_tools
    python3 -I capture.py --api responses --key-file ~/.config/paigasus/openai.key \
        --model gpt-6.1-sol --runs 1 --out <dir> --only apply_patch_forced shell_forced \
        local_shell_forced computer_forced
    python3 -I capture.py --api responses --key-file ~/.config/paigasus/openai.key \
        --model <id> --runs 1 --out <dir> --only m1_unanswered_continuation m6_rewrite_replay
    python3 -I measure.py <dir>

CI does not run the scripts. CI only replays the recorded files.
