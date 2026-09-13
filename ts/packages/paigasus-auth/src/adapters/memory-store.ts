// SPDX-License-Identifier: Apache-2.0
//
// SINGLE-PROCESS ONLY, and that is a correctness statement, not a caveat.
//
// The multi-zone topology runs one Next process per zone. Under this adapter the zones share
// nothing: a user who logs in on zone A is anonymous on zone B, and the single-flight lock is
// per-process, so AC 2's "exactly one refresh" does NOT hold across processes.
//
// createAuthRuntime therefore REFUSES this adapter when the zone map declares more than one zone.
// It is for local development and single-process tests.
import type { SessionRecord } from '../core/session';
import { isSessionRecord } from '../core/session';
import type { LoginTransaction, SessionStore } from '../ports/session-store';

interface Entry<T> {
  value: T;
  expiresAt: number;
}

export class MemorySessionStore implements SessionStore {
  readonly #records = new Map<string, Entry<SessionRecord>>();
  readonly #locks = new Map<string, Entry<string>>();
  readonly #txns = new Map<string, Entry<LoginTransaction>>();

  #live<T>(map: Map<string, Entry<T>>, key: string): T | null {
    const hit = map.get(key);
    if (hit === undefined) return null;
    if (Date.now() >= hit.expiresAt) {
      map.delete(key);
      return null;
    }
    return hit.value;
  }

  get(sid: string): Promise<SessionRecord | null> {
    const rec = this.#live(this.#records, sid);
    // The SAME predicate the Redis adapter uses (SMA-626 § 4). This adapter never parses bytes,
    // so only a caller violating the type could poison it — sharing the rule means the policy is
    // stated once rather than twice and drifting.
    if (rec !== null && !isSessionRecord(rec)) {
      this.#records.delete(sid);
      return Promise.resolve(null);
    }
    return Promise.resolve(rec);
  }

  set(sid: string, rec: SessionRecord, ttlMs: number, expectedRev: number | null): Promise<boolean> {
    const current = this.#live(this.#records, sid);
    if (expectedRev === null) {
      if (current !== null) return Promise.resolve(false);
    } else if (current === null || current.rev !== expectedRev) return Promise.resolve(false);
    this.#records.set(sid, { value: rec, expiresAt: Date.now() + ttlMs });
    return Promise.resolve(true);
  }

  delete(sid: string): Promise<void> {
    this.#records.delete(sid);
    return Promise.resolve();
  }

  tryAcquireLock(sid: string, token: string, ttlMs: number): Promise<boolean> {
    if (this.#live(this.#locks, sid) !== null) return Promise.resolve(false);
    this.#locks.set(sid, { value: token, expiresAt: Date.now() + ttlMs });
    return Promise.resolve(true);
  }

  releaseLock(sid: string, token: string): Promise<void> {
    if (this.#live(this.#locks, sid) === token) this.#locks.delete(sid);
    return Promise.resolve();
  }

  putTransaction(txnId: string, tx: LoginTransaction, ttlMs: number): Promise<void> {
    this.#txns.set(txnId, { value: tx, expiresAt: Date.now() + ttlMs });
    return Promise.resolve();
  }

  // Atomic by construction: JavaScript runs this synchronously to completion, so no other
  // caller can observe the entry between the read and the delete.
  takeTransaction(txnId: string): Promise<LoginTransaction | null> {
    const tx = this.#live(this.#txns, txnId);
    if (tx !== null) this.#txns.delete(txnId);
    return Promise.resolve(tx);
  }

  close(): Promise<void> {
    this.#records.clear();
    this.#locks.clear();
    this.#txns.clear();
    return Promise.resolve();
  }
}
