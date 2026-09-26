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
// `orgPrn` and `principalPrn` reach the revoke command as client-controlled form text, while IAM
// answers `listRoleGrants` with CANONICAL PRNs (lower-case UUID). A byte comparison of the form's
// text against IAM's canonical answer has two failure modes: it refuses a legitimate revoke when
// the form's UUID case differs from IAM's spelling (a false refusal), and it reports a legitimate
// revoke as a no-op "already gone" success when none of the rows match the form's spelling even
// though the grant is there (a false "gone" result — the caller sees `{ ok: true }` and believes
// the revoke ran, but nothing was revoked). So the command builds its own canonical form of both
// PRNs before the bound check, the same way `orgs/[org]/load.ts` builds a canonical org PRN from a
// URL segment, and uses the canonical values for the IAM query and the local comparison. A value
// that does not name an organization, or does not name an IAM principal, at all cannot match any
// bound grant, so it is refused as invalid-input before any IAM call.
import 'server-only';
import { z } from 'zod';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { PrincipalKind } from '@paigasus/sdk/iam/types';
import {
  callIam,
  neverReachedIam,
  organizationPrn,
  parsePrincipalPrn,
  parseTenancyPrn,
  principalPrn as buildPrincipalPrn,
  prnField,
  toActionResult,
  type ActionResult,
  type IamClients,
} from '@paigasus/console-core';
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

/**
 * The canonical form of a principal PRN, whatever case the form used for its UUID. Null when `prn`
 * does not name an IAM principal at all.
 */
function canonicalPrincipalPrn(prn: string): string | null {
  const ref = parsePrincipalPrn(prn);
  return ref === null ? null : buildPrincipalPrn(ref.id);
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
  const principalPrn = canonicalPrincipalPrn(input.principalPrn);
  if (principalPrn === null) return { ok: false, error: notThisGrant() };
  const listed = await callIam(() => deps.authz.listRoleGrants({ principalPrn, scopePrn: orgPrn, roleKey: GATEWAY_ROLE, principalKind: PrincipalKind.USER }));
  if (!listed.ok) return { ok: false, error: listed.error };
  const access = listed.value.grants.filter((grant) => grant.roleKey === GATEWAY_ROLE && grant.scopePrn === orgPrn && grant.principalPrn === principalPrn);
  if (access.length === 0) return { ok: true };
  if (!access.some((grant) => grant.id === input.grantId)) return { ok: false, error: notThisGrant() };
  const revoked = await callIam(() => deps.authz.revokeRole({ id: input.grantId }));
  if (!revoked.ok && revoked.error.presentation === 'not-found') return { ok: true };
  return toActionResult(revoked);
}
