# SMA-381 — Align Moon project `type:` across rust + py with explicit layers

**Status:** Designed (brainstorming complete; scope covers rust + py)
**Date:** 2026-05-27
**Linear:** [SMA-381](https://linear.app/smaschek/issue/SMA-381/align-rust-scaffold-type-with-hand-written-crates-omit-for-library)
**Branch:** `feature/sma-381-align-rust-scaffold-type-with-hand-written-crates-omit-for`
**References:** SMA-380 (surfaced this during PR #5 review), SMA-358 (py uv workspace + py template), SMA-356 (Moon config + rust template).

## Goal

Give every hand-written Moon project an **explicit `type:`** (Moon 2.x: the *layer*), matching the
style the rust **and** python scaffold templates already emit. Today the hand-written projects
declare only `id` + `language` (leaves) or `language` + tasks (the py parent) and rely on Moon's
implicit default.

This is a **cosmetic / consistency** change — no behavior changes (see "Why this is cosmetic
today"). It closes the gap between scaffolded and hand-written projects, in both stacks, so the
convention "every project declares its layer explicitly" holds uniformly.

## Corrected premise (verified against Moon 2.2.5)

The originating Linear issue and the first draft of this spec both claimed *"Moon defaults an absent
`type` to `library`, so there is no functional bug."* **The default is wrong.** Verified empirically
with `moon project <id>` on this repo:

- Every type-less project reports **`Layer: unknown`** (and `Stack: unknown`) — **not** `library`.
- Moon's `type:` accepts **seven** values (`LayerType`), not two:
  `application, automation, configuration, library, scaffolding, tool, unknown`. Default: `unknown`.
- Moon 2.x displays the field as **"Layer"**; `type:` in `moon.yml` is the legacy alias for the
  modern `layer:` key (both deserialize to `LayerType`). We keep `type:` to match the existing files
  and both templates; switching the key name to `layer:` is out of scope.

So the real before/after is `unknown → {library | application | configuration}`, not
`library → …`. The "no functional bug" conclusion still holds (layer is categorization metadata),
but the characterization is now accurate.

## Why this is cosmetic today

Moon's `type`/layer can carry behavior via `constraints.enforceProjectTypeRelationships` (e.g. a
`library` may not depend on an `application`). Verified that this does **not** bite here:

- `.moon/workspace.yml` has **no `constraints` block** (setting sits at its default).
- **Every project has zero `dependsOn` edges** (`moon project <id>` → `Depends on: —` for all 8).
  With no edges, the relationship rules never fire.

So the layer choice is pure categorization right now. It begins to matter only once dependency edges
exist (SMA-357/360 wiring) — which is exactly why getting the layers right now is worthwhile.

## Decision

Chosen direction: **option (b)** from the issue — add an explicit `type:` to each hand-written
project (rather than option (a), stripping `type:` from the template's library archetype). We then
**expand the scope to the python stack too**, because it has the identical gap and the spec's
principle ("explicit over implicit defaults") applies uniformly.

### Why explicitness (option b), strengthened

- **Forward compatibility.** `unknown` is the current default, not a guarantee. Explicit declarations
  are unaffected if a future Moon changes the default or the layer set.
- **Query / CI ergonomics.** `moon query projects --type=library` is only meaningful if every project
  declares its layer; implicit `unknown` hides projects from (and pollutes) such filters.
- **Self-documenting.** A `moon.yml` that omits `type:` forces the reader to know Moon's defaults;
  explicit is self-describing.
- **Trivial cost.** A handful of one-line additions vs. one modified template line — rounding error,
  and it keeps scaffolded and hand-written projects consistent (the templates already declare `type:`).

### Correction to the issue's framing

The issue's option (b) literally says "add `type: 'library'` to the three hand-written crates." That
conflates them: `paigasus-gateway` is under `rs/crates/services/` and is a service binary, so its
correct layer is `application`, **not** `library`. Choosing option (b) is what surfaces and corrects
that crate's silent `unknown` layer.

## Changes

Field order follows the templates: `id` → `type` → `language`. The added line goes between `id` and
`language` (for the py parent, which has no `id`, `type:` goes immediately before `language:`).

### Rust (`rs/crates/*/moon.yml`)

| File | `type:` | Rationale |
|------|---------|-----------|
| `rs/crates/libs/paigasus-kernel/moon.yml` | `library` | pure library crate |
| `rs/crates/bindings/paigasus-py-bindings/moon.yml` | `library` | FFI/cdylib — see caveat below |
| `rs/crates/services/paigasus-gateway/moon.yml` | `application` | service binary |

`paigasus-py-bindings` is `crate-type = ["cdylib"]` (FFI artifact loaded by Python; not an rlib, not
a runnable app). Moon has no FFI-specific layer among its seven, so `library` is the least-wrong fit
(it builds like a library; nothing runs it). To keep this from misleading `--type=library` filters
later, add a comment next to its `type:` line:

```yaml
# Moon-side layer label for this FFI crate (no native `binding` layer exists).
# Built like a library but NOT published as an rlib — ships as a Python wheel
# via maturin. Exclude from regular `--type=library` publish matrices.
type: 'library'
```

### Python (`py/**/moon.yml`)

| File | `type:` | Rationale |
|------|---------|-----------|
| `py/moon.yml` (parent) | `configuration` | builds nothing — workspace task/config aggregate; stays out of `--type=library` |
| `py/packages/paigasus-kernel/moon.yml` | `library` | uv-built package |
| `py/packages/paigasus-ml/moon.yml` | `library` | uv-built package |
| `py/packages/paigasus-proto/moon.yml` | `library` | generated-proto package |
| `py/packages/paigasus-workflows/moon.yml` | `library` | uv-built package |

The py **parent** has no `id`, no sources, and no buildable artifact — it exists to host the
workspace-wide uv tasks (`lint`/`format`/`typecheck`/`test`) and `fileGroups`. `configuration` is the
honest layer for it and keeps it out of library-publish filters. The four leaves are ordinary
publishable packages → `library`.

### No template changes

Both `.moon/templates/rust/moon.yml` and `.moon/templates/python/moon.yml` already emit `type:`
explicitly for the `library`/`service` archetypes. That explicit style **is** what we are aligning
the hand-written projects to, so both templates are left untouched. (Neither template has a
`configuration`/parent archetype; the py parent is a hand-written one-off — not in scope to add one.)

## Out of scope / follow-up

- **`moon.yml` field-order convention in CONTRIBUTING.** This spec sets `id` → `type` →
  `language`; that convention should be documented in CONTRIBUTING.md so it carries to `contracts/`
  (SMA-360) and `ts/` (SMA-359). Cross-cutting; file separately.
- **SPDX carve-out for config files in CONTRIBUTING.** `moon.yml` is config, not source, and
  carries no SPDX header (the existing files don't, and we don't add one). The CONTRIBUTING SPDX rule
  should explicitly exempt config files (yaml/toml). Cross-cutting; file separately.
- Switching the `moon.yml` key from the legacy `type:` to the modern `layer:` — separate cleanup.
- Adding a `configuration`/parent archetype to the python template — YAGNI; the parent is a one-off.
- Any `dependsOn`/tasks changes on the gateway crate — setting `type:` is metadata only.

## Verification (no behavior change expected)

1. **Before/after layer assertion (the strongest finding).** Before this PR,
   `moon project paigasus-gateway-rs` reports `Layer: unknown`; after, `Layer: application`. Confirm
   the same `unknown → typed` transition empirically for each touched project:
   `paigasus-kernel-rs`/`-py-bindings-rs` → `library`, `paigasus-gateway-rs` → `application`, the four
   `*-py` leaves → `library`, and `py` → `configuration`.
2. `moon generate rust … --archetype=library`/`--archetype=service` and the python equivalents still
   render correctly (templates unchanged; sanity check that the scaffold stays the reference).
3. `moon ci :build :test` stays green; no `enforceProjectTypeRelationships` constraint violations
   (expected, since there are no `dependsOn` edges).

## Acceptance criteria

- All eight hand-written `moon.yml` files declare an explicit `type:` with the layers in the tables
  above (`id` → `type` → `language` ordering; parent has `type:` before `language:`).
- `paigasus-gateway-rs` resolves as `Layer: application`; `py` resolves as `Layer: configuration`.
- `paigasus-py-bindings/moon.yml` carries the FFI caveat comment.
- Both scaffold templates are unchanged.
- `moon ci :build :test` is green; `moon project <id>` resolves for all eight projects with the
  expected layer.
