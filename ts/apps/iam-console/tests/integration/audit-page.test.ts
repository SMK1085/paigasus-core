// SPDX-License-Identifier: Apache-2.0
//
// ListAuditEntries (spec § 5.2) through the fake IAM: cursor paging with next_cursor, and the 403 of
// a principal that is not root (application/audit.rs:36-38) as a page error.
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { AUDIT_PAGE_SIZE, loadAuditPage } from '../../app/(console)/audit/load';
import { denial, startFakeIam, type FakeIam } from '../support/fake-iam';
import { IDS, callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const entry = {
  id: 'audit-1',
  occurredAt: { seconds: 1_788_000_000n, nanos: 250_000_000 },
  actorPrn: IDS.principalPrn,
  action: 'CreateTeam',
  resourcePrn: 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a',
  outcome: 'allow',
  determiningPolicies: [],
  detailJson: '{}',
  correlationId: 'corr-audit-1',
};

describe('loadAuditPage', () => {
  it('maps the entries, forwards the cursor, and returns the next cursor', async () => {
    iam.setHandlers({ 'audit.listAuditEntries': () => ({ entries: [entry], nextCursor: 'cursor-2' }) });
    const calls = callsSince(iam);

    const data = await loadAuditPage({ audit: clientsFor(iam).audit }, { cursor: 'cursor-1' });

    expect(data).toEqual({
      ok: true,
      value: {
        cursor: 'cursor-1',
        nextCursor: 'cursor-2',
        rows: [
          {
            id: 'audit-1',
            occurredAt: new Date(1_788_000_000_250).toISOString(),
            actorPrn: IDS.principalPrn,
            action: 'CreateTeam',
            resourcePrn: entry.resourcePrn,
            outcome: 'allow',
            correlationId: 'corr-audit-1',
          },
        ],
      },
    });
    expect(calls('audit.listAuditEntries')[0]?.request).toMatchObject({ cursor: 'cursor-1', limit: AUDIT_PAGE_SIZE });
  });

  it('reads an empty next_cursor as the last page, and an absent timestamp as null', async () => {
    iam.setHandlers({ 'audit.listAuditEntries': () => ({ entries: [{ ...entry, occurredAt: undefined }], nextCursor: '' }) });

    const data = await loadAuditPage({ audit: clientsFor(iam).audit }, { cursor: '' });

    if (!data.ok) throw new Error('expected entries');
    expect(data.value.nextCursor).toBeNull();
    expect(data.value.rows[0]?.occurredAt).toBeNull();
  });

  it('reads a seconds value outside the ECMAScript Date range as null, not a thrown error', async () => {
    // 1e15 seconds is far past Date's +/-8,640,000,000,000 ms ceiling; `new Date(...).toISOString()`
    // throws RangeError on a value like this if it is not validated first.
    iam.setHandlers({ 'audit.listAuditEntries': () => ({ entries: [{ ...entry, occurredAt: { seconds: 1_000_000_000_000_000n, nanos: 0 } }], nextCursor: '' }) });

    const data = await loadAuditPage({ audit: clientsFor(iam).audit }, { cursor: '' });

    if (!data.ok) throw new Error('expected entries');
    expect(data.value.rows[0]?.occurredAt).toBeNull();
  });

  it('returns a denial as a page error with the correlation id', async () => {
    iam.setHandlers({
      'audit.listAuditEntries': () => {
        throw denial({ correlationId: 'corr-audit-403' });
      },
    });

    const data = await loadAuditPage({ audit: clientsFor(iam).audit }, { cursor: '' });

    if (data.ok) throw new Error('expected a denial');
    expect(data.error.presentation).toBe('forbidden');
    expect(data.error.correlationId).toBe('corr-audit-403');
  });
});
