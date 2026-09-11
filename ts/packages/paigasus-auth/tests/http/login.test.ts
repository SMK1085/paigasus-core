// SPDX-License-Identifier: Apache-2.0
//
// GET /auth/login. Two tabs starting a login concurrently must both succeed — that is why the
// transaction cookie is per transaction (design doc § 9.2): a single fixed name at Path=/ would
// let the second tab overwrite the first tab's secret, and the first tab's callback would then
// fail its own security check.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { claimsPrincipalResolver } from '../../src/adapters/claims-resolver.js';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { createOidcClient } from '../../src/adapters/oidc.js';
import { SESSION_COOKIE, TXN_COOKIE_PREFIX, txnCookieName } from '../../src/http/cookies.js';
import { createAuthRoutes } from '../../src/http/routes.js';
import type { AuthEventFields, AuthEventName, AuthLogger } from '../../src/ports/logger.js';
import type { AuthRuntime } from '../../src/runtime.js';
import { startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';

const ZONE_BASE_PATH = '/iam';
const REDIRECT_URI = 'https://rp.example.com/iam/auth/callback';
const LOGIN_URL = 'https://rp.example.com/iam/auth/login';
const CALLBACK_URL = 'https://rp.example.com/iam/auth/callback';

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
    publicOrigin: 'https://rp.example.com',
    redirectUri: REDIRECT_URI,
    postLogoutRedirectUri: 'https://rp.example.com/',
    cookieDomainless: true,
    skewMs: 30_000,
    lockTtlMs: 10_000,
    lockWaitMs: 3_000,
    ttlMs: 28_800_000,
    absoluteTtlMs: 86_400_000,
    zone: 'iam',
    basePath: ZONE_BASE_PATH,
    scopes: 'openid profile email',
  };
});

afterEach(async () => {
  await fixture.close();
  vi.useRealTimers();
});

function loginRequest(search = '', headers?: Record<string, string>): Request {
  return new Request(`${LOGIN_URL}${search}`, headers !== undefined ? { headers } : undefined);
}

function callbackRequest(state: string, cookieHeader?: string): Request {
  const url = `${CALLBACK_URL}?code=test-code&state=${state}`;
  return new Request(url, cookieHeader !== undefined ? { headers: { cookie: cookieHeader } } : undefined);
}

function setCookiesOf(res: Response): string[] {
  return res.headers.getSetCookie();
}

/** Extracts a Set-Cookie's `name=value` pair, ignoring its attributes. */
function nameValueOf(setCookie: string): { name: string; value: string } {
  const pair = setCookie.split(';')[0] ?? '';
  const eq = pair.indexOf('=');
  return { name: pair.slice(0, eq), value: pair.slice(eq + 1) };
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

    const res = await createAuthRoutes(runtime).handle(loginRequest('', { cookie: cookieHeader }));
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

  it('validates returnTo through validateReturnTo, falling back to the zone base path with a trailing slash', async () => {
    const res1 = await createAuthRoutes(runtime).handle(loginRequest('?returnTo=%2Fiam%2Fdashboard'));
    const state1 = new URL(res1.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const tx1 = await store.takeTransaction(state1);
    expect(tx1?.returnTo).toBe('/iam/dashboard');

    // I3 (final fix wave): the fallback is `${basePath}/`, never bare `basePath` — a bare
    // fallback stores an empty `returnTo` on a root-mounted zone (see the next test), and this
    // assertion previously pinned that buggy form.
    const res2 = await createAuthRoutes(runtime).handle(loginRequest('?returnTo=https%3A%2F%2Fevil.com'));
    const state2 = new URL(res2.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const tx2 = await store.takeTransaction(state2);
    expect(tx2?.returnTo).toBe(`${runtime.basePath}/`);

    const res3 = await createAuthRoutes(runtime).handle(loginRequest('?returnTo=%2F%2Fevil.com'));
    const state3 = new URL(res3.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const tx3 = await store.takeTransaction(state3);
    expect(tx3?.returnTo).toBe(`${runtime.basePath}/`);
  });

  // I3 (final fix wave): this is the root-zone test deferred at task 7 and never written — the
  // deferral is what let the bug through. `PAIGASUS_ZONES={"console":"/"}` gives `basePath === ''`
  // (`@paigasus/next-config`'s `canonicalBasePath` collapses `"/"` to `''`), so a bare `basePath`
  // fallback would have stored `returnTo: ''`, which resolves in the browser to the CURRENT url
  // and breaks login on a root-mounted zone forever (see routes.ts's comment on the fix).
  it('falls back to "/" (not "") on a root-mounted zone, i.e. basePath === ""', async () => {
    const rootRuntime: AuthRuntime = { ...runtime, basePath: '' };

    const res = await createAuthRoutes(rootRuntime).handle(new Request('https://rp.example.com/auth/login?returnTo=https%3A%2F%2Fevil.com'));
    const state = new URL(res.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const tx = await store.takeTransaction(state);
    expect(tx?.returnTo).toBe('/');
  });

  // SMA-511 spec § 6.4. validateReturnTo accepts any same-origin path, including this zone's own
  // auth routes. A crafted link with returnTo=/iam/auth/login would send the browser back into the
  // login after a successful callback — one loop per click.
  it.each([
    '/iam/auth/login',
    '/iam/auth/login?returnTo=%2Fiam%2F',
    '/iam/auth/callback?code=x&state=y',
    '/iam/auth/logout',
    '/iam/auth/logout/callback',
    // Dot segments. validateReturnTo keeps these two values unchanged, and the browser resolves the
    // callback's Location to /iam/auth/login for both. A guard on the raw string misses them.
    '/iam/./auth/login',
    '/iam/x/../auth/login',
    // Empty segments. MEASURED: the WHATWG URL parser resolves dot segments but does NOT collapse a
    // duplicate slash, so `new URL(...).pathname` alone keeps both of these and the guard misses
    // them. Whether `/iam//auth/login` then reaches the login route depends on a server
    // path-normalization step this package does not control, so the guard collapses the path itself.
    '/iam//auth/login',
    '/iam/x/..//auth/login',
  ])('replaces a returnTo under the zone auth routes (%s) with the zone root', async (raw) => {
    const res = await createAuthRoutes(runtime).handle(loginRequest(`?returnTo=${encodeURIComponent(raw)}`));
    const state = new URL(res.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const tx = await store.takeTransaction(state);
    expect(tx?.returnTo).toBe('/iam/');
  });

  it('keeps a returnTo that only starts like an auth route', async () => {
    const res = await createAuthRoutes(runtime).handle(loginRequest('?returnTo=%2Fiam%2Fauthors'));
    const state = new URL(res.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const tx = await store.takeTransaction(state);
    expect(tx?.returnTo).toBe('/iam/authors');
  });

  it('replaces an /auth/ returnTo with "/" on a root-mounted zone', async () => {
    const rootRuntime: AuthRuntime = { ...runtime, basePath: '' };
    const res = await createAuthRoutes(rootRuntime).handle(new Request('https://rp.example.com/auth/login?returnTo=%2Fauth%2Flogin'));
    const state = new URL(res.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const tx = await store.takeTransaction(state);
    expect(tx?.returnTo).toBe('/');
  });

  it('emits a login.started event', async () => {
    await createAuthRoutes(runtime).handle(loginRequest());
    expect(events).toContainEqual(['login.started', { zone: 'iam' }]);
  });

  // Review round 1: the previous version of this test asserted only two distinct states and two
  // stored transactions — a property that holds even under a SINGLE fixed cookie name, since the
  // two Request objects never shared a cookie jar. This version simulates a real shared jar
  // receiving both responses, then completes BOTH flows through it — which a fixed cookie name
  // would break, because the second write would silently overwrite the first tab's secret.
  it('uses a per-transaction cookie name: two tabs sharing one cookie jar can both complete', async () => {
    const routes = createAuthRoutes(runtime);

    const res1 = await routes.handle(loginRequest());
    const res2 = await routes.handle(loginRequest());

    const txnCookie1 = setCookiesOf(res1)
      .filter((c) => c.startsWith(TXN_COOKIE_PREFIX))
      .map(nameValueOf);
    const txnCookie2 = setCookiesOf(res2)
      .filter((c) => c.startsWith(TXN_COOKIE_PREFIX))
      .map(nameValueOf);
    expect(txnCookie1.length).toBe(1);
    expect(txnCookie2.length).toBe(1);
    const cookie1 = txnCookie1[0];
    const cookie2 = txnCookie2[0];
    expect(cookie1).toBeDefined();
    expect(cookie2).toBeDefined();

    // THE property under test: the two logins used DIFFERENT cookie names. Under a single fixed
    // name this assertion fails immediately, before either flow is even attempted.
    expect(cookie1?.name).not.toBe(cookie2?.name);

    // A real browser jar receiving both Set-Cookie headers, in order — with per-transaction
    // names both entries survive; with one shared name the second would overwrite the first.
    const jar = new Map<string, string>();
    if (cookie1 !== undefined) jar.set(cookie1.name, cookie1.value);
    if (cookie2 !== undefined) jar.set(cookie2.name, cookie2.value);
    expect(jar.size).toBe(2);

    const state1 = new URL(res1.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const state2 = new URL(res2.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const nonce1 = new URL(res1.headers.get('location') ?? '').searchParams.get('nonce') ?? '';
    const nonce2 = new URL(res2.headers.get('location') ?? '').searchParams.get('nonce') ?? '';

    // Tab 1 completes using ONLY the jar's entry for its own cookie name.
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: nonce1 }));
    const cookieHeader1 = cookie1 !== undefined ? `${cookie1.name}=${jar.get(cookie1.name) ?? ''}` : '';
    const cb1 = await routes.handle(callbackRequest(state1, cookieHeader1));
    expect(cb1.status).toBe(302);

    // Tab 2 completes independently, using ONLY its own jar entry — proving tab 1's completion
    // (which clears every outstanding txn cookie) did not need, and did not depend on, tab 2's.
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: nonce2 }));
    const cookieHeader2 = cookie2 !== undefined ? `${cookie2.name}=${jar.get(cookie2.name) ?? ''}` : '';
    const cb2 = await routes.handle(callbackRequest(state2, cookieHeader2));
    expect(cb2.status).toBe(302);
  });

  describe('the transaction TTL', () => {
    // Review round 1: TXN_TTL_MS reaches both `putTransaction` and the cookie's `Max-Age`, and
    // neither was previously asserted. Pinned at the documented 10 minutes (design doc § 9.3).
    it('the txn cookie Max-Age is exactly 600 seconds', async () => {
      const res = await createAuthRoutes(runtime).handle(loginRequest());
      const state = new URL(res.headers.get('location') ?? '').searchParams.get('state') ?? '';
      const txnCookie = setCookiesOf(res).find((c) => c.startsWith(`${txnCookieName(state)}=`));
      expect(txnCookie).toMatch(/(^|; )Max-Age=600(;|$)/);
    });

    it('the stored transaction is still live just under 10 minutes later', async () => {
      vi.useFakeTimers();
      const res = await createAuthRoutes(runtime).handle(loginRequest());
      const state = new URL(res.headers.get('location') ?? '').searchParams.get('state') ?? '';
      vi.advanceTimersByTime(10 * 60 * 1000 - 1);
      await expect(store.takeTransaction(state)).resolves.not.toBeNull();
    });

    it('the stored transaction has expired just over 10 minutes later', async () => {
      vi.useFakeTimers();
      const res = await createAuthRoutes(runtime).handle(loginRequest());
      const state = new URL(res.headers.get('location') ?? '').searchParams.get('state') ?? '';
      vi.advanceTimersByTime(10 * 60 * 1000 + 1);
      await expect(store.takeTransaction(state)).resolves.toBeNull();
    });
  });

  describe('the top-level-navigation guard (login-forcing via a cross-site cookie write)', () => {
    // /auth/login unconditionally clears __Host-pgs_sid, so a cross-site <img src="…/auth/login">
    // could otherwise force a logout wherever third-party cookie writes are permitted. Requiring
    // Sec-Fetch-Mode: navigate closes that without breaking legitimate top-level navigations.
    it('allows a request with Sec-Fetch-Mode: navigate', async () => {
      const res = await createAuthRoutes(runtime).handle(loginRequest('', { 'sec-fetch-mode': 'navigate' }));
      expect(res.status).toBe(302);
    });

    it('allows a request with no Sec-Fetch-Mode header at all (older browsers, non-browser callers)', async () => {
      const res = await createAuthRoutes(runtime).handle(loginRequest());
      expect(res.status).toBe(302);
    });

    it('rejects a request whose Sec-Fetch-Mode is not navigate (e.g. an <img> or fetch())', async () => {
      const res = await createAuthRoutes(runtime).handle(loginRequest('', { 'sec-fetch-mode': 'no-cors' }));
      expect(res.status).toBe(403);
      // No cookie is cleared or set for a rejected request.
      expect(setCookiesOf(res).length).toBe(0);
    });

    it('rejects Sec-Fetch-Mode: cors too', async () => {
      const res = await createAuthRoutes(runtime).handle(loginRequest('', { 'sec-fetch-mode': 'cors' }));
      expect(res.status).toBe(403);
    });
  });
});

// I6 (final fix wave): `tests/http/callback.test.ts`'s "deletes any pre-existing record for the
// old sid" test hand-builds a Cookie header pairing a LIVE sid with a txn cookie in one request —
// a header a real single-tab re-login can never produce, since `handleLogin`'s own 302 clears the
// sid cookie in the SAME response that starts the new flow. This drives the REAL sequence
// instead: login, callback (session 1), then a second login carrying session 1's cookie, and
// asserts session 1's record is gone — WITHOUT a second callback ever running.
describe('re-login invalidates the previous session (I6)', () => {
  it('a second login deletes the first session even though the browser never re-presents it to a callback', async () => {
    const routes = createAuthRoutes(runtime);

    // First login + callback: establishes session 1.
    const login1 = await routes.handle(loginRequest());
    const location1 = new URL(login1.headers.get('location') ?? '');
    const state1 = location1.searchParams.get('state') ?? '';
    const nonce1 = location1.searchParams.get('nonce') ?? '';
    const txnCookie1 = setCookiesOf(login1)
      .find((c) => c.startsWith(TXN_COOKIE_PREFIX))
      ?.split(';')[0];
    expect(txnCookie1).toBeDefined();

    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: nonce1 }));
    const callback1 = await routes.handle(callbackRequest(state1, txnCookie1));
    expect(callback1.status).toBe(302);
    const sid1 = setCookiesOf(callback1)
      .find((c) => c.startsWith(`${SESSION_COOKIE}=`))
      ?.split(';')[0]
      ?.split('=')[1];
    expect(sid1).toBeTruthy();
    expect(await store.get(sid1 ?? '')).not.toBeNull();

    // Second login, carrying session 1's cookie — exactly what a real browser sends on a
    // single-tab re-login (captured here as the Set-Cookie clear `handleLogin` also issues).
    const login2 = await routes.handle(loginRequest('', { cookie: `${SESSION_COOKIE}=${sid1}` }));
    expect(login2.status).toBe(302);
    const clearedSid = setCookiesOf(login2).find((c) => c.startsWith(`${SESSION_COOKIE}=`));
    expect(clearedSid).toMatch(/Max-Age=0/);

    // Session 1's record is gone — deleted by the SECOND login itself, before any second
    // callback ever ran.
    expect(await store.get(sid1 ?? '')).toBeNull();
  });
});

describe('route dispatch', () => {
  it('responds 405 to a non-GET /auth/login', async () => {
    const res = await createAuthRoutes(runtime).handle(new Request(LOGIN_URL, { method: 'POST' }));
    expect(res.status).toBe(405);
  });

  it('responds 404 to an unrelated path', async () => {
    const res = await createAuthRoutes(runtime).handle(new Request('https://rp.example.com/not-auth-at-all'));
    expect(res.status).toBe(404);
  });

  it("responds 404 to /auth/login under a DIFFERENT zone's base path", async () => {
    // Review round 1, M5: pathname matching is now exact against THIS runtime's own base path,
    // not a suffix match — `/anything/auth/login` no longer matches.
    const res = await createAuthRoutes(runtime).handle(new Request('https://rp.example.com/other-zone/auth/login'));
    expect(res.status).toBe(404);
  });
});
