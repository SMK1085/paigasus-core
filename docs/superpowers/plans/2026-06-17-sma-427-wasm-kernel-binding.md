# SMA-427 — wasm kernel binding (`paigasus-wasm`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the `paigasus-wasm` (wasm-bindgen) binding so `@paigasus/kernel`'s browser/Edge export condition serves a real wasm `sum`, proven by a browser-condition vitest round-trip, with the Moon cascade + affected-graph guard extended.

**Architecture:** A co-located `cdylib` crate wraps `paigasus_kernel::sum` via `#[wasm_bindgen]`, built with wasm-pack (`--target bundler`, proto-pinned) into committed JS/`.d.ts` glue + a gitignored `.wasm`. `@paigasus/kernel` gains a `file:`-linked `@paigasus/wasm` dependency and routes `browser`/`default` exports to it (node stays napi). It mirrors the SMA-420 napi binding; the one real divergence (async wasm instantiation) is handled by `--target bundler` + bundler instantiation-hoisting.

**Tech Stack:** Rust (edition 2024, `wasm32-unknown-unknown`), wasm-bindgen 0.2, wasm-pack 0.15 (proto), Moon 2.3.2, pnpm/vitest 4 (+ vite-plugin-wasm), TypeScript.

**Spec:** `docs/superpowers/specs/2026-06-17-sma-427-wasm-kernel-binding-design.md` (read it — esp. §7 spike gate and the Review dispositions).

---

## Conventions for every task

- **PATH:** proto-managed tools (`moon`, `wasm-pack`, `cargo`, `pnpm`) are off the default Bash PATH. Prefix shell steps with:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` (shims FIRST = repo-pinned versions). Run `moon`/`cargo` from inside `rs/` per CLAUDE.md/`rs/.cargo/config.toml`.
- **Commits:** Conventional Commits with a scope from the allowlist (`rs`, `ts`, `ci`, `repo`, `docs`, …) — commitlint enforces a non-empty scope. End every commit message body with a blank line then:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
  Commits are SSH-signed via 1Password; if a commit fails with "failed to fill whole buffer", 1Password is locked — ask the user to unlock, then retry.
- **Branch:** already on `feature/sma-427-stand-up-the-wasm-kernel-binding-paigasus-wasm-for`.

## File map

| File | Action | Responsibility |
|------|--------|----------------|
| `.proto/plugins/wasm-pack.toml` | create | Vendored proto plugin resolving wasm-pack GitHub releases |
| `.prototools` | modify | Pin `wasm-pack = "0.15.0"` + register the plugin |
| `.moon/toolchains.yml` | modify | Add `rust.targets: ['wasm32-unknown-unknown']` (CI target provisioning — H1) |
| `rs/rust-toolchain.toml` | modify | Comment-only: clarify `targets` here is dev convenience, not the CI mechanism |
| `rs/Cargo.toml` | modify | Add `wasm-bindgen = "0.2"` to `[workspace.dependencies]` + the lockstep invariant comment |
| `rs/crates/bindings/paigasus-wasm/Cargo.toml` | create | cdylib crate manifest; machete ignore; wasm-opt off |
| `rs/crates/bindings/paigasus-wasm/src/lib.rs` | create | `#[wasm_bindgen] sum` → `paigasus_kernel::sum` |
| `rs/crates/bindings/paigasus-wasm/package.json` | create | Hand-written `@paigasus/wasm` (wasm-pack `--no-pack`) |
| `rs/crates/bindings/paigasus-wasm/moon.yml` | create | `id: paigasus-wasm-rs`; kernel→binding edge |
| `rs/crates/bindings/paigasus-wasm/.gitignore` | create | Ignore `*.wasm`; commit the JS/`.d.ts` glue |
| `rs/crates/bindings/paigasus-wasm/paigasus_wasm*.{js,d.ts}` | create (generated) | Committed wasm-pack glue |
| `ts/packages/paigasus-kernel/src/wasm.ts` | create | Re-export `sum` from `@paigasus/wasm` |
| `ts/packages/paigasus-kernel/src/unsupported.ts` | delete | Replaced by the real wasm path |
| `ts/packages/paigasus-kernel/src/binding-parity.types.ts` | create | M5 compile-time guard: wasm/napi `sum` types must match |
| `ts/packages/paigasus-kernel/package.json` | modify | `@paigasus/wasm` file: dep; exports map; vite-plugin devDeps |
| `ts/packages/paigasus-kernel/vitest.config.ts` | modify | Two `test.projects`: node (napi) + browser (wasm) |
| `ts/packages/paigasus-kernel/tests/sum.wasm.test.ts` | create | Browser-condition round-trip (AC #1 proof) |
| `ts/pnpm-workspace.yaml` | modify | Catalog `vite-plugin-wasm` + `vite-plugin-top-level-await` |
| `ts/packages/paigasus-kernel/moon.yml` | modify | `dependsOn` += wasm-rs; build/test drive wasm-pack too |
| `ci/affected-graph/run.sh` | modify | Extend `kernel->bindings`; add `binding-oneway-wasm` |
| `ci/affected-graph/README.md` | modify | Maintenance note: second kernel→ts edge |

---

## Phase 0 — Spike & go/no-go gate (BLOCKER — review before Phase 1)

> Provisions the **real** tooling (kept) and the **real** crate, then validates the build chain and decides the two structural branches (host-gate exclusion? `init()` fallback?) **before** any cascade/guard/consumer wiring. Spec §7. Stop for orchestrator review at the end.

### Task 0.1: Provision wasm-pack + the wasm32 target

**Files:** Create `.proto/plugins/wasm-pack.toml`; Modify `.prototools`, `.moon/toolchains.yml`, `rs/rust-toolchain.toml`.

- [ ] **Step 1: Create the proto plugin** `.proto/plugins/wasm-pack.toml`

```toml
# Vendored proto TOML plugin for wasm-pack (SMA-427).
#
# Resolves official rustwasm/wasm-pack GitHub release tarballs. Same vendoring rationale as
# buf/cargo-machete: a static schema over official release assets. wasm-pack drives the wasm-bindgen
# build for paigasus-wasm; it auto-fetches the wasm-bindgen-cli matching the crate's wasm-bindgen
# version (see the rs/Cargo.toml invariant comment).
#
# Binary nests one directory deep (wasm-pack-v{version}-{target}/wasm-pack), so exe-path is required
# on every platform (like cargo-machete; unlike release-plz whose binary is at the archive root).
# NOTE: wasm-pack publishes NO per-asset .sha256, so — unlike cargo-machete — there is no
# checksum-file/checksum-url. Tags are "v"-prefixed (v0.15.0); asset filenames embed "v{version}".
# Linux is symmetric musl (both x86_64 and aarch64 musl assets exist), so {arch} works as-is.
# Windows ships x86_64 only.

name = "wasm-pack"
type = "cli"

[platform.linux]
download-file = "wasm-pack-v{version}-{arch}-unknown-linux-musl.tar.gz"
exe-path = "wasm-pack-v{version}-{arch}-unknown-linux-musl/wasm-pack"

[platform.macos]
download-file = "wasm-pack-v{version}-{arch}-apple-darwin.tar.gz"
exe-path = "wasm-pack-v{version}-{arch}-apple-darwin/wasm-pack"

[platform.windows]
# Only x86_64 is published for Windows; there is no aarch64-pc-windows-msvc asset.
download-file = "wasm-pack-v{version}-x86_64-pc-windows-msvc.tar.gz"
exe-path = "wasm-pack-v{version}-x86_64-pc-windows-msvc/wasm-pack.exe"

[install]
download-url = "https://github.com/rustwasm/wasm-pack/releases/download/v{version}/{download_file}"

[resolve]
git-url = "https://github.com/rustwasm/wasm-pack"
```

- [ ] **Step 2: Pin wasm-pack in `.prototools`**

Add under the existing CLI pins (alphabetical, after `release-plz`):
```toml
wasm-pack = "0.15.0"
```
And under `[plugins]` (after `release-plz`):
```toml
wasm-pack = "file://./.proto/plugins/wasm-pack.toml"
```

- [ ] **Step 3: Provision the wasm32 target in `.moon/toolchains.yml`**

In the existing `rust:` block, add a `targets` key (verified via `moon toolchain info rust`: this runs `rustup target add`):
```yaml
rust:
  version: '1.95.0'
  components:
    - 'rustfmt'
    - 'clippy'
  # wasm32 target for the paigasus-wasm binding (SMA-427). Provisioned HERE (the path moon's
  # `moon setup` actually uses), NOT via rust-toolchain.toml `targets` — moon reads only the
  # channel from that file (syncToolchainConfig), so a clean CI runner would never install it.
  targets:
    - 'wasm32-unknown-unknown'
```
(Keep the existing `# cargo-nextest is provisioned via …` comment.)

- [ ] **Step 4: Fix the `rs/rust-toolchain.toml` comment** (no functional change)

Append to the existing comment block, above `[toolchain]`:
```
# NOTE: the wasm32-unknown-unknown target is provisioned via .moon/toolchains.yml `rust.targets`
# (moon's setup path), NOT from a `targets` key here — moon reads only `channel` from this file.
```

- [ ] **Step 5: Install and verify the toolchain**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
proto install
wasm-pack --version
rustup target list --installed | grep wasm32-unknown-unknown
```
Expected: `wasm-pack 0.15.0`; `wasm32-unknown-unknown` listed. If wasm32 is absent locally, `rustup target add wasm32-unknown-unknown` and note that `.moon/toolchains.yml` must install it on clean CI (Task in Phase 3 verifies via the actual CI run).

- [ ] **Step 6: Commit**

```bash
git add .proto/plugins/wasm-pack.toml .prototools .moon/toolchains.yml rs/rust-toolchain.toml
git commit  # ci(repo): pin wasm-pack proto plugin + wasm32 target (SMA-427)
```

### Task 0.2: Stand up the `paigasus-wasm` crate and validate the build chain

**Files:** Create the crate (`Cargo.toml`, `src/lib.rs`, `.gitignore`); Modify `rs/Cargo.toml`.

- [ ] **Step 1: Add the workspace dependency** in `rs/Cargo.toml` `[workspace.dependencies]` (after the `napi-build` line, before `paigasus-kernel`):

```toml
# wasm-bindgen — Rust↔browser/Edge FFI for the wasm binding crate (ADR-0005). Consumed via the
# #[wasm_bindgen] macro; the cdylib's wasm imports (__wbindgen_*) resolve at INSTANTIATION on the
# wasm32-unknown-unknown target, so — unlike the PyO3/napi cdylibs — it needs NO rs/.cargo/config.toml
# link flags. wasm-pack (proto-pinned) fetches the matching wasm-bindgen-cli for whatever 0.2.z this
# caret resolves to. INVARIANT: the pinned wasm-pack must support that 0.2.z (crate↔CLI compat is exact
# per 0.2.z) — bump the two together (dependency-bump runbook), or this re-introduces the schema
# mismatch the proto pin was meant to avoid.
wasm-bindgen = "0.2"
```

- [ ] **Step 2: Create** `rs/crates/bindings/paigasus-wasm/Cargo.toml`

```toml
[package]
name = "paigasus-wasm"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
publish = false

[lib]
# Only cdylib: the browser/Edge runtime loads the .wasm; no other Rust crate consumes it. A
# wasm-bindgen cdylib leaves __wbindgen_* imports unresolved until instantiation, so a host test
# harness can't meaningfully link/run — disable the test/doctest targets so `cargo nextest
# --no-tests=pass` stays green (mirrors paigasus-node-bindings / paigasus-py-bindings). Kernel logic
# is unit-tested in paigasus-kernel; the FFI boundary is proven by the wasm round-trip vitest.
crate-type = ["cdylib"]
test = false
doctest = false

[dependencies]
wasm-bindgen.workspace = true
paigasus-kernel.workspace = true

[package.metadata.cargo-machete]
# wasm-bindgen is consumed only through the #[wasm_bindgen] attribute macro — the canonical
# cargo-machete false-positive (like pyo3 on the py crate / napi on the node crate); :machete is a
# blocking gate (SMA-375). Unlike napi-rs, wasm-bindgen ships macro + runtime in one crate, so a
# single ignore suffices. paigasus-kernel is called directly and needs no ignore.
ignored = ["wasm-bindgen"]

[package.metadata.wasm-pack.profile.release]
# Placeholder kernel — skip wasm-opt so no unpinned binaryen is downloaded at build (SMA-427 L3).
# Re-enabling optimization later must pin binaryen via a proto plugin (tracked follow-up).
wasm-opt = false

[lints]
workspace = true
```

- [ ] **Step 3: Create** `rs/crates/bindings/paigasus-wasm/src/lib.rs`

```rust
// SPDX-License-Identifier: Apache-2.0

//! wasm-bindgen binding shim for `paigasus-kernel` (ADR-0005): exposes the kernel's pure functions
//! to browsers/Edge. Compiled to `wasm32-unknown-unknown` and post-processed by wasm-pack
//! (`--target bundler`) into a `.wasm` + JS glue. The affected-graph cascade
//! `paigasus-kernel-rs → paigasus-wasm-rs` is proven by this crate compiling against a real
//! `paigasus_kernel::*` call (SMA-427).

use wasm_bindgen::prelude::wasm_bindgen;

/// Browser-callable wrapper over [`paigasus_kernel::sum`]. Uses `i32` at the FFI boundary so the
/// JS surface is a plain `number` (matching the napi binding); the kernel fn is `i64`, cast at the
/// boundary. A future kernel fn needing the full `i64` range gets explicit handling then (shared
/// across all bindings — SMA-427 L5).
#[wasm_bindgen]
pub fn sum(a: i32, b: i32) -> i32 {
    paigasus_kernel::sum(a as i64, b as i64) as i32
}
```

- [ ] **Step 4: Create** `rs/crates/bindings/paigasus-wasm/.gitignore`

```gitignore
# wasm-pack emits the binary here; commit the JS + .d.ts glue, ignore the built .wasm (mirrors the
# napi crate's `*.node` ignore — SMA-427).
*.wasm
```

And create the hand-written `rs/crates/bindings/paigasus-wasm/package.json` (`--no-pack` means wasm-pack does NOT generate one; this is what the `@paigasus/wasm` `file:` link resolves). Mirror the napi crate's co-located `package.json` shape (no `exports` map — `main`/`module`/`types`, resolved under `moduleResolution: bundler`, exactly as `@paigasus/node-bindings` does):

```json
{
  "name": "@paigasus/wasm",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "license": "Apache-2.0",
  "main": "paigasus_wasm.js",
  "module": "paigasus_wasm.js",
  "types": "paigasus_wasm.d.ts",
  "sideEffects": ["./paigasus_wasm.js", "./snippets/*"],
  "files": [
    "paigasus_wasm.js",
    "paigasus_wasm_bg.js",
    "paigasus_wasm_bg.wasm",
    "paigasus_wasm.d.ts",
    "paigasus_wasm_bg.wasm.d.ts"
  ]
}
```
> The glue filenames (`paigasus_wasm.js`, `paigasus_wasm_bg.js`, `paigasus_wasm.d.ts`, `paigasus_wasm_bg.wasm.d.ts`) are confirmed in Step 6; adjust `main`/`module`/`types`/`files` if the spike recorded different names.

- [ ] **Step 5: Verify host build + lint (M1 go/no-go — §7.1)**

Run (from `rs/`):
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
cargo build -p paigasus-wasm 2>&1 | tail -20
cargo clippy -p paigasus-wasm --all-targets -- -D warnings 2>&1 | tail -20
cargo machete . 2>&1 | tail -10
```
Expected (the spec's "plausible yes"): all succeed (host cdylib links via the existing apple-darwin `-undefined dynamic_lookup` / Linux undefined-symbol tolerance; clippy clean; machete green with the ignore).
**GO/NO-GO:** If `cargo build` fails to link OR clippy emits a `-D warnings` hard error on the host target → record the failure and switch to the FALLBACK: exclude `paigasus-wasm` from the host rust gates (note it for Phase 2's moon wiring and re-confirm the affected edge in Phase 3). Do NOT proceed past this step without recording the outcome in the findings doc (Task 0.4).

- [ ] **Step 6: Verify the wasm32 build via wasm-pack (§7.3)**

Run (from repo root):
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
wasm-pack build rs/crates/bindings/paigasus-wasm --target bundler --release --no-pack --out-dir . --out-name paigasus_wasm 2>&1 | tail -25
ls -1 rs/crates/bindings/paigasus-wasm/paigasus_wasm*
```
Expected: build succeeds; emits `paigasus_wasm.js`, `paigasus_wasm_bg.js`, `paigasus_wasm.d.ts`, `paigasus_wasm_bg.wasm`, `paigasus_wasm_bg.wasm.d.ts`. **Record the exact glue filenames** (Task 0.4) — they pin the `package.json` `files`/`module` and the `.gitignore`. If `--out-dir .` resolves relative to repo root instead of the crate dir, adjust to run wasm-pack with the crate as cwd; record which form worked.

- [ ] **Step 7: Validate vitest + vite-plugin-wasm instantiation (§7.4)** — throwaway scratch check

In `$CLAUDE_JOB_DIR/tmp`, create a minimal vitest that imports the emitted glue through `vite-plugin-wasm` + `vite-plugin-top-level-await` and asserts `sum(2,3) === 5`. (Use `pnpm dlx` or a scratch `node_modules` so this does not touch the repo.) Confirm instantiation works in a Node `browser`-condition run.
**GO/NO-GO:** if the bundler-target glue cannot be instantiated under vite-plugin-wasm at all, escalate — this blocks AC #1's proof mechanism. Record the working plugin config for Phase 2.

- [ ] **Step 8: (time-boxed, ≤30 min) Next.js client-component check (§7.6 / H2)**

Optionally stand up a throwaway Next.js app importing `sum` from the local `@paigasus/wasm` glue in a client component (turbopack and/or webpack `asyncWebAssembly`) and confirm the synchronous call site works. **Not blocking AC** (no live consumer). If it works → keep decision #1 (sync, no `init()`). If it doesn't or is inconclusive in the time box → record the risk and that the pre-agreed `await init()` fallback remains open; proceed with the bundler-sync surface for now.

- [ ] **Step 9: Clean the scratch artifacts**, keep the crate source + the committed glue.

### Task 0.3: Commit the crate + glue

- [ ] **Step 1: Stage and commit**

```bash
git add rs/Cargo.toml rs/crates/bindings/paigasus-wasm/
# (paigasus_wasm_bg.wasm is gitignored; the JS/.d.ts glue is committed)
git status --short   # confirm: Cargo.toml + Cargo.lock + crate files + glue; NO *.wasm
git commit  # feat(rs): add paigasus-wasm wasm-bindgen kernel binding (SMA-427)
```
Expected `git status` after add: `rs/Cargo.toml`, `rs/Cargo.lock`, the four crate files, and the committed glue files — and **no** `*.wasm`.

### Task 0.4: Findings doc + GATE

- [ ] **Step 1: Write** `docs/superpowers/specs/2026-06-17-sma-427-spike-findings.md` recording, for each of §7.1–§7.8: the command run, the observed result, and the go/no-go decision. Explicitly state: (a) host build/lint PASS or FALLBACK-exclude; (b) exact glue filenames; (c) wasm-pack `--out-dir` form that worked; (d) the working vite-plugin-wasm config; (e) Next.js outcome (sync confirmed / risk + `init()` fallback open); (f) whether the machete ignore is actually needed.

- [ ] **Step 2: Commit** `docs(repo): SMA-427 spike findings + go/no-go`.

- [ ] **Step 3: STOP for orchestrator review.** If §7.1 forced the host-gate exclusion or §7.6 forced `init()`, the orchestrator amends Phases 1–3 before continuing.

---

## Phase 1 — Dual-export `@paigasus/kernel` (node → napi, browser/default → wasm)

### Task 1.1: Catalog the vite wasm plugins

**Files:** Modify `ts/pnpm-workspace.yaml`.

- [ ] **Step 1: Add to the `catalog:` block** (after the `@napi-rs/cli` entry):

```yaml
  # Vite wasm handling for the browser-condition kernel round-trip test (SMA-427). The bundler-target
  # wasm-pack glue does a bundler-style `import * as wasm from './…_bg.wasm'`; vitest (Vite) needs
  # these to instantiate it. Browser vitest project only.
  vite-plugin-wasm: ^3.6.0
  vite-plugin-top-level-await: ^1.6.0
```
(Confirm/adjust the caret ranges against the versions the spike installed.)

- [ ] **Step 2: Commit** after the package.json wiring (Task 1.2) so the lockfile updates together.

### Task 1.2: Wire `@paigasus/kernel` package.json + source

**Files:** Modify `ts/packages/paigasus-kernel/package.json`; Create `src/wasm.ts`; Delete `src/unsupported.ts`.

- [ ] **Step 1: Edit** `ts/packages/paigasus-kernel/package.json` — add the dep, the vite-plugin devDeps, flip the exports, update the comment:

```jsonc
{
  "name": "@paigasus/kernel",
  "_comment_exports": "node → the napi binding (compiled .node); browser/default → the wasm binding (paigasus-wasm, .wasm + glue). Conditions point at SOURCE (bundler-aware consumers walk TS); switch to ./dist/* when tsup wiring lands, IN LOCKSTEP with flipping `private: false`. workerd intentionally omitted (no verified workerd path yet — SMA-427 H3); it falls through to `default`.",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "license": "Apache-2.0",
  "exports": {
    ".": {
      "node": "./src/index.ts",
      "browser": "./src/wasm.ts",
      "default": "./src/wasm.ts"
    }
  },
  "scripts": {
    "typecheck": "tsc -p tsconfig.json --noEmit"
  },
  "dependencies": {
    "@paigasus/node-bindings": "file:../../../rs/crates/bindings/paigasus-node-bindings",
    "@paigasus/wasm": "file:../../../rs/crates/bindings/paigasus-wasm"
  },
  "devDependencies": {
    "@napi-rs/cli": "catalog:",
    "typescript": "catalog:",
    "vite-plugin-top-level-await": "catalog:",
    "vite-plugin-wasm": "catalog:",
    "vitest": "catalog:"
  }
}
```

- [ ] **Step 2: Create** `ts/packages/paigasus-kernel/src/wasm.ts`

```typescript
// SPDX-License-Identifier: Apache-2.0
export { sum } from '@paigasus/wasm';
```

- [ ] **Step 3: Delete** `ts/packages/paigasus-kernel/src/unsupported.ts`

```bash
git rm ts/packages/paigasus-kernel/src/unsupported.ts
```

- [ ] **Step 4: Install** so the `file:` link + catalog devDeps resolve:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts install
```
Expected: lockfile updates with `@paigasus/wasm` (file:) + the two vite plugins; no errors.

- [ ] **Step 5: Commit**

```bash
git add ts/pnpm-workspace.yaml ts/pnpm-lock.yaml ts/packages/paigasus-kernel/package.json ts/packages/paigasus-kernel/src/
git commit  # feat(ts): route @paigasus/kernel browser/default exports to paigasus-wasm (SMA-427)
```

### Task 1.3: M5 type-assignability guard

**Files:** Create `ts/packages/paigasus-kernel/src/binding-parity.types.ts`.

- [ ] **Step 1: Write the guard (the "failing test" is a compile error on drift)**

```typescript
// SPDX-License-Identifier: Apache-2.0
export {}; // module marker (isolatedModules); no runtime exports.
// Compile-time guard (SMA-427 M5): the wasm and napi `sum` surfaces must stay type-identical, because
// `@paigasus/kernel`'s typecheck only ever resolves the `node` (napi) condition (tsconfig
// customConditions), so the shipped browser surface is otherwise never type-checked. No runtime effect;
// `tsc --noEmit` fails the build if either binding's `sum` signature drifts.
//
// Uses `typeof import(...)` (a pure type query, no import statement) so it is safe under the repo's
// `verbatimModuleSyntax` + `isolatedModules` — an `import type { sum }` + `typeof sum` would be illegal
// (can't use a type-only binding as a value).
type NapiApi = typeof import('@paigasus/node-bindings');
type WasmApi = typeof import('@paigasus/wasm');

// If a signature diverges, the corresponding alias becomes `never` and the `= true` lines fail to compile.
type _NapiSumAssignableToWasm = NapiApi['sum'] extends WasmApi['sum'] ? true : never;
type _WasmSumAssignableToNapi = WasmApi['sum'] extends NapiApi['sum'] ? true : never;

const _napiOk: _NapiSumAssignableToWasm = true;
const _wasmOk: _WasmSumAssignableToNapi = true;
void _napiOk;
void _wasmOk;
```

- [ ] **Step 2: Verify typecheck passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts/packages/paigasus-kernel exec tsc -p tsconfig.json --noEmit
```
Expected: PASS (both `sum` are `(a: number, b: number) => number`).

- [ ] **Step 3: Prove the guard catches drift** — temporarily edit `rs/crates/bindings/paigasus-wasm/src/lib.rs` `sum` to take one arg (`pub fn sum(a: i32) -> i32 { ... }`), rebuild the glue (`wasm-pack build …`), rerun `tsc --noEmit`.
Expected: FAIL (`Type 'true' is not assignable to type 'never'`). Then **revert** the lib.rs change, rebuild the glue, rerun `tsc` → PASS.

- [ ] **Step 4: Commit**

```bash
git add ts/packages/paigasus-kernel/src/binding-parity.types.ts
git commit  # test(ts): guard @paigasus/wasm and node-bindings sum types stay compatible (SMA-427)
```

---

## Phase 2 — Moon cascade + browser-condition round-trip (AC #1, AC #2 build/test)

### Task 2.1: `paigasus-wasm-rs` Moon project

**Files:** Create `rs/crates/bindings/paigasus-wasm/moon.yml`.

- [ ] **Step 1: Create** the project (near-copy of `paigasus-node-bindings-rs/moon.yml`):

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-wasm-rs'
# Moon-side layer label for this FFI crate (no native `binding` layer). Built like a library but NOT
# published as an rlib — ships as a .wasm + JS glue via wasm-pack (driven from paigasus-kernel-ts).
layer: 'library'
language: 'rust'

# The kernel→binding edge (ADR-0005): a kernel change must rebuild this crate. Task-level `^:build`
# propagates `affected` under `moon ci --include-relations` (a project dependsOn alone does NOT —
# SMA-389 D3). Mirrors paigasus-node-bindings-rs.
dependsOn:
  - 'paigasus-kernel-rs'

tasks:
  build:
    deps: ['^:build']
  test:
    deps: ['^:build']
```
> **If the spike (Task 0.2 Step 5) forced the host-gate exclusion:** add the inherited-task override here to skip/replace the host `build`/`lint` for this crate per the spike's recorded approach, instead of the bare `deps` above.

- [ ] **Step 2: Verify the project + edge register**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects --affected --downstream deep <<< "rs/crates/libs/paigasus-kernel/src/lib.rs" | python3 -c 'import sys,json;print(sorted(p["id"] for p in json.load(sys.stdin)["projects"]))'
```
Expected: includes `paigasus-wasm-rs` (alongside the existing kernel/py/node/gateway/ts ids).

- [ ] **Step 3: Commit** `feat(rs): register paigasus-wasm-rs moon project + kernel cascade edge (SMA-427)`.

### Task 2.2: Drive wasm-pack from `paigasus-kernel-ts` build/test

**Files:** Modify `ts/packages/paigasus-kernel/moon.yml`.

- [ ] **Step 1: Add `paigasus-wasm-rs` to `dependsOn`:**

```yaml
dependsOn:
  - 'paigasus-node-bindings-rs'
  - 'paigasus-wasm-rs'
```

- [ ] **Step 2: Update the `build` task** — `touch` all three sources, run napi build, then wasm-pack build, then typecheck. Replace the existing `build.script` with:

```yaml
    script: 'touch ../../../rs/crates/libs/paigasus-kernel/src/lib.rs ../../../rs/crates/bindings/paigasus-node-bindings/src/lib.rs ../../../rs/crates/bindings/paigasus-wasm/src/lib.rs && pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings && wasm-pack build ../../../rs/crates/bindings/paigasus-wasm --target bundler --release --no-pack --out-dir . --out-name paigasus_wasm && pnpm exec tsc -p tsconfig.json --noEmit'
```
Add to the `build.inputs` (after the node-bindings entries):
```yaml
      - '/rs/crates/bindings/paigasus-wasm/src/**/*'
      - '/rs/crates/bindings/paigasus-wasm/Cargo.toml'
      - '/rs/crates/bindings/paigasus-wasm/package.json'
```
Add to the `build.outputs` (gitignored binary ONLY — mirror napi; M3):
```yaml
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm'
```

- [ ] **Step 3: Update the `test` task** identically — add the wasm sources to `touch`, add `wasm-pack build …` before `pnpm exec vitest run`, and add the same three wasm inputs. Replace `test.script` with:

```yaml
    script: 'touch ../../../rs/crates/libs/paigasus-kernel/src/lib.rs ../../../rs/crates/bindings/paigasus-node-bindings/src/lib.rs ../../../rs/crates/bindings/paigasus-wasm/src/lib.rs && pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings && wasm-pack build ../../../rs/crates/bindings/paigasus-wasm --target bundler --release --no-pack --out-dir . --out-name paigasus_wasm && pnpm exec vitest run'
```
Add the three wasm inputs to `test.inputs` as well.
> Adjust the `wasm-pack build` invocation to whatever form the spike (Task 0.2 Step 6) recorded as working for `--out-dir`.

- [ ] **Step 4: Commit** `feat(ts): build paigasus-wasm in the kernel-ts build/test cascade (SMA-427)`.

### Task 2.3: Two vitest projects + the wasm round-trip test (AC #1 proof)

**Files:** Modify `ts/packages/paigasus-kernel/vitest.config.ts`; Create `tests/sum.wasm.test.ts`.

- [ ] **Step 1: Rewrite** `vitest.config.ts` to a two-project layout (use the exact plugin config the spike recorded; verify the `test.projects` shape against the catalog-pinned vitest 4):

```typescript
// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';
import wasm from 'vite-plugin-wasm';
import topLevelAwait from 'vite-plugin-top-level-await';

// Resolve each binding to its crate dir (where the build rewrites the artifact) instead of pnpm's
// frozen `file:` store copy, which is NOT refreshed by a rebuild (SMA-420 store-copy staleness).
const nodeBindingDir = fileURLToPath(new URL('../../../rs/crates/bindings/paigasus-node-bindings/index.js', import.meta.url));
const wasmBindingDir = fileURLToPath(new URL('../../../rs/crates/bindings/paigasus-wasm/paigasus_wasm.js', import.meta.url));

export default defineConfig({
  test: {
    projects: [
      {
        // node/napi path (unchanged behavior): default conditions, .node external, crate-dir alias.
        test: {
          name: 'node',
          environment: 'node',
          include: ['tests/sum.test.ts'],
          server: { deps: { external: [/\.node$/] } },
        },
        resolve: { alias: { '@paigasus/node-bindings': nodeBindingDir } },
      },
      {
        // browser/wasm path: prepend `browser` to the DEFAULT conditions (do NOT replace them — bare
        // ['browser'] drops module/import and breaks source-exports `.ts` resolution — SMA-427 M4),
        // vite wasm plugins, and the crate-dir alias for fresh glue.
        plugins: [wasm(), topLevelAwait()],
        test: {
          name: 'browser',
          environment: 'node',
          include: ['tests/sum.wasm.test.ts'],
        },
        resolve: {
          conditions: ['browser', 'module', 'import', 'node', 'default'],
          alias: { '@paigasus/wasm': wasmBindingDir },
        },
      },
    ],
  },
});
```
> The exact `conditions` list and the `test.projects` nesting are spike-confirmed (§7.4). If vitest 4's pinned API differs, use the recorded form; keep the node project's behavior byte-for-byte and the browser conditions **additive**.

- [ ] **Step 2: Create** `tests/sum.wasm.test.ts`

```typescript
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { sum } from '@paigasus/kernel';

describe('kernel FFI (wasm)', () => {
  it('crosses the wasm boundary', () => {
    expect(sum(2, 3)).toBe(5);
    expect(sum(-4, 4)).toBe(0);
  });
});
```

- [ ] **Step 3: Run the kernel-ts test task (builds both bindings, runs both projects)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-kernel-ts:test 2>&1 | tail -40
```
Expected: wasm-pack + napi build succeed, BOTH vitest projects pass (`node` napi round-trip + `browser` wasm round-trip).

- [ ] **Step 4: Confirm the freshness cache-bust (§7.5)** — edit the kernel (`rs/crates/libs/paigasus-kernel/src/lib.rs` `sum` to `a + b + 1`), rerun `moon run paigasus-kernel-ts:test` → BOTH tests FAIL (proves the wasm path rebuilt against the edited kernel, not a stale `.wasm`). **Revert** the kernel edit; rerun → PASS.

- [ ] **Step 5: Commit** `test(ts): browser-condition wasm round-trip for @paigasus/kernel (SMA-427)`.

---

## Phase 3 — Affected-graph guard (AC #2)

### Task 3.1: Extend the guard cases

**Files:** Modify `ci/affected-graph/run.sh`, `ci/affected-graph/README.md`.

- [ ] **Step 1: Extend the `kernel->bindings` expected set** — add `paigasus-wasm-rs`. Replace that `run_case` line's CSV with:

```bash
  run_case "kernel->bindings" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs,paigasus-kernel-py,paigasus-node-bindings-rs,paigasus-kernel-ts,paigasus-wasm-rs"
```

- [ ] **Step 2: Add the `binding-oneway-wasm` case** immediately after the `binding-oneway-node` case:

```bash
  # wasm binding edit -> the wasm binding + the ts wrapper that depends on it (SMA-427). One-directional:
  # paigasus-kernel-rs deliberately absent (a binding edit must not rebuild the kernel).
  run_case "binding-oneway-wasm" "rs/crates/bindings/paigasus-wasm/src/lib.rs" \
    "paigasus-wasm-rs,paigasus-kernel-ts"
```

- [ ] **Step 3: Run the guard + negative control**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke 2>&1 | tail -30
bash ci/affected-graph/run.sh --negative-control 2>&1 | tail -15
```
Expected: all cases PASS (including the two new/changed); negative control still reports red as expected.

- [ ] **Step 4: Update** `ci/affected-graph/README.md`'s maintenance note to mention the second kernel→ts edge (`paigasus-kernel-rs → paigasus-wasm-rs → paigasus-kernel-ts`, alongside the napi edge).

- [ ] **Step 5: Commit** `test(ci): extend affected-graph guard for paigasus-wasm cascade (SMA-427)`.

---

## Phase 4 — Full gates + ADR note + runbook (AC #3, final verification)

### Task 4.1: Full local gate run

- [ ] **Step 1: Run the affected build/test the way CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cd ..
moon ci :build --include-relations 2>&1 | tail -40
moon ci :test  --include-relations 2>&1 | tail -40
moon run repo:machete repo:deny 2>&1 | tail -20
```
Expected: green across the board; the kernel→wasm cascade builds; both kernel-ts vitest projects pass.

- [ ] **Step 2:** If anything fails, fix at the source (do not paper over), re-run, then commit the fix with an appropriate scope.

### Task 4.2: ADR-0005 note + dependency-bump runbook invariant

**Files:** ADR-0005 (Notion — manual); the dependency-bump runbook location (CONTRIBUTING.md or wherever bumps are documented — confirm during the task).

- [ ] **Step 1: ADR-0005 note (AC #3)** — append a note in Notion recording: *browser/Edge bound via wasm-bindgen, built with wasm-pack (`--target bundler`), as the second TS-facing binding (after napi-rs); `workerd` deferred; pointer to this spec.* (This is a Notion edit, not a repo file — flag to the user to do it or confirm done.)

- [ ] **Step 2: Record the M2 invariant in the bump runbook** — wherever dependency bumps are documented, add the paired-bump checklist item: *"bumping `wasm-bindgen` (rs/Cargo.toml) or `wasm-pack` (.prototools) requires bumping/checking the other — the pinned wasm-pack must support the resolved `wasm-bindgen 0.2.z`."* If no such runbook exists yet, the Cargo.toml comment (Task 0.2) already captures it; note that to the user.

- [ ] **Step 3: Commit** any repo doc change `docs(repo): record wasm-bindgen↔wasm-pack bump invariant (SMA-427)`.

### Task 4.3: Push + PR

- [ ] **Step 1:** Push the branch and open a PR (only when the user asks). The branch name auto-links the PR to SMA-427 (do not attach the Linear link manually). PR body should summarize the dual-export, the spike outcomes, and the deferred follow-ups (workerd, parity harness, glue-drift check, binaryen pin).

---

## Self-review notes (author)

- **Spec coverage:** AC #1 → Phases 0–2 (crate + wiring + browser round-trip); AC #2 → Phase 2 cascade + Phase 3 guard; AC #3 → Task 4.2 ADR note. Spike §7.1–§7.8 → Phase 0 with explicit go/no-go. Follow-ups (workerd, L1 parity, L2 glue-drift, L3 binaryen) carried to the PR body / §8.
- **Structural branches** (host-gate exclusion §7.1, `init()` fallback §7.6) are gated in Phase 0 and flagged at the exact tasks they would change (2.1, 1.x). The orchestrator amends after the Phase 0 review.
- **Type consistency:** the crate exports `sum(i32,i32)->i32` → JS `sum(number,number):number`, matched by the M5 guard against `@paigasus/node-bindings`' `sum`; the same name is used across crate, glue, `src/wasm.ts`, both tests, and the guard.
