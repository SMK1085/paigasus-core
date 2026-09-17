// SPDX-License-Identifier: Apache-2.0
//
// AC 4 (spec § 6.6, plan D17). PAIGASUS_DISCOVERY_*_MS are 1/2/3 ms in the harness, so each page
// load probes the fake gateway's GET /v1/service-info again, and a test changes its answer between
// two loads. This also proves the capability gate reads a LIVE state, not one cached at login time.
import { signIn } from './support/login';
import { expect, test } from './support/harness';

test('R4: the gateway reported degraded, so the Overview nav entry is disabled with a reason (AC 4)', async ({ page, harness }) => {
  await signIn(page, harness, '/gateway/overview');
  harness.useWorld({ gatewayDescriptor: { status: 503 } });

  await page.reload();

  await expect(page.getByTestId('gateway-state')).toHaveAttribute('data-state', 'degraded');
  await expect(page.getByTestId('gateway-reason')).toBeVisible();

  const nav = page.getByRole('navigation', { name: 'Primary' });
  const entry = nav.getByRole('link', { name: 'Overview', exact: true });
  await expect(entry).toHaveAttribute('aria-disabled', 'true');
  const reasonId = await entry.getAttribute('aria-describedby');
  expect(reasonId).toBeTruthy();
  await expect(page.locator(`[id="${String(reasonId)}"]`)).toHaveText(/\S/);
});

test('R4b: available but the streaming capability is off, so data-streaming is false while data-state stays available (AC 4)', async ({ page, harness }) => {
  await signIn(page, harness, '/gateway/overview');
  // Available (a 200 from the probe), but the descriptor no longer lists gateway.chat.stream.
  harness.useWorld({ gatewayDescriptor: { service: 'gateway', version: '0.0.0-e2e', capabilities: [] } });

  await page.reload();

  await expect(page.getByTestId('gateway-state')).toHaveAttribute('data-state', 'available');
  await expect(page.getByTestId('gateway-streaming')).toHaveAttribute('data-streaming', 'false');
  const nav = page.getByRole('navigation', { name: 'Primary' });
  await expect(nav.getByRole('link', { name: 'Overview', exact: true })).not.toHaveAttribute('aria-disabled', 'true');
});
