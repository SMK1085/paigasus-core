// SPDX-License-Identifier: Apache-2.0
//
// SESSION_COOKIE_NAME is written in lib/auth.ts because @paigasus/auth exports its cookie name
// from no entry an app may import. This test binds the literal to the package's BEHAVIOUR: its own
// middleware must treat exactly this name as the session, and no other.
import { NextRequest } from 'next/server';
import { describe, expect, it } from 'vitest';
import { createAuthMiddleware } from '@paigasus/auth/middleware';
import { SESSION_COOKIE_NAME } from '../../lib/auth';

const middleware = createAuthMiddleware({ publicPaths: ['/auth/login'], loginPath: '/auth/login' });

function withCookie(name: string): NextRequest {
  return new NextRequest('https://console.example.test/iam/orgs', { headers: { cookie: `${name}=x` }, nextConfig: { basePath: '/iam' } });
}

describe('SESSION_COOKIE_NAME', () => {
  it('is the name @paigasus/auth/middleware accepts as a session', () => {
    expect(middleware(withCookie(SESSION_COOKIE_NAME)).headers.get('location')).toBeNull();
  });

  it('is not accepted in any other spelling (the control)', () => {
    expect(middleware(withCookie(`${SESSION_COOKIE_NAME}x`)).headers.get('location')).not.toBeNull();
  });
});
