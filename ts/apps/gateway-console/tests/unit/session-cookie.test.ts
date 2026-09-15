// SPDX-License-Identifier: Apache-2.0
//
// @paigasus/auth exports SESSION_COOKIE from ./server (spec § 5.4). A second, hand-written copy of
// that name or its `__Host-` value anywhere under lib/ or in proxy.ts would silently drift from
// the package's own constant, and the shared session would stop working the day the two disagree.
// This reads every file under lib/ and proxy.ts as TEXT and asserts neither literal appears.
import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const APP_ROOT = fileURLToPath(new URL('../..', import.meta.url));

/** Every file under `dir` (relative to the app root), recursively. */
function filesUnder(dir: string): string[] {
  const abs = new URL(`../../${dir}`, import.meta.url);
  const out: string[] = [];
  for (const entry of readdirSync(abs, { withFileTypes: true })) {
    const relative = `${dir}/${entry.name}`;
    if (entry.isDirectory()) out.push(...filesUnder(relative));
    else out.push(relative);
  }
  return out;
}

const files = [...filesUnder('lib'), 'proxy.ts'];

describe('no zone hand-rolls the session cookie name', () => {
  it.each(files)('%s does not spell out the __Host- literal', (relative) => {
    const source = readFileSync(new URL(`../../${relative}`, import.meta.url), 'utf8');
    expect(source).not.toContain('__Host-');
  });

  it.each(files)('%s does not spell out SESSION_COOKIE_NAME', (relative) => {
    const source = readFileSync(new URL(`../../${relative}`, import.meta.url), 'utf8');
    expect(source).not.toContain('SESSION_COOKIE_NAME');
  });

  it('checked at least lib/auth.ts, lib/config.ts, lib/console.ts, lib/nav.ts and proxy.ts', () => {
    expect(files.sort()).toEqual(['lib/auth.ts', 'lib/config.ts', 'lib/console.ts', 'lib/nav.ts', 'proxy.ts'].sort());
    // Sanity: APP_ROOT resolves, so the relative reads above are actually reading this app's own
    // files and not silently matching nothing.
    expect(APP_ROOT.endsWith('gateway-console/')).toBe(true);
  });
});
