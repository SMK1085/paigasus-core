// SPDX-License-Identifier: Apache-2.0
//
// /iam/dead-letters (SMA-629 spec § 6.2, § 6.3). The screen exists only when IAM reports
// `iam.deadletters`. The gate is a pure function of the ServiceState, so a unit test covers every
// branch. The loader turns IAM's DeadLetterEntry into plain data for the page. SMA-661 added the
// parked-time window; parkedWindow() is shared with the bulk-replay command.
import 'server-only';
import { capabilityOutcome } from '@paigasus/discovery/client';
import type { ServiceState } from '@paigasus/discovery/types';
import { callIam, type IamClients, type IamResult } from '@paigasus/console-core';
import { PAGE_SIZE } from '../../../lib/paging';
import { timestampFromIso, timestampIso, type ProtoTimestamp } from '../../../lib/time';

export type DeadLettersGate = 'not-found' | 'degraded' | 'available';

/**
 * The audit mapping, applied to `iam.deadletters`. Absent, or available without the key (an older
 * IAM, AC 2), is `hidden`, which is a 404. Degraded is NOT a 404, whatever the cached descriptor
 * says (§ 8): the feature may exist.
 */
const GATE = { hidden: 'not-found', shown: 'available', degraded: 'degraded' } as const;

export function deadLettersGate(state: ServiceState): DeadLettersGate {
  return GATE[capabilityOutcome(state, 'iam.deadletters')];
}

/** One dead letter as the page shows it. `null` is IAM's "none", shown as "—". */
export type DeadLetterRow = {
  readonly id: string;
  readonly eventType: string;
  readonly aggregatePrn: string;
  /** A JSON string. The page never parses it and never renders it as HTML (§ 6.3). */
  readonly payload: string;
  readonly schemaVersion: number;
  readonly attempts: number;
  /** ISO 8601, or null for a missing or out-of-range timestamp. */
  readonly parkedAt: string | null;
  readonly occurredAt: string | null;
  readonly actorPrn: string | null;
  readonly correlationId: string | null;
  readonly lastError: string | null;
};

export type DeadLettersPageData = IamResult<{ readonly rows: readonly DeadLetterRow[]; readonly cursor: string; readonly nextCursor: string | null }>;

/** The proto says "empty means none" for actor_prn, correlation_id and last_error (iam.proto:619,621,624). */
function noneIfEmpty(value: string): string | null {
  return value === '' ? null : value;
}

/** The list filter's parked-time window: CANONICAL ISO instants, '' for no bound (SMA-661 spec § 4.4). */
export type ParkedWindow = { readonly parkedFrom: string; readonly parkedTo: string };

/**
 * The window as request fields, for the list and the bulk replay alike. An empty bound is LEFT OUT,
 * and IAM reads an absent timestamp as no filter (iam.proto:662-665). The page and the bulk form's
 * schema refused every value that lib/paging.ts's canonicalParkedBound does not accept, so the
 * conversion cannot fail.
 */
export function parkedWindow(bounds: ParkedWindow): { parkedFrom?: ProtoTimestamp; parkedTo?: ProtoTimestamp } {
  return {
    ...(bounds.parkedFrom === '' ? {} : { parkedFrom: timestampFromIso(bounds.parkedFrom) }),
    ...(bounds.parkedTo === '' ? {} : { parkedTo: timestampFromIso(bounds.parkedTo) }),
  };
}

/**
 * IAM orders the list by id DESCENDING (tests/dead_letters_pg.rs:432), and IAM mints UUIDv7 ids, so
 * this is close to creation order, not park order. The page keeps that order and does not sort.
 */
export async function loadDeadLettersPage(
  deps: { readonly outbox: Pick<IamClients['outbox'], 'listDeadLetters'> },
  params: { readonly cursor: string; readonly eventType: string } & ParkedWindow,
): Promise<DeadLettersPageData> {
  const result = await callIam(() => deps.outbox.listDeadLetters({ eventType: params.eventType, cursor: params.cursor, limit: PAGE_SIZE, ...parkedWindow(params) }));
  if (!result.ok) return result;
  const rows = result.value.entries.map((entry): DeadLetterRow => ({
    id: entry.id,
    eventType: entry.eventType,
    aggregatePrn: entry.aggregatePrn,
    payload: entry.payload,
    schemaVersion: entry.schemaVersion,
    attempts: entry.attempts,
    parkedAt: timestampIso(entry.parkedAt),
    occurredAt: timestampIso(entry.occurredAt),
    actorPrn: noneIfEmpty(entry.actorPrn),
    correlationId: noneIfEmpty(entry.correlationId),
    lastError: noneIfEmpty(entry.lastError),
  }));
  return { ok: true, value: { rows, cursor: params.cursor, nextCursor: result.value.nextCursor === '' ? null : result.value.nextCursor } };
}
