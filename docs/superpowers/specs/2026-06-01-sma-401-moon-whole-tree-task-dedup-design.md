# SMA-401 — Route whole-tree checks by project layer so `lint`/`fmt`/`typecheck`/`test` run once, not (N+1)×

**Status:** Design approved
**Date:** 2026-06-01
**Linear:** SMA-401
**Branch:** `feature/sma-401-moon-root-per-package-whole-tree-tasks-lintfmttypechecktest`
**Related:** SMA-399 (py root `build` exclude — its review finding F1 spun out this issue), SMA-394
(ts: Moon owns the `build`/`typecheck` graph), SMA-395/396 (config-only TS package shape + the
`ts:check-config-only` guard), SMA-374 (Rust *template* task-definition dedup — separate), SMA-379
(no-tests shim that masks the cost today)

## Problem

`.moon/tasks/python.yml` and `.moon/tasks/typescript.yml` attach `lint`/`fmt`/`typecheck`/`test`
(+ `build`) to **every** project of that language via `inheritedBy.languages`. So both the
**configuration root** (`py/moon.yml`, `ts/moon.yml`, `layer: configuration`) **and each package**
under `packages/*` / `apps/*` inherit and run the same whole-tree tasks. The root then runs each
once more on top of the per-package fan-out.

The redundancy is real but **not uniform** — its severity depends on how each tool resolves the
file set it operates on. This nuance drives the design, so it is worth stating precisely:

| Task | Tool behavior | Per-package run covers | Total cost |
| --- | --- | --- | --- |
| **py `typecheck`** (`uv run basedpyright`) | reads `[tool.basedpyright] include = ["packages/*/src", "packages/*/tests"]`, resolved relative to `py/pyproject.toml` — **cwd-independent** | the **entire** `packages/*` tree | **(N+1)×** |
| **py `test`** (`uv run pytest`) | walks up to the config → `rootdir = py/` → `[tool.pytest.ini_options] testpaths = ["packages/*/tests"]` — **cwd-independent** | the **entire** suite (and **re-counts/re-reports** it) | **(N+1)×** |
| **py `lint`/`fmt`** (`ruff check .` / `ruff format --check .`) | the `.` arg scopes to the task cwd | only its **own** dir (per-package runs *partition*) | ~**2×** (partitioned packages + one full root pass) |
| **ts `lint`/`fmt`/`test`** (`eslint .` / `prettier --check .` / `vitest run`) | cwd-scoped | only its **own** dir (partition) | ~**2×** |
| **ts `typecheck`/`build`**, **py `build`** | bound to each project's own `tsconfig.json` / `[project]` | its own project | **already correct** — root-excluded (SMA-394/399), per-project, no overlap |

So the true `(N+1)×` offenders are **py `typecheck` and py `test`** (config is whole-tree and
cwd-independent — each per-package run re-does the *entire* tree, and `pytest` double-counts).
`lint`/`fmt` (both langs) and ts `test` are a milder ~2× (per-package partitions, root adds one
full pass). ts `typecheck`/`build` and py `build` are already clean from SMA-394/399.

This is **masked today** only because the packages are empty bootstrap scaffolds — basedpyright
analyzes 0 files, pytest collects 0 tests (SMA-379 shim). The cost scales the moment real
source/tests land. Rust differs (cargo tasks are workspace-aware via `--workspace`/clippy); the
adjacent Rust *template* dedup is SMA-374. This is a structural/perf cleanup, not a correctness
bug — everything is green today, just redundant.

## Root cause

Inheritance is scoped by **language only** (`inheritedBy.languages`), so a task that conceptually
belongs to *one* level (the whole-tree checks belong to the config root; `build` belongs to each
distribution) is attached to *all* projects of that language regardless of level. SMA-394 and
SMA-399 each patched one symptom of this at the root with `workspace.inheritedTasks.exclude`
(ts excluded `build`/`typecheck`; py excluded `build`). Those are per-project opt-outs of a
mis-scoped global; they treat the symptom, not the cause, and must be repeated on every future
project (the same forget-the-exclude failure mode that already required the `ts:check-config-only`
CI guard).

## Decision

Scope task inheritance by **project layer** as well as language, so each task attaches only at the
level it belongs to. Moon 2.x supports `inheritedBy.layers` combined (AND) with `languages` (and
`or`/`not` operators); this is the precise mechanism for "this task belongs to the config root,
that one to each distribution."

- **Whole-tree checks** → `inheritedBy.layers: ['configuration']` → attach **only** to the
  `py`/`ts` roots (which already run them green today).
- **Per-distribution tasks** (py `build`; ts `build` + `typecheck`) →
  `inheritedBy.layers: ['library', 'application']` → attach **only** to `packages/*` / `apps/*`.

This is the symmetric inverse of the `build` decision (build = per-distribution only; checks =
config-root only), achieved centrally. It produces the **same end-state task graph** that
per-package `inheritedTasks.exclude` (the rejected Alternative A) would — packages simply stop
inheriting the checks — but with **zero per-package boilerplate**, **no template changes**, and the
`(N+1)×` class **structurally eliminated**: a `library`/`application` project can never again
inherit a whole-tree check.

### Target task graph

| Project (layer) | Inherited py tasks | Inherited ts tasks |
| --- | --- | --- |
| `py/`, `ts/` root (`configuration`) | `lint`, `fmt`, `typecheck`, `test` | `lint`, `fmt`, `test` |
| `packages/*` (`library`) | `build` | `build`, `typecheck` |
| `apps/*` (`application`) | — *(none in py today)* | `build`, `typecheck` |

**The one deliberate asymmetry:** ts `typecheck` lives **per-package**, not at the root — it is
bound to each project's own `tsconfig.json`, and the ts root has none (would fail `TS5058`; this is
exactly why SMA-394 excluded it). py `typecheck` lives **at the root** because `basedpyright` reads
the central `[tool.basedpyright]` config and runs clean whole-tree (proven in SMA-399). This
language asymmetry is correct, not drift — and the layer-routing expresses it cleanly (ts
`typecheck` is in the `library`/`application`-scoped file; py `typecheck` is in the
`configuration`-scoped file).

## Mechanism — split each language's task file by scope

`inheritedBy` is **per-file**, so routing different tasks to different layers requires splitting
each language's single task file into a *checks* file (configuration-scoped) and a *dist* file
(library/application-scoped). Rust's file is untouched.

### Python

```yaml
# .moon/tasks/python.yml  — KEEP this name; it now holds the whole-tree checks (root-only).
inheritedBy:
  languages: ['python']
  layers: ['configuration']
fileGroups:            # unchanged; merge with py/moon.yml's packages/*/src extension at the root
  sources: ['src/**/*']
  tests: ['tests/**/*', '**/*_test.py', '**/test_*.py']
tasks:                 # the four task bodies (command + inputs) move verbatim from today's python.yml
  lint:      # uv run ruff check .
  fmt:       # uv run ruff format --check .
  typecheck: # uv run basedpyright
  test:      # uv run pytest
```

```yaml
# .moon/tasks/python-dist.yml  — NEW; per-distribution build.
inheritedBy:
  languages: ['python']
  layers: ['library', 'application']
fileGroups:
  sources: ['src/**/*']     # restated explicitly so build's @group(sources) resolves per-package
tasks:
  build: { command: 'uv build', inputs: ['@group(sources)', 'pyproject.toml'] }
```

### TypeScript

Same split: `typescript.yml` keeps `lint`/`fmt`/`test` (commands unchanged) scoped to
`layers: ['configuration']`; new `typescript-dist.yml` holds `build` + `typecheck` scoped to
`layers: ['library', 'application']`. The `commitlint` and `check-config-only` tasks defined in
`ts/moon.yml` are unrelated and stay there.

### Cache wiring

Add the two new files to `.moon/tasks.yml` `implicitInputs` (alongside the existing
`python.yml`/`typescript.yml`/`rust.yml`) so edits to them bust caches.

> **Naming:** keeping `python.yml`/`typescript.yml` as the *checks* files (they hold the majority
> of tasks) minimizes the diff. The role-explicit alternative (`*-root.yml` + `*-package.yml`) was
> considered; `*-dist.yml` for the new file reads clearly against the kept name. Each file gets a
> header comment stating its scope and why the split exists.

## fileGroups handling (the one technical risk)

`@group(sources)`/`@group(tests)` must resolve correctly on both sides of the split:

- **Root checks** need `packages/*/src/**/*` etc. — supplied by the `fileGroups` extension already
  in `py/moon.yml`/`ts/moon.yml` (**kept as-is**), merged with the checks file's groups. (Moon
  merges, not overrides, fileGroups across inheritance layers — confirmed for this repo in SMA-384.)
- **Package `build`** needs the package's own `src/**/*` — supplied by the global `fileGroups` in
  `.moon/tasks.yml` (which apply to all projects) and restated in the dist file to be explicit.

The subtlety: fileGroups declared in a scoped task file are themselves scoped by that file's
`inheritedBy`. The design is sound (the global `.moon/tasks.yml` groups provide `sources`/`tests`
everywhere as a floor), **but this is the first thing implementation will prototype-verify** — see
Open items. Fallback if resolution misbehaves: keep `sources`/`tests` only in the unscoped global
`.moon/tasks.yml` and drop them from the per-language files.

## Root-exclude cleanup

Central routing makes the SMA-394/399 root excludes dead config (build/`typecheck` are no longer
routed to the `configuration` layer at all). Remove them so there is a single source of truth:

- `py/moon.yml`: drop `workspace.inheritedTasks.exclude: ['build']`. Keep `layer`/`language`/
  `fileGroups`. Replace the exclude comment with a one-liner: *build is routed to the
  library/application layers in `python-dist.yml`; it is not attached to this configuration root.*
- `ts/moon.yml`: drop `workspace.inheritedTasks.exclude: ['build', 'typecheck']`. **Keep** its
  `tasks:` block (`commitlint`, `check-config-only`) and `fileGroups`/`layer`/`language`. Same
  pointer comment.

End behavior is identical (build/typecheck still never run at the roots); only the reason changes
from "excluded here" to "not routed here."

## What deliberately does not change

- **Scaffold templates** (`.moon/templates/{python,typescript}/`): untouched. A generated
  `library`/`application` project is automatically correct by layer — it inherits `build`
  (+ ts `typecheck`) and **not** the whole-tree checks, with no per-project config. This is the
  central payoff over Alternative A.
- **`commitlint-config-ts` + the `config` template archetype**: keep
  `exclude: ['build', 'typecheck']`. It is a `library`-layer package, so `build`/`typecheck` *are*
  routed to it, and it still cannot run `tsc` (no `tsconfig.json` → `TS5058`). The
  `ts:check-config-only` CI guard stays valid and necessary. Its `lint`/`fmt`/`test` coverage is
  preserved — the root `eslint .`/`prettier --check .` already walk `packages/commitlint-config/`.
- **End-user commands** (`py:lint`, `ts:test`, the `--query`-scoped build/typecheck commands):
  unchanged. Both READMEs already describe "checks run once at the root."

## Alternatives considered

- **A. Per-package `inheritedTasks.exclude` (the established repo idiom; SMA-394/5/6/9).** Add an
  exclude block to every `packages/*`/`apps/*` and to both scaffold templates so the checks run
  once at the root. Produces the identical task graph, lowest behavioral risk (the root already
  runs the checks green). **Rejected** because it pushes boilerplate onto every current *and future*
  package + both templates, and re-creates the forget-the-exclude failure mode the
  `check-config-only` guard exists to catch. Layer-routing fixes the cause centrally instead of the
  symptom per project.
- **C. Partition-aware hybrid.** Keep partitionable tools (`lint`/`fmt`, ts `test`) per-package for
  incremental caching and move only the true whole-tree offenders (py `typecheck`/`test`) to
  root-only. **Rejected:** mixed mental model, and per-package-only for the partitionable tools
  risks missing files that live *outside* any package (e.g. `ts/scripts/`, root config files) — a
  coverage gap the AC forbids. The marginal caching win does not justify it for a small monorepo
  with whole-tree central config.
- **Per-package partitioning configs** (give each package its own scoped basedpyright/pytest
  config so per-package runs partition). **Rejected:** directly fights the deliberate central-config
  decision (one `[tool.*]` block in `py/pyproject.toml`) and invites drift.

## Out of scope / non-goals

- **Rust template task-definition dedup** — SMA-374. Rust tasks are workspace-aware
  (`--workspace`/clippy) and not affected by this layer split.
- **Per-package partitioned checks for caching granularity.** We eliminate N+1 by *single-run*, not
  by partitioning; root-only runs also cover root-level files. (See Alternative C.)
- **`contracts` (`layer: tool`).** Unaffected — the `languages` filter (`python`/`typescript`)
  already excludes it; no `tool`-layer routing is added.

## Acceptance criteria

- [ ] `moon ci :typecheck` / `:lint` / `:fmt` / `:test` each execute the whole-tree work **once**
      (at the `configuration` root), not once-per-package-plus-root, for both `py/` and `ts/`.
- [ ] No double-counted pytest collection/reporting (only the root `py:test` collects).
- [ ] `moon project paigasus-kernel-py` shows **only** `build` (no `lint`/`fmt`/`typecheck`/`test`);
      `moon project paigasus-kernel-ts` shows **only** `build` + `typecheck`.
- [ ] `moon project py` shows `lint`/`fmt`/`typecheck`/`test` and **no** `build`; `moon project ts`
      shows `lint`/`fmt`/`test` and **no** `build`/`typecheck`.
- [ ] `moon ci :build` still covers every `packages/*` (+ ts `apps/*`); no coverage lost — every
      `packages/*` source/test dir is still checked (by the root whole-tree run).
- [ ] The SMA-394/399 root excludes are removed; `commitlint-config-ts` keeps its exclude and
      `ts:check-config-only` still passes.
- [ ] Whole-graph `moon run :build|:typecheck|:lint|:fmt|:test` stays green.

## Verification plan

1. **Resolved task lists** (the core assertion):
   ```bash
   moon project py    # expect: lint, fmt, typecheck, test; NO build
   moon project ts    # expect: lint, fmt, test; NO build, NO typecheck
   moon project paigasus-kernel-py   # expect: build only
   moon project paigasus-kernel-ts   # expect: build, typecheck only
   moon project commitlint-config-ts # expect: nothing attached (build/typecheck excluded; checks routed away)
   ```
2. **fileGroups resolve** (the risk): inspect resolved inputs for a package `build` and the root
   checks (`moon project …` / task introspection) — confirm `@group(sources)` expands to the
   package's own `src/**/*` for `build`, and to `packages/*/src/**/*` for the root checks.
3. **Single-run, no duplication:**
   ```bash
   moon ci :test       # expect: one py:test + one ts:test, no per-package test tasks
   moon ci :typecheck  # py:typecheck once at root; ts typecheck per-package (bound to tsconfig)
   moon ci :lint
   moon ci :fmt
   moon ci :build      # every packages/* + ts apps/* build present
   ```
4. **Affected-graph:** edit one file under one `packages/*/src` → the `configuration` root's check
   task is marked affected (runs once whole-tree); confirm **no** per-package check task fires.
5. **Whole-graph green + guard:** `moon run :build|:typecheck|:lint|:fmt|:test`; `moon run
   ts:check-config-only`.

## Open items to confirm during implementation (prototype-first)

1. **fileGroup resolution across the split** (highest risk) — verify before writing the rest; apply
   the global-only fallback if needed.
2. **Exact Moon 2.2.5 `inheritedBy` keys/semantics.** The repo currently uses `inheritedBy.languages`
   successfully; the v2 docs show `layers` (plural list) combined with `languages`/`toolchains` as
   **AND**, plus `or`/`not`. Confirm the `languages` + `layers` AND-combination on the pinned 2.2.5
   (and that the key is `layers`, plural — the project field is `layer:`, singular). If 2.2.5 names
   it differently, adjust; the routing intent is unchanged.
3. **Affected-graph marks the configuration root** when a `packages/*` file changes (so the root
   check runs) — relied on by AC "no coverage lost"; verify in step 4 above.

## Files touched

- `.moon/tasks/python.yml` — add `layers: ['configuration']`; remove the `build` task (moves to
  `python-dist.yml`); keep checks + fileGroups; header comment.
- `.moon/tasks/python-dist.yml` — **new**; library/application-scoped `build` + `sources` fileGroup.
- `.moon/tasks/typescript.yml` — add `layers: ['configuration']`; remove `build` + `typecheck`
  (move to dist); keep `lint`/`fmt`/`test` + fileGroups; header comment.
- `.moon/tasks/typescript-dist.yml` — **new**; library/application-scoped `build` + `typecheck`
  + `sources` fileGroup.
- `.moon/tasks.yml` — add the two new files to `implicitInputs`.
- `py/moon.yml` — remove `inheritedTasks.exclude: ['build']`; pointer comment.
- `ts/moon.yml` — remove `inheritedTasks.exclude: ['build', 'typecheck']`; keep `tasks:` block;
  pointer comment.
- `CONTRIBUTING.md` — "Moon project files": document the layer-routing model (checks → config root;
  build/typecheck → library/application via `inheritedBy.layers`) and that config-only packages
  still need their exclude.
- `ts/README.md` — reword the one `moon.yml` bullet that says the root "excludes build/typecheck"
  → "build/typecheck are routed per-project by layer, not attached to the root."
- `py/README.md` — review; likely no change (it does not describe the build exclusion).
