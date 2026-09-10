// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { CLOCK_SKEW_ALLOWANCE_MS, DEFAULT_TIMINGS, RECORD_VERSION, isFresh, parseRecord, toState, type CacheRecord } from '../src/core/record.js';
import type { ServiceDescriptor } from '../src/types.js';

const descriptor: ServiceDescriptor = {
  service: 'iam',
  version: '0.0.0',
  capabilities: ['iam.audit', 'iam.apikeys'],
};

function rec(over: Partial<CacheRecord> = {}): CacheRecord {
  return {
    version: RECORD_VERSION,
    rev: 1,
    descriptor,
    descriptorAt: 0,
    outcome: 'ok',
    outcomeAt: 0,
    reason: null,
    ...over,
  };
}

describe('isFresh', () => {
  it('honours a successful probe for FRESH_MS', () => {
    expect(isFresh(rec({ outcomeAt: 0 }), 59_000, DEFAULT_TIMINGS)).toBe(true);
    expect(isFresh(rec({ outcomeAt: 0 }), 61_000, DEFAULT_TIMINGS)).toBe(false);
  });

  it('honours a failed probe only for NEGATIVE_MS', () => {
    const failed = rec({ outcome: 'fail', reason: 'network', outcomeAt: 0 });
    expect(isFresh(failed, 9_000, DEFAULT_TIMINGS)).toBe(true);
    expect(isFresh(failed, 11_000, DEFAULT_TIMINGS)).toBe(false);
  });

  it('reads outcomeAt and NEVER descriptorAt', () => {
    // THE REGRESSION THIS TEST EXISTS FOR. A probe that failed at t=0 over a descriptor
    // fetched at t=-5000. At t=20000 the descriptor is inside FRESH_MS (25s < 60s) but the
    // OUTCOME is outside NEGATIVE_MS (20s > 10s). Reading descriptorAt here would leave a
    // down service un-probed for a full 60s and make the negative TTL dead code.
    const failed = rec({ outcome: 'fail', reason: 'network', descriptorAt: -5_000, outcomeAt: 0 });
    expect(isFresh(failed, 20_000, DEFAULT_TIMINGS)).toBe(false);
  });

  it('tolerates small clock skew: a slightly future outcomeAt is still fresh', () => {
    // now=0, outcomeAt=CLOCK_SKEW_ALLOWANCE_MS - 1 => age is a small negative number, inside the
    // allowance. Legitimate skew between zones must not be treated as corrupt.
    const skewed = rec({ outcomeAt: CLOCK_SKEW_ALLOWANCE_MS - 1 });
    expect(isFresh(skewed, 0, DEFAULT_TIMINGS)).toBe(true);
  });

  it('treats a far-future outcomeAt as STALE, not fresh', () => {
    // THE REGRESSION THIS TEST EXISTS FOR (F3c). Without the skew allowance, a large negative
    // `age` trivially satisfies `age < t.freshMs`, so a far-future timestamp (clock error, or a
    // corrupt/malicious value) would suppress revalidation until the hard TTL instead of forcing
    // one.
    const farFuture = rec({ outcomeAt: DEFAULT_TIMINGS.freshMs + CLOCK_SKEW_ALLOWANCE_MS + 1 });
    expect(isFresh(farFuture, 0, DEFAULT_TIMINGS)).toBe(false);
  });
});

describe('toState', () => {
  it('maps ok + descriptor to available', () => {
    expect(toState('iam', rec())).toEqual({
      state: 'available',
      service: 'iam',
      descriptor,
      capabilities: ['iam.audit', 'iam.apikeys'],
    });
  });

  it('maps fail + descriptor to degraded, keeping the capability list', () => {
    const state = toState('iam', rec({ outcome: 'fail', reason: 'network' }));
    expect(state).toEqual({
      state: 'degraded',
      service: 'iam',
      reason: 'network',
      descriptor,
      capabilities: ['iam.audit', 'iam.apikeys'],
    });
  });

  it('maps fail + no descriptor to degraded with an empty capability list', () => {
    const state = toState('iam', rec({ outcome: 'fail', reason: 'timeout', descriptor: null }));
    expect(state).toEqual({
      state: 'degraded',
      service: 'iam',
      reason: 'timeout',
      descriptor: null,
      capabilities: [],
    });
  });

  it('uses the CONFIGURED service name, never the descriptor-reported one', () => {
    // The proto MUST (service_info.proto:84-94): ServiceInfo.service is advisory and never a
    // cache key, because a misconfigured or hostile service could otherwise poison another
    // service's entry.
    const hostile = rec({ descriptor: { ...descriptor, service: 'gateway' } });
    const state = toState('iam', hostile);
    expect(state).not.toBeNull();
    expect(state?.service).toBe('iam');
  });

  it('treats ok + null descriptor as corrupt', () => {
    expect(toState('iam', rec({ descriptor: null }))).toBeNull();
  });
});

describe('parseRecord', () => {
  it('accepts a well-formed record', () => {
    expect(parseRecord(JSON.stringify(rec()))).toEqual(rec());
  });

  it('rejects a record from a different schema version', () => {
    // A rolling upgrade has two console builds writing the same key. Without this, a renamed
    // DegradedReason poisons the cache fleet-wide for the whole 10-minute hard TTL.
    expect(parseRecord(JSON.stringify({ ...rec(), version: 2 }))).toBeNull();
  });

  it('rejects unparseable JSON', () => {
    expect(parseRecord('{not json')).toBeNull();
  });

  it('rejects a structurally wrong record', () => {
    expect(parseRecord(JSON.stringify({ version: RECORD_VERSION, rev: 'x' }))).toBeNull();
  });

  it('rejects a record whose descriptor is structurally wrong (capabilities not an array)', () => {
    const bad = { ...rec(), descriptor: { service: 'iam', version: '0.0.0', capabilities: 'iam.audit' } };
    expect(parseRecord(JSON.stringify(bad))).toBeNull();
  });

  it('rejects a reason outside the DegradedReason vocabulary', () => {
    // THE REGRESSION THIS TEST EXISTS FOR (F3a). An unvalidated `reason` flows through `toState`
    // into a CLOSED union; `disabled.tsx`'s `REASON_TEXT[reason]` lookup then returns `undefined`
    // and renders "<service> is undefined" to the user.
    const bad = { ...rec({ outcome: 'fail' }), reason: 'made-up-reason' };
    expect(parseRecord(JSON.stringify(bad))).toBeNull();
  });

  it('accepts every real DegradedReason value', () => {
    for (const reason of ['timeout', 'network', 'unauthorized', 'not-implemented', 'bad-response', 'server-error', 'cache-unavailable'] as const) {
      const good = rec({ outcome: 'fail', reason });
      expect(parseRecord(JSON.stringify(good))).toEqual(good);
    }
  });

  it('rejects a non-integer or non-positive rev', () => {
    expect(parseRecord(JSON.stringify({ ...rec(), rev: 0 }))).toBeNull();
    expect(parseRecord(JSON.stringify({ ...rec(), rev: -1 }))).toBeNull();
    expect(parseRecord(JSON.stringify({ ...rec(), rev: 1.5 }))).toBeNull();
    // Not safe: 2^53 is not exactly representable, so equality/ordering on it is unreliable.
    expect(parseRecord(JSON.stringify({ ...rec(), rev: Number.MAX_SAFE_INTEGER + 1 }))).toBeNull();
  });

  it('rejects a negative timestamp', () => {
    expect(parseRecord(JSON.stringify({ ...rec(), descriptorAt: -1 }))).toBeNull();
    expect(parseRecord(JSON.stringify({ ...rec(), outcomeAt: -1 }))).toBeNull();
  });

  it('rejects a non-finite timestamp', () => {
    // JSON has no literal for Infinity/NaN, so JSON.stringify would silently coerce them to
    // `null` and defeat this test — an oversized exponent is what JSON.parse itself turns into
    // Infinity, which is how a corrupt or hand-crafted payload could reach this field.
    expect(parseRecord('{"version":1,"rev":1,"descriptor":null,"descriptorAt":0,"outcome":"fail","outcomeAt":1e999,"reason":"network"}')).toBeNull();
  });
});
