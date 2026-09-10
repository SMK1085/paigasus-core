// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { ErrorReason } from '@paigasus/proto';
import { describe, expect, it } from 'vitest';

import { createTerminalFrameParser } from '../src/chat.js';

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');
const CHAT_RS = resolve(REPO_ROOT, 'rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs');

/**
 * Read TERMINAL_SSE_ERROR out of the gateway's Rust source, BY CONSTANT NAME.
 *
 * Keyed on the name and never on a line number: any edit above the constant moves it, and a
 * line-keyed fixture would then compare the wrong line and could pass while the frame changed.
 *
 * `chat.rs` is declared in paigasus-sdk-ts:test's `inputs` (moon.yml) so that editing the constant
 * SELECTS this suite. Without that declaration Moon serves a cached PASS on exactly the PR that
 * changes the frame (spec § 8.4, § 11.2 obligation 9).
 */
function terminalFrameFromRust(): string {
  const src = readFileSync(CHAT_RS, 'utf8');
  const match = /const TERMINAL_SSE_ERROR: &str = "((?:[^"\\]|\\.)*)";/.exec(src);
  if (match === null) throw new Error(`TERMINAL_SSE_ERROR not found in ${CHAT_RS} — the gateway renamed or removed it`);
  // The capture group is not optional in the pattern above, so a non-null match always carries
  // it; `noUncheckedIndexedAccess` (ts/tsconfig.base.json) still types index 1 as possibly
  // undefined, which the assertion below resolves.
  const literal = match[1];
  if (literal === undefined) throw new Error('unreachable: capture group 1 always matches alongside the pattern above');
  // Decode the Rust string literal's escapes. Backslash LAST would double-process the others.
  return literal.replace(/\\(.)/g, (_all, ch: string) => (ch === 'n' ? '\n' : ch === 't' ? '\t' : ch === 'r' ? '\r' : ch));
}

const FRAME = terminalFrameFromRust();
const encode = (s: string): Uint8Array => new TextEncoder().encode(s);

describe('the terminal frame is read from the gateway, not hand-built', () => {
  // Without this the test would assert against an object it wrote itself, and could not fail
  // when the gateway changes the frame — which is the one drift it exists to absorb.
  it('is a well-formed SSE data record carrying the registry code', () => {
    expect(FRAME.startsWith('data: ')).toBe(true);
    expect(FRAME.endsWith('\n\n')).toBe(true);
    expect(FRAME).toContain('"code":"upstream-error"');
  });
});

describe('createTerminalFrameParser', () => {
  it('parses one whole frame in one chunk', () => {
    const parser = createTerminalFrameParser(200);
    const out = parser.push(encode(FRAME));
    expect(out).toHaveLength(1);
    // `noUncheckedIndexedAccess` types `out[0]` as possibly undefined; the length assertion above
    // already proves an element is there.
    const [frame] = out;
    if (frame === undefined) throw new Error('unreachable: toHaveLength(1) above');
    expect(frame.reason).toBe(ErrorReason.UPSTREAM_ERROR);
    expect(frame.rawReason).toBe('upstream-error');
    // Status 200: the head was already committed. The `degraded` presentation comes from the
    // OVERRIDE table, because the HTTP table has no 200 row (spec § 9.4).
    expect(frame.presentation).toBe('degraded');
    expect(frame.transport).toEqual({ kind: 'http', status: 200 });
    // chat.rs:56-62 — the frame deliberately carries no retryable signal.
    expect(frame.retryable).toBeNull();
  });

  // THE case the parser exists for. A chunk boundary can fall anywhere, and a parser over one
  // bare chunk misses the terminal error precisely here.
  it('parses a frame split across two chunks', () => {
    const parser = createTerminalFrameParser(200);
    const split = Math.floor(FRAME.length / 2);
    expect(parser.push(encode(FRAME.slice(0, split)))).toEqual([]);
    const out = parser.push(encode(FRAME.slice(split)));
    expect(out).toHaveLength(1);
    const [frame] = out;
    if (frame === undefined) throw new Error('unreachable: toHaveLength(1) above');
    expect(frame.reason).toBe(ErrorReason.UPSTREAM_ERROR);
  });

  it('returns both frames when two arrive in one chunk', () => {
    const parser = createTerminalFrameParser(200);
    const out = parser.push(encode(FRAME + FRAME));
    expect(out).toHaveLength(2);
  });

  it('holds a partial trailing record and returns nothing', () => {
    const parser = createTerminalFrameParser(200);
    expect(parser.push(encode('data: {"error":{"code":"upstream-error"'))).toEqual([]);
  });

  it('ignores ordinary data records', () => {
    const parser = createTerminalFrameParser(200);
    expect(parser.push(encode('data: {"choices":[{"delta":{"content":"hi"}}]}\n\n'))).toEqual([]);
    expect(parser.push(encode('data: [DONE]\n\n'))).toEqual([]);
  });

  // A per-chunk TextDecoder().decode() splits a multi-byte character on the same boundary the
  // record parser has to survive, so the parser owns a streaming decoder.
  //
  // The split character must sit INSIDE the terminal frame's own `message`, not in an ignored
  // content record before it: MEASURED, a split in an ignored record still parses as valid JSON
  // after a non-streaming decode replaces the two half-bytes with U+FFFD, so that shape passes
  // identically with `{ stream: true }` and without it and proves nothing. This synthetic
  // terminal frame is built directly, rather than reusing `FRAME`, because the discriminating
  // property is the split landing inside the reported `message`. MEASURED discriminating:
  // streaming decode yields "upstream stréam error"; a non-streaming decode yields
  // "upstream str<0xFFFD><0xFFFD>am error".
  it('survives a multi-byte character split across a chunk boundary, inside the reported message', () => {
    const message = 'upstream stréam error';
    const frameText = `data: {"error":{"message":"${message}","type":"api_error","param":null,"code":"upstream-error"}}\n\n`;
    const bytes = encode(frameText);
    // 'é' encodes to the two UTF-8 bytes 0xC3 0xA9. Cut between them so the decoder must carry
    // the leading byte across the `push` boundary.
    const i = bytes.indexOf(0xc3);
    if (i === -1) throw new Error('unreachable: the fixture always contains the multi-byte character');
    const parser = createTerminalFrameParser(200);
    expect(parser.push(bytes.slice(0, i + 1))).toEqual([]);
    const out = parser.push(bytes.slice(i + 1));
    expect(out).toHaveLength(1);
    const [frame] = out;
    if (frame === undefined) throw new Error('unreachable: toHaveLength(1) above');
    expect(frame.message).toBe(message);
  });

  // Ported from PR #231's review: the committed head is not necessarily 200. The gateway forwards
  // the upstream's own success status, so a stream committed on 201 must report 201 — a hardcoded
  // 200 is a quietly wrong answer in a field a caller may log or branch on.
  it('reports the status the head actually committed, not a hardcoded 200', () => {
    const parser = createTerminalFrameParser(201);
    const out = parser.push(encode(FRAME));
    expect(out).toHaveLength(1);
    const frame = out[0];
    if (frame === undefined) throw new Error('unreachable');
    expect(frame.transport).toEqual({ kind: 'http', status: 201 });
    // The override table, not the status table, is still what makes this `degraded` — no 2xx has
    // a row of its own.
    expect(frame.presentation).toBe('degraded');
  });

  // The mid-stream failure is the ONE case whose frame carries no ids of its own. The head had
  // them, so the caller passes them back in rather than leaving a user with nothing to report.
  it('carries the committed head ids into the mapped error', () => {
    const parser = createTerminalFrameParser(200, { correlationId: 'corr-stream', requestId: 'req-stream' });
    const out = parser.push(encode(FRAME));
    const frame = out[0];
    if (frame === undefined) throw new Error('unreachable');
    expect(frame.correlationId).toBe('corr-stream');
    expect(frame.requestId).toBe('req-stream');
  });

  it('leaves the ids null when the head carried none', () => {
    const parser = createTerminalFrameParser(200);
    const frame = parser.push(encode(FRAME))[0];
    if (frame === undefined) throw new Error('unreachable');
    expect(frame.correlationId).toBeNull();
    expect(frame.requestId).toBeNull();
  });

  // The delimiter is any TWO consecutive line terminators, with CRLF consumed whole.
  //
  // BARE CR is the case that discriminates. The previous `/\r?\n\r?\n/` happened to match a mixed
  // `\n\r\n`, but it requires an LF in each half, so `\r\r` never matched and the terminal frame
  // was silently never completed. MEASURED: reverting RECORD_DELIMITER reds this row alone.
  it.each([
    ['CRLF', '\r\n\r\n'],
    ['bare CR', '\r\r'],
    ['mixed LF/CRLF', '\n\r\n'],
  ])('accepts a %s record delimiter', (_label, delimiter) => {
    const parser = createTerminalFrameParser(200);
    expect(parser.push(encode(FRAME.replace(/\n\n$/, delimiter)))).toHaveLength(1);
  });

  it('accepts CRLF record delimiters', () => {
    const parser = createTerminalFrameParser(200);
    const crlf = FRAME.replace(/\n\n$/, '\r\n\r\n');
    expect(parser.push(encode(crlf))).toHaveLength(1);
  });
});
