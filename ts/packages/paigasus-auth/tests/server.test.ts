// SPDX-License-Identifier: Apache-2.0
//
// createAuthRouteHandler — the Next boundary that maps a rejected `/auth/callback` to a Response
// instead of letting it surface as an unhandled rejection (a 500 for a stale tab's benign
// txn_missing, among other reasons). `http/routes.ts`'s own suite (tests/http/callback.test.ts)
// covers how each CallbackRejected reason actually arises; this suite mocks `createAuthRoutes`
// itself so it can assert the MAPPING in isolation, for all five reasons, without re-running a
// full OIDC fixture.
import { describe, expect, it, vi } from 'vitest';
import { CallbackRejected } from '../src/core/errors.js';

// `src/server.ts` opens with `import 'server-only'` (AC 5). Its real module throws
// unconditionally outside a `react-server`-conditioned resolution, which this package's vitest
// config cannot turn on globally without also breaking `next/navigation`'s and
// `react-dom/client`'s OWN conditional exports — see vitest.config.ts's `resolve.alias`, which
// aliases `server-only` to an empty stub for every test in this package. No per-file mock needed
// here (task 10 review round 1 replaced one that used to live in this file).

const handleMock = vi.fn<(req: Request) => Promise<Response>>();

vi.mock('../src/http/routes.js', () => ({
  createAuthRoutes: () => ({ handle: handleMock }),
}));

import { createAuthRouteHandler } from '../src/server.js';
import type { AuthRuntime } from '../src/runtime.js';

function runtime(): AuthRuntime {
  return {
    store: {} as AuthRuntime['store'],
    resolver: {} as AuthRuntime['resolver'],
    logger: { event: () => undefined },
    oidc: {} as AuthRuntime['oidc'],
    redirectUri: 'https://app.example.com/iam/auth/callback',
    postLogoutRedirectUri: 'https://app.example.com/iam/',
    cookieDomainless: true,
    skewMs: 30_000,
    lockTtlMs: 10_000,
    lockWaitMs: 3_000,
    ttlMs: 28_800_000,
    absoluteTtlMs: 86_400_000,
    zone: 'iam',
    basePath: '/iam',
    scopes: 'openid',
  };
}

describe('createAuthRouteHandler', () => {
  it('returns the underlying response unchanged when handle() resolves', async () => {
    const ok = new Response(null, { status: 302, headers: { Location: 'https://idp.example.com/authorize' } });
    handleMock.mockResolvedValueOnce(ok);

    const handler = createAuthRouteHandler(runtime());
    const res = await handler(new Request('https://app.example.com/iam/auth/login'));

    expect(res).toBe(ok);
  });

  it.each([
    ['txn_missing', '/iam/auth/login'],
    ['txn_mismatch', '/iam/auth/login'],
    ['state_unknown', '/iam/auth/login'],
    ['idp_error', '/iam/'],
  ] as const)('maps %s to a 302 redirect to %s', async (reason, expectedLocation) => {
    handleMock.mockRejectedValueOnce(new CallbackRejected(reason));

    const handler = createAuthRouteHandler(runtime());
    const res = await handler(new Request('https://app.example.com/iam/auth/callback'));

    expect(res.status).toBe(302);
    expect(res.headers.get('Location')).toBe(expectedLocation);
  });

  it('maps code_exchange_failed to a 502, not a silent redirect', async () => {
    handleMock.mockRejectedValueOnce(new CallbackRejected('code_exchange_failed'));

    const handler = createAuthRouteHandler(runtime());
    const res = await handler(new Request('https://app.example.com/iam/auth/callback'));

    expect(res.status).toBe(502);
    expect(res.headers.get('Location')).toBeNull();
  });

  it('rethrows any error that is not a CallbackRejected', async () => {
    handleMock.mockRejectedValueOnce(new Error('boom'));

    const handler = createAuthRouteHandler(runtime());

    await expect(handler(new Request('https://app.example.com/iam/auth/callback'))).rejects.toThrow('boom');
  });
});
