// SPDX-License-Identifier: Apache-2.0
//
// design doc § 10.1's stale-cookie recovery: getSession() never redirects, requireSession() does.
// `next/headers` and `next/navigation` are mocked — this suite runs in plain Node, with no Next
// request context to read cookies from or to redirect within.
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { cookiesMock, redirectMock } = vi.hoisted(() => ({
  cookiesMock: vi.fn(),
  // Mirrors Next's own redirect(): it never returns, it throws a sentinel the framework's
  // rendering pipeline catches. Asserting on the thrown message is how these tests observe both
  // WHETHER redirect() was called and with WHAT url, from a single `rejects.toThrow` assertion.
  redirectMock: vi.fn((url: string) => {
    throw new Error(`NEXT_REDIRECT:${url}`);
  }),
}));

vi.mock('next/headers', () => ({ cookies: cookiesMock }));
vi.mock('next/navigation', () => ({ redirect: redirectMock }));

import { getSession, requireSession } from '../../src/next/get-session.js';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { noopLogger } from '../../src/adapters/noop-logger.js';
import { SessionStoreUnavailable } from '../../src/core/errors.js';
import type { SessionRecord } from '../../src/core/session.js';
import { SESSION_COOKIE } from '../../src/http/cookies.js';
import type { SessionStore } from '../../src/ports/session-store.js';
import type { AuthRuntime } from '../../src/runtime.js';

/** A cookie jar shaped like Next's `ReadonlyRequestCookies` — only the `.get` method is used. */
function cookieJar(sid?: string): { get(name: string): { name: string; value: string } | undefined } {
  return {
    get: (name: string) => (name === SESSION_COOKIE && sid !== undefined ? { name, value: sid } : undefined),
  };
}

/** Every method throws: getSession never needs to call the OIDC client or the resolver in this
 * suite (the test records are never near their skew window), so a call here is a defect. */
function unusedOidc(): AuthRuntime['oidc'] {
  const fail = () => Promise.reject(new Error('unexpectedly called'));
  return { buildAuthorizationUrl: fail, authorizationCodeGrant: fail, refresh: fail, revoke: fail, buildEndSessionUrl: fail };
}

function baseRuntime(store: SessionStore): AuthRuntime {
  return {
    store,
    resolver: { resolve: () => Promise.reject(new Error('unexpectedly called')) },
    logger: noopLogger,
    oidc: unusedOidc(),
    redirectUri: 'https://app.example.com/iam/auth/callback',
    postLogoutRedirectUri: 'https://app.example.com/iam/',
    cookieDomainless: true,
    skewMs: 30_000,
    lockTtlMs: 10_000,
    lockWaitMs: 3_000,
    ttlMs: 28_800_000,
    absoluteTtlMs: 86_400_000,
    zone: 'iam',
    basePath: '/iam',
    scopes: 'openid profile email offline_access',
  };
}

function liveRecord(): SessionRecord {
  const now = Date.now();
  return {
    version: 1,
    rev: 0,
    accessToken: 'AT-live',
    accessExpiresAt: now + 999_000,
    absoluteExpiresAt: now + 999_000,
    idTokenClaims: { iss: 'https://idp.example.com', sub: 'u1' },
    principal: { principalPrn: null, issuer: 'https://idp.example.com', subject: 'u1', memberships: [], roleGrants: [], grantsAvailable: false },
  };
}

beforeEach(() => {
  cookiesMock.mockReset();
  redirectMock.mockClear();
});

describe('getSession', () => {
  it('returns null when no cookie is present, and never redirects', async () => {
    cookiesMock.mockResolvedValue(cookieJar());
    const runtime = baseRuntime(new MemorySessionStore());

    await expect(getSession(runtime)).resolves.toBeNull();
    expect(redirectMock).not.toHaveBeenCalled();
  });

  it('returns null when the cookie names a deleted record', async () => {
    // A fresh, empty store: the cookie names a sid nothing was ever written under — the same
    // shape as a record that WAS written and has since been deleted (absolute expiry, a logout
    // in another tab, a version bump). resolveSession treats both identically.
    cookiesMock.mockResolvedValue(cookieJar('ghost-sid'));
    const runtime = baseRuntime(new MemorySessionStore());

    await expect(getSession(runtime)).resolves.toBeNull();
    expect(redirectMock).not.toHaveBeenCalled();
  });

  it('returns null, not a throw, when the store is unavailable', async () => {
    cookiesMock.mockResolvedValue(cookieJar('some-sid'));
    const unavailableStore: SessionStore = {
      get: () => Promise.reject(new SessionStoreUnavailable('session store unavailable (redis://<redacted>)')),
      set: () => Promise.resolve(false),
      delete: () => Promise.resolve(),
      tryAcquireLock: () => Promise.resolve(false),
      releaseLock: () => Promise.resolve(),
      putTransaction: () => Promise.resolve(),
      takeTransaction: () => Promise.resolve(null),
      close: () => Promise.resolve(),
    };
    const runtime = baseRuntime(unavailableStore);

    await expect(getSession(runtime)).resolves.toBeNull();
    expect(redirectMock).not.toHaveBeenCalled();
  });
});

describe('requireSession', () => {
  it("calls next/navigation's redirect() to the login path with returnTo when getSession() is null", async () => {
    cookiesMock.mockResolvedValue(cookieJar());
    const runtime = baseRuntime(new MemorySessionStore());

    await expect(requireSession(runtime, { returnTo: '/iam/dashboard' })).rejects.toThrow('NEXT_REDIRECT:/iam/auth/login?returnTo=%2Fiam%2Fdashboard');
    expect(redirectMock).toHaveBeenCalledWith('/iam/auth/login?returnTo=%2Fiam%2Fdashboard');
  });

  it('falls back to the zone root when returnTo is not a valid same-origin path', async () => {
    cookiesMock.mockResolvedValue(cookieJar());
    const runtime = baseRuntime(new MemorySessionStore());

    await expect(requireSession(runtime, { returnTo: 'https://evil.example.com' })).rejects.toThrow('NEXT_REDIRECT:/iam/auth/login?returnTo=%2Fiam%2F');
  });

  it('returns the record when a session exists, without redirecting', async () => {
    const store = new MemorySessionStore();
    const record = liveRecord();
    await store.set('sid-live', record, 999_000, null);
    cookiesMock.mockResolvedValue(cookieJar('sid-live'));
    const runtime = baseRuntime(store);

    await expect(requireSession(runtime)).resolves.toMatchObject({ accessToken: 'AT-live' });
    expect(redirectMock).not.toHaveBeenCalled();
  });
});
