// SPDX-License-Identifier: Apache-2.0
//
// A session that ended before the user pressed "Create" (final whole-branch review, Important 1).
//
// THE DEFECT THIS COVERS. A Server Action does not reach the browser through Next's app render, so
// a redirect() thrown inside one carries NO basePath: action-handler.js writes the raw url into
// `x-action-redirect`, the client resolves it against the current URL and hard-navigates. With
// requireSession() on the action path the browser therefore lands on `https://<host>/auth/login`,
// OUTSIDE the /iam zone, where nothing serves a login route. The fix returns the missing session as
// an ActionState failure with `presentation: 'relogin'`, so the form shows the "Sign in again" LINK
// (spec § 6.4) and the browser stays put.
//
// THE COOKIE IS REPLACED, NOT CLEARED. proxy.ts checks cookie PRESENCE only (ADR-0017 decision 7),
// so clearing it would make the proxy answer the POST with its own redirect and the action would
// never run. A cookie naming a record the store does not hold is the real expiry shape
// (@paigasus/auth's next/get-session.ts file header), and it is the one the proxy lets through.
import { signIn, waitForHydration } from './support/login';
import { expect, test } from './support/harness';

test('R13: a Server Action whose session ended shows the relogin link and keeps the browser inside /iam (AC 1)', async ({ page, harness }) => {
  await signIn(page, harness);
  const form = page.getByRole('form', { name: 'Create organization' });
  await expect(form).toBeVisible();
  await waitForHydration(page);

  await page.context().addCookies([{ name: '__Host-pgs_sid', value: 'e2e-sid-the-store-never-had', url: harness.origin }]);
  const before = harness.iam.callsTo('tenancy.createOrganization').length;

  await form.getByLabel('Slug').fill('expired-org');
  await form.getByLabel('Name').fill('Expired Org');
  const post = page.waitForResponse((candidate) => candidate.request().method() === 'POST' && new URL(candidate.url()).pathname === '/iam/orgs');
  await form.getByRole('button', { name: 'Create' }).click();
  const actionResponse = await post;

  // The action ran (it was not the proxy that answered), and it never reached IAM.
  expect(actionResponse.status()).toBe(200);
  expect(harness.iam.callsTo('tenancy.createOrganization').length - before).toBe(0);

  const error = page.getByTestId('create-organization-error').getByTestId('form-error');
  await expect(error).toHaveAttribute('data-presentation', 'relogin');
  const link = error.getByRole('link', { name: 'Sign in again' });
  await expect(link).toBeVisible();
  expect(new URL((await link.getAttribute('href')) ?? '', harness.origin).pathname).toBe('/iam/auth/login');

  // THE ASSERTION THE DEFECT BREAKS: the browser is still on the console page, inside the zone.
  // With the redirect on the action path it hard-navigates to /auth/login, outside /iam.
  await expect.poll(() => new URL(page.url()).pathname).toBe('/iam/orgs');
});
