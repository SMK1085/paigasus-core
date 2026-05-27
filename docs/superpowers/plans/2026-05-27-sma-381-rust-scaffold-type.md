# SMA-381 — Align Moon project `type:` across rust + py Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give all eight hand-written Moon `moon.yml` projects an explicit `type:` (layer), matching the style both scaffold templates already emit, so layer no longer relies on Moon's implicit `unknown` default.

**Architecture:** Pure config change — one added/edited line per `moon.yml`, no source code, no template changes. "Tests" are Moon's own introspection: `moon project <id>` must report the expected `Layer`, and `moon ci :build :test` must stay green. Rust and python changes land as two separate conventional commits (`chore(rs)` / `chore(py)`).

**Tech Stack:** Moon 2.2.5 (project config / `LayerType`), proto-pinned toolchain.

**Spec:** `docs/superpowers/specs/2026-05-27-sma-381-rust-scaffold-type-design.md`

---

## Environment note

All `moon` commands assume `moon` is on `PATH`. If it isn't, either run `proto install` (per
CONTRIBUTING.md) or prefix your shell once with:

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
```

Run every command from the repo root: `/Users/smaschek/dev/paigasus/paigasus-core`.

## File Structure

Eight hand-written `moon.yml` files get one explicit `type:` line each (field order `id` → `type` →
`language`; the py parent has no `id`, so `type:` goes immediately before `language:`). No other
files change — **both** `.moon/templates/{rust,python}/moon.yml` are deliberately left untouched.

| File | Add `type:` | Current `Layer` → Target |
|------|-------------|--------------------------|
| `rs/crates/libs/paigasus-kernel/moon.yml` | `library` | `unknown` → `library` |
| `rs/crates/bindings/paigasus-py-bindings/moon.yml` | `library` (+ FFI caveat comment) | `unknown` → `library` |
| `rs/crates/services/paigasus-gateway/moon.yml` | `application` | `unknown` → `application` |
| `py/moon.yml` (parent) | `configuration` | `unknown` → `configuration` |
| `py/packages/paigasus-kernel/moon.yml` | `library` | `unknown` → `library` |
| `py/packages/paigasus-ml/moon.yml` | `library` | `unknown` → `library` |
| `py/packages/paigasus-proto/moon.yml` | `library` | `unknown` → `library` |
| `py/packages/paigasus-workflows/moon.yml` | `library` | `unknown` → `library` |

---

## Task 1: Rust crate layers

**Files:**
- Modify: `rs/crates/libs/paigasus-kernel/moon.yml`
- Modify: `rs/crates/bindings/paigasus-py-bindings/moon.yml`
- Modify: `rs/crates/services/paigasus-gateway/moon.yml`

- [ ] **Step 1: Capture the "failing" before-state**

Run:
```bash
moon project paigasus-kernel-rs | grep -i Layer
moon project paigasus-py-bindings-rs | grep -i Layer
moon project paigasus-gateway-rs | grep -i Layer
```
Expected (all three): `  Layer: unknown`

- [ ] **Step 2: Add `type: 'library'` to `paigasus-kernel`**

Edit `rs/crates/libs/paigasus-kernel/moon.yml` to read exactly:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-kernel-rs'
type: 'library'
language: 'rust'
```

- [ ] **Step 3: Add `type: 'library'` + FFI caveat to `paigasus-py-bindings`**

Edit `rs/crates/bindings/paigasus-py-bindings/moon.yml` to read exactly:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-py-bindings-rs'
# Moon-side layer label for this FFI crate (no native `binding` layer exists).
# Built like a library but NOT published as an rlib — ships as a Python wheel
# via maturin. Exclude from regular `--type=library` publish matrices.
type: 'library'
language: 'rust'
```

- [ ] **Step 4: Add `type: 'application'` to `paigasus-gateway`**

Edit `rs/crates/services/paigasus-gateway/moon.yml` to read exactly:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-gateway-rs'
type: 'application'
language: 'rust'
```

- [ ] **Step 5: Verify the layers changed (the "test passes")**

Run:
```bash
moon project paigasus-kernel-rs | grep -i Layer
moon project paigasus-py-bindings-rs | grep -i Layer
moon project paigasus-gateway-rs | grep -i Layer
```
Expected:
```
  Layer: library
  Layer: library
  Layer: application
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

Run:
```bash
for p in py paigasus-kernel-py paigasus-ml-py paigasus-proto-py paigasus-workflows-py; do
  echo -n "$p: "; moon project "$p" | grep -i Layer
done
```
Expected (all five): `  Layer: unknown`

- [ ] **Step 2: Add `type: 'configuration'` to the py parent**

Edit `py/moon.yml` — insert the `type:` line directly above `language:` so the top reads:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

type: 'configuration'
language: 'python'
```
Leave the `fileGroups:` and `tasks:` blocks below it unchanged.

- [ ] **Step 3: Add `type: 'library'` to each of the four leaf packages**

Edit each file to read exactly (substituting the package name in `id:`):

`py/packages/paigasus-kernel/moon.yml`:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-kernel-py'
type: 'library'
language: 'python'
```

`py/packages/paigasus-ml/moon.yml`:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-ml-py'
type: 'library'
language: 'python'
```

`py/packages/paigasus-proto/moon.yml`:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-proto-py'
type: 'library'
language: 'python'
```

`py/packages/paigasus-workflows/moon.yml`:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-workflows-py'
type: 'library'
language: 'python'
```

- [ ] **Step 4: Verify the layers changed (the "test passes")**

Run:
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

## Task 3: Whole-workspace verification

No file changes — this task only asserts the spec's acceptance criteria hold. (No commit.)

- [ ] **Step 1: Confirm both scaffold templates are untouched**

Run:
```bash
git diff --stat HEAD~2 -- .moon/templates/
```
Expected: **no output** (zero lines) — neither `.moon/templates/rust/moon.yml` nor
`.moon/templates/python/moon.yml` appears in the diff.

- [ ] **Step 2: Confirm exactly the eight intended files changed**

Run:
```bash
git diff --stat HEAD~2 -- 'rs/**/moon.yml' 'py/**/moon.yml'
```
Expected: the eight `moon.yml` files from the File Structure table, and nothing else.

- [ ] **Step 3: Run the affected build/test graph (no constraint violations)**

Run:
```bash
moon ci :build :test --output-style stream
```
Expected: completes green. No `enforceProjectTypeRelationships` constraint errors (there are no
`dependsOn` edges, so the layer changes introduce none).

> If `moon ci :build :test` reports "no affected targets" in a non-TTY shell, that is acceptable for
> a config-only change — the layer edits don't alter any task's inputs. The authoritative check is
> Step 1, Step 2, and the per-task `moon project … | grep Layer` assertions in Tasks 1–2.

---

## Self-review notes

- **Spec coverage:** rs layers (Task 1) ✓; py leaf + parent layers (Task 2) ✓; FFI caveat comment
  (Task 1 Step 3) ✓; templates untouched (Task 3 Step 1) ✓; before/after `unknown → typed` assertion
  (per-task Steps 1 + verify) ✓; `moon ci :build :test` green (Task 3 Step 3) ✓. CONTRIBUTING
  field-order + SPDX carve-out are spec'd as separate follow-ups — intentionally not in this plan.
- **Layer values are consistent** between the File Structure table, the edit steps, and the verify
  steps: kernel-rs/py-bindings-rs = `library`, gateway-rs = `application`, py parent =
  `configuration`, four `*-py` leaves = `library`.
- **No placeholders.** Every edit step shows the complete final file contents.
