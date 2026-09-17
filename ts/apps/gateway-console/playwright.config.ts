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
  // CI ONLY. iam-console's `moon ci` run showed flake shapes under CI's concurrent load (Rust
  // compiles plus several `test-e2e` tasks at once). A local run never sees this load, so local
  // stays 0 retries and 60 s: raising the values there would only hide a real local regression
  // behind a retry. A test that needs a CI retry to pass is reported FLAKY, not just PASSED, so a
  // genuine regression still shows in the report.
  retries: isCI ? 2 : 0,
  reporter: 'list',
  timeout: isCI ? 120_000 : 60_000,
  // OUTSIDE the app directory. The app directory is Tailwind's scan root, the tailwind-source guard
  // walks all of it, and the Moon `test` inputs hash tests/**: none of them may see run artifacts.
  outputDir: path.join(os.tmpdir(), 'gateway-console-e2e-results'),
  // CI ONLY, same reasoning as `timeout` above: a `waitFor`/`toBeVisible` bounded by the 5 s
  // default can fail while the page is still hydrating on a loaded CI runner.
  expect: { timeout: isCI ? 15_000 : 5_000 },
  use: {
    trace: 'retain-on-failure',
    // The terminator and the fake IdP use a self-signed test certificate.
    ignoreHTTPSErrors: true,
  },
  // TWO projects (SMA-512 PR4 task 3, ruling D20): the single-zone rows run one server on the
  // memory store, and the two-zone rows run two servers, a Redis container and a forwarder. Forcing
  // the single-zone rows through the two-zone fixture would make every one of them pay for a
  // container, so they stay on separate fixtures, selected by file name (ruling D21 keeps the specs
  // flat in tests/e2e/, so tests/unit/e2e-rows.test.ts's non-recursive scan still sees them).
  //
  // BASENAME-anchored, not path-anchored (ruling P2): Playwright matches `testMatch` against the
  // file's ABSOLUTE path, and this very branch is named `feature/sma-512-two-zone-tier` — a checkout
  // or worktree directory whose path contains "two-zone" would make `/^(?!.*two-zone).*\.spec\.ts$/`
  // exclude EVERY single-zone spec in silence, a false green of exactly the kind this pull request
  // exists to remove. `[\\/]` anchors each pattern on the path SEPARATOR before the basename, so only
  // the FILE NAME is tested for the "two-zone" prefix, never any ancestor directory.
  //
  // BOTH patterns use `[^\\/]*`, never a bare `.*`, in the basename segment. `.*` spans path
  // separators, so `/[\\/]two-zone.*\.spec\.ts$/` (the first cut of this line, fixed in review round
  // 1) matched any absolute path with "two-zone" ANYWHERE in it — for example a worktree directory
  // named after this branch, `.../worktrees/two-zone-tier/.../login.spec.ts` — which would have put
  // every single-zone spec into the two-zone project too, running each twice against two different
  // stacks. `[^\\/]*` cannot cross a separator, so only the actual FILE NAME is tested. Verified:
  //   /[\\/]two-zone[^\\/]*\.spec\.ts$/.test('/Users/x/worktrees/two-zone-tier/.../login.spec.ts') === false
  //   /[\\/](?!two-zone)[^\\/]*\.spec\.ts$/.test('/Users/x/worktrees/two-zone-tier/.../login.spec.ts') === true
  //
  // ONE CAVEAT (also ruling P2's siblings): Playwright tears worker fixtures down when the worker
  // ENDS, not between files, so with workers: 1 both stacks can be alive at once. That is safe here
  // — every port is ephemeral and every fake binds 127.0.0.1:0 — but the two-zone project's container
  // may outlive the last two-zone spec by the length of the single-zone project. Do not "fix" that by
  // sharing one fixture: paying for a container on every single-zone row is worse.
  projects: [
    { name: 'single-zone', testMatch: /[\\/](?!two-zone)[^\\/]*\.spec\.ts$/, use: { ...devices['Desktop Chrome'] } },
    { name: 'two-zone', testMatch: /[\\/]two-zone[^\\/]*\.spec\.ts$/, use: { ...devices['Desktop Chrome'] } },
  ],
});
