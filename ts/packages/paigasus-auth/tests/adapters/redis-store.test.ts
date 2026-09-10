// SPDX-License-Identifier: Apache-2.0
//
// Unit-level only: this exercises `reconnectStrategy` directly, with no Redis connection. The
// store's own get/set/lock/txn behaviour is covered against a real Redis by
// tests/containers/redis-store.test.ts (test-e2e task) and tests/store-contract.ts.
import { describe, expect, it } from 'vitest';
import { reconnectStrategy } from '../../src/adapters/redis-store.js';

describe('reconnectStrategy', () => {
  // CodeRabbit finding (PR 230): a bounded RETRY COUNT that returns an `Error` past the cap makes
  // node-redis stop reconnecting permanently — every later call raises SessionStoreUnavailable
  // even after Redis recovers. The fix bounds only the backoff DELAY and never gives up.
  it('returns a number, never an Error, for a large retry count', () => {
    for (const retries of [0, 1, 10, 11, 100, 10_000, 1_000_000]) {
      const result = reconnectStrategy(retries);
      expect(result).not.toBeInstanceOf(Error);
      expect(typeof result).toBe('number');
    }
  });

  it('caps the backoff delay at 2000ms', () => {
    expect(reconnectStrategy(1_000_000)).toBe(2000);
  });

  it('grows the delay with the retry count below the cap', () => {
    expect(reconnectStrategy(0)).toBe(0);
    expect(reconnectStrategy(5)).toBe(500);
    expect(reconnectStrategy(19)).toBe(1900);
  });
});
