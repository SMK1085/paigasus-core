# SMA-419 — Wire `paigasus-kernel-py` to the PyO3 wheel (uv↔maturin) + runtime smoke test

**Status:** approved design (brainstorm complete, ready for plan)
**Linear:** [SMA-419](https://linear.app/smaschek/issue/SMA-419/wire-paigasus-kernel-py-to-the-pyo3-wheel-uvmaturin-runtime-smoke-test)
**Date:** 2026-06-15
**ADR:** ADR-0005 (kernel-once — pure Rust kernel bound to Py/Node/WASM), ADR-0006 (Python packaging)
**Follow-up of:** [SMA-409](https://linear.app/smaschek/issue/SMA-409/wire-cross-language-affected-graph-cascade-re-verify-at-phase-2-entry) — landed the `paigasus-kernel-rs → paigasus-py-bindings-rs` edge at **compile level only**; deferred the runtime/wheel chain and the Python-visible binding here (review findings F4 + F5).
**Design context:** `docs/superpowers/specs/2026-06-14-sma-409-affected-graph-cascade-guard-design.md` (Out of scope + Follow-ups).

## Goal

Make the kernel value cross `Rust kernel → PyO3 → maturin wheel → Python` at **runtime**,
prove it with a `pytest`, and extend the affected-graph guard so a kernel (or binding) edit
now legitimately cascades into the Python stack. This completes the deferred **py side of
SMA-409 AC #1** ("kernel wrappers in py").

SMA-409 already shipped the Rust half: `paigasus-py-bindings` (a `cdylib` with
`pyo3 0.29` + `abi3-py312` + `extension-module`) exposes `sum_as_string(a, b) -> String`
via a provisional `#[pymodule] paigasus_py_bindings`, genuinely calling
`paigasus_kernel::sum`. The `paigasus-kernel-rs → paigasus-py-bindings-rs` Moon edge with
`^:build` is wired and guarded. **This issue does not re-touch the Rust crate's logic** — it
builds the Python consumption chain on top and extends the existing guard.

## Decisions resolved during brainstorming

1. **Two Python packages, not one.** A new maturin-built wheel package
   (`py/packages/paigasus-py-bindings`) compiles the existing Rust crate and exposes the
   native module; the existing `paigasus-kernel` uv package stays pure (`uv_build`),
   depends on the wheel package, and re-exports a clean public surface. This keeps the
   *compiled FFI artifact* cleanly separate from the *public Python API*. Rejected
   alternative: making `paigasus-kernel` itself maturin-built (mixed Rust/Python layout) —
   fewer moving parts, but couples the public package to the compiler toolchain and the
   out-of-tree manifest.
2. **maturin is uv-native + pinned in `[build-system].requires`, not `.prototools`.**
   maturin is a PEP 517 build backend, so its correct home is the wheel package's
   `[build-system] requires = ["maturin>=1.7,<2"]`, locked via `uv.lock`. uv drives the
   build in isolation (`uv sync`); no standalone maturin CLI and **no new proto plugin**.
   This reverses the issue text's *speculation* that maturin would land in `.prototools` —
   that would only be warranted if we invoked `maturin develop` outside uv's resolver,
   adding a second pinning system. Single source of truth (uv.lock) wins.
3. **No Rust pymodule rename.** Under the two-package split the provisional native name
   `paigasus_py_bindings` becomes *deliberate* — owned by the wheel package via
   `[tool.maturin] module-name` — and `paigasus_kernel` re-exports from it. "Reconcile the
   provisional name" (issue scope) is satisfied by ownership + re-export, not a rename.
4. **Keep the placeholder `sum_as_string` as the public surface.** The kernel fn is itself
   a deliberate placeholder (SMA-409 decision #3); inventing a "cleaner" API now would be
   premature. The smoke test asserts the FFI round-trip, not a domain contract.
5. **The guard's binding-rs touch case grows too, not just the kernel-edit case.** The
   issue names only the kernel-edit case, but for a coherent guard a `paigasus-py-bindings`
   Rust edit must also be shown to cascade to the two new py projects (they depend on it),
   while still asserting the one-way property that it does **not** drag in
   `paigasus-kernel-rs`.

## 1. Package layout (two packages)

### New: `py/packages/paigasus-py-bindings/` — the compiled wheel

maturin-built, **no Python source of its own** (pure compiled extension):

```toml
# pyproject.toml
[project]
name = "paigasus-py-bindings"
version = "0.0.0"
requires-python = ">=3.12"

[build-system]
requires = ["maturin>=1.7,<2"]
build-backend = "maturin"

[tool.maturin]
manifest-path = "../../../rs/crates/bindings/paigasus-py-bindings/Cargo.toml"
module-name   = "paigasus_py_bindings"
```

Produces a single `abi3` wheel (from the crate's existing `abi3-py312` feature) exposing
the native module `paigasus_py_bindings` with `sum_as_string`. No Rust change required.

### Existing: `py/packages/paigasus-kernel/` — the public wrapper

Stays `uv_build`; gains the dependency and re-exports:

```python
# src/paigasus_kernel/__init__.py  (keeps its SPDX header)
from paigasus_py_bindings import sum_as_string

__all__ = ["sum_as_string"]
```

## 2. uv workspace wiring

`paigasus-kernel`'s `pyproject.toml` adds the workspace dependency:

```toml
dependencies = ["paigasus-py-bindings"]

[tool.uv.sources]
paigasus-py-bindings = { workspace = true }
```

The new package is auto-discovered by the existing `members = ["packages/*"]` glob in
`py/pyproject.toml`. `uv.lock` regenerates and records the `maturin` build pin. `uv sync`
builds the wheel in isolation (PEP 660 editable); CI runs `uv sync` then `pytest`.

## 3. Public surface & runtime smoke test

The round-trip test lives in **`py/packages/paigasus-kernel/tests/test_ffi_roundtrip.py`**
(exercising the public surface, so it transitively proves the whole chain), fitting the
existing `testpaths = ["packages/*/tests"]`:

```python
# SPDX-License-Identifier: Apache-2.0
from paigasus_kernel import sum_as_string


def test_sum_crosses_ffi_boundary():
    assert sum_as_string(2, 3) == "5"
```

This is the first real test in the py stack; the SMA-379 "no tests collected" mask becomes
moot for this package (and stays harmless).

## 4. Build-graph edges (Moon)

Extend the cascade `kernel-rs → py-bindings-rs → py-bindings-py → kernel-py`. Per the
`moon-ci-affected-model` rule, project `dependsOn` alone does **not** propagate
task-affectedness — the task-level `^:build` deps (plus `--include-relations`, already
asserted by the guard) are what carry affectedness across the language boundary.

- **New** `py/packages/paigasus-py-bindings/moon.yml`: id `paigasus-py-bindings-py`,
  `layer: library`, `language: python`, `dependsOn: ['paigasus-py-bindings-rs']`,
  `build`/`test` tasks with `deps: ['^:build']`.
- **Edit** `py/packages/paigasus-kernel/moon.yml`: `dependsOn: ['paigasus-py-bindings-py']`,
  `build`/`test` with `deps: ['^:build']`.

Note: the moon id `paigasus-py-bindings-py` and the Rust crate's `paigasus-py-bindings-rs`
share the leaf dir name across stacks; the `-py`/`-rs` suffix convention disambiguates them
(`moon-project-id-stack-suffix`). The wheel package's build compiles the Rust crate itself
(via maturin→cargo), so the `dependsOn` edge to `paigasus-py-bindings-rs` is primarily for
the affected-graph cascade rather than strict build ordering.

## 5. Affected-graph regression guard (`ci/affected-graph/run.sh` + README)

- **kernel-edit touch** (`rs/crates/libs/paigasus-kernel/src/lib.rs`): move
  `paigasus-kernel-py` and `paigasus-py-bindings-py` from the forbid side to **must-include**.
  Narrow the forbid-regex so it no longer blanket-forbids `-py$`, but **still asserts the
  negatives that are now meaningful**: `-ts$`/`^contracts$`/`^ts$` plus the unrelated py
  packages (`paigasus-proto-py`, `paigasus-workflows-py`, `paigasus-ml-py`) must remain
  unaffected by a kernel edit. (This is exactly the must-exclude revision SMA-409 F5
  anticipated — the expected next edge, not a regression.)
- **binding-rs touch** (`rs/crates/bindings/paigasus-py-bindings/src/lib.rs`): must-include
  grows to also include `paigasus-py-bindings-py` + `paigasus-kernel-py`, while keeping the
  one-way assertion that it does **not** drag in `paigasus-kernel-rs`.
- The `--negative-control` path must still fail red (harness can still detect a broken edge).
- Update `ci/affected-graph/README.md`'s maintenance note to reflect the new topology.

## Verification (maps to acceptance criteria)

1. **AC #1** — `uv sync` builds the wheel; `python -c "from paigasus_kernel import sum_as_string"`
   succeeds; `maturin` pinned in `[build-system].requires` and present in `uv.lock`.
2. **AC #2** — `uv run pytest` (the round-trip test) passes, asserting a value crosses the
   FFI boundary at runtime.
3. **AC #3** — `moon run repo:affected-smoke` passes with the updated must-include +
   narrowed forbid-regex; `ci/affected-graph/run.sh --negative-control` still fails red.
4. **AC #4** — full `moon ci :build`/`:test`/`:deny`/`:machete`/`:affected-smoke` green; the
   `cargo` gates (`fmt`/`clippy`/`nextest`) are untouched.

## Primary risk — de-risk first

**uv↔maturin build isolation with an out-of-tree `manifest-path`.** The first
implementation step is a throwaway spike: confirm `uv sync` resolves the relative
`../../../rs/...` manifest from inside its isolated build env and produces an importable
`abi3` wheel. Everything else builds on this. Known non-blocking caveat: an editable install
will **not** auto-recompile on a Rust source edit — a re-`uv sync` is required (acceptable for
dev iteration and irrelevant to CI's clean build).

## Out of scope (deferred, with follow-ups)

- **TS/napi/wasm kernel binding** — no binding crate exists yet (SMA-409 deferral, ts side).
- **Flipping `publish`/version off `0.0.0`** for `paigasus-kernel` / `paigasus-py-bindings`
  (SMA-376, SMA-407).
- **sdist-correct packaging** — the out-of-tree `manifest-path` will not resolve from a
  published sdist; this is fine while publish is deferred. A note will be left in the wheel
  package's `pyproject.toml` so the constraint is discoverable when publishing is activated.
- **Real kernel domain logic** — `sum` remains a deliberate placeholder.
