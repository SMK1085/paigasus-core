# SMA-383 — Document `moon.yml` conventions + align scaffold templates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Codify the `moon.yml` field-order convention and SPDX config-file carve-out in `CONTRIBUTING.md`, and add the missing `-py` / `-ts` id-suffix line to the python and typescript scaffold templates so they emit projects that already satisfy SMA-380's id-suffix convention.

**Architecture:** Pure docs + config change — one Markdown file edited under `## Code conventions`, two scaffold templates each gain one `id:` line. No source code. "Tests" are: visual diff of the rendered CONTRIBUTING.md, Moon's own introspection on a throwaway generated project (`moon generate` → `grep id:` → `moon project` parses without error), plus `moon ci :build :test` staying green. Lands as two conventional commits: `docs(repo)` and `fix(repo)`.

**Tech Stack:** Moon 2.2.5 (scaffold templating; field is `layer:`, not the pre-2.x `type:`), proto-pinned toolchain.

**Spec:** `docs/superpowers/specs/2026-05-28-sma-383-contributing-moon-conventions-design.md`

---

## Environment note

All `moon` commands assume `moon` is on `PATH`. If it isn't, run `proto install` (per CONTRIBUTING.md)
or prefix your shell once with:

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
```

Run every command from the repo root: `/Users/smaschek/dev/paigasus/paigasus-core`.

Work happens on the branch already created during brainstorming:
`feature/sma-383-document-moonyml-field-order-config-file-spdx-carve-out-in`. The spec doc and
review doc are already committed (commits `b9d6131`, `9a0e73c`). This plan adds two more commits
on top of those.

---

## File Structure

Three files modified across two commits. No new files.

| Commit | File | Change |
|--------|------|--------|
| `docs(repo)` | `CONTRIBUTING.md` | Replace SPDX bullet (1 → 4 bullets) inside `## Code conventions`; append new `### Moon project files` subsection after the per-language formatting bullet. |
| `fix(repo)` | `.moon/templates/python/moon.yml` | Insert `id: '{{ name }}-py'` between `$schema` and `layer`. |
| `fix(repo)` | `.moon/templates/typescript/moon.yml` | Insert `id: '{{ name }}-ts'` between `$schema` and `layer`. |

---

## Task 1: Update CONTRIBUTING.md (`## Code conventions` section)

**Files:**
- Modify: `CONTRIBUTING.md:68-75` (the `## Code conventions` section as it stands today)

The current section is two bullets — the SPDX bullet (line 70–73) and the per-language formatting
bullet (line 74–75). Task 1 replaces the SPDX bullet with four bullets and appends a new `### Moon
project files` subsection after the per-language bullet.

- [ ] **Step 1: Capture the before-state**

```bash
sed -n '68,76p' CONTRIBUTING.md
```
Expected (two bullets, single SPDX bullet listing comment syntax):
```
## Code conventions

- Every source file starts with an SPDX license header, using the language's
  comment syntax:
  - Rust / TypeScript / Protobuf: `// SPDX-License-Identifier: Apache-2.0`
  - Python: `# SPDX-License-Identifier: Apache-2.0`
- Per-language formatting and linting are enforced by each workspace's Moon
  tasks; run the workspace's `lint`/`fmt` tasks before pushing once it's set up.
```

- [ ] **Step 2: Replace the SPDX bullet with the four-bullet form (Change A)**

Replace the single SPDX bullet (lines 70–73 in the before-state) with these four bullets. The
per-language formatting bullet immediately after is **unchanged**.

Final content for the SPDX portion of `## Code conventions`:

```markdown
- Every source file starts with an SPDX license header, using the language's
  comment syntax:
  - Rust / TypeScript / Protobuf: `// SPDX-License-Identifier: Apache-2.0`
  - Python: `# SPDX-License-Identifier: Apache-2.0`
- Hand-written config carries no SPDX header. Examples in this repo:
  `moon.yml`, `*.toml`, `*.yaml` / `*.yml`, `*.json`, and dotfiles like
  `.gitignore` / `.editorconfig`. If you're unsure for a new file type, ask in
  the PR — it's almost always config.
- Generated files (lockfiles such as `Cargo.lock` / `uv.lock` /
  `pnpm-lock.yaml`, plus codegen output) carry whatever header the generator
  emits. Don't hand-edit a generated file's header.
- Markdown docs (`README.md`, `CONTRIBUTING.md`, ADRs, design specs) and the
  `LICENSE` file itself carry no SPDX header.
```

Apply with `Edit` (single `old_string` / `new_string` swap). The `old_string` is the four lines of
the current SPDX bullet:

```
- Every source file starts with an SPDX license header, using the language's
  comment syntax:
  - Rust / TypeScript / Protobuf: `// SPDX-License-Identifier: Apache-2.0`
  - Python: `# SPDX-License-Identifier: Apache-2.0`
```

The `new_string` is the four-bullet block above (with the existing leading `- Every source file…`
preserved verbatim as the first bullet, then the three new exemption bullets).

- [ ] **Step 3: Verify the SPDX swap**

```bash
sed -n '68,86p' CONTRIBUTING.md
```
Expected: the four bullets above, followed by the existing per-language formatting bullet
(`- Per-language formatting and linting are enforced...`).

- [ ] **Step 4: Append the new `### Moon project files` subsection (Change B)**

Add the new subsection immediately after the per-language formatting bullet, before
`## Contributor License Agreement`. Use `Edit` with `old_string` being the per-language bullet plus
the blank line + `## Contributor License Agreement` heading that currently follows it; `new_string`
keeps both anchors and inserts the new subsection between them.

`old_string`:

```
- Per-language formatting and linting are enforced by each workspace's Moon
  tasks; run the workspace's `lint`/`fmt` tasks before pushing once it's set up.

## Contributor License Agreement
```

`new_string`:

```
- Per-language formatting and linting are enforced by each workspace's Moon
  tasks; run the workspace's `lint`/`fmt` tasks before pushing once it's set up.

### Moon project files

Hand-written `moon.yml` files use a fixed top-level field order so diffs
across workspaces stay readable and so generated/scaffolded files line up
with hand-written ones:

1. `$schema`
2. `id` (when present)
3. `layer`
4. `language`
5. `dependsOn`
6. `fileGroups`
7. `tasks`
8. `options`
9. Any remaining fields (alphabetical)

Use `layer:`, not the pre-2.x `type:` — Moon 2.2.5's parser rejects `type:`.
The values in active use are `library` (importable code, e.g. the rust
crates in `rs/crates/libs/` and the py packages in `py/packages/`),
`application` (runnable binary, e.g. `paigasus-gateway-rs`), and
`configuration` (workspace-root project that aggregates child projects,
e.g. `py/moon.yml`). Moon's full set of seven values is documented in its
[project config docs](https://moonrepo.dev/docs/config/project) — pick
`library` if unsure.

The three scaffold templates under `.moon/templates/{rust,python,typescript}/`
emit this same order, so `moon generate` output is consistent with
hand-written projects (SMA-381).

## Contributor License Agreement
```

- [ ] **Step 5: Verify the subsection landed in the right place**

```bash
sed -n '68,110p' CONTRIBUTING.md
```
Expected: `## Code conventions` heading, four SPDX bullets, per-language formatting bullet, blank
line, `### Moon project files` heading, the 9-position list, the `layer:` paragraph, the templates
paragraph, blank line, `## Contributor License Agreement` heading.

- [ ] **Step 6: Sanity-check the SPDX examples against repo reality**

The new SPDX bullets name `moon.yml`, `*.toml`, `*.yaml`, `*.json`, lockfiles, dotfiles, and
markdown docs as exempt. Confirm none of those carry an SPDX header today:

```bash
git ls-files \
  '*.yml' '*.yaml' '*.toml' '*.json' \
  'Cargo.lock' 'uv.lock' 'pnpm-lock.yaml' \
  '.gitignore' '.editorconfig' \
  '*.md' \
  | xargs grep -l 'SPDX-License-Identifier' 2>/dev/null ; echo "exit: $?"
```
Expected: no output, `exit: 1` (no matches). If anything prints, that file would contradict the
documented exemption and needs surfacing before continuing.

- [ ] **Step 7: Sanity-check the field-order matches the 8 hand-written moon.yml files**

```bash
for f in \
  rs/crates/libs/paigasus-kernel/moon.yml \
  rs/crates/bindings/paigasus-py-bindings/moon.yml \
  rs/crates/services/paigasus-gateway/moon.yml \
  py/moon.yml \
  py/packages/paigasus-kernel/moon.yml \
  py/packages/paigasus-ml/moon.yml \
  py/packages/paigasus-proto/moon.yml \
  py/packages/paigasus-workflows/moon.yml ; do
  echo "=== $f ==="
  grep -E '^(\$schema|id|layer|language|dependsOn|fileGroups|tasks|options):' "$f"
done
```
Expected: every file shows fields in the documented order — `$schema` first, then optionally `id`,
then `layer`, then `language`, then any of `dependsOn` / `fileGroups` / `tasks` in that order
where present. (`py/moon.yml` omits `id` and shows `$schema`, `layer`, `language`, `fileGroups`,
`tasks` — still in order.)

- [ ] **Step 8: Commit (Task 1's "tests pass")**

```bash
git add CONTRIBUTING.md
git commit -m "docs(repo): document moon.yml field order and SPDX config carve-out (SMA-383)

Codify two conventions in CONTRIBUTING.md so they carry forward to the
contracts/ (SMA-360) and ts/ (SMA-359) workspaces and don't get re-litigated
on every config-file PR:

- moon.yml field order: \$schema -> id (when present) -> layer -> language
  -> dependsOn -> fileGroups -> tasks -> options -> any remaining
  (alphabetical). Names the three layer: values in active use (library,
  application, configuration) and links to Moon's docs for the full set.
  Reminds readers that the field is layer:, not the pre-2.x type:.
- SPDX guidance split into rule + three exemption rules (hand-written
  config, generated files, markdown docs). Examples are non-exhaustive so
  new file types read the rule instead of re-litigating the list.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Align python and typescript scaffold templates with the id-suffix convention

**Files:**
- Modify: `.moon/templates/python/moon.yml`
- Modify: `.moon/templates/typescript/moon.yml`

Both templates currently omit an `id:` line, so `moon generate python|typescript` produces a
`moon.yml` without the SMA-380 stack suffix unless the contributor remembers to add one. Adding
the `id:` line mirrors the rust template, which already emits `id: '{{ name }}-rs'`.

- [ ] **Step 1: Capture the before-state**

```bash
sed -n '1,5p' .moon/templates/python/moon.yml
echo "---"
sed -n '1,5p' .moon/templates/typescript/moon.yml
```
Expected (no `id:` line in either):
```
$schema: 'https://moonrepo.dev/schemas/project.json'
layer: '{% if archetype == "service" %}application{% else %}library{% endif %}'
language: 'python'
tasks:
  build:
---
$schema: 'https://moonrepo.dev/schemas/project.json'
layer: '{% if archetype == "app" %}application{% else %}library{% endif %}'
language: 'typescript'
tasks:
  build:
```

- [ ] **Step 2: python template — insert `id:` line**

In `.moon/templates/python/moon.yml`, between the `$schema:` line and the `layer:` line, insert
`id: '{{ name }}-py'`. After the edit, the first three lines must read:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: '{{ name }}-py'
layer: '{% if archetype == "service" %}application{% else %}library{% endif %}'
```

Apply via `Edit`:

- `old_string`:
  ```
  $schema: 'https://moonrepo.dev/schemas/project.json'
  layer: '{% if archetype == "service" %}application{% else %}library{% endif %}'
  ```
- `new_string`:
  ```
  $schema: 'https://moonrepo.dev/schemas/project.json'
  id: '{{ name }}-py'
  layer: '{% if archetype == "service" %}application{% else %}library{% endif %}'
  ```

- [ ] **Step 3: typescript template — insert `id:` line**

In `.moon/templates/typescript/moon.yml`, between the `$schema:` line and the `layer:` line,
insert `id: '{{ name }}-ts'`. After the edit, the first three lines must read:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: '{{ name }}-ts'
layer: '{% if archetype == "app" %}application{% else %}library{% endif %}'
```

Apply via `Edit`:

- `old_string`:
  ```
  $schema: 'https://moonrepo.dev/schemas/project.json'
  layer: '{% if archetype == "app" %}application{% else %}library{% endif %}'
  ```
- `new_string`:
  ```
  $schema: 'https://moonrepo.dev/schemas/project.json'
  id: '{{ name }}-ts'
  layer: '{% if archetype == "app" %}application{% else %}library{% endif %}'
  ```

- [ ] **Step 4: Confirm both templates now match the documented field order**

```bash
sed -n '1,5p' .moon/templates/python/moon.yml
echo "---"
sed -n '1,5p' .moon/templates/typescript/moon.yml
echo "---"
sed -n '1,5p' .moon/templates/rust/moon.yml
```
Expected: all three templates show `$schema` → `id` → `layer` → `language` in that order, with
the appropriate `-rs` / `-py` / `-ts` suffix.

- [ ] **Step 5: Render the python template and confirm the generated `id:` is suffixed (the "test passes")**

Generate a throwaway python package, confirm it carries `id: 'throwaway-py'` and that the rendered
`moon.yml` parses, then discard it. Mirrors SMA-381's template verification.

```bash
moon generate python --to py/packages/throwaway --defaults --force -- --name throwaway --archetype library
grep -n 'id:' py/packages/throwaway/moon.yml         # expect: id: 'throwaway-py'
moon project throwaway-py | grep -iE 'Layer|Error'   # expect: Layer: library (no parse error)
rm -rf py/packages/throwaway
```
Expected: `grep` shows `id: 'throwaway-py'`; `moon project` reports `Layer: library` with **no**
`config::parse::failed` error.

> If `moon generate` is non-interactive in this environment and `--defaults --force` doesn't
> bypass the prompt, fall back to: render the template manually by substituting `{{ name }}` and
> the archetype branch, write the result to `py/packages/throwaway/moon.yml`, then run
> `moon project throwaway-py` to confirm it parses. The point is to prove the rendered output is
> valid Moon config; how it's rendered is incidental.

- [ ] **Step 6: Render the typescript template and confirm `id:` is suffixed**

```bash
moon generate typescript --to ts/packages/throwaway --defaults --force -- --name throwaway --archetype library
grep -n 'id:' ts/packages/throwaway/moon.yml         # expect: id: 'throwaway-ts'
```
Expected: `grep` shows `id: 'throwaway-ts'`.

> The `ts/` workspace doesn't exist yet (it lands in SMA-359), so `moon project throwaway-ts`
> won't resolve and we can't fully `moon project`-verify here. Verifying the rendered `id:` line
> is the achievable check; SMA-359 will exercise full parse.

```bash
rm -rf ts/packages/throwaway
```

- [ ] **Step 7: Confirm throwaways are gone**

```bash
git status --short
```
Expected: exactly two modified files — `.moon/templates/python/moon.yml` and
`.moon/templates/typescript/moon.yml`. No `throwaway/` directory left behind under `py/packages/`
or `ts/packages/`.

- [ ] **Step 8: Commit**

```bash
git add .moon/templates/python/moon.yml .moon/templates/typescript/moon.yml
git commit -m "fix(repo): emit id: with stack suffix in python/typescript scaffold templates (SMA-383)

The rust scaffold template already emits id: '{{ name }}-rs', satisfying
SMA-380's stack-suffix convention from the moment a project is generated.
The python and typescript templates omitted id: entirely, so 'moon generate'
produced projects that silently violated the convention until the contributor
remembered to add one.

Add id: '{{ name }}-py' to the python template and id: '{{ name }}-ts' to
the typescript template, in the documented \$schema -> id -> layer -> language
order (SMA-383 CONTRIBUTING.md update).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Whole-PR verification

No file changes — asserts the spec's acceptance criteria. (No commit.)

- [ ] **Step 1: Confirm exactly the intended files changed across the two Task commits**

```bash
git diff --stat HEAD~2 HEAD -- CONTRIBUTING.md '.moon/templates/**/moon.yml'
```
Expected: three files changed — `CONTRIBUTING.md`, `.moon/templates/python/moon.yml`,
`.moon/templates/typescript/moon.yml`. Nothing else.

- [ ] **Step 2: Confirm no `type:` regression slipped into the templates**

```bash
grep -rn '^type:' --include=moon.yml . ; echo "exit: $?"
```
Expected: no matches, `exit: 1`. (Re-asserts SMA-381's invariant — `layer:` only, never `type:`.)

- [ ] **Step 3: Confirm all three templates now emit the documented `$schema → id → layer → language` order**

```bash
for t in rust python typescript; do
  echo "=== .moon/templates/$t/moon.yml ==="
  sed -n '1,5p' ".moon/templates/$t/moon.yml"
done
```
Expected: each template's first four non-blank lines are `$schema`, `id` (with the right suffix:
`-rs`, `-py`, `-ts`), `layer`, `language`.

- [ ] **Step 4: Run the affected build/test graph (sanity check)**

```bash
moon ci :build :test --output-style stream
```
Expected: completes green. (No Moon project config changed; this is a docs + templates PR.)

> If `moon ci :build :test` reports "no affected targets" in a non-TTY shell, that's acceptable —
> the authoritative checks are Steps 1–3 plus the per-task render checks (Task 2 Steps 5–6).

- [ ] **Step 5: Confirm the branch is ready to push**

```bash
git log --oneline main..HEAD
```
Expected (newest first; the two pre-existing spec commits are still there):
```
<sha> fix(repo): emit id: with stack suffix in python/typescript scaffold templates (SMA-383)
<sha> docs(repo): document moon.yml field order and SPDX config carve-out (SMA-383)
9a0e73c docs(repo): apply review feedback to SMA-383 spec
b9d6131 docs(repo): spec moon.yml field-order + SPDX config carve-out (SMA-383)
```

---

## Self-review notes

- **Spec coverage:**
  - Change A (SPDX restructure into rule + three exemption bullets) → Task 1, Steps 2–3.
  - Change B (new `### Moon project files` subsection with 9-position field order + layer
    values + `layer:`-vs-`type:` reminder + templates paragraph) → Task 1, Steps 4–5.
  - Change C (python + typescript template `id:` lines) → Task 2, Steps 2–3.
  - SPDX examples match repo reality → Task 1, Step 6.
  - Field order matches existing hand-written files → Task 1, Step 7.
  - Templates render and parse correctly → Task 2, Steps 5–6.
  - Two commits (`docs(repo)` + `fix(repo)`) referencing SMA-383 → Task 1, Step 8; Task 2, Step 8.
  - `moon ci :build :test` green → Task 3, Step 4.
- **Placeholder scan:** every Edit step shows the exact `old_string` / `new_string` content;
  every verification step shows the exact command and expected output. No "implement later" / "add
  appropriate handling" patterns.
- **Type / field-name consistency:** `layer:` everywhere (never `type:`); id suffixes match
  SMA-380 (`-rs` / `-py` / `-ts`); field order in the doc (9 positions) matches the verification
  greps in Task 1 Step 7 and Task 3 Step 3.
- **Out of scope (per spec):** Notion scoping-doc drift — not in this plan.
