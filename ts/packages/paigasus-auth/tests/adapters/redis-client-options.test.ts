// SPDX-License-Identifier: Apache-2.0
//
// SMA-626 § 5, guards 1 and 2. Both are commented LOAD-BEARING in redis-store.ts and neither would
// have red if deleted. These tests are the answer to "what would fail if this were deleted?".
//
// MEASURED 2026-09-10. Deleting `disableOfflineQueue: true` reds guard 1. Deleting the
// `client.on('error', ...)` line reds both guard-2 tests. Replacing the silent handler with
// `(e) => console.error(e)` reds the DSN assertion while leaving the registration assertion green,
// which is exactly the case a survival-only test would have missed.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

interface CapturedClient {
  on: ReturnType<typeof vi.fn>;
  isOpen: boolean;
  destroy: ReturnType<typeof vi.fn>;
  connect: () => Promise<void>;
  get: () => Promise<string | null>;
  set: () => Promise<string | null>;
  del: () => Promise<number>;
  eval: () => Promise<unknown>;
  close: () => Promise<void>;
}

const captured = { options: undefined as Record<string, unknown> | undefined, client: undefined as CapturedClient | undefined };

vi.mock('redis', () => ({
  createClient: (options: Record<string, unknown>) => {
    // The factory satisfies the production RedisClient duck type (redis-store.ts) plus the
    // `connect` the factory calls. `destroy` mirrors node-redis 6.2.1 exactly: it THROWS when the
    // client is no longer open (@redis/client socket.js destroy()), so a double destroy reds here
    // instead of passing on a forgiving fake.
    const client: CapturedClient = {
      on: vi.fn(),
      isOpen: true,
      destroy: vi.fn(() => {
        if (!client.isOpen) throw new Error('ClientClosedError');
        client.isOpen = false;
      }),
      connect: () => Promise.resolve(),
      get: () => Promise.resolve(null),
      set: () => Promise.resolve('OK'),
      del: () => Promise.resolve(1),
      eval: () => Promise.resolve(1),
      close: () => Promise.reject(new Error('graceful close() must not be called (SMA-651 D5)')),
    };
    captured.options = options;
    captured.client = client;
    return client;
  },
}));

const { createRedisSessionStore } = await import('../../src/adapters/redis-store.js');

let store: Awaited<ReturnType<typeof createRedisSessionStore>>;

beforeEach(async () => {
  captured.options = undefined;
  captured.client = undefined;
  store = await createRedisSessionStore({ url: 'redis://user:pw@redis.internal:6379', commandTimeoutMs: 1234, keyPrefix: '' });
});

afterEach(() => {
  vi.restoreAllMocks();
});

// GUARD 1. node-redis QUEUES commands while disconnected by default, so an outage becomes hung
// requests instead of fast failures and every page in the console stalls rather than erroring.
// Deleting the flag reds this test.
//
// NAMED RESIDUAL: this proves the flag is SET. It does NOT prove node-redis then fails fast during
// a real outage — that needs a container test that stops Redis mid-suite, rejected as slow and
// flake-prone (spec § 5).
describe('guard 1: disableOfflineQueue', () => {
  it('is passed to createClient as true', () => {
    expect(captured.options?.['disableOfflineQueue']).toBe(true);
  });

  it('passes the command timeout and the reconnect strategy alongside it', () => {
    expect(captured.options?.['commandOptions']).toEqual({ timeout: 1234 });
    const socket = captured.options?.['socket'] as { connectTimeout: number; reconnectStrategy: unknown };
    expect(socket.connectTimeout).toBe(1234);
    expect(typeof socket.reconnectStrategy).toBe('function');
  });
});

// SMA-651 D1. pingInterval keeps a healthy idle socket under the idle timer; socketTimeout is the
// only thing that tears down a wedged socket.
describe('SMA-651: the idle timer and the ping', () => {
  it('sets pingInterval to T and socketTimeout to 2T', () => {
    expect(captured.options?.['pingInterval']).toBe(1234);
    const socket = captured.options?.['socket'] as { socketTimeout: number };
    expect(socket.socketTimeout).toBe(2468);
  });

  it('keeps pingInterval at or below half of socketTimeout', () => {
    const socket = captured.options?.['socket'] as { socketTimeout: number };
    expect(captured.options?.['pingInterval'] as number).toBeLessThanOrEqual(socket.socketTimeout / 2);
  });

  // The CAPTURED strategy, not the export: this proves the wiring as well as the function.
  // node-redis's DEFAULT strategy returns false for a SocketTimeoutError, which closes the client
  // for good (SMA-648). This one must return a number for it.
  it('the strategy passed to createClient reconnects after a SocketTimeoutError', async () => {
    const { SocketTimeoutError } = await vi.importActual<typeof import('redis')>('redis');
    const strategy = (captured.options?.['socket'] as { reconnectStrategy: (retries: number, cause: Error) => unknown }).reconnectStrategy;
    for (let retries = 0; retries <= 50; retries += 1) {
      expect(typeof strategy(retries, new SocketTimeoutError(2468))).toBe('number');
    }
  });
});

// SMA-651 D5. close() destroys at once, and only while the client is still open.
describe('SMA-651: close()', () => {
  it('destroys once and does not throw when called twice', async () => {
    await store.close();
    await store.close();
    expect(captured.client?.destroy).toHaveBeenCalledTimes(1);
  });
});

// GUARD 2. node-redis emits 'error' on the client's EventEmitter for connection-level failures;
// with no listener that is an unhandled event that crashes the process.
//
// The listener must also be SILENT. node-redis's own connection errors embed the DSN, which is why
// this package never logs a caught node-redis error object. A test that only asserted "does not
// throw" would stay green if `() => undefined` became `(e) => console.error(e)`, which leaks the
// DSN on every connection blip and breaks the absolute redaction rule.
describe('guard 2: the silent error listener', () => {
  it('registers exactly one listener for the error event', () => {
    const errorListeners = captured.client?.on.mock.calls.filter(([event]) => event === 'error') ?? [];
    expect(errorListeners).toHaveLength(1);
  });

  it('the handler neither throws nor reports the DSN anywhere', () => {
    const spies = {
      error: vi.spyOn(console, 'error').mockImplementation(() => undefined),
      warn: vi.spyOn(console, 'warn').mockImplementation(() => undefined),
      log: vi.spyOn(console, 'log').mockImplementation(() => undefined),
      info: vi.spyOn(console, 'info').mockImplementation(() => undefined),
      debug: vi.spyOn(console, 'debug').mockImplementation(() => undefined),
    };

    const handler = captured.client?.on.mock.calls.find(([event]) => event === 'error')?.[1] as (err: Error) => void;
    expect(handler).toBeTypeOf('function');

    expect(() => {
      handler(new Error('connect ECONNREFUSED redis://user:pw@redis.internal:6379'));
    }).not.toThrow();

    for (const spy of Object.values(spies)) expect(spy).not.toHaveBeenCalled();
  });
});
