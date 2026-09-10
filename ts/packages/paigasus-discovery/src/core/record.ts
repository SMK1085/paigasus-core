// SPDX-License-Identifier: Apache-2.0
import { DEGRADED_REASONS, type DegradedReason, type ServiceDescriptor, type ServiceState } from '../types.js';

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
 * Bounded allowance for clock skew between console replicas that write `outcomeAt` on their own
 * clocks. Do NOT reject every future timestamp outright: legitimate skew between zones produces
 * small positive values, and rejecting those would treat every record from a slightly-ahead
 * replica as corrupt. Instead, a timestamp more than this many milliseconds in the future is
 * treated as STALE (forces a revalidation) rather than fresh — without this, a far-future
 * `outcomeAt` (clock error, or a corrupt/malicious value) would suppress revalidation until the
 * hard TTL, the opposite of the fail-fast behaviour freshness exists to provide.
 */
export const CLOCK_SKEW_ALLOWANCE_MS = 5_000;

/**
 * Whether a record may be served without re-probing.
 *
 * Reads `outcomeAt` in BOTH arms. `descriptorAt` moves only on success and is carried for
 * observability alone — using it for the `ok` arm would leave a failed probe honoured for
 * FRESH_MS whenever a stale descriptor happened to be recent, making NEGATIVE_MS dead code.
 *
 * An `outcomeAt` more than `CLOCK_SKEW_ALLOWANCE_MS` in the future is treated as stale (see the
 * comment on that constant) — checked before either arm below, since a far-future timestamp would
 * otherwise trivially satisfy `age < t.freshMs` via a large negative `age`.
 */
export function isFresh(rec: CacheRecord, now: number, t: Timings): boolean {
  const age = now - rec.outcomeAt;
  if (age < -CLOCK_SKEW_ALLOWANCE_MS) return false;
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
  return typeof d['service'] === 'string' && typeof d['version'] === 'string' && Array.isArray(d['capabilities']) && d['capabilities'].every((c) => typeof c === 'string');
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
  // `rev` starts at 1 on the first write (see single-flight.ts's `recordFor`) and only ever
  // increments, so anything but a positive safe integer is impossible for a genuine record.
  if (typeof r['rev'] !== 'number' || !Number.isSafeInteger(r['rev']) || r['rev'] <= 0) return null;
  if (typeof r['descriptorAt'] !== 'number' || !Number.isFinite(r['descriptorAt']) || r['descriptorAt'] < 0) return null;
  if (typeof r['outcomeAt'] !== 'number' || !Number.isFinite(r['outcomeAt']) || r['outcomeAt'] < 0) return null;
  if (r['outcome'] !== 'ok' && r['outcome'] !== 'fail') return null;
  if (r['descriptor'] !== null && !isDescriptor(r['descriptor'])) return null;
  // A foreign or corrupt `reason` must never reach `toState`/`ServiceState`, a CLOSED union:
  // `disabled.tsx`'s `REASON_TEXT[reason]` lookup would return `undefined` and render "<service>
  // is undefined" to the user. Validated against DEGRADED_REASONS — the one place the vocabulary
  // is declared — so this check cannot drift from the type.
  if (r['reason'] !== null && !(DEGRADED_REASONS as readonly unknown[]).includes(r['reason'])) return null;
  return parsed as CacheRecord;
}
