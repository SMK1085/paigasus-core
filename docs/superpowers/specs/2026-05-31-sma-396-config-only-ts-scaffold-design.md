# SMA-396 — Config-only TS packages as a first-class scaffold shape

**Status:** Design approved
**Date:** 2026-05-31
**Linear:** SMA-396
**Branch:** `feature/sma-396-ts-make-config-only-ts-packages-a-first-class-shape`
**Related:** SMA-395 (special-cased `commitlint-config-ts`, flagged this follow-up), SMA-394 (ts-root exclude + app-build invariant), SMA-361 (CI is live), SMA-381 (scaffold field-order / `layer` semantics)

## Problem

The per-project `build` and `typecheck` tasks inherited from `.moon/tasks/typescript.yml` are
both `pnpm exec tsc -p tsconfig.json --noEmit` (lines 23 and 54). This bakes in the assumption
that **every** `language: typescript` project is a `tsc` compilation unit with a `tsconfig.json`.
A **config-only** package — one that is *not a `tsc` compilation unit* (no `.ts` sources to
type-check: a CommonJS/JSON config such as a shared `eslint`/`prettier`/`commitlint` config) —
violates that. When it has no `tsconfig.json`, the inherited task fails `TS5058` ("The specified
path does not exist: 'tsconfig.json'") on a full `moon run :build` / `moon ci :build`.

SMA-395 fixed the one such package today (`ts/packages/commitlint-config`, `commitlint-config-ts`)
with a per-project opt-out:

```yaml
workspace:
  inheritedTasks:
    exclude: ['build', 'typecheck']
```

But this is a **class, not a one-off**. A future `@paigasus/eslint-config`,
`@paigasus/prettier-config`, or a shared `tsconfig` package would each be config-only and need the
same block — rediscovered per package. SMA-394 does **not** cover this: it dropped the *root*
aggregators and excluded tasks on the `ts` **root** project, leaving the *per-project* inherited
`tsc -p tsconfig.json` task — the actual failure — unchanged. And since CI is now live (SMA-361),
a config-only package missing the block is a **real red PR** with a cryptic `TS5058`, not a
theoretical one.

> **Define the shape by what it is, not by one symptom.** The shape is "not a `tsc` compilation
> unit," *not* "ships no `tsconfig.json`." The two usually coincide, but a shared `tsconfig`
> package's *product* is a `tsconfig.json` — it ships one yet still isn't a compilation unit and
> still wants the exclude (there it fails `TS18003` "No inputs were found", not `TS5058`). The
> convention, the scaffold comment, and the guard below are all framed around "not a `tsc` unit."

## Decision: (a) scaffold + convention, plus a CI guard to enforce it

The issue offered two directions; the recorded decision is **(a)** — a `config` scaffold archetype
plus a documented convention — **augmented with a run-once CI guard** so "first-class shape" means
*enforced*, not merely *documented*:

- **(a) paves the path:** the scaffold emits the exclude block and CONTRIBUTING documents the shape,
  so the common path produces a correct package.
- **The guard enforces it:** because the TS scaffold generates only `moon.yml` (config packages are
  largely hand-authored — `package.json`, `index.cjs`, etc.), a contributor can easily create one
  off-scaffold and miss the block. A run-once CI check converts that cryptic `TS5058` into one
  actionable message.

### Why not (b) — and why the guard is not (b)

**(b) Harden the inherited task** (run `tsc` only if a `tsconfig.json` exists) is rejected:
file presence alone **cannot distinguish** a legitimately config-only package (skip is correct)
from a real TS package that accidentally lost its `tsconfig.json` (must fail) — so (b) would
silently stop type-checking a genuine regression. It also adds wrapper-script indirection to the
hot path of *every* TS project's `build`/`typecheck`.

The **CI guard is the opposite of (b)**: it runs **once in CI** (not on any task's hot path), it
**fails loudly with an actionable message** (never silently skips), and it does not change the
canonical task command. So the two objections to (b) do not apply to it.

### The `layer` is `library`, not `configuration`

CONTRIBUTING's documented `layer` semantics reserve `configuration` for the **workspace-root
project that aggregates child projects** (e.g. `py/moon.yml`, `ts/moon.yml`). A shared config
*package* like `commitlint-config` is importable/published code (`@paigasus/commitlint-config`),
which the same doc classifies as `library`. So the `config` archetype keeps `layer: library`,
matching the existing `commitlint-config-ts`. **No `layer` is reclassified anywhere.**

## Design

### A. Scaffold template — `.moon/templates/typescript/`

**`template.yml`** — extend the `archetype` enum and document the third shape:

```yaml
  archetype:
    type: 'enum'
    values: ['library', 'app', 'config']
    default: 'library'
    prompt: 'Archetype?'
```

The `description` gains a sentence: `config` scaffolds a config-only package (not a `tsc` unit —
e.g. an eslint/prettier/commitlint config) that excludes the inherited `build`/`typecheck`.

**`moon.yml`** — add a `config` branch. `config` falls through to `layer: library` (the existing
`{% else %}`), so only the rendered body differs from `library`:

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

Field order stays `$schema`, `id`, `layer`, `language`, … with `workspace` trailing.

### B. CI enforcement guard

**Script — `ts/scripts/check-config-only.mjs`** (Node ESM; cross-platform; SPDX header). Algorithm:

1. Enumerate `language: typescript` Moon projects (from `moon query projects`, which emits JSON —
   note the `--json` flag is rejected, plain `moon query projects` already prints JSON).
2. For each whose source dir has **no** `tsconfig.json`, resolve its task list
   (`moon project <id> --json` → `.tasks`, which omits excluded tasks — verified:
   `commitlint-config-ts` resolves to `fmt`/`lint`/`test` only).
3. If `build` or `typecheck` is still present, it's a violation (config-only but not excluded, or a
   TS package that lost its `tsconfig.json`).
4. Exit non-zero listing each violator with an actionable message: *add
   `workspace.inheritedTasks.exclude: ['build','typecheck']` (scaffold archetype `config`) or a
   `tsconfig.json` — see CONTRIBUTING*. Exit 0 otherwise.

Representative implementation (exact `moon query` JSON field paths — top-level vs nested under
`.config` — are verified and pinned in the implementation plan):

```js
// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { join } from 'node:path'

const moonJson = (args) => JSON.parse(execFileSync('moon', args, { encoding: 'utf8' }))

const { projects } = moonJson(['query', 'projects'])
const ts = projects.filter((p) => p.language === 'typescript')

const violations = []
for (const p of ts) {
  if (existsSync(join(p.source, 'tsconfig.json'))) continue // a real tsc unit — fine
  const tasks = Object.keys(moonJson(['project', p.id, '--json']).tasks ?? {})
  if (tasks.includes('build') || tasks.includes('typecheck')) violations.push(p)
}

if (violations.length > 0) {
  console.error('Config-only TS packages must exclude the inherited build/typecheck:')
  for (const p of violations) console.error(`  - ${p.id} (${p.source}): no tsconfig.json, build/typecheck still inherited`)
  console.error("Fix: add workspace.inheritedTasks.exclude: ['build', 'typecheck'] (scaffold archetype `config`), or add a tsconfig.json. See CONTRIBUTING \"Moon project files\".")
  process.exit(1)
}
console.log(`config-only guard: ${ts.length} TS projects checked, no violations`)
```

**Moon task — `ts/moon.yml`** — add a `check-config-only` task mirroring the existing `commitlint`
task (explicit-invoke, uncached). Like `commitlint`, do **not** set `runInCI: false` (Moon would
then drop it under `CI=true`, making an explicit `moon run` resolve zero tasks and exit 1):

```yaml
tasks:
  # …existing commitlint task…
  check-config-only:
    # Enforces the config-only TS shape (SMA-396): a language:typescript project with no
    # tsconfig.json must exclude the inherited build/typecheck. Run-once guard, invoked
    # explicitly in CI (never via `moon ci`); converts a cryptic TS5058 into an actionable error.
    command: 'node scripts/check-config-only.mjs'
    inputs: []
    options:
      cache: false
```

**CI wiring — `.github/workflows/ci.yml`** — add an explicit step next to the commitlint step:
`moon run ts:check-config-only`. It is a whole-repo guard, so it is invoked explicitly (like
`ts:commitlint`), not via the `moon ci` affected target list.

**Known limitation (noted, not handled):** the guard keys on "no `tsconfig.json` + build/typecheck
still inherited." A package that *ships* a `tsconfig.json` but has no `.ts` inputs (a shared
`tsconfig` package → `TS18003`) is not caught; it still needs the exclude and would fail loudly on
its own. Rare; out of scope for the guard.

### C. Documentation — `CONTRIBUTING.md`

Add a **"Config-only TS packages:"** paragraph immediately after the existing app-build-invariant
paragraph (~line 164), in parallel structure:

> **Config-only TS packages:** a TypeScript *package* that is not a `tsc` compilation unit (no
> `.ts` sources — a CommonJS/JSON config such as a shared `eslint`/`prettier`/`commitlint` config;
> `commitlint-config` is the one today) MUST exclude the inherited per-project `build`/`typecheck`:
> `workspace.inheritedTasks.exclude: ['build', 'typecheck']`. Those tasks run
> `tsc -p tsconfig.json --noEmit`, which fails `TS5058` with no `tsconfig.json`. It stays
> `language: typescript` (so `lint`/`fmt`/`test` still attach) and `layer: library` (importable/
> published code). The TypeScript scaffold (`.moon/templates/typescript/`, archetype `config`)
> emits this block for you, and the `ts:check-config-only` CI guard fails the build with an
> actionable message if a config-only package is added without it.

The Notion Development Guidelines are synced to match **before this PR merges** (see Verification).

### D. Existing `commitlint-config-ts` — realign comment

`ts/packages/commitlint-config/moon.yml` already carries the correct `exclude` block. Update only
its comment (it currently says *"First use of this field in the repo; SMA-394 will later apply…"* —
both now historical) to mark it as the reference instance of the documented config-only shape
(point at CONTRIBUTING / SMA-396). **Comment-only; the `exclude` block and all other fields are
unchanged.**

## What deliberately stays

- The inherited `lint`/`fmt`/`test` on config-only packages (they pass on CJS/JSON; only
  `build`/`typecheck` are the problem). `language: typescript` is kept for exactly this.
- The `library` and `app` archetypes — unchanged in behavior; only the enum and description grow.

## Alternatives considered

- **(b) Harden the inherited `build`/`typecheck` task.** Rejected — file-presence can't distinguish
  config-only from a regressed TS package, so it risks silently skipping a real type-check, plus
  hot-path indirection on every TS task. The CI guard is explicitly *not* (b) (run-once, loud).
- **Documentation/scaffold only, no guard.** Rejected — paves but doesn't enforce; a hand-authored
  config package still red-fails CI with a cryptic `TS5058`, which is exactly the recurrence this
  issue exists to kill.
- **A separate `language: javascript` for config packages.** Rejected (as in SMA-395): the TS task
  file is scoped `inheritedBy.languages: ['typescript']`, so flipping language drops the wanted
  `lint`/`fmt`/`test` and changes affected-graph language semantics.

## Out of scope / non-goals

- **Adding a real second config package** (`eslint-config`, `prettier-config`, …). None is needed
  yet (YAGNI). The throwaway-generate verification proves the scaffold without leaving one behind.
- **The `TS18003` shared-tsconfig-package edge** for the guard (above) — rare; not handled.
- **Any `layer` reclassification** — `config` = `library` per the repo's documented semantics.

## Acceptance criteria

- [ ] Decision recorded as (a) + a CI guard (this spec).
- [ ] `.moon/templates/typescript/` gains a `config` archetype emitting
      `inheritedTasks.exclude: ['build', 'typecheck']` (+ comment); `template.yml` documents it.
- [ ] The config-only convention is documented in `CONTRIBUTING.md` and mirrored to the Notion
      Development Guidelines before merge.
- [ ] `ts:check-config-only` guard exists, is wired into CI, **passes** on the current tree, and
      **fails with an actionable message** for a config-only TS project that lacks the exclude.
- [ ] A config-only package generated from the `config` archetype has **no** `build`/`typecheck`
      task and does **not** fail a full `moon ci :build` / `:typecheck` out of the box.
- [ ] `commitlint-config-ts` still resolves with no `build`/`typecheck` and keeps `lint`/`fmt`/
      `test` (no regression from the comment realign).

## Verification plan

`moon` is proto-managed; if a shell reports `moon: command not found`, prefix with
`export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"`.

1. **Guard passes on the current tree:**
   ```bash
   moon run ts:check-config-only   # expect: "...no violations", exit 0
   ```
2. **Generate a config package from the new archetype:**
   ```bash
   moon generate typescript ts/packages/scratch-config -- --name scratch-config --archetype config
   cat ts/packages/scratch-config/moon.yml   # expect layer: library, language: typescript, exclude block
   moon project scratch-config-ts | grep -iE 'build|typecheck' || echo "no build/typecheck ✓"
   moon run ts:check-config-only   # still passes (scaffolded package has the exclude)
   moon run :build :typecheck 2>&1 | grep -iE 'TS5058|failed' || echo "no TS5058, 0 failed ✓"
   ```
3. **Guard FAILS for a non-excluded config package** (prove enforcement): remove the exclude block
   from `ts/packages/scratch-config/moon.yml`, then:
   ```bash
   moon run ts:check-config-only   # expect: lists scratch-config-ts, actionable message, exit 1
   ```
4. **Remove the throwaway and its Moon cache residue** (an empty `git status` does NOT prove a
   pristine workspace — `.moon/cache/` is gitignored, so generated cache-state survives a plain
   `rm`; prior `throwaway-*`/`review-throwaway-*` residue under `.moon/cache/states/` is exactly
   this leak):
   ```bash
   rm -rf ts/packages/scratch-config
   moon clean    # drop scratch-config-ts cache state / graph entries
   git status --short ts/   # expect empty (necessary, not sufficient)
   ```
5. **`commitlint-config-ts` unchanged in behavior:**
   ```bash
   moon project commitlint-config-ts | grep -iE 'build|typecheck' || echo "still excluded ✓"
   ```
   Expect: no `build`/`typecheck`; `lint`/`fmt`/`test` still present.
6. **Lint/fmt are clean** (the new `.mjs` lives under the ts lint scope):
   ```bash
   moon run ts:lint ts:fmt   # expect 0 failed
   ```
7. **Notion sync (pre-merge):** mirror the "Config-only TS packages" convention (and confirm the
   app-build invariant is present) into the Notion Development Guidelines page, so the external doc
   does not drift from CONTRIBUTING.

## Files touched

- `.moon/templates/typescript/template.yml` — add `config` to the `archetype` enum; document it.
- `.moon/templates/typescript/moon.yml` — add the `{% elif archetype == "config" %}` exclude branch.
- `ts/scripts/check-config-only.mjs` — **new** guard script (SPDX header).
- `ts/moon.yml` — add the `check-config-only` task (mirrors `commitlint`: `inputs: []`, uncached).
- `.github/workflows/ci.yml` — add an explicit `moon run ts:check-config-only` step.
- `CONTRIBUTING.md` — add the "Config-only TS packages" paragraph after the app-build invariant.
- `ts/packages/commitlint-config/moon.yml` — comment realign only (exclude block unchanged).
- Notion Development Guidelines — mirror the convention (external; done pre-merge).
