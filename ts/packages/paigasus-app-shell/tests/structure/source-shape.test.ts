// SPDX-License-Identifier: Apache-2.0
// @vitest-environment node
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { importsOf, isUseClient, sourceFiles } from '../support/imports';

const PACKAGE_DIR = fileURLToPath(new URL('../..', import.meta.url));
const SRC_DIR = `${PACKAGE_DIR}src`;
const pkg = JSON.parse(readFileSync(`${PACKAGE_DIR}package.json`, 'utf8')) as {
  exports: Record<string, string>;
  dependencies?: Record<string, string>;
  devDependencies?: Record<string, string>;
};

describe('package shape', () => {
  it('has exactly one export, the root, pointing at the source (spec § 5.1)', () => {
    expect(pkg.exports).toEqual({ '.': './src/index.ts' });
  });

  it('never depends on @paigasus/sdk, in any dependency field', () => {
    expect(Object.keys({ ...pkg.dependencies, ...pkg.devDependencies })).not.toContain('@paigasus/sdk');
  });
});

describe('source shape', () => {
  const files = sourceFiles(SRC_DIR);

  it('finds the source files (a guard on the walk itself)', () => {
    expect(files).toContain('index.ts');
  });

  it.each(files)('%s opens with the SPDX header', (rel) => {
    expect(readFileSync(`${SRC_DIR}/${rel}`, 'utf8').split('\n')[0]).toBe('// SPDX-License-Identifier: Apache-2.0');
  });

  it.each(files)("%s: a .tsx component carries 'use client' and a .ts helper never does (spec § 5.2)", (rel) => {
    expect(isUseClient(`${SRC_DIR}/${rel}`)).toBe(rel.endsWith('.tsx'));
  });

  it.each(files)('%s has no relative VALUE import with a .js extension', (rel) => {
    // MEASURED (SMA-510): Turbopack in Next 16.3.4 does not map './x.js' to './x.ts'. A consuming
    // Next app compiles every file here, so such a specifier fails that app's `next build` — and
    // vitest, tsc and ESLint all accept it.
    const offenders = importsOf(`${SRC_DIR}/${rel}`).filter((entry) => entry.specifier.startsWith('.') && !entry.typeOnly && entry.specifier.endsWith('.js'));
    expect(offenders).toEqual([]);
  });
});
