// SPDX-License-Identifier: Apache-2.0
//
// SMA-651 D8. The runtime must hand ITS logger to the Redis store, or store.operation_timeout is
// logged to the no-op logger and production sees nothing. Deleting the `logger` line in
// createAuthRuntime reds this test and nothing else.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AuthEventFields, AuthEventName } from '../src/ports/logger.js';

vi.mock('redis', () => ({
  createClient: () => ({
    on: () => undefined,
    isOpen: true,
    destroy: () => undefined,
    connect: () => Promise.resolve(),
    // A Redis that accepts the command and never replies.
    get: () => new Promise<never>(() => undefined),
    set: () => Promise.resolve('OK'),
    del: () => Promise.resolve(1),
    eval: () => Promise.resolve(1),
  }),
}));

const { createAuthRuntime } = await import('../src/runtime.js');

// Copied from tests/runtime.test.ts's BASE, with the redis store selected.
const CONFIG = {
  PAIGASUS_ZONE: 'iam',
  PAIGASUS_ZONES: { iam: '/iam' },
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.com',
  PAIGASUS_OIDC_CLIENT_ID: 'c',
  PAIGASUS_OIDC_CLIENT_SECRET: 's',
  PAIGASUS_PUBLIC_ORIGIN: 'https://app.example.com',
  PAIGASUS_OIDC_SCOPES: 'openid profile email offline_access',
  PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS: 30,
  PAIGASUS_OIDC_HTTP_TIMEOUT_MS: 3500,
  PAIGASUS_SESSION_STORE: 'redis' as const,
  PAIGASUS_SESSION_REDIS_URL: 'redis://127.0.0.1:6379',
  PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 1000,
  PAIGASUS_SESSION_TTL_SECONDS: 28800,
  PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS: 86400,
  PAIGASUS_SESSION_REFRESH_SKEW_SECONDS: 30,
  PAIGASUS_SESSION_LOCK_TTL_MS: 10000,
  PAIGASUS_SESSION_LOCK_WAIT_MS: 3000,
};

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('createAuthRuntime wires its logger into the Redis store (SMA-651 D8)', () => {
  it('a store timeout reaches the logger the runtime was given', async () => {
    const events: Array<[AuthEventName, AuthEventFields]> = [];
    const runtime = await createAuthRuntime(CONFIG, { logger: { event: (name, fields) => events.push([name, fields]) } });
    const pending = runtime.store.get('sid').catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(4000);
    await pending;
    expect(events).toContainEqual(['store.operation_timeout', { operation: 'get', deadlineMs: 4000 }]);
  });
});
