// SPDX-License-Identifier: Apache-2.0
//
// SMA-636 spec § 4.5, § 7.1. IamClients carries a serviceAccounts client, and the fake IAM routes
// ServiceAccountService and records each call with its bearer and its correlation id.
//
// An unrouted service also answers Unimplemented, so "rejects" alone proves nothing about routing.
// The proof is the call log: the fake records a call only in its own dispatch.
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { createIamClients } from '../../src/iam-clients';
import { startFakeIam, type FakeIam } from '../../testing/index';

const OWNER = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a';
const SA = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000aa';
const CID = '0198f2c1-8888-7000-8000-000000000636';
const METHODS = ['createServiceAccount', 'getServiceAccount', 'listServiceAccounts', 'archiveServiceAccount', 'issueApiKey', 'revokeApiKey', 'listApiKeys'] as const;

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

describe('the serviceAccounts client', () => {
  it('reaches the fake ServiceAccountService with the bearer and the correlation id', async () => {
    iam.setHandlers({
      'serviceAccounts.createServiceAccount': (req) => ({ serviceAccount: { prn: SA, ownerPrn: req.ownerPrn, name: req.name, status: 'active' } }),
    });
    const clients = createIamClients({ baseUrl: iam.grpcUrl, token: 'tok-sa', correlationId: CID });

    const created = await clients.serviceAccounts.createServiceAccount({ ownerPrn: OWNER, name: 'ci-bot' });

    expect(created.serviceAccount?.prn).toBe(SA);
    const [call] = iam.callsTo('serviceAccounts.createServiceAccount');
    expect(call?.token).toBe('tok-sa');
    expect(call?.correlationId).toBe(CID);
    expect(call?.request).toMatchObject({ ownerPrn: OWNER, name: 'ci-bot' });
  });

  it('routes all seven RPCs, and an unscripted one answers Unimplemented', async () => {
    iam.setHandlers({});
    const clients = createIamClients({ baseUrl: iam.grpcUrl, token: 'tok-sa' });
    const before = Object.fromEntries(METHODS.map((method) => [method, iam.callsTo(`serviceAccounts.${method}`).length]));

    const results = await Promise.allSettled([
      clients.serviceAccounts.createServiceAccount({ ownerPrn: OWNER, name: 'x' }),
      clients.serviceAccounts.getServiceAccount({ prn: SA }),
      clients.serviceAccounts.listServiceAccounts({ ownerPrn: OWNER, limit: 51, offset: 0n }),
      clients.serviceAccounts.archiveServiceAccount({ prn: SA }),
      clients.serviceAccounts.issueApiKey({ serviceAccountPrn: SA, scopePrn: OWNER }),
      clients.serviceAccounts.revokeApiKey({ id: 'key-1' }),
      clients.serviceAccounts.listApiKeys({ serviceAccountPrn: SA, limit: 51, offset: 0n }),
    ]);

    for (const result of results) {
      expect(result.status).toBe('rejected');
      if (result.status === 'rejected') expect((result.reason as ConnectError).code).toBe(Code.Unimplemented);
    }
    for (const method of METHODS) {
      expect(iam.callsTo(`serviceAccounts.${method}`).length - (before[method] ?? 0)).toBe(1);
    }
  });
});
