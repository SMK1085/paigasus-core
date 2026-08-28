# SMA-572 — script-pin the three remaining self-scheduled gates

**Status:** design approved 2026-08-28; revised after adversarial challenge
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

A third gap, surfaced by the adversarial challenge on this spec's first draft: the fix
for the second one rests on `repo:actionlint` running on every PR, which is true only
because its `inputs` are `['**/*']` — **and nothing pins that either**. Narrowing it to
`.github/workflows/**` is a plausible cost optimisation, and that narrowing PR is itself
green. Afterwards checks 8/8b/8c/8d and the new 8e run on almost no PR.

### Measurements

Four premises this design rests on, measured at the pinned moon 2.3.2 rather than assumed:

| premise | result |
| -- | -- |
| The root `moon.yml` is not an implicit input to the `repo` project's own tasks | **Confirmed.** `repo:deny` resolves to `inputFiles: ['rs/Cargo.lock','rs/deny.toml']` and no `moon.yml`. Only the injected `.moon/*.{yml,…}` glob is added. |
| `**/*` matches dot-prefixed paths, so `repo:actionlint` really is scheduled by a `.github/`-only PR | **Confirmed.** `.github/workflows/ci.yml` selects `repo:actionlint` and `repo:input-liveness`, whose only authored glob is `**/*`. |
| `ci/affected-graph/run.sh --negative-control` exits before the real suite | **Confirmed.** `run.sh:405-409` exits inside `if [ "$NEGATIVE" = 1 ]`, ahead of `run_suite` at `:412`. |
| `ci/publish-metadata/run.sh --negative-control` does the same | **Confirmed** (`run.sh:1247-1249` dispatches on `$1` with no fall-through), and `ci/error-registry/check.py:448-450` matches argv exactly. **This is harmless for both** — see §2. |

`moon` resolves `affected-smoke`'s nineteen authored entries into four `inputFiles`
(`moon.yml`, `.github/workflows/ci.yml`, `CLAUDE.md`, `.prototools`) and fifteen
`inputGlobs`. Check 8e reads the authored YAML text and so sidesteps that split entirely.

## Non-goals

- Reachability analysis of bash control flow. Check 8e matches lines, as 8/8b/8c/8d do.
  A required line parked in an unindented `if false; then … fi` or heredoc still satisfies
  it; that residual is already recorded in `ci/actionlint/README.md`.
- Pinning `repo:*` gates that neither run a self-test from their script block nor carry a
  load-bearing input list. This registry is about invocation rot, not gate coverage generally.
- Any change to `T`, to CLAUDE.md's marker-delimited command, or to which gates run in CI.

## Design

### §1 — Four new `SELF_SCHEDULED_GATES` entries

`ci/affected-graph/ci_targets.py` gains four keys. Three pin `set -euo pipefail` and both
invocations; the fourth pins a single-line script. All matched as whole stripped lines:

| gate | pinned lines | prefix hazard |
| -- | -- | -- |
| `affected-smoke` | `set -euo pipefail`, `ci/affected-graph/run.sh --negative-control`, `ci/affected-graph/run.sh` | yes |
| `publish-metadata` | `set -euo pipefail`, `bash ci/publish-metadata/run.sh --negative-control`, `bash ci/publish-metadata/run.sh` | yes |
| `error-code-single-site` | `set -euo pipefail`, `python3 ci/error-registry/check.py --self-test`, `python3 ci/error-registry/check.py --single-site` | no |
| `actionlint` | `ci/actionlint/run.sh` | n/a — one command, so its status is the script's; no `pipefail` line to pin |

Verified against `moon query tasks`: the four resolved scripts are exactly these lines.

Whole-line matching is what makes the first two safe: their real-run line is a strict
prefix of their control line, so a substring test would report the script fully wired
after the **real run** had been deleted. `error-code-single-site` has no such hazard
(`--self-test` and `--single-site` are distinct suffixes) but is matched the same way for
uniformity — the table's contract is one rule, not per-entry rules.

`actionlint` is registered here for a second reason as well as its own invocation: §2 pins
its `inputs`, and `check_registry_pairing`'s `orphan_globs` row reports a gate that is in
`SELF_TASK_EXPECTED_GLOBS` but not in `SELF_SCHEDULED_GATES`. Registering it satisfies the
pairing rule honestly rather than by loosening it.

**`affected-smoke`'s third entry has no true-positive coverage, deliberately.** Any state
in which the bare `ci/affected-graph/run.sh` line is absent is a state in which
`check_self_invocation` never runs, so from here that entry can only ever produce a false
red on a harmless reformat. Its real enforcement is check 8e (§3). It is retained so the
table's contract stays "every line, one rule", and both the registry comment and this
section must say so, or a future reader will assume coverage that is not there.

No new fixtures are required for the registry itself, and this was verified rather than
assumed:

- `wired_scripts()` (`ci_targets.py:1298`) builds a wired script for **every** registry
  key, and `self_test()` asserts `set(scripts) == set(SELF_SCHEDULED_GATES)`.
- The per-line deletion loop (`:1406`) iterates `SELF_SCHEDULED_GATES.items()`, so each
  new gate's lines each get a "missed <line> deleted" assertion automatically.
- The absent-script loop (`:1424`) does the same for a gate dropped from `moon.yml` whole.
- The one hardcoded fixture (`:1341`) is input-liveness-specific and unaffected.
- `pairing("real-registries", None, None, None, ([], [], [], [], []))` (`:1658`) drives
  `check_registry_pairing` over the **live** tables, so the new entries must be paired
  correctly or that row fires.

### §2 — Registry pairing: three exact pins, one delegation

`check_registry_pairing` requires every script-pinned gate to appear in
`SELF_TASK_EXPECTED_GLOBS` or in `SELF_TASK_GLOBS_EXEMPT` with a non-empty reason.

**`publish-metadata`, `error-code-single-site` and `actionlint` get exact-match entries in
`SELF_TASK_EXPECTED_GLOBS`.** The first draft exempted the first two on the grounds that
declared-glob liveness is asserted generically by `repo:input-liveness`. That reasoning is
wrong and the challenge caught it: `task_inputs.py` asserts a *declared* glob still matches
a tracked file — it cannot see a *removed declaration*. Both gates have documented
do-not-remove inputs whose deletion would otherwise be green everywhere:

- `moon.yml:520-521` — `security-scan.yml` is an input "because Check 4 ASSERTS ON IT:
  without this, the call-site pin would serve a cached pass on exactly the PR that deletes
  the job."
- `moon.yml:628-630` — `error-code-single-site`'s broad `rs/crates/**/src/**/*.rs` exists
  because "this gate's whole job is to notice a NEW emission site in a NEW file"; narrow it
  and "the one case it exists for would be the one case it never runs on."

Both input sets are **static** — eleven and three authored entries, no runtime discovery —
which is the same shape as `version-lockstep`'s sixteen, so exact match is affordable and
a change to either is a reviewed change. Values below are globs sorted, then literal files
sorted, per `check_gate_inputs`'s comparison order, and were read off `moon query tasks`:

```
"actionlint":              ("**/*",)

"publish-metadata":        ("rs/crates/**/*",
                            ".github/workflows/security-scan.yml", ".gitignore",
                            "ci/publish-metadata/categories.py",
                            "ci/publish-metadata/crates-io-categories.txt",
                            "ci/publish-metadata/run.sh", "rs/.cargo/config.toml",
                            "rs/Cargo.lock", "rs/Cargo.toml",
                            "rs/release-plz.toml", "rs/rust-toolchain.toml")

"error-code-single-site":  ("ci/error-registry/**/*", "rs/crates/**/src/**/*.rs",
                            "contracts/proto/paigasus/common/v1/error.proto")
```

`actionlint`'s entry is what closes the third gap in the Problem section. It is checked
from `ci_targets.py`, which runs inside `repo:affected-smoke` — a different gate — so
there is no self-judging: narrowing `repo:actionlint`'s `inputs` is a root-`moon.yml` edit,
which schedules `affected-smoke`, which reds.

**Only `affected-smoke` is exempted, with a reason naming check 8e**, because its real
inputs pin lives in `ci/actionlint/run.sh` (§3). This is a delegation, not a skip: it is
the harder half, done in the one place where it is not self-judging. The reason string must
say so explicitly, and §4 is what stops the delegation rotting. `SELF_TASK_GLOBS_EXEMPT`
therefore ends up holding the three `release-parity*` entries plus this one.

**Why the shared `--negative-control` early-exit shape is harmless for the other gates.**
The Measurements table records that `publish-metadata`'s control also exits before its real
suite, and that `error-code-single-site`'s two flags are mutually exclusive. That does *not*
give either the zero-true-positive property §1 records for `affected-smoke`, because neither
judges itself: deleting a real-run line from either block is a `moon.yml` edit, which
re-keys `repo:affected-smoke`, which runs `check_self_invocation` against
`moon query tasks`' resolved script and reds. Self-concealment needs the deleted line to be
the thing that would have run the checker — true only for `affected-smoke`.

### §3 — Check 8e in `ci/actionlint/run.sh` (SMA-573's scope)

`repo:affected-smoke` cannot pin its own `inputs` meaningfully — it would be the sole
judge of its own reachability, which is precisely the defect. The repo's established
answer is a second, independently scheduled copy: `repo:actionlint` declares
`inputs: ['**/*']` (now pinned, §2), so it runs on every PR, and it already holds checks
8/8b/8c/8d for exactly this reason. Check 8e joins them, following that idiom exactly.

#### The required sets

```
T_AFFECTED_SMOKE_REQUIRED_INPUTS   # containment — all 19 currently declared
  ci/affected-graph/**/*   .github/workflows/ci.yml   .moon/**/*
  moon.yml                 */moon.yml                 rs/crates/*/*/moon.yml
  py/packages/*/moon.yml   ts/packages/*/moon.yml     ts/apps/*/moon.yml
  rs/**/Cargo.toml         py/packages/*/pyproject.toml
  rs/crates/*/*/pyproject.toml   rs/crates/*/*/package.json
  ts/packages/*/package.json     ts/apps/*/package.json
  ci/actionlint/**/*       ci/release-parity/**/*     CLAUDE.md   .prototools

T_AFFECTED_SMOKE_REQUIRED_SCRIPT   # whole lines, IN ORDER — the three from §1
  set -euo pipefail
  ci/affected-graph/run.sh --negative-control
  ci/affected-graph/run.sh
```

**The required set is the whole current list, not a judged subset.** The first draft chose
seven globs by a stated principle ("an assertion reads it as a file", or "a pin's
reachability depends on it"). The challenge showed the principle, applied honestly, pulls
in most of the rest anyway: `cargo_moon_parity.py:478-479` reads every crate `Cargo.toml`
from disk, so `rs/**/Cargo.toml` qualifies under the first clause; and a crate's own
`moon.yml` is not an input to its own tasks (`cargo_moon_parity.py:315`, SMA-528 F5), which
is exactly *why* `affected-smoke` must key on the four `*/moon.yml` families — drop
`rs/crates/*/*/moon.yml` and a PR changing only a crate's `dependsOn` or
`fileGroups.upstreams` serves a cached PASS on the very edit A5/A6 exist to catch. That is
the same self-concealment shape as the `moon.yml` case.

Requiring all nineteen makes the table a **floor** rather than a judgement call: it removes
the "is this one load-bearing?" question that the next reviewer would otherwise have to
re-litigate, and it answers the `.prototools` question by not asking it. Containment is
still the right mechanism, because the property that matters is that the list may **grow**
freely — which is what exact match punishes and what happens every time a gate keys on a
new directory.

**Removal needs an escape hatch, per the repo's idiom** (`T_EXEMPT`, `ALLOW_DEAD_INPUT`,
`BRANCH_SKIP`, `COE_SKIP`, `SELF_TASK_GLOBS_EXEMPT` all work this way): a
`REQUIRED_INPUT_SKIP` table keyed by glob with a required non-empty reason, so a legitimate
removal is a reviewed, stated decision rather than an edit indistinguishable from an
attacker's. An entry naming a glob that is still declared is itself reported, so skips
cannot outlive their globs.

This also resolves the one conflict with `repo:input-liveness`: if a directory a required
glob names is ever renamed, `task_inputs.py` demands the dead glob be removed while 8e
demands it stay. The resolution is to update `T_AFFECTED_SMOKE_REQUIRED_INPUTS` in the same
commit; `ALLOW_DEAD_INPUT` is **not** an escape from 8e.

#### The extractor contract

One awk extractor over `moon.yml`'s `affected-smoke:` task block emits two **separate**
record streams, `INPUT` and `SCRIPT`, and the two tables are matched against their own
stream only — never a concatenated haystack. `check_self_invocation` documents why
(`ci_targets.py:842-843`): a required string living in the wrong place must not satisfy
another's requirement. A fixture plants a required input string inside the script body and
asserts `missing-input` is still reported.

Accepted authored forms, and the verdict token for everything else:

| shape | handling |
| -- | -- |
| `- 'glob'`, `- "glob"`, `- glob` | accepted; surrounding quotes stripped, one form each |
| `#` comment lines interleaved in the `inputs:` block | skipped (the live file has them at `moon.yml:181-202`) |
| `#` lines inside the `script:` literal block | kept — they are bash, not YAML |
| a commented-out required script line | reports `missing-script`; this is the property whole-line matching buys (`ci_targets.py:836-840`) |
| `script:` folded (`>`) or inline | `bad-script-form` |
| `inputs:` inline flow (`[a, b]`) | `bad-inputs-form` |
| a trailing comment on the task key (`affected-smoke:  # …`) | tolerated on the key line; a non-comment tail is `bad-task-form` |
| a second `inputs:` or `script:` key in the block | `duplicate-key <name>` |
| task absent, file missing/unreadable | `no-task`, `no-file` |

Failing loudly on an unparsed shape rather than skipping in silence is the same rule
CLAUDE.md already records for the actionlint filter extractor. Every row above gets a
fixture.

**Order is asserted for the script table**, not just presence. `check_self_invocation`
compares a *set* of stripped lines (`ci_targets.py:851`), so moving `set -euo pipefail`
below the two invocations keeps every registry entry green while errexit becomes useless.
8e is already parsing the block line by line, so asserting the three lines appear in order
is nearly free here — and this makes `affected-smoke` the one gate where that hole is
closed. It stays open for the others; see R5.

**The tables must not be emptiable.** A verdict function shaped like its siblings
(`ci/actionlint/run.sh:2060-2062`) iterates the array, so an empty array emits zero
verdicts and passes. Check 8c survives that only because its table is a verbatim dual copy
of `RUN_SH_CALL_SITES` and the other copy still asserts the same lines — a mechanism §4
declines for 8e. Instead, two arity floors sit at column 0 in `run.sh` and are themselves
pinned from `ci_targets.py` (§4):

```bash
[ "${#T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}" -ge 19 ] || infra "…"
[ "${#T_AFFECTED_SMOKE_REQUIRED_SCRIPT[@]}" -ge 3 ] || infra "…"
```

`-ge`, not `-eq`, so honest growth needs no second edit; only a shrink — which a
`REQUIRED_INPUT_SKIP` removal implies — requires touching both independently scheduled
files.

**Bash and awk over the raw YAML, not `moon query`.** `ci/actionlint/run.sh` is a pure
bash/awk gate with no toolchain dependency, and keeping it that way is what makes it
independently runnable. Reading the *authored* text is also the more faithful source for
"did someone delete this line", and it avoids the `inputFiles`/`inputGlobs` split moon
applies to this task's nineteen entries.

`SELF_TEST_COUNT` goes 9 → 10 for `affected_smoke_block_self_test`. The gate asserts both
invocations and definitions, and `selftest_mutation_battery` derives its mutants from
`run_self_tests`' own body (`run.sh:3488-3505`), so the new table joins the battery
automatically.

### §4 — The mutual guard

`ACTIONLINT_SH_CALL_SITES` in `ci_targets.py` gains **two** column-0 whole lines — check
8e's production call site and its input-table arity floor:

```
done < <(affected_smoke_block_verdict moon.yml)
[ "${#T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}" -ge 19 ] || infra "…"
```

Whole-line matched, like their siblings, because `affected_smoke_block_verdict` is also
called from inside its own self-test fixtures, so a substring test would be satisfied by
those and survive deleting the production line.

That closes the cycle in three directions rather than two:

```
ci_targets.py                              ci/actionlint/run.sh
  ACTIONLINT_SH_CALL_SITES  ──────────────>  check 8e is still invoked,
                                             and its table is still non-empty
  SELF_TASK_EXPECTED_GLOBS["actionlint"] ─>  repo:actionlint still has inputs: ['**/*'],
                                             so 8/8b/8c/8d/8e still run on every PR
  SELF_TASK_GLOBS_EXEMPT["affected-smoke"]
         ▲                                          │
         └──────────────────────────────────────────┘
           8e: affected-smoke still declares the inputs
           that make every pin in ci_targets.py reachable
```

Neither gate is the sole judge of itself, the same shape as check 8c. A second copy of the
required-glob set inside `ci_targets.py` would add drift risk; the arity floor buys the
non-emptiability that the dual copy would have bought, at one site.

`ci_targets.py`'s new entries are reachable because `repo:affected-smoke` lists
`ci/actionlint/**/*` in its inputs — and check 8e now pins that entry, which is the guard
on the guard.

### §5 — Verification

Every pinned line must be shown to bite, in **both** directions.

**Positive controls first** — a table whose rows all fire cannot tell a working check from
a stuck one (`ci/actionlint/run.sh:2065-2066`, SMA-466). On the unmutated tree:
`affected_smoke_block_verdict moon.yml` must emit **zero** verdicts, and `no-file`,
`no-task` and unreadable-path rows must each report their own token.

**Then the firing direction**, deleting each pinned line in turn and restoring:

- the 10 new `SELF_SCHEDULED_GATES` lines (3 gates × 3, plus `actionlint`'s one), via
  `ci/affected-graph/run.sh`;
- the three new `SELF_TASK_EXPECTED_GLOBS` entries, by dropping one input from each of
  `publish-metadata`, `error-code-single-site` and `actionlint` — the last being the case
  that narrowing `repo:actionlint` to `.github/workflows/**` now reds;
- check 8e's 19 required inputs and 3 required script lines, via `ci/actionlint/run.sh`,
  plus a reorder case (move `set -euo pipefail` last) and an emptied-table case;
- the two new `ACTIONLINT_SH_CALL_SITES` entries, by deleting check 8e's production line
  and by deleting its arity floor.

**The `moon.yml` and real-run cases get a three-part demonstration**, because the obvious
version of each passes vacuously. Deleting `ci/affected-graph/run.sh` from the script block
and then invoking `ci/affected-graph/run.sh` *by hand* reds — but only because the hand
invocation is the one path the deletion does not break, so that evidence says the opposite
of the truth. Instead, for each of those two cases:

1. `moon run repo:affected-smoke --force` must exit **0**, proving §1 alone is blind;
2. `ci/actionlint/run.sh` must exit non-zero naming
   `missing-script ci/affected-graph/run.sh` (resp. `missing-input moon.yml`);
3. restore, and re-run both.

Restore by reverting the edit, never by moving a `.bak` file back: that rolls mtime
backwards and a cached PASS then replays over the restored tree. Where Moon caching could
serve a stale result, invoke the scripts directly or pass `--force`.

Then the full graph, exactly as CI runs it, per CLAUDE.md's marker-delimited command.

**Cost budget.** `ci/actionlint/run.sh` runs on every PR, and the battery is
O(tables × mutants). Measured baseline on this branch before any change: **34.6s**
standalone (min of 3: 37.2 / 39.1 / 34.6 — min, not mean, because the battery is
parallel-bound). The design is reconsidered if the post-change min-of-3 exceeds
**baseline + 10%**; the measured pair goes in the PR either way.

### §6 — Documentation

Five sites carry literal counts or lists that this change invalidates:

- `ci/affected-graph/README.md` — C4's list of script-pinned gates, and the pairing
  paragraph: four new entries, three new exact-glob pins, and the delegation to check 8e.
- `ci/actionlint/README.md` — check 8e, its extractor contract, and its residuals.
- `ci/release-parity/README.md` — **L1 is closed by this change** and must say so rather
  than continue asserting a hole that no longer exists.
- `CLAUDE.md` — the release-parity gotcha's "pins the nine `moon.yml` lines" and
  version-lockstep's "**four**" both change once four more gates are registered.
- `ci/actionlint/run.sh:40` (`SELF_TEST_COUNT=9` and its inline nine-name list) and
  `moon.yml:594-620`, which hardcodes "nine fixture tables", "`--self-test` ALONE … makes
  9", and "ten concurrent subprocesses".

`T` and the marker-delimited command are unchanged.

## Residuals

- **R1.** Deleting check 8e's block *and* `ci_targets.py`'s `ACTIONLINT_SH_CALL_SITES`
  entries in one edit silences both directions at once. Two independently scheduled gates
  are the most the graph offers; closing a combined deletion needs a third, which only
  moves the problem one level out. Same bounded shape as the existing L6.
- **R2.** Whole-line pins are brittle in the false-red direction: reordering a flag or
  adding a trailing comment reds a harmless edit. Accepted tradeoff, per
  `ci/release-parity/README.md`'s L3. The remedy is to **restore the line** — not to shrink
  the table, which the arity floor now makes a two-file edit anyway.
- **R3.** Check 8e matches lines, not reachability. A required line parked in an
  unindented never-executed block still satisfies it, exactly as for 8/8b/8c/8d.
- **R5.** `check_self_invocation` compares a set, so line **order** inside a `script:` block
  is unpinned for every registered gate except `affected-smoke` (§3). Moving
  `set -euo pipefail` below the invocations leaves the registry green while errexit stops
  mattering. Closing it generally means teaching that function about order, which is a
  larger change than this issue.
- **R6.** `REQUIRED_INPUT_SKIP` is itself an unguarded escape hatch — an attacker with
  commit access can add an entry with a plausible reason. That is true of every such
  registry in this repo (`T_EXEMPT`, `ALLOW_DEAD_INPUT`, `BRANCH_SKIP`, `COE_SKIP`); the
  defence is review, and the value is that the decision is explicit and greppable.
