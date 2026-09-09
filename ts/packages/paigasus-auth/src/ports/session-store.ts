// SPDX-License-Identifier: Apache-2.0
import type { SessionRecord } from '../core/session.js';

export interface LoginTransaction {
  codeVerifier: string;
  nonce: string;
  returnTo: string;
  /** SHA-256 of the browser-bound secret. The secret itself lives only in the cookie. */
  secretHash: string;
  createdAt: number;
}

/**
 * PRIMITIVES ONLY — no policy. The single-flight refresh algorithm lives once in
 * core/single-flight.ts and both adapters inherit it, rather than being implemented twice and
 * being right once.
 */
export interface SessionStore {
  get(sid: string): Promise<SessionRecord | null>;

  /**
   * Compare-and-set on `rev`. Returns false when the stored rev has moved, meaning another
   * writer fenced this one. A `null` expectedRev inserts only when no record exists.
   */
  set(sid: string, rec: SessionRecord, ttlMs: number, expectedRev: number | null): Promise<boolean>;

  delete(sid: string): Promise<void>;

  tryAcquireLock(sid: string, token: string, ttlMs: number): Promise<boolean>;
  /** Compare-and-delete: releases only if `token` still owns the lock. */
  releaseLock(sid: string, token: string): Promise<void>;

  putTransaction(txnId: string, tx: LoginTransaction, ttlMs: number): Promise<void>;
  /** ATOMIC get-and-delete. A separate get + delete is a race, so the port does not offer it. */
  takeTransaction(txnId: string): Promise<LoginTransaction | null>;

  close(): Promise<void>;
}
