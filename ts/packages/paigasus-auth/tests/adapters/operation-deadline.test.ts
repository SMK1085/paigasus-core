// SPDX-License-Identifier: Apache-2.0
//
// SMA-651 § 8.1, U1-U12. The deadline decorator, driven with a fake inner store and a plain
// EventEmitter, so no redis mock is needed. Fake timers fake ONLY setTimeout, clearTimeout and
// performance: the circuit reads performance.now() (spec D3).
import { EventEmitter } from 'node:events';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { withOperationDeadline } from '../../src/adapters/operation-deadline.js';
import { SessionStoreTimeout, SessionStoreUnavailable } from '../../src/core/errors.js';
import type { AuthEventFields, AuthEventName } from '../../src/ports/logger.js';
import type { SessionStore } from '../../src/ports/session-store.js';
import { makeRecord } from '../store-contract.js';

const T = 1000;
const DEADLINE = 4 * T;
const COOLDOWN = 4 * T;

type Mode = 'hang' | 'ok' | 'reject';

function fakeInner() {
  const calls: string[] = [];
  const state = { mode: 'ok' as Mode };
  const answer = <V>(name: string, value: V): Promise<V> => {
    calls.push(name);
    if (state.mode === 'hang') return new Promise<V>(() => undefined);
    if (state.mode === 'reject') return Promise.reject(new SessionStoreUnavailable('session store unavailable (redis://<redacted>)'));
    return Promise.resolve(value);
  };
  const store: SessionStore = {
    get: () => answer('get', null),
    set: () => answer('set', true),
    delete: () => answer('delete', undefined),
    tryAcquireLock: () => answer('tryAcquireLock', true),
    releaseLock: () => answer('releaseLock', undefined),
    putTransaction: () => answer('putTransaction', undefined),
    takeTransaction: () => answer('takeTransaction', null),
    close: () => answer('close', undefined),
  };
  return { store, calls, state };
}

function fakeLogger() {
  const events: Array<[AuthEventName, AuthEventFields]> = [];
  return { events, logger: { event: (name: AuthEventName, fields: AuthEventFields) => events.push([name, fields]) } };
}

function setup() {
  const inner = fakeInner();
  const client = new EventEmitter();
  const log = fakeLogger();
  const store = withOperationDeadline(inner.store, client, T, log.logger);
  return { inner, client, log, store };
}

/** Settles `p` into a tagged value so a test can check "still pending" without an unhandled rejection. */
function track<V>(p: Promise<V>) {
  const box: { settled: boolean; value?: V; error?: unknown } = { settled: false };
  p.then(
    (value) => {
      box.settled = true;
      box.value = value;
    },
    (error: unknown) => {
      box.settled = true;
      box.error = error;
    },
  );
  return box;
}

/** Opens the circuit: one call hangs past the deadline. */
async function openCircuit(s: ReturnType<typeof setup>) {
  s.inner.state.mode = 'hang';
  const first = track(s.store.get('a'));
  await vi.advanceTimersByTimeAsync(DEADLINE);
  expect(first.error).toBeInstanceOf(SessionStoreTimeout);
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('withOperationDeadline (SMA-651)', () => {
  it('U1: a call that never settles is pending at 4T - 1 and rejects with phase deadline at 4T', async () => {
    const s = setup();
    s.inner.state.mode = 'hang';
    const box = track(s.store.get('a'));
    await vi.advanceTimersByTimeAsync(DEADLINE - 1);
    expect(box.settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(box.error).toBeInstanceOf(SessionStoreTimeout);
    expect((box.error as SessionStoreTimeout).phase).toBe('deadline');
  });

  it('U2: the timeout is a SessionStoreUnavailable with the inherited code and an explicit name', async () => {
    const s = setup();
    s.inner.state.mode = 'hang';
    const box = track(s.store.get('a'));
    await vi.advanceTimersByTimeAsync(DEADLINE);
    const error = box.error as SessionStoreTimeout;
    expect(error).toBeInstanceOf(SessionStoreUnavailable);
    expect(error.code).toBe('session_store_unavailable');
    expect(error.name).toBe('SessionStoreTimeout');
  });

  it('U3: after an expiry the next call is refused at once and never reaches the inner store', async () => {
    const s = setup();
    await openCircuit(s);
    const callsBefore = s.inner.calls.length;
    s.inner.state.mode = 'ok';
    const box = track(s.store.get('b'));
    await vi.advanceTimersByTimeAsync(0);
    expect((box.error as SessionStoreTimeout).phase).toBe('circuit-open');
    expect(s.inner.calls.length).toBe(callsBefore);
  });

  it('U4: after the cooldown with no ready event, the next call reaches the inner store', async () => {
    const s = setup();
    await openCircuit(s);
    await vi.advanceTimersByTimeAsync(COOLDOWN);
    s.inner.state.mode = 'ok';
    const callsBefore = s.inner.calls.length;
    await expect(s.store.get('b')).resolves.toBeNull();
    expect(s.inner.calls.length).toBe(callsBefore + 1);
  });

  it('U5: after the cooldown, a second expiry re-opens the circuit and logs a second line', async () => {
    const s = setup();
    await openCircuit(s);
    await vi.advanceTimersByTimeAsync(COOLDOWN);
    const again = track(s.store.get('b'));
    await vi.advanceTimersByTimeAsync(DEADLINE);
    expect((again.error as SessionStoreTimeout).phase).toBe('deadline');
    expect(s.log.events.filter(([name]) => name === 'store.operation_timeout')).toHaveLength(2);
    const refused = track(s.store.get('c'));
    await vi.advanceTimersByTimeAsync(0);
    expect((refused.error as SessionStoreTimeout).phase).toBe('circuit-open');
  });

  it('U6: a ready event closes the circuit at once', async () => {
    const s = setup();
    await openCircuit(s);
    s.client.emit('ready');
    s.inner.state.mode = 'ok';
    await expect(s.store.get('b')).resolves.toBeNull();
  });

  it('U7: three concurrent expiries log store.operation_timeout exactly once', async () => {
    const s = setup();
    s.inner.state.mode = 'hang';
    const boxes = [track(s.store.get('a')), track(s.store.delete('b')), track(s.store.releaseLock('c', 'tok'))];
    await vi.advanceTimersByTimeAsync(DEADLINE);
    for (const box of boxes) expect((box.error as SessionStoreTimeout).phase).toBe('deadline');
    expect(s.log.events).toEqual([['store.operation_timeout', { operation: 'get', deadlineMs: DEADLINE }]]);
  });

  it('U8: a call that settles in time leaves no pending timer', async () => {
    const s = setup();
    await expect(s.store.get('a')).resolves.toBeNull();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('U9: a fast rejection passes through unchanged and does not open the circuit', async () => {
    const s = setup();
    s.inner.state.mode = 'reject';
    const error = await s.store.get('a').catch((e: unknown) => e);
    expect(error).toBeInstanceOf(SessionStoreUnavailable);
    expect(error).not.toBeInstanceOf(SessionStoreTimeout);
    s.inner.state.mode = 'ok';
    await expect(s.store.get('b')).resolves.toBeNull();
    expect(s.log.events).toEqual([]);
  });

  // U10 pins the WIRING: deleting the wrap on any one method reds exactly that row.
  const rec = makeRecord();
  const ops: Array<[string, (store: SessionStore) => Promise<unknown>]> = [
    ['get', (store) => store.get('a')],
    ['set', (store) => store.set('a', rec, 1000, null)],
    ['delete', (store) => store.delete('a')],
    ['tryAcquireLock', (store) => store.tryAcquireLock('a', 'tok', 1000)],
    ['releaseLock', (store) => store.releaseLock('a', 'tok')],
    ['putTransaction', (store) => store.putTransaction('x', { codeVerifier: 'v', nonce: 'n', returnTo: '/', secretHash: 'h', createdAt: 0 }, 1000)],
    ['takeTransaction', (store) => store.takeTransaction('x')],
  ];
  it.each(ops)('U10: %s is bounded and names itself', async (operation, call) => {
    const s = setup();
    s.inner.state.mode = 'hang';
    const box = track(call(s.store));
    await vi.advanceTimersByTimeAsync(DEADLINE);
    const error = box.error as SessionStoreTimeout;
    expect(error.phase).toBe('deadline');
    expect(error.message).toBe(`the session store operation "${operation}" did not answer within ${String(DEADLINE)} ms`);
  });

  it('U11: close passes through an open circuit, with no deadline', async () => {
    const s = setup();
    await openCircuit(s);
    const callsBefore = s.inner.calls.length;
    s.inner.state.mode = 'ok';
    await expect(s.store.close()).resolves.toBeUndefined();
    expect(s.inner.calls.slice(callsBefore)).toEqual(['close']);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('U12: no message and no log field holds a URL', async () => {
    const s = setup();
    await openCircuit(s);
    const refused = await s.store.get('b').catch((e: unknown) => e);
    const texts = [(refused as Error).message, JSON.stringify(s.log.events)];
    for (const text of texts) expect(text).not.toMatch(/redis:\/\//);
    expect((refused as Error).message).toBe('the session store is not answering: "get" was refused while the circuit was open');
  });
});
