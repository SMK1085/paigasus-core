// SPDX-License-Identifier: Apache-2.0
import { RefreshRejected } from './errors';
import { newLockToken } from './ids';
import { shouldRefresh } from './refresh-policy';
import type { SessionRecord } from './session';
import type { AuthLogger } from '../ports/logger';
import { sidTag } from '../ports/logger';
import type { SessionStore } from '../ports/session-store';

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

/**
 * `refreshState` says WHY the returned record may be stale, and the two values are opposites:
 *
 *   'pending' — another holder is refreshing right now (the lock wait timed out). The next
 *               request very likely sees a fresh record.
 *   'failed'  — the refresh itself failed transiently and NOBODY is refreshing. The next request
 *               fails the same way.
 *
 * One boolean cannot carry both, and a consumer reading it as "retry shortly" would hot-loop
 * through an identity-provider outage. It replaced the earlier single boolean flag in SMA-626
 * § 2.3, while that flag still had no consumer anywhere in the repository.
 */
export type ResolvedSession = SessionRecord & { refreshState?: 'pending' | 'failed' };

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

        // I4 (final fix wave): a thrown `refresh` (e.g. the IdP token endpoint returning 503)
        // used to propagate all the way to `get-session.ts`'s catch, which logs `store.unavailable`
        // with `stage: 'get_session'` — a store failure and an IdP outage then produce the
        // IDENTICAL event, so an operator investigating a healthy Redis chases the wrong system.
        // Log the distinguishing event HERE, at the point that actually knows which call failed.
        // Same field discipline as the rest: no token, no URL, no raw error object — only the
        // truncated sid, a fixed reason string, and a boolean.
        let tokens: RefreshedTokens;
        try {
          tokens = await refresh(refreshToken);
        } catch (err) {
          // SMA-626 § 2.3. THREE outcomes, not one.
          //
          // A definitive rejection (RefreshRejected — only `invalid_grant`, see
          // adapters/oidc.ts's classifier) means the refresh token is revoked: an administrator
          // ended this session, or the user signed out elsewhere. Sign out, and DELETE — leaving
          // the record would keep a revoked refresh token and a live access token in the store for
          // the full ttlMs, and every later read would re-take the lock and re-call the token
          // endpoint until then. The same reasoning as the `no_refresh_token` and
          // `session.refresh.persist_failed` deletes above and below.
          //
          // A transient failure (a network error, a timeout, a 5xx, an unknown OAuth code) with a
          // still-live access token degrades exactly the way the lock-timeout branch does: the
          // token works, so proceed on it and let the next request retry. Signing the user out
          // there would throw away up to skewMs of perfectly good session because someone else's
          // service blipped.
          //
          // A transient failure with a hard-expired token has nothing left to proceed on.
          //
          // `liveUntil` takes the MINIMUM of the two expiries. handleCallback sets them
          // independently (http/routes.ts:240-241) and only a refresh write clamps accessExpiresAt
          // to the cap, so an IdP whose `expires_in` exceeds PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS
          // mints a first record whose access token outlives its own absolute cap.
          //
          // `reason` is REQUIRED, not decoration. With `degraded` alone, a benign single-session
          // revocation and an outage that signs users out produce the identical line — the exact
          // conflation this whole section exists to remove.
          const rejected = err instanceof RefreshRejected;
          const liveUntil = Math.min(fresh.accessExpiresAt, fresh.absoluteExpiresAt);
          const degraded = !rejected && Date.now() < liveUntil;

          logger.event('session.refresh_failed', { sid: sidTag(sid), reason: rejected ? 'rejected' : 'transient', degraded });

          // The early return still runs the `finally` below, so the lock is released either way.
          if (degraded) return { ...fresh, refreshState: 'failed' };
          if (rejected) {
            await store.delete(sid);
            logger.event('session.deleted', { sid: sidTag(sid), reason: 'refresh_rejected' });
          }
          throw err;
        }
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
      // a defined recovery path, rather than to a 500 from a server component. `Math.min` for the
      // same reason the refresh catch uses it — a first record's accessExpiresAt is not clamped to
      // its absolute cap (SMA-626 § 2.3).
      return Date.now() < Math.min(last.accessExpiresAt, last.absoluteExpiresAt) ? { ...last, refreshState: 'pending' } : null;
    }

    await sleep(backoff(attempt));
    const reread = await store.get(sid);
    if (reread === null) return null;
    if (!shouldRefresh(Date.now(), reread.accessExpiresAt, skewMs)) return reread;
  }
}
