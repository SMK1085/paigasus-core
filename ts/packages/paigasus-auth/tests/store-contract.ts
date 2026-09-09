// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import type { SessionStore } from '../src/ports/session-store.js';
import type { SessionRecord } from '../src/core/session.js';

export function makeRecord(over: Partial<SessionRecord> = {}): SessionRecord {
  return {
    version: 1,
    rev: 0,
    accessToken: 'AT',
    refreshToken: 'RT',
    accessExpiresAt: Date.now() + 300_000,
    absoluteExpiresAt: Date.now() + 86_400_000,
    idTokenClaims: { iss: 'https://idp', sub: 'u1' },
    principal: { principalPrn: null, issuer: 'https://idp', subject: 'u1', memberships: [], roleGrants: [], grantsAvailable: false },
    ...over,
  };
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * ONE suite, run against EVERY adapter. This is what proves the port is honest: an adapter that
 * quietly differs on takeTransaction atomicity or compare-and-delete semantics fails here rather
 * than being discovered in production.
 */
export function runStoreContract(name: string, makeStore: () => Promise<SessionStore>): void {
  describe(`SessionStore contract: ${name}`, () => {
    it('returns null for an unknown sid', async () => {
      const s = await makeStore();
      try {
        expect(await s.get('nope')).toBeNull();
      } finally {
        await s.close();
      }
    });

    it('inserts with a null expectedRev and reads back', async () => {
      const s = await makeStore();
      try {
        expect(await s.set('a', makeRecord(), 60_000, null)).toBe(true);
        expect((await s.get('a'))?.accessToken).toBe('AT');
      } finally {
        await s.close();
      }
    });

    it('refuses a second insert when one already exists', async () => {
      const s = await makeStore();
      try {
        await s.set('a', makeRecord(), 60_000, null);
        expect(await s.set('a', makeRecord({ accessToken: 'OTHER' }), 60_000, null)).toBe(false);
        expect((await s.get('a'))?.accessToken).toBe('AT');
      } finally {
        await s.close();
      }
    });

    it('compare-and-set succeeds on a matching rev and fails on a stale one', async () => {
      const s = await makeStore();
      try {
        await s.set('a', makeRecord({ rev: 0 }), 60_000, null);
        expect(await s.set('a', makeRecord({ rev: 1, accessToken: 'AT2' }), 60_000, 0)).toBe(true);
        // This is the fencing case: a slow writer still holding rev 0 must be rejected.
        expect(await s.set('a', makeRecord({ rev: 1, accessToken: 'STALE' }), 60_000, 0)).toBe(false);
        expect((await s.get('a'))?.accessToken).toBe('AT2');
      } finally {
        await s.close();
      }
    });

    it('deletes', async () => {
      const s = await makeStore();
      try {
        await s.set('a', makeRecord(), 60_000, null);
        await s.delete('a');
        expect(await s.get('a')).toBeNull();
      } finally {
        await s.close();
      }
    });

    it('expires a record after its ttl', async () => {
      const s = await makeStore();
      try {
        await s.set('a', makeRecord(), 30, null);
        await sleep(80);
        expect(await s.get('a')).toBeNull();
      } finally {
        await s.close();
      }
    });

    it('grants a lock to exactly one of two contenders', async () => {
      const s = await makeStore();
      try {
        expect(await s.tryAcquireLock('a', 'tok1', 5_000)).toBe(true);
        expect(await s.tryAcquireLock('a', 'tok2', 5_000)).toBe(false);
      } finally {
        await s.close();
      }
    });

    it('releases a lock only to its owner', async () => {
      const s = await makeStore();
      try {
        await s.tryAcquireLock('a', 'tok1', 5_000);
        await s.releaseLock('a', 'wrong-token');
        expect(await s.tryAcquireLock('a', 'tok2', 5_000)).toBe(false);
        await s.releaseLock('a', 'tok1');
        expect(await s.tryAcquireLock('a', 'tok2', 5_000)).toBe(true);
      } finally {
        await s.close();
      }
    });

    it('frees a lock when its ttl expires', async () => {
      const s = await makeStore();
      try {
        await s.tryAcquireLock('a', 'tok1', 30);
        await sleep(80);
        expect(await s.tryAcquireLock('a', 'tok2', 5_000)).toBe(true);
      } finally {
        await s.close();
      }
    });

    it('takeTransaction is single-use', async () => {
      const s = await makeStore();
      try {
        await s.putTransaction('t1', { codeVerifier: 'v', nonce: 'n', returnTo: '/', secretHash: 'h', createdAt: Date.now() }, 60_000);
        expect((await s.takeTransaction('t1'))?.nonce).toBe('n');
        expect(await s.takeTransaction('t1')).toBeNull();
      } finally {
        await s.close();
      }
    });

    it('takeTransaction is atomic under concurrency — exactly one caller wins', async () => {
      const s = await makeStore();
      try {
        await s.putTransaction('t1', { codeVerifier: 'v', nonce: 'n', returnTo: '/', secretHash: 'h', createdAt: Date.now() }, 60_000);
        const results = await Promise.all(Array.from({ length: 20 }, () => s.takeTransaction('t1')));
        expect(results.filter((r) => r !== null)).toHaveLength(1);
      } finally {
        await s.close();
      }
    });
  });
}
