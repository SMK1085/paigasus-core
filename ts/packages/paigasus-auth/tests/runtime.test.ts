// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { createAuthRuntime, getAuthRuntime } from '../src/runtime.js';
import { AuthConfigError } from '../src/core/errors.js';

// PAIGASUS_SESSION_LOCK_TTL_MS is 10000, not the package default's original 5000 (review round 1,
// Important 2): a refresh now makes two sequential bounded calls under the lock (token endpoint,
// then a JWKS fetch for the non-repudiation check), so the invariant below is `2 * httpTimeoutMs
// < lockTtlMs` — 2 * 3500 = 7000, which needs a lock TTL above 7000, not 5000.
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
  PAIGASUS_SESSION_LOCK_TTL_MS: 10000,
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

  // Invariant 3's control, at the NEW 2x boundary (review round 1, Important 2): a refresh makes
  // two sequential bounded calls under the lock, so the bound is 2 * httpTimeoutMs < lockTtlMs,
  // not 1x. 2 * 5000 = 10000, which is AT the lock TTL below — "at or above" still rejects.
  it('refuses an http timeout whose double is at or above the lock ttl', async () => {
    await expect(createAuthRuntime({ ...BASE, PAIGASUS_OIDC_HTTP_TIMEOUT_MS: 5000, PAIGASUS_SESSION_LOCK_TTL_MS: 10000 })).rejects.toBeInstanceOf(AuthConfigError);
  });

  // The same lock TTL, one ms of httpTimeoutMs under the boundary: 2 * 4999 = 9998 < 10000.
  it('allows an http timeout whose double is just below the lock ttl', async () => {
    await expect(createAuthRuntime({ ...BASE, PAIGASUS_OIDC_HTTP_TIMEOUT_MS: 4999, PAIGASUS_SESSION_LOCK_TTL_MS: 10000 })).resolves.toBeDefined();
  });

  it('refuses a zone with no entry in the zone map', async () => {
    await expect(createAuthRuntime({ ...BASE, PAIGASUS_ZONE: 'ghost' })).rejects.toBeInstanceOf(AuthConfigError);
  });

  it('accepts injected dependencies', async () => {
    const logger = { event: () => undefined };
    expect((await createAuthRuntime(BASE, { logger })).logger).toBe(logger);
  });

  // Smaller fix: PAIGASUS_OIDC_SCOPES was parsed and then unreachable — task 8 needs it to build
  // the authorization URL.
  it('exposes the configured scopes', async () => {
    const rt = await createAuthRuntime({ ...BASE, PAIGASUS_OIDC_SCOPES: 'openid email' });
    expect(rt.scopes).toBe('openid email');
  });

  // Review round 1, "Add a test asserting an AuthConfigError message renders no value": every
  // prior test here uses toBeInstanceOf alone, so a future edit that interpolated a raw config
  // value (e.g. the Redis DSN) into a message would still pass. Asserting the EXACT, hand-authored
  // string is the strongest form of "renders no value" — any interpolation changes the string.
  it('every AuthConfigError carries its fixed, value-free message', async () => {
    await expect(createAuthRuntime({ ...BASE, PAIGASUS_SESSION_STORE: 'redis' })).rejects.toMatchObject({
      message: 'PAIGASUS_SESSION_REDIS_URL is required when PAIGASUS_SESSION_STORE is "redis"',
    });
    await expect(createAuthRuntime({ ...BASE, PAIGASUS_ZONES: { iam: '/iam', gateway: '/gateway' } })).rejects.toMatchObject({
      message: 'PAIGASUS_SESSION_STORE cannot be "memory" when PAIGASUS_ZONES declares more than one zone',
    });
    await expect(createAuthRuntime({ ...BASE, PAIGASUS_OIDC_HTTP_TIMEOUT_MS: 5000, PAIGASUS_SESSION_LOCK_TTL_MS: 10000 })).rejects.toMatchObject({
      message: '2x PAIGASUS_OIDC_HTTP_TIMEOUT_MS must be strictly below PAIGASUS_SESSION_LOCK_TTL_MS',
    });
    await expect(createAuthRuntime({ ...BASE, PAIGASUS_ZONE: 'ghost' })).rejects.toMatchObject({
      message: 'PAIGASUS_ZONE has no entry in PAIGASUS_ZONES',
    });
  });
});

describe('getAuthRuntime', () => {
  // "One runtime per process": the promise is cached after the first successful call and every
  // later call's arguments are ignored — proven here by passing a config on the SECOND call that
  // would throw if it were actually re-validated (an unknown zone), and observing no throw and
  // reference equality with the first call's result instead.
  it('caches the runtime across calls regardless of later arguments', async () => {
    const rt1 = await getAuthRuntime(BASE);
    const rt2 = await getAuthRuntime({ ...BASE, PAIGASUS_ZONE: 'ghost' });
    expect(rt2).toBe(rt1);
  });
});
