// SPDX-License-Identifier: Apache-2.0
//
// A bounded fan-out (SMA-636 spec § 4.2): the organization page lists the projects of each team
// with at most 8 calls in flight. The results keep the input order.

export async function mapWithLimit<T, R>(items: readonly T[], limit: number, fn: (item: T) => Promise<R>): Promise<R[]> {
  if (!Number.isInteger(limit) || limit < 1) throw new RangeError('mapWithLimit: the limit must be a positive integer');
  const results = new Array<R>(items.length);
  let next = 0;
  const worker = async (): Promise<void> => {
    while (next < items.length) {
      const index = next;
      next += 1;
      results[index] = await fn(items[index] as T);
    }
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, () => worker()));
  return results;
}
