// SPDX-License-Identifier: Apache-2.0
//
// SMA-653 § 4: each auth route returns a 503 with a retry control when the session store is
// unavailable. One `describe` per route; each § 4 row runs once with SessionStoreUnavailable and
// once with its SessionStoreTimeout subclass. Every row also asserts redaction (the sentinel DSN
// in the thrown error reaches neither the response nor a log).
import { describe, expect, it, vi } from 'vitest';
import { SessionStoreUnavailable } from '../../src/core/errors.js';
import type { SessionRecord } from '../../src/core/session.js';
import { SESSION_COOKIE } from '../../src/http/cookies.js';
import { createAuthRoutes } from '../../src/http/routes.js';
import { sidTag } from '../../src/ports/logger.js';
import { BASE_PATH, FAILURE_KINDS, ORIGIN, SENTINEL_DSN, expectEventsClean, expectStoreUnavailable, harness, storeError, storeUnavailableEvents } from '../support/store-failure.js';

const RETURN_TO = '/iam/orgs';
const OLD_SID = 'old-session-id-0123456789';

function record(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    version: 1,
    rev: 0,
    accessToken: 'old-access-token',
    refreshToken: 'old-refresh-token',
    accessExpiresAt: Date.now() + 60_000,
    absoluteExpiresAt: Date.now() + 60_000,
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
  });

  it('keeps a hostile returnTo inert in the D9 link', async () => {
    const h = harness(['putTransaction'], () => storeError('unavailable'));
    await h.inner.set(OLD_SID, record(), 60_000, null);
    const res = await createAuthRoutes(h.runtime).handle(loginRequest(HOSTILE, OLD_SID));
    const body = await expectStoreUnavailable(res, { kind: 'link', target: HOSTILE });
    expect(body).toContain('href="/iam/a&quot;b&lt;c&gt;d&amp;e&#39;f#g h"');
  });

  it('maps an error from a SECOND copy of core/errors (D2)', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    expect(foreign.SessionStoreUnavailable).not.toBe(SessionStoreUnavailable);
    const h = harness(['putTransaction'], () => new foreign.SessionStoreUnavailable(`session store unavailable (${SENTINEL_DSN})`));

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO));

    await expectStoreUnavailable(res, { kind: 'link', target: `/iam/auth/login?returnTo=${encodeURIComponent(RETURN_TO)}` });
  });

  it('lets any other store error propagate (D2)', async () => {
    const h = harness(['putTransaction'], () => new Error('boom'));
    await expect(createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO))).rejects.toThrow('boom');
  });
});
