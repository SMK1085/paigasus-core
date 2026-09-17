// SPDX-License-Identifier: Apache-2.0
//
// Runs ONCE, in the Playwright runner process, before any worker starts. It does not start servers:
// see playwright.config.ts. The single-zone build comes from `gateway-console-ts:build` (the Moon
// task's deps). The two-zone tier (SMA-512 PR4 task 3) also needs `iam-console`'s own standalone
// build staged the same way — `gateway-console-ts:test-e2e`'s `deps` names `iam-console-ts:build`
// directly, at `ts/apps/gateway-console/moon.yml` around :272. If that edge is ever missing, run
// both builds by hand:
//   moon run iam-console-ts:build gateway-console-ts:build
import { cpSync, existsSync, rmSync } from 'node:fs';
import path from 'node:path';
import { APP_DIR, IAM_CONSOLE_APP_DIR, IAM_CONSOLE_STANDALONE_DIR, STANDALONE_APP_DIR } from './support/paths';

/** Asserts the build exists, then copies `.next/static` and `public/` into the standalone tree —
 * the standalone output carries no `static` tree (measured, SMA-510), so without this copy every
 * client chunk is a 404, nothing hydrates, and no Server Action can run. */
function stage(moonTask: string, appDir: string, standaloneDir: string): void {
  const serverJs = path.join(standaloneDir, 'server.js');
  if (!existsSync(serverJs)) {
    throw new Error(`the standalone server is missing at ${serverJs}. Run \`moon run ${moonTask}\` first.`);
  }
  const staticTarget = path.join(standaloneDir, '.next', 'static');
  rmSync(staticTarget, { recursive: true, force: true });
  cpSync(path.join(appDir, '.next', 'static'), staticTarget, { recursive: true });
  // Cleared FIRST, exactly like `.next/static` two lines above, and cleared even when the source
  // is absent. cpSync merges into an existing tree rather than replacing it, so without this a
  // file deleted from `public/` would survive in the standalone copy across runs and the tier
  // would serve an asset the app no longer ships. Neither app has a `public/` today, so this
  // branch is currently unreachable — which is the reason to make it correct now rather than
  // when someone adds a favicon and hits it.
  const publicDir = path.join(appDir, 'public');
  const publicTarget = path.join(standaloneDir, 'public');
  rmSync(publicTarget, { recursive: true, force: true });
  if (existsSync(publicDir)) cpSync(publicDir, publicTarget, { recursive: true });
}

export default function globalSetup(): void {
  stage('gateway-console-ts:build', APP_DIR, STANDALONE_APP_DIR);
  stage('iam-console-ts:build', IAM_CONSOLE_APP_DIR, IAM_CONSOLE_STANDALONE_DIR);
}
