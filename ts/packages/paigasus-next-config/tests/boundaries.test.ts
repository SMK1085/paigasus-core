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
    // makes `entry.files[0]` `string | undefined`, and chaining `.split()` straight off it is both
    // a type error and an unsafe-member-access finding under the typed ESLint rules.
    const scoped = boundaryRules.map((entry) => {
      const first: string | undefined = entry.files[0];
      expect(first, `${entry.name} declares no files glob`).toBeDefined();
      return (first ?? '').split('/**')[0];
    });
    for (const dir of Object.keys(BOUNDARY_SCOPES)) {
      expect(scoped).toContain(dir);
    }
  });
});

describe('the workspace eslint config actually applies the preset', () => {
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
