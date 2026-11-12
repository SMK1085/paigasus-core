<!-- SPDX-License-Identifier: Apache-2.0 -->
# Golden tool-interception streams

Each `.sse` file is the interceptor's output for the same-named fixture in `tests/fixtures/<format>/conforming/`,
run with the `block_named` decider (every call named `blocked_*` is refused) and `on_failure = block`.
`tests/golden.rs` checks them byte-exact. Regenerate with `UPDATE_GOLDEN=1` (exactly `1`; empty or `0` does not),
then review the diff. Only the six files listed in `manifest.json` and the manifest are goldens; this README is not.

## Reading the streams

- Lines such as `: paigasus-decision {"v":1,...}` are SSE comments carrying the decision report. A client must
  ignore them as comments. They are not data frames.
- A blocked call is removed from the stream and a notice is added as text.

## `manifest.json`

A JSON array with one entry per golden:

- `file`: path relative to this directory.
- `format`: `"openai"` or `"anthropic"`.
- `allowed`: every call the decider allowed, in decision order, as `{name, arguments, choice}`.
  `arguments` is the raw argument string (OpenAI) or the compact input JSON (Anthropic). `choice` is the choice index (always 0 for Anthropic).
- `notices`: a `string[]` with one notice line per blocked call, in decision order. Assert each as a
  SUBSTRING of the text a client reads: OpenAI may prefix a notice with `"\n\n"`.
- `notice_choices`: `number[]`, the same length as `notices`; the choice index each notice belongs to.
- `finish_reasons`: an object keyed by choice index (as a string) giving that choice's final finish reason, read from the
  golden stream (`tool_calls`, `stop`, ...; for Anthropic the `message_delta` `stop_reason` under key `"0"`).

New fields are additive; existing fields do not change shape.
