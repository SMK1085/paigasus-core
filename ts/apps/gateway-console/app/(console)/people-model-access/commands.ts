// SPDX-License-Identifier: Apache-2.0
//
// The two commands of "Model access for people" (SMA-676 spec § 4.4, § 5.2, § 5.3). Pure and
// dependency-injected; they take NO mayI: IAM decides (§ 6).
//
// The role key is the server constant GATEWAY_ROLE, so a form cannot grant another role. The grant
// does NOT check that the principal is a candidate: D12 is a UI choice, and IAM already lets an
// org_admin grant to any principal with a direct call (§ 6).
//
// The revoke is BOUNDED (§ 5.3 step 2): ListRoleGrants(principal, org, gateway_user) first, and only
// a grant of that set is revoked. The rows are checked for role, scope and principal too, because an
// OLD IAM ignores the filters and answers every grant of the principal (§ 10).
//
// CANONICAL PRN (controller ruling). `orgPrn` reaches the revoke command as client-controlled form
// text; IAM answers `listRoleGrants` with a CANONICAL PRN (lower-case UUID). A byte comparison of
// the two would refuse a legitimate revoke whenever the form's UUID differs only in case from IAM's
// canonical spelling, and — the security-relevant direction — the bound check exists to compare
// like with like, not to trust the form's spelling. So the command builds its own canonical form of
// `orgPrn`, the same way `orgs/[org]/load.ts` builds one from a URL segment (organizationPrn over
// parseTenancyPrn's fields), and uses that canonical value for both the IAM query and the local
// bound check. A value that does not name an organization at all cannot match any bound grant's
// scope, so it is refused as invalid-input before any IAM call.
import 'server-only';
import { z } from 'zod';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { callIam, neverReachedIam, organizationPrn, parseTenancyPrn, prnField, toActionResult, type ActionResult, type IamClients } from '@paigasus/console-core';
import { GATEWAY_ROLE } from '../gateway-role';

type Authz = IamClients['authz'];

export const grantModelAccessForm = z.object({ principalPrn: prnField, orgPrn: prnField });
export type GrantModelAccessInput = z.infer<typeof grantModelAccessForm>;

/** zod 4's z.uuid() is RFC-strict; IAM mints UUIDv7 grant ids, which are RFC 4122 ids. */
export const revokeModelAccessForm = z.object({ principalPrn: prnField, orgPrn: prnField, grantId: z.string().trim().pipe(z.uuid()) });
export type RevokeModelAccessInput = z.infer<typeof revokeModelAccessForm>;

/** The form named a grant that is not this person's model access at this org. */
function notThisGrant(): PaigasusError {
  return neverReachedIam({ presentation: 'invalid-input', message: 'The grant is not model access of this person at this organization.', transport: { kind: 'http', status: 400 } });
}

/**
 * The canonical form of an organization PRN, whatever case the form used for its UUID: the same
 * build `orgs/[org]/load.ts` uses for a URL segment. Null when `orgPrn` does not name an
 * organization at all.
 */
function canonicalOrgPrn(orgPrn: string): string | null {
  const ref = parseTenancyPrn(orgPrn);
  return ref?.kind === 'organization' ? organizationPrn(ref.orgId) : null;
}

/** § 5.2. The idempotent case (D9) is a success too: IAM returns the existing grant. */
export async function grantModelAccess(deps: { readonly authz: Pick<Authz, 'grantRole'> }, input: GrantModelAccessInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.authz.grantRole({ principalPrn: input.principalPrn, roleKey: GATEWAY_ROLE, scopePrn: input.orgPrn })));
}

/**
 * § 5.3. An empty model-access set means the grant is gone (another admin revoked first): success,
 * no call. A set that does not hold the id means a crafted form: invalid-input, no call. A not-found
 * from RevokeRole is a race with another admin: success.
 */
export async function revokeModelAccess(deps: { readonly authz: Pick<Authz, 'listRoleGrants' | 'revokeRole'> }, input: RevokeModelAccessInput): Promise<ActionResult> {
  const orgPrn = canonicalOrgPrn(input.orgPrn);
  if (orgPrn === null) return { ok: false, error: notThisGrant() };
  const listed = await callIam(() => deps.authz.listRoleGrants({ principalPrn: input.principalPrn, scopePrn: orgPrn, roleKey: GATEWAY_ROLE }));
  if (!listed.ok) return { ok: false, error: listed.error };
  const access = listed.value.grants.filter((grant) => grant.roleKey === GATEWAY_ROLE && grant.scopePrn === orgPrn && grant.principalPrn === input.principalPrn);
  if (access.length === 0) return { ok: true };
  if (!access.some((grant) => grant.id === input.grantId)) return { ok: false, error: notThisGrant() };
  const revoked = await callIam(() => deps.authz.revokeRole({ id: input.grantId }));
  if (!revoked.ok && revoked.error.presentation === 'not-found') return { ok: true };
  return toActionResult(revoked);
}
