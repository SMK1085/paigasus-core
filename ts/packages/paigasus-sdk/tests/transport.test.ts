// SPDX-License-Identifier: Apache-2.0
import { afterAll, afterEach, describe, expect, it } from 'vitest';
import { Code, ConnectError, createContextValues } from '@connectrpc/connect';
import type { StreamRequest, UnaryRequest, UnaryResponse } from '@connectrpc/connect';
import { authContextKey, authInterceptor, disposeTransports, getTransport, stableTransportKey } from '../src/transport.js';
import { presentationForGrpcCode } from '../src/errors.js';

// An open HTTP/2 session keeps the Node process alive, so without this `vitest run` can hang after
// the assertions pass (spec § 7.3). Nothing here opens a socket — createGrpcTransport is lazy — but
// the hook is the contract this package exports disposeTransports() for.
afterAll(() => {
  disposeTransports();
});

afterEach(() => {
  disposeTransports();
});

describe('transport cache identity (spec § 7.1)', () => {
  it('returns the SAME object for equal options', () => {
    const a = getTransport({ baseUrl: 'https://iam.invalid' });
    const b = getTransport({ baseUrl: 'https://iam.invalid' });
    expect(a).toBe(b);
  });

  it('returns DIFFERENT objects for differing options', () => {
    const a = getTransport({ baseUrl: 'https://iam.invalid' });
    const b = getTransport({ baseUrl: 'https://gateway.invalid' });
    expect(a).not.toBe(b);
  });

  it('disposeTransports() empties the cache, so the next get rebuilds', () => {
    const before = getTransport({ baseUrl: 'https://iam.invalid' });
    disposeTransports();
    const after = getTransport({ baseUrl: 'https://iam.invalid' });
    expect(after).not.toBe(before);
  });
});

describe('stableTransportKey (spec § 7.1)', () => {
  it('does not collide two distinct base URLs', () => {
    expect(stableTransportKey({ baseUrl: 'https://a.invalid' })).not.toBe(stableTransportKey({ baseUrl: 'https://b.invalid' }));
  });

  // The two tests below used to exercise `serialize`'s order-insensitivity and nested-field
  // handling by smuggling extra, undeclared keys (`z`/`a`, then a hypothetical `nodeOptions`)
  // past the type system via `as never`. Final review (SMA-508) MEASURED that the same trick
  // works with NO cast at all against a variable typed wider than `TransportOptions` — TypeScript's
  // excess-property check only fires on an object literal — and that `serialize` folded the extra
  // key into the cache key regardless, silently forking a transport (and an HTTP/2 session) per
  // distinct value. `TRANSPORT_OPTION_KEYS` now closes that: an undeclared own key throws instead
  // of joining the key. That supersedes both tests, since their premise — an undeclared key
  // reaching `serialize` at all — is exactly what is now refused. `serialize`'s recursive,
  // order-insensitive sort still exists (see the comment above `stableTransportKey`) and will be
  // re-exercised through a real multi-field `TransportOptions` the day a second field is added.
  it('throws for an own key outside TRANSPORT_OPTION_KEYS, so an undeclared option cannot silently join the cache key', () => {
    // `as never`: TypeScript's excess-property check only fires on an object LITERAL passed
    // directly to a typed parameter, never on a wider-typed variable — so this reproduces the
    // exact gap `getTransport(wider)` has at a real call site, rather than hiding it.
    const wider = { baseUrl: 'https://a.invalid', bearer: 'SECRET' } as never;
    expect(() => stableTransportKey(wider)).toThrow(/bearer/);
  });

  it('throws for a future nodeOptions field until TRANSPORT_OPTION_KEYS is extended for it', () => {
    const wider = { baseUrl: 'https://a.invalid', nodeOptions: { ca: 'X' } } as never;
    expect(() => stableTransportKey(wider)).toThrow(/nodeOptions/);
  });

  // SMA-508: `Object.keys`/`Object.entries` see only own enumerable properties, but `getTransport`
  // reads `options.baseUrl` through ordinary property access, which resolves the prototype chain
  // and ignores enumerability. Each case below is a shape where the key check and `serialize` see
  // nothing, while `getTransport` would still read a real `baseUrl` off it.
  it('throws for an inherited baseUrl', () => {
    const inherited = Object.create({ baseUrl: 'https://iam.invalid' }) as never;
    expect(() => stableTransportKey(inherited)).toThrow(/own, enumerable string property/);
  });

  it('throws for an own but non-enumerable baseUrl', () => {
    const nonEnumerable = Object.defineProperty({}, 'baseUrl', {
      value: 'https://iam.invalid',
      enumerable: false,
    }) as never;
    expect(() => stableTransportKey(nonEnumerable)).toThrow(/own, enumerable string property/);
  });

  it('throws for an accessor baseUrl, since a getter could return a different value on a later read', () => {
    const accessor = {
      get baseUrl() {
        return 'https://iam.invalid';
      },
    } as never;
    expect(() => stableTransportKey(accessor)).toThrow(/own, enumerable string property/);
  });

  it('throws for a non-string baseUrl', () => {
    const nonString = { baseUrl: 42 } as never;
    expect(() => stableTransportKey(nonString)).toThrow(/own, enumerable string property/);
  });
});

function fakeUnaryRequest(): UnaryRequest {
  return {
    stream: false,
    header: new Headers(),
    contextValues: createContextValues(),
  } as unknown as UnaryRequest;
}

/**
 * The same stand-in with `stream: true`. `authInterceptor` is an `Interceptor`, whose `next` takes
 * `UnaryRequest | StreamRequest`, and createGrpcTransport installs it on both arms — so a rule that
 * checked `req.stream` would apply to half the surface (spec M5).
 */
function fakeStreamRequest(): StreamRequest {
  return {
    stream: true,
    header: new Headers(),
    contextValues: createContextValues(),
  } as unknown as StreamRequest;
}

// Typed via `Parameters<typeof authInterceptor>[0]` rather than `(req: UnaryRequest) =>
// Promise<UnaryResponse>`: `Interceptor`'s `next` parameter is `AnyFn`, which accepts
// `UnaryRequest | StreamRequest` and returns `Promise<UnaryResponse | StreamResponse>` — a
// function typed to accept ONLY UnaryRequest is not assignable to that wider parameter type
// (measured against @connectrpc/connect@2.2.0's strict function-parameter variance). The runtime
// behavior is unchanged; only the static type of this stand-in `next` moved to match what
// `authInterceptor` actually requires.
//
// Not `async`: this stand-in never awaits anything, and this repo's eslint config enforces
// @typescript-eslint/require-await (measured: `moon run ts:lint` reds an `async` arrow with no
// `await` expression). `Promise.resolve(...)` returns the same `Promise<UnaryResponse |
// StreamResponse>` shape without the keyword.
const noopNext: Parameters<typeof authInterceptor>[0] = (req) => Promise.resolve({ stream: false, header: req.header } as unknown as UnaryResponse);

/**
 * The rejection reason of a promise, or a hard failure if it resolved.
 *
 * `expect(...).rejects.toThrow(/re/)` cannot assert on a thrown value's TYPE or its `code`, and a
 * bare `.catch(e => e)` cannot tell a rejection from a resolution. This does both.
 */
async function rejection(promise: Promise<unknown>): Promise<unknown> {
  try {
    await promise;
  } catch (error: unknown) {
    return error;
  }
  throw new Error('expected the call to reject, but it resolved');
}

describe('authInterceptor (spec § 7.4)', () => {
  it('sets an Authorization header from a bearer Auth', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { bearer: 'token-a' });
    await authInterceptor(noopNext)(req);
    expect(req.header.get('authorization')).toBe('Bearer token-a');
  });

  it('sets NO Authorization header for an anonymous Auth', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    await authInterceptor(noopNext)(req);
    expect(req.header.get('authorization')).toBeNull();
  });

  it('defaults to anonymous when no Auth was bound', async () => {
    const req = fakeUnaryRequest();
    await authInterceptor(noopNext)(req);
    expect(req.header.get('authorization')).toBeNull();
  });

  it('throws, naming the cause, for an empty bearer rather than sending it', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { bearer: '' });
    await expect(authInterceptor(noopNext)(req)).rejects.toThrow(/empty|whitespace/);
  });

  it('throws for a whitespace-only bearer', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { bearer: '   ' });
    await expect(authInterceptor(noopNext)(req)).rejects.toThrow(/empty|whitespace/);
  });

  it('refuses an empty bearer with Code.InvalidArgument, so the error map reports invalid-input', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { bearer: '' });

    // A plain Error reaches the caller as ConnectError(Code.Unknown), and
    // src/errors/transport-status.ts has NO Unknown row — presentationForGrpcCode falls through to
    // `generic`, so the SDK would render a CALLER error as an unclassified SERVICE failure
    // (spec § 3.1). The presentation assertion is what pins the reason, not just the code.
    const error = await rejection(authInterceptor(noopNext)(req));
    expect(error).toBeInstanceOf(ConnectError);
    expect((error as ConnectError).code).toBe(Code.InvalidArgument);
    expect(presentationForGrpcCode((error as ConnectError).code)).toBe('invalid-input');
  });
});

describe('a caller-supplied authorization header is refused (SMA-627 spec § 3)', () => {
  it('refuses it on the anonymous arm, rather than forwarding a credential from a client declared unauthenticated', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    req.header.set('authorization', 'Bearer caller-token');
    await expect(authInterceptor(noopNext)(req)).rejects.toThrow(/authorization/);
  });

  it('refuses it on the bearer arm, rather than silently overwriting it', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { bearer: 'sdk-token' });
    req.header.set('authorization', 'Bearer caller-token');
    await expect(authInterceptor(noopNext)(req)).rejects.toThrow(/authorization/);
  });

  // Headers.has is case-insensitive by construction, so these two cannot fail against any
  // plausible implementation. Kept as defence in depth, NOT counted as coverage (spec § 5.1).
  it.each(['Authorization', 'AUTHORIZATION'])('refuses it spelled %s', async (name) => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    req.header.set(name, 'Bearer caller-token');
    await expect(authInterceptor(noopNext)(req)).rejects.toThrow(/authorization/);
  });

  // A present-but-empty header is still the caller reaching around the binding, and Headers.has
  // reports it as present (spec § 3.4). The bearer row additionally proves the check runs BEFORE
  // req.header.set — after it, the header would be non-empty and the case would be meaningless.
  it.each([
    ['anonymous', { anonymous: true }],
    ['bearer', { bearer: 'sdk-token' }],
  ] as const)('refuses a present-but-empty header on the %s arm', async (_label, auth) => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, auth);
    req.header.set('authorization', '');
    await expect(authInterceptor(noopNext)(req)).rejects.toThrow(/authorization/);
  });

  it('reports the HEADER, not the empty bearer, when a request is wrong in both ways', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { bearer: '' });
    req.header.set('authorization', 'Bearer caller-token');

    // The mutation this detects: a check placed AFTER the contextValues.get branch reports the
    // empty bearer instead, and the ordering spec § 3.6 fixes becomes accidental.
    const message = ((await rejection(authInterceptor(noopNext)(req))) as Error).message;
    expect(message).toMatch(/authorization/);
    expect(message).not.toMatch(/empty|whitespace/);
  });

  it('refuses with Code.InvalidArgument, so the error map reports invalid-input', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    req.header.set('authorization', 'Bearer caller-token');

    const error = await rejection(authInterceptor(noopNext)(req));
    expect(error).toBeInstanceOf(ConnectError);
    expect((error as ConnectError).code).toBe(Code.InvalidArgument);
    expect(presentationForGrpcCode((error as ConnectError).code)).toBe('invalid-input');
  });

  it('names the cause and both remedies, and NEVER echoes the credential', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    req.header.set('authorization', 'Bearer super-secret-value');

    // The mutation this detects: `...: ${req.header.get('authorization')}`, the obvious
    // debugging-friendly form, which writes a live credential into an exception message, a
    // container log and any error reporter (spec § 3.5).
    const message = ((await rejection(authInterceptor(noopNext)(req))) as Error).message;
    expect(message).toMatch(/authorization/);
    expect(message).toMatch(/bearer/i);
    expect(message).toMatch(/anonymous/);
    expect(message).toMatch(/proxy-authorization/);
    expect(message).not.toContain('super-secret-value');
  });

  it('leaves proxy-authorization untouched on the anonymous arm, and adds no authorization', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    req.header.set('proxy-authorization', 'Basic Zm9vOmJhcg==');

    // A RECORDING next, not the shared noopNext: noopNext returns the SAME Headers object it was
    // given (tests/transport.test.ts:120), so asserting on the returned header would prove nothing
    // about `next` having been called at all.
    const seen: (string | null)[] = [];
    const recordingNext: Parameters<typeof authInterceptor>[0] = (r) => {
      seen.push(r.header.get('proxy-authorization'));
      return Promise.resolve({ stream: false, header: r.header } as unknown as UnaryResponse);
    };

    await authInterceptor(recordingNext)(req);
    expect(seen).toEqual(['Basic Zm9vOmJhcg==']);
    expect(req.header.get('authorization')).toBeNull();
  });

  it('claims exactly one header: proxy-authorization, cookie and a custom header survive a bearer call', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { bearer: 'sdk-token' });
    req.header.set('proxy-authorization', 'Basic Zm9vOmJhcg==');
    req.header.set('cookie', 'sid=abc');
    req.header.set('x-paigasus-probe', 'kept');

    // The mutation these detect: a check written on a substring or a regex rather than an exact
    // field name, which would refuse proxy-authorization too (spec § 3.3).
    await authInterceptor(noopNext)(req);
    expect(req.header.get('authorization')).toBe('Bearer sdk-token');
    expect(req.header.get('proxy-authorization')).toBe('Basic Zm9vOmJhcg==');
    expect(req.header.get('cookie')).toBe('sid=abc');
    expect(req.header.get('x-paigasus-probe')).toBe('kept');
  });

  it('refuses it on the STREAMING path too', async () => {
    const req = fakeStreamRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    req.header.set('authorization', 'Bearer caller-token');

    // The mutation this detects: `if (!req.stream && req.header.has(...))`, which every unary case
    // above still passes.
    await expect(authInterceptor(noopNext)(req)).rejects.toThrow(/authorization/);
  });

  it('still binds the bearer on the STREAMING path when no caller header is present', async () => {
    const req = fakeStreamRequest();
    req.contextValues.set(authContextKey, { bearer: 'sdk-token' });
    await authInterceptor(noopNext)(req);
    expect(req.header.get('authorization')).toBe('Bearer sdk-token');
  });
});
