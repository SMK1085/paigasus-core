# SMA-554 — Pin `ci.yml`'s `moon ci` invocation as an exact literal instead of pattern-matching it

**Status:** draft
**Linear:** [SMA-554](https://linear.app/smaschek/issue/SMA-554/repo-pin-ciymls-moon-ci-invocation-lines-as-exact-literals-instead-of)
**Related:** SMA-541 (added C5, the check this replaces), SMA-542 / SMA-553 / SMA-579 / SMA-593 /
SMA-539 (the sibling exact-literal pins in `ci/affected-graph/ci_targets.py` this mirrors)

## 1. Problem

`check_invocation` in `ci/affected-graph/ci_targets.py` (C5, added by SMA-541) asserts that
`ci.yml`'s `moon ci` invocations are actually handed the whole `T` array. C1–C3 assert what is
*in* `T`; C5 is the only check asserting `T` is what *runs*. Subsetting the expansion to
`"${T[@]:0:5}"` leaves every other check green and switches most of the gate graph off.

C5 does this by **pattern-matching the invocation line**:

```python
MOON_CI_LINE_RE = re.compile(r"^[ \t]*moon[ \t]+ci\b.*$", re.MULTILINE)
T_ARRAY_EXPANSION = '"${T[@]}"'
lines = MOON_CI_LINE_RE.findall(ci_yml_text)
rows = [line.strip() for line in lines if T_ARRAY_EXPANSION not in _strip_comment(line)]
```

That approach was bypassed **four separate times** during SMA-541's own review, each bypass
verified real before it was fixed:

1. `moon ci "${T[@]:0:5}" …` — subsetted array (the original motivation)
2. `moon ci --base origin/main "${T[@]:0:5}" …` — subsetted behind a leading flag; seen by neither
   C5 nor `assert_include_relations`
3. `echo moon ci "${T[@]}" …` (non-executing) and `moon    ci "${T[@]:0:5}" …` (multi-space)
4. `moon ci "${T[@]:0:5}" …  # restore "${T[@]}" later` — expansion supplied by a trailing comment

Every one was "the pattern did not describe the thing someone wrote". The same file contains a
controlled comparison: C4 (`RUN_SH_CALL_SITES`) matches **exact literals** and produced **one**
bypass, whose fix was to make it *more* literal (adding the `|| NEG_RC=1` suffix). C5's pattern
matching produced **four**. There is no reason to believe a fifth does not exist.

## 2. Evidence

Measured on 2026-09-03 against the tree at `4b22ace`.

**E1 — the block is stable.** `.github/workflows/ci.yml:235-242` holds the whole branch structure:

```
          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]}" --base origin/main --include-relations
          elif [ -n "${BEFORE:-}" ] && ! printf '%s' "$BEFORE" | grep -qE '^0+$'; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
            # Initial push with no usable base — run the whole graph to warm caches.
            moon run "${T[@]}"
          fi
```

Those eight lines have not changed in the repo's history. The `T=(…)` line directly above them
(`:234`) has changed repeatedly — every new `repo:*` gate appends to it.

**E2 — the issue's stated rationale for keeping the count floor is wrong, and the correction
changes the design.** SMA-554's sketch says `EXPECTED_MOON_CI_INVOCATIONS` "catches a line
vanishing entirely, which an exact-match set cannot see on its own". A **presence**-based literal
set does see a vanishing line: the literal is no longer in the file, so the check reds. What a
literal set genuinely cannot see is an **added** invocation — a third `moon ci` line elsewhere in
`ci.yml`, or a duplicate of the pinned block. That, not deletion, is the count floor's remaining
job, and it is why the floor is kept (§4, D2) rather than dropped as redundant.

**E3 — three current fixtures assert a property this design deliberately reverses.**
`ci_targets.py`'s self-test currently asserts that each of these stays **green**:

| fixture | status today | status after |
| -- | -- | -- |
| `moon ci --base origin/main "${T[@]}"` (reordered, canonical) | green | **red** |
| `moon    ci "${T[@]}"` (multi-space, array intact) | green | **red** |
| `moon ci "${T[@]}" … --include-relations  # PR path` | green | **red** |

SMA-541 decided this explicitly: *"argument ORDER is not the property worth pinning — handing over
the whole array is"*. Under an exact literal, order, spacing and trailing comments **are** pinned.
This is a real reversal, not an incidental one, and §6 states the trade-off.

**E4 — reachability is already in place.** `.github/workflows/ci.yml` is among
`repo:affected-smoke`'s `inputs` (`moon.yml:171`), so a PR editing the pinned block schedules the
gate that pins it. No new `inputs` entry is needed.

**E5 — nothing outside `check_invocation` uses the machinery being deleted.**
`T_ARRAY_EXPANSION` (`:81`) is read only at `:1355`; `_strip_comment` (`:1312`) only at `:1355`;
`MOON_CI_INVOCATION` (`:75`) only in the fix message at `:3081`. `MOON_CI_LINE_RE` is read only at
`:1354`.

## 3. Approach

Replace C5's shape rule with a **contiguous exact-literal block pin**, and demote the existing
regex to a cardinality counter.

`check_invocation(ci_yml_text)` becomes two independent assertions over the same text:

- **A — block pin.** A new module constant `MOON_CI_BRANCH_BLOCK` holds E1's eight lines verbatim,
  leading whitespace included. The check requires them to appear as a **consecutive, in-order
  sublist** of `ci.yml`'s lines, each compared as a whole line with no stripping.
- **B — extras counter.** `MOON_CI_LINE_RE` and `EXPECTED_MOON_CI_INVOCATIONS` survive, but only
  to count command-position `moon ci` lines and require exactly 2.

`T_ARRAY_EXPANSION`, `_strip_comment` and `MOON_CI_INVOCATION` are deleted.

### Why the block, not the two lines

Pinning only the two `moon ci` lines (the issue's literal sketch) leaves three things unpinned that
the block covers for free: the branch **conditions**, the `moon run "${T[@]}"` else-branch, and the
**ordering and adjacency** of all of it. Rewriting `if [ "$EVENT" = "pull_request" ]` is not
something any current check would see. Pinning a contiguous block also removes the last locator
pattern from the check: there is no "find the step" step, only "does this exact sequence of lines
appear", so a failure to find it is a red rather than a silently-empty match set.

The `T=(…)` line is deliberately **outside** the block. Including it would red this gate on every
new `repo:*` gate — the routine, expected edit — and duplicate what C1–C3 already assert about the
array's contents.

### Alternatives rejected

- **Pin the two `moon ci` lines as unordered set membership** (the issue's sketch as written).
  Rejected: strictly weaker than the block for no reduction in maintenance cost. Both approaches
  red on the same routine edits; the block simply covers more.
- **Drop the counter entirely and let the block pin be the whole gate.** Rejected on E2: a third
  `moon ci` added elsewhere in `ci.yml`, or a copy-pasted duplicate of the block, would be
  invisible.
- **Set-equality against an allowlist of every line in `ci.yml` mentioning `moon ci`.** Rejected:
  `ci.yml` carries six prose comments and two `name:` fields mentioning `moon ci`, so every
  comment reword would red the gate. The cost is real and the benefit is covered by B.

## 4. Design decisions

- **D1 — whole-line comparison, unstripped.** Unlike `RELEASE_PARITY_SH_CALL_SITES` and its
  siblings, which strip both sides because their real lines sit at varying indentation inside
  `case` arms, this block's indentation is **fixed and meaningful**: it is a YAML block scalar, so
  the 10/12-space indent is part of what makes the `if`/`elif` bodies nest as they do. Comparing
  unstripped also means a commented-out copy (`          # moon ci "${T[@]}" …`) cannot satisfy the
  pin, the same property the stripped-whole-line haystacks buy from whole-line matching.
- **D2 — keep the counter, demoted to extras-only.** B no longer defines any shape rule, so its
  known false negatives stop being holes in the gate and become stated limitations (L1). It is the
  only half that can see an added invocation (E2). A deliberate third invocation reds and must be
  reviewed — the same default-deny stance SMA-541's D10 takes on a project-scoped `T` entry.
- **D3 — split lines on `"\n"` with a per-line `rstrip("\r")`, not `splitlines()`.** Two reasons.
  `str.splitlines()` also splits on `\x0b`, `\x0c`, `\x1c`–`\x1e`, U+2028 and U+2029, while the
  counter's `re.MULTILINE` anchors split only on `\n` — so the two halves of one check would
  disagree about what a line is. And it makes CRLF work: today's `T_ARRAY_RE` carries a comment
  admitting that on a CRLF checkout the gate reds with a misleading "must stay on one line"
  message. `rstrip("\r")` makes CRLF a non-event instead.
- **D4 — report the best-matching window.** On mismatch, score every 8-line window by how many of
  its lines match the expectation and report the highest-scoring one alongside the expected block,
  so the diff is visible at a glance. If the best score is 0, say the block is absent entirely
  rather than printing an arbitrary window. The message names `MOON_CI_BRANCH_BLOCK` in
  `ci/affected-graph/ci_targets.py` as the constant to update when the edit is deliberate
  (AC #2). Cost is O(lines × 8) over a ~400-line file — negligible.
- **D5 — leave `assert_include_relations` (`ci/affected-graph/run.sh:179`) alone.** The pinned
  block contains `--include-relations` twice verbatim, so that assertion is now largely redundant.
  It is kept: it is scheduled independently of `ci_targets.py`, it costs nothing, and its contract
  ("*every* `moon ci` invocation carries the flag") covers invocations outside the pinned block
  that A cannot see.
- **D6 — invert the three reversed fixtures rather than delete them.** E3's three cases now assert
  red. Keeping them as inverted fixtures, with a comment recording that SMA-541 decided the
  opposite and why this issue overrides it, is what stops the reversal from being re-litigated as a
  bug later.

## 5. The checks, restated

- **A — block pin.** `MOON_CI_BRANCH_BLOCK`'s eight lines appear as a consecutive, in-order,
  byte-identical run of lines in `ci.yml`. Failure prints the expected block and the
  best-matching actual window, and names the constant to update.
- **B — extras counter.** Exactly `EXPECTED_MOON_CI_INVOCATIONS` (2) lines match
  `MOON_CI_LINE_RE`. Failure reports the count found and states that a deliberate new invocation
  means updating the constant.

Both are evaluated on every call; a tree can fail either, both, or neither.

## 6. Trade-off, stated

Any legitimate edit to those eight lines reds the gate until someone updates
`MOON_CI_BRANCH_BLOCK`. That is intended: an edit to how CI invokes its entire gate graph *should*
stop and make a human look. E1 records that the block has never changed. The surface is wider than
the issue's two-line sketch — argument order, inter-word spacing, trailing comments, indentation
and the branch conditions all become pinned (E3) — and each is a case where a human being asked to
confirm the edit is the correct outcome.

## 7. Test plan

All fixtures live in `ci_targets.py`'s `--self-test`, which `repo:affected-smoke`'s
`--negative-control` invocation executes (`ci/affected-graph/run.sh:413`).

**Must report red:**

| # | fixture | caught by |
| -- | -- | -- |
| 1 | `moon ci "${T[@]:0:5}" …` (issue bypass 1) | A |
| 2 | `moon ci --base origin/main "${T[@]:0:5}" …` (bypass 2) | A |
| 3 | `echo moon ci "${T[@]}" …` (bypass 3a, non-executing) | A |
| 4 | `moon    ci "${T[@]:0:5}" …` (bypass 3b, multi-space) | A |
| 5 | `moon ci "${T[@]:0:5}" …  # restore "${T[@]}" later` (bypass 4) | A |
| 6 | unquoted `moon ci $T …` | A |
| 7 | `moon ci :build …` (bypasses `T` entirely) | A |
| 8 | one invocation line deleted (AC #3) | A **and** B |
| 9 | a third `moon ci "${T[@]}" …` added elsewhere | B only |
| 10 | canonical block intact **and** a subsetted invocation added (dead-branch case) | B only |
| 11 | a line inside the block re-indented | A |
| 12 | a line inserted mid-block (contiguity) | A |
| 13 | E3's three reversed cases: reordered-canonical, multi-space-intact, trailing `# PR path` | A |

**Must report clean:**

| # | fixture |
| -- | -- |
| 14 | the canonical block |
| 15 | the canonical block with CRLF line endings (D3) |
| 16 | the block preceded by prose comments and `name:` fields mentioning `moon ci` (B must not miscount them) |

**Acceptance, beyond the fixtures:**

- `moon run repo:affected-smoke --force` passes on the unmutated tree (AC #4).
- `moon run repo:ruff-ci --force` passes — `ci_targets.py` is in its corpus.
- The full CI target graph runs clean before the PR is opened.

## 8. Documentation

- `ci/affected-graph/README.md:301,311` — C5's description and its "line matcher is …" note.
- `docs/superpowers/specs/2026-08-19-sma-541-ci-target-coverage-gate-design.md` — the C5 entry
  (`:257-283`), the fixture table rows (`:356-358`), and limitation L10 (`:432`), each annotated as
  superseded by this issue rather than rewritten, so SMA-541's reasoning stays readable.
- `CLAUDE.md` — **no change needed**. Measured: it names neither C5 nor `check_invocation` nor any
  of the deleted constants. Its marker-delimited command and its `T`-array bullets are untouched by
  this change.

## 9. Limitations

- **L1 — B's recogniser still has false negatives.** `$MOON ci "${T[@]:0:5}"` and other
  command-position spellings the regex does not match are not counted, so such an *added*
  invocation is invisible. This is a real hole, unchanged from today; what changes is that it no
  longer affects the shape rule, only the extras count.
- **L2 — A proves the block is present, not that it executes.** The pinned block parked inside
  `if false; then … fi` with a subsetted invocation added elsewhere passes A. B catches that
  particular shape (fixture 10) by counting three invocations; a variant that keeps the count at
  two — deleting one canonical line and re-adding it in the dead branch — reds A instead. A
  construction defeating both is not currently known but is not excluded.
- **L3 — nothing pins `check_invocation`'s own call site.** `ci_targets.py:2958` calls it and
  `:3074` reports it; deleting both lines leaves the gate green. This is the general shape of
  L6 in `ci/actionlint/README.md` and is not addressed here.
- **L4 — the `T=(…)` line is deliberately unpinned by A** (§3). Its contents are asserted by
  C1–C3; its *placement* relative to the block is not asserted by anything.
