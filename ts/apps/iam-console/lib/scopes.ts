// SPDX-License-Identifier: Apache-2.0
//
// "Your organizations" (spec § 5.1). Only platform_admin can call ListOrganizations or list
// memberships by principal, and a membership is not a grant: CreateOrganization gives the creator
// an org_admin grant and no membership, and a user can hold a team or project role with no
// organization access. So the scopes are the union of the principal's memberships and, when IAM
// reports `iam.authz.cedar`, the scopes of its OWN role grants (a principal may always list its own
// grants; the RPC needs the capability).
import 'server-only';
import { cache } from 'react';
import type { ServiceState } from '@paigasus/discovery/types';
import { discovery } from './discovery';
import { callIam, type IamResult } from './errors';
import { iamClients, sessionToken, type IamClients } from './iam';
import { currentPrincipal, type Principal } from './principal';
import { organizationPrn, parseTenancyPrn, projectPrn, teamPrn, type TenancyRef } from './prn-tenancy';

export type ScopeEntry =
  | { kind: 'organization'; prn: string; orgId: string; label: string | null; denied: boolean }
  | { kind: 'team'; prn: string; orgId: string; teamId: string; label: string | null; denied: boolean }
  | { kind: 'project'; prn: string; orgId: string; teamId: string | null; projectId: string; label: string | null; denied: boolean };

export type MyScopes = { entries: ScopeEntry[]; hiddenCount: number; grantsListed: boolean };

/** The page shows at most this many scopes and states how many more exist (spec § 5.1 step 2). */
export const SCOPE_CAP = 50;

const GRANT_PAGE_SIZE = 200;
/** A bound on the grant walk: 2000 grants. A principal with more sees the first 2000 only. */
const GRANT_MAX_PAGES = 10;
const KIND_ORDER: Readonly<Record<TenancyRef['kind'], number>> = { organization: 0, team: 1, project: 2 };

type Scope = { readonly prn: string; readonly ref: TenancyRef; readonly orgId: string; readonly id: string };

function compare(a: string, b: string): number {
  if (a < b) return -1;
  return a > b ? 1 : 0;
}

/** Null when any page fails: the page then says "memberships only" (spec § 5.1 step 1). */
async function listOwnGrantScopes(authz: Pick<IamClients['authz'], 'listRoleGrants'>, principalPrn: string): Promise<string[] | null> {
  const scopes: string[] = [];
  for (let page = 0; page < GRANT_MAX_PAGES; page += 1) {
    const result = await callIam(() => authz.listRoleGrants({ principalPrn, limit: GRANT_PAGE_SIZE, offset: BigInt(page * GRANT_PAGE_SIZE) }));
    if (!result.ok) return null;
    for (const grant of result.value.grants) scopes.push(grant.scopePrn);
    if (result.value.grants.length < GRANT_PAGE_SIZE) break;
  }
  return scopes;
}

/** Parse, drop non-tenancy PRNs (the root, a service account), deduplicate, group by organization. */
function collectScopes(prns: readonly string[]): Scope[] {
  const byKey = new Map<string, Scope>();
  for (const raw of prns) {
    const ref = parseTenancyPrn(raw);
    if (ref === null) continue;
    const id = ref.id.toLowerCase();
    const orgId = (ref.kind === 'organization' ? ref.id : ref.orgId).toLowerCase();
    const key = `${ref.kind}:${id}`;
    if (byKey.has(key)) continue;
    const prn = ref.kind === 'organization' ? organizationPrn(id) : ref.kind === 'team' ? teamPrn(orgId, id) : projectPrn(orgId, id);
    byKey.set(key, { prn, ref, orgId, id });
  }
  return [...byKey.values()].sort((a, b) => compare(a.orgId, b.orgId) || KIND_ORDER[a.ref.kind] - KIND_ORDER[b.ref.kind] || compare(a.id, b.id));
}

/** One call per shown scope. A failed call never fails the page: the row loses its label. */
async function describeScope(tenancy: Pick<IamClients['tenancy'], 'getOrganization' | 'getTeam' | 'getProject'>, scope: Scope): Promise<ScopeEntry> {
  const { prn, orgId, id } = scope;
  if (scope.ref.kind === 'organization') {
    const result = await callIam(() => tenancy.getOrganization({ prn }));
    return { kind: 'organization', prn, orgId, label: result.ok ? (result.value.organization?.name ?? null) : null, denied: !result.ok && result.error.presentation === 'forbidden' };
  }
  if (scope.ref.kind === 'team') {
    // Both UUIDs are in a team PRN, so the row links straight to the team (spec § 5.1 step 3).
    const result = await callIam(() => tenancy.getTeam({ prn }));
    return { kind: 'team', prn, orgId, teamId: id, label: result.ok ? (result.value.team?.name ?? null) : null, denied: !result.ok && result.error.presentation === 'forbidden' };
  }
  // A project PRN holds the org and the project, not the team. GetProject carries team_prn.
  const result = await callIam(() => tenancy.getProject({ prn }));
  const project = result.ok ? result.value.project : undefined;
  const teamRef = project === undefined ? null : parseTenancyPrn(project.teamPrn);
  return {
    kind: 'project',
    prn,
    orgId,
    teamId: teamRef?.kind === 'team' ? teamRef.id.toLowerCase() : null,
    projectId: id,
    label: project?.name ?? null,
    denied: !result.ok && result.error.presentation === 'forbidden',
  };
}

export async function loadMyScopes(deps: {
  tenancy: Pick<IamClients['tenancy'], 'getOrganization' | 'getTeam' | 'getProject'>;
  authz: Pick<IamClients['authz'], 'listRoleGrants'>;
  principal: Principal;
  cedarCapability: boolean;
}): Promise<MyScopes> {
  // An unnamed principal (spec § 4.5; lib/principal-prn.ts) cannot be the subject of ListRoleGrants,
  // so the walk is SKIPPED rather than sent with an empty PRN — which IAM refuses with
  // InvalidArgument, costing a round trip to reach the same "memberships only" page (review,
  // defect 1). The memberships IAM did send are still listed.
  const prn = deps.principal.prn;
  const grantScopes = deps.cedarCapability && prn !== null ? await listOwnGrantScopes(deps.authz, prn) : null;
  const scopes = collectScopes([...deps.principal.memberships.map((membership) => membership.nodePrn), ...(grantScopes ?? [])]);
  const shown = scopes.slice(0, SCOPE_CAP);
  const entries = await Promise.all(shown.map((scope) => describeScope(deps.tenancy, scope)));
  return { entries, hiddenCount: scopes.length - shown.length, grantsListed: grantScopes !== null };
}

/**
 * True when myScopes() may list the principal's own role grants: IAM is available and reports
 * `iam.authz.cedar`. Any other state selects "memberships only" (spec § 5.1 step 1). A degraded IAM
 * selects it too, even when its last descriptor listed the capability.
 */
export function cedarCapabilityOf(state: ServiceState): boolean {
  return state.state === 'available' && state.capabilities.includes('iam.authz.cedar');
}

/** One per request. The page and the switcher share it (spec § 5.1). */
export const myScopes = cache(async (): Promise<IamResult<MyScopes>> => {
  const principal = await currentPrincipal();
  if (!principal.ok) return principal;
  const [clients, token] = await Promise.all([iamClients(), sessionToken()]);
  const iam = await discovery().getServiceState('iam', token);
  return { ok: true, value: await loadMyScopes({ tenancy: clients.tenancy, authz: clients.authz, principal: principal.value, cedarCapability: cedarCapabilityOf(iam) }) };
});

/**
 * The organization switcher's entries (spec § 5.4): one per ORGANIZATION scope of the same
 * myScopes() result the page shows. A row with no name shows its UUID. A failed myScopes() gives
 * no switcher, not a failed layout. Plain data: OrgSwitcherShell (a client component) builds the
 * hrefs. The shape is app/_components/org-switcher.tsx's OrgSwitcherOrg; lib/ does not import app/.
 */
export function switcherOrgs(scopes: IamResult<MyScopes>): { orgId: string; label: string }[] {
  if (!scopes.ok) return [];
  return scopes.value.entries.flatMap((entry) => (entry.kind === 'organization' ? [{ orgId: entry.orgId, label: entry.label ?? entry.orgId }] : []));
}
