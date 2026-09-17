// SPDX-License-Identifier: Apache-2.0
//
// The fake gateway's self-test (spec § 10.1, plan D15), modelled on iam-console's
// tests/integration/doubles/fake-iam.test.ts. It serves ONE route, GET /v1/service-info, and
// nothing else: decision D11 removed the chat route's only consumer, so a fake chat endpoint would
// let this whole tier pass while the real gateway could not answer one call
// (testing/fake-gateway.ts's own header).
//
// The reachability case is the one that is easy to get wrong: closing the server frees the port, so
// a later restore would bind a DIFFERENT one, silently invalidating a URL a test already handed to
// the app under test. `setReachable(true)` must restore service on the SAME port.
import { afterEach, describe, expect, it } from 'vitest';
import { startFakeGateway, type FakeGateway } from '@paigasus/console-core/testing';

let gateway: FakeGateway | undefined;

afterEach(async () => {
  await gateway?.close();
  gateway = undefined;
});

async function start(opts?: Parameters<typeof startFakeGateway>[0]): Promise<FakeGateway> {
  gateway = await startFakeGateway(opts);
  return gateway;
}

function probe(url: string, token: string | null, extraHeaders: Record<string, string> = {}): Promise<Response> {
  const headers: Record<string, string> = { ...extraHeaders };
  if (token !== null) headers.authorization = `Bearer ${token}`;
  return fetch(`${url}/v1/service-info`, { headers });
}

describe('the fake gateway', () => {
  it('answers 200 with the current descriptor for a bearer-authenticated GET /v1/service-info', async () => {
    const fake = await start();
    const res = await probe(fake.url, 'token-a');
    expect(res.status).toBe(200);
    expect(res.headers.get('content-type')).toBe('application/json');
    expect(await res.json()).toEqual({ service: 'gateway', version: '0.0.0-fake', capabilities: ['gateway.chat.stream'] });
  });

  it('answers 404 for any other method or path — in particular, no chat route', async () => {
    const fake = await start();
    expect((await fetch(`${fake.url}/v1/chat`, { headers: { authorization: 'Bearer token-a' } })).status).toBe(404);
    expect((await fetch(`${fake.url}/v1/service-info`, { method: 'POST', headers: { authorization: 'Bearer token-a' } })).status).toBe(404);
    expect((await fetch(fake.url, { headers: { authorization: 'Bearer token-a' } })).status).toBe(404);
  });

  it('answers 401 when the request carries no Authorization header', async () => {
    const fake = await start();
    expect((await probe(fake.url, null)).status).toBe(401);
  });

  it('setServiceInfo({ status }) makes the NEXT probe answer that status', async () => {
    const fake = await start();
    expect((await probe(fake.url, 'token-a')).status).toBe(200);
    fake.setServiceInfo({ status: 503 });
    expect((await probe(fake.url, 'token-a')).status).toBe(503);
  });

  it('setServiceInfo(descriptor) changes the NEXT probe’s body', async () => {
    const fake = await start();
    fake.setServiceInfo({ service: 'gateway', version: '9.9.9', capabilities: [] });
    expect(await (await probe(fake.url, 'token-a')).json()).toEqual({ service: 'gateway', version: '9.9.9', capabilities: [] });
  });

  it('setReachable(false) fails the next request at the TRANSPORT, not with a status — the fetch rejects', async () => {
    const fake = await start();
    fake.setReachable(false);
    await expect(probe(fake.url, 'token-a')).rejects.toThrow();
  });

  it('setReachable(true) restores service on the SAME port, so a caller already holding the url works again', async () => {
    const fake = await start();
    const before = fake.url;

    fake.setReachable(false);
    await expect(probe(fake.url, 'token-a')).rejects.toThrow();

    fake.setReachable(true);
    expect(fake.url).toBe(before);
    const res = await probe(fake.url, 'token-a');
    expect(res.status).toBe(200);
  });

  it('records every request, with its bearer token and correlation-id header — even a 401', async () => {
    const fake = await start();
    await probe(fake.url, 'token-b', { 'paigasus-correlation-id': 'corr-1' });
    await probe(fake.url, null);
    expect(fake.calls).toEqual([
      { method: 'GET', path: '/v1/service-info', token: 'token-b', correlationId: 'corr-1' },
      { method: 'GET', path: '/v1/service-info', token: null, correlationId: null },
    ]);
  });
});
