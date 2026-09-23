// SPDX-License-Identifier: Apache-2.0
//
// A child-process probe for a wasm glue and binary pair (SMA-634 spec § 5.4). It runs under plain
// `node`, NOT under vitest: the kernel's `node` vitest project has no wasm plugin and its
// `server.deps.external` rule names `.node` files only, so an import through vitest would meet
// Vite's own wasm handling instead of the loader a console server uses (spec F11).
// tests/committed-wasm.test.ts spawns it.
//
//   node tests/wasm-probe.mjs --interfaces <dir>   the module's import and export lists, as JSON
//   node tests/wasm-probe.mjs --corpus <dir>       replay all five parity corpora through <dir>
//
// <dir> holds paigasus_wasm.js, paigasus_wasm_bg.js and paigasus_wasm_bg.wasm. The glue imports
// its binary through a RELATIVE specifier, so the pair must be co-located: a mixed pair is only
// testable by copying one file over the other, which is what the negative controls do.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// tests -> paigasus-kernel -> packages -> ts -> repo root: four `../`, the same walk tests/corpus.ts
// makes.
const VECTORS = new URL('../../../../rs/crates/libs/paigasus-kernel-parity/vectors/', import.meta.url);

function vectors(name) {
  const rows = JSON.parse(readFileSync(new URL(`${name}.json`, VECTORS), 'utf8'));
  if (!Array.isArray(rows) || rows.length === 0) {
    throw new Error(`the ${name} corpus is empty or not an array — a replay over it would assert nothing`);
  }
  return rows;
}

function dirUrl(argument) {
  if (argument === undefined) throw new Error('a directory argument is required');
  return pathToFileURL(`${resolve(argument)}/`);
}

function interfaces(dir) {
  const module = new WebAssembly.Module(readFileSync(new URL('paigasus_wasm_bg.wasm', dir)));
  return {
    imports: WebAssembly.Module.imports(module)
      .map((entry) => `${entry.module}.${entry.name}:${entry.kind}`)
      .sort(),
    exports: WebAssembly.Module.exports(module)
      .map((entry) => `${entry.name}:${entry.kind}`)
      .sort(),
  };
}

async function corpus(dir) {
  // A LinkError here is the defect this probe exists for: the glue and the binary are from
  // different builds (spec F13).
  const api = await import(new URL('paigasus_wasm.js', dir).href);
  const failures = [];
  const same = (label, actual, expected) => {
    if (actual !== expected) failures.push(`${label}: ${JSON.stringify(actual)} !== ${JSON.stringify(expected)}`);
  };

  const sum = vectors('sum');
  for (const row of sum) same(`sum(${row.a}, ${row.b})`, api.sum(row.a, row.b), row.expected);

  const uuid7 = vectors('uuid7');
  for (const row of uuid7) {
    same(`mintUuid7(${row.unix_ms}, ${row.rand_hex})`, api.mintUuid7(row.unix_ms, row.rand_hex), row.expected_uuid);
  }

  const canonical = vectors('prn_canonical');
  for (const row of canonical) {
    same(`prnErrorKind(${row.input})`, api.prnErrorKind(row.input), row.error_kind);
    if (row.error_kind === '') same(`prnCanonicalize(${row.input})`, api.prnCanonicalize(row.input), row.canonical);
  }

  const cedar = vectors('prn_cedar');
  for (const row of cedar) {
    same(`prnCedarEntityType(${row.prn})`, api.prnCedarEntityType(row.prn), row.entity_type);
    same(`prnCedarEntityId(${row.prn})`, api.prnCedarEntityId(row.prn), row.entity_id);
  }

  const fields = vectors('prn_fields');
  for (const row of fields) {
    same(`prnService(${row.prn})`, api.prnService(row.prn), row.service);
    same(`prnRegion(${row.prn})`, api.prnRegion(row.prn), row.region);
    same(`prnOrg(${row.prn})`, api.prnOrg(row.prn), row.org);
    same(`prnResourceType(${row.prn})`, api.prnResourceType(row.prn), row.resource_type);
    same(`prnResourceId(${row.prn})`, api.prnResourceId(row.prn), row.resource_id);
    same(`prnBuild(${row.prn})`, api.prnBuild(row.service, row.region, row.org, row.resource_type, row.resource_id), row.prn);
  }

  if (failures.length > 0) {
    const error = new Error(`${failures.length} corpus failures`);
    error.failures = failures;
    throw error;
  }
  return {
    checked: {
      sum: sum.length,
      uuid7: uuid7.length,
      prn_canonical: canonical.length,
      prn_cedar: cedar.length,
      prn_fields: fields.length,
    },
  };
}

const [mode, target] = process.argv.slice(2);
try {
  const dir = dirUrl(target);
  const result = mode === '--interfaces' ? interfaces(dir) : mode === '--corpus' ? await corpus(dir) : null;
  if (result === null) throw new Error(`unknown mode ${JSON.stringify(mode)} — use --interfaces or --corpus`);
  process.stdout.write(`${JSON.stringify(result)}\n`);
} catch (error) {
  process.stderr.write(`${error.name}: ${error.message}\n`);
  for (const failure of error.failures ?? []) process.stderr.write(`  ${failure}\n`);
  process.exit(1);
}
