// SPDX-License-Identifier: Apache-2.0
//
// /iam/audit (spec § 6.6, AC 4). The page asks discovery, not mayI(): the Audit nav entry is the
// affordance and mayI() hides it (Task 15); a typed URL is a user action, and IAM answers it.
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, ErrorState, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { PRESENTATION_COPY } from '../../_components/error-copy';
import { PageError } from '../../_components/page-error';
import { discovery } from '../../../lib/discovery';
import { iamClients, sessionToken } from '../../../lib/iam';
import { parseCursor } from '../../../lib/paging';
import { auditGate, loadAuditPage } from './load';

type Props = { searchParams: Promise<Record<string, string | string[] | undefined>> };

export default async function AuditPage({ searchParams }: Props): Promise<ReactElement> {
  const [query, token] = await Promise.all([searchParams, sessionToken()]);
  const gate = auditGate(await discovery().getServiceState('iam', token));
  if (gate === 'not-found') notFound();
  if (gate === 'degraded') {
    return (
      <div className="flex flex-col gap-6 p-6" data-testid="audit-degraded">
        <Breadcrumbs items={[{ label: 'Audit' }]} />
        <ErrorState title={PRESENTATION_COPY.degraded.title} description={PRESENTATION_COPY.degraded.body} />
      </div>
    );
  }

  const clients = await iamClients();
  const data = await loadAuditPage({ audit: clients.audit }, { cursor: parseCursor(query.cursor) });
  if (!data.ok) return <PageError error={data.error} />;
  const { rows, cursor, nextCursor } = data.value;

  return (
    <div className="flex flex-col gap-6 p-6">
      <Breadcrumbs items={[{ label: 'Audit' }]} />
      <h1 className="text-2xl font-semibold">Audit log</h1>
      {rows.length === 0 ? (
        <EmptyState title="No audit entries" />
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Time</TableHead>
              <TableHead>Actor</TableHead>
              <TableHead>Action</TableHead>
              <TableHead>Resource</TableHead>
              <TableHead>Outcome</TableHead>
              <TableHead>Correlation id</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((row) => (
              <TableRow key={row.id}>
                <TableCell>{row.occurredAt ?? '—'}</TableCell>
                <TableCell>
                  <code className="text-xs">{row.actorPrn}</code>
                </TableCell>
                <TableCell>{row.action}</TableCell>
                <TableCell>
                  <code className="text-xs">{row.resourcePrn}</code>
                </TableCell>
                <TableCell>{row.outcome}</TableCell>
                <TableCell>
                  <code className="text-xs">{row.correlationId}</code>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      <nav aria-label="Audit pages" className="flex gap-4 text-sm">
        {cursor === '' ? null : (
          <ZoneLink href="/iam/audit" className="hover:underline">
            First page
          </ZoneLink>
        )}
        {nextCursor === null ? null : (
          <ZoneLink href={`/iam/audit?cursor=${encodeURIComponent(nextCursor)}`} className="hover:underline">
            Next
          </ZoneLink>
        )}
      </nav>
    </div>
  );
}
