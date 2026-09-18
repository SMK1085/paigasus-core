// SPDX-License-Identifier: Apache-2.0
//
// SMA-636 spec § 7.2 row 4 and § 6 item 1: the plaintext API key token is the only secret of the
// settings screens. It must be in EXACTLY ONE response body — the issue action's response — and in
// no later HTML, RSC or prefetch response, and in no URL. This row depends on every token rule of
// § 5.4: a useActionState would send the token back in the next request body, a document POST
// would put it in an HTML response, and a revalidated render that read it would put it in an RSC.
//
// F15/F5: this reuses ./support/response-scan.ts (the buffering and the bounded quiesce wait
// token-leak.spec.ts's R5 already shares), rather than a second, `networkidle`-based recorder —
// `finish()` polls the pending count instead of waiting for the network to go idle, so a page that
// never goes quiet fails this row instead of hanging it.
import { signIn, waitForHydration } from './support/login';
import { scanned, startResponseScan, QUIESCE_TIMEOUT_MS } from './support/response-scan';
import { ORG_ID, SEEDED_SA_ID, TOKEN_PREFIX } from './support/world';
import { expect, test } from './support/harness';

const ORG_PATH = `/gateway/orgs/${ORG_ID}`;

test('R16: an issued token is in exactly one response body, the action response, and in none after it (§ 7.2 row 4)', async ({ page, harness }) => {
  harness.useWorld({ seedServiceAccount: true });
  await signIn(page, harness, ORG_PATH);
  await page.goto(harness.url(`${ORG_PATH}?sa=${SEEDED_SA_ID}`));
  await waitForHydration(page);

  // Only same-origin requests under /gateway/orgs/ (the action) and every same-origin RSC GET are
  // intercepted. Documents and static assets fall through untouched, so the interception cannot
  // change how the page loads.
  const scan = await startResponseScan(page, (url) => url.origin === harness.origin && (url.pathname.startsWith('/gateway/orgs/') || url.searchParams.has('_rsc')));

  await page.getByTestId('sa-panel').getByRole('button', { name: 'Issue key' }).click();
  const shown = page.getByTestId('token-value');
  await expect(shown).toHaveText(new RegExp(`^${TOKEN_PREFIX}[0-9a-f]{32}$`));
  const token = (await shown.textContent()) ?? '';
  expect(token.length).toBe(TOKEN_PREFIX.length + 32);

  // Every later response class: a document reload, a client navigation (RSC), and a second document.
  await page.reload();
  await waitForHydration(page);
  await expect(page.getByTestId('token-panel')).toHaveCount(0);
  const rsc = page.waitForResponse((response) => (response.headers()['content-type'] ?? '').startsWith('text/x-component') && response.request().method() === 'GET');
  await page.getByRole('navigation', { name: 'Breadcrumb' }).getByRole('link', { name: 'Overview', exact: true }).click();
  await rsc;
  await page.waitForURL((url) => url.pathname === '/gateway/overview');
  await expect(page.getByTestId('zone-overview')).toBeVisible();
  await page.goto(harness.url(ORG_PATH));
  await expect(page.getByTestId('org-settings')).toBeVisible();

  const { seen, quiesced } = await scan.finish();
  expect(quiesced, `the response stream did not go quiet within ${String(QUIESCE_TIMEOUT_MS)} ms, so this scan may be missing responses and cannot be trusted`).toBe(true);

  const carrying = seen.filter((response) => response.body.includes(token));
  expect(carrying.map((response) => `${response.method} ${response.url}`)).toHaveLength(1);
  expect(carrying[0]?.method).toBe('POST');
  expect(carrying[0]?.action).toBe(true);
  expect(seen.filter((response) => response.url.includes(token))).toEqual([]);

  // The residue: every response that should carry a body, and whose body this row did not read,
  // must belong to the one measured class the README's Known limits records — an unbuffered
  // prefetch (F16) or a redirect Playwright's Response.body() contract refuses.
  expect(seen.filter((response) => response.expectsBody && !response.bodyRead && !response.prefetchRsc && !response.redirect).map((response) => `${response.method} ${response.url}`)).toEqual([]);

  // Vacuity guards: the action body, an HTML body and a non-action RSC body were READ and searched.
  expect(seen.some((response) => response.action && scanned(response))).toBe(true);
  expect(seen.some((response) => response.contentType.startsWith('text/html') && scanned(response))).toBe(true);
  expect(seen.some((response) => response.contentType.startsWith('text/x-component') && !response.action && scanned(response))).toBe(true);
});
