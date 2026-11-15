# SPDX-License-Identifier: Apache-2.0
"""Measures recorded OpenAI Responses streams (SMA-769 spec § 3.3, M2 to M5 and M7).

Only the standard library is used.

    python3 -I measure.py <dir>          # one JSON object per `*.sse` file in <dir>
    python3 -I measure.py --self-test    # two built-in streams; exit 1 when a field is wrong

A position is the 0-based index of a record in the file. A record is the text between two blank
lines. `max_record_bytes` counts the whole record (the `event:` line and the `data:` line), in
bytes, without the blank line that ends it.
"""

import itertools
import json
import pathlib
import sys

CLIENT_RUN = (
    "function_call",
    "custom_tool_call",
    "computer_call",
    "local_shell_call",
    "shell_call",
    "apply_patch_call",
    "tool_search_call",
    "mcp_approval_request",
)
TERMINAL = ("response.completed", "response.incomplete", "response.failed")


def records(raw: bytes) -> list[dict]:
    """Splits SSE bytes into records: {"event", "data", "bytes"}."""
    text = raw.decode("utf-8").replace("\r\n", "\n")
    out = []
    for block in text.split("\n\n"):
        if not block.strip("\n"):
            continue
        block = block.strip("\n")
        event = None
        data = []
        for line in block.split("\n"):
            if line.startswith(":"):
                continue
            field, _, value = line.partition(":")
            value = value[1:] if value.startswith(" ") else value
            if field == "event":
                event = value
            elif field == "data":
                data.append(value)
        if not data:
            continue  # a comment-only block (for example ": ping") is not a record
        out.append({"event": event, "data": "\n".join(data), "bytes": len(block.encode("utf-8"))})
    return out


def presence(flags: list[bool]) -> str:
    if flags and all(flags):
        return "all"
    if not any(flags):
        return "none"
    return "mixed"


def without(item: dict, *keys: str) -> dict:
    return {k: v for k, v in item.items() if k not in keys}


def complete_json(text) -> bool:
    try:
        json.loads(text)
    except (TypeError, ValueError):
        return False
    return True


def differing_keys(a: dict, b: dict) -> list[str]:
    """Keys whose values differ, one level into nested objects (`action.commands`)."""
    out = []
    for k in sorted(set(a) | set(b)):
        x, y = a.get(k), b.get(k)
        if x == y:
            continue
        if isinstance(x, dict) and isinstance(y, dict):
            out += [f"{k}.{n}" for n in sorted(set(x) | set(y)) if x.get(n) != y.get(n)]
        else:
            out.append(k if k in a and k in b else f"{k} (only at {'added' if k in a else 'done'})")
    return out


def without_commands(item: dict) -> dict:
    out = without(item, "status")
    if isinstance(out.get("action"), dict):
        out["action"] = without(out["action"], "commands")
    return out


LIFECYCLE_EMPTY = ("response.created", "response.queued", "response.in_progress")
R9_KEYS = ("type", "id", "call_id", "namespace", "name")


def rule_checks(events: list[tuple[int, dict, dict]]) -> dict[str, list[str]]:
    """R1-R4, R6, R8, R9 and R16 of spec § 6.4. Each value lists the breaks, as "<position>: <what>"."""
    br: dict[str, list[str]] = {r: [] for r in ("R1", "R2", "R3", "R4", "R6", "R8", "R9", "R16")}
    evs = [(pos, d) for pos, _, d in events if d.get("type") not in ("keepalive", "error")]
    if not evs or evs[0][1].get("type") != "response.created":
        br["R1"].append(f"{evs[0][0] if evs else 0}: first event is not response.created")
    ids = {json.dumps((d.get("response") or {}).get("id")) for _, d in evs if "response" in d}
    if len(ids) > 1:
        br["R1"].append(f"-: {len(ids)} response ids")
    terminals = [pos for pos, d in evs if d.get("type") in TERMINAL]
    if len(terminals) != 1:
        br["R3"].append(f"-: {len(terminals)} terminal events")
    elif terminals and terminals[0] != evs[-1][0]:
        br["R3"].append(f"{terminals[0]}: an event follows the terminal event")
    added: dict[int, dict] = {}
    done: dict[int, dict] = {}
    item_ids: set = set()
    call_ids: set = set()
    for pos, d in evs:
        t = d.get("type")
        if t in LIFECYCLE_EMPTY and (d.get("response") or {}).get("output") != []:
            br["R2"].append(f"{pos}: {t} output is not []")
        if t in TERMINAL and not isinstance((d.get("response") or {}).get("output"), list):
            br["R3"].append(f"{pos}: terminal output is not an array")
        if "output_index" not in d:
            continue
        j = d["output_index"]
        if j in done:
            br["R16"].append(f"{pos}: {t} after the done of index {j}")
        if t == "response.output_item.added":
            if j != len(added) or j in added:
                br["R4"].append(f"{pos}: added index {j}, expected {len(added)}")
            item = d.get("item")
            if not isinstance(item, dict) or not isinstance(item.get("type"), str) or not item.get("id"):
                br["R8"].append(f"{pos}: added item has no string type or no id")
                item = item if isinstance(item, dict) else {}
            iid = item.get("id")
            if iid in item_ids:
                br["R8"].append(f"{pos}: duplicate id {iid}")
            if str(iid).startswith("msg_paigasus_"):
                br["R8"].append(f"{pos}: id {iid} has the msg_paigasus_ prefix")
            item_ids.add(iid)
            if "call_id" in item:
                if item["call_id"] in call_ids:
                    br["R8"].append(f"{pos}: duplicate call_id {item['call_id']}")
                call_ids.add(item["call_id"])
            added[j] = item
        elif t == "response.output_item.done":
            if j not in added or j in done:
                br["R4"].append(f"{pos}: done for index {j} without one added")
            done[j] = d.get("item") or {}
        elif "item_id" in d and d["item_id"] != added.get(j, {}).get("id"):
            br["R6"].append(f"{pos}: {t} item_id is not the id of index {j}")
    terminal = next((d for _, d in reversed(evs) if d.get("type") in TERMINAL), None)
    output = (terminal or {}).get("response", {}).get("output")
    output = output if isinstance(output, list) else []
    for j, item in added.items():
        views = [item] + ([done[j]] if j in done else []) + ([output[j]] if j < len(output) else [])
        for k in R9_KEYS:
            if len({json.dumps(v.get(k)) for v in views}) > 1:
                br["R9"].append(f"-: index {j} key {k} differs")
    return br


def done_vs_terminal(items: dict[int, dict], terminal: dict | None) -> dict[str, dict]:
    """R10 per item type: each `output_item.done` item compared with the terminal `output` item."""
    output = ((terminal or {}).get("response") or {}).get("output")
    output = output if isinstance(output, list) else []
    out: dict[str, dict] = {}
    for j in sorted(items):
        done = items[j]["done"]
        if done is None:
            continue
        row = out.setdefault(items[j]["added"].get("type"), {"equal": 0, "different": 0, "keys": []})
        other = output[j] if j < len(output) else None
        if done == other:
            row["equal"] += 1
            continue
        row["different"] += 1
        keys = ["<missing>"] if other is None else [k for k in sorted(set(done) | set(other)) if done.get(k) != other.get(k)]
        row["keys"] = sorted(set(row["keys"]) | set(keys))
    return out


def measure(raw: bytes) -> dict:
    recs = records(raw)
    events = []  # (position, record, parsed)
    done_pos = None
    for pos, r in enumerate(recs):
        if r["data"] == "[DONE]":
            if done_pos is None:
                done_pos = pos
            continue
        events.append((pos, r, json.loads(r["data"])))

    # M2: framing.
    has_event = [r["event"] is not None for _, r, _ in events]
    event_equal = [r["event"] == d.get("type") for _, r, d in events if r["event"] is not None]
    keep = [pos for pos, _, d in events if d.get("type") == "keepalive"]
    numbered = [(pos, d) for pos, _, d in events if d.get("type") not in ("keepalive", "error")]
    seqs = [d.get("sequence_number") for _, d in numbered]
    seq_present = [isinstance(s, int) for s in seqs]
    ints = [s for s in seqs if isinstance(s, int)]
    terminal = next((d for _, _, d in reversed(events) if d.get("type") in TERMINAL), None)

    # M3: contiguity (R5) and index stability.
    open_items: set[int] = set()
    first_break = None
    added_index: dict[str, int] = {}
    index_break = None
    items: dict[int, dict] = {}  # output_index -> {"added", "done", "deltas", "done_value"}
    for pos, _, d in events:
        if "output_index" not in d:
            continue
        j = d["output_index"]
        t = d.get("type")
        others = open_items - {j}
        if t == "response.output_item.added":
            if others and first_break is None:
                first_break = pos
            open_items.add(j)
            items[j] = {"added": d.get("item", {}), "done": None, "deltas": [], "done_value": None, "cmd": {}, "cmd_done": {}}
            added_index[d.get("item", {}).get("id")] = j
        elif t == "response.output_item.done":
            if (others or j not in open_items) and first_break is None:
                first_break = pos
            open_items.discard(j)
            item = d.get("item", {})
            if added_index.get(item.get("id"), j) != j and index_break is None:
                index_break = pos
            if j in items:
                items[j]["done"] = item
            elif index_break is None:
                index_break = pos
        else:
            if (others or j not in open_items) and first_break is None:
                first_break = pos
            if j in items:
                if t.endswith(".delta") and isinstance(d.get("delta"), str):
                    items[j]["deltas"].append(d["delta"])
                if t in ("response.function_call_arguments.done", "response.custom_tool_call_input.done"):
                    items[j]["done_value"] = d.get("arguments", d.get("input"))
                if t in ("response.shell_call_command.added", "response.shell_call_command.delta"):
                    part = d.get("command", "") if t.endswith(".added") else d.get("delta", "")
                    items[j]["cmd"][d.get("command_index")] = items[j]["cmd"].get(d.get("command_index"), "") + part
                if t == "response.shell_call_command.done":
                    items[j]["cmd_done"][d.get("command_index")] = d.get("command")

    # M4 and M5: client-run shapes.
    shapes = []
    statuses = []
    for j in sorted(items):
        it = items[j]
        kind = it["added"].get("type")
        if kind not in CLIENT_RUN:
            continue
        done = it["done"]
        statuses.append({"index": j, "type": kind, "status": "missing" if done is None else done.get("status")})
        s: dict = {"index": j, "type": kind}
        if kind in ("function_call", "custom_tool_call"):
            key = "arguments" if kind == "function_call" else "input"
            s[f"{key}_at_added"] = it["added"].get(key)
            if done is not None:
                joined = "".join(it["deltas"])
                s["deltas_equal_done"] = joined == it["done_value"] == done.get(key)
                if kind == "function_call":
                    s["arguments_complete_json"] = complete_json(done.get(key))
        elif kind == "shell_call":
            s["commands_at_added"] = (it["added"].get("action") or {}).get("commands")
            if done is not None:
                s["added_equals_done_without_status_and_commands"] = without_commands(it["added"]) == without_commands(done)
                cmds = [it["cmd"][i] for i in sorted(it["cmd"])]
                s["commands_from_events_equal_done"] = (
                    cmds == [it["cmd_done"].get(i) for i in sorted(it["cmd"])] == (done.get("action") or {}).get("commands")
                )
        elif done is not None:
            s["added_equals_done_without_status"] = without(it["added"], "status") == without(done, "status")
        if kind == "apply_patch_call" and done is not None:
            s["diff_deltas_equal_done"] = "".join(it["deltas"]) == (done.get("operation") or {}).get("diff")
        if kind not in ("function_call", "custom_tool_call") and done is not None:
            s["differs_from_done"] = differing_keys(without(it["added"], "status"), without(done, "status"))
        shapes.append(s)

    # M2: obfuscation per event type.
    obf_types = sorted({d.get("type") for _, _, d in events if "obfuscation" in d})
    deltas = [d for _, _, d in events if str(d.get("type")).endswith(".delta")]
    no_item_id = sorted(
        {
            d.get("type")
            for _, _, d in events
            if "output_index" in d and "item_id" not in d and not str(d.get("type")).startswith("response.output_item.")
        }
    )

    # M7: record sizes.
    sizes: dict[str, int] = {}
    for _, r, d in events:
        t = d.get("type", "?")
        sizes[t] = max(sizes.get(t, 0), r["bytes"])

    return {
        "records": len(recs),
        "terminal": None if terminal is None else terminal.get("type"),
        "item_types": [items[j]["added"].get("type") for j in sorted(items)],
        "event_lines": {"present": presence(has_event), "equal_to_data_type": all(event_equal) if event_equal else None},
        "done_marker": {
            "present": done_pos is not None,
            "position": done_pos,
            "last": done_pos is not None and done_pos == len(recs) - 1,
        },
        "sequence_number": {
            "present": presence(seq_present),
            "strictly_increasing": all(b > a for a, b in itertools.pairwise(ints)),
            "starts_at_zero": bool(ints) and ints[0] == 0,
            "no_gap": all(b == a + 1 for a, b in itertools.pairwise(ints)),
        },
        "obfuscation": {
            "present": bool(obf_types),
            "types": obf_types,
            "on_every_delta": all("obfuscation" in d for d in deltas),
        },
        "wire_output_text": None if terminal is None else "output_text" in (terminal.get("response") or {}),
        "keepalive": {"count": len(keep), "positions": keep},
        "contiguous": {"holds": first_break is None, "first_break": first_break},
        "index_stable": {"holds": index_break is None, "first_break": index_break},
        "added_shapes": shapes,
        "done_status": statuses,
        "events_without_item_id": no_item_id,
        "rules": rule_checks(events),
        "done_vs_terminal": done_vs_terminal(items, terminal),
        "max_record_bytes": dict(sorted(sizes.items())),
    }


def sse(events: list[dict], done: bool = True) -> bytes:
    parts = [f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events]
    if done:
        parts.append("data: [DONE]\n\n")
    return "".join(parts).encode()


def seqd(events: list[dict], numbers: list[int] | None = None) -> list[dict]:
    numbers = numbers if numbers is not None else list(range(len(events)))
    return [{**e, "sequence_number": n} for e, n in zip(events, numbers, strict=True)]


def fc(i: int, args: str, status: str, name: str = "f", iid: str | None = None) -> dict:
    return {
        "type": "function_call",
        "id": iid or f"fc_{i}",
        "call_id": f"call_{i}",
        "name": name,
        "arguments": args,
        "status": status,
    }


def rs(enc: str = "e1") -> dict:
    return {"type": "reasoning", "id": "rs_0", "summary": [], "encrypted_content": enc}


def base(**k) -> list[dict]:
    """A conforming stream: a reasoning item at index 0, then a function call at index 1.

    Keyword arguments change one part, so that each self-test case breaks exactly one check."""
    idx = k.get("idx", 1)
    args = k.get("args", "{}")
    call_done = fc(1, args, k.get("status", "completed"), name=k.get("done_name", "f"), iid=k.get("fc_id"))
    delta = {"type": "response.function_call_arguments.delta", "output_index": idx, "item_id": k.get("delta_item_id", k.get("fc_id") or "fc_1"), "delta": args}
    if k.get("delta_obfuscation", True):
        delta["obfuscation"] = "xyz"
    created = {"type": "response.created", "response": {"id": "resp_1", "output": k.get("created_output", [])}}
    progress = {"type": "response.in_progress", "response": {"id": "resp_1", "output": []}}
    if k.get("progress_obfuscation"):
        progress["obfuscation"] = "xyz"
    call_events = [
        {"type": "response.output_item.added", "output_index": idx, "item": fc(1, "", "in_progress", iid=k.get("fc_id"))},
        delta,
        {"type": "response.function_call_arguments.done", "output_index": idx, "item_id": k.get("fc_id") or "fc_1", "arguments": args},
        {"type": "response.output_item.done", "output_index": idx, "item": call_done},
    ]
    if k.get("delta_after_done"):
        call_events = [call_events[0], call_events[2], call_events[3], call_events[1]]
    terminal = {
        "type": "response.completed",
        "response": {"id": k.get("terminal_id", "resp_1"), "output": [rs(k.get("terminal_enc", "e1")), call_done]},
    }
    if k.get("terminal_output_null"):
        terminal["response"]["output"] = None
    events = [
        *([progress, created] if k.get("swap_first") else [created, progress]),
        {"type": "response.output_item.added", "output_index": 0, "item": rs("")},
        {"type": "response.output_item.done", "output_index": 0, "item": rs()},
        *call_events,
        *([] if k.get("no_terminal") else [terminal]),
    ]
    if k.get("two_terminals"):
        events.append(terminal)
    if k.get("event_after_terminal"):
        events.append(progress)
    return seqd(events, k.get("numbers"))


def builtins(done_diff: str = "+hi\n") -> list[dict]:
    """A computer call (no `actions` at `added`), a shell call with streamed commands, and a patch."""
    cu_added = {"type": "computer_call", "id": "cu_0", "call_id": "call_0", "status": "in_progress"}
    cu_done = {**cu_added, "status": "completed", "actions": [{"type": "screenshot"}]}
    sh = {"type": "shell_call", "id": "sh_1", "call_id": "call_1", "environment": None}
    sh_added = {**sh, "status": "in_progress", "action": {"commands": [], "max_output_length": None}}
    sh_done = {**sh, "status": "completed", "action": {"commands": ["ls -la"], "max_output_length": 20000}}
    ap = {"type": "apply_patch_call", "id": "apc_2", "call_id": "call_2"}
    ap_added = {**ap, "status": "in_progress", "operation": {"type": "create_file", "diff": "", "path": "a"}}
    ap_done = {**ap, "status": "completed", "operation": {"type": "create_file", "diff": done_diff, "path": "a"}}
    diff = "response.apply_patch_call_operation_diff"
    return seqd(
        [
            {"type": "response.created", "response": {"id": "resp_1", "output": []}},
            {"type": "response.output_item.added", "output_index": 0, "item": cu_added},
            {"type": "response.output_item.done", "output_index": 0, "item": cu_done},
            {"type": "response.output_item.added", "output_index": 1, "item": sh_added},
            {"type": "response.shell_call_command.added", "output_index": 1, "command_index": 0, "command": ""},
            {"type": "response.shell_call_command.delta", "output_index": 1, "command_index": 0, "delta": "ls", "obfuscation": "x"},
            {"type": "response.shell_call_command.delta", "output_index": 1, "command_index": 0, "delta": " -la", "obfuscation": "x"},
            {"type": "response.shell_call_command.done", "output_index": 1, "command_index": 0, "command": "ls -la"},
            {"type": "response.output_item.done", "output_index": 1, "item": sh_done},
            {"type": "response.output_item.added", "output_index": 2, "item": ap_added},
            {"type": f"{diff}.delta", "output_index": 2, "item_id": "apc_2", "delta": "+hi", "obfuscation": "x"},
            {"type": f"{diff}.delta", "output_index": 2, "item_id": "apc_2", "delta": "\n", "obfuscation": "x"},
            {"type": f"{diff}.done", "output_index": 2, "item_id": "apc_2", "diff": "+hi\n"},
            {"type": "response.output_item.done", "output_index": 2, "item": ap_done},
            {"type": "response.completed", "response": {"id": "resp_1", "output": [cu_done, sh_done, ap_done]}},
        ]
    )


RULES = ("R1", "R2", "R3", "R4", "R6", "R8", "R9", "R16")


def self_test() -> int:
    interleaved = seqd(
        [
            {"type": "response.created", "response": {"id": "resp_1", "output": []}},
            {"type": "response.output_item.added", "output_index": 0, "item": fc(0, "", "in_progress")},
            {"type": "response.output_item.added", "output_index": 1, "item": fc(1, "", "in_progress")},
            {"type": "response.function_call_arguments.delta", "output_index": 0, "item_id": "fc_0", "delta": "{}"},
            {"type": "response.function_call_arguments.done", "output_index": 0, "item_id": "fc_0", "arguments": "{}"},
            {"type": "response.output_item.done", "output_index": 0, "item": fc(0, "{}", "completed")},
            {"type": "response.output_item.done", "output_index": 1, "item": fc(1, "", "completed")},
            {"type": "response.completed", "response": {"id": "resp_1", "output": []}},
        ]
    )
    equal = {"reasoning": {"equal": 1, "different": 0, "keys": []}, "function_call": {"equal": 1, "different": 0, "keys": []}}
    # (name, stream, wanted fields, rules that must break; every other rule must hold)
    cases = [
        (
            "conforming",
            base(),
            {
                "event_lines": {"present": "all", "equal_to_data_type": True},
                "done_marker": {"present": True, "position": 9, "last": True},
                "sequence_number": {"present": "all", "strictly_increasing": True, "starts_at_zero": True, "no_gap": True},
                "obfuscation": {"present": True, "types": ["response.function_call_arguments.delta"], "on_every_delta": True},
                "wire_output_text": False,
                "keepalive": {"count": 0, "positions": []},
                "contiguous": {"holds": True, "first_break": None},
                "index_stable": {"holds": True, "first_break": None},
                "added_shapes": [{"index": 1, "type": "function_call", "arguments_at_added": "", "deltas_equal_done": True, "arguments_complete_json": True}],
                "done_status": [{"index": 1, "type": "function_call", "status": "completed"}],
                "done_vs_terminal": equal,
                "events_without_item_id": [],
            },
            (),
        ),
        (
            "interleaved",
            interleaved,
            {
                "contiguous": {"holds": False, "first_break": 2},
                "index_stable": {"holds": True, "first_break": None},
                "done_status": [
                    {"index": 0, "type": "function_call", "status": "completed"},
                    {"index": 1, "type": "function_call", "status": "completed"},
                ],
            },
            (),
        ),
        ("seq_start", base(numbers=list(range(1, 10))), {"sequence_number": {"present": "all", "strictly_increasing": True, "starts_at_zero": False, "no_gap": True}}, ()),
        ("seq_gap", base(numbers=[0, 1, 2, 3, 5, 6, 7, 8, 9]), {"sequence_number": {"present": "all", "strictly_increasing": True, "starts_at_zero": True, "no_gap": False}}, ()),
        (
            "obfuscation_moved",
            base(delta_obfuscation=False, progress_obfuscation=True),
            {"obfuscation": {"present": True, "types": ["response.in_progress"], "on_every_delta": False}},
            (),
        ),
        ("r1_first", base(swap_first=True), {}, ("R1",)),
        ("r1_ids", base(terminal_id="resp_2"), {}, ("R1",)),
        ("r2", base(created_output=[rs()]), {}, ("R2",)),
        ("r3_two", base(two_terminals=True), {}, ("R3",)),
        ("r3_none", base(no_terminal=True), {}, ("R3",)),
        ("r3_after", base(event_after_terminal=True), {}, ("R3",)),
        ("r3_null", base(terminal_output_null=True), {}, ("R3",)),
        ("r4", base(idx=2), {}, ("R4",)),
        ("r6", base(delta_item_id="fc_9"), {}, ("R6",)),
        ("r8", base(fc_id="msg_paigasus_1"), {}, ("R8",)),
        ("r9", base(done_name="g"), {}, ("R9",)),
        ("r16", base(delta_after_done=True), {"contiguous": {"holds": False, "first_break": 7}}, ("R16",)),
        (
            "cut_call",
            base(args='{"a', status="incomplete"),
            {
                "added_shapes": [{"index": 1, "type": "function_call", "arguments_at_added": "", "deltas_equal_done": True, "arguments_complete_json": False}],
                "done_status": [{"index": 1, "type": "function_call", "status": "incomplete"}],
            },
            (),
        ),
        (
            "builtins",
            builtins(),
            {
                "added_shapes": [
                    {"index": 0, "type": "computer_call", "added_equals_done_without_status": False, "differs_from_done": ["actions (only at done)"]},
                    {
                        "index": 1,
                        "type": "shell_call",
                        "commands_at_added": [],
                        "added_equals_done_without_status_and_commands": False,
                        "commands_from_events_equal_done": True,
                        "differs_from_done": ["action.commands", "action.max_output_length"],
                    },
                    {
                        "index": 2,
                        "type": "apply_patch_call",
                        "added_equals_done_without_status": False,
                        "diff_deltas_equal_done": True,
                        "differs_from_done": ["operation.diff"],
                    },
                ],
                "events_without_item_id": [
                    "response.shell_call_command.added",
                    "response.shell_call_command.delta",
                    "response.shell_call_command.done",
                ],
            },
            (),
        ),
        (
            "patch_diff_mismatch",
            builtins(done_diff="+ho\n"),
            {
                # The first two shapes are the ones that the "builtins" case checks.
                "added_shapes": [
                    *measure(sse(builtins()))["added_shapes"][:2],
                    {
                        "index": 2,
                        "type": "apply_patch_call",
                        "added_equals_done_without_status": False,
                        "diff_deltas_equal_done": False,
                        "differs_from_done": ["operation.diff"],
                    },
                ],
            },
            (),
        ),
        (
            "r10_reasoning",
            base(terminal_enc="e2"),
            {"done_vs_terminal": {**equal, "reasoning": {"equal": 0, "different": 1, "keys": ["encrypted_content"]}}},
            (),
        ),
    ]
    failed = 0
    for name, events, want, breaks in cases:
        got = measure(sse(events))
        for field, value in want.items():
            if got.get(field) != value:
                failed += 1
                print(f"FAIL {name}.{field}: got {json.dumps(got.get(field))}, want {json.dumps(value)}")
        for rule in RULES:
            if bool(got["rules"][rule]) != (rule in breaks):
                failed += 1
                print(f"FAIL {name}.rules.{rule}: got {json.dumps(got['rules'][rule])}, want {'a break' if rule in breaks else 'no break'}")
    # A comment-only block (a heartbeat) between two JSON events is not a record.
    plain = sse(base())
    first_end = plain.index(b"\n\n") + 2
    try:
        with_ping = measure(plain[:first_end] + b": ping\n\n" + plain[first_end:])
    except ValueError as e:
        with_ping = f"raised {e!r}"
    if with_ping != measure(plain):
        failed += 1
        print(f"FAIL comment_only_block: got {json.dumps(with_ping)[:200]}, want the measure of the same stream without it")
    print(f"self-test: {len(cases) + 1} cases, " + ("ok" if not failed else f"{failed} field(s) wrong"))
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    if len(argv) != 1:
        print("usage: measure.py <dir> | --self-test", file=sys.stderr)
        return 2
    for path in sorted(pathlib.Path(argv[0]).glob("*.sse")):
        print(json.dumps({"file": path.name, **measure(path.read_bytes())}))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
