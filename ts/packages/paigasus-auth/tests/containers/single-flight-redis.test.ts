// SPDX-License-Identifier: Apache-2.0
//
// Re-runs the AC 2 concurrency assertions from tests/core/single-flight.test.ts against a real
// Redis container. What this proves that the in-memory run does not is that the Lua
// compare-and-set (SET_CAS) and compare-and-delete (UNLOCK) scripts in src/adapters/redis-store.ts
// are correct: MemorySessionStore's checks are synchronous JS and never exercise them.
//
// NO SKIP. If Docker is unreachable this FAILS — see tests/containers/redis-store.test.ts for why
// container suites carry no skip hatch.
import { GenericContainer, type StartedTestContainer } from 'testcontainers';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createRedisSessionStore } from '../../src/adapters/redis-store.js';
import { noopLogger } from '../../src/adapters/noop-logger.js';
import { resolveSession } from '../../src/core/single-flight.js';
import { makeRecord } from '../store-contract.js';
import type { SessionStore } from '../../src/ports/session-store.js';

let container: StartedTestContainer;
let url: string;

beforeAll(async () => {
  container = await new GenericContainer('redis:8-alpine').withExposedPorts(6379).start();
  url = `redis://${container.getHost()}:${String(container.getMappedPort(6379))}/0`;
}, 120_000);

afterAll(async () => {
  await container.stop();
});

function deps(store: SessionStore, refresh: (rt: string) => Promise<{ accessToken: string; refreshToken?: string; expiresIn: number }>) {
  return { store, refresh, logger: noopLogger, skewMs: 30_000, lockTtlMs: 5_000, lockWaitMs: 3_000, ttlMs: 60_000 };
}

// A fresh keyPrefix per test isolates it from every other test sharing the one container.
const makeStore = (): Promise<SessionStore> => createRedisSessionStore({ url, commandTimeoutMs: 1000, keyPrefix: `sf${String(Date.now())}${String(Math.random())}:` });

describe('resolveSession against Redis', () => {
  it('two concurrent callers on an expired token trigger exactly one refresh', async () => {
    const store = await makeStore();
    try {
      let calls = 0;
      const refresh = async () => {
        calls += 1;
        await new Promise((r) => setTimeout(r, 200)); // hold the lock long enough to force contention
        return { accessToken: `AT${String(calls)}`, refreshToken: `RT${String(calls)}`, expiresIn: 300 };
      };
      await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);

      const results = await Promise.all([resolveSession(deps(store, refresh), 's'), resolveSession(deps(store, refresh), 's')]);

      expect(calls).toBe(1);
      expect(results.map((r) => r?.accessToken)).toEqual(['AT1', 'AT1']);
    } finally {
      await store.close();
    }
  });

  it('twenty concurrent callers still trigger exactly one refresh', async () => {
    const store = await makeStore();
    try {
      let calls = 0;
      const refresh = async () => {
        calls += 1;
        await new Promise((r) => setTimeout(r, 200));
        return { accessToken: `AT${String(calls)}`, refreshToken: 'RT2', expiresIn: 300 };
      };
      await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);

      const results = await Promise.all(Array.from({ length: 20 }, () => resolveSession(deps(store, refresh), 's')));

      expect(calls).toBe(1);
      expect(new Set(results.map((r) => r?.accessToken))).toEqual(new Set(['AT1']));
    } finally {
      await store.close();
    }
  });

  it('the rotated refresh token replaces the old one', async () => {
    const store = await makeStore();
    try {
      const refresh = (rt: string) => {
        expect(rt).toBe('RT');
        return Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 });
      };
      await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
      await resolveSession(deps(store, refresh), 's');
      expect((await store.get('s'))?.refreshToken).toBe('RT2');
    } finally {
      await store.close();
    }
  });
});
