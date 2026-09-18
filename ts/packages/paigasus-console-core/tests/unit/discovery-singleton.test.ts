// SPDX-License-Identifier: Apache-2.0
//
// `next dev` recompiles a module graph on every edit, so a MODULE-level singleton is remade each
// time and its Redis client leaks. The fix is the one SMA-511 applied to getAuthRuntime: hold the
// state on globalThis under a Symbol.for key. This test reproduces a recompile with a dynamic
// import that bypasses the module cache.
import { afterEach, describe, expect, it } from 'vitest';
import { descriptorCacheFor, resetDiscoveryForTest } from '../../src/discovery';
import type { ConsoleCoreConfig } from '../../src/config-shape';

const memoryConfig = { PAIGASUS_SESSION_STORE: 'memory' } as ConsoleCoreConfig;

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
