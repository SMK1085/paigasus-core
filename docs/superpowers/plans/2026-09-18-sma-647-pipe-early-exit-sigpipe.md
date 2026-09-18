<!-- moon-diagnosis:ok -->
<!-- The marker above is for check 12 of repo:actionlint: this plan names the ciReport token when it
quotes run.sh and CLAUDE.md, so this file needs the marker. -->

# SMA-647 pipe early-exit SIGPIPE Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove every pipe into an early-exit reader (`grep -q`/`-m`, `head`, `awk … exit`) from
the tracked shell surfaces, and add check 13 to `repo:actionlint` so that a new one reds the gate.

**Architecture:** Nine rewrite tasks change the 46 sites in place, one file group per task, each
with its pins. Then one task adds check 13's verdict, fixtures and self-test, and the last code
task turns check 13 on over the real corpus and pins its call sites. Check 12 also gets a
second-opinion row for its literal probe.

**Tech Stack:** bash (3.2-compatible in `ci/actionlint/run.sh`), awk, POSIX ERE via `grep -E`,
Python (`ci/affected-graph/ci_targets.py`), GitHub Actions YAML, Moon YAML, Markdown.

**Spec:** `docs/superpowers/specs/2026-09-18-sma-647-pipe-early-exit-sigpipe-design.md`

## Global Constraints

- `ci/actionlint/run.sh` stays bash 3.2 compatible: no `declare -A`, no `mapfile`, no `local -n`, no `${var^^}`.
- Every expansion of a table that can be empty uses `${ARR+"${ARR[@]}"}` (the `BRANCH_SKIP` idiom, `run.sh:1115`).
- No here-string (`<<<`) in any new or rewritten line. Homebrew bash 5.3.15 deadlocks on one over about 512 bytes.
- No `>/dev/null` in place of `-q`. It depends on how one grep build treats `/dev/null`.
- No `sed` script with `q`. The value-reader replacement is `sed -n 1p` (or `sed -n 1,8p`, or `sed -n '1s/…//p'`).
- An SPDX header opens every new source file: `# SPDX-License-Identifier: Apache-2.0` (also on the new `.txt` fixtures, as `ci/publish-metadata/crates-io-categories.txt` does).
- `run.sh` is `set -uo pipefail` with no `-e`: `fail()` sets `FAILED=1`, `infra()` exits 2. Never collapse the two.
- Commits are conventional: `fix(ci): … (SMA-647)`, and the body ends with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Implementers ADD commits. Never `git commit --amend`, never `--no-verify`, never `git stash` without a unique tag.
- Line numbers in this plan and in the spec are at `8bbf2f4c`. `HEAD` `533f7615` changes only docs, so the numbers still hold at the start. They shift after each task: every edit below is keyed on exact old TEXT, not on a line number.
- This plan names the `ciReport.json` token (it quotes `run.sh` and CLAUDE.md), so check 12 needs the `moon-diagnosis:ok` marker at the top of this file. Keep it there.
- Shell setup for every command: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. The worktree is `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-647` (call it `$WT`). Never read or write the main checkout.
- Helper scripts live in the session scratchpad, in a directory called `$S/sma647/` below. Replace `$S` with your own scratchpad path. The worktree sandbox refuses long compound commands, so run each helper as `/bin/bash $S/sma647/<script>.sh`.

---

## Spec deviations and new measurements

**D-1. The inventory has 46 sites, not 44.** `ops/nats/check-subjects.sh:46` and `:56` are missing
from spec §3. Both are `grep -nF … "$tmpl" | head -1 | cut -d: -f1 || true` inside `$( )`. They are
"safe today" (grep output to `head` is block-buffered and small, and `|| true` discards the status),
so D3 applies and this plan rewrites them. Evidence: the inventory output in "Site inventory" below.

**D-2. A YAML block-scalar header is not joined.** Spec §5.2 joins every line that ends in `|`. In
the corpus 117 lines end in a pipe, and ALL 117 are YAML headers (`run: |`, `script: |`, `path: |`).
Zero shell lines end in a bare pipe (evidence: the `eol.sh` output below). Joined, a header would put
the first line of its block after a pipe, so a block that starts with `head -1 file` or
`grep -q x file` would fire with no pipe in it. Check 13 therefore does not join a line that matches
`:[ \t]*[|][ \t]*$` or `^[ \t]*-[ \t]+[|][ \t]*$`. The `silent.txt` fixture holds both shapes, and a
measured mutant without this rule reds that fixture (two rows).

**D-3. A comment line never starts a join.** Spec §5.2 says "line joining first, then skip a
full-line comment". In that order a comment that ends in `|` joins the next CODE line and the joined
line is then skipped as a comment, so a real violation hides behind a comment. Check 13 skips a
comment line before it can start a join. No comment in the corpus ends in `|` today (evidence:
`eol.sh`), so no current result changes. The `fires.txt` fixture has the case (lines 22-23), and a
measured mutant without this rule reds that fixture.

**D-4. `WORKFLOW_CREDENTIALS_SH_CALL_SITES` gets three lines for line 93, not two.** Spec §5.4 says
"both lines are pinned" when I2 splits line 93. The I2 form here has three lines: the capture, the
guard on the capture's exit status, and the match. Deleting the status guard alone brings back the
fail-OPEN that Task 5 closes, so the guard is pinned too. The tuple goes from five to seven entries.

**D-5. The allowlist keys on the LOGICAL line text.** After joining, one row can cover two physical
lines. An `EARLY_EXIT_READER_ALLOWED` row therefore carries the joined text that the verdict sees.
The table ships empty (D3), so this only matters for a future row.

**D-6. The I1 half of the handshake needs a third marker.** In spec §5.3 the producer waits for the
flag and then writes. In the I1 form the main shell does not wait for a process substitution (bash
3.2 has no `$!` for one). If the self-test deletes its temp directory first, the producer loops for
10 s and outlives the test. The producer therefore touches a "second write" marker before its second
write, and the self-test waits (bounded, 10 s, `infra` on timeout) for that marker.

**D-7. `ci_targets.py:2522` is two lines.** The `wired_workflow_credentials` assertion line is split
over two Python literals at `ci_targets.py:2521-2522`. Task 5 replaces both.

**M-1. NEW MEASUREMENT: on this macOS host the defect is near-deterministic, not rare.** Measured on
2026-09-18 on Darwin 25.6.0 (macOS 26.6.2), `/usr/bin/grep` = BSD grep 2.6.0-FreeBSD:

| Probe | `/bin/bash` 3.2.57 | Homebrew bash 5.3.15 |
|---|---|---|
| Old check-12 probe on the REAL CLAUDE.md block (2861 bytes), 5 literals × 100 runs | 500/500 false misses, `PIPESTATUS` 141/0 | 500/500 |
| I1 form, same input | 0/500 | 0/500 |
| A 755-byte, 3-line value whose FIRST line holds the match, 200 runs | 200/200 false misses | 199/200 |
| I1 form, same input | 0/200 | 0/200 |

Consequence 1: `ci/release-plan/run.sh --negative-control` fails on the UNMODIFIED tree under
`/bin/bash`, 8 runs of 8, at row 8's line 341 ("the mutant exited 3 without reporting the non-table
[workspace] row"). The mutant output DOES contain the text. This is a local instance of the defect,
and Task 4 uses it as its failing test. Consequence 2: check 12's production call can never pass on
this host with the old line. Nobody saw it, because the whole gate never finishes locally (CLAUDE.md
LOCAL ONLY). The spec's Linux figures (§2: 0 misses without load) still hold for Linux. This plan
does not investigate why the rate differs by platform. The probe scripts are `pipe-probe.sh`,
`block-probe.sh` and `block-pipestatus.sh`; Task 0 re-creates them.

**M-2. MEASURED: the local `--self-test` of `repo:actionlint`.** Run on 2026-09-18 on the unmodified
tree, bounded at 420 s by a background job and `kill`:

- `/bin/bash` 3.2.57: completes in about 7 s, rc 1. Its only failures are the two known FALSE
  `cargo-lock-step` rows ("'continue-on-error: true is reported'" and "'continue-on-error: false is
  clean'", both with `missing-line bash ci/cargo-lock-integrity/run.sh --self-test`).
- Homebrew bash 5.3.15: DEADLOCKS. 0.20 s of CPU time in 420 s, an empty log, killed.

- A later re-run on the same unmodified tree added a THIRD row, `check 11: ci/release-plan/run.sh
  --negative-control failed`: once `ci/release-plan/.venv` exists, uv prints no preamble, the
  matching line of the mutant output becomes line 1, and line 341 loses its producer (M-1). The
  first run passed that row only because uv's "Creating virtual environment" lines came first.

So the local acceptance test for `repo:actionlint` is: `/bin/bash ci/actionlint/run.sh --self-test`
exits 1, and its `actionlint gate:` rows are EXACTLY the two `cargo-lock-step` rows — plus, in Tasks
1-3 only (before Task 4 fixes line 341), the check-11 row. Any other row is a regression.
`$S/sma647/selftest-actionlint.sh` encodes this (argument `pre-task4` for Tasks 0-3).

**M-3. MEASURED: the other touched gates on the unmodified tree** (2026-09-18, each bounded):
`ci/release-plan/run.sh --self-test` rc 0, `--negative-control` rc 1 (M-1, row 8), `--assert` rc 0;
`ci/workflow-credentials/run.sh --self-test`, `--negative-control` and the bare run rc 0;
`/opt/homebrew/bin/bash ci/ruff/run.sh --self-test` rc 0; `ops/nats/check-subjects.sh` (Homebrew
bash) rc 0; `.lefthook/pre-push/check-branch.test.sh` rc 0; `$S/sma647/images-pins.sh`
`assert_pins: OK`. `/opt/homebrew/bin/bash ci/publish-metadata/run.sh --negative-control` HUNG: no
output after `categories self-test: 39 checks passed`, 0.01 s of CPU in 300 s, killed. The script
has four here-strings, so this is the Homebrew here-string deadlock class, although CLAUDE.md
records a pass for SMA-645. Task 6 therefore checks line 1500 with an extraction harness, and CI
decides the whole gate. A concurrent `moon ci` from another session was running during all of these
measurements (load average about 15).

**M-4. MEASURED: check 1 (actionlint + shellcheck) does not finish locally.** `actionlint 1.7.12
-pyflakes= -shellcheck=<py/.venv/bin/shellcheck 0.11.0>` over all seven workflows ran 18 minutes at
about 840% CPU (151 CPU-minutes) with no shellcheck child, and was killed. Over `ci.yml` alone it
was still running after 110 s at about 310% CPU, and was killed. With `-shellcheck=` (shellcheck
off) it finishes in about 1 s at rc 0. So the local workflow check in this plan runs actionlint
with shellcheck OFF, and the shellcheck half is verified in CI only.

---

## File structure

| File | Change | Task |
|---|---|---|
| `ci/actionlint/run.sh` | check 12 site + second opinion; 10 other sites; 5 check-8d copies; check 13 definitions, self-test, production block; counts | 1, 2, 3, 10, 11 |
| `ci/actionlint/fixtures/early-exit/fires.txt` | NEW — every reader form, each must fire | 10 |
| `ci/actionlint/fixtures/early-exit/silent.txt` | NEW — every non-reader form, none may fire | 10 |
| `ci/actionlint/fixtures/early-exit/allow.txt` | NEW — one hit, for the allowlist cases | 10 |
| `ci/actionlint/README.md` | counts, check-13 row, Limitations L33-L40 | 10, 11 |
| `ci/affected-graph/ci_targets.py` | pins: `MOON_CI_BRANCH_BLOCK`, `WORKFLOW_CREDENTIALS_SH_CALL_SITES`, `RELEASE_PLAN_SH_CALL_SITES`, `ACTIONLINT_SH_CALL_SITES`, `wired_workflow_credentials`, `wired_actionlint`, battery cases | 3, 4, 5, 11 |
| `.github/workflows/ci.yml` | line 271 | 3 |
| `ci/release-plan/run.sh` | lines 83, 187, 202, 341 | 4 |
| `ci/workflow-credentials/run.sh` | lines 93, 97 (I2) and a comment | 5 |
| `ci/ruff/run.sh` | line 148 (I2) | 6 |
| `ci/publish-metadata/run.sh` | lines 536, 1500 (I2) | 6 |
| `ci/release-parity/ecosystems/semantic-release.sh` | lines 34-35 | 7 |
| `.lefthook/pre-push/check-branch.sh` | line 22 | 7 |
| `moon.yml` | line 373; actionlint-task comments | 7, 10 |
| `ops/nats/check-subjects.sh` | lines 46, 56, 101, 108 | 7 |
| `ci/images/run.sh` | lines 49, 50, 60, 77, 78, 117 | 8 |
| `.github/workflows/wheels.yml` | lines 312, 323, 521, 544, 677 | 9 |
| `.github/workflows/prebuild.yml` | line 187; comment at 266 | 9, 11 |
| `.github/workflows/release.yml` | comment at 912-919 | 11 |
| `CLAUDE.md` | WC entry, "currently 14", LOCAL ONLY sentence, new Gotchas entry | 5, 10, 11 |

Task order: 0 (helpers, baseline) → 1 (check 12's own site) → 2-9 (the other sites, with their
pins) → 10 (check 13 self-test only) → 11 (check 13 production, pins, docs) → 12 (verification).
Check 13 reads the real corpus only from Task 11. After Task 9 the inventory is 0 rows, so Task 11
turns check 13 on over a clean tree.

---

## Site inventory (re-derived, with the recorded commands)

### Command 1 — the rule itself, stand-alone (`$S/sma647/scan.sh`, created in Task 0)

It lists the corpus with the spec's exact pathspecs, joins lines as check 13 does (with D-2 and
D-3), and applies check 13's ERE. Full output at `533f7615`:

```
corpus: 76 files
.github/workflows/ci.yml:271
.github/workflows/prebuild.yml:187
.github/workflows/wheels.yml:312
.github/workflows/wheels.yml:323
.github/workflows/wheels.yml:521
.github/workflows/wheels.yml:544
.github/workflows/wheels.yml:677
.lefthook/pre-push/check-branch.sh:22
ci/actionlint/run.sh:1066
ci/actionlint/run.sh:1142
ci/actionlint/run.sh:1173
ci/actionlint/run.sh:2482
ci/actionlint/run.sh:2492
ci/actionlint/run.sh:2506
ci/actionlint/run.sh:2519
ci/actionlint/run.sh:2565
ci/actionlint/run.sh:2578
ci/actionlint/run.sh:4273
ci/actionlint/run.sh:4304
ci/actionlint/run.sh:4340
ci/actionlint/run.sh:4375
ci/actionlint/run.sh:4408
ci/actionlint/run.sh:4777
ci/actionlint/run.sh:5618
ci/images/run.sh:117
ci/images/run.sh:49
ci/images/run.sh:50
ci/images/run.sh:60
ci/images/run.sh:77
ci/images/run.sh:78
ci/publish-metadata/run.sh:1500
ci/publish-metadata/run.sh:536
ci/release-parity/ecosystems/semantic-release.sh:34
ci/release-parity/ecosystems/semantic-release.sh:35
ci/release-plan/run.sh:187
ci/release-plan/run.sh:202
ci/release-plan/run.sh:341
ci/release-plan/run.sh:83
ci/ruff/run.sh:148
ci/workflow-credentials/run.sh:93
ci/workflow-credentials/run.sh:97
moon.yml:373
ops/nats/check-subjects.sh:101
ops/nats/check-subjects.sh:108
ops/nats/check-subjects.sh:46
ops/nats/check-subjects.sh:56
rows: 46 (all files)
```

(`run.sh:2482`, `:2492`, `:2519` and `:2565` are the second physical line of a `\`-continued
command; the pipe that fires is on that line.)

### Command 2 — every line that ends in a pipe, with the next line (`$S/sma647/eol.sh`)

```bash
#!/bin/bash
# Lines that end in a pipe (optionally followed by a backslash), and the line after each.
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-647 || exit 2
git ls-files -- ':(glob)**/*.sh' ':(glob).github/workflows/*.yml' ':(glob).github/workflows/*.yaml' 'moon.yml' ':(glob)**/moon.yml' ':(glob).moon/**/*.yml' 'lefthook.yml' > /tmp/eol.$$ || exit 2
while IFS= read -r f; do
  awk -v F="$f" '
    prev != "" { print F ":" NR-1 ": " prev; print F ":" NR  ": >> " $0; prev = "" }
    /[|][ \t]*\\?[ \t]*$/ && !/[|][|][ \t]*\\?[ \t]*$/ { prev = $0 }
  ' "$f"
done < /tmp/eol.$$
echo "== comment lines that end in a pipe =="
while IFS= read -r f; do
  grep -nE '^[[:space:]]*#.*[|][[:space:]]*$' "$f" | sed "s|^|$f:|"
done < /tmp/eol.$$
rm -f /tmp/eol.$$
```

Result at `533f7615`: 117 line pairs. Every first line is a YAML block-scalar header of the form
`<key>: |` (for example `.github/workflows/ci.yml:266: run: |`, `moon.yml:367: script: |`,
`.moon/templates/rust/template.yml:2: description: |`). The count of first lines that do NOT match
`:[[:space:]]*[|][[:space:]]*$` is 0. The "comment lines that end in a pipe" section is empty. No
reader sits on a line after a bare shell pipe, so joining adds no site.

### Command 3 — an independent, cruder cross-check (`$S/sma647/crosscheck.sh`)

`xargs grep -nE '[|].*(grep[^|]*(-[[:alpha:]]*[qml]|--quiet|--silent|--max-count|--files-with)|head|awk[^|]*exit|sed[^|]*[0-9]q|egrep|fgrep)'`
over the same corpus, full-line comments removed, minus the rows of Command 1. 62 crude hits; the 16
that Command 1 does not report are all non-readers:

```
.github/workflows/release.yml:372   | jq -r '.prs[0].head_branch'   ("head" inside a jq path)
ci/actionlint/run.sh:3634, 3804, 3809   | grep -vxF          (-v, reads to EOF)
ci/actionlint/run.sh:4738           grep -q 'ciReport' "$path"  (a file argument, no pipe into it)
ci/actionlint/run.sh:4801, 4807, 4813, 4819, 4823, 4837, 4842, 4848   | grep -v '^stale-allowlist '
ci/actionlint/run.sh:5933           | xargs -0 grep -l          (xargs reads to EOF; -l is a spec §4 non-goal)
ci/actionlint/run.sh:5939           the same text inside an infra message
ci/affected-graph/run.sh:205        | grep -v -- '--include-relations'
```

### Reconciliation with spec §3

- Spec §3 names 44 sites; Command 1 finds 46. The two extra sites are `ops/nats/check-subjects.sh:46`
  and `:56` (D-1).
- `ci/images/run.sh:49-117` is six sites: 49, 50, 60, 77, 78, 117.
- `ci.yml:271` has five copies in `run.sh` (4273, 4304, 4340, 4375, 4408) and one pin in
  `ci_targets.py:105`: seven copies in all, as spec D3 says.
- No `tracked` path in the repository contains a `|` (measured: `git ls-files | grep -c '[|]'` = 0).

### Pin search (spec §5.4) — every rewritten line's old text, searched in every tracked non-doc file

Command (`$S/sma647/pins.sh`): `git grep -nF -e "<distinctive text>" -- ':!docs'` for each site.
Result — the ONLY copies outside the site itself:

| Site | Pin | Task |
|---|---|---|
| `ci.yml:271` | `ci_targets.py:105` (`MOON_CI_BRANCH_BLOCK`), `run.sh:4273, 4304, 4340, 4375, 4408` (check 8d fixtures) | 3 |
| `release-plan/run.sh:83` | `ci_targets.py:1140` (`RELEASE_PLAN_SH_CALL_SITES`); `wired_release_plan` is DERIVED from the tuple | 4 |
| `release-plan/run.sh:341` | `ci_targets.py:1146` (same tuple, escaped `\\[workspace\\]`, so `git grep -F` on the raw text misses it) | 4 |
| `workflow-credentials/run.sh:93` | `ci_targets.py:1088` (`WORKFLOW_CREDENTIALS_SH_CALL_SITES`), `ci_targets.py:2521-2522` (`wired_workflow_credentials`) | 5 |
| every other site | none | — |

No `T_*` table in `run.sh` holds any rewritten line (`T_INVOCATION_ALLOWLIST` at `run.sh:1695`
holds only the `moon ci` line under the `elif`, which does not change). `RUFF_SH_CALL_SITES` does not
hold `ruff/run.sh:148`. `SELF_SCHEDULED_GATES` holds no rewritten `moon.yml` line.

---

## Task 0: Helpers and baseline (no commit)

**Files:** none in the repository. Creates `$S/sma647/*.sh`.

**Interfaces:**
- Produces: `$S/sma647/scan.sh [prefix...]` → prints `corpus: N files`, the matching `<path>:<line>`
  rows (filtered to the prefixes when given), and `rows: N (all files)`.
- Produces: `$S/sma647/bounded.sh <seconds> <logfile> <cmd...>` → prints `rc=<n> after ~<s>s: <cmd>`
  or `TIMEOUT after <s>s: <cmd>` (exit 124).
- Produces: `$S/sma647/selftest-actionlint.sh [pre-task4]` → runs
  `/bin/bash ci/actionlint/run.sh --self-test` bounded, and prints `ACTIONLINT-SELFTEST OK` only
  when the failure rows are exactly the M-2 baseline (with `pre-task4`, the check-11 row is also
  tolerated).

- [ ] **Step 1: Create `$S/sma647/scan.sh`**

```bash
#!/bin/bash
# SMA-647 inventory: check 13's rule, stand-alone, over check 13's corpus. Run from anywhere.
# Optional arguments: path prefixes to keep (for example ci/release-plan/ .github/).
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-647 || exit 2
ERE='(^|[^|])[|][[:space:]]*([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+)*(command[[:space:]]+)?(grep[[:space:]]([^|]*[[:space:]])?(-[[:alpha:]]*[qm]|--(quiet|silent|max-count))|head([[:space:];)]|$)|awk[[:space:]]([^|]*[^[:alnum:]_])?exit([^[:alnum:]_]|$))'
list="$(mktemp)"; joined="$(mktemp)"
git ls-files -- ':(glob)**/*.sh' ':(glob).github/workflows/*.yml' ':(glob).github/workflows/*.yaml' 'moon.yml' ':(glob)**/moon.yml' ':(glob).moon/**/*.yml' 'lefthook.yml' > "$list" || exit 2
echo "corpus: $(wc -l < "$list" | tr -d ' ') files"
while IFS= read -r f; do
  awk -v F="$f" '
    function flush() { if (acc != "") print F ":" start "\t" acc; acc = "" }
    {
      line = $0
      if (acc == "") {
        if (line ~ /^[ \t]*#/) next
        start = NR
        acc = line
      } else {
        acc = acc " " line
      }
      if (acc ~ /:[ \t]*[|][ \t]*$/ || acc ~ /^[ \t]*-[ \t]+[|][ \t]*$/) { flush(); next }
      if (acc ~ /[|][ \t]*\\?[ \t]*$/ && acc !~ /[|][|][ \t]*\\?[ \t]*$/) {
        sub(/\\[ \t]*$/, "", acc)
        next
      }
      flush()
    }
    END { flush() }
  ' "$f"
done < "$list" > "$joined"
grep -E -- "$ERE" "$joined" | cut -f1 | LC_ALL=C sort > "$list.hits"
if [ "$#" -gt 0 ]; then
  for p in "$@"; do grep -F -- "$p" "$list.hits"; done
else
  cat "$list.hits"
fi
echo "rows: $(wc -l < "$list.hits" | tr -d ' ') (all files)"
rm -f "$list" "$joined" "$list.hits"
```

- [ ] **Step 2: Create `$S/sma647/bounded.sh`**

```bash
#!/bin/bash
# Usage: bounded.sh <seconds> <logfile> <command> [args...]
# Runs the command in the background with its output in <logfile>, kills it after <seconds>
# (macOS has no `timeout`), and prints its rc or TIMEOUT. Exit: the command's rc, or 124.
secs="$1"; log="$2"; shift 2
"$@" > "$log" 2>&1 &
pid=$!
n=0
while kill -0 "$pid" 2>/dev/null && [ "$n" -lt "$secs" ]; do sleep 1; n=$((n + 1)); done
if kill -0 "$pid" 2>/dev/null; then
  ps -o pid,pcpu,time -p "$pid" | sed -n 2p | sed 's/^/  still running: pid,cpu%,cputime = /'
  pkill -P "$pid" 2>/dev/null
  kill "$pid" 2>/dev/null
  echo "TIMEOUT after ${secs}s: $*"
  exit 124
fi
wait "$pid"
rc=$?
echo "rc=$rc after ~${n}s: $*"
exit "$rc"
```

- [ ] **Step 3: Create `$S/sma647/selftest-actionlint.sh`**

```bash
#!/bin/bash
# repo:actionlint --self-test under /bin/bash 3.2 (the one local bash that finishes it, M-2).
# Green = rc 1 AND the only `actionlint gate:` rows are the two known false cargo-lock-step rows.
# With the argument `pre-task4`, check 11's negative-control row is tolerated too: it is the M-1
# local instance of the defect, and Task 4 removes it.
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
S="$(cd "$(dirname "$0")" && pwd)"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-647 || exit 2
/bin/bash "$S/bounded.sh" 300 "$S/selftest-actionlint.log" /bin/bash ci/actionlint/run.sh --self-test
rc=$?
grep '^actionlint gate:' "$S/selftest-actionlint.log" > "$S/selftest-actionlint.rows"
other="$(grep -v -e "cargo-lock-step self-test 'continue-on-error: true is reported'" \
  -e "cargo-lock-step self-test 'continue-on-error: false is clean'" "$S/selftest-actionlint.rows")"
if [ "${1:-}" = "pre-task4" ]; then
  other="$(printf '%s\n' "$other" | grep -vxF 'actionlint gate: check 11: ci/release-plan/run.sh')"
fi
known="$(grep -c 'cargo-lock-step self-test' "$S/selftest-actionlint.rows")"
if [ "$rc" -eq 1 ] && [ "$known" -eq 2 ] && [ -z "$other" ]; then
  echo "ACTIONLINT-SELFTEST OK (rc 1, only the known rows${1:+, mode $1})"
  exit 0
fi
echo "ACTIONLINT-SELFTEST REGRESSION: rc=$rc known=$known; other rows follow"
printf '%s\n' "$other"
exit 1
```

(Tested during planning on the unmodified tree: `pre-task4` mode → `ACTIONLINT-SELFTEST OK`; plain
mode → `REGRESSION` naming only the check-11 row.)

- [ ] **Step 4: Create the probe scripts of M-1 (`pipe-probe.sh`, `block-probe.sh`)**

`$S/sma647/block-probe.sh`:

```bash
#!/usr/bin/env bash
# Probe: the OLD check-12 literal probe (pipe form) against the I1 form, on the real CLAUDE.md block.
set -uo pipefail
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-647 || exit 2
lb="$(grep -n 'moon-diagnosis:begin' CLAUDE.md | cut -d: -f1)"
le="$(grep -n 'moon-diagnosis:end' CLAUDE.md | cut -d: -f1)"
block="$(sed -n "$((lb + 1)),$((le - 1))p" CLAUDE.md)"
pipe_miss=0; i1_miss=0; runs=0
for i in $(seq 1 100); do
  for lit in 'operations[]' 'task-execution' '.moon/cache/states/' 'stderr.log' 'lastRunTime'; do
    runs=$((runs + 1))
    printf '%s' "$block" | grep -qF -- "$lit" || pipe_miss=$((pipe_miss + 1))
    grep -qF -- "$lit" < <(printf '%s' "$block") || i1_miss=$((i1_miss + 1))
  done
done
echo "$BASH_VERSION: block ${#block} bytes; pipe form false miss $pipe_miss/$runs, I1 form false miss $i1_miss/$runs"
```

`$S/sma647/pipe-probe.sh`:

```bash
#!/usr/bin/env bash
# Probe: pipe form against the I1 form on a 3-line value whose FIRST line holds the match.
set -o pipefail
line1="FAIL 'a non-table [workspace] is inconclusive': releasable_packages raised InconclusiveError for the wrong reason: $(printf 'x%.0s' $(seq 1 200))"
multi="$line1
FAIL 'an array-of-tables [workspace] is inconclusive': $(printf 'y%.0s' $(seq 1 300))
FAIL 'the five shape markers are mutually exclusive': config_sections did not raise"
pipe_miss=0
i1_miss=0
for i in $(seq 1 200); do
  printf '%s\n' "$multi" | grep -q "a non-table \[workspace\] is inconclusive" || pipe_miss=$((pipe_miss + 1))
  grep -q "a non-table \[workspace\] is inconclusive" < <(printf '%s\n' "$multi") || i1_miss=$((i1_miss + 1))
done
echo "$BASH_VERSION: pipe form false miss $pipe_miss/200, I1 form false miss $i1_miss/200 (bytes=${#multi})"
```

- [ ] **Step 5: Record the baseline**

Run, one command at a time:

```
/bin/bash $S/sma647/scan.sh
/bin/bash $S/sma647/selftest-actionlint.sh pre-task4
/bin/bash $S/sma647/block-probe.sh
/bin/bash $S/sma647/pipe-probe.sh
```

Expected: `rows: 46 (all files)` with the list above; `ACTIONLINT-SELFTEST OK`; the probe numbers
of M-1 (the exact pipe-form counts can vary a little; the I1 counts must be 0). If
`selftest-actionlint.sh` reports any other row on the unmodified tree, STOP and report it: the
baseline in M-2 does not hold on your host.

---

## Task 1: Check 12's literal probe (`run.sh:4777`) and its second opinion

**Files:**
- Modify: `ci/actionlint/run.sh:4776-4778` (`claude_md_block_verdict`'s literal loop)
- Modify: `ci/actionlint/run.sh:4909-4922` (end of `doc_diagnosis_self_test`: one new case)
- Modify: `ci/actionlint/run.sh:5998-6003` (check 12's production `case`: one new arm)

**Interfaces:**
- Consumes: `DOC_DIAGNOSIS_REQUIRED_LITERALS` (unchanged), `expect_doc`, `$good`, `$tmpd`, `lit`
  (already `local` in `doc_diagnosis_self_test`).
- Produces: `claude_md_block_verdict` emits one NEW row kind, `literal-disagreement <literal>`, when
  `grep -qF` misses a literal that `case "$block" in *"$lit"*` finds. Rows `missing-literal <lit>`
  keep their meaning (both matchers miss). Check 12's production loop maps the new row to `fail`.

**Idiom:** I1. The producer is `printf` of a variable: its only possible failure is a write error,
which is exactly the false 141. So `pipefail` never caught a real producer failure here.

**Decision on the PATH-stubbed grep fixture (spec §5.6): YES.** It is the only way to execute the
disagreement arm: after the I1 rewrite the real grep cannot race, so no fixture FILE can produce the
row. Without it, deleting the `literal-disagreement` echo or its production arm survives every
test. The cost is five lines, a scratch directory the self-test already owns, and a `PATH` change
scoped to one command substitution. The stub passes every call through to the real grep except one
whose first argument is `-qF`, because the marker counts in the same function use `grep -o`/`grep -n`.

- [ ] **Step 1: Write the failing test** — insert into `doc_diagnosis_self_test`, between the
deletion loop's `done` and `rm -rf "$tmpd"`. Old text:

```bash
    got="$(claude_md_block_verdict "$tmpd/miss.md")"
    expect_doc "required literal '$lit' deleted fires" "missing-literal $lit"
  done

  rm -rf "$tmpd"
  return "$rc"
}
```

New text:

```bash
    got="$(claude_md_block_verdict "$tmpd/miss.md")"
    expect_doc "required literal '$lit' deleted fires" "missing-literal $lit"
  done

  # SMA-647 §5.6 — the second opinion. A fake `grep` that reports every -qF probe as a miss, put
  # first on PATH inside ONE command substitution, makes the two matchers disagree on purpose. It
  # is the only way to execute the disagreement arm: after the process-substitution rewrite the
  # real grep cannot race, so no fixture file can produce the row. Every other grep call passes
  # through to the real binary, because the marker counts above the literal loop use grep too.
  local real_grep
  real_grep="$(command -v grep)"
  mkdir -p "$tmpd/stub-bin"
  printf '#!/bin/sh\ncase "$1" in -qF) exit 1 ;; esac\nexec "%s" "$@"\n' "$real_grep" > "$tmpd/stub-bin/grep"
  chmod +x "$tmpd/stub-bin/grep"
  printf '%s\n' "$good" > "$tmpd/claude.md"
  got="$(PATH="$tmpd/stub-bin:$PATH"; claude_md_block_verdict "$tmpd/claude.md")"
  expect_doc 'a grep miss that a bash substring match contradicts is a literal-disagreement row' \
    "$(for lit in "${DOC_DIAGNOSIS_REQUIRED_LITERALS[@]}"; do printf 'literal-disagreement %s\n' "$lit"; done)"

  rm -rf "$tmpd"
  return "$rc"
}
```

- [ ] **Step 2: Run it and see it fail.** Create `$S/sma647/check12-block.sh` (it runs the REAL
function from the file on the real CLAUDE.md):

```bash
#!/bin/bash
# Runs the REAL claude_md_block_verdict (extracted from run.sh) over the real CLAUDE.md, 100 times.
set -uo pipefail
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-647 || exit 2
eval "$(awk '/^DOC_DIAGNOSIS_REQUIRED_LITERALS=\($/,/^\)$/' ci/actionlint/run.sh)"
eval "$(awk '/^claude_md_block_verdict\(\) \{$/,/^\}$/' ci/actionlint/run.sh)"
bad=0
for i in $(seq 1 100); do
  out="$(claude_md_block_verdict CLAUDE.md)"
  [ -z "$out" ] || { bad=$((bad + 1)); last="$out"; }
done
echo "non-empty verdicts on the real CLAUDE.md: $bad/100"
[ "$bad" -eq 0 ] || { printf 'last verdict:\n%s\n' "$last"; exit 1; }
```

Then:

```
/bin/bash $S/sma647/check12-block.sh
/bin/bash $S/sma647/selftest-actionlint.sh pre-task4
```

Expected (measured on this host before any change): `non-empty verdicts on the real CLAUDE.md:
100/100`, with the five `missing-literal` rows as the last verdict — the production check 12 can
never pass here with the old line (M-1). The self-test reports `ACTIONLINT-SELFTEST REGRESSION` with
one extra row: `doc-diagnosis self-test 'a grep miss that a bash substring match contradicts is a
literal-disagreement row': got 'missing-literal operations[]` … `expected 'literal-disagreement
operations[]` …. (On a Linux host the harness can print 0/100 before the change; the self-test
row still fails.)

- [ ] **Step 3: Implement.** Old text (`run.sh:4776-4778`):

```bash
  for lit in "${DOC_DIAGNOSIS_REQUIRED_LITERALS[@]}"; do
    printf '%s' "$block" | grep -qF -- "$lit" || echo "missing-literal $lit"
  done
}
```

New text:

```bash
  # Process substitution, NOT a pipe (SMA-647). `grep -q` exits at its first match, so a later
  # write from a piped printf got SIGPIPE, and under `pipefail` that 141 read as a miss. That was
  # the false `missing-literal` row behind three CI reds, and on macOS with BSD grep it fired on
  # every run (MEASURED, 500 of 500).
  #
  # SECOND OPINION (SMA-647 §5.6). A grep miss is checked again with a pure-bash substring match.
  # If bash finds the literal, the two matchers disagree and the row says so instead of reporting
  # a miss. grep cannot race here any more, so such a row is evidence of a second mechanism — the
  # open question SMA-647 keeps its observation window for.
  for lit in "${DOC_DIAGNOSIS_REQUIRED_LITERALS[@]}"; do
    grep -qF -- "$lit" < <(printf '%s' "$block") && continue
    case "$block" in
      *"$lit"*) echo "literal-disagreement $lit" ;;
      *) echo "missing-literal $lit" ;;
    esac
  done
}
```

Then add the production arm. Old text (check 12's second loop):

```bash
    missing-literal\ *)
      fail "check 12: CLAUDE.md's moon-diagnosis block no longer contains
      '${verdict#missing-literal }'. Every entry in DOC_DIAGNOSIS_REQUIRED_LITERALS is a
      load-bearing element of the measured procedure." ;;
    *)
      infra "check 12: unrecognised block verdict '$verdict'" ;;
```

New text:

```bash
    missing-literal\ *)
      fail "check 12: CLAUDE.md's moon-diagnosis block no longer contains
      '${verdict#missing-literal }'. Every entry in DOC_DIAGNOSIS_REQUIRED_LITERALS is a
      load-bearing element of the measured procedure." ;;
    literal-disagreement\ *)
      fail "check 12: grep reported '${verdict#literal-disagreement }' missing from CLAUDE.md's
      moon-diagnosis block, but a bash substring match found it there. The two must agree
      (SMA-647). This is the evidence the SMA-647 observation window waits for: keep this run's
      whole output and attach it to SMA-647 before you re-run anything." ;;
    *)
      infra "check 12: unrecognised block verdict '$verdict'" ;;
```

- [ ] **Step 4: Run it and see it pass**

```
/bin/bash $S/sma647/check12-block.sh
/bin/bash $S/sma647/selftest-actionlint.sh pre-task4
/bin/bash $S/sma647/scan.sh ci/actionlint/run.sh:4777
```

Expected: `non-empty verdicts on the real CLAUDE.md: 0/100`; `ACTIONLINT-SELFTEST OK`; the scan
prints no `run.sh:4777` row and `rows: 45 (all files)`.

- [ ] **Step 5: Commit**

```
git add ci/actionlint/run.sh
git commit -m "fix(ci): stop check 12's literal probe losing its producer to SIGPIPE (SMA-647)" -m "grep -q exits at the first match, so the piped printf got SIGPIPE and pipefail read the match as a miss. Use process substitution, and add a second opinion row, literal-disagreement, for a grep miss that a bash substring match contradicts." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 2: The other `ci/actionlint/run.sh` sites

**Files:** `ci/actionlint/run.sh` lines 1066, 1142, 1173, 2481-2482, 2491-2492, 2506, 2518-2519,
2564-2565, 2578, 5618.

**Interfaces:** no function signature changes. `pattern_verdict`, `origin_has`, `origin_candidates`,
`cargo_lock_step_verdict` and `selftest_expect_tag` return the same values for the same inputs.

No pins: the pin search found no copy of any of these lines.

Per-site table (all I1 sites have a `printf` of a variable as the producer, so `pipefail` never
caught a real producer failure there; every value-reader site is inside `$( )` in a file without
`set -e`, so its status was already discarded):

| Line | Idiom | Old | New |
|---|---|---|---|
| 1066 | I1 | `  if ! printf '%s' "$p" \| grep -qE '^[A-Za-z0-9._/*-]+$'; then` | `  if ! grep -qE '^[A-Za-z0-9._/*-]+$' < <(printf '%s' "$p"); then` |
| 1142 | I1 | `  printf '%s\n' "$ORIGIN_REFS" \| grep -qxF -- "$1"` | `  grep -qxF -- "$1" < <(printf '%s\n' "$ORIGIN_REFS")` |
| 1173 | value | `  ' \| sort -t $'\t' -k1,1 -k2,2 \| cut -f2- \| head -8 \| tr '\n' ' ' \| sed 's/ *$//'` | `  ' \| sort -t $'\t' -k1,1 -k2,2 \| cut -f2- \| sed -n 1,8p \| tr '\n' ' ' \| sed 's/ *$//'` |
| 2482 | value | `    \| grep -nxF -e "${T_CARGO_LOCK_STEP_REQUIRED[0]}" \| head -1 \| cut -d: -f1)"` | `    \| grep -nxF -e "${T_CARGO_LOCK_STEP_REQUIRED[0]}" \| sed -n 1p \| cut -d: -f1)"` |
| 2492 | value | `    \| grep -nE '^- ' \| head -1 \| cut -d: -f1)"` | `    \| grep -nE '^- ' \| sed -n 1p \| cut -d: -f1)"` |
| 2506 | value | `    idx="$(printf '%s\n' "$window" \| grep -nxF -e "$line" \| head -1 \| cut -d: -f1)"` | `    idx="$(printf '%s\n' "$window" \| grep -nxF -e "$line" \| sed -n 1p \| cut -d: -f1)"` |
| 2519 | value | `    -e '- name: moon ci (affected graph)' \| head -1 \| cut -d: -f1)"` | `    -e '- name: moon ci (affected graph)' \| sed -n 1p \| cut -d: -f1)"` |
| 2565 | value | `    \| grep -m1 '^continue-on-error:' \| sed 's/^continue-on-error:[[:space:]]*//')"` | `    \| grep '^continue-on-error:' \| sed -n '1s/^continue-on-error:[[:space:]]*//p')"` |
| 2578 | value | `  cond="$(printf '%s\n' "$keys" \| grep -m1 '^if:' \| sed 's/^if:[[:space:]]*//')"` | `  cond="$(printf '%s\n' "$keys" \| grep '^if:' \| sed -n '1s/^if:[[:space:]]*//p')"` |
| 5618 | I1 | `  if ! printf '%s' "$out" \| grep -qF "[$tag]"; then` | `  if ! grep -qF "[$tag]" < <(printf '%s' "$out"); then` |

(`\|` in the table is Markdown escaping; the file has a bare `|`.)

First-match semantics: `sed -n 1p` prints the first line of grep's output, which is grep's first
match — the same line `head -1` printed. For 2565 and 2578, `grep X | sed -n '1s/…//p'` prints only
line 1, and line 1 always starts with the prefix grep selected, so the substitution always succeeds
and the line always prints: the same output as `grep -m1 X | sed 's/…//'`. With no match, both forms
print nothing.

- [ ] **Step 1: Run the failing check**

```
/bin/bash $S/sma647/scan.sh ci/actionlint/run.sh:1066 ci/actionlint/run.sh:1142 ci/actionlint/run.sh:1173 ci/actionlint/run.sh:24 ci/actionlint/run.sh:25 ci/actionlint/run.sh:5618
```

Expected: exactly the ten rows 1066, 1142, 1173, 2482, 2492, 2506, 2519, 2565, 2578, 5618, then
`rows: 45 (all files)`. (A prefix keeps the rows whose text contains it: `run.sh:24` keeps
2482-2519, `run.sh:25` keeps 2565 and 2578, and no `run.sh:4…` row contains either.)

- [ ] **Step 2: Implement** — apply the ten Edits of the table, each with the exact old line as
`old_string` and the new line as `new_string`. For the multi-line sites, the full old/new pairs are:

Old (2481-2482):
```bash
  n_step="$(printf '%s\n' "$stripped" \
    | grep -nxF -e "${T_CARGO_LOCK_STEP_REQUIRED[0]}" | head -1 | cut -d: -f1)"
```
New:
```bash
  n_step="$(printf '%s\n' "$stripped" \
    | grep -nxF -e "${T_CARGO_LOCK_STEP_REQUIRED[0]}" | sed -n 1p | cut -d: -f1)"
```

Old (2491-2492):
```bash
  n_end="$(printf '%s\n' "$stripped" | tail -n +"$((n_step + 1))" \
    | grep -nE '^- ' | head -1 | cut -d: -f1)"
```
New:
```bash
  n_end="$(printf '%s\n' "$stripped" | tail -n +"$((n_step + 1))" \
    | grep -nE '^- ' | sed -n 1p | cut -d: -f1)"
```

Old (2518-2519):
```bash
  n_moon="$(printf '%s\n' "$stripped" | grep -nxF \
    -e '- name: moon ci (affected graph)' | head -1 | cut -d: -f1)"
```
New:
```bash
  n_moon="$(printf '%s\n' "$stripped" | grep -nxF \
    -e '- name: moon ci (affected graph)' | sed -n 1p | cut -d: -f1)"
```

Old (2564-2565):
```bash
  coe="$(printf '%s\n' "$keys" \
    | grep -m1 '^continue-on-error:' | sed 's/^continue-on-error:[[:space:]]*//')"
```
New:
```bash
  coe="$(printf '%s\n' "$keys" \
    | grep '^continue-on-error:' | sed -n '1s/^continue-on-error:[[:space:]]*//p')"
```

Also update the comment directly above `origin_candidates` (it names `head -8` twice as the old
reader). Old text:
```bash
# Two failure modes of a naive `head -8` on ORIGIN_REFS, fixed here:
```
New text:
```bash
# Two failure modes of a naive first-8-lines cut on ORIGIN_REFS, fixed here (the cut itself is
# `sed -n 1,8p`, not `head -8`: head exits early and would lose the producer to SIGPIPE, SMA-647):
```

- [ ] **Step 3: Run it and see it pass**

```
/bin/bash $S/sma647/scan.sh ci/actionlint/run.sh:1066 ci/actionlint/run.sh:1142 ci/actionlint/run.sh:1173 ci/actionlint/run.sh:24 ci/actionlint/run.sh:25 ci/actionlint/run.sh:5618
/bin/bash $S/sma647/selftest-actionlint.sh pre-task4
```

Expected: no row printed before `rows: 35 (all files)`; `ACTIONLINT-SELFTEST OK`. The self-test
covers every changed function: `path_filter_self_test` (1066), `branch_filter_self_test` (1142,
1173), `cargo_lock_step_self_test` (2482-2578; the two known 3.2 rows must still be the ONLY rows —
their text and count do not change), and check 3's `selftest_expect_tag` runs only in the full gate
(5618; CI covers it).

- [ ] **Step 4: Commit**

```
git add ci/actionlint/run.sh
git commit -m "fix(ci): replace early-exit readers in repo:actionlint's own helpers (SMA-647)" -m "Process substitution for the grep -q probes, sed -n 1p for the head -1 and grep -m1 value reads. Same first-match results, no producer left to die of SIGPIPE." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 3: `ci.yml:271`, its five check-8d copies and its pin

**Files:**
- Modify: `.github/workflows/ci.yml:271`
- Modify: `ci/actionlint/run.sh:4273, 4304, 4340, 4375, 4408` (check 8d fixtures `healthy`,
  `if_false`, `subset`, `branch_swap`, `double_invoke`)
- Modify: `ci/affected-graph/ci_targets.py:105` (`MOON_CI_BRANCH_BLOCK`)

**Interfaces:** `MOON_CI_BRANCH_BLOCK[2]` changes text; `block_execution_verdict`'s expectations do
not change (it derives them from `MOON_STEP_EVENT_PATHS`).

**Idiom:** I1. The producer is `printf '%s' "$BEFORE"` (a 40-character sha, one write). It cannot
fail except by a write error. The status is used by `!` inside `elif`, under `set -euo pipefail`.

- ci.yml old: `          elif [ -n "${BEFORE:-}" ] && ! printf '%s' "$BEFORE" | grep -qE '^0+$'; then`
- ci.yml new: `          elif [ -n "${BEFORE:-}" ] && ! grep -qE '^0+$' < <(printf '%s' "$BEFORE"); then`
- run.sh copies (5×, identical, inside single-quoted fixtures) old:
  `          elif [ -n "${BEFORE:-}" ] && ! printf '"'"'%s'"'"' "$BEFORE" | grep -qE '"'"'^0+$'"'"'; then`
- run.sh copies new:
  `          elif [ -n "${BEFORE:-}" ] && ! grep -qE '"'"'^0+$'"'"' < <(printf '"'"'%s'"'"' "$BEFORE"); then`
- ci_targets.py:105 old:
  `    "          elif [ -n \"${BEFORE:-}\" ] && ! printf '%s' \"$BEFORE\" | grep -qE '^0+$'; then",`
- ci_targets.py:105 new:
  `    "          elif [ -n \"${BEFORE:-}\" ] && ! grep -qE '^0+$' < <(printf '%s' \"$BEFORE\"); then",`

Check 8d executes the step with `"$bash_bin" -c` and `PATH=<stub>:/usr/bin:/bin`. A process
substitution needs only `/dev/fd`, and `grep` is in `/usr/bin` on both runner OSes, so the executed
block keeps working. Check 8's rules (`swallowed`, `wrapped`, …) look only at `moon` lines, and
check 8b's `T_INVOCATION_ALLOWLIST` holds only the `moon ci` line under this `elif`.

- [ ] **Step 1: Write the failing check.** Create `$S/sma647/before-branch.sh`:

```bash
#!/bin/bash
# The new elif condition, on the three BEFORE shapes. Expected: run, run, ci.
set -euo pipefail
for BEFORE in "" "$(printf '%040d' 0)" "$(printf '%040d' 1)"; do
  if [ -n "${BEFORE:-}" ] && ! grep -qE '^0+$' < <(printf '%s' "$BEFORE"); then echo ci; else echo run; fi
done
```

Run `/bin/bash $S/sma647/scan.sh .github/workflows/ci.yml ci/actionlint/run.sh:4` → expected rows
`ci.yml:271`, `run.sh:4273`, `:4304`, `:4340`, `:4375`, `:4408` (and no other `run.sh:4…` row).

- [ ] **Step 2: Implement.** Edit `ci.yml:271`; Edit `run.sh` with `replace_all: true` on the copy
text (exactly five occurrences — confirm with `grep -c` before: 5, after: 0 of the old text and 5 of
the new); Edit `ci_targets.py:105`.

- [ ] **Step 3: Run it and see it pass**

```
/bin/bash $S/sma647/before-branch.sh
/bin/bash $S/sma647/scan.sh .github/workflows/ci.yml ci/actionlint/run.sh:4
/bin/bash $S/sma647/selftest-actionlint.sh pre-task4
cd $WT && python3 ci/affected-graph/ci_targets.py --self-test
cd $WT && python3 ci/affected-graph/ci_targets.py
```

Expected: `run`, `run`, `ci`; no scan row before `rows: 29 (all files)`; `ACTIONLINT-SELFTEST OK`
(`block_execution_self_test` executes all five changed fixtures on four event paths);
`ci-targets self-test OK`; `PASS  ci-targets -> 32 targets: …` (the `MOON_CI_BRANCH_BLOCK` pin now
equals `ci.yml`).

- [ ] **Step 4: Commit**

```
git add .github/workflows/ci.yml ci/actionlint/run.sh ci/affected-graph/ci_targets.py
git commit -m "fix(ci): read BEFORE's zero-sha check through process substitution (SMA-647)" -m "ci.yml's moon ci step, its five check-8d fixture copies and the MOON_CI_BRANCH_BLOCK pin change together." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 4: `ci/release-plan/run.sh` (83, 187, 202, 341) and its pins

**Files:**
- Modify: `ci/release-plan/run.sh:83, 187, 202, 341`
- Modify: `ci/affected-graph/ci_targets.py:1140, 1146` (`RELEASE_PLAN_SH_CALL_SITES`)

**Interfaces:** `github_output()` and `negative_control()` keep their contracts. `wired_release_plan`
is derived from the tuple, so it follows the tuple with no edit.

**Idiom:** I1 at all four. The producer is `printf` of `$out` / `$mut8_out`. The real producers
(`uv run … release_plan.py`) already ran into the variable, and their status is already captured
(`|| rc=$?` at 82, `|| true` at 185/200, `|| mut8_rc=$?` at 339). So `pipefail` never caught a real
producer failure at these four lines; it could only add the false 141. At line 83 (the runtime
fail-safe) a false 141 caused an unneeded build and a `::warning::`; at 187/202/341 it caused a false
negative-control failure. Line 341 is the M-1 local instance: it fails on every run on this host.

| Line | Old | New |
|---|---|---|
| 83 | `  if [ "$rc" -ne 0 ] \|\| ! printf '%s\n' "$out" \| grep -qE '^nothing_to_release=(true\|false)$'; then` | `  if [ "$rc" -ne 0 ] \|\| ! grep -qE '^nothing_to_release=(true\|false)$' < <(printf '%s\n' "$out"); then` |
| 187 | `  if ! printf '%s\n' "$out" \| grep -q '^nothing_to_release=true$'; then` | `  if ! grep -q '^nothing_to_release=true$' < <(printf '%s\n' "$out"); then` |
| 202 | `  if ! printf '%s\n' "$out" \| grep -q '^nothing_to_release=false$'; then` | `  if ! grep -q '^nothing_to_release=false$' < <(printf '%s\n' "$out"); then` |
| 341 | `  if ! printf '%s\n' "$mut8_out" \| grep -q "a non-table \[workspace\] is inconclusive"; then` | `  if ! grep -q "a non-table \[workspace\] is inconclusive" < <(printf '%s\n' "$mut8_out"); then` |

(`\|` is Markdown escaping.) `ci/release-plan/run.sh` is `set -euo pipefail`; each new line is an
`if` condition, so errexit does not apply to it.

Pins, ci_targets.py. Old 1140:
```python
    'if [ "$rc" -ne 0 ] || ! printf \'%s\\n\' "$out" | grep -qE \'^nothing_to_release=(true|false)$\'; then',
```
New:
```python
    'if [ "$rc" -ne 0 ] || ! grep -qE \'^nothing_to_release=(true|false)$\' < <(printf \'%s\\n\' "$out"); then',
```
Old 1146:
```python
    "if ! printf '%s\\n' \"$mut8_out\" | grep -q \"a non-table \\[workspace\\] is inconclusive\"; then",
```
New:
```python
    "if ! grep -q \"a non-table \\[workspace\\] is inconclusive\" < <(printf '%s\\n' \"$mut8_out\"); then",
```

- [ ] **Step 1: Run the failing test**

```
/bin/bash $S/sma647/bounded.sh 300 $S/sma647/rp-neg.log /bin/bash ci/release-plan/run.sh --negative-control
```
(run from `$WT`). Expected on this host: `rc=1`, and the log contains `FAIL the mutant exited 3
without reporting the non-table [workspace] row` although the mutant output below it contains
`'a non-table [workspace] is inconclusive'` (M-1). If your host passes this row, the test is the
scan instead: `/bin/bash $S/sma647/scan.sh ci/release-plan/` → rows 83, 187, 202, 341.

- [ ] **Step 2: Implement** — the four Edits in `run.sh`, then the two Edits in `ci_targets.py`.

- [ ] **Step 3: Run it and see it pass** (from `$WT`)

```
/bin/bash $S/sma647/bounded.sh 300 $S/sma647/rp-self.log /bin/bash ci/release-plan/run.sh --self-test
/bin/bash $S/sma647/bounded.sh 300 $S/sma647/rp-neg.log /bin/bash ci/release-plan/run.sh --negative-control
/bin/bash $S/sma647/bounded.sh 300 $S/sma647/rp-assert.log /bin/bash ci/release-plan/run.sh --assert
/bin/bash $S/sma647/scan.sh ci/release-plan/
python3 ci/affected-graph/ci_targets.py --self-test
python3 ci/affected-graph/ci_targets.py
/bin/bash $S/sma647/selftest-actionlint.sh
```

Expected: `rc=0` three times (the negative control prints `== release-plan negative control passed ==`);
no scan row before `rows: 25 (all files)`; `ci-targets self-test OK`; `PASS  ci-targets …`;
`ACTIONLINT-SELFTEST OK` in PLAIN mode — from this task on, no `pre-task4` argument: the check-11
row must be gone, because `release_plan_self_test` runs `--negative-control`.

Every `RELEASE_PLAN_SH_CALL_SITES` entry must still occur EXACTLY ONCE in the script (the tuple's
own comment says so). `python3 ci/affected-graph/ci_targets.py` fails if a pinned line is missing.
For the count, `grep -c 'nothing_to_release=(true|false)' ci/release-plan/run.sh` → `3` (line 83,
the unchanged `grep -E … | tail -n 1` at line 95, which reads to EOF and is not in the class, and
the unchanged `grep -cE` over a file at line 230), the same count as before the edit,
and `grep -c 'a non-table \\\[workspace\\\] is inconclusive' ci/release-plan/run.sh` → `1`.

- [ ] **Step 4: Commit**

```
git add ci/release-plan/run.sh ci/affected-graph/ci_targets.py
git commit -m "fix(ci): stop release-plan's verdict probes losing their producer to SIGPIPE (SMA-647)" -m "Row 8 of the negative control failed on every local run on macOS: the mutant printed the expected row and the piped grep -q still read as a miss. RELEASE_PLAN_SH_CALL_SITES follows the two pinned lines." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 5: `ci/workflow-credentials/run.sh` (93, 97) — the fail-OPEN site, I2

**Files:**
- Modify: `ci/workflow-credentials/run.sh:44` (comment), `:51` (`local` line), `:90-100`
- Modify: `ci/affected-graph/ci_targets.py:1079-1091` (tuple + comment), `:2519-2522` (fixture)
- Modify: `CLAUDE.md:546-549` (the `WORKFLOW_CREDENTIALS_SH_CALL_SITES` sentence)

**Interfaces:**
- Produces in `negative_control()`: `subjects_out` (the real run's stdout), `subjects_rc` (its exit
  status), both `local`. One new FAIL row: `FAIL the real run exited <rc>, so the subject-set rows
  below cannot assert`.
- `WORKFLOW_CREDENTIALS_SH_CALL_SITES`: 5 → 7 entries (D-4).

**Idiom:** I2. **Analysis — was `pipefail` catching a real producer failure?** The producer is
`bash "$0"`, the REAL checker run, and it CAN fail (rc 1 on a credential finding, rc 2 on infra).
At line 93 the old pipeline was the condition of an `if` whose body is the FAILURE branch. Under
`pipefail` a failing checker made the whole condition false, so the release.yml row PASSED: the
producer failure was not caught, it was hidden (fail-OPEN). A false 141 on the matching path did
the same: the row exists to see `release.yml` in the subject set, and exactly when `grep -q` found it
and exited, the producer could die, the status became 141, and the row passed (fail-OPEN). At line
97 (`if ! …`) a failing checker DID red the row, but with a wrong message ("printed no subjects
line"). The I2 form captures once, asserts the status on its own line with its own message, and then
matches the captured text with I1. A checker failure is now a red with the right message, and the
release.yml match can no longer race.

- [ ] **Step 1: Write the failing test** — update the pins first (`ci_targets.py`), so the pin check
fails until the script changes. Old tuple:

```python
WORKFLOW_CREDENTIALS_SH_CALL_SITES = (
    "--negative-control) MODE=negctl;   shift ;;",
    "negctl)   negative_control ;;",
    "if bash \"$0\" 2>/dev/null | grep '^workflow-credentials: subjects:' | grep -q 'release.yml'; then",
    'if [ "$failures" -gt 0 ]; then',
    "printf 'workflow-credentials negative control: %d row(s) failed\\n' \"$failures\" >&2",
)
```

New tuple:

```python
WORKFLOW_CREDENTIALS_SH_CALL_SITES = (
    "--negative-control) MODE=negctl;   shift ;;",
    "negctl)   negative_control ;;",
    "subjects_out=\"$(bash \"$0\" 2>/dev/null)\" || subjects_rc=$?",
    'if [ "$subjects_rc" -ne 0 ]; then',
    "if grep -q '^workflow-credentials: subjects:.*release.yml' < <(printf '%s\\n' \"$subjects_out\"); then",
    'if [ "$failures" -gt 0 ]; then',
    "printf 'workflow-credentials negative control: %d row(s) failed\\n' \"$failures\" >&2",
)
```

In the comment above the tuple, old text:

```python
# The fifth entry is an ASSERTION line, added after the first four were measured to be
```

New text:

```python
# SMA-647 split the one assertion line into three (entries 3-5): the capture of the real run, the
# guard on its exit status, and the match. The old line piped the real run into an early-exit grep
# under `pipefail`, so a failing checker AND a SIGPIPE on the producer both made the release.yml
# row pass (fail-OPEN). Deleting the status guard alone brings that back, so it is pinned too.
#
# The fifth entry is an ASSERTION line, added after the first four were measured to be
```

Fixture, old text (`ci_targets.py:2519-2522`):

```python
        # The assertion line (SMA-593 F1). Indented in the real script, so it also exercises the
        # stripped-whole-line matching this haystack uses.
        '  if bash "$0" 2>/dev/null | grep \'^workflow-credentials: subjects:\' '
        "| grep -q 'release.yml'; then\n"
```

New text:

```python
        # The assertion lines (SMA-593 F1; split into capture, status guard and match by SMA-647).
        # Indented in the real script, so they also exercise the stripped-whole-line matching this
        # haystack uses.
        '  subjects_out="$(bash "$0" 2>/dev/null)" || subjects_rc=$?\n'
        '  if [ "$subjects_rc" -ne 0 ]; then\n'
        "  if grep -q '^workflow-credentials: subjects:.*release.yml' "
        "< <(printf '%s\\n' \"$subjects_out\"); then\n"
```

- [ ] **Step 2: Run it and see it fail** (from `$WT`)

```
python3 ci/affected-graph/ci_targets.py --self-test
python3 ci/affected-graph/ci_targets.py
```

Expected: `--self-test` prints `ci-targets self-test OK` (the fixture and the tuple agree); the real
run FAILS and names the three missing lines under `ci/workflow-credentials/run.sh`.

- [ ] **Step 3: Implement.** In `ci/workflow-credentials/run.sh`, old (`:44`):

```bash
# These five discrete lines are pinned by ci_targets.py. Pinning the moon.yml INVOCATION
```

New:

```bash
# These seven discrete lines are pinned by ci_targets.py. Pinning the moon.yml INVOCATION
```

Old (`:51`):

```bash
  local failures=0 tmp rc
```

New:

```bash
  local failures=0 tmp rc subjects_out subjects_rc
```

Old (`:90-100`):

```bash
  # Greps the `subjects:` line the checker prints (pre-flight ruling 2). A count-only line
  # would make this row match nothing and assert nothing regardless of what discovery did.
  if bash "$0" 2>/dev/null | grep '^workflow-credentials: subjects:' | grep -q 'release.yml'; then
    printf '  FAIL release.yml appeared in the subject set; it has no pull_request trigger\n' >&2
    failures=$((failures + 1))
  fi
  if ! bash "$0" 2>/dev/null | grep -q '^workflow-credentials: subjects:'; then
    printf '  FAIL the checker printed no subjects line — the row above cannot assert\n' >&2
    failures=$((failures + 1))
  fi
```

New:

```bash
  # Greps the `subjects:` line the checker prints (pre-flight ruling 2). A count-only line
  # would make this row match nothing and assert nothing regardless of what discovery did.
  #
  # Captured ONCE, with the checker's own status asserted on its own line (SMA-647). The old
  # form piped the real run into an early-exit grep under `pipefail`. A failing checker made the
  # release.yml row PASS (a failed pipeline reads as "no match"), and so did a SIGPIPE on the
  # producer at the exact moment grep found release.yml. Both failed OPEN on the row this control
  # exists for.
  subjects_rc=0
  subjects_out="$(bash "$0" 2>/dev/null)" || subjects_rc=$?
  if [ "$subjects_rc" -ne 0 ]; then
    printf '  FAIL the real run exited %s, so the subject-set rows below cannot assert\n' "$subjects_rc" >&2
    failures=$((failures + 1))
  fi
  if grep -q '^workflow-credentials: subjects:.*release.yml' < <(printf '%s\n' "$subjects_out"); then
    printf '  FAIL release.yml appeared in the subject set; it has no pull_request trigger\n' >&2
    failures=$((failures + 1))
  fi
  if ! grep -q '^workflow-credentials: subjects:' < <(printf '%s\n' "$subjects_out"); then
    printf '  FAIL the checker printed no subjects line — the row above cannot assert\n' >&2
    failures=$((failures + 1))
  fi
```

Semantics: the old two greps selected the `subjects:` line, then looked for `release.yml` in it (the
`.` matches any character in both). The new single BRE `^workflow-credentials: subjects:.*release.yml`
matches the same lines. The script is `set -euo pipefail`: the capture is the left side of `||`, so
errexit does not stop it; the `local` is on its own line (spec §5.1 pitfall).

In `CLAUDE.md`, old text:

```
  `WORKFLOW_CREDENTIALS_SH_CALL_SITES` pins **five** lines in `run.sh`, and the fifth is an
  ASSERTION line: with only the flag parse, the dispatch arm and the two report lines pinned,
  deleting every `_expect` and `grep` row left all four byte-identical and the control exited 0
  having asserted nothing (MEASURED).
```

New text:

```
  `WORKFLOW_CREDENTIALS_SH_CALL_SITES` pins **seven** lines in `run.sh`, and three of them are the
  ASSERTION: with only the flag parse, the dispatch arm and the two report lines pinned,
  deleting every `_expect` and `grep` row left all four byte-identical and the control exited 0
  having asserted nothing (MEASURED). SMA-647 split the one assertion line into three: the capture
  of the real run, the guard on its exit status, and the match. The old pipe failed OPEN when the
  checker failed, so deleting the status guard alone is a regression, and it is pinned too.
```

- [ ] **Step 4: Run it and see it pass** (from `$WT`)

```
/bin/bash $S/sma647/bounded.sh 300 $S/sma647/wc-self.log /bin/bash ci/workflow-credentials/run.sh --self-test
/bin/bash $S/sma647/bounded.sh 300 $S/sma647/wc-neg.log /bin/bash ci/workflow-credentials/run.sh --negative-control
/bin/bash $S/sma647/bounded.sh 300 $S/sma647/wc-real.log /bin/bash ci/workflow-credentials/run.sh
/bin/bash $S/sma647/scan.sh ci/workflow-credentials/
python3 ci/affected-graph/ci_targets.py --self-test
python3 ci/affected-graph/ci_targets.py
```

Expected: `rc=0` three times, and `wc-neg.log` ends with `== workflow-credentials negative control
passed ==`; no scan row before `rows: 23 (all files)`; `ci-targets self-test OK` (its battery now
deletes each of the seven entries in turn); `PASS  ci-targets …`.

Prove the new guard bites (then undo by Edit, not `git checkout`): temporarily change
`  subjects_out="$(bash "$0" 2>/dev/null)" || subjects_rc=$?` to
`  subjects_out="$(bash "$0" --no-such-flag 2>/dev/null)" || subjects_rc=$?`, run
`--negative-control` → expected `rc=1` and the log contains `FAIL the real run exited 2`. Restore the
line with a second Edit and re-run → `rc=0`. (The old form passed the release.yml row in this state.)

- [ ] **Step 5: Commit**

```
git add ci/workflow-credentials/run.sh ci/affected-graph/ci_targets.py CLAUDE.md
git commit -m "fix(ci): capture workflow-credentials' real run before matching it (SMA-647)" -m "The release.yml row piped the real run into grep -q under pipefail, so a failing checker or a SIGPIPE on the producer made the row pass. Capture once, assert the status, then match. WORKFLOW_CREDENTIALS_SH_CALL_SITES pins all three lines." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 6: `ci/ruff/run.sh:148` and `ci/publish-metadata/run.sh:536, 1500`

**Files:** `ci/ruff/run.sh:147-148`; `ci/publish-metadata/run.sh:536`, `:1498-1505`.

**Interfaces:** `_row` (ruff) keeps its arguments; one new FAIL row: `FAIL <label>: ruff_corpus
exited <rc>, so the row cannot assert`. The publish-metadata negative control gains the rc in its
existing failure message.

### ruff 148 — I2

**Analysis.** The producer `ruff_corpus` is `git ls-files … | sort`, and it CAN fail. The old
`ruff_corpus "$tmp" | grep -qx "$path" || got=1` turned a producer failure into `got=1` ("absent").
For the rows that expect ABSENT (`want=1`: `non-.py is not found`, the `.venv` row), a git failure
therefore PASSED the row (fail-OPEN); for `want=0` rows it failed with a misleading message. The I2
form asserts the corpus status first, as its own FAIL row.

Old:

```bash
    local label="$1" want="$2" path="$3" got=0
    ruff_corpus "$tmp" | grep -qx "$path" || got=1
```

New:

```bash
    local label="$1" want="$2" path="$3" got=0 corpus corpus_rc=0
    # Capture, check, then match (SMA-647). The old pipe read a failed ruff_corpus as "absent",
    # which PASSED every row that expects absence.
    corpus="$(ruff_corpus "$tmp")" || corpus_rc=$?
    if [ "$corpus_rc" -ne 0 ]; then
      printf '  FAIL %s: ruff_corpus exited %s, so the row cannot assert\n' "$label" "$corpus_rc" >&2
      failures=$((failures + 1))
      return 0
    fi
    grep -qx "$path" < <(printf '%s\n' "$corpus") || got=1
```

(`ci/ruff/run.sh` is `set -euo pipefail`; the capture is the left side of `||`. `RUFF_SH_CALL_SITES`
does not pin these lines; its `git -C "$root" ls-files … | sort` entry is inside `ruff_corpus` and
does not change.)

### publish-metadata 536 — value reader

Old:
```bash
  hit_line="$(grep -nE '^[[:space:]]*(- )?run:[^#]*ci/publish-metadata/run\.sh[^#]*--check-categories-freshness' "$wf" | head -n1 || true)"
```
New:
```bash
  hit_line="$(grep -nE '^[[:space:]]*(- )?run:[^#]*ci/publish-metadata/run\.sh[^#]*--check-categories-freshness' "$wf" | sed -n 1p || true)"
```
The `|| true` stays: grep's no-match status must not stop the `set -e` script, and the `[ -z ]`
check below reports it. The file is readable (checked just above), so grep's rc 2 was not a live
path. Same output: the first matching line.

### publish-metadata 1500 — I2

**Analysis.** `pypi_scan_paths` CAN fail (rc 2 with a `FATAL:` line on stderr). Under `pipefail` the
old `if pypi_scan_paths … | grep -q …; then ok; else NEGATIVE CONTROL FAILED` sent a producer failure
to the failure branch: the failure WAS caught, with a message that named the wrong cause. A false 141
also went there (fail-closed). The I2 form keeps the producer failure a red and adds its rc to the
message.

Old:
```bash
  if pypi_scan_paths "$scanroot" | grep -q '/py/packages/newpkg/pyproject.toml$'; then
    echo "  ok — Check P0 (a NEW py/packages member enters the scan set automatically)"
  else
    echo "NEGATIVE CONTROL FAILED: discovery missed a new py/packages member" >&2
    failures=$((failures + 1))
  fi
```
New:
```bash
  # Capture, check, then match (SMA-647): a piped grep -q could lose the producer to SIGPIPE.
  local newpkg_scan newpkg_rc=0
  newpkg_scan="$(pypi_scan_paths "$scanroot")" || newpkg_rc=$?
  if [ "$newpkg_rc" -eq 0 ] && grep -q '/py/packages/newpkg/pyproject.toml$' < <(printf '%s\n' "$newpkg_scan"); then
    echo "  ok — Check P0 (a NEW py/packages member enters the scan set automatically)"
  else
    echo "NEGATIVE CONTROL FAILED: discovery missed a new py/packages member (pypi_scan_paths exited $newpkg_rc)" >&2
    failures=$((failures + 1))
  fi
```

- [ ] **Step 1: Run the failing check.** Create `$S/sma647/pm-1500.sh`. It runs the REAL rewritten
P0 block, extracted from the file, against a stubbed `pypi_scan_paths` (a hit, a miss, and a failing
producer). The whole negative control hangs under Homebrew bash on this host (M-3), so this is the
local check for line 1500:

```bash
#!/bin/bash
# Runs the REAL rewritten P0 block of ci/publish-metadata/run.sh (extracted) against a stubbed
# pypi_scan_paths: a hit, a miss, and a failing producer.
set -uo pipefail
F="${1:-/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-647/ci/publish-metadata/run.sh}"
block="$(awk '/# Capture, check, then match \(SMA-647\): a piped grep -q could lose/{f=1} f{print} f&&/^  fi$/{exit}' "$F")"
[ -n "$block" ] || { echo "pm-1500: the SMA-647 block is not in $F"; exit 1; }
run_case() { # $1 stub rc, $2 stub output
  local failures=0 scanroot=/x
  pypi_scan_paths() { printf '%s\n' "$STUB_OUT"; return "$STUB_RC"; }
  STUB_RC="$1"; STUB_OUT="$2"
  eval "$block"
  echo "failures=$failures"
}
a="$(run_case 0 '/x/py/packages/newpkg/pyproject.toml' 2>&1)"
b="$(run_case 0 '/x/py/packages/other/pyproject.toml' 2>&1)"
c="$(run_case 2 '/x/py/packages/newpkg/pyproject.toml' 2>&1)"
echo "--- hit";      printf '%s\n' "$a"
echo "--- miss";     printf '%s\n' "$b"
echo "--- producer"; printf '%s\n' "$c"
case "$a" in *'ok — Check P0'*'failures=0') ;; *) echo "pm-1500: hit case wrong"; exit 1 ;; esac
case "$b" in *'NEGATIVE CONTROL FAILED'*'exited 0)'*'failures=1') ;; *) echo "pm-1500: miss case wrong"; exit 1 ;; esac
case "$c" in *'NEGATIVE CONTROL FAILED'*'exited 2)'*'failures=1') ;; *) echo "pm-1500: producer case wrong"; exit 1 ;; esac
echo "pm-1500: OK"
```

Run:

```
/bin/bash $S/sma647/scan.sh ci/ruff/ ci/publish-metadata/
/bin/bash $S/sma647/pm-1500.sh
```

Expected: rows `ci/publish-metadata/run.sh:1500`, `:536`, `ci/ruff/run.sh:148`; and
`pm-1500: the SMA-647 block is not in …` (exit 1). (This harness was checked during planning on a
scratch copy with the Step 2 edit applied: `pm-1500: OK` under bash 3.2.57 and 5.3.15.)

- [ ] **Step 2: Implement** the three Edits above.

- [ ] **Step 3: Run it and see it pass** (from `$WT`; both gate scripts need bash 4+)

```
/bin/bash $S/sma647/scan.sh ci/ruff/ ci/publish-metadata/
/bin/bash $S/sma647/pm-1500.sh
/bin/bash $S/sma647/bounded.sh 300 $S/sma647/ruff-self.log /opt/homebrew/bin/bash ci/ruff/run.sh --self-test
/bin/bash $S/sma647/bounded.sh 300 $S/sma647/ruff-neg.log /opt/homebrew/bin/bash ci/ruff/run.sh --negative-control
/bin/bash $S/sma647/bounded.sh 300 $S/sma647/ruff-real.log /opt/homebrew/bin/bash ci/ruff/run.sh
/bin/bash $S/sma647/bounded.sh 600 $S/sma647/pm-neg.log /opt/homebrew/bin/bash ci/publish-metadata/run.sh --negative-control
```

Expected: no scan row before `rows: 20 (all files)`; `pm-1500: OK`; `rc=0` three times for ruff.
The publish-metadata negative control is EXPECTED to `TIMEOUT` with near-zero CPU on this host
(M-3, measured on the unmodified tree). If it does finish, it must be `rc=0` and `pm-neg.log` must
contain `ok — Check P0 (a NEW py/packages member enters the scan set automatically)`. Either way CI
decides `repo:publish-metadata` (line 536 runs in its real run, which also needs `cargo publish
--dry-run`).

- [ ] **Step 4: Commit**

```
git add ci/ruff/run.sh ci/publish-metadata/run.sh
git commit -m "fix(ci): capture ruff_corpus and pypi_scan_paths before matching them (SMA-647)" -m "A failed ruff_corpus read as 'absent' and passed every expects-absent row. Both producers are now captured, their status asserted, and the match done by process substitution; publish-metadata's check 4 reads its first hit with sed -n 1p." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 7: semantic-release module, check-branch hook, `moon.yml:373`, `ops/nats/check-subjects.sh`

**Files:** `ci/release-parity/ecosystems/semantic-release.sh:34-35`,
`.lefthook/pre-push/check-branch.sh:22`, `moon.yml:373`, `ops/nats/check-subjects.sh:46, 56, 101, 108`.

**Interfaces:** none change.

All I1 producers here are `printf` of a variable (only a write error can fail it, so `pipefail`
never caught a real producer failure). The two value readers keep `|| true` and their first-match
output.

| Site | Idiom | Old | New |
|---|---|---|---|
| semantic-release.sh:34 | I1 | `  if printf '%s' "$subject" \| grep -qE '^[a-z]+(\([^)]*\))?!:' \` | `  if grep -qE '^[a-z]+(\([^)]*\))?!:' < <(printf '%s' "$subject") \` |
| semantic-release.sh:35 | I1 | `     \|\| printf '%s' "$footer" \| grep -q 'BREAKING CHANGE'; then` | `     \|\| grep -q 'BREAKING CHANGE' < <(printf '%s' "$footer"); then` |
| check-branch.sh:22 | I1 | `if printf '%s' "$branch" \| grep -Eq '^feature/[a-z0-9._-]+$'; then` | `if grep -Eq '^feature/[a-z0-9._-]+$' < <(printf '%s' "$branch"); then` |
| moon.yml:373 | I1 | `      if printf '%s\n' "$out" \| grep -qw getrandom; then` | `      if grep -qw getrandom < <(printf '%s\n' "$out"); then` |
| check-subjects.sh:46 | value | `  start=$(grep -nF -- "$nkey_placeholder" "$tmpl" \| head -1 \| cut -d: -f1 \|\| true)` | `  start=$(grep -nF -- "$nkey_placeholder" "$tmpl" \| sed -n 1p \| cut -d: -f1 \|\| true)` |
| check-subjects.sh:56 | value | `    next_start=$(grep -nF -- "$next_placeholder" "$tmpl" \| head -1 \| cut -d: -f1 \|\| true)` | `    next_start=$(grep -nF -- "$next_placeholder" "$tmpl" \| sed -n 1p \| cut -d: -f1 \|\| true)` |
| check-subjects.sh:101 | I1 | `    if ! printf '%s\n' "$pub_declared" \| grep -qxF -- "$g"; then` | `    if ! grep -qxF -- "$g" < <(printf '%s\n' "$pub_declared"); then` |
| check-subjects.sh:108 | I1 | `    if ! printf '%s\n' "$sub_declared" \| grep -qxF -- "$g"; then` | `    if ! grep -qxF -- "$g" < <(printf '%s\n' "$sub_declared"); then` |

(`\|` is Markdown escaping.) Contexts: semantic-release.sh is `set -euo pipefail` and sourced by
`ci/release-parity/run.sh` under bash; check-branch.sh is `set -euo pipefail`, invoked by lefthook
through its `#!/usr/bin/env bash` shebang; `moon.yml:373` is a Moon `script:` (bash, no `pipefail`
there, so it was safe today — D3); `check-subjects.sh` is `set -euo pipefail` and requires bash 4.3.
Do NOT touch the existing here-strings in `check-subjects.sh` (`<<< "$block"` etc.): they are not in
this issue's class, and changing them is out of scope.

- [ ] **Step 1: Run the failing check:**
`/bin/bash $S/sma647/scan.sh ci/release-parity/ .lefthook/ moon.yml: ops/nats/` → 8 rows
(semantic-release 34, 35; check-branch 22; moon.yml:373; check-subjects 46, 56, 101, 108).

- [ ] **Step 2: Implement** the eight Edits.

- [ ] **Step 3: Run it and see it pass** (from `$WT`)

```
/bin/bash $S/sma647/scan.sh ci/release-parity/ .lefthook/ moon.yml: ops/nats/
/bin/bash $S/sma647/bounded.sh 60 $S/sma647/cb.log /bin/bash .lefthook/pre-push/check-branch.test.sh
/bin/bash $S/sma647/bounded.sh 60 $S/sma647/nats.log /opt/homebrew/bin/bash ops/nats/check-subjects.sh
/bin/bash $S/sma647/bounded.sh 600 $S/sma647/wasm.log moon run repo:wasm-getrandom-free --force
/bin/bash $S/sma647/bounded.sh 900 $S/sma647/rpts-neg.log /bin/bash ci/release-parity/run.sh --ecosystem semantic-release --negative-control
/bin/bash $S/sma647/bounded.sh 900 $S/sma647/rpts-real.log /bin/bash ci/release-parity/run.sh --ecosystem semantic-release
```

Expected: no scan row before `rows: 12 (all files)`; `rc=0` five times; `cb.log` has only `ok:`
lines; `nats.log` ends `ops/nats: accounts.conf.tmpl grants exactly the subjects declared in
subjects.env`. Baseline on the unmodified tree (measured): check-branch rc 0, nats rc 0 in ~1 s.
The semantic-release module's `ecosystem::expected` runs inside both release-parity runs.

- [ ] **Step 4: Commit**

```
git add ci/release-parity/ecosystems/semantic-release.sh .lefthook/pre-push/check-branch.sh moon.yml ops/nats/check-subjects.sh
git commit -m "fix(ci): remove early-exit readers from four more shell surfaces (SMA-647)" -m "semantic-release's breaking-change probe, the branch-name hook, wasm-getrandom-free and the NATS subject check." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 8: `ci/images/run.sh` (49, 50, 60, 77, 78, 117) — value readers

**Files:** `ci/images/run.sh:49, 50, 60, 77, 78, 117` (inside `assert_pins`).

**Interfaces:** `assert_pins` returns the same values.

**Analysis.** `assert_pins` runs under `set -euo pipefail` from `case` (`build) assert_pins; …`),
so errexit applies inside it. The producers are `grep` over files. A grep NO-MATCH (rc 1) made the
assignment fail and stopped the script with no `::error::` line; the rewrite keeps that exactly
(`sed -n 1p` reads to EOF, so `pipefail` still reports grep's 1). What changes: `head -1` could exit
before grep finished, and grep's 141 then also stopped the script with no message (the
`release.yml:913` shape). Line 117 keeps its `|| true`.

| Line | Old | New |
|---|---|---|
| 49 | `  channel="$(grep -E '^channel[[:space:]]*=' "$ROOT/rs/rust-toolchain.toml" \| head -1 \| sed -E 's/.*"([^"]+)".*/\1/')"` | `  channel="$(grep -E '^channel[[:space:]]*=' "$ROOT/rs/rust-toolchain.toml" \| sed -n 1p \| sed -E 's/.*"([^"]+)".*/\1/')"` |
| 50 | `  from_version="$(grep -oE '^FROM rust:[0-9]+\.[0-9]+\.[0-9]+' "$dockerfile" \| head -1 \| sed 's/^FROM rust://')"` | `  from_version="$(grep -oE '^FROM rust:[0-9]+\.[0-9]+\.[0-9]+' "$dockerfile" \| sed -n 1p \| sed 's/^FROM rust://')"` |
| 60 | `  rustup_toolchain="$(grep -oE '^ENV RUSTUP_TOOLCHAIN=[0-9]+\.[0-9]+\.[0-9]+' "$dockerfile" \| head -1 \| sed 's/^ENV RUSTUP_TOOLCHAIN=//')"` | `  rustup_toolchain="$(grep -oE '^ENV RUSTUP_TOOLCHAIN=[0-9]+\.[0-9]+\.[0-9]+' "$dockerfile" \| sed -n 1p \| sed 's/^ENV RUSTUP_TOOLCHAIN=//')"` |
| 77 | `  ubuntu_from="$(grep -oE '^FROM ubuntu:[0-9]+\.[0-9]+' "$dockerfile" \| head -1 \| sed 's/^FROM ubuntu://')"` | `  ubuntu_from="$(grep -oE '^FROM ubuntu:[0-9]+\.[0-9]+' "$dockerfile" \| sed -n 1p \| sed 's/^FROM ubuntu://')"` |
| 78 | `  ubuntu_chisel="$(grep -oE 'chisel cut --release ubuntu-[0-9]+\.[0-9]+' "$dockerfile" \| head -1 \| sed 's/.*ubuntu-//')"` | `  ubuntu_chisel="$(grep -oE 'chisel cut --release ubuntu-[0-9]+\.[0-9]+' "$dockerfile" \| sed -n 1p \| sed 's/.*ubuntu-//')"` |
| 117 | `  start_period="$(grep -oE '\-\-start-period=[0-9]+s' "$dockerfile" \| head -1 \| grep -oE '[0-9]+' \|\| true)"` | `  start_period="$(grep -oE '\-\-start-period=[0-9]+s' "$dockerfile" \| sed -n 1p \| grep -oE '[0-9]+' \|\| true)"` |

(`\|` is Markdown escaping.)

- [ ] **Step 1: Write the failing check.** Create `$S/sma647/images-pins.sh`, which runs the REAL
`assert_pins` from the file with no Docker:

```bash
#!/bin/bash
# Runs ci/images/run.sh's assert_pins, extracted, against the real rs/Dockerfile. No Docker needed.
set -euo pipefail
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-647 || exit 2
ROOT="$PWD"
eval "$(awk '/^assert_pins\(\) \{$/,/^\}$/' ci/images/run.sh)"
set -x
assert_pins
set +x
echo "assert_pins: OK"
```

Run `/bin/bash $S/sma647/images-pins.sh` on the unmodified tree → `assert_pins: OK` (the trace shows
the six values), and `/bin/bash $S/sma647/scan.sh ci/images/` → the six rows.

- [ ] **Step 2: Implement** the six Edits.

- [ ] **Step 3: Run it and see it pass:** `/bin/bash $S/sma647/images-pins.sh` → `assert_pins: OK`
with the same six traced values as in Step 1; `/bin/bash $S/sma647/scan.sh ci/images/` → no row
before `rows: 6 (all files)`. CI note: `images.yml` is not a required check. After the PR is pushed,
the author watches its run (its `pull_request` filter covers `ci/images/**`).

- [ ] **Step 4: Commit**

```
git add ci/images/run.sh
git commit -m "fix(ci): read the image pins with sed -n 1p, not head -1 (SMA-647)" -m "head -1 can exit before grep finishes, and under set -euo pipefail grep's 141 stopped assert_pins with no ::error:: line." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 9: `wheels.yml` (312, 323, 521, 544, 677) and `prebuild.yml:187`

**Files:** `.github/workflows/wheels.yml:312, 323, 521, 544, 677`; `.github/workflows/prebuild.yml:187`.

**Interfaces:** none. All six run under bash: `wheels.yml` 312/323 in a `shell: bash` step on macOS,
521/544 in the `sdist` job (`ubuntu-latest`, default bash), 677 in the `face` job
(`ubuntu-latest`); `prebuild.yml:187` in a step with `if: runner.os == 'macOS'` (default bash).

### wheels.yml:521 — the second fail-OPEN site

**Analysis.** The `if` body is the FAILURE branch (`sdist ships moon.yml`). The producer is
`printf '%s\n' "$listing"`; the real producer, `tar tzf`, ran earlier into `listing` under
`set -euo pipefail`, so a tar failure already stops the step. `pipefail` therefore never caught a
real producer failure here. It could only turn the DETECTION into a pass: when `grep -q` found
`moon.yml` and exited, a later `printf` write got SIGPIPE, the condition became 141 (false), and the
step passed with moon.yml in the sdist. I1 removes that path.

| Line | Idiom | Old | New |
|---|---|---|---|
| 312 | value | `                minos="$(otool -l "$so" \| awk '/LC_BUILD_VERSION/{f=1} f&&/^ *minos/{print $2; exit}')"` | `                minos="$(otool -l "$so" \| awk '/LC_BUILD_VERSION/{f=1} f&&/^ *minos/&&!d{v=$2; d=1} END{print v}')"` |
| 323 | value | `                min="$(otool -l "$so" \| awk '/LC_VERSION_MIN_MACOSX/{getline; getline; print $2; exit}')"` | `                min="$(otool -l "$so" \| awk '/LC_VERSION_MIN_MACOSX/&&!d{getline; getline; v=$2; d=1} END{print v}')"` |
| 521 | I1 | `          if printf '%s\n' "$listing" \| grep -q 'moon\.yml'; then` | `          if grep -q 'moon\.yml' < <(printf '%s\n' "$listing"); then` |
| 544 | I1 | `            printf '%s\n' "$listing" \| grep -q "/$required$" \` | `            grep -q "/$required$" < <(printf '%s\n' "$listing") \` |
| 677 | I1 | `          printf '%s\n' "$meta" \| grep -qE '^Requires-Dist: paigasus-py-bindings==' \` | `          grep -qE '^Requires-Dist: paigasus-py-bindings==' < <(printf '%s\n' "$meta") \` |
| prebuild 187 | value | `          x64min="$(otool -l paigasus-node-bindings.darwin-x64.node \| awk '/LC_VERSION_MIN_MACOSX/{getline; getline; print $2; exit}')"` | `          x64min="$(otool -l paigasus-node-bindings.darwin-x64.node \| awk '/LC_VERSION_MIN_MACOSX/&&!d{getline; getline; v=$2; d=1} END{print v}')"` |

(`\|` is Markdown escaping. The `|| { … }` continuation lines under 544 and 677 do not change.)

**otool sites.** The old `awk … exit` could stop reading while `otool` still wrote; under
`set -euo pipefail` otool's 141 made the assignment fail and the step stopped with no `::error::`
line. The new awk reads to EOF, takes the FIRST match only (`!d`), and prints it in `END`. With no
match it prints an empty line, which `$( )` strips to "" — the same value the old form produced, so
the existing `[ -n "$minos" ]` / `[ "$min" = "10.12" ]` checks still fire with their messages. A real
`otool` failure still fails the assignment through `pipefail`, as before.

**544/677.** The producers are `printf` of captured variables (`tar` and `python3` ran earlier under
errexit). Old false 141 = a false `::error::` (fail-closed). No real producer failure was caught.

- [ ] **Step 1: Write the failing check.** Create `$S/sma647/awk-equiv.sh`:

```bash
#!/bin/bash
# Old vs new otool awk programs on synthetic `otool -l` text. Every pair must print the same value.
set -uo pipefail
two='Load command 9
      cmd LC_VERSION_MIN_MACOSX
  cmdsize 16
  version 10.12
      sdk 14.0
Load command 10
      cmd LC_BUILD_VERSION
  cmdsize 32
 platform 1
    minos 11.0
      sdk 14.0
Load command 11
      cmd LC_VERSION_MIN_MACOSX
  cmdsize 16
  version 10.99
      sdk 14.0
Load command 12
      cmd LC_BUILD_VERSION
  cmdsize 32
 platform 1
    minos 12.9
      sdk 14.0'
none='Load command 1
      cmd LC_SEGMENT_64'
bad=0
for input in "$two" "$none"; do
  a_old="$(printf '%s\n' "$input" | awk '/LC_BUILD_VERSION/{f=1} f&&/^ *minos/{print $2; exit}')"
  a_new="$(printf '%s\n' "$input" | awk '/LC_BUILD_VERSION/{f=1} f&&/^ *minos/&&!d{v=$2; d=1} END{print v}')"
  x_old="$(printf '%s\n' "$input" | awk '/LC_VERSION_MIN_MACOSX/{getline; getline; print $2; exit}')"
  x_new="$(printf '%s\n' "$input" | awk '/LC_VERSION_MIN_MACOSX/&&!d{getline; getline; v=$2; d=1} END{print v}')"
  echo "minos old=[$a_old] new=[$a_new]   x64 old=[$x_old] new=[$x_new]"
  [ "$a_old" = "$a_new" ] && [ "$x_old" = "$x_new" ] || bad=1
done
[ "$bad" -eq 0 ] && echo "awk-equiv: OK" || { echo "awk-equiv: MISMATCH"; exit 1; }
```

Run it now (it tests only the programs, so it passes before the edit too; its job is to prove the
NEW programs match the old ones): expected `minos old=[11.0] new=[11.0]   x64 old=[10.12] new=[10.12]`,
`minos old=[] new=[]   x64 old=[] new=[]`, `awk-equiv: OK`. Then run the failing check:
`/bin/bash $S/sma647/scan.sh .github/workflows/` → the six rows.

- [ ] **Step 2: Implement** the six Edits.

- [ ] **Step 3: Run it and see it pass.** Create `$S/sma647/lint-workflows.sh` (check 1 of the gate
with shellcheck OFF — M-4: with shellcheck on, actionlint does not finish locally):

```bash
#!/bin/bash
# Check 1 of repo:actionlint without its shellcheck half (M-4): syntax, expressions, runner labels.
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-647 || exit 2
actionlint -pyflakes= -shellcheck= .github/workflows/*.yml
rc=$?
echo "actionlint (shellcheck off) rc=$rc"
exit "$rc"
```

Then:

```
/bin/bash $S/sma647/scan.sh .github/workflows/
/bin/bash $S/sma647/bounded.sh 120 $S/sma647/lint-workflows.log /bin/bash $S/sma647/lint-workflows.sh
```

Expected: no scan row, and `rows: 0 (all files)` — the whole corpus is clean now; the lint log
ends `actionlint (shellcheck off) rc=0` (about 1 s, measured on the unmodified tree). The shellcheck
half runs in CI's `repo:actionlint`. CI also runs `wheels.yml` and `prebuild.yml` on the PR because
their `pull_request` filters include the workflow files themselves (spec §8).

- [ ] **Step 4: Commit**

```
git add .github/workflows/wheels.yml .github/workflows/prebuild.yml
git commit -m "fix(ci): remove early-exit readers from the wheels and prebuild workflows (SMA-647)" -m "The sdist moon.yml probe failed OPEN: a SIGPIPE on the producer at the moment grep found moon.yml made the step pass. The otool reads now take the first match in awk's END block instead of exiting early." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 10: Check 13's verdict, fixtures and self-test (no production call yet)

**Files:**
- Create: `ci/actionlint/fixtures/early-exit/fires.txt`, `silent.txt`, `allow.txt`
- Modify: `ci/actionlint/run.sh:48-51` (`SELF_TEST_COUNT`), `:66-70` (usage), after `:4922` (the new
  definitions and self-test), `:4927` ("All FOURTEEN"), `:4958` (`run_self_tests`)
- Modify: `ci/actionlint/README.md:38, 45, 715-718, 720-735, 756, 759`
- Modify: `moon.yml:689-695, 708-711` (actionlint task comments)
- Modify: `CLAUDE.md:362-366` ("currently 14")

**Interfaces:**
- Produces: `EARLY_EXIT_READER_ALLOWED` (array of `<path>:<line>|<reason>|<logical line text>`),
  `EARLY_EXIT_ERE` (string), `early_exit_join <file>` (prints `<path>:<line>\t<logical text>` per
  logical line), `early_exit_reader_rows <list-file>` (unsorted rows),
  `early_exit_reader_verdict <list-file>` (rows sorted `LC_ALL=C`), `early_exit_reader_self_test`.
- Row formats: `early-exit-reader <path>:<line>`, `blank-reason <path>:<line>`,
  `stale-allowlist <path>:<line>`, `no-list`, `no-tmp`, `unreadable <path>`, `join-failed <path>`,
  `grep-failed <rc>`.
- `SELF_TEST_COUNT`: 14 → 15.

**Separator choice.** The allowlist row is `<path>:<line>|<reason>|<logical line text>`. The text is
LAST because it always contains a pipe (it is a violation); the parser takes the location up to the
first `|`, the reason up to the second, and the text is the rest, so the text's own pipes cannot
shift a field. The location cannot contain `|`: no tracked path contains one (measured: 0), and
`:<line>` is digits. The reason must not contain `|` (stated at the table). A row with fewer than two
`|` is treated as having no reason and no text, so it never waives a line (it shows as
`stale-allowlist`, and the line still fires): it fails closed.

- [ ] **Step 1: Create the fixtures.**

`ci/actionlint/fixtures/early-exit/fires.txt` (lines 3-18, 20, 23, 24, 25 must fire; 18+19 and
20+21 are joined; 22 is a comment that ends in a pipe and must NOT hide line 23):

```
# SPDX-License-Identifier: Apache-2.0
# SMA-647 check 13 fixture, read by early_exit_reader_self_test: every logical line below fires.
printf '%s' "$v" | grep -q x
printf '%s' "$v" | grep -qF x
printf '%s' "$v" | grep -nxm1 x
printf '%s' "$v" | grep --quiet x
printf '%s' "$v" | grep --silent x
printf '%s' "$v" | grep -m1 x
printf '%s' "$v" | grep -m 1 x
printf '%s' "$v" | grep --max-count=1 x
printf '%s' "$v" | grep -E -q x
printf '%s\n' "$v" | head -1
v="$(printf '%s\n' "$v" | head)"
printf '%s\n' "$v" | head -n1 | cut -d: -f1
otool -l "$so" | awk '/X/{print $2; exit}'
printf '%s' "$v" | LC_ALL=C grep -q x
printf '%s' "$v" | command grep -q x
printf '%s' "$v" |
  grep -q x
printf '%s' "$v" |\
  grep -q x
# a comment that ends in a pipe |
printf '%s' "$v" | grep -q x
  if printf '%s' "$v" | grep -q x; then
          elif [ -n "${BEFORE:-}" ] && ! printf '%s' "$BEFORE" | grep -qE '^0+$'; then
```

`ci/actionlint/fixtures/early-exit/silent.txt` (no line may fire; lines 20-21 and 22-23 are the D-2
YAML-header cases):

```
# SPDX-License-Identifier: Apache-2.0
# SMA-647 check 13 fixture, read by early_exit_reader_self_test: no line below fires.
grep -qF -- "$lit" < <(printf '%s' "$block")
out="$(producer)" || rc=$?
if grep -q x < <(printf '%s\n' "$out"); then
printf '%s\n' "$v" | sed -n 1p | cut -d: -f1
otool -l "$so" | awk '/X/&&!d{v=$2; d=1} END{print v}'
false || grep -q x file
cmd |& tee log
printf '%s\n' "$v" | xargs grep -q x
grep -E 'a|head' file
# printf '%s' "$v" | grep -q x
    # an indented comment | head -1
printf '%s' "$v" | grep -c x
printf '%s' "$v" | grep -o x | tr -d y
printf '%s' "$v" | headline
printf '%s' "$v" | awk '{print $1}'
cmd | sort | tail -n 1
grep '^if:' | sed -n '1s/^if:[[:space:]]*//p'
        run: |
          head -1 file
      - |
        grep -q x file
```

`ci/actionlint/fixtures/early-exit/allow.txt`:

```
# SPDX-License-Identifier: Apache-2.0
# SMA-647 check 13 fixture, read by early_exit_reader_self_test: line 3 is the one hit.
printf '%s' "$v" | grep -q x
```

- [ ] **Step 2: Write the failing test** — add the self-test and its call, bump the count. In
`run_self_tests`, old:

```bash
  doc_diagnosis_self_test

  assert_self_tests_ran "$SELF_TEST_COUNT"
```

New:

```bash
  doc_diagnosis_self_test
  early_exit_reader_self_test

  assert_self_tests_ran "$SELF_TEST_COUNT"
```

`SELF_TEST_COUNT`, old:

```bash
SELF_TEST_COUNT=14  # extractor, path-filter, branch-filter, config, ci-target-floor,
                    # invocation-allowlist, affected-graph-wiring, block-execution,
                    # kill-predicate, affected-smoke-block, release-guard, cargo-lock-step,
                    # release-plan, doc-diagnosis
```

New:

```bash
SELF_TEST_COUNT=15  # extractor, path-filter, branch-filter, config, ci-target-floor,
                    # invocation-allowlist, affected-graph-wiring, block-execution,
                    # kill-predicate, affected-smoke-block, release-guard, cargo-lock-step,
                    # release-plan, doc-diagnosis, early-exit-reader
```

Insert after the closing `}` of `doc_diagnosis_self_test` (old text to anchor on:
`  rm -rf "$tmpd"\n  return "$rc"\n}\n\n# ---------------------------------------------------------------------------------------------\n# Check 7 — the self-tests, and the counter that proves they were invoked.`), between that `}` and
the `# ----` line, the self-test only (the definitions come in Step 4):

```bash

early_exit_reader_self_test() {
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
  local rc=0 fx=ci/actionlint/fixtures/early-exit tmpd list got want n text
  local ps flag_a late_a flag_b late_b second_b i1_rc waited

  expect_early() {
    local name="$1" expected="$2"
    if [ "$got" != "$expected" ]; then
      fail "early-exit self-test '$name': got '$got', expected '$expected'. Check 13 is not
      deciding what it is documented to decide."
      rc=1
    fi
  }

  [ -d "$fx" ] || infra "check 13 self-test: the fixture directory $fx is missing"
  tmpd="$(mktemp -d)" || infra "check 13 self-test: mktemp -d failed"
  list="$tmpd/list"

  # Every reader form fires, once per LOGICAL line: two joined pairs report their first line, and
  # the comment that ends in a pipe (line 22) does not hide line 23 (SMA-647 plan D-3).
  printf '%s\n' "$fx/fires.txt" > "$list"
  got="$(early_exit_reader_verdict "$list")"
  want="$(for n in 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 20 23 24 25; do
    printf 'early-exit-reader %s:%s\n' "$fx/fires.txt" "$n"; done | LC_ALL=C sort)"
  expect_early 'every reader form fires, once per logical line' "$want"

  # Nothing else fires: `||`, `|&`, an xargs-driven grep, a reader word inside a pattern, comments,
  # the I1/I2/value-reader replacements, and the two YAML block-scalar headers (plan D-2).
  printf '%s\n' "$fx/silent.txt" > "$list"
  got="$(early_exit_reader_verdict "$list")"
  expect_early 'no non-reader, comment, I1, I2, value-reader or YAML-header line fires' ''

  # The allowlist's row shapes. The override is scoped INSIDE each command substitution, as check
  # 12's cases are, so no expansion of the real table runs in the main shell. The text comes from
  # the fixture file, never from this file: check 13 scans this file.
  text="$(sed -n 3p "$fx/allow.txt")"
  printf '%s\n' "$fx/allow.txt" > "$list"
  got="$(EARLY_EXIT_READER_ALLOWED=("$fx/allow.txt:3|a stated reason|$text")
         early_exit_reader_verdict "$list")"
  expect_early 'an allowlist row with a reason waives its line' ''

  got="$(EARLY_EXIT_READER_ALLOWED=("$fx/allow.txt:3||$text")
         early_exit_reader_verdict "$list")"
  expect_early 'an allowlist row with an empty reason fires' "blank-reason $fx/allow.txt:3"

  got="$(EARLY_EXIT_READER_ALLOWED=("$fx/allow.txt:3|a stated reason|some other text")
         early_exit_reader_verdict "$list")"
  expect_early 'a row whose text no longer matches is a note, and the line still fires' \
    "$(printf 'early-exit-reader %s:3\nstale-allowlist %s:3' "$fx/allow.txt" "$fx/allow.txt")"

  # THE bash 3.2 case: an EMPTY table under `set -u`. Without the ${ARR+...} idiom the expansion
  # is an unbound-variable error that kills the verdict's subshell, and zero rows come out.
  got="$(EARLY_EXIT_READER_ALLOWED=()
         early_exit_reader_verdict "$list")"
  expect_early 'an EMPTY allowlist still emits the row (the bash 3.2 unbound-array case)' \
    "early-exit-reader $fx/allow.txt:3"

  got="$(early_exit_reader_verdict "$tmpd/nope")"
  expect_early 'a missing list reports no-list' 'no-list'

  printf '%s\n' "$tmpd/absent.txt" > "$list"
  got="$(early_exit_reader_verdict "$list")"
  expect_early 'an unreadable corpus member reports unreadable' "unreadable $tmpd/absent.txt"

  # --- Behavioural proof (SMA-647 spec §5.3): a HANDSHAKE, not a sleep. ----------------------
  # The reader runs grep, closes its own stdin, then touches a flag. The producer writes one
  # line, waits for the flag (bounded, 10 s), and only then writes again — so the second write
  # happens after the reader closed the pipe, on every run, under any load. Check 9 runs this
  # table 16 times concurrently, the load that produced two of the three CI flakes.
  flag_a="$tmpd/flag-a"; late_a="$tmpd/late-a"
  flag_b="$tmpd/flag-b"; late_b="$tmpd/late-b"; second_b="$tmpd/second-b"
  early_exit_producer() { # $1 flag the reader touches, $2 timeout marker, $3 second-write marker
    local w=0
    printf 'hit\n'
    while [ ! -e "$1" ] && [ "$w" -lt 100 ]; do sleep 0.1; w=$((w + 1)); done
    [ -e "$1" ] || : > "$2"
    : > "$3"
    printf 'more\n'
  }

  # The OLD shape. The case must SEE the defect, or it proves nothing: the producer must fail
  # (141 on SIGPIPE, or 1 on EPIPE when SIGPIPE is ignored) while the reader succeeds.
  early_exit_producer "$flag_a" "$late_a" "$tmpd/second-a" 2>/dev/null \
    | ( grep -q hit; r=$?; exec <&-; : > "$flag_a"; exit "$r" )
  ps=("${PIPESTATUS[@]}")
  [ ! -e "$late_a" ] || infra "check 13 self-test: the handshake reader never touched its flag within 10 s, so the pipe case cannot decide anything"
  got="${ps[0]}/${ps[1]}"
  if [ "${ps[0]}" -eq 0 ] || [ "${ps[1]}" -ne 0 ]; then
    fail "early-exit self-test 'the pipe form loses its producer to the closed pipe': got
      PIPESTATUS '$got', expected a non-zero producer and a zero reader. The case cannot see the
      defect it exists to prove fixed."
    rc=1
  fi

  # The I1 shape, same producer, same reader: rc 0. The main shell does not wait for a process
  # substitution (bash 3.2 has no $! for one), so wait — bounded — for the producer to reach its
  # second write before the temp directory goes away (plan D-6).
  ( grep -q hit; r=$?; exec <&-; : > "$flag_b"; exit "$r" ) \
    < <(early_exit_producer "$flag_b" "$late_b" "$second_b" 2>/dev/null)
  i1_rc=$?
  waited=0
  while [ ! -e "$second_b" ] && [ "$waited" -lt 100 ]; do sleep 0.1; waited=$((waited + 1)); done
  [ -e "$second_b" ] || infra "check 13 self-test: the process-substitution producer never reached its second write within 10 s"
  [ ! -e "$late_b" ] || infra "check 13 self-test: the handshake reader never touched its flag within 10 s, so the I1 case cannot decide anything"
  got="$i1_rc"
  expect_early 'the I1 form with the same producer and reader exits 0' '0'

  rm -rf "$tmpd"
  return "$rc"
}
```

- [ ] **Step 3: Run it and see it fail:** `/bin/bash $S/sma647/selftest-actionlint.sh` → expected
`ACTIONLINT-SELFTEST REGRESSION`, with `early-exit self-test` rows whose `got` holds
`early_exit_reader_verdict: command not found`.

- [ ] **Step 4: Implement** — insert the definitions directly ABOVE `early_exit_reader_self_test() {`
(so they sit after `doc_diagnosis_self_test`'s `}`):

```bash

# ---------------------------------------------------------------------------------------------
# Check 13 (definitions) — no pipe into an early-exit reader (SMA-647).
#
# THE DEFECT. A reader that exits at its first match — grep in quiet or max-count mode, head, an
# awk program with exit — closes the pipe while the producer may still write. The producer's next
# write gets SIGPIPE and exits 141, and under `pipefail` that 141 becomes the pipeline status, so
# a match reads as a miss. MEASURED (SMA-647): bash sends a multi-line value in many small writes;
# on Linux the race is rare and needs load, on macOS with BSD grep it fired on every run. Check
# 12's literal probe red three times in CI this way. The replacements: process substitution (the
# producer leaves the pipeline), capture-then-match (when the producer's status matters), and
# `sed -n 1p` in place of a first-line reader. A here-string is not one of them: Homebrew bash
# 5.3.15 deadlocks on one over about 512 bytes.
#
# THE RULE, per LOGICAL line of each corpus file:
#   1. A full-line comment is skipped, and it never starts a join: a comment that ends in a pipe
#      character would otherwise swallow the code line after it.
#   2. A line that ends in a pipe, optionally followed by a backslash, is joined with the next
#      line, because bash accepts a pipe at the end of a line. A YAML block-scalar header
#      (`run: |`, `script: |`, `- |`) is NOT joined: all 117 end-of-line pipes in the corpus were
#      such headers when this was written, and joining one makes the first line of its block look
#      like a pipe consumer.
#   3. The logical line fires if EARLY_EXIT_ERE matches: a pipe that is not part of `||` or `|&`,
#      optional VAR=value words and an optional `command`, then grep with a q or m in a short-flag
#      cluster (or --quiet/--silent/--max-count) before the next pipe, head as a whole word, or
#      awk with the word exit before the next pipe. Only the NEXT command word counts, so an
#      xargs-driven grep and a reader word inside a grep pattern do not fire.
# POSIX classes only, no `\s`/`\b`: this file runs on BSD tools locally and GNU tools in CI (see
# the BSD/GNU note at cargo_lock_step_verdict). The ERE writes every literal pipe as a bracket
# expression and never puts a reader word right after an alternation bar, so its own definition
# line does not match itself.
#
# FIXTURES LIVE OUTSIDE THE CORPUS, in ci/actionlint/fixtures/early-exit/*.txt: this check scans
# THIS file, so a fixture line written here would fire. For the same reason no message in this
# file quotes a banned form literally.
#
# `<path>:<line>|<reason>|<logical line text>` strings, keyed the COE_SKIP way: a row waives a hit
# only while BOTH its location and its text agree, so a shifted row stops matching instead of
# waiving another line. The text is LAST because it always holds a pipe; the location cannot (no
# tracked path contains one) and the reason must not. Every expansion uses the ${ARR+"${ARR[@]}"}
# idiom (see BRANCH_SKIP): bash 3.2 under `set -u` cannot expand an empty array, the error kills
# the verdict's process substitution, and a real violation passes in silence. Ships EMPTY
# (SMA-647 D3): about fifteen "safe today" rows would each be a claim about buffering that a later
# edit can falsify.
# ---------------------------------------------------------------------------------------------
EARLY_EXIT_READER_ALLOWED=(
  # (empty — add entries as "<path>:<line>|<reason, no pipe character>|<logical line text>")
)

EARLY_EXIT_ERE='(^|[^|])[|][[:space:]]*([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+)*(command[[:space:]]+)?(grep[[:space:]]([^|]*[[:space:]])?(-[[:alpha:]]*[qm]|--(quiet|silent|max-count))|head([[:space:];)]|$)|awk[[:space:]]([^|]*[^[:alnum:]_])?exit([^[:alnum:]_]|$))'

# Prints one `<path>:<first line>\t<logical text>` record per logical line of $1, with rules 1 and 2
# above applied. Plain awk regexes only (bracket expressions with a literal space and tab), which
# BSD awk, mawk and gawk all read the same way.
early_exit_join() { # $1 file
  awk -v F="$1" '
    function flush() { if (acc != "") print F ":" start "\t" acc; acc = "" }
    {
      line = $0
      if (acc == "") {
        if (line ~ /^[ \t]*#/) next
        start = NR
        acc = line
      } else {
        acc = acc " " line
      }
      if (acc ~ /:[ \t]*[|][ \t]*$/ || acc ~ /^[ \t]*-[ \t]+[|][ \t]*$/) { flush(); next }
      if (acc ~ /[|][ \t]*\\?[ \t]*$/ && acc !~ /[|][|][ \t]*\\?[ \t]*$/) {
        sub(/\\[ \t]*$/, "", acc)
        next
      }
      flush()
    }
    END { flush() }
  ' "$1"
}

# Emits one row per violation (unsorted), nothing when clean. Takes a FILE listing corpus paths,
# one per line, rather than running `git ls-files` itself — the split that lets the self-test drive
# it against fixture files (the doc_diagnosis_verdict precedent).
early_exit_reader_rows() { # $1 = a file listing corpus paths, one per line
  local list="$1" f tmpd rc row loc text entry e_loc e_rest e_reason e_text waived i used tab
  [ -f "$list" ] && [ -r "$list" ] || { echo "no-list"; return; }
  tmpd="$(mktemp -d)" || { echo "no-tmp"; return; }
  tab="$(printf '\t')"
  : > "$tmpd/joined"
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    if [ ! -f "$f" ] || [ ! -r "$f" ]; then echo "unreadable $f"; continue; fi
    early_exit_join "$f" >> "$tmpd/joined" || echo "join-failed $f"
  done < "$list"
  # grep's own status is ROUTED: 1 is "no line matched", anything above 1 is a broken scan, and a
  # broken scan that emitted nothing would otherwise read as a clean corpus.
  rc=0
  grep -E -- "$EARLY_EXIT_ERE" "$tmpd/joined" > "$tmpd/hits" || rc=$?
  if [ "$rc" -gt 1 ]; then echo "grep-failed $rc"; rm -rf "$tmpd"; return; fi
  used=' '
  while IFS= read -r row; do
    loc="${row%%"$tab"*}"
    text="${row#*"$tab"}"
    waived=0
    i=0
    for entry in ${EARLY_EXIT_READER_ALLOWED+"${EARLY_EXIT_READER_ALLOWED[@]}"}; do
      i=$((i + 1))
      e_loc="${entry%%|*}"
      e_rest="${entry#*|}"
      e_reason="${e_rest%%|*}"
      e_text="${e_rest#*|}"
      # Fewer than two separators: no reason and no text, so the row can never waive a line.
      case "$entry" in *'|'*'|'*) ;; *) e_reason=''; e_text='' ;; esac
      if [ "$e_loc" = "$loc" ] && [ "$e_text" = "$text" ]; then
        waived=1
        used="$used$i "
        [ -n "$e_reason" ] || echo "blank-reason $loc"
        break
      fi
    done
    [ "$waived" -eq 1 ] || echo "early-exit-reader $loc"
  done < "$tmpd/hits"
  # A row that waived nothing is stale: a NOTE, not a red (check 12's stale-allowlist precedent).
  i=0
  for entry in ${EARLY_EXIT_READER_ALLOWED+"${EARLY_EXIT_READER_ALLOWED[@]}"}; do
    i=$((i + 1))
    case "$used" in *" $i "*) ;; *) echo "stale-allowlist ${entry%%|*}" ;; esac
  done
  rm -rf "$tmpd"
}

early_exit_reader_verdict() { # $1 = a file listing corpus paths, one per line
  early_exit_reader_rows "$1" | LC_ALL=C sort
}
```

Then the counts and docs of this task:

`run.sh` usage, old:

```bash
  echo "  --self-test    run the fourteen fixture tables only — extractor, path-filter verdicts," >&2
  echo "                 branch-filter verdicts, config allowlist, ci-target floor, invocation" >&2
  echo "                 allowlist, affected-graph wiring, block execution, kill predicate," >&2
  echo "                 affected-smoke block, release guard, cargo-lock step, release-plan," >&2
  echo "                 doc-diagnosis." >&2
```

New:

```bash
  echo "  --self-test    run the fifteen fixture tables only — extractor, path-filter verdicts," >&2
  echo "                 branch-filter verdicts, config allowlist, ci-target floor, invocation" >&2
  echo "                 allowlist, affected-graph wiring, block execution, kill predicate," >&2
  echo "                 affected-smoke block, release guard, cargo-lock step, release-plan," >&2
  echo "                 doc-diagnosis, early-exit reader. The early-exit-reader table reads its" >&2
  echo "                 fixtures from ci/actionlint/fixtures/early-exit/." >&2
```

`run.sh:4927`, old: `# All FOURTEEN are defined above so this block can run them from ONE call site, reached by both the`
New: `# All FIFTEEN are defined above so this block can run them from ONE call site, reached by both the`

`README.md` (Edits by exact substring):
- `| 7 | Fourteen self-tests against fixture tables` → `| 7 | Fifteen self-tests against fixture tables`
- `release-plan, doc-diagnosis — plus a counter` → `release-plan, doc-diagnosis, early-exit reader — plus a counter`
- `asserting all fourteen ran, and a definition-count check catching a fifteenth table` →
  `asserting all fifteen ran, and a definition-count check catching a sixteenth table`
- `each of the fourteen self-test invocations inside` → `each of the fifteen self-test invocations inside`
- Old:
  ```
  these) roughly fifteen times per full gate run — fourteen mutants plus the unmutated control — so
  the new row's `uv run` is paid roughly 15x per `moon run repo:actionlint`, not once. This does not
  change check 9's own fixture-table or mutant count (still fourteen and fourteen, below); it only
  ```
  New:
  ```
  these) roughly sixteen times per full gate run — fifteen mutants plus the unmutated control, since
  SMA-647 — so the new row's `uv run` is paid roughly 16x per `moon run repo:actionlint`, not once.
  This does not change check 9's own fixture-table or mutant count (fifteen and fifteen, below); it only
  ```
- Old: `State: CURRENT — fourteen fixture tables, fourteen mutants (fifteen concurrent `--self-test``
  `subprocesses in check 9's battery: fourteen mutants plus the unmutated control). Nested one level`
  → New: `State: CURRENT — fifteen fixture tables, fifteen mutants (sixteen concurrent `--self-test``
  `subprocesses in check 9's battery: fifteen mutants plus the unmutated control). Nested one level`
- `so the battery's fifteen concurrent` → `so the battery's sixteen concurrent`
- Insert this paragraph directly before the `State: CURRENT` line:
  ```
  **SMA-647 added a FIFTEENTH self-test.** `early_exit_reader_self_test` (check 13) is a
  fixture-table check at the same `grep`/`sed`/`awk` level as check 12. It reads its fixtures from
  `ci/actionlint/fixtures/early-exit/*.txt` and owns one behavioural case: a handshake between a
  real producer and a real reader, which waits for flag files in steps of 0.1 s. No timing was
  measured for this paragraph; only the row and mutant counts below are asserted.

  ```
- `ci/actionlint/run.sh --self-test   # the fourteen fixture tables only, for fast iteration` →
  `ci/actionlint/run.sh --self-test   # the fifteen fixture tables only, for fast iteration`
- `` `--self-test` runs the fourteen fixture tables and nothing else `` →
  `` `--self-test` runs the fifteen fixture tables and nothing else ``

`moon.yml` (actionlint task comments), old:

```
    # fixtures — and FOURTEEN fixture tables (extractor,
```
New:
```
    # fixtures — and FIFTEEN fixture tables (extractor,
```
Old:
```
    # release guard, cargo-lock step, release plan, doc-diagnosis). The list said eleven until
```
New:
```
    # release guard, cargo-lock step, release plan, doc-diagnosis, early-exit reader). The list said eleven until
```
Old:
```
    # the thirteenth (`release_plan_self_test`), and SMA-597 the fourteenth
    # (`doc_diagnosis_self_test`, check 12).
```
New:
```
    # the thirteenth (`release_plan_self_test`), SMA-597 the fourteenth
    # (`doc_diagnosis_self_test`, check 12), and SMA-647 the fifteenth
    # (`early_exit_reader_self_test`, check 13).
```
Old:
```
    # once per self-test invocation plus once unmutated, FIFTEEN concurrent subprocesses
    # (fourteen mutants — check 9 builds one per `$SELF_TEST_COUNT` invocation — plus the
    # unmutated control; the figure read twelve until SMA-603 corrected it for SMA-601's and
    # SMA-603's own tables, and thirteen until SMA-597 corrected it again for check 12). That
```
New:
```
    # once per self-test invocation plus once unmutated, SIXTEEN concurrent subprocesses
    # (fifteen mutants — check 9 builds one per `$SELF_TEST_COUNT` invocation — plus the
    # unmutated control; the figure read twelve until SMA-603 corrected it for SMA-601's and
    # SMA-603's own tables, thirteen until SMA-597 corrected it again for check 12, and fourteen
    # until SMA-647 added check 13). That
```

`CLAUDE.md`, old:

```
  that and the pin stays green on exactly the PR that breaks it. Adding a fifteenth-and-later
  `*_self_test` table means bumping `SELF_TEST_COUNT` (currently 14 — SMA-579 added the eleventh,
  `release_guard_self_test` at check 10, SMA-601 the twelfth, `cargo_lock_step_self_test` at
  check 8f, SMA-603 the thirteenth, `release_plan_self_test` at check 11, and SMA-597 the
  fourteenth, `doc_diagnosis_self_test` at check 12): the gate asserts
```

New:

```
  that and the pin stays green on exactly the PR that breaks it. Adding a sixteenth-and-later
  `*_self_test` table means bumping `SELF_TEST_COUNT` (currently 15 — SMA-579 added the eleventh,
  `release_guard_self_test` at check 10, SMA-601 the twelfth, `cargo_lock_step_self_test` at
  check 8f, SMA-603 the thirteenth, `release_plan_self_test` at check 11, SMA-597 the
  fourteenth, `doc_diagnosis_self_test` at check 12, and SMA-647 the fifteenth,
  `early_exit_reader_self_test` at check 13): the gate asserts
```

- [ ] **Step 5: Run it and see it pass**

```
/bin/bash $S/sma647/selftest-actionlint.sh
/bin/bash $S/sma647/bounded.sh 90 $S/sma647/selftest-53.log /opt/homebrew/bin/bash ci/actionlint/run.sh --self-test
/bin/bash $S/sma647/scan.sh
grep -c -E '^(function[[:blank:]]+)?[a-z_]+_self_test([[:blank:]]*\(\))?[[:blank:]]*\{' ci/actionlint/run.sh
```

Expected: `ACTIONLINT-SELFTEST OK` (the counter now expects 15, and the definition count is 15);
the Homebrew run is recorded as `TIMEOUT` (M-2) and is NOT a finding; the scan prints `rows: 0 (all
files)` — the new definitions and self-test do not match their own rule; the grep count is `15`.

Prove the self-test bites (restore each by Edit):
1. Delete the line `      if (acc ~ /:[ \t]*[|][ \t]*$/ || acc ~ /^[ \t]*-[ \t]+[|][ \t]*$/) { flush(); next }`
   → `selftest-actionlint.sh` reports `'no non-reader, comment, I1, I2, value-reader or YAML-header
   line fires': got 'early-exit-reader …/silent.txt:20` and `…:22'` (measured in the prototype).
2. Change `        if (line ~ /^[ \t]*#/) next` to `        if (0) next` → the `fires.txt` case
   reports a changed row set (measured in the prototype).
3. Delete `exec <&-; ` from the pipe-form reader → the handshake case fails with PIPESTATUS `0/0`
   (the reader still holds the pipe open while the producer writes again).
4. Change `for entry in ${EARLY_EXIT_READER_ALLOWED+"${EARLY_EXIT_READER_ALLOWED[@]}"}; do` (first
   occurrence) to `for entry in "${EARLY_EXIT_READER_ALLOWED[@]}"; do` → under `/bin/bash` 3.2 the
   EMPTY-allowlist case reports `got ''` (the bash 3.2 unbound-array hole).

- [ ] **Step 6: Commit**

```
git add ci/actionlint/fixtures/early-exit/fires.txt ci/actionlint/fixtures/early-exit/silent.txt ci/actionlint/fixtures/early-exit/allow.txt ci/actionlint/run.sh ci/actionlint/README.md moon.yml CLAUDE.md
git commit -m "feat(ci): add check 13's early-exit-reader verdict and its self-test (SMA-647)" -m "The verdict, its fixtures under ci/actionlint/fixtures/early-exit/, and early_exit_reader_self_test with a handshake case that proves the pipe form loses its producer and process substitution does not. SELF_TEST_COUNT 14 -> 15. Check 13 does not read the real corpus yet." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

(The type is `feat(ci)` because this adds a check; `fix(ci)` is also acceptable. Keep one style.)

---

## Task 11: Turn check 13 on over the real corpus, pin it, document it

**Files:**
- Modify: `ci/actionlint/run.sh` (production block, between check 12's `done < <(claude_md_block_verdict CLAUDE.md)` and `selftest_mutation_battery`)
- Modify: `ci/affected-graph/ci_targets.py` (`ACTIONLINT_SH_CALL_SITES` after `:945`,
  `wired_actionlint` after `:2502`, battery cases before the "Contamination cases" comment at ~`:2790`)
- Modify: `ci/actionlint/README.md` (check table row 13 after row 12; Limitations L33-L40 after L32)
- Modify: `CLAUDE.md` (new Gotchas entry after the check-12 bullet at `:1098-1102`; one sentence in
  the LOCAL ONLY bullet at `:1112`)
- Modify: `.github/workflows/release.yml:912-919` (comment), `.github/workflows/prebuild.yml:265-267` (comment)

**Interfaces:**
- Consumes: `early_exit_reader_verdict`, `EARLY_EXIT_READER_ALLOWED` (Task 10).
- Produces: `EE_LIST`, `EE_RC` (top-level), seven new column-0 lines pinned by
  `ACTIONLINT_SH_CALL_SITES`: the `git ls-files` line, five corpus-floor lines, and
  `done < <(early_exit_reader_verdict "$EE_LIST")`.

- [ ] **Step 1: Write the failing test** — the pins first. In `ci_targets.py`, old (end of
`ACTIONLINT_SH_CALL_SITES`):

```python
    "done < <(claude_md_block_verdict CLAUDE.md)",
)
```

New:

```python
    "done < <(claude_md_block_verdict CLAUDE.md)",
    # SMA-647 — check 13's corpus listing, its five corpus-floor lines and its production call
    # site, at run.sh's top level, column 0 like every other entry above. Same reasons as check
    # 12's four: `early_exit_reader_verdict` is also called from its own self-test, so only this
    # exact production line proves the REAL corpus is scanned, and an empty or partial corpus
    # passes having asserted nothing unless its listing and floor are pinned too.
    "git ls-files -- ':(glob)**/*.sh' ':(glob).github/workflows/*.yml' ':(glob).github/workflows/*.yaml' 'moon.yml' ':(glob)**/moon.yml' ':(glob).moon/**/*.yml' 'lefthook.yml' > \"$EE_LIST\"",
    'grep -qxF \'ci/actionlint/run.sh\' "$EE_LIST" || infra "check 13: corpus floor: ci/actionlint/run.sh is not listed, so a pathspec stopped matching"',
    'grep -qxF \'.github/workflows/ci.yml\' "$EE_LIST" || infra "check 13: corpus floor: .github/workflows/ci.yml is not listed, so a pathspec stopped matching"',
    'grep -qxF \'moon.yml\' "$EE_LIST" || infra "check 13: corpus floor: moon.yml is not listed, so a pathspec stopped matching"',
    'grep -qxF \'.moon/tasks.yml\' "$EE_LIST" || infra "check 13: corpus floor: .moon/tasks.yml is not listed, so a pathspec stopped matching"',
    'grep -qxF \'ops/nats/check-subjects.sh\' "$EE_LIST" || infra "check 13: corpus floor: ops/nats/check-subjects.sh is not listed, so a pathspec stopped matching"',
    'done < <(early_exit_reader_verdict "$EE_LIST")',
)
```

`wired_actionlint`, old (its last lines):

```python
        # ...and Assertion B's own production call site (fix-wave Finding 1, review of SMA-597).
        'done < <(claude_md_block_verdict CLAUDE.md)\n'
    )
```

New:

```python
        # ...and Assertion B's own production call site (fix-wave Finding 1, review of SMA-597).
        'done < <(claude_md_block_verdict CLAUDE.md)\n'
        # SMA-647 — check 13's corpus listing, its five corpus-floor lines and its production call
        # site, at run.sh's top level, outside any function — column 0 like every entry above.
        "git ls-files -- ':(glob)**/*.sh' ':(glob).github/workflows/*.yml' ':(glob).github/workflows/*.yaml' 'moon.yml' ':(glob)**/moon.yml' ':(glob).moon/**/*.yml' 'lefthook.yml' > \"$EE_LIST\"\n"
        'grep -qxF \'ci/actionlint/run.sh\' "$EE_LIST" || infra "check 13: corpus floor: ci/actionlint/run.sh is not listed, so a pathspec stopped matching"\n'
        'grep -qxF \'.github/workflows/ci.yml\' "$EE_LIST" || infra "check 13: corpus floor: .github/workflows/ci.yml is not listed, so a pathspec stopped matching"\n'
        'grep -qxF \'moon.yml\' "$EE_LIST" || infra "check 13: corpus floor: moon.yml is not listed, so a pathspec stopped matching"\n'
        'grep -qxF \'.moon/tasks.yml\' "$EE_LIST" || infra "check 13: corpus floor: .moon/tasks.yml is not listed, so a pathspec stopped matching"\n'
        'grep -qxF \'ops/nats/check-subjects.sh\' "$EE_LIST" || infra "check 13: corpus floor: ops/nats/check-subjects.sh is not listed, so a pathspec stopped matching"\n'
        'done < <(early_exit_reader_verdict "$EE_LIST")\n'
    )
```

Battery cases — insert directly before the line
`    # Contamination cases, THREE of them (SMA-542 review finding I1, plus a round-2 addition). The`:

```python
    # SMA-647 — check 13's seven column-0 lines, each deleted in turn (the deletion case), and its
    # production call INDENTED (the indentation case), mirroring the check-8d pair above. Derived
    # from the registry by the one token the seven share, with the count asserted, so an entry
    # dropped from ACTIONLINT_SH_CALL_SITES reds here as well.
    _ee_sites = [site for site in ACTIONLINT_SH_CALL_SITES if '"$EE_LIST"' in site]
    if len(_ee_sites) != 7:
        failures.append(
            f"check_self_invocation: expected 7 check-13 entries in ACTIONLINT_SH_CALL_SITES, "
            f"found {len(_ee_sites)}"
        )
    for _ee_site in _ee_sites:
        _ee_broken = wired_actionlint.replace(f"{_ee_site}\n", "")
        if _ee_broken == wired_actionlint:
            failures.append(
                f"check_self_invocation: wired_actionlint lacks the check-13 line {_ee_site!r}"
            )
        elif not check_self_invocation(wired, scripts, _ee_broken, wired_release_parity, wired_workflow_credentials, wired_release_plan, wired_ruff, wired_next_public_free):
            failures.append(
                f"check_self_invocation: missed a deleted check-13 line {_ee_site!r}"
            )
    indented_check13_call = wired_actionlint.replace(
        'done < <(early_exit_reader_verdict "$EE_LIST")\n',
        '  done < <(early_exit_reader_verdict "$EE_LIST")\n',
    )
    if not check_self_invocation(wired, scripts, indented_check13_call, wired_release_parity, wired_workflow_credentials, wired_release_plan, wired_ruff, wired_next_public_free):
        failures.append(
            "check_self_invocation: an INDENTED check-13 call site satisfied the column-0 pin"
        )
```

- [ ] **Step 2: Run it and see it fail** (from `$WT`)

```
python3 ci/affected-graph/ci_targets.py --self-test
python3 ci/affected-graph/ci_targets.py
```

Expected: `ci-targets self-test OK` (fixture and registry agree); the real run FAILS and names the
seven check-13 lines as missing from `ci/actionlint/run.sh`.

- [ ] **Step 3: Implement the production block.** Old text (end of `run.sh`):

```bash
done < <(claude_md_block_verdict CLAUDE.md)

selftest_mutation_battery
```

New text:

```bash
done < <(claude_md_block_verdict CLAUDE.md)

# ---------------------------------------------------------------------------------------------
# Check 13 — no pipe into an early-exit reader, over the real tracked corpus (SMA-647). Runs here,
# not in --self-test, because it reads the real tracked tree, like checks 5/6/10/11/12.
#
# CORPUS: `git ls-files` with `:(glob)` magic on every pattern that holds `**`. Without it git
# still needs a literal `/` for `**`, so `.moon/**/*.yml` would miss `.moon/tasks.yml` (the trap
# CLAUDE.md records for repo:ruff-ci). The root `moon.yml` is listed on its own for the same
# reason. It reads the INDEX, as check 12 does: a file not yet `git add`ed is invisible.
#
# THE CORPUS FLOOR IS PART OF THE CHECK. An empty or partial corpus emits zero rows and passes
# having asserted nothing (check 12's `∅ ⊆ allowlist` shape). Five named members must be listed.
# `infra`, not `fail`: a corpus that vanished is a broken gate, not a clean repo.
#
# COLUMN 0 for the listing, the five floor lines and the read loop: ACTIONLINT_SH_CALL_SITES
# matches with no leading whitespace, so indenting any of them reds that pin.
# ---------------------------------------------------------------------------------------------
EE_LIST="$(mktemp)" || infra "check 13: mktemp failed"
git ls-files -- ':(glob)**/*.sh' ':(glob).github/workflows/*.yml' ':(glob).github/workflows/*.yaml' 'moon.yml' ':(glob)**/moon.yml' ':(glob).moon/**/*.yml' 'lefthook.yml' > "$EE_LIST"
EE_RC=$?
[ "$EE_RC" -eq 0 ] || infra "check 13: git ls-files exited $EE_RC, so check 13 cannot know its corpus."
grep -qxF 'ci/actionlint/run.sh' "$EE_LIST" || infra "check 13: corpus floor: ci/actionlint/run.sh is not listed, so a pathspec stopped matching"
grep -qxF '.github/workflows/ci.yml' "$EE_LIST" || infra "check 13: corpus floor: .github/workflows/ci.yml is not listed, so a pathspec stopped matching"
grep -qxF 'moon.yml' "$EE_LIST" || infra "check 13: corpus floor: moon.yml is not listed, so a pathspec stopped matching"
grep -qxF '.moon/tasks.yml' "$EE_LIST" || infra "check 13: corpus floor: .moon/tasks.yml is not listed, so a pathspec stopped matching"
grep -qxF 'ops/nats/check-subjects.sh' "$EE_LIST" || infra "check 13: corpus floor: ops/nats/check-subjects.sh is not listed, so a pathspec stopped matching"

while IFS= read -r verdict; do
  case "$verdict" in
    '') ;;
    no-list|no-tmp)
      infra "check 13: could not build the corpus list or a scratch directory ($verdict)." ;;
    unreadable\ *|join-failed\ *|grep-failed\ *)
      infra "check 13: $verdict — the scan did not read the whole corpus, so a clean result would
      prove nothing." ;;
    early-exit-reader\ *)
      fail "check 13: ${verdict#early-exit-reader } pipes a producer into a reader that can exit
      before the producer ends (grep in quiet or max-count mode, head, or awk with exit). Under
      pipefail the producer's SIGPIPE turns a match into a false failure (SMA-647). Rewrite it:
      'reader < <(producer)' when the producer's status does not matter; capture the producer,
      check its status, then match the variable, when it does; 'sed -n 1p' in place of a
      first-line reader. Not a here-string: see CLAUDE.md." ;;
    blank-reason\ *)
      fail "check 13: the EARLY_EXIT_READER_ALLOWED entry for ${verdict#blank-reason } has an
      empty reason. An unexplained waiver is not a waiver." ;;
    stale-allowlist\ *)
      echo "actionlint gate: check 13 NOTE: EARLY_EXIT_READER_ALLOWED names ${verdict#stale-allowlist }, which no longer matches that line's text — drop the row." >&2 ;;
    *)
      infra "check 13: unrecognised verdict '$verdict'" ;;
  esac
done < <(early_exit_reader_verdict "$EE_LIST")

rm -f "$EE_LIST"

selftest_mutation_battery
```

- [ ] **Step 4: Run it and see it pass.** Create `$S/sma647/check13-harness.sh` — it runs ONLY check
13 (its definitions and its production block, extracted from the real file) over the real corpus:

```bash
#!/bin/bash
# Runs ONLY check 13 from the real ci/actionlint/run.sh over the real corpus, under this bash.
# Exit: 0 clean, 1 a row fired, 2 infra. The full gate has no working local bash (CLAUDE.md).
set -uo pipefail
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-647 || exit 2
FAILED=0
fail() { echo "actionlint gate: $*" >&2; FAILED=1; }
infra() { echo "actionlint gate: INFRASTRUCTURE ERROR: $*" >&2; exit 2; }
defs="$(awk '/^# Check 13 \(definitions\)/{f=1} /^early_exit_reader_self_test\(\) \{$/{f=0} f' ci/actionlint/run.sh)"
prod="$(awk '/^# Check 13 [^(]/{f=1} /^selftest_mutation_battery$/{f=0} f' ci/actionlint/run.sh)"
[ -n "$defs" ] && [ -n "$prod" ] || { echo "harness: could not extract check 13" >&2; exit 2; }
eval "$defs"
eval "$prod"
echo "check 13 harness: FAILED=$FAILED"
exit "$FAILED"
```

Then (from `$WT`):

```
/bin/bash $S/sma647/check13-harness.sh
/opt/homebrew/bin/bash $S/sma647/check13-harness.sh
python3 ci/affected-graph/ci_targets.py --self-test
python3 ci/affected-graph/ci_targets.py
/bin/bash $S/sma647/selftest-actionlint.sh
```

Expected: `check 13 harness: FAILED=0` under both bashes (no here-string runs in check 13, so the
Homebrew deadlock does not apply; if it hangs, record it); `ci-targets self-test OK`;
`PASS  ci-targets …`; `ACTIONLINT-SELFTEST OK`.

Prove the pins bite (restore each by Edit): indent `done < <(early_exit_reader_verdict "$EE_LIST")`
by two spaces → `python3 ci/affected-graph/ci_targets.py` fails naming that line; delete the
`grep -qxF '.moon/tasks.yml' …` line → it fails naming that line.

- [ ] **Step 5: Docs.** README check table — insert after the row that starts `| 12 |`:

```
| 13 | (SMA-647) No tracked shell surface pipes a producer into a reader that can exit before the producer ends: `grep` with a `q` or `m` in a short-flag cluster (or `--quiet`/`--silent`/`--max-count`), `head`, or `awk` with `exit`. Under `pipefail` the producer's SIGPIPE (141) becomes the pipeline status, so a match reads as a miss; this caused three false check-12 reds in CI, and on macOS with BSD grep the old check-12 probe missed on every run. The corpus is `git ls-files` over `**/*.sh`, the workflows, every `moon.yml`, `.moon/**/*.yml` and `lefthook.yml`, with a floor of five named members (`infra` when one is missing). A line that ends in a pipe is joined with the next line, except a YAML block-scalar header; a full-line comment is skipped and never starts a join; only the next command word after the pipe counts. Verdicts: `early-exit-reader <path>:<line>` (a violation), `blank-reason` (a violation), `stale-allowlist` (a note), and `no-list`/`no-tmp`/`unreadable`/`join-failed`/`grep-failed` (infra). `EARLY_EXIT_READER_ALLOWED` is keyed by location AND the logical line's text, and ships empty. `early_exit_reader_self_test` drives the verdict from `ci/actionlint/fixtures/early-exit/*.txt` (outside the corpus, because the check scans `run.sh` itself) and owns one behavioural case: a handshake proves the pipe form loses its producer and process substitution does not. Check 12 gained a second opinion at the same time: a `grep` miss that a bash substring match contradicts is a `literal-disagreement` row |
```

README Limitations — insert after the L32 paragraph (before `## Cost`):

```
**L33 (SMA-647).** Check 13 knows four readers only: `grep` with a `q`/`m` flag or the matching long
flags, `head`, and `awk` with `exit`. Other readers that stop early (`sed` with `q`, `read`,
`perl … last`) are not seen.

**L34 (SMA-647).** Check 13 reads the NEXT command word after the pipe only. A reader behind a word
outside its vocabulary (`timeout 5 grep -q`, `env grep -q`) is not seen, and a `|&` pipe is not
matched at all, into any reader.

**L35 (SMA-647).** Check 13 scans `*.sh`, the workflows, `moon.yml` files, `.moon/**/*.yml` and
`lefthook.yml`. A `subprocess` pipe or a `bash -c` string in a `.py` file, a script in `ts/` or `py/`
that is not `*.sh`, and shell quoted in a Markdown document are not scanned.

**L36 (SMA-647).** Check 13 is textual. A banned form inside a quoted string or a heredoc body in a
corpus file fires. That is why its fixtures are `.txt` files and why no message in `run.sh` quotes a
banned form.

**L37 (SMA-647).** A comment line between a pipe and its reader (`foo |`, then `# note`, then
`grep -q x`) is joined with the comment, and the reader line is not seen. A SHELL line that ends in
`key: |` is not joined either, because it has the shape of a YAML block-scalar header. Neither
shape exists in the corpus.

**L38 (SMA-647).** An `EARLY_EXIT_READER_ALLOWED` row keys on the joined LOGICAL text, which for a
joined pair spans two physical lines.

**L39 (SMA-647).** `repo:actionlint` still has no working local bash for the whole gate. MEASURED
2026-09-18: `/bin/bash` 3.2.57 finishes `--self-test` in about 7 s (rc 1, with only the two known
false `cargo-lock-step` rows); Homebrew bash 5.3.15 deadlocks on it. Check 13's production half runs
locally only through an extraction harness, and otherwise in CI.

**L40 (SMA-647).** Only check 12's literal probe has a second opinion (`literal-disagreement`). The
other rewritten sites rely on the rewrite alone.
```

CLAUDE.md — insert a new Gotchas bullet directly after the bullet that ends
`mentions `ciReport.json`, reds the gate until it carries the marker or is added to the allowlist.`:

```
- **A pipe into a reader that can exit early is a false red under `pipefail`** (MEASURED, SMA-647).
  `grep -q`, `grep -m N`, `head` and `awk … exit` stop reading at their first match. A producer
  that writes again after that gets SIGPIPE and exits 141. Under `pipefail` the pipeline status is
  then 141, although the reader found the match. On Linux in CI the race is rare and needs CPU load,
  so a re-run passes: it caused three false check-12 reds in `repo:actionlint` (PRs 223, 255, 258).
  On macOS with BSD grep 2.6.0 it is near-certain: the old check-12 probe missed on 500 of 500 runs
  against the real block. Use one of three forms instead. When the producer's status does not
  matter, use process substitution: `grep -qF -- "$lit" < <(printf '%s' "$block")`. When the
  status matters, capture the producer into a variable, check its status, then match the variable
  the same way; declare a `local` on its own line, and under `set -e` write the capture as the left
  side of `||`. In place of `head -1` or `grep -m1 … | sed`, use `sed -n 1p`; in place of
  `awk '…{print $2; exit}'`, take the first match in awk's `END` block. Do not use a here-string:
  Homebrew bash 5.3.15 deadlocks on one over about 512 bytes (see the LOCAL ONLY entry). Do not use
  `>/dev/null` in place of `-q`, and do not use a `sed` script with `q`. `ci/actionlint/run.sh`
  check 13 bans the pattern in every tracked `*.sh`, workflow, `moon.yml`, `.moon/**/*.yml` and
  `lefthook.yml`. Its allowlist, `EARLY_EXIT_READER_ALLOWED`, ships empty. Its fixtures live in
  `ci/actionlint/fixtures/early-exit/*.txt`, because check 13 also scans `run.sh`. Check 12 keeps
  a second opinion on each missing literal: a `literal-disagreement` row means `grep` and a bash
  `case` match disagree, which is evidence of a second mechanism. Keep that run's whole output for
  SMA-647 before you re-run.
```

CLAUDE.md LOCAL ONLY bullet, old:

```
  runs neither of these bash builds. The affected-graph suite (`ci/affected-graph/run.sh`) is the
```

New:

```
  runs neither of these bash builds. MEASURED (SMA-647, 2026-09-18): `--self-test` ALONE is
  different. Under `/bin/bash` 3.2.57 it finishes in about 7 s with rc 1, and its only failures are
  the same two false `cargo-lock-step` rows, so it is a usable local check when those two rows are
  its only failures. Under Homebrew bash 5.3.15 it deadlocks too (0.20 s of CPU in 420 s). On the
  same day `/opt/homebrew/bin/bash ci/publish-metadata/run.sh --negative-control` also hung (0.01 s
  of CPU in 300 s, after its categories self-test), although SMA-645 recorded a pass for it.
  The affected-graph suite (`ci/affected-graph/run.sh`) is the
```

`release.yml`, old:

```
      # is the only thing that can fail this step. A version that is empty or not purely dotted
```

New:

```
      # is the only thing that can fail this step. `ci/actionlint/run.sh` check 13 (SMA-647) now
      # bans a pipe into an early-exit reader in every tracked shell surface, not only here.
      # A version that is empty or not purely dotted
```

(Keep the rest of that comment paragraph unchanged; reflow nothing else.)

`prebuild.yml`, old:

```
      # under `pipefail` can red at status 141 on SIGPIPE with no `::error::` line at all.
```

New:

```
      # under `pipefail` can red at status 141 on SIGPIPE with no `::error::` line at all.
      # `ci/actionlint/run.sh` check 13 (SMA-647) now bans that pattern repo-wide.
```

These comment lines start with `#`, so check 13 skips them, and `release_guard.py` reads parsed
YAML, where comments do not exist.

- [ ] **Step 6: Run the docs checks**

```
grep -c 'moon-diagnosis:begin' CLAUDE.md
grep -c 'moon-diagnosis:end' CLAUDE.md
/bin/bash $S/sma647/check12-block.sh
/bin/bash $S/sma647/lint-workflows.sh
```

Expected: `1`, `1` (the new entry adds no marker — check 12 counts them anywhere in the file);
`non-empty verdicts on the real CLAUDE.md: 0/100`; `actionlint (shellcheck off) rc=0`.

- [ ] **Step 7: Commit**

```
git add ci/actionlint/run.sh ci/affected-graph/ci_targets.py ci/actionlint/README.md CLAUDE.md .github/workflows/release.yml .github/workflows/prebuild.yml
git commit -m "feat(ci): turn on check 13 over the real corpus and pin its call sites (SMA-647)" -m "Check 13 now reads every tracked shell surface, with a five-member corpus floor. ACTIONLINT_SH_CALL_SITES pins the listing, the floor and the production call at column 0, with a deletion and an indentation case. CLAUDE.md records the mechanism and the three replacement forms." -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 12: Final verification, the mutation run, and the Linear note

**Files:** none, unless a check fails (then fix in a NEW commit).

- [ ] **Step 1: The whole inventory is clean**

```
/bin/bash $S/sma647/scan.sh
/bin/bash $S/sma647/check13-harness.sh
```

Expected: `rows: 0 (all files)`; `check 13 harness: FAILED=0`.

- [ ] **Step 2: The mutation run (spec §8).** Create `$S/sma647/mutate-4777.sh`. It re-inserts the
OLD line 4777 after the new one, runs the check-13 harness, then DELETES the inserted line (never
`git checkout`, which would also revert the uncommitted state) and proves the file is byte-identical
to before:

```bash
#!/bin/bash
# SMA-647 spec §8: re-introduce the old check-12 probe; check 13 must red with exactly ONE row.
set -uo pipefail
S="$(cd "$(dirname "$0")" && pwd)"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-647 || exit 2
F=ci/actionlint/run.sh
OLD="    printf '%s' \"\$block\" | grep -qF -- \"\$lit\" || echo \"missing-literal \$lit\""
NEW="    grep -qF -- \"\$lit\" < <(printf '%s' \"\$block\") && continue"
before="$(shasum "$F" | cut -d' ' -f1)"
n="$(grep -nxF -- "$NEW" "$F" | cut -d: -f1)"
[ -n "$n" ] || { echo "mutation: the rewritten line is not in $F" >&2; exit 2; }
awk -v n="$n" -v old="$OLD" '{ print } NR == n { print old }' "$F" > "$S/run.sh.mutant"
cat "$S/run.sh.mutant" > "$F"
/bin/bash "$S/check13-harness.sh" > "$S/mutation.log" 2>&1
rc=$?
rows="$(grep -c '^actionlint gate: check 13: ' "$S/mutation.log")"
named="$(grep -c "^actionlint gate: check 13: ci/actionlint/run.sh:$((n + 1)) " "$S/mutation.log")"
grep -vxF -- "$OLD" "$F" > "$S/run.sh.restored"
cat "$S/run.sh.restored" > "$F"
after="$(shasum "$F" | cut -d' ' -f1)"
echo "harness rc=$rc, check-13 rows=$rows, rows naming run.sh:$((n + 1))=$named"
[ "$before" = "$after" ] && echo "restored byte-identical" || { echo "RESTORE MISMATCH"; exit 2; }
[ "$rc" -eq 1 ] && [ "$rows" -eq 1 ] && [ "$named" -eq 1 ] && echo "MUTATION KILLED" || { echo "MUTATION SURVIVED"; exit 1; }
```

Run `/bin/bash $S/sma647/mutate-4777.sh`. Expected: `harness rc=1, check-13 rows=1, rows naming
run.sh:<n+1>=1`, `restored byte-identical`, `MUTATION KILLED`. Then `git status --short` → empty.

- [ ] **Step 3: Every touched gate, with the right bash** (from `$WT`; `bounded.sh` on each)

| Gate | Command | Bash | Expected |
|---|---|---|---|
| `repo:actionlint` self-test | `/bin/bash $S/sma647/selftest-actionlint.sh` | 3.2 | `ACTIONLINT-SELFTEST OK` |
| `repo:actionlint` check 13 | `/bin/bash $S/sma647/check13-harness.sh` | 3.2 | `FAILED=0` |
| `repo:actionlint` check 1 (shellcheck off, M-4) | `/bin/bash $S/sma647/lint-workflows.sh` | — | `actionlint (shellcheck off) rc=0` |
| `repo:affected-smoke` (fast part) | `python3 ci/affected-graph/ci_targets.py --self-test` and `python3 ci/affected-graph/ci_targets.py` | — | `OK` / `PASS` |
| `repo:affected-smoke` (whole) | see Step 4 | 3.2 via a shim | rc 0 |
| `repo:release-plan` (check 11) | `/bin/bash ci/release-plan/run.sh --self-test`, `--negative-control`, `--assert` | 3.2 | rc 0 ×3 |
| `repo:workflow-credentials` | `/bin/bash ci/workflow-credentials/run.sh --self-test`, `--negative-control`, and bare | 3.2 | rc 0 ×3 |
| `repo:ruff-ci` | `/opt/homebrew/bin/bash ci/ruff/run.sh --self-test`, `--negative-control`, and bare | 4+ | rc 0 ×3 |
| `repo:publish-metadata` (line 1500) | `/bin/bash $S/sma647/pm-1500.sh` | 3.2 | `pm-1500: OK` |
| `repo:publish-metadata` (whole) | `/opt/homebrew/bin/bash ci/publish-metadata/run.sh --negative-control`, and bare | 4+ | rc 0 ×2, or the M-3 `TIMEOUT` (then CI decides) |
| `repo:release-parity-ts` | `/bin/bash ci/release-parity/run.sh --ecosystem semantic-release --negative-control`, and without the flag | 3.2 | rc 0 ×2 |
| `repo:nats-permissions` (script half) | `/opt/homebrew/bin/bash ops/nats/check-subjects.sh` | 4.3+ | rc 0 |
| `repo:wasm-getrandom-free` | `moon run repo:wasm-getrandom-free --force` | Moon's | rc 0 |
| lefthook pre-push | `/bin/bash .lefthook/pre-push/check-branch.test.sh` | 3.2 | rc 0 |
| images pins | `/bin/bash $S/sma647/images-pins.sh` | 3.2 | `assert_pins: OK` |
| otool awk | `/bin/bash $S/sma647/awk-equiv.sh` | 3.2 | `awk-equiv: OK` |
| `repo:next-public-free` | not touched by this plan | — | — |

A `TIMEOUT` with near-zero CPU under Homebrew bash is the known deadlock (CLAUDE.md LOCAL ONLY), not
a finding. Record it and let CI decide.

- [ ] **Step 4: `repo:affected-smoke` in full, under bash 3.2.** It needs `/bin/bash` 3.2, and its
nested `bash` calls must resolve to 3.2 too, while `python3` must stay 3.12+ (so do NOT put `/bin`
first on PATH). Create a bash-only shim directory and run it:

```
mkdir -p $S/sma647/bash32-shim
ln -sf /bin/bash $S/sma647/bash32-shim/bash
```

`$S/sma647/affected-smoke.sh`:

```bash
#!/bin/bash
# repo:affected-smoke's own script under bash 3.2, with a bash-only shim first on PATH.
S="$(cd "$(dirname "$0")" && pwd)"
export PATH="$S/bash32-shim:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-647 || exit 2
/bin/bash ci/affected-graph/run.sh --negative-control && /bin/bash ci/affected-graph/run.sh
```

Run `/bin/bash $S/sma647/bounded.sh 1200 $S/sma647/affected-smoke.log /bin/bash $S/sma647/affected-smoke.sh`
→ expected `rc=0`. If a sub-3 s abort shows `proto-shim … Permission denied`, it is the CLAUDE.md
`affected-smoke` abort, not a finding: capture the log, then re-run once.

- [ ] **Step 5: The full CI graph on the PR** is the final authority (spec §8). After the PR is open:
`CI` (the `moon ci` step runs `repo:actionlint`'s whole gate, including check 9's sixteen concurrent
`--self-test` runs of the handshake), `wheels.yml` and `prebuild.yml` (their `pull_request` filters
include the workflow files), and `images.yml`, which is not required: watch its run by hand.

- [ ] **Step 6: The Linear note (controller step, not an implementer step).** Post this comment on
SMA-647 with the Linear tool (`save_comment`). Do not close the issue (spec D5: two weeks of CI on
`main` first):

```
Mechanism (MEASURED): grep -q / -m, head and awk … exit stop reading at the first match. A producer
that writes again gets SIGPIPE and exits 141, and under pipefail the pipeline status is then 141
although the reader matched. Check 12's literal probe was `printf '%s' "$block" | grep -qF -- "$lit"`.

Linux (ubuntu:24.04, bash 5.2.21, GNU grep 3.11): 0 false misses in 15000 runs without load; 2 with
8 CPU hogs (PIPESTATUS 141/0). macOS 26.6.2 (BSD grep 2.6.0): 500 of 500 false misses on the real
CLAUDE.md block, under bash 3.2.57 and 5.3.15; the process-substitution form missed 0 of 500. The
same shape made ci/release-plan/run.sh --negative-control fail on every local run (row 8).

Fix (PR): 46 sites rewritten (process substitution, capture-then-match, or sed -n 1p), check 13
bans the pattern in every tracked *.sh, workflow, moon.yml, .moon/**/*.yml and lefthook.yml, and
check 12 reports literal-disagreement when grep and a bash substring match disagree. The issue stays
open for two weeks of CI on main: it closes if no literal-disagreement row and no check 12 or check 9
flake appears.
```

- [ ] **Step 7: No commit** unless a step above failed and needed a fix. A fix is a new
`fix(ci): … (SMA-647)` commit, never an amend.
