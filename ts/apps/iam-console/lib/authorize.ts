// SPDX-License-Identifier: Apache-2.0
//
// mayI() — the affordance check (spec § 4.6, § 6.3, decision D5). It asks IAM's IsAuthorized about
// the CURRENT principal. A principal may always ask about itself
// (rs/crates/services/paigasus-iam/src/application/authorize.rs:75-80), and IsAuthorized has no
// capability gate (adapters/grpc/authz.rs:81-91). Cedar evaluates the request, so the answer
// follows the resource hierarchy and every custom policy. The app holds NO table of which role
// grants which action.
//
// COSMETIC ONLY. mayI() may hide an affordance. No page and no Server Action refuses anything
// because mayI() said no: every user action calls IAM, and IAM decides.
//
// IT FAILS OPEN. A failed query answers true and logs `authorize.query_failed`: a button that
// should be hidden then shows, and IAM still denies the action. A null principal (IAM could not
// say who this is) also answers true, for the same reason — and logs `authorize.no_principal`, so
// that answer is never SILENT (review, defect 1). It reaches here two ways, and both are covered:
// a failed Introspect, and an Introspect that succeeded but named no principal (lib/principal-prn.ts).
// The second used to arrive as the empty string, which is not null: every affordance then asked IAM
// `isAuthorized({ principalPrn: '' })`, got InvalidArgument, and rendered anyway.
import 'server-only';
import { cache } from 'react';
import type { Client } from '@connectrpc/connect';
import type { AuthorizationService } from '@paigasus/sdk/iam';
import { callIam } from './errors';
import { iamClients } from './iam';
import { logger, type ConsoleLogger } from './logger';
import { currentPrincipal } from './principal';

/** The PascalCase names IAM's Action::parse accepts (rs/crates/libs/paigasus-iam-core/src/authz/action.rs:114-163). */
export type IamAction = 'ListOrganizations' | 'CreateOrganization' | 'CreateTeam' | 'CreateProject' | 'AttachMembership' | 'DetachMembership' | 'ListAuditLog';

export type MayI = (action: IamAction, resourcePrn: string) => Promise<boolean>;

export function createMayI(deps: { authz: Pick<Client<typeof AuthorizationService>, 'isAuthorized'>; principalPrn: string | null; logger: Pick<ConsoleLogger, 'appEvent'> }): MayI {
  const memo = new Map<string, Promise<boolean>>();

  const ask = async (principalPrn: string, action: IamAction, resourcePrn: string): Promise<boolean> => {
    const answer = await callIam(() => deps.authz.isAuthorized({ principalPrn, action, resourcePrn }));
    if (answer.ok) return answer.value.allowed;
    deps.logger.appEvent('authorize.query_failed', { action, presentation: answer.error.presentation });
    return true;
  };

  /** No principal to ask about: fail open like a failed query, and say so in the log. */
  const unnamed = (action: IamAction): Promise<boolean> => {
    deps.logger.appEvent('authorize.no_principal', { action });
    return Promise.resolve(true);
  };

  return (action, resourcePrn) => {
    const principalPrn = deps.principalPrn;
    // The memo covers the unnamed branch too, so a page asking the same question twice logs once.
    // The NUL separator cannot occur in an action name or a PRN, so two pairs never share a key.
    const key = `${action}\u0000${resourcePrn}`;
    let pending = memo.get(key);
    if (pending === undefined) {
      pending = principalPrn === null ? unnamed(action) : ask(principalPrn, action, resourcePrn);
      memo.set(key, pending);
    }
    return pending;
  };
}

/** One MayI per request, so the memo lives exactly one request. */
export const mayI: () => Promise<MayI> = cache(async () => {
  const [principal, clients] = await Promise.all([currentPrincipal(), iamClients()]);
  return createMayI({ authz: clients.authz, principalPrn: principal.ok ? principal.value.prn : null, logger });
});
