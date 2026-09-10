// SPDX-License-Identifier: Apache-2.0
//
// ONE contract, run against EVERY adapter. A semantic divergence between the memory adapter and
// Redis — a non-atomic lock, a fence that does not fence, a version mismatch that is served
// rather than deleted — then fails in CI instead of in production. This mirrors
// ts/packages/paigasus-auth/tests/store-contract.ts.
import { afterEach, describe, expect, it } from 'vitest';
import { RECORD_VERSION, type CacheRecord } from '../src/core/record.js';
import type { DescriptorCache } from '../src/ports/cache.js';

function rec(over: Partial<CacheRecord> = {}): CacheRecord {
  return {
    version: RECORD_VERSION,
    rev: 1,
    descriptor: { service: 'iam', version: '0.0.0', capabilities: ['iam.audit'] },
    descriptorAt: 0,
    outcome: 'ok',
    outcomeAt: 0,
    reason: null,
    ...over,
  };
}

export function runCacheContract(name: string, makeCache: () => Promise<DescriptorCache>): void {
  describe(`DescriptorCache contract: ${name}`, () => {
    let cache: DescriptorCache;
    afterEach(async () => {
      await cache?.close();
    });

    it('returns null for an unknown service', async () => {
      cache = await makeCache();
      expect(await cache.get('iam')).toBeNull();
    });

    it('stores and reads back a record', async () => {
      cache = await makeCache();
      expect(await cache.set('iam', rec(), 60_000, null)).toBe(true);
      expect(await cache.get('iam')).toEqual(rec());
    });

    it('refuses an insert when the key already exists (expectedRev null)', async () => {
      cache = await makeCache();
      await cache.set('iam', rec(), 60_000, null);
      expect(await cache.set('iam', rec({ rev: 9 }), 60_000, null)).toBe(false);
    });

    it('fences a write on rev', async () => {
      cache = await makeCache();
      await cache.set('iam', rec({ rev: 1 }), 60_000, null);
      expect(await cache.set('iam', rec({ rev: 2 }), 60_000, 1)).toBe(true);
      // A writer still holding rev 1 has lost and must be rejected.
      expect(await cache.set('iam', rec({ rev: 2, outcome: 'fail', reason: 'network' }), 60_000, 1)).toBe(false);
      expect((await cache.get('iam'))?.outcome).toBe('ok');
    });

    it('deletes', async () => {
      cache = await makeCache();
      await cache.set('iam', rec(), 60_000, null);
      await cache.delete('iam');
      expect(await cache.get('iam')).toBeNull();
    });

    it('keeps each service in its own entry', async () => {
      cache = await makeCache();
      await cache.set('iam', rec(), 60_000, null);
      await cache.set('gateway', rec({ rev: 5 }), 60_000, null);
      expect((await cache.get('iam'))?.rev).toBe(1);
      expect((await cache.get('gateway'))?.rev).toBe(5);
    });

    it('grants a lock to exactly one holder', async () => {
      cache = await makeCache();
      expect(await cache.tryAcquireLock('iam', 'token-a', 5_000)).toBe(true);
      expect(await cache.tryAcquireLock('iam', 'token-b', 5_000)).toBe(false);
    });

    it('releases a lock it holds', async () => {
      cache = await makeCache();
      await cache.tryAcquireLock('iam', 'token-a', 5_000);
      await cache.releaseLock('iam', 'token-a');
      expect(await cache.tryAcquireLock('iam', 'token-b', 5_000)).toBe(true);
    });

    it('refuses to release a lock held by someone else', async () => {
      // Compare-and-delete. A holder whose TTL expired must not release the NEXT holder's lock.
      cache = await makeCache();
      await cache.tryAcquireLock('iam', 'token-a', 5_000);
      await cache.releaseLock('iam', 'token-b');
      expect(await cache.tryAcquireLock('iam', 'token-c', 5_000)).toBe(false);
    });

    it('locks one service without locking another', async () => {
      cache = await makeCache();
      await cache.tryAcquireLock('iam', 'token-a', 5_000);
      expect(await cache.tryAcquireLock('gateway', 'token-b', 5_000)).toBe(true);
    });

    it('discards a record written by a different schema version', async () => {
      cache = await makeCache();
      await cache.set('iam', rec(), 60_000, null);
      await cache.writeRawForTest?.('iam', JSON.stringify({ ...rec(), version: 99 }));
      expect(await cache.get('iam')).toBeNull();
    });
  });
}
