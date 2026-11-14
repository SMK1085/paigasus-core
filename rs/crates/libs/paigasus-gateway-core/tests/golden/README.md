<!-- SPDX-License-Identifier: Apache-2.0 -->
# Golden tool-interception streams

Each `.sse` file is the interceptor's output for the same-named fixture in `tests/fixtures/<format>/conforming/`,
run with the `block_named` decider (every call named `blocked_*` is refused) and `on_failure = block`.
`tests/golden.rs` checks them byte-exact. Regenerate with `UPDATE_GOLDEN=1` (exactly `1`; empty or `0` does not),
then review the diff. Only the ten files listed in `manifest.json` and the manifest are goldens; this README is not.

## Reading the streams

- Lines such as `: paigasus-decision {"v":1,...}` are SSE comments carrying the decision report. A client must
  ignore them as comments. They are not data frames.
- A blocked call is removed from the stream and a notice is added as text.

## `manifest.json`

A JSON array with one entry per golden:

- `file`: path relative to this directory.
- `format`: `"openai"`, `"anthropic"` or `"responses"`.
- `allowed`: every call the decider allowed, in decision order, as `{name, arguments, choice}`.
  `arguments` is the raw argument string (OpenAI) or the compact input JSON (Anthropic). `choice` is the choice index (always 0 for Anthropic).
- `notices`: a `string[]` with one notice line per blocked call, in decision order. Assert each as a
  SUBSTRING of the text a client reads: OpenAI may prefix a notice with `"\n\n"`.
- `notice_choices`: `number[]`, the same length as `notices`; the choice index each notice belongs to.
- `finish_reasons`: an object keyed by choice index (as a string) giving that choice's final finish reason, read from the
  golden stream (`tool_calls`, `stop`, ...; for Anthropic the `message_delta` `stop_reason` under key `"0"`).

New fields are additive; existing fields do not change shape.

## The `responses` set

The four files in `responses/` are the interceptor's output for the OpenAI Responses API
(`/v1/responses`, spec § 10.4). The source is `tests/fixtures/responses/conforming/<name>.sse`.

- `function_blocked_deltas`: a blocked function call with argument deltas. One call before it is allowed.
- `builtin_blocked_added_done`: a blocked built-in item (`mcp_approval_request`) with only an
  `added` and a `done` event. Its notice name is `<server_label>.<name>`.
- `truncated_item`: a function call that the upstream cut (`status: "incomplete"`). The interceptor
  blocks it with the code `truncated`, whatever the decider says. The terminal event is
  `response.incomplete`.
- `blocked_after_reasoning`: a blocked call after a `reasoning` item.

A blocked item becomes a `message` item with the id `msg_paigasus_<output_index>`. Its four events
repeat the `sequence_number` of the replaced frame (spec § 6.5). The reader sees the notice as text.

A `responses` entry has these keys in addition to the common ones:

- `allowed`: each entry is `{name, arguments, choice, position, id, namespace}`. `position` is the
  `output_index`. `id` is the `call_id`. `namespace` is `null` when the call has none.
- `notice_positions`: `number[]`, the same length as `notices`; the `output_index` each notice belongs to.
- `notice_choices` is `0` for every entry.
- `finish_reasons`: `{"0": <the status of the terminal response>}`, for example `completed` or `incomplete`.
  The Responses API has no `finish_reason`.

The comment `: paigasus-decision {...}` before a rewritten item is the decision report. A client must
ignore it as a comment.
