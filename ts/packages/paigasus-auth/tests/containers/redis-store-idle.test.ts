// SPDX-License-Identifier: Apache-2.0
//
// SMA-651 § 8.2. The session store against a REAL Redis that accepts commands and never replies.
//
// T (commandTimeoutMs) is 1000, so pingInterval is 1000 ms, socketTimeout is 2000 ms, the
// per-operation deadline is 4000 ms and the circuit's cooldown is 4000 ms (spec D1-D3).
//
// A SECOND client (`admin`), never the store's own, pauses the server and reads INFO stats. The
// Redis carries a password, so E4 can assert that no log line holds it.
//
// Errors are matched on `name` and `phase` STRINGS, never with instanceof, so this file runs
// against code that does not have SessionStoreTimeout yet (the red-first run, spec § 8.3).
//
// NO SKIP. If Docker is unreachable this FAILS, like every file in this directory.
import { randomBytes } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';
import { createClient, type RedisClientType } from 'redis';
import { GenericContainer, type StartedTestContainer } from 'testcontainers';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createRedisSessionStore } from '../../src/adapters/redis-store.js';
import type { SessionStore } from '../../src/ports/session-store.js';
import { makeRecord } from '../store-contract.js';

const TIMEOUT_MS = 1000;
const SOCKET_TIMEOUT_MS = TIMEOUT_MS * 2;
const DEADLINE_MS = TIMEOUT_MS * 4;
/** Strictly BELOW socketTimeout, so the driver's writes keep resetting the idle timer. */
const DRIVE_STEP_MS = 500;
const POLL_BUDGET_MS = 5_000;
const POLL_STEP_MS = 100;
const SECRET = `s3cret${randomBytes(8).toString('hex')}`;

let container: StartedTestContainer;
let url: string;
let admin: RedisClientType;

beforeAll(async () => {
  container = await new GenericContainer('redis:8-alpine').withCommand(['redis-server', '--requirepass', SECRET]).withExposedPorts(6379).start();
  url = `redis://:${SECRET}@${container.getHost()}:${String(container.getMappedPort(6379))}/0`;
  admin = createClient({ url });
  // Never log the raw error: node-redis embeds the DSN in its connection errors.
  admin.on('error', () => undefined);
  await admin.connect();
}, 120_000);

afterAll(async () => {
  if (admin?.isOpen) admin.destroy();
  await container?.stop();
});

type Sink = { lines: string[]; logger: { event(name: string, fields: Readonly<Record<string, unknown>>): void } };

function sink(): Sink {
  const lines: string[] = [];
  return { lines, logger: { event: (name, fields) => lines.push(JSON.stringify({ event: name, fields })) } };
}

/** Redis's own count of accepted connections. It rises by one for each reconnect of the store. */
async function connectionsReceived(): Promise<number> {
  const info = await admin.info('stats');
  const match = /total_connections_received:(\d+)/.exec(info);
  if (match === null) throw new Error('INFO stats has no total_connections_received');
  return Number(match[1]);
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
      throw new Error(`${what}: no success within ${String(POLL_BUDGET_MS)} ms; the last error was "${lastError instanceof Error ? lastError.name : 'not an Error'}"`);
    }
    await sleep(POLL_STEP_MS);
  }
}

/** A store with one session written, after a bounded wait for the first write. */
async function warmStore(s: Sink): Promise<{ store: SessionStore; sid: string; rec: ReturnType<typeof makeRecord> }> {
  const store = await createRedisSessionStore({ url, commandTimeoutMs: TIMEOUT_MS, keyPrefix: `idle${randomBytes(4).toString('hex')}:`, logger: s.logger });
  const sid = `sid-${randomBytes(4).toString('hex')}`;
  const rec = makeRecord();
  await eventually(() => store.set(sid, rec, 60_000, null), 'the first set');
  return { store, sid, rec };
}

/** Fire-and-forget reads every DRIVE_STEP_MS until `stop()`. Never awaits a read (spec E2). */
function drive(store: SessionStore, sid: string): { failures: unknown[]; stop: () => Promise<void> } {
  const failures: unknown[] = [];
  const issued: Array<Promise<void>> = [];
  let driving = true;
  const loop = (async () => {
    while (driving) {
      issued.push(
        store.get(sid).then(
          () => undefined,
          (error: unknown) => {
            failures.push(error);
          },
        ),
      );
      await sleep(DRIVE_STEP_MS);
    }
  })();
  return {
    failures,
    stop: async () => {
      driving = false;
      await loop;
      await Promise.all(issued);
    },
  };
}

function isTimeout(error: unknown, phase: 'deadline' | 'circuit-open'): boolean {
  return error instanceof Error && error.name === 'SessionStoreTimeout' && (error as Error & { phase?: string }).phase === phase;
}

describe('the session store against a Redis that never replies (SMA-651)', () => {
  it('E1: an idle client survives 3 x socketTimeout with no reconnect', async () => {
    const s = sink();
    const { store, sid, rec } = await warmStore(s);
    try {
      const before = await connectionsReceived();
      await sleep(3 * SOCKET_TIMEOUT_MS + 500);
      expect(await store.get(sid)).toEqual(rec);
      expect(await connectionsReceived()).toBe(before);
      expect(s.lines.filter((line) => line.includes('store.operation_timeout'))).toEqual([]);
    } finally {
      await store.close();
    }
  });

  it('E2 + E4: a hang under steady traffic is bounded by the deadline, and no line holds the password', async () => {
    const PAUSE_MS = 6_000;
    const s = sink();
    const { store, sid, rec } = await warmStore(s);
    try {
      const driver = drive(store, sid);
      await admin.clientPause(PAUSE_MS, 'ALL');
      // The first read issued inside the pause can go out one drive step late, so it expires at
      // DEADLINE_MS + DRIVE_STEP_MS. Wait one step more, and stay inside the pause.
      await sleep(DEADLINE_MS + 2 * DRIVE_STEP_MS);
      await driver.stop();

      // On TYPE, not on the clock: a node-redis teardown rejection is never a SessionStoreTimeout.
      expect(driver.failures.some((error) => isTimeout(error, 'deadline'))).toBe(true);

      await sleep(PAUSE_MS);
      expect(await eventually(() => store.get(sid), 'a read after the pause')).toEqual(rec);

      // E4. A guard, exempt from red-first: it must see at least one line, or the check below
      // proves nothing.
      expect(s.lines.some((line) => line.includes('store.operation_timeout'))).toBe(true);
      for (const line of s.lines) expect(line).not.toContain(SECRET);
    } finally {
      await store.close();
    }
  });

  it('E3: under traffic for a whole long pause, the circuit lets node-redis reconnect', async () => {
    // Above deadline (4T) + socketTimeout (2T) + one drive step, with a margin.
    const PAUSE_MS = 9_000;
    const s = sink();
    const { store, sid, rec } = await warmStore(s);
    try {
      const before = await connectionsReceived();
      const driver = drive(store, sid);
      await admin.clientPause(PAUSE_MS, 'ALL');
      await sleep(PAUSE_MS + 500);
      await driver.stop();
      expect(await eventually(() => store.get(sid), 'a read after the pause')).toEqual(rec);
      // Our own traffic resets the idle timer, so only the circuit's silence lets it fire. A
      // reconnect is a new accepted connection.
      expect(await connectionsReceived()).toBeGreaterThan(before);
    } finally {
      await store.close();
    }
  });

  it('E5: close() with a command in flight under a pause resolves at once and does not throw', async () => {
    const PAUSE_MS = 3_000;
    const s = sink();
    const { store, sid } = await warmStore(s);
    await admin.clientPause(PAUSE_MS, 'ALL');
    const inFlight = store.get(sid).then(
      () => 'resolved',
      () => 'rejected',
    );
    await sleep(200);
    const closed = await Promise.race([store.close().then(() => 'closed'), sleep(1_000).then(() => 'still waiting')]);
    expect(closed).toBe('closed');
    expect(await inFlight).toBe('rejected');
    await sleep(PAUSE_MS);
  });
});
