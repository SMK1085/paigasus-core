// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { sum } from '@paigasus/kernel';
import { cases } from './corpus';

describe('kernel FFI parity (napi)', () => {
  it('corpus is present and non-empty', () => {
    // Integrity guard: an empty corpus (a bad path) registers zero `it.each` cases below, so
    // without this assertion the file would pass green having compared nothing.
    expect(cases.length).toBeGreaterThan(0);
  });

  it.each(cases)('sum($a, $b) === $expected', ({ a, b, expected }) => {
    expect(sum(a, b)).toBe(expected);
  });
});
