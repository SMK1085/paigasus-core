// SPDX-License-Identifier: Apache-2.0
//
// Forked by tests/containers/single-flight-multiprocess.test.ts as a SEPARATE node process — never
// imported directly. See tests/fixtures/ts-esm-loader.mjs for why it needs a custom `--import`.
//
// Simulates one console ZONE resolving a session that a second zone (the sibling worker process)
// resolves at the same time. AC 2 ("two concurrent requests hitting an expired token trigger
// exactly one refresh") only means something across PROCESSES: MemorySessionStore's lock is
// per-process by construction (see that adapter's own file header), so an in-process proof alone
// cannot show two zones behave correctly. This fixture is what makes the claim testable.
import { createClient } from 'redis';
import { noopLogger } from '../../src/adapters/noop-logger.js';
import { createRedisSessionStore } from '../../src/adapters/redis-store.js';
import { resolveSession } from '../../src/core/single-flight.js';

// F9: prefixed with keyPrefix, matching every other key this process touches, rather than a bare
// literal — a bare key would leak across concurrent test runs sharing one container.
const ARRIVE_KEY_SUFFIX = 'rendezvous:arrive';
const REFRESH_COUNTER_KEY_SUFFIX = 'refresh:count';
const RENDEZVOUS_POLL_MS = 10;
const RENDEZVOUS_DEADLINE_MS = 30_000;
const EXPECTED_WORKERS = 2;

/**
 * Thrown by `waitForRendezvous` when the sibling worker never arrives — a distinct, named
 * failure instead of a poll loop that hangs the test suite forever.
 */
class RendezvousTimeoutError extends Error {
  constructor(key: string, deadlineMs: number) {
    super(`waitForRendezvous timed out after ${deadlineMs}ms waiting on key "${key}"`);
    this.name = 'RendezvousTimeoutError';
  }
}

// Duck-typed, not `ReturnType<typeof createClient>`: node-redis's client type's concrete generic
// instantiation depends on the exact options object passed to `createClient` at the call site
// (see redis-store.ts's own comment on the same trap), so re-typing a fresh client value at a
// separate function boundary does not typecheck under this repo's exactOptionalPropertyTypes.
// `get` is the only method this helper needs.
interface RendezvousClient {
  get(key: string): Promise<string | null>;
}

/**
 * F4: block until both workers have arrived. Without this, one worker can start Node, transform
 * TypeScript, connect two Redis clients and build a store faster than the other, call
 * resolveSession alone, finish its whole 300ms hold, release, and be long gone before the slower
 * worker ever calls tryAcquireLock — which reads as "exactly one refresh" for a reason that has
 * nothing to do with the lock. The rendezvous forces both workers to call resolveSession only
 * after both are fully started, so a genuine race is what produces the outcome.
 */
async function waitForRendezvous(client: RendezvousClient, key: string): Promise<void> {
  const deadline = Date.now() + RENDEZVOUS_DEADLINE_MS;
  for (;;) {
    const value = await client.get(key);
    if (value !== null && Number(value) >= EXPECTED_WORKERS) return;
    if (Date.now() >= deadline) throw new RendezvousTimeoutError(key, RENDEZVOUS_DEADLINE_MS);
    await new Promise((r) => setTimeout(r, RENDEZVOUS_POLL_MS));
  }
}

async function main(): Promise<void> {
  const url = process.argv[2];
  const keyPrefix = process.argv[3];
  const sid = process.argv[4];
  if (url === undefined || keyPrefix === undefined || sid === undefined) {
    throw new Error('usage: refresh-worker.ts <redisUrl> <keyPrefix> <sid>');
  }

  const arriveKey = `${keyPrefix}${ARRIVE_KEY_SUFFIX}`;
  const counterKey = `${keyPrefix}${REFRESH_COUNTER_KEY_SUFFIX}`;

  // A separate raw client for the rendezvous and the shared counter: SessionStore is deliberately
  // primitives-only (see src/ports/session-store.ts's own comment) and exposes no arbitrary INCR.
  const counter = createClient({ url });
  await counter.connect();

  await counter.incr(arriveKey);
  await waitForRendezvous(counter, arriveKey);

  const store = await createRedisSessionStore({ url, commandTimeoutMs: 2000, keyPrefix });

  let ranRefresh = false;
  const refresh = async (): Promise<{ accessToken: string; refreshToken?: string; expiresIn: number }> => {
    ranRefresh = true;
    await counter.incr(counterKey);
    // Hold the lock long enough that the sibling process's concurrent call is guaranteed to still
    // be contending rather than finishing before this one even starts.
    await new Promise((r) => setTimeout(r, 300));
    return { accessToken: 'AT-REFRESHED', refreshToken: 'RT-REFRESHED', expiresIn: 300 };
  };

  const result = await resolveSession({ store, refresh, logger: noopLogger, skewMs: 30_000, lockTtlMs: 10_000, lockWaitMs: 5_000, ttlMs: 60_000 }, sid);

  // The parent reads this line and JSON.parses it — nothing else may write to stdout. `refreshed`
  // reports whether THIS process's own refresh function ran, which is what proves the loser
  // genuinely waited on the lock rather than merely arriving after the winner had already
  // finished (see runWorker's assertion on this field).
  process.stdout.write(`${JSON.stringify({ accessToken: result?.accessToken ?? null, refreshed: ranRefresh })}\n`);

  await store.close();
  await counter.close();
}

main().catch((err: unknown) => {
  console.error(err);
  process.exitCode = 1;
});
