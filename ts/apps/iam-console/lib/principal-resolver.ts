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
// grantsAvailable: false, and logs `principal.resolve_failed`. The pages do not read this snapshot
// (they call currentPrincipal()), so a degraded login costs nothing after the first render.
import 'server-only';
import type { PrincipalResolver, ResolvedPrincipal } from '@paigasus/auth/server';
import { callIam } from './errors';
import type { IamClients } from './iam-clients';
import type { ConsoleLogger } from './logger';

/** The user waits on the login callback, so each call gets 3 s, not the SDK's 10 s (transport.ts:42). */
const DEFAULT_TIMEOUT_MS = 3_000;

export function createIntrospectPrincipalResolver(deps: {
  clientsForToken: (token: string) => Pick<IamClients, 'authn' | 'serviceInfo'>;
  logger: ConsoleLogger;
  timeoutMs?: number;
}): PrincipalResolver {
  const timeoutMs = deps.timeoutMs ?? DEFAULT_TIMEOUT_MS;

  return {
    async resolve({ accessToken, idTokenClaims }): Promise<ResolvedPrincipal> {
      const degraded = (presentation: string): ResolvedPrincipal => {
        deps.logger.appEvent('principal.resolve_failed', { presentation });
        return { principalPrn: null, issuer: idTokenClaims.iss, subject: idTokenClaims.sub, memberships: [], roleGrants: [], grantsAvailable: false };
      };
      // The whole sequence, the mapping included, runs inside one try (spec § 4.5).
      try {
        const clients = deps.clientsForToken(accessToken);
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
          principalPrn: me.principalPrn === '' ? null : me.principalPrn,
          issuer: me.issuer,
          subject: me.subject,
          memberships: me.memberships.map((m) => ({ id: m.id, principalPrn: m.principalPrn, nodePrn: m.nodePrn })),
          roleGrants: [],
          grantsAvailable: false,
        };
      } catch {
        // callIam rethrows everything that is not a ConnectError. Here nothing may escape.
        return degraded('generic');
      }
    },
  };
}
