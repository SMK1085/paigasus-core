// SPDX-License-Identifier: Apache-2.0
// SMA-536 — unit tests for check-installed-bindings.mjs, the installed-typings preflight.
// Run from ts/:  node --test scripts/check-installed-bindings.test.mjs
// `moon run ts:test` runs them, and ci.yml's `:test` target selects that task.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import process from 'node:process';
import { test } from 'node:test';

const SCRIPT = join(import.meta.dirname, 'check-installed-bindings.mjs');
const REPAIR = 'rm -rf ts/node_modules && pnpm -C ts install';
const TYPINGS = 'export declare function sum(a: number, b: number): number\n';
const DECLARING = { name: '@x/k', dependencies: { '@x/b': 'file:../../../rs/b' } };

// <root>/rs/b is the committed binding. <root>/ts/packages/k declares it as `file:../../../rs/b`.
// The installed copy is a COPY under <root>/ts/node_modules/.pnpm (new inodes, equal bytes), and
// <root>/ts/packages/k/node_modules/@x/b is a symlink to it. That is the shape pnpm makes.
function makeFixture({ declaring = DECLARING, install = true } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'sma536-'));
  const committed = join(root, 'rs', 'b');
  mkdirSync(committed, { recursive: true });
  writeFileSync(join(committed, 'package.json'), JSON.stringify({ name: '@x/b', types: 'index.d.ts', files: ['index.js', 'index.d.ts'] }));
  writeFileSync(join(committed, 'index.d.ts'), TYPINGS);
  writeFileSync(join(committed, 'index.js'), 'module.exports = {};\n');
  const pkg = join(root, 'ts', 'packages', 'k');
  mkdirSync(pkg, { recursive: true });
  writeFileSync(join(pkg, 'package.json'), typeof declaring === 'string' ? declaring : JSON.stringify(declaring));
  const installed = join(root, 'ts', 'node_modules', '.pnpm', 'b', 'node_modules', '@x', 'b');
  if (install) {
    cpSync(committed, installed, { recursive: true });
    mkdirSync(join(pkg, 'node_modules', '@x'), { recursive: true });
    symlinkSync(installed, join(pkg, 'node_modules', '@x', 'b'));
  }
  return { root, committed, installed };
}

function check(fx) {
  try {
    return spawnSync(process.execPath, [SCRIPT, '--ts-root', join(fx.root, 'ts')], { encoding: 'utf8' });
  } finally {
    rmSync(fx.root, { recursive: true, force: true });
  }
}

test('equal copies with new inodes give exit 0', () => {
  const fx = makeFixture();
  assert.notEqual(statSync(join(fx.installed, 'index.d.ts')).ino, statSync(join(fx.committed, 'index.d.ts')).ino);
  const r = check(fx);
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /checked 1 file: dependencies/);
});

test('a differing .d.ts gives exit 1 and names the file and the repair command', () => {
  const fx = makeFixture();
  writeFileSync(join(fx.installed, 'index.d.ts'), 'export declare function sum(a: string, b: number): number\n');
  const r = check(fx);
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /@x\/b: the installed index\.d\.ts differs from the committed rs\/b\/index\.d\.ts/);
  assert.ok(r.stderr.includes(REPAIR), r.stderr);
});

test('a differing binding package.json gives exit 1', () => {
  const fx = makeFixture();
  writeFileSync(join(fx.installed, 'package.json'), JSON.stringify({ name: '@x/b', types: 'other.d.ts', files: ['index.js', 'index.d.ts'] }));
  const r = check(fx);
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /@x\/b: the installed package\.json differs/);
  assert.ok(r.stderr.includes(REPAIR), r.stderr);
});

test('a missing installed copy gives exit 1', () => {
  const r = check(makeFixture({ install: false }));
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /@x\/b: no installed copy at ts\/packages\/k\/node_modules\/@x\/b/);
  assert.ok(r.stderr.includes(REPAIR), r.stderr);
});

test('a dangling installed symlink (ts/node_modules removed, no reinstall) gives exit 1', () => {
  const fx = makeFixture();
  rmSync(join(fx.root, 'ts', 'node_modules'), { recursive: true, force: true });
  const r = check(fx);
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /@x\/b: no installed copy/);
  assert.ok(r.stderr.includes(REPAIR), r.stderr);
});

test('an installed copy that lacks a listed .d.ts gives exit 1', () => {
  const fx = makeFixture();
  rmSync(join(fx.installed, 'index.d.ts'));
  const r = check(fx);
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /@x\/b: the installed copy has no index\.d\.ts/);
});

test('a differing listed file that is not a .d.ts (runtime glue) gives exit 0', () => {
  const fx = makeFixture();
  writeFileSync(join(fx.installed, 'index.js'), 'module.exports = { stale: true };\n');
  const r = check(fx);
  assert.equal(r.status, 0, r.stderr);
});

test('a package with no file: dependency gives exit 0', () => {
  const r = check(makeFixture({ declaring: { name: '@x/k', dependencies: { '@x/other': 'workspace:*' } }, install: false }));
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /checked 0 file: dependencies/);
});

test('an unparseable declaring package.json gives exit 2', () => {
  const r = check(makeFixture({ declaring: '{' }));
  assert.equal(r.status, 2, r.stderr);
  assert.match(r.stderr, /installed-bindings: infrastructure error: .*package\.json is not valid JSON/);
});

test('an unparseable binding package.json gives exit 2', () => {
  const fx = makeFixture();
  writeFileSync(join(fx.committed, 'package.json'), '{');
  const r = check(fx);
  assert.equal(r.status, 2, r.stderr);
  assert.match(r.stderr, /installed-bindings: infrastructure error: .*package\.json is not valid JSON/);
});

test('a file: path that does not exist gives exit 2', () => {
  const r = check(makeFixture({ declaring: { name: '@x/k', dependencies: { '@x/b': 'file:../../../rs/ghost' } } }));
  assert.equal(r.status, 2, r.stderr);
  assert.match(r.stderr, /installed-bindings: infrastructure error: @x\/b: the `file:` path .* does not exist/);
});

test('a .d.ts listed in files but not committed gives exit 2', () => {
  const fx = makeFixture();
  rmSync(join(fx.committed, 'index.d.ts'));
  const r = check(fx);
  assert.equal(r.status, 2, r.stderr);
  assert.match(r.stderr, /installed-bindings: infrastructure error: .*`files` lists index\.d\.ts/);
});
