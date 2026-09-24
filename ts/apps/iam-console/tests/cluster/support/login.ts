// SPDX-License-Identifier: Apache-2.0
import { expect, type Page, type Request, type Response } from '@playwright/test';
import { waitForHydration } from '../../e2e/support/hydration';

export { waitForHydration };

export const CONSOLE_HOST = 'console.paigasus.test';
export const IDP_HOST = 'idp.paigasus.test';
export const SESSION_COOKIE = '__Host-pgs_sid';
/** Traefik's body for a request no router matches (spec B10; Review Focus 3). */
export const TRAEFIK_404_BODY = '404 page not found';

function credential(name: 'PAIGASUS_KIND_USERNAME' | 'PAIGASUS_KIND_PASSWORD'): string {
  const value = process.env[name];
  if (value === undefined || value === '') {
    throw new Error(`${name} is not set; run these specs through ci/kind/run.sh specs a|b`);
  }
  return value;
}

/**
 * Open `path` with no session, fill Keycloak's form, and wait until the zone page is back and
 * hydrated (spec § 6.2). It never re-uses and never closes a Keycloak page (SMA-652): the page it
 * leaves behind is the console page, not the Keycloak one.
 */
export async function loginAt(page: Page, path: string): Promise<void> {
  await page.goto(path);
  await expect(page).toHaveURL((url) => url.hostname === IDP_HOST);
  await page.locator('#username').fill(credential('PAIGASUS_KIND_USERNAME'));
  await page.locator('#password').fill(credential('PAIGASUS_KIND_PASSWORD'));
  await page.locator('#kc-login').click();
  await page.waitForURL((url) => url.hostname === CONSOLE_HOST && url.pathname === path);
  await waitForHydration(page);
}

/** The value of the shared session cookie in this page's context. */
export async function sessionCookie(page: Page): Promise<string> {
  const cookie = (await page.context().cookies()).find((c) => c.name === SESSION_COOKIE);
  if (cookie === undefined) throw new Error(`no ${SESSION_COOKIE} cookie in this context`);
  return cookie.value;
}

/** Every hop of the navigation that produced `response`, first hop first. */
export async function redirectChain(response: Response): Promise<{ readonly url: URL; readonly status: number }[]> {
  const chain: { url: URL; status: number }[] = [];
  let request: Request | null = response.request();
  while (request !== null) {
    const hop = await request.response();
    chain.unshift({ url: new URL(request.url()), status: hop?.status() ?? 0 });
    request = request.redirectedFrom();
  }
  return chain;
}
