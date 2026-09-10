// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { createRedisDescriptorCache } from '../src/adapters/redis-cache.js';

function fakeClient(over: Record<string, unknown> = {}): never {
  return {
    options: { disableOfflineQueue: true, commandOptions: { timeout: 1_000 }, socket: { socketTimeout: 2_000 } },
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

  it('refuses a client with no command timeout', () => {
    // Without a per-command timeout a hung Redis blocks the render path indefinitely — the same
    // class of failure disableOfflineQueue guards against, on a path that check cannot reach.
    expect(() => createRedisDescriptorCache(fakeClient({ options: { disableOfflineQueue: true } }))).toThrow(/timeout/);
  });

  it('refuses a client with a non-positive command timeout', () => {
    expect(() => createRedisDescriptorCache(fakeClient({ options: { disableOfflineQueue: true, commandOptions: { timeout: 0 } } }))).toThrow(/timeout/);
  });

  it('refuses a client missing socketTimeout', () => {
    // commandOptions.timeout only bounds a command while it is QUEUED. Without socketTimeout a
    // Redis that accepts a command and never answers blocks the render path indefinitely — the
    // same class of failure the queued-phase check cannot reach.
    expect(() => createRedisDescriptorCache(fakeClient({ options: { disableOfflineQueue: true, commandOptions: { timeout: 1_000 } } }))).toThrow(/socketTimeout/);
  });

  it('refuses a client with a non-positive socketTimeout', () => {
    expect(() => createRedisDescriptorCache(fakeClient({ options: { disableOfflineQueue: true, commandOptions: { timeout: 1_000 }, socket: { socketTimeout: 0 } } }))).toThrow(/socketTimeout/);
  });
});
