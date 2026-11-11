// SPDX-License-Identifier: Apache-2.0
// SMA-749 § 8.2: the console's stream parser reads every OpenAI golden stream. It shows the
// notice as text, ends with `done`, and raises no error. It does not accumulate tool calls
// (P-7), so this asserts parse-and-notice only.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { createChatStreamParser } from '../../lib/chat-stream';

const golden = fileURLToPath(new URL('../../../../../rs/crates/libs/paigasus-gateway-core/tests/golden/', import.meta.url));
const manifest = JSON.parse(readFileSync(`${golden}manifest.json`, 'utf8')) as { file: string; format: string; notices: string[] }[];

describe('the console stream parser on the tool-interception goldens', () => {
  const cases = manifest.filter((c) => c.format === 'openai');
  it('has goldens to read', () => expect(cases.length).toBeGreaterThan(0));
  for (const c of cases) {
    it(c.file, () => {
      const parser = createChatStreamParser();
      const events = [...parser.push(new Uint8Array(readFileSync(`${golden}${c.file}`))), ...parser.end()];
      expect(events.filter((e) => e.kind === 'error')).toEqual([]);
      expect(events.at(-1)).toEqual({ kind: 'done' });
      const text = events.flatMap((e) => (e.kind === 'delta' ? [e.text] : [])).join('');
      // `: paigasus-decision` lines are SSE comments, so they must not surface as text.
      expect(text).not.toContain('paigasus-decision');
      for (const n of c.notices) expect(text).toContain(n);
      expect(text).not.toContain('CANARY');
    });
  }
});
