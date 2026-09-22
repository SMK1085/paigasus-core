// SPDX-License-Identifier: Apache-2.0
//
// The SMA-511 spec (§ 9.4) has thirteen rows, R1–R13. The SMA-630 spec
// (docs/superpowers/specs/2026-09-17-sma-630-tenancy-lifecycle-design.md, § 9.3) adds three, R14–R16.
// The SMA-629 spec (docs/superpowers/specs/2026-09-19-sma-629-dead-letters-capability-design.md,
// § 7.4) adds three, R17–R19. The SMA-661 spec
// (docs/superpowers/specs/2026-09-21-sma-661-dead-letters-bulk-replay-design.md, § 7.5) adds one,
// R20. Each row must have exactly one Playwright test whose title starts with its row id. A deleted
// or renamed scenario then fails this vitest suite, which runs in iam-console-ts:test on every PR
// that touches the app, even when the e2e task does not run.
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const E2E_DIR = fileURLToPath(new URL('../e2e', import.meta.url));
const ROWS = Array.from({ length: 20 }, (_, index) => `R${String(index + 1)}`);

describe('the e2e tier covers every row of SMA-511 § 9.4, SMA-630 § 9.3, SMA-629 § 7.4 and SMA-661 § 7.5', () => {
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
