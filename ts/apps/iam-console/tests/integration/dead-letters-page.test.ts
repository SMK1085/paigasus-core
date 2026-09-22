// SPDX-License-Identifier: Apache-2.0
//
// ListDeadLetters (SMA-629 spec § 6.3, § 7.3; SMA-661 spec § 7.3) through the fake IAM: every field of the row, IAM's
// "empty means none" strings as null, the filter and the cursor on the wire, the last page, and an
// IAM error as the PaigasusError.
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { denial, startFakeIam, type FakeIam } from '@paigasus/console-core/testing';
import { loadDeadLettersPage } from '../../app/(console)/dead-letters/load';
import { PAGE_SIZE } from '../../lib/paging';
import { IDS, callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const ID = '0190a1f0-0000-7000-8000-0000000000d1';
const TEAM_PRN = `prn:pgs:iam::${IDS.orgA}:team/${IDS.teamA1}`;

const entry = {
  id: ID,
  occurredAt: { seconds: 1_788_000_000n, nanos: 250_000_000 },
  eventType: 'iam.team.created',
  schemaVersion: 2,
  aggregatePrn: TEAM_PRN,
  actorPrn: IDS.principalPrn,
  payload: '{"slug":"platform"}',
  correlationId: 'corr-dead-letter-1',
  attempts: 5,
  parkedAt: { seconds: 1_788_000_300n, nanos: 0 },
  lastError: 'nats: no responders available for request',
};

describe('loadDeadLettersPage', () => {
  it('maps every field, sends the filter and the cursor, and returns the next cursor', async () => {
    iam.setHandlers({ 'outbox.listDeadLetters': () => ({ entries: [entry], nextCursor: 'cursor-2' }) });
    const calls = callsSince(iam);

    const data = await loadDeadLettersPage({ outbox: clientsFor(iam).outbox }, { cursor: 'cursor-1', eventType: 'iam.team.created', parkedFrom: '', parkedTo: '' });

    expect(data).toEqual({
      ok: true,
      value: {
        cursor: 'cursor-1',
        nextCursor: 'cursor-2',
        rows: [
          {
            id: ID,
            eventType: 'iam.team.created',
            aggregatePrn: TEAM_PRN,
            payload: '{"slug":"platform"}',
            schemaVersion: 2,
            attempts: 5,
            parkedAt: new Date(1_788_000_300_000).toISOString(),
            occurredAt: new Date(1_788_000_000_250).toISOString(),
            actorPrn: IDS.principalPrn,
            correlationId: 'corr-dead-letter-1',
            lastError: 'nats: no responders available for request',
          },
        ],
      },
    });
    expect(calls('outbox.listDeadLetters')[0]?.request).toMatchObject({ eventType: 'iam.team.created', cursor: 'cursor-1', limit: PAGE_SIZE });
  });

  it('sends the canonical parked bounds as Timestamps, and leaves an empty bound out (SMA-661 § 4.4)', async () => {
    iam.setHandlers({ 'outbox.listDeadLetters': () => ({ entries: [], nextCursor: '' }) });
    const calls = callsSince(iam);

    await loadDeadLettersPage({ outbox: clientsFor(iam).outbox }, { cursor: '', eventType: '', parkedFrom: '2026-09-19T00:00:00.250Z', parkedTo: '' });

    const request = calls('outbox.listDeadLetters')[0]?.request;
    expect(request).toMatchObject({ parkedFrom: { seconds: 1_789_776_000n, nanos: 250_000_000 } });
    expect((request as { parkedTo?: unknown }).parkedTo).toBeUndefined();
  });

  it("maps IAM's empty strings and missing timestamps to null, and an empty next_cursor to the last page", async () => {
    iam.setHandlers({
      'outbox.listDeadLetters': () => ({ entries: [{ ...entry, actorPrn: '', correlationId: '', lastError: '', parkedAt: undefined, occurredAt: undefined }], nextCursor: '' }),
    });

    const data = await loadDeadLettersPage({ outbox: clientsFor(iam).outbox }, { cursor: '', eventType: '', parkedFrom: '', parkedTo: '' });

    if (!data.ok) throw new Error('expected entries');
    expect(data.value.nextCursor).toBeNull();
    expect(data.value.rows[0]).toMatchObject({ actorPrn: null, correlationId: null, lastError: null, parkedAt: null, occurredAt: null });
  });

  it('reads a parked time outside the Date range as null, not a thrown error', async () => {
    iam.setHandlers({ 'outbox.listDeadLetters': () => ({ entries: [{ ...entry, parkedAt: { seconds: 8_640_000_000_001n, nanos: 0 } }], nextCursor: '' }) });

    const data = await loadDeadLettersPage({ outbox: clientsFor(iam).outbox }, { cursor: '', eventType: '', parkedFrom: '', parkedTo: '' });

    if (!data.ok) throw new Error('expected entries');
    expect(data.value.rows[0]?.parkedAt).toBeNull();
  });

  it('returns an IAM denial as the PaigasusError with the correlation id', async () => {
    iam.setHandlers({
      'outbox.listDeadLetters': () => {
        throw denial({ correlationId: 'corr-dead-letters-403' });
      },
    });

    const data = await loadDeadLettersPage({ outbox: clientsFor(iam).outbox }, { cursor: '', eventType: '', parkedFrom: '', parkedTo: '' });

    if (data.ok) throw new Error('expected a denial');
    expect(data.error.presentation).toBe('forbidden');
    expect(data.error.correlationId).toBe('corr-dead-letters-403');
  });
});
