# SMA-715 Redis URL parse redaction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A malformed, empty or whitespace-only `PAIGASUS_SESSION_REDIS_URL` fails with one fixed,
secret-free error at both node-redis `createClient` sites, and the SMA-705 readiness event logs the
`code` of an `AuthError`.

**Architecture:** Each of the two `createClient` call sites (`@paigasus/auth` session store,
`@paigasus/console-core` descriptor cache) gets an empty-value check and a `try { createClient }
catch { throw <fixed error> }` with no catch binding. The two sites stay separate (no shared
helper). `readinessResponse` adds `code` to `readiness.runtime_failed` only for an `AuthError`.

**Tech Stack:** TypeScript, node-redis (`redis@6.2.1`, `@redis/client` 6.2.1), vitest, Moon, pnpm.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-715-redis-url-parse-redaction-design.md`

## Global Constraints

- The fixed message at BOTH sites is exactly `PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL`.
- Auth throws `AuthConfigError` (`src/core/errors.ts`, `code = 'auth_config_invalid'`, one-argument
  constructor). Console-core throws a plain `Error`. No new error class.
- `catch` has NO binding (`catch {`). The original error is never read. No `cause`.
- The try block holds ONLY the `createClient(...)` call. `client.on('error', …)`, the connect, the
  decorator, `watchConnectionLoss` and `state().redisClient = client` stay outside it.
- The empty check is `url.trim() === ''`, before the try. The URL that goes to `createClient` is NOT
  trimmed: `' redis://127.0.0.1:1 '` must keep working.
- Auth: do NOT write `let client: ReturnType<typeof createClient>` (does not typecheck, see
  `redis-store.ts:97-102`). Use a local function `buildClient` with an inferred return type.
- No change to `config.ts`, `runtime.ts`, the Helm chart or the apps. No zod refine (D3).
- `readiness.runtime_failed` logs `code` ONLY when the caught value is `instanceof AuthError`. Never
  from a foreign error (the node-redis `TypeError` carries `code: 'ERR_INVALID_URL'`).
- The new auth test file does NOT `vi.mock('redis')`. It uses the real parser.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Conventional commits with a workspace scope (`fix(ts): …`, `docs(ts): …`). End each commit
  message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never `git commit --amend`, `git reset`, `--no-verify` or `--no-gpg-sign`. Restore a mutation by
  deleting the mutation with Edit, never with `git checkout --`.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
  Worktree root: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-715-redis-url-parse-redaction`
  (called `$WT` below). Every command block below uses `$WT`. In a fresh shell, first run
  `export WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-715-redis-url-parse-redaction`,
  or put the absolute path in place of `$WT`.

## Review Focus

1. A valid URL with outer whitespace (`' redis://127.0.0.1:1 '`) must not be refused. A reviewer
   who "simplifies" the check to `url !== url.trim()` or trims before `createClient` breaks it.
   Pinned by the control rows in Task 1 and Task 2, and by mutation 6c.
2. A future node-redis option validation inside `createClient` would make EVERY URL look invalid.
   Pinned by the real-client valid-URL control in Task 1 (mutation 5).
3. A failed console-core build must leave no half state: `redisClientFromState()` stays `undefined`
   and the next call with a valid URL works (G3). Pinned in Task 2.
4. A foreign error that carries a string `code` must not reach the readiness log. Pinned by the
   unchanged T11/T11b rows in Task 3 (mutation 7b).
5. The empty value `''` must never build a client that connects to `localhost:6379`. Pinned by the
   `''` rows in Task 1 (with a time bound) and Task 2 (no client in state), and by mutation 6a/6b.

---

### Task 1: Auth — refuse a malformed or blank Redis URL without the password

**Files:**
- Modify: `ts/packages/paigasus-auth/src/adapters/redis-store.ts` (imports at line 26; `createRedisSessionStore` at lines 285-303)
- Create: `ts/packages/paigasus-auth/tests/adapters/redis-store-url.test.ts`
- Modify: `ts/packages/paigasus-auth/tests/adapters/redis-client-options.test.ts` (guard 1 describe)
- Modify: `ts/packages/paigasus-auth/README.md` ("### Redaction", line 151)

**Interfaces:**
- Consumes: `AuthConfigError` from `src/core/errors.ts` (`new AuthConfigError(message: string)`,
  `code === 'auth_config_invalid'`, `name === 'AuthConfigError'`).
- Produces: `createRedisSessionStore(opts)` rejects with `AuthConfigError('PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL')`
  for a malformed, empty or whitespace-only `opts.url`. So `createAuthRuntime` rejects with the
  same error. Task 3 relies on this.

- [ ] **Step 1: Write the failing test file**

Create `ts/packages/paigasus-auth/tests/adapters/redis-store-url.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-715. A malformed, empty or whitespace-only PAIGASUS_SESSION_REDIS_URL must not put the Redis
// password into an error (spec § 6.1).
//
// NO vi.mock('redis') IN THIS FILE. The defect is in the REAL node-redis error shape: its
// ERR_INVALID_URL TypeError holds the whole DSN in the enumerable own property `input`. The three
// sibling files redis-client-options, redis-store-connect and redis-store-parse DO mock 'redis'.
// Do not copy their setup. createClient throws before any I/O, so no Redis server is needed.
import { inspect } from 'node:util';
import { createClient } from 'redis';
import { describe, expect, it } from 'vitest';
import { createRedisSessionStore } from '../../src/adapters/redis-store.js';
import { AuthConfigError } from '../../src/core/errors.js';
import { createAuthRuntime } from '../../src/runtime.js';

const SENTINEL = 'SMA715SENTINELPW';
const MESSAGE = 'PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL';

/** Every throw path of node-redis 6.2.1's parseURL (spec § 1.1). Each error class MEASURED 2026-09-27. */
const MALFORMED: ReadonlyArray<readonly [label: string, url: string]> = [
  ['no scheme colon (ERR_INVALID_URL, the password is in `input`)', `redis//u:${SENTINEL}@h`],
  ['a port above 65535 (ERR_INVALID_URL, the password is in `input`)', `redis://u:${SENTINEL}@h:99999`],
  ['a scheme other than redis:, rediss: or unix: (Invalid protocol)', `http://u:${SENTINEL}@h`],
  ['a database path that is not a number (Invalid pathname)', `redis://u:${SENTINEL}@h/abc`],
  ['a bad percent escape in the password (URIError)', `redis://u:%zz${SENTINEL}@h`],
  ['a unix URL with a host (Invalid unix URL)', `unix://u:${SENTINEL}@/`],
  ['a unix URL with a db parameter that is not a number (Invalid db query parameter)', `unix://u:${SENTINEL}@/tmp/s?db=x`],
  ['a unix URL with a bad escape in the path (URIError)', `unix://u:${SENTINEL}@/tmp/%zz`],
];

/** D5. `''` skips all node-redis parsing; the other two throw ERR_INVALID_URL today (spec § 1.2). */
const BLANK: ReadonlyArray<readonly [label: string, url: string]> = [
  ['empty', ''],
  ['spaces only', '   '],
  ['tab and newline', '\t\n'],
];

/**
 * AC3, the structural channel: every own property name and value, enumerable or not
 * (Object.getOwnPropertyNames), walked recursively, so each link of the `cause` chain is walked.
 */
function walkText(value: unknown, seen: Set<object> = new Set()): string[] {
  if (typeof value !== 'object' || value === null) return [String(value)];
  if (seen.has(value)) return [];
  seen.add(value);
  const out: string[] = [];
  for (const name of Object.getOwnPropertyNames(value)) {
    out.push(name);
    out.push(...walkText((value as Record<string, unknown>)[name], seen));
  }
  return out;
}

/** AC3, the rendered channels: String(err), and util.inspect with hidden properties at full depth. */
function renderedText(err: unknown): string[] {
  return [String(err), inspect(err, { showHidden: true, depth: Infinity })];
}

/** AC3 in full: no string of either channel holds the sentinel, and there is no `cause`. */
function expectNoSentinel(err: unknown): void {
  expect(err).toBeInstanceOf(Error);
  expect((err as Error).cause).toBeUndefined();
  for (const text of [...walkText(err), ...renderedText(err)]) expect(text).not.toContain(SENTINEL);
}

function expectFixedAuthError(err: unknown): void {
  expect(err).toBeInstanceOf(AuthConfigError);
  expect((err as AuthConfigError).message).toBe(MESSAGE);
  expect((err as AuthConfigError).code).toBe('auth_config_invalid');
  expectNoSentinel(err);
}

/**
 * Builds a REAL store with the production options. Returns what it threw, or `undefined` after it
 * closed the store it got. Port 1 refuses a connect, so a valid URL resolves after one command
 * timeout on the 'waiting' path (redis-store.ts connectWithin), and close() destroys the client.
 */
async function storeError(url: string, commandTimeoutMs = 50): Promise<unknown> {
  try {
    const store = await createRedisSessionStore({ url, commandTimeoutMs, keyPrefix: '' });
    await store.close();
    return undefined;
  } catch (err) {
    return err;
  }
}

describe('the AC3 helper finds the sentinel in each channel (spec § 6.1 controls)', () => {
  it('(a) a non-enumerable own property: the walk finds it', () => {
    const err = new Error('x');
    Object.defineProperty(err, 'hidden', { value: SENTINEL, enumerable: false });
    expect(walkText(err).join('\n')).toContain(SENTINEL);
  });

  it('(a) a non-enumerable own property: util.inspect with showHidden finds it', () => {
    const err = new Error('x');
    Object.defineProperty(err, 'hidden', { value: SENTINEL, enumerable: false });
    expect(renderedText(err).join('\n')).toContain(SENTINEL);
  });

  it('(b) cause: the walk finds it', () => {
    expect(walkText(new Error('x', { cause: new Error(SENTINEL) })).join('\n')).toContain(SENTINEL);
  });

  it('(c) a nested cause (cause.cause): the walk finds it', () => {
    const err = new Error('x', { cause: new Error('y', { cause: new Error(SENTINEL) }) });
    expect(walkText(err).join('\n')).toContain(SENTINEL);
  });

  it('(d) an enumerable own property: the walk finds it', () => {
    expect(walkText(Object.assign(new Error('x'), { input: SENTINEL })).join('\n')).toContain(SENTINEL);
  });

  it('the RAW node-redis error for a missing scheme colon holds the sentinel (the defect, channel d)', () => {
    let raw: unknown;
    try {
      createClient({ url: `redis//u:${SENTINEL}@h` });
    } catch (err) {
      raw = err;
    }
    expect(raw).toBeInstanceOf(TypeError);
    expect(walkText(raw).join('\n')).toContain(SENTINEL);
  });
});

describe('createRedisSessionStore with a malformed URL (SMA-715 AC1, AC3)', () => {
  it.each(MALFORMED)('%s -> AuthConfigError with the fixed message and no part of the URL', async (_label, url) => {
    expectFixedAuthError(await storeError(url));
  });

  it('a valid URL with the production options builds a real client and does not throw (control)', async () => {
    expect(await storeError('redis://127.0.0.1:1')).toBeUndefined();
  });
});

describe('createRedisSessionStore with an empty or whitespace-only URL (SMA-715 AC8, D5)', () => {
  it.each(BLANK)('%s -> AuthConfigError with the fixed message and no part of the URL', async (_label, url) => {
    expectFixedAuthError(await storeError(url));
  });

  it('the empty value is refused before any client is built: no connect to localhost:6379', async () => {
    // With a 2000 ms command timeout, a store that built a client would resolve only after a
    // connect (a local Redis) or after 2000 ms (the 'waiting' path). Either way it resolves.
    const started = performance.now();
    const err = await storeError('', 2000);
    expect(performance.now() - started).toBeLessThan(500);
    expectFixedAuthError(err);
  });

  it('a valid URL with outer whitespace is NOT refused (control, spec § 1.2 last row)', async () => {
    expect(await storeError(' redis://127.0.0.1:1 ')).toBeUndefined();
  });
});

// Copied from tests/runtime-redis-wiring.test.ts:27-45, WITHOUT that file's vi.mock('redis').
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
  PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 50,
  PAIGASUS_SESSION_TTL_SECONDS: 28800,
  PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS: 86400,
  PAIGASUS_SESSION_REFRESH_SKEW_SECONDS: 30,
  PAIGASUS_SESSION_LOCK_TTL_MS: 10000,
  PAIGASUS_SESSION_LOCK_WAIT_MS: 3000,
};

/** What createAuthRuntime threw, or `undefined` after it closed the store of the runtime it got. */
async function runtimeError(url: string): Promise<unknown> {
  try {
    const runtime = await createAuthRuntime({ ...CONFIG, PAIGASUS_SESSION_REDIS_URL: url });
    await runtime.store.close();
    return undefined;
  } catch (err) {
    return err;
  }
}

describe('createAuthRuntime, the path that Next logs (SMA-715 AC4, AC8)', () => {
  it('a malformed URL -> the fixed AuthConfigError with no part of the URL', async () => {
    expectFixedAuthError(await runtimeError(`redis//u:${SENTINEL}@h`));
  });

  it('an empty URL -> the same AuthConfigError', async () => {
    expectFixedAuthError(await runtimeError(''));
  });
});
```

- [ ] **Step 2: Add the `socket.tls` pin**

In `ts/packages/paigasus-auth/tests/adapters/redis-client-options.test.ts`, inside
`describe('guard 1: disableOfflineQueue', …)`, after the test
`'passes the command timeout and the reconnect strategy alongside it'`, add:

```ts
  // SMA-715 § 3.4. node-redis puts the URL, password included, into the MESSAGE of its
  // tls-mismatch TypeError, which fires only when the caller passes socket.tls. The catch in
  // createRedisSessionStore drops that error too; this pin states the assumption. This file mocks
  // createClient, so these are the options BEFORE node-redis parses them.
  it('passes no socket.tls (SMA-715)', () => {
    const socket = captured.options?.['socket'] as Record<string, unknown> | undefined;
    expect(socket).toBeDefined();
    expect('tls' in (socket as Record<string, unknown>)).toBe(false);
  });
```

- [ ] **Step 3: Run the tests to verify they fail**

Run:
`export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd $WT/ts/packages/paigasus-auth && pnpm exec vitest run tests/adapters/redis-store-url.test.ts tests/adapters/redis-client-options.test.ts`

Expected: FAIL. The eight MALFORMED rows fail (`expected TypeError … to be an instance of
AuthConfigError`, or `URIError`). The `'   '` and `'\t\n'` rows fail (TypeError). The `''` rows fail
(the store resolves, so `storeError` returns `undefined`; the time-bound row also fails its bound
unless a local Redis runs). The AC4 malformed row fails. The six helper controls, the two valid-URL
controls and the tls pin PASS already.

- [ ] **Step 4: Implement the check and the catch**

In `ts/packages/paigasus-auth/src/adapters/redis-store.ts`, change the import on line 26 to:

```ts
import { AuthConfigError, SessionStoreUnavailable } from '../core/errors';
```

Directly above `export async function createRedisSessionStore(`, add:

```ts
/** SMA-715. The one message for a malformed, empty or whitespace-only URL. It holds no part of the URL. */
const INVALID_REDIS_URL = 'PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL';

/**
 * Builds the node-redis client, or throws AuthConfigError(INVALID_REDIS_URL). A local function, so
 * the client keeps the type the compiler infers (see the RedisClient comment above: naming
 * `ReturnType<typeof createClient>` does not typecheck with `commandOptions`).
 */
function buildClient(opts: CreateRedisSessionStoreOptions) {
  // SMA-715 D5: node-redis skips ALL parsing for an empty URL and connects to localhost:6379. The
  // URL that goes to createClient is not trimmed: `new URL()` strips outer whitespace itself, so
  // ' redis://h:1 ' keeps working.
  if (opts.url.trim() === '') throw new AuthConfigError(INVALID_REDIS_URL);
  try {
    return createClient({
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
  } catch {
    // SMA-715: node-redis parses the URL here with `new URL()`. Its ERR_INVALID_URL TypeError holds
    // the whole DSN, password included, in `input`. Rethrow a fixed message with no cause. The catch
    // has no binding on purpose: the original error is never read, so no later edit can copy it.
    throw new AuthConfigError(INVALID_REDIS_URL);
  }
}
```

In `createRedisSessionStore`, replace the whole `const client = createClient({ … });` statement
(the `url` line through the closing `});`) with:

```ts
  const client = buildClient(opts);
```

Leave `client.on('error', …)`, `connectWithin` and the `return withOperationDeadline(…)` unchanged.

- [ ] **Step 5: Run the tests to verify they pass**

Run:
`export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd $WT/ts/packages/paigasus-auth && pnpm exec vitest run tests/adapters/ tests/runtime.test.ts tests/runtime-redis-wiring.test.ts`

Expected: PASS, all files.

- [ ] **Step 6: Update the auth README "Redaction" section**

In `ts/packages/paigasus-auth/README.md`, under `### Redaction`, after the first paragraph (the one
that ends "only a fixed name and message are extracted for logging."), add:

```markdown
A malformed, empty or whitespace-only `PAIGASUS_SESSION_REDIS_URL` fails with an `AuthConfigError`
with the fixed message `PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL` (SMA-715). The
node-redis parse error is dropped: its `input` property holds the whole URL, password included. The
message gives no detail, so check the value for these usual causes:

- an empty or whitespace-only value, for example an empty `session-redis-url` Secret key (without
  this check, node-redis would connect to `localhost:6379`);
- a password that holds `@`, `:`, `/`, `?`, `#` or `%` and is not percent-encoded;
- a port above 65535;
- a scheme other than `redis:`, `rediss:` or `unix:`;
- a database path or a `db` parameter that is not a number.
```

- [ ] **Step 7: Lint, typecheck and format**

Run:
`export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd $WT && moon run paigasus-auth-ts:lint paigasus-auth-ts:typecheck && pnpm -C ts exec prettier --check packages/paigasus-auth`

Expected: all pass. If Prettier reports a file, run `pnpm -C ts exec prettier --write <file>` on
that file only, then re-run the check.

- [ ] **Step 8: Commit**

```bash
cd $WT && [ "$(git branch --show-current)" = feature/sma-715-redis-url-parse-redaction ] && \
git add ts/packages/paigasus-auth/src/adapters/redis-store.ts \
  ts/packages/paigasus-auth/tests/adapters/redis-store-url.test.ts \
  ts/packages/paigasus-auth/tests/adapters/redis-client-options.test.ts \
  ts/packages/paigasus-auth/README.md && \
git commit -m "fix(ts): refuse a malformed Redis URL in @paigasus/auth without the password (SMA-715)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 9: Prove the tests bite (spec § 6.4 steps 1, 2, 4, 5, 6 for auth)**

Do each mutation with Edit, run the file, record the result, then delete the mutation with Edit.
After each restore, `git -C $WT diff --exit-code` must print nothing. Test command:
`cd $WT/ts/packages/paigasus-auth && pnpm exec vitest run tests/adapters/redis-store-url.test.ts`

| # | Mutation (must compile) | Must red |
|---|---|---|
| 1 | In `buildClient`, replace the `try { return createClient(…) } catch { … }` with only `return createClient(…)` (keep the options). | at least the two ERR_INVALID_URL MALFORMED rows and the AC4 malformed row |
| 2 | Change the catch to `catch (e) { throw Object.assign(new AuthConfigError(INVALID_REDIS_URL), { cause: e }); }` | the MALFORMED rows (the `cause` assertion and the walk) |
| 4a | In `walkText`, use `Object.keys` in place of `Object.getOwnPropertyNames` | control "(a) … the walk finds it" |
| 4b | In `renderedText`, drop `showHidden: true` | control "(a) … util.inspect with showHidden finds it". The raw node-redis row stays GREEN (`input` is enumerable, spec § 1.1) |
| 4c | In `walkText`, skip the name `cause` (`if (name === 'cause') continue;`) | controls (b) and (c) |
| 4d | Add a `depth` parameter to `walkText` and stop recursing below depth 1 for `cause` | control (c) |
| 4e | In `walkText`, push the names only (no recursive call on the values) | control (d) and the raw node-redis row |
| 5 | In `buildClient`, replace `return createClient({…})` with `const c = createClient({…}); if (c) throw new Error('x'); return c;` | both valid-URL controls |
| 6a | Delete the `if (opts.url.trim() === '') …` line | the `''` rows (store, time bound, AC4 empty). The `'   '` and `'\t\n'` rows stay GREEN: the parse catch maps their ERR_INVALID_URL (expected, spec § 6.4 step 6) |
| 6b | Mutations 6a and 1 together | the `''`, `'   '` and `'\t\n'` rows |
| 6c | Change the check to `if (opts.url !== opts.url.trim() \|\| opts.url === '')` | the `' redis://127.0.0.1:1 '` control |

Record each row (mutation, the tests that red, the tests that stayed green) in the scratchpad file
`sma-715-mutations.md` (in the session scratchpad directory, NOT in the repo). The PR stage copies it
into the PR body. If a row does not red as the table says, stop and report it: do not change the
test to make it red without a reason that you record.

---

### Task 2: Console-core — refuse a malformed or blank Redis URL without the password

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/discovery.ts` (`redisDescriptorCache`, lines 312-329)
- Modify: `ts/packages/paigasus-console-core/tests/unit/discovery-redis-client.test.ts`
- Modify: `ts/packages/paigasus-console-core/README.md` (new `## Redaction` section)

**Interfaces:**
- Consumes: nothing from Task 1 (the packages share no code for this, spec R2).
- Produces: `descriptorCacheFor(config, log)` with `PAIGASUS_SESSION_STORE: 'redis'` throws a plain
  `Error('PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL')` for a malformed, empty or
  whitespace-only URL, and leaves `state()` untouched.

- [ ] **Step 1: Write the failing tests**

In `ts/packages/paigasus-console-core/tests/unit/discovery-redis-client.test.ts`:

Add `import { inspect } from 'node:util';` as the first import line (after the header comment).

Append at the end of the file:

```ts
// SMA-715 (spec § 6.2). A local copy of the auth helper: the packages share no test support.
const SENTINEL = 'SMA715SENTINELPW';
const MESSAGE = 'PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL';

/** Every throw path of node-redis 6.2.1's parseURL (spec § 1.1). Each error class MEASURED 2026-09-27. */
const MALFORMED: ReadonlyArray<readonly [label: string, url: string]> = [
  ['no scheme colon (ERR_INVALID_URL, the password is in `input`)', `redis//u:${SENTINEL}@h`],
  ['a port above 65535 (ERR_INVALID_URL, the password is in `input`)', `redis://u:${SENTINEL}@h:99999`],
  ['a scheme other than redis:, rediss: or unix: (Invalid protocol)', `http://u:${SENTINEL}@h`],
  ['a database path that is not a number (Invalid pathname)', `redis://u:${SENTINEL}@h/abc`],
  ['a bad percent escape in the password (URIError)', `redis://u:%zz${SENTINEL}@h`],
  ['a unix URL with a host (Invalid unix URL)', `unix://u:${SENTINEL}@/`],
  ['a unix URL with a db parameter that is not a number (Invalid db query parameter)', `unix://u:${SENTINEL}@/tmp/s?db=x`],
  ['a unix URL with a bad escape in the path (URIError)', `unix://u:${SENTINEL}@/tmp/%zz`],
];

const BLANK: ReadonlyArray<readonly [label: string, url: string]> = [
  ['empty', ''],
  ['spaces only', '   '],
  ['tab and newline', '\t\n'],
];

/** AC3: every own property name and value, enumerable or not, walked recursively (so each `cause`). */
function walkText(value: unknown, seen: Set<object> = new Set()): string[] {
  if (typeof value !== 'object' || value === null) return [String(value)];
  if (seen.has(value)) return [];
  seen.add(value);
  const out: string[] = [];
  for (const name of Object.getOwnPropertyNames(value)) {
    out.push(name);
    out.push(...walkText((value as Record<string, unknown>)[name], seen));
  }
  return out;
}

function expectFixedError(err: unknown): void {
  expect(err).toBeInstanceOf(Error);
  // A PLAIN Error, the same class as the missing-URL throw of descriptorCacheFor.
  expect(Object.getPrototypeOf(err)).toBe(Error.prototype);
  expect((err as Error).message).toBe(MESSAGE);
  expect((err as Error).cause).toBeUndefined();
  const texts = [...walkText(err), String(err), inspect(err, { showHidden: true, depth: Infinity })];
  for (const text of texts) expect(text).not.toContain(SENTINEL);
}

describe('descriptorCacheFor with a malformed or blank PAIGASUS_SESSION_REDIS_URL (SMA-715)', () => {
  const quiet = createJsonLogger(() => undefined);
  const configFor = (url: string): ConsoleCoreConfig =>
    ({ PAIGASUS_SESSION_STORE: 'redis', PAIGASUS_SESSION_REDIS_URL: url, PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 500 }) as ConsoleCoreConfig;
  /** What descriptorCacheFor threw, or `undefined`. afterEach's resetDiscoveryForTest destroys any client. */
  const thrownBy = (url: string): unknown => {
    try {
      descriptorCacheFor(configFor(url), quiet);
      return undefined;
    } catch (err) {
      return err;
    }
  };

  it.each(MALFORMED)('%s -> the fixed Error, no client in state, and a valid URL then works (AC2, G3)', (_label, url) => {
    expectFixedError(thrownBy(url));
    expect(redisClientFromState()).toBeUndefined();
    // Port 1 refuses the connection. The client is made at once; the connect runs in the background.
    expect(thrownBy('redis://127.0.0.1:1')).toBeUndefined();
    expect(redisClientFromState()).toBeDefined();
  });

  it.each(BLANK)('%s -> the fixed Error and no client in state (AC8, D5)', (_label, url) => {
    expectFixedError(thrownBy(url));
    expect(redisClientFromState()).toBeUndefined();
  });

  it('a valid URL with outer whitespace is NOT refused and puts a client in state (control)', () => {
    expect(thrownBy(' redis://127.0.0.1:1 ')).toBeUndefined();
    expect(redisClientFromState()).toBeDefined();
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
`export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd $WT/ts/packages/paigasus-console-core && pnpm exec vitest run tests/unit/discovery-redis-client.test.ts`

Expected: FAIL. All eight MALFORMED rows fail (a `TypeError`/`URIError`, not a plain `Error` with the
fixed message). `'   '` and `'\t\n'` fail (TypeError). `''` fails (no throw, and a client is in
state). The whitespace control and the 14 existing tests PASS.

- [ ] **Step 3: Implement the check and the catch**

In `ts/packages/paigasus-console-core/src/discovery.ts`, directly above
`function redisDescriptorCache(`, add:

```ts
/** SMA-715. The same fixed message as @paigasus/auth's session store. It holds no part of the URL. */
const INVALID_REDIS_URL = 'PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL';
```

In `redisDescriptorCache`, replace the `const client: RedisClientType = createClient({ … });`
statement with:

```ts
  // SMA-715 D5: node-redis skips ALL parsing for an empty URL and connects to localhost:6379. The
  // URL that goes to createClient is not trimmed: `new URL()` strips outer whitespace itself.
  if (url.trim() === '') throw new Error(INVALID_REDIS_URL);
  let client: RedisClientType;
  try {
    client = createClient({
      url,
      disableOfflineQueue: true,
      commandOptions: { timeout: timeoutMs },
      // D2: a PING every timeout keeps a healthy idle socket under the idle timer (file header).
      pingInterval: timeoutMs,
      // D1: the idle timer stays; it is the only bound on a hung command. D3: the strategy reopens
      // the socket after the timer fires on a real hang.
      socket: { socketTimeout: timeoutMs * 2, connectTimeout: timeoutMs, reconnectStrategy: descriptorCacheReconnectStrategy },
    });
  } catch {
    // SMA-715: node-redis parses the URL here with `new URL()`. Its ERR_INVALID_URL TypeError holds
    // the whole DSN, password included, in `input`. Rethrow a fixed message with no cause. No catch
    // binding: the original error is never read. Nothing below ran, so state() is untouched and the
    // next call tries again (G3).
    throw new Error(INVALID_REDIS_URL);
  }
```

Leave `watchConnectionLoss(client, log);`, `state().redisClient = client;` and the rest of the
function after the try, unchanged.

- [ ] **Step 4: Run the tests to verify they pass**

Run:
`export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd $WT/ts/packages/paigasus-console-core && pnpm exec vitest run tests/unit/`

Expected: PASS.

- [ ] **Step 5: Add the console-core README section**

In `ts/packages/paigasus-console-core/README.md`, before `## Tests`, add:

```markdown
## Redaction

A malformed, empty or whitespace-only `PAIGASUS_SESSION_REDIS_URL` makes the console descriptor
cache throw a plain `Error` with the fixed message
`PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL` (SMA-715). The node-redis parse error is
dropped: its `input` property holds the whole URL, password included. The message gives no detail,
so check the value for these usual causes:

- an empty or whitespace-only value, for example an empty `session-redis-url` Secret key (without
  this check, node-redis would connect to `localhost:6379`);
- a password that holds `@`, `:`, `/`, `?`, `#` or `%` and is not percent-encoded;
- a port above 65535;
- a scheme other than `redis:`, `rediss:` or `unix:`;
- a database path or a `db` parameter that is not a number.

The next request tries to build the cache again.
```

- [ ] **Step 6: Lint, typecheck and format**

Run:
`export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd $WT && moon run paigasus-console-core-ts:lint paigasus-console-core-ts:typecheck && pnpm -C ts exec prettier --check packages/paigasus-console-core`

Expected: all pass. Fix a Prettier report with `prettier --write` on that file only.

- [ ] **Step 7: Commit**

```bash
cd $WT && [ "$(git branch --show-current)" = feature/sma-715-redis-url-parse-redaction ] && \
git add ts/packages/paigasus-console-core/src/discovery.ts \
  ts/packages/paigasus-console-core/tests/unit/discovery-redis-client.test.ts \
  ts/packages/paigasus-console-core/README.md && \
git commit -m "fix(ts): refuse a malformed Redis URL in the console descriptor cache without the password (SMA-715)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 8: Prove the tests bite (spec § 6.4 steps 3 and 6 for console-core)**

Same rules as Task 1 Step 9 (Edit, run, record, delete the mutation, `git diff --exit-code` empty).
Test command: `cd $WT/ts/packages/paigasus-console-core && pnpm exec vitest run tests/unit/discovery-redis-client.test.ts`

| # | Mutation (must compile) | Must red |
|---|---|---|
| 3 | Replace the try/catch with only `client = createClient({…});` (keep `let client: RedisClientType;`) | at least the two ERR_INVALID_URL MALFORMED rows |
| 3b | Add `state().redisClient = createClient({ url: 'redis://127.0.0.1:1' }) as RedisClientType;` as the FIRST line inside the try (a half state before the parse throws; the client never connects) | the MALFORMED rows (`redisClientFromState()` is not `undefined` after the throw) |
| 6a | Delete the `if (url.trim() === '') …` line | the `''` row (no throw, a client in state). `'   '` and `'\t\n'` stay GREEN (expected) |
| 6b | Mutations 6a and 3 together | the `''`, `'   '` and `'\t\n'` rows |
| 6c | Change the check to `if (url !== url.trim() \|\| url === '')` | the whitespace control |

Append each result to the scratchpad file `sma-715-mutations.md`.

---

### Task 3: Readiness event logs an `AuthError` code (D4), and the SMA-705 texts

**Files:**
- Modify: `ts/packages/paigasus-auth/src/http/readiness.ts` (header lines 21-25; the catch at lines 58-66)
- Modify: `ts/packages/paigasus-auth/src/ports/logger.ts` (comment lines 18-20)
- Modify: `ts/packages/paigasus-auth/tests/http/readiness.test.ts` (line 259; a new row after it)
- Modify: `ts/packages/paigasus-auth/README.md` ("### The readiness route (SMA-705)" bullets and "Known limits")
- Modify: `docs/superpowers/specs/2026-09-27-sma-705-console-oidc-readiness-design.md` (§ 10, lines 431-432)

**Interfaces:**
- Consumes: Task 1's behaviour: `createAuthRuntime` with a malformed Redis URL rejects with
  `AuthConfigError` (`code: 'auth_config_invalid'`). `AuthError` from `src/core/errors.ts`
  (`abstract readonly code: string`).
- Produces: the event `readiness.runtime_failed` with fields `{ error: string }` or
  `{ error: string, code: string }` (`code` only for an `AuthError`).

- [ ] **Step 1: Write the failing tests**

In `ts/packages/paigasus-auth/tests/http/readiness.test.ts`:

(a) In the test `'a real cross-field refusal from createAuthRuntime -> 503 unready, error AuthConfigError'`,
change the expectation to:

```ts
    expect(events).toEqual([['readiness.runtime_failed', { error: 'AuthConfigError', code: 'auth_config_invalid' }]]);
```

(b) Directly after that test (still inside `describe('readinessResponse — the runtime build fails (SMA-705 D10)', …)`), add:

```ts
  // SMA-715 D4. The real error of a malformed Redis URL: the name and the code of AuthConfigError,
  // and no part of the URL anywhere. T11 and T11b above keep `{ error: 'TypeError' }` with NO `code`,
  // although their fixture carries `code: 'ERR_INVALID_URL'`: a foreign code is never logged.
  it('SMA-715: a malformed Redis URL from createAuthRuntime -> error AuthConfigError, code auth_config_invalid, no password', async () => {
    const events: Events = [];
    const getter = (): Promise<AuthRuntime> =>
      createAuthRuntime({ ...BASE_CONFIG, PAIGASUS_SESSION_STORE: 'redis' as const, PAIGASUS_SESSION_REDIS_URL: 'redis//u:sentinel-715@h' });
    await expectProbe(await readinessResponse(getter, recordingLogger(events)), 503, 'unready', ['sentinel-715']);
    expect(events).toEqual([['readiness.runtime_failed', { error: 'AuthConfigError', code: 'auth_config_invalid' }]]);
    expectEventsClean(events, ['sentinel-715']);
  });
```

Do NOT change T11, T11b or T11c.

- [ ] **Step 2: Run the tests to verify they fail**

Run:
`export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd $WT/ts/packages/paigasus-auth && pnpm exec vitest run tests/http/readiness.test.ts`

Expected: FAIL on the cross-field test and on the new SMA-715 row (the event has no `code`). T11,
T11b and T11c PASS.

- [ ] **Step 3: Implement the `code` field**

In `ts/packages/paigasus-auth/src/http/readiness.ts`:

Add the import (after the `AuthLogger` import):

```ts
import { AuthError } from '../core/errors';
```

Below `function errorName(…) { … }`, add:

```ts
/**
 * The event fields of a runtime-build failure (SMA-715 D4): the error's `name`, plus its `code` ONLY
 * when it is one of this package's own errors (an AuthError, a closed literal such as
 * 'auth_config_invalid'). A foreign error's `code` is text this package does not control; the
 * node-redis URL TypeError carries `code: 'ERR_INVALID_URL'`, and it is never logged.
 */
function runtimeFailureFields(err: unknown): Record<string, string> {
  const fields: Record<string, string> = { error: errorName(err) };
  if (err instanceof AuthError) fields['code'] = err.code;
  return fields;
}
```

In the catch of `readinessResponse`, replace
`logger.event('readiness.runtime_failed', { error: errorName(err) });` with:

```ts
      logger.event('readiness.runtime_failed', runtimeFailureFields(err));
```

Replace the file-header lines of `THE LOG (D8, D10)` that read
"A runtime build that fails logs `readiness.runtime_failed { error }`, where `error` is the error's `name`
only. It never logs the error object, its message or its `input`: node-redis parses the Redis URL
with `new URL()`, and that TypeError holds the URL, password included (measured, spec D10)." with:

```ts
// A runtime build that fails logs `readiness.runtime_failed { error, code }`. `error` is the error's
// `name` only. `code` is present only for an AuthError of this package (SMA-715 D4), for example
// `auth_config_invalid`; a foreign error's `code` is never logged. It never logs the error object,
// its message or its `input`: node-redis parses the Redis URL with `new URL()`, and that TypeError
// holds the URL, password included (measured, SMA-705 spec D10). Since SMA-715 the Redis store
// rethrows that TypeError as a fixed AuthConfigError before it reaches this route.
```

- [ ] **Step 4: Update the logger port comment**

In `ts/packages/paigasus-auth/src/ports/logger.ts`, replace the three comment lines that start
"`readiness.runtime_failed` carries `error`, the caught error's `name` only (SMA-705 D10)." with:

```ts
// `readiness.runtime_failed` carries `error`, the caught error's `name` only (SMA-705 D10), and
// `code` only when the error is an AuthError of this package (SMA-715 D4). Never its message, its
// `input` or a foreign error's `code`: node-redis parses the Redis URL with `new URL()`, and that
// TypeError holds the URL, password included, and carries its own `code`.
```

- [ ] **Step 5: Run the tests to verify they pass**

Run:
`export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd $WT/ts/packages/paigasus-auth && pnpm exec vitest run tests/http/ tests/logger.test.ts`

Expected: PASS.

- [ ] **Step 6: Update the auth README readiness text**

In `ts/packages/paigasus-auth/README.md`, section `### The readiness route (SMA-705)`:

Replace the bullet that starts ``- **`readiness.runtime_failed { error }`.**`` (through
"Each probe tries the build again.") with:

```markdown
- **`readiness.runtime_failed { error, code }`.** The runtime build failed: a configuration parse
  error, a cross-field rule, or the Redis client build. `error` is the error's `name` only, for
  example `TypeError`. `code` is present only when the error is one of this package's own errors
  (an `AuthError`). A cross-field rule and a malformed, empty or whitespace-only
  `PAIGASUS_SESSION_REDIS_URL` both log `error: 'AuthConfigError', code: 'auth_config_invalid'`
  (SMA-715; see "Redaction"). The event never holds the error message, its other properties, or
  the `code` of an error from another library. Each probe tries the build again.
```

In "**Known limits.**", replace the bullet
"- A configuration defect (a wrong issuer, a wrong CA, a malformed Redis URL) keeps the pod not ready for ever. The log shows the `reason` or the error name."
with:

```markdown
- A configuration defect (a wrong issuer, a wrong CA, a malformed Redis URL) keeps the pod not
  ready for ever. The log shows the `reason`, or the error name and, for a configuration refusal,
  the `code` `auth_config_invalid`.
```

- [ ] **Step 7: Add the SMA-705 spec § 10 sentence**

In `docs/superpowers/specs/2026-09-27-sma-705-console-oidc-readiness-design.md`, keep the bullet
"- The Redis DSN leak on the user request path (D10). SMA-715 tracks it: `redis-store.ts` must catch the `createClient` URL error and rethrow a redacted error."
and append one sentence to it, so that it ends:

```markdown
  must catch the `createClient` URL error and rethrow a redacted error. Closed by SMA-715
  (`docs/superpowers/specs/2026-09-27-sma-715-redis-url-parse-redaction-design.md`): both
  `createClient` sites rethrow a fixed error, and `readiness.runtime_failed` now also logs the
  `code` of an `AuthError`.
```

Do not delete the old text.

- [ ] **Step 8: Lint, typecheck and format**

Run:
`export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd $WT && moon run paigasus-auth-ts:lint paigasus-auth-ts:typecheck paigasus-auth-ts:test && pnpm -C ts exec prettier --check packages/paigasus-auth`

Expected: all pass.

- [ ] **Step 9: Commit**

```bash
cd $WT && [ "$(git branch --show-current)" = feature/sma-715-redis-url-parse-redaction ] && \
git add ts/packages/paigasus-auth/src/http/readiness.ts ts/packages/paigasus-auth/src/ports/logger.ts \
  ts/packages/paigasus-auth/tests/http/readiness.test.ts ts/packages/paigasus-auth/README.md \
  docs/superpowers/specs/2026-09-27-sma-705-console-oidc-readiness-design.md && \
git commit -m "feat(ts): log the AuthError code in readiness.runtime_failed (SMA-715)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 10: Prove the tests bite (spec § 6.4 step 7)**

Same rules as Task 1 Step 9. Test command:
`cd $WT/ts/packages/paigasus-auth && pnpm exec vitest run tests/http/readiness.test.ts`

| # | Mutation (must compile) | Must red |
|---|---|---|
| 7a | In `runtimeFailureFields`, delete the `if (err instanceof AuthError) …` line | the cross-field test and the SMA-715 row |
| 7b | Replace that line with `const code: unknown = (err as { code?: unknown } \| null)?.code; if (typeof code === 'string') fields['code'] = code;` | T11 (both rows) and T11b |

Append each result to the scratchpad file `sma-715-mutations.md`.

---

### Task 4: Whole-branch verification and the complete mutation battery

**Files:** none changed (verification only). If a step finds a defect, fix it in the file of the
owning task, commit with a new `fix(ts): …` commit, and re-run this task from Step 1.

**Interfaces:**
- Consumes: Tasks 1-3 committed.
- Produces: `sma-715-mutations.md` in the session scratchpad with every row of spec § 6.4, for the
  PR body.

- [ ] **Step 1: Re-run the whole mutation battery**

After all three tasks, re-run EVERY mutation of Task 1 Step 9, Task 2 Step 8 and Task 3 Step 10 on
the final code, in one pass, even if you ran them before (a later edit can make an earlier test
inert). Update `sma-715-mutations.md` with the final results. After the last restore,
`git -C $WT diff --exit-code` must print nothing and `git -C $WT status --short` must show no change
to a tracked file.

- [ ] **Step 2: Run both packages' tests, lint, typecheck and the Prettier gate**

Run:
`export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd $WT && moon run paigasus-auth-ts:test paigasus-auth-ts:lint paigasus-auth-ts:typecheck paigasus-console-core-ts:test paigasus-console-core-ts:lint paigasus-console-core-ts:typecheck ts:fmt`

Expected: all pass.

- [ ] **Step 3: Run the full gate graph**

Docker must run first: a `src/` edit in console-core selects `paigasus-console-core-ts:test-e2e`,
which needs Docker and has no skip hatch. Check with `docker info >/dev/null && echo up`.

Run the command between the `ci-targets` markers of the root `CLAUDE.md`, from `$WT`, with the PATH
prefix. Read the root `CLAUDE.md` "This development Mac only" section first: no single local bash
runs every gate. If a gate fails with a bash-version signature (`mapfile: command not found`,
`declare: -A: invalid option`, or rc 2 "pipe holds only 512 bytes"), re-run that gate directly with
the right bash (`<bash-binary> ci/<gate>/run.sh`) and use that result. Follow the moon-diagnosis
procedure of the root `CLAUDE.md` (Step 0 first) for any other failure.

Expected: every gate green, or each red explained as a host artifact with the direct re-run green.

- [ ] **Step 4: Report**

Report the final commit list (`git -C $WT log --oneline origin/main..HEAD`), the gate results, and
the path of `sma-715-mutations.md`. Do not push; the open-pr stage pushes.
