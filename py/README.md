# py/

Python workspace for paigasus-core, managed with [uv](https://docs.astral.sh/uv/) and
orchestrated by [Moon](https://moonrepo.dev).

## Layout

- `pyproject.toml` — virtual uv workspace root. Declares the workspace members, the shared dev
  toolchain (basedpyright, ruff, pytest, anyio), and all tool config. No `[project]` table — the
  root is not itself a package.
- `.python-version` — pins CPython `3.12.13` (matching `.moon/toolchains.yml`) so uv builds the
  workspace venv on the project's Python, whether invoked directly or via Moon.
- `packages/*` — one package per bounded context; each is a uv workspace member and a Moon project
  (id `paigasus-<name>-py`, suffixed to avoid colliding with the same-named Rust crates):
  - `paigasus-proto` — generated protobuf types.
  - `paigasus-kernel` — thin re-export wrapper over the PyO3 binding.
  - `paigasus-ml` — ML lifecycle code.
  - `paigasus-workflows` — Python-native workflows.

## Commands

The quality gates live on the `py` Moon project and run once over the whole workspace from `py/`
(where the single tool config lives):

| Task | Command |
| --- | --- |
| Lint | `moon run py:lint` |
| Format check | `moon run py:fmt` |
| Type check | `moon run py:typecheck` |
| Test | `moon run py:test` |

Notes:

- The workspace lock resolves all packages' dependencies together. For a faster CI / iteration
  install of a single package's deps: `uv sync --package <name>`.
- `py:test` runs the suite and then a **per-package collection floor**
  (`scripts/assert_test_floor.py`, via `scripts/run_tests.sh`). `testpaths = ["packages/*/tests"]`
  is glob-expanded and concatenated, so losing ONE package's `tests/` directory leaves pytest
  collecting the survivors at exit 0 with no warning at all — measured, 134 passed silently became
  7 passed. The floor pins which packages must contribute tests (`EXPECTED_TEST_PACKAGES`) and
  which are exempt with a stated reason (`NO_TESTS_EXPECTED`), compared by strict equality against
  what pytest actually collected. So **adding a package with tests, or removing a package's tests,
  is a deliberate edit to that file**. Exercise it with
  `uv run python scripts/assert_test_floor.py --self-test`.
- The floor is skipped when you pass arguments through (`moon run py:test -- -k parity`), since a
  filtered run legitimately collects from only some packages.
- `paigasus-kernel` and `paigasus-proto` are expected to ship complete `.pyi` type stubs from their
  codegen pipelines; `reportMissingTypeStubs = "warning"` is for third-party ML libraries, not a
  license to skip first-party stubs.

**Status:** workspace bootstrapped in SMA-358. `paigasus-proto` and `paigasus-kernel` ship code
and test suites; `paigasus-ml` and `paigasus-workflows` are still empty stubs.
