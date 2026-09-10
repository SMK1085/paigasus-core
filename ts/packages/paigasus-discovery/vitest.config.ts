// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// NO `react-server` CONDITION. See tests/support/server-only-stub.ts for the measured reason.
// The list stays ADDITIVE: dropping `import`/`default` breaks source-exports `.ts` resolution
// for every @paigasus/* package.
const conditions = ['node', 'import', 'default'];

const serverOnlyStub = fileURLToPath(new URL('./tests/support/server-only-stub.ts', import.meta.url));

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
    // The container-backed suites live under tests/containers/ and run only in the test-e2e task.
    exclude: ['tests/containers/**', '**/node_modules/**'],
    setupFiles: ['./tests/setup.ts'],
    // React Testing Library registers its automatic cleanup only when a global afterEach exists,
    // and vitest defaults globals to false. Without this the DOM accumulates between tests and
    // produces duplicate-id failures belonging to an EARLIER test (the trap @paigasus/ui records).
    globals: true,
  },
  resolve: { conditions, alias: { 'server-only': serverOnlyStub } },
  ssr: { resolve: { conditions } },
});
