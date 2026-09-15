// SPDX-License-Identifier: Apache-2.0
//
// Spec § 10.4 has six rows (R1-R6). This zone's e2e tier adds two more cases that are not in the
// spec's table: R4b, a second capability case that pairs with R4, and R7, the 403 control this
// zone's plan added deliberately. Each of these eight ids must have exactly one Playwright test
// whose title starts with it. A deleted or renamed scenario then fails this vitest suite, which
// runs in gateway-console-ts:test on every PR that touches the app, even when the e2e task does
// not run. Mirrors ts/apps/iam-console/tests/unit/e2e-rows.test.ts.
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const E2E_DIR = fileURLToPath(new URL('../e2e', import.meta.url));
const ROWS = ['R1', 'R2', 'R3', 'R4', 'R4b', 'R5', 'R6', 'R7'];

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
