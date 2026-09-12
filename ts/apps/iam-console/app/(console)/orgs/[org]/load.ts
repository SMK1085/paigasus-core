// SPDX-License-Identifier: Apache-2.0
//
// The loader of /iam/orgs/[org] (spec § 5.2). URLs use UUIDs because a PRN holds no slug. The
// organization comes first: when IAM denies it, the page is the 403 view and nothing else runs.
// The PRN comes from the URL, so an invalid-input answer (IAM's prn-mismatch, InvalidArgument,
// adapters/grpc/tenancy.rs:176-178) means that the URL names no such node: notFound().
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import type { MayI } from '../../../../lib/authorize';
import { callIam, type IamResult } from '../../../../lib/errors';
import type { IamClients } from '../../../../lib/iam';
import { PAGE_SIZE, nextOffset } from '../../../../lib/paging';
import { isUuid, organizationPrn, parseTenancyPrn } from '../../../../lib/prn-tenancy';
import { loadMembers, type MembersData } from '../members';

export type TeamRow = { readonly prn: string; readonly teamId: string | null; readonly slug: string; readonly name: string };
export type TeamList = { readonly rows: readonly TeamRow[]; readonly offset: number; readonly nextOffset: number | null };
export type OrganizationPageData =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly orgPrn: string;
      readonly organization: { readonly name: string; readonly slug: string };
      readonly teams: IamResult<TeamList>;
      readonly canCreateTeam: boolean;
      readonly members: MembersData;
    };
export type OrganizationPageDeps = {
  readonly tenancy: Pick<IamClients['tenancy'], 'getOrganization' | 'listTeams' | 'listMemberships'>;
  readonly mayI: MayI;
};

export async function loadOrganizationPage(deps: OrganizationPageDeps, params: { readonly org: string; readonly offset: number; readonly membersOffset: number }): Promise<OrganizationPageData> {
  if (!isUuid(params.org)) return { kind: 'not-found' };
  const orgId = params.org.toLowerCase();
  const orgPrn = organizationPrn(orgId);

  const got = await callIam(() => deps.tenancy.getOrganization({ prn: orgPrn }));
  if (!got.ok) return got.error.presentation === 'invalid-input' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const organization = got.value.organization;
  if (organization === undefined) return { kind: 'not-found' };

  const [teams, canCreateTeam, members] = await Promise.all([
    callIam(() => deps.tenancy.listTeams({ orgPrn, limit: PAGE_SIZE, offset: BigInt(params.offset) })),
    deps.mayI('CreateTeam', orgPrn),
    loadMembers(deps, orgPrn, params.membersOffset),
  ]);
  const teamList: IamResult<TeamList> = teams.ok
    ? {
        ok: true,
        value: {
          rows: teams.value.teams.map((team): TeamRow => {
            const ref = parseTenancyPrn(team.prn);
            return { prn: team.prn, teamId: ref?.kind === 'team' ? ref.id.toLowerCase() : null, slug: team.slug, name: team.name };
          }),
          offset: params.offset,
          nextOffset: nextOffset(params.offset, teams.value.teams.length),
        },
      }
    : teams;

  return { kind: 'ok', orgId, orgPrn, organization: { name: organization.name, slug: organization.slug }, teams: teamList, canCreateTeam, members };
}
