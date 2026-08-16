# SMA-520 Cut GitHub Actions Spend — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `prebuild.yml` rebuilding 7 platforms on every merge, migrate `darwin-x64` off the retiring `macos-15-intel` runner, and hand Sven an executable runbook for making the repo public.

**Architecture:** Three edits to one workflow file (triggers, merged darwin job, cache re-key), one `.gitignore` hardening line, and one new ops runbook. No application code changes. The workflow's own `pull_request` trigger — added by Task 1 — makes this PR verify itself, because the PR modifies a file on its own allowlist.

**Tech Stack:** GitHub Actions workflow YAML, napi-rs CLI v3.7.2, `actionlint` (via Docker), `lipo`/`otool` (macOS runner), `gh` CLI.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-16-sma-520-cut-actions-spend-design.md` (revision 2, approved 2026-08-16).
- **Only two workflow behaviours may change:** `prebuild.yml` triggers/concurrency, and the darwin build job. Do **not** touch `ci.yml` — removing its authenticated `main` fetch is a post-flip runbook step and merging it now breaks every PR run.
- Conventional commits, workspace scope from the allowlist in `ts/packages/commitlint-config/index.cjs`: types `feat|fix|docs|chore|refactor|test|ci|build|perf|style|revert`, scopes `rs|py|ts|contracts|ci|docs|deps|release|repo|claude|workspace`. Scope is mandatory. Subject must start lowercase. Header ≤100 chars, body lines ≤100 chars.
- Do **not** put a `#NNN` issue reference in a commit body — it makes commitlint fail `footer-leading-blank`. Write "owner/repo PR NNN".
- Never commit with `--no-verify`.
- Every source file opens with an SPDX header (`#` for YAML/Markdown-adjacent config). `.github/workflows/*.yml` in this repo do **not** carry one — match the existing file, do not add one.
- All 7 platform artifacts must still be produced: `darwin-x64`, `darwin-arm64`, `win32-x64-msvc`, `linux-x64-gnu`, `linux-arm64-gnu`, `linux-x64-musl`, `linux-arm64-musl`.
- Bash tool PATH lacks proto CLIs — prefix with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-520`, branch `feature/sma-520-cut-actions-spend`. Run everything from there.

## File Structure

| File | Action | Responsibility |
| -- | -- | -- |
| `.github/workflows/prebuild.yml` | Modify | Triggers + concurrency (Task 1); merged darwin job + cache re-key (Task 2) |
| `.gitignore` | Modify | Keep agent scratch dirs out of a soon-to-be-public tree (Task 3) |
| `docs/ops/RUNBOOK-go-public.md` | Create | Operator procedure for the visibility flip (Task 4) |
| `docs/superpowers/specs/2026-06-17-sma-428-napi-prebuild-matrix-design.md` | Modify | Supersession note (Task 5) |

---

### Task 1: prebuild.yml — triggers and concurrency

**Files:**
- Modify: `.github/workflows/prebuild.yml:3-16` (the `on:` and `concurrency:` blocks)

**Interfaces:**
- Consumes: nothing.
- Produces: a `pull_request` trigger whose `paths` list includes `.github/workflows/prebuild.yml`. Task 6 depends on this — it is what makes this PR verify itself.

**Why the two path lists differ (do not "tidy" them into one):** `.github/dependabot.yml` emits a grouped `npm-minor-patch` PR every Monday that always touches `ts/pnpm-lock.yaml`. Putting `ts/` paths on the `push` trigger would fire the full matrix weekly and violate AC-3. Putting `rs/**` on the `pull_request` trigger would add a macOS job to most PRs in this repo and *increase* the bill. The split is deliberate.

- [ ] **Step 1: Read the current trigger block**

Run: `sed -n '1,20p' .github/workflows/prebuild.yml`
Expected: `on:` with `workflow_dispatch:` and `push: branches: [main]` and no `paths:`; `concurrency.group` without `github.event_name`.

- [ ] **Step 2: Replace the `on:` block**

Replace lines 3-6 (`on:` through `branches: [main]`) with:

```yaml
on:
  workflow_dispatch:

  # POST-merge verification of Rust changes. Deliberately excludes every `ts/` path:
  # dependabot.yml emits a grouped npm PR every Monday that always touches
  # ts/pnpm-lock.yaml, and listing it here would fire the full matrix weekly for a
  # change that cannot affect the addon's Rust source (SMA-520 AC-3).
  push:
    branches: [main]
    paths:
      - 'rs/**'                              # includes rs/Cargo.lock + rs/rust-toolchain.toml
      - '.github/workflows/prebuild.yml'
      - '.prototools'
      - '.moon/**'                           # `moon setup` needs workspace.yml as well as toolchains.yml

  # PRE-merge verification of the BUILD INPUTS, which all live in ts/. @napi-rs/cli is
  # `catalog:` in the kernel package.json, resolved from ts/pnpm-workspace.yaml and pinned
  # in ts/pnpm-lock.yaml — a napi CLI bump is the change most likely to break a cross-build,
  # so it is verified before it merges rather than after. `rs/**` is deliberately absent:
  # most PRs here touch it, and a macOS job on every one of them would raise the bill.
  # Not a required check (the `Protect main` ruleset requires only `moon ci`), so a skipped
  # run cannot wedge a merge.
  pull_request:
    branches: [main]
    paths:
      - '.github/workflows/prebuild.yml'
      - '.prototools'
      - '.moon/**'
      - 'ts/pnpm-lock.yaml'
      - 'ts/pnpm-workspace.yaml'
      - 'ts/packages/paigasus-kernel/package.json'
      - 'ts/.npmrc'                          # pins the registry for `pnpm install` (steps 83 + 116)
```

- [ ] **Step 3: Replace the `concurrency:` block**

Replace the existing `concurrency:` block with:

```yaml
concurrency:
  # `event_name` is in the GROUP so a manual dispatch cannot cancel a running push-to-main
  # job — without it both resolve to refs/heads/main and share a group.
  group: prebuild-${{ github.workflow }}-${{ github.ref }}-${{ github.event_name }}
  # Never cancel a push run: this workflow's actions/cache step is the ONLY writer to
  # main's cache scope, and a cancelled job does not run cache's post-step save. PR and
  # dispatch runs carry no such value, so superseded ones are cancelled.
  cancel-in-progress: ${{ github.event_name != 'push' }}
```

**Note — refinement over the spec text.** The spec says "keep `cancel-in-progress: ${{ github.event_name == 'workflow_dispatch' }}`". That expression was written before the `pull_request` trigger was finalised in the same revision, and would leave superseded PR runs uncancelled. `!= 'push'` preserves the spec's actual rationale (protect push cache writes, cancel everything else) and matches `ci.yml:16`. Flag this in the PR description.

- [ ] **Step 4: Verify the YAML parses and the trigger shape is correct**

```bash
python3 - <<'PY'
import yaml
d = yaml.safe_load(open('.github/workflows/prebuild.yml'))
on = d[True] if True in d else d['on']          # PyYAML parses bare `on:` as boolean True
assert set(on) == {'workflow_dispatch', 'push', 'pull_request'}, on.keys()
push = set(on['push']['paths']); pr = set(on['pull_request']['paths'])
assert not any(p.startswith('ts/') for p in push), f"AC-3 violated: ts path on push: {push}"
assert 'rs/**' in push and 'rs/**' not in pr
assert '.github/workflows/prebuild.yml' in pr, "PR trigger must include the workflow file"
for p in ('ts/pnpm-lock.yaml','ts/pnpm-workspace.yaml','ts/packages/paigasus-kernel/package.json','ts/.npmrc'):
    assert p in pr, f"missing napi input: {p}"
print("trigger shape OK")
print("push:", sorted(push))
print("pr:  ", sorted(pr))
PY
```

Expected: `trigger shape OK` and the two lists printed.

- [ ] **Step 5: Confirm every referenced path actually exists**

```bash
for f in rs .github/workflows/prebuild.yml .prototools .moon \
         ts/pnpm-lock.yaml ts/pnpm-workspace.yaml \
         ts/packages/paigasus-kernel/package.json ts/.npmrc; do
  [ -e "$f" ] && echo "OK   $f" || { echo "MISS $f"; exit 1; }
done
```

Expected: eight `OK` lines. A `MISS` means a glob that can never match — silently disabling the trigger.

- [ ] **Step 6: Run actionlint**

```bash
docker run --rm -v "$PWD:/repo" -w /repo rhysd/actionlint:latest -color .github/workflows/prebuild.yml
```

Expected: no output, exit 0. If Docker is unavailable, say so and stop — do not skip the lint.

- [ ] **Step 7: Commit**

```bash
git add .github/workflows/prebuild.yml
git commit -m "ci(repo): path-filter prebuild and split pre/post-merge verification (SMA-520)"
```

---

### Task 2: prebuild.yml — merge the darwin legs and re-key the cache

**Files:**
- Modify: `.github/workflows/prebuild.yml` — matrix `include:`, job `name:`/`timeout-minutes:`, cache `key:`/`restore-keys:`, the rustup step, the build steps, a new arch-assertion step, the upload steps

**Interfaces:**
- Consumes: nothing from Task 1 (same file, disjoint blocks — but rebase on Task 1's commit).
- Produces: artifacts named `prebuild-darwin-x64` and `prebuild-darwin-arm64`, which `assemble`'s existing `pattern: prebuild-*` + `merge-multiple: true` download consumes unchanged.

**Load-bearing facts — do not "simplify" these away:**
- `rs/.cargo/config.toml` declares `-undefined dynamic_lookup` link flags for **both** apple triples. Cargo finds that file by walking up from its working directory, which is why `--cwd ../../../rs/crates/bindings/paigasus-node-bindings` must be preserved verbatim on the second build step.
- napi derives the `.node` platform suffix from `--target`, not from the host (proved in-repo: the `linux-x64-musl` leg cross-builds on `ubuntu-latest`). The two darwin builds therefore write different filenames and cannot overwrite each other.
- The cache re-key is not cosmetic. Without it the merged job hits the pre-existing `aarch64-apple-darwin` key, `actions/cache` skips its post-job save, and `rs/target/x86_64-apple-darwin/` is never cached — a full cold compile every run, forever.

- [ ] **Step 1: Delete the `macos-15-intel` matrix entry and extend the arm64 one**

Replace the first two `include:` entries with a single entry:

```yaml
          # Both darwin targets build in ONE macos-latest (arm64) job. macos-15-intel is the
          # last x86_64 macOS image and disappears in August 2027; napi-rs's own generated CI
          # uses macos-latest for both apple triples with no cross flag, because the macOS SDK
          # ships both slices. Merging also drops the duplicated proto/Moon/pnpm setup.
          - { platform: darwin-arm64,     target: aarch64-apple-darwin,       runner: macos-latest,     zig: false, extra_platform: darwin-x64, extra_target: x86_64-apple-darwin }
```

Leave the other five entries byte-identical.

- [ ] **Step 2: Update the job `name:` and `timeout-minutes:`**

```yaml
    name: build ${{ matrix.platform }}${{ matrix.extra_platform && format(' + {0}', matrix.extra_platform) || '' }}
    runs-on: ${{ matrix.runner }}
    # The darwin leg does two full --release builds; 30 was sized for one.
    timeout-minutes: ${{ matrix.extra_target && 45 || 30 }}
```

Safe to rename: the `Protect main` ruleset requires only the `moon ci` check and declares no `required_workflows`.

- [ ] **Step 3: Re-key the Rust cache**

Insert a literal `dual` segment into both the key and the restore-key prefix:

```yaml
          # `dual` segment: the darwin leg now populates TWO target triples under one key.
          # Without a new literal the merged job would hit the pre-change arm64 key, and
          # actions/cache skips its post-job save on an exact primary-key hit — so
          # rs/target/x86_64-apple-darwin/ would be restored-never and saved-never.
          # Precedent: ci.yml's `-line-tables-only-` segment, added for the same reason.
          key: prebuild-rust-${{ runner.os }}-${{ matrix.target }}-dual-${{ hashFiles('rs/rust-toolchain.toml') }}-${{ hashFiles('rs/Cargo.lock') }}
          restore-keys: |
            prebuild-rust-${{ runner.os }}-${{ matrix.target }}-dual-${{ hashFiles('rs/rust-toolchain.toml') }}-
```

The segment applies to all six legs, so every leg pays one cold build once. That is intended — it is a one-time cost that guarantees the darwin fix takes effect.

- [ ] **Step 4: Extend the rustup step to both triples**

```yaml
      - name: Add Rust target(s) (pinned toolchain)
        working-directory: rs
        # rustup accepts several triples at once; matrix.extra_target expands to nothing on
        # the five single-target legs, leaving the original one-target command.
        run: rustup target add ${{ matrix.target }} ${{ matrix.extra_target }}
```

- [ ] **Step 5: Add the second build step immediately after the existing one**

Leave the existing `Build the addon` step untouched, then add:

```yaml
      # Exact copy of the step above with only --target changed. No `-x`: darwin never
      # cross-compiles via zig (that is musl-only), and on a macOS host no cross flag is
      # needed for either apple arch. --cwd is load-bearing — it is how cargo discovers
      # rs/.cargo/config.toml and therefore the -undefined dynamic_lookup link flags.
      - name: Build the addon (second darwin target)
        if: ${{ matrix.extra_target }}
        working-directory: ts/packages/paigasus-kernel
        run: pnpm exec napi build --platform --release --target ${{ matrix.extra_target }} --cwd ../../../rs/crates/bindings/paigasus-node-bindings
```

- [ ] **Step 6: Add the architecture assertion after both builds, before both uploads**

```yaml
      # Nothing in this workflow ever EXECUTES a macOS binary (the only runtime check is
      # linux-x64-gnu resolution in `assemble`, on ubuntu), so a cross-built darwin artifact
      # is otherwise unverified. Exact equality, NOT a substring match: `lipo -archs` prints
      # "x86_64 arm64" for a universal binary, so `grep -q x86_64` would pass for a fat file —
      # vacuously green in precisely the case worth catching.
      - name: Verify Mach-O architecture (darwin only)
        if: runner.os == 'macOS'
        working-directory: rs/crates/bindings/paigasus-node-bindings
        run: |
          set -euo pipefail
          arm="$(lipo -archs paigasus-node-bindings.darwin-arm64.node)"
          x64="$(lipo -archs paigasus-node-bindings.darwin-x64.node)"
          echo "darwin-arm64 archs: [$arm]"
          echo "darwin-x64   archs: [$x64]"
          [ "$arm" = "arm64" ]  || { echo "::error::darwin-arm64 is not pure arm64: [$arm]"; exit 1; }
          [ "$x64" = "x86_64" ] || { echo "::error::darwin-x64 is not pure x86_64: [$x64]"; exit 1; }
          # Record the minimum-OS stamp rather than assume it. Rust pins the deployment target
          # per TARGET not per host, so this should match what macos-15-intel produced — but
          # napi's own generated CI sets MACOSX_DEPLOYMENT_TARGET=10.13 and this workflow does
          # not, so print it. If it regresses, set MACOSX_DEPLOYMENT_TARGET on this job.
          echo "== LC_BUILD_VERSION =="
          otool -l paigasus-node-bindings.darwin-x64.node   | grep -A3 LC_BUILD_VERSION || true
          otool -l paigasus-node-bindings.darwin-arm64.node | grep -A3 LC_BUILD_VERSION || true
```

`if: runner.os == 'macOS'` is mandatory — `lipo` does not exist on ubuntu or windows and an ungated step reds the other five legs.

- [ ] **Step 7: Add the second upload step after the existing one**

```yaml
      # Separate upload (not a glob) so the artifact keeps the prebuild-<platform> name that
      # `assemble`'s `pattern: prebuild-*` + merge-multiple download depends on.
      - name: Upload prebuild artifact (second darwin target)
        if: ${{ matrix.extra_platform }}
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: prebuild-${{ matrix.extra_platform }}
          path: rs/crates/bindings/paigasus-node-bindings/paigasus-node-bindings.${{ matrix.extra_platform }}.node
          if-no-files-found: error
```

Pin the action to the same SHA the existing upload step uses — copy it, do not write `@v7`.

- [ ] **Step 8: Verify the matrix still yields all 7 platforms and no macos-15-intel remains**

```bash
grep -c "macos-15-intel" .github/workflows/prebuild.yml   # expect: 0
python3 - <<'PY'
import yaml
d = yaml.safe_load(open('.github/workflows/prebuild.yml'))
inc = d['jobs']['build']['strategy']['matrix']['include']
plats = [e['platform'] for e in inc] + [e['extra_platform'] for e in inc if 'extra_platform' in e]
expected = {'darwin-x64','darwin-arm64','win32-x64-msvc','linux-x64-gnu',
            'linux-arm64-gnu','linux-x64-musl','linux-arm64-musl'}
assert set(plats) == expected, (set(plats) ^ expected)
assert len(plats) == 7, plats
assert len(inc) == 6, f"expected 6 jobs, got {len(inc)}"
mac = [e for e in inc if e['runner'].startswith('macos')]
assert len(mac) == 1, mac
print(f"OK: {len(inc)} jobs, {len(plats)} platforms, {len(mac)} macOS job")
PY
```

Expected: `0`, then `OK: 6 jobs, 7 platforms, 1 macOS job`.

- [ ] **Step 9: Run actionlint**

```bash
docker run --rm -v "$PWD:/repo" -w /repo rhysd/actionlint:latest -color .github/workflows/prebuild.yml
```

Expected: no output, exit 0. Pay attention to any complaint about `timeout-minutes` accepting an expression; if actionlint rejects it, replace with a literal `45` on the whole job and note the change.

- [ ] **Step 10: Commit**

```bash
git add .github/workflows/prebuild.yml
git commit -m "ci(repo): build both darwin targets on macos-latest (SMA-520)"
```

---

### Task 3: Keep agent scratch directories out of a public tree

**Files:**
- Modify: `.gitignore`

**Why:** `.gitignore` covers `.env`, `*.pem`, `*.key` but neither `.claude/` nor `.entire/`. Both exist untracked in the working tree today and are one `git add -A` from publication. `.claude/worktrees/` in particular holds full repository checkouts.

- [ ] **Step 1: Confirm neither is currently ignored**

```bash
grep -nE "claude|entire" .gitignore || echo "NEITHER PRESENT (expected)"
```

Expected: `NEITHER PRESENT (expected)`.

- [ ] **Step 2: Append the entries**

Add to the end of `.gitignore`:

```gitignore

# Agent scratch state — never publishable. `.claude/worktrees/` holds entire repository
# checkouts, and both dirs exist untracked today, so a stray `git add -A` before the
# SMA-520 visibility flip would publish them.
.claude/
.entire/
```

- [ ] **Step 3: Verify both are now ignored**

```bash
git check-ignore -v .claude/ .entire/
```

Expected: two lines naming `.gitignore` and the new patterns.

- [ ] **Step 4: Verify nothing already tracked becomes ignored**

```bash
git ls-files | grep -E "^\.(claude|entire)/" && echo "WARNING: tracked files now ignored" || echo "OK: nothing tracked under those paths"
```

Expected: `OK: nothing tracked under those paths`.

- [ ] **Step 5: Commit**

```bash
git add .gitignore
git commit -m "chore(repo): ignore agent scratch dirs before going public (SMA-520)"
```

---

### Task 4: The go-public runbook

**Files:**
- Create: `docs/ops/RUNBOOK-go-public.md`

**Interfaces:**
- Consumes: the scan evidence recorded in the spec.
- Produces: the operator procedure referenced by the PR description.

Match the house style of `docs/ops/RUNBOOK-nats.md` and `RUNBOOK-observability.md` — read one first for heading depth and tone.

- [ ] **Step 1: Read an existing runbook for style**

Run: `head -40 docs/ops/RUNBOOK-nats.md`

- [ ] **Step 2: Write the runbook**

Create `docs/ops/RUNBOOK-go-public.md` containing, in this order:

1. **Purpose** — one paragraph: flipping `SMK1085/paigasus-core` to public zeroes the Actions bill, because standard runners are free on public repos. Quote the pricing clause verbatim: *"Standard GitHub-hosted or self-hosted runner usage on public repositories will remain free."* Note it is uniform across runner types with no macOS carve-out.
2. **Irreversibility warning** — a flip cannot be undone with respect to disclosure. Reverting to private detaches existing forks into a new network and does not un-publish anything already cloned or scraped.
3. **Pre-flight A — credential scan.** Record: gitleaks over a mirror clone, **0 findings across 777 commits**, 2026-08-16. State that `git clone --mirror` does **not** fetch `refs/pull/*` (GitHub does not advertise them) and give the command: `git fetch origin '+refs/pull/*:refs/pull/*'`. State that force-push-orphaned objects cannot be enumerated locally, are retained by GitHub, and are served by SHA on public repos — therefore **a clean scan does not prove absence, and any suspected historical credential must be rotated**, recorded as an explicit decision.
4. **Pre-flight B — content review.** 71 tracked files carry internal references (87 `linear.app`, 72 `.internal`, 11 `notion.so`); 59 are in `docs/superpowers/specs/` and `plans/`. **Decision (Sven, 2026-08-16): publish as-is** — the design history is part of what the open repo offers, and those URLs 404 for outsiders. Also note that all historical Actions run logs and artifacts become world-readable.
5. **Flip sequence** — numbered, in this exact order, with the reason each ordering matters:
   1. Disable Actions (Settings → Actions → Disable).
   2. Set an Actions spending limit.
   3. `gh repo edit SMK1085/paigasus-core --visibility public --accept-visibility-change-consequences`
   4. Set fork PR workflow approval to **"Require approval for all outside collaborators"** — chosen over GitHub's newly-public default of "first-time contributors" because `moon ci` runs arbitrary build scripts and testcontainers. Note this control is public-repo-only, which is why it cannot be done before step 3.
   5. Re-enable Actions.
   6. Enable secret scanning **and push protection** (free on public repos; push protection blocks at push rather than reporting after the fact).
   7. Confirm the `Protect main` ruleset and `dependabot.yml` survived.
6. **Post-flip cleanup** — (a) remove `ci.yml`'s "Materialize main ref" authenticated fetch (lines 53-63) and fix the contradictory comments on lines 9/50/58; **this must not be done before the flip** — an anonymous fetch of `main` fails while private. Warn that once it lands, reverting visibility silently breaks every PR run, so a revert must revert this commit in lockstep. (b) `gh cache delete` the orphaned `prebuild-rust-macOS-x86_64-apple-darwin-*` entry, which is never read again after SMA-520.
7. **Verification** — AC-1 is confirmed by Sven on the billing page at the start of the next cycle. Minutes already accrued in the current cycle still invoice.
8. **If the flip does not happen** — AC-1 permanently unmet; W2+W3 deliver under ~15%. Fallback: keep the path filter, accept ~$14/month, and reopen matrix tiering (rejected in the spec purely on the assumption the flip lands).
9. **Note** — on public repos GitHub disables scheduled workflows after 60 days of repository inactivity, which affects `security-scan.yml`'s daily cron.

- [ ] **Step 3: Verify the runbook has no unresolved placeholders**

```bash
grep -nE "TBD|TODO|FIXME|XXX|<placeholder>" docs/ops/RUNBOOK-go-public.md && exit 1 || echo "no placeholders"
```

Expected: `no placeholders`.

- [ ] **Step 4: Verify every command in it is syntactically valid**

```bash
grep -oE '^\s*gh [a-z].*' docs/ops/RUNBOOK-go-public.md
```

Read each line and confirm the subcommand and flags exist (`gh repo edit --visibility`, `gh cache delete`). Do **not** execute them.

- [ ] **Step 5: Commit**

```bash
git add docs/ops/RUNBOOK-go-public.md
git commit -m "docs(ci): add the go-public runbook for SMA-520"
```

---

### Task 5: Supersession note on the SMA-428 spec

**Files:**
- Modify: `docs/superpowers/specs/2026-06-17-sma-428-napi-prebuild-matrix-design.md`

**Why:** it still documents `darwin-x64 → macos-15-intel` and describes the arm64 cross-build as a "fallback if macos-15-intel is constrained". That fallback is now the design. Historical specs are otherwise left frozen — add a pointer, do not rewrite the body.

- [ ] **Step 1: Locate the stale claims**

```bash
grep -n "macos-15-intel" docs/superpowers/specs/2026-06-17-sma-428-napi-prebuild-matrix-design.md
```

Expected: hits around lines 73 and 85-93.

- [ ] **Step 2: Insert a note directly under the document's top-level heading**

```markdown
> **Superseded in part by SMA-520 (2026-08-16).** The `darwin-x64 → macos-15-intel`
> mapping below is obsolete: `macos-15-intel` is the last x86_64 macOS image on Actions
> and retires in August 2027. Both darwin targets now build in a single `macos-latest`
> (arm64) job — what this document called a fallback is now the design. The rest of this
> spec is left as the historical record.
```

- [ ] **Step 3: Verify the note renders and the body is unchanged**

```bash
head -12 docs/superpowers/specs/2026-06-17-sma-428-napi-prebuild-matrix-design.md
git diff --stat docs/superpowers/specs/2026-06-17-sma-428-napi-prebuild-matrix-design.md
```

Expected: the note appears near the top; the diff shows only insertions (no deletions).

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/2026-06-17-sma-428-napi-prebuild-matrix-design.md
git commit -m "docs(ci): note SMA-520 supersedes the SMA-428 darwin runner mapping"
```

---

### Task 6: Prove it on real runners

**Files:** none — this task verifies, it does not edit.

**Interfaces:**
- Consumes: Task 1's `pull_request` trigger and Task 2's darwin job.

**The self-verification property:** this PR modifies `.github/workflows/prebuild.yml`, which Task 1 puts on the `pull_request` allowlist. So opening the PR *automatically* runs the full 6-job matrix under the new configuration. For `pull_request` events GitHub evaluates the workflow from the merge ref, so the new triggers apply to the PR that introduces them. No manual dispatch is needed unless the run fails to appear.

Cost: roughly $0.75–1.00 per full matrix run while the repo is still private. Budget for two (the PR run and the post-merge run).

- [ ] **Step 1: Push the branch**

```bash
git push -u origin feature/sma-520-cut-actions-spend
```

The pre-push hook enforces a `^feature/` branch name — it will pass.

- [ ] **Step 2: Open the PR** (see Stage 5 / `feature-factory:open-pr`), then confirm prebuild fired

```bash
gh pr checks --watch
gh run list --workflow=prebuild.yml --branch feature/sma-520-cut-actions-spend --limit 3
```

Expected: a `prebuild` run exists for the PR. **If no run appears, the `pull_request` paths filter is malformed — stop and fix Task 1 before merging.** Fallback to force a run: `gh workflow run prebuild.yml --ref feature/sma-520-cut-actions-spend`.

- [ ] **Step 3: Confirm the job shape**

```bash
RUN=$(gh run list --workflow=prebuild.yml --branch feature/sma-520-cut-actions-spend --limit 1 --json databaseId --jq '.[0].databaseId')
gh run view "$RUN" --json jobs --jq '.jobs[].name'
```

Expected: 7 names — six `build …` jobs (one reading `build darwin-arm64 + darwin-x64`) plus `assemble + verify (dry-run, no publish)`. **No job name may contain `macos-15-intel`.**

- [ ] **Step 4: Confirm the architecture assertions actually ran and passed**

```bash
gh run view "$RUN" --log | grep -E "darwin-(arm64|x64)\s+archs|LC_BUILD_VERSION|minos|::error"
```

Expected: `darwin-arm64 archs: [arm64]` and `darwin-x64 archs: [x86_64]`, plus the `LC_BUILD_VERSION`/`minos` lines. Record the `minos` value in the PR description. Any `::error` line is a hard stop.

- [ ] **Step 5: Confirm all 7 artifacts exist**

```bash
gh run view "$RUN" --json jobs --jq '.jobs[].name' >/dev/null
gh api "repos/SMK1085/paigasus-core/actions/runs/$RUN/artifacts" --jq '.artifacts[].name' | sort
```

Expected exactly: `prebuild-darwin-arm64`, `prebuild-darwin-x64`, `prebuild-linux-arm64-gnu`, `prebuild-linux-arm64-musl`, `prebuild-linux-x64-gnu`, `prebuild-linux-x64-musl`, `prebuild-win32-x64-msvc`.

- [ ] **Step 6: Record the darwin job's wall-clock**

```bash
gh run view "$RUN" --json jobs --jq '.jobs[] | select(.name | startswith("build darwin")) | {name, startedAt, completedAt}'
```

Put the duration in the PR description. The spec requires a before/after record so a cache regression is visible rather than inferred. Compare against a recent pre-change run: `gh run list --workflow=prebuild.yml --branch main --limit 5`.

- [ ] **Step 7: Confirm `assemble` still passes**

Expected: the `assemble + verify (dry-run, no publish)` job is green — it proves the two darwin uploads kept names its `pattern: prebuild-*` download matches, and that the linux-x64-gnu FFI load still works.

- [ ] **Step 8: After merge — the positive path test**

This PR's merge commit touches `.github/workflows/prebuild.yml`, which is on the **push** allowlist, so prebuild **must** run on `main`:

```bash
gh run list --workflow=prebuild.yml --branch main --limit 3
```

Expected: a run for the merge commit. **If none appears, the `push` paths filter is malformed and prebuild will never run again — revert immediately.** This is the only positive test of the push filter; the negative case (a docs-only merge showing no run) is indistinguishable from a broken filter.

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
| -- | -- |
| W2 merged darwin job, step order, `--cwd` | 2 |
| W2 architecture assertion (exact equality, macOS guard, `otool`) | 2 step 6 |
| W2 cache re-key | 2 step 3 |
| W2 `timeout-minutes` 30→45 | 2 step 2 |
| W2 orphaned x64 cache entry cleanup | 4 (runbook §6b) |
| W3 asymmetric push/PR paths | 1 |
| W3 concurrency group + cancel semantics | 1 step 3 |
| W3 rejected matrix tiering | 4 (runbook §8, conditional on the flip) |
| `.gitignore` hardening | 3 |
| W1 runbook, all pre-flights, sequence, rollback | 4 |
| SMA-428 supersession note | 5 |
| Merge-commit positive test | 6 step 8 |
| Dispatch/PR verification | 6 steps 2-7 |
| `ci.yml` untouched | Global Constraints + runbook §6a |

No gaps.

**Placeholder scan:** none — every step carries the literal YAML, command, or prose to write.

**Type consistency:** `extra_platform`/`extra_target` are the only new matrix keys and are spelled identically in Task 2 steps 1, 2, 4, 5, 6, 7 and in Task 6's expectations. Artifact names `prebuild-<platform>` match `assemble`'s existing `pattern: prebuild-*`. The `dual` cache segment appears in both `key:` and `restore-keys:`.

**Known risk carried into execution:** `timeout-minutes: ${{ … }}` uses an expression. Task 2 step 9 explicitly checks actionlint's verdict and gives the literal-45 fallback.
