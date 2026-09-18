# SMA-650 — descriptor-cache operation deadline: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every `@paigasus/console-core` descriptor-cache operation a real end-to-end deadline, and make the process recover from a Redis that accepts commands and never replies under steady traffic.

**Architecture:** One new wrapper in `ts/packages/paigasus-console-core/src/discovery.ts`, composed outside the existing `afterConnect`. It races each operation against a `4 × timeoutMs` deadline. On expiry it rejects and opens a circuit that stops writing to the socket, which is what lets node-redis's own idle timer fire and its SMA-648 reconnect strategy repair the connection. The circuit closes on the client's next `ready` event or after a `4 × timeoutMs` cooldown.

**Tech Stack:** TypeScript, node-redis (`redis` ^6.2.1 / `@redis/client` 6.2.1), vitest 5, testcontainers, Moon.

**Spec:** `docs/superpowers/specs/2026-09-18-sma-650-descriptor-cache-operation-deadline-design.md` (revision 2, approved 2026-09-18). Read it — the plan argues from it, and every decision reference below (`D1`–`D10`, `§ 2 fact N`) is to that document.

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- `T = PAIGASUS_SESSION_REDIS_TIMEOUT_MS`, default 1000 ms. Deadline `= 4T`. Cooldown `= 4T`.
- **Never log a node-redis error object, its message, the DSN, or the URL.** node-redis embeds the DSN in connection errors (`src/logger.ts:7-9`).
- The wrapper listens for `ready` ONLY. An `error` listener would red the existing pin `expect(client.listenerCount('error')).toBe(1)` at `ts/packages/paigasus-console-core/tests/unit/discovery-redis-client.test.ts:70-72`.
- No behavioural change to `@paigasus/discovery`. Task 5's two edits there are comment and README text only.
- Run everything from the worktree root `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-650`. Prefix any Moon or proto command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Commit after every task. Do NOT use `--no-verify`; the deps are installed, so commitlint works. Keep a `#NNN` or `token: value` line out of the commit BODY — it fails `footer-leading-blank`.

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `ts/packages/paigasus-console-core/src/discovery.ts` | The deadline wrapper, the error class, and the composition. Modify. | 1, 2, 3 |
| `ts/packages/paigasus-console-core/src/logger.ts` | One new `AppEventName` member. Modify (`:14-21`). | 2 |
| `ts/packages/paigasus-console-core/tests/unit/discovery-deadline.test.ts` | The whole default tier for the wrapper, U1–U11. Create. | 1, 2, 3 |
| `ts/packages/paigasus-console-core/tests/containers/descriptor-cache-idle.test.ts` | T6, the Docker acceptance test. Modify. | 4 |
| Four documents + one memory note | Correct the stale "follow-up SMA-650" forward reference. Modify. | 5 |

---

### Task 1: The deadline

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/discovery.ts` (add after `afterConnect`, which ends at `:188`)
- Test: `ts/packages/paigasus-console-core/tests/unit/discovery-deadline.test.ts` (create)

**Interfaces:**
- Consumes: `DescriptorCache` from `@paigasus/discovery/server`; `ConsoleLogger` from `./logger`.
- Produces, all exported from `src/discovery.ts`:
  - `type DescriptorCacheTimeoutPhase = 'deadline' | 'circuit-open'`
  - `class DescriptorCacheTimeoutError extends Error` with `readonly phase: DescriptorCacheTimeoutPhase`
  - `type ReadySource = { on(event: 'ready', listener: () => void): unknown }`
  - `function withOperationDeadline(inner: DescriptorCache, client: ReadySource, timeoutMs: number, log: ConsoleLogger): DescriptorCache`

Task 1 builds the deadline only. The circuit is Task 2, so `withOperationDeadline` already takes
`client` and `log` and simply does not use them yet — that keeps Task 2 from changing the signature
and every call site with it.

- [ ] **Step 1: Write the failing tests (U1, U2, U8, U9, U10)**

Create `ts/packages/paigasus-console-core/tests/unit/discovery-deadline.test.ts`:

```ts
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
    for (const run of [() => cache.get('iam'), () => cache.set('iam', record(), 1000, null), () => cache.delete('iam'), () => cache.tryAcquireLock('iam', 't', 1000), () => cache.releaseLock('iam', 't')]) {
      const pending = run().catch(() => undefined);
      await vi.advanceTimersByTimeAsync(DEADLINE_MS);
      await pending;
    }
    expect(inner.calls).toEqual(['get', 'set', 'delete', 'tryAcquireLock', 'releaseLock']);
    await expect(cache.close()).resolves.toBeUndefined();
    expect(inner.calls).toEqual(['get', 'set', 'delete', 'tryAcquireLock', 'releaseLock', 'close']);
  });
});
```

`COOLDOWN_MS` is unused until Task 2. If `ts:lint` rejects an unused constant, declare it in Task 2
instead — do not change the deadline constants to silence it.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cd ts/packages/paigasus-console-core && pnpm exec vitest run tests/unit/discovery-deadline.test.ts
```

Expected: FAIL. The import of `DescriptorCacheTimeoutError` and `withOperationDeadline` from
`../../src/discovery` does not resolve, so the whole file errors before any test runs.

- [ ] **Step 3: Write the implementation**

In `ts/packages/paigasus-console-core/src/discovery.ts`, insert after `afterConnect` (which ends at
`:188`) and before `redisDescriptorCache`:

```ts
/** How many command timeouts one cache operation may take, end to end (SMA-650 D3). */
const DEADLINE_FACTOR = 4;

/** Which half of the wrapper refused the operation (SMA-650 D9). */
export type DescriptorCacheTimeoutPhase = 'deadline' | 'circuit-open';

/**
 * The descriptor cache did not answer. `phase` says why: `deadline` means this operation itself ran
 * past its bound, `circuit-open` means an earlier one did and this one was refused without touching
 * the socket (SMA-650 D9).
 *
 * `name` is set explicitly, because a production bundle can mangle `constructor.name` — the same
 * reasoning connectionLossReason records above. The message NEVER carries the DSN, the URL, or a
 * node-redis error: `operation` is one of five fixed literals.
 */
export class DescriptorCacheTimeoutError extends Error {
  readonly phase: DescriptorCacheTimeoutPhase;

  constructor(operation: string, deadlineMs: number, phase: DescriptorCacheTimeoutPhase) {
    super(phase === 'deadline' ? `the descriptor cache operation "${operation}" did not answer within ${deadlineMs} ms` : `the descriptor cache is not answering: "${operation}" was refused while the circuit was open`);
    this.name = 'DescriptorCacheTimeoutError';
    this.phase = phase;
  }
}

/**
 * The part of a node-redis client the deadline wrapper reads. Structural, so the default test tier
 * can drive it with a plain EventEmitter — the same shape ConnectionLossSource uses above.
 *
 * It carries `ready` ONLY. An `error` listener here would duplicate watchConnectionLoss's job and
 * would break its test's pin on listenerCount('error') === 1.
 */
export type ReadySource = { on(event: 'ready', listener: () => void): unknown };

/**
 * Bounds every cache operation (SMA-650). `commandOptions.timeout` covers only the queued phase and
 * `socketTimeout` is an idle timer that any write resets, so a Redis that accepts commands and never
 * replies is unbounded under steady traffic. This is the only end-to-end bound.
 */
export function withOperationDeadline(inner: DescriptorCache, client: ReadySource, timeoutMs: number, log: ConsoleLogger): DescriptorCache {
  const deadlineMs = timeoutMs * DEADLINE_FACTOR;

  const bounded = async <T>(operation: string, run: () => Promise<T>): Promise<T> => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const running = run();
    try {
      return await Promise.race([
        running,
        new Promise<never>((_resolve, reject) => {
          timer = setTimeout(() => reject(new DescriptorCacheTimeoutError(operation, deadlineMs, 'deadline')), deadlineMs);
          timer.unref();
        }),
      ]);
    } catch (error) {
      if (error instanceof DescriptorCacheTimeoutError && error.phase === 'deadline') {
        // D7: the abandoned operation keeps running. node-redis rejects it when the socket dies, and
        // an unhandled rejection can end the process. A late SUCCESS is discarded: the caller has
        // already degraded and moved on.
        running.catch(() => undefined);
      }
      throw error;
    } finally {
      // Mandatory. An uncleared timer is a leak per operation, and worse, it would open the circuit
      // long after an operation that already settled.
      clearTimeout(timer);
    }
  };

  return {
    get: (service) => bounded('get', () => inner.get(service)),
    set: (service, rec, ttlMs, expectedRev) => bounded('set', () => inner.set(service, rec, ttlMs, expectedRev)),
    delete: (service) => bounded('delete', () => inner.delete(service)),
    tryAcquireLock: (service, token, ttlMs) => bounded('tryAcquireLock', () => inner.tryAcquireLock(service, token, ttlMs)),
    releaseLock: (service, token) => bounded('releaseLock', () => inner.releaseLock(service, token)),
    close: () => inner.close(),
  };
}
```

`client` and `log` are unused in this task. If `ts:lint` rejects unused parameters, prefix them in
this task only and drop the prefix in Task 2 — do NOT remove them from the signature.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd ts/packages/paigasus-console-core && pnpm exec vitest run tests/unit/discovery-deadline.test.ts
```

Expected: PASS, six tests.

- [ ] **Step 5: Run the package's whole default tier and the typecheck**

```bash
cd ts/packages/paigasus-console-core && pnpm exec vitest run && pnpm exec tsc --noEmit -p tsconfig.json
```

Expected: PASS. In particular `tests/unit/discovery-redis-client.test.ts` must stay green — nothing
is wired yet, so it cannot have changed.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-console-core/src/discovery.ts ts/packages/paigasus-console-core/tests/unit/discovery-deadline.test.ts
git commit -m "feat(ts): bound each descriptor-cache operation with a deadline (SMA-650)"
```

---

### Task 2: The circuit

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/discovery.ts` (the `withOperationDeadline` body from Task 1)
- Modify: `ts/packages/paigasus-console-core/src/logger.ts:14-21`
- Test: `ts/packages/paigasus-console-core/tests/unit/discovery-deadline.test.ts` (extend)

**Interfaces:**
- Consumes: everything Task 1 produced. The signature of `withOperationDeadline` does NOT change.
- Produces: `AppEventName` gains `'discovery.redis_operation_timeout'`, logged with fields
  `{ operation: string, deadlineMs: number }`.

- [ ] **Step 1: Add the event name**

In `ts/packages/paigasus-console-core/src/logger.ts`, add one member to the `AppEventName` union
(currently `:14-21`), after `'discovery.redis_connection_lost'`:

```ts
  | 'discovery.redis_operation_timeout'
```

- [ ] **Step 2: Write the failing tests (U3–U7, U11)**

Append to `tests/unit/discovery-deadline.test.ts`:

```ts
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
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cd ts/packages/paigasus-console-core && pnpm exec vitest run tests/unit/discovery-deadline.test.ts
```

Expected: U3, U5b, U6, U7 and U11 FAIL. U3 fails because the second `get` reaches the inner cache
and parks instead of rejecting. U11 fails with `listenerCount('ready')` of 0. U4 and U5 may pass by
accident — there is no circuit yet, so an operation always reaches the inner cache. That is why U3,
U5b and U11 carry the real weight here.

- [ ] **Step 4: Write the implementation**

In `src/discovery.ts`, add the cooldown constant beside `DEADLINE_FACTOR`:

```ts
/** How many command timeouts the wrapper stays silent after an expiry (SMA-650 D5). */
const COOLDOWN_FACTOR = 4;
```

Then replace the body of `withOperationDeadline` with this version. The `bounded` helper gains the
circuit check at the top and the circuit opening in the catch:

```ts
export function withOperationDeadline(inner: DescriptorCache, client: ReadySource, timeoutMs: number, log: ConsoleLogger): DescriptorCache {
  const deadlineMs = timeoutMs * DEADLINE_FACTOR;
  const cooldownMs = timeoutMs * COOLDOWN_FACTOR;
  // null = closed. Otherwise the Date.now() at which it opened (D4, D5). One field, mutated only
  // from the event loop's single thread, so D6's read-then-write cannot interleave.
  let openedAt: number | null = null;

  // `ready` is the exact signal that node-redis finished a reconnect. NO error listener (D4 note).
  client.on('ready', () => {
    openedAt = null;
  });

  const bounded = async <T>(operation: string, run: () => Promise<T>): Promise<T> => {
    if (openedAt !== null) {
      // While the cooldown runs we write NOTHING. That silence is what lets node-redis's idle timer
      // fire and its reconnect strategy repair the socket (SMA-650 § 2 fact 4) — it is the repair
      // mechanism, not only a cost saving.
      if (Date.now() - openedAt < cooldownMs) throw new DescriptorCacheTimeoutError(operation, deadlineMs, 'circuit-open');
      // The cooldown elapsed with no `ready`. Let this operation through: if the client is still not
      // ready, node-redis refuses it at once (disableOfflineQueue), so the attempt costs nothing.
      openedAt = null;
    }
    let timer: ReturnType<typeof setTimeout> | undefined;
    const running = run();
    try {
      return await Promise.race([
        running,
        new Promise<never>((_resolve, reject) => {
          timer = setTimeout(() => reject(new DescriptorCacheTimeoutError(operation, deadlineMs, 'deadline')), deadlineMs);
          timer.unref();
        }),
      ]);
    } catch (error) {
      if (error instanceof DescriptorCacheTimeoutError && error.phase === 'deadline') {
        // D6: concurrent operations all expire together. Only the FIRST opens the circuit and logs,
        // or one wedge would log a line per operation and the cooldown would never elapse.
        if (openedAt === null) {
          openedAt = Date.now();
          log.appEvent('discovery.redis_operation_timeout', { operation, deadlineMs });
        }
        // D7: see Task 1.
        running.catch(() => undefined);
      }
      throw error;
    } finally {
      clearTimeout(timer);
    }
  };

  return {
    get: (service) => bounded('get', () => inner.get(service)),
    set: (service, rec, ttlMs, expectedRev) => bounded('set', () => inner.set(service, rec, ttlMs, expectedRev)),
    delete: (service) => bounded('delete', () => inner.delete(service)),
    tryAcquireLock: (service, token, ttlMs) => bounded('tryAcquireLock', () => inner.tryAcquireLock(service, token, ttlMs)),
    releaseLock: (service, token) => bounded('releaseLock', () => inner.releaseLock(service, token)),
    close: () => inner.close(),
  };
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd ts/packages/paigasus-console-core && pnpm exec vitest run tests/unit/discovery-deadline.test.ts
```

Expected: PASS, all of U1–U11.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-console-core/src/discovery.ts ts/packages/paigasus-console-core/src/logger.ts ts/packages/paigasus-console-core/tests/unit/discovery-deadline.test.ts
git commit -m "feat(ts): open a circuit when a descriptor-cache operation times out (SMA-650)"
```

---

### Task 3: Wire it into the real client

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/discovery.ts:190-206` (`redisDescriptorCache`)

**Interfaces:**
- Consumes: `withOperationDeadline` from Task 1 and 2.
- Produces: no new exports. `descriptorCacheFor` now returns a deadline-wrapped cache for the Redis path.

Until this task, nothing calls the wrapper. This is where a render actually gets the bound.

- [ ] **Step 1: Write the failing test**

Append to `tests/unit/discovery-deadline.test.ts`:

```ts
describe('descriptorCacheFor wires the deadline in (D1)', () => {
  it('bounds a real Redis path operation instead of hanging', async () => {
    // Port 1 refuses the connection, so the cache never becomes ready. The point of this test is
    // only that the operation SETTLES — before the wiring it would depend entirely on node-redis.
    const cache = descriptorCacheFor({ PAIGASUS_SESSION_STORE: 'redis', PAIGASUS_SESSION_REDIS_URL: 'redis://127.0.0.1:1', PAIGASUS_SESSION_REDIS_TIMEOUT_MS: TIMEOUT_MS } as ConsoleCoreConfig, createJsonLogger(() => undefined));
    const settled = cache.get('iam').then(
      () => 'resolved',
      (error: unknown) => error,
    );
    await vi.advanceTimersByTimeAsync(DEADLINE_MS + 1);
    await expect(settled).resolves.toBeDefined();
  });
});
```

Add the two imports this needs at the top of the file:

```ts
import type { ConsoleCoreConfig } from '../../src/config-shape';
import { descriptorCacheFor, DescriptorCacheTimeoutError, resetDiscoveryForTest, withOperationDeadline } from '../../src/discovery';
```

and an `afterEach(() => { resetDiscoveryForTest(); })`, so the process singleton does not leak into
the next test — the same teardown `tests/unit/discovery-redis-client.test.ts:13-15` uses.

- [ ] **Step 2: Run it**

```bash
cd ts/packages/paigasus-console-core && pnpm exec vitest run tests/unit/discovery-deadline.test.ts
```

Note this test is a WIRING check, not a red-first assertion: it can pass before the change, because
`connectOnce` also bounds the first wait. Record its result now so Step 4's result means something.

- [ ] **Step 3: Wire it**

In `redisDescriptorCache` (`src/discovery.ts:190-206`), replace the final return:

```ts
  const inner = createRedisDescriptorCache(client);
  return afterConnect(inner, connectOnce(client, timeoutMs, log));
```

with:

```ts
  const inner = createRedisDescriptorCache(client);
  // The deadline wraps OUTSIDE afterConnect, so it bounds the `await ready` too and `4 × timeoutMs`
  // is the whole bound, not an addition to the connect wait (SMA-650 § 4.1).
  return withOperationDeadline(afterConnect(inner, connectOnce(client, timeoutMs, log)), client, timeoutMs, log);
```

- [ ] **Step 4: Run the package's whole default tier**

```bash
cd ts/packages/paigasus-console-core && pnpm exec vitest run && pnpm exec tsc --noEmit -p tsconfig.json
```

Expected: PASS. `tests/unit/discovery-redis-client.test.ts` must stay green — especially
`listenerCount('error') === 1` (the wrapper adds a `ready` listener only) and the client-options pin
at `:137-153` (the wrapper changes no client option).

- [ ] **Step 5: Confirm Moon selects the new test file**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon task paigasus-console-core-ts:test --json | head -60
```

Expected: the resolved `inputs` cover `tests/**/*`. The project's own `moon.yml` `test` block does
not list it, so this confirms the inherited group supplies it (spec § 10.3). If it does NOT, add
`'tests/**/*'` to that task's `inputs` and say so in the commit message.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-console-core
git commit -m "feat(ts): apply the operation deadline to the real descriptor cache (SMA-650)"
```

---

### Task 4: The Docker acceptance test

**Files:**
- Modify: `ts/packages/paigasus-console-core/tests/containers/descriptor-cache-idle.test.ts`

**Interfaces:**
- Consumes: `DescriptorCacheTimeoutError` from `../../src/discovery`.
- Produces: nothing. This task only adds a test.

**Docker must be running.** This tier has no skip hatch; it fails loudly when the daemon is
unreachable.

- [ ] **Step 1: Add T6**

At the top of the file, beside the existing constants (`:27-36`), add:

```ts
/** SMA-650 D3: 4 × the command timeout. */
const DEADLINE_MS = TIMEOUT_MS * 4;
/** Strictly BELOW socketTimeout, so the driver's writes keep resetting the idle timer. */
const DRIVE_STEP_MS = 500;
```

Extend the import from `../../src/discovery` (`:24`) to bring in the error class:

```ts
import { DescriptorCacheTimeoutError, descriptorCacheFor, resetDiscoveryForTest } from '../../src/discovery';
```

Add this case inside the existing `describe` block, after T5:

```ts
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
  });
```

- [ ] **Step 2: Run the Docker tier**

```bash
cd ts/packages/paigasus-console-core && pnpm exec vitest run --config vitest.containers.config.ts
```

Expected: PASS, T1–T6.

- [ ] **Step 3: Prove T6 is red without the fix (mutation)**

Temporarily revert ONLY the wiring from Task 3. Edit `src/discovery.ts`'s `redisDescriptorCache`
return to drop the wrapper:

```ts
  return afterConnect(inner, connectOnce(client, timeoutMs, log));
```

Then run:

```bash
cd ts/packages/paigasus-console-core && pnpm exec vitest run --config vitest.containers.config.ts -t 'T6'
```

Expected: FAIL — `deadlineFailures.length` is 0, because without the wrapper the driver keeps
writing, the idle timer never fires, and nothing rejects inside the window.

**Restore by editing the line back**, never with `git checkout --` or `git stash`: the stash stack is
shared with other worktrees and sessions, and a checkout would also discard uncommitted work.
Confirm the restore with `git diff` showing no change to `src/discovery.ts`.

- [ ] **Step 4: Re-run the tier after restoring**

```bash
cd ts/packages/paigasus-console-core && pnpm exec vitest run --config vitest.containers.config.ts
```

Expected: PASS, T1–T6.

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-console-core/tests/containers/descriptor-cache-idle.test.ts
git commit -m "test(ts): prove the descriptor-cache deadline bounds a busy hang (SMA-650)"
```

---

### Task 5: Correct the four stale documents

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/discovery.ts:24`
- Modify: `ts/packages/paigasus-discovery/src/adapters/redis-cache.ts:87`
- Modify: `ts/packages/paigasus-discovery/README.md:104`
- Modify: `CLAUDE.md:1059`
- Modify: `/Users/smaschek/.claude/projects/-Users-smaschek-dev-paigasus-paigasus-core/memory/node-redis-command-vs-socket-timeout.md` and its `MEMORY.md` index line

**Interfaces:** none. Text only. The two `@paigasus/discovery` edits are a COMMENT and a README
paragraph — no behavioural change, which is what the spec's Scope line means.

Each site says the gap is a follow-up that is still open. After this branch it is closed, and
SMA-648's spec records that a wrong document "is how the bug shipped".

- [ ] **Step 1: Correct the four in-tree sites**

Each currently reads "(follow-up SMA-650)" or "(SMA-650)" after a sentence about a hung command
going unnoticed under steady traffic. Replace the parenthetical with a statement of what now bounds
it. For `ts/packages/paigasus-console-core/src/discovery.ts:24`:

```
//   go unnoticed. withOperationDeadline below is the end-to-end bound (SMA-650).
```

For `ts/packages/paigasus-discovery/src/adapters/redis-cache.ts:87` and
`ts/packages/paigasus-discovery/README.md:104`, keep the existing sentence and change the
parenthetical to name the consumer-side fix, because this package does not carry it:

```
replies can go unnoticed. A consumer needs its own per-operation deadline;
@paigasus/console-core's withOperationDeadline is the worked example (SMA-650).
```

For `CLAUDE.md:1059`, change "(follow-up SMA-650)" to
"(SMA-650 added a per-operation deadline in `@paigasus/console-core`; `@paigasus/auth` still has none — SMA-651)".

- [ ] **Step 2: Check you did not break a CLAUDE.md gate**

`CLAUDE.md` carries marker-delimited regions that gates count. Confirm the edit touched none of them:

```bash
grep -c 'ci-targets:begin\|ci-targets:end\|moon-diagnosis:begin\|moon-diagnosis:end' CLAUDE.md
```

Expected: `4`. A different number means the edit damaged a marker — restore it.

- [ ] **Step 3: Update the memory note**

In `/Users/smaschek/.claude/projects/-Users-smaschek-dev-paigasus-paigasus-core/memory/node-redis-command-vs-socket-timeout.md`, the "Open gaps" paragraph says "no per-operation deadline on the descriptor cache (SMA-650)". Change it to record that SMA-650 closed it with a `4 × timeout` deadline plus a circuit that stops writing so the idle timer can fire, and that SMA-651 is still open. Update the matching `MEMORY.md` index line, which currently ends "(SMA-648; gaps SMA-650/651)".

- [ ] **Step 4: Verify nothing else still claims the gap is open**

```bash
grep -rn "follow-up SMA-650" --include='*.ts' --include='*.md' . | grep -v docs/superpowers
```

Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add -A ts CLAUDE.md
git commit -m "docs(ts): record that the descriptor cache now has a per-operation deadline (SMA-650)"
```

---

### Task 6: The full graph

**Files:** none. Verification only.

Per-project Moon tasks do not run the repo-level gates, so run the graph the way CI does before the
PR.

- [ ] **Step 1: Run the affected graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :test-e2e \
  --base origin/main --include-relations
```

- [ ] **Step 2: Read a failure correctly before believing it**

Three known local artefacts, all recorded in `CLAUDE.md`:

- **A wall of "expected rc 0" rows, or an EMPTY stdout with a one-line `declare: -A: invalid option`
  or `mapfile: command not found` on stderr** — that is the wrong bash, not a finding.
  `repo:ruff-ci`, `repo:next-public-free` and `repo:publish-metadata` need bash 4+;
  `repo:affected-smoke` needs system `/bin/bash` 3.2. Re-run the gate directly under the other bash
  and read THAT verdict. `repo:actionlint` has no working local bash at all — CI is its only verdict.
- **A sub-3s `repo:affected-smoke` failure** — capture the full task output BEFORE re-running, then
  grep it for `proto-shim`. A re-run passes and destroys the evidence.
- **`repo:release-parity*` aborting at rc=2** — export `PROTO_REPORTER=text`.

For anything else, use the diagnosis procedure in `CLAUDE.md`: copy `.moon/cache/ciReport.json` and
`.moon/cache/states/<project>/<task>/` out of the repo FIRST, then read the failing action's
`operations[]` entry whose `meta.type` is `task-execution` for the real command and exit code.

- [ ] **Step 3: Run the Prettier gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:fmt
```

`ts:fmt` is its own whole-tree gate, decoupled from `ts:lint` and the typecheck, and this branch
touched several `.ts` files.

- [ ] **Step 4: Commit any formatting fix**

```bash
git add -A ts
git commit -m "style(ts): apply prettier to the deadline wrapper (SMA-650)"
```

Skip this step if the tree is clean.

---

## Self-Review

**Spec coverage.** D1 → Task 3. D2, D3, D7, D9, D10 → Task 1. D4, D5, D6, D8 → Task 2. § 9 → Task 5.
§ 10.1 U1–U11 → Tasks 1 and 2. § 10.2 T6 → Task 4. § 10.3 → Task 3 Step 5.

**Not implemented, by design.** § 5, § 6, § 7 and § 8 are analysis and rejected alternatives; they
produce no code. § 1's note that SMA-651 is needed before a render is truly bounded stays a
non-goal.

**Type consistency.** `withOperationDeadline(inner, client, timeoutMs, log)` keeps one signature
across Tasks 1, 2 and 3. `DescriptorCacheTimeoutError` carries `phase` in every task that reads it.
The event name string `'discovery.redis_operation_timeout'` matches between `src/logger.ts` (Task 2
Step 1), the implementation (Task 2 Step 4) and the test helper `timeouts()` (Task 1 Step 1).

**Placeholder scan.** Clean. An earlier draft of this plan carried two test blocks that would not
have compiled — U8 used an undefined helper and U7 ended with an unusable `rejects.not` chain —
each with a correction printed beneath it. Both are now written correctly in place, and the
corrections are gone. Every code block in this plan is meant to be used as written.
