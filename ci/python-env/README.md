# ci/python-env

`run.sh` is the one place that runs the Python package manager, pip and the proto install of the
package manager. A caller runs `bash ci/python-env/run.sh <operation> [args...]`. Spec:
`docs/superpowers/specs/2026-11-14-sma-770-python-env-wrapper-token-ban-design.md` section 5.1.

Exit codes: 2 for a usage error (the message starts with `python-env:`), 127 for a missing tool.
Every other code is the code of the program that the wrapper runs.

## Operations

| Operation | Command |
|---|---|
| `run py <cmd...>` | `uv run --locked -- <cmd...>` (the directory must be inside `py/`) |
| `run <ci-project> <cmd...>` | `uv run --locked --project <root>/<ci-project> --python '>=3.12' -- <cmd...>` |
| `run-frozen py <cmd...>` | `uv run --frozen -- <cmd...>` (same directory rule) |
| `protoc-plugin` | `uv run --locked --project <root>/py protoc-gen-python_betterproto2` |
| `kernel-test` | in `py/packages/paigasus-kernel`: `uv run --isolated --reinstall-package paigasus-py-bindings pytest tests` |
| `script <file.py> [args...]` | `uv run --isolated --no-project --python '>=3.12' python3 <file.py> [args...]` |
| `build` | `uv build` in the caller's directory |
| `build <package> <outdir>` | in `py/`: `uv build --package <package> --out-dir <absolute outdir>` |
| `lock` | in `py/`: `uv lock` |
| `cache-dir` | `uv cache dir` |
| `install` | `proto install uv` |
| `probe` | `command -v uv`; exit 0 or 127, no output |
| `install-ziglang <python>` | `<python> -m pip install ziglang` |
| `install-ziglang-system <python>` | `<python> -m pip install --break-system-packages ziglang` |
| `wheel-smoke <venv-dir> <wheel>` | `<venv python> -m pip install --no-index --disable-pip-version-check <wheel>` |
| `sdist-install <venv-dir> <sdist>` | `<venv pip> install <sdist>` |

The venv python is `<venv-dir>/bin/python`, else `<venv-dir>/Scripts/python.exe`. The venv pip is
`<venv-dir>/bin/pip`, else `<venv-dir>/Scripts/pip.exe`. If neither exists, the exit code is 127.
Operations with a fixed command refuse extra arguments (exit 2).

## Fixed lists

- `<ci-project>`: `ci/release-plan`, `ci/workflow-credentials`, `ci/wasm-lockstep`, `ci/helm-render`.
- `<package>`: `paigasus-proto`, `paigasus-kernel`.

Any other value exits 2.

## Rules

- R-cwd: `run py` and `run-frozen py` pass no `--project`. They check only that the directory is
  inside `py/`. A sibling such as `py-x/` or the repository root is refused.
- R-outdir: `build <package> <outdir>` makes the out dir absolute before it changes to `py/`. It
  refuses an empty value and a value that starts with `-`.
- R-lock: `lock` exits 2 when `MOON_PROJECT_ID` is set, also when it is empty.
- R-build: `build` never passes `--no-build-isolation`.
- R-script: `script` refuses a file that starts with `-`.
- Style: each uv, pip or proto command is one literal line in one `case` branch. No flag is held
  in a variable or an array. The wrapper does not unset `UV_*` variables.
- Bash: the wrapper runs under bash 3.2, bash 5, and Git bash on Windows. Do not use `mapfile`,
  `declare -A`, `local -n` or a here-string.

## Tests

`ci/affected-graph/python_env_ban.py` (`wrapper_behaviour_failures`) runs the wrapper with fake
tools on `PATH`. The `PYTHON_ENV_TEST_BASH` environment variable selects the bash binary
(default `/bin/bash`):

```bash
python3 -I ci/affected-graph/python_env_ban.py
PYTHON_ENV_TEST_BASH=/opt/homebrew/bin/bash python3 -I ci/affected-graph/python_env_ban.py
```

## How to add an operation

1. Add one `case` branch to `run.sh`. Write the command as one literal line.
2. Add the operation to the table above.
3. Add rows to `WRAPPER_CASES` in `ci/affected-graph/python_env_ban.py`: one success row and one
   usage-error row at least.
4. Update the sha256 of `run.sh` that the gate pins, when the gate exists.
5. Run both commands in the Tests section.
