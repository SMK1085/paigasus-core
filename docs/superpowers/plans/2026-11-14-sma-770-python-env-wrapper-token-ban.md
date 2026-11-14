# SMA-770 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the A14 shell parser with one Python-env wrapper (`ci/python-env/run.sh`) and a
plain token ban. Ship kernel 0.2.1 with the SMA-767 fix. Find a darwin-only host-strip setting, or
record the measured reason why none exists.

**Architecture:** Every uv, pip, proto-install and `.venv` operation in the tree goes through one
bash 3.2 wrapper with fixed operations and literal commands. A new module
`ci/affected-graph/python_env_ban.py` bans the tokens on every tracked line outside a short
exclusion list and pins the wrapper by sha256. `cargo_moon_parity.py` calls it under the `a14` key.

**Tech Stack:** bash 3.2+, Python 3.12 stdlib (`re`, `hashlib`, `tomllib`, `subprocess`), Moon
2.5.3, uv 0.11.16, GitHub Actions, cargo 1.95, Docker `rust:1.95.0-bookworm`.

**Spec:** `docs/superpowers/specs/2026-11-14-sma-770-python-env-wrapper-token-ban-design.md`

## Global Constraints

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-770`, branch
  `feature/sma-770-close-the-sma-765767768-residuals-for-good`. Do not work in the parent checkout.
- Every new source file opens with an SPDX header: `# SPDX-License-Identifier: Apache-2.0`.
- The wrapper runs under `/bin/bash` 3.2, Homebrew bash 5.x, Linux CI bash and Git bash on
  `windows-latest`. Do not use `mapfile`, `declare -A`, `local -n`, `<<<`, or `"${arr[@]}"` on a
  possibly empty array under `set -u`.
- The wrapper starts with `set -euo pipefail` and `export PROTO_REPORTER=text`.
- Wrapper exit codes: usage error 2 with the prefix `python-env:`; missing tool 127; every other
  code of the program passes through unchanged.
- Style rule: each uv, pip or proto command is one literal line in one `case` branch. No flag in a
  variable or an array.
- `<ci-project>` list: `ci/release-plan`, `ci/workflow-credentials`, `ci/wasm-lockstep`,
  `ci/helm-render`. `<package>` list: `paigasus-proto`, `paigasus-kernel`.
- Token list (case-sensitive): `uv`, `uvx`, `pip`, `pip3`, `pip3.<n>`, `ensurepip`, `virtualenv`,
  `.venv`, `UV_[A-Z0-9_]+`, `VIRTUAL_ENV`, `--active`.
- Token boundary: `(?<![\w.-])TOKEN(?![\w.-])`; for `.venv` the left side is `(?<![\w-])`.
- Known non-matches: `uv.lock`, `uv.toml`, `uv-cache`, `pip.exe`, `pipefail`, `pipeline`,
  `uvicorn`, `uv.` at the end of a sentence.
- Never split a token (`"u""v"`, `u\v`, `${x}v`) to pass the ban. A split is a defect.
- The count of `PINNED_LINES` must be lower than 35. Each entry has a reason that says why the text
  cannot change.
- Conventional commits with a scope: `feat(ci)`, `fix(ci)`, `fix(rs)`, `docs(ci)`, `ci(ci)` etc.
  No body line starts with `BREAKING CHANGE` or `BREAKING-CHANGE`. No `!` after the type or scope.
  No body line starts with `#` followed by digits (commitlint `footer-leading-blank`).
- Every commit ends with a blank line and
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never use `--no-verify`.
- The PR title starts with `fix(rs):`.
- Local `ci/affected-graph/run.sh` needs system `/bin/bash` 3.2. `ci/ruff/run.sh`,
  `ci/publish-metadata/run.sh`, `ci/next-public/run.sh` and `ops/nats/check-subjects.sh` need
  `/opt/homebrew/bin/bash`. `ci/actionlint/run.sh` needs bash 5 and a pipe preflight of 65536 bytes.
- Prefix shell commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Do not install host software. Docker is the only way to get a Linux build.
- `SCRATCH` means a directory outside the repository, for example
  `SCRATCH="$TMPDIR/sma-770"`. Make it once with `mkdir -p "$SCRATCH"`.
- Restore a mutation by deleting the inserted text. Do not use `git checkout --` on a file that
  also holds a fix.

## Review Focus

These are the input classes that the spec implies but that no spec row tests. The owning task adds
a test for each.

1. **Token next to shell or YAML punctuation** (Task 3). `$(uv run`, `` `uv` ``, `'uv'`, `"uv"`,
   `(uv)`, `uv;`, `uv:`, `bin/uv`, `[uv]`, `=uv` must be red. `-uv`, `uv-x`, `x.uv`, `uv.lock`
   must be green. The boundary is the whole gate, so a wrong character class is a silent hole.
2. **CRLF and the last line with no newline** (Task 3). A token on a CRLF line is red. A pinned line
   in a CRLF file matches its stripped text. A token on a last line with no `\n` is red.
3. **Non-UTF-8 and attribute edge cases** (Task 3). A non-UTF-8 file marked `binary` is skipped. A
   non-UTF-8 file marked only `-text` raises. A tracked path with a space is scanned. A tracked
   file that is missing on disk raises; it is not skipped.
4. **Wrapper argument transport** (Task 2). After `--`, an empty argument, an argument with a space
   and an argument `--locked` reach the program unchanged and in order. `script` refuses a file
   argument that starts with `-` (exit 2).
5. **Wrapper directory edges** (Task 2). `run py` from a sibling `py-x/`, from the repository root,
   and from `py/` itself: refused, refused, accepted. `build <package> <outdir>` with a relative
   outdir from a subdirectory and with `../x` gives the correct absolute path. `wheel-smoke` with a
   Windows layout (`Scripts/python.exe`) finds the interpreter.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `ci/python-env/run.sh` | create | The wrapper. The only file that runs uv, pip or `proto install uv`. |
| `ci/python-env/README.md` | create | The operation table and the rules R-cwd, R-outdir, R-lock, R-build, R-script, style. |
| `ci/python-env/real-tool-check.sh` | create | Runs the cheap operations against the real tool (§5.5). |
| `ci/affected-graph/python_env_ban.py` | create | The gate: scope, tokens, G-ban, G-wrapper, G-floor, G-toolchain, G4, G5, `uv_config_rows`, `PINNED_LINES`, self-test rows. |
| `ci/affected-graph/cargo_moon_parity.py` | modify | Calls the module under `a14`; new negative control in `main()`; old A14 deleted. |
| `ci/affected-graph/ci_targets.py`, `ci/actionlint/run.sh`, `ci/version-lockstep/run.sh` | modify | Coupled pins move in the same diff as the pinned text. |
| `moon.yml` | modify | `repo:affected-smoke` inputs (`**/*`), the real-tool step, migrated tasks. |
| `.moon/tasks/python.yml`, `.moon/tasks/python-project.yml`, `.moon/templates/python/moon.yml`, `py/**/moon.yml`, `contracts/moon.yml`, `ts/packages/paigasus-kernel/moon.yml` | modify | Task commands use the wrapper. |
| `contracts/buf.gen.yaml` | modify | `protoc-plugin` operation (adds `--locked`). |
| `ci/**/*.sh`, `ci/**/*.py` gate files | modify | Wrapper calls, new words, `.venv` fixture literals. |
| `ci/images/release_decision.py` | modify | New stdin subcommand for R-inline, with its self-test row. |
| `.github/workflows/{ci,release,images,images-rehearsal,wheels,prebuild,security-scan}.yml` | modify | Wrapper calls, `shell: bash`, sparse checkout, path filters, new words. |
| `.gitattributes` | modify | `*.sh text eol=lf`. |
| `ci/affected-graph/README.md`, `ci/CLAUDE.md`, `.github/CLAUDE.md`, `contracts/CLAUDE.md`, `rs/CLAUDE.md` | modify | Docs. |
| `rs/crates/libs/paigasus-kernel/README.md` | modify | The macOS 27 / SMA-767 note (Part 2). |
| `rs/Cargo.toml` or `rs/.cargo/config.toml` | modify only if a Part 3 candidate passes | The darwin host-strip setting. |

## Task 1: Measurements and the coupled-pin inventory

**Deliverable:** a committed section `## Appendix A: Inventory` at the end of this plan file. Later
tasks reference its tables by name (A.1 to A.6). The raw Moon baseline stays in
`$SCRATCH/toolchains-before.json`, outside the repository.

**Files:** modify `docs/superpowers/plans/2026-11-14-sma-770-python-env-wrapper-token-ban.md`.

**Interfaces:** Produces tables A.1 (coupled pins), A.2 (R-lock), A.3 (R-script), A.4 (uv flags and
`--`), A.5 (toolchain baseline), A.6 (token hits, classified).

- [ ] **A.1 Coupled pins.** Find every gate that greps for uv, pip or `.venv` text in another file:
  ```bash
  grep -nE '(^|[^[:alnum:]_.-])(uv|pip3?|\.venv)([^[:alnum:]_.-]|$)' \
    ci/affected-graph/ci_targets.py ci/actionlint/run.sh ci/version-lockstep/run.sh \
    ci/actionlint/release_guard.py ci/images/console-selftest.sh
  grep -n '_CALL_SITES\|T_AFFECTED_SMOKE_REQUIRED_INPUTS' ci/affected-graph/ci_targets.py ci/actionlint/run.sh
  ```
  Record one row per pin: (pinning file:line, pinned file:line, pinned text). Known rows:
  `ci_targets.py:896-897` and `:1321` (`PIPE_CAPACITY_INDENTED_CALL_SITES`, the release-plan
  `keys=` line), `ci/version-lockstep/run.sh:618` (the `uv lock` order pin), and
  `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (`ci/actionlint/run.sh:2127`). Each later task that moves a
  pinned line moves its pin in the same commit.
- [ ] **A.2 R-lock.** Add `echo "SMA770_PROBE MOON_PROJECT_ID=${MOON_PROJECT_ID-unset}"` as the
  first line of the `script:` of `repo:input-liveness` in `moon.yml`. Run
  `moon run repo:input-liveness --force`, then read
  `.moon/cache/states/repo/input-liveness/stdout.log`. Expected: `MOON_PROJECT_ID=repo`. Delete
  the inserted line. Check `git diff --quiet moon.yml`. If the value is `unset`, stop and report:
  R-lock needs a new design.
- [ ] **A.3 R-script.** Check stdlib-only:
  ```bash
  python3 -I -c 'import ast,sys; t=ast.parse(open("ci/images/release_decision.py").read());
  m={(n.module or "").split(".")[0] for n in ast.walk(t) if isinstance(n,ast.ImportFrom)}|{a.name.split(".")[0] for n in ast.walk(t) if isinstance(n,ast.Import) for a in n.names};
  print(sorted(m - set(sys.stdlib_module_names)))'
  ```
  Expected: `[]`. Then compare stdout with and without `--isolated`:
  `uv run [--isolated] --no-project --python '>=3.12' python3 ci/images/release_decision.py --self-test`
  into two files in `$SCRATCH`, and `cmp` them. Expected: identical. Then write
  `$SCRATCH/rc.py` with `import sys; sys.exit(int(sys.argv[1]))` and run it for 2, 3 and 4 through
  `uv run --isolated --no-project --python '>=3.12' python3 "$SCRATCH/rc.py" N`. Expected: `$?`
  equals N each time.
- [ ] **A.4 uv flags.** In `py/`, run `uv run --locked -- python -c pass` and
  `uv run --locked -- python -c 'import sys; print(sys.argv[1:])' --locked ''`. Expected: rc 0, and
  the second prints `['--locked', '']`. Run
  `uv build --package paigasus-proto --out-dir "$SCRATCH/dist"` in `py/`. Expected: rc 0 and one
  sdist plus one wheel. Record whether generated sources must exist first.
- [ ] **A.5 Toolchain baseline.**
  ```bash
  moon query tasks > "$SCRATCH/toolchains-before.json"; echo "rc=$?"
  python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1]));
  [print(p+":"+t, sorted(v.get("toolchains") or [])) for p,ts in sorted(d["tasks"].items()) for t,v in sorted(ts.items())]' \
    "$SCRATCH/toolchains-before.json" > "$SCRATCH/toolchains-before.txt"
  ```
  Expected: rc 0. If the JSON shape differs, adapt the reader and record the shape. Copy only the
  rows of the tasks that Tasks 3 to 6 change into A.5.
- [ ] **A.6 Token hits.** Run the gate's scan rule by hand (the same rule as Task 3):
  ```bash
  git ls-files -z | python3 -I "$SCRATCH/scan.py" > "$SCRATCH/hits.txt"
  ```
  `scan.py` uses the token regex and exclusions of the Global Constraints and prints
  `path:line:text`. Record the count by file. Classify each line as **execute** (becomes a wrapper
  call), **reword**, **pin** (with the reason) or **delete** (old A14 code in
  `cargo_moon_parity.py`, removed in Task 7). Assign each file to the task that changes it. The
  count is the measured baseline; the spec's 325 used a smaller token set.
- [ ] **Commit.**
  ```bash
  git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-770 add docs/superpowers/plans/2026-11-14-sma-770-python-env-wrapper-token-ban.md
  git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-770 commit -m "docs(ci): record the SMA-770 inventory and baselines" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
  ```

## Task 2: The wrapper and its behaviour self-test

**Files:** create `ci/python-env/run.sh`, `ci/python-env/README.md`; create the behaviour part of
`ci/affected-graph/python_env_ban.py` (only `wrapper_behaviour_failures` and its table).

**Interfaces:**
- Produces the CLI `bash ci/python-env/run.sh <operation> [args…]` with the operations below.
- Produces `WRAPPER_PATH = "ci/python-env/run.sh"`,
  `WRAPPER_CASES: tuple[WrapperCase, ...]` and
  `wrapper_behaviour_failures(root: Path) -> list[str]` in `python_env_ban.py`.
- `WrapperCase` is a `NamedTuple(name: str, cwd: str, args: tuple[str, ...], env: dict[str, str],
  fake_rc: int, want_rc: int, want_tool: str | None, want_cwd: str | None, want_argv: tuple[str, ...] | None)`.
  `cwd` and `want_cwd` are relative to the fixture root.

Operation and command, one literal `case` branch each (`$ROOT` is the repository root,
`ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"`):

| Operation | Literal command |
|---|---|
| `run py <cmd…>` | `uv run --locked -- "$@"` (no `cd`) |
| `run <ci-project> <cmd…>` | `uv run --locked --project "$ROOT/$project" --python '>=3.12' -- "$@"` |
| `run-frozen py <cmd…>` | `uv run --frozen -- "$@"` |
| `protoc-plugin` | `uv run --locked --project "$ROOT/py" protoc-gen-python_betterproto2` |
| `kernel-test` | `cd "$ROOT/py/packages/paigasus-kernel"` then `uv run --isolated --reinstall-package paigasus-py-bindings pytest tests` |
| `script <file.py> [args…]` | `uv run --isolated --no-project --python '>=3.12' python3 "$file" "$@"` |
| `build` | `uv build` |
| `build <package> <outdir>` | `cd "$ROOT/py"` then `uv build --package "$package" --out-dir "$abs_outdir"` |
| `lock` | `cd "$ROOT/py"` then `uv lock` |
| `cache-dir` | `uv cache dir` |
| `install` | `proto install uv` |
| `probe` | `command -v uv >/dev/null 2>&1` then exit 0, else exit 127 |
| `install-ziglang <python>` | `"$python" -m pip install ziglang` |
| `install-ziglang-system <python>` | `"$python" -m pip install --break-system-packages ziglang` |
| `wheel-smoke <venv-dir> <wheel>` | `"$vpy" -m pip install --no-index --disable-pip-version-check "$wheel"` |
| `sdist-install <venv-dir> <sdist>` | `"$vpip" install "$sdist"` |

`$vpy` is `<venv-dir>/bin/python`, else `<venv-dir>/Scripts/python.exe`. `$vpip` is
`<venv-dir>/bin/pip`, else `<venv-dir>/Scripts/pip.exe`. Neither present: exit 127. Every uv branch
checks `command -v uv` first and exits 127 with `python-env: uv not found` when it is missing.

- [ ] **Step 1: Write the failing test.** In `python_env_ban.py`, write `WRAPPER_CASES` and
  `wrapper_behaviour_failures`. The function copies `ci/python-env/run.sh` into a temp root at the
  same relative path, makes `py/`, `py/packages/paigasus-kernel/`, `py-x/`, `sub/`, the four
  `<ci-project>` dirs and venv layouts `v/bin/python`, `v/bin/pip`, `w/Scripts/python.exe`. It
  writes fakes `uv`, `pip`, `python`, `python3`, `proto` into `fake/bin`. Each fake writes
  `$PWD` and then each argument on its own NUL-terminated record to `$LOG/<tool>`, and exits
  `$FAKE_RC`. It runs `bash <root>/ci/python-env/run.sh` with `PATH=<fake/bin>:/usr/bin:/bin`,
  and with `MOON_PROJECT_ID` removed from the env unless the case sets it. Required cases
  (minimum):
  ```python
  WrapperCase("run py", "py", ("run", "py", "pytest", "-q"), {}, 0, 0, "uv", "py",
              ("run", "--locked", "--", "pytest", "-q")),
  WrapperCase("run py member dir", "py/packages/paigasus-kernel", ("run", "py", "x"), {}, 0, 0,
              "uv", "py/packages/paigasus-kernel", ("run", "--locked", "--", "x")),
  WrapperCase("run py outside py", "sub", ("run", "py", "x"), {}, 0, 2, None, None, None),
  WrapperCase("run py sibling", "py-x", ("run", "py", "x"), {}, 0, 2, None, None, None),   # Review Focus 5
  WrapperCase("run py root", ".", ("run", "py", "x"), {}, 0, 2, None, None, None),          # Review Focus 5
  WrapperCase("run ci", "sub", ("run", "ci/helm-render", "python3", "a.py"), {}, 0, 0, "uv", "sub",
              ("run", "--locked", "--project", "<ROOT>/ci/helm-render", "--python", ">=3.12", "--", "python3", "a.py")),
  WrapperCase("run unknown project", "sub", ("run", "ci/osv", "x"), {}, 0, 2, None, None, None),
  WrapperCase("run-frozen py", "py", ("run-frozen", "py", "x"), {}, 0, 0, "uv", "py", ("run", "--frozen", "--", "x")),
  WrapperCase("protoc-plugin", "sub", ("protoc-plugin",), {}, 0, 0, "uv", "sub",
              ("run", "--locked", "--project", "<ROOT>/py", "protoc-gen-python_betterproto2")),
  WrapperCase("kernel-test", "sub", ("kernel-test",), {}, 0, 0, "uv", "py/packages/paigasus-kernel",
              ("run", "--isolated", "--reinstall-package", "paigasus-py-bindings", "pytest", "tests")),
  WrapperCase("script rc 3", "sub", ("script", "f.py", "a"), {}, 3, 3, "uv", "sub",
              ("run", "--isolated", "--no-project", "--python", ">=3.12", "python3", "f.py", "a")),
  # plus "script rc 2" and "script rc 4" with fake_rc = want_rc
  WrapperCase("script dash file", "sub", ("script", "-c", "x"), {}, 0, 2, None, None, None),  # Review Focus 4
  WrapperCase("build", "py/packages/paigasus-kernel", ("build",), {}, 0, 0, "uv", "py/packages/paigasus-kernel", ("build",)),
  WrapperCase("build package rel outdir", "sub", ("build", "paigasus-proto", "dist"), {}, 0, 0, "uv", "py",
              ("build", "--package", "paigasus-proto", "--out-dir", "<ROOT>/sub/dist")),
  WrapperCase("build package dotdot outdir", "sub", ("build", "paigasus-kernel", "../out"), {}, 0, 0, "uv", "py",
              ("build", "--package", "paigasus-kernel", "--out-dir", "<ROOT>/out")),           # Review Focus 5
  WrapperCase("build unknown package", "sub", ("build", "paigasus-x", "d"), {}, 0, 2, None, None, None),
  WrapperCase("build dash outdir", "sub", ("build", "paigasus-proto", "-o"), {}, 0, 2, None, None, None),
  WrapperCase("lock", "sub", ("lock",), {}, 0, 0, "uv", "py", ("lock",)),
  WrapperCase("lock under moon", "sub", ("lock",), {"MOON_PROJECT_ID": "repo"}, 0, 2, None, None, None),
  WrapperCase("lock under empty moon id", "sub", ("lock",), {"MOON_PROJECT_ID": ""}, 0, 2, None, None, None),
  WrapperCase("cache-dir", "sub", ("cache-dir",), {}, 0, 0, "uv", "sub", ("cache", "dir")),
  WrapperCase("install", "sub", ("install",), {}, 0, 0, "proto", "sub", ("install", "uv")),
  WrapperCase("install-ziglang", "sub", ("install-ziglang", "<FAKE>/python"), {}, 0, 0, "python", "sub",
              ("-m", "pip", "install", "ziglang")),
  WrapperCase("install-ziglang-system", "sub", ("install-ziglang-system", "<FAKE>/python3"), {}, 0, 0, "python3", "sub",
              ("-m", "pip", "install", "--break-system-packages", "ziglang")),
  WrapperCase("wheel-smoke", "sub", ("wheel-smoke", "<ROOT>/v", "x.whl"), {}, 0, 0, "v/bin/python", "sub",
              ("-m", "pip", "install", "--no-index", "--disable-pip-version-check", "x.whl")),
  WrapperCase("wheel-smoke windows", "sub", ("wheel-smoke", "<ROOT>/w", "x.whl"), {}, 0, 0, "w/Scripts/python.exe", "sub",
              ("-m", "pip", "install", "--no-index", "--disable-pip-version-check", "x.whl")),  # Review Focus 5
  WrapperCase("sdist-install", "sub", ("sdist-install", "<ROOT>/v", "p.tar.gz"), {}, 0, 0, "v/bin/pip", "sub",
              ("install", "p.tar.gz")),
  WrapperCase("argv transport", "py", ("run", "py", "p", "", "a b", "--locked"), {}, 0, 0, "uv", "py",
              ("run", "--locked", "--", "p", "", "a b", "--locked")),                           # Review Focus 4
  WrapperCase("unknown operation", "sub", ("sync",), {}, 0, 2, None, None, None),
  WrapperCase("no operation", "sub", (), {}, 0, 2, None, None, None),
  ```
  Two cases run with no fake `uv` on `PATH`: `probe` wants rc 127 and no stdout; `run py x` wants
  rc 127. One case with the fake: `probe` wants rc 0 and empty stdout. Each usage-error case also
  asserts that stderr starts with `python-env:` and that no fake log exists. The fakes for
  `v/bin/python`, `v/bin/pip` and `w/Scripts/python.exe` are written at those paths. The
  `<ROOT>` and `<FAKE>` markers are replaced with the resolved (`pwd -P`) temp paths. A case
  failure is the string `python-env wrapper, <name>: <what differs>`.
- [ ] **Step 2: Run it.** `python3 -I -c 'import sys; sys.path.insert(0,"ci/affected-graph"); import python_env_ban as m, pathlib; f=m.wrapper_behaviour_failures(pathlib.Path(".")); print(len(f)); sys.exit(1 if f else 0)'`.
  Expected: FAIL, because `ci/python-env/run.sh` does not exist.
- [ ] **Step 3: Implement** `ci/python-env/run.sh` per the table. Use `die() { printf 'python-env: %s\n' "$*" >&2; exit 2; }`.
  R-cwd: `case "$(pwd -P)/" in "$ROOT/py/"*) ;; *) die "run py needs a directory inside py/";; esac`.
  R-outdir: refuse `-*`; make the outdir absolute with `mkdir -p` and `cd … && pwd -P` before the
  `cd "$ROOT/py"`. R-lock: `[ -z "${MOON_PROJECT_ID+x}" ] || die "lock refuses to run inside a Moon task"`.
  R-build: no `--no-build-isolation` anywhere. Write `ci/python-env/README.md` with the operation
  table, the fixed lists, the rules and the "how to add an operation" steps (update the sha256 and
  `WRAPPER_CASES` in `python_env_ban.py`).
- [ ] **Step 4: Run it.** Same command under `/bin/bash` as the default bash and again with
  `PATH=/opt/homebrew/bin:$PATH`. Expected: PASS (`0`) both times. Also run
  `shellcheck` through `bash ci/python-env/run.sh run py shellcheck ../ci/python-env/run.sh` from
  `py/`. Expected: no finding.
- [ ] **Step 5: Commit.**
  `git -C <worktree> add ci/python-env ci/affected-graph/python_env_ban.py`
  `git -C <worktree> commit -m "feat(ci): add the ci/python-env wrapper and its behaviour self-test" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"`

## Task 3: The ban module, wired under `a14`, and the floor task

The old A14 code stays in the file until Task 7, but `collect_findings` and `main()` stop calling
it here. During migration a temporary `MIGRATION_FILES` table lists the files that still hold
unmigrated hits. Tasks 4 to 6 empty it. Task 7 deletes it.

**Files:** modify `ci/affected-graph/python_env_ban.py`, `ci/affected-graph/cargo_moon_parity.py`,
`moon.yml` (affected-smoke inputs), `ci/actionlint/run.sh` (`T_AFFECTED_SMOKE_REQUIRED_INPUTS`),
`py/packages/paigasus-kernel/moon.yml` (the floor task).

**Interfaces (Produces, in `python_env_ban.py`):**
- `TOKENS: tuple[tuple[str, re.Pattern[str]], ...]` — the eleven named tokens of the Global
  Constraints, in that order.
- `NON_MATCHES: tuple[str, ...]` — the eight known non-matches.
- `EXCLUSIONS: tuple[tuple[str, Callable[[str], bool]], ...]` — named: `markdown`, `docs`,
  `uv.lock`, `Cargo.lock`, `pnpm-lock.yaml`, `binary`, `dependabot`, `exempt`.
- `EXEMPT_PATHS = ("ci/affected-graph/python_env_ban.py", "ci/python-env/run.sh")`.
- `WRAPPER_SHA256: str` (64 hex chars), `FLOOR_TASK = "paigasus-kernel-py:test"`,
  `FLOOR_TEXT = "ci/python-env/run.sh kernel-test"`, `MOON_TOOLCHAINS = ".moon/toolchains.yml"`.
- `PINNED_LINES: dict[tuple[str, str], str]` — (path, exact stripped line) to reason.
- `MIGRATION_FILES: frozenset[str]` — temporary.
- `token_hits(line: str) -> list[str]` — names of the tokens on the line.
- `binary_paths(root: Path, paths: list[str]) -> frozenset[str]` — `git check-attr -z binary --stdin`.
- `ban_rows(root, tracked, *, read_text, binary, pinned=PINNED_LINES, migration=MIGRATION_FILES) -> tuple[list[str], set[str]]`
  — rows and the set of paths read. `read_text(path) -> str` raises on non-UTF-8.
- `wrapper_rows(root, expected=WRAPPER_SHA256) -> list[str]`, `floor_rows(projects, task=FLOOR_TASK, text=FLOOR_TEXT) -> list[str]`,
  `toolchain_rows(rel_path, text) -> list[str]` (the moved `moon_toolchain_rows`),
  `uv_configs_of(tracked)`, `uv_config_rows(rel_path, text)` (moved unchanged, with `_yaml_code`).
- `check_python_env(projects, root, tracked, *, read_utf8, smoke_unreached, binary=None) -> list[str]`
  — all rows; `read_utf8` and `smoke_unreached` are passed in from `cargo_moon_parity.py`, so the
  module does not import it (no import cycle) and G4 still raises `MoonOutputError`.
- `self_test_failures(read_utf8, smoke_unreached) -> list[str]`.

**Consumes:** `_read_utf8`, `smoke_unreached`, `MoonOutputError`, `AFFECTED_SMOKE_PROJECT`,
`AFFECTED_SMOKE_TASK` from `cargo_moon_parity.py`; `wrapper_behaviour_failures` from Task 2.

- [ ] **Step 1: Write the failing tests** in `self_test_failures`, in the style of the existing
  A14 rows (a temp root, small project dicts like `a14_proj`, one `failures.append(f"…")` per
  missed assertion):
  ```python
  RED = {  # token name -> lines that must be red
      "uv": ["uv sync", "$(uv run x)", "`uv`", "'uv'", '"uv"', "(uv)", "uv;", "uv:", "bin/uv", "[uv]", "a=uv"],
      "uvx": ["uvx ruff"], "pip": ["pip install x"], "pip3": ["pip3 install x"],
      "pip3.<n>": ["pip3.12 install x"], "ensurepip": ["python -m ensurepip"],
      "virtualenv": ["virtualenv v"], ".venv": ["py/.venv/bin/x", ".venv"],
      "UV_*": ["UV_CACHE_DIR=x", "env: { UV_PYTHON: 3 }"], "VIRTUAL_ENV": ["VIRTUAL_ENV=x"],
      "--active": ["x --active"],
  }
  GREEN = list(NON_MATCHES) + ["-uv", "uv-x", "x.uv", "the uv.", "pip.exe", "uvicorn", "a.venv"]
  for name, lines in RED.items():
      for line in lines:
          if name not in token_hits(line):
              failures.append(f"python-env ban: {line!r} should be red for {name}")
  for line in GREEN:
      if token_hits(line):
          failures.append(f"python-env ban: {line!r} should be green, got {token_hits(line)}")
  # guard the guard
  if {n for n, _ in TOKENS} != set(RED):
      failures.append("python-env ban: a token has no red row, or a red row names no token")
  if set(NON_MATCHES) - set(GREEN):
      failures.append("python-env ban: a known non-match has no green row")
  ```
  Fixture-root rows for `ban_rows` (each one a `failures.append` when the result differs):
  - a hit in `ci/x/run.sh` gives one row that names `ci/x/run.sh:2`;
  - a hit in each exclusion (`a/b.md`, `docs/x.sh`, `py/uv.lock`, `rs/Cargo.lock`,
    `ts/pnpm-lock.yaml`, `.github/dependabot.yml`, each of `EXEMPT_PATHS`, and a non-UTF-8 file in
    `binary`) gives no row; a guard row asserts one fixture per `EXCLUSIONS` name;
  - non-UTF-8 file not in `binary` (only `-text`) raises `MoonOutputError` (Review Focus 3);
  - tracked path with a space, `ci/a b.sh`, is scanned and red (Review Focus 3);
  - tracked path missing on disk raises `MoonOutputError` (Review Focus 3);
  - CRLF file `uv sync\r\n` is red; a pinned line in that CRLF file is green; `x\nuv` with no final
    newline is red on line 2 (Review Focus 2);
  - pinned line with reason `""` gives a row with `empty reason`; a pinned line that matches no line
    gives a row with `stale`; a pinned line that matches two lines gives a row with `ambiguous`;
  - a pinned line matches by exact stripped text only: `  uv sync  ` pinned as `uv sync` is green,
    `uv  sync` is red;
  - a `MIGRATION_FILES` path with no hit gives a row with `stale migration entry`.
  Rule rows:
  - `wrapper_rows` with a wrong hash gives one row that contains the real hash; a missing wrapper
    gives a row;
  - `floor_rows`: a project dict whose `paigasus-kernel-py:test` invocation is
    `touch a && bash ../../../ci/python-env/run.sh kernel-test` gives no row; with the text removed
    it gives a row; with the task missing it gives a `FLOOR:` row;
  - `toolchain_rows`: each of `syncArgs:`, `"venvArgs":`, `{installArgs: [x]}` gives a row; a
    comment `# syncArgs:` gives none (keep the existing `moon_toolchain_rows` fixtures);
  - `uv_config_rows`: keep every existing R7 fixture row unchanged;
  - G5: a read file that no smoke glob matches gives a row; `inputGlobs` absent gives a row;
  - `self_test_failures` calls `wrapper_behaviour_failures` and extends with its result.
  In `cargo_moon_parity.self_test()`, add
  `failures.extend(python_env_ban.self_test_failures(_read_utf8, smoke_unreached))`.
- [ ] **Step 2: Run** `python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo rc=$?`.
  Expected: FAIL (`AttributeError` or the new rows), because the functions do not exist.
- [ ] **Step 3: Implement** the module per the interfaces. Line text for a pinned match is
  `line.rstrip("\r").strip()`. G4: `read_text` is `_read_utf8`, except a path in `binary` is
  skipped before it is read. G5: `smoke_unreached(projects, reads)`. Set `WRAPPER_SHA256` to
  `shasum -a 256 ci/python-env/run.sh`. Fill `MIGRATION_FILES` from table A.6: every file with
  an unmigrated hit, except the files that this task migrates. Start `PINNED_LINES` empty; Tasks 4
  to 6 add entries.
  Migrate the floor task in `py/packages/paigasus-kernel/moon.yml:44` to
  `script: 'touch ../../../rs/crates/libs/paigasus-kernel/src/lib.rs ../../../rs/crates/bindings/paigasus-py-bindings/src/lib.rs && bash ../../../ci/python-env/run.sh kernel-test'`,
  and reword that file's comment hits. Give the task `toolchains:` per A.5 if its set changes.
  In `cargo_moon_parity.py`: `import python_env_ban` (the file already runs with its own directory
  on `sys.path`), replace the `("a14", check_uv_env_isolation(…), …)` entry with
  `("a14", python_env_ban.check_python_env(projects, root, tracked, read_utf8=_read_utf8, smoke_unreached=smoke_unreached), <new fix text>)`.
  Replace the A14 negative control in `main()`: deep-copy `projects`, remove `FLOOR_TEXT` from the
  floor task's invocation, call `python_env_ban.floor_rows(mutated)`, and return 2 with
  `FATAL [parity] A14 negative control: …` unless a row names `paigasus-kernel-py:test`.
  In `moon.yml` `repo:affected-smoke`, add `'**/*'` to `inputs` (keep the old entries: they are
  floored by `T_AFFECTED_SMOKE_REQUIRED_INPUTS`). Add `'**/*'` to `T_AFFECTED_SMOKE_REQUIRED_INPUTS`
  in `ci/actionlint/run.sh` in the same diff (coupled pin, A.1).
- [ ] **Step 4: Run.**
  `python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo rc=$?` — expected rc 0.
  `python3 ci/affected-graph/cargo_moon_parity.py; echo rc=$?` — expected rc 0 and no `a14` row.
  `python3 ci/affected-graph/task_inputs.py; echo rc=$?` — expected rc 0.
  `/bin/bash ci/affected-graph/run.sh --negative-control` — expected rc 0.
  Compare the floor task's toolchains with `$SCRATCH/toolchains-before.txt`. Expected: same set.
- [ ] **Step 5: Commit** `feat(ci): replace the A14 parser call with the python-env token ban`
  (with the Co-Authored-By line).

## Task 4: Migrate batch 1 — Moon tasks, templates, `buf.gen.yaml`, `py/`, `rs/`, `ts/`, root config

**Files:** `.moon/tasks/python.yml`, `.moon/tasks/python-project.yml`, `.moon/tasks/rust.yml`,
`.moon/templates/python/moon.yml`, `.moon/toolchains.yml`, `.moon/workspace.yml`, `moon.yml` (task
commands and comments only), `py/moon.yml`, `py/packages/paigasus-gateway-conformance/moon.yml`,
`py/packages/paigasus-kernel/{pyproject.toml,tests/test_stub_surface.py}`, `py/pyproject.toml`,
`py/scripts/*`, `contracts/moon.yml`, `contracts/buf.gen.yaml`, `ts/packages/paigasus-kernel/moon.yml`,
`rs/crates/bindings/paigasus-py-bindings/{Cargo.toml,src/lib.rs}`,
`rs/crates/libs/paigasus-gateway-core/tests/fixtures/make_fixtures.py`, `.prototools`, `.gitignore`,
`osv-scanner.toml`, `.github/dependabot.yml` (comments only, file stays excluded),
`ci/affected-graph/python_env_ban.py` (`MIGRATION_FILES`, `PINNED_LINES`).

**Interfaces:** Consumes the wrapper operations `run py`, `build`, `protoc-plugin`.

- [ ] **Step 1: Failing check.** Remove this batch's paths from `MIGRATION_FILES`. Run
  `python3 ci/affected-graph/cargo_moon_parity.py 2>&1 | grep -c 'python-env\|a14'`. Expected: FAIL
  with one row per A.6 hit in these files.
- [ ] **Step 2: Migrate.** Per A.6: an `execute` line becomes `bash <rel>/ci/python-env/run.sh <op> …`
  (for example `.moon/tasks/python-project.yml:26` becomes the `build` operation, and
  `contracts/buf.gen.yaml:48` becomes `['bash', '../ci/python-env/run.sh', 'protoc-plugin']`). A
  `reword` line gets new words ("the Python package manager", "the project environment", or the
  wrapper name). A `pin` line gets a `PINNED_LINES` entry with its reason: expected here are
  `.prototools:31`, `.moon/toolchains.yml` (`packageManager: 'uv'`, `unstable_uv:`), `.gitignore:32`,
  `.moon/workspace.yml:64`, `py/pyproject.toml:29`. A `.venv` fixture gets another literal that tests
  the same behaviour, or a pin.
- [ ] **Step 3: Toolchain check.** Re-run A.5 into `$SCRATCH/toolchains-after-4.txt` and
  `diff "$SCRATCH/toolchains-before.txt" "$SCRATCH/toolchains-after-4.txt"`. Expected: no change.
  Where a set changes, add an explicit `toolchains:` entry to the task and re-run until no diff.
- [ ] **Step 4: Run.** `python3 ci/affected-graph/cargo_moon_parity.py; echo rc=$?` — expected rc 0.
  `moon run contracts:generate` then `git status --porcelain contracts py rs ts` — expected empty
  (no codegen drift). `moon run paigasus-kernel-py:test` — expected pass.
- [ ] **Step 5: Commit** `refactor(ci): route Moon tasks and buf through the python-env wrapper`.

## Task 5: Migrate batch 2 — `ci/` gate scripts and their coupled pins

**Files:** every `ci/**` file in A.6 except `ci/affected-graph/cargo_moon_parity.py` (Task 7),
`ci/images/release_decision.py` (Task 6) and the exempt files. Includes `ci/actionlint/run.sh`,
`ci/actionlint/release_guard.py`, `ci/ruff/run.sh`, `ci/release-plan/{run.sh,release_plan.py,pyproject.toml}`,
`ci/workflow-credentials/*`, `ci/wasm-lockstep/*`, `ci/helm-render/*`, `ci/version-lockstep/run.sh`,
`ci/release-parity/**`, `ci/osv/*`, `ci/publish-metadata/run.sh`, `ci/images/{run.sh,console-selftest.sh,chains.toml}`,
`ci/affected-graph/{ci_targets.py,run.sh}`.

**Interfaces:** Consumes `run py`, `run <ci-project>`, `run-frozen py`, `script`, `lock`, `probe`.
The coupled pins of A.1 change in the same commit as their pinned lines.

- [ ] **Step 1: Failing check.** Remove these paths from `MIGRATION_FILES`. Run the parity gate.
  Expected: FAIL with one row per hit in these files.
- [ ] **Step 2: Migrate.** Expected executes: the ruff and shellcheck resolvers (`run py`), the
  pipe-capacity probe (`run py`), the release-plan `keys=` line and the four ci-project calls
  (`run <ci-project>`), `python-semantic-release.sh:48` (`run-frozen py`), `ci/images/run.sh:43`
  (`script`), the `uv lock` in `ci/version-lockstep/run.sh --write` (`lock`), the `command -v uv`
  probes (`probe`). Keep the `[ -x ]` and `tail -n1` guards of the two binary resolvers. Expected
  pins: `release_guard.py:214` and its rows, the fake in `console-selftest.sh`, and only the coupled
  pins that must hold the literal to detect it. Update `PIPE_CAPACITY_INDENTED_CALL_SITES`,
  `ci_targets.py:1321`, `ci/version-lockstep/run.sh:618` and every other A.1 pin to the new text.
- [ ] **Step 3: Run each gate under its own bash.**
  `/bin/bash ci/affected-graph/run.sh` — rc 0.
  `/opt/homebrew/bin/bash ci/ruff/run.sh` — rc 0.
  `/opt/homebrew/bin/bash ci/actionlint/run.sh` — rc 0 when the preflight reports 65536 bytes; on
  a 512-byte host run it in Docker per the memory note `small-pipe-linux-container-workaround`.
  `bash ci/release-plan/run.sh --self-test`, `bash ci/workflow-credentials/run.sh --negative-control`,
  `bash ci/wasm-lockstep/run.sh`, `bash ci/helm-render/run.sh`, `bash ci/release-parity/run.sh`
  with each ecosystem, `bash ci/version-lockstep/run.sh` and `--negative-control` — rc 0 each.
  Then `python3 ci/affected-graph/cargo_moon_parity.py` — rc 0, no `a14` row.
- [ ] **Step 4: Toolchain check** as in Task 4 Step 3, for the `repo:*` tasks of these gates.
- [ ] **Step 5: Commit** `refactor(ci): route the ci gate scripts through the python-env wrapper`.

## Task 6: Migrate batch 3 — workflows and R-inline

**Files:** `.github/workflows/{ci,release,images,images-rehearsal,wheels,prebuild,security-scan}.yml`,
`ci/images/release_decision.py`, `python_env_ban.py` (`MIGRATION_FILES`, `PINNED_LINES`).

**Interfaces:** Produces `release_decision.py revision` — reads one image config JSON on stdin and
prints `config.Labels["org.opencontainers.image.revision"]`; exit 2 on bad JSON or a missing label.
Consumes `script`, `build <package> <outdir>`, `install`, `cache-dir`, `install-ziglang`,
`install-ziglang-system`, `wheel-smoke`, `sdist-install`.

- [ ] **Step 1: Failing test.** Add self-test rows to `release_decision.py` in its existing style:
  `revision` with `{"config":{"Labels":{"org.opencontainers.image.revision":"abc"}}}` prints `abc`
  and returns 0; with `{"config":{"Labels":{}}}` returns 2; with `not json` returns 2.
  Run `python3 ci/images/release_decision.py --self-test; echo rc=$?`. Expected: FAIL (rc 3).
- [ ] **Step 2: Implement** the `revision` subparser. Then in `release.yml:1569` replace the inline
  `python3 -c` with `crane config "$1" | bash ci/python-env/run.sh script ci/images/release_decision.py revision`.
  Run the self-test again. Expected: rc 0.
- [ ] **Step 3: Migrate the workflows.** Remove the seven paths from `MIGRATION_FILES`. 14
  `release_decision.py` lines use `script`. `release.yml:544` uses
  `bash ci/python-env/run.sh build paigasus-proto dist` from the repository root (R-outdir keeps
  `dist/` at the root). `wheels.yml:716` uses `build paigasus-kernel <its outdir>`. The five
  install lines use `install`. `ci.yml:154` uses `cache-dir`. `wheels.yml:188` uses
  `install-ziglang python` and gets `shell: bash`. `prebuild.yml:151` uses
  `install-ziglang-system python3` (check its `working-directory: rs` and give the path
  `../ci/python-env/run.sh`). `wheels.yml:437` uses `wheel-smoke`. `wheels.yml:659` uses
  `sdist-install`. In `sdist-verify`, add before the sdist download:
  ```yaml
  - uses: actions/checkout@<the pinned sha already used in wheels.yml>
    with:
      sparse-checkout: ci/python-env
      sparse-checkout-cone-mode: false
      persist-credentials: false
  ```
  Reword the job comment at `wheels.yml:623` so it stays true: the checkout holds only
  `ci/python-env/`, so no `rust-toolchain.toml` exists, `rustup default` stays authoritative, and
  the package manager builds in a temporary directory. Add `ci/python-env/**` to
  `pull_request.paths` of `wheels.yml`, `prebuild.yml` and `images.yml` (block sequence form). Step
  names `Install uv` become `Install the Python package manager`.
- [ ] **Step 4: Run.** `python3 ci/affected-graph/cargo_moon_parity.py` — rc 0, no `a14` row.
  `/opt/homebrew/bin/bash ci/actionlint/run.sh` (pipe rule as in Task 5) — rc 0, which also runs
  `repo:workflow-credentials`-style checks and shellcheck over the new `run:` lines.
  `bash ci/workflow-credentials/run.sh` — rc 0 (the sparse checkout adds no credential).
- [ ] **Step 5: Commit** `refactor(ci): route workflow steps through the python-env wrapper`.

## Task 7: Delete the A14 parser and turn the full ban on

**Files:** `ci/affected-graph/cargo_moon_parity.py`, `ci/affected-graph/python_env_ban.py`.

**Interfaces:** Removes `UV_*` regex constants (`UV_TOKEN_RE` … `UV_DIRECT_WRITE_RE`), `_uv_*`
helpers, `uv_form_hits`, `uv_mutations`, `_floor_isolated`, `UV_SOURCE_STMT_RE`, `UV_RUN_*`,
`_uv_run_refs`, `_uv_script_closure`, `LOCKSTEP_*`, `_split_commands`, `_expand_words`,
`runs_lockstep_write` (keep `check_version_lockstep_no_write`: an A8 check, not A14),
`REQUIRED_UV_ISOLATED_TASKS`, `ALLOW_UV_ENV_SCRIPT`, `check_uv_env_isolation`, the old
`uv_configs_of`/`uv_config_rows`/`moon_toolchain_rows`/`MOON_UV_ARGS_RE` copies (now in the module),
the A14 self-test block (about lines 7460-7760), and `MIGRATION_FILES` with its stale-row code and
its self-test row.

- [ ] **Step 1: Failing check.** Delete `MIGRATION_FILES` from the module and its use in
  `ban_rows`. Run the parity gate. Expected: FAIL with rows for every hit in the old A14 code of
  `cargo_moon_parity.py`.
- [ ] **Step 2: Delete** the code listed above. Before you delete a name, run
  `grep -n '<name>' ci/affected-graph/*.py ci/**/*.sh` and keep any name that another check uses.
  Reword the remaining hits in `cargo_moon_parity.py` (A1 to A13 comments) or pin them with a
  reason.
- [ ] **Step 3: Run.**
  `python3 ci/affected-graph/cargo_moon_parity.py --self-test` — rc 0.
  `python3 ci/affected-graph/cargo_moon_parity.py` — rc 0.
  `/bin/bash ci/affected-graph/run.sh --negative-control && /bin/bash ci/affected-graph/run.sh` — rc 0.
  `/opt/homebrew/bin/bash ci/ruff/run.sh` — rc 0 (no unused import or name).
  `python3 -c 'import sys; sys.path.insert(0,"ci/affected-graph"); import python_env_ban as m; print(len(m.PINNED_LINES))'`
  — expected a number below 35. Record it and the reasons for the PR.
- [ ] **Step 4: Commit** `refactor(ci): delete the A14 shell parser and its waivers`.

## Task 8: The real-tool check and `eol=lf`

**Files:** create `ci/python-env/real-tool-check.sh`; modify `moon.yml` (`repo:affected-smoke`
script), `.gitattributes`, `python_env_ban.py` (no hash change: the check script is not the wrapper).

**Interfaces:** `bash ci/python-env/real-tool-check.sh` runs, in order: `probe`, `cache-dir` (stdout
is one absolute path), `run py python -c pass` from `py/`, `script "$tmp/ok.py" 3` where `ok.py`
exits with its argument (want rc 3), and `build paigasus-proto "$tmp/dist"` (want one `.tar.gz`
and one `.whl` in `$tmp/dist`). It exits 1 on a wrong result and 2 on a usage error. It holds no
banned token. Decision: a step in `repo:affected-smoke` (no new `repo:*` gate, so none of the
seven registration obligations).

- [ ] **Step 1: Failing test.** Add `bash ci/python-env/real-tool-check.sh` as the last line of the
  `repo:affected-smoke` `script:`. Run `moon run repo:affected-smoke --force`. Expected: FAIL
  (`No such file`).
- [ ] **Step 2: Implement** the script (SPDX, `set -euo pipefail`, `export PROTO_REPORTER=text`,
  `trap 'rm -rf "$tmp"' EXIT`). Check whether `repo:affected-smoke`'s `toolchain: 'system'` puts
  the package manager on `PATH` in CI; if A.5 shows it does not, give the task the Python toolchain
  explicitly and record the toolchain diff.
- [ ] **Step 3: Run** `moon run repo:affected-smoke --force` — rc 0.
- [ ] **Step 4: `eol=lf`.** Add `*.sh text eol=lf` to `.gitattributes`. Then:
  ```bash
  git add --renormalize --dry-run . | grep '\.sh$' ; echo "count=$(git add --renormalize --dry-run . | grep -c '\.sh$')"
  git ls-files --eol -- '*.sh' | grep -v 'i/lf' || echo "all lf"
  ```
  Expected: `count=0` and `all lf`. Any hit: stop and report the file.
- [ ] **Step 5: Commit** `feat(ci): run the python-env real-tool check on every PR`.

## Task 9: Docs

**Files:** `ci/affected-graph/README.md` (A14 section, lines 378-530), `ci/CLAUDE.md`,
`.github/CLAUDE.md`, `contracts/CLAUDE.md`.

- [ ] **Step 1:** Replace the A14 section with the new gate: scope, exclusions, tokens, boundary,
  known non-matches (with the `uv.` false green), G rules, `PINNED_LINES`, the self-test classes,
  and one history line: the parser went away after R1, R2, R10 and the `cd`/`source` false red.
- [ ] **Step 2:** `ci/CLAUDE.md`, one standing rule: every uv, pip and `.venv` operation goes
  through `ci/python-env/run.sh`; to add one, add a wrapper operation, update `WRAPPER_SHA256` and
  `WRAPPER_CASES` in `python_env_ban.py`. Reword the existing uv mentions there only where the
  facts change (docs are out of the ban's scope).
- [ ] **Step 3:** One line each in `.github/CLAUDE.md` (workflow steps are in the ban's scope; use
  the wrapper; `sdist-verify` has a sparse checkout of `ci/python-env/` only) and
  `contracts/CLAUDE.md` (`buf.gen.yaml` is in scope and uses `protoc-plugin`).
- [ ] **Step 4: Run** `/opt/homebrew/bin/bash ci/actionlint/run.sh` (check 12 markers; pipe rule as
  in Task 5) and `python3 ci/affected-graph/ci_targets.py` — rc 0 each. Check that the root
  `CLAUDE.md` marker blocks are unchanged: `git diff --quiet main -- CLAUDE.md`.
- [ ] **Step 5: Commit** `docs(ci): document the python-env wrapper and the token ban`.

## Task 10: Mutation battery

**Files:** none committed. Record results in `$SCRATCH/mutations.txt` for the PR body.

Each mutation leaves the file runnable. For each: apply, run, record rc and the first failing row,
then remove the inserted text. After any fix to the gate, run the whole battery again.

| # | Mutation (in `python_env_ban.py` unless named) | Run | Expected |
|---|---|---|---|
| M1..M11 | remove one entry from `TOKENS` (one run per token) | `--self-test` | rc 1, `has no red row` |
| M12 | widen the boundary to `(?<![\w])` | `--self-test` | rc 1, `-uv` should be green |
| M13 | delete the `"uv"` key from `RED` | `--self-test` | rc 1, `has no red row` |
| M14 | remove one `EXCLUSIONS` entry | `--self-test` | rc 1 |
| M15 | skip the empty-reason check | `--self-test` | rc 1, `empty reason` |
| M16 | skip the stale check | `--self-test` | rc 1, `stale` |
| M17 | skip the ambiguous check | `--self-test` | rc 1, `ambiguous` |
| M18 | `wrapper_rows` returns `[]` | `--self-test` | rc 1 |
| M19 | `floor_rows` returns `[]` | `--self-test`, then real `main()` | rc 1, then rc 2 `negative control` |
| M20 | `toolchain_rows` returns `[]` | `--self-test` | rc 1 |
| M21 | G5 call removed | `--self-test` | rc 1 |
| M22 | G4: catch the decode error and skip | `--self-test` | rc 1 |
| M23 | `run.sh`: add `--no-build-isolation` to `build` | `--self-test` | rc 1 (behaviour row and hash row) |
| M24 | `run.sh`: delete the R-lock line | `--self-test` | rc 1, `lock under moon` |
| M25 | `run.sh`: delete the R-cwd check | `--self-test` | rc 1, `run py outside py` |
| M26 | `run.sh`: delete `--` in `run py` | `--self-test` | rc 1, `argv transport` |
| M27 | `run.sh`: map the program rc to 1 | `--self-test` | rc 1, `script rc 3` |
| M28 | `run.sh`: drop `--isolated` from `script` | `--self-test` | rc 1 |
| M29 | `run.sh`: relative outdir kept relative | `--self-test` | rc 1, `rel outdir` |
| M30 | revert the floor task to the old command | real `cargo_moon_parity.py` | rc 1, `paigasus-kernel-py:test` |
| M31 | add `uv sync` to `ci/osv/run.sh` | real `cargo_moon_parity.py` | rc 1, row names the line |
| M32 | add `syncArgs: ['--reinstall']` to `.moon/toolchains.yml` | real | rc 1 |

`--self-test` means `python3 ci/affected-graph/cargo_moon_parity.py --self-test`. Mutations of
`run.sh` change its hash, so the hash row also fires; the row named in the table must fire too.

- [ ] **Step 1:** Run every row. Expected: each matches the table. A row that stays green is a gap:
  add the missing test to Task 3 or Task 2, commit as `test(ci): …`, then run the whole battery
  again.
- [ ] **Step 2:** `git -C <worktree> status --porcelain` — expected empty after the battery.

## Task 11: Part 2 — kernel README note and the commit-body check

**Files:** `rs/crates/libs/paigasus-kernel/README.md`.

- [ ] **Step 1:** Add a short section: the macOS wheels of 0.2.0 have `stroff mod 8 = 4`
  (SMA-767), 0.2.1 is built with the fix, and macOS 27 rejects the old layout. No other file under
  `rs/crates/libs/paigasus-kernel/` changes: `git diff --stat main -- rs/crates/libs/paigasus-kernel/`
  shows one file.
- [ ] **Step 2: Check** that `README.md` is in the crate's `include`:
  `grep -n 'README.md' rs/crates/libs/paigasus-kernel/Cargo.toml` — line 20. Run
  `( cd rs && cargo package --list --locked -p paigasus-kernel | grep README.md )` — one line.
- [ ] **Step 3: Commit** `fix(rs): note the SMA-767 macOS fix in the kernel README`.
- [ ] **Step 4: Commit-body check** (run again just before the PR opens):
  ```bash
  git log --format='%H%n%B%n--END--' main..HEAD | grep -nE '^(BREAKING[ -]CHANGE)' && echo FAIL || echo ok
  git log --format='%s' main..HEAD | grep -nE '^[a-z]+(\([^)]*\))?!:' && echo FAIL || echo ok
  ```
  Expected: `ok` twice. Record that the PR title must start with `fix(rs):`.

## Task 12: Part 3 — darwin host-strip measurements

**Files:** `rs/CLAUDE.md`; `rs/Cargo.toml` or `rs/.cargo/config.toml`, and `wheels.yml`,
`prebuild.yml`, `ts/packages/paigasus-kernel/moon.yml`, only if a candidate passes.

Docker is required for the Linux half. Do not replace it with a host install. If Docker is not
running, stop and ask the coordinator.

- [ ] **Step 1: darwin count, baseline and per candidate.** For base, C1 (`[profile.release.build-override] debug = "line-tables-only"`
  in `rs/Cargo.toml`) and C3 (`[env] CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_STRIP = "none"` in
  `rs/.cargo/config.toml`), each with a fresh target dir:
  ```bash
  T="$(mktemp -d "$SCRATCH/tgt.XXXX")"
  ( cd rs && CARGO_TARGET_DIR="$T" cargo build --release -v --locked --workspace --target aarch64-apple-darwin ) \
    > "$SCRATCH/<cand>.log" 2>&1; echo rc=$?
  grep 'Running `' "$SCRATCH/<cand>.log" | grep 'rustc ' > "$SCRATCH/<cand>.rustc"
  echo "rustc=$(wc -l < "$SCRATCH/<cand>.rustc") host=$(grep -vc -- '--target ' "$SCRATCH/<cand>.rustc") stripped=$(grep -v -- '--target ' "$SCRATCH/<cand>.rustc" | grep -c 'strip=debuginfo')"
  ```
  Expected: `rustc` above 0 (else the build was cached: a false pass). Record `host` and
  `stripped`. Record `stroff mod 8` for the host binaries with
  `otool -l <bin> | grep -A4 LC_SYMTAB`. Base expected: `stripped` above 0.
- [ ] **Step 2: Linux A/A control.** Two clean builds in Docker with no change:
  ```bash
  docker run --rm -v "$PWD/rs:/src:ro" -v "$SCRATCH/aa1:/out" rust:1.95.0-bookworm \
    bash -c 'cp -r /src /w && cd /w && CARGO_TARGET_DIR=/out/target cargo build --release --locked --workspace'
  ```
  and the same into `aa2`. List the shipped binaries from
  `cargo metadata --format-version 1 --no-deps` (targets of kind `bin` and `cdylib`: the py-bindings
  `.so`, the node-bindings library, the service binaries), record the paths, and `cmp` each pair.
  Expected: all identical. If not, stop: the byte rule cannot decide, and Part 3 records that.
- [ ] **Step 3: Linux per candidate.** Build C1 and C3 the same way. `cmp` each shipped binary
  against `aa1`, and record `du -sm` of each target dir. A candidate passes only if darwin
  `stripped=0` with every host `stroff mod 8 = 0`, Linux shipped binaries are identical, and the
  target dir grows by less than 50 MB.
- [ ] **Step 4: Record C2, C4, C5.** C2 is measured only if C1 passes. C4 is the SMA-767 result (no
  effect on host units). C5 is rejected in writing (no per-target `rustc-wrapper`; Windows legs;
  `RUSTC_WRAPPER` overrides it; it runs on every compile).
- [ ] **Step 5: Result.** If no candidate passes, add the measured table (base, C1, C3 numbers; C2,
  C4, C5 reasons) under the SMA-767 R1 entry in `rs/CLAUDE.md` and keep the
  `CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_STRIP` rule. If one passes, add it, and remove the darwin
  env steps in `wheels.yml` and `prebuild.yml` and the `uname` exports in
  `ts/packages/paigasus-kernel/moon.yml` in the same diff. Remove every candidate edit that did not
  pass: `git diff --stat` shows only the intended files.
- [ ] **Step 6: Commit** `docs(rs): record the SMA-770 darwin host-strip measurements` (or
  `fix(rs): …` if a candidate goes in).

## Task 13: Final verification

- [ ] **Step 1: Full graph under system bash.** Put a bash-only shim dir first on `PATH` (memory
  note `affected-smoke-hang-fixed-by-system-bash`), then run the command between the
  `ci-targets` markers of the root `CLAUDE.md`, exactly as written there, with
  `--base origin/main --include-relations`. Record the result per target.
- [ ] **Step 2: Re-run the bash-4+ gates directly** and use these results in place of the `moon ci`
  verdict for them:
  `/opt/homebrew/bin/bash ci/ruff/run.sh`, `/opt/homebrew/bin/bash ci/next-public/run.sh`,
  `/opt/homebrew/bin/bash ci/publish-metadata/run.sh`,
  `/opt/homebrew/bin/bash ops/nats/check-subjects.sh` and then
  `( cd rs && cargo nextest run --locked --no-tests=pass -p paigasus-iam --test nats_permissions --test docker_preflight --profile iam-nats )`.
- [ ] **Step 3: `repo:actionlint`.** `/opt/homebrew/bin/bash ci/actionlint/run.sh`. Read the
  preflight line first. With 65536 bytes, its verdict counts. With 512 bytes, run it in a Linux
  container, or record "no local verdict" and rely on CI.
- [ ] **Step 4:** `repo:affected-smoke` under `/bin/bash` 3.2 (it ran in Step 1). Confirm the
  real-tool step ran: its output is in `.moon/cache/states/repo/affected-smoke/stdout.log`.
- [ ] **Step 5:** Re-run the Task 11 Step 4 commit-body check. Record for the PR: the pinned-line
  count and reasons, the toolchain diffs (empty, or the added `toolchains:` entries), the mutation
  battery table, and the Part 3 table.

## Appendix A: Inventory

Measured on 2026-11-14 by Task 1 on branch `feature/sma-770-close-the-sma-765767768-residuals-for-good`
(base `de928aa5`). Moon 2.5.3, uv 0.11.16 (proto shim). `$SCRATCH` was
`/var/folders/dc/3qfs44nn0574bmkgl_rk6_bw0000gn/T/sma-770`. The raw Moon baseline is
`$SCRATCH/toolchains-before.json` (outside the repository). `moon.yml` was edited for A.2 and
restored: `git status --short` was empty after Task 1 step A.2.

### A.1 Coupled pins

A coupled pin is a gate that holds the text of a line in another file. When a task moves the pinned
line, the same commit moves its pin. The grep of the brief found rows 1 to 5. Row 6 and row 7 were
found by reading `cargo_moon_parity.py` and `ci/actionlint/run.sh`; no `uv` token is in their pin text.

| # | Pinning file:line | Pinned file:line | Pinned text (today) | Moves in |
|---|---|---|---|---|
| 1 | `ci/affected-graph/ci_targets.py:896` (`PIPE_CAPACITY_INDENTED_CALL_SITES`, stripped whole line) | `ci/actionlint/run.sh:5310` | `pc_out="$(uv run --locked --project py python3 -c "$PIPE_CAPACITY_PROBE_PY" \| tail -n1)" \|\| pc_rc=$?` | Task 5 |
| 2 | `ci/affected-graph/ci_targets.py:897` (same tuple) | `ci/actionlint/run.sh:5311` | `[ "$pc_rc" -eq 0 ] \|\| infra "the pipe-capacity probe failed (rc $pc_rc) via 'uv run --locked --project py'. No check ran."` | Task 5 |
| 3 | `ci/affected-graph/ci_targets.py:1321` (`RELEASE_PLAN_SH_CALL_SITES`, the `keys=` line) | `ci/release-plan/run.sh:121` | `keys="$(uv run --locked --project "$HERE" --python '>=3.12' python3 "$HERE/release_plan.py" --keys "$REPO_ROOT")" \|\| keys_rc=$?` | Task 5 |
| 4 | `ci/affected-graph/ci_targets.py:1337` (same tuple) | `ci/release-plan/run.sh:408` | `_expect_failsafe_outputs "row 6 (uv unreachable)" "$nouv_out"` | Task 5 |
| 5 | `ci/version-lockstep/run.sh:618` (self-test `grep -Fn`) | `ci/version-lockstep/run.sh:1635` | `( cd "$REPO_ROOT/py" && uv lock` (order pin: the static metadata check must come first) | Task 5 |
| 6 | `ci/affected-graph/cargo_moon_parity.py:145` (`FFI_MARKERS` has `"--reinstall-package"`) and `:147-151` (`REQUIRED_FFI_TASKS` floor); fixtures at `:5492` and `:8124` | `py/packages/paigasus-kernel/moon.yml:44` (script ends `uv run --isolated --reinstall-package paigasus-py-bindings pytest tests`) | A5 derives its FFI task set from this marker text in the resolved task command. When Task 4 moves the script to `ci/python-env/run.sh kernel-test`, the marker is gone and the derived set loses `paigasus-kernel-py:test`. `REQUIRED_FFI_TASKS` then reds with a floor row. Task 4 must add the marker `ci/python-env/run.sh kernel-test` to `FFI_MARKERS` in the same commit and update the two fixtures. | Task 4 (and Task 7 when the file is edited) |
| 7 | `ci/actionlint/run.sh:2127` (`T_AFFECTED_SMOKE_REQUIRED_INPUTS`, 39 entries now; floor `-ge 20` at `:5806`, copied at `ci_targets.py:974` and `:2940`) | `moon.yml` `repo:affected-smoke` `inputs:` | An input list, not a text pin. Task 3 adds the `ci/python-env/**` input (and `**/*` minus the excluded paths if the design needs it) to `repo:affected-smoke`. Both tuples must list the new entry in the same commit. The floor of 20 stays below the count. | Task 3 |
| 8 | `ci/affected-graph/ci_targets.py:1319` (`"negctl)   require_uv; negative_control ;;"`) and `:1287`, `:3556-3566` (the C1 trap mutation) | `ci/release-plan/run.sh` function `require_uv` (`:32-42`) and its `negctl)` arm | The function NAME `require_uv` contains `uv` but is not a token match (the `_` is a word character). Keep the name, or change the pin and the mutation text together. | Task 5 |

No other file in `ci/` greps for the text of an `uv`, `pip` or `.venv` line. The brief's grep over
`ci/affected-graph/ci_targets.py`, `ci/actionlint/run.sh`, `ci/version-lockstep/run.sh`,
`ci/actionlint/release_guard.py` and `ci/images/console-selftest.sh` returned only rows 1 to 5, the
`release_guard.py` publish detector (a pin, not a coupled pin: see A.6) and the fake binary in
`console-selftest.sh` (a pin, see A.6).

### A.2 R-lock (Moon sets `MOON_PROJECT_ID` for a `script:` task)

Inserted `echo "SMA770_PROBE MOON_PROJECT_ID=${MOON_PROJECT_ID-unset}"` as the first line of the
`script:` of `repo:input-liveness`, ran `moon run repo:input-liveness --force`, and read
`.moon/cache/states/repo/input-liveness/stdout.log`.

Result: first line `SMA770_PROBE MOON_PROJECT_ID=repo`. R-lock holds: `$MOON_PROJECT_ID` is `repo` for a
root `repo:*` script task. The inserted line was deleted; `git status --short` was empty.

### A.3 R-script (a script run with `--isolated --no-project --python '>=3.12'`)

| Check | Command | Result |
|---|---|---|
| Stdlib only | `python3 -I -c '<ast walk over ci/images/release_decision.py>'` | `[]` (no third-party import) |
| `--isolated` changes nothing | `uv run [--isolated] --no-project --python '>=3.12' python3 ci/images/release_decision.py --self-test`, stdout to two files | both rc 0, 39 bytes each, empty stderr, `cmp` rc 0 (identical) |
| Exit code passes through | `uv run --isolated --no-project --python '>=3.12' python3 $SCRATCH/rc.py N` | N=2 gives rc 2, N=3 gives rc 3, N=4 gives rc 4 |

### A.4 uv flags and `--` (measured in a `git archive HEAD` copy, ruling F5)

The copy was `git archive HEAD` extracted to `$SCRATCH/arch`, outside the repository. So the tree is
clean: no `py/.venv`, no generated files that are not tracked, no `target/`.

| Command (in `$SCRATCH/arch/py`) | Result |
|---|---|
| `uv run --locked -- python -c pass` | rc 0 (it built the four workspace members and installed 67 packages) |
| `uv run --locked -- python -c 'import sys; print(sys.argv[1:])' --locked ''` | rc 0, printed `['--locked', '']` (the `--` ends uv options; later flags and the empty argument reach the program unchanged) |
| `uv build --package paigasus-proto --out-dir "$SCRATCH/dist"` | rc 0; exactly `paigasus_proto-0.7.0.tar.gz` and `paigasus_proto-0.7.0-py3-none-any.whl` (32 files in the wheel) |

Generated sources do NOT need to exist first for `paigasus-proto`: the build worked from the clean
archive, because the generated protobuf modules are tracked in git (15 tracked files under
`py/packages/paigasus-proto/src`). (The same copy built
`paigasus-py-bindings` from `rs/crates/bindings/paigasus-py-bindings` during `uv run --locked`.)
Note for Task 2: the first `uv run --locked` in a clean tree is slow because it builds the bindings.

### A.5 Toolchain baseline

`moon query tasks > $SCRATCH/toolchains-before.json` gave rc 0. The JSON shape is
`{"tasks": {"<project>": {"<task>": {..., "toolchains": [...]}}}}`; the brief's reader works
unchanged (171 task rows in `$SCRATCH/toolchains-before.txt`). Tasks 4 to 6 must leave these rows
unchanged (or add an explicit `toolchains:` entry). Rows for the tasks that Tasks 4, 5 and 6 change:

| Task row | Toolchains | Changed by |
|---|---|---|
| `py:fmt`, `py:lint`, `py:test`, `py:typecheck` | `unstable_python`, `unstable_uv` | Task 4 (`.moon/tasks/python.yml`) |
| `paigasus-kernel-py:build`, `paigasus-ml-py:build`, `paigasus-proto-py:build`, `paigasus-workflows-py:build` | `unstable_python`, `unstable_uv` | Task 4 (`python-project.yml` `build`) |
| `paigasus-kernel-py:test` | `unstable_python`, `unstable_uv` | Task 4 (`py/packages/paigasus-kernel/moon.yml:44`) |
| `paigasus-gateway-conformance-py:test` | `unstable_python`, `unstable_uv` | Task 4 |
| `contracts:breaking`, `contracts:fmt`, `contracts:generate`, `contracts:lint` | `system` | Task 4 (`buf.gen.yaml`, `contracts/moon.yml`) |
| all 29 `repo:*` tasks | `system` (each) | Tasks 3 to 5 |
| `paigasus-kernel-ts:*`, `paigasus-py-bindings-rs:*` | `node`, `pnpm` / `rust` | comments only (Task 4) |

The `repo:*` rows are: actionlint, affected-smoke, deny, error-code-single-site, helm-render,
http-extractor-envelope, iam-docker-policy-single-site, input-liveness, install-hooks, machete,
moon-diagnosis-exec, nats-permissions, next-env-drift, next-public-free, observability-drift, osv,
parity-corpus-drift, promtool, publish-metadata, pyo3-stub-drift, redis-connect-single-site,
release-parity, release-parity-py, release-parity-ts, ruff-ci, version-lockstep,
wasm-getrandom-free, wasm-lockstep, workflow-credentials. There is no `repo:release-plan` task; the
gate `ci/release-plan` runs from `repo:release-parity*` and `ci.yml`. A task that moves from
`command: 'uv ...'` to `command: 'bash ...'` can lose the inferred `unstable_python` and
`unstable_uv` toolchains. Task 4 Step 3 detects this with a `diff` against the baseline.

### A.6 Token hits (the Task 3 scan rule, run by hand)

Scan: `git ls-files -z > $SCRATCH/files.z; python3 -I $SCRATCH/scan.py < $SCRATCH/files.z > $SCRATCH/hits.txt`.
Ruling F10: the scan script is `/var/folders/dc/3qfs44nn0574bmkgl_rk6_bw0000gn/T/sma-770/scan.py`
(outside the repository). Task 3 compares its count with the gate's.

**Total: 704 hits in 59 files.** The spec's 325 used a smaller token set. Exclusions applied:
`**/*.md`, `docs/**`, `uv.lock`/`Cargo.lock`/`pnpm-lock.yaml`, `.github/dependabot.yml`,
`ci/affected-graph/python_env_ban.py` and `ci/python-env/run.sh` (the last two do not exist yet).
One tracked non-UTF-8 file was skipped: `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm`.
`.gitattributes:17` already marks it `binary`, so the gate skips it as the spec allows (G4).

Counts by class: execute 65, reword 227, pin 15, coupled 9, delete 388. A *coupled* hit is a
line whose text a gate pins (A.1). It changes with its pinned line. `PINNED_LINES` need hold only
the 15 pins (limit: lower than 35). A hit that is neither in the `execute` list, the `pin` list nor
the `coupled` list of its row is a `reword` (comment, docstring, message, step name, fixture
literal). Per-line classification: `$SCRATCH/classified.tsv` (columns: path, line, class, task,
note, text); the generator is `$SCRATCH/classify.py`.

Task column: 4 = Task 4 (batch 1), 5 = Task 5 (batch 2), 6 = Task 6 (batch 3), 7 = Task 7.

| File | Task | execute | reword | pin | coupled | delete | execute lines | pin lines | coupled lines |
|---|---|---|---|---|---|---|---|---|---|
| `.gitignore` | 4 | 0 | 0 | 1 | 0 | 0 |  | 32 |  |
| `.moon/tasks/python-project.yml` | 4 | 1 | 1 | 0 | 0 | 0 | 26 |  |  |
| `.moon/tasks/python.yml` | 4 | 4 | 0 | 0 | 0 | 0 | 26,29,52,53 |  |  |
| `.moon/tasks/rust.yml` | 4 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `.moon/templates/python/moon.yml` | 4 | 1 | 0 | 0 | 0 | 0 | 8 |  |  |
| `.moon/toolchains.yml` | 4 | 0 | 3 | 1 | 0 | 0 |  | 35 |  |
| `.moon/workspace.yml` | 4 | 0 | 0 | 1 | 0 | 0 |  | 64 |  |
| `.prototools` | 4 | 0 | 2 | 1 | 0 | 0 |  | 31 |  |
| `contracts/buf.gen.yaml` | 4 | 1 | 1 | 0 | 0 | 0 | 48 |  |  |
| `contracts/moon.yml` | 4 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `moon.yml` | 4 | 0 | 14 | 0 | 0 | 0 |  |  |  |
| `osv-scanner.toml` | 4 | 0 | 2 | 0 | 0 | 0 |  |  |  |
| `py/moon.yml` | 4 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `py/packages/paigasus-gateway-conformance/moon.yml` | 4 | 1 | 1 | 0 | 0 | 0 | 18 |  |  |
| `py/packages/paigasus-kernel/moon.yml` | 4 | 1 | 8 | 0 | 0 | 0 | 44 |  |  |
| `py/packages/paigasus-kernel/pyproject.toml` | 4 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `py/packages/paigasus-kernel/tests/test_stub_surface.py` | 4 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `py/pyproject.toml` | 4 | 0 | 1 | 1 | 0 | 0 |  | 29 |  |
| `py/scripts/assert_test_floor.py` | 4 | 0 | 3 | 0 | 0 | 0 |  |  |  |
| `py/scripts/assert_typecheck_coverage.py` | 4 | 0 | 1 | 1 | 0 | 0 |  | 25 |  |
| `py/scripts/run_tests.sh` | 4 | 2 | 0 | 0 | 0 | 0 | 17,32 |  |  |
| `rs/crates/bindings/paigasus-py-bindings/Cargo.toml` | 4 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `rs/crates/bindings/paigasus-py-bindings/src/lib.rs` | 4 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `rs/crates/libs/paigasus-gateway-core/tests/fixtures/make_fixtures.py` | 4 | 0 | 2 | 0 | 0 | 0 |  |  |  |
| `ts/packages/paigasus-kernel/moon.yml` | 4 | 0 | 3 | 0 | 0 | 0 |  |  |  |
| `ci/actionlint/release_guard.py` | 5 | 0 | 4 | 5 | 0 | 0 |  | 214,3115,3117,3631,4866 |  |
| `ci/actionlint/run.sh` | 5 | 5 | 22 | 0 | 1 | 0 | 4624,4660,4674,5310,5987 |  | 5311 |
| `ci/affected-graph/ci_targets.py` | 5 | 0 | 10 | 0 | 4 | 0 |  |  | 896,897,1321,1337 |
| `ci/affected-graph/run.sh` | 5 | 0 | 2 | 0 | 0 | 0 |  |  |  |
| `ci/helm-render/pyproject.toml` | 5 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `ci/helm-render/run.sh` | 5 | 3 | 3 | 0 | 0 | 0 | 92,93,96 |  |  |
| `ci/images/chains.toml` | 5 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `ci/images/console-selftest.sh` | 5 | 0 | 0 | 2 | 0 | 0 |  | 1462,1463 |  |
| `ci/images/run.sh` | 5 | 1 | 2 | 0 | 0 | 0 | 43 |  |  |
| `ci/osv/run.sh` | 5 | 0 | 5 | 0 | 0 | 0 |  |  |  |
| `ci/osv/shipped_reachability.py` | 5 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `ci/publish-metadata/run.sh` | 5 | 0 | 2 | 0 | 0 | 0 |  |  |  |
| `ci/release-parity/ecosystems/python-semantic-release.sh` | 5 | 1 | 5 | 0 | 0 | 0 | 48 |  |  |
| `ci/release-parity/run.sh` | 5 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `ci/release-plan/pyproject.toml` | 5 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `ci/release-plan/release_plan.py` | 5 | 0 | 4 | 0 | 0 | 0 |  |  |  |
| `ci/release-plan/run.sh` | 5 | 9 | 16 | 0 | 1 | 0 | 41,60,121,140,271,286,390,430,462 |  | 408 |
| `ci/ruff/run.sh` | 5 | 1 | 24 | 0 | 0 | 0 | 90 |  |  |
| `ci/version-lockstep/run.sh` | 5 | 1 | 25 | 2 | 1 | 0 | 1635 | 1249,1278 | 618 |
| `ci/wasm-lockstep/lockstep_check.py` | 5 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `ci/wasm-lockstep/pin_check.py` | 5 | 0 | 2 | 0 | 0 | 0 |  |  |  |
| `ci/wasm-lockstep/run.sh` | 5 | 3 | 3 | 0 | 0 | 0 | 51,52,55 |  |  |
| `ci/workflow-credentials/pyproject.toml` | 5 | 0 | 1 | 0 | 0 | 0 |  |  |  |
| `ci/workflow-credentials/run.sh` | 5 | 4 | 6 | 0 | 0 | 0 | 26,33,70,78 |  |  |
| `ci/workflow-credentials/workflow_credentials.py` | 5 | 0 | 2 | 0 | 0 | 0 |  |  |  |
| `.github/workflows/ci.yml` | 6 | 1 | 6 | 0 | 0 | 0 | 154 |  |  |
| `.github/workflows/images-rehearsal.yml` | 6 | 2 | 1 | 0 | 0 | 0 | 141,167 |  |  |
| `.github/workflows/images.yml` | 6 | 6 | 2 | 0 | 0 | 0 | 129,135,177,179,262,264 |  |  |
| `.github/workflows/prebuild.yml` | 6 | 1 | 1 | 0 | 0 | 0 | 151 |  |  |
| `.github/workflows/release.yml` | 6 | 12 | 13 | 0 | 0 | 0 | 485,544,1288,1299,1327,1335,1409,1462,1490,1569,1577,1725 |  |  |
| `.github/workflows/security-scan.yml` | 6 | 0 | 3 | 0 | 0 | 0 |  |  |  |
| `.github/workflows/wheels.yml` | 6 | 4 | 2 | 0 | 0 | 0 | 188,437,647,716 |  |  |
| `ci/images/release_decision.py` | 6 | 0 | 3 | 0 | 0 | 0 |  |  |  |
| `ci/affected-graph/cargo_moon_parity.py` | 7 | 0 | 3 | 0 | 2 | 388 |  |  | 5492,8124 |

Totals by task: Task 4 has 11 execute, 49 reword, 6 pin. Task 5 has 28 execute, 144 reword, 9 pin,
7 coupled. Task 6 has 26 execute, 31 reword. Task 7 has 388 delete, 3 reword and 2 coupled (all in
`cargo_moon_parity.py`).

**Pins (15).** Task 4: `.prototools:31`, `.moon/toolchains.yml:35` (`packageManager: 'uv'`),
`.gitignore:32`, `.moon/workspace.yml:64`, `py/pyproject.toml:29`,
`py/scripts/assert_typecheck_coverage.py:25` (`EXCLUDE_PARTS`). Task 5: `release_guard.py:214`
(the `uv\s+publish` detector) with its fixtures `:3115`, `:3117`, `:3631`, `:4866`;
`console-selftest.sh:1462`, `:1463` (a fake binary named like the real tool); and
`version-lockstep/run.sh:1249`, `:1278` (the `[tool.uv]` TOML table key, read as data). Task 6 has
no pin. The reasons are the literal each file needs from an external format or tool.

**Decisions left to the migration task (not hidden):**
- `ci/helm-render/run.sh:96` and `ci/wasm-lockstep/run.sh:55` match `"$HERE/.venv/"*` (a guard that
  the resolved interpreter lives in the project environment). Task 5 moves the guard into the
  wrapper `<ci-project>` operation or turns each line into a pin. Count the pin if it stays.
- `ci/ruff/run.sh:137,139,143,169` use a `.venv` directory as a fixture of the ruff corpus
  exclusion. Task 5 uses another ignored directory name that tests the same behaviour, or pins it.
- `ci/release-plan/run.sh:390` runs `command -v uv` under a restricted `PATH` to prove the
  missing-tool arm (row 6). With the wrapper, the missing tool is the wrapper's exit 127. Task 5
  reworks the test with the wrapper and keeps the pin of A.1 row 4 in step.
- `ci/release-plan/release_plan.py:1291-1292` and `ci/images/release_decision.py:732` use the word
  `pip` as an unknown-kind fixture. Reword to another unknown word; the test meaning is unchanged.
- `.github/workflows/release.yml:1569` is the R-inline `python3 -c` (Task 6 Step 2).
- `ci/affected-graph/cargo_moon_parity.py:143, 383, 494` stay in the file (outside A14) and need
  new words. All other hits in that file are A14 code, comments or fixtures and go in Task 7.
- Operation mapping (for the wrapper, from the execute lines): `uv run --locked --project py ...` is
  `run py`; `uv run --locked --project "$HERE" ...` is `run <ci-project>`; `uv run --frozen` in
  `python-semantic-release.sh:48` is `run-frozen py`; `uv run --no-project --python '>=3.12' python3`
  on `release_decision.py` is `script`; `uv build` is `build`; `uv lock` is `lock`;
  `command -v uv` is `probe`; `uv cache dir` is `cache-dir`; `proto install uv` is `install`;
  the `pip` installs are `install-ziglang`, `install-ziglang-system`, `wheel-smoke`,
  `sdist-install`; `buf.gen.yaml:48` is `protoc-plugin`; `paigasus-kernel/moon.yml:44` is
  `kernel-test`.
