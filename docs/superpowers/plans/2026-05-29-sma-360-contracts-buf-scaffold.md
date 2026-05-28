# SMA-360 — Bootstrap `contracts/` proto workspace with buf scaffold — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the `contracts/` buf workspace and the Rust `paigasus-proto` crate so future proto work has a place to land — no `.proto` schemas yet.

**Architecture:** buf config (`buf.yaml` + `buf.gen.yaml` + `buf.lock`) lives at `contracts/` root; sources go under `contracts/proto/`. `buf` is pinned via a **vendored** proto TOML plugin. Four codegen plugins target committed `generated/` dirs in the rs/py/ts workspaces (empty `.gitkeep` stubs for now). A new `paigasus-proto` Rust crate mirrors `paigasus-kernel`. Moon drives `lint`/`format`/`breaking`/`generate` as system-toolchain shell tasks.

**Tech Stack:** Moon 2.2.5, proto (moonrepo), buf 1.70.0, Cargo (edition 2024 / Rust 1.95).

**Source spec:** `docs/superpowers/specs/2026-05-28-sma-360-contracts-buf-scaffold-design.md`

**Prerequisite for every verification step:** `buf` and `moon` are provided by `proto`'s shim dir. Run `proto install` first, and ensure `~/.proto/shims` and `~/.proto/bin` are on `PATH` (a fresh shell after `proto install`, or `eval "$(proto activate zsh)"`). Commands below assume this is done.

---

## File structure (what gets created/modified)

**Created:**
- `.proto/plugins/buf.toml` — vendored proto schema plugin for the buf CLI
- `contracts/buf.yaml` — buf v2 workspace config (modules, deps, lint, breaking)
- `contracts/buf.gen.yaml` — buf v2 codegen config (4 plugins)
- `contracts/buf.lock` — generated dep lockfile (pins googleapis)
- `contracts/moon.yml` — `contracts` project + 4 tasks
- `contracts/proto/paigasus/common/v1/.gitkeep`
- `contracts/proto/paigasus/gateway/v1/.gitkeep`
- `rs/crates/libs/paigasus-proto/Cargo.toml`
- `rs/crates/libs/paigasus-proto/src/lib.rs`
- `rs/crates/libs/paigasus-proto/src/generated/.gitkeep`
- `rs/crates/libs/paigasus-proto/moon.yml`
- `py/packages/paigasus-proto/src/paigasus_proto/generated/.gitkeep`
- `ts/packages/paigasus-proto/src/generated/.gitkeep`

**Modified:**
- `.prototools` — pin `buf` + register the vendored plugin
- `CONTRIBUTING.md` — add `tool` to the sanctioned `layer:` set
- `contracts/README.md` — update status line

---

## Task 1: Vendor the buf proto plugin and pin it

**Files:**
- Create: `.proto/plugins/buf.toml`
- Modify: `.prototools`

- [ ] **Step 1: Create the vendored plugin schema**

Create `.proto/plugins/buf.toml`:

```toml
# Vendored proto TOML plugin for the buf CLI.
#
# Source: https://github.com/stk0vrfl0w/proto-toml-plugins (plugins/buf.toml, MIT).
# Vendored in SMA-360 rather than referenced by URL: the upstream repo is
# effectively unmaintained, and this schema only resolves official, checksummed
# `bufbuild/buf` GitHub release binaries — nothing to maintain, so we own it.
#
# TODO(SMA-387): the `aarch64 = "arm64"` remap below is correct for macOS-arm64
# and Linux-x86_64 but resolves Linux-aarch64 to a non-existent asset
# (buf-Linux-arm64 vs. real buf-Linux-aarch64). Fix before adding Linux-arm CI.

name = "buf"
type = "cli"

[platform.linux]
download-file = "buf-Linux-{arch}"
checksum-file = "sha256.txt"

[platform.macos]
download-file = "buf-Darwin-{arch}"
checksum-file = "sha256.txt"

[platform.windows]
download-file = "buf-Windows-{arch}.exe"
checksum-file = "sha256.txt"

[install]
checksum-url = "https://github.com/bufbuild/buf/releases/download/v{version}/{checksum_file}"
download-url = "https://github.com/bufbuild/buf/releases/download/v{version}/{download_file}"

[install.arch]
aarch64 = "arm64"

[resolve]
git-url = "https://github.com/bufbuild/buf"
```

- [ ] **Step 2: Pin buf and register the plugin in `.prototools`**

The file currently contains only `moon = "2.2.5"`. Replace its full contents with:

```toml
buf = "1.70.0"
moon = "2.2.5"

[plugins]
buf = "file://./.proto/plugins/buf.toml"
```

- [ ] **Step 3: Install and verify (this is the test)**

Run: `proto install`
Then run: `proto run buf -- --version`
Expected: prints `1.70.0`

If `buf` is not yet on `PATH` as a bare command, run `eval "$(proto activate zsh)"` (or open a fresh shell) and confirm: `buf --version` → `1.70.0`.

- [ ] **Step 4: Commit**

```bash
git add .proto/plugins/buf.toml .prototools
git commit -m "build(contracts): vendor buf proto plugin and pin buf 1.70.0 (SMA-360)"
```

---

## Task 2: Scaffold `contracts/buf.yaml`, proto dirs, and the dep lock

**Files:**
- Create: `contracts/proto/paigasus/common/v1/.gitkeep`
- Create: `contracts/proto/paigasus/gateway/v1/.gitkeep`
- Create: `contracts/buf.yaml`
- Create: `contracts/buf.lock` (generated)

- [ ] **Step 1: Create the empty proto package dirs**

```bash
mkdir -p contracts/proto/paigasus/common/v1 contracts/proto/paigasus/gateway/v1
touch contracts/proto/paigasus/common/v1/.gitkeep contracts/proto/paigasus/gateway/v1/.gitkeep
```

- [ ] **Step 2: Create `contracts/buf.yaml`**

```yaml
version: v2
modules:
  - path: proto
# googleapis is declared so the lockfile + lint posture are ready before the
# first proto imports google.protobuf.* / google.api.* — nothing imports it yet.
deps:
  - buf.build/googleapis/googleapis
lint:
  use:
    - STANDARD
  except:
    # The directory already encodes the version, so don't also require the
    # package path to match the directory.
    - PACKAGE_DIRECTORY_MATCH
breaking:
  use:
    - FILE
```

- [ ] **Step 3: Generate the dep lockfile (needs network to BSR)**

Run: `cd contracts && buf dep update && cd ..`
Expected: creates `contracts/buf.lock` pinning a `buf.build/googleapis/googleapis` commit. No error.

- [ ] **Step 4: Verify `buf lint` passes on the empty workspace (this is the test)**

Run: `cd contracts && buf lint && cd ..`
Expected: exits 0, no output (no protos to lint).

- [ ] **Step 5: Commit**

```bash
git add contracts/buf.yaml contracts/buf.lock contracts/proto
git commit -m "feat(contracts): add buf.yaml workspace config + empty proto dirs (SMA-360)"
```

---

## Task 3: Create the committed `generated/` stub dirs (all three languages)

**Files:**
- Create: `rs/crates/libs/paigasus-proto/src/generated/.gitkeep`
- Create: `py/packages/paigasus-proto/src/paigasus_proto/generated/.gitkeep`
- Create: `ts/packages/paigasus-proto/src/generated/.gitkeep`

- [ ] **Step 1: Create the stub dirs**

```bash
mkdir -p rs/crates/libs/paigasus-proto/src/generated
mkdir -p py/packages/paigasus-proto/src/paigasus_proto/generated
mkdir -p ts/packages/paigasus-proto/src/generated
touch rs/crates/libs/paigasus-proto/src/generated/.gitkeep
touch py/packages/paigasus-proto/src/paigasus_proto/generated/.gitkeep
touch ts/packages/paigasus-proto/src/generated/.gitkeep
```

- [ ] **Step 2: Verify all three exist (this is the test)**

Run: `ls rs/crates/libs/paigasus-proto/src/generated/.gitkeep py/packages/paigasus-proto/src/paigasus_proto/generated/.gitkeep ts/packages/paigasus-proto/src/generated/.gitkeep`
Expected: all three paths listed, no "No such file" error.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/libs/paigasus-proto/src/generated/.gitkeep \
        py/packages/paigasus-proto/src/paigasus_proto/generated/.gitkeep \
        ts/packages/paigasus-proto/src/generated/.gitkeep
git commit -m "feat(contracts): add committed generated/ stub dirs for rs/py/ts (SMA-360)"
```

---

## Task 4: Add `contracts/buf.gen.yaml` and verify generate is a clean no-op

**Files:**
- Create: `contracts/buf.gen.yaml`

- [ ] **Step 1: Create `contracts/buf.gen.yaml`**

`out` paths are relative to `contracts/` (`../`). `clean: true` is intentionally
omitted — see the comment.

```yaml
version: v2
# NOTE: `clean: true` (from the canonical config) is intentionally OMITTED while
# the workspace is empty — it would delete the generated/ dirs and their .gitkeep
# stubs on every run. Add `clean: true` in the same PR that lands the first protos
# and removes the stubs (tracked with SMA-389 / the codegen-drift work).

plugins:
  # ─── Rust: prost (messages) + tonic (gRPC stubs) ──────────────────────────
  - remote: buf.build/community/neoeinstein-prost
    out: ../rs/crates/libs/paigasus-proto/src/generated
    opt:
      - bytes=.
      - file_descriptor_set
  - remote: buf.build/community/neoeinstein-tonic
    out: ../rs/crates/libs/paigasus-proto/src/generated
    opt:
      # prost + tonic write to the SAME dir; without no_include, tonic's own
      # include scaffolding collides with prost's modules.
      - no_include
      - compile_well_known_types

  # ─── Python: betterproto2 (pre-stable 0.x per ADR-0004; fallback is ─────────
  #     grpcio-tools + mypy-protobuf if it stalls) ──────────────────────────────
  - remote: buf.build/community/danielgtaylor-betterproto
    out: ../py/packages/paigasus-proto/src/paigasus_proto/generated

  # ─── TypeScript: protobuf-es v2 ───────────────────────────────────────────
  - remote: buf.build/bufbuild/es
    out: ../ts/packages/paigasus-proto/src/generated
    opt:
      - target=ts
      # required for runtime-correct ESM specifiers once @paigasus/proto emits
      # real dist under NodeNext resolution.
      - import_extension=.js
```

- [ ] **Step 2: Verify generate is a no-op that preserves the stubs (this is the test)**

Run: `cd contracts && buf generate && cd ..`
Expected: exits 0, writes nothing (no protos).

Then confirm the stubs survived (proves `clean` is correctly omitted):
Run: `ls rs/crates/libs/paigasus-proto/src/generated/.gitkeep py/packages/paigasus-proto/src/paigasus_proto/generated/.gitkeep ts/packages/paigasus-proto/src/generated/.gitkeep`
Expected: all three still present.

Then confirm git sees no new generated files:
Run: `git status --porcelain rs py ts`
Expected: no output (nothing changed).

- [ ] **Step 3: Commit**

```bash
git add contracts/buf.gen.yaml
git commit -m "feat(contracts): add buf.gen.yaml codegen config (SMA-360)"
```

---

## Task 5: Scaffold the Rust `paigasus-proto` crate

**Files:**
- Create: `rs/crates/libs/paigasus-proto/Cargo.toml`
- Create: `rs/crates/libs/paigasus-proto/src/lib.rs`
- Create: `rs/crates/libs/paigasus-proto/moon.yml`

(The crate is auto-included by the workspace `members = ["crates/*/*"]` glob — no edit to `rs/Cargo.toml`. The `src/generated/.gitkeep` already exists from Task 3.)

- [ ] **Step 1: Create `rs/crates/libs/paigasus-proto/Cargo.toml`**

```toml
[package]
name = "paigasus-proto"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
# TODO(SMA-388): flip publish = true + choose a real 0.x version once generated
# code lands (paigasus-proto is crates.io-bound per the open-core release strategy).
publish = false

[lints]
workspace = true
```

- [ ] **Step 2: Create `rs/crates/libs/paigasus-proto/src/lib.rs`**

No module declarations — an empty `generated/` would fail to compile if declared.

```rust
// SPDX-License-Identifier: Apache-2.0

//! Generated protobuf + gRPC bindings for Paigasus (prost + tonic).
//!
//! The source of truth is `contracts/proto`; code is generated by `buf generate`
//! into `src/generated/` and committed (ADR-0004). Empty until the first protos
//! land; the `generated` module is wired up then (alongside SMA-389).
```

- [ ] **Step 3: Create `rs/crates/libs/paigasus-proto/moon.yml`**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-proto-rs'
layer: 'library'
language: 'rust'
```

- [ ] **Step 4: Verify the crate builds, lints, formats, and tests clean (this is the test)**

```bash
cd rs
cargo build -p paigasus-proto
cargo clippy -p paigasus-proto --all-targets -- -D warnings
cargo fmt -p paigasus-proto --check
cargo nextest run -p paigasus-proto --no-tests=pass
cd ..
```

Expected: all four succeed. (`--no-tests=pass` keeps nextest green with zero tests.)

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-proto/Cargo.toml \
        rs/crates/libs/paigasus-proto/src/lib.rs \
        rs/crates/libs/paigasus-proto/moon.yml
git commit -m "feat(rs): scaffold paigasus-proto crate for generated bindings (SMA-360)"
```

---

## Task 6: Add `contracts/moon.yml` with the buf tasks

**Files:**
- Create: `contracts/moon.yml`

- [ ] **Step 1: Create `contracts/moon.yml`**

`contracts` has no language toolchain, so each task runs under the `system`
toolchain (buf is on `PATH` via proto). Field order follows CONTRIBUTING
(`$schema` → `id` → `layer` → `tasks`).

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'contracts'
layer: 'tool'

tasks:
  generate:
    command: 'buf generate'
    toolchain: 'system'
    inputs:
      - 'proto/**/*'
      - 'buf.yaml'
      - 'buf.gen.yaml'
      - 'buf.lock'
  lint:
    command: 'buf lint'
    toolchain: 'system'
    inputs:
      - 'proto/**/*'
      - 'buf.yaml'
  format:
    command: 'buf format --exit-code'
    toolchain: 'system'
    inputs:
      - 'proto/**/*'
      - 'buf.yaml'
  breaking:
    command: "buf breaking --against '.git#branch=main,subdir=contracts'"
    toolchain: 'system'
    inputs:
      - 'proto/**/*'
      - 'buf.yaml'
```

- [ ] **Step 2: Verify `moon run contracts:lint` runs cleanly (this is the test)**

Run: `moon run contracts:lint`
Expected: task succeeds (green). If Moon reports an unknown `toolchain` field, fall back to removing the `toolchain: 'system'` lines (a project with no `language` already defaults to the system toolchain) and re-run.

Also verify generate + format via Moon:
Run: `moon run contracts:generate contracts:format`
Expected: both succeed.

- [ ] **Step 3: Verify buf resolves in a clean, rc-free shell (M3 — CI proxy)**

This proves the tasks don't depend on interactive shell setup. Run:

```bash
env -i HOME="$HOME" PATH="$HOME/.proto/shims:$HOME/.proto/bin:/usr/bin:/bin" buf --version
```

Expected: prints `1.70.0`. (Captures the proto-activation requirement for the future `ci.yml`.)

- [ ] **Step 4: Commit**

```bash
git add contracts/moon.yml
git commit -m "feat(contracts): add moon project with buf generate/lint/format/breaking tasks (SMA-360)"
```

---

## Task 7: Docs — extend the `layer:` set and update statuses

**Files:**
- Modify: `CONTRIBUTING.md`
- Modify: `contracts/README.md`

- [ ] **Step 1: Add `tool` to the sanctioned `layer:` values in `CONTRIBUTING.md`**

Find this text:

```
`configuration` (workspace-root project that aggregates child projects,
e.g. `py/moon.yml`). Moon's full set of seven values is documented in its
```

Replace with:

```
`configuration` (workspace-root project that aggregates child projects,
e.g. `py/moon.yml`), and `tool` (non-language codegen/utility project,
e.g. `contracts`). Moon's full set of seven values is documented in its
```

- [ ] **Step 2: Add a buf line to the toolchain/setup note in `CONTRIBUTING.md`**

In the local-development/toolchain section, add a sentence noting that
`proto install` now also provides the `buf` CLI (pinned in `.prototools` via a
vendored plugin at `.proto/plugins/buf.toml`). Place it next to the existing
`proto install` instruction so contributors know buf comes from proto, not a
separate install.

- [ ] **Step 3: Update `contracts/README.md` status line**

Find:

```
**Status:** scaffolded in SMA-360. Empty until the buf workspace lands.
```

Replace with:

```
**Status:** buf workspace scaffolded (SMA-360) — `buf.yaml`, `buf.gen.yaml`, and
the rs/py/ts `generated/` targets are wired. No `.proto` schemas yet.
```

- [ ] **Step 4: Verify the docs build/read correctly (this is the test)**

Run: `grep -n "tool" CONTRIBUTING.md | grep -i "codegen"` → expects the new line.
Run: `grep -n "buf workspace scaffolded" contracts/README.md` → expects the new status.

- [ ] **Step 5: Commit**

```bash
git add CONTRIBUTING.md contracts/README.md
git commit -m "docs(repo): sanction layer:tool and document buf via proto (SMA-360)"
```

---

## Task 8: Full acceptance sweep

**Files:** none (verification only)

- [ ] **Step 1: Run the affected lint graph end-to-end**

Run: `moon ci :lint`
Expected: succeeds; `contracts:lint` is included and green. (Per CLAUDE.md, Moon 2.x needs an explicit target — `:lint` here, not bare `moon ci`.)

- [ ] **Step 2: Walk the acceptance criteria**

Confirm each maps to reality:
- `contracts/proto/paigasus/{common,gateway}/v1/` exist (Task 2).
- `contracts/buf.yaml` is v2 with `modules`/`deps`/lint-except/breaking (Task 2).
- `contracts/buf.gen.yaml` has all four plugins with the full opt set (Task 4).
- `contracts/moon.yml` has `generate`/`lint`/`breaking`/`format` (Task 6).
- `generated/` stub dirs exist in all three language workspaces (Task 3).
- `buf lint` passes (Task 2); `moon run contracts:lint` runs cleanly (Task 6).

- [ ] **Step 3: Confirm the working tree is clean**

Run: `git status --porcelain`
Expected: no output (everything committed; no stray generated files).

---

## Out of scope (tracked separately — do NOT do here)

- Actual `.proto` definitions, and re-adding `clean: true` → with first protos.
- Wiring `paigasus-proto:build` → `contracts:generate` build-graph edges → **SMA-389**.
- prost/tonic Rust dependencies in the proto crate → with first protos.
- Linux-aarch64 buf asset fix → **SMA-387**.
- Flipping `paigasus-proto` `publish` → **SMA-388**.
- `codegen-drift.yml` nightly CI → later issue.
