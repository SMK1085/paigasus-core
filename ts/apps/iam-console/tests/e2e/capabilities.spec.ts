// SPDX-License-Identifier: Apache-2.0
//
// AC 4 (spec § 6.6). PAIGASUS_DISCOVERY_*_MS are 1/2/3 ms in the harness, so each page load probes
// the fake's GET /v1/service-info again, and a test changes the IAM state between two loads.
import { AUDIT_ACTION } from './support/world';
import { signIn } from './support/login';
import { expect, test } from './support/harness';

test('R8: iam.audit reported and allowed, so the Audit entry appears and the page works (AC 4)', async ({ page, harness }) => {
  await signIn(page, harness);
  const nav = page.getByRole('navigation', { name: 'Primary' });
  const before = harness.iam.callsTo('audit.listAuditEntries').length;

  await nav.getByRole('link', { name: 'Audit', exact: true }).click();
  await page.waitForURL((url) => url.pathname === '/iam/audit');

  await expect(page.getByRole('heading', { name: 'Audit log' })).toBeVisible();
  await expect(page.getByRole('cell', { name: AUDIT_ACTION })).toBeVisible();
  expect(harness.iam.callsTo('audit.listAuditEntries').length).toBeGreaterThan(before);
});

test('R9: iam.audit not reported, so the entry is absent and /iam/audit is a 404 (AC 4)', async ({ page, harness }) => {
  harness.useWorld({ descriptor: { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar'] } });
  await signIn(page, harness);
  const nav = page.getByRole('navigation', { name: 'Primary' });

  await expect(nav.getByRole('link', { name: 'Organizations', exact: true })).toBeVisible();
  await expect(nav.getByRole('link', { name: 'Audit', exact: true })).toHaveCount(0);
  const before = harness.iam.callsTo('audit.listAuditEntries').length;
  const response = await page.goto(harness.url('/iam/audit'));
  expect(response?.status()).toBe(404);
  expect(harness.iam.callsTo('audit.listAuditEntries').length).toBe(before);
});

test('R10: IAM degraded, so the entries are disabled with a reason and /iam/audit shows the degraded view (AC 4)', async ({ page, harness }) => {
  await signIn(page, harness);
  harness.useWorld({ descriptor: { status: 503 } });

  await page.goto(harness.url('/iam/orgs'));

  const nav = page.getByRole('navigation', { name: 'Primary' });
  for (const label of ['Organizations', 'Audit']) {
    const entry = nav.getByRole('link', { name: label, exact: true });
    await expect(entry).toHaveAttribute('aria-disabled', 'true');
    const reasonId = await entry.getAttribute('aria-describedby');
    expect(reasonId).toBeTruthy();
    await expect(page.locator(`[id="${String(reasonId)}"]`)).toHaveText(/\S/);
  }

  const before = harness.iam.callsTo('audit.listAuditEntries').length;
  const response = await page.goto(harness.url('/iam/audit'));
  expect(response?.status()).toBe(200);
  await expect(page.getByTestId('audit-degraded')).toBeVisible();
  // A degraded IAM never reaches ListAuditEntries.
  expect(harness.iam.callsTo('audit.listAuditEntries').length).toBe(before);
});
