// SPDX-License-Identifier: Apache-2.0
//
// AC 2 across PROCESSES, not just within one. The single-flight lock lives in Redis, so two
// separate node processes contending on the same sid must still trigger exactly one refresh — the
// shape of the real failure this package exists to prevent: two console ZONES are two processes,
// not two async callers sharing one event loop. tests/core/single-flight.test.ts and
// tests/containers/single-flight-redis.test.ts already prove the in-process and single-process
// Redis cases; this is the one only a fork can prove.
//
// NO SKIP. If Docker is unreachable this FAILS — see tests/containers/redis-store.test.ts for why
// container suites carry no skip hatch.
import { fork } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { createClient } from 'redis';
import { GenericContainer, type StartedTestContainer } from 'testcontainers';
import { afterAll, beforeAll, expect, it } from 'vitest';
import { makeRecord } from '../store-contract.js';

let container: StartedTestContainer;
let url: string;
let admin: ReturnType<typeof createClient>;

beforeAll(async () => {
  container = await new GenericContainer('redis:8-alpine').withExposedPorts(6379).start();
  url = `redis://${container.getHost()}:${String(container.getMappedPort(6379))}/0`;
  admin = createClient({ url });
  await admin.connect();
}, 120_000);

afterAll(async () => {
  await admin.close();
  await container.stop();
});

const workerPath = fileURLToPath(new URL('../fixtures/refresh-worker.ts', import.meta.url));
// See tests/fixtures/ts-esm-loader.mjs: a bare `node` process needs this to resolve this
// package's `.js`-suffixed relative imports onto their `.ts` source siblings.
const loaderUrl = pathToFileURL(fileURLToPath(new URL('../fixtures/ts-esm-loader.mjs', import.meta.url))).href;

interface WorkerResult {
  accessToken: string | null;
}

function runWorker(redisUrl: string, keyPrefix: string, sid: string): Promise<WorkerResult> {
  return new Promise((resolvePromise, reject) => {
    const child = fork(workerPath, [redisUrl, keyPrefix, sid], {
      // --experimental-transform-types (not the bare default strip-only mode): src/core/errors.ts
      // is reached transitively via the redis-store import and uses a TS parameter property,
      // which strip-only mode rejects outright (ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX, measured).
      execArgv: ['--experimental-transform-types', '--import', loaderUrl],
      stdio: ['ignore', 'pipe', 'inherit', 'ipc'],
    });
    let out = '';
    child.stdout?.on('data', (chunk: Buffer) => {
      out += chunk.toString('utf8');
    });
    child.on('error', reject);
    child.on('exit', (code) => {
      if (code !== 0) {
        reject(new Error(`refresh-worker exited with code ${String(code)}`));
        return;
      }
      try {
        resolvePromise(JSON.parse(out.trim()) as WorkerResult);
      } catch (err) {
        reject(err instanceof Error ? err : new Error(String(err)));
      }
    });
  });
}

it('two forked processes racing an expired token trigger exactly one refresh', async () => {
  const keyPrefix = `mp${String(Date.now())}${String(Math.random())}:`;
  const sid = 's';

  // Seed with a raw SET rather than through the store: this fixture must stay store-agnostic,
  // and the wire shape (JSON, `version: 1`) is exactly what RedisSessionStore.get expects.
  const sessKey = `${keyPrefix}pgs:sess:${sid}`;
  const record = makeRecord({ accessExpiresAt: Date.now() - 1 });
  await admin.set(sessKey, JSON.stringify(record));

  const [a, b] = await Promise.all([runWorker(url, keyPrefix, sid), runWorker(url, keyPrefix, sid)]);

  const count = await admin.get('refresh:count');
  expect(count).toBe('1');
  expect(a.accessToken).not.toBeNull();
  expect(a.accessToken).toBe(b.accessToken);
}, 30_000);
