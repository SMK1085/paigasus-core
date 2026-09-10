// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// A SEPARATE config, not a CLI path filter on the default one: vitest applies a positional path
// AFTER `exclude`, so `vitest run --config vitest.config.ts tests/containers/` matches zero files
// and passes vacuously. A suite that silently runs nothing is worse than a red.
//
// There is NO SKIP HATCH when Docker is unreachable. This suite fails loudly, the precedent
// @paigasus/auth set deliberately against paigasus-iam's silently-skipping Docker suites.
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
