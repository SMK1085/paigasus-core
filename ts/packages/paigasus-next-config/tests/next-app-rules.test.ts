// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { nextAppRules } from '../src/eslint.mjs';

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
    expect(blocks.map((b) => b.settings.next.rootDir)).toEqual([
      join('/repo/ts/apps', 'a'),
      join('/repo/ts/apps', 'b'),
    ]);
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
  it('spreads nextAppRules over the discovered app directories', () => {
    const config = readFileSync(join(import.meta.dirname, '..', '..', '..', 'eslint.config.js'), 'utf8');
    expect(config).toContain('nextAppRules');
    expect(config).toContain('...nextAppRules(');
    // The hardcoded single-app block must be gone, or coverage silently stops being derived.
    expect(config).not.toContain("files: ['apps/iam-console/**/*.{ts,tsx}']");
  });
});
