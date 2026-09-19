# SMA-651 Session Store Operation Deadline — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound every `@paigasus/auth` Redis session-store operation, and the first connect, against a Redis that accepts commands and never replies, while an idle client stays alive.

**Architecture:** A decorator over the `SessionStore` port (`withOperationDeadline`) gives each operation a 4T deadline and opens a circuit on the first expiry; the circuit's silence lets node-redis's new idle timer (`socketTimeout: 2T`, kept alive by `pingInterval: T`) tear down and reconnect a wedged socket. `createRedisSessionStore` composes the decorator, bounds its first connect at T, and `close()` destroys at once.

**Tech Stack:** TypeScript (ESM, `verbatimModuleSyntax`), node-redis `redis@6.2.1`, vitest 5, testcontainers (`redis:8-alpine`), Moon 2.5.3, pnpm.

**Spec:** `docs/superpowers/specs/2026-09-19-sma-651-session-store-operation-deadline-design.md` (revision 2, approved by Sven on 2026-09-19). Read it before any task. Section and decision numbers below (§ 5, D3, E2, U10 …) refer to it.

## Global Constraints

- **Worktree first.** The worktree is `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-651-auth-redis-deadline`, branch `feature/sma-651-auth-redis-in-flight-deadline`. A subagent starts pinned to the MAIN checkout: its FIRST action is `EnterWorktree` with `path` set to the worktree above, then `git branch --show-current` must print the branch above. Never commit to `main`.
- **Add, don't amend.** Every task makes NEW commits. Never `git commit --amend`, never `git stash`, never `git checkout -- <file>` on a file with uncommitted work you want to keep.
- **Foreground only.** Do not start background jobs and end your turn waiting on them. Run every command in the foreground.
- **PATH.** Prefix shell commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- **Package dir.** `ts/packages/paigasus-auth` (below: `PKG`). Unit tests: `pnpm -C ts/packages/paigasus-auth exec vitest run <file>`. Container tests (need Docker): `pnpm -C ts/packages/paigasus-auth exec vitest run --config vitest.containers.config.ts <file>`.
- **SPDX header** on every new source file: `// SPDX-License-Identifier: Apache-2.0`.
- **Imports inside `src/` are extensionless** (`'../core/errors'`). Test files import `src/` with a `.js` suffix, as the existing tests do (`'../../src/adapters/redis-store.js'`).
- **Template literals:** wrap numbers in `String(...)`, as the package already does.
- **T** is `PAIGASUS_SESSION_REDIS_TIMEOUT_MS` / `commandTimeoutMs`. Exact values: `pingInterval = T`, `socketTimeout = 2T`, `connectTimeout = T`, `commandOptions.timeout = T`, deadline `= 4T`, cooldown `= 4T`, connect wait `= T`, maximum T `= 536_870_911`.
- **Redaction is absolute.** No message, log field or test output may carry the DSN, the URL, or a node-redis error object. The new event is `store.operation_timeout` with exactly the fields `{ operation, deadlineMs }`.
- **Commits:** conventional, scope `ts`, subject ends `(SMA-651)`, body ends with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`. No `#NNN` or `key: value` line in the body (commitlint `footer-leading-blank`).
- **Write in Simplified Technical English** in comments and docs: short sentences, active voice, no idiom.

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `PKG/tests/containers/redis-store-idle.test.ts` | Create | Docker proof: idle survival (E1), bound under traffic (E2), repair (E3), no secret (E4), close under a wedge (E5) |
| `PKG/src/core/errors.ts` | Modify | `SessionStoreTimeout` subclass + `SessionStoreTimeoutPhase` |
| `PKG/src/ports/logger.ts` | Modify | `'store.operation_timeout'` in `AuthEventName` |
| `PKG/src/adapters/operation-deadline.ts` | Create | The decorator: deadline + circuit |
| `PKG/tests/adapters/operation-deadline.test.ts` | Create | U1–U12 |
| `PKG/src/adapters/redis-store.ts` | Modify | Client options, duck type, `close()`, bounded connect, composition, `logger` option, comments |
| `PKG/tests/adapters/redis-client-options.test.ts` | Modify | New option pins, strategy with `SocketTimeoutError`, one `error` listener, `close()` twice |
| `PKG/tests/adapters/redis-store-connect.test.ts` | Create | Bounded connect (D6) |
| `PKG/src/config.ts` | Modify | `.max(536_870_911)` on T (D9) |
| `PKG/src/runtime.ts` | Modify | Logger computed first and passed to the store (D8) |
| `PKG/tests/config.test.ts` | Modify | The maximum |
| `PKG/tests/runtime-redis-wiring.test.ts` | Create | The runtime passes its logger to the store |
| `PKG/README.md` | Modify | Redis failure behaviour, ACL, § 5 consequences |
| `CLAUDE.md` | Modify | The stale "still has none — SMA-651" clause |

---

### Task 1: The Docker-backed proof, written first and measured red

This task writes the acceptance test BEFORE any production change and records which cases fail on unmodified code. It needs Docker.

**Files:**
- Create: `ts/packages/paigasus-auth/tests/containers/redis-store-idle.test.ts`

**Interfaces:**
- Consumes: `createRedisSessionStore({ url, commandTimeoutMs, keyPrefix, logger? })` from `src/adapters/redis-store.ts` (the `logger` option does not exist yet; the test passes it anyway, and TypeScript in vitest does not type-check, so the file runs), `makeRecord()` from `tests/store-contract.ts`.
- Produces: nothing other tasks import. Tasks 5 and 6 run this file.

- [ ] **Step 1: Write the test file**

```ts
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
  const store = await createRedisSessionStore({ url, commandTimeoutMs: TIMEOUT_MS, keyPrefix: `idle${randomBytes(4).toString('hex')}:`, logger: s.logger } as Parameters<typeof createRedisSessionStore>[0]);
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
```

- [ ] **Step 2: Run it against UNMODIFIED code and record the result**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run --config vitest.containers.config.ts tests/containers/redis-store-idle.test.ts`

Expected on unmodified code (spec § 8.3):
- E1 PASSES (unmodified code has no idle timer, so nothing reconnects; E1's red-first is measured in Task 6 with `pingInterval` deleted).
- E2 FAILS (no failure is a `SessionStoreTimeout`).
- E3 FAILS (no reconnect is counted).
- E5 FAILS (`closed` is `'still waiting'`: a graceful `close()` waits for the paused command).

If any of E2, E3 or E5 PASSES here, STOP and report: the test does not measure what the spec says. Copy the vitest summary lines (test name + pass/fail + first assertion message) into your report.

- [ ] **Step 3: Commit the red test**

```bash
git add ts/packages/paigasus-auth/tests/containers/redis-store-idle.test.ts
git commit -m "test(ts): prove the session store hangs on a Redis that never replies (SMA-651)

Red on this commit by design: E2, E3 and E5 fail until the deadline,
the idle timer and the destroying close() land.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: `SessionStoreTimeout`, the new event, and the deadline decorator

**Files:**
- Modify: `ts/packages/paigasus-auth/src/core/errors.ts` (after `SessionStoreUnavailable`, line 11)
- Modify: `ts/packages/paigasus-auth/src/ports/logger.ts:14-25`
- Create: `ts/packages/paigasus-auth/src/adapters/operation-deadline.ts`
- Test: `ts/packages/paigasus-auth/tests/adapters/operation-deadline.test.ts`

**Interfaces:**
- Consumes: `SessionStore`, `LoginTransaction` (`src/ports/session-store.ts`), `AuthLogger` (`src/ports/logger.ts`), `SessionRecord` (`src/core/session.ts`).
- Produces:
  - `export type SessionStoreTimeoutPhase = 'deadline' | 'circuit-open'` and `export class SessionStoreTimeout extends SessionStoreUnavailable { readonly phase: SessionStoreTimeoutPhase; constructor(operation: string, deadlineMs: number, phase: SessionStoreTimeoutPhase) }` in `src/core/errors.ts`. NOT exported from `src/server.ts`.
  - `'store.operation_timeout'` in `AuthEventName`.
  - `export type ReadySource = { on(event: 'ready', listener: () => void): unknown }`, `export const DEADLINE_FACTOR = 4`, `export const COOLDOWN_FACTOR = 4`, and `export function withOperationDeadline(inner: SessionStore, client: ReadySource, timeoutMs: number, logger: AuthLogger): SessionStore` in `src/adapters/operation-deadline.ts`.

- [ ] **Step 1: Write the failing unit tests**

```ts
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
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters/operation-deadline.test.ts`
Expected: FAIL — cannot resolve `../../src/adapters/operation-deadline.js` (and `SessionStoreTimeout` is not exported).

- [ ] **Step 3: Add the error class** — in `src/core/errors.ts`, directly after the `SessionStoreUnavailable` class:

```ts
/** Which half of the deadline decorator refused the operation (SMA-651 D7). */
export type SessionStoreTimeoutPhase = 'deadline' | 'circuit-open';

/**
 * The store did not answer in time (SMA-651). `deadline` means this operation ran past its bound;
 * `circuit-open` means an earlier one did and this one was refused without touching the socket.
 *
 * A SUBCLASS of SessionStoreUnavailable, deliberately: every caller that classifies on that class
 * (next/get-session.ts, core/single-flight.ts's release-time catch) keeps working unchanged, and
 * `code` stays 'session_store_unavailable'. `name` is set explicitly because a production bundle
 * can mangle `constructor.name`. The message names one of seven fixed operation literals and a
 * number, never the DSN.
 *
 * NOT exported from src/server.ts, for the reason RefreshRejected records below: getSession()
 * swallows every failure, so no consumer can observe one.
 */
export class SessionStoreTimeout extends SessionStoreUnavailable {
  readonly phase: SessionStoreTimeoutPhase;

  constructor(operation: string, deadlineMs: number, phase: SessionStoreTimeoutPhase) {
    super(
      phase === 'deadline'
        ? `the session store operation "${operation}" did not answer within ${String(deadlineMs)} ms`
        : `the session store is not answering: "${operation}" was refused while the circuit was open`,
    );
    this.name = 'SessionStoreTimeout';
    this.phase = phase;
  }
}
```

- [ ] **Step 4: Add the event name** — in `src/ports/logger.ts`, change the last union member:

```ts
  | 'store.unavailable'
  | 'store.operation_timeout';
```

- [ ] **Step 5: Write the decorator** — `src/adapters/operation-deadline.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The end-to-end bound on every session-store operation (SMA-651). node-redis's
// `commandOptions.timeout` covers only the QUEUED phase, and `socketTimeout` is an idle timer that
// any write resets, so under steady traffic a Redis that accepts commands and never replies is
// unbounded without this.
//
// Two halves:
// - A DEADLINE of 4 x the command timeout. Without our traffic node-redis's own recovery takes up
//   to pingInterval + socketTimeout = 3T (one more PING goes out after a wedge starts), so 4T
//   keeps a margin above it and matches @paigasus/console-core's descriptor cache (SMA-650).
// - A CIRCUIT. The first expiry opens it, and while it is open every call is refused at once and
//   NOTHING is written to the socket. That silence is the REPAIR: it lets the idle timer fire and
//   the reconnect strategy reopen the socket. `ready` closes it; so does the end of the cooldown.
//
// It is a decorator over the port (ports/session-store.ts names this composition), so the Redis
// adapter stays a plain adapter and this file needs no Redis at all to test.
import { SessionStoreTimeout } from '../core/errors';
import type { AuthLogger } from '../ports/logger';
import type { SessionStore } from '../ports/session-store';

/** How many command timeouts one operation may take, end to end (SMA-651 D2). */
export const DEADLINE_FACTOR = 4;

/** How many command timeouts the circuit stays open after an expiry (SMA-651 D3). */
export const COOLDOWN_FACTOR = 4;

/**
 * The part of a node-redis client the decorator reads. Structural, so a test can pass a plain
 * EventEmitter. `ready` ONLY: an `error` listener here would break the rule that the Redis
 * adapter's silent listener is the only one (tests/adapters/redis-client-options.test.ts).
 */
export type ReadySource = { on(event: 'ready', listener: () => void): unknown };

type Operation = 'get' | 'set' | 'delete' | 'tryAcquireLock' | 'releaseLock' | 'putTransaction' | 'takeTransaction';

export function withOperationDeadline(inner: SessionStore, client: ReadySource, timeoutMs: number, logger: AuthLogger): SessionStore {
  const deadlineMs = timeoutMs * DEADLINE_FACTOR;
  const cooldownMs = timeoutMs * COOLDOWN_FACTOR;
  // null = closed. Otherwise the performance.now() at which it opened. A MONOTONIC clock, so a
  // wall-clock step cannot hold the circuit open (SMA-651 D3). Mutated only on the event loop's
  // one thread, so the read-then-write below cannot interleave.
  let openedAt: number | null = null;

  // `ready` is the exact signal that node-redis finished a reconnect.
  client.on('ready', () => {
    openedAt = null;
  });

  const bounded = async <T>(operation: Operation, run: () => Promise<T>): Promise<T> => {
    if (openedAt !== null) {
      if (performance.now() - openedAt < cooldownMs) throw new SessionStoreTimeout(operation, deadlineMs, 'circuit-open');
      // The cooldown ended with no `ready`. Let EVERY call through again, not one probe. If the
      // client is not ready, node-redis refuses each call at once (disableOfflineQueue), as a
      // plain SessionStoreUnavailable that does not re-open the circuit.
      openedAt = null;
    }
    let timer: ReturnType<typeof setTimeout> | undefined;
    const running = run();
    try {
      return await Promise.race([
        running,
        new Promise<never>((_resolve, reject) => {
          timer = setTimeout(() => {
            reject(new SessionStoreTimeout(operation, deadlineMs, 'deadline'));
          }, deadlineMs);
          timer.unref();
        }),
      ]);
    } catch (error) {
      // Only a DEADLINE expiry opens the circuit. A fast failure (ClientOfflineError during a
      // reconnect) passes through and leaves the circuit as it is.
      if (error instanceof SessionStoreTimeout && error.phase === 'deadline') {
        // Concurrent operations expire together. Only the FIRST opens the circuit and logs.
        if (openedAt === null) {
          openedAt = performance.now();
          logger.event('store.operation_timeout', { operation, deadlineMs });
        }
        // Defensive: Promise.race already handles `running`'s later rejection (the socket
        // teardown). This states the intent; no test can red on its removal.
        running.catch(() => undefined);
      }
      throw error;
    } finally {
      clearTimeout(timer);
    }
  };

  return {
    get: (sid) => bounded('get', () => inner.get(sid)),
    set: (sid, rec, ttlMs, expectedRev) => bounded('set', () => inner.set(sid, rec, ttlMs, expectedRev)),
    delete: (sid) => bounded('delete', () => inner.delete(sid)),
    tryAcquireLock: (sid, token, ttlMs) => bounded('tryAcquireLock', () => inner.tryAcquireLock(sid, token, ttlMs)),
    releaseLock: (sid, token) => bounded('releaseLock', () => inner.releaseLock(sid, token)),
    putTransaction: (txnId, tx, ttlMs) => bounded('putTransaction', () => inner.putTransaction(txnId, tx, ttlMs)),
    takeTransaction: (txnId) => bounded('takeTransaction', () => inner.takeTransaction(txnId)),
    // No deadline and no circuit: an open circuit must never refuse a close (SMA-651 D5), and the
    // Redis adapter's close() destroys at once.
    close: () => inner.close(),
  };
}
```

- [ ] **Step 6: Run the tests and verify they pass**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters/operation-deadline.test.ts`
Expected: PASS, 18 tests (U1–U9, U10 × 7, U11, U12).

If U8 or U11 fails because `vi.getTimerCount()` counts a timer you did not expect, do NOT loosen the assertion: find the timer.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-auth/src/core/errors.ts ts/packages/paigasus-auth/src/ports/logger.ts ts/packages/paigasus-auth/src/adapters/operation-deadline.ts ts/packages/paigasus-auth/tests/adapters/operation-deadline.test.ts
git commit -m "feat(ts): a per-operation deadline and circuit for the session store (SMA-651)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Wire the Redis adapter — options, `close()`, bounded connect, composition

**Files:**
- Modify: `ts/packages/paigasus-auth/src/adapters/redis-store.ts` (header `:1-17`, strategy comment `:52-62`, `RedisClient` `:88-111`, `close()` `:224-229`, options/factory `:231-265`)
- Modify: `ts/packages/paigasus-auth/tests/adapters/redis-client-options.test.ts`
- Create: `ts/packages/paigasus-auth/tests/adapters/redis-store-connect.test.ts`

**Interfaces:**
- Consumes: `withOperationDeadline`, `ReadySource` (Task 2), `AuthLogger`, `noopLogger` (`src/adapters/noop-logger.ts`), `SessionStoreUnavailable`.
- Produces: `CreateRedisSessionStoreOptions { url: string; commandTimeoutMs: number; keyPrefix: string; logger?: AuthLogger }`. `createRedisSessionStore` returns the DECORATED store. Task 4 passes `logger`.

- [ ] **Step 1: Extend `redis-client-options.test.ts` (failing)**

Make these edits to the existing file:

1. Replace the header comment block of the `vi.mock` factory (`:26-28`) with:

```ts
    // The factory satisfies the production RedisClient duck type (redis-store.ts) plus the
    // `connect` the factory calls. `destroy` mirrors node-redis 6.2.1 exactly: it THROWS when the
    // client is no longer open (@redis/client socket.js destroy()), so a double destroy reds here
    // instead of passing on a forgiving fake.
```

2. Extend `CapturedClient` and the fake with `isOpen` and `destroy`:

```ts
interface CapturedClient {
  on: ReturnType<typeof vi.fn>;
  isOpen: boolean;
  destroy: ReturnType<typeof vi.fn>;
  connect: () => Promise<void>;
  get: () => Promise<string | null>;
  set: () => Promise<string | null>;
  del: () => Promise<number>;
  eval: () => Promise<unknown>;
  close: () => Promise<void>;
}
```

and inside the factory:

```ts
    const client: CapturedClient = {
      on: vi.fn(),
      isOpen: true,
      destroy: vi.fn(() => {
        if (!client.isOpen) throw new Error('ClientClosedError');
        client.isOpen = false;
      }),
      connect: () => Promise.resolve(),
      get: () => Promise.resolve(null),
      set: () => Promise.resolve('OK'),
      del: () => Promise.resolve(1),
      eval: () => Promise.resolve(1),
      close: () => Promise.reject(new Error('graceful close() must not be called (SMA-651 D5)')),
    };
```

3. Keep the store the `beforeEach` creates:

```ts
let store: Awaited<ReturnType<typeof createRedisSessionStore>>;

beforeEach(async () => {
  captured.options = undefined;
  captured.client = undefined;
  store = await createRedisSessionStore({ url: 'redis://user:pw@redis.internal:6379', commandTimeoutMs: 1234, keyPrefix: '' });
});
```

4. Add, after the `guard 1` describe block:

```ts
// SMA-651 D1. pingInterval keeps a healthy idle socket under the idle timer; socketTimeout is the
// only thing that tears down a wedged socket.
describe('SMA-651: the idle timer and the ping', () => {
  it('sets pingInterval to T and socketTimeout to 2T', () => {
    expect(captured.options?.['pingInterval']).toBe(1234);
    const socket = captured.options?.['socket'] as { socketTimeout: number };
    expect(socket.socketTimeout).toBe(2468);
  });

  it('keeps pingInterval at or below half of socketTimeout', () => {
    const socket = captured.options?.['socket'] as { socketTimeout: number };
    expect(captured.options?.['pingInterval'] as number).toBeLessThanOrEqual(socket.socketTimeout / 2);
  });

  // The CAPTURED strategy, not the export: this proves the wiring as well as the function.
  // node-redis's DEFAULT strategy returns false for a SocketTimeoutError, which closes the client
  // for good (SMA-648). This one must return a number for it.
  it('the strategy passed to createClient reconnects after a SocketTimeoutError', async () => {
    const { SocketTimeoutError } = await vi.importActual<typeof import('redis')>('redis');
    const strategy = (captured.options?.['socket'] as { reconnectStrategy: (retries: number, cause: Error) => unknown }).reconnectStrategy;
    for (let retries = 0; retries <= 50; retries += 1) {
      expect(typeof strategy(retries, new SocketTimeoutError(2468))).toBe('number');
    }
  });
});

// SMA-651 D5. close() destroys at once, and only while the client is still open.
describe('SMA-651: close()', () => {
  it('destroys once and does not throw when called twice', async () => {
    await store.close();
    await store.close();
    expect(captured.client?.destroy).toHaveBeenCalledTimes(1);
  });
});
```

5. In `guard 2`, replace the first test's body with an EXACT count:

```ts
  it('registers exactly one listener for the error event', () => {
    const errorListeners = captured.client?.on.mock.calls.filter(([event]) => event === 'error') ?? [];
    expect(errorListeners).toHaveLength(1);
  });
```

- [ ] **Step 2: Write `redis-store-connect.test.ts` (failing)**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-651 D6. node-redis's connect() keeps retrying while the reconnect strategy returns a number,
// and this store's strategy always does, so an unreachable Redis at the first request used to hold
// every request of the process. createRedisSessionStore now waits at most T.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { SessionStoreUnavailable } from '../../src/core/errors.js';

const behaviour = { connect: (): Promise<void> => Promise.resolve() };

vi.mock('redis', () => ({
  createClient: () => ({
    on: () => undefined,
    isOpen: true,
    destroy: () => undefined,
    connect: () => behaviour.connect(),
    get: () => Promise.resolve(null),
    set: () => Promise.resolve('OK'),
    del: () => Promise.resolve(1),
    eval: () => Promise.resolve(1),
  }),
}));

const { createRedisSessionStore } = await import('../../src/adapters/redis-store.js');

const T = 1000;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('createRedisSessionStore: the first connect (SMA-651 D6)', () => {
  it('is still waiting at T - 1 and returns the store at T when connect never settles', async () => {
    behaviour.connect = () => new Promise<void>(() => undefined);
    let settled = false;
    const created = createRedisSessionStore({ url: 'redis://127.0.0.1:6379', commandTimeoutMs: T, keyPrefix: '' }).then((store) => {
      settled = true;
      return store;
    });
    await vi.advanceTimersByTimeAsync(T - 1);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    await expect(created).resolves.toBeDefined();
  });

  it('throws SessionStoreUnavailable when connect rejects before T', async () => {
    behaviour.connect = () => Promise.reject(new Error('connect ECONNREFUSED redis://user:pw@host:6379'));
    const error = await createRedisSessionStore({ url: 'redis://user:pw@host:6379', commandTimeoutMs: T, keyPrefix: '' }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(SessionStoreUnavailable);
    expect((error as Error).message).not.toContain('pw');
  });

  it('leaves no pending timer when connect resolves at once', async () => {
    behaviour.connect = () => Promise.resolve();
    await createRedisSessionStore({ url: 'redis://127.0.0.1:6379', commandTimeoutMs: T, keyPrefix: '' });
    expect(vi.getTimerCount()).toBe(0);
  });
});
```

- [ ] **Step 3: Run both files and verify they fail**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters/redis-client-options.test.ts tests/adapters/redis-store-connect.test.ts`
Expected: FAIL — `pingInterval` is `undefined`, `socketTimeout` is `undefined`, the `close()` test rejects with "graceful close() must not be called", and the first connect test times out waiting (the promise never settles, so `settled` stays false at T and `resolves` never completes — vitest reports a test timeout). The strategy test PASSES already (the existing strategy returns a number): that is expected, it is a regression pin, not a red-first test.

- [ ] **Step 4: Change the adapter** — edits to `src/adapters/redis-store.ts`:

1. Imports (after the existing imports):

```ts
import type { AuthLogger } from '../ports/logger';
import { noopLogger } from './noop-logger';
import { withOperationDeadline } from './operation-deadline';
```

2. Replace the comment above `reconnectStrategy` (`:52-62`) with:

```ts
// The backoff DELAY is bounded; the RETRY COUNT never is, and this function never returns an
// `Error` or `false`, FOR ANY CAUSE. node-redis passes `(retries, cause)`; this ignores `cause`
// on purpose. A strategy that returns an `Error` or `false` stops reconnection PERMANENTLY, and
// the store holds one client with no recreate path, so every later call would raise
// SessionStoreUnavailable for the rest of the process. That includes a SocketTimeoutError: since
// SMA-651 sets `socketTimeout`, the idle timer's error comes through here, and node-redis's
// DEFAULT strategy returns `false` for it (the SMA-648 bug). Do not reintroduce a retry cap or a
// per-cause branch.
//
// Fast failure during an outage comes from `disableOfflineQueue: true` (a not-ready client refuses
// a command at once). `commandOptions.timeout` does NOT bound a command in flight: it covers only
// the queued phase. The in-flight bound is withOperationDeadline (./operation-deadline.ts).
```

3. Replace the `RedisClient` interface (keep the long comment above it, but change "five methods" to "the methods below" in both places it appears):

```ts
interface RedisClient {
  get(key: string): Promise<string | null>;
  set(key: string, value: string, options?: SetOptions): Promise<string | null>;
  del(key: string): Promise<number>;
  eval(script: string, options: { keys: string[]; arguments: string[] }): Promise<unknown>;
  readonly isOpen: boolean;
  destroy(): void;
}
```

4. Replace `close()` in `RedisSessionStore`:

```ts
  // DESTROYS AT ONCE (SMA-651 D5). node-redis's graceful close() waits for pending commands and
  // checks only on a `data` event, so against a Redis that never replies it never resolves. No
  // production code closes the store, so a graceful close buys nothing. The `isOpen` guard is
  // required: destroy() THROWS ClientClosedError on a client that is no longer open.
  close(): Promise<void> {
    return this.#guarded(() => {
      if (this.#client.isOpen) this.#client.destroy();
      return Promise.resolve();
    });
  }
```

5. Replace everything from `export interface CreateRedisSessionStoreOptions` to the end of the file:

```ts
export interface CreateRedisSessionStoreOptions {
  url: string;
  commandTimeoutMs: number;
  keyPrefix: string;
  /** Receives `store.operation_timeout`, once per circuit open. Defaults to the no-op logger. */
  logger?: AuthLogger;
}

/**
 * Waits for the first connect, but never longer than one command timeout (SMA-651 D6). node-redis's
 * connect() keeps retrying while the reconnect strategy returns a number, and this store's always
 * does, so an unreachable Redis would otherwise hold the first request, and every request that
 * awaits the same runtime, without end. The connect keeps running in the background: until
 * `ready`, every command fails at once (disableOfflineQueue), and the store works when Redis does.
 */
async function connectWithin(client: { connect(): Promise<unknown> }, timeoutMs: number): Promise<'connected' | 'failed' | 'waiting'> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  // Both handlers are attached HERE, so a later rejection of the background connect is never an
  // unhandled rejection.
  const connected = client.connect().then(
    () => 'connected' as const,
    () => 'failed' as const,
  );
  const waited = new Promise<'waiting'>((resolve) => {
    timer = setTimeout(() => {
      resolve('waiting');
    }, timeoutMs);
    timer.unref();
  });
  try {
    return await Promise.race([connected, waited]);
  } finally {
    clearTimeout(timer);
  }
}

export async function createRedisSessionStore(opts: CreateRedisSessionStoreOptions): Promise<SessionStore> {
  const dsn = new RedactedDsn();
  const client = createClient({
    url: opts.url,
    // LOAD-BEARING: node-redis queues commands while disconnected by default, so an outage
    // becomes hung requests instead of fast failures, and every page in the console stalls
    // rather than erroring. See § 7.2 of the design doc.
    disableOfflineQueue: true,
    commandOptions: { timeout: opts.commandTimeoutMs },
    // SMA-651 D1: a PING every T keeps a healthy idle socket under the idle timer. The Redis user
    // needs `+ping`, or every PING gets -NOPERM (still socket activity, so it is silent).
    pingInterval: opts.commandTimeoutMs,
    socket: {
      connectTimeout: opts.commandTimeoutMs,
      // SMA-651 D1: an IDLE timer, not a reply deadline. It is the only thing that tears down a
      // wedged socket; withOperationDeadline's circuit goes silent so that it can fire.
      socketTimeout: opts.commandTimeoutMs * 2,
      reconnectStrategy,
    },
  });

  // node-redis emits 'error' on the client's EventEmitter for connection-level failures; with no
  // listener that is an unhandled event that crashes the process. This handler is intentionally
  // silent: every store method already converts a failure into SessionStoreUnavailable at the
  // call site, and this listener must never log the node-redis error object either, since it
  // embeds the DSN. It stays the ONLY error listener (tests/adapters/redis-client-options.test.ts).
  client.on('error', () => undefined);

  if ((await connectWithin(client, opts.commandTimeoutMs)) === 'failed') {
    throw new SessionStoreUnavailable(`session store unavailable (${dsn.toString()})`);
  }

  // The decorator wraps OUTSIDE #guarded, so a SessionStoreTimeout is never rewrapped into a plain
  // SessionStoreUnavailable by #guarded's catch-all (SMA-651 D4).
  return withOperationDeadline(new RedisSessionStore(client, opts.keyPrefix, dsn), client, opts.commandTimeoutMs, opts.logger ?? noopLogger);
}
```

6. Header comment (`:1-17`): append this paragraph at the end of the header block, before the imports:

```ts
//
// SMA-651: createRedisSessionStore returns this adapter WRAPPED in withOperationDeadline
// (./operation-deadline.ts), which bounds every operation at 4 x the command timeout, and it
// waits at most one command timeout for the first connect.
```

- [ ] **Step 5: Type-check and run the unit tests**

Run: `pnpm -C ts/packages/paigasus-auth exec tsc --noEmit -p .`
Expected: no errors. If `createClient`'s result is not assignable to `RedisClient` or `ReadySource`, fix the duck type, never with `as unknown as` or `any`.

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters`
Expected: PASS for every file in `tests/adapters/`, including `redis-store-parse.test.ts` and `redis-store.test.ts`, which must stay green unchanged.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-auth/src/adapters/redis-store.ts ts/packages/paigasus-auth/tests/adapters/redis-client-options.test.ts ts/packages/paigasus-auth/tests/adapters/redis-store-connect.test.ts
git commit -m "feat(ts): keep the session store's Redis client alive and bound its connect (SMA-651)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: The runtime passes its logger, and T gets a maximum

**Files:**
- Modify: `ts/packages/paigasus-auth/src/runtime.ts:141-154`
- Modify: `ts/packages/paigasus-auth/src/config.ts:51`
- Modify: `ts/packages/paigasus-auth/tests/config.test.ts`
- Create: `ts/packages/paigasus-auth/tests/runtime-redis-wiring.test.ts`

**Interfaces:**
- Consumes: `CreateRedisSessionStoreOptions.logger` (Task 3), `createAuthRuntime(cfg, deps)` (`src/runtime.ts:97`).
- Produces: nothing new.

- [ ] **Step 1: Write the failing wiring test** — `tests/runtime-redis-wiring.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-651 D8. The runtime must hand ITS logger to the Redis store, or store.operation_timeout is
// logged to the no-op logger and production sees nothing. Deleting the `logger` line in
// createAuthRuntime reds this test and nothing else.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AuthEventFields, AuthEventName } from '../src/ports/logger.js';

vi.mock('redis', () => ({
  createClient: () => ({
    on: () => undefined,
    isOpen: true,
    destroy: () => undefined,
    connect: () => Promise.resolve(),
    // A Redis that accepts the command and never replies.
    get: () => new Promise<never>(() => undefined),
    set: () => Promise.resolve('OK'),
    del: () => Promise.resolve(1),
    eval: () => Promise.resolve(1),
  }),
}));

const { createAuthRuntime } = await import('../src/runtime.js');

// Copied from tests/runtime.test.ts's BASE, with the redis store selected.
const CONFIG = {
  PAIGASUS_ZONE: 'iam',
  PAIGASUS_ZONES: { iam: '/iam' },
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.com',
  PAIGASUS_OIDC_CLIENT_ID: 'c',
  PAIGASUS_OIDC_CLIENT_SECRET: 's',
  PAIGASUS_PUBLIC_ORIGIN: 'https://app.example.com',
  PAIGASUS_OIDC_SCOPES: 'openid profile email offline_access',
  PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS: 30,
  PAIGASUS_OIDC_HTTP_TIMEOUT_MS: 3500,
  PAIGASUS_SESSION_STORE: 'redis' as const,
  PAIGASUS_SESSION_REDIS_URL: 'redis://127.0.0.1:6379',
  PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 1000,
  PAIGASUS_SESSION_TTL_SECONDS: 28800,
  PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS: 86400,
  PAIGASUS_SESSION_REFRESH_SKEW_SECONDS: 30,
  PAIGASUS_SESSION_LOCK_TTL_MS: 10000,
  PAIGASUS_SESSION_LOCK_WAIT_MS: 3000,
};

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('createAuthRuntime wires its logger into the Redis store (SMA-651 D8)', () => {
  it('a store timeout reaches the logger the runtime was given', async () => {
    const events: Array<[AuthEventName, AuthEventFields]> = [];
    const runtime = await createAuthRuntime(CONFIG, { logger: { event: (name, fields) => events.push([name, fields]) } });
    const pending = runtime.store.get('sid').catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(4000);
    await pending;
    expect(events).toContainEqual(['store.operation_timeout', { operation: 'get', deadlineMs: 4000 }]);
  });
});
```

- [ ] **Step 2: Add the failing config test** — in `tests/config.test.ts`, inside the `authEnvShape` describe block:

```ts
  // SMA-651 D9. Node turns a timer delay above 2^31 - 1 ms into 1 ms, and the largest timer the
  // session store sets is 4 x this value.
  it('caps PAIGASUS_SESSION_REDIS_TIMEOUT_MS at 536870911 ms', () => {
    expect(schema.parse({ ...VALID, PAIGASUS_SESSION_REDIS_TIMEOUT_MS: '536870911' }).PAIGASUS_SESSION_REDIS_TIMEOUT_MS).toBe(536_870_911);
    expect(() => schema.parse({ ...VALID, PAIGASUS_SESSION_REDIS_TIMEOUT_MS: '536870912' })).toThrow();
  });
```

- [ ] **Step 3: Run both and verify they fail**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/runtime-redis-wiring.test.ts tests/config.test.ts`
Expected: FAIL — the wiring test finds no `store.operation_timeout` (the runtime does not pass its logger yet), and the config test does not throw for `536870912`.

If the wiring test fails for another reason (for example `createOidcClient` makes a network call), report it; do not work around it with a mock of `src/adapters/oidc.ts` without saying so.

- [ ] **Step 4: Implement**

In `src/config.ts`, replace line 51:

```ts
  // SMA-651 D9: at most 536870911 ms. Node turns a timer delay above 2^31 - 1 ms into 1 ms, and the
  // session store's largest timer is 4 x this value (its per-operation deadline).
  PAIGASUS_SESSION_REDIS_TIMEOUT_MS: z.coerce.number().int().positive().max(536_870_911).default(1000),
```

In `src/runtime.ts`, compute the logger before the store, pass it, and reuse it in the return value:

```ts
  const logger = deps.logger ?? noopLogger;

  const store =
    deps.store ??
    (cfg.PAIGASUS_SESSION_STORE === 'redis'
      ? await createRedisSessionStore({
          url: requireRedisUrl(cfg),
          commandTimeoutMs: cfg.PAIGASUS_SESSION_REDIS_TIMEOUT_MS,
          // All zones share ONE store (design doc § 6.7) — no per-zone key prefix to configure.
          keyPrefix: '',
          // SMA-651 D8: store.operation_timeout goes to the runtime's logger.
          logger,
        })
      : new MemorySessionStore());

  return {
    store,
    resolver: deps.resolver ?? claimsPrincipalResolver,
    logger,
```

(leave every other field of the returned object unchanged).

- [ ] **Step 5: Run the whole unit tier**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run`
Expected: PASS, every file.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-auth/src/config.ts ts/packages/paigasus-auth/src/runtime.ts ts/packages/paigasus-auth/tests/config.test.ts ts/packages/paigasus-auth/tests/runtime-redis-wiring.test.ts
git commit -m "feat(ts): pass the auth runtime's logger to the session store (SMA-651)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: The Docker proof goes green

**Files:** none changed unless a test is WRONG (then fix the test only after writing down why, and report it).

- [ ] **Step 1: Run the new container file**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run --config vitest.containers.config.ts tests/containers/redis-store-idle.test.ts`
Expected: PASS, E1, E2+E4, E3, E5.

- [ ] **Step 2: Run the whole container suite**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run --config vitest.containers.config.ts`
Expected: PASS, every file. The contract suite and the single-flight suites now run through the decorator and the destroying `close()`; they must stay green unchanged.

If a Docker-backed suite fails, first read memory note "Docker-gated suites silently skip / are flaky under parallel load": re-run that one file alone before you debug your diff.

- [ ] **Step 3: No commit** unless a test changed. If one did, commit it with `test(ts): …` and explain the reason in the body.

---

### Task 6: Mutation battery — prove each guard bites

Red-first only proves a test fails before the code exists. This task proves each test fails when the feature is DELETED (memory: "red-first is not proof — delete the feature").

**Rules.** The working tree must be CLEAN before each mutation (`git status --porcelain` prints nothing). Apply ONE mutation with the Edit tool, run the named test, record the result, then restore with `git checkout -- <file>` (safe here only because the tree was clean). Run the battery WHOLE, top to bottom, every time any fix is made.

| # | Mutation (file) | Test to run | Expected |
|---|---|---|---|
| M1 | Delete the `pingInterval: opts.commandTimeoutMs,` line (`redis-store.ts`) | container E1 | FAIL (a reconnect is counted) |
| M2 | Delete the `socketTimeout: opts.commandTimeoutMs * 2,` line | container E3 | FAIL (no reconnect) |
| M3 | Delete `openedAt = performance.now();` (`operation-deadline.ts`) | unit U3, container E3 | both FAIL |
| M4 | Replace `get: (sid) => bounded('get', () => inner.get(sid)),` with `get: (sid) => inner.get(sid),` | unit U10 | the `get` row FAILS |
| M5 | Replace the `putTransaction` line the same way | unit U10 | the `putTransaction` row FAILS |
| M6 | Delete `logger,` from the `createRedisSessionStore` call in `runtime.ts` | `tests/runtime-redis-wiring.test.ts` | FAIL |
| M7 | Delete the `openedAt = null;` line after the cooldown check | unit U5 | FAIL |
| M8 | Replace the `close()` body with `return this.#guarded(async () => { await (this.#client as unknown as { close(): Promise<void> }).close(); });` | container E5 | FAIL |
| M9 | Replace `connectWithin(...)` with `client.connect().then(() => 'connected' as const)` | `tests/adapters/redis-store-connect.test.ts` first case | FAIL |
| M10 | Delete `.max(536_870_911)` (`config.ts`) | `tests/config.test.ts` | FAIL |
| M11 | Change `DEADLINE_FACTOR = 4` to `3` | unit U1 | FAIL |

- [ ] **Step 1:** Run M1–M11 as above. For each, copy the one-line vitest verdict (test name and FAIL/PASS) into a table in your report.
- [ ] **Step 2:** After the last restore, `git status --porcelain` must print nothing, and `pnpm -C ts/packages/paigasus-auth exec vitest run` must PASS.
- [ ] **Step 3:** If any mutation leaves its test GREEN, STOP. That guard does not bite; report which one, do not "fix" the test on your own.
- [ ] **Step 4:** Append the table to this plan file under a new heading `## Mutation battery results (Task 6)` and commit:

```bash
git add docs/superpowers/plans/2026-09-19-sma-651-session-store-operation-deadline.md
git commit -m "docs(ts): record the SMA-651 mutation battery (SMA-651)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: Documentation

**Files:**
- Modify: `ts/packages/paigasus-auth/README.md` (new subsection after `### The session store: \`redis\` vs \`memory\``, before `### Redaction`)
- Modify: `CLAUDE.md` (the bullet that starts `- **node-redis \`socket.socketTimeout\` is an IDLE timer, not a reply deadline**`)

- [ ] **Step 1: README** — insert this subsection:

```markdown
### When Redis stops answering (SMA-651)

T below is `PAIGASUS_SESSION_REDIS_TIMEOUT_MS` (default 1000 ms, at most 536870911 ms, no minimum).

- **Every store operation has a deadline of 4T.** node-redis's own command timeout covers only a
  command that waits in its queue. A Redis that accepts a command and never replies is bounded by
  this deadline alone.
- **The first expiry opens a circuit for 4T.** While it is open, every store call fails at once and
  the store writes nothing to Redis. That silence lets the client's idle timer (2T) tear down the
  wedged connection and reconnect. The circuit closes when the client reconnects, or when the 4T
  cooldown ends.
- **One `store.operation_timeout` event** (`{ operation, deadlineMs }`) is logged each time the
  circuit opens. `getSession()` still logs `store.unavailable` for each failed read.
- **The first connect waits at most T.** If Redis is unreachable at start-up, the store is still
  created, every call fails at once until Redis answers, and the store then works with no restart.
- **The client sends one PING every T** to keep a healthy idle connection open. A console process
  has one such client per zone for sessions, plus one for the descriptor cache.

**A failed store call signs the user out.** `requireSession()` treats a store failure as "no
session" and redirects to `/auth/login`, and `/auth/login` deletes the presented session. So a
Redis stall longer than 4T (a fork stall during BGSAVE, an fsync stall, a failover) signs out every
user who loads a page in that window, and those sessions are gone. Raise T if your Redis can stall
longer than that. Size T against the worst event-loop lag of the Node process too: a stall longer
than 4T in the process itself fires the deadlines before the replies are read.

**Redis ACL.** The store's Redis user needs `+get`, `+set`, `+del`, `+eval` and `+ping`.
`+client|setinfo` is optional (node-redis sends CLIENT SETINFO at connect and ignores the error).
Without `+ping`, every PING gets `-NOPERM`; the connection stays open, and nothing reports the
missing permission.
```

- [ ] **Step 2: CLAUDE.md** — in that bullet, replace the parenthetical

`(SMA-650 added a per-operation deadline in \`@paigasus/console-core\`; \`@paigasus/auth\` still has none — SMA-651)`

with

`(SMA-650 added a per-operation deadline in \`@paigasus/console-core\`; SMA-651 added the same shape to \`@paigasus/auth\`'s session store, as the decorator in \`src/adapters/operation-deadline.ts\`, with a destroying \`close()\` because node-redis's graceful \`close()\` waits for a reply that never comes)`

Do NOT touch any other line of CLAUDE.md, and never touch the `ci-targets` or `moon-diagnosis` markers.

- [ ] **Step 3: Format**

Run: `pnpm -C ts exec prettier --check packages/paigasus-auth`
If it reports files, run `pnpm -C ts exec prettier --write <those files>` and re-check.

- [ ] **Step 4: Commit**

```bash
git add ts/packages/paigasus-auth/README.md CLAUDE.md
git commit -m "docs(ts): document the session store's deadline, circuit and ACL (SMA-651)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 8: Full verification

- [ ] **Step 1: Package tasks through Moon**

Run: `moon run paigasus-auth-ts:test paigasus-auth-ts:test-e2e ts:lint ts:fmt`
Expected: every task passes. If `paigasus-auth-ts:typecheck` exists (`moon task paigasus-auth-ts:typecheck`), run it too.

- [ ] **Step 2: The full CI graph**, as CLAUDE.md requires before a push. Run the marker-delimited `moon ci … --base origin/main --include-relations` command from CLAUDE.md. Read the bash-version entry in CLAUDE.md first: `repo:affected-smoke` needs `/bin/bash` 3.2, and `repo:ruff-ci`, `repo:next-public-free` and `repo:publish-metadata` need bash 4+; `repo:actionlint` has no working local bash. Report each red task with its captured `stdout.log`/`stderr.log` (copy them out of `.moon/cache/states/<project>/<task>/` BEFORE any re-run), and classify it as caused by this branch or not.

- [ ] **Step 3: Report**

Report: the commit list (`git log --oneline origin/main..HEAD`), each task's pass/fail, the Task 6 table, and any red that is not caused by this branch with its evidence.
