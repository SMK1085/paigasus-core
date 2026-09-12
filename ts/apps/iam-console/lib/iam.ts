// SPDX-License-Identifier: Apache-2.0
//
// The session-bound IAM clients (spec § 4.3). Every accessor is a React cache(), so it runs once
// per request and never longer: a client holds a bearer token, and a token must not outlive its
// request (ts/packages/paigasus-sdk/src/iam.ts:23-28).
//
// NOTE: outside a React server render (vitest, a plain script) cache() does NOT memoize — React's
// default build exports a pass-through (measured on react 19.2.8). Tests must not rely on it.
import 'server-only';
import { cache } from 'react';
import { requireSession, type ResolvedSession } from '@paigasus/auth/server';
import { authRuntime } from './auth';
import { requestCorrelationId } from './correlation';
import { iamClientsForToken, type IamClients } from './iam-clients';

export { createIamClients, iamClientsForToken } from './iam-clients';
export type { IamClients } from './iam-clients';

/** The session for this request. Redirects to login when there is none. */
export const currentSession: () => Promise<ResolvedSession> = cache(async () => requireSession(await authRuntime()));

/** The five IAM clients for this request, bound to the session's access token and correlation id. */
export const iamClients: () => Promise<IamClients> = cache(async () => {
  const session = await currentSession();
  return iamClientsForToken(session.accessToken, await requestCorrelationId());
});

/** The session's access token — the bearer that @paigasus/discovery probes with. */
export const sessionToken: () => Promise<string> = cache(async () => (await currentSession()).accessToken);
