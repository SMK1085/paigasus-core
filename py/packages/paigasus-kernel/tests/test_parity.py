# SPDX-License-Identifier: Apache-2.0
"""Cross-binding parity: the PyO3 wheel must reproduce the kernel-computed corpora (SMA-433/448).

Corpora are generated once from the Rust kernel (the single oracle) and committed under the
`paigasus-kernel-parity` crate; here we replay them through the bindings.
"""

import json
from pathlib import Path
from typing import TypedDict, cast

import pytest
from paigasus_kernel import (
    mint_uuid7,
    prn_build,
    prn_canonicalize,
    prn_cedar_entity_id,
    prn_cedar_entity_type,
    prn_error_kind,
    prn_org,
    prn_region,
    prn_resource_id,
    prn_resource_type,
    prn_service,
    sum_as_string,
)

# From this file: tests -> paigasus-kernel -> packages -> py -> repo root == parents[4].
VECTORS = Path(__file__).resolve().parents[4] / "rs/crates/libs/paigasus-kernel-parity/vectors"


class SumCase(TypedDict):
    a: int
    b: int
    expected: int


class Uuid7Case(TypedDict):
    unix_ms: float
    rand_hex: str
    expected_uuid: str


class PrnCanonicalCase(TypedDict):
    input: str
    error_kind: str
    canonical: str


class PrnCedarCase(TypedDict):
    prn: str
    entity_type: str
    entity_id: str


class PrnFieldsCase(TypedDict):
    prn: str
    service: str
    region: str
    org: str
    resource_type: str
    resource_id: str


def _read(name: str) -> object:
    return cast("object", json.loads((VECTORS / f"{name}.json").read_text()))


SUM_CASES = cast("list[SumCase]", _read("sum"))
UUID7_CASES = cast("list[Uuid7Case]", _read("uuid7"))
PRN_CANONICAL_CASES = cast("list[PrnCanonicalCase]", _read("prn_canonical"))
PRN_CEDAR_CASES = cast("list[PrnCedarCase]", _read("prn_cedar"))
PRN_FIELDS_CASES = cast("list[PrnFieldsCase]", _read("prn_fields"))


def test_corpora_present_and_non_empty() -> None:
    # Integrity guard: a wrong path / empty corpus must fail RED (an empty parametrize set is a
    # *skipped* test — a green run that compared nothing).
    corpora: list[tuple[str, int]] = [
        ("sum", len(SUM_CASES)),
        ("uuid7", len(UUID7_CASES)),
        ("prn_canonical", len(PRN_CANONICAL_CASES)),
        ("prn_cedar", len(PRN_CEDAR_CASES)),
        ("prn_fields", len(PRN_FIELDS_CASES)),
    ]
    for name, count in corpora:
        assert count > 0, f"{name} corpus is empty"


@pytest.mark.parametrize("case", SUM_CASES, ids=[f"{c['a']}+{c['b']}" for c in SUM_CASES])
def test_sum_matches_corpus(case: SumCase) -> None:
    assert sum_as_string(case["a"], case["b"]) == str(case["expected"])


@pytest.mark.parametrize("case", UUID7_CASES, ids=[f"{c['unix_ms']}/{c['rand_hex']}" for c in UUID7_CASES])
def test_mint_uuid7_matches_corpus(case: Uuid7Case) -> None:
    assert mint_uuid7(case["unix_ms"], case["rand_hex"]) == case["expected_uuid"]


@pytest.mark.parametrize("case", PRN_CANONICAL_CASES, ids=[c["input"] or "<empty>" for c in PRN_CANONICAL_CASES])
def test_prn_canonical_matches_corpus(case: PrnCanonicalCase) -> None:
    assert prn_error_kind(case["input"]) == case["error_kind"]
    if case["error_kind"] == "":
        assert prn_canonicalize(case["input"]) == case["canonical"]


@pytest.mark.parametrize("case", PRN_CEDAR_CASES, ids=[c["prn"] for c in PRN_CEDAR_CASES])
def test_prn_cedar_matches_corpus(case: PrnCedarCase) -> None:
    assert prn_cedar_entity_type(case["prn"]) == case["entity_type"]
    assert prn_cedar_entity_id(case["prn"]) == case["entity_id"]


@pytest.mark.parametrize("case", PRN_FIELDS_CASES, ids=[c["prn"] for c in PRN_FIELDS_CASES])
def test_prn_fields_matches_corpus(case: PrnFieldsCase) -> None:
    assert prn_service(case["prn"]) == case["service"]
    assert prn_region(case["prn"]) == case["region"]
    assert prn_org(case["prn"]) == case["org"]
    assert prn_resource_type(case["prn"]) == case["resource_type"]
    assert prn_resource_id(case["prn"]) == case["resource_id"]
    assert (
        prn_build(
            case["service"],
            case["region"],
            case["org"],
            case["resource_type"],
            case["resource_id"],
        )
        == case["prn"]
    )
