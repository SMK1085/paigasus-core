# SMA-395 — `commitlint-config-ts:build` fails: inherited `tsc` task but no `tsconfig.json`

**Status:** Design approved
**Date:** 2026-05-29
**Linear:** SMA-395
**Branch:** `feature/sma-395-ts-commitlint-config-tsbuild-fails-inherited-tsc-build-task`

## Problem

A full-graph build fails on the per-project `commitlint-config-ts:build` task:

```
error TS5058: The specified path does not exist: 'tsconfig.json'.
× Task commitlint-config-ts:build failed to run.
```

Moon also warns it cannot hash inputs for the missing `tsconfig.json`. The same failure
applies to `commitlint-config-ts:typecheck` (identical command).

Pre-existing; discovered while implementing SMA-391 (reproduces on the base commit, unrelated
to that fix). It hasn't bitten CI yet because the old recursive `ts:build`
(`pnpm -r --if-present run build`) only ran `package.json` `build` scripts — and this package
has none — so the per-project inherited task is only reached by a full `moon run :build` /
`moon ci :build`. SMA-361's CI will surface it.

## Root cause

`ts/packages/commitlint-config` is a pure **CommonJS config package**: `index.cjs`, no `.ts`
sources, no `tsconfig.json`, no `build` script in `package.json`. Its `moon.yml` declares
`language: 'typescript'`, so it inherits the per-project `build` **and** `typecheck` tasks from
`.moon/tasks/typescript.yml`:

```yaml
build:
  command: 'pnpm exec tsc -p tsconfig.json --noEmit'
typecheck:
  command: 'pnpm exec tsc -p tsconfig.json --noEmit'   # identical command
```

With no `tsconfig.json` for `-p` to point at, `tsc` exits non-zero (`TS5058`) → both inherited
tasks fail.

It is the **only** TypeScript project in this state: the other five packages
(`paigasus-kernel`, `-proto`, `-sdk`, `-ui`) and both apps (`paigasus-console`, `paigasus-docs`)
each ship their own `tsconfig.json`, so the inherited `tsc --noEmit` resolves fine for them.

Note the `ts/moon.yml` root project *does* override `build`/`typecheck` with `merge: replace`
aggregators — but that override is scoped to the `ts` root project itself and does **not**
propagate to child projects. `commitlint-config-ts` inherits the raw `tsc -p tsconfig.json`
form directly from `.moon/tasks/typescript.yml`. That direct inheritance is the failure.

## Decision

Opt `commitlint-config` out of the two inherited tasks that have no target here, using the
project-level `workspace.inheritedTasks.exclude` field. A CJS config package is not a TS
compilation unit; excluding `build`/`typecheck` is the semantically honest fix, and it satisfies
the AC's "or the project is correctly excluded from those tasks" branch.

```yaml
# ts/packages/commitlint-config/moon.yml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'commitlint-config-ts'
layer: 'library'
language: 'typescript'

# Pure CommonJS config package (index.cjs): no tsconfig.json and nothing to
# compile or type-check. It stays `language: typescript` only so lint/fmt/test
# still attach. Opt out of the inherited per-project `build`/`typecheck` tasks
# (.moon/tasks/typescript.yml runs `tsc -p tsconfig.json --noEmit`, which fails
# TS5058 with no tsconfig.json). Same mechanism SMA-394 applies to the `ts` root.
workspace:
  inheritedTasks:
    exclude: ['build', 'typecheck']
```

### Field verified

`workspace.inheritedTasks.exclude` is a valid **project-level** field in Moon (confirmed against
moonrepo.dev project-config docs): it lives under a top-level `workspace:` key, accepts a list of
task names, and removes those inherited tasks from the project. This is the same mechanism
SMA-394 will apply to the `ts` root — so the fix pre-sets that pattern rather than inventing one.

### What deliberately stays

`lint`, `fmt`, `test` remain inherited and run fine on a lone `.cjs`
(`eslint .`, `prettier --check .`, `vitest run --passWithNoTests`). They are not part of the
failure, so we exclude **only** the two broken tasks rather than dropping the whole TS toolchain
(which is why `language: typescript` is kept rather than flipped to `javascript`).

## Why this stays isolated

The exclusion lives on `commitlint-config-ts` only and touches no other project. The root
`ts:build` / `ts:typecheck` aggregators are untouched: they skip this package anyway (it has no
`build`/`typecheck` *script*, so `pnpm -r --if-present` / `--filter` pass over it). SMA-394's
separate structural cleanup of those aggregators stays separate (decided in brainstorm: keep
SMA-395 narrow).

## Alternatives considered

- **Add a minimal `tsconfig.json`** (`extends: ../../tsconfig.base.json`). Rejected: with no
  `.ts` files, `tsc` errors `TS18003` ("No inputs were found") unless given `files: []`, at which
  point the file exists solely to make a task that does nothing succeed — clutter, and dishonest
  about the package having any TypeScript.
- **`merge: replace` no-op task overrides.** Rejected: adds task noise to the project config and
  Moon has no clean builtin no-op (you'd lean on `true`, which is platform-fragile).
- **Flip `language` to `javascript`.** Rejected: `.moon/tasks/typescript.yml` is scoped
  `inheritedBy.languages: ['typescript']`, so this drops *all* inherited tasks including the
  wanted lint/fmt/test, and changes the project's language semantics for affected-graph detection.

## Out of scope

- SMA-394's removal of the recursive `ts:build` / `ts:typecheck` aggregators and the `ts`-root
  `inheritedTasks.exclude`. Separate, lower-priority issue; do not change `ts/moon.yml` here.
- `lint` / `fmt` behavior on `index.cjs` — currently passes; not part of this failure.

## Acceptance criteria

- [ ] `commitlint-config-ts` no longer has `build`/`typecheck` tasks (correctly excluded);
      `moon run :build` / `:typecheck` no longer emit `TS5058` for it.
- [ ] A full cold `moon run :build` completes with **0 failures** (no `TS5058`).
- [ ] `commitlint-config-ts` retains its inherited `lint` / `fmt` / `test` tasks.

## Verification plan

1. **Inspect the resolved task list**:
   ```bash
   moon project commitlint-config-ts
   ```
   Expect: no `build` and no `typecheck`; `lint`, `fmt`, `test` still present.
   (`moon run commitlint-config-ts:build` now reports an unknown task — expected, the
   "correctly excluded" AC branch, not a failure.)
2. **Cold full build graph**:
   ```bash
   moon run :build      # expect all complete, 0 failed, no TS5058
   ```
3. **Cold full typecheck graph**:
   ```bash
   moon run :typecheck  # expect all complete, 0 failed, no TS5058
   ```

## Files touched

- `ts/packages/commitlint-config/moon.yml` — add the `workspace.inheritedTasks.exclude:
  ['build', 'typecheck']` block plus the explanatory comment. No other file changes.
