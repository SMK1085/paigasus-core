// SPDX-License-Identifier: Apache-2.0
//
// AC 3 (task 13 brief): "logout revokes server-side — a stolen cookie is dead immediately after".
// The proof replays the CAPTURED cookie value in a FRESH browser context (a different cookie jar
// entirely), rather than reloading the original page and checking it looks logged out — the
// latter would only prove the original browser's own cookie was cleared, not that the server-side
// record the cookie named is actually gone. A stolen cookie is exactly a copy living somewhere
// else; this test is that somewhere else.
import { expect, test } from '@playwright/test';
import { KEYCLOAK_CLIENT_ID, KEYCLOAK_PASSWORD, KEYCLOAK_USERNAME, SESSION_COOKIE_NAME, ZONE_BASE_PATH } from './constants.js';

test('AC 3: a stolen cookie is dead immediately after logout', async ({ page, context, browser, baseURL }) => {
  await page.goto(`${ZONE_BASE_PATH}/guarded`);
  await page.locator('#username').fill(KEYCLOAK_USERNAME);
  await page.locator('#password').fill(KEYCLOAK_PASSWORD);
  await page.locator('#kc-login').click();
  await expect(page.getByTestId('guarded-heading')).toBeVisible();

  // SMA-681: the realm does not pin a user id, so the subject comes from the page the fixture
  // server renders (tests/e2e/fixture-server.ts:182).
  const subject = ((await page.getByTestId('principal-subject').textContent()) ?? '').trim();
  expect(subject, 'the guarded page must show the principal subject').not.toBe('');

  const cookiesBeforeLogout = await context.cookies();
  const stolen = cookiesBeforeLogout.find((c) => c.name === SESSION_COOKIE_NAME);
  expect(stolen, 'must capture a live session cookie before logging out').toBeDefined();
  if (stolen === undefined) throw new Error('unreachable');

  // SMA-681: routes.ts's handleLogout sends the stored raw ID token as `id_token_hint`, and
  // openid-client appends `client_id`. Keycloak 26.4 needs the hint to skip its confirmation page
  // when an SSO session is live (SMA-681 spec § 3). This realm grants `offline_access` as a default
  // scope, so no SSO session exists here and no page appears either way (SMA-682): this test proves
  // the hint is SENT, and that Keycloak still completes the redirect with it (§ 3.1 row M-i1).
  //
  // Asserting `public-heading` becomes visible afterward is NOT sufficient on its own (fix round
  // 1, Important 1): `routes.ts`'s handleLogout has a DEGRADED arm — if `buildEndSessionUrl`
  // throws (e.g. discovery failing, or no `end_session_endpoint` advertised), it redirects
  // straight to `runtime.postLogoutRedirectUri` WITHOUT ever contacting Keycloak, and that URI
  // defaults to the public page too (`runtime.ts`'s `${PAIGASUS_PUBLIC_ORIGIN}${basePath}/`). A
  // browser landing on `public-heading` is therefore consistent with BOTH "Keycloak was reached
  // and honoured the redirect" and "Keycloak was never contacted at all" — the assertion alone
  // cannot tell the measured case from the never-happened case.
  //
  // The fix: capture the actual navigation to Keycloak's end-session endpoint and assert on ITS
  // query string directly, so this rests on the wire rather than on inference from `oidc.ts` and
  // `tests/adapters/oidc.test.ts`.
  const [endSessionRequest] = await Promise.all([page.waitForRequest((req) => req.url().includes('/protocol/openid-connect/logout')), page.getByTestId('logout-button').click()]);
  const endSessionUrl = new URL(endSessionRequest.url());
  expect(endSessionUrl.searchParams.get('client_id'), 'Keycloak must actually receive client_id on the end-session request').toBe(KEYCLOAK_CLIENT_ID);
  const hint = endSessionUrl.searchParams.get('id_token_hint');
  expect(hint, 'id_token_hint must be present: the session record stores the raw ID token (SMA-681)').not.toBeNull();
  const parts = (hint ?? '').split('.');
  expect(parts, 'id_token_hint must be a three-part JWT').toHaveLength(3);
  const payload = JSON.parse(Buffer.from(parts[1] ?? '', 'base64url').toString('utf8')) as { sub?: unknown; aud?: unknown };
  expect(payload.sub, 'the hint belongs to the signed-in user').toBe(subject);
  const audiences = Array.isArray(payload.aud) ? payload.aud : [payload.aud];
  expect(audiences, 'the hint was issued to this client').toContain(KEYCLOAK_CLIENT_ID);

  // Only now does completing the redirect chain confirm Keycloak honoured that request rather
  // than interposing a confirmation page — the request-capture above already ruled out the
  // degraded no-Keycloak-contact path, so this assertion means what it says.
  await expect(page.getByTestId('public-heading')).toBeVisible();

  // The attacker's browser: a FRESH context (its own cookie jar), seeded with nothing but the
  // captured cookie value.
  if (baseURL === undefined) throw new Error('playwright.config.ts must configure use.baseURL');
  const attackerContext = await browser.newContext({ baseURL, ignoreHTTPSErrors: true });
  await attackerContext.addCookies([stolen]);

  // Guard the guard: `__Host-` prefixed cookies carry strict rules (secure, no `Domain`
  // attribute, `Path=/`), so `addCookies` can silently drop the cookie rather than accept it. If
  // that happened, the guarded page below would redirect to login for lack of ANY cookie, and the
  // test would pass without ever exercising server-side revocation. Assert the attacker context
  // genuinely holds the replayed cookie, with its original value, before navigating.
  const attackerCookies = await attackerContext.cookies();
  const replayed = attackerCookies.find((c) => c.name === SESSION_COOKIE_NAME);
  expect(replayed, 'the attacker context must actually hold the replayed session cookie').toBeDefined();
  expect(replayed?.value, 'the replayed cookie must carry the original stolen value').toBe(stolen.value);

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
