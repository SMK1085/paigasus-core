// SPDX-License-Identifier: Apache-2.0
import { execFile } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { promisify } from 'node:util';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { randomPrefix, startRedis, type RedisFixture } from './support/redis.js';

const run = promisify(execFile);
const worker = fileURLToPath(new URL('./support/worker.ts', import.meta.url));
// See tests/containers/support/ts-esm-loader.mjs: a bare `node` process needs this to resolve
// this package's `.js`-suffixed relative imports onto their `.ts` source siblings. Node 24.16.0
// strips TypeScript types natively with no flag, so only the resolution loader is needed here —
// no `--experimental-strip-types` / `--experimental-transform-types` flag.
const loaderUrl = pathToFileURL(fileURLToPath(new URL('./support/ts-esm-loader.mjs', import.meta.url))).href;

let fixture: RedisFixture;

beforeAll(async () => {
  fixture = await startRedis();
});

afterAll(async () => {
  await fixture?.container.stop();
});

describe('AC3: single-flight across PROCESSES', () => {
  it('four independent processes trigger exactly one probe', async () => {
    // This is the half a fake cannot prove: that SET NX PX and the compare-and-delete release are
    // genuinely atomic in Redis, rather than merely appearing so because one event loop
    // serialized every caller.
    //
    // Each worker (support/worker.ts) awaits a Redis-backed rendezvous barrier before its first
    // `resolveService` call. Without it, `Promise.all(execFile(...))` gives no guarantee the four
    // processes' first cache reads overlap: a fast worker could finish its whole probe-and-write
    // before a slow sibling even reads the (still-empty) cache, so the sibling finds a fresh
    // record and never probes — `probes === 1` would then pass by timing luck even without a real
    // distributed lock.
    //
    const prefix = randomPrefix();
    const results = await Promise.all(Array.from({ length: 4 }, () => run(process.execPath, ['--import', loaderUrl, worker, fixture.url, prefix, '600'])));
    const parsed = results.map((r) => JSON.parse(r.stdout) as { probes: number; state: string });
    expect(parsed.reduce((sum, p) => sum + p.probes, 0)).toBe(1);
    for (const p of parsed) expect(p.state).toBe('available');
  });
});
