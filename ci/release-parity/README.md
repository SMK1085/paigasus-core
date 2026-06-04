# release→semver parity harness (SMA-398)

Asserts the commit→semver classification contract by dry-running the configured
release tool over synthetic Conventional Commits in a disposable fixture repo.

## Why a multi-crate fixture (F4 / SMA-385)
Release tools map commits to packages by **changed file path**, not commit scope
(SMA-385's root cause). The fixture has two independent crates; every case
touches crate `a` and asserts `a` bumps while `b` stays at baseline — testing
path→package attribution, not just bump magnitude. Do NOT "simplify" this into a
single-crate fixture or scope-only commits: that silently stops testing the bug
class this harness exists for.

## Why the fixture config is derived, not copied (F3)
The fixture `release-plz.toml` is generated from the real `rs/release-plz.toml`
(the classification keys are grepped out, semver-check forced off). A hand-copied
config would drift and validate the wrong settings.

## 0.x degeneracy (F2)
With `features_always_increment_minor = true`, `feat:` already bumps minor, so `feat!:`
and `feat:`+footer are NON-discriminating in 0.x (all = 0.2.0). The breaking
marker is only testable on a **patch-base** (`fix!:`, `fix:`+footer): a tool that
drops the marker yields 0.1.1, which the harness catches. Breaking-vs-feature by
magnitude (breaking → major) only becomes discriminating at 1.0 — the 1.x column
in cases.tsv is staged for that transition.

## python-semantic-release adapter (SMA-405)
Run with `run.sh --ecosystem python-semantic-release`. It reuses this same
`cases.tsv`. PSR is aligned to the canonical 0.x contract via `major_on_zero =
false` + `allow_zero_version = true` (its v10 defaults are `true`/`false`, which
would classify breaking-in-0.x as `1.0.0` — the divergence the gate exists to
catch, per ADR-0011 S6).

### Why one git repo per slot (not one repo, two packages)
PSR has **no path-based monorepo attribution** — it versions a package from *all*
commits since that package's last matching tag, regardless of which files
changed. So the fixture gives each slot (`a`, `b`) its own git repo. For
release-plz, slot `b` staying at baseline tests **path→package attribution**
(SMA-385); for PSR it tests that **PSR invents no release without a qualifying
commit** (PSR is still run on `b` via `version --print`, so it's a real
assertion, not a hardcoded constant). Do NOT "simplify" this into a single shared
repo: PSR would then bump `b` from `a`'s commit and the attribution-equivalent
check would silently break.

### Why the fixture config is derived from BOTH real configs
`build_fixture` reads the classification keys (`major_on_zero`,
`allow_zero_version`) from the real `paigasus-ml` **and** `paigasus-workflows`
`pyproject.toml` (scoped to the `[tool.semantic_release]` table), and fails
loudly if either is missing, if `allow_zero_version` isn't `true`, or if the two
packages disagree. Both packages' configs are task inputs, so editing
`-workflows` re-runs this check — the equality guard stops that edit from passing
green against `-ml`-only settings.

## semantic-release adapter (SMA-406)
Run with `run.sh --ecosystem semantic-release`. It reuses this same `cases.tsv`.

Unlike release-plz and PSR — both aligned to the canonical 0.x contract —
semantic-release has **no version-aware 0.x clamp**, so it cannot be cleanly
aligned (its only lever, commit-analyzer `releaseRules`, is version-blind and
would mis-clamp post-1.0). Per the canonical contract from a `0.1.0` baseline:

| commit | canonical | semantic-release |
|--------|-----------|------------------|
| `fix:`  | 0.1.1 | 0.1.1 ✓ |
| `feat:` | 0.2.0 | 0.2.0 ✓ |
| `fix!:` / `feat!:` / `fix:`+`BREAKING CHANGE:` | 0.2.0 | **1.0.0** (documented divergence) |

So this adapter **documents** the divergence rather than aligning (ADR-0011 S6,
amended 2026-06-04). The harness asserts it via run.sh's generic
`ecosystem::expected` hook (breaking→`1.0.0`); the gate goes **red** if a
semantic-release upgrade changes the classification. The sub-1.0 lifecycle
consequence (TS-native packages leave 0.x on their first breaking change) is
routed to SMA-407.

### Why an in-repo path-filter (not a monorepo plugin)
The canonical `semantic-release-monorepo` (pmowrer) is abandoned + ESM-broken.
Per-package isolation is instead a small in-repo plugin
(`ts/tooling/semantic-release-path-filter.mjs`) that restricts `analyzeCommits`
to commits touching the package dir (via `git log -- .`), then delegates to
`@semantic-release/commit-analyzer`. It is the **only** `analyzeCommits` provider
in the `plugins` array — listing commit-analyzer separately would analyze the
unfiltered set and take the max, defeating the filter.

The configs select the **`conventionalcommits` preset**, NOT commit-analyzer's
`angular` default: the angular preset silently ignores the Conventional Commits
`!` breaking marker, so `fix!:`/`feat!:` would classify as non-breaking. The
`conventionalcommits` preset honors `!` (and the `BREAKING CHANGE:` footer),
matching the canonical contract's commit grammar. It loads with no extra
dependency (commit-analyzer provides it, pinned transitively in `ts/pnpm-lock.yaml`).

### One repo, two package dirs; versions via the JS API
The fixture is a single git repo with two package dirs through that same
path-filter, so slot `b` staying at baseline tests **path→package attribution**
(the mechanism the real `sdk`/`ui` config ships) — paralleling release-plz's
cargo attribution. Next versions are computed via the semantic-release **JS API**
(`ts/tooling/semantic-release-next-version.mjs`, `dryRun`), which returns the
structured next release (or `false`), so the adapter never scrapes the CLI log.
The fixture pushes to a **local bare `origin.git`** (git-ignored in the fixture):
semantic-release runs `git ls-remote --heads origin`, so a placeholder remote
won't do.

### Fixture config derived from BOTH real configs (F3)
`build_fixture` reads the classification (the `preset`; absence of `releaseRules`)
from the real `paigasus-sdk` **and** `paigasus-ui` `.releaserc.json`, and fails
loudly if either adds a `releaseRules` clamp (the documented divergence would no
longer hold) or if the two disagree. Both configs are task inputs, so editing
either re-runs this check.
