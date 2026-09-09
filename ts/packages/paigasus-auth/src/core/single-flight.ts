// SPDX-License-Identifier: Apache-2.0
import { newLockToken } from './ids.js';
import { shouldRefresh } from './refresh-policy.js';
import type { SessionRecord } from './session.js';
import type { AuthLogger } from '../ports/logger.js';
import { sidTag } from '../ports/logger.js';
import type { SessionStore } from '../ports/session-store.js';

export interface RefreshedTokens {
  accessToken: string;
  refreshToken?: string;
  expiresIn: number; // seconds
}

export interface ResolveDeps {
  store: SessionStore;
  refresh: (refreshToken: string) => Promise<RefreshedTokens>;
  logger: AuthLogger;
  skewMs: number;
  lockTtlMs: number;
  lockWaitMs: number;
  ttlMs: number;
}

export type ResolvedSession = SessionRecord & { refreshPending?: boolean };

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Exponential with jitter. A fixed backoff synchronises every waiter onto the same wake-up. */
const backoff = (attempt: number): number => Math.min(25 * 2 ** attempt, 250) * (0.5 + Math.random());

// F7: a floor on the refreshed access-token lifetime. An IdP that returns a zero, negative, or
// merely tiny `expiresIn` would otherwise write an accessExpiresAt that is already inside (or
// past) the skew window, so the very next caller decides to refresh again — a refresh loop
// against the IdP that never resolves. Flooring at `skewMs + this buffer` guarantees the freshly
// written record does NOT need another refresh immediately (unless absoluteExpiresAt clamps it
// lower, which is the intended, one-time-only exception).
const MIN_ACCESS_TTL_BUFFER_MS = 1_000;

/**
 * Resolve a session, refreshing under a per-session lock if the access token is at or past its
 * skew window. AT MOST ONE refresh happens per session across concurrent callers.
 *
 * FIVE INVARIANTS. Each is a defect if dropped:
 *
 *  1. The DOUBLE-CHECK after acquiring the lock. Without it two SEQUENTIAL holders both refresh,
 *     and the second presents a refresh token the IdP rotated and revoked a millisecond earlier.
 *     A lock alone does not prevent this — it is the least obvious point in the design. The
 *     absolute-expiry cap is re-checked in the same place, for the same reason: a caller that
 *     waited `lockWaitMs` for the lock can win it after the cap has since passed.
 *  2. The waiter NEVER refreshes on timeout. Falling back to "refresh anyway" reinstates the race
 *     under exactly the load where it matters.
 *  3. A unique lock token with a compare-and-delete release, so a holder whose TTL expired cannot
 *     delete the next holder's lock.
 *  4. Release in `finally`, so a throwing IdP call does not hold the lock for its full TTL. The
 *     release itself is wrapped in its own try/catch (F2): the lock frees by TTL regardless, so a
 *     transient store error at release time must never (a) turn an already-persisted, successful
 *     refresh into a thrown error, or (b) replace the IdP's own error with a store error when
 *     `refresh` itself threw — either would mask a real outcome behind a recoverable one.
 *  5. The write is FENCED on `rev`. A refresh that outlives its lock TTL would otherwise write
 *     its now-revoked token over a newer valid one; compare-and-delete protects the LOCK, never
 *     the WRITE. The caller additionally asserts the refresh HTTP timeout is below `lockTtlMs`
 *     for a LOCK HOLDER. For a WAITER the binding bound is `lockWaitMs`: if a refresh outlasts it
 *     while the token is hard-expired, the timeout branch below returns `null` and every waiter
 *     is signed out — both bounds matter, not just the holder's.
 *
 * `now` is re-read EVERY ITERATION. Binding it once makes the deadline unreachable and the loop
 * never terminates.
 *
 * ON A FAILED COMPARE-AND-SET (F1). Neither adapter's `set` returns `false` for a broken store —
 * both return it ONLY when the record is absent or `rev` has moved (a real store failure THROWS
 * `SessionStoreUnavailable` instead). So a `false` here always means "the state is not what this
 * call fenced on", never "the store is broken", and the three cases are:
 *
 *   - the record is now ABSENT (`winner === null`): a concurrent logout deleted it. Return `null`
 *     and NEVER re-insert — resurrecting a logged-out session is the failure this guards against.
 *   - `winner.rev !== fresh.rev`: another writer already owns the record (rev moved forward, or
 *     — after a delete-then-recreate, e.g. a re-login — moved back to 0). Their record is
 *     authoritative; return it rather than overwriting a newer session with a stale one.
 *   - `winner.rev === fresh.rev`: the CAS failed against UNCHANGED state, which a correct store
 *     should not do. Retry the write once against that same state; if it fails again, delete
 *     rather than leave a record holding a refresh token the IdP has already revoked — a clean
 *     re-login is the recoverable outcome.
 */
export async function resolveSession(deps: ResolveDeps, sid: string): Promise<ResolvedSession | null> {
  const { store, refresh, logger, skewMs, lockTtlMs, lockWaitMs, ttlMs } = deps;

  const rec = await store.get(sid);
  if (rec === null) return null;
  if (Date.now() >= rec.absoluteExpiresAt) {
    await store.delete(sid);
    logger.event('session.deleted', { sid: sidTag(sid), reason: 'absolute_expiry' });
    return null;
  }
  if (!shouldRefresh(Date.now(), rec.accessExpiresAt, skewMs)) return rec;

  const lockToken = newLockToken();
  const deadline = Date.now() + lockWaitMs;

  for (let attempt = 0; ; attempt += 1) {
    if (await store.tryAcquireLock(sid, lockToken, lockTtlMs)) {
      try {
        const fresh = await store.get(sid);
        if (fresh === null) return null; // concurrent logout
        // Re-check the absolute cap here too: this caller may have waited for the lock long
        // enough that the cap passed since the outer check.
        if (Date.now() >= fresh.absoluteExpiresAt) {
          await store.delete(sid);
          logger.event('session.deleted', { sid: sidTag(sid), reason: 'absolute_expiry' });
          return null;
        }
        if (!shouldRefresh(Date.now(), fresh.accessExpiresAt, skewMs)) return fresh; // invariant 1
        if (fresh.refreshToken === undefined) {
          await store.delete(sid);
          logger.event('session.deleted', { sid: sidTag(sid), reason: 'no_refresh_token' });
          return null;
        }
        const refreshToken = fresh.refreshToken;

        const tokens = await refresh(refreshToken);
        const accessTtlMs = Math.max(tokens.expiresIn * 1000, skewMs + MIN_ACCESS_TTL_BUFFER_MS); // F7
        const next: SessionRecord = {
          ...fresh,
          rev: fresh.rev + 1,
          accessToken: tokens.accessToken,
          refreshToken: tokens.refreshToken ?? fresh.refreshToken,
          accessExpiresAt: Math.min(Date.now() + accessTtlMs, fresh.absoluteExpiresAt),
        };

        // Invariant 5 / F1. See the function doc for the three cases a `false` here can mean.
        let written: SessionRecord = next;
        let ok = await store.set(sid, next, ttlMs, fresh.rev);
        if (!ok) {
          const winner = await store.get(sid);
          if (winner === null) return null; // logout resurrection guard — never re-insert
          if (winner.rev !== fresh.rev) return winner; // another writer already owns it
          // winner.rev === fresh.rev: unchanged state, a genuine write failure. Retry once.
          written = { ...next, rev: winner.rev + 1 };
          ok = await store.set(sid, written, ttlMs, winner.rev);
        }
        if (!ok) {
          // Retried once and still could not persist. A clean re-login beats a session that can
          // never refresh again, so delete rather than leave the revoked token in place.
          await store.delete(sid);
          logger.event('session.refresh.persist_failed', { sid: sidTag(sid) });
          return null;
        }

        logger.event('session.refreshed', { sid: sidTag(sid), rev: written.rev }); // F6
        return written;
      } finally {
        // F2: never let a release-time store error mask the try block's own outcome — a
        // successful refresh, or the IdP's own error if `refresh` threw. The lock frees by TTL
        // regardless, so a failed release is recoverable; masking the real outcome is not.
        try {
          await store.releaseLock(sid, lockToken); // invariant 4
        } catch {
          logger.event('store.unavailable', { sid: sidTag(sid), stage: 'release_lock' });
        }
      }
    }

    if (Date.now() >= deadline) {
      // invariant 2
      logger.event('session.refresh_timeout', { sid: sidTag(sid) });
      const last = await store.get(sid);
      if (last === null) return null;
      // Still inside the skew window means the access token is live: proceed on it and let the
      // next request refresh. Only a genuinely expired token degrades to "signed out", which has
      // a defined recovery path, rather than to a 500 from a server component.
      return Date.now() < last.accessExpiresAt ? { ...last, refreshPending: true } : null;
    }

    await sleep(backoff(attempt));
    const reread = await store.get(sid);
    if (reread === null) return null;
    if (!shouldRefresh(Date.now(), reread.accessExpiresAt, skewMs)) return reread;
  }
}
