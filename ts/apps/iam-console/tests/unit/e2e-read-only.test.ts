// SPDX-License-Identifier: Apache-2.0
//
// The e2e tier must not write into a build tree (SMA-655). iam-console's standalone tree serves
// both this app's tier and gateway-console's two-zone tier, Moon runs the two at the same time,
// and when each tier staged the tree for itself, one tier's delete wiped it under the other.
// Staging belongs to the `build` task (moon.yml); the setups only check it.
//
// This is an ALLOWLIST. From `fs`, `node:fs`, `fs/promises` and `node:fs/promises`, only a named
// import from READ_SET, or a type-only import, passes. Every other form is a red: another named
// import, a default or namespace import, a side-effect import, a re-export, `require(...)`,
// `createRequire` and a dynamic `import(...)`.
//
// Limits (spec § 9, R1): this is a text scan, not a parse. It does not see a write through
// `child_process`, or through a helper module outside tests/e2e/. The comment stripper does not
// know regex literals or a template literal's `${…}`: a regex literal holding a quote, `//` or
// `/*`, or a comment inside `${…}`, can shift the scanner's string state, so it can hide real code
// or expose string text as code. No scanned file has such a shape before its imports today.
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const APP_DIR = fileURLToPath(new URL('../..', import.meta.url));
const E2E_DIR = path.join(APP_DIR, 'tests', 'e2e');
const FS_MODULE = /^(?:node:)?fs(?:\/promises)?$/;
const READ_SET: ReadonlySet<string> = new Set(['existsSync', 'readFileSync', 'readdirSync', 'statSync', 'lstatSync']);
const SCRIPT_FILE = /\.(?:[cm]?[jt]s|tsx)$/;
/** File path (relative to the app, `/` separators) → reason. Ships EMPTY; an entry needs a reason. */
const ALLOWED_EXCEPTIONS: ReadonlyMap<string, string> = new Map();

/** Removes `//` and block comments. String and template contents stay, because a module specifier is a string. */
function stripComments(source: string): string {
  let out = '';
  let quote: string | null = null;
  let i = 0;
  while (i < source.length) {
    const c = source[i];
    const next = source[i + 1];
    if (quote !== null) {
      out += c;
      if (c === '\\' && next !== undefined) {
        out += next;
        i += 2;
        continue;
      }
      if (c === quote) quote = null;
      i += 1;
      continue;
    }
    if (c === '"' || c === "'" || c === '`') {
      quote = c;
      out += c;
      i += 1;
      continue;
    }
    if (c === '/' && next === '/') {
      while (i < source.length && source[i] !== '\n') i += 1;
      continue;
    }
    if (c === '/' && next === '*') {
      const end = source.indexOf('*/', i + 2);
      i = end === -1 ? source.length : end + 2;
      out += ' ';
      continue;
    }
    out += c;
    i += 1;
  }
  return out;
}

/** Every use of an fs module in `source` that is not a named or type-only import from READ_SET. */
function findFsViolations(source: string): string[] {
  const code = stripComments(source);
  const found: string[] = [];
  for (const m of code.matchAll(/\bimport\s+(type\s+)?([^'";]*?)\s*\bfrom\s*(['"])([^'"]+)\3/g)) {
    const [, typeOnly, clause = '', , specifier = ''] = m;
    if (!FS_MODULE.test(specifier) || typeOnly !== undefined) continue;
    const named = /^\{([^}]*)\}$/.exec(clause.trim());
    if (named === null) {
      found.push(`import ${clause.trim()} from '${specifier}'`);
      continue;
    }
    for (const raw of (named[1] ?? '').split(',')) {
      const item = raw.trim();
      if (item === '' || /^type\s/.test(item)) continue;
      const name = (item.split(/\s+as\s+/)[0] ?? '').trim();
      if (!READ_SET.has(name)) found.push(`import { ${name} } from '${specifier}'`);
    }
  }
  const forms: ReadonlyArray<readonly [RegExp, (specifier: string) => string]> = [
    [/\bimport\s*(['"])([^'"]+)\1/g, (s) => `import '${s}'`],
    [/\bexport\s+[^;]*?\bfrom\s*(['"])([^'"]+)\1/g, (s) => `export … from '${s}'`],
    [/\brequire\s*\(\s*(['"])([^'"]+)\1/g, (s) => `require('${s}')`],
    [/\bimport\s*\(\s*(['"])([^'"]+)\1/g, (s) => `import('${s}')`],
  ];
  for (const [pattern, describeForm] of forms) {
    for (const m of code.matchAll(pattern)) {
      const specifier = m[2] ?? '';
      if (FS_MODULE.test(specifier)) found.push(describeForm(specifier));
    }
  }
  if (/\bcreateRequire\b/.test(code)) found.push('createRequire');
  return found;
}

function scannedFiles(): string[] {
  const e2e = readdirSync(E2E_DIR, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile() && SCRIPT_FILE.test(entry.name))
    .map((entry) => path.join(entry.parentPath, entry.name));
  return [...e2e, path.join(APP_DIR, 'playwright.config.ts')];
}

function relative(file: string): string {
  return path.relative(APP_DIR, file).split(path.sep).join('/');
}

describe('findFsViolations', () => {
  const red: ReadonlyArray<readonly [string, string]> = [
    ['a named write import', "import { rmSync } from 'node:fs';"],
    ['an aliased write import', "import { rmSync as remove } from 'fs';"],
    ['a default import', "import fs from 'node:fs';"],
    ['a namespace import', "import * as fs from 'node:fs';"],
    ['a default import beside named reads', "import fs, { existsSync } from 'node:fs';"],
    ['require', "const fs = require('node:fs');"],
    ['a dynamic import', "const fs = await import('node:fs');"],
    ['an fs/promises write import', "import { writeFile } from 'node:fs/promises';"],
    ['a bare fs/promises write import', "import { mkdir } from 'fs/promises';"],
    ['createRequire', "import { createRequire } from 'node:module';\nconst load = createRequire(import.meta.url);"],
    ['a side-effect import', "import 'node:fs';"],
    ['a re-export', "export { rmSync } from 'node:fs';"],
    ['a write import after a string holding //', "const url = 'http://x';\nimport { cpSync } from 'node:fs';"],
  ];
  it.each(red)('reds on %s', (_label, source) => {
    expect(findFsViolations(source)).not.toEqual([]);
  });

  const green: ReadonlyArray<readonly [string, string]> = [
    ['named reads', "import { existsSync, readFileSync } from 'node:fs';"],
    ['an aliased read', "import { statSync as stat } from 'fs';"],
    ['a type-only import', "import type { Stats } from 'node:fs';"],
    ['an inline type beside a read', "import { type Stats, lstatSync } from 'node:fs';"],
    ['commented-out writes', "// import { rmSync } from 'node:fs';\n/* import fs from 'fs'; */"],
    ['a write-named import from another module', "import { rmSync } from './helpers';"],
    ['node:path', "import path from 'node:path';"],
  ];
  it.each(green)('accepts %s', (_label, source) => {
    expect(findFsViolations(source)).toEqual([]);
  });
});

describe('the e2e tree is read-only on every build tree (SMA-655)', () => {
  const files = scannedFiles();

  it('scans the e2e directory, its support directory and playwright.config.ts', () => {
    const scanned = files.map(relative);
    expect(scanned.filter((f) => /^tests\/e2e\/[^/]+$/.test(f)).length).toBeGreaterThan(0);
    expect(scanned.filter((f) => f.startsWith('tests/e2e/support/')).length).toBeGreaterThan(0);
    expect(scanned).toContain('playwright.config.ts');
  });

  it('names only scanned files in ALLOWED_EXCEPTIONS', () => {
    const scanned = new Set(files.map(relative));
    expect([...ALLOWED_EXCEPTIONS.keys()].filter((f) => !scanned.has(f))).toEqual([]);
  });

  it('holds no fs write in any scanned file', () => {
    const violations = files.filter((file) => !ALLOWED_EXCEPTIONS.has(relative(file))).flatMap((file) => findFsViolations(readFileSync(file, 'utf8')).map((form) => `${relative(file)}: ${form}`));
    expect(violations).toEqual([]);
  });
});
