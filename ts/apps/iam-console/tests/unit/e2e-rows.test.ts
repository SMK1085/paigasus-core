// SPDX-License-Identifier: Apache-2.0
//
// Spec § 9.4 has twelve rows. Each must have exactly one Playwright test whose title starts with
// its row id. A deleted or renamed scenario then fails this vitest suite, which runs in
// iam-console-ts:test on every PR that touches the app, even when the e2e task does not run.
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const E2E_DIR = fileURLToPath(new URL('../e2e', import.meta.url));
const ROWS = Array.from({ length: 12 }, (_, index) => `R${String(index + 1)}`);

describe('the e2e tier covers every row of spec § 9.4', () => {
  const titles = readdirSync(E2E_DIR)
    .filter((file) => file.endsWith('.spec.ts'))
    .flatMap((file) => [...readFileSync(path.join(E2E_DIR, file), 'utf8').matchAll(/^test\('(R\d+):/gm)].map((match) => match[1]));

  it.each(ROWS)('%s has exactly one test', (row) => {
    expect(titles.filter((title) => title === row)).toHaveLength(1);
  });

  it('has no test for a row the spec does not have', () => {
    expect(titles.filter((title) => title === undefined || !ROWS.includes(title))).toEqual([]);
  });
});
