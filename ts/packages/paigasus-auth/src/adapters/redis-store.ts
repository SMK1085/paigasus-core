// SPDX-License-Identifier: Apache-2.0
//
// Three operations need atomicity Redis commands alone do not give:
//   SET_CAS   — compare-and-set on `rev`, so a refresh that outlived its lock cannot write its
//               now-revoked token over a newer one.
//   UNLOCK    — compare-and-delete, so a holder whose TTL expired cannot release the next
//               holder's lock.
//   TAKE_TXN  — atomic get-and-delete, so `state` is single-use.
//
// TAKE_TXN could be GETDEL instead (M2 confirms it is exposed, as both GETDEL and camelCase
// getDel) — it stays a script so all three go through one code path (#evalGuarded) and one
// error mode, rather than one primitive alone taking a different route.
//
// Every method here is written against the measured node-redis v6 API recorded in
// docs/superpowers/specs/2026-09-09-sma-506-measurements.md under M2, not against v5-era
// examples: v6 changed the SET options shape, and KEYS/ARGV both travel through one
// `{ keys, arguments }` object on `eval`.
import { createClient } from 'redis';
import type { SetOptions } from 'redis';
import type { SessionRecord } from '../core/session.js';
import { SessionStoreUnavailable } from '../core/errors.js';
import type { LoginTransaction, SessionStore } from '../ports/session-store.js';

const SET_CAS = `
local cur = redis.call('GET', KEYS[1])
if ARGV[3] == '' then
  if cur then return 0 end
else
  if not cur then return 0 end
  local ok, parsed = pcall(cjson.decode, cur)
  if not ok or tonumber(parsed.rev) ~= tonumber(ARGV[3]) then return 0 end
end
redis.call('SET', KEYS[1], ARGV[1], 'PX', tonumber(ARGV[2]))
return 1`;

const UNLOCK = `
if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) end
return 0`;

const TAKE_TXN = `
local v = redis.call('GET', KEYS[1])
if v then redis.call('DEL', KEYS[1]) end
return v`;

// The fencing case (task-4 brief, "a note on the Lua compare-and-set") is handled by comparing
// numerically via tonumber() on BOTH sides — cjson.decode renders an integer `rev` as a Lua
// number that tostring() can print as a float (e.g. `3.0`), so a string comparison against
// ARGV[3] (always a string) would reject a legitimate match.

const MAX_RECONNECT_RETRIES = 10;

/**
 * A marker for "this value is a Redis connection string, never print it." Both `toString` and
 * `toJSON` return a fixed redacted literal, so an accidental `console.log(store)` or
 * `JSON.stringify` on anything holding one cannot leak a DSN — which node-redis's OWN connection
 * errors do embed, which is exactly why this package never re-throws or logs a caught node-redis
 * error object.
 *
 * DOES NOT STORE THE RAW VALUE (final fix wave, finding 7). It used to, only so a since-deleted
 * `reveal()` method could hand it back out — `reveal()` was dead code, never called anywhere in
 * this package, so holding a secret in memory for a method nothing used bought nothing. Deleting
 * `reveal()` removed the only reason to store it at all.
 */
class RedactedDsn {
  toString(): string {
    return 'redis://<redacted>';
  }
  toJSON(): string {
    return 'redis://<redacted>';
  }
}

// A duck-typed subset of RedisClientType, not `ReturnType<typeof createClient>`. The full
// generic type has a different concrete instantiation depending on which options were passed
// to createClient (e.g. `commandOptions` changes the inferred module/function/script type
// parameters), so naming "the" return type of createClient and using it for a value created
// with different options does not typecheck. This adapter only ever calls five methods, so it
// only needs to agree on those five.
//
// METHOD SHORTHAND, DELIBERATELY (final fix wave, finding 7). Writing these as method signatures
// (`get(key: string): ...`) rather than property arrow types (`get: (key: string) => ...`) makes
// TypeScript check each parameter BIVARIANTLY instead of contravariantly — the assignability
// check accepts a real `RedisClientType`'s method whose parameter types are a strict subtype OR
// supertype of this interface's, where a property-typed field would only accept a subtype. This
// is an accepted trade-off, not an oversight: the alternative (property syntax) is measurably
// stricter but repeatedly fights the real client's own generic, overload-heavy method signatures
// for no bug this adapter has ever hit — the five methods above are simple enough (string/number
// primitives, no covariant return position that bivariance could silently mismatch) that the
// soundness hole costs nothing in practice here.
interface RedisClient {
  get(key: string): Promise<string | null>;
  set(key: string, value: string, options?: SetOptions): Promise<string | null>;
  del(key: string): Promise<number>;
  eval(script: string, options: { keys: string[]; arguments: string[] }): Promise<unknown>;
  close(): Promise<void>;
}

function sessKey(keyPrefix: string, sid: string): string {
  return `${keyPrefix}pgs:sess:${sid}`;
}
function lockKey(keyPrefix: string, sid: string): string {
  return `${keyPrefix}pgs:lock:${sid}`;
}
function txnKey(keyPrefix: string, txnId: string): string {
  return `${keyPrefix}pgs:txn:${txnId}`;
}

class RedisSessionStore implements SessionStore {
  readonly #client: RedisClient;
  readonly #keyPrefix: string;
  readonly #dsn: RedactedDsn;

  constructor(client: RedisClient, keyPrefix: string, dsn: RedactedDsn) {
    this.#client = client;
    this.#keyPrefix = keyPrefix;
    this.#dsn = dsn;
  }

  // Every command runs through here. A connection or timeout error becomes
  // SessionStoreUnavailable with a message built from the REDACTED DSN — never the raw
  // node-redis error object, which embeds the real connection DSN. `this.#dsn.toString()` IS
  // interpolated per call below (finding 8, final fix wave: this comment previously claimed
  // otherwise), but that is safe, not a leak: `RedactedDsn#toString` always returns the fixed
  // literal `'redis://<redacted>'` — it holds no state to vary it with (see the class doc) — so
  // the interpolation can never expose anything.
  async #guarded<T>(op: () => Promise<T>): Promise<T> {
    try {
      return await op();
    } catch {
      throw new SessionStoreUnavailable(`session store unavailable (${this.#dsn.toString()})`);
    }
  }

  #eval(script: string, keys: string[], args: string[]): Promise<unknown> {
    return this.#client.eval(script, { keys, arguments: args });
  }

  get(sid: string): Promise<SessionRecord | null> {
    return this.#guarded(async () => {
      const raw = await this.#client.get(sessKey(this.#keyPrefix, sid));
      if (raw === null) return null;
      let parsed: SessionRecord;
      try {
        parsed = JSON.parse(raw) as SessionRecord;
      } catch {
        // An unparseable stored value is treated as ABSENT, matching the version-mismatch
        // handling below and the memory adapter's own absent-and-deleted behaviour. Without
        // this, JSON.parse throwing surfaces as SessionStoreUnavailable through #guarded's
        // catch-all, which turns one poisoned key into a permanent sign-out loop for that user.
        await this.#client.del(sessKey(this.#keyPrefix, sid));
        return null;
      }
      // A version mismatch is treated as ABSENT, matching the memory adapter.
      if (parsed.version !== 1) {
        await this.#client.del(sessKey(this.#keyPrefix, sid));
        return null;
      }
      return parsed;
    });
  }

  set(sid: string, rec: SessionRecord, ttlMs: number, expectedRev: number | null): Promise<boolean> {
    return this.#guarded(async () => {
      const result = await this.#eval(SET_CAS, [sessKey(this.#keyPrefix, sid)], [JSON.stringify(rec), String(ttlMs), expectedRev === null ? '' : String(expectedRev)]);
      return (result as number) === 1;
    });
  }

  delete(sid: string): Promise<void> {
    return this.#guarded(async () => {
      await this.#client.del(sessKey(this.#keyPrefix, sid));
    });
  }

  tryAcquireLock(sid: string, token: string, ttlMs: number): Promise<boolean> {
    return this.#guarded(async () => {
      const result = await this.#client.set(lockKey(this.#keyPrefix, sid), token, {
        condition: 'NX',
        expiration: { type: 'PX', value: ttlMs },
      });
      return result !== null;
    });
  }

  releaseLock(sid: string, token: string): Promise<void> {
    return this.#guarded(async () => {
      await this.#eval(UNLOCK, [lockKey(this.#keyPrefix, sid)], [token]);
    });
  }

  putTransaction(txnId: string, tx: LoginTransaction, ttlMs: number): Promise<void> {
    return this.#guarded(async () => {
      await this.#client.set(txnKey(this.#keyPrefix, txnId), JSON.stringify(tx), {
        expiration: { type: 'PX', value: ttlMs },
      });
    });
  }

  takeTransaction(txnId: string): Promise<LoginTransaction | null> {
    return this.#guarded(async () => {
      const raw = (await this.#eval(TAKE_TXN, [txnKey(this.#keyPrefix, txnId)], [])) as string | null;
      if (raw === null) return null;
      return JSON.parse(raw) as LoginTransaction;
    });
  }

  close(): Promise<void> {
    return this.#guarded(async () => {
      await this.#client.close();
    });
  }
}

export interface CreateRedisSessionStoreOptions {
  url: string;
  commandTimeoutMs: number;
  keyPrefix: string;
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
    socket: {
      connectTimeout: opts.commandTimeoutMs,
      // Bounded: return an Error past the retry cap so the client stops instead of retrying
      // forever. The returned number is a backoff delay in ms before the next attempt.
      reconnectStrategy: (retries: number): number | Error => {
        if (retries > MAX_RECONNECT_RETRIES) {
          return new Error('redis reconnect retry cap exceeded');
        }
        return Math.min(retries * 100, 2000);
      },
    },
  });

  // node-redis emits 'error' on the client's EventEmitter for connection-level failures; with no
  // listener that is an unhandled event that crashes the process. This handler is intentionally
  // silent: every store method already converts a failure into SessionStoreUnavailable at the
  // call site, and this listener must never log the node-redis error object either, since it
  // embeds the DSN.
  client.on('error', () => undefined);

  try {
    await client.connect();
  } catch {
    throw new SessionStoreUnavailable(`session store unavailable (${dsn.toString()})`);
  }

  return new RedisSessionStore(client, opts.keyPrefix, dsn);
}
