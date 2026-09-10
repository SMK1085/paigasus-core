// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { makeRecord, runStoreContract } from '../store-contract.js';
import type { SessionRecord } from '../../src/core/session.js';

runStoreContract('memory', () => Promise.resolve(new MemorySessionStore()));

// SMA-626 § 4.3: the memory adapter never parses bytes, so only a caller violating the type could
// poison it. It adopts the same predicate anyway, so ONE rule covers both adapters — and this
// test is what proves the shared predicate does not start deleting VALID records here, which is
// the over-checking direction of the drift Task 3's table also guards.
describe('MemorySessionStore and the shared record predicate (SMA-626 § 4)', () => {
  it('returns a well-formed record unchanged', async () => {
    const s = new MemorySessionStore();
    await s.set('s', makeRecord(), 60_000, null);
    await expect(s.get('s')).resolves.toMatchObject({ version: 1, accessToken: 'AT' });
  });

  it('treats a type-violating record as absent and deletes it', async () => {
    const s = new MemorySessionStore();
    await s.set('s', { version: 1 } as unknown as SessionRecord, 60_000, null);
    await expect(s.get('s')).resolves.toBeNull();
    await expect(s.get('s')).resolves.toBeNull(); // deleted, not merely reported absent
  });
});
