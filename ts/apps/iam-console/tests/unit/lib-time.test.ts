// SPDX-License-Identifier: Apache-2.0
//
// timestampIso (SMA-629 spec § 6.3): ONE copy of the IAM timestamp conversion for the audit and the
// dead-letters pages. The audit loader's own boundary cases stay in tests/integration/audit-page.test.ts.
import { describe, expect, it } from 'vitest';
import { timestampIso } from '../../lib/time';

describe('timestampIso', () => {
  it('reads no timestamp as null', () => {
    expect(timestampIso(undefined)).toBeNull();
  });

  it('adds the nanos as whole milliseconds', () => {
    expect(timestampIso({ seconds: 1_788_000_000n, nanos: 250_999_999 })).toBe(new Date(1_788_000_000_250).toISOString());
  });

  it('keeps the exact Date ceiling and reads one second past it as null, not a thrown RangeError', () => {
    expect(timestampIso({ seconds: 8_640_000_000_000n, nanos: 0 })).toBe(new Date(8_640_000_000_000_000).toISOString());
    expect(timestampIso({ seconds: 8_640_000_000_001n, nanos: 0 })).toBeNull();
    expect(timestampIso({ seconds: 1_000_000_000_000_000n, nanos: 0 })).toBeNull();
  });
});
