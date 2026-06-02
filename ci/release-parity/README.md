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
With `always_bump_minor_for_0 = true`, `feat:` already bumps minor, so `feat!:`
and `feat:`+footer are NON-discriminating in 0.x (all = 0.2.0). The breaking
marker is only testable on a **patch-base** (`fix!:`, `fix:`+footer): a tool that
drops the marker yields 0.1.1, which the harness catches. Breaking-vs-feature by
magnitude (breaking → major) only becomes discriminating at 1.0 — the 1.x column
in cases.tsv is staged for that transition.
