# SMA-384 — Wire `.moon/tasks/python.yml`, slim the python scaffold template, and correct the bogus `--output-style stream` claim

**Status:** Designed (brainstorming complete)
**Date:** 2026-05-28
**Linear:** [SMA-384](https://linear.app/smaschek/issue/SMA-384/wire-python-language-tasks-moontaskspythonyml-correct-moon-output)
**Branch:** `feature/sma-384-wire-python-language-tasks-moontaskspythonyml-correct-moon`
**Stacked on:** SMA-383 (PR #7). The python template slim-down (Change C) depends on SMA-383's `id: '{{ name }}-py'` line in `.moon/templates/python/moon.yml`. PR will target the SMA-383 branch and retarget to `main` automatically once SMA-383 merges.
**References:** SMA-381 (introduced `inheritedBy.languages: ['rust']` pattern in `.moon/tasks/rust.yml`); SMA-380 (`-py`/`-ts` id suffix); SMA-358 (py uv workspace bootstrap, including the existing scaffold template); SMA-356 (originator of the wrong `--output-style stream` claim in CONTRIBUTING.md).

## Goal

Close out the last scaffolding-consistency gap and correct two pieces of wrong documentation:

1. **Wire python language tasks.** Add `.moon/tasks/python.yml` so python projects inherit standard
   `build`/`lint`/`format`/`typecheck`/`test` tasks the same way rust crates inherit from
   `.moon/tasks/rust.yml`. After this, `moon generate python` produces projects that resolve cleanly
   (no `unknown_file_group` validation error), and the python and rust scaffold templates share the
   same minimal shape.
2. **Slim the python scaffold template** to match the rust template's shape — header (`$schema` →
   `id` → `layer` → `language`) plus a service-archetype-only override, no library-archetype tasks
   block.
3. **Correct the bogus `--output-style stream` claim** in both `CLAUDE.md` and `CONTRIBUTING.md`.
   Moon 2.2.5 has no such CLI flag on `moon ci` or `moon run`; `outputStyle` is a per-task config
   option (set globally in `.moon/tasks.yml`'s `taskOptions.outputStyle`, currently
   `buffer-only-failure`).
4. **Update Notion "Polyglot Monorepo Scoping" § 1** to mirror current CONTRIBUTING.md conventions
   (`layer:` not `type:`, `-rs`/`-py`/`-ts` id suffix, documented `moon.yml` field order). Sidecar
   — no file change in this repo; tracked here so it doesn't drift again.

After this lands, no scaffolding-consistency follow-ups remain across `rs` + `py` + `ts-template` +
`contracts-template`, and the in-repo docs that describe Moon CLI behaviour describe it correctly.

## Decision

Three commits on the SMA-384 branch:

- `docs(repo):` — `CLAUDE.md:15` and `CONTRIBUTING.md:50-52` corrections (both carried the same
  bogus `--output-style stream` claim). The fix replaces the misleading text with a description of
  where output style is actually configured.
- `feat(py):` — adds `.moon/tasks/python.yml` and registers it in `.moon/tasks.yml`'s
  `implicitInputs`. This is the language-task wiring; py packages start inheriting standard tasks
  after this commit lands.
- `chore(py):` — slims `.moon/templates/python/moon.yml`'s tasks block (library-archetype tasks are
  now inherited from python.yml; only the service-archetype `start` task stays in the template).

Notion update is performed *before* the PR opens (per the brainstorming timing decision), but
tracked in this spec's acceptance criteria so it doesn't drift again.

The CLAUDE.md "other improvements" audit (per your `Fix CLAUDE.md, also check for other
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

**CLAUDE.md "other improvements" audit, conducted during brainstorming:**

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
and the standard tasks. Commands and inputs are lifted directly from the current python scaffold
template (which Change C strips):

```yaml
$schema: 'https://moonrepo.dev/schemas/tasks.json'

# Moon does NOT scope task files by filename — scope explicitly so these python
# commands only attach to python projects (not rust, and not contracts/ts).
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
  format:
    command: 'uv run ruff format --check .'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock']
  typecheck:
    command: 'uv run basedpyright'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock']
  test:
    command: 'uv run pytest'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock']
```

Notes:

- **`/py/uv.lock`** uses Moon's workspace-anchor syntax — every python package shares the single
  `py/uv.lock` lockfile (uv workspace), not per-package locks. Same input shape as the current
  template.
- **Test discovery patterns** (`**/*_test.py`, `**/test_*.py`) cover both `_test.py` suffix and
  `test_*.py` prefix conventions; ruff/pytest pick up both.
- **No `start` task here** — service archetypes get a `start` task from the template (Change C),
  because the command needs the project's `name` to construct the module path.

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

### Change E — Notion "Polyglot Monorepo Scoping" § 1 update (out-of-repo, before PR opens)

Done before opening the PR (per brainstorming timing choice). I'll:

1. Fetch the current Notion doc via the Notion MCP.
2. Identify § 1's `moon.yml` examples and prose.
3. Propose the diff to you for approval (specifically: replace `type:` with `layer:`; show the
   `-rs`/`-py`/`-ts` id suffix; reference the documented `moon.yml` field order; remove any
   pre-SMA-381 framing).
4. Apply the approved update via the Notion MCP.
5. Tick this off in the acceptance criteria.

Tracked here so the doc drift doesn't recur. No file lands in this repo from Change E.

## Commit grouping

```
docs(repo): correct moon --output-style claim in CLAUDE.md and CONTRIBUTING.md (SMA-384)
feat(py):   add .moon/tasks/python.yml + register implicit input (SMA-384)
chore(py):  slim python scaffold template to match rust pattern (SMA-384)
```

The split puts behavior change (`feat(py)`, which makes existing py packages start inheriting
tasks) in its own commit, separate from the template change (`chore(py)`, which only affects
future-generated projects). The `docs(repo)` commit is pure prose.

## Out of scope / follow-up

- **`py/moon.yml` task-block cleanup.** After python.yml lands, `py/moon.yml`'s own `tasks:` block
  becomes structurally redundant — it defines the same task names and commands as python.yml. It
  still functions because its project-local `fileGroups` override re-scopes `sources` to
  `packages/*/src/**/*`. Removing the redundant tasks block would mean confirming that the
  inherited tasks correctly resolve against the project-level `fileGroups` override. Worth doing as
  a cleanup but scope-creeps this PR; defer to a follow-up.
- **`.moon/tasks/typescript.yml`.** Symmetrically missing for ts; defer to SMA-359 (ts bootstrap),
  which is the right place to land it since the file would have no consumers until then.
- **No ADR.** Per CLAUDE.md, ADRs are for significant choices; this codifies existing rust-task
  pattern for python.

## Verification

1. `~/.proto/shims/moon project paigasus-kernel-py` resolves cleanly and reports the inherited
   `build` / `lint` / `format` / `typecheck` / `test` tasks (previously had no tasks at all).
2. **Library-archetype render check.** `~/.proto/shims/moon generate python --to py/packages/throwaway --defaults --force -- --name throwaway --archetype library`
   produces a 4-line `moon.yml` (header only, no tasks block); `moon project throwaway-py`
   resolves with inherited tasks and **no** `unknown_file_group` error (the bug SMA-383 surfaced).
   Clean up the throwaway.
3. **Service-archetype render check.** Same command with `--archetype service` produces a
   `moon.yml` whose `tasks:` block contains only the `start` task; `moon project throwaway-py`
   resolves with both inherited tasks and `start`. Clean up.
4. `~/.proto/shims/moon ci :build :test` exits cleanly. Now that py projects have inherited tasks,
   this may report some affected py work (it's a behavioural change for existing packages); that's
   expected and acceptable as long as it exits 0. If any inherited task fails for a py package, the
   PR isn't ready.
5. **CLAUDE.md and CONTRIBUTING.md** read the new corrected text. No other content changed.
6. `grep -rn '\-\-output-style' --include='*.md' .` returns no matches in active docs (CLAUDE.md,
   CONTRIBUTING.md); historical plan/spec files under `docs/superpowers/` still contain it (those
   are historical records of past state and are intentionally not retro-edited).
7. **Notion "Polyglot Monorepo Scoping" § 1** examples use `layer:`/`-py`/`-ts` and reference
   CONTRIBUTING.md for the canonical field order.

## Acceptance criteria

- [ ] `.moon/tasks/python.yml` exists with `inheritedBy.languages: ['python']`, python
      `fileGroups` (sources + tests), and the five tasks (`build` / `lint` / `format` /
      `typecheck` / `test`) with the inputs documented in Change B.
- [ ] `.moon/tasks.yml`'s `implicitInputs` list contains `/.moon/tasks/python.yml`.
- [ ] `.moon/templates/python/moon.yml` retains only the service-archetype `start` task; no
      library-archetype tasks block.
- [ ] `CLAUDE.md:15` replaced with the corrected guidance from Change A.
- [ ] `CONTRIBUTING.md:50-52` replaced with the corrected guidance from Change A.
- [ ] `moon generate python --archetype library` produces a project whose `moon project`
      resolution shows the inherited tasks with no `unknown_file_group` error.
- [ ] `moon generate python --archetype service` produces a project with a `tasks:` block
      containing only the `start` task.
- [ ] Notion "Polyglot Monorepo Scoping" § 1 updated to mirror current CONTRIBUTING.md
      conventions (`layer:`, `-rs`/`-py`/`-ts` id suffix, documented `moon.yml` field order).
- [ ] Three commits on the feature branch, scoped `docs(repo)` / `feat(py)` / `chore(py)`,
      each referencing SMA-384.
- [ ] PR is stacked on the SMA-383 branch (base = `feature/sma-383-...`) and will retarget to
      `main` when SMA-383 merges.
