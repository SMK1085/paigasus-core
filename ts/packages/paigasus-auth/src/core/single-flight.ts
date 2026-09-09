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

/**
 * Resolve a session, refreshing under a per-session lock if the access token is at or past its
 * skew window. AT MOST ONE refresh happens per session across concurrent callers.
 *
 * FIVE INVARIANTS. Each is a defect if dropped:
 *
 *  1. The DOUBLE-CHECK after acquiring the lock. Without it two SEQUENTIAL holders both refresh,
 *     and the second presents a refresh token the IdP rotated and revoked a millisecond earlier.
 *     A lock alone does not prevent this — it is the least obvious point in the design.
 *  2. The waiter NEVER refreshes on timeout. Falling back to "refresh anyway" reinstates the race
 *     under exactly the load where it matters.
 *  3. A unique lock token with a compare-and-delete release, so a holder whose TTL expired cannot
 *     delete the next holder's lock.
 *  4. Release in `finally`, so a throwing IdP call does not hold the lock for its full TTL.
 *  5. The write is FENCED on `rev`. A refresh that outlives its lock TTL would otherwise write
 *     its now-revoked token over a newer valid one; compare-and-delete protects the LOCK, never
 *     the WRITE. The caller additionally asserts the refresh HTTP timeout is below lockTtlMs.
 *
 * `now` is re-read EVERY ITERATION. Binding it once makes the deadline unreachable and the loop
 * never terminates.
 *
 * CORRECTION TO THE ORIGINAL BRIEF (SMA-506 task 5): a failed compare-and-set alone cannot tell
 * "a newer writer fenced me" apart from "the store is broken" — both return `false` from `set`.
 * Distinguish them by `rev`: re-read the record, and only treat it as a fencing win when its
 * `rev` is strictly newer than the one this call fenced on. Otherwise the write genuinely failed;
 * retry it once against whatever `rev` is currently stored (or insert fresh if the record is
 * gone), and only then give up. On a persistent failure, delete rather than leave the record in
 * place: the rotated refresh token is already lost, so the stored record holds one the identity
 * provider has already revoked, and every later refresh would fail against it forever. A clean
 * re-login is the recoverable outcome.
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
        if (!shouldRefresh(Date.now(), fresh.accessExpiresAt, skewMs)) return fresh; // invariant 1
        if (fresh.refreshToken === undefined) {
          await store.delete(sid);
          logger.event('session.deleted', { sid: sidTag(sid), reason: 'no_refresh_token' });
          return null;
        }
        const refreshToken = fresh.refreshToken;

        const tokens = await refresh(refreshToken);
        const next: SessionRecord = {
          ...fresh,
          rev: fresh.rev + 1,
          accessToken: tokens.accessToken,
          refreshToken: tokens.refreshToken ?? fresh.refreshToken,
          accessExpiresAt: Math.min(Date.now() + tokens.expiresIn * 1000, fresh.absoluteExpiresAt),
        };

        // Invariant 5. A false here means either a newer writer fenced us, or the store failed.
        let ok = await store.set(sid, next, ttlMs, fresh.rev);
        if (!ok) {
          const winner = await store.get(sid);
          // A newer writer fenced us — their record is authoritative, take it.
          if (winner !== null && winner.rev > fresh.rev) return winner;
          // Otherwise the write genuinely failed. Retry ONCE against current state.
          ok = await store.set(sid, { ...next, rev: (winner?.rev ?? fresh.rev) + 1 }, ttlMs, winner?.rev ?? null);
        }
        if (!ok) {
          // Retried once and still could not persist. A clean re-login beats a session that can
          // never refresh again, so delete rather than leave the revoked token in place.
          await store.delete(sid);
          logger.event('session.refresh.persist_failed', { sid: sidTag(sid) });
          return null;
        }

        logger.event('session.refreshed', { sid: sidTag(sid), rev: next.rev });
        return next;
      } finally {
        await store.releaseLock(sid, lockToken); // invariant 4
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
