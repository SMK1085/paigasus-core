// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from 'vitest/config';

// BOTH condition lists are required, and the ssr one is what actually governs a Node-environment
// test. vitest 5 resolves a Node project's imports through ssr.resolve.conditions, NOT the
// top-level resolve.conditions (MEASURED on 5.0.0, SMA-502). src/server.ts opens with
// `import 'server-only'`, whose exports map is { "react-server": "./empty.js", "default":
// "./index.js" } — and index.js is an unconditional throw. Without the react-server condition
// every test that touches the server entry dies at import.
//
// The list is ADDITIVE and must keep node/import/default: a bare ['react-server'] drops them and
// breaks source-exports .ts resolution for every @paigasus/* package.
const conditions = ['react-server', 'node', 'import', 'default'];

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
    // The container-backed suites live under tests/containers/ and run only in the test-e2e task.
    exclude: ['tests/containers/**', 'tests/e2e/**', '**/node_modules/**'],
  },
  resolve: { conditions },
  ssr: { resolve: { conditions } },
});
