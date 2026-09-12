// SPDX-License-Identifier: Apache-2.0
//
// The zone's proxy (Next 16's name for middleware; spec § 3.3, § 7.5).
//
// COOKIE PRESENCE ONLY (ADR-0017 decision 7). It imports @paigasus/auth/middleware, never /server
// and never the sdk (paigasus/boundaries/app-middleware). A forged or expired cookie passes here and
// is rejected by requireSession() in the (console) layout.
//
// Public paths are basePath-RELATIVE: Next strips the basePath from req.nextUrl.pathname before the
// proxy sees it (spec § 13 #1), and Task 5 makes createAuthMiddleware add it back to the login
// redirect and to returnTo.
import type { NextRequest, NextResponse } from 'next/server';
import { authRoutePaths, createAuthMiddleware } from '@paigasus/auth/middleware';

const authMiddleware = createAuthMiddleware({
  publicPaths: [...authRoutePaths(), '/', '/healthz'],
  loginPath: '/auth/login',
});

export function proxy(req: NextRequest): NextResponse {
  return authMiddleware(req);
}

/*
 * Without a matcher the proxy runs for every CSS and JS file too, and a visitor with no cookie gets
 * a login redirect for each one (spec § 7.5). MEASURED (spec § 13 #4): this pattern, written
 * WITHOUT the /iam prefix, skips the static assets under the basePath.
 */
export const config = {
  matcher: ['/((?!_next/static|_next/image|favicon.ico).*)'],
};
