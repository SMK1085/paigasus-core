// SPDX-License-Identifier: Apache-2.0
//
// The `playground` project's stack (SMA-635 spec § 7.2, D9), ONE per worker (workers: 1):
//
//   browser --https--> TLS terminator --http--> standalone server.js --http--> REAL paigasus-gateway --http--> mock OpenAI
//                                                    |                              \--h2c--> fake IAM (gRPC)
//                                                    \--h2c--> fake IAM (gRPC), fake IAM (HTTP service-info)
//
// PAIGASUS_SERVICES.gateway is the REAL gateway, so discovery reads the gateway's own descriptor.
// No Keycloak and no container. The fake IAM is not Cedar: these rows prove the WIRING (the right
// PRN, principal, action and bearer reach IAM, and a deny is shown), never the Cedar decision.
import { spawn } from 'node:child_process';
import path from 'node:path';
import { test as base } from '@playwright/test';
import { startFakeIam, startFakeIdp, startTlsTerminator, testTls, type FakeIam, type FakeIdp } from '@paigasus/console-core/testing';
import { parseGatewayLog, type GatewayLogLine } from './gateway-env';
import { startGateway } from './gateway-process';
import { closeInOrder, freePort, serverEnv, stop, waitForHealth } from './harness';
import { DONE, delta, startMockOpenAi, type MockOpenAi } from './mock-openai';
import { STANDALONE_APP_DIR } from './paths';
import { DEFAULT_IAM_DESCRIPTOR, worldHandlers, type WorldOptions } from './world';

const isCI = !!process.env.CI;
const MAX_START_ATTEMPTS = 3;

export type PlaygroundHarness = {
  readonly origin: string;
  readonly iam: FakeIam;
  readonly idp: FakeIdp;
  readonly mock: MockOpenAi;
  url(path: string): string;
  serverOutput(): string;
  gatewayLog(): readonly GatewayLogLine[];
  useWorld(options?: Pick<WorldOptions, 'overrides' | 'allow'>): void;
};

async function startStack(): Promise<{ harness: PlaygroundHarness; close: () => Promise<void> }> {
  const tls = testTls();
  const started: (() => Promise<void>)[] = [];
  try {
    const idp = await startFakeIdp({ cert: tls });
    started.unshift(() => idp.close());
    const iam = await startFakeIam({ handlers: worldHandlers() });
    started.unshift(() => iam.close());
    iam.setServiceInfo(DEFAULT_IAM_DESCRIPTOR);
    const mock = await startMockOpenAi();
    started.unshift(() => mock.close());
    const gateway = await startGateway({ iamGrpcUrl: iam.grpcUrl, openAiUrl: mock.url });
    started.unshift(() => gateway.close());

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
          PAIGASUS_ZONE: 'gateway',
          PAIGASUS_ZONES: JSON.stringify({ gateway: '/gateway' }),
          PAIGASUS_OIDC_ISSUER: idp.issuer,
          PAIGASUS_OIDC_CLIENT_ID: idp.clientId,
          PAIGASUS_OIDC_CLIENT_SECRET: idp.clientSecret,
          PAIGASUS_PUBLIC_ORIGIN: terminator.origin,
          PAIGASUS_SESSION_STORE: 'memory',
          PAIGASUS_SERVICES: JSON.stringify({ iam: iam.httpUrl, gateway: gateway.url }),
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
      const state = await waitForHealth(`http://127.0.0.1:${String(port)}/gateway/healthz`, child, () => output);
      if (state === 'ready') {
        const harness: PlaygroundHarness = {
          origin: terminator.origin,
          iam,
          idp,
          mock,
          url: (fullPath) => `${terminator.origin}${fullPath}`,
          serverOutput: () => output,
          gatewayLog: () => parseGatewayLog(gateway.output()),
          useWorld: (options = {}) => {
            iam.setHandlers(worldHandlers(options));
            iam.setServiceInfo(DEFAULT_IAM_DESCRIPTOR);
            mock.setMode({ kind: 'complete', body: `${delta('ok')}${DONE}` });
          },
        };
        return { harness, close: () => closeInOrder(started.splice(0)) };
      }
      failures.push(`attempt ${String(attempt)}: the server exited before it answered\n${output}`);
      started.splice(0, 2);
      await closeInOrder([stopChild, closeTerminator]);
    }
    throw new Error(`the gateway-console server failed to start after ${String(MAX_START_ATTEMPTS)} attempts:\n${failures.join('\n')}`);
  } catch (error) {
    await closeInOrder(started.splice(0)).catch((closeError: unknown) => {
      console.error('the playground stack failed to start, and closing what had started failed too:', closeError);
    });
    throw error;
  }
}

export const test = base.extend<{ world: undefined }, { harness: PlaygroundHarness }>({
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
    // The single-zone fixture's budget plus the gateway's own start (up to 3 attempts).
    { scope: 'worker', timeout: isCI ? 600_000 : 360_000 },
  ],
  world: [
    async ({ harness }, use) => {
      harness.useWorld();
      await use(undefined);
    },
    { auto: true },
  ],
});

export { expect } from '@playwright/test';
