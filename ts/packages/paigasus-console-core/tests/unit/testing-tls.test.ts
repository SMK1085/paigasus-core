// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { testTls } from '../../testing/index';

/** True when the certificate is still valid `seconds` from now. */
function validFor(certPath: string, seconds: number): boolean {
  try {
    execFileSync('openssl', ['x509', '-in', certPath, '-checkend', String(seconds), '-noout']);
    return true;
  } catch {
    return false;
  }
}

const TWO_DAYS = 2 * 24 * 60 * 60;

describe('testTls validity and root isolation', () => {
  it('issues a one-day certificate by default', () => {
    const root = mkdtempSync(path.join(os.tmpdir(), 'tls-default-'));
    const material = testTls({ root });
    expect(validFor(material.certPath, TWO_DAYS)).toBe(false);
  });

  it('issues a longer certificate when days is given', () => {
    const root = mkdtempSync(path.join(os.tmpdir(), 'tls-long-'));
    const material = testTls({ root, days: 30 });
    expect(validFor(material.certPath, TWO_DAYS)).toBe(true);
  });

  it('isolates two roots, so a dev certificate never reaches an e2e run', () => {
    const devRoot = mkdtempSync(path.join(os.tmpdir(), 'tls-dev-'));
    const e2eRoot = mkdtempSync(path.join(os.tmpdir(), 'tls-e2e-'));
    const dev = testTls({ root: devRoot, days: 30 });
    const e2e = testTls({ root: e2eRoot });

    expect(dev.certPath.startsWith(devRoot)).toBe(true);
    expect(e2e.certPath.startsWith(e2eRoot)).toBe(true);
    expect(readFileSync(dev.certPath, 'utf8')).not.toEqual(readFileSync(e2e.certPath, 'utf8'));
    expect(validFor(dev.certPath, TWO_DAYS)).toBe(true);
    expect(validFor(e2e.certPath, TWO_DAYS)).toBe(false);
  });
});
