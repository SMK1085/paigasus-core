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
import { describe, expect, it } from 'vitest';
import type { AuthRuntime, SessionRecord } from '@paigasus/auth/server';
import { setConsolePorts } from '@paigasus/console-core';
import { setRequestCookies, setRequestHeaders } from '../support/next-headers';

// SMA-512 PR 2, task 4: lib/iam.ts is now a barrel over @paigasus/console-core's iam.ts, which
// reads authRuntime() and the IAM gRPC URL through the package's runtime-ports seam rather than
// importing lib/auth.ts directly, so a `vi.mock('../../lib/auth', …)` no longer reaches it. This
// file wires the port directly instead — the same shape task 5's createConsoleRuntime() will wire
// for real.
//
// ORDER MATTERS (task 4 fix round 1). lib/iam.ts's barrel now carries `import './auth'` for its
// own side effect (see its comment), so importing it runs the app's REAL setConsolePorts() call —
// the one that reads the real, unstubbed environment. Importing lib/iam.ts must therefore happen
// BEFORE this file's own setConsolePorts() call below, so the fake one is what is left in place
// when an `it()` runs. Getting this backwards was measured to fail all five cases: the app's real
// authRuntime()/getRuntimeConfig() clobbers the fake, and every accessor then tries to read a real,
// unconfigured environment.
const { iamClients, iamClientsForAction, optionalSession } = await import('../../lib/iam');

const records = new Map<string, unknown>();
const store = { get: (sid: string) => Promise.resolve(records.get(sid) ?? null), put: () => Promise.resolve(), delete: () => Promise.resolve() };
const runtime = {
  basePath: '/iam',
  logger: { event: () => undefined },
  skewMs: 30_000,
  lockTtlMs: 10_000,
  lockWaitMs: 3_000,
  ttlMs: 28_800_000,
  oidc: {},
  records,
};

setConsolePorts({
  // A partial fake, like SessionRecord's below: only the fields requireSession()/getSession()
  // actually read. vi.mock's factory used to hide this from tsc; a real function value does not.
  authRuntime: () => Promise.resolve({ ...runtime, store } as unknown as AuthRuntime),
  config: () => ({
    PAIGASUS_IAM_GRPC_URL: 'http://iam.internal:9090',
    PAIGASUS_SESSION_STORE: 'memory',
    PAIGASUS_SESSION_REDIS_URL: undefined,
    PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 1000,
    PAIGASUS_SERVICES: { iam: 'http://iam.internal:8080' },
    PAIGASUS_DISCOVERY_NEGATIVE_MS: 1000,
    PAIGASUS_DISCOVERY_FRESH_MS: 5000,
    PAIGASUS_DISCOVERY_STALE_MS: 30_000,
    PAIGASUS_DISCOVERY_PROBE_TIMEOUT_MS: 2000,
    PAIGASUS_DISCOVERY_LOCK_WAIT_MS: 3000,
    PAIGASUS_DISCOVERY_LOCK_TTL_MS: 10_000,
  }),
});

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

  // SMA-512 PR 2, task 4: iamClientsForToken() now reads PAIGASUS_IAM_GRPC_URL through the
  // config() port set up above, not through the app's own getRuntimeConfig() — so this case no
  // longer needs to stub a full, schema-valid environment.
  it('returns the five clients when the session resolves', async () => {
    setRequestHeaders({});
    signedIn();

    const result = await iamClientsForAction();

    expect(result.ok).toBe(true);
    if (result.ok) expect(Object.keys(result.value).sort()).toEqual(['audit', 'authn', 'authz', 'serviceInfo', 'tenancy']);
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
