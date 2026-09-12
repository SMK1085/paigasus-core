// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from 'vitest';
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

  // SMA-511 spec § 7.1: createAuthRouteHandler rebuilds a route handler's request URL on this value.
  it('carries PAIGASUS_PUBLIC_ORIGIN as publicOrigin', async () => {
    const rt = await createAuthRuntime(BASE);
    expect(rt.publicOrigin).toBe('https://app.example.com');
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

// The cache lives on globalThis (runtime.ts), so `vi.resetModules()` alone no longer clears it.
// The key comes from the GLOBAL symbol registry, the same way runtime.ts creates it, so this test
// file needs no export of its own from the source.
const RUNTIME_KEY: unique symbol = Symbol.for('paigasus.auth.runtime');

function clearSharedRuntime(): void {
  delete (globalThis as typeof globalThis & { [RUNTIME_KEY]?: unknown })[RUNTIME_KEY];
}

describe('getAuthRuntime', () => {
  beforeEach(clearSharedRuntime);

  // "One runtime per process": the promise is cached after the first successful call and every
  // later call's arguments are ignored — proven here by passing a config on the SECOND call that
  // would throw if it were actually re-validated (an unknown zone), and observing no throw and
  // reference equality with the first call's result instead.
  it('caches the runtime across calls regardless of later arguments', async () => {
    const rt1 = await getAuthRuntime(BASE);
    const rt2 = await getAuthRuntime({ ...BASE, PAIGASUS_ZONE: 'ghost' });
    expect(rt2).toBe(rt1);
  });

  // SMA-511 Task 22. Next compiles a route handler and a server component into separate module
  // graphs, so ONE process holds two copies of runtime.ts. Two copies that each keep their own
  // runtime also keep their own memory session store, and the login loops forever: the callback
  // route writes the session and the page finds none.
  //
  // `vi.resetModules()` between the two imports gives a SECOND module instance — the closest a
  // test in one process can come to Next's two layers. The two instances must still answer with
  // ONE runtime. Move the cache back to a module-level variable and this test reds, while every
  // other test in this file stays green (MEASURED).
  it('shares one runtime between two module instances, as two Next layers get', async () => {
    vi.resetModules();
    const first = await import('../src/runtime.js');
    vi.resetModules();
    const second = await import('../src/runtime.js');
    expect(second).not.toBe(first);

    expect(await second.getAuthRuntime(BASE)).toBe(await first.getAuthRuntime(BASE));
  });
});

// SMA-626 § 5, guard 3. getAuthRuntime's doc comment claims a misconfigured-at-boot process can
// recover once its config is fixed, and only the SUCCESS path was exercised — delete
// `sharedRuntime = undefined` from the catch and every existing test still passed.
//
// THE VACUOUS MODE THIS AVOIDS: the cache is cleared exactly ONCE, before BOTH calls, so both
// calls reach the same cache entry. Clearing it between them would give the second call an empty
// cache anyway, and the test would pass with the reset in the catch deleted.
//
// MEASURED 2026-09-10: commenting out the cache reset in runtime.ts's catch reds both tests here
// and leaves every other test in this file green. SMA-511 Task 22 moved that cache from a
// module-level variable to globalThis, so each test now clears the global slot rather than calling
// vi.resetModules(), which no longer reaches it.
describe('getAuthRuntime failure reset (SMA-626 § 5, guard 3)', () => {
  it('lets a later call succeed after the first one rejected', async () => {
    clearSharedRuntime(); // ONCE — see the block comment above.
    const mod = await import('../src/runtime.js');

    await expect(mod.getAuthRuntime({ ...BASE, PAIGASUS_ZONE: 'ghost' })).rejects.toMatchObject({
      message: 'PAIGASUS_ZONE has no entry in PAIGASUS_ZONES',
    });

    // The SAME module instance. Without the reset in the catch, this replays the rejection above.
    const recovered = await mod.getAuthRuntime(BASE);
    expect(recovered.zone).toBe(BASE.PAIGASUS_ZONE);
  });

  it('caches the recovered runtime, so the reset does not disable memoisation', async () => {
    clearSharedRuntime();
    const mod = await import('../src/runtime.js');

    await expect(mod.getAuthRuntime({ ...BASE, PAIGASUS_ZONE: 'ghost' })).rejects.toThrow();
    const first = await mod.getAuthRuntime(BASE);
    const second = await mod.getAuthRuntime({ ...BASE, PAIGASUS_ZONE: 'ghost' });

    expect(second).toBe(first);
  });
});
