# SMA-572 — script-pin the three remaining self-scheduled gates

**Status:** design approved 2026-08-28
**Issue:** [SMA-572](https://linear.app/smaschek/issue/SMA-572/ci-script-pin-the-three-remaining-self-scheduled-gates-in-self)
**Lands with:** [SMA-573](https://linear.app/smaschek/issue/SMA-573/ci-pin-that-repoaffected-smoke-still-declares-the-inputs-every-ci)
(the containment-match variant; §3 is its whole scope)
**Predecessors:** SMA-530 (`release-parity*` pins, `check_registry_pairing`),
SMA-553 (`input-liveness` pin), SMA-542 (checks 8/8b/8c/8d), SMA-576 (`version-lockstep` pin)

## Problem

Five `repo:*` gates run a self-test or negative control from their own `moon.yml`
`script:` block, control first, under an explicit `set -euo pipefail`. Moon does not
enable errexit for `script:` blocks — a script's status is simply its last command's —
so all three lines are load-bearing, and deleting any of them leaves a gate that still
schedules, still exits 0, and has lost its proof that it can report red.

`SELF_SCHEDULED_GATES` in `ci/affected-graph/ci_targets.py` pins those lines. Three
gates are missing from it:

| gate | runs a self-test from `moon.yml` | script-pinned before this change |
| -- | -- | -- |
| `repo:input-liveness` | yes | yes (SMA-553) |
| `repo:release-parity{,-py,-ts}` | yes | yes (SMA-530) |
| `repo:version-lockstep` | yes | yes (SMA-576) |
| `repo:publish-metadata` | yes | **no** |
| `repo:affected-smoke` | yes | **no** |
| `repo:error-code-single-site` | yes | **no** |

SMA-530 left these out deliberately, because each needs its own verification pass, and
because the registry pairing was an equality assert at the time: script-pinning a gate
forced its whole input list to be duplicated into `ci_targets.py`. `check_registry_pairing`
plus the reasoned `SELF_TASK_GLOBS_EXEMPT` registry removed that cost.

A second gap, recorded as L1 in `ci/release-parity/README.md` and filed as SMA-573:
every pin in `ci_targets.py` — `RUN_SH_CALL_SITES`, `SELF_SCHEDULED_GATES`,
`ACTIONLINT_SH_CALL_SITES`, `RELEASE_PARITY_SH_CALL_SITES` — fires only when
`repo:affected-smoke` is scheduled, and **nothing pins the `inputs` list that schedules
it**. Removing `- 'moon.yml'` is self-concealing: the removal is itself a root-`moon.yml`
edit, and afterwards the task's remaining globs do not match that file (`*/moon.yml`
matches `rs/moon.yml`, not `moon.yml`; `.moon/**/*` does not match it either), so the
removal PR does not schedule the gate and every later PR can delete a pinned line with
nothing red. `repo:input-liveness` does not close this — it asserts a declared glob still
matches a tracked file, not that a required glob is still declared.

## Non-goals

- Reachability analysis of bash control flow. Check 8e matches lines, as 8/8b/8c/8d do.
  A required line parked in an unindented `if false; then … fi` or heredoc still satisfies
  it; that residual is already recorded in `ci/actionlint/README.md`.
- Pinning `repo:*` gates that do **not** run a self-test from their script block. This
  registry is about invocation rot, not about gate coverage generally.
- Any change to `T`, to CLAUDE.md's marker-delimited command, or to which gates run in CI.

## Design

### §1 — Three new `SELF_SCHEDULED_GATES` entries

`ci/affected-graph/ci_targets.py` gains three keys. Each pins `set -euo pipefail` and
both invocations, matched as whole stripped lines:

| gate | pinned lines | prefix hazard |
| -- | -- | -- |
| `affected-smoke` | `set -euo pipefail`, `ci/affected-graph/run.sh --negative-control`, `ci/affected-graph/run.sh` | yes |
| `publish-metadata` | `set -euo pipefail`, `bash ci/publish-metadata/run.sh --negative-control`, `bash ci/publish-metadata/run.sh` | yes |
| `error-code-single-site` | `set -euo pipefail`, `python3 ci/error-registry/check.py --self-test`, `python3 ci/error-registry/check.py --single-site` | no |

Whole-line matching is what makes the first two safe: their real-run line is a strict
prefix of their control line, so a substring test would report the script fully wired
after the **real run** had been deleted. `error-code-single-site` has no such hazard
(`--self-test` and `--single-site` are distinct suffixes) but is matched the same way for
uniformity — the table's contract is one rule, not per-entry rules.

No new fixtures are required, and this was verified rather than assumed:

- `wired_scripts()` (`ci_targets.py:1298`) builds a wired script for **every** registry
  key, and `self_test()` asserts `set(scripts) == set(SELF_SCHEDULED_GATES)`.
- The per-line deletion loop (`:1406`) iterates `SELF_SCHEDULED_GATES.items()`, so each
  new gate's three lines each get a "missed &lt;line&gt; deleted" assertion automatically.
- The absent-script loop (`:1424`) does the same for a gate dropped from `moon.yml` whole.
- `pairing("real-registries", None, None, None, ([], [], [], [], []))` (`:1658`) drives
  `check_registry_pairing` over the **live** tables, so the new entries must be paired
  correctly or that row fires.

### §2 — Registry pairing: two exemptions and one delegation

`check_registry_pairing` requires every script-pinned gate to appear in
`SELF_TASK_EXPECTED_GLOBS` or in `SELF_TASK_GLOBS_EXEMPT` with a non-empty reason.

**`publish-metadata` and `error-code-single-site` are exempted.** Their pins are enforced
from a *different* gate: editing either gate's `moon.yml` block re-keys
`repo:affected-smoke`, which lists `moon.yml` among its inputs, so a deleted line reds on
exactly the PR that deletes it. Their own `inputs` are not what makes their pin reachable,
and declared-glob liveness is asserted generically by `repo:input-liveness`
(`ci/affected-graph/task_inputs.py`). An exact-match copy of their input lists here would
red on every legitimate `inputs` edit and buy nothing — the same reasoning already
recorded for the `release-parity*` trio.

**`affected-smoke` is exempted with a reason naming check 8e**, because its real inputs
pin lives in `ci/actionlint/run.sh` (§3). This is a delegation, not a skip: it is the
harder half, done in the one place where it is not self-judging. The reason string must
say so explicitly, and §4 is what stops the delegation rotting.

### §3 — Check 8e in `ci/actionlint/run.sh` (SMA-573's scope)

`repo:affected-smoke` cannot pin its own `inputs` meaningfully — it would be the sole
judge of its own reachability, which is precisely the defect. The repo's established
answer is a second, independently scheduled copy: `repo:actionlint` declares
`inputs: ['**/*']`, so it runs on every PR, and it already holds checks 8/8b/8c/8d for
exactly this reason. Check 8e joins them, following that idiom exactly.

One awk extractor over `moon.yml`'s `affected-smoke:` task block feeds one verdict
function. `moon.yml` is a plain block-style project config (`tasks:` at column 0, task
keys at 2 spaces, fields at 4, `script: |` body and `inputs:` items at 6), and the
extractor is held to that shape; anything it cannot parse must report a verdict token,
never fall through silently.

```
T_AFFECTED_SMOKE_REQUIRED_INPUTS   # containment — the list may grow freely
  moon.yml
  .moon/**/*
  ci/affected-graph/**/*
  ci/actionlint/**/*
  ci/release-parity/**/*
  .github/workflows/ci.yml
  CLAUDE.md

T_AFFECTED_SMOKE_REQUIRED_SCRIPT   # whole lines — the three from §1
  set -euo pipefail
  ci/affected-graph/run.sh --negative-control
  ci/affected-graph/run.sh

affected_smoke_block_verdict moon.yml
  -> no-file | no-task | missing-input <glob> | missing-script <line>
```

**Containment, not equality.** `SELF_TASK_EXPECTED_GLOBS` is an exact match, which is
right for `repo:input-liveness` (whose entire declaration is `**/*`) and for
`repo:version-lockstep` (sixteen static literal paths). `repo:affected-smoke` declares
nineteen inputs and legitimately gains one every time a gate keys on a new directory, so an
exact-match pin would red on every honest addition. Containment asserts the load-bearing
subset is still declared and lets the rest change freely.

**The required-input set has a stated principle**, so a future entry can be judged rather
than guessed. A glob belongs in it when either:

1. an assertion **reads it as a file** — `moon.yml` (the pins and the graph itself),
   `.github/workflows/ci.yml` (C1/C2/C5), `CLAUDE.md` (C3); or
2. a pin's **reachability depends on it**, which is why `ci/actionlint/**/*` (SMA-542),
   `ci/release-parity/**/*` (SMA-530), `ci/affected-graph/**/*` and `.moon/**/*` each
   carry a do-not-remove comment today. Those comments are currently the only enforcement.

`.prototools` is deliberately **out**. It is listed on the task because the guard shells
out to the proto-pinned `moon`, so a version bump should re-run it — a staleness concern,
not an un-reaching one. No pin becomes unreachable if it is dropped.

**Why the script lines are pinned here too.** `ci/affected-graph/run.sh --negative-control`
exits at `run.sh:406` before the real suite. So of `affected-smoke`'s three script lines,
§1's pin catches two: deleting `--negative-control` or `set -euo pipefail` leaves the real
run executing, and `check_self_invocation` fires. Deleting the **real-run line** leaves
only the control, which asserts against synthetic fixtures and exits 0 — self-concealing,
and the one hole §1 alone would ship. Check 8e already parses the block for its inputs, so
pinning the script costs one more table and a handful of fixture rows over the same parse.

**Bash and awk over the raw YAML, not `moon query`.** `ci/actionlint/run.sh` is a pure
bash/awk gate with no toolchain dependency, and keeping it that way is what makes it
independently runnable. Reading the *authored* text is also the more faithful source for
"did someone delete this line" than moon's resolved output, and it makes 8e's source
genuinely independent of `check_gate_inputs`'s.

`SELF_TEST_COUNT` goes 9 → 10 for `affected_smoke_block_self_test`. The gate asserts both
invocations and definitions, and `selftest_mutation_battery` derives its mutants from the
`*_self_test` definitions, so the new table joins the battery automatically. The battery is
parallel-bound; the added wall-clock cost is measured, not estimated (§5).

### §4 — The mutual guard

`ACTIONLINT_SH_CALL_SITES` in `ci_targets.py` gains check 8e's production call site:

```
done < <(affected_smoke_block_verdict moon.yml)
```

Whole-line matched at column 0, like its five siblings, because
`affected_smoke_block_verdict` is also called from inside its own self-test fixtures, so a
substring test would be satisfied by those and survive deleting the production line.

That closes the cycle:

```
ci_targets.py                          ci/actionlint/run.sh
  ACTIONLINT_SH_CALL_SITES  ────────>    check 8e is still invoked
  SELF_TASK_GLOBS_EXEMPT                 T_AFFECTED_SMOKE_REQUIRED_*
    [affected-smoke] = "see 8e"            │
         ▲                                 ▼
         └──── affected-smoke still declares the inputs that
               make every pin in ci_targets.py reachable
```

Neither gate is the sole judge of itself, which is the same shape as check 8c. A second
copy of the required-glob set inside `ci_targets.py` would add a drift risk with no added
coverage, since `repo:actionlint` runs on every PR — so the set lives at one site.

`ci_targets.py`'s new entry is itself reachable because `repo:affected-smoke` already
lists `ci/actionlint/**/*` in its inputs — and check 8e now pins that entry, which is the
guard on the guard.

### §5 — Verification

Every pinned line must be shown to bite, by deleting it and observing a non-zero exit that
names it:

- the 9 new `SELF_SCHEDULED_GATES` lines (3 gates × 3), via `ci/affected-graph/run.sh`;
- check 8e's 7 required inputs and 3 required script lines, via `ci/actionlint/run.sh`,
  with the `- 'moon.yml'` case done explicitly — it is the self-concealing one, so the
  guard must be seen firing on the very edit that removes it;
- the new `ACTIONLINT_SH_CALL_SITES` entry, by deleting check 8e's production line.

Restore by reverting the edit, never by moving a `.bak` file back: that rolls mtime
backwards and a cached PASS then replays over the restored tree. Where Moon caching could
serve a stale result, invoke the scripts directly or pass `--force`.

Then the full graph, exactly as CI runs it, per CLAUDE.md's marker-delimited command.

Measured before/after wall-clock for `ci/actionlint/run.sh` (min of N, not a mean — the
mutation battery is parallel-bound) is recorded in the PR.

### §6 — Documentation

- `ci/affected-graph/README.md` — C4's list of script-pinned gates, and the pairing
  paragraph, gain the three new entries and the delegation to check 8e.
- `ci/actionlint/README.md` — check 8e, its contract, and its residual.
- `CLAUDE.md` — the gotcha describing `SELF_SCHEDULED_GATES` extends to the new gates.
  `T` and the marker-delimited command are unchanged.

## Residuals

- **R1.** Deleting check 8e's block *and* `ci_targets.py`'s `ACTIONLINT_SH_CALL_SITES`
  entry in one edit silences both directions at once. Two independently scheduled gates
  are the most the graph offers; closing a combined deletion needs a third, which only
  moves the problem one level out. Same bounded shape as the existing L6.
- **R2.** Whole-line pins are brittle in the false-red direction: reordering a flag or
  adding a trailing comment reds a harmless edit. Accepted tradeoff, per
  `ci/release-parity/README.md`'s L3. The fix is to restore the line or update the table.
- **R3.** Check 8e matches lines, not reachability. A required line parked in an
  unindented never-executed block still satisfies it, exactly as for 8/8b/8c/8d.
- **R4.** The required-input set is a judgement call captured as a principle (§3), not a
  derivation. A future load-bearing input that nobody adds to the table is unguarded —
  the do-not-remove comment convention remains the first line of defence.
