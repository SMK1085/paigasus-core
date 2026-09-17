// SPDX-License-Identifier: Apache-2.0
//
// proxy.ts under the real basePath (spec § 7.5, § 13 #1). A NextRequest built with
// `nextConfig: { basePath: '/gateway' }` strips the basePath from nextUrl.pathname exactly as Next does.
import { readFileSync } from 'node:fs';
import { unstable_doesMiddlewareMatch } from 'next/experimental/testing/server';
import { NextRequest } from 'next/server';
import ts from 'typescript';
import { describe, expect, it, vi } from 'vitest';
import { SESSION_COOKIE } from '@paigasus/auth/server';
import { CORRELATION_HEADER, REQUEST_PATH_HEADER } from '@paigasus/console-core';
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
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

/** The basePath next.config.ts compiles in. unstable_doesMiddlewareMatch prefixes each matcher with it, as Next does. */
const NEXT_CONFIG = { basePath: '/gateway' };

/**
 * Every module specifier a source file imports, sorted and without duplicates. TypeScript's own
 * pre-processor reads them: static, type-only, side-effect, re-export, dynamic and require forms,
 * and never an import inside a comment.
 */
function importsOf(relative: string): string[] {
  const source = readFileSync(new URL(relative, import.meta.url), 'utf8');
  return [...new Set(ts.preProcessFile(source, true, true).importedFiles.map((file) => file.fileName))].sort();
}

function request(path: string, init: { cookie?: boolean; headers?: Record<string, string> } = {}): NextRequest {
  const headers = new Headers(init.headers);
  if (init.cookie === true) headers.set('cookie', `${SESSION_COOKIE}=opaque-session-id`);
  return new NextRequest(`${ORIGIN}${path}`, { headers, nextConfig: { basePath: '/gateway' } });
}

/** NextResponse.next({ request: { headers } }) carries each overridden request header as x-middleware-request-<name>. */
function forwarded(res: Response, name: string): string | null {
  return res.headers.get(`x-middleware-request-${name}`);
}

describe('proxy', () => {
  it.each(['/gateway', '/gateway/healthz', '/gateway/auth/login', '/gateway/auth/callback', '/gateway/auth/logout', '/gateway/auth/logout/callback'])('lets %s through with no cookie', (path) => {
    const res = proxy(request(path));
    expect(res.headers.get('location')).toBeNull();
  });

  it('sends a visitor with no cookie to login ONCE under the basePath, and keeps the basePath in returnTo', () => {
    const res = proxy(request('/gateway/overview?offset=50'));
    const location = new URL(res.headers.get('location') ?? '', ORIGIN);
    expect(location.pathname).toBe('/gateway/auth/login');
    expect(location.searchParams.get('returnTo')).toBe('/gateway/overview?offset=50');
  });

  it('lets a request with the session cookie through, and checks presence only', () => {
    const res = proxy(request('/gateway/overview', { cookie: true }));
    expect(res.headers.get('location')).toBeNull();
  });

  it('mints a fresh UUID correlation id into the REQUEST headers, overwriting one the browser sent', () => {
    const first = proxy(request('/gateway/overview', { cookie: true, headers: { 'paigasus-correlation-id': 'attacker-supplied' } }));
    const second = proxy(request('/gateway/overview', { cookie: true }));
    const a = forwarded(first, 'paigasus-correlation-id');
    const b = forwarded(second, 'paigasus-correlation-id');
    expect(a).toMatch(UUID_RE);
    expect(b).toMatch(UUID_RE);
    expect(a).not.toBe(b);
    expect(a).not.toBe('attacker-supplied');
  });

  it('records the public path, with the basePath and without the query', () => {
    const res = proxy(request('/gateway/overview?offset=50', { cookie: true }));
    expect(forwarded(res, 'x-paigasus-request-path')).toBe('/gateway/overview');
  });

  // The proxy's allowed imports, as a strict-equality list. `server-only` is a no-op in the proxy
  // layer, and paigasus/boundaries/app-middleware is a DENY list of direct specifiers. So one new
  // import (for example @paigasus/console-core, which reaches @paigasus/auth/server through its
  // logger) would enter the proxy bundle with no lint error. Here it fails.
  it('imports only the allowed modules in proxy.ts', () => {
    expect(importsOf('../../proxy.ts')).toEqual(['@paigasus/auth/middleware', 'next/server']);
  });

  // This assertion exists because proxy.ts cannot import @paigasus/console-core (the row above,
  // and the DENIED boundary row for it). Without it, the two header names would be stated in two
  // places with nothing binding them: a rename in
  // ts/packages/paigasus-console-core/src/correlation-header.ts would silently stop the proxy's
  // headers from being read anywhere downstream. It reads the names FROM the package — the source
  // of truth — and checks them against the exact literals proxy.ts sets and the `forwarded(...)`
  // assertions above already use ('paigasus-correlation-id', 'x-paigasus-request-path'). Combined
  // with those functional tests (which fail if proxy.ts's own inlined literal ever drifts), the two
  // sides stay bound even though neither can import the other.
  it("binds the package's header names to the literals proxy.ts inlines", () => {
    expect(CORRELATION_HEADER).toBe('paigasus-correlation-id');
    expect(REQUEST_PATH_HEADER).toBe('x-paigasus-request-path');
  });
});

// Next's own matcher evaluation (next/experimental/testing/server), run against the exported
// config under the real basePath. MEASURED: a matcher written WITH the /gateway prefix matches no
// page here (Next prefixes the basePath again), so the "runs" cases fail; a matcher that drops one
// of the three exclusions fails the matching "skips" case.
describe('config.matcher (spec § 7.5, § 13 #4)', () => {
  it('is written WITHOUT the basePath', () => {
    expect(config.matcher).toEqual(['/((?!_next/static|_next/image|favicon.ico).*)']);
  });

  it.each(['/gateway/', '/gateway/overview', '/gateway/overview?offset=50', '/gateway/healthz', '/gateway/auth/login'])('runs the proxy for the page %s', (url) => {
    expect(unstable_doesMiddlewareMatch({ config, url, nextConfig: NEXT_CONFIG })).toBe(true);
  });

  it.each(['/gateway/_next/static/chunks/main.js', '/gateway/_next/image?url=%2Flogo.png&w=64&q=75', '/gateway/favicon.ico'])('skips the static asset %s', (url) => {
    expect(unstable_doesMiddlewareMatch({ config, url, nextConfig: NEXT_CONFIG })).toBe(false);
  });
});
