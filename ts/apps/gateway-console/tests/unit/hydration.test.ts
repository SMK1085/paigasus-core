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
 * Removes block and line comments from source text, and masks the CONTENTS of every string and
 * template literal, before the result is scanned for unbounded `.waitFor(` calls. Length and
 * structure survive — every masked span keeps its original character count, and a string's quote
 * delimiters are kept unmasked — only the characters that could be mistaken for something else
 * are replaced.
 *
 * Comments are removed so prose mentioning the API in a doc comment cannot masquerade as a real
 * call (a false POSITIVE, demonstrated against this very file — see the doc comment on
 * `HYDRATION_TIMEOUT_MS` in `hydration.ts`) and, more dangerously, so a *comment* that happens to
 * contain the substring `timeout` cannot make the scan silently SKIP a real unguarded call next to
 * it (a false NEGATIVE — this repo has a recorded history of assertions going inert exactly this
 * way).
 *
 * String and template-literal CONTENT is masked for the mirror reason: a `.waitFor(` spelled out
 * inside a quoted string or a template literal is prose, not code, and must not be reported as a
 * finding — and, the opposite hazard, a `timeout` spelled inside a real call's own string
 * arguments must not make an unbounded call look bounded. Masking rather than dropping keeps the
 * masked text the same LENGTH as `source` at every position, so `findUnboundedWaitFor` can map a
 * match found here straight back to the same offsets in the real source and report the call's
 * actual, unmasked text.
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
 * is the only thing that ends it.
 *
 * A template literal's `${...}` interpolation is executable CODE, not literal text, so it is
 * scanned as code rather than masked along with the rest of the template: hitting an unescaped
 * `${` while in `template` state pushes a frame onto `interpolationDepths` and switches to `code`;
 * the matching `}` — the one found while that frame's own nested-brace count is back at zero — pops
 * the frame and switches back to `template`. The count is what stops an object literal or a block
 * body written inside the interpolation (`${ f({ a: 1 }) }`) from ending it on its own first `}`:
 * every `{` seen in `code` state while a frame is open increments that frame's count, and every `}`
 * decrements it, so only a `}` at count zero is the interpolation's own closer. A nested template
 * literal inside an interpolation (`${ \`a${b}c\` }`) needs no special case: entering it pushes the
 * ordinary `template` state as usual, and its own interpolations push and pop their own frames on
 * the same stack — the outer frame's count is untouched while the inner template is open, because
 * brace-counting only runs in `code` state and a nested template is its own state.
 */
function stripCommentsAndStringContents(source: string): string {
  let out = '';
  let state: 'code' | 'line' | 'block' | 'single' | 'double' | 'template' = 'code';
  // One entry per currently-open `${...}` interpolation, innermost last. Each entry is the count of
  // `{` seen inside that interpolation (in `code` state) not yet matched by a `}` — see the doc
  // comment above.
  const interpolationDepths: number[] = [];
  for (let i = 0; i < source.length; i++) {
    const ch = source[i] as string;
    const next = source[i + 1];
    if (state === 'code') {
      if (ch === '/' && next === '/') {
        state = 'line';
        out += '  ';
        i++;
        continue;
      }
      if (ch === '/' && next === '*') {
        state = 'block';
        out += '  ';
        i++;
        continue;
      }
      if (interpolationDepths.length > 0 && (ch === '{' || ch === '}')) {
        const top = interpolationDepths[interpolationDepths.length - 1] as number;
        if (ch === '{') {
          interpolationDepths[interpolationDepths.length - 1] = top + 1;
        } else if (top === 0) {
          // The interpolation's own closer, not a brace opened inside it: this `${...}` is done —
          // resume masking the enclosing template literal's text.
          interpolationDepths.pop();
          state = 'template';
        } else {
          interpolationDepths[interpolationDepths.length - 1] = top - 1;
        }
        out += ch;
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
      } else {
        out += ' ';
      }
      continue;
    }
    if (state === 'block') {
      if (ch === '*' && next === '/') {
        state = 'code';
        out += '  ';
        i++;
      } else {
        out += ch === '\n' ? '\n' : ' ';
      }
      continue;
    }
    // An unescaped `${` inside a template literal starts an interpolation: everything from here to
    // its matching `}` is code, not literal text, and must be scanned (and left unmasked) as such.
    if (state === 'template' && ch === '$' && next === '{') {
      interpolationDepths.push(0);
      out += ch + next;
      state = 'code';
      i++;
      continue;
    }
    // single, double, template: opaque string spans. Only an unescaped matching quote closes one.
    // Content is masked to a filler space — preserving the span's length, not its characters — so
    // a `.waitFor(` spelled inside cannot be mistaken for a real call and a `timeout` spelled
    // inside cannot be mistaken for the real option. The quote delimiters are kept, unmasked.
    if (ch === '\\' && next !== undefined) {
      out += '  ';
      i++;
      continue;
    }
    const closer = state === 'single' ? "'" : state === 'double' ? '"' : '`';
    if (ch === closer) {
      out += ch;
      state = 'code';
      continue;
    }
    out += ' ';
  }
  return out;
}

/**
 * The list of `.waitFor(...)` findings in `source` whose call omits an explicit `timeout`.
 * Matching runs against the masked text from `stripCommentsAndStringContents` — so a `.waitFor(`
 * inside a comment or a string is never a finding, and a `timeout` inside a string can never hide
 * a real one — but every masked span keeps the masked text the same length as `source`, so each
 * finding is sliced back out of the ORIGINAL `source` at those same offsets: the reported text is
 * the real, unmasked call, not a string full of filler spaces.
 */
function findUnboundedWaitFor(source: string): string[] {
  const masked = stripCommentsAndStringContents(source);
  return [...masked.matchAll(/\.waitFor\(([^)]*)\)/g)]
    .filter((match) => !(match[1] ?? '').includes('timeout'))
    .map((match) => {
      const start = match.index ?? 0;
      return source.slice(start, start + match[0].length);
    });
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
    // The literal pin. It is also the only thing keeping the two consoles' values equal; each
    // app's relational assertions below constrain only its own copy against its own config. It is
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

  it('does not report a .waitFor( spelled out inside a quoted string or a template literal', () => {
    // Regression fixture for the false positive the old `stripComments` had: it preserved string
    // CONTENT verbatim, so a `.waitFor(` written out as prose inside a quoted string or a template
    // literal matched the same as real code and was reported as an unbounded wait. A documentation
    // example, a log message, or a test fixture string can now mention the API freely without
    // tripping the scan — while a genuine unbounded call elsewhere in the same source is still
    // caught.
    const fixture = [
      `const inSingleOrDouble = "example: page.locator('x').waitFor({ state: 'attached' })";`,
      `const inTemplate = \`example: page.locator('y').waitFor({ state: 'attached' })\`;`,
      `await page.locator('real').waitFor({ state: 'attached', marker: 'still-unbounded' });`,
    ].join('\n');

    expect(findUnboundedWaitFor(fixture)).toEqual([".waitFor({ state: 'attached', marker: 'still-unbounded' })"]);
  });

  it('scans a template literal interpolation as code, not as literal text', () => {
    // A `${...}` interpolation runs as real code, so a `.waitFor(` written inside one is a real
    // defect and must be caught the same as anywhere else. Masking the whole template span (the
    // pre-fix behavior) hid this case entirely — a false NEGATIVE in a gate whose whole purpose is
    // not to go inert. Four shapes: a real unguarded call inside an interpolation (must be
    // reported); a guarded call inside another interpolation (must not); an interpolation whose
    // object-literal argument has its own nested braces, followed by a real unguarded call outside
    // any template (the later call must still be reported — proving the brace-depth counter, not a
    // naive first-`}`-closes-it toggle, is what decides where the interpolation ends); and literal
    // template text mentioning the API outside any interpolation, in a template that also contains
    // a real (guarded) interpolation (must not be reported).
    const fixture = [
      "const mixed = `literal says .waitFor({ state: 'attached', marker: 'literal-not-code' }) then ${page.locator('interp-guarded').waitFor({ state: 'attached', timeout: 2 })} end`;",
      "const unguarded = `before ${page.locator('interp-unbounded').waitFor({ state: 'attached', marker: 'interp-unbounded' })} after`;",
      'const withBraces = `value: ${f({ nested: { a: 1 } })}`;',
      "await page.locator('after-braces').waitFor({ state: 'attached', marker: 'after-braces' });",
    ].join('\n');

    expect(findUnboundedWaitFor(fixture)).toEqual([".waitFor({ state: 'attached', marker: 'interp-unbounded' })", ".waitFor({ state: 'attached', marker: 'after-braces' })"]);
  });
});
