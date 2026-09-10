// SPDX-License-Identifier: Apache-2.0
import type { DegradedReason, ServiceDescriptor, ServiceState } from '../types.js';

/**
 * Bumped whenever CacheRecord's shape or DegradedReason's vocabulary changes.
 *
 * During a rolling upgrade two console builds write the same `pgs:svcinfo:<service>` key. Both
 * adapters treat a version mismatch as absent and DELETE it, the pattern SessionRecord already
 * uses. Without it a renamed reason poisons the cache fleet-wide for the full hard TTL.
 */
export const RECORD_VERSION = 1 as const;

export type CacheRecord = {
  readonly version: typeof RECORD_VERSION;
  /** Fences the write. See single-flight.ts — a compare-and-delete protects the LOCK, not the WRITE. */
  readonly rev: number;
  /** The last GOOD probe. Untouched by a failure, which is what keeps a degraded service's feature list. */
  readonly descriptor: ServiceDescriptor | null;
  readonly descriptorAt: number;
  /** The last probe ATTEMPT, successful or not. */
  readonly outcome: 'ok' | 'fail';
  readonly outcomeAt: number;
  readonly reason: DegradedReason | null;
};

export type Timings = {
  readonly negativeMs: number;
  readonly freshMs: number;
  readonly staleMs: number;
  readonly probeTimeoutMs: number;
  readonly lockWaitMs: number;
  readonly lockTtlMs: number;
};

export const DEFAULT_TIMINGS: Timings = {
  negativeMs: 10_000,
  freshMs: 60_000,
  staleMs: 600_000,
  probeTimeoutMs: 1_500,
  lockWaitMs: 2_500,
  lockTtlMs: 5_000,
};

/**
 * Whether a record may be served without re-probing.
 *
 * Reads `outcomeAt` in BOTH arms. `descriptorAt` moves only on success and is carried for
 * observability alone — using it for the `ok` arm would leave a failed probe honoured for
 * FRESH_MS whenever a stale descriptor happened to be recent, making NEGATIVE_MS dead code.
 */
export function isFresh(rec: CacheRecord, now: number, t: Timings): boolean {
  const age = now - rec.outcomeAt;
  return rec.outcome === 'ok' ? age < t.freshMs : age < t.negativeMs;
}

/**
 * Map a record to a state. Returns null when the record is internally impossible, which the
 * caller treats as corrupt: delete and re-probe.
 *
 * `service` is the CONFIGURED key, never `rec.descriptor.service` — see the proto MUST at
 * contracts/proto/paigasus/common/v1/service_info.proto:84-94.
 */
export function toState(service: string, rec: CacheRecord): ServiceState | null {
  if (rec.outcome === 'ok') {
    if (rec.descriptor === null) return null;
    return {
      state: 'available',
      service,
      descriptor: rec.descriptor,
      capabilities: rec.descriptor.capabilities,
    };
  }
  return {
    state: 'degraded',
    service,
    reason: rec.reason ?? 'network',
    descriptor: rec.descriptor,
    capabilities: rec.descriptor?.capabilities ?? [],
  };
}

function isDescriptor(v: unknown): v is ServiceDescriptor {
  if (typeof v !== 'object' || v === null) return false;
  const d = v as Record<string, unknown>;
  return (
    typeof d['service'] === 'string' &&
    typeof d['version'] === 'string' &&
    Array.isArray(d['capabilities']) &&
    d['capabilities'].every((c) => typeof c === 'string')
  );
}

/** Parse a stored record. Any doubt returns null, and the caller deletes the key. */
export function parseRecord(raw: string): CacheRecord | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof parsed !== 'object' || parsed === null) return null;
  const r = parsed as Record<string, unknown>;
  if (r['version'] !== RECORD_VERSION) return null;
  if (typeof r['rev'] !== 'number') return null;
  if (typeof r['descriptorAt'] !== 'number' || typeof r['outcomeAt'] !== 'number') return null;
  if (r['outcome'] !== 'ok' && r['outcome'] !== 'fail') return null;
  if (r['descriptor'] !== null && !isDescriptor(r['descriptor'])) return null;
  if (r['reason'] !== null && typeof r['reason'] !== 'string') return null;
  return parsed as CacheRecord;
}
