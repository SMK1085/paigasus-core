// SPDX-License-Identifier: Apache-2.0
//
// The loader of /iam/orgs/[org]/teams/[team]/projects/[project] (spec § 5.2). A project PRN holds
// the org and the project, not the team, so [team] is checked through GetProject's team_prn.
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import type { MayI } from '../../../../../../../../lib/authorize';
import { callIam } from '../../../../../../../../lib/errors';
import type { IamClients } from '../../../../../../../../lib/iam';
import { isUuid, projectPrn } from '../../../../../../../../lib/prn';
import { loadMembers, type MembersData } from '../../../../../members';
import { sameNode } from '../../../../../node-ref';

export type ProjectPageData =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly teamId: string;
      readonly projectId: string;
      readonly projectPrn: string;
      readonly project: { readonly name: string; readonly slug: string };
      readonly members: MembersData;
    };
export type ProjectPageDeps = {
  readonly tenancy: Pick<IamClients['tenancy'], 'getProject' | 'listMemberships'>;
  readonly mayI: MayI;
};

export async function loadProjectPage(
  deps: ProjectPageDeps,
  params: { readonly org: string; readonly team: string; readonly project: string; readonly membersOffset: number },
): Promise<ProjectPageData> {
  if (!isUuid(params.org) || !isUuid(params.team) || !isUuid(params.project)) return { kind: 'not-found' };
  const orgId = params.org.toLowerCase();
  const teamId = params.team.toLowerCase();
  const projectId = params.project.toLowerCase();
  const prn = projectPrn(orgId, projectId);

  const got = await callIam(() => deps.tenancy.getProject({ prn }));
  // The PRN comes from the URL: IAM answers a wrong [org] with prn-mismatch, which is invalid-input
  // (tenancy.rs:480-482). Spec § 5.2 makes a mismatched URL a 404 (ruling T18.a).
  if (!got.ok) return got.error.presentation === 'invalid-input' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const project = got.value.project;
  if (project === undefined || !sameNode(project.teamPrn, 'team', teamId) || !sameNode(project.orgPrn, 'organization', orgId)) return { kind: 'not-found' };

  const members = await loadMembers(deps, prn, params.membersOffset);
  return { kind: 'ok', orgId, teamId, projectId, projectPrn: prn, project: { name: project.name, slug: project.slug }, members };
}
