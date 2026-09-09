// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from 'vitest/config';

// The container-backed suites, run only by the `test-e2e` Moon task. They live behind a SEPARATE
// config rather than a CLI path filter on the default one: vitest applies a positional path as a
// filter AFTER `exclude`, so `vitest run --config vitest.config.ts tests/containers/` would match
// zero files and pass vacuously. A suite that silently runs nothing is worse than a red.
//
// The condition lists mirror vitest.config.ts — see the comment there for why both are needed and
// why the list must stay additive.
const conditions = ['react-server', 'node', 'import', 'default'];

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/containers/**/*.test.ts'],
    testTimeout: 120_000,
    hookTimeout: 300_000,
  },
  resolve: { conditions },
  ssr: { resolve: { conditions } },
});
