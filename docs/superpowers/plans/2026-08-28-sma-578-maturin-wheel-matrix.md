# SMA-578 — maturin wheel matrix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `paigasus-py-bindings` and `paigasus-kernel` as installable PyPI artifacts — a seven-wheel cross-platform matrix plus a source-verified sdist — with PyPI packaging metadata gated the way crates.io metadata already is.

**Architecture:** A new reusable `.github/workflows/wheels.yml` builds seven `cp312-abi3` wheels across six runner legs (maturin, with zig retargeting glibc on the four Linux legs) and verifies each against its *binary*, not just its filename tag. A separate three-platform job proves the sdist is a real fallback by installing it from source. `repo:publish-metadata` grows a Python arm carrying spelling-level checks only; behavioural artifact assertions live in the workflow that already builds the artifacts.

**Tech Stack:** maturin 1.9.6 (proto-pinned), PyO3 0.29 `abi3-py312`, cargo-zigbuild via maturin `--zig`, GitHub Actions, Moon 2.3.2, bash + Python 3.12 gates.

**Spec:** `docs/superpowers/specs/2026-08-28-sma-578-maturin-wheel-matrix-design.md`

## Global Constraints

- **Every source file opens with an SPDX header:** `// SPDX-License-Identifier: Apache-2.0` (`#` for Python, bash and YAML).
- **Rust crates are edition 2024 + `rust-version = "1.95"`.** The sdist carries no `rust-toolchain.toml`, so 1.95 is the consumer-facing MSRV.
- **maturin is pinned to exactly `1.9.6`** — the version §2's consumer-path experiment was measured on. The consumer floor in `[build-system] requires` is `maturin>=1.9.6,<2`.
- **Workflow trigger filters are block sequences, never inline flow** (`branches:`, `paths:` and their `-ignore` variants). `repo:actionlint`'s extractor fails all four keys loudly on inline flow.
- **No brace expansion in any `paths:` filter or Moon `inputs:` glob.** Verified: `ci/actionlint/run.sh:1043`'s charset regex `^[A-Za-z0-9._/*-]+$` rejects `rs/Cargo.{lock,toml}` as `rejected-charset`, and `ci/affected-graph/task_inputs.py` carries a self-test row for the same shape. One entry per path.
- **An SPDX `license` expression means the `License ::` trove classifier is OMITTED**, not supplied alongside — PyPI hard-rejects the combination (SMA-378).
- **Bash tool PATH lacks the proto CLIs.** Prefix every command with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` (shims first).
- **Never bypass the git hooks with `--no-verify`.** The worktree is provisioned; commitlint runs on `commit-msg`.
- **Commit subjects start lowercase, are ≤100 chars, and carry a workspace scope.** No `#NNN` issue refs in the body (it breaks `footer-leading-blank`); write "owner/repo PR NNN".
- **Exit-code contract for all `ci/**` gates:** `0` pass, `1` assertion failed (the repo is wrong), `2` infrastructure failed. A broken invocation must never read as "all checks passed".

---

### Task 1: `paigasus-py-bindings` packaging — metadata, LICENSE/README, lint table, `include`

**Files:**
- Modify: `rs/crates/bindings/paigasus-py-bindings/pyproject.toml`
- Modify: `rs/crates/bindings/paigasus-py-bindings/Cargo.toml`
- Create: `rs/crates/bindings/paigasus-py-bindings/LICENSE`
- Create: `rs/crates/bindings/paigasus-py-bindings/README.md`

**Interfaces:**
- Consumes: nothing.
- Produces: the `[tool.paigasus] pypi = true` marker key that Task 6's P0 discovery reads; the `LICENSE`/`README.md` paths that Task 6's P2 and the gate's new `inputs:` entries name; the `include` allowlist that Task 5's `moon.yml`-absence assertion depends on.

**Why the lint table matters (spec §7.3, review B2):** `Cargo.toml:30-31` is `[lints] workspace = true` and `rs/Cargo.toml:241-242` is `[workspace.lints.rust] warnings = "deny"`. **Measured:** the sdist ships the workspace `Cargo.toml` verbatim, that table included, so every sdist consumer compiles this crate as the root package where `--cap-lints allow` does not apply. The first new rustc lint then breaks `pip install` for every sdist consumer, on an already-published version. `paigasus-kernel` was hardened for this; this crate was not.

- [ ] **Step 1: Capture the baseline the change must move**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$HOME/.cargo/bin:$PATH"
SP=/tmp/sma578 && mkdir -p "$SP"
uvx maturin@1.9.6 build --interpreter python3.12 \
  -m rs/crates/bindings/paigasus-py-bindings/Cargo.toml --out "$SP/before"
unzip -p "$SP"/before/*.whl '*.dist-info/METADATA' | wc -c
```

Expected: a small number in the 80–100 byte range (88 at the time of writing). Record it — Step 9 asserts it grew.

- [ ] **Step 2: Write the LICENSE and README**

`LICENSE` is a verbatim copy of the repo root's Apache-2.0 text:

```bash
cp LICENSE rs/crates/bindings/paigasus-py-bindings/LICENSE
```

`README.md` — the MSRV line is load-bearing (spec §7.4): the sdist carries no `rust-toolchain.toml`, so a consumer on older rustc fails mid-`pip install` with a bare cargo error.

```markdown
# paigasus-py-bindings

PyO3 bindings for the Paigasus behavioral kernel — PRN canonicalization and UUIDv7
minting, implemented once in Rust (`paigasus-kernel`) and bound to Python, Node and
WebAssembly (ADR-0005).

Most users want [`paigasus-kernel`](https://pypi.org/project/paigasus-kernel/), the
Python-facing wrapper, rather than this extension module directly.

## Installation

Wheels are published for CPython 3.12+ on macOS (arm64, x86_64), Windows (x86_64) and
Linux (x86_64 and aarch64, glibc and musl). They are `abi3` wheels, so one wheel per
platform covers every CPython from 3.12 onward.

## Building from source

The source distribution builds on any platform with a Rust toolchain:

    pip install paigasus-py-bindings --no-binary paigasus-py-bindings

**Minimum supported Rust version: 1.95** (the crate is edition 2024). The sdist
deliberately ships no `rust-toolchain.toml`, so your installed toolchain is what builds
it; an older rustc fails during `pip install` with a cargo error.

## License

Apache-2.0. See [LICENSE](./LICENSE).
```

- [ ] **Step 3: Rewrite `pyproject.toml`**

Replace the whole file. Note there is **no** `Operating System :: OS Independent` classifier — this is a platform-specific extension module, unlike `paigasus-proto`. The `NOTE (publish deferred)` caveat is replaced with what was measured (spec §10 correction 1).

```toml
[project]
name = "paigasus-py-bindings"
version = "0.1.0"
description = "PyO3 bindings for the Paigasus behavioral kernel."
readme = "README.md"
license = "Apache-2.0"
license-files = ["LICENSE"]
authors = [{ name = "Paigasus contributors" }]
requires-python = ">=3.12"
classifiers = [
  "Programming Language :: Python :: 3",
  "Programming Language :: Python :: 3 :: Only",
  "Programming Language :: Rust",
  "Intended Audience :: Developers",
  "Topic :: Software Development :: Libraries",
  "Typing :: Typed",
]

# PyPI-bound. This marker — NOT the version field — is what repo:publish-metadata's
# Python arm reads to decide the publishable set (SMA-578 review M7): in this repo
# `version` means "in a lockstep family" (repo:version-lockstep writes it), and this
# crate is simultaneously `publish = false` on the Cargo side and PyPI-bound.
[tool.paigasus]
pypi = true

# Floor raised from 1.7 to the version the sdist's macOS build was actually MEASURED on
# (SMA-578 §2/review M2). An sdist consumer resolving maturin 1.7 would build on a
# version that behaviour was never verified against.
[build-system]
requires = ["maturin>=1.9.6,<2"]
build-backend = "maturin"

[tool.maturin]
# Cargo.toml is co-located (same dir) → no manifest-path. Keeping this pyproject INSIDE
# rs/ means maturin runs cargo from within rs/, so rs/.cargo/config.toml's apple-darwin
# link flags resolve and the extension-module cdylib links on macOS without maturin
# (SMA-419; Polyglot Monorepo Scoping §1/§3 co-located fallback).
#
# MEASURED 2026-08-28 (SMA-578 §2), correcting the caveat that stood here: a published
# sdist does NOT need rs/.cargo/config.toml. The sdist was extracted to a directory with
# no .cargo/config.toml anywhere on cargo's upward walk and `maturin build` linked
# cleanly on macOS — maturin supplies the `-undefined dynamic_lookup` arguments itself.
# The control: plain `cargo build` in that same directory FAILS with undefined _Py*
# symbols, which is exactly what rs/.cargo/config.toml exists for ("WITHOUT maturin").
# The sdist is therefore a supported install path, verified per-platform in
# .github/workflows/wheels.yml.
module-name = "paigasus_py_bindings"
```

- [ ] **Step 4: Add the `include` allowlist and the crate's own lint table to `Cargo.toml`**

Add `include` directly after `publish = false`:

```toml
# Sdist allowlist. maturin builds the sdist from `cargo package --list`, so THIS list is
# what controls its contents — MEASURED 2026-08-28: without it the sdist ships moon.yml,
# the same repo-internal leak repo:publish-metadata Check 2b catches on the Cargo side.
# Checks 1d/2b/2c do not reach this crate (it is `publish = false`), so the assertion
# that holds this list honest lives in .github/workflows/wheels.yml, not in the gate.
# Membership is literal: never "**/*", which would reinstate the leak.
include = [
  "src/**/*.rs",
  "Cargo.toml",
  "pyproject.toml",
  "paigasus_py_bindings.pyi",
  "README.md",
  "LICENSE",
]
```

Replace the trailing `[lints]` block:

```toml
# NOT `workspace = true`. maturin ships the workspace Cargo.toml verbatim in the sdist
# (measured), including `[workspace.lints.rust] warnings = "deny"`, and an sdist consumer
# builds this crate as the ROOT package, where `--cap-lints allow` does not apply. An
# inherited deny would make the first new rustc lint break `pip install` from sdist for
# every macOS/Windows/musl user, on an already-published version. Same reasoning, and the
# same fix, as paigasus-kernel's own table (SMA-577 Check 1c). CI strictness is
# unaffected: the Moon `lint` task passes `-D warnings` explicitly.
[lints.rust]
warnings = "warn"

[lints.clippy]
all = "warn"
```

- [ ] **Step 5: Verify the sdist no longer leaks `moon.yml` and now carries the docs**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$HOME/.cargo/bin:$PATH"
uvx maturin@1.9.6 sdist -m rs/crates/bindings/paigasus-py-bindings/Cargo.toml --out /tmp/sma578/sdist
tar tzf /tmp/sma578/sdist/paigasus_py_bindings-0.1.0.tar.gz | sort
```

Expected: **no** `moon.yml` anywhere; `crates/bindings/paigasus-py-bindings/README.md` and `.../LICENSE` present; `pyproject.toml` at the archive **root** (maturin relocates it — measured).

- [ ] **Step 6: Verify the crate no longer inherits `warnings = "deny"`**

```bash
python3 - <<'PY'
import tomllib
m = tomllib.load(open("rs/crates/bindings/paigasus-py-bindings/Cargo.toml","rb"))
lints = m.get("lints", {})
assert "workspace" not in lints, "still inheriting the workspace lint table"
assert lints["rust"]["warnings"] == "warn", lints
print("OK: own non-denying lint table")
PY
```

Expected: `OK: own non-denying lint table`

- [ ] **Step 7: Verify the crate still builds and clippy is still clean**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$HOME/.cargo/bin:$PATH"
( cd rs && cargo clippy -p paigasus-py-bindings -- -D warnings )
( cd rs && cargo fmt --check )
```

Expected: both exit 0. This is what proves the lint change did not weaken CI — `-D warnings` on the command line still denies.

- [ ] **Step 8: Verify the wheel's metadata actually grew**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$HOME/.cargo/bin:$PATH"
uvx maturin@1.9.6 build --interpreter python3.12 \
  -m rs/crates/bindings/paigasus-py-bindings/Cargo.toml --out /tmp/sma578/after
unzip -p /tmp/sma578/after/*.whl '*.dist-info/METADATA'
unzip -l /tmp/sma578/after/*.whl | grep -c 'dist-info/licenses/LICENSE'
```

Expected: `METADATA` now carries `License-Expression: Apache-2.0`, `Summary:`, `Description-Content-Type:` and six `Classifier:` lines (versus the ~88-byte baseline from Step 1); the licenses grep prints `1`. If the licenses file is absent, maturin is not honouring PEP 639 `license-files` — record what it does instead and adjust Task 4's METADATA assertion to match reality rather than forcing this shape.

- [ ] **Step 9: Commit**

```bash
git add rs/crates/bindings/paigasus-py-bindings/
git commit -m "feat(rs): give paigasus-py-bindings real PyPI metadata and an sdist allowlist (SMA-578)"
```

---

### Task 2: SMA-556 — `paigasus-ml` and `paigasus-workflows` packaging

**Files:**
- Create: `py/packages/paigasus-ml/LICENSE`, `py/packages/paigasus-ml/README.md`
- Create: `py/packages/paigasus-workflows/LICENSE`, `py/packages/paigasus-workflows/README.md`
- Modify: `py/packages/paigasus-ml/pyproject.toml`, `py/packages/paigasus-workflows/pyproject.toml`

**Interfaces:**
- Consumes: nothing.
- Produces: the four files `.moon/tasks/python-project.yml:27` already declares as `build` inputs but which do not exist; the `Private :: Do Not Upload` classifier that keeps both packages out of Task 6's P0 set.

**Context:** `.moon/tasks/python-project.yml:27` declares `README.md` and `LICENSE` among every python project's `build` inputs. For these two the files are absent — the only four untracked file inputs in the whole 119-task graph. Both build with `uv_build`, which does **not** auto-glob license files (SMA-378), so a published wheel would ship no license text.

- [ ] **Step 1: Confirm the defect before fixing it**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects \
  | python3 -c '
import json,sys,os
d=json.load(sys.stdin)
missing=[]
for p in d["projects"]:
    for t in (p.get("tasks") or {}).values():
        for f in (t.get("inputFiles") or {}):
            if f.startswith("py/") and not os.path.exists(f):
                missing.append(f)
print("\n".join(sorted(set(missing))) or "NONE")'
```

Expected: exactly four paths — `py/packages/paigasus-ml/{README.md,LICENSE}` and `py/packages/paigasus-workflows/{README.md,LICENSE}`. Note Moon 2.3.2 emits JSON from bare `moon query projects` and REJECTS a `--json` flag (exit 2). If the output shape differs from what this snippet assumes, adapt the reader; treat an absent `inputFiles` key as a violation, never a skip.

- [ ] **Step 2: Create the four files**

```bash
cp LICENSE py/packages/paigasus-ml/LICENSE
cp LICENSE py/packages/paigasus-workflows/LICENSE
```

`py/packages/paigasus-ml/README.md`:

```markdown
# paigasus-ml

Machine-learning utilities for Paigasus.

**Status: stub.** This package has no public API yet and is pinned at `0.0.0`; it is not
published to PyPI and carries the `Private :: Do Not Upload` classifier so an accidental
upload is refused by PyPI itself (ADR-0011 S4, dormant-until-real).

## License

Apache-2.0. See [LICENSE](./LICENSE).
```

`py/packages/paigasus-workflows/README.md`:

```markdown
# paigasus-workflows

Workflow orchestration primitives for Paigasus.

**Status: stub.** This package has no public API yet and is pinned at `0.0.0`; it is not
published to PyPI and carries the `Private :: Do Not Upload` classifier so an accidental
upload is refused by PyPI itself (ADR-0011 S4, dormant-until-real).

## License

Apache-2.0. See [LICENSE](./LICENSE).
```

- [ ] **Step 3: Add `license-files` and the `Private` classifier to both `pyproject.toml`s**

In **each** file, inside `[project]`, ensure these keys are present. Keep the existing `name`, `version` and any `description`; do **not** add a `License ::` trove classifier alongside the SPDX expression.

```toml
license = "Apache-2.0"
license-files = ["LICENSE"]
readme = "README.md"
classifiers = [
  "Private :: Do Not Upload",
]
```

`Private :: Do Not Upload` is not a registered classifier, which is exactly why it works: PyPI rejects any upload carrying an unrecognized classifier. It is the only mechanism that can stop an accidental upload — no CI gate can.

- [ ] **Step 4: Verify the graph is clean**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects \
  | python3 -c '
import json,sys,os
d=json.load(sys.stdin)
missing=[f for p in d["projects"] for t in (p.get("tasks") or {}).values()
         for f in (t.get("inputFiles") or {}) if f.startswith("py/") and not os.path.exists(f)]
assert not missing, missing
print("OK: zero untracked py inputFiles")'
```

Expected: `OK: zero untracked py inputFiles` — SMA-556's fourth acceptance criterion.

- [ ] **Step 5: Verify both still build and the license text ships**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
( cd py && uv build --package paigasus-ml --out-dir /tmp/sma578/ml )
unzip -l /tmp/sma578/ml/*.whl | grep -i licen
```

Expected: the build succeeds and the wheel lists a `LICENSE` under `.dist-info/licenses/`. Repeat for `paigasus-workflows`.

- [ ] **Step 6: Commit**

```bash
git add py/packages/paigasus-ml py/packages/paigasus-workflows
git commit -m "fix(py): give the ml and workflows stubs real LICENSE and README files (SMA-556)"
```

---

### Task 3: Pin maturin via proto

**Files:**
- Create: `.proto/plugins/maturin.toml`
- Modify: `.prototools`

**Interfaces:**
- Consumes: nothing.
- Produces: a `maturin` shim resolving exactly `1.9.6`, which Tasks 4 and 5 invoke as a bare `maturin` in workflow steps.

**Context:** `.prototools` pins ten CLIs behind vendored TOML plugins, and `release.yml:71-78` states the doctrine: install the pinned binary directly rather than through a third-party Action that resolves its own unpinned build. maturin publishes standalone release binaries, so it takes the same shape. The template below follows `.proto/plugins/wasm-pack.toml` — likewise a Rust-ecosystem tool whose binary nests one directory deep.

- [ ] **Step 1: Confirm the asset names before writing the schema**

```bash
curl -sL https://api.github.com/repos/PyO3/maturin/releases/tags/v1.9.6 \
  | python3 -c 'import json,sys; [print(a["name"]) for a in json.load(sys.stdin)["assets"]]'
```

Expected: a list including `maturin-x86_64-unknown-linux-musl.tar.gz`, `maturin-aarch64-unknown-linux-musl.tar.gz`, `maturin-x86_64-apple-darwin.tar.gz`, `maturin-aarch64-apple-darwin.tar.gz`, `maturin-x86_64-pc-windows-msvc.zip`. **Write the plugin from what this prints, not from the template below** — if the names differ (no `v` prefix in the filename, a `.zip` on macOS, a nested directory), adjust `download-file` and `exe-path` to match. A wrong schema fails at `proto install` with a 404, which is loud but wastes a cycle.

- [ ] **Step 2: Write `.proto/plugins/maturin.toml`**

```toml
# Vendored proto TOML plugin for maturin (SMA-578).
#
# Resolves official PyO3/maturin GitHub release tarballs. Same vendoring rationale as
# wasm-pack/cargo-machete: a static schema over official release assets. maturin builds the
# PyO3 extension wheels and the sdist in .github/workflows/wheels.yml.
#
# Version is pinned to the exact release SMA-578 §2's consumer-path experiment was measured
# on. The consumer-facing floor in the crate's [build-system] requires must not drop below
# it either — an sdist consumer resolving an older maturin builds on unmeasured behaviour.
#
# The binary sits at the ARCHIVE ROOT (maturin-{target}/maturin does not exist; the tarball
# contains a bare `maturin`), so no exe-path nesting is needed — unlike wasm-pack. Tags are
# "v"-prefixed (v1.9.6) but asset filenames do NOT embed the version. Linux is symmetric
# musl (both x86_64 and aarch64 assets exist), so {arch} works as-is.

name = "maturin"
type = "cli"

[platform.linux]
download-file = "maturin-{arch}-unknown-linux-musl.tar.gz"

[platform.macos]
download-file = "maturin-{arch}-apple-darwin.tar.gz"

[platform.windows]
download-file = "maturin-x86_64-pc-windows-msvc.zip"
exe-path = "maturin.exe"

[install]
download-url = "https://github.com/PyO3/maturin/releases/download/v{version}/{download_file}"

[resolve]
git-url = "https://github.com/PyO3/maturin"
```

- [ ] **Step 3: Register it in `.prototools`**

Add `maturin = "1.9.6"` to the version list (alphabetical, between `lefthook` and `moon`), and to the `[plugins]` table:

```toml
maturin = "file://./.proto/plugins/maturin.toml"
```

- [ ] **Step 4: Verify the pin resolves**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
proto install maturin
maturin --version
```

Expected: `maturin 1.9.6`. Anything else — including a 404 during install — means the schema in Step 2 is wrong; fix it from the Step 1 output.

- [ ] **Step 5: Commit**

```bash
git add .prototools .proto/plugins/maturin.toml
git commit -m "build(repo): pin maturin 1.9.6 via a vendored proto plugin (SMA-578)"
```

---

### Task 4: `wheels.yml` — the six-leg build matrix

**Files:**
- Create: `.github/workflows/wheels.yml`

**Interfaces:**
- Consumes: Task 1's `pyproject.toml` metadata and `include`; Task 3's pinned `maturin` shim.
- Produces: a workflow callable as `uses: ./.github/workflows/wheels.yml` (SMA-579 consumes it), and seven uploaded artifacts named `wheel-<platform>`.

**Two rules that govern every assertion in this task:**

1. **Exact-equality, never substring.** `prebuild.yml`'s `lipo -archs` lesson: `grep -q x86_64` passes for a universal binary, i.e. is vacuously green in precisely the case worth catching.
2. **A tag is not a binary.** Tags come from `MACOSX_DEPLOYMENT_TARGET`/sysconfig/the requested compatibility, not from the artifact — so a tag assertion alone cannot catch a wheel that installs and then fails at import.

- [ ] **Step 1: Write the workflow header and triggers**

Note the PR filter is **narrow**, and deliberately so: `prebuild.yml:19-25,37-41` documents why `rs/**` is absent from its PR trigger (SMA-520 — "a macOS job on every one of them would raise the bill"). Broad coverage lives on the push trigger.

```yaml
# SPDX-License-Identifier: Apache-2.0
name: wheels

# Builds the PyO3 extension wheels + sdist for paigasus-py-bindings, and the pure-Python
# wheel for its paigasus-kernel face. NOTHING here publishes: SMA-579's gated `release`
# job consumes this workflow via `uses:` and does the uploading.
#
# DECISION (SMA-578 D6): this workflow must NEVER declare `secrets:` or
# `id-token: write`. It carries a `pull_request` trigger, and same-repo PRs receive
# repository secrets — putting a registry credential here is the exfiltration hazard
# SMA-407 §7 review M2 exists to prevent. repo:publish-metadata asserts this.
on:
  workflow_call:

  workflow_dispatch:

  # PRE-merge verification of the narrow set of inputs that can break a wheel build.
  # `rs/**` is DELIBERATELY absent, for the reason prebuild.yml:37-41 records: most PRs
  # here touch it, and a macOS + Windows matrix on every one of them would raise the bill.
  # Not a required check (the `Protect main` ruleset requires only `moon ci`), so a
  # skipped run cannot wedge a merge.
  pull_request:
    branches:
      - main
    paths:
      - '.github/workflows/wheels.yml'
      - '.prototools'
      - '.moon/**'
      - 'rs/.cargo/config.toml'
      - 'rs/crates/bindings/paigasus-py-bindings/pyproject.toml'
      - 'rs/crates/bindings/paigasus-py-bindings/Cargo.toml'
      - 'py/packages/paigasus-kernel/pyproject.toml'

  # POST-merge verification of Rust changes — where the broad coverage lives.
  push:
    branches:
      - main
    paths:
      - 'rs/**'
      - '.github/workflows/wheels.yml'
      - '.prototools'
      - '.moon/**'

permissions:
  contents: read

concurrency:
  group: wheels-${{ github.workflow }}-${{ github.ref }}-${{ github.event_name }}
  cancel-in-progress: ${{ github.event_name != 'push' }}
```

- [ ] **Step 2: Write the build matrix**

```yaml
jobs:
  build:
    name: wheel ${{ matrix.platform }}${{ matrix.extra_platform && format(' + {0}', matrix.extra_platform) || '' }}
    runs-on: ${{ matrix.runner }}
    timeout-minutes: ${{ matrix.extra_target && 45 || 30 }}
    strategy:
      fail-fast: false
      matrix:
        include:
          # Both apple triples in ONE macos-latest job: the macOS SDK ships both slices and
          # merging drops a duplicated toolchain setup (prebuild.yml's precedent, SMA-520).
          - { platform: macosx-arm64, target: aarch64-apple-darwin, runner: macos-latest, zig: false, expect_tag: 'macosx_11_0_arm64', extra_platform: macosx-x86_64, extra_target: x86_64-apple-darwin, extra_expect_tag: 'macosx_10_12_x86_64' }
          - { platform: win-amd64, target: x86_64-pc-windows-msvc, runner: windows-latest, zig: false, expect_tag: 'win_amd64' }
          # ALL FOUR linux legs use zig, not just musl — the deliberate divergence from
          # prebuild.yml. ubuntu-latest ships glibc 2.39, so a NATIVE build tags
          # manylinux_2_39, a wheel almost no consumer can install. The floor comes from the
          # TRIPLE SUFFIX (…-gnu.2.17), not from a bare --zig flag.
          - { platform: manylinux-x86_64, target: x86_64-unknown-linux-gnu.2.17, runner: ubuntu-latest, zig: true, compat: manylinux2014, expect_tag: 'manylinux_2_17_x86_64.manylinux2014_x86_64' }
          - { platform: manylinux-aarch64, target: aarch64-unknown-linux-gnu.2.17, runner: ubuntu-24.04-arm, zig: true, compat: manylinux2014, expect_tag: 'manylinux_2_17_aarch64.manylinux2014_aarch64' }
          - { platform: musllinux-x86_64, target: x86_64-unknown-linux-musl, runner: ubuntu-latest, zig: true, compat: musllinux_1_2, expect_tag: 'musllinux_1_2_x86_64' }
          - { platform: musllinux-aarch64, target: aarch64-unknown-linux-musl, runner: ubuntu-latest, zig: true, compat: musllinux_1_2, expect_tag: 'musllinux_1_2_aarch64' }
```

`ubuntu-24.04-arm` is kept for the gnu-aarch64 leg specifically so that **one aarch64 wheel is actually executed** rather than merely inspected — otherwise that leg family, three of seven wheels, has no runtime verification at all.

- [ ] **Step 3: Write the setup steps**

Copy the checkout / `setup-toolchain` / `moon setup` / cache block from `.github/workflows/prebuild.yml:76-115` verbatim, changing only the cache key prefix to `wheels-rust-` and its literal discriminator. **The discriminator matters:** `actions/cache` skips its post-job save on an exact primary-key hit, so reusing prebuild's key shape would mean this workflow restores prebuild's cache and never saves its own — cold rebuilds forever.

```yaml
    steps:
      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          persist-credentials: false

      - name: Set up proto + Moon
        uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0
        with:
          cache: false

      - name: Install Moon-managed toolchains
        run: moon setup

      - name: Install pinned maturin
        run: proto install maturin

      - name: Cache Rust (cargo + target)
        uses: actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9  # v6.1.0
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            rs/target
          key: wheels-rust-${{ runner.os }}-${{ matrix.platform }}-abi3-${{ hashFiles('rs/rust-toolchain.toml') }}-${{ hashFiles('rs/Cargo.lock') }}
          restore-keys: |
            wheels-rust-${{ runner.os }}-${{ matrix.platform }}-abi3-${{ hashFiles('rs/rust-toolchain.toml') }}-

      # Run from rs/ so rust-toolchain.toml's override selects the pinned 1.95.0 rather
      # than the runner default (prebuild.yml's lesson).
      - name: Add Rust target(s) (pinned toolchain)
        working-directory: rs
        run: rustup target add ${{ matrix.rustup_target }} ${{ matrix.extra_target }}

      - name: Set up zig (Linux cross-compile)
        if: ${{ matrix.zig }}
        run: pip3 install --break-system-packages ziglang
```

**Note on the glibc floor (settled by the first CI run, superseding controller Ruling 2):** an earlier revision gave the gnu legs a `.2.17`-suffixed `matrix.target`, assuming maturin accepted cargo-zigbuild's decorated-triple spelling. **It does not.** maturin passes `--target` straight to `cargo metadata`, and rustc answered `could not find specification for target "x86_64-unknown-linux-gnu.2.17"`, failing both manylinux legs while the other ten jobs passed. The floor comes from `--zig` plus `--compatibility`, both flags — maturin's `--help`: *"`--zig` … Default to manylinux2014/manylinux_2_17 if you do not specify a `--compatibility`"*. So `matrix.target` is always a real triple. The `rustup_target` key stays (identical today) because the build target and the rustup target are different questions.

- [ ] **Step 4: Write the build steps**

`--compatibility` is passed **explicitly** so maturin's built-in auditwheel *errors* on a mismatch rather than silently emitting a `linux_*` tag PyPI rejects. musl needs `-crt-static`: the target defaults to static CRT, which a `crate-type = ["cdylib"]` cannot use.

```yaml
      - name: Build the wheel
        working-directory: rs
        env:
          # musl defaults to crt-static, which a cdylib cannot use.
          RUSTFLAGS: ${{ contains(matrix.target, 'musl') && '-C target-feature=-crt-static' || '' }}
        run: |
          maturin build --release \
            -m crates/bindings/paigasus-py-bindings/Cargo.toml \
            --target ${{ matrix.target }} \
            ${{ matrix.zig && '--zig' || '' }} \
            ${{ matrix.compat && format('--compatibility {0}', matrix.compat) || '' }} \
            --out ../dist

      - name: Build the second darwin wheel
        if: ${{ matrix.extra_target }}
        working-directory: rs
        run: |
          maturin build --release \
            -m crates/bindings/paigasus-py-bindings/Cargo.toml \
            --target ${{ matrix.extra_target }} \
            --out ../dist
```

- [ ] **Step 5: Write the tag-set assertion**

The platform tag is a compressed **set** (`manylinux_2_17_x86_64.manylinux2014_x86_64`), so "equals the expected string" has no single answer. Split on `.` and compare **sets**. Defining this before writing the assertion is what stops an implementer hitting a red on a correct wheel and "fixing" it by loosening to a substring — reintroducing the exact vacuity rule 1 forbids.

```yaml
      - name: Assert the wheel's tag set
        shell: bash
        run: |
          set -euo pipefail
          python3 - "$PWD/dist" "${{ matrix.expect_tag }}" <<'PY'
          import sys, pathlib
          d, want = pathlib.Path(sys.argv[1]), set(sys.argv[2].split("."))
          hits = [w for w in d.glob("*.whl") if set(w.stem.split("-")[-1].split(".")) == want]
          allw = sorted(w.name for w in d.glob("*.whl"))
          if not hits:
              print(f"FAIL: no wheel with tag set {sorted(want)}; found {allw}", file=sys.stderr)
              raise SystemExit(1)
          for w in hits:
              parts = w.stem.split("-")
              assert parts[-3] == "cp312" and parts[-2] == "abi3", f"not a cp312-abi3 wheel: {w.name}"
          print(f"OK: {hits[0].name}")
          PY
```

Repeat the step for `matrix.extra_expect_tag`, guarded by `if: ${{ matrix.extra_target }}`.

**On the first CI run these expected strings are MEASUREMENTS, not verifications** (spec §13). If a leg reds, read what maturin actually produced, confirm it is *correct*, and pin that — do not loosen the comparison.

- [ ] **Step 6: Write the binary-level assertions**

For darwin, port `prebuild.yml:166-180`'s minimum-macOS assertion onto the wheel's `.so`. A cross-built x86_64 slice can otherwise inherit the host SDK's floor and silently drop 10.13–10.15 users while the *tag* still reads `10_12`.

```yaml
      - name: Assert Mach-O arch and minimum macOS (darwin only)
        if: runner.os == 'macOS'
        shell: bash
        run: |
          set -euo pipefail
          work="$(mktemp -d)"
          for whl in dist/*.whl; do
            rm -rf "$work"/*; unzip -q "$whl" -d "$work"
            so="$(find "$work" -name '*.abi3.so' -print -quit)"
            archs="$(lipo -archs "$so")"
            case "$whl" in
              *arm64*)  [ "$archs" = "arm64" ]  || { echo "::error::$whl archs=[$archs]"; exit 1; } ;;
              *x86_64*)
                [ "$archs" = "x86_64" ] || { echo "::error::$whl archs=[$archs]"; exit 1; }
                # BOTH load commands must be matched: x86_64 emits the legacy
                # LC_VERSION_MIN_MACOSX while arm64 emits LC_BUILD_VERSION.
                min="$(otool -l "$so" | awk '/LC_VERSION_MIN_MACOSX/{getline; getline; print $2; exit}')"
                [ "$min" = "10.12" ] || { echo "::error::x86_64 minimum macOS is [$min], but the wheel is tagged 10_12"; exit 1; }
                echo "darwin-x64 minimum macOS: [$min] — agrees with the tag" ;;
            esac
          done

      - name: Assert max GLIBC symbol version (manylinux legs)
        if: ${{ startsWith(matrix.platform, 'manylinux') }}
        run: |
          set -euo pipefail
          work="$(mktemp -d)"; unzip -q dist/*.whl -d "$work"
          so="$(find "$work" -name '*.abi3.so' -print -quit)"
          # An ELF-CLASS check reports only the machine type: a wheel tagged _2_17 whose
          # .so needs GLIBC_2.34 would pass it, install cleanly, and fail at import.
          max="$(objdump -T "$so" | grep -o 'GLIBC_[0-9.]*' | sort -V -u | tail -1)"
          echo "max glibc symbol: $max"
          [ "$max" = "GLIBC_2.17" ] || { echo "::error::wheel is tagged manylinux_2_17 but needs $max"; exit 1; }
```

Like Step 5, `GLIBC_2.17` is a **measurement on the first run**. If zig yields a lower maximum, pin that value.

- [ ] **Step 7: Write the native import smoke test and the METADATA assertion**

Four of seven wheels run on the runner that built them. The METADATA assertion matters because §1's motivating defect — 88 bytes — was measured on the **wheel**, and maturin derives wheel `METADATA` from `[project]` through a different code path than the sdist's `PKG-INFO`.

```yaml
      # Only on legs whose arch the runner can execute: macosx-arm64, win-amd64,
      # manylinux-x86_64, manylinux-aarch64. The cross-built wheels are covered by the
      # binary assertions above instead.
      - name: Install into a clean venv, import and call
        if: ${{ matrix.platform != 'musllinux-x86_64' && matrix.platform != 'musllinux-aarch64' }}
        shell: bash
        run: |
          set -euo pipefail
          python3 -m venv /tmp/smoke
          /tmp/smoke/bin/pip install --no-index dist/*${{ matrix.expect_tag }}*.whl 2>/dev/null \
            || /tmp/smoke/bin/pip install --no-index "$(ls dist/*.whl | head -1)"
          /tmp/smoke/bin/python -c '
          import paigasus_py_bindings as k
          u = k.mint_uuid7(bytes(10))
          assert len(u) == 36, u
          print("FFI load + call OK:", u)'

      - name: Assert wheel METADATA is complete
        shell: bash
        run: |
          set -euo pipefail
          python3 - <<'PY'
          import glob, zipfile, sys
          whl = sorted(glob.glob("dist/*.whl"))[0]
          meta = next(n for n in zipfile.ZipFile(whl).namelist() if n.endswith(".dist-info/METADATA"))
          text = zipfile.ZipFile(whl).read(meta).decode()
          for required in ("Summary:", "License-Expression:", "Description-Content-Type:", "Classifier:"):
              if required not in text:
                  print(f"FAIL: {whl} METADATA lacks {required}\n---\n{text}", file=sys.stderr)
                  raise SystemExit(1)
          names = zipfile.ZipFile(whl).namelist()
          assert any("/licenses/LICENSE" in n for n in names), f"no license file in {whl}: {names}"
          print(f"OK: {whl} METADATA complete ({len(text)} bytes)")
          PY
```

**On the venv step:** the Windows leg's venv paths differ (`Scripts/` not `bin/`). Use `python -m venv` plus `$VENV/Scripts/python.exe` under `runner.os == 'Windows'`, or split into two steps guarded by `if: runner.os == 'Windows'`. Verify on the first run.

**On `mint_uuid7`:** confirm the exported symbol name and signature before writing this — read `rs/crates/bindings/paigasus-py-bindings/src/lib.rs` and `paigasus_py_bindings.pyi`. Use whatever the crate actually exports; the assertion only needs to prove the FFI boundary loads and returns a sane value.

- [ ] **Step 8: Upload the artifacts**

```yaml
      - name: Upload wheel
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: wheel-${{ matrix.platform }}
          path: dist/*.whl
          if-no-files-found: error

      - name: Upload second darwin wheel
        if: ${{ matrix.extra_platform }}
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: wheel-${{ matrix.extra_platform }}
          path: dist/*${{ matrix.extra_expect_tag }}*.whl
          if-no-files-found: error
```

Both darwin wheels land in the same `dist/`, so the first upload's `dist/*.whl` glob would capture both under one artifact name. Give the arm64 upload an explicit `path: dist/*macosx_11_0_arm64*.whl` so each artifact holds exactly one wheel.

- [ ] **Step 9: Lint the workflow locally**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint
```

Expected: pass. This is what catches an inline `branches: [main]`, a brace-expanded path, or an unresolvable branch name — all of which would otherwise disable the workflow silently.

- [ ] **Step 10: Commit**

```bash
git add .github/workflows/wheels.yml
git commit -m "ci(repo): add the reusable cross-platform wheel matrix (SMA-578)"
```

---

### Task 5: `wheels.yml` — the sdist and the pure-Python face

**Files:**
- Modify: `.github/workflows/wheels.yml`

**Interfaces:**
- Consumes: Task 4's workflow skeleton; Task 1's `include` allowlist.
- Produces: artifacts `sdist` and `face-paigasus-kernel`, which SMA-579's publish job uploads alongside the seven platform wheels.

**Why three platforms and not one (review B1):** the first draft made this a single platform-independent job. On Linux the `-undefined dynamic_lookup` question does not arise, so the CI proof would have been vacuous with respect to the very claim it exists to protect — and PyPI versions cannot be reused once a user reports the regression. **The macOS leg is the standing control for spec §2.**

- [ ] **Step 1: Add the sdist build job**

```yaml
  sdist:
    name: build sdist
    runs-on: ubuntu-latest
    timeout-minutes: 15
    steps:
      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          persist-credentials: false

      - name: Set up proto + Moon
        uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0
        with:
          cache: false

      - name: Install Moon-managed toolchains
        run: moon setup

      - name: Install pinned maturin
        run: proto install maturin

      - name: Build the sdist
        working-directory: rs
        run: maturin sdist -m crates/bindings/paigasus-py-bindings/Cargo.toml --out ../dist

      # The `include` allowlist in the crate's Cargo.toml is what keeps moon.yml out
      # (MEASURED: without it the sdist ships it). Checks 1d/2b/2c in
      # repo:publish-metadata do NOT reach this crate — it is `publish = false` — so this
      # assertion is the only thing holding the allowlist honest.
      - name: Assert the sdist ships nothing repo-internal
        run: |
          set -euo pipefail
          listing="$(tar tzf dist/*.tar.gz)"
          echo "$listing"
          if printf '%s\n' "$listing" | grep -q 'moon\.yml'; then
            echo "::error::sdist ships moon.yml — the Cargo include allowlist has regressed"; exit 1
          fi
          for required in README.md LICENSE pyproject.toml Cargo.lock; do
            printf '%s\n' "$listing" | grep -q "/$required$" \
              || { echo "::error::sdist is missing $required"; exit 1; }
          done

      - name: Upload sdist
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: sdist
          path: dist/*.tar.gz
          if-no-files-found: error
```

- [ ] **Step 2: Add the three-platform sdist verification job**

```yaml
  sdist-verify:
    name: verify sdist on ${{ matrix.runner }}${{ matrix.msrv && ' (MSRV 1.95)' || '' }}
    needs: sdist
    runs-on: ${{ matrix.runner }}
    timeout-minutes: 30
    strategy:
      fail-fast: false
      matrix:
        include:
          # macOS is THE control for SMA-578 §2: it is the only platform where the
          # -undefined dynamic_lookup question exists at all. A Linux-only verification
          # would be vacuous with respect to the claim that retired the "no sdist" rule.
          - { runner: macos-latest, msrv: false }
          - { runner: windows-latest, msrv: false }
          - { runner: ubuntu-latest, msrv: false }
          # The sdist ships NO rust-toolchain.toml, so a consumer builds with whatever
          # rustc they have. This leg proves the advertised MSRV is real.
          - { runner: ubuntu-latest, msrv: true }
    steps:
      - name: Download the sdist
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          name: sdist
          path: dist

      - name: Use rustc at the declared MSRV
        if: ${{ matrix.msrv }}
        run: rustup toolchain install 1.95.0 --profile minimal && rustup default 1.95.0

      - name: Install from source into a clean venv, import and call
        shell: bash
        run: |
          set -euo pipefail
          python3 -m venv venv
          bin="venv/bin"; [ -d venv/Scripts ] && bin="venv/Scripts"
          # NOT --no-binary :all: — that forces source builds of BUILD dependencies too,
          # maturin included. Installing a local sdist never consults a wheel for the
          # package itself, so the plain form already proves the source build.
          "$bin/pip" install dist/*.tar.gz
          "$bin/python" -c '
          import paigasus_py_bindings as k
          u = k.mint_uuid7(bytes(10))
          assert len(u) == 36, u
          print("sdist source build + FFI call OK:", u)'
```

Use the same exported symbol Task 4 Step 7 settled on.

- [ ] **Step 3: Add the pure-Python face job**

```yaml
  face:
    name: build paigasus-kernel (pure python)
    runs-on: ubuntu-latest
    timeout-minutes: 15
    steps:
      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          persist-credentials: false

      - name: Set up proto + Moon
        uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0
        with:
          cache: false

      - name: Install Moon-managed toolchains
        run: moon setup

      - name: Build the wrapper distribution
        working-directory: py
        run: uv build --package paigasus-kernel --out-dir ../dist

      # [tool.uv.sources] is DEVELOPMENT-ONLY metadata that uv strips from the built
      # wheel, so the published wrapper depends on the `==` pin alone. Assert the pin
      # survived: without it the wrapper would float against any bindings version.
      - name: Assert the exact dependency pin shipped
        run: |
          set -euo pipefail
          meta="$(python3 -c "
          import glob, zipfile
          w = sorted(glob.glob('dist/paigasus_kernel-*.whl'))[0]
          z = zipfile.ZipFile(w)
          n = next(i for i in z.namelist() if i.endswith('.dist-info/METADATA'))
          print(z.read(n).decode())")"
          printf '%s\n' "$meta" | grep -qE '^Requires-Dist: paigasus-py-bindings==' \
            || { echo "::error::the wrapper lost its == pin on paigasus-py-bindings"; printf '%s\n' "$meta"; exit 1; }
          echo "OK: exact pin present"

      - name: Upload the face distribution
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: face-paigasus-kernel
          path: |
            dist/paigasus_kernel-*.whl
            dist/paigasus_kernel-*.tar.gz
          if-no-files-found: error
```

- [ ] **Step 4: Lint and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint
git add .github/workflows/wheels.yml
git commit -m "ci(repo): verify the sdist from source on macos, windows and linux (SMA-578)"
```

---

### Task 6: `repo:publish-metadata` grows a Python arm

**Files:**
- Modify: `ci/publish-metadata/run.sh`
- Modify: `moon.yml:496-531` (the `publish-metadata` task)

**Interfaces:**
- Consumes: Task 1's `[tool.paigasus] pypi = true` marker and `LICENSE`/`README.md`; Task 2's four new files; Task 4's `wheels.yml` (check P3 asserts on it).
- Produces: nothing downstream.

**Scope boundary (review M6):** this gate is in `ci.yml:214`'s `T=(…)` — the required `moon ci` check — with `toolchain: 'system'`. `ci.yml` installs no maturin and neither does CLAUDE.md's worktree-provisioning sequence, so **no check here may build an artifact**. Behavioural artifact assertions live in `wheels.yml` (Tasks 4 and 5). This arm is spelling-level and pure-Python only.

- [ ] **Step 1: Add the constants**

Next to `EXPECTED_PUBLISHABLE` (`run.sh:69`):

```bash
# The PyPI-bound set, discovered from the `[tool.paigasus] pypi` MARKER — not from the
# version field. In this repo `version != "0.0.0"` means "in a lockstep family"
# (repo:version-lockstep writes it), and paigasus-py-bindings is simultaneously
# `publish = false` on the Cargo side and PyPI-bound (SMA-578 review M7).
#
# py/packages/paigasus-proto is DELIBERATELY absent: it is version-locked with the proto
# family and its name is reserved on PyPI, but no publish path uploads it yet. SMA-579
# owns that decision (SMA-578 §9.3) — it must be recorded, not made by omission.
EXPECTED_PYPI_PUBLISHABLE=("paigasus-kernel" "paigasus-py-bindings")

# The scan set, as literal paths. NOT a filesystem `**/pyproject.toml` glob: that sweeps in
# py/pyproject.toml (a uv virtual root with NO [project] table) and, in a provisioned tree,
# ts/node_modules/.pnpm/…/node-gyp/gyp/pyproject.toml.
PYPI_SCAN=(
  "py/packages/paigasus-kernel/pyproject.toml"
  "py/packages/paigasus-ml/pyproject.toml"
  "py/packages/paigasus-proto/pyproject.toml"
  "py/packages/paigasus-workflows/pyproject.toml"
  "rs/crates/bindings/paigasus-py-bindings/pyproject.toml"
)

# Required [project] keys for a PyPI-bound distribution.
PYPI_REQUIRED_FIELDS=("description" "readme" "license" "license-files" "authors" "classifiers")
```

- [ ] **Step 2: Write the P0/P1/P2 checker**

Takes the scan list as arguments so `--negative-control` can drive the same code with fixtures — the idiom every other check in this file follows.

```bash
# Checks P0/P1/P2 — the PyPI metadata arm (SMA-578 §8). Takes the pyproject paths as
# arguments so --negative-control drives the SAME code with fixtures.
# Exit: 0 pass | 1 the repo is wrong | 2 infrastructure.
assert_pypi_metadata() { # $@ pyproject paths
  python3 - "$@" <<'PY'
import os, sys, tomllib

expected = set(os.environ["EXPECTED_PYPI_PUBLISHABLE"].split())
required = os.environ["PYPI_REQUIRED_FIELDS"].split()
paths, errors, found = sys.argv[1:], [], {}

if not paths:
    print("FATAL: no pyproject paths given — this check would pass vacuously", file=sys.stderr)
    raise SystemExit(2)
if not expected or not required:
    print("FATAL: empty rule set — this check would pass vacuously", file=sys.stderr)
    raise SystemExit(2)

for p in paths:
    try:
        with open(p, "rb") as fh:
            doc = tomllib.load(fh)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        # Infrastructure, not "the repo is wrong": an unreadable or unparsable manifest is
        # a different failure mode from a manifest that is present and wrong.
        print(f"FATAL: cannot read {p}: {exc}", file=sys.stderr)
        raise SystemExit(2)
    proj = doc.get("project")
    if not isinstance(proj, dict) or "name" not in proj:
        print(f"FATAL: {p} has no [project] table with a name", file=sys.stderr)
        raise SystemExit(2)
    name = proj["name"]
    if doc.get("tool", {}).get("paigasus", {}).get("pypi") is True:
        found[name] = (p, proj)

# P0 — strict equality. The set is discovered from the very marker this gate protects, so
# a shrunken set must be a hard failure, not a green run over nothing (mirrors Check 0).
if set(found) != expected:
    errors.append(f"Check P0 FAILED: PyPI-bound set is {sorted(found)}, expected {sorted(expected)}")

for name, (p, proj) in sorted(found.items()):
    # P1 — required metadata.
    for field in required:
        if not proj.get(field):
            errors.append(f"Check P1 FAILED: {name} ({p}) has no [project] {field}")
    # P1 — the SPDX-vs-classifier rule (SMA-378): PyPI hard-rejects an SPDX license
    # expression supplied ALONGSIDE a License :: trove classifier.
    if proj.get("license") and any(
        str(c).startswith("License ::") for c in proj.get("classifiers", [])
    ):
        errors.append(
            f"Check P1 FAILED: {name} supplies an SPDX license AND a 'License ::' "
            f"classifier — PyPI rejects the combination; drop the classifier"
        )
    # P2 — the files those fields name must EXIST. uv_build does not auto-glob license
    # files (SMA-378), so a missing file means a wheel that ships no license text.
    base = os.path.dirname(p)
    for rel in [proj.get("readme")] + list(proj.get("license-files") or []):
        if isinstance(rel, str) and not os.path.isfile(os.path.join(base, rel)):
            errors.append(f"Check P2 FAILED: {name} declares {rel!r} but {base}/{rel} does not exist")

for e in errors:
    print(e, file=sys.stderr)
raise SystemExit(1 if errors else 0)
PY
}
```

Export the two arrays as space-joined env vars immediately before the call, alongside the existing `REPO_ROOT` export at `run.sh:64`.

- [ ] **Step 3: Write the P1 lint-table extension and the D6 assertion**

```bash
# P1 (continued) — Check 1c's rule, extended to crates whose SOURCES SHIP IN A PUBLISHED
# SDIST rather than only to `publish = true` crates. maturin ships the workspace
# Cargo.toml verbatim (measured), so an sdist consumer compiles as the ROOT package where
# --cap-lints allow does not apply. Check 1c misses paigasus-py-bindings precisely because
# that crate is `publish = false` (SMA-578 review B2).
SDIST_SHIPPED_CRATES=("rs/crates/bindings/paigasus-py-bindings" "rs/crates/libs/paigasus-kernel")

assert_sdist_lint_tables() {
  local dir rc=0
  [ "${#SDIST_SHIPPED_CRATES[@]}" -gt 0 ] \
    || die_infra "SDIST_SHIPPED_CRATES is empty — this check would pass vacuously"
  for dir in "${SDIST_SHIPPED_CRATES[@]}"; do
    assert_lint_table "$REPO_ROOT/$dir/Cargo.toml" || rc=$?
    [ "$rc" -ne 2 ] || return 2
  done
  return "$rc"
}

# Assert wheels.yml never gains registry credentials (SMA-578 D6). It carries a
# pull_request trigger, and same-repo PRs receive repository secrets — moving the upload
# into it, the natural refactor once the artifacts are there, would reopen SMA-407 §7/M2.
assert_wheels_has_no_credentials() { # $1 workflow path
  python3 - "$1" <<'PY'
import re, sys
try:
    text = open(sys.argv[1], encoding="utf-8").read()
except OSError as exc:
    print(f"FATAL: cannot read {sys.argv[1]}: {exc}", file=sys.stderr); raise SystemExit(2)
bad = []
if re.search(r'(?m)^\s*id-token\s*:\s*write', text):
    bad.append("declares `id-token: write`")
if re.search(r'(?m)^\s*secrets\s*:', text):
    bad.append("declares `secrets:`")
if bad:
    print("Check P-D6 FAILED: wheels.yml " + " and ".join(bad) +
          " — it is pull_request-triggered, so a same-repo PR would receive the "
          "credential. Publishing belongs in release.yml (SMA-407 §7 review M2).",
          file=sys.stderr)
    raise SystemExit(1)
PY
}
```

`assert_lint_table` already exists at `run.sh:293` and already takes a manifest path — reuse it rather than writing a second copy.

- [ ] **Step 4: Wire the three calls into `main()`**

Add before the final `echo "publish-metadata: all checks passed"` in `main()` (`run.sh:~1240`):

```bash
  status=0; assert_pypi_metadata "${PYPI_SCAN[@]/#/$REPO_ROOT/}" || status=$?
  [ "$status" -eq 0 ] || exit "$status"
  status=0; assert_sdist_lint_tables || status=$?
  [ "$status" -eq 0 ] || exit "$status"
  status=0; assert_wheels_has_no_credentials "$REPO_ROOT/.github/workflows/wheels.yml" || status=$?
  [ "$status" -eq 0 ] || exit "$status"
```

- [ ] **Step 5: Add the negative-control fixtures**

Inside `negative_control()` (`run.sh:775`), using the existing `_expect_rc` helper so exit codes are asserted **exactly** — a harness that cannot tell 1 from 2 silently absorbs a broken invocation.

```bash
  # --- SMA-578: the PyPI arm must be able to report red -------------------------------
  local pyd="$tmp/py"; mkdir -p "$pyd/ok" "$pyd/nolicfile" "$pyd/spdxclash" "$pyd/denylints"

  _pyproj() { # $1 dir  $2 extra-toml
    mkdir -p "$1"
    { printf '[project]\nname = "paigasus-kernel"\nversion = "0.1.0"\n'
      printf 'description = "d"\nreadme = "README.md"\nlicense = "Apache-2.0"\n'
      printf 'license-files = ["LICENSE"]\nauthors = [{ name = "a" }]\n'
      printf 'classifiers = ["Typing :: Typed"]\n%s' "$2"
      printf '\n[tool.paigasus]\npypi = true\n'
    } >"$1/pyproject.toml"
  }
  # A second marked distribution, so the P0 set matches EXPECTED and the rows below fail
  # on the rule they NAME rather than on P0.
  _pybind() { _pyproj "$1" ""; sed -i.bak 's/paigasus-kernel/paigasus-py-bindings/' "$1/pyproject.toml"; }

  _pyproj "$pyd/ok" ""; : >"$pyd/ok/README.md"; : >"$pyd/ok/LICENSE"
  _pybind "$pyd/ok2"; : >"$pyd/ok2/README.md"; : >"$pyd/ok2/LICENSE"
  _expect_rc 0 "P0/P1/P2 accept a well-formed pair" \
    assert_pypi_metadata "$pyd/ok/pyproject.toml" "$pyd/ok2/pyproject.toml"

  # P0 — one distribution short of EXPECTED_PYPI_PUBLISHABLE.
  _expect_rc 1 "P0 rejects a shrunken publishable set" \
    assert_pypi_metadata "$pyd/ok/pyproject.toml"

  # P2 — declared LICENSE does not exist on disk.
  _pyproj "$pyd/nolicfile" ""; : >"$pyd/nolicfile/README.md"
  _pybind "$pyd/nolic2"; : >"$pyd/nolic2/README.md"; : >"$pyd/nolic2/LICENSE"
  _expect_rc 1 "P2 rejects a declared-but-absent LICENSE" \
    assert_pypi_metadata "$pyd/nolicfile/pyproject.toml" "$pyd/nolic2/pyproject.toml"

  # P1 — SPDX expression AND a License:: trove classifier.
  _pyproj "$pyd/spdxclash" 'classifiers = ["License :: OSI Approved :: Apache Software License"]'
  : >"$pyd/spdxclash/README.md"; : >"$pyd/spdxclash/LICENSE"
  _pybind "$pyd/spdx2"; : >"$pyd/spdx2/README.md"; : >"$pyd/spdx2/LICENSE"
  _expect_rc 1 "P1 rejects an SPDX license alongside a License:: classifier" \
    assert_pypi_metadata "$pyd/spdxclash/pyproject.toml" "$pyd/spdx2/pyproject.toml"

  # Infrastructure, not assertion: no [project] table at all (py/pyproject.toml's shape).
  printf '[tool.uv.workspace]\nmembers = ["packages/*"]\n' >"$pyd/virtual.toml"
  _expect_rc 2 "a manifest with no [project] table is INFRASTRUCTURE (rc 2), not rc 1" \
    assert_pypi_metadata "$pyd/virtual.toml"

  # The lint-table rule, on a crate that inherits the workspace's denying table.
  printf '[package]\nname = "x"\n\n[lints]\nworkspace = true\n' >"$tmp/deny-Cargo.toml"
  _expect_rc 1 "the sdist lint rule rejects an inherited workspace table" \
    assert_lint_table "$tmp/deny-Cargo.toml"

  # D6 — wheels.yml must never carry registry credentials.
  printf 'on:\n  pull_request:\njobs:\n  a:\n    permissions:\n      id-token: write\n' \
    >"$tmp/bad-wheels.yml"
  _expect_rc 1 "D6 rejects id-token: write in wheels.yml" \
    assert_wheels_has_no_credentials "$tmp/bad-wheels.yml"
  printf 'on:\n  pull_request:\njobs:\n  a:\n    permissions:\n      contents: read\n' \
    >"$tmp/good-wheels.yml"
  _expect_rc 0 "D6 accepts a credential-free wheels.yml" \
    assert_wheels_has_no_credentials "$tmp/good-wheels.yml"
```

The `ok`/`ok2` positive row is not decoration: without a passing case, a checker that returned 1 unconditionally would satisfy every negative row.

- [ ] **Step 6: Extend the task's `inputs` and description**

In `moon.yml`, add to the `publish-metadata` task's `inputs` (one entry per path — no brace expansion). Every entry must exist after Tasks 1 and 2, because `repo:input-liveness` fails on a glob matching zero tracked files:

```yaml
      - '.github/workflows/wheels.yml'
      - 'py/packages/paigasus-kernel/pyproject.toml'
      - 'py/packages/paigasus-kernel/README.md'
      - 'py/packages/paigasus-kernel/LICENSE'
      - 'py/packages/paigasus-ml/pyproject.toml'
      - 'py/packages/paigasus-ml/README.md'
      - 'py/packages/paigasus-ml/LICENSE'
      - 'py/packages/paigasus-proto/pyproject.toml'
      - 'py/packages/paigasus-workflows/pyproject.toml'
      - 'py/packages/paigasus-workflows/README.md'
      - 'py/packages/paigasus-workflows/LICENSE'
```

`rs/crates/**/*` already covers the bindings pyproject. Update the task `description:` to mention the PyPI arm.

- [ ] **Step 7: Prove the control fires, then that the gate passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/publish-metadata/run.sh --negative-control
```

Expected: every new row prints `ok — …` with its exact rc. Then prove a row *bites* by breaking the repo deliberately:

```bash
mv rs/crates/bindings/paigasus-py-bindings/LICENSE /tmp/L
bash ci/publish-metadata/run.sh; echo "rc=$?"
```

Expected: `Check P2 FAILED: paigasus-py-bindings declares 'LICENSE' but … does not exist`, `rc=1`. Restore with `mv /tmp/L rs/crates/bindings/paigasus-py-bindings/LICENSE`, then:

```bash
bash ci/publish-metadata/run.sh; echo "rc=$?"
```

Expected: `publish-metadata: all checks passed`, `rc=0`.

- [ ] **Step 8: Run the gate through Moon and record the cost**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
time moon run repo:publish-metadata --force
```

Expected: pass. Record the wall-clock delta against `main` in the task's comment, as SMA-530 did for the release-parity controls.

- [ ] **Step 9: Commit**

```bash
git add ci/publish-metadata/run.sh moon.yml
git commit -m "ci(repo): gate PyPI packaging metadata alongside crates.io metadata (SMA-578)"
```

---

### Task 7: Correct the umbrella design, update CLAUDE.md, and verify the whole graph

**Files:**
- Modify: `docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md` (§7, §15)
- Modify: `docs/superpowers/specs/2026-08-28-sma-578-maturin-wheel-matrix-design.md` (status line, §14 Q1/Q2)
- Modify: `CLAUDE.md`

**Interfaces:**
- Consumes: everything above.
- Produces: the corrected record.

- [ ] **Step 1: Amend the umbrella's §7 PyPI paragraph**

Replace the paragraph beginning *"Wheels only, no sdist" closes the macOS sdist trap* with an amendment — **amend, do not delete**: the claim was load-bearing for a decision, so its reversal is worth recording.

```markdown
### The PyPI wheel problem (review M3) — **AMENDED 2026-08-28 (SMA-578)**

The premise recorded here was measured false. This paragraph claimed a published sdist
could not build on macOS because it would not carry `rs/.cargo/config.toml`'s
`-undefined dynamic_lookup` flags. The observation is true; the conclusion is not.

**Measured (SMA-578 §2):** the sdist was extracted to a directory with no
`.cargo/config.toml` anywhere on cargo's upward walk, and `maturin build` linked cleanly
on macOS. maturin supplies those link arguments itself; `rs/.cargo/config.toml` exists so
that a *non-maturin* `cargo build` links, exactly as its own comment says. The control —
plain `cargo build` in that same directory — fails with undefined `_Py*` symbols.

So the design is **wheels plus a verified sdist**, not wheels only. SMA-578 delivers the
seven-wheel matrix and verifies the sdist from source on macOS, Windows and Linux, with
the macOS leg as the standing control for this claim.
```

- [ ] **Step 2: Amend the umbrella's §15 risk table**

Replace the *"PyPI package uninstallable off linux/x86_64"* row and add a rollback row (review M9):

```markdown
| PyPI package uninstallable off linux/x86_64 | **Closed by SMA-578** — a seven-wheel matrix (darwin arm64/x86_64, win x86_64, manylinux + musllinux on x86_64/aarch64) plus an sdist verified from source on three platforms. |
| A partial PyPI upload cannot be retried | PyPI is delete-but-never-reuse: a failed second distribution makes a naive retry hit 400 "file already exists". The publish job uses `skip-existing` and is re-runnable (SMA-578 §9.1, owned by SMA-579). |
```

- [ ] **Step 3: Close §14 Q1 and Q2 in the SMA-578 design**

Both were measured during planning. Replace them with their answers:

```markdown
1. ~~Does maturin honour Cargo's `include` for the sdist file list?~~ **Answered — yes**
   (measured 2026-08-28). maturin builds the sdist from `cargo package --list`, so the
   Cargo `include` allowlist alone controls its contents; adding one removed `moon.yml`.
   No `[tool.maturin] include` is needed.
2. ~~Where does `maturin sdist` place `pyproject.toml`?~~ **Answered — at the archive
   ROOT**, not in the crate directory, even when the crate is nested under
   `crates/bindings/`. The `moon.yml`-absence assertion therefore matches on basename.
```

Update the status line to `Status: Approved (gate 1, 2026-08-28); implemented by this branch.`

- [ ] **Step 4: Add the CLAUDE.md gotchas**

Add to the Gotchas section. These are the facts a future session would otherwise have to re-measure:

```markdown
- `paigasus-py-bindings` ships to PyPI as **seven `cp312-abi3` wheels plus a source-verified
  sdist**, built by `.github/workflows/wheels.yml` (SMA-578) — a *reusable* workflow
  (`on: workflow_call`) that SMA-579's gated `release` job consumes. It must **never** declare
  `secrets:` or `id-token: write`: it carries a `pull_request` trigger, so a same-repo PR would
  receive the credential — `repo:publish-metadata` asserts this. Four facts that cost a
  measurement each: (1) maturin injects the apple-darwin `-undefined dynamic_lookup` args
  **itself**, so an sdist builds on macOS without `rs/.cargo/config.toml` — that file exists for
  plain `cargo build`, as its own comment says, and the old "no sdist" rule rested on a false
  premise; (2) maturin builds the sdist from `cargo package --list`, so the crate's **Cargo**
  `include` allowlist is what keeps `moon.yml` out — `[tool.maturin] include` is not needed, and
  Checks 1d/2b/2c never reach this crate because it is `publish = false`, so the only assertion
  holding that allowlist honest lives in `wheels.yml`; (3) the sdist ships the **workspace**
  `Cargo.toml` verbatim, `[workspace.lints.rust] warnings = "deny"` included, and a consumer
  builds as the ROOT package where `--cap-lints allow` does NOT apply — so every sdist-shipped
  crate needs its own non-denying `[lints.rust]` table, the Check-1c rule extended past
  `publish = true`; (4) `pyo3`'s `abi3-py312` means one wheel per (OS, arch) covers CPython
  3.12+, so the matrix never multiplies by Python version.
- All four **Linux** wheel legs cross-compile with `--zig` — not only the musl ones, unlike
  `prebuild.yml`. `ubuntu-latest` ships glibc 2.39, so a *native* build tags `manylinux_2_39`,
  which almost nothing can install; the floor comes from the **triple suffix**
  (`x86_64-unknown-linux-gnu.2.17`), not from a bare `--zig`. Pass `--compatibility` explicitly so
  maturin's auditwheel **errors** instead of silently emitting a PyPI-rejected `linux_*` tag, and
  set `-C target-feature=-crt-static` on musl (the target defaults to a static CRT a cdylib
  cannot use). A wheel's **tag is not its binary**: assert the compressed tag *set* (split on `.`
  — `manylinux_2_17_x86_64.manylinux2014_x86_64` is one tag), and separately assert the binary via
  `otool -l`'s minimum-macOS on darwin and a max-`GLIBC_` symbol check on manylinux. An ELF-class
  check proves only the machine type and passes for a wheel that fails at import.
```

- [ ] **Step 5: Run the full CI graph exactly as CI does**

Per-project tasks do **not** run the repo-level gates. This is the acceptance evidence.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep --base origin/main --include-relations
```

Expected: all green. If Moon reports an unattributed failure, read
`.moon/cache/ciReport.json` — `jq '.actions[]|select(.status=="failed")'` — rather than
guessing.

**Two gates most likely to red, and what to do:**
- `repo:input-liveness` — a Task 6 `inputs:` entry names a path that does not exist or is untracked. Fix the path; do not add an `ALLOW_DEAD_INPUT` entry, since every path here is a real file.
- `repo:affected-smoke` — adding `wheels.yml` and the new gate inputs can shift an affected set. Re-baseline only after confirming the new set is *correct*; the cases are strict-equality by design.

- [ ] **Step 6: Commit**

```bash
git add docs/ CLAUDE.md
git commit -m "docs(repo): correct the sdist premise and record the wheel-matrix gotchas (SMA-578)"
```

---

## Self-Review

**Spec coverage.** §2 → Task 1 Steps 3/5 (caveat correction) + Task 7 Step 1. §2.1 → Task 1 Steps 4–6 (`include`, lint table). §5.1 triggers → Task 4 Step 1. §5.2 matrix → Task 4 Step 2. §5.3 maturin pin → Task 3. §5.4 verification → Task 4 Steps 5–7. §6 sdist → Task 5 Steps 1–2. §7.1 metadata → Task 1. §7.2 SMA-556 → Task 2. §7.3 lint table → Task 1 Step 4 + Task 6 Step 3. §7.4 MSRV → Task 1 Step 2 (README) + Task 5 Step 2 (MSRV leg). §8 P0/P1/P2 → Task 6 Steps 1–5. §8.1 D6 → Task 6 Step 3. §9 → **deferred to SMA-579 by design**, not implemented here. §10 corrections → Task 7 Steps 1–3. §12 testing → each task's verification steps + Task 7 Step 5.

**Type consistency.** `EXPECTED_PYPI_PUBLISHABLE`, `PYPI_SCAN`, `PYPI_REQUIRED_FIELDS`, `SDIST_SHIPPED_CRATES`, `assert_pypi_metadata`, `assert_sdist_lint_tables`, `assert_wheels_has_no_credentials` are defined in Task 6 Steps 1–3 and called in Step 4 under exactly those names. `assert_lint_table` and `_expect_rc` are pre-existing (`run.sh:293`, `run.sh:~800`) and reused, not redefined. Artifact names `wheel-<platform>`, `sdist`, `face-paigasus-kernel` are produced in Tasks 4–5 and are what SMA-579 will download. The matrix keys `platform`, `target`, `runner`, `zig`, `compat`, `expect_tag`, `extra_*` are declared in Task 4 Step 2 and used consistently in Steps 3–8.

**Three deliberate measure-then-pin points**, flagged in place rather than guessed: the maturin asset filenames (Task 3 Step 1), the wheel tag sets and the max-GLIBC value (Task 4 Steps 5–6), and the `rustup target add` handling of the `.2.17` triple suffix (Task 4 Step 3). Each says explicitly: read what the tool produced, confirm it is correct, pin that — never loosen the comparison to make a red go away.
