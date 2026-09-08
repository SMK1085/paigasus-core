// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { canonicalBasePath } from '../src/base-path.js';

describe('canonicalBasePath', () => {
  it('accepts an already-canonical prefix unchanged', () => {
    expect(canonicalBasePath('/iam', 'test')).toBe('/iam');
  });

  it('treats a trailing slash as equivalent', () => {
    expect(canonicalBasePath('/iam/', 'test')).toBe('/iam');
  });

  it('adds a missing leading slash', () => {
    expect(canonicalBasePath('iam', 'test')).toBe('/iam');
  });

  it('canonicalises a nested prefix', () => {
    expect(canonicalBasePath('/admin/iam/', 'test')).toBe('/admin/iam');
  });

  it("maps the origin root to '' because Next rejects basePath '/'", () => {
    expect(canonicalBasePath('/', 'test')).toBe('');
    expect(canonicalBasePath('', 'test')).toBe('');
  });

  it('rejects a value that is not a path prefix', () => {
    expect(() => canonicalBasePath('https://evil.example/x', 'test')).toThrow(/test/);
  });

  it('never echoes the rejected value, because the same validator reads operator JSON', () => {
    const secret = 'sk-live-should-never-appear';
    expect(() => canonicalBasePath(`https://${secret}.example`, 'PAIGASUS_ZONES["iam"]')).toThrow();
    try {
      canonicalBasePath(`https://${secret}.example`, 'PAIGASUS_ZONES["iam"]');
    } catch (err) {
      expect(String(err)).not.toContain(secret);
      expect(String(err)).toContain('PAIGASUS_ZONES["iam"]');
    }
  });
});
