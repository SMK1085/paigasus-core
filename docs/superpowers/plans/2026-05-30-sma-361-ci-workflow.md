# SMA-361 — GitHub Actions CI workflow (`moon ci` + affected-graph) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the repo's first CI — a single `.github/workflows/ci.yml` that, on every PR and on push to `main`, validates commit messages and gates the full affected task graph (`moon ci`) across all four workspaces.

**Architecture:** One workflow, one job; Moon owns the cross-language affected-graph. The job bootstraps proto + Moon via `moonrepo/setup-toolchain`, restores hybrid caches, runs a commitlint parity gate (the same pinned `@paigasus/commitlint-config` as the local hook, invoked through a Moon task), then `moon ci <targets> --base origin/main`, then a CODEOWNERS drift gate. A small `contracts` `format`→`fmt` rename makes one `:fmt` target span every workspace.

**Tech Stack:** GitHub Actions, Moon 2.2.5 (proto-pinned), `moonrepo/setup-toolchain@v0`, commitlint + `@paigasus/commitlint-config` (pnpm), buf, `gh` CLI.

**Spec:** `docs/superpowers/specs/2026-05-30-sma-361-ci-workflow-design.md`

---

## File structure

| File | Responsibility | Change |
|------|----------------|--------|
| `contracts/moon.yml` | buf tasks for the proto workspace | **Modify** — rename task `format` → `fmt` (command unchanged) |
| `ts/moon.yml` | ts-workspace project config | **Modify** — add a non-cached `commitlint` task so CI runs the pinned commitlint through Moon's node toolchain |
| `.github/workflows/ci.yml` | the CI workflow | **Create** — triggers, permissions, concurrency, caches, commitlint gate, `moon ci`, CODEOWNERS drift gate |

Out of scope here (tracked separately): release-tool smoke job (SMA-398), py-root `:build` junk-wheel fix (SMA-399), `codegen-drift.yml` nightly (post-MVP). Branch protection is a manual close-out (Task 7).

**Branch:** `feature/sma-361-github-actions-ci-workflow-with-moon-ci-affected-graph` (already checked out; the spec commits are on it).

**Moon binary:** `moon` is proto-managed; if it's not on `$PATH` locally use `~/.proto/shims/moon`. Steps below write `moon` — substitute the shim if needed.

---

## Task 1: Rename the `contracts` formatter task `format` → `fmt`

Makes `moon ci … :fmt …` cover the proto workspace, so the gate target list is uniform (mirrors SMA-384's py rename). Command stays `buf format --exit-code`; only the task id changes.

**Files:**
- Modify: `contracts/moon.yml`

- [ ] **Step 1: Edit the task id**

In `contracts/moon.yml`, change the `format:` task key to `fmt:` (leave its `command`, `toolchain`, and `inputs` exactly as-is). Result:

```yaml
  fmt:
    command: 'buf format --exit-code'
    toolchain: 'system'
    inputs:
      - 'proto/**/*'
      - 'buf.yaml'
```

- [ ] **Step 2: Confirm the old task id is gone and the new one resolves**

Run: `moon run contracts:format` 
Expected: FAIL — `Unknown task format` (or similar "no such task") — proves the rename took.

Run: `moon run contracts:fmt` 
Expected: PASS (buf formats the proto; exit 0).

- [ ] **Step 3: Confirm no live references to the old id remain**

Run: `grep -rn "contracts:format" . --include='*.yml' --include='*.yaml' --include='*.cjs' --include='*.js' --include='*.json' --include='*.sh' | grep -v node_modules | grep -v '.moon/cache' | grep -v 'docs/superpowers'`
Expected: no output (the only historical matches live in `docs/superpowers/` archives, which we don't edit).

- [ ] **Step 4: Commit**

```bash
git add contracts/moon.yml
git commit -m "refactor(contracts): rename format task to fmt for uniform :fmt gate (SMA-361)"
```

---

## Task 2: Add a `commitlint` Moon task for CI commit-range validation

SMA-371 AC-E requires CI to run the **same** commitlint binary + `@paigasus/commitlint-config` version as the local hook. pnpm is Moon-managed (declared in `.moon/toolchain.yml`, not `.prototools`), so the robust way to invoke the pinned binary in CI is through a Moon node-toolchain task — Moon installs `ts/` deps and runs `pnpm exec commitlint`. The task lives on the `ts` project because that's where the binary + config are installed; it's invoked explicitly via `moon run` (never part of the affected gate).

**Files:**
- Modify: `ts/moon.yml`

- [ ] **Step 1: Append the task block**

Add this `tasks:` block to the end of `ts/moon.yml` (the file currently ends with the `workspace:` block — add `tasks:` as a new top-level key):

```yaml
# Commit-message validation for CI (SMA-371 AC-E parity gate). Lives here because the
# pinned commitlint binary + @paigasus/commitlint-config are installed under ts/. NOT part
# of the affected gate — invoked explicitly: `moon run ts:commitlint -- --from <a> --to <b>`.
# cache:false because the result depends on git history, not file inputs.
tasks:
  commitlint:
    command: 'pnpm exec commitlint --config commitlint.config.cjs'
    options:
      cache: false
      runInCI: false
```

- [ ] **Step 2: Verify it accepts a conforming commit range**

Run: `moon run ts:commitlint -- --from HEAD~1 --to HEAD`
Expected: PASS — the previous commit (`refactor(contracts): …`) conforms, exit 0. (First run also installs ts deps via Moon; that's expected.)

- [ ] **Step 3: Verify it rejects a non-conforming message**

Run: `printf 'wip\n' | pnpm -C ts exec commitlint`
Expected: FAIL (exit 1) with `type may not be empty` / `subject may not be empty` — proves the gate actually rejects bad messages using the repo's pinned config.

- [ ] **Step 4: Commit**

```bash
git add ts/moon.yml
git commit -m "feat(ci): add commitlint moon task for CI commit-range validation (SMA-361)"
```

---

## Task 3: Create `.github/workflows/ci.yml`

The workflow. Written first with version tags so it's testable; SHA-pinning happens in Task 4.

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Write the workflow file**

Create `.github/workflows/ci.yml` with exactly this content:

```yaml
name: CI

on:
  pull_request:
    branches: [main]
  push:
    branches: [main]

# Least privilege on a public repo: the job only reads and runs checks.
permissions:
  contents: read

concurrency:
  # Cancel superseded PR runs; let push-to-main runs finish (they warm caches + carry status).
  group: ci-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

jobs:
  ci:
    name: moon ci
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - name: Checkout (full history for Moon's affected-graph)
        uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Materialize main ref (for --base origin/main and buf breaking)
        if: github.event_name == 'pull_request'
        run: |
          git fetch --no-tags origin \
            +refs/heads/main:refs/remotes/origin/main \
            +refs/heads/main:refs/heads/main

      - name: Set up proto + Moon (pinned via .prototools)
        uses: moonrepo/setup-toolchain@v0

      - name: Install pinned CLIs from .prototools (buf, lefthook)
        run: proto install

      - name: Cache Rust (cargo + target)
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            rs/target
          key: rust-${{ runner.os }}-${{ hashFiles('rs/Cargo.lock') }}
          restore-keys: |
            rust-${{ runner.os }}-

      - name: Cache pnpm store
        uses: actions/cache@v4
        with:
          path: ~/.local/share/pnpm/store
          key: pnpm-${{ runner.os }}-${{ hashFiles('ts/pnpm-lock.yaml') }}
          restore-keys: |
            pnpm-${{ runner.os }}-

      - name: Cache uv
        uses: actions/cache@v4
        with:
          path: ~/.cache/uv
          key: uv-${{ runner.os }}-${{ hashFiles('py/uv.lock') }}
          restore-keys: |
            uv-${{ runner.os }}-

      - name: Cache Moon task cache
        uses: actions/cache@v4
        with:
          path: .moon/cache
          key: moon-${{ runner.os }}-${{ github.sha }}
          restore-keys: |
            moon-${{ runner.os }}-

      - name: Validate commit messages (commitlint parity gate)
        env:
          BASE: ${{ github.event_name == 'pull_request' && github.event.pull_request.base.sha || github.event.before }}
          HEAD: ${{ github.event_name == 'pull_request' && github.event.pull_request.head.sha || github.sha }}
        run: |
          if printf '%s' "$BASE" | grep -qE '^0+$'; then
            echo "Initial push (no base commit); skipping commit-range lint."
            exit 0
          fi
          moon run ts:commitlint -- --from "$BASE" --to "$HEAD"

      - name: moon ci (affected graph)
        env:
          EVENT: ${{ github.event_name }}
          BEFORE: ${{ github.event.before }}
        run: |
          set -euo pipefail
          T=(:build :test :lint :fmt :typecheck :breaking)
          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]}" --base origin/main
          elif [ -n "${BEFORE:-}" ] && ! printf '%s' "$BEFORE" | grep -qE '^0+$'; then
            moon ci "${T[@]}" --base "$BEFORE"
          else
            # Initial push with no usable base — run the whole graph to warm caches.
            moon run "${T[@]}"
          fi

      - name: CODEOWNERS up-to-date (drift gate)
        run: |
          moon sync code-owners
          git diff --exit-code .github/CODEOWNERS
```

- [ ] **Step 2: Verify the YAML parses**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('YAML OK')"`
Expected: `YAML OK`

- [ ] **Step 3: Optional deeper lint (if available)**

Run: `command -v actionlint >/dev/null && actionlint .github/workflows/ci.yml || echo "actionlint not installed — skipping (PR run is the authority)"`
Expected: either `actionlint` prints no findings, or the skip message. Do **not** install actionlint just for this.

- [ ] **Step 4: Smoke-test the gate command locally against main**

This runs the exact gate command the workflow uses, comparing the branch to `main`.

Run: `moon ci :build :test :lint :fmt :typecheck :breaking --base main`
Expected: PASS (exit 0). Affected projects are `contracts` + `ts` (their `moon.yml` changed) plus anything they depend on; all green. If `moon ci` refuses to run outside a CI environment, fall back to `moon run :build :test :lint :fmt :typecheck :breaking` (whole graph) — also expected green.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "feat(ci): add moon ci GitHub Actions workflow with affected-graph (SMA-361)"
```

---

## Task 4: Pin third-party actions to commit SHAs

Public repo → pin actions to immutable SHAs (spec F3). Resolve the SHA each floating tag currently points to and replace the tags, keeping the tag in a trailing comment.

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Resolve the current SHAs**

Run:
```bash
for spec in actions/checkout@v4 actions/cache@v4 moonrepo/setup-toolchain@v0; do
  repo="${spec%@*}"; ref="${spec#*@}"
  sha=$(gh api "repos/$repo/commits/$ref" --jq '.sha')
  echo "$repo  ($ref)  $sha"
done
```
Expected: three lines, each ending in a 40-hex SHA. Record them.

- [ ] **Step 2: Replace each `uses:` tag with its SHA + version comment**

Edit `.github/workflows/ci.yml`. For every `uses:` line, replace the `@<tag>` with `@<resolved-sha>` and append `# <tag>`. There are 6 `uses:` lines (1× checkout, 4× cache, 1× setup-toolchain). Example shape (use YOUR resolved SHAs from Step 1):

```yaml
        uses: actions/checkout@<sha>            # v4
        uses: moonrepo/setup-toolchain@<sha>    # v0
        uses: actions/cache@<sha>               # v4
```

- [ ] **Step 3: Verify every action is SHA-pinned**

Run: `grep -nE 'uses:' .github/workflows/ci.yml`
Expected: every line shows `@` followed by a 40-character hex SHA and a `# v…` comment — no bare `@v4`/`@v0` remain.

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml')); print('YAML OK')"`
Expected: `YAML OK`

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: pin workflow actions to commit SHAs (SMA-361)"
```

---

## Task 5: Local verification — whole-graph green + affected-graph isolation

No file changes; this produces the evidence the close-out comment needs (spec Verification #3, #4). Run from a clean tree (all prior tasks committed).

- [ ] **Step 1: Whole-graph green (flush any latent per-project failure)**

Run: `moon run :build :typecheck :lint :fmt :test 2>&1 | tail -5`
Expected: ends with a summary like `Tasks: N completed` and overall exit 0 — zero failures across all projects. (Confirms the graph branch protection will key on is genuinely green.)

- [ ] **Step 2: Demonstrate affected-graph isolation (single file → one workspace)**

Edit one Rust source file so exactly one project is affected:

Run: `printf '\n// affected-graph probe (SMA-361) — to be discarded\n' >> rs/crates/libs/paigasus-kernel/src/lib.rs`

Then run the affected set:

Run: `moon run :build :test :lint :fmt --affected 2>&1 | grep -E ':(build|test|lint|fmt)' | sort -u`
Expected: only `paigasus-kernel-rs:*` targets appear (no `py`/`ts`/`contracts` projects) — the affected-graph selects just the touched workspace. Capture this output for the close-out comment. If `--affected` isn't honored, use `moon ci :build :test :lint :fmt --base HEAD` (compares the uncommitted edit against HEAD) for the same result.

- [ ] **Step 3: Discard the probe edit (keep the PR clean)**

Run: `git checkout -- rs/crates/libs/paigasus-kernel/src/lib.rs`
Run: `git status --short`
Expected: no changes to `rs/` (only untracked `.claude/` may remain). No commit from this task.

---

## Task 6: Push the branch, open the PR, confirm CI is green

- [ ] **Step 1: Push the branch**

Run: `git push -u origin feature/sma-361-github-actions-ci-workflow-with-moon-ci-affected-graph`
Expected: push succeeds. (The lefthook `pre-push` hook allows `feature/…` branches.)

- [ ] **Step 2: Open the PR**

Run:
```bash
gh pr create --base main \
  --title "feat(ci): GitHub Actions CI workflow with moon ci + affected-graph (SMA-361)" \
  --body "Implements SMA-361 per docs/superpowers/specs/2026-05-30-sma-361-ci-workflow-design.md.

- moon ci affected-graph gate (:build :test :lint :fmt :typecheck :breaking, --base origin/main)
- commitlint CI-parity gate (SMA-371 AC-E)
- hybrid caching, CODEOWNERS drift gate, contracts format→fmt rename
- permissions: contents: read; actions SHA-pinned; PR-scoped cancel; 30m timeout

Follow-ups: SMA-398 (release smoke job), SMA-399 (py-root build junk wheel)."
```
Expected: PR created and printed. (Do NOT manually attach the Linear link — the integration auto-links by branch name.)

- [ ] **Step 2.5: This PR's CI run is the first-ever execution of `ci.yml`**

Because `ci.yml` is added *in this PR*, the `pull_request` event runs the new workflow against this very diff — the no-op-pass verification (spec Verification #2) is this run itself.

- [ ] **Step 3: Watch the checks**

Run: `gh pr checks --watch`
Expected: the `moon ci` check completes **success**. The job's commitlint step passes (all commits conform) and the affected graph (contracts + ts) builds/tests/lints green.

- [ ] **Step 4: If CI fails, triage against the spec's known risks**

Read the failing step's logs (`gh run view --log-failed`). Map to spec "Risks / to-verify":
- commitlint step → mechanism risk #1 (range args / `moon run ts:commitlint` / deps). 
- `buf breaking` inside `moon ci` → risk #2 (the `main`-ref materialization / fallback ladder: try `#ref=origin/main`, else move `:breaking` to nightly).
- toolchain/`buf not found` → risk #3 (confirm `proto install` ran; setup-toolchain installed Moon 2.2.5).
Fix, commit, push; re-watch. Do not proceed until green.

---

## Task 7: Close-out — branch protection + Linear (manual, after CI is green)

Branch protection lives in GitHub settings, not the workflow file (spec §10). Do this **after** Task 5 Step 1 shows whole-graph-green AND Task 6 shows the `moon ci` check green — so protection keys on a real green signal.

- [ ] **Step 1: Confirm the exact required-check name**

Run: `gh pr checks` (on the open PR)
Expected: a check literally named `moon ci`. Use this exact string below.

- [ ] **Step 2: Configure branch protection on `main`**

Either via the GitHub UI (Settings → Branches → Add branch ruleset / protection rule for `main` → Require status checks to pass → add `moon ci` → Require branches up to date), **or** via API:

```bash
gh api -X PUT repos/SMK1085/paigasus-core/branches/main/protection --input - <<'JSON'
{
  "required_status_checks": { "strict": true, "checks": [ { "context": "moon ci" } ] },
  "enforce_admins": true,
  "required_pull_request_reviews": null,
  "restrictions": null
}
JSON
```
Expected: returns the protection JSON with `required_status_checks.checks[0].context == "moon ci"`.

- [ ] **Step 3: Verify protection is active**

Run: `gh api repos/SMK1085/paigasus-core/branches/main/protection --jq '.required_status_checks.checks'`
Expected: `[{"context":"moon ci",...}]`.

- [ ] **Step 4: Document the close-out in Linear SMA-361**

Post a comment on SMA-361 recording: the required check name (`moon ci`), that branch protection now requires it on `main`, the whole-graph-green evidence (Task 5 Step 1 summary), and the affected-graph proof (Task 5 Step 2 output). Note the two spun-out follow-ups (SMA-398, SMA-399). (Use the Linear MCP `save_comment` / the Linear UI.)

- [ ] **Step 5: Merge + transition**

After review approval and green CI, merge the PR. Moon's auto-link + the merge will close the loop; transition SMA-361 to **Done** in Linear (or let the merge automation do it if configured).

---

## Self-review notes (author checklist — already applied)

- **Spec coverage:** PR trigger + `moon ci --base origin/main` + `fetch-depth: 0` (Task 3); caches incl. repo-local `.moon/cache` (Task 3); fmt/lint/test + build/typecheck/breaking gate (Task 3 Step 1); CODEOWNERS drift gate (Task 3); no-op pass (Task 6 Step 2.5); affected-graph end-to-end (Task 5 Step 2 + Task 6); branch protection close-out (Task 7); commitlint parity gate (Tasks 2–3); `contracts` rename (Task 1); push trigger + PR-scoped cancel + permissions + timeout + SHA pinning (Tasks 3–4). Release smoke job → SMA-398; py junk-wheel → SMA-399 (out of scope, referenced).
- **Type/name consistency:** task id `fmt` (Task 1) matches the `:fmt` gate target (Task 3); `ts:commitlint` task name (Task 2) matches the `moon run ts:commitlint` invocation (Task 3); required-check name `moon ci` (job `name:` in Task 3) matches branch protection (Task 7).
- **No placeholders:** the only `<sha>`/`<tag>` tokens are in Task 4, which resolves them via an exact `gh api` command before they land in the file.
