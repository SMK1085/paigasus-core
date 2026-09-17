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
  // ONE PAGE PER ZONE, in the SAME browser context. The context is what carries the shared
  // session cookie, so the two zones are still one signed-in user; the separate pages are what
  // make each zone's asset list its own.
  //
  // The obvious alternative — one page, one listener, split the array at the index the first
  // navigation ended on — has a race: an IAM prefetch or a deferred asset can land after that
  // snapshot and be counted as a gateway asset, failing the row for no real reason. The other
  // obvious alternative, partitioning by pathname prefix, is worse than the race: this row's
  // whole assertion is that each bucket carries its zone's prefix, so deriving the bucket FROM
  // that prefix makes both checks tautologies that pass even for a genuine collision.
  //
  // A listener per page settles it. The bucket comes from which page made the request — the
  // test's own structure — and the prefix stays an independent claim about it.
  const iamAssets: StaticAsset[] = [];
  const gatewayAssets: StaticAsset[] = [];
  const record = (into: StaticAsset[]) => (response: import('@playwright/test').Response) => {
    const url = new URL(response.url());
    if (url.pathname.includes('/_next/static/')) into.push({ pathname: url.pathname, status: response.status(), type: response.request().resourceType() });
  };

  page.on('response', record(iamAssets));
  await page.goto(harness.url('/iam/orgs'));
  await expect(page.getByRole('region', { name: 'Your organizations' })).toBeVisible();

  const gatewayPage = await page.context().newPage();
  gatewayPage.on('response', record(gatewayAssets));
  await gatewayPage.goto(harness.url('/gateway/overview'));
  await expect(gatewayPage.getByTestId('zone-overview')).toBeVisible();

  const assets = [...iamAssets, ...gatewayAssets];

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
