// SPDX-License-Identifier: Apache-2.0
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
// The fix is to stop resolving `server-only` for real at all: tests/server.test.ts mocks it
// (`vi.mock('server-only', () => ({}))`) instead. That is strictly narrower than a global
// resolve condition — it affects only the one test file that imports `src/server.ts` — and it
// means every other test, including the Next-navigation and React-DOM ones, resolves `react`
// and `react-dom` normally.
//
// The list below is kept ADDITIVE (not just `['node']`) for the same reason the original comment
// gave: dropping `import`/`default` breaks source-exports `.ts` resolution for every @paigasus/*
// package. `react-server` is simply no longer a member.
const conditions = ['node', 'import', 'default'];

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
    // The container-backed suites live under tests/containers/ and run only in the test-e2e task.
    exclude: ['tests/containers/**', 'tests/e2e/**', '**/node_modules/**'],
  },
  resolve: { conditions },
  ssr: { resolve: { conditions } },
});
