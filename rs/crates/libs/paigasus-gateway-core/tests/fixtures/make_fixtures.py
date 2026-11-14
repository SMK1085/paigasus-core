# SPDX-License-Identifier: Apache-2.0
"""Writes the hand-built SSE fixtures for paigasus-gateway-core (SMA-749 spec § 8.3).

Every fixture here is HAND-BUILT, not a recording. Blocked calls are named blocked_* and carry
CANARY in their arguments. Run from py/ with the repo's uv:

    uv run python -I ../rs/crates/libs/paigasus-gateway-core/tests/fixtures/make_fixtures.py \
        ../rs/crates/libs/paigasus-gateway-core/tests/fixtures
"""

import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
BOM = "﻿"


def dump(v):
    return json.dumps(v, separators=(",", ":"))


def d(v):
    return "data: " + (v if isinstance(v, str) else dump(v)) + "\n\n"


def ch(choices, **extra):
    return {"id": "chatcmpl-fx", "object": "chat.completion.chunk", "created": 1790942400, "model": "gpt-4o-mini", "system_fingerprint": "fp_fx", "choices": choices, **extra}


def text(c, s, role=False):
    delta = {"role": "assistant", "content": s} if role else {"content": s}
    return ch([{"index": c, "delta": delta, "finish_reason": None}])


def call(c, i, name, args, first=True, role=None):
    """One tool-call fragment. The first fragment of call 0 carries the role, as OpenAI sends it."""
    e = {"index": i, "id": f"call_{c}_{i}", "type": "function", "function": {"name": name, "arguments": args}} if first else {"index": i, "function": {"arguments": args}}
    delta = {"tool_calls": [e]}
    if role if role is not None else (first and i == 0):
        delta = {"role": "assistant", "content": None, **delta}
    return ch([{"index": c, "delta": delta, "finish_reason": None}])


def fin(c, reason="tool_calls"):
    return ch([{"index": c, "delta": {}, "finish_reason": reason}])


DONE = "data: [DONE]\n\n"
HIDDEN = {"index": 0, "id": "call_h", "type": "function", "function": {"name": "blocked_hidden", "arguments": "CANARY-hidden"}}


def one_blocked(canary):
    return [call(0, 0, "blocked_rm", '{"path":'), call(0, 0, "", f'"{canary}"}}', first=False), fin(0)]


OPENAI = {
    "text_only": [text(0, "Hello", role=True), text(0, " world"), fin(0, "stop")],
    "one_call_allowed": [call(0, 0, "read_file", '{"path":'), call(0, 0, "", '"/tmp/a"}', first=False), fin(0)],
    "one_call_blocked": one_blocked("CANARY-1"),
    "parallel_mixed": [call(0, 0, "read_file", '{"p":1}'), call(0, 1, "blocked_rm", '{"p":"CANARY-2"}'), fin(0)],
    "interleaved": [call(0, 0, "read_file", '{"p":'), call(0, 1, "blocked_rm", '{"p":'), call(0, 0, "", "1}", first=False), call(0, 1, "", '"CANARY-3"}', first=False), fin(0)],
    # n = 2. Only choice 0 may carry tool calls (LangChain reads choices[0] of every chunk), so
    # choice 1 is text.
    "n2_choices": [text(0, "A", role=True), text(1, "B", role=True), call(0, 0, "blocked_exfil", '{"to":"CANARY-4"}', role=False), call(0, 1, "read_file", "{}"), fin(1, "stop"), fin(0)],
    "legacy_blocked": [ch([{"index": 0, "delta": {"role": "assistant", "content": None, "function_call": {"name": "blocked_rm", "arguments": '{"p":"CANARY-5"}'}}, "finish_reason": None}]), fin(0, "function_call")],
    "text_after_calls": [call(0, 0, "blocked_rm", '{"p":"CANARY-6"}'), text(0, "done"), fin(0)],
    "custom_tool_blocked": [ch([{"index": 0, "delta": {"role": "assistant", "content": None, "tool_calls": [{"index": 0, "id": "call_c", "type": "custom", "custom": {"name": "blocked_sql", "input": "DROP CANARY-7"}}]}, "finish_reason": None}]), fin(0)],
    "usage_chunk": one_blocked("CANARY-8") + [{"id": "chatcmpl-fx", "object": "chat.completion.chunk", "choices": [], "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5}}],
    "event_line": one_blocked("CANARY-9"),
}

OPENAI_MALFORMED = {
    "nan_record": [text(0, "a", role=True), '{"x":NaN,"choices":[{"index":0,"delta":{"tool_calls":[' + dump(HIDDEN) + "]}}]}", text(0, "b"), fin(0, "stop")],
    "duplicate_key_mid_hold": [call(0, 0, "read_file", "{}"), '{"choices":[{"index":0,"delta":{"tool_calls":[' + dump(HIDDEN) + ']},"delta":{"content":"x"}}]}', fin(0)],
    "case_folded_key": ['{"choices":[{"index":0,"delta":{"Tool_Calls":[' + dump(HIDDEN) + "]}}]}", text(0, "ok", role=True), fin(0, "stop")],
    "proto_key": ['{"choices":[{"index":0,"delta":{"__proto__":{"tool_calls":[' + dump(HIDDEN) + "]}}}]}", text(0, "ok", role=True), fin(0, "stop")],
    "float_index": [ch([{"index": 0, "delta": {"tool_calls": [{**HIDDEN, "index": 0.0}]}}]), text(0, "ok", role=True), fin(0, "stop")],
    "repeated_name": [call(0, 0, "read_file", "{}"), ch([{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"name": "blocked_x", "arguments": "CANARY-rn"}}]}}]), fin(0)],
    "fragment_after_bodyless_entry": [
        call(0, 0, "read_file", '{"v":"ab'),
        ch([{"index": 0, "delta": {"tool_calls": [{"index": 1, "id": "call_0_1", "type": "custom", "custom": {"name": "note", "input": "x"}}]}, "finish_reason": None}]),
        call(0, 0, "", 'cdefgh","k":"CANARY-bodyless"}', first=False),
        fin(0),
    ],
    "chunk_without_choices": [
        call(0, 0, "read_file", '{"v":"ab'),
        '{"id":"chatcmpl-fx","object":"chat.completion.chunk","created":1790942400,"model":"gpt-4o-mini","note":"CANARY-nochoices"}',
        call(0, 0, "", 'cdefgh"}', first=False),
        fin(0),
    ],
    "choice_without_delta": [
        call(0, 0, "read_file", '{"v":"ab'),
        ch([{"index": 0, "finish_reason": None, "note": "CANARY-nodelta"}]),
        call(0, 0, "", 'cdefgh"}', first=False),
        fin(0),
    ],
    "choice_without_delta_in_opening_record": [
        text(0, "", role=True),
        ch([{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_0_0", "type": "function", "function": {"name": "read_file", "arguments": '{"v":"ab'}}]}, "finish_reason": None}, {"index": 1, "note": "CANARY-opening"}]),
        call(0, 0, "", 'cdefgh"}', first=False),
        fin(0),
        ch([{"index": 1, "delta": {"role": "assistant", "content": "x"}, "finish_reason": "stop"}]),
    ],
    "type_null": [
        call(0, 0, "read_file", '{"v":"ab'),
        ch([{"index": 0, "delta": {"tool_calls": [{"index": 0, "type": None, "function": {"arguments": "CANARY-typenull"}}]}, "finish_reason": None}]),
        call(0, 0, "", 'cdefgh"}', first=False),
        fin(0),
    ],
    "chunk_key": [
        call(0, 0, "read_file", '{"v":"ab'),
        ch([], chunk={"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_ck", "type": "function", "function": {"name": "blocked_chunk", "arguments": "{\"v\":\"CANARY-chunk\"}"}}]}}]}),
        call(0, 0, "", 'cdefgh"}', first=False),
        fin(0),
    ],
    "content_delta_type": [
        call(0, 0, "read_file", '{"v":"ab'),
        {**ch([{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"arguments": "CANARY-cdt"}}]}, "finish_reason": None}]), "type": "content.delta"},
        call(0, 0, "", 'cdefgh"}', first=False),
        fin(0),
    ],
    "array_outside_tool_calls": [
        call(0, 0, "read_file", '{"v":"ab'),
        ch([{"index": 0, "delta": {"foo": [{"index": 0, "note": "CANARY-array"}]}, "finish_reason": None}]),
        ch([{"index": 0, "delta": {"foo": [1]}, "finish_reason": None}]),
        call(0, 0, "", 'cdefgh"}', first=False),
        fin(0),
    ],
    "function_call_key": [
        ch([{"index": 0, "delta": {"role": "assistant", "content": None, "function_call": {"name": "read_file", "arguments": '{"v":"ab'}}, "finish_reason": None}]),
        ch([{"index": 0, "delta": {"function_call": {"arguments": "cd", "x": "CANARY-fckey"}}, "finish_reason": None}]),
        ch([{"index": 0, "delta": {"function_call": {"arguments": "ef", "x": 1}}, "finish_reason": None}]),
        ch([{"index": 0, "delta": {"function_call": {"arguments": 'gh"}'}}, "finish_reason": None}]),
        fin(0, "function_call"),
    ],
    "oversized": [call(0, 0, "read_file", "{"), ch([{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"arguments": "CANARY-big" + "x" * 5000}}]}}]), fin(0)],
}


def write(path, body):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(body.encode("utf-8"))


for name, recs in {**OPENAI, **OPENAI_MALFORMED}.items():
    kind = "malformed" if name in OPENAI_MALFORMED else "conforming"
    body = "".join(d(r) for r in recs) + DONE
    if name == "event_line":
        body = body.replace("data: ", "event: message\ndata: ", 1)
    write(root / "openai" / kind / f"{name}.sse", body)

# Shapes that need raw bytes.
write(root / "openai" / "conforming" / "crlf_endings.sse", "".join(d(r) for r in one_blocked("CANARY-10")).replace("\n", "\r\n") + DONE.replace("\n", "\r\n"))
write(root / "openai" / "conforming" / "bom_prefixed.sse", BOM + "".join(d(r) for r in one_blocked("CANARY-11")) + DONE)
write(
    root / "openai" / "conforming" / "escaped_key.sse",
    d('{"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","tool\\u005fcalls":[{"index":0,"id":"call_e","type":"function","function":{"name":"blocked_rm","arguments":"{\\"p\\":\\"CANARY-12\\"}"}}]}}]}') + d(fin(0)) + DONE,
)
write(root / "openai" / "malformed" / "after_done.sse", "".join(d(r) for r in OPENAI["one_call_allowed"]) + DONE + d(ch([{"index": 0, "delta": {"tool_calls": [HIDDEN]}}])))
write(root / "openai" / "malformed" / "error_mid_hold.sse", d(call(0, 0, "blocked_rm", "CANARY-err")) + d({"error": {"message": "boom", "type": "server_error"}}))
write(root / "openai" / "malformed" / "json_body.sse", dump({"choices": [{"index": 0, "message": {"tool_calls": [HIDDEN]}}]}))
write(root / "openai" / "malformed" / "lax_line.sse", "  data: " + dump(ch([{"index": 0, "delta": {"tool_calls": [HIDDEN]}}])) + "\n\n" + d(text(0, "ok", role=True)) + d(fin(0, "stop")) + DONE)


def ev(v):
    return f"event: {v['type']}\ndata: {dump(v)}\n\n"


START = {"type": "message_start", "message": {"id": "msg_fx", "type": "message", "role": "assistant", "content": [], "model": "claude-x", "stop_reason": None, "usage": {"input_tokens": 1, "output_tokens": 1}}}
STOP = {"type": "message_stop"}


def blk_text(i, s):
    return [{"type": "content_block_start", "index": i, "content_block": {"type": "text", "text": ""}}, {"type": "content_block_delta", "index": i, "delta": {"type": "text_delta", "text": s}}, {"type": "content_block_stop", "index": i}]


def blk_tool(i, name, args):
    half = len(args) // 2
    return [
        {"type": "content_block_start", "index": i, "content_block": {"type": "tool_use", "id": f"toolu_{i}", "name": name, "input": {}}},
        {"type": "content_block_delta", "index": i, "delta": {"type": "input_json_delta", "partial_json": args[:half]}},
        {"type": "content_block_delta", "index": i, "delta": {"type": "input_json_delta", "partial_json": args[half:]}},
        {"type": "content_block_stop", "index": i},
    ]


def mdelta(reason):
    return {"type": "message_delta", "delta": {"stop_reason": reason, "stop_sequence": None}, "usage": {"output_tokens": 7}}


ANTHROPIC = {
    "text_only": [START, *blk_text(0, "Hi."), mdelta("end_turn"), STOP],
    "tool_use_allowed": [START, *blk_text(0, "Reading."), *blk_tool(1, "read_file", '{"path":"/tmp/a"}'), mdelta("tool_use"), STOP],
    "tool_use_blocked": [START, *blk_text(0, "Removing."), *blk_tool(1, "blocked_rm", '{"path":"CANARY-A1"}'), mdelta("tool_use"), STOP],
    "two_tools_mixed": [START, *blk_tool(0, "read_file", '{"p":1}'), *blk_tool(1, "blocked_rm", '{"p":"CANARY-A2"}'), mdelta("tool_use"), STOP],
    "server_tool": [
        START,
        {"type": "content_block_start", "index": 0, "content_block": {"type": "server_tool_use", "id": "srv_1", "name": "web_search", "input": {}}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": '{"q":"x"}'}},
        {"type": "content_block_stop", "index": 0},
        {"type": "content_block_start", "index": 1, "content_block": {"type": "web_search_tool_result", "tool_use_id": "srv_1", "content": []}},
        {"type": "content_block_stop", "index": 1},
        *blk_text(2, "Found."),
        mdelta("end_turn"),
        STOP,
    ],
    "thinking_then_tool_blocked": [
        START,
        {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "hmm"}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}},
        {"type": "content_block_stop", "index": 0},
        *blk_tool(1, "blocked_exfil", '{"to":"CANARY-A3"}'),
        mdelta("tool_use"),
        STOP,
    ],
}

for name, evs in ANTHROPIC.items():
    write(root / "anthropic" / "conforming" / f"{name}.sse", "".join(ev(e) for e in evs))

hidden_start = {"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_h", "name": "blocked_hidden", "input": {}}}
write(
    root / "anthropic" / "malformed" / "event_mismatch.sse",
    ev(START) + "event: ping\ndata: " + dump(hidden_start) + "\n\n" + ev({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "CANARY-em"}}) + ev({"type": "content_block_stop", "index": 0}) + ev(STOP),
)
write(root / "anthropic" / "malformed" / "prefilled_input.sse", ev(START) + ev({**hidden_start, "content_block": {**hidden_start["content_block"], "input": {"v": "CANARY-pi"}}}) + ev({"type": "content_block_stop", "index": 0}) + ev(STOP))
write(
    root / "anthropic" / "malformed" / "index_reuse.sse",
    ev(START) + "".join(ev(e) for e in blk_tool(0, "read_file", "{}")[:1]) + ev({**hidden_start, "content_block": {**hidden_start["content_block"], "name": "blocked_reuse"}}) + ev({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "CANARY-ir"}}) + ev({"type": "content_block_stop", "index": 0}) + ev(STOP),
)
write(root / "anthropic" / "malformed" / "prefilled_message.sse", ev({"type": "message_start", "message": {"content": [{"type": "tool_use", "id": "t", "name": "blocked_hidden", "input": {"v": "CANARY-pm"}}]}}) + ev(STOP))
