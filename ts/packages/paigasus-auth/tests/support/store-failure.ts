// SPDX-License-Identifier: Apache-2.0
//
// SMA-653 route-test harness: a store that fails on chosen methods, a network-free OIDC fake, and
// the assertions that every 503 row shares.
//
// THE SENTINEL. Every thrown store error carries SENTINEL_DSN in its message, the same way the
// real adapter puts the (redacted) DSN there. expectStoreUnavailable and expectEventsClean then
// assert that no byte of it reaches the body, a header, or a logged field. A test that forgets to
// call them is weaker; every row in store-unavailable.test.ts that produces a response calls both.
import { expect } from 'vitest';
import { claimsPrincipalResolver } from '../../src/adapters/claims-resolver.js';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import type { AuthorizationRequest, BuildEndSessionUrlParams, OidcClient, OidcTokens, RefreshedTokens } from '../../src/adapters/oidc.js';
import { SessionStoreTimeout, SessionStoreUnavailable } from '../../src/core/errors.js';
import { STORE_UNAVAILABLE_CSP } from '../../src/http/store-unavailable.js';
import type { AuthEventFields, AuthEventName } from '../../src/ports/logger.js';
import type { SessionStore } from '../../src/ports/session-store.js';
import type { AuthRuntime } from '../../src/runtime.js';

export const SENTINEL_DSN = 'redis://user:sentinel-pw@redis.invalid:6379';
export const ORIGIN = 'https://rp.example.com';
export const BASE_PATH = '/iam';
export const END_SESSION_URL = 'https://issuer.example.com/logout';
export const NEW_REFRESH_TOKEN = 'new-refresh-token';
/** The harness runtime's OIDC client id. */
export const CLIENT_ID = 'paigasus-console';
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
}

/** No network. The code exchange always succeeds and returns NEW_REFRESH_TOKEN. */
export function fakeOidc(): FakeOidc {
  const oidc: FakeOidc = {
    revokeCalls: [],
    failRevoke: false,
    endSessionCalls: [],
    buildAuthorizationUrl: (): Promise<AuthorizationRequest> => Promise.resolve({ url: 'https://issuer.example.com/authorize?client_id=test', codeVerifier: 'a-verifier', nonce: 'a-nonce' }),
    authorizationCodeGrant: (): Promise<OidcTokens> =>
      Promise.resolve({
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

const HTML_ENTITIES: Readonly<Record<string, string>> = { '&amp;': '&', '&lt;': '<', '&gt;': '>', '&quot;': '"', '&#39;': "'" };

export function htmlDecode(value: string): string {
  return value.replace(/&(?:amp|lt|gt|quot|#39);/g, (entity) => HTML_ENTITIES[entity] ?? entity);
}

/** Asserts every § 3 property of a 503, and that the retry target is exactly `target`. */
export async function expectStoreUnavailable(res: Response, expected: { kind: 'link' | 'post'; target: string }): Promise<string> {
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
    expect(text).not.toContain('sentinel-pw');
    expect(text).not.toContain('redis.invalid');
  }
  return body;
}

/** No logged field of any event holds any part of SENTINEL_DSN. */
export function expectEventsClean(events: ReadonlyArray<[AuthEventName, AuthEventFields]>): void {
  const text = JSON.stringify(events);
  expect(text).not.toContain('sentinel-pw');
  expect(text).not.toContain('redis.invalid');
}

/** The fields of each `store.unavailable` event, in order. */
export function storeUnavailableEvents(events: ReadonlyArray<[AuthEventName, AuthEventFields]>): AuthEventFields[] {
  return events.filter(([name]) => name === 'store.unavailable').map(([, fields]) => fields);
}
