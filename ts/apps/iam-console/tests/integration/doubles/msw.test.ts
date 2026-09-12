// SPDX-License-Identifier: Apache-2.0
//
// The MSW handler's self-test: it answers only an authenticated probe, with the bare descriptor.
import { setupServer } from 'msw/node';
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest';
import { serviceInfoHandlers } from '../../support/msw';

const IAM_HTTP = 'http://iam.msw.test';
const server = setupServer();

beforeAll(() => server.listen({ onUnhandledRequest: 'error' }));
afterEach(() => server.resetHandlers());
afterAll(() => server.close());

describe('serviceInfoHandlers', () => {
  it('answers the descriptor as JSON to a request with a bearer token', async () => {
    server.use(...serviceInfoHandlers(IAM_HTTP, { service: 'iam', version: '1.2.3', capabilities: ['iam.audit'] }));
    const res = await fetch(`${IAM_HTTP}/v1/service-info`, { headers: { authorization: 'Bearer t' } });
    expect(res.status).toBe(200);
    expect(res.headers.get('content-type')).toContain('application/json');
    expect(await res.json()).toEqual({ service: 'iam', version: '1.2.3', capabilities: ['iam.audit'] });
  });

  it('answers 401 without a bearer token', async () => {
    server.use(...serviceInfoHandlers(IAM_HTTP, { service: 'iam', version: '1.2.3', capabilities: [] }));
    expect((await fetch(`${IAM_HTTP}/v1/service-info`)).status).toBe(401);
  });

  it('answers the configured status', async () => {
    server.use(...serviceInfoHandlers(IAM_HTTP, { status: 503 }));
    expect((await fetch(`${IAM_HTTP}/v1/service-info`, { headers: { authorization: 'Bearer t' } })).status).toBe(503);
  });
});
