# SMA-378 PyPI Publishing Metadata Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `paigasus-proto` and `paigasus-kernel` complete, valid PyPI-publishable metadata so they can be published per ADR-0006.

**Architecture:** Edit each package's `pyproject.toml` `[project]` table to add `description`/`readme`/`license`/`authors`/`classifiers` and drop the `# TODO(SMA-378)` block; add a packaged `README.md` and a `LICENSE` copy to each package dir so the wheel renders a description and embeds the Apache-2.0 text; track those two new files in the shared Moon build task's inputs. Verify by building each wheel with the same `uv build` invocation Moon uses and inspecting the resulting `METADATA` and file list.

**Tech Stack:** Python packaging (PEP 621 + PEP 639), `uv_build` backend (uv ≥ 0.11.16), Moon task runner.

## Global Constraints

- **Scope:** only `paigasus-proto` and `paigasus-kernel`. Do NOT touch `paigasus-ml` or `paigasus-workflows`.
- **Version stays `0.0.0`** in both packages — the 0.1.0 floor is SMA-407, not this issue.
- **`license = "Apache-2.0"`** is a PEP 639 SPDX expression string. Because of this, do NOT add any `License :: OSI Approved :: ...` classifier — declaring both is a hard PyPI/twine rejection.
- **`authors = [{ name = "Paigasus contributors" }]`** — name only, mirroring `rs/Cargo.toml`. No email.
- **Classifiers** are identical for both packages and contain no Development Status and no per-minor Python version:
  ```toml
  classifiers = [
    "Programming Language :: Python :: 3",
    "Programming Language :: Python :: 3 :: Only",
    "Intended Audience :: Developers",
    "Operating System :: OS Independent",
    "Topic :: Software Development :: Libraries",
    "Typing :: Typed",
  ]
  ```
- **SPDX header convention** applies to source files (`#` for Python), NOT to Markdown docs — the new `README.md` files carry no SPDX comment (consistent with the existing `py/README.md` and root `README.md`).
- **`LICENSE` is a real file copy**, never a symlink.
- **Tool PATH (Bash):** moon/uv are proto-managed and off the default Bash PATH. Every shell command below assumes this has been exported first (shims FIRST so the repo-pinned versions win):
  ```bash
  export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
  ```
- **Commits:** Conventional Commits with a workspace scope from the allowed enum (`build`, `py`, etc.). Commits are SSH-signed via 1Password; if a commit fails with `failed to fill whole buffer`, 1Password is locked — stop and ask the user to unlock, then retry.

---

### Task 1: PyPI metadata for `paigasus-proto`

**Files:**
- Modify: `py/packages/paigasus-proto/pyproject.toml`
- Create: `py/packages/paigasus-proto/README.md`
- Create: `py/packages/paigasus-proto/LICENSE` (copy of root `LICENSE`)

**Interfaces:**
- Consumes: nothing (first task).
- Produces: a buildable `paigasus-proto` distribution whose `METADATA` carries the new fields. Task 3 adds the two new files to the Moon build inputs; the exact filenames it relies on are `README.md` and `LICENSE`.

- [ ] **Step 1: Replace the `[project]` table and drop the TODO block**

Set `py/packages/paigasus-proto/pyproject.toml` to exactly:

```toml
[project]
name = "paigasus-proto"
version = "0.0.0"
description = "Generated protobuf message types and gRPC stubs for Paigasus."
readme = "README.md"
license = "Apache-2.0"
authors = [{ name = "Paigasus contributors" }]
requires-python = ">=3.12"
classifiers = [
  "Programming Language :: Python :: 3",
  "Programming Language :: Python :: 3 :: Only",
  "Intended Audience :: Developers",
  "Operating System :: OS Independent",
  "Topic :: Software Development :: Libraries",
  "Typing :: Typed",
]
dependencies = [
  # Runtime for generated betterproto2 code. The [grpclib] extra is required
  # because the generated HealthService stub imports grpclib at module level.
  # Minor must match betterproto2-compiler (compiler enforces ~=0.10.0).
  "betterproto2[grpclib]>=0.10,<0.11",
]

[build-system]
requires = ["uv_build>=0.11.16,<0.12"]
build-backend = "uv_build"
```

(The `# TODO(SMA-378): ...` two-line comment block is gone; the `dependencies` array and `[build-system]` are unchanged.)

- [ ] **Step 2: Create the package README**

Create `py/packages/paigasus-proto/README.md`:

```markdown
# paigasus-proto

Generated protobuf message types and gRPC stubs for Paigasus, compiled from the
`contracts/` protobuf source of truth (betterproto2).

Licensed under the Apache License, Version 2.0.
```

- [ ] **Step 3: Add the LICENSE copy**

Run from the repo root:

```bash
cp LICENSE py/packages/paigasus-proto/LICENSE
```

- [ ] **Step 4: Build the distribution (the test)**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd py/packages/paigasus-proto && rm -rf dist && uv build
```

Expected: build succeeds, prints `Successfully built dist/paigasus_proto-0.0.0.tar.gz` and `dist/paigasus_proto-0.0.0-py3-none-any.whl`. No error about `license`, license-files, or classifiers.

If the build instead errors on `license = "Apache-2.0"` (e.g. the resolved `uv_build` rejects the SPDX string), do NOT silently work around it — that contradicts the spec's PEP 639 assumption; stop and report, since it likely means a uv_build version older than the lock expects.

- [ ] **Step 5: Assert the METADATA fields (the test assertions)**

Run:

```bash
unzip -p dist/paigasus_proto-0.0.0-py3-none-any.whl '*/METADATA' | grep -E '^(Metadata-Version|Summary|Description-Content-Type|License-Expression|License-File|Author|Classifier):'
```

Expected output contains all of:
- `Metadata-Version: 2.4`
- `Summary: Generated protobuf message types and gRPC stubs for Paigasus.`
- `Description-Content-Type: text/markdown`
- `License-Expression: Apache-2.0`
- `License-File: LICENSE`
- `Author: Paigasus contributors`
- one `Classifier:` line per classifier above (6 lines), and NO `Classifier: License :: ...` line.

- [ ] **Step 6: Assert the packaged files**

Run:

```bash
unzip -l dist/paigasus_proto-0.0.0-py3-none-any.whl | grep -E 'py\.typed|LICENSE'
```

Expected: at least one `paigasus_proto/py.typed` entry (and the nested `generated/py.typed`), plus a `LICENSE` entry inside the `.dist-info/` (and/or package). If `py.typed` is absent, the `Typing :: Typed` classifier is dishonest — stop and report.

- [ ] **Step 7: Clean build artifacts**

Run:

```bash
rm -rf dist
```

(`dist/` is gitignored; this just avoids leaving build output around.)

- [ ] **Step 8: Commit**

```bash
git add py/packages/paigasus-proto/pyproject.toml py/packages/paigasus-proto/README.md py/packages/paigasus-proto/LICENSE
git commit -m "build(py): add PyPI publishing metadata to paigasus-proto

Add description/readme/license/authors/classifiers (SMA-378), a packaged
README, and a LICENSE copy so the wheel embeds the Apache-2.0 text.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: PyPI metadata for `paigasus-kernel`

**Files:**
- Modify: `py/packages/paigasus-kernel/pyproject.toml`
- Create: `py/packages/paigasus-kernel/README.md`
- Create: `py/packages/paigasus-kernel/LICENSE` (copy of root `LICENSE`)

**Interfaces:**
- Consumes: nothing from Task 1 (independent package).
- Produces: a buildable `paigasus-kernel` distribution. Same `README.md` + `LICENSE` filenames Task 3 tracks.

- [ ] **Step 1: Replace the `[project]` table and drop the TODO block**

Set `py/packages/paigasus-kernel/pyproject.toml` to exactly:

```toml
[project]
name = "paigasus-kernel"
version = "0.0.0"
description = "Python bindings for the Paigasus behavioral kernel."
readme = "README.md"
license = "Apache-2.0"
authors = [{ name = "Paigasus contributors" }]
requires-python = ">=3.12"
classifiers = [
  "Programming Language :: Python :: 3",
  "Programming Language :: Python :: 3 :: Only",
  "Intended Audience :: Developers",
  "Operating System :: OS Independent",
  "Topic :: Software Development :: Libraries",
  "Typing :: Typed",
]
dependencies = ["paigasus-py-bindings"]

[build-system]
requires = ["uv_build>=0.11.16,<0.12"]
build-backend = "uv_build"

[tool.uv.sources]
paigasus-py-bindings = { path = "../../../rs/crates/bindings/paigasus-py-bindings" }
```

(`dependencies`, `[build-system]`, and `[tool.uv.sources]` are unchanged; only the `[project]` metadata is expanded and the TODO block removed.)

- [ ] **Step 2: Create the package README**

Create `py/packages/paigasus-kernel/README.md`:

```markdown
# paigasus-kernel

Python bindings for the Paigasus behavioral kernel — a thin, typed re-export over
the PyO3 binding (`paigasus-py-bindings`).

Licensed under the Apache License, Version 2.0.
```

- [ ] **Step 3: Add the LICENSE copy**

Run from the repo root:

```bash
cp LICENSE py/packages/paigasus-kernel/LICENSE
```

- [ ] **Step 4: Build the distribution (the test) — must NOT require cargo**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd py/packages/paigasus-kernel && rm -rf dist && uv build
```

Expected: build succeeds and emits `dist/paigasus_kernel-0.0.0.tar.gz` + `dist/paigasus_kernel-0.0.0-py3-none-any.whl`. The runtime dependency `paigasus-py-bindings` is recorded as `Requires-Dist`, NOT built — so this must succeed with no Rust toolchain and no compilation of the binding. If the build tries to build `paigasus-py-bindings` / invokes cargo / maturin, stop and report (the spec asserts it should not).

- [ ] **Step 5: Assert the METADATA fields**

Run:

```bash
unzip -p dist/paigasus_kernel-0.0.0-py3-none-any.whl '*/METADATA' | grep -E '^(Metadata-Version|Summary|Description-Content-Type|License-Expression|License-File|Author|Classifier|Requires-Dist):'
```

Expected contains: `Metadata-Version: 2.4`, `Summary: Python bindings for the Paigasus behavioral kernel.`, `Description-Content-Type: text/markdown`, `License-Expression: Apache-2.0`, `License-File: LICENSE`, `Author: Paigasus contributors`, the 6 `Classifier:` lines (no `License ::` classifier), and `Requires-Dist: paigasus-py-bindings`.

- [ ] **Step 6: Assert the packaged files**

Run:

```bash
unzip -l dist/paigasus_kernel-0.0.0-py3-none-any.whl | grep -E 'py\.typed|LICENSE'
```

Expected: `paigasus_kernel/py.typed` and a `LICENSE` entry present.

- [ ] **Step 7: Clean build artifacts**

```bash
rm -rf dist
```

- [ ] **Step 8: Commit**

```bash
git add py/packages/paigasus-kernel/pyproject.toml py/packages/paigasus-kernel/README.md py/packages/paigasus-kernel/LICENSE
git commit -m "build(py): add PyPI publishing metadata to paigasus-kernel

Add description/readme/license/authors/classifiers (SMA-378), a packaged
README, and a LICENSE copy so the wheel embeds the Apache-2.0 text.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Track README/LICENSE in the Moon build inputs + full gate run

**Files:**
- Modify: `.moon/tasks/python-project.yml` (the `build` task `inputs`)

**Interfaces:**
- Consumes: the `README.md` + `LICENSE` files created in Tasks 1 & 2.
- Produces: a build cache that busts when those files change. Final deliverable: green `py:*` gates over the whole change.

- [ ] **Step 1: Add `README.md` and `LICENSE` to the build inputs**

In `.moon/tasks/python-project.yml`, find the `build` task's `inputs` line:

```yaml
    inputs: ['@group(sources)', 'pyproject.toml']
```

Change it to:

```yaml
    inputs: ['@group(sources)', 'pyproject.toml', 'README.md', 'LICENSE']
```

(Packages without those files — `paigasus-ml`, `paigasus-workflows` — are unaffected: a missing input glob simply matches nothing.)

- [ ] **Step 2: Verify the Moon build task still resolves and runs for both packages**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd py && moon run paigasus-proto-py:build paigasus-kernel-py:build
```

Expected: both build tasks succeed (no Moon config/parse error, no missing-input error). Moon should now hash `README.md`/`LICENSE` as inputs for these two projects.

- [ ] **Step 3: Run the full Python quality gates**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd py && moon run py:lint py:format py:typecheck py:test
```

Expected: all four pass. The TOML + README/LICENSE additions should not perturb lint/format/typecheck/test (ruff targets `.py`; basedpyright globs `packages/*/src/**` + `tests/**`; the new files are outside both). If `py:format` or `py:lint` flags anything in the changed packages, fix it before committing.

- [ ] **Step 4: Confirm no stray build output is staged**

Run:

```bash
git status --porcelain
```

Expected: the only changes are `.moon/tasks/python-project.yml` (and nothing under any `dist/`). If a `dist/` directory appears, remove it (`rm -rf py/packages/*/dist`).

- [ ] **Step 5: Commit**

```bash
git add .moon/tasks/python-project.yml
git commit -m "build(py): track README and LICENSE in the wheel build inputs

The uv_build wheel embeds README.md (description) and LICENSE; add both to
the shared Moon build task inputs so the cache busts when they change (SMA-378).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:**
- description/readme/license/authors/classifiers on both packages → Tasks 1 & 2, Step 1. ✓
- TODO block removed → Tasks 1 & 2, Step 1. ✓
- Per-package README → Tasks 1 & 2, Step 2. ✓
- Per-package LICENSE copy (license completeness) → Tasks 1 & 2, Step 3; asserted in Step 6. ✓
- Drop `3.12` classifier / no License classifier / no Development Status → Global Constraints + Step 1 tables. ✓
- Moon `build.inputs` += README.md, LICENSE → Task 3, Step 1. ✓
- Build via Moon invocation; kernel without cargo → Tasks 1 & 2 Step 4. ✓
- METADATA assertions (2.4, content-type, License-File, classifiers) → Step 5. ✓
- py.typed honesty → Step 6. ✓
- `py:*` gates green → Task 3, Step 3. ✓
- Prettier unaffected → no `ts` task needed (verified in spec); not a step. ✓

**Placeholder scan:** No TBD/TODO/"handle edge cases"; every code/command step shows exact content. ✓

**Type/name consistency:** filenames (`README.md`, `LICENSE`), project ids (`paigasus-proto-py`, `paigasus-kernel-py`), classifier list, and `License-File: LICENSE` are identical across tasks. ✓
