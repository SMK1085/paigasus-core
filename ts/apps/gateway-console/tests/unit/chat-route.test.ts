// SPDX-License-Identifier: Apache-2.0
//
// The playground's route handler (SMA-635 spec § 6.2, § 7.1), driven through its factory with an
// injected chat client. No Next server, no gateway.
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ChatCallOptions, ChatClient, ChatResult } from '@paigasus/sdk/chat';
import { mapError } from '@paigasus/sdk/errors';
import { logger } from '@paigasus/console-core';
import { CHAT_HEADER_TIMEOUT_MS, UPSTREAM_REJECTED_MESSAGE, createChatRoute, type ChatRouteDeps } from '../../lib/chat-route';

const ORIGIN = 'https://console.test';
const ORG = '0190a100-0000-7000-8000-0000000000e1';
const CORRELATION = '11111111-2222-4333-8444-555555555555';
const GOOD = { org: ORG, model: 'gpt-e2e', messages: [{ role: 'user', content: 'hi' }] };
const TERMINAL = '\n\ndata: {"error":{"message":"upstream stream error","type":"api_error","param":null,"code":"upstream-error"}}\n\n';

afterEach(() => {
  vi.restoreAllMocks();
});

function streamOf(chunks: readonly string[], opts: { error?: boolean; onCancel?: () => void } = {}): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  let i = 0;
  return new ReadableStream<Uint8Array>({
    pull(controller) {
      const next = chunks[i];
      i += 1;
      if (next !== undefined) controller.enqueue(encoder.encode(next));
      else if (opts.error === true) controller.error(new TypeError('terminated'));
      else controller.close();
    },
    cancel() {
      opts.onCancel?.();
    },
  });
}

function setup(result: ChatResult, overrides: Partial<ChatRouteDeps> = {}) {
  // eslint-disable-next-line @typescript-eslint/no-unused-vars -- the stub's signature must match `ChatClient['completions']`, but the fixed result ignores both arguments.
  const completions = vi.fn((_request: Record<string, unknown>, _options?: ChatCallOptions): Promise<ChatResult> => Promise.resolve(result));
  const client: ChatClient = { completions };
  // eslint-disable-next-line @typescript-eslint/no-unused-vars -- the stub's signature must match `ChatRouteDeps['chatClient']`, but the fixed client ignores both arguments.
  const chatClient = vi.fn((_options: { readonly baseUrl: string; readonly headerTimeoutMs: number }, _auth: { readonly bearer: string }): ChatClient => client);
  const route = createChatRoute({
    publicOrigin: () => Promise.resolve(ORIGIN),
    session: () => Promise.resolve({ accessToken: 'session-token' }),
    gatewayBaseUrl: () => 'http://gateway.test',
    correlationId: () => Promise.resolve(CORRELATION),
    chatClient,
    maxBodyBytes: 1_000,
    ...overrides,
  });
  return { route, completions, chatClient };
}

function post(body: BodyInit | null, headers: Record<string, string> = {}): Request {
  return new Request(`${ORIGIN}/gateway/api/chat`, { method: 'POST', headers: { origin: ORIGIN, 'content-type': 'application/json', ...headers }, body, duplex: 'half' } as RequestInit);
}

/** A FRESH stream per call: a ReadableStream can be read once, so a shared constant would lock. */
const streamOk = (): ChatResult => ({ kind: 'stream', body: streamOf(['data: {"choices":[{"delta":{"content":"hi"}}]}\n\n', 'data: [DONE]\n\n']), correlationId: 'gw-corr', requestId: 'gw-req' });

async function errorOf(response: Response): Promise<{ presentation: string; rawReason: string | null; correlationId: string | null; message: string }> {
  return ((await response.json()) as { error: { presentation: string; rawReason: string | null; correlationId: string | null; message: string } }).error;
}

describe('the local checks, in order', () => {
  it('answers 403 when Origin is absent or foreign, and never calls the gateway', async () => {
    const { route, completions } = setup(streamOk());
    for (const origin of [undefined, 'https://evil.test']) {
      const request = new Request(`${ORIGIN}/gateway/api/chat`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', ...(origin === undefined ? {} : { origin }) },
        body: JSON.stringify(GOOD),
      });
      const response = await route(request);
      expect(response.status).toBe(403);
      expect((await errorOf(response)).presentation).toBe('forbidden');
    }
    expect(completions).not.toHaveBeenCalled();
  });

  it('answers 415 for a body that is not application/json', async () => {
    const { route } = setup(streamOk());
    const response = await route(post(JSON.stringify(GOOD), { 'content-type': 'text/plain' }));
    expect(response.status).toBe(415);
    expect((await errorOf(response)).rawReason).toBe('unsupported-content-type');
  });

  it('accepts application/json with parameters', async () => {
    const { route } = setup(streamOk());
    expect((await route(post(JSON.stringify(GOOD), { 'content-type': 'Application/JSON; charset=utf-8' }))).status).toBe(200);
  });

  it('answers 401 JSON, not a redirect, with no session', async () => {
    const { route, completions } = setup(streamOk(), { session: () => Promise.resolve(null) });
    const response = await route(post(JSON.stringify(GOOD)));
    expect(response.status).toBe(401);
    expect(response.headers.get('location')).toBeNull();
    const error = await errorOf(response);
    expect(error.presentation).toBe('relogin');
    expect(error.correlationId).toBe(CORRELATION);
    expect(completions).not.toHaveBeenCalled();
  });

  it('answers 413 for a declared content-length over the limit', async () => {
    const { route, completions } = setup(streamOk());
    const response = await route(post(JSON.stringify(GOOD), { 'content-length': '5000' }));
    expect(response.status).toBe(413);
    expect((await errorOf(response)).rawReason).toBe('request-too-large');
    expect(completions).not.toHaveBeenCalled();
  });

  // Review Focus 3.
  it('answers 413 for a chunked body over the limit with no content-length', async () => {
    const { route, completions } = setup(streamOk());
    const response = await route(post(streamOf(['x'.repeat(600), 'x'.repeat(600)])));
    expect(response.status).toBe(413);
    expect(completions).not.toHaveBeenCalled();
  });

  it('answers 400 invalid-request-body for a body that is not JSON', async () => {
    const { route } = setup(streamOk());
    const response = await route(post('{not json'));
    expect(response.status).toBe(400);
    expect((await errorOf(response)).rawReason).toBe('invalid-request-body');
  });

  it.each([
    ['org is not a UUID', { ...GOOD, org: 'acme' }],
    ['model is empty', { ...GOOD, model: '' }],
    ['messages is empty', { ...GOOD, messages: [] }],
    ['a role is not allowed', { ...GOOD, messages: [{ role: 'tool', content: 'x' }] }],
    ['content is not a string', { ...GOOD, messages: [{ role: 'user', content: 1 }] }],
    ['an extra top-level field', { ...GOOD, temperature: 2 }],
    ['an extra message field', { ...GOOD, messages: [{ role: 'user', content: 'x', name: 'n' }] }],
  ])('answers 400 invalid-request-schema when %s', async (_label, body) => {
    const { route, completions } = setup(streamOk());
    const response = await route(post(JSON.stringify(body)));
    expect(response.status).toBe(400);
    expect((await errorOf(response)).rawReason).toBe('invalid-request-schema');
    expect(completions).not.toHaveBeenCalled();
  });
});

describe('the gateway call', () => {
  it('sends only model, messages and stream: true, with the org, the correlation id, the signal, the bearer and 35 s', async () => {
    const { route, completions, chatClient } = setup(streamOk());
    const request = post(JSON.stringify(GOOD));
    await route(request);
    expect(chatClient).toHaveBeenCalledWith({ baseUrl: 'http://gateway.test', headerTimeoutMs: CHAT_HEADER_TIMEOUT_MS }, { bearer: 'session-token' });
    const [body, options] = completions.mock.calls[0] ?? [];
    expect(body).toEqual({ model: 'gpt-e2e', messages: [{ role: 'user', content: 'hi' }], stream: true });
    expect(options?.org).toBe(ORG);
    expect(options?.correlationId).toBe(CORRELATION);
    expect(options?.signal).toBe(request.signal);
  });

  it('streams with the SSE headers and relays each chunk unchanged', async () => {
    const { route } = setup(streamOk());
    const response = await route(post(JSON.stringify(GOOD)));
    expect(response.status).toBe(200);
    expect(response.headers.get('content-type')).toBe('text/event-stream');
    expect(response.headers.get('cache-control')).toBe('no-cache, no-transform');
    expect(response.headers.get('x-accel-buffering')).toBe('no');
    expect(response.headers.get('paigasus-correlation-id')).toBe(CORRELATION);
    expect(await response.text()).toBe('data: {"choices":[{"delta":{"content":"hi"}}]}\n\ndata: [DONE]\n\n');
  });

  it('injects one paigasus-error event after a chunk that holds the terminal frame', async () => {
    const { route } = setup({ kind: 'stream', body: streamOf(['data: {"choices":[{"delta":{"content":"par', TERMINAL]), correlationId: 'gw-corr', requestId: 'gw-req' });
    const text = await (await route(post(JSON.stringify(GOOD)))).text();
    expect(text.startsWith(`data: {"choices":[{"delta":{"content":"par${TERMINAL}`)).toBe(true);
    const match = /\n\nevent: paigasus-error\ndata: (.+)\n\n$/.exec(text);
    expect(match).not.toBeNull();
    const event = JSON.parse(match?.[1] ?? '{}') as { rawReason: string; correlationId: string };
    expect(event.rawReason).toBe('upstream-error');
    expect(event.correlationId).toBe('gw-corr');
  });

  it('injects a network paigasus-error event when the gateway stream itself fails, then closes', async () => {
    vi.spyOn(logger, 'appEvent').mockImplementation(() => undefined);
    const { route } = setup({ kind: 'stream', body: streamOf(['data: {"choices":[{"delta":{"content":"a"}}]}\n\n'], { error: true }), correlationId: null, requestId: null });
    const text = await (await route(post(JSON.stringify(GOOD)))).text();
    const match = /\n\nevent: paigasus-error\ndata: (.+)\n\n$/.exec(text);
    const event = JSON.parse(match?.[1] ?? '{}') as { transport: { kind: string; cause: string }; correlationId: string };
    expect(event.transport).toEqual({ kind: 'transport', cause: 'network' });
    expect(event.correlationId).toBe(CORRELATION);
  });

  // Review Focus 4.
  it('a cancel of the response body cancels the gateway stream', async () => {
    let cancelled = false;
    const { route } = setup({
      kind: 'stream',
      body: streamOf(['data: {"choices":[{"delta":{"content":"a"}}]}\n\n', 'more'], { onCancel: () => (cancelled = true) }),
      correlationId: null,
      requestId: null,
    });
    const response = await route(post(JSON.stringify(GOOD)));
    const reader = response.body?.getReader();
    await reader?.read();
    await reader?.cancel();
    expect(cancelled).toBe(true);
  });

  it.each([
    [
      'an HTTP error keeps its status',
      mapError({ kind: 'http', status: 403, headers: new Headers(), body: { error: { message: 'no', type: 'invalid_request_error', param: null, code: 'insufficient-permissions' } } }),
      403,
    ],
    ['a timeout is 504', mapError({ kind: 'transport', cause: 'timeout', message: 'chat header timeout' }), 504],
    ['a network failure is 502', mapError({ kind: 'transport', cause: 'network', message: 'fetch failed' }), 502],
  ])('the error arm: %s', async (_label, error, status) => {
    const { route } = setup({ kind: 'error', error });
    const response = await route(post(JSON.stringify(GOOD)));
    expect(response.status).toBe(status);
    expect((await errorOf(response)).correlationId).toBe(CORRELATION);
  });

  it('replaces the upstream text of a body that is not a Paigasus envelope, and keeps the correlation id', async () => {
    const error = mapError({
      kind: 'http',
      status: 401,
      headers: new Headers({ 'paigasus-correlation-id': 'gw-corr' }),
      body: { error: { message: 'Incorrect API key provided: sk-proj-****abcd', type: 'invalid_request_error', param: null, code: 'invalid_api_key' } },
    });
    const { route } = setup({ kind: 'error', error });
    const response = await route(post(JSON.stringify(GOOD)));
    const text = await response.text();
    expect(response.status).toBe(401);
    expect(text).not.toContain('sk-proj');
    const parsed = JSON.parse(text) as { error: { message: string; correlationId: string } };
    expect(parsed.error.message).toBe(UPSTREAM_REJECTED_MESSAGE);
    expect(parsed.error.correlationId).toBe('gw-corr');
  });

  it('answers 502 for the json arm, which a stream: true request never expects', async () => {
    const { route } = setup({ kind: 'json', status: 200, body: {}, correlationId: null, requestId: null });
    expect((await route(post(JSON.stringify(GOOD)))).status).toBe(502);
  });
});
