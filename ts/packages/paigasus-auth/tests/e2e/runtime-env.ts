// SPDX-License-Identifier: Apache-2.0
//
// Bridges tests/e2e/global-setup.ts (which starts Keycloak and Redis) to two OTHER processes that
// need their connection details: the fixture-server.ts child process Playwright's `webServer`
// spawns, and the spec files themselves (recovery.spec.ts reaches Redis directly). Playwright
// workers and the webServer command are separate OS processes from the one globalSetup runs in,
// so an in-memory value cannot cross that boundary — this writes a small JSON file to a FIXED
// path instead, outside the repo (`os.tmpdir()`, never committed, no .gitignore entry needed).
//
// A FIXED filename, not a per-run random one: the fixture server and the specs have no other
// channel to learn a random name. This is a test-only fixture on a single developer/CI machine, so
// the (small) risk of two concurrent `playwright test` runs colliding on this file is accepted
// rather than engineered around.
import { readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export const RUNTIME_ENV_PATH = path.join(os.tmpdir(), 'paigasus-auth-e2e-runtime.json');

export interface RuntimeEnv {
  /** e.g. https://127.0.0.1:<mapped-port>/realms/paigasus-test */
  issuer: string;
  /** e.g. redis://127.0.0.1:<mapped-port>/0 */
  redisUrl: string;
  /** The self-signed cert's path (tls-fixture.ts) — informational; NODE_EXTRA_CA_CERTS is set
   * directly by playwright.config.ts, since that env var must be present before the fixture
   * server process starts (see tls-fixture.ts's own header). */
  certPath: string;
}

export function writeRuntimeEnv(env: RuntimeEnv): void {
  writeFileSync(RUNTIME_ENV_PATH, JSON.stringify(env, null, 2));
}

export function readRuntimeEnv(): RuntimeEnv {
  return JSON.parse(readFileSync(RUNTIME_ENV_PATH, 'utf8')) as RuntimeEnv;
}

/** Non-throwing: fixture-server.ts polls with this until global-setup.ts has written the file. */
export function tryReadRuntimeEnv(): RuntimeEnv | undefined {
  try {
    return readRuntimeEnv();
  } catch {
    return undefined;
  }
}

export function clearRuntimeEnv(): void {
  rmSync(RUNTIME_ENV_PATH, { force: true });
}
