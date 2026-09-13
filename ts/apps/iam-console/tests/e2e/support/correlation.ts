// SPDX-License-Identifier: Apache-2.0
//
// The e2e tier reads FORBIDDEN_VIEW_CORRELATION from lib/correlation.ts as TEXT: the module imports
// server-only, which throws under Playwright. A missing literal is a loud error, not a default.
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { APP_DIR } from './paths';

export type CorrelationMode = 'header' | 'fallback';

export function forbiddenViewCorrelation(): CorrelationMode {
  const source = readFileSync(path.join(APP_DIR, 'lib', 'correlation.ts'), 'utf8');
  const match = /export const FORBIDDEN_VIEW_CORRELATION(?::[^=]+)?= '(header|fallback)'/.exec(source);
  if (match?.[1] === undefined) {
    throw new Error("lib/correlation.ts does not export FORBIDDEN_VIEW_CORRELATION = 'header' | 'fallback' on one line (Task 12 Step 19 sets it)");
  }
  return match[1] as CorrelationMode;
}
