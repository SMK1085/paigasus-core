# SMA-542 Self-Test Invocation Guard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make deleting any self-test invocation in `ci/actionlint/run.sh` red the gate instead of
silently passing, and make removing or suppressing `:affected-smoke` in `ci.yml` red
`repo:actionlint`.

**Architecture:** A cycle of two independently-scheduled gates. `repo:actionlint` gains a counter
over its self-tests, a standing mutation battery that proves the counter fires, and a `T=(…)` floor
check. `repo:affected-smoke` gains a C4 pin over the two new call-site lines — plus the `moon.yml`
input that makes that pin reachable at all.

**Tech Stack:** Bash 3.2 (macOS default — no `declare -A`, no `wait -n`), POSIX `awk`/`sed`/`grep`,
Python 3 (stdlib only), Moon 2.3.2.

**Spec:** `docs/superpowers/specs/2026-08-20-sma-542-self-test-invocation-guard-design.md`

## Global Constraints

- Every source file opens with `# SPDX-License-Identifier: Apache-2.0`. All files here already have it — do not add a second.
- `ci/actionlint/run.sh` runs under `set -uo pipefail` and **deliberately NOT `set -e`**. Never add `set -e`. Check every `cp`/`sed`/`mktemp` status explicitly.
- Exit codes are load-bearing: `1` = assertion failure (via `fail()`, which sets `FAILED`), `2` = infrastructure error (via `infra()`, which exits immediately). Never conflate them.
- Bash 3.2 compatibility: no associative arrays, no `wait -n`, no `${var,,}`. Parallel arrays only.
- `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` before any `moon`/`actionlint` command — the Bash tool PATH lacks the proto-managed CLIs.
- Conventional commits with a workspace scope, subject **starting lowercase**, header ≤100 chars. Allowed types: `feat fix docs chore refactor test ci build perf style revert`. Allowed scopes include `repo`. Never write `#NNN` in a commit body (breaks `footer-leading-blank`).
- Do **not** use `--no-verify`. If the commit-msg hook fails, fix the message.
- `SELF_TEST_COUNT` is `4` after Task 2 and becomes `5` in Task 4. Each task is internally consistent; do not jump ahead.
- Check numbers are logical identities, not execution order. Check 7 stays "the self-tests". New work is checks 8 and 9. **Do not renumber checks 1–7.**

---

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `moon.yml` | `repo:affected-smoke` input that makes the C4 pin reachable | 1 |
| `ci/actionlint/run.sh` | counter, single call site, mutation battery, `T` floor | 2, 3, 4 |
| `ci/affected-graph/ci_targets.py` | C4 pin over `run.sh`'s two call-site lines | 5 |
| `ci/actionlint/README.md` | check table, Limitations, corrected escape hatch, cost | 6 |
| `CLAUDE.md` | one gotcha bullet recording the cycle | 6 |

---

### Task 1: Make the C4 pin reachable

Without this, everything in Task 5 is a no-op on its own threat model: a PR deleting only the two
pinned lines from `ci/actionlint/run.sh` never schedules `repo:affected-smoke`. See spec §2.6.

**Files:**
- Modify: `moon.yml` (the `affected-smoke:` task's `inputs:` list, around lines 130-155)

**Interfaces:**
- Consumes: nothing
- Produces: `repo:affected-smoke` is affected by changes under `ci/actionlint/`

- [ ] **Step 1: Confirm the gap exists**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
touch ci/actionlint/run.sh
moon query tasks --affected 2>/dev/null | grep -c 'affected-smoke'
```

Expected: `0` — the gate that would hold the pin is not scheduled.

- [ ] **Step 2: Add the input**

In `moon.yml`, inside the `affected-smoke:` task's `inputs:` list, immediately **before** the
existing `- 'CLAUDE.md'` entry and its comment, insert:

```yaml
      # SMA-542 — this task now pins the call sites of repo:actionlint's self-tests and mutation
      # battery (ACTIONLINT_SH_CALL_SITES), so a change under ci/actionlint/ MUST re-key it.
      # Without this the pin is real but unreachable: the PR that deletes those lines does not
      # schedule this task, while repo:actionlint (inputs: ['**/*']) still runs and now asserts
      # nothing. Same reasoning as the CLAUDE.md entry below — repo:actionlint's own `**/*` cannot
      # green a different task.
      - 'ci/actionlint/**/*'
```

- [ ] **Step 3: Verify it closes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
touch ci/actionlint/run.sh
moon query tasks --affected 2>/dev/null | grep -c 'affected-smoke'
```

Expected: `1` or more.

- [ ] **Step 4: Verify nothing re-baselines**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke --force 2>&1 | tail -30
```

Expected: PASS. No cascade case keys on `ci/`, and `SELF_TASK_EXPECTED_GLOBS` pins only
`input-liveness`, so no expected-set update is needed. If a case DOES red here, stop and report —
do not re-baseline without saying so.

- [ ] **Step 5: Verify the new glob is live**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:input-liveness --force 2>&1 | tail -20
```

Expected: PASS (the glob matches `run.sh` and `README.md`).

- [ ] **Step 6: Commit**

```bash
git add moon.yml
git commit -m "feat(repo): key repo:affected-smoke on ci/actionlint (SMA-542)" -m "The C4 pin added later in this issue lives in ci_targets.py, which runs inside
repo:affected-smoke. That task did not list ci/actionlint/** among its inputs, so
a PR deleting the pinned lines would not schedule it — while repo:actionlint,
whose inputs are '**/*', would still run and now assert nothing.

Mirrors the CLAUDE.md entry directly below it, for the same reason."
```

---

### Task 2: The counter, the single call site, and the definition count

Spec D2, D3, D13. This is the core restructure.

**Files:**
- Modify: `ci/actionlint/run.sh`
  - insert `SELF_SRC` between `set -uo pipefail` (:23) and `cd …` (:25)
  - insert counter state after `FAILED=0` (:27)
  - add `assert_self_tests_ran` and `run_self_tests` immediately before the `case` dispatch (:1604)
  - add `SELF_TESTS_RAN` increment as the first body line of each of the four self-tests (:273, :1283, :1385, :1551)
  - replace the `case` dispatch (:1604-1620) and the tail invocations (:1866-1873)
  - update the lazy-canary comment (:1186-1191)

**Interfaces:**
- Consumes: Task 1's `moon.yml` input (not referenced in code)
- Produces:
  - `SELF_SRC` — absolute path to this script, valid after the `cd`
  - `SELF_TESTS_RAN` (int), `SELF_TEST_COUNT` (int, `4` in this task)
  - `assert_self_tests_ran <want>` — calls `fail()` on mismatch; message begins `self-test counter:`
  - `run_self_tests` — resets the counter, calls the four self-tests, asserts, resets `ORIGIN_REFS_LOADED`
  - `SELF_TEST_ONLY` (0/1)

- [ ] **Step 1: Write the failing test — a manual mutation that must red**

There is no standing test yet (that is Task 3). Establish the baseline the mutation must break:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
sed '/^extractor_self_test$/d' ci/actionlint/run.sh > /tmp/sma542-mutant.sh
bash /tmp/sma542-mutant.sh --self-test; echo "rc=$?"
```

Expected: `rc=0` — **this is the bug.** A self-test invocation was deleted and the gate is silent.

- [ ] **Step 2: Add `SELF_SRC` before the `cd`**

`$0` is unsafe: `run.sh` `cd`s to the repo root on entry, so `cd ci/actionlint && ./run.sh` leaves
`$0` as `./run.sh`, which no longer resolves. Insert between `set -uo pipefail` and the `cd`:

```bash
# Absolute path to THIS file, captured BEFORE the cd below. `$0` is not usable after it: invoked
# as `cd ci/actionlint && ./run.sh`, `$0` is './run.sh', which stops resolving the moment we move
# to the repo root. Check 9 copies this file, and run_self_tests greps it (SMA-542 D11).
SELF_SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
```

- [ ] **Step 3: Add the counter state after `FAILED=0`**

```bash
# Check 7's counter. A fixture table that is never CALLED is dead code, and deleting the calls was
# the sole survivor of SMA-525's mutation battery. The increment lives inside each self-test (not
# at the call site) so it survives reformatting and cannot be spoofed by a stranded increment.
# Deliberately NOT `readonly`: without `set -e` a reassignment only warns, so readonly buys no
# protection and would break a future harness that sources this file twice (SMA-542 D3).
SELF_TESTS_RAN=0
SELF_TEST_COUNT=4   # extractor, path-filter, branch-filter, config
```

- [ ] **Step 4: Increment inside each self-test**

Add this as the **first line of the function body** in all four — `extractor_self_test` (:273),
`path_filter_self_test` (:1283), `branch_filter_self_test` (:1385), `config_self_test` (:1551).
Place it after the existing `local …` declaration line where one exists:

```bash
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
```

- [ ] **Step 5: Add `assert_self_tests_ran` and `run_self_tests` before the `case` dispatch**

Insert immediately above the `case "$#:${1:-}" in` line:

```bash
# ---------------------------------------------------------------------------------------------
# Check 7 — the self-tests, and the counter that proves they were invoked.
#
# All five (four until SMA-542's floor table lands) are defined above so this block can run them
# from ONE call site, reached by both the --self-test path and the full gate. One call site rather
# than two is deliberate: ci_targets.py's C4 pins this by whole stripped line, and two identical
# lines would let one be deleted while the pin still matched (SMA-542 D2).
# ---------------------------------------------------------------------------------------------
assert_self_tests_ran() {
  local want="$1"
  if [ "$SELF_TESTS_RAN" -ne "$want" ]; then
    fail "self-test counter: $SELF_TESTS_RAN of $want self-tests ran. An invocation is missing from
      run_self_tests. A fixture table that is never called is dead code that guards nothing — this
      is the failure SMA-542 exists to catch. Restore the call."
  fi
}

run_self_tests() {
  local defs
  SELF_TESTS_RAN=0

  extractor_self_test
  path_filter_self_test
  branch_filter_self_test
  config_self_test

  assert_self_tests_ran "$SELF_TEST_COUNT"

  # The counter proves the KNOWN tables ran; it cannot notice a table added tomorrow and never
  # wired up, because the count would still match. Asserting the DEFINITION count closes that —
  # adding a table without calling it reds, and so does deleting one without decrementing
  # SELF_TEST_COUNT. Adding a table is the highest-probability future edit here (SMA-542 D13).
  [ -r "$SELF_SRC" ] || infra "cannot read \$SELF_SRC ($SELF_SRC) to count self-test definitions"
  defs="$(grep -cE '^[a-z_]+_self_test\(\) \{' "$SELF_SRC")"
  if [ "$defs" -ne "$SELF_TEST_COUNT" ]; then
    fail "self-test definitions: $defs '*_self_test' functions are defined but SELF_TEST_COUNT is
      $SELF_TEST_COUNT. A fixture table that is not called from run_self_tests guards nothing.
      Wire it up and bump SELF_TEST_COUNT, or delete it."
  fi

  # branch_filter_self_test reaches its 'no-origin-main' fixture by swapping ORIGIN_REFS for a
  # main-free list and forcing ORIGIN_REFS_LOADED=1, restoring both immediately. That was harmless
  # while the self-tests ran LAST; now they run FIRST, and load_origin_refs early-returns on
  # ORIGIN_REFS_LOADED=1 — so a future botched restore would feed checks 5/6 a fake ref list and
  # turn every real branches: entry into a false 'unresolved' or an infra exit. Resetting here
  # makes checks 5/6 independent of that fixture's bookkeeping rather than trusting it.
  ORIGIN_REFS_LOADED=0
}
```

- [ ] **Step 6: Replace the dispatch and the tail**

Replace the whole `case "$#:${1:-}" in … esac` block (:1604-1620) with:

```bash
SELF_TEST_ONLY=0
case "$#:${1:-}" in
  '0:')
    ;;
  '1:--self-test')
    SELF_TEST_ONLY=1 ;;
  *)
    usage ;;
esac

# Check 7 runs FIRST, and from a single call site. --self-test never shells out to actionlint, so
# this sits AHEAD of the PATH guard below and stays runnable on a machine without the binary. It
# DOES need a git repo carrying origin/main: branch_filter_self_test's control pair asserts a real
# ref resolves (SMA-540 D7). Running the controls before the checks they guard matches the
# convention moon.yml states for repo:affected-smoke, repo:publish-metadata and
# repo:error-code-single-site — a rotted checker must red rather than ship green.
run_self_tests

if [ "$SELF_TEST_ONLY" = 1 ]; then
  exit "$FAILED"
fi
```

Then delete the four invocations at the tail (:1866-1871), leaving the check-7 comment block
replaced by a pointer. The file must end:

```bash
# ---------------------------------------------------------------------------------------------
# Check 7 ran near the top, from run_self_tests — see the comment at its call site for why the
# controls precede the checks they guard.
# ---------------------------------------------------------------------------------------------

exit "$FAILED"
```

- [ ] **Step 7: Update the lazy-canary comment (:1186-1191)**

It states the OLD ordering as a design property and is now false. Replace that comment with:

```bash
  # The canary is LAZY (D7) — only an entry that has survived every filter above actually needs a
  # ref, so checks 1-6 report their own findings before this canary fires. Since SMA-542 that is
  # no longer the whole story for a FULL run: check 7 runs first and asserts the same origin/main
  # precondition unconditionally (branch_filter_self_test), so a checkout without the ref now
  # exits 2 BEFORE actionlint is invoked, and you lose the checks 1-6 findings you used to see.
  # Accepted: README.md gives the one-command recovery and the gate is ~1.5s. Returned as a
  # TOKEN, not an infra call: this function always runs inside $( ), where exit 2 would kill only
  # the subshell.
```

- [ ] **Step 8: Run the mutation again — it must now die**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
sed '/^  extractor_self_test$/d' ci/actionlint/run.sh > /tmp/sma542-mutant.sh
bash /tmp/sma542-mutant.sh --self-test; echo "rc=$?"
```

Expected: `rc=1`, with stderr containing `self-test counter: 3 of 4 self-tests ran`.

Note the leading two spaces in the `sed` address — the call is now indented inside
`run_self_tests`, and an unanchored pattern would also hit the definition line.

- [ ] **Step 9: Verify the healthy paths still pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/actionlint/run.sh --self-test; echo "self-test rc=$?"
ci/actionlint/run.sh;             echo "full rc=$?"
ci/actionlint/run.sh --selftest;  echo "typo rc=$? (expect 2)"
rm -f /tmp/sma542-mutant.sh
```

Expected: `0`, `0`, `2`.

- [ ] **Step 10: Verify AC-2's extraction table is unchanged**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
sed -n '/^extract_filter_keys() {/,/^}/p' ci/actionlint/run.sh > /tmp/sma542-x.sh
bash -c 'source /tmp/sma542-x.sh; for f in .github/workflows/*.yml; do echo "== $f"; extract_filter_keys "$f" | awk -F"\t" "{print \$1, \$2}" | sort | uniq -c; done'
rm -f /tmp/sma542-x.sh
```

Expected, exactly: `ci.yml` 2 KEY/2 ITEM branches; `prebuild.yml` 2 KEY/13 ITEM paths + 2 KEY/2
ITEM branches; `security-scan.yml` 1 KEY/5 ITEM paths + 1 KEY/1 ITEM branches.

- [ ] **Step 11: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "feat(repo): count the actionlint gate's self-test invocations (SMA-542)" -m "Deleting a self-test call left the gate exiting 0 — the sole survivor of
SMA-525's mutation battery. Each self-test now increments a counter from
inside its own body, and run_self_tests asserts the total, so a missing call
reds instead of vanishing.

The two duplicate invocation blocks collapse to one call site, which is what
lets an external pin be unambiguous. That moves check 7 ahead of checks 1-6,
matching the convention moon.yml already states elsewhere; the lazy-canary
comment is updated because that reordering makes its stated property false.

Also asserts the count of *_self_test definitions, so a table added tomorrow
and never wired up reds too."
```

---

### Task 3: The mutation battery (check 9)

Spec D5, D6, D10, D11, D12. This is the standing proof of Task 2.

**Files:**
- Modify: `ci/actionlint/run.sh` — add `selftest_mutation_battery` after `run_self_tests`, and its
  invocation at the tail before `exit "$FAILED"`

**Interfaces:**
- Consumes: `SELF_SRC`, `SELF_TEST_COUNT`, `fail()`, `infra()`, `assert_self_tests_ran`'s
  `self-test counter:` message prefix
- Produces: `selftest_mutation_battery` — full-gate only; never called under `--self-test`

- [ ] **Step 1: Write the failing test**

The battery is itself the test, so the failing case is its absence. Confirm nothing currently
proves the counter fires in CI:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
grep -c 'selftest_mutation_battery' ci/actionlint/run.sh
```

Expected: `0`.

- [ ] **Step 2: Add the battery**

Insert after `run_self_tests`'s closing brace:

```bash
# ---------------------------------------------------------------------------------------------
# Check 9 — prove the counter actually fires, by mutation rather than assertion.
#
# SMA-525's F4: a one-off mutation battery is not a standing control. So the battery runs in CI.
#
# NO RECURSION, BY CONSTRUCTION: mutants are invoked with --self-test, which exits before this
# function is reached. That is why nothing here needs a bypass env var (which would be a live
# switch for turning the gate off) and why each mutant deletes exactly ONE line (SMA-542 D5).
#
# The invocation list is DERIVED from run_self_tests' own body, not hardcoded: adding a table
# extends the battery automatically, and a mismatch against SELF_TEST_COUNT reds.
# ---------------------------------------------------------------------------------------------
selftest_mutation_battery() {
  local dir lines line n removed mutant rc i label
  local pids='' labels=''

  [ -r "$SELF_SRC" ] || infra "check 9: cannot read \$SELF_SRC ($SELF_SRC)"

  lines="$(awk '
    /^run_self_tests\(\) \{$/ { inside = 1; next }
    inside && /^\}$/          { exit }
    inside && /^  [a-z_]+_self_test$/ { print $1 }
  ' "$SELF_SRC")"
  n="$(printf '%s\n' "$lines" | grep -c '[^[:space:]]')"
  if [ "$n" -ne "$SELF_TEST_COUNT" ]; then
    fail "check 9: found $n self-test invocations inside run_self_tests, expected
      \$SELF_TEST_COUNT=$SELF_TEST_COUNT. Either a call is missing or the count is stale."
    return
  fi

  dir="$(mktemp -d)" || infra "check 9: mktemp -d failed"
  # Expanded at trap-SET time (double quotes), because $dir is local and would be out of scope by
  # the time an EXIT trap fired.
  trap "rm -rf '$dir'" EXIT

  # EVERY precondition is validated BEFORE any subprocess is created. A sed that matched nothing
  # would otherwise produce a mutant byte-identical to the original, which exits 0 and reads as a
  # survivor — an accurate red for a completely misleading reason.
  for line in $lines; do
    sed "/^  ${line}\$/d" "$SELF_SRC" > "$dir/$line.sh" \
      || infra "check 9: sed failed while mutating '$line'"
    removed=$(( $(wc -l < "$SELF_SRC") - $(wc -l < "$dir/$line.sh") ))
    if [ "$removed" -ne 1 ]; then
      fail "check 9: mutating '$line' removed $removed lines, expected exactly 1. The invocation
        inside run_self_tests is not on a line of its own, or is duplicated. Check 9 cannot build a
        meaningful mutant until it is."
      return
    fi
  done

  # Spawned concurrently: six sequential --self-test runs would roughly quadruple this gate's
  # standalone cost, and they are independent. Collected by PID, so results do not depend on
  # completion order (SMA-542 D12).
  for line in $lines; do
    bash "$dir/$line.sh" --self-test > "$dir/$line.out" 2>&1 &
    pids="$pids $!"
    labels="$labels $line"
  done
  # The control (D6): the REAL file, unmutated, which must exit 0. Five mutants that all fire
  # cannot tell a working battery from a stuck one (SMA-466). This proves the harness itself —
  # the bash invocation, the cwd, the argument passing — yields 0 on a healthy tree.
  bash "$SELF_SRC" --self-test > "$dir/__control__.out" 2>&1 &
  pids="$pids $!"
  labels="$labels __control__"

  i=0
  set -- $labels
  for pid in $pids; do
    i=$((i + 1))
    eval "label=\${$i}"
    wait "$pid"
    rc=$?
    if [ "$label" = '__control__' ]; then
      if [ "$rc" -ne 0 ]; then
        fail "check 9: the unmutated control exited $rc, expected 0. The battery's own harness is
      broken, so its five dead mutants prove nothing. Output follows."
        sed 's/^/      check 9 [control]: /' "$dir/__control__.out" >&2
      fi
      continue
    fi
    # A KILL is rc 1 carrying the counter's own message — not merely "non-zero". infra() exits 2,
    # a missing file exits 127, and branch_filter_self_test's own precondition exits 2; scoring any
    # of those as a kill would let a transient fault stand in for the proof (SMA-542 D10).
    if [ "$rc" -eq 1 ] && grep -q 'self-test counter:' "$dir/$label.out"; then
      continue
    fi
    if [ "$rc" -eq 2 ]; then
      fail "check 9: mutant '$label' aborted with an infrastructure error (rc 2) before reaching
      the counter, so it proves nothing. Output follows."
    else
      fail "check 9: mutant '$label' exited $rc without the counter's message. Deleting that
      invocation did NOT red the gate — assert_self_tests_ran is missing or neutered, which is
      exactly the silent pass SMA-542 exists to prevent. Output follows."
    fi
    sed "s/^/      check 9 [mutant $label]: /" "$dir/$label.out" >&2
  done
}
```

Note the output handling: mutant output is captured to per-mutant files, **discarded** on the
expected outcome, and re-emitted **prefixed** only on an unexpected one. Every line a mutant prints
starts `actionlint gate:`, so unprefixed it would be indistinguishable from the real gate's output
under `buffer-only-failure`.

- [ ] **Step 3: Invoke it at the tail**

Replace the check-7 pointer comment at the end of the file with:

```bash
# ---------------------------------------------------------------------------------------------
# Check 7 ran near the top, from run_self_tests — see the comment at its call site for why the
# controls precede the checks they guard.
#
# Check 9 runs HERE, and only here: it is deliberately NOT part of --self-test. That is what makes
# recursion structurally impossible (mutants are invoked with --self-test and exit before reaching
# it), and it keeps --self-test the fast iteration path README.md advertises.
# ---------------------------------------------------------------------------------------------
selftest_mutation_battery

exit "$FAILED"
```

- [ ] **Step 4: Verify the battery passes on a healthy tree**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
ci/actionlint/run.sh; echo "rc=$?"
```

Expected: `rc=0`, no output about check 9.

- [ ] **Step 5: Verify `--self-test` does NOT run the battery**

```bash
time ci/actionlint/run.sh --self-test; echo "rc=$?"
```

Expected: `rc=0` and wall time ~1.0s (unchanged from before this task). If it is ~2s or more, the
battery is running under `--self-test` — fix the placement.

- [ ] **Step 6: Prove the battery kills a real mutant (T4)**

Delete `assert_self_tests_ran`'s call and confirm every mutant survives and the battery reds:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
cp ci/actionlint/run.sh /tmp/sma542-keep.sh
sed -i '' '/^  assert_self_tests_ran "\$SELF_TEST_COUNT"$/d' ci/actionlint/run.sh
ci/actionlint/run.sh 2>&1 | grep -c 'did NOT red the gate'
```

Expected: `4` (one per mutant). Then restore:

```bash
cp /tmp/sma542-keep.sh ci/actionlint/run.sh && rm /tmp/sma542-keep.sh
git diff --stat ci/actionlint/run.sh
```

Expected: no diff. **Do not restore with `mv` from a `.bak`** — that rolls mtime backwards and can
leave stale cached state; `cp` over the original keeps mtime moving forward.

- [ ] **Step 7: Measure the cost against the ceiling**

```bash
for i in 1 2 3; do /usr/bin/time -p ci/actionlint/run.sh 2>&1 | grep real; done
```

Baseline before this task was ~1.5s. **The ceiling is 2× that, i.e. ~3s.** If the median exceeds
it, stop and report — the spec's §5 fallback is one mutant plus five textual preconditions, and
that is a decision to surface, not to take unilaterally.

- [ ] **Step 8: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "feat(repo): prove the self-test counter fires, by mutation (SMA-542)" -m "SMA-525's F4: a one-off mutation battery is not a standing control. Check 9
therefore runs in CI — it derives the invocation list from run_self_tests'
own body, builds one mutant per line, and requires each to exit 1 carrying the
counter's message. An rc-2 abort is reported as proving nothing rather than
scored as a kill.

Runs on the full gate only, which makes recursion structurally impossible:
mutants are invoked with --self-test and exit before reaching it. An unmutated
control must exit 0, so a broken harness cannot read as five dead mutants."
```

---

### Task 4: The `T=()` floor and the suppression check (check 8)

Spec D7, D8, D14.

**Files:**
- Modify: `ci/actionlint/run.sh`
  - add `T_FLOOR`, `ci_target_floor_verdict`, `ci_target_floor_self_test` before the dispatch
  - call `ci_target_floor_self_test` from `run_self_tests`; bump `SELF_TEST_COUNT` to `5`
  - add check 8's production call site after the `--self-test` early exit, **before** the PATH guard
  - update `usage()`

**Interfaces:**
- Consumes: `fail()`, `infra()`, `SELF_TESTS_RAN`
- Produces: `ci_target_floor_verdict <path>` emitting `no-file` | `no-array` | `missing <entry>` |
  `swallowed <lineno>` | nothing

- [ ] **Step 1: Write the failing test**

Add `ci_target_floor_self_test` (Step 3 has the full body), wire it into `run_self_tests`, and run:

```bash
ci/actionlint/run.sh --self-test; echo "rc=$?"
```

Expected: FAIL — `ci_target_floor_verdict: command not found`.

- [ ] **Step 2: Add `T_FLOOR` and the verdict function**

Insert before the dispatch, after `config_self_test`:

```bash
# ---------------------------------------------------------------------------------------------
# Check 8 (definitions) — ci.yml's `T=(…)` must still schedule the gate that guards `T`.
#
# Deleting `:affected-smoke` from that array stops repo:affected-smoke running, which in ONE edit
# removes ci_targets.py's C1-C5, all eight cascade cases, cargo_moon_parity's A1-A5 and
# assert_include_relations — every one of them green, because the thing that would complain is the
# thing that stopped running. It is unclosable from inside ci/affected-graph/ for the same reason
# SMA-542 exists, and this gate is the natural host: independently scheduled, and already reading
# every workflow file.
#
# ':actionlint' is DELIBERATELY NOT in the floor. ci_targets.py's C1 (check_forward) is a strict
# equality over T's repo partition, so dropping :actionlint already reds repo:affected-smoke.
# Asserting it HERE would be vacuous: the only run in which the entry is missing is the run in
# which this assertion does not execute (SMA-542 D8).
# ---------------------------------------------------------------------------------------------
T_FLOOR=(':affected-smoke')

# Echoes one verdict token per problem, and nothing for an acceptable file:
#   no-file              the workflow does not exist
#   no-array             zero, or more than one, single-line T=( … )
#   missing <entry>      the array parsed; <entry> is not among its tokens
#   swallowed <lineno>   a `moon` command line discards its own exit status
ci_target_floor_verdict() {
  local f="$1" arrays body tok w found lineno text

  [ -e "$f" ] || { echo 'no-file'; return; }

  # Anchored like ci_targets.py's T_ARRAY_RE, not a bare `T=(` — which would also match `EXPECT=(`.
  # Zero or two matches is a FAILURE, never a skip: an array reformatted across lines is exactly
  # the condition under which this check would otherwise stop asserting anything.
  arrays="$(grep -cE '^[ \t]*T=\(.*\)[ \t]*$' "$f")"
  if [ "$arrays" -ne 1 ]; then
    echo 'no-array'
    return
  fi
  body="$(sed -nE 's/^[ \t]*T=\((.*)\)[ \t]*$/\1/p' "$f")"

  for tok in "${T_FLOOR[@]}"; do
    found=0
    # Whole-token comparison: ':affected-smoke' is a prefix of ':affected-smoke-disabled', so a
    # substring test would accept a renamed-away gate.
    for w in $body; do
      if [ "$w" = "$tok" ]; then found=1; break; fi
    done
    [ "$found" -eq 1 ] || echo "missing $tok"
  done

  # D14 — `|| true` on the moon line silences every gate in T while leaving T itself perfectly
  # correct: C1/C2/C3 pass, C5's expansion test passes, and `set -euo pipefail` does not help
  # because the step exits 0. Complementary to C5, which asserts T is HANDED OVER; this asserts
  # the result is PROPAGATED. It lives here because this is the half that survives
  # repo:affected-smoke being silenced.
  while IFS=: read -r lineno text; do
    case "$text" in
      *'||'*|*'&&'*|*';'*|*'|'*) echo "swallowed $lineno" ;;
    esac
  done < <(grep -nE '^[ \t]*moon[ \t]' "$f")
}
```

- [ ] **Step 3: Add the fixture table**

```bash
# The standing control for check 8. Both directions on every verdict: a table whose rows all fire
# cannot tell a working check from a stuck one (SMA-466).
ci_target_floor_self_test() {
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
  local rc=0 tmp got

  expect_floor() {
    local name="$1" expected="$2" body="$3"
    tmp="$(mktemp)"
    printf '%s' "$body" > "$tmp"
    got="$(ci_target_floor_verdict "$tmp")"
    rm -f "$tmp"
    if [ "$got" != "$expected" ]; then
      fail "ci-target-floor self-test '$name': got '$got', expected '$expected'. Check 8 is not
      deciding what it is documented to decide."
      rc=1
    fi
  }

  expect_floor 'healthy array and invocations' '' \
'          T=(:build :affected-smoke :actionlint)
          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]}" --base origin/main --include-relations
          else
            moon run "${T[@]}"
          fi
'
  expect_floor 'floor entry absent' 'missing :affected-smoke' \
'          T=(:build :actionlint)
          moon ci "${T[@]}" --base origin/main --include-relations
'
  # A prefix of the floor entry must NOT satisfy it.
  expect_floor 'floor entry renamed away' 'missing :affected-smoke' \
'          T=(:build :affected-smoke-disabled)
          moon ci "${T[@]}"
'
  expect_floor 'no array at all' 'no-array' \
'          moon ci --base origin/main
'
  expect_floor 'two arrays' 'no-array' \
'          T=(:affected-smoke)
          T=(:build)
          moon ci "${T[@]}"
'
  expect_floor 'array reformatted across lines' 'no-array' \
'          T=(
            :affected-smoke
          )
          moon ci "${T[@]}"
'
  # A similarly-shaped assignment must not be read as the array.
  expect_floor 'lookalike assignment is not T' 'no-array' \
'          EXPECT=(:affected-smoke)
          moon ci "${T[@]}"
'
  expect_floor 'moon failure swallowed' 'swallowed 2' \
'          T=(:affected-smoke)
          moon ci "${T[@]}" --base origin/main --include-relations || true
'
  # `moon` not at command position is not an invocation — it must not fire.
  expect_floor 'moon mentioned in a comment' '' \
'          T=(:affected-smoke)
          # run moon ci; it will pass
          moon ci "${T[@]}"
'

  got="$(ci_target_floor_verdict /nonexistent/ci.yml)"
  if [ "$got" != 'no-file' ]; then
    fail "ci-target-floor self-test 'missing file': got '$got', expected 'no-file'. A renamed
      workflow must not report the misleading 'keep T on one line' remediation."
    rc=1
  fi

  return $rc
}
```

- [ ] **Step 4: Wire it in and bump the count**

In `run_self_tests`, add after `config_self_test`:

```bash
  ci_target_floor_self_test
```

And change the counter declaration near the top of the file to:

```bash
SELF_TEST_COUNT=5   # extractor, path-filter, branch-filter, config, ci-target-floor
```

- [ ] **Step 5: Run — the table must pass**

```bash
ci/actionlint/run.sh --self-test; echo "rc=$?"
```

Expected: `rc=0`.

- [ ] **Step 6: Add check 8's production call site**

Insert **after** the `if [ "$SELF_TEST_ONLY" = 1 ]; then exit "$FAILED"; fi` block and **before**
the `command -v actionlint` guard — check 8 reads a tracked file and needs no binary, so a machine
without `actionlint` should still get its verdict:

```bash
# ---------------------------------------------------------------------------------------------
# Check 8 — the T=() floor, and the propagation of `moon`'s exit status. Rationale and fixtures
# are with ci_target_floor_verdict above.
# ---------------------------------------------------------------------------------------------
while IFS= read -r verdict; do
  case "$verdict" in
    '') ;;
    no-file)
      fail ".github/workflows/ci.yml does not exist, so this gate cannot confirm that
      repo:affected-smoke is still scheduled. If the workflow was renamed, update this check." ;;
    no-array)
      fail ".github/workflows/ci.yml has no single-line 'T=( … )' array, or has more than one, so
      the target list cannot be read. Keep T on ONE line — ci_targets.py's C3 requires it too."  ;;
    'missing '*)
      fail ".github/workflows/ci.yml's T=( … ) no longer contains '${verdict#missing }'. That
      entry schedules the gate which guards T itself: without it ci_targets.py's C1-C5, the
      affected-graph cascade cases and cargo_moon_parity all stop running, every one of them
      green. Restore it — and the matching entry in CLAUDE.md's ci-targets block." ;;
    'swallowed '*)
      fail ".github/workflows/ci.yml:${verdict#swallowed } runs 'moon' but discards its exit
      status (a '||', '&&', ';' or '|' tail). That greens every gate in T while leaving T itself
      perfectly correct, so no other check in this repo can see it. Remove the tail." ;;
    *)
      infra "unhandled ci-target-floor verdict '$verdict'" ;;
  esac
done < <(ci_target_floor_verdict .github/workflows/ci.yml)
```

- [ ] **Step 7: Update `usage()`**

```bash
usage() {
  echo "usage: $(basename "$0") [--self-test]" >&2
  echo "  (no argument)  run the full gate" >&2
  echo "  --self-test    run the five fixture tables only — extractor, path-filter verdicts," >&2
  echo "                 branch-filter verdicts, config allowlist, ci-target floor. No actionlint" >&2
  echo "                 binary is required, but the branch-filter table needs a git repo" >&2
  echo "                 carrying refs/remotes/origin/main. The check-9 mutation battery is NOT" >&2
  echo "                 part of this — it runs on the full gate only." >&2
  exit 2
}
```

- [ ] **Step 8: Prove check 8 fires on the real file (T9, T11)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
cp .github/workflows/ci.yml /tmp/sma542-ci.yml

sed -i '' 's/ :affected-smoke//' .github/workflows/ci.yml
ci/actionlint/run.sh 2>&1 | grep -c 'no longer contains'
cp /tmp/sma542-ci.yml .github/workflows/ci.yml

sed -i '' 's|moon ci "${T\[@\]}" --base origin/main --include-relations|& \|\| true|' .github/workflows/ci.yml
ci/actionlint/run.sh 2>&1 | grep -c 'discards its exit'
cp /tmp/sma542-ci.yml .github/workflows/ci.yml && rm /tmp/sma542-ci.yml
git diff --stat .github/workflows/ci.yml
```

Expected: `1`, then `1`, then no diff.

- [ ] **Step 9: Verify the healthy paths and the battery still pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/actionlint/run.sh --self-test; echo "self-test rc=$?"
ci/actionlint/run.sh;             echo "full rc=$?"
```

Expected: `0`, `0`. The battery now builds 5 mutants — its derived count must match
`SELF_TEST_COUNT=5` automatically.

- [ ] **Step 10: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "feat(repo): assert ci.yml still schedules the gate that guards T (SMA-542)" -m "Deleting :affected-smoke from ci.yml's T array stops repo:affected-smoke
running, which removes C1-C5, the cascade cases and cargo_moon_parity in one
edit — all of them green, because the thing that would complain is the thing
that stopped running. Check 8 asserts the entry from repo:actionlint, which is
scheduled independently and so survives that deletion.

Also covers the suppression spelling: appending '|| true' to the moon line
silences every gate in T while leaving T itself correct, and nothing else in
the repo sees it.

:actionlint is deliberately absent from the floor — C1's strict equality
already covers it, and asserting it here would be vacuous."
```

---

### Task 5: Pin the call sites from `repo:affected-smoke` (C4)

Spec §2.4, D4.

**Files:**
- Modify: `ci/affected-graph/ci_targets.py`
  - add `ACTIONLINT_SH_CALL_SITES` after `SELF_SCHEDULED_GATES` (~:200)
  - `check_self_invocation` (:539) gains a required third positional parameter
  - `main()` (:1063) reads `ci/actionlint/run.sh`
  - `self_test()` (~:930-985) — update all existing calls, add new cases
  - update the `missing_sites` message (:1143-1147) and the `RUN_SH_CALL_SITES` comment (:171-172)

**Interfaces:**
- Consumes: `run_self_tests` and `selftest_mutation_battery` existing as whole lines in
  `ci/actionlint/run.sh`
- Produces: `check_self_invocation(run_sh_text, scripts, actionlint_sh_text)`

- [ ] **Step 1: Write the failing test**

Add to `self_test()`, immediately after the existing `check_self_invocation` cases:

```python
    wired_actionlint = (
        # Load-bearing, exactly as `assert_ci_targets() {` is above: with the DEFINITION present,
        # `no_actionlint_call` below still contains the bare name `run_self_tests`, so a
        # name-only entry would survive deleting the call. Whole-line matching is what separates
        # them, and dropping this line silently de-fangs that assertion.
        "run_self_tests() {\n  :\n}\n"
        "run_self_tests\n"
        "selftest_mutation_battery\n"
    )
    if check_self_invocation(wired, scripts, wired_actionlint):
        failures.append("check_self_invocation: fired on a wired actionlint tree")
    no_actionlint_call = wired_actionlint.replace("\nrun_self_tests\n", "\n")
    if not check_self_invocation(wired, scripts, no_actionlint_call):
        failures.append("check_self_invocation: missed a deleted run_self_tests call")
    no_battery = wired_actionlint.replace("selftest_mutation_battery\n", "")
    if not check_self_invocation(wired, scripts, no_battery):
        failures.append("check_self_invocation: missed a deleted mutation-battery call")
    # Swap cases, BOTH directions — the existing suite carries these for the other two texts and
    # they are the reason the haystacks are kept separate. A call site living in the wrong file
    # must not satisfy the other's requirement.
    if not check_self_invocation(wired, scripts, wired):
        failures.append("check_self_invocation: accepted affected-graph's text as actionlint's")
    if not check_self_invocation(wired_actionlint, scripts, wired_actionlint):
        failures.append("check_self_invocation: accepted actionlint's text as affected-graph's")
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
python3 ci/affected-graph/ci_targets.py --self-test; echo "rc=$?"
```

Expected: FAIL — `TypeError: check_self_invocation() takes 2 positional arguments but 3 were given`.

- [ ] **Step 3: Add the constant**

Insert after the `SELF_SCHEDULED_GATES` dict:

```python
# C4, actionlint half (SMA-542). repo:actionlint's self-tests and mutation battery are invoked from
# ONE call site each inside ci/actionlint/run.sh. That script cannot assert its own invocation —
# deleting the calls was the sole survivor of SMA-525's mutation battery — so the assertion lives
# here, in a gate scheduled independently of it. The reverse direction is check 8 in that same
# script, which asserts `:affected-smoke` is still in `T`: neither gate is the sole judge of its
# own scheduling.
#
# REACHABILITY IS NOT AUTOMATIC. This check only runs when repo:affected-smoke is scheduled, so
# moon.yml lists `ci/actionlint/**/*` among its inputs. Without that entry a PR deleting these two
# lines would not schedule this task at all, while repo:actionlint (inputs: ['**/*']) still ran and
# asserted nothing — the exact defect this closes. Do not remove that input.
#
# Matched as WHOLE STRIPPED LINES, like SELF_SCHEDULED_GATES and unlike RUN_SH_CALL_SITES:
# `run_self_tests` is a strict substring of its own definition line `run_self_tests() {`, so a
# substring test would report the file as wired after the call had been deleted.
#
# PROPAGATION CONTRACT — these entries carry no `|| RC=1` suffix, and that is not the hole
# RUN_SH_CALL_SITES' suffixes close. Both functions report through run.sh's global `FAILED`, as its
# four self-tests already do (run.sh:29-32), so there is no status to propagate at the call site.
# The consequence is that a future `run_self_tests || FAILED=1` would red this check even though it
# is harmless; restore the bare line, or update this constant.
ACTIONLINT_SH_CALL_SITES = ("run_self_tests", "selftest_mutation_battery")
```

- [ ] **Step 4: Extend `check_self_invocation`**

Replace the signature and body:

```python
def check_self_invocation(run_sh_text, scripts, actionlint_sh_text):
    """Call sites of the affected-graph and actionlint gates missing from where they must appear.

    Three haystacks, matched TWO different ways. run.sh sites are substrings, because they are
    indented and one is a mid-line fragment, and their `|| RC=1` suffixes already make them
    unambiguous. Task-script and actionlint sites are whole stripped LINES, because in each case
    one required token is a strict prefix of something else in the file — `task_inputs.py` of
    `task_inputs.py --self-test`, and `run_self_tests` of `run_self_tests() {`.

    The three texts are checked SEPARATELY rather than against one concatenated haystack, so a call
    site living in the wrong file cannot satisfy another's requirement.

    `actionlint_sh_text` is a REQUIRED positional parameter, deliberately. An optional one
    defaulting to "" would make every existing caller pass vacuously — re-creating the class of
    hole this check exists to close.
    """
    missing = [site for site in RUN_SH_CALL_SITES if site not in run_sh_text]
    for task, required in sorted(SELF_SCHEDULED_GATES.items()):
        present = {line.strip() for line in scripts.get(task, "").splitlines()}
        missing.extend(f"{task} script: {site}" for site in required if site not in present)
    actionlint_lines = {line.strip() for line in actionlint_sh_text.splitlines()}
    missing.extend(
        f"ci/actionlint/run.sh: {site}"
        for site in ACTIONLINT_SH_CALL_SITES
        if site not in actionlint_lines
    )
    return missing
```

- [ ] **Step 5: Update the existing call sites in `self_test()`**

Every existing `check_self_invocation(...)` call in `self_test()` takes two arguments. Add
`wired_actionlint` as the third to each. Move the `wired_actionlint = (...)` definition **above**
the first such call so it is in scope. There are 11 existing calls; all must pass a wired
actionlint text so they continue testing what they were written to test.

- [ ] **Step 6: Read the file in `main()`**

Inside the existing `try:` block, after the `run_sh = read_input(...)` assignment:

```python
        actionlint_sh = read_input(
            root / "ci" / "actionlint" / "run.sh", "ci/actionlint/run.sh"
        )
```

And update the call below the `try:`:

```python
    missing_sites = check_self_invocation(run_sh, scripts, actionlint_sh)
```

- [ ] **Step 7: Update the two comments and the failure message**

In `RUN_SH_CALL_SITES`' comment, replace the sentence *"SMA-542 is the general fix for this class
(spec L6)."* with:

```python
# SMA-542 closed the general case: ACTIONLINT_SH_CALL_SITES below pins repo:actionlint's own call
# sites from here, and check 8 in ci/actionlint/run.sh pins `:affected-smoke` in `T` from there, so
# the two gates guard each other rather than themselves.
```

In the `missing_sites` message, replace the `Fix:` line with:

```python
         "    Fix: restore the exact line; see RUN_SH_CALL_SITES, SELF_SCHEDULED_GATES and\n"
         "    ACTIONLINT_SH_CALL_SITES in ci/affected-graph/ci_targets.py.\n"
         "    A row prefixed `ci/actionlint/run.sh:` means repo:actionlint would run its checks\n"
         "    while asserting nothing — its self-tests or its mutation battery are no longer\n"
         "    invoked."),
```

Also update the message's opening sentence to mention the third source:

```python
         "A gate's own call site is missing: this gate's, from\n"
         "    ci/affected-graph/run.sh; a self-scheduled gate's own invocation from inside its\n"
         "    moon.yml task script; or repo:actionlint's, from ci/actionlint/run.sh — so that\n"
         "    gate (or its negative control) would not run at all.\n"
```

- [ ] **Step 8: Run the self-test**

```bash
python3 ci/affected-graph/ci_targets.py --self-test; echo "rc=$?"
```

Expected: `rc=0`.

- [ ] **Step 9: Prove it fires on the real file (T5)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
cp ci/actionlint/run.sh /tmp/sma542-keep.sh
sed -i '' '/^selftest_mutation_battery$/d' ci/actionlint/run.sh
python3 ci/affected-graph/ci_targets.py 2>&1 | grep -c 'ci/actionlint/run.sh: selftest_mutation_battery'
cp /tmp/sma542-keep.sh ci/actionlint/run.sh && rm /tmp/sma542-keep.sh
git diff --stat ci/actionlint/run.sh
```

Expected: `1`, then no diff.

- [ ] **Step 10: Run the whole gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke --force 2>&1 | tail -20
```

Expected: PASS.

- [ ] **Step 11: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -m "feat(repo): pin repo:actionlint's call sites from affected-smoke (SMA-542)" -m "The actionlint gate cannot assert its own invocation, so C4 now pins its two
call sites — run_self_tests and selftest_mutation_battery — from a gate that
is scheduled independently of it. Matched as whole stripped lines, because
run_self_tests is a strict substring of its own definition.

The third haystack is a required positional parameter, not an optional one:
a default of '' would make all eleven existing callers pass vacuously, which
is the class of hole this check exists to close. Swap cases in both
directions keep the texts from satisfying each other."
```

---

### Task 6: Documentation and full-graph verification

Spec §4 (README), AC-3.

**Files:**
- Modify: `ci/actionlint/README.md`
- Modify: `CLAUDE.md`

**Interfaces:**
- Consumes: everything above
- Produces: nothing

- [ ] **Step 1: Update the check table**

In `README.md`'s "The checks" table, change row 7 and append rows 8 and 9:

```markdown
| 7 | Five self-tests against fixture tables — extractor, path-filter verdicts, branch-filter verdicts, config allowlist, ci-target floor (`run.sh --self-test`), plus a counter asserting all five were invoked |
| 8 | `ci.yml`'s `T=(…)` still contains `:affected-smoke`, and no `moon` line discards its exit status |
| 9 | A mutation battery: deleting any self-test invocation must red the gate, proven per run |
```

- [ ] **Step 2: Add the Limitations section**

Insert before "## Cost". There is no existing Limitations section — this adds one:

```markdown
## Limitations

**L1 — Deleting both `T` entries in one edit.** Removing `:affected-smoke` *and* `:actionlint`
from `T=(…)` together silences both halves of the cycle: neither gate runs, so neither complains.
Inherent — two independently-scheduled gates are the most the graph offers, and a third would only
move the pair to a triple. Bounded: `moon ci`'s target list is a single, short, reviewed line.

**L2 — Coordinated multi-line edits inside `run_self_tests`.** The counter, the definition count
and the mutation battery each red on a single-line change. Editing the body *and* `SELF_TEST_COUNT`
*and* the definitions consistently would pass.

**L3 — The whole-line pin is brittle against reformatting.** A future `run_self_tests || FAILED=1`
reds `ci_targets.py`'s C4 even though it is harmless — propagation is already via the global
`FAILED`. Restore the bare line, or update `ACTIONLINT_SH_CALL_SITES`.

**L4 — The battery proves invocation, not correctness.** A self-test whose fixtures were weakened
still runs, still increments, and still passes. That is check 7's own tables' job.

**L5 — `.git` state remains outside Moon's input hash.** See the `actionlint:` task in `moon.yml`.
The `T` floor reads a tracked file, so it is unaffected; check 5's branch half still is.

**L6 — Suppression spellings beyond `moon ci`.** Check 8 pins the `moon` command lines in `ci.yml`.
`continue-on-error: true` on the step is not covered (worth a follow-up); removing the step
entirely is caught by the required-check configuration on GitHub, outside this repo.
```

- [ ] **Step 3: Correct the escape hatch**

`README.md:86-88` says dropping `:actionlint` from `T=(…)` is the escape hatch. That has been wrong
since SMA-541 shipped. Replace that bullet with:

```markdown
- **Anything worse**: drop `:actionlint` from `T=(…)` in `.github/workflows/ci.yml`. This must
  also be removed from the CLAUDE.md `ci-targets` block, since `repo:affected-smoke` asserts the
  two agree — **and** needs a `T_EXEMPT` entry in `ci/affected-graph/ci_targets.py` with a stated
  reason, or C1's strict equality reds on the now-missing entry.
```

- [ ] **Step 4: Update "Running it" and the cost table**

Change the `--self-test` line and add the note:

```markdown
ci/actionlint/run.sh --self-test   # the five fixture tables only, for fast iteration
```

```markdown
`--self-test` runs the fixture tables and nothing else — the check-9 mutation battery is
full-gate-only, which is what keeps this the fast path and what makes the battery's mutants
(invoked with `--self-test`) unable to recurse.

Since SMA-542 the self-tests run **before** checks 1–6. On a `--depth 1` or `--single-branch`
clone the `origin/main` canary therefore fires before `actionlint` is invoked, so you lose the
checks 1–6 findings you would previously have seen. Recover with the explicit refspec below and
re-run; the gate is a couple of seconds.
```

Then add the measured battery row to the cost table, using the figures from Task 3 Step 7.

- [ ] **Step 5: Add the CLAUDE.md gotcha**

Append one bullet to the "Gotchas" section. Keep it short, and do **not** add a second copy of the
`ci-targets` marker command anywhere — a second copy reds `repo:affected-smoke` (SMA-541):

```markdown
- `repo:actionlint` and `repo:affected-smoke` now **guard each other**, and neither can guard itself
  (SMA-542). `ci/actionlint/run.sh` asserts `:affected-smoke` is still in `ci.yml`'s target array —
  and that no `moon` line discards its exit status, since a `|| true` there greens every gate while
  leaving the array correct. In return, `ci_targets.py`'s `ACTIONLINT_SH_CALL_SITES` pins
  `run_self_tests` and `selftest_mutation_battery` as **whole lines** in `run.sh` (a substring
  match would survive deleting the call, since the name is a prefix of its own definition). That
  pin only works because `repo:affected-smoke` lists `ci/actionlint/**/*` in its `inputs` — remove
  that and the pin stays green on exactly the PR that breaks it. Adding a fifth-and-later
  `*_self_test` table means bumping `SELF_TEST_COUNT`: the gate asserts invocations AND definitions.
```

- [ ] **Step 6: Verify the docs gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
grep -c 'ci-targets:begin' CLAUDE.md   # must be exactly 1
moon run repo:affected-smoke --force 2>&1 | tail -20
```

Expected: `1`, then PASS.

- [ ] **Step 7: Run the full graph, as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity \
  :release-parity-py :release-parity-ts :publish-metadata --base origin/main --include-relations
```

Expected: all green. If Moon reports an unattributed failure, diagnose with:

```bash
jq '.actions[] | select(.status=="failed") | .label' .moon/cache/ciReport.json
```

- [ ] **Step 8: Commit**

```bash
git add ci/actionlint/README.md CLAUDE.md
git commit -m "docs(repo): document the actionlint/affected-smoke guard cycle (SMA-542)" -m "Adds the check table rows for checks 8 and 9, a Limitations section, and the
measured battery cost. Corrects the escape hatch: dropping :actionlint from
the target array has needed a T_EXEMPT entry since SMA-541 shipped, or C1's
strict equality reds on it.

Notes in CLAUDE.md that the two gates guard each other, that the pin depends
on repo:affected-smoke keying on ci/actionlint, and that a new self-test
table means bumping SELF_TEST_COUNT."
```

---

## Self-Review

**Spec coverage.** D1 → Tasks 1/4/5. D2 → Task 2 (Steps 6, 7). D3 → Task 2 (Steps 3, 4, 5). D4 →
Task 5. D5/D6/D10/D11/D12 → Task 3. D7/D8 → Task 4. D9 → Task 1. D13 → Task 2 (Step 5). D14 →
Task 4 (Steps 2, 3, 6). §4 README → Task 6. Testing T1-T15 → Task 3 Steps 4-6, Task 4 Step 8,
Task 5 Steps 8-9, Task 6 Step 7, Task 2 Step 10.

**Naming consistency.** `SELF_SRC`, `SELF_TESTS_RAN`, `SELF_TEST_COUNT`, `SELF_TEST_ONLY`,
`assert_self_tests_ran`, `run_self_tests`, `selftest_mutation_battery`, `T_FLOOR`,
`ci_target_floor_verdict`, `ci_target_floor_self_test`, `ACTIONLINT_SH_CALL_SITES` — each spelled
identically everywhere it appears, including inside the `sed` addresses and the `awk` program.

**Ordering.** `SELF_TEST_COUNT` is `4` from Task 2 and `5` from Task 4; the battery derives its
mutant list from `run_self_tests`' body, so Task 4 extends it with no edit to Task 3's code.
Task 1 must land first — Task 5's pin is unreachable without it.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
