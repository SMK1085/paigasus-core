// SPDX-License-Identifier: Apache-2.0
//
// A permanent test-only stand-in for the real `server-only` package (see vitest.config.ts's
// `resolve.alias`). The real package's default export is an unconditional throw. Aliasing it
// here — rather than adding the `react-server` resolution condition — is deliberate and is
// MEASURED in ts/packages/paigasus-auth/vitest.config.ts: `react-server` is also the condition
// `react`'s own exports map switches on, and that build has no `createContext`, so setting it
// breaks `react-dom/client` under @testing-library/react. There is no flat condition list that
// satisfies both. The alias applies to every test with nothing to remember per file.
export {};
