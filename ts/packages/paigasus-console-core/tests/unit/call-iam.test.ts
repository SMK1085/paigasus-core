// SPDX-License-Identifier: Apache-2.0
//
// callIam (spec § 6.1, § 9.2): a ConnectError becomes data, EVERYTHING else is rethrown unchanged.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { notFound, redirect } from 'next/navigation';
import { callIam } from '../../src/errors';
import { logger } from '../../src/logger';
import { setRequestHeaders } from '../support/next-headers';

// callIam writes one real JSON line per failed call. Silenced here so the suite's output stays
// clean; the last case re-reads the SAME spy, so the log assertion is unchanged.
function silenceLogger() {
  return vi.spyOn(logger, 'appEvent').mockImplementation(() => undefined);
}

let appEvent: ReturnType<typeof silenceLogger>;

beforeEach(() => {
  appEvent = silenceLogger();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('callIam', () => {
  it('returns the value of a successful call', async () => {
    expect(await callIam(() => Promise.resolve(42))).toEqual({ ok: true, value: 42 });
  });

  it.each([
    [Code.PermissionDenied, 'forbidden'],
    [Code.NotFound, 'not-found'],
    [Code.Unauthenticated, 'relogin'],
    [Code.InvalidArgument, 'invalid-input'],
    [Code.AlreadyExists, 'conflict'],
    [Code.Unavailable, 'degraded'],
    [Code.ResourceExhausted, 'rate-limited'],
    [Code.Internal, 'generic'],
  ] as const)('maps a ConnectError with code %s to the %s presentation', async (code, presentation) => {
    const result = await callIam(() => Promise.reject(new ConnectError('boom', code)));
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error.presentation).toBe(presentation);
  });

  it('rethrows a plain Error unchanged — the same object', async () => {
    const bug = new TypeError('a real bug');
    await expect(callIam(() => Promise.reject(bug))).rejects.toBe(bug);
  });

  it('rethrows Next’s control-flow errors, so redirect() and notFound() still navigate', async () => {
    await expect(callIam(() => Promise.resolve().then(() => redirect('/somewhere')))).rejects.toMatchObject({ digest: expect.stringContaining('NEXT_REDIRECT') as string });
    await expect(callIam(() => Promise.resolve().then(() => notFound()))).rejects.toMatchObject({ digest: 'NEXT_HTTP_ERROR_FALLBACK;404' });
  });

  it('logs one iam.call_failed line with both correlation ids and the path, and never the message', async () => {
    const spy = appEvent;
    setRequestHeaders({ 'paigasus-correlation-id': '0198f2c1-8888-7000-8000-000000000001', 'x-paigasus-request-path': '/iam/orgs' });
    await callIam(() => Promise.reject(new ConnectError('secret internal detail', Code.PermissionDenied)));
    expect(spy).toHaveBeenCalledTimes(1);
    const [name, fields] = spy.mock.calls[0] ?? [];
    expect(name).toBe('iam.call_failed');
    expect(fields).toEqual({
      presentation: 'forbidden',
      reason: null,
      code: 'PermissionDenied',
      correlation_id: null,
      request_correlation_id: '0198f2c1-8888-7000-8000-000000000001',
      path: '/iam/orgs',
    });
    expect(JSON.stringify(fields)).not.toContain('secret internal detail');
  });
});

// SMA-662. `callIam` classifies with `instanceof ConnectError`, and this package exists TWICE in
// one process: Next gives a route handler and a page separate module graphs. What makes that safe
// is not this package — it is `ConnectError`'s static Symbol.hasInstance, which falls back to a
// duck-type BRAND (`name === 'ConnectError'` plus code/metadata/details/rawMessage/cause) when the
// prototype does not match (@connectrpc/connect 2.2.0, dist/esm/connect-error.js:82-98). This row
// pins that third-party contract, because nothing else in the repository would notice a connect-es
// release dropping it.
//
// The fixture is a LOCAL class, not a second module copy. `vi.resetModules()` plus a dynamic
// import does NOT duplicate a node_modules package under this config (MEASURED, spec § 10: vitest
// externalizes node_modules, and resetModules clears Vite's registry, not Node's ESM cache), so a
// local class is what reproduces "an object the other copy built" — a foreign prototype carrying
// the right brand.
class ForeignConnectError extends Error {
  readonly code: number;
  readonly metadata = new Headers();
  readonly details: unknown[] = [];
  readonly rawMessage: string;

  constructor(rawMessage: string, code: number) {
    super(`[permission_denied] ${rawMessage}`);
    this.name = 'ConnectError';
    this.code = code;
    this.rawMessage = rawMessage;
    this.cause = undefined;
  }

  /** The brand does not require this; `mapError` calls it. */
  findDetails(): never[] {
    return [];
  }
}

describe('callIam across two module copies (SMA-662)', () => {
  it('maps a ConnectError whose prototype belongs to another copy, instead of rethrowing it', async () => {
    const err = new ForeignConnectError('denied', Code.PermissionDenied);

    // The precondition comes FIRST. Without it the row passes vacuously against a real
    // ConnectError, which is the whole thing it is trying not to be.
    expect(Object.getPrototypeOf(err)).not.toBe(ConnectError.prototype);
    // The BRAND admits it, not the prototype. This is the assertion that reds if connect-es
    // removes Symbol.hasInstance.
    expect(err instanceof ConnectError).toBe(true);

    const result = await callIam(() => Promise.reject(err));

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.error.presentation).toBe('forbidden');
      expect(result.error.transport).toEqual({ kind: 'grpc', code: 7, codeName: 'PermissionDenied' });
    }
  });
});
