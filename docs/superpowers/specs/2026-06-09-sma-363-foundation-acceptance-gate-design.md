# SMA-363: Foundation Acceptance Gate — Design

**Issue:** [SMA-363](https://linear.app/smaschek/issue/SMA-363/foundation-acceptance-gate)
**Date:** 2026-06-09 (revised same day after design review)
**Status:** Approved

## Goal

Verify all Phase 1 deliverables (SMA-355 → SMA-362) work together on current `main`,
producing the "ready to start migrating code" checkpoint that gates Phase 2. This is a
verification exercise, not a feature: the only repo change is a `docs/dev-setup.md`
capturing the validated end-to-end setup path.

## Scope decisions

- **Target:** current `main`, using the issue's AC list as amended below. Post-issue
  additions (lefthook, dormant release configs, cargo-deny/machete, layer-routed CI)
  are exercised implicitly via the builds and CI runs but get no dedicated checklist
  items — except local lefthook hooks, which get one explicit check (see Stage 1),
  since git hooks are never exercised by `moon ci`.
- **Cross-language cascade is deferred.** The original AC expects a
  proto/kernel edit to cascade rebuilds across languages, but no such `dependsOn`
  edges exist on `main` (the only declared edge is
  `paigasus-gateway → [paigasus-proto-rs, paigasus-kernel-rs]`), and the empty stubs
  carry no real dependency content to exercise. This gate verifies **affected-graph
  resolution on the edges that exist today**; the cascade itself is re-verified at
  Phase 2 entry via SMA-409 (filed from this gate, blocking Phase 2), which wires the
  edges alongside real code and adds a regression smoke assertion. The Linear AC list
  is amended to match.
- **≤15-minute DoD is a recorded observation, not a pass/fail gate.**
  Neither the warm-cache local run nor CI measures a cold contributor setup. The
  local timing is recorded in `dev-setup.md` with the warm-cache caveat stated.
- **Fresh-clone environment:** temp-dir clone on the local Mac, following
  CONTRIBUTING.md verbatim, with one addition mirroring CI: **materialize
  `origin/main`** (full-history fetch, as in `ci.yml` "Materialize main ref") so
  `moon ci --base origin/main` and `contracts:breaking` (`buf breaking --against
  …branch=main`) actually execute rather than erroring or resolving to "nothing
  affected".
- **Execution:** sequential manual run-through (no verification script, no parallel
  agents). Evidence quality and traceability beat wall-clock speed for a one-time
  gate.
- **Known naming drift:** the AC says `ts/packages/sdk`; the actual package is
  `ts/packages/paigasus-sdk`. Verified against the real path, noted in evidence.

## Verification matrix

### Stage 1 — Fresh-clone environment

Covers: fresh-clone build, toolchain installation, cross-language tasks, documentation
accuracy, local git hooks, and the setup-time observation.

1. `git clone` into a temp dir; materialize `origin/main` with full history; follow
   CONTRIBUTING.md local-development steps *verbatim* — deviations are the doc bugs
   being hunted. Wall-clock timed from clone to green build, recorded as an
   observation (warm-cache caveat stated).
2. Toolchains: `.prototools` pins resolve via `proto install`; Moon bootstraps its
   toolchains (Rust, `unstable_python` + `unstable_uv`, Node/pnpm) with no manual
   installs beyond documented OS prerequisites.
3. `moon ci :build` and `moon ci :test` in the fresh clone — tasks must resolve and
   run across all four workspaces (contracts, rs, py, ts). Explicit targets always
   (Moon 2.x non-TTY requirement).
4. **"No warnings" defined objectively:** warning-free means the CI
   lint/format/typecheck surface passes — `cargo clippy -- -D warnings`,
   `cargo fmt --check`, ruff (lint + format), basedpyright, ESLint, Prettier check,
   `tsc` typecheck — i.e. the `:lint :fmt :typecheck` targets, plus `:deny`/`:machete`
   as shipped in CI.
5. **lefthook check:** after following CONTRIBUTING verbatim, attempt one
   commit with a non-Conventional subject and confirm the `commit-msg` hook rejects
   it — proving hooks actually installed and fire locally.
6. Every README/CONTRIBUTING claim encountered along the way is diffed against
   reality.

### Stage 2 — Affected-graph resolution (cascade deferred)

In the temp clone, one scratch branch per case: touch a file, then assert the affected
set via `moon query affected` (cross-checked with `moon ci :build` task selection).
Expectation semantics: **the touched project, its declared dependents, and
the root aggregator's whole-tree tasks — nothing cross-stack except via declared
dependencies.** The root `py`/`ts` projects own whole-tree lint/fmt (SMA-399/394/401),
so they are *expected* to appear for leaf edits in their stack.

| Touch | Expected affected set |
| --- | --- |
| `rs/crates/libs/paigasus-kernel/` | `paigasus-kernel-rs` + `paigasus-gateway-rs` (its one declared consumer); no py/ts/contracts projects |
| `py/packages/paigasus-ml/` | `paigasus-ml-py` + root `py` whole-tree tasks; nothing cross-stack |
| `ts/packages/paigasus-sdk/` | `paigasus-sdk-ts` + root `ts` whole-tree tasks; nothing cross-stack |
| `contracts/proto/` | `contracts:*` only (incl. `:breaking`, which must *execute*, not skip) — downstream cascade explicitly deferred |
| Dedup invariant (SMA-401) | For the py and ts edits above, the root whole-tree lint/fmt tasks fire **exactly once** — not zero, not twice |

The original "proto edit → downstream language workspaces rebuild" and "kernel →
py-bindings" expectations are **deferred** to the Phase-2-entry follow-up issue: the
edges don't exist on `main`, and py-bindings has no kernel dependency (the gateway is
the consumer).

### Stage 3 — CODEOWNERS sync

Hand-edit `.github/CODEOWNERS`, run **`moon sync code-owners`** explicitly, confirm it
regenerates the file and overwrites the manual edit — proving the file is fully
Moon-owned. Evidence wording: "explicit `moon sync code-owners`
regenerates, and the CI drift gate (`git diff --exit-code` after sync) fails on stale
CODEOWNERS" — not "auto-syncs on every Moon run", which overstates the mechanism.

### Stage 4 — GitHub-side checks

- **CI parity:** reproduce the **exact PR-path CI invocation** locally at
  the same SHA as the latest green `main` run:
  `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking
  :release-parity :release-parity-py :release-parity-ts --base origin/main`, plus the
  two out-of-`moon ci` gates (`moon run ts:commitlint`, `moon run
  ts:check-config-only`) and the CODEOWNERS sync + diff gate. Diff the resolved task
  set against the GitHub run's logs.
- **Dependabot:** cite the already-merged grouped batches (PRs #23–#26, #30) as
  evidence the first weekly batch produced grouped, CI-green PRs. No test trigger.
- **Branch protection:** read the `main` ruleset via `gh api` and assert
  the required status-check string equals **`CI / moon ci`** exactly (workflow
  `name: CI`, job `name: moon ci`), and that the ruleset blocks merge when the check
  is *absent*, not only when it is red. Cite an existing PR's merge gating as
  empirical evidence.

## Deliverables

1. **`docs/dev-setup.md`** — written from the actual fresh-clone run, not from
   existing docs: prerequisites, exact command sequence that worked, observed timings
   (with warm-cache caveat), and contributor gotchas (non-TTY `moon ci` needs explicit
   targets, nextest `--no-tests=pass`, proto shim PATH, materializing `origin/main`).
   Points to README/CONTRIBUTING where they are already correct rather than
   duplicating them.
2. **Linear evidence comment** on SMA-363 — one line per AC: pass/fail/deferred, the
   command run, and a pointer (output snippet, CI run URL, PR number). AC checkboxes
   ticked in the (amended) issue description.
3. **Follow-up issue SMA-409 (filed from this gate, blocking Phase 2 entry):** wires
   the cross-language `dependsOn` edges alongside real code, re-verifies the cascade
   (proto → all three languages; kernel → bindings), and adds a lightweight
   affected-graph smoke assertion to CI as the regression guard.
4. **One PR** on `feature/sma-363-foundation-acceptance-gate` containing only the doc
   (plus this spec and its plan).

## Failure handling

Three dispositions:

- **Originating-issue defect:** fixed in the originating issue — separate branch + PR
  referencing that issue — then re-verified here. The SMA-363 PR never carries fixes.
- **Documentation drift:** README/CONTRIBUTING inaccuracies found during the run have
  no live originating issue; fixed via small standalone docs PRs.
- **Structural gap** (no single originating issue — e.g. the missing cascade edges):
  spawns a **new issue** blocking Phase 2, tracked from this gate.

## Exit criterion

All ACs (as amended) evidenced as passing or explicitly deferred-with-issue,
`dev-setup.md` merged, Linear checkboxes ticked, and the Phase-2-entry follow-up issue
filed → SMA-363 Done, Phase 2 unblocked.
