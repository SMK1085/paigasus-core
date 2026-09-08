// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts'],
    // Each case boots a real Next standalone server twice; the default 5s is not enough.
    testTimeout: 120_000,
    hookTimeout: 120_000,
  },
});
