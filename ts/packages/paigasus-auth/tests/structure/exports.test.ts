// SPDX-License-Identifier: Apache-2.0
//
// Gap 1 (task 12, escalated from an earlier review): package.json's `_comment_exports` claimed
// "tests/exports.test.ts pins this key set", but no such file existed anywhere in the package —
// the exports map's deliberate absence of a "." root export had no runtime control backing it.
// That absence matters: `import { getSession } from '@paigasus/auth'` must fail to resolve, since
// a root export re-exporting the server surface would let a client bundle walk around the
// `@paigasus/auth/server` app-shell boundary rule. This is that control, and package.json's
// comment now points here.
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const HERE = dirname(fileURLToPath(import.meta.url));
const PACKAGE_JSON = resolve(HERE, '../../package.json');

interface PackageJson {
  exports: Record<string, string>;
}

function readPackageExports(): Record<string, string> {
  const raw = readFileSync(PACKAGE_JSON, 'utf8');
  const pkg = JSON.parse(raw) as PackageJson;
  return pkg.exports;
}

describe('package.json exports map', () => {
  it('exposes exactly ./server, ./client, and ./middleware — and no "." root export', () => {
    // STRICT equality (not toContain/subset): the point is that adding ANY new key here — a "."
    // root export above all — is a deliberate, reviewed edit to this test, never a silent
    // side effect of an unrelated package.json change.
    expect(Object.keys(readPackageExports())).toEqual(['./server', './client', './middleware']);
  });
});
