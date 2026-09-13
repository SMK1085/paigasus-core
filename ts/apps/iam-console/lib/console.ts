// SPDX-License-Identifier: Apache-2.0
//
// The app's ONE call to createConsoleRuntime, at module scope. Called once per module graph, which
// is a correctness requirement, not a convention: each accessor is a React cache() wrapper, and a
// second call would make a second memoization identity. Outside a server render cache() is a
// pass-through, so no unit or integration test can catch a regression here — only an e2e Introspect
// count can.
import 'server-only';
import { createConsoleRuntime, logger } from '@paigasus/console-core';
import { authRuntime } from './auth';
import { getRuntimeConfig } from './config';

export const { currentSession, optionalSession, sessionToken, iamClients, iamClientsForAction, iamClientsForToken, currentPrincipal, mayI, myScopes, discovery } = createConsoleRuntime({
  config: () => getRuntimeConfig(),
  authRuntime,
  logger,
});
