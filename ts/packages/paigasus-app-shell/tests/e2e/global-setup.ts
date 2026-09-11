// SPDX-License-Identifier: Apache-2.0
//
// Builds the Next fixture and starts its standalone server, ONCE for the whole Playwright run.
//
// WHY HERE, AND NOT IN moon.yml OR A `webServer` BLOCK (measured in the SMA-510 spike):
// - The server must be a child of the SAME Node process that stops it. A shell `&` in a Moon
//   script left an orphan server behind.
// - `webServer` needs its port when playwright.config.ts is evaluated, and Playwright evaluates
//   that file again in each worker process, so a free port chosen there differs per process.
//   globalSetup runs once, before any worker starts, and Playwright documents that a value it
//   writes to process.env reaches the workers.
//
// A PRODUCTION build on purpose (spec § 10.3): next/link prefetches differently under `next dev`.
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { cpSync, existsSync, rmSync } from 'node:fs';
import { createServer } from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const PACKAGE_DIR = fileURLToPath(new URL('../..', import.meta.url));
const FIXTURE_DIR = fileURLToPath(new URL('./fixture', import.meta.url));
// createNextConfig pins outputFileTracingRoot to ts/, so Next writes the standalone app under the
// fixture's path RELATIVE TO ts/ (measured in the spike).
const STANDALONE_APP_DIR = path.join(FIXTURE_DIR, '.next', 'standalone', 'packages', 'paigasus-app-shell', 'tests', 'e2e', 'fixture');
const SERVER_READY_TIMEOUT_MS = 30_000;
const STOP_TIMEOUT_MS = 5_000;
const PROBE_TIMEOUT_MS = 2_000;
const MAX_START_ATTEMPTS = 3;

// Thrown by waitForServer only when the child process exited before it answered.
// startServer uses this to tell a dead server (retry with a new port) apart from
// a slow or unresponsive one (fail as before).
class ServerExitedEarlyError extends Error {}

function buildFixture(): void {
  // Old output must not satisfy this run: the static copy below would otherwise mix two builds.
  rmSync(path.join(FIXTURE_DIR, '.next', 'standalone'), { recursive: true, force: true });
  rmSync(path.join(FIXTURE_DIR, '.next', 'static'), { recursive: true, force: true });
  // `pnpm exec next build <dir>` from the package directory is the measured command (spike).
  execFileSync('pnpm', ['exec', 'next', 'build', 'tests/e2e/fixture'], { cwd: PACKAGE_DIR, stdio: 'inherit', env: { ...process.env, NEXT_TELEMETRY_DISABLED: '1' } });
  const serverJs = path.join(STANDALONE_APP_DIR, 'server.js');
  if (!existsSync(serverJs)) {
    throw new Error(`standalone server missing at ${serverJs}: output 'standalone' did not take effect, or the tracing root moved`);
  }
  // The standalone tree has NO .next/static (measured). Without this copy, every client chunk is a
  // 404 and nothing hydrates.
  cpSync(path.join(FIXTURE_DIR, '.next', 'static'), path.join(STANDALONE_APP_DIR, '.next', 'static'), { recursive: true });
}

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const probe = createServer();
    probe.once('error', reject);
    probe.listen(0, '127.0.0.1', () => {
      const address = probe.address();
      if (address === null || typeof address === 'string') {
        probe.close();
        reject(new Error('could not read the probe port'));
        return;
      }
      probe.close(() => {
        resolve(address.port);
      });
    });
  });
}

async function waitForServer(url: string, child: ChildProcess): Promise<void> {
  const deadline = Date.now() + SERVER_READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new ServerExitedEarlyError(`the fixture server exited early with code ${String(child.exitCode)}, signal ${String(child.signalCode)}`);
    }
    try {
      // Bound each probe. An unbounded fetch would hang forever against a server that
      // accepts the TCP connection but never answers.
      const probeTimeoutMs = Math.max(1, Math.min(PROBE_TIMEOUT_MS, deadline - Date.now()));
      const response = await fetch(url, { signal: AbortSignal.timeout(probeTimeoutMs) });
      if (response.ok) return;
    } catch {
      // Not listening yet, or the probe timed out. Try again.
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`the fixture server did not answer at ${url} within ${String(SERVER_READY_TIMEOUT_MS)} ms`);
}

async function stop(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, 'exit').then(() => 'exited' as const);
  child.kill('SIGTERM');
  const timeout = new Promise<'timeout'>((resolve) => setTimeout(() => resolve('timeout'), STOP_TIMEOUT_MS));
  if ((await Promise.race([exited, timeout])) === 'timeout') child.kill('SIGKILL');
}

// Picking a free port and spawning the server is not atomic: another process can take the
// port between freePort() closing its probe and spawn() starting the server, and the child
// then exits with EADDRINUSE. Retry with a fresh port when that happens. Any other failure
// (a timeout, a bad response) is not a port race, so it is not retried.
async function startServer(): Promise<{ child: ChildProcess; origin: string }> {
  const failures: string[] = [];
  for (let attempt = 1; attempt <= MAX_START_ATTEMPTS; attempt++) {
    const port = await freePort();
    const child = spawn(process.execPath, [path.join(STANDALONE_APP_DIR, 'server.js')], {
      cwd: STANDALONE_APP_DIR,
      env: { ...process.env, PORT: String(port), HOSTNAME: '127.0.0.1', NEXT_TELEMETRY_DISABLED: '1' },
      stdio: ['ignore', 'inherit', 'inherit'],
    });
    const origin = `http://127.0.0.1:${String(port)}`;
    try {
      await waitForServer(`${origin}/iam`, child);
      return { child, origin };
    } catch (error) {
      await stop(child);
      if (error instanceof ServerExitedEarlyError) {
        failures.push(error.message);
        if (attempt < MAX_START_ATTEMPTS) continue;
        throw new Error(`the fixture server failed to start after ${String(MAX_START_ATTEMPTS)} attempts: ${failures.join('; ')}`, { cause: error });
      }
      throw error;
    }
  }
  // Every loop iteration above returns or throws, so this is unreachable at runtime.
  throw new Error(`the fixture server failed to start after ${String(MAX_START_ATTEMPTS)} attempts: ${failures.join('; ')}`);
}

export default async function globalSetup(): Promise<() => Promise<void>> {
  buildFixture();
  const { child, origin } = await startServer();
  process.env.APP_SHELL_E2E_ORIGIN = origin;
  return () => stop(child);
}
