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


def without_commands(item: dict) -> dict:
    out = without(item, "status")
    if isinstance(out.get("action"), dict):
        out["action"] = without(out["action"], "commands")
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
            items[j] = {"added": d.get("item", {}), "done": None, "deltas": [], "done_value": None}
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
        elif kind == "shell_call":
            s["commands_at_added"] = (it["added"].get("action") or {}).get("commands")
            if done is not None:
                s["added_equals_done_without_status_and_commands"] = without_commands(it["added"]) == without_commands(done)
        elif done is not None:
            s["added_equals_done_without_status"] = without(it["added"], "status") == without(done, "status")
        shapes.append(s)

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
        },
        "obfuscation": any("obfuscation" in d for _, _, d in events),
        "wire_output_text": None if terminal is None else "output_text" in (terminal.get("response") or {}),
        "keepalive": {"count": len(keep), "positions": keep},
        "contiguous": {"holds": first_break is None, "first_break": first_break},
        "index_stable": {"holds": index_break is None, "first_break": index_break},
        "added_shapes": shapes,
        "done_status": statuses,
        "max_record_bytes": dict(sorted(sizes.items())),
    }


def sse(events: list[dict], done: bool = True) -> bytes:
    parts = [f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events]
    if done:
        parts.append("data: [DONE]\n\n")
    return "".join(parts).encode()


def seqd(events: list[dict]) -> list[dict]:
    return [{**e, "sequence_number": i} for i, e in enumerate(events)]


def fc(i: int, args: str, status: str) -> dict:
    return {"type": "function_call", "id": f"fc_{i}", "call_id": f"call_{i}", "name": "f", "arguments": args, "status": status}


def self_test() -> int:
    resp = {"id": "resp_1", "output": []}
    conforming = seqd(
        [
            {"type": "response.created", "response": resp},
            {"type": "response.output_item.added", "output_index": 0, "item": fc(0, "", "in_progress")},
            {"type": "response.function_call_arguments.delta", "output_index": 0, "item_id": "fc_0", "delta": "{}"},
            {"type": "response.function_call_arguments.done", "output_index": 0, "item_id": "fc_0", "arguments": "{}"},
            {"type": "response.output_item.done", "output_index": 0, "item": fc(0, "{}", "completed")},
            {"type": "response.completed", "response": {"id": "resp_1", "output": [fc(0, "{}", "completed")]}},
        ]
    )
    interleaved = seqd(
        [
            {"type": "response.created", "response": resp},
            {"type": "response.output_item.added", "output_index": 0, "item": fc(0, "", "in_progress")},
            {"type": "response.output_item.added", "output_index": 1, "item": fc(1, "", "in_progress")},
            {"type": "response.function_call_arguments.delta", "output_index": 0, "item_id": "fc_0", "delta": "{}"},
            {"type": "response.function_call_arguments.done", "output_index": 0, "item_id": "fc_0", "arguments": "{}"},
            {"type": "response.output_item.done", "output_index": 0, "item": fc(0, "{}", "completed")},
            {"type": "response.output_item.done", "output_index": 1, "item": fc(1, "", "completed")},
            {"type": "response.completed", "response": {"id": "resp_1", "output": []}},
        ]
    )
    cases = [
        (
            "conforming",
            measure(sse(conforming)),
            {
                "event_lines": {"present": "all", "equal_to_data_type": True},
                "done_marker": {"present": True, "position": 6, "last": True},
                "sequence_number": {"present": "all", "strictly_increasing": True},
                "obfuscation": False,
                "wire_output_text": False,
                "keepalive": {"count": 0, "positions": []},
                "contiguous": {"holds": True, "first_break": None},
                "index_stable": {"holds": True, "first_break": None},
                "added_shapes": [{"index": 0, "type": "function_call", "arguments_at_added": "", "deltas_equal_done": True}],
                "done_status": [{"index": 0, "type": "function_call", "status": "completed"}],
            },
        ),
        (
            "interleaved",
            measure(sse(interleaved)),
            {
                "contiguous": {"holds": False, "first_break": 2},
                "index_stable": {"holds": True, "first_break": None},
                "done_status": [
                    {"index": 0, "type": "function_call", "status": "completed"},
                    {"index": 1, "type": "function_call", "status": "completed"},
                ],
            },
        ),
    ]
    failed = 0
    for name, got, want in cases:
        for field, value in want.items():
            if got.get(field) != value:
                failed += 1
                print(f"FAIL {name}.{field}: got {json.dumps(got.get(field))}, want {json.dumps(value)}")
    print("self-test: " + ("ok" if not failed else f"{failed} field(s) wrong"))
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
