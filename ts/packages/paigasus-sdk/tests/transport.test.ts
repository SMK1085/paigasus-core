// SPDX-License-Identifier: Apache-2.0
import { afterAll, afterEach, describe, expect, it } from 'vitest';
import { createContextValues } from '@connectrpc/connect';
import type { UnaryRequest, UnaryResponse } from '@connectrpc/connect';
import { authContextKey, authInterceptor, disposeTransports, getTransport, stableTransportKey } from '../src/transport.js';

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
});
