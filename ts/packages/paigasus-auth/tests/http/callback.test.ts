// SPDX-License-Identifier: Apache-2.0
//
// GET /auth/callback — the login-CSRF defence (design doc § 9.2).
//
// Without the browser-bound secret checked here, an attacker authenticates as themselves,
// captures their own callback URL without following it, sends it to a victim, and the victim's
// browser is issued a session AS THE ATTACKER — everything the victim then does lands in the
// attacker's account. Every rejection test below also asserts the token endpoint was never
// called: a test that only checks the rejection would still pass against an implementation that
// exchanges the code and THEN rejects, which leaks a code use to the IdP and defeats the point of
// rejecting early.
//
// PKCE (review round 1, Important 2): `fixture.setNextCodeChallenge` is armed in `beforeEach` with
// the S256 challenge for a freshly-generated `codeVerifier`, used as `seedTransaction`'s default.
// Every success test therefore exercises real PKCE enforcement — replacing `tx.codeVerifier` in
// `routes.ts` with any constant now fails every one of them, since the constant cannot match a
// value generated fresh per test. See the mutation record in
// docs/superpowers/specs/2026-09-09-sma-506-measurements.md, "M6 — PKCE".
import { randomUUID } from 'node:crypto';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { claimsPrincipalResolver } from '../../src/adapters/claims-resolver.js';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { createOidcClient, type OidcClient } from '../../src/adapters/oidc.js';
import { hashSecret } from '../../src/core/ids.js';
import { CallbackRejected } from '../../src/core/errors.js';
import type { SessionRecord } from '../../src/core/session.js';
import { SESSION_COOKIE, txnCookieName } from '../../src/http/cookies.js';
import { createAuthRoutes } from '../../src/http/routes.js';
import type { AuthEventFields, AuthEventName, AuthLogger } from '../../src/ports/logger.js';
import type { LoginTransaction } from '../../src/ports/session-store.js';
import type { ResolvedPrincipal } from '../../src/ports/principal-resolver.js';
import type { AuthRuntime } from '../../src/runtime.js';
import { s256CodeChallenge, startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';

const CALLBACK_URL = 'https://rp.example.com/iam/auth/callback';
const NONCE = 'expected-nonce';
const RETURN_TO = '/iam/dashboard';
const CORRECT_SECRET = 'correct-secret-value-32-bytes-ok';

let fixture: OidcFixture;
let store: MemorySessionStore;
let events: Array<[AuthEventName, AuthEventFields]>;
let grantCalls: number;
let runtime: AuthRuntime;
let codeVerifier: string;

function recordingLogger(): AuthLogger {
  return { event: (name, fields) => void events.push([name, { ...fields }]) };
}

/**
 * Wraps the real OIDC client (pointed at the real jwks.ts fixture) with an invocation counter on
 * `authorizationCodeGrant` — the one method that performs the token-endpoint HTTP call. This is
 * what makes "the token endpoint was never called" assertable without modifying the shared
 * fixture: routes.ts can only reach the token endpoint through this method.
 */
function countingOidc(inner: OidcClient): OidcClient {
  return {
    buildAuthorizationUrl: (params) => inner.buildAuthorizationUrl(params),
    authorizationCodeGrant: (params) => {
      grantCalls += 1;
      return inner.authorizationCodeGrant(params);
    },
    refresh: (token) => inner.refresh(token),
    revoke: (token) => inner.revoke(token),
    buildEndSessionUrl: (params) => inner.buildEndSessionUrl(params),
  };
}

beforeEach(async () => {
  fixture = await startOidcFixture();
  store = new MemorySessionStore();
  events = [];
  grantCalls = 0;
  // A fresh verifier per test (never a fixed literal): a mutation replacing routes.ts's
  // `codeVerifier: tx.codeVerifier` with any constant cannot coincidentally match this.
  codeVerifier = randomUUID();
  fixture.setNextCodeChallenge(s256CodeChallenge(codeVerifier));
  runtime = {
    store,
    resolver: claimsPrincipalResolver,
    logger: recordingLogger(),
    oidc: countingOidc(
      createOidcClient({
        issuer: fixture.issuer,
        clientId: fixture.clientId,
        clientSecret: fixture.clientSecret,
        httpTimeoutMs: 5000,
        clockToleranceSeconds: 30,
        allowInsecureRequests: true, // the fixture is plain http on localhost — never set in production
      }),
    ),
    redirectUri: CALLBACK_URL,
    postLogoutRedirectUri: 'https://rp.example.com/',
    cookieDomainless: true,
    skewMs: 30_000,
    lockTtlMs: 10_000,
    lockWaitMs: 3_000,
    ttlMs: 28_800_000,
    absoluteTtlMs: 86_400_000,
    zone: 'iam',
    basePath: '/iam',
    scopes: 'openid profile email',
  };
});

afterEach(async () => {
  await fixture.close();
});

async function seedTransaction(state: string, overrides: Partial<LoginTransaction> = {}): Promise<void> {
  const tx: LoginTransaction = {
    codeVerifier,
    nonce: NONCE,
    returnTo: RETURN_TO,
    secretHash: hashSecret(CORRECT_SECRET),
    createdAt: Date.now(),
    ...overrides,
  };
  await store.putTransaction(state, tx, 600_000);
}

function callbackRequest(state: string | null, cookieHeader?: string, extraParams?: Record<string, string>): Request {
  const params = new URLSearchParams({ code: 'test-code', ...extraParams });
  if (state !== null) params.set('state', state);
  const init = cookieHeader !== undefined ? { headers: { cookie: cookieHeader } } : undefined;
  return new Request(`${CALLBACK_URL}?${params.toString()}`, init);
}

function cookieHeaderFor(state: string, secret: string, extra?: string): string {
  const base = `${txnCookieName(state)}=${secret}`;
  return extra !== undefined ? `${base}; ${extra}` : base;
}

async function expectRejected(promise: Promise<Response>, reason: CallbackRejected['reason']): Promise<void> {
  await expect(promise).rejects.toBeInstanceOf(CallbackRejected);
  await promise.catch((err: unknown) => {
    expect(err).toBeInstanceOf(CallbackRejected);
    expect((err as CallbackRejected).reason).toBe(reason);
  });
}

const fakePrincipal: ResolvedPrincipal = {
  principalPrn: null,
  issuer: 'https://issuer.example.com',
  subject: 'old-subject',
  memberships: [],
  roleGrants: [],
  grantsAvailable: false,
};

function seededRecord(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    version: 1,
    rev: 0,
    accessToken: 'old-access-token',
    accessExpiresAt: Date.now() + 60_000,
    absoluteExpiresAt: Date.now() + 60_000,
    idTokenClaims: { iss: 'https://issuer.example.com', sub: 'old-subject' },
    principal: fakePrincipal,
    ...overrides,
  };
}

describe('GET /auth/callback — rejection (login-CSRF defence)', () => {
  it('rejects with txn_missing when no txn cookie is present, before any token exchange', async () => {
    const state = 'state-1';
    await seedTransaction(state);

    const routes = createAuthRoutes(runtime);
    await expectRejected(routes.handle(callbackRequest(state)), 'txn_missing');

    expect(grantCalls).toBe(0);
    // The store transaction was never touched — the cookie check happens first.
    const tx = await store.takeTransaction(state);
    expect(tx).not.toBeNull();
  });

  it('rejects with txn_missing when only an unrelated cookie is present', async () => {
    const state = 'state-1b';
    await seedTransaction(state);

    const routes = createAuthRoutes(runtime);
    await expectRejected(routes.handle(callbackRequest(state, 'unrelated=1')), 'txn_missing');
    expect(grantCalls).toBe(0);
  });

  it('rejects with txn_mismatch when the cookie secret does not hash to the stored value', async () => {
    const state = 'state-2';
    await seedTransaction(state);

    const routes = createAuthRoutes(runtime);
    const req = callbackRequest(state, cookieHeaderFor(state, 'a-completely-different-secret'));
    await expectRejected(routes.handle(req), 'txn_mismatch');

    expect(grantCalls).toBe(0);
  });

  it('rejects with state_unknown when the state has no stored transaction', async () => {
    const state = 'never-seeded-state';
    const routes = createAuthRoutes(runtime);
    const req = callbackRequest(state, cookieHeaderFor(state, CORRECT_SECRET));
    await expectRejected(routes.handle(req), 'state_unknown');

    expect(grantCalls).toBe(0);
  });

  it('rejects with state_unknown when the callback carries no state at all', async () => {
    const routes = createAuthRoutes(runtime);
    const req = callbackRequest(null);
    await expectRejected(routes.handle(req), 'state_unknown');
    expect(grantCalls).toBe(0);
  });

  it('rejects a replayed callback: the same state twice, the second failing because takeTransaction is single-use', async () => {
    const state = 'state-replay';
    await seedTransaction(state);
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE }));

    const routes = createAuthRoutes(runtime);
    const cookieHeader = cookieHeaderFor(state, CORRECT_SECRET);

    const first = await routes.handle(callbackRequest(state, cookieHeader));
    expect(first.status).toBe(302);
    expect(grantCalls).toBe(1);

    await expectRejected(routes.handle(callbackRequest(state, cookieHeader)), 'state_unknown');
    // The second, rejected attempt made no additional token-endpoint call.
    expect(grantCalls).toBe(1);
  });

  it('emits login.callback_rejected with the reason', async () => {
    const state = 'state-logged';
    await seedTransaction(state);
    const routes = createAuthRoutes(runtime);
    await expectRejected(routes.handle(callbackRequest(state)), 'txn_missing');
    expect(events).toContainEqual(['login.callback_rejected', { reason: 'txn_missing', zone: 'iam' }]);
  });

  // Review round 1, Important 1: the identity provider declining (the user clicked Cancel) must
  // be distinguishable from a genuine token-endpoint failure, and must not burn the code endpoint.
  it('rejects with idp_error when the callback carries an IdP error response, without calling the token endpoint', async () => {
    const state = 'state-idp-error';
    await seedTransaction(state);

    const routes = createAuthRoutes(runtime);
    const req = callbackRequest(state, cookieHeaderFor(state, CORRECT_SECRET), { error: 'access_denied' });
    await expectRejected(routes.handle(req), 'idp_error');

    expect(grantCalls).toBe(0);
    expect(events).toContainEqual(['login.callback_rejected', { reason: 'idp_error', zone: 'iam' }]);
  });

  // Review round 1, Important 1: `code_exchange_failed` is reserved for a GENUINE exchange
  // failure (as opposed to idp_error above, or the CSRF-defence reasons, which never reach the
  // token endpoint at all). A nonce mismatch is a real, independent way the exchange can fail.
  it('rejects with code_exchange_failed when the exchange itself fails (e.g. a nonce mismatch)', async () => {
    const state = 'state-exchange-fail';
    await seedTransaction(state, { nonce: 'the-expected-nonce' });
    // Minted with a DIFFERENT nonce than the transaction expects.
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: 'a-different-nonce' }));

    const routes = createAuthRoutes(runtime);
    const req = callbackRequest(state, cookieHeaderFor(state, CORRECT_SECRET));
    await expectRejected(routes.handle(req), 'code_exchange_failed');

    // The token endpoint WAS called (this is a genuine exchange failure, not a CSRF rejection).
    expect(grantCalls).toBe(1);
  });

  // Review round 1, Important 2 (PKCE): a code_verifier that does not match the challenge
  // presented at authorization must fail the exchange — this is the property that, before the
  // fixture enforced it, could be silently dropped from routes.ts with all 31 tests still green.
  it('rejects with code_exchange_failed when the PKCE verifier does not match the authorization-time challenge', async () => {
    const state = 'state-pkce-mismatch';
    // `codeVerifier` here does NOT match what `fixture.setNextCodeChallenge` was armed with in
    // beforeEach (which used the module-level `codeVerifier`), so the token endpoint's own S256
    // recomputation fails it.
    await seedTransaction(state, { codeVerifier: 'a-verifier-that-does-not-match-the-challenge' });
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE }));

    const routes = createAuthRoutes(runtime);
    const req = callbackRequest(state, cookieHeaderFor(state, CORRECT_SECRET));
    await expectRejected(routes.handle(req), 'code_exchange_failed');

    expect(grantCalls).toBe(1);
  });
});

describe('GET /auth/callback — success', () => {
  it('issues a new session id and deletes any pre-existing record for the old sid', async () => {
    const state = 'state-success-1';
    await seedTransaction(state);
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE }));

    const oldSid = 'old-sid-value';
    await store.set(oldSid, seededRecord(), 60_000, null);

    const routes = createAuthRoutes(runtime);
    const cookieHeader = `${cookieHeaderFor(state, CORRECT_SECRET)}; ${SESSION_COOKIE}=${oldSid}`;
    const res = await routes.handle(callbackRequest(state, cookieHeader));

    expect(res.status).toBe(302);
    expect(await store.get(oldSid)).toBeNull();

    const setCookie = res.headers.getSetCookie().find((c) => c.startsWith(`${SESSION_COOKIE}=`));
    expect(setCookie).toBeDefined();
    const newSid = setCookie?.split(';')[0]?.split('=')[1];
    expect(newSid).toBeTruthy();
    expect(newSid).not.toBe(oldSid);

    const newRecord = newSid !== undefined ? await store.get(newSid) : null;
    expect(newRecord).not.toBeNull();
  });

  it('writes a record whose absoluteExpiresAt is now + ABSOLUTE_TTL', async () => {
    const state = 'state-success-2';
    await seedTransaction(state);
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE }));

    const routes = createAuthRoutes(runtime);
    const before = Date.now();
    const res = await routes.handle(callbackRequest(state, cookieHeaderFor(state, CORRECT_SECRET)));
    const after = Date.now();

    const setCookie = res.headers.getSetCookie().find((c) => c.startsWith(`${SESSION_COOKIE}=`));
    const sid = setCookie?.split(';')[0]?.split('=')[1] ?? '';
    const record = await store.get(sid);
    expect(record).not.toBeNull();
    expect(record?.absoluteExpiresAt).toBeGreaterThanOrEqual(before + runtime.absoluteTtlMs);
    expect(record?.absoluteExpiresAt).toBeLessThanOrEqual(after + runtime.absoluteTtlMs);
  });

  it('redirects to the validated returnTo', async () => {
    const state = 'state-success-3';
    await seedTransaction(state, { returnTo: '/iam/somewhere' });
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE }));

    const routes = createAuthRoutes(runtime);
    const res = await routes.handle(callbackRequest(state, cookieHeaderFor(state, CORRECT_SECRET)));

    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toBe('/iam/somewhere');
  });

  it('clears every outstanding txn cookie', async () => {
    const state = 'state-success-4';
    await seedTransaction(state);
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE }));

    const extraName = txnCookieName('some-other-txn');
    const cookieHeader = `${cookieHeaderFor(state, CORRECT_SECRET)}; ${extraName}=other-secret`;

    const routes = createAuthRoutes(runtime);
    const res = await routes.handle(callbackRequest(state, cookieHeader));

    const setCookies = res.headers.getSetCookie();
    const clearedUsed = setCookies.find((c) => c.startsWith(`${txnCookieName(state)}=`) && /Max-Age=0/.test(c));
    const clearedExtra = setCookies.find((c) => c.startsWith(`${extraName}=`) && /Max-Age=0/.test(c));
    expect(clearedUsed).toBeDefined();
    expect(clearedExtra).toBeDefined();
  });

  it('emits session.created with a truncated sid', async () => {
    const state = 'state-success-5';
    await seedTransaction(state);
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE }));

    const routes = createAuthRoutes(runtime);
    const res = await routes.handle(callbackRequest(state, cookieHeaderFor(state, CORRECT_SECRET)));
    const setCookie = res.headers.getSetCookie().find((c) => c.startsWith(`${SESSION_COOKIE}=`));
    const sid = setCookie?.split(';')[0]?.split('=')[1] ?? '';

    const created = events.find(([name]) => name === 'session.created');
    expect(created).toBeDefined();
    expect(created?.[1]['sid']).toBe(sid.slice(0, 8));
    expect(created?.[1]['zone']).toBe('iam');
  });

  // Review round 1: nothing previously pinned the Set-Cookie string at the CALL SITE — the
  // helper's own defaults being correct (proven in cookies.test.ts) does not mean the call site
  // in routes.ts actually uses them. Adding a Max-Age here, or dropping HttpOnly, reds nothing
  // without this. The expected string is hand-written, not derived by calling `serializeCookie`
  // ourselves, so a regression in the helper AND its call site would still be caught by one of
  // the two test files.
  it('emits the exact __Host-pgs_sid Set-Cookie string: no Max-Age, all four security attributes', async () => {
    const state = 'state-success-6';
    await seedTransaction(state);
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE }));

    const routes = createAuthRoutes(runtime);
    const res = await routes.handle(callbackRequest(state, cookieHeaderFor(state, CORRECT_SECRET)));
    const setCookie = res.headers.getSetCookie().find((c) => c.startsWith(`${SESSION_COOKIE}=`));
    expect(setCookie).toBeDefined();
    const sid = setCookie?.split(';')[0]?.split('=')[1] ?? '';

    expect(setCookie).toBe(`${SESSION_COOKIE}=${sid}; HttpOnly; Secure; SameSite=Lax; Path=/`);
  });
});

describe('route dispatch', () => {
  it('responds 405 to a non-GET /auth/callback', async () => {
    const res = await createAuthRoutes(runtime).handle(new Request(CALLBACK_URL, { method: 'POST' }));
    expect(res.status).toBe(405);
  });
});
