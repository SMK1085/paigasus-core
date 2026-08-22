# SMA-530 — run the `--negative-control` self-test of `release-parity` in CI

**Status:** revised after adversarial challenge, 2026-08-22
**Issue:** [SMA-530](https://linear.app/smaschek/issue/SMA-530/ci-run-the-negative-control-self-test-of-release-parity-in-ci)
**Related:** SMA-376 (publish-metadata control), SMA-534 (affected-smoke control),
SMA-528 (the live vacuity a control was best placed to catch), SMA-542 / SMA-553
(the guard-the-guard machinery this reuses), SMA-398 / SMA-405 / SMA-406 (the harness itself)

## Problem

`ci/release-parity/run.sh` ships a `--negative-control` mode: it drives `check_case`
with a deliberately wrong expectation (`fix!` in 0.x expected as `0.1.1`, when the
canonical contract says `0.2.0`) and asserts the harness reports red. Nothing runs it
automatically. All three Moon tasks invoke the script bare:

```yaml
  release-parity:     script: 'ci/release-parity/run.sh'
  release-parity-py:  script: 'ci/release-parity/run.sh --ecosystem python-semantic-release'
  release-parity-ts:  script: 'ci/release-parity/run.sh --ecosystem semantic-release'
```

Its two sibling gates both run their control first, under an explicit `set -euo pipefail`
(`repo:publish-metadata`, SMA-376; `repo:affected-smoke`, SMA-534). `release-parity` is
the last uncovered one.

### The premise, demonstrated rather than argued

Change `ci/release-parity/run.sh:51` from

```bash
  if [ "$got_a" = "$expected" ] && [ "$got_b" = "$BASELINE" ]; then
```

to `if [ "$got_b" = "$BASELINE" ]; then`. Slot `b` is at baseline in all five `cases.tsv`
rows, so the real run prints `== all parity cases passed ==` and exits **0** — the gate is
now vacuous, asserting nothing about the commit→semver contract it exists to protect. The
control gets rc 0 back from `check_case` and reds with "harness accepted a wrong
expectation".

That is a plausible refactor shape, and it is the whole justification for this issue: the
real run cannot detect a `check_case` that has lost the ability to report red, because
that is exactly when the real run is green. (Credit: found by the spec challenger while
trying to disprove the premise.)

## Measurements

The issue asked for the shape to be chosen from a measurement, on the theory that
`release-parity` builds real git fixtures and its control might be materially slower.

Measured as the **net delta per task** — the same shell running control-then-real versus
real alone, min of 3, warm — because two independently-timed warm runs misattribute
fixed setup cost that the real run already pays (`proto bin release-plz`, the `py/.venv`
bootstrap at `ecosystems/python-semantic-release.sh:28`, node module resolution):

| task | before | after | delta |
| -- | -- | -- | -- |
| `release-parity` | 4320ms | 5210ms | **+890ms (+21%)** |
| `release-parity-py` | 3661ms | 4394ms | **+733ms (+20%)** |
| `release-parity-ts` | 5676ms | 6787ms | **+1111ms (+20%)** |

**+2.7s total across all three**, a consistent ~20% each. The issue's premise does not
hold. An earlier draft reported the control standalone at 1s/3s/1s; the PSR row was
internally inconsistent (a 1-case control timed the same as a 5-case real run) precisely
because it was measuring the venv bootstrap, which the delta method attributes correctly.

These are local, warm numbers. They are load-bearing only for "cheap enough to prepend",
and the *provisioning* they might understate on a cold runner is provisioning the real run
already pays in the same task — the control adds no new toolchain. See Q3 under Open
questions answered.

> Reproduction note, local only: `proto bin` emits NDJSON when it detects an agent shell
> (`AI_AGENT` / `CLAUDECODE` set), which breaks `RELEASE_PLZ_BIN` resolution at
> `ecosystems/release-plz.sh:16` and yields rc 2 ("INCONCLUSIVE: infrastructure error").
> Clear those two vars when running the gate by hand. Agent-environment artifact, not a
> repo defect, and it does not arise in CI.

## Decisions

### D1 — prepend to the existing tasks; do not add a fourth task

The measurement settles it. A separate `repo:release-parity-selftest` task would cost a
new entry in `ci.yml`'s `T=(…)` array, a matching edit inside CLAUDE.md's
`<!-- ci-targets:begin/end -->` block (enforced token-for-token and in order by
`check_docs`, `ci_targets.py:547-564`), and an `:affected-smoke` re-baseline — for no
coverage a prepend does not already buy. Prepending also keeps the control scheduled by
exactly the same affectedness rule as the gate it guards.

### D2 — each of the three tasks carries its own control

**The rationale in the first draft was false and is withdrawn.** It claimed a control on
`release-plz` "says nothing about whether the semantic-release adapter's
`ecosystem::version` still reads a real version". It says exactly that: `run.sh:74-83`
drives the same `build_fixture`/`apply_commit`/`run_update`/`version` quartet for all five
`cases.tsv` rows, so the control's per-ecosystem code path is a strict **subset** of the
real run's. Everything the control uniquely proves lives in tool-agnostic code — the
comparison at `run.sh:51-57` and the control block at `:60-69`.

The real reasons, in order of weight:

1. **Affectedness makes the tasks independent CI units.** The three have disjoint
   ecosystem-specific inputs (`moon.yml:61-89`). A PR touching only
   `ts/packages/paigasus-sdk/.releaserc.json` selects `release-parity-ts` and neither
   sibling. If the control lived on only one task, that PR runs a parity gate with no
   control at all — reintroducing "runs only when someone remembers", one level over.
2. **Cost.** ~0.9s each; there is nothing to buy by economising.
3. **Symmetry.** Nobody has to remember which of three near-identical tasks is the one
   carrying the control, and a future fourth ecosystem inherits the pattern.
4. **Forward-looking.** If `check_case`'s comparison is ever specialised per ecosystem, a
   single control silently under-covers from that day on, with nothing to signal it.

### D3 — control first, under an explicit `set -euo pipefail`

Moon does not enable errexit for `script:` blocks, so a block's exit status is its **last**
command's. Without the pipefail line a failing control is masked by the passing real run,
which makes the change worse than useless — the same trap `repo:promtool`,
`repo:nats-permissions`, `repo:publish-metadata` and `repo:input-liveness` each document.

`run.sh` setting `set -euo pipefail` internally does not help: that governs the script's
own body, not the Moon block that invokes it twice.

Invocation form stays the current bare `ci/release-parity/run.sh` (the file is `+x`, and
this matches `repo:affected-smoke`; `repo:publish-metadata`'s `bash …` prefix is the
minority form). `--negative-control` goes **last**, after `--ecosystem X`.

### D4 — pin the nine `moon.yml` lines, and pin `run.sh`'s control block too

Two holes, one level apart. Closing only the first — as the first draft did — leaves the
thing being guarded deletable.

**D4a, the `moon.yml` half.** Deleting `--negative-control` from a task script reds
nothing today. `ci/affected-graph/ci_targets.py` already has the machinery:
`SELF_SCHEDULED_GATES`, checked by `check_self_invocation` from inside the independently
scheduled `repo:affected-smoke`. It currently pins one gate. Add three, nine strings
verbatim:

```python
"release-parity": (
    "set -euo pipefail",
    "ci/release-parity/run.sh --negative-control",
    "ci/release-parity/run.sh",
),
"release-parity-py": (
    "set -euo pipefail",
    "ci/release-parity/run.sh --ecosystem python-semantic-release --negative-control",
    "ci/release-parity/run.sh --ecosystem python-semantic-release",
),
"release-parity-ts": (
    "set -euo pipefail",
    "ci/release-parity/run.sh --ecosystem semantic-release --negative-control",
    "ci/release-parity/run.sh --ecosystem semantic-release",
),
```

`set -euo pipefail` is pinned as a first-class required line, not decoration: per D3,
deleting it touches neither invocation's text, so a pin covering only the two commands
would stay green while a failing control is silently swallowed. Same reasoning already
recorded at `ci_targets.py:199-209`.

Whole-line matching is load-bearing. In all three tasks the **real-run line is a strict
prefix of the control line** (one direction, not two — the first draft's "the same hazard
runs the other way" was wrong), so a substring test would let the real run be deleted
while the pin stayed green. `SELF_SCHEDULED_GATES` already compares against a set of
stripped whole lines (`ci_targets.py:673`), which is correct for all nine strings.

**D4b, the `run.sh` half — the hole the first draft missed.** `run.sh:14` parses
`--negative-control` into `NEGATIVE`; `:60-69` is the block that acts on it. Delete the
block and leave the flag parse: `run.sh --negative-control` falls through to the real
suite, exits 0, all three tasks stay green, and CI silently runs the five-case suite twice
per task. The D4a pin sees nothing — it pins `moon.yml` text, not semantics. This is the
repo's recurring lesson (CLAUDE.md; SMA-542 I1; CodeRabbit round 4 C1): *a gate check's own
call site is what goes unguarded*.

Close it with a third registry pinning the control's load-bearing lines as stripped whole
lines:

```python
RELEASE_PARITY_SH_CALL_SITES = (
    '--negative-control) NEGATIVE=1; shift ;;',
    'if [ "$NEGATIVE" = 1 ]; then',
    'ec=0; check_case "neg-fix-bang" "fix!: deliberately wrong" "-" "0.1.1" || ec=$?',
    '1) echo "negative-control OK: harness reported red as expected"; exit 0 ;;',
    '0) echo "negative-control FAILED: harness accepted a wrong expectation" >&2; exit 1 ;;',
)
```

> **Amended during implementation.** This draft listed only the three block lines. Review
> then measured two bypasses that leave all three byte-identical, so the registry ships with
> **five** entries — parse, guard, assertion, and both report arms:
>
> - **Neutering the parse** (`--negative-control) shift ;;`) leaves `NEGATIVE` at its `0`
>   initialisation, so the control branch is never entered and the invocation falls through
>   to the real five-case suite at rc 0 — CI runs the real suite twice per task.
> - **Gutting the assertion** (replacing the `check_case` call with a literal `ec=1`) never
>   reaches the real suite at all: it exits inside the control branch printing
>   `negative-control OK: harness reported red as expected` without having invoked the
>   harness — a control that actively asserts a lie, which is worse than one that does
>   nothing.
>
> Pinning the assertion line couples the pin to the fixture case id and the `0.1.1`
> expectation; that is the accepted cost, and L4 already records the same coupling.

**Reachability requires a new input.** `check_self_invocation` runs only when
`repo:affected-smoke` is scheduled, and its inputs (`moon.yml:130-162`) do **not** list
`ci/release-parity/**`. Without adding it the pin is real but unreachable — the identical
trap SMA-542 had to fix by adding `ci/actionlint/**/*`, and the PR deleting the block would
be exactly the PR that does not schedule the gate. So `repo:affected-smoke` gains
`- 'ci/release-parity/**/*'` with a comment saying why it must not be removed.

### D5 — keep the two registries independent; exempt with a recorded reason

`ci_targets.py:1295` asserts `set(SELF_SCHEDULED_GATES) == set(SELF_TASK_EXPECTED_GLOBS)`,
and the latter pins a gate's `inputs`. That equality would force each `release-parity*`
task's narrow input list to be duplicated into `ci_targets.py` — a second maintenance site
that reds on every legitimate `inputs` edit while buying nothing, since (unlike
`input-liveness`, whose `inputs: ['**/*']` is the thing it exists to protect) those globs
are an ordinary affectedness question already asserted live by `repo:input-liveness`.

The first draft relaxed this to `SELF_TASK_EXPECTED_GLOBS ⊆ SELF_SCHEDULED_GATES`. **That
direction is wrong**, for two reasons the challenge surfaced:

- It makes the follow-up this spec advertises *worse*. `repo:affected-smoke`'s inputs are
  the most load-bearing input list in the repo (`moon.yml:130-162`, several entries
  carrying explicit "do not remove" comments, and D4b adds another). Under a subset test,
  script-pinning `affected-smoke` later without pinning those globs is silent.
- It forbids the very thing D5's own Limitation below wants: a globs-only entry, with no
  script pin.

Instead, keep the registries fully independent and require an explicit exemption in the
repo's established idiom (`T_EXEMPT`, `ALLOW_DEAD_INPUT`, `ALLOW_NO_CARGO_BACKING`,
`BRANCH_SKIP`, `COE_SKIP` all do this):

```python
SELF_TASK_GLOBS_EXEMPT = {
    "release-parity": "narrow ecosystem-specific globs, not load-bearing the way "
                      "input-liveness's `**/*` is; asserted live by repo:input-liveness",
    "release-parity-py": "as release-parity",
    "release-parity-ts": "as release-parity",
}
```

with a non-empty-reason assertion and a stale-entry check, mirroring `check_forward`'s
`bad_exempt` / `stale_exempt` (`ci_targets.py:531-532`). The pairing assert becomes:
every `SELF_SCHEDULED_GATES` key must appear in `SELF_TASK_EXPECTED_GLOBS` **or** in
`SELF_TASK_GLOBS_EXEMPT` with a non-empty reason; an exemption naming no script-pinned
gate is itself reported.

### D6 — pin the three tasks as CI-eligible

`REQUIRED_REPO_TASKS` is `("affected-smoke", "promtool", "publish-metadata")`
(`ci_targets.py:151`). `check_forward` computes `want = eligible - exempt` and
`got = T ∩ repo` (`:513-533`), so removing all three tasks from `T` *and* flipping them
CI-ineligible shrinks both sets consistently and passes green — a control whose durability
is bounded by nobody switching the task off is the same shape as one that runs only when a
human remembers.

Add the three names to `REQUIRED_REPO_TASKS`, and update `check_floor`'s self-test fixture
at `ci_targets.py:917-921`, which asserts the exact missing list.

### D7 — the simpler alternative, considered and rejected

**Alternative:** make `run.sh` itself run the control before the suite (keeping
`--negative-control` as a standalone entry point). Identical cost; deletes D4a, D5 and D8
outright — no `moon.yml` edit, no nine pinned strings, no pairing-assert change — and the
control is re-keyed by `ci/release-parity/**/*` forever rather than once.

**Rejected**, on three grounds:

1. **Defence in depth.** With the alternative, the control block *and* its invocation live
   in one file: a single-file edit kills both, and the pin has to catch both from outside.
   Splitting them across `moon.yml` and `run.sh` means one edit kills at most one, and each
   is pinned from a third, independently scheduled place.
2. **Precedent.** Four sibling gates (`publish-metadata`, `affected-smoke`,
   `input-liveness`, `error-code-single-site`) all wire their control in `moon.yml`, and
   their comments cross-reference each other. Diverging gives the next person wiring a
   control two patterns and no rule for choosing.
3. **Visibility.** `moon.yml` is where a reader looks to learn what a gate runs.

The honest cost of rejecting it is the nine pinned strings and their false-red brittleness
(see Limitations). That is a real cost, accepted deliberately.

### D8 — rebuild `check_self_invocation`'s self-test fixtures around a builder

**This is a BLOCKER on the naive implementation.** Every negative fixture in `self_test()`
asserts `if not check_self_invocation(...)`. Once `SELF_SCHEDULED_GATES` has four keys,
every call whose `scripts` argument lacks the three new entries returns non-empty
*regardless of the mutation under test*, so all of them pass for the wrong reason. Two
populations:

- those taking the shared fixture `scripts = {"input-liveness": wired_script}`
  (`ci_targets.py:1074`) — lines 1107, 1110, 1113, 1149, 1152, 1160, 1171, 1182, 1193,
  1208, 1215, 1224, 1232, 1240, 1248, 1263, 1265;
- those passing a **literal one-key dict** — lines 1118, 1122, 1130, 1134, 1138, 1142, 1267.

Only the positive control at `:1101` reds, and it reds loudly — so an implementer who
merely extends line 1074 sees green and ships population (b) permanently vacuous,
including the `prefix hole` fixture at `:1118` that is the only proof whole-line matching
works. That is the exact vacuity class this PR exists to prevent, introduced by this PR.

Required shape, specified here rather than discovered during implementation:

- a fixture **builder** — `wired_scripts(**overrides)` returning a fully-wired dict for
  **every** key of `SELF_SCHEDULED_GATES`, so a fixture mutates exactly one gate's script
  and every other gate stays wired;
- every existing literal one-key dict rewritten through it;
- a per-gate **positive control**: a fully-wired script for each of the four gates must not
  fire;
- a per-gate **deletion fixture** for each of its three required lines (prefix hole, its
  control, its errexit line) — twelve in total, replacing the three that exist today;
- the same treatment for `RELEASE_PARITY_SH_CALL_SITES` (D4b): a wired `run.sh` fixture
  plus one deletion fixture per pinned line.

### D9 — this PR must exercise its own change

`ci/release-parity/**/*` is an input to all three tasks (`moon.yml:62, 71, 83`), so the
`ci/release-parity/README.md` edit the Limitations below require *also* re-keys them, and
CI runs the new control on the PR that introduces it rather than shipping it unexecuted.
This matters because a `moon.yml`-only edit does **not** select these tasks — measured:

```
$ moon query tasks --affected      # after touching moon.yml only
repo:actionlint
repo:affected-smoke
repo:input-liveness
```

Their own `script:` lives in `moon.yml`, but `moon.yml` is not among their inputs. We
deliberately do **not** add it: that would run ~14s of real git-fixture work on every gate
PR (gate PRs edit `moon.yml` constantly), and it is not what protects the lines — D4's
pins are, and they are reachable. The consequence to accept is that a *later* PR editing
only these `script:` blocks will not re-run the tasks; D4a still reds if it deletes a
pinned line, which is the failure that matters.

## Limitations (record in `ci/release-parity/README.md`)

- **L1 — `repo:affected-smoke`'s own `moon.yml` input is unpinned.** Every pin in D4
  depends on `- 'moon.yml'` at `moon.yml:134`. Deleting that entry is itself a root
  `moon.yml` edit, and post-edit the task's remaining globs do not match the root file
  (`*/moon.yml` matches `rs/moon.yml`, not `moon.yml`; `.moon/**/*` does not match it) — so
  the removal PR would not schedule the gate, and every later PR could delete the control
  lines silently. **Pre-existing and not introduced here**: the existing `input-liveness`
  pin rests on the same entry. Closing it needs a *containment* variant of
  `SELF_TASK_EXPECTED_GLOBS` (today an exact-match), which is a design of its own →
  follow-up issue.
- **L2 — the task-script haystack strips both sides.** `ci_targets.py:673`, rationale at
  `:274-277`: an indented copy inside `if false; then … fi` satisfies the pin. The
  column-0 rule that rejects this for the actionlint haystack is unavailable here, because
  Moon task scripts are indented inside YAML. Also, `set +e` inserted *after* the pipefail
  line satisfies all three pins while re-opening exactly D3's masking.
- **L3 — whole-line pins are brittle in the false-red direction.** Making the base task's
  ecosystem explicit (`--ecosystem release-plz`), adding a trailing comment, or reordering
  flags reds the gate although nothing is broken. Restore the exact line or update the
  constant — the same consequence `ACTIONLINT_SH_CALL_SITES` records for its own entries
  at `ci_targets.py:288-294`.
- **L5 — whole-line pins prove presence, not absence.** They cannot see an INSERTION.
  Measured: adding a bare `NEGATIVE=0` immediately before the guard satisfies all five pins
  and falls through to the real suite at rc 0; so does deleting the block, neutering the
  parse, and parking all five pinned lines verbatim in a never-executed heredoc. Same class
  as L2, and the same one `ci_targets.py` already disclaims for the actionlint haystack
  ("THIS IS NOT REACHABILITY ANALYSIS") — parsing bash control flow in Python is fragile and
  out of scope. A narrower fail-safe strengthening is available if it ever bites: a count
  assertion such as `release_parity_sh_text.count("NEGATIVE=") == 2`, which can only
  false-red. Deliberately not implemented.
- **L4 — the control's `0.1.1` is coupled to `cases.tsv`'s contract.** `run.sh:62-63`
  hardcodes it as "deliberately wrong" for `fix!` in 0.x. Should the canonical contract
  ever change so that value is correct, all three controls red spuriously and the
  diagnosis is non-obvious.

## Non-goals

- **Closing the same `moon.yml` gap for `publish-metadata`, `affected-smoke` and
  `error-code-single-site`.** All three run a self-test from `moon.yml` today and none is
  script-pinned — a real, identical exposure, but not SMA-530's scope, and each needs its
  own verification pass. D5's independent registries are what make adding them later a
  small change *without* skipping their globs. File a follow-up, and note explicitly that
  `affected-smoke`'s entry must pin its inputs, not only its script (per L1).
- **Strengthening the control itself.** It exercises one case with a literal expectation,
  bypassing `resolve_expected` / `ecosystem::expected`. Not a hole: for `semantic-release`,
  deleting `ecosystem::expected` makes the *real* run red (it would expect `0.2.0` and get
  `1.0.0`), so the divergence resolver is covered from the other side.
- Adding a fourth ecosystem, or touching `cases.tsv`.

## Open questions answered

- **Q1 — exact argument order?** `--negative-control` last: `run.sh --ecosystem
  python-semantic-release --negative-control`. All nine strings are written verbatim in D4a.
- **Q2 — should the three join `REQUIRED_REPO_TASKS`?** Yes — D6.
- **Q3 — was the control run on a GitHub runner?** No, and it does not need to be for the
  cost decision. The control invokes the *same* binaries as the real run in the *same*
  task, so cold provisioning (`py/.venv`, proto-managed `release-plz`, the pnpm store) is
  paid once by whichever runs first; the delta method above is what isolates the marginal
  cost. Verification 7 confirms the real CI number on this PR.
- **Q4 — rc 2 (INCONCLUSIVE) now kills the task before the real gate runs.** Intended, and
  a non-regression: the same infrastructure error would surface from the real run. It only
  changes which diagnostic appears first.
- **Q5 — slot `b`'s "stayed at baseline" is satisfied by "the tool produced no version at
  all"** for `-py` and `-ts` (`python-semantic-release.sh:183-189`,
  `semantic-release.sh:156`). Sound: no release *is* baseline, and PSR guards the fallback
  with a `git log` check. Non-goal 2 stands.

## Verification

1. **The premise mutation.** Apply the `if [ "$got_b" = "$BASELINE" ]` mutation from the
   Problem section. For each of the three tasks: `moon run repo:<task> --force` exits
   non-zero and `== all parity cases passed ==` never appears — proving the real run was
   not reached. Confirm the same mutation leaves the *unwired* task green, which is the
   whole point. Restore.
2. **Clean tree.** All three tasks green unmodified (guards against a control that reds
   correct code).
3. **`set -euo pipefail` is load-bearing.** Delete it with the control forced to fail;
   confirm the task reports success. Demonstration, then restore.
4. **The pins bite — all fourteen.** Delete each of the nine `moon.yml` lines and each of
   the five `ci/release-parity/run.sh` lines in turn; `ci/affected-graph/run.sh` names the
   missing call site and exits non-zero every time. Script it.
5. **D4b pin bites, and is reachable.** Delete `run.sh:60-69` leaving the flag parse;
   confirm all three tasks still exit 0 (the hole), and that `ci/affected-graph/run.sh`
   reds. Confirm a `ci/release-parity/**` edit now selects `repo:affected-smoke`
   (`moon query tasks --affected`).
6. **D5/D6/D8.** `ci_targets.py --self-test` and `ci/affected-graph/run.sh
   --negative-control` pass; each new fixture fails when its mutation is reverted; a
   `SELF_TASK_GLOBS_EXEMPT` entry with an empty reason, and one naming no script-pinned
   gate, both red; making one of the three tasks CI-ineligible reds `check_floor`.
7. **Full graph** as CI runs it, per CLAUDE.md's marker-delimited command — including
   re-baselining `assert_task_case` if `ci/release-parity/**/*` on `affected-smoke` shifts
   any case.

## Files touched

| file | change |
| -- | -- |
| `moon.yml` | three `script:` blocks (D1-D3); `ci/release-parity/**/*` added to `repo:affected-smoke`'s inputs (D4b) |
| `ci/affected-graph/ci_targets.py` | `SELF_SCHEDULED_GATES` +3 (D4a); `RELEASE_PARITY_SH_CALL_SITES` (D4b); `SELF_TASK_GLOBS_EXEMPT` + reworked pairing assert (D5); `REQUIRED_REPO_TASKS` +3 (D6); self-test fixture builder and per-gate fixtures (D8) |
| `ci/affected-graph/README.md` | C4's description at `:124` ("currently `repo:input-liveness`'s") and the maintenance paragraph at `:137-149` |
| `ci/release-parity/README.md` | the control-runs-in-CI contract, D2's reasoning, and L1-L4 |
| `CLAUDE.md` | one Gotchas line, alongside the SMA-542 / SMA-553 equivalents |
