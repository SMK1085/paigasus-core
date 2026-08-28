<!-- SPDX-License-Identifier: Apache-2.0 -->

# `repo:publish-metadata`

Asserts every publishable crate is genuinely releasable (SMA-376), and that its
`categories` are real crates.io slugs (SMA-529). A second, Python arm asserts the same
for every PyPI-bound distribution (SMA-578) — the `P*` checks below.

## Checks

| Check | What it asserts | Failure |
|---|---|---|
| 0 | The publishable set equals `EXPECTED_PUBLISHABLE` | 1 (2 if empty) |
| 1 | Metadata crates.io accepts at upload time | 1 |
| 1b | Every category is a real crates.io slug | 1 |
| 1c | Every publishable crate declares its own `[lints.*]` table, and it does not deny | 1 / 2 |
| 1d | Every publishable crate declares a non-empty `include` allowlist naming README.md and LICENSE | 1 / 2 |
| 2 | `cargo publish --dry-run` succeeds, once per publish group | 1 / 2 |
| 2b | The packaged file list ships README + LICENSE, not moon.yml | 1 |
| 3 | A 0.0.0 crate is release-blocked | 1 |
| 4 | The freshness job's call site still exists | 1 / 2 |
| P0 | The PyPI-bound set equals `EXPECTED_PYPI_PUBLISHABLE`, discovered at runtime | 1 / 2 |
| P1 | Every PyPI-bound distribution carries the `[project]` metadata PyPI needs, pairs no SPDX expression with a `License ::` classifier, and — for sdist-shipped crates — carries Check 1c's non-denying lint table | 1 / 2 |
| P2 | The README/LICENSE files those `[project]` fields NAME exist on disk | 1 / 2 |

The `P*` rows are the Python arm (SMA-578). They were absent from this table until
SMA-593; the arm was never undocumented in the gate itself — `run.sh`'s own header
describes each one — but this file did not list them. A fourth Python-arm check
(SMA-578 D6) banned registry credentials in `.github/workflows/wheels.yml`. SMA-593
moved that rule out of this gate entirely: it now lives in `repo:workflow-credentials`,
which parses YAML instead of matching regexes and covers every
`pull_request`/`pull_request_target`-triggered workflow, not `wheels.yml` alone.

Exit codes: `0` pass, `1` the repo is wrong, `2` infrastructure failed. Check 4's `1 / 2`
split: `assert_freshness_call_site` returns `2` when the workflow file is missing or
unreadable (it cannot assert anything), and `1` when the file is readable but the
assertion itself fails (the invocation is gone, its exit status is discarded, or a
`continue-on-error`/`if:` can suppress its red).

### Check 1c — each crate's own lint table, and no `deny`

Cargo inlines a crate's *resolved* `[lints]` table into the manifest it publishes, and
docs.rs builds a published crate on nightly **as the root package**, where `--cap-lints
allow` does not apply (that flag only downgrades lints in *dependency* crates). So an
inherited `[lints] workspace = true` — or a hand-written table that resolves
`lints.rust.warnings` or `lints.clippy.all` to `deny`/`forbid` — silently kills the docs.rs
build the first time a new rustc or clippy lint fires, months after the PR that shipped it
(SMA-577). The check therefore requires a crate's own, non-inherited `[lints.*]` table, and
rejects both TOML spellings of `deny`/`forbid` (the bare string form and the
`{ level = ..., priority = ... }` table form).

### Check 1d — each crate's own `include` allowlist

Cargo's default `include` is "every non-ignored file in the package directory", which is how
`moon.yml` and other repo-local cruft leaks into a package (the defect Check 2b exists to
catch after the fact). Check 1d asserts the *rule* up front: a publishable crate must declare
its own `include` as a non-empty list of plain strings containing the literal entries
`README.md` and `LICENSE`. `include.workspace = true` is rejected explicitly — it is
inheritable the same way `[lints]` is, and a "does cargo package README.md/LICENSE" test
would pass it vacuously. Membership is **literal**, so `include = ["**/*"]` fails by design:
a glob that happens to cover both required files would also reinstate the `moon.yml` leak
Check 2b catches, defeating the point of an allowlist.

**1d's denylist is best-effort and cannot be completed** — see Check 2c, which is what
actually holds the invariant.

### Check 2c — the behavioural catch-all detector

1d rejects catch-all `include` entries by **spelling**, and that approach is unfixable in
principle. Measured against cargo 1.95.0 with a probe crate carrying a private file, all of
these package the whole crate root when listed beside the required literals:

```text
/**    /*    **/    /    **/**    */**    */*    ?*    [a-z]*    **/*.*
```

`./**` and a scoped `src/**/*.rs` do not. `/*` is the counter-intuitive one — cargo applies
it recursively, unlike a strict gitignore reading. Any glob that happens to match everything
belongs on that list, so the set is unbounded and a denylist is the wrong tool.

Check 2c enforces the same rule by outcome: **if no tracked file was held back, the include
matched everything and is a catch-all whatever it is spelled.** That holds for a spelling
nobody has thought of yet.

Two details matter:

- It is a **subset** test (`tracked ⊆ packaged`), not equality. Equality was the first
  implementation and was wrong: a catch-all can also sweep *untracked* files in, making the
  packaged set a strict superset while every tracked file still ships. A probe under
  `--allow-dirty` pulled `.git/**` into the tarball and the equality test passed a genuine
  catch-all. There is a negative-control row pinning that shape.
- It compares against **git-tracked** files, not `find` output. Untracked scratch in a
  working tree would inflate the set and mask a catch-all, and a gate's assertion must be
  about the committed tree.

2c is what makes 1d's incompleteness tolerable rather than load-bearing. 2b remains narrower
still: its `FORBIDDEN_PACKAGED` holds one entry (`moon.yml`), so a crate directory containing
no forbidden file gets nothing from it — which is precisely the gap 2c covers.

**Failure direction:** a crate whose every tracked file is legitimately publishable would
false-red here. That is deliberate, and matches the repo's standing preference that a gate be
allowed to false-red but never to absorb a bypass silently. Today every crate directory
carries a `moon.yml` that must not ship, so the case does not arise.

### Check 2 — one dry-run per publish group

Check 2 now runs `cargo publish --dry-run` once per **publish group**: a connected component
of the in-set dependency graph, computed at runtime from `cargo metadata` (nodes are the
publishable crates, an edge joins A–B when A depends on B and both are publishable). Today
that yields two groups, `{paigasus-kernel}` and `{paigasus-proto-derive, paigasus-proto}`.

This is not a workspace shortcut — it is the registry-faithful form. A *per-package*
dry-run of `paigasus-proto` exits 101 (`no matching package named 'paigasus-proto-derive'
found`) as long as the derive crate is absent from crates.io, because cargo resolves an
in-set path dependency against the registry, not the workspace, on a single-package
`--dry-run`. `cargo publish --dry-run -p paigasus-proto-derive -p paigasus-proto` exits 0
instead, resolving the in-set dependency from a locally staged tarball. `paigasus-kernel`
has no in-set dependency, so it stays a group of one and keeps exactly the assertion it had
before this change — grouping never weakens a crate that didn't need it.

**`CHECK2_INVOKED`** — the guard-the-guard for this call site (SMA-542 shape). The Check 2
helper (`check_publish_group`) appends every package name it was *actually invoked with* to
`CHECK2_INVOKED`, and `assert_check2_covered_everything` compares that recorded set against
the set the per-package Check 2b loop enumerated, exiting 2 on mismatch. Because the record
is written by the helper rather than by its caller, deleting one invocation leaves the
recorded set short and the assertion fires — a one-line deletion is caught. What remains
open is deleting the invocation **and** the assertion together, a two-site edit. Closing that
fully would mean an external pin — `PUBLISH_METADATA_SH_CALL_SITES` in
`ci/affected-graph/ci_targets.py` **plus** adding `ci/publish-metadata/run.sh` to
`repo:affected-smoke`'s `inputs` (without which the pin would serve a cached pass on exactly
the PR that breaks it) — and that is deliberately deferred: pinning one check's call sites
while this file's other four stay unpinned would misrepresent the coverage.

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
  unguarded.** Deleting that one line removes all 39 module controls silently. Closing it
  would mean pinning whole lines into `ci/affected-graph/ci_targets.py` AND adding
  `ci/publish-metadata/**/*` to `repo:affected-smoke`'s `inputs` (without which the pin is
  unreachable on the PR that breaks it) — deliberately deferred as out of scope for SMA-529.
- **L5 — `ABSOLUTE_FLOOR` is 2, which equals the number of slugs the repo currently uses.**
  A hand edit that consistently truncates the snapshot AND its `# count:` header down to
  just the used slugs would pass, leaving Check 1b validating against a near-empty
  vocabulary. The scheduled freshness job reds the next morning, and a truncation that drops
  a *used* slug reds rather than greens.
