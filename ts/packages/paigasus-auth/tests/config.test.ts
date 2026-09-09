// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { z } from 'zod';
import { authEnvShape } from '../src/config.js';

const schema = z.object(authEnvShape);

const VALID = {
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.com/realms/paigasus',
  PAIGASUS_OIDC_CLIENT_ID: 'paigasus-console',
  PAIGASUS_OIDC_CLIENT_SECRET: 's3cret',
  PAIGASUS_PUBLIC_ORIGIN: 'https://app.example.com',
  PAIGASUS_SESSION_STORE: 'redis',
  PAIGASUS_SESSION_REDIS_URL: 'redis://localhost:6379/0',
};

describe('authEnvShape', () => {
  it('accepts a complete valid environment', () => {
    expect(schema.parse(VALID).PAIGASUS_OIDC_CLIENT_ID).toBe('paigasus-console');
  });

  it('applies every documented default', () => {
    const parsed = schema.parse(VALID);
    expect(parsed.PAIGASUS_OIDC_SCOPES).toBe('openid profile email offline_access');
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
});
