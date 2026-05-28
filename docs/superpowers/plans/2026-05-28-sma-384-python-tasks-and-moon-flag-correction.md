# SMA-384 — Wire `.moon/tasks/python.yml`, clean up scaffolding, correct moon flag claim Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `.moon/tasks/python.yml` (mirror of `.moon/tasks/rust.yml`) so python projects inherit standard `build`/`lint`/`fmt`/`typecheck`/`test` tasks; slim the python scaffold template and remove `py/moon.yml`'s now-redundant tasks block; correct the wrong `--output-style stream` claim in `CLAUDE.md` and `CONTRIBUTING.md`; and replace Notion's competing convention copy with a redirect to CONTRIBUTING.md.

**Architecture:** Config + docs change. Tasks resolve by language inheritance; the scaffold template and py-workspace project both stop defining tasks locally and start inheriting them from `.moon/tasks/python.yml`. "Tests" are Moon's own introspection (`moon project <id>` showing inherited tasks; `moon generate python --archetype <a>` rendering valid `moon.yml`; `moon ci :build :test` exiting green). Three conventional commits: `docs(repo)`, `feat(py)`, `chore(py)`. Notion update is a sidecar (out-of-repo) tracked in the spec's AC.

**Tech Stack:** Moon 2.2.5 (`inheritedBy.languages`, task-file inheritance, `outputStyle` per-task config; no CLI flag), proto-pinned toolchain, uv 0.8.x workspace at `py/`, ruff 0.13 / basedpyright 1.31 / pytest 8.

**Spec:** `docs/superpowers/specs/2026-05-28-sma-384-python-tasks-and-moon-flag-correction-design.md`

---

## Environment note

All `moon` commands assume `moon` is on `PATH` per `CONTRIBUTING.md`'s `proto install` setup. If
it isn't, run `proto install` or prefix your shell once with:

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
```

Run every command from the repository root (use `git rev-parse --show-toplevel` to locate it).

Branch already created during brainstorming: `feature/sma-384-wire-python-language-tasks-moontaskspythonyml-correct-moon`. The spec and review-feedback-applied-to-spec commits already exist (commits `c0e8997` and `481ac8b` after the review doc was removed); this plan adds three implementation commits on top.

---

## File Structure

Six files modified across three commits, plus one out-of-repo Notion update. No new files except `.moon/tasks/python.yml`.

| Commit | File | Change |
|--------|------|--------|
| `docs(repo)` (Task 1) | `CLAUDE.md` | Replace the `--output-style stream` bullet at line 15 with the corrected guidance. |
| `docs(repo)` (Task 1) | `CONTRIBUTING.md` | Replace the `--output-style stream` blockquote at lines 50–52 with the corrected guidance. |
| `feat(py)` (Task 2) | `.moon/tasks/python.yml` | **New file.** `inheritedBy.languages: ['python']`, python `fileGroups`, and five tasks (`build`/`lint`/`fmt`/`typecheck`/`test`). |
| `feat(py)` (Task 2) | `.moon/tasks.yml` | Add `/.moon/tasks/python.yml` to `implicitInputs`. |
| `chore(py)` (Task 3) | `.moon/templates/python/moon.yml` | Strip library-archetype tasks block; keep header + service-archetype `start` only. |
| `chore(py)` (Task 3) | `py/moon.yml` | Remove `tasks:` block. Keep `$schema`, `layer`, `language`, and the `fileGroups` override. Update the comment to name the inheritance interaction. |
| `(sidecar — Task 4)` | Notion "Polyglot Monorepo Scoping" § 1 | Replace moon.yml examples + prose with a one-line redirect to CONTRIBUTING.md. No file in this repo; tracked in AC. |

---

## Task 1: Correct the `--output-style stream` claim in CLAUDE.md and CONTRIBUTING.md

**Files:**
- Modify: `CLAUDE.md:15`
- Modify: `CONTRIBUTING.md:50-52`

Pure prose change; no behavior. Replaces a wrong claim (Moon 2.2.5 has no `--output-style` CLI flag on either `moon ci` or `moon run`) with the truth (set `outputStyle` per-task or in `.moon/tasks.yml`'s `taskOptions.outputStyle`).

- [ ] **Step 1: Capture the before-state for both files**

```bash
sed -n '13,17p' CLAUDE.md
echo "---"
sed -n '48,54p' CONTRIBUTING.md
```
Expected: `CLAUDE.md:15` contains the bullet `- Append \`--output-style stream\` to watch a long task (default is \`buffer-only-failure\`).` and `CONTRIBUTING.md:50-52` contains the three-line blockquote about appending `--output-style stream` to `moon run`.

- [ ] **Step 2: Replace the CLAUDE.md bullet**

Apply via `Edit`:

- `old_string`:
  ```
  - Append `--output-style stream` to watch a long task (default is `buffer-only-failure`).
  ```
- `new_string`:
  ```
  - Task output style is set in `.moon/tasks.yml` (`taskOptions.outputStyle`,
    currently `buffer-only-failure`). Moon 2.2.5 has no per-invocation CLI flag
    for it; to stream a specific task locally, set `options.outputStyle: 'stream'`
    on the task definition.
  ```

- [ ] **Step 3: Replace the CONTRIBUTING.md blockquote**

Apply via `Edit`:

- `old_string`:
  ```
  > Output is buffered for passing tasks (`buffer-only-failure`). To watch a long
  > task stream locally, append `--output-style stream`, e.g.
  > `moon run <project>:test --output-style stream`.
  ```
- `new_string`:
  ```
  > Output is buffered for passing tasks (`buffer-only-failure`, set as
  > `taskOptions.outputStyle` in `.moon/tasks.yml`). Moon 2.2.5 has no CLI flag
  > to override this per invocation; to stream a specific task locally, set
  > `options.outputStyle: 'stream'` on the task definition.
  ```

- [ ] **Step 4: Verify the swaps**

```bash
echo "===CLAUDE.md===" && sed -n '13,18p' CLAUDE.md
echo "===CONTRIBUTING.md===" && sed -n '48,55p' CONTRIBUTING.md
```
Expected: each file shows the corrected text in place of the wrong claim.

- [ ] **Step 5: Confirm no remaining `--output-style` claim in active docs**

```bash
grep -rn '\-\-output-style' --include='*.md' . | grep -vE 'docs/superpowers/'
```
Expected: no output. (The grep filter excludes historical plan/spec files under `docs/superpowers/`, which are intentionally not retro-edited.)

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md CONTRIBUTING.md
git commit -m "docs(repo): correct moon --output-style claim in CLAUDE.md and CONTRIBUTING.md (SMA-384)

Moon 2.2.5 has no --output-style CLI flag on either moon ci or moon run;
the flag exists only as a per-task config option (outputStyle), set
globally in .moon/tasks.yml's taskOptions.outputStyle (currently
buffer-only-failure). Both CLAUDE.md (line 15) and CONTRIBUTING.md
(lines 50-52) claimed --output-style stream worked as a CLI override
since SMA-356; correct both to describe where outputStyle is actually
configured.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Add `.moon/tasks/python.yml` and register it in implicit inputs

**Files:**
- Create: `.moon/tasks/python.yml`
- Modify: `.moon/tasks.yml`

This is the behavior change for already-existing py packages — they start inheriting `build`/`lint`/`fmt`/`typecheck`/`test` tasks after this commit. The new file mirrors `.moon/tasks/rust.yml`'s shape. Task name is `fmt` (not `format`) to harmonize with rust.yml so `moon ci :fmt` covers both stacks.

- [ ] **Step 1: Capture the "failing" before-state**

```bash
moon project paigasus-kernel-py 2>&1 | grep -iE 'tasks|inheritance|fmt|format|lint' | head -10
```
Expected: no `Tasks` section appears (the existing `py/packages/paigasus-kernel/moon.yml` has no `tasks:` block and nothing inherits because there's no `.moon/tasks/python.yml`). Equivalent: `moon project paigasus-kernel-py | grep -A 5 '^Tasks'` shows no task entries.

- [ ] **Step 2: Create `.moon/tasks/python.yml` — final file contents**

Write the file with this exact content (no SPDX header — `.yml` is config, per CONTRIBUTING.md):

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

- [ ] **Step 3: Register `python.yml` in `.moon/tasks.yml`'s `implicitInputs`**

Apply via `Edit` to `.moon/tasks.yml`:

- `old_string`:
  ```
  implicitInputs:
    - '/.moon/toolchain.yml'
    - '/.moon/tasks.yml'
    - '/.moon/tasks/rust.yml'
  ```
- `new_string`:
  ```
  implicitInputs:
    - '/.moon/toolchain.yml'
    - '/.moon/tasks.yml'
    - '/.moon/tasks/rust.yml'
    - '/.moon/tasks/python.yml'
  ```

- [ ] **Step 4: Verify the new file parses and is inherited by py packages**

```bash
moon project paigasus-kernel-py 2>&1 | grep -A 8 '^Tasks'
```
Expected: shows five tasks — `build`, `lint`, `fmt`, `typecheck`, `test` — with their inherited commands. No `config::parse::failed` error.

- [ ] **Step 5: Sanity-check rust projects still inherit only rust tasks**

```bash
moon project paigasus-kernel-rs 2>&1 | grep -A 6 '^Tasks'
```
Expected: shows the four rust tasks (`build`, `test`, `lint`, `fmt`) — and **no** `typecheck` or `uv` references. The `inheritedBy.languages: ['python']` scope prevents the new file from leaking into rust.

- [ ] **Step 6: Verify the `.moon/tasks.yml` change**

```bash
sed -n '16,21p' .moon/tasks.yml
```
Expected:
```
implicitInputs:
  - '/.moon/toolchain.yml'
  - '/.moon/tasks.yml'
  - '/.moon/tasks/rust.yml'
  - '/.moon/tasks/python.yml'
```

- [ ] **Step 7: Run the affected build/test graph to confirm nothing regresses**

```bash
moon ci :build :test
```
Expected: exits 0. May report some py work as newly affected (the implicitInputs change touches every project's task hashes); that's expected.

- [ ] **Step 8: Commit**

```bash
git add .moon/tasks/python.yml .moon/tasks.yml
git commit -m "feat(py): add .moon/tasks/python.yml + register implicit input (SMA-384)

Mirror .moon/tasks/rust.yml for python: inheritedBy.languages: ['python'],
python fileGroups (sources + pytest-style tests), and the five standard
tasks (build / lint / fmt / typecheck / test). Task name fmt (not format)
matches rust.yml's convention so moon ci :fmt covers both stacks
in one invocation.

After this commit, py packages (paigasus-{kernel,ml,proto,workflows})
start inheriting standard tasks they previously had to redefine
per-project. The scaffold template and py/moon.yml will be slimmed
in the follow-up chore(py) commit.

Register /.moon/tasks/python.yml in .moon/tasks.yml's implicitInputs
so a change to it busts python task caches the same way rust.yml does.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Slim the python scaffold template and clean up `py/moon.yml`

**Files:**
- Modify: `.moon/templates/python/moon.yml`
- Modify: `py/moon.yml`

Both files currently define library-archetype tasks that are now inherited from `python.yml`. Strip them. The template keeps only the service-archetype `start` override; `py/moon.yml` keeps its `fileGroups` override (which re-scopes `sources` to `packages/*/src/**/*` for the inherited tasks).

- [ ] **Step 1: Capture the before-state of the template**

```bash
cat .moon/templates/python/moon.yml
```
Expected: the full template with `$schema`, `id`, `layer`, `language`, then a `tasks:` block containing `build`/`lint`/`format`/`typecheck`/`test` plus the service-archetype-conditional `start` task.

- [ ] **Step 2: Slim the python scaffold template — final file contents**

Write the file with this exact content (overwrites the existing 27-line template):

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

After this, library/binding renders produce a 4-line `moon.yml` (header only); service renders add a `tasks:` block with just `start`. Matches the rust template's exact shape.

- [ ] **Step 3: Capture the before-state of py/moon.yml**

```bash
cat py/moon.yml
```
Expected: `$schema`, `layer: 'configuration'`, `language: 'python'`, a comment about the fileGroups override, `fileGroups: { sources, tests }`, and a `tasks:` block with `lint`/`format`/`typecheck`/`test`.

- [ ] **Step 4: Remove `py/moon.yml`'s tasks block — final file contents**

Write the file with this exact content (overwrites the existing file; preserves header + fileGroups override, drops the entire `tasks:` block, updates the comment to name the inheritance interaction):

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

- [ ] **Step 5: Render the python template (library archetype) — the "test passes"**

```bash
moon generate python --to py/packages/throwaway --defaults --force -- --name throwaway --archetype library
cat py/packages/throwaway/moon.yml
moon project throwaway-py 2>&1 | grep -iE '^Layer|^Tasks|Error|unknown_file_group' | head -10
rm -rf py/packages/throwaway
```
Expected:
- `cat` shows a 4-line `moon.yml`: `$schema`, `id: 'throwaway-py'`, `layer: 'library'`, `language: 'python'`. **No `tasks:` block.**
- `moon project throwaway-py` reports `Layer: library` and a `Tasks` section listing the five inherited tasks. **No** `unknown_file_group` error (the SMA-383-surfaced bug is now fixed).

- [ ] **Step 6: Render the python template (service archetype)**

```bash
moon generate python --to py/packages/throwaway --defaults --force -- --name throwaway --archetype service
cat py/packages/throwaway/moon.yml
moon project throwaway-py 2>&1 | grep -iE '^Layer|^Tasks|start|Error' | head -10
rm -rf py/packages/throwaway
```
Expected:
- `cat` shows the 4-line header plus a `tasks:` block containing only `start` (with `cache: false` and `persistent: true`).
- `moon project throwaway-py` reports `Layer: application`, and the Tasks section lists six tasks (the five inherited + `start`).

- [ ] **Step 7: Verify py-workspace's inherited tasks resolve against its fileGroups override**

```bash
moon project py 2>&1 | grep -iE '^Layer|^Tasks|^Inheritance|sources|tests' | head -15
```
Expected:
- `Layer: configuration`.
- Tasks section lists the five inherited tasks (`build`, `lint`, `fmt`, `typecheck`, `test`).
- The resolved `inputs` for each task **include** `packages/*/src/**/*` and `packages/*/tests/**/*` (py-workspace's fileGroups override). Moon merges (not overrides) fileGroups across the inherited and project-local layers, so the python.yml defaults (`src/**/*`, `tests/**/*`) may also appear in the resolved set — that's expected and harmless because `py/src/` and `py/tests/` don't exist in the workspace.

If `moon project py` doesn't print resolved inputs verbosely, fall back to:
```bash
moon project py --json 2>&1 | jq '.tasks[] | {target, inputs}' | head -40
```
and confirm each task's `inputs` includes the `packages/*/` patterns (inherited defaults may also appear).

- [ ] **Step 8: Confirm no throwaway artifacts remain**

```bash
git status --short
```
Expected: exactly two modified files — `.moon/templates/python/moon.yml` and `py/moon.yml`. No `py/packages/throwaway/` directory.

- [ ] **Step 9: Run `moon ci :build :test` once more to confirm no regression**

```bash
moon ci :build :test
```
Expected: exits 0. Any py work that runs (e.g., inherited `lint`/`fmt`/`typecheck`/`test` on the now-task-bearing py packages) should pass — these packages have empty src/ but the tasks are tolerant of empty input sets.

- [ ] **Step 10: Commit**

```bash
git add .moon/templates/python/moon.yml py/moon.yml
git commit -m "chore(py): slim python scaffold template and remove py/moon.yml tasks block (SMA-384)

After .moon/tasks/python.yml lands (previous commit), both the python
scaffold template and py/moon.yml define tasks that are now structurally
redundant — same names, same commands, just shadowing the inherited
file.

Template: strip the library-archetype tasks block. Keep only the
service-archetype start override (which needs per-project Tera variables
for the module path, so it can't live in python.yml). Matches the rust
template's exact shape.

py/moon.yml: remove the tasks block entirely. The fileGroups override
stays — it re-scopes \`@group(sources)\` and \`@group(tests)\` to
packages/*/src/**/* and packages/*/tests/**/* for the inherited tasks,
so workspace-wide invocations still target the package tree. Comment
updated to name the inheritance interaction.

Behavior parity check: moon run py:{lint,fmt,typecheck,test} continue
to invoke the same commands. The Moon task name format becomes fmt
for cross-language consistency with rust.yml's fmt.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Notion "Polyglot Monorepo Scoping" § 1 — replace with redirect (sidecar, no commit)

No file change in this repo. Done before opening the PR so the durable single-source-of-truth state holds from the moment the PR opens.

- [ ] **Step 1: Locate the Notion doc**

Use the Notion MCP `notion-search` or `notion-fetch` tool. The doc URL from `CLAUDE.md` is `https://www.notion.so/368830e8fbaa8101b0ffded7a3de3b53`. Fetch it.

- [ ] **Step 2: Identify § 1's moon.yml content**

Read the doc; locate the section titled "moon.yml conventions" (or the equivalent within § 1). Verify it currently contains pre-SMA-381 examples (`type:` instead of `layer:`, no `-rs`/`-py`/`-ts` suffixes, no field order). Capture the block (heading + content) so the replacement is scoped correctly.

- [ ] **Step 3: Propose the redirect content for user approval**

Present the proposed replacement block:

```markdown
## moon.yml conventions

These conventions are documented in `CONTRIBUTING.md` in the
[paigasus-core](https://github.com/SMK1085/paigasus-core/blob/main/CONTRIBUTING.md)
repository — the source of truth. See *§ Code conventions → Moon project files* for the
`moon.yml` field order and `layer:` values, and *§ Code conventions* for the SPDX guidance.
```

Wait for explicit user approval before applying. Do **not** apply silently.

- [ ] **Step 4: Apply the redirect via Notion MCP**

Use `notion-update-page` (or the appropriate tool) to replace the § 1 content with the approved redirect. Apply to the exact block identified in Step 2.

- [ ] **Step 5: Verify**

Re-fetch the Notion doc. Confirm:
- The redirect block is present, with the wording from Step 3.
- No moon.yml examples remain in § 1.
- The link to CONTRIBUTING.md resolves to the live `main` URL.

- [ ] **Step 6: Acceptance criterion tick**

Update the PR description to note "Notion § 1 redirected to CONTRIBUTING.md (durable single-source-of-truth fix)" with a link to the Notion page. No git commit; the AC is tracked in the spec.

---

## Task 5: Whole-PR verification

No file changes — asserts the spec's acceptance criteria. (No commit.)

- [ ] **Step 1: Confirm exactly the intended files changed across the three Task commits**

```bash
git diff --stat HEAD~3 HEAD -- CLAUDE.md CONTRIBUTING.md '.moon/tasks.yml' '.moon/tasks/python.yml' '.moon/templates/python/moon.yml' 'py/moon.yml'
```
Expected: six files changed — `CLAUDE.md`, `CONTRIBUTING.md`, `.moon/tasks.yml`, `.moon/tasks/python.yml`, `.moon/templates/python/moon.yml`, `py/moon.yml`. Nothing else.

- [ ] **Step 2: Confirm no `--output-style` claim leaks into active docs**

```bash
grep -rn '\-\-output-style' --include='*.md' . | grep -vE 'docs/superpowers/'
```
Expected: no matches.

- [ ] **Step 3: Confirm no `type:` regression in moon project files**

```bash
grep -rn '^type:' --include=moon.yml . ; echo "exit: $?"
```
Expected: no matches, `exit: 1`. Re-asserts the SMA-381 invariant.

- [ ] **Step 4: Confirm all three scaffold templates emit `$schema → id → layer → language` order**

```bash
for t in rust python typescript; do
  echo "=== .moon/templates/$t/moon.yml ==="
  sed -n '1,5p' ".moon/templates/$t/moon.yml"
done
```
Expected: each template's first four non-blank lines are `$schema`, `id` (with the right suffix: `-rs`/`-py`/`-ts`), `layer`, `language`. The python template's library-archetype tasks block is gone; only the service-archetype `start` remains (visible by reading further).

- [ ] **Step 5: Confirm every python project resolves with inherited tasks**

```bash
for p in paigasus-kernel-py paigasus-ml-py paigasus-proto-py paigasus-workflows-py py; do
  echo "=== $p ==="
  moon project "$p" 2>&1 | grep -A 6 '^Tasks' | head -8
done
```
Expected: each project shows the five inherited tasks (`build`, `lint`, `fmt`, `typecheck`, `test`). `py` (workspace root) shows the same set, resolved against its `packages/*/` fileGroups override.

- [ ] **Step 6: Run the affected build/test graph**

```bash
moon ci :build :test
```
Expected: exits 0. Any inherited py task that runs should pass.

- [ ] **Step 7: Confirm the branch is ready to push**

```bash
git log --oneline main..HEAD
```
Expected (newest first):
```
<sha> chore(py): slim python scaffold template and remove py/moon.yml tasks block (SMA-384)
<sha> feat(py): add .moon/tasks/python.yml + register implicit input (SMA-384)
<sha> docs(repo): correct moon --output-style claim in CLAUDE.md and CONTRIBUTING.md (SMA-384)
481ac8b docs(repo): remove SMA-384 design review doc and clean spec refs
6200439 docs(repo): apply review feedback to SMA-384 spec
c0e8997 docs(repo): spec python tasks wiring + moon flag correction (SMA-384)
```

- [ ] **Step 8: Confirm Task 4 (Notion redirect) is complete**

Open the Notion "Polyglot Monorepo Scoping" doc and visually confirm § 1 shows the redirect block (no moon.yml examples). If this isn't done, the PR isn't ready to open.

---

## Self-review notes

- **Spec coverage:**
  - Change A (CLAUDE.md + CONTRIBUTING.md `--output-style` correction) → Task 1.
  - Change B (`.moon/tasks/python.yml` new file) → Task 2 Step 2.
  - Change C (`.moon/tasks.yml` `implicitInputs` registration) → Task 2 Step 3.
  - Change D (python scaffold template slim-down) → Task 3 Steps 1–2, 5–6.
  - Change D2 (`py/moon.yml` tasks-block removal) → Task 3 Steps 3–4, 7.
  - Change E (Notion redirect) → Task 4.
  - Three implementation commits with the documented scopes → Task 1 Step 6 (`docs(repo)`), Task 2 Step 8 (`feat(py)`), Task 3 Step 10 (`chore(py)`).
  - `moon project py` shows inherited tasks with project-local fileGroups → Task 3 Step 7, Task 5 Step 5.
  - `moon generate python --archetype library/service` round-trip → Task 3 Steps 5–6.
- **Placeholder scan:** every Edit step shows the exact `old_string` / `new_string` content; every Write step shows the full final file contents; every verification step shows the exact command and expected output. No "implement later" / "add appropriate handling" patterns.
- **Type / field-name consistency:** task name `fmt` everywhere (never `format`) in python.yml and verification commands; `layer:` everywhere (never `type:`); id suffixes match SMA-380 (`-rs`/`-py`/`-ts`); `inheritedBy.languages: ['python']` (mirrors rust.yml's `['rust']`).
- **Out of scope (per spec):** `.moon/tasks/typescript.yml` — defer to SMA-359.
