# SPDX-License-Identifier: Apache-2.0
"""Records real OpenAI streams with tool calls (SMA-757 spec § 2.4, § 7.3; SMA-769 spec § 10.5).

The key is read from a file and is never printed. Only the standard library is used.

    python3 -I capture.py --key-file ~/.config/paigasus/openai.key --list-models
    python3 -I capture.py --key-file ~/.config/paigasus/openai.key --model <id> --runs 5 --out <dir>
    python3 -I capture.py --key-file ~/.config/paigasus/openai.key --model gpt-6.1-terra \
        --reasoning-effort none --runs 5 --out <dir>
    python3 -I capture.py --api responses --key-file ~/.config/paigasus/openai.key \
        --model gpt-6.1-sol --runs 3 --out <dir>
    python3 -I capture.py --api responses --dry-run --model gpt-6.1-sol --runs 1 --out <dir>

`--api chat` (the default) POSTs to `/chat/completions`. `--api responses` POSTs to `/responses`.
Each run writes the raw response bytes to `<dir>/<model>__<scenario>__<n>.sse`, unchanged.
A rejected request writes `<model>__<scenario>__<n>.error.json` with the status and the body.

The two `m*` Responses scenarios send more than one request. The first one is not streamed and
makes a function call. The next ones are the continuations. The run writes
`<model>__<scenario>__<n>.measure.json` with each HTTP status and error body (never a header).

`--dry-run` writes each first request body to `<dir>/<stem>.request.json`, sends nothing, and
does not read `--key-file`. It prints the number of files that it wrote.
"""

import argparse
import copy
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


# --- Responses API (SMA-769 spec § 10.5) ---------------------------------------------------------


def flat_function(f: dict) -> dict:
    """A flat Responses function tool. `strict` is explicit, and the schema obeys strict mode."""
    params = {**f["parameters"], "additionalProperties": False}
    return {
        "type": "function",
        "name": f["name"],
        "description": f["description"],
        "parameters": params,
        "strict": True,
    }


R_FUNCTIONS = [flat_function(f) for f in FUNCTIONS]

R_WRITE_FILE = flat_function(
    {
        "name": "write_file",
        "description": "Write a file to the workspace.",
        "parameters": {
            "type": "object",
            "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
            "required": ["path", "content"],
        },
    }
)

R_RUN_SHELL = {"type": "custom", "name": "run_shell", "description": "Run a shell command."}

# About 50 KB of instructions (M7): 500 lines of exactly 100 bytes each.
LONG_INSTRUCTIONS = "".join(
    f"Rule {i:03d}: answer in plain words, keep each answer short, and call a tool when it helps.".ljust(99)
    + "\n"
    for i in range(500)
)

THIRTY_TOOLS = [
    flat_function(
        {
            "name": f"tool_{i:02d}",
            "description": f"Tool number {i}. It takes one text value and one count.",
            "parameters": {
                "type": "object",
                "properties": {"x": {"type": "string"}, "count": {"type": "integer"}},
                "required": ["x", "count"],
            },
        }
    )
    for i in range(30)
]


def user(text: str) -> list:
    return [{"role": "user", "content": text}]


RESPONSES_SCENARIOS = {
    "parallel_functions": {
        "input": user(PARALLEL),
        "tools": R_FUNCTIONS,
        "tool_choice": "required",
        "parallel_tool_calls": True,
    },
    "custom_and_function": {
        "input": user(
            "Call get_weather for Paris (unit c) and also call run_shell with the text "
            "'ls -la /workspace'. Make both calls now, in one response."
        ),
        "tools": [*R_FUNCTIONS, R_RUN_SHELL],
        "tool_choice": "required",
        "parallel_tool_calls": True,
    },
    "custom_forced": {
        "input": user("List the files in /workspace."),
        "tools": [R_RUN_SHELL],
        "tool_choice": {"type": "custom", "name": "run_shell"},
    },
    "reasoning_then_call": {
        "input": user(
            "Find which of Paris, Tokyo and Lima is in the southern hemisphere. Think about it, "
            "then call get_weather for that city only (unit c)."
        ),
        "tools": R_FUNCTIONS,
        "tool_choice": "auto",
        "reasoning": {"summary": "auto"},
    },
    "text_only": {
        "input": user("Write one sentence about the sea."),
    },
    "structured_output": {
        "input": user("Give the capital and the population of France."),
        "text": {
            "format": {
                "type": "json_schema",
                "name": "country",
                "strict": True,
                "schema": {
                    "type": "object",
                    "properties": {"capital": {"type": "string"}, "population": {"type": "integer"}},
                    "required": ["capital", "population"],
                    "additionalProperties": False,
                },
            }
        },
    },
    "web_search_then_function": {
        "input": user(
            "First search the web for the weather in Paris today, then search the web for the "
            "population of Tokyo. After the two searches, call read_file for /workspace/a.txt."
        ),
        "tools": [{"type": "web_search"}, *R_FUNCTIONS],
        "tool_choice": "auto",
    },
    "max_output_tokens_in_call": {
        "input": user("Call write_file for /workspace/poem.txt with a poem of 40 lines about the sea as the content."),
        "tools": [R_WRITE_FILE],
        "tool_choice": {"type": "function", "name": "write_file"},
        "max_output_tokens": 48,
    },
    "long_instructions_30_tools": {
        "instructions": LONG_INSTRUCTIONS,
        "input": user("Call tool_07 with x 'alpha' and count 3."),
        "tools": THIRTY_TOOLS,
        "tool_choice": "auto",
    },
    "apply_patch_forced": {
        "input": user("Create the file hello.txt with the single line 'hello'."),
        "tools": [{"type": "apply_patch"}],
        "tool_choice": {"type": "apply_patch"},
    },
    "shell_forced": {
        "input": user("List the files in the current directory, then print the date."),
        "tools": [{"type": "shell", "environment": {"type": "local"}}],
        "tool_choice": {"type": "shell"},
    },
    "local_shell_forced": {
        "input": user("List the files in the current directory."),
        "tools": [{"type": "local_shell"}],
        "tool_choice": "required",
    },
    "computer_forced": {
        "input": user("Take a screenshot of the screen."),
        "tools": [{"type": "computer"}],
        "tool_choice": {"type": "computer"},
    },
    # M1 (spec § 3.3): a function call, then `previous_response_id` with no `function_call_output`.
    "m1_unanswered_continuation": {
        "input": user("Read the file /workspace/a.txt."),
        "tools": R_FUNCTIONS,
        "tool_choice": {"type": "function", "name": "read_file"},
        "store": True,
    },
    # M6 (spec § 3.3): replay the output with the call replaced by the § 6.5 completed message.
    # The first call asks for the encrypted reasoning, so that every variant can replay it.
    "m6_rewrite_replay": {
        "input": user("Think about which file holds the answer, then read the file /workspace/a.txt."),
        "tools": R_FUNCTIONS,
        "tool_choice": {"type": "function", "name": "read_file"},
        "store": True,
        "include": ["reasoning.encrypted_content"],
    },
}

TWO_REQUEST = ("m1_unanswered_continuation", "m6_rewrite_replay")

M6_VARIANTS = {
    "store_true": {"store": True},
    "store_false": {"store": False},
    "store_false_encrypted": {"store": False, "include": ["reasoning.encrypted_content"]},
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


def post_json(key: str, body: dict) -> tuple[int, dict | None, str]:
    """POST a non-stream Responses body. Returns (status, parsed body or None, error body)."""
    try:
        with urllib.request.urlopen(request(key, "POST", "/responses", body), timeout=300) as r:
            return r.status, json.load(r), ""
    except urllib.error.HTTPError as e:
        return e.code, None, e.read().decode("utf-8", "replace")


def notice(name: str) -> str:
    return f'[paigasus] tool call "{name}" was blocked (policy).'


def rewritten_output(output: list) -> list:
    """The output with each function call replaced by the § 6.5 completed message."""
    out = []
    for i, item in enumerate(output):
        if item.get("type") != "function_call":
            out.append(item)
            continue
        text = notice(item.get("name", ""))
        out.append(
            {
                "type": "message",
                "id": f"msg_paigasus_{i}",
                "role": "assistant",
                "status": "completed",
                "content": [{"type": "output_text", "text": text, "annotations": [], "logprobs": []}],
            }
        )
    return out


def summary(status: int, parsed: dict | None, err: str) -> dict:
    """Status plus the error body, or the output item types. Never a header, never the key."""
    s: dict = {"status": status}
    if parsed is None:
        s["error_body"] = err
    else:
        s["response_status"] = parsed.get("status")
        s["output_types"] = [o.get("type") for o in parsed.get("output", [])]
    return s


def two_request(key: str, model: str, name: str, scenario: dict) -> dict:
    """`scenario` is the full first body, with `model`."""
    status, first, err = post_json(key, scenario)
    result: dict = {"scenario": name, "model": model, "first": summary(status, first, err)}
    if first is None:
        return result
    output = first.get("output", [])
    if name == "m1_unanswered_continuation":
        second = {
            "model": model,
            "previous_response_id": first["id"],
            "input": user("Go on."),
            "tools": scenario["tools"],
        }
        result["continuation"] = summary(*post_json(key, second))
        return result
    result["first"]["blocked_indexes"] = [i for i, o in enumerate(output) if o.get("type") == "function_call"]
    replay = scenario["input"] + rewritten_output(output) + user("Go on, in one sentence.")
    result["variants"] = {}
    for vname, extra in M6_VARIANTS.items():
        body = {"model": model, "input": copy.deepcopy(replay), "tools": scenario["tools"], **extra}
        result["variants"][vname] = summary(*post_json(key, body))
    return result


def first_body(api: str, name: str, model: str, scenario: dict, effort: str | None) -> dict:
    if api == "chat":
        body = {"model": model, "stream": True, "stream_options": {"include_usage": True}, **scenario}
        if effort:
            # gpt-6.x accepts function tools on Chat Completions only with reasoning_effort "none".
            body["reasoning_effort"] = effort
        return body
    body = {"model": model, **scenario} if name in TWO_REQUEST else {"model": model, "stream": True, **scenario}
    if effort:
        body["reasoning"] = {**body.get("reasoning", {}), "effort": effort}
    return body


def capture(
    key: str | None,
    api: str,
    model: str,
    runs: int,
    out: pathlib.Path,
    only: list[str],
    effort: str | None,
    dry_run: bool,
) -> None:
    out.mkdir(parents=True, exist_ok=True)
    safe = model.replace("/", "_")
    scenarios = SCENARIOS if api == "chat" else RESPONSES_SCENARIOS
    path = "/chat/completions" if api == "chat" else "/responses"
    written = 0
    for name, scenario in scenarios.items():
        if only and name not in only:
            continue
        for n in range(1, runs + 1):
            stem = f"{safe}__{name}__{n}"
            body = first_body(api, name, model, scenario, effort)
            if dry_run:
                (out / f"{stem}.request.json").write_text(json.dumps(body, indent=2) + "\n")
                written += 1
                continue
            if api == "responses" and name in TWO_REQUEST:
                result = two_request(key, model, name, body)
                (out / f"{stem}.measure.json").write_text(json.dumps(result, indent=2) + "\n")
                print(f"{stem}.measure.json")
                continue
            try:
                with urllib.request.urlopen(request(key, "POST", path, body), timeout=300) as r:
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
    if dry_run:
        print(f"dry run: wrote {written} request files to {out}")


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--api", choices=["chat", "responses"], default="chat")
    p.add_argument("--key-file", type=pathlib.Path, help="required for every live mode")
    p.add_argument("--dry-run", action="store_true", help="write the request bodies and send nothing")
    p.add_argument("--list-models", action="store_true")
    p.add_argument("--model")
    p.add_argument("--runs", type=int, default=5)
    p.add_argument("--out", type=pathlib.Path, default=pathlib.Path("."))
    p.add_argument(
        "--only",
        nargs="*",
        default=[],
        help=f"chat: {', '.join(SCENARIOS)}. responses: {', '.join(RESPONSES_SCENARIOS)}",
    )
    p.add_argument("--reasoning-effort")
    a = p.parse_args()
    valid = SCENARIOS if a.api == "chat" else RESPONSES_SCENARIOS
    bad = [s for s in a.only if s not in valid]
    if bad:
        print(f"unknown scenario for --api {a.api}: {', '.join(bad)}", file=sys.stderr)
        print(f"valid choices: {', '.join(valid)}", file=sys.stderr)
        return 2
    if a.dry_run and a.list_models:
        print("--dry-run sends nothing, so it cannot list the models", file=sys.stderr)
        return 2
    if not a.list_models and not a.model:
        print("--model is required", file=sys.stderr)
        return 2
    key = None
    if not a.dry_run:
        if a.key_file is None:
            print("--key-file is required for every live mode", file=sys.stderr)
            return 2
        key = a.key_file.expanduser().read_text().strip()
        if not key:
            print("the key file is empty", file=sys.stderr)
            return 2
    if a.list_models:
        list_models(key)
        return 0
    capture(key, a.api, a.model, a.runs, a.out, a.only, a.reasoning_effort, a.dry_run)
    return 0


if __name__ == "__main__":
    sys.exit(main())
