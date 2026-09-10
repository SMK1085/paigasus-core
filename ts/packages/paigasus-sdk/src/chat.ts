// SPDX-License-Identifier: Apache-2.0
//
// The `./chat` entry (spec § 8). GUARDED and at src/ root, like every other guarded entry.
import './server-guard.js';

import { mapError } from './errors/map-error.js';
import type { ErrorInput } from './errors/map-error.js';
import type { PaigasusError } from './errors/types.js';
import type { TransportCause } from './errors/types.js';

/** The registry code the gateway puts in its one terminal SSE frame (chat.rs:63). */
const TERMINAL_CODE = 'upstream-error';

/**
 * A stateful, incremental SSE parser that reports terminal error frames.
 *
 * The SDK NEVER drives this itself, because scanning the stream means buffering it, and buffering
 * would undo the unbuffered forwarding the gateway goes to trouble to preserve (chat.rs:194-220).
 * It is EXPORTED so a caller consuming its own stream can drive it (spec § 8.4).
 *
 * It holds only the trailing partial record and the trailing partial character, so it does not
 * reintroduce the buffering the passthrough avoids.
 */
export function createTerminalFrameParser(): { push(chunk: Uint8Array): PaigasusError[] } {
  // A streaming decoder, not a per-chunk one: a multi-byte character can straddle a chunk
  // boundary exactly as a record can.
  const decoder = new TextDecoder('utf-8');
  let buffer = '';

  return {
    push(chunk: Uint8Array): PaigasusError[] {
      buffer += decoder.decode(chunk, { stream: true });

      const found: PaigasusError[] = [];
      // An SSE record ends at a blank line. The gateway emits "\n\n" (chat.rs:63); "\r\n\r\n" is
      // legal SSE and is not produced here, but accepting it costs one alternation and rejecting
      // it would be a silent miss.
      const delimiter = /\r?\n\r?\n/;
      for (;;) {
        const match = delimiter.exec(buffer);
        if (match === null) break;
        const record = buffer.slice(0, match.index);
        buffer = buffer.slice(match.index + match[0].length);

        const error = terminalErrorFrom(record);
        if (error !== null) found.push(error);
      }
      return found;
    },
  };
}

/** A `PaigasusError` when this record is the terminal error frame, else `null`. */
function terminalErrorFrom(record: string): PaigasusError | null {
  const data = record
    .split(/\r?\n/)
    .filter((line) => line.startsWith('data:'))
    .map((line) => line.slice('data:'.length).trim())
    .join('');
  if (data === '' || data === '[DONE]') return null;

  let body: unknown;
  try {
    body = JSON.parse(data);
  } catch {
    // A stream carries ordinary content chunks too, and a caller may feed us anything. An
    // unparseable record is not an error to report; it is simply not the terminal frame.
    return null;
  }

  const code = (body as { error?: { code?: unknown } } | null)?.error?.code;
  if (code !== TERMINAL_CODE) return null;
  return mapError({ kind: 'terminal-frame', body });
}

/** The default bound on the wait for response HEADERS. Matches the gRPC transport's 10 s (§ 7.2). */
export const DEFAULT_HEADER_TIMEOUT_MS = 10_000;

export interface ChatClientOptions {
  readonly baseUrl: string;
  /**
   * Bounds the wait for response HEADERS only. After the head is committed no wall-clock timeout
   * applies: a long chat completion is a correct slow response, not a stalled one.
   */
  readonly headerTimeoutMs?: number;
  /**
   * The injection seam, read PER CALL. A module-load capture would make `vi.stubGlobal` useless
   * and would freeze whatever dispatcher a host had installed at import time (spec § 8.1).
   */
  readonly fetch?: typeof globalThis.fetch;
}

/**
 * The client never throws a mapped error. A caller branches on `kind`, and on `content-type`
 * rather than on its own `stream` flag — a `stream: true` request that fails before the head is
 * committed answers as plain JSON, not SSE (chat.rs:139-141).
 */
export type ChatResult =
  | { readonly kind: 'json'; readonly status: number; readonly body: unknown }
  | { readonly kind: 'stream'; readonly body: ReadableStream<Uint8Array> }
  | { readonly kind: 'error'; readonly error: PaigasusError };

export interface ChatClient {
  completions(request: unknown, options?: { readonly signal?: AbortSignal }): Promise<ChatResult>;
}

/**
 * The gateway's `POST /v1/chat/completions` — the one hand-written HTTP surface (spec § 4.1).
 *
 * `auth` is REQUIRED and is deliberately NOT the `Auth` union `./iam` uses. No path through the
 * gateway's `chat_completions` accepts an anonymous caller: `require_iam_auth` has already run
 * (chat.rs:5-6) and `MissingBearer` answers 401 (error.rs:122-128). An `{ anonymous: true }` arm
 * here would be a legal value with no legal use, so the narrower type makes it a compile error.
 *
 * This client uses `fetch` and builds no `Transport`, so it shares nothing with the transport
 * cache and § 7.1's cache-key rule does not reach it.
 */
export function createChatClient(options: ChatClientOptions, auth: { readonly bearer: string }): ChatClient {
  // Refused here, where the token is BOUND, rather than sent as `Authorization: Bearer `. A
  // caller reading an unset environment variable gets a clear local error naming the cause —
  // the same rule src/transport.ts applies on the gRPC side.
  if (auth.bearer.trim() === '') {
    throw new Error('@paigasus/sdk: refusing to send an empty or whitespace-only bearer token to the chat endpoint. This usually means an unset environment variable.');
  }

  const headerTimeoutMs = options.headerTimeoutMs ?? DEFAULT_HEADER_TIMEOUT_MS;
  const url = `${options.baseUrl.replace(/\/+$/, '')}/v1/chat/completions`;

  return {
    async completions(request, callOptions): Promise<ChatResult> {
      const fetchImpl = options.fetch ?? globalThis.fetch;
      const callerSignal = callOptions?.signal;

      // NOT AbortSignal.timeout. That signal keeps running after the headers arrive and aborts
      // the streaming body at the deadline, truncating every completion longer than the window —
      // the normal case for this product. A manual controller whose timer is cleared the moment
      // the fetch promise settles bounds the HEAD only (spec § 8.5).
      const controller = new AbortController();
      let timedOut = false;
      const timer = setTimeout(() => {
        timedOut = true;
        controller.abort(new DOMException('chat header timeout', 'TimeoutError'));
      }, headerTimeoutMs);

      let response: Response;
      try {
        response = await fetchImpl(url, {
          method: 'POST',
          headers: { authorization: `Bearer ${auth.bearer}`, 'content-type': 'application/json' },
          body: JSON.stringify(request),
          signal: callerSignal === undefined ? controller.signal : AbortSignal.any([controller.signal, callerSignal]),
        });
      } catch (cause) {
        return { kind: 'error', error: mapError(transportInput(cause, timedOut, callerSignal)) };
      } finally {
        // The BODY STREAM outlives this scope deliberately. Clearing the timer here is what stops
        // the pre-header deadline from ever reaching it.
        clearTimeout(timer);
      }

      if (!response.ok) {
        // The SDK maps it, so the caller needs no error knowledge of its own. The body may be
        // gateway-generated OR an upstream OpenAI envelope forwarded verbatim (chat.rs:113-119),
        // and it may not be JSON at all — mapError is total over both.
        return { kind: 'error', error: mapError({ kind: 'http', status: response.status, headers: response.headers, body: await readBody(response) }) };
      }

      const contentType = response.headers.get('content-type') ?? '';
      if (contentType.includes('text/event-stream')) {
        if (response.body === null) {
          return { kind: 'error', error: mapError({ kind: 'transport', cause: 'network', message: 'the gateway answered text/event-stream with no body' }) };
        }
        // AC 4: the IDENTICAL object. Not read, not buffered, not decoded, not re-encoded.
        // Cancelling it reaches the upstream connection because it IS the platform's stream.
        return { kind: 'stream', body: response.body };
      }

      return { kind: 'json', status: response.status, body: await readBody(response) };
    },
  };
}

/** Parse when we can, hand back the raw text when we cannot. `mapError` handles both. */
async function readBody(response: Response): Promise<unknown> {
  const text = await response.text();
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

function transportInput(cause: unknown, timedOut: boolean, callerSignal: AbortSignal | undefined): ErrorInput {
  const message = cause instanceof Error ? cause.message : String(cause);
  // Order matters: our own timer wins over the generic abort classification, because a timeout
  // ABORTS and would otherwise read as a caller abort.
  const kind: TransportCause = timedOut ? 'timeout' : callerSignal?.aborted === true ? 'aborted' : 'network';
  return { kind: 'transport', cause: kind, message };
}
