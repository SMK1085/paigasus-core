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
import { SESSION_COOKIE } from './http/cookies.js';

export interface AuthMiddlewareOptions {
  /**
   * Exact pathnames (matched against `req.nextUrl.pathname`) that never require a session
   * cookie. MUST include every route `createAuthRoutes` serves for this zone — `/auth/login`,
   * `/auth/callback`, `/auth/logout`, `/auth/logout/callback` — or a signed-out visitor
   * redirected to `loginPath` is redirected right back to itself: `loginPath` has no cookie
   * either, and this file has no way to recognise it is already the login route without being
   * told so explicitly (it does not import `runtime.basePath` or anything else that would let it
   * infer its own routes — see the file header).
   */
  publicPaths: readonly string[];
  /** Where a signed-out visitor is sent. Usually `${runtime.basePath}/auth/login`. */
  loginPath: string;
}

/** Build this zone's middleware. */
export function createAuthMiddleware(options: AuthMiddlewareOptions): (req: NextRequest) => NextResponse {
  const publicPaths = new Set(options.publicPaths);

  return function authMiddleware(req: NextRequest): NextResponse {
    const { pathname } = req.nextUrl;

    if (publicPaths.has(pathname)) {
      return NextResponse.next();
    }

    // PRESENCE ONLY (AC 4) — see the file header. Never call anything that would tell this cookie
    // apart from a forged one.
    if (req.cookies.has(SESSION_COOKIE)) {
      return NextResponse.next();
    }

    const loginUrl = new URL(options.loginPath, req.nextUrl.origin);
    loginUrl.searchParams.set('returnTo', `${pathname}${req.nextUrl.search}`);
    return NextResponse.redirect(loginUrl);
  };
}
