// SPDX-License-Identifier: Apache-2.0
//
// Starts Keycloak (self-signed HTTPS on 8443, `--import-realm`) and Redis via `testcontainers`,
// then publishes their connection details for the fixture-server.ts process and the spec files to
// read (see runtime-env.ts's own header for why a file, not an env var or an in-memory value).
//
// NO SKIP. If Docker is unreachable, `GenericContainer(...).start()` rejects and this rejects the
// whole `playwright test` run before a single spec executes — matching every other container
// suite in this package (tests/containers/redis-store.test.ts's own header: "a task whose only
// purpose is container tests has no reason to skip").
//
// WHY `startE2eInfrastructure` IS EXPORTED SEPARATELY FROM THE DEFAULT `globalSetup` HOOK, AND WHY
// playwright.config.ts CALLS IT DIRECTLY (MEASURED, not the brief's assumed order). Playwright's
// own task order runs every `webServer` plugin's `setup()` — which starts the fixture server
// process and BLOCKS until its `url` responds — before any `globalSetup` file's default export
// ever runs (`playwright/lib/runner/index.js`'s `createGlobalSetupTasks`:
// `[...pluginSetupTasks, ...globalTeardowns, ...globalSetups]`, run in that array order). The
// fixture server cannot open its listening port without an issuer and a Redis URL (see its own
// header), so if this file's container startup ran ONLY inside the `globalSetup` hook, the whole
// run would deadlock: webServer waits forever for a port that never opens, because the code that
// would open it is waiting for globalSetup, which Playwright will not run until webServer is
// already confirmed ready. `startE2eInfrastructure` is therefore called directly from
// playwright.config.ts's own top-level (module-evaluation time, which strictly precedes the whole
// task-runner phase — verified with a top-level-await probe against this exact Playwright version
// before relying on it).
//
// MEASURED SECOND: a module-level singleton is NOT enough on its own, because Playwright's TEST
// WORKERS are separate OS processes, each of which re-imports playwright.config.ts from scratch —
// a per-process memo cannot see a sibling process's containers. Without the cross-process check
// below, the main/runner process's `startE2eInfrastructure()` call started one Keycloak+Redis
// pair while the worker process's own call (re-running the same top-level code) started a SECOND,
// unrelated pair and overwrote runtime-env.json with ITS OWN urls — the fixture server (bound to
// the FIRST pair) kept working, but recovery.spec.ts's direct Redis client (reading whatever
// runtime-env.json said LAST) connected to the wrong instance and found it empty. MEASURED via
// `docker ps` mid-run: two `quay.io/keycloak/keycloak:26.4` containers and two `redis:8-alpine`
// containers for a single `playwright test` invocation. The fix: before starting anything, check
// whether a PUBLISHED env already exists and its Keycloak/Redis are actually reachable — if so,
// adopt it (this process is not the owner and returns a no-op teardown) rather than starting a
// competing pair. A plain existence check is not enough on its own (a crashed prior run can leave
// a stale file behind), so reachability is checked too.
import { readFileSync } from 'node:fs';
import https from 'node:https';
import { GenericContainer, type StartedTestContainer } from 'testcontainers';
import { createClient } from 'redis';
import { KEYCLOAK_HTTPS_PORT, KEYCLOAK_REALM } from './constants.js';
import { ensureKeycloakCert } from './tls-fixture.js';
import { clearRuntimeEnv, tryReadRuntimeEnv, writeRuntimeEnv, type RuntimeEnv } from './runtime-env.js';

const KEYCLOAK_IMAGE = 'quay.io/keycloak/keycloak:26.4';
const REDIS_IMAGE = 'redis:8-alpine';
/** Keycloak's dev-mode boot (JVM + Quarkus + realm import) is slow — see the Rust precedent this
 * mirrors, rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs. */
const KEYCLOAK_READY_TIMEOUT_MS = 240_000;

/** A single insecure-TLS GET, resolving `true` only on a 200. Never throws. */
function probe(url: string): Promise<boolean> {
  return new Promise((resolvePromise) => {
    const req = https.get(url, { rejectUnauthorized: false, timeout: 5_000 }, (res) => {
      res.resume();
      resolvePromise(res.statusCode === 200);
    });
    req.on('error', () => resolvePromise(false));
    req.on('timeout', () => {
      req.destroy();
      resolvePromise(false);
    });
  });
}

/**
 * Polls the discovery document from the HOST side rather than relying on testcontainers' own
 * `Wait.forHttp` strategy — this mirrors the Rust E2E test's own manual poll loop (proven to work
 * against this exact image) and gives a container-log dump on the one failure mode that matters:
 * the realm import itself failing silently.
 */
async function waitForKeycloakReady(issuer: string, container: StartedTestContainer, timeoutMs: number): Promise<void> {
  const discoveryUrl = `${issuer}/.well-known/openid-configuration`;
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await probe(discoveryUrl)) return;
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 1_000));
  }
  const logs = await container.logs();
  const chunks: Buffer[] = [];
  for await (const chunk of logs) chunks.push(Buffer.from(chunk as Buffer));
  throw new Error(`keycloak discovery never became ready at ${discoveryUrl}\n--- keycloak logs ---\n${Buffer.concat(chunks).toString('utf8')}`);
}

async function startContainers(): Promise<{ env: RuntimeEnv; teardown: () => Promise<void> }> {
  const { certPath, keyPath } = ensureKeycloakCert();
  const certPem = readFileSync(certPath);
  const keyPem = readFileSync(keyPath);
  const realmJson = readFileSync(new URL('./keycloak-realm.json', import.meta.url));

  const keycloak = await new GenericContainer(KEYCLOAK_IMAGE)
    .withExposedPorts(KEYCLOAK_HTTPS_PORT)
    .withEnvironment({
      KC_BOOTSTRAP_ADMIN_USERNAME: 'admin',
      KC_BOOTSTRAP_ADMIN_PASSWORD: 'admin',
      KC_HTTPS_CERTIFICATE_FILE: '/opt/keycloak/conf/server.crt.pem',
      KC_HTTPS_CERTIFICATE_KEY_FILE: '/opt/keycloak/conf/server.key.pem',
    })
    .withCopyContentToContainer([
      { content: certPem, target: '/opt/keycloak/conf/server.crt.pem' },
      { content: keyPem, target: '/opt/keycloak/conf/server.key.pem' },
      { content: realmJson, target: `/opt/keycloak/data/import/${KEYCLOAK_REALM}-realm.json` },
    ])
    .withCommand(['start-dev', '--import-realm', `--https-port=${String(KEYCLOAK_HTTPS_PORT)}`])
    .withStartupTimeout(KEYCLOAK_READY_TIMEOUT_MS)
    .start();

  const redis = await new GenericContainer(REDIS_IMAGE).withExposedPorts(6379).start();

  // Keycloak derives `iss` from the request's Host header in dev mode (measured by the Rust E2E
  // test this mirrors), so every party that talks to it — this discovery poll, the fixture
  // server's openid-client discovery, and the browser's own navigation to the authorization
  // endpoint — MUST agree on the exact same `127.0.0.1:<port>` form. `container.getHost()` is
  // deliberately NOT used here: it can return `localhost` on some Docker setups, which would then
  // disagree with the issuer baked into every downstream discovery document.
  const kcPort = keycloak.getMappedPort(KEYCLOAK_HTTPS_PORT);
  const issuer = `https://127.0.0.1:${String(kcPort)}/realms/${KEYCLOAK_REALM}`;
  const redisUrl = `redis://127.0.0.1:${String(redis.getMappedPort(6379))}/0`;

  await waitForKeycloakReady(issuer, keycloak, KEYCLOAK_READY_TIMEOUT_MS);

  const env: RuntimeEnv = { issuer, redisUrl, certPath };
  writeRuntimeEnv(env);

  return {
    env,
    teardown: async () => {
      clearRuntimeEnv();
      await redis.stop();
      await keycloak.stop();
    },
  };
}

/** Reachability check for an env published by ANOTHER process — see the file header's second
 * MEASURED note. A plain `tryReadRuntimeEnv() !== undefined` is not enough: a crashed prior run
 * can leave a stale file whose containers no longer exist. */
async function isReachable(env: RuntimeEnv): Promise<boolean> {
  if (!(await probe(`${env.issuer}/.well-known/openid-configuration`))) return false;
  const client = createClient({ url: env.redisUrl, socket: { connectTimeout: 3_000 } });
  client.on('error', () => undefined); // see redis-store.ts's own identical rationale
  try {
    await client.connect();
    await client.ping();
    return true;
  } catch {
    return false;
  } finally {
    await client.close().catch(() => undefined);
  }
}

let started: ReturnType<typeof adoptOrStartContainers> | undefined;

/**
 * DEFERRED, NAMED KNOWN LIMIT (review round 1): this check-then-adopt is a TOCTOU — two processes
 * could both observe "no existing env" and both proceed to `startContainers()`, recreating the
 * exact double-start this function exists to prevent. It is safe TODAY not because of anything in
 * this function, but because of two external facts about how this suite actually runs: Playwright
 * always finishes the main/runner process's config-evaluation (which publishes the env) before
 * any worker process is spawned at all, and `playwright.config.ts` pins `workers: 1`, so there is
 * only ever ONE worker process to race against the main process, and it starts strictly after.
 * Neither fact is enforced BY this file — a future change to `workers` or to Playwright's own
 * task-scheduling order could reopen the race silently. A real fix (a file lock, or a
 * separate "claim" step) is out of scope for this task.
 */
async function adoptOrStartContainers(): Promise<{ env: RuntimeEnv; teardown: () => Promise<void> }> {
  const existing = tryReadRuntimeEnv();
  if (existing !== undefined && (await isReachable(existing))) {
    // This process did not start these containers, so it must not stop them either — the owning
    // process's own `startContainers()` teardown will.
    return { env: existing, teardown: () => Promise.resolve() };
  }
  return startContainers();
}

/** Memoised WITHIN one process — see the file header for why cross-process safety additionally
 * needs `adoptOrStartContainers`'s reachability check. Safe to call twice in the SAME process
 * (once from playwright.config.ts's top-level, once from Playwright's own `globalSetup`
 * invocation) and safe across the main/runner process and every worker process. */
export function startE2eInfrastructure(): Promise<{ env: RuntimeEnv; teardown: () => Promise<void> }> {
  started ??= adoptOrStartContainers();
  return started;
}

export default async function globalSetup(): Promise<() => Promise<void>> {
  const { teardown } = await startE2eInfrastructure();
  return teardown;
}
