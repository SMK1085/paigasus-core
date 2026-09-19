// SPDX-License-Identifier: Apache-2.0
//
// The loader of /gateway/orgs/[org]/projects/[project] (SMA-636 spec § 4.1, § 4.2, D5: a flat
// route with no team segment). The project PRN is built from the URL's organization and project.
// IAM checks authorization first and the PRN second, so a URL that pairs one organization with
// another organization's project answers 403 or 404 and never shows the project (§ 4.1). The
// console does no pairing check of its own.
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { callIam, isUuid, parseTenancyPrn, projectPrn, type IamClients } from '@paigasus/console-core';
import { lifecycleOf, type NodeLifecycle } from '../../../../node-status';
import { loadServiceAccountSection, type SectionDeps } from '../../../../service-accounts/load';
import type { SectionView } from '../../../../service-accounts/view';
import type { SettingsParams } from '../../load';

export type ProjectSettings =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly projectId: string;
      readonly projectPrn: string;
      readonly project: { readonly name: string; readonly slug: string; readonly lifecycle: NodeLifecycle };
      /** The team's UUID from the project's team_prn, and its name when GetTeam answered. */
      readonly team: { readonly id: string | null; readonly name: string | null };
      /** The organization's name when GetOrganization answered, else null. */
      readonly organizationName: string | null;
      readonly section: SectionView;
    };

export type ProjectSettingsDeps = SectionDeps & { readonly tenancy: Pick<IamClients['tenancy'], 'getProject' | 'getTeam' | 'getOrganization'> };

export async function loadProjectSettings(deps: ProjectSettingsDeps, params: SettingsParams & { readonly org: string; readonly project: string }): Promise<ProjectSettings> {
  if (!isUuid(params.org) || !isUuid(params.project)) return { kind: 'not-found' };
  const orgId = params.org.toLowerCase();
  const projectId = params.project.toLowerCase();
  const prn = projectPrn(orgId, projectId);

  const got = await callIam(() => deps.tenancy.getProject({ prn }));
  if (!got.ok) return got.error.presentation === 'invalid-input' || got.error.presentation === 'not-found' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const project = got.value.project;
  if (project === undefined) return { kind: 'not-found' };
  const lifecycle = lifecycleOf(project);
  const teamRef = parseTenancyPrn(project.teamPrn);

  const [team, organization, section] = await Promise.all([
    callIam(() => deps.tenancy.getTeam({ prn: project.teamPrn })),
    callIam(() => deps.tenancy.getOrganization({ prn: project.orgPrn })),
    loadServiceAccountSection(deps, { ownerPrn: prn, lifecycle, saOffset: params.saOffset, keyOffset: params.keyOffset, sa: params.sa }),
  ]);
  return {
    kind: 'ok',
    orgId,
    projectId,
    projectPrn: prn,
    project: { name: project.name, slug: project.slug, lifecycle },
    team: { id: teamRef?.kind === 'team' ? teamRef.id.toLowerCase() : null, name: team.ok ? (team.value.team?.name ?? null) : null },
    organizationName: organization.ok ? (organization.value.organization?.name ?? null) : null,
    section,
  };
}
