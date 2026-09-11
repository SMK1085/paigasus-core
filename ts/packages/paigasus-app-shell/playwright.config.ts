// SPDX-License-Identifier: Apache-2.0
//
// The browser tier (SMA-510 spec § 10.3). tests/e2e/global-setup.ts builds the Next fixture and
// starts its standalone server; see that file for why it is not a `webServer` block.
import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e',
  // The fixture is a Next app, not a test directory.
  testIgnore: ['**/fixture/**'],
  globalSetup: './tests/e2e/global-setup.ts',
  // One worker: every spec shares ONE fixture server, and the specs count requests per page.
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: 'list',
  use: { trace: 'retain-on-failure' },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
