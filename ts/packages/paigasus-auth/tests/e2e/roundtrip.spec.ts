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
import { expect, test, type Page } from '@playwright/test';
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

test('§ 9.2: two tabs starting a login concurrently both complete', async ({ context }) => {
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
  // that both just navigated to its `/auth` endpoint are racing for that single shared slot.
  // NONDETERMINISTICALLY, either tab's freshly-loaded login form can end up pointing at a session
  // Keycloak no longer recognises ("Your login attempt timed out. Login will start from the
  // beginning.") — which tab loses the race varies between runs. That is Keycloak's own session
  // model under concurrent tabs, not a property of this package: the assertion above (two
  // DISTINCT txn cookies, neither overwriting the other) is what actually proves § 9.2, and it
  // does not depend on which tab wins Keycloak's race. What follows completes login on WHICHEVER
  // tab is still valid, then proves the OTHER tab reaches the same authenticated state too —
  // through the shared __Host-pgs_sid cookie the first one sets, since both tabs are the SAME
  // browser context and therefore the same signed-in user the moment either flow finishes.
  const tab1Completed = await attemptKeycloakLogin(tab1);
  const [primary, secondary] = tab1Completed ? [tab1, tab2] : [tab2, tab1];
  if (!tab1Completed) {
    const tab2Completed = await attemptKeycloakLogin(tab2);
    expect(tab2Completed, 'at least one of the two concurrently-started logins must complete on its first Keycloak submission').toBe(true);
  }
  await expect(primary.getByTestId('guarded-heading')).toBeVisible();

  // The secondary tab never needs Keycloak again — a fresh load of the guarded page finds it
  // already signed in, via the session cookie the primary tab's completion just set.
  await secondary.goto(`${ZONE_BASE_PATH}/guarded`);
  await expect(secondary.getByTestId('guarded-heading')).toBeVisible();

  await tab1.close();
  await tab2.close();
});
