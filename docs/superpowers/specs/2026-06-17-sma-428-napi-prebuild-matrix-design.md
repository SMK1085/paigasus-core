# SMA-428 — napi-rs cross-platform `.node` prebuild matrix (infra-only, publish deferred)

**Status:** approved design (brainstorm complete, ready for plan)
**Linear:** [SMA-428](https://linear.app/smaschek/issue/SMA-428/napi-rs-cross-platform-node-prebuild-matrix-npm-publish-for)
**Date:** 2026-06-17
**ADR:** ADR-0005 (kernel-once — pure Rust kernel bound to Py/Node/WASM), ADR-0006 (open-core boundary / publish discipline), ADR-0010/0011 (release tooling + strategy)
**Follow-up of:** [SMA-420](https://linear.app/smaschek/issue/SMA-420/stand-up-a-ts-kernel-binding-wasmnapi-wire-the-cascade-to-paigasus) — stood up the napi binding for a **single host** (macOS arm64) only; deferred the cross-platform prebuild matrix + npm publish here.
**Related:** SMA-407 (release activation — owns the actual publish), SMA-419 (the py-wheel sibling deferral), SMA-376 (kernel publish), SMA-434 (CI drift check for committed FFI glue).

## Goal

Build and **verify** a cross-platform `.node` prebuild pipeline for `@paigasus/node-bindings` —
the build matrix, per-platform packages, `optionalDependencies` wiring, and npm metadata —
**up to but not including `npm publish`**. Both `@paigasus/node-bindings` and `@paigasus/kernel`
stay `private: true` / `version: 0.0.0`. The `private:false` flip, the real version, the
kernel/proto lockstep, and the live release-plz workflow all remain with **SMA-407** (release
activation). This is the napi sibling of the deferred py-wheel publish (SMA-419 → SMA-407): land
and prove the machinery while it's dormant, so activation is a clean flip.

The single-host build that drives local `moon` build/test (SMA-420) is **untouched**; the matrix
is a separate, CI-only concern.

## Decisions resolved during brainstorming

1. **Infra only; publish deferred (scope boundary vs SMA-407).** SMA-428 builds + verifies the
   prebuild/packaging pipeline but does **not** publish. Both packages stay `private: true` /
   `0.0.0`. The version flip (`0.0.0 → 0.1.0` floor), kernel/proto lockstep versioning, and
   turning on the dormant release-plz workflow are SMA-407's deliberate, risk-managed step
   (ADR-0011 S3 warns against hand-placing the first tag — the SMA-385 Helikon trap). Mirrors how
   SMA-398 landed dormant release config → SMA-407 activates it, and how SMA-419 deferred the
   py-wheel publish to SMA-407.
2. **Dedicated `prebuild.yml`, on `workflow_dispatch` + push-to-main, uploading artifacts.** A
   cross-platform `.node` matrix is inherently multi-OS, so it cannot live in the single
   `ubuntu-latest` `moon ci` job. It runs on manual dispatch (verify) and on push-to-main (catch
   breakage before activation), **not** on every PR — keeps PR CI fast on a placeholder kernel.
   The workflow is decoupled from `moon ci` and the affected-graph model (which is single-host).
3. **Verify = build matrix + dry-run assembly.** All 7 targets build and upload their `.node`; a
   final job runs `napi prepublish --dry-run` + `npm pack --dry-run` to assert the exact publish
   artifact shape (os/cpu/libc fields, `main` paths, `optionalDependencies` resolution) **without
   pushing to npm**. SMA-407 inherits a verified pipeline.
4. **`@paigasus/node-bindings`-focused; `@paigasus/kernel` gets metadata only.** The matrix /
   packaging / dry-run work is entirely a `@paigasus/node-bindings` concern (it is the
   host-coupled native package). `@paigasus/kernel` is pure TS glue whose `exports` point at
   **source** (`./src/*.ts`) with no `dist` build (tsup deferred by SMA-420) and `file:` deps on
   `@paigasus/node-bindings` + `@paigasus/wasm` — so it is **double-blocked** from real packaging
   (needs tsup/dist **and** version activation). SMA-428 only adds the static npm metadata it can
   have now, plus a breadcrumb comment. No tsup/dist work here.
5. **Native runners per target + official napi-rs Alpine Docker images for musl.** Each target
   builds on its matching native-arch GitHub runner (GitHub's free `ubuntu-24.04-arm` removes the
   need to cross-compile arm64); only the two musl targets swap in the official `napi-rs` Alpine
   container. This is the canonical `@napi-rs/cli` scaffold shape — battle-tested and
   copy-adaptable — and avoids zig cross-compilation's sharp edges (Windows-MSVC, macOS SDK).

## 1. Target matrix (7)

| napi platform     | Rust triple                  | Runner / method                                   |
| ----------------- | ---------------------------- | ------------------------------------------------- |
| `darwin-x64`      | `x86_64-apple-darwin`        | `macos-13` (Intel)                                |
| `darwin-arm64`    | `aarch64-apple-darwin`       | `macos-latest`                                    |
| `win32-x64-msvc`  | `x86_64-pc-windows-msvc`     | `windows-latest`                                  |
| `linux-x64-gnu`   | `x86_64-unknown-linux-gnu`   | `ubuntu-latest`                                   |
| `linux-arm64-gnu` | `aarch64-unknown-linux-gnu`  | `ubuntu-24.04-arm`                                |
| `linux-x64-musl`  | `x86_64-unknown-linux-musl`  | `ubuntu-latest` + napi-rs Alpine image            |
| `linux-arm64-musl`| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` + napi-rs Alpine image         |

Every leg builds on its **native arch** (no cross-compiling for glibc/musl arm); musl just runs
the build inside the official Alpine container. `macos-13` is the Intel fallback path if a leg
cannot run on a native Intel runner — see §6 risk on macos-13 retirement.

## 2. New workflow — `.github/workflows/prebuild.yml`

Decoupled from `moon ci` (single-host, affected-graph-bound). Tooling is still pinned via
`moonrepo/setup-toolchain` + `proto install` so node/pnpm/rust match `.prototools` /
`rs/rust-toolchain.toml`; napi is then invoked directly (not through Moon).

- **Triggers:** `workflow_dispatch` and `push: branches: [main]`.
- **Permissions:** `contents: read` only — no publish creds. SMA-407 adds registry auth /
  `id-token` when it turns publish on.
- **`build` job** — `strategy.matrix` over the 7 targets `{ platform, target, runner, useContainer }`:
  1. checkout
  2. `moonrepo/setup-toolchain` + `proto install` (pinned node/pnpm)
  3. `rustup target add <triple>` against the pinned **1.95.0** toolchain (run from `rs/` so the
     `rust-toolchain.toml` override applies — see §6 toolchain-pin risk)
  4. `pnpm --dir ts install --frozen-lockfile`
  5. `pnpm exec napi build --platform --release --target <triple>` in
     `rs/crates/bindings/paigasus-node-bindings` (`--platform` emits the platform-suffixed
     `paigasus-node-bindings.<platform>.node` filename)
  6. upload `paigasus-node-bindings.<platform>.node` as a CI artifact
  - musl legs run steps 3–5 inside the official napi-rs Alpine container.
- **`assemble` job** (`needs: build`):
  1. download all build artifacts
  2. `napi artifacts` — sort each downloaded `.node` into its **committed** `npm/<platform>/` dir
     (the dirs themselves are committed scaffolds, §4 — `create-npm-dirs` is the authoring-time
     step that produced them, not a CI step; re-generating them in CI is SMA-434's drift concern,
     not this job's)
  3. `napi prepublish --dry-run` + `npm pack --dry-run` on the main + per-platform packages —
     assert os/cpu/libc, `main` paths, and `optionalDependencies` all resolve
  - **No `npm publish` anywhere.** Both packages stay `private: true` / `0.0.0`.

## 3. `rs/crates/bindings/paigasus-node-bindings/package.json`

- Extend the `napi` block with `targets` = the 7 triples (so `create-npm-dirs` / `prepublish`
  know the full set).
- Add `optionalDependencies`: the 7 `@paigasus/node-bindings-<platform>` entries pinned to
  `0.0.0` — committed explicitly so the structure is reviewable + drift-checkable (SMA-434),
  rather than materialized only at publish time.
- Add npm metadata: `repository`, `homepage`, `keywords`, `description`, `engines.node`,
  `publishConfig.access: public`. Keep `private: true` / `version: 0.0.0`.
- **Fix `files`:** drop `*.node` from `files` (currently `["index.js", "index.d.ts", "*.node"]`).
  In the optionalDependencies model the main package ships **only** the loader glue; the `.node`
  binaries ship in the per-platform packages. Leaving `*.node` in would wrongly bundle a
  locally-built host `.node` into the main tarball — the `npm pack --dry-run` in §2 surfaces this.

## 4. Per-platform package scaffolds — `rs/crates/bindings/paigasus-node-bindings/npm/<platform>/package.json` (new ×7)

Committed as `napi create-npm-dirs` emits them:

- name `@paigasus/node-bindings-<platform>`, `version: 0.0.0`, `license: Apache-2.0`
- `os` / `cpu` (and `libc` for musl) constraints so npm resolves the right prebuild on install
- `main` → `paigasus-node-bindings.<platform>.node`, `files: ["*.node"]`

The built `.node` lands in these dirs at CI time via `napi artifacts` and is **gitignored** (the
existing `.gitignore` already ignores `*.node`). Only the `package.json` scaffolds are committed.

## 5. `ts/packages/paigasus-kernel/package.json` (metadata only)

- Add static npm metadata: `repository`, `keywords`, `description`, `publishConfig`. Keep
  `private: true` / `version: 0.0.0`; **no** `exports` change, **no** tsup/dist.
- Extend the existing `_comment_exports` breadcrumb (or add a sibling `_comment`) noting that
  publish is double-blocked: (a) `exports` point at source — needs tsup/dist (SMA-420 deferral),
  and (b) version activation lives in SMA-407.

## 6. `moon.yml` — unchanged

`rs/crates/bindings/paigasus-node-bindings/moon.yml` and
`ts/packages/paigasus-kernel/moon.yml` are **not** touched. The local single-host build/test
chain (`paigasus-kernel-ts:build`/`:test` running `napi build --platform` for the dev host) is
unchanged, so local dev and the existing `moon ci` are unaffected. The matrix is a separate
CI-only workflow.

## Primary risks → de-risk first (spike before the workflow)

1. **Toolchain pin on cross legs.** `rs/rust-toolchain.toml` pins **1.95.0**; each matrix leg must
   `rustup target add <triple>` against *that* toolchain (run from `rs/`), and the musl Alpine
   image must use the pin — or we accept the image's default Rust with a written rationale. This
   is the napi analog of the SMA-427 wasm-pack toolchain trap (wasm-pack resolved the wrong
   rustup toolchain when invoked from the wrong cwd).
2. **`@napi-rs/cli` v3 command + schema surface.** Confirm the exact v3 subcommands
   (`create-npm-dirs`, `artifacts`, `prepublish --dry-run`) and the `napi.targets` package.json
   schema against the pinned `^3` — v3 renamed some v2 commands.
3. **`macos-13` Intel runner availability.** If retired, build `darwin-x64` via
   `--target x86_64-apple-darwin` on `macos-latest` instead of a native Intel runner.

## Verification (maps to acceptance criteria)

1. **Matrix build** — `prebuild.yml` dispatched: all 7 build legs green, each uploads its
   `paigasus-node-bindings.<platform>.node` artifact.
2. **Dry-run assembly** — the `assemble` job's `napi prepublish --dry-run` + `npm pack --dry-run`
   succeed and show: a loader-only main package (no `.node`), exactly one `.node` per platform
   package, correct `os`/`cpu`/`libc`, and 7 `optionalDependencies`.
3. **No publish / no state change** — no `npm publish` runs; both packages remain
   `private: true` / `0.0.0`.
4. **No regression** — existing `moon ci` stays green and unchanged; local
   `moon run paigasus-kernel-ts:build`/`:test` still work.

## Out of scope (deferred, with owners)

- **Actual publish** — `private: false`, version off `0.0.0`, kernel/proto lockstep versioning,
  and the live release-plz workflow → **SMA-407** (ADR-0011).
- **tsup/dist for `@paigasus/kernel`** — which also unblocks `@paigasus/kernel` + `@paigasus/wasm`
  publish → SMA-420 deferral / its own issue.
- **maturin py-wheel matrix** (manylinux/musllinux/macos/windows) — the py sibling of this work →
  SMA-419 / SMA-407.
- **CI drift check for committed FFI glue** (`index.js` + the new `npm/<platform>/` scaffolds) →
  **SMA-434**.
- **Per-OS real install/import smoke** (pack tarballs, install into a scratch project on each OS,
  `import { sum }`) — considered and declined for a placeholder kernel; dry-run assembly is the
  chosen fidelity level.
- **Real kernel domain logic** — `sum` remains a deliberate placeholder.
