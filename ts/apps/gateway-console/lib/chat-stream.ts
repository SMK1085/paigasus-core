// SPDX-License-Identifier: Apache-2.0
//
// The browser's reader of the playground's SSE body (SMA-635 spec § 6.3). PURE: no DOM, no
// 'server-only' and no @paigasus/* import, so the client component and a node vitest both run it.
// It never touches a token: the route handler holds the session, the browser holds only this
// stream.
//
// The rules: a `data:` record with `choices` yields its `choices[0].delta.content`; a `data:`
// record WITHOUT `choices` is ignored (this includes the gateway's raw `{"error": …}` frame, for
// which the route handler injects a `paigasus-error` event); `data: [DONE]` ends the turn; an
// `event: paigasus-error` record is an error; an end with neither is an error; a comment line is
// ignored. The pending record is bounded the way @paigasus/sdk's createTerminalFrameParser bounds
// its own, and a partial multi-byte character survives a chunk boundary (a streaming decoder).

export type ChatStreamError = { readonly message: string; readonly correlationId: string | null; readonly reason: string | null };

export type ChatStreamEvent = { readonly kind: 'delta'; readonly text: string } | { readonly kind: 'done' } | { readonly kind: 'error'; readonly error: ChatStreamError };

export type ChatStreamParser = { push(chunk: Uint8Array): ChatStreamEvent[]; end(): ChatStreamEvent[] };

/** 64 KiB, the same bound as @paigasus/sdk's terminal-frame parser (chat.ts MAX_PENDING_RECORD). */
export const MAX_PENDING_RECORD = 64 * 1024;
export const INCOMPLETE_STREAM_MESSAGE = 'The answer stopped before it was complete.';
export const GENERIC_STREAM_ERROR = 'The answer failed.';

/** A blank line ends a record; the grammar's line terminator is CRLF, LF or CR. */
const RECORD_DELIMITER = /(?:\r\n|\r|\n){2}/;

function errorOf(payload: string): ChatStreamError {
  let body: unknown;
  try {
    body = JSON.parse(payload);
  } catch {
    return { message: GENERIC_STREAM_ERROR, correlationId: null, reason: null };
  }
  if (typeof body !== 'object' || body === null) return { message: GENERIC_STREAM_ERROR, correlationId: null, reason: null };
  const fields = body as { message?: unknown; correlationId?: unknown; rawReason?: unknown };
  return {
    message: typeof fields.message === 'string' && fields.message !== '' ? fields.message : GENERIC_STREAM_ERROR,
    correlationId: typeof fields.correlationId === 'string' ? fields.correlationId : null,
    reason: typeof fields.rawReason === 'string' ? fields.rawReason : null,
  };
}

function contentOf(payload: string): string | null {
  let body: unknown;
  try {
    body = JSON.parse(payload);
  } catch {
    return null;
  }
  if (typeof body !== 'object' || body === null) return null;
  const choices = (body as { choices?: unknown }).choices;
  if (!Array.isArray(choices)) return null;
  const first: unknown = choices[0];
  const delta = typeof first === 'object' && first !== null ? (first as { delta?: unknown }).delta : undefined;
  const content = typeof delta === 'object' && delta !== null ? (delta as { content?: unknown }).content : undefined;
  // Empty strings are dropped on purpose: appending '' changes nothing and wastes space.
  return typeof content === 'string' && content !== '' ? content : null;
}

function eventOf(record: string): ChatStreamEvent | null {
  let name = 'message';
  const data: string[] = [];
  for (const line of record.split(/\r\n|\r|\n/)) {
    if (line === '' || line.startsWith(':')) continue;
    const colon = line.indexOf(':');
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? '' : line.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    if (field === 'event') name = value;
    else if (field === 'data') data.push(value);
  }
  if (data.length === 0) return null;
  const payload = data.join('\n');
  if (name === 'paigasus-error') return { kind: 'error', error: errorOf(payload) };
  if (payload === '[DONE]') return { kind: 'done' };
  const text = contentOf(payload);
  return text === null ? null : { kind: 'delta', text };
}

export function createChatStreamParser(): ChatStreamParser {
  const decoder = new TextDecoder('utf-8');
  let buffer = '';
  let resynchronising = false;
  let finished = false;

  function drain(): ChatStreamEvent[] {
    const out: ChatStreamEvent[] = [];
    while (!finished) {
      const match = RECORD_DELIMITER.exec(buffer);
      if (match === null) break;
      const record = buffer.slice(0, match.index);
      buffer = buffer.slice(match.index + match[0].length);
      if (resynchronising) {
        resynchronising = false;
        continue;
      }
      const event = eventOf(record);
      if (event === null) continue;
      out.push(event);
      if (event.kind !== 'delta') finished = true;
    }
    if (finished) buffer = '';
    if (buffer.length > MAX_PENDING_RECORD) {
      buffer = '';
      resynchronising = true;
    }
    return out;
  }

  return {
    push(chunk) {
      if (finished) return [];
      buffer += decoder.decode(chunk, { stream: true });
      return drain();
    },
    end() {
      if (finished) return [];
      // Flush the decoder, and close a last record the producer left without a blank line.
      buffer += `${decoder.decode()}\n\n`;
      const out = drain();
      if (finished) return out;
      finished = true;
      return [...out, { kind: 'error', error: { message: INCOMPLETE_STREAM_MESSAGE, correlationId: null, reason: null } }];
    },
  };
}
