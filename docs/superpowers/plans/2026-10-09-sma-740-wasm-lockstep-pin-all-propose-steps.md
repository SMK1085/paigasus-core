# SMA-740 wasm-lockstep pin of every propose step Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `pin_check.py` refuses any change to any of the ten `propose` steps of `.github/workflows/wasm-lockstep.yml` (other than `name:` and the action SHA of `checkout`, `download` and `token`), and any `propose` job key that is not on an allow-list. Both are part of rule P26.

**Architecture:** One file holds the rules, the fixture, the self-test and the negative control: `ci/wasm-lockstep/pin_check.py`. Task 1 adds the five `run:` pins, rebuilds the fixture from them, moves the general propose rows onto the pinned `commit` step, adds the `ONLY <P26 message start>` want kind and the new rows. It runs red first, then adds the six `PROPOSE_PINNED` entries, the `token` action and the job-key allow-list. Task 2 adds six mutations of the real workflow. Task 3 updates the README and runs the full verification.

**Tech Stack:** Python 3.12, PyYAML, uv (`ci/wasm-lockstep/pyproject.toml`), moon task `repo:wasm-lockstep`, ruff (`repo:ruff-ci`).

**Spec:** `docs/superpowers/specs/2026-10-09-sma-740-wasm-lockstep-pin-all-propose-steps-design.md`

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740`. Use absolute paths.
- The workflow `.github/workflows/wasm-lockstep.yml` must NOT change.
- Every source file keeps its SPDX header (`# SPDX-License-Identifier: Apache-2.0`). No new file in `ci/`.
- Exit codes stay: 0 pass, 3 assertion, 2 infrastructure. No new rule id: P26 gets wider.
- Put `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` and `export PROTO_REPORTER=text` first in every command block that runs `uv` or `moon`.
- Run the checker through uv: `uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py …`.
- `S` is your session scratchpad directory. Set it at the top of each command block that uses it. Put every helper script there, never in the repo.
- If the sandbox refuses a compound command, write it to a script file in `$S` and run `/bin/bash "$S/<script>.sh"`.
- ruff (`py/pyproject.toml`, `repo:ruff-ci`) must pass: line-length 200, `RUF` on. Use `*NAME,` in a literal, never `tuple + tuple` (RUF005).
- Commits: conventional, scope `ci`, subject ends with `(SMA-740)`, for example `fix(ci): … (SMA-740)`. The message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `#NNN` line in the body.
- Write the message to `$S/msg.txt` and commit with `git commit -F "$S/msg.txt"`. A multi-line `-m` is refused in this sandbox.
- Do NOT use `--no-verify`. Do NOT use `git commit --amend` together with `git reset`. Do NOT restore with `git checkout --`, `git restore` or `git stash`.
- Mutation checks run on COPIES of `pin_check.py` in `$S`. Do not mutate the repo file to prove a check.
- Do NOT install host software. Do NOT start background jobs.
- Messages relayed from the user are for the coordinator, not for you.

## Review Focus

1. **An `ONLY` want that cannot fail.** If the `ONLY` branch of `self_test` accepts an empty `got`, a deny message, or a message of the wrong step, the five `ONLY` rows prove nothing. Owned by Task 1: Step 12 (every `ONLY` row is red with `got == []` before the pins) and Step 17 (two harness mutations: a deny message added, and the wrong step named; each must make exactly one row red).
2. **A row that changes a step other than the one it names.** `self_test` replaces only the FIRST match, and lines such as `set -euo pipefail` and the `gh pr list` command occur in several steps. Owned by Task 1: every rewritten or new row is in `ANCHORED_ROWS`, Step 11 checks each anchor occurs once, and the row "a PR body text that says cargo update" anchors on the whole `gh pr create` line.
3. **A `run:` constant that is not byte-equal to the PyYAML load.** The `push` script holds comment lines, `\'` escapes and a `'"status":"404"'` argument. An error makes the real workflow red, or a wrong fixture passes. Owned by Task 1: Step 3 (a direct compare with `yaml.safe_load`) and Step 15 (`satisfies P0-P26`).
4. **An f-string that collapses `${{` into `${`.** A fixture piece written as an f-string with `${{ github.sha }}` loads as `${ github.sha }`, so the fixture differs from the pin. Owned by Task 1: the fixture pieces use plain strings for every `${{ }}` line, and the row "the fixture passes" (PASS) reds on any such error.
5. **The job-key allow-list on a non-string key, and on the two old job-env rows.** `sorted()` on a job with the key `on:` (it loads as `True`) raises `TypeError`. The rows "a propose job env PATH" (P26) and "ENV in a job env" (P19) must stay green. Owned by Task 1: the new row "an on: key on the propose job" (want `P26 the propose job declares True`), Step 15, and the variant `nostr` in Step 17.

---

### Task 1: Pin the six remaining propose steps and the propose job keys

**Files:**
- Modify: `ci/wasm-lockstep/pin_check.py`
  - constants: ~lines 126-152 (the P26 comment to the end of `PROPOSE_PINNED`), and `TOKEN_WITH` ~lines 163-168
  - `_propose_pin_violations`: ~lines 687-695
  - fixture pieces: ~lines 815-823
  - `FIXTURE` propose tail: ~lines 876-909
  - row constants: ~lines 911-925
  - `ANCHORED_ROWS`: ~lines 933-965
  - comment above `SELF_TEST_ROWS` and the rows: ~lines 968-1147
  - `self_test`: ~lines 1173-1176

**Interfaces:**
- Consumes (existing): `VERIFY_RUN`, `APPLY_RUN`, `PROPOSE_IF`, `_indent`, `ANCHORED_ROWS`, `ANCHORED_LABELS`, `PUSH_LINE`, `_first_diff_line`.
- Produces (module constants, used by Task 2 and Task 3):
  - `COMMIT_RUN`, `BASE_RUN`, `PUSH_RUN`, `PR_RUN`, `CLOSE_RUN: str` — the pinned `run:` texts.
  - `PUSH_IF: str`, `TOKEN_OUT: str`, `PROPOSE_JOB_KEYS: frozenset[str]`.
  - `TOKEN_WITH` moves above `PROPOSE_PINNED` (same value).
  - `PROPOSE_ACTIONS["token"] = "actions/create-github-app-token"`; `PROPOSE_PINNED` has ten entries.
  - Fixture pieces: `TOKEN_HEAD`, `TOKEN_USES`, `TOKEN_STEP`, `GH_TOKEN_OUT_LINE`, `COMMIT_RUN_BLOCK`, `COMMIT_STEP`, `BUILT_ON_LINE`, `BASE_STEP`, `PUSH_HEAD`, `PUSH_TOKEN_LINE`, `PUSH_STEP`, `PR_HEAD`, `PR_STEP`, `CLOSE_IF_LINE`, `CLOSE_HEAD`, `CLOSE_RUN_BLOCK`, `CLOSE_STEP`, `GIT_ADD_LINE`, `BASE_API_LINE`, `PR_CREATE_LINE`.
  - The want kind `ONLY <P26 message start>` in `self_test`.
  - The message `P26 the propose job declares <key>`, one for each key not in `PROPOSE_JOB_KEYS`, in sorted order.
- Removes: `UNPINNED_PROPOSE_RUN`. `PR_LIST` changes its value (it now holds `--repo "$GITHUB_REPOSITORY"`).

- [ ] **Step 1: Record the baseline**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --self-test 2>&1 | tail -1
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py .github/workflows/wasm-lockstep.yml
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --negative-control .github/workflows/wasm-lockstep.yml 2>&1 | tail -1
```

Expected:
```text
pin_check self-test: 201 rows, 0 failed
pin_check: .github/workflows/wasm-lockstep.yml satisfies P0-P26
pin_check negative control: 14 mutations, 0 failed
```

- [ ] **Step 2: Replace the P26 constants block**

Replace the text from the line `# P26 (SMA-739): the propose steps up to the last checker, as PyYAML loads them from the workflow.` (~line 126) to the closing `}` of `PROPOSE_PINNED` (~line 152), inclusive, with this block. `PROPOSE_ACTIONS` and `PROPOSE_PINNED` stay as they are in this step (red first).

```python
# P26 (SMA-739, SMA-740): every propose step, as PyYAML loads it from the workflow. A pin of `run:`
# alone is not enough: `if: false` or `env: {SHELLOPTS: noexec}` also skips a checker, and a step
# after the checkers can change the tree again (`git add` of another path, `git commit --amend`).
# So each step is pinned whole, without `name` (and without `uses` for the three action steps,
# which P6 checks, so that a dependabot bump stays green). Change these WITH the workflow.
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
COMMIT_RUN = "\n".join((
    'set -euo pipefail',
    'git config user.name "paigasusbot[bot]"',
    'git config user.email "285361405+paigasusbot[bot]@users.noreply.github.com"',
    'git add -- rs/Cargo.lock rs/crates/bindings/paigasus-wasm',
    'git commit -F "$RUNNER_TEMP/pr-title.txt"',
)) + "\n"
BASE_RUN = "\n".join((
    'set -euo pipefail',
    'rc=0',
    'gh api "repos/${GITHUB_REPOSITORY}/git/ref/heads/main" --jq .object.sha > "$RUNNER_TEMP/main-sha.txt" || rc=$?',
    'if [ "$rc" -ne 0 ]; then',
    '  cat "$RUNNER_TEMP/main-sha.txt"',
    '  echo "::error::gh api could not read refs/heads/main (exit ${rc})."',
    '  exit 1',
    'fi',
    'now="$(cat "$RUNNER_TEMP/main-sha.txt")"',
    'if [ "${#now}" -ne 40 ]; then',
    '  echo "::error::gh api returned \'${now}\', not a commit SHA."',
    '  exit 1',
    'fi',
    'if [ "$now" != "$BUILT_ON" ]; then',
    '  echo "moved=true" >> "$GITHUB_OUTPUT"',
    '  echo "::notice::main moved from ${BUILT_ON} to ${now}. Nothing was pushed; the next run proposes on the new base."',
    'else',
    '  echo "moved=false" >> "$GITHUB_OUTPUT"',
    'fi',
)) + "\n"
PUSH_RUN = "\n".join((
    'set -euo pipefail',
    'rc=0',
    'gh api "repos/${GITHUB_REPOSITORY}/git/ref/heads/deps/wasm-bindgen-lockstep" --jq .object.sha > "$RUNNER_TEMP/branch-sha.txt" || rc=$?',
    'if [ "$rc" -eq 0 ]; then',
    '  lease="$(cat "$RUNNER_TEMP/branch-sha.txt")"',
    '  author="$(gh api "repos/${GITHUB_REPOSITORY}/commits/${lease}" --jq .author.login)"',
    "  # .author.login comes from the commit's author EMAIL, which a pusher can set. This check",
    '  # guards against a mistake (a person pushed to the bot branch), not against an attacker',
    '  # with push access.',
    '  if [ "$author" != "paigasusbot[bot]" ]; then',
    '    echo "::error::A person pushed to the bot branch (head ${lease} by \'${author}\'). Merge or close that pull request, or delete the branch deps/wasm-bindgen-lockstep, first."',
    '    exit 1',
    '  fi',
    'elif grep -qF \'"status":"404"\' "$RUNNER_TEMP/branch-sha.txt"; then',
    '  lease=""',
    'else',
    '  cat "$RUNNER_TEMP/branch-sha.txt"',
    '  echo "::error::gh api could not read the bot branch (exit ${rc})."',
    '  exit 1',
    'fi',
    'git push --force-with-lease="refs/heads/deps/wasm-bindgen-lockstep:${lease}" "https://x-access-token:${PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git" HEAD:refs/heads/deps/wasm-bindgen-lockstep',
)) + "\n"
PR_RUN = "\n".join((
    'set -euo pipefail',
    'gh pr list --repo "$GITHUB_REPOSITORY" --head deps/wasm-bindgen-lockstep --state open --json number,isCrossRepository --jq \'.[] | select(.isCrossRepository | not) | .number\' > "$RUNNER_TEMP/open-prs.txt"',
    'number="$(sed -n 1p "$RUNNER_TEMP/open-prs.txt")"',
    'if [ -z "$number" ]; then',
    '  gh pr create --repo "$GITHUB_REPOSITORY" --base main --head deps/wasm-bindgen-lockstep --title "$(cat "$RUNNER_TEMP/pr-title.txt")" --body-file "$RUNNER_TEMP/pr-body.md"',
    'else',
    '  gh pr edit "$number" --repo "$GITHUB_REPOSITORY" --title "$(cat "$RUNNER_TEMP/pr-title.txt")" --body-file "$RUNNER_TEMP/pr-body.md"',
    'fi',
)) + "\n"
CLOSE_RUN = "\n".join((
    'set -euo pipefail',
    'gh pr list --repo "$GITHUB_REPOSITORY" --head deps/wasm-bindgen-lockstep --state open --json number,isCrossRepository --jq \'.[] | select(.isCrossRepository | not) | .number\' > "$RUNNER_TEMP/open-prs.txt"',
    'while read -r number; do',
    '  gh pr close "$number" --repo "$GITHUB_REPOSITORY" --comment "The wasm-bindgen family is current on main. This proposal is obsolete."',
    'done < "$RUNNER_TEMP/open-prs.txt"',
)) + "\n"
PROPOSE_IF = "needs.build.outputs.changed == 'true'"
PUSH_IF = "needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'"
TOKEN_OUT = "${{ steps.token.outputs.token }}"
TOKEN_WITH = {
    "client-id": "${{ secrets.PAIGASUS_BOT_APP_ID }}",
    "private-key": "${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}",
    "permission-contents": "write",
    "permission-pull-requests": "write",
}
# P26 (SMA-740): the keys that the propose job may declare. A new key (concurrency:, strategy:,
# outputs:, or a key that GitHub adds later) fails closed. The values of name:, runs-on: and
# timeout-minutes: are not pinned. P10 and P11 check the values of the other keys.
PROPOSE_JOB_KEYS = frozenset({"name", "needs", "if", "runs-on", "timeout-minutes", "environment", "permissions", "steps"})
PROPOSE_ACTIONS = {"checkout": "actions/checkout", "download": "actions/download-artifact"}
PROPOSE_PINNED = {
    "checkout": {"id": "checkout", "if": PROPOSE_IF, "with": {"ref": "${{ github.sha }}", "persist-credentials": False}},
    "download": {"id": "download", "if": PROPOSE_IF, "with": {"name": "wasm-lockstep", "path": "${{ runner.temp }}/lockstep"}},
    "verify": {"id": "verify", "if": PROPOSE_IF, "run": VERIFY_RUN},
    "apply": {"id": "apply", "if": PROPOSE_IF, "run": APPLY_RUN},
}
```

Then delete the old `TOKEN_WITH = {…}` block (six lines, directly before `REFSPEC = …`, ~line 163). `TOKEN_WITH` must now occur exactly once as a definition:

```bash
grep -c '^TOKEN_WITH = {' /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740/ci/wasm-lockstep/pin_check.py   # expected: 1
```

- [ ] **Step 3: Prove the five run constants equal the PyYAML load**

Write `$S/compare_runs.py`:

```python
import sys

import yaml

sys.path.insert(0, "/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740/ci/wasm-lockstep")
import pin_check as p  # noqa: E402

doc = yaml.safe_load(open("/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740/.github/workflows/wasm-lockstep.yml", encoding="utf-8"))
steps = {s["id"]: s for s in doc["jobs"]["propose"]["steps"]}
for sid, const in (("commit", p.COMMIT_RUN), ("base", p.BASE_RUN), ("push", p.PUSH_RUN), ("pr", p.PR_RUN), ("close", p.CLOSE_RUN)):
    print(sid, "equal" if steps[sid]["run"] == const else f"DIFFERS at line {p._first_diff_line(steps[sid]['run'], const)}")
print("job keys", "equal" if set(doc["jobs"]["propose"]) <= p.PROPOSE_JOB_KEYS else sorted(set(doc["jobs"]["propose"]) - p.PROPOSE_JOB_KEYS))
```

Run:
```bash
S=<your scratchpad>
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
uv run --locked --project ci/wasm-lockstep python3 "$S/compare_runs.py"
```

Expected:
```text
commit equal
base equal
push equal
pr equal
close equal
job keys equal
```

If a line says `DIFFERS`, fix the constant, not the workflow.

- [ ] **Step 4: Replace the fixture pieces**

Replace this block (~lines 815-823):

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

with this block. Keep the one blank line after it, before `FIXTURE = """\`.

```python
# SMA-739 P26: the pinned propose steps of the fixture, as pieces, so that the rows can name them.
ARTIFACT_LINE = VERIFY_RUN.split("\n")[1]
STATUS_LINE = APPLY_RUN.split("\n")[-2]
VERIFY_HEAD = f"      - id: verify\n        if: {PROPOSE_IF}\n"
APPLY_HEAD = f"      - id: apply\n        if: {PROPOSE_IF}\n"
VERIFY_STEP = VERIFY_HEAD + "        run: |\n" + _indent(VERIFY_RUN)
APPLY_STEP = APPLY_HEAD + "        run: |\n" + _indent(APPLY_RUN)
PROPOSE_CHECKOUT_USES = f"        if: {PROPOSE_IF}\n        uses: actions/checkout@" + "1" * 40 + "\n"
# SMA-740 P26: the other six propose steps, built from the pin constants, so that the base fixture
# equals the pin. Plain strings, not f-strings, hold the `${{ }}` lines: an f-string reads `{{` as `{`.
TOKEN_HEAD = "      - id: token\n"
TOKEN_USES = "        uses: actions/create-github-app-token@" + "4" * 40 + "\n"
TOKEN_STEP = TOKEN_HEAD + TOKEN_USES + """\
        with:
          client-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}
          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}
          permission-contents: write
          permission-pull-requests: write
"""
GH_TOKEN_OUT_LINE = "          GH_TOKEN: " + TOKEN_OUT + "\n"
COMMIT_RUN_BLOCK = "        run: |\n" + _indent(COMMIT_RUN)
COMMIT_STEP = f"      - id: commit\n        if: {PROPOSE_IF}\n" + COMMIT_RUN_BLOCK
BUILT_ON_LINE = "          BUILT_ON: ${{ github.sha }}\n"
BASE_STEP = "".join((
    f"      - id: base\n        if: {PROPOSE_IF}\n",
    "        env:\n",
    "          GH_TOKEN: ${{ github.token }}\n",
    BUILT_ON_LINE,
    "        run: |\n",
    _indent(BASE_RUN),
))
PUSH_HEAD = f"      - id: push\n        if: {PUSH_IF}\n"
PUSH_TOKEN_LINE = "          PUSH_TOKEN: " + TOKEN_OUT + "\n"
PUSH_STEP = PUSH_HEAD + "        env:\n" + GH_TOKEN_OUT_LINE + PUSH_TOKEN_LINE + "        run: |\n" + _indent(PUSH_RUN)
PR_HEAD = f"      - id: pr\n        if: {PUSH_IF}\n"
PR_STEP = PR_HEAD + "        env:\n" + GH_TOKEN_OUT_LINE + "        run: |\n" + _indent(PR_RUN)
CLOSE_IF_LINE = "        if: needs.build.outputs.changed == 'false'\n"
CLOSE_HEAD = "      - id: close\n" + CLOSE_IF_LINE
CLOSE_RUN_BLOCK = "        run: |\n" + _indent(CLOSE_RUN)
CLOSE_STEP = CLOSE_HEAD + "        env:\n" + GH_TOKEN_OUT_LINE + CLOSE_RUN_BLOCK
```

- [ ] **Step 5: Replace the propose tail of `FIXTURE`**

In `FIXTURE`, replace this text (~lines 876-909, from the `VERIFY_STEP` join to the closing `"""` of `FIXTURE`):

```text
""" + VERIFY_STEP + APPLY_STEP + """\
      - id: token
        uses: actions/create-github-app-token@""" + "4" * 40 + """
        with:
          client-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}
          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}
          permission-contents: write
          permission-pull-requests: write
      - id: commit
        if: needs.build.outputs.changed == 'true'
        run: git commit -F "$RUNNER_TEMP/pr-title.txt"
      - id: base
        if: needs.build.outputs.changed == 'true'
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          now="$(gh api "repos/${GITHUB_REPOSITORY}/git/ref/heads/main" --jq .object.sha)"
          echo "moved=false" >> "$GITHUB_OUTPUT"
      - id: push
        if: needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'
        env:
          PUSH_TOKEN: ${{ steps.token.outputs.token }}
        run: |
          git push --force-with-lease="refs/heads/deps/wasm-bindgen-lockstep:${lease}" "https://x-access-token:${PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git" HEAD:refs/heads/deps/wasm-bindgen-lockstep
      - id: pr
        if: needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'
        run: |
          gh pr list --head deps/wasm-bindgen-lockstep --state open --json number,isCrossRepository --jq '.[] | select(.isCrossRepository | not) | .number'
          gh pr create --title "$(cat "$RUNNER_TEMP/pr-title.txt")" --body "run cargo update"
      - id: close
        if: needs.build.outputs.changed == 'false'
        run: |
          gh pr list --head deps/wasm-bindgen-lockstep --state open --json number,isCrossRepository --jq '.[] | select(.isCrossRepository | not) | .number'
"""
```

with this one line:

```python
""" + VERIFY_STEP + APPLY_STEP + TOKEN_STEP + COMMIT_STEP + BASE_STEP + PUSH_STEP + PR_STEP + CLOSE_STEP
```

- [ ] **Step 6: Replace the row constants after `FIXTURE`**

Replace this block (~lines 911-925, from `PUSH_LINE = …` to `BUILD_RUN = …`):

```python
PUSH_LINE = 'git push --force-with-lease="refs/heads/deps/wasm-bindgen-lockstep:${lease}" "https://x-access-token:${PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git" HEAD:refs/heads/deps/wasm-bindgen-lockstep'
DOCKER_LINE = 'docker run --rm --cap-drop=ALL --security-opt=no-new-privileges --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE"'
TOKEN_STEP = "      - id: token\n        uses: actions/create-github-app-token@" + "4" * 40 + """
        with:
          client-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}
          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}
          permission-contents: write
          permission-pull-requests: write
"""
PR_LIST = "gh pr list --head deps/wasm-bindgen-lockstep --state open --json number,isCrossRepository --jq '.[] | select(.isCrossRepository | not) | .number'"
BUILD_STEP_ANCHOR = "      - id: update\n"
# The general propose rows run on the UNPINNED commit step. The pinned steps (P26) would add a P26
# message to every row, and a PASS row would fail.
UNPINNED_PROPOSE_RUN = '        run: git commit -F "$RUNNER_TEMP/pr-title.txt"\n'
BUILD_RUN = "container.sh update\n"  # the update step: it is not pinned by P25
```

with:

```python
PUSH_LINE = 'git push --force-with-lease="refs/heads/deps/wasm-bindgen-lockstep:${lease}" "https://x-access-token:${PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git" HEAD:refs/heads/deps/wasm-bindgen-lockstep'
DOCKER_LINE = 'docker run --rm --cap-drop=ALL --security-opt=no-new-privileges --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE"'
# The gh pr list command of the pr and close steps, without its redirect. self_test replaces only
# the FIRST match, so the PR_LIST rows change the pr step. They stay outside ANCHORED_ROWS.
PR_LIST = "gh pr list --repo \"$GITHUB_REPOSITORY\" --head deps/wasm-bindgen-lockstep --state open --json number,isCrossRepository --jq '.[] | select(.isCrossRepository | not) | .number'"
BUILD_STEP_ANCHOR = "      - id: update\n"
BUILD_RUN = "container.sh update\n"  # the update step: it is not pinned by P25
# SMA-740: lines of the pinned steps that occur once in FIXTURE, with their fixture indentation.
GIT_ADD_LINE = "          " + COMMIT_RUN.split("\n")[3] + "\n"
BASE_API_LINE = "          " + BASE_RUN.split("\n")[2] + "\n"
PR_CREATE_LINE = "          " + PR_RUN.split("\n")[4] + "\n"
```

`PR_CREATE_LINE` has twelve leading spaces: ten from the fixture block and two from the `if` body.

- [ ] **Step 7: Remove the 36 rows that move into `ANCHORED_ROWS`**

The 33 rows that hold `UNPINNED_PROPOSE_RUN`, the row "a boolean if: false on a step" and the two `close` rows (two source lines each) leave `SELF_TEST_ROWS`. Step 8 adds them again, rewritten, to `ANCHORED_ROWS`. The first row changes to `("the fixture passes", (), "PASS"),`.

Write `$S/move_rows.py`:

```python
"""SMA-740 Task 1 Step 7: remove from SELF_TEST_ROWS the rows that move into ANCHORED_ROWS."""
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as handle:
    lines = handle.read().split("\n")
start = lines.index("SELF_TEST_ROWS: tuple[tuple[str, tuple[tuple[str, str], ...], str], ...] = (")
two_line = ('    ("a close step without a --head list"', '    ("a close step that lists with --head"')
out, removed, renamed = lines[:start + 1], 0, 0
i = start + 1
while i < len(lines):
    line = lines[i]
    if "UNPINNED_PROPOSE_RUN" in line or line.startswith('    ("a boolean if: false on a step"'):
        removed += 1
        i += 1
        continue
    if line.startswith(two_line):
        removed += 1
        i += 2
        continue
    if line.startswith('    ("the fixture passes, its PR body text says cargo update"'):
        line = '    ("the fixture passes", (), "PASS"),'
        renamed += 1
    out.append(line)
    i += 1
assert (removed, renamed) == (36, 1), (removed, renamed)
with open(path, "w", encoding="utf-8") as handle:
    handle.write("\n".join(out))
print(f"removed {removed} rows, renamed {renamed}")
```

Run:
```bash
S=<your scratchpad>
python3 "$S/move_rows.py" /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740/ci/wasm-lockstep/pin_check.py
grep -c 'UNPINNED_PROPOSE_RUN' /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740/ci/wasm-lockstep/pin_check.py
```

Expected: `removed 36 rows, renamed 1`, then `0`.

- [ ] **Step 8: Add the moved, rewritten and new rows to `ANCHORED_ROWS`**

`ANCHORED_ROWS` ends with the row `("the propose download step runs checkout", …),` and then `)` (~line 965). Insert this block directly before that `)`. Labels of the moved rows do not change. Only the two `PASS` rows on `commit` change their want to `ONLY …`, and the four rewritten rows change their anchor.

```python
    # SMA-740: the general propose rows, moved from SELF_TEST_ROWS. They run on the pinned commit step.
    ("a secrets read in propose outside the token step", ((COMMIT_RUN_BLOCK, COMMIT_RUN_BLOCK + "        env:\n          K: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}\n"),), "P3"),
    ("a cargo command in propose", ((COMMIT_RUN_BLOCK, "        run: cargo update -p wasm-bindgen\n"),), "P5"),
    ("cargo inside $(...) in propose", ((COMMIT_RUN_BLOCK, '        run: echo "$(cargo metadata)"\n'),), "P5"),
    ("a backtick in propose", ((COMMIT_RUN_BLOCK, "        run: echo `id`\n"),), "P5"),
    ("bash -c in propose", ((COMMIT_RUN_BLOCK, "        run: bash -c 'cargo build'\n"),), "P5"),
    ("python3 -c in propose", ((COMMIT_RUN_BLOCK, "        run: python3 -c 'print(1)'\n"),), "P5"),
    ("env as a command wrapper in propose", ((COMMIT_RUN_BLOCK, "        run: env cargo build\n"),), "P5"),
    ("cargo after a pipe in propose", ((COMMIT_RUN_BLOCK, "        run: cat a | cargo build\n"),), "P5"),
    ("a second git push", ((COMMIT_RUN_BLOCK, "        run: git push origin HEAD:refs/heads/x\n"),), "P8"),
    ("needs.build.outputs.x in a run in propose", ((COMMIT_RUN_BLOCK, '        run: echo "${{ needs.build.outputs.changed }}"\n'),), "P9"),
    ("needs['build'] in an env in propose", ((COMMIT_RUN_BLOCK, COMMIT_RUN_BLOCK + "        env:\n          C: ${{ needs['build'].outputs.changed }}\n"),), "P9"),
    ("shell: python on a propose step", ((COMMIT_RUN_BLOCK, COMMIT_RUN_BLOCK + "        shell: python\n"),), "P13"),
    ("status as a whole last command", ((COMMIT_RUN_BLOCK, "        run: |\n          set -euo pipefail\n          git status > s\n          python3 ci/wasm-lockstep/lockstep_check.py status --file s\n"),), "ONLY P26 the commit step script differs"),
    ("a process substitution in propose: cat <(...)", ((COMMIT_RUN_BLOCK, "        run: cat <(cargo build)\n"),), "P5"),
    ("a process substitution in propose: > >(...)", ((COMMIT_RUN_BLOCK, "        run: echo x > >(cargo build)\n"),), "P5"),
    ("working-directory on a propose step", ((COMMIT_RUN_BLOCK, COMMIT_RUN_BLOCK + "        working-directory: /tmp\n"),), "P19"),
    ("gh api -X POST in propose", ((COMMIT_RUN_BLOCK, "        run: gh api repos/x/y -X POST\n"),), "P20"),
    ("gh api --method=DELETE in propose", ((COMMIT_RUN_BLOCK, "        run: gh api repos/x/y --method=DELETE\n"),), "P20"),
    ("gh api -XPUT in propose", ((COMMIT_RUN_BLOCK, "        run: gh api repos/x/y -XPUT\n"),), "P20"),
    ("gh api -f in propose (an implicit POST)", ((COMMIT_RUN_BLOCK, "        run: gh api repos/x/y -f a=b\n"),), "P20"),
    ("gh api graphql in propose", ((COMMIT_RUN_BLOCK, "        run: gh api graphql\n"),), "P20"),
    ("gh api --method GET in propose", ((COMMIT_RUN_BLOCK, "        run: gh api repos/x/y --method GET\n"),), "ONLY P26 the commit step script differs"),
    ("git -c before the subcommand in propose", ((COMMIT_RUN_BLOCK, "        run: git -c core.fsmonitor=x status\n"),), "P20"),
    ("git -ckey=value in propose", ((COMMIT_RUN_BLOCK, "        run: git -ccore.pager=x log\n"),), "P20"),
    ("sudo chown in propose", ((COMMIT_RUN_BLOCK, '        run: sudo chown -R 65534:65534 "$RUNNER_TEMP/work"\n'),), "P5"),
    ("needs in an env key named if in propose", ((COMMIT_RUN_BLOCK, COMMIT_RUN_BLOCK + "        env:\n          if: ${{ needs.build.outputs.changed }}\n"),), "P9"),
    ("gh pr merge in propose", ((COMMIT_RUN_BLOCK, '        run: gh pr merge "$number" --squash --auto\n'),), "P22"),
    ("gh workflow run in propose", ((COMMIT_RUN_BLOCK, "        run: gh workflow run x.yml\n"),), "P22"),
    ("gh repo delete in propose", ((COMMIT_RUN_BLOCK, "        run: gh repo delete x\n"),), "P22"),
    ("gh secret set in propose", ((COMMIT_RUN_BLOCK, "        run: gh secret set X\n"),), "P22"),
    ("steps.build.outputs in a host step of propose", ((COMMIT_RUN_BLOCK, '        run: echo "${{ steps.build.outputs.v }}"\n'),), "P24"),
    ("steps.update.outputs in an env of propose", ((COMMIT_RUN_BLOCK, COMMIT_RUN_BLOCK + "        env:\n          V: ${{ steps.update.outputs.v }}\n"),), "P24"),
    ("STEPS.Update.Outputs in another case in propose", ((COMMIT_RUN_BLOCK, '        run: echo "${{ STEPS.Update.Outputs.v }}"\n'),), "P24"),
    # SMA-740: rows on the steps after apply. All ten propose steps are pinned now, so a mutation of
    # the wrong step also gives a P26 message. The count check of self_test keeps each row on its step.
    ("a PR body text that says cargo update", ((PR_CREATE_LINE, PR_CREATE_LINE.replace('--body-file "$RUNNER_TEMP/pr-body.md"', '--body "run cargo update"')),), "ONLY P26 the pr step script differs"),
    ("a boolean if: false on a step", ((CLOSE_HEAD, "      - id: close\n        if: false\n"),), "ONLY P26 the if of propose step close"),
    ("a close step without a --head list", ((CLOSE_RUN_BLOCK, "        run: |\n          gh pr close 1\n"),), "P23"),
    ("a close step that lists with --head", ((CLOSE_RUN_BLOCK, "        run: |\n          " + PR_LIST + "\n          gh pr close 1\n"),), "ONLY P26 the close step script differs"),
    ("echo into rs/Cargo.lock before git add in commit", ((GIT_ADD_LINE, "          echo x > rs/Cargo.lock\n" + GIT_ADD_LINE),), "P26"),
    ("git add -- . in commit", ((GIT_ADD_LINE, "          git add -- .\n"),), "P26"),
    ("echo into rs/Cargo.lock before git add, by the exact message", ((GIT_ADD_LINE, "          echo x > rs/Cargo.lock\n" + GIT_ADD_LINE),), "P26 the commit step script differs from the pinned text, first at line 4"),
    ("git commit --amend before the gh api call of base", ((BASE_API_LINE, "          git commit --amend --no-edit\n" + BASE_API_LINE),), "P26"),
    ("BUILT_ON reads github.event.after in base", ((BUILT_ON_LINE, "          BUILT_ON: ${{ github.event.after }}\n"),), "P26"),
    ("git commit --amend before the push line", ((f"          {PUSH_LINE}\n", f"          git commit --amend --no-edit\n          {PUSH_LINE}\n"),), "P26"),
    ("PUSH_TOKEN reads github.token in push", ((PUSH_TOKEN_LINE, "          PUSH_TOKEN: ${{ github.token }}\n"),), "P26"),
    ("the pr if: without the moved condition", ((PR_HEAD, f"      - id: pr\n        if: {PROPOSE_IF}\n"),), "P26"),
    ("env GIT_DIR on close", ((CLOSE_IF_LINE + "        env:\n", CLOSE_IF_LINE + "        env:\n          GIT_DIR: /x\n"),), "P26"),
    ("if: false on token", ((TOKEN_HEAD, TOKEN_HEAD + "        if: false\n"),), "P26"),
    ("env NODE_OPTIONS on token", ((TOKEN_HEAD, TOKEN_HEAD + "        env:\n          NODE_OPTIONS: --require ./x.js\n"),), "P26"),
    ("a dependabot SHA bump of the token action", ((TOKEN_USES, TOKEN_USES.replace("4" * 40, "6" * 40)),), "PASS"),
    ("run: |- on push", ((PUSH_TOKEN_LINE + "        run: |\n", PUSH_TOKEN_LINE + "        run: |-\n"),), "P26"),
    ("the token step runs checkout", ((TOKEN_USES, "        uses: actions/checkout@" + "4" * 40 + "\n"),), "P26 the propose step token must use actions/create-github-app-token"),
    ("concurrency on the propose job", (("    environment: release-pr\n", "    environment: release-pr\n    concurrency: x\n"),), "P26 the propose job declares concurrency"),
    ("strategy on the propose job", (("    environment: release-pr\n", "    environment: release-pr\n    strategy:\n      matrix:\n        n: [1, 2]\n"),), "P26 the propose job declares strategy"),
    ("an on: key on the propose job", (("    environment: release-pr\n", "    environment: release-pr\n    on: x\n"),), "P26 the propose job declares True"),
```

The new rows map to spec 4.6 as follows: rows 1-16 are the rows from "echo into rs/Cargo.lock before git add in commit" to "strategy on the propose job", in spec order (row 12 is "a dependabot SHA bump of the token action", row 13 is "run: |- on push", row 14 is "the token step runs checkout"). The last row is Review Focus 5. Rows that add a key after a `run: |` block (`env:`, `shell:`, `working-directory:`) keep the eight-space key indentation, so the YAML stays valid.

- [ ] **Step 9: Add the `ONLY` want kind to `self_test`**

In `self_test` (~line 1173), replace:

```python
        got = _rules_of(text)
        # A `want` with a space is the start of one exact message; else it is a rule id.
        prefix = want if " " in want else want + " "
        ok = not got if want == "PASS" else any(v.startswith(prefix) for v in got)
```

with:

```python
        got = _rules_of(text)
        if want.startswith("ONLY "):
            # ONLY <P26 message start>: the deny rules stay silent (every message is P26), and the
            # mutation reached the step that the row names (one message starts with the text).
            start = want.removeprefix("ONLY ")
            ok = bool(got) and all(v.startswith("P26 ") for v in got) and any(v.startswith(start) for v in got)
        else:
            # A `want` with a space is the start of one exact message; else it is a rule id.
            prefix = want if " " in want else want + " "
            ok = not got if want == "PASS" else any(v.startswith(prefix) for v in got)
```

The three conditions are spec 4.5 conditions 1, 2 and 3, in this order.

- [ ] **Step 10: Name the three want kinds above `SELF_TEST_ROWS`**

Replace the line:

```python
# (label, (old, new) replacements on FIXTURE, rule id that must appear or "PASS")
```

with:

```python
# (label, (old, new) replacements on FIXTURE, want). The want has one of three kinds:
#   PASS                      no rule fires.
#   ONLY <P26 message start>  every message is P26, and one message starts with this text.
#   a rule id, or the start of one exact message (a want with a space): one message starts with it.
```

- [ ] **Step 11: Check that each anchor of `ANCHORED_ROWS` occurs once**

Write `$S/anchors.py`:

```python
import sys

sys.path.insert(0, "/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740/ci/wasm-lockstep")
import pin_check as p  # noqa: E402

bad = [(label, p.FIXTURE.count(old)) for label, reps, _w in p.ANCHORED_ROWS for old, _n in reps if p.FIXTURE.count(old) != 1]
print("anchored rows:", len(p.ANCHORED_ROWS), "self-test rows:", len(p.SELF_TEST_ROWS), "bad:", bad)
```

Run:
```bash
S=<your scratchpad>
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
uv run --locked --project ci/wasm-lockstep python3 "$S/anchors.py"
```

Expected: `anchored rows: 90 self-test rows: 219 bad: []` (36 SMA-739 rows, 33 moved, 4 rewritten, 17 new).

- [ ] **Step 12: Run the self-test RED and compare with the expected red state**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --self-test 2>&1 | grep -v '^  ok '
```

Expected: exactly these 21 `FAIL` lines, then `pin_check self-test: 219 rows, 21 failed`.

- The five `ONLY` rows, each with `got []`: "status as a whole last command", "gh api --method GET in propose", "a PR body text that says cargo update", "a boolean if: false on a step", "a close step that lists with --head". If an `ONLY` row passes here, the want kind cannot fail. Stop and fix Step 9.
- The attack rows, each with `got []` (spec rows 1, 2, 4-11, 13, 15, 16 and the `on:` row): "echo into rs/Cargo.lock before git add in commit", "git add -- . in commit", "git commit --amend before the gh api call of base", "BUILT_ON reads github.event.after in base", "git commit --amend before the push line", "PUSH_TOKEN reads github.token in push", "the pr if: without the moved condition", "env GIT_DIR on close", "if: false on token", "env NODE_OPTIONS on token", "run: |- on push", "concurrency on the propose job", "strategy on the propose job", "an on: key on the propose job". This is the evidence that no deny rule catches these attacks today.
- Spec row 3, "echo into rs/Cargo.lock before git add, by the exact message", with `got []`.
- Spec row 14, "the token step runs checkout", with `got` = one `P17 jobs.propose.steps[4] (token): a checkout must set persist-credentials: false` message and one `P16 the token step must use actions/create-github-app-token …` message. (The spec says "P16 only". P17 also fires, because the step now runs `actions/checkout` without `persist-credentials: false`. The row still fails, as the spec requires.)
- "a dependabot SHA bump of the token action" (PASS) is NOT in the list.

Every other row, the moved rows included, passes. If any other row fails, stop: a fixture piece or an anchor is wrong.

- [ ] **Step 13: Add the six `PROPOSE_PINNED` entries and the `token` action**

Replace:

```python
PROPOSE_ACTIONS = {"checkout": "actions/checkout", "download": "actions/download-artifact"}
PROPOSE_PINNED = {
    "checkout": {"id": "checkout", "if": PROPOSE_IF, "with": {"ref": "${{ github.sha }}", "persist-credentials": False}},
    "download": {"id": "download", "if": PROPOSE_IF, "with": {"name": "wasm-lockstep", "path": "${{ runner.temp }}/lockstep"}},
    "verify": {"id": "verify", "if": PROPOSE_IF, "run": VERIFY_RUN},
    "apply": {"id": "apply", "if": PROPOSE_IF, "run": APPLY_RUN},
}
```

with:

```python
PROPOSE_ACTIONS = {"checkout": "actions/checkout", "download": "actions/download-artifact", "token": "actions/create-github-app-token"}
PROPOSE_PINNED = {
    "checkout": {"id": "checkout", "if": PROPOSE_IF, "with": {"ref": "${{ github.sha }}", "persist-credentials": False}},
    "download": {"id": "download", "if": PROPOSE_IF, "with": {"name": "wasm-lockstep", "path": "${{ runner.temp }}/lockstep"}},
    "verify": {"id": "verify", "if": PROPOSE_IF, "run": VERIFY_RUN},
    "apply": {"id": "apply", "if": PROPOSE_IF, "run": APPLY_RUN},
    "token": {"id": "token", "with": TOKEN_WITH},
    "commit": {"id": "commit", "if": PROPOSE_IF, "run": COMMIT_RUN},
    "base": {"id": "base", "if": PROPOSE_IF, "env": {"GH_TOKEN": "${{ github.token }}", "BUILT_ON": "${{ github.sha }}"}, "run": BASE_RUN},
    "push": {"id": "push", "if": PUSH_IF, "env": {"GH_TOKEN": TOKEN_OUT, "PUSH_TOKEN": TOKEN_OUT}, "run": PUSH_RUN},
    "pr": {"id": "pr", "if": PUSH_IF, "env": {"GH_TOKEN": TOKEN_OUT}, "run": PR_RUN},
    "close": {"id": "close", "if": "needs.build.outputs.changed == 'false'", "env": {"GH_TOKEN": TOKEN_OUT}, "run": CLOSE_RUN},
}
```

The `token` entry has `with`, so the existing `drop` rule removes `uses` from the key compare, and the action check reads `PROPOSE_ACTIONS["token"]`.

- [ ] **Step 14: Replace the job `env:` check with the job-key allow-list, and update the docstring**

In `_propose_pin_violations` (~line 687), replace:

```python
    """P26 (SMA-739 spec 4.4). The propose steps up to the last checker are pinned whole, without
    `name` (and without `uses` for the action steps, which P6 checks). A pin of `run:` alone is not
    enough: `if: false` or `env: {SHELLOPTS: noexec}` also skips the checker, and a checkout `ref:`
    change runs a checker from a different tree. The propose job may not declare env."""
    out = []
    if "env" in propose:
        out.append("P26 the propose job declares env")
```

with:

```python
    """P26 (SMA-739 spec 4.4, SMA-740 spec 4.3). Every propose step is pinned whole, without `name`
    (and without `uses` for the action steps, which P6 checks), and the job keys are an allow-list.
    A pin of `run:` alone is not enough: `if: false` or `env: {SHELLOPTS: noexec}` also skips a
    checker, a checkout `ref:` change runs a checker from a different tree, and a step after the
    checkers can change the tree again. The propose job declares only the keys in PROPOSE_JOB_KEYS."""
    # str(): a bare `on:` key loads as True, and sorted() refuses a mix of bool and str.
    out = [f"P26 the propose job declares {key}" for key in sorted(map(str, propose)) if key not in PROPOSE_JOB_KEYS]
```

For `env` the message is the same as before, so the row "a propose job env PATH" does not change.

- [ ] **Step 15: Run green: self-test, real run, negative control, ruff**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --self-test 2>&1 | grep -v '^  ok '
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py .github/workflows/wasm-lockstep.yml; echo "rc=$?"
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --negative-control .github/workflows/wasm-lockstep.yml 2>&1 | tail -1
uv run --locked --project py ruff check --config py/pyproject.toml --no-cache ci/wasm-lockstep/pin_check.py
```

Expected:
```text
pin_check self-test: 219 rows, 0 failed
pin_check: .github/workflows/wasm-lockstep.yml satisfies P0-P26
rc=0
pin_check negative control: 14 mutations, 0 failed
All checks passed!
```

The row "a propose job env PATH" (P26) and "ENV in a job env" (P19) are both `ok`.

- [ ] **Step 16: Commit**

Write `$S/msg.txt`:

```text
fix(ci): pin every wasm-lockstep propose step and the propose job keys (SMA-740)

P26 now pins the steps token, commit, base, push, pr and close whole, as
it already pinned checkout, download, verify and apply. A step after the
checkers can no longer write rs/Cargo.lock again, git add another path,
or amend the commit. The propose job keys are an allow-list, so a new
key such as concurrency: or strategy: fails closed.

The general propose self-test rows move onto the pinned commit step and
into ANCHORED_ROWS. A new want kind, ONLY <P26 message start>, keeps the
proof that the deny rules do not fire too much on the pinned steps.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

```bash
S=<your scratchpad>
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740
git add ci/wasm-lockstep/pin_check.py
git commit -F "$S/msg.txt"
git status --short
```

Expected: the commit succeeds (commitlint passes), and `git status --short` prints nothing.

- [ ] **Step 17: Delete-the-feature check (spec 6.3) on scratch copies**

Each item below is removed from a COPY of the committed `pin_check.py` in `$S`. The repo file does not change. Before you run, read the expected set of red rows for each item. After the run, compare. A difference is a finding: stop and report it.

| Variant | What is removed or changed | Self-test rows that must be red (and only these) |
|---|---|---|
| `token` | the `"token"` entry of `PROPOSE_PINNED` | "if: false on token", "env NODE_OPTIONS on token", "the token step runs checkout" |
| `commit` | the `"commit"` entry | "status as a whole last command", "gh api --method GET in propose", "echo into rs/Cargo.lock before git add in commit", "git add -- . in commit", "echo into rs/Cargo.lock before git add, by the exact message" |
| `base` | the `"base"` entry | "git commit --amend before the gh api call of base", "BUILT_ON reads github.event.after in base" |
| `push` | the `"push"` entry | "git commit --amend before the push line", "PUSH_TOKEN reads github.token in push", "run: \|- on push" |
| `pr` | the `"pr"` entry | "a PR body text that says cargo update", "the pr if: without the moved condition" |
| `close` | the `"close"` entry | "a boolean if: false on a step", "a close step that lists with --head", "env GIT_DIR on close" |
| `action` | `PROPOSE_ACTIONS["token"]` | "the token step runs checkout" |
| `allow_none` | the job-key check (the list becomes `out = []`) | "a propose job env PATH", "concurrency on the propose job", "strategy on the propose job", "an on: key on the propose job" |
| `allow_env` | the job-key check, put back to the SMA-739 `env`-only check | "concurrency on the propose job", "strategy on the propose job", "an on: key on the propose job" |
| `nostr` | `sorted(map(str, propose))` becomes `sorted(propose)` | none: the self-test stops with `TypeError: '<' not supported between instances of 'bool' and 'str'` (rc 1) |
| `only_deny` | harness: the row "gh api --method GET in propose" also runs `cargo x` | "gh api --method GET in propose" (condition 2: a P5 message is present) |
| `only_step` | harness: the same row wants `ONLY P26 the pr step script differs` | "gh api --method GET in propose" (condition 3: no message of the pr step) |

In every variant, the real run still prints `satisfies P0-P26`. "the fixture passes" and "a dependabot SHA bump of the token action" stay `ok` in every variant.

Write `$S/variants.py`:

```python
"""SMA-740: write the delete-the-feature variants of pin_check.py into the scratchpad."""
import sys

src_path, out_dir = sys.argv[1], sys.argv[2]
with open(src_path, encoding="utf-8") as handle:
    src = handle.read()
CHECK = '    out = [f"P26 the propose job declares {key}" for key in sorted(map(str, propose)) if key not in PROPOSE_JOB_KEYS]\n'


def variant(name: str, old: str, new: str) -> None:
    assert src.count(old) == 1, (name, src.count(old))
    with open(f"{out_dir}/v_{name}.py", "w", encoding="utf-8") as handle:
        handle.write(src.replace(old, new))


for sid in ("token", "commit", "base", "push", "pr", "close"):
    line = next(ln for ln in src.splitlines(keepends=True) if ln.startswith(f'    "{sid}": {{"id": "{sid}"'))
    variant(sid, line, "")
variant("action", ', "token": "actions/create-github-app-token"}', "}")
variant("allow_none", CHECK, "    out = []\n")
variant("allow_env", CHECK, '    out = []\n    if "env" in propose:\n        out.append("P26 the propose job declares env")\n')
variant("nostr", "sorted(map(str, propose))", "sorted(propose)")
variant("only_deny", '"        run: gh api repos/x/y --method GET\\n"', '"        run: |\\n          gh api repos/x/y --method GET\\n          cargo x\\n"')
variant("only_step", '"        run: gh api repos/x/y --method GET\\n"),), "ONLY P26 the commit step script differs"',
        '"        run: gh api repos/x/y --method GET\\n"),), "ONLY P26 the pr step script differs"')
print("12 variants written")
```

Write `$S/run_variants.sh`:

```bash
#!/bin/bash
# usage: /bin/bash run_variants.sh <scratchpad> --self-test|--negative-control
S="$1"
MODE="$2"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740 || exit 2
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
W=.github/workflows/wasm-lockstep.yml
for v in token commit base push pr close action allow_none allow_env nostr only_deny only_step; do
  echo "=== $v"
  if [ "$MODE" = --self-test ]; then
    uv run --locked --project ci/wasm-lockstep python3 "$S/v_$v.py" --self-test 2>&1 | grep -E '^  FAIL|rows,|Error'
  else
    uv run --locked --project ci/wasm-lockstep python3 "$S/v_$v.py" --negative-control "$W" 2>&1 | grep -E '^  FAIL|mutations,'
  fi
  uv run --locked --project ci/wasm-lockstep python3 "$S/v_$v.py" "$W" 2>&1 | tail -1
done
```

Run:
```bash
S=<your scratchpad>
python3 "$S/variants.py" /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740/ci/wasm-lockstep/pin_check.py "$S"
/bin/bash "$S/run_variants.sh" "$S" --self-test
```

Compare each `=== <variant>` section with the table. The `FAIL` labels must be exactly the expected set. The failed count is 3, 5, 2, 3, 2, 3, 1, 4, 3, (TypeError), 1, 1.

- [ ] **Step 18: Confirm the repo is unchanged by Step 17**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740
git status --short
```

Expected: no output.

---

### Task 2: Six negative-control mutations of the real workflow

**Files:**
- Modify: `ci/wasm-lockstep/pin_check.py`, `negative_control` (~line 1254 before Task 1, now later: directly after the line `expect("SHELLOPTS: noexec in the propose job env", noexec, "P26")`).

**Interfaces:**
- Consumes: `step_run` (the existing local helper in `negative_control`), `expect`, `real`, `copy`, `PROPOSE_IF`.
- Produces: a local helper `propose_step(step_id, change) -> dict`, and six mutations. The summary line becomes `pin_check negative control: 20 mutations, 0 failed`.

- [ ] **Step 1: Add the six mutations**

Re-read `negative_control` first. Directly after:

```python
    expect("SHELLOPTS: noexec in the propose job env", noexec, "P26")
```

insert:

```python
    # SMA-740: P26 on the REAL steps after apply, one mutation for each step.
    def propose_step(step_id: str, change) -> dict:
        mutated = copy.deepcopy(real)
        for step in mutated["jobs"]["propose"]["steps"]:
            if step.get("id") == step_id:
                change(step)
        return mutated

    expect("echo into rs/Cargo.lock before the git add line of commit", step_run("propose", "commit", lambda ln: "echo x > rs/Cargo.lock\n" + ln if ln.startswith("git add ") else ln), "P26")
    expect("git commit --amend before the gh api line of base", step_run("propose", "base", lambda ln: "git commit --amend --no-edit\n" + ln if "/git/ref/heads/main" in ln else ln), "P26")
    expect("git commit --amend before the git push line", step_run("propose", "push", lambda ln: "git commit --amend --no-edit\n" + ln if ln.startswith("git push ") else ln), "P26")
    expect("the moved condition removed from the pr if:", propose_step("pr", lambda s: s.update({"if": PROPOSE_IF})), "P26")
    expect("env GIT_DIR added to the close step", propose_step("close", lambda s: s.setdefault("env", {}).update({"GIT_DIR": "/x"})), "P26")
    expect("if: false on the token step", propose_step("token", lambda s: s.update({"if": False})), "P26")
```

`step_run` splits the loaded `run:` text into lines with no indentation, so `ln.startswith("git add ")` and `ln.startswith("git push ")` match the real lines.

- [ ] **Step 2: Run the negative control and ruff**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --negative-control .github/workflows/wasm-lockstep.yml 2>&1 | tail -7
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --self-test 2>&1 | tail -1
uv run --locked --project py ruff check --config py/pyproject.toml --no-cache ci/wasm-lockstep/pin_check.py
```

Expected:
```text
  ok    echo into rs/Cargo.lock before the git add line of commit: P26
  ok    git commit --amend before the gh api line of base: P26
  ok    git commit --amend before the git push line: P26
  ok    the moved condition removed from the pr if:: P26
  ok    env GIT_DIR added to the close step: P26
  ok    if: false on the token step: P26
pin_check negative control: 20 mutations, 0 failed
pin_check self-test: 219 rows, 0 failed
All checks passed!
```

- [ ] **Step 3: Commit**

Write `$S/msg.txt`:

```text
test(ci): add wasm-lockstep negative-control mutations for the steps after apply (SMA-740)

Six mutations of the real workflow prove that P26 bites on each of the
steps token, commit, base, push, pr and close. The count goes from 14
to 20.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

```bash
S=<your scratchpad>
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740
git add ci/wasm-lockstep/pin_check.py
git commit -F "$S/msg.txt"
git status --short
```

Expected: the commit succeeds, and `git status --short` prints nothing.

- [ ] **Step 4: Prove each new mutation bites (spec 6.3, the negative-control half)**

Re-generate the variants from the new commit, then run the negative control on each:

```bash
S=<your scratchpad>
python3 "$S/variants.py" /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740/ci/wasm-lockstep/pin_check.py "$S"
/bin/bash "$S/run_variants.sh" "$S" --negative-control
```

Read the expected set first. Each line gives the only mutation that must be red, and the summary:

| Variant | Red mutation | Summary |
|---|---|---|
| `token` | "if: false on the token step" | `20 mutations, 1 failed` |
| `commit` | "echo into rs/Cargo.lock before the git add line of commit" | `20 mutations, 1 failed` |
| `base` | "git commit --amend before the gh api line of base" | `20 mutations, 1 failed` |
| `push` | "git commit --amend before the git push line" | `20 mutations, 1 failed` |
| `pr` | "the moved condition removed from the pr if:" | `20 mutations, 1 failed` |
| `close` | "env GIT_DIR added to the close step" | `20 mutations, 1 failed` |
| `action` | none | `20 mutations, 0 failed` |
| `allow_none` | "SHELLOPTS: noexec in the propose job env" | `20 mutations, 1 failed` |
| `allow_env` | none | `20 mutations, 0 failed` |
| `nostr` | none | `20 mutations, 0 failed` |
| `only_deny` | none | `20 mutations, 0 failed` |
| `only_step` | none | `20 mutations, 0 failed` |

The real run of every variant prints `satisfies P0-P26`. A difference from the table is a finding: stop and report it. Then check `git status --short` prints nothing.

---

### Task 3: README, and the full verification

**Files:**
- Modify: `ci/wasm-lockstep/README.md` (line numbers for `607412fa`: 112, 135, 174-191, 222-227, 251-256, 264-267, 271-272).

**Interfaces:**
- Consumes: the names from Task 1 (`PROPOSE_PINNED`, `PROPOSE_JOB_KEYS`, `COMMIT_RUN`, `BASE_RUN`, `PUSH_RUN`, `PR_RUN`, `CLOSE_RUN`, `VERIFY_RUN`, `APPLY_RUN`).
- Produces: README text only. No code change.

The README uses ASD-STE100 Simplified Technical English: short sentences, active voice, approved words, no idiom. Each edit below is an exact old-to-new replacement. An earlier edit moves the lines after it, so re-read the area of each edit before you make it. The Edit tool matches text, not line numbers.

- [ ] **Step 1: The P3 row (spec 4.8 item 1)**

Old:
```text
| P3 | `build` declares no environment and reads no `secrets` context. The workflow level reads none. `propose` reads it only in the step `token` (its `env:` and `if:` are not checked). |
```
New:
```text
| P3 | `build` declares no environment and reads no `secrets` context. The workflow level reads none. `propose` reads it only in the step `token`. P26 refuses an `env:` or an `if:` on `token`. P3 skips the `token` step, and P26 does not compare `name:`. So nothing checks the `name:` of `token` for a `secrets` read. |
```

- [ ] **Step 2: The P26 row (item 2)**

Old:
```text
| P26 | The `propose` steps up to the last checker (SMA-739). The checks are in the list below the P25 list. |
```
New:
```text
| P26 | Every `propose` step, and the `propose` job keys (SMA-739, SMA-740). The checks are in the list below the P25 list. |
```

- [ ] **Step 3: P26 checks 1, 2 and 3 (items 3, 4 and 5)**

Old:
```text
1. The exact pin. The steps `checkout`, `download`, `verify` and `apply` of `propose` equal
   `PROPOSE_PINNED` in `pin_check.py`, key by key. `name:` is not compared. For `checkout` and `download`, only the action
   name in `uses:` is compared, not the SHA. P6 checks the SHA, so a dependabot bump stays green. The
   `run:` text of `verify` equals `VERIFY_RUN`, and of `apply` equals `APPLY_RUN`. If you edit one
   of these four steps, change `pin_check.py` in the same commit.
2. An extra key on these steps is refused. So `env:` (for example `SHELLOPTS: noexec`), `shell:`,
   `if: false` and a changed `with:` (for example a checkout `ref:` of a pull request head) are
   refused.
3. The `propose` job has no `env:`.
```
New:
```text
1. The exact pin. The ten `propose` steps (`checkout`, `download`, `verify`, `apply`, `token`,
   `commit`, `base`, `push`, `pr` and `close`) equal `PROPOSE_PINNED` in `pin_check.py`, key by
   key. `name:` is not compared. For `checkout`, `download` and `token`, only the action name in
   `uses:` is compared, not the SHA. P6 checks the SHA, so a dependabot bump stays green. The
   `run:` text of each step equals its constant: `VERIFY_RUN`, `APPLY_RUN`, `COMMIT_RUN`,
   `BASE_RUN`, `PUSH_RUN`, `PR_RUN` and `CLOSE_RUN`. The comment lines in the `push` script are
   part of the text. If you edit a `propose` step, change `pin_check.py` in the same commit.
2. An extra key on the ten steps is refused. So `env:` (for example `SHELLOPTS: noexec`), `shell:`,
   `if: false` and a changed `with:` (for example a checkout `ref:` of a pull request head) are
   refused.
3. The `propose` job has only the keys in `PROPOSE_JOB_KEYS`. So `env:`, `concurrency:` and
   `strategy:` are refused. The values of `name:`, `runs-on:` and `timeout-minutes:` are not
   checked. If you add a key to the `propose` job, change `pin_check.py` in the same commit.
```

- [ ] **Step 4: The list of checks that only repeat the pin (item 6)**

Old:
```text
On `verify` and `apply`, P7's `artifact` check and P18 only repeat the P26 pin. They give a
clearer message. P14 and P13 on these two steps also only repeat it.
```
New:
```text
On the ten `propose` steps, these step-level checks only repeat the P26 pin. They give a clearer
message:

- P5, P8 (the `propose` part), P13 (step `shell:`), P14 (step `continue-on-error:`), P15, P16
  (`token`), P17 (`checkout`), P18, P19 (step keys and scripts), P20, P21, P22 and P23.

These checks still carry a control of their own:

- The P7 step order.
- P13 on a workflow `defaults:`.
- P14 on the job `continue-on-error:`, and P12. On `build`, only these rules refuse the keys. On
  `propose`, the P26 job-key allow-list also refuses them.
- P3, P9 and P24 on a step `name:` and on the job keys.

The workflow-level part of P19 repeats the P25 `env:` allow-list, not P26.
```

- [ ] **Step 5: The trust model (item 7)**

Old:
```text
  never runs. This depends on P26, which pins the steps up to the last checker, on P15, which refuses a status
  function in a step `if:`, and on the P25 workflow `env:` allow-list.
```
New:
```text
  never runs. This depends on P26, which pins every `propose` step, and on the P25 workflow `env:`
  allow-list. P15 refuses a status function in a step `if:`. On `propose`, P15 now only repeats
  the P26 pin.
```

- [ ] **Step 6: The deny-rule limits, first group (item 9)**

Old:
```text
- The `GIT_CONFIG_COUNT` environment variable, `gh alias set` and `gh extension exec` pass the
  `propose` allowlist.
```
New:
```text
- The `GIT_CONFIG_COUNT` environment variable, `gh alias set` and `gh extension exec` pass the
  `propose` allowlist.
- In `propose`, P26 refuses each change in the four bullets above, because it pins every step.
  They stay limits of the rules themselves. The first two bullets still apply to `build`.
```

(The four bullets are: `cp` and `git -C`; `tar --checkpoint-action` and GNU `sed e`; `git config <key>`; `GIT_CONFIG_COUNT`, `gh alias set` and `gh extension exec`. `build` has `cp`, `git`, `tar` and `sed` on its allowlist. The last two bullets name `propose` only.)

- [ ] **Step 7: Replace the residual bullet (items 8 and 10)**

Old:
```text
- The steps after `apply` (`commit`, `base`, `push`, `pr`, `close`) are not pinned. They can
  change the tree after the checks: write `rs/Cargo.lock` again, `git add` another path, or run
  `git commit --amend`. P26 proves that the workflow text runs the two checkers unchanged. It does not prove that the pushed
  tree is the checked tree. SMA-740 tracks it.
```
New:
```text
- P26 proves that the ten `propose` steps are the pinned text, except `name:` and the action SHA.
  The pushed tree also depends on the code of the three actions (R2; `token` also receives the App
  private key, after the checks), on the runner image (`runs-on:`), and on `lockstep_check.py` and
  `.gitattributes` on `main`. PR review is the control for the last two.
```

- [ ] **Step 8: The deny-rule limits, second group (item 9)**

Old:
```text
- A bundled short flag, such as `gh api -iXPOST`, passes P20.
- P21 matches the jq `select` with a regular expression. It does not parse the jq.
```
New:
```text
- A bundled short flag, such as `gh api -iXPOST`, passes P20.
- P21 matches the jq `select` with a regular expression. It does not parse the jq.
- In `propose`, P26 refuses each of these two changes, because it pins every step. They stay
  limits of the rules themselves.
```

- [ ] **Step 9: Check that no stale text is left**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740
grep -n 'up to the last checker\|SMA-740 tracks\|these four steps\|The `propose` job has no\|not checked)' ci/wasm-lockstep/README.md ci/wasm-lockstep/pin_check.py
grep -c 'SMA-740' ci/wasm-lockstep/README.md
```

Expected: the first `grep` prints nothing. The second prints `1` (the P26 row).

- [ ] **Step 10: Run the gate tasks**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --self-test 2>&1 | tail -1
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py .github/workflows/wasm-lockstep.yml; echo "rc=$?"
uv run --locked --project ci/wasm-lockstep python3 ci/wasm-lockstep/pin_check.py --negative-control .github/workflows/wasm-lockstep.yml 2>&1 | tail -1
moon run repo:wasm-lockstep repo:ruff-ci
```

Expected:
```text
pin_check self-test: 219 rows, 0 failed
pin_check: .github/workflows/wasm-lockstep.yml satisfies P0-P26
rc=0
pin_check negative control: 20 mutations, 0 failed
```
and both moon tasks pass. `repo:ruff-ci` needs bash 4 or later (`mapfile`). If it fails with `mapfile: command not found` on an empty `stdout.log`, that is a bash-version artifact of this Mac, not a finding. Run `/opt/homebrew/bin/bash ci/ruff/run.sh` and read that result.

- [ ] **Step 11: Commit the README**

Write `$S/msg.txt`:

```text
docs(ci): describe the P26 pin of every wasm-lockstep propose step (SMA-740)

The README now states that P26 pins all ten propose steps and the
propose job keys. It lists the checks that only repeat the pin on
propose, and the checks that still carry a control of their own. The
residual "the steps after apply are not pinned" is replaced by what P26
still does not prove.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

```bash
S=<your scratchpad>
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740
git add ci/wasm-lockstep/README.md
git commit -F "$S/msg.txt"
git status --short
```

Expected: the commit succeeds, and `git status --short` prints nothing.

- [ ] **Step 12: Run the full gate graph like CI**

This is the command of the root `CLAUDE.md` `ci-targets` block, as one line:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-740
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci :next-public-free :helm-render :moon-diagnosis-exec :wasm-lockstep :test-e2e --base origin/main --include-relations
```

Expected: every task passes. On this development Mac, some gates need a specific local bash, and some hang on a small pipe. Before you call a failure a finding, read the root `CLAUDE.md` section "This development Mac only" and the "Diagnosing an unattributed `moon ci` failure" procedure:

- `repo:affected-smoke` needs system bash 3.2. `repo:ruff-ci`, `repo:next-public-free` and `repo:publish-metadata` need bash 4 or later. A `mapfile` or `declare -A` error is a bash-version artifact.
- `repo:actionlint` exits rc 2 with a `small` pipe message when the host pipe holds 512 bytes. That is a host state, not a finding.
- Re-run a gate that needs the other bash directly (`<bash-binary> ci/<gate>/run.sh`) and read that result.

Do NOT change code to make a host artifact green. Report each failure with its gate, its captured output, and your reading of it.

---

## Spec coverage

| Spec section | Plan step |
|---|---|
| 4.2 Constants (five `run:` texts, six entries, `PUSH_IF`, `TOKEN_OUT`, `PROPOSE_ACTIONS["token"]`, `PROPOSE_JOB_KEYS`, comment) | Task 1 Steps 2, 3, 13 |
| 4.3 Check (allow-list loop, message, docstring) | Task 1 Step 14 |
| 4.4 Fixture (pieces from the pins, `PR_LIST` without the redirect, no `name:` or `timeout-minutes:`) | Task 1 Steps 4, 5, 6 |
| 4.5 Existing rows (`COMMIT_RUN_BLOCK`, `CLOSE_RUN_BLOCK`, `ANCHORED_ROWS`, `ONLY` want, the split row, the comment) | Task 1 Steps 7, 8, 9, 10, 11 |
| 4.6 New rows 1-16 | Task 1 Step 8 |
| 4.7 Negative control (six mutations, 20) | Task 2 Steps 1, 2 |
| 4.8 README items 1-10 | Task 3 Steps 1-8 (item 8 and item 10 are one replacement, Step 7) |
| 6.1 Red first | Task 1 Step 12 |
| 6.2 Green | Task 1 Step 15 |
| 6.3 Delete-the-feature | Task 1 Step 17 (rows), Task 2 Step 4 (mutations) |
| 6.4 Real run | Task 1 Steps 3, 15; Task 3 Step 10 |
| 6.5 Negative control count | Task 2 Step 2; Task 3 Step 10 |
| 6.6 moon tasks and full graph | Task 3 Steps 10, 12 |
