// SPDX-License-Identifier: Apache-2.0
//
// The fake IAM's self-test: each IAM behaviour the console depends on (tests/support/fake-iam.ts's
// header lists them) is observable through the REAL SDK transport, over h2c.
import { connect } from 'node:net';
import { afterAll, afterEach, beforeEach, describe, expect, it } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { ErrorDomain, ErrorReason, mapError } from '@paigasus/sdk/errors';
import { AuthnService, createIamClient, disposeTransports, ServiceInfoService, TenancyService } from '@paigasus/sdk/iam';
import { denial, errorInfoOf, IAM_ERROR_DOMAIN, startFakeIam, type FakeIam } from '../../support/fake-iam';

const DENIED = 'prn:pgs:iam::0192f1c0-0000-7000-8000-000000000002:organization/0192f1c0-0000-7000-8000-000000000002';
const ALLOWED = 'prn:pgs:iam::0192f1c0-0000-7000-8000-000000000009:organization/0192f1c0-0000-7000-8000-000000000009';
const SENT_ID = '0198f2c1-8888-7000-8000-000000000042';

/** Whether a TCP connection to the port of `url` is accepted. A closed listener refuses it. */
function accepts(url: string): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = connect(Number(new URL(url).port), '127.0.0.1');
    socket.once('connect', () => {
      socket.destroy();
      resolve(true);
    });
    socket.once('error', () => resolve(false));
  });
}

async function rejection(promise: Promise<unknown>): Promise<ConnectError> {
  try {
    await promise;
  } catch (err) {
    if (err instanceof ConnectError) return err;
    throw err;
  }
  throw new Error('expected the call to fail');
}

describe('the fake IAM', () => {
  let fake: FakeIam;

  beforeEach(async () => {
    fake = await startFakeIam();
  });
  afterEach(() => fake.close());
  afterAll(() => disposeTransports());

  it('serves a scripted answer and records the call with its token', async () => {
    fake.setHandlers({ 'tenancy.getOrganization': (req) => ({ organization: { prn: req.prn, slug: 'acme', name: 'Acme' } }) });
    const tenancy = createIamClient(TenancyService, { baseUrl: fake.grpcUrl }, { bearer: 'token-a' });
    const answer = await tenancy.getOrganization({ prn: ALLOWED });
    expect(answer.organization?.name).toBe('Acme');
    expect(fake.callsTo('tenancy.getOrganization')).toEqual([expect.objectContaining({ token: 'token-a', correlationId: null })]);
  });

  it('builds a denial the SDK maps to forbidden, carrying the correlation id it ADOPTED', async () => {
    fake.setHandlers({
      'tenancy.getOrganization': (req) => {
        if (req.prn === DENIED) throw denial();
        return { organization: { prn: req.prn } };
      },
    });
    const tenancy = createIamClient(TenancyService, { baseUrl: fake.grpcUrl }, { bearer: 'token-a' });
    const raw = await rejection(tenancy.getOrganization({ prn: DENIED }, { headers: { 'paigasus-correlation-id': SENT_ID } }));
    const error = mapError({ kind: 'grpc', error: raw });
    expect(error).toMatchObject({ presentation: 'forbidden', reason: ErrorReason.FORBIDDEN, rawReason: 'forbidden', domain: ErrorDomain.IAM, retryable: false, correlationId: SENT_ID });
    // The ErrorInfo DETAIL, read directly. mapError prefers this `correlation_id` over the
    // response header, and the fake sets both to the same value — so only this assertion can tell
    // a fake that stamps the detail from one that relies on the header (MEASURED).
    expect(errorInfoOf(raw)).toEqual({ reason: 'forbidden', domain: IAM_ERROR_DOMAIN, metadata: { retryable: 'false', correlation_id: SENT_ID } });
    expect(fake.callsTo('tenancy.getOrganization')[0]?.correlationId).toBe(SENT_ID);
  });

  it('mints a correlation id when the incoming one is not a UUID', async () => {
    fake.setHandlers({
      'tenancy.getOrganization': () => {
        throw denial();
      },
    });
    const tenancy = createIamClient(TenancyService, { baseUrl: fake.grpcUrl }, { bearer: 'token-a' });
    const error = mapError({ kind: 'grpc', error: await rejection(tenancy.getOrganization({ prn: DENIED }, { headers: { 'paigasus-correlation-id': 'not-a-uuid' } })) });
    expect(error.correlationId).toMatch(/^[0-9a-f-]{36}$/);
    expect(error.correlationId).not.toBe('not-a-uuid');
  });

  it('answers identity-not-provisioned until the token makes a bearer-enforced call', async () => {
    const authn = createIamClient(AuthnService, { baseUrl: fake.grpcUrl }, { anonymous: true });
    const before = mapError({ kind: 'grpc', error: await rejection(authn.introspect({ token: 'token-b' })) });
    expect(before).toMatchObject({ presentation: 'forbidden', reason: ErrorReason.IDENTITY_NOT_PROVISIONED });

    const info = createIamClient(ServiceInfoService, { baseUrl: fake.grpcUrl }, { bearer: 'token-b' });
    expect((await info.getServiceInfo({})).serviceInfo?.service).toBe('iam');
    expect(fake.provisioned.has('token-b')).toBe(true);

    const after = await authn.introspect({ token: 'token-b' });
    expect(after.principalPrn).toBe(fake.principalPrnFor('token-b'));
    expect(after.roleGrants).toEqual([]);
  });

  it('keeps role_grants empty even when a script returns some', async () => {
    fake.provisioned.add('token-c');
    fake.setHandlers({ 'authn.introspect': () => ({ roleGrants: [{ scopePrn: ALLOWED, roleKey: 'org_admin' }], memberships: [{ id: 'm1', principalPrn: 'p', nodePrn: ALLOWED }] }) });
    const authn = createIamClient(AuthnService, { baseUrl: fake.grpcUrl }, { anonymous: true });
    const answer = await authn.introspect({ token: 'token-c' });
    expect(answer.roleGrants).toEqual([]);
    expect(answer.memberships.map((m) => m.nodePrn)).toEqual([ALLOWED]);
  });

  it('refuses a bearer-enforced call that carries no token', async () => {
    const tenancy = createIamClient(TenancyService, { baseUrl: fake.grpcUrl }, { anonymous: true });
    const error = mapError({ kind: 'grpc', error: await rejection(tenancy.listOrganizations({})) });
    expect(error.presentation).toBe('relogin');
  });

  it('serves GET /v1/service-info over HTTP, and a status override only there', async () => {
    const probe = (token: string | null) => fetch(`${fake.httpUrl}/v1/service-info`, { headers: token === null ? {} : { authorization: `Bearer ${token}` } });
    expect((await probe(null)).status).toBe(401);
    const ok = await probe('token-d');
    expect(ok.headers.get('content-type')).toBe('application/json');
    expect(await ok.json()).toEqual({ service: 'iam', version: '0.0.0-fake', capabilities: ['iam.authz.cedar', 'iam.audit'] });
    expect(fake.provisioned.has('token-d')).toBe(true);

    fake.setServiceInfo({ status: 503 });
    expect((await probe('token-d')).status).toBe(503);
    const info = createIamClient(ServiceInfoService, { baseUrl: fake.grpcUrl }, { bearer: 'token-d' });
    expect((await info.getServiceInfo({})).serviceInfo?.capabilities).toEqual(['iam.authz.cedar', 'iam.audit']);

    fake.setServiceInfo({ service: 'iam', version: '9.9.9', capabilities: [] });
    expect(await (await probe('token-d')).json()).toEqual({ service: 'iam', version: '9.9.9', capabilities: [] });
    expect(fake.callsTo('http.getServiceInfo')).toHaveLength(4);
  });

  // Task 21 starts a fake per Playwright WORKER. These two cases pin what that harness assumes.
  it('REPLACES the handler map rather than merging it', async () => {
    fake.setHandlers({ 'tenancy.getOrganization': () => ({ organization: { prn: ALLOWED, name: 'first' } }) });
    fake.setHandlers({ 'tenancy.listOrganizations': () => ({ organizations: [] }) });
    const tenancy = createIamClient(TenancyService, { baseUrl: fake.grpcUrl }, { bearer: 'token-e' });
    // The second call dropped the first map, so getOrganization falls back to the built-in
    // behaviour — which has no default for it.
    const error = await rejection(tenancy.getOrganization({ prn: ALLOWED }));
    expect(error.code).toBe(Code.Unimplemented);
    expect((await tenancy.listOrganizations({})).organizations).toEqual([]);
  });

  it('releases both ports on close, so a later worker can bind again', async () => {
    const extra = await startFakeIam();
    expect([await accepts(extra.grpcUrl), await accepts(extra.httpUrl)]).toEqual([true, true]);
    await extra.close();
    expect([await accepts(extra.grpcUrl), await accepts(extra.httpUrl)]).toEqual([false, false]);
  });
});
