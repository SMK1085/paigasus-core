# SMA-396 — Config-only TS package scaffold shape + CI guard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make "config-only TS package" (a `language: typescript` project that is not a `tsc` compilation unit — no `.ts` sources, no `tsconfig.json`) a first-class shape: a `config` scaffold archetype that emits the `inheritedTasks.exclude: ['build','typecheck']` block, a documented convention, and a run-once CI guard that fails loudly (not the cryptic `TS5058`) when a config-only package is added without the exclude.

**Architecture:** Three layers — (1) the TS scaffold template gains a `config` archetype (paves the path); (2) a Node guard script + Moon task `ts:check-config-only`, wired into CI, enumerates `language: typescript` projects via `moon query projects` and fails if any lacks a `tsconfig.json` yet still has inherited `build`/`typecheck` (enforces the path); (3) CONTRIBUTING + Notion document it. `config` packages are `layer: library` (importable/published code) per the repo's documented `layer` semantics.

**Tech Stack:** Moon 2.2.5 (project/template config, `moon query projects`, `moon generate`), Tera templates, Node ESM (guard script), GitHub Actions, pnpm/eslint/prettier.

**Spec:** `docs/superpowers/specs/2026-05-31-sma-396-config-only-ts-scaffold-design.md`

---

## Pre-flight: environment

`moon` is proto-managed and is **not** on a non-interactive shell's `PATH`. If `moon: command not found` appears, prefix with:

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
```

`moon` *is* on PATH inside a Moon task (verified `moon 2.2.5`), so the guard task can spawn `moon query projects`.

## Verified facts the code depends on

- `moon query projects` prints JSON; the `--json` flag is **rejected** (exit 2). Each `.projects[]` entry has top-level `id`, `source` (repo-root-relative), `root` (absolute path), `language`, and `tasks` (resolved, **exclusions applied** — `commitlint-config-ts` → `fmt,lint,test`).
- The `ts` root project (`id: ts`, `language: typescript`) has no `tsconfig.json` but already excludes `build`/`typecheck`, so the guard treats it as compliant (verified).
- `ts/eslint.config.js` declares **no** node globals; `js.configs.recommended` (with `no-undef`) lints `.mjs`. The guard script therefore imports `process` and uses `process.stdout/stderr.write` (no bare `console`/`process` globals). ES built-ins (`JSON`, `Object`) are fine.
- Prettier: `printWidth: 200, semi: true, singleQuote: true, trailingComma: 'all', arrowParens: 'always'`.

## File Structure

- `.moon/templates/typescript/template.yml` — scaffold metadata; `archetype` enum gains `config`.
- `.moon/templates/typescript/moon.yml` — scaffold body; gains a `config` branch emitting the exclude block.
- `ts/scripts/check-config-only.mjs` — **new** guard script (one responsibility: enumerate TS projects, flag config-only-without-exclude).
- `ts/moon.yml` — gains the `check-config-only` Moon task (alongside `commitlint`).
- `.github/workflows/ci.yml` — gains an explicit guard step.
- `CONTRIBUTING.md` — gains the "Config-only TS packages" convention paragraph.
- `ts/packages/commitlint-config/moon.yml` — comment realign (exclude block unchanged).
- Notion Development Guidelines — mirror the convention (external; pre-merge).

---

### Task 1: Add the `config` archetype to the TS scaffold template

**Files:**
- Modify: `.moon/templates/typescript/template.yml`
- Modify: `.moon/templates/typescript/moon.yml`

- [ ] **Step 1: Extend the archetype enum + description in `template.yml`**

Replace the `description` block and the `archetype` variable. The file becomes:

```yaml
title: 'TypeScript project'
description: |
  Scaffolds a moon.yml for a TypeScript project. `library` for a publishable
  package under ts/packages, `app` for a deployable under ts/apps (e.g. Next.js),
  `config` for a config-only package that is not a tsc compilation unit (no .ts
  sources — e.g. a shared eslint/prettier/commitlint config).
  Library renders a header-only moon.yml; app adds a `build` task with `outputs:`
  overriding the inherited `tsc --noEmit` with `next build`; config adds
  `inheritedTasks.exclude: ['build','typecheck']` (those inherited tasks run
  `tsc -p tsconfig.json --noEmit`, which fails TS5058 with no tsconfig.json).
  The app `build`+`outputs:` task is REQUIRED for any app that emits an artifact:
  the ts root excludes the inherited build/typecheck (SMA-394), so an app that
  only inherited the default would run `tsc --noEmit` and silently emit nothing.
  Lint/format/test/typecheck come from .moon/tasks/typescript.yml (ESLint +
  Prettier per ADR-0009; Vitest).
variables:
  name:
    type: 'string'
    default: ''
    required: true
    prompt: 'Project name (e.g. sdk)?'
  archetype:
    type: 'enum'
    values: ['library', 'app', 'config']
    default: 'library'
    prompt: 'Archetype?'
```

- [ ] **Step 2: Add the `config` branch in `moon.yml`**

Replace the trailing `{%- if archetype == "app" %} … {%- endif %}` block so the file reads:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: '{{ name }}-ts'
layer: '{% if archetype == "app" %}application{% else %}library{% endif %}'
language: 'typescript'
{%- if archetype == "app" %}
# App-build invariant (SMA-394): an app that emits a build artifact MUST define its own
# `build` task with `outputs:`. The ts root excludes the inherited build/typecheck, so an
# app that only inherited the default would run `tsc --noEmit` and silently emit nothing.
tasks:
  build:
    command: 'pnpm exec next build'
    # next.config.{ts,js,mjs} are all supported by Next; list each so the config
    # file edits invalidate cache regardless of which extension a future project uses.
    inputs: ['@group(sources)', 'tsconfig.json', 'package.json', 'next.config.ts', 'next.config.js', 'next.config.mjs']
    outputs: ['.next']
    options:
      merge: replace
{%- elif archetype == "config" %}
# Config-only TS package: not a tsc compilation unit (no .ts sources to type-check — e.g. an
# eslint/prettier/commitlint config). Stays language: typescript so lint/fmt/test still attach;
# excludes the inherited per-project build/typecheck, which run `tsc -p tsconfig.json --noEmit`
# and fail TS5058 with no tsconfig.json. The config-only CI guard (ts:check-config-only) enforces
# this exclude repo-wide. See CONTRIBUTING "Moon project files". (SMA-396)
workspace:
  inheritedTasks:
    exclude: ['build', 'typecheck']
{%- endif %}
```

- [ ] **Step 3: Verify the `config` archetype renders correctly**

Run:
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon generate typescript --to ts/packages/scratch-config -- --name scratch-config --archetype config
cat ts/packages/scratch-config/moon.yml
```
Expected: a `moon.yml` with `id: 'scratch-config-ts'`, `layer: 'library'`, `language: 'typescript'`, the config comment, and:
```yaml
workspace:
  inheritedTasks:
    exclude: ['build', 'typecheck']
```
Then confirm the `library` and `app` archetypes still render (no `config` body leaking):
```bash
moon generate typescript --to ts/packages/scratch-lib -- --name scratch-lib --archetype library
cat ts/packages/scratch-lib/moon.yml   # header-only: $schema, id, layer: library, language; NO workspace block
```

- [ ] **Step 4: Clean up the throwaway packages + Moon cache residue**

`.moon/cache` is gitignored, so a plain `rm` + empty `git status` does NOT prove the workspace is pristine — `moon generate` leaves cache-state. Remove both:
```bash
rm -rf ts/packages/scratch-config ts/packages/scratch-lib
moon clean
git status --short ts/   # expect empty (necessary, not sufficient)
```

- [ ] **Step 5: Commit**

```bash
git add .moon/templates/typescript/template.yml .moon/templates/typescript/moon.yml
git commit -m "feat(ts): add a config-only archetype to the TS scaffold template (SMA-396)"
```
(Expect `✔️ commitlint`.)

---

### Task 2: Config-only CI guard — script + Moon task (TDD)

**Files:**
- Create: `ts/scripts/check-config-only.mjs`
- Modify: `ts/moon.yml` (add the `check-config-only` task)

- [ ] **Step 1: Write the guard script**

Create `ts/scripts/check-config-only.mjs` with exactly:

```js
// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import process from 'node:process';

// Enumerate Moon's resolved project graph (moon query projects prints JSON; the --json
// flag is rejected). Each entry has top-level id/source/root/language/tasks, and tasks
// already reflects workspace.inheritedTasks.exclude.
const { projects } = JSON.parse(execFileSync('moon', ['query', 'projects'], { encoding: 'utf8' }));
const tsProjects = projects.filter((p) => p.language === 'typescript');

// A config-only package is not a tsc compilation unit (no tsconfig.json), so the inherited
// `tsc -p tsconfig.json --noEmit` build/typecheck would fail TS5058. It must exclude them.
const violations = tsProjects.filter((p) => {
  if (existsSync(join(p.root, 'tsconfig.json'))) return false;
  const tasks = Object.keys(p.tasks ?? {});
  return tasks.includes('build') || tasks.includes('typecheck');
});

if (violations.length > 0) {
  const lines = [
    'Config-only TS packages (no tsconfig.json) must exclude the inherited build/typecheck:',
    ...violations.map((p) => `  - ${p.id} (${p.source})`),
    '',
    "Fix: add `workspace.inheritedTasks.exclude: ['build', 'typecheck']` to the project's moon.yml",
    '(scaffold archetype `config`), or add a tsconfig.json. See CONTRIBUTING "Moon project files".',
  ];
  process.stderr.write(`${lines.join('\n')}\n`);
  process.exit(1);
}

process.stdout.write(`config-only guard: ${tsProjects.length} TS projects checked, no violations\n`);
```

- [ ] **Step 2: Add the `check-config-only` Moon task to `ts/moon.yml`**

Under the existing `tasks:` block (next to `commitlint`), add:

```yaml
  check-config-only:
    # Enforces the config-only TS shape (SMA-396): a language:typescript project with no
    # tsconfig.json MUST exclude the inherited build/typecheck. Run-once guard, invoked
    # explicitly in CI (never via `moon ci`); turns a cryptic TS5058 into an actionable error.
    # Do NOT set runInCI: false (Moon would then drop it under CI=true → explicit `moon run`
    # resolves zero tasks and exits 1, same as the commitlint task).
    command: 'node scripts/check-config-only.mjs'
    inputs: []
    options:
      cache: false
```

- [ ] **Step 3: GREEN — guard passes on the current tree**

```bash
moon run ts:check-config-only
```
Expected: `config-only guard: 7 TS projects checked, no violations` (or current count), exit 0.

- [ ] **Step 4: RED — guard fails for a config-only package missing the exclude**

Generate a config package, then strip its exclude block to simulate a hand-authored mistake:
```bash
moon generate typescript --to ts/packages/scratch-config -- --name scratch-config --archetype config
# Remove the `workspace:` exclude block so it becomes a violation:
printf "%s\n" "\$schema: 'https://moonrepo.dev/schemas/project.json'" "id: 'scratch-config-ts'" "layer: 'library'" "language: 'typescript'" > ts/packages/scratch-config/moon.yml
moon run ts:check-config-only; echo "exit=$?"
```
Expected: lists `scratch-config-ts (ts/packages/scratch-config)`, prints the `Fix:` message, **exit 1**.

- [ ] **Step 5: GREEN again — restore the exclude, guard passes; then clean up**

```bash
# Re-generate the correct (excluded) form, confirm pass, then remove + clean cache:
moon generate typescript --to ts/packages/scratch-config --force -- --name scratch-config --archetype config
moon run ts:check-config-only; echo "exit=$?"        # expect exit 0
rm -rf ts/packages/scratch-config
moon clean
git status --short ts/                                # expect empty
```

- [ ] **Step 6: Lint + format the new script (it lives under the ts lint/fmt scope)**

```bash
moon run ts:fmt ts:lint
```
Expected: 0 failed. If `ts:fmt` fails, run `pnpm --dir ts exec prettier --write scripts/check-config-only.mjs` and re-run; the script was written to the repo prettier style (semi, single quotes, trailing commas, 200 width) and uses only imported bindings + ES built-ins, so `no-undef` passes.

- [ ] **Step 7: Commit**

```bash
git add ts/scripts/check-config-only.mjs ts/moon.yml
git commit -m "feat(ts): add ts:check-config-only guard enforcing the config-only exclude (SMA-396)"
```

---

### Task 3: Wire the guard into CI

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add an explicit guard step**

In `.github/workflows/ci.yml`, between the `Validate commit messages (commitlint parity gate)` step and the `moon ci (affected graph)` step, insert (no `if:` — it runs on push **and** PR, unlike the commitlint step):

```yaml
      - name: Config-only TS guard (no tsconfig.json ⇒ must exclude build/typecheck)
        run: |
          set -euo pipefail
          moon run ts:check-config-only
```

- [ ] **Step 2: Verify the workflow YAML is well-formed and the step resolves**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
node -e "const y=require('node:fs').readFileSync('.github/workflows/ci.yml','utf8'); if(!y.includes('ts:check-config-only')) throw new Error('guard step missing'); console.log('guard step present ✓')"
moon run ts:check-config-only   # the exact command CI runs; expect exit 0
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: run the config-only TS guard on every push and PR (SMA-396)"
```

---

### Task 4: Documentation — CONTRIBUTING paragraph + commitlint-config-ts comment realign

**Files:**
- Modify: `CONTRIBUTING.md` (after the app-build-invariant paragraph, ~line 164)
- Modify: `ts/packages/commitlint-config/moon.yml` (comment only)

- [ ] **Step 1: Add the "Config-only TS packages" paragraph to `CONTRIBUTING.md`**

Find the app-build-invariant paragraph ending `…archetype `app`) emits this task for you.` immediately before the `## Contributor License Agreement` heading, and insert a new paragraph between them:

```markdown
**Config-only TS packages:** a TypeScript *package* that is not a `tsc` compilation unit
(no `.ts` sources — a CommonJS/JSON config such as a shared `eslint`/`prettier`/`commitlint`
config; `commitlint-config` is the one today) MUST exclude the inherited per-project
`build`/`typecheck`: `workspace.inheritedTasks.exclude: ['build', 'typecheck']`. Those tasks
run `tsc -p tsconfig.json --noEmit`, which fails `TS5058` with no `tsconfig.json`. It stays
`language: typescript` (so `lint`/`fmt`/`test` still attach) and `layer: library`
(importable/published code). The TypeScript scaffold (`.moon/templates/typescript/`, archetype
`config`) emits this block for you, and the `ts:check-config-only` CI guard fails the build with
an actionable message if a config-only package is added without it.
```

- [ ] **Step 2: Realign the `commitlint-config-ts` comment**

In `ts/packages/commitlint-config/moon.yml`, replace the existing comment (the
`# Pure CommonJS config package … SMA-394 will later apply the same field to the ts root.` block)
with — leaving `workspace.inheritedTasks.exclude` and every other field unchanged:

```yaml
# Reference instance of the config-only TS package shape (SMA-396): a pure CommonJS config
# package (index.cjs) that is not a tsc compilation unit — no tsconfig.json, nothing to compile
# or type-check. Stays `language: typescript` so lint/fmt/test still attach; excludes the
# inherited per-project build/typecheck (.moon/tasks/typescript.yml runs `tsc -p tsconfig.json
# --noEmit`, which fails TS5058 with no tsconfig.json). See CONTRIBUTING "Moon project files";
# the ts:check-config-only CI guard enforces this exclude.
```

- [ ] **Step 3: Verify docs render and commitlint-config-ts is unchanged in behavior**

```bash
grep -n 'Config-only TS packages' CONTRIBUTING.md          # paragraph present
moon project commitlint-config-ts | grep -iE 'build|typecheck' || echo "still excluded ✓"
moon run ts:check-config-only                              # still 0 violations
```

- [ ] **Step 4: Commit**

```bash
git add CONTRIBUTING.md ts/packages/commitlint-config/moon.yml
git commit -m "docs(ts): document the config-only TS package convention (SMA-396)"
```

---

### Task 5: Notion Development Guidelines sync (pre-merge)

**Files:** none in-repo (external Notion page).

- [ ] **Step 1: Locate the Development Guidelines page**

Use the Notion MCP tools: `notion-search` for "Development Guidelines" (the Paigasus dev guidelines linked from CONTRIBUTING). Confirm it is the canonical conventions page (it should already carry the Moon project-shape / app-build-invariant material).

- [ ] **Step 2: Mirror the config-only convention**

Add a "Config-only TS packages" entry mirroring the CONTRIBUTING paragraph (the shape, the `exclude` block, `layer: library`, the `ts:check-config-only` guard) into the Moon project-files / conventions section, alongside the existing app-build invariant. Use `notion-update-page` (or `notion-create-pages` for a sub-block) as the page structure dictates.

- [ ] **Step 3: Confirm the mirror**

`notion-fetch` the page and verify the config-only section is present and consistent with CONTRIBUTING. (No git commit — external.) This MUST be done before the PR merges.

---

### Task 6: Final verification + PR

**Files:** none (verification + PR).

- [ ] **Step 1: Full-graph green, guard green, no cruft**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run ts:check-config-only                      # 0 violations
moon run :build :typecheck :lint :fmt --force 2>&1 | grep -iE 'failed|Tasks:' | tail -5
git status --short                                 # expect empty working tree
```
Expected: guard passes; whole graph 0 failed; clean tree.

- [ ] **Step 2: Confirm the commit set**

```bash
git log --oneline origin/main..HEAD
```
Expected: the spec/plan commits plus `feat(ts)` (archetype), `feat(ts)` (guard), `ci` (wiring), `docs(ts)` (convention).

- [ ] **Step 3: Push and open the PR**

```bash
git push -u origin feature/sma-396-ts-make-config-only-ts-packages-a-first-class-shape
gh pr create --base main --fill
```
Do **not** attach the Linear link manually (auto-links by branch name). Title e.g. `feat(ts): config-only TS package scaffold shape + CI guard (SMA-396)`. Ensure the Notion sync (Task 5) is complete before merging.

---

## Self-Review

**1. Spec coverage** — every spec section maps to a task:
- Decision A (scaffold `config` archetype) → Task 1.
- Decision B (CI guard: script + Moon task + CI step) → Tasks 2 (script+task) and 3 (CI wiring).
- Decision C (CONTRIBUTING paragraph) → Task 4 Step 1; Notion mirror → Task 5.
- Decision D (commitlint-config-ts comment realign) → Task 4 Step 2.
- "guard passes on current tree" AC → Task 2 Step 3 + Task 3 Step 2 + Task 6 Step 1.
- "guard fails for non-excluded config package" AC → Task 2 Step 4 (RED).
- "config archetype package has no build/typecheck, no TS5058 out of the box" AC → Task 1 Step 3 + Task 2 Step 5.
- "commitlint-config-ts unchanged" AC → Task 4 Step 3.
- `layer: library` decision → encoded in Task 1 Step 2 (config falls through to `library`).
- Out-of-scope (no real second config package; TS18003 edge; no layer reclass) → honored; not implemented.

**2. Placeholder scan** — no TBD/TODO; every code/YAML/markdown block is literal final content; every command has an expected result. Task 5 (Notion) is inherently external, so its steps describe the MCP actions rather than in-repo code — that is the nature of the task, not a placeholder.

**3. Type/identifier consistency** — `ts:check-config-only` (task id), `check-config-only.mjs` (script), `scratch-config`/`scratch-config-ts` (throwaway id), the `moon query projects` fields (`id`/`source`/`root`/`language`/`tasks`), `workspace.inheritedTasks.exclude: ['build', 'typecheck']`, and `archetype` values `['library','app','config']` are used consistently across all tasks and match the spec and the live `.moon/tasks/typescript.yml` / `ts/eslint.config.js` / `ts/.prettierrc.js`.
