# SMA-356 — Set up Moon configuration

**Date:** 2026-05-26
**Linear:** [SMA-356](https://linear.app/smaschek/issue/SMA-356/set-up-moon-configuration)
**Status:** Design approved (brainstorm); staff-eng review pass incorporated (see
"Review incorporation"); pending final spec sign-off → implementation plan
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
| `CONTRIBUTING.md` (edit) | Add a **"Local development setup"** subsection: `proto` → `moon` install order. |
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
- **Workspace ↔ package-manager coherence check** (Moon project list vs.
  `Cargo.toml`/`uv`/`pnpm` workspace members) → **SMA-363**. Moon globs and the
  per-language workspace member lists must agree; nothing enforces it automatically.
  SMA-363 already verifies the affected-graph end-to-end and is the right home for a
  `moon query projects`-vs-members diff gate. (Review S2.)
- **Root `repo` project + `vcs.hooks` population** → **SMA-371**. That issue needs a
  `repo` project (for `moon run repo:install-hooks`) which does **not** exist in this
  issue's `projects:` list; it will add one via the sources-map form
  (`projects: { globs: [...], sources: { repo: '.' } }`). SMA-356 deliberately does
  **not** reserve it now — a root project without a `moon.yml` could pull the
  workspace root into the graph and jeopardize the clean-empty-workspace gates.
  SMA-356 also leaves `vcs.hooks` **empty** (per SMA-371: lefthook owns `.git/hooks`).
  (Review S4.)

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
  `unstable_python` toolchain plus a separate `unstable_uv` block (Moon 2.2.5
  built-ins — see decision 2).

### 2. Python/uv toolchains — `unstable_python` + `unstable_uv` (Moon 2.2.5 built-ins), with a documented fallback

Verified against the pinned Moon 2.2.5 binary (`moon toolchain info`): the built-in
toolchains are `unstable_python` and a **separate** `unstable_uv` — the unprefixed
`python`/`uv` keys are *not* built-in in 2.2.5 (the hosted JSON schema, which shows
`python`, is ahead of the 2.2.5 binary; the binary is authoritative). The Python
version pins under `unstable_python.version` (with `packageManager: 'uv'`); the
**uv** version pins under `unstable_uv.version` — it is *not* a nested field of
`unstable_python`. We document the fallback explicitly: **if these toolchains prove
unreliable, drop the blocks and run `uv` via plain `command` tasks in each Python
project.** That loses toolchain-layer version pinning for Python but gains
stability — a config-time switch, not a structural change.

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

`codeowners.sync: true` makes Moon (re)generate a CODEOWNERS file when a target
is run. SMA-355 created a static root `CODEOWNERS` (`* @SMK1085`) and explicitly
anticipated SMA-356 taking it over. Decision:

- Set `vcs.provider: 'github'` and `codeowners.globalPaths: { '*': ['@SMK1085'] }`
  in `workspace.yml` so the generated file preserves the default owner.
- Let Moon generate CODEOWNERS and **remove the now-redundant hand-written root
  file**, so there is exactly one (Moon-managed) CODEOWNERS.

> **⚠️ Field-name correction (not in the AC).** The AC and the Notion scoping
> doc both say `codeowners.syncOnRun: true`. Verified against current Moon docs
> (`config/workspace`, `guides/codeowners`), the actual field is **`sync`** —
> `syncOnRun` is **not** a valid current field and would silently never sync.
> This spec uses `sync: true`. The AC wording should be treated as a documentation
> bug; flag for correction in Linear. Re-confirm against the Moon version pinned
> in `.prototools` at implementation (schema validation will catch a mismatch).

**Sequencing (Review B2).** Moon's docs describe `sync` as aggregating *project*
owners; whether `globalPaths` **alone** (zero projects) emits a CODEOWNERS at all
is **unconfirmed**. So the static file is **not** deleted blind:

1. Run Moon on the empty workspace and inspect the generated file; confirm it
   exists and contains `* @SMK1085`.
2. Only then remove the conflicting static location, in the same commit.
3. If `globalPaths` alone produces nothing, keep a minimal static CODEOWNERS and
   record that Moon-managed generation only takes effect once real projects with
   `owners` land — do **not** leave the repo with zero CODEOWNERS.

Output path also needs confirming (GitHub provider → docs say `.github/CODEOWNERS`;
if so, the root `CODEOWNERS` is the file to delete). Note branch protection is
**not yet enabled** (SMA-355), and end-to-end CODEOWNERS-sync verification is owned
by **SMA-363** — so a transient gap here is low-risk but still avoided by the
sequencing above.

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
  client: 'git'       # Moon 2.x renamed `manager` → `client`.
  defaultBranch: 'main'
  provider: 'github'
  # `hooks` intentionally left unset — lefthook owns .git/hooks (per SMA-371).

codeowners:
  sync: true          # NOT `syncOnRun` — see decision §5; that AC name is invalid.
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

# Python/uv are Moon 2.2.5 built-in toolchains keyed `unstable_python` and the
# separate `unstable_uv` (verified via `moon toolchain info`; unprefixed
# `python`/`uv` are NOT built-in in 2.2.5). uv's version pins under `unstable_uv`,
# not nested in `unstable_python`. Fallback: drop these blocks and run uv via
# plain `command` tasks per project.
unstable_python:
  version: '3.12.<latest-patch>'
  packageManager: 'uv'
unstable_uv:
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

# Inserted into every inherited task's inputs so a workspace-level toolchain or
# global-task change busts caches. (Caching is ON by default, and an undeclared
# task `inputs` already defaults to all project files `**/*`, so per-project
# edits invalidate correctly without further config — see decision §note below.)
implicitInputs:
  - '/.moon/toolchain.yml'
  - '/.moon/tasks.yml'

taskOptions:
  outputStyle: 'buffer-only-failure'
```

**On caching and `inputs` (Review B1, evaluated and partially rejected).** The
review flagged "`cache: true` + templates with no `inputs` → stale green builds."
That premise is **inverted**: Moon's docs (`config/project`) state *"If not defined
or inherited, then all files within a project are considered an input (`**/*`),
excluding root-level tasks."* An undeclared `inputs` is therefore the **conservative**
default (any project file change invalidates the cache), not a stale-cache trap.
Disabling caching globally (`cache: false`) was rejected — caching + the
affected-graph is the core reason ADR-0008 chose Moon. The **one** legitimate gap —
shared/workspace files (e.g. `toolchain.yml`) sit outside a project's `**/*` — is
closed by the `implicitInputs` above. Templates additionally declare explicit
`inputs` (below) as good practice, matching the scoping doc's `contracts/moon.yml`.
`outputStyle: buffer-only-failure` is valid (docs confirm the value) and chiefly
affects *transitive* targets; directly-run tasks still stream, so local ergonomics
are unaffected (a `--output-style stream` note goes in CONTRIBUTING regardless).

### `.moon/templates/<lang>/`

Each language template directory contains:

- `template.yml` — template metadata: `title`, `description`, a `variables`
  block declaring `archetype` (an `enum` constrained to that language's valid
  archetypes, with a default), and any naming variables. The `description` must
  carry two caveats (Review S1, N2):
  - **Generated config references projects that may not exist yet.** The `service`
    archetype emits `dependsOn: [paigasus-proto, paigasus-kernel]` and
    `deps: ['contracts:generate']`, which only resolve after SMA-357/360. Generate
    into a workspace where those have landed, or hand-edit the references.
  - **Generated `moon.yml` is a starting point, not final.** In particular the
    `library` archetype emits **no** `dependsOn` on purpose — most libs depend on
    `paigasus-kernel`, but `paigasus-kernel` and `paigasus-proto` must not (self/
    cycle), so the author adds `dependsOn` by hand. (A blanket `dependsOn: kernel`
    default would be wrong for exactly those crates — why we don't auto-add it.)
- `moon.yml` — a Tera-templated project config that switches `type`, `dependsOn`,
  and task commands on `archetype`, and declares explicit `inputs` (see B1 note).

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
    inputs: ['@group(sources)', 'Cargo.toml']
{% if archetype == "service" %}    deps: ['contracts:generate', '^:build']{% endif %}
  test:
    command: 'cargo nextest run -p {{ project_name }}'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
{% if archetype == "service" %}    deps: ['contracts:generate']{% endif %}
  lint:
    command: 'cargo clippy -p {{ project_name }} -- -D warnings'
    inputs: ['@group(sources)', 'Cargo.toml']
  fmt:
    command: 'cargo fmt --check -p {{ project_name }}'
    inputs: ['@group(sources)']
```

(`outputs` are intentionally left unset for cargo tasks: build artifacts land in
the shared workspace `target/`, and cargo's own incrementality covers rebuilds —
the cache value here is at the test/lint **result** layer, keyed on `inputs`.)

`python/` and `typescript/` follow the same shape with their respective
toolchains:

- **python** — `archetype` ∈ {`library`, `service`}; tasks for `ruff` (lint/format),
  `basedpyright` (typecheck), `pytest` (test) run via `uv`; `service` adds a `start`
  task and `type: application`. The `template.yml` description notes (Review S3):
  a Python package that builds native code via **maturin** must `dependsOn` the
  corresponding Rust binding crate so Moon provisions **both** the Python and Rust
  toolchains in the task context. Not exercised yet (first surfaces in the
  kernel-bindings work, post-MVP) — recorded so SMA-358 doesn't rediscover it cold.
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
  (+pnpm), and Python 3.12.x (+uv) under `unstable_python` + `unstable_uv`, all at
  concrete committed versions.
- `.moon/tasks.yml` defines `sources` and `tests` file groups and global task
  defaults.
- The three generator templates exist and `moon generate` lists/recognizes them;
  a dry-run generate of each archetype renders a syntactically valid `moon.yml`.
- `moon query projects` runs without error and returns **zero** projects — proving
  the globs parse and resolve cleanly on the empty workspace. (Proportionate
  substitute for Review S5's throwaway-stub idea: the globs are verbatim from the
  canonical scoping doc and SMA-363 owns the end-to-end affected-graph test, so a
  stub-then-delete project adds churn without commensurate assurance.)
- `codeowners.sync: true` is set (not `syncOnRun`); running Moon produces a
  CODEOWNERS containing `* @SMK1085`. Apply the §5 sequencing: confirm the
  generated file exists **before** removing the static one; end with exactly one
  CODEOWNERS and never zero.
- **`moon ci :build` runs cleanly** on the empty workspace (one task-less project,
  no affected tasks, no errors) — AC gate 1.
- **`moon ci :test` succeeds** as a no-op across all language workspaces — AC gate 2.
  (Moon 2.x: `moon ci` takes explicit `[TARGETS]`; `moon check` takes project IDs,
  so `moon check :build` is invalid. SMA-363's AC was updated to this form.)
- After the first `moon ci`/`moon sync`, no un-ignored intermediate state appears
  under `.moon/` (`git status` clean; `.gitignore` already covers
  `.moon/cache/` + `.moon/docker/`). (Review N7.)
- `CONTRIBUTING.md` has a "Local development setup" subsection documenting the
  `proto` → `moon` install order (Review N3); a fresh clone can reach a working
  `moon` with only that documented prerequisite (feeds SMA-363's fresh-clone AC).
- `.moon/.gitkeep` is deleted; `git status` clean afterward. (Review N8.)
- `git status` is clean on the feature branch after commit; a PR is opened to `main`.

## Open items to confirm during implementation

1. **CODEOWNERS on a project-less workspace** — does `codeowners.sync` + `globalPaths`
   alone emit a file (and where: root vs `.github/CODEOWNERS`)? Resolve empirically
   before deleting the static file (decision §5). *(was two items; `moon check :build`
   semantics are now resolved — SMA-363 uses that exact form.)*
2. **Toolchain download on empty workspace** (Review N4) — does `moon ci` provision
   any toolchain when there are no tasks to run? Expected: no. Resolve with one
   `moon ci` invocation on the feature branch and record the answer here and for
   SMA-361's cache strategy.
3. **Target-scope tokens vs pinned Moon** (Review N5, minor) — confirm `^:build`
   (and `~:` if used) are accepted by the Moon version pinned in `.prototools`.
   These are current, stable scopes (SMA-363 relies on `:build`/`:test`), so this is
   a sanity check, not an expected break.

## Review incorporation

Staff-eng review pass (review doc removed after incorporation, per repo
convention); each item verified against Moon docs / the real sibling issues
before acting:

| Item | Disposition |
|------|-------------|
| **B1** cache + missing `inputs` | **Partly rejected.** Premise inverted — undeclared `inputs` defaults to `**/*` (all project files), so it's conservative, not stale. Kept caching on (the point of ADR-0008); added `implicitInputs` for `toolchain.yml`/`tasks.yml` and explicit `inputs` in templates for the narrow shared-file gap. |
| **B2** CODEOWNERS delete sequencing | **Adopted.** §5 now verifies the generated file exists before removing the static one, never leaving zero. Impact noted as lower than stated (branch protection off; SMA-363 owns e2e). |
| **S1** templates ref future projects | **Adopted** — `template.yml` description caveat. |
| **S2** glob ↔ member coherence | **Adopted as handoff** to SMA-363 (out-of-scope note). |
| **S3** maturin cross-toolchain | **Adopted** — Python `template.yml` note. |
| **S4** `repo` project for SMA-371 | **Confirmed real; "reserve now" rejected.** Handoff note instead (reserving a root project risks the empty-workspace gates; review's syntax was invalid). Also captured SMA-371's `vcs.hooks`-empty constraint. |
| **S5** throwaway `_smoke` project | **Pushed back.** Replaced with `moon query projects` returns-zero check. |
| **N1** stale Notion pins | **Adopted as separate action** (needs sign-off — edits canonical Notion). |
| **N2** library `dependsOn: kernel` | **Partly rejected.** Auto-adding kernel is wrong for kernel/proto themselves; documented as hand-edit instead of a Tera var. |
| **N3** install order in CONTRIBUTING | **Adopted** — now in scope; grounded in SMA-363 + SMA-371. |
| **N4** toolchain download on empty | **Adopted** — open item #2. |
| **N5** `^:`/`~:` syntax | **Adopted (minor)** — open item #3; current stable scopes. |
| **N6** outputStyle local ergonomics | **Adopted (mitigated)** — affects transitive targets only; CONTRIBUTING `--output-style stream` note. |
| **N7/N8** gitignore/.gitkeep sanity | **Adopted** — verification checklist. |

**New finding the review missed:** the AC's and scoping doc's `codeowners.syncOnRun`
is not a valid current Moon field — the field is **`codeowners.sync`**. Corrected
throughout; flagged for an AC fix in Linear (decision §5).

## Post-implementation outcomes (Moon 2.2.5)

Recorded after implementing SMA-356 (PR #2). The build matched this design; a few
values and command/field names resolved differently than the 1.x-era scoping doc
assumed:

- **Moon version:** pinned to **2.2.5** in `.prototools` (latest stable at build time).
- **Resolved toolchain pins:** Rust 1.95.0, Node 22.22.3, pnpm 11.3.0, Python
  3.12.13, uv 0.11.16 — Python under `unstable_python`, uv under the **separate**
  `unstable_uv` block (Moon 2.2.5 built-ins, verified via `moon toolchain info`;
  the unprefixed `python`/`uv` keys are not built-in in 2.2.5, and uv is not a
  nested field of `unstable_python`).
- **Moon 1.x → 2.x renames applied:** `vcs.manager` → `vcs.client`; the codeowners
  field is `sync` (not `syncOnRun`); the sync subcommand is `moon sync code-owners`.
- **Open item #1 (CODEOWNERS on empty workspace) — RESOLVED:** `globalPaths` alone
  *does* generate a file even with no per-project owners. Moon wrote
  `.github/CODEOWNERS` (`* @SMK1085`); the static root `CODEOWNERS` was removed.
  Exactly one authoritative CODEOWNERS results.
- **Project graph:** Moon resolves **one task-less project** (`contracts`, a dir with
  only a README), not zero — the `rs`/`py`/`ts` globs still match nothing. The gates
  pass because a task-less project has nothing to run. (This refines the "zero
  projects" framing used above.)
- **Open item #2 (toolchain download on empty) + AC gate commands — RESOLVED:** in
  Moon 2.x `moon ci` requires explicit `[TARGETS]` and `moon check` takes project IDs,
  so the AC's literal `moon ci` / `moon check :build` do not run as written
  (bare `moon ci` errors `app::tty::required_id` in non-TTY). Verified gate:
  **`moon ci :build` → exit 0** (no affected tasks; no toolchain download on the empty
  workspace). `moon check :build` is invalid 2.x syntax. SMA-363's AC was updated to
  `moon ci :build`/`:test` and SMA-361 was flagged to use explicit targets +
  `codeowners.sync`.

## References

- **ADR-0008** — Moon as the polyglot task orchestrator (decision, rationale,
  revisit triggers).
- **Polyglot Monorepo Scoping § 1** — `workspace.yml` / `toolchain.yml` /
  per-project `moon.yml` examples; affected-graph behavior. (Notion, internal.)
- **SMA-355 design** — `docs/superpowers/specs/2026-05-26-bootstrap-monorepo-design.md`
  (CODEOWNERS interim decision §4; `.moon/.gitkeep` removal note §1).
