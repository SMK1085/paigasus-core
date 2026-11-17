# Request rewrite fixtures (SMA-752 § 6.5.1)

Each case is one pair: `<case>.in.json` is the request, and `<case>.out.json` is the exact bytes
that `rewrite_request` must make from it. `tests/request_rewrite.rs` plans each case from a
fixture snapshot: an enforce-mode tenant that permits every tool and forbids the tools in its
`CASES` row for every input.

The request shapes come from the provider API reference pages below. SMA-776 sent the Responses
result types to the real upstream (2026-11-16); the section "Responses result types, real
upstream" below holds the answers. No other case was sent to a real upstream. The PR quotes the
table as the list of cases that are not measured.

| case | fixture | source (doc page) | measured against the real upstream |
|---|---|---|---|
| Chat: remove one of two tools | `chat_remove_one_of_two` | OpenAI Chat Completions API reference, `tools` (https://platform.openai.com/docs/api-reference/chat/create) | not measured (SMA-776 measured only the Responses result types) |
| Chat: remove the last tool (drops `tools`, `tool_choice`, `parallel_tool_calls`) | `chat_remove_last_tool` | OpenAI Chat Completions API reference, `tools`, `tool_choice`, `parallel_tool_calls` | not measured (SMA-776 measured only the Responses result types) |
| Chat: legacy `functions`, last removal (drops `functions`, `function_call`) | `chat_legacy_functions_last` | OpenAI Chat Completions API reference, deprecated `functions` and `function_call` | not measured (SMA-776 measured only the Responses result types) |
| Chat: a named `tool_choice` reset to `"auto"` | `chat_named_reset_to_auto` | OpenAI Chat Completions API reference, `tool_choice` | not measured (SMA-776 measured only the Responses result types) |
| Chat: `allowed_tools` narrowed (the `mode` stays) | `chat_allowed_tools_narrowed` | OpenAI Chat Completions API reference, `tool_choice` (allowed tools) | not measured (SMA-776 measured only the Responses result types) |
| Chat: `role:"tool"` content replaced | `chat_tool_result_replaced` | OpenAI Chat Completions API reference, tool message | not measured (SMA-776 measured only the Responses result types) |
| Chat: legacy `role:"function"` content replaced | `chat_legacy_function_result_replaced` | OpenAI Chat Completions API reference, deprecated function message | not measured (SMA-776 measured only the Responses result types) |
| Responses: remove a function tool (a hosted tool stays) | `responses_remove_function_tool` | OpenAI Responses API reference, `tools` (https://platform.openai.com/docs/api-reference/responses/create) | not measured (SMA-776 measured only the Responses result types) |
| Responses: remove the last tool (drops `tools`, `tool_choice`, `parallel_tool_calls`) | `responses_remove_last_tool` | OpenAI Responses API reference, `tools`, `tool_choice`, `parallel_tool_calls` | not measured (SMA-776 measured only the Responses result types) |
| Responses: remove an inner namespace tool, drop the empty namespace | `responses_namespace_emptied` | OpenAI Responses API reference, `tools` (namespace) | not measured (SMA-776 measured only the Responses result types) |
| Responses: every Text output type gets the notice string; `shell_call_output` gets one notice chunk; the `computer_call_output` case is not a fixture, because the gateway refuses it (inline tests in `src/policy/rewrite.rs` and `src/policy/plan.rs`); and a `local_shell_call_output` with only `id` replaced when that `id` is the `call_id` or the item `id` of a denied `local_shell_call` (an `id` that matches no call stays) | `responses_outputs_replaced` | OpenAI Responses API reference, `input` items (`function_call_output`, `custom_tool_call_output`, `computer_call_output`, `local_shell_call_output`, `shell_call_output`, `apply_patch_call_output`) | measured (SMA-776, see below), except `local_shell_call_output` |
| Responses: `mcp_approval_response` changed to `"approve": false` with `"reason"` set to the notice | `responses_mcp_approval_refused` | OpenAI Responses API reference, `input` items (`mcp_approval_request`, `mcp_approval_response`) | not measured: it needs an `mcp_approval_request` from a real MCP server |
| Anthropic: remove a tool | `anthropic_remove_tool` | Anthropic Messages API reference, `tools` (https://docs.anthropic.com/en/api/messages) | not measured (SMA-776 measured only the Responses result types) |
| Anthropic: the last tool, no `tool_use`/`tool_result` blocks (drops `tools`, `tool_choice`) | `anthropic_last_tool_no_blocks` | Anthropic Messages API reference, `tools`, `tool_choice` | not measured (SMA-776 measured only the Responses result types) |
| Anthropic: the last tool with a `tool_result` block in the messages (the definition stays) | `anthropic_last_tool_kept_with_tool_result` | Anthropic Messages API reference, `tools`, content blocks | not measured (SMA-776 measured only the Responses result types) |
| Anthropic: `tool_result` content replaced and `is_error: true` | `anthropic_tool_result_replaced` | Anthropic Messages API reference, `tool_result` content block | not measured (SMA-776 measured only the Responses result types) |

## Responses result types, real upstream (2026-11-16, SMA-776)

Synthetic bodies, `POST https://api.openai.com/v1/responses`, model `gpt-6-sol`,
`store: false`, `max_output_tokens: 16`. Each body has a user message, one call, and one output
for that call. "no tool" means that `tools` is absent. This is the body that the rewrite sends
when the denied tool was the last definition.

| case | `tools` | `output` | answer |
|---|---|---|---|
| shell control | `shell` | `[{"stdout":"127.0.0.1 localhost","stderr":"","outcome":{"type":"exit","exit_code":0}}]` | 200 |
| computer control | `computer` | `{"type":"computer_screenshot","image_url":"data:image/png;base64,<1x1 PNG>"}` | 200 |
| shell, the SMA-752 rewrite | `shell` | the notice string | 400 `invalid_type`, `input[2].output`: "expected an array of objects, but got a string instead." |
| computer, the SMA-752 rewrite | `computer` | the notice string | 400 `invalid_type`, `input[2].output`: "expected a computer call output, but got a string instead." |
| shell, notice in `stderr`, exit 1 | `shell` | `[{"stdout":"","stderr":"<notice>","outcome":{"type":"exit","exit_code":1}}]` | 200 |
| shell, notice in `stdout`, exit 0 | `shell` | `[{"stdout":"<notice>","stderr":"","outcome":{"type":"exit","exit_code":0}}]` | 200 |
| shell, notice chunk | no tool | the `stderr` chunk | 200 |
| shell, two commands, one notice chunk | `shell` | the `stderr` chunk | 200 |
| shell, two commands, one notice chunk | no tool | the `stderr` chunk | 200 |
| shell, `max_output_length: 10` on the output item | no tool | the `stderr` chunk (the notice is longer than 10) | 200 |
| `function_call_output` | no tool | the notice string | 200 |
| `custom_tool_call_output` | no tool | the notice string | 200 |
| `apply_patch_call_output` | no tool | the notice string | 200 |
| `apply_patch_call_output` | `apply_patch` | the notice string | 200 |
| `local_shell_call_output` | `local_shell`, and no tool | the notice string | not measurable: 400 `invalid_value` at the `local_shell_call` item (`input[1]`) in both bodies |

More facts from the same session:

- With the GA `computer` tool, the upstream rejects a `computer_call` that has `action`:
  "Computer call input must include exactly one of `action` or `actions`." The body with
  `actions: [{"type":"screenshot"}]` passes. The gateway does not read either member: the plan
  matches a result to its call by `call_id` only.
- `mcp_approval_response` is not measured. It needs an `mcp_approval_request` from a real MCP
  server.
- The schema source is the openai-python SDK types (`response_input_item_param.py`). A
  `shell_call_output` chunk has three required members: `stdout`, `stderr`, `outcome`. `outcome`
  is `{"type":"exit","exit_code":<int>}` or `{"type":"timeout"}`. The screenshot object has no
  text member.
