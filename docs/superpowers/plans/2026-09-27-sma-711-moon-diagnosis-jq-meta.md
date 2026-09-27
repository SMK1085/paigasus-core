<!-- moon-diagnosis:ok -->
# SMA-711 moon-diagnosis Step 1 query Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the root `CLAUDE.md` moon-diagnosis Step 1 query print the real command and exit code, and stop seven dated files from certifying the old query.

**Architecture:** Two documentation edits. Task 1 rewrites the Step 1 query and its prose inside the gated block, proven by a scratchpad script that extracts the query from the file and runs it against three saved reports. Task 2 changes a marker in seven dated files. Task 3 runs the gate.

**Tech Stack:** Markdown, `jq`, bash, `ci/actionlint/run.sh` (check 12).

**Spec:** `docs/superpowers/specs/2026-09-27-sma-711-moon-diagnosis-jq-meta-design.md`

## Global Constraints

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-711-moon-diagnosis-jq`, branch `feature/sma-711-moon-diagnosis-jq-meta`. Run every command from there.
- Scratchpad: `S=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/0314622e-5958-41db-8d40-bba0c19c9047/scratchpad`.
- The `moon-diagnosis` block keeps exactly one `begin` and one `end` marker, begin first, non-empty body.
- The block keeps the five literals: `operations[]`, `task-execution`, `.moon/cache/states/`, `stderr.log`, `lastRunTime`.
- The new `CLAUDE.md` text must not contain the string `command, exitCode`.
- New prose is ASD-STE100 Simplified Technical English. Do not reword sentences that do not change.
- `git add` every changed file before any gate run (check 12 reads `git ls-files`).
- Conventional commits, scope `docs` or `ci`; end each message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `--no-verify`. Do not amend. Do not `git reset`.
- Do not install host software. Foreground commands only.

## Review Focus

1. A failed action with no `task-execution` operation must still print its `label` (`exec: []`), not vanish — Task 1's script covers it with the synthetic report.
2. A timed-out task must show `status: "timed-out"` and `exitCode: null`, not look like the SMA-711 defect — Task 1's script covers it with the fresh report.
3. The query text in the file, not a retyped copy, is what runs — Task 1 extracts it from `CLAUDE.md`.
4. The old query must fail the same script, or the script proves nothing — Task 1 Step 2 is the red control.
5. A marker change must not leave a second exact `<!-- moon-diagnosis:ok -->` line in any of the seven files — Task 2 Step 3 checks it.

---

### Task 1: Correct the Step 1 query and prose in `CLAUDE.md`

**Files:**
- Modify: `CLAUDE.md:176-186` (inside `<!-- moon-diagnosis:begin -->` … `<!-- moon-diagnosis:end -->`)
- Test (not committed): `$S/verify-sma-711.sh`, with the report copies in `$S/recorded/`, `$S/fresh/` and `$S/synthetic/`

**Interfaces:**
- Consumes: `$S/recorded/.moon/cache/ciReport.json` (sha256 starts `328750d0`), `$S/moon-probe/ciReport.fresh.json` (sha256 starts `38cd4a78`), `$S/synthetic.json`.
- Produces: `$S/verify-sma-711.sh <path-to-CLAUDE.md>`; exit 0 when all three cases pass, exit 1 on any mismatch, exit 2 when the query cannot be extracted. Task 3 re-runs it.

- [ ] **Step 0: Provision the worktree for commits**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
proto install
pnpm -C ts install
```

Expected: both exit 0. `ts/node_modules/.bin/commitlint` exists.

- [ ] **Step 1: Write the verification script**

Set up the three report directories:

```bash
S=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/0314622e-5958-41db-8d40-bba0c19c9047/scratchpad
mkdir -p "$S/fresh/.moon/cache" "$S/synthetic/.moon/cache"
cp "$S/moon-probe/ciReport.fresh.json" "$S/fresh/.moon/cache/ciReport.json"
cp "$S/synthetic.json" "$S/synthetic/.moon/cache/ciReport.json"
shasum -a 256 "$S/recorded/.moon/cache/ciReport.json" "$S/fresh/.moon/cache/ciReport.json"
```

Expected: the hashes start with `328750d0` and `38cd4a78`.

Write `$S/verify-sma-711.sh`:

```bash
#!/bin/bash
# Extracts the Step 1 query from a CLAUDE.md and runs it against three saved reports.
set -uo pipefail
S=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/0314622e-5958-41db-8d40-bba0c19c9047/scratchpad
SRC="${1:?usage: verify-sma-711.sh <CLAUDE.md>}"
q="$(awk '/\*\*Step 1 /{f=1} f&&/^  ```bash$/{g=1;next} g&&/^  ```$/{exit} g{print}' "$SRC")"
[ -n "$q" ] || { echo "EXTRACT FAILED from $SRC"; exit 2; }
echo "--- extracted query:"; printf '%s\n' "$q"
rc=0
check() {
  local dir="$1" expected="$2" got
  got="$(cd "$S/$dir" && bash -c "$q" | jq -c .)"
  if [ "$got" = "$expected" ]; then echo "PASS $dir"; else
    echo "FAIL $dir"; echo "  expected: $expected"; echo "  got:      $got"; rc=1; fi
}
check recorded '{"label":"RunTask(repo:next-public-free)","error":"Task repo:next-public-free failed to run.","exec":[{"status":"failed","command":"set -euo pipefail bash ci/next-public/run.sh --self-test bash ci/next-public/run.sh --negative-control bash ci/next-public/run.sh","exitCode":1}]}
{"label":"RunTask(repo:actionlint)","error":"Task repo:actionlint failed to run.","exec":[{"status":"failed","command":"ci/actionlint/run.sh","exitCode":1}]}'
check fresh '{"label":"RunTask(probe:fail3)","error":"Task probe:fail3 failed to run.","exec":[{"status":"failed","command":"exit 3","exitCode":3}]}
{"label":"RunTask(probe:missing)","error":"Task probe:missing failed to run.","exec":[{"status":"failed","command":"no-such-binary-xyz","exitCode":127}]}
{"label":"RunTask(probe:multi)","error":"Task probe:multi failed to run.","exec":[{"status":"failed","command":"echo a exit 4","exitCode":4}]}
{"label":"RunTask(probe:slow)","error":"Task probe:slow failed to run.","exec":[{"status":"timed-out","command":"sleep 20","exitCode":null}]}'
check synthetic '{"label":"RunTask(x:y)","error":"hash failed","exec":[]}'
exit "$rc"
```

- [ ] **Step 2: Run the script against the current file (red control)**

Run: `bash "$S/verify-sma-711.sh" CLAUDE.md; echo "rc=$?"`
Expected: the extracted query is printed (so extraction works), `FAIL recorded` and `FAIL fresh` show `"command":null,"exitCode":null`, `FAIL synthetic` shows empty output, and `rc=1`.

- [ ] **Step 3: Edit the block**

In `CLAUDE.md`, replace this exact text:

````
  **Step 1 — which task, what command, what exit code.**
  ```bash
  jq '.actions[] | select(.status=="failed")
      | {label, error,
         exec: (.operations[] | select(.meta.type=="task-execution") | {command, exitCode})}' \
     .moon/cache/ciReport.json
  ```
  There is **no action-level `exitCode` key** — `has("exitCode")` is `false`. The widely copied
  query projects `{label, status, exitCode}` and so reports `null` for a key moon never writes,
  which is why this file has a reputation for being empty. It is not: the real exit code and the
  full command are in `operations[]`, on the entry whose `meta.type` is `task-execution`.
````

with:

````
  **Step 1 — which task, what command, what exit code.**
  ```bash
  jq '.actions[] | select(.status=="failed")
      | {label, error,
         exec: [.operations[] | select(.meta.type=="task-execution")
                | {status, command: .meta.command, exitCode: .meta.exitCode}]}' \
     .moon/cache/ciReport.json
  ```
  There is **no action-level `exitCode` key** — `has("exitCode")` is `false`. The widely copied
  query projects `{label, status, exitCode}` and so reports `null` for a key moon never writes,
  which is why this file has a reputation for being empty. It is not: the real exit code and the
  command are in `operations[]`. They are the `command` and `exitCode` fields of the `.meta`
  object of the entry whose `meta.type` is `task-execution`. Until SMA-711 this query read the
  two fields from the entry itself, so it printed `null`. No gate runs this query
  (`ci/actionlint/README.md` L29, SMA-714).
  `exec: []` means that the action failed before a task ran. Read `error` and the other
  operations of that action. A `null` exit code with the `status` `timed-out` means that moon
  stopped the task, so there is no exit code. `command` is a one-line display string, not the
  script: moon joins the lines of a `script:` with spaces. For a `script:` task, read the script
  in its `moon.yml` and reproduce with Step 3.
````

- [ ] **Step 4: Run the script against the edited file (green)**

Run: `bash "$S/verify-sma-711.sh" CLAUDE.md; echo "rc=$?"`
Expected: `PASS recorded`, `PASS fresh`, `PASS synthetic`, `rc=0`.

- [ ] **Step 5: Run the script against `origin/main` (control stays red)**

Run: `git show origin/main:CLAUDE.md > "$S/claude-main.md"` then `bash "$S/verify-sma-711.sh" "$S/claude-main.md"; echo "rc=$?"`
Expected: three `FAIL` rows and `rc=1`.

- [ ] **Step 6: Check the block constraints**

```bash
grep -c 'moon-diagnosis:begin' CLAUDE.md
grep -c 'moon-diagnosis:end' CLAUDE.md
awk '/moon-diagnosis:begin/,/moon-diagnosis:end/' CLAUDE.md > "$S/block.txt"
for lit in 'operations[]' 'task-execution' '.moon/cache/states/' 'stderr.log' 'lastRunTime'; do
  grep -qF -- "$lit" "$S/block.txt" && echo "ok $lit" || echo "MISSING $lit"; done
grep -c 'command, exitCode' CLAUDE.md
```

Expected: `1`, `1`, five `ok` rows, and `0`.

- [ ] **Step 7: Commit**

```bash
git add CLAUDE.md docs/superpowers/specs/2026-09-27-sma-711-moon-diagnosis-jq-meta-design.md docs/superpowers/plans/2026-09-27-sma-711-moon-diagnosis-jq-meta.md
git commit -m "docs: read command and exit code from .meta in the moon-diagnosis query (SMA-711)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 2: Mark the seven dated copies `:superseded`

**Files:**
- Modify: `docs/superpowers/specs/2026-09-03-sma-597-moon-failure-diagnosis-design.md:1`
- Modify: `docs/superpowers/plans/2026-09-03-sma-597-moon-failure-diagnosis.md:1`
- Modify: `docs/superpowers/plans/2026-09-08-sma-502-next-config.md:2652`
- Modify: `docs/superpowers/plans/2026-09-09-sma-508-sdk-package-transport-iam.md:1422`
- Modify: `docs/superpowers/plans/2026-09-10-sma-509-capability-discovery.md:3601`
- Modify: `docs/superpowers/plans/2026-09-20-sma-632-whoami-rpc.md:1924`
- Modify: `docs/superpowers/plans/2026-09-20-sma-633-introspect-role-grants.md:817`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: seven files, each with one exact `<!-- moon-diagnosis:superseded -->` line and no exact `<!-- moon-diagnosis:ok -->` line.

- [ ] **Step 1: Confirm the red state**

Run: `git grep -lx '<!-- moon-diagnosis:ok -->' -- docs/superpowers | grep -E 'sma-(597|502|508|509|632|633)-'`
Expected: exactly the seven paths above.

- [ ] **Step 2: Replace each marker line**

In each of the seven files, replace the one line that is exactly `<!-- moon-diagnosis:ok -->` with these four lines. Use the Edit tool on the line number given above, and check the line content first. Change nothing else.

```
<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-711).** The Step 1 `jq` query in this document prints `null` for the command
> and the exit code, because moon writes both under `.meta`. The corrected procedure is in
> `CLAUDE.md` between the `moon-diagnosis` markers.
```

In the two sma-597 files the marker is line 1, above the `#` heading. Keep a blank line between the new blockquote and the heading, as the file has now.

- [ ] **Step 3: Verify**

```bash
git grep -lx '<!-- moon-diagnosis:ok -->' -- docs/superpowers | grep -E 'sma-(597|502|508|509|632|633)-' ; echo "ok-lines rc=$?"
git grep -cx '<!-- moon-diagnosis:superseded -->' -- docs/superpowers | grep -E 'sma-(597|502|508|509|632|633)-'
git diff --stat
```

Expected: the first command prints nothing and `ok-lines rc=1`. The second prints seven paths, each with count `1`. The diff stat shows seven files, each `4 insertions(+), 1 deletion(-)`.

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/2026-09-03-sma-597-moon-failure-diagnosis-design.md docs/superpowers/plans/2026-09-03-sma-597-moon-failure-diagnosis.md docs/superpowers/plans/2026-09-08-sma-502-next-config.md docs/superpowers/plans/2026-09-09-sma-508-sdk-package-transport-iam.md docs/superpowers/plans/2026-09-10-sma-509-capability-discovery.md docs/superpowers/plans/2026-09-20-sma-632-whoami-rpc.md docs/superpowers/plans/2026-09-20-sma-633-introspect-role-grants.md
git commit -m "docs: mark the dated copies of the old diagnosis query superseded (SMA-711)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 3: Run the gate

**Files:** none changed.

- [ ] **Step 1: Re-run the Task 1 script**

Run: `bash "$S/verify-sma-711.sh" CLAUDE.md; echo "rc=$?"`
Expected: three `PASS` rows, `rc=0`.

- [ ] **Step 2: Check 12 corpus replica**

```bash
git grep -l ciReport | while read -r f; do
  grep -q 'moon-diagnosis:superseded\|moon-diagnosis:ok' "$f" || echo "UNMARKED $f"; done
```

Expected: only `CLAUDE.md`, `ci/actionlint/run.sh` and `ci/actionlint/README.md` may print (they have `CIREPORT_MENTIONS_ALLOWED` rows). No other path.

- [ ] **Step 3: Run `repo:actionlint`**

Run: `/opt/homebrew/bin/bash ci/actionlint/run.sh; echo "rc=$?"` with the proto shims first on `PATH`.
Expected: rc 0 and no `FAIL` row. If the preflight prints that the pipe holds 512 bytes and exits rc 2, do not treat it as a finding. Then run the gate in a Linux container instead (`docker run --rm -v "$PWD":/repo -w /repo ubuntu:24.04 …`, see the memory note on the small-pipe workaround), or report the Step 2 replica as the local evidence and name CI as the final verdict. Report which method ran.
