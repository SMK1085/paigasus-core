// SPDX-License-Identifier: Apache-2.0
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { ESLint, type Linter } from 'eslint';
import tseslint from 'typescript-eslint';
import { describe, expect, it } from 'vitest';
import { BOUNDARY_SCOPES, boundaryRules, sourceRules } from '../src/eslint.mjs';

/**
 * A TypeScript-aware parser, with no type-checked rules attached. `boundaryRules` on its own
 * carries no `languageOptions`, so `overrideConfigFile: true` falls back to ESLint's default
 * (espree) parser — fine for the plain-ESM rows below, but espree cannot parse TypeScript-only
 * syntax such as `import type { X } from 'y'`, and fails CLOSED with a fatal parse error rather
 * than a `no-restricted-imports` message. A DENIED row built on that syntax would then report an
 * empty message array for the wrong reason — proving the parser choked, not that the rule passed
 * — and read as a false ALLOW. `tseslint.parser` alone (no `parserOptions.project`) is enough:
 * this only needs to PARSE the syntax, not type-check it.
 */
const TS_PARSER_CONFIG: Linter.Config = { languageOptions: { parser: tseslint.parser } };

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
  const eslint = new ESLint({ cwd: TS_ROOT, overrideConfigFile: true, overrideConfig: [TS_PARSER_CONFIG, ...boundaryRules] });
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
  ['discovery must not import next', 'packages/paigasus-discovery/src/probe.ts', "import { x } from 'next';"],
  ['discovery must not import a next SUBPATH', 'packages/paigasus-discovery/src/probe.ts', "import { after } from 'next/server';"],
  ['discovery must not import the sdk', 'packages/paigasus-discovery/src/probe.ts', "import { x } from '@paigasus/sdk';"],
  ['discovery must not import a sdk SUBPATH', 'packages/paigasus-discovery/src/probe.ts', "import { x } from '@paigasus/sdk/client';"],
  ['apps must not import proto', 'apps/iam-console/app/page.tsx', "import { x } from '@paigasus/proto';"],
  ['apps must not import a proto SUBPATH', 'apps/iam-console/app/page.tsx', "import { x } from '@paigasus/proto/gen/iam';"],
  ['auth/client must not import openid-client', 'packages/paigasus-auth/src/client.ts', "import * as c from 'openid-client';"],
  ['auth/client must not import redis', 'packages/paigasus-auth/src/client.ts', "import { createClient } from 'redis';"],
  ['auth/client must not import a node builtin', 'packages/paigasus-auth/src/client.ts', "import { randomBytes } from 'node:crypto';"],
  ['auth/client must not import a BARE node builtin', 'packages/paigasus-auth/src/client.ts', "import { randomBytes } from 'crypto';"],
  // SIBLING-RELATIVE. src/client.ts reaches src/adapters as './adapters/…', never '../'. A
  // ../-only group is inert on the one file this rule exists to protect.
  ['auth/client must not reach adapters via ./', 'packages/paigasus-auth/src/client.ts', "import { x } from './adapters/redis-store.js';"],
  ['auth/client must not reach core via ./', 'packages/paigasus-auth/src/client.ts', "import { x } from './core/single-flight.js';"],
  // TYPE-ONLY, on purpose. This is the exact import that started the fix-round-1 investigation:
  // `import type` is erased at compile time, but the preset bans type imports alongside value
  // ones everywhere (eslint.mjs:28-31) because a type import still couples the two sides. The
  // fix was to move the shared vocabulary OUT of core/ into ./session-view.js, not to carve a
  // `./core/session.js` exception into this rule — so a type-only reach into core/ must stay
  // rejected, deliberately, rather than by accident.
  ['auth/client must not reach core via ./, even a TYPE-ONLY import', 'packages/paigasus-auth/src/client.ts', "import type { SessionView } from './core/session.js';"],
  // EXTENSION-BEARING. This codebase wrote `.js` on every relative import until SMA-511 (a real
  // file wrote `from './runtime.js'`, never `from './runtime'`); package src/ is extensionless since.
  // no-restricted-imports matches the specifier AS WRITTEN. A bare `'./runtime'` pattern with no `.js`
  // sibling and no glob matched nothing a real file imported then — these four rows are what proved
  // that (fix round 2). The SMA-511 rows below prove that the groups also match the extensionless form.
  ['auth/client must not reach runtime.ts (the composition root)', 'packages/paigasus-auth/src/client.ts', "import { x } from './runtime.js';"],
  ['auth/client must not reach config.ts', 'packages/paigasus-auth/src/client.ts', "import { x } from './config.js';"],
  ['auth/middleware must not import the store', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './adapters/redis-store.js';"],
  ['auth/middleware must not import single-flight', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './core/single-flight.js';"],
  ['auth/middleware must not import the session store port', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './ports/session-store.js';"],
  // Four dead entries survived earlier in this branch because a bare './runtime' did not match
  // the '.js'-suffixed specifier a real file wrote then — these use the `.js` form that real files
  // wrote until SMA-511 (extensionless since), the same lesson the auth/client rows above record.
  ['auth/middleware must not reach the session type module', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './core/session.js';"],
  ['auth/middleware must not reach the http composition-root surface', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './http/routes.js';"],
  ['auth/middleware must not reach runtime.ts (the composition root)', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './runtime.js';"],
  ['auth/middleware must not reach config.ts', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './config.js';"],
  ['an app middleware must not import auth/server', 'apps/iam-console/middleware.ts', "import { getSession } from '@paigasus/auth/server';"],
  ['an app middleware must not import the sdk', 'apps/iam-console/middleware.ts', "import { x } from '@paigasus/sdk';"],
  // SMA-511: package sources are EXTENSIONLESS now (spec § 7.2). The `.js` rows above prove the
  // groups match the old spelling; these prove they match what real files write today.
  ['auth/client must not reach runtime.ts, extensionless', 'packages/paigasus-auth/src/client.ts', "import { x } from './runtime';"],
  ['auth/client must not reach core, extensionless', 'packages/paigasus-auth/src/client.ts', "import { x } from './core/single-flight';"],
  ['auth/middleware must not reach the http composition root, extensionless', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './http/routes';"],
  ['auth/middleware must not reach the session store port, extensionless', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './ports/session-store';"],
  ['auth/middleware must not reach config.ts, extensionless', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './config';"],
  ['auth/server must not reach the client-only surface, extensionless', 'packages/paigasus-auth/src/server.ts', "import { x } from './client';"],
  // Reverse-direction proof for the new `paigasus/boundaries/auth-server` rule (fix round,
  // finding 6): without a `files` glob matching src/server.ts, the two ALLOWED rows below passed
  // vacuously — no rule applied to that path at all, so any import would have reported []. This
  // row proves the new rule actually applies and actually denies something.
  ['auth/server must not reach the client-only surface', 'packages/paigasus-auth/src/server.ts', "import { x } from './client.js';"],
  // SMA-510 — the app-shell ALLOWLIST (spec § 9.1). Within @paigasus/*, src/ may import only ui,
  // auth/client, discovery/types and discovery/client. Each row below is a banned entry, or a
  // subpath of one. Type imports are banned too (the core rule's default).
  ['app-shell must not type-import the sdk', 'packages/paigasus-app-shell/src/header.tsx', "import type { X } from '@paigasus/sdk';"],
  ['app-shell must not import a sdk SUBPATH', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/sdk/errors';"],
  ['app-shell must not import auth/middleware', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/auth/middleware';"],
  ['app-shell must not import a SUBPATH below auth/client', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/auth/client/x';"],
  // The bare roots. The parent negation that makes `auth/client` importable also un-bans the bare
  // name, so a `paths` entry bans it again (Spec issue 4c, measured).
  ['app-shell must not import the bare @paigasus/auth root', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/auth';"],
  ['app-shell must not import the bare @paigasus/discovery root', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/discovery';"],
  ['app-shell must not import discovery/server', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/discovery/server';"],
  ['app-shell must not import discovery/react, even in a test', 'packages/paigasus-app-shell/tests/nav.test.tsx', "import { Capability } from '@paigasus/discovery/react';"],
  ['app-shell must not import a ui SUBPATH', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/ui/button';"],
  ['app-shell must not import next-config outside the fixture', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/next-config';"],
  ['app-shell must not import next-config/runtime', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/next-config/runtime';"],
  ['app-shell must not import proto', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/proto';"],
  ['app-shell must not import kernel', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/kernel';"],
  ['app-shell src must not import its own package name', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/app-shell';"],
  // The fixture exception is narrow: next-config ROOT and the package's own root, nothing more.
  ['the fixture must not import next-config/runtime', 'packages/paigasus-app-shell/tests/e2e/fixture/app/page.tsx', "import { x } from '@paigasus/next-config/runtime';"],
  ['the fixture must not import the sdk', 'packages/paigasus-app-shell/tests/e2e/fixture/app/page.tsx', "import { x } from '@paigasus/sdk';"],
  ['the fixture must not import auth/server', 'packages/paigasus-app-shell/tests/e2e/fixture/app/page.tsx', "import { x } from '@paigasus/auth/server';"],
  ['the fixture must not import discovery/server', 'packages/paigasus-app-shell/tests/e2e/fixture/app/page.tsx', "import { x } from '@paigasus/discovery/server';"],
  ['the fixture must not import an app-shell SUBPATH', 'packages/paigasus-app-shell/tests/e2e/fixture/app/page.tsx', "import { x } from '@paigasus/app-shell/src/zone/resolve';"],
  // SMA-511 spec § 7.4. Next 16 names the middleware file `proxy.ts`. The app-middleware block
  // REPLACES the apps block's options for these files, so it restates the proto ban; these rows
  // prove both halves on both file names.
  ['an app proxy must not import auth/server', 'apps/iam-console/proxy.ts', "import { getSession } from '@paigasus/auth/server';"],
  ['an app proxy must not import the sdk', 'apps/iam-console/proxy.ts', "import { x } from '@paigasus/sdk';"],
  ['an app proxy must not import a sdk SUBPATH', 'apps/iam-console/proxy.ts', "import { x } from '@paigasus/sdk/iam';"],
  ['an app proxy must not import proto — the restated ban', 'apps/iam-console/proxy.ts', "import { x } from '@paigasus/proto';"],
  ['an app proxy must not import a proto SUBPATH', 'apps/iam-console/proxy.ts', "import { x } from '@paigasus/proto/iam';"],
  ['an app middleware must not import proto — the restated ban', 'apps/iam-console/middleware.ts', "import { x } from '@paigasus/proto';"],
  // The test-double exemption is NARROW: only apps/*/tests/support/**.
  ['app lib code must not import proto', 'apps/iam-console/lib/iam.ts', "import { x } from '@paigasus/proto';"],
  ['an app test outside tests/support must not import proto', 'apps/iam-console/tests/unit/errors.test.ts', "import { ErrorInfoSchema } from '@paigasus/proto';"],
];

const ALLOWED: ReadonlyArray<readonly [string, string, string]> = [
  ['ui may import react', 'packages/paigasus-ui/src/button.tsx', "import { useState } from 'react';"],
  ['sdk may import proto', 'packages/paigasus-sdk/src/iam.ts', "import { x } from '@paigasus/proto';"],
  ['sdk may import a proto SUBPATH', 'packages/paigasus-sdk/src/iam.ts', "import { x } from '@paigasus/proto/gen/iam';"],
  ['app-shell may import auth/client', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/auth/client';"],
  ['app-shell may import ui', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/ui';"],
  ['discovery may import proto', 'packages/paigasus-discovery/src/core/state.ts', "import { x } from '@paigasus/proto';"],
  ['apps may import the sdk', 'apps/iam-console/app/page.tsx', "import { x } from '@paigasus/sdk';"],
  ['apps may import ui directly — the deliberate § 7.3 deviation', 'apps/iam-console/app/page.tsx', "import { x } from '@paigasus/ui';"],
  ['apps may import next', 'apps/iam-console/app/page.tsx', "import Link from 'next/link';"],
  ['auth/client may import react', 'packages/paigasus-auth/src/client.ts', "import { createContext } from 'react';"],
  // The fix for the type-only-import finding above: SessionView now lives in a leaf module with
  // no server machinery, one directory level above core/adapters/ports, so client.ts can reach it
  // without a `./core/**`-shaped specifier ever appearing in its import list.
  ['auth/client may import the shared session-view module', 'packages/paigasus-auth/src/client.ts', "import type { SessionView } from './session-view.js';"],
  // Fix round, finding 6: these two rows used to pass VACUOUSLY — no `boundaryRules` entry's
  // `files` glob matched src/server.ts at all, so `restrictedImportsFor` returned [] for ANY
  // import, proving nothing. The new `paigasus/boundaries/auth-server` rule above now covers this
  // path (denying only a reach back into ./client.js — see the DENIED row of the same name), so
  // these rows genuinely exercise "the rule that covers this file does not ban this import."
  ['auth/server may import openid-client', 'packages/paigasus-auth/src/server.ts', "import * as c from 'openid-client';"],
  ['auth/server may reach its own adapters', 'packages/paigasus-auth/src/server.ts', "import { x } from './adapters/redis-store.js';"],
  ['an app middleware may import auth/middleware', 'apps/iam-console/middleware.ts', "import { createAuthMiddleware } from '@paigasus/auth/middleware';"],
  // Proves the finding-5 widening stayed precise: src/middleware.ts's real, legitimate import of
  // cookie NAME constants (ADR-0017 decision 7's cookie-presence check) must keep working — only
  // the composition-root file, './http/routes.js', is banned, not the whole './http/**' directory.
  ['auth/middleware may still import cookie constants', 'packages/paigasus-auth/src/middleware.ts', "import { SESSION_COOKIE } from './http/cookies.js';"],
  // SMA-510 — the four allowed @paigasus/* entries, and `next`.
  ['app-shell may import discovery/types', 'packages/paigasus-app-shell/src/nav/state.ts', "import type { ServiceState } from '@paigasus/discovery/types';"],
  ['app-shell may import discovery/client', 'packages/paigasus-app-shell/src/nav/state.ts', "import { capabilityOutcome } from '@paigasus/discovery/client';"],
  ['app-shell may import next/link', 'packages/paigasus-app-shell/src/zone/zone-link.tsx', "import NextLink from 'next/link';"],
  ['app-shell may import next/navigation', 'packages/paigasus-app-shell/src/nav/primary-nav.tsx', "import { usePathname } from 'next/navigation';"],
  ['the fixture may import the next-config root (its next.config.ts)', 'packages/paigasus-app-shell/tests/e2e/fixture/next.config.ts', "import { createNextConfig } from '@paigasus/next-config';"],
  ['the fixture may import the package by its own name', 'packages/paigasus-app-shell/tests/e2e/fixture/app/page.tsx', "import { ZoneLink } from '@paigasus/app-shell';"],
  ['the fixture may import auth/client', 'packages/paigasus-app-shell/tests/e2e/fixture/app/providers.tsx', "import { SessionProvider } from '@paigasus/auth/client';"],
  ['the fixture may import ui', 'packages/paigasus-app-shell/tests/e2e/fixture/app/page.tsx', "import { Link } from '@paigasus/ui';"],
  // SMA-511 spec § 7.4.
  ['an app proxy may import auth/middleware', 'apps/iam-console/proxy.ts', "import { authRoutePaths, createAuthMiddleware } from '@paigasus/auth/middleware';"],
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
      } else {
        // SMA-510: a scope whose directory EXISTS must say 'exists'. Otherwise a stale "has not
        // landed yet" note keeps passing after the package lands, and nobody re-reads it.
        expect(note, `${dir} exists on disk, so BOUNDARY_SCOPES must say 'exists'`).toBe('exists');
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
    const messages = await realConfigRestrictedImportsFor('apps/iam-console/app/probe.mjs', "import { x } from '@paigasus/proto';\nexport const y = x;\n");
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

  // SMA-511 spec § 7.2. The source rule is a SEPARATE export, so the boundary-entry check above does
  // not see it. Deleting only the spread from ts/eslint.config.js would leave every other test green.
  it('carries every sourceRules entry in its EXPORTED array', async () => {
    const shipped = (await import('../../../eslint.config.js')).default as Array<{ files?: string[]; ignores?: string[] }>;
    for (const entry of sourceRules) {
      expect(shipped, `ts/eslint.config.js dropped the ${entry.name} block`).toContainEqual(expect.objectContaining({ files: entry.files, ignores: entry.ignores }));
    }
  });

  // Lints through the REAL config, so a global `ignores` entry that silences packages/*/src fails
  // here. The path is a REAL, tracked file: the shipped config lints every .ts path with
  // projectService, and a path no tsconfig includes gives one fatal parse error and runs no rule.
  // lintText uses the source given here, not the file on disk.
  it('lints a .js relative specifier in package src through the REAL config', async () => {
    const eslint = new ESLint({ cwd: TS_ROOT });
    const [result] = await eslint.lintText("import { SESSION_VIEW_KEYS } from './core/session.js';\nexport const keys = SESSION_VIEW_KEYS;\n", {
      filePath: 'packages/paigasus-auth/src/session-view.ts',
      warnIgnored: false,
    });
    const messages = result?.messages ?? [];
    expect(messages.filter((m) => m.fatal === true)).toEqual([]);
    expect(messages.filter((m) => m.ruleId === 'paigasus/no-js-relative-specifier')).toHaveLength(1);
  }, 120_000);

  // SMA-511 spec § 7.4 — the app test-double exemption (`ignores: ['apps/*/tests/support/**']`),
  // through the REAL config. Not an ALLOWED row: through `boundaryRules` alone no block matches a
  // tests/support `.ts` path after the `ignores`, so ESLint does not lint it, and an empty result
  // would prove nothing (measured, pre-flight T7.a). The real config lints every `.mjs` path
  // (`js.configs.recommended` has no `files` key). The `isPathIgnored` check proves that the path
  // is linted, so an empty list here means that no rule bans the import.
  const TEST_DOUBLE_PATH = 'apps/iam-console/tests/support/fake-iam.mjs';
  const TEST_DOUBLE_IMPORTS: ReadonlyArray<readonly [string, string]> = [
    ['proto (it builds ErrorInfo details)', "import { ErrorInfoSchema } from '@paigasus/proto';\nexport const y = ErrorInfoSchema;\n"],
    ['a proto SUBPATH', "import { TenancyService } from '@paigasus/proto/iam';\nexport const y = TenancyService;\n"],
  ];

  it.each(TEST_DOUBLE_IMPORTS)('an app test double under tests/support may import %s, through the REAL config', async (_label, source) => {
    const ignored = await new ESLint({ cwd: TS_ROOT }).isPathIgnored(TEST_DOUBLE_PATH);
    expect(ignored, 'the real config does not lint this path, so an empty result would prove nothing').toBe(false);
    expect(await realConfigRestrictedImportsFor(TEST_DOUBLE_PATH, source)).toEqual([]);
  });

  // The DENIED twin: the same import one directory over, through the same config. If the exemption
  // is widened (for example to `apps/*/tests/**`), this case fails.
  it('an app test OUTSIDE tests/support still may not import proto, through the REAL config', async () => {
    const messages = await realConfigRestrictedImportsFor('apps/iam-console/tests/unit/errors.mjs', "import { ErrorInfoSchema } from '@paigasus/proto';\nexport const y = ErrorInfoSchema;\n");
    expect(messages, 'the test-double exemption covers more than apps/*/tests/support/**').not.toHaveLength(0);
  });
});
