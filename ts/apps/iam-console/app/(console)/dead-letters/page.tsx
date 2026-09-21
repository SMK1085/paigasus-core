// SPDX-License-Identifier: Apache-2.0
//
// /iam/dead-letters (SMA-629 spec § 6.2–§ 6.4, AC 1 and AC 2). The page asks discovery, not mayI()
// (SMA-511 § 6.3): the Dead letters nav entry is the affordance and mayI() hides it; a typed URL is a
// user action, and IAM answers it. Every OutboxService RPC is Root-only inside IAM.
//
// Every view that does not throw renders inside DeadLettersFrame, keyed by
// `${eventType}|${parkedFrom}|${parkedTo}|${cursor}` with the CANONICAL bounds (SMA-661 spec § 4.2),
// so a revalidated render after a replay or discard keeps the frame and its result, two spellings of
// one window share a frame, and a move to another page or filter starts an empty frame. The four
// query parsers are pure and run before the degraded branch ONLY to compute that key; a degraded IAM
// gets the degraded view whatever the query.
import type { ReactElement, ReactNode } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, ErrorState, Field, Input, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';
import { PRESENTATION_COPY } from '../../_components/error-copy';
import { PageError } from '../../_components/page-error';
import { SectionError } from '../../_components/section-error';
import { discovery, iamClients, sessionToken } from '../../../lib/console';
import { listHref, MAX_EVENT_TYPE_LENGTH, MAX_PARKED_BOUND_LENGTH, parseCursor, parseEventType, parseParkedBound, type ParkedBoundField, type ParsedParkedBound } from '../../../lib/paging';
import { discardDeadLetterAction, replayDeadLetterAction } from './actions';
import { DeadLetterTable } from './dead-letter-table';
import { DeadLettersFrame } from './dead-letters-frame';
import { deadLettersGate, loadDeadLettersPage } from './load';

type Props = { searchParams: Promise<Record<string, string | string[] | undefined>> };

/** The full path, as the ingress sees it (ZoneLink needs it). */
const PATH = '/iam/dead-letters';

/** SMA-661 spec § 4.3. Both bounds are inclusive in IAM's query (`>=` and `<=`), and the input cannot show that. */
const PARKED_PLACEHOLDER = '2026-09-19T00:00:00Z';
const PARKED_DESCRIPTION = 'Both bounds are included. Use a zone, for example 2026-09-19T00:00:00Z or +02:00.';

/** The three filter values: the event type, and each parked bound as parsed (typed value, canonical value or error). */
type Filter = { readonly eventType: string; readonly parkedFrom: ParsedParkedBound; readonly parkedTo: ParsedParkedBound };

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
 * One parked-time input (SMA-661 spec § 4.3). It shows the TYPED value, so a refused bound keeps
 * what the operator wrote (D6). A refused bound's sentence goes to Field's `error`, which wires
 * aria-describedby and aria-invalid. `maxLength` bounds typing only; the parser never cuts a value.
 */
function parkedField(label: string, id: string, name: ParkedBoundField, bound: ParsedParkedBound): ReactElement {
  const input = <Input name={name} defaultValue={bound.raw} placeholder={PARKED_PLACEHOLDER} maxLength={MAX_PARKED_BOUND_LENGTH} autoComplete="off" />;
  return bound.ok ? (
    <Field label={label} htmlFor={id} description={PARKED_DESCRIPTION}>
      {input}
    </Field>
  ) : (
    <Field label={label} htmlFor={id} description={PARKED_DESCRIPTION} error={bound.error.message}>
      {input}
    </Field>
  );
}

/**
 * A plain GET form with NO `action` attribute: it submits to the current URL, /iam/dead-letters. A
 * basePath-relative action="/dead-letters" would leave the zone (§ 6.3). It needs no JavaScript, and a
 * new filter starts at the first page.
 */
function filterForm(filter: Filter): ReactElement {
  return (
    <form method="get" aria-label="Filter dead letters" className="flex flex-wrap items-end gap-2">
      <Field label="Event type" htmlFor="dead-letters-event-type">
        <Input name="eventType" defaultValue={filter.eventType} maxLength={MAX_EVENT_TYPE_LENGTH} autoComplete="off" />
      </Field>
      {parkedField('Parked from', 'dead-letters-parked-from', 'parkedFrom', filter.parkedFrom)}
      {parkedField('Parked to', 'dead-letters-parked-to', 'parkedTo', filter.parkedTo)}
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
  const parkedFrom = parseParkedBound(query.parkedFrom, 'parkedFrom');
  const parkedTo = parseParkedBound(query.parkedTo, 'parkedTo');
  const cursor = parseCursor(query.cursor);
  const frameKey = `${eventType.ok ? eventType.value : ''}|${parkedFrom.ok ? parkedFrom.iso : ''}|${parkedTo.ok ? parkedTo.iso : ''}|${cursor.ok ? cursor.cursor : ''}`;

  if (gate === 'degraded') {
    return shell(
      frameKey,
      <div data-testid="dead-letters-degraded">
        <ErrorState title={PRESENTATION_COPY.degraded.title} description={PRESENTATION_COPY.degraded.body} />
      </div>,
    );
  }

  // Every parser runs BEFORE the clients are built: a refused query never becomes an IAM call.
  if (!eventType.ok) return <PageError error={eventType.error} />;
  if (!cursor.ok) return <PageError error={cursor.error} />;
  const filter: Filter = { eventType: eventType.value, parkedFrom, parkedTo };
  // SMA-661 D6: a refused bound is a typing mistake, not a hand-edited cursor. The filter form comes
  // back with every typed value and the field's error. There is no table and no IAM call.
  if (!parkedFrom.ok || !parkedTo.ok) return shell(frameKey, filterForm(filter));

  const clients = await iamClients();
  const data = await loadDeadLettersPage({ outbox: clients.outbox }, { cursor: cursor.cursor, eventType: eventType.value, parkedFrom: parkedFrom.iso, parkedTo: parkedTo.iso });
  if (!data.ok) {
    // A fresh GET keeps its real 403 or 404. Every other list error stays inside the frame.
    if (data.error.presentation === 'forbidden' || data.error.presentation === 'not-found') return <PageError error={data.error} />;
    return shell(
      frameKey,
      <>
        {filterForm(filter)}
        <SectionError error={data.error} />
      </>,
    );
  }

  const { rows, nextCursor } = data.value;
  // The paging links carry the CANONICAL filter (SMA-661 spec § 4.5). listHref leaves out an empty value.
  const kept = { eventType: eventType.value, parkedFrom: parkedFrom.iso, parkedTo: parkedTo.iso };
  return shell(
    frameKey,
    <>
      {filterForm(filter)}
      {rows.length === 0 ? <EmptyState title="No dead letters" /> : <DeadLetterTable rows={rows} />}
      <nav aria-label="Dead-letter pages" className="flex gap-4 text-sm">
        {cursor.cursor === '' ? null : (
          <ZoneLink href={listHref(PATH, kept)} className="hover:underline">
            First page
          </ZoneLink>
        )}
        {nextCursor === null ? null : (
          <ZoneLink href={listHref(PATH, { ...kept, cursor: nextCursor })} className="hover:underline">
            Next
          </ZoneLink>
        )}
      </nav>
    </>,
  );
}
