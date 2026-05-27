# SMA-380 — Suffix Rust Moon Project IDs with `-rs` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the three Rust crate Moon projects explicit `-rs`-suffixed ids (mirroring the `-py` side), without breaking the `cargo` task commands, and update the scaffold template so future crates stay consistent.

**Architecture:** Drop `-p $project` from the shared `.moon/tasks/rust.yml` first (Moon runs each task from the crate dir, so bare `cargo` targets the right package and the task file becomes id-agnostic), *then* rename the three project ids. Finally rewrite the `.moon/templates/rust/` scaffold to emit `-rs` ids, `-rs` `dependsOn`, and the no-`-p` convention. No Rust code, Cargo crate names, directory layout, or `contracts` id changes.

**Tech Stack:** Moon 2.2.5 (proto-pinned), Cargo workspace (rust 1.95.0), `cargo-nextest`.

**Spec:** `docs/superpowers/specs/2026-05-27-sma-380-rust-moon-id-suffix-design.md`

---

## Prerequisites & conventions

- All `moon` commands assume the proto-provided `moon` is on `PATH` (see CONTRIBUTING.md → local development). If it is not, prefix with the proto path: `~/.proto/bin/moon …`.
- Work happens on branch `feature/sma-380-suffix-rust-moon-project-ids-with-rs-for-cross-stack` (already created).
- Commit scope is `chore(rs)` with a trailing `(SMA-380)`, matching the bootstrap commit style.
- **Task order matters:** Task 1 (drop `-p`) MUST land before Task 2 (rename ids). If the ids were renamed first, `cargo … -p paigasus-kernel-rs` would fail (Cargo expects the crate name `paigasus-kernel`, not the Moon id). Doing Task 1 first keeps every commit green.

## Files touched

- Modify: `.moon/tasks/rust.yml` — drop `-p $project` from all four task commands (Task 1).
- Modify: `rs/crates/libs/paigasus-kernel/moon.yml` — add `id: 'paigasus-kernel-rs'` (Task 2).
- Modify: `rs/crates/bindings/paigasus-py-bindings/moon.yml` — add `id: 'paigasus-py-bindings-rs'` (Task 2).
- Modify: `rs/crates/services/paigasus-gateway/moon.yml` — add `id: 'paigasus-gateway-rs'` (Task 2).
- Modify: `.moon/templates/rust/moon.yml` — restructure: `id`, `type`, service-only `dependsOn`/tasks (Task 3).
- Modify: `.moon/templates/rust/template.yml` — update prose caveats to `-rs` forms (Task 3).
- Verify only (no expected change): docs under `CLAUDE.md`, `CONTRIBUTING.md`, `*/README.md` (Task 4).

---

## Task 1: Make the shared Rust task file id-agnostic (drop `-p`)

**Files:**
- Modify: `.moon/tasks/rust.yml`

- [ ] **Step 1: Confirm the current (pre-change) commands still carry `-p $project`**

Run: `moon project paigasus-kernel`
Expected: under `Tasks`, the commands read `cargo build -p paigasus-kernel`, `cargo nextest run -p paigasus-kernel --no-tests=pass`, etc. (Moon expands `$project` to the bare id today.)

- [ ] **Step 2: Edit `.moon/tasks/rust.yml` — remove `-p $project` from every command**

Replace the `tasks:` block so it reads exactly:

```yaml
tasks:
  build:
    command: 'cargo build'
    inputs: ['@group(sources)', 'Cargo.toml']
  test:
    command: 'cargo nextest run --no-tests=pass'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
  lint:
    command: 'cargo clippy --all-targets -- -D warnings'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
  fmt:
    command: 'cargo fmt --check'
    inputs: ['@group(sources)']
```

Leave the `$schema`, `inheritedBy`, and `fileGroups` blocks above it untouched.

- [ ] **Step 3: Verify the commands are now bare**

Run: `moon project paigasus-kernel`
Expected: `Tasks` now show `cargo build`, `cargo nextest run --no-tests=pass`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — no `-p`. (Ids are still bare at this point.)

- [ ] **Step 4: Verify all three crates still build/lint/fmt/test green under the bare commands**

Run: `moon run :build :lint :fmt :test`
Expected: all targets succeed across `paigasus-kernel`, `paigasus-py-bindings`, `paigasus-gateway`. `:test` passes via `--no-tests=pass` (the stub crates have no tests). This proves bare `cargo` (run from each crate dir) targets the correct package.

- [ ] **Step 5: Confirm `cargo fmt --check` from a crate dir scopes to that crate only (spec Verification / N5)**

Run: `cd rs/crates/libs/paigasus-kernel && cargo fmt --check && cd -`
Expected: exits 0 and reports nothing for the workspace at large — bare `cargo fmt --check` (no `-p`, no `--all`) formats only the package whose `Cargo.toml` is in the cwd. (If it unexpectedly touched other crates, stop and reconsider — but with rust 1.95.0 it scopes to the current package.)

- [ ] **Step 6: Commit**

```bash
git add .moon/tasks/rust.yml
git commit -m "chore(rs): drop -p \$project from shared rust task file (SMA-380)

Moon runs each Rust task from the crate dir, so bare cargo targets the
current package. Makes the shared task file independent of project ids,
so the upcoming -rs id rename cannot break it.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Add explicit `-rs` ids to the three crates

**Files:**
- Modify: `rs/crates/libs/paigasus-kernel/moon.yml`
- Modify: `rs/crates/bindings/paigasus-py-bindings/moon.yml`
- Modify: `rs/crates/services/paigasus-gateway/moon.yml`

- [ ] **Step 1: Add `id` to `rs/crates/libs/paigasus-kernel/moon.yml`**

The whole file becomes:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-kernel-rs'
language: 'rust'
```

- [ ] **Step 2: Add `id` to `rs/crates/bindings/paigasus-py-bindings/moon.yml`**

The whole file becomes:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-py-bindings-rs'
language: 'rust'
```

- [ ] **Step 3: Add `id` to `rs/crates/services/paigasus-gateway/moon.yml`**

The whole file becomes:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-gateway-rs'
language: 'rust'
```

- [ ] **Step 4: Verify the new ids resolve and the old ones are gone**

Run: `moon project paigasus-kernel-rs`
Expected: resolves; `Source: rs/crates/libs/paigasus-kernel`; the four bare-cargo tasks are listed.

Run: `moon project paigasus-kernel`
Expected: **fails** — no project with that id any more (Moon reports it cannot find a project named `paigasus-kernel`). This confirms the rename took effect.

Run: `moon project paigasus-py-bindings-rs` and `moon project paigasus-gateway-rs`
Expected: both resolve to their respective sources.

- [ ] **Step 5: Verify the Python side is untouched (no regression)**

Run: `moon project paigasus-kernel-py`
Expected: still resolves (`Source: py/packages/paigasus-kernel`) — the rs rename did not disturb the `-py` projects.

- [ ] **Step 6: Verify every task still runs under the new ids**

Run: `moon run :build :lint :fmt :test`
Expected: all succeed across the three `-rs` crates (Moon target globs resolve by the new ids). A run of a specific new target also works: `moon run paigasus-kernel-rs:build` → success.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/libs/paigasus-kernel/moon.yml rs/crates/bindings/paigasus-py-bindings/moon.yml rs/crates/services/paigasus-gateway/moon.yml
git commit -m "chore(rs): suffix Rust crate Moon project ids with -rs (SMA-380)

paigasus-kernel-rs, paigasus-py-bindings-rs, paigasus-gateway-rs —
mirrors the -py naming convention from SMA-358 for cross-stack symmetry.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Update the scaffold template for consistency

**Files:**
- Modify: `.moon/templates/rust/moon.yml`
- Modify: `.moon/templates/rust/template.yml`

- [ ] **Step 1: Rewrite `.moon/templates/rust/moon.yml`**

Replace the entire file with:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: '{{ name }}-rs'
type: '{% if archetype == "service" %}application{% else %}library{% endif %}'
language: 'rust'
{%- if archetype == "service" %}
dependsOn:
  - 'paigasus-proto-rs'
  - 'paigasus-kernel-rs'
tasks:
  build:
    command: 'cargo build --release'
    inputs: ['@group(sources)', 'Cargo.toml']
    deps: ['contracts:generate', '^:build']
  test:
    command: 'cargo nextest run --no-tests=pass'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
    deps: ['contracts:generate']
{%- endif %}
```

Note: libraries now emit no `tasks` block (they inherit all four from `.moon/tasks/rust.yml`, matching the hand-written crates); services override only `build`/`test` for their `--release` + `contracts:generate` deltas and inherit `lint`/`fmt`. `contracts:generate` stays bare — `contracts` keeps its bare id (spec Key decision §2).

- [ ] **Step 2: Update the prose caveats in `.moon/templates/rust/template.yml`**

In the `description:` block, change the two bare references to their `-rs` forms. The service caveat line:

```
  - The `service` archetype emits dependsOn paigasus-proto-rs/paigasus-kernel-rs and a
```

and the library caveat hand-edit note:

```
    paigasus-kernel-rs, but paigasus-kernel-rs and paigasus-proto-rs must NOT (self/cycle).
    Add `dependsOn: ['paigasus-kernel-rs']` by hand where appropriate.
```

(Leave the `variables:` block — `name`, `archetype` — unchanged; `prompt: 'Crate name (e.g. paigasus-kernel)?'` refers to the Cargo crate name and stays bare.)

- [ ] **Step 3: Render a `library` and confirm the output**

```bash
moon generate rust --to .tmp-sma380-lib --defaults -- --name=demo-lib --archetype=library
cat .tmp-sma380-lib/moon.yml
```

Expected `moon.yml`:
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: 'demo-lib-rs'
type: 'library'
language: 'rust'
```
(no `dependsOn`, no `tasks` block.)

- [ ] **Step 4: Render a `service` and confirm the output**

```bash
moon generate rust --to .tmp-sma380-svc --defaults -- --name=demo-svc --archetype=service
cat .tmp-sma380-svc/moon.yml
```

Expected `moon.yml`: `id: 'demo-svc-rs'`, `type: 'application'`, `dependsOn` listing `paigasus-proto-rs` and `paigasus-kernel-rs`, and `build`/`test` tasks with `cargo build --release` (+ `deps: ['contracts:generate', '^:build']`) and `cargo nextest run --no-tests=pass` (+ `deps: ['contracts:generate']`).

- [ ] **Step 5: Delete the scratch output (must NOT be committed)**

```bash
rm -rf .tmp-sma380-lib .tmp-sma380-svc
git status --porcelain
```
Expected: only `.moon/templates/rust/moon.yml` and `.moon/templates/rust/template.yml` show as modified — no `.tmp-sma380-*` paths.

- [ ] **Step 6: Commit**

```bash
git add .moon/templates/rust/moon.yml .moon/templates/rust/template.yml
git commit -m "chore(rs): scaffold template emits -rs ids and dependsOn (SMA-380)

Generated crates now set id: '<name>-rs'; the service archetype's
dependsOn points at paigasus-proto-rs/paigasus-kernel-rs and overrides
only build/test (libraries inherit all tasks from .moon/tasks/rust.yml).
Also fixes the template test gaining --no-tests=pass.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Final verification sweep (docs + full graph)

**Files:**
- Verify only: `CLAUDE.md`, `CONTRIBUTING.md`, `rs/README.md`, `py/README.md` (no change expected).

- [ ] **Step 1: Re-grep for any LIVE Moon-target references to the renamed crates**

Run:
```bash
grep -rn "paigasus-kernel:\|paigasus-gateway:\|paigasus-py-bindings:\|moon run .*paigasus" \
  --include="*.md" . | grep -v "docs/superpowers/"
```
Expected: **no matches.** Live docs reference crate *names* (e.g. "the `paigasus-kernel` crate"), not Moon *targets* (`paigasus-kernel:build`). The date-prefixed `docs/superpowers/` plan/spec files are historical records and are intentionally excluded. If a real `<crate>:target` reference surfaces, update it to the `-rs` form and include it in this task's commit.

- [ ] **Step 2: Full graph green-check across both stacks**

Run: `moon run :build :lint :fmt :test`
Expected: all rs targets succeed.

Run: `moon ci :build :test`
Expected: resolves and runs without error in non-TTY (explicit targets given, per the Moon 2.x requirement).

- [ ] **Step 3: Sanity-check the workspace project list**

Run: `moon project paigasus-gateway-rs` and `moon project paigasus-kernel-py`
Expected: both resolve — confirms the rs `-rs` ids and the untouched py `-py` ids coexist.

- [ ] **Step 4: Commit (only if Step 1 surfaced a doc edit)**

If a live target reference was fixed:
```bash
git add <changed docs>
git commit -m "docs: update Moon target references to -rs ids (SMA-380)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Otherwise no commit — this task is purely the final verification gate.

> **Note (spec Verification / N3):** The first `moon ci` run after this branch merges will be a one-time cold-cache run — renaming three project ids changes the workspace-graph hash and invalidates `.moon/cache/states/workspaceGraph.json`. This is expected, not a failure.

---

## Done criteria

- `moon project paigasus-{kernel,py-bindings,gateway}-rs` all resolve; bare `paigasus-kernel` no longer does.
- `moon run :build :lint :fmt :test` and `moon ci :build :test` are green; `paigasus-*-py` projects still resolve.
- `moon generate rust … --archetype=library|service` emits correct `-rs` ids (and `-rs` `dependsOn` for services).
- No `.tmp-sma380-*` scratch dirs committed; no Cargo crate names, directories, or `contracts` id changed.
