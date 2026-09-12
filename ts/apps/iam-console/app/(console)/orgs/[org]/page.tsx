// SPDX-License-Identifier: Apache-2.0
//
// /iam/orgs/[org] (spec § 5.2). A navigation is a user action, so this page always asks IAM; the
// only things mayI() hides are the create and membership forms (spec § 6.3).
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { CreateForm } from '../../../_components/create-form';
import { PageError } from '../../../_components/page-error';
import { Pager } from '../../../_components/pager';
import { SectionError } from '../../../_components/section-error';
import { mayI } from '../../../../lib/authorize';
import { iamClients } from '../../../../lib/iam';
import { parseOffset } from '../../../../lib/paging';
import { isUuid } from '../../../../lib/prn';
import { MembersSection } from '../members-section';
import { createTeamAction } from './actions';
import { loadOrganizationPage, type TeamList } from './load';

type Props = {
  params: Promise<{ org: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};

function TeamTable({ orgId, list, membersOffset }: { readonly orgId: string; readonly list: TeamList; readonly membersOffset: number }): ReactElement {
  if (list.rows.length === 0 && list.offset === 0) return <EmptyState title="No teams yet" />;
  return (
    <>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Slug</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {list.rows.map((row) => (
            <TableRow key={row.prn}>
              <TableCell>
                {row.teamId === null ? (
                  row.name
                ) : (
                  <ZoneLink href={`/iam/orgs/${orgId}/teams/${row.teamId}`} className="hover:underline">
                    {row.name}
                  </ZoneLink>
                )}
              </TableCell>
              <TableCell>{row.slug}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <Pager label="Team pages" path={`/iam/orgs/${orgId}`} param="offset" offset={list.offset} nextOffset={list.nextOffset} keep={{ moffset: membersOffset }} />
    </>
  );
}

export default async function OrganizationPage({ params, searchParams }: Props): Promise<ReactElement> {
  const [{ org }, query] = await Promise.all([params, searchParams]);
  if (!isUuid(org)) notFound();
  // Each list's pager keeps the other list's offset in its links (pageHref, lib/paging.ts).
  const offset = parseOffset(query.offset);
  const membersOffset = parseOffset(query.moffset);
  const [clients, may] = await Promise.all([iamClients(), mayI()]);
  const data = await loadOrganizationPage({ tenancy: clients.tenancy, mayI: may }, { org, offset, membersOffset });
  if (data.kind === 'not-found') notFound();
  if (data.kind === 'error') return <PageError error={data.error} />;

  return (
    <div className="flex flex-col gap-8 p-6">
      <Breadcrumbs items={[{ label: 'Organizations', href: '/iam/orgs' }, { label: data.organization.name }]} />
      <div>
        <h1 className="text-2xl font-semibold">{data.organization.name}</h1>
        <p className="text-muted-foreground text-sm">{data.organization.slug}</p>
      </div>
      <section aria-labelledby="teams-heading" className="flex flex-col gap-3">
        <h2 id="teams-heading" className="text-lg font-semibold">
          Teams
        </h2>
        {data.teams.ok ? <TeamTable orgId={data.orgId} list={data.teams.value} membersOffset={membersOffset} /> : <SectionError error={data.teams.error} />}
        {data.canCreateTeam ? <CreateForm testId="create-team" title="Create team" submitLabel="Create" action={createTeamAction} hidden={{ orgPrn: data.orgPrn }} /> : null}
      </section>
      <MembersSection nodePrn={data.orgPrn} path={`/iam/orgs/${data.orgId}`} data={data.members} keep={{ offset }} />
    </div>
  );
}
