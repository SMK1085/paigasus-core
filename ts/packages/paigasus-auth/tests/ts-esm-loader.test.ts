// SPDX-License-Identifier: Apache-2.0
//
// SMA-511 spec § 7.2. Two plain-`node` harnesses run this package's source through
// tests/fixtures/ts-esm-loader.mjs: the e2e fixture server and the multi-process single-flight
// worker. Both run only in `test-e2e`, behind Docker. This suite proves the loader itself on every
// `test` run, against a throwaway module tree, so a broken retry fails here and not only in e2e.
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const LOADER = pathToFileURL(fileURLToPath(new URL('./fixtures/ts-esm-loader.mjs', import.meta.url))).href;

let root: string;

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'auth-ts-esm-loader-'));
  mkdirSync(join(root, 'dir'));
  writeFileSync(join(root, 'leaf.ts'), 'export const leaf: number = 40;\n');
  writeFileSync(join(root, 'dir', 'index.ts'), 'export const fromDir: number = 2;\n');
  writeFileSync(join(root, 'extensionless.ts'), "import { leaf } from './leaf';\nimport { fromDir } from './dir';\nconsole.log(String(leaf + fromDir));\n");
  writeFileSync(join(root, 'js-suffixed.ts'), "import { leaf } from './leaf.js';\nconsole.log(String(leaf));\n");
  writeFileSync(join(root, 'missing.ts'), "import { nothing } from './does-not-exist';\nconsole.log(String(nothing));\n");
});

afterAll(() => {
  rmSync(root, { recursive: true, force: true });
});

function run(entry: string, withLoader = true) {
  const args = withLoader ? ['--import', LOADER, join(root, entry)] : [join(root, entry)];
  return spawnSync(process.execPath, args, { encoding: 'utf8', timeout: 30_000 });
}

describe('tests/fixtures/ts-esm-loader.mjs', () => {
  it('resolves an extensionless specifier to ./x.ts, and a directory to ./x/index.ts', () => {
    const result = run('extensionless.ts');
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout.trim()).toBe('42');
  });

  it('still resolves a .js specifier to its .ts sibling (the test files keep that form)', () => {
    const result = run('js-suffixed.ts');
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout.trim()).toBe('40');
  });

  it('still fails on a specifier that names no file, instead of hiding it', () => {
    const result = run('missing.ts');
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('ERR_MODULE_NOT_FOUND');
    expect(result.stderr).toContain('does-not-exist');
  });

  it('control: plain node without the loader cannot resolve the extensionless tree', () => {
    const result = run('extensionless.ts', false);
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('ERR_MODULE_NOT_FOUND');
  });
});
