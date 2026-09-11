// SPDX-License-Identifier: Apache-2.0
import { expect, test } from '@playwright/test';
import { documentsAfter, loadHydrated, origin, record } from './support/recorder.js';

// Absence has no event to wait for. This bounds how long a wrong navigation gets to appear.
const ABSENCE_WINDOW_MS = 1_000;

test('E5: the degraded entry navigates on neither a click nor Enter (AC 3)', async ({ page }) => {
  const log = await record(page);
  await loadHydrated(page, '/iam/shell');
  const entry = page.getByRole('link', { name: 'Gateway', exact: true });
  await expect(entry).toHaveAttribute('aria-disabled', 'true');
  await expect(page.getByText('gateway is not answering (timed out)')).toBeVisible();

  const start = log.length;
  // { force: true }: aria-disabled="true" carries no CSS pointer-events block, so a real mouse
  // click DOES land on this element (measured: Playwright's default actionability check refuses
  // to click an aria-disabled target and the call times out instead of proving the assertion).
  // force: true bypasses that check and clicks through, which is the faithful simulation here.
  await entry.click({ force: true });
  await entry.focus();
  await expect(entry).toBeFocused();
  await page.keyboard.press('Enter');
  await page.waitForTimeout(ABSENCE_WINDOW_MS);
  expect(page.url()).toBe(`${origin()}/iam/shell`);
  expect(documentsAfter(log, start)).toEqual([]);
});

for (const path of ['mouse', 'keyboard'] as const) {
  test(`E6 (${path}): Sign out sends exactly one POST document request to /iam/auth/logout`, async ({ page }) => {
    const log = await record(page);
    await loadHydrated(page, '/iam/shell');
    const trigger = page.getByRole('button', { name: 'Ada Lovelace' });
    const start = log.length;
    const logout = page.waitForRequest((request) => request.method() === 'POST' && new URL(request.url()).pathname === '/iam/auth/logout', { timeout: 10_000 });

    if (path === 'mouse') {
      // The fixture has NO exit animation (no Tailwind build), so this path fails if the form
      // unmounts with the menu (F18, spec § 8.5).
      await trigger.click();
      await page.getByRole('menuitem', { name: 'Sign out' }).click();
    } else {
      await trigger.focus();
      await page.keyboard.press('Enter');
      await expect(page.getByRole('menuitem', { name: 'Sign out' })).toBeFocused();
      await page.keyboard.press('Enter');
    }

    await logout;
    await page.waitForURL(`${origin()}/iam/auth/logout`);
    const posts = documentsAfter(log, start).filter((entry) => entry.method === 'POST' && entry.pathname === '/iam/auth/logout');
    expect(posts).toHaveLength(1);
  });
}
