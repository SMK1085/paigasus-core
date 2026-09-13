// SPDX-License-Identifier: Apache-2.0
import type { DegradedReason } from '../types';

/**
 * HTTP status to reason.
 *
 * Owned here, not imported from @paigasus/sdk: that package's `Presentation` union
 * ('relogin' | 'forbidden' | 'not-found' | 'degraded' | ...) cannot express any of these, its
 * HTTP table has no 500 row, and it deliberately has no 2xx row (spec F8). Taking the dependency
 * would also give @paigasus/app-shell a transitive path to the SDK, which its eslint boundary
 * block bans.
 */
export function reasonForStatus(status: number): DegradedReason {
  if (status === 401 || status === 403) return 'unauthorized';
  // 404 means a service predating the descriptor, or one whose capability routes are unmounted —
  // ADR-0020 A2 makes those deliberately indistinguishable.
  if (status === 404) return 'not-implemented';
  return 'server-error';
}

/** A thrown fetch failure to a reason. */
export function reasonForThrown(err: unknown): DegradedReason {
  if (err instanceof Error && (err.name === 'TimeoutError' || err.name === 'AbortError')) {
    return 'timeout';
  }
  // `fetch` reports connection refused, DNS failure, TLS failure AND a refused redirect as
  // TypeError. All are 'network' for our purposes.
  return 'network';
}
