// SPDX-License-Identifier: Apache-2.0
//
// M8 (task 13 brief). The brief's original plan was to start the fixture server with
// `NODE_OPTIONS=--conditions=react-server`, so that `import 'server-only'` in src/server.ts (whose
// exports map is `{ "react-server": "./empty.js", "default": "./index.js" }`, and `index.js` is an
// unconditional throw) resolves to its no-op branch.
//
// MEASURED: that plan does not survive task 10. vitest.config.ts's own header explains why in
// full — `react-server` is ALSO the condition `react`'s exports map switches on, and
// `next/navigation` (which src/server.ts re-exports through src/next/get-session.ts) calls
// `React.createContext` at MODULE SCOPE in its react-server build, which has no `createContext`.
// Running this fixture server with `--conditions=react-server` reproduces that exact crash (see
// task-13-report.md's M8 entry for the captured error) — it is not a vitest-only trap.
//
// task 10's actual fix was a permanent ALIAS, not a condition flip: vitest.config.ts resolves
// `server-only` to tests/support/server-only-stub.ts unconditionally, leaving the condition list
// untouched (`['node', 'import', 'default']`, i.e. Node's ordinary defaults with no `react-server`
// member) so `next/navigation` and `next/headers` resolve completely normally. A bare `node`
// process has no `resolve.alias`, so this file is the same fix implemented as a Node module
// customization hook (the same mechanism tests/fixtures/ts-esm-loader.mjs already uses) instead:
// intercept the bare specifier `server-only` and redirect it to that same stub, before the
// `.js`-to-`.ts` fallback hook (or the default resolver) ever sees it.
//
// SECOND, UNRELATED PROBLEM MEASURED WHILE PROVING THE ABOVE. Removing `--conditions=react-server`
// was not enough on its own: a bare `node` process could not resolve `import { cookies } from
// 'next/headers'` AT ALL, condition list aside. `next`'s package.json has no `exports` map, so
// Node's ESM resolver falls back to legacy extensionless resolution for a bare deep import — and,
// unlike Vite's resolver (which is what makes this work under vitest), plain Node does NOT probe
// `.js`/`.mjs`/`index.js` for a bare specifier with no extension. The exact error
// (`Cannot find module '.../node_modules/next/headers' ... Did you mean to import
// "next/headers.js"?`) reproduces identically with or without `--conditions=react-server` — it is
// not a `server-only`/react-server issue at all, and `next/headers.js`/`next/navigation.js` do
// resolve directly once the extension is supplied. The retry below is the general form of exactly
// the `.js`-to-`.ts` fallback tests/fixtures/ts-esm-loader.mjs already performs for this package's
// OWN relative imports, applied here to `next`'s bare deep imports instead.
import { register } from 'node:module';
import { URL } from 'node:url';

register(import.meta.url);

const stubUrl = new URL('../support/server-only-stub.ts', import.meta.url).href;

/** @type {import('node:module').ResolveHook} */
export async function resolve(specifier, context, nextResolve) {
  if (specifier === 'server-only') {
    return nextResolve(stubUrl, context);
  }
  try {
    return await nextResolve(specifier, context);
  } catch (err) {
    const code = err && typeof err === 'object' && 'code' in err ? err.code : undefined;
    if (code === 'ERR_MODULE_NOT_FOUND' && !specifier.endsWith('.js') && !specifier.startsWith('.') && !specifier.startsWith('/')) {
      return nextResolve(`${specifier}.js`, context);
    }
    throw err;
  }
}
