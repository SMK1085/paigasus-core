// SPDX-License-Identifier: Apache-2.0
//
// Helpers for the tier-2 tests (spec § 9.3, task 4). Unlike iam-console, this app wires its pages
// straight to lib/console.ts's runtime (no per-route load.ts — Task 3), so a test that exercises a
// page must go through the REAL currentSession()/discovery()/iamClients(), not an injected fake.
//
// A session is installed by writing a record straight into the process's SessionStore and pointing
// the tests/support/next-headers.ts double's cookie jar at it — no fake IdP and no OIDC round trip,
// the same technique @paigasus/auth's own tests/next/get-session.test.ts uses. That store comes from
// lib/auth.ts's `authRuntime()`, which is @paigasus/auth's getAuthRuntime() underneath: a promise
// cached ONCE PER ZONE on `globalThis` (Symbol.for-keyed) for the life of this module registry, so
// every call in this test file — the pages under test included — resolves the SAME runtime and the
// SAME store.
import { randomUUID } from 'node:crypto';
import { SESSION_COOKIE, type SessionRecord } from '@paigasus/auth/server';
import { startFakeGateway, startFakeIam, type FakeGateway, type FakeIam } from '@paigasus/console-core/testing';
import { authRuntime } from '../../lib/auth';
import { setRequestCookies } from '../support/next-headers';
import { stubConsoleEnv } from '../support/env';

/** Fixed UUIDs so a test's expectations read as data, not as `expect.any(String)`. */
export const IDS = {
  orgA: '0190a100-0000-7000-8000-00000000000a',
  orgB: '0190a100-0000-7000-8000-00000000000b',
} as const;

export type IntegrationEnv = { readonly iam: FakeIam; readonly gateway: FakeGateway };

/**
 * Starts a fake IAM and a fake gateway, and stubs the environment so lib/config.ts parses against
 * them: `PAIGASUS_SERVICES` carrying both, `PAIGASUS_IAM_GRPC_URL` the fake IAM's gRPC address, and
 * the three `PAIGASUS_DISCOVERY_*_MS` values at 1/2/3 ms so every render re-probes rather than
 * serving a cached descriptor from an earlier case.
 *
 * `getRuntimeConfig()` memoizes its first successful parse for the life of the module
 * (lib/config.ts's own header), so this must run — and env must be stubbed — before anything in the
 * test file reads config, i.e. from a `beforeAll`.
 */
export async function startIntegrationEnv(): Promise<IntegrationEnv> {
  const [iam, gateway] = await Promise.all([startFakeIam(), startFakeGateway()]);
  stubConsoleEnv({
    PAIGASUS_SERVICES: JSON.stringify({ iam: iam.httpUrl, gateway: gateway.url }),
    PAIGASUS_IAM_GRPC_URL: iam.grpcUrl,
    PAIGASUS_DISCOVERY_NEGATIVE_MS: '1',
    PAIGASUS_DISCOVERY_FRESH_MS: '2',
    PAIGASUS_DISCOVERY_STALE_MS: '3',
  });
  return { iam, gateway };
}

export async function stopIntegrationEnv(env: IntegrationEnv): Promise<void> {
  await Promise.all([env.iam.close(), env.gateway.close()]);
}

const SESSION_TTL_MS = 999_000;

/**
 * Writes a fresh, live session record into the runtime's store and points the cookie double at its
 * sid. A FRESH sid every call: `SessionStore.set`'s `expectedRev: null` inserts only when no record
 * exists yet (ports/session-store.ts), so reusing one sid across calls would silently no-op on the
 * second install. Returns the bearer token every IAM/gateway call in the test will carry —
 * `lib/console.ts`'s `iamClients()`/`discovery()` pass `session.accessToken` straight through.
 */
export async function installSession(token = 'tok-integration'): Promise<string> {
  const runtime = await authRuntime();
  const sid = randomUUID();
  const now = Date.now();
  const record: SessionRecord = {
    version: 1,
    rev: 0,
    accessToken: token,
    accessExpiresAt: now + SESSION_TTL_MS,
    absoluteExpiresAt: now + SESSION_TTL_MS,
    idTokenClaims: { iss: 'https://idp.example.test', sub: 'integration-subject' },
    principal: { principalPrn: null, issuer: 'https://idp.example.test', subject: 'integration-subject', memberships: [], roleGrants: [], grantsAvailable: false },
  };
  const inserted = await runtime.store.set(sid, record, SESSION_TTL_MS, null);
  if (!inserted) throw new Error('installSession: the store refused the insert (an sid collision — this should never happen)');
  setRequestCookies({ [SESSION_COOKIE]: sid });
  return token;
}
