# SMA-427 — Stand up the wasm kernel binding (`paigasus-wasm`) for browser/Edge + dual-export `@paigasus/kernel`

**Status:** approved design (brainstorm complete, ready for plan)
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

1. **Sync `sum(a, b): number`, bundler-instantiated (API parity with the node path).** Browser wasm
   instantiation is asynchronous, unlike the synchronous napi `.node` load — this is the one place the
   wasm binding genuinely departs from the napi mirror. We use wasm-pack `--target bundler`: the
   consumer's bundler (webpack/turbopack with `asyncWebAssembly`) instantiates the wasm, so
   `import { sum } from '@paigasus/kernel'` then `sum(2, 3)` is callable **synchronously** with the same
   `(number, number) => number` signature as the node/napi path. **Consequence:** a consuming bundler
   must enable async-wasm support — *not blocking today*, because no ts package imports `@paigasus/kernel`
   yet (verified in SMA-420), so the AC is satisfied by the export condition resolving to a real wasm
   path plus the runtime round-trip test; the Next.js console wiring is forward-looking. Rejected
   alternatives: an explicit `await init()` (diverges the browser surface from node by one step) and a
   lazy `sum(): Promise<number>` (diverges the signature).
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
# link flags. wasm-pack (proto-pinned) fetches the matching wasm-bindgen-cli for this version.
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
    "workerd": "./src/wasm.ts",
    "default": "./src/wasm.ts"
  }
}
```

```typescript
// src/wasm.ts  (browser/workerd/default condition — keeps its SPDX header)
export { sum } from '@paigasus/wasm';
```

- **Delete `src/unsupported.ts`** — its throwing stub is replaced by the real wasm path. `src/index.ts`
  (the napi re-export) is unchanged.
- The `@paigasus/wasm` `file:` specifier is the wasm analog of the napi `file:` link — a cross-`ts/` link,
  not a pnpm workspace member. pnpm does not install a `file:` dep's devDeps, so the build tooling
  (`vite-plugin-wasm` etc.) is declared on `@paigasus/kernel`, not inherited (the SMA-420 spike S2
  lesson). wasm-pack itself is proto-pinned (not a pnpm dep), so it is not subject to this.
- **`_comment_exports`** is updated: the Node-only caveat is gone; the `node` path loads a compiled
  `.node`, the `browser`/`workerd`/`default` path loads a `.wasm`. Conditions still point at **source**
  until tsup/dist lands (in lockstep with flipping `private: false`).
- **`tsconfig.json`**: keep `customConditions: ["node"]` — the self-referential `@paigasus/kernel` type
  resolution stays on the napi surface. `src/wasm.ts` is type-checked anyway because it is in `include`
  and imports `@paigasus/wasm` directly (resolved via the committed `.d.ts` glue). Both `sum` surfaces
  share the `(number, number) => number` signature, so the type view is consistent regardless of which
  condition is checked.

## 3. Build tooling

- **`rs/rust-toolchain.toml`**: add `targets = ["wasm32-unknown-unknown"]`. rustup reads this file on every
  cargo invocation and auto-installs the target — CI-friendly (no separate `rustup target add` step), the
  same mechanism that already pins the channel/components.
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
  - **`outputs`**: add the committed wasm glue (and the crate-dir `*.wasm` if Moon should cache it).
  - Rationale is identical to the napi tasks (ordering + cache-bust in one step), just doubled. The
    double-tool-build is the deliberate cost of a single package that ships both bindings.
- **No new `paigasus-wasm-ts` Moon project** — the `.wasm` + glue are built as part of the
  `paigasus-kernel-ts` build chain, exactly as the `.node` is (and as the py wheel was for
  `paigasus-kernel-py`). The graph stays `kernel-rs → {node,wasm}-bindings-rs → kernel-ts`.

## 5. Public surface & runtime smoke test

- **`ts/packages/paigasus-kernel/vitest.config.ts`**: define **two vitest projects**:
  - the existing **node** project (`resolve.conditions` default / `node`, the `@paigasus/node-bindings`
    crate-dir alias) running `tests/sum.test.ts` against the napi path (unchanged behavior);
  - a new **browser** project with `resolve.conditions: ['browser']`, the `vite-plugin-wasm` +
    `vite-plugin-top-level-await` plugins, and an alias `@paigasus/wasm` → the crate-dir glue (the
    pnpm `file:`-store-copy staleness fix, mirroring the napi alias), running the new wasm test.
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

## 7. Primary risk — spike first

The wasm orchestration across the `rs/` ↔ `ts/` boundary is the real unknown. The first implementation
step is a throwaway spike proving the chain end-to-end on the user's macOS host, checking **all** of:

1. **Host workspace gates vs a wasm crate (the issue's explicit concern).** Does `cargo build --workspace`
   / `cargo clippy` / `cargo nextest` tolerate the wasm-bindgen cdylib on the **host** target?
   - *Expected:* yes. clippy doesn't link; `test = false`/`doctest = false` makes nextest a no-op for the
     crate; and the host cdylib link is covered by the same undefined-symbol deferral the napi/pyo3
     cdylibs already rely on (macOS `-undefined dynamic_lookup` in `rs/.cargo/config.toml`, Linux's
     default tolerance of undefined symbols in shared objects) — `__wbindgen_*` defers like `_Py*`/`napi_*`.
     The result is a useless host cdylib that is never shipped (the wasm32 build in the moon task is the
     real artifact), but the gates stay green with no special handling.
   - *Fallback if the host build fails to compile/link:* exclude `paigasus-wasm` from the host workspace
     gates (e.g. `--exclude paigasus-wasm`, or per-target config) and build it only for
     `wasm32-unknown-unknown` in its moon task. Record which path the spike confirms.
2. **wasm-pack invocation + release-asset shape.** wasm-pack runs from the crate dir against the shared
   `rs/target/` workspace; `--no-pack` + hand-written `package.json` resolves via the `file:` link; glue
   lands in the crate dir. Confirm the wasm-pack proto plugin's `exe-path`/asset matrix (the
   `cargo-machete.toml`-vs-`release-plz.toml` nesting question) and that `cargo` + the wasm32 target are
   provisioned when the build triggers (CI: proto install + rust-toolchain.toml target; locally: after
   `proto install`).
3. **vitest + `vite-plugin-wasm` instantiation.** A Node `browser`-condition vitest project actually
   instantiates the `--target bundler` glue (via `vite-plugin-wasm` [+ top-level-await]) and
   `sum(2, 3) === 5`. Confirm the two-projects split keeps the node/napi test on the `node` condition.
4. **Freshness / cache-bust on a Rust edit.** A kernel- or wasm-source edit re-runs the wasm-pack
   **compile** (the `touch` mtime fix) rather than asserting against a stale `.wasm`/glue, and the
   crate-dir alias loads the fresh glue rather than pnpm's frozen `file:` store copy (the napi store-copy
   staleness, in wasm form).
5. **`wasm-opt` off + binaryen.** Confirm `wasm-opt = false` keeps the build from downloading binaryen
   (no unpinned network dependency), and the unoptimized `.wasm` instantiates fine for the placeholder.

## 8. ADR note (AC #3)

ADR-0005 already names `paigasus-wasm` and decides the napi/wasm hybrid, so **no new ADR**. AC #3
("binding tool/approach recorded") is satisfied by a short note appended to ADR-0005 recording that
**browser/Edge is bound via wasm-bindgen, built with wasm-pack (`--target bundler`), as the second
TS-facing binding** (alongside the napi-first note from SMA-420), with a pointer to this spec. Recorded in
Notion, where the ADRs live.

## Verification (maps to acceptance criteria)

1. **AC #1** — `paigasus-wasm` wraps `paigasus_kernel::sum`; `@paigasus/kernel`'s
   `browser`/`workerd`/`default` export condition resolves to the real wasm path (`src/unsupported.ts`
   deleted); the browser-condition vitest round-trip (`import { sum } from '@paigasus/kernel'` →
   `sum(2,3) === 5`) passes; `cargo machete` / `cargo deny` stay green over `rs/`.
2. **AC #2** — `moon ci :build`/`:test` cascade a kernel edit to `paigasus-wasm-rs` and
   `paigasus-kernel-ts` under `--include-relations`; `moon run repo:affected-smoke` passes with the
   extended `kernel->bindings` set + new `binding-oneway-wasm` case; `--negative-control` still fails red;
   existing gates (including the napi path) unaffected.
3. **AC #3** — binding tool/approach (wasm-bindgen via wasm-pack, `--target bundler`) recorded as a note
   on ADR-0005.
4. **Cross-stack isolation preserved** — a kernel edit does not drag in `contracts`, the `*-py` packages
   other than `paigasus-kernel-py`, or the `-ts` packages other than `paigasus-kernel-ts`.

## Out of scope (deferred, with follow-ups)

- **workerd/Edge independently verified** — the `workerd` condition points at the same bundler artifact
  but is not separately exercised (no live consumer yet). If a Cloudflare-Workers/Edge consumer lands and
  needs a different instantiation path, that is its own issue.
- **Cross-target prebuild matrix + npm publish** — `private: false` / version off `0.0.0` for
  `@paigasus/kernel`/`@paigasus/wasm`/`@paigasus/node-bindings`, the wasm analog of the deferred napi
  prebuild + Python wheel publish (ADR-0006, SMA-376/407). Single-host build only here.
- **Real kernel domain logic** — `sum` stays the deliberate placeholder.
- **tsup/dist build** — export conditions still point at source; they flip to `./dist/*` in lockstep with
  `private: false` when tsup wiring lands.
- **Affected-graph completeness meta-check** — already tracked from SMA-420 F4; not folded in here.
