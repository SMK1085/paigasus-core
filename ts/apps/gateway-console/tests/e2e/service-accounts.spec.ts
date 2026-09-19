// SPDX-License-Identifier: Apache-2.0
//
// The settings pages in the single-zone tier (SMA-636 spec § 7.2 rows 1, 2, 3, 5 and 6). The world
// is stateful (support/world.ts): an account, a key and a grant that one action makes are what the
// next render reads. Row 4, the token exposure, is api-key-token.spec.ts.
import { Code } from '@connectrpc/connect';
import { denial } from '@paigasus/console-core/testing';
import { signIn, waitForHydration } from './support/login';
import { ORG_ID, PROJECT_ID, PROJECT_NAME, SEEDED_SA_ID, TOKEN_PREFIX } from './support/world';
import { expect, test } from './support/harness';

const ORG_PATH = `/gateway/orgs/${ORG_ID}`;
const PROJECT_PATH = `/gateway/orgs/${ORG_ID}/projects/${PROJECT_ID}`;

test('R13: create an account that can call models, issue a key once, revoke it, archive the account (§ 7.2 row 1)', async ({ page, harness }) => {
  await signIn(page, harness, ORG_PATH);
  const section = page.getByTestId('service-accounts');
  const result = section.getByTestId('sa-result');
  // The single-zone map has no IAM zone, so there is no "Manage in IAM" link (§ 4.4).
  await expect(page.getByTestId('manage-in-iam')).toHaveCount(0);

  const create = section.getByRole('form', { name: 'Create service account' });
  await create.getByLabel('Name').fill('ci-bot');
  await create.getByRole('button', { name: 'Create' }).click();
  await expect(result.getByText('Service account created. It can call models.')).toBeVisible();

  await result.getByRole('link', { name: 'Select it' }).click();
  await page.waitForURL((url) => url.searchParams.has('sa'));
  const panel = section.getByTestId('sa-panel');
  // The world answers "yes" only from the gateway_user grant the create made.
  await expect(panel.getByTestId('model-calls')).toHaveAttribute('data-state', 'yes');

  const issue = panel.getByRole('form', { name: 'Issue API key' });
  await issue.getByLabel('Expiry').selectOption('30');
  await issue.getByRole('button', { name: 'Issue key' }).click();
  const shown = section.getByTestId('token-value');
  await expect(shown).toHaveText(new RegExp(`^${TOKEN_PREFIX}[0-9a-f]{32}$`));
  const token = (await shown.textContent()) ?? '';
  expect(page.url()).not.toContain(token);

  await page.reload();
  await waitForHydration(page);
  await expect(section.getByTestId('token-panel')).toHaveCount(0);
  expect(await page.content()).not.toContain(token);
  const key = panel.getByTestId('api-key-row');
  await expect(key).toHaveAttribute('data-status', 'active');

  await key.getByRole('button', { name: 'Revoke', exact: true }).click();
  await key.getByRole('button', { name: 'Confirm revoke' }).click();
  await expect(result.getByText('Key revoked.')).toBeVisible();
  await expect(key).toHaveAttribute('data-status', 'revoked');
  // F17c: the row's own status text, not only the data attribute.
  await expect(key).toContainText('Revoked');

  await panel.getByRole('button', { name: 'Archive', exact: true }).click();
  await panel.getByRole('button', { name: 'Confirm archive' }).click();
  await expect(result.getByText('Service account archived.')).toBeVisible();
  await expect(panel.getByTestId('sa-panel-archived')).toHaveText('Archived');
  await expect(panel.getByTestId('model-calls')).toHaveAttribute('data-state', 'archived');
  await expect(key).toHaveAttribute('data-status', 'inactive');
  await expect(key).toContainText('Inactive (account archived)');
});

test('R14: a failed grant leaves the account unable to call models, and "Allow model calls" repairs it (§ 7.2 row 2)', async ({ page, harness }) => {
  harness.useWorld({ failFirstGrant: true });
  await signIn(page, harness, ORG_PATH);
  const section = page.getByTestId('service-accounts');
  const result = section.getByTestId('sa-result');

  const create = section.getByRole('form', { name: 'Create service account' });
  await create.getByLabel('Name').fill('repair-bot');
  await create.getByRole('button', { name: 'Create' }).click();
  await expect(result.getByText('Service account created, but it cannot call models yet.')).toBeVisible();

  await result.getByRole('link', { name: 'Select it' }).click();
  await page.waitForURL((url) => url.searchParams.has('sa'));
  const panel = section.getByTestId('sa-panel');
  await expect(panel.getByTestId('model-calls')).toHaveAttribute('data-state', 'no');

  await panel.getByRole('button', { name: 'Allow model calls' }).click();
  await expect(result.getByText('Model calls allowed.')).toBeVisible();
  await expect(panel.getByTestId('model-calls')).toHaveAttribute('data-state', 'yes');
});

test('R15: a viewer sees no mutation control, and a user who may not list accounts gets the section denial with HTTP 200 (§ 7.2 row 3)', async ({ page, harness }) => {
  harness.useWorld({ allow: [], seedServiceAccount: true });
  await signIn(page, harness, ORG_PATH);
  await page.goto(harness.url(`${ORG_PATH}?sa=${SEEDED_SA_ID}`));
  await waitForHydration(page);
  const section = page.getByTestId('service-accounts');
  await expect(section.getByTestId('sa-row')).toHaveCount(1);
  await expect(section.getByTestId('sa-panel')).toBeVisible();
  await expect(section.getByRole('form')).toHaveCount(0);
  await expect(section.getByRole('button')).toHaveCount(0);

  harness.useWorld({
    overrides: {
      'serviceAccounts.listServiceAccounts': () => {
        throw denial({ code: Code.PermissionDenied, reason: 'forbidden' });
      },
    },
  });
  const response = await page.goto(harness.url(ORG_PATH));
  expect(response?.status()).toBe(200);
  await expect(page.getByTestId('service-accounts-denied')).toHaveText('You cannot view service accounts here.');
  await expect(page.getByTestId('projects')).toBeVisible();
});

test('R17: a project_admin reaches the project page from "Your projects", and the organization page is a 403 for them (§ 7.2 row 5)', async ({ page, harness }) => {
  harness.useWorld({ projectAdmin: true });
  await signIn(page, harness, '/gateway/overview');

  await page.getByTestId('your-projects').getByRole('link', { name: PROJECT_NAME }).click();
  await page.waitForURL((url) => url.pathname === PROJECT_PATH);
  await expect(page.getByTestId('project-settings')).toBeVisible();
  await expect(page.getByTestId('service-accounts')).toBeVisible();

  const response = await page.goto(harness.url(ORG_PATH));
  expect(response?.status()).toBe(403);
  await expect(page.getByTestId('forbidden-view')).toBeVisible();
});

test('R18: without iam.apikeys in the IAM descriptor, no key control shows and no ListApiKeys call is made (§ 7.2 row 6)', async ({ page, harness }) => {
  harness.useWorld({ seedServiceAccount: true, iamDescriptor: { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar'] } });
  await signIn(page, harness, ORG_PATH);
  const before = harness.iam.callsTo('serviceAccounts.listApiKeys').length;

  await page.goto(harness.url(`${ORG_PATH}?sa=${SEEDED_SA_ID}`));
  await waitForHydration(page);
  const panel = page.getByTestId('sa-panel');
  // Vacuity: the panel renders, and a control that is not a key control still shows.
  await expect(panel.getByRole('button', { name: 'Archive', exact: true })).toBeVisible();
  await expect(panel.getByRole('form', { name: 'Issue API key' })).toHaveCount(0);
  await expect(panel.getByTestId('api-keys')).toHaveCount(0);
  expect(harness.iam.callsTo('serviceAccounts.listApiKeys').length - before).toBe(0);
});
