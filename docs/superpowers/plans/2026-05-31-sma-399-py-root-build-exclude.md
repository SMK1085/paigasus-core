# SMA-399 — Exclude the inherited `build` at the py root Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the `py` Moon root from running the inherited `uv build`, which emits a junk `unknown-0.0.0` wheel + `packages.egg-info/` at the virtual uv workspace root, while keeping every real `py/packages/*` build and the root's whole-tree `typecheck`/`lint`/`fmt`/`test`.

**Architecture:** Add `workspace.inheritedTasks.exclude: ['build']` to `py/moon.yml` (the same idiom `ts/moon.yml` and `commitlint-config-ts` already use), removing the inherited `build` task from the `py` root project only. Add a comment-only back-reference in `ts/moon.yml` so the deliberate `ts(['build','typecheck'])` vs `py(['build'])` divergence reads as intentional. No task-graph change beyond dropping the junk root build.

**Tech Stack:** Moon 2.2.5 (project config YAML, `workspace.inheritedTasks.exclude`), uv (uv_build backend), basedpyright.

**Spec:** `docs/superpowers/specs/2026-05-31-sma-399-py-root-build-exclude-design.md`

---

## Pre-flight: environment

`moon` is proto-managed and is **not** on a non-interactive shell's `PATH`. If a `moon: command not found` error appears, prefix the shell with:

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
```

All `moon …` commands below assume `moon` resolves (interactive shells already have it).

There are no traditional unit tests for a Moon config change — **the spec's verification commands are the test suite.** Each task captures the failing/baseline state first (red), makes the change, then asserts the desired state (green).

## File Structure

- `py/moon.yml` — the functional change. Gains a `workspace.inheritedTasks.exclude: ['build']` stanza with an explanatory comment. Keeps `$schema`, `layer`, `language`, `fileGroups`. (Currently ends at the `tests:` fileGroup; the new `workspace:` block is appended last per the CONTRIBUTING field-order rule.)
- `ts/moon.yml` — comment-only. A one-line back-reference added inside the existing exclude comment block, just before its `workspace:` stanza. No behavior change.

---

### Task 1: Exclude the inherited `build` at the py root

**Files:**
- Modify: `py/moon.yml` (append a `workspace:` block after the `fileGroups:` block, currently ~line 16)

- [ ] **Step 1: Capture the red baseline — prove the junk build exists today**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/py
moon project py | grep -iE '^build|build ' || moon project py
moon run py:build --force 2>&1 | grep -iE 'workspace root without a Python project|unknown-0.0.0|packages-0.0.0'
ls dist | grep -iE 'unknown-0.0.0|packages-0.0.0'
```
Expected (the bug): `moon project py` lists a `build` task; `py:build` prints the `appears to be a workspace root without a Python project` warning; `dist/` contains `unknown-0.0.0-py3-none-any.whl` and `packages-0.0.0.tar.gz`, and `py/packages.egg-info/` exists.

- [ ] **Step 2: Make the change — add the exclude stanza to `py/moon.yml`**

Append this block to the end of `py/moon.yml` (after the `fileGroups:` `tests:` list). The file currently ends with:

```yaml
fileGroups:
  sources:
    - 'packages/*/src/**/*'
  tests:
    - 'packages/*/tests/**/*'
```

Add immediately after it:

```yaml

# The py root owns no build of its own: Moon's per-project fan-out owns the whole :build
# graph — each py/packages/* inherits `uv build` and emits a real wheel via the uv_build
# backend. We EXCLUDE (not merely omit) the inherited build here, because py/pyproject.toml
# is a virtual uv workspace root ([tool.uv.workspace], NO [project] table); `uv build` there
# falls back to legacy setuptools and emits a junk UNKNOWN-0.0.0 wheel + packages.egg-info/.
# Unlike ts/ (SMA-394, which excludes typecheck too), typecheck is KEPT here: `uv run
# basedpyright` from py/ runs clean (it reads the central [tool.basedpyright] config), so it has
# none of the root problem `build` has. It stays inherited alongside the whole-tree lint/fmt/test
# for consistency — not as a uniquely necessary pass (per-package typecheck already covers the
# same configured tree; the N+1 whole-tree redundancy is tracked in SMA-401).
workspace:
  inheritedTasks:
    exclude: ['build']
```

- [ ] **Step 3: Assert green — the root build task is gone and emits no junk**

First clear the stale local junk so the assertion is honest, then re-run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
rm -rf py/dist py/packages.egg-info
moon project py | grep -iE 'build' || echo "no build task on py root ✓"
moon run py:build 2>&1 | grep -iE 'unknown task|has not been configured|No such task' && echo "py:build is unknown ✓"
moon run :build --force 2>&1 | tail -20
ls py/dist 2>/dev/null | grep -iE 'unknown-0.0.0|packages-0.0.0' && echo "JUNK STILL PRESENT ✗" || echo "no junk wheel ✓"
test ! -e py/packages.egg-info && echo "no egg-info ✓"
```
Expected:
- `moon project py` no longer lists a `build` task (prints `no build task on py root ✓`).
- `moon run py:build` reports the target is unknown/unconfigured (`py:build is unknown ✓`).
- `moon run :build --force` finishes with **0 failed**; only `paigasus-kernel-py:build`, `paigasus-ml-py:build`, `paigasus-proto-py:build`, `paigasus-workflows-py:build` (and the non-py builds) run — **no** `py:build` line.
- `py/dist/` has the real `paigasus_*-0.0.0-*` wheels but **no** `unknown-0.0.0` / `packages-0.0.0`; `py/packages.egg-info/` is gone.

- [ ] **Step 4: Assert green — affected-graph CI form still covers every py package**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
moon ci :build --base origin/main 2>&1 | tail -25
```
Expected: the resolved targets include each `paigasus-*-py:build`; **no** root `py:build`. Exit 0. (If "No tasks affected" appears because nothing under `py/` changed relative to `origin/main` except this `moon.yml`, that is fine — the point is no error and no `py:build` target.)

- [ ] **Step 5: Assert green — the KEPT typecheck still passes (load-bearing half of the decision)**

Run:
```bash
moon run py:typecheck --force 2>&1 | tail -5
```
Expected: `py:typecheck | 0 errors, 0 warnings, 0 notes`, task succeeds. (Confirms excluding `build` did not disturb the inherited whole-tree `typecheck` at the root.)

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git add py/moon.yml
git commit -m "fix(py): exclude inherited build at the py root so it stops emitting a junk UNKNOWN wheel (SMA-399)"
```
(The lefthook `commit-msg` hook runs commitlint; expect `✔️ commitlint`.)

---

### Task 2: Cross-reference the deliberate ts/py divergence in `ts/moon.yml`

**Files:**
- Modify: `ts/moon.yml` (comment-only; insert a back-reference inside the existing exclude comment block, just before its `workspace:` stanza)

- [ ] **Step 1: Make the change — add the back-reference comment**

In `ts/moon.yml`, find the end of the exclude comment block and the `workspace:` line. It currently reads:

```yaml
# commitlint-config (SMA-395), which forward-referenced this change.
workspace:
  inheritedTasks:
    exclude: ['build', 'typecheck']
```

Change it to insert the back-reference comment before `workspace:`:

```yaml
# commitlint-config (SMA-395), which forward-referenced this change.
#
# (SMA-399: py/moon.yml deliberately excludes only ['build'] — basedpyright reads a central
#  config and runs fine at the py root, whereas tsc needs a root tsconfig.json this dir lacks.)
workspace:
  inheritedTasks:
    exclude: ['build', 'typecheck']
```

- [ ] **Step 2: Assert green — ts task graph is unchanged (comment-only)**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
moon project ts | grep -iE 'build|typecheck' || echo "ts root still excludes build+typecheck ✓"
moon run :build --query "language=typescript" --force 2>&1 | tail -15
```
Expected: `moon project ts` still lists **no** `build` and **no** `typecheck` (prints the `✓` line); the TS build graph runs with **0 failed** (`paigasus-console-ts:build` once, libs' no-op `tsc --noEmit`). The comment edit changed nothing functional.

- [ ] **Step 3: Commit**

```bash
git add ts/moon.yml
git commit -m "docs(ts): note that py/moon.yml deliberately excludes only build, not typecheck (SMA-399)"
```
(Expect `✔️ commitlint`.)

---

### Task 3: Whole-graph green check + open the PR

**Files:** none (verification + PR only)

- [ ] **Step 1: Final whole-graph build is green**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run :build --force 2>&1 | tail -15
ls py/dist 2>/dev/null | grep -iE 'unknown-0.0.0|packages-0.0.0' && echo "JUNK ✗" || echo "clean ✓"
```
Expected: 0 failed across the whole repo; `clean ✓` (no junk wheel regenerated).

- [ ] **Step 2: Confirm the working tree is clean and the branch is ready**

```bash
git status --short    # expect: empty (py/dist + packages.egg-info are gitignored)
git log --oneline origin/main..HEAD
```
Expected: clean working tree; the log shows the two new `fix(py)` / `docs(ts)` commits on top of the spec commits.

- [ ] **Step 3: Push and open the PR**

```bash
git push -u origin feature/sma-399-py-py-root-build-emits-junk-unknown-wheel-exclude-inherited
gh pr create --base main --fill
```
Do **not** attach the Linear link manually — the Linear↔GitHub integration auto-links by branch name. Title should follow conventional commits, e.g. `fix(py): exclude inherited build at the py root (SMA-399)`. After the PR opens, CI (`moon ci`) runs the affected graph; expect it green.

---

## Self-Review

**1. Spec coverage** — every spec section maps to a task:
- Decision (`py/moon.yml` exclude `['build']` + comment) → Task 1, Step 2.
- Cross-file legibility / F4 (`ts/moon.yml` back-reference) → Task 2.
- Verification plan steps 1–5 → Task 1 Steps 1,3,4,5 + Task 2 Step 2 + Task 3 Step 1.
- AC "moon run py:build no longer runs uv build / no junk" → Task 1 Steps 3.
- AC "moon ci :build still covers every py/packages/* project" → Task 1 Step 4.
- AC "whole-graph moon run :build stays green" → Task 3 Step 1.
- Out-of-scope N+1 redundancy → not implemented here by design; referenced in the `py/moon.yml` comment as SMA-401.
- "No README fallout" → confirmed in spec; no doc task needed.

**2. Placeholder scan** — no TBD/TODO; every code/YAML block is the literal content to write; every command has expected output.

**3. Type/identifier consistency** — task target names (`py:build`, `py:typecheck`, `paigasus-*-py:build`), the field `workspace.inheritedTasks.exclude`, and the exclude value `['build']` are consistent across all tasks and match the spec end-state and the live `.moon/tasks/python.yml` task names.
