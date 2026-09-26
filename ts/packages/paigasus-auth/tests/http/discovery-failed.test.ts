// SPDX-License-Identifier: Apache-2.0
//
// SMA-656: /auth/login and /auth/callback answer an OIDC discovery failure with a 503 that names
// the identity provider (spec D1-D10; T1-T7, T12-T15). The header of store-unavailable.test.ts
// limits that file to store failures, so the discovery rows live here. The fake OIDC client rejects
// with a constructed error; tests/adapters/oidc.test.ts measures the real adapter's errors (T9).
//
// Every error a row throws carries SENTINEL_IDP_URL in its message, and every row that produces a
// response asserts that no IDP_SENTINELS string reaches the body, a header or a logged field (T4).
import { describe, expect, it, vi } from 'vitest';
import { OIDC_DISCOVERY_FAILURE_REASONS, OidcDiscoveryFailed, type OidcDiscoveryFailureReason } from '../../src/core/errors.js';
import type { SessionRecord } from '../../src/core/session.js';
import { SESSION_COOKIE } from '../../src/http/cookies.js';
import { createAuthRoutes } from '../../src/http/routes.js';
import { BASE_PATH, IDP_SENTINELS, ORIGIN, SENTINEL_IDP_URL, expectEventsClean, expectStoreUnavailable, harness } from '../support/store-failure.js';

const RETURN_TO = '/iam/orgs';
const HOSTILE = `/iam/a"b<c>d&e'f#g h`;
const OLD_SID = 'old-session-id-0123456789';
const LOGIN_TARGET = `/iam/auth/login?returnTo=${encodeURIComponent(RETURN_TO)}`;
const IDP_SENTENCE = 'The identity provider is not available. Try again in a few seconds.';
const SESSION_SENTENCE = 'The session service did not answer.';

/** No store call fails in these rows. The wrapper still records each call in `h.calls`. */
const noStoreFailure = (): Error => new Error('no store call fails in the SMA-656 rows');

function discoveryError(reason: OidcDiscoveryFailureReason): OidcDiscoveryFailed {
  return new OidcDiscoveryFailed(`oidc discovery failed at ${SENTINEL_IDP_URL}`, reason);
}

function sessionRecord(): SessionRecord {
  return {
    version: 2,
    rev: 0,
    accessToken: 'old-access-token',
    refreshToken: 'old-refresh-token',
    accessExpiresAt: Date.now() + 60_000,
    absoluteExpiresAt: Date.now() + 60_000,
    idToken: 'old-id-token',
    idTokenClaims: { iss: 'https://issuer.example.com', sub: 'a-subject' },
    principal: { principalPrn: null, issuer: 'https://issuer.example.com', subject: 'a-subject', memberships: [], roleGrants: [], grantsAvailable: false },
  };
}

function loginRequest(returnTo: string, sid?: string): Request {
  const init = sid !== undefined ? { headers: { cookie: `${SESSION_COOKIE}=${sid}` } } : undefined;
  return new Request(`${ORIGIN}${BASE_PATH}/auth/login?returnTo=${encodeURIComponent(returnTo)}`, init);
}

/** The IdP 503: every D5 header, no Set-Cookie, the target, the D4 sentence and NOT the session sentence. */
async function expectIdpUnavailable(res: Response, target: string): Promise<string> {
  const body = await expectStoreUnavailable(res, { kind: 'link', target }, IDP_SENTINELS);
  expect(body).toContain('<h1>Sign-in is temporarily unavailable</h1>');
  expect(body).toContain(IDP_SENTENCE);
  expect(body).not.toContain(SESSION_SENTENCE);
  return body;
}

describe('GET /auth/login — OIDC discovery failed (SMA-656)', () => {
  it.each(OIDC_DISCOVERY_FAILURE_REASONS)('T1, T3, T4, no session cookie (%s): 503, link to login with returnTo', async (reason) => {
    const h = harness([], noStoreFailure);
    h.oidc.authorizationError = discoveryError(reason);

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO));

    await expectIdpUnavailable(res, LOGIN_TARGET);
    // D9: no store call, so no transaction and no delete. Exactly one event, and no login.started.
    expect(h.calls).toEqual([]);
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'login', reason }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });

  it.each(OIDC_DISCOVERY_FAILURE_REASONS)('T2, T3, T4, with a session cookie (%s): the link goes to returnTo, the session survives', async (reason) => {
    const h = harness([], noStoreFailure);
    await h.inner.set(OLD_SID, sessionRecord(), 60_000, null);
    h.oidc.authorizationError = discoveryError(reason);

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO, OLD_SID));

    await expectIdpUnavailable(res, RETURN_TO);
    expect(h.calls).toEqual([]);
    expect(await h.inner.get(OLD_SID)).not.toBeNull();
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'login', reason }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });

  it('T5: a plain Error (the build_authorization_url case) propagates, and nothing is logged', async () => {
    const h = harness([], noStoreFailure);
    const boom = new Error('oidc build_authorization_url failed: TypeError');
    h.oidc.authorizationError = boom;

    await expect(createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO))).rejects.toBe(boom);

    expect(h.events).toEqual([]);
    expect(h.calls).toEqual([]);
  });

  it('T6a: an OidcDiscoveryFailed from a SECOND copy of core/errors gives the 503 (D1)', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    // Precondition: without this, the test passes vacuously if the import returns the same module.
    expect(foreign.OidcDiscoveryFailed).not.toBe(OidcDiscoveryFailed);
    const h = harness([], noStoreFailure);
    h.oidc.authorizationError = new foreign.OidcDiscoveryFailed(`oidc discovery failed at ${SENTINEL_IDP_URL}`, 'dns');

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO));

    await expectIdpUnavailable(res, LOGIN_TARGET);
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'login', reason: 'dns' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });

  it('T6b: a plain Error with the code and no reason gives the 503 with reason other', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.authorizationError = Object.assign(new Error(`oidc discovery failed at ${SENTINEL_IDP_URL}`), { code: 'oidc_discovery_failed' });

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO));

    await expectIdpUnavailable(res, LOGIN_TARGET);
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'login', reason: 'other' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });

  it.each([
    ['a URL', 'https://idp.invalid/x'],
    ['a near miss', 'TIMEOUT'],
    ['an empty string', ''],
    ['a number', 42],
  ] as const)('T7: a reason outside the list (%s) is logged as other', async (_label, reason) => {
    const h = harness([], noStoreFailure);
    h.oidc.authorizationError = Object.assign(new Error(`oidc discovery failed at ${SENTINEL_IDP_URL}`), { code: 'oidc_discovery_failed', reason });

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO));

    await expectIdpUnavailable(res, LOGIN_TARGET);
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'login', reason: 'other' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });

  // Review Focus 1.
  it('round-trips a hostile returnTo through the IdP login link', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.authorizationError = discoveryError('network');

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(HOSTILE));

    const body = await expectIdpUnavailable(res, `/iam/auth/login?returnTo=${encodeURIComponent(HOSTILE)}`);
    // encodeURIComponent leaves ' as is, so the HTML layer must escape it.
    expect(body).toContain('&#39;');
  });

  it('keeps a hostile returnTo inert in the D6 link', async () => {
    const h = harness([], noStoreFailure);
    await h.inner.set(OLD_SID, sessionRecord(), 60_000, null);
    h.oidc.authorizationError = discoveryError('network');

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(HOSTILE, OLD_SID));

    const body = await expectIdpUnavailable(res, HOSTILE);
    expect(body).toContain('href="/iam/a&quot;b&lt;c&gt;d&amp;e&#39;f#g h"');
  });
});
