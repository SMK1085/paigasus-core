// SPDX-License-Identifier: Apache-2.0
//
// SMA-626 § 4. The parse guard in redis-store.ts's `get` is defeated by three stored values, each
// of which restores the failure it was written to close. These tests are written BEFORE the fix,
// and each one is a measurement: if it passes against unmodified code, the hole is not real and
// the spec is wrong.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { makeRecord } from '../store-contract.js';

const state = { value: null as string | null, deleted: [] as string[] };

vi.mock('redis', () => ({
  createClient: () => ({
    connect: () => Promise.resolve(),
    on: () => undefined,
    get: () => Promise.resolve(state.value),
    del: (key: string) => {
      state.deleted.push(key);
      state.value = null;
      return Promise.resolve(1);
    },
    set: () => Promise.resolve('OK'),
    eval: () => Promise.resolve(1),
    close: () => Promise.resolve(),
  }),
}));

const { createRedisSessionStore } = await import('../../src/adapters/redis-store.js');

async function store() {
  return createRedisSessionStore({ url: 'redis://127.0.0.1:6379', commandTimeoutMs: 1000, keyPrefix: '' });
}

beforeEach(() => {
  state.value = null;
  state.deleted = [];
});

describe('a poisoned session record is absent-and-deleted, never a store failure', () => {
  it('accepts a well-formed record (the positive control)', async () => {
    state.value = JSON.stringify(makeRecord());
    await expect((await store()).get('s')).resolves.toMatchObject({ version: 1, accessToken: 'AT' });
    expect(state.deleted).toEqual([]);
  });

  // HOLE 1. JSON.parse('null') SUCCEEDS, so the parse guard never fires. Before the fix, reading
  // `.version` off null throws a TypeError that #guarded turns into SessionStoreUnavailable — a
  // store-outage signal raised against a perfectly healthy Redis.
  it('treats a stored literal null as absent and deletes it', async () => {
    state.value = 'null';
    await expect((await store()).get('s')).resolves.toBeNull();
    expect(state.deleted).toEqual(['pgs:sess:s']);
  });

  // HOLE 2. Parses fine, version is 1, everything else is missing. Before the fix this is
  // returned as a LIVE session carrying accessToken: undefined.
  it('treats a { version: 1 } body as absent and deletes it', async () => {
    state.value = '{"version":1}';
    await expect((await store()).get('s')).resolves.toBeNull();
    expect(state.deleted).toEqual(['pgs:sess:s']);
  });

  // HOLE 3. The one with a security consequence: can() fails OPEN on grantsAvailable
  // (src/client.ts:67), so this record makes every browser-side capability check return true.
  it('treats a principal shaped { roleGrants: [] } as absent and deletes it', async () => {
    state.value = JSON.stringify({ ...makeRecord(), principal: { roleGrants: [] } });
    await expect((await store()).get('s')).resolves.toBeNull();
    expect(state.deleted).toEqual(['pgs:sess:s']);
  });

  it('still treats unparseable JSON as absent and deletes it', async () => {
    state.value = '{not valid json';
    await expect((await store()).get('s')).resolves.toBeNull();
    expect(state.deleted).toEqual(['pgs:sess:s']);
  });

  it('still treats a version mismatch as absent and deletes it', async () => {
    state.value = JSON.stringify({ ...makeRecord(), version: 2 });
    await expect((await store()).get('s')).resolves.toBeNull();
    expect(state.deleted).toEqual(['pgs:sess:s']);
  });
});
