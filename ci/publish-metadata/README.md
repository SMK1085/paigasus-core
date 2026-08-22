<!-- SPDX-License-Identifier: Apache-2.0 -->

# `repo:publish-metadata`

Asserts every publishable crate is genuinely releasable (SMA-376), and that its
`categories` are real crates.io slugs (SMA-529).

## Checks

| Check | What it asserts | Failure |
|---|---|---|
| 0 | The publishable set equals `EXPECTED_PUBLISHABLE` | 1 (2 if empty) |
| 1 | Metadata crates.io accepts at upload time | 1 |
| 1b | Every category is a real crates.io slug | 1 |
| 2 | `cargo publish --dry-run` succeeds | 1 / 2 |
| 2b | The packaged file list ships README + LICENSE, not moon.yml | 1 |
| 3 | A 0.0.0 crate is release-blocked | 1 |
| 4 | The freshness job's call site still exists | 1 / 2 |

Exit codes: `0` pass, `1` the repo is wrong, `2` infrastructure failed. Check 4's `1 / 2`
split: `assert_freshness_call_site` returns `2` when the workflow file is missing or
unreadable (it cannot assert anything), and `1` when the file is readable but the
assertion itself fails (the invocation is gone, its exit status is discarded, or a
`continue-on-error`/`if:` can suppress its red).

## The category snapshot

`crates-io-categories.txt` is a committed snapshot of
`https://crates.io/api/v1/category_slugs`. Refresh it with:

```bash
ci/publish-metadata/run.sh --refresh-categories
```

Two things to know before touching it:

- **crates.io returns 403 without a `User-Agent`.** The pinned one lives in
  `categories.py`.
- **Use `/api/v1/category_slugs`, not `/api/v1/categories`.** The latter returns only the
  58 top-level categories and would falsely reject every `::`-nested subcategory.

### Case sensitivity — the trap

crates.io's **publish** path matches slugs exactly and case-sensitively
(`update_crate` uses `categories::slug.eq_any(slugs)` with no lowercasing). Its **read**
API lowercases (`with_slug` uses `lower(slug)`), so `GET /api/v1/categories/Data-Structures`
returns 200 while publishing `Data-Structures` silently drops the category.

Check 1b is therefore **exact**, and the negative control pins it. Do not "fix" a
case-mismatch red by relaxing the comparison.

### Why an unknown slug is invisible without this gate

crates.io drops unknown slugs and publishes anyway. cargo *does* print
`the following are not valid category slugs and were ignored: …` — but only after
`registry.publish()`, and `cargo publish --dry-run` returns before that call. So Check 2
provably cannot see it, and the only moment it appears is the irreversible upload.

## Limitations

- **L1 — a single combined edit defeats Check 4.** `assert_freshness_call_site` now
  requires the invocation on a real, non-comment `run:` line with no discarded exit status,
  and rejects a stray `continue-on-error:` or `if:` in the workflow. What still gets past it
  is deleting the `assert_freshness_call_site` call from `run.sh` itself *and* removing the
  freshness job from `security-scan.yml` in the same commit — the pin lives inside the file
  it is pinning, so removing both at once leaves nothing to catch it. Same bounded shape as
  `ci/actionlint/README.md`'s L6.
- **L2 — removal detection is not in a required check.** `moon ci` is the only required
  status check; the freshness job is not. A slug retired upstream is caught on the PR path
  only after the 90-day staleness bound forces a refresh.
- **L3 — the staleness bound reds a PR unrelated to categories.** By design: the
  alternative is a freshness mechanism that can switch itself off silently.
- **L4 — the `categories.py --self-test` invocation inside `negative_control` is itself
  unguarded.** Deleting that one line removes all 35 module controls silently. Closing it
  would mean pinning whole lines into `ci/affected-graph/ci_targets.py` AND adding
  `ci/publish-metadata/**/*` to `repo:affected-smoke`'s `inputs` (without which the pin is
  unreachable on the PR that breaks it) — deliberately deferred as out of scope for SMA-529.
- **L5 — `ABSOLUTE_FLOOR` is 2, which equals the number of slugs the repo currently uses.**
  A hand edit that consistently truncates the snapshot AND its `# count:` header down to
  just the used slugs would pass, leaving Check 1b validating against a near-empty
  vocabulary. The scheduled freshness job reds the next morning, and a truncation that drops
  a *used* slug reds rather than greens.
