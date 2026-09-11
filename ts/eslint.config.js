// SPDX-License-Identifier: Apache-2.0
import path from 'node:path';
import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import reactPlugin from '@eslint-react/eslint-plugin';
import reactHooks from 'eslint-plugin-react-hooks';
import jsxA11y from 'eslint-plugin-jsx-a11y';
import nextPlugin from '@next/eslint-plugin-next';
import { boundaryRules } from '@paigasus/next-config/eslint';

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
  // Next.js rules — scoped to the console app only. Lifted from a per-project
  // eslint.config.js so the workspace-level `moon run ts:lint` task enforces
  // Next.js rules too (the per-project task alone wasn't a complete CI gate).
  // `settings.next.rootDir` is required so `no-html-link-for-pages` resolves
  // the App Router at apps/iam-console/app/ rather than searching
  // the cwd (ts/ or ts/apps/iam-console/ depending on invocation).
  // Using an absolute path anchored to import.meta.dirname makes it
  // cwd-independent.
  {
    files: ['apps/iam-console/**/*.{ts,tsx}'],
    settings: {
      next: { rootDir: path.join(import.meta.dirname, 'apps/iam-console') },
    },
    plugins: { '@next/next': nextPlugin },
    rules: { ...nextPlugin.configs.recommended.rules },
  },
  // Package dependency direction (Frontend Architecture Scoping § 6, SMA-502). The rules live in
  // @paigasus/next-config/eslint so they ship with the package that owns the boundary, and are
  // unit-tested there against synthetic paths — including the app-shell and auth scopes, which do
  // not exist on disk yet. paigasus-next-config-ts:test asserts this spread is still here, and
  // that package's moon.yml lists /ts/eslint.config.js among its test inputs so the assertion is
  // reachable on the PR that removes it.
  ...boundaryRules,
);
