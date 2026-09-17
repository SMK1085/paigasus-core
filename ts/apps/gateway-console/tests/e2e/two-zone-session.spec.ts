// SPDX-License-Identifier: Apache-2.0
//
// AC 1 (spec § 10.5): a session minted at one zone must carry into the other with no second
// authorization, and a cross-zone link must be a hard navigation while a same-zone one stays
// soft (ADR-0017). Both rows run against the two-zone stack (support/two-zone-harness.ts), which
// is the only place in this repository two real Next apps share one origin and one session store.
import type { Page } from '@playwright/test';
import { waitForHydration } from './support/login';
import { ORG_ID } from './support/world';
import { expect, test } from './support/two-zone-harness';

type RecordedRequest = { readonly pathname: string; readonly resourceType: string };

/** Requests of type `document` recorded after index `start` (the technique
 * ts/packages/paigasus-app-shell/tests/e2e/zones.spec.ts:45-82 established). */
function documentsAfter(log: readonly RecordedRequest[], start: number): RecordedRequest[] {
  return log.slice(start).filter((entry) => entry.resourceType === 'document');
}

/** Put a marker on `window`. A document navigation clears it; a soft navigation keeps it. */
async function setMarker(page: Page): Promise<void> {
  await page.evaluate(() => {
    (window as unknown as { __pgsMarker?: string }).__pgsMarker = 'first-load';
  });
}

async function readMarker(page: Page): Promise<string | null> {
  return page.evaluate(() => (window as unknown as { __pgsMarker?: string }).__pgsMarker ?? null);
}

test('R8: a session from the IAM zone carries into the gateway zone with no second authorization (AC 1)', async ({ page, harness }) => {
  await page.goto(harness.url('/iam/orgs'));
  expect(new URL(page.url()).pathname).toBe('/iam/orgs');
  await expect(page.getByRole('region', { name: 'Your organizations' })).toBeVisible();

  // Vacuity guard for R10 (two-zone-isolation.spec.ts), placed HERE rather than there (spec §
  // 10.5, task 4 Step 3): this sign-in is real traffic through /iam, so the forwarder must have
  // counted something by now. Without this assertion somewhere, a forwarder that was never wired
  // into the terminator at all would also report zero connections, and R10 would pass whether or
  // not the design has the isolation property it claims to prove.
  expect(harness.iamConsole.connections()).toBeGreaterThan(0);

  // Snapshot, not an absolute count: `authorizeRedirectUris` keeps growing across the whole
  // worker (it is never cleared between tests), so it is the sum of every login that ran before
  // this one in the same worker. "Exactly one" against that sum is flaky, and "at least one"
  // asserts nothing — only the DELTA over this one navigation says what this row claims.
  const authorizeBefore = harness.idp.authorizeRedirectUris.length;

  await page.goto(harness.url('/gateway/overview'));

  await expect(page.getByTestId('zone-overview')).toBeVisible();
  // The whole point of this row: reaching the gateway zone's protected page, with a session
  // minted at the IAM zone, sent the browser to the IdP's /authorize endpoint exactly zero more
  // times. A second authorization here would mean the two zones are not sharing one session.
  expect(harness.idp.authorizeRedirectUris.length - authorizeBefore).toBe(0);
});

test('R9: the cross-zone link is a hard navigation and the same-zone one is not (ADR-0017)', async ({ page, harness }) => {
  const log: RecordedRequest[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    log.push({ pathname: url.pathname, resourceType: request.resourceType() });
  });

  // A cold sign-in at the overview, then a same-zone navigation to the scope route — the
  // established pattern (tests/e2e/token-leak.spec.ts:144-145) — so both halves of this test run
  // from a page that carries the primary nav's cross-zone "IAM" entry and the scope route's own
  // breadcrumb "Overview" entry back to it.
  await page.goto(harness.url('/gateway/overview'));
  expect(new URL(page.url()).pathname).toBe('/gateway/overview');
  await waitForHydration(page);

  await page.goto(harness.url(`/gateway/orgs/${ORG_ID}`));
  await waitForHydration(page);
  await expect(page.getByTestId('zone-overview')).toBeVisible();
  await setMarker(page);

  // SAME ZONE: the scope route's breadcrumb "Overview" link targets `/gateway/overview`, in the
  // same zone, so ZoneLink renders it as a Next <Link> (a soft, client-side navigation).
  const sameZoneStart = log.length;
  await page.getByRole('navigation', { name: 'Breadcrumb' }).getByRole('link', { name: 'Overview', exact: true }).click();
  await page.waitForURL((url) => url.pathname === '/gateway/overview');
  // No `document` request at all: a soft navigation never asks the network for a new document.
  expect(documentsAfter(log, sameZoneStart)).toEqual([]);
  // The window was never replaced, so the marker this test set is still there.
  expect(await readMarker(page)).toBe('first-load');

  // CROSS ZONE: the primary nav's "IAM" entry targets `/iam/orgs`, a different zone, so ZoneLink
  // renders it as a plain <a> (a hard navigation — spec § 6.2, ADR-0017).
  const crossZoneStart = log.length;
  await page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'IAM', exact: true }).click();
  await page.waitForURL(`${harness.origin}/iam/orgs`);
  // Exactly one `document` request, for the target path: a real, full page load.
  expect(documentsAfter(log, crossZoneStart).map((entry) => entry.pathname)).toEqual(['/iam/orgs']);
  // A hard navigation replaces the window, so the marker is gone — not because anything cleared
  // it, but because it is a different `window` than the one that had it set.
  expect(await readMarker(page)).toBeNull();
});
