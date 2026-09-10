// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { shouldRefresh } from '../../src/core/refresh-policy.js';

// Every case varies the STORED timestamp, never the clock. That is the whole reason this package
// has no Clock port: moving expiresAt is equivalent to moving now, and it needs no machinery.
describe('shouldRefresh', () => {
  const skew = 30_000;
  const now = 1_000_000;

  it.each([
    ['far in the future', now + 600_000, false],
    ['just outside the skew window', now + skew + 1, false],
    ['exactly at the skew boundary', now + skew, true],
    ['just inside the skew window', now + skew - 1, true],
    ['exactly at expiry', now, true],
    ['already expired', now - 1, true],
    ['long expired', now - 600_000, true],
  ])('%s', (_label, expiresAt, expected) => {
    expect(shouldRefresh(now, expiresAt, skew)).toBe(expected);
  });

  it('a zero skew refreshes only at or past expiry', () => {
    expect(shouldRefresh(now, now + 1, 0)).toBe(false);
    expect(shouldRefresh(now, now, 0)).toBe(true);
  });
});
