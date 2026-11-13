# SMA-768 A14 allow-list Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace A14's uv shell parser with an allow-list of approved `uv` forms, and close the SMA-768 gaps (spec §4).

**Architecture:** All code is in `ci/affected-graph/cargo_moon_parity.py`. The self-test is the function `self_test()` in the same file. There is no separate test file. A "failing test" step adds rows to `self_test()` and runs `--self-test`.

**Tech Stack:** Python 3 (stdlib only), bash 3.2 for `ci/affected-graph/run.sh`, Moon 2.5.3, uv 0.11.16.

**Spec:** `docs/superpowers/specs/2026-11-13-sma-768-a14-uv-gate-design.md`. Read §4 and §5 before every task.

## Global Constraints

- Do not change `SOURCE_STMT_RE`, `task_script_closure`, A8 or A10. The only allowed change there is the G4 error class (Task 1).
- Keep the row format: a per-task row starts with `"{target}: "`. A script row is `"{target}: {path}:{line} {reason}"`. `main()` matches `r.startswith("paigasus-kernel-py:test")`.
- Keep `check_uv_env_isolation(projects, root, allow=None, floor=None)`, `uv_mutations(text)`, `ALLOW_UV_ENV_SCRIPT`, `REQUIRED_UV_ISOLATED_TASKS`, `_uv_script_closure`, `UV_SOURCE_STMT_RE`, `UV_TOKEN_RE` and the findings key `a14`.
- Approved forms (spec §4.2 rule 5): flags `--locked`, `--frozen`, `--isolated`; `--project` with exactly `py`, `ci/release-plan` or `"$HERE"`; `--python` with exactly `'>=3.12'`; `--reinstall-package paigasus-py-bindings` only with `--isolated`; command word matches `[A-Za-z0-9_][A-Za-z0-9_.+/-]*`; `uv build` with no further word; `command -v uv` with only `>/dev/null`, `2>/dev/null` or `2>&1` after it.
- No change to any `moon.yml`, gate script, root `CLAUDE.md` or `ci/CLAUDE.md`.
- Every new rule gets a self-test row. Task 6 proves each row with a mutation.
- Do not install host software. Run every command in the foreground. No `git stash`. No `git commit --amend` followed by `git reset`. Never use `--no-verify`.
- If `git commit` fails with `1Password: failed to fill whole buffer`, stop and report. Do not bypass signing.
- Commit messages: conventional, scope `ci`, end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No line that starts with `#` followed by a number in the body.
- Environment for commands that need moon or uv: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text`. Run `git` in a separate command without that export (the sandbox refuses the mix).
- Run `ci/affected-graph/run.sh` with `/bin/bash` (system 3.2), never with Homebrew bash.

## Review Focus

- A scanned script line that names `uv` in Python heredoc data (`subprocess.run(["uv", "sync"])`) must red. Test in Task 2.
- A `uv` inside `$(...)` in a double-quoted assignment (`PY="$(uv run --locked --project "$HERE" python3 -c '…')"`) must stay green when the form is approved. Test in Task 2 (green row with that exact text).
- A waived line whose physical text ends in ` \` must match its waiver. Test in Task 2.
- A file with a CRLF line end (`uv sync\r`) must still red. Test in Task 2.
- A `uv` token at the very end of a line (`echo uv`) must red, not crash. Test in Task 2.

---

### Task 1: G4 — a script that is not UTF-8 is an infrastructure error

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — `_executable_text` (the `Path(path).read_text()` near line 1198), A14's read in `check_uv_env_isolation` (near line 3614), `self_test()`.

**Interfaces:**
- Produces: `_read_utf8(path) -> str`. Reads with `encoding="utf-8"`. On `UnicodeDecodeError` it raises `MoonOutputError(f"{path}: not valid UTF-8 ({exc.reason} at byte {exc.start}) - the scan cannot read it")`.

- [ ] **Step 1: Write the failing row.** In `self_test()`, in the A14 block, write `bytes([0x75, 0x76, 0x20, 0xff, 0x0a])` to `py/scripts/latin.sh` in `a14_root`. Assert that `a14_run(a14_proj(a14_good + " && bash scripts/latin.sh"))` raises `MoonOutputError` and that `"latin.sh"` and `"not valid UTF-8"` are in its message. Else append `"A14 did not raise MoonOutputError on a script that is not UTF-8"` to `failures`.
- [ ] **Step 2: Run it.** `python3 ci/affected-graph/cargo_moon_parity.py --self-test`. Expected: rc 1, and the output names the new failure (today a `UnicodeDecodeError` escapes, which also counts as a fail).
- [ ] **Step 3: Implement `_read_utf8` and use it in both reads.**
- [ ] **Step 4: Run it.** Same command. Expected: rc 0.
- [ ] **Step 5: Commit.** `fix(ci): report a script that is not UTF-8 as an infrastructure error (SMA-768)`.

### Task 2: The allow-list rule, the floor, and the waivers

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — the A14 block (`:3225-3636`), `ALLOW_UV_ENV_SCRIPT` (`:532-560`), the A14 parser table in `self_test()` (`:6865-6933`).

**Interfaces:**
- Consumes: `_read_utf8` (Task 1).
- Produces:
  - `uv_form_hits(text: str) -> list[tuple[int, str]]`: sorted `(physical line, reason)` for each red `uv` token or environment name in `text`. Replaces `_uv_hits`.
  - `uv_mutations(text: str) -> list[str]`: the reasons of `uv_form_hits`. Same name as today.
  - `_floor_isolated(blob: str) -> bool`: spec §4.2 "The floor".
  - Constants: `UV_APPROVED_FLAGS`, `UV_APPROVED_VALUES` (a dict from option to a tuple of exact values), `UV_BINDINGS_REINSTALL = ("--reinstall-package", "paigasus-py-bindings")`, `UV_COMMAND_WORD_RE`, `UV_REDIRECT_WORDS`, `UV_ENV_NAME_RE` (rule 6: `UV_[A-Z0-9_]+`, `VIRTUAL_ENV`, `--active`, with the word boundaries of spec §4.2 rule 6).

- [ ] **Step 1: Rewrite the parser table** in `self_test()` as spec §5.1 says. Keep the 41 red rows and the 6 green rows that stay green. Change the 14 rows of spec M7 to red with the label prefix `not an approved form: `. Add every new red and green row of spec §5.1 that tests `uv_mutations` directly (the parser-level rows; the check-level rows come in Tasks 3 and 4). Add the five Review Focus rows:
  - red: `python3 - <<'EOF'\nimport subprocess\nsubprocess.run(["uv", "sync"])\nEOF`;
  - green: `PY="$(uv run --locked --project "$HERE" python3 -c 'import sys; print(sys.executable)')"`;
  - red: `uv sync\r`;
  - red: `echo uv`;
  - the ` \` waiver row goes in Step 2.
- [ ] **Step 2: Add the check-level waiver row.** In the A14 block, write `ci/x/cont.sh` with `echo "run 'uv sync' now" \` + newline + `  >&2` + newline. With `allow={("ci/x/cont.sh", "echo \"run 'uv sync' now\" \\"): "PROSE: test"}`, `a14_run` on `a14_good + " && bash ci/x/cont.sh"` gives no row. Without the entry it gives a row that names the physical line of the match: assert that `ci/x/cont.sh:1` is in a row.
- [ ] **Step 3: Run the self-test.** `python3 ci/affected-graph/cargo_moon_parity.py --self-test`. Expected: rc 1. The failures name the new rows (for example `uv lock`, `uv run --python 3.13 pytest`).
- [ ] **Step 4: Implement the rule** (spec §4.2 rules 1-6 and 9) and the floor. Delete the helpers and constants that rule 9 lists. Keep a quote tracker for the floor only. Change `check_uv_env_isolation` to call `uv_form_hits` and to key a waiver by the stripped physical line.
- [ ] **Step 5: Replace `ALLOW_UV_ENV_SCRIPT`.** Run A14 on the real tree (Step 6 command). Add one entry per red line. Start each reason with `PROSE:`, `DATA:` or `EXECUTED:`. Exactly one `EXECUTED:` entry: `ci/version-lockstep/run.sh`, the `( cd "$REPO_ROOT/py" && uv lock >/dev/null ) || die_infra "uv lock failed (site 17)"` line. Its reason says that the line is in `run_write`, that no Moon task passes `--write`, and that the A14 `--write` guard (Task 3) keeps that true. If a red line is an executed call other than this one, stop and report it. Expect about 27 entries (spec M6). Remove the comment-line entry for `python-semantic-release.sh` (`# \`uv sync\` CI step).`).
- [ ] **Step 6: Run the self-test and the real tree.**
  - `python3 ci/affected-graph/cargo_moon_parity.py --self-test` → rc 0.
  - With the moon PATH: `python3 ci/affected-graph/cargo_moon_parity.py` → rc 0. No `a14` row. The A14 negative control does not print `FATAL`.
- [ ] **Step 7: Commit.** `refactor(ci): replace the A14 uv parser with an allow-list of approved forms (SMA-768)`.

### Task 3: Rules 7 and 8 — `UV_*` task env keys and the version-lockstep `--write` guard

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — `check_uv_env_isolation`, `self_test()`.

**Interfaces:**
- Consumes: `check_uv_env_isolation` from Task 2.
- Produces: two new row forms. `"{target}: the task \`env\` sets {name}"` for a key that is `VIRTUAL_ENV` or starts with `UV_`. `"{target}: runs ci/version-lockstep/run.sh with --write, which runs \`uv lock\` (the EXECUTED waiver in ALLOW_UV_ENV_SCRIPT assumes no Moon task does)"`.

- [ ] **Step 1: Write the failing rows.** `a14_run(a14_proj(a14_good, env={"UV_PYTHON": "3.13"}))` gives a row that names `UV_PYTHON`. `a14_run(a14_proj(a14_good + " && bash ci/version-lockstep/run.sh --write"))` gives a row that contains `--write`. Create `ci/version-lockstep/run.sh` in `a14_root` with `#!/bin/bash\ntrue\n` so the reference resolves. Also assert that `a14_good + " && bash ci/version-lockstep/run.sh --self-test"` gives no `--write` row.
- [ ] **Step 2: Run** `--self-test`. Expected: rc 1 with the two new failures.
- [ ] **Step 3: Implement** rule 7 (widen the task `env:` test) and rule 8 (a blob that contains `ci/version-lockstep/run.sh` and a `--write` word in the same invocation line).
- [ ] **Step 4: Run** `--self-test` → rc 0, and the real tree → rc 0, no `a14` row.
- [ ] **Step 5: Commit.** `feat(ci): red UV_ task env keys and a version-lockstep --write task in A14 (SMA-768)`.

### Task 4: G5 — every file A14 reads must reach an input of `repo:affected-smoke`

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — `check_uv_env_isolation`, the `a14_proj` fixture and the A14 rows in `self_test()`.

**Interfaces:**
- Consumes: `AFFECTED_SMOKE_PROJECT`, `AFFECTED_SMOKE_TASK`, `_moon_glob_re` (existing). A13's match at `:3190-3208` is the model. A13's rows must not change.
- Produces: rows `"{path} is read by A14 but matches no input of repo:affected-smoke, so an edit to it alone does not schedule the gate that reads it"` and `"repo:affected-smoke reported no \`inputGlobs\`, so A14 cannot check that an edit to a script it reads schedules it"`.

- [ ] **Step 1: Update the fixture.** `a14_proj` adds a `repo` project with task `affected-smoke`, `task_input_globs = {"affected-smoke": ["ci/**/*", "py/scripts/**/*"]}` and `task_inputs = {"affected-smoke": []}`. Every A14 row of today must keep its meaning.
- [ ] **Step 2: Write the failing rows.**
  - A project `q` with `source_dir="other"` and blob `bash scripts/x.sh`, with `other/scripts/x.sh` present: a row that names `other/scripts/x.sh` and `matches no input`.
  - The same with the smoke globs `["ci/**/*", "py/scripts/**/*", "other/**/*", "!other/scripts/x.sh"]`: still a row.
  - A projects dict without the `repo` project and a non-empty blob: a row that contains `inputGlobs`.
- [ ] **Step 3: Run** `--self-test`. Expected: rc 1 with the three new failures.
- [ ] **Step 4: Implement G5** in `check_uv_env_isolation`. Collect every path A14 reads. Check each one after the scan. Sharing a helper with A13 is your choice. If you share one, A13's self-test rows must stay green unchanged.
- [ ] **Step 5: Run** `--self-test` → rc 0, the real tree → rc 0, no `a14` row.
- [ ] **Step 6: Commit.** `feat(ci): red an A14-read script that does not schedule affected-smoke (SMA-768)`.

### Task 5: Documentation and the `Fix:` text

**Files:**
- Modify: `ci/affected-graph/README.md` (the A14 entry, near line 365), `ci/affected-graph/cargo_moon_parity.py` (the `a14` tuple in `collect_findings`, near line 7272; the comment block above the A14 code).

- [ ] **Step 1: Rewrite the README A14 entry** for spec §4.2, §4.3 and §4.4. Name the six SMA-768 gaps by number with their result: 1 closed (allow-list), 2 closed (rule 6), 3 closed as invalid (already red; row kept), 4 closed (G4), 5 closed (G5), 6 residual R1. List residuals R1-R8. Write in ASD-STE100 style: short sentences, active voice.
- [ ] **Step 2: Rewrite the `a14` `Fix:` text** as spec §6 says. Name `moon.yml` and `T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh` for a G5 row.
- [ ] **Step 3: Run** `--self-test` → rc 0, and the README names the moon CI report file no more often than before the edit (actionlint check 12 reds a file that names it without a marker).
- [ ] **Step 4: Commit.** `docs(ci): describe the A14 allow-list and its residuals (SMA-768)`.

### Task 6: Mutation battery and the full proof

**Files:**
- No repo change. Write the results to `$SCRATCH/sma768-mutations.md`, where `$SCRATCH` is a directory that you create under the system temp dir.

- [ ] **Step 1: Run each mutation of spec §5.2** (23 rows) on a copy of `cargo_moon_parity.py` that you place in `ci/affected-graph/` next to the original: `cp ci/affected-graph/cargo_moon_parity.py ci/affected-graph/m<N>.py`, edit `ci/affected-graph/m<N>.py`, run `python3 ci/affected-graph/m<N>.py --self-test`, then remove the copy. A copy in another directory fails: the self-test finds the repo root from its own path and imports the sibling `task_inputs`. Each mutation must import and run (no `SyntaxError`, no `NameError`). Expected per mutation: rc 1, and the output names the row that spec §5.2 lists. Record mutation, command, rc and the failing row text.
- [ ] **Step 2: Prove the real-tree negative control against the rule.** In the repo file, make mutation 10 (approve `--reinstall-package` without `--isolated`). With the moon PATH, run `python3 ci/affected-graph/cargo_moon_parity.py`. Expected: rc 2 and `FATAL [parity] A14 negative control`. Then `git checkout -- ci/affected-graph/cargo_moon_parity.py` (all work is committed, so this restores the committed file). Run `git status --short` and expect it clean.
- [ ] **Step 3: Run the gates.**
  - `/bin/bash ci/affected-graph/run.sh --negative-control` → rc 0.
  - `/bin/bash ci/affected-graph/run.sh` → rc 0.
  - `/opt/homebrew/bin/bash ci/ruff/run.sh` → rc 0 (needs bash 4+).
- [ ] **Step 4: Report** the results file path and a summary table. No commit.
