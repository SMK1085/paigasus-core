// SPDX-License-Identifier: Apache-2.0
//
// The e2e tier (spec § 9.4). tests/e2e/global-setup.ts checks the build; tests/e2e/support/harness.ts
// starts the fakes, the TLS terminator and the standalone server in the WORKER, because a test
// scripts the fake IAM and counts its calls, and globalSetup runs in another process.
import os from 'node:os';
import path from 'node:path';
import { defineConfig, devices } from '@playwright/test';

const isCI = !!process.env.CI;

export default defineConfig({
  testDir: './tests/e2e',
  testMatch: '**/*.spec.ts',
  // A committed test.only would drop the AC proofs from CI in silence.
  forbidOnly: !!process.env.CI,
  globalSetup: './tests/e2e/global-setup.ts',
  // One worker: every spec shares ONE server and ONE fake IAM, and the specs count calls.
  fullyParallel: false,
  workers: 1,
  // CI ONLY. PR #237's `moon ci` run showed three flake shapes under CI's concurrent load (Rust
  // compiles plus four `test-e2e` tasks at once): login.spec.ts:42 hit `net::ERR_NETWORK_CHANGED`
  // 216 ms into a `page.goto`, and capabilities.spec.ts:9 and login.spec.ts:60 each exceeded the
  // 60 s test timeout on a `waitFor`. A local run never sees this load, so local stays 0 retries
  // and 60 s: raising the values there would only hide a real local regression behind a retry. A
  // test that needs a CI retry to pass is reported FLAKY, not just PASSED, so a genuine regression
  // still shows in the report.
  retries: isCI ? 2 : 0,
  reporter: 'list',
  timeout: isCI ? 120_000 : 60_000,
  // OUTSIDE the app directory. The app directory is Tailwind's scan root, the tailwind-source guard
  // walks all of it, and the Moon `test` inputs hash tests/**: none of them may see run artifacts.
  outputDir: path.join(os.tmpdir(), 'iam-console-e2e-results'),
  // CI ONLY, same reasoning as `timeout` above: a `waitFor`/`toBeVisible` bounded by the 5 s
  // default can fail while the page is still hydrating on a loaded CI runner.
  expect: { timeout: isCI ? 15_000 : 5_000 },
  use: {
    trace: 'retain-on-failure',
    // The terminator and the fake IdP use a self-signed test certificate.
    ignoreHTTPSErrors: true,
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
