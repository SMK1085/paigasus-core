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
    // The production RedisClient interface (redis-store.ts:105-111) deliberately omits `on`, so
    // this factory satisfies that interface PLUS the `on` the real node-redis client carries, and
    // is cast once here rather than widening the production type.
    const client: CapturedClient = {
      on: vi.fn(),
      connect: () => Promise.resolve(),
      get: () => Promise.resolve(null),
      set: () => Promise.resolve('OK'),
      del: () => Promise.resolve(1),
      eval: () => Promise.resolve(1),
      close: () => Promise.resolve(),
    };
    captured.options = options;
    captured.client = client;
    return client;
  },
}));

const { createRedisSessionStore } = await import('../../src/adapters/redis-store.js');

beforeEach(async () => {
  captured.options = undefined;
  captured.client = undefined;
  await createRedisSessionStore({ url: 'redis://user:pw@redis.internal:6379', commandTimeoutMs: 1234, keyPrefix: '' });
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

// GUARD 2. node-redis emits 'error' on the client's EventEmitter for connection-level failures;
// with no listener that is an unhandled event that crashes the process.
//
// The listener must also be SILENT. node-redis's own connection errors embed the DSN, which is why
// this package never logs a caught node-redis error object. A test that only asserted "does not
// throw" would stay green if `() => undefined` became `(e) => console.error(e)`, which leaks the
// DSN on every connection blip and breaks the absolute redaction rule.
describe('guard 2: the silent error listener', () => {
  it('registers a listener for the error event', () => {
    expect(captured.client?.on).toHaveBeenCalledWith('error', expect.any(Function));
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
