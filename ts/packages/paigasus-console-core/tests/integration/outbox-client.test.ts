// SPDX-License-Identifier: Apache-2.0
//
// SMA-629 spec § 5.3. IamClients carries an outbox client, and the fake IAM routes OutboxService
// and records each call with its bearer and its correlation id.
//
// An unrouted service also answers Unimplemented, so "rejects" alone proves nothing about routing.
// The proof is the call log: the fake records a call only in its own dispatch.
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { createIamClients } from '../../src/iam-clients';
import { startFakeIam, type FakeIam } from '../../testing/index';

const ID = '0190a1f0-0000-7000-8000-0000000000d1';
const CID = '0198f2c1-8888-7000-8000-000000000629';
const METHODS = ['listDeadLetters', 'replayDeadLetter', 'bulkReplayDeadLetters', 'discardDeadLetter'] as const;

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

describe('the outbox client', () => {
  it('reaches the fake OutboxService with the bearer and the correlation id', async () => {
    iam.setHandlers({
      'outbox.listDeadLetters': (req) => ({ entries: [{ id: ID, eventType: req.eventType }], nextCursor: '' }),
    });
    const clients = createIamClients({ baseUrl: iam.grpcUrl, token: 'tok-outbox', correlationId: CID });

    const listed = await clients.outbox.listDeadLetters({ eventType: 'iam.team.created', cursor: '', limit: 50 });

    expect(listed.entries.map((entry) => entry.id)).toEqual([ID]);
    const [call] = iam.callsTo('outbox.listDeadLetters');
    expect(call?.token).toBe('tok-outbox');
    expect(call?.correlationId).toBe(CID);
    expect(call?.request).toMatchObject({ eventType: 'iam.team.created', limit: 50 });
  });

  it('routes all four RPCs, and an unscripted one answers Unimplemented', async () => {
    iam.setHandlers({});
    const clients = createIamClients({ baseUrl: iam.grpcUrl, token: 'tok-outbox' });
    const before = Object.fromEntries(METHODS.map((method) => [method, iam.callsTo(`outbox.${method}`).length]));

    const results = await Promise.allSettled([
      clients.outbox.listDeadLetters({}),
      clients.outbox.replayDeadLetter({ id: ID }),
      clients.outbox.bulkReplayDeadLetters({ maxRows: 1n }),
      clients.outbox.discardDeadLetter({ id: ID }),
    ]);

    for (const result of results) {
      expect(result.status).toBe('rejected');
      if (result.status === 'rejected') expect((result.reason as ConnectError).code).toBe(Code.Unimplemented);
    }
    for (const method of METHODS) {
      expect(iam.callsTo(`outbox.${method}`).length - (before[method] ?? 0)).toBe(1);
    }
  });
});
