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
- `paigasus-docs-ts:build` → `tsc --noEmit` (inherited, no files; docs has no `moon.yml` and no `package.json` build script)

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
  command: 'pnpm --filter "./packages/**" --if-present run build'   # was: pnpm -r --if-present run build
  # inputs / options unchanged
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

Only the task `command` changes. `inputs`, `options.merge: replace`, and the surrounding
`ts/moon.yml` are otherwise untouched.

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
  never collides — only minor wasted work. Not this issue's concern; note as a possible later
  cleanup, do not change here.

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

- `ts/moon.yml` — change the `build` task `command` only (one line).
