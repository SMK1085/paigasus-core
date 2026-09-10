// SPDX-License-Identifier: Apache-2.0
import type { CacheRecord } from '../core/record.js';

/**
 * Primitives only, no policy. The single-flight algorithm lives once in core/single-flight.ts and
 * runs identically against every adapter — the same split @paigasus/auth's SessionStore uses.
 *
 * `set` is a COMPARE-AND-SET on `rev`. This is invariant 5: a lock's compare-and-delete protects
 * the LOCK, never the WRITE. Pass `expectedRev: null` to mean "only if absent".
 * It returns false when the fence lost.
 *
 * Every method may throw. The caller degrades rather than propagating — one Redis blip must not
 * 500 every server component rendering navigation.
 */
export interface DescriptorCache {
  get(service: string): Promise<CacheRecord | null>;
  set(service: string, rec: CacheRecord, ttlMs: number, expectedRev: number | null): Promise<boolean>;
  delete(service: string): Promise<void>;
  tryAcquireLock(service: string, token: string, ttlMs: number): Promise<boolean>;
  releaseLock(service: string, token: string): Promise<void>;
  close(): Promise<void>;

  /**
   * TEST SEAM, optional. Plants a raw stored value so the shared contract can prove that a record
   * from a FOREIGN SCHEMA VERSION is discarded and deleted rather than served — the rolling-upgrade
   * case, which no other route can set up because every normal write goes through `set`.
   *
   * Declared here rather than bolted on from the test file with `declare module`: augmenting a
   * module through a relative specifier is fragile, and it would put a production interface's
   * shape inside a test.
   */
  writeRawForTest?(service: string, raw: string): Promise<void>;
}
