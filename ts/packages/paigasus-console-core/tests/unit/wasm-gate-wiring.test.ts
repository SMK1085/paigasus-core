// SPDX-License-Identifier: Apache-2.0
//
// The SMA-634 wasm drift gate can stop running with nothing red. This file pins its wiring.
//
// Checks 1-3 live in ts/packages/paigasus-kernel/tests/committed-wasm.test.ts, and that project's
// `node` vitest project takes an EXPLICIT `include` list. Delete the one entry naming the file and
// the three checks vanish while vitest reports a green run. Check 4 is
// `assertInstalledWasmMatchesCommitted()`, called at module scope from three vitest setup files;
// delete a call, or drop a setup file from its config's `setupFiles`, and check 4 is gone just as
// quietly. Neither deletion fails anything, because a test that does not run cannot fail.
//
// This file lives in @paigasus/console-core, not in the kernel, for one reason: this package's
// vitest `include` is a GLOB (`tests/**/*.test.ts`), so this test cannot itself be dropped by
// deleting a list entry. The kernel's config, the one this file reads, could do exactly that to a
// pin placed inside it. The paths it reads are declared as `inputs` on
// `paigasus-console-core-ts:test` in moon.yml, so an edit to any of them re-runs this file rather
// than serving a cached pass.
//
// This is a TEXT scan, not a parse. It cannot import the kernel's config: that config loads
// `vite-plugin-wasm`, a devDependency of the kernel package alone. The app setup files are matched
// on a line-anchored call, so a mention inside a `//` comment does not satisfy them.
//
// Residual: the scan proves the entries are PRESENT, not that the checks they start are sound.
// tests/committed-wasm.test.ts and testing/installed-wasm.ts carry their own controls for that.
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const HERE = dirname(fileURLToPath(import.meta.url));
// unit -> tests -> paigasus-console-core -> packages -> ts -> repo root: five levels up.
const ROOT = join(HERE, '../../../../../');
const APPS_DIR = join(ROOT, 'ts/apps');
const KERNEL_DIR = join(ROOT, 'ts/packages/paigasus-kernel');
const CONSOLE_CORE_DIR = join(ROOT, 'ts/packages/paigasus-console-core');

/** The file holding checks 1-3, relative to the kernel package — exactly as its `include` spells it. */
const COMMITTED_WASM_TEST = 'tests/committed-wasm.test.ts';
/** Check 4's entry point. */
const CHECK_4 = 'assertInstalledWasmMatchesCommitted';
/** The setup path each config must list, exactly as `setupFiles` spells it. */
const SETUP_ENTRY = './tests/support/setup.ts';

function read(path: string): string {
  return readFileSync(path, 'utf8');
}

/** Every quoted entry of every `include: [...]` array in a vitest config. */
function includeEntries(source: string): string[] {
  return [...source.matchAll(/include:\s*\[([^\]]*)\]/g)].flatMap((block) => [...block[1].matchAll(/['"]([^'"]+)['"]/g)].map((entry) => entry[1]));
}

/** Every quoted entry of every `setupFiles: [...]` array in a vitest config. */
function setupFileEntries(source: string): string[] {
  return [...source.matchAll(/setupFiles:\s*\[([^\]]*)\]/g)].flatMap((block) => [...block[1].matchAll(/['"]([^'"]+)['"]/g)].map((entry) => entry[1]));
}

/**
 * Every console app directory. DISCOVERED, not listed: a third zone that copies the setup file but
 * forgets the check 4 call must red here, and a hand-written list would simply not mention it.
 * A directory counts as an app when it holds a package.json, the same rule ci/next-env and
 * ci/affected-graph use.
 */
function appDirs(): string[] {
  return readdirSync(APPS_DIR, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && existsSync(join(APPS_DIR, entry.name, 'package.json')))
    .map((entry) => entry.name)
    .sort();
}

describe('checks 1-3 stay selected by the kernel vitest config', () => {
  it('the node project includes tests/committed-wasm.test.ts', () => {
    const config = join(KERNEL_DIR, 'vitest.config.ts');
    expect(includeEntries(read(config))).toContain(COMMITTED_WASM_TEST);
  });

  it('the included file exists', () => {
    expect(existsSync(join(KERNEL_DIR, COMMITTED_WASM_TEST))).toBe(true);
  });
});

describe('check 4 stays wired into every console vitest run', () => {
  // The discovery itself needs a control: an empty list would make every case below vacuous.
  it('finds the console apps', () => {
    const apps = appDirs();
    expect(apps.length).toBeGreaterThanOrEqual(2);
    expect(apps).toContain('iam-console');
    expect(apps).toContain('gateway-console');
  });

  const packages = ['ts/packages/paigasus-console-core', ...appDirs().map((app) => `ts/apps/${app}`)];

  it.each(packages)('%s calls it at module scope in its setup file', (pkg) => {
    const setup = join(ROOT, pkg, 'tests/support/setup.ts');
    expect(existsSync(setup)).toBe(true);
    const source = read(setup);
    // Line-anchored, so the identifier inside a `//` comment cannot satisfy either assertion.
    expect(source).toMatch(new RegExp(`^import \\{[^}]*\\b${CHECK_4}\\b`, 'm'));
    expect(source).toMatch(new RegExp(`^${CHECK_4}\\(\\);$`, 'm'));
  });

  it.each(packages)('%s lists that setup file in its vitest setupFiles', (pkg) => {
    const config = join(ROOT, pkg, 'vitest.config.ts');
    expect(existsSync(config)).toBe(true);
    expect(setupFileEntries(read(config))).toContain(SETUP_ENTRY);
  });

  it('console-core exports it from the testing subpath the apps import', () => {
    const index = read(join(CONSOLE_CORE_DIR, 'testing/index.ts'));
    expect(index).toMatch(new RegExp(`\\b${CHECK_4}\\b`));
  });
});
