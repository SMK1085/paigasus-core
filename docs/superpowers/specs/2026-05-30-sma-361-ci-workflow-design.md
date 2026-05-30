# SMA-361 — GitHub Actions CI workflow with `moon ci` + affected-graph

**Status:** Designed (brainstorming complete)
**Date:** 2026-05-30
**Linear:** [SMA-361](https://linear.app/smaschek/issue/SMA-361/github-actions-ci-workflow-with-moon-ci-affected-graph)
**Branch:** `feature/sma-361-github-actions-ci-workflow-with-moon-ci-affected-graph`
**Targets:** `main` (currently `25bc0b7`).
**References:** ADR-0008 (Moon as the polyglot task orchestrator); SMA-356 (Moon config — pinned Moon 2.2.5, `codeowners.sync`, the "bare `moon ci` errors in non-TTY" finding); SMA-384 (harmonized py task name `format` → `fmt` so `moon ci :fmt` spans stacks — this spec applies the same to `contracts`); SMA-360 (contracts buf scaffold — origin of the `format`/`breaking` tasks); Moon CI guide (`moonrepo/setup-toolchain@v0` backbone).

## Goal

Stand up the repo's first CI: a single `.github/workflows/ci.yml` that runs `moon ci` on every PR (and on push to `main`), gating the **full affected task graph** across all four workspaces. The load-bearing property is Moon's affected-graph — only projects touched by a PR rebuild, so CI stays fast as the monorepo grows.

Concretely, after this lands:

1. `.github/workflows/ci.yml` exists and runs the affected build/test/lint/fmt/typecheck/breaking graph via `moon ci … --base origin/main`.
2. `contracts`' formatter task is renamed `format` → `fmt` so a single `:fmt` target covers every workspace (no per-stack target alternation).
3. CODEOWNERS staleness is caught in CI (drift gate), since `.github/CODEOWNERS` is Moon-generated.
4. Branch protection on `main` is documented as a manual close-out step (it lives in GitHub settings, not in a workflow file).

## Decision

**One workflow file, one job, Moon owns the graph.** The workflow is a thin bootstrap (`checkout` → `setup-toolchain` → caches → `moon ci`); Moon decides what's affected across Rust/Python/TypeScript/proto and parallelizes internally. No per-language jobs — splitting would fragment the cross-language affected-graph and duplicate caching for no MVP benefit.

Deliverables on this branch:

- `feat(ci):` — `.github/workflows/ci.yml`.
- `refactor(contracts):` — rename the `format` task to `fmt` in `contracts/moon.yml` (command unchanged: `buf format --exit-code`).

Verification (no-op pass, affected-graph end-to-end) is performed on **this branch's own PR**. Branch protection is configured afterward in the GitHub UI and recorded in the issue close-out comment.

## Design

### 1. Triggers, concurrency, job shell

```yaml
name: CI
on:
  pull_request:
    branches: [main]
  push:
    branches: [main]      # warms the cache + gives main a CI status

concurrency:
  group: ci-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  ci:
    name: moon ci
    runs-on: ubuntu-latest
```

- **PR trigger** is the AC requirement. **Push-to-`main`** is added so post-merge runs warm the shared cache and `main` carries a green/red status.
- **`concurrency`** cancels superseded runs on the same ref (rapid re-pushes don't pile up).

### 2. Job steps

| # | Step | Purpose |
|---|------|---------|
| 1 | `actions/checkout@v4` with `fetch-depth: 0` | Moon's affected-graph needs full git history. |
| 2 | Materialize the `main` ref (see §4) | So `--base origin/main` **and** the buf `:breaking` check resolve on a PR checkout. |
| 3 | `moonrepo/setup-toolchain@v0` | Installs proto + the pinned Moon **2.2.5** from `.prototools`; caches the proto store (Moon + language toolchains). |
| 4 | `actions/cache` × {Rust, pnpm, uv, Moon} (see §5) | Warm build-artifact caches. |
| 5 | `moon ci :build :test :lint :fmt :typecheck :breaking --base origin/main` | The gate. |
| 6 | CODEOWNERS drift gate (see §6) | Fail if generated CODEOWNERS is stale. |

`moon ci` **auto-installs the language toolchains** (Rust 1.95, Node 22 + pnpm 11, Python 3.12 + uv) from `.moon/toolchain.yml` — no `setup-rust`/`setup-node`/`setup-python` steps are needed. Moon also runs affected tasks across its own thread pool, so one job parallelizes internally.

### 3. The gated task set

Targets are **explicit** because bare `moon ci` errors with `app::tty::required_id` in non-TTY on Moon 2.2.5 (SMA-356 finding, codified in CLAUDE.md). The list is the union of gated task names across all workspaces:

```
:build :test :lint :fmt :typecheck :breaking
```

- `:build :test :lint :fmt` — Rust + Python + TypeScript (and `contracts:fmt` after the rename, §7).
- `:typecheck` — Python + TypeScript only (Rust/contracts have no such task; Moon simply skips projects without it).
- `:breaking` — `contracts` only (buf proto breaking-change check).
- **Excluded:** `:generate` (proto codegen drift) and a dependency audit — deferred to a post-MVP `codegen-drift.yml` nightly, per the issue's own note. `repo:install-hooks` is already `runInCI: false` and never matches anyway.

### 4. Affected base + the `main`-ref problem (the one real risk)

Two checks need a `main` reference that a `pull_request` checkout does **not** guarantee, even with `fetch-depth: 0` (checkout fetches the PR merge ref's history, not necessarily a usable `origin/main` / local `main`):

- **Moon affected:** `--base origin/main` needs the `origin/main` remote-tracking ref.
- **buf breaking:** `contracts:breaking` runs `buf breaking --against '../.git#branch=main,subdir=contracts'`, which needs a **local `main` branch**.

**Resolution (verify empirically during implementation):** on PR events, fetch `main` into both ref namespaces before running Moon:

```yaml
- name: Materialize main ref
  if: github.event_name == 'pull_request'
  run: |
    git fetch --no-tags origin \
      +refs/heads/main:refs/remotes/origin/main \
      +refs/heads/main:refs/heads/main
```

This populates `origin/main` (for `--base`) and a local `main` (for buf). It is guarded to PR events because on a push-to-`main` build the runner is already on `main` and writing `refs/heads/main` would be refused; there, Moon's CI auto-detection compares against the previous commit, and `:breaking` compares `main` against itself (a no-op), both fine.

Belt-and-suspenders: Moon auto-detects base/head from the GitHub Actions environment, so even if `--base origin/main` were dropped the affected detection would still work — but the AC names `--base origin/main` explicitly, so we keep it.

**Open item for implementation:** confirm the exact buf incantation works in CI. Fallbacks, in order of preference, if `#branch=main` misbehaves: (a) keep the local-`main` fetch above; (b) switch the `breaking` task's `--against` to `#ref=origin/main`; (c) drop `:breaking` from the PR gate and move it to the nightly. Pick the first that verifies green.

### 5. Caching (hybrid)

`setup-toolchain` caches the proto store (Moon binary + toolchains). Layered on top, explicit `actions/cache` entries cover the heavy build artifacts the AC calls out:

| Cache | Paths | Key (with `restore-keys` prefix) |
|-------|-------|----------------------------------|
| Rust | `~/.cargo/registry`, `~/.cargo/git`, `rs/target` | `rust-${{ runner.os }}-${{ hashFiles('rs/Cargo.lock') }}` |
| pnpm store | output of `pnpm store path` (default `~/.local/share/pnpm/store`) | `pnpm-${{ runner.os }}-${{ hashFiles('ts/pnpm-lock.yaml') }}` |
| uv cache | uv cache dir (`~/.cache/uv` on Linux) | `uv-${{ runner.os }}-${{ hashFiles('py/uv.lock') }}` |
| Moon | `.moon/cache` (**repo-local**, not `~/.moon/cache`) | `moon-${{ runner.os }}-${{ github.sha }}` |

Notes:
- The AC's `~/.moon/cache` path is a Moon 1.x assumption; 2.2.5 keeps the workspace task cache **repo-local at `.moon/cache`** (it's gitignored). Cache that path.
- Exact pnpm/uv cache directories are resolved at runtime (`pnpm store path`, `uv cache dir`) rather than hard-coded, to stay correct across runner image changes.
- All keys carry `restore-keys` prefixes for partial-hit warm starts when a lockfile changes.

### 6. CODEOWNERS drift gate

`.github/CODEOWNERS` is Moon-generated (`codeowners.sync: true` in `.moon/workspace.yml`; CLAUDE.md: "don't hand-edit"). Rather than letting sync run silently (the AC's literal wording), CI actively **catches drift**:

```yaml
- name: CODEOWNERS up-to-date
  run: |
    moon sync code-owners
    git diff --exit-code .github/CODEOWNERS
```

A stale committed CODEOWNERS fails the build with a visible diff. (AC field corrected: `codeowners.sync`, not the issue's `codeowners.syncOnRun`.)

### 7. `contracts` `format` → `fmt` rename

`contracts/moon.yml` currently names its formatter task `format`, while Rust/Python/TypeScript all use `fmt`. Renaming the task id to `fmt` (command stays `buf format --exit-code`) lets a single `:fmt` target cover every workspace, so the gate list stays uniform instead of carrying a contracts-only `:format`.

This mirrors SMA-384, which renamed py's `format` → `fmt` for exactly this cross-stack-consistency reason. Scope check: the only **live** reference to the `format` task id is `contracts/moon.yml` itself — `lefthook.yml` doesn't invoke it, and the other matches are point-in-time entries in `docs/superpowers/` archives (historical records, not edited). `contracts` sets `layer: 'tool'` with no `language`, so it inherits no language task file and there's no inheritance collision.

### 8. Error handling / edge cases

- **No-test crates:** `cargo nextest` exits non-zero on a workspace with no tests; already neutralized by `--no-tests=pass` in `.moon/tasks/rust.yml`. Python/TS test tasks use `--passWithNoTests` / `pytest`-with-guarded-`conftest.py` similarly.
- **Empty affected set:** a PR that touches nothing gated → `moon ci` no-ops green.
- **Superseded runs:** handled by `concurrency` (§1).

### 9. Out of scope (close-out)

- **Branch protection on `main`** requiring the `moon ci` check — configured in GitHub UI / via `gh api` after the first green run, then recorded in the SMA-361 close-out comment. The required status check name will be the job name (`moon ci`); confirm the exact check name from the first run before wiring protection.
- **`codegen-drift.yml` nightly** (proto codegen drift, dependency audit) — explicitly post-MVP, not this branch.

## Verification plan (on this branch's PR)

1. **No-op pass:** open the PR; confirm the `ci` job runs green. (The PR's own diff — the workflow file + the contracts rename — exercises the contracts/`ci`-config path; an essentially no-op follow-up commit confirms a clean pass.)
2. **Affected-graph end-to-end:** push a commit touching a single file in one workspace (e.g. `rs/crates/libs/paigasus-kernel/src/lib.rs`) and confirm the Moon run summary shows **only that workspace's** tasks executing, not the universe. Capture the run output for the close-out comment.
3. **CODEOWNERS gate:** (optional sanity) locally hand-edit `.github/CODEOWNERS`, confirm the drift step would fail.

## Acceptance-criteria mapping

| AC (issue) | How satisfied |
|------------|---------------|
| `ci.yml` with PR trigger, `moon ci … --base origin/main`, checkout `fetch-depth: 0` | §1–§3. (`moon ci` gets explicit targets per Moon 2.2.5.) |
| Cache strategy (cargo/target, Moon, pnpm, uv) | §5 (hybrid; `~/.moon/cache` corrected to repo-local `.moon/cache`). |
| Required checks: fmt, lint, test via `moon ci` | §3 — and extended to build/typecheck/breaking (full graph). |
| CODEOWNERS auto-sync in CI | §6 — drift gate (field corrected to `codeowners.sync`). |
| CI passes on a no-op PR | Verification plan #1. |
| Affected-graph verified end-to-end | Verification plan #2. |
| Branch protection on `main` (configure separately, document in close-out) | §9. |

## Risks / to-verify during implementation

1. **buf `:breaking` base ref in CI** — the §4 `main`-ref materialization; confirm the exact incantation green, with the §4 fallback ladder if not.
2. **`setup-toolchain@v0` + Moon 2.2.5** — confirm the action installs the proto-pinned Moon 2.2.5 (not a newer default) and that `moon ci` then auto-installs the language toolchains headlessly.
3. **Cache path resolution** — resolve pnpm/uv cache dirs at runtime rather than hard-coding.
4. **Required-check name** — capture the actual status-check name from the first run before configuring branch protection (§9).
