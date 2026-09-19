// SPDX-License-Identifier: Apache-2.0
//
// The response-buffering and quiesce-wait machinery ADR-0017's token-leak rows share
// (tests/e2e/token-leak.spec.ts, § 7.4; Task 19's R16 reuses this too — controller ruling F5). It
// buffers every streaming response (a Server Action POST and a same-origin RSC GET) through
// `page.route`, so the scan never depends on Playwright retaining a streamed body: a streamed
// answer can arrive with `bodySize: -1`, and `response.body()` then rejects or hangs under load.
// `page.route` fetches those responses, reads their text in this process, and fulfils each request
// with the same bytes.
//
// F15: never `networkidle`. `finish()` waits for the response stream to go quiet with a bounded
// poll of the pending count instead, so a page that never goes quiet fails the row rather than
// hanging it.
import type { Page, Request, Response } from '@playwright/test';

/** The response stream counts as quiet after this long with no new response. */
export const QUIESCE_MS = 500;
/** A bound on the quiesce wait, so a page that never goes quiet fails the row rather than hanging it. */
export const QUIESCE_TIMEOUT_MS = 10_000;

export type Seen = {
  readonly url: string;
  readonly method: string;
  readonly contentType: string;
  /** A POST with Next's `next-action` header: a Server Action call. */
  readonly action: boolean;
  /**
   * Whether HTTP permits this response to carry a body at all. Only HEAD, 204, 205 and 304 are
   * truly bodyless. A 3xx MAY carry one, so it is no longer classified away — see `redirect`.
   */
  readonly expectsBody: boolean;
  /** A 3xx whose body Playwright's Response.body() contract refuses. Exempt POSITIVELY, like prefetchRsc. */
  readonly redirect: boolean;
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

export type ResponseScan = {
  /**
   * Detach the `response` listener, wait (bounded, per F15) for the stream to go quiet, and
   * return every response seen. `quiesced` is false when the bound expired first, in which case
   * the scan may be missing late responses and the caller must not trust it.
   */
  finish(): Promise<{ readonly seen: readonly Seen[]; readonly quiesced: boolean }>;
};

/**
 * The LAST-RESORT class of response whose body a row may leave unread, matched POSITIVELY on the
 * request: `Next-Router-Prefetch: 1` together with the `_rsc=` query the router adds. See
 * iam-console's token-leak.spec.ts for the measurement this mirrors.
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

/** An RSC payload fetch: the GET the App Router makes for a client navigation or a prefetch. */
function isRscGet(request: Request): boolean {
  return request.method() === 'GET' && new URL(request.url()).searchParams.has('_rsc');
}

async function capture(response: Response, buffered: ReadonlyMap<Request, string>): Promise<Seen> {
  const request = response.request();
  const headers = await response.allHeaders();
  const requestHeaders = await request.allHeaders();
  const status = response.status();
  // HTTP-correct, not status-class shorthand. Only these four cases genuinely carry no body.
  const expectsBody = request.method() !== 'HEAD' && status !== 204 && status !== 205 && status !== 304;
  const redirect = status >= 300 && status < 400;
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
    redirect,
    bodyRead,
    buffered: fromRoute !== undefined,
    prefetchRsc,
    body,
    text: `${JSON.stringify(headers)}\n${body}`,
  };
}

/** true when the body was read and is not empty, so the leak scan searched it. */
export function scanned(response: Seen): boolean {
  return response.bodyRead && response.body.length > 0;
}

/**
 * Start buffering and recording. `matches` scopes what `page.route` inspects, beyond origin — for
 * example an action pathname prefix, together with the `_rsc` query every RSC GET carries. Only a
 * Server Action POST or an RSC GET is actually intercepted; anything else `matches` catches falls
 * through untouched, so the interception cannot change how the page loads.
 */
export async function startResponseScan(page: Page, matches: (url: URL) => boolean): Promise<ResponseScan> {
  // The buffered bodies, keyed by the Request the response carries — the SAME object the route
  // handler saw, so no url or timing match is needed.
  const buffered = new Map<Request, string>();
  await page.route(matches, async (route) => {
    const request = route.request();
    if (!isServerAction(request) && !isRscGet(request)) {
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
  });

  const pending: Promise<Seen>[] = [];
  // NAMED, so the listener can be detached before the drain in finish(). An anonymous listener
  // cannot be, and a response arriving after Promise.all consumed the array would be silently
  // unscanned.
  const onResponse = (response: Response): void => {
    pending.push(capture(response, buffered));
  };
  page.on('response', onResponse);

  return {
    async finish() {
      // Wait for the stream to go quiet BEFORE detaching the listener, so a late payload is
      // scanned rather than dropped. Bounded, so a chatty page cannot hang this row (F15: never
      // `networkidle`).
      const quiesceDeadline = Date.now() + QUIESCE_TIMEOUT_MS;
      let settledCount = -1;
      while (pending.length !== settledCount && Date.now() < quiesceDeadline) {
        settledCount = pending.length;
        await page.waitForTimeout(QUIESCE_MS);
      }
      const quiesced = pending.length === settledCount;
      // Detach FIRST, then drain: after this line the array cannot grow, so one Promise.all is total.
      page.off('response', onResponse);
      const seen = await Promise.all(pending);
      return { seen, quiesced };
    },
  };
}
