// SPDX-License-Identifier: Apache-2.0
//
// AC 2 after `helm upgrade` with the gateway zone off (spec § 6.4). The nav must render first (the
// in-phase control), so a count of 0 cannot come from a page that failed. The locator ignores the
// state, so R3 also fails if the entry comes back as an available <a>. R3 cannot tell whether the
// zone-map rule (lib/nav.ts:38-41) or the `absent` rule removed it; both are the chart's contract
// (parent D6). The last clause, "the gateway console Deployment does not exist", runs in
// ci/kind/run.sh specs b.
import { expect, test } from '@playwright/test';
import { TRAEFIK_404_BODY, loginAt } from '../support/login';

test('R3: with the gateway zone disabled, the zone leaves no trace (AC 2)', async ({ page }) => {
  await loginAt(page, '/iam/orgs');
  const nav = page.getByRole('navigation', { name: 'Primary' });
  await expect(nav.getByRole('link', { name: 'Organizations', exact: true })).toBeVisible();
  await expect(nav.getByRole('link', { name: 'Gateway', exact: true })).toHaveCount(0);

  const response = await page.goto('/gateway/overview');
  if (response === null) throw new Error('page.goto(/gateway/overview) returned no response');
  expect(response.status()).toBe(404);
  // Traefik's own body, so a Next not-found page from a surviving gateway console cannot pass.
  expect((await response.text()).trim()).toBe(TRAEFIK_404_BODY);
});
