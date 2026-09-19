// SPDX-License-Identifier: Apache-2.0
//
// Runs ONCE, in the Playwright runner process, before any worker starts. It does not start servers:
// see playwright.config.ts. The single-zone build comes from `gateway-console-ts:build` (the Moon
// task's deps). The two-zone tier (SMA-512 PR4 task 3) also needs `iam-console`'s own standalone
// build — `gateway-console-ts:test-e2e`'s `deps` names `iam-console-ts:build` directly, in
// `ts/apps/gateway-console/moon.yml`. If that edge is ever missing, run both builds by hand:
//   moon run iam-console-ts:build gateway-console-ts:build
//
// This setup only CHECKS both build trees; it never writes into either (SMA-655). Each `build`
// stages its own standalone tree (moon.yml). iam-console's tree also serves iam-console's own
// tier, Moon runs the two tiers at the same time, and the delete-then-copy this setup used to do
// there wiped the tree under that tier. tests/unit/e2e-read-only.test.ts reds any fs write under
// tests/e2e/.
import { existsSync } from 'node:fs';
import path from 'node:path';
import { APP_DIR, IAM_CONSOLE_APP_DIR, IAM_CONSOLE_STANDALONE_DIR, STANDALONE_APP_DIR } from './support/paths';
import { assertStagedBuild } from './support/staged-build';

function check(moonTask: string, appDir: string, standaloneDir: string): void {
  const serverJs = path.join(standaloneDir, 'server.js');
  if (!existsSync(serverJs)) {
    throw new Error(`the standalone server is missing at ${serverJs}. Run \`moon run ${moonTask}\` first.`);
  }
  assertStagedBuild(appDir, standaloneDir, moonTask);
}

export default function globalSetup(): void {
  check('gateway-console-ts:build', APP_DIR, STANDALONE_APP_DIR);
  check('iam-console-ts:build', IAM_CONSOLE_APP_DIR, IAM_CONSOLE_STANDALONE_DIR);
}
