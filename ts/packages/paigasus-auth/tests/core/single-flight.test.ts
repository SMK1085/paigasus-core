// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from 'vitest';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { noopLogger } from '../../src/adapters/noop-logger.js';
import { RefreshFailed, RefreshRejected, SessionStoreTimeout } from '../../src/core/errors.js';
import { failingStore, type StoreMethod } from '../support/store-failure.js';
import { shouldRefresh } from '../../src/core/refresh-policy.js';
import { resolveSession, type ResolveDeps } from '../../src/core/single-flight.js';
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
function deps(store: SessionStore, refresh: ResolveDeps['refresh']) {
  // `revoke` is called only for a refresh token that no record holds (SMA-681). A test that checks
  // it passes its own recording function over this one.
  return { store, refresh, revoke: () => Promise.resolve(), logger: noopLogger, skewMs: 30_000, lockTtlMs: 5_000, lockWaitMs: 3_000, ttlMs: 60_000 };
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
  // retry at single-flight.ts:201-202 could be deleted entirely and that test would still see
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

  // SMA-657. The SAME case as the row above, with the error built by a SECOND copy of core/errors
  // — the shape Next 16 produces, because `refresh` delegates to the shared `runtime.oidc` while
  // `resolveSession` runs in whichever copy serves the request. `resolveSession` was imported
  // statically at the top of this file, so it keeps its FIRST-copy binding: this is the real
  // class-identity split; the refresh dependency is injected here rather than taken from the
  // runtime.
  //
  // The fixture must be inside the skew window AND still live, or single-flight.ts returns early
  // and never attempts a refresh at all (shouldRefresh is `now >= expiresAt - skewMs`). The live
  // token is the point: it is the case where the defect changes the RETURN path, not just the log.
  it('classifies a definitive rejection from a SECOND module copy', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    // Precondition: without this, the test passes vacuously if the import returns the same module.
    expect(foreign.RefreshRejected).not.toBe(RefreshRejected);
    const foreignRejected = () => Promise.reject(new foreign.RefreshRejected('invalid_grant'));

    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 30_000 }), 60_000, null);
    const { logger, events } = recordingLogger();

    // NOT `toBeInstanceOf(RefreshRejected)`: that is FALSE for a foreign-copy error, which is the
    // entire defect. Asserting it would red this test both before AND after the fix.
    await expect(resolveSession({ ...deps(store, foreignRejected), logger, skewMs: 60_000 }, 's')).rejects.toBeInstanceOf(foreign.RefreshRejected);
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'rejected', degraded: false }]);
    expect(events).toContainEqual(['session.deleted', { sid: sidTag('s'), reason: 'refresh_rejected' }]);
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

  // § 2.3's cap re-check. The window is NARROW: a record whose absolute cap has already passed
  // never reaches the refresh at all (single-flight.ts:103 and :120 both delete it first), so the
  // only way to reach this branch with a passed cap is for the cap to expire DURING the refresh
  // call — which runtime.ts:118-120 bounds at 2x the OIDC HTTP timeout.
  //
  // The cap is ANCHORED TO THE POST-LOCK READ, not to setup time (review I2). A fixture-time
  // `absoluteExpiresAt: Date.now() + 40` would put the whole of setup, the outer read and the lock
  // acquisition inside the 40ms budget — and a loaded CI runner stalling there makes :120 delete
  // the record and return null, so `rejects` fails with "resolved null". Overriding the SECOND
  // `store.get` (the same idiom as the post-lock re-read test above) means only the gap between
  // :116 and :120 counts, which is one continuation with no `await` in it. The 150ms setTimeout
  // then supplies the entire margin, and a timer can fire late but never early.
  //
  // Without Math.min, the degrade returns a session that is past its own absolute cap.
  it('does not degrade past the absolute cap when the cap expires during the refresh', async () => {
    const store = new MemorySessionStore();
    // The stored record's own cap is a day out, so the OUTER check at :103 can never fire.
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 30_000 }), 60_000, null);
    const slowTransient = () => new Promise<never>((_, reject) => setTimeout(() => reject(new Error('oidc refresh_token_grant failed: TypeError')), 150));

    const originalGet = store.get.bind(store);
    let getCalls = 0;
    store.get = async (sid: string) => {
      getCalls += 1;
      const rec = await originalGet(sid);
      // Call 2 is the post-lock re-read. Give it a cap 40ms out, measured from THIS moment — the
      // refresh below takes 150ms, so the cap is reliably past by the time the catch classifies.
      return getCalls === 2 && rec !== null ? { ...rec, absoluteExpiresAt: Date.now() + 40 } : rec;
    };

    await expect(resolveSession({ ...deps(store, slowTransient), skewMs: 60_000 }, 's')).rejects.toThrow(/refresh_token_grant/);
    expect(getCalls).toBeGreaterThanOrEqual(2); // the post-lock re-read really was the one capped
  });

  // The timeout branch at single-flight.ts:236 has the identical hole and gets the identical fix.
  // Here the time passes in the lock WAIT rather than in the refresh: a 750ms lockWaitMs against a
  // 250ms cap means the deadline is reached well after the cap expired.
  //
  // THE EVENT ASSERTION IS WHAT MAKES THIS TEST HONEST (review I1), not the margin. `out` being
  // null is ALSO what the outer cap check at :103-106 returns when the cap passes during setup —
  // so on `expect(out).toBeNull()` alone, a slow runner would not flake red, it would pass GREEN
  // while never reaching the branch under test, and it would keep passing with Math.min deleted
  // from :236. `session.refresh_timeout` fires at :228, INSIDE this branch and nowhere else, so
  // asserting it makes the vacuous pass impossible at any load. The wider margins below are the
  // second measure, not the control.
  it('the lock-timeout branch does not degrade past the absolute cap', async () => {
    const store = new MemorySessionStore();
    const now = Date.now();
    await store.set('s', makeRecord({ accessExpiresAt: now + 30_000, absoluteExpiresAt: now + 250 }), 60_000, null);
    store.tryAcquireLock = () => Promise.resolve(false); // never win the lock
    const { logger, events } = recordingLogger();

    const out = await resolveSession({ ...deps(store, () => Promise.reject(new Error('unused'))), logger, skewMs: 60_000, lockWaitMs: 750 }, 's');

    // Proof the lock-timeout branch was reached at all. Without this the assertion below is
    // satisfied by the outer cap delete, which never touches the code this test exists to guard.
    expect(events).toContainEqual(['session.refresh_timeout', { sid: sidTag('s') }]);
    expect(out).toBeNull();
  });

  // SMA-657 § 7. The `if (rejected)` delete became reachable from BOTH module copies with this
  // fix, and that delete goes through the SMA-651 deadline decorator. A store that fails there
  // REPLACES the RefreshRejected as the thrown value, so `session.deleted` never fires and the
  // record survives for its TTL. That outcome is accepted, not fixed: getSession returns null on
  // any throw, so the user is still signed out, and the next request retries the delete. This row
  // exists so the accepted outcome is pinned rather than discovered later as a surprise.
  it('a failing delete on the rejection path replaces the error and leaves the record', async () => {
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() + 30_000 }), 60_000, null);
    const calls: string[] = [];
    const store = failingStore(inner, new Set<StoreMethod>(['delete']), () => new SessionStoreTimeout('delete', 4000, 'deadline'), calls);
    const { logger, events } = recordingLogger();

    await expect(resolveSession({ ...deps(store, rejected), logger, skewMs: 60_000 }, 's')).rejects.toBeInstanceOf(SessionStoreTimeout);
    // The classification still happened and was logged...
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'rejected', degraded: false }]);
    // ...the delete WAS attempted...
    expect(calls).toContain('delete:s');
    // ...but it did not land, so there is no session.deleted and the record survives.
    expect(events.some(([name]) => name === 'session.deleted')).toBe(false);
    expect(await inner.get('s')).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------------------------
// SMA-681 § 4.3 (D5, D6). A refresh response MAY carry a new ID token. OIDC Core § 12.2 requires
// its `iss` and `sub` to equal the login token's. Nothing upstream compares them (spec § 4.2), so
// resolveSession does. A match is stored, for logout's id_token_hint. A mismatch is a definitive
// refresh failure: nothing is written, the record is deleted, and the new and old refresh tokens
// are revoked after the lock is released.
// ---------------------------------------------------------------------------------------------
describe('a refreshed ID token (SMA-681 D5, D6)', () => {
  // makeRecord()'s own login claims (tests/store-contract.ts).
  const LOGIN_CLAIMS = { iss: 'https://idp', sub: 'u1' };

  function recordingRevoke(): { revoke: (token: string) => Promise<void>; revoked: string[] } {
    const revoked: string[] = [];
    const revoke = (token: string): Promise<void> => {
      revoked.push(token);
      return Promise.resolve();
    };
    return { revoke, revoked };
  }

  it('stores a refreshed ID token whose iss and sub match, and leaves idTokenClaims unchanged', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();
    // An extra claim on the refreshed token: if the code copied these claims into the record, the
    // `toEqual` below would see `email`.
    const refresh = () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300, rotatedIdToken: { token: 'IDT2', claims: { ...LOGIN_CLAIMS, email: 'new@example.com' } } });

    const out = await resolveSession({ ...deps(store, refresh), logger }, 's');

    expect(out?.idToken).toBe('IDT2');
    const stored = await store.get('s');
    expect(stored?.idToken).toBe('IDT2');
    expect(stored?.idTokenClaims).toEqual(LOGIN_CLAIMS);
    expect(events).toContainEqual(['session.refreshed', { sid: sidTag('s'), rev: 1, idTokenRotated: true }]);
    // Review Focus 1: no event carries the token.
    expect(JSON.stringify(events)).not.toContain('IDT2');
  });

  it('keeps the stored ID token when the refresh response carries none', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();

    const out = await resolveSession({ ...deps(store, () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 })), logger }, 's');

    expect(out?.idToken).toBe('IDT');
    expect((await store.get('s'))?.idToken).toBe('IDT');
    expect(events).toContainEqual(['session.refreshed', { sid: sidTag('s'), rev: 1, idTokenRotated: false }]);
  });

  it.each([
    ['sub', { iss: 'https://idp', sub: 'someone-else' }],
    ['iss', { iss: 'https://other-idp', sub: 'u1' }],
  ])('a refreshed ID token with a different %s signs out: delete, revoke the new and old refresh tokens, return null', async (_field, claims) => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();
    const { revoke, revoked } = recordingRevoke();
    const refresh = () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300, rotatedIdToken: { token: 'IDT-OTHER', claims } });

    const out = await resolveSession({ ...deps(store, refresh), revoke, logger }, 's');

    expect(out).toBeNull();
    expect(await store.get('s')).toBeNull();
    expect(revoked).toEqual(['RT2', 'RT']);
    expect(events).toContainEqual(['session.refresh.id_token_mismatch', { sid: sidTag('s') }]);
    expect(events).toContainEqual(['session.deleted', { sid: sidTag('s'), reason: 'id_token_mismatch' }]);
    expect(events.some(([name]) => name === 'session.refreshed')).toBe(false);
    // Review Focus 1: neither the new ID token nor the new refresh token reaches a log line.
    expect(JSON.stringify(events)).not.toContain('IDT-OTHER');
    expect(JSON.stringify(events)).not.toContain('RT2');
  });

  // A non-rotating IdP returns no refresh token. The old one is then still valid at the IdP, and
  // the delete removes the only copy that anything would ever revoke.
  it('a mismatch whose response has no refresh token revokes the old one', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { revoke, revoked } = recordingRevoke();
    const refresh = () => Promise.resolve({ accessToken: 'AT2', expiresIn: 300, rotatedIdToken: { token: 'IDT-OTHER', claims: { iss: 'https://idp', sub: 'someone-else' } } });

    expect(await resolveSession({ ...deps(store, refresh), revoke }, 's')).toBeNull();
    expect(revoked).toEqual(['RT']);
  });

  it('a mismatch whose response repeats the old refresh token revokes it once', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { revoke, revoked } = recordingRevoke();
    const refresh = () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT', expiresIn: 300, rotatedIdToken: { token: 'IDT-OTHER', claims: { iss: 'https://idp', sub: 'someone-else' } } });

    expect(await resolveSession({ ...deps(store, refresh), revoke }, 's')).toBeNull();
    expect(revoked).toEqual(['RT']);
  });

  // runtime.ts invariant 3 budgets only the refresh's two HTTP calls inside the lock TTL. A revoke
  // is a third IdP call, so it must run after the lock is released.
  it('the mismatch revokes run after the lock is released', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const order: string[] = [];
    const release = store.releaseLock.bind(store);
    store.releaseLock = (sid, token) => {
      order.push('release');
      return release(sid, token);
    };
    const revoke = (token: string): Promise<void> => {
      order.push(`revoke:${token}`);
      return Promise.resolve();
    };
    const refresh = () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300, rotatedIdToken: { token: 'IDT-OTHER', claims: { iss: 'https://idp', sub: 'someone-else' } } });

    expect(await resolveSession({ ...deps(store, refresh), revoke }, 's')).toBeNull();
    expect(order).toEqual(['release', 'revoke:RT2', 'revoke:RT']);
  });

  it('a revoke that throws synchronously does not change the mismatch outcome', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const attempted: string[] = [];
    const revoke = (token: string): Promise<void> => {
      attempted.push(token);
      throw new Error('synchronous revoke failure');
    };
    const refresh = () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300, rotatedIdToken: { token: 'IDT-OTHER', claims: { iss: 'https://idp', sub: 'someone-else' } } });

    expect(await resolveSession({ ...deps(store, refresh), revoke }, 's')).toBeNull();
    expect(await store.get('s')).toBeNull();
    // The first throw does not stop the second revoke.
    expect(attempted).toEqual(['RT2', 'RT']);
    // The lock is free: a throw inside the revokes did not skip the release.
    expect(await store.tryAcquireLock('s', 'next', 5_000)).toBe(true);
  });

  it('a failing revoke on the mismatch path still returns null', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const refresh = () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300, rotatedIdToken: { token: 'IDT-OTHER', claims: { iss: 'https://idp', sub: 'someone-else' } } });

    const out = await resolveSession({ ...deps(store, refresh), revoke: () => Promise.reject(new Error('idp unreachable')) }, 's');

    expect(out).toBeNull();
    expect(await store.get('s')).toBeNull();
  });

  // Review Focus 3. The delete goes through the SMA-651 deadline decorator, so it can fail. The
  // store error then propagates, as on the refresh_rejected path. The new and old refresh tokens
  // are still revoked, because nothing else would ever revoke them.
  it('a failing delete on the mismatch path propagates the store error and still revokes', async () => {
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const calls: string[] = [];
    const store = failingStore(inner, new Set<StoreMethod>(['delete']), () => new SessionStoreTimeout('delete', 4000, 'deadline'), calls);
    const { logger, events } = recordingLogger();
    const { revoke, revoked } = recordingRevoke();
    const refresh = () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300, rotatedIdToken: { token: 'IDT-OTHER', claims: { iss: 'https://idp', sub: 'someone-else' } } });

    await expect(resolveSession({ ...deps(store, refresh), revoke, logger }, 's')).rejects.toBeInstanceOf(SessionStoreTimeout);
    expect(calls).toContain('delete:s');
    expect(revoked).toEqual(['RT2', 'RT']);
    expect(events).toContainEqual(['session.refresh.id_token_mismatch', { sid: sidTag('s') }]);
    expect(events.some(([name]) => name === 'session.deleted')).toBe(false);
  });

  // Review Focus 4: the single-flight guarantee holds when the refresh rotates the ID token.
  it('two concurrent callers during a rotating refresh: one refresh, both see the new ID token', async () => {
    const store = new MemorySessionStore();
    let calls = 0;
    const refresh = async () => {
      calls += 1;
      await new Promise((r) => setTimeout(r, 40)); // hold the lock long enough to force contention
      return { accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300, rotatedIdToken: { token: 'IDT2', claims: LOGIN_CLAIMS } };
    };
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);

    const results = await Promise.all([resolveSession(deps(store, refresh), 's'), resolveSession(deps(store, refresh), 's')]);

    expect(calls).toBe(1);
    expect(results.map((r) => r?.idToken)).toEqual(['IDT2', 'IDT2']);
  });
});

// ---------------------------------------------------------------------------------------------
// SMA-681 review F3. Two branches return null after a successful refresh: a concurrent logout
// deleted the record, or the write failed twice. The IdP has then issued a refresh token that no
// record holds. It is revoked, best effort, after the lock is released.
// ---------------------------------------------------------------------------------------------
describe('a refresh token that no record holds (SMA-681)', () => {
  /** `lockFree` records, per revoke, whether the lock was free at that moment. */
  function recordingRevoke(store: SessionStore): { revoke: (token: string) => Promise<void>; revoked: string[]; lockFree: boolean[] } {
    const revoked: string[] = [];
    const lockFree: boolean[] = [];
    const revoke = async (token: string): Promise<void> => {
      revoked.push(token);
      const won = await store.tryAcquireLock('s', `probe-${token}`, 5_000);
      lockFree.push(won);
      if (won) await store.releaseLock('s', `probe-${token}`);
    };
    return { revoke, revoked, lockFree };
  }

  /** The F1a shape: a concurrent logout deletes the record just before the compare-and-set. */
  function logoutDuringPersist(store: MemorySessionStore): void {
    const originalSet = store.set.bind(store);
    store.set = async (sid, rec, ttl, expectedRev) => {
      if (expectedRev === 0) await store.delete('s');
      return originalSet(sid, rec, ttl, expectedRev);
    };
  }

  it('revokes the new refresh token when a concurrent logout deleted the record', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    logoutDuringPersist(store);
    const { revoke, revoked, lockFree } = recordingRevoke(store);

    const out = await resolveSession({ ...deps(store, () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 })), revoke }, 's');

    expect(out).toBeNull();
    expect(await store.get('s')).toBeNull();
    expect(revoked).toEqual(['RT2']);
    expect(lockFree).toEqual([true]);
  });

  it.each([
    ['no refresh token', {}],
    ['the same refresh token', { refreshToken: 'RT' }],
  ])('revokes nothing after a concurrent logout when the response carries %s', async (_case, extra) => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    logoutDuringPersist(store);
    const { revoke, revoked } = recordingRevoke(store);

    expect(await resolveSession({ ...deps(store, () => Promise.resolve({ accessToken: 'AT2', expiresIn: 300, ...extra })), revoke }, 's')).toBeNull();
    expect(revoked).toEqual([]);
  });

  it('revokes the new refresh token when the write fails twice (persist_failed)', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    store.set = () => Promise.resolve(false); // every persist fails
    const { logger, events } = recordingLogger();
    const { revoke, revoked, lockFree } = recordingRevoke(store);

    const out = await resolveSession({ ...deps(store, () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 })), revoke, logger }, 's');

    expect(out).toBeNull();
    expect(revoked).toEqual(['RT2']);
    expect(lockFree).toEqual([true]);
    expect(events).toContainEqual(['session.refresh.persist_failed', { sid: sidTag('s') }]);
    expect(JSON.stringify(events)).not.toContain('RT2');
  });

  it('revokes the old refresh token when a non-rotating IdP response fails to persist (persist_failed)', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null); // default refreshToken 'RT'
    store.set = () => Promise.resolve(false); // every persist fails
    const { logger, events } = recordingLogger();
    const { revoke, revoked, lockFree } = recordingRevoke(store);

    // No `refreshToken` in the response: a non-rotating IdP. The old token 'RT' stays live at the
    // IdP, but the delete below removes the only record that held it.
    const out = await resolveSession({ ...deps(store, () => Promise.resolve({ accessToken: 'AT2', expiresIn: 300 })), revoke, logger }, 's');

    expect(out).toBeNull();
    expect(revoked).toEqual(['RT']);
    expect(lockFree).toEqual([true]);
    expect(events).toContainEqual(['session.refresh.persist_failed', { sid: sidTag('s') }]);
  });

  it('revokes the old refresh token when a non-rotating IdP echoes it back and persist fails (persist_failed)', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null); // default refreshToken 'RT'
    store.set = () => Promise.resolve(false); // every persist fails
    const { logger, events } = recordingLogger();
    const { revoke, revoked, lockFree } = recordingRevoke(store);

    // The response echoes the old token 'RT' back: a non-rotating IdP. It equals `refreshToken`,
    // so `orphanNewRefreshToken()` skips it, and the delete below removes the only record that
    // held it.
    const out = await resolveSession({ ...deps(store, () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT', expiresIn: 300 })), revoke, logger }, 's');

    expect(out).toBeNull();
    expect(revoked).toEqual(['RT']);
    expect(lockFree).toEqual([true]);
    expect(events).toContainEqual(['session.refresh.persist_failed', { sid: sidTag('s') }]);
  });

  it('persist_failed with a failing delete propagates the store error and still revokes', async () => {
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const calls: string[] = [];
    const store = failingStore(inner, new Set<StoreMethod>(['delete']), () => new SessionStoreTimeout('delete', 4000, 'deadline'), calls);
    store.set = () => Promise.resolve(false);
    const { revoke, revoked } = recordingRevoke(inner);

    await expect(resolveSession({ ...deps(store, () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 })), revoke }, 's')).rejects.toBeInstanceOf(SessionStoreTimeout);
    expect(revoked).toEqual(['RT2']);
  });

  it('a revoke that throws synchronously does not change the persist_failed outcome', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    store.set = () => Promise.resolve(false);
    const revoke = (): Promise<void> => {
      throw new Error('synchronous revoke failure');
    };

    expect(await resolveSession({ ...deps(store, () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 })), revoke }, 's')).toBeNull();
  });

  it('a successful refresh revokes nothing', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { revoke, revoked } = recordingRevoke(store);

    expect((await resolveSession({ ...deps(store, () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 })), revoke }, 's'))?.refreshToken).toBe('RT2');
    expect(revoked).toEqual([]);
  });
});

// ---------------------------------------------------------------------------------------------
// SMA-692 D10. A transient refresh failure used to log only `reason: 'transient'`. After a scope
// change, an `invalid_scope` then looked the same as a network error. The log line now carries
// the OAuth code, from a closed set. It never carries the error object, its message or a URL.
// ---------------------------------------------------------------------------------------------
describe('the OAuth code in the refresh log (SMA-692 D10)', () => {
  it('logs the OAuth code of a transient failure', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();
    const failed = () => Promise.reject(new RefreshFailed('invalid_scope', 'ResponseBodyError'));

    await expect(resolveSession({ ...deps(store, failed), logger }, 's')).rejects.toBeInstanceOf(RefreshFailed);
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: false, oauthError: 'invalid_scope' }]);
  });

  it('logs the code on the degraded path too, and keeps the session', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 30_000 }), 60_000, null);
    const { logger, events } = recordingLogger();
    const failed = () => Promise.reject(new RefreshFailed('invalid_client', 'ResponseBodyError'));

    const out = await resolveSession({ ...deps(store, failed), logger, skewMs: 60_000 }, 's');

    expect(out?.refreshState).toBe('failed');
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: true, oauthError: 'invalid_client' }]);
    expect(await store.get('s')).not.toBeNull();
  });

  // SMA-657 shape: `refresh` runs in the copy that built the runtime, resolveSession in another.
  it('logs the code of a RefreshFailed from a SECOND module copy', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    expect(foreign.RefreshFailed).not.toBe(RefreshFailed);
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();
    const failed = () => Promise.reject(new foreign.RefreshFailed('invalid_scope', 'ResponseBodyError'));

    await expect(resolveSession({ ...deps(store, failed), logger }, 's')).rejects.toBeInstanceOf(foreign.RefreshFailed);
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: false, oauthError: 'invalid_scope' }]);
  });

  it('logs other, never the raw string, for an error that only claims the code', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();
    const forged = Object.assign(new Error('x'), { code: 'oidc_refresh_failed', oauthError: 'https://idp.example.com/secret' });

    await expect(resolveSession({ ...deps(store, () => Promise.reject(forged)), logger }, 's')).rejects.toBe(forged);
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: false, oauthError: 'other' }]);
  });
});
