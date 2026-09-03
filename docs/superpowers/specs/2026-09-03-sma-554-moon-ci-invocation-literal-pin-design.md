# SMA-554 — Pin `ci.yml`'s `moon ci` invocation as an exact literal instead of pattern-matching it

**Status:** revised after adversarial review (2026-09-03)
**Linear:** [SMA-554](https://linear.app/smaschek/issue/SMA-554/repo-pin-ciymls-moon-ci-invocation-lines-as-exact-literals-instead-of)
**Related:** SMA-541 (added C5, the check this replaces), **SMA-542** (added checks 8b and 8d in
`ci/actionlint/run.sh`, which already guard the same eight lines — see E0), SMA-553 / SMA-579 /
SMA-593 / SMA-539 (the sibling exact-literal pins in `ci_targets.py` this mirrors)

## 1. Problem

`check_invocation` in `ci/affected-graph/ci_targets.py` (C5, added by SMA-541) asserts that
`ci.yml`'s `moon ci` invocations are actually handed the whole `T` array. It does this by
**pattern-matching the invocation line**:

```python
MOON_CI_LINE_RE = re.compile(r"^[ \t]*moon[ \t]+ci\b.*$", re.MULTILINE)
T_ARRAY_EXPANSION = '"${T[@]}"'
lines = MOON_CI_LINE_RE.findall(ci_yml_text)
rows = [line.strip() for line in lines if T_ARRAY_EXPANSION not in _strip_comment(line)]
```

That approach was bypassed **four separate times** during SMA-541's own review, each bypass
verified real before it was fixed:

1. `moon ci "${T[@]:0:5}" …` — subsetted array (the original motivation)
2. `moon ci --base origin/main "${T[@]:0:5}" …` — subsetted behind a leading flag
3. `echo moon ci "${T[@]}" …` (non-executing) and `moon    ci "${T[@]:0:5}" …` (multi-space)
4. `moon ci "${T[@]:0:5}" …  # restore "${T[@]}" later` — expansion supplied by a trailing comment

Every one was "the pattern did not describe the thing someone wrote". The same file contains a
controlled comparison: C4 (`RUN_SH_CALL_SITES`) matches **exact literals** and produced **one**
bypass, whose fix was to make it *more* literal. C5's pattern matching produced **four**.

**The residual this closes is narrower than the issue states, and E0 is why.** SMA-554 was filed on
2026-08-19, before SMA-542's checks 8b and 8d landed. Those two already cover all four bypasses
above, one of them behaviourally. So this is not an unguarded hole; it is the removal of the
weakest of three controls over the same eight lines, and its replacement with a form that has no
tail to enumerate — in a gate scheduled independently of the other two. §3 argues that is still
worth doing, and §6 states what it costs.

## 2. Evidence

Measured on 2026-09-03 against the tree at `4b22ace`.

**E0 — two independently-written controls already pin these lines, and the spec's first draft did
not know it.** *(Found by adversarial review; every claim here re-verified by hand.)*

- **Check 8b**, `ci/actionlint/run.sh:1692-1696`. `T_INVOCATION_ALLOWLIST` is an exact-literal,
  indentation-included pin of all three `"${T[@]}"` lines, with the count of such lines pinned to
  the array's length. Its own comment says they are *"copied VERBATIM from
  .github/workflows/ci.yml, indentation included"*. `ci/actionlint/README.md:35` calls it *"the
  PRIMARY guard on the INVOCATION LINES themselves"*.
- **Check 8d**, `ci/actionlint/run.sh:4025-4135` (`block_execution_verdict`). It extracts the
  `moon ci (affected graph)` step's `run:` block, dedents it, and **executes** it against a `moon`
  stubbed into a `mktemp -d` bin dir on a minimal PATH, once per GitHub event path
  (`pull_request`; `push` with a real `BEFORE`; `push` with all-zero `BEFORE`; `push` with empty
  `BEFORE`), then compares the logged argv against an expectation **derived from `T` itself**
  (`:4093-4104`). Its standing control (`:4136+`) already fixtures the `if false; then … fi` wrap
  and a branch-body swap.

Consequences, each checked against the current code:

| §1 bypass | caught today by |
| -- | -- |
| 1 subsetted | 8b (`"${T[@]}"` count falls) **and** 8d (`bad-args`) |
| 2 leading flag + subset | 8b **and** 8d |
| 3a `echo`-prefixed | 8b (`not-allowlisted`) **and** 8d (`zero-invocations`) |
| 3b multi-space | 8b **and** 8d |
| 4 trailing comment | 8b (`not-allowlisted`) |

Deleting 8d is not free either: `SELF_TEST_COUNT=13` (`ci/actionlint/run.sh:48`) asserts both the
count of `*_self_test` **definitions** and their invocations, so removing
`block_execution_self_test` reds. `ACTIONLINT_SH_CALL_SITES` in `ci_targets.py` pins
`run_self_tests` and `selftest_mutation_battery` from the other side.

**E1 — the block has changed once, and a further change is already scheduled.** *(Corrects the
first draft, which claimed it had never changed.)*
`docs/superpowers/plans/2026-05-30-sma-361-ci-workflow.md:234-241` shows the original block with
neither `moon ci` line carrying `--include-relations`; the flag was added later (SMA-528). Two of
the eight lines have therefore changed at least once. And CLAUDE.md carries a standing instruction
to re-measure that flag's effect — *"Re-run that A/B on the next moon bump — the delta moved once
and can move again"* — so an edit to exactly those two lines is foreseeable work, not hypothetical.

The `T=(…)` line directly above the block (`:234`) changes far more often: every new `repo:*` gate
appends to it.

**E2 — the issue's stated rationale for keeping the count floor is wrong, and the correction
changes the design.** SMA-554's sketch says `EXPECTED_MOON_CI_INVOCATIONS` "catches a line
vanishing entirely, which an exact-match set cannot see on its own". A **presence**-based literal
set does see a vanishing line: the literal is no longer in the file, so the check reds. What a
literal set cannot see is an **added** invocation — a third `moon ci` line elsewhere in `ci.yml`,
or a duplicate of the pinned block. That, not deletion, is the count floor's remaining job, and it
is why the floor is kept (D2) rather than dropped as redundant.

**E3 — three current fixtures assert a property this design reverses, but the *repo* already
behaves the new way.** `ci_targets.py`'s self-test currently asserts each of these stays green:

| fixture | C5 today | C5 after | check 8b today |
| -- | -- | -- | -- |
| `moon ci --base origin/main "${T[@]}"` (reordered) | green | **red** | already **red** |
| `moon    ci "${T[@]}"` (multi-space, intact) | green | **red** | already **red** |
| `moon ci "${T[@]}" …  # PR path` | green | **red** | already **red** |

SMA-541 decided this explicitly — *"argument ORDER is not the property worth pinning"* — but 8b
matches each `"${T[@]}"`-carrying line against an exact allowlist, so all three are red in the
repository *today*. This change therefore reverses **one gate's fixtures**, not the repo's
behaviour, which makes it considerably more defensible than the first draft claimed.

**E4 — reachability is already in place.** `.github/workflows/ci.yml` is among
`repo:affected-smoke`'s `inputs` (`moon.yml:171`), so a PR editing the pinned block schedules the
gate that pins it. No new `inputs` entry is needed.

**E5 — nothing outside `check_invocation` uses the machinery being deleted.**
`T_ARRAY_EXPANSION` (`:81`) is read only at `:1355`; `_strip_comment` (`:1312`) only at `:1355`;
`MOON_CI_INVOCATION` (`:75`) only in the fix message at `:3081`; `MOON_CI_LINE_RE` only at `:1354`.

**E6 — `read_text()` translates newlines, so no CRLF ever reaches these checks.** *(Corrects the
first draft's D3.)* `read_input` uses `path.read_text()` (`ci_targets.py:1081`), i.e. text mode
with `newline=None`, so `\r\n` and lone `\r` become `\n` before any check sees the text —
measured. `T_ARRAY_RE`'s comment (`:60-66`), which claims a CRLF checkout reds with a misleading
message, is therefore describing an unreachable state; it is corrected as part of this change.

**E7 — `parse_t` guarantees exactly one `T=` line.** `ci_targets.py:1098-1111` raises
`GateAssertionError` unless there is exactly one `T=(...)` array *and* exactly one `T=`/`T+=`
assignment. This is what makes D7's positional anchor sound: a decoy copy of the block cannot bring
its own `T=` anchor along.

## 3. Approach

Replace C5's shape rule with a **positionally-anchored, contiguous exact-literal block pin**, and
demote the existing regex to a cardinality counter.

`check_invocation(ci_yml_text)` becomes two assertions over the same text:

- **A — anchored block pin.** A new module constant `MOON_CI_BRANCH_BLOCK` holds E1's eight lines
  verbatim, leading whitespace included. The check requires them to appear as a consecutive,
  in-order, byte-identical run of lines **beginning on the line immediately after the single
  `T=(…)` line** (D7).
- **B — extras counter.** `MOON_CI_LINE_RE` and `EXPECTED_MOON_CI_INVOCATIONS` survive, but only to
  count command-position `moon ci` lines and require exactly 2.

`T_ARRAY_EXPANSION`, `_strip_comment` and `MOON_CI_INVOCATION` are deleted.

### What this is for, stated honestly

Given E0, A is **not** the thing standing between the repo and an under-run graph — 8d is, and it
proves control flow behaviourally in a way no literal can. A's justification is the one this repo
accepts elsewhere (SMA-542's mutual guarding): a second, independently-scheduled assertion so no
single gate is the sole judge, plus the removal of the last pattern-matched shape rule from
`ci_targets.py`. §6 states the cost of that honestly.

### Why the block, not the two lines

Pinning only the two `moon ci` lines (the issue's literal sketch) leaves the branch **conditions**,
the `moon run "${T[@]}"` else-branch, and the **ordering and adjacency** of all of it unpinned by
this gate. Pinning a contiguous block also removes the last locator pattern: there is no "find the
step" step, only "does this exact sequence appear at this position", so failing to find it is a red
rather than a silently-empty match set.

The `T=(…)` line is deliberately **outside** the block (though A anchors to it). Including it would
red this gate on every new `repo:*` gate — the single most routine edit in this repo — and
duplicate what C1–C3 already assert about the array's contents.

### Alternatives rejected

- **Do nothing; close SMA-554 as covered by 8b/8d.** Defensible on E0, and the honest fallback if
  the fourth update site in §6 is judged not worth it. Rejected because C5's pattern rule stays in
  the tree either way, and a rule that reads as a guard while being the weakest of three is worse
  than either a strong guard or no guard.
- **Delete C5's shape rule entirely, keep only the counter.** Rejected: it removes the
  independently-scheduled second opinion and leaves a regex behind anyway, so it neither ends the
  pattern tail nor simplifies the story.
- **Pin the two lines as unordered set membership** (the issue's sketch as written). Rejected:
  strictly weaker than the block, reds on the same edits, and duplicates 8b exactly rather than
  adding a different kind of assertion.
- **Set-equality against an allowlist of every `ci.yml` line mentioning `moon ci`.** Rejected:
  `ci.yml` carries several prose comments and two `name:` fields mentioning it, so every comment
  reword would red the gate.

## 4. Design decisions

- **D1 — whole-line comparison, unstripped.** Two real reasons, neither of which is the one the
  first draft gave. *(Corrected: bash nesting comes from the `if`/`elif`/`fi` keywords, not
  indentation, and YAML block scalars strip the common indent set by the first non-empty line — a
  uniform shift of all eight lines yields a byte-identical script.)* (i) An indentation change can
  move a line **out of** the `run:` block scalar entirely, which YAML does see; keeping the pin
  unstripped makes any such change a red. (ii) An unstripped constant is copy-pasteable verbatim
  from `ci.yml`, which is exactly the rule `T_INVOCATION_ALLOWLIST` states for itself — *"do not
  hand-format a new entry"* — and keeping both pins on the same rule is what makes co-updating them
  mechanical. Note explicitly: it is **whole-line** matching, not unstrippedness, that rejects a
  commented-out copy.
- **D2 — keep the counter, demoted to extras-only.** B defines no shape rule any more, so its known
  false negatives stop being holes in a guard and become stated limitations (L1). It is the only
  half that can see an added invocation (E2). A deliberate third invocation reds and must be
  reviewed — the same default-deny stance as SMA-541's D10. B deliberately counts `moon ci` only:
  the `moon run` else-branch line is pinned by A and by 8b, and widening the counter to `moon run`
  would red on any unrelated `moon run` in the file.
- **D3 — split lines on `"\n"`, not `splitlines()`.** One reason, not two: `str.splitlines()` also
  splits on `\x0b`, `\x0c`, `\x1c`–`\x1e`, U+2028 and U+2029, while the counter's `re.MULTILINE`
  anchors split only on `\n`, so the two halves of one check would disagree about what a line is.
  *(The first draft also claimed a CRLF benefit. E6 shows CRLF never reaches the check; no
  `rstrip("\r")` is added, and the stale `T_ARRAY_RE` comment is corrected instead.)*
- **D4 — report a unified diff.** On mismatch emit `difflib.unified_diff(expected, actual_window)`,
  where `actual_window` is the eight lines at the anchor. A diff is correct under insertion, where
  positional side-by-side scoring reads worse than no diff at all. The message names
  `MOON_CI_BRANCH_BLOCK` in `ci/affected-graph/ci_targets.py` **and** the co-update sites from §6,
  so a human editing the block is told every place that must move together (AC #2). A fixture
  asserts the constant's name appears in the emitted rows.
- **D5 — leave `assert_include_relations` (`ci/affected-graph/run.sh:179`) alone.** Kept as
  cost-free redundancy, *not* for coverage. *(Corrected: its grep is `moon ci +"`, which SMA-541
  measured as blind to a leading flag, so the first draft's claim that it "covers invocations
  outside the pinned block" does not survive scrutiny.)*
- **D6 — invert the three reversed fixtures rather than delete them,** with a comment recording
  that SMA-541 decided the opposite and that E3 shows 8b already behaves the new way. That is what
  stops a deliberate reversal being re-litigated as a bug.
- **D7 — anchor A to the `T=(…)` line.** The block must start on the line immediately following the
  sole `T=` assignment, which E7 shows is unique. This closes the **decoy** family — a verbatim
  copy of the eight lines pasted into an unrelated step, satisfying an unanchored whole-file search
  while the real step is rewritten — and subsumes L4 of the first draft (the block's placement
  relative to `T` was previously asserted by nothing). It costs no extra maintenance: any edit that
  moves the block already reds A.
- **D8 — assert the two constants agree, at module scope.** `MOON_CI_BRANCH_BLOCK` must contain
  exactly `EXPECTED_MOON_CI_INVOCATIONS` lines matching `MOON_CI_LINE_RE`. B's "exactly 2" only
  means "the block's two and no others" because that happens to hold today; nothing asserted it. A
  future edit changing the block's invocation count while updating only one constant would leave B
  silently meaning something else. This is the same drift class the rest of the file guards with
  floors.
- **D9 — `MOON_CI_BRANCH_BLOCK` is a tuple of eight strings,** not one triple-quoted blob. Fixtures
  derive their text from it by joining, and D4's diff consumes it directly.
- **D10 — A requires the anchored occurrence, and says nothing about others.** A verbatim second
  copy elsewhere in the file is not reported by A (D7 makes it harmless), but it *is* reported by B
  if it carries `moon ci` lines, and by 8b's count. Stated so the multiplicity question is not left
  to the reader.

## 5. The checks, restated

- **A — anchored block pin.** `MOON_CI_BRANCH_BLOCK`'s eight lines appear as a consecutive,
  in-order, byte-identical run beginning immediately after the sole `T=(…)` line. Failure emits a
  unified diff against the eight lines actually found there, names the constant, and lists the
  co-update sites.
- **B — extras counter.** Exactly `EXPECTED_MOON_CI_INVOCATIONS` (2) lines in the whole file match
  `MOON_CI_LINE_RE`. Failure reports the count found and states that a deliberate new invocation
  means updating the constant.

**Soundness, stated rather than left to the reader:** B's bound is meaningful only because A's
block contributes exactly two matching lines (D8 asserts this), so "exactly 2 in the file" is
equivalent to "none outside the block".

## 6. Trade-off, stated

Any legitimate edit to those eight lines reds the gate until someone updates
`MOON_CI_BRANCH_BLOCK`. E1 shows such an edit is foreseeable — a moon bump re-opens the
`--include-relations` A/B by CLAUDE.md's own standing instruction.

**That edit already touches three sites; this makes it four:**

1. `.github/workflows/ci.yml:235-242` — the block itself.
2. `T_INVOCATION_ALLOWLIST`, `ci/actionlint/run.sh:1692-1696` — up to three entries.
3. Check 8d's derived expectation, `ci/actionlint/run.sh:4093-4104`, plus its
   `healthy`/`subset`/`branch_swap` fixtures.
4. **New:** `MOON_CI_BRANCH_BLOCK` in `ci/affected-graph/ci_targets.py`.

D4's message is what keeps that from being a discovery exercise: it names all four. A reciprocal
comment at `MOON_CI_BRANCH_BLOCK` names 8b and 8d, so nobody later deletes 8d on the grounds that
`ci_targets.py` now pins the same lines.

The pinned surface is wider than the issue's two-line sketch: argument order, inter-word spacing,
trailing comments, **trailing whitespace**, indentation, the branch conditions and the block's
position relative to `T` all become pinned by this gate. E3 shows the first three are already red
in the repo via 8b, so the marginal new surface is smaller than it looks.

## 7. Test plan

All fixtures live in `ci_targets.py`'s `--self-test`, which `repo:affected-smoke`'s
`--negative-control` invocation executes (`ci/affected-graph/run.sh:413`).

**Fixture construction is a requirement, not an implementation detail.** Every fixture text is
**derived** from `MOON_CI_BRANCH_BLOCK` — `canonical = "\n".join(MOON_CI_BRANCH_BLOCK)`, embedded
in a synthetic step carrying a `T=(…)` anchor line, then mutated. *(The existing fixtures mutate a
hand-written 7-line `invoked` string at `ci_targets.py:1886-1894` that has no `elif` branch and no
comment line. Under A, every mutation of that string would red because the canonical block was
never present — the mutation would assert nothing. This is the SMA-526 vacuous-fixture class, and
D6's three inverted fixtures would be its worst case: they are meant to prove the E3 reversal is
deliberate, and would red identically if the reversal were undone.)*

**Every red row is paired with its own revert-to-green twin**, asserting the mutation — not the
scaffolding — is what reds it.

**Must report red:**

| # | fixture | caught by |
| -- | -- | -- |
| 1 | `moon ci "${T[@]:0:5}" …` (issue bypass 1) | A |
| 2 | `moon ci --base origin/main "${T[@]:0:5}" …` (bypass 2) | A |
| 3 | `echo moon ci "${T[@]}" …` (bypass 3a) | A |
| 4 | `moon    ci "${T[@]:0:5}" …` (bypass 3b) | A |
| 5 | `moon ci "${T[@]:0:5}" …  # restore "${T[@]}" later` (bypass 4) | A |
| 6 | unquoted `moon ci $T …` | A |
| 7 | `moon ci :build …` (bypasses `T` entirely) | A |
| 8 | one invocation line deleted (AC #3) | A **and** B |
| 9 | a third `moon ci "${T[@]}" …` added elsewhere | B only |
| 10 | canonical block intact **and** a subsetted invocation added (dead-branch case) | B only |
| 11 | a line inside the block re-indented | A |
| 12 | a line inserted mid-block (contiguity) | A |
| 13 | E3's three reversed cases: reordered-canonical, multi-space-intact, trailing `# PR path` | A |
| 14 | trailing whitespace added to one line | A |
| 15 | **decoy**: the block rewritten in place, a verbatim copy pasted elsewhere (D7) | A |
| 16 | the block moved away from the `T=` line, everything else identical (D7) | A |
| 17 | D4's message for any of the above does **not** name `MOON_CI_BRANCH_BLOCK` | meta-fixture |

**Must report clean:**

| # | fixture |
| -- | -- |
| 18 | the canonical block at the anchor — a **tautology** by construction (derived from the constant); its non-vacuous twin is the production run in AC #4, not this row |
| 19 | prose comments and `name:` fields mentioning `moon ci` around the block (B must not miscount them) |
| 20 | an added `$MOON ci "${T[@]:0:5}"` — **documents L1**, in the both-directions style this file already uses |

**Plus** the module-scope D8 invariant, asserted as the first self-test row.

**Acceptance, beyond the fixtures:**

- `moon run repo:affected-smoke --force` passes on the unmutated tree (AC #4).
- `moon run repo:actionlint --force` passes — 8b/8d must be unaffected.
- `moon run repo:ruff-ci --force` passes — `ci_targets.py` is in its corpus.
- The full CI target graph runs clean before the PR is opened.

## 8. Documentation

- `ci/affected-graph/README.md:301,311` — C5's description and its "line matcher is …" note.
- `ci/affected-graph/ci_targets.py:1330-1352` — `check_invocation`'s docstring argues the
  **opposite** of the new design ("Argument ORDER is not the property worth pinning") and is
  rewritten, not edited.
- `ci/affected-graph/ci_targets.py:3074-3086` — the fix-message block has three clauses that become
  dead: the `MOON_CI_INVOCATION` reference, the "(no … invocation anywhere in the file)" row shape,
  and the already-stale "quote-gated regex" clause.
- `ci/affected-graph/ci_targets.py:60-66` — `T_ARRAY_RE`'s CRLF comment, corrected per E6.
- `docs/…/2026-08-19-sma-541-ci-target-coverage-gate-design.md` — the C5 entry (`:257-286`) and the
  fixture rows (`:356-358`) annotated as **superseded**; L10 (`:432`) annotated as **still open**
  and cross-referenced to `ci/actionlint/README.md` L12 and check 8d. *(Corrected: L10 says C5
  cannot see a step-level `if: false`, which remains true after this change — annotating it
  superseded would delete a live limitation.)*
- `CLAUDE.md` — **no change needed.** Measured: it names neither C5 nor `check_invocation` nor any
  deleted constant. Its marker-delimited command and `T`-array bullets are untouched.

## 9. Limitations

- **L1 — B's recogniser still has false negatives.** `$MOON ci "${T[@]:0:5}"`, a `FOO=1 moon ci …`
  env-assignment prefix, a `moon()` shell-function shadow, and a `\`-continued invocation are not
  counted, so such an *added* invocation is invisible to B. Unchanged from today; what changes is
  that it no longer affects a shape rule. **Check 8d catches three of the four behaviourally** —
  the `FOO=1 moon ci …` prefix, the `moon()` shadow, and the `\`-continued invocation — by
  executing the block and counting real `moon` invocations. Measured false for `$MOON ci …`
  specifically: appended after the block's `fi`, 8d logs exactly one correct invocation and
  reports clean (the `set -u` abort on the undefined `$MOON` happens after the real call is
  already counted). `$MOON ci` is uncaught by any control in the repo.
- **L2 — A proves the block is present at the anchor, not that it executes.** At least three
  constructions defeat both A and B; this list is not exhaustive. A **step-level
  `if: ${{ false }}`** on the `- name: moon ci (affected graph)` step (SMA-541's own L10;
  `ci/actionlint/README.md:296-304`) and an **`if false; then … fi` wrap** of the block with an
  L1-invisible invocation added both leave all eight lines byte-identical and the count at 2. Only
  the wrap is closed: check 8d executes the block against a stubbed `moon` and catches it,
  fixtured in both directions. The step-level `if:` is closed by **nothing** — 8d's
  `extract_moon_step_block` skips every step key that is not `run:` while seeking the block, so a
  step-level `if:` leaves the extracted text unchanged and 8d reports clean on all four event
  paths regardless.

  A third construction, independently verified with real bash, defeats every control in the repo,
  this one included:

  ```yaml
        run: |
          set -euo pipefail
          trap 'exit 0' ERR          # inserted ABOVE the T=(…) anchor
          T=(:build … :ruff-ci)
          if [ "$EVENT" = "pull_request" ]; then
          …
  ```

  `set -euo pipefail` plus `trap 'exit 0' ERR` makes a failing command exit **0** (without the
  trap line the same script exits 1). It defeats C5's A (the trap sits above the anchor, so the
  eight pinned lines still start at anchor+1, byte-identical), C5's B (still 2 invocations), check
  8's `swallowed`/`block-swallowed` (need a `||`/`&&`/`;`/`|` tail) and `wrapped` (its token
  vocabulary — `command|env|time|eval|exec|if|while|until|!` — has no `trap`), 8b (no `"${T[@]}"`
  on the trap line), and 8d (its stub exits 0, so the trap never fires). A two-line variant is
  `set -uo pipefail` plus a trailing `exit 0` after `fi`. This is a **pre-existing repo hole, not
  introduced by this branch**, and it remains unclosed today. *(The first draft claimed no such
  construction was known. It was wrong, and the correction is the main reason §3 reframes A as
  defence in depth.)*
- **L3 — nothing pins `check_invocation`'s own call site.** `ci_targets.py:2958` calls it and
  `:3074` reports it; deleting both in one edit leaves the gate green. This is the same shape as
  L6 in `ci/actionlint/README.md` and is not addressed here.
- **L4 — the co-update relationship between the four sites in §6 is documentation, not a gate.**
  D4's message and the reciprocal comments make it discoverable; nothing asserts that
  `MOON_CI_BRANCH_BLOCK` and `T_INVOCATION_ALLOWLIST` agree. A gate for that is a reasonable
  follow-up and is out of scope here.
