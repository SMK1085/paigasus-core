// SPDX-License-Identifier: Apache-2.0
//
// FIX 1 (final whole-branch review, SMA-508) — `createIamClient` was unguarded by any test.
// MEASURED: replacing its body with `return createClient(service, getTransport(options));` —
// discarding `auth` entirely — left the suite at 23/23 passing. Every token-isolation proof in
// tests/iam.test.ts drives the exported-for-test `bindAuth` directly; the only test that touches
// the production factory asserted `typeof client.getOrganization === 'function'`, which bare
// `createClient` also satisfies. This file drives `createIamClient` itself, end to end, by
// mocking `@connectrpc/connect-node` the way tests/transport-wiring.test.ts:27-32 already does,
// so `getTransport`'s real `createGrpcTransport` call is redirected to a recording Transport.
//
// A separate file from tests/iam.test.ts, not an addition to it: `vi.mock` applies to the WHOLE
// file it is declared in, and iam.test.ts's existing tests deliberately exercise `bindAuth`
// against a hand-built Transport with no module mocking involved — mixing the two here would mock
// `@connectrpc/connect-node` for tests that neither need nor expect it.
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ContextValues, StreamResponse, Transport, UnaryResponse } from '@connectrpc/connect';
import type { DescMessage, DescMethodStreaming, DescMethodUnary, MessageInitShape } from '@bufbuild/protobuf';
import { TenancyService } from '@paigasus/proto/iam';

// `HeadersInit` is declared by connect's own d.ts against the DOM lib, which this package's
// tsconfig deliberately excludes (spec § 6.2 layer 4), so the name itself does not resolve here.
// `unknown` is a supertype of every member of that union, so it still satisfies the interface's
// contravariant parameter position (type-only fix, forced by tsc).
type HeaderParam = unknown;

// `vi.hoisted`, not a plain `const` — see tests/transport-wiring.test.ts:9-22 for why: vi.mock's
// factory is hoisted above every top-level statement in the file, so a factory closing over a
// plain `const` throws a TDZ ReferenceError at mock time.
const { seen, createGrpcTransport } = vi.hoisted(() => {
  const seen: (ContextValues | undefined)[] = [];
  // Typed with Transport's own generics, exactly as tests/iam.test.ts's local recordingTransport
  // is — an object-literal method shorthand otherwise infers a narrower, non-generic signature.
  // Not `async`: neither method awaits anything, and @typescript-eslint/require-await treats an
  // async function with no await expression as an error (measured: `moon run ts:lint` reds on
  // exactly that rule).
  const transport: Transport = {
    unary<I extends DescMessage, O extends DescMessage>(
      _method: DescMethodUnary<I, O>,
      _signal: AbortSignal | undefined,
      _timeoutMs: number | undefined,
      _header: HeaderParam,
      _input: MessageInitShape<I>,
      contextValues: ContextValues | undefined,
    ): Promise<UnaryResponse<I, O>> {
      seen.push(contextValues);
      return Promise.resolve({
        stream: false,
        message: {},
        header: new Headers(),
        trailer: new Headers(),
      } as unknown as UnaryResponse<I, O>);
    },
    stream<I extends DescMessage, O extends DescMessage>(
      _method: DescMethodStreaming<I, O>,
      _signal: AbortSignal | undefined,
      _timeoutMs: number | undefined,
      _header: HeaderParam,
      _input: AsyncIterable<MessageInitShape<I>>,
      contextValues: ContextValues | undefined,
    ): Promise<StreamResponse<I, O>> {
      seen.push(contextValues);
      throw new Error('not used');
    },
  };
  return {
    seen,
    // Explicit call-signature type argument, not inferred from the implementation — see
    // tests/transport-wiring.test.ts:13-22 for why a zero-arg inference makes `.mock.calls[0]` a
    // zero-length tuple under vitest@5's Mock<T> typing.
    createGrpcTransport: vi.fn<(options: unknown) => Transport>(() => transport),
  };
});

vi.mock('@connectrpc/connect-node', async (importOriginal) => {
  // Spread the real module: transport.ts also imports Http2SessionManager from it, and a factory
  // returning only the spy would break the constructor call rather than the assertion.
  const actual = await importOriginal<typeof import('@connectrpc/connect-node')>();
  return { ...actual, createGrpcTransport };
});

const { authContextKey, disposeTransports } = await import('../src/transport.js');
const { createIamClient } = await import('../src/iam.js');

afterEach(() => {
  disposeTransports();
  createGrpcTransport.mockClear();
  seen.length = 0;
});

describe('createIamClient wires the bound Auth through the real production path (FIX 1, final review)', () => {
  it('binds the given Auth so a real call carries it as ContextValues', async () => {
    const client = createIamClient(TenancyService, { baseUrl: 'https://iam.invalid' }, { bearer: 'alice-token' });

    await client.getOrganization({});

    expect(seen).toHaveLength(1);
    expect(seen[0]?.get(authContextKey)).toEqual({ bearer: 'alice-token' });
  });
});
