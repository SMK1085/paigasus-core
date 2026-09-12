// SPDX-License-Identifier: Apache-2.0
//
// ADR-0017: the browser never receives a token. Every response the page receives in a full session
// is collected: HTML documents, RSC payloads (navigations and prefetches), static assets, and a
// Server Action result. None may contain the access or the refresh token the fake IdP issued.
//
// Three kinds of assertion close this. The leak scan itself; the vacuity guards, which prove that a
// non-empty BODY of each kind the row names was in fact read and scanned; and the residue
// assertion, which pins the set of bodies the row does NOT read to one measured class (a prefetch
// the route interceptor could not buffer, see `isPrefetchRsc`). A response whose body read failed
// counts for no guard and reds the residue assertion.
//
// THE SERVER ACTION BODY AND EVERY RSC BODY ARE BUFFERED BY THIS TEST, not read from Playwright
// afterwards. MEASURED under a full `moon ci` (SMA-511 Task 23, 3 of 3 runs): the action's answer
// arrives CHUNKED, with `bodySize: -1`, and `response.body()` then REJECTS. MEASURED again on CI
// (SMA-511 CI fix): the RSC GET of a client navigation fails the same way — the residue assertion
// reported `GET .../iam/orgs/<id>?_rsc=…` unread, on a run that passed locally. Both are the
// payloads ADR-0017's rule is most about, so losing them exactly under load is worse than a red.
// `page.route` fetches those responses, reads their text in this process and fulfils each request
// with the same bytes, so the scan never depends on Playwright retaining a streamed body.
//
// Buffering, not skipping, is the fix. Widening the skip would have made the scan quietly smaller
// on exactly the response class the rule is about; buffering makes it strictly larger, and the
// prefetch bodies that Chromium never handed over are now scanned too.
import type { Request, Response } from '@playwright/test';
import { ORG_ID, ORG_NAME, PROJECT_ID, TEAM_ID } from './support/world';
import { signIn, waitForHydration } from './support/login';
import { expect, test } from './support/harness';

type Seen = {
  readonly url: string;
  readonly method: string;
  readonly contentType: string;
  readonly action: boolean;
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
 * the request: `Next-Router-Prefetch: 1` together with the `_rsc=` query the router adds.
 *
 * MEASURED on Next 16.3.4: Chromium never hands a PREFETCH RSC body to Playwright, because the
 * renderer keeps the stream for its prefetch cache. `response.body()` for such a response never
 * settles, so reading one turns this row into a 60 s timeout as soon as another spec runs before
 * it in the same worker (measured: `Promise.all` waited 59 s on three of them).
 *
 * Since the CI fix this is a SAFETY NET, not the ordinary path: `isRscGet` sends every same-origin
 * `_rsc` GET — prefetch and real navigation alike — through the route interceptor, so its body is
 * buffered and scanned. This clause still covers a prefetch the interceptor could not buffer (a
 * request the router cancelled, say), which would otherwise red the residue assertion for a reason
 * that is not a leak.
 *
 * The match is deliberately NOT a timeout. A blanket bound would drop any slow-but-real body out
 * of the scan without a signal; the residue assertion at the end of the test pins the unread set
 * to exactly this class, so a NEW unread response reds the row.
 */
async function isPrefetchRsc(response: Response): Promise<boolean> {
  const request = response.request();
  const headers = await request.allHeaders();
  return headers['next-router-prefetch'] === '1' && new URL(request.url()).searchParams.has('_rsc');
}

/** A Server Action call: a POST that carries Next's own action header. */
function isServerAction(request: Request): boolean {
  return request.method() === 'POST' && request.headers()['next-action'] !== undefined;
}

/**
 * An RSC payload fetch: the GET the App Router makes for a client navigation or a prefetch. The
 * `_rsc=` query is the router's own mark and is the whole test — a prefetch and a navigation are
 * the same request shape here, and both are buffered.
 */
function isRscGet(request: Request): boolean {
  return request.method() === 'GET' && new URL(request.url()).searchParams.has('_rsc');
}

async function capture(response: Response, buffered: ReadonlyMap<Request, string>): Promise<Seen> {
  const request = response.request();
  const headers = await response.allHeaders();
  const requestHeaders = await request.allHeaders();
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
    action: requestHeaders['next-action'] !== undefined,
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

test('R11: no response body, header, RSC payload or action result contains a fake token (ADR-0017)', async ({ page, harness }) => {
  // The buffered bodies, keyed by the Request the response carries — the SAME object the route
  // handler saw, so no url or timing match is needed.
  const buffered = new Map<Request, string>();
  // Only the streaming classes are fetched and refilled: the Server Action POST and every
  // same-origin RSC GET. Documents and static assets fall through untouched, so the interception
  // cannot change how the page loads. The URL predicate is the coarse filter; the two predicates
  // in the handler are the real test, and anything else takes the ordinary path.
  await page.route(
    (url) => url.origin === harness.origin && (url.pathname === '/iam/orgs' || url.searchParams.has('_rsc')),
    async (route) => {
      const request = route.request();
      if (!isServerAction(request) && !isRscGet(request)) {
        await route.fallback();
        return;
      }
      try {
        // maxRedirects: 0 — the browser must see a redirect as a redirect. Playwright's default
        // follows it HERE, which would hide the hop from `page.on('response')` and from the auth
        // flow this app depends on.
        const answer = await route.fetch({ maxRedirects: 0 });
        const text = await answer.text();
        buffered.set(request, text);
        // `body` is the DECODED text, so the stored content-encoding and content-length headers of
        // `answer` no longer describe it. They are dropped, and the rest is passed through.
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

  // The RESIDUE: every response that should carry a body, and whose body this row did not read,
  // must belong to the one measured class. A positive match, so a NEW unread response — a body
  // read that failed, or another class Next starts to stream — reds this row instead of leaving
  // the scan quietly smaller.
  expect(seen.filter((response) => response.expectsBody && !response.bodyRead && !response.prefetchRsc).map((response) => `${response.method} ${response.url}`)).toEqual([]);
  // The census of the scan, so a SHRINKING scan is visible in the log and not only in a red. Moon
  // discards a passing task's output, so this costs nothing on a green run and is there on a red.
  console.log(
    `R11 scan: ${String(seen.length)} responses, ${String(seen.filter(scanned).length)} scanned, ${String(seen.filter((response) => response.buffered).length)} buffered, ${String(seen.filter((response) => response.expectsBody && !response.bodyRead).length)} unread, ${String(seen.filter((response) => response.prefetchRsc).length)} prefetch RSC (unread before the buffering fix)`,
  );

  // Vacuity guards: for each kind of response the row names, at least one non-empty body was READ
  // and scanned. A response is not enough: if every body read of a kind failed, the scan above
  // searched only headers for that kind, and the guard fails.
  expect(seen.some((response) => response.contentType.startsWith('text/html') && scanned(response))).toBe(true);
  expect(seen.some((response) => response.contentType.startsWith('text/x-component') && !response.action && scanned(response))).toBe(true);
  expect(seen.some((response) => response.method === 'POST' && response.action && scanned(response))).toBe(true);

  // The last two guards keep the load regressions from returning in silence: a Server Action body
  // AND an RSC GET body were each scanned FROM THE ROUTE BUFFER. The guards above are satisfied by
  // either path, so on their own they would go green again the moment `response.body()` happens to
  // work — which is exactly what made the CI-only failure invisible locally.
  expect(seen.some((response) => response.action && response.method === 'POST' && response.buffered && scanned(response))).toBe(true);
  expect(seen.some((response) => response.method === 'GET' && !response.action && response.contentType.startsWith('text/x-component') && response.buffered && scanned(response))).toBe(true);
});
