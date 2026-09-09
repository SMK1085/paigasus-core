// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// NO `react-server` CONDITION HERE (task 10 correction). An earlier revision of this file carried
// it, anticipating `src/server.ts`'s `import 'server-only'` — whose exports map is
// `{ "react-server": "./empty.js", "default": "./index.js" }`, and `index.js` is an unconditional
// throw. That reasoning was correct in isolation but incomplete: MEASURED once `src/server.ts`
// (and `src/next/get-session.ts`, which it re-exports) actually existed and imported
// `next/navigation` — `react-server` is ALSO the condition `react`'s own exports map switches on
// (`{ "react-server": "./react.react-server.js", "default": "./index.js" }`), and the
// react-server build has no `createContext`. `next/navigation`'s client dist file calls it at
// module scope, so the condition that makes `server-only` a no-op simultaneously makes
// `next/navigation` (and, for the same reason, `react-dom/client` under `@testing-library/react`
// in tests/client.test.tsx) fail with `createContext is not a function` / "not supported in React
// Server Components". There is no single flat condition list that satisfies both — `server-only`
// needs the condition PRESENT, everything reachable from `next/navigation` or `react-dom/client`
// needs it ABSENT.
//
// The fix (task 10 review round 1) is a permanent ALIAS, not a per-file mock: `server-only` always
// resolves to `tests/support/server-only-stub.ts`, an empty module, for every test in this package.
// A per-file `vi.mock('server-only', ...)` in only the test that happened to need it was strictly
// narrower than the condition it replaced, but it left a trap of its own — any FUTURE test that
// imports `src/server.ts` and forgets that one-line mock dies at import with the real package's
// opaque "cannot be imported from a Client Component" throw. The alias makes the stub apply to
// every test unconditionally, so there is nothing left to forget, and it touches nothing else:
// `react`, `react-dom`, and `next/navigation` all resolve completely normally, exactly as they
// would in a real Node process.
//
// The list below is kept ADDITIVE (not just `['node']`) for the same reason the original comment
// gave: dropping `import`/`default` breaks source-exports `.ts` resolution for every @paigasus/*
// package. `react-server` is simply no longer a member.
const conditions = ['node', 'import', 'default'];

const serverOnlyStub = fileURLToPath(new URL('./tests/support/server-only-stub.ts', import.meta.url));

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
    // The container-backed suites live under tests/containers/ and run only in the test-e2e task.
    exclude: ['tests/containers/**', 'tests/e2e/**', '**/node_modules/**'],
  },
  resolve: { conditions, alias: { 'server-only': serverOnlyStub } },
  ssr: { resolve: { conditions } },
});
