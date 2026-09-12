// SPDX-License-Identifier: Apache-2.0
//
// SMA-508 AC 1, moved here (spec § 7.7). A 'use client' component that imports @paigasus/sdk/iam
// must FAIL `next build`, with the server-only error. The POSITIVE CONTROL builds the same code
// from a server component and must succeed; without it, a build that failed for any other reason
// (a missing module, a bad config) would pass the negative case.
//
// The two fixtures are two directories because a Next build compiles every file of its project.
// The last case pins that their sdk-user.tsx files differ ONLY in the directive, so the control
// stays "the same fixture with the import in a server component".
//
// The negative case needs exit code 1 exactly: `null` (a spawn error, or the kill at the timeout)
// is not a build failure. The message regex matches Next 16.3.4's own error, "'server-only' cannot
// be imported from a Client Component module. It should only be used from a Server Component."
// (next/dist/build/webpack-config.js:1183; the swc binary carries the same text for Turbopack), and
// server-only's own throw, "This module cannot be imported from a Client Component module.". A
// looser /client component/i also matches Turbopack's layer labels ("Client Component Browser"),
// so a build that fails for another client-side reason would pass it.
//
// Measured build output (Next 16.3.4, Turbopack) carries the matched text as two separate lines,
// not one: "Error: 'server-only' cannot be imported from a Client Component module" and, further
// down under a code frame, "It should only be used from a Server Component." Both `toMatch` calls
// still pass, because each regex matches a substring of the full combined output independently.
import { spawnSync } from 'node:child_process';
import { readFileSync, rmSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const APP_DIR = fileURLToPath(new URL('../..', import.meta.url));
const FIXTURES_DIR = path.join(APP_DIR, 'tests', 'fixtures');
const BUILD_TIMEOUT_MS = 300_000;

type Fixture = 'client-imports-sdk' | 'server-imports-sdk';

function buildEnv(): NodeJS.ProcessEnv {
  const env: Record<string, string | undefined> = {};
  for (const [key, value] of Object.entries(process.env)) {
    // vitest sets NODE_ENV=test; `next build` must run as production. The fixture reads no config.
    if (key === 'NODE_ENV' || key.startsWith('PAIGASUS_') || key.startsWith('__NEXT')) continue;
    env[key] = value;
  }
  return { ...env, NODE_ENV: 'production', NEXT_TELEMETRY_DISABLED: '1' };
}

function build(fixture: Fixture): { status: number | null; output: string } {
  rmSync(path.join(FIXTURES_DIR, fixture, '.next'), { recursive: true, force: true });
  // `pnpm exec next build <dir>` from the app directory: the command app-shell's e2e fixture uses.
  const result = spawnSync('pnpm', ['exec', 'next', 'build', `tests/fixtures/${fixture}`], {
    cwd: APP_DIR,
    env: buildEnv(),
    encoding: 'utf8',
    timeout: BUILD_TIMEOUT_MS,
  });
  return { status: result.status, output: `${result.stdout}\n${result.stderr}` };
}

describe('a client component cannot import @paigasus/sdk (spec § 7.7)', () => {
  it('fails `next build` with the server-only error when a client component imports the SDK', { timeout: BUILD_TIMEOUT_MS + 10_000 }, () => {
    const { status, output } = build('client-imports-sdk');
    expect(status, output).toBe(1);
    expect(output).toMatch(/server-only/);
    expect(output).toMatch(/cannot be imported from a Client Component module/);
  });

  it('builds the same code when a server component imports the SDK (positive control)', { timeout: BUILD_TIMEOUT_MS + 10_000 }, () => {
    const { status, output } = build('server-imports-sdk');
    expect(status, output).toBe(0);
  });

  it('keeps the two fixtures identical except for the directive', () => {
    const read = (fixture: Fixture, file: string) => readFileSync(path.join(FIXTURES_DIR, fixture, file), 'utf8');
    expect(read('client-imports-sdk', 'app/sdk-user.tsx').replace("'use client';\n\n", '')).toBe(read('server-imports-sdk', 'app/sdk-user.tsx'));
    for (const file of ['next.config.ts', 'tsconfig.json', 'app/layout.tsx', 'app/page.tsx']) {
      expect(read('client-imports-sdk', file), file).toBe(read('server-imports-sdk', file));
    }
  });
});
