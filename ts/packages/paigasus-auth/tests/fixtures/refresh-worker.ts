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

const REFRESH_COUNTER_KEY = 'refresh:count';

async function main(): Promise<void> {
  const url = process.argv[2];
  const keyPrefix = process.argv[3];
  const sid = process.argv[4];
  if (url === undefined || keyPrefix === undefined || sid === undefined) {
    throw new Error('usage: refresh-worker.ts <redisUrl> <keyPrefix> <sid>');
  }

  // A separate raw client for the shared counter: SessionStore is deliberately primitives-only
  // (see src/ports/session-store.ts's own comment) and exposes no arbitrary INCR.
  const counter = createClient({ url });
  await counter.connect();

  const store = await createRedisSessionStore({ url, commandTimeoutMs: 2000, keyPrefix });

  const refresh = async (): Promise<{ accessToken: string; refreshToken?: string; expiresIn: number }> => {
    await counter.incr(REFRESH_COUNTER_KEY);
    // Hold the lock long enough that the sibling process's concurrent call is guaranteed to still
    // be contending rather than finishing before this one even starts.
    await new Promise((r) => setTimeout(r, 300));
    return { accessToken: 'AT-REFRESHED', refreshToken: 'RT-REFRESHED', expiresIn: 300 };
  };

  const result = await resolveSession({ store, refresh, logger: noopLogger, skewMs: 30_000, lockTtlMs: 10_000, lockWaitMs: 5_000, ttlMs: 60_000 }, sid);

  // The parent reads this line and JSON.parses it — nothing else may write to stdout.
  process.stdout.write(`${JSON.stringify({ accessToken: result?.accessToken ?? null })}\n`);

  await store.close();
  await counter.close();
}

main().catch((err: unknown) => {
  console.error(err);
  process.exitCode = 1;
});
