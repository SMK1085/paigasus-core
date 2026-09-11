// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const pkg = JSON.parse(readFileSync(fileURLToPath(new URL('../../package.json', import.meta.url)), 'utf8')) as { exports: Record<string, string>; dependencies: Record<string, string> };

const read = (rel: string): string => readFileSync(fileURLToPath(new URL(`../../${rel}`, import.meta.url)), 'utf8');

const parse = (rel: string): ts.SourceFile => ts.createSourceFile(rel, read(rel), ts.ScriptTarget.Latest, true);

type ModuleImport = { readonly specifier: string; readonly typeOnly: boolean };

/**
 * Every import and re-export of a file, read with the TypeScript parser, so a specifier inside a
 * comment never counts. "Type-only" means a CLAUSE-level `import type` / `export type`. Under
 * verbatimModuleSyntax, `import { type A } from './a'` still emits `import './a'`, so the bundler
 * still resolves it.
 */
function importsOf(rel: string): ModuleImport[] {
  const found: ModuleImport[] = [];
  for (const statement of parse(rel).statements) {
    if (ts.isImportDeclaration(statement) && ts.isStringLiteral(statement.moduleSpecifier)) {
      found.push({ specifier: statement.moduleSpecifier.text, typeOnly: statement.importClause?.isTypeOnly === true });
    } else if (ts.isExportDeclaration(statement) && statement.moduleSpecifier !== undefined && ts.isStringLiteral(statement.moduleSpecifier)) {
      found.push({ specifier: statement.moduleSpecifier.text, typeOnly: statement.isTypeOnly });
    }
  }
  return found;
}

/** True when the FIRST STATEMENT is the 'use client' directive (comments before it are allowed). */
function firstStatementIsUseClient(rel: string): boolean {
  const first = parse(rel).statements[0];
  return first !== undefined && ts.isExpressionStatement(first) && ts.isStringLiteral(first.expression) && first.expression.text === 'use client';
}

/**
 * The files a consuming Next app compiles when it imports `@paigasus/discovery/client` (spec § 9.2),
 * with each file's exact import set. A later import that pulls a server module (or protobuf-es,
 * through core/state.ts) into this entry fails here.
 */
const CLIENT_GRAPH: Readonly<Record<string, readonly string[]>> = {
  'src/client.ts': ['./core/outcome', './disabled'],
  'src/disabled.tsx': ['./types.js', 'react'],
  'src/core/outcome.ts': ['../types', './service-of'],
  'src/core/service-of.ts': [],
};

describe('package boundary', () => {
  it('exposes exactly four subpaths and NO root export', () => {
    // A root export re-exporting the server surface would let a client component import it and
    // walk around the boundary — the reason @paigasus/auth omits one too.
    expect(Object.keys(pkg.exports).sort()).toEqual(['./client', './react', './server', './types']);
    expect(pkg.exports['./client']).toBe('./src/client.ts');
    expect(pkg.exports['.']).toBeUndefined();
  });

  it('does not depend on @paigasus/sdk', () => {
    // Three reasons: the SDK's Presentation union cannot express a DegradedReason; the probe is
    // a bare fetch, so @connectrpc/* is pure cost; and `paigasus/boundaries/app-shell` bans the
    // SDK from app-shell, which a transitive edge through this package would violate.
    expect(Object.keys(pkg.dependencies)).not.toContain('@paigasus/sdk');
  });

  it('guards the server entries and deliberately leaves ./types and ./client unguarded', () => {
    expect(read('src/server.ts')).toMatch(/^import 'server-only';/m);
    expect(read('src/react.tsx')).toMatch(/^import 'server-only';/m);
    // ./types and ./client are client-safe on purpose (spec § 9.2).
    expect(read('src/types.ts')).not.toMatch(/server-only/);
  });

  it.each(Object.entries(CLIENT_GRAPH))('%s imports exactly its pinned set, and never server-only', (rel, expected) => {
    const specifiers = importsOf(rel)
      .map((entry) => entry.specifier)
      .sort();
    expect(specifiers).toEqual([...expected].sort());
    expect(specifiers).not.toContain('server-only');
  });

  it.each(Object.keys(CLIENT_GRAPH))('%s has no relative VALUE import with a .js extension', (rel) => {
    // MEASURED (SMA-510): Turbopack in Next 16.3.4 does not map './x.js' to './x.ts'. A clause-level
    // `import type` is erased before resolution and is safe. Any other `.js` relative specifier here
    // fails the consuming app's `next build` — and nothing in this package would notice.
    const offenders = importsOf(rel).filter((entry) => entry.specifier.startsWith('.') && !entry.typeOnly && entry.specifier.endsWith('.js'));
    expect(offenders).toEqual([]);
  });

  it('client.ts carries no directive; disabled.tsx keeps its own', () => {
    // client.ts must stay directive-free: a server layout calls capabilityOutcome through it, and an
    // export of a 'use client' module is only a client reference on the server (spec § 5.2).
    expect(firstStatementIsUseClient('src/client.ts')).toBe(false);
    expect(firstStatementIsUseClient('src/disabled.tsx')).toBe(true);
    expect(firstStatementIsUseClient('src/core/outcome.ts')).toBe(false);
  });

  it('never sets the react-server vitest condition', () => {
    // MEASURED in @paigasus/auth: react-server is also the condition `react`'s own exports map
    // switches on, and that build has no createContext, so setting it breaks react-dom/client
    // under @testing-library/react. The `server-only` alias is the fix instead.
    //
    // The assertion checks for the QUOTED condition value, not the bare word: this file's own
    // comment explaining the omission legitimately contains the bare word "react-server" in
    // backticks, and a plain substring check would fail against that prose rather than against
    // an actual condition setting.
    const config = read('vitest.config.ts');
    expect(config).not.toContain("'react-server'");
    expect(config).toContain("alias: { 'server-only'");
  });
});
