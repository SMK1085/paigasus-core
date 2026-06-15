# Wire `paigasus-kernel-py` to the PyO3 wheel (uv↔maturin) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a kernel value cross `Rust kernel → PyO3 → maturin wheel → Python` at runtime, prove it with pytest, and extend the affected-graph guard so a kernel/binding edit cascades into the Python stack.

**Architecture:** Co-located maturin layout (Polyglot Monorepo Scoping §1/§3 fallback): a maturin `pyproject.toml` lives *inside* the existing binding crate `rs/crates/bindings/paigasus-py-bindings/` (next to its `Cargo.toml`, no `manifest-path`), so maturin runs cargo from within `rs/` and `rs/.cargo/config.toml`'s macOS link flags resolve. The pure-`uv_build` `py/packages/paigasus-kernel` package depends on the wheel via a uv path source and re-exports its public surface. Moon gets one new cross-language edge (`paigasus-kernel-py → paigasus-py-bindings-rs`); the affected-graph guard moves `paigasus-kernel-py` from forbid → must-include.

**Tech Stack:** Rust (pyo3 0.29, abi3-py312, cdylib), maturin (PEP 517 backend, pinned in `[build-system].requires`), uv workspace, Moon 2.x, bash guard script.

**Spec:** `docs/superpowers/specs/2026-06-15-sma-419-paigasus-kernel-py-pyo3-wheel-design.md`

---

## Conventions for every task

- Run all commands from the repo root `/Users/smaschek/dev/paigasus/paigasus-core` unless a step says otherwise.
- Proto-managed tools (`moon`, `uv`, `cargo`/rust, `buf`) are off the default non-interactive PATH. Ensure they're reachable first: `export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"`. There is no macOS `timeout` binary.
- Commits are **SSH-signed via 1Password** (`op-ssh-sign`). If `git commit` fails with `1Password: failed to fill whole buffer`, 1Password is locked — unlock it and retry. A `commit-msg` lefthook runs commitlint: Conventional Commits, **scope required**, allowed types `feat|fix|docs|chore|refactor|test|ci|build|perf|style|revert`, allowed scopes `rs|py|ts|contracts|ci|docs|deps|release|repo|claude|workspace`, header ≤100 chars, body lines ≤100 chars.
- Branch is already `feature/sma-419-wire-paigasus-kernel-py-to-the-pyo3-wheel-uvmaturin-runtime`.
- End every commit body with the footer (blank line before it):
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `rs/crates/bindings/paigasus-py-bindings/pyproject.toml` | maturin build of the existing crate into an abi3 wheel exposing `paigasus_py_bindings` | **Create** |
| `rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi` | type stub for the compiled module (so basedpyright resolves the re-export) | **Create** (spike confirms placement) |
| `py/packages/paigasus-kernel/pyproject.toml` | depend on the wheel via a uv path source | **Modify** |
| `py/packages/paigasus-kernel/src/paigasus_kernel/__init__.py` | re-export the public surface | **Modify** |
| `py/packages/paigasus-kernel/tests/test_ffi_roundtrip.py` | runtime FFI round-trip proof | **Create** |
| `py/packages/paigasus-kernel/moon.yml` | new cross-language edge + toolchain provisioning + task wiring | **Modify** |
| `py/uv.lock` | records the maturin pin + the path source | **Modify** (regenerated) |
| `ci/affected-graph/run.sh` | guard cases: must-include + forbid-regex + negative-control | **Modify** |
| `ci/affected-graph/README.md` | guard maintenance note reflecting the landed py edge | **Modify** |
| `moon.yml` (root) | add the co-located pyproject to `affected-smoke` inputs | **Modify** |
| `docs/superpowers/specs/2026-06-15-sma-419-spike-findings.md` | record the 6 spike answers (load-bearing) | **Create** |

---

## Task 1: Spike — prove the uv↔maturin chain on macOS (load-bearing; do this first)

This task is exploratory by design (the spec names it the headline risk). It creates the real co-located files, validates the integration against six concrete checks on the macOS host, and **records the answers** in a findings note that Tasks 2–4 depend on. If any check fails, stop and follow its stated fallback before proceeding.

**Files:**
- Create: `rs/crates/bindings/paigasus-py-bindings/pyproject.toml`
- Modify: `py/packages/paigasus-kernel/pyproject.toml`
- Create: `docs/superpowers/specs/2026-06-15-sma-419-spike-findings.md`

- [ ] **Step 1: Create the co-located maturin pyproject**

Create `rs/crates/bindings/paigasus-py-bindings/pyproject.toml` (pyproject is SPDX-exempt as a config file, matching the existing py pyprojects):

```toml
[project]
name = "paigasus-py-bindings"
version = "0.0.0"
requires-python = ">=3.12"

[build-system]
requires = ["maturin>=1.7,<2"]
build-backend = "maturin"

[tool.maturin]
# Cargo.toml is co-located (same dir) → no manifest-path. Keeping this pyproject INSIDE
# rs/ means maturin runs cargo from within rs/, so rs/.cargo/config.toml's apple-darwin
# link flags resolve and the extension-module cdylib links on macOS (SMA-419; Polyglot
# Monorepo Scoping §1/§3 co-located fallback). NOTE (publish deferred): a published sdist
# won't carry rs/.cargo/config.toml — revisit packaging when flipping publish off 0.0.0.
module-name = "paigasus_py_bindings"
```

- [ ] **Step 2: Point `paigasus-kernel` at the wheel (temporary for the spike)**

Edit `py/packages/paigasus-kernel/pyproject.toml`: set `dependencies = ["paigasus-py-bindings"]` and add a `[tool.uv.sources]` table. Final file:

```toml
[project]
name = "paigasus-kernel"
version = "0.0.0"
requires-python = ">=3.12"
dependencies = ["paigasus-py-bindings"]
# TODO(SMA-378): before first PyPI publish, paigasus-proto & paigasus-kernel need
# description/readme/license = "Apache-2.0"/authors/classifiers (ADR-0006). (PyPI-bound only.)

[build-system]
requires = ["uv_build>=0.11.16,<0.12"]
build-backend = "uv_build"

[tool.uv.sources]
paigasus-py-bindings = { path = "../../../rs/crates/bindings/paigasus-py-bindings" }
```

- [ ] **Step 3: Run the integration and record each answer**

Run each command, note the result in the findings note (Step 4). Stop on the first hard failure and apply its fallback.

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"

# (Check 1+2+3) Build the wheel in uv isolation and prove the import round-trips.
# Expected: maturin compiles the crate, the abi3 wheel links on macOS (NO undefined _Py*
# symbols), and the import prints 5.
cd py && uv sync 2>&1 | tee /tmp/sma419-sync.log && \
  uv run python -c "from paigasus_kernel import sum_as_string; print(sum_as_string(2,3))"
cd ..
# Check 1 (macOS link): /tmp/sma419-sync.log shows a successful maturin/cargo build, no
#   "Undefined symbols ... _Py..." linker error. FALLBACK if it fails: the cwd is escaping
#   rs/ — confirm maturin runs cargo from rs/crates/bindings/paigasus-py-bindings/; do NOT
#   re-introduce a cross-dir manifest-path (that is the failure mode we chose this layout to
#   avoid). Worst case, set the link flags via RUSTFLAGS in the task env.
# Check 2 (cargo reachable): the build did not fail with "cargo: command not found" inside
#   uv's isolated build env. FALLBACK: ensure cargo is on PATH for the build (rust toolchain
#   via `moon setup` / proto shims).
# Check 3 (path source + import): the python -c printed exactly `5`.

# (Check 6) Whole-tree py tasks still pass now that uv run triggers a wheel build.
moon run py:typecheck   # Check 5: basedpyright resolves `from paigasus_py_bindings import …`
moon run py:test        # Check 6a: whole-tree pytest builds the wheel (cargo must be on PATH here)
moon run py:lint py:fmt # Check 6b: ruff whole-tree tasks still green
# Check 5 FALLBACK: if basedpyright errors on the stub-less compiled import, a .pyi stub is
#   required — proceed to Step 5 (it is included in this plan by default).
# Check 6 FALLBACK: if py:test/py:typecheck fail with "cargo: command not found", the `py`
#   config-root tasks lack the rust toolchain. Record this; Task 3 must make cargo reachable
#   for whole-tree py tasks (e.g. confirm proto rust shims are globally on PATH in CI, or add
#   the toolchain to the py root). Do not mark Task 1 done until py:test is green.

# (Check 4) A Rust-source edit must rebuild the wheel, not serve a cached one.
sed -i.bak 's/a + b/a + b + 0/' rs/crates/libs/paigasus-kernel/src/lib.rs  # no-op value change to bust source hash
cd py && uv sync --reinstall-package paigasus-py-bindings 2>&1 | tee /tmp/sma419-resync.log && \
  uv run python -c "from paigasus_kernel import sum_as_string; print(sum_as_string(2,3))"
cd ..
mv rs/crates/libs/paigasus-kernel/src/lib.rs.bak rs/crates/libs/paigasus-kernel/src/lib.rs  # revert
# Check 4: determine whether a plain `uv sync` rebuilds on Rust-source change, or whether
#   `--reinstall-package` (shown) is required. Record the mechanism — Task 3 uses it to
#   guarantee the smoke test never asserts against a stale wheel (review F4).

# (F2) Note maturin's target dir vs rs/target/ (cache reuse vs double disk).
grep -iE "Compiling paigasus|Finished|target" /tmp/sma419-sync.log | head
```

- [ ] **Step 4: Write the findings note**

Create `docs/superpowers/specs/2026-06-15-sma-419-spike-findings.md` recording, for each of the 6 checks: PASS/FAIL, the observed behavior, and the decision it drives. Explicitly record: (S4) the exact freshness mechanism Task 3 must use; (S5) whether a `.pyi` stub is required and where maturin places it in the wheel; (S6) whether whole-tree py tasks need extra cargo-PATH wiring; (F2) maturin's target dir.

- [ ] **Step 5: Add the type stub (default — keeps `typecheck` green and ships a typed surface)**

Create `rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi`:

```python
# SPDX-License-Identifier: Apache-2.0
def sum_as_string(a: int, b: int) -> str: ...
```

Confirm (from the spike) that maturin includes a co-located `<module>.pyi` in the wheel; if it does not auto-include, add it explicitly under `[tool.maturin]` (e.g. `include = ["paigasus_py_bindings.pyi"]`) and re-run `moon run py:typecheck` to confirm green.

- [ ] **Step 6: Commit the validated integration + findings**

```bash
git add rs/crates/bindings/paigasus-py-bindings/pyproject.toml \
        rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi \
        py/packages/paigasus-kernel/pyproject.toml \
        py/uv.lock \
        docs/superpowers/specs/2026-06-15-sma-419-spike-findings.md
git commit -m "build(py): co-locate maturin pyproject for the PyO3 wheel + validate uv↔maturin chain

Spike findings recorded in docs/superpowers/specs/2026-06-15-sma-419-spike-findings.md.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Re-export the public surface + runtime FFI smoke test (TDD)

**Files:**
- Modify: `py/packages/paigasus-kernel/src/paigasus_kernel/__init__.py`
- Test: `py/packages/paigasus-kernel/tests/test_ffi_roundtrip.py`

- [ ] **Step 1: Write the failing test**

Create `py/packages/paigasus-kernel/tests/test_ffi_roundtrip.py`:

```python
# SPDX-License-Identifier: Apache-2.0
"""Runtime proof a value crosses kernel -> PyO3 -> wheel -> Python (SMA-419)."""

from paigasus_kernel import sum_as_string


def test_sum_crosses_ffi_boundary() -> None:
    assert sum_as_string(2, 3) == "5"
    assert sum_as_string(-4, 4) == "0"
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd py && uv run pytest packages/paigasus-kernel/tests/test_ffi_roundtrip.py -v; cd ..
```
Expected: FAIL — `ImportError: cannot import name 'sum_as_string' from 'paigasus_kernel'` (the wheel is wired from Task 1, but `__init__.py` does not re-export it yet).

- [ ] **Step 3: Add the re-export**

Replace `py/packages/paigasus-kernel/src/paigasus_kernel/__init__.py` with:

```python
# SPDX-License-Identifier: Apache-2.0
from paigasus_py_bindings import sum_as_string

__all__ = ["sum_as_string"]
```

- [ ] **Step 4: Run the test to verify it passes**

Run:
```bash
cd py && uv run pytest packages/paigasus-kernel/tests/test_ffi_roundtrip.py -v; cd ..
```
Expected: PASS — `test_sum_crosses_ffi_boundary PASSED`.

- [ ] **Step 5: Confirm the whole-tree gates still pass**

Run:
```bash
moon run py:typecheck py:lint py:fmt py:test
```
Expected: all PASS (the new `tests/` file and re-export are ruff/basedpyright-clean; `py:test` includes the smoke test).

- [ ] **Step 6: Commit**

```bash
git add py/packages/paigasus-kernel/src/paigasus_kernel/__init__.py \
        py/packages/paigasus-kernel/tests/test_ffi_roundtrip.py
git commit -m "feat(py): re-export sum_as_string via the PyO3 wheel + runtime FFI smoke test

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Wire the Moon cross-language edge + cascade freshness

**Files:**
- Modify: `py/packages/paigasus-kernel/moon.yml`

- [ ] **Step 1: Add the dependsOn edge, `^:build`, and the freshness mechanism**

Replace `py/packages/paigasus-kernel/moon.yml` with the following. The `dependsOn` to a **Rust** project is what makes Moon provision the Rust toolchain alongside Python for this project's tasks (the `.moon/templates/python/template.yml` caveat); the `^:build` on `build`/`test` is what propagates `affected` under `moon ci --include-relations` (project `dependsOn` alone does not — `moon-ci-affected-model`):

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-kernel-py'
layer: 'library'
language: 'python'

# Cross-language edge to the PyO3 wheel crate (ADR-0005). Also provisions the Rust toolchain
# in this project's task context so `uv sync`'s maturin build can shell out to cargo
# (.moon/templates/python/template.yml caveat). The task-level `^:build` is what carries
# `affected` in `moon ci --include-relations`; a project `dependsOn` alone does not (SMA-389 D3).
dependsOn:
  - 'paigasus-py-bindings-rs'

tasks:
  build:
    deps: ['^:build']
  # Dedicated runtime smoke test for the FFI boundary. It lives here (not only in the
  # whole-tree py:test) so the kernel→bindings→py cascade actually re-runs it on a Rust edit
  # (review F4); `deps: ['^:build']` pulls it in under --include-relations.
  test:
    command: 'uv run pytest tests'
    deps: ['^:build']
    inputs: ['tests/**/*', 'src/**/*', 'pyproject.toml', '/py/uv.lock']
```

> **Spike-contingent (from Task 1 findings S4 + S6), apply before committing:**
> - **S4 (freshness):** if a plain `uv run`/`uv sync` does **not** rebuild the wheel on a Rust-source change, change the `test` task to a `script` that forces it, e.g. `script: 'uv sync --reinstall-package paigasus-py-bindings && uv run pytest tests'` (keep `deps`/`inputs`). If `uv` rebuilds automatically, leave `command` as above.
> - **S6 (cargo PATH for whole-tree tasks):** if the spike showed `py:test`/`py:typecheck` fail with `cargo: command not found`, apply the recorded fix here (the rust toolchain must be reachable for the `py` config-root tasks, not just `paigasus-kernel-py`).

- [ ] **Step 2: Verify the build/test graph runs and the cascade reaches the wrapper**

Run:
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon run paigasus-kernel-py:build paigasus-kernel-py:test
# Expected: both PASS; paigasus-kernel-py:test runs the smoke test green.

# Project-reachability: a kernel edit must reach the py wrapper.
printf '%s\n' 'rs/crates/libs/paigasus-kernel/src/lib.rs' \
  | moon query projects --affected --downstream deep \
  | python3 -c 'import sys,json;print(sorted(p["id"] for p in json.load(sys.stdin)["projects"]))'
```
Expected: the printed list includes `paigasus-kernel-rs`, `paigasus-py-bindings-rs`, **`paigasus-kernel-py`**, `paigasus-gateway-rs` (and `repo`).

- [ ] **Step 3: Commit**

```bash
git add py/packages/paigasus-kernel/moon.yml
git commit -m "build(repo): wire paigasus-kernel-py moon edge to the PyO3 binding crate

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Extend the affected-graph guard for the kernel→py cascade

**Files:**
- Modify: `ci/affected-graph/run.sh`
- Modify: `moon.yml` (root — `affected-smoke` inputs)
- Modify: `ci/affected-graph/README.md`

- [ ] **Step 1: Update the `kernel->bindings` case (must-include + forbid-regex)**

In `ci/affected-graph/run.sh`, replace the `kernel->bindings` case (currently lines ~88–91):

```bash
  # kernel edit -> kernel + binding + gateway; nothing cross-stack (no *-py / *-ts / contracts).
  run_case "kernel->bindings" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs" \
    '(-py|-ts)$|^contracts$|^py$|^ts$'
```

with (adds `paigasus-kernel-py` to must-include; narrows forbid so the kernel's own py wrapper is allowed while ts, contracts, the py root, and the *unrelated* py packages stay forbidden — SMA-409 F5):

```bash
  # kernel edit -> kernel + binding + gateway + the py wrapper (SMA-419). Still nothing else
  # cross-stack: no *-ts / contracts / py root, and no UNRELATED py packages (proto/workflows/ml).
  run_case "kernel->bindings" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs,paigasus-kernel-py" \
    '-ts$|^contracts$|^py$|^ts$|^paigasus-(proto|workflows|ml)-py$'
```

- [ ] **Step 2: Update the `binding-oneway` case (must-include grows; still one-way)**

Replace the `binding-oneway` case (currently lines ~92–94):

```bash
  # binding edit -> only the binding; the edge is one-directional (must not drag in the kernel).
  run_case "binding-oneway"   "rs/crates/bindings/paigasus-py-bindings/src/lib.rs" \
    "paigasus-py-bindings-rs" '^paigasus-kernel-rs$'
```

with (a binding edit now also reaches the py wrapper, but still must not drag in the kernel crate):

```bash
  # binding edit -> the binding + the py wrapper that depends on it (SMA-419); still
  # one-directional w.r.t. the kernel (must not drag in paigasus-kernel-rs).
  run_case "binding-oneway"   "rs/crates/bindings/paigasus-py-bindings/src/lib.rs" \
    "paigasus-py-bindings-rs,paigasus-kernel-py" '^paigasus-kernel-rs$'
```

- [ ] **Step 3: Fix the negative control (it must stay a genuinely-wrong expectation)**

The negative control requires `paigasus-kernel-py` to be missing from a kernel edit's affected set — but that edge now exists, so it would no longer fail red. Switch it to an unrelated py package. Replace lines ~102–104:

```bash
  echo "== negative control: assert a deliberately-wrong expectation reports red =="
  # paigasus-kernel-py is NOT a dependent of the kernel crate, so requiring it MUST fail.
  rc=0
  assert_case "neg-wrong-expect" "rs/crates/libs/paigasus-kernel/src/lib.rs" "paigasus-kernel-py" "" || rc=$?
```

with:

```bash
  echo "== negative control: assert a deliberately-wrong expectation reports red =="
  # paigasus-proto-py is NOT a dependent of the kernel crate, so requiring it MUST fail.
  # (paigasus-kernel-py IS a dependent now (SMA-419), so it can no longer serve as the wrong
  # expectation.)
  rc=0
  assert_case "neg-wrong-expect" "rs/crates/libs/paigasus-kernel/src/lib.rs" "paigasus-proto-py" "" || rc=$?
```

- [ ] **Step 4: Add the co-located pyproject to the `affected-smoke` task inputs**

In the root `moon.yml`, the `affected-smoke` task's `inputs` list watches graph-defining files but not the new `rs/crates/.../pyproject.toml`. Add it under the existing `py/packages/*/pyproject.toml` input line:

```yaml
      - 'py/packages/*/pyproject.toml'
      - 'rs/crates/*/*/pyproject.toml'
```

- [ ] **Step 5: Update the guard README**

In `ci/affected-graph/README.md`, replace the kernel/binding bullets (lines ~12–14):

```markdown
- **kernel edit** → `paigasus-kernel-rs` + `paigasus-py-bindings-rs` + `paigasus-gateway-rs`,
  and **nothing cross-stack** (no `*-py` / `*-ts` / `contracts`).
- **binding edit** → only `paigasus-py-bindings-rs` (the edge is one-directional).
```

with:

```markdown
- **kernel edit** → `paigasus-kernel-rs` + `paigasus-py-bindings-rs` + `paigasus-gateway-rs`
  + `paigasus-kernel-py` (the py wrapper now wraps the wheel, SMA-419); still **no `*-ts` /
  `contracts` / unrelated `*-py`** (`paigasus-proto/workflows/ml-py`).
- **binding edit** → `paigasus-py-bindings-rs` + `paigasus-kernel-py`; still one-directional
  w.r.t. the kernel (never drags in `paigasus-kernel-rs`).
```

And update the maintenance note (lines ~22–29) to record that the py edge has landed and only the **ts** wrapper remains deferred:

```markdown
## Maintenance — the must-exclude assertions are topology-coupled (SMA-409 F5)

The **must-include** sets are durable. The **must-exclude** (cross-stack-isolation)
assertions track current topology. The **py** wrapper edge landed in SMA-419
(`paigasus-kernel-py` moved from forbid → must-include). The remaining deferred edge is the
**ts** kernel wrapper: when it lands, a kernel edit *should* affect it, and the
`kernel->bindings` forbid-regex here will correctly need its `-ts$` term loosened. A failure
there is the expected next edge, not a regression; update this guard alongside that work.
```

- [ ] **Step 6: Verify the guard passes and can still fail**

Run:
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon run repo:affected-smoke
# Expected: PASS lines for contracts->proto, kernel->bindings, binding-oneway,
# ci-include-relations; ends "== affected-graph cascade intact ==".

ci/affected-graph/run.sh --negative-control
# Expected: "negative-control OK: harness reported red as expected" (exit 0).
```

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/run.sh ci/affected-graph/README.md moon.yml
git commit -m "ci(repo): extend affected-graph guard for the kernel→py cascade (SMA-419)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Full-gate verification (no regression)

**Files:** none (verification only; commit only if a gate reformats a file or regenerates a lockfile).

- [ ] **Step 1: Run the full `moon ci` gate set locally**

Run (mirrors the CI task array; `moon run` builds the whole affected set without a base diff):
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon run :build :test :lint :fmt :deny :machete :typecheck :affected-smoke
```
Expected: every task PASS. Pay attention to `:deny` and `:machete` (the binding crate's `Cargo.toml` is unchanged, so they should be unaffected) and `:test`/`:typecheck` (the py whole-tree tasks now build the wheel).

- [ ] **Step 2: Confirm the Rust gates are untouched**

Run:
```bash
cd rs && cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo nextest run --workspace --no-tests=pass; cd ..
```
Expected: all green (this issue added no Rust logic; the crate only gained a sibling `pyproject.toml` + `.pyi`).

- [ ] **Step 3: Verify against acceptance criteria**

Confirm each AC with the evidence gathered:
1. `uv sync` builds the wheel; `from paigasus_kernel import sum_as_string` works; `maturin` is in `py/uv.lock`. (`grep -n 'name = "maturin"' py/uv.lock`)
2. `paigasus-kernel-py:test` / `py:test` pass the round-trip assertion.
3. `moon run repo:affected-smoke` green; `--negative-control` reports red.
4. The full gate set in Step 1 is green; the Rust gates in Step 2 are green.

- [ ] **Step 4: Commit any gate-produced changes (if any)**

```bash
# Only if Step 1 reformatted a file or regenerated a lockfile:
git add -A && git commit -m "chore(repo): SMA-419 gate-produced fixups

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review notes (author)

- **Spec coverage:** §1 layout → Task 1; §2 uv wiring → Task 1; §3 smoke test → Task 2; §4 Moon edges → Task 3; §5 guard → Task 4; §6 double-compile (F2) → spike note + Task 5 (`:machete`/`:deny`/Rust gates) ; spike checks 1–6 → Task 1; verification/ACs → Task 5.
- **Spike-gated items** are flagged inline (S4 freshness, S5 stub, S6 cargo-PATH, F2 target-dir) and resolved by Task 1's findings note before the dependent task runs — they are decisions to record, not placeholders.
- **Type consistency:** `sum_as_string(a: int, b: int) -> str` and the native module name `paigasus_py_bindings` are used identically in the stub, the re-export, the test, and the guard cases.
