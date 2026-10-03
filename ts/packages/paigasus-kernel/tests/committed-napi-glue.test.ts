// SPDX-License-Identifier: Apache-2.0
//
// The drift gate for the two committed napi glue files (SMA-667). `paigasus-kernel-ts:test` builds
// the glue into sibling scratch files (`index.fresh.js`, `index.fresh.d.ts`). This file compares the
// committed files with them. The committed files are what npm publishes (release.yml), what the
// prebuild assembly packs (prebuild.yml) and what ts/Dockerfile copies. No CI step rebuilds them
// before those readers.
//
//   0. the fresh files exist;
//   1. each committed file equals its fresh file, byte for byte;
//   2. each pair is not vacuous: every export name in index.js is declared in index.d.ts, the set
//      of index.js names holds `sum`, and it has more than one member. index.d.ts may declare more
//      names, such as an `export interface`;
//   3. every `napi build` in the `build`, `test` and `generate-napi-glue` tasks in moon.yml passes
//      the scratch flags. The `build` and `test` tasks also do not name the committed files, so
//      neither task writes them.
//
// The checks read files only. They do not load index.fresh.js.
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const CRATE = new URL('../../../../rs/crates/bindings/paigasus-node-bindings/', import.meta.url);
const MOON_YML = new URL('../moon.yml', import.meta.url);
const SCRATCH_FLAGS = '--js index.fresh.js --dts index.fresh.d.ts';
const NO_DTS_CACHE = '--no-dts-cache';
const COMMITTED_PATHS = ['paigasus-node-bindings/index.js', 'paigasus-node-bindings/index.d.ts'];
const PAIRS = [
  ['index.js', 'index.fresh.js'],
  ['index.d.ts', 'index.fresh.d.ts'],
] as const;

const REGENERATE = 'Run `moon run paigasus-kernel-ts:generate-napi-glue` and commit index.js and index.d.ts under rs/crates/bindings/paigasus-node-bindings/.';

// The first capture group of every match, sorted. `flatMap` drops a match without a group, which
// keeps the result `string[]` under `noUncheckedIndexedAccess`.
function captures(source: string, pattern: RegExp): string[] {
  return [...source.matchAll(pattern)].flatMap((m) => (m[1] === undefined ? [] : [m[1]])).sort();
}

// `module.exports.<name> =` lines. The bare `module.exports = nativeBinding` line has no name and
// does not match.
export function jsExportNames(source: string): string[] {
  return captures(source, /^module\.exports\.([A-Za-z_$][\w$]*)\s*=/gm);
}

// `export [declare] function|const|class|enum|const enum|namespace|type|interface <name>` lines.
// napi writes a `#[napi]` enum as `const enum`, `enum` or `type`, a `js_name` alias as `type`, and a
// module as `namespace`. `const enum` stays before `const`, so the capture is the name.
export function dtsExportNames(source: string): string[] {
  return captures(source, /^export (?:declare )?(?:const enum|enum|function|const|class|namespace|type|interface) ([A-Za-z_$][\w$]*)/gm);
}

// Returns a list of problems. An empty list means the pair is not vacuous. The d.ts may declare
// more names than index.js exports, such as an `export interface`, so the check is a subset.
export function vacuityProblems(js: string, dts: string): string[] {
  const jsNames = jsExportNames(js);
  const dtsNames = dtsExportNames(dts);
  const problems: string[] = [];
  const undeclared = jsNames.filter((n) => !dtsNames.includes(n));
  if (undeclared.length > 0) {
    problems.push(`index.js exports [${undeclared.join(', ')}] but index.d.ts does not declare them`);
  }
  if (!jsNames.includes('sum')) problems.push('index.js exports no `sum`');
  if (jsNames.length < 2) problems.push(`index.js exports only ${jsNames.length} name(s)`);
  return problems;
}

// The `script:` line of one task in moon.yml, found by text: the first `    script:` line after the
// `  <task>:` line.
export function taskScript(moonYml: string, task: string): string | undefined {
  const lines = moonYml.split(/\r?\n/);
  const start = lines.indexOf(`  ${task}:`);
  if (start < 0) return undefined;
  for (const line of lines.slice(start + 1)) {
    if (/^ {2}\S/.test(line)) return undefined; // the next task started first
    if (line.startsWith('    script:')) return line;
  }
  return undefined;
}

// Returns a list of problems with one task script. Every `napi build` segment (split on `&&`) must
// carry the scratch flags and `--no-dts-cache`. With `forbidCommitted`, the script must not name the
// committed glue files.
export function scriptProblems(task: string, script: string, forbidCommitted: boolean): string[] {
  const problems: string[] = [];
  const builds = script.split('&&').filter((segment) => segment.includes('napi build'));
  if (builds.length === 0) problems.push(`the ${task} task has no \`napi build\` call`);
  for (const segment of builds) {
    if (!segment.includes(SCRATCH_FLAGS)) {
      problems.push(`every \`napi build\` in the ${task} task must pass \`${SCRATCH_FLAGS}\`, or it overwrites the committed glue (SMA-667).`);
    }
    if (!segment.includes(NO_DTS_CACHE)) {
      problems.push(`every \`napi build\` in the ${task} task must pass \`${NO_DTS_CACHE}\`, or concurrent tasks share one type-def folder (SMA-667).`);
    }
  }
  if (forbidCommitted) {
    for (const path of COMMITTED_PATHS) {
      if (script.includes(path)) problems.push(`the ${task} task must not name the committed file ${path}. Only generate-napi-glue writes it.`);
    }
  }
  return problems;
}

const read = (name: string): string => readFileSync(new URL(name, CRATE), 'utf8');

describe('the gate helpers can fail', () => {
  // Without these rows, a helper that always returns "no problem" would make checks 2 and 3 vacuous.
  it('flags a d.ts that declares only __napiBindingTarget', () => {
    const js = 'module.exports.__napiBindingTarget = x\n';
    const dts = "export declare const __napiBindingTarget: 'native'\n";
    expect(vacuityProblems(js, dts)).not.toEqual([]);
  });

  it('flags js names that the d.ts does not declare', () => {
    const js = 'module.exports.sum = b.sum\nmodule.exports.prnOrg = b.prnOrg\n';
    const dts = 'export declare function sum(a: number, b: number): number\nexport declare function prnRegion(s: string): string\n';
    expect(vacuityProblems(js, dts)).not.toEqual([]);
  });

  it('accepts a matching, non-trivial pair', () => {
    const js = 'module.exports = b\nmodule.exports.sum = b.sum\nmodule.exports.prnOrg = b.prnOrg\n';
    const dts = 'export declare function sum(a: number, b: number): number\nexport declare function prnOrg(s: string): string\n';
    expect(vacuityProblems(js, dts)).toEqual([]);
  });

  it('accepts a d.ts that declares a type-only name which index.js does not export', () => {
    const js = 'module.exports.sum = b.sum\nmodule.exports.prnOrg = b.prnOrg\n';
    const dts = 'export interface Obj {\n  a: number\n}\nexport declare function sum(a: number, b: number): number\nexport declare function prnOrg(s: string): string\n';
    expect(vacuityProblems(js, dts)).toEqual([]);
  });

  it('flags an index.js without `sum` and an index.js with one name', () => {
    expect(vacuityProblems('module.exports.a = b.a\nmodule.exports.b = b.b\n', 'export declare function a(): void\nexport declare function b(): void\n')).not.toEqual([]);
    expect(vacuityProblems('module.exports.sum = b.sum\n', 'export declare function sum(): void\n')).not.toEqual([]);
  });

  it('captures the forms napi writes for runtime exports', () => {
    expect(dtsExportNames('export type JsPrn = Prn\n')).toEqual(['JsPrn']);
    expect(dtsExportNames('export declare namespace ns {\n  export declare function f(): void\n}\n')).toEqual(['ns']);
    expect(dtsExportNames('export declare const enum Color {\n  Red = 0,\n}\n')).toEqual(['Color']);
    expect(dtsExportNames('export type Mode = "a" | "b"\n')).toEqual(['Mode']);
  });

  it('accepts a napi enum declaration', () => {
    const js = 'module.exports.sum = b.sum\nmodule.exports.Color = b.Color\n';
    const dts = 'export declare function sum(a: number, b: number): number\nexport declare const enum Color {\n  Red = 0,\n}\n';
    expect(vacuityProblems(js, dts)).toEqual([]);
    expect(dtsExportNames('export declare enum Mode {\n  A = 0,\n}\n')).toEqual(['Mode']);
  });

  it('finds a task script in a file with CRLF line ends', () => {
    const yml = "tasks:\r\n  build:\r\n    script: 'a'\r\n  test:\r\n    script: 'b'\r\n";
    expect(taskScript(yml, 'build')).toBe("    script: 'a'");
    expect(taskScript(yml, 'test')).toBe("    script: 'b'");
  });

  it('flags a script with a napi build that lacks the scratch flags', () => {
    const good = `pnpm exec napi build --platform ${SCRATCH_FLAGS} ${NO_DTS_CACHE} && tsc`;
    expect(scriptProblems('t', good, true)).toEqual([]);
    expect(scriptProblems('t', `napi build ${SCRATCH_FLAGS} ${NO_DTS_CACHE} && napi build ${NO_DTS_CACHE} && tsc`, true)).not.toEqual([]);
    expect(scriptProblems('t', `napi build ${SCRATCH_FLAGS} && tsc`, true)).not.toEqual([]);
    expect(scriptProblems('t', 'tsc', true)).not.toEqual([]);
  });

  it('flags a script that names the committed glue, unless that is allowed', () => {
    const bad = `napi build ${SCRATCH_FLAGS} ${NO_DTS_CACHE} && cp a ../x/paigasus-node-bindings/index.js`;
    expect(scriptProblems('t', bad, true)).not.toEqual([]);
    expect(scriptProblems('t', `napi build ${SCRATCH_FLAGS} ${NO_DTS_CACHE} && cp a ../x/paigasus-node-bindings/index.d.ts`, true)).not.toEqual([]);
    expect(scriptProblems('t', bad, false)).toEqual([]);
  });

  it('finds a task script and misses an absent task', () => {
    const yml = "tasks:\n  build:\n    script: 'a --js index.fresh.js --dts index.fresh.d.ts'\n  test:\n    deps: []\n  other:\n    script: 'b'\n";
    expect(taskScript(yml, 'build')).toContain(SCRATCH_FLAGS);
    expect(taskScript(yml, 'test')).toBeUndefined();
    expect(taskScript(yml, 'absent')).toBeUndefined();
  });
});

describe('the committed napi glue agrees with the generator', () => {
  it.each(PAIRS)('check 0: the fresh build of %s exists', (_committed, fresh) => {
    expect(existsSync(new URL(fresh, CRATE)), `${fileURLToPath(new URL(fresh, CRATE))} is missing — run \`moon run paigasus-kernel-ts:test\`, which builds it.`).toBe(true);
  });

  it.each(PAIRS)('check 1: the committed %s equals the fresh build', (committed, fresh) => {
    expect(read(committed), `the committed ${committed} is not the generator's output. ${REGENERATE}`).toBe(read(fresh));
  });

  it.each([
    ['committed', 'index.js', 'index.d.ts'],
    ['fresh', 'index.fresh.js', 'index.fresh.d.ts'],
  ] as const)('check 2: the %s pair is not vacuous', (label, js, dts) => {
    expect(vacuityProblems(read(js), read(dts)), `the ${label} glue is vacuous or inconsistent. ${REGENERATE}`).toEqual([]);
  });

  it.each([
    ['build', true],
    ['test', true],
    ['generate-napi-glue', false],
  ] as const)('check 3: the %s task builds only into the scratch files', (task, forbidCommitted) => {
    const script = taskScript(readFileSync(MOON_YML, 'utf8'), task);
    expect(script, `moon.yml has no \`script:\` line for the ${task} task`).toBeDefined();
    expect(scriptProblems(task, script ?? '', forbidCommitted)).toEqual([]);
  });
});
