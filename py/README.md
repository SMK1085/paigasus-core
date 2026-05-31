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
  - `paigasus-proto` — generated protobuf types (post-MVP).
  - `paigasus-kernel` — thin re-export wrapper over the PyO3 binding (post-MVP).
  - `paigasus-ml` — ML lifecycle code.
  - `paigasus-workflows` — Python-native workflows.
- `conftest.py` — keeps `pytest` green on the empty workspace (0 tests collected); removed once
  real tests land (SMA-379).

## Commands

The quality gates live on the `py` Moon project and run once over the whole workspace from `py/`
(where the single tool config lives):

| Task | Command |
| --- | --- |
| Lint | `moon run py:lint` |
| Format check | `moon run py:format` |
| Type check | `moon run py:typecheck` |
| Test | `moon run py:test` |

Notes:

- The workspace lock resolves all packages' dependencies together. For a faster CI / iteration
  install of a single package's deps: `uv sync --package <name>`.
- `paigasus-kernel` and `paigasus-proto` are expected to ship complete `.pyi` type stubs from their
  codegen pipelines; `reportMissingTypeStubs = "warning"` is for third-party ML libraries, not a
  license to skip first-party stubs.

**Status:** workspace bootstrapped in SMA-358; packages are empty stubs.
