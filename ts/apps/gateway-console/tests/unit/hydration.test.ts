// SPDX-License-Identifier: Apache-2.0
//
// waitForHydration (SMA-639). The helper is deliberately free of any RUNTIME @playwright/test
// import, so these cases run with no browser and exercise the failure path on every pull request
// — the path an e2e run never reaches while the tier is green.
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import config from '../../playwright.config';
import { HYDRATION_TIMEOUT_MS, waitForHydration, type HydrationPage } from '../e2e/support/hydration';

type Recorded = { selector: string; options: { state: 'attached'; timeout: number } };

/** A stub page recording what the helper asked for, and resolving or rejecting on command. */
function stubPage(outcome: { reject?: Error } = {}): { page: HydrationPage; calls: Recorded[] } {
  const calls: Recorded[] = [];
  const page: HydrationPage = {
    locator: (selector: string) => ({
      waitFor: async (options: { state: 'attached'; timeout: number }): Promise<void> => {
        calls.push({ selector, options });
        if (outcome.reject !== undefined) throw outcome.reject;
      },
    }),
  };
  return { page, calls };
}

function timeoutError(message: string): Error {
  const error = new Error(message);
  error.name = 'TimeoutError';
  return error;
}

describe('waitForHydration', () => {
  it('waits for the hydration attribute with an explicit timeout', async () => {
    const { page, calls } = stubPage();
    await waitForHydration(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.selector).toBe('html[data-hydrated="true"]');
    expect(calls[0]?.options.state).toBe('attached');
    expect(calls[0]?.options.timeout).toBe(HYDRATION_TIMEOUT_MS);
  });

  it('resolves silently when the attribute attaches', async () => {
    const { page } = stubPage();
    await expect(waitForHydration(page)).resolves.toBeUndefined();
  });

  it('explains a timeout: the attribute, the effect, and each of the three causes', async () => {
    const { page } = stubPage({ reject: timeoutError('locator.waitFor: Timeout 15000ms exceeded.') });
    const error = await waitForHydration(page).catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(Error);
    const message = (error as Error).message;
    // Each cause is asserted SEPARATELY. One whole-string equality would pass against a second
    // copy of the same literal and prove nothing about the message actually shipped.
    expect(message).toContain('html[data-hydrated="true"]');
    expect(message).toContain('Providers');
    expect(message).toContain('client bundle did not run');
    expect(message).toContain('404');
    expect(message).toContain('hydration error');
    expect(message).toContain('CPU-starved');
  });

  it('keeps the original timeout error as the cause and quotes its message', async () => {
    const original = timeoutError('locator.waitFor: Timeout 15000ms exceeded.');
    const { page } = stubPage({ reject: original });
    const error = await waitForHydration(page).catch((caught: unknown) => caught);
    expect((error as Error).cause).toBe(original);
    expect((error as Error).message).toContain('Timeout 15000ms exceeded.');
  });

  it('rethrows a non-timeout rejection unchanged', async () => {
    // Playwright rejects a pending waitFor this way during teardown. Reporting it as "the client
    // bundle did not run" would be a confident WRONG diagnosis.
    const original = new Error('Target page, context or browser has been closed');
    const { page } = stubPage({ reject: original });
    const error = await waitForHydration(page).catch((caught: unknown) => caught);
    expect(error).toBe(original);
    expect((error as Error).message).not.toContain('Providers');
  });

  it('is bounded well inside the test budget and never below the expect timeout', () => {
    // The literal pin. It is also the ONLY thing keeping this app's value equal to iam-console's:
    // the two relational assertions below constrain each app against its own config only.
    expect(HYDRATION_TIMEOUT_MS).toBe(15_000);
    // defineConfig's return type makes both optional and this project is strict, so an absent
    // value must FAIL here rather than skip the assertion.
    const budget = config.timeout;
    const expectTimeout = config.expect?.timeout;
    expect(typeof budget).toBe('number');
    expect(typeof expectTimeout).toBe('number');
    expect(HYDRATION_TIMEOUT_MS).toBeGreaterThanOrEqual(expectTimeout as number);
    expect(HYDRATION_TIMEOUT_MS).toBeLessThanOrEqual((budget as number) / 4);
  });

  it('leaves no unbounded waitFor anywhere in this app e2e tree', () => {
    // What stops the defect returning in a NEW helper in this app. It cannot see a third app —
    // that residual is stated in the spec, § 8 D5.
    const root = fileURLToPath(new URL('../e2e', import.meta.url));
    const files: string[] = [];
    const walk = (dir: string): void => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) walk(full);
        else if (entry.name.endsWith('.ts')) files.push(full);
      }
    };
    walk(root);
    expect(files.length).toBeGreaterThan(0);
    const unbounded = files.flatMap((file) =>
      [...readFileSync(file, 'utf8').matchAll(/\.waitFor\(([^)]*)\)/g)]
        .filter((match) => !(match[1] ?? '').includes('timeout'))
        .map((match) => `${path.relative(root, file)}: .waitFor(${match[1] ?? ''})`),
    );
    expect(unbounded).toEqual([]);
  });
});
