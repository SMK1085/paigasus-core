// SPDX-License-Identifier: Apache-2.0
//
// AC 4: middleware checks cookie PRESENCE only, never validity, and cannot even import anything
// that knows how to validate one — CVE-2025-29927 was exactly a middleware auth bypass.
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import ts from 'typescript';
import { NextRequest } from 'next/server';
import { createAuthMiddleware } from '../src/middleware.js';
import { SESSION_COOKIE } from '../src/http/cookies.js';

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
// ---------------------------------------------------------------------------------------------

interface ImportGraph {
  /** Every relative-import file reached, as absolute paths, INCLUDING the entry file itself. */
  files: Set<string>;
  /** Every bare (non-relative) module specifier encountered — recorded, never walked into. */
  packages: Set<string>;
}

/** Resolve a relative import specifier (repo convention: `.js` extension, real file is `.ts`/`.tsx`) to a file on disk. */
function resolveRelative(fromFile: string, specifier: string): string | null {
  if (!specifier.startsWith('.')) return null;
  let base = resolve(dirname(fromFile), specifier);
  if (base.endsWith('.js')) base = base.slice(0, -'.js'.length);
  for (const ext of ['.ts', '.tsx']) {
    const candidate = `${base}${ext}`;
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

/** Every module specifier an `import`/`export ... from` declaration in this file names. */
function moduleSpecifiers(file: string): string[] {
  const source = readFileSync(file, 'utf8');
  const sourceFile = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true);
  const specifiers: string[] = [];
  sourceFile.forEachChild((node) => {
    const hasModuleSpecifier = ts.isImportDeclaration(node) || ts.isExportDeclaration(node);
    if (!hasModuleSpecifier) return;
    const moduleSpecifier = node.moduleSpecifier;
    if (moduleSpecifier !== undefined && ts.isStringLiteral(moduleSpecifier)) {
      specifiers.push(moduleSpecifier.text);
    }
  });
  return specifiers;
}

/** BFS over the relative-import closure starting at `entry`, recording every bare specifier found. */
function collectImportGraph(entry: string): ImportGraph {
  const files = new Set<string>();
  const packages = new Set<string>();
  const stack = [entry];

  while (stack.length > 0) {
    const file = stack.pop();
    if (file === undefined || files.has(file)) continue;
    files.add(file);

    for (const specifier of moduleSpecifiers(file)) {
      const resolved = resolveRelative(file, specifier);
      if (resolved !== null) {
        stack.push(resolved);
      } else {
        packages.add(specifier);
      }
    }
  }

  return { files, packages };
}

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
