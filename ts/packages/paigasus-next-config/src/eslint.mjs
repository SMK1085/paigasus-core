// SPDX-License-Identifier: Apache-2.0
//
// Package-boundary rules for the ts workspace (Frontend Architecture Scoping § 6, SMA-502).
//
//   apps → app-shell → { ui, auth/client }
//   apps → { sdk, auth/server, next-config }
//   sdk  → proto
//   ui   → React only
//
// DELIBERATE DEVIATION: an app may import @paigasus/ui directly. The diagram shows layering, not
// exclusivity, and forcing every component through an app-shell re-export barrel buys no safety.
// The four rules below are the ones that carry real safety.
//
// EVERY GROUP CARRIES BOTH THE BARE AND THE `/**` FORM, WITH NEGATIONS DOUBLED TOO. An earlier
// revision of this comment claimed `*` does not cross `/`, so the bare form alone would miss a
// subpath — that claim is DISPROVEN. `no-restricted-imports` matches `patterns[].group` through
// the `ignore` package's gitignore-style semantics, which recurse into a matched prefix: '@paigasus/*'
// ALREADY matches '@paigasus/sdk/client' and '@paigasus/proto/gen/iam' on its own. Measured by
// dropping the `/**` variants from the ui and apps groups and re-running tests/boundaries.test.ts —
// every subpath row (including the SUBPATH-labelled ones) stayed green. The `/**` positives are
// kept anyway as belt-and-braces and to document intent, but they are not what makes subpaths get
// caught.
//
// What IS load-bearing is the doubled NEGATION. Also measured: dropping '!@paigasus/proto/**' from
// the sdk group's negation makes 'permits: sdk may import a proto SUBPATH' fail, i.e. it starts
// banning a legitimate import. Do not remove either negation form.
//
// Type imports are banned alongside value imports: an `import type` of @paigasus/sdk from
// @paigasus/ui still couples the packages. That is the core rule's default behaviour, which is
// why @typescript-eslint/no-restricted-imports (whose added feature here is `allowTypeImports`)
// is not used.
//
// Written as plain ESM rather than TypeScript: ts/eslint.config.js is loaded by ESLint's own
// resolver, and configuration data gains little from types (spec § 13 M5).
//
// The app-shell blocks are LIVE since SMA-510. They use the ALLOWLIST form, like the sdk block, and
// a second block carries the fixture's narrow exception. @paigasus/auth has its own scope (the
// `paigasus/boundaries/auth-*` and `paigasus/boundaries/app-middleware` blocks). Every scope is
// covered by the liveness assertions in tests/boundaries.test.ts, so a package that lands under a
// different directory name reds instead of silently disabling its rule.

/**
 * Package directories these rules expect, mapped to a status string. `'exists'` means the
 * directory is on disk today; anything else is the stated reason it is not, which the liveness
 * test requires so an inert rule cannot go unnoticed.
 *
 * @type {Record<string, string>}
 */
export const BOUNDARY_SCOPES = {
  'packages/paigasus-ui': 'exists',
  'packages/paigasus-sdk': 'exists',
  'packages/paigasus-app-shell': 'exists',
  // The second app-shell block's scope. The reverse liveness loop needs every block's scope here.
  'packages/paigasus-app-shell/tests/e2e/fixture': 'exists',
  'packages/paigasus-auth': 'exists',
  'packages/paigasus-discovery': 'exists',
  apps: 'exists',
};

const restrict = (patterns) => ({ 'no-restricted-imports': ['error', { patterns }] });

/**
 * The @paigasus/* allowlist for @paigasus/app-shell (SMA-510 spec § 9.1).
 *
 * THE PARENT NEGATION IS LOAD-BEARING (measured, SMA-510). `no-restricted-imports` matches with
 * gitignore semantics, and gitignore cannot re-include a path whose PARENT is excluded. So
 * `'!@paigasus/auth/client'` alone does nothing: `@paigasus/*` already excluded `@paigasus/auth`.
 * The working form negates the parent, bans its children again, then re-includes the one child.
 */
const APP_SHELL_ALLOWLIST = [
  '@paigasus/*',
  '@paigasus/*/**',
  '!@paigasus/ui',
  '!@paigasus/auth',
  '@paigasus/auth/*',
  '!@paigasus/auth/client',
  '!@paigasus/discovery',
  '@paigasus/discovery/*',
  '!@paigasus/discovery/types',
  '!@paigasus/discovery/client',
];

const APP_SHELL_MESSAGE =
  '@paigasus/app-shell is client-reachable. Within @paigasus/*, it may import only @paigasus/ui, @paigasus/auth/client, @paigasus/discovery/types and @paigasus/discovery/client (SMA-510 spec § 9.1; § 6 rule 3).';

/**
 * The parent negations above also un-ban the BARE roots `@paigasus/auth` and `@paigasus/discovery`.
 * Neither package has a root export, but the rule must still say no, so `paths` (exact names) bans
 * them again. Measured: every allowed specifier stays allowed.
 */
const APP_SHELL_BARE_ROOTS = [
  { name: '@paigasus/auth', message: `${APP_SHELL_MESSAGE} @paigasus/auth has no root export.` },
  { name: '@paigasus/discovery', message: `${APP_SHELL_MESSAGE} @paigasus/discovery has no root export.` },
];

/**
 * The boundary blocks, as an ESLint flat-config array.
 *
 * The `@type` annotation is load-bearing for the consumer, not decoration: `tests/boundaries.test.ts`
 * is TypeScript with `noUncheckedIndexedAccess` and the typed-ESLint `no-unsafe-*` rules on, and an
 * unannotated export from a `.mjs` gives it loosely-inferred types that trip those rules. Fix the
 * typing here rather than adding an eslint-disable in the one test that proves these rules are wired.
 *
 * Annotated with ESLint's OWN exported config type rather than a hand-rolled shape: a
 * `rules: Record<string, unknown>` approximation is not assignable to ESLint's real
 * `rules?: Partial<RulesConfig>` (`unknown` is not a `RuleConfig`), which is exactly what
 * `paigasus-next-config-ts:typecheck` caught. Consumers get ESLint's real typings this way — a
 * hand-rolled shape can diverge from them again on any future ESLint upgrade.
 *
 * @type {import('eslint').Linter.Config[]}
 */
export const boundaryRules = [
  {
    name: 'paigasus/boundaries/ui',
    files: ['packages/paigasus-ui/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    rules: restrict([
      {
        group: ['next', 'next/*', 'next/**'],
        message:
          '@paigasus/ui is plain React and must not import next/* — it has to test in jsdom with no Next runtime and stay usable from the docs app (§ 6 rule 1). Inject navigation instead of reaching for next/link.',
      },
      {
        group: ['@paigasus/*', '@paigasus/*/**'],
        message: '@paigasus/ui depends on React only. Nothing in the workspace may be imported here (§ 6 rule 1).',
      },
    ]),
  },
  {
    name: 'paigasus/boundaries/sdk',
    files: ['packages/paigasus-sdk/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    rules: restrict([
      {
        group: ['@paigasus/*', '@paigasus/*/**', '!@paigasus/proto', '!@paigasus/proto/**'],
        message: '@paigasus/sdk depends on @paigasus/proto only (§ 6: sdk → proto).',
      },
      {
        group: ['react', 'react-dom', 'react-dom/*', 'next', 'next/*', 'next/**'],
        message: '@paigasus/sdk is server-only. It attaches bearer tokens, so it must never be reachable from a client bundle (§ 6 rule 2).',
      },
    ]),
  },
  {
    name: 'paigasus/boundaries/app-shell',
    files: ['packages/paigasus-app-shell/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    rules: {
      'no-restricted-imports': ['error', { paths: APP_SHELL_BARE_ROOTS, patterns: [{ group: APP_SHELL_ALLOWLIST, message: APP_SHELL_MESSAGE }] }],
    },
  },
  {
    // The fixture's one exception (spec § 9.1): its next.config.ts imports the next-config ROOT, and
    // its pages import the package by its OWN name (Spec issue 4a). A later flat-config block
    // REPLACES the rule's options for a file, it does not merge them, so this block restates the
    // whole allowlist and adds the two roots. Their subpaths stay banned.
    name: 'paigasus/boundaries/app-shell-fixture',
    files: ['packages/paigasus-app-shell/tests/e2e/fixture/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    rules: {
      'no-restricted-imports': [
        'error',
        {
          paths: APP_SHELL_BARE_ROOTS,
          patterns: [
            {
              group: [...APP_SHELL_ALLOWLIST, '!@paigasus/next-config', '@paigasus/next-config/*', '!@paigasus/app-shell', '@paigasus/app-shell/*'],
              message: `${APP_SHELL_MESSAGE} The fixture may also import the @paigasus/next-config root and @paigasus/app-shell itself, and no subpath of either.`,
            },
          ],
        },
      ],
    },
  },
  {
    name: 'paigasus/boundaries/discovery',
    files: ['packages/paigasus-discovery/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    rules: restrict([
      {
        group: ['next', 'next/*', 'next/**'],
        message: '@paigasus/discovery must not import from `next`. Next primitives are INJECTED — `waitUntil` takes `after` as a parameter — so the package tests without a Next runtime (§ 8.1).',
      },
      {
        group: ['@paigasus/sdk', '@paigasus/sdk/**'],
        message:
          '@paigasus/discovery must not depend on @paigasus/sdk: its `Presentation` union cannot express a DegradedReason, and a transitive edge would give @paigasus/app-shell a path to the sdk that `paigasus/boundaries/app-shell` bans (spec F8, § 2.1).',
      },
    ]),
  },
  {
    name: 'paigasus/boundaries/apps',
    files: ['apps/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    rules: restrict([
      {
        group: ['@paigasus/proto', '@paigasus/proto/**'],
        message: 'Apps reach the contract through @paigasus/sdk, never @paigasus/proto directly (§ 6).',
      },
    ]),
  },
  {
    name: 'paigasus/boundaries/auth-client',
    // `files[0]` MUST begin `packages/paigasus-auth/**` — the liveness test derives its scope key
    // by cutting at the first `/**`, and a narrower first entry (e.g. one scoped to `src/client`)
    // would derive a key the reverse loop can never pair with a real package name.
    files: ['packages/paigasus-auth/**/client.ts', 'packages/paigasus-auth/**/client.tsx', 'packages/paigasus-auth/src/client/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    rules: restrict([
      {
        group: [
          'openid-client',
          'redis',
          'server-only',
          'node:*',
          'crypto',
          'fs',
          'net',
          'tls',
          'http',
          'https',
          'stream',
          'buffer',
          './adapters/**',
          './core/**',
          './ports/**',
          './http/**',
          './next/**',
          './runtime',
          './runtime.js',
          './config',
          './config.js',
          '../adapters/**',
          '../core/**',
          '../ports/**',
          '../http/**',
          '../next/**',
        ],
        message:
          '@paigasus/auth/client is React-only and must never reach the server surface — it would put a token in a browser bundle (AC 5). Import the shared vocabulary from ./session-view.js only.',
      },
    ]),
  },
  {
    name: 'paigasus/boundaries/auth-middleware',
    files: ['packages/paigasus-auth/**/middleware.ts'],
    rules: restrict([
      {
        group: [
          'openid-client',
          'redis',
          './adapters/**',
          './core/single-flight',
          './core/single-flight.js',
          './core/session',
          './core/session.js',
          './ports/session-store',
          './ports/session-store.js',
          './next/**',
          // NOT './http/**' — src/middleware.ts legitimately imports './http/cookies.js' for
          // the cookie-presence check ADR-0017 decision 7 actually authorizes. What must stay
          // banned is the composition-root surface, './http/routes.js', which pulls in the full
          // session-resolution machinery (openid-client, the store) that middleware must never
          // reach.
          './http/routes',
          './http/routes.js',
          './runtime',
          './runtime.js',
          './config',
          './config.js',
          '../adapters/**',
          '../ports/**',
          '../http/routes',
          '../http/routes.js',
        ],
        message: 'Next middleware does cookie-presence checks only (ADR-0017 decision 7; CVE-2025-29927 was a middleware auth bypass). Resolve the session in a server component or route handler.',
      },
    ]),
  },
  {
    // Mirrors `paigasus/boundaries/auth-client` in reverse: that rule stops the client surface
    // reaching the server surface (AC 5); this one stops the server composition root reaching
    // back into the client-only module, which would be the same coupling from the other side and
    // a path for `react` to leak into node-only code.
    name: 'paigasus/boundaries/auth-server',
    files: ['packages/paigasus-auth/**/server.ts'],
    rules: restrict([
      {
        group: ['./client', './client.js'],
        message: '@paigasus/auth/server is the server composition root and must never reach the client-only surface (the reverse of AC 5).',
      },
    ]),
  },
  {
    name: 'paigasus/boundaries/app-middleware',
    // 'apps/**/middleware…' rather than 'apps/*/middleware…': the latter derives the scope key
    // 'apps/*/middleware.{ts,js,mts,cts,mjs,cjs}' (a file glob, not a directory), which the
    // liveness test's `existsSync` check can never resolve. This form derives 'apps' instead —
    // the SAME key the `paigasus/boundaries/apps` block above already owns in BOUNDARY_SCOPES —
    // so it needs no scope entry of its own.
    files: ['apps/**/middleware.{ts,js,mts,cts,mjs,cjs}'],
    rules: restrict([
      {
        group: ['@paigasus/auth/server', '@paigasus/auth/server/**', '@paigasus/sdk', '@paigasus/sdk/**'],
        message:
          "An app's middleware must import @paigasus/auth/middleware, never /server or the sdk. `server-only` is a NO-OP in the middleware layer, so nothing else stops a token-bearing module being bundled there.",
      },
    ]),
  },
];

export default boundaryRules;
