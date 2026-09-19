// SPDX-License-Identifier: Apache-2.0
//
// The two dead-letter commands (SMA-629 spec § 6.4, § 7.3) against the fake IAM: each sends the id,
// and IAM's NotFound maps to `not-found`. A command takes no mayI and does no validation of its own:
// the action parses the form (tests/unit/actions-revalidate.test.ts holds the zero-call rule).
import { Code } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { denial, startFakeIam, type FakeIam } from '@paigasus/console-core/testing';
import { deadLetterForm, discardDeadLetter, replayDeadLetter } from '../../app/(console)/dead-letters/commands';
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
