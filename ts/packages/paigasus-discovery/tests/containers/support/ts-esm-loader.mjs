// SPDX-License-Identifier: Apache-2.0
//
// single-flight-multiprocess.test.ts spawns tests/containers/support/worker.ts as a standalone
// `node` process — not through vitest/Vite. Node's built-in TypeScript support (unflagged since
// Node 23.6, measured here on Node 24.16) strips types but resolves module specifiers LITERALLY:
// it does not map a `.js` specifier onto a sibling `.ts` file. That mapping is exactly what this
// package's "relative imports carry a .js extension" convention relies on everywhere else,
// because vitest's Vite-based resolver already performs it for every test file. This hook
// restores the same behaviour for a plain `node` process: on a failed resolution of a `.js`
// specifier, retry once against the `.ts` sibling before giving up. Mirrors
// ts/packages/paigasus-auth/tests/fixtures/ts-esm-loader.mjs.
import { register } from 'node:module';

register(import.meta.url);

/** @type {import('node:module').ResolveHook} */
export async function resolve(specifier, context, nextResolve) {
  try {
    return await nextResolve(specifier, context);
  } catch (err) {
    const code = err && typeof err === 'object' && 'code' in err ? err.code : undefined;
    if (specifier.endsWith('.js') && code === 'ERR_MODULE_NOT_FOUND') {
      return nextResolve(`${specifier.slice(0, -3)}.ts`, context);
    }
    throw err;
  }
}
