// SPDX-License-Identifier: Apache-2.0
//
// The e2e tier (spec § 9.4). tests/e2e/global-setup.ts checks the build; tests/e2e/support/harness.ts
// starts the fakes, the TLS terminator and the standalone server in the WORKER, because a test
// scripts the fake IAM and counts its calls, and globalSetup runs in another process.
import os from 'node:os';
import path from 'node:path';
import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e',
  testMatch: '**/*.spec.ts',
  // A committed test.only would drop the AC proofs from CI in silence.
  forbidOnly: !!process.env.CI,
  globalSetup: './tests/e2e/global-setup.ts',
  // One worker: every spec shares ONE server and ONE fake IAM, and the specs count calls.
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: 'list',
  timeout: 60_000,
  // OUTSIDE the app directory. The app directory is Tailwind's scan root, the tailwind-source guard
  // walks all of it, and the Moon `test` inputs hash tests/**: none of them may see run artifacts.
  outputDir: path.join(os.tmpdir(), 'iam-console-e2e-results'),
  use: {
    trace: 'retain-on-failure',
    // The terminator and the fake IdP use a self-signed test certificate.
    ignoreHTTPSErrors: true,
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
