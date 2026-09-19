// SPDX-License-Identifier: Apache-2.0
//
// AC 1 (task 13 brief): "a full auth-code round trip works with config only". This is the ONLY
// evidence for that claim in the whole test pyramid — every lower-tier suite in this package
// fakes the identity provider (tests/fixtures/jwks.ts), which cannot observe what a real IdP's
// cross-site redirect back to /auth/callback does to SameSite cookie delivery, nor whether a real
// browser honours the `__Host-` cookie prefix over this fixture server's own scheme (M11).
//
// The session cookie's NAME is hardcoded here (`SESSION_COOKIE_NAME`, tests/e2e/constants.ts)
// rather than imported from src/http/cookies.ts: this suite drives the running fixture server as
// an external black box, so it treats the cookie name as part of the OBSERVABLE contract this AC
// is about, not as a fact reached by importing the module under test.
import { expect, test, type BrowserContext, type Page, type Request, type Response, type TestInfo } from '@playwright/test';
import { KEYCLOAK_PASSWORD, KEYCLOAK_USERNAME, SESSION_COOKIE_NAME, ZONE_BASE_PATH } from './constants.js';

async function fillKeycloakLoginForm(page: Page): Promise<void> {
  await page.locator('#username').fill(KEYCLOAK_USERNAME);
  await page.locator('#password').fill(KEYCLOAK_PASSWORD);
  await page.locator('#kc-login').click();
}

/** Submits the Keycloak login form and reports whether it actually reached the guarded page,
 * rather than throwing on a failure — see the § 9.2 test's own comment for why a submission can
 * legitimately fail here. */
async function attemptKeycloakLogin(page: Page): Promise<boolean> {
  await fillKeycloakLoginForm(page);
  try {
    await page.getByTestId('guarded-heading').waitFor({ state: 'visible', timeout: 5_000 });
    return true;
  } catch {
    return false;
  }
}

/** The session cookie's value, or `undefined`. The value is compared, never printed. */
async function readSessionCookieValue(context: BrowserContext): Promise<string | undefined> {
  return (await context.cookies()).find((c) => c.name === SESSION_COOKIE_NAME)?.value;
}

/** SMA-652: explains a failed fresh-tab check. CI keeps only the task's stdout and stderr (no
 * Playwright attachment or trace survives the runner), so the same text goes to stderr AND to a
 * text/plain attachment. Paths, statuses, cookie NAMES only: a Keycloak URL carries `state`, a
 * callback URL carries `code`, and a cookie value is a credential. */
async function reportFreshTabFailure(testInfo: TestInfo, context: BrowserContext, fresh: Page, response: Response | null, authRequests: readonly string[]): Promise<void> {
  const chain: string[] = [];
  let request: Request | null = response?.request() ?? null;
  while (request !== null) {
    const hop = await request.response();
    const url = new URL(request.url());
    chain.unshift(`${String(hop?.status() ?? 0)} ${url.origin}${url.pathname}`);
    request = request.redirectedFrom();
  }
  const finalUrl = new URL(fresh.url());
  const cookies = (await context.cookies()).map((c) => `${c.name} domain=${c.domain} path=${c.path}`);
  const text = [
    'SMA-652 fresh-tab diagnostics',
    `redirect chain (${String(chain.length)} hop(s)): ${chain.length === 0 ? '(no response)' : chain.join(' -> ')}`,
    `final page: ${finalUrl.origin}${finalUrl.pathname}`,
    `cookies (${String(cookies.length)}): ${cookies.join('; ')}`,
    `auth requests after the primary completed (${String(authRequests.length)}): ${authRequests.join(', ') || '(none)'}`,
  ].join('\n');
  console.error(text);
  await testInfo.attach('sma-652-fresh-tab-diagnostics', { body: text, contentType: 'text/plain' });
}

test('AC 1: the full authorization-code round trip works with config only', async ({ page, context, baseURL }) => {
  await page.goto(`${ZONE_BASE_PATH}/guarded`);
  await fillKeycloakLoginForm(page);

  await expect(page.getByTestId('guarded-heading')).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`${ZONE_BASE_PATH}/guarded$`));

  const cookies = await context.cookies();
  const sessionCookie = cookies.find((c) => c.name === SESSION_COOKIE_NAME);
  expect(sessionCookie, 'the __Host-pgs_sid cookie must be set after a successful login').toBeDefined();
  if (sessionCookie === undefined) throw new Error('unreachable');

  // M11: does a __Host- prefixed, Secure cookie actually get set over this fixture server's own
  // `http://127.0.0.1` origin in Chromium? Loopback addresses are a "potentially trustworthy
  // origin" per the Secure Contexts spec, which is what this assertion is measuring in practice —
  // if it failed, `sessionCookie` above would already be undefined (Chromium refuses to store a
  // Secure cookie set over plain HTTP on a non-trustworthy origin), so REACHING this line at all
  // is the passing result for M11. Recorded here rather than skipped so a future regression in
  // Chromium's secure-context handling for loopback addresses reds this test, loudly.
  expect(sessionCookie.httpOnly).toBe(true);
  expect(sessionCookie.secure).toBe(true);
  expect(sessionCookie.sameSite).toBe('Lax');
  expect(sessionCookie.path).toBe('/');
  expect(sessionCookie.domain).toBe(new URL(baseURL ?? '').hostname); // no Domain beyond the host

  // Ground truth from the fixture server's own debug endpoint (test-only — see
  // fixture-server.ts), not a heuristic: the cookie value must never contain the real access
  // token, which only ever lives in the Redis-backed session record.
  //
  // Fetched via page.evaluate (the BROWSER's own fetch), not page.request: Playwright's separate
  // APIRequestContext does not extend Chromium's own "loopback is a secure context" allowance to
  // a Secure cookie, so it never attaches __Host-pgs_sid to a plain `http://127.0.0.1` request
  // (measured — the debug endpoint 404s with no cookie header at all). The in-page fetch uses the
  // exact cookie jar the login flow itself just populated.
  const debugBody = await page.evaluate(async (path) => {
    const res = await fetch(path);
    if (!res.ok) throw new Error(`debug endpoint returned ${String(res.status)}`);
    return (await res.json()) as { accessToken: string; refreshToken: string | null };
  }, `${ZONE_BASE_PATH}/__test__/session`);
  expect(sessionCookie.value).not.toContain(debugBody.accessToken);
  // The sid is 32 random bytes (base64url) with none of a JWT's structure (three dot-separated
  // segments) — an independent structural check alongside the direct containment check above.
  expect(sessionCookie.value).not.toMatch(/^[\w-]+\.[\w-]+\.[\w-]+$/);

  // M6 part 3 / fix round 1, Important 3: `offline_access` being present in the realm fixture's
  // defaultClientScopes was previously proven only by the ABSENCE of a failure (Keycloak rejects
  // the whole exchange with `error: 'not_allowed'` without it — see the M6 measurement). Nothing
  // asserted a refresh token was actually ISSUED, and a provider silently returning none is
  // exactly the named failure mode (design doc): the single-flight refresh would then have
  // nothing to refresh. Asserted here against the real token response.
  expect(debugBody.refreshToken, 'Keycloak must actually issue a refresh_token for offline_access to mean anything').not.toBeNull();
});

test('§ 9.2: two concurrent logins mint distinct txn cookies, and one completion signs in the whole context', async ({ context }, testInfo) => {
  const tab1 = await context.newPage();
  const tab2 = await context.newPage();

  // Both tabs START a login CONCURRENTLY — this is the property § 9.2 is actually about: each
  // `/auth/login` call mints its OWN __Host-pgs_txn_<id> cookie (routes.ts's per-transaction
  // cookie, never a single fixed name), so two logins racing does not let one overwrite the
  // other's secret. Asserted directly below, against real Keycloak, rather than inferred.
  await Promise.all([tab1.goto(`${ZONE_BASE_PATH}/guarded`), tab2.goto(`${ZONE_BASE_PATH}/guarded`)]);

  // m1 (fix round 1): this is the SINGLE assertion in this test that reds if routes.ts regressed
  // to a fixed transaction-cookie name instead of `txnCookieName(txnId)` — a fixed name would
  // still let one tab's login complete (the second /auth/login would just overwrite the first
  // cookie, and whichever transaction is left standing can still finish), so the completion
  // logic below this point does NOT independently catch that regression. Do not "simplify" this
  // count away believing the rest of the test covers it.
  const txnCookiesAfterBothStarted = (await context.cookies()).filter((c) => c.name.startsWith('__Host-pgs_txn_'));
  expect(txnCookiesAfterBothStarted, 'two concurrent /auth/login calls must mint two DISTINCT txn cookies, not one overwriting the other').toHaveLength(2);

  // MEASURED while building this test: Keycloak 26.4 (dev mode) shares ONE `KC_RESTART` /
  // `AUTH_SESSION_ID` cookie pair across the WHOLE browser context, not one per tab, so two tabs
  // that both just navigated to its `/auth` endpoint race for that single shared slot, and
  // NONDETERMINISTICALLY either tab's login form can point at a session Keycloak no longer
  // recognises ("Your login attempt timed out"). That is Keycloak's session model, not this
  // package's; the txn-cookie count above is what proves § 9.2. What follows completes login on
  // WHICHEVER tab is still valid.
  //
  // SMA-652 (measured on Keycloak 26.4.7; global-setup.ts uses the floating tag `keycloak:26.4`):
  // a Keycloak login page must never stay open, and never be reused, once the other login has
  // completed. The two pages also race for `KC_AUTH_SESSION_HASH`; the losing page's
  // `authChecker.js` (`checkAuthSession`, one-shot, 1000 ms after load) calls `location.reload()`
  // on the mismatch, and `beforeunload` does not cancel that timer — so a later `goto` on that tab
  // can reach /guarded (200) and then be overridden by the reload, leaving the tab on the Keycloak
  // form. An open login page also polls every 2 s and may follow the SSO session into
  // /auth/callback, where `txn_missing` -> /auth/login would delete the shared session. So each
  // Keycloak tab is closed as soon as the test no longer needs it, and the shared-session
  // property is proved on a FRESH page, which no Keycloak document can navigate.
  // Evidence: docs/superpowers/specs/2026-09-19-sma-652-auth-two-tab-e2e-flake-design.md.
  const tab1Completed = await attemptKeycloakLogin(tab1);
  testInfo.annotations.push({ type: 'sma-652-mode', description: tab1Completed ? 'tab1-won' : 'tab1-lost' });
  let primary: Page;
  let secondary: Page | null;
  if (tab1Completed) {
    primary = tab1;
    secondary = tab2;
  } else {
    await tab1.close();
    const tab2Completed = await attemptKeycloakLogin(tab2);
    expect(tab2Completed, 'at least one of the two concurrently-started logins must complete on its first Keycloak submission').toBe(true);
    primary = tab2;
    secondary = null;
  }
  await expect(primary.getByTestId('guarded-heading')).toBeVisible();

  const pageLabels = new Map<Page, string>([[primary, 'primary']]);
  if (secondary !== null) pageLabels.set(secondary, 'secondary');
  const authRequests: string[] = [];
  context.on('request', (request) => {
    if (!request.isNavigationRequest()) return;
    const { pathname } = new URL(request.url());
    if (pathname !== `${ZONE_BASE_PATH}/auth/login` && pathname !== `${ZONE_BASE_PATH}/auth/callback`) return;
    authRequests.push(`${pageLabels.get(request.frame().page()) ?? 'other'} ${pathname}`);
  });

  const sidBefore = await readSessionCookieValue(context);
  expect(sidBefore !== undefined, 'the primary login must have set __Host-pgs_sid').toBe(true);
  if (secondary !== null) await secondary.close();

  const fresh = await context.newPage();
  pageLabels.set(fresh, 'fresh');
  const response = await fresh.goto(`${ZONE_BASE_PATH}/guarded`);
  try {
    // The heading alone cannot tell the shared cookie apart from a SILENT SSO re-login
    // (/guarded -> /auth/login -> Keycloak -> /auth/callback -> a NEW session -> /guarded). The
    // one-hop check and the unchanged sid both fail on that path.
    expect(response !== null, 'the fresh tab navigation must produce a response').toBe(true);
    if (response === null) throw new Error('unreachable');
    expect(response.status()).toBe(200);
    expect(response.request().redirectedFrom() === null, 'the fresh tab must reach /guarded in ONE hop, with no login redirect').toBe(true);
    expect(new URL(response.url()).pathname).toBe(`${ZONE_BASE_PATH}/guarded`);
    await expect(fresh.getByTestId('guarded-heading')).toBeVisible();
    const sidNow = await readSessionCookieValue(context);
    expect(sidNow === sidBefore, 'the fresh tab must reuse the SAME __Host-pgs_sid, not a new session').toBe(true);
  } catch (error) {
    await reportFreshTabFailure(testInfo, context, fresh, response, authRequests);
    throw error;
  }
});
