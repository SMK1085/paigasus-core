// SPDX-License-Identifier: Apache-2.0
//
// The two-zone dev stack (SMA-641). One command starts everything `next dev` needs and spawns both
// zone servers against it:
//
//   browser --https--> TLS terminator :8443 -- /iam/*     --> next dev :3000 (iam-console)
//                                            |- /gateway/* --> next dev :3001 (gateway-console)
//                                            \- /*         --> the default-zone handler below
//
// WHY A SUPERVISOR AND NOT A .env.local. Three reasons, in order of weight. The fake IAM, the fake
// gateway and Redis take RANDOM ports, so PAIGASUS_SERVICES, PAIGASUS_IAM_GRPC_URL and
// PAIGASUS_SESSION_REDIS_URL are not knowable before the stack runs. Both children must agree on
// ONE PAIGASUS_PUBLIC_ORIGIN, which is the terminator's. And NODE_EXTRA_CA_CERTS cannot come from
// an env file at all: Node captures it while it initializes, before Next loads .env.local.
//
// DOCKER IS REQUIRED AND FAILS LOUDLY, the same rule the two-zone e2e tier states: two zones force
// PAIGASUS_SESSION_STORE=redis, because createAuthRuntime refuses the memory store once
// PAIGASUS_ZONES names more than one zone.
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer, request as httpRequest, type IncomingMessage, type ServerResponse } from 'node:http';
import { createConnection, type AddressInfo } from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { GenericContainer } from 'testcontainers';
import { buildDevEnv, DEV_GATEWAY_DESCRIPTOR, DEV_IAM_DESCRIPTOR, devWorld, startFakeGateway, startFakeIam, startFakeIdp, startTlsTerminator, testTls } from '@paigasus/console-core/testing';

/** Browser-facing, so FIXED: a browser stores a certificate exception per origin, and a random
 * port would force a new exception on every restart. */
const TERMINATOR_PORT = 8443;
const IDP_PORT = 8444;
/** Not browser-facing. Fixed only so the terminator's routes exist before the children start. */
const IAM_PORT = 3000;
const GATEWAY_PORT = 3001;

/** A dev certificate that expired daily would mean accepting two exceptions again every day. */
const CERT_DAYS = 30;
const TLS_ROOT = path.join(os.tmpdir(), 'paigasus-dev-stack-tls');

const REDIS_START_TIMEOUT_MS = 120_000;
/** In dev the FIRST request compiles the route, so the e2e budget is far too short. */
const READY_TIMEOUT_MS = 180_000;
const STOP_GRACE_MS = 5_000;

const TS_ROOT = path.resolve(fileURLToPath(new URL('..', import.meta.url)));

type Zone = 'iam' | 'gateway';

const ZONE_DIR: Record<Zone, string> = {
  iam: path.join(TS_ROOT, 'apps', 'iam-console'),
  gateway: path.join(TS_ROOT, 'apps', 'gateway-console'),
};
const ZONE_PORT: Record<Zone, number> = { iam: IAM_PORT, gateway: GATEWAY_PORT };

function log(message: string): void {
  console.log(`[dev-stack] ${message}`);
}

/**
 * Advisory only. It RACES anything that takes the port in between, so the authoritative check is
 * each server's own listen error. It exists because failing here names all four ports at once,
 * before a Docker pull.
 */
async function preflight(): Promise<void> {
  const busy: number[] = [];
  for (const port of [TERMINATOR_PORT, IDP_PORT, IAM_PORT, GATEWAY_PORT]) {
    const free = await new Promise<boolean>((resolve) => {
      const probe = createConnection({ host: '127.0.0.1', port });
      probe.once('connect', () => {
        probe.destroy();
        resolve(false);
      });
      probe.once('error', () => resolve(true));
    });
    if (!free) busy.push(port);
  }
  if (busy.length > 0) {
    throw new Error(`dev-stack: ${busy.join(', ')} already in use. Stop whatever holds them, or an earlier dev:stack that did not shut down.`);
  }
}

/**
 * The terminator's catch-all upstream, and it solves two problems at once.
 *
 * The origin ROOT has no zone route, so `/` and `/favicon.ico` would answer 502. And Next's
 * dev-overlay endpoints are ROOT-RELATIVE — `/__nextjs_original-stack-frames`, `/__nextjs_font/…`,
 * `/__nextjs_source-map` and the rest — with no basePath, so the terminator's prefix matcher
 * cannot reach them (it needs `pathname === prefix` or `prefix + '/'`).
 *
 * KNOWN LIMITATION: two zones on one origin cannot both own `/__nextjs_*`. The iam zone is the
 * default, so the GATEWAY zone's error overlay loses its source-mapped frames, its fonts and
 * "open in editor". The overlay still reports the error.
 */
async function startDefaultZone(): Promise<{ url: string; close: () => Promise<void> }> {
  const server = createServer((req: IncomingMessage, res: ServerResponse) => {
    if (req.url === '/' || req.url === '') {
      res.writeHead(302, { location: '/iam' });
      res.end();
      return;
    }
    const upstream = httpRequest({ hostname: '127.0.0.1', port: IAM_PORT, method: req.method, path: req.url, headers: req.headers }, (upstreamRes) => {
      res.writeHead(upstreamRes.statusCode ?? 502, upstreamRes.headers);
      upstreamRes.pipe(res);
    });
    upstream.on('error', () => {
      if (res.headersSent) return;
      res.writeHead(502, { 'content-type': 'text/plain' });
      res.end('dev-stack: the default zone did not answer');
    });
    req.pipe(upstream);
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  return {
    url: `http://127.0.0.1:${String(port)}`,
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.closeAllConnections();
        server.close((err) => (err ? reject(err) : resolve()));
      }),
  };
}

/** SIGTERM, then SIGKILL after a grace period — the shape the e2e harness's own stop() uses. */
function stop(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
  return new Promise<void>((resolve) => {
    const timer = setTimeout(() => child.kill('SIGKILL'), STOP_GRACE_MS);
    child.once('exit', () => {
      clearTimeout(timer);
      resolve();
    });
    child.kill('SIGTERM');
  });
}

/**
 * `next dev` forks a child of its own and installs its own signal handlers. Spawning it through
 * `pnpm run dev` can leave SIGTERM at the pnpm layer and orphan a process holding 3000 or 3001 —
 * after which the NEXT run fails on a port that looks free. So resolve Next's own bin and run it
 * with this Node directly, the shape the e2e harness uses for server.js.
 */
function spawnZone(zone: Zone, env: Record<string, string>): ChildProcess {
  const cwd = ZONE_DIR[zone];
  const nextBin = createRequire(path.join(cwd, 'package.json')).resolve('next/dist/bin/next');
  const child = spawn(process.execPath, [nextBin, 'dev'], {
    cwd,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...env, PORT: String(ZONE_PORT[zone]), HOSTNAME: '127.0.0.1', PAIGASUS_ZONE: zone },
  });
  const prefix = (chunk: Buffer): void => {
    for (const line of chunk.toString('utf8').split('\n')) {
      if (line.trim() !== '') console.log(`[${zone}] ${line}`);
    }
  };
  child.stdout?.on('data', prefix);
  child.stderr?.on('data', prefix);
  return child;
}

/**
 * Ready when the zone's health route answers 2xx. A refusal or a timeout means "still compiling";
 * a 5xx is FATAL, because that is a rejected configuration rather than a slow build — the same
 * rule the e2e harness uses, with a much longer deadline.
 */
async function waitForZone(zone: Zone, child: ChildProcess): Promise<void> {
  const url = `http://127.0.0.1:${String(ZONE_PORT[zone])}/${zone}/healthz`;
  const deadline = Date.now() + READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`dev-stack: the ${zone} server exited with code ${String(child.exitCode)} before it became ready`);
    try {
      const response = await fetch(url);
      if (response.ok) return;
      if (response.status >= 500) {
        throw new Error(`dev-stack: the ${zone} server answered ${String(response.status)} at ${url}. That is a rejected configuration, not a slow compile — read the [${zone}] lines above.`);
      }
    } catch (error) {
      if (error instanceof Error && error.message.startsWith('dev-stack:')) throw error;
      // Not listening yet, or still compiling the route.
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(`dev-stack: the ${zone} server was not ready within ${String(READY_TIMEOUT_MS / 1000)}s`);
}

async function main(): Promise<void> {
  // NEWEST FIRST: the order to close in. Every start step pushes its own closer.
  const started: (() => Promise<void>)[] = [];
  const closeAll = async (): Promise<void> => {
    for (const close of started.splice(0)) {
      await close().catch((error: unknown) => console.error('[dev-stack] a shutdown step failed:', error));
    }
  };

  try {
    await preflight();

    log('generating the TLS material');
    const tls = testTls({ root: TLS_ROOT, days: CERT_DAYS });

    log('starting Redis (Docker is required)');
    const redis = await new GenericContainer('redis:8-alpine').withExposedPorts(6379).withStartupTimeout(REDIS_START_TIMEOUT_MS).start();
    started.unshift(async () => {
      await redis.stop();
    });
    const redisUrl = `redis://${redis.getHost()}:${String(redis.getMappedPort(6379))}/0`;

    log('starting the fakes');
    const idp = await startFakeIdp({ cert: tls, port: IDP_PORT });
    started.unshift(() => idp.close());
    const iam = await startFakeIam({ handlers: devWorld() });
    started.unshift(() => iam.close());
    iam.setServiceInfo(DEV_IAM_DESCRIPTOR);
    const gateway = await startFakeGateway();
    started.unshift(() => gateway.close());
    gateway.setServiceInfo(DEV_GATEWAY_DESCRIPTOR);

    const defaultZone = await startDefaultZone();
    started.unshift(() => defaultZone.close());

    const terminator = await startTlsTerminator({
      tls,
      port: TERMINATOR_PORT,
      routes: [
        { prefix: '/iam', target: `http://127.0.0.1:${String(IAM_PORT)}` },
        { prefix: '/gateway', target: `http://127.0.0.1:${String(GATEWAY_PORT)}` },
        // Last by the longest-prefix rule: the root redirect and every root-relative dev path.
        { prefix: '/', target: defaultZone.url },
      ],
    });
    started.unshift(() => terminator.close());

    const env = buildDevEnv({
      parentEnv: process.env,
      certPath: tls.certPath,
      idp: { issuer: idp.issuer, clientId: idp.clientId, clientSecret: idp.clientSecret },
      iam: { grpcUrl: iam.grpcUrl, httpUrl: iam.httpUrl },
      gatewayUrl: gateway.url,
      redisUrl,
      publicOrigin: terminator.origin,
    });

    log('starting both zones with `next dev` — the first request compiles, so this takes a while');
    const children: Record<Zone, ChildProcess> = { iam: spawnZone('iam', env), gateway: spawnZone('gateway', env) };
    started.unshift(() => stop(children.gateway));
    started.unshift(() => stop(children.iam));

    await waitForZone('iam', children.iam);
    await waitForZone('gateway', children.gateway);

    console.log('');
    console.log(`  open  ${terminator.origin}/iam`);
    console.log('');
    console.log('  Accept the certificate once for BOTH origins — the second is where login redirects:');
    console.log(`    ${terminator.origin}`);
    console.log(`    ${idp.issuer}`);
    console.log('');
    console.log('  The environment both servers received:');
    for (const key of Object.keys(env)
      .filter((name) => name.startsWith('PAIGASUS_') || name === 'NODE_EXTRA_CA_CERTS')
      .sort()) {
      console.log(`    ${key}=${env[key] ?? ''}`);
    }
    console.log('');
    console.log('  Ctrl-C stops everything. A restart logs you out: Redis is new each run.');
    console.log('');

    await new Promise<void>((resolve) => {
      const shutdown = (): void => {
        log('shutting down');
        resolve();
      };
      process.once('SIGINT', shutdown);
      process.once('SIGTERM', shutdown);
    });
    await closeAll();
  } catch (error) {
    // Close what started, then report the START error: it says why the stack is not up. A close
    // failure goes to stderr and must not hide it.
    await closeAll();
    throw error;
  }
}

main().catch((error: unknown) => {
  console.error('[dev-stack] failed to start:', error);
  process.exitCode = 1;
});
