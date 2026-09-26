// SPDX-License-Identifier: Apache-2.0
//
// The loader of /gateway/orgs/[org] (SMA-636 spec § 4.2). URLs use UUIDs because a PRN holds no
// slug. GetOrganization comes first: when IAM denies it, the page is the 403 view and nothing else
// runs. The PRN comes from the URL, so an invalid-input answer (IAM's prn-mismatch) or a not-found
// answer means the URL names no such node: not-found.
//
// Then, in parallel: the service-accounts section, the Projects list — ListTeams (limit 51),
// then ListProjects (limit 51) for each SHOWN team, at most 8 calls in flight — and, since SMA-676,
// the "Model access for people" section (people-model-access/load.ts). Any Projects or people
// failure is that section's own error; the page stays 200.
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { callIam, isUuid, organizationPrn, parseTenancyPrn, type IamClients, type IamResult } from '@paigasus/console-core';
import { mapWithLimit } from '../../../../lib/concurrency';
import { REQUEST_LIMIT, pageOf } from '../../../../lib/paging';
import { lifecycleOf, lifecycleView, type NodeLifecycle } from '../../node-status';
import { loadPeopleModelAccess, type PeopleModelAccessDeps } from '../../people-model-access/load';
import type { PeopleModelAccessView } from '../../people-model-access/view';
import { loadServiceAccountSection, type SectionDeps } from '../../service-accounts/load';
import type { SectionView } from '../../service-accounts/view';

/** § 4.2: the org page makes at most this many ListProjects calls at once. */
export const PROJECT_CALLS_IN_FLIGHT = 8;

type Tenancy = IamClients['tenancy'];
type ProjectsAnswer = Awaited<ReturnType<Tenancy['listProjects']>>;

export type SettingsParams = { readonly saOffset: number; readonly keyOffset: number; readonly sa: string | null };

export type ProjectLink = { readonly projectId: string | null; readonly name: string; readonly slug: string };
export type TeamGroup = { readonly teamId: string | null; readonly name: string; readonly projects: readonly ProjectLink[]; readonly moreProjects: boolean };
export type ProjectsView = { readonly kind: 'error'; readonly error: PaigasusError } | { readonly kind: 'ok'; readonly teams: readonly TeamGroup[]; readonly moreTeams: boolean };

export type OrganizationSettings =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly orgPrn: string;
      readonly organization: { readonly name: string; readonly slug: string; readonly lifecycle: NodeLifecycle };
      readonly section: SectionView;
      readonly projects: ProjectsView;
      readonly people: PeopleModelAccessView;
    };

/** The page head both the org page and the playground page load first (SMA-635 spec § 6.1). */
export type OrganizationHead =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | { readonly kind: 'ok'; readonly orgId: string; readonly orgPrn: string; readonly organization: { readonly name: string; readonly slug: string; readonly lifecycle: NodeLifecycle } };

export type OrganizationSettingsDeps = SectionDeps & PeopleModelAccessDeps & { readonly tenancy: Pick<Tenancy, 'getOrganization' | 'listTeams' | 'listProjects'> };

function idOf(prn: string, kind: 'team' | 'project'): string | null {
  const ref = parseTenancyPrn(prn);
  return ref?.kind === kind ? ref.id.toLowerCase() : null;
}

function failure<T>(results: readonly IamResult<T>[]): PaigasusError | null {
  for (const result of results) {
    if (!result.ok) return result.error;
  }
  return null;
}

async function loadProjects(tenancy: Pick<Tenancy, 'listTeams' | 'listProjects'>, orgPrn: string): Promise<ProjectsView> {
  const teams = await callIam(() => tenancy.listTeams({ orgPrn, limit: REQUEST_LIMIT, offset: 0n }));
  if (!teams.ok) return { kind: 'error', error: teams.error };
  const shown = pageOf(teams.value.teams, 0);
  const lists = await mapWithLimit(shown.rows, PROJECT_CALLS_IN_FLIGHT, (team) => callIam(() => tenancy.listProjects({ teamPrn: team.prn, limit: REQUEST_LIMIT, offset: 0n })));
  const failed = failure(lists);
  if (failed !== null) return { kind: 'error', error: failed };
  const groups = shown.rows.map((team, index): TeamGroup => {
    const list = lists[index];
    const received: ProjectsAnswer['projects'] = list?.ok === true ? list.value.projects : [];
    const projects = pageOf(received, 0);
    return {
      teamId: idOf(team.prn, 'team'),
      name: team.name,
      projects: projects.rows.map((project) => ({ projectId: idOf(project.prn, 'project'), name: project.name, slug: project.slug })),
      moreProjects: projects.nextOffset !== null,
    };
  });
  return { kind: 'ok', teams: groups, moreTeams: shown.nextOffset !== null };
}

/**
 * The URL's UUID and GetOrganization: a value that is not a UUID, an invalid-input answer (IAM's
 * prn-mismatch) and a not-found answer are all not-found; any other failure is the page's error,
 * which PageError renders as the 403 view for a denial.
 */
export async function loadOrganizationHead(tenancy: Pick<Tenancy, 'getOrganization'>, org: string): Promise<OrganizationHead> {
  if (!isUuid(org)) return { kind: 'not-found' };
  const orgId = org.toLowerCase();
  const orgPrn = organizationPrn(orgId);
  const got = await callIam(() => tenancy.getOrganization({ prn: orgPrn }));
  if (!got.ok) return got.error.presentation === 'invalid-input' || got.error.presentation === 'not-found' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const organization = got.value.organization;
  if (organization === undefined) return { kind: 'not-found' };
  return { kind: 'ok', orgId, orgPrn, organization: { name: organization.name, slug: organization.slug, lifecycle: lifecycleOf(organization) } };
}

export async function loadOrganizationSettings(deps: OrganizationSettingsDeps, params: SettingsParams & { readonly org: string }): Promise<OrganizationSettings> {
  const head = await loadOrganizationHead(deps.tenancy, params.org);
  if (head.kind !== 'ok') return head;
  // SMA-676: the people section runs in parallel with the other two. D14: controls need the org and
  // its ancestors active, which is the lifecycle view's 'active'.
  const [section, projects, people] = await Promise.all([
    loadServiceAccountSection(deps, { ownerPrn: head.orgPrn, lifecycle: head.organization.lifecycle, saOffset: params.saOffset, keyOffset: params.keyOffset, sa: params.sa }),
    loadProjects(deps.tenancy, head.orgPrn),
    loadPeopleModelAccess(deps, { orgPrn: head.orgPrn, orgActive: lifecycleView(head.organization.lifecycle) === 'active' }),
  ]);
  return { kind: 'ok', orgId: head.orgId, orgPrn: head.orgPrn, organization: head.organization, section, projects, people };
}
