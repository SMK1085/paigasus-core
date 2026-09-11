// SPDX-License-Identifier: Apache-2.0
//
// SMA-511 spec § 7.2 — `paigasus/no-js-relative-specifier`. Turbopack in Next 16.3.4 does not map
// './x.js' to './x.ts' (measured, SMA-510). A `.js` relative specifier in a package's src/ therefore
// fails the `next build` of every zone that compiles the package, while vitest, tsc and ESLint all
// accept it. This file proves the rule reports each specifier shape, and that `sourceRules` applies
// it to package sources only.
import { fileURLToPath } from 'node:url';
import { ESLint, RuleTester, type Linter } from 'eslint';
import tseslint from 'typescript-eslint';
import { describe, expect, it } from 'vitest';
import { boundaryRules, noJsRelativeSpecifier, sourceRules } from '../src/eslint.mjs';

// RuleTester looks for global describe/it. This package's vitest config does not set
// `globals: true`, so give it vitest's functions explicitly.
RuleTester.describe = describe;
RuleTester.it = it;

// The TypeScript parser, because `import type` and `export type` are TypeScript-only syntax.
const ruleTester = new RuleTester({ languageOptions: { parser: tseslint.parser } });

const FILE = 'packages/paigasus-auth/src/probe.ts';
const valid = (code: string) => ({ code, filename: FILE });
const invalid = (code: string, specifiers: readonly string[]) => ({
  code,
  filename: FILE,
  errors: specifiers.map((specifier) => ({ messageId: 'jsSpecifier', data: { specifier } })),
});

ruleTester.run('paigasus/no-js-relative-specifier', noJsRelativeSpecifier, {
  valid: [
    valid("import { a } from './a';"),
    valid("import { a } from '../b/c';"),
    valid("import './side-effect';"),
    valid("import type { A } from './types';"),
    valid("export { a } from './a';"),
    valid("export * from './a';"),
    valid('export const x = 1;'),
    valid("const m = import('./a');"),
    valid('declare const name: string;\nconst m = import(name);'),
    // Bare specifiers are package names, not files: Turbopack resolves them through the package.
    valid("import Link from 'next/link';"),
    valid("import { x } from 'some-package/file.js';"),
    // Only `.js` is reported, as the spec states.
    valid("import styles from './styles.css';"),
    valid("import { a } from './a.mjs';"),
    valid("import { a } from './a.jsx';"),
  ],
  invalid: [
    invalid("import { a } from './a.js';", ['./a.js']),
    invalid("import { a } from '../b/c.js';", ['../b/c.js']),
    invalid("import './side-effect.js';", ['./side-effect.js']),
    // A clause-level `import type` is erased before Turbopack resolves anything, but the rule
    // reports it too: one convention for every relative specifier is easier to hold than two.
    invalid("import type { A } from './types.js';", ['./types.js']),
    invalid("export { a } from './a.js';", ['./a.js']),
    invalid("export type { A } from './types.js';", ['./types.js']),
    invalid("export * from './a.js';", ['./a.js']),
    invalid("export * as ns from './a.js';", ['./a.js']),
    invalid("const m = import('./lazy.js');", ['./lazy.js']),
    invalid("import { a } from './a.js';\nimport { b } from '../b.js';", ['./a.js', '../b.js']),
  ],
});

/** ESLint's default parser cannot parse TypeScript; attach the TypeScript parser, no typed rules. */
const TS_PARSER_CONFIG: Linter.Config = { languageOptions: { parser: tseslint.parser } };

/** The ts workspace root — `files` globs in the preset are relative to it. */
const TS_ROOT = fileURLToPath(new URL('../../..', import.meta.url));

const PROBE = "import { a } from './a.js';\nexport const b = a;\n";

async function ruleMessagesFor(filePath: string): Promise<string[]> {
  const eslint = new ESLint({ cwd: TS_ROOT, overrideConfigFile: true, overrideConfig: [TS_PARSER_CONFIG, ...sourceRules] });
  const [result] = await eslint.lintText(PROBE, { filePath, warnIgnored: false });
  return (result?.messages ?? []).filter((m) => m.ruleId === 'paigasus/no-js-relative-specifier').map((m) => m.message);
}

describe('sourceRules scope', () => {
  it.each(['packages/paigasus-auth/src/server.ts', 'packages/paigasus-sdk/src/errors/map-error.ts', 'packages/paigasus-discovery/src/react.tsx', 'packages/paigasus-proto/src/index.mts'])(
    'reports a .js relative specifier in %s',
    async (filePath) => {
      const messages = await ruleMessagesFor(filePath);
      expect(messages).toHaveLength(1);
      expect(messages[0]).toContain("'./a.js'");
    },
  );

  it.each([
    ['a test file inside src/', 'packages/paigasus-proto/src/index.test.ts'],
    ['a file under tests/', 'packages/paigasus-auth/tests/http/login.test.ts'],
    ['a helper under tests/', 'packages/paigasus-auth/tests/support/import-graph.ts'],
    ['the app-shell e2e fixture', 'packages/paigasus-app-shell/tests/e2e/fixture/app/page.tsx'],
    ['an app file (next build reports its own .js specifiers)', 'apps/iam-console/lib/iam.ts'],
    ['a plain .mjs module', 'packages/paigasus-next-config/src/eslint.mjs'],
  ])('does not apply to %s', async (_label, filePath) => {
    expect(await ruleMessagesFor(filePath)).toEqual([]);
  });

  it('is a separate export, not a boundaryRules block', () => {
    // A `packages/*/src` block inside boundaryRules would fail the reverse liveness loop in
    // tests/boundaries.test.ts, which derives a BOUNDARY_SCOPES key from every block's files[0].
    // So assert on the real objects: sourceRules holds exactly the one block with the rule, and no
    // boundaryRules block scopes to packages/*/ or turns the rule on. Move the block into
    // boundaryRules, change its glob, or set the rule to 'warn', and this case fails.
    expect(sourceRules).toHaveLength(1);
    const [block] = sourceRules;
    expect(block?.name).toBe('paigasus/source/no-js-relative-specifier');
    expect(block?.files).toEqual(['packages/*/src/**/*.{ts,tsx,mts,cts}']);
    expect(block?.ignores).toEqual(['**/*.test.*', '**/tests/**']);
    expect(block?.rules).toEqual({ 'paigasus/no-js-relative-specifier': 'error' });
    expect(block?.plugins?.['paigasus']?.rules?.['no-js-relative-specifier']).toBe(noJsRelativeSpecifier);
    expect(boundaryRules.length).toBeGreaterThan(0);
    for (const entry of boundaryRules) {
      expect(String(entry.files?.[0]), `${entry.name} scopes to packages/*/`).not.toMatch(/^packages\/\*\//);
      expect(Object.keys(entry.rules ?? {}), `${entry.name} turns the source rule on`).not.toContain('paigasus/no-js-relative-specifier');
    }
  });
});
