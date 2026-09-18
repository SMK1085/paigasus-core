// SPDX-License-Identifier: Apache-2.0
//
// `next dev` recompiles a module graph on every edit, so a MODULE-level singleton is remade each
// time and its Redis client leaks. The fix is the one SMA-511 applied to getAuthRuntime: hold the
// state on globalThis under a Symbol.for key. This test reproduces a recompile with a dynamic
// import that bypasses the module cache.
import { afterEach, describe, expect, it } from 'vitest';
import { descriptorCacheFor, resetDiscoveryForTest } from '../../src/discovery';
import type { ConsoleCoreConfig } from '../../src/config-shape';
import { createJsonLogger } from '../../src/logger';
import { redisClientFromState } from '../support/discovery-state';

const memoryConfig = { PAIGASUS_SESSION_STORE: 'memory' } as ConsoleCoreConfig;

// Port 1 refuses the connection, so no test in this file needs a Redis.
const unreachableRedisConfig = { PAIGASUS_SESSION_STORE: 'redis', PAIGASUS_SESSION_REDIS_URL: 'redis://127.0.0.1:1', PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 50 } as ConsoleCoreConfig;

const silentLog = createJsonLogger(() => undefined);

afterEach(() => {
  resetDiscoveryForTest();
});

describe('the descriptor cache survives a module reload', () => {
  it('returns the same cache to two module instances', async () => {
    const first = descriptorCacheFor(memoryConfig);
    // A fresh query string defeats the module cache, which is what a dev recompile does.
    const reloaded = (await import(`../../src/discovery?reload=${String(Date.now())}`)) as typeof import('../../src/discovery');
    const second = reloaded.descriptorCacheFor(memoryConfig);
    expect(second).toBe(first);
  });

  it('still forgets the cache when the reset seam is called', () => {
    const first = descriptorCacheFor(memoryConfig);
    resetDiscoveryForTest();
    expect(descriptorCacheFor(memoryConfig)).not.toBe(first);
  });
});

describe('the reset seam (SMA-648 D10)', () => {
  // node-redis's destroy() THROWS ClientClosedError on a client that is already closed
  // (@redis/client 6.2.1, dist/lib/client/socket.js destroy()). A throw in the seam skips the two
  // lines that clear the state, so the next caller inherits a dead cache.
  it('does not throw when the Redis client is already closed, and still clears the state', () => {
    descriptorCacheFor(unreachableRedisConfig, silentLog);
    const client = redisClientFromState();
    expect(client?.isOpen).toBe(true);
    client?.destroy();
    expect(client?.isOpen).toBe(false);
    expect(() => {
      resetDiscoveryForTest();
    }).not.toThrow();
    expect(redisClientFromState()).toBeUndefined();
  });
});
