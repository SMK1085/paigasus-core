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
      waitFor: (options: { state: 'attached'; timeout: number }): Promise<void> => {
        calls.push({ selector, options });
        return outcome.reject === undefined ? Promise.resolve() : Promise.reject(outcome.reject);
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

/**
 * Removes block and line comments from source text before it is scanned for unbounded
 * `.waitFor(` calls, so prose mentioning the API in a doc comment cannot masquerade as a real
 * call (a false POSITIVE, demonstrated against this very file — see the doc comment on
 * `HYDRATION_TIMEOUT_MS` in `hydration.ts`) and, more dangerously, so a *comment* that happens to
 * contain the substring `timeout` cannot make the scan silently SKIP a real unguarded call next to
 * it (a false NEGATIVE — this repo has a recorded history of assertions going inert exactly this
 * way).
 *
 * String-aware: a single-pass character scanner, not a pair of regexes. It tracks one of six
 * states — plain code, a single-quoted string, a double-quoted string, a template literal, a line
 * comment, or a block comment — and a slash-star or double-slash only starts a comment while the
 * scanner is in plain code. A regex-based strip runs the comment pattern over the whole source
 * regardless of what it is inside, so a slash-star or double-slash that occurs INSIDE a string
 * literal used to start a "comment" there too, and the lazy match could run past the string and
 * swallow a real `.waitFor(` call before the next genuine block-comment closer (or, for a line
 * comment, the rest of that line). The scanner never makes that mistake: string content is
 * consumed character by character, an escaping backslash is consumed together with the character
 * it escapes so an escaped quote cannot close the string early, and the matching unescaped quote
 * is the only thing that ends it. String content (and the real code around it) is kept in the
 * output; only comment characters are dropped.
 */
function stripComments(source: string): string {
  let out = '';
  let state: 'code' | 'line' | 'block' | 'single' | 'double' | 'template' = 'code';
  for (let i = 0; i < source.length; i++) {
    const ch = source[i] as string;
    const next = source[i + 1];
    if (state === 'code') {
      if (ch === '/' && next === '/') {
        state = 'line';
        i++;
        continue;
      }
      if (ch === '/' && next === '*') {
        state = 'block';
        i++;
        continue;
      }
      if (ch === "'" || ch === '"' || ch === '`') state = ch === "'" ? 'single' : ch === '"' ? 'double' : 'template';
      out += ch;
      continue;
    }
    if (state === 'line') {
      if (ch === '\n') {
        state = 'code';
        out += ch;
      }
      continue;
    }
    if (state === 'block') {
      if (ch === '*' && next === '/') {
        state = 'code';
        i++;
      }
      continue;
    }
    // single, double, template: opaque string spans. Only an unescaped matching quote closes one.
    out += ch;
    if (ch === '\\' && next !== undefined) {
      out += next;
      i++;
      continue;
    }
    const closer = state === 'single' ? "'" : state === 'double' ? '"' : '`';
    if (ch === closer) state = 'code';
  }
  return out;
}

/** The list of `.waitFor(...)` findings in `source` whose call omits an explicit `timeout`. */
function findUnboundedWaitFor(source: string): string[] {
  return [...stripComments(source).matchAll(/\.waitFor\(([^)]*)\)/g)].filter((match) => !(match[1] ?? '').includes('timeout')).map((match) => `.waitFor(${match[1] ?? ''})`);
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

  it('explains a timeout: the attribute, the effect, and each of the four causes', async () => {
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
    expect(message).toContain('error boundary');
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
    // the two relational assertions below constrain each app against its own config only. It is
    // also the ONLY tight constraint on the value in CI: MEASURED, with this literal removed, a
    // `30_000` constant still passes both relational assertions below under `CI=1`, since CI's
    // 120 s budget permits up to 30 s — the `/4` bound is tight only locally.
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

  it('leaves no unbounded locator.waitFor( in this app e2e tree', () => {
    // What stops the defect returning in a NEW helper in this app. It cannot see a third app —
    // that residual is stated in the spec, § 8 D5. It also does not cover the sibling
    // `waitForURL`/`waitForResponse`/`waitForRequest`/`waitForLoadState` APIs: MEASURED,
    // `use.navigationTimeout` also defaults to 0 in playwright@1.63.0, so those are equally
    // unbounded, and the scan's `\.waitFor\(` regex cannot see any of their ~15 live call sites
    // across both apps' e2e trees. Widening the regex is out of scope for this change.
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
    // Pins the walk's SCOPE, not just its non-emptiness: narrowing the root to `support/` would
    // otherwise leave this case green while it scanned almost nothing (measured).
    expect(files.filter((file) => file.endsWith('.spec.ts')).length).toBeGreaterThan(0);
    const unbounded = files.flatMap((file) => findUnboundedWaitFor(readFileSync(file, 'utf8')).map((finding) => `${path.relative(root, file)}: ${finding}`));
    expect(unbounded).toEqual([]);
  });

  it('strips comments before scanning so prose mentions are ignored and real code survives', () => {
    // Four shapes in one fixture: a real unguarded call (must be reported), a real guarded call
    // (must not), the same unguarded shape quoted inside a block comment AND a line comment (must
    // not — this is the false-positive case that bit hydration.ts's own doc comment), and a
    // `https://` URL on a line with real code after it (the code must survive: the `//` is inside
    // a string literal, so the scanner never leaves plain code and never starts a line comment
    // there — proving the string-aware tracking, not a separate regex guard).
    const fixture = [
      `await page.locator('unguarded').waitFor({ state: 'attached', marker: 'real-call' });`,
      `await page.locator('guarded').waitFor({ state: 'attached', timeout: 1 });`,
      `/**`,
      ` * Prose: page.locator('block').waitFor({ state: 'attached', marker: 'block-comment' });`,
      ` */`,
      `// Prose: page.locator('line').waitFor({ state: 'attached', marker: 'line-comment' });`,
      `const docs = 'see https://example.com for details';`,
      `await page.locator('after-url').waitFor({ state: 'attached', marker: 'after-url' });`,
    ].join('\n');

    expect(findUnboundedWaitFor(fixture)).toEqual([".waitFor({ state: 'attached', marker: 'real-call' })", ".waitFor({ state: 'attached', marker: 'after-url' })"]);
  });

  it('is string-aware: a stray /* or // inside a string literal cannot swallow real code', () => {
    // Regression fixture for the false negative the old two-regex `stripComments` had: an
    // unbalanced `/*` or a `//` inside a string ran the corresponding regex past the string and
    // ate a real, unguarded `.waitFor(` call, so the scan reported clean when it should have
    // reported a finding.
    const fixture = [
      // Case 1: a string containing `/*` with no closer on its own line, followed by a real
      // unguarded call. A later GENUINE block comment supplies the accidental closing `*/` that
      // let the old block-comment regex run past the string and the real call in between.
      `const withStar = 'contains /* but nothing closes it here';`,
      `await page.locator('unbounded-star').waitFor({ state: 'attached', marker: 'string-slash-star' });`,
      `/** a real block comment whose closer used to accidentally close the fake one above */`,
      // Case 2: a string containing `//`, followed by a real unguarded call on the SAME line —
      // the old line-comment regex matched to end-of-line and used to eat the call.
      `const withSlashSlash = "contains // not a comment"; await page.locator('unbounded-slash').waitFor({ state: 'attached', marker: 'string-slash-slash' });`,
      // Case 3 (kept covered): a genuine comment mentioning `.waitFor(` with no timeout must
      // still be ignored.
      `// a genuine comment: page.locator('prose').waitFor({ state: 'attached', marker: 'prose-only' });`,
    ].join('\n');

    expect(findUnboundedWaitFor(fixture)).toEqual([".waitFor({ state: 'attached', marker: 'string-slash-star' })", ".waitFor({ state: 'attached', marker: 'string-slash-slash' })"]);
  });
});
