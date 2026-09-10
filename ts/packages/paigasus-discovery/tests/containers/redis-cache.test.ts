// SPDX-License-Identifier: Apache-2.0
import { afterAll, beforeAll } from 'vitest';
import type { RedisClientType } from 'redis';
import { createRedisDescriptorCache } from '../../src/adapters/redis-cache.js';
import { runCacheContract } from '../cache-contract.js';
import { connect, randomPrefix, startRedis, type RedisFixture } from './support/redis.js';

let fixture: RedisFixture;
let client: RedisClientType;

beforeAll(async () => {
  fixture = await startRedis();
  client = await connect(fixture.url);
});

afterAll(async () => {
  await client?.quit();
  await fixture?.container.stop();
});

// The SAME contract the memory adapter runs. A semantic divergence — a lock that is not atomic,
// a fence that does not fence, a foreign schema version that is served rather than deleted —
// fails here rather than in production.
runCacheContract('redis', () => Promise.resolve(createRedisDescriptorCache(client, { keyPrefix: randomPrefix() })));
