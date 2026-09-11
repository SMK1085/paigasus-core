// SPDX-License-Identifier: Apache-2.0
//
// The middleware factory — AC 4: no authorization logic runs here, structurally, not by
// convention. Design doc § 10: `createAuthMiddleware({ publicPaths, loginPath })`.
//
// WHY THIS IS ITS OWN PACKAGE ENTRY POINT. Next's middleware runs in the edge runtime, which has
// no dynamic `process.env` (see @paigasus/next-config's src/runtime.ts) and no Redis client. If
// this file could import a store, a resolver, or `openid-client`, a future edit could make it
// start VALIDATING the cookie instead of merely checking its presence — and a middleware that can
// authorize is exactly the shape of CVE-2025-29927 (a middleware auth-bypass). Being its own entry
// point with its own import graph makes that class of edit impossible to land here by accident:
// there is no import path from this file to anything that knows what a session record, a grant,
// or a token look like. `tests/middleware.test.ts` asserts the transitive import graph directly,
// rather than trusting this comment to stay true.
//
// PRESENCE, NEVER VALIDITY. A forged or expired `__Host-pgs_sid` cookie passes THIS check and is
// rejected downstream, by the store lookup inside `getSession()` / `requireSession()`
// (src/next/get-session.ts). That is the control, not a limitation: the alternative — letting
// middleware decide who is authenticated — is the bug class this file exists to avoid.
import { NextResponse } from 'next/server';
import type { NextRequest } from 'next/server';
import { SESSION_COOKIE } from './http/cookies';
import { AUTH_ROUTE_SUFFIXES } from './http/route-table';

export interface AuthMiddlewareOptions {
  /**
   * BasePath-RELATIVE pathnames that never require a session cookie, matched exactly against
   * `req.nextUrl.pathname`. Next removes the zone's basePath from that value before the proxy sees
   * it (measured, SMA-511 spec § 13 row 1: under `basePath: '/iam'` a request for `/iam/auth/login`
   * has `nextUrl.pathname === '/auth/login'`). So list `/auth/login`, never `/iam/auth/login`: a
   * prefixed entry never matches, and the login route then redirects to itself.
   *
   * MUST include every route `createAuthRoutes` serves — `/auth/login`, `/auth/callback`,
   * `/auth/logout`, `/auth/logout/callback` — or a signed-out visitor redirected to `loginPath` is
   * redirected right back to itself: `loginPath` has no cookie either, and this file has no way to
   * recognise it is already the login route without being told so explicitly.
   *
   * Use `authRoutePaths()` below to build this list instead of hand-copying it — a hand-copied list
   * drifts from `createAuthRoutes`'s own route table with no error anywhere: an app that forgets the
   * callback path here still builds and deploys, and the failure is a silent infinite redirect loop
   * (I5, final fix wave) rather than a build-time or lint-time signal.
   */
  publicPaths: readonly string[];
  /**
   * Where a signed-out visitor is sent, basePath-RELATIVE: usually `/auth/login`. The redirect
   * keeps the zone's basePath, so the browser lands on `/iam/auth/login`.
   */
  loginPath: string;
}

/**
 * The four basePath-RELATIVE pathnames `createAuthRoutes` dispatches on, for use as
 * `AuthMiddlewareOptions.publicPaths` — I5, final fix wave.
 *
 * THE FAILURE THIS CLOSES. `publicPaths` used to be the caller's own hand-copied list, with nothing
 * binding it to `http/routes.ts`'s actual route table. Omit one path there — the callback path is
 * the easy one to miss — and `/auth/login` clears the session cookie, the IdP's redirect back to
 * `/auth/callback` arrives with no cookie, middleware (seeing a non-public path with no cookie)
 * bounces it BACK to `/auth/login`, which clears the cookie again and redirects again: the user
 * loops forever, with no error anywhere.
 *
 * BASEPATH-RELATIVE SINCE SMA-511 (spec § 7.1). The middleware compares `req.nextUrl.pathname`,
 * which Next gives without the basePath, so this list carries no basePath and takes no argument.
 * The core route table (`http/routes.ts`) is still keyed by the FULL path; `createAuthRouteHandler`
 * puts the basePath back for a route handler.
 *
 * Derived from `route-table.ts`'s shared `AUTH_ROUTE_SUFFIXES` tuple (SMA-626 § 3) — the same table
 * `http/routes.ts` builds its `Record<AuthRouteSuffix, …>` dispatch from. That module imports
 * nothing, so importing it here adds no new edge into this entry point's module graph, and AC 4's
 * import-graph guarantee is unchanged (see the file header and `tests/middleware.test.ts`).
 */
export function authRoutePaths(): readonly string[] {
  return [...AUTH_ROUTE_SUFFIXES];
}

/** Build this zone's middleware (Next 16: the zone's `proxy.ts`). */
export function createAuthMiddleware(options: AuthMiddlewareOptions): (req: NextRequest) => NextResponse {
  const publicPaths = new Set(options.publicPaths);

  return function authMiddleware(req: NextRequest): NextResponse {
    // basePath-RELATIVE: Next removes the zone's basePath from `nextUrl.pathname` (see publicPaths).
    const { pathname, basePath, search } = req.nextUrl;

    if (publicPaths.has(pathname)) {
      return NextResponse.next();
    }

    // PRESENCE ONLY (AC 4) — see the file header. Never call anything that would tell this cookie
    // apart from a forged one.
    if (req.cookies.has(SESSION_COOKIE)) {
      return NextResponse.next();
    }

    // A clone of nextUrl keeps Next's basePath configuration, so a basePath-relative loginPath
    // redirects to `/iam/auth/login` exactly once (measured, spec § 13 row 1). `returnTo` is a FULL
    // path: the callback sends it back as a raw Location from a route handler, and Next does not
    // prefix that.
    const loginUrl = req.nextUrl.clone();
    loginUrl.pathname = options.loginPath;
    loginUrl.search = '';
    loginUrl.hash = '';
    loginUrl.searchParams.set('returnTo', `${basePath}${pathname}${search}`);
    return NextResponse.redirect(loginUrl);
  };
}
