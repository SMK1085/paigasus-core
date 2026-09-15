// SPDX-License-Identifier: Apache-2.0
//
// ADR-0017: the browser never receives a token. Every response the page receives in a signed-in
// session is collected: HTML documents and RSC payloads (navigations and prefetches) alike. None
// may contain the access or the refresh token the fake IdP issued.
//
// This zone has no Server Action (spec § 13: the scope route changes only the URL and the
// breadcrumbs), so this is the HTML/RSC half of iam-console's tests/e2e/token-leak.spec.ts, with no
// action guard. EVERY RSC BODY IS BUFFERED BY THIS TEST, not read from Playwright afterwards, for
// the same measured reason iam-console's copy documents: a client navigation's RSC GET can arrive
// with `bodySize: -1`, and `response.body()` then rejects or hangs under load. `page.route` fetches
// those responses, reads their text in this process, and fulfils each request with the same bytes,
// so the scan never depends on Playwright retaining a streamed body.
import type { Request, Response } from '@playwright/test';
import { ORG_ID } from './support/world';
import { signIn, waitForHydration } from './support/login';
import { expect, test } from './support/harness';

type Seen = {
  readonly url: string;
  readonly method: string;
  readonly contentType: string;
  /** false for a redirect, which carries no body. The residue assertion ignores those. */
  readonly expectsBody: boolean;
  /** true only after the body was read. */
  readonly bodyRead: boolean;
  /** true when the body came from the route interceptor, not from `response.body()`. */
  readonly buffered: boolean;
  /** The one class the row may leave unread, when the interceptor could not buffer it. */
  readonly prefetchRsc: boolean;
  readonly body: string;
  /** The headers and the body: what the leak scan searches. */
  readonly text: string;
};

/**
 * The LAST-RESORT class of response whose body this row may leave unread, matched POSITIVELY on
 * the request: `Next-Router-Prefetch: 1` together with the `_rsc=` query the router adds. See
 * iam-console's token-leak.spec.ts for the measurement this mirrors.
 */
async function isPrefetchRsc(response: Response): Promise<boolean> {
  const request = response.request();
  const headers = await request.allHeaders();
  return headers['next-router-prefetch'] === '1' && new URL(request.url()).searchParams.has('_rsc');
}

/** An RSC payload fetch: the GET the App Router makes for a client navigation or a prefetch. */
function isRscGet(request: Request): boolean {
  return request.method() === 'GET' && new URL(request.url()).searchParams.has('_rsc');
}

async function capture(response: Response, buffered: ReadonlyMap<Request, string>): Promise<Seen> {
  const request = response.request();
  const headers = await response.allHeaders();
  const status = response.status();
  const expectsBody = status < 300 || status >= 400;
  const prefetchRsc = await isPrefetchRsc(response);
  const fromRoute = buffered.get(request);
  let body = fromRoute ?? '';
  let bodyRead = fromRoute !== undefined;
  if (!bodyRead && expectsBody && !prefetchRsc) {
    try {
      body = (await response.body()).toString('utf8');
      bodyRead = true;
    } catch {
      // The read failed. bodyRead stays false, so no vacuity guard counts this response as
      // scanned AND the residue assertion reports it.
    }
  }
  return {
    url: response.url(),
    method: request.method(),
    contentType: headers['content-type'] ?? '',
    expectsBody,
    bodyRead,
    buffered: fromRoute !== undefined,
    prefetchRsc,
    body,
    text: `${JSON.stringify(headers)}\n${body}`,
  };
}

/** true when the body was read and is not empty, so the leak scan searched it. */
function scanned(response: Seen): boolean {
  return response.bodyRead && response.body.length > 0;
}

test('R5: no response body, header or RSC payload contains a fake token (ADR-0017)', async ({ page, harness }) => {
  // The buffered bodies, keyed by the Request the response carries — the SAME object the route
  // handler saw, so no url or timing match is needed.
  const buffered = new Map<Request, string>();
  // Only same-origin RSC GETs are intercepted. Documents and static assets fall through untouched,
  // so the interception cannot change how the page loads.
  await page.route(
    (url) => url.origin === harness.origin && url.searchParams.has('_rsc'),
    async (route) => {
      const request = route.request();
      if (!isRscGet(request)) {
        await route.fallback();
        return;
      }
      try {
        // maxRedirects: 0 — the browser must see a redirect as a redirect.
        const answer = await route.fetch({ maxRedirects: 0 });
        const text = await answer.text();
        buffered.set(request, text);
        const headers = { ...answer.headers() };
        delete headers['content-encoding'];
        delete headers['content-length'];
        await route.fulfill({ status: answer.status(), headers, body: text });
      } catch {
        // The router cancelled the request, or the page closed. Put it back on the ordinary path
        // rather than failing the handler: capture() then records it unread, and the residue
        // assertion reports it unless it is the measured prefetch class.
        await route.fallback().catch(() => undefined);
      }
    },
  );

  const pending: Promise<Seen>[] = [];
  page.on('response', (response) => {
    pending.push(capture(response, buffered));
  });
  const issuedBefore = harness.idp.issued.length;

  const { accessToken, refreshToken } = await signIn(page, harness, '/gateway/overview');
  await page.goto(harness.url(`/gateway/orgs/${ORG_ID}`));
  await waitForHydration(page);

  // A client-side navigation, so the RSC payload path is exercised, not only full documents.
  const rsc = page.waitForResponse((response) => (response.headers()['content-type'] ?? '').startsWith('text/x-component') && response.request().method() === 'GET');
  await page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'Overview', exact: true }).click();
  await rsc;
  await page.waitForURL((url) => url.pathname === '/gateway/overview');

  const seen = await Promise.all(pending);
  const tokens = [accessToken, refreshToken, ...harness.idp.issued.slice(issuedBefore).flatMap((issued) => [issued.accessToken, issued.refreshToken])];
  expect(tokens.length).toBeGreaterThanOrEqual(2);
  for (const token of tokens) expect(token.length).toBeGreaterThanOrEqual(16);

  const leaks = seen.filter((response) => tokens.some((token) => response.text.includes(token))).map((response) => `${response.method} ${response.url}`);
  expect(leaks).toEqual([]);

  // The RESIDUE: every response that should carry a body, and whose body this row did not read,
  // must belong to the one measured class. A positive match, so a NEW unread response reds this
  // row instead of leaving the scan quietly smaller.
  expect(seen.filter((response) => response.expectsBody && !response.bodyRead && !response.prefetchRsc).map((response) => `${response.method} ${response.url}`)).toEqual([]);
  // The census of the scan, so a SHRINKING scan is visible in the log and not only in a red.
  console.log(
    `R5 scan: ${String(seen.length)} responses, ${String(seen.filter(scanned).length)} scanned, ${String(seen.filter((response) => response.buffered).length)} buffered, ${String(seen.filter((response) => response.expectsBody && !response.bodyRead).length)} unread`,
  );

  // Vacuity guards: at least one non-empty HTML body and one non-empty RSC body were READ and
  // scanned. A response is not enough: if every body read of a kind failed, the scan above
  // searched only headers for that kind, and the guard fails.
  expect(seen.some((response) => response.contentType.startsWith('text/html') && scanned(response))).toBe(true);
  expect(seen.some((response) => response.contentType.startsWith('text/x-component') && scanned(response))).toBe(true);
  // Keeps the load regression from returning in silence: an RSC GET body was scanned FROM THE
  // ROUTE BUFFER, not merely because response.body() happened to work.
  expect(seen.some((response) => response.method === 'GET' && response.contentType.startsWith('text/x-component') && response.buffered && scanned(response))).toBe(true);
});
