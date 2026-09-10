// SPDX-License-Identifier: Apache-2.0

/**
 * Should the access token be refreshed now?
 *
 * Pure and total. The clock is an ARGUMENT, not a dependency — the caller that already performs
 * I/O reads it. In the single-flight loop the caller re-reads it ONCE PER ITERATION; binding it
 * once outside the loop makes the waiter's deadline unreachable and the loop never terminates.
 */
export function shouldRefresh(now: number, expiresAt: number, skewMs: number): boolean {
  return now >= expiresAt - skewMs;
}
