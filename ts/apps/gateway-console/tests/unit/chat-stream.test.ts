// SPDX-License-Identifier: Apache-2.0
//
// The browser's reader of the playground stream (SMA-635 spec § 6.3, § 7.1).
import { describe, expect, it } from 'vitest';
import { GENERIC_STREAM_ERROR, INCOMPLETE_STREAM_MESSAGE, MAX_PENDING_RECORD, createChatStreamParser, type ChatStreamEvent } from '../../lib/chat-stream';

const encode = (s: string): Uint8Array => new TextEncoder().encode(s);
const delta = (text: string): string => `data: ${JSON.stringify({ choices: [{ index: 0, delta: { content: text } }] })}\n\n`;

function all(chunks: readonly (string | Uint8Array)[], end = true): ChatStreamEvent[] {
  const parser = createChatStreamParser();
  const out: ChatStreamEvent[] = [];
  for (const chunk of chunks) out.push(...parser.push(typeof chunk === 'string' ? encode(chunk) : chunk));
  if (end) out.push(...parser.end());
  return out;
}

describe('createChatStreamParser', () => {
  it('appends choices[0].delta.content and ends at [DONE]', () => {
    expect(all([delta('Hel'), delta('lo'), 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'Hel' }, { kind: 'delta', text: 'lo' }, { kind: 'done' }]);
  });

  it('keeps a record split over two chunks', () => {
    const record = delta('split');
    expect(all([record.slice(0, 10), record.slice(10), 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'split' }, { kind: 'done' }]);
  });

  it('keeps a multi-byte character split over two chunks', () => {
    const bytes = encode(delta('é€'));
    const cut = bytes.indexOf(0xe2) + 1; // inside the three bytes of '€'
    expect(all([bytes.slice(0, cut), bytes.slice(cut), 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'é€' }, { kind: 'done' }]);
  });

  it('shows a paigasus-error event as an error and stops', () => {
    const error = { message: 'upstream stream error', correlationId: 'c-1', rawReason: 'upstream-error' };
    const out = all([delta('part'), `\n\nevent: paigasus-error\ndata: ${JSON.stringify(error)}\n\n`, delta('after')]);
    expect(out).toEqual([
      { kind: 'delta', text: 'part' },
      { kind: 'error', error: { message: 'upstream stream error', correlationId: 'c-1', reason: 'upstream-error' } },
    ]);
  });

  // The route handler injects a paigasus-error event for this frame, so the raw frame is ignored.
  it('ignores the gateway raw {"error": …} record', () => {
    const raw = '\n\ndata: {"error":{"message":"upstream stream error","type":"api_error","param":null,"code":"upstream-error"}}\n\n';
    expect(all([delta('a'), raw, 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'a' }, { kind: 'done' }]);
  });

  it('reports an end with no [DONE] and no paigasus-error as an error', () => {
    expect(all([delta('cut')])).toEqual([
      { kind: 'delta', text: 'cut' },
      { kind: 'error', error: { message: INCOMPLETE_STREAM_MESSAGE, correlationId: null, reason: null } },
    ]);
  });

  it('ignores a comment line', () => {
    expect(all([': keep-alive\n\n', delta('x'), 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'x' }, { kind: 'done' }]);
  });

  it('bounds the pending record and resynchronises at the next boundary', () => {
    const huge = `data: ${'x'.repeat(MAX_PENDING_RECORD + 10)}`;
    expect(all([huge, 'tail-of-the-huge-record\n\n', delta('next'), 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'next' }, { kind: 'done' }]);
  });

  it('uses a generic message when a paigasus-error event carries no usable JSON', () => {
    expect(all(['event: paigasus-error\ndata: not-json\n\n'])).toEqual([{ kind: 'error', error: { message: GENERIC_STREAM_ERROR, correlationId: null, reason: null } }]);
  });

  it('accepts CRLF record delimiters', () => {
    expect(all([delta('crlf').replace(/\n\n$/, '\r\n\r\n'), 'data: [DONE]\r\n\r\n'])).toEqual([{ kind: 'delta', text: 'crlf' }, { kind: 'done' }]);
  });
});
