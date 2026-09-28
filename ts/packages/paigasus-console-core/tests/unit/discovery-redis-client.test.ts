// SPDX-License-Identifier: Apache-2.0
//
// SMA-648 § 5.2, the default tier (no Docker): the reconnect strategy, the connection-loss logger
// against a fake emitter, and a pin on the client options descriptorCacheFor passes to node-redis.
import { EventEmitter } from 'node:events';
import { inspect } from 'node:util';
import { SocketClosedUnexpectedlyError, SocketTimeoutDuringMaintenanceError, SocketTimeoutError } from 'redis';
import { afterEach, describe, expect, it } from 'vitest';
import type { ConsoleCoreConfig } from '../../src/config-shape';
import { descriptorCacheFor, descriptorCacheReconnectStrategy, resetDiscoveryForTest, watchConnectionLoss } from '../../src/discovery';
import { createJsonLogger } from '../../src/logger';
import { redisClientFromState } from '../support/discovery-state';

afterEach(() => {
  resetDiscoveryForTest();
});

describe('descriptorCacheReconnectStrategy (D3)', () => {
  it('returns a finite delay in [0, 2199] ms for every retry count from 0 to 20', () => {
    for (let retries = 0; retries <= 20; retries += 1) {
      // The jitter is random, so sample each retry count many times.
      for (let sample = 0; sample < 50; sample += 1) {
        const delay = descriptorCacheReconnectStrategy(retries);
        expect(Number.isFinite(delay)).toBe(true);
        expect(delay).toBeGreaterThanOrEqual(0);
        expect(delay).toBeLessThanOrEqual(2199);
      }
    }
  });

  it('returns a delay, never false or an Error, when node-redis passes a SocketTimeoutError', () => {
    // node-redis calls the strategy as (retries, cause). Its DEFAULT strategy returns false here.
    const asNodeRedisCallsIt = descriptorCacheReconnectStrategy as (retries: number, cause: Error) => unknown;
    expect(typeof asNodeRedisCallsIt(0, new SocketTimeoutError(1000))).toBe('number');
  });
});

/** A fake node-redis client: an EventEmitter with settable isOpen / isReady. */
class FakeClient extends EventEmitter {
  isOpen = true;
  isReady = false;
}

function watched(): { client: FakeClient; lines: string[]; lost: () => unknown[]; becomeReady: () => void; loseSocket: (error: unknown) => void } {
  const lines: string[] = [];
  const client = new FakeClient();
  watchConnectionLoss(
    client,
    createJsonLogger((line) => {
      lines.push(line);
    }),
  );
  const lost = (): unknown[] =>
    lines
      .map((line) => JSON.parse(line) as { event: string; fields: unknown })
      .filter((line) => line.event === 'discovery.redis_connection_lost')
      .map((line) => line.fields);
  const becomeReady = (): void => {
    client.isReady = true;
    client.emit('ready');
  };
  // node-redis's #onSocketError sets isReady = false BEFORE it emits, and isOpen stays true.
  const loseSocket = (error: unknown): void => {
    client.isReady = false;
    client.emit('error', error);
  };
  return { client, lines, lost, becomeReady, loseSocket };
}

describe('watchConnectionLoss (D6, D7, D8)', () => {
  it('registers exactly one error listener, which createRedisDescriptorCache requires', () => {
    const { client } = watched();
    expect(client.listenerCount('error')).toBe(1);
  });

  it('logs nothing for an error before the first ready (a connect failure)', () => {
    const { client, lines } = watched();
    client.emit('error', new Error('connect ECONNREFUSED'));
    expect(lines).toEqual([]);
  });

  it('logs nothing for an error while the socket stays ready (an error reply, e.g. -NOPERM to a PING)', () => {
    const { client, lines, becomeReady } = watched();
    becomeReady();
    client.emit('error', new Error("NOPERM this user has no permissions to run the 'ping' command"));
    expect(lines).toEqual([]);
  });

  it('logs nothing for an error after destroy() (isOpen false)', () => {
    const { client, lines, becomeReady } = watched();
    becomeReady();
    client.isOpen = false;
    client.isReady = false;
    client.emit('error', new Error('Disconnects client'));
    expect(lines).toEqual([]);
  });

  it('logs one line for a socket loss (isOpen true, isReady false)', () => {
    const { lost, becomeReady, loseSocket } = watched();
    becomeReady();
    loseSocket(new SocketTimeoutError(1000));
    expect(lost()).toEqual([{ reason: 'socket_timeout' }]);
  });

  it('logs no more lines for the handshake errors of the same reconnect loop', () => {
    const { client, lost, becomeReady, loseSocket } = watched();
    becomeReady();
    loseSocket(new SocketTimeoutError(1000));
    client.emit('error', new SocketTimeoutError(1000));
    client.emit('error', new Error('connect ECONNREFUSED'));
    expect(lost()).toEqual([{ reason: 'socket_timeout' }]);
  });

  it('logs one more line for a second loss after a new ready', () => {
    const { lost, becomeReady, loseSocket } = watched();
    becomeReady();
    loseSocket(new SocketTimeoutError(1000));
    becomeReady();
    loseSocket(new SocketClosedUnexpectedlyError());
    expect(lost()).toEqual([{ reason: 'socket_timeout' }, { reason: 'socket_closed' }]);
  });

  it.each([
    { error: new SocketTimeoutError(1000), reason: 'socket_timeout' },
    { error: new SocketClosedUnexpectedlyError(), reason: 'socket_closed' },
    // Redis Enterprise maintenance only. NOT a subclass of SocketTimeoutError (D7), so it is `other`.
    { error: new SocketTimeoutDuringMaintenanceError(1000), reason: 'other' },
    { error: new Error('connect ECONNREFUSED redis://:s3cret-password@redis.internal:6379'), reason: 'other' },
  ])('maps $error.constructor.name to reason "$reason", and never logs the message', ({ error, reason }) => {
    const { lines, lost, becomeReady, loseSocket } = watched();
    becomeReady();
    loseSocket(error);
    expect(lost()).toEqual([{ reason }]);
    expect(lines.join('\n')).not.toContain('s3cret-password');
    expect(lines.join('\n')).not.toContain(error.message);
  });
});

describe('the client options descriptorCacheFor passes to node-redis (D2, D3)', () => {
  it('sets a ping of at most half the idle timer, and the reconnect strategy', () => {
    // Port 1 refuses the connection. The options are fixed when the client is made.
    descriptorCacheFor(
      { PAIGASUS_SESSION_STORE: 'redis', PAIGASUS_SESSION_REDIS_URL: 'redis://127.0.0.1:1', PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 500 } as ConsoleCoreConfig,
      createJsonLogger(() => undefined),
    );
    const options = redisClientFromState()?.options;
    const pingInterval = options?.pingInterval;
    const socketTimeout = options?.socket?.socketTimeout;
    expect(pingInterval).toBeTypeOf('number');
    expect(socketTimeout).toBeTypeOf('number');
    expect(pingInterval as number).toBeGreaterThan(0);
    expect(pingInterval as number).toBeLessThanOrEqual((socketTimeout as number) / 2);
    expect(options?.socket?.reconnectStrategy).toBe(descriptorCacheReconnectStrategy);
  });
});

// SMA-715 (spec § 6.2). A local copy of the auth helper: the packages share no test support.
const SENTINEL = 'SMA715SENTINELPW';
const MESSAGE = 'PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL';

/** Every throw path of node-redis 6.2.1's parseURL (spec § 1.1). Each error class MEASURED 2026-09-27. */
const MALFORMED: ReadonlyArray<readonly [label: string, url: string]> = [
  ['no scheme colon (ERR_INVALID_URL, the password is in `input`)', `redis//u:${SENTINEL}@h`],
  ['a port above 65535 (ERR_INVALID_URL, the password is in `input`)', `redis://u:${SENTINEL}@h:99999`],
  ['a scheme other than redis:, rediss: or unix: (Invalid protocol)', `http://u:${SENTINEL}@h`],
  ['a database path that is not a number (Invalid pathname)', `redis://u:${SENTINEL}@h/abc`],
  ['a bad percent escape in the password (URIError)', `redis://u:%zz${SENTINEL}@h`],
  ['a unix URL with a host (Invalid unix URL)', `unix://u:${SENTINEL}@/`],
  ['a unix URL with a db parameter that is not a number (Invalid db query parameter)', `unix://u:${SENTINEL}@/tmp/s?db=x`],
  ['a unix URL with a bad escape in the path (URIError)', `unix://u:${SENTINEL}@/tmp/%zz`],
];

const BLANK: ReadonlyArray<readonly [label: string, url: string]> = [
  ['empty', ''],
  ['spaces only', '   '],
  ['tab and newline', '\t\n'],
];

/** AC3: every own property name and value, enumerable or not, walked recursively (so each `cause`). */
function walkText(value: unknown, seen: Set<object> = new Set()): string[] {
  if (typeof value !== 'object' || value === null) return [String(value)];
  if (seen.has(value)) return [];
  seen.add(value);
  const out: string[] = [];
  for (const name of Object.getOwnPropertyNames(value)) {
    out.push(name);
    out.push(...walkText((value as Record<string, unknown>)[name], seen));
  }
  return out;
}

function expectFixedError(err: unknown): void {
  expect(err).toBeInstanceOf(Error);
  // A PLAIN Error, the same class as the missing-URL throw of descriptorCacheFor.
  expect(Object.getPrototypeOf(err)).toBe(Error.prototype);
  expect((err as Error).message).toBe(MESSAGE);
  expect((err as Error).cause).toBeUndefined();
  const texts = [...walkText(err), String(err), inspect(err, { showHidden: true, depth: Infinity })];
  for (const text of texts) expect(text).not.toContain(SENTINEL);
}

describe('descriptorCacheFor with a malformed or blank PAIGASUS_SESSION_REDIS_URL (SMA-715)', () => {
  const quiet = createJsonLogger(() => undefined);
  const configFor = (url: string): ConsoleCoreConfig => ({ PAIGASUS_SESSION_STORE: 'redis', PAIGASUS_SESSION_REDIS_URL: url, PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 500 }) as ConsoleCoreConfig;
  /** What descriptorCacheFor threw, or `undefined`. afterEach's resetDiscoveryForTest destroys any client. */
  const thrownBy = (url: string): unknown => {
    try {
      descriptorCacheFor(configFor(url), quiet);
      return undefined;
    } catch (err) {
      return err;
    }
  };

  it.each(MALFORMED)('%s -> the fixed Error, no client in state, and a valid URL then works (AC2, G3)', (_label, url) => {
    expectFixedError(thrownBy(url));
    expect(redisClientFromState()).toBeUndefined();
    // Port 1 refuses the connection. The client is made at once; the connect runs in the background.
    expect(thrownBy('redis://127.0.0.1:1')).toBeUndefined();
    expect(redisClientFromState()).toBeDefined();
  });

  it.each(BLANK)('%s -> the fixed Error and no client in state (AC8, D5)', (_label, url) => {
    expectFixedError(thrownBy(url));
    expect(redisClientFromState()).toBeUndefined();
  });

  it('a valid URL with outer whitespace is NOT refused and puts a client in state (control)', () => {
    expect(thrownBy(' redis://127.0.0.1:1 ')).toBeUndefined();
    expect(redisClientFromState()).toBeDefined();
  });
});
