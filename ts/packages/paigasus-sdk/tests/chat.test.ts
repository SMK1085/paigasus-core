// SPDX-License-Identifier: Apache-2.0
import { afterEach, describe, expect, it, vi } from 'vitest';

import { createChatClient } from '../src/chat.js';

const BASE = 'https://gateway.example.test';

/**
 * A stub `fetch` that models undici's abort behaviour faithfully.
 *
 * The body stream ERRORS when the signal aborts. That is what makes the "deadline does not
 * outlive the head" test discriminating: a build using AbortSignal.timeout keeps its signal live
 * after the headers arrive, so this stub's stream errors and the test fails. A stub that ignored
 * the signal would pass under both the correct and the broken build.
 */
function streamingFetch(chunks: readonly { afterMs: number; text: string }[]): typeof globalThis.fetch {
  // Not `async`: the body has no `await`, and an explicit `Promise.resolve` keeps the mock's
  // inferred return type at `Promise<Response>` — the same type an `async` version would have
  // produced — so it stays assignable to `typeof globalThis.fetch` with no cast needed.
  return vi.fn((_url: string | URL | Request, init?: RequestInit) => {
    const signal = init?.signal ?? undefined;
    const timers: NodeJS.Timeout[] = [];
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        for (const { afterMs, text } of chunks) {
          timers.push(setTimeout(() => controller.enqueue(new TextEncoder().encode(text)), afterMs));
        }
        timers.push(setTimeout(() => controller.close(), Math.max(...chunks.map((c) => c.afterMs)) + 10));
        signal?.addEventListener('abort', () => {
          for (const t of timers) clearTimeout(t);
          controller.error(signal.reason);
        });
      },
    });
    return Promise.resolve(new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } }));
  });
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('the chat client sends a credential', () => {
  it('attaches the bearer and posts to /v1/chat/completions', async () => {
    const fetchImpl = vi.fn(() => Promise.resolve(new Response(JSON.stringify({ id: 'x' }), { status: 200, headers: { 'content-type': 'application/json' } })));
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'TOKEN' });

    await client.completions({ model: 'gpt-4o', messages: [] });

    // Without this, a `fetch` that is never called fails the cast below with the unhelpful
    // "undefined is not iterable" rather than a clear "expected fetchImpl to have been called".
    expect(fetchImpl).toHaveBeenCalledOnce();
    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe(`${BASE}/v1/chat/completions`);
    expect(init.method).toBe('POST');
    expect(new Headers(init.headers).get('authorization')).toBe('Bearer TOKEN');
    expect(new Headers(init.headers).get('content-type')).toBe('application/json');
  });

  it('refuses an empty bearer locally rather than sending "Bearer "', () => {
    expect(() => createChatClient({ baseUrl: BASE }, { bearer: '   ' })).toThrow(/empty or whitespace-only bearer/);
  });
});

// Found by the author of the parallel implementation on PR #231, not by CodeRabbit: its finding
// named map-error.ts only, and fixing exactly what was named left the identical defect here.
describe('an empty identifier header on a ChatResult is normalized to null', () => {
  it('on the json arm', async () => {
    const fetchImpl = vi.fn(() =>
      Promise.resolve(new Response(JSON.stringify({ id: 'x' }), { status: 200, headers: { 'content-type': 'application/json', 'paigasus-correlation-id': '', 'paigasus-request-id': '' } })),
    );
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'T' });
    const result = await client.completions({});
    if (result.kind !== 'json') throw new Error('unreachable');
    expect(result.correlationId).toBeNull();
    expect(result.requestId).toBeNull();
  });

  it('on the stream arm', async () => {
    const body = new ReadableStream<Uint8Array>({ start: (c) => c.close() });
    const fetchImpl = vi.fn(() => Promise.resolve(new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream', 'paigasus-correlation-id': '', 'paigasus-request-id': '' } })));
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'T' });
    const result = await client.completions({ stream: true });
    if (result.kind !== 'stream') throw new Error('unreachable');
    expect(result.correlationId).toBeNull();
    expect(result.requestId).toBeNull();
  });
});

describe('the two response shapes', () => {
  it('returns kind json for a 2xx JSON response', async () => {
    const fetchImpl = vi.fn(() => Promise.resolve(new Response(JSON.stringify({ id: 'chat-1' }), { status: 200, headers: { 'content-type': 'application/json' } })));
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'T' });

    const result = await client.completions({});
    expect(result).toEqual({ kind: 'json', status: 200, body: { id: 'chat-1' }, correlationId: null, requestId: null });
  });

  // Fix wave item 4: `correlation.rs:174-175` sets both id headers on every response head,
  // success included, so the `json` arm must carry them rather than dropping them silently.
  it('carries the correlation and request ids on a 2xx JSON response when the head has them', async () => {
    const fetchImpl = vi.fn(() =>
      Promise.resolve(
        new Response(JSON.stringify({ id: 'chat-1' }), { status: 200, headers: { 'content-type': 'application/json', 'paigasus-correlation-id': 'corr-json', 'paigasus-request-id': 'req-json' } }),
      ),
    );
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'T' });

    const result = await client.completions({});
    expect(result.kind).toBe('json');
    if (result.kind !== 'json') throw new Error('unreachable');
    expect(result.correlationId).toBe('corr-json');
    expect(result.requestId).toBe('req-json');
  });

  // AC 4. The SDK does not read, buffer, decode or re-encode the stream.
  it('returns the IDENTICAL ReadableStream object for a 2xx SSE response', async () => {
    const body = new ReadableStream<Uint8Array>({ start: (c) => c.close() });
    const response = new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } });
    const fetchImpl = vi.fn(() => Promise.resolve(response));
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'T' });

    const result = await client.completions({ stream: true });
    expect(result.kind).toBe('stream');
    if (result.kind !== 'stream') throw new Error('unreachable');
    expect(result.body).toBe(response.body);
    // No id header was set on this response, so both must read back as null, not undefined.
    expect(result.correlationId).toBeNull();
    expect(result.requestId).toBeNull();
  });

  // THE case the fix wave closes: a stream that fails mid-flight has no head of its own to read
  // an id from (map-error.ts's terminal-frame arm calls `mapHttp(200, new Headers(), body)`), so
  // the id has to come off the ORIGINAL response head, carried on the `stream` arm itself.
  it('carries the correlation and request ids on a 2xx SSE response when the head has them', async () => {
    const body = new ReadableStream<Uint8Array>({ start: (c) => c.close() });
    const response = new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream', 'paigasus-correlation-id': 'corr-stream', 'paigasus-request-id': 'req-stream' } });
    const fetchImpl = vi.fn(() => Promise.resolve(response));
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'T' });

    const result = await client.completions({ stream: true });
    expect(result.kind).toBe('stream');
    if (result.kind !== 'stream') throw new Error('unreachable');
    expect(result.correlationId).toBe('corr-stream');
    expect(result.requestId).toBe('req-stream');
  });

  // chat.rs:139-141 — a stream:true request that fails BEFORE the head is committed answers as
  // plain JSON, not SSE. The caller must branch on content-type, not on its own stream flag.
  it('maps a non-2xx JSON answer to a stream:true request', async () => {
    const fetchImpl = vi.fn(() =>
      Promise.resolve(
        new Response(JSON.stringify({ error: { message: 'no', type: 'invalid_request_error', param: null, code: 'streaming-disabled' } }), {
          status: 501,
          headers: { 'content-type': 'application/json', 'paigasus-correlation-id': 'corr-9' },
        }),
      ),
    );
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'T' });

    const result = await client.completions({ stream: true });
    expect(result.kind).toBe('error');
    if (result.kind !== 'error') throw new Error('unreachable');
    expect(result.error.presentation).toBe('disabled');
    expect(result.error.correlationId).toBe('corr-9');
  });
});

describe('the client never throws a mapped error', () => {
  // MEASURED on Node 22.22.3: `Response.text()` on a body stream that errors REJECTS, not
  // throws synchronously. `readBody` is called on the error path here, so an unhandled rejection
  // would turn a recoverable failure into an unhandled exception — the failure this suite exists
  // to close off. This row proves `completions()` RESOLVES to `{ kind: 'error' }` instead.
  it('resolves to kind error, not a rejection, when the body stream errors mid-read', async () => {
    const body = new ReadableStream<Uint8Array>({
      pull(controller) {
        controller.error(new Error('stream reset'));
      },
    });
    const fetchImpl = vi.fn(() => Promise.resolve(new Response(body, { status: 500, headers: { 'content-type': 'application/json' } })));
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'T' });

    const result = await client.completions({});
    expect(result.kind).toBe('error');
    if (result.kind !== 'error') throw new Error('unreachable');
    expect(result.error.message).toBe('HTTP 500');
  });

  // The SUCCESS path is the one that could lie. `readBody` reports a failed read distinctly from a
  // parsed JSON `null`, so a 2xx whose body breaks mid-read becomes a transport error rather than
  // `{ kind: 'json', body: null }` — which a caller could not tell apart from a server that really
  // did send `null`.
  it('does not pass off a failed body read on a 2xx as a null JSON body', async () => {
    const body = new ReadableStream<Uint8Array>({
      pull(controller) {
        controller.error(new Error('stream reset'));
      },
    });
    const fetchImpl = vi.fn(() => Promise.resolve(new Response(body, { status: 200, headers: { 'content-type': 'application/json' } })));
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'T' });

    const result = await client.completions({});
    expect(result.kind).toBe('error');
    if (result.kind !== 'error') throw new Error('unreachable');
    expect(result.error.transport).toEqual({ kind: 'transport', cause: 'network' });
    expect(result.error.presentation).toBe('degraded');
  });

  it('maps a rejected fetch to the transport arm', async () => {
    // A real `fetch` REJECTS on a network failure; it does not throw synchronously. A rejection
    // is caught the same way a synchronous throw would be, since the call happens inside the
    // `try` around `await fetchImpl(...)` in src/chat.ts — this form just matches reality.
    const fetchImpl = vi.fn(() => Promise.reject(new TypeError('fetch failed')));
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'T' });

    const result = await client.completions({});
    expect(result.kind).toBe('error');
    if (result.kind !== 'error') throw new Error('unreachable');
    expect(result.error.transport).toEqual({ kind: 'transport', cause: 'network' });
    expect(result.error.presentation).toBe('degraded');
  });

  it('reports a caller abort as aborted, not as a service fault', async () => {
    const controller = new AbortController();
    // `throwIfAborted()` is the ONLY way this mock produces the "aborted" outcome — no
    // unconditional fallback throw. A fallback throw would make this row pass even if the
    // caller's signal never reached `fetch` at all: src/chat.ts classifies an abort from its
    // OWN `callerSignal` variable, not from what got thrown, so any throw here would read as
    // "aborted" regardless of whether `init.signal` itself was aborted. Falling through to a
    // mundane 2xx response instead means this row only reports "aborted" when the signal
    // `fetch` actually received is the one that's aborted — exactly what dropping caller-signal
    // forwarding (src/chat.ts's composite `signal:` expression) would break.
    const fetchImpl = vi.fn((_u: unknown, init?: RequestInit) => {
      init?.signal?.throwIfAborted();
      return Promise.resolve(new Response(null, { status: 200 }));
    });
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'T' });
    controller.abort();

    const result = await client.completions({}, { signal: controller.signal });
    expect(result.kind).toBe('error');
    if (result.kind !== 'error') throw new Error('unreachable');
    expect(result.error.transport).toEqual({ kind: 'transport', cause: 'aborted' });
    expect(result.error.presentation).toBe('generic');
  });
});

describe('the pre-header deadline', () => {
  it('fires when the headers never arrive', async () => {
    const fetchImpl = vi.fn(async (_u: unknown, init?: RequestInit) => {
      return await new Promise<Response>((_res, rej) => {
        init?.signal?.addEventListener('abort', () => {
          // `signal.reason` is typed `any`; narrow it before handing it to `reject` so the
          // rejection value is always Error-like without changing which branch this test
          // exercises — chat.ts's `timedOut` flag decides that, not this value's identity.
          const reason: unknown = init.signal?.reason;
          rej(reason instanceof Error ? reason : new Error(String(reason)));
        });
      });
    });
    const client = createChatClient({ baseUrl: BASE, headerTimeoutMs: 40, fetch: fetchImpl }, { bearer: 'T' });

    const result = await client.completions({});
    expect(result.kind).toBe('error');
    if (result.kind !== 'error') throw new Error('unreachable');
    expect(result.error.transport).toEqual({ kind: 'transport', cause: 'timeout' });
  });

  // THE row that separates the correct build from the broken one. Under
  // AbortSignal.any([caller, AbortSignal.timeout(N)]) the signal stays live after the headers
  // arrive, so the stub's stream errors at N and this test fails. Under a manual
  // AbortController cleared when fetch settles, the chunk arrives.
  it('does NOT abort the body after the headers arrive', async () => {
    const fetchImpl = streamingFetch([{ afterMs: 120, text: 'data: {"choices":[]}\n\n' }]);
    const client = createChatClient({ baseUrl: BASE, headerTimeoutMs: 40, fetch: fetchImpl }, { bearer: 'T' });

    const result = await client.completions({ stream: true });
    expect(result.kind).toBe('stream');
    if (result.kind !== 'stream') throw new Error('unreachable');

    const chunks: string[] = [];
    const decoder = new TextDecoder();
    for await (const chunk of result.body as unknown as AsyncIterable<Uint8Array>) {
      chunks.push(decoder.decode(chunk));
    }
    expect(chunks.join('')).toContain('"choices"');
  });
});

describe('cancellation', () => {
  // The SDK returns the platform's OWN stream, so propagation to a socket is undici's behaviour,
  // not this package's. What the SDK controls is that the object is the platform's — which is
  // what makes propagation possible — and that is asserted here and in the identity row above.
  // End-to-end propagation to a socket is NOT tested; see the plan's stated gaps.
  it('cancelling the returned stream reaches the source', async () => {
    let cancelled = false;
    const body = new ReadableStream<Uint8Array>({
      start: (c) => c.enqueue(new TextEncoder().encode('data: {}\n\n')),
      cancel: () => {
        cancelled = true;
      },
    });
    const fetchImpl = vi.fn(() => Promise.resolve(new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } })));
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'T' });

    const result = await client.completions({ stream: true });
    if (result.kind !== 'stream') throw new Error('unreachable');
    await result.body.cancel();
    expect(cancelled).toBe(true);
  });

  // Closes the gap the code review found: no row asserted that the CALLER'S OWN signal reaches
  // `fetch`, nor that it stays live past `clearTimeout` — which is exactly what the caller needs
  // once the head is committed, since nothing else can end the stream at that point. This row
  // requires BOTH: the composite signal must include `callerSignal` at the `fetch` call, and it
  // must still be listening after src/chat.ts's `finally { clearTimeout(timer); }` has run.
  it('forwards a caller abort into the underlying fetch after the head is committed', async () => {
    const callerController = new AbortController();
    const fetchImpl = streamingFetch([{ afterMs: 100, text: 'data: {"choices":[]}\n\n' }]);
    const client = createChatClient({ baseUrl: BASE, fetch: fetchImpl }, { bearer: 'T' });

    const result = await client.completions({ stream: true }, { signal: callerController.signal });
    expect(result.kind).toBe('stream');
    if (result.kind !== 'stream') throw new Error('unreachable');

    // The head is committed and the header timer already cleared by this point. Aborting the
    // caller's OWN controller now is the only thing left in this test that can reach the stream.
    callerController.abort();

    // The assertion is that this loop rejects, not what it yields — `chunks` is never asserted on.
    const chunks: Uint8Array[] = [];
    await expect(async () => {
      for await (const chunk of result.body as unknown as AsyncIterable<Uint8Array>) {
        chunks.push(chunk);
      }
    }).rejects.toThrow();
  });
});
