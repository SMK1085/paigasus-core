// SPDX-License-Identifier: Apache-2.0
import { expect, type Page, type Request, type Response } from '@playwright/test';
import type { Harness } from './harness';

export type SignedIn = { readonly accessToken: string; readonly refreshToken: string; readonly response: Response };

/**
 * Waits until React hydrated the page: Task 10's `Providers` sets `html[data-hydrated="true"]` in an
 * effect. A click before that is a plain document request, not a client navigation and not a
 * Server Action call (no `Next-Action` header), so every test waits for it before a click.
 */
export async function waitForHydration(page: Page): Promise<void> {
  await page.locator('html[data-hydrated="true"]').waitFor({ state: 'attached' });
}

/**
 * Open `path` with no session and follow the whole login: proxy -> /iam/auth/login -> the fake IdP
 * (it approves at once) -> /iam/auth/callback -> `path`. Every hop is an HTTP redirect, so one
 * page.goto() follows all of them. Returns the tokens the IdP issued for THIS login, after the page
 * hydrated.
 */
export async function signIn(page: Page, harness: Harness, path = '/iam/orgs'): Promise<SignedIn> {
  const before = harness.idp.issued.length;
  const response = await page.goto(harness.url(path));
  if (response === null) throw new Error(`page.goto(${path}) returned no response`);
  expect(new URL(page.url()).pathname).toBe(path);
  await expect(page.getByRole('navigation', { name: 'Primary' })).toBeVisible();
  await waitForHydration(page);
  const issued = harness.idp.issued.slice(before);
  expect(issued).toHaveLength(1);
  const [tokens] = issued;
  if (tokens === undefined) throw new Error('the fake IdP issued no token for this login');
  return { accessToken: tokens.accessToken, refreshToken: tokens.refreshToken, response };
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
