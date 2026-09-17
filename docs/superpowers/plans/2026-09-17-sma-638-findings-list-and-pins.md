# SMA-638 — one findings list, a key floor, and two new pins

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make it impossible for `ci/affected-graph/ci_targets.py` to drop a check from its verdict or its report, and close the two unpinned lines the negative control depends on.

**Architecture:** `main()` currently writes three wires per check — the call, the pass-branch condition, and the report tuple — and nothing connects them. The sibling file `cargo_moon_parity.py` already solved this: one `collect_findings()` returning `(key, rows, title)` triples, read by both the verdict and the report, with an `EXPECTED_FINDING_KEYS` tuple pinned in `self_test()`. This plan adopts that shape verbatim, then adds two entries to each of the two call-site pin tables.

**Tech Stack:** Python 3.12 (`ast`-free; plain data), bash 3.2-compatible shell, Moon tasks.

**Spec:** `docs/superpowers/specs/2026-09-17-sma-638-main-wiring-guard-design.md`

## Global Constraints

- Every source file opens with an SPDX header. Both files touched already have one; do not add a second.
- `ci/**/*.py` must pass `repo:ruff-ci`: rules `E,F,W,I,N,UP,B,A,C4,SIM,TCH,RUF`, `line-length = 200`, `target-version = "py312"`, `ignore = ["E501"]`.
- `ci/**/*.sh` must stay **bash 3.2 compatible**. No `mapfile`, no associative arrays, no `${var^^}`.
- Run `ci/affected-graph/*` gates with **bash 3.2**, because Homebrew bash 5.3.15 deadlocks on this machine. Use a **bash-only shim**, not a bare `/bin` prefix: `mkdir -p /tmp/bashshim && ln -sf /bin/bash /tmp/bashshim/bash`, then `export PATH="/tmp/bashshim:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. MEASURED 2026-09-17: prepending `/bin:/usr/bin` also downgrades `python3` to 3.9.6, and `cargo_moon_parity.py` imports `tomllib` (stdlib only since 3.11), so `moon run repo:affected-smoke` dies with `ModuleNotFoundError: No module named 'tomllib'` and prints `negative-control FAILED` — which reads as a gate failure and is not one. Running `python3 ci/affected-graph/ci_targets.py` directly is unaffected either way.
- `export PROTO_REPORTER=text` before any command whose stdout you capture.
- `repo:actionlint` **cannot be run to completion locally on this machine.** Task 4 edits it. Verify Task 4 by running its self-test function only, and state in the commit that CI is the proof.
- Commit subjects must be lowercase after the `type(scope):` prefix — commitlint rejects a subject starting with an uppercase token. Allowed scopes: `rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace`.
- Do not put a bare `#NNN` line or a `token: value` line in a commit **body**; commitlint's `footer-leading-blank` rejects it.
- End every commit message with:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_017MQim8V5eDw39pkaFR4Zt3
  ```

---

## File Structure

| File | Responsibility after this plan |
|---|---|
| `ci/affected-graph/ci_targets.py` | Gains `EXPECTED_FINDING_KEYS` and `collect_findings()`. `main()` shrinks to: read inputs, call `collect_findings`, derive verdict and report from it. `self_test()` gains three floor rows and loses the `main_src.count(...)` block. `RUN_SH_CALL_SITES` gains two entries. |
| `ci/actionlint/run.sh` | `T_AFFECTED_GRAPH_CALL_SITES` gains the same two entries; `affected_graph_wiring_self_test` gains two fixtures. |
| `ci/next-env/run.sh` | The `dirs` liveness loop is guarded on array length. |
| `ci/affected-graph/README.md` | Two stale statements corrected; the new floor's limits recorded. |
| `CLAUDE.md` | One entry for the floor and the two new pins. |

---

### Task 1: `collect_findings` and `EXPECTED_FINDING_KEYS`

**Files:**
- Modify: `ci/affected-graph/ci_targets.py` — add `EXPECTED_FINDING_KEYS` and `collect_findings()`; rewrite `main()`'s body from `dead = check_reverse(...)` (line 3551) through `return 1` (line 3772)
- Test: `ci/affected-graph/ci_targets.py`'s own `self_test()` (no separate test file exists in this repo)

**Interfaces:**
- Consumes: the eleven existing `check_*` functions, unchanged. Do not alter any of their signatures or bodies.
- Produces: `collect_findings(tasks, t_targets, raw_tasks, scripts, ci_yml, doc_targets, region, tailwind_apps, sh)` → `list[tuple[str, list, str]]` in report order; `EXPECTED_FINDING_KEYS: tuple[str, ...]` of length 23. Task 2 depends on both.

The 23 keys, in the exact order the current report tuple lists them (`ci_targets.py:3584-3767`):

| # | key | rows expression, copied verbatim from the current tuple |
|---|---|---|
| 1 | `floor` | `floor` |
| 2 | `t-missing` | `[":" + name for name in missing]` |
| 3 | `t-unexpected` | `[":" + name for name in unexpected]` |
| 4 | `t-exempt-noreason` | `bad_exempt` |
| 5 | `t-exempt-stale` | `stale_exempt` |
| 6 | `t-dead` | `[":" + name for name in dead]` |
| 7 | `docs` | `doc_problems` |
| 8 | `call-sites` | `missing_sites` |
| 9 | `ci-invocation` | `bad_invocation` |
| 10 | `gate-inputs` | `bad_gate_inputs` |
| 11 | `generate-inputs` | `bad_generate_inputs` |
| 12 | `selfsched-unregistered` | `[":" + name for name in unregistered_self_scheduled]` |
| 13 | `selfsched-exempt-noreason` | `bad_coverage_exempt` |
| 14 | `selfsched-exempt-stale` | `stale_coverage_exempt` |
| 15 | `pairing-unpinned` | `[":" + name for name in pairing_unpinned]` |
| 16 | `pairing-exempt-noreason` | `pairing_bad_exempt` |
| 17 | `pairing-exempt-stale` | `pairing_stale_exempt` |
| 18 | `pairing-both` | `[":" + name for name in pairing_both]` |
| 19 | `pairing-orphan-globs` | `[":" + name for name in pairing_orphan_globs]` |
| 20 | `tw-unregistered` | `tw_unregistered` |
| 21 | `tw-missing-lines` | `tw_missing_lines` |
| 22 | `tw-stale` | `tw_stale` |
| 23 | `tw-no-project` | `tw_no_project` |

**Each title string moves verbatim.** Do not reword, re-wrap or re-indent any of the 23 title strings. They are the operator-facing remediation text and several are quoted in the README.

- [ ] **Step 1: Write the failing self-test rows**

Insert into `self_test()`, immediately before the existing `main_src = inspect.getsource(main)` block at line 3116 (Task 2 deletes that block; leaving it here for now keeps the two tasks independently revertible):

```python
    # SMA-638. The findings list is what makes "collected but not judged" and "judged but not
    # reported" impossible, and EXPECTED_FINDING_KEYS is what stops the list itself being
    # shrunk. cargo_moon_parity.py:2290-2337 records the measurement that a name-based guard is
    # NOT enough: three deletions from its findings list left --self-test green with a real
    # assertion gone. Arity first, so a shrunk list says so plainly, then the exact sequence.
    if not EXPECTED_FINDING_KEYS:
        failures.append("EXPECTED_FINDING_KEYS is empty — the findings floor would assert nothing")
    # `{"repo": {}}` and `""`, not `{}` and `None`: MEASURED — `check_forward` raises
    # MoonOutputError("'moon query tasks' reported no 'repo' project") on an empty `tasks`, and
    # `check_docs` does `flag not in region`, which raises TypeError on None. `parse_doc_targets`
    # always returns a str, so `""` is the type-correct empty case.
    _fk_findings = collect_findings(
        {"repo": {}}, [], {}, {}, "", [], "", [], dict.fromkeys(_CALL_SITE_SOURCE_KEYS, ""),
    )
    if len(_fk_findings) != len(EXPECTED_FINDING_KEYS):
        failures.append(
            f"collect_findings returned {len(_fk_findings)} entries, expected "
            f"{len(EXPECTED_FINDING_KEYS)} — a check was added or dropped without updating "
            f"EXPECTED_FINDING_KEYS"
        )
    _fk_keys = tuple(key for key, _, _ in _fk_findings)
    if _fk_keys != EXPECTED_FINDING_KEYS:
        failures.append(
            f"collect_findings reported {_fk_keys}, expected {EXPECTED_FINDING_KEYS} — a check "
            f"was dropped, added or reordered in the findings list"
        )
```

- [ ] **Step 2: Run the self-test to verify it fails**

```bash
export PATH="/bin:/usr/bin:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py --self-test
```

Expected: FAIL with `NameError: name 'EXPECTED_FINDING_KEYS' is not defined`.

- [ ] **Step 3: Add `EXPECTED_FINDING_KEYS` and the source-key tuple**

Place both directly above `def main():` (line 3484):

```python
# SMA-638. The membership floor for collect_findings' list. `self_test` asserts BOTH its arity
# and its exact sequence, because the restructure below removes "judged but not reported" as a
# possible state and leaves exactly one: a triple deleted from the list outright. A name-based
# guard cannot close that — cargo_moon_parity.py:2290 measured three such deletions passing —
# so this pins the LIST. Re-baseline it deliberately when a check is genuinely added or removed.
EXPECTED_FINDING_KEYS = (
    "floor", "t-missing", "t-unexpected", "t-exempt-noreason", "t-exempt-stale", "t-dead",
    "docs", "call-sites", "ci-invocation", "gate-inputs", "generate-inputs",
    "selfsched-unregistered", "selfsched-exempt-noreason", "selfsched-exempt-stale",
    "pairing-unpinned", "pairing-exempt-noreason", "pairing-exempt-stale", "pairing-both",
    "pairing-orphan-globs", "tw-unregistered", "tw-missing-lines", "tw-stale", "tw-no-project",
)

# The seven shell sources check_self_invocation reads, keyed so collect_findings' signature does
# not grow eight positional parameters that a caller could silently transpose.
_CALL_SITE_SOURCE_KEYS = (
    "run", "actionlint", "release_parity", "workflow_credentials", "release_plan", "ruff",
    "next_public_free",
)
```

- [ ] **Step 4: Add `collect_findings`**

Insert directly below `EXPECTED_FINDING_KEYS`, above `def main():`. Move the eleven check calls out of `main()` into it, and move all 23 `(rows, title)` pairs in verbatim, each gaining its key from the table above:

```python
def collect_findings(tasks, t_targets, raw_tasks, scripts, ci_yml, doc_targets, region,
                     tailwind_apps, sh):
    """Every assertion's rows, as `(key, rows, title)`, in report order.

    ONE list, used for BOTH the pass/fail verdict and the report. They used to be written
    separately, so a check folded into one and not the other was a green no-op — the defect
    SMA-638 reports for check_tailwind_guard_invocations, which applied to all eleven checks.
    That restructure is necessary but NOT sufficient on its own: what makes the list itself hard
    to shrink is `EXPECTED_FINDING_KEYS` above, asserted by `self_test`. This mirrors
    cargo_moon_parity.py's collect_findings, in this same directory, deliberately.

    Raises GateAssertionError (rc 1) and the INFRA_ERRORS members its checks raise (rc 2), so
    `main` keeps the call inside its try and maps them as it always has.
    """
    floor = check_floor(tasks)
    missing, unexpected, bad_exempt, stale_exempt = check_forward(tasks, t_targets)
    scripts_ = scripts
    bad_gate_inputs = check_gate_inputs(raw_tasks)
    bad_generate_inputs = check_contracts_generate_inputs(raw_tasks)
    tw_unregistered, tw_missing_lines, tw_stale, tw_no_project = (
        check_tailwind_guard_invocations(tailwind_apps, raw_tasks)
    )
    dead = check_reverse(tasks, t_targets)
    doc_problems = check_docs(t_targets, doc_targets, region)
    missing_sites = check_self_invocation(
        sh["run"], scripts_, sh["actionlint"], sh["release_parity"],
        sh["workflow_credentials"], sh["release_plan"], sh["ruff"], sh["next_public_free"],
    )
    bad_invocation = check_invocation(ci_yml)
    unregistered_self_scheduled, bad_coverage_exempt, stale_coverage_exempt = (
        check_self_scheduled_coverage(scripts_)
    )
    pairing_unpinned, pairing_bad_exempt, pairing_stale_exempt, pairing_both, pairing_orphan_globs = (
        check_registry_pairing()
    )

    return [
        ("floor", floor,
         "A task this gate REQUIRES to be present is absent from the parsed `repo` set, so the\n"
         "    comparison below may be between two empty sets and assert nothing.\n"
         "    Fix: if the task was genuinely renamed or removed, update REQUIRED_REPO_TASKS in\n"
         "    ci/affected-graph/ci_targets.py. Otherwise the project filter or moon's output\n"
         "    shape has changed — investigate before touching anything else."),
        ("t-missing", [":" + name for name in missing],
         "A CI-eligible `repo:*` task is NOT in ci.yml's `T=(...)` array, so it does not run in\n"
         "    CI at all — it passes locally and silently does not exist on any PR (SMA-541).\n"
         "    Fix: append `:<name>` to `T` in .github/workflows/ci.yml AND to the command\n"
         "    between the <!-- ci-targets:begin/end --> markers in CLAUDE.md."),
        # ... entries 3 through 23, each the existing (rows, title) pair from
        # ci_targets.py:3596-3766 with its key from the table prefixed. Copy the title strings
        # byte for byte.
    ]
```

> **Implementer note.** Entries 3-23 are a mechanical transcription of the existing tuple at
> `ci_targets.py:3596-3766`. Take each `(rows, title)` pair in file order, prefix the key from
> the table in this task, and change nothing else. After the move, `git diff` should show the
> title strings as moved, not modified. If any title line differs beyond indentation, you have
> introduced a defect — revert that hunk and redo it.

`scripts_` is a local alias only to keep the diff of the moved `check_self_invocation` and
`check_self_scheduled_coverage` calls minimal; if ruff's `F841` or a reviewer objects, use
`scripts` directly and drop the alias.

- [ ] **Step 5: Rewrite `main()`'s tail**

Replace everything in `main()` from `tw_unregistered, tw_missing_lines, ... = (` (line 3539) through the final `return 1` (line 3772) with:

```python
        findings = collect_findings(
            tasks, t_targets, raw_tasks, scripts, ci_yml, doc_targets, region, tailwind_apps,
            {
                "run": run_sh, "actionlint": actionlint_sh,
                "release_parity": release_parity_sh,
                "workflow_credentials": workflow_credentials_sh,
                "release_plan": release_plan_sh, "ruff": ruff_sh,
                "next_public_free": next_public_free_sh,
            },
        )
    except GateAssertionError as exc:
        # An authorial mistake, NOT a broken tool: rc 1 so run.sh records a red suite instead of
        # aborting the whole affected-graph guard and losing every other assertion's output (D2).
        print(f"FAIL  [ci-targets] {exc}", file=sys.stderr)
        return 1
    except INFRA_ERRORS as exc:
        print(f"FATAL [ci-targets] could not read the inputs: {exc}", file=sys.stderr)
        return 2

    if not any(rows for _, rows, _ in findings):
        print(
            f"PASS  {'ci-targets':<18} -> {len(t_targets)} targets: every CI-eligible repo task is "
            "in ci.yml's T, every entry resolves, CLAUDE.md mirrors it"
        )
        return 0

    print("FAIL  [ci-targets] ci.yml's moon ci target array is out of sync", file=sys.stderr)
    for _, rows, title in findings:
        if rows:
            print(f"  {title}", file=sys.stderr)
            for row in rows:
                print(f"      {row}", file=sys.stderr)
    return 1
```

Delete the now-duplicated check calls that used to sit between the `except` clauses and the old
condition (`dead = check_reverse(...)` at line 3551 through `check_registry_pairing()` at 3563)
— `collect_findings` owns them now.

This moves six checks that previously ran *after* the `try` to *inside* it. That is an
improvement, and deliberate: a `GateAssertionError` from `check_reverse` or `check_docs`
previously escaped `main()` as an uncaught traceback. It now takes the documented rc-1 path with
its `FAIL [ci-targets]` prefix. Record this in the commit body.

- [ ] **Step 6: Run the self-test and expect it to be RED**

```bash
python3 ci/affected-graph/ci_targets.py --self-test
```

Expected: **rc 1**, with exactly five rows and nothing else:

```
main() wiring[pairing_unpinned]: found 0 occurrence(s) in main()'s source, want at least 3 ...
main() wiring[pairing_bad_exempt]: ...
main() wiring[pairing_stale_exempt]: ...
main() wiring[pairing_both]: ...
main() wiring[pairing_orphan_globs]: ...
```

**This is correct and expected.** The superseded `main_src.count(...)` block counts those five
names in `main()`'s source; this task moves them into `collect_findings`, so the count drops to
0 and the block reds. **Task 2 is what returns the suite to green** — it is load-bearing, not
cleanup. Do not try to fix this here, and do not read the branch as green between the two
commits.

Confirm the three NEW assertions added in Step 1 do **not** appear in the failure list. If any
does, that is a real defect in this task.

- [ ] **Step 7: Run the real gate to verify the verdict and report still work**

```bash
export PATH="/bin:/usr/bin:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
python3 ci/affected-graph/ci_targets.py
```

Expected: the same `PASS  ci-targets  -> NN targets: ...` line the gate printed before the
change, rc 0.

- [ ] **Step 8: Prove the floor bites**

Delete the `("tw-no-project", tw_no_project, ...)` triple from `collect_findings`' returned list,
then:

```bash
python3 ci/affected-graph/ci_targets.py --self-test
```

Expected: FAIL, naming both `collect_findings returned 22 entries, expected 23` and the key
sequence mismatch. **Restore the triple by re-adding it, not by `git checkout --`** — a checkout
would also revert the rest of this task's uncommitted work.

Re-run the self-test and confirm rc 0 before continuing.

- [ ] **Step 9: Lint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py ruff check --config py/pyproject.toml -- ci/affected-graph/ci_targets.py
```

Expected: `All checks passed!`

- [ ] **Step 10: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -F <message file>
```

Subject: `refactor(ci): build ci_targets' verdict and report from one findings list (SMA-638)`

---

### Task 2: Delete the superseded `main_src.count(...)` block

**Files:**
- Modify: `ci/affected-graph/ci_targets.py:3116-3127` (delete)

**Interfaces:**
- Consumes: `EXPECTED_FINDING_KEYS` and `collect_findings` from Task 1.
- Produces: nothing new.

This is its own task because it is the step a reviewer could reject while approving Task 1: it
removes a working control, and its safety depends entirely on Task 1's floor being real.

- [ ] **Step 1: Confirm the starting state is RED**

```bash
python3 ci/affected-graph/ci_targets.py --self-test
```

Expected: **rc 1**, with exactly five `main() wiring[pairing_*]` rows and nothing else — the
state Task 1 leaves behind. This task is what returns the suite to green, so Step 4 is the real
proof of the task.

(An earlier revision of this plan said "confirm the old block currently passes". That was wrong:
the block cannot pass once Task 1 has moved the five `pairing_*` names into `collect_findings`.)

- [ ] **Step 2: Delete the block**

Remove these lines and the comment paragraph directly above them (`ci_targets.py:3107-3127`):

```python
    main_src = inspect.getsource(main)
    for _name in (
        "pairing_unpinned", "pairing_bad_exempt", "pairing_stale_exempt", "pairing_both",
        "pairing_orphan_globs",
    ):
        _count = main_src.count(_name)
        if _count < 3:
```

...through the end of that `if`'s `failures.append(...)` call.

- [ ] **Step 3: Check whether `inspect` is still used**

```bash
grep -n "inspect\." ci/affected-graph/ci_targets.py
```

If this returns nothing, delete `import inspect` (line 20) — ruff's `F401` will otherwise red.
If it returns hits (the file uses `inspect.signature` elsewhere for default-pinning), leave the
import.

- [ ] **Step 4: Run the self-test**

```bash
python3 ci/affected-graph/ci_targets.py --self-test
```

Expected: `ci-targets self-test OK`, rc 0.

- [ ] **Step 5: Prove the replacement covers what was deleted**

The deleted block's one unique property was catching a *whole check* being removed. Prove the
floor covers it: delete all five `pairing-*` triples from `collect_findings`' list **and** the
`check_registry_pairing()` call above them, then run:

```bash
python3 ci/affected-graph/ci_targets.py --self-test
```

Expected: FAIL, naming `collect_findings returned 18 entries, expected 23`. Restore by re-adding
the call and the five triples, then confirm rc 0.

- [ ] **Step 6: Lint and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py ruff check --config py/pyproject.toml -- ci/affected-graph/ci_targets.py
git add ci/affected-graph/ci_targets.py
git commit -F <message file>
```

Subject: `refactor(ci): drop the substring wiring count the findings floor supersedes (SMA-638)`

The body must state which property was deleted and which assertion now covers it.

---

### Task 3: Pin the negative control's flag parse in `RUN_SH_CALL_SITES`

**Files:**
- Modify: `ci/affected-graph/ci_targets.py:439-449` (`RUN_SH_CALL_SITES`) and its `wired` fixture at `:2367-2374`

**Interfaces:**
- Consumes: nothing from Tasks 1-2.
- Produces: two new pinned strings that Task 4 must mirror **byte for byte**:
  - `[ "${1-}" = "--negative-control" ] && NEGATIVE=1`
  - `if [ "$NEGATIVE" = 1 ]; then`

- [ ] **Step 1: Write the failing negative fixtures**

Add to `self_test()`, directly after the existing `silenced` fixture at `ci_targets.py:2559-2561`:

```python
    # SMA-638. The two call sites above are reachable ONLY if the flag parse and the branch guard
    # survive. Deleting the flag parse leaves NEGATIVE at its initialised 0, so the whole
    # --negative-control branch is skipped, run.sh falls through to run_suite and the gate exits 0
    # having run the real suite twice and proved nothing. CLAUDE.md records that exact bypass as
    # MEASURED for repo:release-parity, which closed it by pinning its own flag parse.
    no_flag_parse = wired.replace(
        '[ "${1-}" = "--negative-control" ] && NEGATIVE=1\n', ""
    )
    if not check_self_invocation(no_flag_parse, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials, wired_release_plan, wired_ruff, wired_next_public_free):
        failures.append("check_self_invocation: missed a deleted --negative-control flag parse")
    no_negative_guard = wired.replace('if [ "$NEGATIVE" = 1 ]; then\n', "")
    if not check_self_invocation(no_negative_guard, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials, wired_release_plan, wired_ruff, wired_next_public_free):
        failures.append("check_self_invocation: missed a deleted NEGATIVE branch guard")
```

- [ ] **Step 2: Run the self-test to verify it fails**

```bash
python3 ci/affected-graph/ci_targets.py --self-test
```

Expected: FAIL with both new `failures.append` messages — the `wired` fixture does not yet
contain the two lines, so `.replace()` is a no-op and `check_self_invocation` returns clean.

- [ ] **Step 3: Extend the `wired` fixture**

Replace `ci_targets.py:2367-2374` with:

```python
    wired = (
        # Load-bearing: with the function DEFINITION present, `no_call` below still contains
        # the bare name `assert_ci_targets`, so a name-only RUN_SH_CALL_SITES entry would
        # survive deleting the call. Dropping this line silently de-fangs that assertion.
        'assert_ci_targets() {\n  :\n}\n'
        '[ "${1-}" = "--negative-control" ] && NEGATIVE=1\n'
        '  assert_ci_targets || SUITE_RC=1\n'
        'if [ "$NEGATIVE" = 1 ]; then\n'
        '  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1\n'
    )
```

- [ ] **Step 4: Add the two entries to `RUN_SH_CALL_SITES`**

Append to the tuple at `ci_targets.py:439-449`, after the existing `--self-test` entry:

```python
    # SMA-638. The two entries above pin the CALLS; these two pin the only path that REACHES the
    # --self-test call. `run.sh` initialises NEGATIVE=0, so deleting the flag parse leaves the
    # --negative-control branch unentered: run.sh falls through to run_suite and exits 0 having
    # run the real suite twice. Both pinned strings are SUBSTRING-matched like their neighbours,
    # so a commented-out copy still satisfies them — recorded in ci/affected-graph/README.md,
    # not closed here. ci/actionlint/run.sh's T_AFFECTED_GRAPH_CALL_SITES carries the same two.
    '[ "${1-}" = "--negative-control" ] && NEGATIVE=1',
    'if [ "$NEGATIVE" = 1 ]; then',
```

- [ ] **Step 5: Run the self-test to verify it passes**

```bash
python3 ci/affected-graph/ci_targets.py --self-test
```

Expected: `ci-targets self-test OK`, rc 0.

- [ ] **Step 6: Run the real gate against the real `run.sh`**

```bash
export PATH="/bin:/usr/bin:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
python3 ci/affected-graph/ci_targets.py
```

Expected: rc 0. If it reports the two sites missing, the pinned strings do not match
`ci/affected-graph/run.sh:28` and `:714` byte for byte — read those two lines and copy them
exactly rather than adjusting the comparison.

- [ ] **Step 7: Lint and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py ruff check --config py/pyproject.toml -- ci/affected-graph/ci_targets.py
git add ci/affected-graph/ci_targets.py
git commit -F <message file>
```

Subject: `fix(ci): pin the negative control's flag parse and branch guard (SMA-638)`

---

### Task 4: Mirror the two pins into `ci/actionlint/run.sh`

**Files:**
- Modify: `ci/actionlint/run.sh:2051-2054` (`T_AFFECTED_GRAPH_CALL_SITES`) and `affected_graph_wiring_self_test` at `:3126-3195`

**Interfaces:**
- Consumes: the two exact strings Task 3 pinned. They must match byte for byte — the two tables
  are independent copies and nothing asserts they agree.
- Produces: nothing later tasks need.

- [ ] **Step 1: Write the failing fixtures**

Add to `affected_graph_wiring_self_test`, after the `selftest_swallowed` fixture at
`ci/actionlint/run.sh:3179-3186`:

```bash
  # SMA-638. The two call sites above are reachable only through the flag parse and the branch
  # guard. Deleting the flag parse leaves NEGATIVE at 0, the --negative-control branch is never
  # entered, and run.sh falls through to the real suite having proved nothing — the bypass
  # CLAUDE.md records as MEASURED for repo:release-parity.
  local no_flag_parse='assert_ci_targets() {
  :
}
  assert_ci_targets || SUITE_RC=1
if [ "$NEGATIVE" = 1 ]; then
  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1
'
  expect_wiring 'the --negative-control flag parse deleted fires' \
    'missing [ "${1-}" = "--negative-control" ] && NEGATIVE=1' "$no_flag_parse"

  local no_negative_guard='assert_ci_targets() {
  :
}
[ "${1-}" = "--negative-control" ] && NEGATIVE=1
  assert_ci_targets || SUITE_RC=1
  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1
'
  expect_wiring 'the NEGATIVE branch guard deleted fires' \
    'missing if [ "$NEGATIVE" = 1 ]; then' "$no_negative_guard"
```

Also extend the healthy control `wired` fixture at `:3131-3137` so it contains both new lines —
otherwise it starts reporting them as missing and every other row in this table fails for the
wrong reason:

```bash
  local wired='assert_ci_targets() {
  :
}
[ "${1-}" = "--negative-control" ] && NEGATIVE=1
  assert_ci_targets || SUITE_RC=1
if [ "$NEGATIVE" = 1 ]; then
  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1
'
```

The `both call sites missing fires both, independently` row at `:3192-3196` now expects four
`missing` lines rather than two. Update its expected string to list all four, in
`T_AFFECTED_GRAPH_CALL_SITES` order.

- [ ] **Step 2: Run this self-test function alone to verify it fails**

The full gate cannot run locally. Drive the one function:

```bash
export PATH="/bin:/usr/bin:$PATH"
/bin/bash -c 'set -uo pipefail; source ci/actionlint/run.sh --self-test' 2>&1 | grep -i "affected-graph-wiring" | head
```

If sourcing the script proves impractical (it executes at load), instead run the whole
`--self-test` path with a timeout and read only the affected-graph-wiring rows:

```bash
/bin/bash ci/actionlint/run.sh --self-test 2>&1 | grep -i "affected-graph-wiring\|wiring self-test" | head -20
```

Expected: the two new rows report a missing verdict that the table does not yet produce.

> If neither invocation completes — CLAUDE.md records that no local bash runs this gate to
> completion on this machine — record that fact in the task notes and rely on CI. Do **not**
> report the step as passing.

- [ ] **Step 3: Add the two entries**

Append to `T_AFFECTED_GRAPH_CALL_SITES` at `ci/actionlint/run.sh:2051-2054`:

```bash
  # SMA-638. Copied VERBATIM from ci_targets.py's RUN_SH_CALL_SITES, like the two above. These
  # pin the path that REACHES the --self-test call: run.sh initialises NEGATIVE=0, so deleting
  # the flag parse skips the whole --negative-control branch and the gate exits 0 having run the
  # real suite twice. Substring-matched, so a commented-out copy still satisfies them.
  '[ "${1-}" = "--negative-control" ] && NEGATIVE=1'
  'if [ "$NEGATIVE" = 1 ]; then'
```

- [ ] **Step 4: Re-run the self-test rows**

Same command as Step 2. Expected: the affected-graph-wiring rows pass.

- [ ] **Step 5: Verify shellcheck is clean on the edited region**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py shellcheck -s bash ci/actionlint/run.sh 2>&1 | head -20
```

Expected: no new findings attributable to the added lines. Pre-existing findings elsewhere in
the file are not this task's scope.

- [ ] **Step 6: Commit**

```bash
git add ci/actionlint/run.sh
git commit -F <message file>
```

Subject: `fix(ci): mirror the negative-control pins into actionlint check 8c (SMA-638)`

The body must state that `repo:actionlint` cannot be run to completion locally on this machine,
and that CI is the proof for this task.

---

### Task 5: Guard the `dirs` loop on array length

**Files:**
- Modify: `ci/next-env/run.sh:140-146`

**Interfaces:**
- Consumes: nothing.
- Produces: nothing.

- [ ] **Step 1: Reproduce the defect in isolation**

```bash
/bin/bash -c 'set -u; dirs=(); n=0; for d in "${dirs[@]:-}"; do n=$((n+1)); done; echo "iterations: $n"'
```

Expected: `iterations: 1` — the empty array iterates once with an empty `d`. This is the defect.

```bash
/bin/bash -c 'set -u; dirs=(); n=0; if [ "${#dirs[@]}" -gt 0 ]; then for d in "${dirs[@]}"; do n=$((n+1)); done; fi; echo "iterations: $n"'
```

Expected: `iterations: 0` — the fix, proven on system bash 3.2 under `set -u`.

- [ ] **Step 2: Apply the fix**

Replace `ci/next-env/run.sh:140-146`:

```bash
missing=()
for d in "${dirs[@]:-}"; do
  found=0
  for a in "${apps[@]}"; do
    [ "$a" = "$d" ] && found=1
  done
  [ "$found" -eq 1 ] || missing+=("$d")
done
```

with:

```bash
missing=()
# `"${dirs[@]:-}"` iterates ONCE with an empty `d` when `dirs` is empty, which printed a blank
# row under the heading below. Unreachable today — `apps` needs a package.json AND a config, and
# the exit above fires when `apps` is empty, so reaching here proves `dirs` is non-empty too —
# but the `:-` idiom is wrong regardless, and a future reordering of the two blocks would make it
# live. The length guard is the bash-3.2-safe form under `set -u`; lines 126 and 148 use it too.
if [ "${#dirs[@]}" -gt 0 ]; then
  for d in "${dirs[@]}"; do
    found=0
    for a in "${apps[@]}"; do
      [ "$a" = "$d" ] && found=1
    done
    [ "$found" -eq 1 ] || missing+=("$d")
  done
fi
```

- [ ] **Step 3: Run the gate**

```bash
export PATH="/bin:/usr/bin:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
/bin/bash ci/next-env/run.sh
echo "rc=$?"
```

Expected: rc 0, with the two `next-env gate: ts/apps/<app>/next-env.d.ts matches` lines, exactly
as the pre-change baseline printed.

- [ ] **Step 4: Prove the liveness assertion still fires**

Temporarily create a `package.json`-bearing app directory with no config:

```bash
mkdir -p ts/apps/zz-probe && printf '{"name":"zz-probe"}\n' > ts/apps/zz-probe/package.json
/bin/bash ci/next-env/run.sh; echo "rc=$?"
rm -rf ts/apps/zz-probe
```

Expected: rc 2, and the missing-config heading followed by `  ts/apps/zz-probe` — a real
directory name, never a blank line. Confirm the directory is removed afterwards with
`git status --short`.

- [ ] **Step 5: shellcheck and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py shellcheck -s bash ci/next-env/run.sh
git add ci/next-env/run.sh
git commit -F <message file>
```

Subject: `fix(ci): guard next-env's dirs loop on array length (SMA-638)`

---

### Task 6: Correct the README and record the change in CLAUDE.md

**Files:**
- Modify: `ci/affected-graph/README.md:350-354`
- Modify: `CLAUDE.md` (append to the `repo:affected-smoke` / `ci_targets.py` cluster)

**Interfaces:**
- Consumes: `EXPECTED_FINDING_KEYS` from Task 1 and the two pins from Tasks 3-4.
- Produces: nothing.

- [ ] **Step 1: Correct the two stale README statements**

`ci/affected-graph/README.md:350-354` currently reads:

```
  The function that asserts this pairing, `check_registry_pairing`, is not called from `main()` — it is
  exercised only via the `--self-test` path, which CI reaches through
  `repo:affected-smoke` → `ci/affected-graph/run.sh --negative-control` → run.sh:404's
  `python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1`, a line pinned by
  `RUN_SH_CALL_SITES` above and mirrored by `ci/actionlint/run.sh`'s check 8c.
```

Both facts are wrong. `main()` does call `check_registry_pairing` (`ci_targets.py:3561-3562`,
and after Task 1 it is called from `collect_findings`), and the line is `run.sh:760`, not 404.
Rewrite the passage to say that the pairing check runs on the real gate path *and* the
`--self-test` path, and cite `run.sh:760`.

- [ ] **Step 2: Add a Limitations note to the README**

Record, in the README's own voice:

- `EXPECTED_FINDING_KEYS` proves membership, not semantics. A key whose `rows` are always empty
  satisfies it.
- Both pin tables are substring pins (`ci_targets.py:1669` uses `site not in run_sh_text`;
  `ci/actionlint/run.sh:2084` uses `grep -qF`), so a commented-out copy of a pinned line still
  satisfies them.
- `self_test()`'s own `if failures:` report guard (`ci_targets.py:3475`) is unpinned. It is
  closable the way `RUFF_SH_CALL_SITES` closes the same shape for `ci/ruff/run.sh`, and is left
  open as scope rather than as an inherent limit.

- [ ] **Step 3: Add the CLAUDE.md entry**

Append one bullet to the cluster describing `repo:affected-smoke` and `ci_targets.py`. It must
state:

- `ci_targets.py`'s verdict and report are both built from `collect_findings`, so a check wired
  into one and not the other cannot exist; `EXPECTED_FINDING_KEYS` (23 keys) is the floor that
  stops the list being shrunk, and it must be re-baselined deliberately when a check is added or
  removed.
- `RUN_SH_CALL_SITES` and `T_AFFECTED_GRAPH_CALL_SITES` now carry **four** entries each, not two:
  the two calls plus `run.sh`'s flag parse and `NEGATIVE` branch guard. Both copies must be
  edited together; nothing asserts they agree.

Do **not** mention `ciReport.json` anywhere in the new text: `ci/actionlint/run.sh` check 12
requires a `<!-- moon-diagnosis:ok -->` marker on any file naming it, and CLAUDE.md's existing
marked block must not gain a second, unmarked mention.

- [ ] **Step 4: Verify the CLAUDE.md target-list markers are untouched**

```bash
grep -c "ci-targets:begin\|ci-targets:end" CLAUDE.md
```

Expected: `2`. A second copy of either marker reds `repo:affected-smoke` (SMA-541).

- [ ] **Step 5: Run the full gate**

```bash
export PATH="/bin:/usr/bin:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
python3 ci/affected-graph/ci_targets.py --self-test && python3 ci/affected-graph/ci_targets.py
```

Expected: both rc 0. The second run parses CLAUDE.md, so a malformed edit reds here.

- [ ] **Step 6: Commit**

```bash
git add ci/affected-graph/README.md CLAUDE.md
git commit -F <message file>
```

Subject: `docs(ci): correct the affected-graph README and record the findings floor (SMA-638)`

---

## Final verification

Run after Task 6, before opening the PR:

```bash
export PATH="/bin:/usr/bin:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
moon run repo:affected-smoke --force
/bin/bash ci/next-env/run.sh
```

Then, with Homebrew bash on `PATH` (ruff's gate needs bash 4+):

```bash
export PATH="/opt/homebrew/bin:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:ruff-ci --force
```

`repo:actionlint` cannot be verified locally. Say so explicitly in the PR body rather than
implying the local run covered it.

## Self-review notes

- **Spec coverage.** Spec §3.1-3.2 → Task 1. §3.3 → Task 1 steps 3, 8 and Task 2 step 5. §3.4 →
  Task 2. §3.5 → Task 1 step 1. §4 → Tasks 3 and 4. §5-5.1 → Task 5. §5.2 → no task, deliberately
  (the spec declines to change it). §6 → Task 6. §7-8 → Task 6 step 2.
- **Type consistency.** `collect_findings` returns `list[tuple[str, list, str]]` in Task 1 and is
  consumed as `for _, rows, title in findings` in the same task and as
  `tuple(key for key, _, _ in ...)` in its self-test row. `EXPECTED_FINDING_KEYS` is a 23-tuple of
  `str` everywhere it appears.
- **Known gap.** Task 1 step 4 transcribes 21 of the 23 triples rather than printing them in
  full. They are an unmodified move of `ci_targets.py:3596-3766`, and the step tells the
  implementer to verify via `git diff` that the strings moved without changing. Printing 170
  lines of existing text into the plan would raise transcription risk, not lower it.
