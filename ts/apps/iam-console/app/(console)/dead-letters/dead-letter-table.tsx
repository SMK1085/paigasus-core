// SPDX-License-Identifier: Apache-2.0
//
// The dead-letter list (SMA-629 spec § 6.3). NO directive: the page, a server component, renders it
// inside DeadLettersFrame, and tests/unit/dead-letters-frame.test.tsx renders the same code in jsdom.
// It keeps IAM's order (id descending). The payload and the last error render as TEXT in a <pre>:
// React escapes them, and the page never parses the payload and never renders it as HTML. The payload
// has no size bound (only last_error has one, 1 KB), so each <pre> has a maximum height and scrolls.
import { Fragment, type ReactElement } from 'react';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { DeadLetterRowControls } from './dead-letters-frame';
import type { DeadLetterRow } from './load';

const NONE = '—';
const PRE_CLASS = 'bg-muted max-h-64 overflow-auto rounded-pgs p-2 text-xs break-all whitespace-pre-wrap';
const COLUMNS = 7;

function Code({ value }: { readonly value: string | null }): ReactElement {
  return value === null ? <>{NONE}</> : <code className="text-xs">{value}</code>;
}

/**
 * `canReplay` (SMA-661 D4) reaches every row's controls; it defaults to true. The caption names the
 * NULL `parked_at` blind spot (SMA-661 § 4.6): with a time bound set, IAM never lists such a row.
 */
export function DeadLetterTable({ rows, canReplay = true }: { readonly rows: readonly DeadLetterRow[]; readonly canReplay?: boolean }): ReactElement {
  return (
    <Table>
      <caption className="text-muted-foreground mb-2 caption-top text-left text-sm">Newest events first. With a parked-time filter set, an event with no parked time is not listed.</caption>
      <TableHeader>
        <TableRow>
          <TableHead>Event id</TableHead>
          <TableHead>Parked</TableHead>
          <TableHead>Event type</TableHead>
          <TableHead>Aggregate</TableHead>
          <TableHead>Attempts</TableHead>
          <TableHead>Correlation id</TableHead>
          <TableHead>Actions</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((row) => (
          <Fragment key={row.id}>
            <TableRow data-testid="dead-letter-row" data-id={row.id}>
              <TableCell>
                <Code value={row.id} />
              </TableCell>
              <TableCell>{row.parkedAt ?? NONE}</TableCell>
              <TableCell>{row.eventType}</TableCell>
              <TableCell>
                <Code value={row.aggregatePrn} />
              </TableCell>
              <TableCell>{row.attempts}</TableCell>
              <TableCell>
                <Code value={row.correlationId} />
              </TableCell>
              <TableCell>
                <DeadLetterRowControls canReplay={canReplay} id={row.id} />
              </TableCell>
            </TableRow>
            <TableRow>
              <TableCell colSpan={COLUMNS}>
                <details>
                  <summary className="cursor-pointer text-sm">{`Details of event ${row.id}`}</summary>
                  <dl className="mt-2 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
                    <dt>Occurred</dt>
                    <dd>{row.occurredAt ?? NONE}</dd>
                    <dt>Schema version</dt>
                    <dd>{row.schemaVersion}</dd>
                    <dt>Actor</dt>
                    <dd>
                      <Code value={row.actorPrn} />
                    </dd>
                  </dl>
                  <p className="mt-2 text-sm font-medium">Payload</p>
                  <pre data-testid="dead-letter-payload" className={PRE_CLASS}>
                    {row.payload}
                  </pre>
                  <p className="mt-2 text-sm font-medium">Last error</p>
                  <pre data-testid="dead-letter-last-error" className={PRE_CLASS}>
                    {row.lastError ?? NONE}
                  </pre>
                </details>
              </TableCell>
            </TableRow>
          </Fragment>
        ))}
      </TableBody>
    </Table>
  );
}
