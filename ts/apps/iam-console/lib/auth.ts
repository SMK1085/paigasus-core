// SPDX-License-Identifier: Apache-2.0
//
// The auth composition root (spec § 4.2). getAuthRuntime is a PROCESS singleton that returns a
// Promise (ts/packages/paigasus-auth/src/runtime.ts:185), and its first call fixes the resolver and
// the logger for the life of the process. Nothing here runs at module scope.
//
// Task 13 adds the Introspect principal resolver. Until then the package's claims resolver runs,
// which reports principalPrn: null (ts/packages/paigasus-auth/src/adapters/claims-resolver.ts).
import 'server-only';
import { getAuthRuntime, type AuthRuntime } from '@paigasus/auth/server';
import { getRuntimeConfig } from './config';
import { logger } from './logger';

/**
 * The session cookie's name. @paigasus/auth does not export it from any entry an app may import,
 * so it is written here once; tests/unit/session-cookie.test.ts proves that @paigasus/auth's own
 * middleware treats exactly this name as the session.
 */
export const SESSION_COOKIE_NAME = '__Host-pgs_sid';

export function authRuntime(): Promise<AuthRuntime> {
  return getAuthRuntime(getRuntimeConfig(), { logger });
}
