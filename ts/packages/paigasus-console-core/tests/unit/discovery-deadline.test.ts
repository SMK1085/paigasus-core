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
const COOLDOWN_MS = TIMEOUT_MS * 4;

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

  it('U2c: the deadline is exactly 4 × the command timeout, not shorter', async () => {
    const { cache } = wrapped();
    const settled = cache.get('iam').then(
      () => 'resolved',
      () => 'rejected',
    );
    // One millisecond before the deadline the operation is still running. This is what fails if
    // DEADLINE_FACTOR is ever narrowed to 3 or less (spec D3).
    await vi.advanceTimersByTimeAsync(DEADLINE_MS - 1);
    await expect(Promise.race([settled, Promise.resolve('still running')])).resolves.toBe('still running');
    // The remaining millisecond fires it.
    await vi.advanceTimersByTimeAsync(1);
    await expect(settled).resolves.toBe('rejected');
  });

  it('U8: an inner rejection propagates unchanged, and the deadline timer was cleared', async () => {
    const { cache, inner } = wrapped();
    const boom = new Error('inner failed');
    const pending = cache.get('iam').catch((error: unknown) => error);
    inner.fail(boom);
    await expect(pending).resolves.toBe(boom);
    // The deadline timer must be gone once the operation settles. This is the only assertion that
    // actually fails if `clearTimeout` is removed from the `finally` — a stale timer rejects a race
    // that already settled, so it changes no observable behaviour, in this task or in Task 2.
    expect(vi.getTimerCount()).toBe(0);
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
    const { cache, inner, client } = wrapped();
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
      // SMA-650 Task 2: this operation just expired and opened the circuit (D4). Close it before the
      // next one so this test still proves each of the five methods reaches the inner cache, not the
      // circuit's own behaviour — that is covered separately in the "circuit" describe block below.
      client.emit('ready');
    }
    expect(inner.calls).toEqual(['get', 'set', 'delete', 'tryAcquireLock', 'releaseLock']);
    await expect(cache.close()).resolves.toBeUndefined();
    expect(inner.calls).toEqual(['get', 'set', 'delete', 'tryAcquireLock', 'releaseLock', 'close']);
  });
});

describe('withOperationDeadline — the circuit (D4, D5, D6, D8)', () => {
  /** Drives one operation to its deadline and returns once it has rejected. */
  async function expire(cache: DescriptorCache): Promise<void> {
    const pending = cache.get('iam').catch(() => undefined);
    await vi.advanceTimersByTimeAsync(DEADLINE_MS);
    await pending;
  }

  it('U3: after an expiry the next operation rejects at once with phase "circuit-open", and does not call the inner cache', async () => {
    const { cache, inner } = wrapped();
    await expire(cache);
    expect(inner.calls).toEqual(['get']);
    await expect(cache.get('iam')).rejects.toMatchObject({ name: 'DescriptorCacheTimeoutError', phase: 'circuit-open' });
    // Still one call: the refused operation never reached the inner cache.
    expect(inner.calls).toEqual(['get']);
  });

  it('U4: a ready event closes the circuit', async () => {
    const { cache, inner, client } = wrapped();
    await expire(cache);
    client.emit('ready');
    const pending = cache.get('iam');
    inner.settle(record());
    await expect(pending).resolves.toEqual(record());
    expect(inner.calls).toEqual(['get', 'get']);
  });

  it('U5: with no ready event, the circuit closes after the cooldown', async () => {
    const { cache, inner } = wrapped();
    await expire(cache);
    await vi.advanceTimersByTimeAsync(COOLDOWN_MS);
    const pending = cache.get('iam');
    inner.settle(record());
    await expect(pending).resolves.toEqual(record());
    expect(inner.calls).toEqual(['get', 'get']);
  });

  it('U5b: the circuit is still open one millisecond before the cooldown elapses', async () => {
    const { cache } = wrapped();
    await expire(cache);
    await vi.advanceTimersByTimeAsync(COOLDOWN_MS - 1);
    await expect(cache.get('iam')).rejects.toMatchObject({ phase: 'circuit-open' });
  });

  it('U6: one wedge logs one line, with the operation and the deadline', async () => {
    const { cache, log } = wrapped();
    await expire(cache);
    await expect(cache.get('iam')).rejects.toThrow();
    await expect(cache.delete('iam')).rejects.toThrow();
    expect(log.timeouts()).toEqual([{ operation: 'get', deadlineMs: DEADLINE_MS }]);
  });

  it('U7: three concurrent expiries open the circuit once, and the cooldown runs from the FIRST', async () => {
    const inner = fakeInner();
    const client = new EventEmitter();
    const log = sink();
    // A fake inner cache that parks EVERY call, so three can be in flight at once.
    const parked = { ...inner.cache, get: () => new Promise<never>(() => undefined) } as unknown as DescriptorCache;
    const cache = withOperationDeadline(parked, client, TIMEOUT_MS, log.log);
    const pending = [cache.get('iam').catch(() => undefined), cache.get('gateway').catch(() => undefined), cache.get('other').catch(() => undefined)];
    await vi.advanceTimersByTimeAsync(DEADLINE_MS);
    await Promise.all(pending);
    expect(log.timeouts()).toEqual([{ operation: 'get', deadlineMs: DEADLINE_MS }]);
    // The cooldown runs from the FIRST expiry, so it has elapsed after exactly COOLDOWN_MS more.
    await vi.advanceTimersByTimeAsync(COOLDOWN_MS);
    const after = cache.get('iam').catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(0);
    // It parked inside the inner cache rather than being refused, so nothing has settled yet.
    await expect(Promise.race([after, Promise.resolve('still running')])).resolves.toBe('still running');
  });

  it('U11: the wrapper registers a ready listener and no error listener', () => {
    const { client } = wrapped();
    expect(client.listenerCount('ready')).toBe(1);
    expect(client.listenerCount('error')).toBe(0);
  });
});
