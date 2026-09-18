// SPDX-License-Identifier: Apache-2.0
import { GenericContainer, type StartedTestContainer } from 'testcontainers';
import { createClient, type RedisClientType } from 'redis';

export type RedisFixture = {
  readonly url: string;
  readonly container: StartedTestContainer;
};

export async function startRedis(): Promise<RedisFixture> {
  const container = await new GenericContainer('redis:8-alpine').withExposedPorts(6379).start();
  return { url: `redis://${container.getHost()}:${container.getMappedPort(6379)}`, container };
}

/**
 * A client carrying the six options createRedisDescriptorCache asserts. `pingInterval` is half of
 * `socketTimeout`, the largest value the precondition allows, and the reconnect strategy returns a
 * delay for every cause, a socket timeout included (SMA-648 D4).
 */
export async function connect(url: string): Promise<RedisClientType> {
  const client: RedisClientType = createClient({
    url,
    disableOfflineQueue: true,
    commandOptions: { timeout: 5_000 },
    pingInterval: 5_000,
    socket: { socketTimeout: 10_000, reconnectStrategy: (retries: number) => Math.min(retries * 100, 2_000) },
  });
  // Never log the raw error: node-redis embeds the DSN in its connection errors.
  client.on('error', () => undefined);
  await client.connect();
  return client;
}

/** A fresh prefix per test, so one container serves the whole file without cross-test leakage. */
export function randomPrefix(): string {
  return `d${Date.now().toString(36)}${Math.random().toString(36).slice(2)}:`;
}
