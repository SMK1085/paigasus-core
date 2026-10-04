// SPDX-License-Identifier: Apache-2.0
//
// SMA-514 scenario 1 (spec § 5): the auth round trip against the kind stack. A cold visit goes to
// the IdP and back through the callback. Logout through the shell then kills the session on the
// server, in BOTH zones, not only in the browser.
//
// The seven step titles are pinned by ci/kind/journeys-report.mjs (EXPECTED_STEPS): `run.sh specs
// journeys` checks them in this file before the run and in the JSON report after it, so a step
// that never ran fails the job although every counter says "passed" (spec § 7.2).
//
// A replay context holds ONLY the session cookie; it does not stop the redirect. Measured on
// Playwright 1.63 (Task 4 review, decision D8): a `context.route` handler never sees a
// server-side redirect hop, so a route cannot stop one. The real handleLogin therefore runs in
// step 6: it deletes the presented sid, which is already dead, and sends the new context to the
// Keycloak form. That context is never reused or closed after it shows Keycloak (SMA-652). The
// third step's replay control has no stop either: if the control fails, handleLogin deletes the
// LIVE sid, but the test has already failed at that control by then.
//
// SSO (SMA-682 D2, D3): the kind realm grants `offline_access` only as an OPTIONAL client scope,
// and ci/kind/values/a.yaml sets `oidc.scopes` to `openid profile email`. So the console gets an
// online refresh token, and Keycloak keeps its SSO session after the code exchange. Steps 4 and 7
// ask Keycloak about that session with a prompt=none authorization request. Each request runs in
// a NEW context that holds only the two Keycloak cookies that step 4 read from context A after the
// login (SMA-514 D10, restored). The end-session response expires KEYCLOAK_IDENTITY in context A,
// so a probe in context A could show login_required while the server session is still alive. A
// replay of the old values asks the SERVER. Step 4 is the control of step 7: the same cookies and
// the same request returned a code before logout.
//
// The probe's redirect_uri is https://console.paigasus.test/kind-sso-probe, a registered redirect
// URI that no ingress rule routes (SMA-682 D6). With /iam/auth/callback, the foreign state gives
// txn_missing, the callback sends the browser to /iam/auth/login, and handleLogin logs in again
// silently through SSO. That adds a console session and can hide a failure. Step 4's code is never
// exchanged, and Keycloak lets it expire. With prompt=none, Keycloak never shows its form (OIDC
// Core § 3.1.2.1), so no Keycloak tab exists to reuse (SMA-652).
//
// D8 (SMA-682): step 5 asserts that Keycloak answers end-session with a redirect, not a page, and
// that no Keycloak document after Sign out has status 200. Since SMA-681, logout sends
// `id_token_hint`, and step 5 asserts it: a three-part JWT issued to `paigasus-console`.
// handleLogout revokes the refresh token BEFORE it builds the end-session redirect. Measured
// (SMA-682 spec § 13, row M6): that revoke already ends the SSO session. So D8 proves only that the
// console's real logout sequence shows no confirmation page. It is not a proof of SMA-681 AC 1 with
// a live session. Step 7 proves that the logout sequence (revoke, then end-session) ends the IdP
// session, not that end-session alone does (SMA-682 residual R7).
import { randomUUID } from 'node:crypto';
import { expect, test, type Browser, type BrowserContext, type Cookie, type Request, type Response } from '@playwright/test';
import { CONSOLE_HOST, IDP_HOST, SESSION_COOKIE, credential, redirectChain, sessionCookie, waitForHydration } from '../support/login';

const ORIGIN = `https://${CONSOLE_HOST}`;
const ZONES = [
  { base: '/iam', page: '/iam/orgs' },
  { base: '/gateway', page: '/gateway/overview' },
] as const;
const WAIT = { timeout: 30_000 } as const;
const PROBE_REDIRECT = `${ORIGIN}/kind-sso-probe`;
const KEYCLOAK_COOKIES = ['KEYCLOAK_IDENTITY', 'KEYCLOAK_SESSION'] as const;

function isConsolePath(request: Request, pathname: string): boolean {
  const url = new URL(request.url());
  return url.hostname === CONSOLE_HOST && url.pathname === pathname;
}

/** A new context with no state but `sid`. Nothing stops a redirect: handleLogin runs for real. */
async function replayContext(browser: Browser, sid: string): Promise<BrowserContext> {
  const context = await browser.newContext({ baseURL: ORIGIN, ignoreHTTPSErrors: true });
  // `url`, not `domain`: a __Host- cookie must be host-only (no Domain attribute), Secure, Path=/.
  await context.addCookies([{ name: SESSION_COOKIE, value: sid, url: `${ORIGIN}/`, secure: true, httpOnly: true, sameSite: 'Lax' }]);
  return context;
}

/** The earliest request in `response`'s redirect chain (the one the caller's `goto` made). */
function firstRequestOf(response: Response): Request {
  let request = response.request();
  for (let from = request.redirectedFrom(); from !== null; from = request.redirectedFrom()) request = from;
  return request;
}

/**
 * A prompt=none authorization request, sent as a browser navigation in a NEW context that holds
 * only `idpCookies`. Returns the Location of the Keycloak 302. The server-side SSO session decides
 * the answer, not the cookie state of any other context. The context is never closed or reused.
 */
async function silentAuthorize(browser: Browser, authorizationEndpoint: URL, idpCookies: readonly Cookie[]): Promise<URL> {
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  await context.addCookies(idpCookies);
  const page = await context.newPage();
  const state = randomUUID();
  const url = new URL(authorizationEndpoint.href);
  url.search = new URLSearchParams({ client_id: 'paigasus-console', response_type: 'code', scope: 'openid', redirect_uri: PROBE_REDIRECT, state, prompt: 'none' }).toString();
  const response = await page.goto(url.href);
  if (response === null) throw new Error('page.goto(prompt=none) returned no response');
  const chain = await redirectChain(response);
  const hops = chain.map((hop) => `${hop.url.hostname}${hop.url.pathname} ${String(hop.status)}`).join(' -> ');
  const [first, second] = chain;
  expect(first?.url.hostname, `prompt=none: the first hop must be the IdP: ${hops}`).toBe(IDP_HOST);
  expect(first?.status ?? 0, `prompt=none: the IdP must answer with a redirect, not a page: ${hops}`).toBeGreaterThanOrEqual(300);
  expect(first?.status ?? 0, `prompt=none: the IdP must answer with a redirect, not a page: ${hops}`).toBeLessThan(400);
  expect(second === undefined ? undefined : `${second.url.origin}${second.url.pathname}`, `prompt=none: the IdP must redirect to the probe URI: ${hops}`).toBe(PROBE_REDIRECT);
  // D6 (c): no hop may reach a console zone, where a txn_missing callback would log in again.
  expect(
    chain.filter((hop) => hop.url.hostname === CONSOLE_HOST && ZONES.some((zone) => hop.url.pathname.startsWith(zone.base))).map((hop) => hop.url.pathname),
    `prompt=none: no hop may reach a console zone: ${hops}`,
  ).toEqual([]);
  if (second === undefined) throw new Error(`prompt=none: the chain has no second hop: ${hops}`);
  expect(second.url.searchParams.get('state'), 'prompt=none: the IdP must return the same state').toBe(state);
  // The probe URI's own status is not asserted (no ingress rule routes it; 404 expected). The
  // annotation records it, and the job log prints it on a green run (SMA-682 M11).
  const result = second.url.searchParams.has('code') ? 'code' : `error=${second.url.searchParams.get('error') ?? '(none)'}`;
  test.info().annotations.push({ type: 'prompt=none probe', description: `${hops} (${result})` });
  return second.url;
}

test('J1: a cold visit logs in through the IdP, and logout ends the session in both zones (SMA-514 scenario 1)', async ({ browser, context, page }) => {
  const authorizationEndpoint = await test.step('cold visit: /iam/orgs goes through /iam/auth/login to the IdP form', async () => {
    expect(await context.cookies(), 'context A starts with no cookie').toEqual([]);
    const response = await page.goto('/iam/orgs');
    if (response === null) throw new Error('page.goto(/iam/orgs) returned no response');
    const chain = await redirectChain(response);
    const hops = chain.map((hop) => `${hop.url.hostname}${hop.url.pathname} ${String(hop.status)}`);
    expect(
      chain.some((hop) => hop.url.hostname === CONSOLE_HOST && hop.url.pathname === '/iam/auth/login'),
      `the chain must pass /iam/auth/login: ${hops.join(' -> ')}`,
    ).toBe(true);
    const last = chain.at(-1);
    expect(last?.url.hostname, `the chain must end at the IdP: ${hops.join(' -> ')}`).toBe(IDP_HOST);
    expect(last?.url.pathname.endsWith('/protocol/openid-connect/auth'), `the chain must end at the authorization endpoint: ${hops.join(' -> ')}`).toBe(true);
    await expect(page.locator('#username')).toBeVisible();
    if (last === undefined) throw new Error('the cold-visit chain is empty');
    // Steps 4 and 7 use this endpoint: no discovery request and no second source of truth.
    return new URL(`${last.url.origin}${last.url.pathname}`);
  });

  const sid = await test.step('login: the callback returns to /iam/orgs with a session', async () => {
    const callback = page.waitForRequest((request) => isConsolePath(request, '/iam/auth/callback'), WAIT);
    const landing = page.waitForResponse((response) => response.request().resourceType() === 'document' && isConsolePath(response.request(), '/iam/orgs'), WAIT);
    await page.locator('#username').fill(credential('PAIGASUS_KIND_USERNAME'));
    await page.locator('#password').fill(credential('PAIGASUS_KIND_PASSWORD'));
    await page.locator('#kc-login').click();
    await callback;
    expect((await landing).status(), '/iam/orgs after the callback').toBe(200);
    await waitForHydration(page);
    return sessionCookie(page);
  });

  await test.step('control: the sid replays in both zones', async () => {
    // Without this control, the "old sid is refused" step could pass because the replay method is
    // broken, not because the session is dead.
    for (const zone of ZONES) {
      const replay = await replayContext(browser, sid);
      const replayPage = await replay.newPage();
      const response = await replayPage.goto(zone.page);
      if (response === null) throw new Error(`page.goto(${zone.page}) returned no response`);
      expect(response.request().redirectedFrom(), `${zone.page}: the replayed sid must be accepted with no redirect`).toBeNull();
      expect(response.status(), zone.page).toBe(200);
      expect(new URL(replayPage.url()).pathname).toBe(zone.page);
      await waitForHydration(replayPage);
    }
  });

  const idpCookies = await test.step('control: the IdP session answers prompt=none with a code', async () => {
    // SMA-682 AC 1. On the old scope setup (offline_access as a default client scope), Keycloak
    // keeps both cookies, and this step fails at the code check with login_required (SMA-682 spec
    // § 13.1, bite 2).
    const cookies = (await context.cookies(`https://${IDP_HOST}/realms/paigasus/`)).filter((cookie) => KEYCLOAK_COOKIES.some((name) => name === cookie.name));
    const names = cookies.map((cookie) => cookie.name);
    for (const name of KEYCLOAK_COOKIES) expect(names, `Keycloak keeps ${name} after the console login (SMA-682)`).toContain(name);
    expect(names, 'exactly one cookie per Keycloak cookie name').toHaveLength(KEYCLOAK_COOKIES.length);
    const location = await silentAuthorize(browser, authorizationEndpoint, cookies);
    expect(location.searchParams.get('error'), 'prompt=none with a live IdP session: no error').toBeNull();
    expect(location.searchParams.get('code'), 'prompt=none with a live IdP session: a code').not.toBeNull();
    return cookies;
  });

  await test.step('logout: the shell form ends at the IdP and returns to /iam/ with no session cookie', async () => {
    const requested: string[] = [];
    const documents: { readonly url: URL; readonly status: number }[] = [];
    page.on('request', (request) => requested.push(new URL(request.url()).pathname));
    page.on('response', (response) => {
      if (response.request().resourceType() === 'document') documents.push({ url: new URL(response.url()), status: response.status() });
    });
    const post = page.waitForRequest((request) => request.method() === 'POST' && isConsolePath(request, '/iam/auth/logout'), WAIT);
    const endSession = page.waitForRequest((request) => {
      const url = new URL(request.url());
      return url.hostname === IDP_HOST && url.pathname.endsWith('/protocol/openid-connect/logout');
    }, WAIT);
    const back = page.waitForRequest((request) => {
      const url = new URL(request.url());
      return url.hostname === CONSOLE_HOST && url.pathname === '/iam/' && url.searchParams.has('state');
    }, WAIT);

    // The user menu is the LAST menu trigger in the header (the org switcher comes before it).
    await page.getByRole('banner').locator('button[aria-haspopup="menu"]').last().click();
    await page.getByRole('menuitem', { name: 'Sign out' }).click();

    expect((await (await post).response())?.status(), 'POST /iam/auth/logout').toBe(302);
    // The 302 alone does not prove the SMA-653 defect class is absent: under a CSP form-action that
    // blocks the redirect, Chromium gets the 302 and then refuses to follow it. The end-session
    // request observed below is the proof that the browser actually followed it to the IdP.
    // page.waitForRequest sees the wire: handleLogout's degraded arm never contacts the IdP
    // (docs/superpowers/specs/2026-09-09-sma-506-measurements.md:803-838).
    const endSessionRequest = await endSession;
    expect(new URL(endSessionRequest.url()).searchParams.get('client_id'), 'end-session client_id').toBe('paigasus-console');

    // D8 (SMA-682): a 3xx, not a page. It comes BEFORE the hint assertions and before the wait for
    // /iam/, so a logout with no hint fails here with the D8 message (SMA-682 spec § 6.2).
    const endSessionStatus = (await endSessionRequest.response())?.status() ?? 0;
    expect(endSessionStatus, 'D8: Keycloak answered end-session with a page, not a redirect (a confirmation page?)').toBeGreaterThanOrEqual(300);
    expect(endSessionStatus, 'D8: Keycloak answered end-session with a page, not a redirect (a confirmation page?)').toBeLessThan(400);

    // SMA-681 AC 2: the end-session request carries the stored ID token. Its `aud` must name the
    // client: logout sends the hint only then (http/routes.ts), and Keycloak rejects any other `aud`.
    const hint = new URL(endSessionRequest.url()).searchParams.get('id_token_hint');
    expect(hint, 'end-session id_token_hint (SMA-681)').not.toBeNull();
    const parts = (hint ?? '').split('.');
    expect(parts, 'id_token_hint is a three-part JWT').toHaveLength(3);
    const claims = JSON.parse(Buffer.from(parts[1] ?? '', 'base64url').toString('utf8')) as { aud?: unknown };
    const audiences: unknown[] = Array.isArray(claims.aud) ? claims.aud : [claims.aud];
    expect(audiences.includes('paigasus-console'), 'id_token_hint aud names paigasus-console').toBe(true);

    // The revoke already ended the SSO session (SMA-682 spec § 13, row M6), so D8 above does not
    // prove what the hint does (SMA-682 residual R7).
    await back;
    await page.waitForURL((url) => url.hostname === CONSOLE_HOST && /^\/iam\/?$/.test(url.pathname));
    await expect(page.getByTestId('public-home')).toBeVisible();
    expect(
      documents.filter((doc) => doc.url.hostname === IDP_HOST && doc.status === 200).map((doc) => doc.url.pathname),
      'D8: no Keycloak document after Sign out has status 200',
    ).toEqual([]);

    // Measured, not assumed: Next can answer /iam/ with a trailing-slash redirect to /iam.
    const home = documents.filter((doc) => doc.url.hostname === CONSOLE_HOST && /^\/iam\/?$/.test(doc.url.pathname));
    test.info().annotations.push({
      type: 'post-logout documents',
      description: [`${new URL(endSessionRequest.url()).pathname} (no confirmation page, SMA-682 D8)`, ...home.map((doc) => `${doc.url.pathname}${doc.url.search} ${String(doc.status)}`)].join(' -> '),
    });
    expect(home.at(-1)?.status, 'the public page after logout').toBe(200);
    // The chart sets no PAIGASUS_OIDC_POST_LOGOUT_REDIRECT_URI, so the IdP returns to `${origin}/iam/`.
    expect(
      requested.filter((path) => path === '/iam/auth/logout/callback'),
      '/iam/auth/logout/callback is not requested',
    ).toEqual([]);
    expect(
      (await context.cookies()).filter((cookie) => cookie.name === SESSION_COOKIE),
      'the session cookie is gone',
    ).toEqual([]);
  });

  await test.step('the old sid is refused by both zones', async () => {
    for (const zone of ZONES) {
      // One new context per URL (spec § 5 step 5): each sees the old sid exactly once. The chain
      // is NOT stopped (see the header comment): it runs to the real IdP form, so only the first
      // two hops are asserted here, never the final URL.
      const replay = await replayContext(browser, sid);
      const replayPage = await replay.newPage();
      const response = await replayPage.goto(zone.page);
      if (response === null) throw new Error(`page.goto(${zone.page}) returned no response`);
      const chain = await redirectChain(response);
      const hops = chain.map((hop) => `${hop.url.hostname}${hop.url.pathname} ${String(hop.status)}`);
      const cookieHeader = (await firstRequestOf(response).allHeaders())['cookie'] ?? '';
      expect(cookieHeader.split(/;\s*/), `${zone.page}: the first request must carry the old sid`).toContain(`${SESSION_COOKIE}=${sid}`);
      const first = chain[0];
      expect(first?.status, `${zone.page}: the first hop must be a redirect: ${hops.join(' -> ')}`).toBeGreaterThanOrEqual(300);
      expect(first?.status, `${zone.page}: the first hop must be a redirect: ${hops.join(' -> ')}`).toBeLessThan(400);
      // The proxy checks only that the cookie exists (middleware.ts:122), so a redirect to login
      // here can come only from requireSession()'s store lookup: the Redis record is gone.
      expect(chain[1]?.url.pathname, `${zone.page}: the next hop must be ${zone.base}/auth/login: ${hops.join(' -> ')}`).toBe(`${zone.base}/auth/login`);
    }
  });

  await test.step('the IdP session is dead: prompt=none returns login_required', async () => {
    // Step 4 is the control: the same cookie values (captured before logout) and the same request
    // returned a code. The values are replayed in a new context, so this result cannot come from
    // the cookie clear in context A. It can come only from the server-side end of the SSO session.
    const location = await silentAuthorize(browser, authorizationEndpoint, idpCookies);
    expect(location.searchParams.get('code'), 'prompt=none after logout: no code').toBeNull();
    expect(location.searchParams.get('error'), 'prompt=none after logout').toBe('login_required');
  });
});
