// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { createAuthRuntime } from '../src/runtime.js';
import { AuthConfigError } from '../src/core/errors.js';

const BASE = {
  PAIGASUS_ZONE: 'iam',
  PAIGASUS_ZONES: { iam: '/iam' },
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.com',
  PAIGASUS_OIDC_CLIENT_ID: 'c',
  PAIGASUS_OIDC_CLIENT_SECRET: 's',
  PAIGASUS_PUBLIC_ORIGIN: 'https://app.example.com',
  PAIGASUS_OIDC_SCOPES: 'openid profile email offline_access',
  PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS: 30,
  PAIGASUS_OIDC_HTTP_TIMEOUT_MS: 3500,
  PAIGASUS_SESSION_STORE: 'memory' as const,
  PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 1000,
  PAIGASUS_SESSION_TTL_SECONDS: 28800,
  PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS: 86400,
  PAIGASUS_SESSION_REFRESH_SKEW_SECONDS: 30,
  PAIGASUS_SESSION_LOCK_TTL_MS: 5000,
  PAIGASUS_SESSION_LOCK_WAIT_MS: 3000,
};

describe('createAuthRuntime', () => {
  it('derives the redirect URI from the origin and the zone base path', async () => {
    const rt = await createAuthRuntime(BASE);
    expect(rt.redirectUri).toBe('https://app.example.com/iam/auth/callback');
    expect(rt.postLogoutRedirectUri).toBe('https://app.example.com/iam/');
  });

  it('honours an explicit redirect URI override', async () => {
    const rt = await createAuthRuntime({ ...BASE, PAIGASUS_OIDC_REDIRECT_URI: 'https://proxy/cb' });
    expect(rt.redirectUri).toBe('https://proxy/cb');
  });

  // The cross-field rule that a flat ZodRawShape cannot express.
  it('requires a redis url when the store is redis', async () => {
    await expect(createAuthRuntime({ ...BASE, PAIGASUS_SESSION_STORE: 'redis' })).rejects.toBeInstanceOf(AuthConfigError);
  });

  // memory is single-process: under multi-zone the zones share nothing and AC 2 does not hold.
  it('refuses the memory store when more than one zone is declared', async () => {
    await expect(createAuthRuntime({ ...BASE, PAIGASUS_ZONES: { iam: '/iam', gateway: '/gateway' } })).rejects.toBeInstanceOf(AuthConfigError);
  });

  it('allows the memory store for a single zone', async () => {
    await expect(createAuthRuntime(BASE)).resolves.toBeDefined();
  });

  // Invariant 5's second control.
  it('refuses an http timeout at or above the lock ttl', async () => {
    await expect(createAuthRuntime({ ...BASE, PAIGASUS_OIDC_HTTP_TIMEOUT_MS: 5000, PAIGASUS_SESSION_LOCK_TTL_MS: 5000 })).rejects.toBeInstanceOf(AuthConfigError);
  });

  it('refuses a zone with no entry in the zone map', async () => {
    await expect(createAuthRuntime({ ...BASE, PAIGASUS_ZONE: 'ghost' })).rejects.toBeInstanceOf(AuthConfigError);
  });

  it('accepts injected dependencies', async () => {
    const logger = { event: () => undefined };
    expect((await createAuthRuntime(BASE, { logger })).logger).toBe(logger);
  });
});
