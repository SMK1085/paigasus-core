// SPDX-License-Identifier: Apache-2.0
//
// The loader of /iam/orgs/[org]/teams/[team] (spec § 5.2). SMA-630 spec § 5.1 adds three affordance
// questions about the team's OWN PRN, and the lifecycle of the team and of each project row.
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { PAGE_SIZE, nextOffset } from '../../../../../../lib/paging';
import { callIam, isUuid, parseTenancyPrn, teamPrn, type IamClients, type IamResult, type MayI } from '@paigasus/console-core';
import { loadMembers, type MembersData } from '../../../members';
import { sameNode } from '../../../node-ref';
import { lifecycleOf, type NodeLifecycle } from '../../../../node-status';

export type ProjectRow = { readonly prn: string; readonly projectId: string | null; readonly slug: string; readonly name: string; readonly lifecycle: NodeLifecycle };
export type ProjectList = { readonly rows: readonly ProjectRow[]; readonly offset: number; readonly nextOffset: number | null };
export type TeamPageData =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly teamId: string;
      readonly teamPrn: string;
      readonly team: { readonly name: string; readonly slug: string; readonly lifecycle: NodeLifecycle };
      readonly projects: IamResult<ProjectList>;
      readonly canCreateProject: boolean;
      readonly canRename: boolean;
      readonly canArchive: boolean;
      readonly canRestore: boolean;
      readonly members: MembersData;
    };
export type TeamPageDeps = {
  readonly tenancy: Pick<IamClients['tenancy'], 'getTeam' | 'listProjects' | 'listMemberships'>;
  readonly mayI: MayI;
};

export async function loadTeamPage(deps: TeamPageDeps, params: { readonly org: string; readonly team: string; readonly offset: number; readonly membersOffset: number }): Promise<TeamPageData> {
  if (!isUuid(params.org) || !isUuid(params.team)) return { kind: 'not-found' };
  const orgId = params.org.toLowerCase();
  const teamId = params.team.toLowerCase();
  const prn = teamPrn(orgId, teamId);

  const got = await callIam(() => deps.tenancy.getTeam({ prn }));
  // The PRN comes from the URL: IAM answers a wrong [org] with prn-mismatch, which is invalid-input
  // (tenancy.rs:331-333). Spec § 5.2 makes a mismatched URL a 404 (ruling T18.a).
  if (!got.ok) return got.error.presentation === 'invalid-input' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const team = got.value.team;
  if (team === undefined || !sameNode(team.orgPrn, 'organization', orgId)) return { kind: 'not-found' };

  const [projects, canCreateProject, canRename, canArchive, canRestore, members] = await Promise.all([
    callIam(() => deps.tenancy.listProjects({ teamPrn: prn, limit: PAGE_SIZE, offset: BigInt(params.offset) })),
    deps.mayI('CreateProject', prn),
    deps.mayI('RenameTeam', prn),
    deps.mayI('ArchiveTeam', prn),
    deps.mayI('RestoreTeam', prn),
    loadMembers(deps, prn, params.membersOffset),
  ]);
  const projectList: IamResult<ProjectList> = projects.ok
    ? {
        ok: true,
        value: {
          rows: projects.value.projects.map((project): ProjectRow => {
            const ref = parseTenancyPrn(project.prn);
            return { prn: project.prn, projectId: ref?.kind === 'project' ? ref.id.toLowerCase() : null, slug: project.slug, name: project.name, lifecycle: lifecycleOf(project) };
          }),
          offset: params.offset,
          nextOffset: nextOffset(params.offset, projects.value.projects.length),
        },
      }
    : projects;

  return {
    kind: 'ok',
    orgId,
    teamId,
    teamPrn: prn,
    team: { name: team.name, slug: team.slug, lifecycle: lifecycleOf(team) },
    projects: projectList,
    canCreateProject,
    canRename,
    canArchive,
    canRestore,
    members,
  };
}
