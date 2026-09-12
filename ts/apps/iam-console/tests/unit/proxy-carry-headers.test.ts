// SPDX-License-Identifier: Apache-2.0
//
// proxy.ts carries the auth middleware's ALLOW response onto its own `next()` (final whole-branch
// review, minor 6). The middleware sets nothing on that response today, so only a double can show
// the carry — hence this file, separate from tests/unit/proxy.test.ts, which runs the real
// middleware and pins proxy.ts's import list.
import { NextRequest, NextResponse } from 'next/server';
import { describe, expect, it, vi } from 'vitest';

vi.mock('@paigasus/auth/middleware', () => ({
  authRoutePaths: () => ['/auth/login', '/auth/callback', '/auth/logout', '/auth/logout/callback'],
  createAuthMiddleware: () => (): NextResponse => {
    const allowed = NextResponse.next();
    allowed.headers.set('x-auth-note', 'kept');
    allowed.cookies.set('auth-marker', 'carried');
    return allowed;
  },
}));

const { proxy } = await import('../../proxy');

describe('proxy carries the auth middleware allow response', () => {
  const res = proxy(new NextRequest('https://console.example.test/iam/orgs', { nextConfig: { basePath: '/iam' } }));

  it('keeps a response header the middleware set', () => {
    expect(res.headers.get('x-auth-note')).toBe('kept');
  });

  it('keeps a cookie the middleware set', () => {
    expect(res.cookies.get('auth-marker')?.value).toBe('carried');
  });

  // The carry must not overwrite the keys that describe THIS response's own request overrides.
  it('still forwards the two request headers proxy.ts mints', () => {
    expect(res.headers.get('x-middleware-request-paigasus-correlation-id')).toMatch(/^[0-9a-f-]{36}$/);
    expect(res.headers.get('x-middleware-request-x-paigasus-request-path')).toBe('/iam/orgs');
  });
});
