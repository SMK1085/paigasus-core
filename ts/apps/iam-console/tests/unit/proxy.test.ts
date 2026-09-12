// SPDX-License-Identifier: Apache-2.0
//
// proxy.ts under the real basePath (spec § 7.5, § 13 #1). A NextRequest built with
// `nextConfig: { basePath: '/iam' }` strips the basePath from nextUrl.pathname exactly as Next does.
import { unstable_doesMiddlewareMatch } from 'next/experimental/testing/server';
import { NextRequest } from 'next/server';
import { describe, expect, it, vi } from 'vitest';
import { SESSION_COOKIE_NAME } from '../../lib/auth';
import { config, proxy } from '../../proxy';

// Next's server installs globalThis.AsyncLocalStorage before any other module
// (next/dist/server/node-environment-baseline.js); vitest does not. Loading
// next/experimental/testing/server patches `console`, and without the global every later console
// call in this file throws "AsyncLocalStorage accessed in runtime where it is not available"
// (MEASURED, Next 16.3.4 under vitest 5.0.0). vi.hoisted runs before the imports above.
vi.hoisted(() => {
  const scope = globalThis as { AsyncLocalStorage?: unknown };
  scope.AsyncLocalStorage ??= process.getBuiltinModule('node:async_hooks').AsyncLocalStorage;
});

const ORIGIN = 'https://console.example.test';

/** The basePath next.config.ts compiles in. unstable_doesMiddlewareMatch prefixes each matcher with it, as Next does. */
const NEXT_CONFIG = { basePath: '/iam' };

function request(path: string, init: { cookie?: boolean; headers?: Record<string, string> } = {}): NextRequest {
  const headers = new Headers(init.headers);
  if (init.cookie === true) headers.set('cookie', `${SESSION_COOKIE_NAME}=opaque-session-id`);
  return new NextRequest(`${ORIGIN}${path}`, { headers, nextConfig: { basePath: '/iam' } });
}

describe('proxy', () => {
  it.each(['/iam', '/iam/healthz', '/iam/auth/login', '/iam/auth/callback', '/iam/auth/logout', '/iam/auth/logout/callback'])('lets %s through with no cookie', (path) => {
    const res = proxy(request(path));
    expect(res.headers.get('location')).toBeNull();
  });

  it('sends a visitor with no cookie to login ONCE under the basePath, and keeps the basePath in returnTo', () => {
    const res = proxy(request('/iam/orgs?offset=50'));
    const location = new URL(res.headers.get('location') ?? '', ORIGIN);
    expect(location.pathname).toBe('/iam/auth/login');
    expect(location.searchParams.get('returnTo')).toBe('/iam/orgs?offset=50');
  });

  it('lets a request with the session cookie through, and checks presence only', () => {
    const res = proxy(request('/iam/orgs', { cookie: true }));
    expect(res.headers.get('location')).toBeNull();
  });
});

// Next's own matcher evaluation (next/experimental/testing/server), run against the exported
// config under the real basePath. MEASURED: a matcher written WITH the /iam prefix matches no page
// here (Next prefixes the basePath again), so the "runs" cases fail; a matcher that drops one of
// the three exclusions fails the matching "skips" case.
describe('config.matcher (spec § 7.5, § 13 #4)', () => {
  it('is written WITHOUT the basePath', () => {
    expect(config.matcher).toEqual(['/((?!_next/static|_next/image|favicon.ico).*)']);
  });

  it.each(['/iam/', '/iam/orgs', '/iam/orgs?offset=50', '/iam/healthz', '/iam/auth/login'])('runs the proxy for the page %s', (url) => {
    expect(unstable_doesMiddlewareMatch({ config, url, nextConfig: NEXT_CONFIG })).toBe(true);
  });

  it.each(['/iam/_next/static/chunks/main.js', '/iam/_next/image?url=%2Flogo.png&w=64&q=75', '/iam/favicon.ico'])('skips the static asset %s', (url) => {
    expect(unstable_doesMiddlewareMatch({ config, url, nextConfig: NEXT_CONFIG })).toBe(false);
  });
});
