# SPDX-License-Identifier: Apache-2.0
"""Cross-binding parity: the PyO3 wheel must reproduce the kernel-computed corpus (SMA-433).

The corpus is generated once from the Rust kernel (the single oracle) and committed under the
`paigasus-kernel-parity` crate; here we replay it through `sum_as_string`. Parity is decoded-value
equality — the PyO3 surface returns a stringified i64, so we compare against `str(expected)`.
"""

import json
from pathlib import Path
from typing import cast

import pytest
from paigasus_kernel import sum_as_string

# Single resolved path constant (the committed corpus lives in the Rust parity crate). From this
# file: tests -> paigasus-kernel -> packages -> py -> repo root == parents[4].
CORPUS_PATH = Path(__file__).resolve().parents[4] / "rs/crates/libs/paigasus-kernel-parity/vectors/sum.json"
CASES: list[dict[str, int]] = cast("list[dict[str, int]]", json.loads(CORPUS_PATH.read_text()))


def test_corpus_is_present_and_non_empty() -> None:
    # Integrity guard: a wrong path / empty corpus must fail RED. An empty `parametrize` set is
    # reported by pytest as a *skipped* test (`got empty parameter set`), i.e. a green run that
    # compared nothing — the worst failure mode for a safety net.
    assert CORPUS_PATH.exists(), f"parity corpus not found at {CORPUS_PATH}"
    assert len(CASES) > 0


@pytest.mark.parametrize("case", CASES, ids=[f"{c['a']}+{c['b']}" for c in CASES])
def test_sum_as_string_matches_corpus(case: dict[str, int]) -> None:
    assert sum_as_string(case["a"], case["b"]) == str(case["expected"])
