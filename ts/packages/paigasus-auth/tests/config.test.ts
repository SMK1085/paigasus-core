// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { z } from 'zod';
import { authEnvShape } from '../src/config.js';

const schema = z.object(authEnvShape);

const VALID = {
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.com/realms/paigasus',
  PAIGASUS_OIDC_CLIENT_ID: 'iam-console',
  PAIGASUS_OIDC_CLIENT_SECRET: 's3cret',
  PAIGASUS_PUBLIC_ORIGIN: 'https://app.example.com',
  PAIGASUS_SESSION_STORE: 'redis',
  PAIGASUS_SESSION_REDIS_URL: 'redis://localhost:6379/0',
};

describe('authEnvShape', () => {
  it('accepts a complete valid environment', () => {
    expect(schema.parse(VALID).PAIGASUS_OIDC_CLIENT_ID).toBe('iam-console');
  });

  it('applies every documented default', () => {
    const parsed = schema.parse(VALID);
    expect(parsed.PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS).toBe(30);
    expect(parsed.PAIGASUS_OIDC_HTTP_TIMEOUT_MS).toBe(3500);
    expect(parsed.PAIGASUS_SESSION_REDIS_TIMEOUT_MS).toBe(1000);
    expect(parsed.PAIGASUS_SESSION_TTL_SECONDS).toBe(28800);
    expect(parsed.PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS).toBe(86400);
    expect(parsed.PAIGASUS_SESSION_REFRESH_SKEW_SECONDS).toBe(30);
    expect(parsed.PAIGASUS_SESSION_LOCK_TTL_MS).toBe(10000);
    expect(parsed.PAIGASUS_SESSION_LOCK_WAIT_MS).toBe(3000);
  });

  it('rejects a non-https issuer', () => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_OIDC_ISSUER: 'http://idp.example.com' })).toThrow();
  });

  it('rejects an unknown store backend', () => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_SESSION_STORE: 'postgres' })).toThrow();
  });

  it('rejects an origin with a trailing slash', () => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_PUBLIC_ORIGIN: 'https://app.example.com/' })).toThrow();
  });

  it('does NOT reject a missing redis url — that is a cross-field rule owned by createAuthRuntime', () => {
    // This key exists only to drop it from withoutUrl below, so the binding is intentionally unused.
    // eslint-disable-next-line @typescript-eslint/no-unused-vars -- rest-sibling destructuring
    const { PAIGASUS_SESSION_REDIS_URL: _omitted, ...withoutUrl } = VALID;
    expect(() => schema.parse(withoutUrl)).not.toThrow();
  });

  it('declares neither zone key — defineRuntimeConfig throws if an extra shape does', () => {
    expect(Object.keys(authEnvShape)).not.toContain('PAIGASUS_ZONE');
    expect(Object.keys(authEnvShape)).not.toContain('PAIGASUS_ZONES');
  });

  // Review round 1, smaller fix: these were a bare z.string().optional(), so a malformed
  // override was accepted in silence and only surfaced later as an opaque openid-client failure.
  it('rejects a malformed redirect URI override', () => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_OIDC_REDIRECT_URI: 'not a url' })).toThrow();
  });

  it('accepts a well-formed redirect URI override', () => {
    const parsed = schema.parse({ ...VALID, PAIGASUS_OIDC_REDIRECT_URI: 'https://proxy.example.com/cb' });
    expect(parsed.PAIGASUS_OIDC_REDIRECT_URI).toBe('https://proxy.example.com/cb');
  });

  it('rejects a malformed post-logout redirect URI override', () => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_OIDC_POST_LOGOUT_REDIRECT_URI: 'not a url' })).toThrow();
  });

  // SMA-651 D9. Node turns a timer delay above 2^31 - 1 ms into 1 ms, and the largest timer the
  // session store sets is 4 x this value.
  it('caps PAIGASUS_SESSION_REDIS_TIMEOUT_MS at 536870911 ms', () => {
    expect(schema.parse({ ...VALID, PAIGASUS_SESSION_REDIS_TIMEOUT_MS: '536870911' }).PAIGASUS_SESSION_REDIS_TIMEOUT_MS).toBe(536_870_911);
    expect(() => schema.parse({ ...VALID, PAIGASUS_SESSION_REDIS_TIMEOUT_MS: '536870912' })).toThrow();
  });

  // SMA-692 D3-a. No default here: createAuthRuntime applies the default list for the
  // authorization request only, so "absent" must stay visible after the parse.
  it('gives PAIGASUS_OIDC_SCOPES no default', () => {
    expect(schema.parse(VALID).PAIGASUS_OIDC_SCOPES).toBeUndefined();
  });

  // SMA-692 D5. A token-exact match on whitespace-separated tokens.
  it.each(['openid', 'openid profile email offline_access', 'profile email openid', 'openid api://paigasus-api/access'])('accepts PAIGASUS_OIDC_SCOPES %j, which holds the token openid', (value) => {
    expect(schema.parse({ ...VALID, PAIGASUS_OIDC_SCOPES: value }).PAIGASUS_OIDC_SCOPES).toBe(value);
  });

  it.each(['profile email', 'openidx profile', 'profile openid-connect', 'OPENID profile', '', '   '])('refuses PAIGASUS_OIDC_SCOPES %j, which does not hold the token openid', (value) => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_OIDC_SCOPES: value })).toThrow();
  });

  // Final fix I1. RFC 6749 § 3.3 allows only a single space between scope tokens. TS and the
  // chart now normalize on the SAME explicit class ([\t\n\f\r ]), not `\s`, so a value that
  // reaches the render agrees with the value that reached the parse. Each row asserts the exact
  // normalized string, not the raw input.
  it.each([
    ['openid\tprofile', 'openid profile'],
    ['openid profile\n', 'openid profile'],
    [' openid profile ', 'openid profile'],
    ['openid   profile', 'openid profile'],
    ['profile\topenid', 'profile openid'],
    ['openid\r\nprofile\femail', 'openid profile email'],
  ])('normalizes PAIGASUS_OIDC_SCOPES %j to %j', (value, normalized) => {
    expect(schema.parse({ ...VALID, PAIGASUS_OIDC_SCOPES: value }).PAIGASUS_OIDC_SCOPES).toBe(normalized);
  });

  // NBSP (U+00A0) is not in the explicit separator class, so the two words stay ONE token and
  // that token is not `openid` — unlike the old `\s`-based split, which would have accepted it.
  it('refuses PAIGASUS_OIDC_SCOPES holding openid\\u00a0profile — NBSP is not a separator', () => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_OIDC_SCOPES: 'openid profile' })).toThrow();
  });

  // A value that normalizes to empty (only separator characters) is refused, distinctly from the
  // openid check — it never has a token to test.
  it.each(['\t', '\n', '  \t \n  '])('refuses PAIGASUS_OIDC_SCOPES %j, which normalizes to empty', (value) => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_OIDC_SCOPES: value })).toThrow();
  });

  // SMA-692 D1.
  it('leaves PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE undefined when absent', () => {
    expect(schema.parse(VALID).PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE).toBeUndefined();
  });

  it('accepts a set PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE', () => {
    expect(schema.parse({ ...VALID, PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE: 'https://api.example.com' }).PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE).toBe('https://api.example.com');
  });

  it.each(['', ' https://api.example.com', 'https://api.example.com ', '\thttps://api.example.com'])('refuses PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE %j', (value) => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE: value })).toThrow();
  });
});
