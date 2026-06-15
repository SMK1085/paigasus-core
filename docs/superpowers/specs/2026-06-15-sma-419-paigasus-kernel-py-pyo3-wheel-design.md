# SMA-419 — Wire `paigasus-kernel-py` to the PyO3 wheel (uv↔maturin) + runtime smoke test

**Status:** approved design (brainstorm + staff review incorporated, ready for plan)
**Linear:** [SMA-419](https://linear.app/smaschek/issue/SMA-419/wire-paigasus-kernel-py-to-the-pyo3-wheel-uvmaturin-runtime-smoke-test)
**Date:** 2026-06-15
**ADR:** ADR-0005 (kernel-once — pure Rust kernel bound to Py/Node/WASM)
**Layout authority:** Notion *Polyglot Monorepo Scoping* §1 (Directory layout) + §3 (Shared Rust core + FFI) — the maturin cross-directory caveat and its co-located fallback.
**Follow-up of:** [SMA-409](https://linear.app/smaschek/issue/SMA-409/wire-cross-language-affected-graph-cascade-re-verify-at-phase-2-entry) — landed the `paigasus-kernel-rs → paigasus-py-bindings-rs` edge at **compile level only**; deferred the runtime/wheel chain and the Python-visible binding here (review findings F4 + F5).
**Design context:** `docs/superpowers/specs/2026-06-14-sma-409-affected-graph-cascade-guard-design.md` (Out of scope + Follow-ups).
**Reviewed by:** staff-engineer design review (`2026-06-15-…-review.md`); dispositions in the final section.

## Goal

Make the kernel value cross `Rust kernel → PyO3 → maturin wheel → Python` at **runtime**,
prove it with a `pytest`, and extend the affected-graph guard so a kernel (or binding) edit
now legitimately cascades into the Python stack. This completes the deferred **py side of
SMA-409 AC #1** ("kernel wrappers in py").

SMA-409 already shipped the Rust half: `paigasus-py-bindings` (a `cdylib` with
`pyo3 0.29` + `abi3-py312` + `extension-module`) exposes `sum_as_string(a, b) -> String`
via a `#[pymodule] paigasus_py_bindings`, genuinely calling `paigasus_kernel::sum`. The
`paigasus-kernel-rs → paigasus-py-bindings-rs` Moon edge with `^:build` is wired and guarded,
and `rs/.cargo/config.toml` carries the macOS link flags that let `cargo build` the
`extension-module` cdylib without maturin. **This issue does not re-touch the Rust crate's
logic** — it builds the Python consumption chain on top and extends the existing guard.

## Decisions resolved during brainstorming

1. **Co-located maturin wheel + pure-Python wrapper (the *Polyglot Monorepo Scoping*
   fallback layout).** The maturin `pyproject.toml` lives **inside the binding crate**
   (`rs/crates/bindings/paigasus-py-bindings/`, next to its `Cargo.toml` — no `manifest-path`),
   and the pure-`uv_build` `paigasus-kernel` package depends on the built wheel via a uv
   **path source**, re-exporting its public surface. This keeps the compiled FFI artifact
   distinct from the public API while avoiding a cross-directory `manifest-path`. *(Revised
   after staff review — see decision-revision note below and disposition F1.)*
2. **maturin is uv-native + pinned in `[build-system].requires`, not `.prototools`.**
   maturin is a PEP 517 build backend, so its home is the wheel package's
   `[build-system] requires = ["maturin>=1.7,<2"]`, locked via `uv.lock`. uv drives the
   build in isolation (`uv sync`); no standalone maturin CLI and **no new proto plugin**.
   This reverses the issue text's *speculation* that maturin would land in `.prototools`.
3. **No Rust pymodule rename.** The native name `paigasus_py_bindings` stays — it is now
   *deliberately owned* by the co-located wheel package, and `paigasus_kernel` re-exports
   from it. "Reconcile the provisional name" (issue scope) is satisfied by ownership +
   re-export, not a rename.
4. **Keep the placeholder `sum_as_string` as the public surface.** The kernel fn is itself
   a deliberate placeholder (SMA-409 decision #3); the smoke test asserts the FFI round-trip,
   not a domain contract.
5. **The guard's binding-rs touch case grows too, not just the kernel-edit case.** For a
   coherent guard a `paigasus-py-bindings` Rust edit must also be shown to cascade to
   `paigasus-kernel-py` (which now depends on it), while still asserting it does **not** drag
   in `paigasus-kernel-rs`.

**Decision-revision note (decision #1).** Brainstorming first chose a *new*
`py/packages/paigasus-py-bindings/` maturin package with a cross-directory `manifest-path`.
Staff review (F1) showed that is a third path the canonical doc doesn't describe — it takes
on the documented cross-directory sharp edge *and* adds a package the doc doesn't call for.
The decisive factor (F3): Cargo discovers `.cargo/config.toml` from the **working
directory's** ancestors. A cross-directory `manifest-path` makes maturin run cargo from a cwd
**outside `rs/`**, so `rs/.cargo/config.toml`'s macOS link flags are missed and the wheel
fails to link on macOS with undefined `_Py*` symbols. The co-located fallback keeps cargo
running **inside `rs/`**, so the flags resolve. The canonical *primary* layout (a single
maturin `paigasus-kernel`) shares the same cross-directory hazard, so we take the documented
fallback instead.

## 1. Package layout (co-located fallback)

### `rs/crates/bindings/paigasus-py-bindings/` — gains a co-located maturin `pyproject.toml`

Sits beside the existing `Cargo.toml`; no `manifest-path` (maturin finds the crate in-place):

```toml
# pyproject.toml  (SPDX per CONTRIBUTING config-file exemption)
[project]
name = "paigasus-py-bindings"
version = "0.0.0"
requires-python = ">=3.12"

[build-system]
requires = ["maturin>=1.7,<2"]
build-backend = "maturin"

[tool.maturin]
module-name = "paigasus_py_bindings"   # matches the existing #[pymodule]; no rename
```

Produces a single `abi3` wheel (from the crate's `abi3-py312` feature) exposing the native
module `paigasus_py_bindings` with `sum_as_string`. The crate's `moon.yml`
(`paigasus-py-bindings-rs`, `language: rust`) is unchanged — it keeps its `cargo` `build`/`test`
tasks. The added `pyproject.toml` is **not** a uv workspace member (it's outside `py/`); it is
reached only via the path source below.

### `py/packages/paigasus-kernel/` — the public wrapper (stays `uv_build`)

```toml
# pyproject.toml additions
dependencies = ["paigasus-py-bindings"]

[tool.uv.sources]
paigasus-py-bindings = { path = "../../../rs/crates/bindings/paigasus-py-bindings" }
```

```python
# src/paigasus_kernel/__init__.py  (keeps its SPDX header)
from paigasus_py_bindings import sum_as_string

__all__ = ["sum_as_string"]
```

(`editable = true` on the path source is an optional dev-ergonomics choice; the compiled
extension still won't auto-recompile on a Rust edit either way — see §6. Default to the
non-editable path source, which is what CI needs.)

## 2. uv workspace wiring

`uv sync` resolves the path source and builds the co-located wheel in isolation (maturin from
`[build-system].requires`, shelling out to cargo). `uv.lock` regenerates and records the
maturin pin. The new `pyproject.toml` is not added to `members = ["packages/*"]`.

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

Extend the existing cascade `kernel-rs → py-bindings-rs` with a single new cross-language edge
to the wrapper. Per the `moon-ci-affected-model` rule, the task-level `^:build` deps (plus
`--include-relations`, already asserted by the guard) are what carry affectedness — a
project-level `dependsOn` alone does not.

- **Edit** `py/packages/paigasus-kernel/moon.yml`: `dependsOn: ['paigasus-py-bindings-rs']`,
  `build`/`test` tasks with `deps: ['^:build']`. The `dependsOn` to a **Rust** project is also
  what makes Moon provision **both** the Python and Rust toolchains in this task's context —
  exactly the case the `.moon/templates/python/template.yml` caveat anticipated ("first
  surfaces in the kernel-bindings work"), since `uv sync` here invokes maturin → cargo.
- **No new `paigasus-py-bindings-py` project** — the wheel is built as part of
  `paigasus-kernel-py:build` (via `uv sync`), so the graph stays
  `kernel-rs → py-bindings-rs → kernel-py`.

## 5. Affected-graph regression guard (`ci/affected-graph/run.sh` + README)

- **kernel-edit touch** (`rs/crates/libs/paigasus-kernel/src/lib.rs`): add `paigasus-kernel-py`
  to **must-include** (`paigasus-py-bindings-rs` is already there). Narrow the forbid-regex so
  it no longer blanket-forbids `-py$`, but **still asserts the negatives that remain true**:
  `-ts$`/`^contracts$`/`^ts$` plus the unrelated py packages (`paigasus-proto-py`,
  `paigasus-workflows-py`, `paigasus-ml-py`) must stay unaffected by a kernel edit. (This is
  the must-exclude revision SMA-409 F5 anticipated — the expected next edge, not a regression.)
- **binding-rs touch** (`rs/crates/bindings/paigasus-py-bindings/src/lib.rs`): must-include
  grows to also include `paigasus-kernel-py`, while keeping the one-way assertion that it does
  **not** drag in `paigasus-kernel-rs`.
- `ci/affected-graph/run.sh --negative-control` must still fail red.
- Update `ci/affected-graph/README.md`'s maintenance note to reflect the new topology.

## 6. Build mechanics & the double-compile (review F2)

In a full `moon ci :build`, the binding crate compiles **twice**: `paigasus-py-bindings-rs:build`
runs `cargo build` (for the crate's own `fmt`/`clippy`/`nextest` gates), and
`paigasus-kernel-py:build` runs `uv sync → maturin → cargo` (for the wheel). Both invoke cargo
against `rs/target/`. This is named deliberately, not free:

- **Decision:** accept the double-compile as the cost of keeping a uniform `:build` gate and an
  independently buildable Rust crate. The cargo `build` on `paigasus-py-bindings-rs` is what the
  `clippy`/`nextest`/`fmt` gates compile against; the maturin build produces the shippable wheel.
- **Target-dir contention:** if Moon schedules the two cargo invocations concurrently against a
  shared `rs/target/`, cargo serializes on its target-dir lock (safe; the second waits). The
  spike confirms whether maturin shares `rs/target/` or uses its own, and we choose the shared
  dir intentionally (cache reuse over double disk). This is the same two-builders-into-one-area
  class as the SMA-391 `.next`-lock collision, in cargo form — hence calling it out.

## Primary risk — de-risk first (spike before anything else)

The first implementation step is a throwaway spike proving the uv↔maturin chain end-to-end on
the user's macOS host, checking **all** of:

1. **`.cargo/config.toml` discovery (macOS link).** Confirm that when `uv sync` builds the
   co-located path source, maturin runs cargo with a cwd **inside `rs/`** so the
   `apple-darwin` link flags resolve and the `abi3` wheel links (no undefined `_Py*` symbols).
2. **cargo reachable from inside uv's build isolation (F3).** uv's isolated PEP 517 env
   provides maturin but **not** cargo/the Rust toolchain — confirm cargo is on the system PATH
   when `uv sync` triggers the maturin build (CI: Moon installs the Rust toolchain via the
   `dependsOn` provisioning; locally: after `proto install`).
3. **Path source resolves + imports.** `uv sync` builds the out-of-`py/` path source and
   `from paigasus_kernel import sum_as_string` succeeds.
4. **Cache-bust on a Rust edit (F4).** Since SMA-361's CI caches the uv cache, confirm a
   kernel/binding **Rust-source** change actually re-runs the maturin **compile** (the affected
   cascade busts the build), so the smoke test isn't asserting against a stale wheel — not
   merely a uv re-resolution.

Known non-blocking caveat: an editable install will **not** auto-recompile on a Rust edit — a
re-`uv sync` is required (acceptable for dev iteration, irrelevant to CI's clean build).

## Verification (maps to acceptance criteria)

1. **AC #1** — `uv sync` builds the wheel; `python -c "from paigasus_kernel import sum_as_string"`
   succeeds; `maturin` pinned in `[build-system].requires` and present in `uv.lock`.
2. **AC #2** — `uv run pytest` (the round-trip test) passes at runtime.
3. **AC #3** — `moon run repo:affected-smoke` passes with the updated must-include +
   narrowed forbid-regex; `ci/affected-graph/run.sh --negative-control` still fails red.
4. **AC #4** — full `moon ci :build`/`:test`/`:deny`/`:machete`/`:affected-smoke` green; the
   `cargo` gates (`fmt`/`clippy`/`nextest`) are untouched.

## Out of scope (deferred, with follow-ups)

- **TS/napi/wasm kernel binding** — no binding crate exists yet (SMA-409 deferral, ts side).
- **Publishing** — flipping `publish`/version off `0.0.0` for `paigasus-kernel` /
  `paigasus-py-bindings`, and the PyPI metadata + release-artifact discipline (ADR-0006,
  ADR-0011, SMA-376/407). Note: a published **sdist** of the wheel package would need its own
  packaging treatment; while publish is deferred this is irrelevant, but a discoverable note
  goes in the wheel package's `pyproject.toml`.
- **Real kernel domain logic** — `sum` remains a deliberate placeholder.

## Review dispositions (staff review, 2026-06-15)

- **F1 (Medium — layout) — accepted, layout changed.** Switched from the invented
  `py/packages/paigasus-py-bindings/` cross-dir `manifest-path` to the documented co-located
  fallback (§1). ADR-0006 was mis-cited as the layout authority — it governs the
  publish/open-core-boundary discipline (per the ADR-0003/0005 cross-refs); the layout
  authority is *Polyglot Monorepo Scoping* §1/§3. Decisive tie-breaker: the macOS
  `.cargo/config.toml` cwd-discovery hazard (see decision-revision note + spike check #1).
- **F2 (Medium — double-compile / target lock) — accepted.** Named explicitly in §6 with a
  deliberate decision and a spike check on target-dir sharing/contention.
- **F3 (Low — cargo on PATH in uv isolation) — accepted, and elevated.** It is the concrete
  mechanism behind F1's macOS failure; folded into spike checks #1 and #2.
- **F4 (Low — stale wheel in cached CI) — accepted.** Spike check #4 verifies a Rust edit
  busts the maturin build, not just the uv resolution.
