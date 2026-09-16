// SPDX-License-Identifier: Apache-2.0
//
// SMA-513 AC 3 rehearsal (spec § 10.5): two Next apps have never run at one origin in this
// repository before this pull request, and the base path is the only thing separating their
// asset roots. A collision here means a chunk requested at the wrong prefix, which is a 404, not
// a wrong-but-loading asset — so a 404 among these responses IS the failure this row exists to
// catch.
import { expect, test } from './support/two-zone-harness';

type StaticAsset = { readonly pathname: string; readonly status: number; readonly type: string };

test('R12: each zone loads its own _next assets under its own base path (SMA-513 AC 3 rehearsal)', async ({ page, harness }) => {
  const assets: StaticAsset[] = [];
  page.on('response', (response) => {
    const url = new URL(response.url());
    if (url.pathname.includes('/_next/static/')) assets.push({ pathname: url.pathname, status: response.status(), type: response.request().resourceType() });
  });

  await page.goto(harness.url('/iam/orgs'));
  await expect(page.getByRole('region', { name: 'Your organizations' })).toBeVisible();
  const iamAssets = [...assets];

  await page.goto(harness.url('/gateway/overview'));
  await expect(page.getByTestId('zone-overview')).toBeVisible();
  const gatewayAssets = assets.slice(iamAssets.length);

  // Every static request during the IAM visit is under /iam/_next/, and every one during the
  // gateway visit is under /gateway/_next/ — the base path is the only thing keeping the two
  // zones' chunks apart under one origin.
  expect(iamAssets.filter((asset) => !asset.pathname.startsWith('/iam/_next/'))).toEqual([]);
  expect(gatewayAssets.filter((asset) => !asset.pathname.startsWith('/gateway/_next/'))).toEqual([]);
  // A 404 here is the collision a shared _next root would produce: a chunk requested at the
  // wrong prefix, not a chunk that loaded slowly or wrong.
  expect(assets.filter((asset) => asset.status !== 200)).toEqual([]);
  // At least one stylesheet and one script in each zone, so this row cannot pass by observing
  // nothing — a page with zero recorded assets would otherwise satisfy every check above.
  expect(iamAssets.filter((asset) => asset.type === 'stylesheet').length).toBeGreaterThan(0);
  expect(iamAssets.filter((asset) => asset.type === 'script').length).toBeGreaterThan(0);
  expect(gatewayAssets.filter((asset) => asset.type === 'stylesheet').length).toBeGreaterThan(0);
  expect(gatewayAssets.filter((asset) => asset.type === 'script').length).toBeGreaterThan(0);
});
