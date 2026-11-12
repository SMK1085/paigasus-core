// SPDX-License-Identifier: Apache-2.0
// SMA-749 § 8.2: the SDK's terminal-frame parser finds no error in any golden stream. A
// rewritten stream must never look like a failed one. It does not accumulate tool calls (P-7).
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { createTerminalFrameParser } from '../src/chat';

const golden = fileURLToPath(new URL('../../../../rs/crates/libs/paigasus-gateway-core/tests/golden/', import.meta.url));
const chatRs = fileURLToPath(new URL('../../../../rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs', import.meta.url));

/** The gateway's TERMINAL_SSE_ERROR, read by constant name as terminal-frame.test.ts reads it. */
function terminalFrame(): string {
  const literal = /const TERMINAL_SSE_ERROR: &str = "((?:[^"\\]|\\.)*)";/.exec(readFileSync(chatRs, 'utf8'))?.[1];
  if (literal === undefined) throw new Error(`TERMINAL_SSE_ERROR not found in ${chatRs}`);
  return literal.replace(/\\(.)/g, (_all, ch: string) => (ch === 'n' ? '\n' : ch === 't' ? '\t' : ch === 'r' ? '\r' : ch));
}
const manifest = JSON.parse(readFileSync(`${golden}manifest.json`, 'utf8')) as { file: string; format: string }[];

describe('the terminal-frame parser on the tool-interception goldens', () => {
  const cases = manifest.filter((m) => m.format === 'openai');
  it('has goldens to read', () => expect(cases.length).toBeGreaterThan(0));
  for (const c of cases) {
    it(c.file, () => {
      const parser = createTerminalFrameParser(200);
      expect(parser.push(new Uint8Array(readFileSync(`${golden}${c.file}`)))).toEqual([]);
    });
    // Positive control: the same parser on the same golden DOES see the gateway's terminal frame
    // when one follows, so the empty result above is not a parser that finds nothing.
    it(`${c.file} + the terminal frame`, () => {
      const parser = createTerminalFrameParser(200);
      const stream = readFileSync(`${golden}${c.file}`, 'utf8');
      expect(parser.push(new TextEncoder().encode(stream + terminalFrame()))).toHaveLength(1);
    });
  }
});
