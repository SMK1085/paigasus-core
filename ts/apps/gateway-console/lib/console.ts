// SPDX-License-Identifier: Apache-2.0
//
// The app's ONE call to createConsoleRuntime, at module scope. Called once per module graph, which
// is a correctness requirement, not a convention: each accessor is a React cache() wrapper, and a
// second call would make a second memoization identity. Outside a server render cache() is a
// pass-through, so no unit or integration test can catch a regression here — only an e2e WhoAmI
// count can (SMA-632).
import 'server-only';
import { createConsoleRuntime, logger } from '@paigasus/console-core';
import { authRuntime } from './auth';
import { getRuntimeConfig } from './config';

// `authRuntime: () => authRuntime()` wraps the import in a fresh arrow rather than passing the
// imported binding straight through. This module and ./auth import from each other, and Next
// gives a route handler and a page SEPARATE module graphs (CLAUDE.md, SMA-511), so both entry
// orders occur in production. The wrapper resolves the `authRuntime` identifier at CALL time, not
// at module-evaluation time, so it stays safe no matter which declaration form ./auth uses — a
// hoisted `export function` or a plain `export const` alike.
export const { currentSession, optionalSession, sessionToken, iamClients, iamClientsForAction, iamClientsForToken, currentPrincipal, mayI, myScopes, discovery } = createConsoleRuntime({
  config: () => getRuntimeConfig(),
  authRuntime: () => authRuntime(),
  logger,
});
