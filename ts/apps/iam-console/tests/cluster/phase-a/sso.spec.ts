// SPDX-License-Identifier: Apache-2.0
//
// AC 4: one login serves both zones. R1 counts IdP requests because Keycloak keeps its own SSO
// cookie: a second console login would pass with no form, so "no form appeared" proves nothing.
// R1-control proves the gateway zone WOULD redirect without the session (spec § 6.3).
import { expect, test } from '@playwright/test';
import { CONSOLE_HOST, IDP_HOST, loginAt, redirectChain, sessionCookie, waitForHydration } from '../support/login';

test('R1: a login in the IAM zone admits the gateway zone with no IdP request (AC 4)', async ({ page, browser }) => {
  await loginAt(page, '/iam/orgs');
  const before = await sessionCookie(page);

  const second = await (await browser.newContext({ baseURL: 'https://console.paigasus.test', ignoreHTTPSErrors: true })).newPage();
  const idpRequests: string[] = [];
  second.on('request', (request) => {
    const url = new URL(request.url());
    if (url.hostname === IDP_HOST) idpRequests.push(url.pathname);
  });
  const response = await second.goto('/gateway/overview');
  if (response === null) throw new Error('page.goto(/gateway/overview) returned no response');

  expect(response.request().redirectedFrom(), 'the document must arrive in one hop, with no redirect').toBeNull();
  expect(response.status()).toBe(200);
  expect(new URL(second.url()).pathname).toBe('/gateway/overview');
  expect(await sessionCookie(second)).toBe(before);
  // The listener stays attached across hydration, so this also counts any IdP request hydration
  // itself makes (review Task 9 (a)).
  await waitForHydration(second);
  expect(idpRequests, 'the gateway zone must not send the browser to the IdP').toEqual([]);
});

test('R1-control: without the session, the gateway zone redirects to its own login (AC 4)', async ({ page }) => {
  expect(await page.context().cookies()).toEqual([]);
  // A browser navigation, not `request.get`: the Node-side request API does not see Chromium's
  // --host-resolver-rules, so it cannot resolve console.paigasus.test (Spec defect 4).
  const response = await page.goto('/gateway/overview');
  if (response === null) throw new Error('page.goto(/gateway/overview) returned no response');
  const [first, second] = await redirectChain(response);
  expect(first?.url.pathname).toBe('/gateway/overview');
  expect([301, 302, 303, 307, 308]).toContain(first?.status ?? 0);
  expect(second?.url.hostname).toBe(CONSOLE_HOST);
  expect(second?.url.pathname).toBe('/gateway/auth/login');
});
