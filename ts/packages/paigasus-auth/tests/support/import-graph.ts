// SPDX-License-Identifier: Apache-2.0
//
// Shared static import-graph walker. Built in tests/middleware.test.ts (task 10) and hardened in
// its fix round (the dynamic import()/require() backstop below), then extracted here in task 12 so
// tests/structure/import-graph.test.ts can reuse the exact same resolver and backstop instead of
// maintaining a second copy that could drift from the reviewed one. tests/middleware.test.ts now
// imports from here too — there is exactly one walker in this package, not two.
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import ts from 'typescript';

export interface ImportGraph {
  /** Every relative-import file reached, as absolute paths, INCLUDING the entry file itself. */
  files: Set<string>;
  /** Every bare (non-relative) module specifier encountered — recorded, never walked into. */
  packages: Set<string>;
}

// WHAT THIS WALKER SEES, AND WHAT IT DOES NOT (review round 1). `moduleSpecifiers` below reads
// only STATIC `ImportDeclaration`/`ExportDeclaration` nodes — the ones ECMAScript requires to sit
// at a module's top level, which is exactly why a single-level `forEachChild` (not a recursive
// visit) is enough to find every one of them. It is blind to a DYNAMIC `import(...)` call or a
// `require(...)` call: both are ordinary call expressions, legal anywhere in a function body, and
// invisible to a walk that only looks for import/export declaration nodes. A future edit that
// swapped a static `import { redisStore } from './adapters/redis-store.js'` for
// `await import('./adapters/redis-store.js')` would satisfy the declaration-based check while
// still giving the entry file a real, working path to the store at runtime.
//
// The regex below is a deliberately blunt BACKSTOP for that gap, not a replacement for the
// declaration walk: it fails the whole graph if ANY walked file contains the text `import(` or
// `require(` at all, without trying to resolve what it points at. That is conservative by design —
// a legitimate dynamic import would also fail it — but nothing in this package's `src/` uses one
// today, and "the graph must not even ATTEMPT a dynamic escape hatch" is exactly what both AC 4
// (middleware) and AC 5 (client) need.
export const DYNAMIC_IMPORT_OR_REQUIRE = /\b(?:import|require)\s*\(/;

/** Every walked file whose raw source contains a dynamic `import(...)` or `require(...)` call. */
export function filesWithDynamicImportOrRequire(files: Set<string>): string[] {
  return [...files].filter((file) => DYNAMIC_IMPORT_OR_REQUIRE.test(readFileSync(file, 'utf8')));
}

/** Resolve a relative import specifier (repo convention: `.js` extension, real file is `.ts`/`.tsx`) to a file on disk. */
export function resolveRelative(fromFile: string, specifier: string): string | null {
  if (!specifier.startsWith('.')) return null;
  let base = resolve(dirname(fromFile), specifier);
  if (base.endsWith('.js')) base = base.slice(0, -'.js'.length);
  for (const ext of ['.ts', '.tsx']) {
    const candidate = `${base}${ext}`;
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

/**
 * Every module specifier an `import`/`export ... from` declaration in this file names.
 *
 * This reads BOTH value and type-only declarations: `ts.isImportDeclaration` returns true for
 * `import type { X } from './y.js'` just as it does for a value import, and this function does not
 * inspect `importClause.isTypeOnly` to filter it out. That is a deliberate choice (task 12), not an
 * oversight — see each call site's own comment for why treating a type-only specifier as a real
 * edge is the right call for that particular walk.
 */
export function moduleSpecifiers(file: string): string[] {
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

/**
 * Thrown by `collectImportGraph` when a relative specifier cannot be resolved to a file on disk —
 * a distinct, named failure instead of silently dropping the specifier into `packages` and
 * stopping traversal there. This walker enforces AC 4 and AC 5 (see tests/middleware.test.ts and
 * tests/structure/import-graph.test.ts): a silent truncation would make both assertions weaker
 * than they read, since a banned directory reached only through an unresolvable relative import
 * would never show up in `files` at all.
 */
export class UnresolvedRelativeImportError extends Error {
  constructor(fromFile: string, specifier: string) {
    super(`cannot resolve relative import "${specifier}" from "${fromFile}"`);
    this.name = 'UnresolvedRelativeImportError';
  }
}

/** BFS over the relative-import closure starting at `entry`, recording every bare specifier found. */
export function collectImportGraph(entry: string): ImportGraph {
  const files = new Set<string>();
  const packages = new Set<string>();
  const stack = [entry];

  while (stack.length > 0) {
    const file = stack.pop();
    if (file === undefined || files.has(file)) continue;
    files.add(file);

    for (const specifier of moduleSpecifiers(file)) {
      if (!specifier.startsWith('.')) {
        packages.add(specifier);
        continue;
      }
      const resolved = resolveRelative(file, specifier);
      if (resolved === null) throw new UnresolvedRelativeImportError(file, specifier);
      stack.push(resolved);
    }
  }

  return { files, packages };
}
