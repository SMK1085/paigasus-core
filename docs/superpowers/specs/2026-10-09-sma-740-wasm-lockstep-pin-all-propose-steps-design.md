# SMA-740: pin every propose step of wasm-lockstep

- Linear: SMA-740
- Date: 2026-10-09
- Base: `main` at `607412fa` (SMA-739, PR 394, merged)
- Related: SMA-739 spec `docs/superpowers/specs/2026-10-06-sma-739-wasm-lockstep-propose-pins-design.md`,
  section 8 (D1, R1)

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
- G2. The self-test has fixture rows that prove G1 for each of the six new steps. The rows include
  the attacks in section 1.
- G3. Each existing self-test row keeps its purpose. A row that wants a rule id still proves that
  this rule fires. A row that proves that a deny rule does not fire too much keeps that proof.
- G4. `--negative-control` has mutations of the REAL workflow for the new steps.
- G5. The README no longer states the residual "the steps after `apply` are not pinned". It
  describes P26 for all ten steps.

## 3. Non-goals

- The workflow file does not change. Only `pin_check.py`, its fixture and the README change.
- The job-level keys of `propose` (`runs-on:`, `timeout-minutes:`, `name:`) are not pinned. The
  SMA-739 spec, section 3, gives the reason for `runs-on:`. `env:` stays refused (P26), and
  `defaults:`, `container:` and `services:` stay refused (P12).
- P5, P7, P8, P13, P14, P15, P18, P19, P20, P21, P22, P23 and P24 do not change and are not
  removed. On `propose`, they now only repeat the pin (section 4.6). They give a more specific
  message, and they stay as a second check if the pin changes.
- No new rule id. P26 gets wider, in the same way as SMA-739 made the pin of P25 wider.
- Residual R2 of SMA-739 (P6 accepts an "impostor commit" SHA) is not fixed.

## 4. Design

### 4.1 Approach

Use the SMA-739 method on the six remaining steps: pin the whole step mapping exactly, without
`name:` (and without `uses:` for `token`). A deny-list of shell forms cannot close the class
(SMA-739 spec, 4.1).

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
action-name check covers it.

The comment above the constants changes from "the propose steps up to the last checker" to "every
propose step". It keeps the sentence: change these constants WITH the workflow.

### 4.3 Check

`_propose_pin_violations` does not change. It already iterates `PROPOSE_PINNED`, so the new entries
are checked with the existing messages:

- `P26 the propose step <id> is missing`
- `P26 the propose step <id> has keys <sorted got>, expected <sorted want>`
- `P26 the propose step <id> must use <action>, not <uses>`
- `P26 the <id> step script differs from the pinned text, first at line N`
- `P26 the <key> of propose step <id> must be exactly <want>, not <got>`

Only its docstring changes ("up to the last checker" to "every propose step"). The module docstring
keeps "P0-P26".

### 4.4 Fixture

The fixture `token`, `commit`, `base`, `push`, `pr` and `close` steps change from their short forms
to the pinned form. Each one is built from the pin constants, in the same way as `VERIFY_STEP`:

- `TOKEN_STEP` already exists and matches `TOKEN_WITH`. The fixture uses it.
- `COMMIT_STEP`, `BASE_STEP`, `PUSH_STEP`, `PR_STEP` and `CLOSE_STEP` are new. Each one is a head
  (`- id:`, `if:`, and the `env:` block if any) plus `        run: |\n` plus `_indent(<X>_RUN)`.
- The `env:` lines of the fixture are written in the same order as the workflow. The pin is a
  dict, so the order has no effect on the check.

So the base fixture equals the pin by construction. Row 1 ("the fixture passes") proves that.

`PUSH_LINE` already equals the real push line. `PR_LIST` changes to the real `gh pr list` line of
`pr`, which includes `--repo "$GITHUB_REPOSITORY"`. The same line occurs in `close`. `self_test`
replaces only the first match, so the `PR_LIST` rows change `pr`, as they do today.

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

**The new want `ONLY P26`.** A row with this want passes when `violations()` gives at least one
message and every message starts with `P26 `. It proves that the deny rules do not fire on the
mutation, and that P26 does. `self_test` handles it before the prefix rule. The `#` comment above
`SELF_TEST_ROWS` names the three want kinds: `PASS`, `ONLY P26`, and a rule id or exact message.

**Rows that change to `ONLY P26`:**

| Row | Today | Change |
|---|---|---|
| "the fixture passes, its PR body text says cargo update" | PASS, no replacement | Split. Row 1 stays `("the fixture passes", (), "PASS")`. A new row replaces `--body-file "$RUNNER_TEMP/pr-body.md"` in the `gh pr create` line of `pr` with `--body "run cargo update"`, want `ONLY P26`. |
| "a boolean if: false on a step" | PASS on `close` | Anchor on the `close` head, want `ONLY P26`. |
| "status as a whole last command" | PASS on `commit` | Anchor on `COMMIT_RUN_BLOCK`, want `ONLY P26`. |
| "gh api --method GET in propose" | PASS on `commit` | Anchor on `COMMIT_RUN_BLOCK`, want `ONLY P26`. |
| "a close step that lists with --head" | PASS on `close` | Anchor on `CLOSE_RUN` lines, want `ONLY P26` (see below). |

**The two `close` rows.** "a close step without a --head list" (P23) and "a close step that lists
with --head" anchor on the short `close` form. Both change to anchor on the whole `run:` block of
`close` (`CLOSE_RUN_BLOCK`), and replace it with the same short scripts as today. The first row keeps
want P23. The second row changes to `ONLY P26`.

**Other anchors.** The P15 rows anchor on `      - id: push\n        if: <PUSH_IF>\n`. This text
stays in the new `PUSH_STEP`, so these rows keep their anchor. The P8 rows on `PUSH_LINE` and the
P21 and P23 rows on `PR_LIST` keep their anchors. The rows on `      - id: token\n` keep their
anchor.

### 4.6 New rows

All new rows go into `ANCHORED_ROWS`, so `self_test` asserts that each `old` occurs exactly once in
the fixture.

| # | Step | Mutation | Want |
|---|---|---|---|
| 1 | `commit` | insert `echo x > rs/Cargo.lock` before the `git add` line | P26 |
| 2 | `commit` | `git add -- rs/Cargo.lock rs/crates/bindings/paigasus-wasm` to `git add -- .` | P26 |
| 3 | `commit` | the exact message for row 1: `P26 the commit step script differs from the pinned text, first at line 4` | exact message |
| 4 | `base` | insert `git commit --amend --no-edit` before the `gh api … ref/heads/main` line | P26 |
| 5 | `base` | `BUILT_ON` env value changed to `${{ github.event.after }}` | P26 |
| 6 | `push` | insert `git commit --amend --no-edit` before the push line | P26 |
| 7 | `push` | `PUSH_TOKEN` env value changed to `${{ github.token }}` | P26 |
| 8 | `pr` | `if:` without `&& steps.base.outputs.moved == 'false'` | P26 |
| 9 | `close` | `env:` key `GIT_DIR: /x` added | P26 |
| 10 | `token` | `if: false` added | P26 |
| 11 | `token` | `env: {NODE_OPTIONS: --require ./x.js}` added | P26 |
| 12 | `token` | `uses:` with a different 40-hex SHA | PASS |
| 13 | `push` | `run: \|` to `run: \|-` | P26 |

Row 12 proves that a dependabot bump of the token action does not red P26. Row 3 proves the line
number for a step after `apply`. The anchor of rows 1 and 3 is the `git add` line with its
indentation, which occurs once. The anchor of row 6 is `PUSH_LINE` with its indentation; the row
inserts the amend line before it. A line that occurs in more than one step (`set -euo pipefail`,
`rc=0`, the `gh pr list` line) is never an anchor of a new row.

### 4.7 Negative control

Four new mutations of the real workflow, with the existing `step_run` helper where it fits:

- `git commit --amend --no-edit` inserted before the `git push` line of `push` → P26.
- `echo x > rs/Cargo.lock` inserted before the `git add` line of `commit` → P26.
- `if: false` on `token` → P26.
- `env: {"GIT_DIR": "/x"}` added to `close` → P26.

The summary count is counted already. It goes from 14 to 18.

### 4.8 README

In `ci/wasm-lockstep/README.md`:

- The P26 row of the rule table (line 135): "Every `propose` step (SMA-739, SMA-740)."
- P26 check 1 (lines 175-179): name all ten steps and the five new `run:` constants. For
  `checkout`, `download` and `token`, only the action name in `uses:` is compared. "If you edit
  one of these four steps" changes to "If you edit a `propose` step".
- The paragraph at line 190: add that on `propose`, P5, P8, P13, P14, P15, P19, P20, P21, P22,
  P23 and P24 now only repeat the P26 pin. P16 only repeats it on `token`. They give a clearer
  message and stay as a second check.
- The trust model, line 226: "which pins the steps up to the last checker" changes to "which pins
  every `propose` step".
- "What the checks do not prove": remove the bullet "The steps after `apply` … SMA-740 tracks it"
  (lines 264-267). Add after the deny-rule limits (`GIT_CONFIG_COUNT`, `gh alias set`,
  `gh extension exec`, `gh api -iXPOST`, the P21 regex) one sentence: "In `propose`, P26 refuses
  each of these changes, because it pins every step. They stay limits of the rules themselves."
- State the remaining residual: the actions `checkout`, `download` and `token` run code that P26
  does not see (P6, R2). A change to the action can change the tree. P26 proves only that the
  workflow text is the pinned text.

## 5. Error handling

No new error path. The exit codes stay: 0 pass, 3 assertion, 2 infrastructure. A row whose `old`
is not in the fixture still fails with "the row tests nothing", and an anchored row whose `old`
occurs more than once still fails.

## 6. Testing

1. **Red first.** Add the fixture change and the new rows before the new `PROPOSE_PINNED` entries.
   Run `python3 ci/wasm-lockstep/pin_check.py --self-test`. Record the `got` list of each new row. Rows 1-11 and 13 must
   fail because no message starts with `P26`, and each `ONLY P26` row must fail.
2. Add the six entries. Re-run the self-test: all rows pass.
3. **Delete-the-feature check.** Remove each new `PROPOSE_PINNED` entry, one at a time. Record which
   rows red for each. Every new entry must red at least one row. Restore it.
4. `python3 ci/wasm-lockstep/pin_check.py .github/workflows/wasm-lockstep.yml` →
   `satisfies P0-P26`, rc 0. This proves that the five `run:` constants equal the workflow text.
5. `python3 ci/wasm-lockstep/pin_check.py --negative-control .github/workflows/wasm-lockstep.yml`
   → `18 mutations, 0 failed`, rc 0.
6. `moon run repo:wasm-lockstep repo:ruff-ci`, and the full-graph `moon ci` from the root
   `CLAUDE.md` before the push.

## 7. Risks

- **Maintenance cost.** Each later edit to the `propose` text must change `pin_check.py` in the
  same commit, or `repo:wasm-lockstep` reds. This now includes the comments inside the `push`
  script and the `close` comment text. This is the same cost as P25, and it is intended. The
  README states it.
- **Rows that prove less than they did.** A general propose row now gets a P26 message too. It
  still proves its own rule, because the want is a prefix match on its rule id. The `ONLY P26` want
  keeps the proof that a deny rule does not fire too much.

## 8. Decisions

- **D1. Pin all ten steps, not only the seven with `run:`.** Sven chose this on 2026-10-09. The
  `token` step then has no unpinned key, so the rule is "every `propose` step is pinned whole".
- **D2. The want `ONLY P26` for the PASS rows on `propose`** (approach 1). Rejected: delete the
  PASS rows (loses the proof that P20, P21 and P23 do not fire too much), and a flag that turns P26
  off for the self-test (a test-only path in `violations()`). Sven approved approach 1 on
  2026-10-09.
- **D3. One rule id.** P26 gets wider. No P27.
