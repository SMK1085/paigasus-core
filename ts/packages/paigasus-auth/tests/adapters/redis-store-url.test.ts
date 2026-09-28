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
