// SPDX-License-Identifier: Apache-2.0
//
// @paigasus/auth's PrincipalResolver port, implemented over IAM (spec § 4.5). The login callback
// calls it once (ts/packages/paigasus-auth/src/http/routes.ts:236), with the new access token.
//
// It lives in the APP because @paigasus/auth must not import @paigasus/sdk
// (ts/packages/paigasus-auth/src/ports/principal-resolver.ts:3-8), and the sdk boundary rule bans
// every @paigasus/* import except proto. SMA-631 records a shared home for SMA-512.
//
// IT NEVER FAILS THE LOGIN. On any failure it returns principalPrn: null, empty lists and
// grantsAvailable: false. An IAM answer that is a failure logs `principal.resolve_failed`; a
// thrown error — a bug in this function, not an IAM answer — logs `principal.resolve_crashed`
// instead, so the two causes stay distinguishable in the log. The pages do not read this snapshot
// (they call currentPrincipal()), so a degraded login costs nothing after the first render.
import 'server-only';
import type { PrincipalResolver, ResolvedPrincipal } from '@paigasus/auth/server';
import { callIam } from './errors';
import type { IamClients } from './iam-clients';
import type { ConsoleLogger } from './logger';
import { principalPrnOf } from './principal-prn';

/** The user waits on the login callback, so each call gets 3 s, not the SDK's 10 s (transport.ts:42). */
const DEFAULT_TIMEOUT_MS = 3_000;

export function createIntrospectPrincipalResolver(deps: {
  /**
   * May be async: lib/auth.ts reads the request's correlation id here, so the login callback's IAM
   * calls join the id proxy.ts minted. A factory that throws — or a promise that rejects — is a bug,
   * not an IAM answer, so it lands in the `resolve_crashed` catch below like any other throw.
   */
  clientsForToken: (token: string) => Pick<IamClients, 'authn' | 'serviceInfo'> | Promise<Pick<IamClients, 'authn' | 'serviceInfo'>>;
  logger: ConsoleLogger;
  timeoutMs?: number;
}): PrincipalResolver {
  const timeoutMs = deps.timeoutMs ?? DEFAULT_TIMEOUT_MS;

  return {
    async resolve({ accessToken, idTokenClaims }): Promise<ResolvedPrincipal> {
      const degradedPrincipal = (): ResolvedPrincipal => ({
        principalPrn: null,
        issuer: idTokenClaims.iss,
        subject: idTokenClaims.sub,
        memberships: [],
        roleGrants: [],
        grantsAvailable: false,
      });
      const degraded = (presentation: string): ResolvedPrincipal => {
        deps.logger.appEvent('principal.resolve_failed', { presentation });
        return degradedPrincipal();
      };
      // The try is broad ON PURPOSE (spec § 4.5): it wraps the two calls AND the response mapping.
      // By the time this runs, the login callback has already spent the single-use OIDC code, so a
      // throw here would strand the user with no way to retry — a degraded login is the right
      // outcome whether IAM refused or this function has a bug. The two causes still need to read
      // apart in the log, so they use different events: a `callIam` result of `ok: false` is an IAM
      // ANSWER and logs `principal.resolve_failed` with its presentation, from inside the try below.
      // Anything that instead THROWS out of the try — a bug, not an IAM answer — is caught here and
      // logs `principal.resolve_crashed`, so an operator can tell a console bug from an IAM outage.
      try {
        const clients = await deps.clientsForToken(accessToken);
        // 1. The provisioning call, with the NEW token as the bearer.
        const provisioned = await callIam(() => clients.serviceInfo.getServiceInfo({}, { timeoutMs }));
        if (!provisioned.ok) return degraded(provisioned.error.presentation);
        // 2. Introspect reads the token from the REQUEST field (authn.rs:59), not from the header.
        const answer = await callIam(() => clients.authn.introspect({ token: accessToken }, { timeoutMs }));
        if (!answer.ok) return degraded(answer.error.presentation);
        const me = answer.value;
        // 3. IAM reports no role grants here (authenticate_token.rs:161-165), and the port says
        //    to treat that as UNKNOWN (ports/principal-resolver.ts:32-36).
        return {
          // The SAME reading as the live path (lib/principal-prn.ts). The two used to disagree.
          principalPrn: principalPrnOf(me.principalPrn),
          issuer: me.issuer,
          subject: me.subject,
          memberships: me.memberships.map((m) => ({ id: m.id, principalPrn: m.principalPrn, nodePrn: m.nodePrn })),
          roleGrants: [],
          grantsAvailable: false,
        };
      } catch (err) {
        // callIam rethrows everything that is not a ConnectError, and a bug in the mapping above
        // throws too. Neither is an IAM answer, so neither is `principal.resolve_failed`. No token,
        // DSN or cookie is logged here — only the error's own shape.
        const name = err instanceof Error ? err.constructor.name : typeof err;
        const message = err instanceof Error ? err.message : String(err);
        deps.logger.appEvent('principal.resolve_crashed', { name, message });
        return degradedPrincipal();
      }
    },
  };
}
