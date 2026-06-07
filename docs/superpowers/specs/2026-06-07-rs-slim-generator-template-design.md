# Slim Rust generator template & unify build profile (SMA-374)

- **Linear:** SMA-374
- **Branch:** `feature/sma-374-slim-rust-generator-template-and-unify-build-profile-with`
- **Date:** 2026-06-07
- **Status:** Design approved (revised post-review); ready for implementation plan
- **Follow-up from:** SMA-357 (review S8), generator template from SMA-356
- **Review:** [`2026-06-07-rs-slim-generator-template-design-review.md`](./2026-06-07-rs-slim-generator-template-design-review.md)
  (findings F1–F4 incorporated — see "Review incorporated" below)

## Problem

`.moon/tasks/rust.yml` already owns `build`/`test`/`lint`/`fmt` for every Rust crate
(debug `cargo build`, scoped by `inheritedBy.languages: ['rust']`). The generator
template `.moon/templates/rust/moon.yml` still redefines those tasks per-crate for the
`service` archetype — including `cargo build --release`.

Two problems follow:

1. **Latent mixed-profile bug.** A service generated from the template would build
   `--release` while sibling crates build debug under one `moon ci :build`, producing
   mixed-profile artifacts in `rs/target/`.
2. **Redundancy.** The service archetype re-declares `test` verbatim and re-states the
   `build` command, duplicating what `rust.yml` already provides.

The four checked-in crate `moon.yml` files are already slim (`id`/`layer`/`language`
only), so the bug is **latent in the template, not live** in the repo today. Separately,
the checked-in `paigasus-gateway` service lacks the proto/kernel `dependsOn` a service
should carry per the template's intent.

## Approach

Make `rust.yml` the single source of build tasks; the template and crates override **only
what differs**. Keep the change **purely mechanical** — template slimming, profile
unification, and a static `dependsOn` reconciliation.

**Codegen and affected-graph edges are explicitly out of scope** and deferred to
**SMA-389**. `contracts:generate` (buf) writes the prost/tonic output into
`paigasus-proto`'s `src/generated/`, which is **committed** (ADR-0004) so that builds
compile the committed source offline — no prebuild. SMA-360 deliberately left the
`contracts:generate` build edges un-wired (no protos to order yet, and depending on a
no-op `generate` would force `buf` onto PATH for every proto build). SMA-389 owns wiring
`paigasus-proto-rs:build`/`:test` → `contracts:generate` (plus py/ts) and lands with the
first real `.proto` definitions. SMA-374 therefore adds **no** `contracts:generate` edge,
**no** `^:build` ordering, and **does not touch `paigasus-proto`**.

## Changes

### 1. `.moon/tasks/rust.yml`

Add a dedicated release task. `build` stays debug; `test`/`lint`/`fmt` unchanged.

```yaml
tasks:
  build:
    command: 'cargo build'
    inputs: ['@group(sources)', 'Cargo.toml']
  build-release:
    command: 'cargo build --release'
    inputs: ['@group(sources)', 'Cargo.toml']
  test:    # unchanged
    command: 'cargo nextest run --no-tests=pass'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
  lint:    # unchanged
    command: 'cargo clippy --all-targets -- -D warnings'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
  fmt:     # unchanged
    command: 'cargo fmt --check'
    inputs: ['@group(sources)']
```

### 2. `.moon/templates/rust/moon.yml`

The `service` archetype drops **all** task overrides; it only emits project `dependsOn`.
The `library` archetype is unchanged (still emits no `dependsOn` on purpose — see template
caveats).

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: '{{ name }}-rs'
layer: '{% if archetype == "service" %}application{% else %}library{% endif %}'
language: 'rust'
{%- if archetype == "service" %}
dependsOn:
  - 'paigasus-proto-rs'
  - 'paigasus-kernel-rs'
{%- endif %}
```

`build`/`build-release`/`test`/`lint`/`fmt` all inherit from `rust.yml`. The `--release`
bug, the redundant `test` redefinition, and the premature `contracts:generate`/`^:build`
edges are all gone (the codegen edge returns via SMA-389 on `paigasus-proto-rs`, not here).

### 3. `rs/crates/services/paigasus-gateway/moon.yml`

Reconcile the live service to match the slimmed template: add the static project deps, no
task overrides. This is a benign project-graph edge — no `buf`-on-PATH, no network, no
task-ordering change.

```yaml
dependsOn:
  - 'paigasus-proto-rs'
  - 'paigasus-kernel-rs'
```

### Not changed

- `rs/crates/libs/paigasus-proto/moon.yml` — untouched. Its `contracts:generate` edge is
  SMA-389's, landing with the first protos.

## Out of scope

- **`contracts:generate` build edges and `^:build` affected-graph ordering** — owned by
  SMA-389 (lands with first protos). `contracts:generate` stays orphan in CI until then,
  which is the deliberate post-SMA-360 state.
- **Adding `:build-release` to CI's `T=(…)` array** — release artifacts are dormant per
  ADR-0011; activation is SMA-407 (see F3 note below).
- `paigasus-py-bindings` dependency wiring (separate concern).
- `clean: true` in `buf.gen.yaml` (tracked under SMA-389 / codegen-drift).

## Review incorporated

- **F1 (Medium):** the earlier "Model B" (wiring `contracts:generate` onto
  `paigasus-proto-rs:build` here) reversed ADR-0004's no-prebuild property, made affected
  builds network-dependent, and validated regenerated rather than committed code — and it
  duplicated SMA-389, which deliberately defers that wiring. **Resolved by deferring all
  codegen edges to SMA-389; `proto-rs` is untouched.**
- **F2 (Low):** the `mergeDeps: append` concern is now moot — no `deps:`-only partial
  overrides remain in this change.
- **F3 (Low):** `build-release` is intentionally **not** added to CI's `T` array, so the
  release profile won't compile in CI until activation. Flagged for **SMA-407** so the
  first real release build isn't the first time the release profile compiles in CI.
- **F4 (Nit):** the template smoke test leaves Moon cache residue
  (`.moon/cache/states/<name>/`, gitignored) — run `moon clean` afterward and do not treat
  a clean `git status` as proof of discard.

## Verification

- `moon ci :build` → inspect `rs/target/`: only `debug/`, **no** `release/`
  (uniform debug profile across all crates).
- `moon run :build-release` → `rs/target/release/` appears (run on demand; not in CI).
- `moon project paigasus-gateway-rs` shows `dependsOn` proto/kernel and inherited tasks
  with no per-crate overrides.
- Template smoke test: generate a throwaway service, confirm its `moon.yml` carries
  `dependsOn` only and **no** per-crate tasks; then `moon clean` and remove the generated
  crate (don't rely on `git status` alone — Moon cache residue is gitignored).

Implementation note: moon/buf are proto-managed and off the Bash tool PATH — export
`~/.proto/bin:~/.proto/shims`. No macOS `timeout` available.
