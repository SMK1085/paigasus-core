// SPDX-License-Identifier: Apache-2.0
import { createClient } from 'redis';
import { GenericContainer, type StartedTestContainer } from 'testcontainers';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createRedisSessionStore } from '../../src/adapters/redis-store.js';
import { runStoreContract } from '../store-contract.js';

let container: StartedTestContainer;
let url: string;

// NO SKIP. If Docker is unreachable this FAILS. That is the whole reason the container suites
// live in their own Moon task: a task whose only purpose is container tests has no reason to
// skip, so there is no hatch, no canary, and no way to green a run that tested nothing.
beforeAll(async () => {
  container = await new GenericContainer('redis:8-alpine').withExposedPorts(6379).start();
  url = `redis://${container.getHost()}:${String(container.getMappedPort(6379))}/0`;
}, 120_000);

afterAll(async () => {
  await container.stop();
});

// The contract suite's fixed sids ('a', 't1') would collide between test files sharing one
// container process without a per-run prefix — this is the ONE authorized difference from the
// memory adapter's invocation. `Date.now()` ALONE is not unique: two files (or two `it` blocks
// in the same file, per makeStore's per-test invocation) starting in the same millisecond would
// share a prefix and collide on the fixed sids. `Math.random()` gives per-call uniqueness the
// same way the sibling single-flight container suites already do.
runStoreContract('redis', async () => createRedisSessionStore({ url, commandTimeoutMs: 1000, keyPrefix: `t${String(Date.now())}${String(Math.random())}:` }));

// Finding 7 (CodeRabbit fix round): an unparseable stored value used to surface as
// SessionStoreUnavailable, turning one poisoned key into a permanent sign-out loop for that
// user. It must instead be treated as absent-and-deleted, matching the version-mismatch handling
// right below it in redis-store.ts and the memory adapter's own behaviour. This writes the
// malformed value directly through a raw client — sessKey's format (`${keyPrefix}pgs:sess:${sid}`)
// is not exported, so it is reproduced here the same way tests/containers/single-flight-multiprocess.test.ts
// already does for its own raw-client assertions.
describe('a malformed stored record', () => {
  it('is treated as absent, not as a store failure', async () => {
    const keyPrefix = `m${String(Date.now())}${String(Math.random())}:`;
    const sid = 'poisoned';
    const raw = createClient({ url });
    await raw.connect();
    try {
      await raw.set(`${keyPrefix}pgs:sess:${sid}`, '{not valid json');

      const store = await createRedisSessionStore({ url, commandTimeoutMs: 1000, keyPrefix });
      try {
        expect(await store.get(sid)).toBeNull();
      } finally {
        await store.close();
      }

      // Absent-AND-DELETED, not merely reported absent: the poisoned key must not keep failing
      // this same way on every subsequent read.
      expect(await raw.get(`${keyPrefix}pgs:sess:${sid}`)).toBeNull();
    } finally {
      await raw.close();
    }
  });
});
