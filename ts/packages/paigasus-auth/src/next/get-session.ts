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
import { SessionStoreUnavailable } from '../core/errors';
import { validateReturnTo } from '../core/return-to';
import { resolveSession, type ResolvedSession } from '../core/single-flight';
import { SESSION_COOKIE } from '../http/cookies';
import { sidTag } from '../ports/logger';
import type { AuthRuntime } from '../runtime';

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
 * Read the current session, redirecting to this zone's login path when there is none. Safe to call
 * from a SERVER COMPONENT — a page or a layout: `redirect()` is the one recovery a server component
 * may perform, and it is what turns a stale `__Host-pgs_sid` cookie into a working "sign in again"
 * prompt instead of a permanently blank page.
 *
 * THE REDIRECT TARGET IS BASEPATH-RELATIVE (SMA-511 spec § 7.1). Next's redirect() adds the
 * basePath itself, with no duplicate check: `redirect('/auth/login?…')` gives `Location:
 * /iam/auth/login?…`, and `redirect('/iam/auth/login')` gives `/iam/iam/auth/login` (measured, spec
 * § 13 row 1). `returnTo` keeps the basePath, because the callback sends it back as a raw Location
 * header from a route handler, which Next passes through unchanged. Do not call this from a route
 * handler; a route handler returns its own redirect Response.
 *
 * NOT SAFE FROM A SERVER ACTION UNDER A BASEPATH. An action takes a different path through Next,
 * and that path adds NO basePath: `next/dist/server/app-render/action-handler.js:261` writes the
 * RAW url into the `x-action-redirect` header, and `:906` writes the RAW url into `Location` for a
 * no-JS post. Only the internal RSC pre-fetch at `:267` prefixes the basePath. The browser resolves
 * the raw value against the current URL and hard-navigates
 * (`server-action-reducer.js:134`, `:274-279`), so `/auth/login?…` sends the user to
 * `https://<host>/auth/login`, OUTSIDE the zone, where no login route exists. An action must call
 * `getSession()` and report the missing session as DATA instead — `@paigasus/auth` cannot do that
 * for the caller, because only the caller knows its own result shape. The IAM console's
 * `lib/iam.ts` `iamClientsForAction()` is the worked example.
 */
export async function requireSession(runtime: AuthRuntime, options: RequireSessionOptions = {}): Promise<ResolvedSession> {
  const session = await getSession(runtime);
  if (session !== null) return session;

  const returnTo = validateReturnTo(options.returnTo, `${runtime.basePath}/`);
  redirect(`/auth/login?returnTo=${encodeURIComponent(returnTo)}`);
}
