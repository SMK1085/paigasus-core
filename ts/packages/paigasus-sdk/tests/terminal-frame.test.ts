// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { createTerminalFrameParser } from '../src/errors/terminal-frame.js';
import { CHAT_RS, TERMINAL_SSE_ERROR } from './fixtures/terminal-frame.js';

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');
const IDS = { correlationId: 'corr-1', requestId: 'req-1' };
const encode = (s: string): Uint8Array => new TextEncoder().encode(s);

describe('the fixture still matches the Rust constant', () => {
  it('finds the frame verbatim in chat.rs', () => {
    const source = readFileSync(resolve(REPO_ROOT, CHAT_RS), 'utf8');
    // The Rust literal escapes its quotes and newlines; reconstruct what it denotes.
    const literal = /const TERMINAL_SSE_ERROR: &str = "(.*)";/.exec(source)?.[1];
    expect(literal).toBeDefined();
    const denoted = literal!.replaceAll('\\"', '"').replaceAll('\\n', '\n');
    expect(denoted).toBe(TERMINAL_SSE_ERROR);
  });
});

describe('the parser', () => {
  it('returns the mapped error for the pinned frame', () => {
    const parser = createTerminalFrameParser(IDS);
    const error = parser.push(encode(TERMINAL_SSE_ERROR));

    expect(error).not.toBeNull();
    expect(error!.rawReason).toBe('upstream-error');
    expect(error!.message).toBe('upstream stream error');
    expect(error!.correlationId).toBe('corr-1');
    expect(error!.requestId).toBe('req-1');
    // No header can change after the head is committed, so the frame asserts nothing here.
    expect(error!.retryable).toBeNull();
  });

  it('finds a frame split across two chunks', () => {
    const parser = createTerminalFrameParser(IDS);
    const half = Math.floor(TERMINAL_SSE_ERROR.length / 2);
    expect(parser.push(encode(TERMINAL_SSE_ERROR.slice(0, half)))).toBeNull();
    expect(parser.push(encode(TERMINAL_SSE_ERROR.slice(half)))).not.toBeNull();
  });

  it('finds the terminal frame after a data frame in the same chunk', () => {
    const parser = createTerminalFrameParser(IDS);
    const error = parser.push(encode(`data: {"id":"chatcmpl-1"}\n\n${TERMINAL_SSE_ERROR}`));
    expect(error).not.toBeNull();
    expect(error!.rawReason).toBe('upstream-error');
  });

  it('returns null for ordinary data frames', () => {
    const parser = createTerminalFrameParser(IDS);
    expect(parser.push(encode('data: {"id":"a"}\n\ndata: [DONE]\n\n'))).toBeNull();
  });

  it('returns null for a partial trailing record that never completes', () => {
    const parser = createTerminalFrameParser(IDS);
    expect(parser.push(encode('data: {"error":{"code":"upstream-'))).toBeNull();
  });

  it('survives a multi-byte character split across a chunk boundary', () => {
    // A fresh TextDecoder per chunk would corrupt this. The parser owns ONE streaming decoder.
    const frame = 'data: {"error":{"message":"café ☕","type":"api_error","param":null,"code":"upstream-error"}}\n\n';
    const bytes = encode(frame);
    const cut = frame.indexOf('café') + 4; // lands inside the two-byte é
    const parser = createTerminalFrameParser(IDS);
    expect(parser.push(bytes.slice(0, cut))).toBeNull();
    const error = parser.push(bytes.slice(cut));
    expect(error).not.toBeNull();
    expect(error!.message).toBe('café ☕');
  });

  it.each([['\n\n'], ['\r\n\r\n'], ['\r\r']])('accepts the %j record delimiter', (delimiter) => {
    const parser = createTerminalFrameParser(IDS);
    const frame = TERMINAL_SSE_ERROR.replace('\n\n', delimiter);
    expect(parser.push(encode(frame))).not.toBeNull();
  });

  it('returns null forever after the first terminal error', () => {
    const parser = createTerminalFrameParser(IDS);
    expect(parser.push(encode(TERMINAL_SSE_ERROR))).not.toBeNull();
    expect(parser.push(encode(TERMINAL_SSE_ERROR))).toBeNull();
  });

  it('drops the buffer rather than growing without bound', () => {
    // An upstream that never sends a blank line must not exhaust memory. Dropping is correct: the
    // parser is a best-effort observer, never a reason to fail a stream still delivering data.
    const parser = createTerminalFrameParser(IDS);
    expect(parser.push(encode('data: '.padEnd(70_000, 'x')))).toBeNull();
    expect(parser.push(encode(TERMINAL_SSE_ERROR))).not.toBeNull();
  });

  it('accepts a string chunk from a caller that already decoded', () => {
    const parser = createTerminalFrameParser(IDS);
    expect(parser.push(TERMINAL_SSE_ERROR)).not.toBeNull();
  });
});
