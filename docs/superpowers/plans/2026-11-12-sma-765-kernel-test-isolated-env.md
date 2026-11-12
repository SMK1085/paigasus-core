# SMA-765 Kernel Test Isolated Environment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `paigasus-kernel-py:test` stops writing the shared `py/.venv`, and a new gate A14 keeps it that way.

**Architecture:** The kernel test script changes from `uv sync --reinstall-package … && uv run pytest tests` to one `uv run --isolated --reinstall-package paigasus-py-bindings pytest tests`, so uv uses a temporary environment in its cache. Gate A14 (`check_uv_env_isolation`) in `ci/affected-graph/cargo_moon_parity.py` reds any Moon task that mutates a uv project environment in place, with a named floor on the kernel task and a real-tree negative control.

**Tech Stack:** Moon 2.5.3, uv 0.11.16, Python 3 (stdlib only, like the rest of the gate), bash.

**Spec:** `docs/superpowers/specs/2026-11-12-sma-765-kernel-test-own-venv-design.md` (approved at GATE 1). Read §6.2 for the full A14 rule and §7 for the proof.

## Global Constraints

- Every shell command starts with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text`.
- Work only in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-765`, branch `feature/sma-765-py-kernel-pytest-uv-sync-removes-packages`.
- Conventional commits with a scope (`fix(py): …`, `test(ci): …`). End every message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `#` at the start of a body line. No `--no-verify`, no `--amend`, no `git reset`, no force-push.
- Every new source file opens with an SPDX header. No new dependencies; the gate uses the stdlib only.
- Run the gate's Python directly (`python3 ci/affected-graph/cargo_moon_parity.py --self-test`) and its shell suite with system bash: `/bin/bash ci/affected-graph/run.sh`. Do NOT run it through Homebrew bash 5.3.15 (it deadlocks on this Mac).
- The binding built on this Mac does not load (`dlopen … mis-aligned LINKEDIT string pool`, spec F6). The kernel task's pytest step fails locally before and after the change. That is expected; do not try to fix it. CI is the authority for the kernel tests.
- Do not install host software.

## Review Focus

1. A message line that names `uv sync` inside a followed script (for example `ci/release-parity/ecosystems/python-semantic-release.sh:54`, `ci/ruff/run.sh:92`, `ci/actionlint/run.sh:5982`): A14 must red on it unless it is in the allowlist with a reason; the real tree must be green only through explicit allowlist entries. Pinned in Task 1 Step 6.
2. uv global options and odd whitespace between `uv` and the verb (`uv -q sync`, `uv --project py sync`, two spaces, a tab, a `\`+newline continuation): each must red. Pinned in Task 1 Step 1.
3. `uv run --isolated` in the same invocation as `--reinstall-package` must stay green, while `--isolated` in a DIFFERENT `uv` invocation of the same blob must not excuse a mutating one. Pinned in Task 1 Step 1.
4. The existing self-test base fixture (`cargo_moon_parity.py:4430`) still has the old `uv sync` text. Leaving it makes every fixture-based self-test row gain an `a14` row. Pinned in Task 1 Step 3.
5. `py/uv.lock` refresh: today the kernel task's bare `uv sync` relocks `py/uv.lock` (`.github/CLAUDE.md:188`). If `uv run --isolated` does not, a release flow changes silently. Pinned in Task 2 Step 1 (measure, stop if different).

---

### Task 1: Gate A14 — `check_uv_env_isolation`

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` (new constants near `REQUIRED_FFI_TASKS` at :154 and `ALLOW_UNLOCKED_CARGO_SCRIPT` at :470; `moon_projects()` at :3177; new check function after `check_ts_vitest_inputs` (:3068); `self_test()` at :3263; `EXPECTED_FINDING_KEYS` at :6419; `collect_findings()` at :6422; `main()` at :6602)
- Modify: `ci/affected-graph/README.md` (new **A14** entry after A13, line ~320)
- Modify: `moon.yml` (`repo:affected-smoke` task inputs, :184 onward): add `'/py/scripts/**/*'`

**Interfaces:**
- Produces:
  - `REQUIRED_UV_ISOLATED_TASKS = ("paigasus-kernel-py:test",)`
  - `ALLOW_UV_ENV_SCRIPT: dict[tuple[str, str], str]` — key `(repo-relative script path, exact stripped line text)`, value a non-empty reason.
  - `PROJECT_SCRIPT_REF_RE` — matches `(?:bash\s+|sh\s+)?(scripts/[\w./-]+\.sh)` refs, resolved against the project's `source_dir`.
  - `uv_mutations(text: str) -> list[str]` — one description per mutating uv use found in `text` (empty list = clean).
  - `check_uv_env_isolation(projects, root, allow=None, floor=None) -> list[str]` — the A14 rows.
  - `moon_projects()` gains a per-project key `"task_env": {task_name: dict}`; a task without `env` gets `{}`.

- [ ] **Step 1: Write the failing self-test rows for `uv_mutations`**

In `self_test()`, add an A14 block in the same style as the A13 parser block (:5786-5873): a table of `(label, text, want_red)` rows, call `uv_mutations(text)`, and append to `failures` when `bool(result) != want_red`. Rows (spec §6.2):

red (`True`): `"uv sync"`, `"uv  sync"`, `"uv\tsync"`, `"uv -q sync"`, `"uv --project py sync"`, `"uv \\\n  sync"`, `"uv sync --inexact"`, `"uv pip sync r.txt"`, `"uv pip install x"`, `"uv pip uninstall x"`, `"uv venv"`, `"uv add x"`, `"uv remove x"`, `"uv run --reinstall-package p pytest"`, `"uv run --reinstall pytest"`, `"uv run --exact pytest"`, `"uv run --refresh-package p pytest"`, `"UV_PROJECT_ENVIRONMENT=.venv uv run pytest"`, `"export VIRTUAL_ENV=x"`, `"unset UV_PROJECT_ENVIRONMENT"`, `"uv run --active x"`, `"echo \"run uv sync\""`, `"uv run --isolated true && uv run --reinstall-package p pytest"`.

green (`False`): `"uv run --isolated --reinstall-package paigasus-py-bindings pytest tests"`, `"uv run pytest"`, `"uv run --locked --project py ruff"`, `"uv run --frozen x"`, `"touch a b && uv run --isolated --reinstall-package p pytest tests"`.

- [ ] **Step 2: Run the self-test to verify it fails**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: FAIL with `NameError: name 'uv_mutations' is not defined` (or the self-test's own failure line naming A14).

- [ ] **Step 3: Implement `uv_mutations`, the constants, and fix the base fixture**

- Split the text into `uv` invocations first: an invocation starts at `\buv\b` and ends at the next unquoted `&&`, `||`, `;`, `|` or newline that is not a `\` continuation. Judge each invocation on its own, so row "`--isolated` in a different invocation" reds.
- Skip uv global options (`-q`, `-v`, `--quiet`, `--verbose`, `--offline`, `--no-cache`, and the value-taking `--project X`, `--directory X`, `--cache-dir X`, `--config-file X`, `--color X`) before reading the verb.
- Mutating verbs: `sync`, `venv`, `add`, `remove`, and `pip` followed by `sync|install|uninstall`. For `run`: mutating when any of `--exact`, `--reinstall`, `--reinstall-package`, `--refresh-package` is present and `--isolated` is not.
- Independently of invocations, any `UV_PROJECT_ENVIRONMENT` or `VIRTUAL_ENV` assignment, `export` or `unset`, and any `--active` flag, is a finding.
- Strings are NOT stripped (spec §6.2, fail-closed). An `echo` line counts.
- Change the base fixture at :4430 to `"touch ... && uv run --isolated --reinstall-package paigasus-py-bindings pytest tests"` so the existing FFI rows keep matching (`FFI_MARKERS` contains `--reinstall-package`, :145).
- Update the comment at :143 and :457 that says the kernel task runs `uv sync --reinstall-package` to the new text.

- [ ] **Step 4: Run the self-test to verify the parser rows pass**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: no A14 parser failure lines.

- [ ] **Step 5: Write the failing self-test rows for `check_uv_env_isolation`**

Build small `projects` dicts in the self-test (same shape as the fixture at :4420) and assert the exact rows:

- the floor task missing → a row starting `FLOOR:` naming `paigasus-kernel-py:test`;
- the floor task with `uv run --reinstall-package paigasus-py-bindings pytest tests` (no `--isolated`) → a row;
- the floor task with `uv run --isolated pytest tests` (no `--reinstall-package paigasus-py-bindings`) → a `FLOOR:` row;
- a task with `task_env == {"UV_PROJECT_ENVIRONMENT": ".venv-x"}` → a row;
- a task whose blob runs `bash ci/x/run.sh` where that script (a temp file under a temp root) holds `uv sync` → a row naming the script;
- a task whose blob runs `bash scripts/run_tests.sh` with `source_dir` `py`, and `py/scripts/run_tests.sh` holds `uv pip install x` → a row;
- an allowlisted `(script, line)` → no row; an allowlist entry with an empty reason → a row; an allowlist entry that matches no line in any scanned script → a row (stale entry);
- the clean floor task alone → no rows.

- [ ] **Step 6: Implement `check_uv_env_isolation(projects, root, allow=None, floor=None)`**

- For every task in every project: text = blob (`projects[pid]["invocations"][name]`) + the text of every script from `task_script_closure(projects, root, target)` (:1230) + every `PROJECT_SCRIPT_REF_RE` match resolved under `root / source_dir` (raise `MoonOutputError` on an unreadable referenced script, like `task_script_refs` :1077).
- Run `uv_mutations` per source (blob, then each script) so a row names where the hit is. For a script hit, drop it when `(script_rel, stripped_line)` is in `allow` with a non-empty reason.
- A non-empty `task_env` holding `UV_PROJECT_ENVIRONMENT` or `VIRTUAL_ENV` is a row.
- Floor: every target in `floor` must exist and its blob must contain one invocation with both `--isolated` and `--reinstall-package paigasus-py-bindings`; else a `FLOOR:` row.
- Stale allowlist: after the scan, any `allow` key never matched is a row.
- Extend `moon_projects()` with `"task_env"` (`dict(task.get("env") or {})`).
- Run the check once on the real tree: `python3 ci/affected-graph/cargo_moon_parity.py`. Expected now: A14 reds on `paigasus-kernel-py:test` (red-first, the kernel task still runs `uv sync`), plus hits on message lines in followed scripts. For each message-only hit, add an `ALLOW_UV_ENV_SCRIPT` entry with a one-sentence reason ("prints a remedy in an error message; executes no uv command"). Do NOT allowlist an executing line — stop and report it instead.

- [ ] **Step 7: Wire A14 into the report, the registry and the real-tree negative control**

- `EXPECTED_FINDING_KEYS`: append `"a14"`.
- `collect_findings`: add `("a14", check_uv_env_isolation(projects, root), <title>)`. Title, in the house style of A13: what is wrong (a task can change a uv environment in place that another task reads, SMA-765), the fix (`uv run --isolated`, never `uv sync` in a Moon task), and that a `FLOOR:` row means the check cannot be trusted.
- `main()`: extend the PASS sentence with "and no task changes a uv environment in place". After the findings, deep-copy `projects` (json round-trip), remove `--isolated` from `paigasus-kernel-py:test`'s blob, run `check_uv_env_isolation` on the copy, and `return 2` with `FATAL [parity] A14 negative control: …` on stderr if it returns no rows.

- [ ] **Step 8: Run the self-test and the real tree**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test` → Expected: exit 0.
Run: `python3 ci/affected-graph/cargo_moon_parity.py` → Expected: exit 1, the only failing section is A14, and its only row is `paigasus-kernel-py:test` (Task 2 fixes it).

- [ ] **Step 9: Add the README entry and the input**

- `ci/affected-graph/README.md`: an **A14** entry after A13, same form: function name, SMA-765, findings key `a14`; the rule; the scan scope (blob + `task_script_closure` + project `scripts/*.sh`); strings not stripped and the allowlist; the floor; the real-tree negative control in `main()`; the named limit "A14 does not see Moon toolchain actions such as `InstallDependencies(unstable_python, py)`".
- `moon.yml`, `repo:affected-smoke` inputs: add `'/py/scripts/**/*'` with a one-line comment (A14 reads `py/scripts/run_tests.sh`).
- Run `/bin/bash ci/affected-graph/run.sh --self-test` → Expected: exit 0.

- [ ] **Step 10: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py ci/affected-graph/README.md moon.yml
git commit -m "test(ci): add affected-smoke A14 for in-place uv environment writes (SMA-765)"
```

---

### Task 2: The kernel test runs in an isolated environment

**Files:**
- Modify: `py/packages/paigasus-kernel/moon.yml` (task `test`, script at :36 and its comment block)
- Modify: `py/README.md` (the stale-binding remedy, spec R-stale)
- Modify, only if Step 1 shows a change: `.github/CLAUDE.md:188`

**Interfaces:**
- Consumes: Task 1's A14 (`python3 ci/affected-graph/cargo_moon_parity.py` must go from red to green).

- [ ] **Step 1: Measure the lockfile behavior (stop if it differs)**

In a scratch copy of the workspace state (a temp `git worktree` or a copy of `py/`, never the branch's `py/uv.lock`), make `py/uv.lock` stale by bumping `version` in `py/packages/paigasus-kernel/pyproject.toml`, then in `py/packages/paigasus-kernel` run (a) `uv sync --reinstall-package paigasus-py-bindings` and (b) `uv run --isolated --reinstall-package paigasus-py-bindings python -c 0`, each from the same stale state. Record whether each one rewrote `uv.lock`.
Expected: both rewrite it. If (b) does NOT, stop and report to the coordinator with the measurement; do not change the task.

- [ ] **Step 2: Record the red-first baseline (spec §7)**

```bash
(cd py && uv sync --all-packages --locked)
(cd py && uv pip list --python .venv) | grep -E '^(grpclib|anthropic) '   # must print 2 lines
m=$(mktemp); moon run paigasus-kernel-py:test --force; find py/.venv -newer "$m" | wc -l
(cd py && uv run --no-sync python -c 'import grpclib, anthropic')
```
Expected BEFORE the change: the `find` count is > 0, and the import fails with `ModuleNotFoundError`. (The moon run itself fails at pytest because of the local dlopen limit; that is expected.) Save the numbers for the PR body.

- [ ] **Step 3: Change the task**

Script becomes: `touch ../../../rs/crates/libs/paigasus-kernel/src/lib.rs ../../../rs/crates/bindings/paigasus-py-bindings/src/lib.rs && uv run --isolated --reinstall-package paigasus-py-bindings pytest tests`. Keep `deps` and `inputs` unchanged. Rewrite the comment above the task: why `--isolated` (SMA-765; an exact `uv sync` here removed 18 packages from the shared `py/.venv` while `py:test` collected, CI run 43295812102), that `--reinstall-package` still forces the maturin rebuild (spike S4), and the R-stale residual with a pointer to `py/README.md`.

- [ ] **Step 4: Verify the fix**

Repeat Step 2's commands.
Expected AFTER: the `find` count is 0, and the import succeeds.
Run: `python3 ci/affected-graph/cargo_moon_parity.py` → Expected: exit 0, PASS line.

- [ ] **Step 5: Document the residual**

`py/README.md`: a short section "After a Rust change" — `py:test` and `py:typecheck` use the binding that is installed in `py/.venv`, which no Moon task refreshes; to refresh it when no Moon run is active: `(cd py && uv sync --all-packages --reinstall-package paigasus-py-bindings)`. Name SMA-765. If Step 1 showed a lockfile difference that the coordinator accepted, update `.github/CLAUDE.md:188` to match.

- [ ] **Step 6: Commit**

```bash
git add py/packages/paigasus-kernel/moon.yml py/README.md
git commit -m "fix(py): run the kernel test in an isolated uv environment (SMA-765)"
```

---

### Task 3: Whole-branch verification

- [ ] **Step 1:** `python3 ci/affected-graph/cargo_moon_parity.py --self-test` → exit 0; `python3 ci/affected-graph/cargo_moon_parity.py` → exit 0.
- [ ] **Step 2:** `/bin/bash ci/affected-graph/run.sh --negative-control` → exit 0; `/bin/bash ci/affected-graph/run.sh` → exit 0.
- [ ] **Step 3:** `/opt/homebrew/bin/bash ci/ruff/run.sh` → exit 0 (ruff covers the gate's Python; this gate needs bash 4+).
- [ ] **Step 4:** `moon run py:test py:lint py:typecheck --force` → `py:lint` exits 0; record `py:test` and `py:typecheck` results; failures that only name `paigasus_py_bindings` loading are the known local limit (F6), anything else is a finding.
- [ ] **Step 5:** `git diff origin/main --stat` lists only the spec, the plan, and the files of Tasks 1-2.
