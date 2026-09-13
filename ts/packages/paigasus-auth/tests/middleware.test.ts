// SPDX-License-Identifier: Apache-2.0
//
// AC 4: middleware checks cookie PRESENCE only, never validity, and cannot even import anything
// that knows how to validate one — CVE-2025-29927 was exactly a middleware auth bypass.
import { randomUUID } from 'node:crypto';
import { readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { NextRequest, type NextResponse } from 'next/server';
import { authRoutePaths, createAuthMiddleware } from '../src/middleware.js';
import { SESSION_COOKIE } from '../src/http/cookies.js';
import { createAuthRoutes } from '../src/http/routes.js';
import { AUTH_ROUTE_SUFFIXES } from '../src/http/route-table.js';
import type { AuthRuntime } from '../src/runtime.js';
import { collectImportGraph, filesWithDynamicImportOrRequire } from './support/import-graph.js';

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(HERE, '../src');

// basePath-RELATIVE since SMA-511 (spec § 7.1): the middleware compares `req.nextUrl.pathname`,
// which Next gives WITHOUT the zone's basePath.
const OPTIONS = { publicPaths: authRoutePaths(), loginPath: '/auth/login' };

/** What Next hands a zone's proxy: a NextRequest whose NextURL knows the compiled basePath. */
function request(path: string, cookie?: string, basePath = '/iam'): NextRequest {
  const headers = cookie !== undefined ? { cookie } : {};
  return new NextRequest(`https://app.example.com${path}`, { headers, nextConfig: { basePath } });
}

describe('createAuthMiddleware', () => {
  it('redirects a guarded path with no session cookie to the login path, with a returnTo that keeps the basePath', () => {
    const res = createAuthMiddleware(OPTIONS)(request('/iam/dashboard?tab=1'));

    expect(res.status).toBe(307); // NextResponse.redirect's default status
    const location = new URL(res.headers.get('location') ?? '');
    // Exactly one `/iam`: a basePath-relative loginPath on a clone of nextUrl (spec § 13 row 1).
    expect(location.pathname).toBe('/iam/auth/login');
    expect(location.searchParams.get('returnTo')).toBe('/iam/dashboard?tab=1');
    expect([...location.searchParams.keys()]).toEqual(['returnTo']);
  });

  it('passes through a guarded path carrying ANY cookie value, including a forged one', () => {
    const res = createAuthMiddleware(OPTIONS)(request('/iam/dashboard', `${SESSION_COOKIE}=totally-forged-not-a-real-sid`));

    expect(res.headers.get('location')).toBeNull();
    // NextResponse.next() sets this marker header — proof this is a "continue" response, not a
    // freshly constructed 200 that merely looks like one.
    expect(res.headers.get('x-middleware-next')).toBe('1');
  });

  it.each(AUTH_ROUTE_SUFFIXES)('passes through the auth route /iam%s with no cookie at all', (suffix) => {
    const res = createAuthMiddleware(OPTIONS)(request(`/iam${suffix}`));

    expect(res.headers.get('location')).toBeNull();
    expect(res.headers.get('x-middleware-next')).toBe('1');
  });

  // The SMA-511 failure, kept as a test. A basePath-PREFIXED public path never MATCHES, because
  // Next never hands the proxy a prefixed pathname — so the zone's own login route redirected to
  // itself, forever, with no error anywhere. Both options are plain `string`, so the old full-path
  // form still type checks; since the review's defect 2 it is refused at runtime instead (see
  // `assertBasePathRelative`), which turns a silent loop into a loud failure.
  it('refuses a basePath-PREFIXED publicPaths entry, naming the relative form', () => {
    const run = (): NextResponse => createAuthMiddleware({ publicPaths: ['/iam/auth/login'], loginPath: '/auth/login' })(request('/iam/auth/login'));

    expect(run).toThrow(/publicPaths must be basePath-RELATIVE/);
    expect(run).toThrow(/Write "\/auth\/login" instead/);
  });

  it('refuses a basePath-PREFIXED loginPath, naming the relative form', () => {
    const run = (): NextResponse => createAuthMiddleware({ publicPaths: authRoutePaths(), loginPath: '/iam/auth/login' })(request('/iam/dashboard'));

    expect(run).toThrow(/loginPath must be basePath-RELATIVE/);
    expect(run).toThrow(/Write "\/auth\/login" instead/);
  });

  // The other direction: the CORRECT form is not refused, for a guarded path and a public one
  // alike. Without this, an assertion that threw on everything would pass the two cases above.
  it('accepts the basePath-RELATIVE form', () => {
    expect(() => createAuthMiddleware(OPTIONS)(request('/iam/dashboard'))).not.toThrow();
    expect(() => createAuthMiddleware(OPTIONS)(request('/iam/auth/login'))).not.toThrow();
  });

  // A root-mounted zone has no prefix to carry, so the two forms are one string and nothing is
  // refused. An assertion that compared against `''` would reject every path here.
  it('refuses nothing on a root-mounted zone, where basePath is the empty string', () => {
    expect(() => createAuthMiddleware(OPTIONS)(request('/dashboard', undefined, ''))).not.toThrow();
  });

  it('works for a root-mounted zone, where basePath is the empty string', () => {
    const res = createAuthMiddleware(OPTIONS)(request('/dashboard', undefined, ''));

    const location = new URL(res.headers.get('location') ?? '');
    expect(location.pathname).toBe('/auth/login');
    expect(location.searchParams.get('returnTo')).toBe('/dashboard');
  });
});

// I5 (final fix wave). `publicPaths` used to be the caller's own hand-copied list, bound to
// `http/routes.ts`'s route table by nothing — omit one path there (the callback path is the easy
// one to miss) and a signed-out visitor loops between `/auth/login` and `/auth/callback` forever,
// with no error anywhere. `authRoutePaths()` replaces the hand copy with a derivation; this
// cross-checks it against the REAL route table in `http/routes.ts`.
describe('authRoutePaths (I5)', () => {
  it('returns the four basePath-RELATIVE paths createAuthRoutes dispatches on', () => {
    expect(authRoutePaths()).toEqual(['/auth/login', '/auth/callback', '/auth/logout', '/auth/logout/callback']);
  });

  // Each path, under the zone's basePath, must be recognised by createAuthRoutes (a 405 on the
  // wrong method, never a 404), and a lookalike path must still 404. Only `runtime.basePath` is
  // read before routes.ts's method dispatch, so a partial fixture is enough here.
  it('every path, under the basePath, is recognised by createAuthRoutes, and a lookalike is not', async () => {
    const fakeRuntime = { basePath: '/iam' } as unknown as AuthRuntime;
    const routes = createAuthRoutes(fakeRuntime);

    for (const path of authRoutePaths()) {
      const wrongMethod = path.endsWith('/logout') ? 'GET' : 'POST';
      const res = await routes.handle(new Request(`https://rp.example.com/iam${path}`, { method: wrongMethod }));
      expect(res.status).toBe(405);
    }

    const res = await routes.handle(new Request('https://rp.example.com/iam/auth/not-a-real-route'));
    expect(res.status).toBe(404);
  });
});

// ---------------------------------------------------------------------------------------------
// AC 4's structural assertion: src/middleware.ts's transitive import graph reaches no store, no
// resolver, and no openid-client module. A doc comment saying so is not a control; this is.
//
// The walker itself (ImportGraph, resolveRelative, moduleSpecifiers, collectImportGraph, and the
// DYNAMIC_IMPORT_OR_REQUIRE backstop) now lives in tests/support/import-graph.ts (task 12), so
// tests/structure/import-graph.test.ts's client-entry walk reuses this exact, already-reviewed
// implementation instead of a second copy that could drift from it. Only the middleware-specific
// forbidden-path predicate stays local, since the client walk's forbidden set is different.
// ---------------------------------------------------------------------------------------------

/** True if any reached file is a store, resolver, or single-flight module — see AC 4. */
function reachesStoreOrResolver(files: Set<string>): string[] {
  const forbidden = /[/\\](adapters[/\\](redis-store|memory-store|claims-resolver|oidc)|ports[/\\](session-store|principal-resolver)|core[/\\]single-flight)\.ts$/;
  return [...files].filter((f) => forbidden.test(f));
}

describe('middleware import graph (AC 4)', () => {
  it('reaches no store, resolver, or openid-client module', () => {
    const graph = collectImportGraph(resolve(SRC, 'middleware.ts'));

    expect(graph.packages.has('openid-client')).toBe(false);
    expect(graph.packages.has('redis')).toBe(false);
    expect(reachesStoreOrResolver(graph.files)).toEqual([]);
    // Backstop (review round 1): no file the STATIC walk reached may itself contain a dynamic
    // `import(...)`/`require(...)` call — see the comment above `DYNAMIC_IMPORT_OR_REQUIRE` for
    // why the declaration-only walk above cannot see those on its own.
    expect(filesWithDynamicImportOrRequire(graph.files)).toEqual([]);
  });

  // POSITIVE CONTROL. Without this, a broken walker (e.g. one that silently resolves nothing)
  // would report an empty package set for EVERY entry file, and the assertion above would pass
  // vacuously forever. src/server.ts is known, by construction, to reach openid-client (it
  // re-exports createOidcClient from adapters/oidc.ts) — if the walker cannot see that, it cannot
  // be trusted to prove middleware.ts does NOT reach it either.
  it('positive control: the same walker reaches openid-client from src/server.ts', () => {
    const graph = collectImportGraph(resolve(SRC, 'server.ts'));

    expect(graph.packages.has('openid-client')).toBe(true);
    expect(reachesStoreOrResolver(graph.files).length).toBeGreaterThan(0);
  });
});

// The backstop's own positive control — otherwise a typo in DYNAMIC_IMPORT_OR_REQUIRE (or a
// filter predicate that always returns false) would pass every assertion above vacuously, exactly
// the failure shape the rest of this file's positive control exists to rule out for the walker
// itself.
describe('the dynamic-import/require backstop is not vacuous', () => {
  function withTempFile(contents: string, run: (path: string) => void): void {
    const path = join(tmpdir(), `middleware-backstop-${randomUUID()}.ts`);
    writeFileSync(path, contents);
    try {
      run(path);
    } finally {
      rmSync(path);
    }
  }

  it('flags a file containing a dynamic import() call', () => {
    withTempFile("export const x = await import('./whatever.js');\n", (path) => {
      expect(filesWithDynamicImportOrRequire(new Set([path]))).toEqual([path]);
    });
  });

  it('flags a file containing a require() call', () => {
    withTempFile("const x = require('./whatever.js');\n", (path) => {
      expect(filesWithDynamicImportOrRequire(new Set([path]))).toEqual([path]);
    });
  });

  it('does not flag a file with only static imports', () => {
    withTempFile("import { foo } from './foo.js';\nexport { foo };\n", (path) => {
      expect(filesWithDynamicImportOrRequire(new Set([path]))).toEqual([]);
    });
  });
});

// ---------------------------------------------------------------------------------------------
// SMA-626 § 3. authRoutePaths and createAuthRoutes's dispatch used to be two independent lists.
// The existing cross-check above catches a STALE path; nothing caught a route ADDED to the
// dispatch and forgotten in authRoutePaths, which is the infinite-redirect-loop failure the helper
// exists to prevent. Both now derive from one table, so the type system closes it — a fifth suffix
// with no handler, or a handler with no suffix, fails typecheck.
// ---------------------------------------------------------------------------------------------
describe('the shared route table (SMA-626 § 3)', () => {
  it('authRoutePaths is exactly the table, basePath-relative', () => {
    expect(authRoutePaths()).toEqual([...AUTH_ROUTE_SUFFIXES]);
  });

  it('holds four suffixes, each starting with /auth/', () => {
    expect(AUTH_ROUTE_SUFFIXES).toHaveLength(4);
    for (const suffix of AUTH_ROUTE_SUFFIXES) expect(suffix.startsWith('/auth/')).toBe(true);
  });

  // THE BACKSTOP. The Record is the primary control, but a hand-written `if (pathname === ...)`
  // arm placed before the lookup would still bypass it. After this task there must be ZERO direct
  // comparisons of `pathname` in routes.ts, so any occurrence reds — a much stronger assertion
  // than counting to four, which `==`, reversed operands, or a comment all defeat.
  //
  // RESIDUAL, stated: a comparison written another way — a `switch (pathname)`, an aliased
  // variable, a dispatch in a delegated file, or a `pathname.endsWith(...)` /
  // `.startsWith(...)` check (the exact shape this file carried before review round 1's M5 fix,
  // and the one bypass in this list with a proven history here) — escapes this. The Record is
  // what closes the case that actually happens.
  it('routes.ts contains no direct pathname comparison', () => {
    const source = readFileSync(resolve(SRC, 'http/routes.ts'), 'utf8');
    const matches = source.match(/pathname\s*===?|===?\s*pathname/g) ?? [];
    expect(matches).toEqual([]);
  });

  // THE GATE THAT GATES THE GATE. Every control above (this file's own tests, typecheck) reads
  // only the tuple's VALUES or the file's TEXT for a `pathname` comparison — none of them notices
  // if `ROUTES`'s annotation is silently widened from `Record<AuthRouteSuffix, RouteEntry>` to
  // `Record<string, RouteEntry>`. That widening is the one edit that disables the whole task:
  // typecheck stops requiring the Record to hold exactly the four tuple keys, both Step-6 probes
  // (a fifth suffix with no entry, an entry with no suffix) stop failing, and the comment above
  // `ROUTES` claiming "Both directions, at build time" becomes false with nothing to report it.
  //
  // MEASURED 2026-09-10: with the annotation widened to `Record<string, RouteEntry>` (no other
  // change), this assertion REDS — `expect(source).toContain(...)` fails because the exact
  // substring `Record<AuthRouteSuffix, RouteEntry>` is no longer present — while every OTHER test
  // in this file stays GREEN (14 passed, 1 failed: this one).
  //
  // CORRECTION to the widening's predicted blast radius: `moon run paigasus-auth-ts:typecheck`
  // does NOT stay green under that same edit in this repo — it separately reds with a
  // `noUncheckedIndexedAccess`-driven error (`tsconfig.base.json` sets that flag), because
  // `ROUTES[suffix]` (line 83's Map-building call) then types as `RouteEntry | undefined`
  // instead of `RouteEntry`, which fails to satisfy `Map`'s constructor overloads. That is a
  // real, independent build-time signal — but it is a side effect of THIS FILE'S particular
  // `ROUTES[suffix]` indexing shape, not of the Record's key set being widened per se: a
  // differently-written lookup (e.g. one going through `.get()` with a fallback) could widen the
  // same way and keep typechecking clean. This assertion is what closes that gap: it does not
  // depend on how `ROUTES` happens to be indexed elsewhere in the file. Reverted immediately
  // after confirming both reds (edited back, not `git checkout --`).
  it('the ROUTES table is still typed as Record<AuthRouteSuffix, RouteEntry>, not a widened Record<string, ...>', () => {
    const source = readFileSync(resolve(SRC, 'http/routes.ts'), 'utf8');
    expect(source).toContain('Record<AuthRouteSuffix, RouteEntry>');
  });
});
