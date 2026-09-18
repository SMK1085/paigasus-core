// SPDX-License-Identifier: Apache-2.0
//
// Reads the Redis client that src/discovery.ts holds on globalThis (SMA-648 § 5.2). The key string
// is the one src/discovery.ts declares. Symbol.for resolves it through the process-wide registry,
// so this helper needs no export from src/.
import type { RedisClientType } from 'redis';

const DISCOVERY_STATE = Symbol.for('paigasus.console-core.discovery-state.v1');

export function redisClientFromState(): RedisClientType | undefined {
  const holder = globalThis as typeof globalThis & { [DISCOVERY_STATE]?: { redisClient?: RedisClientType } };
  return holder[DISCOVERY_STATE]?.redisClient;
}
