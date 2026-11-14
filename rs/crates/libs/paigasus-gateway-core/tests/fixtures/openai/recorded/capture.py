# SPDX-License-Identifier: Apache-2.0
"""Records real OpenAI Chat Completions streams with tool calls (SMA-757 spec § 2.4, § 7.3).

The key is read from a file and is never printed. Only the standard library is used.

    python3 -I capture.py --key-file ~/.config/paigasus/openai.key --list-models
    python3 -I capture.py --key-file ~/.config/paigasus/openai.key --model <id> --runs 5 --out <dir>
    python3 -I capture.py --key-file ~/.config/paigasus/openai.key --model gpt-6.1-terra \
        --reasoning-effort none --runs 5 --out <dir>

Each run writes the raw response bytes to `<dir>/<model>__<scenario>__<n>.sse`, unchanged. The
repository keeps 12 of the 60 streams of 2026-11-14: one for each model and scenario.
A rejected request writes `<model>__<scenario>__<n>.error.json` with the status and the body.
"""

import argparse
import json
import pathlib
import sys
import urllib.error
import urllib.request

API = "https://api.openai.com/v1"

FUNCTIONS = [
    {
        "name": "read_file",
        "description": "Read a file from the workspace.",
        "parameters": {
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
        },
    },
    {
        "name": "get_weather",
        "description": "Get the weather for a city.",
        "parameters": {
            "type": "object",
            "properties": {"city": {"type": "string"}, "unit": {"type": "string", "enum": ["c", "f"]}},
            "required": ["city", "unit"],
        },
    },
]

PARALLEL = (
    "Call get_weather for Paris, for Tokyo and for Lima (unit c), and read_file for "
    "/workspace/a.txt. Make all four calls now, in one response."
)

SCENARIOS = {
    # Parallel function calls in one response.
    "parallel_functions": {
        "messages": [{"role": "user", "content": PARALLEL}],
        "tools": [{"type": "function", "function": f} for f in FUNCTIONS],
        "tool_choice": "required",
        "parallel_tool_calls": True,
    },
    # A custom (free-form) tool next to function tools.
    "custom_and_function": {
        "messages": [
            {
                "role": "user",
                "content": "Call get_weather for Paris (unit c) and also call run_shell with the text "
                "'ls -la /workspace'. Make both calls now, in one response.",
            }
        ],
        "tools": [{"type": "function", "function": f} for f in FUNCTIONS]
        + [{"type": "custom", "custom": {"name": "run_shell", "description": "Run a shell command."}}],
        "tool_choice": "required",
        "parallel_tool_calls": True,
    },
    # The custom tool, forced: a custom call in the stream.
    "custom_forced": {
        "messages": [{"role": "user", "content": "List the files in /workspace."}],
        "tools": [{"type": "custom", "custom": {"name": "run_shell", "description": "Run a shell command."}}],
        "tool_choice": {"type": "custom", "custom": {"name": "run_shell"}},
    },
    # One function tool and one custom tool only, so that both kinds can be in one response.
    "custom_and_one_function": {
        "messages": [
            {
                "role": "user",
                "content": "Do two things now, in one response, with two tool calls: call get_weather for "
                "Paris (unit c), and call the run_shell tool with the input 'ls -la /workspace'.",
            }
        ],
        "tools": [{"type": "function", "function": FUNCTIONS[1]}]
        + [{"type": "custom", "custom": {"name": "run_shell", "description": "Run a shell command."}}],
        "tool_choice": "required",
        "parallel_tool_calls": True,
    },
    # The deprecated `functions` field (a legacy `function_call` in the stream).
    "legacy_functions": {
        "messages": [{"role": "user", "content": "Read the file /workspace/a.txt."}],
        "functions": FUNCTIONS,
        "function_call": {"name": "read_file"},
    },
}


def request(key: str, method: str, path: str, body: dict | None = None) -> urllib.request.Request:
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(API + path, data=data, method=method)
    req.add_header("Authorization", "Bearer " + key)
    req.add_header("User-Agent", "sse-capture/1")
    if data is not None:
        req.add_header("Content-Type", "application/json")
    return req


def list_models(key: str) -> None:
    with urllib.request.urlopen(request(key, "GET", "/models"), timeout=60) as r:
        ids = sorted(m["id"] for m in json.load(r)["data"])
    for i in ids:
        if i.startswith(("gpt-", "o")):
            print(i)


def capture(key: str, model: str, runs: int, out: pathlib.Path, only: list[str], effort: str | None) -> None:
    out.mkdir(parents=True, exist_ok=True)
    safe = model.replace("/", "_")
    for name, scenario in SCENARIOS.items():
        if only and name not in only:
            continue
        for n in range(1, runs + 1):
            body = {"model": model, "stream": True, "stream_options": {"include_usage": True}, **scenario}
            if effort:
                # gpt-6.x accepts function tools on Chat Completions only with reasoning_effort "none".
                body["reasoning_effort"] = effort
            stem = f"{safe}__{name}__{n}"
            try:
                with urllib.request.urlopen(request(key, "POST", "/chat/completions", body), timeout=300) as r:
                    raw = r.read()
                (out / f"{stem}.sse").write_bytes(raw)
                # One current result per stem: drop a stale error from an earlier run.
                (out / f"{stem}.error.json").unlink(missing_ok=True)
                print(f"{stem}.sse {len(raw)} bytes")
            except urllib.error.HTTPError as e:
                err = {"status": e.code, "body": e.read().decode("utf-8", "replace")}
                (out / f"{stem}.error.json").write_text(json.dumps(err, indent=2) + "\n")
                (out / f"{stem}.sse").unlink(missing_ok=True)
                print(f"{stem}: HTTP {e.code}")


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--key-file", required=True, type=pathlib.Path)
    p.add_argument("--list-models", action="store_true")
    p.add_argument("--model")
    p.add_argument("--runs", type=int, default=5)
    p.add_argument("--out", type=pathlib.Path, default=pathlib.Path("."))
    p.add_argument("--only", nargs="*", default=[], choices=sorted(SCENARIOS))
    p.add_argument("--reasoning-effort")
    a = p.parse_args()
    key = a.key_file.expanduser().read_text().strip()
    if not key:
        print("the key file is empty", file=sys.stderr)
        return 2
    if a.list_models:
        list_models(key)
        return 0
    if not a.model:
        print("--model is required", file=sys.stderr)
        return 2
    capture(key, a.model, a.runs, a.out, a.only, a.reasoning_effort)
    return 0


if __name__ == "__main__":
    sys.exit(main())
