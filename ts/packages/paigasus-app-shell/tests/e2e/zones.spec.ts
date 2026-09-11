// SPDX-License-Identifier: Apache-2.0
//
// AC 1 and AC 2 in a real Chromium against a PRODUCTION build of the fixture (spec § 10.3).
// next/link prefetches differently under `next dev`, and the ADR-0017 bug is a production prefetch.
import { expect, test, type Page } from '@playwright/test';
import { documentsAfter, loadHydrated, origin, readMarker, record, rscPathnames, setMarker, type RecordedRequest } from './support/recorder.js';

/** The pathnames the main page MAY send an RSC request to: its own same-zone targets. */
const MAIN_ALLOWLIST: readonly string[] = ['/iam/gateway/raw', '/iam/settings', '/iam/users'];
/** The three controls, which come after the links under test in DOM order. */
const CONTROLS: readonly string[] = ['/iam/gateway/raw', '/iam/settings', '/iam/users'];
const LINKS_UNDER_TEST: readonly string[] = ['Gateway usage', 'Gateway via ui Link'];

async function exerciseLinksUnderTest(page: Page): Promise<void> {
  for (const name of LINKS_UNDER_TEST) {
    const link = page.getByRole('link', { name, exact: true });
    await link.scrollIntoViewIfNeeded();
    await link.hover();
  }
}

/** Wait until Next processed the prefetch queue past the links under test (spec § 10.3, E2). */
async function waitForControls(log: readonly RecordedRequest[]): Promise<void> {
  await expect.poll(() => rscPathnames(log), { timeout: 15_000, message: 'the controls never sent their RSC prefetch' }).toEqual(expect.arrayContaining([...CONTROLS]));
}

/** The allowlist rule: ANY RSC request outside the page's allowlist fails (spec § 10.3). */
function expectAllowlistHolds(log: readonly RecordedRequest[]): void {
  const outside = rscPathnames(log).filter((pathname) => !MAIN_ALLOWLIST.includes(pathname));
  expect(outside, 'an RSC request left the page allowlist: some link sent a soft prefetch that it must not send').toEqual([]);
}

test('E1: a same-zone ZoneLink click is a soft navigation (AC 2)', async ({ page }) => {
  const log = await record(page);
  await loadHydrated(page, '/iam');
  await setMarker(page);
  const start = log.length;
  await page.getByRole('link', { name: 'Users', exact: true }).click();
  await page.waitForURL(`${origin()}/iam/users`);
  await expect(page.getByTestId('pathname')).toHaveText('/users');
  expect(documentsAfter(log, start)).toEqual([]);
  expect(await readMarker(page)).toBe('first-load');
});

test('E2 + E3: no RSC request leaves the allowlist, the negative control is seen, and a cross-zone click is a hard navigation (AC 1)', async ({ page }) => {
  const log = await record(page);
  await loadHydrated(page, '/iam');
  await setMarker(page);
  await exerciseLinksUnderTest(page);
  // waitForControls already polls until '/iam/gateway/raw' (one of CONTROLS) has an RSC request,
  // so this line already proves E3. The explicit E3 assertion below restates that on the SAME
  // page load, for a reader who is not tracing waitForControls back to its CONTROLS list.
  await waitForControls(log);

  // E2: the allowlist rule. It does not depend on where a wrong link sends its prefetch: a ZoneLink
  // that wrongly used next/link for /gateway/usage would prefetch /iam/gateway/usage (F3).
  expectAllowlistHolds(log);

  // E3: the negative control, on the SAME page load. The raw next/link to another zone produced
  // exactly the failure the allowlist exists to catch, so the recorder can see it.
  expect(rscPathnames(log), 'E3: the recorder did not see the raw next/link prefetch').toContain('/iam/gateway/raw');

  const start = log.length;
  await page.getByRole('link', { name: 'Gateway usage', exact: true }).click();
  await page.waitForURL(`${origin()}/gateway/usage`);
  expect(documentsAfter(log, start).map((entry) => entry.pathname)).toEqual(['/gateway/usage']);
  expect(await readMarker(page)).toBeNull();
});

test('E4 cross zone: a @paigasus/ui Link under LinkProvider link={ZoneLink} is a hard navigation', async ({ page }) => {
  const log = await record(page);
  await loadHydrated(page, '/iam');
  await setMarker(page);
  await exerciseLinksUnderTest(page);
  await waitForControls(log);
  expectAllowlistHolds(log);

  const start = log.length;
  await page.getByRole('link', { name: 'Gateway via ui Link', exact: true }).click();
  await page.waitForURL(`${origin()}/gateway/ui`);
  expect(documentsAfter(log, start).map((entry) => entry.pathname)).toEqual(['/gateway/ui']);
  expect(await readMarker(page)).toBeNull();
});

test('E4 same zone: the injection keeps a @paigasus/ui Link soft — this fails with two copies of @paigasus/ui', async ({ page }) => {
  const log = await record(page);
  await loadHydrated(page, '/iam');
  await setMarker(page);
  await expect.poll(() => rscPathnames(log), { timeout: 15_000, message: 'no RSC prefetch for /iam/settings: the injection did not take effect' }).toContain('/iam/settings');

  const start = log.length;
  await page.getByRole('link', { name: 'Settings via ui Link', exact: true }).click();
  await page.waitForURL(`${origin()}/iam/settings`);
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();
  expect(documentsAfter(log, start)).toEqual([]);
  expect(await readMarker(page)).toBe('first-load');
});

test('E7: usePathname() on /iam/users returns /users (F19)', async ({ page }) => {
  await loadHydrated(page, '/iam/users');
  await expect(page.getByTestId('pathname')).toHaveText('/users');
});
