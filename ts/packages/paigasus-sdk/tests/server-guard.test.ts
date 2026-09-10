// SPDX-License-Identifier: Apache-2.0
//
// AC 1, spec § 6.2 layer 3. Driven off package.json's `exports` map rather than a hand-written
// list, so an entry point added later is covered the day it is added — which is what makes it safe
// for PR B to ship two entries while the design spec's § 6.1 names five (see the plan's D1).
import { readdirSync, readFileSync, writeFileSync, unlinkSync } from 'node:fs';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const PKG_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');

// Entries that deliberately carry NO guard. `./errors/types` (PR C) holds only types: under
// `verbatimModuleSyntax` an `import type` emits nothing, so a client component can name those types
// with no runtime import and no server-only evaluation (spec § 6.3). Every OTHER entry is guarded.
const UNGUARDED_ENTRIES = new Set(['./errors/types']);

// The guard's expected specifier depends on where the entry FILE sits, not on one fixed literal.
// `src/index.ts` needs './server-guard.js'; a nested entry such as `src/errors/map-error.ts` needs
// '../server-guard.js'. Spec § 6.2 layer 3 always said this test "resolves the path relative to
// each entry" — the original literal did not, so a nested guarded entry could not satisfy it and
// the file layout had to be bent around the test. Ported from PR #231, which fixed the test
// instead. (SMA-625)
const GUARD_MODULE = 'src/server-guard.js';

function expectedGuardImport(target: string): string {
  const fromDir = dirname(resolve(PKG_ROOT, target));
  const specifier = relative(fromDir, resolve(PKG_ROOT, GUARD_MODULE)).split(sep).join('/');
  return `import '${specifier.startsWith('.') ? specifier : `./${specifier}`}';`;
}

/** Does this source carry an import declaration of the server guard, at any relative depth? */
function importsServerGuard(source: string, fileName: string): boolean {
  const parsed = ts.createSourceFile(fileName, source, ts.ScriptTarget.ESNext, true);
  return parsed.statements.some((statement) => ts.isImportDeclaration(statement) && ts.isStringLiteral(statement.moduleSpecifier) && /(?:^|\/)server-guard\.js$/.test(statement.moduleSpecifier.text));
}

function readPackageExports(): Record<string, string> {
  const raw = readFileSync(resolve(PKG_ROOT, 'package.json'), 'utf8');
  const parsed = JSON.parse(raw) as { exports?: Record<string, string> };
  const exportsMap = parsed.exports;
  if (exportsMap === undefined) throw new Error('package.json declares no exports map');
  return exportsMap;
}

// The FIRST IMPORT STATEMENT, not the first line: every file opens with the SPDX header and most
// carry a comment block after it.
function firstImportStatement(source: string): string | undefined {
  return source.split('\n').find((line) => line.startsWith('import '));
}

describe('AC 1 — every guarded entry point imports the server guard first', () => {
  const entries = Object.entries(readPackageExports());

  it('finds at least one entry to check', () => {
    expect(entries.length).toBeGreaterThan(0);
  });

  // `entries.length > 0` above protects the whole map, not the GUARDED partition specifically:
  // moving every entry into UNGUARDED_ENTRIES would leave that assertion green with the `it.each`
  // below iterating zero cases — a suite that reports all-pass while checking nothing. This
  // assertion pins the partition itself, not only the map it is drawn from.
  it('finds at least one GUARDED entry to check', () => {
    expect(entries.filter(([name]) => !UNGUARDED_ENTRIES.has(name)).length).toBeGreaterThan(0);
  });

  it.each(entries.filter(([name]) => !UNGUARDED_ENTRIES.has(name)))('entry %s imports the guard as its first import statement', (_name, target) => {
    const source = readFileSync(resolve(PKG_ROOT, target), 'utf8');
    expect(firstImportStatement(source)).toBe(expectedGuardImport(target));
  });

  // PARSED, not pattern-matched, and for the reason the second describe block below already
  // records: a text match cannot tell an import from a mention. The first attempt here used a
  // regex and immediately flagged the prose in `errors/types.ts` that says the file must not gain
  // a guard. It also rejects ANY server-guard specifier rather than only the spelling this
  // entry's depth would produce, so a stray './server-guard.js' in a nested file is still caught.
  it.each(entries.filter(([name]) => UNGUARDED_ENTRIES.has(name)))('entry %s deliberately carries no guard', (_name, target) => {
    const file = resolve(PKG_ROOT, target);
    expect(importsServerGuard(readFileSync(file, 'utf8'), file)).toBe(false);
  });
});

// Parse rather than pattern-match. Three review rounds each found a different VALID import that a
// regex missed — a double-quoted specifier, a trailing line comment, and a block comment carrying
// a line terminator. Each fix invited the next evasion, because "does this file import
// server-only" is a question about syntax, not about text. `typescript` is already a devDependency
// here, so the exact answer is cheap: the parser sees through every quote style, comment
// placement and line break by construction.
function importsServerOnly(source: string, fileName: string): boolean {
  const parsed = ts.createSourceFile(fileName, source, ts.ScriptTarget.ESNext, true);
  return parsed.statements.some((statement) => ts.isImportDeclaration(statement) && ts.isStringLiteral(statement.moduleSpecifier) && statement.moduleSpecifier.text === 'server-only');
}

describe("AC 1 — 'server-only' is imported at exactly one site", () => {
  it('server-guard.ts imports it first', () => {
    const source = readFileSync(resolve(PKG_ROOT, 'src/server-guard.ts'), 'utf8');
    expect(firstImportStatement(source)).toBe("import 'server-only';");
  });

  it('no other file in src/ imports it', () => {
    const others = globSourceFiles().filter((f) => f !== resolve(PKG_ROOT, 'src/server-guard.ts'));
    const offenders = others.filter((f) => importsServerOnly(readFileSync(f, 'utf8'), f));
    expect(offenders).toEqual([]);
  });

  it('detects double-quoted server-only import as offender', () => {
    const probeFile = resolve(PKG_ROOT, 'src/__quote-probe.ts');
    try {
      const content = `// SPDX-License-Identifier: Apache-2.0
import "server-only";
`;
      writeFileSync(probeFile, content, 'utf8');

      const others = globSourceFiles().filter((f) => f !== resolve(PKG_ROOT, 'src/server-guard.ts'));
      const offenders = others.filter((f) => importsServerOnly(readFileSync(f, 'utf8'), f));
      expect(offenders).toContain(probeFile);
    } finally {
      try {
        unlinkSync(probeFile);
      } catch {
        // Ignore cleanup errors
      }
    }
  });

  it('does not flag server-only inside a comment as offender', () => {
    const probeFile = resolve(PKG_ROOT, 'src/__comment-probe.ts');
    try {
      const content = `// SPDX-License-Identifier: Apache-2.0
// This mentions server-only in prose, not as an import.
export const guard = true;
`;
      writeFileSync(probeFile, content, 'utf8');

      const others = globSourceFiles().filter((f) => f !== resolve(PKG_ROOT, 'src/server-guard.ts'));
      const offenders = others.filter((f) => importsServerOnly(readFileSync(f, 'utf8'), f));
      expect(offenders).not.toContain(probeFile);
    } finally {
      try {
        unlinkSync(probeFile);
      } catch {
        // Ignore cleanup errors
      }
    }
  });

  it('detects server-only import with trailing comment as offender', () => {
    const probeFile = resolve(PKG_ROOT, 'src/__trailing-comment-probe.ts');
    try {
      const content = `// SPDX-License-Identifier: Apache-2.0
import "server-only"; // boundary
`;
      writeFileSync(probeFile, content, 'utf8');

      const others = globSourceFiles().filter((f) => f !== resolve(PKG_ROOT, 'src/server-guard.ts'));
      const offenders = others.filter((f) => importsServerOnly(readFileSync(f, 'utf8'), f));
      expect(offenders).toContain(probeFile);
    } finally {
      try {
        unlinkSync(probeFile);
      } catch {
        // Ignore cleanup errors
      }
    }
  });

  it('detects server-only import with block comment noise as offender', () => {
    const probeFile = resolve(PKG_ROOT, 'src/__block-comment-probe.ts');
    try {
      const content = `// SPDX-License-Identifier: Apache-2.0
import /* boundary */ "server-only";
`;
      writeFileSync(probeFile, content, 'utf8');

      const others = globSourceFiles().filter((f) => f !== resolve(PKG_ROOT, 'src/server-guard.ts'));
      const offenders = others.filter((f) => importsServerOnly(readFileSync(f, 'utf8'), f));
      expect(offenders).toContain(probeFile);
    } finally {
      try {
        unlinkSync(probeFile);
      } catch {
        // Ignore cleanup errors
      }
    }
  });

  it('detects server-only import with a multiline block comment as offender', () => {
    const probeFile = resolve(PKG_ROOT, 'src/__multiline-block-comment-probe.ts');
    try {
      // The block comment carries a REAL line terminator (not an escaped "\\n" inside one
      // source line) — this is the case a regex anchored on `[^'"\n]*` cannot see, since the
      // comment crosses a newline before the specifier is reached.
      const content = `// SPDX-License-Identifier: Apache-2.0
import /* boundary
*/ "server-only";
`;
      writeFileSync(probeFile, content, 'utf8');

      const others = globSourceFiles().filter((f) => f !== resolve(PKG_ROOT, 'src/server-guard.ts'));
      const offenders = others.filter((f) => importsServerOnly(readFileSync(f, 'utf8'), f));
      expect(offenders).toContain(probeFile);
    } finally {
      try {
        unlinkSync(probeFile);
      } catch {
        // Ignore cleanup errors
      }
    }
  });

  it('does not flag unrelated import with server-only in comment as offender', () => {
    const probeFile = resolve(PKG_ROOT, 'src/__unrelated-import-probe.ts');
    try {
      const content = `// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs'; // not a server-only import
`;
      writeFileSync(probeFile, content, 'utf8');

      const others = globSourceFiles().filter((f) => f !== resolve(PKG_ROOT, 'src/server-guard.ts'));
      const offenders = others.filter((f) => importsServerOnly(readFileSync(f, 'utf8'), f));
      expect(offenders).not.toContain(probeFile);
    } finally {
      try {
        unlinkSync(probeFile);
      } catch {
        // Ignore cleanup errors
      }
    }
  });
});

function globSourceFiles(): string[] {
  // readdirSync with recursive: true — no glob dependency, Node 24 supports it natively.
  return readdirSync(resolve(PKG_ROOT, 'src'), { recursive: true, encoding: 'utf8' })
    .filter((p) => p.endsWith('.ts'))
    .map((p) => resolve(PKG_ROOT, 'src', p));
}
