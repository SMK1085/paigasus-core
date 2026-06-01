# SMA-401 — Route tasks by project layer so whole-tree checks run once, not (N+1)×

**Status:** Design approved (staff-engineer review pass incorporated 2026-06-01)
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
under `packages/*` / `apps/*` inherit and run the same tasks. The root then runs each once more on
top of the per-package fan-out.

The redundancy is real but **not uniform** — its severity depends on how each tool resolves the
file set it operates on. This nuance drives the design:

| Task | Tool behavior | Per-package run covers | Today's cost |
| --- | --- | --- | --- |
| **py `typecheck`** (`basedpyright`) | reads central `[tool.basedpyright] include = ["packages/*/src", …]`, resolved relative to `py/pyproject.toml` — **cwd-independent** | the **entire** `packages/*` tree | **(N+1)×** |
| **py `test`** (`pytest`) | central `[tool.pytest] testpaths = ["packages/*/tests"]`, `rootdir = py/` — **cwd-independent** | the **entire** suite (and **re-counts** it) | **(N+1)×** |
| **py `lint`/`fmt`** (`ruff … .`) | central `[tool.ruff]` config; the `.` arg scopes the file set to the cwd | only its **own** dir (per-package runs *partition*) | ~**2×** |
| **ts `lint`/`fmt`** (`eslint .` / `prettier --check .`) | central `eslint.config.js` / `.prettierrc.js`; `.` scopes to cwd | only its **own** dir (partition) | ~**2×** |
| **ts `test`** (`vitest run`) | **no central config exists** (no `vitest.config.*`/`vitest.workspace.*` in `ts/`); per-package cwd/env | only its **own** package | ~**2×** |
| **ts `typecheck`/`build`**, **py `build`** | bound to each project's own `tsconfig.json` / `[project]` | its own project | already correct (root-excluded SMA-394/399; per-project; no overlap) |

The true `(N+1)×` offenders are **py `typecheck` and py `test`** (central, cwd-independent config —
each per-package run re-does the *entire* tree, and `pytest` double-counts). `lint`/`fmt` (both
langs) and ts `test` are a milder ~2× (per-package partitions, root adds one full pass). ts
`typecheck`/`build` and py `build` are already clean from SMA-394/399.

**Masked today** only because the packages are empty scaffolds — basedpyright analyzes 0 files,
pytest collects 0 tests (SMA-379 shim). The cost scales the moment real source/tests land. Rust
differs (workspace-aware via `--workspace`/clippy); its template dedup is SMA-374. This is a
structural/perf cleanup, not a correctness bug — everything is green today, just redundant.

## Root cause

Inheritance is scoped by **language only** (`inheritedBy.languages`), so a task that conceptually
belongs to *one* level is attached to *all* projects of that language. SMA-394 and SMA-399 each
patched one symptom at the root with `workspace.inheritedTasks.exclude`; those are per-project
opt-outs of a mis-scoped global — they treat the symptom, not the cause, and must be repeated on
every future project (the same forget-the-exclude failure mode that already required the
`ts:check-config-only` CI guard).

## Decision

Scope task inheritance by **project layer** as well as language, so each task attaches only at the
level it belongs to. Moon 2.x supports `inheritedBy.layers` combined (AND) with `languages`.

**The discriminator** (this is the whole thesis): a task is routed to the **`configuration` root**
iff its tool reads a **central, cwd-independent config** that makes a *single whole-tree invocation
both correct and complete*. Otherwise it is routed **per-project** (`library`/`application`).

- **Configuration root** — py `lint`/`fmt`/`typecheck`/`test` (all read `py/pyproject.toml`), and
  ts `lint`/`fmt` (read `ts/eslint.config.js` / `.prettierrc.js`). One whole-tree run is correct,
  complete (it also covers root-level files like `ts/scripts/`), and authoritative.
- **Per-project** — ts `typecheck` (bound to each `tsconfig.json`; no root `tsconfig`), ts `test`
  (**no central vitest config**; per-package cwd + environments — e.g. jsdom for `paigasus-ui`,
  node for pure libs), py `build` and ts `build` (per-distribution). There is no root-level unit
  for any of these (no root tsconfig, no root tests, no root distribution), so per-project is
  complete.

This is the symmetric generalization of the SMA-394/399 `build` decision, achieved centrally. It
eliminates the `(N+1)×` class with **zero per-package/template boilerplate** — a `library`/
`application` project can never again inherit a whole-tree check.

### Target task graph

| Project (layer) | Inherited py tasks | Inherited ts tasks |
| --- | --- | --- |
| `py/`, `ts/` root (`configuration`) | `lint`, `fmt`, `typecheck`, `test` | `lint`, `fmt` |
| `packages/*` (`library`) | `build` | `build`, `typecheck`, `test` |
| `apps/*` (`application`) | — *(none in py today)* | `build`, `typecheck`, `test` |

The per-language asymmetry (`typecheck` **and** `test` at the py root but per-package on ts) is not
drift — it falls directly out of the discriminator: py has central config for both; ts has neither
(tsc is per-`tsconfig`, vitest has no central config). Routing by layer expresses this without
special cases. *(This corrects an earlier draft that routed ts `test` to the root alongside
lint/fmt.)*

## Mechanism — split each language's task file by scope

`inheritedBy` is **per-file**, so routing different tasks to different layers requires splitting
each language's single task file into a *checks* file (`configuration`) and a *project* file
(`library`/`application`). Rust's file is untouched. Naming: keep `python.yml`/`typescript.yml`
(they hold the whole-tree checks) and add `python-project.yml`/`typescript-project.yml` for the
per-project tasks; each gets a header comment stating its scope. *(Role-explicit alternative
`*-root.yml`/`*-project.yml` was considered; flag in review if preferred.)*

```yaml
# .moon/tasks/python.yml — whole-tree checks, configuration-root only
inheritedBy:
  languages: ['python']
  layers: ['configuration']
tasks:                 # task bodies (command + inputs) move verbatim from today's python.yml
  lint:      # uv run ruff check .
  fmt:       # uv run ruff format --check .
  typecheck: # uv run basedpyright
  test:      # uv run pytest

# .moon/tasks/python-project.yml — per-distribution, library/application only
inheritedBy:
  languages: ['python']
  layers: ['library', 'application']
tasks:
  build:     # uv build
```

```yaml
# .moon/tasks/typescript.yml — whole-tree checks, configuration-root only
inheritedBy:
  languages: ['typescript']
  layers: ['configuration']
tasks:
  lint:      # pnpm exec eslint .
  fmt:       # pnpm exec prettier --check .

# .moon/tasks/typescript-project.yml — per-project, library/application only
inheritedBy:
  languages: ['typescript']
  layers: ['library', 'application']
tasks:
  build:     # pnpm exec tsc -p tsconfig.json --noEmit (apps override with next build + outputs:)
  typecheck: # pnpm exec tsc -p tsconfig.json --noEmit
  test:      # pnpm exec vitest run --passWithNoTests
```

(The `commitlint` / `check-config-only` tasks defined in `ts/moon.yml` are unrelated and stay
there.) Add both new files to `.moon/tasks.yml` `implicitInputs` so edits bust caches.

## fileGroups — kept in each scoped task file

`@group(sources)`/`@group(tests)` are consumed by the task `inputs` on both sides of the split.

**Implementation finding (prototype, Open item #1 — resolved).** An earlier draft of this spec
proposed centralizing fileGroups in the unscoped global `.moon/tasks.yml`. The Task 1 prototype
disproved it: **Moon 2.2.5 does not propagate global-file fileGroups to a project that inherits a
task from a *scoped* task file** — `moon project paigasus-kernel-py` errored
`project::unknown_file_group sources`. So fileGroups must live **in each scoped task file**, next to
the tasks that reference them — the pattern `.moon/tasks/rust.yml` already uses. Concretely:

- `python.yml` / `typescript.yml` (checks) **keep** their `sources`/`tests` fileGroups (consumed by
  the root checks; merged with the `py|ts/moon.yml` `packages/*` extensions, which are also kept).
- `python-project.yml` / `typescript-project.yml` (per-project) **carry** the `fileGroups` their
  tasks need: `sources` for py `build` and ts `build`/`typecheck`; `sources` + `tests` for ts
  `test`.
- The global `.moon/tasks.yml` fileGroups are **left as-is** (pre-existing; not load-bearing for the
  split). The only change to `.moon/tasks.yml` is adding the two new task files to `implicitInputs`.

Net resolution (verified): a package `build`'s `@group(sources)` → its own `src/**/*` (from the
`-project.yml` group); the root checks' groups → `src/**/*` (empty at root) **+** `py|ts/moon.yml`'s
`packages/*/…` extension. (Moon merges, not overrides, fileGroups across layers — confirmed for this
repo in SMA-384.)

## Root-exclude cleanup (gated)

Central routing makes the SMA-394/399 root excludes dead config (build/`typecheck` are no longer
routed to `configuration` at all). Remove them for a single source of truth — **but only after**
confirming the routing is in force, in the same change:

1. Land the routing; run `moon project py` / `moon project ts` and confirm they resolve with **no**
   `build`/`typecheck`.
2. *Then* remove the excludes:
   - `py/moon.yml`: drop `inheritedTasks.exclude: ['build']`; keep `layer`/`language`; replace the
     comment with a pointer to `python-project.yml`.
   - `ts/moon.yml`: drop `inheritedTasks.exclude: ['build', 'typecheck']`; **keep** its `tasks:`
     block (`commitlint`, `check-config-only`); same pointer comment.

Removing them before confirming routing would re-expose exactly the bugs SMA-394/399 fixed (py junk
`UNKNOWN` wheel, ts root `TS5058`) if a `layers`-key mismatch silently no-ops the routing.

## Trade-off accepted

Routing the whole-tree checks to the root means **any** `packages/*` edit marks the root check
affected and re-runs it over the whole tree — no per-package check caching for lint/fmt (both
langs) and py typecheck/test. For py typecheck/test this loses nothing (those per-package runs were
already whole-tree). ts `typecheck`/`test`/`build` keep per-package caching (they stay per-project).
For a small monorepo this is the consciously-accepted inverse of the cost being fixed; **revisit if
the repo grows large enough that whole-tree-on-every-change checks dominate CI time.**

## What deliberately does not change

- **Scaffold templates** (`.moon/templates/{python,typescript}/`): untouched. A generated
  `library`/`application` project is automatically correct by layer — the central payoff over the
  rejected per-package-exclude approach.
- **`commitlint-config-ts` + the `config` template archetype**: keep `exclude: ['build',
  'typecheck']`. Still `library` → still gets `build`/`typecheck`/`test` routed to it. It keeps
  excluding build/typecheck (can't run `tsc` → `TS5058`); the inherited `test` runs
  `vitest run --passWithNoTests` (0 tests → green, harmless — not worth widening the guard). The
  `ts:check-config-only` guard stays valid. Its lint/fmt coverage comes from the root pass.
- **Commands:** `py:lint`/`py:fmt`/`py:typecheck`/`py:test` all stay on the py root. ts `lint`/`fmt`
  stay as `ts:lint`/`ts:fmt`; ts `test` joins `typecheck`/`build` on the `--query`-scoped form
  (see Docs).

## Alternatives considered

- **A. Per-package `inheritedTasks.exclude` (the established idiom; SMA-394/5/6/9).** Same task
  graph, lowest behavioral risk, but pushes boilerplate onto every current *and future* package +
  both templates and re-creates the forget-the-exclude failure mode. Rejected — layer-routing fixes
  the cause centrally.
- **C. Partition-aware hybrid** (keep partitionable tools per-package for caching). Rejected: mixed
  model, and per-package-only for lint/fmt risks missing files outside any package (`ts/scripts/`,
  root configs) — a coverage gap. (The granularity trade is captured above instead.)
- **Per-package partitioning configs** (scoped per-package basedpyright/pytest). Rejected: fights
  the deliberate central-config decision and invites drift.
- **ts `test` sub-alternative — root-only + a vitest projects/workspace config** as a companion
  deliverable (so one root run serves heterogeneous per-package environments). Rejected in favor of
  per-package ts `test`: no new config, matches the discriminator, and preserves per-package
  environments/caching naturally. (If a workspace-wide vitest config is ever wanted, it's a
  separate issue.)

## Out of scope / non-goals

- Rust template task-definition dedup — SMA-374 (Rust is workspace-aware; unaffected).
- The root `ts/package.json` `"test"` npm script — orthogonal to Moon task routing; left as-is.
- `contracts` (`layer: tool`) and the `repo` root (`language: bash`) — both excluded by the
  `languages` filter; no `tool`/`bash` routing added.

## Acceptance criteria

- [ ] `moon ci :typecheck` / `:lint` / `:fmt` / `:test` each execute their whole-tree work **once**,
      not once-per-package-plus-root: py `lint`/`fmt`/`typecheck`/`test` and ts `lint`/`fmt` run
      once at the root; ts `typecheck`/`test` run per-project with **no** root duplicate.
- [ ] No double-counted pytest collection/reporting (only the root `py:test` collects).
- [ ] `moon project paigasus-kernel-py` shows **only** `build`; `moon project paigasus-kernel-ts`
      shows **only** `build`/`typecheck`/`test`.
- [ ] `moon project py` shows `lint`/`fmt`/`typecheck`/`test` and **no** `build`; `moon project ts`
      shows **only** `lint`/`fmt` (no `build`/`typecheck`/`test`).
- [ ] `moon ci :build` still covers every `packages/*` (+ ts `apps/*`); no coverage lost — every
      source/test dir is still checked (py via the root run; ts per-project + the root lint/fmt).
- [ ] SMA-394/399 root excludes removed **only after** the resolved-task-list check passes;
      `commitlint-config-ts` keeps its exclude; `ts:check-config-only` still passes.
- [ ] Whole-graph `moon run :build|:typecheck|:lint|:fmt|:test` stays green.

## Verification plan

1. **Resolved task lists** (core assertion — and the exclude-removal gate; run *before* removing excludes):
   ```bash
   moon project py    # lint, fmt, typecheck, test; NO build
   moon project ts    # lint, fmt; NO build/typecheck/test
   moon project paigasus-kernel-py    # build only
   moon project paigasus-kernel-ts    # build, typecheck, test only
   moon project paigasus-console-ts   # build (next), typecheck, test
   moon project commitlint-config-ts  # test only (build/typecheck excluded)
   ```
2. **fileGroups resolve** (Open item #1): inspect resolved `inputs` for a package `build`
   (`@group(sources)` → own `src/**/*`) and the root checks (→ `packages/*/src` + `…/tests`).
3. **Single-run, no duplication:**
   ```bash
   moon ci :test       # one py:test (root) + per-package ts test; no per-package py test, no root ts test
   moon ci :typecheck  # py:typecheck once at root; ts typecheck per-package
   moon ci :lint ; moon ci :fmt ; moon ci :build
   ```
4. **Affected-graph:** edit one file under one `packages/*/src` → the root check task is affected
   (runs once whole-tree); confirm no per-package py check fires.
5. **Whole-graph green + guard:** `moon run :build|:typecheck|:lint|:fmt|:test`;
   `moon run ts:check-config-only`.

## Open items to confirm during implementation (prototype-first)

1. **fileGroup resolution** (highest risk) — **resolved by the Task 1 prototype:** global-only is
   unsupported on Moon 2.2.5; fileGroups now live in each scoped task file (see the fileGroups
   section). Verify step 2 still passes for ts in Task 2.
2. **Exact Moon 2.2.5 `inheritedBy` keys.** Review confirmed v2 supports `languages` + `layers`
   (AND) with `layers` a plural list; confirm on the pinned 2.2.5 (project field is `layer:`,
   singular). If named differently, adjust — intent unchanged.
3. **Affected-graph marks the `configuration` root** when a `packages/*` file changes (AC "no
   coverage lost") — verify in step 4.

## Files touched

- `.moon/tasks/python.yml` — add `layers: ['configuration']`; remove `build` (→ `python-project.yml`);
  keep checks **and `fileGroups`**; header comment.
- `.moon/tasks/python-project.yml` — **new**; library/application-scoped `build` + its `sources`
  fileGroup.
- `.moon/tasks/typescript.yml` — add `layers: ['configuration']`; remove `build`/`typecheck`/`test`
  (→ `typescript-project.yml`); keep `lint`/`fmt` **and `fileGroups`**; header comment.
- `.moon/tasks/typescript-project.yml` — **new**; library/application-scoped `build`/`typecheck`/`test`
  + `sources`/`tests` fileGroups.
- `.moon/tasks.yml` — add the two new task files to `implicitInputs` (global fileGroups left as-is;
  not load-bearing for the split — see the fileGroups section).
- `py/moon.yml` — remove `inheritedTasks.exclude: ['build']`; pointer comment (gated on step 1).
- `ts/moon.yml` — remove `inheritedTasks.exclude: ['build', 'typecheck']`; keep `tasks:` block;
  pointer comment (gated on step 1).
- `CONTRIBUTING.md` — "Moon project files": document the layer-routing model (whole-tree checks →
  configuration root; per-project tasks → library/application via `inheritedBy.layers`) and the
  central-config discriminator; note config-only packages still need their exclude.
- `ts/README.md` — Commands: `Test` moves from `moon run ts:test` to `moon run :test --query
  "language=typescript"` (now per-project, like `typecheck`/`build`); reword the "lint/fmt/test run
  once at the root" prose to "lint/fmt run once at the root; typecheck, test, and build fan out per
  project," and the `moon.yml` bullet (no longer "excludes build/typecheck" — "not routed to the
  root by layer").
- `py/README.md` — review; expected no change (py `lint`/`fmt`/`typecheck`/`test` all stay on the
  root as documented).
