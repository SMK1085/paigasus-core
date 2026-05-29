# SMA-394 — Let Moon own the TS build/typecheck graph (drop the recursive pnpm aggregators)

**Status:** Design approved
**Date:** 2026-05-29
**Linear:** SMA-394
**Branch:** `feature/sma-394-ts-let-moon-own-the-ts-buildtypecheck-graph-drop-recursive`

## Problem

The `ts` root project (`ts/moon.yml`, `layer: configuration`) overrides two tasks with
recursive pnpm aggregators:

```yaml
tasks:
  typecheck:
    command: 'pnpm -r --if-present run typecheck'          # whole pnpm workspace
  build:
    command: 'pnpm --filter "./packages/**" --if-present run build'   # packages only (SMA-391)
```

SMA-391 narrowed `build` from `pnpm -r` to `pnpm --filter "./packages/**"` to stop
`paigasus-console` building twice into one `.next` (Next.js build-lock collision), but
deliberately left the aggregator in place as a "forward-looking hook." That retained
aggregator **re-creates the same bug class it was meant to live beside**: when a future
`packages/*` gains a real artifact build (e.g. `"build": "tsup"`), it will — following the
console's own pattern — override its per-project Moon `build` (with `outputs:`) **and** still
be picked up by `ts:build`'s pnpm recursion → the same two-builders-into-one-output collision.

`ts:typecheck` carries the identical double-run shape. It is harmless today (`tsc --noEmit`
writes nothing and holds no lock) but the asymmetry with the now packages-scoped `build` is a
standing readability cost.

Both aggregators are also **redundant**: every TS project already inherits per-project `build`
and `typecheck` from `.moon/tasks/typescript.yml` (`pnpm exec tsc -p tsconfig.json --noEmit`),
and apps override `build` with their own `outputs:`. Moon's per-project fan-out already owns the
graph; the recursive aggregators cover nothing the per-project model doesn't.

## Root cause / context

`.moon/tasks/typescript.yml` attaches `build`, `typecheck`, `lint`, `fmt`, `test` to **every**
`language: typescript` project (scoped `inheritedBy.languages: ['typescript']`). The inherited
`build` and `typecheck` are both `pnpm exec tsc -p tsconfig.json --noEmit`.

So `moon run :build` / `moon run :typecheck` already run a per-project task on each of the 4
libs (`paigasus-kernel`, `-proto`, `-sdk`, `-ui`) and 2 apps. `paigasus-console` overrides
`build` with `next build` + `outputs: ['.next']`; `commitlint-config` is excluded from both
(SMA-395, a config-only CJS package with no `tsconfig.json`). The recursive `ts:*` aggregators
sit *on top of* that fan-out, duplicating it.

The one snag with simply deleting the overrides: the `ts` root is itself `language: typescript`,
so it would then **fall back** to the inherited `tsc -p tsconfig.json --noEmit` run from `ts/`.
`ts/` has only `tsconfig.base.json` — no `tsconfig.json` — so that inherited task fails `TS5058`
(exactly the failure SMA-395 fixed for `commitlint-config`). The fix must therefore *exclude* the
inherited build/typecheck at the root, not merely omit the overrides.

## Decision

Remove the `tasks:` block entirely and **exclude** the two inherited tasks that have no valid
target at the root, using the same `workspace.inheritedTasks.exclude` field `commitlint-config`
already uses (whose comment forward-references this issue). The root keeps `language:
typescript` and its inherited whole-tree `lint`/`fmt`/`test`; Moon's per-project fan-out owns
the entire `:build`/`:typecheck` graph.

> **Not a "mirror py."** An earlier draft justified this as mirroring `py/moon.yml`. That framing
> was wrong and is removed. `py/moon.yml` is `layer: configuration`, `language: python`, and
> defines only `fileGroups` — but it does **not** exclude anything; it *inherits* `build:
> uv build` and `typecheck: uv run basedpyright` from `.moon/tasks/python.yml`. `py:build` even
> succeeds today (verified: exit 0, ~940ms) but builds a meaningless `unknown-0.0.0` wheel,
> because `py/pyproject.toml` has `[tool.uv.workspace]` and **no `[project]` table**. So `py` is
> in the *same pre-fix shape* this issue removes from `ts`; after this change the two roots
> **diverge** on exactly the field being added. `py` is a follow-up twin (see Out of scope), not
> the model. The justification for the `ts` exclude stands on its own: without it the root falls
> back to the inherited `tsc -p tsconfig.json --noEmit`, which fails `TS5058` (no root
> `tsconfig.json`).

### `ts/moon.yml` end state

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

layer: 'configuration'
language: 'typescript'

# The inherited fileGroups from .moon/tasks/typescript.yml assume src/ at the project root.
# The ts workspace keeps sources under packages/*/src and apps/*/{src,app}, so extend the
# inherited groups here. Moon merges (not overrides) fileGroups across the layers; the
# resolved @group(sources)/@group(tests) feed the inputs of the inherited whole-tree
# lint/fmt/test that still run from ts/.
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

Field order follows the CONTRIBUTING rule ($schema, layer, language, fileGroups, … then
remaining fields → `workspace` trailing).

### What deliberately stays

- `language: typescript` — kept so the root still inherits `lint`/`fmt`/`test`.
- The inherited whole-tree `lint` (`eslint .`), `fmt` (`prettier --check .`), and `test`
  (`vitest run --passWithNoTests`) on the root. These walk from `ts/` cwd and cover the whole
  tree; left unchanged.
- `fileGroups` — still feed the input globs (hence cache invalidation) of those inherited tasks.

### Field already proven

`workspace.inheritedTasks.exclude` is a valid project-level field in Moon 2.2.5, established and
verified in the repo by SMA-395 on `commitlint-config-ts`. This change is the second use of the
same field, in the exact place SMA-395's comment said it would land.

## `paigasus-docs` layer fix

`ts/apps/paigasus-docs/moon.yml` is `layer: library` despite living under `apps/`. Flip it to
`layer: application`. This is functionally inert — task inheritance keys on `language`, not
`layer`, so no task graph changes — and is purely an honest classification. `paigasus-docs`
produces no build artifact yet (it has only a `typecheck` script and inherits the no-op
`tsc --noEmit` build), so reclassifying does **not** trip the app-build invariant documented
below; it will need its own `build` + `outputs:` task only once it grows a real docs build.

Nothing in the repo depends on `paigasus-docs` (grep-confirmed), so Moon's
`constraints.enforceProjectTypeRelationships` — on by default; under it an `application` may not
be *depended upon* by another project — stays satisfied by the flip. A future `apps/*`
reclassification should re-check dependents, since that constraint, not task inheritance, is the
one dimension `layer` actually affects.

## Documentation — the app-build invariant

Every `apps/*` that produces a build artifact MUST define its own Moon `build` task with
`outputs:` (as `paigasus-console` does). Otherwise it only inherits `tsc -p tsconfig.json
--noEmit`, which type-checks but **silently emits nothing** — a green build that produces no
artifact. Document this in two places:

- **CONTRIBUTING.md** — a short note in/near the "### Moon project files" section stating the
  invariant.
- **`.moon/templates/typescript/moon.yml`** — add a comment in the `app` archetype branch
  explaining *why* the branch emits a `build` task with `outputs:` (today it comments only on
  the `next.config.*` extension list), and strengthen the `template.yml` `description` to state
  the invariant. The template already emits the correct task, so this is documentation of an
  already-enforced rule, not a behavior change.

## README fallout (necessary, beyond the ACs)

`ts/README.md` currently instructs `moon run ts:typecheck` and `moon run ts:build` — both
targets disappear when the root overrides are removed. Update:

- The command table: `moon run ts:typecheck` → `moon run :typecheck --query "language=typescript"`,
  and `moon run ts:build` → `moon run :build --query "language=typescript"`. **The query is
  required** — a *bare* `moon run :build`/`:typecheck` runs the task in every project across all
  languages (rust + py both define `build`), so in a TS-workspace README it must be scoped to
  `language=typescript` to preserve the old `ts:build` intent. (`--query` is a real `moon run`
  flag; the query was verified to resolve only the TS projects.) **Relabel the "Build (libs)"
  row** to "Build (all TS)" — `:build` runs every TS project's build (libs' no-op `tsc --noEmit`
  *and* apps' real builds). Keep the per-app `moon run paigasus-console-ts:build` row, and leave
  `lint`/`fmt`/`test` on `moon run ts:<task>` (those tasks still live on the root).
- The Layout-section `moon.yml` bullet (the one reading *"Owns workspace-wide `typecheck` and
  `build` (recursive `pnpm -r ...`)"*): reword so it no longer states the removed behavior —
  the root excludes build/typecheck (Moon's per-project tasks own them) while it still owns
  whole-tree lint/fmt/test.

The general guidance line ("invoke pnpm via `moon run ts:<task>`") stays valid for the tasks
that remain on the root (`ts:lint`, `ts:fmt`, `ts:test`).

## Alternatives considered

- **Redefine root `build`/`typecheck` as no-op tasks** (`merge: replace` with a `true`-style
  command). Rejected: Moon has no clean builtin no-op, `true` is platform-fragile, and it adds
  task noise. `inheritedTasks.exclude` is the established repo idiom (SMA-395).
- **Drop `language: typescript` from the root.** Rejected: that also removes the inherited
  whole-tree `lint`/`fmt`/`test` and changes affected-graph language semantics — a much larger
  blast radius than the targeted exclude, and it diverges from `py/moon.yml`.
- **Keep the aggregators, just document the hazard.** Rejected: leaves the latent
  two-builders-into-one-output collision in place, which is the whole point of the issue.

## Out of scope / non-goals

- **`lint`/`fmt`/`test` root-vs-per-project redundancy.** The root runs these whole-tree while
  each project also runs them per-dir. This is pre-existing, repo-wide (`py` does the same), and
  not part of this issue. Left untouched.
- **CI asserting each app emits its expected output.** The issue suggests SMA-361 "ideally"
  assert this. `.github/workflows` is still an empty `.gitkeep`; the assertion is deferred to
  **SMA-361** and noted there, not built here.
- **Promoting `inheritedTasks.exclude` to a documented config-only-package convention.** That is
  SMA-396's recurring-shape problem; not subsumed here.
- **`py/moon.yml` is the same pre-fix shape (py twin).** The py configuration root inherits
  `build`/`typecheck` without excluding them, and `py:build` runs `uv build` at a
  `[project]`-less workspace root → a meaningless `unknown-0.0.0` wheel. It *succeeds* (exit 0),
  so it does **not** block this issue's AC — but it is the py-side twin of exactly this cleanup.
  File a follow-up to apply `inheritedTasks.exclude: ['build', 'typecheck']` to `py/moon.yml`;
  deliberately **not** folded into this TS issue (same scope discipline that spun the config-only
  convention out to SMA-396, and that kept SMA-391 narrow).

## Acceptance criteria

- [ ] No recursive `pnpm -r` / `pnpm --filter` build or typecheck aggregator remains in
      `ts/moon.yml`; Moon's per-project tasks own the full `:build` and `:typecheck` graphs.
- [ ] `moon ci :build` and `moon ci :typecheck` (the affected-graph form PR CI runs) succeed,
      and `paigasus-console` builds exactly once. Whole-graph `moon run :build` /
      `moon run :typecheck` are also green today (`py:build` verified passing), though their
      greenness spans the whole repo, not just this change.
- [ ] The `ts` root project no longer fails on an inherited `tsc --noEmit` (inheritance
      excluded — `moon project ts` shows no `build`/`typecheck`).
- [ ] App-build invariant documented in CONTRIBUTING + the TS app scaffold template.
- [ ] `paigasus-docs` is `layer: application`.

## Verification plan

1. **Inspect the resolved root task list:**
   ```bash
   moon project ts
   ```
   Expect: no `build`, no `typecheck`; `lint`, `fmt`, `test` still present.
   `moon run ts:build` / `moon run ts:typecheck` now report an unknown task — expected (proves
   the aggregators are gone), not a failure.
2. **Cold full build graph:**
   ```bash
   moon run :build      # expect 0 failed; paigasus-console's `next build` appears exactly once
   ```
   Confirm `ts/apps/paigasus-console/.next` exists and is produced by `paigasus-console-ts:build`
   only.
3. **Cold full typecheck graph:**
   ```bash
   moon run :typecheck  # expect 0 failed, no TS5058 from the root
   ```
4. **Affected-graph CI form** (matches what PR CI runs):
   ```bash
   moon ci :build
   moon ci :typecheck
   ```

## Files touched

- `ts/moon.yml` — remove the `tasks:` block (both `typecheck` and `build` overrides); add
  `workspace.inheritedTasks.exclude: ['build', 'typecheck']` with the explanatory comment; keep
  `fileGroups` and `language: typescript`.
- `ts/apps/paigasus-docs/moon.yml` — `layer: library` → `layer: application`.
- `CONTRIBUTING.md` — document the app-build invariant.
- `.moon/templates/typescript/moon.yml` — comment the `app` branch's `build`+`outputs:` task
  with the invariant rationale.
- `.moon/templates/typescript/template.yml` — strengthen the `description` to state the invariant.
- `ts/README.md` — update the command table (`ts:build`/`ts:typecheck` →
  `moon run :build --query "language=typescript"` / `:typecheck --query ...`), the "gates"
  sentence, the Notes section, and the `moon.yml` Layout bullet.
