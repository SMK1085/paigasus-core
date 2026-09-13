// SPDX-License-Identifier: Apache-2.0
//
// AC 2 in both directions (spec § 6.3). IAM is authoritative: a denial renders the 403 (page: the
// forbidden() boundary with a REAL HTTP 403; section and form: inline, with the correlation id).
// And mayI() only hides: a Server Action whose button is hidden still reaches IAM. R6 posts through
// the REAL browser form, rendered while mayI() said yes, after IsAuthorized has flipped to no.
import { PRESENTATION_COPY } from '../../app/_components/error-copy';
import { denial } from '../support/fake-iam';
import { forbiddenViewCorrelation } from './support/correlation';
import { signIn } from './support/login';
import { ALL_ACTIONS, ORG_ID, ORG_NAME, ORG_PRN } from './support/world';
import { expect, test } from './support/harness';

test('R4: a denied page read is an HTTP 403 with the 403 view inside the shell (AC 2)', async ({ page, harness }) => {
  await signIn(page, harness);
  harness.useWorld({
    overrides: {
      // No explicit correlation id: the fake stamps the id of the call, as IAM does. That is the id
      // proxy.ts minted for this request, because IAM adopts a UUID it receives (Task 12). So the
      // view, IAM and the console log must all show ONE id.
      'tenancy.getOrganization': () => {
        throw denial();
      },
    },
  });
  const before = harness.iam.callsTo('tenancy.getOrganization').length;
  const pagePath = `/iam/orgs/${ORG_ID}`;

  const response = await page.goto(harness.url(pagePath));

  expect(response?.status()).toBe(403);
  await expect(page.getByTestId('forbidden-view')).toBeVisible();
  await expect(page.getByRole('heading', { name: PRESENTATION_COPY.forbidden.title })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Primary' })).toBeVisible();
  const denied = harness.iam
    .callsTo('tenancy.getOrganization')
    .slice(before)
    .filter((call) => (call.request as { prn: string }).prn === ORG_PRN);
  expect(denied.length).toBeGreaterThan(0);
  // The paigasus-correlation-id header as it ARRIVED at the fake: the id proxy.ts minted. Every call
  // of one request carries the same id (lib/iam.ts reads it once per request).
  const correlationId = denied.at(-1)?.correlationId ?? '';
  expect(correlationId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);

  if (forbiddenViewCorrelation() === 'header') {
    // The view shows exactly the id IAM adopted for the denied call.
    await expect(page.getByTestId('correlation-id')).toHaveText(correlationId);
  } else {
    // The fallback: no id in the view, and callIam logged one iam.call_failed line with that id
    // and the page path.
    await expect(page.getByTestId('correlation-id')).toHaveCount(0);
    await expect
      .poll(() =>
        harness
          .serverOutput()
          .split('\n')
          .some((line) => line.includes('"event":"iam.call_failed"') && line.includes(`"correlation_id":"${correlationId}"`) && line.includes(`"path":"${pagePath}"`)),
      )
      .toBe(true);
  }
});

test('R5: mayI says yes and IAM denies, so the 403 renders inline in the section (AC 2)', async ({ page, harness }) => {
  await signIn(page, harness);
  harness.useWorld({
    overrides: {
      'tenancy.listOrganizations': () => {
        throw denial({ correlationId: 'corr-e2e-section-403' });
      },
    },
  });
  const before = harness.iam.callsTo('tenancy.listOrganizations').length;

  const response = await page.reload();

  expect(response?.status()).toBe(200);
  const section = page.getByRole('region', { name: 'All organizations' });
  await expect(section).toContainText(PRESENTATION_COPY.forbidden.title);
  // A section 403 shows the id in BOTH correlation modes: the PaigasusError is right there (Task 12).
  await expect(section.getByTestId('correlation-id')).toHaveText('corr-e2e-section-403');
  expect(harness.iam.callsTo('tenancy.listOrganizations').length - before).toBe(1);
});

test('R6: mayI says no, so the button hides, the reads render, and a POST from the form still reaches IAM (AC 2)', async ({ page, harness }) => {
  await signIn(page, harness);
  const form = page.getByRole('form', { name: 'Create organization' });
  await expect(form).toBeVisible();

  // IsAuthorized now refuses CreateOrganization. The form above was rendered before this change.
  harness.useWorld({ allow: ALL_ACTIONS.filter((action) => action !== 'CreateOrganization') });
  const before = harness.iam.callsTo('tenancy.createOrganization').length;
  await form.getByLabel('Slug').fill('e2e-org');
  await form.getByLabel('Name').fill('E2E Org');
  const post = page.waitForResponse((candidate) => candidate.request().method() === 'POST' && new URL(candidate.url()).pathname === '/iam/orgs');
  await form.getByRole('button', { name: 'Create' }).click();
  const actionResponse = await post;

  // A Server Action POST under the basePath, through the TLS terminator, passed Next's origin check.
  expect(actionResponse.status()).toBe(200);
  expect((await actionResponse.request().allHeaders())['next-action']).toBeTruthy();
  const created = harness.iam.callsTo('tenancy.createOrganization').slice(before);
  expect(created).toHaveLength(1);
  expect(created[0]?.request).toMatchObject({ slug: 'e2e-org', name: 'E2E Org' });

  // The action's revalidatePath refreshed the page, BEFORE any reload (P5b-16): the new
  // organization is in "All organizations" (the world's ListOrganizations now returns it). Delete
  // the revalidatePath call and the old list stays, so this fails. A WRONG path fails here only
  // when Next matches the path to the page: Next 16.3.4 still refreshes the page for any path.
  await expect(page.getByRole('region', { name: 'All organizations' }).getByRole('link', { name: 'E2E Org' })).toBeVisible();

  await page.reload();
  await expect(page.getByRole('form', { name: 'Create organization' })).toHaveCount(0);
  await expect(page.getByRole('region', { name: 'Your organizations' }).getByRole('link', { name: ORG_NAME })).toBeVisible();
});

test('R7: a denied Server Action shows an inline 403 in the form (AC 2)', async ({ page, harness }) => {
  await signIn(page, harness);
  harness.useWorld({
    overrides: {
      'tenancy.createOrganization': () => {
        throw denial({ correlationId: 'corr-e2e-action-403' });
      },
    },
  });
  const form = page.getByRole('form', { name: 'Create organization' });
  await form.getByLabel('Slug').fill('denied-org');
  await form.getByLabel('Name').fill('Denied Org');

  await form.getByRole('button', { name: 'Create' }).click();

  const error = page.getByTestId('create-organization-error');
  // A form 403 shows the id in BOTH correlation modes (Task 12's FormError).
  await expect(error.getByTestId('correlation-id')).toHaveText('corr-e2e-action-403');
  await expect(form).toBeVisible();
});
