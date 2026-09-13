// SPDX-License-Identifier: Apache-2.0
//
// mayI against the fake IAM over the wire: the question IAM receives is exactly
// (me, PascalCase action, resource), and a denial of the QUESTION itself fails open.
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { disposeTransports } from '@paigasus/sdk/iam';
import { createMayI } from '../../lib/authorize';
import { createIamClients } from '../../lib/iam-clients';
import { denial, startFakeIam, type FakeIam } from '../support/fake-iam';

const ORG = 'prn:pgs:iam::0192f1c0-0000-7000-8000-000000000002:organization/0192f1c0-0000-7000-8000-000000000002';

describe('mayI over the wire', () => {
  let fake: FakeIam;
  beforeEach(async () => {
    fake = await startFakeIam();
  });
  afterEach(() => fake.close());
  afterAll(() => disposeTransports());

  it('sends IAM the principal, the action and the resource, and returns its answer', async () => {
    fake.setHandlers({ 'authz.isAuthorized': (req) => ({ allowed: req.action === 'CreateTeam' }) });
    const me = fake.principalPrnFor('token-a');
    const mayI = createMayI({ authz: createIamClients({ baseUrl: fake.grpcUrl, token: 'token-a' }).authz, principalPrn: me, logger: { appEvent: vi.fn() } });
    expect(await mayI('CreateTeam', ORG)).toBe(true);
    expect(await mayI('DetachMembership', ORG)).toBe(false);
    expect(fake.callsTo('authz.isAuthorized').map((call) => call.request)).toEqual([
      expect.objectContaining({ principalPrn: me, action: 'CreateTeam', resourcePrn: ORG }),
      expect.objectContaining({ principalPrn: me, action: 'DetachMembership', resourcePrn: ORG }),
    ]);
  });

  it('fails open when IAM refuses the question itself', async () => {
    fake.setHandlers({
      'authz.isAuthorized': () => {
        throw denial();
      },
    });
    const appEvent = vi.fn();
    const mayI = createMayI({ authz: createIamClients({ baseUrl: fake.grpcUrl, token: 'token-a' }).authz, principalPrn: fake.principalPrnFor('token-a'), logger: { appEvent } });
    expect(await mayI('CreateOrganization', ORG)).toBe(true);
    expect(appEvent).toHaveBeenCalledWith('authorize.query_failed', { action: 'CreateOrganization', presentation: 'forbidden' });
  });
});
