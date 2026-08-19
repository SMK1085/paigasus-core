# SMA-540 — Gate `branches:` filters the way SMA-525 gates `paths:` filters

**Status:** approved (2026-08-19)
**Linear:** [SMA-540](https://linear.app/smaschek/issue/SMA-540/repo-gate-branches-filters-the-way-sma-525-gates-paths-filters)
**Related:** SMA-525 (built the gate; this closes its limitation L5), SMA-448 (the same silent-failure class), SMA-538 (the canary precedent)

## 1. Problem

SMA-525 closed the silent-failure hole for `paths:` filters. A glob that comes to match nothing
now reds `repo:actionlint` instead of quietly disabling a workflow forever.

`branches:` has exactly the same property and is not covered. `branches: [mian]` is a valid glob,
actionlint accepts it (measured, §2.3), and the workflow simply stops running — the same silent
and permanent failure, one key over. There is no red check and no notification; the trigger just
goes quiet. All three workflows in this repo hang off `branches: [main]`: `ci.yml` (both the
`pull_request` and `push` events, i.e. the only required status check), `prebuild.yml` (the
7-platform napi verification) and `security-scan.yml`.

SMA-525 excluded this deliberately, as its limitation L5, to keep the blast radius of a change to
the only required check contained. Branch-name checking has false-red surface that path checking
does not: a workflow may legitimately filter on a `release/**` branch that does not exist yet, and
a CI checkout does not resolve the same refs a developer's checkout does (§2.2).

## 2. Evidence

Measured on 2026-08-19 against `actionlint` 1.7.12 and the tree at `origin/main` (`d7a2ccd`),
before any design decisions were made. Everything in this section was observed, not reasoned
about.

### 2.1 The existing extractor cannot read a single one of the five filters

All five `branches:` filters in the repo are written in the inline **flow sequence** form:

```
.github/workflows/ci.yml:5:            branches: [main]
.github/workflows/ci.yml:7:            branches: [main]
.github/workflows/prebuild.yml:11:     branches: [main]
.github/workflows/prebuild.yml:26:     branches: [main]
.github/workflows/security-scan.yml:24: branches: [main]
```

`extract_paths_keys` deliberately does not parse that form. A block opens only when the value
after the colon is empty; a non-empty value emits a `KEY` record with no `ITEM` records, which
check 6 turns into a loud `no-items` failure rather than a silent skip. Running the real extractor
on the structural analogue confirms it:

```
$ printf 'on:\n  pull_request:\n    paths: [main]\n  push:\n    paths: [main]\n' > fx.yml
$ extract_paths_keys fx.yml
KEY	paths	3
KEY	paths	5
```

Two keys, zero items. So the issue's own sketch — "reuse `extract_paths_keys` and the check-6
counting" — reds all three workflows on the first run unless something gives. This is the fact
that drives D1.

### 2.2 A ref lookup answers differently in CI than on a developer's machine

The issue's first sketch bullet proposes `git show-ref --verify refs/remotes/origin/<name>`. That
is not host-independent. This checkout resolves eight remote-tracking refs — `origin/main`,
`origin/HEAD`, four dependabot branches, a feature branch, `entire/checkpoints/v1`. A CI checkout
resolves essentially one: `actions/checkout` fetches a narrow refspec even at `fetch-depth: 0`,
which is exactly why `ci.yml` carries an explicit "Materialize main ref" step
(`+refs/heads/main:refs/remotes/origin/main`) for the `pull_request` event.

So a wildcard-free entry naming any branch other than `main` passes on a laptop and reds in CI —
a false red in the only required check, which is the failure mode SMA-525 spent its design budget
avoiding. Two alternatives were measured and discarded before the design settled:

- `git ls-remote` — answers correctly everywhere, but puts a network call inside the required
  check. Rejected on hermeticity and flake grounds; no other gate in `ci/` reaches the network.
- `refs/remotes/origin/HEAD` as a way to learn the default branch without hardcoding it —
  `actions/checkout` never creates it, so it resolves here and not in CI.

### 2.3 What actionlint already covers, and what it does not

Three defect classes, each fed to the pinned `actionlint` via stdin:

| Fixture | actionlint verdict |
|---|---|
| `branches` and `branches-ignore` on the same event | **exit 1** — `both "branches" and "branches-ignore" filters cannot be used for the same event "push"` `[events]` |
| all-negated `branches: ["!main"]` | exit 0 — missed |
| `branches: [mian]` | exit 0 — missed |

The first is already guarded and becomes an explicit non-goal (§8). The second and third are the
holes this issue closes.

### 2.4 GitHub's documented matching rules

From the [workflow syntax reference](https://docs.github.com/en/actions/reference/workflow-syntax-for-github-actions),
verbatim:

- *"If you define a branch with the `!` character, you must also define at least one branch without
  the `!` character."* — the all-negated rule, documented **more** explicitly for `branches` than
  it is for `paths`. This is what licenses extending check 6's `all-negated` verdict.
- *"You cannot use both the `branches` and `branches-ignore` filters for the same event in a
  workflow."* — already caught by actionlint, §2.3.
- Branch filters *"accept glob patterns that use characters like `*`, `**`, `+`, `?`, `!` and
  others"*. `+` matters: GitHub reads it as "one or more of the preceding character", so `foo+`
  matches the branch `foo`, not a branch literally named `foo+`. `+` is nonetheless legal in a git
  ref name, which drives D4.

## 3. Decisions

**D1 — The five `branches: [main]` filters are rewritten in block style; the extractor is not
taught flow sequences.** §2.1 forces a choice between changing the workflows and growing the
parser. The parser is hand-rolled YAML in the only required check, and `run.sh` says of itself
that this is "exactly the kind of thing that silently does the wrong thing" — so the change that
adds zero parser surface wins. The rule this leaves is uniform across all four filter keys ("this
gate reads block sequences"), it is already enforced by an existing message that names the remedy,
and it costs one line per filter. The cost accepted: the repo deviates from GitHub's documented
`[main]` idiom, and a future workflow author meets the gate once.

Rejected: teaching the extractor flow sequences for `branches` only (bakes in a key-specific
inconsistency a reader must be told rather than deduce), and for all keys (changes `paths:`
behaviour and forces edits to the `path_filter_self_test` fixtures that assert `no-items` for that
form — precisely the blast radius AC-3 asks to avoid).

**D2 — The assertion is resolve-as-a-ref, or an entry in a documented skip list.** One rule decides
the outcome for every entry, wildcards included: a wildcard can never resolve, so it must be
skip-listed to pass. It still gets its own verdict token, but only so the message can name the
actual reason (D4) — the pass/fail rule itself has no wildcard special case to keep in sync.
Anchoring to a real ref rather than to
a list of names inside `run.sh` is what makes the check bite when a name is typo'd identically in
two places — a pure in-repo allowlist only asserts that one string in this repo equals another
string in this repo, so `mian` copy-pasted into both passes.

**D3 — The ref namespace is `refs/remotes/origin/*` only; local `refs/heads/*` is deliberately
excluded.** A workflow triggers on branches as they exist on GitHub. A local branch that was never
pushed does not exist there, so a filter naming it is already dead — accepting it as proof would
launder the exact failure this gate exists to catch. It also shrinks the local-versus-CI
divergence of §2.2 rather than widening it.

**D4 — Glob metacharacters are rejected before the ref lookup, and `+` counts as one.** Order is
load-bearing, and it is the lesson `pattern_verdict` already encodes by placing its
`rejected-charclass` case above its pathspec guard: if `git check-ref-format` ran first,
`release/**` would be told it is not a legal branch name — true, but not the actionable reason,
and the author is left with nothing to do. `+` is treated as a glob despite being legal in a ref
name (§2.4): a branch literally named `foo+` would otherwise resolve and yield a confidently wrong
`ok` for a pattern that GitHub reads as matching `foo`.

**D5 — `branches-ignore:` is extracted and counted, but never resolved.** Mirrors the settled
`paths-ignore:` precedent exactly. A typo'd exclusion makes a workflow run *more* often, which is
the fail-safe direction; the dangerous direction for an `-ignore` key is matching everything, which
resolution does not test. Extraction still happens so that an unreadable `branches-ignore:` reds as
`no-items` instead of vanishing.

**D6 — A preflight canary asserts `refs/remotes/origin/main` resolves, exiting 2.** Without it, a
checkout with no fetched refs fails every entry at once, each message phrased as though the author
typo'd a name they did not typo. The canary converts that into one accurate message and follows the
file's documented exit-code split (1 = assertion, 2 = infrastructure) and the SMA-538 precedent of
one loud red over a pile of misleading results. It also closes the one case D2's ref-anchoring
cannot see on its own: if `main` were ever renamed, every `branches: [main]` in the repo would be
dead, and this reds rather than reporting five separate typos. Hardcoding `main` is not new — it is
already hardcoded in `ci.yml`'s `--base origin/main` and in CLAUDE.md. The canary runs under
`--self-test` too, because the fixture asserting `main → ok` shares its precondition.

**D7 — The checks are renumbered: the new branch assertion is check 7, the self-test bundle moves
to check 8.** The alternative — numbering the new check 8 while it runs inside the check-5/6 loop,
before check 7 — leaves a reader tripping over an ordering that looks like an accident. The cost is
about five comment references plus the README table.

## 4. Architecture

`run.sh` already has the five layers this needs; nothing new is stacked beside them.

### 4.1 Extractor

`extract_paths_keys` is renamed **`extract_filter_keys`** — it now recognises four kinds
(`paths`, `paths-ignore`, `branches`, `branches-ignore`) rather than two, and a name claiming
otherwise is the kind of drift this file's comments exist to prevent. One pass, one function; the
records already carry their kind, so no caller has to be told which key it is reading:

```
KEY\t<paths|paths-ignore|branches|branches-ignore>\t<lineno>
ITEM\t<kind>\t<value>
```

The depth rule is unchanged and continues to apply generically: only a key **two levels deep**
inside `on:` is a filter, so a `workflow_dispatch` input legitimately *named* `branches` (level 3)
is ignored, exactly as an input named `paths` is today. The `flow_keys` helper gets the same
vocabulary extension, so a flow-mapping event value — `push: { branches: [main] }` — emits a `KEY`
with no items and fails loudly rather than being skipped in silence.

### 4.2 Verdict

**`branch_verdict()`** — new, pure, echoes exactly one stable token, sibling to `pattern_verdict`
and separated from its messages for the same reason (a function that both decides and prints
cannot be asserted against a fixture table):

```
in BRANCH_SKIP                     -> skipped        documented escape hatch
starts with '!'                    -> negated        counted, never resolved
contains * ? + [ ]                 -> unverifiable   FAIL: skip-list it
git check-ref-format rejects it    -> invalid-name   FAIL: not a legal branch name
resolves refs/remotes/origin/<n>   -> ok
otherwise                          -> unresolved     FAIL: typo? or skip-list it
```

The two git calls are `git check-ref-format "refs/heads/$n"` and
`git show-ref --verify --quiet "refs/remotes/origin/$n"`. `check-ref-format` is used in preference
to a hand-rolled character class because it is the same validity rule git itself applies — it
catches `..`, `~`, `^`, `:`, control characters and a trailing `.lock` without this gate having to
enumerate them.

`BRANCH_SKIP` is the AC-2 escape hatch and mirrors `SKIP_PATTERNS`: an array in `run.sh`, every
entry carrying a comment justifying it and saying what verifies it instead. It ships empty.

### 4.3 Scanner and call site

`scan_workflow_records` dispatches on kind — `paths` to `pattern_verdict`, `branches` to
`branch_verdict`, the two `-ignore` kinds to counting only. Its `all-negated` rule widens from
`paths` to `paths|branches` on the documented grounds of §2.4; its `no-items` rule was already
kind-generic and needs no change. The production call site gains one message per new verdict token,
each naming the file, the entry and the remedy, in the prose style the existing messages use.

### 4.4 Workflows

Five filters move to block style (`ci.yml` ×2, `prebuild.yml` ×2, `security-scan.yml` ×1). No
`T=(…)` change in `ci.yml`: this extends the existing `repo:actionlint`, which is already in the
array, so `repo:affected-smoke` is untouched and there is no new gate to wire.

## 5. Verification

- `ci/actionlint/run.sh --self-test` and a full `ci/actionlint/run.sh` both pass on the real tree.
- **AC-3 regression proof.** The extractor's `paths`-kind records over the three real workflow
  files are captured before and after the change and must be byte-identical. This is the direct
  test of "the existing `paths:` behaviour and the real-file extraction counts are unchanged".
- **`branch_filter_self_test`** (new fixture table, invoked unconditionally as part of check 8): one
  fixture per verdict token, including both directions of the control pair — `main → ok` and a
  synthetic non-existent name → `unresolved`. An all-firing table cannot distinguish a working
  check from a stuck one, which is the trap SMA-466 recorded for the promtool fixtures.
- **`extractor_self_test`** gains fixtures for: `branches` block form, `branches` flow sequence
  (→ `KEY`, no `ITEM`), `branches-ignore`, a flow-mapping event carrying `branches`, and a
  `workflow_dispatch` input named `branches` at depth 3 being correctly ignored.
- **`path_filter_self_test`** is untouched.
- **Mutation battery.** SMA-525's round-3 finding F4 was that checks 5 and 6 could be neutered one
  code path at a time with the gate still exiting 0, because the only thing exercising them was the
  repo's own clean files. Each new path gets the same treatment: delete the resolve call, force
  `branch_verdict` to return `ok`, drop `branches` from the extractor's key vocabulary, remove the
  `all-negated` widening. Each mutation must red.
- **End-to-end negative test.** A temporary `branches: [mian]` in a real workflow must red the gate
  naming the file and the entry. Run through `ci/actionlint/run.sh` directly, not `moon run`, so a
  cached PASS cannot replay.
- The full `moon ci … --base origin/main --include-relations` graph before pushing, per the
  repo-gates rule in CLAUDE.md.

## 6. Rollout and rollback

Ships in one PR: workflow rewrite, `run.sh`, `ci/actionlint/README.md`, a CLAUDE.md gotcha, this
spec. No Moon task, `T=(…)`, or `.prototools` change, so the affected graph and the required-check
wiring are untouched.

Rollback is graduated, cheapest first: add the offending entry to `BRANCH_SKIP` with a
justification; or drop the four branch-kind regexes from `extract_filter_keys`, which disables the
new check while leaving `paths` intact; or drop `:actionlint` from `T=(…)`, one line, as SMA-525
already documents.

## 7. Limitations

Stated deliberately, so nothing reads as a stronger guarantee than it is, and in the spirit of
SMA-525 §7.

- **L1 — Wildcard branch entries are unverifiable by construction.** `release/**` cannot be
  resolved against anything, so it must be skip-listed. The gate proves such an entry was an
  explicit decision, not that it is correct.
- **L2 — A branch that exists on the remote but is unfetched in CI reds there while passing
  locally.** The §2.2 divergence, narrowed by D3 but not eliminated. The remedy is one skip-list
  line and the failure message says so. It surfaces on the PR that introduces the name, never
  after merge.
- **L3 — Negated entries are not validated.** `branch_verdict` returns early on `!`, so a malformed
  exclusion is never checked. Mirrors SMA-525's L7 and is accepted for the same reason: a broken
  exclusion can only fail to exclude, which makes the workflow run more often — the fail-safe
  direction. An all-negated block is still caught, by check 6.
- **L4 — `tags:` and `tags-ignore:` are not covered.** They carry the identical silent-death
  property: a typo'd tag filter means a release workflow never fires. No workflow here uses them
  today, and real tag filters are near-universally wildcards (`v*`), which would land in the skip
  list by L1 and make the check close to vacuous — so it would add parser and fixture surface to
  the required check while proving almost nothing. Revisit when a tag filter is actually added.
- **L5 — The gate cannot prove a workflow ran.** Inherited verbatim from SMA-525's L4. It proves a
  filter is well-formed and its entries name real branches; a filter whose entries all exist but
  which collectively never match a real event is still possible.
- **L6 — `BRANCH_SKIP` is honour-based.** Like `SKIP_PATTERNS`, nothing verifies that a skip entry's
  justification is true. It converts a silent pass into a recorded decision, which is the AC, not
  into a proof.

## 8. Non-goals

- **`branches` + `branches-ignore` coexistence.** Already caught by actionlint with a precise
  `[events]` message (§2.3). Re-implementing it would add a second, weaker copy of a working check.
- **Teaching the extractor inline flow sequences.** D1. The workflows move instead.
- **Checking branch existence over the network.** §2.2.
- **Any change to `paths:` verdicts, fixtures, or the `T=(…)` array.** AC-3 and §4.4.
