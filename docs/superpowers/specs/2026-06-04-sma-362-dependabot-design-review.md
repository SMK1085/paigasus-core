# Review — SMA-362 Dependabot setup

**Reviews:** [`2026-06-04-sma-362-dependabot-design.md`](./2026-06-04-sma-362-dependabot-design.md)
**Reviewer perspective:** staff engineer
**Date:** 2026-06-04
**Sources cross-referenced:** Linear SMA-362, the live `ts/packages/commitlint-config/index.cjs` + `ts/commitlint.config.cjs` + `.github/workflows/ci.yml`, and Dependabot's documented ecosystem support (uv, pnpm catalogs) and commit-trailer behavior.

## Verdict

Well-scoped and largely correct — ship it with two adjustments. The hard part of a Dependabot setup in this repo is *getting bot PRs through the commitlint gate*, and the spec nails the subject side: I confirmed `build(deps)`/`ci(deps)` are valid (the `deps` scope and `build`/`ci` types are in the live enums), and the deliberate avoidance of Dependabot's `include: "scope"` (which would emit the non-enumerated `deps-dev`) is a sharp catch. The body-length exemption via the `Signed-off-by: dependabot[bot]` trailer is the right mechanism and the trailer is real.

Two things to change before merge: the exemption is placed in the **published, cross-repo** commitlint package when it belongs in the repo's consumer config; and the spec flags **uv** as the risky ecosystem while under-weighting the one that's actually most likely to break here — **pnpm catalogs**, which is where essentially all the repo's TS versions live and which has documented 2026 regressions.

## What the spec gets right (calibration)

- **Conventional-Commit-safe prefixes, verified.** The live `index.cjs` has `type-enum` including `build`/`ci` and `scope-enum` including `deps`, so `build(deps): …` (cargo/npm/uv) and `ci(deps): …` (actions) pass. Hardcoding `prefix` + `prefix-development` to the same `build(deps)` to dodge `deps-dev` (not in the enum) is correct and non-obvious.
- **The body-length problem is the real gate risk, correctly identified.** `body-max-line-length: [2,'always',100]` is live, and Dependabot's grouped bodies (markdown/URLs) routinely exceed it. The `ignores` predicate keyed on the `Signed-off-by: dependabot[bot]` trailer is the right fix — and I confirmed Dependabot does emit that trailer. commitlint runs `--from/--to` over the PR range (ci.yml line 125), so the per-commit predicate correctly skips bot commits while still linting human ones.
- **Majors un-grouped is well-reasoned** — grouping `[minor, patch]` only, leaving majors as individual PRs, deliberately avoids hiding a breaking bump inside a green-looking group. Defensible reading of the AC.
- **SHA-pinned actions are real and covered.** ci.yml pins `actions/checkout`/`actions/cache`/`moonrepo/setup-toolchain` by SHA with `# v4`/`# v0` comments (the SMA-361 review's hardening landed), and the `github-actions` Dependabot entry maintains both the SHA and the comment.
- **Scope discipline + honesty** — CLA split to a follow-up; the spec is transparent that the Linear ticket edits were blocked by the permission classifier and remain pending.

## Findings

### F1 — [Medium] pnpm catalogs, not uv, is the ecosystem most likely to fail here — and the spec calls it "harmless"

The repo's TS dependencies are **catalog-centric**: every per-package `package.json` references `"<dep>": "catalog:"`, and the real version pins live in `ts/pnpm-workspace.yaml`'s `catalog:` block (react, next, typescript, eslint, vitest, …). So the only thing the `npm` (`/ts`) Dependabot entry can usefully bump *is the catalog* — the per-package refs have nothing to bump on their own.

Dependabot's pnpm-catalog support is GA (Feb 2025), but it has **documented 2026 regressions**: reports that catalog updates stopped working (last successful catalog run ~2025-03-28), that it "does not update `pnpm-workspace.yaml`'s catalog" (dependabot-core #11953), and that pnpm+catalog can produce an **incorrect `pnpm-lock.yaml`** (#14339). The spec, however, flags **uv** as "a real verification step, not a formality" and describes pnpm catalogs as "harmless here." That prioritization is backwards for this repo: a uv hiccup affects a handful of dev-group tools, but a catalog failure means the entire TS ecosystem's Dependabot entry silently updates nothing (or worse, ships a broken lockfile).

**Recommendation:** treat the `npm`/`/ts` entry as the **highest**-risk ecosystem. The verification plan (#4) watches uv and github-actions but not catalogs — add an explicit check that the first npm PR actually bumps a `catalog:` entry in `pnpm-workspace.yaml` *and* produces a resolvable `pnpm-lock.yaml`, and record a fallback (e.g. accept manual catalog bumps, or move a few hot deps out of the catalog) if Dependabot's catalog support is still regressed when this lands.

### F2 — [Medium] The Dependabot `ignores` predicate belongs in the consumer config, not the published package

The spec adds the `ignores` predicate to `ts/packages/commitlint-config/index.cjs` — which is `@paigasus/commitlint-config`, the **canonical, publishable, cross-repo** ruleset (its own header: "Source of truth for the type + scope allowlists"; ADR-0010's rationale is that other Paigasus repos consume it). A Dependabot bot-exemption is a **repo-operational** concern, not part of the canonical commit grammar other repos should inherit.

The repo already has the right home: `ts/commitlint.config.cjs` (`extends: ['@paigasus/commitlint-config']`) is the consumer override, and it's what both CI (`moon run ts:commitlint`) and local lefthook resolve. Putting `ignores` there covers both contexts identically **and** keeps the shared package a clean ruleset. **Recommendation:** move the `ignores` predicate to `ts/commitlint.config.cjs`; leave `@paigasus/commitlint-config/index.cjs` as rules-only. (As a bonus, this keeps the published package free of a JS function in what is otherwise a declarative ruleset.)

### F3 — [Low] The actor-guard fallback is coarser than the predicate and breaks "human commits on a bot branch still linted"

The fallback (`&& github.actor != 'dependabot[bot]'` on the commitlint step) skips the **whole** step for any Dependabot-actored PR — so a human commit pushed to a Dependabot PR (e.g. to fix a failing test or adjust a bumped API) would escape linting entirely. That contradicts the spec's own verification #3 ("confirm a human commit on the same branch still gets linted"), which only the config-level predicate preserves. The two are not interchangeable. Keep the predicate as primary (as the spec intends), but note that the fallback sacrifices per-commit granularity, so it's a genuine downgrade, not an equivalent — worth a sentence so a future maintainer doesn't reach for it casually.

### F4 — [Low] "At most one PR per ecosystem per week" is read liberally

The AC says "at most one PR per ecosystem per week"; grouping only `[minor, patch]` means N major bumps in a week produce N additional PRs, so the literal cap isn't guaranteed. The spec acknowledges and defends this (don't bury breaking changes in a green group), and the trade is the right one — but it is a liberal reading of the AC, not strict compliance. Fine to proceed; just flagging it's a documented deviation rather than a satisfied criterion, in case the AC wording matters for the SMA-363 foundation gate that SMA-362 blocks.

## Bottom line

Land it — the commitlint-safe prefixes, the trailer-based body exemption, and the majors-separate grouping are all correct and verified, and it closes the SHA-pin loop from the CI review. Two changes first: move the `ignores` predicate from the published `@paigasus/commitlint-config` to the repo's `ts/commitlint.config.cjs` (F2), and re-rank the ecosystem risk — the pnpm **catalog** path is the one to prove out on first run, since that's where the TS versions actually live and Dependabot's catalog support is currently shaky (F1). Note the fallback's coarser blast radius (F3) and that the weekly-PR cap is a liberal reading (F4).

## Sources

- Spec under review: `docs/superpowers/specs/2026-06-04-sma-362-dependabot-design.md`
- [Linear SMA-362 — Dependabot + CLA bot setup](https://linear.app/smaschek/issue/SMA-362/dependabot-cla-bot-setup) (blocks SMA-363; CLA split to a follow-up)
- Repo: `ts/packages/commitlint-config/index.cjs` (live rules — `build`/`ci` types, `deps` scope, `body-max-line-length: 100`), `ts/commitlint.config.cjs` (the consumer override — the better home for the bot `ignores`), `.github/workflows/ci.yml` (SHA-pinned actions; `moon run ts:commitlint --from/--to`)
- Dependabot ecosystem support: [pnpm catalogs GA (2025-02)](https://github.blog/changelog/2025-02-04-dependabot-now-supports-pnpm-workspace-catalogs-ga/) and the 2026 regressions ([dependabot-core #11953](https://github.com/dependabot/dependabot-core/issues/11953), incorrect-lockfile #14339); uv is a supported ecosystem; Dependabot emits a `Signed-off-by: dependabot[bot]` commit trailer ([dependabot-core #3480](https://github.com/dependabot/dependabot-core/issues/3480))
