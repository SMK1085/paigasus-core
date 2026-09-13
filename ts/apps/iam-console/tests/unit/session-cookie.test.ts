// SPDX-License-Identifier: Apache-2.0
//
// SESSION_COOKIE is exported from @paigasus/auth/server (http/cookies.ts). This test binds the
// constant to the package's own BEHAVIOUR: its middleware must treat exactly this name as the
// session, and no other. The literal string is pinned too, so a rename inside @paigasus/auth still
// fails a test here even though the name now comes from the same package as the middleware.
import { NextRequest } from 'next/server';
import { describe, expect, it } from 'vitest';
import { createAuthMiddleware } from '@paigasus/auth/middleware';
import { SESSION_COOKIE } from '@paigasus/auth/server';

const middleware = createAuthMiddleware({ publicPaths: ['/auth/login'], loginPath: '/auth/login' });

function withCookie(name: string): NextRequest {
  return new NextRequest('https://console.example.test/iam/orgs', { headers: { cookie: `${name}=x` }, nextConfig: { basePath: '/iam' } });
}

describe('SESSION_COOKIE', () => {
  it('is the literal __Host-pgs_sid', () => {
    expect(SESSION_COOKIE).toBe('__Host-pgs_sid');
  });

  it('is the name @paigasus/auth/middleware accepts as a session', () => {
    expect(middleware(withCookie(SESSION_COOKIE)).headers.get('location')).toBeNull();
  });

  it('is not accepted in any other spelling (the control)', () => {
    expect(middleware(withCookie(`${SESSION_COOKIE}x`)).headers.get('location')).not.toBeNull();
  });
});
