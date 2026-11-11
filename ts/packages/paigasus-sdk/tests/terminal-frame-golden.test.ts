// SPDX-License-Identifier: Apache-2.0
// SMA-749 § 8.2: the SDK's terminal-frame parser finds no error in any golden stream. A
// rewritten stream must never look like a failed one. It does not accumulate tool calls (P-7).
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { createTerminalFrameParser } from '../src/chat';

const golden = fileURLToPath(new URL('../../../../rs/crates/libs/paigasus-gateway-core/tests/golden/', import.meta.url));
const manifest = JSON.parse(readFileSync(`${golden}manifest.json`, 'utf8')) as { file: string; format: string }[];

describe('the terminal-frame parser on the tool-interception goldens', () => {
  const cases = manifest.filter((m) => m.format === 'openai');
  it('has goldens to read', () => expect(cases.length).toBeGreaterThan(0));
  for (const c of cases) {
    it(c.file, () => {
      const parser = createTerminalFrameParser(200);
      expect(parser.push(new Uint8Array(readFileSync(`${golden}${c.file}`)))).toEqual([]);
    });
  }
});
