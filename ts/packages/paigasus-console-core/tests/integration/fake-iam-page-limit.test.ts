// SPDX-License-Identifier: Apache-2.0
//
// SMA-676 spec § 7.2. IAM's Page::new refuses a limit over 200 (application/pagination.rs:11) with
// InvalidArgument + invalid-pagination. ListMemberships always pages; ListRoleGrants pages only when
// a filter beyond the bare principal is set (D6). The fake must refuse the same requests, or a
// console test that asks for 500 rows passes against the fake and fails against IAM.
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { disposeTransports } from '@paigasus/sdk/iam';
import { PrincipalKind } from '@paigasus/sdk/iam/types';
import { callIam } from '../../src/errors';
import { createIamClients } from '../../src/iam-clients';
import { logger } from '../../src/logger';
import { startFakeIam, type FakeIam } from '@paigasus/console-core/testing';

const ORG = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a';
const ME = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000aa';

describe('the fake IAM page limit', () => {
  let fake: FakeIam;

  beforeEach(async () => {
    vi.spyOn(logger, 'appEvent').mockImplementation(() => undefined);
    fake = await startFakeIam({
      handlers: { 'tenancy.listMemberships': () => ({ memberships: [] }), 'authz.listRoleGrants': () => ({ grants: [] }) },
    });
  });
  afterEach(async () => {
    vi.restoreAllMocks();
    await fake.close();
  });
  afterAll(() => disposeTransports());

  const clients = () => createIamClients({ baseUrl: fake.grpcUrl, token: 'tok-page-limit' });

  it('refuses ListMemberships with limit 201 as invalid-pagination, and accepts 200', async () => {
    const refused = await callIam(() => clients().tenancy.listMemberships({ filter: { case: 'nodePrn', value: ORG }, limit: 201, offset: 0n }));
    expect(refused.ok).toBe(false);
    if (refused.ok) return;
    expect(refused.error.rawReason).toBe('invalid-pagination');
    expect(refused.error.transport).toMatchObject({ kind: 'grpc', code: 3 });
    expect((await callIam(() => clients().tenancy.listMemberships({ filter: { case: 'nodePrn', value: ORG }, limit: 200, offset: 0n }))).ok).toBe(true);
  });

  it.each([
    ['a scope', { scopePrn: ORG }],
    ['a role key', { principalPrn: ME, roleKey: 'gateway_user' }],
    ['a kind', { principalPrn: ME, principalKind: PrincipalKind.USER }],
  ] as const)('refuses ListRoleGrants with limit 201 when the request has %s', async (_label, filter) => {
    const refused = await callIam(() => clients().authz.listRoleGrants({ ...filter, limit: 201, offset: 0n }));
    expect(refused.ok).toBe(false);
    if (refused.ok) return;
    expect(refused.error.rawReason).toBe('invalid-pagination');
  });

  it('ignores the limit on the bare principal request, as IAM does (D6)', async () => {
    expect((await callIam(() => clients().authz.listRoleGrants({ principalPrn: ME, limit: 500, offset: 0n }))).ok).toBe(true);
  });
});
