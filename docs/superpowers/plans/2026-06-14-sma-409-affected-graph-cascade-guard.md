# SMA-409 — kernel→bindings cascade + affected-graph regression guard — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the one genuinely-missing cross-language affected-graph edge — `paigasus-kernel-rs → paigasus-py-bindings-rs` — on real PyO3 code, and add a CI regression guard that asserts the whole cascade so a deleted edge fails red instead of silently under-building.

**Architecture:** A pure kernel function (`paigasus_kernel::sum`, unit-tested in Rust) is exposed to Python by a PyO3 `abi3`/`extension-module` binding that genuinely calls it (so `cargo machete` forces the dependency to be real). A Moon `dependsOn` + `^:build` edge on the binding makes a kernel edit cascade through `moon ci --include-relations` (a bare `dependsOn` does not propagate `affected` — only the task-level `^:build` does, per SMA-389 D3). A `ci/affected-graph/run.sh` script driven by a root `repo:affected-smoke` Moon task (release-parity pattern) feeds synthetic touched-files to `moon query projects --affected --downstream deep` and asserts the affected set per touch case, plus that `ci.yml`'s `moon ci` carries `--include-relations`.

**Tech Stack:** Rust (edition 2024, rustc 1.95), PyO3 0.29 (abi3-py312, extension-module), Moon 2.3.2, Bash + python3 (JSON parsing), GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-06-14-sma-409-affected-graph-cascade-guard-design.md`

**Prerequisites:** proto-managed `moon`/`cargo`/`python3` on PATH (see CONTRIBUTING). For non-interactive shells: `export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"`. Branch `feature/sma-409-wire-cross-language-affected-graph-cascade-re-verify-at` is already checked out with the spec committed.

**Commit conventions:** Conventional commits with a workspace scope (`feat(rs):`, `build(rs):`, `ci(repo):`). lefthook runs commitlint on `commit-msg`. Allowed types: `feat fix docs chore refactor test ci build perf style revert`; allowed scopes: `rs py ts contracts ci docs deps release repo claude workspace`. Limits: header ≤100, body lines ≤100, and the footer (`Co-Authored-By`) must be separated by a blank line.

---

## File Structure

- `rs/crates/libs/paigasus-kernel/src/lib.rs` — **modify**: add `pub fn sum` + unit test (the first real kernel primitive).
- `rs/Cargo.toml` — **modify**: add `pyo3` and `paigasus-kernel` to `[workspace.dependencies]`.
- `rs/.cargo/config.toml` — **create**: macOS-only link flags so the `extension-module` cdylib links under plain `cargo build` (CI is Linux and unaffected).
- `rs/crates/bindings/paigasus-py-bindings/Cargo.toml` — **modify**: consume `pyo3` + `paigasus-kernel`, disable the un-linkable test target, add the cargo-machete `pyo3` ignore.
- `rs/crates/bindings/paigasus-py-bindings/src/lib.rs` — **modify**: `#[pyfunction]` calling the kernel + `#[pymodule]`.
- `rs/crates/bindings/paigasus-py-bindings/moon.yml` — **modify**: `dependsOn: [paigasus-kernel-rs]` + `^:build` edges.
- `ci/affected-graph/run.sh` — **create**: the regression-guard harness.
- `ci/affected-graph/README.md` — **create**: what it guards + the maintenance note (must-exclude assertions are topology-coupled).
- `moon.yml` (root `repo` project) — **modify**: add the `affected-smoke` task.
- `.github/workflows/ci.yml` — **modify**: add `:affected-smoke` to the `moon ci` task array.

---

## Task 1: First real kernel function + Rust unit test

**Files:**
- Modify: `rs/crates/libs/paigasus-kernel/src/lib.rs`

- [ ] **Step 1: Write the failing test + the function signature's call site**

Append to `rs/crates/libs/paigasus-kernel/src/lib.rs` (after the existing module doc comment):

```rust
/// Sum two integers — the kernel's first real, pure primitive. Deliberately minimal
/// (placeholder for real domain logic); its purpose is to give the PyO3 binding a genuine
/// kernel call to consume so the `paigasus-kernel-rs → paigasus-py-bindings-rs` edge is real
/// (ADR-0005, SMA-409).
#[must_use]
pub fn sum(a: i64, b: i64) -> i64 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::sum;

    #[test]
    fn sums_two_integers() {
        assert_eq!(sum(2, 3), 5);
        assert_eq!(sum(-4, 4), 0);
    }
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `moon run paigasus-kernel-rs:test`
Expected: the `sums_two_integers` test runs and PASSES (`cargo nextest run --no-tests=pass`). (This crate had no tests before; it now has one real test.)

> Note: this task writes test + impl together because the function is trivial and the test won't compile without it. The red/green discipline is preserved at the cascade level in Task 5 (the guard is shown failing then passing).

- [ ] **Step 3: Verify lint + format are clean**

Run: `moon run paigasus-kernel-rs:lint paigasus-kernel-rs:fmt`
Expected: both PASS (`clippy -D warnings`, `fmt --check`). `a + b` is not flagged by `clippy::all`.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/libs/paigasus-kernel/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(rs): add paigasus-kernel::sum, the first real kernel primitive

A minimal pure function (placeholder for real domain logic) so the PyO3 binding
has a genuine kernel call to consume — making the kernel→bindings dependency edge
real rather than artificial (ADR-0005, SMA-409). Unit-tested in Rust.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Workspace dependencies + macOS link config

**Files:**
- Modify: `rs/Cargo.toml`
- Create: `rs/.cargo/config.toml`

- [ ] **Step 1: Add `pyo3` and `paigasus-kernel` to the workspace dependency table**

In `rs/Cargo.toml`, inside `[workspace.dependencies]`, add (place near the other entries):

```toml
# PyO3 — Rust↔Python FFI for the bindings crates (ADR-0005). `abi3-py312` builds one
# stable-ABI wheel for CPython >=3.12 (matches the py workspace's requires-python); the
# `extension-module` feature omits the libpython link so plain `cargo build` works without
# an embedded interpreter. macOS needs the link flags in rs/.cargo/config.toml.
pyo3 = { version = "0.29", features = ["abi3-py312", "extension-module"] }
# In-tree path dep: the bindings (and future consumers) build against the live kernel, so a
# kernel edit rebuilds them. This is also the cargo-side half of the affected-graph edge.
paigasus-kernel = { path = "crates/libs/paigasus-kernel" }
```

- [ ] **Step 2: Create `rs/.cargo/config.toml`**

```toml
# SMA-409: paigasus-py-bindings is a PyO3 `extension-module` cdylib — at link time its
# libpython symbols are intentionally undefined (CPython resolves them when it loads the
# module). Linux/ELF permits undefined symbols in shared objects by default; the macOS
# linker rejects them unless told to defer resolution. These flags let `cargo build` link
# the extension on macOS WITHOUT maturin. Scoped to apple-darwin so every other target keeps
# strict link-time symbol checking; CI (Linux) is unaffected.
[target.x86_64-apple-darwin]
rustflags = ["-C", "link-arg=-undefined", "-C", "link-arg=dynamic_lookup"]

[target.aarch64-apple-darwin]
rustflags = ["-C", "link-arg=-undefined", "-C", "link-arg=dynamic_lookup"]
```

- [ ] **Step 3: Verify the workspace still resolves and builds (deps are inert until consumed)**

Run: `moon run paigasus-kernel-rs:build paigasus-gateway-rs:build`
Expected: both PASS. Adding entries to `[workspace.dependencies]` does not pull them into any crate yet (a crate must opt in via `.workspace = true`), so nothing changes downstream.

- [ ] **Step 4: Verify the supply-chain gate accepts the new (not-yet-linked) pins**

Run: `moon run repo:deny`
Expected: PASS. (PyO3's tree is MIT/Apache-2.0/`Apache-2.0 WITH LLVM-exception`/Unicode — all in `rs/deny.toml`'s allow-list. A `multiple-versions` warning is non-blocking.)

- [ ] **Step 5: Commit**

```bash
git add rs/Cargo.toml rs/.cargo/config.toml
git commit -m "$(cat <<'EOF'
build(rs): add pyo3 + paigasus-kernel workspace deps and macOS link config

Declares pyo3 (abi3-py312, extension-module) and the in-tree paigasus-kernel path
dep in [workspace.dependencies] (inert until the bindings crate opts in), and adds
apple-darwin link flags so the extension-module cdylib links under plain cargo
build off maturin. Prep for the kernel→bindings edge (SMA-409).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: PyO3 binding that calls the kernel (compile-level proof)

**Files:**
- Modify: `rs/crates/bindings/paigasus-py-bindings/Cargo.toml`
- Modify: `rs/crates/bindings/paigasus-py-bindings/src/lib.rs`

- [ ] **Step 1: Update the crate manifest**

Replace `rs/crates/bindings/paigasus-py-bindings/Cargo.toml` with:

```toml
[package]
name = "paigasus-py-bindings"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
publish = false

[lib]
# Only cdylib: Python loads this artifact; no other Rust crate consumes it.
crate-type = ["cdylib"]
# A PyO3 `extension-module` cdylib leaves libpython symbols undefined, so a Rust test
# harness for this target can't link. Kernel logic is unit-tested in `paigasus-kernel`; the
# FFI boundary is proven by compilation. Disable the (un-linkable, empty) test/doctest
# targets so `cargo nextest --no-tests=pass` stays green (SMA-409).
test = false
doctest = false

[dependencies]
pyo3.workspace = true
paigasus-kernel.workspace = true

[package.metadata.cargo-machete]
# pyo3 is consumed only through attribute macros (#[pyfunction]/#[pymodule]) — the canonical
# cargo-machete false-positive, and :machete is a blocking gate (SMA-375). paigasus-kernel is
# called directly and needs no ignore.
ignored = ["pyo3"]

[lints]
workspace = true
```

- [ ] **Step 2: Write the binding source**

Replace `rs/crates/bindings/paigasus-py-bindings/src/lib.rs` with:

```rust
// SPDX-License-Identifier: Apache-2.0

//! PyO3 binding shim for `paigasus-kernel` (ADR-0005): exposes the kernel's pure functions
//! to Python. Compiled as an `abi3` `extension-module` cdylib; packaging into a wheel and
//! wiring it into the uv workspace are a later issue. The affected-graph cascade
//! `paigasus-kernel-rs → paigasus-py-bindings-rs` is proven by this crate compiling against a
//! real `paigasus_kernel::*` call (SMA-409).

use pyo3::prelude::*;

/// Python-callable wrapper over [`paigasus_kernel::sum`], returning the result as a string
/// (the canonical PyO3 first-binding shape — a real value crossing the FFI boundary).
#[pyfunction]
fn sum_as_string(a: i64, b: i64) -> String {
    paigasus_kernel::sum(a, b).to_string()
}

/// The extension module. Its name is provisional — it will be reconciled with the
/// `paigasus-kernel-py` wrapper when the wheel-integration issue lands.
#[pymodule]
fn paigasus_py_bindings(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(sum_as_string, m)?)?;
    Ok(())
}
```

- [ ] **Step 3: Verify it compiles (the cascade's compile-level proof)**

Run: `moon run paigasus-py-bindings-rs:build`
Expected: PASS — `cargo build` compiles the `extension-module` cdylib. (On Linux, undefined libpython symbols are allowed; on macOS the Task 2 link flags apply.)

If the PyO3 build script can't find a Python interpreter, set `PYO3_PYTHON="$(command -v python3)"` and retry; `moon setup` provides python 3.12 on PATH in CI.

- [ ] **Step 4: Verify lint, format, unused-deps all green**

Run: `moon run paigasus-py-bindings-rs:lint paigasus-py-bindings-rs:fmt paigasus-py-bindings-rs:test repo:machete`
Expected: all PASS. `:test` is green because `test = false` means no test binary is built (`--no-tests=pass`). `repo:machete` is green because of the `ignored = ["pyo3"]` metadata; `paigasus-kernel` is used directly.
If clippy flags PyO3 macro-generated code under `-D warnings`, add the minimal `#[allow(clippy::…)]` named in the error (PyO3 0.29 is normally clippy-clean).

- [ ] **Step 5: Commit**

```bash
git add rs/crates/bindings/paigasus-py-bindings/Cargo.toml rs/crates/bindings/paigasus-py-bindings/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(rs): bind paigasus-kernel::sum to Python via PyO3

Adds a #[pyfunction]/#[pymodule] that genuinely calls paigasus_kernel::sum, making
the kernel→bindings dependency real (cargo-machete forbids a dead use). Built as an
abi3 extension-module cdylib; the FFI boundary is proven by compilation, the kernel
logic by its Rust unit test. Disables the un-linkable test target; ignores pyo3 for
cargo-machete (macro-only use). Runtime/wheel proof is deferred (SMA-409).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: The Moon edge — make the kernel edit cascade to the binding

**Files:**
- Modify: `rs/crates/bindings/paigasus-py-bindings/moon.yml`

- [ ] **Step 1: Confirm the edge is missing today (baseline)**

Run:
```bash
printf 'rs/crates/libs/paigasus-kernel/src/lib.rs\n' | moon query projects --affected --downstream deep \
  | python3 -c 'import sys,json; print(sorted(p["id"] for p in json.load(sys.stdin)["projects"]))'
```
Expected: `['paigasus-gateway-rs', 'paigasus-kernel-rs', 'repo']` — note `paigasus-py-bindings-rs` is **absent**. This is the gap.

- [ ] **Step 2: Add the `dependsOn` + `^:build` edges**

Replace `rs/crates/bindings/paigasus-py-bindings/moon.yml` with:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-py-bindings-rs'
# Moon-side layer label for this FFI crate (no native `binding` layer exists).
# Built like a library but NOT published as an rlib — ships as a Python wheel
# via maturin. Exclude from any layer=library publish matrix.
layer: 'library'
language: 'rust'

# The kernel→binding edge (ADR-0005): a kernel change must rebuild this crate. The
# task-level `^:build` is what propagates `affected` in `moon ci --include-relations`
# — a project-level `dependsOn` alone does NOT (SMA-389 D3). Mirrors paigasus-gateway-rs.
dependsOn:
  - 'paigasus-kernel-rs'

tasks:
  build:
    deps: ['^:build']
  test:
    deps: ['^:build']
```

- [ ] **Step 3: Verify the cascade now resolves**

Run:
```bash
printf 'rs/crates/libs/paigasus-kernel/src/lib.rs\n' | moon query projects --affected --downstream deep \
  | python3 -c 'import sys,json; print(sorted(p["id"] for p in json.load(sys.stdin)["projects"]))'
```
Expected: `['paigasus-gateway-rs', 'paigasus-kernel-rs', 'paigasus-py-bindings-rs', 'repo']` — `paigasus-py-bindings-rs` now appears. The cascade works.

- [ ] **Step 4: Verify the binding still builds through the new edge**

Run: `moon run paigasus-py-bindings-rs:build`
Expected: PASS, and the run also builds `paigasus-kernel-rs` first (the `^:build` dep).

- [ ] **Step 5: Commit**

```bash
git add rs/crates/bindings/paigasus-py-bindings/moon.yml
git commit -m "$(cat <<'EOF'
feat(rs): declare the kernel→py-bindings Moon edge so kernel edits cascade

Adds dependsOn: [paigasus-kernel-rs] + task-level ^:build to paigasus-py-bindings,
mirroring paigasus-gateway-rs. A kernel touch now resolves paigasus-py-bindings-rs
into the affected set under moon ci --include-relations (verified via moon query).
Closes the one missing cross-language cascade edge (SMA-409).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: The regression guard script

**Files:**
- Create: `ci/affected-graph/run.sh`
- Create: `ci/affected-graph/README.md`

- [ ] **Step 1: Write the guard harness**

Create `ci/affected-graph/run.sh`:

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# SMA-409 — affected-graph regression guard.
#
# `moon ci` USES the affected graph but never ASSERTS it is correct, so a deleted
# dependsOn edge (or a dropped `moon ci --include-relations`) silently under-builds and
# stays green. This guard feeds a synthetic touched-file to `moon query projects
# --affected --downstream deep` and asserts the resulting project set per known case, so
# such a regression fails red. See
# docs/superpowers/specs/2026-06-14-sma-409-affected-graph-cascade-guard-design.md.
#
# usage: run.sh [--negative-control]
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
CI_YML="$REPO_ROOT/.github/workflows/ci.yml"
NEGATIVE=0
[ "${1-}" = "--negative-control" ] && NEGATIVE=1

# affected_ids FILE -> newline-sorted project ids, minus `repo` (its source is the repo
# root `.`, so it owns every file and appears for every touch — pure noise here).
affected_ids() { # file
  printf '%s\n' "$1" \
    | moon query projects --affected --downstream deep \
    | python3 -c 'import sys,json; print("\n".join(sorted(p["id"] for p in json.load(sys.stdin)["projects"] if p["id"] != "repo")))'
}

# assert_case LABEL FILE MUST_INCLUDE_CSV FORBID_REGEX
#   MUST_INCLUDE_CSV : comma-separated project ids that MUST be present (positive superset)
#   FORBID_REGEX     : extended regex; any matching id present = cross-stack leak (empty = skip)
# returns 0 pass / 1 assertion fail / 2 infrastructure error
assert_case() {
  local label="$1" file="$2" inc="$3" forbid="$4" got rc=0 p leaked
  got="$(affected_ids "$file")" || { echo "FATAL [$label]: moon query failed" >&2; return 2; }
  for p in ${inc//,/ }; do
    grep -qx "$p" <<<"$got" || { echo "FAIL  [$label] missing expected project: $p" >&2; rc=1; }
  done
  if [ -n "$forbid" ]; then
    leaked="$(grep -E "$forbid" <<<"$got" || true)"
    [ -z "$leaked" ] || { echo "FAIL  [$label] cross-stack leak: $(tr '\n' ' ' <<<"$leaked")" >&2; rc=1; }
  fi
  [ "$rc" = 0 ] && printf 'PASS  %-18s -> %s\n' "$label" "$(tr '\n' ' ' <<<"$got")"
  return "$rc"
}

# Every `moon ci` invocation in ci.yml must carry --include-relations: it is the flag that
# activates relation/dependent rebuilds. The edges are inert without it, so guarding the
# edges but not the flag would leave a hole (SMA-409 review F1).
assert_include_relations() {
  local bad
  bad="$(grep -nE '\bmoon ci\b' "$CI_YML" | grep -v -- '--include-relations' || true)"
  if [ -n "$bad" ]; then
    echo "FAIL  [ci-include-relations] a 'moon ci' invocation lacks --include-relations:" >&2
    printf '%s\n' "$bad" >&2
    return 1
  fi
  printf 'PASS  %-18s -> every `moon ci` carries --include-relations\n' "ci-include-relations"
}

run_suite() {
  local rc=0
  # contracts proto edit -> proto packages in all three languages + the gateway rebuild.
  assert_case "contracts->proto" "contracts/proto/paigasus/gateway/v1/health.proto" \
    "contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs" "" || rc=1
  # kernel edit -> kernel + binding + gateway; nothing cross-stack (no *-py / *-ts / contracts).
  assert_case "kernel->bindings" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs" \
    '(-py|-ts)$|^contracts$|^py$|^ts$' || rc=1
  # binding edit -> only the binding; the edge is one-directional (must not drag in the kernel).
  assert_case "binding-oneway"   "rs/crates/bindings/paigasus-py-bindings/src/lib.rs" \
    "paigasus-py-bindings-rs" '^paigasus-kernel-rs$' || rc=1
  assert_include_relations || rc=1
  return "$rc"
}

if [ "$NEGATIVE" = 1 ]; then
  echo "== negative control: assert a deliberately-wrong expectation reports red =="
  # paigasus-kernel-py is NOT a dependent of the kernel crate, so requiring it MUST fail.
  rc=0
  assert_case "neg-wrong-expect" "rs/crates/libs/paigasus-kernel/src/lib.rs" "paigasus-kernel-py" "" || rc=$?
  case "$rc" in
    1) echo "negative-control OK: harness reported red as expected"; exit 0 ;;
    0) echo "negative-control FAILED: harness accepted a wrong expectation" >&2; exit 1 ;;
    *) echo "negative-control INCONCLUSIVE: infrastructure error (rc=$rc)" >&2; exit 2 ;;
  esac
fi

if run_suite; then
  echo "== affected-graph cascade intact =="
else
  echo "== affected-graph REGRESSION (see FAILs above) ==" >&2
  exit 1
fi
```

- [ ] **Step 2: Make it executable**

Run: `chmod +x ci/affected-graph/run.sh`

- [ ] **Step 3: Run the guard — expect green (the graph is wired from Task 4)**

Run: `ci/affected-graph/run.sh`
Expected: three `PASS` case lines + `PASS ci-include-relations` + `== affected-graph cascade intact ==`, exit 0.

- [ ] **Step 4: Prove the guard actually catches a regression (red on a removed edge)**

```bash
# Temporarily delete the kernel→binding edge, run the guard, confirm it goes red, then restore.
cp rs/crates/bindings/paigasus-py-bindings/moon.yml /tmp/pyb-moon.bak
# strip the dependsOn block (kernel-rs) to simulate a regression:
python3 - <<'PY'
import re, pathlib
p = pathlib.Path("rs/crates/bindings/paigasus-py-bindings/moon.yml")
s = p.read_text()
s = re.sub(r"\ndependsOn:\n(?:  - .*\n)+", "\n", s)
p.write_text(s)
PY
ci/affected-graph/run.sh; echo "guard exit: $?"   # expect FAIL on kernel->bindings, exit 1
cp /tmp/pyb-moon.bak rs/crates/bindings/paigasus-py-bindings/moon.yml   # restore
ci/affected-graph/run.sh; echo "guard exit: $?"   # expect green again, exit 0
```
Expected: first run prints `FAIL  [kernel->bindings] missing expected project: paigasus-py-bindings-rs` and exits 1; after restore it is green and exits 0. (Confirm `git status` shows the moon.yml unchanged after restore.)

- [ ] **Step 5: Sanity-check the negative-control mode**

Run: `ci/affected-graph/run.sh --negative-control`
Expected: `negative-control OK: harness reported red as expected`, exit 0.

- [ ] **Step 6: Write the README**

Create `ci/affected-graph/README.md`:

```markdown
# affected-graph regression guard (SMA-409)

`moon ci` *uses* the affected graph but never *asserts* it is correct: a deleted
`dependsOn` edge — or a dropped `moon ci --include-relations` — makes the affected set
silently shrink, so CI under-builds and stays **green**. This guard closes that gap.

`run.sh` feeds a synthetic touched-file to `moon query projects --affected --downstream
deep` and asserts the affected project set per known case (`repo`, which owns the whole
tree as its source, is filtered out):

- **contracts edit** → `contracts` + `paigasus-proto-{rs,py,ts}` + `paigasus-gateway-rs`.
- **kernel edit** → `paigasus-kernel-rs` + `paigasus-py-bindings-rs` + `paigasus-gateway-rs`,
  and **nothing cross-stack** (no `*-py` / `*-ts` / `contracts`).
- **binding edit** → only `paigasus-py-bindings-rs` (the edge is one-directional).

It also asserts every `moon ci` invocation in `.github/workflows/ci.yml` carries
`--include-relations` (the edges are inert without it).

Run locally: `moon run repo:affected-smoke` (or `ci/affected-graph/run.sh`).
Prove it can fail: `ci/affected-graph/run.sh --negative-control`.

## Maintenance — the must-exclude assertions are topology-coupled (SMA-409 F5)

The **must-include** sets are durable. The **must-exclude** (cross-stack-isolation)
assertions hold only because the py/ts kernel wrappers are deferred. When the deferred
uv↔maturin integration lands and `paigasus-kernel-py` genuinely wraps the wheel, a kernel
edit *should* affect the py wrapper — and the `kernel->bindings` forbid-regex here will
correctly need loosening. A failure there is the expected next edge, not a regression;
update this guard alongside each deferred binding.
```

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/run.sh ci/affected-graph/README.md
git commit -m "$(cat <<'EOF'
ci(repo): add the affected-graph regression guard harness

ci/affected-graph/run.sh feeds synthetic touched-files to `moon query projects
--affected --downstream deep` and asserts the cascade per case (contracts->proto,
kernel->bindings, binding one-way), plus that every ci.yml `moon ci` carries
--include-relations. Verified it goes red on a removed edge and via --negative-control
(SMA-409, review F1/F5).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Wire the guard into Moon + CI

**Files:**
- Modify: `moon.yml` (root `repo` project)
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add the `affected-smoke` task to the root project**

In `moon.yml`, inside `tasks:`, add (after the `release-parity-ts` task):

```yaml
  affected-smoke:
    description: 'Assert the cross-language affected graph still cascades; fail red on a deleted edge or a dropped --include-relations (SMA-409).'
    script: 'ci/affected-graph/run.sh'
    toolchain: 'system'
    inputs:
      - 'ci/affected-graph/**/*'
      - '.github/workflows/ci.yml'
      - '.moon/**/*'
      - 'moon.yml'
      - '*/moon.yml'
      - 'rs/crates/*/*/moon.yml'
      - 'py/packages/*/moon.yml'
      - 'ts/packages/*/moon.yml'
      - 'ts/apps/*/moon.yml'
      - 'rs/**/Cargo.toml'
      - 'py/packages/*/pyproject.toml'
      - 'ts/packages/*/package.json'
      - 'ts/apps/*/package.json'
```

- [ ] **Step 2: Verify the task runs via Moon**

Run: `moon run repo:affected-smoke`
Expected: PASS (same output as `ci/affected-graph/run.sh` in Task 5; confirms the nested `moon query` works inside a Moon task).

- [ ] **Step 3: Add `:affected-smoke` to the CI task array**

In `.github/workflows/ci.yml`, change the task array line (currently):

```bash
          T=(:build :test :lint :fmt :deny :machete :typecheck :breaking :release-parity :release-parity-py :release-parity-ts)
```
to:
```bash
          T=(:build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :release-parity :release-parity-py :release-parity-ts)
```

- [ ] **Step 4: Verify the target resolves and the guard is selected when graph config changes**

Run: `moon ci :affected-smoke --base origin/main --include-relations`
Expected: resolves `:affected-smoke` to `repo:affected-smoke` (the only project defining it) and runs it green. (This PR touches `moon.yml`/`Cargo.toml`/`ci.yml`, all in the task's `inputs`, so it is affected and runs.)

- [ ] **Step 5: Commit**

```bash
git add moon.yml .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
ci(repo): run the affected-graph guard in CI as repo:affected-smoke

Adds the repo:affected-smoke Moon task (release-parity pattern) gated on the
graph-defining inputs (moon.yml files, manifests, .moon/**, ci.yml, the script) and
lists :affected-smoke in the moon ci task array. The cascade wiring is now
continuously protected, not certified once (SMA-409 AC #4).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Full verification sweep

**Files:** none (verification only)

- [ ] **Step 1: Run the affected build/test/lint surface for the touched projects**

Run:
```bash
moon run paigasus-kernel-rs:build paigasus-kernel-rs:test paigasus-kernel-rs:lint paigasus-kernel-rs:fmt \
         paigasus-py-bindings-rs:build paigasus-py-bindings-rs:test paigasus-py-bindings-rs:lint paigasus-py-bindings-rs:fmt
```
Expected: all PASS.

- [ ] **Step 2: Run the workspace-wide gates the change touches**

Run: `moon run repo:deny repo:machete repo:affected-smoke`
Expected: all PASS (deny: licenses OK; machete: no unused deps thanks to the pyo3 ignore + real kernel use; affected-smoke: cascade intact).

- [ ] **Step 3: Simulate the PR-path CI selection end-to-end**

Run:
```bash
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
        :release-parity :release-parity-py :release-parity-ts --base origin/main --include-relations
```
Expected: green. Confirm the affected set includes `paigasus-kernel-rs`, `paigasus-py-bindings-rs`, `paigasus-gateway-rs`, and `repo:affected-smoke` for this branch's diff.

- [ ] **Step 4: Confirm the working tree is clean and the branch diff is the intended set**

Run: `git status -s && git diff --name-status main...HEAD`
Expected: clean tree; changed files limited to the spec + plan docs and the files in this plan's File Structure (kernel lib, rs/Cargo.toml, rs/.cargo/config.toml, the binding crate's Cargo.toml/src/moon.yml, ci/affected-graph/*, moon.yml, ci.yml).

- [ ] **Step 5: Push and open the PR (only when asked)**

```bash
git push -u origin feature/sma-409-wire-cross-language-affected-graph-cascade-re-verify-at
gh pr create --fill --base main
```
The PR auto-links to SMA-409 by branch name (do not attach the Linear link manually). Confirm CI is green, especially the `affected-smoke` step.

---

## Self-Review (author check)

**Spec coverage:**
- Real kernel fn + PyO3 binding (spec §1) → Tasks 1–3.
- Moon `dependsOn` + `^:build` edge (spec §2) → Task 4.
- Regression guard: script + `repo:affected-smoke`, synthetic injection, `repo` filtered, positive-superset + forbid-regex negatives, `--include-relations` assertion, graph-defining inputs (spec §3, incl. F1/F5) → Tasks 5–6.
- cargo-machete `ignored = ["pyo3"]` (spec §1 / F3) → Task 3.
- Compile-level proof, runtime smoke deferred (spec decision #2 / F4) → Tasks 3–4 (no maturin anywhere).
- Verification → ACs (spec) → Task 7.

**Placeholder scan:** none — every step has concrete code/commands/expected output.

**Type/name consistency:** `paigasus_kernel::sum` defined in Task 1, called in Task 3; project id `paigasus-py-bindings-rs`, task `repo:affected-smoke`, and the `moon query projects --affected --downstream deep` invocation are identical across Tasks 4–7 and the script.

---

## As-built deltas (implementation)

Discoveries during execution that diverged from the task text above. Recorded so the plan
stays honest; the committed code is the source of truth.

- **D1 — Task 3 also touched `rs/Cargo.lock` and `rs/Cargo.toml`.** Building the binding
  pulls PyO3 into `rs/Cargo.lock`, so that lockfile is part of the commit (the original
  `git add` list omitted it). Separately, `cargo-deny`'s `wildcards = "deny"` rejects a
  *path-only* workspace dep once it is actually consumed, so the `paigasus-kernel` workspace
  entry gained `version = "0.0.0"` (matching the crate's own version) — fixing the root cause
  rather than weakening `deny.toml`.
- **D2 — Task 5 guard `--include-relations` matcher tightened, + infra-error distinction.**
  The first-draft `grep -E '\bmoon ci\b'` false-matched the workflow's job name
  (`name: moon ci`), step name, and comments — and an attempt to work around it by *renaming*
  the job would have broken the `CI / moon ci` required status check. The committed matcher is
  `grep -E 'moon ci +"'` (only real `moon ci "${T[@]}"` shell invocations) plus a
  "no invocation found → FAIL" fail-safe. Also added a `run_case` helper so an infrastructure
  error (e.g. `moon query` dying) aborts with exit 2, distinct from an assertion failure
  (exit 1) — mirroring `ci/release-parity/run.sh`. All three exit codes were verified.
- **D3 — Task 6 commit body uses `AC4`, not `AC #4`.** commitlint's conventional parser reads
  `... #4)` as a footer token and trips `footer-leading-blank`; `AC4` preserves the meaning.
- **D4 — guard `inputs` deliberately exclude lock/workspace-root files (review decision).** A
  Task 6 quality review suggested adding `rs/Cargo.lock`, `ts/pnpm-workspace.yaml`,
  `py/pyproject.toml`, etc. Declined: Moon discovers projects from `.moon/workspace.yml`
  `projects.globs` (covered by `.moon/**/*`) and the cascade topology from `moon.yml`
  `dependsOn` edges (covered by the `moon.yml` globs) — lock files and pnpm/uv workspace-root
  files do **not** change what `moon query projects --affected` returns (verified: a
  `rs/Cargo.lock` touch resolves to just `repo`). Adding them would re-run the guard on every
  Dependabot bump for zero added protection. The current inputs already cover every real
  regression vector (a removed `dependsOn`, a changed project glob, a dropped
  `--include-relations`).
