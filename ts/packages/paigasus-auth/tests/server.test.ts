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

function runtime(overrides: Partial<AuthRuntime> = {}): AuthRuntime {
  return {
    store: {} as AuthRuntime['store'],
    resolver: {} as AuthRuntime['resolver'],
    logger: { event: () => undefined },
    oidc: {} as AuthRuntime['oidc'],
    publicOrigin: 'https://app.example.com',
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
    ...overrides,
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

// SMA-511 spec § 7.1. Next removes the basePath from a route handler's `req.url` and puts the
// server's bind address in it (measured: `http://0.0.0.0:<port>/auth/callback?…`). The core route
// table is keyed by the full path on the public origin, so the handler rebuilds the URL first.
describe('createAuthRouteHandler rebuilds the request URL (SMA-511 spec § 7.1)', () => {
  async function requestSeenBy(rt: AuthRuntime, input: string, init?: RequestInit): Promise<Request> {
    handleMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
    await createAuthRouteHandler(rt)(new Request(input, init));
    const seen = handleMock.mock.lastCall?.[0];
    if (seen === undefined) throw new Error('createAuthRoutes().handle was not called');
    return seen;
  }

  it('adds the basePath back and uses PAIGASUS_PUBLIC_ORIGIN for what Next hands a route handler', async () => {
    const seen = await requestSeenBy(runtime(), 'http://0.0.0.0:3000/auth/callback?code=c&state=s');
    expect(seen.url).toBe('https://app.example.com/iam/auth/callback?code=c&state=s');
  });

  it('keeps a full path, as a plain node:http server passes it (tests/e2e/fixture-server.ts)', async () => {
    const seen = await requestSeenBy(runtime(), 'http://127.0.0.1:4000/iam/auth/login?returnTo=%2Fiam%2Fx');
    expect(seen.url).toBe('https://app.example.com/iam/auth/login?returnTo=%2Fiam%2Fx');
  });

  // The route table decides, not a prefix test: with basePath '/auth', the stripped path '/auth/login'
  // STARTS with the basePath and is still not a full path.
  it('is not confused by a zone whose basePath is /auth', async () => {
    const rt = runtime({ basePath: '/auth' });
    expect((await requestSeenBy(rt, 'http://0.0.0.0:3000/auth/login')).url).toBe('https://app.example.com/auth/auth/login');
    expect((await requestSeenBy(rt, 'http://127.0.0.1:4000/auth/auth/login')).url).toBe('https://app.example.com/auth/auth/login');
  });

  it('works for a root-mounted zone', async () => {
    const seen = await requestSeenBy(runtime({ basePath: '' }), 'http://0.0.0.0:3000/auth/login');
    expect(seen.url).toBe('https://app.example.com/auth/login');
  });

  it('passes a non-auth path on with only the origin changed, so the route table still 404s it', async () => {
    const seen = await requestSeenBy(runtime(), 'http://0.0.0.0:3000/elsewhere');
    expect(seen.url).toBe('https://app.example.com/elsewhere');
  });

  // SMA-511 final review, minor 9. publicRequestUrl CONCATENATES onto the absolute origin on
  // purpose, and until now nothing pinned that choice. Rewrite it as `new URL(pathname, origin)`
  // and a pathname that begins with `//` resolves as a PROTOCOL-RELATIVE url, so the host becomes
  // evil.example — the whole login flow then builds its redirects on an attacker's origin. The
  // assertion is on the HOST, not on the whole string, because the host is what the defect moves.
  //
  // The backslash row is not a duplicate of the first: MEASURED, the WHATWG parser normalises `\`
  // to `/` for a special scheme, so `/\evil.example` ARRIVES as `//evil.example`. That is the shape
  // a caller cannot spot by reading the raw string, which is why it is listed.
  it.each([
    ['//evil.example/auth/callback', 'https://app.example.com//evil.example/auth/callback'],
    ['/\\evil.example/auth/callback', 'https://app.example.com//evil.example/auth/callback'],
    ['//evil.example//auth/login', 'https://app.example.com//evil.example//auth/login'],
  ])('keeps the public origin for the host-confusing path %s', async (path, expected) => {
    const seen = await requestSeenBy(runtime(), `http://0.0.0.0:3000${path}`);
    expect(new URL(seen.url).host).toBe('app.example.com');
    expect(seen.url).toBe(expected);
  });

  it('keeps the method, the headers and the body', async () => {
    const seen = await requestSeenBy(runtime(), 'http://0.0.0.0:3000/auth/logout', { method: 'POST', headers: { cookie: 'a=b' }, body: 'x=1' });
    expect(seen.method).toBe('POST');
    expect(seen.headers.get('cookie')).toBe('a=b');
    expect(await seen.text()).toBe('x=1');
  });
});
