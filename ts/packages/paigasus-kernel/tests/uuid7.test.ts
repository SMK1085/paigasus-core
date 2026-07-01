// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { mintUuid7 } from '@paigasus/kernel';
import { uuid7Cases } from './corpus';

describe('kernel UUIDv7 parity (napi)', () => {
  it('corpus is present and non-empty', () => {
    expect(uuid7Cases.length).toBeGreaterThan(0);
  });

  it.each(uuid7Cases)('mintUuid7($unix_ms, $rand_hex)', ({ unix_ms, rand_hex, expected_uuid }) => {
    expect(mintUuid7(unix_ms, rand_hex)).toBe(expected_uuid);
  });
});
