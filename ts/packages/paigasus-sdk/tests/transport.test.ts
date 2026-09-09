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
  it('is insensitive to property order', () => {
    // Cast: TransportOptions holds ONE field today. The key function is written for the whole
    // options object because spec § 7.1's rule is about the SECOND field — nodeOptions, the TLS
    // trust material — and this assertion is what keeps the rule honest before that field exists.
    const one = stableTransportKey({ baseUrl: 'https://a.invalid', z: 1, a: 2 } as never);
    const two = stableTransportKey({ a: 2, z: 1, baseUrl: 'https://a.invalid' } as never);
    expect(one).toBe(two);
  });

  it('separates two option sets that differ only in a nested field', () => {
    const one = stableTransportKey({ baseUrl: 'https://a.invalid', nodeOptions: { ca: 'X' } } as never);
    const two = stableTransportKey({ baseUrl: 'https://a.invalid', nodeOptions: { ca: 'Y' } } as never);
    expect(one).not.toBe(two);
  });

  it('does not collide two distinct base URLs', () => {
    expect(stableTransportKey({ baseUrl: 'https://a.invalid' })).not.toBe(stableTransportKey({ baseUrl: 'https://b.invalid' }));
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
});
