// SPDX-License-Identifier: Apache-2.0
//
// R7 — an addition beyond spec § 10.4 (Task 5 Step 8). Task 3 built (console)/forbidden.tsx and
// nothing else proves it renders with a REAL HTTP 403 in this zone; next.config.ts's
// experimental.authInterrupts is EXPERIMENTAL, so without this row a Next upgrade that changes the
// flag would silently render the view with status 200 and nothing here would catch it.
import { Code } from '@connectrpc/connect';
import { denial } from '@paigasus/console-core/testing';
import { forbiddenViewCorrelation } from './support/correlation';
import { signIn } from './support/login';
import { ORG_ID } from './support/world';
import { expect, test } from './support/harness';

test('R7: a denied GetOrganization is a real HTTP 403, with the forbidden view inside the console shell', async ({ page, harness }) => {
  await signIn(page, harness, '/gateway/overview');
  harness.useWorld({
    overrides: {
      'tenancy.getOrganization': () => {
        throw denial({ code: Code.PermissionDenied, reason: 'forbidden' });
      },
    },
  });
  const before = harness.iam.callsTo('tenancy.getOrganization').length;

  const response = await page.goto(harness.url(`/gateway/orgs/${ORG_ID}`));

  expect(response?.status()).toBe(403);
  await expect(page.getByTestId('forbidden-view')).toBeVisible();
  // The boundary is per-segment: it renders INSIDE (console)/layout.tsx, so the shell survives.
  await expect(page.getByRole('navigation', { name: 'Primary' })).toBeVisible();

  const denied = harness.iam.callsTo('tenancy.getOrganization').slice(before);
  expect(denied.length).toBeGreaterThan(0);
  // The paigasus-correlation-id header as it ARRIVED at the fake: the id proxy.ts minted.
  const correlationId = denied.at(-1)?.correlationId ?? '';
  expect(correlationId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);

  if (forbiddenViewCorrelation() === 'header') {
    // The view shows exactly the id IAM adopted for the denied call.
    await expect(page.getByTestId('correlation-id')).toHaveText(correlationId);
  } else {
    // The fallback: no id in the view.
    await expect(page.getByTestId('correlation-id')).toHaveCount(0);
  }
});
