// SPDX-License-Identifier: Apache-2.0
// TEMPORARY (SMA-512 PR 2, task 4 → task 5). A barrel so app files keep compiling while the
// package takes ownership. Task 5 deletes these and re-points the app at lib/console.ts.
//
// The side-effect import below is load-bearing. Before this task, importing lib/iam.ts
// transitively imported lib/auth.ts (`import { authRuntime } from './auth'`), which is what made
// EVERY page/load/action file that reaches an accessor here also run auth.ts's module-scope
// setConsolePorts() call. This barrel points straight at @paigasus/console-core instead, which has
// no edge back to the app's lib/auth.ts, so that guarantee is gone unless restored here. Without
// it, the first request in a fresh process to any accessor before app/auth/[...auth]/route.ts (the
// ONE remaining importer of lib/auth.ts) has run throws "createConsoleRuntime() was never called"
// instead of requireSession() redirecting to login — a 500 where a redirect belongs. Task 5's
// createConsoleRuntime() call replaces this whole mechanism.
import './auth';
import 'server-only';
export { currentSession, iamClients, iamClientsForAction, optionalSession, sessionToken, type IamClients } from '@paigasus/console-core';
