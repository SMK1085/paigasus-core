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
