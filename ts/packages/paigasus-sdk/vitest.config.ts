// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts'],
  },
  // `src/server-guard.ts` imports `server-only`, whose exports map is
  // { "react-server": "./empty.js", "default": "./index.js" } — and index.js is one unconditional
  // throw. Vitest does not set the `react-server` condition, so without this every test in the
  // package dies at import. The fix is the condition, NEVER deleting the import: that line is the
  // structural guard keeping this package out of client bundles.
  //
  // The other three entries are the additive module-resolution defaults. `react-server` ALONE
  // would drop `import`/`default` and break source-exports `.ts` resolution for @paigasus/proto —
  // the same trap ts/packages/paigasus-kernel/vitest.config.ts records for its browser project.
  resolve: {
    conditions: ['react-server', 'node', 'import', 'default'],
  },
  // Vitest 5 resolves a Node-environment test file's imports through `ssr.resolve.conditions`, NOT
  // the top-level `resolve.conditions` above — MEASURED on 5.0.0 in SMA-502. Setting only the
  // top-level key has no effect on an `environment: 'node'` project. Both blocks are kept: this one
  // governs resolution for this package's tests, and the top-level one is what a future
  // browser-mode project in this file would read.
  ssr: {
    resolve: {
      conditions: ['react-server', 'node', 'import', 'default'],
    },
  },
});
