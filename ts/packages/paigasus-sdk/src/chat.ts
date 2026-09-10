// SPDX-License-Identifier: Apache-2.0
import './server-guard.js';

import { mapError } from './errors/map-error.js';
import type { PaigasusError } from './errors/types.js';
// TYPE-ONLY, deliberately. `transport.ts` imports @connectrpc/connect-node at module scope; a
// value import here would drag the whole HTTP/2 stack into every `./chat` consumer, defeating the
// reason § 6.1 gives for having subpaths at all. Under verbatimModuleSyntax this emits nothing.
import type { Auth } from './transport.js';

const CORRELATION_ID_HEADER = 'paigasus-correlation-id';
const REQUEST_ID_HEADER = 'paigasus-request-id';

/** Matches the gRPC transport's default (spec § 8.4), so the two surfaces agree. */
export const DEFAULT_CHAT_TIMEOUT_MS = 10_000;

/**
 * The gateway requires `model` and `messages` (dto.rs:22-34); a body missing either is rendered as
 * a 400 `invalid-request-schema`. Requiring them here turns a round-trip into a compile error.
 *
 * `messages` is `unknown[]` on purpose: the gateway does not model message content, and pinning
 * OpenAI's message union here would make every upstream addition a breaking change in this
 * package. The index signature mirrors the gateway's `#[serde(flatten)]` passthrough.
 */
export interface ChatCompletionRequest {
  model: string;
  messages: unknown[];
  stream?: boolean;
  [key: string]: unknown;
}

export interface ChatOptions {
  readonly baseUrl: string;
  readonly auth: Auth;
  /** Bounds the wait for the RESPONSE HEAD only. Defaults to `DEFAULT_CHAT_TIMEOUT_MS`. */
  readonly timeoutMs?: number;
  readonly signal?: AbortSignal;
}

interface ChatResultBase {
  readonly status: number;
  readonly correlationId: string | null;
  readonly requestId: string | null;
}

/**
 * What `chatCompletion` throws. It CARRIES the mapped error rather than being it.
 *
 * `PaigasusError` is a plain data object by design (spec § 6.3) so it can cross a server/client
 * prop boundary. A thrown value needs the opposite: a stack, and `instanceof Error`, which every
 * logger and error boundary tests for. Carrying rather than replacing gets both — a caller renders
 * `err.error`, and a logger still sees a real Error.
 */
export class PaigasusHttpError extends Error {
  constructor(readonly error: PaigasusError) {
    super(error.message);
    this.name = 'PaigasusHttpError';
  }
}

export type ChatCompletionResult = (ChatResultBase & { readonly kind: 'json'; readonly body: unknown }) | (ChatResultBase & { readonly kind: 'stream'; readonly body: ReadableStream<Uint8Array> });

/** A body may be labelled application/json and not be JSON — the gateway forces the header. */
async function readBody(response: Response): Promise<unknown> {
  const text = await response.text();
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}

/**
 * `POST /v1/chat/completions` on the gateway.
 *
 * Returns on 2xx and THROWS a `PaigasusError` otherwise — one rule, not two. A third
 * `{ kind: 'error' }` variant would let a caller ignore a failure by not checking a discriminant.
 */
export async function chatCompletion(request: ChatCompletionRequest, options: ChatOptions): Promise<ChatCompletionResult> {
  const headers = new Headers({ 'content-type': 'application/json' });
  if ('bearer' in options.auth) headers.set('authorization', `Bearer ${options.auth.bearer}`);

  // A pre-header deadline, merged with the caller's signal. The timer is cleared the moment the
  // head lands: one signal held past that point would abort the BODY too, which is why a single
  // AbortSignal cannot serve as both a header deadline and a live-stream lifeline (spec § 8.4).
  const deadline = new AbortController();
  const timer = setTimeout(() => {
    deadline.abort(new DOMException('the response head did not arrive before the deadline', 'TimeoutError'));
  }, options.timeoutMs ?? DEFAULT_CHAT_TIMEOUT_MS);
  const signal = options.signal === undefined ? deadline.signal : AbortSignal.any([options.signal, deadline.signal]);

  let response: Response;
  try {
    response = await fetch(`${options.baseUrl}/v1/chat/completions`, { method: 'POST', headers, body: JSON.stringify(request), signal });
  } catch (cause) {
    // A transport failure or an expired deadline. 504 is the honest status: nothing came back.
    throw new PaigasusHttpError(
      mapError({ kind: 'gateway-http', status: 504, headers: new Headers(), body: { error: { message: cause instanceof Error ? cause.message : 'the chat request failed' } } }),
    );
  } finally {
    clearTimeout(timer);
  }

  const correlationId = response.headers.get(CORRELATION_ID_HEADER);
  const requestId = response.headers.get(REQUEST_ID_HEADER);

  if (!response.ok) {
    throw new PaigasusHttpError(mapError({ kind: 'gateway-http', status: response.status, headers: response.headers, body: await readBody(response) }));
  }

  // Branch on what the gateway ACTUALLY sent, never on `request.stream`: a `stream:true` request
  // whose upstream answered non-2xx comes back as JSON (chat.rs:138-141).
  const isStream = response.headers.get('content-type')?.includes('text/event-stream') === true;

  if (isStream && response.body !== null) {
    // AC 4: the identical object. No pipeThrough, no reader, no decode. Cancellation propagates
    // to the upstream connection because this IS the upstream body.
    return { kind: 'stream', status: response.status, body: response.body, correlationId, requestId };
  }

  return { kind: 'json', status: response.status, body: await readBody(response), correlationId, requestId };
}

// Re-exported from `./chat` rather than `./errors`, for two reasons. It would be a CYCLE on
// `./errors` — terminal-frame.ts already imports map-error.ts, so map-error.ts re-exporting it
// closes the loop. And a caller reaches for this while consuming a chat stream, which is what
// spec § 8.5 describes it as being for.
export { createTerminalFrameParser } from './errors/terminal-frame.js';
export type { FrameIds } from './errors/map-error.js';
