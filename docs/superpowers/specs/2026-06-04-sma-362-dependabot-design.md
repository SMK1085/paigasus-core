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

**This issue delivers Dependabot only.** The CLA acceptance criteria are split into a
**new follow-up Linear issue** ("CLA bot setup (cla-assistant)", Low, `area:deps`,
do-before-first-external-PR), and SMA-362's description is trimmed to the Dependabot ACs.

> **Pending action (not yet done):** creating the follow-up CLA issue and trimming SMA-362
> were blocked by the permission classifier during brainstorm (a plain "go ahead" wasn't
> read as authorizing a new-ticket write). These Linear edits are an implementation-phase
> task and need an explicit go-ahead (or the maintainer doing them). Until then, the CLA
> ACs remain on SMA-362 and the references below to the follow-up issue are forward-looking.

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
passes. The risk is the **body**: Dependabot's auto-generated commit bodies (especially
grouped ones, with markdown links / long URLs) routinely exceed 100 chars/line and would
trip `body-max-line-length`.

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

### 2. commitlint exemption for Dependabot bodies

Add an `ignores` predicate to the shared config so machine-generated Dependabot commits skip
the rules (the subject is already kept Conventional by the `dependabot.yml` prefix; the body
is what would otherwise fail `body-max-line-length`). Humans are unaffected — the predicate
only matches Dependabot's `Signed-off-by` trailer.

```js
// ts/packages/commitlint-config/index.cjs
module.exports = {
  extends: ['@commitlint/config-conventional'],
  // Dependabot commits are machine-generated; their bodies carry long markdown/URL lines
  // that legitimately exceed body-max-line-length. dependabot.yml keeps the *subject*
  // Conventional (build(deps)/ci(deps)); skip the whole commit by its sign-off trailer.
  ignores: [(message) => /^Signed-off-by: dependabot\[bot\]/m.test(message)],
  rules: {
    /* …unchanged: type-enum, scope-enum, scope-empty, subject-empty,
       header-max-length, body-max-line-length, footer-leading-blank… */
  },
};
```

**Fallback if the trailer is absent.** If the first real Dependabot PR turns out not to
carry `Signed-off-by: dependabot[bot]`, fall back to an actor guard in `ci.yml` on the
commitlint step (`&& github.actor != 'dependabot[bot]'`). Decided in brainstorm to prefer
the config-level predicate (centralized, works for CI and local lefthook); the CI guard is
the documented fallback, not both.

## Ecosystem coverage notes (verify on first run)

- **uv** — `package-ecosystem: uv` is natively supported, but has known sync rough edges:
  it won't bump a dependency in `uv.lock` if `pyproject.toml` carries no version constraint
  for it, and there are case-sensitivity bugs between `pyproject.toml` and `uv.lock`. The
  first uv PR landing cleanly is a real verification step, not a formality.
- **pnpm catalogs** — GA in Dependabot since 2025-02; it reads `catalog:` entries in
  `ts/pnpm-workspace.yaml`. It classifies all catalog deps as `production` (can't infer
  dev/prod) — harmless here since both prefixes are `build(deps)`.
- **github-actions** — handles SHA-pinned `uses:` plus the `# v4` version comment (current
  pins: `actions/checkout`, `actions/cache`, `moonrepo/setup-toolchain`).

## Alternatives considered

- **Uniform `chore(deps)` / `build(deps)` for all four ecosystems.** Workable (all in the
  enum) but less semantically precise; maintainer chose the `build` (package managers) /
  `ci` (actions) split.
- **`include: "scope"` instead of a hardcoded prefix.** Rejected: emits `deps-dev` for dev
  deps, which is not in `scope-enum` → commitlint failure.
- **Group majors too (`update-types: [major, minor, patch]`).** Rejected: hides breaking
  changes inside a grouped PR that may pass CI.
- **Relax `body-max-line-length` globally / CI-only actor skip.** Rejected in favor of the
  config-level `ignores` predicate (keeps the human gate intact; single source of truth).

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
- [ ] commitlint exempts Dependabot commits (the `ignores` predicate) so
      `body-max-line-length` doesn't fail bot PRs; human commits unaffected.
- [ ] Minor+patch updates produce **at most one PR per ecosystem per week** (majors may
      arrive as separate individual PRs, by design).
- [ ] First Dependabot PR runs cleanly through CI (`moon ci` + commitlint green).

## Verification plan

1. **Syntax** — push the branch; GitHub parses `dependabot.yml` (Insights → Dependency graph
   → Dependabot shows the four ecosystems with no config error).
2. **Trigger a run** — Insights → Dependency graph → Dependabot → **Check for updates** per
   ecosystem (or `gh api`), rather than waiting for Monday.
3. **Inspect the first PRs**:
   - One grouped PR per ecosystem (minor+patch).
   - Subject is `build(deps): …` / `ci(deps): …`.
   - commitlint step passes (the bot PR is skipped by the `ignores` predicate); confirm a
     **human** commit on the same branch still gets linted.
   - `moon ci` passes.
4. **Watch the caveats** — confirm the **uv** PR actually updates `uv.lock` (constraint/case
   issues), and that the **github-actions** PR bumps both the SHA and the `# v4` comment.
5. **commitlint locally** — `echo "build(deps): bump serde from 1 to 2" | pnpm --dir ts exec
   commitlint` passes; a body line >100 chars *without* the dependabot trailer still fails
   (proves the exemption is scoped to the bot).

## Files touched

- `.github/dependabot.yml` — **new.** The four-ecosystem config above.
- `ts/packages/commitlint-config/index.cjs` — add the `ignores` predicate (one line + a
  comment); rules unchanged.

## Pending Linear edits (implementation phase, needs go-ahead)

- Create follow-up issue "CLA bot setup (cla-assistant)" (Low, `area:deps`, MVP milestone,
  related to SMA-362) carrying the four CLA ACs + ADR-0007 ref.
- Trim SMA-362's description to the Dependabot ACs, pointing at the follow-up.
