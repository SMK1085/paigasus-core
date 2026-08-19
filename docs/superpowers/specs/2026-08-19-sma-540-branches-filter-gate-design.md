# SMA-540 — Gate `branches:` filters the way SMA-525 gates `paths:` filters

**Status:** revised after adversarial review (2026-08-19)
**Linear:** [SMA-540](https://linear.app/smaschek/issue/SMA-540/repo-gate-branches-filters-the-way-sma-525-gates-paths-filters)
**Related:** SMA-525 (built the gate; this closes its limitation L5), SMA-448 (the same silent-failure class), SMA-538 (the canary precedent), SMA-541 (in flight; touches the same two files, §6)

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
the only required check contained.

## 2. Evidence

Measured on 2026-08-19 against `actionlint` 1.7.12 and the tree at `origin/main` (`d7a2ccd`).
Everything in this section was observed. Where an earlier draft of this spec reasoned instead of
measuring, it got the answer wrong — see §2.2, which reverses it.

### 2.1 The existing extractor cannot read a single one of the five filters

All five `branches:` filters in the repo are written in the inline **flow sequence** form:

```
.github/workflows/ci.yml:5              branches: [main]
.github/workflows/ci.yml:7              branches: [main]
.github/workflows/prebuild.yml:11       branches: [main]
.github/workflows/prebuild.yml:26       branches: [main]
.github/workflows/security-scan.yml:24  branches: [main]
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
counting" — reds all three workflows on the first run unless something gives. This drives D1.

### 2.2 CI fetches ALL heads — the opposite of what this spec first assumed

The issue's first sketch bullet proposes `git show-ref --verify refs/remotes/origin/<name>`, and
an earlier draft of this spec rejected it on the grounds that `actions/checkout` fetches a narrow
refspec even at `fetch-depth: 0`, inferring that from the presence of `ci.yml`'s "Materialize main
ref" step. **That inference was wrong.** Grepping the checkout step of two real `ci.yml` runs:

```
# run 32266399274, event: push, branch: main
git -c protocol.version=2 fetch --no-tags --prune --no-recurse-submodules origin \
  +refs/heads/*:refs/remotes/origin/* +refs/tags/*:refs/tags/*

# run 32265614417, event: pull_request
git -c protocol.version=2 fetch --no-tags --prune --no-recurse-submodules origin \
  +refs/heads/*:refs/remotes/origin/* +refs/tags/*:refs/tags/* \
  +1b6cdf7f…:refs/remotes/pull/141/merge
```

Both paths fetch **every** head into `refs/remotes/origin/*`. The "Materialize main ref" step is
not there to create `refs/remotes/origin/main` at all — its load-bearing half is
`+refs/heads/main:refs/heads/main`, a *local* `main` that checkout never creates on a PR (it
checks out a detached `refs/remotes/pull/N/merge`), which `contracts:breaking` needs for
`--against '../.git#branch=main'`.

Three consequences, all of which reverse the earlier draft:

1. A branch that exists on the remote resolves in CI as well as locally. The false-red risk that
   SMA-525's L5 cited as the reason to defer this work, and that the earlier draft carried forward
   as a limitation, **does not exist**.
2. The residual divergence runs the *other* way: a local checkout can be stale, so a
   newly-created remote branch reds locally and passes in CI. Benign — the remedy is `git fetch`.
3. The genuinely new risk is that **ephemeral** branches resolve. Dependabot and feature branches
   are all present in `refs/remotes/origin/*` and are deleted on merge. See L2.

Two alternatives were also discarded:

- `git ls-remote` — correct everywhere, but a network call inside the required check. No other
  gate in `ci/` reaches the network.
- `refs/remotes/origin/HEAD` as a way to learn the default branch without hardcoding it —
  `actions/checkout` never creates it.

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
  the `!` character."* — documented **more** explicitly for `branches` than for `paths`. The rule
  "≥1 negated ⟹ ≥1 non-negated" is violated exactly when there are ≥1 entries and zero positive
  ones, which is precisely check 6's existing condition. This licenses D5's widening.
- *"You cannot use both the `branches` and `branches-ignore` filters for the same event in a
  workflow."* — already caught by actionlint, §2.3.
- Branch filters *"accept glob patterns that use characters like `*`, `**`, `+`, `?`, `!` and
  others"*. `+` matters: GitHub reads it as "one or more of the preceding character", so `foo+`
  matches the branch `foo`, not a branch literally named `foo+`. `+` is nonetheless legal in a git
  ref name, which drives D4.

The quoted `!` rule does **not** distinguish `branches` from `branches-ignore`. D6's exemption of
`branches-ignore` is therefore an argued analogy to `paths-ignore`, not something these docs
establish; stated here rather than implied.

### 2.5 `moon ci` does not require `origin/main` on every path

`ci.yml:216-223` passes `--base origin/main` **only** on `pull_request`. On push-to-`main` it
passes `--base "$BEFORE"` (a SHA); the initial-push fallback runs `moon run` with no base. So
`moon ci` is not a pre-existing requirer of that ref on two of three paths, and D7 may not lean on
it. What does hold, from §2.2, is that `actions/checkout` materialises `refs/remotes/origin/main`
on both event paths regardless.

## 3. Decisions

**D1 — The five `branches: [main]` filters are rewritten in block style; the extractor is not
taught flow sequences.** §2.1 forces a choice between changing the workflows and growing the
parser. The parser is hand-rolled YAML in the only required check, and `run.sh` says of itself that
this is "exactly the kind of thing that silently does the wrong thing" — so the change that adds
zero parser surface wins. The rule this leaves is uniform across all four filter keys ("this gate
reads block sequences"), it is already enforced by an existing message that names the remedy, and
it costs one line per filter. Cost accepted: the repo deviates from GitHub's documented `[main]`
idiom, and a future workflow author meets the gate once.

Rejected: teaching the extractor flow sequences for `branches` only (a key-specific inconsistency a
reader must be told rather than deduce), and for all keys (changes `paths:` behaviour and forces
edits to the `path_filter_self_test` fixtures asserting `no-items` for that form — the blast radius
AC-3 asks to avoid).

**D2 — The assertion is resolve-as-a-ref, or an entry in a documented skip list.** One rule decides
the outcome for every entry, wildcards included: a wildcard can never resolve, so it must be
skip-listed to pass. It still gets its own verdict token, but only so the message can name the
actual reason (D4) — the pass/fail rule has no wildcard special case to keep in sync. Anchoring to
a real ref rather than to a list of names inside `run.sh` is what makes the check bite when a name
is typo'd identically in two places; a pure in-repo allowlist only asserts that one string in this
repo equals another string in this repo.

Reconsidered after §2.2 and kept: an allowlist of long-lived branch names would immunise against
the ephemeral-branch case of L2, but at the cost of the property that makes D2 worth having. The
ephemeral case requires someone to deliberately write a dependabot or feature branch into a
workflow filter, which no workflow here does; L2 states it rather than designing around it.

**D3 — The ref namespace is `refs/remotes/origin/*` only; local `refs/heads/*` is excluded.** The
earlier draft justified this as narrowing a CI divergence that §2.2 has since shown does not
exist. The rationale that survives is simpler and still sound: a workflow triggers on branches as
they exist on GitHub, `refs/remotes/origin/*` is exactly that set, and it is the set CI and a
fetched local checkout agree on. `refs/heads/*` is a developer's private branch set locally and
just `main` (push) or nothing (PR) in CI — the one namespace guaranteed to disagree.

**D4 — Glob metacharacters are rejected before the ref lookup, and `+` counts as one.** Order is
load-bearing for two reasons. First, message quality — the lesson `pattern_verdict` already encodes
by placing `rejected-charclass` above its pathspec guard: if `git check-ref-format` ran first,
`release/**` would be told it is not a legal branch name, which is true but not actionable.
Second, **safety**: `branch_verdict` deliberately does not carry `pattern_verdict`'s
`^[A-Za-z0-9._/*-]+$` allowlist, relying on `check-ref-format` instead. That is sound only because
`check-ref-format` runs before `show-ref` and rejects spaces, control characters and `~^:?*[\`,
and because the argument is always prefixed (`refs/heads/`, `refs/remotes/origin/`) so it can never
be read as an option. Reordering these two calls silently removes that guarantee.

`+` is treated as a glob despite being legal in a ref name (§2.4): a branch literally named `foo+`
would otherwise resolve and yield a confidently wrong `ok` for a pattern GitHub reads as matching
`foo`.

**D5 — check 6's `all-negated` verdict widens to `branches`, which requires two edits, not one.**
The condition in `flush_key` is the visible half. The other is that `key_positive` is currently
incremented inside a `kind = 'paths'` guard in `scan_workflow_records`, so widening only the
condition makes every rewritten `branches:` block report `key_items=1, key_positive=0` and fire
`all-negated` on all five filters — redding the required check on a clean tree. The counting moves
out of the guard so it is kind-generic; only *verdict dispatch* stays kind-specific.

**D6 — `branches-ignore:` is extracted and counted, but never resolved.** Mirrors the settled
`paths-ignore:` precedent. A typo'd exclusion makes a workflow run *more* often — the fail-safe
direction; the dangerous direction for an `-ignore` key is matching everything, which resolution
does not test. Extraction still happens so an unreadable `branches-ignore:` reds as `no-items`
instead of vanishing. Per §2.4 this is an argued analogy, not a documented rule.

**D7 — The `origin/main` canary is lazy, not a preflight.** It runs immediately before the first
`show-ref` that `branch_verdict` actually needs — i.e. only once at least one entry has survived
skip, negated and glob filtering — and exits 2 with a message distinguishing a broken checkout from
a typo. Making it a preflight, as an earlier draft had it, would mean a checkout without
`origin/main` (`git clone --single-branch`, a remote named `upstream`, a partial-clone container)
loses checks 1–6 as well: actionlint would stop linting workflows entirely, in a gate that today
has **zero** ref dependency, since `pattern_verdict` uses only `git ls-files`. `exit 2` fails a
Moon task exactly as hard as `exit 1`; the infrastructure/assertion split is a message
distinction, not a blast-radius one.

Laziness buys checks 1–6 the chance to run and report their own findings first; it does not buy
the overall run a pass. `branch_filter_self_test` is invoked unconditionally as part of check 7,
and its own precondition calls `origin_has 'main' || no_origin_main_infra` before a single real
workflow's `branches:` entries are even considered. So a checkout without `origin/main` still
exits 2 at the end of a full (non-`--self-test`) run — whether or not any workflow in the tree
declares a `branches:` key at all. What laziness actually preserves is diagnostic order: checks
1–6 run to completion and surface their own findings first, rather than the whole run being
pre-empted by a preflight ref check before check 1 even starts.

The canary is placed after `cd "$(git rev-parse --show-toplevel)"` and after the `--self-test`
early exit, inside the verdict path both modes share, so the three placements the reviewer asked
about collapse to one.

Laziness narrows but does not fully preserve `--self-test`'s contract, and the difference is worth
being exact about. What that mode documents today is that it *"never shells out to actionlint"* and
so deliberately exits before the PATH guard — it must keep working on a machine without the binary.
Laziness preserves that. It does not make the mode ref-free: `branch_filter_self_test` is added to
the `--self-test` list, and its `main → ok` fixture is precisely the control that needs a resolvable
ref, so in a checkout lacking `origin/main` that mode now exits 2 where it previously passed. The
alternative — dropping the `ok` direction from the fixture table — is worse, because a table whose
verdicts all fire cannot distinguish a working check from a stuck one. The comment on the
`--self-test` branch is updated to say "no actionlint binary; a git repo with `origin/main`" rather
than leaving the reader with the old, now-incomplete promise.

It also covers the case D2's ref-anchoring cannot see alone: if `main` were renamed, every
`branches: [main]` in the repo would be dead, and this reds once accurately rather than five times
as "typo". Hardcoding `main` is not new — `ci.yml` and CLAUDE.md already do. §2.5 corrects the
earlier claim that `moon ci` independently requires the ref; the argument now rests on the measured
checkout behaviour of §2.2 instead.

**D8 — Branch findings get their own record type, `BRANCH`, carrying a line number.** `PATTERN`
records are `verdict\tpattern` with no line, so a typo in one of `ci.yml`'s two identical
`branches: [main]` entries could name the file and the string but not *which* filter — weak against
AC-1's "names the file and the entry". A separate record type is preferred over adding a field to
`PATTERN` for three reasons: `PATTERN`'s shape stays byte-identical, so `path_filter_self_test` and
AC-3 are untouched; the call site dispatches on record type, so the two verdict vocabularies cannot
collide into the wrong message or an `infra "unhandled verdict"` exit 2; and `scan_workflow_records`
already tracks `key_line`, so the value is free.

**D9 — Existing check-6 messages are de-hardcoded.** Both currently say `'paths:'` in their prose
(the `all-negated` message hardcodes the label, the `no-items` message illustrates with
`paths: [a, b]`). Neither is a new verdict token, so nothing would otherwise prompt the edit, and an
all-negated `branches:` block would be reported as a `paths:` problem at a line where no `paths:`
key exists. D1 depends on the `no-items` message naming the right remedy, so this is load-bearing
for D1 as well as cosmetic.

**D10 — The branch assertion extends check 5; nothing is renumbered.** An earlier draft made it a
new numbered check and moved the self-test bundle from 7 to 8 so that "the self-tests run last"
stayed true. That was churn in payment for a distinction that does not exist: the branch assertion
executes *inside* the existing check-5/6 per-file loop, and it is per-entry verdict logic — exactly
what check 5 already is, one kind over. So check 5 becomes "every `paths:` glob matches the tree and
every `branches:` entry resolves", check 6 stays the key/entry counting rule for all four kinds, and
check 7 stays the self-test bundle, now four tables instead of three. Zero comment-reference churn,
no ordering oddity to explain, and review attention stays on the logic rather than on renumbered
headers.

## 4. Architecture

`run.sh` already has the five layers this needs; nothing new is stacked beside them.

### 4.1 Extractor

`extract_paths_keys` is renamed **`extract_filter_keys`** — it recognises four kinds
(`paths`, `paths-ignore`, `branches`, `branches-ignore`) rather than two, and a name claiming
otherwise is the drift this file's comments exist to prevent:

```
KEY\t<paths|paths-ignore|branches|branches-ignore>\t<lineno>
ITEM\t<kind>\t<value>
```

The depth rule is unchanged and applies generically: only a key **two levels deep** inside `on:` is
a filter, so a `workflow_dispatch` input legitimately *named* `branches` (level 3) is ignored,
exactly as an input named `paths` is. `flow_keys` gets the same vocabulary extension, so
`push: { branches: [main] }` emits a `KEY` with no items and fails loudly rather than being skipped.

The block-mechanics were traced adversarially against `prebuild.yml`'s exact post-rewrite shape — a
`branches:` block immediately followed by a sibling `paths:` block at the same indent — and are
correct as they stand. Two existing mechanisms are load-bearing and must not be "simplified": the
`ind <= key_ind` non-item branch closes the block and **falls through** rather than `next`ing, so
the following `paths:` line is still seen as a key; and `while (depth > 0 && indstack[depth] >= ind)
depth--` pops the stale level-2 entry before re-pushing at the same indent, so depth returns to 2
rather than 3. No interleaving misattributes a `branches` ITEM to a `paths` KEY, in either order,
and the early `next` on `-` lines still protects a sibling `schedule:` sequence.

### 4.2 Verdict

**`branch_verdict()`** — new, pure, echoes exactly one stable token, sibling to `pattern_verdict`
and separated from its messages for the same reason (a function that both decides and prints cannot
be asserted against a fixture table):

```
in BRANCH_SKIP                     -> skipped        documented escape hatch
starts with '!'                    -> negated        counted, never resolved
contains * ? + [ ]                 -> unverifiable   FAIL: skip-list it
git check-ref-format rejects it    -> invalid-name   FAIL: not a legal branch name
resolves refs/remotes/origin/<n>   -> ok
otherwise                          -> unresolved     FAIL: typo? or skip-list it
```

The git calls are `git check-ref-format "refs/heads/$n"` and a lookup against a **single**
`git for-each-ref --format='%(refname:short)' refs/remotes/origin/` collected once per run — one
subprocess instead of one per entry, and it makes both the canary (is the list missing `main`?) and
the `unresolved` message (which names near-miss candidates from the list, rather than reporting a
bare boolean) fall out for free. `check-ref-format` is preferred to a hand-rolled character class
because it is git's own validity rule: it catches `..`, `~`, `^`, `:`, control characters and a
trailing `.lock` without this gate enumerating them.

`BRANCH_SKIP` is the AC-2 escape hatch: a **separate** array from `SKIP_PATTERNS`, read by a
separate `is_branch_skipped` helper, so the two namespaces cannot merge and a path skip cannot
silence a branch entry. Matching is **exact string**, like `is_skipped` — `release/**` and
`release/*` need two entries, which is the intended strictness: a skip must silence exactly what
its author looked at. Every entry carries a comment justifying it and saying what verifies it
instead. It ships empty.

### 4.3 Scanner and call site

`scan_workflow_records` dispatches on kind — `paths` to `pattern_verdict` emitting `PATTERN`,
`branches` to `branch_verdict` emitting `BRANCH` (D8), the two `-ignore` kinds to counting only.
Per D5 the `key_positive` counting moves out of its `paths`-only guard and the `all-negated`
condition widens to `paths|branches`; `no-items` was already kind-generic. The call site gains one
message per new verdict token and de-hardcodes the two existing check-6 messages (D9).

### 4.4 Workflows

Five filters move to block style (`ci.yml` ×2, `prebuild.yml` ×2, `security-scan.yml` ×1). No
`T=(…)` change: this extends the existing `repo:actionlint`, already in the array, so there is no
new gate to wire.

## 5. Verification

- `ci/actionlint/run.sh --self-test` and a full `ci/actionlint/run.sh` both pass on the real tree.
- **AC-3 regression proof.** The naive form — capture records before and after — is *infeasible*:
  the same PR rewrites five `branches: [main]` lines into two lines each, so every downstream KEY
  record shifts (`prebuild.yml` `KEY paths 12`→`13` and `27`→`29`; `security-scan.yml` `25`→`26`).
  The sound form is to run the **new** `extract_filter_keys` against the **pre-rewrite** workflow
  files and diff its `paths`-kind records against the old `extract_paths_keys` on the same files;
  those must be byte-identical. The post-rewrite line shifts are then enumerated separately so a
  reviewer can tell an expected shift from a regression.
- **Six existing `extractor_self_test` fixtures change expectation** — they contain a level-2
  `branches:` and will now emit records. Each new expectation is correct-by-design, not a
  regression, and must be updated deliberately rather than made green:

  | Fixture | Change |
  |---|---|
  | `dedent closes the block` | gains `KEY branches 6` |
  | `a paths: line inside a run block is ignored` | `""` → `KEY branches 4` |
  | flow-mapping event (`push: { branches: [main], paths: [...] }`) | gains `KEY branches 3` **before** the paths KEY (`flow_keys` scans left to right) |
  | flow-mapping event with `paths-ignore` | gains `KEY branches 3` |
  | `a flow-mapping event without a path filter is not a KEY` | `""` → `KEY branches 3`; **rename it** — the name becomes false |
  | flow mapping on `on:` itself | gains `KEY branches 2` |

- **New standing controls, not just a mutation battery.** SMA-525's F4 finding was that checks 5/6
  could be neutered one path at a time with the gate still exiting 0, because a one-off battery is
  not a standing control. So `branch_filter_self_test` — the fourth table under check 7, invoked
  unconditionally there and from the `--self-test` early exit — specifies both per-entry and
  end-to-end fixtures:
  - a `branch_verdict` fixture per token, including **both** directions of the control pair —
    `main → ok` and a synthetic non-existent name → `unresolved`. An all-firing table cannot
    distinguish a working check from a stuck one (the SMA-466 trap).
  - an `expect_scan` fixture asserting an all-negated `branches` block yields
    `KEY all-negated branches …` — otherwise D5's widening has no regression guard.
  - an `expect_scan` fixture asserting a `branches` block containing a bad entry produces a
    `BRANCH` record end-to-end, which is the only thing proving `scan_workflow_records` actually
    *dispatches* branch items to `branch_verdict`.
- **`extractor_self_test`** additionally gains fixtures for: `branches` block form, `branches` flow
  sequence (→ `KEY`, no `ITEM`), `branches-ignore`, and a `workflow_dispatch` input named
  `branches` at depth 3 being correctly ignored.
- **`path_filter_self_test`** is untouched.
- **Mutation battery**, on top of the standing controls: delete the resolve call, force
  `branch_verdict` to return `ok`, drop `branches` from the extractor's key vocabulary, remove the
  `all-negated` widening, revert the `key_positive` move. Each must red.
- **End-to-end negative test.** A temporary `branches: [mian]` in a real workflow must red the gate
  naming the file, the line and the entry. Run `ci/actionlint/run.sh` directly, not `moon run`, so
  a cached PASS cannot replay.
- The full `moon ci … --base origin/main --include-relations` graph before pushing.
- **Post-merge.** `ci.yml`'s `push.branches` filter is read from the merge commit, not from the PR
  head, so a mistake in it is invisible on the PR and silently stops main's post-merge CI and cache
  warming — the SMA-448 shape, applied to the required check's own trigger, and L5 concedes this
  gate cannot prove a workflow ran. After merge, confirm a `CI / moon ci` run appeared on `main`;
  if it did not, revert the `ci.yml` hunk immediately.

## 6. Rollout and rollback

Ships in one PR: the workflow rewrite, `ci/actionlint/run.sh`, `ci/actionlint/README.md`,
`moon.yml` (the `repo:actionlint` `description:` says "every paths: filter glob" and goes stale),
a CLAUDE.md gotcha, and this spec. No Moon task, `T=(…)` or `.prototools` change.

The README edit touches more than the check table. Per D10 no row is renumbered — rows 5, 6 and 7
are reworded in place (5 gains branch resolution, 6 covers all four kinds, 7 becomes four tables) —
but its subtitle, its intro, its depth-rule paragraph, its "Supported glob vocabulary" section and
its "Escape hatches" list all describe a paths-only gate and need a branch counterpart.

**This PR fires the full 7-platform `prebuild` matrix** — `prebuild.yml` lists its own path in
`pull_request.paths`, and D1 edits it. Unavoidable, and expected rather than a regression against
SMA-520's spend work.

**SMA-541 collision.** SMA-541 is in flight and edits the same two files. On this branch's base
(`origin/main`) CLAUDE.md has no `ci-targets` markers and `ci/affected-graph/ci_targets.py` does
not exist; SMA-541 adds both, asserting that `ci.yml`'s `T=(…)` array and CLAUDE.md's
marker-delimited command agree exactly, that `T` stays a single-line bash array, and that a second
copy of either marker anywhere in CLAUDE.md reds the gate. Consequences:

- The two PRs touch different hunks (`ci.yml`'s `on:` block versus its `T=(…)` array; different
  CLAUDE.md sections), so they merge cleanly in either order; whichever lands second rebases.
- This PR's CLAUDE.md gotcha must be **prose only** — no marker strings, no pasted `moon ci …`
  command — or it reds `repo:affected-smoke` the moment SMA-541 lands.
- SMA-525's documented emergency hatch, "drop `:actionlint` from `T=(…)`, one line", becomes wrong
  once SMA-541 lands: it is then two places, `T` **and** the CLAUDE.md marker block. This PR must
  not add a third stale citation of it; `ci/actionlint/README.md:57` already carries one and is
  corrected here.

Rollback is graduated, cheapest first: **(0)** short-circuit the canary, restoring the gate's
zero-ref-dependency behaviour; **(1)** add the offending entry to `BRANCH_SKIP` with a
justification; **(2)** drop the branch-kind regexes from `extract_filter_keys`, disabling the new
check while leaving `paths` intact; **(3)** remove `:actionlint` from `T=(…)` *and* from the
CLAUDE.md marker block.

## 7. Limitations

Stated deliberately, so nothing reads as a stronger guarantee than it is, in the spirit of
SMA-525 §7.

- **L1 — Wildcard branch entries are unverifiable by construction.** `release/**` cannot be
  resolved, so it must be skip-listed. The gate proves such an entry was an explicit decision, not
  that it is correct — and a typo'd wildcard (`mian/**`) receives identical advice to a legitimate
  one. Today all five entries in the repo are wildcard-free and genuinely verified; a repo that
  later adopts `release/**` would see the skip list become the common path rather than the
  exception, at which point this gate's value should be re-assessed rather than assumed.
- **L2 — Ephemeral branches resolve, so a filter naming one passes now and reds later.** Because CI
  fetches all heads (§2.2), a dependabot or feature branch resolves while it exists and stops
  resolving when GitHub deletes it on merge — a red on the only required check, on every open PR
  simultaneously, caused by no code change at all. Nothing here filters on such a branch, and doing
  so would be a deliberate act, but the failure mode is worse than any this gate prevents because
  it is untriggered by the change that causes it. Escape hatch: `BRANCH_SKIP`, or delete the entry.
- **L3 — Negated entries are not validated.** `branch_verdict` returns early on `!`, so a malformed
  exclusion is never checked. Mirrors SMA-525's L7 for the same reason: a broken exclusion can only
  fail to exclude, making the workflow run more often — the fail-safe direction. An all-negated
  block is still caught by check 6.
- **L4 — `tags:` and `tags-ignore:` are not covered.** Identical silent-death property, but real tag
  filters are near-universally wildcards (`v*`), which L1 sends straight to the skip list — the
  check would be close to vacuous while adding parser and fixture surface to the required check.
  Revisit when a tag filter is actually added.
- **L5 — The gate cannot prove a workflow ran.** Inherited from SMA-525's L4. It proves a filter is
  well-formed and its entries name real branches; a filter whose entries all exist but which
  collectively never match a real event is still possible.
- **L6 — `BRANCH_SKIP` is honour-based.** Like `SKIP_PATTERNS`, nothing verifies a skip entry's
  justification. It converts a silent pass into a recorded decision, which is the AC — not a proof.
- **L7 — Empty and whitespace-only entries are silently dropped.** The extractor prints an ITEM only
  when non-empty, so `branches:` followed by `- ''` yields a KEY with zero items and the
  `no-items` message, which says the gate found no entries it could read — false, there is one and
  it is empty. Pre-existing for `paths`; noted because a branch name is likelier to be blanked by
  hand than a glob is.
- **L8 — A stale local checkout reds where CI passes.** The reverse of the divergence the earlier
  draft feared: a branch created on the remote since your last fetch does not resolve locally.
  Remedy is `git fetch`; CI is unaffected.

## 8. Non-goals

- **`branches` + `branches-ignore` coexistence.** Already caught by actionlint with a precise
  `[events]` message (§2.3). A second, weaker copy would not help.
- **Teaching the extractor inline flow sequences.** D1. The workflows move instead.
- **Checking branch existence over the network.** §2.2.
- **An allowlist of long-lived branch names.** D2 — it would immunise L2 at the cost of the
  reality-anchoring that makes the check bite on a doubly-typo'd name.
- **Any change to `paths:` verdicts, `PATTERN` record shape, `path_filter_self_test`, or the
  `T=(…)` array.** AC-3, D8 and §4.4.
