# SMA-740: pin every propose step of wasm-lockstep

- Linear: SMA-740
- Date: 2026-10-09
- Base: `main` at `607412fa` (SMA-739, PR 394, merged)
- Related: SMA-739 spec `docs/superpowers/specs/2026-10-06-sma-739-wasm-lockstep-propose-pins-design.md`,
  section 8 (D1, R1)
- Challenge: one spec-challenger pass (verdict APPROVE WITH CHANGES). Section 9 lists what changed.

## 1. Problem

SMA-739 added rule P26 to `ci/wasm-lockstep/pin_check.py`. P26 pins the `propose` steps
`checkout`, `download`, `verify` and `apply` of `.github/workflows/wasm-lockstep.yml` whole,
without `name:` (and without `uses:` for the two action steps). The six steps after `apply` are not
pinned whole (SMA-739 decision D1, option b).

The steps `commit`, `base` and `push` can change the tree after both checkers passed:

- `commit` can write any bytes into `rs/Cargo.lock`, or `git add` another path. Nothing checks the
  target of a redirect, and `echo` is on the allowlist.
- `base` and `push` can run `git commit --amend`. P20 refuses only `-c` and `--config-env` on `git`.

So P26 proves that the two checkers run. It does not prove that the pushed tree is the checked
tree. The README states this as a residual (`ci/wasm-lockstep/README.md`, "What the checks do not
prove").

The `token` step is the only other `propose` step that is not pinned whole. P16 pins the prefix of
its `uses:` and its `with:`. An added `if:` or `env:` passes P16.

## 2. Goals

- G1. `pin_check.py` refuses any change to any of the ten `propose` steps, other than `name:` and
  the SHA in the `uses:` of `checkout`, `download` and `token`.
- G2. `pin_check.py` refuses any key on the `propose` job that is not on an allow-list (D4).
- G3. The self-test has fixture rows that prove G1 for each of the six new steps, and G2. The rows
  include the attacks in section 1.
- G4. Each existing self-test row keeps its purpose. A row that wants a rule id still proves that
  this rule fires. A row that proves that a deny rule does not fire too much keeps that proof, on
  the step that it names.
- G5. `--negative-control` has a mutation of the REAL workflow for each of the six new steps.
- G6. The README no longer states the residual "the steps after `apply` are not pinned". It
  describes P26 for all ten steps and the job keys, and it states exactly which checks only repeat
  the pin.

## 3. Non-goals

- The workflow file does not change. Only `pin_check.py`, its fixture and the README change.
- The VALUES of the `propose` job keys `name:`, `runs-on:` and `timeout-minutes:` are not pinned.
  The SMA-739 spec, section 3, gives the reason for `runs-on:`. `needs:`, `if:`, `environment:` and
  `permissions:` stay under P11 and P10. G2 refuses any other key.
- No rule is removed. Section 4.8 lists which checks only repeat the pin on `propose` and which
  checks still carry a control of their own (P7 order, P13 workflow `defaults:`, P14 job
  `continue-on-error:`, P12, and P3, P9 and P24 on `name:` and on the job keys).
- No new rule id. P26 gets wider, in the same way as SMA-739 made the pin of P25 wider.
- Residual R2 of SMA-739 (P6 accepts an "impostor commit" SHA) is not fixed.
- Characters that a YAML 1.1 parser and a YAML 1.2 parser read differently in a literal block
  (U+0085, U+2028, U+2029) are not refused. Section 8, Q1, states the open question.

## 4. Design

### 4.1 Approach

Use the SMA-739 method on the six remaining steps: pin the whole step mapping exactly, without
`name:` (and without `uses:` for `token`). A deny-list of shell forms cannot close the class
(SMA-739 spec, 4.1). Add an allow-list of the `propose` job keys, so that a new job key fails
closed.

### 4.2 Constants

Five new `run:` constants hold the text exactly as PyYAML loads it from the workflow. They use the
same form as `VERIFY_RUN`: a `"\n".join((…)) + "\n"` of the lines.

- `COMMIT_RUN` (workflow lines 258-262)
- `BASE_RUN` (lines 274-292)
- `PUSH_RUN` (lines 304-324). The comment lines inside the script (310-312) are part of the text.
- `PR_RUN` (lines 336-343)
- `CLOSE_RUN` (lines 351-355)

The line numbers are for `main` at `607412fa`. The implementer copies the text from the workflow and
proves it with the real-workflow run (section 6, step 4).

`PROPOSE_PINNED` gets six entries, in workflow order:

| Step | Expected keys and values |
|---|---|
| `token` | `id`, `with: TOKEN_WITH` |
| `commit` | `id`, `if: PROPOSE_IF`, `run: COMMIT_RUN` |
| `base` | `id`, `if: PROPOSE_IF`, `env: {"GH_TOKEN": "${{ github.token }}", "BUILT_ON": "${{ github.sha }}"}`, `run: BASE_RUN` |
| `push` | `id`, `if: PUSH_IF`, `env: {"GH_TOKEN": TOKEN_OUT, "PUSH_TOKEN": TOKEN_OUT}`, `run: PUSH_RUN` |
| `pr` | `id`, `if: PUSH_IF`, `env: {"GH_TOKEN": TOKEN_OUT}`, `run: PR_RUN` |
| `close` | `id`, `if: "needs.build.outputs.changed == 'false'"`, `env: {"GH_TOKEN": TOKEN_OUT}`, `run: CLOSE_RUN` |

with `PUSH_IF = "needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'"` and
`TOKEN_OUT = "${{ steps.token.outputs.token }}"`.

`PROPOSE_ACTIONS` gets `"token": "actions/create-github-app-token"`. The existing `drop` rule
(`("name", "uses") if "with" in want`) then removes `uses` from the `token` comparison, and the
action-name check covers it. P16 also refuses another action, so row 14 (4.6) is the only proof of
this entry.

`PROPOSE_JOB_KEYS = frozenset({"name", "needs", "if", "runs-on", "timeout-minutes", "environment",
"permissions", "steps"})`. These are the keys of the real `propose` job (workflow lines 190-200).

The comment above the constants changes from "the propose steps up to the last checker" to "every
propose step". It keeps the sentence: change these constants WITH the workflow.

### 4.3 Check

`_propose_pin_violations` iterates `PROPOSE_PINNED`, so the new entries are checked with the
existing messages:

- `P26 the propose step <id> is missing`
- `P26 the propose step <id> has keys <sorted got>, expected <sorted want>`
- `P26 the propose step <id> must use <action>, not <uses>`
- `P26 the <id> step script differs from the pinned text, first at line N`
- `P26 the <key> of propose step <id> must be exactly <want>, not <got>`

One change: the check `if "env" in propose` becomes a check of every job key. For each key of
`propose` that is not in `PROPOSE_JOB_KEYS`, in sorted order, report
`P26 the propose job declares <key>`. For `env` this is the same message as today, so the existing
row "a propose job env PATH" does not change.

The docstring changes from "up to the last checker" to "every propose step, and the job keys". The
module docstring keeps "P0-P26".

### 4.4 Fixture

The fixture `token`, `commit`, `base`, `push`, `pr` and `close` steps change from their short forms
to the pinned form. Each one is built from the pin constants, in the same way as `VERIFY_STEP`:

- `TOKEN_STEP` already exists and matches `TOKEN_WITH`. The fixture uses it.
- `COMMIT_STEP`, `BASE_STEP`, `PUSH_STEP`, `PR_STEP` and `CLOSE_STEP` are new. Each one is a head
  (`- id:`, `if:`, and the `env:` block if any) plus `        run: |\n` plus `_indent(<X>_RUN)`.
- The `env:` lines of the fixture are written in the same order as the workflow. The pin is a
  dict, so the order has no effect on the check.

So the base fixture equals the pin by construction. Row 1 ("the fixture passes") proves that.

The fixture `propose` job has no `timeout-minutes:` and no `name:` today. It stays so. Both keys
are on the allow-list, and their absence is allowed.

`PUSH_LINE` already equals the real push line. `PR_LIST` changes to the real `gh pr list` command
of `pr`, which includes `--repo "$GITHUB_REPOSITORY"`. It does NOT include the redirect
`> "$RUNNER_TEMP/open-prs.txt"`, so that it stays a substring that the rows can change. The same
line occurs in `close`. `self_test` replaces only the first match, so the `PR_LIST` rows change
`pr`, as they do today. These rows rely on the first match on purpose, and they stay outside
`ANCHORED_ROWS`.

### 4.5 Existing rows

A row that wants a rule id passes if any message starts with that id (`self_test`, `prefix`). So
an existing row that now also gives a P26 message still passes, and it still proves its own rule.
Only the anchors and the PASS rows need work.

**Anchor of the general propose rows.** 33 rows anchor on `UNPINNED_PROPOSE_RUN`, the one-line
`run:` of the fixture `commit` step. That line goes away. The new anchor is
`COMMIT_RUN_BLOCK = "        run: |\n" + _indent(COMMIT_RUN)`, which occurs once in the fixture
(its `git config user.name` line occurs nowhere else). Each row replaces this block with its
current `new` text. Rows that append a key (`env:`, `shell:`, `working-directory:`) append it after
the block. `UNPINNED_PROPOSE_RUN` is removed. One name has one meaning, and no propose step is
unpinned now.

**Every row that this section rewrites moves into `ANCHORED_ROWS`.** These are the 33
`COMMIT_RUN_BLOCK` rows, the two `CLOSE_RUN_BLOCK` rows, the `close` head row and the new `pr` row
below. `self_test` then asserts that the `old` of each one occurs exactly once in the fixture. Now
all ten steps are pinned, so a mutation of the wrong step also gives a P26 message. The count
check is what stops a row from testing a step other than the one it names.

**The new want `ONLY P26 <message start>`.** The want is the text `ONLY ` followed by the start of
one P26 message, for example `ONLY P26 the pr step script differs` or
`ONLY P26 the if of propose step close`. A row with this want passes when all three are true:

1. `violations()` gives at least one message.
2. Every message starts with `P26 `.
3. At least one message starts with the text after `ONLY `.

Condition 2 proves that the deny rules do not fire on the mutation. Condition 3 proves that the
mutation reached the step that the row names. `self_test` handles this want before the prefix rule.
The `#` comment above `SELF_TEST_ROWS` names the three want kinds: `PASS`, `ONLY <P26 message
start>`, and a rule id or the start of an exact message.

**Rows that change to `ONLY P26`:**

| Row | Today | Change |
|---|---|---|
| "the fixture passes, its PR body text says cargo update" | PASS, no replacement | Split. Row 1 stays `("the fixture passes", (), "PASS")`. A new row "a PR body text that says cargo update" replaces, in the whole `gh pr create` line of `pr` (workflow line 340, which occurs once), `--body-file "$RUNNER_TEMP/pr-body.md"` with `--body "run cargo update"`. Want `ONLY P26 the pr step script differs`. The `--body-file` text alone occurs three times (`verify`, `gh pr create`, `gh pr edit`), so it is never the anchor. |
| "a boolean if: false on a step" | PASS on `close` | Anchor on the `close` head, want `ONLY P26 the if of propose step close`. |
| "status as a whole last command" | PASS on `commit` | Anchor on `COMMIT_RUN_BLOCK`, want `ONLY P26 the commit step script differs`. |
| "gh api --method GET in propose" | PASS on `commit` | Anchor on `COMMIT_RUN_BLOCK`, want `ONLY P26 the commit step script differs`. |
| "a close step that lists with --head" | PASS on `close` | Anchor on `CLOSE_RUN_BLOCK`, want `ONLY P26 the close step script differs`. |

**The two `close` rows.** "a close step without a --head list" (P23) and "a close step that lists
with --head" anchor on the short `close` form. Both change to anchor on the whole `run:` block of
`close` (`CLOSE_RUN_BLOCK = "        run: |\n" + _indent(CLOSE_RUN)`), and replace it with the same
short scripts as today. The first row keeps want P23. The second row changes to `ONLY P26` as
above.

**Other anchors.** The P15 rows anchor on `      - id: push\n        if: <PUSH_IF>\n`. This text
stays in the new `PUSH_STEP`, so these rows keep their anchor. The P8 rows on `PUSH_LINE` and the
P21 and P23 rows on `PR_LIST` keep their anchors. The rows on `      - id: token\n` keep their
anchor.

### 4.6 New rows

All new rows go into `ANCHORED_ROWS`, so `self_test` asserts that each `old` occurs exactly once in
the fixture.

| # | Step | Anchor (`old`) | Mutation | Want |
|---|---|---|---|---|
| 1 | `commit` | the `git add` line, with its indentation | insert `echo x > rs/Cargo.lock` before it | P26 |
| 2 | `commit` | the `git add` line | `git add -- rs/Cargo.lock rs/crates/bindings/paigasus-wasm` to `git add -- .` | P26 |
| 3 | `commit` | as row 1 | as row 1 | `P26 the commit step script differs from the pinned text, first at line 4` |
| 4 | `base` | the `gh api … ref/heads/main` line of `base` | insert `git commit --amend --no-edit` before it | P26 |
| 5 | `base` | `          BUILT_ON: ${{ github.sha }}\n` | value to `${{ github.event.after }}` | P26 |
| 6 | `push` | `PUSH_LINE` with its indentation | insert `git commit --amend --no-edit` before it | P26 |
| 7 | `push` | `          PUSH_TOKEN: ${{ steps.token.outputs.token }}\n` | value to `${{ github.token }}` | P26 |
| 8 | `pr` | `      - id: pr\n        if: <PUSH_IF>\n` | `if:` without `&& steps.base.outputs.moved == 'false'` | P26 |
| 9 | `close` | `        if: needs.build.outputs.changed == 'false'\n        env:\n` | add the env key `GIT_DIR: /x` | P26 |
| 10 | `token` | `      - id: token\n` | add `if: false` | P26 |
| 11 | `token` | `      - id: token\n` | add `env: {NODE_OPTIONS: --require ./x.js}` | P26 |
| 12 | `token` | the `uses:` line of `token` | a different 40-hex SHA | PASS |
| 13 | `push` | `          PUSH_TOKEN: ${{ steps.token.outputs.token }}\n        run: \|\n` | `run: \|` to `run: \|-` | P26 |
| 14 | `token` | the `uses:` line of `token` | `actions/checkout@<40 hex>` | `P26 the propose step token must use actions/create-github-app-token` |
| 15 | job | `    environment: release-pr\n` | add `    concurrency: x\n` after it | `P26 the propose job declares concurrency` |
| 16 | job | `    environment: release-pr\n` | add `    strategy:\n      matrix:\n        n: [1, 2]\n` after it | `P26 the propose job declares strategy` |

Row 12 proves that a dependabot bump of the token action does not red P26. Row 3 proves the line
number for a step after `apply`. Row 14 is the only row that proves `PROPOSE_ACTIONS["token"]`,
because P16 also refuses another action.

These lines occur in more than one step, so they are never an anchor of a new row:
`set -euo pipefail`, `rc=0`, the `gh pr list` command, the `if:` of `push` and `pr` without the
`- id:` line, `GH_TOKEN: ${{ steps.token.outputs.token }}`, `"$RUNNER_TEMP/pr-title.txt"`,
`--body-file "$RUNNER_TEMP/pr-body.md"`, `        run: |` and `        env:` alone.

### 4.7 Negative control

Six new mutations of the real workflow, one for each new step, with the existing `step_run` helper
where it fits:

- `commit`: `echo x > rs/Cargo.lock` inserted before the `git add` line → P26.
- `base`: `git commit --amend --no-edit` inserted before the `gh api` line → P26.
- `push`: `git commit --amend --no-edit` inserted before the `git push` line → P26.
- `pr`: the `moved` condition removed from the `if:` → P26.
- `close`: `env: {"GIT_DIR": "/x"}` added → P26.
- `token`: `if: false` added → P26.

The summary count is counted already. It goes from 14 to 20.

### 4.8 README

In `ci/wasm-lockstep/README.md` (line numbers for `main` at `607412fa`):

1. The P3 row (line 112): remove "(its `env:` and `if:` are not checked)" for `token`. P26 now
   refuses both. State that the `name:` of `token` is not checked for a `secrets` read: P3 skips
   the `token` step, and P26 does not compare `name:` (section 8, D5).
2. The P26 row of the rule table (line 135): "Every `propose` step, and the `propose` job keys
   (SMA-739, SMA-740)."
3. P26 check 1 (lines 176-180): name all ten steps and the five new `run:` constants. For
   `checkout`, `download` and `token`, only the action name in `uses:` is compared. "If you edit
   one of these four steps" changes to "If you edit a `propose` step".
4. P26 check 2 (lines 181-183): "these steps" changes to "the ten steps".
5. P26 check 3 (line 184): "The `propose` job has no `env:`" changes to "The `propose` job has
   only the keys in `PROPOSE_JOB_KEYS`. So `env:`, `concurrency:` and `strategy:` are refused."
6. The paragraph at line 190 changes to this list. On the ten `propose` steps, these step-level
   checks only repeat the pin: P5, P8 (the `propose` part), P13 (step `shell:`), P14 (step
   `continue-on-error:`), P15, P16 (`token`), P17 (`checkout`), P18, P19 (step keys and scripts),
   P20, P21, P22 and P23. These checks still carry a control of their own: the P7 step order,
   P13 on a workflow `defaults:`, P14 on the job `continue-on-error:`, P12, and P3, P9 and P24 on
   step `name:` and on the job keys. The workflow-level part of P19 repeats the P25 env allow-list,
   not P26.
7. The trust model (lines 226-227): "which pins the steps up to the last checker" changes to
   "which pins every `propose` step". The sentence that names P15 states that P15 now only repeats
   the pin on `propose`.
8. "What the checks do not prove": remove the bullet "The steps after `apply` … SMA-740 tracks it"
   (lines 264-267).
9. The deny-rule limits that apply to `propose` commands are at lines 251-256 (`cp` from the work
   copy, `git -C`, GNU `sed e`, `git config <key>`, `GIT_CONFIG_COUNT`, `gh alias set`,
   `gh extension exec`) and at lines 271-272 (`gh api -iXPOST`, the P21 regex). Add one sentence
   after each of the two groups: "In `propose`, P26 refuses each of these changes, because it pins
   every step. They stay limits of the rules themselves, and they still apply to `build`." Where a
   limit applies only to `propose`, the sentence says "They stay limits of the rules themselves."
10. Replace the removed bullet with this residual: "P26 proves that the ten `propose` steps are the
    pinned text, except `name:` and the action SHA. The pushed tree also depends on the code of
    the three actions (R2; `token` also receives the App private key, after the checks), on the
    runner image (`runs-on:`), and on `lockstep_check.py` and `.gitattributes` on `main`. PR review
    is the control for the last two."

The implementer re-reads each line range before the edit, because an earlier edit in the same file
moves the lines after it.

## 5. Error handling

No new error path. The exit codes stay: 0 pass, 3 assertion, 2 infrastructure. A row whose `old`
is not in the fixture still fails with "the row tests nothing", and an anchored row whose `old`
occurs more than once still fails.

## 6. Testing

1. **Red first.** Add the fixture change and the new rows before the six new `PROPOSE_PINNED`
   entries, the `token` entry of `PROPOSE_ACTIONS` and the job-key allow-list. Run
   `python3 ci/wasm-lockstep/pin_check.py --self-test`, and record the `got` list of each new row.
   - Attack rows 1, 2, 4-11, 13, 15 and 16 must give `got == []`. This is the evidence that no
     deny rule catches the attack today.
   - Each `ONLY P26` row must fail with `got == []`. If an `ONLY P26` row passes in this state, the
     want kind is a test that cannot fail. Stop and fix it.
   - Row 14 gives P16 only, and so it fails.
2. Add the entries and the allow-list. Re-run the self-test: all rows pass.
3. **Delete-the-feature check.** Remove each of these, one at a time: the six new `PROPOSE_PINNED`
   entries, `PROPOSE_ACTIONS["token"]`, and the job-key allow-list. Before each removal, write down
   the set of rows and negative-control mutations that must red. Compare the result with that set.
   Restore the item.
4. `python3 ci/wasm-lockstep/pin_check.py .github/workflows/wasm-lockstep.yml` →
   `satisfies P0-P26`, rc 0. This proves that the five `run:` constants and the job-key allow-list
   match the workflow.
5. `python3 ci/wasm-lockstep/pin_check.py --negative-control .github/workflows/wasm-lockstep.yml`
   → `20 mutations, 0 failed`, rc 0.
6. `moon run repo:wasm-lockstep repo:ruff-ci`, and the full-graph `moon ci` from the root
   `CLAUDE.md` before the push.

## 7. Risks

- **Maintenance cost.** Each later edit to the `propose` text must change `pin_check.py` in the
  same commit, or `repo:wasm-lockstep` reds. This now includes the comments inside the `push`
  script and a new key on the `propose` job. This is the same cost as P25, and it is intended. The
  README states it.
- **Rows that prove less than they did.** A general propose row now gets a P26 message too. It
  still proves its own rule, because the want is a prefix match on its rule id. The `ONLY P26`
  want keeps the proof that a deny rule does not fire too much, and condition 3 keeps the proof of
  the step. The `ONLY P26` rows cannot see a change that stops a deny rule on a pinned step. The
  positive rows on the same step do that: the P20 rows on `commit`, the P23 `close` row, the P15
  rows and the P18 rows.

## 8. Decisions and open questions

- **D1. Pin all ten steps, not only the seven with `run:`.** Sven chose this on 2026-10-09. The
  `token` step then has no unpinned key, so the rule is "every `propose` step is pinned whole".
- **D2. The want `ONLY P26` for the PASS rows on `propose`** (approach 1). Rejected: delete the
  PASS rows (loses the proof that P20, P21 and P23 do not fire too much), and a flag that turns P26
  off for the self-test (a test-only path in `violations()`). Sven approved approach 1 on
  2026-10-09. The challenge added condition 3 (the message start) to the want. Without it, a row
  that changed the wrong step passed.
- **D3. One rule id.** P26 gets wider. No P27.
- **D4. An allow-list of the `propose` job keys.** Added after the challenge. Sven approved the spec
  as written at GATE 1 on 2026-10-09, so D4 is in. Without it, `concurrency:`, `strategy:`, `outputs:` and a key that GitHub adds
  later pass in silence. The challenger found no route through these keys that changes the tree
  today, so this is a fail-closed measure, not a fix of a known attack.
- **D5. The `name:` of `token` is not checked for a `secrets` read.** It is stated in the README.
  This gap existed before SMA-740. Nobody measured whether GitHub masks a secret in a step name.
- **Q1. YAML 1.1 and YAML 1.2.** P26 compares the PyYAML (YAML 1.1) parse. In a literal block,
  PyYAML reads U+0085 (NEL) as a line break. YAML 1.2 does not. If the GitHub parser keeps NEL as
  a normal character, bash can see one line where P26 sees two. Nobody measured the GitHub parser.
  A cheap measure is a P0 check that refuses U+0085, U+2028 and U+2029 in the raw workflow text.
  This spec does not include it. Sven approved the spec as written at GATE 1 on 2026-10-09. The check is out of scope for SMA-740.
  No follow-up issue exists yet.
- **Q2. Runners.** The SMA-739 reason for an unpinned `runs-on:` assumes only GitHub-hosted
  runners. If the repository has a self-hosted runner or a larger runner with a custom image,
  `runs-on:` can select a different system `git` configuration for `git add`. Sven gave no answer
  at GATE 1. This question stays open.

## 9. Challenge changelog

| Finding | Severity | Action |
|---|---|---|
| The split "cargo update" row changes `verify`, not `pr`; `ONLY P26` cannot see it | MAJOR | Folded in: anchor on the whole `gh pr create` line; every rewritten row moves into `ANCHORED_ROWS`; `ONLY P26` carries a message start (4.5, D2). |
| The "only repeat the pin" lists are wrong (P7, P13, P14, P3/P9/P24, P19) and disagree | MAJOR | Folded in: one corrected list in 4.8 item 6; section 3 refers to it. |
| Unpinned job keys (`strategy:`, `concurrency:`, `outputs:`) | MINOR | Folded in as a job-key allow-list (G2, 4.2, 4.3, rows 15-16, D4). Flagged for GATE 1. |
| The residual text says more than P26 proves | MINOR | Folded in: 4.8 item 10. |
| Anchors of rows 8, 9 and 13 not named; "never an anchor" list incomplete | MINOR | Folded in: the anchor column in 4.6 and the full list. |
| No row proves `PROPOSE_ACTIONS["token"]` | MINOR | Folded in: row 14, and 6.3. |
| The test plan is weaker than SMA-739 | MINOR | Folded in: `got == []` for attack rows and `ONLY P26` rows, and an expected set for each deletion (6.1, 6.3). |
| The negative control covers four of six steps | MINOR | Folded in: `base` and `pr` mutations (20). Rejected: a real-workflow expectation that a token SHA bump passes. `negative_control` expects only failures, and fixture row 12 runs the same code path. |
| README line ranges wrong; five stale passages | MINOR | Folded in: 4.8 items 1-10. |
| YAML 1.1 and 1.2 read NEL differently | QUESTION | Open: Q1, for Sven at GATE 1. |
| `PR_LIST` with or without the redirect | QUESTION | Folded in: without the redirect (4.4). |
| Self-hosted runners | QUESTION | Open: Q2, for Sven at GATE 1. |
| `name:` of `token` can read `secrets` | QUESTION | Accepted as a residual, stated in the README (D5). |
