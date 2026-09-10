// SPDX-License-Identifier: Apache-2.0
import '../server-guard.js';

import { mapError } from './map-error.js';
import type { FrameIds } from './map-error.js';
import type { PaigasusError } from './types.js';

/**
 * Beyond this, a stream that never sends a blank line would grow the buffer without bound.
 *
 * A count of UTF-16 code units (`buffer.length`), not bytes: the real byte cap can be a small
 * multiple higher for multi-byte content. The cap works fine as a memory guard either way; only
 * the old name overstated its precision.
 */
const MAX_BUFFER_CHARS = 64 * 1024;

/**
 * A record ends at a blank line — any TWO consecutive line terminators, each of which may be
 * CRLF, LF or CR. `chat.rs:63` emits `\n\n`, but upstream chunks pass through this gateway
 * verbatim, so the grammar is what this follows, not that one producer's habit.
 *
 * The alternation puts `\r\n` first so a CRLF is consumed whole rather than as a bare CR — which
 * is what makes `\r\n\r\n` match as one four-character delimiter instead of two two-character ones.
 */
const RECORD_DELIMITER = /(?:\r\n|\r|\n){2}/;

function firstDelimiter(buffer: string): { index: number; length: number } | null {
  const match = RECORD_DELIMITER.exec(buffer);
  if (match === null) return null;
  return { index: match.index, length: match[0].length };
}

/**
 * A stateful, incremental parser over an SSE stream, looking for the gateway's ONE terminal error
 * frame (spec § 8.5).
 *
 * Exported because the SDK never reads the stream itself — scanning would mean buffering, which
 * defeats the passthrough AC 4 requires. A caller consuming its own stream drives this with the
 * chunks it is already reading. The parser holds only the trailing partial record, so it does not
 * reintroduce that buffering.
 *
 * A chunk boundary can fall anywhere, so a parser over one chunk would miss the terminal frame in
 * exactly the split case it exists to catch — hence the state.
 *
 * `committedStatus` is the gateway's real 2xx response status. The parser itself sees only bytes
 * and cannot know it; `chatCompletion` returns it on `ChatCompletionResult.status`, so the caller
 * — the only thing that constructs a parser — always has it. chat.rs:128 branches on
 * `is_success()`, so 200 is the usual value but not the only reachable one (spec § 9.5 arm 4).
 */
export function createTerminalFrameParser(ids: FrameIds, committedStatus: number): { push(chunk: Uint8Array | string): PaigasusError | null } {
  // ONE decoder for the parser's lifetime. A fresh TextDecoder per chunk corrupts a multi-byte
  // character split across a chunk boundary — the same defect class as a split record, one level
  // down.
  const decoder = new TextDecoder('utf-8');
  let buffer = '';
  let done = false;

  return {
    push(chunk: Uint8Array | string): PaigasusError | null {
      if (done) return null;

      buffer += typeof chunk === 'string' ? chunk : decoder.decode(chunk, { stream: true });

      let found: PaigasusError | null = null;
      for (;;) {
        const delimiter = firstDelimiter(buffer);
        if (delimiter === null) break;
        const record = buffer.slice(0, delimiter.index);
        buffer = buffer.slice(delimiter.index + delimiter.length);

        const error = parseRecord(record, ids, committedStatus);
        if (error !== null) {
          found = error;
          done = true;
          buffer = '';
          break;
        }
      }

      // Best-effort: drop an unbounded partial rather than fail a stream still delivering data.
      if (buffer.length > MAX_BUFFER_CHARS) buffer = '';
      return found;
    },
  };
}

/** An SSE record is an error only if its `data:` payload is an OpenAI envelope carrying a code. */
function parseRecord(record: string, ids: FrameIds, committedStatus: number): PaigasusError | null {
  const data = record
    .split(/\r\n|\n|\r/)
    .filter((line) => line.startsWith('data:'))
    .map((line) => line.slice('data:'.length).trim())
    .join('\n');
  if (data === '' || data === '[DONE]') return null;

  let body: unknown;
  try {
    body = JSON.parse(data);
  } catch {
    return null;
  }
  if (typeof body !== 'object' || body === null) return null;
  const error = (body as { error?: unknown }).error;
  if (typeof error !== 'object' || error === null) return null;
  if (typeof (error as { code?: unknown }).code !== 'string') return null;

  return mapError({ kind: 'terminal-frame', status: committedStatus, body, ids });
}
