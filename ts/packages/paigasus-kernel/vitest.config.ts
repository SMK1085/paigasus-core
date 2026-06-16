// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// Resolve @paigasus/node-bindings to the binding crate dir (where `napi build` rewrites the .node
// + glue) instead of pnpm's frozen `file:` store copy, which is NOT refreshed by a rebuild.
const bindingDir = fileURLToPath(new URL('../../../rs/crates/bindings/paigasus-node-bindings/index.js', import.meta.url));

export default defineConfig({
  test: {
    environment: 'node',
    // Keep the native addon out of vitest's transform/bundle pipeline so the CJS loader's
    // require() of the .node goes through Node directly (vitest's `server.deps` lives under `test`).
    server: { deps: { external: [/\.node$/] } },
  },
  resolve: { alias: { '@paigasus/node-bindings': bindingDir } },
});
