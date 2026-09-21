// SPDX-License-Identifier: Apache-2.0
//
// IAM timestamps as ISO strings (SMA-629 spec § 6.3). ONE copy, moved from the audit page's loader:
// the audit page and the dead-letters page both use it.
import 'server-only';

/** A protobuf Timestamp as protobuf-es gives it. */
export type ProtoTimestamp = { readonly seconds: bigint; readonly nanos: number };

/**
 * `seconds` is a protobuf int64 and can hold a value outside the ECMAScript Date range
 * (±8,640,000,000,000 ms from the epoch). `Date#toISOString()` throws on an out-of-range Date, so
 * validate first and return null rather than crash the page on a malformed IAM timestamp.
 */
export function timestampIso(value: ProtoTimestamp | undefined): string | null {
  if (value === undefined) return null;
  const date = new Date(Number(value.seconds) * 1000 + Math.floor(value.nanos / 1_000_000));
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

/**
 * The inverse of `timestampIso`, for a request (SMA-661 spec § 4.4). `iso` is a value that
 * lib/paging.ts's `canonicalParkedBound` already accepted, so `Date.parse` is finite. The seconds are
 * floored, so an instant before the epoch keeps non-negative nanos, as the Timestamp type requires.
 * The spec named protobuf-es's `timestampFromDate`; this app does not depend on `@bufbuild/protobuf`,
 * and a plain `{ seconds, nanos }` is a valid message init for a Timestamp field.
 */
export function timestampFromIso(iso: string): ProtoTimestamp {
  const ms = Date.parse(iso);
  const seconds = Math.floor(ms / 1000);
  return { seconds: BigInt(seconds), nanos: (ms - seconds * 1000) * 1_000_000 };
}
