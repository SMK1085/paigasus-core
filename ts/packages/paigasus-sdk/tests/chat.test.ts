// SPDX-License-Identifier: Apache-2.0
import { afterEach, describe, expect, it, vi } from 'vitest';

import { chatCompletion, PaigasusHttpError } from '../src/chat.js';

const OPTIONS = { baseUrl: 'https://gateway.test', auth: { bearer: 'tok' } } as const;
const REQUEST = { model: 'gpt-4o', messages: [{ role: 'user', content: 'hi' }] };

// `BodyInit` is not a global name under this package's `lib: ["ES2022"], types: ["node"]`
// tsconfig (spec § 6.2 layer 4 bans the DOM lib). `Response` itself IS global — @types/node's
// web-globals/fetch.d.ts declares it — so deriving the parameter type from its own constructor
// avoids naming the un-importable type at all.
function respond(body: ConstructorParameters<typeof Response>[0] | null, init: ResponseInit): Response {
  return new Response(body, { ...init, headers: { 'paigasus-correlation-id': 'corr-1', 'paigasus-request-id': 'req-1', ...init.headers } });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('the non-streaming path', () => {
  it('returns kind json with the ids read from the response headers', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(respond(JSON.stringify({ id: 'chatcmpl-1' }), { status: 200, headers: { 'content-type': 'application/json' } }))),
    );

    const result = await chatCompletion(REQUEST, OPTIONS);

    expect(result.kind).toBe('json');
    expect(result.status).toBe(200);
    // Both variants carry the ids, so a caller logging a SUCCESS has something to log.
    expect(result.correlationId).toBe('corr-1');
    expect(result.requestId).toBe('req-1');
    if (result.kind !== 'json') throw new Error('unreachable');
    expect(result.body).toEqual({ id: 'chatcmpl-1' });
  });

  it('sends the bearer and the body', async () => {
    // Explicit call-signature type argument, not inference from the zero-arg implementation: see
    // tests/transport-wiring.test.ts:13-22 for why a zero-arg inference makes `.mock.calls[0]` a
    // zero-length tuple and the `as [string, RequestInit]` cast below a compile error (measured
    // against vitest@5.0.0's Mock<T> typing).
    const fetchSpy = vi.fn<(url: string, init: RequestInit) => Promise<Response>>(() => Promise.resolve(respond('{}', { status: 200, headers: { 'content-type': 'application/json' } })));
    vi.stubGlobal('fetch', fetchSpy);

    await chatCompletion(REQUEST, OPTIONS);

    const [url, init] = fetchSpy.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('https://gateway.test/v1/chat/completions');
    expect(new Headers(init.headers).get('authorization')).toBe('Bearer tok');
    expect(JSON.parse(init.body as string)).toEqual(REQUEST);
  });

  it('sends no authorization header for an anonymous caller', async () => {
    const fetchSpy = vi.fn<(url: string, init: RequestInit) => Promise<Response>>(() => Promise.resolve(respond('{}', { status: 200, headers: { 'content-type': 'application/json' } })));
    vi.stubGlobal('fetch', fetchSpy);

    await chatCompletion(REQUEST, { baseUrl: 'https://gateway.test', auth: { anonymous: true } });

    const [, init] = fetchSpy.mock.calls[0] as [string, RequestInit];
    expect(new Headers(init.headers).has('authorization')).toBe(false);
  });
});

describe('the streaming path — AC 4', () => {
  it('returns the IDENTICAL ReadableStream object', async () => {
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new TextEncoder().encode('data: {}\n\n'));
        controller.close();
      },
    });
    const response = respond(body, { status: 200, headers: { 'content-type': 'text/event-stream' } });
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(response)),
    );

    const result = await chatCompletion({ ...REQUEST, stream: true }, OPTIONS);

    expect(result.kind).toBe('stream');
    if (result.kind !== 'stream') throw new Error('unreachable');
    // Identity, not equivalence. The SDK must not read, buffer, decode or re-encode.
    expect(result.body).toBe(response.body);
    expect(result.correlationId).toBe('corr-1');
    expect(result.requestId).toBe('req-1');
  });

  // chat.rs:138-141 — a `stream:true` request whose upstream answered non-2xx comes back as JSON,
  // not SSE. A client that trusts its own flag misreads this.
  it('branches on content-type, not on the caller stream flag', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(respond(JSON.stringify({ ok: true }), { status: 200, headers: { 'content-type': 'application/json' } }))),
    );

    const result = await chatCompletion({ ...REQUEST, stream: true }, OPTIONS);
    expect(result.kind).toBe('json');
  });
});

describe('a non-2xx throws a PaigasusError', () => {
  it('maps the gateway envelope', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() =>
        Promise.resolve(
          respond(JSON.stringify({ error: { message: 'streaming is disabled', type: 'invalid_request_error', param: 'stream', code: 'streaming-disabled' } }), {
            status: 400,
            headers: { 'content-type': 'application/json', 'paigasus-retryable': 'false' },
          }),
        ),
      ),
    );

    const error = (await chatCompletion(REQUEST, OPTIONS).catch((e: unknown) => e)) as PaigasusHttpError;
    // Pins the contract (spec § 8.2): the thrown value is a real Error, not the plain PaigasusError.
    expect(error).toBeInstanceOf(PaigasusHttpError);
    expect(error.error.presentation).toBe('invalid-input');
    expect(error.error.correlationId).toBe('corr-1');
    expect(error.error.metadata).toEqual({ param: 'stream' });
  });

  it('maps an upstream 429 to rate-limited without resolving OpenAI vocabulary', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() =>
        Promise.resolve(
          respond(JSON.stringify({ error: { message: 'You exceeded your current quota', type: 'insufficient_quota', param: null, code: 'insufficient_quota' } }), {
            status: 429,
            headers: { 'content-type': 'application/json' },
          }),
        ),
      ),
    );

    const error = (await chatCompletion(REQUEST, OPTIONS).catch((e: unknown) => e)) as PaigasusHttpError;
    expect(error.error.presentation).toBe('rate-limited');
    expect(error.error.reason).toBeNull();
    expect(error.error.rawReason).toBe('insufficient_quota');
  });

  it('does not throw while mapping a body that is not JSON', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(respond('<html>502 Bad Gateway</html>', { status: 502, headers: { 'content-type': 'application/json' } }))),
    );

    const error = (await chatCompletion(REQUEST, OPTIONS).catch((e: unknown) => e)) as PaigasusHttpError;
    expect(error.error.presentation).toBe('degraded');
    expect(error.error.correlationId).toBe('corr-1');
  });
});

describe('deadlines', () => {
  it('rejects when the response head does not arrive in time', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_url: string, init: RequestInit) => {
        return await new Promise<Response>((_resolve, reject) => {
          init.signal?.addEventListener('abort', () => {
            reject(new DOMException('aborted', 'AbortError'));
          });
        });
      }),
    );

    const promise = chatCompletion(REQUEST, { ...OPTIONS, timeoutMs: 10 });
    const error = (await promise.catch((e: unknown) => e)) as PaigasusHttpError;
    expect(error.error.presentation).toBe('degraded');
  });

  // F2: the caller-supplied `signal` branch of `AbortSignal.any([options.signal, deadline.signal])`
  // had no test — every other case exercised only the deadline timer.
  it('rejects when the caller aborts while the head is still pending', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn((_url: string, init: RequestInit) => {
        return new Promise<Response>((_resolve, reject) => {
          init.signal?.addEventListener('abort', () => {
            reject(new DOMException('aborted', 'AbortError'));
          });
        });
      }),
    );

    const caller = new AbortController();
    const promise = chatCompletion(REQUEST, { ...OPTIONS, signal: caller.signal });
    caller.abort();

    const error = (await promise.catch((e: unknown) => e)) as PaigasusHttpError;
    expect(error).toBeInstanceOf(PaigasusHttpError);
    expect(error.error.presentation).toBe('degraded');
  });

  // The deadline timer must be CLEARED once the head lands, or the same signal would abort the
  // body mid-stream. This is the proof.
  it('does not abort a slow body after the head has arrived', async () => {
    let signal: AbortSignal | undefined;
    const body = new ReadableStream<Uint8Array>({ start() {} });
    vi.stubGlobal(
      'fetch',
      vi.fn((_url: string, init: RequestInit) => {
        signal = init.signal ?? undefined;
        return Promise.resolve(respond(body, { status: 200, headers: { 'content-type': 'text/event-stream' } }));
      }),
    );

    const result = await chatCompletion({ ...REQUEST, stream: true }, { ...OPTIONS, timeoutMs: 10 });
    expect(result.kind).toBe('stream');
    await new Promise((resolve) => setTimeout(resolve, 40));
    expect(signal?.aborted).toBe(false);
  });
});

describe('cancellation', () => {
  // SCOPED to forwarding: a stub fetch cannot prove a socket closed. See spec § 8.4.
  it('forwards a cancel on the returned stream to the upstream body', async () => {
    const cancel = vi.fn(async () => {});
    const body = new ReadableStream<Uint8Array>({ start() {}, cancel });
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(respond(body, { status: 200, headers: { 'content-type': 'text/event-stream' } }))),
    );

    const result = await chatCompletion({ ...REQUEST, stream: true }, OPTIONS);
    if (result.kind !== 'stream') throw new Error('unreachable');
    await result.body.cancel();

    expect(cancel).toHaveBeenCalled();
  });
});
