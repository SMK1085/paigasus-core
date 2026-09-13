// SPDX-License-Identifier: Apache-2.0
//
// The certificate helper's self-test. It uses its OWN root, so it never touches the pair that the
// other doubles use. The rotation case is the parallel-worker race in sequential form: a process
// that finds the published pair expiring must publish a new pair, and must not delete or change the
// pair that another process already holds.
import { createPrivateKey, X509Certificate } from 'node:crypto';
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import { testTls, type TlsMaterial } from '../../support/tls';

const root = mkdtempSync(path.join(os.tmpdir(), 'paigasus-iam-console-tls-selftest-'));
afterAll(() => rmSync(root, { recursive: true, force: true }));

/** The files on disk are the pair in memory, and the key belongs to the cert. */
function expectIntact(tls: TlsMaterial): void {
  expect(readFileSync(tls.certPath, 'utf8')).toBe(tls.cert);
  expect(readFileSync(tls.keyPath, 'utf8')).toBe(tls.key);
  expect(new X509Certificate(tls.cert).checkPrivateKey(createPrivateKey(tls.key))).toBe(true);
}

describe('testTls', () => {
  it('reuses the published pair while it stays valid', () => {
    const first = testTls({ root });
    const second = testTls({ root });
    expect(second).toEqual(first);
    expectIntact(second);
  });

  it('publishes a new pair when the current one expires, and leaves the held pair intact', () => {
    const held = testTls({ root });
    // Two days is longer than the one-day cert lives, so the published pair counts as expiring.
    const rotated = testTls({ root, minValidSeconds: 2 * 86_400 });
    expect(rotated.certPath).not.toBe(held.certPath);
    expect(rotated.cert).not.toBe(held.cert);
    expectIntact(held);
    expectIntact(rotated);
    // The pointer moved: a call with the default threshold now reuses the new pair.
    expect(testTls({ root })).toEqual(rotated);
  });

  // MEASURED: an older layout under the same tmpdir name left `current` as a DIRECTORY, and the
  // rename then failed with a bare `EISDIR` that named neither this module nor the remedy.
  it('reports a foreign `current`, and neither removes it nor leaks a scratch pointer', () => {
    const foreignRoot = mkdtempSync(path.join(os.tmpdir(), 'paigasus-iam-console-tls-foreign-'));
    mkdirSync(path.join(foreignRoot, 'current'));
    expect(() => testTls({ root: foreignRoot })).toThrow(/is not a regular file/);
    // The foreign directory survives, and no `current-*.tmp` scratch pointer is left behind.
    expect(readdirSync(foreignRoot).filter((name) => name.startsWith('current'))).toEqual(['current']);
    rmSync(foreignRoot, { recursive: true, force: true });
  });
});
