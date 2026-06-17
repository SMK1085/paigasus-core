// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig, type Plugin } from 'vitest/config';
import wasm from 'vite-plugin-wasm';

// Resolve each binding to a fresh build artifact (NOT pnpm's frozen `file:` store copy, which a
// rebuild doesn't refresh — SMA-420 store-copy staleness). The napi .node is aliased to the crate
// dir (where `napi build` rewrites it). The wasm glue is aliased to the test task's OWN scratch
// out-dir (`.wasmpack-test-out`): the `paigasus-kernel-ts:test` Moon task builds wasm there rather
// than copying into the crate dir, so it doesn't race the concurrent `build` task's crate-dir copy
// (SMA-427 CI). See ts/packages/paigasus-kernel/moon.yml.
const nodeBindingDir = fileURLToPath(new URL('../../../rs/crates/bindings/paigasus-node-bindings/index.js', import.meta.url));
const wasmBindingDir = fileURLToPath(new URL('../../../rs/crates/bindings/paigasus-wasm/.wasmpack-test-out/paigasus_wasm.js', import.meta.url));

// The browser project must load @paigasus/kernel's `browser` export (src/wasm.ts → @paigasus/wasm),
// but vitest forces `node` into resolve.conditions for an `environment: 'node'` run, and the kernel's
// self-referencing exports map declares `node` first — so condition order alone resolves the package
// to src/index.ts (the napi path). Alias @paigasus/kernel straight to its browser-export entry so the
// round-trip provably crosses the WASM boundary (src/wasm.ts is exactly what the `browser` condition
// points at); @paigasus/wasm under it is then aliased to the fresh scratch glue (SMA-427 M4).
const kernelWasmEntry = fileURLToPath(new URL('./src/wasm.ts', import.meta.url));

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
        // browser/wasm path: `browser` + the additive module-resolution defaults (NOT a bare
        // ['browser'] — that drops module/import and breaks source-exports `.ts` resolution —
        // SMA-427 M4), vite-plugin-wasm to instantiate the bundler-target `.wasm`, the
        // @paigasus/kernel→src/wasm.ts alias (see the kernelWasmEntry note above), and the
        // @paigasus/wasm crate-dir alias for fresh glue.
        //
        // vite-plugin-top-level-await is intentionally NOT used: the plan paired it with
        // vite-plugin-wasm, but (a) the `--target bundler` glue is synchronous — `import * as wasm`
        // + a sync `__wbindgen_start()`, NO top-level await — so it is unnecessary here, and (b)
        // vite-plugin-top-level-await@1.6.0 hard-`require("rollup")`s at load, while vitest 4.1.9
        // pulls Vite 8 (rolldown, no classic `rollup`), so importing it crashes config load
        // (`Cannot find module 'rollup'`). vite-plugin-wasm alone instantiates the glue and both
        // round-trip tests pass. If a future kernel fn forces async wasm init, revisit with a
        // rolldown-compatible TLA shim (SMA-427).
        // vite-plugin-wasm's factory is typed `() => any`; cast to vite's `Plugin` (re-exported by
        // vitest/config — `vite` itself isn't a direct dep here) so the typed-ESLint
        // no-unsafe-assignment rule passes without a disable.
        plugins: [wasm() as Plugin],
        test: {
          name: 'browser',
          environment: 'node',
          include: ['tests/sum.wasm.test.ts'],
        },
        resolve: {
          conditions: ['browser', 'module', 'import', 'default'],
          alias: {
            '@paigasus/kernel': kernelWasmEntry,
            '@paigasus/wasm': wasmBindingDir,
          },
        },
      },
    ],
  },
});
