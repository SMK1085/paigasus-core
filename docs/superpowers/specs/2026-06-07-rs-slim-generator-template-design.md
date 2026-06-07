# Slim Rust generator template & unify build profile (SMA-374)

- **Linear:** SMA-374
- **Branch:** `feature/sma-374-slim-rust-generator-template-and-unify-build-profile-with`
- **Date:** 2026-06-07
- **Status:** Design approved; ready for implementation plan
- **Follow-up from:** SMA-357 (review S8), generator template from SMA-356

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
the checked-in `paigasus-gateway` service lacks the proto/kernel dependency wiring a
service should carry.

`buf generate` (`contracts:generate`) writes the prost/tonic Rust output **into the
`paigasus-proto` crate** (`src/generated/`), so proto-rs — not each service — is the real
consumer of generation.

## Approach

Make `rust.yml` the single source of build tasks; the template and special crates
override **only what differs**. Wire generation with **Model B**: the crate that consumes
generated code (`paigasus-proto-rs`) owns the `contracts:generate` dependency, and
services pull it transitively through build ordering rather than declaring it themselves.

Moon's default `mergeDeps` strategy is `append`, so a task override that specifies only
`deps:` merges onto the inherited `command`/`inputs` from `rust.yml` — no need to restate
the command.

## Changes

### 1. `.moon/tasks/rust.yml`

Add a dedicated release task and dependency-build ordering. `build` keeps debug.

```yaml
tasks:
  build:
    command: 'cargo build'
    inputs: ['@group(sources)', 'Cargo.toml']
    deps: ['^:build']            # build project-deps first (no-op for dep-less libs)
  build-release:
    command: 'cargo build --release'
    inputs: ['@group(sources)', 'Cargo.toml']
    deps: ['^:build-release']
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

`^:build` generalizes the ordering the old template expressed per-service. It is a no-op
for `paigasus-kernel-rs` / `paigasus-proto-rs` (no project-level `dependsOn`), so it
introduces no cycles.

### 2. `.moon/templates/rust/moon.yml`

The `service` archetype drops **all** task overrides; it only needs project deps. The
`library` archetype is unchanged (still emits no `dependsOn` on purpose — see template
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
bug and the redundant `test` redefinition are gone.

### 3. `rs/crates/libs/paigasus-proto/moon.yml`

The consumer of generated code owns the `contracts:generate` dependency. These deps merge
onto the inherited commands via Moon's default `append` mergeDeps.

```yaml
tasks:
  build:         { deps: ['contracts:generate'] }
  build-release: { deps: ['contracts:generate'] }
  test:          { deps: ['contracts:generate'] }
```

### 4. `rs/crates/services/paigasus-gateway/moon.yml`

Reconcile the live service to match the slimmed template (adopt Model B): add project deps,
no task overrides. It inherits `build` (with `^:build` → builds `paigasus-proto-rs` →
runs `contracts:generate`).

```yaml
dependsOn:
  - 'paigasus-proto-rs'
  - 'paigasus-kernel-rs'
```

## Out of scope

- `paigasus-py-bindings` dependency wiring (separate concern).
- `lint`/`fmt` gaining the `contracts:generate` dep — generated code is committed
  (ADR-0004), so clippy reads it as-is; ordering only matters for the `:build`/`:test`
  graph addressed here. Deeper affected-rebuild granularity is SMA-401 / SMA-389 territory.
- `clean: true` in `buf.gen.yaml` (tracked under SMA-389 / codegen-drift).

## Assumptions & risks

- **Moon `mergeDeps` defaults to `append`** so partial `deps:`-only overrides keep the
  inherited command. Confirm during implementation (`moon project paigasus-proto-rs`
  should show the merged command + appended dep). If a project somewhere sets
  `mergeDeps: replace`, the proto overrides would need the full command restated.
- **`buf generate` must be runnable wherever `paigasus-proto-rs:build` runs when
  affected** (it uses remote buf plugins → network). Because generated code is committed,
  builds that don't touch protos won't trigger generation, so the common path is
  unaffected.

## Verification

- `moon ci :build` → inspect `rs/target/`: only `debug/`, **no** `release/`
  (uniform debug profile across all crates).
- `moon run :build-release` → `rs/target/release/` appears.
- `moon project paigasus-proto-rs` (or the task graph) shows `build` depends on
  `contracts:generate`; `paigasus-gateway-rs` `build` depends on `^:build`.
- Template smoke test: generate a throwaway service, confirm its `moon.yml` carries
  `dependsOn` only and **no** per-crate tasks; discard.

Implementation note: moon/buf are proto-managed and off the Bash tool PATH — export
`~/.proto/bin:~/.proto/shims`. No macOS `timeout` available.
