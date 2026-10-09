# SMA-739 wasm-lockstep propose pins Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `pin_check.py` refuses any change to the `propose` steps `checkout`, `download`, `verify` and `apply` (other than `name:`, and `uses:` for the two action steps), and any `env:` on the `propose` job, as the new rule P26.

**Architecture:** One file holds the rules, the fixture, the self-test and the negative control: `ci/wasm-lockstep/pin_check.py`. Task 1 moves the fixture to the pinned form with no rule change (all rows stay green). Task 2 adds the P26 rows red-first, then the rule. Task 3 adds the real-workflow mutations. Task 4 updates the README.

**Tech Stack:** Python 3.12, PyYAML, uv (`ci/wasm-lockstep/pyproject.toml`), moon task `repo:wasm-lockstep`, ruff (`repo:ruff-ci`).

**Spec:** `docs/superpowers/specs/2026-10-06-sma-739-wasm-lockstep-propose-pins-design.md`

## Global Constraints

- The workflow `.github/workflows/wasm-lockstep.yml` does NOT change.
- Every source file keeps its SPDX header. No new file in `ci/`.
- Exit codes stay: 0 pass, 3 assertion, 2 infrastructure.
- P26 message prefixes are exactly as in Task 2 Step 3. A self-test `want` that holds a space is the start of one exact message (`pin_check.py:1067`).
- Run Python through uv: `uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py …`. Put `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` first.
- ruff (`py/pyproject.toml`): line-length 200, `E501` ignored, `RUF` on. So no `tuple + tuple` concatenation of collections (RUF005): use `*NAME,` inside the literal.
- Commits: conventional, scope `ci`, for example `fix(ci): …`. End each message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `#NNN` line in the body.
- Do NOT restore a mutation with `git checkout --` or `git stash`. Commit first, mutate, then undo the edit by hand, and check that `git diff` is empty.
- Do NOT install host software. Do NOT start background jobs.
- Messages relayed from the user are for the coordinator, not for you.

## Review Focus

1. A dependabot SHA bump of `actions/checkout` or `actions/download-artifact` in `propose` must still pass. Owned by Task 2 (row "a dependabot SHA bump of the propose checkout", want PASS).
2. A YAML key that loads as a non-string (for example `on:` → `True`) in a pinned step must give a P26 message, not a `TypeError` from `sorted()`. Owned by Task 2 (row "an on: key on the apply step").
3. A missing pinned step must give `P26 the propose step <id> is missing`, not a `KeyError`. Owned by Task 2 (row "the apply step missing").
4. `persist-credentials: "false"` (a quoted string) passes P17 but must fail P26, because the pin holds the boolean. Owned by Task 2 (row "persist-credentials as the string false").
5. The real workflow must still pass after the change (`satisfies P0-P26`). Owned by Task 2 Step 6 and Task 3.

---

### Task 1: Move the fixture to the pinned propose form (no rule change)

**Files:**
- Modify: `ci/wasm-lockstep/pin_check.py` (constants after `STAGE_RUN`, ~line 126; fixture pieces ~716-857; `FIXTURE` ~784-805; rows ~860-1045; `self_test` ~1055)

**Interfaces:**
- Produces (module constants, used by Tasks 2 and 3):
  - `VERIFY_RUN: str`, `APPLY_RUN: str` — the pinned `run:` texts.
  - `PROPOSE_IF: str` — `"needs.build.outputs.changed == 'true'"`.
  - `ARTIFACT_LINE: str`, `STATUS_LINE: str` — the checker lines of the two steps.
  - `VERIFY_HEAD`, `APPLY_HEAD`, `VERIFY_STEP`, `APPLY_STEP: str` — fixture pieces.
  - `PROPOSE_CHECKOUT_USES: str` — fixture piece, unique in `FIXTURE`.
  - `UNPINNED_PROPOSE_RUN: str` — the old `PROPOSE_RUN`, now the `commit` step line.
  - `ANCHORED_ROWS` — rows whose every `old` must occur exactly once in `FIXTURE`. `ANCHORED_LABELS: frozenset[str]`.
- Removes: the fixture constant `VERIFY_RUN` (old one-line form, ~line 849) and `PROPOSE_RUN`.

- [ ] **Step 1: Record the baseline**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --self-test 2>&1 | tail -1
```
Expected: `pin_check self-test: 184 rows, 0 failed` (the number may differ; record it as N).

- [ ] **Step 2: Add the pinned texts after `STAGE_RUN`**

Directly after the `STAGE_RUN = …` block (it ends with `)) + "\n"`, ~line 125), insert:

```python
# P26 (SMA-739): the propose steps up to the last checker, as PyYAML loads them from the workflow.
# A pin of `run:` alone is not enough: `if: false` or `env: {SHELLOPTS: noexec}` also skips the
# checker. So each step is pinned whole, without `name` (and without `uses` for the two action
# steps, which P6 checks, so that a dependabot bump stays green). Change these WITH the workflow.
VERIFY_RUN = "\n".join((
    'set -euo pipefail',
    'python3 ci/wasm-lockstep/lockstep_check.py artifact --dir "$RUNNER_TEMP/lockstep" --old rs/Cargo.lock --body-file "$RUNNER_TEMP/pr-body.md" --title-file "$RUNNER_TEMP/pr-title.txt"',
)) + "\n"
APPLY_RUN = "\n".join((
    'set -euo pipefail',
    'for f in rs/Cargo.lock rs/crates/bindings/paigasus-wasm/paigasus_wasm.js rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm; do',
    '  if test -f "$RUNNER_TEMP/lockstep/$f"; then',
    '    cp "$RUNNER_TEMP/lockstep/$f" "$f"',
    '  fi',
    'done',
    'git status --porcelain --untracked-files=all > "$RUNNER_TEMP/status.txt"',
    'cat "$RUNNER_TEMP/status.txt"',
    'python3 ci/wasm-lockstep/lockstep_check.py status --file "$RUNNER_TEMP/status.txt"',
)) + "\n"
PROPOSE_IF = "needs.build.outputs.changed == 'true'"
```

These values were read from the workflow with PyYAML on 2026-10-06. If Step 7 reds on the real workflow, print the loaded `run` with `yaml.safe_load` and compare; do not edit the workflow.

- [ ] **Step 3: Replace the old fixture pieces**

Delete these two definitions (~lines 849-853):

```python
VERIFY_RUN = "        run: python3 ci/wasm-lockstep/lockstep_check.py artifact --dir d --old rs/Cargo.lock\n"
VERIFY_STEP = """      - id: verify
        if: needs.build.outputs.changed == 'true'
        run: python3 ci/wasm-lockstep/lockstep_check.py artifact --dir d --old rs/Cargo.lock
"""
```

Replace `PROPOSE_RUN = "        run: cp a b\n"` (~line 856) with:

```python
# The general propose rows run on the UNPINNED commit step. The pinned steps (P26) would add a P26
# message to every row, and a PASS row would fail.
UNPINNED_PROPOSE_RUN = '        run: git commit -F "$RUNNER_TEMP/pr-title.txt"\n'
```

Then replace every use of the name `PROPOSE_RUN` in the rows with `UNPINNED_PROPOSE_RUN`:

BSD `sed` on macOS has no `\b`, so use `perl`. The definition line already says `UNPINNED_PROPOSE_RUN`, and `\b` does not match inside it:

```bash
perl -pi -e 's/\bPROPOSE_RUN\b/UNPINNED_PROPOSE_RUN/g' ci/wasm-lockstep/pin_check.py
grep -c 'UNPINNED_UNPINNED' ci/wasm-lockstep/pin_check.py      # expected: 0
grep -nE '(^|[^_])PROPOSE_RUN' ci/wasm-lockstep/pin_check.py   # expected: no output
```

- [ ] **Step 4: Add the new fixture pieces after `UPLOAD_STEP`**

Directly after the `UPLOAD_STEP = "".join((…))` block (~line 750), insert:

```python
# SMA-739 P26: the pinned propose steps of the fixture, as pieces, so that the rows can name them.
ARTIFACT_LINE = VERIFY_RUN.split("\n")[1]
STATUS_LINE = APPLY_RUN.split("\n")[-2]
VERIFY_HEAD = f"      - id: verify\n        if: {PROPOSE_IF}\n"
APPLY_HEAD = f"      - id: apply\n        if: {PROPOSE_IF}\n"
VERIFY_STEP = VERIFY_HEAD + "        run: |\n" + _indent(VERIFY_RUN)
APPLY_STEP = APPLY_HEAD + "        run: |\n" + _indent(APPLY_RUN)
PROPOSE_CHECKOUT_USES = f"        if: {PROPOSE_IF}\n        uses: actions/checkout@" + "1" * 40 + "\n"
```

- [ ] **Step 5: Change the propose steps of `FIXTURE`**

In `FIXTURE`, replace this text (~lines 792-805):

```text
      - id: checkout
        if: needs.build.outputs.changed == 'true'
        uses: actions/checkout@""" + "1" * 40 + """
        with:
          persist-credentials: false
      - id: download
        if: needs.build.outputs.changed == 'true'
        uses: actions/download-artifact@""" + "3" * 40 + """
      - id: verify
        if: needs.build.outputs.changed == 'true'
        run: python3 ci/wasm-lockstep/lockstep_check.py artifact --dir d --old rs/Cargo.lock
      - id: apply
        if: needs.build.outputs.changed == 'true'
        run: cp a b
      - id: token
```

with:

```text
      - id: checkout
        if: needs.build.outputs.changed == 'true'
        uses: actions/checkout@""" + "1" * 40 + """
        with:
          ref: ${{ github.sha }}
          persist-credentials: false
      - id: download
        if: needs.build.outputs.changed == 'true'
        uses: actions/download-artifact@""" + "3" * 40 + """
        with:
          name: wasm-lockstep
          path: ${{ runner.temp }}/lockstep
""" + VERIFY_STEP + APPLY_STEP + """\
      - id: token
```

- [ ] **Step 6: Rebuild the rows that the new fixture breaks**

a. Row "the token step before the checker step" (~line 892-893). Replace the whole row with:

```python
    ("the token step before the checker step", ((VERIFY_STEP + APPLY_STEP + TOKEN_STEP, TOKEN_STEP + VERIFY_STEP + APPLY_STEP),), "P7"),
```

b. Row "NEEDS in another case in a with: in propose" (~line 906) and row "steps['build'].outputs in a with: of propose" (~line 987). The checkout `with:` now holds `ref:`, so a second `ref:` is a duplicate key (P0). Change the inserted key from `ref:` to `repository:` in both rows:

```python
    ("NEEDS in another case in a with: in propose", (("          persist-credentials: false\n      - id: download", "          persist-credentials: false\n          repository: ${{ NEEDS.build.outputs.changed }}\n      - id: download"),), "P9"),
```
```python
    ("steps['build'].outputs in a with: of propose", (("          persist-credentials: false\n      - id: download", "          persist-credentials: false\n          repository: ${{ steps['build'].outputs.v }}\n      - id: download"),), "P24"),
```

c. Row "persist-credentials: true on a checkout" (~line 918). `with:` is no longer directly above `persist-credentials` in propose. Replace with:

```python
    ("persist-credentials: true on a checkout", (("          persist-credentials: false\n      - id: download", "          persist-credentials: true\n      - id: download"),), "P17"),
```

d. Delete the eight rows from `("verify joined with || true", …` to `("status joined with || true", …` (~lines 925-932). They move to `ANCHORED_ROWS` (Step 7).

e. Row "status as a whole last command" (~line 933) now runs on `UNPINNED_PROPOSE_RUN` after the rename of Step 3. Keep it as it is. It must stay PASS.

- [ ] **Step 7: Add `ANCHORED_ROWS` and the unique-anchor check**

Directly above `# (label, (old, new) replacements on FIXTURE, rule id that must appear or "PASS")` (~line 859), insert:

```python
# Rows on the pinned propose steps. `self_test` replaces only the FIRST match, and some lines of
# the pinned steps also occur in LOCK_RUN or STAGE_RUN. So every `old` of these rows must occur
# exactly once in FIXTURE, and self_test checks it.
_A = f"          {ARTIFACT_LINE}\n"
_S = f"          {STATUS_LINE}\n"
ANCHORED_ROWS: tuple[tuple[str, tuple[tuple[str, str], ...], str], ...] = (
    ("verify joined with || true", ((_A, f"          {ARTIFACT_LINE} || true\n"),), "P18"),
    ("verify joined with || rc=$?", ((_A, f"          {ARTIFACT_LINE} || rc=$?\n"),), "P18"),
    ("verify followed by ; true", ((_A, f"          {ARTIFACT_LINE} ; true\n"),), "P18"),
    ("verify piped into cat", ((_A, f"          {ARTIFACT_LINE} | cat\n"),), "P18"),
    ("verify behind an && guard", ((_A, f"          test -f x && {ARTIFACT_LINE}\n"),), "P18"),
    ("verify inside a skipped if", ((_A, f"          if false; then\n            {ARTIFACT_LINE}\n          fi\n"),), "P18"),
    ("verify followed by exit 0", ((_A, f"{_A}          exit 0\n"),), "P18"),
    ("status joined with || true", ((_S, f"          {STATUS_LINE} || true\n"),), "P18"),
    ("continue-on-error: true on verify", ((VERIFY_HEAD, VERIFY_HEAD + "        continue-on-error: true\n"),), "P14"),
    ('continue-on-error: "true" (a string) on verify', ((VERIFY_HEAD, VERIFY_HEAD + '        continue-on-error: "true"\n'),), "P14"),
)
ANCHORED_LABELS = frozenset(label for label, _r, _w in ANCHORED_ROWS)
```

Delete the two old rows `("continue-on-error: true on verify", …)` and `('continue-on-error: "true" (a string) on verify', …)` (~lines 914-915), because `ANCHORED_ROWS` now holds them.

At the end of `SELF_TEST_ROWS`, directly before its closing `)` (~line 1045), add:

```python
    *ANCHORED_ROWS,
```

In `self_test`, directly after `drift = [old for old, _new in replacements if old not in text]`, insert:

```python
        shared = [old for old, _new in replacements if label in ANCHORED_LABELS and FIXTURE.count(old) != 1]
        if shared:
            print(f"  FAIL  {label}: the anchor {shared[0]!r} occurs {FIXTURE.count(shared[0])} times in the fixture, not once", file=sys.stderr)
            failures += 1
            continue
```

- [ ] **Step 8: Run the self-test, the real check and the negative control**

```bash
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --self-test 2>&1 | tail -3
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py .github/workflows/wasm-lockstep.yml
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --negative-control .github/workflows/wasm-lockstep.yml 2>&1 | tail -1
```

Expected: `pin_check self-test: N rows, 0 failed` (the same N as Step 1), then `… satisfies P0-P25`, then `pin_check negative control: 10 mutations, 0 failed`. If a row fails, read its `got` list. A row that now gets P0 has a duplicate key; a row that reports drift has an anchor that the new fixture no longer holds.

- [ ] **Step 9: Prove the unique-anchor check bites**

Temporarily change the `_S` row "status joined with || true" to anchor on `"          set -euo pipefail\n"`. Run the self-test. Expected: `FAIL  status joined with || true: the anchor '          set -euo pipefail\n' occurs 4 times …` (the count is ≥ 2). Undo the edit by hand and check `git diff --stat` shows only the Task 1 changes.

- [ ] **Step 10: ruff, then commit**

```bash
uv run --locked --project py ruff check ci/wasm-lockstep/pin_check.py
git add ci/wasm-lockstep/pin_check.py
git commit -m "test(ci): move the wasm-lockstep fixture to the pinned propose steps (SMA-739)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: P26 — rows red-first, then the rule

**Files:**
- Modify: `ci/wasm-lockstep/pin_check.py` (docstring line 5; `_build_violations` ~614-632; `_propose_violations` ~656-684; `ANCHORED_ROWS`; `main` ~1149)

**Interfaces:**
- Consumes: everything Task 1 produces.
- Produces: `PROPOSE_PINNED: dict[str, dict]`, `_first_diff_line(got: str, want: str) -> int`, `_propose_pin_violations(propose: dict) -> list[str]`. Messages:
  - `P26 the propose job declares env`
  - `P26 the propose step <id> is missing`
  - `P26 the propose step <id> has keys <sorted got>, expected <sorted want>`
  - `P26 the <id> step script differs from the pinned text, first at line <N>`
  - `P26 the <key> of propose step <id> must be exactly <want!r>, not <got!r>`

- [ ] **Step 1: Add the P26 rows to the end of `ANCHORED_ROWS`**

Directly above `ANCHORED_ROWS`, add:

```python
EARLY_EXITS = ("exit 0", "exit", "set -n", "set -o noexec")
```

Append these rows inside `ANCHORED_ROWS`, after the last existing row:

```python
    # SMA-739 P26: an early exit before the checker, and the other keys that skip it.
    *((f"{form} before the artifact command", ((_A, f"          {form}\n{_A}"),), "P26") for form in EARLY_EXITS),
    *((f"{form} before the status command", ((_S, f"          {form}\n{_S}"),), "P26") for form in EARLY_EXITS),
    ("a changed --title-file path on verify", ((_A, _A.replace("pr-title.txt", "pr-titel.txt")),), "P26"),
    ("a changed status --file path on apply", ((_S, _S.replace("status.txt", "statuz.txt")),), "P26"),
    ("run: |- on verify", ((VERIFY_HEAD + "        run: |\n", VERIFY_HEAD + "        run: |-\n"),), "P26"),
    ("exit 0 before the artifact command, by the exact message", ((_A, f"          exit 0\n{_A}"),), "P26 the verify step script differs from the pinned text, first at line 2"),
    ("if: false on verify", ((VERIFY_HEAD, "      - id: verify\n        if: false\n"),), "P26"),
    ("if: false on apply", ((APPLY_HEAD, "      - id: apply\n        if: false\n"),), "P26"),
    ("env SHELLOPTS: noexec on verify", ((VERIFY_HEAD, VERIFY_HEAD + "        env:\n          SHELLOPTS: noexec\n"),), "P26"),
    ("shell: bash added on apply", ((APPLY_HEAD, APPLY_HEAD + "        shell: bash\n"),), "P26"),
    ("an on: key on the apply step", ((APPLY_HEAD, APPLY_HEAD + "        on: x\n"),), "P26"),
    ("the apply step missing", ((APPLY_STEP, ""),), "P26 the propose step apply is missing"),
    ("a propose job env PATH", (("  propose:\n", "  propose:\n    env:\n      PATH: /x\n"),), "P26"),
    ("checkout ref names a pull request head", (("          ref: ${{ github.sha }}\n", "          ref: refs/pull/1/head\n"),), "P26"),
    ("checkout repository added", (("          ref: ${{ github.sha }}\n", "          ref: ${{ github.sha }}\n          repository: someone/fork\n"),), "P26"),
    ("persist-credentials as the string false", (("          persist-credentials: false\n      - id: download", "          persist-credentials: 'false'\n      - id: download"),), "P26"),
    ("download path into the workspace", (("          path: ${{ runner.temp }}/lockstep\n", "          path: ${{ github.workspace }}\n"),), "P26"),
    ("a dependabot SHA bump of the propose checkout", ((PROPOSE_CHECKOUT_USES, PROPOSE_CHECKOUT_USES.replace("1" * 40, "5" * 40)),), "PASS"),
```

- [ ] **Step 2: Run the self-test and record the red-first result**

```bash
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --self-test 2>&1 | grep -E 'FAIL|rows'
```

Expected: every new row that wants P26 FAILS; the PASS row "a dependabot SHA bump …" passes. For each of the eight `… before the artifact command` / `… before the status command` rows, the output MUST be `got []`. Copy these eight lines into the task report. If one of them shows a non-empty `got`, stop and report it: the row does not show the gap.

- [ ] **Step 3: Add `PROPOSE_PINNED`, the helper and the rule**

After `PROPOSE_IF = …` (Task 1 Step 2), add:

```python
PROPOSE_PINNED = {
    "checkout": {"id": "checkout", "if": PROPOSE_IF, "with": {"ref": "${{ github.sha }}", "persist-credentials": False}},
    "download": {"id": "download", "if": PROPOSE_IF, "with": {"name": "wasm-lockstep", "path": "${{ runner.temp }}/lockstep"}},
    "verify": {"id": "verify", "if": PROPOSE_IF, "run": VERIFY_RUN},
    "apply": {"id": "apply", "if": PROPOSE_IF, "run": APPLY_RUN},
}
```

Directly above `def _build_violations`, add:

```python
def _first_diff_line(got: str, want: str) -> int:
    """The 1-based number of the first line where two scripts differ (P25, P26)."""
    a, b = got.split("\n"), want.split("\n")
    return next((n + 1 for n in range(min(len(a), len(b))) if a[n] != b[n]), min(len(a), len(b)) + 1)
```

In `_build_violations`, replace:

```python
        if text != pinned:
            got, want = text.split("\n"), pinned.split("\n")
            line = next((n + 1 for n in range(min(len(got), len(want))) if got[n] != want[n]), min(len(got), len(want)) + 1)
            out.append(f"P25 the {step_id} step script differs from the pinned text, first at line {line}")
```

with:

```python
        if text != pinned:
            out.append(f"P25 the {step_id} step script differs from the pinned text, first at line {_first_diff_line(text, pinned)}")
```

Directly above `def _propose_violations`, add:

```python
def _propose_pin_violations(propose: dict) -> list[str]:
    """P26 (SMA-739 spec 4.4). The propose steps up to the last checker are pinned whole, without
    `name` (and without `uses` for the action steps, which P6 checks). A pin of `run:` alone is not
    enough: `if: false` or `env: {SHELLOPTS: noexec}` also skips the checker, and a checkout `ref:`
    change runs a checker from a different tree. The propose job may not declare env."""
    out = []
    if "env" in propose:
        out.append("P26 the propose job declares env")
    by_id = {str(s.get("id")): s for s in _steps(propose, "propose")}
    for step_id, want in PROPOSE_PINNED.items():
        if step_id not in by_id:
            out.append(f"P26 the propose step {step_id} is missing")
            continue
        drop = ("name", "uses") if "with" in want else ("name",)
        got = {str(k): v for k, v in by_id[step_id].items() if k not in drop}
        if sorted(got) != sorted(want):
            out.append(f"P26 the propose step {step_id} has keys {sorted(got)}, expected {sorted(want)}")
            continue
        for key, value in want.items():
            if got[key] == value:
                continue
            if key == "run":
                out.append(f"P26 the {step_id} step script differs from the pinned text, first at line {_first_diff_line(str(got[key]), value)}")
            else:
                out.append(f"P26 the {key} of propose step {step_id} must be exactly {value!r}, not {got[key]!r}")
    return out
```

At the end of `_propose_violations`, directly before its `return out`, add:

```python
    out += _propose_pin_violations(propose)
```

Note: `got[key] == value` compares `False == 0` as equal in Python. YAML `0` for `persist-credentials` is not a real risk (P17 already refuses it), so no extra type check.

- [ ] **Step 4: Change "P0-P25" to "P0-P26"**

In the module docstring (line 5): `The rules (P0-P25)` → `The rules (P0-P26)`.
In `main` (~line 1149): `satisfies P0-P25` → `satisfies P0-P26`.

- [ ] **Step 5: Run the self-test**

```bash
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --self-test 2>&1 | tail -2
```

Expected: `pin_check self-test: <N + 24> rows, 0 failed` (24 new rows: 8 early exits and 16 others).

- [ ] **Step 6: Run the real check and the negative control**

```bash
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py .github/workflows/wasm-lockstep.yml
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --negative-control .github/workflows/wasm-lockstep.yml 2>&1 | tail -1
```

Expected: `pin_check: .github/workflows/wasm-lockstep.yml satisfies P0-P26` (rc 0), then `pin_check negative control: 10 mutations, 0 failed`.

- [ ] **Step 7: Commit**

```bash
uv run --locked --project py ruff check ci/wasm-lockstep/pin_check.py
git add ci/wasm-lockstep/pin_check.py
git commit -m "fix(ci): pin the wasm-lockstep propose checker steps whole as P26 (SMA-739)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 8: Delete-the-feature checks (after the commit)**

Do each mutation, run the self-test, record the FAIL labels, then undo by hand and check `git diff` is empty.

1. Delete the line `out += _propose_pin_violations(propose)`. Expected: every row that wants P26 FAILS.
2. Delete the two lines `if "env" in propose:` and `out.append("P26 the propose job declares env")`. Expected: only "a propose job env PATH" FAILS.
3. Delete the `"apply": …` entry of `PROPOSE_PINNED`. Expected: the nine `apply` rows FAIL ("… before the status command" ×4, "a changed status --file path on apply", "if: false on apply", "shell: bash added on apply", "an on: key on the apply step", "the apply step missing").
4. Delete the `"checkout": …` entry. Expected: "checkout ref names …", "checkout repository added", "persist-credentials as the string false" FAIL.

Put the four FAIL lists into the task report.

---

### Task 3: Mutations of the real workflow

**Files:**
- Modify: `ci/wasm-lockstep/pin_check.py` (`negative_control` ~1079-1134)

**Interfaces:**
- Consumes: the P26 rule from Task 2.
- Produces: `negative_control` prints `pin_check negative control: <count> mutations, <failures> failed`, where `<count>` is the number of `expect` calls.

- [ ] **Step 1: Count the `expect` calls**

In `negative_control`, replace:

```python
    def expect(label: str, mutated: dict, want: str) -> None:
        nonlocal failures
        got = violations(mutated)
```

with:

```python
    checked = 0

    def expect(label: str, mutated: dict, want: str) -> None:
        nonlocal failures, checked
        checked += 1
        got = violations(mutated)
```

and replace `print(f"pin_check negative control: 10 mutations, {failures} failed")` with:

```python
    print(f"pin_check negative control: {checked} mutations, {failures} failed")
```

- [ ] **Step 2: Generalise `build_run` to `step_run`**

Replace the `build_run` definition:

```python
    def build_run(step_id: str, change) -> dict:
        mutated = copy.deepcopy(real)
        for step in mutated["jobs"]["build"]["steps"]:
```

with:

```python
    def step_run(job: str, step_id: str, change) -> dict:
        mutated = copy.deepcopy(real)
        for step in mutated["jobs"][job]["steps"]:
```

Then change each of the five calls `build_run("stage", …)` / `build_run("lock", …)` to `step_run("build", "stage", …)` / `step_run("build", "lock", …)`. Change the comment `# SMA-738: three mutations of the REAL build job.` to `# SMA-738: mutations of the REAL build job.`

- [ ] **Step 3: Add the four P26 mutations**

Directly before the `print(…negative control…)` line, add:

```python
    # SMA-739: P26 on the REAL propose job. The fixture rows prove the rule; these prove it bites on
    # the real steps.
    expect("exit 0 inserted before the artifact line of verify", step_run("propose", "verify", lambda ln: "exit 0\n" + ln if " artifact --dir " in ln else ln), "P26")
    expect("set -n inserted before the status line of apply", step_run("propose", "apply", lambda ln: "set -n\n" + ln if " status --file " in ln else ln), "P26")
    skipped = copy.deepcopy(real)
    for step in skipped["jobs"]["propose"]["steps"]:
        if step.get("id") == "verify":
            step["if"] = False
    expect("if: false on the verify step", skipped, "P26")
    noexec = copy.deepcopy(real)
    noexec["jobs"]["propose"]["env"] = {"SHELLOPTS": "noexec"}
    expect("SHELLOPTS: noexec in the propose job env", noexec, "P26")
```

- [ ] **Step 4: Run it**

```bash
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --negative-control .github/workflows/wasm-lockstep.yml
```

Expected: 14 `ok` lines, then `pin_check negative control: 14 mutations, 0 failed`, rc 0.

- [ ] **Step 5: Prove the four new mutations bite**

Delete the line `out += _propose_pin_violations(propose)`. Run Step 4 again. Expected: the four SMA-739 mutations FAIL (`want P26, got […]`), the other ten pass, the summary says `14 mutations, 4 failed`, rc 3. Undo by hand; `git diff` must show only the Task 3 edits.

- [ ] **Step 6: Commit**

```bash
uv run --locked --project py ruff check ci/wasm-lockstep/pin_check.py
git add ci/wasm-lockstep/pin_check.py
git commit -m "test(ci): mutate the real wasm-lockstep propose steps in the negative control (SMA-739)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: README, and the full gate

**Files:**
- Modify: `ci/wasm-lockstep/README.md` (rule table ~134; after the P25 notes ~171; trust model ~203-206; "What the checks do not prove" ~243-245)

**Interfaces:**
- Consumes: the P26 messages and the 14-mutation count from Tasks 2 and 3.

- [ ] **Step 1: Add the P26 row to the rule table**

After the line `| P25 | The \`build\` job around the run-1 lock compare (SMA-738). The checks are in the list below the table. |`, add:

```markdown
| P26 | The `propose` steps up to the last checker (SMA-739). The checks are in the list below the P25 list. |
```

- [ ] **Step 2: Add the P26 list**

After the paragraph that ends `A new action, or a tag in place of a SHA, does.` (~line 171) and before `## The trust model`, add:

```markdown
P26 holds these checks:

1. The exact pin. The steps `checkout`, `download`, `verify` and `apply` of `propose` equal
   `PROPOSE_PINNED` in `pin_check.py`, key by key. `name:` is not compared. `uses:` of `checkout`
   and `download` is not compared, because P6 checks it, so a dependabot bump stays green. The
   `run:` text of `verify` equals `VERIFY_RUN`, and of `apply` equals `APPLY_RUN`. If you edit one
   of these four steps, change `pin_check.py` in the same commit.
2. An extra key on these steps is refused. So `env:` (for example `SHELLOPTS: noexec`), `shell:`,
   `if: false` and a changed `with:` (for example a checkout `ref:` of a pull request head) are
   refused.
3. The `propose` job has no `env:`.

A pin of `run:` alone is not enough. `if: false` skips the step, and `SHELLOPTS=noexec` in the
environment makes bash read the script without running it. This was measured in `ubuntu:24.04`:
the script printed nothing and exited 0.

On `verify` and `apply`, P7's `artifact` check and P18 only repeat the P26 pin. They give a
clearer message. P14 and P13 on these two steps also only repeat it.
```

- [ ] **Step 3: Update the trust-model `propose` bullet**

Replace:

```markdown
  no moon, and no script from the artifact. A refusal fails the `verify` step, so the token step
  never runs.
```

with:

```markdown
  no moon, and no script from the artifact. A refusal fails the `verify` step, so the token step
  never runs. This depends on P26, which pins the steps up to the last checker, and on the P25
  workflow `env:` allow-list.
```

- [ ] **Step 4: Update "What the checks do not prove"**

Replace the bullet:

```markdown
- In the `propose` step `verify`, an `exit 0` or a `set -n` before the `lockstep_check.py artifact`
  command passes `pin_check.py`. The gap existed before SMA-738 and is outside G6. SMA-739
  tracks it.
```

with:

```markdown
- The steps after `apply` (`commit`, `base`, `push`, `pr`, `close`) are not pinned. They can
  change the tree after the checks: write `rs/Cargo.lock` again, `git add` another path, or run
  `git commit --amend`. P26 proves that the two checkers run. It does not prove that the pushed
  tree is the checked tree. SMA-740 tracks it.
- P6 accepts any 40-hex SHA for an allowlisted action. GitHub can resolve a commit SHA from a fork
  of the action repository (an "impostor commit"). `checkout` and `download` run before `verify`.
  This gap existed before SMA-739.
```

- [ ] **Step 5: Run the gate task and ruff**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:wasm-lockstep repo:ruff-ci --force
```

Expected: both tasks pass. `repo:ruff-ci` needs bash 4+ on this Mac (root `CLAUDE.md`). If it fails with `mapfile: command not found`, run `/opt/homebrew/bin/bash ci/ruff/run.sh` and read that result instead.

- [ ] **Step 6: Commit**

```bash
git add ci/wasm-lockstep/README.md
git commit -m "docs(ci): describe P26 and the propose residuals in the wasm-lockstep README (SMA-739)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 7: Full graph (coordinator, before the push)**

Run the full-graph `moon ci` command from the root `CLAUDE.md` (between the `ci-targets` markers) with `--base origin/main --include-relations`. Read any failing gate with the bash rules of the root `CLAUDE.md` before you call it a finding.
