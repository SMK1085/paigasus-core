# SMA-542 — Guard the actionlint gate's self-test invocations from deletion

**Status:** revised after adversarial review (2026-08-20)
**Linear:** [SMA-542](https://linear.app/smaschek/issue/SMA-542/repo-actionlint-gate-guard-the-self-test-invocations-from-deletion)
**Related:** SMA-525 (`repo:actionlint`, whose mutation battery found this as the sole survivor),
SMA-540 (`branches:` half of check 5, which added the third and fourth self-tests), SMA-541
(`ci_targets.py`, whose C1/C4/C5 are the other half of the cycle below, and whose limitation L6
this issue absorbs), SMA-553 (`repo:input-liveness`, the most recent extension of C4)

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
`branch_filter_self_test`'s header (`run.sh:1382-1383`) records the principle this violates, from
SMA-525's finding F4: *a one-off mutation battery is not a standing control.*

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
`branch_filter_self_test` (`:1385`) and `config_self_test` (`:1551-1602`) each accumulate a local
`rc` **and** call `fail()`, which sets the global `FAILED=1`. The tail invocations (`:1868-1871`)
discard the return values; propagation is entirely through the global. Removing a call therefore
removes its verdict outright — there is no residual signal.

### 2.2 The invocations appear twice

The four calls exist in two places: the `--self-test` case arm (`run.sh:1611-1614`) and the tail
(`:1868-1871`). The blocks are not byte-identical — the arm is indented four spaces — but
`check_self_invocation` compares **stripped** lines (`ci_targets.py:553`), so both collapse to the
same token. Deleting one leaves the other matching, and an external pin cannot tell a wired file
from a half-wired one. D2 collapses them to a single call site.

### 2.3 C1 already covers `:actionlint`

`check_forward` (`ci_targets.py:397-417`) computes `want = eligible - set(exempt)` over the `repo`
project's CI-eligible tasks and reports `sorted(want - got)` as `missing` — **strict equality**, not
a superset test. `T_EXEMPT` is empty (`ci_targets.py:144`) and `repo:actionlint` carries no
`runInCI: false` (`moon.yml:481-511`), so dropping `:actionlint` from `T` already reds
`repo:affected-smoke`.

This is load-bearing for the design and it inverts part of the issue comment's suggestion: an
assertion inside `run.sh` that `:actionlint` is present in `T` would be **vacuous**, because the
only run in which the entry is missing is the run in which the assertion does not execute. It is
deliberately excluded from the floor in D8, with that reasoning recorded at the site.

It also means `README.md:86-88`'s escape hatch — *"drop `:actionlint` from `T=(…)`"* — is already
wrong, and has been since SMA-541 shipped: C1 reds unless a `T_EXEMPT` entry is added at the same
time. §4 corrects it.

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
three conventions, including a **swap case in both directions**, which the existing self-test
already carries (`ci_targets.py:980-985`).

`RUN_SH_CALL_SITES`' own comment names this issue as the general fix for its residual: *"A PARTIAL
mitigation, not a closure: deleting the `assert_ci_targets` call removes C4 along with it.
SMA-542 is the general fix for this class (spec L6)."*

### 2.5 AC-2's baseline numbers predate SMA-540

AC-2 pins the real-file extraction counts as `prebuild.yml` 2 KEY / 13 ITEM, `security-scan.yml`
1 KEY / 5 ITEM, `ci.yml` none. Measured on `a607ce0`, `extract_filter_keys` reports:

| File | `paths` KEY / ITEM | `branches` KEY / ITEM | Total KEY / ITEM |
|---|---|---|---|
| `ci.yml` | 0 / 0 | 2 / 2 | 2 / 2 |
| `prebuild.yml` | 2 / 13 | 2 / 2 | 4 / 15 |
| `security-scan.yml` | 1 / 5 | 1 / 1 | 2 / 6 |

The AC's numbers match the **`paths` family exactly**. The `branches` rows arrived with SMA-540,
after the AC was written. AC-2 is therefore satisfied by holding this whole table constant, and the
table is recorded here so a future reader does not misread the totals as a regression.

### 2.6 `repo:affected-smoke` cannot currently see `ci/actionlint/`

This is why the first draft's C4 extension would have been a no-op on its own threat model, and it
is the single most important finding of the adversarial review.

`repo:affected-smoke`'s `inputs` (`moon.yml:130-155`) list `ci/affected-graph/**/*`,
`.github/workflows/ci.yml`, `.moon/**/*`, the `moon.yml`s, the manifests, `CLAUDE.md` and
`.prototools`. **`ci/actionlint/**` is not among them.** A PR whose only change is deleting the two
pinned lines from `ci/actionlint/run.sh` therefore leaves `repo:affected-smoke` unaffected;
`moon ci --base origin/main` never schedules it; C4 never runs. Meanwhile `repo:actionlint` *is*
scheduled — its `inputs: ['**/*']` covers the file — and now asserts nothing. The deletion ships
green, reproducing the exact defect the fix exists to prevent.

The precedent for the fix sits four lines below the gap, in the same list. `CLAUDE.md` was added
for the identical reason, with the note: *"(repo:actionlint's `**/*` already covers CLAUDE.md, but
that is a different task and cannot green this one.)"*

Adding `ci/actionlint/**/*` re-baselines nothing: no cascade case in `ci/affected-graph/run.sh`
keys on any `ci/` path (verified — zero matches), and `SELF_TASK_EXPECTED_GLOBS`
(`ci_targets.py:155`) pins only `input-liveness`'s inputs, not `affected-smoke`'s.

### 2.7 `moon ci`'s failure can be swallowed without touching `T`

`ci_targets.py` C5 (`check_invocation`, `:468`) already asserts that `T` is *handed to* `moon ci`,
via `T_ARRAY_EXPANSION = '"${T[@]}"'` matched per invocation line. It does not assert that the
invocation's **failure propagates**. Appending `|| true` to `ci.yml:218`:

```bash
moon ci "${T[@]}" --base origin/main --include-relations || true
```

leaves `T` untouched (C1/C2/C3 green), leaves the expansion intact (C5 green), and `set -euo
pipefail` at `ci.yml:216` does not help — the step exits 0. Every gate in `T` is silenced by one
edit, including `repo:affected-smoke`'s own red. Nothing in the repo catches it today.

This is the *suppression* spelling of L6, as against the *removal* spelling in §1.1. It is one
line away from the array check 8 already reads, so D14 closes both rather than claiming AC-4
against half of it.

## 3. Design decisions

### D1 — A cycle of independently-scheduled gates, not a chain

```
ci.yml   T=( … :actionlint … :affected-smoke … )     moon ci "${T[@]}" … (no `|| true`)
            ▲                          ▲                        ▲
            │ C1 check_forward         │ check 8 — T_FLOOR      │ check 8 — D14
            │ (strict equality)        │ (':affected-smoke')    │
            │                          │                        │
     repo:affected-smoke ────────────► repo:actionlint ◄────────┘
            │   C4 pins run.sh's two call-site lines (NEW)
            │   moon.yml gains 'ci/actionlint/**/*' so this is reachable (§2.6)
            ▼
      ci_targets.py
```

Each gate asserts the *other's* continued scheduling. Neither is the sole judge of its own
configuration — the principle `repo:input-liveness` already applies to its own `inputs`.

Rejected: a linear chain (actionlint guards affected-graph, nothing guards actionlint). It moves
the hole rather than closing it.

### D2 — One call site for the self-tests; the battery sits after the early exit

```bash
SELF_TEST_ONLY=0
case "$#:${1:-}" in
  '0:')            ;;
  '1:--self-test') SELF_TEST_ONLY=1 ;;
  *)               usage ;;
esac

run_self_tests                                    # check 7 — ONE call site, both modes
if [ "$SELF_TEST_ONLY" = 1 ]; then exit "$FAILED"; fi

ci_target_floor_check                             # check 8 — reads a tracked file, needs no binary
command -v actionlint >/dev/null 2>&1 || infra "actionlint not on PATH — run 'proto install actionlint'"

# … checks 1-6 …

selftest_mutation_battery                         # check 9 — full gate only
exit "$FAILED"
```

Four properties, in order of importance:

1. **One line to pin.** §2.2's ambiguity disappears; C4 gets an unambiguous target.
2. **The battery cannot recurse.** Mutants are invoked with `--self-test`, which exits before
   check 9 is reached. This is structural, not a guard that can be removed — and it is why D5 needs
   no two-line deletion and no `PAIGASUS_ACTIONLINT_MUTANT` bypass env var.
3. **`--self-test` stays fast and binary-free.** It runs the six tables and nothing else, which is
   what `README.md:125` advertises it as. The call sits ahead of the PATH guard, preserving the
   property that guard's comment exists to protect.
4. **Self-tests run first.** Matching the convention `moon.yml` already states for
   `repo:affected-smoke`, `repo:publish-metadata` and `repo:error-code-single-site` — *"a rotted
   checker must red rather than ship green"*.

Check 8 is placed **before** the PATH guard deliberately: it reads a tracked file and needs no
binary, so a machine without `actionlint` should still get its verdict rather than infra-exiting
first.

Check **numbers are not renumbered**. They are logical identities, not execution order — checks 5
and 6 are already defined ahead of check 1. Check 7 remains "the self-tests"; the new work is
checks 8 and 9. All six self-test functions are defined earlier in the file, each ahead of the
single dispatch call site (the `case "$#:${1:-}"` block that sets `SELF_TEST_ONLY` and calls
`run_self_tests`, near the bottom of the script), so the single call site can reach every one.

**Accepted cost of running self-tests first (adversarial review, MAJOR).** `branch_filter_self_test`
asserts its `origin/main` precondition unconditionally (`run.sh:1391-1392`) and exits 2 if it is
missing. Today that happens *after* checks 1–6 have reported; under D2 it happens before
`actionlint` is ever invoked, so on a `--depth 1` / `--single-branch` checkout a developer loses
the checks 1–6 findings they would previously have seen. This is a deliberate trade, not an
oversight:

- The comment at `run.sh:1186-1191` already concedes the full run is not ref-free — *"a checkout
  without it still exits 2"*. D2 changes *when*, not *whether*.
- Learning about a failed precondition immediately, rather than after the linter runs, is the
  better failure mode, and `README.md` already gives the one-command recovery
  (`git fetch origin +refs/heads/main:refs/remotes/origin/main`).
- The full gate is ~1.5s, so re-running after the fetch costs nothing.

The lazy-canary comment at `run.sh:1186-1191` states the old ordering as a design property and
**must be updated**, along with the README's "Running it" note. A stale comment asserting a
now-false invariant is exactly the rot this gate exists to prevent.

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

`SELF_TEST_COUNT` is a plain variable, **not `readonly`**. Under `set -uo pipefail` without
`set -e` a reassignment prints an error and continues, so `readonly` buys no protection; its only
real effect would be to break a future harness that sources this file twice.

### D4 — What covers `assert_self_tests_ran`, and the deliberate redundancy

Deleting the assertion line inside `run_self_tests` is caught by check 9: every mutant would then
exit 0 and the battery reports *"mutant survived"*. Deleting the battery's invocation is caught by
C4. Deleting **both** is still caught by C4, because the battery line is gone.

So C4 pins exactly two lines — `run_self_tests` and `selftest_mutation_battery` — and the third is
covered by a control that cannot be satisfied vacuously. Pinning it as well would add brittleness
against reformatting for no coverage.

**The redundancy is intentional and worth stating** (adversarial review, QUESTION): check 9 *also*
covers a deleted `run_self_tests` call — with it gone, every mutant's `--self-test` run asserts
nothing and exits 0, so all six survive and the battery reds. That means the primary AC-1
mechanism does not depend solely on C4 being reachable, which is a useful second line of defence
behind the `moon.yml` input fix in §2.6.

### D5 — Mutants cannot recurse, by construction

Per D2, check 9 runs only on the full-gate path, and mutants are invoked with `--self-test`, which
exits before check 9. A mutant therefore never re-enters the battery. Each mutant is built by
deleting **exactly one** line — the targeted self-test invocation — with no need to also remove the
battery's own call.

Rejected: a `PAIGASUS_ACTIONLINT_MUTANT=1` recursion guard. It would be a live bypass surface in a
gate whose premise is that it cannot be silently switched off, and D2 makes it unnecessary.

The battery validates **all six preconditions before creating any subprocess**: each `sed` must
remove exactly one line. A reformatted or duplicated invocation therefore reds with a precise
message rather than producing a mutant byte-identical to the original (which would exit 0 and be
misreported as a survivor). `cp` and `sed` failures are checked explicitly, not assumed — the
script runs without `set -e`.

The `sed` address is **anchored to the exact indented call line** inside `run_self_tests`
(`/^  extractor_self_test$/d`), never a bare name match: the bare name also appears on the
definition line and in six comments, and a loose pattern would either over-delete or leave an
orphaned function body that dies of a bash syntax error — killed for the wrong reason.

### D6 — The control is the real script, unmutated

Six mutants that all fire cannot distinguish a working battery from a stuck one — SMA-466's
lesson, and the reason `branch_filter_self_test` carries both directions. Check 9 therefore also
runs `bash "$SELF_SRC" --self-test` and requires exit **0**.

Because the battery is full-gate-only (D2), the control needs **no `sed` and no temp copy at all**
— it is the unmodified file. This removes the first draft's unbounded-recursion hazard, which the
adversarial review correctly flagged as a BLOCKER: a control built by deleting the battery line
would recurse forever if that `sed` ever matched zero lines.

The control is not redundant with the outer `run_self_tests` call. It proves the *harness* — the
`bash` invocation, the working directory, the argument passing — yields 0 on a healthy tree. A
battery whose subprocess invocation was broken would otherwise report six dead mutants and read as
maximally healthy.

### D7 — The `T` floor lives in bash, inside `run.sh`

`ci_targets.py` already parses `T=(…)` and has self-tests for it, so reusing it would be
single-site. Rejected: it would couple the gate that exists to survive affected-graph's compromise
to a file in affected-graph's own directory, and that independence is the property the design is
buying. The floor check is ~15 lines of tokenisation against a single-line array — below the
threshold where duplication costs more than the independence it buys.

Divergence between the two parsers is contained by **check 8's own behaviour**, not by appeal to
C3 (which lives in the gate check 8 exists to guard, and so cannot be leaned on here): check 8
fails loudly on anything it cannot parse as exactly one single-line `T=(…)` array. There is no
input on which it silently asserts nothing.

### D8 — The floor contains `:affected-smoke` only

Per §2.3, `:actionlint` is covered by C1 and would be vacuous here. `:promtool` and
`:publish-metadata` are in `REQUIRED_REPO_TASKS`, which the floor keeps alive by keeping
`:affected-smoke` scheduled — restating them would be redundant. The floor is the minimal set the
rest of the graph cannot assert about itself: one entry.

### D9 — One `moon.yml` line, and no new Moon task

`repo:affected-smoke` gains `ci/actionlint/**/*` in its `inputs` (§2.6). That is the whole of the
`moon.yml` change and it is not optional — without it the C4 extension is unreachable.

Nothing is added to `ci.yml`'s `T=(…)`, so CLAUDE.md's `ci-targets` marker block is untouched and
`repo:affected-smoke`'s C3 has nothing new to check. `repo:input-liveness` will assert the new glob
matches tracked files; it matches `run.sh` and `README.md`.

### D10 — The kill criterion is `rc == 1` plus the counter's own message

"Non-zero exit" is not a kill. `run.sh` has three non-zero exits: `fail()` → `exit "$FAILED"` = 1;
`infra()` → `exit 2` (`run.sh:34-37`); and `no_origin_main_infra` → `exit 2` from inside
`branch_filter_self_test` itself (`run.sh:1392`). A mutant dying at exit 2 for a transient reason —
a `git for-each-ref` hiccup, a `mktemp` failure, exit 127 from a missing file — would be scored
"correctly killed", and the battery is AC-1's sole standing proof.

A mutant counts as killed only when **`rc == 1` and its captured stderr contains
`assert_self_tests_ran`'s distinctive message**. `rc == 2` is reported as its own outcome —
*"mutant aborted before reaching the assertion"* — and fails the battery rather than passing it.

### D11 — The mutation source is captured before the `cd`

`run.sh` does `cd "$(git rev-parse --show-toplevel)"` on entry (`:25`). `$0` is therefore unsafe:
under `cd ci/actionlint && ./run.sh` it is `./run.sh`, which no longer resolves after the `cd`.
Without `set -e` the `cp` would fail silently, `bash <missing>` would exit 127, and all six
mutants would score as killed — a false pass on AC-1.

The absolute path is captured **before** the `cd`:

```bash
SELF_SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
```

### D12 — Mutants run in parallel, with captured output

Measured on `a607ce0`: `--self-test` is ~1.0s, of which `extractor_self_test` alone is 0.67s (it
forks `mktemp` per fixture); the full gate is ~1.5s. Six sequential subprocesses would add ~6s —
4× the gate's standalone cost, on the required check's critical path.

The six runs are independent, so they are spawned concurrently and collected by PID. Wall cost
falls to roughly one subprocess (~1.2s), taking the full gate to ~3s standalone. Determinism is
preserved: each PID is waited on individually and mapped to its label, so results do not depend on
completion order.

Each mutant's stdout and stderr go to its own file. On the expected outcome they are **discarded**;
on an unexpected one they are re-emitted **prefixed** (`check 9 [mutant extractor_self_test]: …`).
Without this, mutant output is indistinguishable from the real gate's — every line is prefixed
`actionlint gate:` (`run.sh:30`) — and under `.moon/tasks.yml`'s `buffer-only-failure` five
mutants' worth of noise would surface on any unrelated failure of this task, exactly when the log
most needs to be readable.

The temp directory is removed via `trap 'rm -rf "$dir"' EXIT`, not a best-effort tail call, so an
`infra` exit between creation and cleanup does not leak it.

**Kept at five mutants rather than one.** The adversarial review argued that all five exercise the
single arithmetic mechanism `SELF_TESTS_RAN != SELF_TEST_COUNT`, so four of them add no
discriminating power over the free textual preconditions, and proposed one mutant plus five
preconditions (2 subprocesses). The counter-argument, and the reason five stay: AC-1 is written as
*"deleting or bypassing **any** self-test invocation"*, and inferring four cases from one is
precisely the inference this repo's gates decline to make elsewhere — `run_task_case`'s strict
equality lists every crate rather than sampling one. Parallelism reduces the disagreement to about
one wall-second, which is a cheap price for an assertion that matches its AC literally.

**Superseded by the fix wave (§9).** This five-vs-one debate predates M3, which added a sixth
self-test (`kill_predicate_self_test`) for an unrelated reason (T3's kill-predicate coverage). The
battery now runs six mutants — seven concurrent `--self-test` subprocesses including the control —
and the parallelism argument above is unaffected by the extra mutant. Current measured cost is in
§9's M3 entry and §5's cost-ceiling addendum.

### D13 — A self-test that is never wired is caught too

The counter proves six invocations ran; it cannot notice a *seventh* fixture table added tomorrow
and never called. `SELF_TESTS_RAN` would still equal `SELF_TEST_COUNT`, the battery only mutates
lines already inside `run_self_tests`, and C4 pins only the two call-site lines. That is this
issue's own defect class, one step out — and adding a table is the highest-probability future edit.

`run_self_tests` therefore also asserts that the number of `*_self_test` **definitions** in
`$SELF_SRC` equals `SELF_TEST_COUNT`:

```bash
grep -cE '^(function[[:blank:]]+)?[a-z_]+_self_test([[:blank:]]*\(\))?[[:blank:]]*\{' "$SELF_SRC"
# must equal SELF_TEST_COUNT
```

Broadened during the fix wave (M3/M8, §9) to also count the `function name {` keyword form and a
space before the parens — a table written either way must still be counted, or this check's own
hole reopens for the style it does not recognise.

Adding a table without wiring it reds; so does deleting a table's definition without decrementing
the count. This also closes the first draft's L2 escape (see §6).

### D14 — Check 8 also asserts `moon ci`'s failure is not swallowed

Per §2.7, `|| true` on the `moon ci` line silences every gate in `T` while leaving `T` itself
correct, and no existing check sees it. Check 8 is already reading `ci.yml`, so it also asserts
that no line whose first token is `moon` carries a `||`, `&&`, `;` or `|` tail after the command.

This is complementary to C5, not a duplicate: C5 asserts the array is *handed over*; D14 asserts
the result is *propagated*. They live in different gates on purpose, and D14 is the half that
survives `repo:affected-smoke` being silenced.

Without it, AC-4's claim to close SMA-541's L6 would cover the removal spelling and not the
suppression spelling — an overclaim the adversarial review caught.

## 4. Components

### `ci/actionlint/run.sh`

**Self path and counter state**, declared before the `cd` (`SELF_SRC`) and alongside `FAILED`:

```bash
SELF_SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
…
SELF_TESTS_RAN=0
SELF_TEST_COUNT=6   # extractor, path-filter, branch-filter, config, ci-target-floor, kill-predicate
```

(`SELF_TEST_COUNT` was 5 as originally shipped; the sixth entry, `kill-predicate`, was added by the
fix wave below closing T3 — see §9.)

**`assert_self_tests_ran <want>`** — compares and calls `fail()` on mismatch with a distinctive,
greppable message (D10 matches on it) naming the likely cause and pointing at `run_self_tests`.

**`run_self_tests`** — resets the counter, calls the six self-tests, asserts the total (D3),
asserts the definition count (D13), and resets `ORIGIN_REFS_LOADED=0`.

That last line is not incidental. `branch_filter_self_test` deliberately poisons `ORIGIN_REFS` with
a `main`-free list and forces `ORIGIN_REFS_LOADED=1` to reach the `no-origin-main` token
(`run.sh:1438-1444`), restoring both immediately. Today those poisons are harmless because the
self-tests run *last*, after checks 5/6 have finished with the cache. Under D2 they run *first*,
and `load_origin_refs` early-returns on `ORIGIN_REFS_LOADED=1` (`run.sh:1093`) — so a future botched
restore would silently feed checks 5/6 a fake ref list, turning every real `branches:` entry into a
false `unresolved` or an infra exit. Resetting the flag makes checks 5/6 independent of the
fixture's bookkeeping rather than trusting it.

**`ci_target_floor_verdict <ci-yml-path>`** — mirrors the existing `pattern_verdict` /
`config_verdict` decide-here-print-there split, which is what makes check 8 testable by a fixture
table at all (`run.sh:994-996` records that lesson):

| Verdict | Meaning |
|---|---|
| *(empty)* | every floor entry present, every `moon` line propagates |
| `no-file` | `.github/workflows/ci.yml` does not exist |
| `no-array` | zero, or more than one, single-line `T=(…)` |
| `missing <entry>` | the array parsed; `<entry>` is not among its tokens |
| `continued <lineno>` | a `moon` command line is continued onto another physical line (fix wave, §9, I2) |
| `swallowed <lineno>` | a `moon` command line carries a `\|\|`, `&&`, `;` or `\|` tail (D14) |
| `continue-on-error <lineno>` | a step's `continue-on-error:` value is not literally `false` |

`no-file` is distinct from `no-array` so the remediation text is not the misleading *"keep `T` on
one line"* when the real problem is a renamed workflow. `no-array` is a failure, never a skip — a
`T` array reformatted across lines is exactly the condition under which this check would otherwise
stop asserting anything. `continued` is the same shape: never a skip, and checked *before*
`swallowed` on the same matched line, because a wrapped invocation hides any real tail from the
line-at-a-time scan below it — reporting `swallowed` there would name a problem this check cannot
actually confirm (fix wave I2, §9).

The array regex is anchored `^[ \t]*T=\((.*?)\)[ \t]*$`, mirroring `T_ARRAY_RE`
(`ci_targets.py:66`) rather than a bare `T=(` (which would also match `EXPECT=(`). Membership
compares **whole whitespace-separated tokens**, since `:affected-smoke` is a prefix of a
hypothetical `:affected-smoke-disabled`.

**`ci_target_floor_self_test`** — the fifth self-test as originally shipped. Fixture table with
control pairs: an array containing `:affected-smoke` yields no verdict; one without it yields
`missing :affected-smoke`; a text with no `T=(` line and one with two both yield `no-array`; a
missing path yields `no-file`; a backslash-continued `moon` invocation yields `continued <lineno>`
and is not misread as `swallowed` (fix wave I2); a `moon ci "${T[@]}" … || true` line on ONE
physical line yields `swallowed`, and the real if/elif/else form (three separate single-line `moon`
invocations, matching `ci.yml`) does not.

**Check 8 (production call site)** — runs `ci_target_floor_verdict` against the real
`.github/workflows/ci.yml` and converts each verdict into a `fail()` with remediation text.

```bash
T_FLOOR=(':affected-smoke')
```

with the comment recording D8 — why `:actionlint` is deliberately absent.

**`SWALLOWED_SKIP`** (fix wave, §9, M6) — a `COE_SKIP`-shaped escape hatch, keyed the identical
`"<lineno>:<exact text>"` way, for a `moon` line `swallowed` cannot know is harmless (a diagnostic
pipe in an unrelated job, say). `continued` deliberately has no equivalent hatch — see the verdict
table note above.

**`mutant_is_killed <rc> <outfile>`** (fix wave, §9, M3 / spec T3) — the kill predicate, extracted
out of `selftest_mutation_battery`'s collection loop so `kill_predicate_self_test` (the sixth
self-test) can drive it directly against synthetic `(rc, output)` pairs: rc 1 with the counter's
message is a kill; rc 2/126/127, and rc 1 without the message, are not.

**`selftest_mutation_battery` (check 9)** — per D5/D6/D10/D11/D12. Validate all six preconditions,
then spawn six mutants plus the unmutated control concurrently, collect by PID, and require
`rc == 1` with the counter's message for each mutant and `rc == 0` for the control — via
`mutant_is_killed`, not an inline comparison, since the fix wave.

**`usage()`** — `--self-test` still runs the fixture tables only; the count changes from four
(originally) to five (as shipped) to six (fix wave). The mutation battery is explicitly *not* part
of it.

**Comment at `run.sh:1186-1191`** — updated for D2's reordering, which makes its stated lazy-canary
property false.

### `ci/affected-graph/ci_targets.py`

New constant, whole-line matched per §2.4. Originally two entries; the fix wave (§9, I1) added a
third, `done < <(ci_target_floor_verdict .github/workflows/ci.yml)` — check 8's own production call
site, which the reviewer found deletable with the full gate still exiting 0 and this check still
reporting PASS:

```python
ACTIONLINT_SH_CALL_SITES = (
    "run_self_tests",
    "selftest_mutation_battery",
    "done < <(ci_target_floor_verdict .github/workflows/ci.yml)",
)
```

`run_self_tests` is a strict substring of its own definition line `run_self_tests() {`, which is
the `SELF_SCHEDULED_GATES` trap verbatim — a substring test would survive deleting the call.
Whole-line matching distinguishes them. The third entry has the identical property against
`ci_target_floor_verdict`'s own self-test call sites (`ci_target_floor_verdict "$tmp"` /
`ci_target_floor_verdict /nonexistent/ci.yml`) — neither is the whole `done < <(...)` line, so
whole-line matching keeps them apart too.

A comment records the **propagation contract**, which differs from `RUN_SH_CALL_SITES` and would
otherwise read as the hole §2.4 just closed: `run_self_tests` and `selftest_mutation_battery` carry
no `|| RC=1` suffix because propagation is through the global `FAILED`, as it already is for the
self-tests (`run.sh:29-32`); the `done < <(...)` entry has nothing to propagate at all — it is the
tail of a `while` loop whose body already calls `fail()` per verdict. It also notes the
consequence — a future reformat of any of the three lines would red C4 even though harmless (see §6
L3).

`check_self_invocation` gains a **required third positional parameter** for
`ci/actionlint/run.sh`'s text, checked against its own required set rather than a shared haystack.
All twelve existing call sites in `self_test()` are updated to pass a wired actionlint text; an
optional parameter defaulting to `""` is rejected, because a default that vacuously passes
re-creates the hole being fixed.

`main()` reads the file through `read_input`, so a missing file routes to rc 1 (an authorial
mistake) rather than aborting the whole guard with rc 2.

The `--self-test` block gains synthetic-mutation cases in the established `wired.replace(...)`
style: a wired tree must not fire; deleting each of the (now three) lines must fire; a tree
retaining only the definition `run_self_tests() {` must fire; and **both swap directions** must
fire — an actionlint call site in the affected-graph text, and vice versa.

The `RUN_SH_CALL_SITES` comment naming SMA-542 as the general fix for its residual is updated to
say what shipped. The failure message extends the existing *"restore the exact line; see
RUN_SH_CALL_SITES and SELF_SCHEDULED_GATES"* text to name the new constant.

### `moon.yml`

`repo:affected-smoke`'s `inputs` gains `'ci/actionlint/**/*'`, with a comment mirroring the
`CLAUDE.md` entry four lines below it: this task now asserts call sites inside `ci/actionlint/`, so
a change there must re-key it, and `repo:actionlint`'s own `**/*` cannot green a different task.

### `ci/actionlint/README.md`

- The check table (`README.md:36-46`) gains rows 8 and 9; row 7 becomes five self-tests.
- "Running it" (`README.md:120-140`) notes that `--self-test` runs five tables and that the battery
  is full-gate-only, and reflects D2's reordering in the shallow-clone paragraph.
- A **new Limitations section** is added carrying §6. The first draft claimed to "replace the
  four-line residual"; there is no such section and no such text — `README.md:17`'s only use of
  "limitation" refers to SMA-525's L5, which SMA-540 closed.
- `README.md:86-88`'s escape hatch is corrected: dropping `:actionlint` from `T=(…)` now also
  requires a `T_EXEMPT` entry in `ci_targets.py`, or C1 reds (§2.3).
- The cost table gains the measured battery overhead.

## 5. Testing

| # | Assertion | How |
|---|---|---|
| T1 | Each of the six invocations, deleted, reds the gate | Check 9, standing, in CI |
| T2 | An unmutated tree exits 0 | Check 9's control (D6) |
| T3 | A mutant dying at rc 2 (or 126/127, or rc 1 without the message) is not scored as killed | Implemented (fix wave, §9, M3): `kill_predicate_self_test`, the sixth self-test, drives `mutant_is_killed` directly against synthetic `(rc, output)` pairs — `--self-test`, standing |
| T4 | `assert_self_tests_ran` deleted → all mutants survive → red | Manual mutation, recorded in the PR |
| T5 | `run_self_tests` / `selftest_mutation_battery` / the check-8 production call site (fix wave, I1) call deleted → C4 reds | `ci_targets.py --self-test`, standing |
| T6 | `run_self_tests() {` definition alone does not satisfy C4 | `ci_targets.py --self-test`, standing |
| T7 | A call site in the wrong file does not satisfy C4 (both directions) | `ci_targets.py --self-test`, standing |
| T8 | An unwired seventh self-test reds | `--self-test` fixture over the definition count (D13) |
| T9 | `:affected-smoke` removed from `T` → check 8 reds | `ci_target_floor_self_test`, standing; plus one manual edit of the real `ci.yml` |
| T10 | `T` reformatted across lines → `no-array`; `ci.yml` renamed → `no-file` | `ci_target_floor_self_test`, standing |
| T11 | `\|\| true` on a single-line `moon ci` invocation → `swallowed` | `ci_target_floor_self_test`, standing (D14) |
| T12 | `:actionlint` removed from `T` → C1 reds | Manual, confirms §2.3's premise on the real file |
| T13 | Deleting the three call-site lines (fix wave: was two) schedules `repo:affected-smoke` | `moon query tasks --affected` after the `moon.yml` change (§2.6) |
| T14 | AC-2's extraction table unchanged | Direct comparison against §2.5 |
| T15 | Full graph green | `moon ci …` per CLAUDE.md's marker block |
| T16 | A backslash-continued `moon` invocation → `continued <lineno>`, never `swallowed`; the real unwrapped if/elif/else form does not fire | Added (fix wave, §9, I2): `ci_target_floor_self_test`, standing; plus one manual reproduction of the reviewer's exact scenario against a temp copy of the real `ci.yml` |
| T17 | `SWALLOWED_SKIP` silences an exact lineno+text match, does not leak to a different line, and a stale (lineno-only) match does not silence | Added (fix wave, §9, M6): `ci_target_floor_self_test`, standing, mirroring `COE_SKIP`'s three-row shape |
| T18 | Deleting the check-8 production call site (`done < <(ci_target_floor_verdict …)`) → full gate rc 0 and `ci_targets.py` PASS before the fix; `ci_targets.py` reports it after | Added (fix wave, §9, I1): manual deletion of the real block, confirmed red, restored |

T4, T9, T12, T16 and T18 are one-off manual mutations by construction — they mutate the very
mechanisms the standing controls run through, or (T16/T18) reproduce a reviewer finding directly on
the real files. Their results go in the PR body / fix-wave report.

**Cost ceiling (adversarial review, QUESTION) — SUPERSEDED during implementation (§8).** The
original ceiling required the full gate to stay **under 2× its current ~1.5s standalone** — i.e.
~3s — with a fallback to one mutant plus five textual preconditions (2 subprocesses) if the
measured figures missed it. The gate measures ~3.36s standalone (min-of-7), and the five-mutant
battery was kept as designed rather than falling back. Two reasons the ceiling itself was retired
rather than the design: it was 2× a *pre-change* baseline, but check 8 (the `T` floor plus the
swallowed/`continue-on-error` verdicts) legitimately grew the gate beyond what a bare self-test
counter would have cost, so "2× the old number" stopped being the right question once check 8
existed; and the battery's cost is parallel-bound rather than proportional to mutant count —
measured concurrent `--self-test` invocations: 1 → 1.09s, 2 → 1.21s, 6 → 2.07s, so a future sixth
mutant costs a fraction of a second, not a whole extra invocation. Moon's own per-task floor in
this repo is ~9–11s regardless of what a task does, so the gate — even at ~3.36s standalone — is
nowhere near the critical path. `ci/actionlint/README.md`'s cost table carries the current
measured figures for both the full gate and `--self-test`.

**Further superseded by the fix wave (§9).** T3's implementation added a sixth self-test, and I2
added the `continued` check, taking the battery to six mutants (seven concurrent `--self-test`
subprocesses including the control). Standalone cost is now ~4.11s full gate / ~1.26s `--self-test`
(min-of-7). Still nowhere near Moon's ~9–11s per-task floor, so the conclusion is unchanged —
`ci/actionlint/README.md`'s cost table carries the current figures.

## 6. Limitations

**L1 — Deleting both `T` entries in one edit.** Removing `:affected-smoke` *and* `:actionlint`
from `T=(…)` together silences both halves of the cycle: neither gate runs, so neither complains.
Inherent — two independently-scheduled gates are the most the graph offers, and a third would only
move the pair to a triple. Bounded: `moon ci`'s target list is a single, short, reviewed line, and
the deletion has no plausible innocent cause.

**L2 — Coordinated multi-line edits inside `run_self_tests`.** The counter (D3), the definition
count (D13) and the battery (D5) each red on a single-line change. Editing the body *and*
`SELF_TEST_COUNT` *and* the definitions consistently would pass — but that is no longer the
first draft's two-line escape, and every such edit reads as deliberate sabotage in review.

**L3 — The whole-line pin is brittle against reformatting.** A future `run_self_tests || FAILED=1`
reds C4 even though it is harmless (propagation is already via the global). This is the accepted
cost of the whole-line match D2 requires; the fix is to update the constant, and the failure
message says so.

**L4 — The battery proves invocation, not correctness.** A self-test whose fixtures were weakened
still runs, still increments, and still passes. That is check 7's own tables' job, and they carry
their own control pairs; the battery is orthogonal.

**L5 — `.git` state remains outside Moon's input hash.** Unchanged by this issue, stated at length
on the `actionlint:` task in `moon.yml` (SMA-540 L9). The `T` floor reads a tracked file, so it is
unaffected; check 5's branch half still is.

**L6 — The cycle is asymmetric (superseded during implementation, §8).** The original L6 said
`continue-on-error: true` was not covered by D14 and was worth a follow-up issue; check 8 now
covers it (any spelling but the literal `false`, with `COE_SKIP` as the escape hatch for an
unrelated later step). What replaces it: `repo:affected-smoke` pins `repo:actionlint`'s call
sites (`ACTIONLINT_SH_CALL_SITES`), but `repo:actionlint` pins only `:affected-smoke`'s *presence
in `T`* — i.e. its scheduling, not its own internal correctness. Deleting
`assert_ci_targets || SUITE_RC=1` from `ci/affected-graph/run.sh` therefore still removes that
half of the guard silently, with everything green: `repo:affected-smoke` keeps running and keeps
exiting 0, because the line that would have made it fail is gone. `ci_targets.py`'s
`RUN_SH_CALL_SITES` comment describes this residual accurately (SMA-542 review finding I2); it is
a deliberate, deferred follow-up, not something this change claims to close.

## 7. Acceptance criteria

1. Deleting or bypassing any self-test invocation in `ci/actionlint/run.sh` causes a failure rather
   than a silent pass, proven by mutation rather than asserted — check 9, standing in CI, with a
   control pair (D6) and a kill criterion that cannot be satisfied by an infra abort (D10).
2. The real-file extraction counts are unchanged, per §2.5's table. AC-2's stated figures are the
   `paths` family of that table and hold exactly.
3. `ci/actionlint/README.md` carries a Limitations section reflecting §6, and its
   now-incorrect `:actionlint` escape hatch (§2.3) is corrected.
4. From the issue's 2026-08-19 comment: both the **removal** spelling (`:affected-smoke` dropped
   from `T`) and the **suppression** spelling (`|| true` on the `moon ci` line) red
   `repo:actionlint`, closing SMA-541's L6 in both directions.
5. `repo:affected-smoke` is scheduled by a change to `ci/actionlint/**`, so the C4 extension is
   reachable on the PR that would trip it (§2.6).

## 8. Changelog — adversarial review (2026-08-20)

**Folded in.** The `moon.yml` input gap (§2.6) — a BLOCKER that made the C4 extension a no-op on
its own threat model. The control-recursion BLOCKER, resolved structurally by moving the battery
after the `--self-test` early exit (D2/D6), which also removed D5's two-line deletion and fixed the
`--self-test` cost regression. The kill criterion (D10), `$0` safety (D11), mutant output capture
and `trap` cleanup (D12), the `ORIGIN_REFS_LOADED` coupling and the lazy-canary comment (D2/§4),
the unwired-sixth-table hole (D13), the `|| true` suppression hole (D14), the required-third-
parameter and swap-case corrections to `check_self_invocation` (§4), the README's non-existent
"four-line residual" and already-broken escape hatch (§4), plus the `sed`/regex/whole-token
anchoring, the `no-file` verdict, dropping `readonly`, moving check 8 ahead of the PATH guard,
D7's circular fail-safe argument, and three factual slips (F4's location, `config_self_test`'s
end line, "byte-identical" in §2.2).

**Not folded in.** The proposal to cut the battery to one mutant plus five textual preconditions.
Reason in D12: AC-1 is written as *"any self-test invocation"*, and D12's parallelism reduces the
cost difference to about one wall-second. Recorded as the explicit fallback in §5's cost ceiling if
the measured figures miss it.

**Decided during implementation (task 6, 2026-08-20).** Two things this spec drafted as open
questions were settled once real code and real measurements existed, and both sections above have
been updated in place rather than left to silently drift from what shipped:

- §5's cost ceiling (2× the ~1.5s pre-change baseline, with the one-mutant fallback) was
  superseded rather than triggered. The gate measures ~3.36s standalone with the five-mutant
  battery kept as designed — the fallback was never invoked. See §5 for the reasoning: check 8
  legitimately grew the gate beyond what the pre-change baseline priced in, and the battery's cost
  is parallel-bound rather than proportional to mutant count.
- §6's L6 ("`continue-on-error: true` is not covered, worth a follow-up issue") is now false: check
  8 covers it directly, with `COE_SKIP` as the escape hatch. L6 was replaced with the residual that
  actually remains — the cycle's asymmetry (`repo:affected-smoke` pins `repo:actionlint`'s call
  sites, but not the reverse: `repo:actionlint` pins only `:affected-smoke`'s scheduling, not
  `repo:affected-smoke`'s own internal correctness) — which is the more important limitation this
  work leaves standing.

## 9. Changelog — fix wave (2026-08-20)

A final whole-branch code review, run before this branch was proposed for merge, found two
Important-severity findings (labelled I1/I2 in this section — **distinct** from the earlier
task-4b-review I1/I2 labels inside `run.sh`'s `continue-on-error` comments, and from the
cycle-asymmetry "review finding I2" cited in §6's L6, both of which predate this pass) and six
Minor findings (M3-M8). All eight were fixed in this wave; none were pushed back on.

**I1 — check 8's own production call site was unpinned.** The reviewer deleted the
`while IFS= read -r verdict … done < <(ci_target_floor_verdict .github/workflows/ci.yml)` block
from `run.sh` and measured: full gate rc 0, `ci_targets.py` PASS — check 8's entire T-floor/
`swallowed`/`continue-on-error` machinery asserted nothing, with `ACTIONLINT_SH_CALL_SITES`
pinning only `run_self_tests` and `selftest_mutation_battery`. This was the branch's own defect
class (an unpinned, deletable assertion) applied to the check the branch had just added. Fixed by
adding the block's distinctive final line, `done < <(ci_target_floor_verdict
.github/workflows/ci.yml)`, as a third `ACTIONLINT_SH_CALL_SITES` entry (whole-line matched, per
§4/§2.4), with a matching line in the `wired_actionlint` self-test fixture and a new deletion case
(`no_floor_call`) in `ci_targets.py --self-test`. Verified by reproducing the reviewer's exact
deletion, confirming `ci_targets.py` now reports the missing site, and restoring the file.

**I2 — `swallowed` read only the first physical line of a `moon` invocation.** A backslash-
continued invocation —
```
moon ci "${T[@]}" \
  --base origin/main \
  --include-relations || true
```
— hid its own `|| true` tail from `ci_target_floor_verdict`'s line-at-a-time scan, returning an
EMPTY verdict (measured) and silencing every gate in `T` while `T` itself stayed correct. Fixed
with a new `continued <lineno>` verdict, checked *before* `swallowed` on the same matched line: a
`moon`-prefixed line ending in a literal backslash is rejected outright, with remediation text
demanding the invocation be rejoined onto one physical line — mirroring `no-array`'s stance on `T`
itself — rather than misdiagnosing it as `swallowed`. Fixture rows added in both directions:
the reviewer's continued form fires `continued <lineno>`; the real if/elif/else form (three
separate single-line `moon` invocations, matching `ci.yml`'s actual shape) does not.

**M3 — T3 (the kill-predicate standing proof) was unimplemented.** The kill decision was inline in
check 9's mutant-collection loop, so nothing proved rc 2/126/127 (or rc 1 without the counter's
message) is never scored as a kill — a gap in a file that cites SMA-466's all-firing-fixture lesson
four times. Implemented, not struck: the decision was extracted into `mutant_is_killed <rc>
<outfile>`, and a **sixth self-test**, `kill_predicate_self_test`, drives it directly against six
synthetic `(rc, output)` pairs (both directions). This is the one finding whose fix changes
`SELF_TEST_COUNT` (5 → 6) — every place that counts self-tests was updated in lockstep: `run.sh`'s
own comment, `usage()`, `README.md`'s check table and "Running it" section, `moon.yml`'s cost
comment, and this spec's §4 components, §5 testing table (T1/T3/T8/T13), and cost-ceiling addendum.
Check 9's battery grew from five mutants to six (seven concurrent `--self-test` subprocesses
including the control); standalone cost moved from ~3.68s/~1.25s (as originally shipped) to
~4.11s/~1.26s (min-of-7, this fix wave) — see `ci/actionlint/README.md`'s cost table.

**M4 — the `actionlint:` task comment in `moon.yml` was stale in four ways**, and
`run.sh:1206`'s "~1.5s" claim was stale in a fifth: the fixture-table count ("four" → six, having
missed the ci-target floor even before M3), the standalone timing (~1.0s → the current measured
figure), the `git ls-files` multiplication (now by seven concurrent `--self-test` subprocesses, not
six), and the `description:` field (still crediting only SMA-525/SMA-540). All five corrected;
`description:` now names SMA-542 and the self-test/mutation-battery guarantee.

**M5 — `arrays` (the `T=(…)` match count) was not numerically validated**, unlike the `defs` and
`n` counters hardened earlier in the branch. An empty `grep -c` result would have made `[ "$arrays"
-ne 1 ]` exit 2 under `set -uo pipefail`'s no-`set -e`, silently skipping the `no-array` report.
Fixed with the identical `case "$arrays" in ''|*[!0-9]*) …` guard already used for `defs`/`n`.

**M6 — `swallowed` had no escape hatch**, unlike `continue-on-error` (which has `COE_SKIP`). Fixed
by adding `SWALLOWED_SKIP`, a `COE_SKIP`-shaped list keyed the identical `"<lineno>:<exact text>"`
way, for a `moon` line this syntactic, file-wide check cannot know is harmless. Chosen over
rewording the failure text alone because an escape hatch, not just better prose, is what a future
legitimate case (a diagnostic `moon run x | tee log` in an unrelated job) will actually need — and
`COE_SKIP` already established the drift-proof keying convention this reuses verbatim. `continued`
deliberately gets **no** equivalent hatch: unlike a harmless pipe, a wrapped invocation has no
legitimate form this check can recognise, so it is rejected outright, like `no-array`.

**M7 — four cross-file line/text citations into `run.sh` had gone stale** as earlier SMA-542 work
reorganised the file (`task_inputs.py:59,118,130`, `ci_targets.py:654`). Re-anchored to the
current lines and verified each quoted string appears there (see the PR/report for the exact
before/after line numbers, which continued to shift as this fix wave's own edits landed).

**M8 — the self-test definition-count regex (`run.sh`'s D13 check) was style-brittle**, missing
`sixth_self_test () {` (space before the parens) and the `function name {` keyword form — either
of which would leave a real table both unwired AND uncounted, reopening the hole D13 exists to
close. Fixed by broadening the regex to
`^(function[[:blank:]]+)?[a-z_]+_self_test([[:blank:]]*\(\))?[[:blank:]]*\{`, covering both
spacing variants and the `function` keyword form (with or without explicit `()`), verified against
synthetic examples of each. The one residual — a definition split across lines
(`name()\n{`) — is accepted and noted in a comment; this file uses no such style today.
