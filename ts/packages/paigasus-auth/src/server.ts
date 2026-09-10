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
import { CallbackRejected } from './core/errors.js';
import { createAuthRoutes, type AuthRoutes } from './http/routes.js';
import type { AuthRuntime } from './runtime.js';

export { createAuthRuntime, getAuthRuntime } from './runtime.js';
export type { AuthRuntime, ComposedConfig, CreateAuthRuntimeDeps } from './runtime.js';

export { createAuthRoutes };
export type { AuthRoutes };

export { getSession, requireSession } from './next/get-session.js';
export type { RequireSessionOptions } from './next/get-session.js';

/**
 * Builds the same four routes as `createAuthRoutes`, mounted as a single Next route handler
 * (`app/[zone]/auth/[...auth]/route.ts`'s `export const GET/POST`), but with `CallbackRejected`
 * mapped to a `Response` instead of left to propagate.
 *
 * `http/routes.ts` deliberately lets `CallbackRejected` reject `handle()`'s promise rather than
 * deciding an HTTP response itself (see that file's header) — this is the Next boundary that
 * makes that decision, so a stale tab hitting `/auth/callback` a second time becomes a redirect,
 * not an unhandled rejection a Next route handler turns into a 500.
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
 */
export function createAuthRouteHandler(runtime: AuthRuntime): AuthRoutes['handle'] {
  const routes = createAuthRoutes(runtime);

  const redirectTo = (path: string): Response => new Response(null, { status: 302, headers: new Headers({ Location: path }) });

  return async function handle(req: Request): Promise<Response> {
    try {
      return await routes.handle(req);
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

export { SESSION_VIEW_KEYS, toSessionView } from './core/session.js';
export type { SessionRecord, SessionView, SessionViewKey } from './core/session.js';

export type { ResolvedSession } from './core/single-flight.js';

export { authEnvShape } from './config.js';
export type { AuthEnv } from './config.js';

export { AuthConfigError, AuthError, SessionStoreUnavailable } from './core/errors.js';
export { CallbackRejected };

// Ports.
export type { LoginTransaction, SessionStore } from './ports/session-store.js';
export type { IdTokenClaims, Membership, PrincipalResolver, ResolvedPrincipal, RoleGrantRef } from './ports/principal-resolver.js';
export { sidTag } from './ports/logger.js';
export type { AuthEventFields, AuthEventName, AuthLogger } from './ports/logger.js';

// Adapters.
export { MemorySessionStore } from './adapters/memory-store.js';
export { createRedisSessionStore } from './adapters/redis-store.js';
export type { CreateRedisSessionStoreOptions } from './adapters/redis-store.js';
export { claimsPrincipalResolver } from './adapters/claims-resolver.js';
export { noopLogger } from './adapters/noop-logger.js';
export { createOidcClient } from './adapters/oidc.js';
export type {
  AuthorizationCodeGrantParams,
  AuthorizationRequest,
  BuildAuthorizationUrlParams,
  BuildEndSessionUrlParams,
  CreateOidcClientOptions,
  OidcClient,
  OidcTokens,
  RefreshedTokens as OidcRefreshedTokens,
} from './adapters/oidc.js';
