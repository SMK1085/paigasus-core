// SPDX-License-Identifier: Apache-2.0
//
// The loader of /iam/orgs/[org]/teams/[team] (spec § 5.2).
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import type { MayI } from '../../../../../../lib/authorize';
import { callIam, type IamResult } from '../../../../../../lib/errors';
import type { IamClients } from '../../../../../../lib/iam';
import { PAGE_SIZE, nextOffset } from '../../../../../../lib/paging';
import { isUuid, parseTenancyPrn, teamPrn } from '../../../../../../lib/prn-tenancy';
import { loadMembers, type MembersData } from '../../../members';
import { sameNode } from '../../../node-ref';

export type ProjectRow = { readonly prn: string; readonly projectId: string | null; readonly slug: string; readonly name: string };
export type ProjectList = { readonly rows: readonly ProjectRow[]; readonly offset: number; readonly nextOffset: number | null };
export type TeamPageData =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly teamId: string;
      readonly teamPrn: string;
      readonly team: { readonly name: string; readonly slug: string };
      readonly projects: IamResult<ProjectList>;
      readonly canCreateProject: boolean;
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

  const [projects, canCreateProject, members] = await Promise.all([
    callIam(() => deps.tenancy.listProjects({ teamPrn: prn, limit: PAGE_SIZE, offset: BigInt(params.offset) })),
    deps.mayI('CreateProject', prn),
    loadMembers(deps, prn, params.membersOffset),
  ]);
  const projectList: IamResult<ProjectList> = projects.ok
    ? {
        ok: true,
        value: {
          rows: projects.value.projects.map((project): ProjectRow => {
            const ref = parseTenancyPrn(project.prn);
            return { prn: project.prn, projectId: ref?.kind === 'project' ? ref.id.toLowerCase() : null, slug: project.slug, name: project.name };
          }),
          offset: params.offset,
          nextOffset: nextOffset(params.offset, projects.value.projects.length),
        },
      }
    : projects;

  return { kind: 'ok', orgId, teamId, teamPrn: prn, team: { name: team.name, slug: team.slug }, projects: projectList, canCreateProject, members };
}
