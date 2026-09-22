// SPDX-License-Identifier: Apache-2.0
//
// The dead-letter commands (SMA-629 spec § 6.4, § 7.3; SMA-661 spec § 7.3) against the fake IAM: each sends the id,
// and IAM's NotFound maps to `not-found`. A command takes no mayI and does no validation of its own:
// the action parses the form (tests/unit/actions-revalidate.test.ts holds the zero-call rule).
import { Code } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { denial, startFakeIam, type FakeIam } from '@paigasus/console-core/testing';
import { bulkReplayDeadLetters, bulkReplayForm, deadLetterForm, discardDeadLetter, replayDeadLetter } from '../../app/(console)/dead-letters/commands';
import { callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const ID = '0190a1f0-0000-7000-8000-0000000000d1';

const COMMANDS = [
  ['replayDeadLetter', 'outbox.replayDeadLetter', replayDeadLetter],
  ['discardDeadLetter', 'outbox.discardDeadLetter', discardDeadLetter],
] as const;

describe.each(COMMANDS)('%s', (_name, method, command) => {
  it('sends the id and answers ok', async () => {
    iam.setHandlers({ 'outbox.replayDeadLetter': () => ({ entry: { id: ID } }), 'outbox.discardDeadLetter': () => ({ entry: { id: ID } }) });
    const calls = callsSince(iam);

    const result = await command({ outbox: clientsFor(iam).outbox }, { id: ID });

    expect(result).toEqual({ ok: true });
    expect(calls(method)).toHaveLength(1);
    expect(calls(method)[0]?.request).toMatchObject({ id: ID });
  });

  it('maps NotFound to not-found with the correlation id', async () => {
    const gone = (): never => {
      throw denial({ code: Code.NotFound, reason: 'not-found', correlationId: 'corr-dead-letter-404' });
    };
    iam.setHandlers({ 'outbox.replayDeadLetter': gone, 'outbox.discardDeadLetter': gone });

    const result = await command({ outbox: clientsFor(iam).outbox }, { id: ID });

    expect(result).toMatchObject({ ok: false, error: { presentation: 'not-found', correlationId: 'corr-dead-letter-404' } });
  });
});

describe('deadLetterForm', () => {
  it('trims and accepts an RFC 4122 id, and refuses anything else', () => {
    expect(deadLetterForm.safeParse({ id: ` ${ID} ` }).data).toEqual({ id: ID });
    for (const bad of ['', 'not-a-uuid', '0190a1f000007000800000000000d1', null]) {
      expect(deadLetterForm.safeParse({ id: bad }).success).toBe(false);
    }
  });
});

// SMA-661 spec § 6.5. The schema that stands between a form and IAM: it trims, normalises the
// bounds with the GET parser's own check, and refuses every max_rows IAM would clamp or misread.
describe('bulkReplayForm', () => {
  it('trims, normalises a bound, reads an absent bound as no filter, and reads the budget as a number', () => {
    expect(bulkReplayForm.safeParse({ eventType: ' iam.team.created ', parkedFrom: '2026-09-19 00:00Z', parkedTo: null, maxRows: ' 500 ' }).data).toEqual({
      eventType: 'iam.team.created',
      parkedFrom: '2026-09-19T00:00:00.000Z',
      parkedTo: '',
      maxRows: 500,
    });
    expect(bulkReplayForm.safeParse({ eventType: '', parkedFrom: '', parkedTo: '', maxRows: '10000' }).data?.maxRows).toBe(10_000);
  });

  it.each(['', '0', '10001', '100000', '7.5', '0x10', '0b1010', '+7', '7.', '1e3', null])('refuses the budget %j', (maxRows) => {
    expect(bulkReplayForm.safeParse({ eventType: '', parkedFrom: '', parkedTo: '', maxRows }).success).toBe(false);
  });

  it('refuses a bound with no zone, and a missing event type', () => {
    expect(bulkReplayForm.safeParse({ eventType: '', parkedFrom: '2026-09-19T00:00:00', parkedTo: '', maxRows: '5' }).success).toBe(false);
    expect(bulkReplayForm.safeParse({ eventType: null, parkedFrom: '', parkedTo: '', maxRows: '5' }).success).toBe(false);
  });
});

describe('bulkReplayDeadLetters', () => {
  it('sends the scope and the budget, leaves an empty bound out, and answers the count as a number', async () => {
    iam.setHandlers({ 'outbox.bulkReplayDeadLetters': () => ({ replayed: 8n }) });
    const calls = callsSince(iam);

    const result = await bulkReplayDeadLetters({ outbox: clientsFor(iam).outbox }, { eventType: 'iam.team.created', parkedFrom: '2026-09-19T00:00:00.000Z', parkedTo: '', maxRows: 500 });

    expect(result).toEqual({ ok: true, replayed: 8 });
    const request = calls('outbox.bulkReplayDeadLetters')[0]?.request;
    expect(request).toMatchObject({ eventType: 'iam.team.created', parkedFrom: { seconds: 1_789_776_000n, nanos: 0 }, maxRows: 500n });
    expect((request as { parkedTo?: unknown }).parkedTo).toBeUndefined();
  });

  it('sends both bounds when both are set', async () => {
    iam.setHandlers({ 'outbox.bulkReplayDeadLetters': () => ({ replayed: 0n }) });
    const calls = callsSince(iam);

    const result = await bulkReplayDeadLetters({ outbox: clientsFor(iam).outbox }, { eventType: '', parkedFrom: '2026-09-19T00:00:00.000Z', parkedTo: '2026-09-20T00:00:00.000Z', maxRows: 1 });

    expect(result).toEqual({ ok: true, replayed: 0 });
    expect(calls('outbox.bulkReplayDeadLetters')[0]?.request).toMatchObject({
      eventType: '',
      parkedFrom: { seconds: 1_789_776_000n, nanos: 0 },
      parkedTo: { seconds: 1_789_862_400n, nanos: 0 },
      maxRows: 1n,
    });
  });

  it('maps a denial to forbidden with the correlation id', async () => {
    iam.setHandlers({
      'outbox.bulkReplayDeadLetters': () => {
        throw denial({ correlationId: 'corr-bulk-replay-403' });
      },
    });

    const result = await bulkReplayDeadLetters({ outbox: clientsFor(iam).outbox }, { eventType: '', parkedFrom: '', parkedTo: '', maxRows: 5 });

    expect(result).toMatchObject({ ok: false, error: { presentation: 'forbidden', correlationId: 'corr-bulk-replay-403' } });
  });
});
