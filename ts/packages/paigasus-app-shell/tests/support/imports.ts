// SPDX-License-Identifier: Apache-2.0
import { readFileSync, readdirSync } from 'node:fs';
import ts from 'typescript';

export type ModuleImport = { readonly specifier: string; readonly typeOnly: boolean };

function parse(file: string): ts.SourceFile {
  return ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true);
}

/**
 * Every import and re-export of a file, read with the TypeScript parser, so a specifier inside a
 * comment never counts. "Type-only" means a CLAUSE-level `import type` / `export type`. Under
 * verbatimModuleSyntax, `import { type A } from './a'` still emits `import './a'`, so the bundler
 * still resolves it.
 */
export function importsOf(file: string): ModuleImport[] {
  const found: ModuleImport[] = [];
  for (const statement of parse(file).statements) {
    if (ts.isImportDeclaration(statement) && ts.isStringLiteral(statement.moduleSpecifier)) {
      found.push({ specifier: statement.moduleSpecifier.text, typeOnly: statement.importClause?.isTypeOnly === true });
    } else if (ts.isExportDeclaration(statement) && statement.moduleSpecifier !== undefined && ts.isStringLiteral(statement.moduleSpecifier)) {
      found.push({ specifier: statement.moduleSpecifier.text, typeOnly: statement.isTypeOnly });
    }
  }
  return found;
}

/** True when the file's FIRST STATEMENT is the 'use client' directive (comments before it are allowed). */
export function isUseClient(file: string): boolean {
  const first = parse(file).statements[0];
  return first !== undefined && ts.isExpressionStatement(first) && ts.isStringLiteral(first.expression) && first.expression.text === 'use client';
}

/** Every .ts and .tsx file under `dir`, as a sorted path relative to `dir`. */
export function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { recursive: true, encoding: 'utf8' })
    .filter((rel) => /\.tsx?$/.test(rel))
    .sort();
}
