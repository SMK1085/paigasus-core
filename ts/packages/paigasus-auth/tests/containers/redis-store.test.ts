// SPDX-License-Identifier: Apache-2.0
import { GenericContainer, type StartedTestContainer } from 'testcontainers';
import { afterAll, beforeAll } from 'vitest';
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
// memory adapter's invocation.
runStoreContract('redis', async () => createRedisSessionStore({ url, commandTimeoutMs: 1000, keyPrefix: `t${String(Date.now())}:` }));
