// SPDX-License-Identifier: Apache-2.0
//
// SMA-511 spec § 7.2. Turbopack in Next 16.3.4 does not resolve './x.js' to './x.ts', and every Next
// zone compiles this generated code through @paigasus/sdk. ts/eslint.config.js ignores
// **/generated/**, so `paigasus/no-js-relative-specifier` never sees this tree: this test is the
// control. contracts/buf.gen.yaml and contracts/buf.gen.googleapis.yaml OMIT protobuf-es's
// `import_extension` option for that reason (the default, `none`, writes extensionless imports).
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const GENERATED = fileURLToPath(new URL('./generated', import.meta.url));

function generatedFiles(): string[] {
  return readdirSync(GENERATED, { recursive: true, encoding: 'utf8' })
    .filter((p) => p.endsWith('.ts'))
    .map((p) => join(GENERATED, p));
}

/** Every RELATIVE specifier of an import or export declaration, read with the TypeScript parser. */
function relativeSpecifiers(file: string): string[] {
  const source = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true);
  const found: string[] = [];
  for (const statement of source.statements) {
    if ((ts.isImportDeclaration(statement) || ts.isExportDeclaration(statement)) && statement.moduleSpecifier !== undefined && ts.isStringLiteral(statement.moduleSpecifier)) {
      if (statement.moduleSpecifier.text.startsWith('.')) found.push(statement.moduleSpecifier.text);
    }
  }
  return found;
}

describe('generated protobuf-es output (SMA-511 spec § 7.2)', () => {
  const files = generatedFiles();

  it('finds the generated tree', () => {
    expect(files.length).toBeGreaterThanOrEqual(8);
  });

  // Without this, a tree whose relative imports all vanished would pass the check below vacuously.
  it('carries relative imports at all', () => {
    expect(files.flatMap(relativeSpecifiers).length).toBeGreaterThan(0);
  });

  it.each(files)('%s has no relative specifier that ends in .js', (file) => {
    expect(relativeSpecifiers(file).filter((specifier) => specifier.endsWith('.js'))).toEqual([]);
  });
});
