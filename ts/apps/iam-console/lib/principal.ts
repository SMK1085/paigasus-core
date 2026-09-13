// SPDX-License-Identifier: Apache-2.0
// TEMPORARY (SMA-512 PR 2, task 4 → task 5). A barrel so app files keep compiling while the
// package takes ownership. Task 5 deletes these and re-points the app at lib/console.ts.
//
// The side-effect import below restores an edge this barrel form otherwise drops — see
// lib/iam.ts's copy of this comment for the full story: importing any of these five barrels must
// still run lib/auth.ts's setConsolePorts() call, the way importing the pre-move lib/iam.ts used
// to (`import { authRuntime } from './auth'`), or the first accessor call in a fresh process
// throws instead of requireSession() redirecting to login.
import './auth';
import 'server-only';
export { currentPrincipal, introspectWithProvisioning, type Principal } from '@paigasus/console-core';
