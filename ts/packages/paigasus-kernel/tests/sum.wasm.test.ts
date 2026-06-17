// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { sum } from '@paigasus/kernel';

describe('kernel FFI (wasm)', () => {
  it('crosses the wasm boundary', () => {
    expect(sum(2, 3)).toBe(5);
    expect(sum(-4, 4)).toBe(0);
  });
});
