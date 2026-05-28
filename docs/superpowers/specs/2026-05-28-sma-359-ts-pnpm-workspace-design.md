# SMA-359 — Bootstrap `ts/` pnpm workspace with ESLint + Prettier

**Status:** Designed (brainstorming complete; design review pass incorporated 2026-05-28)
**Date:** 2026-05-28
**Linear:** [SMA-359](https://linear.app/smaschek/issue/SMA-359/bootstrap-ts-pnpm-workspace-with-eslint-prettier-config)
**Branch:** `feature/sma-359-bootstrap-ts-pnpm-workspace-with-eslint-prettier-config`
**References:** ADR-0009 (ESLint + Prettier over Biome); SMA-358 (py uv workspace — direct precedent
for topology + pinning + SPDX); SMA-384 (added `.moon/tasks/python.yml`; explicitly defers
`.moon/tasks/typescript.yml` to this issue); SMA-380 (`-py`/`-ts` Moon id suffix); SMA-381 (`layer:`
not `type:`); SMA-383 (Moon file field order, config-file SPDX carve-out). A design review pass
was incorporated; the disposition is recorded in §J.

## Goal

Scaffold the TypeScript workspace under `ts/` with the pnpm workspace conventions and the
toolchain decisions from ADR-0009: a single pnpm-workspace root holding shared dev tooling and all
tool config, four inert library stubs and two app stubs that are first-class Moon projects, and a
`ts` parent Moon project that runs the workspace-wide quality gates. Also wire the missing
`.moon/tasks/typescript.yml` (mirror of `.moon/tasks/python.yml`) and slim the typescript scaffold
template, both deferred from SMA-384 to this issue. No real package logic — bootstrapping only.

## Key decisions

1. **Moon topology — nested, mirroring py.** `ts/packages/*` and `ts/apps/*` are real Moon projects
   (identity, CODEOWNERS, future per-package `build`, affected-graph nodes), and a `ts` parent
   project owns the workspace-wide gates (`lint`/`fmt`/`typecheck`/`test`/`build`). ESLint, Prettier,
   and `tsc` resolve their config from the project they're invoked in and don't walk up parents
   reliably, so the only cwd where bare invocations see `ts/eslint.config.js`, `ts/.prettierrc.js`,
   and `ts/tsconfig.json` together is `ts/` itself — which must therefore be a project. Same
   reasoning as the py spec (§A1). Topology asymmetry across the three stacks is intentional; see
   §A.0.
2. **Wire `.moon/tasks/typescript.yml` here.** SMA-384 deferred this file to "SMA-359 (ts bootstrap),
   which is the right place to land it since the file would have no consumers until then." Without
   it, Moon's `moon ci :lint`/`:fmt`/`:typecheck`/`:test`/`:build` won't cover ts projects, and the
   existing typescript scaffold template's tasks-block would shadow any future inherited tasks.
3. **Slim the typescript scaffold template.** Matches what SMA-384 did to the python template: header
   (`$schema` → `id` → `layer` → `language`) plus an app-archetype-only override for the `build`
   command (`next build` + `.next` outputs). Library archetype gets bare header.
4. **Pinned, bounded devDependencies via pnpm Catalog.** Centralize all shared versions in
   `ts/pnpm-workspace.yaml`'s `catalog:` block; per-package `package.json` references them as
   `"<dep>": "catalog:"`. Single bump-point; same intent as py's bounded version constraints. The
   catalog uses caret-bounded ranges (`^X.Y.Z`); `pnpm-lock.yaml` pins exact versions.
5. **ESLint 10 + the modern React-tooling stack.** ESLint 10.4.0 is the current stable (verified
   against the public npm registry on 2026-05-28). The plugin set is the 2026 modern stack:
   `@eslint-react/eslint-plugin` (flat-config-native, TS-aware) **in place of** the legacy
   `eslint-plugin-react` named in ADR-0009; `eslint-plugin-react-hooks` v7 (which folds in the React
   Compiler rule from its v6 line); `eslint-plugin-jsx-a11y` 6.10.x; and `typescript-eslint` v8 —
   whose 8.x line declares `eslint: "^8.57.0 || ^9.0.0 || ^10.0.0"` as its peer range, so ESLint 10
   is supported. This deviates from ADR-0009's named plugin set; see §K for the ADR amendment plan.
6. **React rules glob-scoped to JSX/TSX only.** The base flat config has TS rules for all
   `**/*.{ts,tsx,mts,cts}` and a separate config object that turns on
   `@eslint-react`/`react-hooks`/`jsx-a11y` only for `**/*.{tsx,jsx}`. Non-React libraries
   (`@paigasus/proto`, `@paigasus/kernel`, `@paigasus/sdk`) get only TS rules — surgical, not lucky.
7. **Vitest now with `--passWithNoTests`.** The AC doesn't list a test runner, but skipping leaves
   `moon ci :test` not covering ts. Wire Vitest in the catalog and the inherited `test` task;
   `--passWithNoTests` keeps the empty workspace green. Drop the flag when the first test lands
   (mirrors py's SMA-379 future-delta). The cross-stack "empty test suite is success" pattern is
   now established in three forms and gets formalized as a CONTRIBUTING.md convention in a
   follow-up (see §H).
8. **Next.js 16 via `create-next-app` + trim.** The AC's "Next.js 15" is outpaced by current
   releases; we scaffold the `paigasus-console` app with `pnpm dlx create-next-app@16` and prune
   the parts that don't fit our conventions (Next-bundled ESLint, the default Tailwind/CSS assets,
   generated `README.md`/`.gitignore`). Hand-write only what we override.
9. **Source-as-entrypoint for now, with explicit downstream linkage.** Every library package's
   `package.json` declares `"exports": { ".": "./src/index.ts" }` and stays `private: true`. With
   `moduleResolution: "bundler"`, Next/Vitest/tsc all happily consume TS source at the workspace
   boundary; there is no `dist/` until a per-package `tsup` build lands post-MVP. **Footgun:**
   anything that runs raw Node (CLI tools, Lambda handlers, simple scripts) will fail on `.ts`
   imports with `ERR_UNKNOWN_FILE_EXTENSION`, and once `private: true` flips, external consumers
   of the published package see whatever is in `exports` — so the export-map flip MUST happen in
   lockstep with the tsup wiring. Tracked as a single linked task in §H.
10. **Cross-stack line-length bump to 200.** Departure from the AC's `printWidth: 100`. Applied
    everywhere — `ts/.prettierrc.js`, `py/pyproject.toml` (`tool.ruff.line-length`), and a new
    `rs/rustfmt.toml` (`max_width = 200`). **Rationale:** maintainer preference for wider readable
    lines on current monitor setups (2026); the polyglot stays consistent at a single number
    (preserving the current cross-stack-uniform-at-100 invariant, just shifted upward). Grouped
    into its own commit ahead of the ts scaffold so subsequent ts files are formatted at the new
    width from the start. The decision to land cross-stack changes in the same PR (rather than a
    standalone "standardize line length" Linear issue) is deliberate: keeping the polyglot
    invariant intact is the whole point of the bump; splitting risks landing TS-at-200 + py/rs-at-100
    on main even briefly.
11. **Moon project IDs use the full `paigasus-<name>` directory prefix.** Cross-stack consistency
    with rs (`paigasus-kernel-rs`) and py (`paigasus-kernel-py`): TS directories under `packages/`
    and `apps/` use the `paigasus-<name>` form on disk, so Moon's derived id (`{{name}}-ts`) yields
    `paigasus-kernel-ts`, `paigasus-proto-ts`, `paigasus-sdk-ts`, `paigasus-ui-ts`,
    `paigasus-console-ts`, `paigasus-docs-ts`. npm package names stay `@paigasus/<short>` (pnpm
    doesn't care that the directory name differs from the package name), so consumers see clean
    `@paigasus/kernel` imports while Moon graph queries use the uniform `paigasus-<name>-<stack>`
    pattern. This is the design-review S1 fix.

## A. Topology and Moon wiring

### A.0 Topology asymmetry across the three stacks — by design

After SMA-359 lands the three stacks have intentionally different topologies. Recorded here so
future contributors don't read it as historical drift:

| Stack | Parent project? | Per-language inherited tasks file |
| --- | --- | --- |
| rs | No (leaves only) | `.moon/tasks/rust.yml` |
| py | Yes (`py/moon.yml`, `layer: configuration`) | `.moon/tasks/python.yml` |
| ts | Yes (`ts/moon.yml`, `layer: configuration`) | `.moon/tasks/typescript.yml` (this PR) |

**Why the asymmetry:** rs can be leaves-only because `cargo` walks up to find the workspace root
reliably — invoking `cargo build` from any crate dir or from the workspace root produces sensible
behavior. py and ts can't: `ruff`/`basedpyright`/`pytest` (and `eslint`/`prettier`/`tsc`) resolve
config from cwd and don't walk up the way cargo does. So workspace-wide invocations need a project
*at* the workspace root, which becomes the parent project. The asymmetry is permanent (no path
back to symmetry without a tooling change in one stack or the other), so the convention is: read
each stack on its own terms.

### A.1 `.moon/workspace.yml` — register `'ts'` as a project

Add `'ts'` to `projects`, alongside the existing `'ts/packages/*'` and `'ts/apps/*'`. Moon permits a
project whose source contains nested project sources (the root-level-project pattern that py uses).
CODEOWNERS regenerates via `codeowners.sync`; do not hand-edit `.github/CODEOWNERS`.

### A.2 `ts/moon.yml` — the parent project

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

layer: 'configuration'
language: 'typescript'

# The inherited fileGroups from .moon/tasks/typescript.yml assume src/ at the project root.
# The ts workspace keeps sources under packages/*/src and apps/*/{src,app}, so extend the
# inherited groups here. Moon merges (not overrides) fileGroups across the layers, so the
# resolved @group(sources) and @group(tests) contain both typescript.yml's defaults and these
# additions — fine in practice because ts/src/ and ts/tests/ don't exist; only the package/app
# subdirs actually match. (Same merge semantics confirmed for py in SMA-384 Correction 2.)
fileGroups:
  sources:
    - 'packages/*/src/**/*'
    - 'apps/*/src/**/*'
    - 'apps/*/app/**/*'
  tests:
    - 'packages/*/tests/**/*'
    - 'apps/*/tests/**/*'

# Override the inherited per-project commands for the two tasks that need a workspace-wide
# entry point. typecheck fans out via pnpm recursion (each package owns a `typecheck` script).
# build fans out the same way; packages without a `build` script no-op cleanly. The inherited
# lint/fmt/test commands (`pnpm exec eslint .`, `pnpm exec prettier --check .`,
# `pnpm exec vitest run --passWithNoTests`) work from `ts/` cwd unchanged — they walk from
# cwd to find their config and pick up the whole tree.
tasks:
  typecheck:
    command: 'pnpm -r --if-present run typecheck'
    inputs: ['@group(sources)', '@group(tests)', 'tsconfig.base.json', 'package.json', 'pnpm-workspace.yaml', 'pnpm-lock.yaml']
  build:
    command: 'pnpm -r --if-present run build'
    inputs: ['@group(sources)', 'tsconfig.base.json', 'package.json', 'pnpm-workspace.yaml', 'pnpm-lock.yaml']
```

### A.3 `.moon/tasks/typescript.yml` — new file

Mirror `.moon/tasks/python.yml`'s shape exactly. Task name `fmt` (not `format`) harmonizes with
rust/py so `moon ci :fmt` covers all three stacks. Commands invoke `pnpm exec <tool>` so they
resolve from `ts/node_modules` deterministically regardless of Moon's node toolchain shimming.

```yaml
$schema: 'https://moonrepo.dev/schemas/tasks.json'

# Moon does NOT scope task files by filename — scope explicitly so these TypeScript
# commands only attach to typescript projects (not rust, python, or contracts).
inheritedBy:
  languages: ['typescript']

fileGroups:
  sources:
    - 'src/**/*'
  tests:
    - 'tests/**/*'
    - '**/*.test.ts'
    - '**/*.test.tsx'
    - '**/*.spec.ts'
    - '**/*.spec.tsx'

tasks:
  build:
    # No `outputs:` — tsc --noEmit produces no files; Moon's per-project output check would fail
    # (same shape as SMA-384 Correction 1 for `uv build`). Cache invalidation runs off `inputs:`.
    # Apps that produce artifacts (Next.js) override this task in their own moon.yml.
    command: 'pnpm exec tsc -p tsconfig.json --noEmit'
    inputs: ['@group(sources)', 'tsconfig.json', 'package.json', '/ts/tsconfig.base.json', '/ts/pnpm-lock.yaml']
  lint:
    command: 'pnpm exec eslint .'
    inputs: ['@group(sources)', '@group(tests)', 'eslint.config.js', 'package.json', '/ts/eslint.config.js', '/ts/pnpm-lock.yaml']
  fmt:
    command: 'pnpm exec prettier --check .'
    inputs: ['@group(sources)', '@group(tests)', '.prettierrc.js', '.prettierignore', 'package.json', '/ts/.prettierrc.js', '/ts/.prettierignore', '/ts/pnpm-lock.yaml']
  typecheck:
    command: 'pnpm exec tsc -p tsconfig.json --noEmit'
    inputs: ['@group(sources)', 'tsconfig.json', 'package.json', '/ts/tsconfig.base.json', '/ts/pnpm-lock.yaml']
  test:
    command: 'pnpm exec vitest run --passWithNoTests'
    inputs: ['@group(sources)', '@group(tests)', 'package.json', 'vitest.config.ts', '/ts/pnpm-lock.yaml']
```

Workspace-anchor paths (`/ts/...`) ensure a change to the root configs busts every ts project's
cache; local relative paths (`eslint.config.js`, `.prettierrc.js`) catch the per-package overlay
files where they exist. Both forms together cover the full inheritance chain.

Notes:

- **`build` is library-flavored** (`tsc --noEmit`). The `paigasus-console` app overrides this in
  its own `moon.yml` to `next build` with `outputs: ['.next']`. `paigasus-docs` keeps the inherited
  no-op build until its framework is chosen.
- **`/ts/pnpm-lock.yaml`** uses Moon's workspace-anchor syntax — every ts project shares the single
  `ts/pnpm-lock.yaml`. Same shape as `/py/uv.lock` in `python.yml`.
- **`test`'s `--passWithNoTests` flag** keeps the empty workspace green; tracked for removal in §H.

### A.4 `.moon/tasks.yml` — register the new file as an implicit input

Add `'/.moon/tasks/typescript.yml'` to the `implicitInputs` list so a change to typescript.yml busts
every ts task's cache.

```yaml
implicitInputs:
  - '/.moon/toolchain.yml'
  - '/.moon/tasks.yml'
  - '/.moon/tasks/rust.yml'
  - '/.moon/tasks/python.yml'
  - '/.moon/tasks/typescript.yml'   # added
```

### A.5 `.moon/templates/typescript/moon.yml` — slim down

Matches the python template's post-SMA-384 shape, with one shape difference: TS has an `app`
archetype (Next.js) where the build command genuinely differs from the library default. Python's
template has a `service` archetype that defines `start`; TS's template has an `app` archetype that
overrides `build`.

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: '{{ name }}-ts'
layer: '{% if archetype == "app" %}application{% else %}library{% endif %}'
language: 'typescript'
{%- if archetype == "app" %}
tasks:
  build:
    command: 'next build'
    inputs: ['@group(sources)', 'tsconfig.json', 'package.json', 'next.config.ts']
    outputs: ['.next']
    options:
      merge: replace
{%- endif %}
```

After this, library-archetype renders produce a 4-line `moon.yml` (header only); app archetype adds
the `tasks:` block with the `build` override. No `test`/`lint`/`fmt`/`typecheck` definitions remain
in the template — they all come from the inherited `.moon/tasks/typescript.yml`.

**`options: merge: replace` is required**, not optional. Moon 2.2.5's default task-merge semantics
combine a local `command:` with the inherited task's `command:`, which for the inherited
`pnpm exec tsc -p tsconfig.json --noEmit` + the local `next build` produces a corrupted
`next exec tsc -p tsconfig.json --noEmit build` invocation. `merge: replace` forces Moon to
fully replace the inherited task definition with the local one. Discovered during Phase 3
smoke-testing and applied uniformly to every per-project app-archetype `build` override (template
+ hand-written `paigasus-console/moon.yml`).

### A.6 Per-package and per-app `moon.yml`

Minimal — `id`, `layer`, `language`. Tasks come from inheritance except where overridden (only the
Next.js `paigasus-console` app needs an override; see §D).

## B. Workspace-root files under `ts/`

### B.1 `ts/pnpm-workspace.yaml`

Versions verified against the public npm registry on 2026-05-28; `^X.Y.Z` ranges cap the upgrade
window, `pnpm-lock.yaml` pins exact resolved versions.

```yaml
packages:
  - 'packages/*'
  - 'apps/*'

# Catalog — single bump-point for shared dep versions across the workspace.
# Per-package package.json references these as "<dep>": "catalog:".
catalog:
  # Runtime (React + Next)
  react: ^19.1.0
  react-dom: ^19.1.0
  next: ^16.0.0
  # TypeScript + ESLint + Prettier
  typescript: ^5.7.0
  eslint: ^10.4.0
  '@eslint/js': ^10.4.0
  '@eslint-react/eslint-plugin': ^5.8.0
  'eslint-plugin-react-hooks': ^7.1.0
  'eslint-plugin-jsx-a11y': ^6.10.2
  'typescript-eslint': ^8.60.0
  prettier: ^3.4.0
  # Test runner
  vitest: ^3.0.0
  # Per-package add-ons (catalog-listed so they bump centrally even when used in only one app)
  '@next/eslint-plugin-next': ^16.2.0
  '@tanstack/eslint-plugin-query': ^5.62.0
```

`typescript-eslint` is intentionally v8 (not v9 — v9 doesn't exist yet); its 8.x peer-range
`"^8.57.0 || ^9.0.0 || ^10.0.0"` supports ESLint 10 directly. `@eslint-react/eslint-plugin` is at
v5 (the legacy `eslint-plugin-react` of ADR-0009 is replaced wholesale; see §K). Exact lower
bounds get re-verified at implementation time and locked by `pnpm-lock.yaml`.

### B.2 `ts/package.json`

```json
{
  "name": "@paigasus/workspace",
  "private": true,
  "type": "module",
  "engines": { "node": ">=22" },
  "scripts": {
    "lint": "eslint .",
    "format": "prettier --check .",
    "format:write": "prettier --write .",
    "typecheck": "pnpm -r --if-present run typecheck",
    "test": "vitest run --passWithNoTests"
  },
  "devDependencies": {
    "typescript": "catalog:",
    "eslint": "catalog:",
    "@eslint/js": "catalog:",
    "@eslint-react/eslint-plugin": "catalog:",
    "eslint-plugin-react-hooks": "catalog:",
    "eslint-plugin-jsx-a11y": "catalog:",
    "typescript-eslint": "catalog:",
    "prettier": "catalog:",
    "vitest": "catalog:"
  }
}
```

`scripts.typecheck` runs each package's own `typecheck` script recursively. Every package/app
declares `"scripts": { "typecheck": "tsc -p tsconfig.json --noEmit" }` so the workspace-level
invocation fans out cleanly. **No TS solution file** — we considered one with `references:` to drive
a single `tsc --build`, but Next.js apps can't be `composite: true` (it conflicts with their
`noEmit: true` + their own incremental build), and mixing composite libraries with non-composite
apps in one solution is messy. Recursive per-package typecheck is simpler and just as fast in
practice on a small workspace.

### B.3 `ts/tsconfig.base.json`

The `compilerOptions` every package extends. AC-enumerated flags plus two modest additions that go
beyond the AC (`noFallthroughCasesInSwitch`, `noImplicitReturns`), in the same spirit as py's
beyond-minimum basedpyright config.

```jsonc
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "lib": ["ES2022"],
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "verbatimModuleSyntax": true,
    "noImplicitOverride": true,
    "exactOptionalPropertyTypes": true,
    "noFallthroughCasesInSwitch": true,
    "noImplicitReturns": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "isolatedModules": true,
    "resolveJsonModule": true,
    "forceConsistentCasingInFileNames": true
  }
}
```

### B.4 No solution-file tsconfig — recursive typecheck instead

We **do not** ship a `ts/tsconfig.json` solution file. See §B.2's note: Next.js apps can't be
`composite: true`, and mixing composite libraries with non-composite apps in one solution is messy.
The workspace-level typecheck (`pnpm -r run typecheck`) fans out to each package's own
`typecheck` script, which calls `tsc -p tsconfig.json --noEmit` from that package's cwd. Tracked
as a possible future revisit in §H if the workspace grows to ~20+ packages.

### B.5 `ts/eslint.config.js`

Three layered objects: ignore globs, TS rules across all `*.{ts,tsx,mts,cts}`, React-specific rules
glob-scoped to `**/*.{tsx,jsx}` only.

```js
// SPDX-License-Identifier: Apache-2.0
import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import reactPlugin from '@eslint-react/eslint-plugin';
import reactHooks from 'eslint-plugin-react-hooks';
import jsxA11y from 'eslint-plugin-jsx-a11y';

export default tseslint.config(
  { ignores: ['**/dist/**', '**/.next/**', '**/node_modules/**', '**/*.d.ts'] },
  js.configs.recommended,
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
);
```

**Expect tuning when real code lands.** `recommendedTypeChecked` enables type-aware rules
(`no-floating-promises`, `no-misused-promises`, the `no-unsafe-*` family) that are famously noisy
on real app code that touches untyped libraries, implicit `Promise<void>` returns, or `any`-typed
boundary types. On the empty stubs no rule fires; on the first real PR with non-trivial code, the
team should expect to downgrade the noisiest rules to `warn` rather than chase every finding to
`error`-clean. Tracked in §H.

**Perf cost.** Type-aware linting requires a full TS program. `projectService: true` resolves
projects on the fly, which is instant on the empty workspace and stays acceptable at small package
counts; at ~20+ packages with real code, `pnpm lint` time becomes noticeable. Also tracked in §H.

### B.6 `ts/.prettierrc.js`

```js
// SPDX-License-Identifier: Apache-2.0
/** @type {import('prettier').Config} */
export default {
  printWidth: 200,
  semi: true,
  singleQuote: true,
  trailingComma: 'all',
  arrowParens: 'always',
};
```

### B.7 `ts/.prettierignore`

```
dist
.next
node_modules
pnpm-lock.yaml
coverage
```

## C. The four stub packages

`@paigasus/proto`, `@paigasus/kernel`, `@paigasus/sdk`, `@paigasus/ui` (the npm names). Each lives
under a `paigasus-<short>` directory (per decision §11) to keep Moon ids consistent across the
three stacks:

```text
ts/packages/<paigasus-name>/         # e.g. paigasus-proto, paigasus-kernel, paigasus-sdk, paigasus-ui
├── moon.yml                          # id 'paigasus-<short>-ts', layer 'library', language 'typescript'
├── package.json                      # npm name '@paigasus/<short>'
├── tsconfig.json                     # extends ../../tsconfig.base.json
└── src/
    └── index.ts                      # SPDX header + `export {};`
```

### C.1 `package.json` template

`@paigasus/proto` shown; the others are identical except for `name` and (for `@paigasus/ui`)
peerDependencies. The npm package name uses the `@paigasus/<short>` form while the directory
carries the full `paigasus-<short>` prefix — pnpm doesn't require name/dir alignment.

```json
{
  "name": "@paigasus/proto",
  "_comment_exports": "Source-only exports. Bundler-aware consumers only (Next/Vitest/tsc walk through TS via moduleResolution: bundler). Switch to ./dist/index.js when tsup wiring lands — must happen IN LOCKSTEP with flipping `private: false` for publishable packages.",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "license": "Apache-2.0",
  "exports": { ".": "./src/index.ts" },
  "scripts": { "typecheck": "tsc -p tsconfig.json --noEmit" },
  "devDependencies": { "typescript": "catalog:" }
}
```

(`_comment_exports` is a benign extra field — npm/pnpm ignore unknown top-level keys, so it
survives `pnpm install` and renders as visible documentation for the next reader. JSON doesn't
support comments, so this is the closest we can get to inline annotation.)

`@paigasus/ui` additionally carries React peers:

```json
"peerDependencies": { "react": "catalog:", "react-dom": "catalog:" }
```

**Note on `peerDependencies: { ... "catalog:" }`:** pnpm supports catalog refs in peer dependencies
on recent versions (9.x and later). Pinned pnpm in this repo is 11.3.0 (per
`.moon/toolchain.yml`), which is well past that line — should work. Verified during
implementation; if a regression appears, fall back to explicit version strings for peer deps and
document the carve-out.

`@paigasus/sdk` stays React-free (public SDK is not a React lib).

`TODO(SMA-NNN)` placeholders in §H telegraph the publish-time prerequisites (drop `private: true`,
add `description`/`repository`/`homepage`); the actual TODO note lives in the workspace README
(single source of truth).

### C.2 `tsconfig.json` template

```jsonc
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "outDir": "./dist",
    "rootDir": "./src",
    "noEmit": true
  },
  "include": ["src/**/*"]
}
```

`outDir`/`rootDir` are stated so a future `tsup`/`tsc --build` setup has the right defaults already
in place. `noEmit: true` since the bootstrap doesn't produce JS output — the package's `typecheck`
script (declared in §C.1's `package.json`) runs this tsconfig with `--noEmit` redundantly to be
explicit. No `composite: true` — we dropped the solution-file pattern (§B.4).

### C.3 `src/index.ts` for every stub

```ts
// SPDX-License-Identifier: Apache-2.0

export {};
```

Minimal valid ESM module satisfying `verbatimModuleSyntax` and giving ESLint/tsc a file to chew on.

### C.4 `moon.yml` per package

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-proto-ts'
layer: 'library'
language: 'typescript'
```

(Identical shape for `paigasus-kernel-ts`, `paigasus-sdk-ts`, `paigasus-ui-ts`.)

**Forward-looking roles** (no code yet, stated in the README):

- `@paigasus/proto` — generated proto types post-MVP (consumes `contracts/`). Once SMA-360
  (contracts bootstrap) lands, this grows runtime deps for the bufbuild/es generated-code runtime
  (`@bufbuild/protobuf` + `@connectrpc/connect`, per Polyglot Monorepo Scoping § 2). Telegraphed in
  §H.
- `@paigasus/kernel` — thin wrapper over the napi-rs binding to `paigasus-kernel-rs`, post-MVP
- `@paigasus/sdk` — public SDK placeholder
- `@paigasus/ui` — shared React components for the console

## D. The two apps

### D.1 `ts/apps/paigasus-console/` — Next.js 16 (App Router)

Generated via:

```bash
pnpm dlx create-next-app@16 ts/apps/paigasus-console \
  --ts --app --tailwind=false --src-dir=false --import-alias='@/*' \
  --use-pnpm --eslint=false --turbopack
```

Then trimmed. Final layout:

```text
ts/apps/paigasus-console/
├── moon.yml            # id 'paigasus-console-ts', layer 'application', language 'typescript'
├── package.json        # next + react + react-dom from catalog; private; type: module
├── tsconfig.json       # extends ../../tsconfig.base.json, plus Next's required overrides
├── next.config.ts      # minimal — empty config object
├── next-env.d.ts       # Next's TS shim (auto-managed; never hand-edited; SPDX exempt — see §E.1)
├── eslint.config.js    # extends root, adds @next/eslint-plugin-next
└── app/
    ├── layout.tsx      # SPDX + minimal HTML shell
    └── page.tsx        # SPDX + "Paigasus console" h1
```

**Files deleted from the CLI output:**

- `README.md` (the workspace README covers it)
- `.gitignore` (covered by root `.gitignore`)
- `public/` (default favicon/svg — unneeded for a stub)
- `app/favicon.ico`, `app/globals.css`, any styling from the Tailwind-disabled scaffold
- Any `.eslintrc*` or ESLint config Next 16 scaffolds despite `--eslint=false`

**`package.json`** (catalog-driven):

```json
{
  "name": "@paigasus/console",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "next dev",
    "build": "next build",
    "start": "next start",
    "typecheck": "tsc -p tsconfig.json --noEmit"
  },
  "dependencies": {
    "next": "catalog:",
    "react": "catalog:",
    "react-dom": "catalog:"
  },
  "devDependencies": {
    "typescript": "catalog:",
    "@next/eslint-plugin-next": "catalog:"
  }
}
```

**`tsconfig.json`** — Next requires several overrides; we keep `extends` but layer them:

```jsonc
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "lib": ["DOM", "DOM.Iterable", "ES2022"],
    "jsx": "preserve",
    "incremental": true,
    "plugins": [{ "name": "next" }],
    "paths": { "@/*": ["./*"] },
    "noEmit": true
  },
  "include": ["next-env.d.ts", "**/*.ts", "**/*.tsx", ".next/types/**/*.ts"],
  "exclude": ["node_modules"]
}
```

No `composite: true` — Next manages its own incremental builds via `.next/`, and `composite`
conflicts with `noEmit: true`. The decision to drop the workspace solution file (§B.4) means this
isn't needed anyway.

**`eslint.config.js`** per Next app:

```js
// SPDX-License-Identifier: Apache-2.0
import root from '../../eslint.config.js';
import nextPlugin from '@next/eslint-plugin-next';

export default [
  ...root,
  {
    files: ['**/*.{ts,tsx}'],
    plugins: { '@next/next': nextPlugin },
    rules: { ...nextPlugin.configs.recommended.rules },
  },
];
```

**`moon.yml`** (overrides the inherited `build`; `options: merge: replace` is required — see §A.5
for the rationale):

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-console-ts'
layer: 'application'
language: 'typescript'

tasks:
  build:
    command: 'next build'
    inputs: ['@group(sources)', 'tsconfig.json', 'package.json', 'next.config.ts']
    outputs: ['.next']
    options:
      merge: replace
```

**`app/layout.tsx`** — minimal HTML shell:

```tsx
// SPDX-License-Identifier: Apache-2.0
import type { ReactNode } from 'react';

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
```

**`app/page.tsx`**:

```tsx
// SPDX-License-Identifier: Apache-2.0
export default function Page() {
  return <h1>Paigasus console</h1>;
}
```

### D.2 `ts/apps/paigasus-docs/` — placeholder

Framework TBD per the AC. Skeleton matches a library package, and the Moon `layer` is **`library`**
(not `application`) until the framework is chosen — an empty placeholder with `src/index.ts` of
`export {};` isn't really an application yet, and `library` is a more honest label until the
framework decision lands.

```text
ts/apps/paigasus-docs/
├── moon.yml            # id 'paigasus-docs-ts', layer 'library' (flip to 'application' with framework), language 'typescript'
├── package.json        # name '@paigasus/docs', private, type module, no deps
├── tsconfig.json       # extends ../../tsconfig.base.json
└── src/
    └── index.ts        # SPDX + export {};
```

`moon.yml` is bare-header — no task override. The inherited `build` is `tsc -p tsconfig.json
--noEmit`, which succeeds on the empty stub. The workspace README carries a TODO note pointing
to a follow-up issue for picking the docs framework (tracked in §H).

`package.json`:

```json
{
  "name": "@paigasus/docs",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "scripts": { "typecheck": "tsc -p tsconfig.json --noEmit" },
  "devDependencies": { "typescript": "catalog:" }
}
```

`tsconfig.json` matches the library template (§C.2):

```jsonc
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "outDir": "./dist",
    "rootDir": "./src",
    "noEmit": true
  },
  "include": ["src/**/*"]
}
```

## E. Conventions, vitest "0 tests" handling, README, cross-stack line-length bump

### E.1 SPDX headers

Every `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs` source file starts with:

```ts
// SPDX-License-Identifier: Apache-2.0
```

That includes `eslint.config.js`, `.prettierrc.js`, `next.config.ts`, and every `src/index.ts` /
`app/*.tsx`. **Config files in JSON/YAML/TOML carry no header** (matches the SMA-383 carve-out in
CONTRIBUTING.md): no SPDX on `package.json`, `tsconfig.*.json`, `pnpm-workspace.yaml`, `moon.yml`.

**Generator-managed files are also SPDX-exempt.** `next-env.d.ts` is regenerated by Next on every
`next build`/`next dev`, so any hand-added header would be clobbered and reappear in diff churn —
useless and noisy. The same logic applies to future generator output (proto codegen, tsup `.d.ts`
emit). The current CONTRIBUTING.md SPDX carve-out documents "config files"; refining it to also
name "generator-managed files" is tracked in §H (likely SMA-383's follow-up).

### E.2 Vitest "0 tests" handling

Vitest exits non-zero when no tests are collected unless `--passWithNoTests` is set, same shape as
`cargo nextest --no-tests=pass` (rust) and py's `conftest.py` exit-5 shim. The inherited `test`
task in `.moon/tasks/typescript.yml` ships with `--passWithNoTests`. The cross-stack convention
("empty test suite is success during bootstrap") is now established in three different forms;
formalizing it as a CONTRIBUTING.md note is tracked in §H. Removal of `--passWithNoTests` mirrors
py's SMA-379 future-delta.

### E.3 `ts/README.md`

Rewritten from the current "Empty until the pnpm workspace lands" to describe the real layout.
Mirrors `py/README.md` structure:

- Layout description (`packages/`, `apps/`, the workspace files at root, the catalog)
- Commands table:

  | Task        | Command                              |
  | ----------- | ------------------------------------ |
  | Lint        | `moon run ts:lint`                   |
  | Format      | `moon run ts:fmt`                    |
  | Type check  | `moon run ts:typecheck`              |
  | Test        | `moon run ts:test`                   |
  | Build (lib) | `moon run ts:build`                  |
  | Build (app) | `moon run paigasus-console-ts:build` |

- Operator notes:
  - Invoke pnpm via `moon run ts:<task>` for env parity (Moon's pinned Node, not whatever's on PATH)
  - Per-package install: `pnpm --filter @paigasus/<name> add <dep>`
  - Catalog block in `pnpm-workspace.yaml` is the single bump-point for shared dep versions
  - `@paigasus/proto`, `@paigasus/kernel`, `@paigasus/sdk`, `@paigasus/ui` ship `private: true`
    until first publish (drop the flag + add `description`/`repository`/`homepage` then)
- Status: "bootstrapped in SMA-359; packages are empty stubs"

### E.4 Cross-stack line-length bump to 200

Applied everywhere in the same issue so the polyglot stays at a single number (preserving the
pre-bump cross-stack-uniform invariant, just shifted upward):

- `ts/.prettierrc.js` → `printWidth: 200` (this issue's own file)
- `py/pyproject.toml` → `[tool.ruff] line-length = 200` (was `100`)
- New `rs/rustfmt.toml` → `max_width = 200` (rustfmt's default is 100; no existing config file)

**Rationale:** maintainer preference for wider readable lines on current monitor setups (2026).
The polyglot consistency argument runs both ways — if we left py/rs at 100 and bumped only TS, we'd
ship a real cross-stack inconsistency that future contributors have to memorize. Same-PR migration
keeps the invariant intact and avoids a transition window where main has mixed widths. Grouped
into its own commit (`chore(repo): widen line length to 200 across rs/py/ts`) so the diff is
reviewable independent of the ts scaffold and so subsequent ts files are written at the new width
from the start.

**No reformat sweep of existing py/rs source** — the workspaces are essentially empty (the py
spec's amended AC, and the rs crates that are bootstrapped but contain no library code yet). If a
file in py/rs happens to exceed the new width when actually formatted later, that's a no-op change;
no need to chase it now.

## F. Verification (maps to acceptance criteria)

| Acceptance criterion (AC, with amendments) | Verification |
| --- | --- |
| `ts/pnpm-workspace.yaml` with `packages/*` + `apps/*` + catalog | File present (§B.1) |
| `ts/package.json` with workspace metadata + shared scripts + devDeps | File present (§B.2) |
| Four stub packages with `package.json`, `tsconfig.json`, `src/index.ts` | Files present (§C); dir names use the `paigasus-<short>` prefix (§11, §C) |
| Two stub apps (paigasus-console = Next 16, paigasus-docs = placeholder) — **AC amended:** Next 16, not 15 | Files present (§D) |
| `ts/tsconfig.base.json` with all enumerated flags | File present (§B.3); two non-AC additions (`noFallthroughCasesInSwitch`, `noImplicitReturns`) documented |
| `ts/eslint.config.js` (flat) — **AC amended:** ESLint 10 + `@eslint-react/eslint-plugin` (replaces legacy `eslint-plugin-react`); typescript-eslint v8 (its 8.x supports ESLint 10 via peer range); react-hooks v7 | File present (§B.5); ADR-0009 amendment plan in §K |
| `ts/.prettierrc.js` — **AC amended:** `printWidth: 200` (cross-stack), not `100` | File present (§B.6) |
| All packages `"type": "module"` (ESM) | Verified via §C/§D templates |
| `pnpm install` succeeds | `pnpm install` from `ts/` exits 0; `pnpm-lock.yaml` written |
| `pnpm lint` passes on the empty workspace | `pnpm lint` from `ts/` exits 0 |
| `pnpm format --check` passes | `pnpm format` from `ts/` exits 0 |
| `pnpm tsc --noEmit` passes across the workspace | `pnpm typecheck` from `ts/` exits 0 (recursive per-package; §B.4) |
| (Beyond AC) Moon parity | `moon run ts:lint` / `:fmt` / `:typecheck` / `:test` / `:build` exit 0; `moon ci :lint :fmt :typecheck :test :build` exits 0 |
| (Beyond AC) `.moon/tasks/typescript.yml` resolved | `moon project paigasus-console-ts` and `moon project paigasus-proto-ts` report inherited tasks; no `unknown_file_group` errors |
| (Beyond AC) py + rs line-length bumped to 200 | `grep '^line-length' py/pyproject.toml` returns `200`; `cat rs/rustfmt.toml` shows `max_width = 200`; `moon run py:lint` and `moon ci :build` exit 0 |

Sanity: `moon ci :build` resolves the graph with `ts` + the four package projects + the two app
projects registered alongside the existing py/rs projects, no errors about overlapping or nested
sources.

## G. Out of scope

- Real package code, proto codegen wiring, the napi-rs binding (`@paigasus/kernel` wrapper), tsup
  per-package builds
- Per-package READMEs (added when packages get real code)
- Publishing-prerequisite metadata (`description`/`repository`/`homepage`) — tracked in §H for
  the first publish
- Lefthook git hooks (owned by SMA-371)
- Reformatting existing py/rs source for the new line length — those workspaces are essentially
  empty; reformat happens organically as files are touched
- The docs app's framework choice — TBD post-MVP

## H. Future deltas (telegraphed for downstream reviewers)

- **First package with tests:** drop `--passWithNoTests` from `.moon/tasks/typescript.yml`'s `test`
  task (mirrors py's SMA-379 future-delta)
- **Linked: first library build artifact AND publish-flip.** When a library is ready to publish,
  the same PR must:
  1. add `tsup` (per-package build artifact),
  2. switch `exports` from `./src/index.ts` to a conditional/dist pointer (and remove the
     `_comment_exports` docstring),
  3. flip `private: true` to `false`,
  4. add publishing metadata (`description`, `repository`, `homepage`, `keywords`),
  5. verify the published artifact via `npm pack` dry-run.
  
  These steps are dangerous out of order (a `private: false` flip without the export-map flip
  would publish a TS-source-only package that can't run in Node), so they're listed as one linked
  task, not a bullet list of independents.
- **`@paigasus/proto` grows runtime deps post-SMA-360.** Once contracts bootstrap lands, this
  package gains `@bufbuild/protobuf` + `@connectrpc/connect` runtime deps (per Polyglot Monorepo
  Scoping § 2's bufbuild/es codegen choice). Worth telegraphing so SMA-360's PR doesn't surprise
  reviewers with non-trivial dep additions to a previously-empty stub.
- **`@paigasus/kernel`/`@paigasus/proto` wiring:** consume the napi-rs binding output and the
  contracts codegen output once those land (separate SMA-NNNs).
- **`@paigasus/ui` first real component:** moves React from peerDeps into devDeps for local
  testing; `jsx-a11y` rules start mattering.
- **First Next.js console feature:** layout/page get real content; possible additions of
  `@tanstack/eslint-plugin-query` to the console's `eslint.config.js` if TanStack Query is used.
- **paigasus-docs app framework choice:** pick framework, expand `apps/paigasus-docs/` skeleton,
  flip `layer:` from `library` to `application`, possibly add a `build` task override.
- **`recommendedTypeChecked` tuning when real code lands.** Expect to downgrade `no-floating-promises`,
  `no-misused-promises`, and the `no-unsafe-*` family to `warn` on the first PR with real app
  code; chasing them all to `error`-clean tends to be more friction than value early on.
- **`tsc --build` solution-file revisit at ~20+ packages.** The recursive `pnpm -r` typecheck is
  fine at this scale; at large scale `tsc --build` shares the type-graph across packages and is
  meaningfully faster. Worth revisiting (likely with composite libraries + per-app independent
  typecheck) when typecheck time starts to bite.
- **Catalog-aware dependency bumping infrastructure.** The `"catalog:"` version ref is pnpm-only;
  Dependabot has historically not understood it without a custom workflow, and Renovate needs
  explicit pnpm-workspace.yaml parsing config. Whichever automation lands in SMA-362 (or a
  dedicated follow-up) needs catalog-aware config from day one, or version bumps to catalog
  entries become a manual chore that defeats the single-bump-point benefit.
- **Formalize the cross-stack "empty test suite is success" convention.** rs uses
  `cargo nextest --no-tests=pass`, py uses a `conftest.py` exit-5 shim, ts uses
  `vitest --passWithNoTests`. Three different forms of the same workaround. A short
  CONTRIBUTING.md section codifying the convention (and naming the per-language removal trigger:
  "drop the workaround when the first real test lands") would prevent the next bootstrap from
  having to rediscover the pattern.
- **SMA-383 SPDX carve-out refinement.** Add "generator-managed files" (e.g. `next-env.d.ts`,
  proto codegen output, tsup `.d.ts`) as a named category alongside "config files." Same SMA-383
  follow-up.

## I. Commit grouping

```
chore(repo): widen line length to 200 across rs/py/ts                (cross-stack tooling bump)
feat(ts):    add .moon/tasks/typescript.yml + register implicit input (Moon language tasks)
chore(ts):   slim typescript scaffold template (app override only)
chore(ts):   bootstrap ts/ pnpm workspace with ESLint + Prettier      (the actual scaffold)
```

The "widen line length" commit lands first so all subsequent prettier/ruff/rustfmt-formatted files
in the ts scaffold commit are at the new width from the start (no later reformat churn). The
`feat(ts):` and `chore(ts):` splits mirror the SMA-384 grouping pattern (behavior change separated
from template/scaffold cleanup).

The ADR-0009 amendment (§K) is a Notion-side change and is performed before the PR opens (same
shape as SMA-384's Notion redirect); it doesn't appear in this repo's commit log but is tracked
in §K and §J.

## J. Design review disposition

A design review pass was incorporated on 2026-05-28 (review document since removed; this section
preserves the outcome). Disposition:

**Blockers**
- **B1 (cross-stack 100 → 200 line-length scope).** Reviewer argued the cross-stack bump deserves a
  separate Linear issue. **Resolved: keep here, with rationale.** Decision §10 now documents the
  maintainer-preference rationale and the polyglot-uniform-invariant argument; landing the bump
  cross-stack in a single PR avoids a transition window where main holds mixed widths. Reviewer's
  concern about silent cascading is mitigated by the dedicated commit (§I) and the explicit
  rationale.
- **B2 (ESLint 10 release + ADR-0009 deviation).** Reviewer flagged "ESLint 10 may not be
  released." **Resolved: verified, false alarm.** ESLint 10.4.0 is the current stable on the public
  npm registry (verified 2026-05-28); `typescript-eslint` v8's peer range
  `"^8.57.0 || ^9.0.0 || ^10.0.0"` supports it. The spec's earlier "typescript-eslint v9+" claim
  was wrong (v9 doesn't exist); decision §5 and §B.1 now say v8. The ADR-0009 deviation is real
  and tracked in §K (amendment plan).

**Significant**
- **S1 (Moon ID asymmetry: `kernel-ts` vs `paigasus-kernel-rs`).** **Resolved: rename TS dirs.**
  Decision §11 + §C/§D updated. TS directories now carry the `paigasus-` prefix on disk; Moon
  ids become `paigasus-<short>-ts` uniformly. npm package names stay `@paigasus/<short>`.
- **S2 (pnpm catalog tooling support).** **Resolved: telegraphed in §H** — catalog-aware
  Dependabot/Renovate config noted as a SMA-362 (or follow-up) prereq.
- **S3 (source-as-entrypoint footgun).** **Resolved: strengthened.** Decision §9 now names the
  footgun explicitly (Node-runtime consumers, external publish risk). §C.1's package.json carries
  a `_comment_exports` docstring. §H lists the publish-flip + tsup + export-map flip as a single
  linked task.
- **S4 (`recommendedTypeChecked` noise + perf).** **Resolved: §B.5 + §H** acknowledge the noise
  cost (downgrade noisy rules to `warn` when real code lands) and the perf cost (revisit at ~20+
  packages).
- **S5 (topology asymmetry rationale).** **Resolved: §A.0** explicitly documents why rs is
  leaves-only while py and ts use the parent+leaves pattern.
- **S6 (peerDependencies catalog support).** **Resolved: §C.1 note** — pnpm 11.3.0 (this repo's
  pinned version) supports catalog refs in peer deps; verified during implementation.

**Smaller (N1–N6)**
- **N1 (`tsc --build` scaling note).** §H bullet added.
- **N2 (`next-env.d.ts` SPDX exemption rationale).** §E.1 strengthened with the "clobbered on
  rebuild → useless and noisy" explanation, plus the SMA-383 follow-up to refine the carve-out.
- **N3 (cross-stack "empty test suite" convention).** §E.2 + §H follow-up to formalize in
  CONTRIBUTING.md.
- **N4 (`no outputs on build` comment).** §A.3 inline YAML comment added.
- **N5 (`@paigasus/proto` future-dep growth).** §C.4 + §H bullet added.
- **N6 (`paigasus-docs` layer).** **Resolved: layer flipped to `library`** (§D.2) until the
  framework is chosen.

## K. ADR-0009 amendment plan

ADR-0009 currently names the legacy plugin set:

> "`eslint-plugin-react` + `eslint-plugin-react-hooks` — React packages and apps"

The ts workspace uses `@eslint-react/eslint-plugin` (the modern flat-config-native fork) instead
of the legacy `eslint-plugin-react`. This is a substantive deviation from a documented ADR and
follows the precedent set by SMA-357 (Rust edition 2024 deviation: AC amended + spec rationale
recorded). The amendment:

1. **Before this PR opens**, update ADR-0009 in Notion to name the modern plugin set:
   - `@eslint-react/eslint-plugin` (replaces `eslint-plugin-react`)
   - `eslint-plugin-react-hooks` (v7+, with React Compiler rule folded in from its v6 line)
   - `eslint-plugin-jsx-a11y`
   - `typescript-eslint` (v8+, with the wider peer range that supports ESLint 10)
2. Add a one-paragraph rationale to ADR-0009 noting:
   - The legacy `eslint-plugin-react` had a famously slow flat-config migration; the
     `@eslint-react/eslint-plugin` fork is flat-config-native and TS-aware.
   - The ADR was originally written when ESLint 9 was current; the move to ESLint 10 + the modern
     plugin set is the natural progression.
3. Reference the amended ADR from §B.5 of this spec once it's live.

This is the same shape as SMA-384's Notion redirect for the Polyglot Monorepo Scoping § 1 — done
out-of-repo before the PR opens, tracked in the spec so the drift can't recur.
