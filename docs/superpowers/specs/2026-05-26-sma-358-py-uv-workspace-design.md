# SMA-358 — Bootstrap `py/` uv workspace with basedpyright + ruff

**Status:** Designed (brainstorming complete)
**Date:** 2026-05-26
**Linear:** [SMA-358](https://linear.app/smaschek/issue/SMA-358/bootstrap-py-uv-workspace-with-basedpyright-ruff-config)
**Branch:** `feature/sma-358-bootstrap-py-uv-workspace-with-basedpyright-ruff-config`
**References:** ADR-0002 (basedpyright over mypy/pyright), Python development guidelines (Notion)

## Goal

Scaffold the Python workspace under `py/` with the uv workspace conventions and the
toolchain decisions from ADR-0002: a virtual uv workspace root holding shared dev tooling
and all tool config, four inert stub packages, and a single Moon project that runs the
workspace-wide quality gates. No real package logic — this is bootstrapping only.

## Key decisions

Two architectural forks were resolved during brainstorming:

1. **Moon topology — one `py/` root project (not per-package).** Every acceptance criterion
   is a workspace-root invocation (`ruff check .`, `basedpyright` over `packages/*/src`,
   `uv run pytest` over `packages/*/tests`), and the config lives once in `py/pyproject.toml`.
   Running the tools per-package would make `basedpyright`/`pytest` re-discover config they
   don't own (pyright does not walk up the tree for config), fighting the single-root-config
   design. A single `py` Moon project also sidesteps Moon's nested-project gotcha. The four
   packages are uv workspace members but **not** individual Moon projects yet; they get
   promoted when they grow real build needs (e.g. `paigasus-kernel` wrapping the PyO3 binding
   via maturin).

2. **Build backend — `uv_build`.** Astral's native PEP 517 backend (stable since uv 0.8;
   we pin 0.11.16). Zero extra dependency, assumes the `src/<module>/` layout we already use,
   and ships `py.typed` automatically when it lives inside the module directory. `uv sync`
   installs each workspace member as editable, which is why every package needs a build
   backend even though "build" is not an acceptance criterion.

## A. Topology & Moon wiring

- New `py/moon.yml`: `language: 'python'`, no `type` (mirrors `paigasus-kernel`'s `moon.yml`,
  which sets only `language`). Defines its own `fileGroups` because the global groups in
  `.moon/tasks.yml` assume `src/` at the project root:
  - `sources`: `packages/*/src/**/*`
  - `tests`: `packages/*/tests/**/*`
- Tasks (commands are bare — Moon's uv toolchain puts `py/.venv` on PATH, matching the
  existing `.moon/templates/python` template):
  - `lint`: `ruff check .`
  - `format`: `ruff format --check .`
  - `typecheck`: `basedpyright`
  - `test`: `uv run pytest`
  - Task `inputs` include the relevant file group plus `pyproject.toml` and `uv.lock`.
- **No `build` task.** The root is a *virtual* uv workspace (no `[project]` table), so
  `uv build` there is awkward; packages get build tasks when promoted to their own Moon
  projects.
- `.moon/workspace.yml`: replace the `'py/packages/*'` entry in `projects` with `'py'`.
  This avoids the nested-project gotcha (a `py` project cannot contain `py/packages/*`
  projects). `.github/CODEOWNERS` regenerates via `codeowners.sync` — do **not** hand-edit it.
- The existing `.moon/templates/python` template stays in place, dormant, for the future
  promotion of a package to its own Moon project.

## B. Workspace root — `py/pyproject.toml` (virtual root)

No `[project]` table. Holds the workspace declaration, the shared dev toolchain, and all
tool config (verbatim from the Python development guidelines):

```toml
[tool.uv.workspace]
members = ["packages/*"]

[dependency-groups]
dev = ["basedpyright", "ruff", "pytest", "pytest-asyncio"]   # pinned to current versions at impl

[tool.basedpyright]
typeCheckingMode = "all"
pythonVersion = "3.12"
include = ["packages/*/src"]
exclude = ["**/__pycache__", "**/node_modules", "**/.venv"]
reportMissingTypeStubs = "warning"
reportUnnecessaryTypeIgnoreComment = "error"
reportImplicitOverride = "error"
reportImportCycles = "error"

[tool.ruff]
line-length = 100
target-version = "py312"

[tool.ruff.lint]
select = ["E", "F", "W", "I", "N", "UP", "B", "A", "C4", "SIM", "TCH", "RUF"]
ignore = ["E501"]   # line length handled by the formatter

[tool.pytest.ini_options]
testpaths = ["packages/*/tests"]
asyncio_mode = "auto"
markers = [
  "integration: integration tests that may hit real infrastructure",
  "slow: tests that take more than 1s",
]
```

`pytest-asyncio` is in the dev group because `asyncio_mode = "auto"` is its ini key. The
guidelines specify this setting even though their prose prefers `anyio`; when real async
tests land we revisit adding the anyio pytest plugin. The dev group installs by default on
`uv sync`, including in a virtual workspace root.

Direct dependencies are pinned (per the guidelines); `uv.lock` pins transitive. The
`dev` versions are resolved to current latest at implementation time and committed in
`uv.lock`.

## C. The four stub packages

`paigasus-proto`, `paigasus-kernel`, `paigasus-ml`, `paigasus-workflows`. Each:

```
py/packages/<name>/
├── pyproject.toml
└── src/<module>/            # module = name with - → _, e.g. paigasus_proto
    ├── __init__.py          # SPDX header only
    └── py.typed             # empty PEP 561 marker
```

Each `pyproject.toml` is minimal:

```toml
[project]
name = "paigasus-proto"
version = "0.0.0"
requires-python = ">=3.12"
dependencies = []

[build-system]
requires = ["uv_build>=0.11.16,<0.12"]
build-backend = "uv_build"
```

`uv_build` derives the module name from the normalized project name and includes everything
under `src/<module>/`, so `py.typed` ships without extra config. Module names:
`paigasus_proto`, `paigasus_kernel`, `paigasus_ml`, `paigasus_workflows`.

Package roles (forward-looking, no code yet): `paigasus-proto` (generated proto types
post-MVP), `paigasus-kernel` (wraps the PyO3 binding post-MVP), `paigasus-ml` (ML lifecycle),
`paigasus-workflows` (Python-native workflows).

## D. The "0 tests" gotcha + conventions

- **pytest exits 5 on "no tests collected"**, which Moon treats as a failure — the same
  shape as the `cargo nextest --no-tests=pass` gotcha already documented in CLAUDE.md. Fix
  with a dependency-free root `py/conftest.py` that maps `NO_TESTS_COLLECTED` → exit 0 in a
  `pytest_sessionfinish` hook by setting `session.exitstatus = 0`. (Chosen over the
  `pytest-custom-exit-code` plugin to avoid a dependency.) The rootdir `conftest.py` is
  always loaded regardless of `testpaths`.
- **SPDX headers** (`# SPDX-License-Identifier: Apache-2.0`) on every `.py` file
  (`__init__.py`, `conftest.py`). Not on `pyproject.toml` or `py.typed` — config files and
  markers carry no header, consistent with `Cargo.toml` in `rs/`.
- `py/README.md` updated from "empty until the uv workspace lands" to describe the real
  layout (workspace root, four stub packages, how to run the checks via Moon).

## E. Verification (maps 1:1 to acceptance criteria)

Run from `py/`:

| Acceptance criterion | Verification |
| --- | --- |
| `py/pyproject.toml` with `[tool.uv.workspace] members = ["packages/*"]` | File present (§B) |
| Four stub packages with `pyproject.toml`, `src/<pkg>/__init__.py`, `py.typed` | Files present (§C) |
| Workspace basedpyright config | `[tool.basedpyright]` present (§B) |
| Workspace ruff config (rule set) | `[tool.ruff.lint] select` present (§B) |
| Workspace pytest config (`asyncio_mode`, markers) | `[tool.pytest.ini_options]` present (§B) |
| `uv sync` succeeds | `uv sync` exits 0; `uv.lock` written |
| `basedpyright` passes on the empty workspace | `moon run py:typecheck` exits 0 |
| `ruff check .` and `ruff format --check .` pass | `moon run py:lint` + `py:format` exit 0 |
| `uv run pytest` runs cleanly (0 tests collected) | `moon run py:test` exits 0 (via §D shim) |

Sanity: `moon ci :build` / equivalent resolves the project graph with `py` registered and
no nested-project warnings.

## F. Out of scope

Real package code, proto generation, the PyO3/maturin wiring, per-package Moon projects,
lefthook hooks (SMA-371).
