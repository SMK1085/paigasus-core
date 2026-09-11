// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { ZoneConfigError, ZoneLinkError } from '../../src/zone/errors';
import { assertZoneMap, isAtOrUnder, pathOf, resolveZone, type ZoneMap, type ZoneTarget } from '../../src/zone/resolve';

const ZONES: ZoneMap = { iam: '/iam', gateway: '/gateway' };
const WITH_ROOT: ZoneMap = { console: '', iam: '/iam', gateway: '/gateway' };

/** The message of the error `fn` throws. Fails the test when `fn` does not throw. */
function thrownMessage(fn: () => unknown): string {
  try {
    fn();
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
  throw new Error('expected the call to throw');
}

const MATCHES: ReadonlyArray<readonly [string, string, ZoneMap, ZoneTarget]> = [
  ['the exact base path', '/iam', ZONES, { zone: 'iam', basePath: '/iam', rest: '/' }],
  ['a path under the base path', '/iam/users', ZONES, { zone: 'iam', basePath: '/iam', rest: '/users' }],
  ['another zone', '/gateway/usage', ZONES, { zone: 'gateway', basePath: '/gateway', rest: '/usage' }],
  ['query and fragment kept in rest', '/iam/users?page=2#top', ZONES, { zone: 'iam', basePath: '/iam', rest: '/users?page=2#top' }],
  ['an empty remainder with a query becomes "/?…"', '/iam?tab=1', ZONES, { zone: 'iam', basePath: '/iam', rest: '/?tab=1' }],
  ['an empty remainder with a fragment becomes "/#…"', '/iam#top', ZONES, { zone: 'iam', basePath: '/iam', rest: '/#top' }],
  ['a trailing slash stays in rest', '/iam/users/', ZONES, { zone: 'iam', basePath: '/iam', rest: '/users/' }],
  ['a dot segment inside the QUERY is not a path segment', '/iam/users?next=../x', ZONES, { zone: 'iam', basePath: '/iam', rest: '/users?next=../x' }],
  ['an already percent-encoded path', '/iam/caf%C3%A9', ZONES, { zone: 'iam', basePath: '/iam', rest: '/caf%C3%A9' }],
  ['the root zone as a fallback', '/billing/x', WITH_ROOT, { zone: 'console', basePath: '', rest: '/billing/x' }],
  ['the longest match beats the root zone', '/iam/users', WITH_ROOT, { zone: 'iam', basePath: '/iam', rest: '/users' }],
  ['a segment-edge near miss falls to the root zone', '/iamx/users', WITH_ROOT, { zone: 'console', basePath: '', rest: '/iamx/users' }],
];

const REJECTED: ReadonlyArray<readonly [string, string]> = [
  ['an absolute URL', 'https://evil.example/iam'],
  ['a mailto: URL', 'mailto:someone@example.test'],
  ['a protocol-relative URL', '//evil.example/iam'],
  ['a backslash after the slash', '/\\evil.example'],
  ['a backslash inside the path', '/iam\\users'],
  ['a tab', '/\t/evil.example'],
  ['a newline', '/iam/\nusers'],
  ['a space', '/iam/ users'],
  ['a NUL', '/iam/\u0000'],
  ['DEL', '/iam/\u007f'],
  ['a non-breaking space', '/iam/\u00a0users'],
  ['a relative path', 'users'],
  ['a dot-relative path', './users'],
  ['an empty string', ''],
  ['a dot-dot segment', '/iam/../gateway/x'],
  ['a single-dot segment', '/iam/./users'],
  ['a percent-encoded dot-dot segment', '/iam/%2e%2e/gateway/x'],
  ['a raw non-ASCII path, which the URL parser re-encodes', '/iam/café'],
];

describe('resolveZone', () => {
  it.each(MATCHES)('matches %s', (_label, href, zones, expected) => {
    expect(resolveZone(href, zones)).toEqual(expected);
  });

  it('a segment-edge near miss matches nothing when there is no root zone', () => {
    // Rule 2 (spec § 6.3): "/iamx" is not under "/iam", because the match needs "/iam" + "/".
    expect(resolveZone('/iamx/users', ZONES)).toBeNull();
  });

  it('returns null when no zone matches', () => {
    expect(resolveZone('/billing/x', ZONES)).toBeNull();
  });

  it('reads only OWN keys, so constructor and __proto__ are ordinary zone ids', () => {
    // JSON.parse creates an OWN `__proto__` property, as the operator JSON would.
    const zones = JSON.parse('{"__proto__":"/proto","constructor":"/ctor"}') as ZoneMap;
    expect(resolveZone('/proto/x', zones)).toEqual({ zone: '__proto__', basePath: '/proto', rest: '/x' });
    expect(resolveZone('/ctor', zones)).toEqual({ zone: 'constructor', basePath: '/ctor', rest: '/' });
    // A plain map WITHOUT those keys must not resolve anything through Object.prototype (spec § 6.1).
    expect(resolveZone('/constructor', ZONES)).toBeNull();
  });

  it.each(REJECTED)('rejects %s with ZoneLinkError', (_label, href) => {
    expect(() => resolveZone(href, ZONES)).toThrow(ZoneLinkError);
  });

  it('never echoes the href in the error message, because an href can carry user data', () => {
    const message = thrownMessage(() => resolveZone('/iam/../secret-token-abc', ZONES));
    expect(message).not.toContain('secret-token-abc');
  });
});

describe('pathOf', () => {
  it('cuts at the first ? or #', () => {
    expect(pathOf('/users?page=2#top')).toBe('/users');
    expect(pathOf('/users#top?x')).toBe('/users');
    expect(pathOf('/users')).toBe('/users');
  });
});

describe('assertZoneMap', () => {
  it("returns the current zone's base path", () => {
    expect(assertZoneMap('gateway', ZONES)).toBe('/gateway');
    expect(assertZoneMap('console', WITH_ROOT)).toBe('');
  });

  it('throws when the current zone is not a key of the map', () => {
    expect(() => assertZoneMap('billing', ZONES)).toThrow(ZoneConfigError);
  });

  it("throws for 'constructor' when the map has no OWN key of that name", () => {
    // A plain object INHERITS `constructor` — and getPublicConfig() now returns a plain object (F17).
    expect(() => assertZoneMap('constructor', ZONES)).toThrow(ZoneConfigError);
  });

  it('throws when two zones share a base path, naming both zone ids', () => {
    expect(() => assertZoneMap('iam', { iam: '/iam', legacy: '/iam' })).toThrow(/"iam" and "legacy"/);
    expect(() => assertZoneMap('a', { a: '', b: '' })).toThrow(ZoneConfigError);
  });
});

describe('isAtOrUnder', () => {
  const TABLE: ReadonlyArray<readonly [string, string, boolean]> = [
    ['/iam', '/iam', true],
    ['/iam/x', '/iam', true],
    ['/iamx', '/iam', false],
    ['/anything', '', true],
    ['/', '/', true],
    ['/users', '/', false],
  ];

  it.each(TABLE)('isAtOrUnder(%s, %s) is %s', (pathname, prefix, expected) => {
    expect(isAtOrUnder(pathname, prefix)).toBe(expected);
  });
});
