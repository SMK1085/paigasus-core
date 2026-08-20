# actionlint gate

Lints `.github/workflows/**`, proves every `paths:` filter glob still matches the tree, and
proves every `branches:` filter entry names a branch that exists.

## Why

A `paths:` filter that comes to match nothing does not error. The workflow stops running,
forever, with no red check and no notification — `prebuild.yml` triggers only on
push-to-`main`, `workflow_dispatch` and a narrow `pull_request` filter, so its 7-platform
verification would silently cease. See SMA-525 and
`docs/superpowers/specs/2026-08-16-sma-525-actionlint-gate-design.md`.

actionlint alone is **not** sufficient: it validates syntax and has no view of the file tree,
so a valid-but-never-matching glob (`rz/**`) passes it cleanly. Checks 5–7 close that.

`branches:` has the identical property and was SMA-525's stated limitation L5. `branches: [mian]`
is a valid glob, actionlint accepts it, and the workflow stops running — silently and permanently,
one key over. All three workflows here trigger off a `branches:` filter naming `main`, including
the required check. See SMA-540 and
`docs/superpowers/specs/2026-08-19-sma-540-branches-filter-gate-design.md`.

## The checks

| # | Check |
|---|---|
| 1 | `actionlint` over the auto-discovered workflow set |
| 2 | `.github/actionlint.{yaml,yml}` declares nothing but `self-hosted-runner`, and no `ignore` key in any style (either would neuter check 1 invisibly) |
| 3 | Four stdin fixtures, one per defect class, each must fail **with its expected rule tag** |
| 4 | A healthy stdin fixture must pass — the control for check 3 |
| 5 | Every `paths:` glob is in the supported vocabulary and matches the tracked tree, and every `branches:` entry resolves as a ref or is skip-listed |
| 6 | Every extracted filter key carries at least one sequence entry; a `paths:`/`branches:` key must also have at least one of them positive (the `-ignore` variants are exempt) |
| 7 | Eight self-tests against fixture tables — extractor, path-filter verdicts, branch-filter verdicts, config allowlist, ci-target floor, invocation allowlist, affected-graph wiring, kill predicate — plus a counter (`SELF_TESTS_RAN`/`SELF_TEST_COUNT`) asserting all eight ran, and a definition-count check catching a ninth table that is defined but never wired into `run_self_tests` (`run.sh --self-test`) |
| 8 | `ci.yml`'s `T=(…)` still schedules the gate that guards `T` itself, and nothing silences that gate's result. Six verdict families: **(a)** the floor — `:affected-smoke` present in `T` (`missing`), or the array can't even be read (`no-array`/`no-file`); **(b)** no `moon` command line is continued onto another physical line, where a discarded exit status would be invisible to this check (`continued`); **(c)** no single-line `moon` command line discards its own exit status (`swallowed`), with `SWALLOWED_SKIP` as the escape hatch for an unrelated `moon` line this check cannot know is harmless; **(d)** no line CLOSING a block (`fi`/`done`/`}`) discards its own exit status either, the same tail on a different line (`block-swallowed`), sharing `SWALLOWED_SKIP`; **(e)** no `moon ci`/`moon run` invocation sits behind a known command wrapper (`command`/`env`/`time`/`eval`/`exec`/`if`/`while`/`until`/`!`) on the same line, where propagation cannot be confirmed (`wrapped`), sharing `SWALLOWED_SKIP` as its escape hatch; **(f)** no step's `continue-on-error:` value suppresses it — any spelling but the literal `false` (`continue-on-error`), with `COE_SKIP` as the escape hatch for an unrelated later step |
| 8b | Every line in `ci.yml` carrying the target-array expansion `"${T[@]}"` matches one of `T_INVOCATION_ALLOWLIST` (declared with `T_FLOOR`) **exactly** — indentation included — and the number of such lines matches the array's length. This is the PRIMARY guard on the INVOCATION LINES themselves (SMA-542 CodeRabbit round 3, finding B — a bare `VAR=value` assignment prefix defeated BOTH check 8's `swallowed` and `wrapped`, since it has neither `moon` at column 0 nor a recognized wrapper token there); check 8's `continued`/`swallowed`/`block-swallowed`/`wrapped` stay for their more specific diagnostics and are consulted first, so a line they already explain is not also reported here as `not-allowlisted`. It is NOT a complete guard on the step's control flow — see L12 |
| 8c | `ci/affected-graph/run.sh` still contains its own two call sites into `ci_targets.py` — `assert_ci_targets \|\| SUITE_RC=1` and `"$HERE/ci_targets.py" --self-test \|\| NEG_RC=1` — WITH each `\|\| RC=1` propagation suffix intact (`missing <site>`), and that the file itself exists and is readable (`no-file`). Closes L6 (SMA-542 residual closure, PR 150 follow-up): check 8 above pins only `:affected-smoke`'s *scheduling*; this pins the two lines that actually INVOKE the gate which, in turn, pins THIS file's own call sites back (`ci_targets.py`'s `ACTIONLINT_SH_CALL_SITES`). Scheduled independently of `ci/affected-graph/`, so it survives a deletion there that would otherwise green both directions of the cycle silently |
| 9 | A mutation battery, full-gate only: each of the eight self-test invocations inside `run_self_tests`, deleted one at a time, run concurrently against the real unmutated control — every mutant must die at the counter's own message (a kill predicate driven by its own fixture table, not merely "non-zero"), or the battery itself reds |

Only a `paths:`/`paths-ignore:`/`branches:`/`branches-ignore:` key **two levels deep** inside
`on:` — `on.<event>.paths` — is a filter. A workflow input may legitimately be *named* `paths` or
`branches`, and it sits one level deeper, under `on.workflow_dispatch.inputs`; checks 5 and 6
ignore it. This depth rule holds in flow style too,
not just block style: a top-level flow `on: { workflow_dispatch: { inputs: { paths: {...} } } }`
or an event's own flow value `push: { inputs: { paths: x } }` both correctly ignore the nested
`inputs.paths`, quoted or not — the extractor tracks brace depth rather than matching a `paths`
token at any nesting level. Conversely a flow-mapping event value, `push: { paths: [...] }`, is
not parsed for entries, so it is reported by check 6 as a key with no entries rather than skipped
in silence — same for the equivalent depth in a fully flow-style `on: { push: { paths: [...] } }`.

## Supported glob vocabulary

`git ls-files ':(glob)P'` is not a sound model of GitHub filter patterns, so check 5 accepts
only the subset where both provably agree:

- **literals** — must be an *exact* tracked file path. A bare directory name (`rs`) matches
  nothing on GitHub, though git's pathspec would match everything beneath it.
- **`dir/**`**, **`**/name`** — `**` as a whole path component.
- **`*`** within a single segment.

Rejected loudly, never guessed at: `?`, `+`, `[]`, and `**` embedded in a segment (`**.js`).

## Branch filter entries

`branches:` is read as a **block sequence** — the inline `branches: [main]` form is deliberately
not parsed and fails check 6 by design, exactly as `paths: [a, b]` does. Each entry must:

- **resolve** as `refs/remotes/origin/<name>`, or
- appear in `BRANCH_SKIP` in `run.sh` with a comment justifying it.

Local `refs/heads/*` is deliberately **not** consulted: a workflow triggers on branches as they
exist on GitHub, and a local-only branch does not. A glob metacharacter (`*`, `**`, `?`, `+`,
`[]`) makes an entry a pattern rather than a name, so it cannot be resolved and must be
skip-listed — `+` counts as a glob even though git allows it in a ref name, because GitHub reads
it as "one or more of the preceding character".

`branches-ignore:` is extracted and counted but never resolved: a typo'd exclusion makes a
workflow run *more* often, which is the fail-safe direction.

`tags:` and `tags-ignore:` are not covered — see the spec's §7 L4.

## Escape hatches

- A **new GitHub runner label** the pinned actionlint does not know: add it to
  `self-hosted-runner.labels` in `.github/actionlint.yaml`. Check 2 permits that file, and
  `self-hosted-runner` is the one top-level key it allows there.
- A **GitHub-valid pattern outside the vocabulary**: add it to `SKIP_PATTERNS` in `run.sh` with
  a comment justifying it and saying what verifies it instead.
- A **branch that does not exist yet**, or a branch pattern: add it to `BRANCH_SKIP` in `run.sh`
  with a comment justifying it and saying what verifies it instead.
- A **`moon` command line check 8 cannot know is harmless** (any `moon` line other than the one
  guarding `T`, carrying a `;`/`|`/`&&`/`||` tail for its own legitimate reason, **or** a `moon
  ci`/`moon run` line sitting behind a wrapper check 8 recognizes but this one is harmless): add
  it to `SWALLOWED_SKIP` in `run.sh` with a comment justifying it and saying what verifies its own
  failure instead — the one skip list covers both `swallowed` and `wrapped`, since they are the
  same underlying problem spelled two ways. There is deliberately **no** equivalent hatch for
  `continued` — a backslash-continued `moon` invocation is rejected outright, the same way
  `no-array` never skips; put it back on one physical line.
- A **deliberate, reviewed change to how ci.yml invokes `moon`** (a genuinely new invocation form,
  or a genuinely new number of invocations): update `T_INVOCATION_ALLOWLIST` in `run.sh`, copying
  the new line(s) VERBATIM from the file, indentation included. There is deliberately **no**
  separate skip list for check 8b — the array IS the reviewed exception mechanism, the same way
  `T_FLOOR` has none.
- **Anything worse**: drop `:actionlint` from `T=(…)` in `.github/workflows/ci.yml`. This must
  also be removed from the CLAUDE.md `ci-targets` block, since `repo:affected-smoke` asserts the
  two agree — **and** needs a `T_EXEMPT` entry in `ci/affected-graph/ci_targets.py` with a stated
  reason, or C1's strict equality reds on the now-missing entry (true since SMA-541 shipped).

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

**L6 — The cycle's second half is now closed (SMA-542 residual closure, PR 150 follow-up).**
Previously `repo:actionlint` pinned only `:affected-smoke`'s *presence in `T`* — its scheduling —
never `ci/affected-graph/run.sh`'s own two call sites into `ci_targets.py`. Check 8c above closes
that: it reads `ci/affected-graph/run.sh` directly and asserts `assert_ci_targets || SUITE_RC=1`
and `"$HERE/ci_targets.py" --self-test || NEG_RC=1` are both still present, propagation suffix
intact — mirroring `ci_targets.py`'s own `RUN_SH_CALL_SITES`, which is the ONE place that pin used
to live (and which pins `ACTIONLINT_SH_CALL_SITES`, this file's five call sites, in return). Both
directions are now guarded from a location independent of the file being guarded.

What remains, and is inherent rather than an oversight: deleting check 8c's OWN production call
site (`done < <(affected_graph_wiring_verdict ...)`, below) AND `assert_ci_targets || SUITE_RC=1`
in the SAME edit still silences both directions at once — the former is pinned by
`ci_targets.py`'s `ACTIONLINT_SH_CALL_SITES`, which runs FROM the very call the latter deletes.
This is the same shape L1 already names for `T`'s own two entries: two independently-scheduled
gates are the most the graph offers, and closing a combined deletion needs a third, which only
moves the same problem one level out. Bounded for the same reason L1 is: the two lines this
residual depends on sit next to each other inside a five-line function, not scattered across the
tree.

**L7 — `COE_SKIP` and `SWALLOWED_SKIP` are exact-text, not semantic.** Each entry is keyed by both
line number and the matched line's exact text (leading blanks included), so a later reformat of
that one line — even whitespace-only — makes the entry stop matching. That is the fail-safe
direction: the check fires again and asks for the entry to be updated, rather than silently
continuing to skip a line whose content has since changed underneath it. Both ship empty today.

**L8 — `continued` cannot see past its own continuation.** A backslash-continued `moon` line is
always rejected, whether or not a tail on a later physical line would actually have been a
problem — this check reads one physical line at a time and does not reassemble a continuation
before scanning it. Deliberate (SMA-542 fix-wave finding I2): guessing at what follows the backslash risks
the exact misdiagnosis ("swallowed" when the tail wasn't there, or vice versa) this check exists to
avoid, so it demands the invocation be rejoined onto one line instead.

**L9 — `wrapped` recognizes a closed, enumerated vocabulary, not "any wrapper," and `moon` must
be the very next command word.** Only `command`/`env`/`time`/`eval`/`exec`/`if`/`while`/`until`/`!`
are checked, only when the wrapper and the `moon ci`/`moon run` invocation share one physical line,
and only with whitespace-separated "glue" tolerated in between: further wrapper tokens
(`command env moon ci …`), a negation (`if ! moon ci …`), or a `VAR=value` assignment
(`env FOO=bar moon ci …`). A wrapper outside that list (a shell function, `sudo`, `nice`, a `case`
arm, ...), or `moon` reached through anything other than whitespace (`true && moon ci …`), is
invisible to it. A wrapper on one physical line with the invocation on a LATER one shares L8's
physical-line-only parsing — but unlike L8, where a plain backslash-continued `moon` line IS
reported (as `continued`), a wrapper split across lines this way is not reported by check 8 at
all: `command \` alone on its line has no `moon` for this pattern to match, and `moon ci …` alone
on the next line has no wrapper for `wrapped` to see (it may not even reach `swallowed` either, if
that second line carries no tail of its own). T_INVOCATION_ALLOWLIST does not close this either —
see L11. This IS what closes the
CodeRabbit round-2 false positive (`if test -n "$X"; then echo "moon ci failed"; fi` — a wrapper at
line start with "moon ci" appearing only inside a string later on the line): with `test` as the
next command word rather than `moon` or recognized glue, the pattern does not match that line at
all. Deliberate (CodeRabbit, PR 150): parsing arbitrary bash wrapping accurately needs the same
kind of control-flow analysis this file's own history shows goes wrong (flow vs block style,
SMA-525/540 rounds 2–3) — reject the enumerated shapes loudly rather than guess at the rest.

**L10 — `ci_targets.py`'s column-0 pin on `ACTIONLINT_SH_CALL_SITES` is not reachability
analysis.** It closes the common case — an indented copy of a required line, the shape wrapping
one of the three calls in a conditional block conventionally produces — but a required line copied
into an **unindented** `if false; then … fi` block, or an **unindented** heredoc, still sits at
column 0 and still satisfies the pin even though neither ever executes. Deliberate (CodeRabbit, PR
150): genuine bash-reachability analysis in Python is fragile and out of scope; see the comment at
`ACTIONLINT_SH_CALL_SITES` in `ci/affected-graph/ci_targets.py`.

**L11 — `T_INVOCATION_ALLOWLIST` matches per LINE, so a wrapper split across physical lines (L9's
own residual) slips past it too.** `invocation_allowlist_verdict` only examines lines that
literally contain `"${T[@]}"`; a line reading `command \` (the wrapper alone, continued) does not
contain that substring at all, so it is never scanned, and the SECOND line — `moon ci "${T[@]}"
--base origin/main --include-relations`, say — can be BYTE-IDENTICAL to an allowed entry once
split onto its own line this way, so it passes both the exact-match rule and the count (the line
still carries the expansion and still counts toward the total). Closing this needs either
reassembling continuations before matching (parsing the same bash control flow this file has
repeatedly gotten wrong, per L9) or literally forbidding a backslash on any line adjacent to one
containing `"${T[@]}"` — neither attempted here. Not believed reachable through an *unwrapped*
continuation: a plain `moon ci \` (no wrapper) still starts with `moon` at column 0, so check 8's
`continued` verdict catches THAT shape before `invocation_allowlist_verdict` ever needs to (see
Check 8b's row in the table above — `continued`/`swallowed`/`wrapped` are consulted first).

**L12 — check 8b pins the invocation LINES, not the control flow around them; it is not a
complete guard on the step, and the table calling it "the PRIMARY guard" should be read that
narrowly.** Two shapes independently reviewed and found to still leave the gate at rc 0 while
silencing every gate in `T` (SMA-542, independent review of PR 150, finding I4):

```
fi || true      # ci.yml's real closing 'fi', a tail appended — no moon, no "${T[@]}", no
                # wrapper token; T_INVOCATION_ALLOWLIST never even scans this line
{ … } || true   # the WHOLE if/fi wrapped in a brace group — the invocation lines inside stay
                # byte-identical to T_INVOCATION_ALLOWLIST, so check 8b sees nothing wrong there
```

The first — `fi || true` — is now CLOSED, by check 8's new `block-swallowed` verdict (a `fi`/
`done`/`}` line is checked for a discarded exit status the same way a `moon` line already was).
That verdict also closes the second shape's OWN closing line (the `}` in `{ … } || true` is itself
a `}`-first-token line with a tail), so both of the concretely-identified cases above are closed —
but the closed set is still `fi`/`done`/`}` specifically, not "any control-flow construct that can
discard a status". What remains genuinely open is arbitrary shell placed AFTER the block with no
recognized terminator token of its own — a trailing `exit 0` on its own line, say, or a later step
in the same job overwriting the outcome — and, symmetrically, an always-false OUTER
conditional WRAPPING the whole block (`if false; then … fi`), where all three invocation
lines stay byte-identical to `T_INVOCATION_ALLOWLIST` and check 8b sees nothing wrong while
nothing executes on any event path (CodeRabbit round 5 on PR 150). Closing that one needs
either enclosing-branch analysis or extracting the step's `run:` block and executing it
against a mocked `moon` per event path — a materially new mechanism, not a rule tweak.
None of these is caught by a LINE-shaped rule without reassembling
the step's actual control flow, the same reachability-analysis line every other limitation in this
file (L9, L11, ci_targets.py's L10) declines to cross.

## Cost

`inputs: ['**/*']` is deliberate (see the WHY comment on the `actionlint:` task in `moon.yml`),
and it was benchmarked before being accepted (SMA-525): Moon's own per-task floor in this repo is
~9–11s regardless of what a task does — an existing narrow-input task (`repo:promtool`) measures
about the same. Once `.moon/workspace.yml`'s `hasher.ignorePatterns` excludes gitignored
dependency trees (`node_modules`, `target`, `.venv`) from the hash walk, broad `inputs: ['**/*']`
costs only ~1s over a narrow input list.

Without that filter it costs **~87s**. Alternating `moon run repo:actionlint --force` runs
(macOS, warm):

| Configuration | Time |
|---|---|
| `repo:promtool` — existing narrow-input task, i.e. Moon's floor | ~8.7s |
| this gate, narrow input list | ~10.4s |
| this gate, `inputs: ['**/*']` **with** `hasher.ignorePatterns` | ~11.6s |
| this gate, `inputs: ['**/*']` **without** it | ~98.6s |

Narrowing this task's inputs would not meaningfully help; do not do it without also revisiting
`hasher.ignorePatterns`.

**Standalone cost, since SMA-542.** Check 9's mutation battery — eight mutants plus an
unmutated control, each a full `--self-test` invocation, run concurrently, full-gate only — is the
dominant addition; check 8's floor/`continued`/`swallowed`/`continue-on-error` assertions, check
8b's allowlist/count assertions and check 8c's two-call-site assertion are a handful of
`grep`/`sed` passes over one file each and cost nothing worth measuring by comparison. Three
tables below, EACH LABELED WITH THE STATE IT MEASURES (independent review of PR 150 round 4,
finding F3 — an unlabeled table reads as "the current numbers" regardless): measured min-of-7
(`ci/actionlint/run.sh`, bypassing Moon; `uptime` immediately before read load averages
2.02/3.35/4.36 — this box runs other concurrent sessions and a mean can read several times
inflated under a load spike, hence min-of-7 rather than a mean).

State: six fixture tables, six mutants (the ORIGINAL SMA-542 fix wave — superseded by the second
table below; kept for the before/after narrative that follows it, not as current numbers):

| Invocation | Min-of-7 |
|---|---|
| `ci/actionlint/run.sh` (full gate, with the battery) | ~4.11s |
| `ci/actionlint/run.sh --self-test` (six fixture tables, no battery) | ~1.26s |

Before SMA-542 the full gate measured ~1.5s standalone and `--self-test` ~1.0s. SMA-542 itself
(five self-tests, five-mutant battery) brought those to ~3.68s / ~1.25s. That fix wave added the
sixth self-test (`kill_predicate_self_test`, closing spec T3) and check 8's `continued` verdict,
taking the mutant count to six (seven concurrent `--self-test` subprocesses, control included).
A later wave (SMA-542 CodeRabbit rounds 3-4) added the SEVENTH self-test
(`invocation_allowlist_self_test`, closing round-3 finding B) and check 8b's allowlist/count
verdicts, taking the mutant count to seven (eight concurrent `--self-test` subprocesses, control
included) — superseded by the third table below; numbers kept here for the before/after
narrative, not as current figures. A still later wave (SMA-542 residual closure, PR 150
follow-up) added the EIGHTH self-test (`affected_graph_wiring_self_test`, closing L6) and check
8c's two-call-site assertion, taking the mutant count to eight (nine concurrent `--self-test`
subprocesses, control included) — this is the CURRENT state.

State: seven fixture tables, seven mutants (superseded by the third table below; kept for the
before/after narrative, not as current numbers; load averages 2.87/2.49/2.79 immediately before):

| Invocation | Min-of-7 |
|---|---|
| `ci/actionlint/run.sh` (full gate, with the battery) | ~5.20s |
| `ci/actionlint/run.sh --self-test` (seven fixture tables, no battery) | ~1.66s |

State: CURRENT — eight fixture tables, eight mutants (load averages 3.79/5.68/5.27 immediately
before, after waiting for the box's load to settle from an initial 6.82/9.18 — see the gotcha on
this box's shared-session spikes):

| Invocation | Min-of-7 |
|---|---|
| `ci/actionlint/run.sh` (full gate, with the battery) | ~6.33s |
| `ci/actionlint/run.sh --self-test` (eight fixture tables, no battery) | ~2.01s |

**Do not conclude `hasher.ignorePatterns` is inert from the log.** It does *not* silence the
~2000 `only files can be hashed` warnings about pnpm's symlinked store — those appear identically
with and without it (verified). The warnings come from input collection; the filter skips the
hashing that follows. Judge it by the wall time above, not by the warnings.

## Running it

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"   # proto CLIs (moon, actionlint) aren't
                                                            # on a default shell PATH
moon run repo:actionlint      # via Moon, as CI does
ci/actionlint/run.sh          # directly, bypassing the Moon cache
ci/actionlint/run.sh --self-test   # the eight fixture tables only, for fast iteration
```

`--self-test` runs the eight fixture tables and nothing else — check 9's mutation battery is
full-gate-only, which is what keeps `--self-test` the fast path and what makes the battery's own
mutants (each internally invoked with `--self-test`) unable to recurse into a battery of their
own.

`--self-test` still needs no `actionlint` binary — that is the point of it — but since SMA-540 it
does need a git checkout carrying `refs/remotes/origin/main`, because the branch-filter table's
control pair asserts that a real ref resolves. Since SMA-542 the self-tests run **before** checks
1–6, not after, so on a `--single-branch` or `--depth 1` clone the canary now fires — and the gate
exits 2 — before `actionlint` itself is ever invoked. You therefore lose whatever checks 1–6 would
have found on that run, not merely the self-test tables. Recover with the **explicit refspec** —
a bare `git fetch origin` re-uses the clone's single-branch refspec and fetches nothing else, and
`git fetch origin main` updates only `FETCH_HEAD`, so neither creates the ref (both measured):

```bash
git fetch origin +refs/heads/main:refs/remotes/origin/main
```

then re-run; the whole gate costs a few seconds.

Any other argument exits 2 with a usage line — a typo'd `--selftest` must not run the full gate
and report a pass for something you did not ask for.

Exit codes: `1` = assertion failure, `2` = infrastructure error.
