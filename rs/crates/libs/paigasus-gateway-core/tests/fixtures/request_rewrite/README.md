# Request rewrite fixtures (SMA-752 § 6.5.1)

Each case is one pair: `<case>.in.json` is the request, and `<case>.out.json` is the exact bytes
that `rewrite_request` must make from it. `tests/request_rewrite.rs` plans each case from a
fixture snapshot: an enforce-mode tenant that permits every tool and forbids the tools in its
`CASES` row for every input.

The request shapes come from the provider API reference pages below, not from a live call. No API
key exists for this change, so no case was sent to a real upstream. Spec § 6.5.1 asks for the real
upstream answer of each case where a key is available. The PR quotes this table as the list of
cases that are not measured.

| case | fixture | source (doc page) | measured against the real upstream |
|---|---|---|---|
| Chat: remove one of two tools | `chat_remove_one_of_two` | OpenAI Chat Completions API reference, `tools` (https://platform.openai.com/docs/api-reference/chat/create) | not measured (no API key in this change) |
| Chat: remove the last tool (drops `tools`, `tool_choice`, `parallel_tool_calls`) | `chat_remove_last_tool` | OpenAI Chat Completions API reference, `tools`, `tool_choice`, `parallel_tool_calls` | not measured (no API key in this change) |
| Chat: legacy `functions`, last removal (drops `functions`, `function_call`) | `chat_legacy_functions_last` | OpenAI Chat Completions API reference, deprecated `functions` and `function_call` | not measured (no API key in this change) |
| Chat: a named `tool_choice` reset to `"auto"` | `chat_named_reset_to_auto` | OpenAI Chat Completions API reference, `tool_choice` | not measured (no API key in this change) |
| Chat: `allowed_tools` narrowed (the `mode` stays) | `chat_allowed_tools_narrowed` | OpenAI Chat Completions API reference, `tool_choice` (allowed tools) | not measured (no API key in this change) |
| Chat: `role:"tool"` content replaced | `chat_tool_result_replaced` | OpenAI Chat Completions API reference, tool message | not measured (no API key in this change) |
| Chat: legacy `role:"function"` content replaced | `chat_legacy_function_result_replaced` | OpenAI Chat Completions API reference, deprecated function message | not measured (no API key in this change) |
| Responses: remove a function tool (a hosted tool stays) | `responses_remove_function_tool` | OpenAI Responses API reference, `tools` (https://platform.openai.com/docs/api-reference/responses/create) | not measured (no API key in this change) |
| Responses: remove the last tool (drops `tools`, `tool_choice`, `parallel_tool_calls`) | `responses_remove_last_tool` | OpenAI Responses API reference, `tools`, `tool_choice`, `parallel_tool_calls` | not measured (no API key in this change) |
| Responses: remove an inner namespace tool, drop the empty namespace | `responses_namespace_emptied` | OpenAI Responses API reference, `tools` (namespace) | not measured (no API key in this change) |
| Responses: every output type of § 6.5.2 replaced in `output` | `responses_outputs_replaced` | OpenAI Responses API reference, `input` items (`function_call_output`, `custom_tool_call_output`, `computer_call_output`, `local_shell_call_output`, `shell_call_output`, `apply_patch_call_output`) | not measured (no API key in this change) |
| Responses: `mcp_approval_response` changed to `"approve": false` with `"reason"` set to the notice | `responses_mcp_approval_refused` | OpenAI Responses API reference, `input` items (`mcp_approval_request`, `mcp_approval_response`) | not measured (no API key in this change) |
| Anthropic: remove a tool | `anthropic_remove_tool` | Anthropic Messages API reference, `tools` (https://docs.anthropic.com/en/api/messages) | not measured (no API key in this change) |
| Anthropic: the last tool, no `tool_use`/`tool_result` blocks (drops `tools`, `tool_choice`) | `anthropic_last_tool_no_blocks` | Anthropic Messages API reference, `tools`, `tool_choice` | not measured (no API key in this change) |
| Anthropic: the last tool with a `tool_result` block in the messages (the definition stays) | `anthropic_last_tool_kept_with_tool_result` | Anthropic Messages API reference, `tools`, content blocks | not measured (no API key in this change) |
| Anthropic: `tool_result` content replaced and `is_error: true` | `anthropic_tool_result_replaced` | Anthropic Messages API reference, `tool_result` content block | not measured (no API key in this change) |

Two shapes need the real upstream answer most. A `computer_call_output` and a `shell_call_output`
take a structured `output` in the API reference (a screenshot object, an array of results). The
rewrite puts the notice string there, as spec § 6.5.2 says. Nobody has measured whether the
upstream accepts a string in these two places.
