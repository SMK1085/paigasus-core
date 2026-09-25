// SPDX-License-Identifier: Apache-2.0
//
// SMA-635 spec § 7.2. Real IAM answers IntrospectApiKey for a bearer that is not an API key with
// Unauthenticated + reason invalid-token (paigasus-iam convert.rs:141). The fake's default now does
// the same. Before, it answered Unimplemented, which the gateway maps to an OUTAGE, so every
// playground request took the 503 branch.
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { disposeTransports } from '@paigasus/sdk/iam';
import { callIam } from '../../src/errors';
import { createIamClients } from '../../src/iam-clients';
import { logger } from '../../src/logger';
import { startFakeIam, type FakeIam } from '@paigasus/console-core/testing';

describe('the fake IAM introspectApiKey default', () => {
  let fake: FakeIam;

  beforeEach(async () => {
    vi.spyOn(logger, 'appEvent').mockImplementation(() => undefined);
    fake = await startFakeIam();
  });
  afterEach(async () => {
    vi.restoreAllMocks();
    await fake.close();
  });
  afterAll(() => disposeTransports());

  it('answers Unauthenticated with the reason invalid-token, as IAM does', async () => {
    const clients = createIamClients({ baseUrl: fake.grpcUrl, token: 'unused' });
    const result = await callIam(() => clients.authn.introspectApiKey({ token: 'an-oidc-token' }));
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error.rawReason).toBe('invalid-token');
    expect(result.error.transport).toMatchObject({ kind: 'grpc', code: 16 });
  });

  it('still lets a scripted handler answer', async () => {
    fake.setHandlers({
      'authn.introspectApiKey': () => ({
        principalPrn: 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000aa',
        status: 'active',
        keyId: 'k-1',
        scopePrn: 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000e1',
      }),
    });
    const clients = createIamClients({ baseUrl: fake.grpcUrl, token: 'unused' });
    const result = await callIam(() => clients.authn.introspectApiKey({ token: 'pgs_sk_x' }));
    expect(result.ok).toBe(true);
  });
});
