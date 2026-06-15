# SMA-419 Spike Findings — uv ↔ maturin co-located integration (macOS)

Date: 2026-06-15
Host: macOS (darwin 25.5.0, aarch64-apple-darwin)
Tools: uv 0.11.16, cargo 1.95.0, moon 2.3.2, maturin 1.14.0 (resolved from `maturin>=1.7,<2`)

This is the recorded output of the Task 1 spike. Tasks 2–4 depend on the decisions below.
All commands were run for real from the repo root (or `py/` where noted) with
`export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"`.

## Layout under test

- `rs/crates/bindings/paigasus-py-bindings/pyproject.toml` — co-located maturin build
  backend (`build-backend = "maturin"`), `module-name = "paigasus_py_bindings"`, NO
  `manifest-path` (Cargo.toml is the sibling in the same dir).
- `py/packages/paigasus-kernel/pyproject.toml` — `dependencies = ["paigasus-py-bindings"]`
  with `[tool.uv.sources] paigasus-py-bindings = { path = "../../../rs/crates/bindings/paigasus-py-bindings" }`.
- `py/uv.lock` gained the package: `source = { directory = "../rs/crates/bindings/paigasus-py-bindings" }`.

## The 6 checks

### Check 1 — macOS link (extension-module cdylib links, no undefined `_Py*`): PASS
`uv sync` (log `/tmp/sma419-sync.log`) built the binding cleanly:

```
   Building paigasus-py-bindings @ file:///.../rs/crates/bindings/paigasus-py-bindings
      Built paigasus-py-bindings @ file:///.../rs/crates/bindings/paigasus-py-bindings
```

No "Undefined symbols ... _Py..." linker error in any spike log. The cdylib linked:
`rs/target/release/libpaigasus_py_bindings.dylib` exists. This CONFIRMS the co-located
layout makes maturin run cargo from inside `rs/`, so `rs/.cargo/config.toml`'s
apple-darwin `-undefined dynamic_lookup` flags resolve. The whole reason for co-location
holds on this host.

### Check 2 — cargo reachable inside uv's isolated build env: PASS
The build did NOT fail with "cargo: command not found". maturin found cargo
(via `~/.cargo/bin/cargo`, the standard rustup-managed cargo home — NOT a proto shim) on PATH.
Caveat for CI / S6: this only matters on a COLD build (see S6).

### Check 3 — path source resolves + FFI import round-trips: PASS (with a Task-2 caveat)
- `uv run python -c "from paigasus_py_bindings import sum_as_string; print(sum_as_string(2,3))"`
  → prints `5`. The compiled wheel imports, the path source resolves, and the FFI call
  reaches `paigasus_kernel::sum` across the boundary. End-to-end integration is PROVEN.
- The plan's literal command `from paigasus_kernel import sum_as_string` currently FAILS
  with `ImportError: cannot import name 'sum_as_string' from 'paigasus_kernel'`. This is
  EXPECTED and not a layout problem: `py/packages/paigasus-kernel/src/paigasus_kernel/__init__.py`
  is still empty (SPDX only). Wiring the re-export `from paigasus_py_bindings import sum_as_string`
  into that `__init__.py` is explicitly **Task 2** ("Re-export + runtime FFI smoke test"),
  which has not run yet. The integration the check exists to validate (path source + wheel
  build + FFI) passes via the direct import; only the re-export shim is pending.

### Check 4 — Rust-source edit rebuilds the wheel (freshness): PASS, but ONLY with `--reinstall-package`
Edited the kernel crate (`a + b` → `a + b + 0`, a Cargo dependency of the binding, then reverted):
- Plain `uv sync`: `Resolved 60 packages ... Checked 57 packages` — **NO rebuild**, serves the
  cached wheel (log `/tmp/sma419-plain-sync.log`).
- `uv sync --reinstall-package paigasus-py-bindings`: shows `Building paigasus-py-bindings` /
  `Built ...` / `Uninstalled 1 ... Installed 1` — **rebuilds** (log `/tmp/sma419-resync.log`).

Also tested the binding's OWN `src/lib.rs`: both `touch` (mtime bump) and a real content
change (added a comment line) under a plain `uv sync` / `uv run` did NOT trigger a rebuild
either. uv treats the path-source build as fresh and does not re-invoke maturin on Rust
source changes. The `--reinstall-package` flag (or `--reinstall`) is the ONLY observed way
to force the maturin rebuild. See S4.

### Check 5 — basedpyright resolves the compiled import (whole-tree typecheck): PASS
`moon run py:typecheck` → `0 errors, 0 warnings, 0 notes`. The co-located `.pyi` stub ships
in the wheel and basedpyright (mode "all") resolves `paigasus_py_bindings`. See S5.

### Check 6 — whole-tree py tasks green: PASS (6a + 6b)
- 6a `moon run py:test` → `1 passed` (the existing paigasus-proto smoke test). Note: the venv
  was already populated by the prior `uv sync`, so `uv run pytest` did not need to rebuild —
  see S6 for the cold-build implication.
- 6b `moon run py:lint py:fmt` → `All checks passed!` / `6 files already formatted`.

## Decisions for downstream tasks

### S4 — exact freshness mechanism Task 3 must use
A plain `uv sync` / `uv run` does **NOT** rebuild the maturin wheel when EITHER the binding's
own Rust source OR its Cargo dependency (`paigasus-kernel`) changes — it serves uv's cached
build. The ONLY mechanism observed to force a rebuild is:

```
uv sync --reinstall-package paigasus-py-bindings
```

Task 3's Moon cascade must therefore NOT rely on a bare `uv sync`/`uv run` to pick up Rust
changes. It must invoke `uv sync --reinstall-package paigasus-py-bindings` (or `--reinstall`)
when an upstream rust crate (`paigasus-kernel-rs` / `paigasus-py-bindings-rs`) is affected.
This is the freshness edge the affected-graph cascade has to encode.

### S5 — is a `.pyi` stub required, and where does maturin place it in the wheel?
A stub IS provided and SHIPS. We authored `rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi`
(co-located, matching `module-name`). maturin auto-includes a co-located `<module>.pyi` with
NO extra `[tool.maturin] include` needed. In the installed wheel maturin lays it out as a
PACKAGE (not a top-level module):

```
paigasus_py_bindings/__init__.py        # maturin-generated: `from .paigasus_py_bindings import *`
paigasus_py_bindings/__init__.pyi       # OUR stub, renamed from paigasus_py_bindings.pyi
paigasus_py_bindings/paigasus_py_bindings.abi3.so
paigasus_py_bindings/py.typed           # maturin-generated
```

So: the stub is auto-included (no `include =` line required), and typecheck is green (Check 5).
KEEP the `.pyi`. For Task 2's re-export, import from the package `paigasus_py_bindings`
(`from paigasus_py_bindings import sum_as_string`); the symbol is re-exported by maturin's
generated `__init__.py` via `*`.

### S6 — do whole-tree py tasks need extra cargo-PATH wiring?
On THIS run, no `py:` task failed with "cargo: command not found", because the wheel was
already built/cached before the moon tasks ran, and (per S4) plain `uv run`/`uv sync` does NOT
re-invoke maturin on source change. So incremental `py:` task runs do NOT need cargo.

HOWEVER, the cargo requirement is real on a COLD cache (first build, or after
`--reinstall-package`): that build DOES shell out to cargo, which must be on PATH. The `py`
config-root project (and the per-package library projects) have NO `dependsOn` edge to a rust
crate, so Moon does not guarantee cargo/rust toolchain availability for them. Implication for
Task 3:
- The first wheel build (and any `--reinstall-package` refresh task) MUST run with cargo on
  PATH. In CI, the `py:` tasks run after `uv sync` in the moon-managed env where cargo is on
  PATH via proto, so this happens to work — but it is IMPLICIT, not wired.
- Task 3 should make this explicit: either give the freshness/refresh task a `dependsOn` /
  toolchain that guarantees cargo, or ensure the rust toolchain is on PATH for the project
  that triggers the maturin build. Do NOT assume the bare `py:` tasks carry cargo.

This run did NOT reproduce a "cargo: command not found" failure for `py:` tasks; the risk is
specifically the cold/`--reinstall` path, which Task 3 must wire.

### F2 — maturin's target dir vs rs/target/
maturin shares the cargo WORKSPACE target dir `rs/target/` — it does NOT create a separate
per-crate target. Observed artifacts:
- `rs/target/release/libpaigasus_py_bindings.dylib`
- `rs/target/wheels/paigasus_py_bindings-0.0.0-cp312-abi3-macosx_11_0_arm64.whl`
- no `rs/crates/bindings/paigasus-py-bindings/target/` directory.

This is because the co-located pyproject makes maturin run cargo from inside `rs/`, so cargo
resolves the workspace and its target dir (`rs/target/`). The wheel tag is
`cp312-abi3-macosx_11_0_arm64` (Generator: maturin 1.14.0, Root-Is-Purelib: false) — correct
abi3 platform wheel. No target-dir collision concerns for Moon caching beyond what `rs/target/`
already incurs.

## Surprises / notes

1. uv freshness for a maturin path source ignores Rust source mtime AND content; `uv sync`
   considers the build cached. Forcing a rebuild requires `--reinstall-package` (S4). This is
   the single most important downstream decision.
2. maturin promotes a single-module `<module>.pyi` into a package layout
   (`<module>/__init__.pyi` + generated `__init__.py`), rather than shipping a top-level
   `<module>.pyi`. Stub authored as `paigasus_py_bindings.pyi`; do not be surprised it lands as
   `paigasus_py_bindings/__init__.pyi` in the wheel.
3. Check 3's literal `from paigasus_kernel import …` fails today purely because the Task-2
   re-export is not yet in `paigasus_kernel/__init__.py`; the direct `from paigasus_py_bindings`
   import proves the FFI chain works.
