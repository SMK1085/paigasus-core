# SPDX-License-Identifier: Apache-2.0
"""Measures recorded Anthropic Messages streams (SMA-750 spec § 10.1, D15, D18).

Only the standard library is used.

    python3 -I measure.py <dir>          # one JSON object per `*.sse` file in <dir>
    python3 -I measure.py --self-test    # one built-in stream; exit 1 when a field is wrong

A record is the text between two blank lines. `max_record_bytes` counts the whole record (the
`event:` line and the `data:` line) in bytes, without the blank line that ends it. Seconds come
from `<name>.timing.json`: the arrival time of the network chunk that holds the end of a record.
"""

import json
import pathlib
import sys


def records(raw: bytes) -> list:
    """Splits SSE bytes into records: {"event", "data", "bytes", "end"} (`end` is a byte offset)."""
    out = []
    pos = 0
    for block in raw.replace(b"\r\n", b"\n").split(b"\n\n"):
        end = pos + len(block) + 2
        pos = end
        text = block.decode("utf-8").strip("\n")
        if not text:
            continue
        event, data = None, []
        for line in text.split("\n"):
            if line.startswith("event:"):
                event = line[6:].strip()
            elif line.startswith("data:"):
                data.append(line[5:].lstrip())
        try:
            value = json.loads("\n".join(data)) if data else None
        except ValueError:
            value = None
        out.append({"event": event, "data": value, "bytes": len(block), "end": min(end, len(raw))})
    return out


def chunk_time(timing, end: int):
    """Arrival time of the first chunk whose cumulative end offset covers `end`."""
    if not timing or not timing["chunks"]:
        return None
    for c in timing["chunks"]:
        if c["end"] >= end:
            return c["t"]
    return timing["chunks"][-1]["t"]


def measure(raw: bytes, timing=None) -> dict:
    recs = records(raw)
    event_types, block_types, delta_types = set(), set(), set()
    max_by_event = {}
    max_start = 0
    blocks = {}  # index -> {"type", "bytes", "start_t"}
    tool_uses = []
    for r in recs:
        d = r["data"] or {}
        ev = r["event"] or d.get("type")
        event_types.add(ev)
        max_by_event[ev] = max(max_by_event.get(ev, 0), r["bytes"])
        if ev == "content_block_start":
            cb = d.get("content_block", {})
            block_types.add(cb.get("type"))
            max_start = max(max_start, r["bytes"])
            blocks[d.get("index")] = {"type": cb.get("type"), "bytes": 0, "start_t": chunk_time(timing, r["end"])}
        elif ev == "content_block_delta":
            delta = d.get("delta", {})
            delta_types.add(delta.get("type"))
            b = blocks.get(d.get("index"))
            if b is not None and delta.get("type") == "input_json_delta":
                b["bytes"] += len(delta.get("partial_json", "").encode("utf-8"))
        elif ev == "content_block_stop":
            b = blocks.get(d.get("index"))
            if b is not None and b["type"] == "tool_use":
                stop_t = chunk_time(timing, r["end"])
                secs = None if b["start_t"] is None or stop_t is None else round(stop_t - b["start_t"], 3)
                tool_uses.append({"index": d.get("index"), "input_bytes": b["bytes"], "seconds": secs})
    return {
        "event_types": sorted(t for t in event_types if t),
        "block_types": sorted(t for t in block_types if t),
        "delta_types": sorted(t for t in delta_types if t),
        "max_record_bytes": dict(sorted((k, v) for k, v in max_by_event.items() if k)),
        "max_content_block_start_bytes": max_start,
        "tool_use": tool_uses,
        "total_bytes": len(raw),
    }


SELF = (
    b'event: message_start\ndata: {"type":"message_start","message":{"id":"m","content":[]}}\n\n'
    b'event: content_block_start\ndata: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"w","input":{}}}\n\n'
    b'event: content_block_delta\ndata: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\\"a\\":"}}\n\n'
    b'event: content_block_delta\ndata: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"1}"}}\n\n'
    b'event: content_block_stop\ndata: {"type":"content_block_stop","index":0}\n\n'
    b'event: message_stop\ndata: {"type":"message_stop"}\n\n'
)


def self_test() -> int:
    first = SELF.index(b"event: content_block_delta")
    timing = {"chunks": [{"t": 0.1, "end": first}, {"t": 2.1, "end": len(SELF)}]}
    m = measure(SELF, timing)
    want = {
        "event_types": ["content_block_delta", "content_block_start", "content_block_stop", "message_start", "message_stop"],
        "block_types": ["tool_use"],
        "delta_types": ["input_json_delta"],
        "tool_use": [{"index": 0, "input_bytes": 7, "seconds": 2.0}],
    }
    bad = [k for k, v in want.items() if m[k] != v]
    if m["max_content_block_start_bytes"] <= 0 or m["max_record_bytes"].get("message_stop", 0) <= 0:
        bad.append("sizes")
    if bad:
        print("self-test failed:", bad, json.dumps(m), file=sys.stderr)
        return 1
    print("self-test ok")
    return 0


def main(argv: list) -> int:
    if argv == ["--self-test"]:
        return self_test()
    if len(argv) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    d = pathlib.Path(argv[0])
    for f in sorted(d.glob("*.sse")):
        tf = f.with_suffix(".timing.json")
        timing = json.loads(tf.read_text()) if tf.exists() else None
        print(json.dumps({"file": f.name, **measure(f.read_bytes(), timing)}))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
