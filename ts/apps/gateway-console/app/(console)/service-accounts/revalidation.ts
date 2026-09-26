// SPDX-License-Identifier: Apache-2.0
//
// Which action results refresh the settings pages (SMA-636 spec § 5.2-5.6). A plain module: a
// Server Actions file may export only async functions, so these rules cannot live in actions.ts.
//
// A `relogin` result NEVER revalidates (plan SPEC DEVIATION 6). The post-action render runs the
// (console) layout's requireSession(), whose redirect carries no basePath and leaves the zone
// (@paigasus/console-core's src/runtime.ts, on iamClientsForAction).
import type { ActionResult } from '@paigasus/console-core';
import type { CreateState, IssueKeyState } from './view';

/**
 * § 5.2's table: every create result except `invalid-input` (and `relogin`). A `degraded` or
 * `generic` failure revalidates too: step 1 may have committed before the response was lost. A
 * `partial` whose grant answered `relogin` does NOT revalidate either: the session ended between
 * the two calls, and the rule above holds for it too.
 */
export function refreshesAfterCreate(state: Exclude<CreateState, null>): boolean {
  if (state.kind === 'created') return true;
  if (state.kind === 'partial') return state.error.presentation !== 'relogin';
  return state.error.presentation !== 'invalid-input' && state.error.presentation !== 'relogin';
}

/** §§ 5.3, 5.5, 5.6: after every result. A `generic` allow can have committed before the answer was lost. */
export function refreshesAfterMutation(result: ActionResult): boolean {
  return result.ok || result.error.presentation !== 'relogin';
}

/**
 * § 5.4, after a success only (plan SPEC DEVIATION 8), so the new key row shows at once. The
 * revalidated render reads IAM, which never returns a token.
 */
export function refreshesAfterIssue(state: Exclude<IssueKeyState, null>): boolean {
  return state.ok;
}
