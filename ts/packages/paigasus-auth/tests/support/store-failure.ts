// SPDX-License-Identifier: Apache-2.0
//
// SMA-653 route-test harness: a store that fails on chosen methods, a network-free OIDC fake, and
// the assertions that every 503 row shares.
//
// SMA-656 uses it for the OIDC discovery rows too (tests/http/discovery-failed.test.ts). The fake
// can reject `buildAuthorizationUrl` and `authorizationCodeGrant` with a chosen error. The
// transaction seed and the callback request live here, so both route test files use one copy. The
// two redaction assertions take the forbidden strings as a parameter.
//
// THE SENTINELS. Every thrown store error carries SENTINEL_DSN in its message, the same way the
// real adapter puts the (redacted) DSN there. A discovery test error carries SENTINEL_IDP_URL, a URL
// that the OIDC library could put in its own error text. expectStoreUnavailable and
// expectEventsClean then assert that no forbidden string reaches the body, a header, or a logged
// field. The default is STORE_SENTINELS, so the SMA-653 rows did not change. A test that forgets to
// call them is weaker; every row in store-unavailable.test.ts and discovery-failed.test.ts that
// produces a response calls both.
import { expect } from 'vitest';
import { claimsPrincipalResolver } from '../../src/adapters/claims-resolver.js';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import type { AuthorizationRequest, BuildEndSessionUrlParams, OidcClient, OidcTokens, RefreshedTokens } from '../../src/adapters/oidc.js';
import { SessionStoreTimeout, SessionStoreUnavailable } from '../../src/core/errors.js';
import { hashSecret } from '../../src/core/ids.js';
import { SESSION_COOKIE, txnCookieName } from '../../src/http/cookies.js';
import { STORE_UNAVAILABLE_CSP } from '../../src/http/store-unavailable.js';
import type { AuthEventFields, AuthEventName } from '../../src/ports/logger.js';
import type { SessionStore } from '../../src/ports/session-store.js';
import type { AuthRuntime } from '../../src/runtime.js';

export const SENTINEL_DSN = 'redis://user:sentinel-pw@redis.invalid:6379';
/** The parts of SENTINEL_DSN that no response and no event may hold (the SMA-653 rows). */
export const STORE_SENTINELS: readonly string[] = ['sentinel-pw', 'redis.invalid'];
/** SMA-656: a URL the OIDC library could put in its error text. Each discovery test error's message holds it. */
export const SENTINEL_IDP_URL = 'https://idp.invalid/.well-known/openid-configuration?sentinel-656';
/** The parts of SENTINEL_IDP_URL that no response and no event may hold (the SMA-656 rows). */
export const IDP_SENTINELS: readonly string[] = ['idp.invalid', 'sentinel-656'];
export const ORIGIN = 'https://rp.example.com';
export const BASE_PATH = '/iam';
export const END_SESSION_URL = 'https://issuer.example.com/logout';
export const NEW_REFRESH_TOKEN = 'new-refresh-token';
/** The harness runtime's OIDC client id. */
export const CLIENT_ID = 'paigasus-console';
/** The callback rows' transaction id (the `state`) and its browser-bound secret. */
export const STATE = 'state-0123456789';
export const TXN_SECRET = 'correct-secret-value-32-bytes-ok';
const b64 = (value: unknown): string => Buffer.from(JSON.stringify(value)).toString('base64url');
/**
 * The raw ID token that `fakeOidc().authorizationCodeGrant` returns. JWT-shaped, not signed. Its
 * `aud` is CLIENT_ID, so logout would send it as the hint (http/routes.ts `hintAudienceMatches`).
 */
export const FAKE_ID_TOKEN = `${b64({ alg: 'RS256', typ: 'JWT' })}.${b64({ iss: 'https://issuer.example.com', sub: 'a-subject', aud: CLIENT_ID })}.fake-signature`;

export type FailureKind = 'unavailable' | 'timeout';
export const FAILURE_KINDS: readonly FailureKind[] = ['unavailable', 'timeout'];

export type StoreMethod = 'get' | 'set' | 'delete' | 'putTransaction' | 'takeTransaction';

export function storeError(kind: FailureKind): Error {
  return kind === 'unavailable' ? new SessionStoreUnavailable(`session store unavailable (${SENTINEL_DSN})`) : new SessionStoreTimeout(SENTINEL_DSN, 4000, 'deadline');
}

/** Wraps a real store. Records `method:arg` for each guarded call, and rejects on `failOn`. */
export function failingStore(inner: SessionStore, failOn: ReadonlySet<StoreMethod>, makeError: () => Error, calls: string[]): SessionStore {
  const guard = <T>(method: StoreMethod, arg: string, op: () => Promise<T>): Promise<T> => {
    calls.push(`${method}:${arg}`);
    return failOn.has(method) ? Promise.reject(makeError()) : op();
  };
  return {
    get: (sid) => guard('get', sid, () => inner.get(sid)),
    set: (sid, rec, ttlMs, expectedRev) => guard('set', sid, () => inner.set(sid, rec, ttlMs, expectedRev)),
    delete: (sid) => guard('delete', sid, () => inner.delete(sid)),
    tryAcquireLock: (sid, token, ttlMs) => inner.tryAcquireLock(sid, token, ttlMs),
    releaseLock: (sid, token) => inner.releaseLock(sid, token),
    putTransaction: (txnId, tx, ttlMs) => guard('putTransaction', txnId, () => inner.putTransaction(txnId, tx, ttlMs)),
    takeTransaction: (txnId) => guard('takeTransaction', txnId, () => inner.takeTransaction(txnId)),
    close: () => inner.close(),
  };
}

export interface FakeOidc extends OidcClient {
  revokeCalls: string[];
  /** When true, `revoke` rejects with an error whose message holds SENTINEL_DSN. */
  failRevoke: boolean;
  /** The parameters of every `buildEndSessionUrl` call, in order (SMA-681: does it carry a hint?). */
  endSessionCalls: BuildEndSessionUrlParams[];
  /** SMA-656: when set, `buildAuthorizationUrl` rejects with this error. */
  authorizationError?: Error;
  /** SMA-656: when set, `authorizationCodeGrant` rejects with this error. */
  codeGrantError?: Error;
}

/** No network. Unless `codeGrantError` is set, the code exchange succeeds and returns NEW_REFRESH_TOKEN. */
export function fakeOidc(): FakeOidc {
  const oidc: FakeOidc = {
    revokeCalls: [],
    failRevoke: false,
    endSessionCalls: [],
    buildAuthorizationUrl: (): Promise<AuthorizationRequest> =>
      oidc.authorizationError !== undefined
        ? Promise.reject(oidc.authorizationError)
        : Promise.resolve({ url: 'https://issuer.example.com/authorize?client_id=test', codeVerifier: 'a-verifier', nonce: 'a-nonce' }),
    authorizationCodeGrant: (): Promise<OidcTokens> =>
      oidc.codeGrantError !== undefined
        ? Promise.reject(oidc.codeGrantError)
        : Promise.resolve({
            accessToken: 'new-access-token',
            refreshToken: NEW_REFRESH_TOKEN,
            expiresIn: 300,
            idToken: FAKE_ID_TOKEN,
            idTokenClaims: { iss: 'https://issuer.example.com', sub: 'a-subject' },
          }),
    refresh: (): Promise<RefreshedTokens> => Promise.reject(new Error('refresh is not used by the auth routes')),
    revoke: (token: string): Promise<void> => {
      oidc.revokeCalls.push(token);
      return oidc.failRevoke ? Promise.reject(new Error(`revoke failed at ${SENTINEL_DSN}`)) : Promise.resolve();
    },
    buildEndSessionUrl: (params: BuildEndSessionUrlParams): Promise<string> => {
      oidc.endSessionCalls.push(params);
      return Promise.resolve(END_SESSION_URL);
    },
  };
  return oidc;
}

export interface Harness {
  runtime: AuthRuntime;
  /** The real store behind the failing wrapper: seed and inspect it directly. */
  inner: MemorySessionStore;
  calls: string[];
  events: Array<[AuthEventName, AuthEventFields]>;
  oidc: FakeOidc;
}

export function harness(failOn: readonly StoreMethod[], makeError: () => Error): Harness {
  const inner = new MemorySessionStore();
  const calls: string[] = [];
  const events: Array<[AuthEventName, AuthEventFields]> = [];
  const oidc = fakeOidc();
  const runtime: AuthRuntime = {
    store: failingStore(inner, new Set(failOn), makeError, calls),
    resolver: claimsPrincipalResolver,
    logger: { event: (name, fields) => void events.push([name, { ...fields }]) },
    oidc,
    publicOrigin: ORIGIN,
    redirectUri: `${ORIGIN}${BASE_PATH}/auth/callback`,
    postLogoutRedirectUri: `${ORIGIN}${BASE_PATH}/`,
    clientId: CLIENT_ID,
    cookieDomainless: true,
    skewMs: 30_000,
    lockTtlMs: 10_000,
    lockWaitMs: 3_000,
    ttlMs: 28_800_000,
    absoluteTtlMs: 86_400_000,
    zone: 'iam',
    basePath: BASE_PATH,
  };
  return { runtime, inner, calls, events, oidc };
}

/** Stores the callback rows' transaction under STATE, directly in the real store (no recorded call). */
export async function seedTransaction(h: Harness, returnTo: string): Promise<void> {
  await h.inner.putTransaction(STATE, { codeVerifier: 'a-verifier', nonce: 'a-nonce', returnTo, secretHash: hashSecret(TXN_SECRET), createdAt: Date.now() }, 600_000);
}

/** A callback for STATE with its transaction cookie, and a session cookie when `sid` is given. */
export function callbackRequest(sid?: string): Request {
  const cookies = [`${txnCookieName(STATE)}=${TXN_SECRET}`, ...(sid !== undefined ? [`${SESSION_COOKIE}=${sid}`] : [])];
  return new Request(`${ORIGIN}${BASE_PATH}/auth/callback?code=a-code&state=${STATE}`, { headers: { cookie: cookies.join('; ') } });
}

const HTML_ENTITIES: Readonly<Record<string, string>> = { '&amp;': '&', '&lt;': '<', '&gt;': '>', '&quot;': '"', '&#39;': "'" };

export function htmlDecode(value: string): string {
  return value.replace(/&(?:amp|lt|gt|quot|#39);/g, (entity) => HTML_ENTITIES[entity] ?? entity);
}

/** Asserts every § 3 property of a 503, that the retry target is exactly `target`, and that no `forbidden` string is in the body or a header. */
export async function expectStoreUnavailable(res: Response, expected: { kind: 'link' | 'post'; target: string }, forbidden: readonly string[] = STORE_SENTINELS): Promise<string> {
  expect(res.status).toBe(503);
  expect(res.headers.get('retry-after')).toBe('5');
  expect(res.headers.get('cache-control')).toBe('no-store');
  expect(res.headers.get('content-type')).toBe('text/html; charset=utf-8');
  expect(res.headers.get('referrer-policy')).toBe('no-referrer');
  expect(res.headers.get('content-security-policy')).toBe(STORE_UNAVAILABLE_CSP);
  expect(res.headers.getSetCookie()).toEqual([]);
  const body = await res.text();
  const attribute = expected.kind === 'link' ? 'href' : 'action';
  const match = new RegExp(`${attribute}="([^"]*)"`).exec(body);
  expect(match, `no ${attribute} attribute in the 503 body`).not.toBeNull();
  expect(htmlDecode(match?.[1] ?? '')).toBe(expected.target);
  if (expected.kind === 'post') expect(body).toContain('method="post"');
  for (const text of [body, ...[...res.headers].map(([name, value]) => `${name}: ${value}`)]) {
    for (const sentinel of forbidden) expect(text).not.toContain(sentinel);
  }
  return body;
}

/** No logged field of any event holds a `forbidden` string. */
export function expectEventsClean(events: ReadonlyArray<[AuthEventName, AuthEventFields]>, forbidden: readonly string[] = STORE_SENTINELS): void {
  const text = JSON.stringify(events);
  for (const sentinel of forbidden) expect(text).not.toContain(sentinel);
}

/** The fields of each `store.unavailable` event, in order. */
export function storeUnavailableEvents(events: ReadonlyArray<[AuthEventName, AuthEventFields]>): AuthEventFields[] {
  return events.filter(([name]) => name === 'store.unavailable').map(([, fields]) => fields);
}
