<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-529 — crates.io category slug validation in `repo:publish-metadata`

**Status:** design (revised after adversarial challenge)
**Date:** 2026-08-22
**Issue:** [SMA-529](https://linear.app/smaschek/issue/SMA-529/ci-validate-cratesio-category-slugs-in-the-publish-metadata-gate)
**Deferred from:** SMA-376 (PR 128), which added `repo:publish-metadata`

## Problem

`ci/publish-metadata/run.sh` Check 1 enforces crates.io's *countable* metadata rules — at
most 5 keywords of at most 20 chars each matching `^[A-Za-z0-9][A-Za-z0-9_-]*$`, at most 5
categories, a description of at most 1000 chars. It never checks that a category is a
**real crates.io slug**.

crates.io **drops** unknown category slugs and publishes the crate anyway, uncategorized —
losing exactly the discoverability the `categories` field exists to buy.

The issue states this happens with "no error anywhere — at publish time or after". That is
**not quite right**, and the correction strengthens the case rather than weakening it. On a
real upload crates.io returns the rejected slugs in `warnings.invalid_categories`, and
cargo prints them (`src/ops/registry/cargo_publish.rs:677-690`):

> `the following are not valid category slugs and were ignored: …`

But that warning is unreachable from this gate: `cargo publish --dry-run` — precisely what
Check 2 runs — returns at `cargo_publish.rs:661-664` (`"aborting upload due to dry run"`)
**before** `registry.publish()` at line 667. So the only moment the warning appears is the
one irreversible moment, scrolling past in a publish log, on a version number that can
never be reused. A warning at that point is not a control.

`paigasus-kernel`'s two slugs (`data-structures`, `parser-implementations`) were verified
by hand during SMA-376 and are correct today. Nothing keeps them correct, and SMA-388 will
add `paigasus-proto` with its own slugs.

## Measured facts

Established empirically or from upstream source on 2026-08-22, not assumed.

| Fact | Evidence |
|---|---|
| `GET /api/v1/category_slugs` returns **all 104** slugs, including `::`-nested subcategories (`aerospace::drones`, `science::bioinformatics::genomics`). | live `curl` → `{"category_slugs":[{id,slug,description}, …]}` |
| `GET /api/v1/categories` returns only the **58 top-level** categories. Validating against it would falsely reject every legitimate subcategory slug. | `meta.total` = 58 |
| crates.io returns **403 without a `User-Agent`**. | `curl -H 'User-Agent:'` → 403; descriptive UA → 200 |
| **The publish path matches slugs EXACTLY and case-sensitively.** | crates.io `crates/crates_io_database/src/models/category.rs:53` — `update_crate` uses `.filter(categories::slug.eq_any(slugs))`, and builds `invalid_categories` at :57-61 via `c.slug == **s`. No lowercasing. |
| The **read** route is case-*in*sensitive — which is why a naive probe misleads. | same file :37 — `with_slug` uses `categories::slug.eq(crate::fns::lower(slug))` |
| cargo warns on invalid categories only on a real upload; `--dry-run` returns first. | cargo `src/ops/registry/cargo_publish.rs:661-664` vs `:677-690` |
| The gate is **already network-bound** via Check 2's `cargo publish --dry-run`. | `ci/publish-metadata/run.sh` Check 2 + `classify_cargo_failure` |
| `:publish-metadata` is **already** in `ci.yml`'s `T=(…)` array. | `.github/workflows/ci.yml:214` |
| **`moon ci` is the only required status check.** `security-scan.yml` is not required. | ruleset 17082810 → `required_status_checks` = `["moon ci"]` |

### The case-sensitivity correction

An earlier draft of this design asserted crates.io matched case-insensitively, on the
evidence that `GET /api/v1/categories/Data-Structures` returns 200, and specified a
case-insensitive membership test.

**That evidence measured the wrong code path.** The read route lowercases; the publish
route does not. `Data-Structures` in a `Cargo.toml` is therefore dropped by crates.io while
a case-insensitive test would have passed it — a false green in exactly the failure class
this gate exists to catch.

The membership test is **exact and case-sensitive**, and the negative control asserts that
a differently-cased slug **reds**. Recorded at length because the wrong version was
plausible, was "measured", and would have been invisible.

## Decision: vendored snapshot + scheduled freshness + an offline staleness bound

The PR path stays deterministic and offline, validating against a committed snapshot. A
daily scheduled job refetches live and reds on drift. The snapshot's provenance header
carries its fetch date, and the **offline** check reds when that date is too old.

That third element exists because of a defect the challenge found in the earlier draft,
which claimed "the snapshot is current or the job is red; there is no third state". The
third state is *the job never ran*: `security-scan.yml`'s own header records that GitHub
"delays or silently drops scheduled runs under load", and GitHub disables `schedule:`
triggers entirely after 60 days of repository inactivity. Neither produces a red. A
freshness design whose only mechanism is a cron is a design that can switch itself off
silently — the exact failure `repo:actionlint` was filed for, one trigger key over.

The staleness bound converts a dead cron into a red on a path someone actually reads, and
it is the only mechanism here that does not depend on the cron.

**It also gives removal detection a route into the required gate**, which matters because
`moon ci` is the only required check. The chain: the staleness bound forces a
`--refresh-categories` within the window → the refresh rewrites the snapshot from live →
if a slug the repo *uses* was retired upstream, it is now absent from the snapshot → the
offline used-slug membership check reds inside `moon ci`. Removal detection is therefore
bounded by the staleness window rather than resting solely on a non-required job.

### Rejected alternatives

*Live fetch on every run.* The issue's stated reason ("puts a network dependency inside a
gate") is weaker than it reads — Check 2 already reaches crates.io. The real cost is
narrower and still decisive: `cargo publish --dry-run` fetches the **sparse index**
(`index.crates.io`, a static CDN); the categories endpoint is the **crates.io API** (a
Rails application that rate-limits and 503s). Putting it on the PR path means an unrelated
PR reds when the API has a bad minute, for an answer that changes a few times a year.

*Warn-only.* A warning inside a gate nobody reads is not a control — and per the Problem
section, that is precisely the status quo cargo already provides at publish time.

*A hand-maintained approved-slug allowlist.* Matches the file's `EXPECTED_PUBLISHABLE`
idiom and needs no network, but never contacts crates.io: it only forces a typo to be made
twice, and cannot notice a retired slug.

*Auto-opening a refresh PR on drift.* Would remove the scheduled noise and let the snapshot
self-heal, but needs `contents: write` + `pull-requests: write` against a workflow that is
deliberately `contents: read`. Real machinery for a 104-line file. Revisit if the reds
become an actual annoyance.

## Design

### 1. The snapshot — `ci/publish-metadata/crates-io-categories.txt`

One slug per line, sorted, deduplicated. Leading `#` lines carry provenance and are ignored
by the parser:

```
# crates.io category slugs — DO NOT EDIT BY HAND.
# source: https://crates.io/api/v1/category_slugs
# fetched: 2026-08-22
# count: 104
# refresh: ci/publish-metadata/run.sh --refresh-categories
```

Plain text over raw JSON: the check needs only the slug, and a drift diff is then one
readable line per added/removed category.

**Sorting is done in Python (`sorted()`, codepoint order), never shell `sort`.** Collation
is locale-dependent and slugs are hyphen-rich: under `en_US.UTF-8` (macOS default) the
hyphen is ignored at primary weight so `database` < `data-structures`; under `LC_ALL=C` the
reverse. A refresh on a developer's Mac would otherwise produce a different byte order than
one on `ubuntu-latest` — the same BSD-vs-GNU class that cost a cycle on PR 150. **The
freshness comparison is set-based, not text-based**, so ordering can never cause a false
red; ordering serves diff readability only.

`count:` is load-bearing — see §3.

### 2. PR-path check — Check 1b, offline

`metadata_checks` gains a fourth argument, the snapshot path, passed as a path rather than a
value for the same reason the existing three are: `--negative-control` drives the identical
code with fixtures.

For every publishable crate, each `categories` entry must appear in the snapshot, compared
**exactly**. Entries are stripped of surrounding whitespace before comparison; a non-string
entry is a repo error, not a crash. A miss appends to the existing `errors` list and
therefore exits **1**, uniform with every other Check 1 rule:

```
paigasus-kernel: category 'data-structure' is not a crates.io category slug.
  crates.io DROPS unknown slugs — the publish succeeds and the crate appears
  uncategorized, and `cargo publish --dry-run` cannot surface the warning.
  Did you mean 'data-structures'?
  Valid slugs: ci/publish-metadata/crates-io-categories.txt
```

The "did you mean" clause is `difflib.get_close_matches(slug, snapshot, n=1, cutoff=0.8)` —
stdlib, deterministic, one line — and is **omitted entirely when it returns nothing**. It is
specified this precisely because the earlier draft said only "simple edit-distance", which
two implementers would build two ways and then assert differently in fixtures.

### 3. Non-vacuity guards

The issue's binding constraint: this must not become a fourth vacuous path. Each condition
below makes the check unable to assert anything, and none may pass.

**Exit-code assignment follows the file's own contract** (`0` pass / `1` the repo is wrong /
`2` infrastructure), which puts most of these at **1**, not the 2 the issue suggested. The
precedent is in this very file: `run.sh:127-137` deliberately routes a *missing*
`release-plz.toml` to exit 1, commenting "that IS the repo defect Check 3 exists to catch,
not an infrastructure problem". `ci/affected-graph/ci_targets.py:326-342` says the same and
explains why — rc 2 "would triage as 're-run the job'" for what is an authorial mistake.
Re-running fixes none of these. The issue's requirement is *never pass*, and that is met.

| Condition | Exit | Rationale |
|---|---|---|
| Snapshot missing / unreadable | 1 | A deleted tracked file is an authorial mistake. Message: restore it, or run `--refresh-categories`. |
| Parsed slug count ≠ the header's `count:` | 1 | Truncation detector, see below. |
| Zero parsed slugs, or no `count:` header | 1 | Corrupt committed data. |
| A line fails the corruption check | 1 | Corrupt committed data. |
| Snapshot `fetched:` date older than 90 days | 1 | The staleness bound. |
| Live fetch fails / non-200 / unparseable / empty | 2 | Genuine infrastructure; re-running is the right triage. |

**Truncation is caught by the header count, not a magic floor.** An earlier draft proposed
`MIN_CATEGORY_SLUGS = 50` justified by the Dependabot Cargo lockfile truncation (543 → 172
packages). The challenge showed the number fails on its own terms: a floor of 50 against a
live 104 tolerates a 52% cut, so *the cited incident's own ratio would have passed it* — and
it is a hand-maintained constant that goes stale in the wrong direction as crates.io grows.
`--refresh-categories` writes the entry count into the header; the parser asserts
`parsed_count == header_count`. That catches truncation exactly, is self-maintaining, and
needs no threshold. A small absolute floor of 2 remains only as a backstop against a
both-header-and-body truncation.

**The line check is a corruption detector, not a grammar.** It rejects a line containing
whitespace, an uppercase character, a byte outside `[a-z0-9:-]`, or exceeding 64 chars. It
is deliberately *not* the full slug grammar: crates.io owns this vocabulary, and a strict
grammar would mean the day upstream introduces a slug shaped unexpectedly,
`--refresh-categories` writes a snapshot the offline check then rejects — and since
`:publish-metadata` is in `ci.yml`'s `T=(…)`, that is **every PR red with no path forward
except editing the gate under a red CI**. An external party's routine data change must not
become a self-inflicted outage. An `ALLOW_SLUG_SHAPE` table with a required non-blank reason
provides the escape hatch, matching `SKIP_PATTERNS` / `BRANCH_SKIP` / `ALLOW_DEAD_INPUT` /
`T_EXEMPT` elsewhere in `ci/`.

**CRLF is stripped before any of this.** The repo ships no `.gitattributes` — stated as a
known fact in `ci/affected-graph/ci_targets.py:62-66` — so on a CRLF checkout every line
would carry a trailing `\r`, fail the corruption check, and red every PR with a "corrupt
snapshot" message that is wrong. Lines are `.rstrip()`ed and blanks skipped before
validation.

### 4. Fetch and refresh

Network access uses **Python `urllib`, not `curl`**. Nothing under `ci/` shells out to a
host `curl` today (`ci/images/run.sh` uses a digest-pinned `curlimages/curl` container), and
Python is already this file's scripting language. It also removes the flag-portability
question and the `local x="$(cmd)"` exit-status masking trap that `run.sh:387-388` already
documents.

Two pure functions, both fixture-driveable:

- **`validate_live_payload(http_status, body) -> slugs | error`** — the half that decides
  whether a fetch is trustworthy. Rejects non-200, a non-JSON body (an HTML CDN error page),
  a missing `category_slugs` key, a truncated body, and an empty array.
- **`diff_slug_sets(live, snapshot) -> added, removed`** — set-based.

`fetch_live_slugs()` is a thin wrapper: request with the pinned User-Agent
`paigasus-core-ci (+https://github.com/SMK1085/paigasus-core)` (measured: 403 without one),
a 30s timeout, and 3 retries with exponential backoff, then hand the result to
`validate_live_payload`. Declarations and assignments stay on separate lines.

Two new dispatch arms:

- **`--refresh-categories`** — fetch, validate, then write **via temp file + atomic rename**,
  so a failed or empty fetch can never truncate the committed snapshot. Exits 0 on success,
  2 on fetch/validation failure. **Refuses to run when `CI` is set** (it mutates the tree).
  Writes to an absolute path under `$REPO_ROOT`, since `main()` does `cd "$RS_DIR"`.
- **`--check-categories-freshness`** — fetch, validate, `diff_slug_sets` against the
  snapshot. Exit 1 on drift listing added and removed separately; exit 2 on fetch or
  validation failure.

Neither mode ever falls back to the snapshot on a failed fetch — a silent fallback is the
vacuous path the issue forbids. The usage string at `run.sh:419` gains both arms, and the
unrecognized-argument-exits-2 dispatch contract is preserved.

### 5. Strict equality on drift

Any difference reds — an upstream addition as much as a removal — fixed by one command plus
a commit. Expected cost is roughly 2–5 red scheduled runs a year.

The challenge argued for redding on removals only and merely reporting additions, on the
grounds that additions generate all the noise and protect against none of the
correctness-critical direction. That is a fair reading and is **raised for re-decision at
the approval gate**; the strict-equality policy here reflects an explicit prior choice, and
it is now backstopped by the staleness bound (§3) so the cron is no longer the only
mechanism. Because `security-scan.yml` is not a required check, this noise never blocks a
merge.

### 6. Scheduled wiring — a second job in `security-scan.yml`

A `category-slug-freshness` job on the existing 07:17 UTC cron, `permissions: contents:
read`, `timeout-minutes: 10` (the sibling `osv` job carries 15), running
`ci/publish-metadata/run.sh --check-categories-freshness` **directly, not through `moon
run`** — Moon reports the task cached whenever the tree is unchanged, which is the normal
case here and precisely the state the job exists to re-check. The `osv` job documents the
identical reasoning.

`ci/publish-metadata/**` joins the workflow's existing `pull_request.paths:` **block
sequence** (never the inline flow form — `repo:actionlint`'s extractor fails all four
trigger keys loudly on inline flow), so the job self-validates on the PR that changes it.
**This means a PR touching the gate reds if the snapshot is stale**, including this one.
That is intended, not an oversight: a PR editing the gate is the right moment to require a
fresh snapshot. The earlier draft wired this without saying so, contradicting its own
argument about mid-feature reds.

That workflow's header comment ("A red run here means an advisory landed on shipped code")
becomes inaccurate once a second, non-advisory job lives there. Both the comment and the new
job's name and failure message must carry the distinction.

### 7. Guarding the guard

`--check-categories-freshness` is invoked in exactly one place, a workflow job scheduled by
GitHub rather than Moon. Deleting that job, typo'ing the flag into a different dispatch arm,
or adding `continue-on-error: true` is **silent and permanent**. `repo:actionlint`'s check
8/8b/8c machinery is keyed on `.github/workflows/ci.yml` only
(`ci/affected-graph/ci_targets.py:298-322`); `repo:input-liveness`, `repo:affected-smoke`
and the `T=(…)` array all cover Moon tasks. None covers a workflow job.

This is the repo's recurring **guard-the-guard** failure: a new check's own call site is
what goes unguarded, because fixture tables exercise the verdict function and never its
invocation, so deleting the production block passes green (SMA-542).

So: `run.sh` gains a check asserting that `.github/workflows/security-scan.yml` contains the
literal `--check-categories-freshness` invocation line and that no `continue-on-error:` value
other than the literal `false` covers that step — mirroring `ACTIONLINT_SH_CALL_SITES`. That
file joins `repo:publish-metadata`'s `inputs`, so the assertion cannot serve a cached pass on
the PR that breaks it. The check is a counted self-test in `--negative-control`, not a bare
assertion.

### 8. Negative control

Every row drives the same code the real run drives.

**Two existing defects must be repaired first, or this change silently voids the eleven
controls already in the file.** `_expect_red` (`run.sh:286-294`) asserts only *non-zero*.
Adding a fourth argument to `metadata_checks` while the eleven call sites at `run.sh:307-347`
still pass three would make the Python heredoc raise `IndexError` and exit 1 — so every one
of Check 0's, Check 1's and Check 3's controls would report "ok — reports red" while proving
nothing about the rule it names. Separately, the shared `base` fixture at `run.sh:300-302`
carries `"categories":["c"]`, which is not a real slug: once a snapshot is threaded through,
every derived fixture would red on the bogus category rather than on its own mutation, and
the clean positive control at `run.sh:359` would fail outright.

Required repairs:

1. All eleven existing `_expect_red` call sites pass a valid snapshot path.
2. `base`'s `categories` becomes a real slug drawn from the snapshot.
3. `_expect_red` is replaced by **`_expect_rc <want-rc> <label> <cmd…>`**, asserting an
   exact code. The file's headline invariant is the 1-vs-2 contract (`run.sh:20`); a harness
   that cannot tell them apart leaves it unasserted. Existing rows are re-expressed too.
4. `metadata_checks` treats an absent or blank 4th argument as **exit 2** with an explicit
   "the snapshot path was not passed" message — never as "skip Check 1b".

New fixtures:

| Fixture | Expected |
|---|---|
| Category not in the snapshot | rc 1 |
| Category valid but differently cased (`Data-Structures`) | **rc 1** — pins case-sensitivity against the publish path |
| `::`-nested category present in the snapshot | rc 0 |
| `::`-nested category absent from the snapshot | rc 1 |
| Category with surrounding whitespace | rc 1 |
| Missing snapshot file | rc 1 |
| Snapshot present but empty | rc 1 |
| Parsed count ≠ header `count:` | rc 1 |
| No `count:` header | rc 1 |
| Line with CRLF only (should be tolerated) | rc 0 |
| Line failing the corruption check | rc 1 |
| `fetched:` date older than 90 days | rc 1 |
| 4th argument absent | rc 2 |
| `validate_live_payload`: 403 body | rc 2 |
| `validate_live_payload`: HTML error page | rc 2 |
| `validate_live_payload`: `{"category_slugs":[]}` | rc 2 |
| `validate_live_payload`: truncated JSON | rc 2 |
| `validate_live_payload`: valid payload | rc 0 |
| `diff_slug_sets`: live has an added slug | rc 1 |
| `diff_slug_sets`: live dropped a vendored slug | rc 1 |
| `diff_slug_sets`: identical sets | rc 0 |
| Call-site pin: freshness invocation present | rc 0 |
| Call-site pin: invocation line removed from the workflow | rc 1 |
| Call-site pin: step carries `continue-on-error: true` | rc 1 |

The case-sensitivity and CRLF rows are controls, not conveniences: the first stops a future
"relax this to case-insensitive" edit from silently reintroducing the false green; the second
stops a CRLF checkout from redding every PR.

### 9. Moon wiring

`inputs` gains **two literal paths**, and the existing `ci/publish-metadata/run.sh` literal
stays:

```yaml
- 'ci/publish-metadata/run.sh'
- 'ci/publish-metadata/crates-io-categories.txt'
- '.github/workflows/security-scan.yml'
```

An earlier draft widened this to the glob `ci/publish-metadata/**/*`. The challenge showed
that trades a strong pin for a weak one: `ci/affected-graph/task_inputs.py` checks
`inputFiles` for *exact tracked membership* but checks `inputGlobs` only for "matches ≥ 1
tracked file" — which `run.sh` alone satisfies forever, so deleting the snapshot would stop
redding `repo:input-liveness`. It also silently assumed wax collapses `**/` to zero
components. Literal paths keep both files exactly pinned and make the wax question moot.

**No `T=(…)` or CLAUDE.md marker change**: `:publish-metadata` is already in the array and no
new Moon target is added, so `repo:affected-smoke` is untouched.

Known cost: the snapshot is now a task input, so a data-only refresh commit re-keys the task
and drags a full `cargo publish --dry-run` verify build. Acceptable a few times a year.

### 10. `ci/publish-metadata/README.md`

`ci/actionlint/`, `ci/affected-graph/`, `ci/error-registry/` and `ci/release-parity/` each
carry one; this directory does not. It gains one recording the refresh workflow, the
User-Agent requirement, the publish-vs-read case-sensitivity finding, and a **Limitations**
section in the style of `ci/actionlint/README.md`'s L6 — see Residual risks below.

## Out of scope

**SPDX validation of `license`.** The issue raises it as a secondary question. Excluded, and
not merely because risk is lower: an invalid SPDX expression is **rejected** by crates.io at
upload — a loud failure, the exact opposite of the silently-dropped category this gate is
being extended to catch. It needs no gate to make it visible. The field is also
workspace-inherited and stable. Recorded as a decision, not an omission.

## Residual risks

1. **A single combined edit** that removes both the freshness job from `security-scan.yml`
   *and* the call-site pin from `run.sh` passes green. Same bounded shape as the `T`-array
   cycle documented in `ci/actionlint/README.md` L6.
2. **Removal detection is not in a required check.** It reaches `moon ci` only via the
   staleness bound forcing a refresh (§3), so the worst-case detection latency for a retired
   slug is the 90-day window.
3. **The 90-day staleness bound reds a PR unrelated to categories.** The fix is one command;
   the alternative is a freshness design that can switch itself off silently.

## Success criteria

1. A typo'd category slug in any publishable crate reds `repo:publish-metadata` with rc 1,
   naming the crate, the bad slug, and — when one qualifies — the nearest valid slug.
2. A validly-spelled slug in non-canonical case reds with rc 1.
3. A missing, empty, count-mismatched, or corrupt snapshot reds — never passes.
4. A snapshot older than 90 days reds with rc 1 and quotes the refresh command.
5. `--check-categories-freshness` reds rc 1 on any live/snapshot difference and rc 2 rather
   than passing when the fetch fails or returns an untrustworthy payload.
6. Removing the freshness invocation from `security-scan.yml`, or suppressing it with
   `continue-on-error: true`, reds `repo:publish-metadata`.
7. `--negative-control` exercises every row in §8 with **exact** rc assertions, including the
   eleven pre-existing rows, and passes.
8. `moon ci` over the full target list is green.

Criterion 5's live half and `--refresh-categories` regenerating a green snapshot are
network-dependent and therefore **not** assertable in `--negative-control`; they are verified
once by hand during implementation and thereafter by the scheduled job.
