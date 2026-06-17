# SMA-427 — Stand up the wasm kernel binding (`paigasus-wasm`) for browser/Edge + dual-export `@paigasus/kernel`

**Status:** approved design (brainstorm + staff review incorporated; spike-gated, ready for plan)
**Linear:** [SMA-427](https://linear.app/smaschek/issue/SMA-427/stand-up-the-wasm-kernel-binding-paigasus-wasm-for-browseredge-dual)
**Date:** 2026-06-17
**ADR:** ADR-0005 (kernel-once — pure Rust kernel bound to Py/Node/WASM). ADR-0005 already names the three binding crates (`paigasus-py-bindings`, `paigasus-node-bindings`, `paigasus-wasm`), binds **Node via napi-rs** and **browser/Edge via wasm-bindgen**, and explicitly *rejected* "WASM only" in favour of the hybrid. This issue stands up the **second** of the two TS-facing bindings.
**Follow-up of:** [SMA-420](https://linear.app/smaschek/issue/SMA-420/stand-up-a-ts-kernel-binding-wasmnapi-wire-the-cascade-to-paigasus) — which stood up `paigasus-node-bindings` (napi-rs) as the first TS-facing binding and left `@paigasus/kernel` **Node-only** (its `browser`/`default` condition served by a throwing `src/unsupported.ts` stub). Design context: `docs/superpowers/specs/2026-06-15-sma-420-ts-kernel-napi-binding-design.md`.
**Mirrors:** SMA-419 (PyO3/maturin) and SMA-420 (napi-rs) — the structurally-symmetric third binding. Same co-located-artifact layout, same `^:build` + `--include-relations` cascade, same strict-equality affected-graph guard (SMA-429).

## Goal

Make the kernel value cross `Rust kernel → wasm-bindgen → .wasm + JS glue → browser/Edge` at **runtime**,
prove it with a `vitest` that resolves the **browser export condition**, and extend the affected-graph
guard so a kernel (or wasm-binding) edit cascades into the TS stack via this second edge. This replaces
the `src/unsupported.ts` stub landed in SMA-420 with the real wasm path, so `@paigasus/kernel` is no
longer Node-only and is consumable from the Next.js console's client components.

**This issue does not re-touch the kernel logic** (`paigasus_kernel::sum` stays the deliberate
placeholder from SMA-409) — it stands up the wasm binding crate, wires it into `@paigasus/kernel`'s
conditional exports, and extends the existing build graph + guard.

## Decisions resolved during brainstorming

1. **`sum(a, b): number` via `--target bundler` — synchronous *iff* the consuming bundler hoists
   instantiation.** Browser wasm instantiation is asynchronous, unlike the synchronous napi `.node`
   load — this is the one place the wasm binding genuinely departs from the napi mirror. We use wasm-pack
   `--target bundler`, which emits ESM glue that statically imports `…_bg.wasm`. Under webpack 5 /
   Next.js the relevant mode is `experiments.asyncWebAssembly`, which makes the wasm an **async module**
   whose top-level-await semantics propagate to importers: `sum` is then synchronous *to call*, but only
   after the async module graph has resolved — i.e. a clean synchronous `import { sum } from
   '@paigasus/kernel'; sum(2,3)` in a React client component depends on the consuming bundler hoisting
   instantiation (asyncWebAssembly + a TLA/bootstrap entry). **So "synchronous" is conditional on the
   consumer, not guaranteed by this crate.** The signature matches the node/napi path
   (`(number, number) => number`), and we prefer this over an explicit `await init()` or a lazy
   `sum(): Promise<number>`; but if the Next.js spike (§7.6) shows the synchronous call site can't be
   made to work cleanly, the **pre-agreed fallback** is `await init()` (or an async accessor) for the
   browser surface. *Not blocking today*, because no ts package imports `@paigasus/kernel` yet (verified
   in SMA-420), so AC #1 is met by the export condition resolving to a real wasm path plus the runtime
   round-trip test — **but note the vitest proof leans on `vite-plugin-top-level-await` to hoist
   instantiation, so a green test is evidence the export resolves and round-trips, NOT evidence that the
   Next.js client-component path works** (§5 / §7.6).
2. **wasm-pack pinned via a vendored proto plugin.** wasm-pack is a Rust-ecosystem binary, so it belongs
   with the proto-pinned Rust/CLI tooling (buf, cargo-deny/machete/nextest, lefthook, release-plz —
   SMA-375), **not** the pnpm catalog where the JS-native `@napi-rs/cli` lives. A vendored
   `.proto/plugins/wasm-pack.toml` (GitHub-release schema, like `cargo-machete.toml`) + a `wasm-pack` pin
   in `.prototools`. wasm-pack **auto-fetches the `wasm-bindgen-cli` matching the crate's `wasm-bindgen`
   version**, so there is no separate CLI↔crate lockstep pin to maintain (the failure mode of pinning
   `wasm-bindgen-cli` directly: a version skew aborts the build with a schema mismatch). `wasm-opt` is
   disabled for the placeholder so there is no unpinned binaryen download. Rejected alternatives:
   wasm-pack via the pnpm catalog (tightest napi mirror, but muddies the "Rust → proto, JS → catalog"
   line and pulls a binary on npm install) and `wasm-bindgen-cli` directly (lowest-level, but the
   lockstep pin is a standing footgun).
   **Residual invariant (not eliminated, relocated):** wasm-bindgen's crate↔CLI compatibility is exact
   per `0.2.z`. wasm-pack fetches the CLI matching whatever `0.2.z` Cargo.lock resolves, so the floating
   `wasm-bindgen = "0.2"` (§1, house style — cf. `pyo3 = "0.29"`, `napi = "3"`) is normally fine; the one
   failure mode is a `0.2.z` newer than the *pinned* wasm-pack knows how to fetch/drive. So the standing
   invariant is **"the pinned `wasm-pack` must support whatever `wasm-bindgen 0.2.z` Cargo.lock
   resolves"** — recorded on the workspace dep (§1) and added to the dependency-bump runbook as a
   paired-bump checklist item. (Optional tightening: pin `wasm-bindgen = "=0.2.z"` until publish, since
   nothing else gates it.)
3. **Runtime round-trip via vitest + `vite-plugin-wasm`, resolving the browser condition.** AC #1 wants
   the wasm *consumed by `@paigasus/kernel`'s browser/Edge export condition*, not just the raw crate. The
   `--target bundler` glue uses a bundler-style `import * as wasm from './…_bg.wasm'`, which vitest (Vite)
   does not handle natively. We add `vite-plugin-wasm` (+ `vite-plugin-top-level-await`) and a second
   vitest project with `resolve.conditions: ['browser']`, so the test imports `@paigasus/kernel`,
   resolves the real wasm branch, Vite instantiates it, and asserts `sum(2, 3) === 5`. This exercises the
   exact shipped export end-to-end and keeps a single artifact. The existing node/napi test keeps the
   `node` condition. Rejected alternatives: a separate `--target nodejs` test artifact (tests a different
   artifact than ships) and hand-instantiating the raw `.wasm` (bypasses the glue and the export
   condition).
4. **`wasm32-unknown-unknown` sidesteps the macOS link hazard entirely.** The PyO3/napi cdylibs needed
   `-undefined dynamic_lookup` in `rs/.cargo/config.toml` (their host-runtime symbols resolve at load).
   The wasm target resolves undefined imports natively at instantiation, so **no `.cargo/config.toml`
   change** is needed for the wasm build. The host-target build of the wasm crate (if `cargo build
   --workspace` compiles it) is the only place the existing flags are even relevant — see §7.
5. **Keep the placeholder `sum` as the public surface.** Unchanged from SMA-409/420. `i32` at the FFI
   boundary (kernel `sum` is `i64`, cast at the shim) — the same deterministic-`number` choice the napi
   crate made; the wasm/JS-number range concern is the wasm analog of napi's `i64`→`BigInt` hazard, so
   `i32` keeps both bindings identical and the smoke test asserting the FFI round-trip, not a domain
   contract.

## 1. Package layout (co-located, mirrors `paigasus-node-bindings`)

### `rs/crates/bindings/paigasus-wasm/` — new wasm-bindgen binding crate

```
rs/crates/bindings/paigasus-wasm/
  Cargo.toml        # [lib] crate-type = ["cdylib"]; test = false / doctest = false; wasm-opt off
  package.json      # @paigasus/wasm — hand-written (wasm-pack --no-pack); consumed via file:
  src/lib.rs        # #[wasm_bindgen] fn sum(a, b) -> calls paigasus_kernel::sum (real call → machete honest)
  moon.yml          # id: paigasus-wasm-rs
  .gitignore        # *.wasm (the built binary; commit the JS + .d.ts glue)
```

- **`Cargo.toml`**: `[lib] crate-type = ["cdylib"]`, `test = false`/`doctest = false` (mirror napi/pyo3 —
  the cdylib's wasm imports are unresolved on a host test link; kernel logic is unit-tested in
  `paigasus-kernel`, the FFI boundary is proven by compilation + the runtime smoke test). Dependencies:
  `wasm-bindgen.workspace = true` and `paigasus-kernel.workspace = true`.
  `[package.metadata.cargo-machete] ignored = ["wasm-bindgen"]` — `wasm-bindgen` is consumed only through
  the `#[wasm_bindgen]` attribute macro (the canonical cargo-machete false-positive, exactly like `pyo3`
  and `napi`); `:machete` is a blocking gate (SMA-375). Unlike napi-rs (which splits `napi`/`napi-derive`),
  wasm-bindgen ships the macro and runtime in **one** crate, so a single ignore entry suffices.
  `paigasus-kernel` is called directly and needs no ignore. wasm-opt disabled via
  `[package.metadata.wasm-pack.profile.release] wasm-opt = false`.
- **`src/lib.rs`**:
  ```rust
  // SPDX-License-Identifier: Apache-2.0
  use wasm_bindgen::prelude::wasm_bindgen;

  #[wasm_bindgen]
  pub fn sum(a: i32, b: i32) -> i32 {
      paigasus_kernel::sum(a as i64, b as i64) as i32
  }
  ```
  The `#[wasm_bindgen]`-annotated `sum` **calls `paigasus_kernel::sum`** so the Cargo edge is real and
  `cargo machete` stays green — identical shape to the napi shim.
- **`package.json` (`@paigasus/wasm`)**: hand-written (we pass `wasm-pack --no-pack` so wasm-pack does not
  generate its own), giving full control over the `file:`-link surface and parity with the co-located
  napi `package.json`. `private: true`, `version: 0.0.0`, `type: module`, `main`/`module`/`types`
  pointing at the committed glue, `files` listing the glue + `*.wasm`. SPDX per the CONTRIBUTING
  config-file exemption.

### `rs/Cargo.toml` `[workspace.dependencies]`

Add `wasm-bindgen` with a comment mirroring the pyo3/napi entries:

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

### macOS link flags — unchanged

`rs/.cargo/config.toml` is **not modified**. The wasm32 target resolves its imports natively; the
existing apple-darwin `-undefined dynamic_lookup` flags are relevant only to the (never-shipped)
host-target build of this crate — see §7.

## 2. `@paigasus/kernel` wiring — dual export (node → napi, browser/Edge → wasm)

```jsonc
// package.json
"dependencies": {
  "@paigasus/node-bindings": "file:../../../rs/crates/bindings/paigasus-node-bindings",
  "@paigasus/wasm": "file:../../../rs/crates/bindings/paigasus-wasm"
},
"exports": {
  ".": {
    "node": "./src/index.ts",      // napi re-export (unchanged)
    "browser": "./src/wasm.ts",     // wasm re-export
    "default": "./src/wasm.ts"      // anything not Node (browsers, bundlers) → wasm
  }
}
```

> **`workerd` deliberately omitted (H3).** `--target bundler` glue (`import * as wasm from
> './…_bg.wasm'`) is generally *not* what workerd/Cloudflare-Workers wants — its wasm story expects a
> `WebAssembly.Module` import with explicit instantiation (closer to `--target web`/manual). Advertising
> a `workerd` condition that resolves to an artifact nobody has run on workerd is worse than its absence:
> the first Edge consumer would resolve it and likely fail at runtime. Omitting the key, workerd falls
> through to `default` (the same wasm artifact) — *zero behavior change* — but we make no workerd-specific
> guarantee. A dedicated workerd path is a tracked follow-up (§8).

```typescript
// src/wasm.ts  (browser/default condition — keeps its SPDX header)
export { sum } from '@paigasus/wasm';
```

- **Delete `src/unsupported.ts`** — its throwing stub is replaced by the real wasm path. `src/index.ts`
  (the napi re-export) is unchanged.
- The `@paigasus/wasm` `file:` specifier is the wasm analog of the napi `file:` link — a cross-`ts/` link,
  not a pnpm workspace member. pnpm does not install a `file:` dep's devDeps, so the build tooling
  (`vite-plugin-wasm` etc.) is declared on `@paigasus/kernel`, not inherited (the SMA-420 spike S2
  lesson). wasm-pack itself is proto-pinned (not a pnpm dep), so it is not subject to this.
- **`_comment_exports`** is updated: the Node-only caveat is gone; the `node` path loads a compiled
  `.node`, the `browser`/`default` path loads a `.wasm`. Conditions still point at **source** until
  tsup/dist lands (in lockstep with flipping `private: false`).
- **`default → wasm` is a conscious change from SMA-420 (L4).** SMA-420 routed `default` to the throwing
  `unsupported.ts` stub; it now serves the real wasm path. Real Node always asserts the `node` condition,
  so this is low-risk — but Node-resident tooling that does *not* assert `node` (some test runners,
  `tsx`, non-standard resolvers) would now resolve the wasm path (slower than napi, possibly
  non-instantiating under plain Node ESM) instead of a clear throw. Flagged as intentional; revisit if a
  Node consumer trips on it.
- **`tsconfig.json` — `customConditions: ["node"]` kept; the shipped browser surface is *not*
  type-checked (M5).** The self-referential `import { sum } from '@paigasus/kernel'` (in both test files)
  resolves the `node` condition → `src/index.ts` (napi), so tsc validates against the napi `sum` type
  even in `tests/sum.wasm.test.ts` (which vitest runs against the wasm surface). `src/wasm.ts` the *file*
  is checked (it's in `include`), but the wasm `sum` *as consumed through `@paigasus/kernel`* is not.
  Benign today (both are `(number, number) => number`), but wasm-bindgen's `--target bundler` `.d.ts` also
  emits a default-export/init type, and future glue-signature drift would pass typecheck silently.
  **Guard (cheap):** add a compile-time assignability assertion — `@paigasus/wasm`'s `sum` type must be
  assignable to `@paigasus/node-bindings`'s `sum` type — so any divergence in the shipped browser `.d.ts`
  fails the build instead of surfacing at a consumer. (Authoring view stays on one surface; the guard
  catches drift across them.)

## 3. Build tooling

- **Provision the `wasm32-unknown-unknown` target via `.moon/toolchains.yml` (H1).** Add
  `targets: ['wasm32-unknown-unknown']` to the existing `rust:` block. Verified via
  `moon toolchain info rust`: Moon's Rust toolchain has a first-class `targets: [string]` field ("List of
  Rust targets to automatically install with `rustup`"), and `syncToolchainConfig` only syncs the
  *version/channel* to `rust-toolchain.toml`. **Do NOT rely on `rust-toolchain.toml` `targets` for CI** —
  Moon provisions the toolchain from `.moon/toolchains.yml`, not from the `targets` key of the version
  file, so a clean CI runner would never install `wasm32` and the first `wasm-pack build` would abort
  ("can't find crate for `std`"). This is latent on a dev host whose plain `cargo` is the rustup proxy
  (which *does* honor the file's `targets`), so it is the classic green-locally/red-in-CI trap — §7.2
  makes "wasm32 present on a clean CI runner" an explicit, asserted spike checkpoint. (Adding `targets`
  to `rust-toolchain.toml` as well is harmless dev convenience, but is **not** the CI mechanism — the
  comment there must not claim otherwise.)
- **`.proto/plugins/wasm-pack.toml`** (new, vendored): a static GitHub-release schema over the official
  `rustwasm/wasm-pack` release assets, modeled on `cargo-machete.toml` (per-platform `download-file` /
  `checksum-file` / `exe-path`, `[install]` download URLs, `[resolve]` git-url). Confirm at spike time
  whether wasm-pack's release tarballs nest the binary one directory deep (→ `exe-path` needed, like
  cargo-machete) or place it at the root (→ none, like release-plz), and the libc/arch asset matrix
  (CI x86_64-linux + local macOS arm64 at minimum; defer others as the other plugins do).
- **`.prototools`**: add `wasm-pack = "<pinned>"` under the existing CLI pins, and
  `wasm-pack = "file://./.proto/plugins/wasm-pack.toml"` under `[plugins]`.
- **Invocation** (run from the crate dir, like the napi build runs from its crate dir):
  ```
  wasm-pack build --target bundler --release --no-pack --out-dir . --out-name paigasus_wasm
  ```
  `--target bundler` (decision #1) · `--no-pack` (hand-written package.json, decision/§1) · `--out-dir .`
  so the glue + `.wasm` land in the crate dir (where the `file:` link + vitest alias resolve them) ·
  `--out-name paigasus_wasm` for stable glue filenames. Output uses the shared `rs/target/` cargo
  target-dir.
- **Committed vs ignored**: commit the generated JS + `.d.ts` glue (repo commit-generated-code posture →
  typecheck resolves types with no prebuild, exactly as napi commits `index.js`/`index.d.ts`); gitignore
  `*.wasm` (the built binary, like napi's `*.node`).
- **`wasm-opt = false` is standing config (L3).** Correct for the placeholder (no unpinned binaryen
  download), but re-enabling it later reintroduces exactly the unpinned-binary problem the proto-pinning
  posture exists to prevent. Follow-up (§8): pin `wasm-opt`/binaryen via a proto plugin (or vendor it)
  when optimization is turned on.

## 4. Build-graph edges (Moon)

The cascade gains a second kernel→binding→wrapper path:
`paigasus-kernel-rs → paigasus-wasm-rs → paigasus-kernel-ts` (alongside the napi path), propagated by
task-level `^:build` under `moon ci --include-relations` (a project `dependsOn` alone does not mark a
dependent task-affected — SMA-389 D3).

- **`rs/crates/bindings/paigasus-wasm/moon.yml`** (`id: paigasus-wasm-rs`, `layer: library`,
  `language: rust`): `dependsOn: ['paigasus-kernel-rs']`, `build`/`test` with `deps: ['^:build']` — a
  near-copy of `paigasus-node-bindings-rs/moon.yml`. Its `build` is the plain `cargo build` gate (what
  `fmt`/`clippy`/`nextest` compile against) — see §7 for the host-target question.
- **`ts/packages/paigasus-kernel/moon.yml`**: add `paigasus-wasm-rs` to `dependsOn`. `@paigasus/kernel`
  now ships **and tests both bindings**, so its `build`/`test` tasks drive **both** tool-builds:
  - `build`: `touch` the kernel + **both** binding sources (the mtime-freshness fix, extended to the wasm
    crate), then `napi build … && wasm-pack build …`, then `tsc --noEmit`.
  - `test`: same `touch` + `napi build` + `wasm-pack build`, then `pnpm exec vitest run` (which runs both
    vitest projects — §5).
  - **`inputs`**: add the wasm crate's `src/**/*`, `Cargo.toml`, and `package.json` (so a wasm-source or
    wasm-config change re-keys the task), alongside the existing kernel + napi inputs.
  - **`outputs`**: the gitignored `*.wasm` **only** — mirroring the napi task, which lists only the
    gitignored `*.node` and deliberately keeps the committed `index.js`/`index.d.ts` *out* of `outputs`
    (M3, verified against `paigasus-node-bindings`). Moon hydrates `outputs` from cache, so listing the
    *committed* glue risks a cached copy clobbering what's in git and muddying `git status`/diffs. The
    glue's content is fully determined by listed `inputs` (Rust src + `pnpm-lock.yaml`), so it is
    cache-correct without being an output.
  - Rationale is identical to the napi tasks (ordering + cache-bust in one step), just doubled. The
    double-tool-build is the deliberate cost of a single package that ships both bindings.
- **No new `paigasus-wasm-ts` Moon project** — the `.wasm` + glue are built as part of the
  `paigasus-kernel-ts` build chain, exactly as the `.node` is (and as the py wheel was for
  `paigasus-kernel-py`). The graph stays `kernel-rs → {node,wasm}-bindings-rs → kernel-ts`.

## 5. Public surface & runtime smoke test

- **`ts/packages/paigasus-kernel/vitest.config.ts`**: convert the current single `defineConfig` to a
  **`test.projects`** layout — two projects (M4). Pin-check at spike: the catalog pins `vitest ^4.1.8`,
  so confirm the exact projects API against *that* version (vitest 4 `test.projects`, not the deprecated
  `test.workspace` or the latest-docs shape). Plugins and aliases are declared **per project** so the
  wasm tooling does not leak into the node project:
  - the existing **node** project — unchanged behavior: default conditions, the `@paigasus/node-bindings`
    crate-dir alias, `server.deps.external: [/\.node$/]`, running `tests/sum.test.ts` against napi;
  - a new **browser** project — `vite-plugin-wasm` + `vite-plugin-top-level-await`, an alias
    `@paigasus/wasm` → the crate-dir glue (the pnpm `file:`-store-copy staleness fix, mirroring the napi
    alias), and `resolve.conditions` set to **`['browser', ...defaults]`** — *prepend* `browser`, do not
    replace. Setting `resolve.conditions: ['browser']` bare drops Vite's default conditions
    (`module`/`import`/`development|production`/`node`), which breaks resolution of source-`exports`
    `.ts` packages (including `@paigasus/kernel` itself, whose conditions point at `.ts`). Spike-confirm
    the additive list and that the node project keeps the existing resolution.
- **`ts/packages/paigasus-kernel/tests/sum.wasm.test.ts`** (new):
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
  Importing `@paigasus/kernel` under the browser condition resolves `src/wasm.ts` → `@paigasus/wasm`, so
  the test transitively proves the whole `kernel → wasm-bindgen → glue → browser export` chain.
- Add `vite-plugin-wasm` + `vite-plugin-top-level-await` to the pnpm **catalog** and `@paigasus/kernel`'s
  devDeps (catalog-listed so they bump centrally, like the napi CLI).

## 6. Affected-graph regression guard (`ci/affected-graph/run.sh` + README)

Strict-equality / default-deny model (SMA-429), so the change is to the expected sets:

- **`kernel->bindings`**: add `paigasus-wasm-rs` to the expected CSV. A kernel edit now legitimately
  reaches: `paigasus-kernel-rs, paigasus-py-bindings-rs, paigasus-gateway-rs, paigasus-kernel-py,
  paigasus-node-bindings-rs, paigasus-kernel-ts, paigasus-wasm-rs`. (`paigasus-kernel-ts` is already
  present via the napi edge; the wasm edge adds `paigasus-wasm-rs`, not a new ts dependent.)
- **New `binding-oneway-wasm` case**: touch `rs/crates/bindings/paigasus-wasm/src/lib.rs` → expected set
  `paigasus-wasm-rs, paigasus-kernel-ts`. One-directional w.r.t. the kernel (`paigasus-kernel-rs`
  deliberately absent — a binding edit must not rebuild the kernel), enforced implicitly by strict
  equality. Direct mirror of `binding-oneway-node`.
- The existing `contracts->proto`, `binding-oneway` (py), `binding-oneway-node`,
  `assert_include_relations`, and `--negative-control` cases are unchanged.
- Update `ci/affected-graph/README.md`'s maintenance note to reflect the second kernel→ts edge.

## 7. Primary risk — spike is a **blocker gate**, not step one of many (P1)

The wasm orchestration across the `rs/` ↔ `ts/` boundary is the real unknown, and three of the checks
below (7.1 host build, 7.2 clean-CI target, 7.6 Next.js) have outcomes that can **force structural
change** — a workspace-gate exclusion, an explicit CI provisioning step, or the `init()` fallback that
changes the public surface. So the spike is sequenced as an explicit gate: **run it, review the results,
amend this spec, THEN build the crate/tooling/guard/ADR wiring** — not the reverse, or a failed
assumption gets discovered after the moon.yml/CI/guard are already in place and has to be unwound. Run
the platform-sensitive checks on **both Linux (the CI image) and macOS**, not just the dev host. Each
check is a recorded go/no-go.

1. **Host workspace gates vs a wasm crate — binary go/no-go on Linux + macOS (M1).** The inherited Moon
   `build` is `cargo build` and `lint` is `cargo clippy --all-targets -- -D warnings` (confirmed in
   `.moon/tasks/rust.yml`), and the workspace is `warnings = "deny"` — so the host *does* attempt the
   wasm cdylib link and *does* hard-error on any warning. Does the wasm-bindgen cdylib build + lint
   cleanly on the host target?
   - *Plausible yes:* clippy checks without linking; `test = false`/`doctest = false` makes nextest a
     no-op; the host cdylib link is covered by the same undefined-symbol deferral the napi/pyo3 cdylibs
     rely on (macOS `-undefined dynamic_lookup`, Linux's default tolerance) — `__wbindgen_*` defers like
     `_Py*`/`napi_*`. *But* wasm-bindgen on a non-`wasm32` target is not its primary configuration; any
     cfg-gated `dead_code`/`unused` warning becomes a hard error under `-D warnings`. **This is a real
     risk, not a footnote** — decide it before wiring.
   - *Fallback, pre-designed:* exclude `paigasus-wasm` from the host gates (`--exclude paigasus-wasm`, or
     per-target config). Not free — it removes the crate from `cargo build`/`clippy`/`fmt` coverage, so
     re-confirm the `paigasus-kernel-rs → paigasus-wasm-rs` affected edge and the strict-equality guard
     still behave with a crate that builds only for wasm32. Record which path is taken **in this spec**
     before implementation proceeds.
2. **`wasm32` present on a *clean CI runner* (H1) — separate from "builds on my Mac".** Assert that after
   `proto install` + `moon setup` on a fresh runner (no prior `rustup target add`), the
   `.moon/toolchains.yml` `rust.targets` entry actually installs `wasm32-unknown-unknown` and the first
   `wasm-pack build` finds `std`. This is the explicit checkpoint that catches the green-locally/red-in-CI
   trap; do **not** let a passing dev-host build stand in for it.
3. **wasm-pack invocation + release-asset shape.** wasm-pack runs from the crate dir against the shared
   `rs/target/` workspace; `--no-pack` + hand-written `package.json` resolves via the `file:` link; glue
   lands in the crate dir. Confirm the wasm-pack proto plugin's `exe-path`/asset matrix (the
   `cargo-machete.toml`-vs-`release-plz.toml` nesting question) and that `cargo` is on PATH when the build
   triggers.
4. **vitest + `vite-plugin-wasm` instantiation.** A Node `browser`-condition vitest project actually
   instantiates the `--target bundler` glue (via `vite-plugin-wasm` [+ top-level-await]) and
   `sum(2, 3) === 5`. Confirm the `test.projects` API against the catalog-pinned vitest, the **additive**
   `resolve.conditions` (`['browser', ...defaults]`), and that the node/napi project keeps its existing
   resolution.
5. **Freshness / cache-bust on a Rust edit.** A kernel- or wasm-source edit re-runs the wasm-pack
   **compile** (the `touch` mtime fix) rather than asserting against a stale `.wasm`/glue, and the
   crate-dir alias loads the fresh glue rather than pnpm's frozen `file:` store copy (the napi store-copy
   staleness, in wasm form).
6. **Next.js client-component import — gates the "no `init()`" decision (H2).** Stand up a throwaway
   Next.js app importing `sum` from `@paigasus/kernel` in a **client component**, on **both turbopack and
   webpack** (`experiments.asyncWebAssembly`), and confirm `sum(2,3)` is callable as designed — i.e. the
   synchronous call site works once instantiation is hoisted. The vitest proof (check 4) leans on
   `vite-plugin-top-level-await`, so it is **not** evidence for this path. If the synchronous call site
   can't be made to work cleanly, switch the browser surface to the pre-agreed `await init()` (or async
   accessor) fallback (decision #1) — decide this **before** committing to "no `init()`".
7. **`wasm-opt` off + binaryen.** Confirm `wasm-opt = false` keeps the build from downloading binaryen
   (no unpinned network dependency), and the unoptimized `.wasm` instantiates fine for the placeholder.
8. **(10-second check) machete ignore necessity.** `src/lib.rs` uses an explicit
   `use wasm_bindgen::prelude::wasm_bindgen;`. pyo3 keeps its machete ignore despite a glob `use`, so the
   wasm ignore is very likely needed too — but confirm whether `cargo machete` still flags `wasm-bindgen`
   with the explicit `use` present, and drop the ignore if not.

## 8. ADR note (AC #3)

ADR-0005 already names `paigasus-wasm` and decides the napi/wasm hybrid, so **no new ADR**. AC #3
("binding tool/approach recorded") is satisfied by a short note appended to ADR-0005 recording that
**browser/Edge is bound via wasm-bindgen, built with wasm-pack (`--target bundler`), as the second
TS-facing binding** (alongside the napi-first note from SMA-420), with a pointer to this spec. Recorded in
Notion, where the ADRs live.

## Verification (maps to acceptance criteria)

1. **AC #1** — `paigasus-wasm` wraps `paigasus_kernel::sum`; `@paigasus/kernel`'s `browser`/`default`
   export condition resolves to the real wasm path (`src/unsupported.ts` deleted); the browser-condition
   vitest round-trip (`import { sum } from '@paigasus/kernel'` → `sum(2,3) === 5`) passes; `cargo machete`
   / `cargo deny` stay green over `rs/`. *(Caveat — H2: the vitest round-trip proves the export resolves
   and round-trips, not that the Next.js client-component synchronous call site works; that is the §7.6
   spike, with the `init()` fallback pre-agreed.)*
2. **AC #2** — `moon ci :build`/`:test` cascade a kernel edit to `paigasus-wasm-rs` and
   `paigasus-kernel-ts` under `--include-relations`; `moon run repo:affected-smoke` passes with the
   extended `kernel->bindings` set + new `binding-oneway-wasm` case; `--negative-control` still fails red;
   existing gates (including the napi path) unaffected.
3. **AC #3** — binding tool/approach (wasm-bindgen via wasm-pack, `--target bundler`) recorded as a note
   on ADR-0005.
4. **Cross-stack isolation preserved** — a kernel edit does not drag in `contracts`, the `*-py` packages
   other than `paigasus-kernel-py`, or the `-ts` packages other than `paigasus-kernel-ts`.

## Out of scope (deferred, with follow-ups)

- **Dedicated workerd/Edge path (H3)** — the `workerd` export key is omitted (§2); workerd falls through
  to `default` (the same bundler artifact) with no workerd-specific guarantee. A first-class workerd path
  (likely a `--target web`/`WebAssembly.Module` variant + its own export condition, verified on workerd)
  is **its own follow-up issue** — file it now so the gap is tracked, not rediscovered by the first Edge
  consumer.
- **Cross-binding behavioral-parity suite (L1)** — ADR-0005 / the Development Guidelines call for a
  property-based suite run against the Rust impl AND each binding (Py/Node/WASM) to catch drift. This is
  the **third** binding and there is still no parity harness (each binding has only a local smoke test).
  Harmless while `sum` is a placeholder, but the safety net must exist *before* real domain logic lands.
  **Recommend opening/scheduling the parity-harness issue as a prerequisite to "real kernel domain
  logic," not after.**
- **Committed-glue drift CI check (L2)** — committing the napi `index.js`/`.d.ts` and the wasm JS/`.d.ts`
  without a CI check that they match a fresh build means a forgotten rebuild ships a lying `.d.ts` (the
  `touch` fix addresses *build* freshness, not *commit* freshness). A CI step that rebuilds and runs
  `git diff --exit-code` over the committed glue closes it. Shared debt with napi/py → **systemic
  follow-up ticket**, not wasm-only.
- **Pin binaryen/`wasm-opt` when optimization is enabled (L3)** — `wasm-opt = false` is right for the
  placeholder; turning it on later reintroduces an unpinned binary download. Follow-up: pin via a proto
  plugin (or vendor it) at that point.
- **`i32` FFI surface over the `i64` kernel (L5)** — `paigasus_kernel::sum(a as i64, b as i64) as i32`
  truncates/wraps silently. Consistent with the napi crate (decision #5), so not a new defect — but the
  `i32` boundary is latent debt to retire deliberately (explicit `BigInt`/checked conversion) across
  **all** bindings at once, when a kernel fn actually needs the range.
- **Cross-target prebuild matrix + npm publish** — `private: false` / version off `0.0.0` for
  `@paigasus/kernel`/`@paigasus/wasm`/`@paigasus/node-bindings`, the wasm analog of the deferred napi
  prebuild + Python wheel publish (ADR-0006, SMA-376/407). Single-host build only here.
- **Real kernel domain logic** — `sum` stays the deliberate placeholder.
- **tsup/dist build** — export conditions still point at source; they flip to `./dist/*` in lockstep with
  `private: false` when tsup wiring lands.
- **Affected-graph completeness meta-check** — already tracked from SMA-420 F4; not folded in here.

## Review dispositions (staff review, 2026-06-17)

Findings from a staff-engineering design review, verified against the live repo before disposition.

- **H1 (High — `rust-toolchain.toml` `targets` ignored by Moon → no `wasm32` in CI) — accepted, verified,
  design changed.** Confirmed via `moon toolchain info rust`: Moon's Rust toolchain has a first-class
  `targets: [string]` field and `syncToolchainConfig` only syncs the *channel* to `rust-toolchain.toml`.
  §3 now provisions `wasm32-unknown-unknown` via `.moon/toolchains.yml` `rust.targets`; the
  `rust-toolchain.toml` route is demoted to dev convenience. §7.2 is an explicit clean-CI checkpoint.
- **H2 (High — "synchronous `sum`" unproven in Next.js; vitest TLA-rigged) — accepted, design changed.**
  Decision #1 reframed as "sync *iff* the consuming bundler hoists instantiation"; the vitest proof is
  explicitly recorded as *not* Next.js evidence; §7.6 adds a turbopack+webpack client-component spike and
  the `await init()` fallback is pre-agreed.
- **H3 (High — `workerd` wired but unverified; bundler glue wrong for workerd) — accepted, design
  changed.** The `workerd` export key is dropped (§2); it falls through to `default` (same artifact, zero
  behavior change) with no workerd guarantee. Dedicated workerd path is an explicit §8 follow-up.
- **M1 (Medium — host `cargo build`/`clippy -D warnings` vs the wasm cdylib) — accepted, verified.**
  Confirmed `.moon/tasks/rust.yml` is `cargo build` + `cargo clippy --all-targets -- -D warnings`. §7.1 is
  now a binary go/no-go on Linux + macOS with the `--exclude` fallback pre-designed.
- **M2 (Medium — floating `wasm-bindgen` × pinned wasm-pack lockstep) — accepted with nuance.** wasm-pack
  fetches the CLI matching the resolved `0.2.z`, so the caret is normally fine; the residual failure mode
  is a `0.2.z` newer than the pinned wasm-pack can drive. Recorded the "pinned wasm-pack ⊇ resolved
  `wasm-bindgen 0.2.z`" invariant on the workspace dep + the bump runbook (decision #2 / §1); kept the
  caret per house style.
- **M3 (Medium — committed glue in Moon `outputs`) — accepted, verified.** Mirrored napi: §4 `outputs` is
  the gitignored `*.wasm` only; committed glue is cache-keyed via `inputs`, never an output.
- **M4 (Medium — two-vitest-projects underspecified; `conditions` replacement) — accepted.** §5 specifies
  the `test.projects` layout, per-project plugins/aliases, and **prepending** `browser` to the default
  conditions (not replacing); pin-checked against catalog vitest at §7.4.
- **M5 (Medium — shipped browser surface never type-checked) — accepted.** §2 records the limitation and
  adds a compile-time assignability guard (`@paigasus/wasm` `sum` ⊑ `@paigasus/node-bindings` `sum`).
- **L1–L5 — accepted as notes/follow-ups.** Parity harness (L1) and committed-glue drift check (L2) added
  to §8 as systemic follow-ups; binaryen pin on wasm-opt re-enable (L3) in §3/§8; `default → wasm`
  conscious change (L4) flagged in §2; `i32` truncation debt (L5) in §8.
- **P1 (Process — spike as blocker gate) — accepted.** §7 reframed as a gate sequenced *before* the
  crate/tooling/guard/ADR wiring; this is carried into the implementation plan ordering.

## Implementation notes (as-built, 2026-06-17)

Deviations discovered during implementation (plan:
`docs/superpowers/plans/2026-06-17-sma-427-wasm-kernel-binding.md`; spike findings:
`2026-06-17-sma-427-spike-findings.md`). Pins: **wasm-pack 0.15.0**, **wasm-bindgen 0.2.125** (wasm-pack
auto-fetched the matching CLI — the §1 invariant held).

- **`vite-plugin-top-level-await` was NOT used (supersedes §3/§5/§7.4's "+ top-level-await").** Two
  reasons: (a) the `--target bundler` glue instantiates **synchronously** (`import * as wasm` + a sync
  `__wbindgen_start()`), so no top-level await is needed for this surface; (b) `vite-plugin-top-level-await@1.6.0`
  hard-`require("rollup")`s at load, but vitest 4.1.9 pulls **Vite 8 (rolldown, no classic `rollup`)**, so
  importing it crashes config load. **`vite-plugin-wasm` alone** instantiates the glue and both round-trip
  tests pass. A future async-`init()` surface (the H2 fallback) would need a rolldown-compatible TLA shim.
- **The browser vitest project aliases `@paigasus/kernel` → `src/wasm.ts`** (not only `@paigasus/wasm`).
  vitest forces `node` into `resolve.conditions` for an `environment: 'node'` run, and the kernel's
  `node`-first exports map would self-resolve to `src/index.ts` (napi) — a silent false green. The alias
  forces the real browser-export entry. Round-trip fidelity was proven by a **wasm-shim-specific
  perturbation** (a `+100` in `paigasus-wasm/src/lib.rs` moved only the browser test, not the node test).
- **wasm-pack destructively cleans its `--out-dir`** (deletes `package.json`, overwrites `.gitignore`).
  The `paigasus-kernel-ts` build/test tasks therefore build into a gitignored `.wasmpack-out` scratch dir
  and copy the `paigasus_wasm*` glue back into the crate root, leaving the committed
  `package.json`/`.gitignore`/glue intact.
- **§7.6 (H2): the synchronous surface was kept.** No live consumer exists; a throwaway Next.js check had
  turbopack compiling/prerendering fine but webpack failing at static prerender — neither a hydration
  disproof. The `await init()` fallback remains documented and open for the workerd/consumer follow-up.
- **§7.1 host build/lint: PASS** (macOS) — `cargo build`/`clippy --all-targets -D warnings`/`machete` all
  green against the wasm cdylib on the host; no host-gate exclusion needed.
- **Final verification:** `moon ci :build`/`:test --include-relations` green (`paigasus-kernel-ts:test` =
  2/2, both the napi `node` and wasm `browser` vitest projects); `repo:affected-smoke` PASS (incl. the new
  `binding-oneway-wasm` case and `paigasus-wasm-rs` in `kernel->bindings`); `repo:machete`/`repo:deny`/
  `cargo fmt`/`clippy`/`nextest` green.
