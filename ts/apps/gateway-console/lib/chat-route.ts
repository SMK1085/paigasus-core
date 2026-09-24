// SPDX-License-Identifier: Apache-2.0
//
// The playground's route handler, as a FACTORY (SMA-635 spec § 6.2). app/api/chat/route.ts only
// connects the real dependencies; the unit tests call this factory with doubles.
//
// Order: Origin (403), content type (415), session (401), body (413 / 400), then the gateway call.
// With the Origin check, the media-type check and the SameSite=Lax session cookie, a cross-site
// form post has three separate controls against it. Every local failure is `{ "error":
// <PaigasusError> }` with the request's correlation id.
//
// The stream is relayed UNCHANGED and at once, chunk by chunk. The gateway's terminal frame and a
// failure of the gateway stream itself each produce ONE injected `event: paigasus-error` record,
// with the fixed text STREAM_FAILED_MESSAGE; its leading blank line closes any partial record. A
// pull-based ReadableStream is used, not a
// TransformStream: a TransformStream is errored together with its source, so it could not inject
// an event after a source error. Its cancel() cancels the SDK body, so a browser cancel reaches
// the gateway even when request.signal does not fire (plan Task 1, M2).
import 'server-only';
import { z } from 'zod';
import { createTerminalFrameParser, type ChatClient } from '@paigasus/sdk/chat';
import { ErrorReason, mapError, type PaigasusError } from '@paigasus/sdk/errors';
import { isUuid, neverReachedIam, sessionExpired } from '@paigasus/console-core';

/** Longer than the gateway's 30 s first-byte wait (config.rs:188) plus its three IAM RPCs. */
export const CHAT_HEADER_TIMEOUT_MS = 35_000;
/** The gateway's default `max_request_bytes` (config.rs:190). The gateway enforces its own limit too. */
export const GATEWAY_DEFAULT_MAX_REQUEST_BYTES = 1_048_576;
/** Replaces an upstream message that is not a Paigasus envelope: an OpenAI 401 holds a masked piece of the gateway's key. Used ONLY for the error-arm scrub. */
export const UPSTREAM_REJECTED_MESSAGE = 'The model provider rejected the request.';
/** Used for every injected `paigasus-error` event on an already-started stream: the gateway's own terminal frame and a source-stream failure alike. Never the upstream's text. */
export const STREAM_FAILED_MESSAGE = 'The answer stream failed.';

const CORRELATION_HEADER = 'paigasus-correlation-id';

export type ChatRouteDeps = {
  readonly publicOrigin: () => Promise<string>;
  readonly session: () => Promise<{ readonly accessToken: string } | null>;
  readonly gatewayBaseUrl: () => string | null;
  readonly correlationId: () => Promise<string | null>;
  readonly chatClient: (options: { readonly baseUrl: string; readonly headerTimeoutMs: number }, auth: { readonly bearer: string }) => ChatClient;
  readonly maxBodyBytes: number;
};

const ChatBody = z.strictObject({
  org: z.string().refine(isUuid),
  model: z.string().min(1),
  messages: z.array(z.strictObject({ role: z.enum(['system', 'user', 'assistant']), content: z.string() })).min(1),
});

function localError(
  status: number,
  presentation: PaigasusError['presentation'],
  message: string,
  correlationId: string | null,
  reason: ErrorReason | null = null,
  rawReason: string | null = null,
): PaigasusError {
  return { ...neverReachedIam({ presentation, message, transport: { kind: 'http', status } }), reason, rawReason, correlationId };
}

function errorResponse(status: number, error: PaigasusError, correlationId: string | null): Response {
  const headers = new Headers({ 'cache-control': 'no-store' });
  if (correlationId !== null) headers.set(CORRELATION_HEADER, correlationId);
  return Response.json({ error }, { status, headers });
}

function originOf(value: string | null): string | null {
  if (value === null || !URL.canParse(value)) return null;
  return new URL(value).origin;
}

function mediaTypeOf(request: Request): string {
  return (request.headers.get('content-type') ?? '').split(';')[0]?.trim().toLowerCase() ?? '';
}

type BodyRead = { readonly ok: true; readonly text: string } | { readonly ok: false; readonly tooLarge: boolean };

/** Reads at most `max` bytes. A declared or a counted size over it is 413; a read failure is not. */
async function readBounded(request: Request, max: number): Promise<BodyRead> {
  const declared = request.headers.get('content-length');
  if (declared !== null && Number(declared) > max) return { ok: false, tooLarge: true };
  if (request.body === null) return { ok: true, text: '' };
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > max) {
        await reader.cancel();
        return { ok: false, tooLarge: true };
      }
      chunks.push(value);
    }
  } catch {
    return { ok: false, tooLarge: false };
  }
  const all = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    all.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return { ok: true, text: new TextDecoder().decode(all) };
}

/** An HTTP error whose body was not a Paigasus envelope keeps its id but loses the upstream text. */
function scrub(error: PaigasusError): PaigasusError {
  return error.transport.kind === 'http' && error.reason === null ? { ...error, message: UPSTREAM_REJECTED_MESSAGE } : error;
}

function withCorrelation(error: PaigasusError, correlationId: string | null): PaigasusError {
  return error.correlationId === null ? { ...error, correlationId } : error;
}

function statusOf(error: PaigasusError): number {
  if (error.transport.kind === 'http') return error.transport.status;
  if (error.transport.kind === 'transport' && error.transport.cause === 'timeout') return 504;
  return 502;
}

function relay(source: ReadableStream<Uint8Array>, ids: { readonly correlationId: string | null; readonly requestId: string | null }): ReadableStream<Uint8Array> {
  const reader = source.getReader();
  const parser = createTerminalFrameParser(200, ids);
  const encoder = new TextEncoder();
  // ALWAYS the generic STREAM_FAILED_MESSAGE, never `scrub`'s conditional one and never the
  // upstream's own text: the upstream controls every byte of a forwarded frame, including
  // `code: "upstream-error"` paired with any `message` it likes (fix round 1, item 6).
  // `upstream-error` is itself a REGISTERED reason (ERROR_REASON_UPSTREAM_ERROR = 307), so
  // `scrub`'s `reason === null` guard does not fire for it and would otherwise let an
  // upstream-chosen string (a leaked key fragment, for example) straight into the injected event.
  const event = (error: PaigasusError): Uint8Array =>
    encoder.encode(`\n\nevent: paigasus-error\ndata: ${JSON.stringify(withCorrelation({ ...error, message: STREAM_FAILED_MESSAGE }, ids.correlationId))}\n\n`);
  let finished = false;
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      if (finished) return;
      let read: ReadableStreamReadResult<Uint8Array>;
      try {
        read = await reader.read();
      } catch {
        if (finished) return;
        finished = true;
        controller.enqueue(event(mapError({ kind: 'transport', cause: 'network', message: 'the gateway stream failed' })));
        controller.close();
        return;
      }
      // The consumer's cancel() can land while the read above is in flight. Without this check, a
      // stream already cancelled (finished = true, its controller no longer accepting input) still
      // has close() called on it below (fix round 1, item 8).
      if (finished) return;
      if (read.done) {
        finished = true;
        controller.close();
        return;
      }
      controller.enqueue(read.value);
      for (const error of parser.push(read.value)) controller.enqueue(event(error));
    },
    cancel(reason) {
      finished = true;
      return reader.cancel(reason);
    },
  });
}

export function createChatRoute(deps: ChatRouteDeps): (request: Request) => Promise<Response> {
  return async function chatRoute(request: Request): Promise<Response> {
    const correlationId = await deps.correlationId();
    const fail = (status: number, error: PaigasusError): Response => errorResponse(status, error, correlationId);

    const expected = originOf(await deps.publicOrigin());
    const origin = originOf(request.headers.get('origin'));
    if (origin === null || origin !== expected) return fail(403, localError(403, 'forbidden', 'The request did not come from this console.', correlationId));

    if (mediaTypeOf(request) !== 'application/json') {
      return fail(415, localError(415, 'invalid-input', 'The request body must be application/json.', correlationId, ErrorReason.UNSUPPORTED_CONTENT_TYPE, 'unsupported-content-type'));
    }

    const session = await deps.session();
    if (session === null) return fail(401, { ...sessionExpired(), correlationId });

    const read = await readBounded(request, deps.maxBodyBytes);
    if (!read.ok) {
      return read.tooLarge
        ? fail(413, localError(413, 'invalid-input', 'The request body is too large.', correlationId, ErrorReason.REQUEST_TOO_LARGE, 'request-too-large'))
        : fail(400, localError(400, 'invalid-input', 'The request body could not be read.', correlationId, ErrorReason.INVALID_REQUEST_BODY, 'invalid-request-body'));
    }
    let json: unknown;
    try {
      json = JSON.parse(read.text);
    } catch {
      return fail(400, localError(400, 'invalid-input', 'The request body is not JSON.', correlationId, ErrorReason.INVALID_REQUEST_BODY, 'invalid-request-body'));
    }
    const parsed = ChatBody.safeParse(json);
    if (!parsed.success) return fail(400, localError(400, 'invalid-input', 'The chat request has the wrong shape.', correlationId, ErrorReason.INVALID_REQUEST_SCHEMA, 'invalid-request-schema'));

    const baseUrl = deps.gatewayBaseUrl();
    if (baseUrl === null) return fail(502, localError(502, 'degraded', 'The gateway is not configured.', correlationId));

    const { org, model, messages } = parsed.data;
    const client = deps.chatClient({ baseUrl, headerTimeoutMs: CHAT_HEADER_TIMEOUT_MS }, { bearer: session.accessToken });
    // The gateway request is built HERE: no other client field is forwarded.
    const result = await client.completions({ model, messages, stream: true }, correlationId === null ? { signal: request.signal, org } : { signal: request.signal, org, correlationId });

    switch (result.kind) {
      case 'error':
        return fail(statusOf(result.error), withCorrelation(scrub(result.error), correlationId));
      case 'json':
        return fail(502, localError(502, 'degraded', 'The gateway answered without a stream.', correlationId));
      case 'stream': {
        const headers = new Headers({ 'content-type': 'text/event-stream', 'cache-control': 'no-cache, no-transform', 'x-accel-buffering': 'no' });
        if (correlationId !== null) headers.set(CORRELATION_HEADER, correlationId);
        return new Response(relay(result.body, { correlationId: result.correlationId ?? correlationId, requestId: result.requestId }), { status: 200, headers });
      }
    }
  };
}
