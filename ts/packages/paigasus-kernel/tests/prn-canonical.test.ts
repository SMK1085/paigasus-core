// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { prnCanonicalize, prnErrorKind } from '@paigasus/kernel';
import { prnCanonicalCases } from './corpus';

describe('kernel PRN canonical parity (napi)', () => {
  it('corpus is present and non-empty', () => {
    expect(prnCanonicalCases.length).toBeGreaterThan(0);
  });

  it.each(prnCanonicalCases)('prn($input)', ({ input, error_kind, canonical }) => {
    expect(prnErrorKind(input)).toBe(error_kind);
    if (error_kind === '') {
      expect(prnCanonicalize(input)).toBe(canonical);
    }
  });
});
