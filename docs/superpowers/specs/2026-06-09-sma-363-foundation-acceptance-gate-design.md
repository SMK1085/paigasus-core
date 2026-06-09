# SMA-363: Foundation Acceptance Gate — Design

**Issue:** [SMA-363](https://linear.app/smaschek/issue/SMA-363/foundation-acceptance-gate)
**Date:** 2026-06-09
**Status:** Approved

## Goal

Verify all Phase 1 deliverables (SMA-355 → SMA-362) work together on current `main`,
producing the "ready to start migrating code" checkpoint that gates Phase 2. This is a
verification exercise, not a feature: the only repo change is a `docs/dev-setup.md`
capturing the validated end-to-end setup path.

## Scope decisions

- **Target:** current `main`, using the issue's AC list as-is. Post-issue additions
  (lefthook, dormant release configs, cargo-deny/machete, layer-routed CI) are exercised
  implicitly via the builds and CI runs but get no dedicated checklist items.
- **Fresh-clone environment:** temp-dir clone on the local Mac, following
  CONTRIBUTING.md verbatim. Limitation acknowledged in evidence: proto's tool cache is
  warm locally, so download timings are understated; GitHub Actions (Linux, fresh
  checkout each run) covers the genuinely cold path.
- **Execution:** sequential manual run-through (no verification script, no parallel
  agents). Evidence quality and traceability beat wall-clock speed for a one-time gate.
- **Known naming drift:** the AC says `ts/packages/sdk`; the actual package is
  `ts/packages/paigasus-sdk`. Verified against the real path, noted in evidence.

## Verification matrix

### Stage 1 — Fresh-clone environment

Covers: fresh-clone build, toolchain installation, cross-language tasks, documentation
accuracy, and the ≤ 15-minute definition-of-done.

1. `git clone` into a temp dir; follow CONTRIBUTING.md local-development steps
   *verbatim* — deviations are the doc bugs being hunted. Wall-clock timed from clone
   to green build.
2. Toolchains: `.prototools` pins resolve via `proto install`; Moon bootstraps its
   toolchains (Rust, `unstable_python` + `unstable_uv`, Node/pnpm) with no manual
   installs beyond documented OS prerequisites.
3. `moon ci :build` and `moon ci :test` in the fresh clone — tasks must resolve and run
   across all four workspaces (contracts, rs, py, ts). Explicit targets always (Moon
   2.x non-TTY requirement).
4. Every README/CONTRIBUTING claim encountered along the way is diffed against reality.

### Stage 2 — Affected-graph experiments

In the temp clone, one scratch branch per case: touch a file, then assert the affected
set via `moon query affected` (cross-checked with `moon ci :build` task selection).
Expectation semantics: **touched project plus its declared dependents, nothing
cross-stack except via declared dependencies.**

| Touch | Expected affected set |
| --- | --- |
| `rs/crates/libs/paigasus-kernel/` | kernel crate + declared dependents (e.g. py-bindings); no py/ts/contracts projects |
| `py/packages/paigasus-ml/` | only that package's tasks |
| `ts/packages/paigasus-sdk/` | only that package's tasks |
| `contracts/proto/` | `contracts:generate`, then downstream proto/binding projects in all three languages |

### Stage 3 — CODEOWNERS sync

Hand-edit `.github/CODEOWNERS`, run a Moon command, confirm sync regenerates the file
and overwrites the manual edit — proving the file is fully Moon-owned (the invariant
behind the AC).

### Stage 4 — GitHub-side checks

- **CI parity:** run `moon ci :build` locally at the same SHA as the latest green
  `main` workflow run; compare resolved task set and results.
- **Dependabot:** cite the already-merged grouped batches (PRs #23–#26, #30) as
  evidence the first weekly batch produced grouped, CI-green PRs. No test trigger.
- **Branch protection:** read the `main` ruleset via `gh api`; confirm PRs are blocked
  without green required checks; cite an existing PR's merge gating as empirical
  evidence.

## Deliverables

1. **`docs/dev-setup.md`** — written from the actual fresh-clone run, not from existing
   docs: prerequisites, exact command sequence that worked, observed timings, and
   contributor gotchas (non-TTY `moon ci` needs explicit targets, nextest
   `--no-tests=pass`, proto shim PATH). Points to README/CONTRIBUTING where they are
   already correct rather than duplicating them.
2. **Linear evidence comment** on SMA-363 — one line per AC: pass/fail, command run,
   pointer (output snippet, CI run URL, PR number). AC checkboxes ticked in the issue
   description.
3. **One PR** on `feature/sma-363-foundation-acceptance-gate` containing only the doc
   (plus this spec and its plan).

## Failure handling

- Any failed AC is fixed in the **originating issue** — separate branch + PR
  referencing that issue — then re-verified here. The SMA-363 PR never carries fixes.
- Exception: pure documentation drift (README/CONTRIBUTING inaccuracies) found during
  the run has no live originating issue; fixed via small standalone docs PRs.

## Exit criterion

All nine ACs evidenced as passing, `dev-setup.md` merged, Linear checkboxes ticked →
SMA-363 Done, Phase 2 unblocked.
