// SPDX-License-Identifier: Apache-2.0
//
// createConsoleRuntime() composes this package's per-request accessors behind ONE factory (spec
// § 4.2-4.6, SMA-512 PR 2, task 5). Before this task, each accessor was a module-level `cache()`
// export reading a package-wide "ports" singleton (`runtime-ports.ts`, now deleted) that an app had
// to set once, separately, before any accessor ran — a step a refactor could silently drop (that
// exact regression shipped once; see the app's `tests/unit/lib-console-composition.test.ts`). Now
// the app builds one runtime with one call and every accessor closes over `deps` directly: there is
// no separate step left to forget.
//
// CALL THIS EXACTLY ONCE PER APP, AT MODULE SCOPE. Each accessor below (apart from
// `iamClientsForToken` and `iamClientsForAction`, see their own comments) is a React `cache()`
// wrapper, and `cache()` memoizes by the WRAPPER's identity, not by its arguments. A second
// `createConsoleRuntime()` call therefore makes a second, independent memoization identity for the
// same request: every accessor built from it runs its live call again — a second `Introspect`, a
// second `ListRoleGrants` walk (up to 50 tenancy reads), a second discovery probe. Outside a React
// server render `cache()` is a pass-through (measured on react 19.2.8), so no vitest tier can
// observe two calls behaving differently from one — only an e2e `Introspect` count can (PR 3).
import 'server-only';
import { cache } from 'react';
import { after } from 'next/server';
import { getSession, requireSession, type AuthRuntime, type ResolvedSession } from '@paigasus/auth/server';
import type { Discovery } from '@paigasus/discovery/server';
import type { ConsoleCoreConfig } from './config-shape';
import type { ConsoleLogger } from './logger';
import { sessionExpired, type IamResult } from './errors';
import { createIamClients, type IamClients } from './iam-clients';
import { requestCorrelationId } from './correlation';
import { whoAmI, type Principal } from './principal';
import { createMayI, type MayI } from './authorize';
import { cedarCapabilityOf, loadMyScopes, type MyScopes } from './scopes';
import { createAppDiscovery } from './discovery';

export type ConsoleRuntime = {
  currentSession: () => Promise<ResolvedSession>;
  optionalSession: () => Promise<ResolvedSession | null>;
  sessionToken: () => Promise<string>;
  iamClients: () => Promise<IamClients>;
  iamClientsForAction: () => Promise<IamResult<IamClients>>;
  iamClientsForToken: (token: string, correlationId?: string | null) => IamClients;
  currentPrincipal: () => Promise<IamResult<Principal>>;
  mayI: () => Promise<MayI>;
  myScopes: () => Promise<IamResult<MyScopes>>;
  discovery: () => Discovery;
};

export function createConsoleRuntime(deps: { config: () => ConsoleCoreConfig; authRuntime: () => Promise<AuthRuntime>; logger: ConsoleLogger }): ConsoleRuntime {
  /**
   * The clients for a token the caller already holds, over the app's configured IAM gRPC address.
   * No session lookup — used by `iamClients`/`iamClientsForAction` below and by the login
   * callback's resolver factory (the app's `lib/auth.ts`). NOT a `cache()`: it is parameterized by
   * `token`, and every call site already runs behind its own per-request memoization.
   */
  const iamClientsForToken = (token: string, correlationId: string | null = null): IamClients => createIamClients({ baseUrl: deps.config().PAIGASUS_IAM_GRPC_URL, token, correlationId });

  /** The session for this request. Redirects to login when there is none. PAGE RENDERS ONLY. */
  const currentSession: () => Promise<ResolvedSession> = cache(async () => requireSession(await deps.authRuntime()));

  /** The session for this request, WITHOUT a redirect. `null` when there is none. */
  const optionalSession: () => Promise<ResolvedSession | null> = cache(async () => getSession(await deps.authRuntime()));

  /** The six IAM clients for this request, bound to the session's access token and correlation id. */
  const iamClients: () => Promise<IamClients> = cache(async () => {
    const session = await currentSession();
    return iamClientsForToken(session.accessToken, await requestCorrelationId());
  });

  /** The session's access token — the bearer that @paigasus/discovery probes with. */
  const sessionToken: () => Promise<string> = cache(async () => (await currentSession()).accessToken);

  /**
   * The six IAM clients for a SERVER ACTION. Deliberately NOT a `cache()`: it returns a `relogin`
   * failure rather than redirecting, so a Server Action can render an inline error.
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
  const iamClientsForAction = async (): Promise<IamResult<IamClients>> => {
    const session = await optionalSession();
    if (session === null) return { ok: false, error: sessionExpired() };
    return { ok: true, value: iamClientsForToken(session.accessToken, await requestCorrelationId()) };
  };

  /** Per request: a LIVE WhoAmI. Bearer enforcement provisions the caller, so there is no retry. */
  const currentPrincipal: () => Promise<IamResult<Principal>> = cache(async () => whoAmI(await iamClients()));

  /** One MayI per request, so the memo lives exactly one request. */
  const mayI: () => Promise<MayI> = cache(async () => {
    const [principal, clients] = await Promise.all([currentPrincipal(), iamClients()]);
    return createMayI({ authz: clients.authz, principalPrn: principal.ok ? principal.value.prn : null, logger: deps.logger });
  });

  /** One handle per request; background revalidation runs after the response, through after(). */
  const discovery: () => Discovery = cache(() => createAppDiscovery({ config: deps.config(), log: deps.logger, waitUntil: (p) => after(p) }));

  /** One per request. The page and the switcher share it (spec § 5.1). */
  const myScopes: () => Promise<IamResult<MyScopes>> = cache(async () => {
    const principal = await currentPrincipal();
    if (!principal.ok) return principal;
    const [clients, token] = await Promise.all([iamClients(), sessionToken()]);
    const iam = await discovery().getServiceState('iam', token);
    return { ok: true, value: await loadMyScopes({ tenancy: clients.tenancy, authz: clients.authz, principal: principal.value, cedarCapability: cedarCapabilityOf(iam) }) };
  });

  return { currentSession, optionalSession, sessionToken, iamClients, iamClientsForAction, iamClientsForToken, currentPrincipal, mayI, myScopes, discovery };
}
