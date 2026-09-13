// SPDX-License-Identifier: Apache-2.0
//
// /iam/orgs/[org]/teams/[team]/projects/[project] (spec § 5.2). Mutations: attach and detach only.
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs } from '@paigasus/app-shell';
import { PageError } from '../../../../../../../_components/page-error';
import { mayI } from '../../../../../../../../lib/authorize';
import { iamClients } from '../../../../../../../../lib/iam';
import { parseOffset } from '../../../../../../../../lib/paging';
import { isUuid } from '../../../../../../../../lib/prn-tenancy';
import { MembersSection } from '../../../../../members-section';
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
        <h1 className="text-2xl font-semibold">{data.project.name}</h1>
        <p className="text-muted-foreground text-sm">{data.project.slug}</p>
      </div>
      <MembersSection nodePrn={data.projectPrn} path={`${teamPath}/projects/${data.projectId}`} data={data.members} />
    </div>
  );
}
