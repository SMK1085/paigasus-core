// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// `src/*` opens with `import 'server-only'`, whose exports map is
// { "react-server": "./empty.js", "default": "./index.js" } — and index.js is one unconditional
// throw. `react-server` is the condition that switches it off, so it is INCLUDED here (unlike
// @paigasus/auth's and @paigasus/discovery's configs, which measured a conflict between that
// condition and next/navigation — this package's leaf modules import neither).
//
// The list stays ADDITIVE: dropping `import`/`default` breaks source-exports `.ts` resolution for
// every @paigasus/* package (CLAUDE.md, SMA-502).
const conditions = ['react-server', 'node', 'import', 'default'];

const serverOnlyStub = fileURLToPath(new URL('./tests/support/server-only-stub.ts', import.meta.url));

// Both real modules throw outside a Next request scope. These doubles let a test set the request
// headers and cookies, and record revalidatePath() calls — copied from
// ts/apps/iam-console/vitest.config.ts, since a later task's moved code imports both.
const nextHeadersDouble = fileURLToPath(new URL('./tests/support/next-headers.ts', import.meta.url));
const nextCacheDouble = fileURLToPath(new URL('./tests/support/next-cache.ts', import.meta.url));

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
  },
  resolve: { conditions, alias: { 'server-only': serverOnlyStub, 'next/headers': nextHeadersDouble, 'next/cache': nextCacheDouble } },
  // Vitest 5 resolves a Node-environment test file's imports through `ssr.resolve.conditions`, NOT
  // the top-level `resolve.conditions` above — MEASURED on 5.0.0 in SMA-502. Setting only the
  // top-level key has no effect on an `environment: 'node'` project.
  ssr: { resolve: { conditions } },
});
