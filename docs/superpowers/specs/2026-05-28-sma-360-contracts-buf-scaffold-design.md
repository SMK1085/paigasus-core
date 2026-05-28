# SMA-360 — Bootstrap `contracts/` proto workspace with buf scaffold

**Status:** approved design
**Linear:** [SMA-360](https://linear.app/smaschek/issue/SMA-360/bootstrap-contracts-proto-workspace-with-buf-scaffold)
**Date:** 2026-05-28
**ADR:** ADR-0004 (Protobuf + buf as the single source of truth for wire contracts)

## Goal

Stand up the `contracts/` proto workspace with a buf scaffold so future
contracts work has a place to land. **No `.proto` schemas are defined yet** —
this issue establishes the directory layout and tooling only. Generated code
will be committed (not gitignored); the codegen-drift nightly CI comes later.

## Decisions resolved during brainstorming

The acceptance criteria (written 2026-05-26) drifted from the scaffold that has
since landed. The following decisions reconcile them:

1. **Rust `paigasus-proto` crate is scaffolded in this issue.** The AC routed
   prost/tonic output at a crate that did not exist (`rs/crates/libs/` held only
   `paigasus-kernel`). The Py and TS proto packages already exist, so we create
   the Rust one here for symmetry. The Rust task template already references
   `paigasus-proto-rs` and `contracts:generate`, confirming both ids.
2. **TS output path corrected** from the AC's `ts/packages/proto/...` to the
   real package `ts/packages/paigasus-proto/src/generated`.
3. **buf is pinned via proto**, using a **vendored** TOML plugin schema (see §2)
   rather than the unmaintained community URL.
4. **buf file layout: config at `contracts/` root** (not inside `proto/`),
   resolving the AC's internal inconsistency (`modules: [{ path: proto }]` vs.
   `../../` output paths — incompatible if co-located). Config at the root makes
   `module path: proto` and `out: ../…` both correct.

## Layout

```
contracts/
  buf.yaml            # v2: modules, deps, lint, breaking rules
  buf.gen.yaml        # v2: 4 codegen plugins → ../{rs,py,ts}
  buf.lock            # generated; pins googleapis commit
  moon.yml            # id: contracts; tasks: generate/lint/breaking/format
  README.md           # status line updated
  proto/
    paigasus/common/v1/.gitkeep
    paigasus/gateway/v1/.gitkeep
```

moon runs buf with the working directory at `contracts/`, so both
`module path: proto` and the `out: ../…` paths resolve correctly.

## 1. buf tooling — pinned via proto, vendored plugin

- Root `.prototools` gains:
  ```toml
  buf = "1.70.0"

  [plugins]
  buf = "file://./.proto/plugins/buf.toml"
  ```
- New `.proto/plugins/buf.toml` — the ~25-line TOML schema vendored from the
  community repo (`stk0vrfl0w/proto-toml-plugins`, MIT), with an attribution
  comment recording the source URL + license. It resolves versions from buf's
  git tags and downloads the official `bufbuild/buf` GitHub release binaries with
  SHA-256 checksum verification. **Rationale:** the community repo is
  effectively unmaintained (1 star, single hobbyist maintainer, sporadic commits
  for unrelated tools); referencing its `https://` URL would fetch a mutable
  file from an unmaintained source at every `proto install`. Vendoring the trivial
  schema gives identical behavior (official, checksummed binaries) with no
  external runtime dependency.
- **Known limitation:** the schema's `aarch64 → arm64` arch remap is correct for
  macOS-arm64 (dev) and Linux-x86_64 (CI), but Linux-aarch64 would resolve to a
  wrong asset name (`buf-Linux-arm64` vs. actual `buf-Linux-aarch64`). Recorded
  as a TODO in the vendored file; out of scope until/unless Linux-ARM CI is added.
- `CONTRIBUTING.md`: note that `proto install` now provides buf.

## 2. `contracts/buf.yaml`

```yaml
version: v2
modules:
  - path: proto
deps:
  - buf.build/googleapis/googleapis
lint:
  use:
    - STANDARD
  except:
    - PACKAGE_DIRECTORY_MATCH
breaking:
  use:
    - FILE
```

`buf.lock` is generated via `buf dep update` (network to BSR, run once locally)
and committed.

## 3. `contracts/buf.gen.yaml`

Four plugins; `out` paths relative to `contracts/`:

| plugin | out |
|---|---|
| `buf.build/community/neoeinstein-prost` | `../rs/crates/libs/paigasus-proto/src/generated` |
| `buf.build/community/neoeinstein-tonic` | `../rs/crates/libs/paigasus-proto/src/generated` |
| `buf.build/community/danielgtaylor-betterproto` | `../py/packages/paigasus-proto/src/paigasus_proto/generated` |
| `buf.build/bufbuild/es` (opt `target=ts`) | `../ts/packages/paigasus-proto/src/generated` |

## 4. `contracts/moon.yml`

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'contracts'
layer: 'tool'
```

System-toolchain shell tasks (buf provided on PATH by proto):

- `lint` → `buf lint`
- `format` → `buf format --exit-code` (CI-safe check mode)
- `breaking` → `buf breaking --against '.git#branch=main,subdir=contracts'`
- `generate` → `buf generate`

All four run cleanly on the empty workspace. The id must be exactly `contracts`
because the Rust task template already references `contracts:generate`.

## 5. Rust `paigasus-proto-rs` crate (scaffolded here)

`rs/crates/libs/paigasus-proto/`, mirroring `paigasus-kernel`:

- `Cargo.toml`: `name = "paigasus-proto"`, `version = "0.0.0"`, workspace-inherited
  `edition`/`license`/`rust-version`/`authors`, `publish = false` (with a TODO to
  flip once generated code lands), `[lints] workspace = true`. **No prost/tonic
  deps yet** — added by the first real consumer when protos land, per the
  workspace's stated minimal-baseline philosophy.
- `src/lib.rs`: SPDX header + doc comment, **no module declarations** (an empty
  `generated/` would fail to compile if declared).
- `src/generated/.gitkeep`
- `moon.yml`: `id: paigasus-proto-rs`, `layer: library`, `language: rust`.
- Auto-included via the workspace `members = ["crates/*/*"]` glob — no edit to
  `rs/Cargo.toml`.

## 6. Generated-output stubs (all three languages)

`.gitkeep` placeholder in each `generated/` directory:

- `rs/crates/libs/paigasus-proto/src/generated/.gitkeep`
- `py/packages/paigasus-proto/src/paigasus_proto/generated/.gitkeep`
- `ts/packages/paigasus-proto/src/generated/.gitkeep`

Generated code will be committed (not gitignored). The `codegen-drift.yml`
nightly comes in a later issue.

## 7. SPDX headers

Per the SMA-383 config-file carve-out, `.yaml`/`.toml`/`.gitkeep` files get no
SPDX header. Only `src/lib.rs` carries `// SPDX-License-Identifier: Apache-2.0`.
The vendored `buf.toml` carries an attribution comment (source + MIT) instead.

## Verification (maps to acceptance criteria)

1. `proto install` provides buf 1.70.0.
2. `moon run contracts:lint` runs cleanly.
3. `buf lint` passes on the empty workspace.
4. `cargo build -p paigasus-proto` compiles clean (empty lib).
5. `buf generate` produces no output (no protos) without erroring.
6. `contracts/proto/paigasus/{common,gateway}/v1/` directories exist.
7. All four `generated/` stub dirs exist in their language workspaces.

## Out of scope

- Any actual `.proto` definitions (post-MVP).
- `codegen-drift.yml` nightly CI.
- prost/tonic Rust dependencies (added with the first real protos).
- Linux-aarch64 buf binary resolution.
