// SPDX-License-Identifier: Apache-2.0
//
// SMA-705: readinessResponse, the handler behind each console's `<basePath>/readyz` (spec D2, D3,
// D7, D8, D10, D11; T5-T13b). T5-T12 use the network-free FakeOidc of tests/support/store-failure.ts.
// T13a, T13b and the D8 overlap row use the REAL adapter through createAuthRuntime.
//
// A detached promise settles after the handler returns. A row that checks a log or an unhandled
// rejection settles the fake's promise, then awaits one setImmediate: Node emits
// `unhandledRejection` only after the microtask queue drains. A listener records every unhandled
// rejection, and each such row asserts that the list is empty.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createOidcClient } from '../../src/adapters/oidc.js';
import { OidcDiscoveryFailed } from '../../src/core/errors.js';
import { readinessResponse } from '../../src/http/readiness.js';
import { createAuthRoutes } from '../../src/http/routes.js';
import type { AuthEventFields, AuthEventName, AuthLogger } from '../../src/ports/logger.js';
import { createAuthRuntime, type AuthRuntime } from '../../src/runtime.js';
import { startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';
import { IDP_SENTINELS, SENTINEL_IDP_URL, expectEventsClean, harness, type Harness } from '../support/store-failure.js';

type Events = Array<[AuthEventName, AuthEventFields]>;

/** The Redis URL of D10's measured leak. No body, header or logged field may hold its password. */
const REDIS_SENTINEL = 'redis://u:sentinel-705@h';
const RUNTIME_SENTINELS: readonly string[] = ['sentinel-705'];

/** Port 1 is on the Fetch "bad port" list, so discovery fails at once with no connect (SMA-656). */
const UNREACHABLE_ISSUER = 'http://127.0.0.1:1';

/** A valid composed configuration: tests/runtime.test.ts's BASE, one zone and the memory store. */
const BASE_CONFIG = {
  PAIGASUS_ZONE: 'iam',
  PAIGASUS_ZONES: { iam: '/iam' },
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.com',
  PAIGASUS_OIDC_CLIENT_ID: 'c',
  PAIGASUS_OIDC_CLIENT_SECRET: 's',
  PAIGASUS_PUBLIC_ORIGIN: 'https://app.example.com',
  PAIGASUS_OIDC_SCOPES: 'openid profile email offline_access',
  PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS: 30,
  PAIGASUS_OIDC_HTTP_TIMEOUT_MS: 3500,
  PAIGASUS_SESSION_STORE: 'memory' as const,
  PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 1000,
  PAIGASUS_SESSION_TTL_SECONDS: 28800,
  PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS: 86400,
  PAIGASUS_SESSION_REFRESH_SKEW_SECONDS: 30,
  PAIGASUS_SESSION_LOCK_TTL_MS: 10000,
  PAIGASUS_SESSION_LOCK_WAIT_MS: 3000,
};

/** No store call happens in these rows. */
const noStoreFailure = (): Error => new Error('no store call in the SMA-705 rows');

let unhandled: unknown[] = [];
const onUnhandled = (reason: unknown): void => {
  unhandled.push(reason);
};

beforeEach(() => {
  unhandled = [];
  process.on('unhandledRejection', onUnhandled);
});

afterEach(() => {
  process.off('unhandledRejection', onUnhandled);
});

function recordingLogger(events: Events): AuthLogger {
  return { event: (name, fields) => void events.push([name, { ...fields }]) };
}

function getterFor(h: Harness): () => Promise<AuthRuntime> {
  return () => Promise.resolve(h.runtime);
}

/** Lets every settled promise run its handlers: all microtasks, then one macrotask turn. */
function settle(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

/** The promise's value, or 'timed_out' after `ms`. */
async function within<T>(promise: Promise<T>, ms: number): Promise<T | 'timed_out'> {
  let timer: NodeJS.Timeout | undefined;
  const timeout = new Promise<'timed_out'>((resolve) => {
    timer = setTimeout(() => resolve('timed_out'), ms);
  });
  try {
    return await Promise.race([promise, timeout]);
  } finally {
    clearTimeout(timer);
  }
}

/** D7: the exact body, `cache-control: no-store`, and no `forbidden` string in the body or a header. */
async function expectProbe(res: Response, code: 200 | 503, status: 'ready' | 'unready', forbidden: readonly string[] = []): Promise<void> {
  expect(res.status).toBe(code);
  expect(res.headers.get('cache-control')).toBe('no-store');
  const body = await res.text();
  expect(body).toBe(JSON.stringify({ status }));
  for (const text of [body, ...[...res.headers].map(([name, value]) => `${name}: ${value}`)]) {
    for (const sentinel of forbidden) expect(text).not.toContain(sentinel);
  }
}

/** D10's measured shape: node-redis's `new URL()` TypeError holds the URL in its message and `input`. */
function redisUrlError(): TypeError {
  return Object.assign(new TypeError(`Invalid URL: ${REDIS_SENTINEL}`), { code: 'ERR_INVALID_URL', input: REDIS_SENTINEL });
}

describe('readinessResponse — the discovery states (SMA-705 D2, D3, D7)', () => {
  it('T5: discovered -> 200 ready, no ensureDiscovered() call, no event', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'discovered';
    await expectProbe(await readinessResponse(getterFor(h), h.runtime.logger), 200, 'ready');
    expect(h.oidc.ensureDiscoveredCalls).toBe(0);
    expect(h.events).toEqual([]);
  });

  it('T6: discovering -> 503 unready, no ensureDiscovered() call', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'discovering';
    await expectProbe(await readinessResponse(getterFor(h), h.runtime.logger), 503, 'unready');
    expect(h.oidc.ensureDiscoveredCalls).toBe(0);
    expect(h.events).toEqual([]);
  });

  it('T7: idle -> 503 unready at once, with exactly one ensureDiscovered() call that is still pending', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'idle';
    h.oidc.holdEnsureDiscovered = true;
    const res = await within(readinessResponse(getterFor(h), h.runtime.logger), 1_000);
    expect(res, 'the handler waited for discovery (D3)').not.toBe('timed_out');
    await expectProbe(res as Response, 503, 'unready');
    expect(h.oidc.ensureDiscoveredCalls).toBe(1);
    h.oidc.resolveEnsureDiscovered?.();
    await settle();
  });

  it('T10: idle, and ensureDiscovered() resolves -> no event', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'idle';
    h.oidc.holdEnsureDiscovered = true;
    await expectProbe(await readinessResponse(getterFor(h), h.runtime.logger), 503, 'unready');
    h.oidc.resolveEnsureDiscovered?.();
    await settle();
    expect(h.events).toEqual([]);
    expect(unhandled).toEqual([]);
  });
});

describe('readinessResponse — a discovery failure it started (SMA-705 D8, D11)', () => {
  it('T8: OidcDiscoveryFailed with a URL in its message -> one readiness event, nothing leaks', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'idle';
    h.oidc.holdEnsureDiscovered = true;
    const res = await readinessResponse(getterFor(h), h.runtime.logger);
    h.oidc.rejectEnsureDiscovered?.(new OidcDiscoveryFailed(`oidc discovery failed at ${SENTINEL_IDP_URL}`, 'dns'));
    await settle();
    await expectProbe(res, 503, 'unready', IDP_SENTINELS);
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'dns' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
    expect(unhandled).toEqual([]);
  });

  it('T9: an OidcDiscoveryFailed from a second module copy -> the event has its reason', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    expect(foreign.OidcDiscoveryFailed).not.toBe(OidcDiscoveryFailed);
    const h = harness([], noStoreFailure);
    h.oidc.status = 'idle';
    h.oidc.holdEnsureDiscovered = true;
    await expectProbe(await readinessResponse(getterFor(h), h.runtime.logger), 503, 'unready');
    h.oidc.rejectEnsureDiscovered?.(new foreign.OidcDiscoveryFailed(`oidc discovery failed at ${SENTINEL_IDP_URL}`, 'tls'));
    await settle();
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'tls' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
    expect(unhandled).toEqual([]);
  });

  it('T9: a plain Error -> one event with the reason other', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'idle';
    h.oidc.holdEnsureDiscovered = true;
    await expectProbe(await readinessResponse(getterFor(h), h.runtime.logger), 503, 'unready');
    h.oidc.rejectEnsureDiscovered?.(new Error(`fetch failed at ${SENTINEL_IDP_URL}`));
    await settle();
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'other' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
    expect(unhandled).toEqual([]);
  });

  it('T12: a logger that throws inside the discovery catch -> no unhandled rejection', async () => {
    const h = harness([], noStoreFailure);
    h.runtime.logger = {
      event: () => {
        throw new Error('the logger failed');
      },
    };
    h.oidc.status = 'idle';
    h.oidc.holdEnsureDiscovered = true;
    const res = await readinessResponse(getterFor(h), { event: () => undefined });
    h.oidc.rejectEnsureDiscovered?.(new OidcDiscoveryFailed('oidc discovery failed: TypeError', 'network'));
    await settle();
    expect(unhandled).toEqual([]);
    await expectProbe(res, 503, 'unready');
  });
});

describe('readinessResponse — the runtime build fails (SMA-705 D10)', () => {
  it.each([
    ['rejects', (): Promise<AuthRuntime> => Promise.reject(redisUrlError())],
    [
      'throws synchronously',
      (): Promise<AuthRuntime> => {
        throw redisUrlError();
      },
    ],
  ])('T11: a runtime getter that %s -> 503 unready and one readiness.runtime_failed with the name only', async (_label, getter) => {
    const events: Events = [];
    await expectProbe(await readinessResponse(getter, recordingLogger(events)), 503, 'unready', RUNTIME_SENTINELS);
    expect(events).toEqual([['readiness.runtime_failed', { error: 'TypeError' }]]);
    expectEventsClean(events, RUNTIME_SENTINELS);
  });

  // Review Focus 4. getAuthRuntime clears its slot on a failure, so the next probe builds again.
  // The handler must keep no state of its own.
  it('T11b: a getter that fails once, then works -> the second call answers 200', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'discovered';
    const events: Events = [];
    let calls = 0;
    const getter = (): Promise<AuthRuntime> => {
      calls += 1;
      return calls === 1 ? Promise.reject(redisUrlError()) : Promise.resolve(h.runtime);
    };
    await expectProbe(await readinessResponse(getter, recordingLogger(events)), 503, 'unready', RUNTIME_SENTINELS);
    await expectProbe(await readinessResponse(getter, recordingLogger(events)), 200, 'ready');
    expect(calls).toBe(2);
    expect(events).toEqual([['readiness.runtime_failed', { error: 'TypeError' }]]);
  });

  it('T11c. The logger throws on the runtime-failure path -> still 503 unready (D7)', async () => {
    let calls = 0;
    const throwingLogger: AuthLogger = {
      event: () => {
        calls += 1;
        throw new Error('the logger failed');
      },
    };
    const getter = (): Promise<AuthRuntime> => Promise.reject(redisUrlError());
    await expectProbe(await readinessResponse(getter, throwingLogger), 503, 'unready', RUNTIME_SENTINELS);
    expect(calls).toBe(1);
  });

  // Review Focus 5. AuthConfigError sets its own `name` (plan deviation 3, Step 0).
  it('a real cross-field refusal from createAuthRuntime -> 503 unready, error AuthConfigError', async () => {
    const events: Events = [];
    const getter = (): Promise<AuthRuntime> => createAuthRuntime({ ...BASE_CONFIG, PAIGASUS_ZONES: { iam: '/iam', gateway: '/gateway' } });
    await expectProbe(await readinessResponse(getter, recordingLogger(events)), 503, 'unready');
    expect(events).toEqual([['readiness.runtime_failed', { error: 'AuthConfigError', code: 'auth_config_invalid' }]]);
  });

  // SMA-715 D4. The real error of a malformed Redis URL: the name and the code of AuthConfigError,
  // and no part of the URL anywhere. T11 and T11b above keep `{ error: 'TypeError' }` with NO `code`,
  // although their fixture carries `code: 'ERR_INVALID_URL'`: a foreign code is never logged.
  it('SMA-715: a malformed Redis URL from createAuthRuntime -> error AuthConfigError, code auth_config_invalid, no password', async () => {
    const events: Events = [];
    const getter = (): Promise<AuthRuntime> => createAuthRuntime({ ...BASE_CONFIG, PAIGASUS_SESSION_STORE: 'redis' as const, PAIGASUS_SESSION_REDIS_URL: 'redis//u:sentinel-715@h' });
    await expectProbe(await readinessResponse(getter, recordingLogger(events)), 503, 'unready', ['sentinel-715']);
    expect(events).toEqual([['readiness.runtime_failed', { error: 'AuthConfigError', code: 'auth_config_invalid' }]]);
    expectEventsClean(events, ['sentinel-715']);
  });
});

describe('readinessResponse through the real adapter (SMA-705 T13a, T13b, D8)', () => {
  let fixture: OidcFixture;

  beforeEach(async () => {
    fixture = await startOidcFixture();
  });

  afterEach(async () => {
    await fixture.close();
  });

  function realRuntime(issuer: string, events: Events): Promise<AuthRuntime> {
    return createAuthRuntime(
      { ...BASE_CONFIG, PAIGASUS_OIDC_ISSUER: issuer },
      {
        logger: recordingLogger(events),
        // The fixture is plain http on localhost. createAuthRuntime does not pass
        // allowInsecureRequests, so the factory adds it. Never in production.
        oidcClientFactory: (o) => createOidcClient({ ...o, allowInsecureRequests: true }),
      },
    );
  }

  it('T13a: a healthy IdP -> 503 first, then 200 after the attempt settles, with one discovery request', async () => {
    const events: Events = [];
    const runtime = await realRuntime(fixture.issuer, events);
    const getter = (): Promise<AuthRuntime> => Promise.resolve(runtime);
    const first = await readinessResponse(getter, runtime.logger);
    await runtime.oidc.ensureDiscovered();
    await settle();
    await expectProbe(first, 503, 'unready');
    await expectProbe(await readinessResponse(getter, runtime.logger), 200, 'ready');
    expect(fixture.requests().filter((r) => r.endpoint === 'discovery').length).toBe(1);
    expect(events).toEqual([]);
  });

  it('T13b: an unreachable IdP -> 503 on every call, and one readiness event per settled attempt', async () => {
    const events: Events = [];
    const runtime = await realRuntime(UNREACHABLE_ISSUER, events);
    const getter = (): Promise<AuthRuntime> => Promise.resolve(runtime);
    for (let attempt = 1; attempt <= 3; attempt += 1) {
      const res = await readinessResponse(getter, runtime.logger);
      await runtime.oidc.ensureDiscovered().catch(() => undefined);
      await settle();
      await expectProbe(res, 503, 'unready');
      expect(events).toHaveLength(attempt);
    }
    expect(events).toEqual([
      ['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'network' }],
      ['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'network' }],
      ['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'network' }],
    ]);
    expect(unhandled).toEqual([]);
  });

  // Review Focus 3. D8: each caller that needed a configuration logs its own event.
  it('D8: a login next to a failing readiness attempt -> two 503s and one event for each stage', async () => {
    const events: Events = [];
    const runtime = await realRuntime(UNREACHABLE_ISSUER, events);
    const probe = await readinessResponse(() => Promise.resolve(runtime), runtime.logger);
    const login = await createAuthRoutes(runtime).handle(new Request('https://app.example.com/iam/auth/login'));
    await settle();
    await expectProbe(probe, 503, 'unready');
    expect(login.status).toBe(503);
    expect(events.map(([name, fields]) => `${name}:${String(fields['stage'])}:${String(fields['reason'])}`).sort()).toEqual([
      'oidc.discovery_failed:login:network',
      'oidc.discovery_failed:readiness:network',
    ]);
    expect(unhandled).toEqual([]);
  });
});
