// SPDX-License-Identifier: Apache-2.0
//
// The zone's proxy (Next 16's name for middleware; spec § 3.3, § 7.5). Two jobs, and no more:
//
// 1. COOKIE PRESENCE ONLY (ADR-0017 decision 7). It imports @paigasus/auth/middleware, never
//    /server and never the sdk (paigasus/boundaries/app-middleware). A forged or expired cookie
//    passes here and is rejected by requireSession() in the (console) layout.
// 2. A per-request correlation id (spec § 6.2), minted into the REQUEST headers so that
//    lib/iam.ts can send it to IAM and forbidden.tsx can read it with headers().
//
// Public paths are basePath-RELATIVE: Next strips the basePath from req.nextUrl.pathname before
// the proxy sees it (spec § 13 #1).
import { NextResponse, type NextRequest } from 'next/server';
import { authRoutePaths, createAuthMiddleware } from '@paigasus/auth/middleware';
import { CORRELATION_HEADER, REQUEST_PATH_HEADER } from './lib/correlation-header';

const authMiddleware = createAuthMiddleware({
  publicPaths: [...authRoutePaths(), '/', '/healthz'],
  loginPath: '/auth/login',
});

export function proxy(req: NextRequest): NextResponse {
  const decision = authMiddleware(req);
  // A redirect to login is final. Anything else continues, with the two request headers set.
  if (decision.headers.has('location')) return decision;
  const headers = new Headers(req.headers);
  // ALWAYS a fresh id: a value the browser sent is overwritten, so the id in IAM's logs is one this
  // server chose.
  headers.set(CORRELATION_HEADER, crypto.randomUUID());
  headers.set(REQUEST_PATH_HEADER, `${req.nextUrl.basePath}${req.nextUrl.pathname}`);
  return NextResponse.next({ request: { headers } });
}

/*
 * Without a matcher the proxy runs for every CSS and JS file too, and a visitor with no cookie gets
 * a login redirect for each one (spec § 7.5). MEASURED (spec § 13 #4): this pattern, written
 * WITHOUT the /iam prefix, skips the static assets under the basePath.
 */
export const config = {
  matcher: ['/((?!_next/static|_next/image|favicon.ico).*)'],
};
