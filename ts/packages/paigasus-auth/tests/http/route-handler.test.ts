// SPDX-License-Identifier: Apache-2.0
//
// SMA-511 spec § 7.1 — createAuthRouteHandler under a Next basePath, with the REAL routes and the
// real OIDC fixture. A Next route handler receives `req.url` with the basePath REMOVED and the
// server's BIND address as the host (measured, spec § 13 row 1). This drives a login, a callback and
// a logout with exactly that URL shape and asserts the redirect_uri the token request carries.
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { claimsPrincipalResolver } from '../../src/adapters/claims-resolver.js';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { createOidcClient, type OidcClient } from '../../src/adapters/oidc.js';
import { hashSecret } from '../../src/core/ids.js';
import { SESSION_COOKIE, TXN_COOKIE_PREFIX, txnCookieName } from '../../src/http/cookies.js';
import type { AuthRuntime } from '../../src/runtime.js';
import { createAuthRouteHandler } from '../../src/server.js';
import { startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';
import { expectStoreUnavailable, failingStore, storeError } from '../support/store-failure.js';

const BIND = 'http://0.0.0.0:3000';
const PUBLIC_ORIGIN = 'https://console.example.com';

const IDP_SENTENCE = 'The identity provider is not available. Try again in a few seconds.';

/**
 * The REAL adapter, on an issuer where discovery fails at once. Port 1 is on the Fetch "bad port"
 * list, so undici refuses it with no connect and no wait (SMA-656 plan, Review Focus 5).
 */
function unreachableOidc(): OidcClient {
  return createOidcClient({
    issuer: 'http://127.0.0.1:1',
    clientId: 'paigasus-console',
    clientSecret: 'a-client-secret',
    httpTimeoutMs: 2000,
    clockToleranceSeconds: 30,
    scopes: 'openid',
    allowInsecureRequests: true,
  });
}

let fixture: OidcFixture;
let runtime: AuthRuntime;
let grantUrls: string[];

beforeEach(async () => {
  fixture = await startOidcFixture();
  grantUrls = [];
  const inner = createOidcClient({
    issuer: fixture.issuer,
    clientId: fixture.clientId,
    clientSecret: fixture.clientSecret,
    httpTimeoutMs: 5000,
    clockToleranceSeconds: 30,
    scopes: 'openid profile email',
    allowInsecureRequests: true, // the fixture is plain http on localhost — never set in production
  });
  runtime = {
    store: new MemorySessionStore(),
    resolver: claimsPrincipalResolver,
    logger: { event: () => undefined },
    oidc: {
      buildAuthorizationUrl: (params) => inner.buildAuthorizationUrl(params),
      authorizationCodeGrant: (params) => {
        grantUrls.push(params.currentUrl.href);
        return inner.authorizationCodeGrant(params);
      },
      refresh: (token) => inner.refresh(token),
      revoke: (token) => inner.revoke(token),
      buildEndSessionUrl: (params) => inner.buildEndSessionUrl(params),
    },
    publicOrigin: PUBLIC_ORIGIN,
    redirectUri: `${PUBLIC_ORIGIN}/iam/auth/callback`,
    postLogoutRedirectUri: `${PUBLIC_ORIGIN}/iam/`,
    clientId: 'paigasus-console',
    cookieDomainless: true,
    skewMs: 30_000,
    lockTtlMs: 10_000,
    lockWaitMs: 3_000,
    ttlMs: 28_800_000,
    absoluteTtlMs: 86_400_000,
    zone: 'iam',
    basePath: '/iam',
  };
});

afterEach(async () => {
  await fixture.close();
});

describe('createAuthRouteHandler under basePath /iam (SMA-511 spec § 7.1)', () => {
  it('completes a login and a callback from basePath-stripped, bind-address URLs', async () => {
    const handle = createAuthRouteHandler(runtime);

    const login = await handle(new Request(`${BIND}/auth/login?returnTo=%2Fiam%2Forgs`));
    expect(login.status).toBe(302);
    const authorize = new URL(login.headers.get('location') ?? '');
    expect(authorize.origin + authorize.pathname).toBe(`${fixture.issuer}/authorize`);
    expect(authorize.searchParams.get('redirect_uri')).toBe(`${PUBLIC_ORIGIN}/iam/auth/callback`);
    const state = authorize.searchParams.get('state') ?? '';
    const nonce = authorize.searchParams.get('nonce') ?? '';
    const txnCookie = login.headers
      .getSetCookie()
      .find((c) => c.startsWith(TXN_COOKIE_PREFIX))
      ?.split(';')[0];
    expect(txnCookie).toBeDefined();

    fixture.setNextIdToken(await fixture.mintIdToken({ nonce }));
    const callback = await handle(new Request(`${BIND}/auth/callback?code=test-code&state=${state}`, { headers: { cookie: txnCookie ?? '' } }));

    expect(callback.status).toBe(302);
    expect(callback.headers.get('location')).toBe('/iam/orgs');
    expect(callback.headers.getSetCookie().some((c) => c.startsWith(`${SESSION_COOKIE}=`))).toBe(true);
    // The token request's redirect_uri equals the /authorize one, not the bind address.
    expect(grantUrls).toEqual([`${PUBLIC_ORIGIN}/iam/auth/callback?code=test-code&state=${state}`]);
  });

  it('logs out with a POST to the basePath-stripped path', async () => {
    const res = await createAuthRouteHandler(runtime)(new Request(`${BIND}/auth/logout`, { method: 'POST' }));
    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toMatch(new RegExp(`^${fixture.issuer}/logout\\?`));
  });

  it('still 404s a path that is not an auth route', async () => {
    const res = await createAuthRouteHandler(runtime)(new Request(`${BIND}/orgs`));
    expect(res.status).toBe(404);
  });

  it('passes a store-failure 503 through, with a full-path retry link (SMA-653)', async () => {
    runtime = { ...runtime, store: failingStore(new MemorySessionStore(), new Set(['putTransaction']), () => storeError('timeout'), []) };

    const res = await createAuthRouteHandler(runtime)(new Request(`${BIND}/auth/login?returnTo=%2Fiam%2Forgs`));

    await expectStoreUnavailable(res, { kind: 'link', target: '/iam/auth/login?returnTo=%2Fiam%2Forgs' });
  });

  it('T11: a discovery failure on /auth/login is the IdP 503, not a 500 (SMA-656)', async () => {
    const events: Array<[string, unknown]> = [];
    runtime = { ...runtime, oidc: unreachableOidc(), logger: { event: (name, fields) => void events.push([name, { ...fields }]) } };

    const res = await createAuthRouteHandler(runtime)(new Request(`${BIND}/auth/login?returnTo=%2Fiam%2Forgs`));

    const body = await expectStoreUnavailable(res, { kind: 'link', target: '/iam/auth/login?returnTo=%2Fiam%2Forgs' }, ['127.0.0.1']);
    expect(body).toContain(IDP_SENTENCE);
    expect(events).toEqual([['oidc.discovery_failed', { zone: runtime.zone, stage: 'login', reason: 'network' }]]);
  });

  it('T16: a discovery failure on /auth/callback is the IdP 503, not the login-failed 502 (SMA-656)', async () => {
    const events: Array<[string, unknown]> = [];
    runtime = { ...runtime, oidc: unreachableOidc(), logger: { event: (name, fields) => void events.push([name, { ...fields }]) } };
    // Seeded directly: a login first cannot seed it, because on this issuer the login gives a 503.
    const txnId = 'txn-sma-656-0123456789';
    const secret = 'callback-secret-value-32-bytes-x';
    await runtime.store.putTransaction(txnId, { codeVerifier: 'a-verifier', nonce: 'a-nonce', returnTo: '/iam/orgs', secretHash: hashSecret(secret), createdAt: Date.now() }, 600_000);

    const res = await createAuthRouteHandler(runtime)(new Request(`${BIND}/auth/callback?code=x&state=${txnId}`, { headers: { cookie: `${txnCookieName(txnId)}=${secret}` } }));

    const body = await expectStoreUnavailable(res, { kind: 'link', target: '/iam/auth/login?returnTo=%2Fiam%2Forgs' }, ['127.0.0.1']);
    expect(body).toContain(IDP_SENTENCE);
    expect(body).not.toContain('login failed');
    expect(events).toEqual([['oidc.discovery_failed', { zone: runtime.zone, stage: 'callback', reason: 'network' }]]);
  });
});
