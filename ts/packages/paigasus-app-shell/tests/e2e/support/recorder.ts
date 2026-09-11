// SPDX-License-Identifier: Apache-2.0
//
// The request recorder for the browser tier (spec § 10.3). It records EVERY request of a page and
// answers the other zone and the logout route with a stub document, so the specs never need a
// second app. An RSC request is one with the header `rsc: 1` or a `_rsc` query parameter.
import { expect, type Page, type Route } from '@playwright/test';

export type RecordedRequest = {
  readonly pathname: string;
  readonly method: string;
  readonly resourceType: string;
  readonly rsc: boolean;
};

const STUB_DOCUMENT = '<!doctype html><html lang="en"><head><title>stub</title></head><body><p>stub document</p></body></html>';

/** The fixture server's origin, which tests/e2e/global-setup.ts publishes. */
export function origin(): string {
  const value = process.env.APP_SHELL_E2E_ORIGIN;
  if (value === undefined || value === '') {
    throw new Error('APP_SHELL_E2E_ORIGIN is not set: tests/e2e/global-setup.ts did not run, or did not start the fixture server');
  }
  return value;
}

/** Start recording `page`'s requests and install the stubs. Call it BEFORE the first navigation. */
export async function record(page: Page): Promise<RecordedRequest[]> {
  const log: RecordedRequest[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    log.push({ pathname: url.pathname, method: request.method(), resourceType: request.resourceType(), rsc: request.headers()['rsc'] === '1' || url.searchParams.has('_rsc') });
  });
  const stub = (route: Route): Promise<void> => route.fulfill({ status: 200, contentType: 'text/html; charset=utf-8', body: STUB_DOCUMENT });
  // Both "/gateway/…" (a correct cross-zone link) and "/iam/gateway/…" (where a WRONG next/link
  // sends its prefetch and its navigation, F3) get the stub.
  await page.route(/\/(?:iam\/)?gateway\//, stub);
  await page.route(/\/iam\/auth\/logout(?:$|\?)/, stub);
  return log;
}

/** Requests of type `document` recorded after index `start`. */
export function documentsAfter(log: readonly RecordedRequest[], start: number): RecordedRequest[] {
  return log.slice(start).filter((entry) => entry.resourceType === 'document');
}

/** The distinct pathnames of every RSC request so far, sorted. */
export function rscPathnames(log: readonly RecordedRequest[]): string[] {
  return [...new Set(log.filter((entry) => entry.rsc).map((entry) => entry.pathname))].sort();
}

/** Load `path` on the fixture and wait for hydration (Providers sets the marker in an effect). */
export async function loadHydrated(page: Page, path: string): Promise<void> {
  // 'load', NOT 'networkidle': viewport prefetch keeps the network busy and 'networkidle' hangs (spike).
  await page.goto(`${origin()}${path}`, { waitUntil: 'load' });
  await expect(page.locator('html[data-hydrated="true"]')).toHaveCount(1);
}

/** Put a marker on `window`. A document navigation clears it; a soft navigation keeps it. */
export async function setMarker(page: Page): Promise<void> {
  await page.evaluate(() => {
    (window as unknown as { __pgsMarker?: string }).__pgsMarker = 'first-load';
  });
}

export async function readMarker(page: Page): Promise<string | null> {
  return page.evaluate(() => (window as unknown as { __pgsMarker?: string }).__pgsMarker ?? null);
}
