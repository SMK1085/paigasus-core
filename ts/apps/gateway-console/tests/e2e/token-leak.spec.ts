// SPDX-License-Identifier: Apache-2.0
//
// ADR-0017: the browser never receives a token. Every response the page receives in a signed-in
// session is collected: HTML documents, RSC payloads (navigations and prefetches) alike, and — since
// SMA-636 gave this zone its first Server Actions — a Server Action result. None may contain the
// access or the refresh token the fake IdP issued. SMA-636 § 7.4 extends this row the way
// iam-console's copy already is: it runs one action (create a service account) and requires the
// action's body to be buffered and scanned.
//
// The buffering, the quiesce wait and the Seen shape live in ./support/response-scan.ts, shared with
// Task 19's R16 (controller ruling F5) rather than copied.
import { ORG_ID } from './support/world';
import { signIn, waitForHydration } from './support/login';
import { scanned, startResponseScan, QUIESCE_TIMEOUT_MS } from './support/response-scan';
import { expect, test } from './support/harness';

test('R5: no response body, header, RSC payload or action result contains a fake token (ADR-0017)', async ({ page, harness }) => {
  // Only same-origin requests under /gateway/orgs/ (the action) and every same-origin RSC GET are
  // intercepted. Documents and static assets fall through untouched, so the interception cannot
  // change how the page loads.
  const scan = await startResponseScan(page, (url) => url.origin === harness.origin && (url.pathname.startsWith('/gateway/orgs/') || url.searchParams.has('_rsc')));
  const issuedBefore = harness.idp.issued.length;

  const { accessToken, refreshToken } = await signIn(page, harness, '/gateway/overview');
  await page.goto(harness.url(`/gateway/orgs/${ORG_ID}`));
  await waitForHydration(page);

  // One Server Action (SMA-636 § 7.4): create a service account through the form.
  const create = page.getByTestId('service-accounts').getByRole('form', { name: 'Create service account' });
  await create.getByLabel('Name').fill('leak-check');
  await create.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByTestId('sa-result').getByRole('status')).toContainText('Service account created.');

  // A client-side navigation, so the RSC payload path is exercised, not only full documents.
  const rsc = page.waitForResponse((response) => (response.headers()['content-type'] ?? '').startsWith('text/x-component') && response.request().method() === 'GET');
  await page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'Overview', exact: true }).click();
  await rsc;
  await page.waitForURL((url) => url.pathname === '/gateway/overview');

  const { seen, quiesced } = await scan.finish();

  expect(quiesced, `the response stream did not go quiet within ${String(QUIESCE_TIMEOUT_MS)} ms, so this scan may be missing responses and cannot be trusted`).toBe(true);
  const tokens = [accessToken, refreshToken, ...harness.idp.issued.slice(issuedBefore).flatMap((issued) => [issued.accessToken, issued.refreshToken])];
  expect(tokens.length).toBeGreaterThanOrEqual(2);
  for (const token of tokens) expect(token.length).toBeGreaterThanOrEqual(16);

  const leaks = seen.filter((response) => tokens.some((token) => response.text.includes(token))).map((response) => `${response.method} ${response.url}`);
  expect(leaks).toEqual([]);

  // The RESIDUE: every response that should carry a body, and whose body this row did not read,
  // must belong to the one measured class.
  expect(seen.filter((response) => response.expectsBody && !response.bodyRead && !response.prefetchRsc && !response.redirect).map((response) => `${response.method} ${response.url}`)).toEqual([]);
  console.log(
    `R5 scan: ${String(seen.length)} responses, ${String(seen.filter(scanned).length)} scanned, ${String(seen.filter((response) => response.buffered).length)} buffered, ${String(seen.filter((response) => response.action).length)} action, ${String(seen.filter((response) => response.expectsBody && !response.bodyRead).length)} unread`,
  );

  // Vacuity guards: for each kind of response the row names, at least one non-empty body was READ
  // and scanned.
  expect(seen.some((response) => response.contentType.startsWith('text/html') && scanned(response))).toBe(true);
  expect(seen.some((response) => response.contentType.startsWith('text/x-component') && !response.action && scanned(response))).toBe(true);
  expect(seen.some((response) => response.method === 'POST' && response.action && scanned(response))).toBe(true);
  // The load regressions stay visible: the action body AND an RSC GET body were scanned FROM THE
  // ROUTE BUFFER, not merely because response.body() happened to work.
  expect(seen.some((response) => response.action && response.method === 'POST' && response.buffered && scanned(response))).toBe(true);
  expect(seen.some((response) => response.method === 'GET' && !response.action && response.contentType.startsWith('text/x-component') && response.buffered && scanned(response))).toBe(true);
});
