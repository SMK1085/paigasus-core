// SPDX-License-Identifier: Apache-2.0
//
// SMA-650 § 10.1, the default tier (no Docker): the per-operation deadline and the circuit, driven
// against a fake inner cache and a fake ready source.
//
// `toFake` is passed EXPLICITLY. Relying on vitest's default set would make this file depend on a
// default that can change under it (CLAUDE.md, SMA-502). `Date` is in the list because the circuit
// compares `Date.now()` against its cooldown.
import { EventEmitter } from 'node:events';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DescriptorCache } from '@paigasus/discovery/server';
import { DescriptorCacheTimeoutError, withOperationDeadline } from '../../src/discovery';
import { createJsonLogger, type ConsoleLogger } from '../../src/logger';

const TIMEOUT_MS = 1000;
const DEADLINE_MS = TIMEOUT_MS * 4;

type CacheRecord = Parameters<DescriptorCache['set']>[1];

function record(): CacheRecord {
  return { version: 1, rev: 1, descriptor: { service: 'iam', version: '1.0.0', capabilities: [] }, descriptorAt: 0, outcome: 'ok', outcomeAt: 0, reason: null };
}

/** A fake inner cache. Every method parks on a promise the test resolves, and counts its calls. */
function fakeInner(): { cache: DescriptorCache; calls: string[]; settle: (value: unknown) => void; fail: (error: unknown) => void } {
  const calls: string[] = [];
  let resolveNext: ((value: unknown) => void) | undefined;
  let rejectNext: ((error: unknown) => void) | undefined;
  const park = (name: string) => {
    calls.push(name);
    return new Promise<never>((resolve, reject) => {
      resolveNext = resolve as (value: unknown) => void;
      rejectNext = reject;
    });
  };
  const cache = {
    get: () => park('get'),
    set: () => park('set'),
    delete: () => park('delete'),
    tryAcquireLock: () => park('tryAcquireLock'),
    releaseLock: () => park('releaseLock'),
    close: () => {
      calls.push('close');
      return Promise.resolve();
    },
  } as unknown as DescriptorCache;
  return { cache, calls, settle: (value) => resolveNext?.(value), fail: (error) => rejectNext?.(error) };
}

function sink(): { log: ConsoleLogger; lines: string[]; timeouts: () => unknown[] } {
  const lines: string[] = [];
  const log = createJsonLogger((line) => {
    lines.push(line);
  });
  const timeouts = (): unknown[] =>
    lines
      .map((line) => JSON.parse(line) as { event: string; fields: unknown })
      .filter((line) => line.event === 'discovery.redis_operation_timeout')
      .map((line) => line.fields);
  return { log, lines, timeouts };
}

function wrapped(): { cache: DescriptorCache; inner: ReturnType<typeof fakeInner>; client: EventEmitter; log: ReturnType<typeof sink> } {
  const inner = fakeInner();
  const client = new EventEmitter();
  const log = sink();
  const cache = withOperationDeadline(inner.cache, client, TIMEOUT_MS, log.log);
  return { cache, inner, client, log };
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('withOperationDeadline — the deadline (D3, D7, D9, D10)', () => {
  it('U1: an operation that settles inside the deadline returns its value, and calls the inner cache once', async () => {
    const { cache, inner } = wrapped();
    const pending = cache.get('iam');
    inner.settle(record());
    await expect(pending).resolves.toEqual(record());
    expect(inner.calls).toEqual(['get']);
  });

  it('U2: an operation that never settles rejects at the deadline with phase "deadline"', async () => {
    const { cache } = wrapped();
    const pending = cache.get('iam');
    const assertion = expect(pending).rejects.toMatchObject({ name: 'DescriptorCacheTimeoutError', phase: 'deadline' });
    await vi.advanceTimersByTimeAsync(DEADLINE_MS);
    await assertion;
  });

  it('U2b: the rejection is a DescriptorCacheTimeoutError and its message holds no URL', async () => {
    const { cache } = wrapped();
    const pending = cache.get('iam').catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(DEADLINE_MS);
    const error = await pending;
    expect(error).toBeInstanceOf(DescriptorCacheTimeoutError);
    expect((error as Error).message).toContain('get');
    expect((error as Error).message).not.toContain('redis://');
  });

  it('U8: an inner rejection propagates unchanged, and the deadline timer was cleared', async () => {
    const { cache, inner, log } = wrapped();
    const boom = new Error('inner failed');
    const pending = cache.get('iam').catch((error: unknown) => error);
    inner.fail(boom);
    await expect(pending).resolves.toBe(boom);
    // If the timer had not been cleared it would open the circuit here.
    await vi.advanceTimersByTimeAsync(DEADLINE_MS);
    expect(log.timeouts()).toEqual([]);
  });

  it('U9: a late rejection from the abandoned operation is not an unhandled rejection', async () => {
    const { cache, inner } = wrapped();
    const seen: unknown[] = [];
    const onUnhandled = (reason: unknown): void => {
      seen.push(reason);
    };
    process.on('unhandledRejection', onUnhandled);
    try {
      const pending = cache.get('iam').catch(() => undefined);
      await vi.advanceTimersByTimeAsync(DEADLINE_MS);
      await pending;
      inner.fail(new Error('the socket died long after we gave up'));
      // Give the microtask queue a turn, which is when an unhandled rejection would be reported.
      await new Promise((resolve) => setImmediate(resolve));
    } finally {
      process.off('unhandledRejection', onUnhandled);
    }
    expect(seen).toEqual([]);
  });

  it('U10: all five operations are wrapped, and close() passes through unwrapped', async () => {
    const { cache, inner } = wrapped();
    for (const run of [
      () => cache.get('iam'),
      () => cache.set('iam', record(), 1000, null),
      () => cache.delete('iam'),
      () => cache.tryAcquireLock('iam', 't', 1000),
      () => cache.releaseLock('iam', 't'),
    ]) {
      const pending = run().catch(() => undefined);
      await vi.advanceTimersByTimeAsync(DEADLINE_MS);
      await pending;
    }
    expect(inner.calls).toEqual(['get', 'set', 'delete', 'tryAcquireLock', 'releaseLock']);
    await expect(cache.close()).resolves.toBeUndefined();
    expect(inner.calls).toEqual(['get', 'set', 'delete', 'tryAcquireLock', 'releaseLock', 'close']);
  });
});
