// SPDX-License-Identifier: Apache-2.0
//
// /iam/dead-letters (SMA-629 spec § 6.2–§ 6.4, AC 1 and AC 2). The page asks discovery, not mayI()
// (SMA-511 § 6.3): the Dead letters nav entry is the affordance and mayI() hides it; a typed URL is a
// user action, and IAM answers it. Every OutboxService RPC is Root-only inside IAM.
//
// Every view that does not throw renders inside DeadLettersFrame, keyed by `${eventType}|${cursor}`,
// so a revalidated render after a replay or discard keeps the frame and its result, and a move to
// another page or filter starts an empty frame. The two query parsers are pure and run before the
// degraded branch ONLY to compute that key; a degraded IAM gets the degraded view whatever the query.
import type { ReactElement, ReactNode } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, ErrorState, Field, Input, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';
import { PRESENTATION_COPY } from '../../_components/error-copy';
import { PageError } from '../../_components/page-error';
import { SectionError } from '../../_components/section-error';
import { discovery, iamClients, sessionToken } from '../../../lib/console';
import { listHref, MAX_EVENT_TYPE_LENGTH, parseCursor, parseEventType } from '../../../lib/paging';
import { discardDeadLetterAction, replayDeadLetterAction } from './actions';
import { DeadLetterTable } from './dead-letter-table';
import { DeadLettersFrame } from './dead-letters-frame';
import { deadLettersGate, loadDeadLettersPage } from './load';

type Props = { searchParams: Promise<Record<string, string | string[] | undefined>> };

/** The full path, as the ingress sees it (ZoneLink needs it). */
const PATH = '/iam/dead-letters';

/** The breadcrumbs, the heading and the frame around one view. A plain function, not a component. */
function shell(frameKey: string, body: ReactNode): ReactElement {
  return (
    <div className="flex flex-col gap-6 p-6">
      <Breadcrumbs items={[{ label: 'Dead letters' }]} />
      <h1 className="text-2xl font-semibold">Dead letters</h1>
      <DeadLettersFrame key={frameKey} actions={{ replay: replayDeadLetterAction, discard: discardDeadLetterAction }}>
        {body}
      </DeadLettersFrame>
    </div>
  );
}

/**
 * A plain GET form with NO `action` attribute: it submits to the current URL, /iam/dead-letters. A
 * basePath-relative action="/dead-letters" would leave the zone (§ 6.3). It needs no JavaScript, and a
 * new filter starts at the first page.
 */
function filterForm(eventType: string): ReactElement {
  return (
    <form method="get" aria-label="Filter dead letters" className="flex flex-wrap items-end gap-2">
      <Field label="Event type" htmlFor="dead-letters-event-type">
        <Input name="eventType" defaultValue={eventType} maxLength={MAX_EVENT_TYPE_LENGTH} autoComplete="off" />
      </Field>
      <button type="submit" className={SECONDARY_BUTTON_CLASS}>
        Filter
      </button>
    </form>
  );
}

export default async function DeadLettersPage({ searchParams }: Props): Promise<ReactElement> {
  const [query, token] = await Promise.all([searchParams, sessionToken()]);
  const gate = deadLettersGate(await discovery().getServiceState('iam', token));
  if (gate === 'not-found') notFound();

  const eventType = parseEventType(query.eventType);
  const cursor = parseCursor(query.cursor);
  const frameKey = `${eventType.ok ? eventType.value : ''}|${cursor.ok ? cursor.cursor : ''}`;

  if (gate === 'degraded') {
    return shell(
      frameKey,
      <div data-testid="dead-letters-degraded">
        <ErrorState title={PRESENTATION_COPY.degraded.title} description={PRESENTATION_COPY.degraded.body} />
      </div>,
    );
  }

  // Both parsers run BEFORE the clients are built: a refused query never becomes an IAM call.
  if (!eventType.ok) return <PageError error={eventType.error} />;
  if (!cursor.ok) return <PageError error={cursor.error} />;

  const clients = await iamClients();
  const data = await loadDeadLettersPage({ outbox: clients.outbox }, { cursor: cursor.cursor, eventType: eventType.value });
  if (!data.ok) {
    // A fresh GET keeps its real 403 or 404. Every other list error stays inside the frame.
    if (data.error.presentation === 'forbidden' || data.error.presentation === 'not-found') return <PageError error={data.error} />;
    return shell(
      frameKey,
      <>
        {filterForm(eventType.value)}
        <SectionError error={data.error} />
      </>,
    );
  }

  const { rows, nextCursor } = data.value;
  return shell(
    frameKey,
    <>
      {filterForm(eventType.value)}
      {rows.length === 0 ? <EmptyState title="No dead letters" /> : <DeadLetterTable rows={rows} />}
      <nav aria-label="Dead-letter pages" className="flex gap-4 text-sm">
        {cursor.cursor === '' ? null : (
          <ZoneLink href={listHref(PATH, { eventType: eventType.value })} className="hover:underline">
            First page
          </ZoneLink>
        )}
        {nextCursor === null ? null : (
          <ZoneLink href={listHref(PATH, { eventType: eventType.value, cursor: nextCursor })} className="hover:underline">
            Next
          </ZoneLink>
        )}
      </nav>
    </>,
  );
}
