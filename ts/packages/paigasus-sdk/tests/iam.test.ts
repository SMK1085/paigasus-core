// SPDX-License-Identifier: Apache-2.0
//
// AC 5. There is no live IAM service in CI (spec § 10 states this gap), so the proof runs against a
// recording Transport: it captures the ContextValues each call carries, and then drives the REAL
// authInterceptor over a synthetic request to turn that into the header a server would see.
import { afterEach, describe, expect, it } from 'vitest';
import { createClient, createContextValues } from '@connectrpc/connect';
import type { ContextValues, StreamRequest, StreamResponse, Transport, UnaryRequest, UnaryResponse } from '@connectrpc/connect';
import type { DescMessage, DescMethodStreaming, DescMethodUnary, MessageInitShape } from '@bufbuild/protobuf';
import { TenancyService } from '@paigasus/proto/iam';
import { authContextKey, authInterceptor, disposeTransports } from '../src/transport.js';
import { bindAuth, createIamClient } from '../src/iam.js';

afterEach(() => {
  disposeTransports();
});

// `HeadersInit` is declared by connect's own d.ts against the DOM lib, which this package's
// tsconfig deliberately excludes (spec § 6.2 layer 4), so the name itself does not resolve here.
// `unknown` is used for this unread parameter instead: it is a supertype of every member of the
// `HeadersInit` union, so it still satisfies the interface's contravariant parameter position
// (type-only fix, forced by tsc, not the brief).
type HeaderParam = unknown;

type Recorder = { readonly transport: Transport; readonly seen: (ContextValues | undefined)[] };

function recordingTransport(): Recorder {
  const seen: (ContextValues | undefined)[] = [];
  const transport: Transport = {
    // Typed with Transport's own generics rather than left to inference: an object-literal method
    // shorthand infers a NON-generic signature from its first call site, which is narrower than the
    // `Transport` interface it must satisfy (spec-adjacent type-only fix, forced by tsc, not the brief).
    // Not `async`: neither method awaits anything, and `@typescript-eslint/require-await` treats
    // an `async` function with no `await` as an error. The interface's `Promise<...>` return type
    // is satisfied directly via `Promise.resolve` / a synchronous `throw` instead (type-only fix,
    // forced by eslint, not the brief).
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
  return { transport, seen };
}

// Turn a captured ContextValues into the header the real interceptor would produce.
async function headerFor(contextValues: ContextValues | undefined): Promise<Headers> {
  const header = new Headers();
  const req = { stream: false, header, contextValues: contextValues ?? createContextValues() } as unknown as UnaryRequest;
  // Widened to the Interceptor's own `UnaryRequest | StreamRequest` union: a `next` accepting only
  // `UnaryRequest` is not assignable to `AnyFn` under strict function-parameter variance (type-only
  // fix, forced by tsc, not the brief).
  // Not `async`, for the same `require-await` reason as `recordingTransport` above.
  const next = (r: UnaryRequest | StreamRequest): Promise<UnaryResponse | StreamResponse> => Promise.resolve({ stream: false, header: r.header } as unknown as UnaryResponse);
  await authInterceptor(next)(req);
  return header;
}

describe('AC 5 — two clients over ONE transport do not share a token (spec § 7.5)', () => {
  it('produces two different Authorization headers, neither carrying the other token', async () => {
    const { transport, seen } = recordingTransport();

    const alice = bindAuth(createClient(TenancyService, transport), { bearer: 'alice-token' });
    const bob = bindAuth(createClient(TenancyService, transport), { bearer: 'bob-token' });

    await alice.getOrganization({});
    await bob.getOrganization({});

    expect(seen).toHaveLength(2);
    const [first, second] = await Promise.all([headerFor(seen[0]), headerFor(seen[1])]);

    expect(first.get('authorization')).toBe('Bearer alice-token');
    expect(second.get('authorization')).toBe('Bearer bob-token');
    expect(first.get('authorization')).not.toBe(second.get('authorization'));
    expect(first.get('authorization')).not.toContain('bob-token');
    expect(second.get('authorization')).not.toContain('alice-token');
  });

  it('sends no Authorization header for an anonymous client', async () => {
    const { transport, seen } = recordingTransport();
    const anon = bindAuth(createClient(TenancyService, transport), { anonymous: true });

    await anon.getOrganization({});

    const header = await headerFor(seen[0]);
    expect(header.get('authorization')).toBeNull();
  });

  it('binds the Auth on EVERY call, not only the first', async () => {
    const { transport, seen } = recordingTransport();
    const alice = bindAuth(createClient(TenancyService, transport), { bearer: 'alice-token' });

    await alice.getOrganization({});
    await alice.getOrganization({});

    expect(seen).toHaveLength(2);
    for (const captured of seen) {
      expect(captured?.get(authContextKey)).toEqual({ bearer: 'alice-token' });
    }
  });
});

describe('D2 — a caller-supplied contextValues is refused, not silently dropped', () => {
  it('throws, naming the reason', () => {
    const { transport } = recordingTransport();
    const client = bindAuth(createClient(TenancyService, transport), { bearer: 'alice-token' });

    expect(() => client.getOrganization({}, { contextValues: createContextValues() })).toThrow(/contextValues/);
  });
});

describe('createIamClient wires the cached transport', () => {
  it('returns a client whose methods are callable', () => {
    const client = createIamClient(TenancyService, { baseUrl: 'https://iam.invalid' }, { anonymous: true });
    expect(typeof client.getOrganization).toBe('function');
  });
});
