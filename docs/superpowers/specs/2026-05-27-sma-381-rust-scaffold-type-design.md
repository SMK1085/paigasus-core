# SMA-381 — Align Moon project `layer:` across rust + py (and fix the scaffold templates)

**Status:** Designed (brainstorming complete; corrected during implementation — field is `layer:`, not `type:`; templates brought into scope)
**Date:** 2026-05-27
**Linear:** [SMA-381](https://linear.app/smaschek/issue/SMA-381/align-rust-scaffold-type-with-hand-written-crates-omit-for-library)
**Branch:** `feature/sma-381-align-rust-scaffold-type-with-hand-written-crates-omit-for`
**References:** SMA-380 (surfaced this during PR #5 review), SMA-358 (py uv workspace + py template), SMA-356 (Moon config + rust template).

## Goal

Give every hand-written Moon project an **explicit `layer:`**, and fix all three scaffold templates
(rust, python, typescript) to emit a valid `layer:` (they currently emit an invalid `type:` field).
After this, scaffolded and hand-written projects share one explicit, *parseable* style across the
rust, python, and typescript stacks.

## Corrected premise (verified against Moon 2.2.5)

The originating Linear issue, and the first draft of this spec, were written around a `type:` field
and the claim that *"Moon defaults an absent `type` to `library`."* **Both are wrong for Moon 2.2.5**,
verified empirically on this repo:

- **The field is `layer:`, not `type:`.** Moon 2.2.5's project parser hard-errors on `type:`
  (`unknown field 'type'`). The accepted fields are `$schema, dependsOn, deps, docker, env,
  fileGroups, id, language, layer, owners, project, stack, tags, tasks, toolchains, workspace`.
  `type` was renamed to `layer` in Moon 2.x; the published JSON schema still lists `type` as an
  alias, but the 2.2.5 binary rejects it.
- **The default is `unknown`, not `library`.** A project with no `layer:` reports `Layer: unknown`
  (and `Stack: unknown`) under `moon project <id>`.
- **`layer:` accepts seven values** (`LayerType`): `application, automation, configuration, library,
  scaffolding, tool, unknown`.

So the real before/after is `unknown → {library | application | configuration}`.

### This makes the scaffold templates a real bug, not a cosmetic mismatch

All three scaffold templates (`.moon/templates/{rust,python,typescript}/moon.yml`) emit a
`type: '…'` archetype conditional. Because `type:` is rejected, **any project generated from these
templates produces a `moon.yml` that fails to parse.** It hasn't bitten anyone only because no
`moon generate` has run since the field rename. (The typescript template exists ahead of the `ts/`
workspace setup in SMA-359, but carries the same bug, so it's fixed here too.) Fixing the templates
(`type:` → `layer:`) is therefore part of this issue — both to remove the bug and because true
scaffold/hand-written alignment requires the templates to use `layer:` too.

## Why this is otherwise cosmetic today

Moon's `layer` can carry behavior via `constraints.enforceLayerRelationships` (e.g. a `library`
may not depend on an `application`). Verified this does **not** bite here:

- `.moon/workspace.yml` has **no `constraints` block** (setting sits at its default).
- **Every project has zero `dependsOn` edges** (`moon project <id>` → `Depends on: —` for all 8).
  With no edges, the relationship rules never fire.

So the layer values are pure categorization right now; they begin to matter once dependency edges
exist (SMA-357/360 wiring) — which is why setting them correctly now is worthwhile.

## Decision

Add an explicit `layer:` to each hand-written project (option (b) from the issue — explicitness over
stripping the field), expanded to **both stacks** and to **both templates**:

- **Explicitness rationale:** forward-compatibility (don't rely on the `unknown` default), query/CI
  ergonomics (`moon query` layer filters only work if every project declares a layer), and
  self-documenting config. Cost is a handful of one-line edits.
- **Issue framing correction:** the issue's option (b) said "add `type: 'library'` to the three
  hand-written crates," but `paigasus-gateway` (under `rs/crates/services/`) is a service binary, so
  its correct layer is `application`. Choosing option (b) is what surfaces and corrects that crate's
  silent `unknown` layer.

## Changes

Field order follows the templates: `id` → `layer` → `language`. The added line goes between `id` and
`language` (the py parent has no `id`, so `layer:` goes immediately before `language:`).

### Rust (`rs/crates/*/moon.yml`)

| File | `layer:` | Rationale |
|------|----------|-----------|
| `rs/crates/libs/paigasus-kernel/moon.yml` | `library` | pure library crate |
| `rs/crates/bindings/paigasus-py-bindings/moon.yml` | `library` (+ FFI caveat comment) | FFI/cdylib — see below |
| `rs/crates/services/paigasus-gateway/moon.yml` | `application` | service binary |

`paigasus-py-bindings` is `crate-type = ["cdylib"]` (FFI artifact loaded by Python; not an rlib, not
a runnable app). Moon has no FFI-specific layer among its seven, so `library` is the least-wrong fit.
A comment next to its `layer:` line records this so it isn't mistaken for a publishable rlib:

```yaml
# Moon-side layer label for this FFI crate (no native `binding` layer exists).
# Built like a library but NOT published as an rlib — ships as a Python wheel
# via maturin. Exclude from any layer=library publish matrix.
layer: 'library'
```

### Python (`py/**/moon.yml`)

| File | `layer:` | Rationale |
|------|----------|-----------|
| `py/moon.yml` (parent) | `configuration` | builds nothing — workspace task/config aggregate; stays out of layer=library filters |
| `py/packages/paigasus-kernel/moon.yml` | `library` | uv-built package |
| `py/packages/paigasus-ml/moon.yml` | `library` | uv-built package |
| `py/packages/paigasus-proto/moon.yml` | `library` | generated-proto package |
| `py/packages/paigasus-workflows/moon.yml` | `library` | uv-built package |

The py **parent** has no `id`, no sources, and no buildable artifact — it hosts the workspace-wide uv
tasks and `fileGroups`. `configuration` is the honest layer and keeps it out of library-publish
filters. The four leaves are ordinary packages → `library`.

### Templates (`.moon/templates/*/moon.yml`)

Change the emitted field from the invalid `type:` to `layer:` in **all three** templates; the
archetype conditional (`application` for the app/service archetype, else `library`) is unchanged:

| File | Change |
|------|--------|
| `.moon/templates/rust/moon.yml` | `type:` → `layer:` (line emitting the archetype conditional) |
| `.moon/templates/python/moon.yml` | `type:` → `layer:` (same) |
| `.moon/templates/typescript/moon.yml` | `type:` → `layer:` (same; archetype values `app`/else) |

After this, generated projects parse, and they match the hand-written `layer:` style.

## Commit grouping

- `chore(rs)` — the three rust crate files.
- `chore(py)` — the py parent + four leaf files.
- `fix(repo)` — all three scaffold templates (a genuine bug fix: generate currently yields unparseable output).

## Out of scope / follow-up

- **`moon.yml` field-order convention in CONTRIBUTING** (`$schema` → `id` → `layer` → `language`), so
  it carries to `contracts/` (SMA-360) and `ts/` (SMA-359). Cross-cutting; file separately.
- **SPDX carve-out for config files in CONTRIBUTING.** `moon.yml` is config, not source, and carries
  no SPDX header. The CONTRIBUTING SPDX rule should exempt config files (yaml/toml). File separately.
- Adding a `configuration`/parent archetype to the python template — YAGNI; the parent is a one-off.
- Any `dependsOn`/tasks changes on the gateway crate — setting `layer:` is metadata only.

## Verification

1. **Before/after layer assertion.** Before: `moon project <id>` reports `Layer: unknown` for all
   eight hand-written projects. After: `paigasus-kernel-rs`/`-py-bindings-rs` → `library`,
   `paigasus-gateway-rs` → `application`, the four `*-py` leaves → `library`, `py` → `configuration`.
   Confirm empirically.
2. **Templates parse and emit `layer:`.** Generate a throwaway project from a template, confirm the
   rendered `moon.yml` contains `layer:` (not `type:`) and that `moon project` loads it without a
   parse error; then discard the throwaway. Also assert no `moon.yml` (including the three templates)
   still contains a `type:` line.
3. `moon ci :build :test` stays green; no `enforceLayerRelationships` violations (no edges).

## Acceptance criteria

- All eight hand-written `moon.yml` files declare an explicit `layer:` with the values above
  (`id` → `layer` → `language` ordering; parent has `layer:` before `language:`).
- `paigasus-gateway-rs` resolves as `Layer: application`; `py` resolves as `Layer: configuration`.
- `paigasus-py-bindings/moon.yml` carries the FFI caveat comment.
- All three scaffold templates emit `layer:` (not `type:`), and a generated project parses.
- `moon ci :build :test` is green; `moon project <id>` resolves for all eight projects with the
  expected layer.
