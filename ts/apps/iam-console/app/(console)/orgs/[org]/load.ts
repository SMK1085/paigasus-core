// SPDX-License-Identifier: Apache-2.0
//
// The loader of /iam/orgs/[org] (spec § 5.2). URLs use UUIDs because a PRN holds no slug. The
// organization comes first: when IAM denies it, the page is the 403 view and nothing else runs.
// The PRN comes from the URL, so an invalid-input answer (IAM's prn-mismatch, InvalidArgument,
// adapters/grpc/tenancy.rs:176-178) means that the URL names no such node: notFound().
//
// SMA-630 spec § 5.1: three more affordance questions, all about the organization's OWN PRN, and
// the lifecycle of the organization and of each team row.
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { PAGE_SIZE, nextOffset } from '../../../../lib/paging';
import { callIam, isUuid, organizationPrn, parseTenancyPrn, type IamClients, type IamResult, type MayI } from '@paigasus/console-core';
import { loadMembers, type MembersData } from '../members';
import { lifecycleOf, type NodeLifecycle } from '../../node-status';

export type TeamRow = { readonly prn: string; readonly teamId: string | null; readonly slug: string; readonly name: string; readonly lifecycle: NodeLifecycle };
export type TeamList = { readonly rows: readonly TeamRow[]; readonly offset: number; readonly nextOffset: number | null };
export type OrganizationPageData =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly orgPrn: string;
      readonly organization: { readonly name: string; readonly slug: string; readonly lifecycle: NodeLifecycle };
      readonly teams: IamResult<TeamList>;
      readonly canCreateTeam: boolean;
      readonly canRename: boolean;
      readonly canArchive: boolean;
      readonly canRestore: boolean;
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

  const [teams, canCreateTeam, canRename, canArchive, canRestore, members] = await Promise.all([
    callIam(() => deps.tenancy.listTeams({ orgPrn, limit: PAGE_SIZE, offset: BigInt(params.offset) })),
    deps.mayI('CreateTeam', orgPrn),
    deps.mayI('RenameOrganization', orgPrn),
    deps.mayI('ArchiveOrganization', orgPrn),
    deps.mayI('RestoreOrganization', orgPrn),
    loadMembers(deps, orgPrn, params.membersOffset),
  ]);
  const teamList: IamResult<TeamList> = teams.ok
    ? {
        ok: true,
        value: {
          rows: teams.value.teams.map((team): TeamRow => {
            const ref = parseTenancyPrn(team.prn);
            return { prn: team.prn, teamId: ref?.kind === 'team' ? ref.id.toLowerCase() : null, slug: team.slug, name: team.name, lifecycle: lifecycleOf(team) };
          }),
          offset: params.offset,
          nextOffset: nextOffset(params.offset, teams.value.teams.length),
        },
      }
    : teams;

  return {
    kind: 'ok',
    orgId,
    orgPrn,
    organization: { name: organization.name, slug: organization.slug, lifecycle: lifecycleOf(organization) },
    teams: teamList,
    canCreateTeam,
    canRename,
    canArchive,
    canRestore,
    members,
  };
}
