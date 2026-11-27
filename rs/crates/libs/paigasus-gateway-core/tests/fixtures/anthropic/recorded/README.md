# Recorded Anthropic streams

These files are live recordings of Anthropic Messages answers. They were recorded on 2026-11-25
(SMA-750). Each `.sse` file holds the raw response bytes. Two changes are made. The D21 scrub:
a top-level `request_id` in a data line becomes `req_paigasus_fixture`. The `web_fetch` stream
(`server_tool_web_fetch.sse`) is synthetic in two places, because the fetched file is a
third-party document: the base64 `source.data` is a generated PDF header and filler of the
measured length, and every letter in a `text_delta` is `x`. The record structure and every byte
length are the real ones. `python3 -I capture.py --synthesize-web-fetch FILE` makes this change,
and a live run applies it before it writes. Each `.timing.json` file holds
the arrival time of each network chunk, with the cumulative byte offset of its end.

The non-stream bodies, the request bodies and the scrubbed response headers
(`<scenario>.response.json`) are in `rs/crates/services/paigasus-gateway/tests/fixtures/anthropic/`.
A service test reads only that directory. The `.sse` files stay here, because the CORE replay
reads them.

- Models: `claude-sonnet-5-5` and `claude-sonnet-4-6`. `claude-sonnet-5-5` refuses `tool_choice`
  of type `any` and `tool`, and refuses `thinking.type: "enabled"`. So `parallel_tool_use`,
  `thinking_then_tool_use`, `large_tool_input`, `max_tokens_in_tool_use`, `non_stream_tool_use`
  and `tool_input_examples` were recorded on `claude-sonnet-4-6`. The 400 answers of the first try
  are kept as `rejected_5_5_*`.
- Scenarios with a stream: `text_stream`, `parallel_tool_use`, `thinking_then_tool_use`,
  `large_tool_input`, `server_tool_web_fetch`, `max_tokens_in_tool_use`.
- Scenarios with a body: `non_stream_tool_use`, `prompt_too_long`, `count_tokens`, `models`,
  `structured_output_output_format`, `structured_output_output_config`, `tool_input_examples`,
  `rate_limited`.
- Beta values used: `web-fetch-2025-09-10` (`server_tool_web_fetch`),
  `structured-outputs-2025-11-13` (`structured_output_output_format` only),
  `advanced-tool-use-2025-11-20` (`tool_input_examples`). `anthropic-version` was `2023-06-01`.
- Accepted: both structured-output forms (`output_format` with its beta, and `output_config.format`
  with no beta), `input_examples`, and `web_fetch`.
- Rejected: see the models line above.
- Not measured: a 429 with its headers (200 `count_tokens` calls gave none), a mid-stream `error`
  event, and the `web_search` fallback (`web_fetch` worked).

Spec section 15 holds the results (record sizes, hold time, data subtrees, types).

The test `anthropic_recorded_streams_replay` in `tests/fixtures.rs` replays every `.sse` file. The
test `anthropic_recordings_hold_no_key` scans every data file here. The GW test
`every_anthropic_fixture_is_scrubbed` scans the other directory.

## Record again

`capture.py` and `measure.py` use only the Python standard library. Keep the API key in a file
outside the repository. The script reads the key from that file and never prints it.

    python3 -I capture.py --key-file ~/.config/paigasus/anthropic-api-key --list-models
    python3 -I capture.py --key-file ~/.config/paigasus/anthropic-api-key --model <id> \
        --thinking-model <id> --out <dir>
    python3 -I capture.py --key-file ~/.config/paigasus/anthropic-api-key --model <id> \
        --thinking-model <id> --only large_tool_input --out <dir>
    python3 -I measure.py <dir>

`--dry-run` writes the request bodies and sends nothing. The script refuses to write a file that
holds the key prefix. A live run costs money. The scenarios `large_tool_input` and
`server_tool_web_fetch` are the largest.

CI does not run the scripts. CI only replays the recorded files.
