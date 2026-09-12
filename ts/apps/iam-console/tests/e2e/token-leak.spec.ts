// SPDX-License-Identifier: Apache-2.0
//
// ADR-0017: the browser never receives a token. Every response the page receives in a full session
// is collected: HTML documents, RSC payloads (navigations and prefetches), static assets, and a
// Server Action result. None may contain the access or the refresh token the fake IdP issued. The
// vacuity guards at the end prove that a non-empty BODY of each kind was in fact read and scanned:
// a response whose body read failed counts for no guard.
import type { Response } from '@playwright/test';
import { ORG_ID, ORG_NAME, PROJECT_ID, TEAM_ID } from './support/world';
import { signIn, waitForHydration } from './support/login';
import { expect, test } from './support/harness';

type Seen = {
  readonly url: string;
  readonly method: string;
  readonly contentType: string;
  readonly action: boolean;
  /** true only after the body was read. A redirect has no body, so it stays false. */
  readonly bodyRead: boolean;
  readonly body: string;
  /** The headers and the body: what the leak scan searches. */
  readonly text: string;
};

/**
 * The bound on ONE body read. MEASURED on Next 16.3.4: a PREFETCH RSC response (its URL carries
 * `?_rsc=`) answers 200 `text/x-component`, and Chromium never hands its body to Playwright — the
 * renderer keeps the stream for its prefetch cache. `response.body()` for such a response waits
 * for the whole test timeout, so an unbounded read turns this row into a 60 s timeout as soon as
 * another spec runs before it in the same worker (measured: three prefetch responses,
 * `/iam/audit?_rsc=…`, `/iam/orgs?_rsc=…` and `/iam/orgs/<org>?_rsc=…`).
 *
 * WHAT THE BOUND COSTS, stated plainly: the body of a prefetch response is NOT scanned. Only its
 * headers are. The row still scans every body the browser delivers — every HTML document, the RSC
 * payload of the real client navigation, and the Server Action result — and the vacuity guards at
 * the end prove one of each kind was read.
 */
const BODY_READ_TIMEOUT_MS = 3_000;

/** The body as text, or null when the read failed or did not finish inside the bound. */
async function readBody(response: Response): Promise<string | null> {
  // The rejection is handled HERE, not by the race: a rejection after the race resolved would
  // otherwise be an unhandled rejection.
  const body = response.body().then(
    (buffer) => buffer.toString('utf8'),
    () => null,
  );
  const bound = new Promise<null>((resolve) => setTimeout(resolve, BODY_READ_TIMEOUT_MS, null));
  return Promise.race([body, bound]);
}

async function capture(response: Response): Promise<Seen> {
  const request = response.request();
  const headers = await response.allHeaders();
  const requestHeaders = await request.allHeaders();
  let body = '';
  let bodyRead = false;
  const status = response.status();
  if (status < 300 || status >= 400) {
    const text = await readBody(response);
    if (text !== null) {
      body = text;
      bodyRead = true;
    }
  }
  return {
    url: response.url(),
    method: request.method(),
    contentType: headers['content-type'] ?? '',
    action: requestHeaders['next-action'] !== undefined,
    bodyRead,
    body,
    text: `${JSON.stringify(headers)}\n${body}`,
  };
}

/** true when the body was read and is not empty, so the leak scan searched it. */
function scanned(response: Seen): boolean {
  return response.bodyRead && response.body.length > 0;
}

test('R11: no response body, header, RSC payload or action result contains a fake token (ADR-0017)', async ({ page, harness }) => {
  const pending: Promise<Seen>[] = [];
  page.on('response', (response) => {
    pending.push(capture(response));
  });
  const issuedBefore = harness.idp.issued.length;

  await signIn(page, harness);
  for (const path of [`/iam/orgs/${ORG_ID}`, `/iam/orgs/${ORG_ID}/teams/${TEAM_ID}`, `/iam/orgs/${ORG_ID}/teams/${TEAM_ID}/projects/${PROJECT_ID}`, '/iam/audit', '/iam/orgs']) {
    await page.goto(harness.url(path));
  }
  // A client-side navigation, so the RSC payload path is exercised, not only full documents. It
  // needs a hydrated page: before hydration the click is a plain document request. Wait for the
  // navigation's RSC response and for the new URL. waitForLoadState('networkidle') would resolve at
  // once, because the document reached that state at its first load, and the page.goto below could
  // then abort the RSC fetch.
  await waitForHydration(page);
  const rsc = page.waitForResponse((response) => (response.headers()['content-type'] ?? '').startsWith('text/x-component') && response.request().method() === 'GET');
  await page.getByRole('region', { name: 'Your organizations' }).getByRole('link', { name: ORG_NAME }).click();
  await rsc;
  await page.waitForURL((url) => url.pathname.startsWith('/iam/orgs/'));
  await page.goto(harness.url('/iam/orgs'));
  await waitForHydration(page);
  const form = page.getByRole('form', { name: 'Create organization' });
  await form.getByLabel('Slug').fill('leak-check');
  await form.getByLabel('Name').fill('Leak Check');
  await form.getByRole('button', { name: 'Create' }).click();
  await expect(form.getByRole('status')).toHaveText('Created.');
  await page.waitForLoadState('networkidle');

  const seen = await Promise.all(pending);
  const tokens = harness.idp.issued.slice(issuedBefore).flatMap((issued) => [issued.accessToken, issued.refreshToken]);
  expect(tokens.length).toBeGreaterThanOrEqual(2);
  for (const token of tokens) expect(token.length).toBeGreaterThanOrEqual(16);

  const leaks = seen.filter((response) => tokens.some((token) => response.text.includes(token))).map((response) => `${response.method} ${response.url}`);
  expect(leaks).toEqual([]);

  // Vacuity guards: for each kind of response the row names, at least one non-empty body was READ
  // and scanned. A response is not enough: if every body read of a kind failed, the scan above
  // searched only headers for that kind, and the guard fails.
  expect(seen.some((response) => response.contentType.startsWith('text/html') && scanned(response))).toBe(true);
  expect(seen.some((response) => response.contentType.startsWith('text/x-component') && !response.action && scanned(response))).toBe(true);
  expect(seen.some((response) => response.method === 'POST' && response.action && scanned(response))).toBe(true);
});
