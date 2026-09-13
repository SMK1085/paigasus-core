// SPDX-License-Identifier: Apache-2.0
//
// The proxy's matcher (spec § 7.5) must let static assets through with NO cookie. Without it, the
// default matcher is a catch-all and every CSS and JS file of the public page is a login redirect.
import { expect, test } from './support/harness';

test('R1: the public page /iam/ loads with no cookie, with its CSS and JS (shell)', async ({ page, harness }) => {
  const assets: { pathname: string; status: number; type: string }[] = [];
  page.on('response', (response) => {
    const url = new URL(response.url());
    if (url.pathname.startsWith('/iam/_next/static/')) assets.push({ pathname: url.pathname, status: response.status(), type: response.request().resourceType() });
  });

  const response = await page.goto(harness.url('/iam/'));

  expect(response?.status()).toBe(200);
  // Next may normalise /iam/ to /iam (trailingSlash is off). Either way the proxy did not redirect to login.
  expect(new URL(page.url()).pathname).toMatch(/^\/iam\/?$/);
  expect(assets.filter((asset) => asset.type === 'stylesheet').length).toBeGreaterThan(0);
  expect(assets.filter((asset) => asset.type === 'script').length).toBeGreaterThan(0);
  expect(assets.filter((asset) => asset.status !== 200)).toEqual([]);
  // The CSS applied: globals.css paints the body with the design token.
  expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).not.toBe('rgba(0, 0, 0, 0)');
  expect((await page.context().cookies()).filter((cookie) => cookie.name === '__Host-pgs_sid')).toEqual([]);
});
