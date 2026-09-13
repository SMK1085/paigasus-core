// SPDX-License-Identifier: Apache-2.0
//
// lib/discovery.ts (spec § 4.4, § 6.6): the app's composition of @paigasus/discovery, with MSW
// serving IAM's `GET /v1/service-info` (AC 5). No live service and no Docker.
//
// The process-wide descriptor cache is module state, so each case imports fresh modules and resets
// the cache afterwards.
import { setupServer } from 'msw/node';
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { stubConsoleEnv } from '../support/env';
import { serviceInfoHandlers } from '../support/msw';

const IAM_HTTP = 'http://iam.msw.test';
const server = setupServer();

beforeAll(() => server.listen({ onUnhandledRequest: 'error' }));
afterEach(() => {
  server.resetHandlers();
  vi.unstubAllEnvs();
});
afterAll(() => server.close());

async function load(overrides: Record<string, string>) {
  vi.resetModules();
  stubConsoleEnv({ PAIGASUS_SERVICES: JSON.stringify({ iam: IAM_HTTP }), ...overrides });
  const { getRuntimeConfig } = await import('../../lib/config');
  const { createJsonLogger } = await import('../../lib/logger');
  const discovery = await import('../../lib/discovery');
  const lines: string[] = [];
  const handle = discovery.createAppDiscovery({ config: getRuntimeConfig(), log: createJsonLogger((line) => lines.push(line)), waitUntil: () => undefined });
  return { handle, lines, reset: discovery.resetDiscoveryForTest };
}

describe('the app’s discovery', () => {
  it('reports IAM available with the capabilities it serves (memory store)', async () => {
    server.use(...serviceInfoHandlers(IAM_HTTP, { service: 'iam', version: '1.0.0', capabilities: ['iam.authz.cedar', 'iam.audit'] }));
    const { handle, reset } = await load({});
    try {
      expect(await handle.getServiceState('iam', 'token-a')).toMatchObject({ state: 'available', capabilities: ['iam.authz.cedar', 'iam.audit'] });
      expect(await handle.hasCapability('iam.audit', 'token-a')).toBe(true);
    } finally {
      reset();
    }
  });

  it('reports a service that is not configured as absent, without a request', async () => {
    const { handle, reset } = await load({});
    try {
      expect(await handle.getServiceState('gateway', 'token-a')).toEqual({ state: 'absent', service: 'gateway' });
    } finally {
      reset();
    }
  });

  it('reports IAM degraded when its descriptor route fails', async () => {
    server.use(...serviceInfoHandlers(IAM_HTTP, { status: 503 }));
    const { handle, reset } = await load({});
    try {
      expect(await handle.getServiceState('iam', 'token-a')).toMatchObject({ state: 'degraded', reason: 'server-error' });
    } finally {
      reset();
    }
  });

  // The file header's "no silent fallback" rule, from the other side. lib/config.ts's flat zod
  // shape cannot express this cross-field rule, so the pair reaches descriptorCacheFor. Before the
  // final-review fix it read as "use memory", and every zone then cached in its own process.
  it('refuses the redis store with no URL, instead of falling back to the memory cache', async () => {
    await expect(load({ PAIGASUS_SESSION_STORE: 'redis' })).rejects.toThrow('PAIGASUS_SESSION_REDIS_URL is required when PAIGASUS_SESSION_STORE is "redis"');
  });

  it('with an unreachable Redis: degrades to cache-unavailable, and logs the failure once without the DSN', async () => {
    server.use(...serviceInfoHandlers(IAM_HTTP, { service: 'iam', version: '1.0.0', capabilities: [] }));
    const { handle, lines, reset } = await load({ PAIGASUS_SESSION_STORE: 'redis', PAIGASUS_SESSION_REDIS_URL: 'redis://:hunter2@127.0.0.1:1', PAIGASUS_SESSION_REDIS_TIMEOUT_MS: '200' });
    try {
      expect(await handle.getServiceState('iam', 'token-a')).toMatchObject({ state: 'degraded', reason: 'cache-unavailable' });
      const events = lines.map((line) => (JSON.parse(line) as { event: string }).event);
      expect(events.filter((event) => event === 'discovery.redis_connect_failed')).toHaveLength(1);
      expect(lines.join('\n')).not.toContain('hunter2');
    } finally {
      reset();
    }
  });
});
