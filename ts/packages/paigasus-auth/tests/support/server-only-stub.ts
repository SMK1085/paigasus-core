// SPDX-License-Identifier: Apache-2.0
//
// A permanent test-only stand-in for the real `server-only` package (see vitest.config.ts's
// `resolve.alias`). The real package's default export is an unconditional throw — correct in a
// real client bundle, useless in a Node test run that has no bundler-applied `react-server`
// condition to switch it to its no-op branch. Aliasing `server-only` to this empty module, rather
// than relying on that condition (task 10 review round 1), means ANY test that imports
// `src/server.ts` — now or later — resolves it silently, with no per-file `vi.mock` to remember.
export {};
