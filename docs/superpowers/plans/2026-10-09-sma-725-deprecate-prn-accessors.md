<!-- moon-diagnosis:ok -->
<!-- The marker above is for check 12 of repo:actionlint: Task 4 names the ciReport token when it
quotes Step 0 of the moon-diagnosis procedure in CLAUDE.md, so this file needs the marker. -->
# SMA-725 Deprecate the Six Single-Field PRN Accessors — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mark the six single-field PRN accessors of `@paigasus/kernel` as `@deprecated`, and stop new callers with a custom ESLint rule.

**Architecture:** The two kernel entry files re-export each accessor as a JSDoc-tagged `const` (no Rust change). A new custom rule `paigasus/no-single-field-prn-accessor` lives in the existing `paigasusPlugin` of `@paigasus/next-config/eslint`. It ships as a separate flat-config export, `kernelAccessorRules`, which `ts/eslint.config.js` spreads.

**Tech Stack:** TypeScript 6.0.3 (compiler API), ESLint 10.12.0 flat config, `typescript-eslint` 8.71.0 (parser only), vitest 5, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-10-09-sma-725-deprecate-prn-accessors-design.md` (read it first; this plan argues from it).

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Commits: conventional, with a workspace scope, for example `feat(ts): …`, `test(ts): …`. End each message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never use `--no-verify`. Never `--amend` a commit you did not make in this task.
- Do NOT change anything under `rs/`. The napi glue and the five wasm artifacts must stay byte-identical (spec A9).
- Do NOT use the core `no-restricted-imports` rule or `@typescript-eslint/no-restricted-imports` anywhere in this change (spec D3).
- The six names, exactly: `prnErrorKind`, `prnService`, `prnRegion`, `prnOrg`, `prnResourceType`, `prnResourceId`.
- The four exact watched specifiers: `@paigasus/kernel`, `@paigasus/kernel/napi`, `@paigasus/node-bindings`, `@paigasus/wasm`. The two watched prefixes: `@paigasus/node-bindings/`, `@paigasus/wasm/`.
- The rule message (exact): `'{{name}}' is deprecated (SMA-725). Use prnParse(prn): one kernel call returns every field or the error kind.`
- Every shell needs the proto PATH first: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Run every command from the worktree root `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-725` unless a step says otherwise.
- Do not install host software (no `brew install`). Do not start a background job and then wait for it.
- Write comments and docs in ASD-STE100 Simplified Technical English: short sentences, active voice.

## Review Focus

1. **A local variable that has the same name as a namespace import.** `function f(k) { k.prnService() }` inside a file that also has `import * as k from '@paigasus/kernel'` must NOT be reported, because the parameter shadows the import. Task 2 has a `valid` case for this.
2. **A string-literal import name.** `import { 'prnService' as s } from '@paigasus/kernel'` is valid ES2022 syntax. The rule must report it. Task 2 has an `invalid` case for this.
3. **A rename on a re-export.** `export { prnService as service } from '@paigasus/kernel'` must be reported under the name `prnService`, not `service`. Task 2 has an `invalid` case for this.
4. **A probe that does not resolve.** If the language-service probe in Task 1 cannot resolve `@paigasus/kernel`, it gets 2307 and zero 6385, and a careless assertion passes. Task 1 asserts zero semantic diagnostics before it counts 6385.
5. **An exempt path that ESLint does not lint at all.** An "allowed" result for a file that no block matches proves nothing. Task 3 uses `calculateConfigForFile` and asserts that the config is defined before it asserts that the rule is absent.

---

### Task 1: The `@deprecated` tag on both kernel entries

**Files:**
- Create: `ts/packages/paigasus-kernel/tests/deprecated-accessors.test.ts`
- Modify: `ts/packages/paigasus-kernel/vitest.config.ts` (the `node` project's `include` list, lines 31-42)
- Modify: `ts/packages/paigasus-kernel/src/index.ts`
- Modify: `ts/packages/paigasus-kernel/src/wasm.ts`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: the six names stay exported from both entries with the same types. Later tasks only depend on the names.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-kernel/tests/deprecated-accessors.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-725 spec § 6.1. The six single-field PRN accessors carry a `@deprecated` tag that names
// `prnParse`, and a consumer sees it through BOTH package entries. The test asks the TypeScript
// language service, as an editor does, so it proves what a caller sees, not what a file contains.
//
// The probe files exist only in the language-service host. They sit INSIDE this package, so
// package self-reference (`name` + `exports`) resolves `@paigasus/kernel`: ts/node_modules has no
// copy of this package. The compiler options are written here, not read from a tsconfig, so this
// test needs no tsconfig in the Moon task inputs.
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const KERNEL_ROOT = fileURLToPath(new URL('..', import.meta.url));
const ACCESSORS = ['prnErrorKind', 'prnService', 'prnRegion', 'prnOrg', 'prnResourceType', 'prnResourceId'] as const;
/** TypeScript diagnostic 6385: "'{0}' is deprecated." */
const DEPRECATED = 6385;
/** A cold language service can go past vitest's 5 s default in CI (SMA-725 spec § 6.1). */
const TIMEOUT = 30_000;

const ENTRIES = [
  { specifier: '@paigasus/kernel', probe: 'tests/__probe_wasm__.ts', source: 'src/wasm.ts' },
  { specifier: '@paigasus/kernel/napi', probe: 'tests/__probe_napi__.ts', source: 'src/index.ts' },
] as const;

const OPTIONS: ts.CompilerOptions = {
  module: ts.ModuleKind.ESNext,
  moduleResolution: ts.ModuleResolutionKind.Bundler,
  target: ts.ScriptTarget.ES2022,
  strict: true,
  noEmit: true,
  skipLibCheck: true,
};

/** Import statements only: one per accessor, then `prnParse`. A use site would add more 6385s. */
function probeSource(specifier: string): string {
  return [...ACCESSORS, 'prnParse'].map((name) => `import { ${name} } from '${specifier}';`).join('\n') + '\n';
}

const files = new Map<string, string>(ENTRIES.map((e) => [path.join(KERNEL_ROOT, e.probe), probeSource(e.specifier)]));

const host: ts.LanguageServiceHost = {
  getScriptFileNames: () => [...files.keys()],
  getScriptVersion: () => '1',
  getScriptSnapshot: (fileName) => {
    const text = files.get(fileName) ?? ts.sys.readFile(fileName);
    return text === undefined ? undefined : ts.ScriptSnapshot.fromString(text);
  },
  getCurrentDirectory: () => KERNEL_ROOT,
  getCompilationSettings: () => OPTIONS,
  getDefaultLibFileName: (options) => ts.getDefaultLibFilePath(options),
  fileExists: (fileName) => files.has(fileName) || ts.sys.fileExists(fileName),
  readFile: (fileName) => files.get(fileName) ?? ts.sys.readFile(fileName),
  readDirectory: (...args) => ts.sys.readDirectory(...args),
  directoryExists: (dir) => ts.sys.directoryExists(dir),
  getDirectories: (dir) => ts.sys.getDirectories(dir),
  realpath: (p) => (ts.sys.realpath ? ts.sys.realpath(p) : p),
};

const service = ts.createLanguageService(host, ts.createDocumentRegistry());

/** The `@deprecated` tag text of a symbol, following an alias to its target, or `undefined`. */
function deprecatedText(symbol: ts.Symbol, checker: ts.TypeChecker): string | undefined {
  const chain = symbol.flags & ts.SymbolFlags.Alias ? [symbol, checker.getAliasedSymbol(symbol)] : [symbol];
  for (const s of chain) {
    const tag = s.getJsDocTags(checker).find((t) => t.name === 'deprecated');
    if (tag !== undefined) return ts.displayPartsToString(tag.text);
  }
  return undefined;
}

/** Map every export of an entry file that carries `@deprecated` to its tag text. */
function deprecatedExports(sourceRel: string): Map<string, string> {
  const program = service.getProgram();
  if (program === undefined) throw new Error('the language service has no program');
  const checker = program.getTypeChecker();
  const sourceFile = program.getSourceFile(path.join(KERNEL_ROOT, sourceRel));
  if (sourceFile === undefined) throw new Error(`${sourceRel} is not in the probe program`);
  const moduleSymbol = checker.getSymbolAtLocation(sourceFile);
  if (moduleSymbol === undefined) throw new Error(`${sourceRel} has no module symbol`);
  const result = new Map<string, string>();
  for (const exported of checker.getExportsOfModule(moduleSymbol)) {
    const text = deprecatedText(exported, checker);
    if (text !== undefined) result.set(exported.getName(), text);
  }
  return result;
}

describe.each(ENTRIES)('the deprecated accessors through $specifier', ({ specifier, probe, source }) => {
  const probePath = path.join(KERNEL_ROOT, probe);
  const probeText = files.get(probePath) ?? '';

  it(
    'resolves the probe with no semantic diagnostic (a failed resolution gives 2307 and no 6385)',
    () => {
      const messages = service.getSemanticDiagnostics(probePath).map((d) => `${d.code}: ${ts.flattenDiagnosticMessageText(d.messageText, '\n')}`);
      expect(messages).toEqual([]);
    },
    TIMEOUT,
  );

  it(
    'reports 6385 exactly once at each accessor import specifier, and never at prnParse',
    () => {
      const starts = service
        .getSuggestionDiagnostics(probePath)
        .filter((d) => d.code === DEPRECATED)
        .map((d) => d.start)
        .sort((a, b) => (a ?? 0) - (b ?? 0));
      const expected = ACCESSORS.map((name) => probeText.indexOf(`{ ${name} }`) + 2).sort((a, b) => a - b);
      expect(starts).toEqual(expected);
      expect(starts).not.toContain(probeText.indexOf('{ prnParse }') + 2);
    },
    TIMEOUT,
  );

  it(
    `deprecates exactly the six accessors in ${source}, each with a text that names prnParse`,
    () => {
      const deprecated = deprecatedExports(source);
      expect([...deprecated.keys()].sort()).toEqual([...ACCESSORS].sort());
      for (const [name, text] of deprecated) {
        expect(text, `${name} @deprecated text`).toContain('prnParse');
      }
    },
    TIMEOUT,
  );
});

describe('the two entries', () => {
  it(
    'carry the same @deprecated text for each accessor',
    () => {
      const wasm = deprecatedExports('src/wasm.ts');
      const napi = deprecatedExports('src/index.ts');
      for (const name of ACCESSORS) {
        expect(napi.get(name), name).toBe(wasm.get(name));
      }
    },
    TIMEOUT,
  );
});
```

- [ ] **Step 2: Add the test to the `node` project**

In `ts/packages/paigasus-kernel/vitest.config.ts`, add one line to the `node` project's `include` list, after `'tests/optimize-wasm.test.ts',`:

```ts
            'tests/deprecated-accessors.test.ts',
```

Vitest collects only the files in these lists. Without this line, the test never runs and the task still passes (spec § 3).

- [ ] **Step 3: Run the test through Moon and verify it fails**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-kernel-ts:test 2>&1 | tail -60`

Expected: FAIL. The two "resolves the probe" cases PASS (they prove that resolution works). These fail:
- "reports 6385 exactly once…": `starts` is `[]`, and the expected list has six offsets.
- "deprecates exactly the six accessors…": the set is `[]`.
- "carry the same @deprecated text…" passes vacuously here (both `undefined`). That is acceptable: the other cases fail.

Use Moon, not a file filter, so this run also proves the `include` line from Step 2. If the resolution cases FAIL, stop. Read the 2307 message and fix the probe setup before you continue. Record the failing case names in the task report.

- [ ] **Step 4: Tag the six accessors in `src/wasm.ts`**

Replace the whole content of `ts/packages/paigasus-kernel/src/wasm.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
import {
  sum,
  prnCanonicalize,
  prnErrorKind as rawPrnErrorKind,
  prnBuild,
  prnService as rawPrnService,
  prnRegion as rawPrnRegion,
  prnOrg as rawPrnOrg,
  prnResourceType as rawPrnResourceType,
  prnResourceId as rawPrnResourceId,
  prnParseFields,
  mintUuid7,
  prnCedarEntityType,
  prnCedarEntityId,
} from '@paigasus/wasm';
import { randHex10 } from './mint-util';
import { toPrnParseResult, type PrnParseResult } from './prn-parse';

// `prnParseFields` is imported, NOT re-exported: `prnParse` below is its only public form (SMA-673 D2).
export { sum, prnCanonicalize, prnBuild, mintUuid7, prnCedarEntityType, prnCedarEntityId };
export type { PrnParseResult };

// SMA-725: the six single-field accessors are deprecated. Each one parses the PRN again. Each is
// the raw binding function under a JSDoc-tagged `const`, so the behaviour does not change. The six
// blocks are the same in src/index.ts; tests/deprecated-accessors.test.ts holds the two in step.
// `paigasus/no-single-field-prn-accessor` (@paigasus/next-config/eslint) stops new callers.

/**
 * Return the stable `PrnError::kind()` token for an invalid PRN, or `""` if `s` parses.
 *
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind. For an
 * invalid PRN it returns `{ ok: false, errorKind }` (SMA-725). This accessor parses the PRN again.
 */
export const prnErrorKind = rawPrnErrorKind;

/**
 * Parse `s` and return its service field, or throw `kind()`.
 *
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind
 * (SMA-725). This accessor parses the PRN again on each call.
 */
export const prnService = rawPrnService;

/**
 * Parse `s` and return its region field, or throw `kind()`.
 *
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind
 * (SMA-725). This accessor parses the PRN again on each call.
 */
export const prnRegion = rawPrnRegion;

/**
 * Parse `s` and return its org field (hyphenated UUID, or `""` if absent), or throw `kind()`.
 *
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind
 * (SMA-725). This accessor parses the PRN again on each call.
 */
export const prnOrg = rawPrnOrg;

/**
 * Parse `s` and return its resource-type field, or throw `kind()`.
 *
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind
 * (SMA-725). This accessor parses the PRN again on each call.
 */
export const prnResourceType = rawPrnResourceType;

/**
 * Parse `s` and return its resource-id field (hyphenated UUID), or throw `kind()`.
 *
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind
 * (SMA-725). This accessor parses the PRN again on each call.
 */
export const prnResourceId = rawPrnResourceId;

/** Parse a PRN with ONE kernel call. Never throws for any input; a `TypeError` means a glue defect. */
export function prnParse(prn: string): PrnParseResult {
  return toPrnParseResult(prnParseFields(prn));
}

/** Mint a UUIDv7 from the ambient clock + CSPRNG (the injected FFI mint is pure). */
export function mint(): string {
  return mintUuid7(Date.now(), randHex10());
}
```

- [ ] **Step 5: Make `src/index.ts` the same except for the import source**

Copy `src/wasm.ts` to `src/index.ts`, then change only the line `} from '@paigasus/wasm';` to `} from '@paigasus/node-bindings';`.

Verify: `diff ts/packages/paigasus-kernel/src/index.ts ts/packages/paigasus-kernel/src/wasm.ts`
Expected: exactly one changed line (line 16), the import source. This is the same one-line difference as on `origin/main`.

- [ ] **Step 6: Run the tests and verify they pass**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-kernel-ts:test paigasus-kernel-ts:typecheck paigasus-console-core-ts:test 2>&1 | tail -40`

Expected: PASS. All eight cases in `deprecated-accessors.test.ts` pass. `prn-fields*`, `prn-canonical*`, `binding-parity` (via typecheck) and the console-core delegation test pass unchanged.

- [ ] **Step 7: Prove that the A2 assertion can fail (mutation)**

1. In `src/wasm.ts`, put `/** @deprecated SMA-725 mutation probe. */` on the line above `export function prnParse`. Replace its existing JSDoc line for the probe.
2. Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-kernel-ts:test 2>&1 | grep -E 'deprecates exactly|✗|×|FAIL' | head`
   Expected: FAIL in "deprecates exactly the six accessors in src/wasm.ts" (the set holds `prnParse` too). The 6385 case for `@paigasus/kernel` also fails (`prnParse` now gets a 6385).
3. Restore the original line: `/** Parse a PRN with ONE kernel call. Never throws for any input; a \`TypeError\` means a glue defect. */`. Do NOT use `git checkout --` for this: that also reverts Step 4.
4. Run `moon run paigasus-kernel-ts:test` again. Expected: PASS.

Record both runs (the failing case names, then the pass) in the task report.

- [ ] **Step 8: Check that the bindings did not change**

Run: `git diff --exit-code origin/main...HEAD -- rs/ && git status --porcelain -- rs/`
Expected: exit 0 and no output.

- [ ] **Step 9: Commit**

```bash
git add ts/packages/paigasus-kernel/src/index.ts ts/packages/paigasus-kernel/src/wasm.ts \
  ts/packages/paigasus-kernel/tests/deprecated-accessors.test.ts ts/packages/paigasus-kernel/vitest.config.ts
git commit -m "feat(ts): deprecate the six single-field PRN accessors in @paigasus/kernel (SMA-725)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The custom rule `paigasus/no-single-field-prn-accessor`

**Files:**
- Modify: `ts/packages/paigasus-next-config/src/eslint.mjs` (add the rule before `/** @type {import('eslint').ESLint.Plugin} */ const paigasusPlugin`, about line 400; add it to `paigasusPlugin.rules`)
- Create: `ts/packages/paigasus-next-config/tests/no-single-field-prn-accessor.test.ts`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `export const noSingleFieldPrnAccessor` (an `import('eslint').Rule.RuleModule` with `messageId: 'deprecatedAccessor'`, `data: { name }`), registered as `paigasusPlugin.rules['no-single-field-prn-accessor']`. Task 3 uses both.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-next-config/tests/no-single-field-prn-accessor.test.ts`:

```ts
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
    messageId: 'deprecatedAccessor',
    data: { name },
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
```

- [ ] **Step 2: Run the test and verify it fails**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-next-config && pnpm exec vitest run tests/no-single-field-prn-accessor.test.ts 2>&1 | tail -20`

Expected: FAIL at import: `noSingleFieldPrnAccessor` is not exported by `../src/eslint.mjs` (RuleTester gets `undefined`).

- [ ] **Step 3: Implement the rule**

In `ts/packages/paigasus-next-config/src/eslint.mjs`, insert this block directly above the line `/** @type {import('eslint').ESLint.Plugin} */` that opens `const paigasusPlugin`:

```js
/** The six single-field PRN accessors that SMA-725 deprecates. */
const DEPRECATED_PRN_ACCESSORS = new Set(['prnErrorKind', 'prnService', 'prnRegion', 'prnOrg', 'prnResourceType', 'prnResourceId']);

/** Module specifiers that export the six names: the two kernel entries and the two raw bindings. */
const PRN_ACCESSOR_MODULES = new Set(['@paigasus/kernel', '@paigasus/kernel/napi', '@paigasus/node-bindings', '@paigasus/wasm']);

/** The raw binding packages have no `exports` map, so a deep path into them also type-checks. */
const PRN_ACCESSOR_MODULE_PREFIXES = ['@paigasus/node-bindings/', '@paigasus/wasm/'];

/** @param {unknown} source */
function isPrnAccessorModule(source) {
  if (typeof source !== 'string') return false;
  return PRN_ACCESSOR_MODULES.has(source) || PRN_ACCESSOR_MODULE_PREFIXES.some((prefix) => source.startsWith(prefix));
}

/**
 * The name an import or export specifier names, or a property key names. An Identifier gives its
 * `name`; a string Literal (`import { 'prnService' as s }`, `k['prnOrg']`) gives its `value`.
 *
 * @param {any} node
 * @returns {string | undefined}
 */
function nameOf(node) {
  if (node?.type === 'Identifier') return node.name;
  if (node?.type === 'Literal' && typeof node.value === 'string') return node.value;
  return undefined;
}

/**
 * `paigasus/no-single-field-prn-accessor` (SMA-725 spec § 4.2).
 *
 * The six single-field PRN accessors of @paigasus/kernel are deprecated. Each one parses the PRN
 * again, and `prnParse` returns every field in ONE kernel call. This rule reports, for a watched
 * module: a named import of one of the six (value or type, aliased or not), a named re-export of
 * one, and a use of one through a namespace import (`k.prnService`, `k['prnOrg']`,
 * `const { prnRegion } = k`). It allows a namespace import itself, `export *`, and a dynamic
 * `import()` (spec D4, L1).
 *
 * WHY A CUSTOM RULE. The core `no-restricted-imports` already carries the boundary rules, and a
 * second block for the same files REPLACES their options. `@typescript-eslint/no-restricted-imports`
 * is deprecated since typescript-eslint 8.64.0 and points to the core rule (spec § 2, D3).
 *
 * The namespace check follows the import binding through ESLint's scope analysis. So a parameter or
 * a local variable that shadows the namespace name is not reported (spec limit L2 records what the
 * check does not follow).
 *
 * @type {import('eslint').Rule.RuleModule}
 */
export const noSingleFieldPrnAccessor = {
  meta: {
    type: 'suggestion',
    docs: {
      description: 'Disallow the six deprecated single-field PRN accessors of @paigasus/kernel; use prnParse (SMA-725)',
    },
    schema: [],
    messages: {
      deprecatedAccessor: "'{{name}}' is deprecated (SMA-725). Use prnParse(prn): one kernel call returns every field or the error kind.",
    },
  },
  create(context) {
    /** @param {any} node @param {string | undefined} name */
    const report = (node, name) => {
      if (name !== undefined && DEPRECATED_PRN_ACCESSORS.has(name)) {
        context.report({ node, messageId: 'deprecatedAccessor', data: { name } });
      }
    };

    /** @param {any} namespaceSpecifier */
    const checkNamespaceUses = (namespaceSpecifier) => {
      for (const variable of context.sourceCode.getDeclaredVariables(namespaceSpecifier)) {
        for (const reference of variable.references) {
          const id = reference.identifier;
          const parent = /** @type {any} */ (id).parent;
          if (parent?.type === 'MemberExpression' && parent.object === id) {
            // `k.prnService` names the property; `k['prnOrg']` names it with a string literal. A
            // computed key that is a variable (`k[n]`) names nothing we can know, so it is skipped.
            const property = parent.property;
            report(property, parent.computed ? (property.type === 'Literal' ? nameOf(property) : undefined) : nameOf(property));
          } else if (parent?.type === 'VariableDeclarator' && parent.init === id && parent.id.type === 'ObjectPattern') {
            for (const property of parent.id.properties) {
              if (property.type === 'Property' && !property.computed) report(property.key, nameOf(property.key));
            }
          }
        }
      }
    };

    return {
      ImportDeclaration(node) {
        if (!isPrnAccessorModule(node.source.value)) return;
        for (const specifier of node.specifiers) {
          if (specifier.type === 'ImportSpecifier') report(specifier, nameOf(specifier.imported));
          else if (specifier.type === 'ImportNamespaceSpecifier') checkNamespaceUses(specifier);
        }
      },
      ExportNamedDeclaration(node) {
        if (node.source === null || node.source === undefined || !isPrnAccessorModule(node.source.value)) return;
        for (const specifier of node.specifiers) report(specifier, nameOf(specifier.local));
      },
    };
  },
};

```

Then change `paigasusPlugin.rules` from:

```js
  rules: { 'no-js-relative-specifier': noJsRelativeSpecifier },
```

to:

```js
  rules: { 'no-js-relative-specifier': noJsRelativeSpecifier, 'no-single-field-prn-accessor': noSingleFieldPrnAccessor },
```

- [ ] **Step 4: Run the test and verify it passes**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-next-config && pnpm exec vitest run tests/no-single-field-prn-accessor.test.ts 2>&1 | tail -20`

Expected: PASS, every `valid` and `invalid` case. If a TypeScript-parser case fails because the
parser gives `ImportSpecifier.imported` or a `Literal` in another shape, fix `nameOf`, not the test.

- [ ] **Step 5: Prove the computed-key guard can fail (mutation)**

1. In `checkNamespaceUses`, temporarily change the `report(property, …)` line to
   `report(property, nameOf(property));` (it then reads an identifier key as a name).
2. Run the Step 4 command. Expected: FAIL in the `valid` case
   `const prnService = 'prnParse';\nk[prnService];` (one unexpected report).
3. Restore the original line exactly. Run again. Expected: PASS.

Record both runs in the task report. The shadowing `valid` case (Review Focus 1) needs no mutation:
a name-based implementation reports it, so the case itself is the control.

- [ ] **Step 6: Run the package checks**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-next-config-ts:test paigasus-next-config-ts:typecheck 2>&1 | tail -30`

Expected: PASS. The existing `boundaries.test.ts` and `no-js-relative-specifier.test.ts` stay green. The case "is a separate export, not a boundaryRules block" checks `block?.plugins?.['paigasus']?.rules?.['no-js-relative-specifier']` only, so the second plugin rule does not break it.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-next-config/src/eslint.mjs ts/packages/paigasus-next-config/tests/no-single-field-prn-accessor.test.ts
git commit -m "feat(ts): add the paigasus/no-single-field-prn-accessor lint rule (SMA-725)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Apply the rule in the workspace config

**Files:**
- Modify: `ts/packages/paigasus-next-config/src/eslint.mjs` (add `export const kernelAccessorRules` after `sourceRules`)
- Modify: `ts/eslint.config.js` (import and spread `kernelAccessorRules` after `...sourceRules`)
- Create: `ts/packages/paigasus-next-config/tests/kernel-accessor-rules.test.ts`

**Interfaces:**
- Consumes: `noSingleFieldPrnAccessor` and `paigasusPlugin` from Task 2 (same file).
- Produces: `export const kernelAccessorRules: import('eslint').Linter.Config[]` with exactly one block named `paigasus/kernel/no-single-field-prn-accessor`.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-next-config/tests/kernel-accessor-rules.test.ts`:

```ts
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
  // A dead `import` satisfies a grep; only the exported array proves the spread.
  it('carries every kernelAccessorRules entry in its EXPORTED array', async () => {
    const shipped = (await import('../../../eslint.config.js')).default as Array<{ files?: string[]; ignores?: string[] }>;
    for (const entry of kernelAccessorRules) {
      expect(shipped, `ts/eslint.config.js dropped the ${entry.name} block`).toContainEqual(expect.objectContaining({ files: entry.files, ignores: entry.ignores }));
    }
  });

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
```

- [ ] **Step 2: Run the test and verify it fails**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-next-config && pnpm exec vitest run tests/kernel-accessor-rules.test.ts 2>&1 | tail -30`

Expected: FAIL. `kernelAccessorRules` is `undefined`, so the structural case and the carry case fail. The COVERED cases fail (the rule is not configured). The exempt cases may pass vacuously. The boundary row passes. Record the failing case names.

- [ ] **Step 3: Add `kernelAccessorRules`**

In `ts/packages/paigasus-next-config/src/eslint.mjs`, add this block directly after the closing `];` of `export const sourceRules = [ … ];`:

```js

/**
 * The SMA-725 kernel-accessor block, as an ESLint flat-config array. `ts/eslint.config.js` spreads
 * it after `sourceRules`, and `tests/kernel-accessor-rules.test.ts` pins that spread.
 *
 * WHY NOT INSIDE `boundaryRules`. The liveness test there derives a `BOUNDARY_SCOPES` key from every
 * block's `files[0]`, and `**` is not a package directory.
 *
 * Scope: every linted source file, JS included, because the rule needs no type information.
 * Exempt: the kernel's own entry files (they import the raw names to re-export them), the four
 * kernel tests that test the deprecated accessors while they exist (spec D1), and the console-core
 * delegation guard, which imports the six to prove that nobody calls them. A NEW kernel test is not
 * exempt. The block registers its own plugin, so it depends on no other block.
 *
 * @type {import('eslint').Linter.Config[]}
 */
export const kernelAccessorRules = [
  {
    name: 'paigasus/kernel/no-single-field-prn-accessor',
    files: ['**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    ignores: [
      'packages/paigasus-kernel/src/**',
      'packages/paigasus-kernel/tests/prn-fields.test.ts',
      'packages/paigasus-kernel/tests/prn-fields.wasm.test.ts',
      'packages/paigasus-kernel/tests/prn-canonical.test.ts',
      'packages/paigasus-kernel/tests/prn-canonical.wasm.test.ts',
      'packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts',
    ],
    plugins: { paigasus: paigasusPlugin },
    rules: { 'paigasus/no-single-field-prn-accessor': 'error' },
  },
];
```

`sourceRules` and `kernelAccessorRules` both register the plugin under the key `paigasus` with the SAME object, so ESLint accepts both blocks for one file.

- [ ] **Step 4: Spread it in `ts/eslint.config.js`**

Change the import line:

```js
import { boundaryRules, nextAppRules, sourceRules } from '@paigasus/next-config/eslint';
```

to:

```js
import { boundaryRules, kernelAccessorRules, nextAppRules, sourceRules } from '@paigasus/next-config/eslint';
```

Then change the end of the config from:

```js
  ...sourceRules,
);
```

to:

```js
  ...sourceRules,
  // SMA-725: the six single-field PRN accessors of @paigasus/kernel are deprecated; use prnParse.
  // A custom rule with its OWN name, never a second `no-restricted-imports` block, which would
  // replace the boundary options above. paigasus-next-config-ts:test asserts this spread.
  ...kernelAccessorRules,
);
```

- [ ] **Step 5: Run the tests and verify they pass**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-next-config-ts:test paigasus-next-config-ts:typecheck 2>&1 | tail -30`

Expected: PASS, every case in all four test files of the package.

- [ ] **Step 6: Red-first 1 — remove the spread**

1. Delete only the line `  ...kernelAccessorRules,` from `ts/eslint.config.js`.
2. Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd ts/packages/paigasus-next-config && pnpm exec vitest run tests/kernel-accessor-rules.test.ts 2>&1 | tail -30`
   Expected: FAIL in "carries every kernelAccessorRules entry", in every COVERED case, and in "reports a deprecated accessor import…".
3. Put the line back exactly. Do NOT use `git checkout --` (that also reverts Step 4's import and comment).
4. Run the same command. Expected: PASS.

- [ ] **Step 7: Red-first 2 — use the core rule name**

1. In `kernelAccessorRules`, temporarily replace the `rules` line with:
   ```js
       rules: { 'no-restricted-imports': ['error', { paths: [{ name: '@paigasus/kernel', importNames: ['prnService'] }] }] },
   ```
2. Run the same vitest command as Step 6.
   Expected: FAIL in the structural case ("never configures no-restricted-imports") and in "keeps the console-core boundary rule on the same TS file" (the core rule options are replaced, so `@paigasus/proto` is no longer reported).
3. Restore the line `    rules: { 'paigasus/no-single-field-prn-accessor': 'error' },` exactly.
4. Run the command again. Expected: PASS.

Record the failing case names of Steps 6 and 7 in the task report. If the boundary row stays green in Step 7, STOP and report it: the A7 guard does not work and the spec must be revisited.

- [ ] **Step 8: Run the whole lint and the formatter**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run ts:lint ts:fmt 2>&1 | tail -30`

Expected: PASS. No file in the tree imports one of the six outside the exempt paths (spec § 3). If `ts:fmt` reports a file, run `pnpm -C ts exec prettier --write <file>` on the files this plan changed only, and run `ts:fmt` again.

- [ ] **Step 9: Commit**

```bash
git add ts/packages/paigasus-next-config/src/eslint.mjs ts/eslint.config.js ts/packages/paigasus-next-config/tests/kernel-accessor-rules.test.ts
git commit -m "feat(ts): apply the kernel accessor rule in the workspace eslint config (SMA-725)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Regression, the full gate graph and the follow-up issue (coordinator)

The coordinator runs this task, not an implementer subagent: it files a Linear issue.

**Files:** none changed, unless a gate finds a defect.

- [ ] **Step 1: The affected packages**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run ts:lint ts:fmt paigasus-kernel-ts:test paigasus-kernel-ts:typecheck paigasus-next-config-ts:test paigasus-next-config-ts:typecheck paigasus-console-core-ts:test paigasus-console-core-ts:typecheck 2>&1 | tail -30`

Expected: PASS.

- [ ] **Step 2: The bindings did not change (A9)**

Run: `git diff --exit-code origin/main...HEAD -- rs/crates/bindings/`
Expected: exit 0, no output.

- [ ] **Step 3: The full gate graph**

Run the command between the `ci-targets` markers in the root `CLAUDE.md`, with the proto PATH. Read the "This development Mac only" section first: pick one bash for the run, and re-run the gates that need the other bash directly. Diagnose any failure with the `moon-diagnosis` procedure in the root `CLAUDE.md` (Step 0 first: copy `.moon/cache/ciReport.json` before any re-run).

Expected: every gate this change affects passes. Record any host-only failure (bash version, pipe size) as such, with its evidence, not as a finding.

- [ ] **Step 4: File the follow-up Linear issue (A10)**

Create the issue with the Linear MCP `save_issue`:

- team: Sven Maschek; project: Paigasus Polyglot; milestone: Frontend; priority: 4 (Low)
- labels: `area:frontend`, `area:ffi`, `Improvement`; relatedTo: `SMA-725`
- title: `ts: remove the six deprecated single-field PRN accessors from @paigasus/kernel`
- description: the text of spec § 8, with a link to the spec file.

Record the new issue key in the PR body.
