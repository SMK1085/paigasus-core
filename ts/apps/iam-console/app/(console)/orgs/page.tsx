// SPDX-License-Identifier: Apache-2.0
//
// "Your organizations" (spec § 5.1, AC 1). The login ends at /iam/, which sends a signed-in user
// here. The page never uses the login snapshot: myScopes() runs a live Introspect per request.
import type { ReactElement } from 'react';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { CreateForm } from '../../_components/create-form';
import { PageError } from '../../_components/page-error';
import { Pager } from '../../_components/pager';
import { SectionError } from '../../_components/section-error';
import { mayI } from '../../../lib/authorize';
import { iamClients } from '../../../lib/iam';
import { parseOffset } from '../../../lib/paging';
import { myScopes, type ScopeEntry } from '../../../lib/scopes';
import { createOrganizationAction } from './actions';
import { loadOrganizationsPage, type OrganizationList } from './load';

type SearchParams = Promise<Record<string, string | string[] | undefined>>;

const KIND_LABEL: Readonly<Record<ScopeEntry['kind'], string>> = { organization: 'Organization', team: 'Team', project: 'Project' };

/** A denied row does not link: the link would only lead to a 403. */
function scopeHref(entry: ScopeEntry): string | null {
  if (entry.denied) return null;
  if (entry.kind === 'organization') return `/iam/orgs/${entry.orgId}`;
  if (entry.kind === 'team') return `/iam/orgs/${entry.orgId}/teams/${entry.teamId}`;
  return entry.teamId === null ? null : `/iam/orgs/${entry.orgId}/teams/${entry.teamId}/projects/${entry.projectId}`;
}

function scopeLabel(entry: ScopeEntry): string {
  if (entry.label !== null) return entry.label;
  return entry.denied ? 'No access to details' : 'Details not available';
}

function groupByOrganization(entries: readonly ScopeEntry[]): { orgId: string; title: string; entries: ScopeEntry[] }[] {
  const groups = new Map<string, ScopeEntry[]>();
  for (const entry of entries) {
    const list = groups.get(entry.orgId) ?? [];
    list.push(entry);
    groups.set(entry.orgId, list);
  }
  return [...groups.entries()].map(([orgId, list]) => {
    const organization = list.find((entry) => entry.kind === 'organization' && entry.label !== null);
    return { orgId, title: organization?.label ?? `Organization ${orgId}`, entries: list };
  });
}

function OrganizationTable({ list }: { readonly list: OrganizationList }): ReactElement {
  if (list.rows.length === 0 && list.offset === 0) return <EmptyState title="No organizations" />;
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
                {row.orgId === null ? (
                  row.name
                ) : (
                  <ZoneLink href={`/iam/orgs/${row.orgId}`} className="hover:underline">
                    {row.name}
                  </ZoneLink>
                )}
              </TableCell>
              <TableCell>{row.slug}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <Pager label="Organization pages" path="/iam/orgs" param="offset" offset={list.offset} nextOffset={list.nextOffset} />
    </>
  );
}

export default async function OrganizationsPage({ searchParams }: { searchParams: SearchParams }): Promise<ReactElement> {
  const [query, scopes, clients, may] = await Promise.all([searchParams, myScopes(), iamClients(), mayI()]);
  if (!scopes.ok) return <PageError error={scopes.error} />;
  const data = await loadOrganizationsPage({ tenancy: clients.tenancy, mayI: may }, { offset: parseOffset(query.offset) });
  const groups = groupByOrganization(scopes.value.entries);

  return (
    <div className="flex flex-col gap-8 p-6">
      <Breadcrumbs items={[{ label: 'Organizations' }]} />
      <section aria-labelledby="scopes-heading" className="flex flex-col gap-3">
        <h1 id="scopes-heading" className="text-2xl font-semibold">
          Your organizations
        </h1>
        {scopes.value.grantsListed ? null : <p className="text-muted-foreground text-sm">Only your memberships are shown. Your role grants are not listed.</p>}
        {groups.length === 0 ? (
          <EmptyState title="You have no organizations yet" description="Ask an administrator to add you to an organization." />
        ) : (
          groups.map((group) => (
            <div key={group.orgId} className="flex flex-col gap-1">
              <h2 className="text-lg font-medium">{group.title}</h2>
              <ul className="flex flex-col gap-1">
                {group.entries.map((entry) => {
                  const href = scopeHref(entry);
                  const label = scopeLabel(entry);
                  return (
                    <li key={entry.prn} className="flex flex-wrap items-baseline gap-2 text-sm">
                      <span className="text-muted-foreground">{KIND_LABEL[entry.kind]}</span>
                      {href === null ? (
                        <span>{label}</span>
                      ) : (
                        <ZoneLink href={href} className="hover:underline">
                          {label}
                        </ZoneLink>
                      )}
                      {entry.denied ? <code className="text-muted-foreground text-xs">{entry.prn}</code> : null}
                    </li>
                  );
                })}
              </ul>
            </div>
          ))
        )}
        {scopes.value.hiddenCount > 0 ? <p className="text-muted-foreground text-sm">{`${String(scopes.value.hiddenCount)} more scopes are not shown.`}</p> : null}
      </section>
      {data.all === null ? null : (
        <section aria-labelledby="all-orgs-heading" className="flex flex-col gap-3">
          <h2 id="all-orgs-heading" className="text-lg font-semibold">
            All organizations
          </h2>
          {data.all.ok ? <OrganizationTable list={data.all.value} /> : <SectionError error={data.all.error} />}
        </section>
      )}
      {data.canCreateOrganization ? <CreateForm testId="create-organization" title="Create organization" submitLabel="Create" action={createOrganizationAction} /> : null}
    </div>
  );
}
