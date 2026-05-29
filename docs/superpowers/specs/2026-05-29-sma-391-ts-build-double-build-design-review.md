# Review — SMA-391 `ts:build` double-build fix

**Reviews:** [`2026-05-29-sma-391-ts-build-double-build-design.md`](./2026-05-29-sma-391-ts-build-double-build-design.md)
**Reviewer perspective:** staff engineer
**Date:** 2026-05-29
**Sources cross-referenced:** Linear SMA-391 (+ related SMA-359/361/371), and the live tree (`ts/moon.yml`, `.moon/tasks/typescript.yml`, `ts/apps/*`, `ts/packages/*`).

## Verdict

The diagnosis is correct, the one-line fix is safe to ship, and the spec adds real value over the Linear issue (it catches that the issue's own proposed snippet omits `--if-present` and would fail on every run). Ship it.

My staff-level pushback is not about the fix's correctness but its framing: it is a local patch that narrows the blast radius while **keeping the structural cause** — a Moon task that shells out to `pnpm -r`, which is opaque to Moon's scheduler. That opacity is *why* the console got built twice, and the spec's "forward-looking hook" rationale re-creates the exact same bug class for the next package that grows a real build. There is also a new, opposite-direction gap (apps can now go *un-built*) that the spec doesn't name, and one factual error about the docs app. None block the immediate fix; all should shape the follow-up.

## What the spec gets right (calibration)

- **Root cause is accurate and verified.** `ts/moon.yml build = pnpm -r --if-present run build` (with `merge: replace`), the inherited per-project `build = pnpm exec tsc -p tsconfig.json --noEmit` (no `outputs`), and the console override `next build` / `outputs: ['.next']` are all exactly as described. `ts/apps/paigasus-console/package.json` is confirmed the **sole** `package.json` with a `build` script.
- **The `--if-present` catch is the headline value-add.** Verified: `pnpm --filter "./packages/**" run build` exits 1 (`ERR_PNPM_RECURSIVE_RUN_NO_SCRIPT`) today; the issue's snippet would have made `ts:build` fail on every run. Requiring `--if-present` is correct.
- **Both rejected alternatives are correctly reasoned.** Serialization still double-builds; removing the override falls back to inherited `tsc -p tsconfig.json --noEmit` at the `ts/` root, which has only `tsconfig.base.json` (no `tsconfig.json`) — confirmed, so that path would indeed fail.
- **Verification plan is good** — deterministic before/after repro plus a cold full `moon ci :build`.

## Findings

### F1 — [Medium-High] The retained `ts:build` hook re-introduces the same bug class for future packages

The spec keeps `ts:build` as a "forward-looking hook": when a future `packages/*` adds a `package.json` `build` script, the recursive task picks it up automatically. But the established pattern in this very repo (the console) is that a package needing a real build *also overrides its own Moon `build` task* — because the inherited `tsc --noEmit` isn't a real build, and Moon needs `outputs` declared for caching. So a future `paigasus-kernel` with `"build": "tsup"` would be built by **both** `ts:build` (via pnpm recursion) **and** `paigasus-kernel-ts:build` (its Moon override) — the identical two-builders-into-one-output shape that caused this ticket. If that build writes to a lock-guarded or non-atomic output dir, you get a fresh collision; if not, you get silent duplicate work and racy writes to the same `dist/`.

In other words, the hook's stated purpose (auto-pick-up future package builds) is the precise mechanism that recreates the bug it's meant to live alongside. And per the spec's own implication #1, library packages are *already* covered by their inherited per-project Moon tasks — so the recursive aggregator covers nothing that Moon's per-project model doesn't already own, for both current and future packages.

**Recommendation (follow-up, not a blocker):** remove the aggregator build entirely, or make it a genuine Moon no-op that does not shell to pnpm, and let each project's own Moon `build` task be the single source of truth (override per package when a real artifact build is needed, exactly as the console does). The spec considered "remove the override → inherit `tsc --noEmit`" and rightly rejected it (no root `tsconfig.json`), but it did not consider the distinct option of giving the `ts` root project **no build task / a `noop` build** so Moon — not pnpm — owns the entire graph. That is the version of this fix that removes the smell instead of fencing it.

### F2 — [Medium] New silent failure mode: apps are now excluded, so a future app with no Moon build task goes un-built

Before the fix, `pnpm -r` was an (accidental) safety net that built any app with a `build` script. After the fix, `apps/**` is excluded from `ts:build`, and apps are expected to build via their own `<app>-ts:build` Moon task. That is the right model — but it converts "apps double-built" into "apps **not built at all** by `moon ci :build` unless they declare their own build override." There's no guard that a new app does so.

This is concrete, not hypothetical: `ts/apps/paigasus-docs` already exists as a stub. When it becomes a real docs site with a `build` script, it must define a Moon `build` override (like the console) or `moon ci :build` will only run its inherited `tsc --noEmit` and never produce the site. **Recommendation:** document the invariant ("every `apps/*` that produces an artifact must define its own Moon `build` task with `outputs`") in CONTRIBUTING or the TS app template, and/or have SMA-361's CI assert each app emits its expected output. A one-line fix that shifts a failure from loud-and-intermittent to silent-and-permanent deserves an explicit guard.

### F3 — [Low] Factual error: the docs app *does* have a `moon.yml`

The spec (root-cause enumeration) states *"docs has no `moon.yml` and no `package.json` build script."* The first half is wrong: `ts/apps/paigasus-docs/moon.yml` exists (`id: paigasus-docs-ts`, `layer: library`). The conclusion still holds (no `build` override → inherited `tsc --noEmit`, no real build), so the fix is unaffected — but the design doc's stated graph is inaccurate, and the inaccuracy hides the F2 gap. Worth correcting. (Also note the latent oddity it reveals: `paigasus-docs` lives in `apps/` but is typed `layer: library` — the fix's mental model is directory-based, so it's treated as an app by the pnpm path filter regardless of layer; fine, but the mislabel will confuse the next reader.)

### F4 — [Low] `build` and `typecheck` now diverge in recursion scope

The fix scopes `build` to `packages/**` but leaves `typecheck` as `pnpm -r` (all projects, including apps). So the two sibling aggregator tasks in the same file now use different filter semantics, and the console keeps getting typechecked twice (`ts:typecheck` recursion + inherited `paigasus-console-ts:typecheck`). The spec correctly notes the typecheck double-run is harmless (no lock) and out of scope — but leaving the two tasks asymmetric is a readability/consistency cost a future maintainer will trip over. **Recommendation:** in the same follow-up that addresses F1, apply the same scoping (or the same removal) to `typecheck` so the two tasks stay parallel; if not, add a one-line comment in `ts/moon.yml` explaining why they differ.

### F5 — [Nit] Stale `inputs` after the command narrows to packages

The spec keeps the `build` task `inputs` unchanged "for minimal diff," but those inputs still list `apps/**/tsconfig.json`. Post-fix the task no longer touches apps, so an app `tsconfig.json` change will still bust `ts:build`'s cache and re-run the (now) no-op. Harmless, but narrowing `inputs` to `packages/**` would make the task's declared inputs match its actual scope. Minor; fine to defer.

## Bottom line

Ship the one-line fix as written (with `--if-present`). Then open a small follow-up that does the structural version: drop or neutralize the `pnpm`-recursive aggregator so Moon owns every TS build/typecheck node, add the "apps must declare their own build task" invariant, and correct the docs-app statement. That converts a fence around the bug into removal of its cause.

## Sources

- Spec under review: `docs/superpowers/specs/2026-05-29-sma-391-ts-build-double-build-design.md`
- [Linear SMA-391 — ts:build double-builds paigasus-console](https://linear.app/smaschek/issue/SMA-391/tsbuild-double-builds-paigasus-console-concurrently-nextjs-next-lock) (origin: SMA-359 bootstrap; will flake under SMA-361 CI)
- Repo: `ts/moon.yml` (`build`/`typecheck` = `pnpm -r --if-present`, `merge: replace`), `.moon/tasks/typescript.yml` (inherited `build`/`typecheck` = `tsc --noEmit`, no `outputs`), `ts/apps/paigasus-console/{moon.yml,package.json}` (sole `build` script + `next build` override, `outputs: ['.next']`), `ts/apps/paigasus-docs/{moon.yml,package.json}` (exists; `layer: library`, no build script), `ts/tsconfig.base.json` only (no root `tsconfig.json`)
