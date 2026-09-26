// SPDX-License-Identifier: Apache-2.0
//
// SMA-653 § 4: each auth route returns a 503 with a retry control when the session store is
// unavailable. One `describe` per route; each § 4 row runs once with SessionStoreUnavailable and
// once with its SessionStoreTimeout subclass. Every row also asserts redaction (the sentinel DSN
// in the thrown error reaches neither the response nor a log).
import { describe, expect, it, vi } from 'vitest';
import { hashSecret } from '../../src/core/ids.js';
import { SessionStoreUnavailable } from '../../src/core/errors.js';
import type { SessionRecord } from '../../src/core/session.js';
import { SESSION_COOKIE, txnCookieName } from '../../src/http/cookies.js';
import { createAuthRoutes } from '../../src/http/routes.js';
import { sidTag } from '../../src/ports/logger.js';
import {
  BASE_PATH,
  END_SESSION_URL,
  FAILURE_KINDS,
  FAKE_ID_TOKEN,
  NEW_REFRESH_TOKEN,
  ORIGIN,
  SENTINEL_DSN,
  expectEventsClean,
  expectStoreUnavailable,
  harness,
  storeError,
  storeUnavailableEvents,
  type Harness,
} from '../support/store-failure.js';

const RETURN_TO = '/iam/orgs';
const OLD_SID = 'old-session-id-0123456789';

function record(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    version: 2,
    rev: 0,
    accessToken: 'old-access-token',
    refreshToken: 'old-refresh-token',
    accessExpiresAt: Date.now() + 60_000,
    absoluteExpiresAt: Date.now() + 60_000,
    // A token logout would send as the hint, so row 6 shows that only the failed read stops it.
    idToken: FAKE_ID_TOKEN,
    idTokenClaims: { iss: 'https://issuer.example.com', sub: 'a-subject' },
    principal: { principalPrn: null, issuer: 'https://issuer.example.com', subject: 'a-subject', memberships: [], roleGrants: [], grantsAvailable: false },
    ...overrides,
  };
}

function loginRequest(returnTo: string, sid?: string): Request {
  const init = sid !== undefined ? { headers: { cookie: `${SESSION_COOKIE}=${sid}` } } : undefined;
  return new Request(`${ORIGIN}${BASE_PATH}/auth/login?returnTo=${encodeURIComponent(returnTo)}`, init);
}

describe.each(FAILURE_KINDS)('GET /auth/login with the store down (%s)', (kind) => {
  it('row 1, no session cookie: putTransaction fails -> 503, link to login with returnTo', async () => {
    const h = harness(['putTransaction'], () => storeError(kind));

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO));

    await expectStoreUnavailable(res, { kind: 'link', target: `/iam/auth/login?returnTo=${encodeURIComponent(RETURN_TO)}` });
    expect(storeUnavailableEvents(h.events)).toEqual([{ zone: 'iam', stage: 'login_put_transaction' }]);
    expect(h.events.map(([name]) => name)).not.toContain('login.started');
    expectEventsClean(h.events);
  });

  it('row 1, with a session cookie: the link goes to returnTo (D9), and the session survives', async () => {
    const h = harness(['putTransaction'], () => storeError(kind));
    await h.inner.set(OLD_SID, record(), 60_000, null);

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO, OLD_SID));

    await expectStoreUnavailable(res, { kind: 'link', target: RETURN_TO });
    expect(h.calls).not.toContain(`delete:${OLD_SID}`);
    expect(await h.inner.get(OLD_SID)).not.toBeNull();
    expectEventsClean(h.events);
  });

  // Row 2 needs a presented sid: with no session cookie, handleLogin makes no delete call at all.
  it('row 2: the delete of the presented session fails -> 503, link to returnTo (D9)', async () => {
    const h = harness(['delete'], () => storeError(kind));
    await h.inner.set(OLD_SID, record(), 60_000, null);

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO, OLD_SID));

    await expectStoreUnavailable(res, { kind: 'link', target: RETURN_TO });
    expect(storeUnavailableEvents(h.events)).toEqual([{ zone: 'iam', stage: 'login_delete', sid: sidTag(OLD_SID) }]);
    expect(h.events.map(([name]) => name)).not.toContain('login.started');
    expectEventsClean(h.events);
  });
});

describe('GET /auth/login — escaping and classification', () => {
  const HOSTILE = `/iam/a"b<c>d&e'f#g h`;

  it('round-trips a hostile returnTo through the login link', async () => {
    const h = harness(['putTransaction'], () => storeError('unavailable'));
    const res = await createAuthRoutes(h.runtime).handle(loginRequest(HOSTILE));
    const body = await expectStoreUnavailable(res, { kind: 'link', target: `/iam/auth/login?returnTo=${encodeURIComponent(HOSTILE)}` });
    // encodeURIComponent leaves ' as is, so the HTML layer must escape it.
    expect(body).toContain('&#39;');
    const href = /href="([^"]*)"/.exec(body)?.[1] ?? '';
    expect(new URL(href.replace(/&#39;/g, "'").replace(/&amp;/g, '&'), ORIGIN).searchParams.get('returnTo')).toBe(HOSTILE);
    expectEventsClean(h.events);
  });

  it('keeps a hostile returnTo inert in the D9 link', async () => {
    const h = harness(['putTransaction'], () => storeError('unavailable'));
    await h.inner.set(OLD_SID, record(), 60_000, null);
    const res = await createAuthRoutes(h.runtime).handle(loginRequest(HOSTILE, OLD_SID));
    const body = await expectStoreUnavailable(res, { kind: 'link', target: HOSTILE });
    expect(body).toContain('href="/iam/a&quot;b&lt;c&gt;d&amp;e&#39;f#g h"');
    expectEventsClean(h.events);
  });

  it('maps an error from a SECOND copy of core/errors (D2)', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    expect(foreign.SessionStoreUnavailable).not.toBe(SessionStoreUnavailable);
    const h = harness(['putTransaction'], () => new foreign.SessionStoreUnavailable(`session store unavailable (${SENTINEL_DSN})`));

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO));

    await expectStoreUnavailable(res, { kind: 'link', target: `/iam/auth/login?returnTo=${encodeURIComponent(RETURN_TO)}` });
    expectEventsClean(h.events);
  });

  it('lets any other store error propagate (D2)', async () => {
    const h = harness(['putTransaction'], () => new Error('boom'));
    await expect(createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO))).rejects.toThrow('boom');
  });
});

const STATE = 'state-0123456789';
const TXN_SECRET = 'correct-secret-value-32-bytes-ok';

async function seedTransaction(h: Harness): Promise<void> {
  await h.inner.putTransaction(STATE, { codeVerifier: 'a-verifier', nonce: 'a-nonce', returnTo: RETURN_TO, secretHash: hashSecret(TXN_SECRET), createdAt: Date.now() }, 600_000);
}

function callbackRequest(sid?: string): Request {
  const cookies = [`${txnCookieName(STATE)}=${TXN_SECRET}`, ...(sid !== undefined ? [`${SESSION_COOKIE}=${sid}`] : [])];
  return new Request(`${ORIGIN}${BASE_PATH}/auth/callback?code=a-code&state=${STATE}`, { headers: { cookie: cookies.join('; ') } });
}

describe.each(FAILURE_KINDS)('GET /auth/callback with the store down (%s)', (kind) => {
  it('row 3: takeTransaction fails -> 503, link to login, no exchange, no revoke', async () => {
    const h = harness(['takeTransaction'], () => storeError(kind));
    await seedTransaction(h);

    const res = await createAuthRoutes(h.runtime).handle(callbackRequest());

    await expectStoreUnavailable(res, { kind: 'link', target: '/iam/auth/login' });
    expect(storeUnavailableEvents(h.events)).toEqual([{ zone: 'iam', stage: 'callback_take_transaction' }]);
    expect(h.calls).toEqual([`takeTransaction:${STATE}`]);
    expect(h.oidc.revokeCalls).toEqual([]);
    expectEventsClean(h.events);
  });

  it('row 4: the delete of the presented session fails -> revoke the new token, then 503', async () => {
    const h = harness(['delete'], () => storeError(kind));
    await seedTransaction(h);

    const res = await createAuthRoutes(h.runtime).handle(callbackRequest(OLD_SID));

    await expectStoreUnavailable(res, { kind: 'link', target: `/iam/auth/login?returnTo=${encodeURIComponent(RETURN_TO)}` });
    expect(storeUnavailableEvents(h.events)).toEqual([{ zone: 'iam', stage: 'callback_delete', sid: sidTag(OLD_SID) }]);
    expect(h.oidc.revokeCalls).toEqual([NEW_REFRESH_TOKEN]);
    expect(h.calls.some((call) => call.startsWith('set:'))).toBe(false);
    expect(h.events.map(([name]) => name)).not.toContain('session.created');
    expectEventsClean(h.events);
  });

  it('row 5: the set of the new session fails -> revoke the new token, then 503', async () => {
    const h = harness(['set'], () => storeError(kind));
    await seedTransaction(h);

    const res = await createAuthRoutes(h.runtime).handle(callbackRequest());

    await expectStoreUnavailable(res, { kind: 'link', target: `/iam/auth/login?returnTo=${encodeURIComponent(RETURN_TO)}` });
    const setCall = h.calls.find((call) => call.startsWith('set:'));
    expect(setCall).toBeDefined();
    const newSid = (setCall ?? '').slice('set:'.length);
    expect(storeUnavailableEvents(h.events)).toEqual([{ zone: 'iam', stage: 'callback_set', sid: sidTag(newSid) }]);
    expect(h.oidc.revokeCalls).toEqual([NEW_REFRESH_TOKEN]);
    expect(h.events.map(([name]) => name)).not.toContain('session.created');
    expectEventsClean(h.events);
  });

  it('rows 4 and 5: a failing revoke changes nothing in the response or the log', async () => {
    const h = harness(['set'], () => storeError(kind));
    h.oidc.failRevoke = true;
    await seedTransaction(h);

    const res = await createAuthRoutes(h.runtime).handle(callbackRequest());

    await expectStoreUnavailable(res, { kind: 'link', target: `/iam/auth/login?returnTo=${encodeURIComponent(RETURN_TO)}` });
    expect(h.oidc.revokeCalls).toEqual([NEW_REFRESH_TOKEN]);
    expect(storeUnavailableEvents(h.events).map((fields) => fields.stage)).toEqual(['callback_set']);
    expectEventsClean(h.events);
  });
});

function logoutRequest(sid: string): Request {
  return new Request(`${ORIGIN}${BASE_PATH}/auth/logout`, { method: 'POST', headers: { cookie: `${SESSION_COOKIE}=${sid}` } });
}

describe.each(FAILURE_KINDS)('POST /auth/logout with the store down (%s)', (kind) => {
  it('row 6: the read fails -> the delete still runs, and logout completes (D5)', async () => {
    const h = harness(['get'], () => storeError(kind));
    await h.inner.set(OLD_SID, record(), 60_000, null);

    const res = await createAuthRoutes(h.runtime).handle(logoutRequest(OLD_SID));

    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toBe(END_SESSION_URL);
    expect(res.headers.getSetCookie().some((cookie) => cookie.startsWith(`${SESSION_COOKIE}=;`))).toBe(true);
    expect(h.calls).toEqual([`get:${OLD_SID}`, `delete:${OLD_SID}`]);
    expect(await h.inner.get(OLD_SID)).toBeNull();
    expect(h.oidc.revokeCalls).toEqual([]);
    expect(h.events).toEqual([
      ['store.unavailable', { zone: 'iam', stage: 'logout_get', sid: sidTag(OLD_SID) }],
      ['logout.completed', { zone: 'iam', sid: sidTag(OLD_SID), revoked: false, endSessionRedirected: true, idTokenHintSent: false }],
    ]);
    expectEventsClean(h.events);
    // SMA-681 AC 3: a failed read gives no token, so the end-session request carries no hint.
    expect(h.oidc.endSessionCalls).toHaveLength(1);
    expect(h.oidc.endSessionCalls[0]).not.toHaveProperty('idTokenHint');
  });

  it('row 7: the delete fails -> revoke the read token, 503 with a POST form, cookie kept (D6)', async () => {
    const h = harness(['delete'], () => storeError(kind));
    await h.inner.set(OLD_SID, record(), 60_000, null);

    const res = await createAuthRoutes(h.runtime).handle(logoutRequest(OLD_SID));

    await expectStoreUnavailable(res, { kind: 'post', target: '/iam/auth/logout' });
    expect(h.oidc.revokeCalls).toEqual(['old-refresh-token']);
    expect(await h.inner.get(OLD_SID)).not.toBeNull();
    expect(h.events).toEqual([['store.unavailable', { zone: 'iam', stage: 'logout_delete', sid: sidTag(OLD_SID) }]]);
    expectEventsClean(h.events);
    // Review Focus 2: the 503 branch does not redirect to the IdP, so no hint leaves the server.
    expect(h.oidc.endSessionCalls).toEqual([]);
  });

  it('row 7: a failing revoke still gives the same 503, and logs nothing more', async () => {
    const h = harness(['delete'], () => storeError(kind));
    h.oidc.failRevoke = true;
    await h.inner.set(OLD_SID, record(), 60_000, null);

    const res = await createAuthRoutes(h.runtime).handle(logoutRequest(OLD_SID));

    await expectStoreUnavailable(res, { kind: 'post', target: '/iam/auth/logout' });
    expect(h.oidc.revokeCalls).toEqual(['old-refresh-token']);
    expect(h.events).toEqual([['store.unavailable', { zone: 'iam', stage: 'logout_delete', sid: sidTag(OLD_SID) }]]);
    expectEventsClean(h.events);
  });

  it('row 8: the read and the delete both fail -> 503, no revoke, two events in order', async () => {
    const h = harness(['get', 'delete'], () => storeError(kind));
    await h.inner.set(OLD_SID, record(), 60_000, null);

    const res = await createAuthRoutes(h.runtime).handle(logoutRequest(OLD_SID));

    await expectStoreUnavailable(res, { kind: 'post', target: '/iam/auth/logout' });
    expect(h.oidc.revokeCalls).toEqual([]);
    expect(h.calls).toEqual([`get:${OLD_SID}`, `delete:${OLD_SID}`]);
    expect(h.events).toEqual([
      ['store.unavailable', { zone: 'iam', stage: 'logout_get', sid: sidTag(OLD_SID) }],
      ['store.unavailable', { zone: 'iam', stage: 'logout_delete', sid: sidTag(OLD_SID) }],
    ]);
    expectEventsClean(h.events);
  });
});
