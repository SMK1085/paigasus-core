// SPDX-License-Identifier: Apache-2.0
//
// @paigasus/auth's PrincipalResolver port, implemented over IAM (spec § 4.5). The login callback
// calls it once (ts/packages/paigasus-auth/src/http/routes.ts:293), with the new access token,
// and the resolver itself makes ONE IAM call (SMA-632): WhoAmI is bearer-enforced, so it
// provisions the caller as a side effect and there is nothing left to retry.
//
// It lives in @paigasus/console-core, not in @paigasus/auth, because @paigasus/auth must not import
// @paigasus/sdk (ts/packages/paigasus-auth/src/ports/principal-resolver.ts:3-8), and the sdk
// boundary rule bans every @paigasus/* import except proto. So neither of those two packages can
// hold a resolver that needs both.
//
// IT NEVER FAILS THE LOGIN. On any failure it returns principalPrn: null, empty lists and
// grantsAvailable: false. An IAM answer that is a failure logs `principal.resolve_failed`; a
// thrown error — a bug in this function, not an IAM answer — logs `principal.resolve_crashed`
// instead, so the two causes stay distinguishable in the log. The pages do not read this snapshot
// (they call currentPrincipal()), so a degraded login costs nothing after the first render.
//
// THIS IS THE ONE PRODUCT OF THIS PACKAGE THAT LIVES ON SHARED STATE (SMA-662). The app's
// lib/auth.ts hands this resolver to getAuthRuntime, whose singleton lives on globalThis under a
// key carrying the ZONE (@paigasus/auth's runtime.ts:203) — one per zone per process, not one per
// process. Its FIRST call fixes the resolver for the life of that zone, so one copy's resolver
// serves requests the other copy handles. It
// is safe because the closure supplies BOTH sides: deps.clientsForToken builds the client and
// callIam below classifies its errors, and both belong to the copy that built this resolver. The
// two `err instanceof Error` reads in the catch test a BUILTIN, which both copies share.
import 'server-only';
import type { PrincipalResolver, ResolvedPrincipal } from '@paigasus/auth/server';
import { principalPrnOf } from './principal-prn';
import type { ConsoleLogger } from './logger';
import { callIam } from './errors';
import type { IamClients } from './iam-clients';

/** The user waits on the login callback, so each call gets 3 s, not the SDK's 10 s (transport.ts:42). */
const DEFAULT_TIMEOUT_MS = 3_000;

export function createPrincipalResolver(deps: {
  /**
   * May be async: the app's `lib/auth.ts` reads the request's correlation id here, so the login callback's IAM
   * calls join the id proxy.ts minted. A factory that throws — or a promise that rejects — is a bug,
   * not an IAM answer, so it lands in the `resolve_crashed` catch below like any other throw.
   */
  clientsForToken: (token: string) => Pick<IamClients, 'authn'> | Promise<Pick<IamClients, 'authn'>>;
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
      // The try is broad ON PURPOSE (spec § 4.5): it wraps the call AND the response mapping.
      // By the time this runs, the login callback has already spent the single-use OIDC code, so a
      // throw here would strand the user with no way to retry — a degraded login is the right
      // outcome whether IAM refused or this function has a bug. The two causes still need to read
      // apart in the log, so they use different events: a `callIam` result of `ok: false` is an IAM
      // ANSWER and logs `principal.resolve_failed` with its presentation, from inside the try below.
      // Anything that instead THROWS out of the try — a bug, not an IAM answer — is caught here and
      // logs `principal.resolve_crashed`, so an operator can tell a console bug from an IAM outage.
      try {
        const clients = await deps.clientsForToken(accessToken);
        // ONE bearer-enforced call (SMA-632). WhoAmI provisions the caller because enforcement
        // covers it, so the GetServiceInfo-then-Introspect sequence this used to run is gone.
        const answer = await callIam(() => clients.authn.whoAmI({}, { timeoutMs }));
        if (!answer.ok) return degraded(answer.error.presentation);
        const me = answer.value;
        // IAM now reports role grants here (authenticate_token.rs:170-181, SMA-633), but this
        // resolver still discards them: consuming `me.roleGrants` and gating on the
        // `iam.authn.grants` capability is a deliberate follow-up, not this branch (spec D1).
        return {
          // The SAME reading as the live path (principal-prn.ts). The two used to disagree.
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
