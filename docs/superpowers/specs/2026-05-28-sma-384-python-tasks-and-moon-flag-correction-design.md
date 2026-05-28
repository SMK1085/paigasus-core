# SMA-384 — Wire `.moon/tasks/python.yml`, slim the python scaffold template, clean up `py/moon.yml`, and correct the bogus `--output-style stream` claim

**Status:** Designed (brainstorming complete; staff-eng review pass applied 2026-05-28)
**Date:** 2026-05-28
**Linear:** [SMA-384](https://linear.app/smaschek/issue/SMA-384/wire-python-language-tasks-moontaskspythonyml-correct-moon-output)
**Branch:** `feature/sma-384-wire-python-language-tasks-moontaskspythonyml-correct-moon`
**Targets:** `main` (SMA-383 PR #7 has merged as `0a96b2f`; this branch is rebased onto the new main with just the spec commit).
**References:** SMA-381 (introduced `inheritedBy.languages: ['rust']` pattern in `.moon/tasks/rust.yml`); SMA-380 (`-py`/`-ts` id suffix); SMA-358 (py uv workspace bootstrap, including the existing scaffold template); SMA-356 (originator of the wrong `--output-style stream` claim in CONTRIBUTING.md).

## Goal

Close out the last scaffolding-consistency gaps and correct two pieces of wrong documentation:

1. **Wire python language tasks.** Add `.moon/tasks/python.yml` so python projects inherit standard
   `build`/`lint`/`fmt`/`typecheck`/`test` tasks the same way rust crates inherit from
   `.moon/tasks/rust.yml`. After this, `moon generate python` produces projects that resolve cleanly
   (no `unknown_file_group` validation error).
2. **Slim the python scaffold template** to match the rust template's shape — header (`$schema` →
   `id` → `layer` → `language`) plus a service-archetype-only override, no library-archetype tasks
   block.
3. **Clean up `py/moon.yml`'s now-redundant tasks block.** After python.yml lands, py-workspace's
   local tasks block defines the same names + commands as the inherited tasks. Removing it
   eliminates the dead-by-design redundancy and makes the inherited file the single source of
   truth for what py tasks do.
4. **Harmonize the task name `format` → `fmt`** in python.yml so `moon ci :fmt` covers both rust
   crates and python projects without a per-language alternation. (Rust already calls it `fmt`;
   python.yml's `format` would have created a cross-stack consistency gap.)
5. **Correct the bogus `--output-style stream` claim** in both `CLAUDE.md` and `CONTRIBUTING.md`.
   Moon 2.2.5 has no such CLI flag on `moon ci` or `moon run`; `outputStyle` is a per-task config
   option (set globally in `.moon/tasks.yml`'s `taskOptions.outputStyle`, currently
   `buffer-only-failure`).
6. **Replace Notion "Polyglot Monorepo Scoping" § 1's moon.yml content with a one-line redirect to
   CONTRIBUTING.md.** Notion stops trying to maintain a competing copy of conventions; CONTRIBUTING.md
   becomes the durable single source of truth. Sidecar — no file change in this repo; tracked here
   so the drift can't recur.

After this lands, no scaffolding-consistency follow-ups remain across `rs` + `py` + `ts-template` +
`contracts-template`, and the in-repo docs that describe Moon CLI behaviour describe it correctly.

## Decision

Three commits on the SMA-384 branch:

- `docs(repo):` — `CLAUDE.md:15` and `CONTRIBUTING.md:50-52` corrections. Pure prose change.
- `feat(py):` — adds `.moon/tasks/python.yml` and registers it in `.moon/tasks.yml`'s
  `implicitInputs`. py packages start inheriting standard tasks after this commit.
- `chore(py):` — slims `.moon/templates/python/moon.yml`'s tasks block AND removes
  `py/moon.yml`'s redundant tasks block. Both changes are scaffolding/resolution cleanup, not a
  behavior change for the actual commands.

rust.yml stays as-is — the cross-language name harmonization happens by using `fmt` in python.yml
(not by renaming rust's existing `fmt`).

Notion redirect is performed *before* the PR opens, but tracked in this spec's acceptance criteria
so the recurring drift problem ends.

The CLAUDE.md "other improvements" audit (per the `Fix CLAUDE.md, also check for other
improvements to it` instruction) was performed inline during brainstorming. Only the `--output-style`
line was factually wrong; the rest of CLAUDE.md is accurate. No additional rewrites.

## Changes

### Change A — `CLAUDE.md:15` and `CONTRIBUTING.md:50-52` correction (`docs(repo)`)

**`CLAUDE.md`, line 15.** Replace the wrong bullet:

```markdown
- Append `--output-style stream` to watch a long task (default is `buffer-only-failure`).
```

with:

```markdown
- Task output style is set in `.moon/tasks.yml` (`taskOptions.outputStyle`,
  currently `buffer-only-failure`). Moon 2.2.5 has no per-invocation CLI flag
  for it; to stream a specific task locally, set `options.outputStyle: 'stream'`
  on the task definition.
```

**`CONTRIBUTING.md`, lines 50–52.** Replace the blockquote:

```markdown
> Output is buffered for passing tasks (`buffer-only-failure`). To watch a long
> task stream locally, append `--output-style stream`, e.g.
> `moon run <project>:test --output-style stream`.
```

with:

```markdown
> Output is buffered for passing tasks (`buffer-only-failure`, set as
> `taskOptions.outputStyle` in `.moon/tasks.yml`). Moon 2.2.5 has no CLI flag
> to override this per invocation; to stream a specific task locally, set
> `options.outputStyle: 'stream'` on the task definition.
```

**CLAUDE.md "other improvements" audit (conducted during brainstorming):**

| Line / item | Verified | Action |
|---|---|---|
| Setup commands (`proto install`, `moon`) | Accurate | No change. |
| `moon ci :build` / `:test` with explicit targets | Accurate; bare `moon ci` does fail in non-TTY | No change. |
| `--output-style stream` claim | **Wrong** (Moon 2.2.5 has no such flag) | Fixed (this change). |
| `cargo` commands | Verified all still valid for the rust workspace | No change. |
| Architecture summary (contracts/buf, rs/crates, kernel-in-rust) | Matches current repo + ADRs | No change. |
| SPDX header rule (line 30–31) | Says "every source file"; covered by CONTRIBUTING.md's exemption rules (SMA-383) | No change — CLAUDE.md is intentionally a high-level overview; CONTRIBUTING.md is the authoritative reference. |
| Moon 2.2.5 gotchas (`vcs.client`, `codeowners.sync`, `unstable_python`/`unstable_uv`) | Verified against `.moon/workspace.yml` and `.moon/toolchain.yml` | No change. |
| `cargo nextest --no-tests=pass` | Verified | No change. |
| `.github/CODEOWNERS` Moon-generated note | Accurate | No change. |
| `vcs.hooks` + lefthook note (SMA-371) | Matches `.moon/workspace.yml`'s comment | No change. |
| Workflow (brainstorm → spec → plan → implement) | Matches actual practice | No change. |

The audit surfaces nothing else factually wrong in CLAUDE.md.

### Change B — `.moon/tasks/python.yml` (new file, `feat(py)`)

Mirror `.moon/tasks/rust.yml`'s shape exactly — `inheritedBy.languages: ['python']`, file groups,
and the standard tasks. Commands and inputs are lifted from the current python scaffold template
(which Change D strips) and `py/moon.yml`'s current tasks block (which Change D2 strips), merged.
Task name `fmt` (not `format`) matches rust.yml's convention so `moon ci :fmt` covers both stacks.

```yaml
$schema: 'https://moonrepo.dev/schemas/tasks.json'

# Moon does NOT scope task files by filename — scope explicitly so these python
# commands only attach to python projects (not rust, and not contracts/ts).
#
# `start` is intentionally absent from this file; service-archetype projects
# get it from the python scaffold template, which needs per-project Tera
# variables (the module path) that Moon's task-file syntax can't reach. A
# hand-written python service (none today) would need to add `start` to its
# own moon.yml.
inheritedBy:
  languages: ['python']

fileGroups:
  sources:
    - 'src/**/*'
  tests:
    - 'tests/**/*'
    - '**/*_test.py'
    - '**/test_*.py'

tasks:
  build:
    command: 'uv build'
    inputs: ['@group(sources)', 'pyproject.toml']
    outputs: ['dist']
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

Notes:

- **Task name `fmt`, not `format`** — harmonizes with rust.yml so `moon ci :fmt` covers both stacks.
  The command stays `uv run ruff format --check .` — only the Moon task name changes.
  ruff itself is still called via its `format` subcommand; this is purely Moon-side naming.
- **`/py/uv.lock`** uses Moon's workspace-anchor syntax — every python package shares the single
  `py/uv.lock` lockfile (uv workspace), not per-package locks. Same input shape as the current
  template.
- **`conftest.py`** is included in the `test` task's inputs to preserve `py/conftest.py`'s cache
  invalidation (it was in `py/moon.yml`'s test task). Resolves per-project: present at
  `py/conftest.py` for py-workspace, absent for child packages (Moon tolerates missing literal-path
  inputs without error — they simply don't contribute to cache keys).
- **Test discovery patterns** (`**/*_test.py`, `**/test_*.py`) cover both `_test.py` suffix and
  `test_*.py` prefix conventions; pytest picks up both.
- **No `start` task here** — see the file's header comment.

### Change C — `.moon/tasks.yml` `implicitInputs` (same `feat(py)` commit)

Register the new language tasks file so a change to `python.yml` busts every python task's cache:

```yaml
implicitInputs:
  - '/.moon/toolchain.yml'
  - '/.moon/tasks.yml'
  - '/.moon/tasks/rust.yml'
  - '/.moon/tasks/python.yml'   # added
```

Two lines edited (one add, one in context). Same commit as Change B since the new file is useless
without the cache-bust registration.

### Change D — Python scaffold template slim-down (`chore(py)`)

After python.yml inherits the library-archetype tasks, the template's tasks block is redundant for
non-service archetypes. Slim it to match rust template's shape — only the service-archetype `start`
task remains:

**Before** (current state, post-SMA-383):

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: '{{ name }}-py'
layer: '{% if archetype == "service" %}application{% else %}library{% endif %}'
language: 'python'
tasks:
  build:
    command: 'uv build'
    inputs: ['@group(sources)', 'pyproject.toml']
    outputs: ['dist']
  lint:
    command: 'uv run ruff check .'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock']
  format:
    command: 'uv run ruff format --check .'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock']
  typecheck:
    command: 'uv run basedpyright'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock']
  test:
    command: 'uv run pytest'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock']
{%- if archetype == "service" %}
  start:
    command: 'uv run python -m {{ name | replace(from="-", to="_") }}'
    options:
      cache: false
      persistent: true
{%- endif %}
```

**After:**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: '{{ name }}-py'
layer: '{% if archetype == "service" %}application{% else %}library{% endif %}'
language: 'python'
{%- if archetype == "service" %}
tasks:
  start:
    command: 'uv run python -m {{ name | replace(from="-", to="_") }}'
    options:
      cache: false
      persistent: true
{%- endif %}
```

After this, library- and binding-archetype renders produce a 4-line `moon.yml` (header only); the
service archetype adds a `tasks:` block with the single `start` task. Matches the rust template's
exact shape.

### Change D2 — `py/moon.yml` redundant tasks-block removal (same `chore(py)` commit as Change D)

After python.yml lands, `py/moon.yml`'s tasks block defines the same task names and commands as the
inherited tasks (modulo the `format` → `fmt` rename). The local definition shadows the
inherited one, which means future python.yml edits silently no-op for py-workspace. Remove the
local tasks block so the inherited file becomes the single source of truth.

**Before:**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

layer: 'configuration'
language: 'python'

# The global fileGroups in .moon/tasks.yml assume src/ at the project root; the py workspace
# keeps sources under packages/*/src, so redefine them here.
fileGroups:
  sources:
    - 'packages/*/src/**/*'
  tests:
    - 'packages/*/tests/**/*'

tasks:
  lint:
    command: 'uv run ruff check .'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', 'uv.lock']
  format:
    command: 'uv run ruff format --check .'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', 'uv.lock']
  typecheck:
    command: 'uv run basedpyright'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', 'uv.lock']
  test:
    command: 'uv run pytest'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', 'uv.lock', 'conftest.py']
```

**After:**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

layer: 'configuration'
language: 'python'

# The global fileGroups in .moon/tasks.yml assume src/ at the project root; the py workspace
# keeps sources under packages/*/src, so redefine them here. The inherited python tasks
# (.moon/tasks/python.yml) resolve `@group(sources)` and `@group(tests)` against these
# overridden fileGroups, so workspace-wide invocations still target the package tree.
fileGroups:
  sources:
    - 'packages/*/src/**/*'
  tests:
    - 'packages/*/tests/**/*'
```

The `fileGroups` override stays — it's what re-scopes `sources` to `packages/*/src/**/*` for the
inherited tasks. The comment is updated to name the inheritance interaction.

Behavior parity check:

- **`moon run py:lint`** — was: local `uv run ruff check .` with explicit inputs; now: inherited
  `uv run ruff check .` (identical command) with `inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock']`. The local override's `uv.lock` (resolves to `py/uv.lock`) and inherited's `/py/uv.lock` are the same file.
- **`moon run py:fmt`** — was: local `format` task with `uv run ruff format --check .`; now:
  inherited `fmt` task with the same command. **Task name changes from `:format` to `:fmt`** for
  py-workspace; same change as for child packages.
- **`moon run py:typecheck`**, **`moon run py:test`** — identical command + inputs (modulo
  `uv.lock` vs `/py/uv.lock` resolution; same file). `conftest.py` is preserved in the inherited
  test task's inputs.

### Change E — Notion "Polyglot Monorepo Scoping" § 1 → redirect to CONTRIBUTING.md (out-of-repo, before PR opens)

Done before opening the PR (per brainstorming timing choice). The durable strategy:

1. Fetch the current Notion doc via the Notion MCP.
2. Identify § 1's moon.yml examples + prose.
3. Replace the section's content with a one-line redirect (preserve the heading):

   > These conventions are documented in `CONTRIBUTING.md` in the
   > [paigasus-core](https://github.com/SMK1085/paigasus-core/blob/main/CONTRIBUTING.md)
   > repository — the source of truth. See *§ Code conventions → Moon project files* for the
   > `moon.yml` field order and `layer:` values, and *§ Code conventions* for the SPDX guidance.

4. Apply the approved update via the Notion MCP.
5. Tick this off in the acceptance criteria.

After this, Notion no longer maintains a competing copy of the conventions — drift can't recur
because there's no parallel content to drift from. Permanently closes the "Notion scoping-doc
drift" review note that's appeared on every prior PR.

## Commit grouping

```
docs(repo): correct moon --output-style claim in CLAUDE.md and CONTRIBUTING.md (SMA-384)
feat(py):   add .moon/tasks/python.yml + register implicit input (SMA-384)
chore(py):  slim python scaffold template and remove py/moon.yml tasks block (SMA-384)
```

The split puts behavior change (`feat(py)`, which makes existing py packages start inheriting
tasks) in its own commit, separate from the template + py-root-tasks-block changes (`chore(py)`,
both of which only affect resolution/scaffolding, not what the commands do). The `docs(repo)`
commit is pure prose.

## Post-implementation corrections

Two prescriptive details in this spec turned out to be wrong when the change actually ran against
Moon 2.2.5 + the uv workspace. The implementation deviated from the spec text; both deviations are
empirically justified. Recorded here so the spec doesn't trap future readers (e.g. SMA-359 mirroring
this pattern for `typescript.yml`).

### Correction 1 — Change B's `build` task: drop `outputs: ['dist']`

The spec prescribed `outputs: ['dist']` on the `build` task in `.moon/tasks/python.yml`. Verified
during Task 2 implementation that this triggers `task_runner::missing_outputs` for every py-package
build: `uv build` in a uv workspace emits the artifact to the workspace-root `py/dist/`, not to the
per-package `py/packages/<pkg>/dist/`. So Moon's per-project output check fails even though the
build itself succeeded.

**Final shape (what's actually in `.moon/tasks/python.yml`):**

```yaml
  build:
    command: 'uv build'
    inputs: ['@group(sources)', 'pyproject.toml']
```

(No `outputs:` declaration.) The task still works; cache invalidation runs off `inputs:`; downstream
tasks (none today depend on `build`) just don't get a specific output artifact. The original spec
text was lifted from the pre-existing python scaffold template, which had the same latent bug — it
never triggered because no py package had a `build` task until python.yml landed.

### Correction 2 — Change D2's `py/moon.yml` comment: "override" → "merge"

The spec prescribed a comment that described Moon's fileGroups inheritance as "override" semantics.
Verified via `moon project py --json` during Task 3 implementation that Moon actually **merges**
fileGroups across the inherited layer (`.moon/tasks/python.yml`'s `src/**/*` / `tests/**/*`) and the
project-local layer (`py/moon.yml`'s `packages/*/src/**/*` / `packages/*/tests/**/*`). The resolved
`@group(sources)` for `py` contains both `["py/src/**/*", "py/packages/*/src/**/*"]`.

In practice this is harmless (`py/src/` and `py/tests/` don't exist), but a future maintainer adding
`py/src/foo.py` and seeing it picked up by `moon run py:lint` would be confused by an "override"
comment.

**Final shape (what's actually in `py/moon.yml`):**

```yaml
# The inherited fileGroups from .moon/tasks/python.yml assume src/ at the project root.
# The py workspace keeps sources under packages/*/src, so extend the inherited groups
# here. Moon merges (not overrides) fileGroups across the layers, so the resolved
# @group(sources) and @group(tests) contain both python.yml's defaults and these
# additions — fine in practice because py/src/ and py/tests/ don't exist; only
# packages/*/src/** and packages/*/tests/** actually match.
fileGroups:
  sources:
    - 'packages/*/src/**/*'
  tests:
    - 'packages/*/tests/**/*'
```

The fileGroups patterns themselves are unchanged; only the comment was corrected.

## Out of scope / follow-up

- **`.moon/tasks/typescript.yml`** — symmetrically missing for ts; defer to SMA-359 (ts bootstrap),
  which is the right place to land it since the file would have no consumers until then.
- **No ADR.** Per CLAUDE.md, ADRs are for significant choices; this codifies existing rust-task
  pattern for python.

## Verification

The Environment Note at the top of the future plan will document the `proto install` shell setup;
verification commands assume `moon` on `PATH` per that setup.

1. `moon project paigasus-kernel-py` resolves cleanly and reports the inherited `build` / `lint` /
   `fmt` / `typecheck` / `test` tasks (previously had no tasks at all).
2. **Library-archetype render check.** `moon generate python --to py/packages/throwaway --defaults --force -- --name throwaway --archetype library`
   produces a 4-line `moon.yml` (header only, no tasks block); `moon project throwaway-py`
   resolves with inherited tasks and **no** `unknown_file_group` error (the bug SMA-383 surfaced).
   Clean up the throwaway.
3. **Service-archetype render check.** Same command with `--archetype service` produces a
   `moon.yml` whose `tasks:` block contains only the `start` task; `moon project throwaway-py`
   resolves with both inherited tasks and `start`. Clean up.
4. **`py/moon.yml` cleanup check.** `moon project py` resolves with the five inherited tasks
   (`build` / `lint` / `fmt` / `typecheck` / `test`) and reports `Layer: configuration`. Each
   inherited task's resolved `inputs` should reference the project-local `fileGroups` (i.e.,
   `packages/*/src/**/*` and `packages/*/tests/**/*`), not python.yml's defaults.
5. `moon ci :build :test` exits cleanly. Now that py projects have inherited tasks, this may
   report some affected py work (it's a behavioural change for existing packages); that's
   expected and acceptable as long as it exits 0. If any inherited task fails for a py package,
   the PR isn't ready.
6. **CLAUDE.md and CONTRIBUTING.md** read the new corrected text. No other content changed.
7. `grep -rn '\-\-output-style' --include='*.md' . | grep -vE 'docs/superpowers/'` returns no
   matches. Active docs (`CLAUDE.md`, `CONTRIBUTING.md`) are clean; historical plan/spec files
   under `docs/superpowers/` are intentionally not retro-edited and are filtered out.
8. **Notion "Polyglot Monorepo Scoping" § 1** is a one-line redirect to CONTRIBUTING.md (no
   moon.yml examples remain in Notion).

## Acceptance criteria

- [ ] `.moon/tasks/python.yml` exists with `inheritedBy.languages: ['python']`, python
      `fileGroups` (sources + tests), and the five tasks (`build` / `lint` / `fmt` /
      `typecheck` / `test`) with the inputs documented in Change B, including `conftest.py` in
      the test task's inputs.
- [ ] `.moon/tasks.yml`'s `implicitInputs` list contains `/.moon/tasks/python.yml`.
- [ ] `.moon/templates/python/moon.yml` retains only the service-archetype `start` task; no
      library-archetype tasks block.
- [ ] `py/moon.yml` no longer carries a `tasks:` block; the `fileGroups` override remains.
- [ ] `CLAUDE.md:15` replaced with the corrected guidance from Change A.
- [ ] `CONTRIBUTING.md:50-52` replaced with the corrected guidance from Change A.
- [ ] `moon generate python --archetype library` produces a project whose `moon project`
      resolution shows the inherited tasks with no `unknown_file_group` error.
- [ ] `moon generate python --archetype service` produces a project with a `tasks:` block
      containing only the `start` task.
- [ ] `moon project py` resolves with the five inherited tasks; their resolved `inputs` include
      the project-local `fileGroups` patterns (`packages/*/src/**/*`, `packages/*/tests/**/*`).
      Moon merges (not overrides) fileGroups across layers, so python.yml's inherited defaults
      (`src/**/*`, `tests/**/*`) may also appear in the resolved set — that's expected.
- [ ] Notion "Polyglot Monorepo Scoping" § 1 is a one-line redirect to CONTRIBUTING.md (no
      moon.yml examples remain).
- [ ] Three commits on the feature branch, scoped `docs(repo)` / `feat(py)` / `chore(py)`,
      each referencing SMA-384. Branch targets `main` (no longer stacked — SMA-383 has merged).
