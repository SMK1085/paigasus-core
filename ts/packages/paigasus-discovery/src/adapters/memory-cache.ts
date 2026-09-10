// SPDX-License-Identifier: Apache-2.0
import { parseRecord, type CacheRecord } from '../core/record.js';
import type { DescriptorCache } from '../ports/cache.js';

type Entry = { readonly raw: string; readonly expiresAt: number };
type Lock = { readonly token: string; readonly expiresAt: number };

/**
 * A single-process cache for dev and for the fast test tier.
 *
 * It stores the SERIALIZED record and reads it back through `parseRecord`, exactly as the Redis
 * adapter does. Holding the object directly would let the memory adapter pass a contract case the
 * Redis adapter fails — the divergence the shared contract suite exists to prevent.
 */
export function createMemoryDescriptorCache(now: () => number = Date.now): DescriptorCache {
  const entries = new Map<string, Entry>();
  const locks = new Map<string, Lock>();

  const live = (service: string): Entry | null => {
    const e = entries.get(service);
    if (e === undefined) return null;
    if (now() >= e.expiresAt) {
      entries.delete(service);
      return null;
    }
    return e;
  };

  return {
    get(service) {
      const e = live(service);
      if (e === null) return Promise.resolve(null);
      const parsed = parseRecord(e.raw);
      if (parsed === null) {
        entries.delete(service);
        return Promise.resolve(null);
      }
      return Promise.resolve(parsed);
    },

    set(service, rec: CacheRecord, ttlMs, expectedRev) {
      const e = live(service);
      if (expectedRev === null) {
        if (e !== null) return Promise.resolve(false);
      } else {
        if (e === null) return Promise.resolve(false);
        const cur = parseRecord(e.raw);
        if (cur === null || cur.rev !== expectedRev) return Promise.resolve(false);
      }
      entries.set(service, { raw: JSON.stringify(rec), expiresAt: now() + ttlMs });
      return Promise.resolve(true);
    },

    delete(service) {
      entries.delete(service);
      return Promise.resolve();
    },

    tryAcquireLock(service, token, ttlMs) {
      const l = locks.get(service);
      if (l !== undefined && now() < l.expiresAt) return Promise.resolve(false);
      locks.set(service, { token, expiresAt: now() + ttlMs });
      return Promise.resolve(true);
    },

    releaseLock(service, token) {
      // Compare-and-delete: a holder whose TTL expired must not release the next holder's lock.
      if (locks.get(service)?.token === token) locks.delete(service);
      return Promise.resolve();
    },

    close() {
      entries.clear();
      locks.clear();
      return Promise.resolve();
    },

    writeRawForTest(service, raw) {
      entries.set(service, { raw, expiresAt: now() + 60_000 });
      return Promise.resolve();
    },
  };
}
