// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from 'vitest';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { noopLogger } from '../../src/adapters/noop-logger.js';
import { resolveSession } from '../../src/core/single-flight.js';
import { makeRecord } from '../store-contract.js';
import type { SessionStore } from '../../src/ports/session-store.js';

// REAL TIMERS THROUGHOUT. The waiter backs off with setTimeout; under vi.useFakeTimers() that
// never fires unless the test advances timers by hand, so the failure mode is a HANG rather than
// an assertion failure. Expiry is produced by writing a past accessExpiresAt, never by moving a
// clock — which is why this package needs no Clock port.
function deps(store: SessionStore, refresh: (rt: string) => Promise<{ accessToken: string; refreshToken?: string; expiresIn: number }>) {
  return { store, refresh, logger: noopLogger, skewMs: 30_000, lockTtlMs: 5_000, lockWaitMs: 3_000, ttlMs: 60_000 };
}

describe('resolveSession', () => {
  it('returns the record untouched when the token is fresh', async () => {
    const store = new MemorySessionStore();
    const refresh = vi.fn();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 600_000 }), 60_000, null);
    expect((await resolveSession(deps(store, refresh), 's'))?.accessToken).toBe('AT');
    expect(refresh).not.toHaveBeenCalled();
  });

  it('TWO CONCURRENT CALLERS ON AN EXPIRED TOKEN TRIGGER EXACTLY ONE REFRESH', async () => {
    const store = new MemorySessionStore();
    let calls = 0;
    const refresh = async () => {
      calls += 1;
      await new Promise((r) => setTimeout(r, 40)); // hold the lock long enough to force contention
      return { accessToken: `AT${String(calls)}`, refreshToken: `RT${String(calls)}`, expiresIn: 300 };
    };
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);

    const results = await Promise.all([resolveSession(deps(store, refresh), 's'), resolveSession(deps(store, refresh), 's')]);

    expect(calls).toBe(1);
    expect(results.map((r) => r?.accessToken)).toEqual(['AT1', 'AT1']);
  });

  it('TWENTY concurrent callers still trigger exactly one refresh', async () => {
    const store = new MemorySessionStore();
    let calls = 0;
    const refresh = async () => {
      calls += 1;
      await new Promise((r) => setTimeout(r, 40));
      return { accessToken: `AT${String(calls)}`, refreshToken: 'RT2', expiresIn: 300 };
    };
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);

    const results = await Promise.all(Array.from({ length: 20 }, () => resolveSession(deps(store, refresh), 's')));

    expect(calls).toBe(1);
    expect(new Set(results.map((r) => r?.accessToken))).toEqual(new Set(['AT1']));
  });

  it('the rotated refresh token replaces the old one', async () => {
    const store = new MemorySessionStore();
    const refresh = (rt: string) => {
      expect(rt).toBe('RT');
      return Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 });
    };
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    await resolveSession(deps(store, refresh), 's');
    expect((await store.get('s'))?.refreshToken).toBe('RT2');
  });

  it('increments rev on every refresh', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ rev: 7, accessExpiresAt: Date.now() - 1 }), 60_000, null);
    await resolveSession(
      deps(store, () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 })),
      's',
    );
    expect((await store.get('s'))?.rev).toBe(8);
  });

  it('returns null and deletes when the absolute cap has passed', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ absoluteExpiresAt: Date.now() - 1 }), 60_000, null);
    expect(
      await resolveSession(
        deps(store, () => {
          throw new Error('must not refresh');
        }),
        's',
      ),
    ).toBeNull();
    expect(await store.get('s')).toBeNull();
  });

  it('clamps a refreshed accessExpiresAt to the absolute cap', async () => {
    const store = new MemorySessionStore();
    const cap = Date.now() + 10_000;
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1, absoluteExpiresAt: cap }), 60_000, null);
    const out = await resolveSession(
      deps(store, () => Promise.resolve({ accessToken: 'AT2', expiresIn: 3600 })),
      's',
    );
    expect(out?.accessExpiresAt).toBeLessThanOrEqual(cap);
  });

  it('returns null when the record has no refresh token', async () => {
    const store = new MemorySessionStore();
    // makeRecord's parameter is Partial<SessionRecord>, so exactOptionalPropertyTypes rejects an
    // explicit `refreshToken: undefined` in the literal — build a record and remove the optional
    // field instead, which is exactly what "no refresh token" means.
    const rec = makeRecord({ accessExpiresAt: Date.now() - 1 });
    delete rec.refreshToken;
    await store.set('s', rec, 60_000, null);
    expect(
      await resolveSession(
        deps(store, () => {
          throw new Error('must not refresh');
        }),
        's',
      ),
    ).toBeNull();
  });

  it('returns null when the record is deleted between the read and the lock', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const original = store.tryAcquireLock.bind(store);
    // Simulate a concurrent POST /auth/logout landing in the window.
    store.tryAcquireLock = async (sid, tok, ttl) => {
      await store.delete('s');
      return original(sid, tok, ttl);
    };
    expect(
      await resolveSession(
        deps(store, () => {
          throw new Error('must not refresh');
        }),
        's',
      ),
    ).toBeNull();
  });

  it('returns the stale-but-live record when the lock cannot be won in time', async () => {
    const store = new MemorySessionStore();
    // Inside the skew window but NOT yet expired: still usable.
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 5_000 }), 60_000, null);
    await store.tryAcquireLock('s', 'someone-else', 60_000);
    const d = {
      ...deps(store, () => {
        throw new Error('must not refresh');
      }),
      lockWaitMs: 120,
    };
    const out = await resolveSession(d, 's');
    expect(out?.accessToken).toBe('AT');
    expect(out?.refreshPending).toBe(true);
  });

  it('returns null when the lock cannot be won and the token is genuinely expired', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    await store.tryAcquireLock('s', 'someone-else', 60_000);
    const d = {
      ...deps(store, () => {
        throw new Error('must not refresh');
      }),
      lockWaitMs: 120,
    };
    expect(await resolveSession(d, 's')).toBeNull();
  });

  it('deletes the record when the refreshed value cannot be persisted twice', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    store.set = () => Promise.resolve(false); // every persist fails
    expect(
      await resolveSession(
        deps(store, () => Promise.resolve({ accessToken: 'AT2', expiresIn: 300 })),
        's',
      ),
    ).toBeNull();
  });

  it('releases the lock when the refresh call throws', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    await expect(
      resolveSession(
        deps(store, () => {
          throw new Error('idp down');
        }),
        's',
      ),
    ).rejects.toThrow('idp down');
    // If the lock leaked, this would be false.
    expect(await store.tryAcquireLock('s', 'next', 5_000)).toBe(true);
  });
});
