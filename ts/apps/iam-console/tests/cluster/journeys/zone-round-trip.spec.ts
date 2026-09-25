// SPDX-License-Identifier: Apache-2.0
//
// SMA-514 scenario 2 (spec § 6): a logged-in user clicks the shell's link from /iam to /gateway
// and back. Each click must be a hard navigation (a new document request), the session cookie must
// not change, no request may go to the IdP, and no RSC request may leave its zone.
//
// The gateway zone is `available` only after `ci/kind/run.sh stub up` (spec § 4). The seven step
// titles are pinned by ci/kind/journeys-report.mjs (EXPECTED_STEPS).
//
// RSC rule (step 7). An RSC request has the header `rsc` or `next-router-prefetch`, or the query
// parameter `_rsc`; header names are lower-cased before the match. It fails on a request to the
// OTHER zone, on a NESTED cross-zone path (/iam/gateway…, /gateway/iam…), and on a 404. The nested
// rule is needed because a cross-zone NextLink adds its own basePath: it prefetches
// /iam/gateway/?_rsc=…, which IS in the IAM zone (Review Focus 4). The positive control asks for at
// least one same-zone RSC request, so a matcher that never matches cannot pass. The rule checks
// use expect.soft, so a violation after hydration (step 4) does not hide the navigation failure
// that the same defect causes in step 5 (mutation run M1).
import { expect, test, type Request } from '@playwright/test';
import { CONSOLE_HOST, IDP_HOST, loginAt, sessionCookie, waitForHydration } from '../support/login';

type Seen = { readonly request: Request; readonly frameUrl: string };

/** Assumption A1: the default discovery timings; `degraded` -> `available` takes about 10 s plus two renders. */
const STUB_DEADLINE_MS = 30_000;

function zoneOf(pathname: string): 'iam' | 'gateway' | null {
  for (const zone of ['iam', 'gateway'] as const) {
    if (pathname === `/${zone}` || pathname.startsWith(`/${zone}/`)) return zone;
  }
  return null;
}

function isRsc(request: Request): boolean {
  const names = Object.keys(request.headers()).map((name) => name.toLowerCase());
  return names.includes('rsc') || names.includes('next-router-prefetch') || new URL(request.url()).searchParams.has('_rsc');
}

/** The RSC requests that break the rule, and the count of same-zone RSC requests (the positive control). */
function rscFindings(seen: readonly Seen[], statuses: ReadonlyMap<Request, number>): { violations: string[]; sameZone: number } {
  const violations: string[] = [];
  let sameZone = 0;
  for (const { request, frameUrl } of seen) {
    const url = new URL(request.url());
    if (url.hostname !== CONSOLE_HOST || !isRsc(request)) continue;
    let fromPath: string;
    try {
      fromPath = new URL(frameUrl).pathname;
    } catch {
      fromPath = '';
    }
    const label = `${request.method()} ${url.pathname}${url.search} (from ${fromPath === '' ? 'an unknown frame' : fromPath})`;
    if (url.pathname.startsWith('/iam/gateway') || url.pathname.startsWith('/gateway/iam')) {
      violations.push(`nested cross-zone path: ${label}`);
    } else if (zoneOf(url.pathname) === null || zoneOf(url.pathname) !== zoneOf(fromPath)) {
      violations.push(`other zone: ${label}`);
    } else {
      sameZone += 1;
    }
    if (statuses.get(request) === 404) violations.push(`404: ${label}`);
  }
  return { violations, sameZone };
}

function consoleDocuments(seen: readonly Seen[]): Seen[] {
  return seen.filter((entry) => entry.request.resourceType() === 'document' && new URL(entry.request.url()).hostname === CONSOLE_HOST);
}

function idpRequests(seen: readonly Seen[]): string[] {
  return seen.filter((entry) => new URL(entry.request.url()).hostname === IDP_HOST).map((entry) => entry.request.url());
}

/**
 * Waits until the RSC request recording is stable, before a rule check reads it. A recorded RSC
 * request may still be in flight (a queued Next prefetch, for example a nested
 * `/iam/gateway/?_rsc=…` under a cross-zone `NextLink`), and a new RSC request may still arrive.
 * This polls (bound 15 s) until BOTH hold: the count of recorded RSC requests has not changed for
 * about 2 s, and every recorded RSC request has a status (a response arrived) or has failed (the
 * `requestfailed` event fired). A failed RSC request still counts as settled: it is still a
 * violation under the existing zone/path rules, only its 404 check cannot apply to it.
 */
async function waitForStableRsc(seen: readonly Seen[], statuses: ReadonlyMap<Request, number>, failed: ReadonlySet<Request>): Promise<void> {
  const STABLE_MS = 2_000;
  let lastCount = -1;
  let stableSince = Date.now();
  await expect
    .poll(
      () => {
        const rscSeen = seen.filter((entry) => new URL(entry.request.url()).hostname === CONSOLE_HOST && isRsc(entry.request));
        if (rscSeen.length !== lastCount) {
          lastCount = rscSeen.length;
          stableSince = Date.now();
        }
        const allSettled = rscSeen.every((entry) => statuses.has(entry.request) || failed.has(entry.request));
        return allSettled && Date.now() - stableSince >= STABLE_MS;
      },
      { message: 'the RSC recording must be stable: the count unchanged for 2 s and every RSC request settled', timeout: 15_000 },
    )
    .toBe(true);
}

test('J2: the shell links IAM to gateway and back with hard navigations, one session and no cross-zone RSC request (SMA-514 scenario 2)', async ({ context, page }) => {
  const seen: Seen[] = [];
  const statuses = new Map<Request, number>();
  const failed = new Set<Request>();
  const nav = page.getByRole('navigation', { name: 'Primary' });
  const gateway = nav.getByRole('link', { name: 'Gateway', exact: true });

  await test.step('record every request of the context', () => {
    // Before anything else, so the recording includes the prefetches that hydration starts.
    context.on('request', (request) => {
      let frameUrl: string;
      try {
        frameUrl = request.frame().url();
      } catch {
        frameUrl = '';
      }
      seen.push({ request, frameUrl });
    });
    context.on('response', (response) => {
      statuses.set(response.request(), response.status());
    });
    // A failed RSC request (for example to the other zone or a nested cross-zone path) never gets
    // a response, so it must count as SETTLED too, or waitForStableRsc would wait out its timeout.
    context.on('requestfailed', (request) => {
      failed.add(request);
    });
  });

  await test.step('log in at /iam/orgs', async () => {
    await loginAt(page, '/iam/orgs');
  });

  await test.step('wait until the Gateway nav entry is a link', async () => {
    const deadline = Date.now() + STUB_DEADLINE_MS;
    for (;;) {
      await expect(gateway).toHaveCount(1);
      const tag = await gateway.evaluate((element) => element.tagName.toLowerCase());
      if (tag === 'a' && (await gateway.getAttribute('href')) !== null && (await gateway.getAttribute('aria-disabled')) === null) break;
      if (Date.now() >= deadline) {
        // The degraded reason tells `network` (stub not up), `bad-response` (wrong content type)
        // and `not-implemented` (wrong path) apart (primary-nav.tsx:102-104).
        const reasonId = await gateway.getAttribute('aria-describedby');
        const reason = reasonId === null ? '(no reason element)' : ((await page.locator(`[id="${reasonId}"]`).textContent()) ?? '(empty reason)');
        throw new Error(`the Gateway nav entry is still disabled ${String(STUB_DEADLINE_MS / 1000)} s after 'run.sh stub up': "${reason}" (spec § 6 step 3, assumption A1)`);
      }
      await page.waitForTimeout(2_000);
      await page.reload();
      await waitForHydration(page);
    }
  });

  await test.step('RSC check after hydration, before any click', async () => {
    // The IAM nav's same-zone NextLinks prefetch after hydration. Wait for the recording to settle
    // first: a queued prefetch (for example a nested cross-zone one) may not have arrived, or may
    // still be in flight, at the first poll where the positive control already holds (final review).
    await waitForStableRsc(seen, statuses, failed);
    await expect.poll(() => rscFindings(seen, statuses).sameZone, { message: 'positive control: at least one same-zone RSC request after hydration', timeout: 15_000 }).toBeGreaterThan(0);
    expect.soft(rscFindings(seen, statuses).violations, 'no RSC request may leave its zone (after hydration)').toEqual([]);
  });

  await test.step('IAM to gateway: a hard navigation with the same session', async () => {
    const before = await sessionCookie(page);
    const mark = seen.length;
    // href /gateway/ (iam-console/lib/nav.ts:40): the gateway public page redirects to /overview
    // when the cookie exists (gateway-console/app/(public)/page.tsx:17).
    await gateway.click();
    await page.waitForURL((url) => url.hostname === CONSOLE_HOST && url.pathname === '/gateway/overview');
    const documents = consoleDocuments(seen.slice(mark));
    // Measured, not assumed: the chain can start with a trailing-slash redirect.
    test.info().annotations.push({
      type: 'IAM to gateway documents',
      description: documents.map((entry) => `${new URL(entry.request.url()).pathname} ${String(statuses.get(entry.request) ?? 0)}`).join(' -> '),
    });
    expect(
      documents.some((entry) => new URL(entry.request.url()).pathname.startsWith('/gateway/')),
      'the Gateway link must make a document request into /gateway/ (a hard navigation)',
    ).toBe(true);
    const landing = documents.filter((entry) => new URL(entry.request.url()).pathname === '/gateway/overview').at(-1);
    expect(landing === undefined ? 0 : statuses.get(landing.request), 'the final /gateway/overview document').toBe(200);
    await waitForHydration(page);
    expect(await sessionCookie(page), 'the session cookie must not change').toBe(before);
    expect(idpRequests(seen.slice(mark)), 'no request may go to the IdP').toEqual([]);
  });

  await test.step('gateway to IAM: a hard navigation with the same session', async () => {
    const before = await sessionCookie(page);
    const mark = seen.length;
    await nav.getByRole('link', { name: 'IAM', exact: true }).click();
    await page.waitForURL((url) => url.hostname === CONSOLE_HOST && url.pathname === '/iam/orgs');
    const documents = consoleDocuments(seen.slice(mark));
    test.info().annotations.push({
      type: 'gateway to IAM documents',
      description: documents.map((entry) => `${new URL(entry.request.url()).pathname} ${String(statuses.get(entry.request) ?? 0)}`).join(' -> '),
    });
    const landing = documents.filter((entry) => new URL(entry.request.url()).pathname === '/iam/orgs').at(-1);
    expect(landing, 'the IAM link must make a document request to /iam/orgs (a hard navigation)').toBeDefined();
    expect(landing === undefined ? 0 : statuses.get(landing.request), 'the final /iam/orgs document').toBe(200);
    await expect(page.getByRole('heading', { level: 1, name: 'Your organizations' })).toBeVisible();
    await waitForHydration(page);
    expect(await sessionCookie(page), 'the session cookie must not change').toBe(before);
    expect(idpRequests(seen.slice(mark)), 'no request may go to the IdP').toEqual([]);
  });

  await test.step('no RSC request leaves its zone', async () => {
    // Same wait as step 4 (final review): the second click's own prefetches may still be settling.
    await waitForStableRsc(seen, statuses, failed);
    const { violations, sameZone } = rscFindings(seen, statuses);
    expect(sameZone, 'positive control: at least one same-zone RSC request in the whole recording').toBeGreaterThan(0);
    expect.soft(violations, 'no RSC request may leave its zone (the whole recording)').toEqual([]);
  });
});
