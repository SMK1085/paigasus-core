# SMA-391 — Fix `ts:build` double-building `paigasus-console` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `moon ci :build` from building `paigasus-console` twice into the same `.next` (Next.js 16 build-lock collision) by scoping the recursive `ts:build` task to library packages only.

**Architecture:** Single-line behavior change to one Moon task in `ts/moon.yml`. The recursive `ts:build` currently runs `pnpm -r --if-present run build`, which recurses into `apps/paigasus-console` and runs its `next build` concurrently with the app's own `paigasus-console-ts:build` Moon task — both writing `.next`. Scoping the recursion to `./packages/**` (with `--if-present`) removes the duplicate entirely; apps build only via their own per-app Moon tasks.

**Tech Stack:** Moon 2.2.5, pnpm workspace, Next.js 16, TypeScript.

**Spec:** `docs/superpowers/specs/2026-05-29-sma-391-ts-build-double-build-design.md`

**Branch:** `feature/sma-391-tsbuild-double-builds-paigasus-console-concurrently-nextjs` (already checked out)

---

## Context the implementer needs

- This is a **config change, not code** — there is no unit-test harness for `moon.yml`. The "test" is a behavioral verification: a cold full `moon ci :build` must complete with the console built exactly once and no `.next` lock error.
- The deterministic single-task repro is `moon run ts:build paigasus-console-ts:build --force` on a cold `.next`. Forcing both tasks concurrently is what triggers the lock; it is reliable but, being a race, may occasionally not collide on a given run — so the authoritative green signal is **(a)** the cold full `moon ci :build` passing **and** **(b)** the console appearing as a build target exactly once.
- `--if-present` is **mandatory**: with no `packages/*` defining a `build` script today, `pnpm --filter "./packages/**" run build` exits 1 with `ERR_PNPM_RECURSIVE_RUN_NO_SCRIPT`. Verified during design.
- Only `apps/paigasus-console/package.json` has a `build` script in the whole TS workspace. Library packages are type-checked via their inherited per-project Moon `build` task (`tsc --noEmit`), not via this recursive task.
- Run all `moon`/`pnpm` commands from the repo root unless noted; `pnpm` filter commands run from `ts/`.

---

### Task 1: Reproduce the collision (establish the "red" state)

**Files:** none (observation only)

- [ ] **Step 1: Clear the console build cache and Moon cache for a cold run**

```bash
rm -rf ts/apps/paigasus-console/.next
```

- [ ] **Step 2: Run the two builders forced + concurrent to trigger the lock**

Run from repo root:

```bash
moon run ts:build paigasus-console-ts:build --force 2>&1 | tee /tmp/sma391-before.log; echo "exit: ${PIPESTATUS[0]}"
```

Expected: a **non-zero exit**, with the log containing either `Another next build process is already running` or `ERR_PNPM_RECURSIVE_RUN_FIRST_FAIL` / `task_runner::run_failed` for `ts:build`.

- [ ] **Step 3: If it did NOT collide, retry up to 3 times**

The collision is a cold-cache race. If Step 2 passed, re-run:

```bash
rm -rf ts/apps/paigasus-console/.next && moon run ts:build paigasus-console-ts:build --force 2>&1 | tee /tmp/sma391-before.log; echo "exit: ${PIPESTATUS[0]}"
```

Repeat up to 3 times. If it still never collides, that's acceptable — proceed; the fix is validated by Tasks 3–4 regardless (the structural duplicate is removed either way). Note in the eventual commit/PR that the race did not reproduce locally.

- [ ] **Step 4: Confirm the duplicate exists structurally (independent of the race)**

```bash
cd ts && pnpm -r --if-present run build --dry-run 2>&1 | grep -i console || true; cd ..
```

Expected: confirms `pnpm -r` reaches `@paigasus/console`. (If `--dry-run` is unsupported by the script, instead just note that `apps/paigasus-console/package.json` is the sole `build` script — already verified in the spec.)

---

### Task 2: Apply the fix to `ts/moon.yml`

**Files:**
- Modify: `ts/moon.yml` (the `build` task under `tasks:`)

- [ ] **Step 1: Edit the `build` task**

Replace this exact block in `ts/moon.yml`:

```yaml
  build:
    command: 'pnpm -r --if-present run build'
    inputs:
      - '@group(sources)'
      - 'tsconfig.base.json'
      - 'packages/**/tsconfig.json'
      - 'apps/**/tsconfig.json'
      - 'package.json'
      - 'pnpm-workspace.yaml'
      - 'pnpm-lock.yaml'
    options:
      merge: replace
```

with:

```yaml
  build:
    # Library packages only — apps own their build via per-app Moon tasks (see paigasus-console).
    # `typecheck` above intentionally still uses `pnpm -r` over the whole tree: its `tsc --noEmit`
    # writes nothing and holds no lock, so the double-run is harmless. The asymmetry is deliberate;
    # both converge in the structural follow-up (SMA-394). See SMA-391 design doc.
    command: 'pnpm --filter "./packages/**" --if-present run build'
    inputs:
      - '@group(sources)'
      - 'tsconfig.base.json'
      - 'packages/**/tsconfig.json'
      - 'package.json'
      - 'pnpm-workspace.yaml'
      - 'pnpm-lock.yaml'
    options:
      merge: replace
```

Changes: `command` scoped to `./packages/**` + `--if-present`; the `apps/**/tsconfig.json` input line removed (the task no longer touches apps); explanatory comment added. **Do not** touch the `typecheck` task or any other part of the file.

- [ ] **Step 2: Validate the YAML / Moon config parses**

Run from repo root:

```bash
moon project ts 2>&1 | tail -20; echo "exit: ${PIPESTATUS[0]}"
```

Expected: exit 0, and the printed `build` task command shows `pnpm --filter "./packages/**" --if-present run build`. (If `moon project` output doesn't show the command, fall back to `moon task ts:build` or simply confirm exit 0 / no parse error.)

---

### Task 3: Verify the fix — single-task repro + `ts:build` no-ops cleanly (the "green" state)

**Files:** none (verification only)

- [ ] **Step 1: Cold-run the previously-colliding command**

```bash
rm -rf ts/apps/paigasus-console/.next
moon run ts:build paigasus-console-ts:build --force 2>&1 | tee /tmp/sma391-after.log; echo "exit: ${PIPESTATUS[0]}"
```

Expected: **exit 0**. The log must NOT contain `Another next build process is already running` or `task_runner::run_failed`.

- [ ] **Step 2: Confirm `ts:build` no longer builds the console**

```bash
grep -ci "next build\|@paigasus/console" /tmp/sma391-after.log
```

Inspect the log: `ts:build`'s own output must show no `next build` invocation (it should be a clean no-op — no matching `packages/*` build script). The single `next build` present must be attributed to `paigasus-console-ts:build`, not `ts:build`.

- [ ] **Step 3: Confirm `ts:build` alone is a clean no-op**

```bash
moon run ts:build --force 2>&1 | tee /tmp/sma391-tsbuild.log; echo "exit: ${PIPESTATUS[0]}"
```

Expected: exit 0, no `next build`, no `ERR_PNPM_RECURSIVE_RUN_NO_SCRIPT`.

---

### Task 4: Verify the full cold `:build` graph (authoritative AC check)

**Files:** none (verification only)

- [ ] **Step 1: Cold full affected build**

```bash
rm -rf ts/apps/paigasus-console/.next
moon ci :build 2>&1 | tee /tmp/sma391-ci.log; echo "exit: ${PIPESTATUS[0]}"
```

Expected: **exit 0**; summary shows all tasks completed, **0 failed**; no `Another next build process is already running` anywhere in the log.

- [ ] **Step 2: Confirm the console was built exactly once**

```bash
grep -c "next build" /tmp/sma391-ci.log
```

Expected: the console's `next build` appears under exactly one task (`paigasus-console-ts:build`). Confirm `.next` exists:

```bash
test -d ts/apps/paigasus-console/.next && echo ".next present (built once)"
```

- [ ] **Step 3: Confirm library packages still covered**

Inspect `/tmp/sma391-ci.log`: each library project (`paigasus-kernel-ts`, `-sdk`, `-proto`, `-ui`, `-commitlint-config`) shows its own `build` task running (inherited `tsc --noEmit`). This satisfies "library packages remain covered."

---

### Task 5: Commit

**Files:**
- Modify: `ts/moon.yml`

- [ ] **Step 1: Stage and commit the fix**

```bash
git add ts/moon.yml
git commit -m "fix(ts): scope ts:build to packages so paigasus-console builds once

The recursive ts:build (pnpm -r --if-present run build) recursed into
apps/paigasus-console and ran next build concurrently with the app's own
paigasus-console-ts:build task, both writing .next. On a cold cache Next.js
16's build lock rejected the second runner, making moon ci :build flaky.

Scope the recursion to ./packages/** with --if-present (mandatory: no
package has a build script today, so an unguarded filter would exit 1).
Apps build via their own per-app Moon tasks. Also drop the now-irrelevant
apps/**/tsconfig.json input.

Closes SMA-391."
```

(The repo's commit-msg hook enforces Conventional Commits — the `fix(ts):` prefix satisfies it. Do NOT manually attach the PR link to Linear; the integration auto-links by branch name.)

- [ ] **Step 2: Confirm clean tree**

```bash
git status --short
```

Expected: no modified tracked files remaining (untracked `.claude/` is fine).

---

## Acceptance criteria (from spec) — final check

- [ ] Cold `moon run ts:build paigasus-console-ts:build --force` succeeds with no `.next` lock collision (Task 3).
- [ ] Cold full `moon ci :build` succeeds, no "Another next build process is already running" (Task 4).
- [ ] `paigasus-console` is built exactly once per `moon ci :build` (Task 4 Step 2).
- [ ] Library packages remain covered by their inherited per-project Moon `build` tasks (Task 4 Step 3).

## Out of scope (do NOT do here)

- The structural removal of the recursive aggregators (`ts:build`/`ts:typecheck` → Moon-owned via `workspace.inheritedTasks.exclude`) is tracked separately as **SMA-394**. Leave `typecheck` untouched.
- Do not change `paigasus-docs` `layer:` or any app's `moon.yml`.
