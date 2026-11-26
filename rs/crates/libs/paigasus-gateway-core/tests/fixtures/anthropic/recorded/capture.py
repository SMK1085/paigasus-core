# SPDX-License-Identifier: Apache-2.0
"""Records real Anthropic Messages answers (SMA-750 spec § 10.1, D15, D18, D21).

The key is read from a file and is never printed. Only the standard library is used.

    python3 -I capture.py --key-file ~/.config/paigasus/anthropic-api-key --list-models
    python3 -I capture.py --key-file ~/.config/paigasus/anthropic-api-key --model <id> \
        --thinking-model <id> --out <dir>
    python3 -I capture.py --dry-run --model m --thinking-model m --out <dir>

Each scenario writes, into `<out>`:

    <scenario>.request.json    the request body (not for a GET)
    <scenario>.sse             a stream answer: the raw response bytes
    <scenario>.timing.json     a stream answer: the arrival time of each network chunk
    <scenario>.body.json       a non-stream answer
    <scenario>.response.json   {"status": N, "headers": {...}}, scrubbed (D21)

A `web_fetch` stream holds a third-party document. The script replaces it with a synthetic body of
the same length (`--synthesize-web-fetch FILE`, also applied on a live run), so no copyrighted
text is written.

D21 scrub, applied before any file is written: only the headers `content-type`, `request-id`,
`retry-after` and `x-should-retry` are kept. The `request-id` value and every top-level
`request_id` body value become `req_paigasus_fixture`. The script exits 1 and writes nothing when
the text it would write holds the Anthropic key prefix.
"""

import argparse
import base64
import concurrent.futures
import json
import pathlib
import re
import sys
import threading
import time
import urllib.error
import urllib.request

API = "https://api.anthropic.com"
VERSION = "2023-06-01"
KEY_PREFIX = "sk-ant" + "-"
FIXED_ID = "req_paigasus_fixture"
KEPT_HEADERS = ("content-type", "request-id", "retry-after", "x-should-retry")
BETA_WEB_FETCH = "web-fetch-2025-09-10"
BETA_STRUCTURED = "structured-outputs-2025-11-13"
BETA_EXAMPLES = "advanced-tool-use-2025-11-20"

TOOLS = [
    {
        "name": "get_weather",
        "description": "Get the weather for a city.",
        "input_schema": {
            "type": "object",
            "properties": {"city": {"type": "string"}, "unit": {"type": "string", "enum": ["c", "f"]}},
            "required": ["city", "unit"],
        },
    },
    {
        "name": "read_file",
        "description": "Read a file from the workspace.",
        "input_schema": {
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
        },
    },
]
WRITE_FILE = {
    "name": "write_file",
    "description": "Write a file to the workspace.",
    "input_schema": {
        "type": "object",
        "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
        "required": ["path", "content"],
    },
}
NAME_SCHEMA = {
    "type": "object",
    "properties": {"Name": {"type": "string"}, "Age": {"type": "integer"}},
    "required": ["Name", "Age"],
    "additionalProperties": False,
}


def user(text: str) -> list:
    return [{"role": "user", "content": text}]


def scenarios(model: str, thinking_model: str) -> dict:
    """Name -> spec. A spec: path, stream, body, and optionally method, beta, group."""
    post = "/v1/messages"
    return {
        "text_stream": dict(path=post, stream=True, body={
            "model": model, "max_tokens": 100, "stream": True,
            "messages": user("Say hello in one short sentence."),
        }),
        "parallel_tool_use": dict(path=post, stream=True, body={
            "model": model, "max_tokens": 600, "stream": True, "tools": TOOLS,
            "tool_choice": {"type": "any"},
            "messages": user(
                "Call get_weather for Paris, for Tokyo and for Lima (unit c), and read_file for "
                "/workspace/a.txt. Make all four calls now, in one response."
            ),
        }),
        "thinking_then_tool_use": dict(path=post, stream=True, body={
            "model": thinking_model, "max_tokens": 4096, "stream": True, "tools": TOOLS,
            "thinking": {"type": "enabled", "budget_tokens": 1024},
            "messages": user(
                "Think about which tool you need, then call get_weather for Oslo (unit c)."
            ),
        }),
        "large_tool_input": dict(path=post, stream=True, body={
            "model": model, "max_tokens": 24000, "stream": True, "tools": [WRITE_FILE],
            "tool_choice": {"type": "tool", "name": "write_file"},
            "messages": user(
                "Call write_file with path /workspace/long.txt. The content must be exactly 1400 "
                "lines. Line N reads 'line NNNN: lorem ipsum dolor sit amet', with NNNN the line "
                "number padded to 4 digits. Write all 1400 lines. Do not stop early."
            ),
        }),
        "server_tool_web_fetch": dict(path=post, stream=True, beta=[BETA_WEB_FETCH], body={
            "model": model, "max_tokens": 400, "stream": True,
            "tools": [{"type": "web_fetch_20250910", "name": "web_fetch", "max_uses": 1}],
            "messages": user(
                "Fetch https://arxiv.org/pdf/1706.03762 with web_fetch and tell me its title in one line."
            ),
        }),
        "server_tool_web_search": dict(path=post, stream=True, fallback_of="server_tool_web_fetch", body={
            "model": model, "max_tokens": 400, "stream": True,
            "tools": [{"type": "web_search_20250305", "name": "web_search", "max_uses": 1}],
            "messages": user("Search the web for the Anthropic API status page and name it in one line."),
        }),
        "max_tokens_in_tool_use": dict(path=post, stream=True, body={
            "model": model, "max_tokens": 60, "stream": True, "tools": [WRITE_FILE],
            "tool_choice": {"type": "tool", "name": "write_file"},
            "messages": user(
                "Call write_file with path /workspace/story.txt and a long story of 2000 words as content."
            ),
        }),
        "non_stream_tool_use": dict(path=post, stream=False, body={
            "model": model, "max_tokens": 300, "tools": TOOLS,
            "tool_choice": {"type": "tool", "name": "get_weather"},
            "messages": user("What is the weather in Paris, in celsius?"),
        }),
        "prompt_too_long": dict(path=post, stream=False, keep_request=False, body={
            "model": model, "max_tokens": 16,
            "messages": user("lorem ipsum dolor sit amet " * 450000),
        }),
        "count_tokens": dict(path="/v1/messages/count_tokens", stream=False, body={
            "model": model, "tools": TOOLS,
            "messages": user("What is the weather in Paris, in celsius?"),
        }),
        "models": dict(method="GET", path="/v1/models?limit=1000", stream=False, body=None),
        "structured_output_output_format": dict(
            path=post, stream=False, beta=[BETA_STRUCTURED], group="structured_output", body={
                "model": model, "max_tokens": 200,
                "output_format": {"type": "json_schema", "schema": NAME_SCHEMA},
                "messages": user("Give a person: the name Ada and the age 36, as JSON."),
            }),
        "structured_output_output_config": dict(path=post, stream=False, group="structured_output", body={
            "model": model, "max_tokens": 200,
            "output_config": {"format": {"type": "json_schema", "schema": NAME_SCHEMA}},
            "messages": user("Give a person: the name Ada and the age 36, as JSON."),
        }),
        "tool_input_examples": dict(path=post, stream=False, beta=[BETA_EXAMPLES], body={
            "model": model, "max_tokens": 300,
            "tools": [{
                "name": "create_user",
                "description": "Create a user.",
                "input_schema": {
                    "type": "object",
                    "properties": {"Name": {"type": "string"}},
                    "required": ["Name"],
                },
                "input_examples": [{"Name": "Ada"}, {"Name": "Grace"}],
            }],
            "tool_choice": {"type": "tool", "name": "create_user"},
            "messages": user("Create a user named Linus."),
        }),
        # A burst: handled by `burst`, not by `run_scenario`.
        "rate_limited": dict(path="/v1/messages/count_tokens", stream=False, burst=True, body={
            "model": model,
            "messages": user("hi"),
        }),
    }


# --- The D21 scrub ---------------------------------------------------------------------------------

REQUEST_ID_RE = re.compile(r'("request_id"\s*:\s*)"[^"]*"')


def scrub_headers(headers) -> dict:
    out = {}
    for name, value in headers:
        low = name.lower()
        if low in KEPT_HEADERS:
            out[low] = FIXED_ID if low == "request-id" else value
    return out


def top_level_request_id(text: str) -> bool:
    try:
        v = json.loads(text)
    except ValueError:
        return False
    return isinstance(v, dict) and "request_id" in v


def scrub_sse(raw: bytes) -> bytes:
    """Replaces a top-level `request_id` in a data line. Every other byte stays."""
    lines = raw.decode("utf-8").split("\n")
    for i, line in enumerate(lines):
        if line.startswith("data:") and top_level_request_id(line[5:].strip()):
            lines[i] = REQUEST_ID_RE.sub(lambda m: m.group(1) + '"' + FIXED_ID + '"', line)
    return "\n".join(lines).encode("utf-8")


def scrub_body(raw: bytes) -> str:
    text = raw.decode("utf-8")
    try:
        v = json.loads(text)
    except ValueError:
        return text
    if isinstance(v, dict) and "request_id" in v:
        v["request_id"] = FIXED_ID
    return json.dumps(v, ensure_ascii=False, indent=2) + "\n"


FILLER = b"% synthetic filler for the paigasus fixture, not a real document\n"


def synthetic_pdf(n_raw: int) -> bytes:
    """A deterministic non-copyrighted body of exactly `n_raw` bytes: a PDF header, then filler."""
    head = b"%PDF-1.5\n"
    body = head + FILLER * (n_raw // len(FILLER) + 1)
    return body[:n_raw]


def synthesize_web_fetch(raw: bytes) -> bytes:
    """Replaces the fetched document and the model text of a `web_fetch` stream (copyright).

    The `source.data` of a `web_fetch_result` becomes the base64 of `synthetic_pdf` with the same
    length. Every letter of a `text_delta` becomes `x`. The record structure and every length stay.
    """
    lines = raw.decode("utf-8").split("\n")
    for i, line in enumerate(lines):
        if not line.startswith("data:"):
            continue
        try:
            v = json.loads(line[5:])
        except ValueError:
            continue
        cb = v.get("content_block") or {}
        if cb.get("type") == "web_fetch_tool_result":
            src = ((cb.get("content") or {}).get("content") or {}).get("source") or {}
            old = src.get("data")
            if isinstance(old, str):
                if len(old) % 4:
                    sys.exit("synthesize: the base64 length is not a multiple of 4")
                new = base64.b64encode(synthetic_pdf(len(old) // 4 * 3)).decode("ascii")
                assert len(new) == len(old)
                lines[i] = line.replace('"data":"' + old + '"', '"data":"' + new + '"', 1)
                if json.loads(lines[i][5:])["content_block"]["content"]["content"]["source"]["data"] == old:
                    sys.exit("synthesize: a web_fetch source data was not replaced (line format differs)")
        delta = v.get("delta") or {}
        if delta.get("type") == "text_delta" and isinstance(delta.get("text"), str):
            masked = re.sub(r"[A-Za-z]", "x", delta["text"])
            lines[i] = line.replace(json.dumps(delta["text"]), json.dumps(masked), 1)
            after = json.loads(lines[i][5:])["delta"]["text"]
            if re.search(r"[A-Za-z]", after.replace("x", "")):
                sys.exit("synthesize: a text_delta still holds letters (line format differs)")
    return "\n".join(lines).encode("utf-8")


def write(path: pathlib.Path, data) -> None:
    raw = data.encode("utf-8") if isinstance(data, str) else data
    if KEY_PREFIX.encode() in raw:
        print(f"refused: {path.name} holds the key prefix; nothing written", file=sys.stderr)
        sys.exit(1)
    path.write_bytes(raw)


# --- HTTP ------------------------------------------------------------------------------------------


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


def request(key: str, spec: dict, body, timeout: float = 900.0):
    """Returns (status, headers, chunks) with chunks = [(seconds, bytes)]."""
    headers = {"x-api-key": key, "anthropic-version": VERSION, "content-type": "application/json"}
    if spec.get("beta"):
        headers["anthropic-beta"] = ",".join(spec["beta"])
    data = None if body is None else json.dumps(body).encode("utf-8")
    req = urllib.request.Request(API + spec["path"], data=data, headers=headers, method=spec.get("method", "POST"))
    start = time.monotonic()
    try:
        resp = OPENER.open(req, timeout=timeout)
    except urllib.error.HTTPError as e:
        resp = e
    chunks = []
    while True:
        part = resp.read1(65536) if hasattr(resp, "read1") else resp.read()
        if not part:
            break
        chunks.append((round(time.monotonic() - start, 3), part))
    return resp.status, list(resp.headers.items()), chunks


def run_scenario(key: str, name: str, spec: dict, out: pathlib.Path) -> dict:
    status, headers, chunks = request(key, spec, spec["body"])
    raw = b"".join(c for _, c in chunks)
    ctype = " ".join(v for k, v in headers if k.lower() == "content-type")
    is_sse = "event-stream" in ctype
    write(out / f"{name}.response.json", json.dumps({"status": status, "headers": scrub_headers(headers)}, indent=2) + "\n")
    if is_sse:
        clean = scrub_sse(raw)
        if name.startswith("server_tool_web_fetch"):
            clean = synthesize_web_fetch(clean)
        if len(clean) != len(raw):
            sys.exit(f"{name}: the scrubbed stream length differs from the raw length; the timing offsets would be wrong")
        write(out / f"{name}.sse", clean)
        end, ends = 0, []
        for t, c in chunks:
            end += len(c)
            ends.append({"t": t, "end": end})
        write(out / f"{name}.timing.json", json.dumps({"chunks": ends}, indent=1) + "\n")
        if b"event: error" in raw:
            write(out / f"mid_stream_error_{name}.sse", clean)
    else:
        write(out / f"{name}.body.json", scrub_body(raw))
    return {"scenario": name, "status": status, "bytes": len(raw), "sse": is_sse}


def burst(key: str, name: str, spec: dict, out: pathlib.Path, limit: int = 200) -> dict:
    """At most `limit` count_tokens calls. Stops at the first 429, which is the kept answer."""
    stop = threading.Event()
    lock = threading.Lock()
    state = {"sent": 0, "hit": None, "last": None}

    def one(_):
        if stop.is_set():
            return
        with lock:
            if state["sent"] >= limit:
                return
            state["sent"] += 1
        status, headers, chunks = request(key, spec, spec["body"], timeout=60)
        raw = b"".join(c for _, c in chunks)
        with lock:
            state["last"] = (status, headers, raw)
            if status == 429 and state["hit"] is None:
                state["hit"] = (status, headers, raw)
                stop.set()

    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        list(pool.map(one, range(limit)))
    status, headers, raw = state["hit"] or state["last"]
    write(out / f"{name}.response.json", json.dumps({"status": status, "headers": scrub_headers(headers)}, indent=2) + "\n")
    write(out / f"{name}.body.json", scrub_body(raw))
    return {"scenario": name, "status": status, "sent": state["sent"], "got_429": state["hit"] is not None}


def list_models(key: str) -> None:
    status, _, chunks = request(key, {"method": "GET", "path": "/v1/models?limit=1000"}, None, timeout=60)
    data = json.loads(b"".join(c for _, c in chunks))
    print("status", status)
    for m in data.get("data", []):
        print(m.get("id"))


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--key-file")
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--list-models", action="store_true")
    p.add_argument("--model")
    p.add_argument("--thinking-model")
    p.add_argument("--only")
    p.add_argument("--out")
    p.add_argument("--synthesize-web-fetch", metavar="SSE", help="rewrite a web_fetch .sse file in place")
    a = p.parse_args()

    if a.synthesize_web_fetch:
        f = pathlib.Path(a.synthesize_web_fetch)
        f.write_bytes(synthesize_web_fetch(f.read_bytes()))
        return 0

    key = None
    if not a.dry_run:
        if not a.key_file:
            p.error("--key-file is required for a live mode")
        key = pathlib.Path(a.key_file).expanduser().read_text().strip()
    if a.list_models:
        list_models(key)
        return 0
    if not (a.model and a.thinking_model and a.out):
        p.error("--model, --thinking-model and --out are required")
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    table = scenarios(a.model, a.thinking_model)
    only = set(a.only.split(",")) if a.only else None

    def wanted(name: str, spec: dict) -> bool:
        return only is None or name in only or spec.get("group") in only

    if a.dry_run:
        for name, spec in table.items():
            if wanted(name, spec) and spec["body"] is not None:
                write(out / f"{name}.request.json", json.dumps(spec["body"]))
        return 0

    for name, spec in table.items():
        if not wanted(name, spec) or spec.get("fallback_of"):
            continue
        if spec["body"] is not None and spec.get("keep_request", True):
            write(out / f"{name}.request.json", json.dumps(spec["body"], indent=2) + "\n")
        res = burst(key, name, spec, out) if spec.get("burst") else run_scenario(key, name, spec, out)
        print(json.dumps(res), flush=True)
        # The web_fetch server tool may be off for the account: then record `web_search`.
        if name == "server_tool_web_fetch" and res["status"] != 200:
            fb = table["server_tool_web_search"]
            write(out / "server_tool_web_search.request.json", json.dumps(fb["body"], indent=2) + "\n")
            print(json.dumps(run_scenario(key, "server_tool_web_search", fb, out)), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
