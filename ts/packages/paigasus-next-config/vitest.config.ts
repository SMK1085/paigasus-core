// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts'],
  },
  // `src/runtime.ts` imports `server-only`, whose exports map resolves to a module that throws
  // unconditionally under every condition except `react-server` (measured on server-only@0.0.1:
  // "." -> { "react-server": "./empty.js", "default": "./index.js" }, and index.js is one throw).
  // Vitest does not set that condition, so without this the whole suite fails at import.
  //
  // The remaining three conditions are the additive module-resolution defaults. Listing
  // `react-server` ALONE would drop `import`/`default` and break source-exports `.ts` resolution
  // for the workspace packages — the same trap ts/packages/paigasus-kernel/vitest.config.ts
  // records for its browser project.
  resolve: {
    conditions: ['react-server', 'node', 'import', 'default'],
  },
  // Vitest 5.0.0 (bumped from 4.1.11 the same day as the design spec) resolves a Node-environment
  // test file's imports through `ssr.resolve.conditions`, not the top-level `resolve.conditions`
  // above — measured: `resolve.conditions` alone still hits `server-only`'s unconditional throw,
  // and `ssr.resolve.conditions` alone (with the top-level block removed) is sufficient. Both
  // blocks are kept: this one is what actually governs resolution for this package's `environment:
  // 'node'` tests, and the top-level one is left in place per the design spec's own guidance for
  // any future browser-mode project in this file (see the comment above).
  ssr: {
    resolve: {
      conditions: ['react-server', 'node', 'import', 'default'],
    },
  },
});
