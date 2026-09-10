// SPDX-License-Identifier: Apache-2.0
//
// The browser tier (SMA-509): a real Chromium proving the hit-testing property jsdom cannot
// model — see tests/browser/capability-hit-test.spec.ts's header. No `webServer` here: each spec
// starts and stops its own ephemeral-port fixture server (tests/browser/fixture-server.ts)
// directly, since this suite needs no shared external infrastructure the way @paigasus/auth's
// Keycloak-backed E2E tier does.
import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests/browser',
  // NOT fullyParallel, and a single worker: `fullyParallel: true` runs the two tests in this one
  // file in SEPARATE worker processes, each calling `startFixtureServer` — two concurrent Vite
  // dev servers both try to bind Vite's default HMR websocket port and one loses the race
  // (MEASURED: "WebSocket server error: Port 24678 is already in use"), harmless here only because
  // `hmr: false` still lets the losing server serve requests. One worker avoids the race outright.
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: 'list',
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
