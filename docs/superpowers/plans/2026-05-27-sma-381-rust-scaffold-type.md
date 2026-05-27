# SMA-381 — Align Moon project `layer:` across rust + py (and fix templates) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give all eight hand-written Moon projects an explicit `layer:`, and fix both scaffold templates to emit `layer:` instead of the invalid `type:` field they currently emit.

**Architecture:** Pure config change — one added/edited line per file, no source code. "Tests" are Moon's own introspection: `moon project <id>` must report the expected `Layer`, generated projects must parse, and `moon ci :build :test` must stay green. Lands as three conventional commits: `chore(rs)`, `chore(py)`, `fix(repo)`.

**Tech Stack:** Moon 2.2.5 (project config / `LayerType`; field is `layer:`, not the pre-2.x `type:`), proto-pinned toolchain.

**Spec:** `docs/superpowers/specs/2026-05-27-sma-381-rust-scaffold-type-design.md`

---

## Environment note

All `moon` commands assume `moon` is on `PATH`. If it isn't, run `proto install` (per CONTRIBUTING.md)
or prefix your shell once with:

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
```

Run every command from the repo root: `/Users/smaschek/dev/paigasus/paigasus-core`.

> **Key-name note:** Moon 2.2.5 rejects `type:` (`unknown field 'type'`); the field is `layer:`. This
> plan uses `layer:` everywhere. Do not write `type:`.

## File Structure

Eight hand-written `moon.yml` files get one explicit `layer:` line each (field order `id` → `layer` →
`language`; the py parent has no `id`, so `layer:` goes immediately before `language:`). Both scaffold
templates have their emitted field corrected from `type:` to `layer:`.

| File | `layer:` | Current `Layer` → Target |
|------|----------|--------------------------|
| `rs/crates/libs/paigasus-kernel/moon.yml` | `library` | `unknown` → `library` |
| `rs/crates/bindings/paigasus-py-bindings/moon.yml` | `library` (+ FFI caveat comment) | `unknown` → `library` |
| `rs/crates/services/paigasus-gateway/moon.yml` | `application` | `unknown` → `application` |
| `py/moon.yml` (parent) | `configuration` | `unknown` → `configuration` |
| `py/packages/paigasus-kernel/moon.yml` | `library` | `unknown` → `library` |
| `py/packages/paigasus-ml/moon.yml` | `library` | `unknown` → `library` |
| `py/packages/paigasus-proto/moon.yml` | `library` | `unknown` → `library` |
| `py/packages/paigasus-workflows/moon.yml` | `library` | `unknown` → `library` |
| `.moon/templates/rust/moon.yml` | emit `layer:` not `type:` | (latent generate bug) |
| `.moon/templates/python/moon.yml` | emit `layer:` not `type:` | (latent generate bug) |

---

## Task 1: Rust crate layers

**Files:**
- Modify: `rs/crates/libs/paigasus-kernel/moon.yml`
- Modify: `rs/crates/bindings/paigasus-py-bindings/moon.yml`
- Modify: `rs/crates/services/paigasus-gateway/moon.yml`

- [ ] **Step 1: Capture the "failing" before-state**

```bash
for p in paigasus-kernel-rs paigasus-py-bindings-rs paigasus-gateway-rs; do
  echo -n "$p: "; moon project "$p" | grep -i Layer
done
```
Expected (all three): `  Layer: unknown`

- [ ] **Step 2: `paigasus-kernel` — final file contents**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-kernel-rs'
layer: 'library'
language: 'rust'
```

- [ ] **Step 3: `paigasus-py-bindings` — final file contents**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-py-bindings-rs'
# Moon-side layer label for this FFI crate (no native `binding` layer exists).
# Built like a library but NOT published as an rlib — ships as a Python wheel
# via maturin. Exclude from any layer=library publish matrix.
layer: 'library'
language: 'rust'
```

- [ ] **Step 4: `paigasus-gateway` — final file contents**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-gateway-rs'
layer: 'application'
language: 'rust'
```

- [ ] **Step 5: Verify the layers changed (the "test passes")**

```bash
for p in paigasus-kernel-rs paigasus-py-bindings-rs paigasus-gateway-rs; do
  echo -n "$p: "; moon project "$p" | grep -i Layer
done
```
Expected:
```
paigasus-kernel-rs:   Layer: library
paigasus-py-bindings-rs:   Layer: library
paigasus-gateway-rs:   Layer: application
```

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-kernel/moon.yml \
        rs/crates/bindings/paigasus-py-bindings/moon.yml \
        rs/crates/services/paigasus-gateway/moon.yml
git commit -m "chore(rs): set explicit Moon layer on hand-written crates (SMA-381)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Python project layers

**Files:**
- Modify: `py/moon.yml`
- Modify: `py/packages/paigasus-kernel/moon.yml`
- Modify: `py/packages/paigasus-ml/moon.yml`
- Modify: `py/packages/paigasus-proto/moon.yml`
- Modify: `py/packages/paigasus-workflows/moon.yml`

- [ ] **Step 1: Capture the "failing" before-state**

```bash
for p in py paigasus-kernel-py paigasus-ml-py paigasus-proto-py paigasus-workflows-py; do
  echo -n "$p: "; moon project "$p" | grep -i Layer
done
```
Expected (all five): `  Layer: unknown`

- [ ] **Step 2: py parent — insert `layer:` above `language:`**

Edit `py/moon.yml` so the top reads exactly as below; leave the `fileGroups:` and `tasks:` blocks
underneath unchanged:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

layer: 'configuration'
language: 'python'
```

- [ ] **Step 3: four leaf packages — final file contents**

`py/packages/paigasus-kernel/moon.yml`:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-kernel-py'
layer: 'library'
language: 'python'
```

`py/packages/paigasus-ml/moon.yml`:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-ml-py'
layer: 'library'
language: 'python'
```

`py/packages/paigasus-proto/moon.yml`:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-proto-py'
layer: 'library'
language: 'python'
```

`py/packages/paigasus-workflows/moon.yml`:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-workflows-py'
layer: 'library'
language: 'python'
```

- [ ] **Step 4: Verify the layers changed**

```bash
for p in py paigasus-kernel-py paigasus-ml-py paigasus-proto-py paigasus-workflows-py; do
  echo -n "$p: "; moon project "$p" | grep -i Layer
done
```
Expected:
```
py:   Layer: configuration
paigasus-kernel-py:   Layer: library
paigasus-ml-py:   Layer: library
paigasus-proto-py:   Layer: library
paigasus-workflows-py:   Layer: library
```

- [ ] **Step 5: Commit**

```bash
git add py/moon.yml \
        py/packages/paigasus-kernel/moon.yml \
        py/packages/paigasus-ml/moon.yml \
        py/packages/paigasus-proto/moon.yml \
        py/packages/paigasus-workflows/moon.yml
git commit -m "chore(py): set explicit Moon layer on packages + parent (SMA-381)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Fix scaffold templates (`type:` → `layer:`)

**Files:**
- Modify: `.moon/templates/rust/moon.yml`
- Modify: `.moon/templates/python/moon.yml`

Both templates currently emit an invalid `type:` field; any project generated from them fails to
parse. Rename the emitted field to `layer:`. The archetype conditional is unchanged.

- [ ] **Step 1: rust template — change the emitted field**

In `.moon/templates/rust/moon.yml`, change the line:
```yaml
type: '{% if archetype == "service" %}application{% else %}library{% endif %}'
```
to:
```yaml
layer: '{% if archetype == "service" %}application{% else %}library{% endif %}'
```

- [ ] **Step 2: python template — change the emitted field**

In `.moon/templates/python/moon.yml`, change the line:
```yaml
type: '{% if archetype == "service" %}application{% else %}library{% endif %}'
```
to:
```yaml
layer: '{% if archetype == "service" %}application{% else %}library{% endif %}'
```

- [ ] **Step 3: Verify a generated project parses (the "test passes")**

Generate a throwaway crate from the rust template, confirm it emits `layer:` and loads without a
parse error, then discard it:
```bash
moon generate rust ./rs/crates/libs/tmp-layer-check --defaults --template-vars 'name=tmp-layer-check'
grep -n 'layer:' rs/crates/libs/tmp-layer-check/moon.yml   # expect: layer: 'library'
moon sync projects >/dev/null 2>&1; moon project tmp-layer-check-rs | grep -i Layer  # expect: Layer: library
rm -rf rs/crates/libs/tmp-layer-check
```
Expected: the `grep` shows `layer: 'library'`; `moon project` reports `Layer: library` with **no**
`config::parse::failed` error. (If `moon generate` flag names differ in your Moon build, generate
interactively instead; the assertion is that the rendered file contains `layer:` and parses.)

- [ ] **Step 4: Confirm the throwaway is gone**

```bash
git status --short   # expect only the two template files modified; no tmp-layer-check/ left behind
```

- [ ] **Step 5: Commit**

```bash
git add .moon/templates/rust/moon.yml .moon/templates/python/moon.yml
git commit -m "fix(repo): emit layer: not invalid type: in Moon scaffold templates (SMA-381)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Whole-workspace verification

No file changes — asserts the spec's acceptance criteria. (No commit.)

- [ ] **Step 1: Confirm exactly the intended files changed across the three commits**

```bash
git diff --stat HEAD~3 -- '**/moon.yml' '.moon/templates/**/moon.yml'
```
Expected: the eight hand-written `moon.yml` files plus the two template files — and nothing else.

- [ ] **Step 2: Confirm no project carries the invalid `type:` field anymore**

```bash
grep -rn '^type:' --include=moon.yml . ; echo "exit: $?"
```
Expected: no matches (grep exit 1). Only `layer:` should appear.

- [ ] **Step 3: Run the affected build/test graph (no constraint violations)**

```bash
moon ci :build :test --output-style stream
```
Expected: completes green; no `enforceProjectTypeRelationships` errors (no `dependsOn` edges exist).

> If `moon ci :build :test` reports "no affected targets" in a non-TTY shell, that is acceptable for a
> config-only change. The authoritative checks are the per-task `moon project … | grep Layer`
> assertions, the Task 3 generate-and-parse check, and Steps 1–2 here.

---

## Self-review notes

- **Spec coverage:** rs layers (Task 1) ✓; py leaf + parent layers (Task 2) ✓; FFI caveat comment
  (Task 1 Step 3) ✓; template fix `type:`→`layer:` (Task 3) ✓; before/after `unknown → typed`
  assertions (per-task) ✓; generated project parses (Task 3 Step 3) ✓; `moon ci :build :test` green
  (Task 4 Step 3) ✓. CONTRIBUTING field-order + SPDX carve-out are spec'd as separate follow-ups —
  intentionally not in this plan.
- **Field name is `layer:` everywhere** — the invalid `type:` appears only where the plan describes
  removing it (Task 3, Task 4 Step 2).
- **Layer values are consistent** between the File Structure table, the edit steps, and the verify
  steps: kernel-rs/py-bindings-rs = `library`, gateway-rs = `application`, py parent =
  `configuration`, four `*-py` leaves = `library`.
- **No placeholders.** Every edit step shows the complete final file contents (or the exact line
  change for the templates).
