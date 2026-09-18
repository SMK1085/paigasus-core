// SPDX-License-Identifier: Apache-2.0
//
// mapWithLimit (SMA-636 spec § 4.2): the organization page lists projects per team with at most
// 8 calls in flight, and keeps the order of the teams.
import { describe, expect, it } from 'vitest';
import { mapWithLimit } from '../../lib/concurrency';

const later = <T>(value: T, ms: number): Promise<T> => new Promise((resolve) => setTimeout(() => resolve(value), ms));
const items = (count: number): number[] => Array.from({ length: count }, (_, index) => index);

describe('mapWithLimit', () => {
  it('keeps the input order in the results, whatever order the calls finish in', async () => {
    expect(await mapWithLimit([30, 10, 20], 2, (ms) => later(ms * 10, ms))).toEqual([300, 100, 200]);
  });

  it('never has more than the limit in flight, and reaches the limit when there is work for it', async () => {
    let inFlight = 0;
    let peak = 0;
    await mapWithLimit(items(20), 8, async () => {
      inFlight += 1;
      peak = Math.max(peak, inFlight);
      await later(null, 5);
      inFlight -= 1;
    });
    expect(peak).toBe(8);
  });

  it('answers an empty list with no call', async () => {
    let calls = 0;
    const results = await mapWithLimit([], 8, () => {
      calls += 1;
      return Promise.resolve(null);
    });
    expect(results).toEqual([]);
    expect(calls).toBe(0);
  });

  it('rejects a limit below 1', async () => {
    await expect(mapWithLimit([1], 0, (n) => Promise.resolve(n))).rejects.toThrow('limit');
  });
});
