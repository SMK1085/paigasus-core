// SPDX-License-Identifier: Apache-2.0
//
// AC 1. The login flows under the /iam basePath (spec § 7.1): the proxy, /auth/login, the callback's
// redirect_uri, and requireSession(). And the provisioning order (spec § 4.5): IAM provisions a
// principal only inside a bearer-enforced call, so GetServiceInfo must come before Introspect.
import { ORG_NAME, OTHER_PROJECT_NAME, TEAM_NAME } from './support/world';
import { redirectChain, signIn } from './support/login';
import { expect, test } from './support/harness';

test('R2: an unauthenticated /iam/orgs logs in through the IdP and lands on "Your organizations" (AC 1)', async ({ page, harness }) => {
  const { accessToken, response } = await signIn(page, harness);

  const chain = await redirectChain(response);
  const idpOrigin = new URL(harness.idp.issuer).origin;
  const hops = chain.map(({ url }) => (url.origin === idpOrigin ? 'idp' : url.pathname)).filter((hop, index, all) => hop !== 'idp' || all[index - 1] !== 'idp');
  expect(hops).toEqual(['/iam/orgs', '/iam/auth/login', 'idp', '/iam/auth/callback', '/iam/orgs']);
  expect([302, 303, 307]).toContain(chain[0]?.status);
  expect(chain[1]?.url.searchParams.get('returnTo')).toBe('/iam/orgs');

  // Spec § 7.1 part 3: the token request's redirect_uri equals the authorization request's.
  const callback = harness.url('/iam/auth/callback');
  expect(harness.idp.authorizeRedirectUris.at(-1)).toBe(callback);
  expect(harness.idp.tokenRedirectUris.at(-1)).toBe(callback);

  const scopes = page.getByRole('region', { name: 'Your organizations' });
  await expect(scopes.getByRole('link', { name: ORG_NAME })).toBeVisible();
  await expect(scopes.getByRole('link', { name: TEAM_NAME })).toBeVisible();
  // From ListRoleGrants: the default descriptor reports iam.authz.cedar.
  await expect(scopes.getByRole('link', { name: OTHER_PROJECT_NAME })).toBeVisible();

  const mine = harness.iam.calls.filter((call) => call.token === accessToken);
  expect(mine[0]?.method).toBe('serviceInfo.getServiceInfo');
  expect(mine.some((call) => call.method === 'authn.introspect')).toBe(true);
});

test('R3: a first-time identity that IAM has never seen lands on the same screen (AC 1)', async ({ page, harness }) => {
  harness.useWorld({ memberships: false, allow: [] });

  const { accessToken } = await signIn(page, harness);

  expect(harness.iam.provisioned.has(accessToken)).toBe(true);
  const mine = harness.iam.calls.filter((call) => call.token === accessToken);
  const firstProvisioning = mine.findIndex((call) => call.method === 'serviceInfo.getServiceInfo');
  const firstIntrospect = mine.findIndex((call) => call.method === 'authn.introspect');
  expect(firstProvisioning).toBe(0);
  expect(firstIntrospect).toBeGreaterThan(firstProvisioning);

  const scopes = page.getByRole('region', { name: 'Your organizations' });
  await expect(scopes.getByText('You have no organizations yet')).toBeVisible();
  await expect(page.getByRole('region', { name: 'All organizations' })).toHaveCount(0);
  await expect(page.getByRole('form', { name: 'Create organization' })).toHaveCount(0);
});

test('R12: sign out posts /iam/auth/logout, and /iam/orgs then redirects to login again (AC 1)', async ({ page, harness }) => {
  await signIn(page, harness);
  const logout = page.waitForRequest((request) => request.method() === 'POST' && new URL(request.url()).pathname === '/iam/auth/logout');

  // The user menu is the LAST menu trigger in the header (the org switcher comes before it).
  await page.getByRole('banner').locator('button[aria-haspopup="menu"]').last().click();
  await page.getByRole('menuitem', { name: 'Sign out' }).click();
  const request = await logout;
  // waitForRequest resolves when the POST is SENT, and waitForLoadState() would resolve at once (the
  // old page is loaded). So wait for the post-logout page instead: handleLogout clears
  // __Host-pgs_sid in its 302 to the IdP's end_session_endpoint (after it revokes at the IdP), and
  // the fake IdP sends the browser back to `${PAIGASUS_PUBLIC_ORIGIN}/iam/`. When that URL is
  // current, the clearing response has arrived.
  await page.waitForURL((url) => /^\/iam\/?$/.test(url.pathname));
  expect((await request.response())?.status()).toBe(302);

  expect((await page.context().cookies()).filter((cookie) => cookie.name === '__Host-pgs_sid')).toEqual([]);
  const again = await page.request.get(harness.url('/iam/orgs'), { maxRedirects: 0 });
  expect([302, 303, 307]).toContain(again.status());
  expect(new URL(again.headers()['location'] ?? '', harness.origin).pathname).toBe('/iam/auth/login');
});
