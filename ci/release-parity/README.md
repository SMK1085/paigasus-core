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

PSR has **no path-based monorepo attribution** — it versions a package from _all_
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

| commit                                         | canonical | semantic-release                  |
| ---------------------------------------------- | --------- | --------------------------------- |
| `fix:`                                         | 0.1.1     | 0.1.1 ✓                           |
| `feat:`                                        | 0.2.0     | 0.2.0 ✓                           |
| `fix!:` / `feat!:` / `fix:`+`BREAKING CHANGE:` | 0.2.0     | **1.0.0** (documented divergence) |

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
won't do. The runner also **strips CI env markers** (`GITHUB_ACTIONS`, `GITHUB_REF`,
`GITHUB_HEAD_REF`, …) before calling the JS API: inside CI, semantic-release's
`env-ci` would otherwise read the ambient PR branch instead of the fixture's `main`
and classify it as a non-release branch → no release (`ci:false` skips CI
_verification_ but not the ambient-branch read).

### Fixture config derived from BOTH real configs (F3)

`build_fixture` reads the classification (the `preset`; absence of `releaseRules`)
from the real `paigasus-sdk` **and** `paigasus-ui` `.releaserc.json`, and fails
loudly if either adds a `releaseRules` clamp (the documented divergence would no
longer hold) or if the two disagree. Both configs are task inputs, so editing
either re-runs this check.

## The negative control runs in CI (SMA-530)

All three Moon tasks run `--negative-control` before the real suite, under an explicit
`set -euo pipefail`:

```yaml
script: |
  set -euo pipefail
  ci/release-parity/run.sh --negative-control
  ci/release-parity/run.sh
```

**Why the real run cannot substitute for it.** Change `run.sh:51` from
`if [ "$got_a" = "$expected" ] && [ "$got_b" = "$BASELINE" ]` to
`if [ "$got_b" = "$BASELINE" ]`. Slot `b` is at baseline in all five `cases.tsv` rows, so
the real run prints `== all parity cases passed ==` and exits 0 — the gate is vacuous. The
control reds. A gate that has lost the ability to report red is green exactly when it
matters.

**Why the `set -euo pipefail` line.** The load-bearing half is `-e` (errexit): Moon does
not enable it for `script:` blocks, so without it the shell simply continues past a failing
control and the block's status becomes its LAST command's — the passing real run. (`-o
pipefail` only changes how a *pipeline's* status is computed, and `-u` only catches unset
variables; neither is what stops the masking.) `run.sh`'s own `set -euo pipefail` does not
help: it governs that script's body, not the Moon block that invokes it twice.

**Why all three tasks.** Their *ecosystem-specific* inputs are distinct
(`moon.yml:57-119`), so a PR touching only `ts/packages/paigasus-sdk/.releaserc.json`
selects `release-parity-ts` and neither sibling; one shared control would leave that PR
uncontrolled. The input sets are **not** disjoint overall — all three also list
`ci/release-parity/**/*` and `.prototools`, and that shared entry is what makes an edit
under `ci/release-parity/` schedule all three at once. Net measured cost
+890ms/+733ms/+1111ms (~20% each). Note the control's per-ecosystem code path is a strict
*subset* of the real run's — the argument is affectedness and symmetry, not extra
adapter coverage.

**What guards it.** `ci/affected-graph/ci_targets.py` pins the nine `moon.yml` lines
(`SELF_SCHEDULED_GATES`) and five discrete lines inside `run.sh` itself
(`RELEASE_PARITY_SH_CALL_SITES`: the flag parse `--negative-control) NEGATIVE=1; shift ;;`,
the `if [ "$NEGATIVE" = 1 ]; then` guard, the assertion body `check_case "neg-fix-bang" …`,
and both report arms — the `exit 0` on "reported red as expected" and the `exit 1` on
"accepted a wrong expectation"), from inside `repo:affected-smoke` — a separately scheduled
gate, so neither judges its own wiring. Five discrete lines, not one span, because pinning
the block as a unit left two MEASURED bypasses with different failure shapes: neutering the
flag parse (dropping `NEGATIVE=1`) leaves `NEGATIVE` at its initialized 0, so the control
branch is never entered and the invocation falls through to the real suite, which then just
runs twice and proves nothing; gutting the assertion body (replacing the `check_case` call
with a bare `ec=1`) never calls the harness at all yet still prints "reported red as
expected" — a control that actively lies rather than one that merely no-ops. Those two are
what pinning five lines instead of one span closes; they are not the full set of ways to
defeat the control — see L5 for a residual that survives all five pins.
`repo:affected-smoke` lists `ci/release-parity/**/*` in its inputs to make this pin
reachable.

### Limitations

- **L1 — `repo:affected-smoke`'s own inputs are unpinned.** Every pin above depends on that
  task being scheduled at all, and NONE of its `inputs` entries is itself pinned by anything —
  including `- 'ci/release-parity/**/*'` (`moon.yml:187`), the entry this branch adds and the
  one `RELEASE_PARITY_SH_CALL_SITES` is reachable through. Deleting that entry is green today
  and would silently un-reach `RELEASE_PARITY_SH_CALL_SITES` for every later PR under
  `ci/release-parity/`. The same is true of `- 'moon.yml'` (`moon.yml:164`): deleting it is
  itself a root `moon.yml` edit, and afterwards the task's remaining globs do not match the
  root file (`*/moon.yml` matches `rs/moon.yml`, not `moon.yml`; `.moon/**/*` does not match
  it), so the removal PR would not schedule the gate. Pre-existing — the `input-liveness` pin
  rests on the same `moon.yml` entry, and matches the accepted `ci/actionlint/**/*` precedent —
  and closing it needs a *containment* variant of `SELF_TASK_EXPECTED_GLOBS`, which is today an
  exact match.
- **L2 — the task-script haystack strips both sides** (`ci_targets.py:792`), so an
  indented copy inside `if false; then … fi` satisfies the pin. The column-0 rule that
  rejects this for the actionlint haystack is unavailable here: Moon task scripts are
  indented inside YAML. Separately, `set +e` inserted *after* the pipefail line satisfies
  all three pins while re-opening the masking they exist to prevent.
- **L3 — whole-line pins are brittle in the false-red direction.** Making the base task's
  ecosystem explicit (`--ecosystem release-plz`), adding a trailing comment, or reordering
  flags reds the gate although nothing is broken. Restore the exact line or update the
  constant.
- **L4 — the control's `0.1.1` is coupled to `cases.tsv`'s contract.** `run.sh:62-63`
  hardcodes it as "deliberately wrong" for `fix!` in 0.x. Should the canonical contract
  ever change so that value becomes correct, all three controls red spuriously and the
  diagnosis is non-obvious.
- **L5 — whole-line pins prove presence, not exclusivity.** All five
  `RELEASE_PARITY_SH_CALL_SITES` entries staying byte-identical proves the required lines are
  still THERE; it proves nothing about what else was added around them. Two bypasses were
  measured that satisfy every one of the five pins while still defeating the control: inserting
  a bare `NEGATIVE=0` on its own line immediately before the `if [ "$NEGATIVE" = 1 ]; then` guard
  (the guard is never entered, execution falls straight through to the real suite, exit 0 — the
  same outcome as the flag-parse bypass above), and deleting the block, neutering the parse, and
  parking all five pinned lines verbatim inside a never-executed heredoc
  (`: <<'DEADPIN' … DEADPIN`). This is the same class L2 records for the task-script haystack —
  it strips both sides too, so the same insertion/heredoc shape applies there just as much as
  here — and the same one `ci_targets.py` already disclaims for the actionlint haystack ("THIS IS
  NOT REACHABILITY ANALYSIS"): parsing bash control flow in Python to close this generally is
  fragile and out of scope (spec decision). The narrower fail-safe available if this ever bites:
  a count assertion such as `release_parity_sh_text.count("NEGATIVE=") == 2`, which can only
  false-red, never silently pass a bypass — not implemented here, just recorded as the option.
