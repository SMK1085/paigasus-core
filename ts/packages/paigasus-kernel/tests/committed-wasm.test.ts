// SPDX-License-Identifier: Apache-2.0
//
// The drift gate for the five committed wasm artifacts (SMA-634 spec § 5.4). Three checks, all
// host-independent, so they hold in CI although the binary bytes differ per host (spec F12):
//
//   1. the committed glue equals the glue of the fresh build this task already makes;
//   2. the committed binary has the same import and export lists as the fresh one, and that list is
//      the REAL kernel interface, not an empty pair of lists;
//   3. the committed glue and binary instantiate together and replay all five parity corpora.
//
// Check 4 (the pnpm-installed copy) is NOT here: Moon's hasher ignores node_modules, so a cached
// pass would replay while the installed copy is another branch's. It lives in the setupFiles of the
// console-core and app vitest configs.
//
// Checks 2 and 3 run in a CHILD node process (tests/wasm-probe.mjs). This project has no wasm
// plugin, so an import through vitest would not be the loader a console server uses (spec F11).
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const CRATE = new URL('../../../../rs/crates/bindings/paigasus-wasm/', import.meta.url);
const FRESH = new URL('.wasmpack-test-out/', CRATE);
const PROBE = fileURLToPath(new URL('./wasm-probe.mjs', import.meta.url));
const GLUE = ['paigasus_wasm.js', 'paigasus_wasm_bg.js', 'paigasus_wasm.d.ts', 'paigasus_wasm_bg.wasm.d.ts'];

// The measured interface of the wasm binary. It is asserted LITERALLY because
// `wasm-probe.mjs --interfaces` has no non-empty check of its own: a trivial module prints two empty
// lists and exits 0, and an equality between two empty lists is a vacuous pass. These literals are
// what makes check 2 bite.
//
// Twelve of the exports are the kernel's own API; the rest are wasm-bindgen's runtime surface. The
// two imports are the glue functions the binary calls back into.
const EXPECTED_EXPORTS = [
  '__abort_handler:global',
  '__externref_table_dealloc:function',
  '__instance_terminated:global',
  '__wbindgen_externrefs:table',
  '__wbindgen_free:function',
  '__wbindgen_malloc:function',
  '__wbindgen_realloc:function',
  '__wbindgen_start:function',
  'memory:memory',
  'mintUuid7:function',
  'prnBuild:function',
  'prnCanonicalize:function',
  'prnCedarEntityId:function',
  'prnCedarEntityType:function',
  'prnErrorKind:function',
  'prnOrg:function',
  'prnRegion:function',
  'prnResourceId:function',
  'prnResourceType:function',
  'prnService:function',
  'sum:function',
];

// The import names carry ONE field that is not a name: wasm-bindgen suffixes its `Error` shim with a
// 16-hex content hash (`__wbg_Error_408e67f47ca7b58b` at the time of writing). A literal there would
// red this gate on a wasm-bindgen bump that changed the hash and nothing else — a failure that
// `generate-wasm` cannot repair, so the gate would lie. Every other segment is literal, the count is
// literal, and the hash itself is held to its shape.
const EXPECTED_IMPORTS = [/^\.\/paigasus_wasm_bg\.js\.__wbg_Error_[0-9a-f]{16}:function$/, /^\.\/paigasus_wasm_bg\.js\.__wbindgen_init_externref_table:function$/];

const REGENERATE = 'Run `moon run paigasus-kernel-ts:generate-wasm` and commit all five files under rs/crates/bindings/paigasus-wasm/ (paigasus_wasm_bg.wasm and the four glue files).';

interface Interfaces {
  imports: string[];
  exports: string[];
}

function probe(mode: string, dir: URL): unknown {
  // A failure must name the probe's own stderr: a LinkError or a corpus row is the finding.
  try {
    return JSON.parse(execFileSync(process.execPath, [PROBE, mode, fileURLToPath(dir)], { encoding: 'utf8' }));
  } catch (error) {
    const detail = (error as { stderr?: string }).stderr ?? String(error);
    throw new Error(`wasm-probe.mjs ${mode} failed for ${fileURLToPath(dir)}:\n${detail}\n${REGENERATE}`, { cause: error });
  }
}

describe('the committed wasm artifacts agree with the Rust source', () => {
  it('has the fresh build this task makes', () => {
    // Without this the three checks below would compare against an absent directory and report a
    // confusing failure. The `test` task's own script builds it.
    expect(existsSync(fileURLToPath(new URL('paigasus_wasm_bg.wasm', FRESH))), `${fileURLToPath(FRESH)} is missing — run \`moon run paigasus-kernel-ts:test\`, which builds it.`).toBe(true);
  });

  it.each(GLUE)('check 1: the committed %s equals the fresh build', (name) => {
    const committed = readFileSync(new URL(name, CRATE));
    const fresh = readFileSync(new URL(name, FRESH));
    // A byte comparison, reported as text: the glue is JavaScript and TypeScript, so a diff is
    // readable, and the four files are byte-identical on every host (spec F15).
    expect(committed.toString('utf8'), `the committed ${name} is not this source's. ${REGENERATE}`).toBe(fresh.toString('utf8'));
  });

  it('check 2: the committed binary has the fresh interface', () => {
    expect(probe('--interfaces', CRATE), `the committed binary's imports or exports differ from the fresh build's. ${REGENERATE}`).toEqual(probe('--interfaces', FRESH));
  });

  it('check 2: the interface both binaries carry is the real kernel one', () => {
    // The guard on the check above. Two empty lists are equal to each other, so the equality alone
    // would pass for a module that exports nothing. Assert BOTH binaries against the measured
    // interface, so neither an empty committed binary nor an empty fresh one can satisfy check 2.
    for (const [label, dir] of [
      ['committed', CRATE],
      ['fresh', FRESH],
    ] as [string, URL][]) {
      const actual = probe('--interfaces', dir) as Interfaces;
      expect(actual.exports, `the ${label} binary does not export the kernel's 21 names. ${REGENERATE}`).toEqual(EXPECTED_EXPORTS);
      expect(actual.imports, `the ${label} binary does not import the glue's 2 callbacks. ${REGENERATE}`).toHaveLength(EXPECTED_IMPORTS.length);
      EXPECTED_IMPORTS.forEach((pattern, index) => {
        expect(actual.imports[index], `the ${label} binary's import ${index} does not match ${String(pattern)}. ${REGENERATE}`).toMatch(pattern);
      });
    }
  });

  it('check 3: the committed pair instantiates and replays every corpus', () => {
    const result = probe('--corpus', CRATE) as { checked: Record<string, number> };
    // The counts guard against a vacuous pass: an empty corpus file would otherwise replay nothing.
    // wasm-probe.mjs also rejects an empty corpus, so this is the second control on the same thing.
    expect(Object.keys(result.checked).sort()).toEqual(['prn_canonical', 'prn_cedar', 'prn_fields', 'sum', 'uuid7']);
    for (const [name, count] of Object.entries(result.checked)) expect(count, `the ${name} corpus is empty`).toBeGreaterThan(0);
  });
});
