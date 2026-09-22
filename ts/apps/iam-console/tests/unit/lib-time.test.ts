// SPDX-License-Identifier: Apache-2.0
//
// timestampIso (SMA-629 spec § 6.3): ONE copy of the IAM timestamp conversion for the audit and the
// dead-letters pages. The audit loader's own boundary cases stay in tests/integration/audit-page.test.ts.
import { describe, expect, it } from 'vitest';
import { timestampFromIso, timestampIso } from '../../lib/time';

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

// SMA-661 spec § 4.4. The request side: a canonical instant as a protobuf Timestamp.
describe('timestampFromIso', () => {
  it('is the inverse of timestampIso for a canonical instant', () => {
    expect(timestampFromIso('2026-09-19T00:00:00.250Z')).toEqual({ seconds: 1_789_776_000n, nanos: 250_000_000 });
    expect(timestampIso(timestampFromIso('2026-09-19T00:00:00.250Z'))).toBe('2026-09-19T00:00:00.250Z');
  });

  it('floors the seconds before the epoch, so the nanos stay non-negative', () => {
    expect(timestampFromIso('1969-12-31T23:59:59.500Z')).toEqual({ seconds: -1n, nanos: 500_000_000 });
  });
});
