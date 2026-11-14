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

## After a Rust change

`py:test` and `py:typecheck` use the binding that is installed in `py/.venv`. No Moon task
rebuilds that binding after a Rust source change (SMA-765). The kernel test builds its own isolated environment instead. To
refresh the binding when no Moon run is active, run this from the repository root:
`touch rs/crates/libs/paigasus-kernel/src/lib.rs rs/crates/bindings/paigasus-py-bindings/src/lib.rs && (cd py && uv sync --all-packages --reinstall-package paigasus-py-bindings)`.
The `touch` forces a rebuild from the current Rust source. Without it, a checkout or rebase can
leave a source file older than the cached build, and the reinstall then gets a stale binding
(the same reason as the `touch` in the kernel test task).
After you pull the SMA-767 change, run this command once. `py/.venv` keeps the old binding, and macOS 27 cannot
load it. See the SMA-767 entry in `rs/CLAUDE.md`.

## After a `pyproject.toml` change

Every Moon task runs `uv run --locked` (SMA-765 R-relock). If you change a `pyproject.toml` and do
not relock, `py:lint`, `py:fmt`, `py:typecheck` and `py:test` stop with a uv error that says the
lockfile must be updated. Run `(cd py && uv lock)` when no Moon run is active, and commit `py/uv.lock`.
The tasks do not relock by themselves, because a relock during a run can change `py/.venv` while
another task reads it. `repo:affected-smoke` (A14) reds a `uv run` with no `--locked`, `--frozen`
or `--isolated`.

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
