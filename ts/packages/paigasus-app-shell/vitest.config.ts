// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    name: 'app-shell',
    environment: 'jsdom',
    setupFiles: ['./tests/setup.ts'],
    include: ['tests/**/*.test.{ts,tsx}'],
    // The browser tier (tests/e2e/**) runs under Playwright in the test-e2e task, never here. The
    // fixture's .next/ tree also sits under tests/e2e/, so this exclude keeps vitest out of it.
    exclude: ['tests/e2e/**', '**/node_modules/**'],
    // React Testing Library registers its automatic cleanup only when a global afterEach exists,
    // and vitest defaults globals to false. Without this the DOM accumulates between tests and axe
    // reports duplicate-id and duplicate-landmark violations that belong to an EARLIER test — the
    // trap @paigasus/ui records.
    globals: true,
  },
});
