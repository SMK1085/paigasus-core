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
| 7 | Five self-tests against fixture tables — extractor, path-filter verdicts, branch-filter verdicts, config allowlist, ci-target floor — plus a counter (`SELF_TESTS_RAN`/`SELF_TEST_COUNT`) asserting all five ran, and a definition-count check catching a sixth table that is defined but never wired into `run_self_tests` (`run.sh --self-test`) |
| 8 | `ci.yml`'s `T=(…)` still schedules the gate that guards `T` itself, and nothing silences that gate's result. Three verdict families: **(a)** the floor — `:affected-smoke` present in `T` (`missing`), or the array can't even be read (`no-array`/`no-file`); **(b)** no `moon` command line discards its own exit status (`swallowed`); **(c)** no step's `continue-on-error:` value suppresses it — any spelling but the literal `false` (`continue-on-error`), with `COE_SKIP` as the escape hatch for an unrelated later step |
| 9 | A mutation battery, full-gate only: each of the five self-test invocations inside `run_self_tests`, deleted one at a time, run concurrently against the real unmutated control — every mutant must die at the counter's own message, or the battery itself reds |

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

**L6 — The cycle is asymmetric.** `repo:affected-smoke` pins `repo:actionlint`'s call sites, but
`repo:actionlint` pins only `:affected-smoke`'s *presence in `T`* — i.e. its scheduling. Deleting
`assert_ci_targets || SUITE_RC=1` from `ci/affected-graph/run.sh` therefore still removes that
half of the guard silently, with everything green. `ci/affected-graph/ci_targets.py`'s
`RUN_SH_CALL_SITES` comment describes this accurately; keep the two consistent.

**L7 — `COE_SKIP` is exact-text, not semantic.** An entry is keyed by both line number and the
matched line's exact text (leading blanks included), so a later reformat of that one line — even
whitespace-only — makes the entry stop matching. That is the fail-safe direction: the check fires
again and asks for the entry to be updated, rather than silently continuing to skip a line whose
content has since changed underneath it. `COE_SKIP` ships empty today.

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

**Standalone cost, since SMA-542.** Check 9's mutation battery — five mutants plus an unmutated
control, each a full `--self-test` invocation, run concurrently, full-gate only — is the dominant
addition; check 8's floor/swallowed/`continue-on-error` assertions are a handful of `grep`/`sed`
passes over one workflow file and cost nothing worth measuring by comparison. Measured min-of-7
(`ci/actionlint/run.sh`, bypassing Moon; `uptime` immediately before read load averages
7.50/6.34/5.66 — this box runs other concurrent sessions and a mean can read several times
inflated under a load spike, hence min-of-7 rather than a mean):

| Invocation | Min-of-7 |
|---|---|
| `ci/actionlint/run.sh` (full gate, with the battery) | ~3.68s |
| `ci/actionlint/run.sh --self-test` (five fixture tables, no battery) | ~1.25s |

Before SMA-542 the full gate measured ~1.5s standalone and `--self-test` ~1.0s — the battery adds
roughly 2s, essentially all of it the five concurrent `--self-test` subprocesses check 9 spawns.

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
ci/actionlint/run.sh --self-test   # the five fixture tables only, for fast iteration
```

`--self-test` runs the five fixture tables and nothing else — check 9's mutation battery is
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
