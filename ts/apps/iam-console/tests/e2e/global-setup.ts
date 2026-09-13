// SPDX-License-Identifier: Apache-2.0
//
// Runs ONCE, in the Playwright runner process, before any worker starts. It does not start servers:
// see playwright.config.ts. The build comes from `iam-console-ts:build` (the Moon task's deps).
import { cpSync, existsSync, rmSync } from 'node:fs';
import path from 'node:path';
import { APP_DIR, STANDALONE_APP_DIR } from './support/paths';

export default function globalSetup(): void {
  const serverJs = path.join(STANDALONE_APP_DIR, 'server.js');
  if (!existsSync(serverJs)) {
    throw new Error(`the standalone server is missing at ${serverJs}. Run \`moon run iam-console-ts:build\` first (iam-console-ts:test-e2e depends on it).`);
  }
  // The standalone tree has NO .next/static (measured, SMA-510). Without this copy every client
  // chunk is a 404, nothing hydrates, and no Server Action can run.
  const staticTarget = path.join(STANDALONE_APP_DIR, '.next', 'static');
  rmSync(staticTarget, { recursive: true, force: true });
  cpSync(path.join(APP_DIR, '.next', 'static'), staticTarget, { recursive: true });
  const publicDir = path.join(APP_DIR, 'public');
  if (existsSync(publicDir)) cpSync(publicDir, path.join(STANDALONE_APP_DIR, 'public'), { recursive: true });
}
