// SPDX-License-Identifier: Apache-2.0
//
// AC 3 (task 13 brief): "logout revokes server-side — a stolen cookie is dead immediately after".
// The proof replays the CAPTURED cookie value in a FRESH browser context (a different cookie jar
// entirely), rather than reloading the original page and checking it looks logged out — the
// latter would only prove the original browser's own cookie was cleared, not that the server-side
// record the cookie named is actually gone. A stolen cookie is exactly a copy living somewhere
// else; this test is that somewhere else.
import { expect, test } from '@playwright/test';
import { KEYCLOAK_PASSWORD, KEYCLOAK_USERNAME, SESSION_COOKIE_NAME, ZONE_BASE_PATH } from './constants.js';

test('AC 3: a stolen cookie is dead immediately after logout', async ({ page, context, browser, baseURL }) => {
  await page.goto(`${ZONE_BASE_PATH}/guarded`);
  await page.locator('#username').fill(KEYCLOAK_USERNAME);
  await page.locator('#password').fill(KEYCLOAK_PASSWORD);
  await page.locator('#kc-login').click();
  await expect(page.getByTestId('guarded-heading')).toBeVisible();

  const cookiesBeforeLogout = await context.cookies();
  const stolen = cookiesBeforeLogout.find((c) => c.name === SESSION_COOKIE_NAME);
  expect(stolen, 'must capture a live session cookie before logging out').toBeDefined();
  if (stolen === undefined) throw new Error('unreachable');

  // POST /auth/logout via the real logout form (routes.ts requires POST specifically — see that
  // file's own header on why logout is never a GET).
  await page.getByTestId('logout-button').click();

  // M12: routes.ts's handleLogout deliberately omits `id_token_hint` from the end-session
  // redirect (the raw ID token JWT is never stored — see that function's own doc comment) and
  // relies on openid-client appending `client_id` unconditionally instead. Does Keycloak 26.4
  // honour that combination, or does it interpose a confirmation page first? Asserted here as a
  // real, timing-sensitive check, not inferred from the test merely finishing: if Keycloak showed
  // a confirmation interstitial instead of completing the redirect, this would time out waiting
  // for the public page rather than silently passing.
  await expect(page.getByTestId('public-heading')).toBeVisible();

  // The attacker's browser: a FRESH context (its own cookie jar), seeded with nothing but the
  // captured cookie value.
  if (baseURL === undefined) throw new Error('playwright.config.ts must configure use.baseURL');
  const attackerContext = await browser.newContext({ baseURL, ignoreHTTPSErrors: true });
  await attackerContext.addCookies([stolen]);
  const attackerPage = await attackerContext.newPage();
  await attackerPage.goto(`${ZONE_BASE_PATH}/guarded`);

  // The guarded page's own redirect chain ends on Keycloak's hosted login form, not literally on
  // this app's transient /auth/login path (that route itself redirects onward immediately) — so
  // the observable proof is "not on the authenticated guarded page, and back at the IdP's login
  // form", not a literal URL match on /auth/login.
  await expect(attackerPage.getByTestId('guarded-heading')).toHaveCount(0);
  await expect(attackerPage.locator('#username')).toBeVisible();

  await attackerContext.close();
});
