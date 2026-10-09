# SMA-725 — deprecate the six single-field PRN accessors in `@paigasus/kernel` (TS)

- Linear: SMA-725 (follow-up of SMA-673, open question Q3).
- Status: design approved in chat on 2026-10-09. D3 and D4 were decided again after the spec
  challenge (§ 10). This file is the written spec.
- Scope: the TypeScript workspace `ts/` only.

## 1. Problem

SMA-673 added `prnParse` to `@paigasus/kernel`. One kernel call parses a PRN and returns every
field, or the error kind. The six single-field accessors stay exported:

`prnErrorKind`, `prnService`, `prnRegion`, `prnOrg`, `prnResourceType`, `prnResourceId`.

Each accessor parses the PRN again. Nothing marks them as old, and nothing stops a new caller from
reading a PRN field by field (SMA-673 spec, limit L1).

## 2. Decisions (approved by Sven on 2026-10-09)

| Id | Question | Decision |
|----|----------|----------|
| D1 | End state | Deprecate now. Remove later, in a follow-up issue (§ 8). The kernel's own accessor tests stay until that removal. |
| D2 | Where the `@deprecated` tag lives | In the TS entry files `src/index.ts` and `src/wasm.ts` only. No Rust change. No regeneration. |
| D3 | Enforcement | A custom ESLint rule, `paigasus/no-single-field-prn-accessor`, in the existing `paigasusPlugin` of `@paigasus/next-config/eslint`. |
| D4 | Namespace imports | Allow `import * as k` and `export *`. Report a use of one of the six through the namespace (`k.prnService`). |

Measured reasons for D3:

- The repo-wide `@typescript-eslint/no-deprecated` rule is not used. With it enabled for every TS
  file, `eslint .` in `ts/` reported 12 errors in 9 files on 2026-10-09. None of them is a PRN
  accessor. They come from React `FormEvent`, vitest `toMatchTypeOf`, `isTypeOnly`,
  `allowInsecureRequests` and `ServiceInfoSchema`.
- The core `no-restricted-imports` rule is not used. `@paigasus/next-config/eslint` already uses it
  for the package boundaries. In a flat config, a later block that sets the same rule for the same
  files REPLACES the earlier configuration (`ts/CLAUDE.md`, the Turbopack bullet). A second block
  would switch the boundary rules off with no error.
- `@typescript-eslint/no-restricted-imports` is not used. In `typescript-eslint` 8.71.0 its `meta`
  says `deprecated: { deprecatedSince: '8.64.0', replacedBy: no-restricted-imports }` (measured).
  A later major can remove it. Its upstream notice also tells a maintainer to change to the core
  rule, which causes the replacement fault above.
- A custom rule has its own name. It follows the existing custom rule
  `paigasus/no-js-relative-specifier` (`sourceRules`, SMA-511). It needs no type information and no
  `@typescript-eslint` plugin, so it can also check `.js` and `.mjs` files. It can see a use through
  a namespace import (D4), which `no-restricted-imports` cannot.

## 3. Current state (measured on `origin/main` 576967f8)

- `@paigasus/kernel` `exports`: `.` → `src/wasm.ts`, `./napi` → `src/index.ts`. Both are
  hand-written TS. Both re-export the six accessors unchanged (line 21) from `@paigasus/wasm` and
  `@paigasus/node-bindings`. Their types and JSDoc come from the generated, committed binding `.d.ts`
  files.
- `ts/node_modules/@paigasus/kernel` does not exist. A file inside the kernel package reaches
  `@paigasus/kernel` through package self-reference (`name` + `exports`).
- Production callers: none. `ts/packages/paigasus-console-core/src/prn-tenancy.ts` already uses
  `prnParse` (SMA-673).
- Test callers:
  - `ts/packages/paigasus-kernel/tests/prn-fields.test.ts`, `prn-fields.wasm.test.ts`,
    `prn-canonical.test.ts`, `prn-canonical.wasm.test.ts`. These test the accessors themselves.
  - `ts/packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts`. It imports the six
    to mock them and to assert that `prn-tenancy.ts` never calls them (SMA-673 A4). It is a
    negative guard and must keep the imports.
- `ts/packages/paigasus-kernel/tests/wasm-probe.mjs` calls the accessors as `api.*`. It imports the
  raw wasm glue by a file URL (line 51), not by one of the specifiers in § 4.2. The rule does not see
  it, and it needs no exemption.
- `ts/packages/paigasus-kernel/src/binding-parity.types.ts` compares the napi and wasm binding
  types with `Exact<>`. It reads the raw binding types, not the kernel re-exports.
- `ts/packages/paigasus-kernel/vitest.config.ts` lists its test files BY NAME, in a `node` project
  (lines 31-42) and a `browser` project (lines 69-76). Vitest does not collect a file that is in
  neither list, and the run still exits 0.

## 4. Design

### 4.1 The deprecation tag (D2)

In `ts/packages/paigasus-kernel/src/index.ts` and `ts/packages/paigasus-kernel/src/wasm.ts`:

1. Import each of the six accessors under an alias, for example `prnService as rawPrnService`.
2. Remove the six from the plain `export { … }` list.
3. Export each one again as a `const` with a JSDoc block. The block keeps the summary line of the
   binding `.d.ts`, so the editor hover does not lose it:

```ts
/**
 * Parse `s` and return its service field, or throw `kind()`.
 *
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind
 * (SMA-725). This accessor parses the PRN again on each call.
 */
export const prnService = rawPrnService;
```

The text for `prnErrorKind` says that `prnParse(prn)` returns `{ ok: false, errorKind }` for an
invalid PRN. The six `@deprecated` texts are the same in both entries.

The type of each `const` is the type of the raw binding function. The hover changes from
`function prnService(s: string): string` to `const prnService: (s: string) => string`. Call sites
see no type change. The behaviour does not change: the `const` is the same function object. The
form is valid under `verbatimModuleSyntax` and `isolatedModules`. The kernel has no `sideEffects`
field, so tree shaking does not change. The console-core delegation test mocks
`@paigasus/kernel` with a factory, so the form does not affect it.

The Rust crates, the napi glue (`index.js`, `index.d.ts`) and the five wasm artifacts do not change.
So these checks are not affected: `committed-napi-glue.test.ts`, `committed-wasm.test.ts`, the
wasm-lockstep workflow, `binding-parity.types.ts`.

### 4.2 The custom rule (D3, D4)

`ts/packages/paigasus-next-config/src/eslint.mjs` gets:

- a rule object, `noSingleFieldPrnAccessor`, exported for its `RuleTester` test, as
  `noJsRelativeSpecifier` is;
- an entry `'no-single-field-prn-accessor': noSingleFieldPrnAccessor` in `paigasusPlugin.rules`;
- a new export, `kernelAccessorRules`. It is a separate export, like `sourceRules`, and not a
  block inside `boundaryRules`.

```js
/** @type {import('eslint').Linter.Config[]} */
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

**Watched specifiers.** A module specifier is watched if it is one of these:

- exactly `@paigasus/kernel`, `@paigasus/kernel/napi`, `@paigasus/node-bindings` or
  `@paigasus/wasm`;
- a deep path that starts with `@paigasus/node-bindings/` or `@paigasus/wasm/`. These two packages
  have no `exports` map, so `@paigasus/wasm/paigasus_wasm.js` type-checks.

**The six names** are `prnErrorKind`, `prnService`, `prnRegion`, `prnOrg`, `prnResourceType`,
`prnResourceId`.

**The rule reports these forms** when the specifier is watched:

| Form | Example |
|------|---------|
| Named import, value or type-only, aliased or not | `import { prnOrg as org } from '@paigasus/kernel'` |
| Named re-export | `export { prnService } from '@paigasus/kernel'` |
| Member use of a namespace import (D4) | `import * as k from '@paigasus/kernel'; k.prnService(p)` and `k['prnService']` |
| Destructuring of a namespace import (D4) | `const { prnRegion } = k` |

**The rule does not report these forms:**

| Form | Reason |
|------|--------|
| `import * as k` alone, or `k.prnParse` | D4. |
| `export * from '@paigasus/kernel'` and `export * as k from …` | D4. The re-exporting module is then a new module, and its importers are not watched (L1). |
| A dynamic `import('@paigasus/kernel')` | L1. |
| Any name that is not one of the six | Not deprecated. |

**The message** is one fixed text with the name as a placeholder:
`'{{name}}' is deprecated (SMA-725). Use prnParse(prn): one kernel call returns every field or the error kind.`

**Why each exempt path is exempt:**

| Path | Reason |
|------|--------|
| `packages/paigasus-kernel/src/**` | The entry files must import the raw names to re-export them. |
| The four kernel accessor test files | They test the deprecated API while it exists (D1). A new kernel test is NOT exempt. |
| `packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts` | The negative guard imports the six to prove that nobody calls them. |

`ts/eslint.config.js` imports `kernelAccessorRules` and spreads it after `sourceRules`, with a
comment in the style of the `sourceRules` comment.

The block registers its own plugin, as `sourceRules` does. It depends on no other block. It does not
configure core `no-restricted-imports`, so the boundary rules stay in force.
`ts/packages/paigasus-next-config/package.json` gets no new dependency.

### 4.3 Callers

No production caller remains, so no production code moves. The test callers in § 3 stay. They are
exempt by § 4.2, or the rule does not see them (`wasm-probe.mjs`).

## 5. Acceptance criteria

- A1. The six accessors exported by `@paigasus/kernel` and by `@paigasus/kernel/napi` carry a
  `@deprecated` JSDoc tag that names `prnParse`.
- A2. In each entry, the set of exports with a `@deprecated` tag is exactly the six accessors.
- A3. ESLint reports `paigasus/no-single-field-prn-accessor` for each form in the "reports" table of
  § 4.2, for each of the six names and each watched specifier, in any linted file outside the exempt
  paths. The message names the accessor, `prnParse` and SMA-725.
- A4. ESLint reports nothing for each form in the "does not report" table of § 4.2.
- A5. The real `ts/eslint.config.js` does not apply the rule to the six exempt paths, and does apply
  it to a non-exempt file beside each one.
- A6. `ts/eslint.config.js` applies `kernelAccessorRules`. A test fails if the spread is removed.
- A7. The boundary rules still apply to a real TS source file under the real `ts/eslint.config.js`.
  A structural test fails if a `kernelAccessorRules` block configures core `no-restricted-imports`.
- A8. `moon run ts:lint` passes on the branch.
- A9. The committed napi glue and the five wasm artifacts do not change on the branch.
- A10. A follow-up Linear issue for the removal exists (§ 8).

## 6. Tests

### 6.1 The tag reaches a consumer (A1, A2)

A new kernel test, `ts/packages/paigasus-kernel/tests/deprecated-accessors.test.ts`. It goes into
the `node` project's `include` list in `ts/packages/paigasus-kernel/vitest.config.ts`. It needs no
wasm plugin.

- Make a TypeScript language service over two in-memory probe files. The probe files are INSIDE the
  kernel package, at `ts/packages/paigasus-kernel/tests/__probe_wasm__.ts` and
  `…/tests/__probe_napi__.ts`, so package self-reference resolves `@paigasus/kernel`. The files exist
  only in the language-service host, never on disk.
- The compiler options are written in the test: `module: 'esnext'`,
  `moduleResolution: 'bundler'`, `target: 'es2022'`, `strict: true`, `noEmit: true`. The test does not
  read a tsconfig, so `paigasus-kernel-ts:test` needs no new tsconfig input.
- Each probe holds import statements only: the six accessors and `prnParse`, from
  `@paigasus/kernel` in one file and from `@paigasus/kernel/napi` in the other.
- Assert zero semantic diagnostics on each probe. This proves that the module resolved. A failed
  resolution gives 2307 and no 6385.
- Use `getSuggestionDiagnostics` (the only public API that returns 6385 in TypeScript 6.0.3).
  Assert exactly one 6385 ("'{0}' is deprecated") at the position of each of the six import
  specifiers in each probe, and none at `prnParse`.
- For A2: for each entry, list the exports with `checker.getExportsOfModule(...)`. Collect the names
  whose symbol (followed through aliases) has a `deprecated` JSDoc tag. Assert that the set is
  exactly the six.
- Read the tag text from `getJSDocTags()` (or quick info `tags[].text`), not from `documentation`.
  Assert that each text contains `prnParse` and that the six texts are the same in both entries.
- Timeout: 30 s. A cold language service can go past the 5 s default in CI.

Red-first:

1. Before § 4.1, run `moon run paigasus-kernel-ts:test` (not a file filter, so the `include` entry is
   proven too). The 6385 and A2 assertions fail. Record the failure.
2. After § 4.1, add `@deprecated` to `prnParse` in one entry. The A2 assertion fails. Remove it.
   Record both runs.

### 6.2 The rule (A3, A4)

A new file `ts/packages/paigasus-next-config/tests/no-single-field-prn-accessor.test.ts`. It uses
ESLint's `RuleTester`, as `no-js-relative-specifier.test.ts` does. Use `tseslint.parser` for the TS
cases, and the default parser for one `.mjs` case.

- `invalid` cases: each of the six names as a named import from each of the four exact specifiers
  (24 cases); a deep `@paigasus/wasm/paigasus_wasm.js` and a deep `@paigasus/node-bindings/index.js`
  import; an aliased import; an `import type`; a named re-export; `k.prnService(p)`;
  `k['prnOrg']`; `const { prnRegion } = k`; one `.mjs` file. Each case asserts the message text,
  with the name filled in.
- `valid` cases: `prnParse`, `prnBuild` and `prnCanonicalize` from each specifier; `import * as k`
  with `k.prnParse(p)`; `export * from '@paigasus/kernel'`; `export * as k from '@paigasus/kernel'`;
  a dynamic `import('@paigasus/kernel')`; `prnService` imported from an unwatched module
  (`./local`); a local variable named `k` that is not a namespace import, with `k.prnService`.

### 6.3 The real config (A5, A6, A7)

A new file `ts/packages/paigasus-next-config/tests/kernel-accessor-rules.test.ts`.

- Structural: `ts/eslint.config.js`, loaded as a module, carries every `kernelAccessorRules`
  entry, as the existing `sourceRules` check does (`boundaries.test.ts:342`).
- Structural: no `kernelAccessorRules` block sets `no-restricted-imports` or
  `@typescript-eslint/no-restricted-imports`.
- Exemptions (A5): `new ESLint({ cwd: TS_ROOT }).calculateConfigForFile(path)` on the real config.
  For each exempt path, the rule is absent. For a non-exempt sibling, the rule is present. Siblings:
  `packages/paigasus-kernel/tests/prn-parse.test.ts`,
  `packages/paigasus-console-core/tests/unit/prn-tenancy.test.ts`,
  `packages/paigasus-kernel/tests/wasm-probe.mjs` (present: the block covers `.mjs`).
- Liveness (A6): lint a DENIED source through the real config, with `cwd: TS_ROOT` and the file
  path `packages/paigasus-console-core/src/prn-tenancy.ts`. That file is real, non-exempt and in a
  tsconfig. Assert that no message is `fatal` and that the rule reports. Timeout: 120 s, as at
  `boundaries.test.ts:358-367`.
- Boundary liveness (A7): lint `import { x } from '@paigasus/proto';` through the real config at
  the same path. Assert that no message is `fatal` and that core `no-restricted-imports` reports.
  The existing real-config rows use `.mjs` paths only, which a TS-only block would not touch, so
  they cannot catch a replacement on TS files.

Red-first:

1. Remove the spread from `ts/eslint.config.js`. The liveness row and the structural carry check
   fail. Record it, then restore the spread.
2. Change the block's rule key to core `no-restricted-imports` with `paths` options. The boundary
   liveness row and the structural check fail. Record it, then restore.

`paigasus-next-config-ts:test` already lists `/ts/eslint.config.js` as an input, so a change to the
spread selects these tests.

### 6.4 Regression (A8, A9)

- `moon run ts:lint ts:fmt paigasus-kernel-ts:test paigasus-kernel-ts:typecheck
  paigasus-next-config-ts:test paigasus-next-config-ts:typecheck paigasus-console-core-ts:test`.
- `git diff --exit-code origin/main...HEAD -- rs/crates/bindings/` is empty.
- Before the push, run the full gate graph (root `CLAUDE.md`, "Before you push").

## 7. Limits (known, not fixed)

- L1. The rule does not see a dynamic `import()`. It does not follow a module that does
  `export * from '@paigasus/kernel'`: an importer of that module is not watched. The
  `@deprecated` tag still shows a strikethrough in an editor in both cases.
- L2. The rule follows a namespace import only through a direct member access or a destructuring of
  the namespace identifier. It does not follow the namespace when it is assigned to another
  variable or passed to a function.
- L3. The raw binding packages `@paigasus/node-bindings` and `@paigasus/wasm` carry no
  `@deprecated` tag in their generated `.d.ts`. The rule covers their imports (§ 4.2).
- L4. The Python accessors stay and carry no mark (SMA-673 Q1).
- L5. `contracts/proto/paigasus/common/v1/actor.proto:16` names `prnResourceType` as the TS/JS way to
  read a resource type. The comment stays. A change means a codegen run in three languages for one
  comment. The follow-up removal issue (§ 8) updates it.
- L6. An `eslint-disable` comment turns the rule off for one line, and nothing forbids it.

## 8. Follow-up

File one Linear issue: remove the six accessors from the `@paigasus/kernel` TS surface. Decide in
that issue whether the Rust napi and wasm bindings also drop them. That changes the wasm binary, the
`EXPECTED_EXPORTS` list in `committed-wasm.test.ts`, the napi glue and `binding-parity.types.ts`.
Remove `kernelAccessorRules` and the rule in the same issue, or keep the rule to block a return of
the names. Update `actor.proto` (L5). Fields: project Paigasus Polyglot, milestone Frontend, priority
Low, labels `area:frontend`, `area:ffi`, Improvement, related to SMA-725.

## 9. Out of scope

- The Rust kernel and the Rust binding crates.
- The Python binding.
- The 12 unrelated `@typescript-eslint/no-deprecated` findings (§ 2).

## 10. Spec challenge (2026-10-09)

Verdict: APPROVE WITH CHANGES. One BLOCKER, four MAJOR, fifteen MINOR, four questions.

Folded in:

- BLOCKER: `@typescript-eslint/no-restricted-imports` is deprecated since 8.64.0 (measured). Sven
  decided D3 again: a custom rule. He decided D4: allow namespace imports, report a member use.
- MAJOR: L1 was false for the core rule (it reports namespace imports). The custom rule follows D4,
  and L1 and L2 now describe it.
- MAJOR: the kernel vitest config lists files by name. § 6.1 adds the file to `include` and runs the
  red-first through moon.
- MAJOR: A2 had no test. § 6.1 now checks the full export set, with a mutation.
- MAJOR: no test caught a change to the core rule name. § 6.3 adds a TS boundary liveness row and a
  structural check.
- MINOR, folded: the test writes its compiler options (no tsconfig input); the probe path is inside
  the kernel; 6385 is counted per import specifier; the tag text is read from `tags`; the summary
  line is kept; deep imports are watched; the exemption checks use `calculateConfigForFile`; the
  liveness row names a real path, checks `fatal` and sets a timeout; the block registers its own
  plugin; the `@type` annotation is in the sample; the message is asserted; A9 uses a three-dot
  diff; `wasm-probe.mjs` is described correctly; A3 says "linted file"; § 6.1 has a timeout; § 6.4
  adds `ts:fmt`, the typecheck tasks and the full graph.
- QUESTION, folded: the kernel test exemption now names the four accessor test files, not
  `tests/**`.

Not acted on:

- "Update the comment at `eslint.mjs:28-31`": with the custom rule, the comment that
  `@typescript-eslint/no-restricted-imports` is not used stays true.
- "Are the rows for `@paigasus/wasm` and `@paigasus/node-bindings` worth it?": yes. With a custom
  rule they cost one list entry each, and they also cover deep imports.
