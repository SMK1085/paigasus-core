// SPDX-License-Identifier: Apache-2.0
//
// A test-only stand-in for `server-only` (vitest.config.ts `resolve.alias`). The real package's
// default export is an unconditional throw. The `react-server` condition that switches it off also
// switches `react` to its server build, which breaks next/navigation (measured in @paigasus/auth;
// see its vitest.config.ts). Same pattern as ts/packages/paigasus-auth/tests/support/.
export {};
