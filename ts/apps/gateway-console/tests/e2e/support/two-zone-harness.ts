// SPDX-License-Identifier: Apache-2.0
//
// The TWO-ZONE e2e stack (SMA-512 PR4 task 3, spec § 10.5), ONE per worker (workers: 1, so one per
// run; Playwright starts a new worker, and with it a new stack, after a failed test — a globalSetup
// hook runs in a DIFFERENT process and cannot script or restart anything, so everything below lives
// in the worker fixture):
//
//   browser --https--> TLS terminator ---- /iam/*     --> counting forwarder --http--> iam-console server.js
//                                       \- /gateway/* ------------------------------> gateway-console server.js
//                                                             both --h2c--> fake IAM (gRPC)
//                                                             both --http--> fake IAM + fake gateway (service-info)
//                                                             both --https--> fake IdP
//                                                             both --------> Redis container (one session store)
//
// DOCKER IS REQUIRED AND FAILS LOUDLY. There is no skip hatch, no canary and no environment escape:
// if the daemon is unreachable, `GenericContainer(...).start()` rejects, the worker fixture rejects
// with it, and the run fails — a tier that skips when Docker is unreachable reports green having
// proved nothing (ts/packages/paigasus-discovery/vitest.containers.config.ts's own header states the
// same precedent for the same reason).
//
// WORKER-FIXTURE CONTAINER MEASUREMENT (task 3 Step 1, recorded verbatim in the task report): a
// `redis:8-alpine` container started from a Playwright WORKER fixture starts exactly once per run
// under `workers: 1`, and a forced mid-run test failure — which restarts the worker process, as
// Playwright always does after a failure — tears the first container down COMPLETELY (removed, not
// merely stopped) before the new worker's fixture starts a second one; the two never coexist. That
// is the opposite of `@paigasus/auth`'s cross-process module-eval memo, which exists because THAT
// pattern (containers started at config module-evaluation time, re-run by every worker process) can
// double-start. A worker-scoped fixture does not share that failure mode, so no deterministic label
// or stale-container sweep is implemented here — see the task report for the full measurement.
import { spawn, type ChildProcess } from 'node:child_process';
import path from 'node:path';
import { test as base } from '@playwright/test';
import { GenericContainer } from 'testcontainers';
import { startFakeGateway, startFakeIam, startFakeIdp, startTlsTerminator, testTls, type FakeGateway, type FakeIam, type FakeIdp } from '@paigasus/console-core/testing';
import { closeInOrder, freePort, serverEnv, stop, waitForHealth } from './harness';
import { startCountingForwarder, type CountingForwarder } from './counting-forwarder';
import { IAM_CONSOLE_STANDALONE_DIR, STANDALONE_APP_DIR } from './paths';
import { DEFAULT_GATEWAY_DESCRIPTOR, DEFAULT_IAM_DESCRIPTOR, worldHandlers, type WorldOptions } from './world';

// CI ONLY, same reasoning as harness.ts's own copy of these two constants: a loaded CI runner can
// stretch out both a Keycloak-class container start and a server boot alike.
const isCI = !!process.env.CI;
// These two mirror harness.ts's own module-private READY_TIMEOUT_MS/MAX_START_ATTEMPTS: they are
// NOT imported (waitForHealth already closes over its own copy in harness.ts, so this file's copy
// only has to agree on the NUMBER, for its own worker-fixture timeout budget below — it does not
// duplicate any logic).
const READY_TIMEOUT_MS = isCI ? 120_000 : 60_000;
const MAX_START_ATTEMPTS = 3;
// GenericContainer's own default startup wait is generous already; this only bounds it explicitly
// so a hung Docker daemon fails with a named timeout rather than the fixture's outer one.
const REDIS_START_TIMEOUT_MS = 120_000;

export type Zone = 'iam' | 'gateway';

export type TwoZoneHarness = {
  readonly origin: string;
  readonly iam: FakeIam;
  readonly idp: FakeIdp;
  readonly gateway: FakeGateway;
  /** The forwarder sitting between the terminator and the iam-console app (spec § 10.5, ruling P1):
   * `iamConsole.connections()` / `.requests()` are acceptance criterion 2's isolation proof. */
  readonly iamConsole: CountingForwarder;
  /** `origin + path`. `path` is the full path the browser sees, for example `/gateway/overview`. */
  url(path: string): string;
  /** Everything one zone's server wrote to stdout and stderr since it started. */
  serverOutput(zone: Zone): string;
  /** Script the fake IAM and fake gateway for the current test. The auto fixture below resets it before each test. */
  useWorld(options?: WorldOptions): void;
};

// BYTE-IDENTICAL in both zones (spec § 11): a mismatch here is exactly the bug this tier exists to
// catch, so it is built once and reused for both children, never composed per-zone.
const ZONES = JSON.stringify({ iam: '/iam', gateway: '/gateway' });

async function startStack(): Promise<{ harness: TwoZoneHarness; close: () => Promise<void> }> {
  const tls = testTls();
  // Everything that started so far, NEWEST FIRST: the order to close it in (both servers, the
  // terminator, the forwarder, the three fakes, then Redis last). A start step that throws closes
  // all of it before the error goes up.
  const started: (() => Promise<void>)[] = [];
  try {
    const redis = await new GenericContainer('redis:8-alpine').withExposedPorts(6379).withStartupTimeout(REDIS_START_TIMEOUT_MS).start();
    started.unshift(async () => {
      await redis.stop();
    });
    const redisUrl = `redis://${redis.getHost()}:${String(redis.getMappedPort(6379))}/0`;

    const idp = await startFakeIdp({ cert: tls });
    started.unshift(() => idp.close());
    const iam = await startFakeIam({ handlers: worldHandlers() });
    started.unshift(() => iam.close());
    iam.setServiceInfo(DEFAULT_IAM_DESCRIPTOR);
    const gateway = await startFakeGateway();
    started.unshift(() => gateway.close());
    gateway.setServiceInfo(DEFAULT_GATEWAY_DESCRIPTOR);

    const outputs: Record<Zone, string> = { iam: '', gateway: '' };
    const captureOutput = (zone: Zone, child: ChildProcess): void => {
      child.stdout?.on('data', (chunk: Buffer) => {
        outputs[zone] += chunk.toString('utf8');
      });
      child.stderr?.on('data', (chunk: Buffer) => {
        outputs[zone] += chunk.toString('utf8');
      });
    };

    const failures: string[] = [];
    for (let attempt = 1; attempt <= MAX_START_ATTEMPTS; attempt += 1) {
      // Allocate the ports first, then build the front, then spawn the servers onto those ports:
      // both servers need PAIGASUS_PUBLIC_ORIGIN, which is the terminator's origin, and the
      // terminator needs the servers' ports — the cycle is broken exactly as the single-zone
      // harness breaks it.
      const iamPort = await freePort();
      const gatewayPort = await freePort();
      outputs.iam = '';
      outputs.gateway = '';

      const forwarder = await startCountingForwarder({ target: `http://127.0.0.1:${String(iamPort)}` });
      const closeForwarder = (): Promise<void> => forwarder.close();
      started.unshift(closeForwarder);

      const terminator = await startTlsTerminator({
        tls,
        routes: [
          { prefix: '/iam', target: forwarder.url },
          { prefix: '/gateway', target: `http://127.0.0.1:${String(gatewayPort)}` },
        ],
      });
      const closeTerminator = (): Promise<void> => terminator.close();
      started.unshift(closeTerminator);

      const shared = serverEnv({
        NEXT_TELEMETRY_DISABLED: '1',
        NODE_EXTRA_CA_CERTS: tls.certPath,
        PAIGASUS_ZONES: ZONES,
        PAIGASUS_OIDC_ISSUER: idp.issuer,
        PAIGASUS_OIDC_CLIENT_ID: idp.clientId,
        PAIGASUS_OIDC_CLIENT_SECRET: idp.clientSecret,
        PAIGASUS_PUBLIC_ORIGIN: terminator.origin,
        // MANDATORY, not a preference (global constraints): createAuthRuntime throws on the memory
        // store when PAIGASUS_ZONES names more than one zone.
        PAIGASUS_SESSION_STORE: 'redis',
        PAIGASUS_SESSION_REDIS_URL: redisUrl,
        PAIGASUS_SERVICES: JSON.stringify({ iam: iam.httpUrl, gateway: gateway.url }),
        PAIGASUS_IAM_GRPC_URL: iam.grpcUrl,
        PAIGASUS_DISCOVERY_NEGATIVE_MS: '1',
        PAIGASUS_DISCOVERY_FRESH_MS: '2',
        PAIGASUS_DISCOVERY_STALE_MS: '3',
      });

      const iamChild = spawn(process.execPath, [path.join(IAM_CONSOLE_STANDALONE_DIR, 'server.js')], {
        cwd: IAM_CONSOLE_STANDALONE_DIR,
        stdio: ['ignore', 'pipe', 'pipe'],
        env: { ...shared, PORT: String(iamPort), HOSTNAME: '127.0.0.1', PAIGASUS_ZONE: 'iam' },
      });
      const stopIam = (): Promise<void> => stop(iamChild);
      started.unshift(stopIam);
      captureOutput('iam', iamChild);

      const gatewayChild = spawn(process.execPath, [path.join(STANDALONE_APP_DIR, 'server.js')], {
        cwd: STANDALONE_APP_DIR,
        stdio: ['ignore', 'pipe', 'pipe'],
        env: { ...shared, PORT: String(gatewayPort), HOSTNAME: '127.0.0.1', PAIGASUS_ZONE: 'gateway' },
      });
      const stopGateway = (): Promise<void> => stop(gatewayChild);
      started.unshift(stopGateway);
      captureOutput('gateway', gatewayChild);

      // Through 127.0.0.1:<port> DIRECTLY, not through the terminator, so a health failure names the
      // server rather than the front. The explicit `label` argument is what makes it name the RIGHT
      // server: with two children, harness.ts's own default label ('gateway-console') would be
      // wrong for the iam-console child.
      const iamState = await waitForHealth(`http://127.0.0.1:${String(iamPort)}/iam/healthz`, iamChild, () => outputs.iam, 'iam-console');
      const gatewayState =
        iamState === 'ready' ? await waitForHealth(`http://127.0.0.1:${String(gatewayPort)}/gateway/healthz`, gatewayChild, () => outputs.gateway, 'gateway-console') : ('exited' as const);

      if (iamState === 'ready' && gatewayState === 'ready') {
        const harness: TwoZoneHarness = {
          origin: terminator.origin,
          iam,
          idp,
          gateway,
          iamConsole: forwarder,
          url: (fullPath) => `${terminator.origin}${fullPath}`,
          serverOutput: (zone) => outputs[zone],
          useWorld: (options = {}) => {
            iam.setHandlers(worldHandlers(options));
            iam.setServiceInfo(options.iamDescriptor ?? DEFAULT_IAM_DESCRIPTOR);
            gateway.setServiceInfo(options.gatewayDescriptor ?? DEFAULT_GATEWAY_DESCRIPTOR);
            gateway.setReachable(true);
          },
        };
        // Both servers, then the terminator in front of them, then the forwarder, then the fakes,
        // then Redis last (the array is already in that order — newest first).
        return { harness, close: () => closeInOrder(started.splice(0)) };
      }
      failures.push(`attempt ${String(attempt)}: iam=${iamState} gateway=${gatewayState}\n--- iam-console output ---\n${outputs.iam}\n--- gateway-console output ---\n${outputs.gateway}`);
      // This attempt's four newest entries (gateway child, iam child, terminator, forwarder) close;
      // the container and the three fakes stay up for the next attempt.
      started.splice(0, 4);
      await closeInOrder([stopGateway, stopIam, closeTerminator, closeForwarder]);
    }
    throw new Error(`the two-zone stack failed to start after ${String(MAX_START_ATTEMPTS)} attempts:\n${failures.join('\n')}`);
  } catch (error) {
    // Close what started, then rethrow the START error: it tells why the stack is not up. A close
    // failure goes to stderr, because it must not hide the start error.
    await closeInOrder(started.splice(0)).catch((closeError: unknown) => {
      console.error('the two-zone e2e stack failed to start, and closing what had started failed too:', closeError);
    });
    throw error;
  }
}

export const test = base.extend<{ world: undefined }, { harness: TwoZoneHarness }>({
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
    // The container start on top of the two servers: REDIS_START_TIMEOUT_MS (120 s) for Redis, plus
    // the server budget, plus room for the three fakes, the forwarder, the terminator, the port
    // probes and the cleanup.
    //
    // The server budget is MAX_START_ATTEMPTS * 2 * READY_TIMEOUT_MS, and the 2 is load-bearing:
    // THIS stack waits on two health endpoints per attempt, and waitForHealth can spend the full
    // READY_TIMEOUT_MS on each before it observes a late child exit. Budgeting one probe per
    // attempt lets this wrapper fire first on a slow start, and Playwright then reports an opaque
    // fixture timeout instead of startStack's aggregated error, which names the server that failed.
    { scope: 'worker', timeout: REDIS_START_TIMEOUT_MS + MAX_START_ATTEMPTS * 2 * READY_TIMEOUT_MS + 60_000 },
  ],
  // Before EVERY test: the default world and the default descriptors, and a reachable gateway — the
  // same three resets the single-zone harness does, for the same reason (a test that made the
  // gateway unreachable must not leak that into a later test in the same worker).
  world: [
    async ({ harness }, use) => {
      harness.useWorld();
      await use(undefined);
    },
    { auto: true },
  ],
});

export { expect } from '@playwright/test';
