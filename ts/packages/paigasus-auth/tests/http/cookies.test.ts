// SPDX-License-Identifier: Apache-2.0
//
// The __Host- prefix is the point of this file. Host-only (no `Domain`) constrains what THIS
// server writes; it does nothing about a sibling `*.example.com` host writing
// `pgs_sid=...; Domain=example.com`, which arrives as a second value in the same `Cookie` header
// and most parsers take first. On a multi-zone console sharing one origin a sibling host is
// normal, not hypothetical. Browsers refuse a `Domain`-scoped write of a `__Host-`-prefixed name.
import { describe, expect, it } from 'vitest';
import { SESSION_COOKIE, clearCookie, readCookies, serializeCookie, txnCookieName } from '../../src/http/cookies.js';

describe('cookie names', () => {
  it('SESSION_COOKIE is exactly __Host-pgs_sid', () => {
    expect(SESSION_COOKIE).toBe('__Host-pgs_sid');
  });

  it('txnCookieName derives a per-transaction __Host- prefixed name', () => {
    expect(txnCookieName('abc')).toBe('__Host-pgs_txn_abc');
  });
});

describe('serializeCookie', () => {
  it('sets HttpOnly, Secure, SameSite=Lax and Path=/', () => {
    const cookie = serializeCookie(SESSION_COOKIE, 'sid-value');
    // Exact attribute match, not a substring (review round 1): `toContain('Path=/')` would pass
    // on `Path=/iam` too — a value a browser REFUSES for a `__Host-`-prefixed cookie.
    const attributes = cookie.split('; ');
    expect(attributes).toContain('HttpOnly');
    expect(attributes).toContain('Secure');
    expect(attributes).toContain('SameSite=Lax');
    expect(attributes).toContain('Path=/');
  });

  it('never carries a Domain attribute', () => {
    const cookie = serializeCookie(SESSION_COOKIE, 'sid-value');
    expect(cookie).not.toMatch(/domain=/i);
  });

  it('the session cookie carries no Max-Age and no Expires — it is a browser-session cookie', () => {
    const cookie = serializeCookie(SESSION_COOKIE, 'sid-value');
    expect(cookie).not.toMatch(/max-age=/i);
    expect(cookie).not.toMatch(/expires=/i);
  });

  it('a supplied maxAgeSeconds is rendered as Max-Age', () => {
    const cookie = serializeCookie(txnCookieName('abc'), 'secret-value', { maxAgeSeconds: 600 });
    expect(cookie).toContain('Max-Age=600');
  });
});

describe('clearCookie', () => {
  it('emits Max-Age=0', () => {
    expect(clearCookie(SESSION_COOKIE)).toMatch(/Max-Age=0/);
  });

  it('still carries the security attributes', () => {
    const cookie = clearCookie(SESSION_COOKIE);
    const attributes = cookie.split('; ');
    expect(attributes).toContain('HttpOnly');
    expect(attributes).toContain('Secure');
    expect(attributes).toContain('SameSite=Lax');
    expect(attributes).toContain('Path=/');
  });
});

describe('readCookies', () => {
  it('parses multiple cookies and returns the value for a name', () => {
    const cookies = readCookies('a=1; __Host-pgs_sid=abc123; b=2');
    expect(cookies.get('__Host-pgs_sid')).toBe('abc123');
    expect(cookies.get('a')).toBe('1');
    expect(cookies.get('b')).toBe('2');
  });

  it('returns an empty map for a null or undefined header', () => {
    expect(readCookies(null).size).toBe(0);
    expect(readCookies(undefined).size).toBe(0);
  });

  it('returns an empty map for an empty header', () => {
    expect(readCookies('').size).toBe(0);
  });

  // Review round 1, M6: a duplicate name is exactly the "cookie tossing" hazard the file header
  // describes — a sibling host writing a second, competing value under the same name. Guessing
  // which of the two is legitimate (by keeping either the first or the last) is unsafe, so both
  // occurrences are dropped and the name reads as absent.
  it('treats a duplicate cookie name as absent, not as the first or the last value', () => {
    const cookies = readCookies('a=1; dup=legit; dup=tossed; b=2');
    expect(cookies.has('dup')).toBe(false);
    expect(cookies.get('a')).toBe('1');
    expect(cookies.get('b')).toBe('2');
  });

  it('drops a name duplicated three or more times the same way', () => {
    const cookies = readCookies('dup=1; dup=2; dup=3');
    expect(cookies.has('dup')).toBe(false);
  });
});
