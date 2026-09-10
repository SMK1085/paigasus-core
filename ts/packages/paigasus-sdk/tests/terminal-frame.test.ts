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
    const parser = createTerminalFrameParser();
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
    const parser = createTerminalFrameParser();
    const split = Math.floor(FRAME.length / 2);
    expect(parser.push(encode(FRAME.slice(0, split)))).toEqual([]);
    const out = parser.push(encode(FRAME.slice(split)));
    expect(out).toHaveLength(1);
    const [frame] = out;
    if (frame === undefined) throw new Error('unreachable: toHaveLength(1) above');
    expect(frame.reason).toBe(ErrorReason.UPSTREAM_ERROR);
  });

  it('returns both frames when two arrive in one chunk', () => {
    const parser = createTerminalFrameParser();
    const out = parser.push(encode(FRAME + FRAME));
    expect(out).toHaveLength(2);
  });

  it('holds a partial trailing record and returns nothing', () => {
    const parser = createTerminalFrameParser();
    expect(parser.push(encode('data: {"error":{"code":"upstream-error"'))).toEqual([]);
  });

  it('ignores ordinary data records', () => {
    const parser = createTerminalFrameParser();
    expect(parser.push(encode('data: {"choices":[{"delta":{"content":"hi"}}]}\n\n'))).toEqual([]);
    expect(parser.push(encode('data: [DONE]\n\n'))).toEqual([]);
  });

  // A per-chunk TextDecoder().decode() splits a multi-byte character on the same boundary the
  // record parser has to survive, so the parser owns a streaming decoder.
  it('survives a multi-byte character split across a chunk boundary', () => {
    const parser = createTerminalFrameParser();
    const bytes = encode('data: {"choices":[{"delta":{"content":"é"}}]}\n\n' + FRAME);
    const cut = 40;
    expect(parser.push(bytes.slice(0, cut))).toEqual([]);
    const out = parser.push(bytes.slice(cut));
    expect(out).toHaveLength(1);
    const [frame] = out;
    if (frame === undefined) throw new Error('unreachable: toHaveLength(1) above');
    expect(frame.reason).toBe(ErrorReason.UPSTREAM_ERROR);
  });

  it('accepts CRLF record delimiters', () => {
    const parser = createTerminalFrameParser();
    const crlf = FRAME.replace(/\n\n$/, '\r\n\r\n');
    expect(parser.push(encode(crlf))).toHaveLength(1);
  });
});
