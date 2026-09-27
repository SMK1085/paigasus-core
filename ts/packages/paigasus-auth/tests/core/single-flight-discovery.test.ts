// SPDX-License-Identifier: Apache-2.0
//
// SMA-704 test 13, the measurement (spec § 5.4). A REAL createOidcClient on a COLD client, driven
// through resolveSession, against the fixture IdP (tests/fixtures/jwks.ts). The fixture logs the
// arrival time of each discovery, token and JWKS request. A store wrapper logs when the session
// lock was taken and released.
//
// The pass or fail assertions are CAUSAL, not durations: which requests arrived while the lock was
// held. The delays (discovery 0.6T, token and JWKS 0.3T each, T = 1000 ms) only make the
// durations readable. The durations are printed, not asserted; the spec's § 7 records them.
//
// `depsFor` returns its object from a function, so tsc does not check it for excess members. That
// is how this file compiled against the code BEFORE the fix, when ResolveDeps had no
// `prepareRefresh` and resolveSession ignored it (plan Task 2).
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { createOidcClient, type OidcClient } from '../../src/adapters/oidc.js';
import { isOidcDiscoveryFailed } from '../../src/core/errors.js';
import { resolveSession } from '../../src/core/single-flight.js';
import type { AuthEventFields, AuthEventName, AuthLogger } from '../../src/ports/logger.js';
import { sidTag } from '../../src/ports/logger.js';
import type { SessionStore } from '../../src/ports/session-store.js';
import { startDiscoveryFailureFixture, type DiscoveryFailureFixture } from '../fixtures/discovery-failures.js';
import { FIXTURE_CLIENT_ID, FIXTURE_CLIENT_SECRET, startOidcFixture, type FixtureEndpoint, type OidcFixture } from '../fixtures/jwks.js';
import { makeRecord } from '../store-contract.js';

const T = 1000;
const DISCOVERY_DELAY_MS = 0.6 * T;
const TOKEN_DELAY_MS = 0.3 * T;
const JWKS_DELAY_MS = 0.3 * T;

/** When the lock was taken and released, and how often tryAcquireLock was called. */
interface LockWindow {
  acquiredAt: number | undefined;
  releasedAt: number | undefined;
  attempts: number;
}

function newLockWindow(): LockWindow {
  return { acquiredAt: undefined, releasedAt: undefined, attempts: 0 };
}

/** Wraps a real store. Records the lock window in `window`. */
function lockTimingStore(inner: MemorySessionStore, window: LockWindow): SessionStore {
  return {
    get: (sid) => inner.get(sid),
    set: (sid, rec, ttlMs, expectedRev) => inner.set(sid, rec, ttlMs, expectedRev),
    delete: (sid) => inner.delete(sid),
    tryAcquireLock: async (sid, token, ttlMs) => {
      window.attempts += 1;
      const won = await inner.tryAcquireLock(sid, token, ttlMs);
      if (won) window.acquiredAt = Date.now();
      return won;
    },
    releaseLock: (sid, token) => {
      window.releasedAt = Date.now();
      return inner.releaseLock(sid, token);
    },
    putTransaction: (txnId, tx, ttlMs) => inner.putTransaction(txnId, tx, ttlMs),
    takeTransaction: (txnId) => inner.takeTransaction(txnId),
    close: () => inner.close(),
  };
}

function recordingLogger(): { logger: AuthLogger; events: Array<[AuthEventName, AuthEventFields]> } {
  const events: Array<[AuthEventName, AuthEventFields]> = [];
  return { logger: { event: (name, fields) => void events.push([name, { ...fields }]) }, events };
}

/** A new, cold client: it has sent no discovery request yet. */
function coldClient(issuer: string, httpTimeoutMs: number): OidcClient {
  return createOidcClient({
    issuer,
    clientId: FIXTURE_CLIENT_ID,
    clientSecret: FIXTURE_CLIENT_SECRET,
    httpTimeoutMs,
    clockToleranceSeconds: 30,
    scopes: 'openid profile email offline_access',
    allowInsecureRequests: true, // the fixtures are plain http on localhost — never set in production
  });
}

/** The production wiring's shape (next/get-session.ts resolveDepsFor): all three IdP calls go to ONE client. */
function depsFor(store: SessionStore, oidc: OidcClient, logger: AuthLogger) {
  return {
    store,
    prepareRefresh: () => oidc.ensureDiscovered(),
    refresh: (refreshToken: string) => oidc.refresh(refreshToken),
    revoke: (token: string) => oidc.revoke(token),
    logger,
    skewMs: 30_000,
    lockTtlMs: 5_000,
    lockWaitMs: 3_000,
    ttlMs: 60_000,
  };
}

// RED-FIRST (SMA-704 plan, Task 2). Both cases are `it.fails` in the commit that measured them on
// the code before the fix: `it.fails` passes only while its test fails. The fix commit (Task 3)
// changes them to `it` and changes nothing else in them.
describe('OIDC discovery and the session lock (SMA-704 test 13)', () => {
  let fixture: OidcFixture;
  let hanging: DiscoveryFailureFixture | undefined;

  beforeEach(async () => {
    fixture = await startOidcFixture();
  });

  afterEach(async () => {
    await fixture.close();
    await hanging?.close();
    hanging = undefined;
  });

  it.fails(
    'SMA-704 test 13a: no discovery request reaches the IdP while the lock is held',
    async () => {
      fixture.setResponseDelay('discovery', DISCOVERY_DELAY_MS);
      fixture.setResponseDelay('token', TOKEN_DELAY_MS);
      fixture.setResponseDelay('jwks', JWKS_DELAY_MS);
      // The refresh response carries an ID token, so the non-repudiation hook fetches JWKS. Its `iss`
      // and `sub` equal the record's login claims, so the D5 id_token_mismatch branch does not run.
      fixture.setNextIdToken(await fixture.mintIdToken());
      const inner = new MemorySessionStore();
      await inner.set('s', makeRecord({ accessExpiresAt: Date.now() - 1, idTokenClaims: { iss: fixture.issuer, sub: 'user-1' } }), 60_000, null);
      const window = newLockWindow();
      const { logger, events } = recordingLogger();

      const startedAt = Date.now();
      await resolveSession(depsFor(lockTimingStore(inner, window), coldClient(fixture.issuer, T), logger), 's');
      const totalMs = Date.now() - startedAt;

      const { acquiredAt, releasedAt } = window;
      if (acquiredAt === undefined || releasedAt === undefined) throw new Error('the lock was never taken and released');
      const log = fixture.requests();
      const underLock = (endpoint: FixtureEndpoint): number => log.filter((r) => r.endpoint === endpoint && r.at >= acquiredAt && r.at <= releasedAt).length;
      const beforeLock = (endpoint: FixtureEndpoint): number => log.filter((r) => r.endpoint === endpoint && r.at < acquiredAt).length;

      // Printed BEFORE the assertions, so a red run shows the numbers too. The spec's § 7 records them.
      console.info(
        `SMA-704 test 13a: lock hold ${String(releasedAt - acquiredAt)} ms; resolveSession ${String(totalMs)} ms; ` +
          `under the lock: discovery ${String(underLock('discovery'))}, token ${String(underLock('token'))}, jwks ${String(underLock('jwks'))}; ` +
          `before the lock: discovery ${String(beforeLock('discovery'))}`,
      );

      expect(underLock('discovery'), 'discovery requests while the lock is held').toBe(0);
      expect(underLock('token'), 'token requests while the lock is held').toBe(1);
      expect(underLock('jwks'), 'JWKS requests while the lock is held').toBe(1);
      expect(beforeLock('discovery'), 'discovery requests before the lock is taken').toBe(1);
      expect(events).toContainEqual(['session.refreshed', { sid: sidTag('s'), rev: 1, idTokenRotated: true }]);
    },
    15_000,
  );

  // The worst case with every call AT its timeout is a failed refresh, so this case covers the
  // timeout. Its own client uses T = 200 ms, to keep the run short.
  it.fails('SMA-704 test 13b: a hanging discovery never reaches tryAcquireLock', async () => {
    hanging = await startDiscoveryFailureFixture('hang');
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const window = newLockWindow();
    const { logger } = recordingLogger();

    const err: unknown = await resolveSession(depsFor(lockTimingStore(inner, window), coldClient(hanging.issuer, 200), logger), 's').catch((e: unknown) => e);

    console.info(`SMA-704 test 13b: tryAcquireLock calls ${String(window.attempts)}`);
    expect(window.attempts, 'tryAcquireLock calls').toBe(0);
    expect(isOidcDiscoveryFailed(err)).toBe(true);
    expect(hanging.requests).toBe(1);
    expect(await inner.get('s')).not.toBeNull();
  });
});
