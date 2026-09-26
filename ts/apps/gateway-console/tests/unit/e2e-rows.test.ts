// SPDX-License-Identifier: Apache-2.0
//
// Spec § 10.4 has six rows (R1-R6). This zone's e2e tier adds two more cases that are not in the
// spec's table: R4b, a second capability case that pairs with R4, and R7, the 403 control this
// zone's plan added deliberately. Spec § 10.5 contributes five more (R8-R12): the two-zone tier
// (SMA-512 PR4 task 4). SMA-636 § 7.2 and § 7.3 add nine for the settings pages: R13-R18 (the
// single-zone rows), R19-R20 (the per-request call count) and R21 (the two-zone "Manage in IAM"
// link). Each id must have exactly one Playwright test whose title starts with it. A deleted or
// renamed scenario then fails this vitest suite, which runs in gateway-console-ts:test on every PR
// that touches the app, even when the e2e task does not run. Mirrors
// ts/apps/iam-console/tests/unit/e2e-rows.test.ts.
// SMA-635 § 7.2 adds eight: R22-R28 (the playground project, which runs the real gateway binary) and R29 (the streaming-off composer row, in the single-zone project).
// SMA-676 § 7.3 adds R30 (one identity grants model access to themself, in the playground project).
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const E2E_DIR = fileURLToPath(new URL('../e2e', import.meta.url));
const ROWS = [
  'R1',
  'R2',
  'R3',
  'R4',
  'R4b',
  'R5',
  'R6',
  'R7',
  'R8',
  'R9',
  'R10',
  'R11',
  'R12',
  'R13',
  'R14',
  'R15',
  'R16',
  'R17',
  'R18',
  'R19',
  'R20',
  'R21',
  'R22',
  'R23',
  'R24',
  'R25',
  'R26',
  'R27',
  'R28',
  'R29',
  'R30',
];

describe('the e2e tier covers every row this zone ships', () => {
  const titles = readdirSync(E2E_DIR)
    .filter((file) => file.endsWith('.spec.ts'))
    .flatMap((file) => [...readFileSync(path.join(E2E_DIR, file), 'utf8').matchAll(/^test\('(R\d+[a-z]?):/gm)].map((match) => match[1]));

  it.each(ROWS)('%s has exactly one test', (row) => {
    expect(titles.filter((title) => title === row)).toHaveLength(1);
  });

  it('has no test for a row this list does not have', () => {
    expect(titles.filter((title) => title === undefined || !ROWS.includes(title))).toEqual([]);
  });
});
