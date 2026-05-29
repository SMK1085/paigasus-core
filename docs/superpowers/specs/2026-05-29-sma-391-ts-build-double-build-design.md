# SMA-391 — Fix `ts:build` double-building `paigasus-console` (Next.js `.next` lock collision)

**Status:** Design approved
**Date:** 2026-05-29
**Linear:** SMA-391
**Branch:** `feature/sma-391-tsbuild-double-builds-paigasus-console-concurrently-nextjs`

## Problem

`moon ci :build` intermittently fails with:

```
ts:build | apps/paigasus-console build: ⨯ Another next build process is already running.
ERR_PNPM_RECURSIVE_RUN_FIRST_FAIL  @paigasus/console@0.0.0 build: `next build`
× Task ts:build failed to run.
```

The `paigasus-console` Next.js app is built **twice, concurrently, into the same `.next`**.
Next.js 16 holds a build lock on `.next`; on a cold cache Moon schedules both builders at
once and the second hits the lock.

## Root cause

The TypeScript `:build` graph contains two tasks that both build the console:

| Task | Command | Effect |
|---|---|---|
| `ts:build` (`ts/moon.yml`) | `pnpm -r --if-present run build` | recurses the whole pnpm workspace; the **only** `package.json` with a `build` script is `apps/paigasus-console` (`next build`) → builds `.next` |
| `paigasus-console-ts:build` (`ts/apps/paigasus-console/moon.yml`) | `pnpm exec next build`, `outputs: ['.next']` | builds `.next` |

Both write `.next`. On a cold cache they run concurrently → lock collision. It is intermittent
because it only collides when **both** miss cache and execute simultaneously; if either is warm
(the common case) there is no collision, so it surfaces only in cold full `moon ci :build` runs.

### Wider context discovered during brainstorm

Every TypeScript project **already inherits a per-project `build` task** from
`.moon/tasks/typescript.yml` (`pnpm exec tsc -p tsconfig.json --noEmit`). The full cold
`:build` graph is therefore:

- `ts:build` → `pnpm -r --if-present run build` → only reaches the console's `next build`
- `paigasus-console-ts:build` → `next build` (override) → `.next` — **collides with `ts:build`**
- `paigasus-kernel-ts:build`, `-sdk`, `-proto`, `-ui`, `-commitlint-config` → `tsc --noEmit` (inherited, no files)
- `paigasus-docs-ts:build` → `tsc --noEmit` (inherited, no files). Note: `ts/apps/paigasus-docs/moon.yml`
  **does** exist (`id: paigasus-docs-ts`, `layer: library`, `language: typescript`) but defines **no
  `build` override**, so it inherits the `tsc --noEmit` form; its `package.json` has no `build` script.
  (Latent oddity, not fixed here: `paigasus-docs` lives under `apps/` but is typed `layer: library`. The
  fix's scope is *directory*-based — the `./apps/**` vs `./packages/**` pnpm path filter — so docs is
  treated as an app regardless of its `layer:`. Worth a separate cleanup.)

Implications:

1. **Library packages are already covered** by their own inherited per-project Moon `build`
   tasks (`tsc --noEmit`) — *not* by the recursive `ts:build`. The recursive `pnpm -r run build`
   only ever executes `package.json` `build` scripts, of which the console is the sole one.
2. The recursive `ts:build` is thus **redundant for packages and harmful for the app** — its
   single real effect today is the duplicate, colliding console build.

## Decision

Scope the parent recursive build to **library packages only**, and let the per-app Moon task
own the app build. This removes the duplicate entirely (also faster) rather than merely
serializing the two builders.

```yaml
# ts/moon.yml — build task
build:
  # Library packages only — apps own their build via per-app Moon tasks (see paigasus-console).
  # `typecheck` below intentionally still uses `pnpm -r` (whole tree): tsc --noEmit writes nothing
  # and holds no lock, so its double-run is harmless. The asymmetry is deliberate, not an oversight;
  # both are slated to converge in the structural follow-up (see "Follow-up").
  command: 'pnpm --filter "./packages/**" --if-present run build'   # was: pnpm -r --if-present run build
  inputs:
    - '@group(sources)'
    - 'tsconfig.base.json'
    - 'packages/**/tsconfig.json'   # `apps/**/tsconfig.json` dropped — the task no longer touches apps
    - 'package.json'
    - 'pnpm-workspace.yaml'
    - 'pnpm-lock.yaml'
  options:
    merge: replace
```

### The `--if-present` flag is mandatory (not in the issue's snippet)

Verified empirically — with no `packages/*` defining a `build` script today:

```
$ pnpm --filter "./packages/**" run build
 ERR_PNPM_RECURSIVE_RUN_NO_SCRIPT  None of the selected packages has a "build" script
 exit: 1
$ pnpm --filter "./packages/**" --if-present run build
 exit: 0
```

The issue's proposed snippet (`pnpm --filter "./packages/**" run build`) omits `--if-present`
and would therefore make `ts:build` **fail on every run** today. The fix must include it.

### What `ts:build` becomes

A clean **no-op aggregator today** (no `packages/*` has a `package.json` `build` script). It is
retained as a forward-looking hook: any future library package that needs a real artifact build
step (e.g. `tsup`/bundling beyond plain `tsc --noEmit`) gets picked up automatically by adding a
`build` script to its `package.json`. Apps (`paigasus-console`, future `paigasus-docs`) build via
their own `<app>-ts:build` Moon tasks, which Moon already orders and caches independently.

The task `command` changes and the now-irrelevant `apps/**/tsconfig.json` input is dropped.
`options.merge: replace` and the rest of `ts/moon.yml` are otherwise untouched.

## Invariant this fix establishes

Excluding `apps/**` from the recursive build converts the old failure mode ("apps double-built,
loud and intermittent") into a new one ("an app with no Moon `build` override is **silently never
built** by `moon ci :build`"). The accidental `pnpm -r` safety net is gone. So:

> **Every `apps/*` that produces a build artifact MUST define its own Moon `build` task (with
> `outputs:`), exactly as `paigasus-console` does.** Otherwise it only inherits `tsc --noEmit` and
> emits nothing.

`paigasus-docs` is the live near-miss: today it's a stub with no `build` script, so there's no
regression — but the day it becomes a real docs site it must add a Moon `build` override or its
output will silently never be produced. This invariant should be recorded in the TS app
scaffold/template and CONTRIBUTING (out of scope for this one-line fix; capture as the follow-up
below), and ideally asserted by SMA-361 CI (each app emits its expected output).

## Alternatives considered

- **Exclude apps via negation** — `pnpm -r --if-present --filter "!./apps/**" run build`.
  Same end effect; rejected for clarity (positive "packages only" reads better than a `!`-filter,
  and matches the issue's intent).
- **Serialize the two builders** (Moon dependency ordering). Rejected: still double-builds the
  console and is slower; doesn't address the redundancy.
- **Remove the `ts:build` override entirely** (fall back to inherited `tsc --noEmit`). Rejected:
  `ts/` has no `tsconfig.json` (only `tsconfig.base.json`), so the inherited per-project build
  would fail at the workspace root.

## Out of scope

- `ts:typecheck` has the same double-run shape (`pnpm -r run typecheck` recursion **plus** the
  inherited per-project `typecheck`), but `tsc --noEmit` writes nothing and holds no lock, so it
  never collides — only minor wasted work. Not this issue's concern; folded into the follow-up
  below, do not change here.

## Follow-up (structural fix — tracked as SMA-394, decided not to do here)

This one-liner fully resolves the flaky-CI bug and is safe to ship, but it is a **fence around the
cause, not its removal**. The recursive `ts:build` survives as a "forward-looking
hook," and that hook is precisely the mechanism that recreates the bug: a future `packages/*` that
gains a real artifact build (e.g. `"build": "tsup"`) will, following the console's own pattern,
override its per-project Moon `build` (with `outputs:`) **and** still be picked up by `ts:build`'s
pnpm recursion → the same two-builders-into-one-output collision. Since library packages are
already covered by their inherited per-project Moon tasks, the recursive aggregator covers nothing
Moon's per-project model doesn't already own.

The structural fix (deferred by decision, no `packages/*` triggers it today):

- Drop the recursive `ts:build` aggregator; let `moon ci :build`'s per-project fan-out own the
  whole graph. Use `workspace.inheritedTasks.exclude: ['build']` on the `ts` root project so it
  doesn't fall back to the inherited `tsc --noEmit` (which fails — no root `tsconfig.json`).
- Apply the same treatment to `ts:typecheck` so the two sibling aggregators stay parallel.
- Record the app-build invariant (above) in CONTRIBUTING + the TS app scaffold template.

This touches the workspace-aggregator pattern introduced in SMA-359, so it gets its own issue.

## Acceptance criteria

- [ ] A cold `moon run ts:build paigasus-console-ts:build --force` succeeds with no `.next`
      lock collision.
- [ ] A cold full `moon ci :build` succeeds (no "Another next build process is already running").
- [ ] `paigasus-console` is built exactly once per `moon ci :build`.
- [ ] Library packages remain covered (by their inherited per-project Moon `build` tasks;
      `ts:build` continues to cover any future `packages/*` that adds a `package.json` `build`
      script).

## Verification plan

1. **Deterministic repro before the fix** (confirm the collision):
   ```bash
   rm -rf ts/apps/paigasus-console/.next
   moon run ts:build paigasus-console-ts:build --force   # expect lock failure
   ```
2. **After the fix** — same command succeeds:
   ```bash
   rm -rf ts/apps/paigasus-console/.next
   moon run ts:build paigasus-console-ts:build --force   # expect success
   ```
3. **`ts:build` no-ops cleanly** (exit 0, no console build in its output).
4. **Cold full graph**:
   ```bash
   rm -rf ts/apps/paigasus-console/.next
   moon ci :build                                        # expect all complete, 0 failed
   ```
5. Confirm `.next` is produced exactly once (by `paigasus-console-ts:build` only).

## Files touched

- `ts/moon.yml` — `build` task only: change `command` to the packages-scoped + `--if-present`
  form, drop the now-irrelevant `apps/**/tsconfig.json` input, and add the comment explaining
  the deliberate `build`/`typecheck` asymmetry. No other task or file changes.
