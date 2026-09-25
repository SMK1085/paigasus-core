// SPDX-License-Identifier: Apache-2.0
//
// The in-phase control for R3 (spec § 6.3): with the gateway zone ENABLED and its backend down
// (ci/kind/manifests/gateway-stub.yaml at 0 replicas, before `run.sh stub up`), the IAM nav shows
// the Gateway entry as degraded — a span with role="link" and aria-disabled (primary-nav.tsx:98).
// R3 uses the same locator in phase B.
import { expect, test } from '@playwright/test';
import { loginAt } from '../support/login';

test('R3-control: the IAM console shows the Gateway entry as degraded (AC 2)', async ({ page }) => {
  await loginAt(page, '/iam/orgs');
  const gateway = page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'Gateway', exact: true });
  await expect(gateway).toHaveCount(1);
  await expect(gateway).toHaveAttribute('aria-disabled', 'true');
});
