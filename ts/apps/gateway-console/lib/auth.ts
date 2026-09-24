// SPDX-License-Identifier: Apache-2.0
//
// The auth composition root (spec § 4.2). getAuthRuntime is a PROCESS singleton that returns a
// Promise (ts/packages/paigasus-auth/src/runtime.ts:185), and its first call fixes the resolver and
// the logger for the life of the process. Nothing here runs at module scope.
import 'server-only';
import { getAuthRuntime, type AuthRuntime } from '@paigasus/auth/server';
import { getRuntimeConfig } from './config';
import { createPrincipalResolver, logger, requestCorrelationId } from '@paigasus/console-core';
import { iamClientsForToken } from './console';

/**
 * The runtime is a process singleton, but `clientsForToken` runs PER REQUEST, inside the login
 * callback's route handler. So it reads the correlation id there, and the login-time IAM calls
 * (WhoAmI) carry the same id as every later call of that request. proxy.ts sets
 * the header on `/auth/callback` too — the route is public, which makes the middleware allow it, not
 * skip the header. Outside a request scope `requestCorrelationId()` answers null, never throws.
 */
// This module and ./console import from each other (this function from ./console, and ./console
// imports authRuntime from here). What makes that cycle safe is ./console's call site, not the
// declaration form here: it passes `authRuntime: () => authRuntime()`, a fresh arrow that
// resolves the `authRuntime` identifier at CALL time rather than capturing the binding at
// module-evaluation time. Next gives a route handler and a page SEPARATE module graphs
// (CLAUDE.md, SMA-511), so both entry orders occur in production, and without that wrapper one
// order would hit a TDZ ReferenceError if this were ever changed to `export const authRuntime =
// () => ...`.
export function authRuntime(): Promise<AuthRuntime> {
  return getAuthRuntime(getRuntimeConfig(), {
    logger,
    resolver: createPrincipalResolver({ clientsForToken: async (token) => iamClientsForToken(token, await requestCorrelationId()), logger }),
  });
}
