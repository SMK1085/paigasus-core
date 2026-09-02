# SPDX-License-Identifier: Apache-2.0
"""Arm 2 of the PyO3 stub-drift gate (SMA-600): the COMPILED module vs the hand-written stub.

repo:pyo3-stub-drift reads source text, so it cannot see what PyO3's macros actually emit — a
`macro_rules!`-generated `#[pyfunction]`, a registration under an alias, a submodule, or a change
in macro behaviour across a PyO3 upgrade. It refuses those shapes rather than guessing. This test
is the other half: it asks the imported module what it really exports. Neither arm can replace the
other, because `__text_signature__` carries parameter names but no TYPES (design doc §4.4, §6).

This lives in paigasus-kernel-py:test because that task already forces a wheel rebuild
(`uv sync --reinstall-package`) and already keys on the stub, both Rust sources and every
manifest — so the check costs no new build and no registry wiring.
"""

import ast
import inspect
import types
from pathlib import Path

import paigasus_py_bindings

# From this file: tests -> paigasus-kernel -> packages -> py -> repo root == parents[4], the same
# derivation test_parity.py uses. The WORKING-TREE stub, deliberately: maturin relocates the file
# to <package>/__init__.pyi on install, and the working tree is the unambiguous source (§1.3).
STUB = (
    Path(__file__).resolve().parents[4]
    / "rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi"
)


def _exported_callables():
    """The module's real exported surface.

    MEASURED (§1.3): `dir()` yields 13 names, not 12 — maturin wraps the extension in a package,
    so the inner module object is itself visible as an attribute. Filtering to callables while
    excluding ModuleType is what makes this 12; without it the test reds on a correct tree.
    """
    out = {}
    for name in dir(paigasus_py_bindings):
        if name.startswith("_"):
            continue
        value = getattr(paigasus_py_bindings, name)
        if isinstance(value, types.ModuleType) or not callable(value):
            continue
        out[name] = value
    return out


def _stub_defs():
    tree = ast.parse(STUB.read_text(), filename=str(STUB))
    return {n.name: [a.arg for a in n.args.args] for n in tree.body if isinstance(n, ast.FunctionDef)}


def test_stub_exists():
    assert STUB.is_file(), f"{STUB} is missing — the stub moved and this test would assert nothing"


def test_exported_names_match_the_stub():
    exported = set(_exported_callables())
    stubbed = set(_stub_defs())
    assert exported, "the module exported no callables — the import or the filter is wrong"
    assert exported == stubbed, (
        f"exported-but-unstubbed: {sorted(exported - stubbed)}; "
        f"stubbed-but-not-exported: {sorted(stubbed - exported)}"
    )


def test_parameter_names_and_order_match_the_stub():
    stub = _stub_defs()
    drift = {}
    for name, fn in sorted(_exported_callables().items()):
        if name not in stub:
            continue  # reported by the test above; do not double-report
        live = list(inspect.signature(fn).parameters)
        if live != stub[name]:
            drift[name] = (live, stub[name])
    assert not drift, f"parameter names/order drift (live vs stub): {drift}"
