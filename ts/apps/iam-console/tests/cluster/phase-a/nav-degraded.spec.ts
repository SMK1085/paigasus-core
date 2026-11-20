// SPDX-License-Identifier: Apache-2.0
//
// The in-phase control for R3 (spec § 6.3): with the gateway zone ENABLED and its backend down
// (ci/kind/manifests/gateway-stub.yaml at 0 replicas, before `run.sh stub up`), the IAM console shows
// the AI Gateway entry as degraded. Since SMA-783 that entry is an item of the console switcher on
// the brand, not of the Primary nav: a disabled menu item (aria-disabled) whose accessible name is
// the label followed by the reason (console-switcher.tsx), so the name is matched as a prefix. R3
// checks in phase B that the switcher is gone.
import { expect, test } from '@playwright/test';
import { loginAt } from '../support/login';

test('R3-control: the IAM console shows the Gateway entry as degraded (AC 2)', async ({ page }) => {
  await loginAt(page, '/iam/orgs');
  await page.getByRole('banner').getByRole('button', { name: 'Switch console: Paigasus IAM' }).click();
  const gateway = page.getByRole('menuitem', { name: /^AI Gateway/ });
  await expect(gateway).toHaveCount(1);
  await expect(gateway).toHaveAttribute('aria-disabled', 'true');
});
