// SPDX-License-Identifier: Apache-2.0
//
// SMA-636 spec § 4.4 and § 7.2 (two-zone tier): the organization page's "Manage in IAM" link comes
// from the zone map's `iam` entry and resolves to the same organization in the IAM zone. It is a
// cross-zone link, so ZoneLink renders it as a plain <a> and the click is a hard navigation. The
// single-zone tier has no IAM zone, and R13 asserts the link is absent there.
import { ORG_ID, ORG_NAME } from './support/world';
import { expect, test } from './support/two-zone-harness';

test('R21: the organization page links to the same organization in the IAM zone (SMA-636 § 4.4)', async ({ page, harness }) => {
  await page.goto(harness.url(`/gateway/orgs/${ORG_ID}`));
  await expect(page.getByTestId('org-settings')).toBeVisible();

  const link = page.getByTestId('manage-in-iam');
  await expect(link).toHaveAttribute('href', `/iam/orgs/${ORG_ID}`);
  await link.click();

  await page.waitForURL(`${harness.origin}/iam/orgs/${ORG_ID}`);
  await expect(page.getByRole('heading', { level: 1, name: ORG_NAME })).toBeVisible();
});
