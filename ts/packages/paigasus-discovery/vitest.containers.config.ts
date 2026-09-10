// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// A SEPARATE config, not a CLI path filter on the default one. Under vitest 5.0.0, a positional
// path filter on the default config (`vitest run --config vitest.config.ts tests/containers/`)
// does not pass vacuously — it exits 1 with "No test files found" — but the separate-config design
// is right regardless: this suite needs its own environment, timeouts and `cache: false`, none of
// which a CLI filter on the default config could supply.
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
