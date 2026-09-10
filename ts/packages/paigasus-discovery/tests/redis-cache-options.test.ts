// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { createRedisDescriptorCache } from '../src/adapters/redis-cache.js';

function fakeClient(over: Record<string, unknown> = {}): never {
  return {
    options: { disableOfflineQueue: true },
    listenerCount: () => 1,
    ...over,
  } as never;
}

describe('createRedisDescriptorCache preconditions', () => {
  it('accepts a correctly configured client', () => {
    expect(() => createRedisDescriptorCache(fakeClient())).not.toThrow();
  });

  it('refuses a client without disableOfflineQueue', () => {
    // Without it a Redis outage becomes HUNG REQUESTS rather than fast failures — node-redis
    // queues commands while disconnected. @paigasus/auth records this as load-bearing.
    expect(() => createRedisDescriptorCache(fakeClient({ options: { disableOfflineQueue: false } }))).toThrow(/disableOfflineQueue/);
  });

  it('refuses a client with no error listener', () => {
    // node-redis emits 'error' on an EventEmitter; with no listener Node crashes the process.
    expect(() => createRedisDescriptorCache(fakeClient({ listenerCount: () => 0 }))).toThrow(/error listener/);
  });
});
