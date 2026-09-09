// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// The container-backed suites, run only by the `test-e2e` Moon task. They live behind a SEPARATE
// config rather than a CLI path filter on the default one: vitest applies a positional path as a
// filter AFTER `exclude`, so `vitest run --config vitest.config.ts tests/containers/` would match
// zero files and pass vacuously. A suite that silently runs nothing is worse than a red.
//
// The condition list and the `server-only` alias mirror vitest.config.ts — see the comment there
// for why `react-server` was removed (task 10: it makes `server-only` a no-op but simultaneously
// breaks `react`'s own conditional export map, which `next/navigation` and `react-dom/client` both
// need intact) and why the stub is a permanent alias rather than a per-file mock (task 10 review
// round 1). None of the container suites touch `src/server.ts` or `next/navigation` today, but
// keeping the two files' resolution config in sync avoids re-discovering the same trap here later.
const conditions = ['node', 'import', 'default'];

const serverOnlyStub = fileURLToPath(new URL('./tests/support/server-only-stub.ts', import.meta.url));

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/containers/**/*.test.ts'],
    testTimeout: 120_000,
    hookTimeout: 300_000,
  },
  resolve: { conditions, alias: { 'server-only': serverOnlyStub } },
  ssr: { resolve: { conditions } },
});
