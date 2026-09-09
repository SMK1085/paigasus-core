// SPDX-License-Identifier: Apache-2.0
import { defineConfig, type Plugin } from 'vitest/config';

/**
 * AC 1 (SMA-503, ADR-0021 decision 3): @paigasus/ui imports nothing from `next/*`.
 *
 * pnpm's isolated node_modules already makes `next` unresolvable from this package, because
 * next is a dependency of @paigasus/console and not of this one. That protection is real but
 * INCIDENTAL — adding next to this package's devDependencies for any reason would disarm it
 * silently, with no red anywhere. This plugin does not depend on where pnpm places a package.
 *
 * It cannot see `import type { Metadata } from 'next'`, which verbatimModuleSyntax erases
 * before Vite resolves anything. SMA-502's eslint boundary rule is what covers that.
 *
 * tests/no-next-imports.test.ts is the negative control. Without it this plugin is untested
 * code that could stop matching and never red.
 */
function banNextImports(): Plugin {
  return {
    name: 'paigasus-ban-next-imports',
    enforce: 'pre',
    resolveId(source: string) {
      if (source === 'next' || source.startsWith('next/')) {
        throw new Error(`@paigasus/ui must not import from \`next\`: got "${source}" (ADR-0021 decision 3)`);
      }
      return null;
    },
  };
}

export default defineConfig({
  plugins: [banNextImports()],
  test: {
    name: 'ui',
    environment: 'jsdom',
    setupFiles: ['./tests/setup.ts'],
    include: ['tests/**/*.test.{ts,tsx}'],
    // React Testing Library registers its automatic cleanup only when a global afterEach
    // exists, and vitest defaults globals to false. Without this the DOM accumulates between
    // tests and axe then reports duplicate-id and duplicate-landmark violations belonging to
    // an EARLIER test — a flake that reads as a real accessibility failure.
    globals: true,
  },
});
