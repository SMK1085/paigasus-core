# SMA-725 — deprecate the six single-field PRN accessors in `@paigasus/kernel` (TS)

- Linear: SMA-725 (follow-up of SMA-673, open question Q3).
- Status: design approved in chat on 2026-10-09. This file is the written spec.
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
| D3 | Enforcement | A targeted lint ban with `@typescript-eslint/no-restricted-imports`. Not the repo-wide `@typescript-eslint/no-deprecated` rule. Not a new `repo:*` gate. |

Measured reason for D3: with `@typescript-eslint/no-deprecated` enabled for every TS file, `eslint .`
in `ts/` reported 12 errors in 9 files on 2026-10-09. None of them is a PRN accessor. They come from
React `FormEvent`, vitest `toMatchTypeOf`, `isTypeOnly`, `allowInsecureRequests` and
`ServiceInfoSchema`. That rule would make this issue fix unrelated code.

Measured reason for the rule name: `@paigasus/next-config/eslint` already uses the core
`no-restricted-imports` rule for the package boundaries. In a flat config, a later block that sets the
same rule for the same files REPLACES the earlier configuration (`ts/CLAUDE.md`, the Turbopack
bullet). `@typescript-eslint/no-restricted-imports` takes the same options under a different rule
name, so it cannot switch the boundary rules off. `typescript-eslint` 8.71.0 ships the rule.

## 3. Current state (measured on `origin/main` 576967f8)

- `@paigasus/kernel` `exports`: `.` → `src/wasm.ts`, `./napi` → `src/index.ts`. Both are
  hand-written TS. Both re-export the six accessors unchanged (line 21) from `@paigasus/wasm` and
  `@paigasus/node-bindings`. Their types and JSDoc come from the generated, committed binding `.d.ts`
  files.
- Production callers: none. `ts/packages/paigasus-console-core/src/prn-tenancy.ts` already uses
  `prnParse` (SMA-673).
- Test callers:
  - `ts/packages/paigasus-kernel/tests/prn-fields.test.ts`, `prn-fields.wasm.test.ts`,
    `prn-canonical.test.ts`, `prn-canonical.wasm.test.ts`. These test the accessors themselves.
  - `ts/packages/paigasus-kernel/tests/wasm-probe.mjs` (plain Node, `api.*` calls).
  - `ts/packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts`. It imports the six
    to mock them and to assert that `prn-tenancy.ts` never calls them (SMA-673 A4). It is a
    negative guard and must keep the imports.
- `ts/packages/paigasus-kernel/src/binding-parity.types.ts` compares the napi and wasm binding
  types with `Exact<>`. It reads the raw binding types, not the kernel re-exports.

## 4. Design

### 4.1 The deprecation tag (D2)

In `ts/packages/paigasus-kernel/src/index.ts` and `ts/packages/paigasus-kernel/src/wasm.ts`:

1. Import each of the six accessors under an alias, for example `prnService as rawPrnService`.
2. Remove the six from the plain `export { … }` list.
3. Export each one again as a `const` with a JSDoc block:

```ts
/**
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind
 * (SMA-725). This accessor parses the PRN again on each call.
 */
export const prnService = rawPrnService;
```

The text for `prnErrorKind` says that `prnParse(prn)` returns `{ ok: false, errorKind }` for an
invalid PRN. The type of each `const` is the type of the raw binding function, so a caller sees no
type change. The behaviour does not change: the `const` is the same function object.

The two entries must hold the same six JSDoc blocks. The Rust crates, the napi glue
(`index.js`, `index.d.ts`) and the five wasm artifacts do not change. So these checks are not
affected: `committed-napi-glue.test.ts`, `committed-wasm.test.ts`, the wasm-lockstep workflow,
`binding-parity.types.ts`.

### 4.2 The lint ban (D3)

`ts/packages/paigasus-next-config/src/eslint.mjs` gets a new export, `kernelAccessorRules`. It is a
separate export, like `sourceRules`, and not a block inside `boundaryRules`.

```js
export const kernelAccessorRules = [
  {
    name: 'paigasus/kernel/no-single-field-prn-accessor',
    files: ['**/*.{ts,tsx,mts,cts}'],
    ignores: [
      'packages/paigasus-kernel/src/**',
      'packages/paigasus-kernel/tests/**',
      'packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts',
    ],
    rules: {
      '@typescript-eslint/no-restricted-imports': ['error', { paths: [/* one entry per specifier */] }],
    },
  },
];
```

- Specifiers (four `paths` entries): `@paigasus/kernel`, `@paigasus/kernel/napi`,
  `@paigasus/node-bindings`, `@paigasus/wasm`.
- `importNames` on each entry: the six accessors.
- `message` on each entry names `prnParse` and SMA-725.
- `allowTypeImports` stays at its default (`false`). A type-only import of an accessor is also
  reported. No such import exists, and none has a use.
- The block registers no plugin. The workspace config registers `@typescript-eslint` for the same
  `files` glob in its type-checked block. A unit test that lints with `overrideConfigFile: true`
  must add `tseslint.plugin` and `tseslint.parser` itself.

Why each exempt path is exempt:

| Path | Reason |
|------|--------|
| `packages/paigasus-kernel/src/**` | The entry files must import the raw names to re-export them. |
| `packages/paigasus-kernel/tests/**` | These tests test the deprecated API while it exists (D1). |
| `packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts` | The negative guard imports the six to prove that nobody calls them. |

`ts/eslint.config.js` imports `kernelAccessorRules` and spreads it after `sourceRules`.

`ts/packages/paigasus-next-config/package.json` gets no new dependency: the block names the rule by
string only.

### 4.3 Callers

No production caller remains, so no production code moves. The test callers in § 3 stay. They are
exempt by § 4.2.

## 5. Acceptance criteria

- A1. The six accessors exported by `@paigasus/kernel` and by `@paigasus/kernel/napi` carry a
  `@deprecated` JSDoc tag that names `prnParse`.
- A2. `prnParse` and the other kernel exports carry no `@deprecated` tag.
- A3. `eslint` reports `@typescript-eslint/no-restricted-imports` for an import of any of the six from
  any of the four specifiers, in any TS file outside the three exempt paths.
- A4. `eslint` reports nothing for an import of `prnParse`, `prnBuild` or `prnCanonicalize` from the
  same specifiers.
- A5. `eslint` reports nothing for the three exempt paths.
- A6. `ts/eslint.config.js` applies `kernelAccessorRules`. A test fails if the spread is removed.
- A7. `moon run ts:lint` passes on the branch. The existing `boundaryRules` and `sourceRules` tests
  still pass.
- A8. The committed napi glue and the five wasm artifacts are byte-identical to `origin/main`.
- A9. A follow-up Linear issue for the removal exists (§ 8).

## 6. Tests

### 6.1 The tag reaches a consumer (A1, A2)

A new kernel test, `ts/packages/paigasus-kernel/tests/deprecated-accessors.test.ts`:

- Make an in-memory TypeScript program with the TypeScript compiler API. Use the kernel's
  `tsconfig.json` options. The source imports the six accessors and `prnParse` from
  `@paigasus/kernel`, and in a second file from `@paigasus/kernel/napi`.
- Use a language service and `getSuggestionDiagnostics`. Collect the diagnostics with code 6385
  ("'{0}' is deprecated").
- Assert that each of the 12 accessor references (6 names × 2 entries) has one 6385 diagnostic.
- Assert that `prnParse` has none.
- Assert that the JSDoc text of each of the 12 names contains `prnParse`.

Red-first: before § 4.1 is in place, the 12 accessor assertions fail. The plan records the
measured failure.

### 6.2 The lint rule (A3–A6)

New rows in `ts/packages/paigasus-next-config/tests/`, in a new file
`kernel-accessor-rules.test.ts`. It uses the `restrictedImportsFor` style from `boundaries.test.ts`,
with `tseslint.plugin` and `tseslint.parser` added to the override config.

- DENIED rows: each of the six names from each of the four specifiers (24 rows); an aliased import
  (`import { prnOrg as org } from '@paigasus/kernel'`); a re-export
  (`export { prnService } from '@paigasus/kernel'`); a `.tsx` file in an app.
- ALLOWED rows: `prnParse`, `prnBuild`, `prnCanonicalize` from each specifier; an accessor import in
  each of the three exempt paths.
- Liveness row: lint one DENIED source through the real `ts/eslint.config.js` (no
  `overrideConfigFile`). It must report the rule. This proves the spread (A6).
- Red-first for the liveness row: remove the spread from `ts/eslint.config.js`, run the test,
  record the failure, then restore the spread.

`paigasus-next-config-ts:test` already lists `/ts/eslint.config.js` as an input, so a change to
the spread selects this test.

### 6.3 Regression (A7, A8)

- `moon run ts:lint paigasus-kernel-ts:test paigasus-next-config-ts:test paigasus-console-core-ts:test`.
- `git diff --exit-code origin/main -- rs/crates/bindings/` is empty.
- `moon run paigasus-kernel-ts:typecheck` passes (the `const` re-exports type-check).

## 7. Limits (known, not fixed)

- L1. The rule does not catch a namespace import (`import * as k from '@paigasus/kernel'`, then
  `k.prnService`). It does not catch a dynamic `import()` either. The `@deprecated` tag still shows a
  strikethrough in an editor for `k.prnService`.
- L2. The rule checks only `.ts`, `.tsx`, `.mts` and `.cts` files. A `.js` or `.mjs` file can still
  import an accessor. `wasm-probe.mjs` does so on purpose.
- L3. The raw binding packages `@paigasus/node-bindings` and `@paigasus/wasm` carry no
  `@deprecated` tag in their generated `.d.ts`. The lint rule covers their imports (§ 4.2).
- L4. The Python accessors stay and carry no mark (SMA-673 Q1).
- L5. `contracts/proto/paigasus/common/v1/actor.proto:16` names `prnResourceType` as the TS/JS way to
  read a resource type. The comment stays. A change means a codegen run in three languages for one
  comment. The follow-up removal issue (§ 8) updates it.
- L6. A new exempt path needs an edit to `kernelAccessorRules`. An `eslint-disable` comment also
  works, and nothing forbids it.

## 8. Follow-up

File one Linear issue: remove the six accessors from the `@paigasus/kernel` TS surface. Decide in
that issue whether the Rust napi and wasm bindings also drop them. That changes the wasm binary, the
`EXPECTED_EXPORTS` list in `committed-wasm.test.ts`, the napi glue and `binding-parity.types.ts`.
Update `actor.proto` (L5) in the same issue. Fields: project Paigasus Polyglot, milestone Frontend,
priority Low, labels `area:frontend`, `area:ffi`, Improvement, related to SMA-725.

## 9. Out of scope

- The Rust kernel and the Rust binding crates.
- The Python binding.
- The 12 unrelated `@typescript-eslint/no-deprecated` findings (§ 2).
