// SPDX-License-Identifier: Apache-2.0
//
// The `./chat` entry (spec § 8). GUARDED and at src/ root, like every other guarded entry.
import './server-guard.js';

import { mapError } from './errors/map-error.js';
import type { PaigasusError } from './errors/types.js';

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
