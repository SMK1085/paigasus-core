# SMA-362 — Dependabot setup (CLA deferred)

**Status:** Design approved
**Date:** 2026-06-04
**Linear:** SMA-362
**Branch:** `feature/sma-362-dependabot-cla-bot-setup`

## Problem

The repo has no automated dependency updates. Four ecosystems (Cargo, npm/pnpm, uv,
GitHub Actions) drift manually, and the GitHub Actions in `ci.yml` are SHA-pinned by hand.
SMA-362 asks for `dependabot.yml` with **grouped weekly** updates per ecosystem (one
consolidated PR per ecosystem, not a PR-per-dependency storm), plus a CLA bot.

The issue explicitly allows splitting: Dependabot is straightforward and valuable from day
one; the CLA "can wait until just before the first external PR is expected" (priority Low).

## Scope decision

**This issue delivers Dependabot only.** The CLA acceptance criteria were split into a
follow-up Linear issue — **SMA-408** ("CLA bot setup (cla-assistant)", Low, `area:deps`, MVP
milestone, related to SMA-362, do-before-first-external-PR). SMA-362 was retitled "Dependabot
setup" and its description trimmed to the Dependabot ACs (CLA portion → SMA-408).

## The CI gates a Dependabot PR must pass

A Dependabot PR runs the full `ci.yml`. Two gates are the reason "first Dependabot PR runs
cleanly through CI" is not automatic:

1. **commitlint** (`ts:commitlint`, PRs only) enforces Conventional Commits via the shared
   `@paigasus/commitlint-config`: `type-enum`, `scope-enum`, `scope-empty: never`,
   `header-max-length: 100`, **`body-max-line-length: 100`**. Dependabot commits are linted
   like any other.
2. **`moon ci`** (`:build :test :lint :fmt :typecheck …`) — the dependency bump itself must
   build/test green. Nothing Dependabot-specific; covered by the existing graph.

The commitlint config already permits a `deps` scope and `build`/`ci`/`chore` types
(`ts/packages/commitlint-config/index.cjs`), so a correctly-prefixed Dependabot **subject**
passes. The **body** is fine too: although Dependabot bodies have long lines, commitlint's
`body-max-line-length` has a **built-in carve-out for lines containing a URL**, and real
Dependabot bodies are dominated by URL lines (`Bumps [x](url)…`, `- [Release notes](url)`,
`- [Commits](url)`). Verified empirically — a realistic grouped Dependabot body passes
`body-max-line-length` with no special handling (see §2). So neither gate needs a
Dependabot-specific exemption; the only real work is getting the **subject** prefix right.

## Decision

### 1. `.github/dependabot.yml`

Four `updates` entries, one per ecosystem, all weekly (Monday 06:00 UTC), each with a single
minor+patch group and a hardcoded Conventional-Commit prefix. No SPDX header (repo YAML
config files don't carry one — verified against `ci.yml` and all `*.yml`).

```yaml
version: 2
updates:
  # ---- Rust: Cargo workspace at rs/ ----
  - package-ecosystem: cargo
    directory: /rs
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
      timezone: Etc/UTC
    commit-message:
      prefix: "build(deps)"
      prefix-development: "build(deps)"
    groups:
      cargo-minor-patch:
        applies-to: version-updates
        update-types: ["minor", "patch"]

  # ---- JS/TS: pnpm workspace + catalog at ts/ ----
  - package-ecosystem: npm
    directory: /ts
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
      timezone: Etc/UTC
    commit-message:
      prefix: "build(deps)"
      prefix-development: "build(deps)"
    groups:
      npm-minor-patch:
        applies-to: version-updates
        update-types: ["minor", "patch"]

  # ---- Python: uv workspace at py/ ----
  - package-ecosystem: uv
    directory: /py
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
      timezone: Etc/UTC
    commit-message:
      prefix: "build(deps)"
      prefix-development: "build(deps)"
    groups:
      uv-minor-patch:
        applies-to: version-updates
        update-types: ["minor", "patch"]

  # ---- GitHub Actions: workflows at repo root ----
  - package-ecosystem: github-actions
    directory: /
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
      timezone: Etc/UTC
    commit-message:
      prefix: "ci(deps)"
    groups:
      actions-minor-patch:
        applies-to: version-updates
        update-types: ["minor", "patch"]
```

**Commit prefix (Conventional-Commit safe).** Hardcoded in `commit-message.prefix` so the
subject is `<type>(deps): …` with the enum-allowed `deps` scope:

- cargo / npm / uv → **`build(deps)`** (Angular convention: `build` = external deps)
- github-actions → **`ci(deps)`** (CI configuration)

We deliberately do **not** use Dependabot's `include: "scope"`, which would emit
`build(deps-dev): …` for dev dependencies — and `deps-dev` is **not** in the repo's
`scope-enum`, so it would fail commitlint. Setting `prefix` and `prefix-development` to the
same `build(deps)` keeps every subject inside the enum. (github-actions has no dev/prod
split, so only `prefix` is set there.)

**Grouping (majors arrive separately).** Each group covers `update-types: [minor, patch]`
only, so **major** bumps come as individual PRs that get reviewed one at a time. This is a
deliberate reading of the AC's "at most one PR per ecosystem per week": minor+patch (the
high-volume, low-risk churn) is capped at one grouped PR per ecosystem; majors (rare,
higher-risk) are intentionally un-grouped for individual attention. The alternative — adding
`"major"` to `update-types` to strictly cap at one PR — was rejected as hiding breaking
bumps inside a green-looking group.

**Defaults left implicit.** `open-pull-requests-limit` (default 5), `target-branch`
(default `main`), and `versioning-strategy` (default `auto`) are not set — the defaults are
correct here and explicit values would be noise.

### 2. No commitlint change needed (URL carve-out)

An earlier draft added an `ignores` predicate to exempt Dependabot's long bodies. **Dropped as
YAGNI** after verifying it isn't needed: commitlint's `body-max-line-length` already ignores
any line **containing a URL**, and Dependabot body lines are URL-laden. Empirical check against
the live `ts/commitlint.config.cjs` (commitlint 21.0.1, `--edit <file>`):

| Commit body | `body-max-line-length` |
| --- | --- |
| Realistic grouped Dependabot body (`Bumps [x](url)…`, `- [Commits](url)`) | **passes** |
| Long line **with** a URL | **passes** (carve-out) |
| Long line **without** a URL | fails |

So **`ts/commitlint.config.cjs` and the published `@paigasus/commitlint-config` are left
untouched**. The subject is kept Conventional purely by the `dependabot.yml` prefix.

**Reactive fallback (only if it ever fires).** The single residual risk is a Dependabot line
that exceeds 100 chars *without* a URL (e.g. a very long group name). If a real Dependabot PR
ever fails CI on `body-max-line-length`, add an `ignores` predicate to the **consumer** config
`ts/commitlint.config.cjs` (not the published package), keyed on Dependabot's sign-off trailer:
`ignores: [(m) => /^Signed-off-by: dependabot\[bot\]/m.test(m)]`. Local lefthook already exits
0 for `*[bot]@*` authors, so CI is the only path that would ever need it. Not added now.

## Ecosystem coverage notes (verify on first run)

Ranked by likelihood of breaking *in this repo*:

- **pnpm catalogs (`npm` / `/ts`) — highest risk.** Every TS `package.json` references
  `"<dep>": "catalog:"`; the real version pins live in `ts/pnpm-workspace.yaml`'s `catalog:`
  block, so the catalog is the **only** thing this entry can usefully bump. Dependabot's
  catalog support is GA (2025-02) but carries open defects — reported to stop updating
  `pnpm-workspace.yaml`'s catalog (dependabot-core #11953) and to emit a broken
  `pnpm-lock.yaml` for pnpm+catalog (#14339). If catalog support is regressed when this lands,
  the `/ts` entry silently bumps nothing (or ships an unresolvable lockfile). Separately it
  classifies all catalog deps as `production` (can't infer dev/prod) — *that* part is harmless
  to us since both prefixes are `build(deps)`. **Prove this ecosystem out first.**
- **uv (`/py`) — medium risk.** `package-ecosystem: uv` is natively supported but has sync
  rough edges: it won't bump a dependency in `uv.lock` if `pyproject.toml` carries no version
  constraint for it, and there are case-sensitivity bugs between `pyproject.toml` and
  `uv.lock`. Smaller blast radius than catalogs (a handful of dev-group tools), but the first
  uv PR landing cleanly is a real check, not a formality.
- **github-actions (`/`) — low risk.** Handles SHA-pinned `uses:` plus the `# v4`/`# v0`
  version comment (current pins: `actions/checkout`, `actions/cache`,
  `moonrepo/setup-toolchain`).
- **cargo (`/rs`) — low risk.** Standard Cargo workspace; no special caveats.

## Alternatives considered

- **Uniform `chore(deps)` / `build(deps)` for all four ecosystems.** Workable (all in the
  enum) but less semantically precise; maintainer chose the `build` (package managers) /
  `ci` (actions) split.
- **`include: "scope"` instead of a hardcoded prefix.** Rejected: emits `deps-dev` for dev
  deps, which is not in `scope-enum` → commitlint failure.
- **Group majors too (`update-types: [major, minor, patch]`).** Rejected: hides breaking
  changes inside a grouped PR that may pass CI.
- **Add an `ignores` predicate / actor skip / relax `body-max-line-length`.** Rejected as
  YAGNI: commitlint's built-in URL carve-out already lets real Dependabot bodies pass
  (verified, §2). A trailer-keyed predicate is the documented reactive fallback if a non-URL
  long line ever trips the rule — not added pre-emptively.

## Out of scope

- **CLA bot** — split to the follow-up issue (see Scope decision).
- **proto-managed CLIs** (`buf`, `lefthook`, `moon`, `release-plz` in `.prototools`) — no
  Dependabot ecosystem covers proto; they stay manually pinned.
- **Dependabot security updates** — a repo-Settings toggle (GitHub UI), separate from this
  version-update config. Can later add `applies-to: security-updates` groups; not part of
  this issue.
- **CONTRIBUTING.md CLA wording** — stays as-is ("currently being set up"); flipped by the
  CLA follow-up issue.

## Acceptance criteria

- [ ] `.github/dependabot.yml` exists with grouped weekly updates for `cargo` (`/rs`),
      `npm` (`/ts`), `uv` (`/py`), and `github-actions` (`/`), each minor+patch grouped.
- [ ] Subjects are Conventional-Commit valid: `build(deps): …` for cargo/npm/uv,
      `ci(deps): …` for github-actions (verified against commitlint).
- [ ] No commitlint change is needed: real Dependabot bodies pass `body-max-line-length` via
      commitlint's built-in URL carve-out (verified). A trailer-keyed `ignores` predicate in
      `ts/commitlint.config.cjs` is the documented reactive fallback if a non-URL long line
      ever trips it — not added now.
- [ ] Minor+patch updates produce **at most one PR per ecosystem per week**. This is a
      deliberate reading of the AC: majors are intentionally left **un-grouped** (separate
      individual PRs) so a breaking bump isn't hidden in a green group — a documented
      deviation from a strictly-literal "one PR per ecosystem," flagged in case the wording
      matters for the SMA-363 foundation gate this issue blocks.
- [ ] First Dependabot PR runs cleanly through CI (`moon ci` + commitlint green).

## Verification plan

1. **Syntax** — push the branch; GitHub parses `dependabot.yml` (Insights → Dependency graph
   → Dependabot shows the four ecosystems with no config error).
2. **Trigger a run** — Insights → Dependency graph → Dependabot → **Check for updates** per
   ecosystem (or `gh api`), rather than waiting for Monday.
3. **Inspect the first PRs**:
   - One grouped PR per ecosystem (minor+patch).
   - Subject is `build(deps): …` / `ci(deps): …`.
   - commitlint step passes (the body's URL lines are exempt via the built-in carve-out).
   - `moon ci` passes.
4. **Watch the caveats, catalogs first** — confirm the **npm** PR actually bumps a `catalog:`
   entry in `ts/pnpm-workspace.yaml` *and* produces a resolvable `pnpm-lock.yaml` (the
   highest-risk path); confirm the **uv** PR actually updates `uv.lock` (constraint/case
   issues); confirm the **github-actions** PR bumps both the SHA and the `# v4` comment.
   **Catalog fallback:** if Dependabot's catalog support is still regressed when this lands,
   accept manual catalog bumps for now (or lift a few hot deps out of the catalog), note it on
   the PR, and don't block the other three ecosystems on the one shaky path.
5. **commitlint locally (sanity)** — from `ts/`, `pnpm --dir ts exec commitlint --config
   commitlint.config.cjs --edit <file>`: a `build(deps): …` subject passes; a realistic grouped
   Dependabot body (URL lines) passes `body-max-line-length`, while a long body line *without*
   a URL fails — confirming the URL carve-out is what carries Dependabot bodies.

## Files touched

- `.github/dependabot.yml` — **new, and the only file changed.** The four-ecosystem config
  above.
- _(No commitlint change — `ts/commitlint.config.cjs` and `@paigasus/commitlint-config` are
  left untouched; see §2.)_

## Linear split (done)

- Created **SMA-408** "CLA bot setup (cla-assistant)" (Low, `area:deps`, MVP milestone,
  related to SMA-362) carrying the four CLA ACs + ADR-0007 ref.
- Retitled SMA-362 → "Dependabot setup" and trimmed its description to the Dependabot ACs,
  pointing at SMA-408. The `sma-362` key remains in the branch name, so PR auto-linking is
  unaffected.
