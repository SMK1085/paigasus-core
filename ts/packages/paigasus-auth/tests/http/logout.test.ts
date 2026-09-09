// SPDX-License-Identifier: Apache-2.0
//
// POST /auth/logout and GET /auth/logout/callback — AC 3 ("logout revokes server-side — a stolen
// cookie is dead immediately after"), design doc § 9.5.
//
// ORDER IS THE ACCEPTANCE CRITERION. Step 1 (delete the record) happens before ANY network call,
// so a slow or unreachable identity provider can never leave a live session behind. Checking only
// the OUTCOME ("the record is gone at the end") would pass an implementation that revokes first
// and deletes afterwards — exactly the ordering this design rejects — so `orderedOidc` and
// `orderedStore` below record the SEQUENCE of calls and the tests assert on it directly.
//
// NO CSRF TOKEN. `SameSite=Lax` already withholds __Host-pgs_sid from a cross-site form POST, so a
// forged POST arrives with no session and does nothing — asserted here so the reasoning is
// recorded rather than assumed (matching the omission comment in routes.ts's dispatcher).
//
// `id_token_hint` IS DELIBERATELY OMITTED, not a known gap left to close later (final fix wave,
// finding 7: this comment previously called it a "KNOWN GAP" — superseded). `SessionRecord`
// stores only decoded `idTokenClaims`, never the raw ID token JWT the parameter needs, and
// `OidcTokens` (adapters/oidc.ts) does not surface it either — storing the raw token would add a
// THIRD bearer credential to `SessionRecord` for no benefit to either provider this design
// targets (design doc § 9.5, routes.ts's own "NAMED RESIDUAL" comment on `handleLogout`).
// M12 (docs/superpowers/specs/2026-09-09-sma-506-measurements.md) measured Keycloak 26.4 on the
// wire: it completes the end-session redirect immediately on `client_id` alone, with no
// `id_token_hint` and no confirmation interstitial — `openid-client@6.8.8` appends `client_id`
// unconditionally when the caller supplies none. The test below for the end-session redirect
// therefore asserts `post_logout_redirect_uri` and `state`, and explicitly that `id_token_hint`
// is absent — a positive assertion of the design, not a placeholder for missing coverage.
import { beforeEach, describe, expect, it } from 'vitest';
import { claimsPrincipalResolver } from '../../src/adapters/claims-resolver.js';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import type { AuthorizationRequest, BuildEndSessionUrlParams, OidcClient, OidcTokens, RefreshedTokens } from '../../src/adapters/oidc.js';
import { resolveSession } from '../../src/core/single-flight.js';
import type { SessionRecord } from '../../src/core/session.js';
import { SESSION_COOKIE, txnCookieName } from '../../src/http/cookies.js';
import { createAuthRoutes } from '../../src/http/routes.js';
import type { AuthEventFields, AuthEventName, AuthLogger } from '../../src/ports/logger.js';
import type { ResolvedPrincipal } from '../../src/ports/principal-resolver.js';
import type { SessionStore } from '../../src/ports/session-store.js';
import type { AuthRuntime } from '../../src/runtime.js';

const ZONE_BASE_PATH = '/iam';
const LOGOUT_URL = 'https://rp.example.com/iam/auth/logout';
const LOGOUT_CALLBACK_URL = 'https://rp.example.com/iam/auth/logout/callback';
const POST_LOGOUT_REDIRECT_URI = 'https://rp.example.com/iam/';
const END_SESSION_URL = 'https://issuer.example.com/logout';

const fakePrincipal: ResolvedPrincipal = {
  principalPrn: null,
  issuer: 'https://issuer.example.com',
  subject: 'a-subject',
  memberships: [],
  roleGrants: [],
  grantsAvailable: false,
};

function seededRecord(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    version: 1,
    rev: 0,
    accessToken: 'an-access-token',
    refreshToken: 'a-refresh-token',
    accessExpiresAt: Date.now() + 60_000,
    absoluteExpiresAt: Date.now() + 60_000,
    idTokenClaims: { iss: 'https://issuer.example.com', sub: 'a-subject' },
    principal: fakePrincipal,
    ...overrides,
  };
}

/**
 * A minimal fake OidcClient. `revoke` and `buildEndSessionUrl` are the only two methods logout
 * ever reaches; the other three throw if called, which would fail any test that mistakenly
 * exercises the login/callback/refresh paths through this fake.
 */
function fakeOidc(opts: { revokeImpl?: (token: string) => Promise<void>; endSessionUrl?: string; onCall?: (name: string) => void } = {}): OidcClient & {
  buildEndSessionUrlCalls: BuildEndSessionUrlParams[];
  revokeCalls: string[];
} {
  const buildEndSessionUrlCalls: BuildEndSessionUrlParams[] = [];
  const revokeCalls: string[] = [];
  return {
    buildEndSessionUrlCalls,
    revokeCalls,
    buildAuthorizationUrl(): Promise<AuthorizationRequest> {
      throw new Error('not used in logout tests');
    },
    authorizationCodeGrant(): Promise<OidcTokens> {
      throw new Error('not used in logout tests');
    },
    refresh(): Promise<RefreshedTokens> {
      throw new Error('not used in logout tests');
    },
    async revoke(token: string): Promise<void> {
      opts.onCall?.('oidc.revoke');
      revokeCalls.push(token);
      if (opts.revokeImpl !== undefined) await opts.revokeImpl(token);
    },
    buildEndSessionUrl(params: BuildEndSessionUrlParams): Promise<string> {
      buildEndSessionUrlCalls.push(params);
      return Promise.resolve(opts.endSessionUrl ?? END_SESSION_URL);
    },
  };
}

/** Wraps a real store, recording every `delete` call's start — used to prove ORDER, not outcome. */
function orderedStore(inner: SessionStore, calls: string[]): SessionStore {
  return {
    get: (sid) => inner.get(sid),
    set: (sid, rec, ttlMs, expectedRev) => inner.set(sid, rec, ttlMs, expectedRev),
    delete: (sid) => {
      calls.push('store.delete');
      return inner.delete(sid);
    },
    tryAcquireLock: (sid, token, ttlMs) => inner.tryAcquireLock(sid, token, ttlMs),
    releaseLock: (sid, token) => inner.releaseLock(sid, token),
    putTransaction: (txnId, tx, ttlMs) => inner.putTransaction(txnId, tx, ttlMs),
    takeTransaction: (txnId) => inner.takeTransaction(txnId),
    close: () => inner.close(),
  };
}

let store: MemorySessionStore;
let events: Array<[AuthEventName, AuthEventFields]>;
let runtime: AuthRuntime;

function recordingLogger(): AuthLogger {
  return { event: (name, fields) => void events.push([name, { ...fields }]) };
}

function baseRuntime(oidc: OidcClient): AuthRuntime {
  return {
    store,
    resolver: claimsPrincipalResolver,
    logger: recordingLogger(),
    oidc,
    redirectUri: 'https://rp.example.com/iam/auth/callback',
    postLogoutRedirectUri: POST_LOGOUT_REDIRECT_URI,
    cookieDomainless: true,
    skewMs: 30_000,
    lockTtlMs: 10_000,
    lockWaitMs: 3_000,
    ttlMs: 28_800_000,
    absoluteTtlMs: 86_400_000,
    zone: 'iam',
    basePath: ZONE_BASE_PATH,
    scopes: 'openid profile email',
  };
}

beforeEach(() => {
  store = new MemorySessionStore();
  events = [];
});

function logoutRequest(cookieHeader?: string): Request {
  return cookieHeader !== undefined ? new Request(LOGOUT_URL, { method: 'POST', headers: { cookie: cookieHeader } }) : new Request(LOGOUT_URL, { method: 'POST' });
}

function logoutCallbackRequest(search = ''): Request {
  return new Request(`${LOGOUT_CALLBACK_URL}${search}`);
}

describe('route dispatch', () => {
  it('GET /auth/logout returns 405 — a GET logout is forgeable by an <img src> on any page', async () => {
    runtime = baseRuntime(fakeOidc());
    const res = await createAuthRoutes(runtime).handle(new Request(LOGOUT_URL, { method: 'GET' }));
    expect(res.status).toBe(405);
  });

  it('a non-GET /auth/logout/callback returns 405', async () => {
    runtime = baseRuntime(fakeOidc());
    const res = await createAuthRoutes(runtime).handle(new Request(LOGOUT_CALLBACK_URL, { method: 'POST' }));
    expect(res.status).toBe(405);
  });
});

describe('POST /auth/logout — delete-first ordering (AC 3)', () => {
  it('deletes the record before any network call: the store is empty even when revocation rejects', async () => {
    const sid = 'sid-under-test';
    await store.set(sid, seededRecord(), 60_000, null);

    const oidc = fakeOidc({ revokeImpl: () => Promise.reject(new Error('idp unreachable')) });
    runtime = baseRuntime(oidc);

    const res = await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));

    expect(res.status).toBe(302);
    expect(await store.get(sid)).toBeNull();
    // The revocation call really was attempted (and really did reject) — this is not passing
    // vacuously because revoke was never called at all.
    expect(oidc.revokeCalls).toEqual(['a-refresh-token']);
  });

  it('a failing revocation call does not fail the logout: the response is still a 302 redirect', async () => {
    const sid = 'sid-2';
    await store.set(sid, seededRecord(), 60_000, null);
    const oidc = fakeOidc({ revokeImpl: () => Promise.reject(new Error('idp unreachable')) });
    runtime = baseRuntime(oidc);

    const res = await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));
    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toBeTruthy();
  });

  // MUTATION-PROOF: asserting only the final state (record gone) also passes an implementation
  // that revokes FIRST and deletes AFTERWARDS, as long as revoke doesn't throw before the delete
  // runs. Recording the call SEQUENCE and asserting delete precedes revoke is what actually pins
  // the ordering design doc § 9.5 requires.
  it('calls store.delete strictly before oidc.revoke', async () => {
    const sid = 'sid-order';
    await store.set(sid, seededRecord(), 60_000, null);

    const calls: string[] = [];
    const oidc = fakeOidc({ onCall: (name) => calls.push(name) });
    runtime = baseRuntime(oidc);
    runtime.store = orderedStore(store, calls);

    const res = await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));

    expect(res.status).toBe(302);
    expect(calls).toEqual(['store.delete', 'oidc.revoke']);
  });

  it("after logout, resolveSession with the old sid returns null (AC 3's actual claim)", async () => {
    const sid = 'sid-resolve';
    await store.set(sid, seededRecord(), 60_000, null);
    const oidc = fakeOidc();
    runtime = baseRuntime(oidc);

    await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));

    const result = await resolveSession(
      {
        store,
        // Never expected to be called: the record is gone, so resolveSession returns before ever
        // needing a refresh function.
        refresh: () => Promise.reject(new Error('must not be called: the session is already deleted')),
        logger: recordingLogger(),
        skewMs: runtime.skewMs,
        lockTtlMs: runtime.lockTtlMs,
        lockWaitMs: runtime.lockWaitMs,
        ttlMs: runtime.ttlMs,
      },
      sid,
    );
    expect(result).toBeNull();
  });
});

describe('POST /auth/logout — cookies', () => {
  it('clears __Host-pgs_sid', async () => {
    const sid = 'sid-cookie';
    await store.set(sid, seededRecord(), 60_000, null);
    runtime = baseRuntime(fakeOidc());

    const res = await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));
    const cleared = res.headers.getSetCookie().find((c) => c.startsWith(`${SESSION_COOKIE}=`));
    expect(cleared).toBeDefined();
    expect(cleared).toMatch(/Max-Age=0/);
  });

  it('clears every outstanding txn cookie', async () => {
    const sid = 'sid-cookie-2';
    await store.set(sid, seededRecord(), 60_000, null);
    runtime = baseRuntime(fakeOidc());

    const txnName = txnCookieName('some-leftover-txn');
    const cookieHeader = `${SESSION_COOKIE}=${sid}; ${txnName}=some-secret`;
    const res = await createAuthRoutes(runtime).handle(logoutRequest(cookieHeader));

    const clearedTxn = res.headers.getSetCookie().find((c) => c.startsWith(`${txnName}=`) && /Max-Age=0/.test(c));
    expect(clearedTxn).toBeDefined();
  });

  it('a logout with no session cookie is a no-op that still redirects — it does not throw', async () => {
    runtime = baseRuntime(fakeOidc());
    const res = await createAuthRoutes(runtime).handle(logoutRequest());
    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toBeTruthy();
    const cleared = res.headers.getSetCookie().find((c) => c.startsWith(`${SESSION_COOKIE}=`));
    expect(cleared).toBeDefined();
  });

  it('does not call oidc.revoke at all when there is no session cookie', async () => {
    const oidc = fakeOidc();
    runtime = baseRuntime(oidc);
    await createAuthRoutes(runtime).handle(logoutRequest());
    expect(oidc.revokeCalls).toEqual([]);
  });

  it('does not call oidc.revoke when the session record carries no refresh token', async () => {
    const sid = 'sid-no-refresh';
    // eslint-disable-next-line @typescript-eslint/no-unused-vars -- rest-sibling destructuring
    const { refreshToken: _omitted, ...withoutRefresh } = seededRecord();
    await store.set(sid, withoutRefresh, 60_000, null);
    const oidc = fakeOidc();
    runtime = baseRuntime(oidc);

    const res = await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));
    expect(res.status).toBe(302);
    expect(oidc.revokeCalls).toEqual([]);
  });
});

describe('POST /auth/logout — end-session redirect', () => {
  it('redirects to end_session_endpoint carrying post_logout_redirect_uri and a state', async () => {
    const sid = 'sid-redirect';
    await store.set(sid, seededRecord(), 60_000, null);
    const oidc = fakeOidc({ endSessionUrl: END_SESSION_URL });
    runtime = baseRuntime(oidc);

    const res = await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));

    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toBe(END_SESSION_URL);
    expect(oidc.buildEndSessionUrlCalls).toHaveLength(1);
    const call = oidc.buildEndSessionUrlCalls[0];
    expect(call?.postLogoutRedirectUri).toBe(POST_LOGOUT_REDIRECT_URI);
    expect(call?.state).toBeTruthy();
    // DELIBERATE, not a gap (see routes.ts's "NAMED RESIDUAL" comment and design doc § 9.5):
    // SessionRecord never carries the raw ID token JWT, only its decoded claims, and M12 measured
    // Keycloak 26.4 completing the end-session redirect on `client_id` alone with no
    // `id_token_hint` needed.
    expect(call?.idTokenHint).toBeUndefined();
  });

  it('mints a fresh state per logout call', async () => {
    const oidc = fakeOidc();
    runtime = baseRuntime(oidc);
    await createAuthRoutes(runtime).handle(logoutRequest());
    await createAuthRoutes(runtime).handle(logoutRequest());
    const [first, second] = oidc.buildEndSessionUrlCalls;
    expect(first?.state).toBeTruthy();
    expect(second?.state).toBeTruthy();
    expect(first?.state).not.toBe(second?.state);
  });

  it('emits logout.completed with the zone, revoked outcome and a truncated sid', async () => {
    const sid = 'sid-logged';
    await store.set(sid, seededRecord(), 60_000, null);
    runtime = baseRuntime(fakeOidc());

    await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));

    const completed = events.find(([name]) => name === 'logout.completed');
    expect(completed).toBeDefined();
    expect(completed?.[1]['zone']).toBe('iam');
    expect(completed?.[1]['revoked']).toBe(true);
    expect(completed?.[1]['sid']).toBe(sid.slice(0, 8));
    expect(completed?.[1]['endSessionRedirected']).toBe(true);
  });

  it('emits logout.completed with revoked: false when revocation rejected', async () => {
    const sid = 'sid-logged-2';
    await store.set(sid, seededRecord(), 60_000, null);
    runtime = baseRuntime(fakeOidc({ revokeImpl: () => Promise.reject(new Error('idp down')) }));

    await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));

    const completed = events.find(([name]) => name === 'logout.completed');
    expect(completed?.[1]['revoked']).toBe(false);
  });

  it('falls back to postLogoutRedirectUri when buildEndSessionUrl throws (e.g. no end_session_endpoint advertised)', async () => {
    const oidc = fakeOidc();
    oidc.buildEndSessionUrl = (): Promise<string> => Promise.reject(new Error('no end_session_endpoint'));
    runtime = baseRuntime(oidc);

    const res = await createAuthRoutes(runtime).handle(logoutRequest());
    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toBe(POST_LOGOUT_REDIRECT_URI);
  });

  // Review round 1: the step-4 degradation above previously logged nothing, unlike step 3's
  // revocation outcome — an operator had no signal the end-session redirect ever fell back.
  it('logs endSessionRedirected: false when buildEndSessionUrl throws', async () => {
    const oidc = fakeOidc();
    oidc.buildEndSessionUrl = (): Promise<string> => Promise.reject(new Error('no end_session_endpoint'));
    runtime = baseRuntime(oidc);

    await createAuthRoutes(runtime).handle(logoutRequest());

    const completed = events.find(([name]) => name === 'logout.completed');
    expect(completed?.[1]['endSessionRedirected']).toBe(false);
  });
});

describe('GET /auth/logout/callback', () => {
  it('with a valid state redirects to the zone root', async () => {
    const oidc = fakeOidc();
    runtime = baseRuntime(oidc);
    // "Valid" here means: the exact state a real /auth/logout call just minted. The fake's
    // buildEndSessionUrl returns a fixed URL regardless of params, so the state is read back from
    // the recorded call rather than parsed out of the (fake) redirect Location.
    await createAuthRoutes(runtime).handle(logoutRequest());
    const state = oidc.buildEndSessionUrlCalls[0]?.state ?? '';
    expect(state).toBeTruthy();

    const res = await createAuthRoutes(runtime).handle(logoutCallbackRequest(`?state=${state}`));
    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toBe(POST_LOGOUT_REDIRECT_URI);
  });

  it('with an unknown state also redirects to the zone root and is not an error', async () => {
    runtime = baseRuntime(fakeOidc());
    const res = await createAuthRoutes(runtime).handle(logoutCallbackRequest('?state=never-seen-before'));
    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toBe(POST_LOGOUT_REDIRECT_URI);
  });

  it('with no state at all still redirects to the zone root and is not an error', async () => {
    runtime = baseRuntime(fakeOidc());
    const res = await createAuthRoutes(runtime).handle(logoutCallbackRequest());
    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toBe(POST_LOGOUT_REDIRECT_URI);
  });

  it('clears __Host-pgs_sid', async () => {
    runtime = baseRuntime(fakeOidc());
    const res = await createAuthRoutes(runtime).handle(logoutCallbackRequest('?state=whatever'));
    const cleared = res.headers.getSetCookie().find((c) => c.startsWith(`${SESSION_COOKIE}=`));
    expect(cleared).toBeDefined();
    expect(cleared).toMatch(/Max-Age=0/);
  });
});

describe('logout does not touch unrelated cookies', () => {
  it('leaves a non-txn, non-session cookie header unaffected in the Set-Cookie list', async () => {
    runtime = baseRuntime(fakeOidc());
    const res = await createAuthRoutes(runtime).handle(logoutRequest('unrelated=1'));
    const setCookies = res.headers.getSetCookie();
    const touchesUnrelated = setCookies.some((c) => c.startsWith('unrelated='));
    expect(touchesUnrelated).toBe(false);
  });
});
