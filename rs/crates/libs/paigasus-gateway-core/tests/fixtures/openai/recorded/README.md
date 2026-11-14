# Recorded OpenAI streams

These files are live recordings of OpenAI Chat Completions streams with tool calls. They were
recorded on 2026-11-14. They are not hand-built. Each file holds the raw response bytes,
unchanged.

- Models: `gpt-4.1`, `gpt-5.5` and `gpt-6.1-terra`. Calls to `gpt-6.1-terra` use
  `reasoning_effort: "none"`. Without it, the API refuses function tools on this endpoint.
- Scenarios: `parallel_functions`, `legacy_functions`, `custom_and_function`,
  `custom_and_one_function` and `custom_forced`. `gpt-4.1` rejects `custom` tools, so it has only
  the first two.
- Each request set `stream_options.include_usage`.

The test `recorded_streams_pass_unchanged` in `tests/fixtures.rs` replays every file. The
interceptor must pass each stream through with no dropped record and no `on_failure` path.

## Record again

`capture.py` uses only the Python standard library. Keep the API key in a file outside the
repository. The script reads the key from that file and never prints it.

    python3 -I capture.py --key-file ~/.config/paigasus/openai.key --list-models
    python3 -I capture.py --key-file ~/.config/paigasus/openai.key --model <id> --runs 5 --out <dir>
    python3 -I capture.py --key-file ~/.config/paigasus/openai.key --model gpt-6.1-terra \
        --reasoning-effort none --runs 5 --out <dir>

Use `--reasoning-effort none` for `gpt-6.1-terra`. The 2026-11-14 run made 60 streams. This
directory keeps 12 of them: one for each model and scenario.

CI does not run the script. CI only replays the recorded files.

## What the 60 recorded streams showed (spec section 2.4)

- R1. No stream interleaves calls. The fragments of each call are in contiguous records.
- R2. No stream holds a chunk without `choices`, a choice entry without `delta`, a
  `"type": null`, a top-level `chunk` or `type` key, or a choice other than 0.
- R3. The only chunk with empty `choices` is the final usage chunk (60 of 60).
- R4. A `custom` call streams as `{"index","id","type":"custom","custom":{...}}` and then
  `{"index","custom":{"input":...}}` entries. They were never next to a function call.
- R5. Not measured: a `custom` call and a function call in one response. In 10 tries the models
  never did it.
