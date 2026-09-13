// SPDX-License-Identifier: Apache-2.0
//
// single-flight-multiprocess.test.ts spawns tests/containers/support/worker.ts as a standalone
// `node` process — not through vitest/Vite. Node's built-in TypeScript support strips types but
// resolves module specifiers LITERALLY: it maps no `.js` specifier onto a `.ts` file and it probes
// no extension at all. vitest's Vite-based resolver does both. This hook restores them for a plain
// `node` process, in two retries after a failed resolution:
//
//   1. `./x.js` → `./x.ts` — the test files, which keep `.js` specifiers.
//   2. `./x` → `./x.ts`, then `./x/index.ts` — the package sources, which are EXTENSIONLESS since
//      SMA-511 because Turbopack (Next 16.3.4) does not resolve `./x.js` to `./x.ts` (spec § 7.2).
//
// A specifier that still names no file fails with its ORIGINAL error. tests/ts-esm-loader.test.ts
// proves all three outcomes. This file is a copy of
// ts/packages/paigasus-auth/tests/fixtures/ts-esm-loader.mjs; change both together.
import { register } from 'node:module';

register(import.meta.url);

const NOT_FOUND = new Set(['ERR_MODULE_NOT_FOUND', 'ERR_UNSUPPORTED_DIR_IMPORT']);
const HAS_MODULE_EXTENSION = /\.(?:[cm]?[jt]sx?|json)$/;

/** @param {unknown} err */
function codeOf(err) {
  return err && typeof err === 'object' && 'code' in err ? err.code : undefined;
}

/** @type {import('node:module').ResolveHook} */
export async function resolve(specifier, context, nextResolve) {
  try {
    return await nextResolve(specifier, context);
  } catch (err) {
    const code = codeOf(err);
    if (specifier.endsWith('.js') && code === 'ERR_MODULE_NOT_FOUND') {
      return nextResolve(`${specifier.slice(0, -3)}.ts`, context);
    }
    if (/^\.\.?\//.test(specifier) && !HAS_MODULE_EXTENSION.test(specifier) && NOT_FOUND.has(code)) {
      for (const candidate of [`${specifier}.ts`, `${specifier}/index.ts`]) {
        try {
          return await nextResolve(candidate, context);
        } catch (retryErr) {
          if (!NOT_FOUND.has(codeOf(retryErr))) throw retryErr;
        }
      }
    }
    throw err;
  }
}
