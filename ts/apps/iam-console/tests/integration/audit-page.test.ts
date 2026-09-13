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

  // Review, defect 6 — reported as "Number(seconds) rounds BEFORE the range check, so a big value
  // renders a WRONG timestamp". MEASURED false, and pinned here rather than left to prose.
  // `Number(bigint)` is exact up to 2^53 = 9,007,199,254,740,992, and Date accepts at most
  // ±8,640,000,000,000,000 ms, i.e. ±8,640,000,000,000 SECONDS. Every seconds value that can yield
  // a valid Date is therefore three orders of magnitude below the point where precision is lost,
  // and every value that loses precision is three orders of magnitude past the ceiling, so
  // `Number.isNaN(date.getTime())` already rejects it. A sweep of the boundaries plus 200,000
  // random int64 seconds found zero disagreements with exact bigint arithmetic.
  it.each([
    ['the exact Date ceiling', 8_640_000_000_000n, new Date(8_640_000_000_000_000).toISOString()],
    ['one second past the ceiling', 8_640_000_000_001n, null],
    ['the first seconds value Number() cannot represent', 9_007_199_254_740_993n, null],
  ])('reads %s correctly, never as a rounded-but-valid timestamp', async (_label, seconds, expected) => {
    iam.setHandlers({ 'audit.listAuditEntries': () => ({ entries: [{ ...entry, occurredAt: { seconds, nanos: 0 } }], nextCursor: '' }) });

    const data = await loadAuditPage({ audit: clientsFor(iam).audit }, { cursor: '' });

    if (!data.ok) throw new Error('expected entries');
    expect(data.value.rows[0]?.occurredAt).toBe(expected);
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
