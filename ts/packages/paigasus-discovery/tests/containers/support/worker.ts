// SPDX-License-Identifier: Apache-2.0
//
// Runs in a FORKED process. Two console zones are two processes, not two async callers sharing
// one event loop — an in-process test cannot distinguish a real distributed lock from the event
// loop merely serializing everything.
import { createRedisDescriptorCache } from '../../../src/adapters/redis-cache.js';
import { noopLogger } from '../../../src/adapters/noop-logger.js';
import { DEFAULT_TIMINGS } from '../../../src/core/record.js';
import { resolveService } from '../../../src/core/single-flight.js';
import { connect } from './redis.js';

const WORKER_COUNT = 4;
const BARRIER_TIMEOUT_MS = 10_000;
const BARRIER_POLL_MS = 20;

/**
 * Rendezvous barrier so all `WORKER_COUNT` workers begin `resolveService` together. Without this,
 * `Promise.all(execFile(...))` starts the four child processes with no synchronisation between
 * their first cache reads — one worker can complete its whole probe-and-write before a slower
 * sibling has even read the (still-empty) cache, so that sibling finds a fresh record and never
 * probes at all. `probes === 1` would then pass even for an implementation with NO distributed
 * locking, exercising the single-flight path only by timing luck.
 *
 * Built on Redis itself, which every worker already connects to: `INCR` is atomic, so the count is
 * exact regardless of arrival order, and every worker polls the SAME key until it reads
 * `WORKER_COUNT`, which is what makes them start `resolveService` together rather than in
 * arrival order. Keyed under the test's random `prefix` so concurrent test runs cannot collide.
 */
async function awaitBarrier(client: Awaited<ReturnType<typeof connect>>, prefix: string): Promise<void> {
  const barrierKey = `${prefix}barrier`;
  await client.incr(barrierKey);
  const deadline = Date.now() + BARRIER_TIMEOUT_MS;
  for (;;) {
    const count = Number(await client.get(barrierKey));
    if (count >= WORKER_COUNT) return;
    if (Date.now() >= deadline) {
      throw new Error(`awaitBarrier: timed out waiting for ${WORKER_COUNT} workers, saw ${count}`);
    }
    await new Promise((r) => setTimeout(r, BARRIER_POLL_MS));
  }
}

async function main(): Promise<void> {
  const [url, prefix, probeMs] = process.argv.slice(2);
  const client = await connect(url as string);
  await awaitBarrier(client, prefix as string);
  let probes = 0;
  const state = await resolveService(
    {
      cache: createRedisDescriptorCache(client, { keyPrefix: prefix as string }),
      logger: noopLogger,
      timings: { ...DEFAULT_TIMINGS, lockWaitMs: 4_000, probeTimeoutMs: 3_000, lockTtlMs: 8_000 },
      now: Date.now,
      probe: async () => {
        probes += 1;
        await new Promise((r) => setTimeout(r, Number(probeMs)));
        return { ok: true, descriptor: { service: 'iam', version: '1.0.0', capabilities: ['iam.audit'] } };
      },
    },
    'iam',
    'tok',
  );
  process.stdout.write(JSON.stringify({ probes, state: state.state }));
  await client.quit();
}

void main();
