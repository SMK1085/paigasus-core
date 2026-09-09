// SPDX-License-Identifier: Apache-2.0
import { createHash, randomBytes, timingSafeEqual } from 'node:crypto';

const bytes = (n: number): string => randomBytes(n).toString('base64url');

/** Opaque, 32 bytes. This is the only thing the browser ever holds. */
export const newSessionId = (): string => bytes(32);

/** Unique per acquisition, so compare-and-delete cannot release someone else's lock. */
export const newLockToken = (): string => bytes(16);

/** Short id for the per-transaction cookie name and the `state` parameter. */
export const newTransactionId = (): string => bytes(9);

/** The browser-bound secret. Only its SHA-256 is stored server-side. */
export const newTransactionSecret = (): string => bytes(32);

export const hashSecret = (secret: string): string => createHash('sha256').update(secret).digest('base64url');

/**
 * Constant-time comparison. The realistic exposure is low — an attacker in the login-CSRF
 * scenario already holds their own secret — but timingSafeEqual costs nothing and removes the
 * argument entirely. It THROWS on a length mismatch, so the lengths are checked first and a
 * mismatch returns false rather than propagating.
 */
export function secretMatchesHash(secret: string, expectedHash: string): boolean {
  const actual = Buffer.from(hashSecret(secret), 'base64url');
  const expected = Buffer.from(expectedHash, 'base64url');
  if (actual.length !== expected.length) return false;
  return timingSafeEqual(actual, expected);
}
