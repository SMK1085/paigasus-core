// SPDX-License-Identifier: Apache-2.0
//
// /iam/orgs/[org]/teams/[team] (spec § 5.2). The breadcrumb does not name the organization: a user
// with a team role only can have no access to GetOrganization, and this page must not need it.
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { CreateForm } from '../../../../../_components/create-form';
import { PageError } from '../../../../../_components/page-error';
import { Pager } from '../../../../../_components/pager';
import { SectionError } from '../../../../../_components/section-error';
import { isUuid } from '@paigasus/console-core';
import { iamClients, mayI } from '../../../../../../lib/console';
import { parseOffset } from '../../../../../../lib/paging';
import { ManageSection } from '../../../../manage-section';
import { statusColumnLabel } from '../../../../node-status';
import { StatusBadge } from '../../../../status-badge';
import { MembersSection } from '../../../members-section';
import { archiveTeamAction, createProjectAction, renameTeamAction, restoreTeamAction } from './actions';
import { loadTeamPage, type ProjectList } from './load';

type Props = {
  params: Promise<{ org: string; team: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};

function ProjectTable({ base, list, membersOffset }: { readonly base: string; readonly list: ProjectList; readonly membersOffset: number }): ReactElement {
  if (list.rows.length === 0 && list.offset === 0) return <EmptyState title="No projects yet" />;
  return (
    <>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Slug</TableHead>
            <TableHead>Status</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {list.rows.map((row) => (
            <TableRow key={row.prn}>
              <TableCell>
                {row.projectId === null ? (
                  row.name
                ) : (
                  <ZoneLink href={`${base}/projects/${row.projectId}`} className="hover:underline">
                    {row.name}
                  </ZoneLink>
                )}
              </TableCell>
              <TableCell>{row.slug}</TableCell>
              <TableCell>{statusColumnLabel(row.lifecycle)}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <Pager label="Project pages" path={base} param="offset" offset={list.offset} nextOffset={list.nextOffset} keep={{ moffset: membersOffset }} />
    </>
  );
}

export default async function TeamPage({ params, searchParams }: Props): Promise<ReactElement> {
  const [{ org, team }, query] = await Promise.all([params, searchParams]);
  if (!isUuid(org) || !isUuid(team)) notFound();
  // Each list's pager keeps the other list's offset in its links (pageHref, lib/paging.ts).
  const offset = parseOffset(query.offset);
  const membersOffset = parseOffset(query.moffset);
  const [clients, may] = await Promise.all([iamClients(), mayI()]);
  const data = await loadTeamPage({ tenancy: clients.tenancy, mayI: may }, { org, team, offset, membersOffset });
  if (data.kind === 'not-found') notFound();
  if (data.kind === 'error') return <PageError error={data.error} />;
  const base = `/iam/orgs/${data.orgId}/teams/${data.teamId}`;

  return (
    <div className="flex flex-col gap-8 p-6">
      <Breadcrumbs items={[{ label: 'Organizations', href: '/iam/orgs' }, { label: 'Organization', href: `/iam/orgs/${data.orgId}` }, { label: data.team.name }]} />
      <div>
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-2xl font-semibold">{data.team.name}</h1>
          <StatusBadge lifecycle={data.team.lifecycle} />
        </div>
        <p className="text-muted-foreground text-sm">{data.team.slug}</p>
      </div>
      <section aria-labelledby="projects-heading" className="flex flex-col gap-3">
        <h2 id="projects-heading" className="text-lg font-semibold">
          Projects
        </h2>
        {data.projects.ok ? <ProjectTable base={base} list={data.projects.value} membersOffset={membersOffset} /> : <SectionError error={data.projects.error} />}
        {data.canCreateProject ? <CreateForm testId="create-project" title="Create project" submitLabel="Create" action={createProjectAction} hidden={{ teamPrn: data.teamPrn }} /> : null}
      </section>
      <MembersSection nodePrn={data.teamPrn} path={base} data={data.members} keep={{ offset }} />
      <ManageSection
        node="team"
        prn={data.teamPrn}
        name={data.team.name}
        slug={data.team.slug}
        lifecycle={data.team.lifecycle}
        can={{ rename: data.canRename, archive: data.canArchive, restore: data.canRestore }}
        actions={{ rename: renameTeamAction, archive: archiveTeamAction, restore: restoreTeamAction }}
      />
    </div>
  );
}
