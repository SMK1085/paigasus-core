// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from 'vitest';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { noopLogger } from '../../src/adapters/noop-logger.js';
import { RefreshRejected } from '../../src/core/errors.js';
import { shouldRefresh } from '../../src/core/refresh-policy.js';
import { resolveSession } from '../../src/core/single-flight.js';
import { makeRecord } from '../store-contract.js';
import type { AuthEventFields, AuthEventName, AuthLogger } from '../../src/ports/logger.js';
import { sidTag } from '../../src/ports/logger.js';
import type { SessionStore } from '../../src/ports/session-store.js';

/** Records every event emitted, so a test can assert on WHICH event fired, not just that one did. */
function recordingLogger(): { logger: AuthLogger; events: Array<[AuthEventName, AuthEventFields]> } {
  const events: Array<[AuthEventName, AuthEventFields]> = [];
  return { logger: { event: (name, fields) => void events.push([name, { ...fields }]) }, events };
}

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
    expect(out?.refreshState).toBe('pending');
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

  // I4 (final fix wave): before this, a thrown `refresh` (an IdP outage) propagated all the way
  // to get-session.ts's catch, which logs `store.unavailable` — the SAME event a genuine Redis
  // outage produces, so an operator investigating a healthy store chased the wrong system. This
  // asserts the two stay distinguishable: an IdP failure logs `session.refresh_failed`, not
  // `store.unavailable`, with the redaction discipline intact (only a truncated sid, never the
  // caught error object — which could embed a token endpoint URL).
  it('emits session.refresh_failed, not store.unavailable, when the refresh call throws (I4)', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();

    await expect(resolveSession({ ...deps(store, () => Promise.reject(new Error('token endpoint returned 503'))), logger }, 's')).rejects.toThrow('token endpoint returned 503');

    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: false }]);
    expect(events.some(([name]) => name === 'store.unavailable')).toBe(false);
  });

  // The contrasting case, distinguished from the one above: a genuine STORE failure (as opposed
  // to an IdP one) must never be misreported as session.refresh_failed either. `store.get` is the
  // very first call `resolveSession` makes, so it throws before `refresh` is ever reached — this
  // documents that `session.refresh_failed` is scoped to the refresh call specifically, not to
  // "anything failed during a refresh-eligible resolve".
  it('does not emit session.refresh_failed when the failure is the STORE, not the IdP', async () => {
    const store = new MemorySessionStore();
    store.get = () => Promise.reject(new Error('redis unavailable'));
    const { logger, events } = recordingLogger();

    await expect(resolveSession({ ...deps(store, () => Promise.resolve({ accessToken: 'AT', expiresIn: 300 })), logger }, 's')).rejects.toThrow('redis unavailable');

    expect(events.some(([name]) => name === 'session.refresh_failed')).toBe(false);
  });

  // F1a — reviewer-identified: the ORIGINAL persist-failure branch treated "record now absent"
  // (winner === null) as if it might mean "the store is broken", and re-inserted with
  // expectedRev: null on that path. A concurrent logout landing between the refresh call and the
  // persist deletes the record for real; a re-insert there resurrects a session the user just
  // signed out of. The corrected rule: winner === null means the record was deleted — return
  // null, never re-insert.
  it('does not resurrect a session deleted during a concurrent refresh (F1a)', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ rev: 5, accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const originalSet = store.set.bind(store);
    // Simulate a concurrent POST /auth/logout landing between the refresh call and the persist:
    // by the time this CAS runs, the record has already been deleted.
    store.set = async (sid, rec, ttl, expectedRev) => {
      if (expectedRev === 5) await store.delete('s');
      return originalSet(sid, rec, ttl, expectedRev);
    };

    const out = await resolveSession(
      deps(store, () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 })),
      's',
    );

    expect(out).toBeNull();
    expect(await store.get('s')).toBeNull(); // must NOT be resurrected
  });

  // F1b — reviewer-identified: comparing only `winner.rev > fresh.rev` misses the case where a
  // logout-then-re-login resets rev BACKWARD (to 0). The old CAS on a higher rev then "wins" the
  // retry and overwrites the new login's record with the old session's stale claims/principal.
  // The corrected rule: ANY mismatch (winner.rev !== fresh.rev), not just an increase, means
  // another writer owns the record — return their record rather than overwriting it.
  it('does not clobber a fresh re-login when the old CAS loses the race (F1b)', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ rev: 5, accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const originalSet = store.set.bind(store);
    const newLogin = makeRecord({ rev: 0, accessToken: 'NEW-LOGIN-AT', accessExpiresAt: Date.now() + 600_000 });
    // Simulate a logout-then-re-login landing between the refresh call and the persist: a
    // brand-new session (rev reset to 0) is written before this caller's CAS on rev 5 runs.
    store.set = async (sid, rec, ttl, expectedRev) => {
      if (expectedRev === 5) {
        await store.delete('s');
        await originalSet('s', newLogin, ttl, null);
      }
      return originalSet(sid, rec, ttl, expectedRev);
    };

    const out = await resolveSession(
      deps(store, () => Promise.resolve({ accessToken: 'STALE-AT', refreshToken: 'STALE-RT', expiresIn: 300 })),
      's',
    );

    expect(out?.accessToken).toBe('NEW-LOGIN-AT');
    expect((await store.get('s'))?.accessToken).toBe('NEW-LOGIN-AT'); // must NOT be overwritten
  });

  // F2 — reviewer-identified: the ORIGINAL `finally` block called `store.releaseLock` unguarded.
  // Against Redis every command error THROWS, so a transient error at release time turned an
  // already-persisted, successful refresh into a thrown error the caller sees as a failure — even
  // though the refresh itself fully succeeded and the lock frees by TTL regardless.
  it('does not mask a successful refresh when releaseLock throws (F2)', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    // MemorySessionStore.releaseLock never throws on its own — this is exactly the case the
    // reviewer noted the existing suite could not see.
    store.releaseLock = () => {
      throw new Error('redis unavailable');
    };

    const out = await resolveSession(
      deps(store, () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 })),
      's',
    );

    expect(out?.accessToken).toBe('AT2');
    expect((await store.get('s'))?.accessToken).toBe('AT2'); // the refresh really was persisted
  });

  // F2 (second half) — the worse case: when `refresh` ITSELF throws (the exact case invariant 4
  // exists for), an unguarded `finally` that also throws REPLACES the IdP's own error with the
  // store's, and the operator loses the real cause.
  it('propagates the IdP error, not a releaseLock failure, when both fail (F2)', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    store.releaseLock = () => {
      throw new Error('redis unavailable');
    };

    await expect(
      resolveSession(
        deps(store, () => {
          throw new Error('idp down');
        }),
        's',
      ),
    ).rejects.toThrow('idp down');
  });

  // F5 — reviewer-identified: nothing in the original suite would fail if the post-lock
  // double-check (invariant 1) were deleted. Force the disagreement deterministically: the
  // PRE-lock read (line ~66) sees a stale, expired copy; every read from then on — in particular
  // the POST-lock double-check — sees the record another holder already refreshed. `refresh` must
  // never be called.
  it('the post-lock double-check stops a second holder from refreshing an already-refreshed record (F5, invariant 1)', async () => {
    const store = new MemorySessionStore();
    const refresh = vi.fn();
    const stale = makeRecord({ rev: 0, accessExpiresAt: Date.now() - 1 });
    const live = makeRecord({ rev: 1, accessToken: 'ALREADY-FRESH-AT', accessExpiresAt: Date.now() + 300_000 });
    await store.set('s', live, 60_000, null);

    const originalGet = store.get.bind(store);
    let getCalls = 0;
    store.get = (sid: string) => {
      getCalls += 1;
      // The FIRST read (the pre-lock read that decides whether to even attempt a refresh) sees
      // the stale, expired copy. Every later read (in particular the post-lock double-check)
      // returns whatever is really stored — the already-refreshed record.
      return getCalls === 1 ? Promise.resolve(stale) : originalGet(sid);
    };

    const out = await resolveSession(deps(store, refresh), 's');

    expect(refresh).not.toHaveBeenCalled();
    expect(out?.accessToken).toBe('ALREADY-FRESH-AT');
  });

  // F7 — reviewer-identified: an IdP returning a zero, negative, or merely tiny `expiresIn` would
  // otherwise write an accessExpiresAt already inside (or past) the skew window, so the very next
  // caller decides to refresh again — a refresh loop against the IdP. `single-flight.ts` floors
  // the refreshed lifetime at `skewMs + a buffer` so this cannot happen.
  it('floors a non-positive expiresIn so the refreshed token does not need an immediate second refresh (F7)', async () => {
    const store = new MemorySessionStore();
    const skewMs = 30_000;
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);

    await resolveSession({ ...deps(store, () => Promise.resolve({ accessToken: 'AT2', expiresIn: 0 })), skewMs }, 's');

    const rec = await store.get('s');
    expect(rec).not.toBeNull();
    // If the floor were absent this would be true again immediately, and the caller would loop.
    expect(shouldRefresh(Date.now(), rec!.accessExpiresAt, skewMs)).toBe(false);
  });

  // Reviewer-identified, "one more thing": the absolute-expiry cap was checked only BEFORE the
  // lock wait. A caller that waits lockWaitMs for the lock can win it after the cap has since
  // passed; without a re-check next to the double-check, the clamp then writes an
  // already-expired accessExpiresAt and the NEXT call deletes it instead of this one.
  it('re-checks the absolute cap after acquiring the lock, not only before waiting for it', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1, absoluteExpiresAt: Date.now() + 5_000 }), 60_000, null);
    const original = store.tryAcquireLock.bind(store);
    // Simulate the absolute cap passing while this caller was waiting to acquire the lock.
    store.tryAcquireLock = async (sid, tok, ttl) => {
      const current = await store.get('s');
      if (current !== null) await store.set('s', { ...current, absoluteExpiresAt: Date.now() - 1 }, 60_000, current.rev);
      return original(sid, tok, ttl);
    };

    const out = await resolveSession(
      deps(store, () => {
        throw new Error('must not refresh');
      }),
      's',
    );

    expect(out).toBeNull();
    expect(await store.get('s')).toBeNull();
  });

  // SMA-626 § 5 guard 4. The existing persist-failure test stubs `set` to fail FOREVER, so the
  // retry at single-flight.ts:151-153 could be deleted entirely and that test would still see
  // `null` and stay green. This one fails the CAS exactly ONCE against UNCHANGED state, which is
  // the only path that reaches the retry — delete the retry and this test sees `null` instead of
  // a refreshed record.
  //
  // The stub is installed AFTER the setup write, because every fixture here seeds the record with
  // `store.set(..., null)` and a stub counting "the first set call" would break that seed.
  //
  // MEASURED 2026-09-10: with the two retry lines deleted this test reds (`out` is null) while
  // the fail-forever test above stays green.
  it('retries the compare-and-set ONCE against unchanged state, and persists (F1, guard 4)', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ rev: 7, accessExpiresAt: Date.now() - 1 }), 60_000, null);

    const originalSet = store.set.bind(store);
    let refused = false;
    store.set = (sid, rec, ttl, expectedRev) => {
      // Refuse exactly the first fenced write, and write NOTHING — so the record stays at rev 7
      // and resolveSession's re-read finds `winner.rev === fresh.rev`, the retry branch.
      if (!refused && expectedRev === 7) {
        refused = true;
        return Promise.resolve(false);
      }
      return originalSet(sid, rec, ttl, expectedRev);
    };

    const out = await resolveSession(
      deps(store, () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 })),
      's',
    );

    expect(refused).toBe(true); // the retry branch really was reached
    expect(out?.accessToken).toBe('AT2');
    expect(out?.rev).toBe(8);
    expect((await store.get('s'))?.accessToken).toBe('AT2');
  });
});

// ---------------------------------------------------------------------------------------------
// SMA-626 § 2.3. The lock-timeout path already degrades to a still-live access token; a THROWN
// refresh did not, so a transient IdP outage signed users out with up to skewMs of token life
// left. Classification is what makes the degrade safe: only invalid_grant means the refresh token
// is actually revoked.
// ---------------------------------------------------------------------------------------------
describe('a failing refresh (SMA-626 § 2.3)', () => {
  const transient = () => Promise.reject(new Error('oidc refresh_token_grant failed: TypeError'));
  const rejected = () => Promise.reject(new RefreshRejected('invalid_grant'));

  it('degrades to the live access token when the failure is transient', async () => {
    const store = new MemorySessionStore();
    const rec = makeRecord({ accessExpiresAt: Date.now() + 30_000 }); // inside skew, still live
    await store.set('s', rec, 60_000, null);
    const { logger, events } = recordingLogger();

    const out = await resolveSession({ ...deps(store, transient), logger, skewMs: 60_000 }, 's');

    expect(out?.accessToken).toBe('AT');
    expect(out?.refreshState).toBe('failed');
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: true }]);
    expect(await store.get('s')).not.toBeNull(); // a transient failure must NOT delete
  });

  it('signs out when the failure is transient and the token is hard-expired', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();

    await expect(resolveSession({ ...deps(store, transient), logger }, 's')).rejects.toThrow(/refresh_token_grant/);
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: false }]);
  });

  // A revoked refresh token means the session is genuinely dead. Degrading would keep it alive for
  // up to skewMs, which § 2.1 calls a real exposure.
  it('never degrades a definitive rejection, even with a live access token', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 30_000 }), 60_000, null);
    const { logger, events } = recordingLogger();

    await expect(resolveSession({ ...deps(store, rejected), logger, skewMs: 60_000 }, 's')).rejects.toBeInstanceOf(RefreshRejected);
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'rejected', degraded: false }]);
    // The delete is not conditional on the token being dead: a live access token belonging to a
    // revoked session is exactly the exposure § 2.1 names, so it must go too.
    expect(await store.get('s')).toBeNull();
  });

  // Without this, a revoked refresh token and a live access token sit in Redis for the full ttlMs
  // and every later getSession() re-takes the lock and re-calls the token endpoint.
  it('DELETES the record on a definitive rejection, and says why', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();

    await expect(resolveSession({ ...deps(store, rejected), logger }, 's')).rejects.toBeInstanceOf(RefreshRejected);
    expect(await store.get('s')).toBeNull();
    expect(events).toContainEqual(['session.deleted', { sid: sidTag('s'), reason: 'refresh_rejected' }]);
  });

  // The counterpart to the delete above, and the reason it has to be conditional: a TRANSIENT
  // failure must leave no `session.deleted` behind at all. Without this, widening the delete to
  // every failure reds only the "must NOT delete" line in the first test and nothing would say
  // the EVENT vocabulary had been corrupted too.
  it('emits no session.deleted when the failure is transient', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();

    await expect(resolveSession({ ...deps(store, transient), logger }, 's')).rejects.toThrow(/refresh_token_grant/);
    expect(events.some(([name]) => name === 'session.deleted')).toBe(false);
    expect(await store.get('s')).not.toBeNull();
  });

  // `reason` is what keeps the two apart. With `degraded` alone, both of these log the identical
  // line, which is the conflation § 2 exists to remove.
  it('a rejection and a hard-expired transient failure log DIFFERENT reasons', async () => {
    const run = async (refresh: () => Promise<never>) => {
      const store = new MemorySessionStore();
      await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
      const { logger, events } = recordingLogger();
      await resolveSession({ ...deps(store, refresh), logger }, 's').catch(() => undefined);
      return events.find(([name]) => name === 'session.refresh_failed')?.[1];
    };

    expect(await run(rejected)).toMatchObject({ reason: 'rejected' });
    expect(await run(transient)).toMatchObject({ reason: 'transient' });
  });

  // Invariant 4 still holds on the NEW path. The degrade returns EARLY from inside the try block,
  // so it depends entirely on the `finally` to release the lock. If that early return were ever
  // moved outside the try, every later request for this session would wait the full lockWaitMs
  // behind a lock nobody holds — and no other test in this file exercises a return (rather than a
  // throw) out of the refresh catch.
  it('releases the lock when the refresh fails and the caller degrades', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 30_000 }), 60_000, null);

    const out = await resolveSession({ ...deps(store, transient), skewMs: 60_000 }, 's');

    expect(out?.refreshState).toBe('failed');
    expect(await store.tryAcquireLock('s', 'next', 5_000)).toBe(true); // false if the lock leaked
  });

  // The degrade must return the POST-lock re-read, not the pre-lock copy: between the two, another
  // holder may have persisted a newer record, and returning the stale outer one would hand the
  // caller an access token that has already been rotated away.
  it('degrades on the post-lock re-read, not the stale pre-lock copy', async () => {
    const store = new MemorySessionStore();
    const stale = makeRecord({ rev: 0, accessToken: 'STALE-AT', accessExpiresAt: Date.now() + 30_000 });
    const newer = makeRecord({ rev: 1, accessToken: 'NEWER-AT', accessExpiresAt: Date.now() + 30_000 });
    await store.set('s', newer, 60_000, null);

    const originalGet = store.get.bind(store);
    let getCalls = 0;
    store.get = (sid: string) => {
      getCalls += 1;
      return getCalls === 1 ? Promise.resolve(stale) : originalGet(sid);
    };

    const out = await resolveSession({ ...deps(store, transient), skewMs: 60_000 }, 's');

    expect(out?.accessToken).toBe('NEWER-AT');
    expect(out?.refreshState).toBe('failed');
  });

  // § 2.3's cap re-check. The window is NARROW and the test is deliberately slow because of it:
  // a record whose absolute cap has already passed never reaches the refresh at all (:90 and :107
  // both delete it first), so the only way to reach this branch with a passed cap is for the cap
  // to expire DURING the refresh call — which runtime.ts:118-120 bounds at 2x the OIDC HTTP
  // timeout. The 40ms cap against a 150ms refresh gives a 110ms margin.
  //
  // Without Math.min, the degrade returns a session that is past its own absolute cap.
  it('does not degrade past the absolute cap when the cap expires during the refresh', async () => {
    const store = new MemorySessionStore();
    const now = Date.now();
    await store.set('s', makeRecord({ accessExpiresAt: now + 30_000, absoluteExpiresAt: now + 40 }), 60_000, null);
    const slowTransient = () => new Promise<never>((_, reject) => setTimeout(() => reject(new Error('oidc refresh_token_grant failed: TypeError')), 150));

    await expect(resolveSession({ ...deps(store, slowTransient), skewMs: 60_000 }, 's')).rejects.toThrow(/refresh_token_grant/);
  });

  // The timeout branch at single-flight.ts:180-185 has the identical hole and gets the identical
  // fix. Here the time passes in the lock WAIT rather than in the refresh: lockWaitMs of 150
  // against a 40ms cap means the deadline is reached well after the cap expired.
  it('the lock-timeout branch does not degrade past the absolute cap', async () => {
    const store = new MemorySessionStore();
    const now = Date.now();
    await store.set('s', makeRecord({ accessExpiresAt: now + 30_000, absoluteExpiresAt: now + 40 }), 60_000, null);
    store.tryAcquireLock = () => Promise.resolve(false); // never win the lock

    const out = await resolveSession({ ...deps(store, () => Promise.reject(new Error('unused'))), skewMs: 60_000, lockWaitMs: 150 }, 's');

    expect(out).toBeNull();
  });
});
