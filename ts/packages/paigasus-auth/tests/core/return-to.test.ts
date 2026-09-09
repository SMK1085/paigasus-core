// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { validateReturnTo } from '../../src/core/return-to.js';

const FALLBACK = '/iam';

describe('validateReturnTo', () => {
  it.each([
    ['a plain path', '/iam/orgs', '/iam/orgs'],
    ['a path with a query', '/iam/orgs?page=2', '/iam/orgs?page=2'],
    ['a path with a fragment', '/iam/orgs#top', '/iam/orgs#top'],
    ['the root', '/', '/'],
  ])('accepts %s', (_l, input, expected) => {
    expect(validateReturnTo(input, FALLBACK)).toBe(expected);
  });

  // Each of these is an open redirect if it gets through.
  it.each([
    ['a protocol-relative URL', '//evil.com'],
    ['a protocol-relative URL with a path', '//evil.com/x'],
    ['an absolute http URL', 'http://evil.com'],
    ['an absolute https URL', 'https://evil.com'],
    ['a backslash-relative URL', '/\\evil.com'],
    ['a double backslash', '\\\\evil.com'],
    ['a scheme-relative with backslash', '/\\/evil.com'],
    ['a javascript scheme', 'javascript:alert(1)'],
    ['a data scheme', 'data:text/html,x'],
    ['a percent-encoded scheme separator', '/%2f%2fevil.com'],
    ['an encoded backslash', '/%5Cevil.com'],
    ['a relative path with no leading slash', 'iam/orgs'],
    ['an empty string', ''],
    ['whitespace only', '   '],
    ['a leading-whitespace protocol-relative URL', '  //evil.com'],
    ['a tab-prefixed absolute URL', '\thttps://evil.com'],
  ])('rejects %s and falls back', (_l, input) => {
    expect(validateReturnTo(input, FALLBACK)).toBe(FALLBACK);
  });

  it('rejects undefined and falls back', () => {
    expect(validateReturnTo(undefined, FALLBACK)).toBe(FALLBACK);
  });
});
