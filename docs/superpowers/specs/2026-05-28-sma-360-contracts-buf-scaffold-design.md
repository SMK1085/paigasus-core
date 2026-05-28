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
5. **`buf.gen.yaml` carries the canonical opt set verbatim** (see §3); the
   reduced table in an earlier draft was a defect. The one deferral is
   `clean: true` (see §3).
6. **`layer: 'tool'`** for `contracts`, with CONTRIBUTING's active-set list
   extended to include `tool` in the same PR (see §4) — a deliberate convention
   extension, not silent drift.

### Reconciliation with the staff-engineer design review (2026-05-28)

This spec incorporates a staff-engineer review of the original draft. The
findings (referenced below by their `H#`/`M#`/`L#` labels) and their
disposition:

- **H1** (dropped plugin opts) — accepted; full opt set restored in §3.
- **H2** (`clean: true`) — accepted; deferred with an explicit policy in §3.
- **H3** (orphan `contracts:generate`) — accepted as a *documented deferral*
  rather than wiring now: with zero protos, `generate` is a no-op, and adding a
  build dep would force `buf` onto PATH for every proto build for no current
  benefit. Edges named in §8 and tracked by **SMA-389**.
- **M1** (stale Notion §2) — resolved by updating Notion §2 directly to the
  as-built config (no tracking issue needed).
- **M2** / **M3** — accepted as documented caveats (§9).
- **M4** (`layer`) — `tool` + CONTRIBUTING extension (§4).
- **L1** → **SMA-387**, **L2** → **SMA-388**, **L3** + nits — §3/§9.

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
  as a TODO in the vendored file and tracked by **SMA-387**; out of scope here
  (dev = macOS-arm64, CI = Linux-x86_64).
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
and committed. Note: `googleapis` is a pinned external dep that **nothing imports
yet** (no protos); add a comment so it isn't mistaken for a live import. It is
declared now so the lockfile and lint posture are in place before the first
proto that uses `google.protobuf.*` / `google.api.*` types lands.

## 3. `contracts/buf.gen.yaml`

The **full canonical opt set** from Notion §2, with `out` paths rebased to
`contracts/` (`../` not `../../`) and the TS path corrected to the real
`paigasus-proto` package:

```yaml
version: v2
# clean: true is intentionally OMITTED while the workspace is empty — it would
# wipe the generated/ dirs and delete the .gitkeep stubs (§6) on every generate.
# Add `clean: true` in the same PR that lands the first protos and removes the
# stubs (alongside SMA-389 / the codegen-drift work).

plugins:
  - remote: buf.build/community/neoeinstein-prost
    out: ../rs/crates/libs/paigasus-proto/src/generated
    opt:
      - bytes=.
      - file_descriptor_set
  - remote: buf.build/community/neoeinstein-tonic
    out: ../rs/crates/libs/paigasus-proto/src/generated
    opt:
      - no_include                # prost + tonic write to the SAME dir; without
      - compile_well_known_types  # this, tonic include scaffolding collides
  # betterproto2 is pre-stable (0.x) per ADR-0004; conservative fallback is
  # grpcio-tools + mypy-protobuf if it stalls. No opts (matches §2).
  - remote: buf.build/community/danielgtaylor-betterproto
    out: ../py/packages/paigasus-proto/src/paigasus_proto/generated
  - remote: buf.build/bufbuild/es
    out: ../ts/packages/paigasus-proto/src/generated
    opt:
      - target=ts
      - import_extension=.js      # runtime-correct ESM specifiers once
                                  # @paigasus/proto emits real dist (NodeNext)
```

The opts are not cosmetic: `tonic: no_include` prevents a hard build break when
prost and tonic generate into the same `src/generated`; `prost: bytes=.` is a
pre-generation API decision (`Bytes` vs `Vec<u8>`) that can't be retrofitted
without a breaking change; `es: import_extension=.js` is latent today
(tsconfig `moduleResolution: bundler`) but required once `@paigasus/proto`
flips to `dist/index.js` + `private: false`.

## 4. `contracts/moon.yml`

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'contracts'
layer: 'tool'
```

`layer: 'tool'` is the best Moon semantic fit for a codegen project but is not
in CONTRIBUTING's currently-sanctioned set (`library`/`application`/
`configuration`). **This PR also adds `tool` to that list in CONTRIBUTING**
(extending the SMA-383 convention deliberately, rather than drifting from it).

System-toolchain shell tasks (buf provided on PATH by proto):

- `lint` → `buf lint`
- `format` → `buf format --exit-code` (CI-safe check mode)
- `breaking` → `buf breaking --against '.git#branch=main,subdir=contracts'`
- `generate` → `buf generate`

The id must be exactly `contracts` because the Rust task template already
references `contracts:generate`. `lint`/`format`/`generate` run cleanly on the
empty workspace. **`breaking` is effectively a no-op at bootstrap** — on the PR
that introduces `contracts/`, `main` has no `buf.yaml`/module baseline, so buf
has nothing to compare against (M2). Confirm buf's missing-baseline behavior
(no-op vs. error) before wiring `breaking` into `moon ci`; the AC only verifies
`lint`.

## 5. Rust `paigasus-proto-rs` crate (scaffolded here)

`rs/crates/libs/paigasus-proto/`, mirroring `paigasus-kernel`:

- `Cargo.toml`: `name = "paigasus-proto"`, `version = "0.0.0"`, workspace-inherited
  `edition`/`license`/`rust-version`/`authors`, `publish = false` with a
  `TODO(SMA-388)` to flip once generated code lands (mirroring the kernel's
  `TODO(SMA-376)` style), `[lints] workspace = true`. **No prost/tonic
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
nightly comes in a later issue. Note: the Python `generated/` dir is not an
importable subpackage until betterproto emits an `__init__.py`; confirm the
generator emits package markers rather than relying on namespace-package
behavior when the first protos land.

## 7. SPDX headers

Per the SMA-383 config-file carve-out, `.yaml`/`.toml`/`.gitkeep` files get no
SPDX header. Only `src/lib.rs` carries `// SPDX-License-Identifier: Apache-2.0`.
The vendored `buf.toml` carries an attribution comment (source + MIT) instead.

## 8. Deferred build-graph wiring (H3 — tracked by SMA-389)

The proto→downstream affected graph (touch proto → `contracts:generate` →
`paigasus-proto:build` → downstream rebuilds) is the headline win of the
monorepo, but it requires the proto packages' build to *depend on*
`contracts:generate`. None of the three proto packages establish that edge today:

- the rust template emits `deps: ['contracts:generate', '^:build']` only for the
  **service** archetype; `paigasus-proto-rs` (library) inherits no such edge;
- `py`/`ts` proto `moon.yml` are bare.

`paigasus-proto-rs` is the special case — its source *is* the generated code, so
depending on `contracts:generate` is correct and creates no cycle.

**Decision: defer, don't wire now.** With zero protos, `generate` is a no-op and
adding the dep would force `buf` onto PATH for every `paigasus-proto-rs:build`
for no benefit. The exact edges to add (`paigasus-proto-rs:build`/`:test` →
`contracts:generate`, plus the py/ts equivalents) are captured in **SMA-389**,
to land with the first real protos.

## 9. Known caveats

- **CI PATH for `buf` (M3):** `buf` is a *proto* plugin, not a Moon toolchain, so
  Moon won't inject it onto a task's PATH the way it does for managed Rust/Node/
  Python. The `contracts` tasks run under the **system** toolchain and rely on
  proto's shim dir being on PATH. This holds locally after `proto install` but is
  unproven in CI (`.github/workflows/` is currently a `.gitkeep`). When `ci.yml`
  lands it must activate proto's shims *before* `moon ci`. Verification below adds
  a clean-environment check.
- **`breaking` baseline (M2):** see §4 — no-op until `main` carries a contracts
  baseline.

## Verification (maps to acceptance criteria)

1. `proto install` provides buf 1.70.0.
2. `moon run contracts:lint` runs cleanly — **also verify `buf` resolves in a
   clean, shell-rc-free shell** (proxy for CI), not just an interactive shell.
3. `buf lint` passes on the empty workspace.
4. `cargo build -p paigasus-proto` compiles clean (empty lib).
5. `buf generate` produces no output (no protos) without erroring **and does not
   delete the `.gitkeep` stubs** (confirms `clean` is correctly omitted).
6. `contracts/proto/paigasus/{common,gateway}/v1/` directories exist.
7. All four `generated/` stub dirs exist in their language workspaces.

## Out of scope

- Any actual `.proto` definitions (post-MVP).
- `codegen-drift.yml` nightly CI, and re-introducing `clean: true` (with first protos).
- prost/tonic Rust dependencies (added with the first real protos).
- Build-graph dependency edges on `contracts:generate` → **SMA-389**.
- Linux-aarch64 buf binary resolution → **SMA-387**.
- Flipping `paigasus-proto` `publish` → **SMA-388**.

## Follow-ups created from the design review

- **SMA-387** — fix Linux-aarch64 buf asset resolution (L1).
- **SMA-388** — flip `paigasus-proto` `publish=false` once codegen lands (L2).
- **SMA-389** — wire `contracts:generate` build-graph edges with first protos (H3).
- Notion §2 (Polyglot Monorepo Scoping) updated to the as-built config (M1).
