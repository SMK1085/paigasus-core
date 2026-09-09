// SPDX-License-Identifier: Apache-2.0
//
// AC 1, spec § 6.2 layer 3. Driven off package.json's `exports` map rather than a hand-written
// list, so an entry point added later is covered the day it is added — which is what makes it safe
// for PR B to ship two entries while the design spec's § 6.1 names five (see the plan's D1).
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const PKG_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');

// Entries that deliberately carry NO guard. `./errors/types` (PR C) holds only types: under
// `verbatimModuleSyntax` an `import type` emits nothing, so a client component can name those types
// with no runtime import and no server-only evaluation (spec § 6.3). Every OTHER entry is guarded.
const UNGUARDED_ENTRIES = new Set(['./errors/types']);

const GUARD_IMPORT = "import './server-guard.js';";

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
    expect(firstImportStatement(source)).toBe(GUARD_IMPORT);
  });

  it.each(entries.filter(([name]) => UNGUARDED_ENTRIES.has(name)))('entry %s deliberately carries no guard', (_name, target) => {
    const source = readFileSync(resolve(PKG_ROOT, target), 'utf8');
    expect(source).not.toContain(GUARD_IMPORT);
  });
});

describe("AC 1 — 'server-only' is imported at exactly one site", () => {
  it('server-guard.ts imports it first', () => {
    const source = readFileSync(resolve(PKG_ROOT, 'src/server-guard.ts'), 'utf8');
    expect(firstImportStatement(source)).toBe("import 'server-only';");
  });

  it('no other file in src/ imports it', () => {
    const others = globSourceFiles().filter((f) => f !== resolve(PKG_ROOT, 'src/server-guard.ts'));
    const offenders = others.filter((f) => readFileSync(f, 'utf8').includes("'server-only'"));
    expect(offenders).toEqual([]);
  });
});

function globSourceFiles(): string[] {
  // readdirSync with recursive: true — no glob dependency, Node 24 supports it natively.
  return readdirSync(resolve(PKG_ROOT, 'src'), { recursive: true, encoding: 'utf8' })
    .filter((p) => p.endsWith('.ts'))
    .map((p) => resolve(PKG_ROOT, 'src', p));
}
