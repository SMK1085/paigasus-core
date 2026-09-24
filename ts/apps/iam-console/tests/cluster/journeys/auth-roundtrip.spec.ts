// SPDX-License-Identifier: Apache-2.0
//
// SMA-514 scenario 1 (spec § 5): the auth round trip against the kind stack. A cold visit goes to
// the IdP and back through the callback. Logout through the shell then kills the session on the
// server, in BOTH zones, not only in the browser.
//
// The five step titles are pinned by ci/kind/journeys-report.mjs (EXPECTED_STEPS): `run.sh specs
// journeys` checks them in this file before the run and in the JSON report after it, so a step
// that never ran fails the job although every counter says "passed" (spec § 7.2).
//
// A replay context holds ONLY the session cookie; it does not stop the redirect. Measured on
// Playwright 1.63 (Task 4 review, decision D8): a `context.route` handler never sees a
// server-side redirect hop, so a route cannot stop one. The real handleLogin therefore runs in
// the last step: it deletes the presented sid, which is already dead, and sends the new context to
// the Keycloak form. That context is never reused or closed after it shows Keycloak (SMA-652). The
// third step's replay control has no stop either: if the control fails, handleLogin deletes the
// LIVE sid, but the test has already failed at that control by then.
//
// D11: J1 no longer checks the IdP session. After the console's code exchange, Keycloak keeps no
// SSO session (the offline_access grant; measured in CI diag run 36055036506 and locally), so a
// silent-SSO control cannot pass and a check on the IdP session cannot prove anything. SMA-682
// owns the product finding and restores both checks.
//
// D9: step 4 no longer clicks the logout confirmation page. On the kind stack Keycloak shows
// no confirmation page because no SSO session exists (SMA-682): a request with `client_id` and
// no `id_token_hint` redirects at once rather than showing "Do you want to log out?". When
// SMA-682 restores the SSO session, the page returns until SMA-681 makes logout send
// `id_token_hint`. The test makes no assumptions about this page and does not interact with it.
import { expect, test, type Browser, type BrowserContext, type Request, type Response } from '@playwright/test';
import { CONSOLE_HOST, IDP_HOST, SESSION_COOKIE, credential, redirectChain, sessionCookie, waitForHydration } from '../support/login';

const ORIGIN = `https://${CONSOLE_HOST}`;
const ZONES = [
  { base: '/iam', page: '/iam/orgs' },
  { base: '/gateway', page: '/gateway/overview' },
] as const;
const WAIT = { timeout: 30_000 } as const;

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

test('J1: a cold visit logs in through the IdP, and logout ends the session in both zones (SMA-514 scenario 1)', async ({ browser, context, page }) => {
  await test.step('cold visit: /iam/orgs goes through /iam/auth/login to the IdP form', async () => {
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

    // D9: on the kind stack no SSO session exists (SMA-682), so Keycloak redirects at once
    // without showing a confirmation page. The test makes no assumptions about this page.
    await back;
    await page.waitForURL((url) => url.hostname === CONSOLE_HOST && /^\/iam\/?$/.test(url.pathname));
    await expect(page.getByTestId('public-home')).toBeVisible();

    // Measured, not assumed: Next can answer /iam/ with a trailing-slash redirect to /iam.
    const home = documents.filter((doc) => doc.url.hostname === CONSOLE_HOST && /^\/iam\/?$/.test(doc.url.pathname));
    test.info().annotations.push({
      type: 'post-logout documents',
      description: [
        `${new URL(endSessionRequest.url()).pathname} (SMA-682: no confirmation page on kind stack)`,
        ...home.map((doc) => `${doc.url.pathname}${doc.url.search} ${String(doc.status)}`),
      ].join(' -> '),
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
});
