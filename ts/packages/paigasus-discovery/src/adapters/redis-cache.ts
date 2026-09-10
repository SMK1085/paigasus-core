// SPDX-License-Identifier: Apache-2.0
import type { RedisClientType } from 'redis';
import { parseRecord, type CacheRecord } from '../core/record.js';
import type { DescriptorCache } from '../ports/cache.js';

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

export type RedisDescriptorCacheOptions = {
  /** '' in production: all zones share ONE store. Tests use a random prefix per test. */
  readonly keyPrefix?: string;
};

/**
 * Wrap an ALREADY-CONNECTED node-redis client.
 *
 * The package never opens a connection: the composition root decides whether to share
 * @paigasus/auth's client or open a second one, and that decision is out of this issue's scope.
 *
 * The two preconditions below are ASSERTED rather than documented. A precondition nobody checks
 * is a precondition nobody keeps, and both failures are silent until production.
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
        await client.del(recKey(service));
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
