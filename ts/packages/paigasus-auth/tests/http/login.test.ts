// SPDX-License-Identifier: Apache-2.0
//
// GET /auth/login. Two tabs starting a login concurrently must both succeed — that is why the
// transaction cookie is per transaction (design doc § 9.2): a single fixed name at Path=/ would
// let the second tab overwrite the first tab's secret, and the first tab's callback would then
// fail its own security check.
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { claimsPrincipalResolver } from '../../src/adapters/claims-resolver.js';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { createOidcClient } from '../../src/adapters/oidc.js';
import { SESSION_COOKIE, TXN_COOKIE_PREFIX, txnCookieName } from '../../src/http/cookies.js';
import { createAuthRoutes } from '../../src/http/routes.js';
import type { AuthEventFields, AuthEventName, AuthLogger } from '../../src/ports/logger.js';
import type { AuthRuntime } from '../../src/runtime.js';
import { startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';

const REDIRECT_URI = 'https://rp.example.com/auth/callback';

let fixture: OidcFixture;
let store: MemorySessionStore;
let events: Array<[AuthEventName, AuthEventFields]>;
let runtime: AuthRuntime;

function recordingLogger(): AuthLogger {
  return { event: (name, fields) => void events.push([name, { ...fields }]) };
}

beforeEach(async () => {
  fixture = await startOidcFixture();
  store = new MemorySessionStore();
  events = [];
  runtime = {
    store,
    resolver: claimsPrincipalResolver,
    logger: recordingLogger(),
    oidc: createOidcClient({
      issuer: fixture.issuer,
      clientId: fixture.clientId,
      clientSecret: fixture.clientSecret,
      httpTimeoutMs: 5000,
      clockToleranceSeconds: 30,
      allowInsecureRequests: true, // the fixture is plain http on localhost — never set in production
    }),
    redirectUri: REDIRECT_URI,
    postLogoutRedirectUri: 'https://rp.example.com/',
    cookieDomainless: true,
    skewMs: 30_000,
    lockTtlMs: 10_000,
    lockWaitMs: 3_000,
    ttlMs: 28_800_000,
    absoluteTtlMs: 86_400_000,
    zone: 'iam',
    basePath: '/iam',
    scopes: 'openid profile email',
  };
});

afterEach(async () => {
  await fixture.close();
});

function loginRequest(search = '', cookieHeader?: string): Request {
  const init = cookieHeader !== undefined ? { headers: { cookie: cookieHeader } } : undefined;
  return new Request(`https://rp.example.com/auth/login${search}`, init);
}

function setCookiesOf(res: Response): string[] {
  return res.headers.getSetCookie();
}

describe('GET /auth/login', () => {
  it('responds 302 to the issuer authorization endpoint', async () => {
    const res = await createAuthRoutes(runtime).handle(loginRequest());
    expect(res.status).toBe(302);
    const location = res.headers.get('location');
    expect(location).not.toBeNull();
    const url = new URL(location ?? '');
    expect(url.origin + url.pathname).toBe(`${fixture.issuer}/authorize`);
  });

  it('includes code_challenge, code_challenge_method=S256, state, nonce and the derived redirect_uri', async () => {
    const res = await createAuthRoutes(runtime).handle(loginRequest());
    const url = new URL(res.headers.get('location') ?? '');
    expect(url.searchParams.get('code_challenge_method')).toBe('S256');
    expect(url.searchParams.get('code_challenge')).toBeTruthy();
    expect(url.searchParams.get('state')).toBeTruthy();
    expect(url.searchParams.get('nonce')).toBeTruthy();
    expect(url.searchParams.get('redirect_uri')).toBe(REDIRECT_URI);
  });

  it('sets a __Host-pgs_txn_<id> cookie whose id matches the state', async () => {
    const res = await createAuthRoutes(runtime).handle(loginRequest());
    const state = new URL(res.headers.get('location') ?? '').searchParams.get('state');
    expect(state).toBeTruthy();
    const cookieName = txnCookieName(state ?? '');
    const found = setCookiesOf(res).some((c) => c.startsWith(`${cookieName}=`));
    expect(found).toBe(true);
  });

  it('stores a transaction holding only the SHA-256 of the cookie secret — the raw secret never appears in the record', async () => {
    const res = await createAuthRoutes(runtime).handle(loginRequest());
    const state = new URL(res.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const cookieName = txnCookieName(state);
    const setCookie = setCookiesOf(res).find((c) => c.startsWith(`${cookieName}=`));
    expect(setCookie).toBeDefined();
    const secret = setCookie?.split(';')[0]?.split('=')[1];
    expect(secret).toBeTruthy();

    const tx = await store.takeTransaction(state);
    expect(tx).not.toBeNull();
    expect(tx?.secretHash).not.toBe(secret);
    // The raw secret must not appear anywhere in the serialised record.
    expect(JSON.stringify(tx)).not.toContain(secret);
  });

  it('clears __Host-pgs_sid in the same response', async () => {
    const res = await createAuthRoutes(runtime).handle(loginRequest());
    const cleared = setCookiesOf(res).find((c) => c.startsWith(`${SESSION_COOKIE}=`));
    expect(cleared).toBeDefined();
    expect(cleared).toMatch(/Max-Age=0/);
  });

  it('keeps at most 4 outstanding txn cookies, clearing the oldest on the 5th', async () => {
    const names = ['t1', 't2', 't3', 't4'].map((id) => txnCookieName(id));
    const cookieHeader = names.map((name) => `${name}=some-secret-value`).join('; ');

    const res = await createAuthRoutes(runtime).handle(loginRequest('', cookieHeader));
    const setCookies = setCookiesOf(res);

    const firstName = names[0];
    expect(firstName).toBeDefined();
    const clearedOldest = setCookies.find((c) => firstName !== undefined && c.startsWith(`${firstName}=`) && /Max-Age=0/.test(c));
    expect(clearedOldest).toBeDefined();

    for (const name of names.slice(1)) {
      const wasCleared = setCookies.some((c) => c.startsWith(`${name}=`) && /Max-Age=0/.test(c));
      expect(wasCleared).toBe(false);
    }

    // Exactly one brand-new (non-cleared) txn cookie is set for this 5th login.
    const newTxnCookies = setCookies.filter((c) => c.startsWith(TXN_COOKIE_PREFIX) && !/Max-Age=0/.test(c));
    expect(newTxnCookies.length).toBe(1);
  });

  it('validates returnTo through validateReturnTo, falling back to the zone base path', async () => {
    const res1 = await createAuthRoutes(runtime).handle(loginRequest('?returnTo=%2Fiam%2Fdashboard'));
    const state1 = new URL(res1.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const tx1 = await store.takeTransaction(state1);
    expect(tx1?.returnTo).toBe('/iam/dashboard');

    const res2 = await createAuthRoutes(runtime).handle(loginRequest('?returnTo=https%3A%2F%2Fevil.com'));
    const state2 = new URL(res2.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const tx2 = await store.takeTransaction(state2);
    expect(tx2?.returnTo).toBe(runtime.basePath);

    const res3 = await createAuthRoutes(runtime).handle(loginRequest('?returnTo=%2F%2Fevil.com'));
    const state3 = new URL(res3.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const tx3 = await store.takeTransaction(state3);
    expect(tx3?.returnTo).toBe(runtime.basePath);
  });

  it('emits a login.started event', async () => {
    await createAuthRoutes(runtime).handle(loginRequest());
    expect(events).toContainEqual(['login.started', { zone: 'iam' }]);
  });

  it('two concurrent login flows both succeed independently', async () => {
    const routes = createAuthRoutes(runtime);
    const [res1, res2] = await Promise.all([routes.handle(loginRequest()), routes.handle(loginRequest())]);
    const state1 = new URL(res1.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const state2 = new URL(res2.headers.get('location') ?? '').searchParams.get('state') ?? '';
    expect(state1).not.toBe(state2);

    const tx1 = await store.takeTransaction(state1);
    const tx2 = await store.takeTransaction(state2);
    expect(tx1).not.toBeNull();
    expect(tx2).not.toBeNull();
  });
});
