// SPDX-License-Identifier: Apache-2.0
//
// The E2E tier (task 13): a real Chromium browser driving a real Keycloak, through the fixture
// server (tests/e2e/fixture-server.ts). See tests/e2e/global-setup.ts's header for why Keycloak
// and Redis are started HERE, at module-evaluation time, rather than only inside the `globalSetup`
// hook the brief originally called for — Playwright starts `webServer` before it ever runs
// `globalSetup`, so the fixture server (which cannot open its port without an issuer and a Redis
// URL) would deadlock waiting on a hook that has not run yet. `startE2eInfrastructure` is memoised,
// so `globalSetup` below still runs and is still what registers the teardown Playwright calls
// after the whole test run.
import { defineConfig, devices } from '@playwright/test';
import { startE2eInfrastructure } from './tests/e2e/global-setup.js';
import { FIXTURE_SERVER_ORIGIN, ZONE_BASE_PATH } from './tests/e2e/constants.js';

const { env: e2eEnv } = await startE2eInfrastructure();

const tsEsmLoaderUrl = new URL('./tests/fixtures/ts-esm-loader.mjs', import.meta.url).href;
const e2eLoaderUrl = new URL('./tests/e2e/e2e-loader.mjs', import.meta.url).href;

export default defineConfig({
  testDir: './tests/e2e',
  // Single worker, deliberately: this suite drives one shared Keycloak + Redis + fixture server
  // triple per run, not a fixture per test, and Keycloak's own login form is not built for
  // high-concurrency automated form submission. The one test that NEEDS concurrency (§ 9.2's two
  // tabs) creates it itself, inside a single test, via two `Page`s in one browser context.
  workers: 1,
  fullyParallel: false,
  retries: 0,
  reporter: 'list',
  use: {
    baseURL: FIXTURE_SERVER_ORIGIN,
    // The self-signed Keycloak cert (M3/M11) — the browser navigates to Keycloak's own
    // origin directly (the authorization endpoint, the login form), so this is what lets that
    // navigation proceed instead of showing Chromium's interstitial warning page.
    ignoreHTTPSErrors: true,
    trace: 'retain-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  globalSetup: './tests/e2e/global-setup.ts',
  webServer: {
    command: `node --experimental-transform-types --import "${tsEsmLoaderUrl}" --import "${e2eLoaderUrl}" tests/e2e/fixture-server.ts`,
    url: `${FIXTURE_SERVER_ORIGIN}${ZONE_BASE_PATH}/`,
    // Keycloak and Redis are already up by the time this process is launched (see the file
    // header) — this is a generous ceiling for the fixture server's own startup, not a budget
    // that needs to cover container boot.
    timeout: 30_000,
    reuseExistingServer: false,
    stdout: 'pipe',
    stderr: 'pipe',
    env: {
      // M3: trusts Keycloak's self-signed cert for the fixture server's own openid-client
      // discovery/token/end-session calls. Read lazily by Node's TLS layer on first use, not at
      // process bootstrap, so the file existing by the time this env var is DEFINED (rather than
      // by the time the process STARTS) is not required either way — see tls-fixture.ts.
      NODE_EXTRA_CA_CERTS: e2eEnv.certPath,
    },
  },
});
