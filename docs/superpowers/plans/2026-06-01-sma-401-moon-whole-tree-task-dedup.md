# SMA-401 — Layer-routed whole-tree task dedup — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Moon's `lint`/`fmt`/`typecheck`/`test` run exactly once instead of (N+1)× by routing task inheritance with `inheritedBy.layers` — whole-tree checks attach to the `configuration` roots, per-project tasks to `library`/`application` projects.

**Architecture:** Split each language's single `.moon/tasks/<lang>.yml` into a *checks* file (`configuration`-scoped) and a *project* file (`library`/`application`-scoped). Move `fileGroups` to the unscoped global `.moon/tasks.yml`. Then remove the now-redundant SMA-394/399 root excludes (gated on the routing being in force), and fix the comments/docs that the new model makes stale.

**Tech Stack:** Moon 2.2.5 (`inheritedBy.languages` + `inheritedBy.layers`, AND-combined), YAML task config. No application code.

**Spec:** [`docs/superpowers/specs/2026-06-01-sma-401-moon-whole-tree-task-dedup-design.md`](../specs/2026-06-01-sma-401-moon-whole-tree-task-dedup-design.md)

---

## Preconditions

- [ ] **On the feature branch** `feature/sma-401-moon-root-per-package-whole-tree-tasks-lintfmttypechecktest`. Verify: `git branch --show-current`.
- [ ] **Moon on PATH.** Moon/uv/buf are proto-managed and off the default Bash-tool PATH. Run this once per shell before any `moon` command in this plan:
  ```bash
  export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
  moon --version    # expect: 2.2.5
  ```
- [ ] **Capture the baseline (the bug, for before/after diff).** Record what each project resolves *today*:
  ```bash
  moon project py                  # note: build, lint, fmt, typecheck, test  (build is the SMA-399 exclude target — absent today; lint/fmt/typecheck/test PRESENT)
  moon project paigasus-kernel-py  # note: build, lint, fmt, typecheck, test  (ALL present — this is the redundancy)
  moon project ts                  # note: lint, fmt, test  (build/typecheck excluded by SMA-394)
  moon project paigasus-kernel-ts  # note: build, typecheck, lint, fmt, test  (lint/fmt/test are the redundancy)
  ```
  The fix removes the whole-tree checks from the per-package rows.

---

## Task 1: Route Python tasks by layer

**Files:**
- Modify: `.moon/tasks/python.yml` (becomes the configuration-scoped checks file)
- Create: `.moon/tasks/python-project.yml` (library/application-scoped `build`)
- Modify: `.moon/tasks.yml` (central `tests` fileGroup + `implicitInputs`)

- [ ] **Step 1: Move `fileGroups` to the global file and register the new task file**

Replace the `fileGroups` and `implicitInputs` blocks in `.moon/tasks.yml` so the global `tests` group becomes a superset of the per-language groups (adds the pytest `test_*` prefix form), and the new Python project file is a cache input. Full target file:

```yaml
$schema: 'https://moonrepo.dev/schemas/tasks.json'

fileGroups:
  sources:
    - 'src/**/*'
  tests:
    - 'tests/**/*'
    - '**/*.test.*'
    - '**/*.spec.*'
    - '**/*_test.*'
    - '**/test_*.*' # pytest prefix form (was in python.yml's tests group; centralized here — SMA-401)

# Inserted into every inherited task's inputs so a workspace-level toolchain or
# global-task change busts caches. (Caching is on by default, and an undeclared
# task `inputs` already defaults to all project files '**/*', so per-project
# edits invalidate correctly without further config.)
implicitInputs:
  - '/.moon/toolchains.yml'
  - '/.moon/tasks.yml'
  - '/.moon/tasks/rust.yml'
  - '/.moon/tasks/python.yml'
  - '/.moon/tasks/python-project.yml'
  - '/.moon/tasks/typescript.yml'
  - '/.moon/tasks/typescript-project.yml'

taskOptions:
  outputStyle: 'buffer-only-failure'
```

> Note: `typescript-project.yml` is listed now but created in Task 2. A non-existent `implicitInput` path is treated as an absent input by Moon (no error) — verified in Step 4 of this task.

- [ ] **Step 2: Rewrite `.moon/tasks/python.yml` as the configuration-scoped checks file**

Add `layers: ['configuration']`, drop the `build` task (moves to the project file) and the `fileGroups` block (now global). Full target file:

```yaml
$schema: 'https://moonrepo.dev/schemas/tasks.json'

# Whole-tree quality checks for the py workspace. Scoped to the configuration-layer root
# (py/moon.yml) ONLY: ruff/basedpyright/pytest all read the central config in py/pyproject.toml,
# so one run from py/ covers (and de-duplicates) the whole packages/* tree — running them
# per-package would re-do the same whole-tree work N times (SMA-401). Per-distribution `build`
# lives in python-project.yml; fileGroups live centrally in .moon/tasks.yml.
inheritedBy:
  languages: ['python']
  layers: ['configuration']

tasks:
  lint:
    command: 'uv run ruff check .'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock']
  fmt:
    command: 'uv run ruff format --check .'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock']
  typecheck:
    command: 'uv run basedpyright'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock']
  test:
    command: 'uv run pytest'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock', 'conftest.py']
```

- [ ] **Step 3: Create `.moon/tasks/python-project.yml`**

```yaml
$schema: 'https://moonrepo.dev/schemas/tasks.json'

# Per-distribution tasks for the py workspace. Scoped to library/application layers ONLY:
# each py/packages/* builds its own wheel via the uv_build backend. NOT routed to the
# configuration root — py/pyproject.toml is a virtual workspace root with no [project] table,
# so a root `uv build` emits a junk UNKNOWN wheel (the bug SMA-399 excluded; now simply not
# routed here). (SMA-401)
#
# `start` is intentionally absent: service-archetype projects get it from the python scaffold
# template, which needs per-project Tera variables (the module path) Moon's task-file syntax
# can't reach. A hand-written python service (none today) adds `start` to its own moon.yml.
inheritedBy:
  languages: ['python']
  layers: ['library', 'application']

tasks:
  build:
    command: 'uv build'
    inputs: ['@group(sources)', 'pyproject.toml']
```

- [ ] **Step 4: Verify the resolved Python task graph (the "test")**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon project py
moon project paigasus-kernel-py
```
Expected:
- `moon project py` → tasks `lint`, `fmt`, `typecheck`, `test`; **no** `build`.
- `moon project paigasus-kernel-py` → task `build` **only**; no `lint`/`fmt`/`typecheck`/`test`.
- Neither command errors on the not-yet-created `typescript-project.yml` implicitInput.

- [ ] **Step 5: Verify fileGroups resolved correctly across the split (highest-risk check)**

```bash
moon task paigasus-kernel-py:build   # inspect the printed "Inputs"
moon task py:typecheck               # inspect the printed "Inputs"
```
Expected: the package `build` Inputs include its own `src/**/*` (from the global `sources` group in `.moon/tasks.yml`); the root `typecheck` Inputs include `packages/*/src/**/*` and `packages/*/tests/**/*` (the global group merged with `py/moon.yml`'s extension). If a group resolves to nothing, STOP: this is the spec's Open item #1 — confirm `.moon/tasks.yml` still defines `sources`/`tests` unscoped (the floor for every project) and that `py/moon.yml` still extends them, then re-run. Do not proceed to Task 3 until inputs resolve.

- [ ] **Step 6: Confirm the checks still run green at the root**

```bash
moon run py:typecheck py:lint py:fmt py:test
```
Expected: all pass (0 files / 0 tests on the empty scaffolds, as today).

- [ ] **Step 7: Commit**

```bash
git add .moon/tasks/python.yml .moon/tasks/python-project.yml .moon/tasks.yml
git commit -m "refactor(ci): route py tasks by project layer (SMA-401)

Split .moon/tasks/python.yml into configuration-scoped checks
(lint/fmt/typecheck/test) and library/application-scoped build
(python-project.yml), so the whole-tree checks attach to the py root only and
stop re-running per package. Centralize fileGroups in .moon/tasks.yml (add the
pytest test_* prefix glob)."
```

---

## Task 2: Route TypeScript tasks by layer

**Files:**
- Modify: `.moon/tasks/typescript.yml` (becomes the configuration-scoped `lint`/`fmt` file)
- Create: `.moon/tasks/typescript-project.yml` (library/application-scoped `build`/`typecheck`/`test`)

(`.moon/tasks.yml` already lists `typescript-project.yml` from Task 1.)

- [ ] **Step 1: Rewrite `.moon/tasks/typescript.yml` as the configuration-scoped checks file**

Add `layers: ['configuration']`; keep only `lint`/`fmt`; drop `build`/`typecheck`/`test` and the `fileGroups` block. Full target file:

```yaml
$schema: 'https://moonrepo.dev/schemas/tasks.json'

# Whole-tree lint/format for the ts workspace. Scoped to the configuration-layer root
# (ts/moon.yml) ONLY: eslint/prettier read the central ts/eslint.config.js & .prettierrc.js,
# so one run from ts/ covers the whole tree (including root-level files like ts/scripts/) —
# running them per-package would re-lint the same files (SMA-401). Per-project
# build/typecheck/test live in typescript-project.yml; fileGroups live centrally in
# .moon/tasks.yml.
inheritedBy:
  languages: ['typescript']
  layers: ['configuration']

tasks:
  lint:
    command: 'pnpm exec eslint .'
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - 'package.json'
      - '/ts/eslint.config.js'
      - '/ts/pnpm-lock.yaml'
  fmt:
    command: 'pnpm exec prettier --check .'
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - '.prettierrc.js'
      - '.prettierignore'
      - 'package.json'
      - '/ts/.prettierrc.js'
      - '/ts/.prettierignore'
      - '/ts/pnpm-lock.yaml'
```

- [ ] **Step 2: Create `.moon/tasks/typescript-project.yml`**

```yaml
$schema: 'https://moonrepo.dev/schemas/tasks.json'

# Per-project tasks for the ts workspace. Scoped to library/application layers ONLY:
# `tsc -p tsconfig.json` binds to each project's own tsconfig (the ts root has none → TS5058,
# the bug SMA-394 excluded; now simply not routed here), and vitest has NO central config so
# tests run per-package in each package's own cwd/environment. Apps override `build` with their
# own outputs: (see paigasus-console). `typecheck` is the canonical type-check task that never
# gets overridden; `build` is the override surface. (SMA-401)
inheritedBy:
  languages: ['typescript']
  layers: ['library', 'application']

tasks:
  build:
    # No outputs — tsc --noEmit produces no files; cache invalidation runs off inputs.
    # Apps that produce artifacts (Next.js) override this task with outputs:.
    command: 'pnpm exec tsc -p tsconfig.json --noEmit'
    inputs:
      - '@group(sources)'
      - 'tsconfig.json'
      - 'package.json'
      - '/ts/tsconfig.base.json'
      - '/ts/pnpm-lock.yaml'
  typecheck:
    command: 'pnpm exec tsc -p tsconfig.json --noEmit'
    inputs:
      - '@group(sources)'
      - 'tsconfig.json'
      - 'package.json'
      - '/ts/tsconfig.base.json'
      - '/ts/pnpm-lock.yaml'
  test:
    command: 'pnpm exec vitest run --passWithNoTests'
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - 'package.json'
      - '/ts/pnpm-lock.yaml'
```

- [ ] **Step 3: Verify the resolved TypeScript task graph**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon project ts
moon project paigasus-kernel-ts
moon project paigasus-console-ts
moon project commitlint-config-ts
```
Expected:
- `moon project ts` → `lint`, `fmt` **only**; no `build`/`typecheck`/`test`.
- `moon project paigasus-kernel-ts` → `build`, `typecheck`, `test` only; no `lint`/`fmt`.
- `moon project paigasus-console-ts` → `build` (the `next build` override), `typecheck`, `test`.
- `moon project commitlint-config-ts` → `test` only (`build`/`typecheck` excluded by its own moon.yml; `lint`/`fmt` routed to the root).

- [ ] **Step 4: Confirm the checks run green**

```bash
moon run ts:lint ts:fmt
moon run :test --query "language=typescript"
moon run :typecheck --query "language=typescript"
```
Expected: all pass; `:test`/`:typecheck` resolve per-package targets (e.g. `paigasus-kernel-ts:test`), and there is **no** `ts:test`/`ts:typecheck` target.

- [ ] **Step 5: Commit**

```bash
git add .moon/tasks/typescript.yml .moon/tasks/typescript-project.yml
git commit -m "refactor(ci): route ts tasks by project layer (SMA-401)

Split .moon/tasks/typescript.yml into configuration-scoped lint/fmt and
library/application-scoped build/typecheck/test (typescript-project.yml). ts
test moves per-project (no central vitest config; per-package environments),
matching ts typecheck/build; the ts root now owns only the whole-tree lint/fmt."
```

---

## Task 3: Remove the now-redundant root excludes (gated)

**Files:**
- Modify: `py/moon.yml`
- Modify: `ts/moon.yml`

- [ ] **Step 1: GATE — confirm the routing is in force before removing anything**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon project py | grep -iE 'build|typecheck' || echo "py: no build/typecheck ✓"
moon project ts | grep -iE 'build|typecheck' || echo "ts: no build/typecheck ✓"
```
Expected: both print the "✓" line (the roots resolve no `build`/`typecheck` purely from layer routing, with the excludes still present). If `build`/`typecheck` appear, STOP — the `layers` routing is not taking effect (see Open item #2 in the spec: confirm the `layers` key/semantics on 2.2.5). Do **not** remove the excludes until this gate passes.

- [ ] **Step 2: Edit `py/moon.yml` — drop the exclude, keep fileGroups, update comments**

Full target file:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

layer: 'configuration'
language: 'python'

# fileGroups live centrally in .moon/tasks.yml; extend them here for the whole-tree checks
# (lint/fmt/typecheck/test) that run from this configuration root — the py workspace keeps
# sources under packages/*/src. Moon merges (not overrides) fileGroups across layers, so the
# resolved @group(sources)/@group(tests) cover packages/*/src/** and packages/*/tests/**.
fileGroups:
  sources:
    - 'packages/*/src/**/*'
  tests:
    - 'packages/*/tests/**/*'

# No `build` here: it is routed to the library/application layers in
# .moon/tasks/python-project.yml, so it never attaches to this configuration root. This
# replaces the SMA-399 `workspace.inheritedTasks.exclude: ['build']`, now redundant under
# layer routing. (SMA-401)
```

- [ ] **Step 3: Edit `ts/moon.yml` — drop the exclude, keep the `tasks:` block, update comments**

Full target file:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

layer: 'configuration'
language: 'typescript'

# fileGroups live centrally in .moon/tasks.yml; extend them here for the whole-tree lint/fmt
# that run from this configuration root — the ts workspace keeps sources under packages/*/src
# and apps/*/{src,app}. Moon merges (not overrides) fileGroups across layers.
fileGroups:
  sources:
    - 'packages/*/src/**/*'
    - 'apps/*/src/**/*'
    - 'apps/*/app/**/*'
  tests:
    - 'packages/*/tests/**/*'
    - 'apps/*/tests/**/*'

# No build/typecheck/test here: they are routed to the library/application layers in
# .moon/tasks/typescript-project.yml, so they never attach to this configuration root. This
# replaces the SMA-394 `workspace.inheritedTasks.exclude: ['build', 'typecheck']`, now redundant
# under layer routing (ts `test` also moved per-project — no central vitest config). (SMA-401)

# Commit-message validation for CI (SMA-371 AC-E parity gate). Lives here because the
# pinned commitlint binary + @paigasus/commitlint-config are installed under ts/. Invoked
# explicitly — `moon run ts:commitlint -- --from <a> --to <b>` — never via `moon ci`; it
# stays out of the gate because the workflow's `moon ci` target list never includes it.
# Do NOT set `runInCI: false`: Moon also excludes such tasks from `moon run` whenever CI=true,
# which would make the CI gate resolve zero tasks and exit 1.
tasks:
  commitlint:
    # `--config` path is relative to the ts/ task cwd (the local lefthook hook uses the
    # repo-rooted `ts/commitlint.config.cjs` instead — same file, different cwd; don't "fix").
    command: 'pnpm exec commitlint --config commitlint.config.cjs'
    inputs: []
    options:
      cache: false
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

- [ ] **Step 4: Re-verify routing still holds and no regression resurfaces**

```bash
moon project py | grep -iE 'build|typecheck' || echo "py: still no build/typecheck ✓"
moon project ts | grep -iE 'build|typecheck' || echo "ts: still no build/typecheck ✓"
rm -rf py/dist py/packages.egg-info
moon run :build
ls py/dist 2>/dev/null || echo "py/dist absent ✓"
```
Expected: both "✓" lines; `moon run :build` is green; `ls py/dist` shows **no** `unknown-0.0.0*.whl` / `packages-0.0.0.tar.gz` (the py root no longer runs `uv build`), and no `py/packages.egg-info/`. (These artifacts are gitignored; this only confirms they stop being generated.)

- [ ] **Step 5: Commit**

```bash
git add py/moon.yml ts/moon.yml
git commit -m "refactor(ci): drop redundant root task excludes (SMA-401)

Layer routing means build/typecheck are never routed to the configuration
roots, so the SMA-394 (ts: build,typecheck) and SMA-399 (py: build) root
inheritedTasks.exclude blocks are dead config. Remove them; the pointer
comments now reference the *-project.yml routing. End behavior is identical."
```

---

## Task 4: Fix comments the new model makes stale (config-only rationale)

The "stays `language: typescript` so `lint`/`fmt`/`test` still attach" rationale is now inaccurate: under layer routing `lint`/`fmt` attach at the **ts root**, and only `test` attaches to the package. Comment-only changes; no functional change. (`scripts/check-config-only.mjs` needs no change — its logic and text don't assert this rationale.)

**Files:**
- Modify: `ts/packages/commitlint-config/moon.yml`
- Modify: `.moon/templates/typescript/moon.yml`

- [ ] **Step 1: Update `ts/packages/commitlint-config/moon.yml` comment**

Full target file:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'commitlint-config-ts'
layer: 'library'
language: 'typescript'

# Reference instance of the config-only TS package shape (SMA-396): a pure CommonJS config
# package (index.cjs) that is not a tsc compilation unit — no tsconfig.json, nothing to compile
# or type-check. Excludes the inherited per-project build/typecheck (.moon/tasks/typescript-
# project.yml runs `tsc -p tsconfig.json --noEmit`, which fails TS5058 with no tsconfig.json).
# It stays `language: typescript` so the workspace lint/fmt (run once at the ts root) still cover
# its files and the per-project `test` (vitest --passWithNoTests) still attaches harmlessly. See
# CONTRIBUTING "Moon project files"; the ts:check-config-only guard enforces this exclude. (SMA-401)
workspace:
  inheritedTasks:
    exclude: ['build', 'typecheck']
```

- [ ] **Step 2: Update the `config` archetype comment in `.moon/templates/typescript/moon.yml`**

Replace the `elif archetype == "config"` branch comment. The full target file:

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
# eslint/prettier/commitlint config). Excludes the inherited per-project build/typecheck, which
# run `tsc -p tsconfig.json --noEmit` and fail TS5058 with no tsconfig.json. Stays
# language: typescript so the workspace lint/fmt (run once at the ts root) cover it and the
# per-project test attaches harmlessly. The ts:check-config-only CI guard enforces this exclude
# repo-wide. See CONTRIBUTING "Moon project files". (SMA-396, SMA-401)
workspace:
  inheritedTasks:
    exclude: ['build', 'typecheck']
{%- endif %}
```

- [ ] **Step 3: Verify the config-only guard still passes and the template still scaffolds**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon run ts:check-config-only
```
Expected: `config-only guard: N TS projects checked, no violations`.

- [ ] **Step 4: Commit**

```bash
git add ts/packages/commitlint-config/moon.yml .moon/templates/typescript/moon.yml
git commit -m "docs(ci): correct config-only rationale for layer routing (SMA-401)

Under layer routing, lint/fmt run at the ts root (not per-package) and only
test attaches to a config-only package. Update the now-stale 'lint/fmt/test
still attach' comments in commitlint-config and the typescript 'config'
template archetype. Comment-only; the exclude and the check-config-only guard
are unchanged."
```

---

## Task 5: Documentation

**Files:**
- Modify: `CONTRIBUTING.md` (add a layer-routing subsection; fix the config-only paragraph)
- Modify: `ts/README.md` (Commands table + prose: `test` is now per-project)
- Modify: `py/README.md` (review only — expected no change)

- [ ] **Step 1: Add a "Task routing by layer" subsection to CONTRIBUTING.md**

Insert immediately **before** the `### Moon project files` heading (around line 126). New content:

```markdown
### Task routing by layer (SMA-401)

Per-language task files in `.moon/tasks/` route tasks by **project layer** (`inheritedBy.layers`,
combined with `languages`), so each task attaches only where it belongs:

- **Whole-tree checks** run **once** at the `configuration`-layer workspace roots
  (`py/moon.yml`, `ts/moon.yml`) — their tools read a central config, so one invocation covers
  the whole tree (including root-level files). These are py `lint`/`fmt`/`typecheck`/`test` and
  ts `lint`/`fmt`, defined in `.moon/tasks/python.yml` / `.moon/tasks/typescript.yml`.
- **Per-project tasks** attach to `library`/`application` projects only — they bind to each
  project's own config (`tsconfig.json`, `[project]`) or have no central config. These are py
  `build` and ts `build`/`typecheck`/`test`, defined in `.moon/tasks/python-project.yml` /
  `.moon/tasks/typescript-project.yml`.

The discriminator is *"does a central, cwd-independent config make one whole-tree run correct and
complete?"* — yes for the checks above (py reads `py/pyproject.toml`; ts lint/fmt read
`ts/eslint.config.js` / `.prettierrc.js`), no for ts `typecheck` (per-`tsconfig`) and ts `test`
(no central vitest config; per-package environments). This is why the workspace roots define no
`build`/`typecheck` (and the ts root no `test`), and why those tasks fan out per project — Moon
owns each fan-out graph. A new `library`/`application` project is correct automatically; no
per-project opt-out is needed.
```

- [ ] **Step 2: Fix the config-only paragraph in CONTRIBUTING.md**

Find this sentence in the "Config-only TS packages" paragraph (around line 172):

```
It stays `language: typescript` (so `lint`/`fmt`/`test` still attach) and
`layer: library` (importable/published code).
```

Replace with:

```
It stays `language: typescript` (so the workspace `lint`/`fmt` at the `ts` root cover its
files and the per-project `test` still attaches) and `layer: library` (importable/published
code).
```

- [ ] **Step 3: Update the `ts/README.md` Commands section**

(a) Replace the prose sentence above the table (line 24):

```
`lint`/`fmt`/`test` run once over the whole workspace from the `ts` Moon project; `typecheck` and `build` fan out per project (Moon owns the `:typecheck`/`:build` graph), so they are addressed with a TypeScript-scoped query — a bare `moon run :build` would also build the `rust`/`py` workspaces:
```

with:

```
`lint`/`fmt` run once over the whole workspace from the `ts` Moon project; `typecheck`, `test`, and `build` fan out per project (Moon owns those graphs by layer — SMA-401), so they are addressed with a TypeScript-scoped query — a bare `moon run :test` would also run the `rust`/`py` workspaces:
```

(b) Replace the `Test` row of the table:

```
| Test            | `moon run ts:test`                                  |
```

with:

```
| Test            | `moon run :test --query "language=typescript"`      |
```

(c) Replace the second Notes bullet (the one explaining the `--query` scoping, around line 38):

```
- `Type check` and `Build` use a TypeScript-scoped query (`moon run :typecheck --query "language=typescript"` / `moon run :build --query "language=typescript"`): the `ts` root no longer defines those tasks — Moon's per-project tasks own them (SMA-394) — and a bare `moon run :build` would also build the `rust`/`py` workspaces, so the query scopes it to TS. `lint`/`fmt`/`test` still run once from the `ts` project.
```

with:

```
- `Type check`, `Test`, and `Build` use a TypeScript-scoped query: the `ts` root no longer defines those tasks — Moon's per-project tasks own them (`typecheck`/`build` since SMA-394; `test` since SMA-401, as vitest has no central config and runs per-package) — and a bare `moon run :build`/`:test` would also hit the `rust`/`py` workspaces, so the query scopes it to TS. `lint`/`fmt` still run once from the `ts` project.
```

(d) Update the `moon.yml` Layout bullet (line 12) — remove the "Excludes the inherited build/typecheck" phrasing:

```
- `moon.yml` — workspace parent project (`layer: configuration`). Excludes the inherited `build`/`typecheck` — Moon's per-project tasks own the full `:build`/`:typecheck` graph, and the root has no `tsconfig.json` to run `tsc` against; still owns the whole-tree `lint`/`fmt`/`test` inherited from `.moon/tasks/typescript.yml`.
```

with:

```
- `moon.yml` — workspace parent project (`layer: configuration`). Owns the whole-tree `lint`/`fmt` (run once from `ts/`); `build`/`typecheck`/`test` are routed per-project by layer (`.moon/tasks/typescript-project.yml`), not to the root — Moon owns those fan-out graphs (SMA-401).
```

- [ ] **Step 4: Review `py/README.md` — confirm no change needed**

```bash
grep -nE 'build|exclude|:test|:typecheck|:lint|:format' py/README.md
```
Expected: the Commands table lists `py:lint`/`py:format`/`py:typecheck`/`py:test` — all still on the py root after this change, so **no edit required**. (If a `py:build` row exists, remove it — but per the SMA-399 spec there is none.) Make no change unless the grep shows a now-false statement.

- [ ] **Step 5: Commit**

```bash
git add CONTRIBUTING.md ts/README.md
git commit -m "docs(repo): document layer-routed task graph (SMA-401)

Add a 'Task routing by layer' subsection to CONTRIBUTING (checks at the config
roots, build/typecheck/test per project), fix the config-only paragraph, and
update ts/README so the Test command uses the TypeScript-scoped query (ts test
is now per-project). py/README needs no change."
```

---

## Task 6: Full verification sweep

No file changes — confirm the ACs end-to-end before opening the PR.

- [ ] **Step 1: Resolved task lists (all assertions in one place)**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
for p in py ts paigasus-kernel-py paigasus-kernel-ts paigasus-console-ts commitlint-config-ts; do
  echo "=== $p ==="; moon project "$p"
done
```
Read the TASKS section of each. Expected: `py` → lint/fmt/typecheck/test; `ts` → lint/fmt; `paigasus-kernel-py` → build; `paigasus-kernel-ts` → build/typecheck/test; `paigasus-console-ts` → build/typecheck/test; `commitlint-config-ts` → test.

- [ ] **Step 2: Each whole-tree check runs once (no per-package duplication)**

```bash
moon run :typecheck    # py:typecheck once at root; ts typecheck per-package; NO paigasus-*-py:typecheck
moon run :test         # py:test once at root; ts test per-package; NO paigasus-*-py:test, NO ts:test
moon run :lint         # py:lint + ts:lint once each; NO per-package py/ts lint
moon run :fmt          # py:fmt + ts:fmt once each
moon run :build        # every py/packages/* + ts packages/apps; NO py:build / ts:build at the roots
```
Expected: all green; the printed target list matches the comments above (one root py check each; ts checks per-package; build per-package only).

- [ ] **Step 3: Affected-graph marks the root check (no coverage lost)**

```bash
f=$(find py/packages/paigasus-kernel/src -name '*.py' | head -1); echo "probing via $f"
printf '\n# affected-graph probe\n' >> "$f"
moon query projects --affected 2>/dev/null | grep -iE '"id": *"py"' && echo "py root affected ✓" || echo "see note below"
git checkout -- "$f"
```
Expected: the `py` root project is in the affected set (its `typecheck`/`test` inputs include `packages/*/src/**/*`), so a single package edit triggers the whole-tree root check exactly once — there is no `paigasus-kernel-py:typecheck` to run. If your Moon version's affected-query flags differ, the underlying guarantee is the resolved Inputs from Task 1 Step 5 (the root checks list `packages/*/src/**/*`), so cache invalidation on any package source edit hits the root task.

- [ ] **Step 4: Guards green**

```bash
moon run ts:check-config-only
```
Expected: no violations.

- [ ] **Step 5: Update the Linear issue / open the PR via the finishing skill**

Stop here and hand back. Branch is ready for `superpowers:finishing-a-development-branch` (PR auto-links to SMA-401 by branch name — do not attach the link manually).

---

## Notes for the implementer

- **Prototype-first:** Task 1 Steps 4–5 are the load-bearing verification of the whole approach (`inheritedBy.layers` semantics + fileGroup resolution on Moon 2.2.5). If either fails, stop and reconcile against the spec's "Open items" before continuing — don't push through to Task 3.
- **`layers` key:** the spec notes the project field is `layer:` (singular) while `inheritedBy` uses `layers:` (plural list). If `moon project py` in Task 1 Step 4 still shows `build`, the key/semantics differ on 2.2.5 — check `moon`'s schema (`https://moonrepo.dev/schemas/tasks.json`) and adjust before proceeding.
- **Commit hygiene:** the `commit-msg` lefthook hook runs commitlint; the messages above use allowed types/scopes (`refactor`/`docs`, `ci`/`repo`). A blank line before the body is required (already present).
- **No `timeout`:** macOS has no `timeout` binary; don't wrap `moon` commands in it.
```
