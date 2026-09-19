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
// `createRequire`, `process.getBuiltinModule(...)` and a dynamic `import(...)`. `require(...)`,
// `import(...)` and `getBuiltinModule(...)` each accept a plain (no-interpolation) backtick
// specifier too, not only `'`/`"`.
//
// Two further checks below hold what the import scan alone cannot see: the `test-e2e` task's
// `script:` block in moon.yml, checked against an ALLOWLIST of the two lines the script may hold
// (a copy or delete brought the shared-tree write back once with no fs import at all, and a
// denylist of command words separately missed `sed -i`, `truncate`, `dd` and a `node -e` fs
// call), and playwright.config.ts's `globalSetup`/`globalTeardown` paths (a setup file outside
// tests/e2e/ would not be scanned by the import check above).
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
import config from '../../playwright.config';

const APP_DIR = fileURLToPath(new URL('../..', import.meta.url));
const E2E_DIR = path.join(APP_DIR, 'tests', 'e2e');
const FS_MODULE = /^(?:node:)?fs(?:\/promises)?$/;
const READ_SET: ReadonlySet<string> = new Set(['existsSync', 'readFileSync', 'readdirSync', 'statSync', 'lstatSync']);
const SCRIPT_FILE = /\.(?:[cm]?[jt]s|tsx)$/;
/** File path (relative to the app, `/` separators) → reason. Ships EMPTY; an entry needs a reason. */
const ALLOWED_EXCEPTIONS: ReadonlyMap<string, string> = new Map();
/** SMA-655: the only line text the test-e2e script may hold, after trimming each line. */
const ALLOWED_SCRIPT_LINES: ReadonlySet<string> = new Set(['set -euo pipefail', 'pnpm exec playwright test']);

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
    // SMA-655: require/import()/getBuiltinModule() are function calls, so a no-interpolation
    // backtick specifier is valid JS there too — unlike the static `import … from` / `export …
    // from` forms above, which only ever accept a quoted string literal.
    [/\brequire\s*\(\s*(['"`])([^'"`]+)\1/g, (s) => `require('${s}')`],
    [/\bimport\s*\(\s*(['"`])([^'"`]+)\1/g, (s) => `import('${s}')`],
    [/\bgetBuiltinModule\s*\(\s*(['"`])([^'"`]+)\1/g, (s) => `getBuiltinModule('${s}')`],
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

/** Leading-whitespace length of `line`, or 0 for an empty line. */
function indentOf(line: string): number {
  return /^(\s*)/.exec(line)?.[1]?.length ?? 0;
}

/**
 * Extracts the `test-e2e:` task's `script: |` block body from a moon.yml file's TEXT, by
 * indentation alone — no YAML parser and no new dependency. Throws if the task or its script
 * block cannot be found, so a renamed task cannot pass the mutation check below vacuously.
 */
function extractTestE2eScript(moonYmlText: string): string {
  const lines = moonYmlText.split('\n');
  const taskLine = lines.findIndex((l) => /^\s*test-e2e:\s*$/.test(l));
  if (taskLine === -1) throw new Error("moon.yml: no 'test-e2e:' task found");
  const taskIndent = indentOf(lines[taskLine] ?? '');
  let taskEnd = lines.length;
  for (let i = taskLine + 1; i < lines.length; i += 1) {
    const line = lines[i] ?? '';
    if (line.trim() !== '' && indentOf(line) <= taskIndent) {
      taskEnd = i;
      break;
    }
  }
  let scriptLine = -1;
  let scriptIndent = 0;
  for (let i = taskLine + 1; i < taskEnd; i += 1) {
    const line = lines[i] ?? '';
    if (/^\s*script:\s*\|\s*$/.test(line)) {
      scriptLine = i;
      scriptIndent = indentOf(line);
      break;
    }
  }
  if (scriptLine === -1) throw new Error("moon.yml: 'test-e2e:' task has no 'script: |' block");
  const bodyLines: string[] = [];
  for (let i = scriptLine + 1; i < taskEnd; i += 1) {
    const line = lines[i] ?? '';
    if (line.trim() === '') {
      bodyLines.push('');
      continue;
    }
    if (indentOf(line) <= scriptIndent) break;
    bodyLines.push(line);
  }
  const indents = bodyLines.filter((l) => l.trim() !== '').map(indentOf);
  const dedent = indents.length === 0 ? 0 : Math.min(...indents);
  const script = bodyLines
    .map((l) => (l.trim() === '' ? '' : l.slice(dedent)))
    .join('\n')
    .trim();
  if (script === '') throw new Error("moon.yml: 'test-e2e:' task's script block is empty");
  return script;
}

/** Every non-empty, trimmed line of `script` that is not exactly one of ALLOWED_SCRIPT_LINES. */
function findDisallowedScriptLines(script: string): string[] {
  const found: string[] = [];
  for (const rawLine of script.split('\n')) {
    const line = rawLine.trim();
    if (line === '') continue;
    if (!ALLOWED_SCRIPT_LINES.has(line)) found.push(line);
  }
  return found;
}

/** Whether `script` holds the required Playwright invocation, so an emptied script cannot pass. */
function scriptRunsPlaywright(script: string): boolean {
  return script.split('\n').some((rawLine) => rawLine.trim() === 'pnpm exec playwright test');
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
    ['process.getBuiltinModule', "const fs = process.getBuiltinModule('node:fs');"],
    ['a dynamic import with a backtick specifier', 'await import(`node:fs`);'],
    ['require with a backtick specifier', 'const fs = require(`node:fs`);'],
    ['getBuiltinModule with a backtick specifier', 'const fs = process.getBuiltinModule(`node:fs`);'],
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

  it('gives every ALLOWED_EXCEPTIONS entry a non-empty reason', () => {
    const blank = [...ALLOWED_EXCEPTIONS.entries()].filter(([, reason]) => reason.trim() === '');
    expect(blank).toEqual([]);
  });

  it('holds no fs write in any scanned file', () => {
    const violations = files.filter((file) => !ALLOWED_EXCEPTIONS.has(relative(file))).flatMap((file) => findFsViolations(readFileSync(file, 'utf8')).map((form) => `${relative(file)}: ${form}`));
    expect(violations).toEqual([]);
  });
});

// SMA-655: a copy or delete added directly to the `test-e2e` task's `script:` in moon.yml brings
// the shared-tree write back with no fs import at all, so the scan above cannot see it. This is
// an ALLOWLIST, not a denylist of mutating command words: a denylist missed `sed -i`, `truncate`,
// `dd` and a `node -e` fs call, so every non-empty trimmed line must be exactly one of
// ALLOWED_SCRIPT_LINES.
describe('extractTestE2eScript / findDisallowedScriptLines', () => {
  /** A minimal moon.yml holding a `test-e2e:` task whose script body is `script`. */
  function fixtureMoonYml(script: string): string {
    const body = script
      .split('\n')
      .map((l) => (l === '' ? '' : `      ${l}`))
      .join('\n');
    return `id: 'fixture'\ntasks:\n  test-e2e:\n    script: |\n${body}\n    deps: []\n  other:\n    script: |\n      echo unrelated\n`;
  }

  const redScripts: ReadonlyArray<readonly [string, string]> = [
    ['a copy into the standalone tree', 'set -euo pipefail\ncp -R .next/static x\npnpm exec playwright test'],
    ['a delete', 'set -euo pipefail\nrm -rf x\npnpm exec playwright test'],
    ['a redirection', 'set -euo pipefail\necho hi > x\npnpm exec playwright test'],
    ['a stream edit', "set -euo pipefail\nsed -i 's/a/b/' .next/x\npnpm exec playwright test"],
    ['a truncate', 'set -euo pipefail\ntruncate -s 0 .next/x\npnpm exec playwright test'],
    ['a raw disk write', 'set -euo pipefail\ndd if=/dev/zero of=.next/x\npnpm exec playwright test'],
    ['a node fs call', "set -euo pipefail\nnode -e \"require('fs').rmSync('.next',{recursive:true})\"\npnpm exec playwright test"],
  ];
  it.each(redScripts)('reds on %s', (_label, script) => {
    const extracted = extractTestE2eScript(fixtureMoonYml(script));
    expect(findDisallowedScriptLines(extracted)).not.toEqual([]);
  });

  it('accepts the real playwright invocation', () => {
    const extracted = extractTestE2eScript(fixtureMoonYml('set -euo pipefail\npnpm exec playwright test'));
    expect(findDisallowedScriptLines(extracted)).toEqual([]);
    expect(scriptRunsPlaywright(extracted)).toBe(true);
  });

  it('passes the allowlist but fails the presence check on an emptied script', () => {
    const extracted = extractTestE2eScript(fixtureMoonYml('set -euo pipefail'));
    expect(findDisallowedScriptLines(extracted)).toEqual([]);
    expect(scriptRunsPlaywright(extracted)).toBe(false);
  });

  it('throws when the test-e2e task cannot be found ("block not found")', () => {
    expect(() => extractTestE2eScript("id: 'fixture'\ntasks:\n  build:\n    script: |\n      echo hi\n")).toThrow();
  });

  it('throws when the task has no script block ("block not found")', () => {
    expect(() => extractTestE2eScript("id: 'fixture'\ntasks:\n  test-e2e:\n    deps: []\n")).toThrow();
  });

  describe("this app's real test-e2e script (SMA-655)", () => {
    const moonYmlText = readFileSync(path.join(APP_DIR, 'moon.yml'), 'utf8');

    it('is found and non-empty', () => {
      const script = extractTestE2eScript(moonYmlText);
      expect(script.trim().length).toBeGreaterThan(0);
    });

    it('holds only the allowed script lines', () => {
      const script = extractTestE2eScript(moonYmlText);
      expect(findDisallowedScriptLines(script)).toEqual([]);
    });

    it('runs the playwright suite', () => {
      const script = extractTestE2eScript(moonYmlText);
      expect(scriptRunsPlaywright(script)).toBe(true);
    });
  });
});

// SMA-655: a globalSetup/globalTeardown pointed outside tests/e2e/ would not be scanned above.
describe('globalSetup and globalTeardown stay under tests/e2e/ (SMA-655)', () => {
  function configuredPaths(): string[] {
    const values: ReadonlyArray<string | readonly string[] | undefined> = [config.globalSetup, config.globalTeardown];
    const out: string[] = [];
    for (const value of values) {
      if (value === undefined) continue;
      if (typeof value === 'string') out.push(value);
      else out.push(...value);
    }
    return out;
  }

  it('configures at least one of globalSetup/globalTeardown', () => {
    expect(configuredPaths().length).toBeGreaterThan(0);
  });

  it('resolves every configured path under tests/e2e/', () => {
    for (const candidate of configuredPaths()) {
      const resolved = path.resolve(APP_DIR, candidate);
      const rel = path.relative(E2E_DIR, resolved);
      const isUnderE2eDir = rel !== '' && !rel.startsWith('..') && !path.isAbsolute(rel);
      expect(isUnderE2eDir).toBe(true);
    }
  });
});
