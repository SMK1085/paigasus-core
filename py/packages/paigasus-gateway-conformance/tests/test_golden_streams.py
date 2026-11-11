# SPDX-License-Identifier: Apache-2.0
"""The gateway's golden tool-call streams through the real SDK stream helpers (SMA-749 § 8.2).

The goldens carry `: paigasus-decision` SSE comment lines. The SDK decoders must ignore them as comments, so
nothing here strips them: a decoder that tripped on one would fail these tests.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import TypedDict, cast

import anthropic
import httpx2
import openai
import pytest

GOLDEN = Path(__file__).resolve().parents[4] / "rs" / "crates" / "libs" / "paigasus-gateway-core" / "tests" / "golden"


class Allowed(TypedDict):
    name: str
    arguments: str
    choice: int


class Case(TypedDict):
    file: str
    format: str
    allowed: list[Allowed]
    notices: list[str]
    notice_choices: list[int]
    finish_reasons: dict[str, str]


MANIFEST = cast("list[Case]", json.loads((GOLDEN / "manifest.json").read_text()))


def _transport(body: bytes) -> httpx2.MockTransport:
    def handler(_request: httpx2.Request) -> httpx2.Response:
        return httpx2.Response(200, headers={"content-type": "text/event-stream"}, content=body)

    return httpx2.MockTransport(handler)


def _cases(fmt: str) -> list[Case]:
    cases = [c for c in MANIFEST if c["format"] == fmt]
    assert cases, f"no {fmt} golden in the manifest"
    return cases


def _case_id(case: object) -> str:
    return cast("Case", case)["file"]


def _compact(raw: str) -> str:
    return json.dumps(json.loads(raw), separators=(",", ":"))


def test_manifest_lists_six_goldens() -> None:
    assert len(MANIFEST) == 6
    for case in MANIFEST:
        assert (GOLDEN / case["file"]).is_file()
        assert len(case["notices"]) == len(case["notice_choices"])


@pytest.mark.parametrize("case", _cases("openai"), ids=_case_id)
def test_openai_stream_helper_sees_only_allowed_calls(case: Case) -> None:
    body = (GOLDEN / case["file"]).read_bytes()
    client = openai.OpenAI(api_key="x", base_url="http://gateway.test/v1", http_client=httpx2.Client(transport=_transport(body)))
    with client.chat.completions.stream(model="m", messages=[{"role": "user", "content": "hi"}]) as stream:
        final = stream.get_final_completion()
    tool_calls = [(ch.index, tc) for ch in final.choices for tc in (ch.message.tool_calls or [])]
    # Every golden call is a function call; a non-function call reaching the client is a leak.
    assert all(tc.type == "function" for _, tc in tool_calls)
    calls = sorted((index, tc.function.name, tc.function.arguments) for index, tc in tool_calls)
    assert calls == sorted((a["choice"], a["name"], a["arguments"]) for a in case["allowed"])
    # `choices` is positional in the SDK snapshot, so position == index here.
    texts = {pos: ch.message.content or "" for pos, ch in enumerate(final.choices)}
    for notice, choice in zip(case["notices"], case["notice_choices"], strict=True):
        assert notice in texts[choice]
    finish = {str(pos): ch.finish_reason for pos, ch in enumerate(final.choices)}
    assert finish == case["finish_reasons"]
    assert "CANARY" not in body.decode()


@pytest.mark.parametrize("case", _cases("anthropic"), ids=_case_id)
def test_anthropic_stream_helper_sees_only_allowed_calls(case: Case) -> None:
    body = (GOLDEN / case["file"]).read_bytes()
    client = anthropic.Anthropic(api_key="x", base_url="http://gateway.test", http_client=httpx2.Client(transport=_transport(body)))
    with client.messages.stream(model="m", max_tokens=16, messages=[{"role": "user", "content": "hi"}]) as stream:
        final = stream.get_final_message()
    calls = sorted((b.name, json.dumps(b.input, separators=(",", ":"))) for b in final.content if b.type == "tool_use")
    assert calls == sorted((a["name"], _compact(a["arguments"])) for a in case["allowed"])
    assert all(a["choice"] == 0 for a in case["allowed"])
    text = "".join(b.text for b in final.content if b.type == "text")
    for notice, choice in zip(case["notices"], case["notice_choices"], strict=True):
        assert choice == 0
        assert notice in text
    assert {"0": final.stop_reason} == case["finish_reasons"]
    assert "CANARY" not in body.decode()
