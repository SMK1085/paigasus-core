// SPDX-License-Identifier: Apache-2.0
//
// SMA-725 spec § 4.2 and § 6.2 — `paigasus/no-single-field-prn-accessor`. The six single-field PRN
// accessors of @paigasus/kernel are deprecated: each parses the PRN again, and `prnParse` returns
// every field in one kernel call. This file proves which import forms the rule reports and which
// it allows. `kernel-accessor-rules.test.ts` proves where the workspace config applies it.
import { RuleTester } from 'eslint';
import tseslint from 'typescript-eslint';
import { describe, it } from 'vitest';
import { noSingleFieldPrnAccessor } from '../src/eslint.mjs';

// RuleTester looks for global describe/it. This package's vitest config does not set
// `globals: true`, so give it vitest's functions explicitly.
RuleTester.describe = describe;
RuleTester.it = it;

// The TypeScript parser, because `import type` and type annotations are TypeScript-only syntax.
const ruleTester = new RuleTester({ languageOptions: { parser: tseslint.parser } });
// The default parser, for the plain-ESM case: the rule needs no TypeScript.
const jsRuleTester = new RuleTester();

const FILE = 'apps/iam-console/lib/probe.ts';
const ACCESSORS = ['prnErrorKind', 'prnService', 'prnRegion', 'prnOrg', 'prnResourceType', 'prnResourceId'] as const;
const SPECIFIERS = ['@paigasus/kernel', '@paigasus/kernel/napi', '@paigasus/node-bindings', '@paigasus/wasm'] as const;

const valid = (code: string) => ({ code, filename: FILE });
const invalid = (code: string, names: readonly string[]) => ({
  code,
  filename: FILE,
  errors: names.map((name) => ({
    // ESLint 10 RuleTester rejects `message` together with `messageId`; `message` checks the exact text.
    message: `'${name}' is deprecated (SMA-725). Use prnParse(prn): one kernel call returns every field or the error kind.`,
  })),
});

ruleTester.run('paigasus/no-single-field-prn-accessor', noSingleFieldPrnAccessor, {
  valid: [
    // The replacement and the other kernel exports, from every watched specifier.
    ...SPECIFIERS.flatMap((s) => [
      valid(`import { prnParse } from '${s}';`),
      valid(`import { prnBuild } from '${s}';`),
      valid(`import { prnCanonicalize } from '${s}';`),
    ]),
    // D4: a namespace import is allowed, and so is a use of a name that is not deprecated.
    valid("import * as k from '@paigasus/kernel';\nk.prnParse('p');"),
    valid("export * from '@paigasus/kernel';"),
    valid("export * as k from '@paigasus/kernel';"),
    // L1: a dynamic import is not checked.
    valid("const m = import('@paigasus/kernel');"),
    // Not a watched module.
    valid("import { prnService } from './local';"),
    valid("import { prnService } from '@paigasus/kernelx';"),
    valid("import { prnService } from '@paigasus/kernel/other';"),
    // A local `k` that is not a namespace import.
    valid("const k = { prnService: () => '' };\nk.prnService();"),
    // Review Focus 1: a parameter shadows the namespace import, so its member is not the kernel's.
    valid("import * as k from '@paigasus/kernel';\nfunction f(k: { prnService(): void }) { k.prnService(); }\nk.prnParse('p');"),
    // A computed member with a non-literal key cannot be judged, even when the VARIABLE is named
    // like an accessor: its value is what counts, not its name.
    valid("import * as k from '@paigasus/kernel';\ndeclare const n: string;\nk[n];"),
    valid("import * as k from '@paigasus/kernel';\nconst prnService = 'prnParse';\nk[prnService];"),
  ],
  invalid: [
    // Each of the six names, from each of the four exact specifiers (24 cases).
    ...SPECIFIERS.flatMap((s) => ACCESSORS.map((name) => invalid(`import { ${name} } from '${s}';`, [name]))),
    // Deep paths into the two raw binding packages (they have no `exports` map).
    invalid("import { prnService } from '@paigasus/wasm/paigasus_wasm.js';", ['prnService']),
    invalid("import { prnOrg } from '@paigasus/node-bindings/index.js';", ['prnOrg']),
    // Aliased, type-only and string-literal names (Review Focus 2).
    invalid("import { prnOrg as org } from '@paigasus/kernel';", ['prnOrg']),
    invalid("import type { prnRegion } from '@paigasus/kernel';", ['prnRegion']),
    invalid("import { 'prnService' as s } from '@paigasus/kernel';", ['prnService']),
    // Re-exports, plain and renamed (Review Focus 3).
    invalid("export { prnService } from '@paigasus/kernel';", ['prnService']),
    invalid("export { prnService as service } from '@paigasus/kernel';", ['prnService']),
    // Several names in one statement give one report each.
    invalid("import { prnParse, prnService, prnOrg } from '@paigasus/kernel';", ['prnService', 'prnOrg']),
    // D4: a use through a namespace import.
    invalid("import * as k from '@paigasus/kernel';\nk.prnService('p');", ['prnService']),
    invalid("import * as k from '@paigasus/kernel';\nk['prnOrg']('p');", ['prnOrg']),
    invalid("import * as k from '@paigasus/kernel';\nconst { prnRegion } = k;", ['prnRegion']),
    invalid("import * as k from '@paigasus/kernel';\nconst { prnResourceId: id, prnParse } = k;", ['prnResourceId']),
  ],
});

jsRuleTester.run('paigasus/no-single-field-prn-accessor (plain ESM)', noSingleFieldPrnAccessor, {
  valid: [{ code: "import { prnParse } from '@paigasus/kernel';", filename: 'packages/paigasus-kernel/tests/probe.mjs' }],
  invalid: [
    {
      code: "import { prnService } from '@paigasus/kernel';",
      filename: 'packages/paigasus-kernel/tests/probe.mjs',
      errors: [{ messageId: 'deprecatedAccessor', data: { name: 'prnService' } }],
    },
  ],
});
