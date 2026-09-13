// SPDX-License-Identifier: Apache-2.0
//
// The auth composition root (spec § 4.2). getAuthRuntime is a PROCESS singleton that returns a
// Promise (ts/packages/paigasus-auth/src/runtime.ts:185), and its first call fixes the resolver and
// the logger for the life of the process. Nothing here runs at module scope.
import 'server-only';
import { getAuthRuntime, type AuthRuntime } from '@paigasus/auth/server';
import { getRuntimeConfig } from './config';
import { requestCorrelationId } from './correlation';
import { iamClientsForToken } from './iam-clients';
import { logger } from './logger';
import { createIntrospectPrincipalResolver } from './principal-resolver';

/**
 * The session cookie's name. @paigasus/auth does not export it from any entry an app may import,
 * so it is written here once; tests/unit/session-cookie.test.ts proves that @paigasus/auth's own
 * middleware treats exactly this name as the session.
 */
export const SESSION_COOKIE_NAME = '__Host-pgs_sid';

/**
 * The runtime is a process singleton, but `clientsForToken` runs PER REQUEST, inside the login
 * callback's route handler. So it reads the correlation id there, and the login-time IAM calls
 * (GetServiceInfo, Introspect) carry the same id as every later call of that request. proxy.ts sets
 * the header on `/auth/callback` too — the route is public, which makes the middleware allow it, not
 * skip the header. Outside a request scope `requestCorrelationId()` answers null, never throws.
 */
export function authRuntime(): Promise<AuthRuntime> {
  return getAuthRuntime(getRuntimeConfig(), {
    logger,
    resolver: createIntrospectPrincipalResolver({ clientsForToken: async (token) => iamClientsForToken(token, await requestCorrelationId()), logger }),
  });
}
