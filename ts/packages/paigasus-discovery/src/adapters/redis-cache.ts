// SPDX-License-Identifier: Apache-2.0
import { SocketTimeoutError, type RedisClientType } from 'redis';
import { parseRecord, type CacheRecord } from '../core/record';
import type { DescriptorCache } from '../ports/cache';

/**
 * Compare-and-set on `rev`. Invariant 5: a lock's compare-and-delete protects the LOCK, never the
 * WRITE. ARGV[3] is the empty string to mean "only if absent".
 *
 * `tonumber` on both sides, not a string compare: cjson can render an integer as a float, the
 * same trap @paigasus/auth's SET_CAS records.
 */
const SET_CAS = `
local cur = redis.call('GET', KEYS[1])
if ARGV[3] == '' then
  if cur then return 0 end
else
  if not cur then return 0 end
  local ok, parsed = pcall(cjson.decode, cur)
  if not ok or tonumber(parsed.rev) ~= tonumber(ARGV[3]) then return 0 end
end
redis.call('SET', KEYS[1], ARGV[1], 'PX', tonumber(ARGV[2]))
return 1
`;

/** Compare-and-delete, so a holder whose TTL expired cannot release the next holder's lock. */
const UNLOCK = `
if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) end
return 0
`;

/**
 * Compare-and-delete for `get()`'s corrupt-record cleanup. Mirrors `UNLOCK` above: a bare `DEL`
 * here is wrong for the same reason a bare `DEL` would be wrong for the lock — a concurrent
 * writer can replace the invalid value we read with a VALID record between our `GET` and our
 * `DEL`, and an unconditional `DEL` would then destroy that valid record, turning a successful
 * refresh into a cache miss. Delete only if the value is still the exact string we read.
 */
const DELETE_IF_UNCHANGED = `
if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) end
return 0
`;

export type RedisDescriptorCacheOptions = {
  /** '' in production: all zones share ONE store. Tests use a random prefix per test. */
  readonly keyPrefix?: string;
};

/**
 * SMA-648 D4 (b): true when node-redis would reconnect after a socket timeout. `undefined` means
 * node-redis's default strategy, which returns `false` for a SocketTimeoutError, and `false` never
 * reconnects. A number reconnects only if it is finite and non-negative. A function is called ONCE,
 * with the arguments node-redis passes on a first timeout, and must return a finite, non-negative
 * number.
 */
function reconnectsAfterSocketTimeout(strategy: unknown, socketTimeoutMs: number): boolean {
  if (typeof strategy === 'number') return Number.isFinite(strategy) && strategy >= 0;
  if (typeof strategy !== 'function') return false;
  let delay: unknown;
  try {
    delay = (strategy as (retries: number, cause: Error) => unknown)(0, new SocketTimeoutError(socketTimeoutMs));
  } catch {
    return false;
  }
  return typeof delay === 'number' && Number.isFinite(delay) && delay >= 0;
}

/**
 * Wrap an ALREADY-CONNECTED node-redis client.
 *
 * The package never opens a connection: the composition root decides whether to share
 * @paigasus/auth's client or open a second one, and that decision is out of this issue's scope.
 *
 * The six preconditions below are ASSERTED rather than documented. A precondition nobody checks
 * is a precondition nobody keeps, and all six failures are silent until production.
 *
 * The two timeouts bound DIFFERENT phases and neither substitutes for the other.
 * `commandOptions.timeout` bounds a command only while it is QUEUED, client-side, before it is
 * written to the socket. Once node-redis has written the command, `commandOptions.timeout` no
 * longer applies.
 *
 * `socket.socketTimeout` is an IDLE timer, not a reply deadline (SMA-648, read from @redis/client
 * 6.2.1 socket.js). node-redis destroys the socket with a SocketTimeoutError after that many
 * milliseconds with no socket activity, and any read OR write resets the timer. So it bounds a
 * command in flight only while the socket is otherwise silent: under steady traffic each new write
 * moves the deadline, and a Redis that accepts commands and never replies can go unnoticed.
 * A consumer needs its own per-operation deadline; @paigasus/console-core's withOperationDeadline
 * is the worked example (SMA-650).
 *
 * `socket.socketTimeout` also fires on a quiet, healthy connection. Two more preconditions make
 * that safe. `pingInterval` keeps an idle socket alive; it must be at most half of `socketTimeout`,
 * which leaves one ping interval of margin for event-loop lag. And `socket.reconnectStrategy` must
 * return a delay for a SocketTimeoutError: node-redis's default strategy returns `false` for that
 * cause, so without one the first idle-timer close, or one lost PING reply, closes the client for
 * the life of the process.
 */
export function createRedisDescriptorCache(client: RedisClientType, options: RedisDescriptorCacheOptions = {}): DescriptorCache {
  if (client.options?.disableOfflineQueue !== true) {
    throw new Error('createRedisDescriptorCache: the client must be created with `disableOfflineQueue: true`, ' + 'or a Redis outage becomes hung page renders instead of fast failures');
  }
  if (client.listenerCount('error') === 0) {
    throw new Error(
      'createRedisDescriptorCache: the client must have an error listener, or node-redis ' + 'crashes the process on a connection error. It must never log the raw error, which embeds the DSN',
    );
  }
  const commandTimeoutMs = client.options?.commandOptions?.timeout;
  if (typeof commandTimeoutMs !== 'number' || !Number.isFinite(commandTimeoutMs) || commandTimeoutMs <= 0) {
    throw new Error(
      'createRedisDescriptorCache: the client must be created with `commandOptions: { timeout }` set to a ' +
        'positive, finite number of milliseconds, or a hung Redis blocks the render path indefinitely ' +
        'WHILE THE COMMAND IS QUEUED. This does not bound a command already in flight — see `socketTimeout` below',
    );
  }
  const socketTimeoutMs = client.options?.socket?.socketTimeout;
  if (typeof socketTimeoutMs !== 'number' || !Number.isFinite(socketTimeoutMs) || socketTimeoutMs <= 0) {
    throw new Error(
      'createRedisDescriptorCache: the client must be created with `socket: { socketTimeout }` set to a ' +
        'positive, finite number of milliseconds. It is an idle timer that any read or write resets, and it is ' +
        'the only bound on a command already written to the socket, while the socket is otherwise silent. ' +
        '`commandOptions.timeout` bounds a command only while it is queued',
    );
  }
  const pingIntervalMs = client.options?.pingInterval;
  if (typeof pingIntervalMs !== 'number' || !Number.isFinite(pingIntervalMs) || pingIntervalMs <= 0 || pingIntervalMs > socketTimeoutMs / 2) {
    throw new Error(
      'createRedisDescriptorCache: the client must be created with `pingInterval` set to a positive, finite ' +
        'number of milliseconds of at most half of socketTimeout. socketTimeout is an idle timer, so without a ' +
        'ping a quiet, healthy connection is closed; the half leaves one ping interval of margin for event-loop lag',
    );
  }
  if (!reconnectsAfterSocketTimeout(client.options?.socket?.reconnectStrategy, socketTimeoutMs)) {
    throw new Error(
      'createRedisDescriptorCache: the client must be created with a `socket.reconnectStrategy` that returns a ' +
        'finite, non-negative delay for a SocketTimeoutError. The node-redis default returns false for that cause, ' +
        'so the first idle-timer close, or one lost PING reply, would close the client for the life of the process',
    );
  }

  const prefix = options.keyPrefix ?? '';
  const recKey = (service: string): string => `${prefix}pgs:svcinfo:${service}`;
  const lockKey = (service: string): string => `${prefix}pgs:svcinfo:lock:${service}`;

  return {
    async get(service) {
      const raw = await client.get(recKey(service));
      if (raw === null) return null;
      const parsed = parseRecord(raw);
      if (parsed === null) {
        // A foreign schema version or a corrupt value. Delete it: during a rolling upgrade two
        // console builds write this key, and serving an unreadable record would poison the whole
        // fleet for the hard TTL.
        //
        // A bare `client.del(recKey(service))` here would be WRONG: a concurrent writer can
        // replace the invalid value with a VALID record between our `GET` above and this `DEL`,
        // and an unconditional delete would destroy that valid record, turning a successful
        // refresh into a cache miss. Delete only if the key still holds the exact raw string we
        // read.
        await client.eval(DELETE_IF_UNCHANGED, { keys: [recKey(service)], arguments: [raw] });
        return null;
      }
      return parsed;
    },

    async set(service, rec: CacheRecord, ttlMs, expectedRev) {
      const result = await client.eval(SET_CAS, {
        keys: [recKey(service)],
        arguments: [JSON.stringify(rec), String(ttlMs), expectedRev === null ? '' : String(expectedRev)],
      });
      return result === 1;
    },

    async delete(service) {
      await client.del(recKey(service));
    },

    async tryAcquireLock(service, token, ttlMs) {
      // A single atomic primitive; no script needed for acquisition.
      const res = await client.set(lockKey(service), token, {
        condition: 'NX',
        expiration: { type: 'PX', value: ttlMs },
      });
      return res === 'OK';
    },

    async releaseLock(service, token) {
      await client.eval(UNLOCK, { keys: [lockKey(service)], arguments: [token] });
    },

    close() {
      // The client is INJECTED, so its lifetime belongs to whoever created it. Closing it here
      // would tear down @paigasus/auth's session store when the two share one connection.
      return Promise.resolve();
    },

    async writeRawForTest(service, raw) {
      await client.set(recKey(service), raw, { expiration: { type: 'PX', value: 60_000 } });
    },
  };
}
