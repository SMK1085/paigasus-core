// SPDX-License-Identifier: Apache-2.0
//
// AC 6 and the other half of AC 5. The cache tests prove getTransport() memoizes; these prove WHAT
// it memoizes — that the real transport carries the auth interceptor and an explicit deadline, and
// that no token is among the arguments. Without this, `interceptors: [authInterceptor]` could be
// deleted and every other test in the package would still pass.
import { afterEach, describe, expect, it, vi } from 'vitest';

// `vi.hoisted`, NOT a plain `const`. vi.mock's factory is hoisted above every top-level statement
// in the file, so a factory closing over a plain `const` throws a TDZ ReferenceError at mock time.
// vi.hoisted runs its initializer in that same hoisted phase, which is what makes the spy reachable
// from the factory.
// The mock implementation is typed with an explicit (unused) `_options?: unknown` parameter
// rather than `()`: vi.fn() infers the Mock's call-signature arity from the implementation
// passed to it, so a zero-arg implementation makes `.mock.calls[0]` a zero-length tuple and
// `.mock.calls[0]?.[0]` below a compile error (measured against vitest@5.0.0's Mock<T> typing —
// TS2493, "Tuple type '[]' of length '0' has no element at index '0'"). Runtime behavior is
// unchanged: the mock already ignored its argument either way.
const { createGrpcTransport } = vi.hoisted(() => ({
  createGrpcTransport: vi.fn((_options?: unknown) => ({ unary: vi.fn(), stream: vi.fn() })),
}));

vi.mock('@connectrpc/connect-node', async (importOriginal) => {
  // Spread the real module: transport.ts also imports Http2SessionManager from it, and a factory
  // that returned only the spy would break the constructor call rather than the assertion.
  const actual = await importOriginal<typeof import('@connectrpc/connect-node')>();
  return { ...actual, createGrpcTransport };
});

const { DEFAULT_TIMEOUT_MS, authInterceptor, disposeTransports, getTransport } = await import(
  '../src/transport.js'
);

afterEach(() => {
  disposeTransports();
  createGrpcTransport.mockClear();
});

describe('what getTransport builds (spec § 7.2, § 7.4)', () => {
  it('passes the auth interceptor', () => {
    getTransport({ baseUrl: 'https://iam.invalid' });
    const options = createGrpcTransport.mock.calls[0]?.[0] as { interceptors?: unknown[] };
    expect(options.interceptors).toContain(authInterceptor);
  });

  it('sets an explicit 10 s default deadline', () => {
    getTransport({ baseUrl: 'https://iam.invalid' });
    const options = createGrpcTransport.mock.calls[0]?.[0] as { defaultTimeoutMs?: number };
    expect(options.defaultTimeoutMs).toBe(DEFAULT_TIMEOUT_MS);
    expect(DEFAULT_TIMEOUT_MS).toBe(10_000);
  });

  it('passes NO token-shaped argument — the transport identity stays token-free', () => {
    getTransport({ baseUrl: 'https://iam.invalid' });
    const options = createGrpcTransport.mock.calls[0]?.[0] as Record<string, unknown>;
    expect(Object.keys(options).sort()).toEqual(
      ['baseUrl', 'defaultTimeoutMs', 'interceptors', 'sessionManager'].sort(),
    );
  });

  it('builds the transport once per distinct options object', () => {
    getTransport({ baseUrl: 'https://iam.invalid' });
    getTransport({ baseUrl: 'https://iam.invalid' });
    getTransport({ baseUrl: 'https://gateway.invalid' });
    expect(createGrpcTransport).toHaveBeenCalledTimes(2);
  });
});
