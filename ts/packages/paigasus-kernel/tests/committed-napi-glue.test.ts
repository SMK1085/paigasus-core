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
//   2. each pair is not vacuous: the export names in index.js equal those in index.d.ts, the set
//      holds `sum`, and it has more than one member;
//   3. the `build` and `test` tasks in moon.yml pass the scratch flags, so neither task writes the
//      committed files.
//
// The checks read files only. They do not load index.fresh.js.
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const CRATE = new URL('../../../../rs/crates/bindings/paigasus-node-bindings/', import.meta.url);
const MOON_YML = new URL('../moon.yml', import.meta.url);
const SCRATCH_FLAGS = '--js index.fresh.js --dts index.fresh.d.ts';
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

// `export declare function|const|class <name>` lines.
export function dtsExportNames(source: string): string[] {
  return captures(source, /^export declare (?:function|const|class) ([A-Za-z_$][\w$]*)/gm);
}

// Returns a list of problems. An empty list means the pair is not vacuous.
export function vacuityProblems(js: string, dts: string): string[] {
  const jsNames = jsExportNames(js);
  const dtsNames = dtsExportNames(dts);
  const problems: string[] = [];
  if (JSON.stringify(jsNames) !== JSON.stringify(dtsNames)) {
    problems.push(`index.js exports [${jsNames.join(', ')}] but index.d.ts declares [${dtsNames.join(', ')}]`);
  }
  if (!dtsNames.includes('sum')) problems.push('index.d.ts declares no `sum`');
  if (dtsNames.length < 2) problems.push(`index.d.ts declares only ${dtsNames.length} name(s)`);
  return problems;
}

// The `script:` line of one task in moon.yml, found by text: the first `    script:` line after the
// `  <task>:` line.
export function taskScript(moonYml: string, task: string): string | undefined {
  const lines = moonYml.split('\n');
  const start = lines.indexOf(`  ${task}:`);
  if (start < 0) return undefined;
  for (const line of lines.slice(start + 1)) {
    if (/^ {2}\S/.test(line)) return undefined; // the next task started first
    if (line.startsWith('    script:')) return line;
  }
  return undefined;
}

const read = (name: string): string => readFileSync(new URL(name, CRATE), 'utf8');

describe('the gate helpers can fail', () => {
  // Without these rows, a helper that always returns "no problem" would make checks 2 and 3 vacuous.
  it('flags a d.ts that declares only __napiBindingTarget', () => {
    const js = 'module.exports.__napiBindingTarget = x\n';
    const dts = "export declare const __napiBindingTarget: 'native'\n";
    expect(vacuityProblems(js, dts)).not.toEqual([]);
  });

  it('flags a js and d.ts whose names differ', () => {
    const js = 'module.exports.sum = b.sum\nmodule.exports.prnOrg = b.prnOrg\n';
    const dts = 'export declare function sum(a: number, b: number): number\nexport declare function prnRegion(s: string): string\n';
    expect(vacuityProblems(js, dts)).not.toEqual([]);
  });

  it('accepts a matching, non-trivial pair', () => {
    const js = 'module.exports = b\nmodule.exports.sum = b.sum\nmodule.exports.prnOrg = b.prnOrg\n';
    const dts = 'export declare function sum(a: number, b: number): number\nexport declare function prnOrg(s: string): string\n';
    expect(vacuityProblems(js, dts)).toEqual([]);
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

  it.each(['build', 'test'])('check 3: the %s task writes only the scratch files', (task) => {
    const script = taskScript(readFileSync(MOON_YML, 'utf8'), task);
    expect(script, `moon.yml has no \`script:\` line for the ${task} task`).toBeDefined();
    expect(script, `the ${task} task must pass \`${SCRATCH_FLAGS}\` to napi build, or it overwrites the committed glue (SMA-667).`).toContain(SCRATCH_FLAGS);
  });
});
