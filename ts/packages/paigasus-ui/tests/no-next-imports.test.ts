// SPDX-License-Identifier: Apache-2.0
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, sep } from 'node:path';
import { describe, expect, it } from 'vitest';

/*
 * `process.cwd()`, not `import.meta.url`: under the jsdom environment `import.meta.url` is not a
 * file: URL and `fileURLToPath` throws on it. Vitest and the Moon task both run with the package
 * root as the working directory. A wrong root would make the scan find nothing — which FAILS the
 * strict-equality assertion below rather than passing vacuously, so this fails closed.
 */
const TESTS_DIR = join(process.cwd(), 'tests') + sep;

/*
 * Assembled from fragments so THIS file does not contain the literal it searches for — the same
 * trick ci/tailwind-source/run.mjs uses for its sentinel names, and for the same reason: a
 * checker that matches itself proves nothing.
 */
const ESLINT_DIRECTIVE = 'no-restricted' + '-imports';

function collectTestSources(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) collectTestSources(full, out);
    else if (entry.endsWith('.ts') || entry.endsWith('.tsx')) out.push(full);
  }
  return out;
}

describe('the next/* resolver ban', () => {
  it('rejects a module that imports next/link', async () => {
    await expect(import('./fixtures/imports-next')).rejects.toThrow(/must not import from `next`/);
  });

  it('does not reject an ordinary relative import', async () => {
    await expect(import('../src/lib/cn')).resolves.toBeDefined();
  });

  /*
   * SMA-502's eslint boundary rule bans next/* across packages/paigasus-ui/**, tests included —
   * correctly, because a test importing next/link would pull a Next runtime into this jsdom tier
   * and that is exactly what AC 2 forbids. tests/fixtures/imports-next.ts is the one sanctioned
   * exception: its whole job is to violate the ban so the test above can observe the violation
   * being caught.
   *
   * That exception was a comment until now. This assertion makes it a gate: a SECOND suppression
   * anywhere under tests/ means the rule is being worked around rather than excepted, and this
   * reds instead of nobody noticing. Strict equality on the path, not a count, so it also names
   * which file is allowed to carry it.
   */
  it('sanctions exactly one suppression of the boundary rule, in the fixture that needs it', () => {
    const suppressing = collectTestSources(TESTS_DIR)
      .filter((file) => readFileSync(file, 'utf8').includes(ESLINT_DIRECTIVE))
      .map((file) => file.slice(TESTS_DIR.length).split(sep).join('/'))
      .sort();

    expect(suppressing).toEqual(['fixtures/imports-next.ts']);
  });
});
