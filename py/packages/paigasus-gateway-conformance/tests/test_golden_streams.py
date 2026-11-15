# SPDX-License-Identifier: Apache-2.0
"""The gateway's golden tool-call streams through the real SDK stream helpers (SMA-749 § 8.2).

The goldens carry `: paigasus-decision` SSE comment lines. The SDK decoders must ignore them as comments, so
nothing here strips them: a decoder that tripped on one would fail these tests.
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import NotRequired, TypedDict, cast

import anthropic
import httpx2
import openai
import pytest
from openai.types.responses import (
    Response,
    ResponseCompletedEvent,
    ResponseFailedEvent,
    ResponseIncompleteEvent,
    ResponseOutputItem,
    ResponseOutputMessage,
    ResponseOutputText,
)

GOLDEN = Path(__file__).resolve().parents[4] / "rs" / "crates" / "libs" / "paigasus-gateway-core" / "tests" / "golden"


class Allowed(TypedDict):
    name: str
    arguments: str
    choice: int
    position: NotRequired[int]
    id: NotRequired[str]
    namespace: NotRequired[str | None]


class Case(TypedDict):
    file: str
    format: str
    allowed: list[Allowed]
    notices: list[str]
    notice_choices: list[int]
    notice_positions: NotRequired[list[int]]
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


def test_manifest_lists_ten_goldens() -> None:
    assert len(MANIFEST) == 10
    for case in MANIFEST:
        assert (GOLDEN / case["file"]).is_file()
        assert len(case["notices"]) == len(case["notice_choices"])
        if case["format"] == "responses":
            assert "notice_positions" in case
            assert len(case["notices"]) == len(case["notice_positions"])


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


def _responses_client(body: bytes) -> openai.OpenAI:
    return openai.OpenAI(api_key="x", base_url="http://gateway.test/v1", http_client=httpx2.Client(transport=_transport(body)))


def _client_run_calls(items: list[tuple[int, ResponseOutputItem]]) -> list[tuple[int, str, str, str]]:
    """The (position, call_id, name, arguments) of every item the client would run.

    The manifest `allowed[].id` is the `call_id` (`call_0`), not the item id (`fc_0`).
    """
    calls: list[tuple[int, str, str, str]] = []
    for position, item in items:
        # Only function calls may reach the client; any other tool-like item type is a leak.
        assert item.type in {"function_call", "message", "reasoning"}, item
        if item.type == "function_call":
            calls.append((position, item.call_id, item.name, item.arguments))
    return sorted(calls)


def _expected_call(allowed: Allowed) -> tuple[int, str, str, str]:
    assert "position" in allowed
    assert "id" in allowed
    return (allowed["position"], allowed["id"], allowed["name"], allowed["arguments"])


def _expected_calls(case: Case) -> list[tuple[int, str, str, str]]:
    return sorted(_expected_call(a) for a in case["allowed"])


def _message_text(item: ResponseOutputMessage) -> str:
    return "".join(part.text for part in item.content if isinstance(part, ResponseOutputText))


@pytest.mark.parametrize("case", _cases("responses"), ids=_case_id)
def test_responses_stream_helper_sees_only_allowed_calls(case: Case) -> None:
    body = (GOLDEN / case["file"]).read_bytes()
    terminal: Response | None = None
    with _responses_client(body).responses.stream(model="m", input="hi") as stream:
        # Iterate so the helper accumulates every event. In openai-python 3.48.0 the helper keeps a final response
        # only for `response.completed`, so `get_final_response()` raises for an incomplete or failed stream, as it
        # does for real OpenAI traffic. The gateway must behave the same way, so the raise is pinned here.
        for event in stream:
            if isinstance(event, ResponseCompletedEvent | ResponseIncompleteEvent | ResponseFailedEvent):
                terminal = cast("Response", event.response)
        assert terminal is not None
        if terminal.status == "completed":
            assert stream.get_final_response().status == "completed"
        else:
            with pytest.raises(RuntimeError, match=re.escape("response.completed")):
                _ = stream.get_final_response()
    final = terminal
    output = list(enumerate(final.output))
    assert _client_run_calls(output) == _expected_calls(case)
    assert "notice_positions" in case
    for notice, position in zip(case["notices"], case["notice_positions"], strict=True):
        item = final.output[position]
        assert isinstance(item, ResponseOutputMessage)
        assert item.id == f"msg_paigasus_{position}"
        assert _message_text(item) == notice
    assert final.status == case["finish_reasons"]["0"]
    assert "CANARY" not in body.decode()


@pytest.mark.parametrize("case", _cases("responses"), ids=_case_id)
def test_responses_raw_stream_sees_only_allowed_calls(case: Case) -> None:
    body = (GOLDEN / case["file"]).read_bytes()
    done: list[tuple[int, ResponseOutputItem]] = []
    terminal: Response | None = None
    # The repeated `sequence_number` of spec 6.5 must raise nothing in the SDK decoder.
    for event in _responses_client(body).responses.create(model="m", input="hi", stream=True):
        if event.type == "response.output_item.done":
            done.append((event.output_index, event.item))
        elif isinstance(event, ResponseCompletedEvent | ResponseIncompleteEvent | ResponseFailedEvent):
            terminal = event.response
    assert terminal is not None
    assert _client_run_calls(done) == _expected_calls(case)
    assert _client_run_calls(list(enumerate(terminal.output))) == _expected_calls(case)
    assert terminal.status == case["finish_reasons"]["0"]
    assert "CANARY" not in body.decode()
