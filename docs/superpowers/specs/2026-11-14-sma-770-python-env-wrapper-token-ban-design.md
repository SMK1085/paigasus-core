# SMA-770 — one Python-env wrapper and a token ban; kernel 0.2.1; darwin host strip

- Linear: SMA-770 (follows SMA-765, SMA-767, SMA-768; PRs #457–#461)
- Date: 2026-11-14
- Status: revision 2, after the builder check and the spec challenge (§11). For GATE 1.

## 1. Intent

SMA-770 closes the residuals of SMA-765, SMA-767 and SMA-768 in one branch. It has three parts.

- **Part 1.** Check A14 in `ci/affected-graph/cargo_moon_parity.py` reads shell text with
  regular expressions. Every review round found a new edge case (R1, R2, R10, and the
  `cd`/`source` false red). More text rules cannot close the list. Part 1 replaces the parser
  with one wrapper script and a plain token ban.
- **Part 2.** The published `paigasus-py-bindings` 0.2.0 macOS wheels have the SMA-767 defect
  (`stroff mod 8 = 4`). Part 2 ships kernel 0.2.1, which is built with the SMA-767 fix.
- **Part 3.** A hand-typed `cargo build --release --target <apple-darwin>` on a darwin host still
  strips host units with `rust-objcopy`. Part 3 finds a repository setting that stops this on
  darwin only, or records the measured reason why no such setting exists.

## 2. Measured facts

All facts are from this branch (base `11e99a21`), measured on 2026-11-14.

- M1. **A14 today.** About 800 lines of production code and about 450 lines of inline self-test
  in `cargo_moon_parity.py`. The waiver table `ALLOW_UV_ENV_SCRIPT` (from `:569`) has 35 entries.
  34 are prose or data. 1 is executed: `uv lock` in `ci/version-lockstep/run.sh`.
- M2. **A14's scope today.** Moon task text from `moon query projects`, the scripts that tasks
  reference, every `pyproject.toml` and `uv.toml`, and `.moon/toolchains.yml`. It does not scan
  workflows, `contracts/buf.gen.yaml`, Dockerfiles, `lefthook.yml` or `package.json` scripts.
- M3. **Token hits.** With the boundary of §5.3 (the same boundary as today's `UV_TOKEN_RE`,
  `(?<![\w.-])…(?![\w.-])`), a scan of every tracked file except the §5.3 exclusions finds
  **325 lines in 59 files**. Of these, 298 lines in 48 files are in moon files, `.moon/`,
  workflows, `ci/`, `ops/` and `scripts/` dirs. The other 27 lines are in 11 files:
  `.github/dependabot.yml` (11), `.prototools` (3), `contracts/buf.gen.yaml` (2),
  `osv-scanner.toml` (2), `py/pyproject.toml` (2), and one line each in 6 other files.
  `py/uv.lock`, `uv.toml` and `uv-cache` do not match.
- M4. **An executed call outside A14.** `contracts/buf.gen.yaml:48` runs
  `['uv', 'run', '--project', '../py', 'protoc-gen-python_betterproto2']` with no `--locked`, on
  every `contracts:generate` and in the CI codegen-drift step.
- M5. **Executed workflow operations.**
  - 14 lines of `uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py …`
    (`release.yml` image jobs, `images.yml`, `images-rehearsal.yml`), and one inline
    `crane config "$1" | uv run --no-project … python3 -c '…'` (`release.yml:1569`).
  - `uv build --package paigasus-proto` (`release.yml:544`, job `proto-dist`) and
    `uv build --package paigasus-kernel` (`wheels.yml:716`).
  - `python -m pip install ziglang` (`wheels.yml:188`, the setup-python interpreter, no
    `shell: bash`) and `pip3 install --break-system-packages ziglang` (`prebuild.yml:151`, system
    Python, in `rs/`).
  - a wheel smoke install, `"$vpy" -m pip install --no-index --disable-pip-version-check "$WHEEL"`
    (`wheels.yml:437`).
  - an sdist install in job `sdist-verify` (`wheels.yml:585-671`): `"$vpip" install dist/*.tar.gz`.
    That job has **no checkout** on purpose (`:623`). The install builds from source and needs an
    index.
  - `proto install uv` (5 lines: `release.yml` `plan` and two image jobs, `images.yml`,
    `images-rehearsal.yml`) and `uv cache dir` (`ci.yml:154`).
  - `ci/images/run.sh:43` also uses `uv run --no-project`.
- M6. **uv 0.11.16 flags.** `uv run` accepts `--isolated`, `--locked`, `--frozen`, `--no-project`.
  `uv build` accepts none of `--isolated`, `--locked`, `--frozen`. It builds in a temporary
  environment by default. Only `--no-build-isolation` turns that off.
- M7. **Moon toolchain.** `.moon/toolchains.yml:33-37` names `packageManager: 'uv'` and the key
  `unstable_uv:`. Moon runs its own `uv sync --no-managed-python --no-python-downloads
  --no-progress`. SMA-765 accepted that action (R-toolchain).
- M8. **Locks.** `py`, `ci/release-plan`, `ci/workflow-credentials`, `ci/wasm-lockstep` and
  `ci/helm-render` each have a committed `pyproject.toml` and `uv.lock`. `ci/images` has no project.
- M9. **`UV_*`, `VIRTUAL_ENV`, `--active`.** 4 lines in scope, all comments. No workflow sets a
  `UV_*` variable.
- M10. **Windows legs.** The `wheels.yml` and `prebuild.yml` steps that run pip after a checkout
  have `shell: bash`, except `wheels.yml:188`. No step on a Windows leg runs a repository `.sh` file
  today. `.gitattributes` has no `eol=lf` rule for `*.sh`.
- M11. **The SMA-767 fix is in no release.** Kernel tags are `v0.1.0`, `v0.1.1`, `v0.2.0`. No
  commit since `paigasus-kernel-v0.2.0` touches `rs/crates/libs/paigasus-kernel/`. Commit
  `3220c9ca` (`fix(rs)`) touched only the py-bindings crate, which is `publish = false`, so
  release-plz bumped nothing.
- M12. **Kernel package files.** `rs/crates/libs/paigasus-kernel/Cargo.toml:20` has
  `include = [… "README.md" …]`. A README change is a change to the packaged crate.
- M13. **Squash merge settings.** `squash_merge_commit_title = COMMIT_OR_PR_TITLE` and
  `squash_merge_commit_message = COMMIT_MESSAGES`. So the squash body holds every commit message
  of the branch. `release_commits` (`rs/release-plz.toml:46`) matches `feat|fix|perf` at the start
  of the message only. Its `BREAKING CHANGE:` part is multi-line, so it matches a body line.
- M14. **No stable darwin-only cargo switch is known** (from the cargo docs; §7 measures it).
  Profile keys cannot be scoped to a target. `[target.<triple>] rustflags` do not reach host units
  when `--target` is given. The `[host]` table and `target-applies-to-host` are unstable.
  `[profile.release.build-override] strip = "none"` was measured in SMA-767: the Linux target dir
  grew by 385 MB and every shipped Linux binary changed, because the proc-macro hashes changed.

## 3. Decisions by Sven

- D1 (2026-11-14, in the issue). Use one approved wrapper and a token ban, not a shell parser.
  Delete the A14 parser code and the waivers that the ban makes unnecessary.
- D2 (2026-11-14, in the issue). Ship a kernel patch release. This replaces the SMA-767 decision
  "no extra 0.2.0 release". Fix all three parts in one branch.
- D3 (2026-11-14, in the issue). Part 3 is done when a darwin-only setting is found, or when the
  measured reason is in `rs/CLAUDE.md`. Do not leave it open.
- D4 (2026-11-14, brainstorm). **Reword every line that only names the tool.** There is no comment
  skip and no prose waiver list. Rejected: a (file, line) prose waiver list; a skip of `#` lines
  (a `#` line in a heredoc runs, so the skip is a parser again).
- D5 (2026-11-14, brainstorm). **Approach 2 (§4).** A small set of generic wrapper operations
  that write every uv flag themselves.
- D6 (2026-11-14, builder check). **Accept build isolation as the isolation of `build`
  (R-build).** Rejected: a `uv lock --check` before each build; keeping `uv build` out of the
  wrapper.

## 4. Approaches

1. **Closed named operations.** One operation per call site. About 20 operations. Every new tool
   use needs a wrapper edit. Not chosen.
2. **Generic operations with fixed flags. Chosen (D5).** About 14 operations. The caller names an
   operation, a project from a fixed list, and the program to run. The wrapper writes every uv
   flag.
3. **A Python wrapper.** It needs a Python before uv exists, on Windows legs and under bash 3.2
   gates. Rejected.

## 5. Design — Part 1

### 5.1 The wrapper: `ci/python-env/run.sh`

- The path holds no banned token.
- Bash. It runs under system `/bin/bash` 3.2, Homebrew bash 5.x, the Linux CI bash and Git bash on
  `windows-latest`. No `mapfile`, `declare -A`, `local -n`, here-string, or expansion of a
  possibly empty array under `set -u`.
- It finds the repository root from its own location.
- SPDX header. `set -euo pipefail`. `export PROTO_REPORTER=text` at the top.
- Usage errors exit 2 with a `python-env:` prefix. A missing tool exits 127. Every other exit code
  of the program that the wrapper runs passes through unchanged (`release_decision.py` uses 2, 3
  and 4).
- **Style rule.** Each uv, pip or proto command is one literal line per branch of a `case`. No
  flag is held in a variable or an array. So the text of the wrapper shows every command in full.

**Operations.** The builder may rename an operation. The builder may not add a caller-supplied
uv flag.

| Operation | Command the wrapper runs | Callers |
|---|---|---|
| `run py <cmd…>` | `uv run --locked -- <cmd…>` in the caller's directory, which must be inside `py/` | `.moon/tasks/python.yml`, `.moon/templates/python/moon.yml`, gateway-conformance `test`, `ci/ruff`, `ci/actionlint` |
| `run <ci-project> <cmd…>` | `uv run --locked --project <root>/<ci-project> --python '>=3.12' -- <cmd…>` | `ci/release-plan`, `ci/workflow-credentials`, `ci/wasm-lockstep`, `ci/helm-render`, `ci/actionlint` |
| `run-frozen py <cmd…>` | `uv run --frozen -- <cmd…>` (same directory rule) | `ci/release-parity/ecosystems/python-semantic-release.sh:48` |
| `protoc-plugin` | `uv run --locked --project <root>/py protoc-gen-python_betterproto2` | `contracts/buf.gen.yaml:48` (new `--locked`) |
| `kernel-test` | `uv run --isolated --reinstall-package paigasus-py-bindings pytest tests` in the kernel package dir | `paigasus-kernel-py:test` (the SMA-765 floor) |
| `script <file.py> [args…]` | `uv run --isolated --no-project --python '>=3.12' python3 <file.py> [args…]` | 14 workflow lines, `ci/images/run.sh:43` |
| `build` | `uv build` in the caller's directory | `.moon/tasks/python-project.yml:26` |
| `build <package> <outdir>` | `uv build --package <package> --out-dir <abs outdir>` in `py/` | `release.yml:544`, `wheels.yml:716` |
| `lock` | `uv lock` in `py/` | `ci/version-lockstep/run.sh` `--write` |
| `cache-dir` | `uv cache dir` | `ci.yml:154` |
| `install` | `proto install uv` | 5 workflow lines |
| `probe` | `command -v uv`; exit 0 or 127, no output | `ci/release-plan`, `ci/workflow-credentials` |
| `install-ziglang <python>` | `<python> -m pip install [--break-system-packages] ziglang`, as each caller does today | `wheels.yml:188`, `prebuild.yml:151` |
| `wheel-smoke <venv-dir> <wheel>` | `<venv python> -m pip install --no-index --disable-pip-version-check <wheel>` | `wheels.yml:437` |
| `sdist-install <venv-dir> <sdist>` | `<venv pip> install <sdist>` (with an index; a source build) | `wheels.yml:659` |

Fixed lists in the wrapper:
- `<ci-project>` is one of `ci/release-plan`, `ci/workflow-credentials`, `ci/wasm-lockstep`,
  `ci/helm-render`. Any other value exits 2.
- `<package>` is `paigasus-proto` or `paigasus-kernel`. Any other value exits 2.

Rules:
- **R-cwd.** `run py` and `run-frozen py` pass no `--project`. uv finds the project from the
  caller's directory, exactly as today. So a task in `py/packages/paigasus-gateway-conformance`
  still selects that member. The wrapper only checks that the directory is inside `py/`. The other
  operations change the directory only where the table says so.
- **R-outdir.** `build <package> <outdir>` makes `<outdir>` absolute before it changes to `py/`.
  It refuses a value that starts with `-`. `proto-dist` keeps its `dist/` path at the repository
  root.
- **R-lock.** `lock` exits 2 when `MOON_PROJECT_ID` is set, so no Moon task can re-lock. This
  replaces A14 rule 8, and R10 cannot occur. The plan measures that Moon 2.5.3 sets
  `MOON_PROJECT_ID` in a task. A task that starts the wrapper under `env -i` removes the variable;
  this is a residual (§9).
- **R-build.** `build` never passes `--no-build-isolation` (D6).
- **R-script.** `script` adds `--isolated`. Today's callers do not pass it. The plan measures that
  `release_decision.py` uses only the standard library, that `--isolated --no-project` prints no
  extra output on stdout, and that exit codes 2, 3 and 4 pass through.
- **R-inline.** The inline `python3 -c` at `release.yml:1569` moves into a new
  `release_decision.py` subcommand that reads stdin, with its own unit test. Then it uses `script`.
- `--` separates the uv options from the program, so a program argument cannot become a uv flag.
- No `--self-test` in the wrapper. The gate's self-test runs it with a fake `uv`, `pip`, `python`
  and `proto` on `PATH` (§5.5).
- The wrapper does not unset `UV_*`. A developer's `UV_*` value is a residual (§9).

### 5.2 Call sites

Every executed operation in scope goes through the wrapper. Every other hit gets new words (D4).
In order of preference:

1. An executed operation becomes a wrapper call.
2. A comment, message, step name or fixture name gets new words: "the Python package manager",
   "the project environment", or the wrapper's name. `Install uv` becomes
   `Install the Python package manager`.
3. A literal that cannot change is a **pinned line** (§5.4). Each one has a reason that says why the
   text cannot change. Expected classes:
   - external vocabulary: `.prototools:31` (`uv = "0.11.16"`), `.moon/toolchains.yml` (M7),
     `.gitignore:32` (`.venv/`), the `.venv` exclude globs in `.moon/workspace.yml:64` and
     `py/pyproject.toml:29`;
   - detection data that must hold the literal to detect it: `ci/actionlint/release_guard.py`
     (`:214` `r"uv\s+publish"` and its rows), the fake `uv` in `ci/images/console-selftest.sh`, the
     `version-lockstep` order pin at `ci/version-lockstep/run.sh:618`, and the coupled pins in
     `ci/affected-graph/ci_targets.py`.
   - **Never split a token** (`"u""v"`, `u\v`) to pass the ban. A split is a defect. The self-test
     does not detect it; review does.

There is no fixed target count. The PR states the count of pinned lines and the reason of each.
The count must be lower than today's 35.

Coupled pins: a gate that greps for uv text in another gate's file (for example
`ci/version-lockstep/run.sh:618`, `ci/affected-graph/ci_targets.py:896-897` and `:1321`,
`T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh:2127`) changes in the same diff. The plan
lists every such pin before the first call site moves.

Fixtures that test a `.venv` path (for example `ci/ruff/run.sh:137-169`, and the
`"$HERE/.venv/"*)` compare in `ci/wasm-lockstep` and `ci/helm-render`) get a different literal that
still tests the same behaviour where they can. Where they cannot, they become pinned lines.

Workflow specifics:
- `sdist-verify` gets a sparse checkout of `ci/python-env/` only, placed before the sdist download.
  The job comment (`wheels.yml:623`) says that a checkout is harmless there: `rustup default`
  stays authoritative, and pip builds in a temporary directory. The plan keeps that comment true.
- `wheels.yml:188` gets `shell: bash`.
- `.gitattributes` gets `*.sh text eol=lf`, so Git bash on Windows does not see CR. The plan
  checks that no tracked `.sh` file changes bytes.
- `ci/python-env/**` is added to the `pull_request.paths` filters of `wheels.yml`, `prebuild.yml`
  and `images.yml`, so a wrapper change runs those workflows on its PR.

### 5.3 The gate (replaces A14)

The new check is a small module, `ci/affected-graph/python_env_ban.py`, which
`cargo_moon_parity.py` imports. It keeps A14's key in `collect_findings` and its place in
`repo:affected-smoke`. Only the module and the wrapper are exempt from the ban. So the rest of
`cargo_moon_parity.py` (A1–A13) stays in scope.

**Scope.** Every file from `git ls-files`, except:
- `**/*.md` and `docs/**` (prose for people);
- lockfiles (`**/uv.lock`, `**/Cargo.lock`, `**/pnpm-lock.yaml`);
- binary files (a file that is not UTF-8 is skipped only when `.gitattributes` marks it `binary`;
  any other non-UTF-8 file raises `MoonOutputError`, as G4 does today);
- `.github/dependabot.yml`. Reason: it uses dependabot's ecosystem names (`package-ecosystem: uv`)
  and runs nothing. Its comments get new words anyway.
- `ci/affected-graph/python_env_ban.py` and `ci/python-env/run.sh`.

A new location is in scope by default. The scope uses no references from task text, so the gate
reads no shell text. R2 and the `cd`/`source` false red cannot occur.

**Tokens.** Case-sensitive, with the boundary `(?<![\w.-])TOKEN(?![\w.-])`:
- `uv`, `uvx`, `pip`, `pip3`, `pip3.<n>`, `ensurepip`, `virtualenv`;
- `.venv` (with `(?<![\w-])` before it, so a path part matches);
- `UV_[A-Z0-9_]+` and `VIRTUAL_ENV` (A14 rules 6 and 7; a Moon task `env:` block is text in a
  moon file, so it is in scope);
- `--active` (A14 rule 6).

Known non-matches, recorded in the self-test: `uv.lock`, `uv.toml`, `uv-cache`, `pip.exe`,
`pipefail`, `pipeline`, `uvicorn`, and `uv.` at the end of a sentence. The last one is a false
green. It is accepted: a sentence is prose, and an executed `uv.` is not a command.

**Rules.**
- G-ban. Any token on any line of a scoped file is a row, except a pinned line (§5.4).
- G-wrapper. The sha256 of `ci/python-env/run.sh` must equal a constant in the module. The row
  prints the new hash. So any change to the wrapper needs a change to the gate in the same diff,
  and review sees both. The behaviour checks are in the self-test (§5.5).
- G-floor. The command of `paigasus-kernel-py:test` from `moon query projects` must contain the
  exact text `ci/python-env/run.sh kernel-test`. This is a substring check. The real-tree negative
  control in `main()` removes that text from a copy and must see the row.
- G-toolchain. `.moon/toolchains.yml` must not have `syncArgs`, `venvArgs` or `installArgs`.
- G4 (non-UTF-8) and G5 (every file the gate reads matches an input of `repo:affected-smoke`) stay.
  Because the scope is now the whole tree, `repo:affected-smoke` gets `**/*` as an input minus the
  exclusions, or the builder shows a smaller input set that G5 still proves.
- The TOML key check (`uv_config_rows`: `reinstall`, `upgrade`, `cache-keys` in `pyproject.toml`
  and `uv.toml`) stays. It reads TOML with `tomllib`. It is not shell parsing.

**Deleted.** The A14 parser and its regex constants, `_uv_*` helpers, `uv_form_hits`,
`uv_mutations`, `_floor_isolated`, the lockstep `--write` guard (`LOCKSTEP_*`, `_split_commands`,
`_expand_words`, `runs_lockstep_write`), `REQUIRED_UV_ISOLATED_TASKS`, `ALLOW_UV_ENV_SCRIPT`, and
their self-test rows.

**Moon toolchain inference.** A task command that changes from `uv …` to
`bash ci/python-env/run.sh …` can change the toolchains that Moon infers for the task, and so
whether Moon runs its setup and puts the tool on `PATH`. The plan compares
`moon query tasks` output (each task's `toolchains`) before and after. Where the set changes, the
task gets an explicit `toolchains:` entry.

### 5.4 Pinned lines

`PINNED_LINES` is a table of `(path, exact stripped line, reason)`. A token on a pinned line is not
a row. An entry with an empty reason is a row. An entry that matches no line, or more than one line,
is a row (stale or ambiguous). This replaces `ALLOW_UV_ENV_SCRIPT`.

### 5.5 Self-test and mutations

- One red row per token, and one green row per known non-match (§5.3).
- One row per G rule, including a pinned line with an empty reason, a stale pinned line and an
  ambiguous pinned line, and a file under each exclusion.
- **Guard the guard.** A row checks that every token in the token list has at least one red row,
  and that every exclusion has a row. So a token removed from the gate, or a row removed from the
  self-test, fails the self-test.
- **Wrapper behaviour.** The self-test runs the real wrapper with fake `uv`, `pip`, `python` and
  `proto` on `PATH`. Each fake records its argument list and its working directory. For every
  operation the self-test compares the record with an expected list. It also checks every usage
  error (unknown operation, unknown project, unknown package, `-` outdir, `lock` with
  `MOON_PROJECT_ID`, `run py` outside `py/`) and the exit-code pass-through.
- **Real-tool check.** A new step in `repo:affected-smoke` (or a separate small task, the builder
  decides) runs the cheap operations against the real uv on every PR: `probe`, `cache-dir`,
  `run py python -c pass`, `script` with a small file, and `build paigasus-proto <tmp>`. So the
  operations that run only on `main` (`install`, `build`, `lock`, `script`) are proven on the PR.
- A mutation battery in the plan, run by hand and recorded in the PR: remove each rule in turn and
  show that the self-test fails. Every mutation must leave the file runnable, so the failure is the
  assertion, not a syntax error.

### 5.6 Docs

- `ci/affected-graph/README.md`: replace the A14 section with the new gate. Keep a short history
  line on why the parser went away (R1, R2, R10, `cd`/`source`).
- `ci/CLAUDE.md`: one rule. Every uv, pip and `.venv` operation goes through
  `ci/python-env/run.sh`. To add one, add a wrapper operation, update the sha256 and the expected
  list in `python_env_ban.py`.
- `ci/python-env/README.md`: the operation table and the rules.
- `.github/CLAUDE.md` and `contracts/CLAUDE.md`: one line each, because workflow steps and
  `buf.gen.yaml` are now in scope.

## 6. Design — Part 2: kernel 0.2.1

- This PR touches one file under `rs/crates/libs/paigasus-kernel/`: a short note in its README
  about macOS 27 and the SMA-767 fix. `README.md` is in the crate's `include` list (M12), so
  release-plz sees a change to the crate.
- The PR title starts with `fix(rs):`. The squash title is the PR title (M13). `fix(rs)` gives a
  patch bump in 0.x. The squash body holds every commit message (M13). So **no commit on this
  branch has a body line that starts with `BREAKING CHANGE:` or `BREAKING-CHANGE:`**, and no
  commit uses `!`. The plan checks this before the PR opens.
- After the merge, release-plz opens the release PR. Its job runs
  `ci/version-lockstep/run.sh --write`, which stamps 0.2.1 on the bindings (SMA-685). Nobody bumps a
  version by hand. Nobody places a tag by hand.
- Sven reviews and merges the release PR. Then he approves `release-approval`. Only then does the
  release publish.

**Risk of the release path.** The 0.2.1 run is the first run of the new `install` (job `plan`),
`build` (job `proto-dist`) and `lock` (the release-PR stamp) on `main`. Each of these jobs runs
**before** `approve-release`. So a broken call stops the release before anything publishes. It does
not leave a half-published release. The real-tool check (§5.5) runs `build` and `script` on the PR.
A broken `probe` fails safe in `ci/release-plan` (it builds and then waits at approval, SMA-603); the
self-test covers `probe`. The image chains run after `approve-images-*`, but a kernel-only release
does not run them. Their first run is at the next image release. The real-tool check and the
`release_decision.py` unit tests cover `script`.

Checks after the release (Sven approves first; Claude runs them and reports):

```bash
# macOS wheels from PyPI (arm64 and x86_64)
otool -l <extracted .so> | grep -A4 LC_SYMTAB         # stroff mod 8 = 0
otool -l <extracted .so> | grep -A5 LC_BUILD_VERSION  # record the sdk
python -c 'import paigasus_py_bindings'               # in a venv on macOS 27
# npm darwin packages: the same otool check on the .node file
```

These checks happen after GATE 2. The PR cannot show them.

## 7. Design — Part 3: darwin host strip

**Method.**
- A host unit is a rustc call in `cargo build --release -v --target aarch64-apple-darwin` output
  with no `--target` argument.
- Use a fresh `CARGO_TARGET_DIR` for each candidate. A cached build prints no rustc lines, so a
  count of 0 from a cached build is a false pass.
- On darwin, count the host units with `-C strip=debuginfo`. Record `stroff mod 8` for the host
  binaries. All 9 are aligned today (`rs/CLAUDE.md`), so `stroff` cannot tell candidates apart. The
  count is the measure.
- On Linux, build in Docker (`rust:1.95.0-bookworm`, as in SMA-767; no host install). First run an
  A/A control: two clean builds with no change, to prove that the build is byte-reproducible. Then
  compare the shipped binaries (the py-bindings `.so`, the node-bindings `.node`, and the service
  binaries; the plan lists the paths) and the size of the target directory.

| # | Candidate | Expected (from docs) |
|---|---|---|
| C1 | `[profile.release.build-override] debug = "line-tables-only"` in `rs/Cargo.toml` | darwin 0 stripped; Linux bytes change (proc-macro hashes) |
| C2 | the same key in `rs/.cargo/config.toml` `[profile]` | as C1 (not measured separately unless C1 passes) |
| C3 | `[env] CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_STRIP = "none"` in `rs/.cargo/config.toml` | no effect: `[env]` sets the env of processes that cargo starts, not cargo's own config |
| C4 | `[target.aarch64-apple-darwin] rustflags = ["-C", "strip=none"]` with `--target` | no effect on host units (measured in SMA-767) |
| C5 | `[build] rustc-wrapper` that removes `-C strip=debuginfo` from host calls on Darwin | rejected without a measurement, see below |

C5 is rejected in writing. `[build] rustc-wrapper` has no per-target form, so it applies on Windows
too, where a bash wrapper cannot run (the wheels and prebuild Windows legs build in `rs/`). An
env `RUSTC_WRAPPER` (for example sccache) overrides it. It runs on every compile.

A candidate passes when all of these are true:
- darwin: 0 host units with `-C strip=debuginfo`, and every host binary has `stroff mod 8 = 0`;
- Linux: the shipped binaries are byte-identical (after a passing A/A control);
- Linux: the target directory grows by less than 50 MB. My call for "not a large amount": about 3%
  of the 1.45 GB measured in SMA-767. The byte rule decides first.

Measure C1 and C3. If C1 to C4 fail, the measured table is the result (D3). `rs/CLAUDE.md` records
it, and the `CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_STRIP` rule stays. If a candidate passes, it goes
in, and the darwin env steps in `wheels.yml` and `prebuild.yml` and the `uname` exports in
`ts/packages/paigasus-kernel/moon.yml` are removed in the same diff. Nightly features and a
`direnv` `.envrc` are out of scope.

## 8. Goals, non-goals, anti-goals

- Goals: §5 to §7.
- Non-goals: change Moon's own `uv sync` action; change what any gate checks other than A14; change
  the release workflow structure.
- Anti-goals:
  - A token ban with a hidden parser (a comment skip, a quote tracker, a reference walk).
  - A split token to pass the ban.
  - A release that publishes before Sven approves `release-approval`.
  - A change in the bytes of a shipped Linux binary because of Part 3.

## 9. Residuals

- **Deliberate split.** `u""v` or a token built at run time passes any token ban. The threat model
  is a mistake, not an attacker. Review catches it (§5.2).
- **Env from outside the repository.** A developer's `UV_*` value before a local run. No static gate
  sees it. Accepted.
- **R-lock under `env -i`.** A task that clears the env removes `MOON_PROJECT_ID`. Accepted; the
  ban still forces the call through the wrapper, and review sees `env -i`.
- **`uv.` at the end of a sentence** is a false green (§5.3). Accepted.
- **R-toolchain.** Moon's own `uv sync` stays accepted (SMA-765).
- **`uv build` has no `--locked`** (D6).
- **Image chains** first run the new wrapper calls at the next image release (§6).

## 10. Left to the builder

- Operation names, and the exact per-runner form of `install-ziglang`.
- The new words for each reworded line, and the literal for each `.venv` fixture.
- Whether the real-tool check is a step in `repo:affected-smoke` or its own task.
- The `repo:affected-smoke` input set that G5 proves.
- The order of the plan's tasks. The coupled-pin list comes first.
- The bash for local runs: `ci/affected-graph/run.sh` needs system `/bin/bash` 3.2.

## 11. Review log

**Builder check (Sonnet).** 10 questions. 9 answered by measurement or settled as my call (§5.1
R-cwd, M8, M9, M10, M12, §5.5, §5.2, §7). 1 went to Sven (D6).

**Spec challenge (Opus), verdict NEEDS REWORK.** Folded in:
- BLOCKER scope misses `buf.gen.yaml` → scope inverted to every tracked file (§5.3), new
  `protoc-plugin` operation (M4).
- BLOCKER M3 regex matched `uv.lock` → one boundary, re-measured (M3).
- BLOCKER `sdist-verify` has no checkout → sparse checkout and `sdist-install` (§5.2).
- MAJOR release-only call sites → R-inline, PR path filters, real-tool check (§5.5), risk analysis
  (§6).
- MAJOR G-wrapper sees only token lines → sha256 pin plus behaviour rows (§5.3, §5.5); style rule
  (§5.1).
- MAJOR relative outdir → R-outdir.
- MAJOR Windows `.sh` → `eol=lf` and `shell: bash` (§5.2).
- MAJOR pinned-line target, detection data, coupled pins → no fixed target, no splits, coupled pin
  list first (§5.2).
- MAJOR R-cwd not equal → `run py` passes no `--project` (§5.1).
- MAJOR Part 3 method → fresh target dir, host-unit definition, A/A control, C5 rejected in writing
  (§7).
- MINOR items → `install-ziglang <python>`, own module so A1–A13 stay in scope, R-lock measurement
  and residual, `env:` blocks are in scope as text, line numbers corrected.
- QUESTIONS → M11 (no kernel commit since v0.2.0, so 0.2.1), M13 and the commit-body rule (§6),
  Moon toolchain inference (§5.3), R-script measurements (§5.1), removal of the darwin steps if C
  passes (§7).

Rejected:
- "Ship Part 2 first in its own PR, or keep the `release.yml` call sites until 0.2.1 has
  published." Reason: Sven asked for one branch (D2). Every changed call site in the kernel release
  path runs before `approve-release`, so a break stops the release before anything publishes (§6).
  The real-tool check proves the operations on the PR.
