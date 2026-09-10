// SPDX-License-Identifier: Apache-2.0
//
// The Next binding's read path (task 10; design doc § 10.1's stale-cookie recovery).
//
// THE TRAP THIS FILE CLOSES. `__Host-pgs_sid` is a browser-session cookie the browser keeps
// sending until it expires or the browser clears it. The STORE record it names can disappear for
// reasons the browser never learns about — the record's absolute expiry, a logout in another tab,
// a `version` bump (core/session.ts), or a Redis wipe. `src/middleware.ts` checks only whether the
// cookie is PRESENT (AC 4), so none of those cases make it redirect. And a Next SERVER COMPONENT
// cannot delete a cookie — only a route handler / server action can set `Set-Cookie`, and
// `/auth/login` is the one route in this package that does (http/routes.ts, unconditionally).
//
// Without a deliberate recovery path every page would render unauthenticated forever, with the
// browser still holding a cookie and no route back to login — the console looks broken and the
// user has no way to fix it themselves. The recovery is these two functions, with DISTINCT
// contracts:
//
//   getSession()     never redirects — a page that renders differently when signed out (rather
//                     than requiring a session at all) must be able to call this without a
//                     surprise navigation.
//   requireSession()  redirects to the login path when getSession() resolves null. Redirecting
//                     IS allowed from a server component (design doc § 10.1); writing a cookie is
//                     not. `/auth/login` clears the stale cookie at the one place allowed to,
//                     which is what closes the loop.
//
// A STORE OUTAGE DEGRADES TO SIGNED-OUT, NEVER TO A 500. `resolveSession` (core/single-flight.ts)
// propagates a thrown `SessionStoreUnavailable` (or any other store failure) rather than
// swallowing it — that is correct for callers that need to distinguish the two. This file is not
// that caller: a page rendering "please sign in" during a Redis blip is a far better outcome than
// every page in the console 500ing, so the catch below is deliberately broad, not narrowed to
// `SessionStoreUnavailable` alone.
import { cookies } from 'next/headers';
import { redirect } from 'next/navigation';
import { SessionStoreUnavailable } from '../core/errors.js';
import { validateReturnTo } from '../core/return-to.js';
import { resolveSession, type ResolvedSession } from '../core/single-flight.js';
import { SESSION_COOKIE } from '../http/cookies.js';
import { sidTag } from '../ports/logger.js';
import type { AuthRuntime } from '../runtime.js';

/**
 * Read the current session, if any. NEVER redirects and NEVER throws — every failure mode (no
 * cookie, a cookie naming a deleted record, a store outage) collapses to `null`. Callers that must
 * have a session should use `requireSession` instead.
 */
export async function getSession(runtime: AuthRuntime): Promise<ResolvedSession | null> {
  const jar = await cookies();
  const sid = jar.get(SESSION_COOKIE)?.value;
  if (sid === undefined) return null;

  try {
    return await resolveSession(
      {
        store: runtime.store,
        refresh: (refreshToken) => runtime.oidc.refresh(refreshToken),
        logger: runtime.logger,
        skewMs: runtime.skewMs,
        lockTtlMs: runtime.lockTtlMs,
        lockWaitMs: runtime.lockWaitMs,
        ttlMs: runtime.ttlMs,
      },
      sid,
    );
  } catch (err) {
    // A store blip degrades to signed-out rather than a 500 — see the file header. That degrade
    // must not also be SILENT, so every failure still logs exactly one line.
    //
    // WHICH line is the point (SMA-626 § 2.4). This catch is deliberately broad, and it used to
    // log `store.unavailable` for every failure — so an identity-provider outage raised the
    // store-outage rate while Redis was healthy, and an operator investigating a mass sign-out
    // went and inspected a perfectly good store. Classifying here costs one `instanceof` and makes
    // `store.unavailable` mean the store.
    //
    // Same field discipline either way: a truncated sid, a fixed stage name, never the caught
    // error object (it may embed a DSN).
    if (err instanceof SessionStoreUnavailable) {
      runtime.logger.event('store.unavailable', { sid: sidTag(sid), stage: 'get_session' });
    } else {
      runtime.logger.event('session.resolve_failed', { sid: sidTag(sid), stage: 'get_session' });
    }
    return null;
  }
}

export interface RequireSessionOptions {
  /**
   * The path to return to after a successful login. Validated by `core/return-to.ts`'s
   * same-origin rule — an invalid value falls back to the zone root, never to an open redirect.
   */
  returnTo?: string;
}

/**
 * Read the current session, redirecting to this zone's login path when there is none. Safe to
 * call from a server component: `redirect()` is the one recovery a server component may perform,
 * and it is what turns a stale `__Host-pgs_sid` cookie into a working "sign in again" prompt
 * instead of a permanently blank page.
 */
export async function requireSession(runtime: AuthRuntime, options: RequireSessionOptions = {}): Promise<ResolvedSession> {
  const session = await getSession(runtime);
  if (session !== null) return session;

  const loginPath = `${runtime.basePath}/auth/login`;
  const returnTo = validateReturnTo(options.returnTo, `${runtime.basePath}/`);
  redirect(`${loginPath}?returnTo=${encodeURIComponent(returnTo)}`);
}
