# SMA-356 — Set up Moon configuration

**Date:** 2026-05-26
**Linear:** [SMA-356](https://linear.app/smaschek/issue/SMA-356/set-up-moon-configuration)
**Status:** Design approved (brainstorm), pending spec review → implementation plan
**Blocked by:** SMA-355 (bootstrap monorepo structure) — merged.
**Blocks:** SMA-357, SMA-358, SMA-359, SMA-360, SMA-361, SMA-363, SMA-371.

## Goal

Wire [Moon](https://moonrepo.dev) as the polyglot task orchestrator for
`paigasus-core`: workspace project graph, toolchain version pinning, global task
defaults, and per-language project-config generator templates. This is the
second Phase-1 issue; every later workspace-bootstrap issue (Cargo, uv, pnpm,
buf) and the CI workflow depend on this config existing.

Per **ADR-0008** (Moon as the polyglot task orchestrator). Config snippets are
adapted from **Polyglot Monorepo Scoping § 1**.

## Scope

### In scope

| Path | Purpose |
|------|---------|
| `.moon/workspace.yml` | Project globs, VCS config, Moon-owned CODEOWNERS, generator registration. |
| `.moon/toolchain.yml` | Pin Rust / Node+pnpm / Python+uv toolchains. |
| `.moon/tasks.yml` | Global inherited task defaults + `sources`/`tests` file groups. |
| `.moon/templates/rust/` | Generator template: Rust `library`\|`service` archetypes. |
| `.moon/templates/python/` | Generator template: Python `library`\|`service` archetypes. |
| `.moon/templates/typescript/` | Generator template: TS `library`\|`app` archetypes. |
| `.prototools` | Pin the Moon (and proto) binary version for reproducible runs. |
| `CODEOWNERS` (Moon-generated) | Replaces the static hand-written root file. |
| _delete_ `.moon/.gitkeep` | Interim placeholder from SMA-355; real config now lands. |

### Out of scope (owned by other issues)

Deliberately **not** created here:

- Real Cargo workspace + `rs/crates/{libs,bindings,services}/` projects → **SMA-357**
- Real uv workspace + `py/packages/*` projects → **SMA-358**
- Real pnpm workspace + `ts/packages/*`, `ts/apps/*` projects → **SMA-359**
- `contracts/` proto sources, `buf.yaml`, `buf.gen.yaml`, and `contracts/moon.yml` → **SMA-360**
- The GitHub Actions CI workflow that runs `moon ci --base origin/main` → **SMA-361**
- Local git hooks (commit-msg / branch-name) → **SMA-371**
- Per-language inherited task files (`.moon/tasks/rust.yml`, etc.) — added alongside
  each language's real workspace by SMA-357/358/359 when concrete tasks exist.
- A dedicated Rust **binding** archetype (PyO3/napi/wasm build tasks) — folded into
  the `rust`/`library` archetype for now; SMA-357 may add it.

## Key design decisions

These four were resolved during brainstorming.

### 1. Toolchain versions — bump to current stable, pin exact patches

The scoping doc's pins (`rust 1.83.0`, `node 22.11.0`, `pnpm 9.15.0`,
`python 3.12.7`, `uv 0.5.0`) are stale relative to today (2026-05-26) and the
local environment (cargo 1.94.1, pnpm 10.33.2, uv 0.11.7). The AC also says
"Rust **latest stable**". Decision: pin **current stable** versions within the
AC's constraints, resolving the exact latest patch for each via `proto` at
implementation time and committing it literally (not a floating partial
version — reproducibility is a stated goal, see decision 4):

- **Rust** — latest stable (≈ 1.94.x), components `rustfmt` + `clippy`, bin `cargo-nextest`.
- **Node** — latest 22.x LTS patch, package manager `pnpm` (current 10.x).
- **Python** — latest 3.12.x patch, package manager `uv` (current 0.11.x), under the
  `unstable_python` toolchain key.

### 2. Python toolchain stays on `unstable_python`, with a documented fallback

Per the AC and ADR-0008, Moon's Python toolchain is still on the
`unstable_python` tier as of mid-2026. We engage it (it gives version pinning)
but document the fallback explicitly: **if `unstable_python` proves unreliable,
remove it and run `uv` via plain `command` tasks in each Python project.** That
loses toolchain-layer version pinning for Python but gains stability. This is a
config-time switch, not a structural change.

### 3. `moon.yml` templates — Moon's generator system, one per language, archetype-parameterized

The AC asks for "per-project `moon.yml` templates for library, service, and app
types committed as reference docs". Because `type` (`library`/`application`) and
the underlying task commands differ by language, a single generic per-archetype
template cannot work. Decision: **one Moon generator template per language**,
each declaring an `archetype` variable that selects the project type via Tera
conditionals in the rendered `moon.yml`.

| Template | `archetype` values | Covers project globs |
|----------|--------------------|----------------------|
| `.moon/templates/rust/` | `library` \| `service` | `rs/crates/libs/*`, `rs/crates/services/*` |
| `.moon/templates/python/` | `library` \| `service` | `py/packages/*` |
| `.moon/templates/typescript/` | `library` \| `app` | `ts/packages/*`, `ts/apps/*` |

- These are real, usable generators (`moon generate rust …`) registered via
  `generator.templates` in `workspace.yml`; SMA-357/358/359 can scaffold their
  first projects from them. They double as the AC's "reference docs".
- **Python includes a `service` archetype** (e.g. a future `paigasus-workflows`),
  not just `library`.
- Rust **bindings** reuse the `rust`/`library` archetype for now (see Out of scope).
- `contracts` gets **no** template — it is a singleton whose `moon.yml` is authored
  directly in SMA-360, not a repeated archetype.

### 4. Moon binary pinned via `.prototools`

Moon (and `proto`) are not installed locally. Beyond installing them to run the
AC verification gates, we commit a `.prototools` pinning the Moon binary version
so `moon ci` is reproducible across the maintainer's machine and the future CI
workflow (SMA-361 inherits it). Exact Moon version = latest stable, resolved at
implementation.

### 5. CODEOWNERS — Moon becomes the single source of truth

`codeowners.syncOnRun: true` makes Moon (re)generate a CODEOWNERS file on every
run. SMA-355 created a static root `CODEOWNERS` (`* @SMK1085`) and explicitly
anticipated SMA-356 taking it over. Decision:

- Set `vcs.provider: 'github'` and `codeowners.globalPaths: { '*': ['@SMK1085'] }`
  in `workspace.yml` so the generated file preserves the default owner.
- Let Moon generate CODEOWNERS and **remove the now-redundant hand-written root
  file**, so there is exactly one (Moon-managed) CODEOWNERS.
- **Open implementation detail to verify against Moon docs:** Moon's output path
  for the GitHub provider (root `CODEOWNERS` vs `.github/CODEOWNERS`). Whichever
  Moon writes becomes the canonical file; the other location must not retain a
  stale copy. If Moon writes `.github/CODEOWNERS`, delete the root file; if it
  writes the root file, it overwrites in place.

## Configuration content

### `.moon/workspace.yml`

```yaml
$schema: 'https://moonrepo.dev/schemas/workspace.json'

projects:
  - 'contracts'
  - 'rs/crates/libs/*'
  - 'rs/crates/bindings/*'
  - 'rs/crates/services/*'
  - 'py/packages/*'
  - 'ts/packages/*'
  - 'ts/apps/*'

vcs:
  manager: 'git'
  defaultBranch: 'main'
  provider: 'github'

codeowners:
  syncOnRun: true
  globalPaths:
    '*': ['@SMK1085']

generator:
  templates:
    - './.moon/templates'
```

The globs match **zero** directories today (all workspace dirs hold only a
README), so Moon resolves **zero projects**. This is the property that makes the
AC's verification gates pass on the empty workspace.

### `.moon/toolchain.yml`

Exact patch versions resolved at implementation (shown here as the target):

```yaml
$schema: 'https://moonrepo.dev/schemas/toolchain.json'

node:
  version: '22.<latest-lts-patch>'
  packageManager: 'pnpm'
  pnpm:
    version: '10.<latest>'

rust:
  version: '1.<latest-stable>'
  components:
    - 'rustfmt'
    - 'clippy'
  bins:
    - 'cargo-nextest'

# Moon's Python toolchain is still on the 'unstable' tier as of mid-2026.
# Pin uv explicitly; if this tier proves unreliable, remove this block and run
# uv via plain `command` tasks per project (loses pinning, gains stability).
unstable_python:
  version: '3.12.<latest-patch>'
  packageManager: 'uv'
  uv:
    version: '0.11.<latest>'
```

### `.moon/tasks.yml`

Global, language-agnostic defaults inherited by every project. (Not present in
the scoping doc — designed here from the AC.)

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

taskOptions:
  cache: true
  outputStyle: 'buffer-only-failure'
```

### `.moon/templates/<lang>/`

Each language template directory contains:

- `template.yml` — template metadata: `title`, `description`, a `variables`
  block declaring `archetype` (an `enum` constrained to that language's valid
  archetypes, with a default), and any naming variables.
- `moon.yml` — a Tera-templated project config that switches `type`, `dependsOn`,
  and task commands on `archetype`.

Illustrative `rust/moon.yml` (final form firmed up in the plan):

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
{% if archetype == "service" %}type: 'application'{% else %}type: 'library'{% endif %}
language: 'rust'
{% if archetype == "service" %}
dependsOn:
  - 'paigasus-proto'
  - 'paigasus-kernel'
{% endif %}
tasks:
  build:
    command: 'cargo build -p {{ project_name }}{% if archetype == "service" %} --release{% endif %}'
{% if archetype == "service" %}    deps: ['contracts:generate', '^:build']{% endif %}
  test:
    command: 'cargo nextest run -p {{ project_name }}'
{% if archetype == "service" %}    deps: ['contracts:generate']{% endif %}
  lint:
    command: 'cargo clippy -p {{ project_name }} -- -D warnings'
  fmt:
    command: 'cargo fmt --check -p {{ project_name }}'
```

`python/` and `typescript/` follow the same shape with their respective
toolchains:

- **python** — `archetype` ∈ {`library`, `service`}; tasks for `ruff` (lint/format),
  `basedpyright` (typecheck), `pytest` (test) run via `uv`; `service` adds a `start`
  task and `type: application`.
- **typescript** — `archetype` ∈ {`library`, `app`}; tasks for `tsc`/build, `eslint`,
  `prettier`, and test via `pnpm`; `app` sets `type: application` (Next.js).

The exact task wiring for python/typescript is intentionally left to the plan and
will mirror the conventions the corresponding bootstrap issues (358/359) adopt;
the templates are reference scaffolds, not the authoritative project configs.

### `.prototools`

```toml
moon = "1.<latest-stable>"
```

## Verification / done criteria

Maps to the issue's acceptance criteria.

- `proto` + `moon` installed (Moon pinned via `.prototools`); `moon --version`
  matches the pin.
- `.moon/workspace.yml` exists with all seven project globs, `vcs` config, and
  `codeowners` config.
- `.moon/toolchain.yml` pins Rust (+rustfmt/clippy/cargo-nextest), Node 22.x LTS
  (+pnpm), and Python 3.12.x (+uv) under `unstable_python`, all at concrete
  committed versions.
- `.moon/tasks.yml` defines `sources` and `tests` file groups and global task
  defaults.
- The three generator templates exist and `moon generate` lists/recognizes them;
  a dry-run generate of each archetype renders a syntactically valid `moon.yml`.
- `codeowners.syncOnRun: true` is set; running Moon produces a CODEOWNERS that
  contains `* @SMK1085`; there is exactly one CODEOWNERS file in the repo and the
  static hand-written one is gone.
- **`moon ci` runs cleanly** on the empty workspace (zero projects, no project
  errors) — AC gate 1.
- **`moon check :build` succeeds** as a no-op across all language workspaces — AC
  gate 2. (Verify exact CLI phrasing on the empty workspace; if Moon's CLI
  prefers `moon run :build` / `moon check --all`, document the working form.)
- `.moon/.gitkeep` is deleted.
- `git status` is clean on the feature branch after commit; a PR is opened to `main`.

## Open items to confirm during implementation

1. Moon's CODEOWNERS output path for the `github` provider (root vs `.github/`).
2. Exact CLI semantics of `moon check :build` on a project-less workspace.
3. Whether `moon ci` on an empty workspace attempts any toolchain download
   (expected: no — no tasks to run, so no toolchain is provisioned; this is why
   the gates are satisfiable before any real project exists).

## References

- **ADR-0008** — Moon as the polyglot task orchestrator (decision, rationale,
  revisit triggers).
- **Polyglot Monorepo Scoping § 1** — `workspace.yml` / `toolchain.yml` /
  per-project `moon.yml` examples; affected-graph behavior. (Notion, internal.)
- **SMA-355 design** — `docs/superpowers/specs/2026-05-26-bootstrap-monorepo-design.md`
  (CODEOWNERS interim decision §4; `.moon/.gitkeep` removal note §1).
