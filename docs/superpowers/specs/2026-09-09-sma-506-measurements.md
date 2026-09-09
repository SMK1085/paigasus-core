# SMA-506 measurements

Behavioural claims backing the `@paigasus/auth` implementation, measured against the
installed toolchain rather than assumed. See
`docs/superpowers/specs/2026-09-09-sma-506-auth-design.md` § 20 for the full list this
package owes.

## M2 — node-redis v6 API

Measured against the pinned `redis@6.2.1` (`ts/pnpm-workspace.yaml` catalog), by reading
the installed type declarations directly rather than from memory or v5-era examples.

### `SET` with `NX` and `PX`

File: `ts/node_modules/.pnpm/@redis+client@6.2.1/node_modules/@redis/client/dist/lib/commands/SET.d.ts`

```ts
export interface SetOptions {
    expiration?: {
        type: 'EX' | 'PX' | 'EXAT' | 'PXAT';
        value: number;
    } | { type: 'KEEPTTL'; } | 'KEEPTTL';
    /** @deprecated Use `expiration` { type: 'EX', value: number } instead */
    EX?: number;
    /** @deprecated Use `expiration` { type: 'PX', value: number } instead */
    PX?: number;
    condition?: 'NX' | 'XX' | 'IFEQ' | 'IFNE' | 'IFDEQ' | 'IFDNE';
    matchValue?: RedisArgument;
    /** @deprecated Use `{ condition: 'NX' }` instead. */
    NX?: boolean;
    /** @deprecated Use `{ condition: 'XX' }` instead. */
    XX?: boolean;
    GET?: boolean;
}
```

The v6 shape is `{ condition: 'NX', expiration: { type: 'PX', value: number } }`. The
flat `NX`/`PX` boolean/number fields still exist but are marked `@deprecated` in the
type itself — the adapter uses the non-deprecated `condition`/`expiration` shape.

`client.set(key, value, options)` return type (from `SET.d.ts`):
`SimpleStringReply<'OK'> | BlobStringReply | NullReply`, which resolves under the
default (empty) `TYPE_MAPPING` to `'OK' | string | null`. A failed `NX` set returns
`null`; a successful one returns `'OK'`.

### `GETDEL`

File: `ts/node_modules/.pnpm/@redis+client@6.2.1/node_modules/@redis/client/dist/lib/commands/GETDEL.d.ts`

```ts
declare const _default: {
    readonly parseCommand: (this: void, parser: CommandParser, key: RedisArgument) => void;
    readonly transformReply: () => BlobStringReply | NullReply;
};
```

`GETDEL` **is** exposed, as both `GETDEL` and camelCase `getDel` (confirmed at
`ts/node_modules/.pnpm/@redis+client@6.2.1/node_modules/@redis/client/dist/lib/commands/index.d.ts:3138,3146`).
It is atomic get-and-delete natively, no script required.

**Decision: `takeTransaction` still uses the `TAKE_TXN` Lua script, not `getDel`.**
The task-4 brief's own reasoning is followed: all three primitives that need
atomicity beyond a single command (`SET_CAS`, `UNLOCK`, `TAKE_TXN`) go through one
code path (`#evalGuarded`) and therefore one error-handling and one key-building
path, rather than `takeTransaction` alone taking a different route through
`getDel`. `getDel`'s availability was the thing to verify before choosing, and it
is available — the choice not to use it is deliberate, not a fallback.

### `eval` / `evalSha`

File: `ts/node_modules/.pnpm/@redis+client@6.2.1/node_modules/@redis/client/dist/lib/commands/EVAL.d.ts`

```ts
export interface EvalOptions {
    keys?: Array<RedisArgument>;
    arguments?: Array<RedisArgument>;
}
declare const _default: {
    readonly parseCommand: (this: void, parser: CommandParser, script: RedisArgument, options?: EvalOptions) => void;
    readonly transformReply: () => ReplyUnion;
};
```

`client.eval(script, { keys: [...], arguments: [...] })` — KEYS and ARGV are both
passed via one options object, not as separate positional arrays and not as a
flat variadic list. `evalSha` (`EVALSHA.d.ts`) takes the identical `EvalOptions`
shape, with a sha1 in place of the script body. The adapter uses plain `eval`
(not `evalSha`) since there is no script-caching requirement here and it avoids a
`NOSCRIPT` failure mode entirely.

The reply type (`ReplyUnion`) is a large structural union of RESP-typed markers
(`NumberReply`, `BlobStringReply`, `NullReply`, ...) that resolve under the
client's default (empty) `TYPE_MAPPING` to plain JS primitives (`number`,
`string`, `null`, ...) per `ReplyWithTypeMapping` in
`.../dist/lib/RESP/types.d.ts:70`. The adapter asserts the narrower type it
expects from each script with `as` (`as number` for `SET_CAS`/`UNLOCK`, which
`RETURN 0/1` and `DEL`'s integer reply; `as string | null` for `TAKE_TXN`, which
returns the stored value or a nil reply).

### `createClient` reconnection / offline-queueing options

File: `ts/node_modules/.pnpm/@redis+client@6.2.1/node_modules/@redis/client/dist/lib/client/index.d.ts`
and `.../dist/lib/client/socket.d.ts`.

- `disableOfflineQueue?: boolean` — top-level `RedisClientOptions` field (not
  under `socket`). "When `true`, commands are rejected when the client is
  reconnecting. When `false`, commands are queued for execution after
  reconnection." Default is `false`.
- `socket.reconnectStrategy?: false | number | ((retries: number, cause: Error) => false | Error | number)`
  — `false` or a returned `Error` both stop reconnection; a returned `number` is
  a delay in ms before the next attempt.
- `socket.connectTimeout?: number` — "Connection timeout (in milliseconds)".
- `commandOptions?: CommandOptions` at the top level of `RedisClientOptions`
  applies as the *default* for every command issued through the client;
  `CommandOptions.timeout?: number` is "Timeout for the command in
  milliseconds" (`.../dist/lib/client/commands-queue.d.ts:6-17`). This is what
  the adapter uses to honour `commandTimeoutMs` — there is no separate
  per-call timeout argument on `get`/`set`/`eval`/etc.

Error classes thrown on failure (`.../dist/lib/errors.d.ts`): `TimeoutError`,
`ConnectionTimeoutError`, `SocketTimeoutError`, `ClientClosedError`,
`ClientOfflineError`, `SocketClosedUnexpectedlyError`, `ReconnectStrategyError`.
The adapter does not discriminate between these — every error raised by a
redis-store method is caught and replaced with a fixed-message
`SessionStoreUnavailable`, never re-thrown and never carrying the original error
(as a `cause` or otherwise), because several of node-redis's own error messages
embed the connection DSN.

### Redis image tag

`redis:8-alpine` (as named in the task-4 brief and in
`tests/containers/redis-store.test.ts`) exists and was used directly — no fallback
to `redis:7-alpine` was needed.

## M5 — single-flight refresh: proving AC 2's test can fail

Task 5's Step 8 is mandatory: an AC 2 test that cannot fail proves nothing. The
guard was mutated in `src/core/single-flight.ts` by commenting out
`if (await store.tryAcquireLock(sid, lockToken, lockTtlMs))` and replacing it
with an unconditional `if (true)`, so every caller enters the refresh critical
section instead of contending for the per-session lock. Markers
`M5-MUTATION-START` / `M5-MUTATION-END` bounded the change during the run and
were deleted afterward — never reverted with `git checkout --`, since that
would also have discarded the (uncommitted, at the time) Task 5 implementation.

`pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight.test.ts`
against the mutated file: **5 of 13 tests failed.**

The two tests this step exists to break both failed on the refresh counter,
which is the AC 2 claim itself:

- `TWO CONCURRENT CALLERS ON AN EXPIRED TOKEN TRIGGER EXACTLY ONE REFRESH` —
  `expected 2 to be 1`. **Observed refresh count: 2.**
- `TWENTY concurrent callers still trigger exactly one refresh` —
  `expected 20 to be 1`. **Observed refresh count: 20** (one call per
  concurrent caller — the lock was providing zero exclusion).

Three further tests failed as a side effect of the same mutation, for a
different reason: `returns null when the record is deleted between the read
and the lock`, `returns the stale-but-live record when the lock cannot be won
in time`, and `returns null when the lock cannot be won and the token is
genuinely expired` all threw `Error: must not refresh`, because their fixtures
rely on losing the lock race and removing the guard makes every caller "win"
it. This is expected collateral damage of this specific mutation, not a
separate defect — it is further evidence the guard is load-bearing, not an
additional AC 2 finding.

After confirming the failure, the guard was restored by deleting the two
marker comments and the `if (true)` line (never `git checkout --`), and
`tests/core/single-flight.test.ts` was re-run: 13 of 13 tests passed again.
