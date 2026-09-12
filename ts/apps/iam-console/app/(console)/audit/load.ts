// SPDX-License-Identifier: Apache-2.0
//
// /iam/audit (spec § 6.6, AC 4). The screen exists only when IAM reports `iam.audit`. The gate is
// a pure function of the ServiceState, so a unit test covers every branch.
import 'server-only';
import { capabilityOutcome } from '@paigasus/discovery/client';
import type { ServiceState } from '@paigasus/discovery/types';
import { callIam, type IamResult } from '../../../lib/errors';
import type { IamClients } from '../../../lib/iam';

export type AuditGate = 'not-found' | 'degraded' | 'available';

/**
 * The branch table is @paigasus/discovery's capabilityOutcome: there is one copy. This maps its
 * answer to the page. Absent or available-without-the-capability is `hidden`, which is a 404.
 * Degraded is NOT a 404: the feature may exist.
 */
const GATE = { hidden: 'not-found', shown: 'available', degraded: 'degraded' } as const;

export function auditGate(state: ServiceState): AuditGate {
  return GATE[capabilityOutcome(state, 'iam.audit')];
}

export const AUDIT_PAGE_SIZE = 50;

export type AuditRow = {
  readonly id: string;
  /** ISO 8601, or null when IAM sent no timestamp. */
  readonly occurredAt: string | null;
  readonly actorPrn: string;
  readonly action: string;
  readonly resourcePrn: string;
  readonly outcome: string;
  readonly correlationId: string;
};

export type AuditPageData = IamResult<{ readonly rows: readonly AuditRow[]; readonly cursor: string; readonly nextCursor: string | null }>;

export async function loadAuditPage(deps: { readonly audit: Pick<IamClients['audit'], 'listAuditEntries'> }, params: { readonly cursor: string }): Promise<AuditPageData> {
  const result = await callIam(() => deps.audit.listAuditEntries({ cursor: params.cursor, limit: AUDIT_PAGE_SIZE }));
  if (!result.ok) return result;
  const rows = result.value.entries.map((entry): AuditRow => ({
    id: entry.id,
    occurredAt: entry.occurredAt === undefined ? null : new Date(Number(entry.occurredAt.seconds) * 1000 + Math.floor(entry.occurredAt.nanos / 1_000_000)).toISOString(),
    actorPrn: entry.actorPrn,
    action: entry.action,
    resourcePrn: entry.resourcePrn,
    outcome: entry.outcome,
    correlationId: entry.correlationId,
  }));
  return { ok: true, value: { rows, cursor: params.cursor, nextCursor: result.value.nextCursor === '' ? null : result.value.nextCursor } };
}
