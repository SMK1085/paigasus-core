// SPDX-License-Identifier: Apache-2.0
//
// SMA-725 spec § 6.3 — where the REAL ts/eslint.config.js applies
// `paigasus/no-single-field-prn-accessor`, and that it does not displace the boundary rules.
// `no-single-field-prn-accessor.test.ts` proves what the rule reports.
import { fileURLToPath } from 'node:url';
import { ESLint, type Linter } from 'eslint';
import { describe, expect, it } from 'vitest';
import { kernelAccessorRules, noSingleFieldPrnAccessor } from '../src/eslint.mjs';

/** The ts workspace root — `files` globs in the preset are relative to it. */
const TS_ROOT = fileURLToPath(new URL('../../..', import.meta.url));
const RULE = 'paigasus/no-single-field-prn-accessor';
/**
 * A REAL, tracked, tsconfig-included, non-exempt TS file. The shipped config lints every TS path
 * with projectService, and a path that no tsconfig includes gives one fatal parse error and runs
 * no rule. lintText uses the source given here, not the file on disk.
 */
const LIVE_PATH = 'packages/paigasus-console-core/src/prn-tenancy.ts';
/**
 * 120 s: this boots a real ESLint against the SHIPPED flat config with typed linting. The first
 * call pays the whole config-resolution cost, and a cold CI runner took 9.7 s on a lighter row.
 */
const REAL_CONFIG_TIMEOUT = 120_000;

const EXEMPT_PATHS = [
  'packages/paigasus-kernel/src/wasm.ts',
  'packages/paigasus-kernel/src/index.ts',
  'packages/paigasus-kernel/tests/prn-fields.test.ts',
  'packages/paigasus-kernel/tests/prn-fields.wasm.test.ts',
  'packages/paigasus-kernel/tests/prn-canonical.test.ts',
  'packages/paigasus-kernel/tests/prn-canonical.wasm.test.ts',
  'packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts',
] as const;

/** A non-exempt file beside each exempt group (spec A5). */
const COVERED_PATHS = [
  'packages/paigasus-kernel/tests/prn-parse.test.ts',
  'packages/paigasus-kernel/tests/deprecated-accessors.test.ts',
  'packages/paigasus-kernel/tests/wasm-probe.mjs',
  'packages/paigasus-console-core/tests/unit/prn-tenancy.test.ts',
  'packages/paigasus-console-core/src/prn-tenancy.ts',
  'apps/iam-console/lib/iam.ts',
] as const;

/** ESLint normalises a configured severity to a number or a [number, …options] array. */
function severity(entry: Linter.RuleEntry | undefined): number | undefined {
  const level = Array.isArray(entry) ? entry[0] : entry;
  if (level === 'error') return 2;
  if (level === 'warn') return 1;
  if (level === 'off') return 0;
  return level;
}

async function realConfigFor(filePath: string): Promise<Linter.Config | undefined> {
  return (await new ESLint({ cwd: TS_ROOT }).calculateConfigForFile(filePath)) as Linter.Config | undefined;
}

describe('kernelAccessorRules', () => {
  it('is one block that turns the rule on with its own plugin, and never configures no-restricted-imports', () => {
    expect(kernelAccessorRules).toHaveLength(1);
    const [block] = kernelAccessorRules;
    expect(block?.name).toBe('paigasus/kernel/no-single-field-prn-accessor');
    expect(block?.files).toEqual(['**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}']);
    expect(block?.ignores).toEqual([
      'packages/paigasus-kernel/src/**',
      'packages/paigasus-kernel/tests/prn-fields.test.ts',
      'packages/paigasus-kernel/tests/prn-fields.wasm.test.ts',
      'packages/paigasus-kernel/tests/prn-canonical.test.ts',
      'packages/paigasus-kernel/tests/prn-canonical.wasm.test.ts',
      'packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts',
    ]);
    expect(block?.rules).toEqual({ [RULE]: 'error' });
    expect(block?.plugins?.['paigasus']?.rules?.['no-single-field-prn-accessor']).toBe(noSingleFieldPrnAccessor);
    // spec D3 / A7: a second core `no-restricted-imports` block REPLACES the boundary options.
    for (const entry of kernelAccessorRules) {
      expect(Object.keys(entry.rules ?? {})).not.toContain('no-restricted-imports');
      expect(Object.keys(entry.rules ?? {})).not.toContain('@typescript-eslint/no-restricted-imports');
    }
  });
});

describe('the workspace eslint config applies kernelAccessorRules', () => {
  // A dead `import` satisfies a grep; only the exported array proves the spread. The first import
  // of the shipped config took more than 5 s on a CI runner (PR 396), so it gets the same timeout.
  it(
    'carries every kernelAccessorRules entry in its EXPORTED array',
    async () => {
      const shipped = (await import('../../../eslint.config.js')).default as Array<{ files?: string[]; ignores?: string[] }>;
      for (const entry of kernelAccessorRules) {
        expect(shipped, `ts/eslint.config.js dropped the ${entry.name} block`).toContainEqual(expect.objectContaining({ files: entry.files, ignores: entry.ignores }));
      }
    },
    REAL_CONFIG_TIMEOUT,
  );

  // Review Focus 5: assert the config EXISTS before asserting the rule is absent. An undefined
  // config means ESLint does not lint the file at all, which would prove nothing.
  it.each(EXEMPT_PATHS)(
    'does not apply the rule to the exempt path %s',
    async (filePath) => {
      const config = await realConfigFor(filePath);
      expect(config, `the real config does not lint ${filePath}`).toBeDefined();
      expect(config?.rules?.[RULE]).toBeUndefined();
    },
    REAL_CONFIG_TIMEOUT,
  );

  it.each(COVERED_PATHS)(
    'applies the rule as an error to %s',
    async (filePath) => {
      const config = await realConfigFor(filePath);
      expect(config, `the real config does not lint ${filePath}`).toBeDefined();
      expect(severity(config?.rules?.[RULE])).toBe(2);
    },
    REAL_CONFIG_TIMEOUT,
  );

  it(
    'reports a deprecated accessor import in package src through the REAL config',
    async () => {
      const eslint = new ESLint({ cwd: TS_ROOT });
      const [result] = await eslint.lintText("import { prnService } from '@paigasus/kernel';\nexport const service = prnService;\n", { filePath: LIVE_PATH, warnIgnored: false });
      const messages = result?.messages ?? [];
      expect(messages.filter((m) => m.fatal === true)).toEqual([]);
      expect(messages.filter((m) => m.ruleId === RULE).map((m) => m.message)).toEqual([
        "'prnService' is deprecated (SMA-725). Use prnParse(prn): one kernel call returns every field or the error kind.",
      ]);
    },
    REAL_CONFIG_TIMEOUT,
  );

  // spec A7. The boundary rows in boundaries.test.ts lint `.mjs` paths only. A block that replaced
  // the boundary options on TS files would leave them green, so this row lints a TS file.
  it(
    'keeps the console-core boundary rule on the same TS file through the REAL config',
    async () => {
      const eslint = new ESLint({ cwd: TS_ROOT });
      const [result] = await eslint.lintText("import { x } from '@paigasus/proto';\nexport const y = x;\n", { filePath: LIVE_PATH, warnIgnored: false });
      const messages = result?.messages ?? [];
      expect(messages.filter((m) => m.fatal === true)).toEqual([]);
      expect(messages.filter((m) => m.ruleId === 'no-restricted-imports')).not.toHaveLength(0);
    },
    REAL_CONFIG_TIMEOUT,
  );
});
