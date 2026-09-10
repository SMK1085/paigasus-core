// SPDX-License-Identifier: Apache-2.0
//
// The four route suffixes, in ONE place (SMA-626 § 3).
//
// THIS MODULE IMPORTS NOTHING, and that is load-bearing rather than incidental. `src/middleware.ts`
// is its own package entry point whose transitive import graph must reach no store, no resolver
// and no `openid-client` (AC 4, asserted by tests/middleware.test.ts). A leaf module with no edges
// of its own can be shared with `src/http/routes.ts` — which does reach all of those — without
// putting a single new edge into the middleware graph.
//
// WHY IT EXISTS. `authRoutePaths` and `createAuthRoutes`'s dispatch used to be two hand-written
// lists. A route missing from `publicPaths` means `/auth/login` clears the session cookie, the
// identity provider's redirect arrives without one, middleware bounces it back to `/auth/login`,
// and the user loops forever with no error anywhere. `routes.ts` builds a
// `Record<AuthRouteSuffix, …>` from this tuple, so a fifth suffix with no handler and a handler
// with no suffix BOTH fail typecheck.
//
// A third, ungated copy of this list lives in README.md's route table. It is prose; nothing binds
// it, and that is a stated residual rather than an oversight.

export const AUTH_ROUTE_SUFFIXES = ['/auth/login', '/auth/callback', '/auth/logout', '/auth/logout/callback'] as const;

export type AuthRouteSuffix = (typeof AUTH_ROUTE_SUFFIXES)[number];
