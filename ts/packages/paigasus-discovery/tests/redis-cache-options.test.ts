// SPDX-License-Identifier: Apache-2.0
import { SocketTimeoutError } from 'redis';
import { describe, expect, it } from 'vitest';
import { createRedisDescriptorCache } from '../src/adapters/redis-cache.js';

// A client that satisfies ALL SIX preconditions. Each row below breaks exactly one of them.
const VALID_OPTIONS = {
  disableOfflineQueue: true,
  commandOptions: { timeout: 1_000 },
  socket: { socketTimeout: 2_000, reconnectStrategy: (): number => 100 },
  pingInterval: 1_000,
};

// One fragment per precondition, and each fragment occurs ONLY in that precondition's message
// (SMA-648 § 4.3). `expectRefusedBy` also asserts that the message matches no OTHER fragment, so a
// deleted check cannot hide behind a later check's message.
const DISABLE_OFFLINE_QUEUE = /created with `disableOfflineQueue: true`/;
const ERROR_LISTENER = /must have an error listener/;
const COMMAND_TIMEOUT = /created with `commandOptions: \{ timeout \}`/;
const SOCKET_TIMEOUT = /created with `socket: \{ socketTimeout \}`/;
const PING_INTERVAL = /created with `pingInterval`/;
const RECONNECT_STRATEGY = /created with a `socket\.reconnectStrategy`/;
const FRAGMENTS = [DISABLE_OFFLINE_QUEUE, ERROR_LISTENER, COMMAND_TIMEOUT, SOCKET_TIMEOUT, PING_INTERVAL, RECONNECT_STRATEGY];

function fakeClient(over: Record<string, unknown> = {}): never {
  return { options: VALID_OPTIONS, listenerCount: () => 1, ...over } as never;
}

function withOptions(over: Record<string, unknown>): never {
  return fakeClient({ options: { ...VALID_OPTIONS, ...over } });
}

function withSocket(over: Record<string, unknown>): never {
  return withOptions({ socket: { ...VALID_OPTIONS.socket, ...over } });
}

function expectRefusedBy(client: never, fragment: RegExp): void {
  let message: string | undefined;
  try {
    createRedisDescriptorCache(client);
  } catch (error) {
    message = error instanceof Error ? error.message : 'a non-Error was thrown';
  }
  expect(message, 'createRedisDescriptorCache accepted the client').toBeDefined();
  expect(message).toMatch(fragment);
  for (const other of FRAGMENTS.filter((candidate) => candidate !== fragment)) {
    expect(message).not.toMatch(other);
  }
}

describe('createRedisDescriptorCache preconditions', () => {
  it('accepts a correctly configured client', () => {
    expect(() => createRedisDescriptorCache(fakeClient())).not.toThrow();
  });

  it('refuses a client without disableOfflineQueue', () => {
    // Without it a Redis outage becomes HUNG REQUESTS rather than fast failures — node-redis
    // queues commands while disconnected. @paigasus/auth records this as load-bearing.
    expectRefusedBy(withOptions({ disableOfflineQueue: false }), DISABLE_OFFLINE_QUEUE);
  });

  it('refuses a client with no error listener', () => {
    // node-redis emits 'error' on an EventEmitter; with no listener Node crashes the process.
    expectRefusedBy(fakeClient({ listenerCount: () => 0 }), ERROR_LISTENER);
  });

  it('refuses a client with no command timeout', () => {
    // Without a per-command timeout a hung Redis blocks the render path indefinitely — the same
    // class of failure disableOfflineQueue guards against, on a path that check cannot reach.
    expectRefusedBy(withOptions({ commandOptions: undefined }), COMMAND_TIMEOUT);
  });

  it('refuses a client with a non-positive command timeout', () => {
    expectRefusedBy(withOptions({ commandOptions: { timeout: 0 } }), COMMAND_TIMEOUT);
  });

  it('refuses a client missing socketTimeout', () => {
    // commandOptions.timeout only bounds a command while it is QUEUED. socketTimeout is the only
    // bound on a command in flight, and only while the socket is otherwise silent.
    expectRefusedBy(withSocket({ socketTimeout: undefined }), SOCKET_TIMEOUT);
  });

  it('refuses a client with a non-positive socketTimeout', () => {
    expectRefusedBy(withSocket({ socketTimeout: 0 }), SOCKET_TIMEOUT);
  });

  // SMA-648 D4 (a). socketTimeout is an IDLE timer: without a ping, a quiet, healthy connection is
  // closed. The ping must be at most half of socketTimeout, which leaves one ping interval of
  // margin for event-loop lag.
  it('refuses a client missing pingInterval', () => {
    expectRefusedBy(withOptions({ pingInterval: undefined }), PING_INTERVAL);
  });

  it('refuses a client with a pingInterval of 0', () => {
    expectRefusedBy(withOptions({ pingInterval: 0 }), PING_INTERVAL);
  });

  it.each([Number.NaN, Number.POSITIVE_INFINITY])('refuses a client with a pingInterval that is not finite (%s)', (pingInterval) => {
    expectRefusedBy(withOptions({ pingInterval }), PING_INTERVAL);
  });

  it('refuses a client with a pingInterval above socketTimeout / 2', () => {
    expectRefusedBy(withOptions({ pingInterval: 1_001 }), PING_INTERVAL);
  });

  it('accepts a pingInterval of exactly socketTimeout / 2', () => {
    expect(() => createRedisDescriptorCache(withOptions({ pingInterval: 1_000 }))).not.toThrow();
  });

  // SMA-648 D4 (b). node-redis's DEFAULT strategy (`undefined`) returns false for a
  // SocketTimeoutError, so the first idle-timer close, or one lost PING reply, would close the
  // client for the life of the process.
  it('refuses a client with no reconnectStrategy (node-redis default)', () => {
    expectRefusedBy(withSocket({ reconnectStrategy: undefined }), RECONNECT_STRATEGY);
  });

  it('refuses a client with reconnectStrategy false', () => {
    expectRefusedBy(withSocket({ reconnectStrategy: false }), RECONNECT_STRATEGY);
  });

  it('refuses a strategy function that returns false', () => {
    expectRefusedBy(withSocket({ reconnectStrategy: () => false }), RECONNECT_STRATEGY);
  });

  it('refuses a strategy function that returns an Error', () => {
    expectRefusedBy(withSocket({ reconnectStrategy: () => new Error('stop') }), RECONNECT_STRATEGY);
  });

  it('refuses a strategy that returns false only for a SocketTimeoutError (the default strategy shape)', () => {
    // Proves the probe passes a REAL SocketTimeoutError, not just any error.
    expectRefusedBy(withSocket({ reconnectStrategy: (_retries: number, cause: Error) => (cause instanceof SocketTimeoutError ? false : 100) }), RECONNECT_STRATEGY);
  });

  it('refuses a strategy function that throws', () => {
    expectRefusedBy(
      withSocket({
        reconnectStrategy: () => {
          throw new Error('boom');
        },
      }),
      RECONNECT_STRATEGY,
    );
  });

  it.each([Number.NaN, -1])('refuses a strategy function that returns %s', (delay) => {
    expectRefusedBy(withSocket({ reconnectStrategy: () => delay }), RECONNECT_STRATEGY);
  });

  it.each([0, 500])('accepts a numeric reconnectStrategy (%s)', (reconnectStrategy) => {
    expect(() => createRedisDescriptorCache(withSocket({ reconnectStrategy }))).not.toThrow();
  });

  it.each([Number.NaN, -1, Number.POSITIVE_INFINITY])('refuses a numeric reconnectStrategy of %s', (reconnectStrategy) => {
    expectRefusedBy(withSocket({ reconnectStrategy }), RECONNECT_STRATEGY);
  });

  it('calls a strategy function once, with retries 0 and a SocketTimeoutError', () => {
    const calls: unknown[][] = [];
    const reconnectStrategy = (...args: unknown[]): number => {
      calls.push(args);
      return 0;
    };
    expect(() => createRedisDescriptorCache(withSocket({ reconnectStrategy }))).not.toThrow();
    expect(calls).toHaveLength(1);
    expect(calls[0]?.[0]).toBe(0);
    expect(calls[0]?.[1]).toBeInstanceOf(SocketTimeoutError);
  });
});
