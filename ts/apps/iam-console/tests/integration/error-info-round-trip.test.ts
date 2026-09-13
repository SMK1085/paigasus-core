// SPDX-License-Identifier: Apache-2.0
//
// The ErrorInfo trailer round trip over the WIRE (spec § 9.3). The SDK's own tests build the
// ConnectError in memory and say they do not test the wire (ts/packages/paigasus-sdk/tests/map-error.test.ts:10-15).
// Here a real h2c call reaches the fake IAM, which denies it the way IAM does, and callIam maps
// what came back. It also proves the correlation-id header route of spec § 6.2: the id the console
// sends is the id IAM puts into the error.
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ErrorDomain, ErrorReason } from '@paigasus/sdk/errors';
import { disposeTransports } from '@paigasus/sdk/iam';
import { callIam } from '../../lib/errors';
import { createIamClients } from '../../lib/iam-clients';
import { logger } from '../../lib/logger';
import { denial, startFakeIam, type FakeIam } from '../support/fake-iam';

const ORG = 'prn:pgs:iam::0192f1c0-0000-7000-8000-000000000002:organization/0192f1c0-0000-7000-8000-000000000002';
const REQUEST_ID = '0198f2c1-8888-7000-8000-00000000beef';

describe('the ErrorInfo round trip', () => {
  let fake: FakeIam;

  beforeEach(async () => {
    // callIam writes one real JSON line per failed call, and these cases fail on purpose.
    vi.spyOn(logger, 'appEvent').mockImplementation(() => undefined);
    fake = await startFakeIam({
      handlers: {
        'tenancy.getOrganization': () => {
          throw denial();
        },
      },
    });
  });
  afterEach(async () => {
    vi.restoreAllMocks();
    await fake.close();
  });
  afterAll(() => disposeTransports());

  it('maps a wire denial to forbidden, with the reason, the domain and the id the console sent', async () => {
    const clients = createIamClients({ baseUrl: fake.grpcUrl, token: 'token-a', correlationId: REQUEST_ID });
    const result = await callIam(() => clients.tenancy.getOrganization({ prn: ORG }));
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error).toMatchObject({ presentation: 'forbidden', reason: ErrorReason.FORBIDDEN, domain: ErrorDomain.IAM, retryable: false, correlationId: REQUEST_ID });
    expect(fake.callsTo('tenancy.getOrganization')).toEqual([expect.objectContaining({ token: 'token-a', correlationId: REQUEST_ID })]);
  });

  it('sends the correlation header from every one of the five clients', async () => {
    const clients = createIamClients({ baseUrl: fake.grpcUrl, token: 'token-a', correlationId: REQUEST_ID });
    // Some calls fail (the beforeEach handler denies getOrganization, and the fake has no default
    // for listAuditEntries). That does not matter here: the fake records each call as it ARRIVED,
    // before it runs a handler, so the header of every call is in fake.calls.
    await callIam(() => clients.tenancy.getOrganization({ prn: ORG }));
    await callIam(() => clients.serviceInfo.getServiceInfo({}));
    await callIam(() => clients.authn.introspect({ token: 'token-a' }));
    await callIam(() => clients.authz.isAuthorized({ principalPrn: fake.principalPrnFor('token-a'), action: 'ListOrganizations', resourcePrn: ORG }));
    await callIam(() => clients.audit.listAuditEntries({}));
    const sent = fake.calls.map((call) => [call.method, call.correlationId]);
    expect(sent).toEqual([
      ['tenancy.getOrganization', REQUEST_ID],
      ['serviceInfo.getServiceInfo', REQUEST_ID],
      ['authn.introspect', REQUEST_ID],
      ['authz.isAuthorized', REQUEST_ID],
      ['audit.listAuditEntries', REQUEST_ID],
    ]);
  });

  it('sends no correlation header when the request has none, and IAM then mints one', async () => {
    const clients = createIamClients({ baseUrl: fake.grpcUrl, token: 'token-a' });
    const result = await callIam(() => clients.tenancy.getOrganization({ prn: ORG }));
    expect(fake.calls[0]?.correlationId).toBeNull();
    expect(result.ok ? null : result.error.correlationId).toMatch(/^[0-9a-f-]{36}$/);
  });
});
