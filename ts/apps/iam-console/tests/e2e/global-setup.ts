// SPDX-License-Identifier: Apache-2.0
//
// Runs ONCE, in the Playwright runner process, before any worker starts. It does not start servers:
// see playwright.config.ts. The build comes from `iam-console-ts:build` (the Moon task's deps).
//
// This setup only CHECKS the build tree; it never writes into it (SMA-655). The standalone output
// has no .next/static of its own (measured, SMA-510), so `build` stages it (moon.yml). An e2e
// setup must not do that copy: gateway-console's two-zone tier serves from this same tree, Moon
// runs the two tiers at the same time, and a delete-then-copy here wiped the tree under the other
// tier. tests/unit/e2e-read-only.test.ts reds any fs write under tests/e2e/.
import { existsSync } from 'node:fs';
import path from 'node:path';
import { APP_DIR, STANDALONE_APP_DIR } from './support/paths';
import { assertStagedBuild } from './support/staged-build';

export default function globalSetup(): void {
  const serverJs = path.join(STANDALONE_APP_DIR, 'server.js');
  if (!existsSync(serverJs)) {
    throw new Error(`the standalone server is missing at ${serverJs}. Run \`moon run iam-console-ts:build\` first (iam-console-ts:test-e2e depends on it).`);
  }
  assertStagedBuild(APP_DIR, STANDALONE_APP_DIR, 'iam-console-ts:build');
}
