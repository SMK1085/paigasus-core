# SMA-358 — Bootstrap `py/` uv workspace with basedpyright + ruff

**Status:** Designed (brainstorming complete; revised after staff-eng review)
**Date:** 2026-05-26
**Linear:** [SMA-358](https://linear.app/smaschek/issue/SMA-358/bootstrap-py-uv-workspace-with-basedpyright-ruff-config)
**Branch:** `feature/sma-358-bootstrap-py-uv-workspace-with-basedpyright-ruff-config`
**References:** ADR-0002 (basedpyright over mypy/pyright), Python development guidelines (Notion).
A staff-eng design review was incorporated (disposition in §H) and then removed.

## Goal

Scaffold the Python workspace under `py/` with the uv workspace conventions and the toolchain
decisions from ADR-0002: a virtual uv workspace root holding shared dev tooling and all tool
config, four inert stub packages that are first-class Moon projects, and a `py` parent Moon
project that runs the workspace-wide quality gates. No real package logic — bootstrapping only.

## Key decisions

1. **Moon topology — nested: per-package projects + a `py` parent project.** `py/packages/*` are
   real Moon projects (identity, CODEOWNERS, future per-package `build`, affected-graph nodes), and
   a `py` parent project owns the workspace-wide gates (`lint`/`format`/`typecheck`/`test`). This is
   Moon's documented root-level-project / nesting pattern. **Why a parent project rather than
   per-package gates:** a Moon task runs only from its project dir or the *repo* root — never from
   `py/` (`runFromWorkspaceRoot` is the repo root, where `.moon/` lives). basedpyright resolves its
   config from the cwd/project root and does **not** walk up parents; pytest's `testpaths` globs all
   packages from the shared rootdir. So the only cwd where bare `basedpyright`/`ruff`/`pytest` all
   resolve `py/pyproject.toml` is `py/` itself — which means `py/` must be a project. The gates run
   once over the whole workspace (cheap, and catches cross-package type errors); `build` stays
   per-package. **No future migration** when a package gets real build needs — it is already a project.

2. **Build backend — `uv_build`.** Astral's native PEP 517 backend (stable since uv 0.8; we pin
   0.11.16). Zero extra dependency, assumes the `src/<module>/` layout, ships `py.typed`
   automatically. `uv sync` installs each workspace member as editable, which is why every package
   needs a backend even though "build" is not an AC.

3. **anyio over pytest-asyncio** (amended AC). The guidelines are anyio-first; we use
   `@pytest.mark.anyio` + `anyio[trio]` and drop `asyncio_mode = "auto"`. Linear AC corrected.

4. **Typecheck tests too** (amended AC). basedpyright `include` covers `packages/*/src` **and**
   `packages/*/tests`, so `typeCheckingMode = "all"` applies to test code.

5. **Pinned dev tools.** Dev tools carry bounded version constraints in `pyproject.toml` (not bare
   names), so a lock regeneration can't silently bump basedpyright/ruff rule behavior.

## A. Topology & Moon wiring

- `.moon/workspace.yml` `projects`: **add `'py'`**, keep `'py/packages/*'`. Moon permits a project
  whose source contains nested project sources (its root-level-project pattern — a project at `.`
  contains every other project). CODEOWNERS regenerates via `codeowners.sync`; do **not** hand-edit
  `.github/CODEOWNERS`.
- `py/moon.yml` — the parent project that owns the gates:
  - `language: 'python'`, no `type`.
  - Explicit `fileGroups` (the global groups in `.moon/tasks.yml` assume `src/` at the project root):
    `sources: ['packages/*/src/**/*']`, `tests: ['packages/*/tests/**/*']`.
  - Tasks (all via `uv run` so they resolve from `py/.venv` regardless of Moon venv activation):
    - `lint`: `uv run ruff check .`
    - `format`: `uv run ruff format --check .`
    - `typecheck`: `uv run basedpyright`
    - `test`: `uv run pytest`
  - Each task sets explicit `inputs` (relevant file group + `pyproject.toml` + `uv.lock`) so the
    parent project's tasks don't default to the greedy `**/*` (the documented root-project caveat).
  - No `build` task (the root is a virtual uv workspace; `uv build` there is awkward).
- `py/packages/<name>/moon.yml` — minimal per-package projects: an explicit `id: 'paigasus-<name>-py'`
  plus `language: 'python'`, no tasks yet. They exist for identity/CODEOWNERS/affected-graph; a
  `build` task is added per package when one needs to produce an artifact.
- **Moon project-ID convention (`-py` suffix).** Moon derives a project's ID from its directory leaf,
  so `py/packages/paigasus-kernel` collides with the Rust crate `rs/crates/libs/paigasus-kernel`
  (both → `paigasus-kernel`), and `paigasus-proto` would collide once a Rust proto crate lands. All
  four Python package projects therefore carry an explicit `-py`-suffixed id (`paigasus-proto-py`,
  `paigasus-kernel-py`, `paigasus-ml-py`, `paigasus-workflows-py`). The `py` parent project keeps the
  derived id `py` (so the gates stay `moon run py:<task>`). The Rust crates get the mirror `-rs`
  suffix in a follow-up (SMA-380); `rs/` is not touched here.
- **No `.moon/tasks/python.yml` yet.** A language-scoped inherited task file would attach the same
  tasks to *both* the `py` parent and every package (the gates belong only on `py`; `build` only on
  packages), so inheritance doesn't separate them cleanly. When packages start needing `build`,
  either add a per-package `build` task or introduce a carefully-scoped `python.yml` then.
- `.moon/templates/python` stays in place for scaffolding future package `moon.yml`s; its task
  commands are updated to the `uv run <tool>` form too (and `uv run python -m <pkg>` for `start`), so
  generated projects stay consistent with the `py` parent project (resolves review S5).

## B. Workspace root — `py/pyproject.toml` (virtual root)

No `[project]` table. Holds the workspace declaration, the shared (pinned) dev toolchain, and all
tool config:

```toml
[tool.uv.workspace]
members = ["packages/*"]

# Bounded constraints so a lock regen can't silently bump rule behavior (basedpyright/ruff).
# Exact lower bounds resolved to current latest at implementation; uv.lock pins the exact versions.
[dependency-groups]
dev = [
  "basedpyright>=1.X,<2",    # exact lower bound = latest at impl; uv.lock pins the resolved version
  "ruff>=0.X,<0.Y",          # ruff is pre-1.0; pin to the minor (e.g. >=0.11,<0.12)
  "pytest>=8,<9",
  "anyio[trio]>=4,<5",       # anyio ships the pytest plugin (@pytest.mark.anyio)
]

# Python version lives in THREE places that must move together on a floor bump:
# requires-python (per package), tool.basedpyright.pythonVersion, tool.ruff.target-version.
[tool.basedpyright]
typeCheckingMode = "all"
pythonVersion = "3.12"
include = ["packages/*/src", "packages/*/tests"]   # tests typechecked too (amended AC)
exclude = ["**/__pycache__", "**/node_modules", "**/.venv", "**/dist", "**/build"]
reportMissingTypeStubs = "warning"                  # ML libs without stubs warn, don't fail
reportUnnecessaryTypeIgnoreComment = "error"
reportImplicitOverride = "error"
reportImportCycles = "error"
# reportAny is already enabled by typeCheckingMode = "all"; not restated.

[tool.ruff]
line-length = 100
target-version = "py312"

[tool.ruff.lint]
select = ["E", "F", "W", "I", "N", "UP", "B", "A", "C4", "SIM", "TCH", "RUF"]
ignore = ["E501"]   # line length handled by the formatter

[tool.pytest.ini_options]
testpaths = ["packages/*/tests"]
markers = [
  "integration: integration tests that may hit real infrastructure",
  "slow: tests that take more than 1s",
]
# No asyncio_mode: async tests use @pytest.mark.anyio (anyio-first per the guidelines).
```

The `dev` group installs by default on `uv sync`, including in a virtual workspace root. Direct deps
pinned (per the guidelines); `uv.lock` pins transitive.

`py/.python-version` pins `3.12.13` (matching `.moon/toolchain.yml`). Without it, raw `uv` selects the
newest interpreter satisfying `requires-python >=3.12` (e.g. 3.14 on the dev machine), and Moon's
`uv run` reuses whatever `py/.venv` already exists — so nothing actually runs on the pinned 3.12.
The `.python-version` file makes both raw uv and Moon build the venv on 3.12.13, which is the point of
pinning. (This supersedes the original reliance on "run via Moon" for version parity, which did not
hold in practice.)

## C. The four stub packages

`paigasus-proto`, `paigasus-kernel`, `paigasus-ml`, `paigasus-workflows`. Each:

```text
py/packages/<name>/
├── moon.yml                 # id 'paigasus-<name>-py', language python (identity; no tasks yet)
├── pyproject.toml
└── src/<module>/            # module = name with - → _, e.g. paigasus_proto
    ├── __init__.py          # SPDX header only
    └── py.typed             # empty PEP 561 marker
```

Each `pyproject.toml`:

```toml
[project]
name = "paigasus-proto"
version = "0.0.0"
requires-python = ">=3.12"
dependencies = []
# TODO(SMA-378): before first PyPI publish, paigasus-proto & paigasus-kernel need
# description/readme/license = "Apache-2.0"/authors/classifiers (ADR-0006). (PyPI-bound only.)

[build-system]
requires = ["uv_build>=0.11.16,<0.12"]
build-backend = "uv_build"
```

`uv_build` derives the module name from the normalized project name and includes everything under
`src/<module>/`, so `py.typed` ships without extra config. Module names: `paigasus_proto`,
`paigasus_kernel`, `paigasus_ml`, `paigasus_workflows`.

Roles (forward-looking, no code yet): `paigasus-proto` (generated proto types post-MVP — pure
re-export wrapper; the maturin-built native artifact comes from
`rs/crates/bindings/paigasus-py-bindings/` and is consumed as a wheel), `paigasus-kernel` (thin
re-export wrapper over the PyO3 binding post-MVP), `paigasus-ml` (ML lifecycle), `paigasus-workflows`
(Python-native workflows). `uv_build` is correct for all four (none builds native code itself).

## D. Conventions, the "0 tests" shim, and the venv assumption

- **Gate commands run via `uv run`.** Every gate task invokes `uv run <tool>` (`uv run ruff`,
  `uv run basedpyright`, `uv run pytest`), which resolves the tool from `py/.venv` deterministically
  regardless of whether Moon's toolchain puts the venv on PATH. `.moon/templates/python` is updated
  to the same `uv run` form so scaffolded projects stay consistent (resolves review S5).
- **pytest exits 5 on "no tests collected"**, which Moon treats as failure — same shape as the
  `cargo nextest --no-tests=pass` gotcha in CLAUDE.md. A dependency-free root `py/conftest.py` maps
  it to success, but **selectively** (per review S4) to avoid masking a real test-discovery
  regression once tests exist:

  ```python
  # SPDX-License-Identifier: Apache-2.0
  # TODO(SMA-379): remove this shim once at least one package has tests; until then it keeps the
  # empty workspace green. The on-disk guard means it does NOT mask a "discovery broke" regression
  # in a package that previously had tests.
  import pytest


  def pytest_sessionfinish(session: pytest.Session, exitstatus: int) -> None:
      if exitstatus == pytest.ExitCode.NO_TESTS_COLLECTED and not any(
          p.is_dir() for p in session.config.rootpath.glob("packages/*/tests")
      ):
          session.exitstatus = 0
  ```

  Once any `packages/*/tests/` directory exists, a zero-collection result is treated as the failure
  it is.
- **SPDX headers** (`# SPDX-License-Identifier: Apache-2.0`) on every `.py` file (`__init__.py`,
  `conftest.py`). Not on `pyproject.toml`/`moon.yml`/`py.typed` — config files and markers carry no
  header, consistent with `Cargo.toml` in `rs/`.
- **`py/README.md`** updated from "empty until the uv workspace lands" to describe the real layout,
  plus two operator notes:
  - Per-package install for CI / fast iteration: `uv sync --package <name>` (the workspace lock
    resolves all packages' deps together; N1).
  - For env parity, invoke uv via `moon run py:<task>` so Moon's pinned Python is used, not whatever
    `uv python find` discovers (N3).
  - `paigasus-kernel`/`paigasus-proto` are expected to ship complete `.pyi` stubs from their codegen
    pipelines; `reportMissingTypeStubs = "warning"` is for third-party ML libs, not an excuse for
    missing first-party stubs (N2).

## E. Verification (maps to acceptance criteria)

Run via Moon (cwd resolves to `py/` for the `py` project):

| Acceptance criterion | Verification |
| --- | --- |
| `py/pyproject.toml` with `[tool.uv.workspace] members = ["packages/*"]` | File present (§B) |
| Four stub packages with `pyproject.toml`, `src/<pkg>/__init__.py`, `py.typed` | Files present (§C) |
| basedpyright config (strict set; incl. tests) | `[tool.basedpyright]` present (§B) |
| ruff config (rule set) | `[tool.ruff.lint] select` present (§B) |
| pytest config (anyio + markers) | `[tool.pytest.ini_options]` present (§B) |
| `uv sync` succeeds | `uv sync` exits 0; `uv.lock` written |
| `basedpyright` passes on the empty workspace | `moon run py:typecheck` exits 0 |
| `ruff check .` and `ruff format --check .` pass | `moon run py:lint` + `py:format` exit 0 |
| `uv run pytest` runs cleanly (0 tests collected) | `moon run py:test` exits 0 (via §D shim) |

Sanity: `moon ci :build`-equivalent resolves the graph with `py` + the four package projects
registered and no errors about overlapping/nested project sources.

## F. Out of scope

Real package code, proto generation, the PyO3/maturin wiring, per-package `build` tasks, lefthook
hooks (SMA-371).

## G. Future deltas (telegraphed so downstream PRs don't surprise reviewers)

- **SMA-360 (proto codegen):** `paigasus-proto`'s `dependencies` grow to include the generated-code
  runtime (`betterproto2` per Polyglot Monorepo Scoping § 2, or fallback `grpcio-tools` +
  `mypy-protobuf`). Other stubs stay dependency-free until their roles land. (Review S7.)
- **First package `build`:** when a package must produce an artifact, add a per-package `build` task
  or a scoped `.moon/tasks/python.yml`; do not duplicate task definitions across packages. (N6.)
- **First PyPI publish (SMA-378):** add `description`/`readme`/`license`/`classifiers` to the
  PyPI-bound packages (the TODO in §C). (N4.)
- **First package with tests (SMA-379):** remove the conftest exit-5 shim (§D).
- **First real `.pyi` stubs:** consider per-package `reportMissingTypeStubs = "error"` for
  `paigasus-kernel`/`paigasus-proto`. (N2.)
- **SMA-380 (rust `-rs` ids):** mirror this issue's `-py` Moon-id convention on the Rust crates
  (`paigasus-kernel-rs`, …) for cross-stack consistency. Touches landed SMA-357 crates and the
  `cargo -p $project` task wiring, so tracked separately.

## H. Review disposition

Disposition of the staff-eng design review (since removed; this section preserves the outcome):

- **Applied:** B1 (chose nested topology — corrects the review's `runFromWorkspaceRoot` premise:
  that flag is the repo root, not `py/`), S1 (anyio; AC corrected), S2 (typecheck tests), S3 (pin
  dev tools), S4 (selective conftest shim + TODO → SMA-379), S5 (standardized on `uv run`; template
  updated to match), S6 (version-of-truth comment), S7 (proto-deps note), N8 (exclude
  `dist`/`build`), N1/N2/N3 (README notes), N4 (TODO → SMA-378), N6 (future-delta note).
- **Noted, no config change:** N5 (no `fix`/`format-fix` tasks — matches `rs/`; revisit as a
  polyglot-wide convention), N7 (`reportAny` already on under `typeCheckingMode = "all"`).
- **Withdrawn by reviewer:** B2 (`paigasus-kernel` is a pure re-export wrapper; `uv_build` correct).
