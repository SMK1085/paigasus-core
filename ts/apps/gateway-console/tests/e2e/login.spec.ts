// SPDX-License-Identifier: Apache-2.0
//
// AC 1. The login flows under the /gateway basePath (spec § 7.1): the proxy, /auth/login, the
// callback's redirect_uri, and requireSession(). And the provisioning call (spec § 4.5):
// IAM provisions a principal only inside a bearer-enforced call. WhoAmI IS that call (SMA-632),
// so the login makes one gRPC call where it used to make two.
import { signIn } from './support/login';
import { expect, test } from './support/harness';

test('R2: an unauthenticated /gateway/overview logs in through the IdP and lands on the zone overview (AC 1)', async ({ page, harness }) => {
  const { accessToken } = await signIn(page, harness, '/gateway/overview');

  await expect(page.getByTestId('zone-overview')).toBeVisible();

  // Filtered to THIS login's own token: harness.iam.calls accumulates across the whole worker (it
  // is never cleared between tests), so an unfiltered check can pick up an earlier test's calls
  // instead of this one's — MEASURED, when this test ran after R4/R4b/R7 in the same worker.
  //
  // IAM provisions a principal only inside a bearer-enforced call, and the login callback's own
  // first such call is the gRPC WhoAmI the principal resolver makes (lib/auth.ts, byte-identical
  // to iam-console's own — its login.spec.ts R2 asserts the same call as `mine[0]`). WhoAmI also
  // answers the memberships myScopes() renders (SMA-632), so no separate Introspect call follows
  // it. MEASURED (debug capture of `mine.map(call => call.method)`): the discovery package's HTTP
  // service-info probe (`http.getServiceInfo`) runs LATER, during the overview page's own render —
  // after, not before, this first WhoAmI — so that call is not the one this assertion is about.
  const mine = harness.iam.calls.filter((call) => call.token === accessToken);
  expect(mine[0]?.method).toBe('authn.whoAmI');
});

test('R3: a first-time, unprovisioned user reaches the same screen, with no organization switcher (AC 1)', async ({ page, harness }) => {
  harness.useWorld({ memberships: false });

  await signIn(page, harness, '/gateway/overview');

  await expect(page.getByTestId('zone-overview')).toBeVisible();
  // myScopes() returns no organization-kind scope, so switcherOrgs() is empty and the Switcher
  // renders nothing (its own early return) rather than an empty menu.
  await expect(page.getByText(/^Organization:/)).toHaveCount(0);
});

test('R6: sign out, then a protected page needs a login again (AC 1)', async ({ page, harness }) => {
  await signIn(page, harness, '/gateway/overview');
  const logout = page.waitForRequest((request) => request.method() === 'POST' && new URL(request.url()).pathname === '/gateway/auth/logout');

  // The user menu is the last (and only) menu trigger in the header for this zone.
  await page.getByRole('banner').locator('button[aria-haspopup="menu"]').last().click();
  await page.getByRole('menuitem', { name: 'Sign out' }).click();
  const request = await logout;
  // waitForRequest resolves when the POST is SENT, and waitForLoadState() would resolve at once
  // (the old page is loaded). So wait for the post-logout page instead: handleLogout clears
  // __Host-pgs_sid in its 302 to the IdP's end_session_endpoint (after it revokes at the IdP), and
  // the fake IdP sends the browser back to `${PAIGASUS_PUBLIC_ORIGIN}/gateway/`. When that URL is
  // current, the clearing response has arrived.
  await page.waitForURL((url) => /^\/gateway\/?$/.test(url.pathname));
  expect((await request.response())?.status()).toBe(302);
  expect((await page.context().cookies()).filter((cookie) => cookie.name === '__Host-pgs_sid')).toEqual([]);

  // NOT page.goto(): the fake IdP approves every /authorize call unconditionally (it has no
  // concept of "already logged out"), so a real browser navigation to a protected page would
  // silently complete a BRAND NEW login and land back on /gateway/overview — proving nothing about
  // sign-out. maxRedirects: 0 stops at the FIRST hop instead, which is the one requireSession()
  // itself produces, and is what "a protected page needs a login again" actually means here.
  const again = await page.request.get(harness.url('/gateway/overview'), { maxRedirects: 0 });
  expect([302, 303, 307]).toContain(again.status());
  expect(new URL(again.headers()['location'] ?? '', harness.origin).pathname).toBe('/gateway/auth/login');
});
