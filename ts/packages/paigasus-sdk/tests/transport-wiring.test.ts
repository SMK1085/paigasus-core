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
// The mock is given an explicit call-signature type argument, `(options: unknown) => {...}`,
// rather than inferring one from a zero-arg implementation: vi.fn() otherwise infers the Mock's
// call-signature arity from the implementation function passed to it, so `() => ({...})` makes
// `.mock.calls[0]` a zero-length tuple and `.mock.calls[0]?.[0]` below a compile error (measured
// against vitest@5.0.0's Mock<T> typing — TS2493, "Tuple type '[]' of length '0' has no element
// at index '0'"). A named-but-unused parameter (`_options`) would dodge that error but trip
// @typescript-eslint/no-unused-vars instead, since this repo's eslint config carries no
// `argsIgnorePattern: '^_'` (measured: `moon run ts:lint` reds on exactly that rule). The explicit
// type argument gets the 1-tuple without introducing a binding at all. Runtime behavior is
// unchanged: the mock implementation still ignores its argument.
const { createGrpcTransport } = vi.hoisted(() => ({
  createGrpcTransport: vi.fn<(options: unknown) => { unary: ReturnType<typeof vi.fn>; stream: ReturnType<typeof vi.fn> }>(() => ({ unary: vi.fn(), stream: vi.fn() })),
}));

vi.mock('@connectrpc/connect-node', async (importOriginal) => {
  // Spread the real module: transport.ts also imports Http2SessionManager from it, and a factory
  // that returned only the spy would break the constructor call rather than the assertion.
  const actual = await importOriginal<typeof import('@connectrpc/connect-node')>();
  return { ...actual, createGrpcTransport };
});

const { DEFAULT_TIMEOUT_MS, authInterceptor, disposeTransports, getTransport } = await import('../src/transport.js');

afterEach(() => {
  disposeTransports();
  createGrpcTransport.mockClear();
});

describe('what getTransport builds (spec § 7.2, § 7.4)', () => {
  it('passes the auth interceptor', () => {
    // Pinned to the exact array, not `.toContain`: MEASURED that `expect(undefined).toContain(fn)`
    // PASSES in vitest 5, so a `.toContain` version of this assertion stays green even if the
    // whole `interceptors` key is deleted from the `createGrpcTransport` call in src/transport.ts
    // — see the fix report for the before/after proof.
    getTransport({ baseUrl: 'https://iam.invalid' });
    const options = createGrpcTransport.mock.calls[0]?.[0] as { interceptors?: unknown[] };
    expect(options.interceptors).toEqual([authInterceptor]);
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
    expect(Object.keys(options).sort()).toEqual(['baseUrl', 'defaultTimeoutMs', 'interceptors', 'sessionManager'].sort());
  });

  it('builds the transport once per distinct options object', () => {
    getTransport({ baseUrl: 'https://iam.invalid' });
    getTransport({ baseUrl: 'https://iam.invalid' });
    getTransport({ baseUrl: 'https://gateway.invalid' });
    expect(createGrpcTransport).toHaveBeenCalledTimes(2);
  });
});
