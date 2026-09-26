// SPDX-License-Identifier: Apache-2.0
//
// AC 1 and AC 3 through a real ingress (spec § 6.3 R2). The gateway zone's IAM link is a cross-zone
// <ZoneLink>, a plain <a> with no client router (spec F2), so the click must make a new DOCUMENT
// request; a same-zone link would not. "Your organizations" renders only when IAM accepted the
// access token. A hydrated page on both sides proves each zone's `_next` assets load under its own
// base path through Traefik: the one-origin half of AC 3 (docs/ops/RUNBOOK-containers.md, section 6,
// the bullet on the zone row of smoke_consoles).
import { expect, test } from '@playwright/test';
import { CONSOLE_HOST, loginAt, waitForHydration } from '../support/login';

test('R2: the gateway zone links to IAM with a hard navigation, and both zones hydrate (AC 1, AC 3)', async ({ page }) => {
  await loginAt(page, '/gateway/overview');

  const documents: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (request.resourceType() === 'document' && url.hostname === CONSOLE_HOST) documents.push(url.pathname);
  });
  await page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'IAM', exact: true }).click();
  await page.waitForURL((url) => url.pathname === '/iam/orgs');

  expect(documents, 'the IAM link must be a hard navigation (a new document request)').toContain('/iam/orgs');
  await expect(page.getByRole('heading', { level: 1, name: 'Your organizations' })).toBeVisible();
  await waitForHydration(page);
});
