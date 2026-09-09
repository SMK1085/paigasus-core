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
// The app-shell BLOCK is INERT until SMA-510 lands: `packages/paigasus-app-shell` does not exist
// yet, so its files glob matches nothing. @paigasus/auth now has its own scope, added below —
// see the three `paigasus/boundaries/auth-*` and `paigasus/boundaries/app-middleware` blocks — so
// it is no longer only a DENIED TARGET inside the app-shell rule, and gains real coverage now that
// SMA-506 lands. The app-shell block is written now, tested against synthetic paths, and covered
// by a liveness assertion so a package landing under a different directory name reds instead of
// silently disabling its rule.

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
  'packages/paigasus-app-shell': 'SMA-510 has not landed yet; the rule is inert until it does',
  'packages/paigasus-auth': 'exists',
  apps: 'exists',
};

const restrict = (patterns) => ({ 'no-restricted-imports': ['error', { patterns }] });

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
    rules: restrict([
      {
        group: ['@paigasus/sdk', '@paigasus/sdk/**', '@paigasus/auth/server', '@paigasus/auth/server/**'],
        message: '@paigasus/app-shell is client-reachable. It may use @paigasus/auth/client, never /server, and never the sdk (§ 6 rule 3).',
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
          './config',
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
        group: ['openid-client', 'redis', './adapters/**', './core/single-flight', './ports/session-store', './next/**', '../adapters/**', '../ports/**'],
        message: 'Next middleware does cookie-presence checks only (ADR-0017 decision 7; CVE-2025-29927 was a middleware auth bypass). Resolve the session in a server component or route handler.',
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
