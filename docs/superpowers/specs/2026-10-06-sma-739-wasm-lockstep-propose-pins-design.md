# SMA-739: pin the propose checker steps of wasm-lockstep

- Linear: SMA-739
- Date: 2026-10-06
- Base: `main` at `15bd61b7` (SMA-738, PR 390, merged)
- Related: SMA-738 spec `docs/superpowers/specs/2026-10-05-sma-738-wasm-lockstep-edge-flips-design.md`, section 4.5
- Challenge: one spec-challenger pass (verdict APPROVE WITH CHANGES). Section 9 lists what changed.

## 1. Problem

The `propose` job of `.github/workflows/wasm-lockstep.yml` has two steps that end with a
`lockstep_check.py` command:

| Step | Last command | What the command proves |
|---|---|---|
| `verify` | `lockstep_check.py artifact …` | AC4.1 and AC4.2: the downloaded artifact and the lock are acceptable. |
| `apply` | `lockstep_check.py status --file …` | The copy changed only the allowed files. |

`pin_check.py` P18 checks only that the checker command is the last whole command of its step,
with nothing joined to it. A command BEFORE the checker command is not checked. These four
forms before the checker command pass `pin_check.py` today:

- `exit 0`
- a bare `exit`
- `set -n`
- `set -o noexec`

With each form, the checker does not run and the step exits 0. The token step then mints the App
token. The final review of SMA-738 found the `verify` case with a probe of `violations()` (result
`[]`). The `apply` case is the same class. It was found during the SMA-739 intake, and Sven decided
to close both in this issue.

The same result is possible through keys other than `run:`:

- **`if:`.** No rule pins the `if:` of `verify` or `apply`. P15 refuses only `always()`,
  `failure()` and `cancelled()`. With `if: false`, the step is skipped and the job continues.
- **`env:` of the step or of the `propose` job.** `_env_violations` is a deny-list. These keys pass
  it today:
  - `SHELLOPTS: noexec`. MEASURED in `ubuntu:24.04` (2026-10-06): `SHELLOPTS=noexec bash -e s.sh`
    with `s.sh` = `echo ran` printed nothing and exited 0. So this is `set -o noexec` by a
    different route.
  - `SHELLOPTS: xtrace` with `PS4: '$(…)'`. Bash expands `PS4` before each traced command. Not
    measured.
  - `BASH_FUNC_python3%%`. Bash imports it as a function that shadows `python3`. Not measured,
    and it is not known if GitHub accepts the key name.
  - `PATH`, `PYTHONPATH`, `LD_PRELOAD`. These need a file in the checkout, which the same PR can
    add. SMA-738 closed this class for the `build` job.
- **The `with:` of `checkout` and `download`.** P17 checks only `persist-credentials`. A checkout
  `ref:` or `repository:` change makes `verify` run the checker and read `rs/Cargo.lock` from a
  different tree. A download `path:` change can write into the workspace.

SMA-738 closed the same class for the build steps `lock` and `stage` with an exact pin of the
whole `run:` text (P25, `LOCK_RUN`, `STAGE_RUN`), and with an allow-list for the workflow `env:`
and the `build` job `env:`.

## 2. Goals

- G1. `pin_check.py` refuses any change to the `propose` steps `verify` and `apply`, other than
  their `name:`. This includes the four `run:` forms in section 1, any `if:` change and any added
  key (`env:`, `shell:`, `timeout-minutes:`, `continue-on-error:`, `working-directory:`).
- G2. `pin_check.py` refuses any `env:` on the `propose` job.
- G3. `pin_check.py` refuses any change to the `if:` and `with:` of the `propose` steps `checkout`
  and `download`. Their `uses:` stays under P6, so a dependabot SHA bump does not red the gate.
- G4. The self-test has fixture rows that prove G1, G2 and G3. `--negative-control` has mutations
  of the REAL workflow for the `run:`, `if:` and `env:` routes.
- G5. The README no longer lists the `verify` gap as open. It describes P26 and states the
  residuals in section 8.

## 3. Non-goals

- The workflow does not change. Only `pin_check.py`, its fixture and the README change.
- The `run:` texts of the `propose` steps `commit`, `base`, `push`, `pr` and `close` are not pinned
  (decision D1, section 8). These steps run no `lockstep_check.py` command, so an early exit in
  them skips no check. But they can change the tree after the checks. The README states this
  residual.
- P7 and P18 do not change. They give a more specific message for their cases, and they stay as a
  second check.
- The `runs-on:` of `propose` is not pinned. A Windows label gives `pwsh`, and the scripts then
  fail at `set -euo pipefail`, so that case fails closed. A self-hosted label is outside the threat
  model of this workflow, which uses only GitHub-hosted runners.

## 4. Design

### 4.1 Approach

Pin the whole step mapping exactly, without `name:` (and without `uses:` for the two action
steps). This is the P25 method, made wider: P25 reads only the `run:` text, and the challenge
showed that `if:` and `env:` reach the same result. A deny-list of shell forms or of `env:` keys
cannot close the class: a quoted name, an indirect name, `set -n` or `SHELLOPTS` gets past it. A
deny-list is rejected.

### 4.2 Rule id

A new rule, **P26**: "the `propose` steps up to the last checker are pinned". P25 is the structure
of the `build` job, so P26 is a separate id. Every "P0-P25" text in `pin_check.py` changes to
"P0-P26":

- the module docstring (`pin_check.py:5`)
- the pass message of `main` (`pin_check.py:1149`)

### 4.3 Constants

- `VERIFY_RUN` and `APPLY_RUN` hold the `run:` text exactly as PyYAML loads it from the workflow.
  They use the same form as `LOCK_RUN` and `STAGE_RUN`: a `"\n".join((…)) + "\n"` of the lines.
- `PROPOSE_IF = "needs.build.outputs.changed == 'true'"`.
- `PROPOSE_PINNED` maps each pinned step id to its expected mapping, without `name` and `uses`:

  | Step | Expected keys and values |
  |---|---|
  | `checkout` | `id`, `if: PROPOSE_IF`, `with: {"ref": "${{ github.sha }}", "persist-credentials": False}` |
  | `download` | `id`, `if: PROPOSE_IF`, `with: {"name": "wasm-lockstep", "path": "${{ runner.temp }}/lockstep"}` |
  | `verify` | `id`, `if: PROPOSE_IF`, `run: VERIFY_RUN` |
  | `apply` | `id`, `if: PROPOSE_IF`, `run: APPLY_RUN` |

- The comment above the constants says: change these constants WITH the workflow.

The fixture has a module constant `VERIFY_RUN` today (`pin_check.py:849`). It is the one-line
fixture form that the P18 rows replace. The pin takes the name `VERIFY_RUN`. The fixture constant
goes away, because the fixture `verify` step now holds the pinned text (section 4.5). One name has
one meaning.

### 4.4 Check

A new function `_propose_pin_violations(propose)` runs from `_propose_violations`. For each step
id in `PROPOSE_PINNED`:

1. Take the step mapping without `name` (and without `uses` for `checkout` and `download`).
2. If the step is missing, report `P26 the propose step <id> is missing`.
3. If a key is extra or missing, report
   `P26 the propose step <id> has keys <sorted got>, expected <sorted want>`.
4. If `run` differs from the pin, report
   `P26 the <id> step script differs from the pinned text, first at line N`.
5. If another value differs, report `P26 the <key> of propose step <id> must be exactly <want>, not <got>`.

The line-number code in `_build_violations` moves into a helper `_first_diff_line(got, want)`.
P25 and P26 both use it. The P25 message text does not change.

A second check: if `"env" in propose`, report `P26 the propose job declares env`. This is the same
form as `P25 the build job declares env` (`pin_check.py:595`).

### 4.5 Fixture

The fixture `checkout`, `download`, `verify` and `apply` steps of `propose` change to the pinned
form. `verify` and `apply` use `_indent(VERIFY_RUN)` and `_indent(APPLY_RUN)`, in the same way as
`LOCK_STEP` and `STAGE_STEP`. Today they are one-line forms (`VERIFY_STEP`,
`PROPOSE_RUN = "        run: cp a b\n"`). The one-line forms do not pass P26, so the base fixture
must hold the pinned form, or every row reds.

This breaks about 44 existing rows. The plan handles them as follows:

- **34 rows anchor on `PROPOSE_RUN`** (`pin_check.py:873-986`), the old `apply` one-liner. They
  test general `propose` rules, not `apply`. They move to an unpinned step. The fixture `commit`
  line `        run: git commit -F "$RUNNER_TEMP/pr-title.txt"\n` occurs once, so it is the new
  anchor. The constant is renamed `UNPINNED_PROPOSE_RUN`, to match its new meaning.
- **The two PASS rows** "status as a whole last command" (`:933`) and "gh api --method GET in
  propose" (`:955`) stay PASS rows. "gh api …" moves to the unpinned step. "status as a whole
  last command" becomes a check that the base fixture `apply` step passes, or it moves to the
  unpinned step with a `status` command, whichever keeps it a PASS row. The plan fixes which.
- **The P7 row** at `:892-893` contains the literal `run: cp a b`. It is rebuilt from the new step
  constants.
- **The P18 rows for `verify`** (`:925-931`, 7 rows) and the P18 row for `status` (`:932`) are
  rebuilt on the pinned text. They must still want P18, and P18 must still fire for them. P26 also
  fires; a row matches if any message starts with its want.
- **The 2 rows on `VERIFY_STEP`** (`:914-915`) are rebuilt on the new `verify` step.

**Anchor rule for every new or rebuilt row:** the `old` text must occur exactly once in the
fixture. `self_test` replaces only the first match (`pin_check.py:1061`). The `apply` line
`for f in rs/Cargo.lock …; do` is identical to a `STAGE_RUN` line, and `set -euo pipefail` occurs
first in `LOCK_STEP`. So a row anchors on the checker line, or on a span that starts at
`      - id: apply\n` or `      - id: verify\n`. The self-test asserts `FIXTURE.count(old) == 1`
for each replacement of the new rows, and reports a row that breaks it as a failure.

### 4.6 Self-test rows

New rows. Each insertion row inserts a line directly before the checker line:

| # | Step | Mutation | Want |
|---|---|---|---|
| 1-4 | `verify` | insert `exit 0`, `exit`, `set -n`, `set -o noexec` | P26 |
| 5-8 | `apply` | insert `exit 0`, `exit`, `set -n`, `set -o noexec` | P26 |
| 9 | `verify` | change one character in the `--title-file` path | P26 |
| 10 | `apply` | change one character in the `status --file` path | P26 |
| 11 | `verify` | `run: |` → `run: |-` | P26 |
| 12 | `verify` | the exact message `P26 the verify step script differs from the pinned text, first at line 2` for the `exit 0` insertion | exact message |
| 13 | `verify` | `if: false` | P26 |
| 14 | `apply` | `if: false` | P26 |
| 15 | `verify` | step `env: {SHELLOPTS: noexec}` | P26 |
| 16 | `apply` | step `shell: bash` added (a value that P13 allows) | P26 |
| 17 | `propose` job | `env: {PATH: …}` | P26 |
| 18 | `checkout` | `ref: refs/pull/1/head` | P26 |
| 19 | `checkout` | `repository: someone/fork` added | P26 |
| 20 | `download` | `path: ${{ github.workspace }}` | P26 |
| 21 | `checkout` | `uses:` with a different 40-hex SHA | PASS |

Row 21 proves that a dependabot bump of a pinned action does not red P26.

### 4.7 Negative control

New mutations of the real workflow. A helper `step_run(job, step_id, change)` replaces the
SMA-738 `build_run`, and the build mutations use it with `job="build"`.

- `exit 0` inserted before the `artifact` line of `verify` → P26.
- `set -n` inserted before the `status` line of `apply` → P26.
- `if: false` on `verify` → P26.
- `SHELLOPTS: noexec` in the `env:` of the `propose` job → P26.

The count message no longer is a literal. `expect` counts its calls, and the summary prints that
count (`pin_check.py:1133`). Today the count is 10, and after the change it is 14.

### 4.8 README

In `ci/wasm-lockstep/README.md`:

- Add a P26 row to the rule table (near line 134), and a short list of the P26 checks below the
  P25 list.
- Below that list, state which rules only repeat the P26 pin, in the same way as lines 158-168 do
  for P25: on `verify` and `apply`, P7's `artifact` check and P18 only repeat the pin.
- In the trust model, the `propose` bullet (lines 203-206) says "A refusal fails the `verify`
  step, so the token step never runs". Add that this depends on P26 and on the P25 workflow
  `env:` allow-list.
- In "What the checks do not prove", remove the `verify` gap bullet (lines 243-245), and add the
  residuals of section 8.

## 5. Error handling

No new error path. A missing step, a missing key or a non-string `run:` gives a P26 message. The
exit codes stay: 0 pass, 3 assertion, 2 infrastructure.

## 6. Testing

1. **Red first.** Add rows 1-8 before the P26 check. Run
   `python3 ci/wasm-lockstep/pin_check.py --self-test`. Record that `violations()` gives `[]` for
   each of the eight rows, not only "no P26". The challenger traced `commands()` and
   `_checker_tail` for all eight forms and found no violation today; the test records it.
2. Add the P26 check. Re-run the self-test: all rows pass.
3. **Delete-the-feature check.** Remove the P26 comparison for one step at a time, and remove the
   job `env:` check. Record that the rows for each removed part red. Restore it.
4. `python3 ci/wasm-lockstep/pin_check.py .github/workflows/wasm-lockstep.yml` →
   `satisfies P0-P26`, rc 0.
5. `python3 ci/wasm-lockstep/pin_check.py --negative-control .github/workflows/wasm-lockstep.yml`
   → `14 mutations, 0 failed`, rc 0.
6. `moon run repo:wasm-lockstep repo:ruff-ci`, and the full-graph `moon ci` from the root
   `CLAUDE.md` before the push.

## 7. Risks

- **Maintenance cost.** A later edit to the four pinned steps must change the pin in the same
  commit, or `repo:wasm-lockstep` reds. This is the same cost as P25, and it is intended. The
  README states it.
- **YAML form.** The pin is the text as PyYAML loads it. A change of the block style (`|` to `|-`)
  changes the trailing newline, and P26 reds (row 11). This is intended.

## 8. Decisions and residuals

- **D1. The steps after `apply` stay unpinned (option b).** The challenger showed that `commit`,
  `base` and `push` can change the tree after both checks passed. Examples: write any bytes into
  `rs/Cargo.lock`, `git add` another path, `git commit --amend` (P20 refuses only `-c` and
  `--config-env`). The four `run:` forms in `verify` alone do not cause a push, because `commit`
  then fails on the missing `pr-title.txt`. A second edit in `commit` is needed. Option (a) is to
  pin all seven `propose` `run:` texts. Option (a) is stronger, but it changes the base form of
  every P8, P20 and P21 row and roughly doubles this change. This spec takes option (b): it states
  the residual in the README, and a new Linear issue tracks option (a). Sven decides at GATE 1.
- **Residual R1 (README):** "Steps after `apply` can change the tree after the checks. P26 proves
  that the two checkers run. It does not prove that the pushed tree is the checked tree."
- **Residual R2 (README):** P6 accepts any 40-hex SHA for an action. GitHub can resolve a commit
  SHA from a fork of the action repository ("impostor commit"). `checkout` and `download` run
  before `verify`. This gap existed before SMA-739.

## 9. Challenge changelog

| Finding | Severity | Action |
|---|---|---|
| P26 pins `run:` only; `if:` and `env:` can still skip the checker | BLOCKER | Folded in: P26 pins the whole step mapping; the `propose` job may not declare `env:` (4.3, 4.4). `SHELLOPTS=noexec` measured. |
| The `with:` of `checkout` and `download` is not pinned | MAJOR | Folded in: both pinned without `uses:` (G3, 4.3). |
| Steps after `apply` can change the tree after the checks | MAJOR | Option (b) taken, residual R1 in the README; option (a) to Sven at GATE 1 (D1). Section 1 corrected. |
| The fixture plan breaks about 44 rows | MAJOR | Folded in: 4.5 names each group and its handling. |
| Red-first wording weaker than 4.6 | MINOR | Folded in: 6.1 records `[]`. |
| Anchors can match `stage` or `lock` first | MINOR | Folded in: anchor rule and `count == 1` assertion (4.5). |
| No row checks the line-number helper | MINOR | Folded in: row 12. |
| The mutation count is a literal | MINOR | Folded in: counted (4.7). |
| The README location was wrong | MINOR | Folded in: exact lines in 4.8. |
| No `|-` row | MINOR | Folded in: row 11. |
| P6 impostor commits | MINOR | Folded in as README residual R2. Not fixed: outside scope. |
| `runs-on:` not pinned | QUESTION | Rejected: fails closed for a GitHub-hosted label (section 3). |
| `BASH_FUNC_python3%%` accepted by GitHub? | QUESTION | Not measured. The step-mapping pin refuses it in any case. |
