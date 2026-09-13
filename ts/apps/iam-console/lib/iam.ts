// SPDX-License-Identifier: Apache-2.0
// TEMPORARY (SMA-512 PR 2, task 4 → task 5). A barrel so app files keep compiling while the
// package takes ownership. Task 5 deletes these and re-points the app at lib/console.ts.
import 'server-only';
export { currentSession, iamClients, iamClientsForAction, optionalSession, sessionToken, type IamClients } from '@paigasus/console-core';
