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
import { getSession, requireSession, type ResolvedSession } from '@paigasus/auth/server';
import { authRuntime } from './auth';
import { requestCorrelationId } from './correlation';
import type { IamResult } from './errors';
import { sessionExpired } from './form';
import { iamClientsForToken, type IamClients } from './iam-clients';

export type { IamClients } from './iam-clients';

/** The session for this request. Redirects to login when there is none. PAGE RENDERS ONLY. */
export const currentSession: () => Promise<ResolvedSession> = cache(async () => requireSession(await authRuntime()));

/** The five IAM clients for this request, bound to the session's access token and correlation id. */
export const iamClients: () => Promise<IamClients> = cache(async () => {
  const session = await currentSession();
  return iamClientsForToken(session.accessToken, await requestCorrelationId());
});

/** The session for this request, WITHOUT a redirect. `null` when there is none. */
export const optionalSession: () => Promise<ResolvedSession | null> = cache(async () => getSession(await authRuntime()));

/**
 * The five IAM clients for a SERVER ACTION. It returns a `relogin` failure instead of redirecting.
 *
 * WHY AN ACTION MUST NOT REDIRECT HERE. `requireSession()` redirects basePath-RELATIVE, which a
 * page render needs, because Next's app render adds the basePath itself. A Server Action takes a
 * different path through Next: `next/dist/server/app-render/action-handler.js:261` writes the RAW
 * url into the `x-action-redirect` header, and `:906` writes the RAW url into `Location` for a
 * no-JS post. Only the internal RSC pre-fetch at `:267` adds the basePath. The browser then
 * resolves the raw value against the current URL and hard-navigates, so a user whose session
 * expired before pressing "Create" would land on `https://<host>/auth/login`, OUTSIDE the `/iam`
 * zone, where nothing serves a login route.
 *
 * The failure this returns carries `presentation: 'relogin'`, so `FormError` renders the
 * `SignInAgain` link, which builds `${basePath}${pathname}` itself and stays in the zone (spec
 * § 6.4: relogin is a link, never an automatic redirect).
 *
 * The action must also return BEFORE its `revalidatePath` call. Next skips the post-action page
 * render when the action revalidated nothing (`action-handler.js:990`), so the `(console)` layout
 * — which does call `requireSession()` — never runs for this response and cannot redirect either.
 */
export async function iamClientsForAction(): Promise<IamResult<IamClients>> {
  const session = await optionalSession();
  if (session === null) return { ok: false, error: sessionExpired() };
  return { ok: true, value: iamClientsForToken(session.accessToken, await requestCorrelationId()) };
}

/** The session's access token — the bearer that @paigasus/discovery probes with. */
export const sessionToken: () => Promise<string> = cache(async () => (await currentSession()).accessToken);
