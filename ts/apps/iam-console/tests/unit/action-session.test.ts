// SPDX-License-Identifier: Apache-2.0
//
// The action path reads the session WITHOUT a redirect (final whole-branch review, Important 1).
//
// THE DEFECT THIS PINS. requireSession() redirects basePath-RELATIVE, which is right for a page
// render, because Next's app render adds the basePath. A Server Action does not go through that
// render: action-handler.js writes the RAW url into `x-action-redirect` and into `Location`, so the
// browser leaves the /iam zone. So iamClientsForAction() must RETURN a relogin failure, and
// iamClients() must keep redirecting for pages. The two cases below run under the SAME conditions
// and must differ, which is what makes this test a control rather than a restatement.
import { describe, expect, it, vi } from 'vitest';
import type { SessionRecord } from '@paigasus/auth/server';
import { setRequestCookies, setRequestHeaders } from '../support/next-headers';

// The runtime is built in a vi.hoisted block: the factory below runs while lib/iam.ts's own import
// graph is evaluated, which is before this file's body.
const { runtime, store } = vi.hoisted(() => {
  const records = new Map<string, unknown>();
  return {
    records,
    store: { get: (sid: string) => Promise.resolve(records.get(sid) ?? null), put: () => Promise.resolve(), delete: () => Promise.resolve() },
    runtime: {
      basePath: '/iam',
      logger: { event: () => undefined },
      skewMs: 30_000,
      lockTtlMs: 10_000,
      lockWaitMs: 3_000,
      ttlMs: 28_800_000,
      oidc: {},
      records,
    },
  };
});

vi.mock('../../lib/auth', () => ({
  SESSION_COOKIE_NAME: '__Host-pgs_sid',
  authRuntime: () => Promise.resolve({ ...runtime, store }),
}));

const { iamClients, iamClientsForAction, optionalSession } = await import('../../lib/iam');

const SID = 'sid-for-the-action-path';

function signedIn(): void {
  const record: SessionRecord = {
    version: 1,
    sid: SID,
    principalPrn: 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0',
    subject: 'sub-1',
    issuer: 'https://idp.example.test',
    accessToken: 'access-token-1',
    refreshToken: 'refresh-token-1',
    idToken: 'id-token-1',
    accessExpiresAt: Date.now() + 3_600_000,
    absoluteExpiresAt: Date.now() + 86_400_000,
    createdAt: Date.now(),
    memberships: [],
    grants: [],
  } as unknown as SessionRecord;
  runtime.records.set(SID, record);
  setRequestCookies({ '__Host-pgs_sid': SID });
}

describe('iamClientsForAction (the Server Action session read)', () => {
  it('returns a relogin failure instead of redirecting when no cookie is present', async () => {
    const result = await iamClientsForAction();

    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error.presentation).toBe('relogin');
    // No IAM data: this error never reached IAM, so FormError shows the SignInAgain link and no id.
    expect(result.error.correlationId).toBeNull();
    expect(result.error.reason).toBeNull();
  });

  // The realistic shape of an expired session: the browser still sends the cookie, and the STORE
  // record it names is gone (get-session.ts's file header).
  it('returns a relogin failure when the cookie names a record the store no longer holds', async () => {
    setRequestCookies({ '__Host-pgs_sid': 'sid-the-store-forgot' });

    const result = await iamClientsForAction();

    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error.presentation).toBe('relogin');
  });

  it('returns the five clients when the session resolves', async () => {
    vi.stubEnv('PAIGASUS_IAM_GRPC_URL', 'http://iam.internal:9090');
    vi.stubEnv('PAIGASUS_ZONE', 'iam');
    vi.stubEnv('PAIGASUS_ZONES', '{"iam":"/iam"}');
    vi.stubEnv('PAIGASUS_OIDC_ISSUER', 'https://idp.example.test');
    vi.stubEnv('PAIGASUS_OIDC_CLIENT_ID', 'console');
    vi.stubEnv('PAIGASUS_OIDC_CLIENT_SECRET', 'console-secret');
    vi.stubEnv('PAIGASUS_PUBLIC_ORIGIN', 'https://console.example.test');
    vi.stubEnv('PAIGASUS_SESSION_STORE', 'memory');
    vi.stubEnv('PAIGASUS_SERVICES', '{"iam":"http://iam.internal:8080"}');
    setRequestHeaders({});
    signedIn();

    const result = await iamClientsForAction();

    expect(result.ok).toBe(true);
    if (result.ok) expect(Object.keys(result.value).sort()).toEqual(['audit', 'authn', 'authz', 'serviceInfo', 'tenancy']);
    vi.unstubAllEnvs();
  });

  it('reads the session through getSession, so optionalSession never redirects either', async () => {
    await expect(optionalSession()).resolves.toBeNull();
  });

  // THE CONTRAST. Same missing session, same request scope: the PAGE accessor still throws Next's
  // redirect. Make iamClientsForAction() call requireSession() and this file's first case throws
  // here instead of returning data.
  it('is the opposite of iamClients(), which still redirects for a page render', async () => {
    const error = await iamClients().then(
      () => null,
      (err: unknown) => err,
    );

    expect(error).not.toBeNull();
    const digest = (error as { digest?: string }).digest ?? '';
    expect(digest).toContain('NEXT_REDIRECT');
    // basePath-RELATIVE, because Next's app render adds /iam itself (spec § 7.1, § 13 row 1).
    expect(digest).toContain('/auth/login');
    expect(digest).not.toContain('/iam/auth/login');
  });
});
