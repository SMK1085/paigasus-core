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
 * A client carrying the options createRedisDescriptorCache asserts. Mirrors the production shape
 * in @paigasus/auth's createRedisSessionStore.
 */
export async function connect(url: string): Promise<RedisClientType> {
  const client: RedisClientType = createClient({ url, disableOfflineQueue: true, commandOptions: { timeout: 5_000 }, socket: { socketTimeout: 10_000 } });
  // Never log the raw error: node-redis embeds the DSN in its connection errors.
  client.on('error', () => undefined);
  await client.connect();
  return client;
}

/** A fresh prefix per test, so one container serves the whole file without cross-test leakage. */
export function randomPrefix(): string {
  return `d${Date.now().toString(36)}${Math.random().toString(36).slice(2)}:`;
}
