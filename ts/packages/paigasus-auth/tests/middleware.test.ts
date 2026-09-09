// SPDX-License-Identifier: Apache-2.0
//
// AC 4: middleware checks cookie PRESENCE only, never validity, and cannot even import anything
// that knows how to validate one — CVE-2025-29927 was exactly a middleware auth bypass.
import { randomUUID } from 'node:crypto';
import { rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { NextRequest } from 'next/server';
import { createAuthMiddleware } from '../src/middleware.js';
import { SESSION_COOKIE } from '../src/http/cookies.js';
import { collectImportGraph, filesWithDynamicImportOrRequire } from './support/import-graph.js';

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(HERE, '../src');

const OPTIONS = {
  publicPaths: ['/iam/auth/login', '/iam/auth/callback', '/iam/auth/logout', '/iam/auth/logout/callback'],
  loginPath: '/iam/auth/login',
};

function request(path: string, cookie?: string): NextRequest {
  const headers = cookie !== undefined ? { cookie } : {};
  return new NextRequest(`https://app.example.com${path}`, { headers });
}

describe('createAuthMiddleware', () => {
  it('redirects a guarded path with no session cookie to the login path with returnTo', () => {
    const middleware = createAuthMiddleware(OPTIONS);
    const res = middleware(request('/iam/dashboard?tab=1'));

    expect(res.status).toBe(307); // NextResponse.redirect's default status
    const location = new URL(res.headers.get('location') ?? '');
    expect(location.pathname).toBe('/iam/auth/login');
    expect(location.searchParams.get('returnTo')).toBe('/iam/dashboard?tab=1');
  });

  it('passes through a guarded path carrying ANY cookie value, including a forged one', () => {
    const middleware = createAuthMiddleware(OPTIONS);
    const res = middleware(request('/iam/dashboard', `${SESSION_COOKIE}=totally-forged-not-a-real-sid`));

    expect(res.headers.get('location')).toBeNull();
    // NextResponse.next() sets this marker header — proof this is a "continue" response, not a
    // freshly constructed 200 that merely looks like one.
    expect(res.headers.get('x-middleware-next')).toBe('1');
  });

  it('passes through a public path with no cookie at all', () => {
    const middleware = createAuthMiddleware(OPTIONS);
    const res = middleware(request('/iam/auth/login'));

    expect(res.headers.get('location')).toBeNull();
    expect(res.headers.get('x-middleware-next')).toBe('1');
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
