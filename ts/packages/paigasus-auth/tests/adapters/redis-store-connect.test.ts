// SPDX-License-Identifier: Apache-2.0
//
// SMA-651 D6. node-redis's connect() keeps retrying while the reconnect strategy returns a number,
// and this store's strategy always does, so an unreachable Redis at the first request used to hold
// every request of the process. createRedisSessionStore now waits at most T.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { SessionStoreUnavailable } from '../../src/core/errors.js';

const behaviour = { connect: (): Promise<void> => Promise.resolve() };

vi.mock('redis', () => ({
  createClient: () => ({
    on: () => undefined,
    isOpen: true,
    destroy: () => undefined,
    connect: () => behaviour.connect(),
    get: () => Promise.resolve(null),
    set: () => Promise.resolve('OK'),
    del: () => Promise.resolve(1),
    eval: () => Promise.resolve(1),
  }),
}));

const { createRedisSessionStore } = await import('../../src/adapters/redis-store.js');

const T = 1000;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('createRedisSessionStore: the first connect (SMA-651 D6)', () => {
  it('is still waiting at T - 1 and returns the store at T when connect never settles', async () => {
    behaviour.connect = () => new Promise<void>(() => undefined);
    let settled = false;
    const created = createRedisSessionStore({ url: 'redis://127.0.0.1:6379', commandTimeoutMs: T, keyPrefix: '' }).then((store) => {
      settled = true;
      return store;
    });
    await vi.advanceTimersByTimeAsync(T - 1);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    await expect(created).resolves.toBeDefined();
  });

  it('throws SessionStoreUnavailable when connect rejects before T', async () => {
    behaviour.connect = () => Promise.reject(new Error('connect ECONNREFUSED redis://user:pw@host:6379'));
    const error = await createRedisSessionStore({ url: 'redis://user:pw@host:6379', commandTimeoutMs: T, keyPrefix: '' }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(SessionStoreUnavailable);
    expect((error as Error).message).not.toContain('pw');
  });

  it('leaves no pending timer when connect resolves at once', async () => {
    behaviour.connect = () => Promise.resolve();
    await createRedisSessionStore({ url: 'redis://127.0.0.1:6379', commandTimeoutMs: T, keyPrefix: '' });
    expect(vi.getTimerCount()).toBe(0);
  });
});
