// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// `lib/*` opens with `import 'server-only'`, which throws under every condition except
// `react-server` — and that condition breaks next/navigation. So `server-only` is ALIASED to an
// empty stub, as in every @paigasus/* package with a server entry.
//
// The condition list stays ADDITIVE (not just ['node']): dropping `import`/`default` breaks the
// source-exports `.ts` resolution of the @paigasus/* packages. vitest 5 resolves a node test's
// imports through `ssr.resolve.conditions`, so both blocks carry the list (CLAUDE.md, SMA-502).
const conditions = ['node', 'import', 'default'];

const serverOnlyStub = fileURLToPath(new URL('./tests/support/server-only-stub.ts', import.meta.url));

// MEASURED (SMA-511 plan, Task 9): vite:oxc loads the NEAREST tsconfig.json for every file it
// transforms. This app's tsconfig.json extends '@paigasus/next-config/tsconfig-app' through pnpm's
// symlink, which oxc's resolver cannot follow — every import of lib/ or app/ failed with
// "[TSCONFIG_ERROR] Failed to load tsconfig … Tsconfig not found" — and the preset sets
// `jsx: preserve`, which Node cannot run. `tsconfig: false` stops the lookup, and the JSX runtime
// is stated here instead. Vite's OxcOptions type omits `tsconfig`, but the plugin spreads every key
// into rolldown's transformSync (vite 8.0.16), so a non-literal object carries it.
const oxc = { tsconfig: false, jsx: { runtime: 'automatic', importSource: 'react' } } as const;

export default defineConfig({
  oxc,
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts'],
    // Each standalone-runtime case boots a real Next standalone server twice; the default 5s is not enough.
    testTimeout: 120_000,
    hookTimeout: 120_000,
  },
  resolve: { conditions, alias: { 'server-only': serverOnlyStub } },
  ssr: { resolve: { conditions } },
});
