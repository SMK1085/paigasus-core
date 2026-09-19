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
