// SPDX-License-Identifier: Apache-2.0
//
// SMA-648 § 5.1: the descriptor cache's Redis client must survive an idle socket. This drives the
// REAL call site, descriptorCacheFor, against a real Redis, because the defect lived in the client
// options that call site passes to node-redis.
//
// PAIGASUS_SESSION_REDIS_TIMEOUT_MS is 1000, so `socketTimeout` is 2000 ms and `pingInterval` is
// 1000 ms. Every poll is bounded by POLL_BUDGET_MS. Each test has its own log sink, so a late event
// from the previous test's destroy() cannot land in the next test's lines.
//
// The Redis carries a password (`--requirepass`), so every URL in this file holds a secret, and T5
// can assert that no log line ever contains it.
//
// Log lines are read by the EVENT NAME STRING, not through the AppEventName type. So this file
// compiles and runs against code that does not have `discovery.redis_connection_lost` yet, which
// the red-first run (spec § 5.1) needs.
import { randomBytes } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';
import { GenericContainer, type StartedTestContainer } from 'testcontainers';
import { createClient, type RedisClientType } from 'redis';
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest';
import type { DescriptorCache } from '@paigasus/discovery/server';
import type { ConsoleCoreConfig } from '../../src/config-shape';
import { DescriptorCacheTimeoutError, descriptorCacheFor, resetDiscoveryForTest } from '../../src/discovery';
import { createJsonLogger, type ConsoleLogger } from '../../src/logger';

const TIMEOUT_MS = 1000;
const SOCKET_TIMEOUT_MS = TIMEOUT_MS * 2;
/** Three times socketTimeout. */
const IDLE_MS = 3 * SOCKET_TIMEOUT_MS;
/** More than pingInterval + 2 × socketTimeout = 5000 ms, so the idle timer MUST fire during it. */
const PAUSE_MS = 6_000;
const POLL_BUDGET_MS = 5_000;
const POLL_STEP_MS = 100;
/** SMA-650 D3: 4 × the command timeout. */
const DEADLINE_MS = TIMEOUT_MS * 4;
/** Strictly BELOW socketTimeout, so the driver's writes keep resetting the idle timer. */
const DRIVE_STEP_MS = 500;
const LOST_EVENT = 'discovery.redis_connection_lost';
const SECRET = `s3cret${randomBytes(8).toString('hex')}`;

type CacheRecord = Parameters<DescriptorCache['set']>[1];
type LogLine = { readonly event: string; readonly fields: Readonly<Record<string, unknown>> };

let container: StartedTestContainer;
let url: string;
let admin: RedisClientType;

beforeAll(async () => {
  container = await new GenericContainer('redis:8-alpine').withCommand(['redis-server', '--requirepass', SECRET]).withExposedPorts(6379).start();
  url = `redis://:${SECRET}@${container.getHost()}:${container.getMappedPort(6379)}`;
  // A SECOND client, never the cache's own: it pauses the server and kills the cache's connection.
  admin = createClient({ url });
  // Never log the raw error: node-redis embeds the DSN in its connection errors.
  admin.on('error', () => undefined);
  await admin.connect();
});

afterEach(() => {
  resetDiscoveryForTest();
});

afterAll(async () => {
  if (admin?.isOpen) admin.destroy();
  await container?.stop();
});

function redisConfig(): ConsoleCoreConfig {
  return { PAIGASUS_SESSION_STORE: 'redis', PAIGASUS_SESSION_REDIS_URL: url, PAIGASUS_SESSION_REDIS_TIMEOUT_MS: TIMEOUT_MS } as ConsoleCoreConfig;
}

function record(): CacheRecord {
  return { version: 1, rev: 1, descriptor: { service: 'iam', version: '1.0.0', capabilities: ['iam.audit'] }, descriptorAt: 0, outcome: 'ok', outcomeAt: 0, reason: null };
}

/** One sink per test. `lost()` returns the fields of every connection-lost line, in order. */
function logSink(): { log: ConsoleLogger; lines: string[]; lost: () => Array<LogLine['fields']> } {
  const lines: string[] = [];
  const log = createJsonLogger((line) => {
    lines.push(line);
  });
  const lost = (): Array<LogLine['fields']> =>
    lines
      .map((line) => JSON.parse(line) as LogLine)
      .filter((line) => line.event === LOST_EVENT)
      .map((line) => line.fields);
  return { log, lines, lost };
}

/** Retries `attempt` every POLL_STEP_MS until it resolves, for at most POLL_BUDGET_MS. */
async function eventually<T>(attempt: () => Promise<T>, what: string): Promise<T> {
  const deadline = Date.now() + POLL_BUDGET_MS;
  let lastError: unknown;
  for (;;) {
    try {
      return await attempt();
    } catch (error) {
      lastError = error;
    }
    if (Date.now() >= deadline) {
      throw new Error(`${what}: no success within ${POLL_BUDGET_MS} ms; the last error was "${lastError instanceof Error ? lastError.message : 'not an Error'}"`);
    }
    await sleep(POLL_STEP_MS);
  }
}

/** Makes the process cache and waits (bounded) until a first write succeeds, before any idle gap. */
async function warmCache(log: ConsoleLogger): Promise<{ cache: DescriptorCache; service: string }> {
  const cache = descriptorCacheFor(redisConfig(), log);
  const service = `idle-${randomBytes(4).toString('hex')}`;
  await eventually(() => cache.set(service, record(), 60_000, null), 'the first set');
  return { cache, service };
}

/** Kills every normal connection except the admin client's own, found by `CLIENT ID`. */
async function killCacheConnections(): Promise<number> {
  const adminId = await admin.clientId();
  const others = (await admin.clientList({ TYPE: 'NORMAL' })).filter((client) => client.id !== adminId);
  for (const client of others) {
    await admin.clientKill({ filter: 'ID', id: client.id });
  }
  return others.length;
}

describe('the descriptor cache Redis client (SMA-648)', () => {
  it('T1: a read after an idle gap past socketTimeout returns the record, and no loss is logged', async () => {
    const sink = logSink();
    const { cache, service } = await warmCache(sink.log);
    await sleep(IDLE_MS);
    expect(await cache.get(service)).toEqual(record());
    expect(sink.lost()).toEqual([]);
  });

  it('T2: a read after each of two idle gaps returns the record', async () => {
    const sink = logSink();
    const { cache, service } = await warmCache(sink.log);
    await sleep(IDLE_MS);
    expect(await cache.get(service)).toEqual(record());
    await sleep(IDLE_MS);
    expect(await cache.get(service)).toEqual(record());
    expect(sink.lost()).toEqual([]);
  });

  it('T3: a real hang (CLIENT PAUSE) fires the idle timer, the client recovers, and one socket_timeout loss is logged', async () => {
    const sink = logSink();
    const { cache, service } = await warmCache(sink.log);
    // The cache's next PING is in flight with no reply, and nothing else writes to its socket, so
    // the idle timer fires. Nothing in this test touches the cache until the pause is over.
    await admin.clientPause(PAUSE_MS, 'ALL');
    await sleep(PAUSE_MS + 200);
    expect(await eventually(() => cache.get(service), 'a read after the pause')).toEqual(record());
    expect(sink.lost()).toEqual([{ reason: 'socket_timeout' }]);
  });

  it('T4: a server-side close (CLIENT KILL) recovers, and one socket_closed loss is logged', async () => {
    const sink = logSink();
    const { cache, service } = await warmCache(sink.log);
    expect(await killCacheConnections()).toBeGreaterThan(0);
    expect(await eventually(() => cache.get(service), 'a read after the server-side close')).toEqual(record());
    expect(sink.lost()).toEqual([{ reason: 'socket_closed' }]);
  });

  it('T5: no log line holds the password from the DSN', async () => {
    const sink = logSink();
    const { cache, service } = await warmCache(sink.log);
    expect(await killCacheConnections()).toBeGreaterThan(0);
    await eventually(() => cache.get(service), 'a read after the server-side close');
    // A guard, exempt from red-first: it must see at least one line, or it proves nothing.
    expect(sink.lost().length).toBeGreaterThan(0);
    for (const line of sink.lines) expect(line).not.toContain(SECRET);
  });

  it('T6: a hang under STEADY TRAFFIC is bounded by the operation deadline (SMA-650)', async () => {
    const sink = logSink();
    const { cache, service } = await warmCache(sink.log);
    const failures: unknown[] = [];
    const issued: Array<Promise<void>> = [];
    let driving = true;

    // FIRE AND FORGET, never awaited, at an interval strictly below socketTimeout. Awaiting each
    // operation would stop the traffic, the idle timer would fire, and node-redis would reject the
    // command by itself — which is T3's path, and it passes WITHOUT this feature. Keeping several
    // operations in flight is the whole point of this test.
    const driver = (async () => {
      while (driving) {
        issued.push(
          cache.get(service).then(
            () => undefined,
            (error: unknown) => {
              failures.push(error);
            },
          ),
        );
        await sleep(DRIVE_STEP_MS);
      }
    })();

    await admin.clientPause(PAUSE_MS, 'ALL');
    // The first operation issued INSIDE the pause can go out as late as one drive step in, so it
    // expires at DEADLINE_MS + DRIVE_STEP_MS. Wait one more drive step than that, so the assertion
    // is not made exactly on the boundary — and stay inside PAUSE_MS, so the server is still paused.
    await sleep(DEADLINE_MS + 2 * DRIVE_STEP_MS);
    driving = false;
    await driver;
    await Promise.all(issued);

    // The assertion is on the TYPE, not on the clock. A node-redis teardown rejection can never be
    // a DescriptorCacheTimeoutError, so this discriminates the new path from T3's idle-timer path
    // exactly, with no wall-clock window to go flaky on a loaded CI runner.
    const deadlineFailures = failures.filter((error) => error instanceof DescriptorCacheTimeoutError && error.phase === 'deadline');
    expect(deadlineFailures.length).toBeGreaterThan(0);

    // The cache recovers once the pause ends.
    await sleep(PAUSE_MS);
    expect(await eventually(() => cache.get(service), 'a read after the paused-with-traffic window')).toEqual(record());

    // This is the ONE place discovery.redis_operation_timeout is emitted against a real client
    // whose DSN carries a password, so the no-secret-in-logs claim for that event rests on this
    // assertion, not only on reading the code. A guard, exempt from red-first like T5's own: it
    // must see at least one such line, or the secret check below proves nothing.
    expect(sink.lines.some((line) => line.includes('discovery.redis_operation_timeout'))).toBe(true);
    for (const line of sink.lines) expect(line).not.toContain(SECRET);
  });
});
