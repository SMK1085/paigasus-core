# SPDX-License-Identifier: Apache-2.0
"""Writes the hand-built SSE fixtures for paigasus-gateway-core (SMA-749 spec § 8.3).

Every fixture here is HAND-BUILT, not a recording. Blocked calls are named blocked_* and carry
CANARY in their arguments. Run from py/ through the python-env wrapper:

    bash ../ci/python-env/run.sh run py python -I ../rs/crates/libs/paigasus-gateway-core/tests/fixtures/make_fixtures.py \
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
    # A bodyless call with `"type": null` and no id: kind Other, named `unknown` (spec § 5.3).
    "bodyless_type_null": [text(0, "", role=True), ch([{"index": 0, "delta": {"tool_calls": [{"index": 0, "type": None}]}, "finish_reason": None}]), fin(0)],
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
    # The rewrite removes the blocked call 0, so record 3 is the first bodyless record in the
    # output, and LangChain would hold only a prefix of call 1 (spec § 5.1).
    "fragment_after_blocked_bodyless_entry": [
        ch([{"index": 0, "delta": {"role": "assistant", "content": None, "tool_calls": [{"index": 0, "id": "call_0_0", "type": "custom", "custom": {"name": "blocked_early", "input": "CANARY-early"}}]}, "finish_reason": None}]),
        call(0, 1, "read_file", '{"v":"ab'),
        ch([{"index": 0, "delta": {"tool_calls": [{"index": 2, "id": "call_0_2", "type": "custom", "custom": {"name": "note", "input": "x"}}]}, "finish_reason": None}]),
        call(0, 1, "", 'cdefgh","k":"CANARY-later"}', first=False),
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


# ---- OpenAI Responses (SMA-769 spec § 10.4) -----------------------------------------------------
#
# Hand-built from the openai-python 3.48.0 types and the shapes that spec § 3.3 measured. A
# conforming stream follows M2/M3/M4: `event:` lines equal to `data.type`, `sequence_number` from 0,
# `obfuscation` on every `*.delta`, contiguous items, a function `added` with `arguments: ""`, a
# `reasoning` terminal item whose `encrypted_content` differs from its `done` item (A1), and
# built-in `added` items that are partial views of `done` (A2). A malformed fixture has one bad
# record after the first client-run `added`. Every blocked call is named `blocked_*`, and its
# arguments carry CANARY.

R_PREFIX = {
    "function": "fc",
    "custom": "ctc",
    "message": "msg",
    "reasoning": "rs",
    "computer": "cu",
    "local_shell": "lsh",
    "shell": "sh",
    "apply_patch": "ap",
    "tool_search": "ts",
    "mcp_approval": "mcpr",
    "web_search": "ws",
    "file_search": "fs",
}
R_CALL_ID = {"function", "custom", "computer", "local_shell", "shell", "apply_patch", "tool_search"}
R_CLIENT_RUN = R_CALL_ID | {"mcp_approval"}
ADDED_T = "response.output_item.added"
DONE_T = "response.output_item.done"


def I(kind, name="", text="", pieces=2, ns=None):
    return {"kind": kind, "name": name, "text": text, "pieces": pieces, "ns": ns}


def r_split(s, pieces):
    if pieces <= 1 or len(s) < 2:
        return [s]
    step = -(-len(s) // pieces)
    return [s[k : k + step] for k in range(0, len(s), step)]


def scoped(ty, i, item_id, **extra):
    v = {"type": ty, **extra}
    v["item_id"] = item_id
    v["output_index"] = i
    return v


def ev_added(i, item):
    return {"type": ADDED_T, "item": item, "output_index": i}


def ev_done(i, item):
    return {"type": DONE_T, "item": item, "output_index": i}


def with_status(item, status):
    return {**item, "status": status}


def hidden_call(item_id):
    return {"type": "function_call", "id": item_id, "status": "completed", "arguments": '{"v":"CANARY-hidden"}', "call_id": "call_hidden", "name": "blocked_hidden"}


def r_deltas(ty, i, item_id, text, pieces):
    out = []
    for d in r_split(text, pieces):
        out.append(("ev", {**scoped(ty, i, item_id, delta=d), "obfuscation": "ob"}))
    return out


def build_item(i, it, item_id, call_id):
    """Returns (seg, terminal_item). A seg is a dict of record lists."""
    k, text = it["kind"], it["text"]
    seg = {"pre": [], "added": None, "middle": [], "inner": [], "done": None, "post": []}
    if k in ("function", "custom"):
        ty, key, fam = ("function_call", "arguments", "response.function_call_arguments") if k == "function" else ("custom_tool_call", "input", "response.custom_tool_call_input")
        done = {"type": ty, "id": item_id, "status": "completed", "call_id": call_id, "name": it["name"], key: text}
        if it["ns"]:
            done["namespace"] = it["ns"]
        opened = {**with_status(done, "in_progress"), key: ""}
        seg["middle"] = r_deltas(f"{fam}.delta", i, item_id, text, it["pieces"])
        seg["middle"].append(("ev", scoped(f"{fam}.done", i, item_id, **{key: text})))
        opened_done_term = (opened, done, dict(done))
    elif k == "message":
        part = {"type": "output_text", "annotations": [], "logprobs": [], "text": text}
        empty = {**part, "text": ""}
        opened = {"type": "message", "id": item_id, "status": "in_progress", "content": [], "role": "assistant"}
        done = {"type": "message", "id": item_id, "status": "completed", "content": [part], "role": "assistant"}
        seg["middle"].append(("ev", scoped("response.content_part.added", i, item_id, content_index=0, part=empty)))
        for d in r_split(text, it["pieces"]):
            seg["middle"].append(("ev", {**scoped("response.output_text.delta", i, item_id, content_index=0, delta=d, logprobs=[]), "obfuscation": "ob"}))
        seg["middle"].append(("ev", scoped("response.output_text.done", i, item_id, content_index=0, logprobs=[], text=text)))
        seg["middle"].append(("ev", scoped("response.content_part.done", i, item_id, content_index=0, part=part)))
        opened_done_term = (opened, done, dict(done))
    elif k == "reasoning":
        summary = {"type": "summary_text", "text": text}
        opened = {"type": "reasoning", "id": item_id, "content": [], "encrypted_content": f"enc-added-{i}", "summary": []}
        done = {"type": "reasoning", "id": item_id, "content": [], "encrypted_content": f"enc-done-{i}", "summary": [summary]}
        seg["middle"].append(("ev", scoped("response.reasoning_summary_part.added", i, item_id, part={"type": "summary_text", "text": ""}, summary_index=0)))
        for d in r_split(text, it["pieces"]):
            seg["middle"].append(("ev", {**scoped("response.reasoning_summary_text.delta", i, item_id, delta=d, summary_index=0), "obfuscation": "ob"}))
        seg["middle"].append(("ev", scoped("response.reasoning_summary_text.done", i, item_id, summary_index=0, text=text)))
        seg["middle"].append(("ev", scoped("response.reasoning_summary_part.done", i, item_id, part=summary, summary_index=0)))
        # Amendment A1: the terminal `encrypted_content` differs from the `done` one.
        opened_done_term = (opened, done, {**done, "encrypted_content": f"enc-terminal-{i}"})
    elif k == "computer":
        action = {"type": "type", "text": text} if text else {"type": "screenshot"}
        opened = {"type": "computer_call", "id": item_id, "status": "in_progress", "call_id": call_id}
        done = {"type": "computer_call", "id": item_id, "status": "completed", "actions": [action], "call_id": call_id}  # A2: `actions` only at `done`
        opened_done_term = (opened, done, dict(done))
    elif k == "local_shell":
        done = {"type": "local_shell_call", "id": item_id, "status": "completed", "call_id": call_id, "action": {"type": "exec", "command": ["ls", text], "env": {}}}
        opened_done_term = (with_status(done, "in_progress"), done, dict(done))
    elif k == "shell":
        commands = [f"ls {text}"] + (["date"] if it["pieces"] == 3 else [])
        opened = {"type": "shell_call", "id": item_id, "status": "in_progress", "action": {"commands": [], "max_output_length": None, "timeout_ms": None}, "call_id": call_id, "environment": None}
        done = {"type": "shell_call", "id": item_id, "status": "completed", "action": {"commands": commands, "max_output_length": 20000, "timeout_ms": None}, "call_id": call_id, "environment": None}
        for n, c in enumerate(commands):
            def cmd(ty, **extra):
                return {"type": ty, **extra, "command_index": n, "output_index": i}
            seg["middle"].append(("ev", cmd("response.shell_call_command.added", command="")))
            for d in r_split(c, it["pieces"]):
                seg["middle"].append(("ev", {**cmd("response.shell_call_command.delta", delta=d), "obfuscation": "ob"}))
            seg["middle"].append(("ev", cmd("response.shell_call_command.done", command=c)))
        opened_done_term = (opened, done, dict(done))
    elif k == "apply_patch" and not text:
        done = {"type": "apply_patch_call", "id": item_id, "status": "completed", "call_id": call_id, "operation": {"type": "delete_file", "path": "old.txt"}}
        opened_done_term = (with_status(done, "in_progress"), done, dict(done))
    elif k == "apply_patch":
        diff = f"+{text}\n"
        done = {"type": "apply_patch_call", "id": item_id, "status": "completed", "call_id": call_id, "operation": {"type": "create_file", "diff": diff, "path": "a.txt"}}
        opened = with_status(done, "in_progress")
        opened["operation"] = {**done["operation"], "diff": ""}
        seg["middle"] = r_deltas("response.apply_patch_call_operation_diff.delta", i, item_id, diff, it["pieces"])
        seg["middle"].append(("ev", scoped("response.apply_patch_call_operation_diff.done", i, item_id, diff=diff)))
        opened_done_term = (opened, done, dict(done))
    elif k == "tool_search":
        done = {"type": "tool_search_call", "id": item_id, "status": "completed", "arguments": {"query": text}, "call_id": call_id, "execution": "client"}
        opened_done_term = ({**with_status(done, "in_progress"), "arguments": {}}, done, dict(done))
    elif k == "mcp_approval":
        done = {"type": "mcp_approval_request", "id": item_id, "arguments": text, "name": it["name"], "server_label": "srv"}
        opened_done_term = (dict(done), done, dict(done))
    elif k in ("web_search", "file_search"):
        fam = "response.web_search_call" if k == "web_search" else "response.file_search_call"
        for t in ("in_progress", "searching", "completed"):
            seg["middle"].append(("ev", scoped(f"{fam}.{t}", i, item_id)))
        if k == "web_search":
            opened = {"type": "web_search_call", "id": item_id, "status": "in_progress"}
            done = {"type": "web_search_call", "id": item_id, "status": "completed", "action": {"type": "search", "queries": [text], "query": text}}
        else:
            opened = {"type": "file_search_call", "id": item_id, "status": "in_progress", "queries": []}
            done = {"type": "file_search_call", "id": item_id, "status": "completed", "queries": [text], "results": None}
        opened_done_term = (opened, done, dict(done))
    else:
        raise ValueError(k)
    opened, done, term = opened_done_term
    seg["added"] = ("ev", ev_added(i, opened))
    seg["done"] = ("ev", ev_done(i, done))
    return seg, term


def lifecycle(ty, **resp):
    return ("ev", {"type": ty, "response": {"id": "resp_1", "object": "response", "status": "in_progress", "output": [], **resp}})


class RS:
    """One Responses stream under construction. A quirk edits `segs`, `output` or the head."""

    def __init__(self, items, seq=True, ev_lines=True, done_marker=False, resp_extra=None):
        self.items = items
        self.seq, self.ev_lines, self.done_marker = seq, ev_lines, done_marker
        self.resp_extra = resp_extra or {}
        self.ids = [f"{R_PREFIX[it['kind']]}_{i}" for i, it in enumerate(items)]
        self.call_ids = [f"call_{i}" if it["kind"] in R_CALL_ID else "" for i, it in enumerate(items)]
        self.head = [lifecycle("response.created", **self.resp_extra), lifecycle("response.in_progress", **self.resp_extra)]
        self.term_type, self.term_id, self.output_mode = "response.completed", "resp_1", "items"
        self.tail, self.term_present, self.eof, self.bom, self.final_blank = [], True, False, False, True
        self.built = False

    def build(self):
        self.segs, self.output = [], []
        for i, it in enumerate(self.items):
            seg, term = build_item(i, it, self.ids[i], self.call_ids[i])
            self.segs.append(seg)
            self.output.append(term)
        self.built = True
        return self

    def first_client_run(self):
        return next(i for i, it in enumerate(self.items) if it["kind"] in R_CLIENT_RUN)

    def after_first_added(self, rec):
        self.segs[self.first_client_run()]["middle"].insert(0, rec)

    def render(self):
        if not self.built:
            self.build()
        recs = list(self.head)
        for seg in self.segs:
            for key in ("pre", "added", "middle", "inner", "done", "post"):
                v = seg[key]
                if v is None:
                    continue
                recs.extend(v if isinstance(v, list) else [v])
        if self.term_present:
            status = self.term_type.split(".")[1]
            resp = {"id": self.term_id, "object": "response", "status": status, **self.resp_extra}
            if self.output_mode == "items":
                resp["output"] = self.output
            elif self.output_mode == "null":
                resp["output"] = None
            if self.term_type == "response.failed":
                resp["error"] = {"code": "server_error", "message": "failed"}
            elif self.term_type == "response.incomplete":
                resp["incomplete_details"] = {"reason": "max_output_tokens"}
            else:
                resp["usage"] = {"input_tokens": 3, "output_tokens": 5, "total_tokens": 8}
            recs.append(("ev", {"type": self.term_type, "response": resp}))
        recs.extend(self.tail)
        n = 0
        body = "﻿" if self.bom else ""
        for kind, v, *rest in [(r[0], r[1], *r[2:]) for r in recs]:
            name, data = None, None
            if kind in ("ev", "named"):
                pinned = v.get("_seq") if isinstance(v, dict) else None
                v = {k: x for k, x in v.items() if k != "_seq"}
                if pinned is not None:
                    v["sequence_number"] = pinned
                elif self.seq:
                    v = {**v, "sequence_number": n}
                    n += 1
            if kind == "raw":
                data = v
            else:
                data = dump(v)
            if kind == "named":
                name = rest[0]
            elif kind in ("ev", "bare") and self.ev_lines:
                name = v["type"]
            if name is not None:
                body += f"event: {name}\n"
            body += f"data: {data}\n\n"
        if self.done_marker and not self.eof:
            body += "data: [DONE]\n\n"
        if not self.final_blank:
            body = body.rstrip("\n")
        return body


def fn(name, args, pieces=2, ns=None):
    return I("function", name, args, pieces, ns)


def write_rs(kind, name, rs):
    write(root / "responses" / kind / f"{name}.sse", rs.render() if isinstance(rs, RS) else rs)


READ = fn("read_file", '{"p":1}')


def rcanary(tag):
    return fn("blocked_rm", f'{{"path":"CANARY-{tag}"}}', 3)


STRUCT = {
    "text": {"format": {"type": "json_schema", "name": "out", "strict": True, "schema": {"type": "object", "properties": {"Name": {"type": "string"}, "ID": {"type": "integer", "maximum": 1e300}, "Type": {"type": "string"}, "constructor": {"type": "string"}}}}},
    "metadata": {"Id": "m1", "Type": "t"},
}

CONFORMING = {
    "text_only": RS([I("message", text="Hello there.", pieces=3)]),
    "reasoning_then_function": RS([I("reasoning", text="Let me look.", pieces=2), fn("read_file", '{"p":1}')]),
    "function_allowed": RS([READ]),
    "function_blocked": RS([rcanary("1")]),
    "parallel_mixed": RS([READ, rcanary("2")]),
    "custom_blocked": RS([I("custom", "blocked_sql", "DROP CANARY-3", 3)]),
    "namespace_function": RS([fn("blocked_lookup", '{"q":"CANARY-4"}', 2, "crm")]),
    "computer_call": RS([I("computer", text="hello"), rcanary("5")]),
    "local_shell_call": RS([I("local_shell", text="-la"), rcanary("6")]),
    "shell_call_commands": RS([I("shell", text="-la", pieces=3), rcanary("7")]),
    "apply_patch_call": RS([I("apply_patch", text="line", pieces=3), rcanary("8")]),
    "tool_search_call": RS([I("tool_search", text="weather"), rcanary("9")]),
    "mcp_approval_request": RS([I("mcp_approval", "blocked_deploy", '{"env":"CANARY-10"}')]),
    "web_search_then_function": RS([I("web_search", text="paigasus"), I("web_search", text="gateway"), fn("read_file", '{"p":2}'), rcanary("11")]),
    "provider_only": RS([I("web_search", text="x"), I("file_search", text="y"), I("message", text="Found.")]),
    "no_sequence_numbers": RS([READ, rcanary("12")], seq=False),
    "done_marker": RS([READ, rcanary("13")], done_marker=True),
    "structured_output_keys": RS([I("message", text='{"Name":"a","ID":1}', pieces=2)], resp_extra=STRUCT),
    # The golden sources (spec § 10.4). `truncated_item` is a cut call: the interceptor blocks it as
    # `Truncated` even under allow-all (A3), so its own test row expects the rewrite.
    "function_blocked_deltas": RS([fn("read_file", '{"p":"a.txt"}'), fn("blocked_rm", '{"path":"CANARY-g1","force":true}', 4)]),
    "builtin_blocked_added_done": RS([I("mcp_approval", "blocked_deploy", '{"env":"CANARY-g2"}')]),
    "truncated_item": RS([fn("read_file", '{"v":"CANARY-g3","w":"abcdef"', 3)]),
    "blocked_after_reasoning": RS([I("reasoning", text="Thinking.", pieces=2), fn("blocked_exfil", '{"to":"CANARY-g4"}', 3)]),
}
for name, rs in CONFORMING.items():
    rs.build()
    if name == "truncated_item":
        rs.segs[0]["done"][1]["item"]["status"] = "incomplete"
        rs.output[0]["status"] = "incomplete"
        rs.term_type = "response.incomplete"
    write_rs("conforming", name, rs)

# A keepalive has no `sequence_number` (R7 exemption) and its own `event:` line.
rs = RS([READ, rcanary("14")]).build()
rs.segs[0]["middle"].insert(1, ("bare", {"type": "keepalive"}))
rs.segs[1]["middle"].insert(0, ("bare", {"type": "keepalive"}))
write_rs("conforming", "keepalive_events", rs)


# ---- Responses: one bad record per rule (spec § 6.4) -----------------------------------------------
# The bad record comes after the first client-run `added`. The `malformed` set holds the cases whose
# outcome is a `Malformed` stream failure. `edge` holds the cases with another outcome (an upstream
# error, a truncation, a limit, a record after the terminal event, the SSE layer, a clean stream).

def base(extra=None):
    return RS([READ, rcanary("m"), I("message", text="ok")] if extra is None else extra).build()


MALFORMED, EDGE = {}, {}


def bad(name, rs, edge=False):
    (EDGE if edge else MALFORMED)[name] = rs


def mk(name, f, items=None, edge=False):
    rs = base(items)
    f(rs)
    bad(name, rs, edge)


def sub(rs, i, key, fnc):
    fnc(rs.segs[i][key])


# R1 / R3 / R6: stream and lifecycle shape.
mk("event_error_name", lambda rs: rs.segs[0]["middle"].__setitem__(0, ("named", rs.segs[0]["middle"][0][1], "error")))
mk("done_trailing", lambda rs: rs.after_first_added(("raw", "[DONE]x")))
mk("unknown_type", lambda rs: rs.after_first_added(("ev", {"type": "response.unknown_kind", "item": hidden_call("fc_hidden")})))
mk("repeated_index", lambda rs: rs.segs[0]["post"].append(("ev", ev_added(0, hidden_call("fc_hidden")))))
mk("index_gap", lambda rs: rs.segs[0]["post"].append(("ev", ev_added(2, hidden_call("fc_hidden")))))
mk("item_null", lambda rs: rs.segs[0]["post"].append(("ev", {"type": ADDED_T, "item": None, "output_index": 1})))
mk("repeated_created", lambda rs: rs.after_first_added(lifecycle("response.created")))
mk("first_not_created", lambda rs: rs.head.pop(0))
mk("response_id_changed", lambda rs: rs.head[1][1]["response"].__setitem__("id", "resp_other"))
mk("early_output", lambda rs: rs.head[1][1]["response"].__setitem__("output", [hidden_call("fc_hidden")]))
mk("terminal_output_absent", lambda rs: setattr(rs, "output_mode", "absent"))
mk("terminal_output_null", lambda rs: setattr(rs, "output_mode", "null"))
mk("terminal_item_never_streamed", lambda rs: rs.output.append(hidden_call("fc_hidden")))
mk("terminal_item_never_streamed_then_unknown_record", lambda rs: (rs.output.append(hidden_call("fc_hidden")), setattr(rs, "term_type", "response.failed"), rs.tail.append(("ev", {"type": "response.unknown_kind"}))))
# R7: an event repeats the previous `sequence_number` (`_seq` pins the number).
mk("sequence_not_increasing", lambda rs: rs.segs[0]["middle"].__setitem__(0, ("ev", {**rs.segs[0]["middle"][0][1], "_seq": 2})))

# R4 / R6 / R8: order, ids and names.
mk("delta_before_added", lambda rs: rs.segs[1]["pre"].append(rs.segs[1]["middle"].pop(0)))
mk("item_id_mismatch", lambda rs: rs.segs[0]["middle"][-1][1].__setitem__("item_id", "fc_other"))
mk("wrong_item_type_event", lambda rs: rs.segs[2]["middle"].insert(0, ("ev", scoped("response.function_call_arguments.delta", 2, "msg_2", delta='{"v":"CANARY-wt"}'))))
mk("duplicate_id", lambda rs: (rs.ids.__setitem__(1, rs.ids[0]), rs.build()))
mk("duplicate_call_id", lambda rs: (rs.call_ids.__setitem__(1, rs.call_ids[0]), rs.build()))
mk("reserved_id", lambda rs: (rs.ids.__setitem__(2, "msg_paigasus_2"), rs.build()))
def interleave(rs):
    """Item 1 starts before item 0 is done (R5): its records move into item 0."""
    nxt = rs.segs[1]
    moved = [nxt["added"], *nxt["middle"], nxt["done"]]
    rs.segs[0]["inner"] = moved
    nxt.update({"added": None, "middle": [], "done": None})


mk("item_interleaved", interleave)
# R9 / R10: terminal items.
mk("type_changes_at_done", lambda rs: (rs.segs[2]["done"][1].__setitem__("item", hidden_call("msg_2")), rs.output.__setitem__(2, hidden_call("msg_2"))))
mk("type_changes_in_terminal", lambda rs: rs.output.__setitem__(2, hidden_call("msg_2")))
mk("terminal_new_name", lambda rs: rs.output[0].__setitem__("name", "blocked_hidden"))
mk("terminal_new_namespace", lambda rs: rs.output[0].__setitem__("namespace", "evil"))
mk("terminal_args_differ", lambda rs: rs.output[0].__setitem__("arguments", '{"v":"CANARY-td"}'))
# R11 / R12 / R13 / R14: the call surfaces must agree.
mk("added_arguments_not_empty", lambda rs: rs.segs[0]["added"][1]["item"].__setitem__("arguments", '{"p":1}'))
mk("arguments_mismatch", lambda rs: rs.segs[0]["middle"].insert(len(rs.segs[0]["middle"]) - 1, ("ev", scoped("response.function_call_arguments.delta", 0, "fc_0", delta="x", obfuscation="ob"))))
mk("arguments_done_mismatch", lambda rs: rs.segs[0]["middle"][-1][1].__setitem__("arguments", '{"p":1}x'))
mk("invalid_json_arguments", lambda rs: None, items=[fn("read_file", '{"v":"CANARY-ij"'), rcanary("m")])
mk(
    "input_mismatch",
    lambda rs: rs.segs[0]["middle"][-1][1].__setitem__("input", "SELECT 1x"),
    items=[I("custom", "run_sql", "SELECT 1", 2), rcanary("m")],
)
mk(
    "shell_command_mismatch",
    lambda rs: (rs.segs[0]["done"][1]["item"]["action"]["commands"].__setitem__(0, "rm -rf /"), rs.output[0]["action"]["commands"].__setitem__(0, "rm -rf /")),
    items=[I("shell", text="-la", pieces=2), rcanary("m")],
)
mk(
    "diff_mismatch",
    lambda rs: (rs.segs[0]["done"][1]["item"]["operation"].__setitem__("diff", "+changed\n"), rs.output[0]["operation"].__setitem__("diff", "+changed\n")),
    items=[I("apply_patch", text="line", pieces=2), rcanary("m")],
)
mk(
    "added_not_subset",
    lambda rs: rs.segs[0]["added"][1]["item"].__setitem__("extra", "x"),
    items=[I("computer", text="hello"), rcanary("m")],
)
mk(
    "added_not_subset_value",
    lambda rs: rs.segs[0]["added"][1]["item"].__setitem__("actions", [{"type": "click", "button": "left"}]),
    items=[I("computer", text="hello"), rcanary("m")],
)
# R5 / R16: contiguity.
mk("event_after_done", lambda rs: rs.segs[0]["post"].insert(0, ("ev", scoped("response.function_call_arguments.delta", 0, "fc_0", delta="CANARY-late", obfuscation="ob"))))
mk("item_interleaved_no_done", lambda rs: (interleave(rs), rs.segs[0].__setitem__("done", None)))

# ---- Responses: the other outcomes (spec § 6.3, § 6.4 R3 R15, D13, D16, D17) --------------------------
# An upstream error record is not a deviation: it is forwarded, or it aborts an open hold (D17).
mk("top_level_error", lambda rs: rs.after_first_added(("ev", {"type": "response.error", "error": {"code": "server_error", "message": "boom"}})), edge=True)
mk("error_event_mid_item", lambda rs: rs.after_first_added(("ev", {"type": "error", "code": "server_error", "message": "boom", "param": None})), edge=True)
# R3: a record after the terminal event is dropped and counted.
mk("record_after_terminal", lambda rs: rs.tail.append(("ev", ev_added(3, hidden_call("fc_hidden")))), edge=True)
# R15: a client-run item that never gets its `done`.
def no_done_failed(rs):
    rs.segs[0]["done"] = None
    rs.term_type = "response.failed"
    rs.output[0] = rs.segs[0]["added"][1]["item"]  # the partial item (a terminal view of an open call)


mk("failed_with_open_item", no_done_failed, items=[rcanary("m")], edge=True)


def eof_in_item(rs):
    rs.segs[0]["done"] = None
    rs.term_present, rs.eof = False, True


mk("eof_in_item", eof_in_item, items=[rcanary("m")], edge=True)
mk("no_terminal", lambda rs: setattr(rs, "term_present", False), edge=True)
mk("absent_status_function", lambda rs: (rs.segs[0]["done"][1]["item"].pop("status"), rs.output[0].pop("status")), edge=True)
# A custom call has no `status` in the model, so an absent status is not a truncation.
mk("absent_status_custom", lambda rs: (rs.segs[0]["done"][1]["item"].pop("status"), rs.output[0].pop("status")), items=[I("custom", "note", "x", 2), rcanary("m")], edge=True)
# D16: a record over `max_record_bytes` (the tests use 4096).
def oversized(rs):
    big = "CANARY-big" + "x" * 5000
    rs.segs[0]["middle"].insert(1, ("ev", scoped("response.function_call_arguments.delta", 0, "fc_0", delta=big)))


mk("oversized_delta", oversized, edge=True)


def oversized_terminal(rs):
    h = hidden_call("fc_big")
    h["arguments"] = '{"v":"CANARY-big' + "x" * 5000 + '"}'
    rs.output.append(h)


mk("oversized_terminal", oversized_terminal, edge=True)
# D13: one open call that holds more than `max_held_bytes` (the tests use 16 KiB) in small records.
mk("hold_limit", lambda rs: None, items=[fn("blocked_big", '{"v":"CANARY-' + "y" * 20000 + '"}', 20), READ], edge=True)
# SSE layer: a BOM, and no blank line after the last event. Both are clean streams.
def bom(rs):
    rs.bom = True


mk("bom", bom, edge=True)
mk("no_final_blank_line", lambda rs: setattr(rs, "final_blank", False), edge=True)
# An `event:` name that is not the type, on a record before any hold: the record is dropped and the
# stream goes on (no stream-level failure).
mk("event_error_name_before_hold", lambda rs: rs.head.__setitem__(1, ("named", rs.head[1][1], "error")))

for name, rs in MALFORMED.items():
    write_rs("malformed", name, rs)
for name, rs in EDGE.items():
    write_rs("edge", name, rs)
