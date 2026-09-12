// SPDX-License-Identifier: Apache-2.0
//
// The e2e stack, ONE per worker (workers: 1, so one per run; Playwright starts a new worker, and
// with it a new stack, after a failed test):
//
//   browser --https--> TLS terminator --http--> standalone server.js --h2c--> fake IAM (gRPC)
//                                                    |  \--http--> fake IAM (GET /v1/service-info)
//                                                    \--https--> fake IdP (NODE_EXTRA_CA_CERTS)
//
// The terminator forwards Host and sets X-Forwarded-Proto: https, so PAIGASUS_PUBLIC_ORIGIN is a
// real https origin, the __Host- cookies work, and Next's Server Action origin check passes.
import { spawn, type ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:net';
import path from 'node:path';
import { test as base } from '@playwright/test';
import { startFakeIam, type FakeIam } from '../../support/fake-iam';
import { startFakeIdp, type FakeIdp } from '../../support/fake-idp';
import { testTls } from '../../support/tls';
import { startTlsTerminator } from '../../support/tls-terminator';
import { STANDALONE_APP_DIR } from './paths';
import { DEFAULT_DESCRIPTOR, worldHandlers, type WorldOptions } from './world';

const READY_TIMEOUT_MS = 60_000;
const PROBE_TIMEOUT_MS = 2_000;
const STOP_TIMEOUT_MS = 5_000;
const MAX_START_ATTEMPTS = 3;

export type Harness = {
  readonly origin: string;
  readonly iam: FakeIam;
  readonly idp: FakeIdp;
  /** `origin + path`. `path` is the full path the browser sees, for example `/iam/orgs`. */
  url(path: string): string;
  /** Everything the server wrote to stdout and stderr since it started. */
  serverOutput(): string;
  /** Script the fake IAM for the current test. The auto fixture below resets it before each test. */
  useWorld(options?: WorldOptions): void;
};

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
      probe.close(() => resolve(address.port));
    });
  });
}

async function stop(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, 'exit').then(() => 'exited' as const);
  child.kill('SIGTERM');
  let timeoutHandle: NodeJS.Timeout | undefined;
  const timeout = new Promise<'timeout'>((resolve) => {
    timeoutHandle = setTimeout(() => resolve('timeout'), STOP_TIMEOUT_MS);
  });
  try {
    if ((await Promise.race([exited, timeout])) === 'timeout') {
      child.kill('SIGKILL');
      await exited;
    }
  } finally {
    clearTimeout(timeoutHandle);
  }
}

/** 'ready', or 'exited' when the process died first (a port race: the caller retries). */
async function waitForHealth(url: string, child: ChildProcess, output: () => string): Promise<'ready' | 'exited'> {
  const deadline = Date.now() + READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) return 'exited';
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(PROBE_TIMEOUT_MS) });
      if (response.ok) return 'ready';
      // A 500 here is a config parse failure at first request. It never recovers: fail now.
      if (response.status >= 500) throw new Error(`GET ${url} answered ${String(response.status)}\n--- server output ---\n${output()}`);
    } catch (error) {
      if (error instanceof Error && error.message.startsWith('GET ')) throw error;
      // Not listening yet, or the probe timed out.
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`the iam-console server did not answer ${url} within ${String(READY_TIMEOUT_MS)} ms\n--- server output ---\n${output()}`);
}

/**
 * The parent's env minus anything that could leak into or mis-configure the server.
 *
 * NODE_ENV is STATED, not inherited: this is a production build, and the parent runs under the
 * Playwright runner. Next's type augmentation also makes the key required on ProcessEnv.
 */
function serverEnv(values: Readonly<Record<string, string>>): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { NODE_ENV: 'production' };
  for (const [key, value] of Object.entries(process.env)) {
    if (key === 'NODE_ENV' || key.startsWith('PAIGASUS_') || key.startsWith('__NEXT')) continue;
    env[key] = value;
  }
  return { ...env, ...values };
}

/** Runs every close step in order, also after one throws, so one failed step cannot leak the rest. */
async function closeInOrder(steps: readonly (() => Promise<void>)[]): Promise<void> {
  const failures: unknown[] = [];
  for (const step of steps) {
    try {
      await step();
    } catch (error) {
      failures.push(error);
    }
  }
  if (failures.length > 0) throw new AggregateError(failures, `${String(failures.length)} of ${String(steps.length)} close steps failed`);
}

async function startStack(): Promise<{ harness: Harness; close: () => Promise<void> }> {
  const tls = testTls();
  // Everything that started so far, NEWEST FIRST: the order to close it in. A start step that
  // throws closes all of it before the error goes up. waitForHealth throws on a 5xx answer and at
  // its timeout, and the worker fixture gets no `close` from a start that threw. Without this, the
  // server.js child, the terminator and both fakes stay alive after a failed start.
  const started: (() => Promise<void>)[] = [];
  try {
    const idp = await startFakeIdp({ cert: tls });
    started.unshift(() => idp.close());
    const iam = await startFakeIam({ handlers: worldHandlers() });
    started.unshift(() => iam.close());
    iam.setServiceInfo(DEFAULT_DESCRIPTOR);

    let output = '';
    const failures: string[] = [];
    for (let attempt = 1; attempt <= MAX_START_ATTEMPTS; attempt += 1) {
      const port = await freePort();
      const terminator = await startTlsTerminator({ target: `http://127.0.0.1:${String(port)}`, tls });
      const closeTerminator = (): Promise<void> => terminator.close();
      started.unshift(closeTerminator);
      output = '';
      const child = spawn(process.execPath, [path.join(STANDALONE_APP_DIR, 'server.js')], {
        cwd: STANDALONE_APP_DIR,
        stdio: ['ignore', 'pipe', 'pipe'],
        env: serverEnv({
          PORT: String(port),
          HOSTNAME: '127.0.0.1',
          NEXT_TELEMETRY_DISABLED: '1',
          NODE_EXTRA_CA_CERTS: tls.certPath,
          PAIGASUS_ZONE: 'iam',
          PAIGASUS_ZONES: JSON.stringify({ iam: '/iam' }),
          PAIGASUS_OIDC_ISSUER: idp.issuer,
          PAIGASUS_OIDC_CLIENT_ID: idp.clientId,
          PAIGASUS_OIDC_CLIENT_SECRET: idp.clientSecret,
          PAIGASUS_PUBLIC_ORIGIN: terminator.origin,
          PAIGASUS_SESSION_STORE: 'memory',
          PAIGASUS_SERVICES: JSON.stringify({ iam: iam.httpUrl }),
          PAIGASUS_IAM_GRPC_URL: iam.grpcUrl,
          PAIGASUS_DISCOVERY_NEGATIVE_MS: '1',
          PAIGASUS_DISCOVERY_FRESH_MS: '2',
          PAIGASUS_DISCOVERY_STALE_MS: '3',
        }),
      });
      const stopChild = (): Promise<void> => stop(child);
      started.unshift(stopChild);
      child.stdout?.on('data', (chunk: Buffer) => {
        output += chunk.toString('utf8');
      });
      child.stderr?.on('data', (chunk: Buffer) => {
        output += chunk.toString('utf8');
      });

      const state = await waitForHealth(`http://127.0.0.1:${String(port)}/iam/healthz`, child, () => output);
      if (state === 'ready') {
        const harness: Harness = {
          origin: terminator.origin,
          iam,
          idp,
          url: (fullPath) => `${terminator.origin}${fullPath}`,
          serverOutput: () => output,
          useWorld: (options = {}) => {
            iam.setHandlers(worldHandlers(options));
            iam.setServiceInfo(options.descriptor ?? DEFAULT_DESCRIPTOR);
          },
        };
        // The server, then the terminator in front of it, then the fake IAM and the fake IdP.
        return { harness, close: () => closeInOrder(started.splice(0)) };
      }
      failures.push(`attempt ${String(attempt)}: the server exited before it answered\n${output}`);
      // This attempt's server exited first (a port race). Remove its two entries (the newest two)
      // and close them. The fakes stay up for the next attempt.
      started.splice(0, 2);
      await closeInOrder([stopChild, closeTerminator]);
    }
    throw new Error(`the iam-console server failed to start after ${String(MAX_START_ATTEMPTS)} attempts:\n${failures.join('\n')}`);
  } catch (error) {
    // Close what started, then rethrow the START error: it tells why the stack is not up. A close
    // failure goes to stderr, because it must not hide the start error.
    await closeInOrder(started.splice(0)).catch((closeError: unknown) => {
      console.error('the e2e stack failed to start, and closing what had started failed too:', closeError);
    });
    throw error;
  }
}

export const test = base.extend<{ world: undefined }, { harness: Harness }>({
  harness: [
    // eslint-disable-next-line no-empty-pattern -- Playwright requires an object pattern as the first argument.
    async ({}, use) => {
      const { harness, close } = await startStack();
      try {
        await use(harness);
      } finally {
        await close();
      }
    },
    { scope: 'worker', timeout: 180_000 },
  ],
  // Before EVERY test: the default world and the default descriptor. A test that needs another
  // world calls harness.useWorld(...) itself, after this reset.
  world: [
    async ({ harness }, use) => {
      harness.useWorld();
      await use(undefined);
    },
    { auto: true },
  ],
});

export { expect } from '@playwright/test';
