// SPDX-License-Identifier: Apache-2.0
//
// The cluster tier (SMA-513 PR 3, spec § 6): Playwright against the chart installed in kind behind
// Traefik, with a real Keycloak. Run it only through `ci/kind/run.sh specs a|b`, which selects a
// project and passes the per-run credentials. It lives INSIDE tests/ so the app's typecheck and
// lint inputs (`tests/**/*`) reach it; a config at the app root would get a cached PASS.
import os from 'node:os';
import path from 'node:path';
import { defineConfig, devices } from '@playwright/test';

// OUTSIDE the app directory: the app is Tailwind's scan root and Moon hashes it.
const outputRoot = process.env.PAIGASUS_KIND_OUTPUT_DIR ?? path.join(os.tmpdir(), 'iam-console-cluster-results');

export default defineConfig({
  testDir: '.',
  testMatch: '**/*.spec.ts',
  forbidOnly: !!process.env.CI,
  fullyParallel: false,
  workers: 1,
  // 0, on purpose: the kind job is the measurement (spec § 10), and a retry would hide a flake.
  retries: 0,
  reporter: [['list'], ['html', { outputFolder: path.join(outputRoot, 'report'), open: 'never' }]],
  outputDir: path.join(outputRoot, 'results'),
  // Every limit is explicit: in Playwright 1.63 actions, navigations and `waitFor` have NO
  // default limit (ts/CLAUDE.md), so an unbounded wait would eat the step's timeout.
  timeout: 120_000,
  expect: { timeout: 15_000 },
  use: {
    baseURL: 'https://console.paigasus.test',
    // The throwaway CA from ci/kind/run.sh up.
    ignoreHTTPSErrors: true,
    actionTimeout: 15_000,
    navigationTimeout: 30_000,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    // The job never edits /etc/hosts (decision B4). Only Chromium's own resolver sees these names,
    // so no spec may use Playwright's Node-side request API (APIRequestContext) against them.
    launchOptions: {
      args: ['--host-resolver-rules=MAP console.paigasus.test 127.0.0.1, MAP idp.paigasus.test 127.0.0.1'],
    },
  },
  projects: [
    { name: 'phase-a', testDir: './phase-a', use: { ...devices['Desktop Chrome'] } },
    { name: 'phase-b', testDir: './phase-b', use: { ...devices['Desktop Chrome'] } },
  ],
});
