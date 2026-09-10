// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { RECORD_VERSION, type CacheRecord } from '../src/core/record.js';
import { createMemoryDescriptorCache } from '../src/adapters/memory-cache.js';
import { runCacheContract } from './cache-contract.js';

// `Promise.resolve(...)`, not an `async` arrow: the factory is synchronous, and an async arrow
// with no `await` trips `@typescript-eslint/require-await`, which reds `moon ci :lint`.
runCacheContract('memory', () => Promise.resolve(createMemoryDescriptorCache()));

describe('memory adapter TTL', () => {
  it('treats an expired entry as absent, and as re-insertable', async () => {
    let now = 1_000;
    const cache = createMemoryDescriptorCache(() => now);
    const record: CacheRecord = {
      version: RECORD_VERSION,
      rev: 1,
      descriptor: { service: 'iam', version: '0.0.0', capabilities: ['iam.audit'] },
      descriptorAt: 0,
      outcome: 'ok',
      outcomeAt: 0,
      reason: null,
    };

    expect(await cache.set('iam', record, 5_000, null)).toBe(true);
    now = 5_999;
    expect(await cache.get('iam')).not.toBeNull();
    now = 6_001;
    expect(await cache.get('iam')).toBeNull();
    // An expired entry must be GONE, not merely masked: an insert-only set proves it.
    expect(await cache.set('iam', record, 5_000, null)).toBe(true);
  });
});
