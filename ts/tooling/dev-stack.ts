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
//
// This file has no automated coverage and never will — CI cannot run Docker plus two dev servers —
// so a review pass and one hand-verification are the only gates it ever gets (SMA-641 dev-stack
// review round 1).
//
// NOTHING TYPECHECKS THIS FILE EITHER (final whole-branch review, minor 5). `ts/tooling` has no
// `package.json`, so the root `typecheck` script (`pnpm -r --if-present run typecheck`) never
// reaches `tooling/tsconfig.json`, and Moon's `ts:typecheck` is routed to the library/application
// layers, never the root project (`ts/moon.yml:28-30`). Typed ESLint rules report lint problems,
// not type errors. So a type error introduced here reds nothing in CI — a local
// `pnpm --dir ts exec tsc -p tooling/tsconfig.json --noEmit` is the only way to catch one.
import { spawn, type ChildProcess } from 'node:child_process';
import { readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer, request as httpRequest, type IncomingHttpHeaders, type IncomingMessage, type ServerResponse } from 'node:http';
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
/** Per-request cap on a single health-check fetch, so one wedged request cannot outlast READY_TIMEOUT_MS. */
const HEALTH_FETCH_TIMEOUT_MS = 5_000;

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
 * Hop-by-hop headers (RFC 9110 § 7.6.1). A proxy must not forward them. Copied from
 * tls-terminator.ts's own `HOP_BY_HOP` / `forwardable` rather than imported — that module's
 * `./testing` export map exposes only what the e2e harness needs, and this file is the only other
 * caller, the same call this repo already made for fake-idp.ts's JWKS helper ("copied rather than
 * imported") (SMA-641 dev-stack review round 1, finding 2).
 */
const HOP_BY_HOP = new Set(['connection', 'keep-alive', 'proxy-connection', 'te', 'trailer', 'transfer-encoding', 'upgrade']);

function forwardable(headers: IncomingHttpHeaders): IncomingHttpHeaders {
  const out: IncomingHttpHeaders = {};
  for (const [name, value] of Object.entries(headers)) {
    if (!HOP_BY_HOP.has(name) && value !== undefined) out[name] = value;
  }
  return out;
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
    // A bare root carrying a query string — `/?_rsc=1`, the shape Next's RSC prefetch produces —
    // is not `=== '/'` against the raw req.url. Strip the query before matching, the same rule
    // tls-terminator.ts applies to its own route matching. `req.url === ''` is unreachable for a
    // Node HTTP server, so it is no longer checked (SMA-641 dev-stack review round 1, minor D).
    const pathname = (req.url ?? '/').split('?')[0] ?? '/';
    if (pathname === '/') {
      res.writeHead(302, { location: '/iam' });
      res.end();
      return;
    }
    const upstream = httpRequest({ hostname: '127.0.0.1', port: IAM_PORT, method: req.method, path: req.url, headers: forwardable(req.headers) }, (upstreamRes) => {
      res.writeHead(upstreamRes.statusCode ?? 502, forwardable(upstreamRes.headers));
      upstreamRes.pipe(res);
    });
    upstream.on('error', () => {
      if (res.headersSent) return;
      res.writeHead(502, { 'content-type': 'text/plain' });
      res.end('dev-stack: the default zone did not answer');
    });
    // Mirrors tls-terminator.ts's forward(): the downstream (browser-facing, via the terminator)
    // response closed — a client abort, e.g. a cancelled RSC prefetch, which `next dev` produces a
    // constant stream of — before the upstream finished. Destroying it releases the socket pinned
    // against the iam child; without this handler every aborted request leaked one (SMA-641
    // dev-stack review round 1, finding 1).
    res.on('close', () => upstream.destroy());
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

/**
 * SIGTERM, then SIGKILL after a grace period — the shape the e2e harness's own stop() uses.
 *
 * A child whose spawn failed (an 'error' event — see spawnZone) never got a pid, so `kill()` is a
 * no-op and there is nothing to wait for: Node documents 'exit' as "may or may not" fire after
 * 'error' (it emits 'close' instead), so waiting on it here could stall forever and, since this is
 * always the FIRST closer pushed for a zone, take every later closer down with it (SMA-641
 * dev-stack review round 2, finding 2).
 */
function stop(child: ChildProcess): Promise<void> {
  if (child.pid === undefined) return Promise.resolve();
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
  // `--hostname` as a CLI FLAG, not the HOSTNAME env var. MEASURED on Next 16.3.5: `next dev`
  // ignores HOSTNAME (it prints a `Network:` line and binds every interface), while the standalone
  // `server.js` the e2e harness spawns does honour it — which is why the harness needs no flag and
  // this does. The bind host is what Next puts in `blockCrossSiteDEV`'s allowlist, so without the
  // flag every hot-reload socket arriving through the terminator is refused with "Blocked
  // cross-origin request to Next.js dev resource", and hot reload silently does not work.
  const child = spawn(process.execPath, [nextBin, 'dev', '--hostname', '127.0.0.1'], {
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
  // `spawn` reports an async launch failure (EACCES, EAGAIN — the binary resolved but could not be
  // started) as an 'error' event. With no listener, EventEmitter rethrows it as an UNCAUGHT
  // exception outside main()'s try/catch, so nothing would be torn down. waitForZone below also
  // listens, to fail fast instead of polling to the deadline (SMA-641 dev-stack review round 1,
  // finding 5).
  child.on('error', (error) => {
    console.error(`[${zone}] failed to start:`, error);
  });
  return child;
}

/**
 * Ready when the zone's health route answers 2xx. A refusal or a timeout means "still compiling";
 * a 5xx is FATAL, because that is a rejected configuration rather than a slow build — the same
 * rule the e2e harness uses, with a much longer deadline. An `'error'` event, or an exit by code
 * OR signal (`stop()` already checks both, so this does too, SMA-641 dev-stack review round 1,
 * minor C), is also fatal and fails fast rather than polling to the deadline (finding 5).
 * `isInterrupted` is checked every iteration so a signal that lands during the up-to-180s compile
 * wait does not sit out the rest of the timeout (finding 4).
 */
async function waitForZone(zone: Zone, child: ChildProcess, isInterrupted: () => boolean): Promise<void> {
  const url = `http://127.0.0.1:${String(ZONE_PORT[zone])}/${zone}/healthz`;
  const deadline = Date.now() + READY_TIMEOUT_MS;
  // Node's ChildProcess always reports its 'error' event with an actual Error object.
  let childError: Error | undefined;
  const onError = (error: Error): void => {
    childError = error;
  };
  child.once('error', onError);
  try {
    while (Date.now() < deadline) {
      if (isInterrupted()) throw new Error('dev-stack: interrupted by signal during startup');
      if (childError !== undefined) throw new Error(`dev-stack: the ${zone} server failed to start: ${childError.message}`);
      if (child.exitCode !== null || child.signalCode !== null) {
        throw new Error(`dev-stack: the ${zone} server exited (code=${String(child.exitCode)}, signal=${String(child.signalCode)}) before it became ready`);
      }
      let response: Response | undefined;
      try {
        // Bound the fetch itself: without an abort signal, a child that accepts the TCP
        // connection but never answers (a wedged compile, a hung process) leaves this `await`
        // unsettled forever, and neither the deadline above nor `isInterrupted()` gets re-checked
        // until it resolves. Use the smaller of a short per-request cap and the time left until
        // the deadline, floored at 1ms so a near-expired deadline never produces a non-positive
        // timeout.
        const remainingMs = deadline - Date.now();
        const fetchTimeoutMs = Math.max(1, Math.min(HEALTH_FETCH_TIMEOUT_MS, remainingMs));
        response = await fetch(url, { signal: AbortSignal.timeout(fetchTimeoutMs) });
      } catch {
        // Not listening yet, still compiling the route, or the per-request cap above was hit —
        // all three mean the same thing here: "not ready yet".
      }
      // OUTSIDE the try/catch above: a real HTTP answer, fatal or not, is never "still compiling".
      // The previous shape sniffed `error.message.startsWith('dev-stack:')` to tell its own throw
      // apart from a fetch failure, which made an explicit status rule depend on a message string
      // (SMA-641 dev-stack review round 1, minor A).
      if (response !== undefined) {
        if (response.ok) return;
        if (response.status >= 500) {
          throw new Error(`dev-stack: the ${zone} server answered ${String(response.status)} at ${url}. That is a rejected configuration, not a slow compile — read the [${zone}] lines above.`);
        }
      }
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
    throw new Error(`dev-stack: the ${zone} server was not ready within ${String(READY_TIMEOUT_MS / 1000)}s`);
  } finally {
    child.off('error', onError);
  }
}

/**
 * Logs loudly if a zone dies after it was already reported ready. Without this the supervisor
 * keeps running and the browser just gets 502s with nothing in the log naming the zone (SMA-641
 * dev-stack review round 1, minor E).
 *
 * `isShuttingDown` gates the log: `stop()` sends SIGTERM (and in a terminal, Ctrl-C usually kills
 * the whole foreground process group with SIGINT before closeAll even runs), so an UNGATED
 * listener fired this same "exited unexpectedly" alarm on every ordinary shutdown — training a
 * developer to ignore the one case this function exists to catch (SMA-641 dev-stack review
 * round 2, finding 1).
 */
function watchZone(zone: Zone, child: ChildProcess, isShuttingDown: () => boolean): void {
  child.once('exit', (code, signal) => {
    if (isShuttingDown()) return;
    console.error(`[dev-stack] the ${zone} server exited unexpectedly (code=${String(code)}, signal=${String(signal)})`);
  });
}

/**
 * `next dev` dirties the working tree in two ways this supervisor must undo: it rewrites the
 * TRACKED `next-env.d.ts` (a `.next/types/...` reference becomes `.next/dev/types/...`, which
 * `repo:next-env-drift` watches for), and Next 16 drops untracked `AGENTS.md` / `CLAUDE.md` files
 * at the app root. The second matters more than it looks — a stray `CLAUDE.md` is read by a
 * future agent session as project instructions, so leaving it behind actively misleads (SMA-641
 * dev-stack review round 3).
 */
const RESTORABLE_DOC_NAMES = new Set(['AGENTS.md', 'CLAUDE.md']);

type ZoneSnapshot = { readonly nextEnvContent: string; readonly entries: ReadonlySet<string> };

/** Taken BEFORE spawning the zone's `next dev` child, so `entries` reflects what was on disk
 * before that child could write anything. */
async function snapshotZone(zone: Zone): Promise<ZoneSnapshot> {
  const dir = ZONE_DIR[zone];
  const nextEnvContent = await readFile(path.join(dir, 'next-env.d.ts'), 'utf8');
  const entries = new Set(await readdir(dir));
  return { nextEnvContent, entries };
}

/**
 * Restores `next-env.d.ts` to its startup content if `next dev` changed it, and removes any
 * `AGENTS.md` / `CLAUDE.md` that appeared directly in the app directory and was NOT there at
 * startup. By NAME, by presence-at-startup, and non-recursive ONLY — a dev tool that deletes
 * files needs a narrow, obvious rule; nothing else in the directory is touched. A read/write/list
 * failure here is logged and does not stop the rest of teardown, the same rule every other closer
 * follows.
 */
async function restoreZone(zone: Zone, snapshot: ZoneSnapshot): Promise<void> {
  const dir = ZONE_DIR[zone];
  const nextEnvPath = path.join(dir, 'next-env.d.ts');
  try {
    const current = await readFile(nextEnvPath, 'utf8');
    if (current !== snapshot.nextEnvContent) {
      await writeFile(nextEnvPath, snapshot.nextEnvContent, 'utf8');
      log(`restored ${nextEnvPath} (next dev rewrote it)`);
    }
  } catch (error) {
    console.error(`[dev-stack] could not restore ${nextEnvPath}:`, error);
  }

  try {
    const current = await readdir(dir);
    for (const name of current) {
      if (snapshot.entries.has(name)) continue;
      if (!RESTORABLE_DOC_NAMES.has(name)) continue;
      const filePath = path.join(dir, name);
      try {
        await rm(filePath);
        log(`removed ${filePath} (next dev created it)`);
      } catch (error) {
        console.error(`[dev-stack] could not remove ${filePath}:`, error);
      }
    }
  } catch (error) {
    console.error(`[dev-stack] could not list ${dir}:`, error);
  }
}

async function main(): Promise<void> {
  // NEWEST FIRST: the order to close in. Every start step pushes its own closer.
  const started: (() => Promise<void>)[] = [];
  let closing = false;
  const closeAll = async (): Promise<void> => {
    // Idempotent: the signal path and the normal/catch path could both reach this, and must not
    // run concurrently (SMA-641 dev-stack review round 1, finding 4 / minor B).
    if (closing) return;
    closing = true;
    for (const close of started.splice(0)) {
      // try/catch, not `.catch()` on the call: a closer that threw SYNCHRONOUSLY used to escape
      // `.catch()`, abort this loop, and take every remaining closer with it — gone from both the
      // spliced copy and `started` (SMA-641 dev-stack review round 1, minor B).
      try {
        await close();
      } catch (error) {
        console.error('[dev-stack] a shutdown step failed:', error);
      }
    }
  };

  // Registered BEFORE preflight(), not after both zones are ready: that used to leave the longest
  // window in the program — a Docker pull plus up to 180s of compile per zone — with NO handler at
  // all, so a Ctrl-C there hit Node's default SIGINT behaviour (immediate exit, no teardown)
  // instead of this one. `shutdownRequested` is also checked between start steps below (`checkpoint`)
  // and inside waitForZone's own poll loop, so a signal does not sit out whichever wait was in
  // flight (SMA-641 dev-stack review round 1, finding 4).
  let ready = false;
  let shutdownRequested = false;
  let notifyShutdown!: () => void;
  const shutdown = new Promise<void>((resolve) => {
    notifyShutdown = resolve;
  });
  const checkpoint = (): void => {
    if (shutdownRequested) throw new Error('dev-stack: interrupted by signal during startup');
  };
  const onSignal = (signal: NodeJS.Signals): void => {
    // Teardown has not actually started yet here: it begins once the in-flight start step (or, if
    // already ready, the very next line) settles and reaches a checkpoint. Say so, rather than
    // implying closeAll() is already running (SMA-641 dev-stack review round 2, wording fix).
    log(ready ? `received ${signal}, shutting down` : `received ${signal}; finishing the current start step, then shutting down`);
    shutdownRequested = true;
    // A signal that lands before both zones are ready is an INTERRUPTED startup, not a clean
    // stop, so the process exits non-zero.
    if (!ready) process.exitCode = 1;
    notifyShutdown();
  };
  process.once('SIGINT', () => onSignal('SIGINT'));
  process.once('SIGTERM', () => onSignal('SIGTERM'));

  try {
    await preflight();
    checkpoint();

    log('generating the TLS material');
    const tls = testTls({ root: TLS_ROOT, days: CERT_DAYS });

    log('starting Redis (Docker is required)');
    const redis = await new GenericContainer('redis:8-alpine').withExposedPorts(6379).withStartupTimeout(REDIS_START_TIMEOUT_MS).start();
    started.unshift(async () => {
      await redis.stop();
    });
    const redisUrl = `redis://${redis.getHost()}:${String(redis.getMappedPort(6379))}/0`;
    checkpoint();

    log('starting the fakes');
    const idp = await startFakeIdp({ cert: tls, port: IDP_PORT });
    started.unshift(() => idp.close());
    const iam = await startFakeIam({ handlers: devWorld() });
    started.unshift(() => iam.close());
    iam.setServiceInfo(DEV_IAM_DESCRIPTOR);
    const gateway = await startFakeGateway();
    started.unshift(() => gateway.close());
    gateway.setServiceInfo(DEV_GATEWAY_DESCRIPTOR);
    checkpoint();

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
    checkpoint();

    const env = buildDevEnv({
      parentEnv: process.env,
      certPath: tls.certPath,
      idp: { issuer: idp.issuer, clientId: idp.clientId, clientSecret: idp.clientSecret },
      iam: { grpcUrl: iam.grpcUrl, httpUrl: iam.httpUrl },
      gatewayUrl: gateway.url,
      redisUrl,
      publicOrigin: terminator.origin,
    });

    // BEFORE spawning either child: snapshot what next dev is about to dirty, so it can be put
    // back. The restore closers are pushed here too, ahead of the children's own `stop()` closers
    // below, so — newest-first — both children are fully stopped before restoreZone ever reads
    // their app directory (SMA-641 dev-stack review round 3).
    const iamSnapshot = await snapshotZone('iam');
    const gatewaySnapshot = await snapshotZone('gateway');
    started.unshift(() => restoreZone('iam', iamSnapshot));
    started.unshift(() => restoreZone('gateway', gatewaySnapshot));

    log('starting both zones with `next dev` — the first request compiles, so this takes a while');
    // Split so each child's closer is registered before the NEXT child starts: with both spawned
    // inside one object literal, a synchronous throw from the second spawnZone call (e.g. the
    // gateway zone's own `createRequire(...).resolve(...)` failing on a partially provisioned
    // worktree) left the first child running and unreachable by closeAll(). This also fixes the
    // previously inverted teardown order between the two (SMA-641 dev-stack review round 1,
    // finding 3).
    const iamChild = spawnZone('iam', env);
    started.unshift(() => stop(iamChild));
    const gatewayChild = spawnZone('gateway', env);
    started.unshift(() => stop(gatewayChild));

    await waitForZone('iam', iamChild, () => shutdownRequested);
    await waitForZone('gateway', gatewayChild, () => shutdownRequested);
    checkpoint();
    ready = true;
    watchZone('iam', iamChild, () => shutdownRequested || closing);
    watchZone('gateway', gatewayChild, () => shutdownRequested || closing);

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

    await shutdown;
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
