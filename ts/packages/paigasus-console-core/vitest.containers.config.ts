// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// The Docker-backed tier (SMA-648 § 4.4), run only by the `test-e2e` Moon task. A SEPARATE config,
// copied from @paigasus/discovery's vitest.containers.config.ts: this suite needs its own include,
// timeouts and `cache: false`, which a CLI path filter on the default config cannot supply.
//
// The `server-only` ALIAS is what makes the stub apply, not a setup file: src/discovery.ts and
// src/logger.ts both `import 'server-only'`, whose default export is an unconditional throw. The
// alias is the same one vitest.config.ts carries.
//
// There is NO SKIP HATCH when Docker is unreachable. This suite fails loudly, the precedent
// @paigasus/auth and @paigasus/discovery set.
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
