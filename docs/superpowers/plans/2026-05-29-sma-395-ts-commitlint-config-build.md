# SMA-395 — `commitlint-config-ts` build/typecheck fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the per-project `commitlint-config-ts:build` and `:typecheck` tasks from failing `TS5058`, by opting that config-only package out of the inherited `tsc` tasks.

**Architecture:** `ts/packages/commitlint-config` is a pure CommonJS config package (`index.cjs`, no `tsconfig.json`). Because its `moon.yml` is `language: typescript`, it inherits `build`/`typecheck` from `.moon/tasks/typescript.yml` (`pnpm exec tsc -p tsconfig.json --noEmit`), which fail with no `tsconfig.json`. The fix adds a project-level `workspace.inheritedTasks.exclude: ['build', 'typecheck']` block to that one `moon.yml`. Nothing else changes; `lint`/`fmt`/`test` stay inherited.

**Tech Stack:** Moon 2.2.5 (task runner), pnpm workspace, TypeScript `tsc`. No application code; the only artifact is a `moon.yml` config edit.

**Why no unit test:** This is a Moon config change with no code under test. The TDD discipline is preserved as **reproduce → fix → verify** against Moon's own task resolution: Step 1 reproduces the `TS5058` failure (the "red"), the edit is the fix (the "green"), and the `moon project` / `moon run` checks prove it.

**Spec:** `docs/superpowers/specs/2026-05-29-sma-395-ts-commitlint-config-build-design.md`

---

## File Structure

One file is modified; nothing is created or deleted.

- **Modify:** `ts/packages/commitlint-config/moon.yml` — add a `workspace.inheritedTasks.exclude` block (with an explanatory comment) so this project no longer inherits the `build`/`typecheck` tasks. Single responsibility: this project's Moon task configuration.

No `tsconfig.json` is added (rejected in the spec). No change to `.moon/tasks/typescript.yml` or `ts/moon.yml` (out of scope; SMA-394 owns those).

> **Note:** `moon.yml` files in this repo do **not** carry the SPDX header (that convention is for source files; the existing `moon.yml` files start at `$schema:`). Do not add one.

---

### Task 1: Exclude inherited `build`/`typecheck` on `commitlint-config-ts`

**Files:**
- Modify: `ts/packages/commitlint-config/moon.yml`

- [ ] **Step 1: Reproduce the failure (the "red")**

Confirm the bug exists before fixing it. `--force` bypasses Moon's cache so the task actually executes.

Run:
```bash
moon run commitlint-config-ts:build --force
```
Expected: **non-zero exit**, output contains:
```
error TS5058: The specified path does not exist: 'tsconfig.json'.
× Task commitlint-config-ts:build failed to run.
```

Then confirm `typecheck` fails identically:
```bash
moon run commitlint-config-ts:typecheck --force
```
Expected: same `TS5058` failure.

- [ ] **Step 2: Apply the fix**

Edit `ts/packages/commitlint-config/moon.yml`. It currently reads in full:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'commitlint-config-ts'
layer: 'library'
language: 'typescript'
```

Replace its entire contents with:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'commitlint-config-ts'
layer: 'library'
language: 'typescript'

# Pure CommonJS config package (index.cjs): no tsconfig.json and nothing to
# compile or type-check. It stays `language: typescript` only so lint/fmt/test
# still attach. Opt out of the inherited per-project `build`/`typecheck` tasks
# (.moon/tasks/typescript.yml runs `tsc -p tsconfig.json --noEmit`, which fails
# TS5058 with no tsconfig.json). First use of this field in the repo; SMA-394
# will later apply the same field to the `ts` root.
workspace:
  inheritedTasks:
    exclude: ['build', 'typecheck']
```

- [ ] **Step 3: Verify the resolved task list (the "green")**

Run:
```bash
moon project commitlint-config-ts
```
Expected: under TASKS, there is **no `build`** and **no `typecheck`**; `lint`, `fmt`, and `test` are still listed. (If `build`/`typecheck` still appear, the `workspace:` block is mis-indented or the field name is wrong — recheck against Step 2.)

- [ ] **Step 4: Confirm both tasks are gone (not just one)**

Run:
```bash
moon run commitlint-config-ts:build
moon run commitlint-config-ts:typecheck
```
Expected: each reports the target **does not exist** (e.g. `Unknown task build` / `No such task`) and exits non-zero. This is the "correctly excluded" branch of the AC — an *unknown-task* error, not a *task-failed* error, and crucially **no `TS5058`**.

- [ ] **Step 5: Confirm the kept tasks still run on the lone `.cjs`**

Run:
```bash
moon run commitlint-config-ts:lint commitlint-config-ts:fmt commitlint-config-ts:test
```
Expected: all three **succeed** (exit 0). `test` passes via `vitest run --passWithNoTests`; `lint`/`fmt` operate on `index.cjs`.

- [ ] **Step 6: Cold full build graph — no `TS5058`**

Run:
```bash
moon run :build
```
Expected: the run completes with **`commitlint-config-ts` absent from any failure** and **no `error TS5058`** anywhere in the output. (Success criterion for this fix is specifically the absence of `TS5058` / any `commitlint-config-ts` failure. An unrelated failure in another workspace — rs/py/contracts — is out of scope for SMA-395; if one occurs, confirm it is not `commitlint-config-ts` and not `TS5058`, then note it.)

- [ ] **Step 7: Cold full typecheck graph — no `TS5058`**

Run:
```bash
moon run :typecheck
```
Expected: same as Step 6 — no `TS5058`, `commitlint-config-ts` not in failures.

- [ ] **Step 8: Commit**

```bash
git add ts/packages/commitlint-config/moon.yml
git commit -m "fix(ts): exclude commitlint-config from inherited tsc build/typecheck (SMA-395)"
```
(The repo's lefthook `commit-msg` hook runs commitlint; the Conventional-Commit message above passes it.)

---

## Self-Review

**1. Spec coverage:**
- Spec "Decision" (add `workspace.inheritedTasks.exclude: ['build','typecheck']` with comment) → Task 1, Step 2. ✓
- AC "no longer has build/typecheck; no `TS5058`" → Steps 3, 4, 6, 7. ✓
- AC "full cold `moon run :build` 0 failures (no `TS5058`)" → Step 6 (+ Step 7 for typecheck). ✓
- AC "retains lint/fmt/test" → Steps 3 and 5. ✓
- Spec verification plan (steps 1–3: `moon project`, `moon run :build`, `moon run :typecheck`) → Steps 3, 6, 7. ✓
- Out-of-scope items (SMA-394 aggregators, the recurring-class follow-up) → correctly **not** in any task. ✓

**2. Placeholder scan:** No TBD/TODO/"handle edge cases"/"similar to". Every step has the exact command and expected output, and Step 2 shows the complete resulting file. ✓

**3. Type consistency:** Single config field used consistently — `workspace.inheritedTasks.exclude` with task names `build`/`typecheck` in Step 2 match the absence checked in Steps 3/4 and the kept tasks (`lint`/`fmt`/`test`) checked in Step 5. Project id `commitlint-config-ts` is identical across all steps. ✓
