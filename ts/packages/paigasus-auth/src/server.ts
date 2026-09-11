// SPDX-License-Identifier: Apache-2.0
//
// The server-only package entry (task 10). `import 'server-only'` is the first statement after
// this header: it throws when the `react-server` export condition is ABSENT — i.e. in a client
// component bundle — so a stray `import { getAuthRuntime } from '@paigasus/auth/server'` from
// client code fails at build time rather than shipping a store adapter and an OIDC client secret
// into the browser (AC 5). It is a client-bundle guard only; it does not cover the edge
// runtime — that is `src/middleware.ts`'s job, and exactly why middleware is its OWN entry point
// with no import path back to this file (AC 4).
//
// This module composes everything task 8/9 built plus the Next binding this task adds:
// `createAuthRuntime`/`getAuthRuntime` (the composition root), `createAuthRoutes` (the four HTTP
// routes), `createAuthRouteHandler` (the same routes, with `CallbackRejected` mapped to a
// response — see its own doc comment below), `getSession`/`requireSession` (the Next read path),
// `toSessionView` (the ONLY function that may construct what crosses to `@paigasus/auth/client`),
// `authEnvShape` (the env shape an app composes into `@paigasus/next-config`'s
// `defineRuntimeConfig`), and every port and adapter.
import 'server-only';
import { CallbackRejected } from './core/errors';
import { AUTH_ROUTE_SUFFIXES } from './http/route-table';
import { createAuthRoutes, type AuthRoutes } from './http/routes';
import type { AuthRuntime } from './runtime';

export { createAuthRuntime, getAuthRuntime } from './runtime';
export type { AuthRuntime, ComposedConfig, CreateAuthRuntimeDeps } from './runtime';

export { createAuthRoutes };
export type { AuthRoutes };

export { getSession, requireSession } from './next/get-session';
export type { RequireSessionOptions } from './next/get-session';

/** A `RequestInit` that can carry a streamed body: Node's `Request` needs `duplex: 'half'` for one. */
interface RequestInitWithDuplex extends RequestInit {
  duplex?: 'half';
}

/**
 * The URL `createAuthRoutes` must see: this zone's PUBLIC origin, the FULL path (basePath
 * included) and the incoming query string (SMA-511 spec § 7.1).
 *
 * A Next route handler gets a `req.url` with the basePath REMOVED and the server's BIND address as
 * its origin — measured on Next 16.3.4: `http://0.0.0.0:<port>/auth/callback?…` under
 * `basePath: '/iam'` (spec § 13 row 1). The core route table is keyed by the full path, so this
 * puts the basePath back. A plain `node:http` caller (tests/e2e/fixture-server.ts) passes the full
 * path already. The route table tells the two apart, not a prefix test: a path that is already one
 * of this zone's auth routes is kept, and a path that becomes one with the basePath added gets it.
 * So a zone whose basePath is `/auth` is not ambiguous.
 */
function publicRequestUrl(runtime: AuthRuntime, routePaths: ReadonlySet<string>, raw: string): string {
  const incoming = new URL(raw);
  const prefixed = `${runtime.basePath}${incoming.pathname}`;
  const pathname = !routePaths.has(incoming.pathname) && routePaths.has(prefixed) ? prefixed : incoming.pathname;
  // Concatenated onto the absolute origin, never `new URL(path, origin)`: a path such as
  // `//evil.example/auth/login` would otherwise resolve as a protocol-relative URL on another host.
  return `${runtime.publicOrigin}${pathname}${incoming.search}`;
}

/** The same request at another URL. Method, headers, body and abort signal carry over. */
function withUrl(req: Request, url: string): Request {
  const init: RequestInitWithDuplex = { method: req.method, headers: req.headers, signal: req.signal };
  if (req.method !== 'GET' && req.method !== 'HEAD' && req.body !== null) {
    init.body = req.body;
    init.duplex = 'half';
  }
  return new Request(url, init);
}

/**
 * Builds the same four routes as `createAuthRoutes`, mounted as a single Next route handler
 * (`app/auth/[...auth]/route.ts`'s `GET`/`POST`), with two differences.
 *
 * 1. The request URL is rebuilt on `PAIGASUS_PUBLIC_ORIGIN` with the basePath put back (see
 *    `publicRequestUrl` above), so the core route table, keyed by the full path, finds the route.
 *
 * 2. `CallbackRejected` is mapped to a `Response` instead of left to propagate. `http/routes.ts`
 *    deliberately lets `CallbackRejected` reject `handle()`'s promise rather than deciding an HTTP
 *    response itself (see that file's header) — this is the Next boundary that makes that decision,
 *    so a stale tab hitting `/auth/callback` a second time becomes a redirect, not an unhandled
 *    rejection a Next route handler turns into a 500.
 *
 * The five reasons split into two outcomes. `txn_missing`, `txn_mismatch`, and `state_unknown` all
 * mean "this callback cannot be completed with what the server has" — a stale tab, an expired
 * (10-minute) transaction, or a replayed request — and the safe, unsurprising recovery is the same
 * for all three: send the browser back to start a fresh login. `idp_error` is the user themselves
 * declining at the identity provider (e.g. clicking Cancel), so it returns them to the zone root
 * rather than straight back into another login attempt. `code_exchange_failed` is the one reason
 * that is a genuine failure (an unreachable or erroring token endpoint) rather than an expected
 * outcome, so it surfaces as a 502 rather than a silent redirect — an operator must be able to
 * tell it apart from ordinary traffic.
 *
 * The redirect Locations below are FULL paths. A raw `Response` Location from a route handler
 * passes through Next unchanged (measured, spec § 13 row 1).
 */
export function createAuthRouteHandler(runtime: AuthRuntime): AuthRoutes['handle'] {
  const routes = createAuthRoutes(runtime);
  const routePaths: ReadonlySet<string> = new Set(AUTH_ROUTE_SUFFIXES.map((suffix) => `${runtime.basePath}${suffix}`));

  const redirectTo = (path: string): Response => new Response(null, { status: 302, headers: new Headers({ Location: path }) });

  return async function handle(req: Request): Promise<Response> {
    try {
      return await routes.handle(withUrl(req, publicRequestUrl(runtime, routePaths, req.url)));
    } catch (err) {
      if (!(err instanceof CallbackRejected)) throw err;

      switch (err.reason) {
        case 'txn_missing':
        case 'txn_mismatch':
        case 'state_unknown':
          return redirectTo(`${runtime.basePath}/auth/login`);
        case 'idp_error':
          return redirectTo(`${runtime.basePath}/`);
        case 'code_exchange_failed':
          return new Response('login failed', { status: 502 });
        default: {
          // Exhaustiveness guard: a future sixth CallbackRejected reason fails typecheck here
          // rather than silently falling through to an unhandled rejection again.
          const exhaustive: never = err.reason;
          throw new Error(`unreachable: unmapped CallbackRejected reason ${String(exhaustive)}`, { cause: err });
        }
      }
    }
  };
}

export { SESSION_VIEW_KEYS, toSessionView } from './core/session';
export type { SessionRecord, SessionView, SessionViewKey } from './core/session';

export type { ResolvedSession } from './core/single-flight';

export { authEnvShape } from './config';
export type { AuthEnv } from './config';

export { AuthConfigError, AuthError, SessionStoreUnavailable } from './core/errors';
export { CallbackRejected };

// Ports.
export type { LoginTransaction, SessionStore } from './ports/session-store';
export type { IdTokenClaims, Membership, PrincipalResolver, ResolvedPrincipal, RoleGrantRef } from './ports/principal-resolver';
export { sidTag } from './ports/logger';
export type { AuthEventFields, AuthEventName, AuthLogger } from './ports/logger';

// Adapters.
export { MemorySessionStore } from './adapters/memory-store';
export { createRedisSessionStore } from './adapters/redis-store';
export type { CreateRedisSessionStoreOptions } from './adapters/redis-store';
export { claimsPrincipalResolver } from './adapters/claims-resolver';
export { noopLogger } from './adapters/noop-logger';
export { createOidcClient } from './adapters/oidc';
export type {
  AuthorizationCodeGrantParams,
  AuthorizationRequest,
  BuildAuthorizationUrlParams,
  BuildEndSessionUrlParams,
  CreateOidcClientOptions,
  OidcClient,
  OidcTokens,
  RefreshedTokens as OidcRefreshedTokens,
} from './adapters/oidc';
