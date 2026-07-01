// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { prnCedarEntityId, prnCedarEntityType } from '@paigasus/kernel';
import { prnCedarCases } from './corpus';

describe('kernel PRN→Cedar parity (wasm)', () => {
  it('corpus is present and non-empty', () => {
    expect(prnCedarCases.length).toBeGreaterThan(0);
  });

  it.each(prnCedarCases)('cedar($prn)', ({ prn, entity_type, entity_id }) => {
    expect(prnCedarEntityType(prn)).toBe(entity_type);
    expect(prnCedarEntityId(prn)).toBe(entity_id);
  });
});
