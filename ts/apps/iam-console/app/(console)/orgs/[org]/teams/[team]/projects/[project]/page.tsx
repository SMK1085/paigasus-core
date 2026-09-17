// SPDX-License-Identifier: Apache-2.0
//
// /iam/orgs/[org]/teams/[team]/projects/[project] (spec § 5.2). Mutations: attach and detach, and
// (SMA-630) rename, archive and restore.
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs } from '@paigasus/app-shell';
import { PageError } from '../../../../../../../_components/page-error';
import { isUuid } from '@paigasus/console-core';
import { iamClients, mayI } from '../../../../../../../../lib/console';
import { parseOffset } from '../../../../../../../../lib/paging';
import { ManageSection } from '../../../../../../manage-section';
import { StatusBadge } from '../../../../../../status-badge';
import { MembersSection } from '../../../../../members-section';
import { archiveProjectAction, renameProjectAction, restoreProjectAction } from './actions';
import { loadProjectPage } from './load';

type Props = {
  params: Promise<{ org: string; team: string; project: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};

export default async function ProjectPage({ params, searchParams }: Props): Promise<ReactElement> {
  const [{ org, team, project }, query] = await Promise.all([params, searchParams]);
  if (!isUuid(org) || !isUuid(team) || !isUuid(project)) notFound();
  const [clients, may] = await Promise.all([iamClients(), mayI()]);
  const data = await loadProjectPage({ tenancy: clients.tenancy, mayI: may }, { org, team, project, membersOffset: parseOffset(query.moffset) });
  if (data.kind === 'not-found') notFound();
  if (data.kind === 'error') return <PageError error={data.error} />;
  const teamPath = `/iam/orgs/${data.orgId}/teams/${data.teamId}`;

  return (
    <div className="flex flex-col gap-8 p-6">
      <Breadcrumbs
        items={[{ label: 'Organizations', href: '/iam/orgs' }, { label: 'Organization', href: `/iam/orgs/${data.orgId}` }, { label: 'Team', href: teamPath }, { label: data.project.name }]}
      />
      <div>
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-2xl font-semibold">{data.project.name}</h1>
          <StatusBadge lifecycle={data.project.lifecycle} />
        </div>
        <p className="text-muted-foreground text-sm">{data.project.slug}</p>
      </div>
      <MembersSection nodePrn={data.projectPrn} path={`${teamPath}/projects/${data.projectId}`} data={data.members} />
      <ManageSection
        node="project"
        prn={data.projectPrn}
        name={data.project.name}
        slug={data.project.slug}
        lifecycle={data.project.lifecycle}
        can={{ rename: data.canRename, archive: data.canArchive, restore: data.canRestore }}
        actions={{ rename: renameProjectAction, archive: archiveProjectAction, restore: restoreProjectAction }}
      />
    </div>
  );
}
