# Let Moon own the TS build/typecheck graph (drop recursive pnpm aggregators) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the recursive `pnpm` `ts:build`/`ts:typecheck` aggregators from `ts/moon.yml` and let Moon's per-project fan-out own the whole `:build`/`:typecheck` graph, excluding the inherited (failing) `tsc` tasks on the `ts` root.

**Architecture:** `ts/moon.yml` (`layer: configuration`) currently overrides `build`/`typecheck` with whole-tree `pnpm` recursion. Those overrides are redundant with the per-project tasks every TS project already inherits from `.moon/tasks/typescript.yml` (`tsc -p tsconfig.json --noEmit`; apps override `build` with their own `outputs:`). We delete the overrides and add `workspace.inheritedTasks.exclude: ['build', 'typecheck']` so the root does not fall back to the inherited `tsc` (which fails `TS5058` — `ts/` has only `tsconfig.base.json`). We also reclassify `paigasus-docs` to `application`, document the app-build invariant in CONTRIBUTING + the TS app scaffold template, and update `ts/README.md`.

**Tech Stack:** Moon 2.2.5 (proto-pinned), pnpm workspace, TypeScript, YAML config.

**Spec:** `docs/superpowers/specs/2026-05-29-sma-394-ts-moon-owns-build-typecheck-graph-design.md`

---

## Prerequisites

- **Branch:** work on `feature/sma-394-ts-let-moon-own-the-ts-buildtypecheck-graph-drop-recursive` (already created; the spec is committed there).
- **`moon` on PATH:** this shell does not have `moon` by default — it is a proto shim. Before any `moon` command in this plan, ensure it resolves:
  ```bash
  export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
  moon --version   # expect 2.2.5
  ```
- **Commit hygiene:** the repo's `commit-msg` hook runs commitlint. Subjects MUST start lowercase (no leading capital/acronym), avoid em-dashes, and stay ≤100 chars; a blank line is required before the body and before any footer. All commit messages below already comply.
- **No SPDX headers** on any file in this plan: `moon.yml`/`template.yml` are config and Markdown docs carry no header (per CONTRIBUTING "Code conventions").

## File Structure

| File | Responsibility | Change |
| --- | --- | --- |
| `ts/moon.yml` | `ts` root Moon project config | Remove `tasks:` block; add `workspace.inheritedTasks.exclude`; reword comments |
| `ts/apps/paigasus-docs/moon.yml` | docs app project config | `layer: library` → `application`; add invariant comment |
| `CONTRIBUTING.md` | repo-wide contributor guide | Add the app-build invariant note |
| `.moon/templates/typescript/moon.yml` | TS scaffold output | Comment the `app` branch's `build`+`outputs:` task with the invariant |
| `.moon/templates/typescript/template.yml` | TS scaffold metadata | Strengthen `description` to state the invariant |
| `ts/README.md` | TS workspace README | Fix the `moon.yml` bullet, the "gates" sentence, and the command table |

Tasks 1–4 are independent edits (each its own commit). Task 5 is the final verification gate that depends on Tasks 1–2 landing.

---

### Task 1: Remove the recursive aggregators from `ts/moon.yml`

**Files:**
- Modify: `ts/moon.yml` (full rewrite of the file)

- [ ] **Step 1: Capture the baseline (the state we are removing)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon project ts 2>&1 | sed -n '/Tasks/,/^$/p'
```
Expected: a `build` task showing `pnpm --filter "./packages/**" --if-present run build` and a `typecheck` task showing `pnpm -r --if-present run typecheck`. These are the two overrides this task deletes. (`lint`/`fmt`/`test` also appear — those stay.)

- [ ] **Step 2: Overwrite `ts/moon.yml` with the new content**

Replace the entire file with exactly this (note: `exclude: ['build', 'typecheck']` uses the same prettier-clean flow-sequence syntax as `ts/packages/commitlint-config/moon.yml`, so `ts:fmt` stays green):

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

layer: 'configuration'
language: 'typescript'

# The inherited fileGroups from .moon/tasks/typescript.yml assume src/ at the project root.
# The ts workspace keeps sources under packages/*/src and apps/*/{src,app}, so extend the
# inherited groups here. Moon merges (not overrides) fileGroups across the layers, so the
# resolved @group(sources)/@group(tests) feed the inputs of the inherited whole-tree
# lint/fmt/test that still run from ts/. (Same merge semantics confirmed for py in SMA-384.)
fileGroups:
  sources:
    - 'packages/*/src/**/*'
    - 'apps/*/src/**/*'
    - 'apps/*/app/**/*'
  tests:
    - 'packages/*/tests/**/*'
    - 'apps/*/tests/**/*'

# The ts root owns no build/typecheck of its own: Moon's per-project fan-out owns the whole
# :build / :typecheck graph — each package/app inherits `tsc -p tsconfig.json --noEmit`, and
# apps override `build` with their own `outputs:` (see paigasus-console). We must EXCLUDE (not
# merely omit) the inherited build/typecheck here, because ts/ has only tsconfig.base.json —
# no tsconfig.json — so the inherited `tsc -p tsconfig.json --noEmit` would fail TS5058 at the
# root cwd. lint/fmt/test stay inherited and run whole-tree from ts/. Same field/idiom as
# commitlint-config (SMA-395), which forward-referenced this change.
workspace:
  inheritedTasks:
    exclude: ['build', 'typecheck']
```

- [ ] **Step 3: Verify the root no longer defines build/typecheck**

```bash
moon project ts 2>&1 | sed -n '/Tasks/,/^$/p'
```
Expected: `lint`, `fmt`, `test` present; **no** `build` and **no** `typecheck`.

```bash
moon run ts:build 2>&1 | tail -3
moon run ts:typecheck 2>&1 | tail -3
```
Expected: both report an **unknown/unknown-task** error (the targets are gone) — this is success, not a failure.

- [ ] **Step 4: Confirm the config still parses across the workspace**

```bash
moon project-graph --dot >/dev/null 2>&1 && echo "graph OK" || moon sync projects 2>&1 | tail -5
```
Expected: `graph OK` (or a clean `moon sync projects`). No parse/validation error for the `workspace.inheritedTasks` field.

- [ ] **Step 5: Commit**

```bash
git add ts/moon.yml
git commit -m "refactor(ts): drop recursive build/typecheck aggregators; exclude on ts root (SMA-394)

The recursive ts:build / ts:typecheck overrides duplicated the per-project
tasks every TS project already inherits from .moon/tasks/typescript.yml, and
recreated the SMA-391 two-builders-into-one-output hazard. Remove them and
add workspace.inheritedTasks.exclude so the root does not fall back to the
inherited tsc -p tsconfig.json --noEmit (which fails TS5058: ts/ has only
tsconfig.base.json). lint/fmt/test stay inherited and run whole-tree.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Reclassify `paigasus-docs` as an application

**Files:**
- Modify: `ts/apps/paigasus-docs/moon.yml` (full rewrite)

- [ ] **Step 1: Overwrite `ts/apps/paigasus-docs/moon.yml`**

Replace the entire file with exactly this:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-docs-ts'
layer: 'application'
language: 'typescript'

# No `build` task yet: this app currently only type-checks (it inherits the no-op
# `tsc --noEmit`). When it grows a real docs build it MUST add its own `build` task
# with `outputs:` (see paigasus-console and the app-build invariant in CONTRIBUTING) —
# otherwise it would silently emit nothing.
```

- [ ] **Step 2: Verify the layer flipped and the project graph is still valid**

```bash
moon project paigasus-docs-ts 2>&1 | grep -iE 'layer|type' | head -3
moon sync projects 2>&1 | tail -5
```
Expected: layer shows `application`; `moon sync projects` completes with no constraint error. (Moon's `enforceProjectTypeRelationships` would reject an `application` being *depended upon*; nothing depends on `paigasus-docs`, so this is clean — verified in the spec.)

- [ ] **Step 3: Commit**

```bash
git add ts/apps/paigasus-docs/moon.yml
git commit -m "fix(ts): correct paigasus-docs layer to application (SMA-394)

paigasus-docs lives under apps/ but was typed layer: library. layer is pure
metadata here (task inheritance keys on language), so this is an inert
classification fix; nothing depends on the project, so the application
constraint is satisfied.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Document the app-build invariant (CONTRIBUTING + TS app template)

**Files:**
- Modify: `CONTRIBUTING.md` (append a note in the "### Moon project files" section)
- Modify: `.moon/templates/typescript/moon.yml` (comment the `app` branch)
- Modify: `.moon/templates/typescript/template.yml` (strengthen `description`)

- [ ] **Step 1: Add the invariant note to `CONTRIBUTING.md`**

Find this paragraph (end of the "### Moon project files" subsection):

```markdown
The three scaffold templates under `.moon/templates/{rust,python,typescript}/`
emit this same order, so `moon generate` output is consistent with
hand-written projects (SMA-381).
```

Insert immediately **after** it (new paragraph, blank line before):

```markdown
**App build artifacts (TypeScript):** every `ts/apps/*` that produces a build
artifact MUST define its own Moon `build` task with `outputs:` — as
`paigasus-console` does (`next build` → `outputs: ['.next']`). The `ts` root
excludes the inherited `build`/`typecheck` (SMA-394), so Moon's per-project
tasks own the build graph; a project that only inherits the default `build`
runs `tsc -p tsconfig.json --noEmit`, which type-checks but **emits nothing**.
An app without its own `build` task therefore passes a green build while
producing no artifact. The TypeScript app scaffold
(`.moon/templates/typescript/`, archetype `app`) emits this task for you.
```

- [ ] **Step 2: Comment the `app` branch in `.moon/templates/typescript/moon.yml`**

Find:

```yaml
{%- if archetype == "app" %}
tasks:
```

Replace with:

```yaml
{%- if archetype == "app" %}
# App-build invariant (SMA-394): an app that emits a build artifact MUST define its own
# `build` task with `outputs:`. The ts root excludes the inherited build/typecheck, so an
# app that only inherited the default would run `tsc --noEmit` and silently emit nothing.
tasks:
```

- [ ] **Step 3: Strengthen the `description` in `.moon/templates/typescript/template.yml`**

Replace the whole `description: |` block:

```yaml
description: |
  Scaffolds a moon.yml for a TypeScript project. `library` for a publishable
  package under ts/packages, `app` for a deployable under ts/apps (e.g. Next.js).
  Library archetype renders a header-only moon.yml; app archetype adds a `build`
  task overriding the inherited `tsc --noEmit` with `next build`. Lint/format/test
  /typecheck come from .moon/tasks/typescript.yml (ESLint + Prettier per ADR-0009;
  Vitest).
```

with:

```yaml
description: |
  Scaffolds a moon.yml for a TypeScript project. `library` for a publishable
  package under ts/packages, `app` for a deployable under ts/apps (e.g. Next.js).
  Library archetype renders a header-only moon.yml; app archetype adds a `build`
  task with `outputs:` overriding the inherited `tsc --noEmit` with `next build`.
  That `build`+`outputs:` task is REQUIRED for any app that emits an artifact: the
  ts root excludes the inherited build/typecheck (SMA-394), so an app that only
  inherited the default would run `tsc --noEmit` and silently emit nothing.
  Lint/format/test/typecheck come from .moon/tasks/typescript.yml (ESLint +
  Prettier per ADR-0009; Vitest).
```

- [ ] **Step 4: Verify the template still renders**

```bash
moon generate typescript /tmp/sma394-tpl --dry-run --defaults 2>&1 | tail -15 || \
moon generate typescript /tmp/sma394-tpl --dry-run 2>&1 | tail -15
```
Expected: no template/Tera parse error; dry-run prints the would-be `moon.yml`. (If `moon generate` prompts despite `--defaults`, it is enough that it parses `template.yml` without error — Ctrl-C out.)

- [ ] **Step 5: Commit**

```bash
git add CONTRIBUTING.md .moon/templates/typescript/moon.yml .moon/templates/typescript/template.yml
git commit -m "docs(ts): document the app-build invariant in CONTRIBUTING and TS app template (SMA-394)

Every ts/apps/* that emits a build artifact must define its own Moon build
task with outputs:, else it only inherits tsc --noEmit and silently emits
nothing. State this in CONTRIBUTING and in the TS app scaffold template
(comment + description); the template already emits the task, so this is
documentation of an already-enforced rule.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Update `ts/README.md`

**Files:**
- Modify: `ts/README.md` (3 prose edits + table; then prettier-normalize)

- [ ] **Step 1: Fix the `moon.yml` Layout bullet (line 12)**

Replace:

```markdown
- `moon.yml` — workspace parent project (`layer: configuration`). Owns workspace-wide `typecheck` and `build` (recursive `pnpm -r --if-present run <task>`); inherits `lint`/`fmt`/`test` from `.moon/tasks/typescript.yml`.
```

with:

```markdown
- `moon.yml` — workspace parent project (`layer: configuration`). Excludes the inherited `build`/`typecheck` — Moon's per-project tasks own the full `:build`/`:typecheck` graph, and the root has no `tsconfig.json` to run `tsc` against; still owns the whole-tree `lint`/`fmt`/`test` inherited from `.moon/tasks/typescript.yml`.
```

- [ ] **Step 2: Fix the "gates" sentence (line 24)**

Replace:

```markdown
The workspace-wide gates live on the `ts` Moon project and run once over the whole workspace from `ts/`:
```

with:

```markdown
`lint`/`fmt`/`test` run once over the whole workspace from the `ts` Moon project; `typecheck` and `build` fan out per project (Moon owns the `:typecheck`/`:build` graph), so they are addressed with the all-projects target form:
```

- [ ] **Step 3: Update the command table (lines 30, 32, 33)**

Replace the table body so the `Type check` and `Build` rows use the all-projects form (leave `lint`/`fmt`/`test` on `ts:` — those tasks still live on the root). Exact replacement for the three changed rows:

Change line 30 from:
```markdown
| Type check       | `moon run ts:typecheck`              |
```
to:
```markdown
| Type check       | `moon run :typecheck`                |
```

Change lines 32–33 from:
```markdown
| Build (libs)     | `moon run ts:build`                  |
| Build (Next app) | `moon run paigasus-console-ts:build` |
```
to:
```markdown
| Build (all)      | `moon run :build`                    |
| Build (one app)  | `moon run paigasus-console-ts:build` |
```

(Exact padding does not matter — Step 5 runs `prettier --write` to re-align the table.)

- [ ] **Step 4: Add a Notes bullet explaining the `ts:`/`:` asymmetry**

After the first Notes bullet (the one starting "For env parity, invoke pnpm via `moon run ts:<task>`…"), add:

```markdown
- `Type check` and `Build` use the all-projects target form (`moon run :typecheck` / `moon run :build`): the `ts` root no longer defines those tasks — Moon's per-project tasks own them (SMA-394) — whereas `lint`/`fmt`/`test` still run once from the `ts` project.
```

- [ ] **Step 5: Normalize formatting and verify it is prettier-clean**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm exec prettier --write README.md && cd ..
moon run ts:fmt 2>&1 | tail -5
```
Expected: `prettier --write` reports `README.md` (re-aligned table); `moon run ts:fmt` passes (no formatting violations across the ts tree).

- [ ] **Step 6: Confirm no stale `ts:build`/`ts:typecheck` references remain**

```bash
grep -nE 'ts:build|ts:typecheck' ts/README.md || echo "clean"
```
Expected: `clean`.

- [ ] **Step 7: Commit**

```bash
git add ts/README.md
git commit -m "docs(ts): update ts README for Moon-owned build/typecheck graph (SMA-394)

ts:build / ts:typecheck no longer exist; document the all-projects form
(moon run :build / :typecheck), reword the moon.yml layout bullet and the
gates sentence, and relabel the build rows (:build covers libs and apps).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Full verification gate (acceptance criteria)

No code changes — this task proves the ACs. If any check fails, fix the offending file, re-run, and commit the fix before proceeding.

**Files:** none (verification only)

- [ ] **Step 1: Root task list is correct**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon project ts 2>&1 | sed -n '/Tasks/,/^$/p'
```
Expected: `lint`, `fmt`, `test` only; no `build`/`typecheck`.

- [ ] **Step 2: Cold affected-graph typecheck (the issue's AC form)**

```bash
moon ci :typecheck --base main 2>&1 | tail -20
```
Expected: exit 0, 0 failed. Every TS project's `tsc --noEmit` passes; the `ts` root and `commitlint-config` are absent (excluded). No `TS5058` from the root.

- [ ] **Step 3: Cold affected-graph build; console builds exactly once**

```bash
rm -rf ts/apps/paigasus-console/.next
moon ci :build --base main 2>&1 | tee /tmp/sma394-build.log | tail -25
echo "next build occurrences:"; grep -c 'next build\|paigasus-console-ts:build' /tmp/sma394-build.log
ls -d ts/apps/paigasus-console/.next && echo ".next produced"
```
Expected: exit 0, 0 failed; `paigasus-console-ts:build` runs **once** (no second builder into `.next`); `.next` exists. (If `moon ci --base main` resolves nothing as affected in your checkout, fall back to `moon run paigasus-console-ts:build --force` plus `moon run :typecheck`.)

- [ ] **Step 4: Aggregator targets are gone**

```bash
moon run ts:build 2>&1 | tail -2
moon run ts:typecheck 2>&1 | tail -2
```
Expected: both report an unknown task (success — proves the aggregators were removed).

- [ ] **Step 5: lint/fmt still green (regression guard for the edited YAML + README)**

```bash
moon run ts:lint 2>&1 | tail -5
moon run ts:fmt 2>&1 | tail -5
```
Expected: both pass — the hand-written `ts/moon.yml`, `paigasus-docs/moon.yml`, and `README.md` edits are lint/format clean.

- [ ] **Step 6: Final review against the acceptance criteria**

Confirm each spec AC is green:
- [ ] No recursive `pnpm -r` / `pnpm --filter` build/typecheck aggregator remains in `ts/moon.yml` (Task 1 Step 2).
- [ ] `moon ci :build` / `:typecheck` succeed; `paigasus-console` builds exactly once (Steps 2–3).
- [ ] The `ts` root no longer fails on an inherited `tsc --noEmit` — it has no `build`/`typecheck` (Step 1).
- [ ] App-build invariant documented in CONTRIBUTING + the TS app scaffold template (Task 3).
- [ ] `paigasus-docs` is `layer: application` (Task 2).

---

## Notes / Out of scope (do not do here)

- **Do NOT touch `py/moon.yml`.** It is the same pre-fix shape (inherits `build`/`typecheck`; `py:build` builds a junk root wheel), but `py:build` succeeds so it does not block this issue. It is a separately-filed py twin follow-up — see the spec's "Out of scope".
- **Do NOT add a CI assertion that apps emit their output.** Deferred to SMA-361 (the `.github/workflows` dir is still an empty `.gitkeep`).
- **Do NOT touch the root `lint`/`fmt`/`test` redundancy.** Pre-existing, repo-wide; out of scope.

## Self-Review (performed against the spec)

- **Spec coverage:** all five files in the spec's "Files touched" map to Tasks 1–4; all five ACs map to Task 5 Step 6. The two F1/F2/F3 review corrections are reflected (README "Build (all)" relabel, application-constraint check in Task 2 Step 2, no "mirror py" claim, py twin called out as out-of-scope).
- **Placeholder scan:** no TBD/TODO/"handle errors"; every edit shows exact before/after content and exact commands with expected output.
- **Consistency:** the `workspace.inheritedTasks.exclude: ['build', 'typecheck']` syntax matches `commitlint-config/moon.yml` (proven prettier-clean and resolvable in Moon 2.2.5 per SMA-395); commit subjects are commitlint-compliant (lowercase lead, no em-dash, ≤100 chars).
