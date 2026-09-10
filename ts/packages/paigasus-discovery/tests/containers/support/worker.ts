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

async function main(): Promise<void> {
  const [url, prefix, probeMs] = process.argv.slice(2);
  const client = await connect(url as string);
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
