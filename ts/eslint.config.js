// SPDX-License-Identifier: Apache-2.0
import { readdirSync } from 'node:fs';
import path from 'node:path';
import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import reactPlugin from '@eslint-react/eslint-plugin';
import reactHooks from 'eslint-plugin-react-hooks';
import jsxA11y from 'eslint-plugin-jsx-a11y';
import nextPlugin from '@next/eslint-plugin-next';
import { boundaryRules, nextAppRules, sourceRules } from '@paigasus/next-config/eslint';

const appsDir = path.join(import.meta.dirname, 'apps');

export default tseslint.config(
  // NOTE: adding 'packages/**' or 'apps/**' here switches every boundary block off for real code.
  // `tests/boundaries.test.ts` lints through THIS file with no config override for that reason.
  { ignores: ['**/dist/**', '**/.next/**', '**/node_modules/**', '**/*.d.ts', '**/generated/**'] },
  js.configs.recommended,
  // Node CLI tooling scripts (e.g. the SMA-406 semantic-release parity helpers under
  // tooling/): plain Node ESM, outside the typed app/library graph. Provide Node globals
  // so `no-undef` doesn't flag `process` etc.
  {
    files: ['tooling/**/*.{js,mjs,cjs}'],
    languageOptions: { globals: { process: 'readonly', console: 'readonly' } },
  },
  // Type-checked rules only on TS files. JS config files (eslint.config.js itself,
  // .prettierrc.js, next.config.ts) without a tsconfig entry would otherwise fail
  // projectService resolution.
  {
    files: ['**/*.{ts,tsx,mts,cts}'],
    extends: [...tseslint.configs.recommendedTypeChecked],
    languageOptions: {
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
  },
  // React-only — glob-scoped to JSX/TSX so non-React libraries don't see these rules
  {
    files: ['**/*.{tsx,jsx}'],
    ...reactPlugin.configs.recommended,
  },
  {
    files: ['**/*.{tsx,jsx}'],
    plugins: { 'react-hooks': reactHooks, 'jsx-a11y': jsxA11y },
    rules: {
      ...reactHooks.configs.recommended.rules,
      ...jsxA11y.configs.recommended.rules,
    },
  },
  /*
   * Next.js rules — ONE BLOCK PER APP, derived from the filesystem (SMA-512).
   *
   * This was a single hardcoded `apps/iam-console/**` block. A second zone app would have shipped
   * with no Next rules at all and nothing would have said so. The blocks are built by
   * nextAppRules in @paigasus/next-config/eslint, which is unit-tested there; this file's job is
   * only to supply the app list. paigasus-next-config-ts:test asserts this spread is still here.
   */
  ...nextAppRules({
    appsDir,
    appNames: readdirSync(appsDir, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name)
      .sort(),
    plugin: nextPlugin,
  }),
  // Package dependency direction (Frontend Architecture Scoping § 6, SMA-502). The rules live in
  // @paigasus/next-config/eslint so they ship with the package that owns the boundary, and are
  // unit-tested there against synthetic paths — including the app-shell and auth scopes, which do
  // not exist on disk yet. paigasus-next-config-ts:test asserts this spread is still here, and
  // that package's moon.yml lists /ts/eslint.config.js among its test inputs so the assertion is
  // reachable on the PR that removes it.
  ...boundaryRules,
  // Source hygiene (SMA-511 spec § 7.2): no `.js` relative specifier in packages/*/src, because
  // Turbopack does not resolve it to a `.ts` file. A SEPARATE export, never a boundaryRules block —
  // see its doc comment. paigasus-next-config-ts:test asserts this spread too.
  ...sourceRules,
);
