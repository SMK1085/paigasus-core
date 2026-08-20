# SMA-542 — Guard the actionlint gate's self-test invocations from deletion

**Status:** drafted (2026-08-20)
**Linear:** [SMA-542](https://linear.app/smaschek/issue/SMA-542/repo-actionlint-gate-guard-the-self-test-invocations-from-deletion)
**Related:** SMA-525 (`repo:actionlint`, whose mutation battery found this as the sole survivor),
SMA-540 (`branches:` half of check 5, which added the third and fourth self-tests), SMA-541
(`ci_targets.py`, whose C1/C4 are the other half of the cycle below, and whose limitation L6 this
issue absorbs), SMA-553 (`repo:input-liveness`, the most recent extension of C4)

## 1. Problem

`ci/actionlint/run.sh` ends with four unconditional calls:

```bash
extractor_self_test
path_filter_self_test
branch_filter_self_test
config_self_test

exit "$FAILED"
```

Each self-test reports through the global `FAILED` (via `fail()`) as well as its return code, so
the invocation *is* the assertion. Delete the four lines and the four fixture tables become dead
code: the gate still runs checks 1–6, still exits 0, and check 7 — the standing control for the
vocabulary rules that close the failure this gate was filed for — silently stops existing.

SMA-525's final review ran a mutation battery over this file. It killed 22/22 logic mutants.
Deleting the invocations was the **sole survivor**, including when combined with neutering check 5.
`run.sh`'s own header records the principle this violates, from SMA-525's finding F4: *a one-off
mutation battery is not a standing control.*

A script cannot assert its own invocation. The fix has to come from outside it.

### 1.1 The same shape, one level up

Sven's 2026-08-19 comment on this issue folds in SMA-541's L6, which is the identical defect with a
larger blast radius.

Deleting `:affected-smoke` from `ci.yml`'s `T=(…)` array means `moon ci` never schedules
`repo:affected-smoke`. That removes, in one edit:

- `ci_targets.py`'s C1–C5, including the `REQUIRED_REPO_TASKS` floor
- all eight affected-graph cascade cases
- `cargo_moon_parity.py`'s A1–A5
- `assert_include_relations`

Every check stays green, because the thing that would have complained is the thing that stopped
running. **The one entry of `T` that must never be removed is the entry that guards `T`.**

It is unclosable from inside `ci/affected-graph/run.sh` for exactly the reason this issue exists.
It needs an independently-scheduled gate — and `repo:actionlint` is the natural host: it already
declares `inputs: ['**/*']`, already parses every workflow file, and is scheduled separately from
`repo:affected-smoke`, so it survives precisely the deletion that silences the other.

## 2. Evidence

### 2.1 The invocations are the assertion

`extractor_self_test` (`run.sh:273`), `path_filter_self_test` (`:1283`),
`branch_filter_self_test` (`:1385`) and `config_self_test` (`:1551`) each accumulate a local `rc`
**and** call `fail()`, which sets the global `FAILED=1`. The tail invocations (`:1868-1871`)
discard the return values; propagation is entirely through the global. Removing a call therefore
removes its verdict outright — there is no residual signal.

### 2.2 The invocations appear twice

The four calls exist in two places: the `--self-test` case arm (`run.sh:1611-1614`) and the tail
(`:1868-1871`). Two byte-identical blocks are hostile to a source-text pin: deleting one leaves the
other matching, so an external check that greps for `extractor_self_test` cannot tell a wired file
from a half-wired one. §3 D2 collapses them.

### 2.3 C1 already covers `:actionlint`

`check_forward` (`ci_targets.py:397`) computes `want = eligible - set(exempt)` over the `repo`
project's CI-eligible tasks and reports `sorted(want - got)` as `missing` — **strict equality**, not
a superset test. `repo:actionlint` is CI-eligible and is not in `T_EXEMPT`, so dropping `:actionlint`
from `T` already reds `repo:affected-smoke`.

This is load-bearing for the design and it inverts part of the issue comment's suggestion: an
assertion inside `run.sh` that `:actionlint` is present in `T` would be **vacuous**, because the
only run in which the entry is missing is the run in which the assertion does not execute. It is
deliberately excluded from the floor in §4, with that reasoning recorded at the site.

### 2.4 C4's existing shape, and its two matching modes

`check_self_invocation` (`ci_targets.py:539`) already pins call sites in two different ways, and
the asymmetry is documented as deliberate:

- `RUN_SH_CALL_SITES` — **substring** match, because the lines are indented and one is a mid-line
  fragment. Each entry carries its `|| RC=1` propagation suffix, which is what makes it
  unambiguous. CodeRabbit round 3 on SMA-541 found that matching the command prefix alone let
  `--self-test || true` look identical to a wired call site.
- `SELF_SCHEDULED_GATES` — **whole stripped line** match, because
  `python3 ci/affected-graph/task_inputs.py` is a strict prefix of the same line plus
  ` --self-test`, so a substring test would report the script as wired after the real run had been
  deleted.

The two texts are checked separately rather than against one concatenated haystack, so a call site
living in the wrong file cannot satisfy the other's requirement. The extension in §4 follows all
three of these conventions.

`RUN_SH_CALL_SITES`' own comment names this issue as the general fix for its residual: *"A PARTIAL
mitigation, not a closure: deleting the `assert_ci_targets` call removes C4 along with it.
SMA-542 is the general fix for this class (spec L6)."*

### 2.5 AC-2's baseline numbers predate SMA-540

AC-2 pins the real-file extraction counts as `prebuild.yml` 2 KEY / 13 ITEM, `security-scan.yml`
1 KEY / 5 ITEM, `ci.yml` none. Measured on `a607ce0`, `extract_filter_keys` now reports:

| File | `paths` KEY / ITEM | `branches` KEY / ITEM | Total KEY / ITEM |
|---|---|---|---|
| `ci.yml` | 0 / 0 | 2 / 2 | 2 / 2 |
| `prebuild.yml` | 2 / 13 | 2 / 2 | 4 / 15 |
| `security-scan.yml` | 1 / 5 | 1 / 1 | 2 / 6 |

The AC's numbers match the **`paths` family exactly**. The `branches` rows arrived with SMA-540,
after the AC was written. AC-2 is therefore satisfied by holding this whole table constant, and the
table is recorded here so a future reader does not misread the totals as a regression against the
AC's text.

## 3. Design decisions

### D1 — A cycle of independently-scheduled gates, not a chain

```
ci.yml   T=( … :actionlint … :affected-smoke … )
            ▲                          ▲
            │ C1 check_forward         │ check 8  (NEW, in ci/actionlint/run.sh)
            │ (strict equality)        │ T_FLOOR=(':affected-smoke')
            │                          │
     repo:affected-smoke ──────────► repo:actionlint
            │   C4 pins run.sh's two call-site lines (NEW)
            ▼
      ci_targets.py
```

Each gate asserts the *other's* continued scheduling. Neither is the sole judge of its own
configuration — the same principle `repo:input-liveness` already applies to its own `inputs`
(asserted by both its own D13 floor and `ci_targets.py`).

Rejected: a linear chain (actionlint guards affected-graph, nothing guards actionlint). It moves
the hole rather than closing it.

### D2 — One call site for the self-tests, not two

The `--self-test` arm and the tail collapse into a single invocation placed after the argument
dispatch and **before** the `command -v actionlint` guard:

```bash
SELF_TEST_ONLY=0
case "$#:${1:-}" in
  '0:')            ;;
  '1:--self-test') SELF_TEST_ONLY=1 ;;
  *)               usage ;;
esac

run_self_tests
selftest_mutation_battery
if [ "$SELF_TEST_ONLY" = 1 ]; then exit "$FAILED"; fi

command -v actionlint >/dev/null 2>&1 || infra "actionlint not on PATH — run 'proto install actionlint'"
```

Three properties, in order of importance:

1. **One line to pin.** §2.2's duplicate-block ambiguity disappears; C4 gets an unambiguous target.
2. **`--self-test` still needs no `actionlint` binary.** The call sits ahead of the PATH guard,
   preserving the property the guard's own comment exists to protect.
3. **Self-tests run first.** This matches the convention `moon.yml` already states for
   `repo:affected-smoke`, `repo:publish-metadata` and `repo:error-code-single-site` — *"a rotted
   checker must red rather than ship green"*.

Check **numbers are not renumbered**. They are logical identities, not execution order — checks 5
and 6 are already defined ahead of check 1 in the file. Check 7 remains "the self-tests"; the new
work is checks 8 and 9.

All four self-test functions are defined by `run.sh:1551`, comfortably ahead of the dispatch at
`:1604`, so the single call site can reach every one of them.

### D3 — The counter increments *inside* each self-test

```bash
extractor_self_test() {
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
  …
```

Not at the call site. Two reasons: it survives reformatting of the call block, and it cannot be
spoofed by an increment left stranded after its call is deleted. This is the form the issue
prefers, and its advantage over a grep is that it does not depend on matching source text.

`run_self_tests` resets `SELF_TESTS_RAN=0` before the block and asserts the total after it, so the
counter is a property of one function rather than of the whole script's control flow.

### D4 — `assert_self_tests_ran` needs no external pin

Deleting the assertion line inside `run_self_tests` is caught by check 9: every mutant would then
exit 0 and the battery reports *"mutant survived"*. Deleting the battery's invocation is caught by
C4. Deleting **both** is still caught by C4, because the battery line is gone.

So C4 pins exactly two lines — `run_self_tests` and `selftest_mutation_battery` — and the third is
covered by a control that cannot be satisfied vacuously. Pinning it as well would add brittleness
against reformatting for no coverage. The reasoning is recorded at the constant.

### D5 — Mutants suppress the battery by construction, not by an env var

Check 9 runs each mutant with `--self-test`, which under D2 also runs the battery — so a mutant
would recurse. Every mutant is therefore built by deleting **two** lines: the targeted self-test
invocation *and* the `selftest_mutation_battery` invocation.

Rejected: a `PAIGASUS_ACTIONLINT_MUTANT=1` recursion guard. It would be a live bypass surface in a
gate whose entire premise is that it cannot be silently switched off, and it is unnecessary — the
recursion is structurally impossible once the line is gone.

The battery asserts **exactly two lines were removed** before spawning anything. A reformatted
invocation makes the `sed` match nothing, and without this precondition the mutant would be
byte-identical to the original, exit 0, and be reported as a survivor — an accurate red for a
misleading reason, or worse, a recursion if only the self-test line matched. The precondition turns
both into a precise failure before a subprocess exists.

### D6 — The battery carries a control pair

Five mutants that all fire cannot distinguish a working battery from a stuck one — SMA-466's
lesson, and the reason `branch_filter_self_test` carries both directions. Check 9 therefore also
runs an **unmutated control**: `run.sh` with only the battery invocation removed, which must exit
**0**. Without it, a battery whose `bash` invocation was broken would report five dead mutants and
read as maximally healthy.

### D7 — The `T` floor lives in bash, inside `run.sh`

`ci_targets.py` already parses `T=(…)` and has self-tests for it, so reusing it would be
single-site. Rejected: it would couple the gate that exists to survive affected-graph's compromise
to a file in affected-graph's own directory, and the coupling is the one property the design is
buying. The floor check is ~15 lines of tokenisation against a single-line array — far below the
threshold where duplication costs more than the independence it buys.

Divergence between the two parsers fails **safe**: `ci_targets.py`'s C3 already requires `T` to be
a single-line bash array, and the floor check fails loudly rather than skipping when it cannot find
exactly one `T=(` line.

### D8 — The floor contains `:affected-smoke` only

Per §2.3, `:actionlint` is covered by C1 and would be vacuous here. `:promtool` and
`:publish-metadata` are in `ci_targets.py`'s `REQUIRED_REPO_TASKS`, which the floor keeps alive by
keeping `:affected-smoke` scheduled — so restating them would be redundant. The floor is the
minimal set that the rest of the graph cannot assert about itself: one entry.

### D9 — No new Moon task

Nothing is added to `ci.yml`'s `T=(…)`, so CLAUDE.md's `ci-targets` marker block is untouched, and
`repo:affected-smoke`'s C3 and `repo:input-liveness` have nothing new to check. The gate that hosts
the new work is already wired. This keeps the diff to two scripts plus documentation.

## 4. Components

### `ci/actionlint/run.sh`

**Counter state**, declared alongside `FAILED`:

```bash
SELF_TESTS_RAN=0
readonly SELF_TEST_COUNT=5   # extractor, path-filter, branch-filter, config, ci-target-floor
```

**`assert_self_tests_ran <want>`** — compares and calls `fail()` on mismatch, with a message
naming the likely cause (a deleted invocation) and pointing at `run_self_tests`.

**`run_self_tests`** — resets the counter, calls the five self-tests, asserts the total.

**`ci_target_floor_verdict <ci-yml-path>`** — mirrors the existing `pattern_verdict` /
`config_verdict` shape, emitting verdicts on stdout for the call site to turn into `fail()` calls:

| Verdict | Meaning |
|---|---|
| *(empty)* | every floor entry present |
| `no-array` | zero or more than one single-line `T=(` in the file |
| `missing <entry>` | the array parsed, `<entry>` is not among its tokens |

`no-array` is a failure, never a skip — a `T` array reformatted across lines is exactly the
condition under which this check would otherwise stop asserting anything, which is the defect class
the whole gate exists to prevent.

**`ci_target_floor_self_test`** — the fifth self-test. Fixture table with a control pair: a
synthetic `T=(…)` containing `:affected-smoke` must yield no verdict; one without it must yield
`missing :affected-smoke`; a file with no `T=(` line and one with two must both yield `no-array`.

**Check 8 (production call site)** — runs `ci_target_floor_verdict` against the real
`.github/workflows/ci.yml` and converts each verdict into a `fail()` with remediation text. Full
gate only, matching how checks 5/6 relate to `path_filter_self_test`.

```bash
T_FLOOR=(':affected-smoke')
```

with the comment recording D8 — why `:actionlint` is deliberately absent.

**`selftest_mutation_battery` (check 9)** — per D5/D6. For each of the five invocation lines inside
`run_self_tests`, build a mutant with that line and the battery's own invocation removed, assert
exactly two lines went, run `bash <mutant> --self-test`, and require a non-zero exit. Then run the
control with only the battery line removed and require exit 0.

Mutants are written under `mktemp -d` and removed on the way out. `run.sh` `cd`s to the repo root
on entry, so a mutant executed from a temp path still resolves every relative input.

**`usage()`** — updated: `--self-test` now runs five fixture tables plus the mutation battery.

### `ci/affected-graph/ci_targets.py`

New constant, whole-line matched per §2.4:

```python
ACTIONLINT_SH_CALL_SITES = ("run_self_tests", "selftest_mutation_battery")
```

`run_self_tests` is a strict substring of its own definition line `run_self_tests() {`, which is
the `SELF_SCHEDULED_GATES` trap verbatim — so a substring test would survive deleting the call.
Whole-line matching is what distinguishes them.

`check_self_invocation` gains a third parameter for `ci/actionlint/run.sh`'s text, checked against
its own required set rather than a shared haystack. `main()` reads the file through `read_input`,
so a missing file routes to rc 1 (an authorial mistake) rather than aborting the whole guard with
rc 2.

The `--self-test` block gains synthetic-mutation cases in the established `wired.replace(...)`
style: a wired tree must not fire; deleting each of the two lines must fire; and — the case the
constant exists for — a tree retaining only the **definition** `run_self_tests() {` must fire.

The `RUN_SH_CALL_SITES` comment naming SMA-542 as the general fix for its residual is updated to
say what actually shipped.

The failure message extends the existing *"restore the exact line; see RUN_SH_CALL_SITES and
SELF_SCHEDULED_GATES"* text to name the new constant.

### `ci/actionlint/README.md`

- The check table gains rows 8 and 9; row 7 becomes five self-tests.
- The "Running it" section notes that `--self-test` now also runs the mutation battery.
- A new **Limitations** section replaces the four-line residual with what remains true (§6).
- The cost table gains the measured battery overhead.

## 5. Testing

| # | Assertion | How |
|---|---|---|
| T1 | Each of the five invocations, deleted, reds the gate | Check 9, standing, in CI |
| T2 | An unmutated tree exits 0 | Check 9's control (D6) |
| T3 | `assert_self_tests_ran` deleted → every mutant survives → red | Manual mutation, recorded here and in the PR |
| T4 | `run_self_tests` / `selftest_mutation_battery` call deleted → C4 reds | `ci_targets.py --self-test`, standing |
| T5 | `run_self_tests() {` definition alone does not satisfy C4 | `ci_targets.py --self-test`, standing |
| T6 | `:affected-smoke` removed from `T` → check 8 reds | `ci_target_floor_self_test`, standing; plus one manual edit of the real `ci.yml` |
| T7 | `T` reformatted across lines → `no-array` failure, not a skip | `ci_target_floor_self_test`, standing |
| T8 | `:actionlint` removed from `T` → C1 reds | Manual, confirms §2.3's premise on the real file |
| T9 | AC-2's extraction table unchanged | Direct comparison against §2.5 |
| T10 | Full graph green | `moon ci …` per CLAUDE.md's marker block |

T3, T6 and T8 are one-off manual mutations by construction — they mutate the very mechanisms the
standing controls run through. Their results go in the PR body.

The battery's wall-clock cost is measured against `README.md`'s existing baseline (~1.0s
standalone, ~11.6s through Moon) and the table updated. Six extra `bash` subprocesses each running
five fixture tables is the expected cost; if it materially changes the through-Moon figure, the
README says so rather than leaving the old number standing.

## 6. Limitations

**L1 — Deleting both `T` entries in one edit.** Removing `:affected-smoke` *and* `:actionlint`
from `T=(…)` together silences both halves of the cycle: neither gate runs, so neither complains.
This is inherent — two independently-scheduled gates are the most the graph offers, and a third
would only move the pair to a triple. It is bounded: `moon ci`'s target list is a single, short,
reviewed line, and the deletion has no plausible innocent cause.

**L2 — Deleting `run_self_tests`' *body*.** The counter asserts that five self-tests ran; C4
asserts the block is invoked. Replacing the function body wholesale with `:` would red — the count
would be 0 — but replacing `SELF_TEST_COUNT` with `0` at the same time would not. Two coordinated
edits in one file, both of which read as deliberate sabotage in review. Not closed.

**L3 — The battery proves invocation, not correctness.** A self-test whose fixtures were
weakened still runs, still increments, and still passes. That is check 7's own tables' job, and
they have their own control pairs; the battery is orthogonal to it.

**L4 — `.git` state remains outside Moon's input hash.** Unchanged by this issue, and stated at
length on the `actionlint:` task in `moon.yml` (SMA-540 L9). The `T` floor reads a tracked file, so
it is not affected; check 5's branch half still is.

## 7. Acceptance criteria

1. Deleting or bypassing any self-test invocation in `ci/actionlint/run.sh` causes a failure
   rather than a silent pass, proven by mutation rather than asserted — check 9, standing in CI,
   with a control pair (D6).
2. The real-file extraction counts are unchanged, per §2.5's table. AC-2's stated figures are the
   `paths` family of that table and hold exactly.
3. `ci/actionlint/README.md`'s stated limitation is replaced by §6.
4. Additionally, from the issue's 2026-08-19 comment: removing `:affected-smoke` from `ci.yml`'s
   `T=(…)` array reds `repo:actionlint`, closing SMA-541's L6.
