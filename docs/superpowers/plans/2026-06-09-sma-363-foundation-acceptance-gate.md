# SMA-363 Foundation Acceptance Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute the SMA-363 verification gate against current `main`, producing per-AC evidence, `docs/dev-setup.md`, a Linear evidence comment with ticked checkboxes, and one PR.

**Architecture:** Sequential manual run-through per the approved spec
(`docs/superpowers/specs/2026-06-09-sma-363-foundation-acceptance-gate-design.md`):
a fresh temp-dir clone serves Stages 1–3 (build, toolchains, hooks, affected-graph,
CODEOWNERS), then GitHub-side checks (CI parity, branch protection, Dependabot) run
via `gh`. Evidence accumulates in `/tmp/sma-363-evidence.md` and feeds the final doc
and Linear comment. **No fixes land in this branch** — defects route per the spec's
three dispositions (originating issue / standalone docs PR / new issue).

**Tech Stack:** Moon 2.2.5 (proto-pinned), `gh` CLI, Linear MCP tools.

**Execution notes (read first):**

- Every shell needs proto on PATH: `export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"`.
  macOS has no `timeout` command — don't use it.
- Moon task output is buffered (`buffer-only-failure`); passing tasks print summaries
  only. That is expected, not a hang.
- `moon ci` ALWAYS needs explicit targets in non-TTY (`app::tty::required_id` otherwise).
- The cross-language cascade is **out of scope** (deferred to SMA-409). Do not "fix"
  missing `dependsOn` edges if the affected sets look sparse — sparse is expected.
- If any verification FAILS: record it in the evidence file, do NOT patch it in this
  branch, and stop to report — the user decides the disposition per the spec.

---

### Task 1: Preflight and evidence scaffold

**Files:**
- Create: `/tmp/sma-363-evidence.md` (scratch, never committed)

- [ ] **Step 1: Verify environment**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
git branch --show-current   # expect: feature/sma-363-foundation-acceptance-gate
git status --short          # expect: empty (clean tree)
gh auth status              # expect: Logged in to github.com
moon --version              # expect: 2.2.5
```

If the branch is wrong: `git checkout feature/sma-363-foundation-acceptance-gate`.

- [ ] **Step 2: Create the evidence file**

```bash
cat > /tmp/sma-363-evidence.md <<'EOF'
# SMA-363 evidence — run date 2026-06-09
Repo: SMK1085/paigasus-core   Gate branch: feature/sma-363-foundation-acceptance-gate
Format per AC: PASS/FAIL/DEFERRED — command — pointer/output snippet
EOF
```

Append a dated entry to this file after EVERY verification step in Tasks 2–11.

### Task 2: Fresh temp-dir clone with materialized main

**Files:**
- Create: temp clone at `$CLONE` (outside the repo, never committed)

- [ ] **Step 1: Clone and start the setup timer**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
export CLONE_ROOT=$(mktemp -d /tmp/sma363.XXXXXX)
export CLONE="$CLONE_ROOT/paigasus-core"
echo "CLONE=$CLONE" >> /tmp/sma-363-evidence.md
SETUP_START=$(date +%s); echo "SETUP_START=$SETUP_START" >> /tmp/sma-363-evidence.md
git clone git@github.com:SMK1085/paigasus-core.git "$CLONE"
```

Expected: clone succeeds; default branch `main` checked out. (Full clone — not
shallow — so Moon's affected-graph has history.)

`$CLONE` and `$SETUP_START` are used by Tasks 2–7. If executing tasks in separate
sessions, re-read `CLONE=` from `/tmp/sma-363-evidence.md` and re-export.

- [ ] **Step 2: Materialize origin/main explicitly (mirrors ci.yml "Materialize main ref")**

```bash
cd "$CLONE"
git fetch --no-tags origin "+refs/heads/main:refs/remotes/origin/main"
git rev-parse origin/main   # expect: same SHA as main's HEAD
```

- [ ] **Step 3: Record evidence**

Append to `/tmp/sma-363-evidence.md`: clone command, HEAD SHA (`git rev-parse HEAD`),
confirmation that `origin/main` resolves.

### Task 3: Toolchain install + hooks + JS deps (CONTRIBUTING verbatim)

This follows CONTRIBUTING.md "Local development" exactly — deviations from the doc
are findings for Task 11, not things to silently work around.

- [ ] **Step 1: proto install (pinned CLIs: moon, buf, lefthook, cargo-deny, cargo-machete, cargo-nextest)**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd "$CLONE"
proto install
moon --version   # expect: 2.2.5
buf --version    # expect: pinned version from .prototools
```

Expected: all pins resolve and install with no errors. (Warm-cache caveat applies —
note it in evidence.)

- [ ] **Step 2: Install git hooks (CONTRIBUTING order: proto install BEFORE workspace deps)**

```bash
moon run repo:install-hooks
ls "$CLONE/.git/hooks/commit-msg" "$CLONE/.git/hooks/pre-push"   # expect: both exist
```

- [ ] **Step 3: Install JS workspace deps (mirrors ci.yml)**

```bash
pnpm --dir ts install --frozen-lockfile
```

Expected: exits 0; lockfile not modified (`git status --short` clean apart from
nothing — node_modules is ignored).

- [ ] **Step 4: Moon toolchain bootstrap check**

```bash
moon setup
```

Expected: Moon installs/links node, pnpm, rust, python, uv per `.moon/toolchains.yml`
with no manual installs. Record any prompt or failure verbatim in evidence (AC:
"Toolchain installation").

### Task 4: Whole-graph build/test + warning-free surface + `moon ci` resolution

Covers ACs "Fresh-clone build", "Cross-language tasks", and the setup-time
observation.

- [ ] **Step 1: Whole-graph build and test (the meaningful fresh-clone check)**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd "$CLONE"
moon run :build :test
```

Expected: all projects' build/test tasks pass. Empty test sets are fine
(`cargo nextest` runs with `--no-tests=pass` per task config).

- [ ] **Step 2: Stop the setup timer (clone → green build) and record the observation**

```bash
SETUP_END=$(date +%s)
echo "Setup wall-clock: $((SETUP_END - SETUP_START)) seconds (warm proto/tool caches — observation, NOT a pass/fail gate)" >> /tmp/sma-363-evidence.md
```

- [ ] **Step 3: Warning-free surface (objective definition from the spec)**

```bash
moon run :lint :fmt :typecheck repo:deny repo:machete
```

Expected: all pass — this is clippy `-D warnings`, rustfmt, ruff lint+format,
basedpyright, ESLint, Prettier, tsc, cargo-deny, cargo-machete. Any warning that
fails a task = AC failure; record verbatim.

- [ ] **Step 4: `moon ci` resolution check (AC wording)**

```bash
moon ci :build --base origin/main
moon ci :test --base origin/main
```

Expected: exit 0. On an untouched clone the affected set is empty — the check here
is that targets RESOLVE and the command succeeds in non-TTY, not that tasks run
(Step 1 already ran them all).

- [ ] **Step 5: Record evidence for all three ACs**

### Task 5: lefthook hook-fire check

- [ ] **Step 1: Attempt a commit with a non-Conventional subject**

```bash
cd "$CLONE"
git checkout -b feature/sma-363-hook-probe
echo "probe" > /tmp/hook-probe.txt && cp /tmp/hook-probe.txt .
git add hook-probe.txt
git commit -m "this subject violates conventional commits" 2>&1 | tee /tmp/sma363-hook-out.txt
echo "exit=$?"
```

Expected: commit REJECTED — lefthook `commit-msg` panel, commitlint
`subject may not be empty`/`type may not be empty` errors, non-zero exit. If the
commit SUCCEEDS, that is an AC failure (hooks not firing) — record and stop.

- [ ] **Step 2: Clean up the probe**

```bash
git reset --hard HEAD && rm -f hook-probe.txt
git checkout main && git branch -D feature/sma-363-hook-probe
git status --short   # expect: empty — a leftover untracked probe file would pollute Task 6's affected queries
```

- [ ] **Step 3: Record evidence** (paste the lefthook/commitlint output snippet)

### Task 6: Affected-graph resolution matrix

One scratch branch per case in `$CLONE`. For each: touch → query affected (working
tree) → cross-check task selection via `moon ci :build --base origin/main` on the
committed change → reset. The **expected sets** below are the pass criteria; extra
*cross-stack language projects* are failures, but `repo` gate tasks whose declared
inputs match the touched file are expected (root `moon.yml`: `machete` inputs
include `rs/**/*.rs`).

- [ ] **Step 1: Case rs — kernel edit**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd "$CLONE"
git checkout -b probe/affected-rs main
echo "// SPDX probe comment" >> rs/crates/libs/paigasus-kernel/src/lib.rs
moon query projects --affected --downstream deep
git commit -am "test: probe" --no-verify
moon ci :build --base origin/main 2>&1 | tee /tmp/sma363-affected-rs.txt
git checkout main && git branch -D probe/affected-rs
```

Expected affected projects: `paigasus-kernel-rs`, `paigasus-gateway-rs` (its one
declared consumer), `repo` (machete inputs match `rs/**/*.rs`). NO py/ts/contracts
projects. The cascade to py-bindings/wrappers is DEFERRED (SMA-409) — its absence
is expected, not a failure.

- [ ] **Step 2: Case py — paigasus-ml edit**

```bash
git checkout -b probe/affected-py main
PYFILE=$(find py/packages/paigasus-ml/src -name '*.py' | head -1)
echo "# probe comment" >> "$PYFILE"
moon query projects --affected --downstream deep
git commit -am "test: probe" --no-verify
moon ci :build :lint :fmt --base origin/main 2>&1 | tee /tmp/sma363-affected-py.txt
git checkout main && git branch -D probe/affected-py
```

Expected: `paigasus-ml-py` + root `py` project (whole-tree lint/fmt — its
`@group(sources)` covers `packages/*/src/**`). NO rs/ts/contracts projects.
**Dedup invariant (SMA-401):** in the `moon ci` output each root `py:` whole-tree
task appears EXACTLY once — not zero, not twice.

- [ ] **Step 3: Case ts — paigasus-sdk edit**

```bash
git checkout -b probe/affected-ts main
TSFILE=$(find ts/packages/paigasus-sdk/src -name '*.ts' | head -1)
echo "// probe comment" >> "$TSFILE"
moon query projects --affected --downstream deep
git commit -am "test: probe" --no-verify
moon ci :build :lint :fmt --base origin/main 2>&1 | tee /tmp/sma363-affected-ts.txt
git checkout main && git branch -D probe/affected-ts
```

Expected: `paigasus-sdk-ts` + root `ts` project whole-tree tasks, exactly once each.
NO rs/py/contracts projects. (Note actual path is `ts/packages/paigasus-sdk` — the
original AC's `ts/packages/sdk` was naming drift.)

- [ ] **Step 4: Case contracts — proto edit**

```bash
git checkout -b probe/affected-proto main
PROTOFILE=$(find contracts/proto -name '*.proto' | head -1)
printf '\n// probe comment\n' >> "$PROTOFILE"
moon query projects --affected --downstream deep
git commit -am "test: probe" --no-verify
moon ci :build :breaking --base origin/main 2>&1 | tee /tmp/sma363-affected-proto.txt
git checkout main && git branch -D probe/affected-proto
```

Expected: `contracts` project only; the `contracts:breaking` task EXECUTES (passes
against `main` — a trailing comment is not a breaking change) rather than skipping.
Downstream language rebuilds: DEFERRED to SMA-409, absence expected.

- [ ] **Step 5: Record evidence** — per case: touched file, affected project list,
      task list, dedup observation, explicit "cascade deferred" notes for rs/proto.

### Task 7: CODEOWNERS sync check

- [ ] **Step 1: Manual edit gets overwritten by explicit sync**

```bash
cd "$CLONE"
echo "# manual-edit-probe" >> .github/CODEOWNERS
moon sync code-owners
grep -c "manual-edit-probe" .github/CODEOWNERS || echo "probe removed"
git diff --exit-code .github/CODEOWNERS && echo "drift gate would PASS"
```

Expected: probe line gone after sync (`probe removed`), `git diff --exit-code`
clean → file fully Moon-owned; the CI drift gate (`moon sync code-owners` +
`git diff --exit-code`) catches stale edits. If the probe SURVIVES sync, AC failure.

- [ ] **Step 2: Restore and record**

```bash
git checkout -- .github/CODEOWNERS 2>/dev/null || true
git status --short   # expect clean
```

Append evidence with the exact wording from the spec: "explicit `moon sync
code-owners` regenerates; CI drift gate fails on stale CODEOWNERS" (NOT "auto-syncs
on every Moon run").

### Task 8: CI parity at the latest green main run

- [ ] **Step 1: Identify the run and its base**

```bash
gh run list --repo SMK1085/paigasus-core --branch main --workflow CI --status success --limit 1 \
  --json databaseId,headSha --jq '.[0]'
```

Record `databaseId` as `RUN_ID`, `headSha` as `HEAD_SHA`. The push-path run used
`--base $BEFORE` where BEFORE is the previous main head; for squash-merge history
that is `HEAD_SHA^`:

```bash
cd "$CLONE"
git checkout "$HEAD_SHA"   # likely == main HEAD
BASE_SHA=$(git rev-parse "$HEAD_SHA^")
```

- [ ] **Step 2: Reproduce the full CI surface locally (same SHA, same base, same targets)**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :release-parity :release-parity-py :release-parity-ts --base "$BASE_SHA" 2>&1 | tee /tmp/sma363-parity-local.txt
moon run ts:check-config-only
moon sync code-owners && git diff --exit-code .github/CODEOWNERS
git checkout main
```

Expected: exit 0 on all three, matching the green CI run. (`ts:commitlint` needs a
PR base/head range; CI runs it on pull_request only — record it as covered by the
gate branch's own upcoming PR rather than rerun here.)

- [ ] **Step 3: Diff the resolved task set against the GitHub run log**

```bash
gh run view "$RUN_ID" --repo SMK1085/paigasus-core --log | grep -oE '[a-z0-9-]+:[a-z0-9-]+ \(cached|\bRunning [a-z0-9-]+:[a-z0-9-]+' | sort -u > /tmp/sma363-parity-ci-tasks.txt
```

Compare the task list in `/tmp/sma363-parity-local.txt` against CI's. Expected: same
task set, same pass results (cache hits may differ — that's fine; selection and
outcomes must match). Record both lists in evidence.

### Task 9: Branch protection

- [ ] **Step 1: Read the active rules for main**

```bash
gh api repos/SMK1085/paigasus-core/rules/branches/main --jq '.'
gh api repos/SMK1085/paigasus-core/branches/main/protection --jq '.' 2>/dev/null || echo "no classic protection (rulesets only)"
```

Expected assertions (record the raw JSON in evidence):
1. A `pull_request` rule exists (PRs required to reach main).
2. A `required_status_checks` rule lists context EXACTLY `CI / moon ci` (workflow
   `name: CI`, job `name: moon ci`). Any other string (old `moon check`, bare `ci`)
   = AC FAILURE — stale required-check name.
3. The configuration blocks merge when the check is ABSENT (rulesets:
   required_status_checks blocks until the named check reports; note the
   `do_not_enforce_on_create` / integration_id fields as found).

- [ ] **Step 2: Empirical evidence**

```bash
gh pr view 32 --repo SMK1085/paigasus-core --json statusCheckRollup,mergedAt --jq '{mergedAt, checks: [.statusCheckRollup[].name]}'
```

Expected: PR #32 (latest merged) shows `moon ci` in its checks and merged only after
green. Record.

### Task 10: Dependabot evidence

- [ ] **Step 1: Cite the merged Dependabot PRs**

```bash
for n in 24 25 26 30; do
  gh pr view $n --repo SMK1085/paigasus-core --json number,title,author,mergedAt,labels --jq '{number,title,author: .author.login,mergedAt}'
done
```

Expected: authors `app/dependabot`; #26 (`uv-minor-patch` group) and #30
(`npm-minor-patch` group) demonstrate GROUPED batches; #24/#25 are the
github-actions ecosystem bumps; all merged ⇒ passed required CI. Record as the AC
evidence — no test trigger needed. (PR #23 was the config PR, not Dependabot
output — don't cite it as batch evidence.)

### Task 11: Documentation accuracy reconciliation

- [ ] **Step 1: Compile the doc-drift list**

Review the notes accumulated in `/tmp/sma-363-evidence.md` from Tasks 2–7: every
point where README.md or CONTRIBUTING.md said something that did not match reality
(wrong path, missing step, wrong command, stale reference). Cross-check explicitly:

```bash
cd "$CLONE"
grep -n "moon " README.md CONTRIBUTING.md | head -40
ls contracts rs py ts   # compare against README's layout description
```

- [ ] **Step 2: Disposition per the spec**

- Zero drift → record "Documentation accuracy: PASS" with the claims checked.
- Drift found → record each item as FAIL evidence; queue a SEPARATE standalone docs
  PR (disposition 2) — do NOT fix in the gate branch. Stop and report the list to
  the user before opening that PR.

### Task 12: Write `docs/dev-setup.md`

**Files:**
- Create: `docs/dev-setup.md` (in the MAIN working copy, on the gate branch)

- [ ] **Step 1: Write the doc from the evidence file (not from existing docs)**

Structure (fill every section from `/tmp/sma-363-evidence.md` actuals — real
commands run, real timings, real output; no aspirational content):

```markdown
# Dev setup — verified end-to-end (SMA-363)

What a fresh clone actually required on 2026-06-09, executed for the foundation
acceptance gate. Canonical setup lives in [CONTRIBUTING.md](../CONTRIBUTING.md#local-development);
this records the verified path and timings.

## Prerequisites (OS-level)
<the actual minimal set: git, proto, SSH access; per the verified run>

## Verified sequence
<the exact commands from Tasks 2–4, in order, with the observed result of each>

## Observed timing
Clone → green `moon run :build :test`: <N> seconds on macOS (Darwin 25.5.0) with a
WARM proto/tool cache. This is a recorded observation, not a guarantee — a cold
machine pays toolchain download costs CI absorbs via caches.

## Gotchas (verified)
- `moon ci` requires explicit targets in non-TTY environments (Moon 2.2.5).
- `cargo nextest` on a no-test workspace needs `--no-tests=pass` (already in task config).
- proto shims live at `~/.proto/bin` and `~/.proto/shims` — GUI clients and CI shells
  may need them added to PATH explicitly.
- Affected-graph runs (`moon ci --base …`, `contracts:breaking`) need `origin/main`
  materialized with full history.
<plus anything new the run surfaced>
```

- [ ] **Step 2: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git add docs/dev-setup.md
git commit -m "docs(repo): add verified dev-setup guide from SMA-363 gate run

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

Expected: commitlint passes (`docs(repo)` scope is valid).

### Task 13: Linear evidence comment + AC checkboxes

- [ ] **Step 1: Post the evidence comment on SMA-363**

Use the Linear MCP `save_comment` tool (issue `SMA-363`). Body: one line per AC in
the amended list — `PASS`/`FAIL`/`DEFERRED (SMA-409)` — with the command run and the
pointer (output snippet, CI run URL `https://github.com/SMK1085/paigasus-core/actions/runs/<RUN_ID>`,
PR numbers). Source everything from `/tmp/sma-363-evidence.md`.

- [ ] **Step 2: Tick the checkboxes**

Use Linear MCP `save_issue` (id `SMA-363`): rewrite the description with `- [x]` for
every AC that passed (the cascade sub-bullet stays as part of the affected-graph AC,
marked deferred inline — it already says so). Leave any failed AC unticked.

### Task 14: PR and cleanup

- [ ] **Step 1: Push and open the PR**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git push -u origin feature/sma-363-foundation-acceptance-gate
gh pr create --title "docs(repo): foundation acceptance gate evidence + dev-setup (SMA-363)" --body "## Summary
- Executes the SMA-363 foundation acceptance gate against current main
- Adds docs/dev-setup.md written from the verified fresh-clone run
- Spec + plan under docs/superpowers/ ; per-AC evidence on the Linear issue
- Cross-language cascade explicitly deferred to SMA-409 (Phase-2-entry checkpoint)

## Verification
- All amended ACs evidenced on SMA-363 (Linear comment dated 2026-06-09)

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

Expected: PR opens; Linear auto-links via branch name (do NOT attach links
manually); CI goes green — which is itself the final live demonstration of the
commitlint gate, branch protection, and `moon ci` PR path.

- [ ] **Step 2: Remove the temp clone**

```bash
rm -rf "$CLONE_ROOT"
```

- [ ] **Step 3: Report** — summarize pass/fail/deferred per AC to the user; SMA-363
      moves to Done only after the PR merges and the user confirms.
