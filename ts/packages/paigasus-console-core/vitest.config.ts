// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// NO `react-server` CONDITION HERE (task 3 correction, SMA-512 PR 2). Task 1's version of this
// file carried it, reasoning that `src/*`'s `import 'server-only'` (exports map
// { "react-server": "./empty.js", "default": "./index.js" }, `index.js` an unconditional throw)
// needed the condition present, and that the package's leaf modules imported neither
// `next/navigation` nor `react-dom/client`, so the condition cost nothing. That stopped being true
// once task 3 moved `src/correlation.ts` (`unstable_rethrow` from `next/navigation`) and its test
// (`notFound`/`redirect` from `next/navigation`) into this package: `react-server` is ALSO the
// condition `react`'s own exports map switches on, and the react-server build has no
// `createContext`, which `next/navigation`'s client dist file calls at module scope — the same
// conflict @paigasus/auth's vitest.config.ts records (its own task 10 correction). There is no
// flat condition list that satisfies both, so the fix is the same one: a permanent ALIAS below
// makes `server-only` resolve to an empty stub unconditionally, which needs no `react-server`
// condition at all.
//
// The list stays ADDITIVE: dropping `import`/`default` breaks source-exports `.ts` resolution for
// every @paigasus/* package (CLAUDE.md, SMA-502).
const conditions = ['node', 'import', 'default'];

const serverOnlyStub = fileURLToPath(new URL('./tests/support/server-only-stub.ts', import.meta.url));

// Both real modules throw outside a Next request scope. These doubles let a test set the request
// headers and cookies, and record revalidatePath() calls — copied from
// ts/apps/iam-console/vitest.config.ts, since a later task's moved code imports both.
const nextHeadersDouble = fileURLToPath(new URL('./tests/support/next-headers.ts', import.meta.url));
const nextCacheDouble = fileURLToPath(new URL('./tests/support/next-cache.ts', import.meta.url));

// MEASURED (SMA-511 plan, Task 9; hit again here in task 3): vite:oxc loads the NEAREST
// tsconfig.json for every file it transforms. `tests/integration/principal.test.ts` TEMPORARILY
// reaches into `ts/apps/iam-console/lib/{authorize,principal}.ts` (task 3 → task 4) — and that
// app's tsconfig.json extends '@paigasus/next-config/tsconfig-app' through pnpm's symlink, which
// oxc's resolver cannot follow, failing with "[TSCONFIG_ERROR] Tsconfig not found". `tsconfig:
// false` stops the lookup for every file this config transforms, this package's own included; see
// ts/apps/iam-console/vitest.config.ts for the original occurrence. This package has no .tsx
// source or test file, but the `jsx` option is set anyway — MEASURED: `{ tsconfig: false }` alone
// fails `tsc`'s overload resolution for `defineConfig` (TS2769), and matching the app's exact
// shape (`tsconfig` plus `jsx`) is what resolves the right overload.
const oxc = { tsconfig: false, jsx: { runtime: 'automatic', importSource: 'react' } } as const;

export default defineConfig({
  oxc,
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
    setupFiles: ['./tests/support/setup.ts'],
  },
  resolve: { conditions, alias: { 'server-only': serverOnlyStub, 'next/headers': nextHeadersDouble, 'next/cache': nextCacheDouble } },
  // Vitest 5 resolves a Node-environment test file's imports through `ssr.resolve.conditions`, NOT
  // the top-level `resolve.conditions` above — MEASURED on 5.0.0 in SMA-502. Setting only the
  // top-level key has no effect on an `environment: 'node'` project.
  ssr: { resolve: { conditions } },
});
