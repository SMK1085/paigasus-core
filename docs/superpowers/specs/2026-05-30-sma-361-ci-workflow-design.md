# SMA-361 — GitHub Actions CI workflow with `moon ci` + affected-graph

**Status:** Designed (brainstorming complete; staff-eng review incorporated 2026-05-30)
**Date:** 2026-05-30
**Linear:** [SMA-361](https://linear.app/smaschek/issue/SMA-361/github-actions-ci-workflow-with-moon-ci-affected-graph)
**Branch:** `feature/sma-361-github-actions-ci-workflow-with-moon-ci-affected-graph`
**Targets:** `main` (currently `25bc0b7`).
**References:** ADR-0008 (Moon as the polyglot task orchestrator); ADR-0010 (lefthook + commitlint; CI-parity invariant); SMA-356 (Moon config — pinned Moon 2.2.5, `codeowners.sync`, the "bare `moon ci` errors in non-TTY" finding); SMA-371 (local git hooks — **AC-E delegates the CI commitlint-parity gate + release smoke job to this issue**); SMA-384 (harmonized py task name `format` → `fmt`); SMA-360 (contracts buf scaffold); SMA-394 (Moon owns the TS build/typecheck graph — `ts/moon.yml` `inheritedTasks.exclude`); Moon CI guide (`moonrepo/setup-toolchain@v0`).
**Spun-out follow-ups:** [SMA-398](https://linear.app/smaschek/issue/SMA-398) (release-tool dry-run smoke job — deferred, blocked on release tooling existing); [SMA-399](https://linear.app/smaschek/issue/SMA-399) (py-root `:build` emits a junk `UNKNOWN` wheel — py-twin of SMA-394).

## Goal

Stand up the repo's first CI: a single `.github/workflows/ci.yml` that, on every PR (and on push to `main`), gates the **full affected task graph** across all four workspaces **and** validates commit messages. The load-bearing property is Moon's affected-graph — only projects touched by a PR rebuild, so CI stays fast as the monorepo grows.

After this lands:

1. `.github/workflows/ci.yml` runs the affected build/test/lint/fmt/typecheck/breaking graph via `moon ci … --base origin/main`.
2. A **commitlint gate** validates the PR's commit messages with the same pinned `@paigasus/commitlint-config` as the local hook — satisfying SMA-371 AC-E's CI-parity invariant (the teeth behind "CI catches `--no-verify` bypasses and bot commits").
3. `contracts`' formatter task is renamed `format` → `fmt` so a single `:fmt` target covers every workspace.
4. CODEOWNERS staleness is caught in CI (drift gate), since `.github/CODEOWNERS` is Moon-generated.
5. Branch protection on `main` is documented as a manual close-out step, keyed on a **whole-graph-green** signal (not an affected subset).

## Decision

**One workflow file, one job, Moon owns the graph.** The workflow is a thin bootstrap (`checkout` → `setup-toolchain` → caches → commitlint gate → `moon ci`); Moon decides what's affected across Rust/Python/TypeScript/proto and parallelizes internally. No per-language jobs — splitting would fragment the cross-language affected-graph and duplicate caching.

The **commitlint gate is a step in the same job, placed before `moon ci`** (fail fast on a bad commit message before the expensive build), rather than a separate parallel job — it reuses the one checkout + toolchain setup, and keeps branch protection to a single required check. (Promotable to a parallel job later if fail-fast-without-the-toolchain ever matters.)

Deliverables on this branch:

- `feat(ci):` — `.github/workflows/ci.yml` (the graph gate + the commitlint gate).
- `refactor(contracts):` — rename the `format` task to `fmt` in `contracts/moon.yml` (command unchanged: `buf format --exit-code`).

Verification (commit-lint pass, no-op pass, affected-graph end-to-end, **whole-graph green**) is performed on this branch's own PR. Branch protection is configured afterward in the GitHub UI and recorded in the issue close-out comment.

## Design

### 1. Triggers, concurrency, permissions, job shell

```yaml
name: CI
on:
  pull_request:
    branches: [main]
  push:
    branches: [main]      # warms the cache + gives main a CI status

permissions:
  contents: read          # least privilege; the job only reads + runs checks

concurrency:
  # Cancel superseded PR runs; let push-to-main runs complete (they warm cache + carry status).
  group: ci-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

jobs:
  ci:
    name: moon ci
    runs-on: ubuntu-latest
    timeout-minutes: 30   # cap a stalled toolchain download / hung task
```

- **PR trigger** is the AC requirement; **push-to-`main`** warms the shared cache and gives `main` a status.
- **`permissions: contents: read`** — `paigasus-core` is a public repo; declare least privilege explicitly (the CODEOWNERS gate only does `git diff`, no writes). [F3]
- **`cancel-in-progress` is scoped to PRs** so a rapid second push to `main` doesn't cancel the run that's warming the cache / setting `main`'s status. [F4]
- **`timeout-minutes`** caps runaway jobs (GitHub's default is 6h). [F5]

### 2. Job steps

| # | Step | Purpose |
|---|------|---------|
| 1 | `actions/checkout@v4` (pinned to SHA) with `fetch-depth: 0` | Moon's affected-graph needs full git history. |
| 2 | Materialize the `main` ref (see §5) | So `--base origin/main` **and** the buf `:breaking` check resolve on a PR checkout. |
| 3 | `moonrepo/setup-toolchain@<sha>` | Installs proto + the pinned Moon **2.2.5** from `.prototools`; caches the proto store. |
| 4 | `actions/cache@<sha>` × {Rust, pnpm, uv, Moon} (see §6) | Warm build-artifact caches. |
| 5 | **commitlint gate** (see §4) | Validate the PR/push commit range — fail fast. |
| 6 | `moon ci :build :test :lint :fmt :typecheck :breaking --base origin/main` | The graph gate. |
| 7 | CODEOWNERS drift gate (see §7) | Fail if generated CODEOWNERS is stale. |

**Action pinning [F3]:** on a public repo, pin third-party actions to a commit SHA (with a version comment). This applies to `actions/checkout`, `actions/cache`, and `moonrepo/setup-toolchain` — the last is a floating `v0` (pre-1.0, mutable) and is the subject of risk #2 (it could install a newer Moon than the pinned 2.2.5).

`moon ci` **auto-installs the language toolchains** (Rust 1.95, Node 22 + pnpm 11, Python 3.12 + uv) from `.moon/toolchain.yml` — no `setup-rust`/`setup-node`/`setup-python` steps. Moon also runs affected tasks across its own thread pool, so one job parallelizes internally.

### 3. The gated task set

Targets are **explicit** because bare `moon ci` errors with `app::tty::required_id` in non-TTY on Moon 2.2.5 (SMA-356 finding, codified in CLAUDE.md). The list is the union of gated task names across all workspaces:

```
:build :test :lint :fmt :typecheck :breaking
```

- `:build :test :lint :fmt` — Rust + Python + TypeScript (and `contracts:fmt` after the rename, §8).
- `:typecheck` — Python + TypeScript only (Rust/contracts have no such task; Moon skips projects without it).
- `:breaking` — `contracts` only (buf proto breaking-change check).
- **Excluded:** `:generate` (proto codegen drift) and a dependency audit — deferred to a post-MVP `codegen-drift.yml` nightly. `repo:install-hooks` is already `runInCI: false`.

### 4. Commit-message CI-parity gate (SMA-371 AC-E)

SMA-371 (Done) explicitly delegates to SMA-361: *"CI runs the **same** commitlint binary against the **same** `@paigasus/commitlint-config` version as the local hook — both pinned via `pnpm-lock.yaml`."* The local `commit-msg` hook (lefthook) runs `ts/node_modules/.bin/commitlint --edit … --config ts/commitlint.config.cjs`; CI runs the same binary + config over a commit **range**.

Behavior:

- **Range.** On `pull_request`: `--from ${{ github.event.pull_request.base.sha }} --to ${{ github.event.pull_request.head.sha }}` (validates the PR's own commits; commitlint's default-ignores skip merge commits). On `push` to `main`: validate `${{ github.event.before }}..${{ github.sha }}`, guarding the initial all-zeros `before`.
- **Bots are validated in CI.** The local hook skips `*[bot]@*` authors by design; SMA-371 AC-D makes CI the authoritative gate for bot commits, so the CI gate does **not** skip them (Dependabot's `chore(deps): …` conforms to the allowlist).
- **Same config, lockfile-pinned.** Uses `ts/commitlint.config.cjs` → `@paigasus/commitlint-config`, resolved from `ts/pnpm-lock.yaml`. This realizes the parity invariant directly.

**Mechanism (to verify in implementation):** pnpm is Moon-managed (declared in `.moon/toolchain.yml`, not `.prototools`), so the cleanest invocation is **through Moon** — a non-cached `commitlint` task on a node-toolchain project (e.g. the `ts` root) invoked as `moon run <proj>:commitlint -- --from <base> --to <head>`, letting Moon provide pnpm + install `ts/` deps. Fallback if that's awkward: an explicit `pnpm -C ts install --frozen-lockfile` (pnpm via Moon's node toolchain on `PATH`) then `pnpm -C ts exec commitlint …`. Constraint either way: the binary + config must be the lockfile-pinned ones (parity), and the step must always run (not be gated by the affected-graph).

**Deferred:** SMA-371 AC-E's second half — a release-tool (`release-plz`/`semantic-release`) dry-run classification smoke job — is **not** implementable yet (no release tooling exists in the repo, verified). Tracked as **SMA-398**, blocked on that tooling landing.

### 5. Affected base + the `main`-ref problem (the one real risk)

Two checks need a `main` reference a `pull_request` checkout does **not** guarantee, even with `fetch-depth: 0`:

- **Moon affected:** `--base origin/main` needs the `origin/main` remote-tracking ref.
- **buf breaking:** `contracts:breaking` runs `buf breaking --against '../.git#branch=main,subdir=contracts'`, which needs a **local `main` branch**.

**Resolution (verify empirically):** on PR events, fetch `main` into both ref namespaces before running Moon:

```yaml
- name: Materialize main ref
  if: github.event_name == 'pull_request'
  run: |
    git fetch --no-tags origin \
      +refs/heads/main:refs/remotes/origin/main \
      +refs/heads/main:refs/heads/main
```

Populates `origin/main` (for `--base`) and a local `main` (for buf). Guarded to PR events because on a push-to-`main` build the runner is already on `main` and writing `refs/heads/main` would be refused; there, Moon's CI auto-detection compares against the previous commit and `:breaking` compares `main` against itself (a no-op).

Belt-and-suspenders: Moon also auto-detects base/head from the GitHub Actions environment, so affected detection works even without `--base`; the AC names `--base origin/main` explicitly, so we keep it.

**Open item:** confirm the buf incantation works in CI. Fallback ladder if `#branch=main` misbehaves: (a) the local-`main` fetch above; (b) switch the `breaking` task's `--against` to `#ref=origin/main`; (c) move `:breaking` to the nightly. Pick the first that verifies green.

### 6. Caching (hybrid)

`setup-toolchain` caches the proto store (Moon binary + toolchains). Layered on top, explicit `actions/cache` entries cover the heavy build artifacts the AC calls out:

| Cache | Paths | Key (with `restore-keys` prefix) |
|-------|-------|----------------------------------|
| Rust | `~/.cargo/registry`, `~/.cargo/git`, `rs/target` | `rust-${{ runner.os }}-${{ hashFiles('rs/Cargo.lock') }}` |
| pnpm store | output of `pnpm store path` (default `~/.local/share/pnpm/store`) | `pnpm-${{ runner.os }}-${{ hashFiles('ts/pnpm-lock.yaml') }}` |
| uv cache | uv cache dir (`~/.cache/uv` on Linux) | `uv-${{ runner.os }}-${{ hashFiles('py/uv.lock') }}` |
| Moon | `.moon/cache` (**repo-local**, not `~/.moon/cache`) | `moon-${{ runner.os }}-${{ github.sha }}` |

Notes:
- The AC's `~/.moon/cache` path is a Moon 1.x assumption; 2.2.5 keeps the workspace task cache **repo-local at `.moon/cache`** (gitignored). Cache that path.
- Resolve pnpm/uv cache directories at runtime (`pnpm store path`, `uv cache dir`) rather than hard-coding, to stay correct across runner image changes.
- All keys carry `restore-keys` prefixes for partial-hit warm starts.
- **Fork-PR note [F5]:** the branch-name policy invites external fork PRs. Fork PRs run with a read-only token and **cannot write `actions/cache`** — CI still works (the §5 `git fetch origin …main` resolves since `main` lives in the base repo) but fork PRs won't warm caches and run slower. Expected, not a misconfiguration.

### 7. CODEOWNERS drift gate

`.github/CODEOWNERS` is Moon-generated (`codeowners.sync: true`; CLAUDE.md: "don't hand-edit"). Rather than letting sync run silently (the AC's literal wording), CI actively **catches drift**:

```yaml
- name: CODEOWNERS up-to-date
  run: |
    moon sync code-owners
    git diff --exit-code .github/CODEOWNERS
```

A stale committed CODEOWNERS fails the build with a visible diff. (AC field corrected: `codeowners.sync`, not the issue's `codeowners.syncOnRun`; `moon sync code-owners` is the correct hyphenated Moon v2 subcommand.)

### 8. `contracts` `format` → `fmt` rename

`contracts/moon.yml` names its formatter task `format`, while Rust/Python/TypeScript use `fmt`. Renaming the task id to `fmt` (command stays `buf format --exit-code`) lets a single `:fmt` target cover every workspace, so the gate list stays uniform instead of carrying a contracts-only `:format`.

Mirrors SMA-384 (py `format` → `fmt`). Scope check: the only **live** reference to the `format` task id is `contracts/moon.yml` itself — `lefthook.yml` doesn't invoke it; other matches are point-in-time entries in `docs/superpowers/` archives (not edited). `contracts` (`layer: tool`, no `language`) inherits no language task file, so no collision.

### 9. Error handling / edge cases

- **No-test crates:** `cargo nextest` exits non-zero with no tests; neutralized by `--no-tests=pass` in `.moon/tasks/rust.yml`. Python/TS use `pytest`-with-guarded-`conftest.py` / `--passWithNoTests`.
- **Empty affected set:** a PR touching nothing gated → `moon ci` no-ops green.
- **Superseded runs:** handled by `concurrency` (§1), PR-scoped.

### 10. Out of scope (close-out)

- **Branch protection on `main`** requiring the `moon ci` check — configured in GitHub UI / via `gh api` after the **whole-graph-green** signal (see Verification), then recorded in the SMA-361 close-out comment. The required status-check name will be the job name (`moon ci`); confirm it from the first run before wiring protection.
- **Release-tool smoke job** — SMA-398 (blocked on release tooling).
- **`py:build` junk `UNKNOWN` wheel** — SMA-399 (py-twin of SMA-394). Latent quality bug, **green not red**, so not a blocker here.
- **`codegen-drift.yml` nightly** — post-MVP.

## Verification plan (on this branch's PR)

1. **Commit-lint gate:** confirm the commitlint step passes on this branch's (conventional) commits; sanity-check that a deliberately bad message would fail it.
2. **No-op pass:** open the PR; confirm the `ci` job runs green.
3. **Affected-graph end-to-end:** push a commit touching a single file in one workspace (e.g. `rs/crates/libs/paigasus-kernel/src/lib.rs`) and confirm the Moon run summary shows **only that workspace's** tasks executing, not the universe. Capture the output for the close-out comment.
4. **Whole-graph green [F2]:** before declaring CI green / configuring branch protection, run the **entire** graph once (not just affected subsets) to flush any latent per-project failure: `moon run :build :typecheck :lint :fmt :test` across all projects. *Already exercised during design on `25bc0b7`: 78 tasks completed, exit 0, zero failures — the graph is currently green (`py:build` included; it passes, see SMA-399 for its junk-artifact quality issue).* Re-confirm on the PR head before keying branch protection on it.

## Acceptance-criteria mapping

| AC (issue) | How satisfied |
|------------|---------------|
| `ci.yml` with PR trigger, `moon ci … --base origin/main`, checkout `fetch-depth: 0` | §1–§3 (explicit targets per Moon 2.2.5). |
| Cache strategy (cargo/target, Moon, pnpm, uv) | §6 (hybrid; `~/.moon/cache` corrected to repo-local `.moon/cache`). |
| Required checks: fmt, lint, test via `moon ci` | §3 — extended to build/typecheck/breaking (full graph). |
| CODEOWNERS auto-sync in CI | §7 — drift gate (`codeowners.sync`). |
| CI passes on a no-op PR | Verification #2. |
| Affected-graph verified end-to-end | Verification #3. |
| Branch protection on `main` (configure separately, document in close-out) | §10 (keyed on whole-graph-green). |
| **(SMA-371 AC-E) CI commitlint parity** | §4 — same binary + `@paigasus/commitlint-config`, lockfile-pinned. |
| **(SMA-371 AC-E) release-tool smoke job** | Deferred → SMA-398 (no release tooling yet). |

## Risks / to-verify during implementation

1. **commitlint invocation mechanism (§4)** — pnpm is Moon-managed; confirm the `moon run …:commitlint` task (or the `pnpm -C ts` fallback) resolves the lockfile-pinned binary + config and runs unconditionally (not affected-gated). Confirm the PR-range and push-range args, and that bot commits pass.
2. **buf `:breaking` base ref in CI (§5)** — the `main`-ref materialization; confirm green, with the fallback ladder if not.
3. **`setup-toolchain` + Moon 2.2.5** — confirm the pinned action SHA installs Moon **2.2.5** (not a newer default) and that `moon ci` then auto-installs the language toolchains headlessly.
4. **Cache path resolution (§6)** — resolve pnpm/uv cache dirs at runtime.
5. **Required-check name (§10)** — capture the actual status-check name from the first run before configuring branch protection.
