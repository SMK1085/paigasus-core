// SPDX-License-Identifier: Apache-2.0
//
// Provisioning (spec § 4.5, § 9.3), against the fake IAM over the real SDK transport:
//   - the login resolver calls GetServiceInfo BEFORE Introspect, and never fails the login;
//   - introspectWithProvisioning retries ONCE after identity-not-provisioned.
import { afterAll, afterEach, beforeEach, describe, expect, it } from 'vitest';
import { ErrorReason } from '@paigasus/sdk/errors';
import { disposeTransports } from '@paigasus/sdk/iam';
import { createIamClients } from '../../lib/iam-clients';
import { createJsonLogger } from '../../lib/logger';
import { introspectWithProvisioning } from '../../lib/principal';
import { createIntrospectPrincipalResolver } from '../../lib/principal-resolver';
import { denial, FAKE_IAM_ISSUER, startFakeIam, type FakeIam } from '../support/fake-iam';

const ORG = 'prn:pgs:iam::0192f1c0-0000-7000-8000-000000000002:organization/0192f1c0-0000-7000-8000-000000000002';
const CLAIMS = { iss: 'https://idp.example.test', sub: 'user-1' };

function captureLogger() {
  const lines: string[] = [];
  return { logger: createJsonLogger((line) => lines.push(line)), events: () => lines.map((line) => JSON.parse(line) as { event: string; fields: Record<string, unknown> }) };
}

describe('provisioning', () => {
  let fake: FakeIam;
  const clientsFor = (token: string) => createIamClients({ baseUrl: fake.grpcUrl, token });

  beforeEach(async () => {
    fake = await startFakeIam();
  });
  afterEach(() => fake.close());
  afterAll(() => disposeTransports());

  describe('the login resolver', () => {
    it('provisions with GetServiceInfo first, then maps Introspect, reporting grants as unknown', async () => {
      fake.setHandlers({ 'authn.introspect': (req) => ({ memberships: [{ id: 'm-1', principalPrn: fake.principalPrnFor(req.token), nodePrn: ORG }] }) });
      const { logger, events } = captureLogger();
      const resolver = createIntrospectPrincipalResolver({ clientsForToken: clientsFor, logger });
      const principal = await resolver.resolve({ accessToken: 'first-login', idTokenClaims: CLAIMS });
      expect(fake.calls.map((call) => call.method)).toEqual(['serviceInfo.getServiceInfo', 'authn.introspect']);
      expect(principal).toEqual({
        principalPrn: fake.principalPrnFor('first-login'),
        issuer: FAKE_IAM_ISSUER,
        subject: expect.any(String) as string,
        memberships: [{ id: 'm-1', principalPrn: fake.principalPrnFor('first-login'), nodePrn: ORG }],
        roleGrants: [],
        grantsAvailable: false,
      });
      expect(events()).toEqual([]);
    });

    it('degrades to a null principal and logs principal.resolve_failed when IAM refuses', async () => {
      fake.setHandlers({
        'serviceInfo.getServiceInfo': () => {
          throw denial();
        },
      });
      const { logger, events } = captureLogger();
      const principal = await createIntrospectPrincipalResolver({ clientsForToken: clientsFor, logger }).resolve({ accessToken: 't', idTokenClaims: CLAIMS });
      expect(principal).toEqual({ principalPrn: null, issuer: CLAIMS.iss, subject: CLAIMS.sub, memberships: [], roleGrants: [], grantsAvailable: false });
      expect(events()).toContainEqual({ event: 'principal.resolve_failed', fields: { presentation: 'forbidden' }, time: expect.any(String) as string });
    });

    it('degrades within its short timeout when IAM does not answer', async () => {
      fake.setHandlers({ 'serviceInfo.getServiceInfo': () => new Promise(() => undefined) });
      const { logger, events } = captureLogger();
      const started = Date.now();
      const principal = await createIntrospectPrincipalResolver({ clientsForToken: clientsFor, logger, timeoutMs: 200 }).resolve({ accessToken: 't', idTokenClaims: CLAIMS });
      expect(Date.now() - started).toBeLessThan(2_000);
      expect(principal.principalPrn).toBeNull();
      expect(events().map((e) => e.fields['presentation'])).toEqual(['degraded']);
    });

    it('degrades when the client factory itself throws', async () => {
      const { logger } = captureLogger();
      const resolver = createIntrospectPrincipalResolver({
        clientsForToken: () => {
          throw new Error('no config');
        },
        logger,
      });
      expect((await resolver.resolve({ accessToken: 't', idTokenClaims: CLAIMS })).principalPrn).toBeNull();
    });
  });

  describe('introspectWithProvisioning', () => {
    it('retries ONCE after identity-not-provisioned, provisioning in between', async () => {
      const result = await introspectWithProvisioning(clientsFor('new-user'), 'new-user', { provisionFirst: false });
      expect(result).toEqual({ ok: true, value: { prn: fake.principalPrnFor('new-user'), memberships: [] } });
      expect(fake.calls.map((call) => call.method)).toEqual(['authn.introspect', 'serviceInfo.getServiceInfo', 'authn.introspect']);
    });

    it('makes one Introspect call for a provisioned user', async () => {
      fake.provisioned.add('known-user');
      await introspectWithProvisioning(clientsFor('known-user'), 'known-user');
      expect(fake.calls.map((call) => call.method)).toEqual(['authn.introspect']);
    });

    it('returns the second identity-not-provisioned as an error instead of looping', async () => {
      // The fake provisions a token BEFORE it runs the handler, so this handler undoes it: the
      // provisioning call succeeds and the retry still finds no principal.
      fake.setHandlers({
        'serviceInfo.getServiceInfo': (_req, ctx) => {
          if (ctx.token !== null) fake.provisioned.delete(ctx.token);
          return { serviceInfo: { service: 'iam', version: 'x', capabilities: [] } };
        },
      });
      const result = await introspectWithProvisioning(clientsFor('ghost'), 'ghost');
      expect(result.ok ? null : result.error.reason).toBe(ErrorReason.IDENTITY_NOT_PROVISIONED);
      expect(fake.callsTo('authn.introspect')).toHaveLength(2);
    });
  });
});
