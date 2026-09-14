// SPDX-License-Identifier: Apache-2.0
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { nextAppRules } from '../src/eslint.mjs';

/** The shape of one entry in the shipped `ts/eslint.config.js` flat-config array. */
type ShippedBlock = {
  files?: string[];
  plugins?: Record<string, unknown>;
  settings?: { next?: { rootDir?: string } };
};

/**
 * Imports the REAL `ts/eslint.config.js`, not a text scan. A substring check (`toContain`) would
 * still pass for a dead import, or for `...nextAppRules({ appsDir, appNames: ['iam-console'],
 * plugin: nextPlugin })` — a hardcoded list that still calls the helper — which is exactly the
 * regression this task exists to prevent. Same pattern as tests/boundaries.test.ts's "carries
 * every ... entry in its EXPORTED array" cases (`await import('../../../eslint.config.js')`).
 */
async function nextBlocksInShippedConfig(): Promise<ShippedBlock[]> {
  const shipped = (await import('../../../eslint.config.js')).default as ShippedBlock[];
  return shipped.filter((entry) => entry.plugins?.['@next/next'] !== undefined);
}

/**
 * SMA-512. The Next plugin's rules were scoped to one hardcoded app, so a second zone app would
 * have shipped with NO Next lint rules and nothing would have said so — the same silent-skip
 * shape as the next-env and tailwind-source gates.
 */
describe('nextAppRules', () => {
  const plugin = { configs: { recommended: { rules: { 'x/y': 'error' } } } };

  it('returns one block per app name', () => {
    const blocks = nextAppRules({ appsDir: '/repo/ts/apps', appNames: ['a', 'b'], plugin });
    expect(blocks).toHaveLength(2);
    expect(blocks.map((b) => b.files[0])).toEqual(['apps/a/**/*.{ts,tsx}', 'apps/b/**/*.{ts,tsx}']);
  });

  it('gives every block its OWN rootDir', () => {
    const blocks = nextAppRules({ appsDir: '/repo/ts/apps', appNames: ['a', 'b'], plugin });
    expect(blocks.map((b) => b.settings.next.rootDir)).toEqual([join('/repo/ts/apps', 'a'), join('/repo/ts/apps', 'b')]);
  });

  it('carries the plugin and its recommended rules', () => {
    const [block] = nextAppRules({ appsDir: '/repo/ts/apps', appNames: ['a'], plugin });
    // Optional chaining, not block.plugins/block.rules: ts/tsconfig.base.json sets
    // noUncheckedIndexedAccess, so destructuring the first element of an array leaves `block`
    // possibly undefined. Same convention as tests/no-js-relative-specifier.test.ts's `block?.name`.
    expect(block?.plugins['@next/next']).toBe(plugin);
    expect(block?.rules).toEqual({ 'x/y': 'error' });
  });

  it('returns nothing for no apps, rather than one catch-all block', () => {
    expect(nextAppRules({ appsDir: '/repo/ts/apps', appNames: [], plugin })).toEqual([]);
  });
});

/** Mirrors the existing boundaryRules/sourceRules spread assertions. */
describe('ts/eslint.config.js', () => {
  // 30s, not vitest's default 5s: this test boots a real ESLint against the SHIPPED flat
  // config, and the FIRST such call in a file pays the whole config-resolution cost — the
  // siblings after it run in tens of milliseconds. It timed out on CI (9.7s and 5.9s) while
  // passing locally, because a cold runner resolves every workspace package from scratch.
  // The timeout is the cost of the real-config assertion, not slack for a slow unit test.
  it('carries a Next block for the app that exists today', async () => {
    const nextBlocks = await nextBlocksInShippedConfig();
    // Computed the same way ts/eslint.config.js computes it (three levels up from this test file
    // to ts/, then apps/iam-console), so a wrong `appsDir` join in the real config fails here too
    // — not only a check against a literal string that could drift from the real path.
    const expectedRootDir = join(import.meta.dirname, '..', '..', '..', 'apps', 'iam-console');
    expect(nextBlocks).toContainEqual(
      expect.objectContaining({
        files: ['apps/iam-console/**/*.{ts,tsx}'],
        settings: { next: { rootDir: expectedRootDir } },
      }),
    );
  }, 30_000);

  // 30s, not vitest's default 5s: this test boots a real ESLint against the SHIPPED flat
  // config, and the FIRST such call in a file pays the whole config-resolution cost — the
  // siblings after it run in tens of milliseconds. It timed out on CI (9.7s and 5.9s) while
  // passing locally, because a cold runner resolves every workspace package from scratch.
  // The timeout is the cost of the real-config assertion, not slack for a slow unit test.
  it('carries exactly one Next block per app directory actually on disk', async () => {
    // The regression this task exists to prevent: a hardcoded appNames LIST would still satisfy
    // the case above (today there is only one app), so this asserts against the real filesystem
    // instead — it fails the day someone hardcodes the list and a second app exists.
    const appsDir = join(import.meta.dirname, '..', '..', '..', 'apps');
    const appNames = readdirSync(appsDir, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name)
      .sort();
    const nextBlocks = await nextBlocksInShippedConfig();
    // noUncheckedIndexedAccess makes `block.files?.[0]` `string | undefined`; an `undefined` here
    // (a Next block with no `files` entry) would make this array NOT equal `appNames.map(...)`
    // (all real strings), so the assertion still fails loudly rather than passing vacuously.
    const shippedFiles = nextBlocks.map((block) => block.files?.[0]).sort();
    expect(shippedFiles).toEqual(appNames.map((name) => `apps/${name}/**/*.{ts,tsx}`));
  }, 30_000);

  it('no longer carries the old single hardcoded block', () => {
    // Cheap regression guard for the specific shape removed by this task, kept alongside the
    // stronger real-config assertions above.
    const config = readFileSync(join(import.meta.dirname, '..', '..', '..', 'eslint.config.js'), 'utf8');
    expect(config).not.toContain("files: ['apps/iam-console/**/*.{ts,tsx}']");
  });
});
