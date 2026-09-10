// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { hashSecret, newLockToken, newSessionId, newTransactionId, newTransactionSecret, secretMatchesHash } from '../../src/core/ids.js';

describe('ids', () => {
  it('a session id is 32 bytes of base64url', () => {
    const id = newSessionId();
    expect(id).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(Buffer.from(id, 'base64url')).toHaveLength(32);
  });

  it('session ids do not repeat across 1000 draws', () => {
    expect(new Set(Array.from({ length: 1000 }, newSessionId)).size).toBe(1000);
  });

  it('lock tokens and transaction ids are distinct values', () => {
    expect(newLockToken()).not.toBe(newLockToken());
    expect(newTransactionId()).not.toBe(newTransactionId());
  });

  it('a secret matches only its own hash', () => {
    const s = newTransactionSecret();
    const h = hashSecret(s);
    expect(secretMatchesHash(s, h)).toBe(true);
    expect(secretMatchesHash(newTransactionSecret(), h)).toBe(false);
  });

  it('a hash comparison against a wrong-length input is false, not a throw', () => {
    // timingSafeEqual throws on a length mismatch; the wrapper must not propagate that.
    expect(secretMatchesHash('short', hashSecret(newTransactionSecret()))).toBe(false);
    expect(secretMatchesHash(newTransactionSecret(), 'not-a-hash')).toBe(false);
  });
});
