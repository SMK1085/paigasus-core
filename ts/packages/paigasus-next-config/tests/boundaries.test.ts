// SPDX-License-Identifier: Apache-2.0
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { ESLint } from 'eslint';
import { describe, expect, it } from 'vitest';
import { BOUNDARY_SCOPES, boundaryRules } from '../src/eslint.mjs';

/** The ts workspace root — `files` globs in the preset are relative to it. */
const TS_ROOT = fileURLToPath(new URL('../../..', import.meta.url));

/**
 * Map every `ts/packages/*` package's declared `package.json` `name` to its directory. Used to
 * catch a package landing under a directory name the boundary preset does not expect — see
 * `expectedPackageName` below.
 */
function packageDirsByName(): Map<string, string> {
  const packagesDir = path.join(TS_ROOT, 'packages');
  const map = new Map<string, string>();
  for (const entry of readdirSync(packagesDir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const pkgJsonPath = path.join(packagesDir, entry.name, 'package.json');
    if (!existsSync(pkgJsonPath)) continue;
    const pkg: unknown = JSON.parse(readFileSync(pkgJsonPath, 'utf8'));
    const name = typeof pkg === 'object' && pkg !== null && 'name' in pkg ? pkg.name : undefined;
    if (typeof name === 'string') {
      map.set(name, `packages/${entry.name}`);
    }
  }
  return map;
}

/**
 * The `@paigasus/<x>` package name a `packages/paigasus-<x>` boundary scope expects, derived from
 * the scope path itself rather than hand-maintained as a second list — two lists drift. A scope
 * outside `packages/` (e.g. `apps`) is not a single package, so there is nothing to derive.
 */
function expectedPackageName(scopeDir: string): string | undefined {
  const suffix = /^packages\/paigasus-(.+)$/.exec(scopeDir)?.[1];
  return suffix === undefined ? undefined : `@paigasus/${suffix}`;
}

async function restrictedImportsFor(filePath: string, source: string): Promise<string[]> {
  const eslint = new ESLint({ cwd: TS_ROOT, overrideConfigFile: true, overrideConfig: boundaryRules });
  const [result] = await eslint.lintText(source, { filePath, warnIgnored: false });
  return (result?.messages ?? []).filter((m) => m.ruleId === 'no-restricted-imports').map((m) => m.message);
}

const DENIED: ReadonlyArray<readonly [string, string, string]> = [
  ['ui must not import next', 'packages/paigasus-ui/src/button.tsx', "import Link from 'next/link';"],
  ['ui must not import a workspace package', 'packages/paigasus-ui/src/button.tsx', "import { x } from '@paigasus/sdk';"],
  ['ui must not import a workspace SUBPATH', 'packages/paigasus-ui/src/button.tsx', "import { x } from '@paigasus/sdk/client';"],
  ['sdk must not import ui', 'packages/paigasus-sdk/src/iam.ts', "import { x } from '@paigasus/ui';"],
  ['sdk must not import a ui SUBPATH', 'packages/paigasus-sdk/src/iam.ts', "import { x } from '@paigasus/ui/button';"],
  ['sdk must not import react', 'packages/paigasus-sdk/src/iam.ts', "import { useState } from 'react';"],
  ['app-shell must not import the sdk', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/sdk';"],
  ['app-shell must not import auth/server', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/auth/server';"],
  ['apps must not import proto', 'apps/paigasus-console/app/page.tsx', "import { x } from '@paigasus/proto';"],
  ['apps must not import a proto SUBPATH', 'apps/paigasus-console/app/page.tsx', "import { x } from '@paigasus/proto/gen/iam';"],
];

const ALLOWED: ReadonlyArray<readonly [string, string, string]> = [
  ['ui may import react', 'packages/paigasus-ui/src/button.tsx', "import { useState } from 'react';"],
  ['sdk may import proto', 'packages/paigasus-sdk/src/iam.ts', "import { x } from '@paigasus/proto';"],
  ['sdk may import a proto SUBPATH', 'packages/paigasus-sdk/src/iam.ts', "import { x } from '@paigasus/proto/gen/iam';"],
  ['app-shell may import auth/client', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/auth/client';"],
  ['app-shell may import ui', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/ui';"],
  ['apps may import the sdk', 'apps/paigasus-console/app/page.tsx', "import { x } from '@paigasus/sdk';"],
  ['apps may import ui directly — the deliberate § 7.3 deviation', 'apps/paigasus-console/app/page.tsx', "import { x } from '@paigasus/ui';"],
  ['apps may import next', 'apps/paigasus-console/app/page.tsx', "import Link from 'next/link';"],
];

describe('boundary preset', () => {
  it.each(DENIED)('reports: %s', async (_label, filePath, source) => {
    expect(await restrictedImportsFor(filePath, source)).not.toHaveLength(0);
  });

  it.each(ALLOWED)('permits: %s', async (_label, filePath, source) => {
    expect(await restrictedImportsFor(filePath, source)).toHaveLength(0);
  });

  it('every scope either exists on disk or states why it does not yet', () => {
    const packagesByName = packageDirsByName();
    for (const [dir, note] of Object.entries(BOUNDARY_SCOPES)) {
      const present = existsSync(new URL(`../../../${dir}`, import.meta.url));
      if (!present) {
        expect(note, `${dir} is absent, so BOUNDARY_SCOPES must state why`).not.toBe('exists');
        expect(note.trim().length, `${dir} needs a real reason, not a blank one`).toBeGreaterThan(0);

        // The reason string alone never re-validates itself: if the package this scope expects
        // lands under a DIFFERENT directory name, `dir` stays permanently absent and the stale
        // reason keeps the checks above green forever while the rule silently never applies. So
        // also assert that no package anywhere under ts/packages declares the name this scope
        // expects — the moment it does, this fails until the scope (and the rule's files glob)
        // is updated to the real directory.
        const expectedName = expectedPackageName(dir);
        if (expectedName !== undefined) {
          const foundAt = packagesByName.get(expectedName);
          expect(
            foundAt,
            `${expectedName} now exists at ${String(foundAt)}, but BOUNDARY_SCOPES still expects ${dir} — update the scope (and its rule's files glob) to match, or the rule stays permanently inert`,
          ).toBeUndefined();
        }
      }
    }
  });

  it('every rule scope has a matching entry in the preset', () => {
    // Indexed through a typed local rather than a chained member access: `noUncheckedIndexedAccess`
    // makes `entry.files?.[0]` `string | string[] | undefined` — `boundaryRules` is typed as
    // ESLint's own `Linter.Config[]`, whose `files` entries can themselves be a nested `string[]`,
    // not only a plain glob string. Narrowed with a `typeof` guard rather than a cast, so a config
    // entry that actually used the nested-array form would fail loudly here instead of being
    // silently coerced.
    const scoped = boundaryRules.map((entry) => {
      const label = entry.name ?? '(unnamed boundary entry)';
      const first: string | string[] | undefined = entry.files?.[0];
      expect(first, `${label} declares no files glob`).toBeDefined();
      expect(typeof first === 'string', `${label} files[0] is a nested string[], not a plain glob — this test does not handle that shape`).toBe(true);
      return typeof first === 'string' ? first.split('/**')[0] : '';
    });
    for (const dir of Object.keys(BOUNDARY_SCOPES)) {
      expect(scoped).toContain(dir);
    }
    // And the REVERSE. The loop above proves every declared scope has a rule; on its own it says
    // nothing about a rule added WITHOUT a scope entry, which would get no liveness coverage at
    // all — the package could land under a different directory name and the rule would sit
    // permanently inert with nothing to notice.
    for (const dir of scoped) {
      expect(Object.keys(BOUNDARY_SCOPES), `the ${dir} rule has no BOUNDARY_SCOPES entry, so nothing proves its files glob still matches a real directory`).toContain(dir);
    }
  });
});

/**
 * Lint through the REAL `ts/eslint.config.js` — no `overrideConfigFile`, no `overrideConfig`.
 *
 * Every other row in this file passes `overrideConfigFile: true`, which bypasses the shipped
 * config entirely. That is right for testing the preset in isolation and wrong as the ONLY
 * coverage: adding `'packages/**'` to `ts/eslint.config.js`'s global `ignores` array switches all
 * four boundary blocks off for real code, and leaves every other assertion here green — the
 * structural comparison below inspects `files` arrays and never lints.
 *
 * The probe paths end in `.mjs`, not `.tsx`. MEASURED: the shipped config turns on typed linting
 * for every ts/tsx/mts/cts path with `projectService: true`, and lintText on a TypeScript path
 * that no tsconfig includes returns one FATAL parsing error and runs no rules at all — so a `.tsx`
 * probe would report zero restricted imports whether or not the boundary rules applied, and this
 * row would be permanently red for the wrong reason. The boundary rules' own `files` globs cover
 * `mjs` alongside `tsx`, and the global `ignores` array this row exists to police is applied
 * before any of that, so the `.mjs` path tests exactly the same thing.
 */
async function realConfigRestrictedImportsFor(filePath: string, source: string): Promise<string[]> {
  const eslint = new ESLint({ cwd: TS_ROOT });
  const [result] = await eslint.lintText(source, { filePath, warnIgnored: false });
  return (result?.messages ?? []).filter((m) => m.ruleId === 'no-restricted-imports').map((m) => m.message);
}

describe('the workspace eslint config actually applies the preset', () => {
  // The file need not exist on disk — lintText takes the source and the path it is to be judged
  // as. A path under packages/ is what an `ignores: ['packages/**']` entry would silence.
  it('lints a denied packages/ import through the REAL config, not an override', async () => {
    const messages = await realConfigRestrictedImportsFor('packages/paigasus-ui/src/probe.mjs', "import { x } from '@paigasus/sdk';\nexport const y = x;\n");
    expect(messages, 'ts/eslint.config.js did not apply the ui boundary rule to a packages/ path — check its global `ignores` array').not.toHaveLength(0);
  });

  // The apps/ half, because a single `ignores` entry silences one tree at a time: `'packages/**'`
  // leaves the row above red and this one green, and `'apps/**'` does the reverse.
  it('lints a denied apps/ import through the REAL config, not an override', async () => {
    const messages = await realConfigRestrictedImportsFor('apps/paigasus-console/app/probe.mjs', "import { x } from '@paigasus/proto';\nexport const y = x;\n");
    expect(messages, 'ts/eslint.config.js did not apply the apps boundary rule to an apps/ path — check its global `ignores` array').not.toHaveLength(0);
  });

  it('carries every boundary entry in its EXPORTED array, not merely as an import', async () => {
    // Importing the real config is what makes this an assertion rather than a text scan: a dead
    // `import` statement satisfies a grep, and deleting only the spread would leave every test
    // above green while no app is actually governed by these rules.
    const shipped = (await import('../../../eslint.config.js')).default as Array<{ files?: string[] }>;
    for (const entry of boundaryRules) {
      expect(shipped, `ts/eslint.config.js dropped the ${entry.name} boundary block`).toContainEqual(expect.objectContaining({ files: entry.files }));
    }
  });
});
