// SPDX-License-Identifier: Apache-2.0

/**
 * Waits until React hydrated the page: `Providers` sets `html[data-hydrated="true"]` in an effect.
 * A click before that is a plain document request, not a client navigation and not a Server Action
 * call (no `Next-Action` header), so every test waits for it before a click.
 */

/**
 * The bound on that wait (SMA-639). ONE value, deliberately not a `process.env.CI` branch: both
 * playwright.config.ts files set `retries: isCI ? 2 : 0`, so a CI-branched constant would have put
 * the TIGHTER bound on the run with NO retry to absorb it.
 *
 * 15 s is the number this repository already calibrated for a hydrating page on a loaded runner —
 * it is both configs' CI `expect.timeout`, and the comment above that setting says why. It is 1/8
 * of the 120 s CI test budget and 1/4 of the 60 s local one, so the wait fails fast in both.
 *
 * Without it, `locator.waitFor()` has NO timeout at all in Playwright 1.63 (no config here sets
 * `use.actionTimeout`), so the wait consumes the whole test budget and then reports the locator
 * rather than a cause.
 */
export const HYDRATION_TIMEOUT_MS = 15_000;

/**
 * The surface the helper uses, structurally — NOT `Page`, and NOT `Pick<Page, 'locator'>`.
 *
 * MEASURED: `Pick<Page, 'locator'>` keeps `locator()`'s return type as the full `Locator`
 * interface, so a plain stub is rejected with `error TS2322` and a unit test would need
 * `as unknown as Locator`, which removes tsc from the stub entirely. This form is satisfied by a
 * real `Page` (method-parameter bivariance) AND by a plain object, with no cast either way.
 *
 * A real `Page` satisfies this structurally (method-parameter bivariance); `login.ts`'s `signIn`
 * passes one, so `typecheck` proves it at a real call site and this module needs no
 * `@playwright/test` import at all.
 */
export type HydrationPage = {
  locator(selector: string): { waitFor(options: { state: 'attached'; timeout: number }): Promise<void> };
};

const SELECTOR = 'html[data-hydrated="true"]';

export async function waitForHydration(page: HydrationPage): Promise<void> {
  try {
    await page.locator(SELECTOR).waitFor({ state: 'attached', timeout: HYDRATION_TIMEOUT_MS });
  } catch (error: unknown) {
    // ONLY a timeout gets the explanation. Playwright rejects a pending waitFor with "Target page,
    // context or browser has been closed" or "Test ended." during teardown, and reporting those as
    // "the client bundle did not run" would be a confident WRONG diagnosis — worse than the bare
    // locator this helper exists to replace. The check is on `name` rather than
    // `instanceof TimeoutError` because importing that class would give this module a RUNTIME
    // @playwright/test dependency and make its own unit test impossible.
    if (!(error instanceof Error) || error.name !== 'TimeoutError') throw error;
    throw new Error(
      `React never hydrated: ${SELECTOR} was not attached within ${String(HYDRATION_TIMEOUT_MS)} ms. ` +
        '`Providers` sets that attribute in an effect, so its absence means the client bundle did not run — ' +
        'a 404 on a chunk, a hydration error thrown before the effect, or a browser too CPU-starved to reach it. ' +
        `Playwright reported: ${error.message}`,
      { cause: error },
    );
  }
}
