# SMA-359 — Bootstrap `ts/` pnpm workspace with ESLint + Prettier

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Scaffold the `ts/` pnpm workspace with a parent Moon project, four library stubs, two app stubs (Next.js 16 console + framework-TBD docs), ESLint 10 + `@eslint-react/eslint-plugin` flat config, Prettier, Vitest, pnpm Catalog; wire `.moon/tasks/typescript.yml` (deferred by SMA-384) and slim the TS scaffold template; bump cross-stack line length to 200.

**Architecture:** Mirror the py post-SMA-358/SMA-384 topology — a `ts` parent Moon project at `ts/moon.yml` owns workspace-wide gates, per-package/app Moon projects are alias leaves. ESLint flat config; tsc via recursive `pnpm -r run typecheck` (no solution file because Next apps can't be `composite`). All dep versions centralized in `pnpm-workspace.yaml`'s `catalog:` block.

**Tech Stack:** Moon 2.2.5 · Node 22.22.3 · pnpm 11.3.0 · TypeScript 5.7 · ESLint 10.4 · `@eslint-react/eslint-plugin` 5.8 · `typescript-eslint` 8.60 · `eslint-plugin-react-hooks` 7.1 · `eslint-plugin-jsx-a11y` 6.10 · Prettier 3.4 · Vitest 3.0 · Next.js 16.

**Spec:** `docs/superpowers/specs/2026-05-28-sma-359-ts-pnpm-workspace-design.md`

---

## Pre-flight checks

- [ ] **Step P1: Confirm branch and clean tree**

Run from `/Users/smaschek/dev/paigasus/paigasus-core`:

```bash
git rev-parse --abbrev-ref HEAD
git status --short
```

Expected: branch `feature/sma-359-bootstrap-ts-pnpm-workspace-with-eslint-prettier-config`, working tree clean (only untracked files allowed).

- [ ] **Step P2: Confirm Moon is on PATH and pinned version**

```bash
moon --version
```

Expected: `2.2.5` (or whatever `.prototools` pins; run `proto install` first if missing).

- [ ] **Step P3: Re-verify catalog versions against npm (~1 min sanity check)**

```bash
for pkg in eslint typescript-eslint '@eslint-react/eslint-plugin' eslint-plugin-react-hooks eslint-plugin-jsx-a11y prettier vitest typescript next '@next/eslint-plugin-next'; do
  echo -n "$pkg: "
  curl -fsS "https://registry.npmjs.org/$pkg/latest" 2>/dev/null | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('version','?'))" 2>/dev/null || echo "fetch failed"
done
```

If any version's current latest is meaningfully ahead of the spec's `^X.Y.Z` floor, bump the catalog entry in Phase 4 / Task 4. Note any deltas before continuing.

- [ ] **Step P4: Notion: ADR-0009 amendment (out-of-repo, do BEFORE the PR opens)**

Per spec §K — amend ADR-0009 in Notion to name the modern plugin set (`@eslint-react/eslint-plugin`, `eslint-plugin-react-hooks` v7+, `typescript-eslint` v8+ with ESLint 10 peer-range support) and add the one-paragraph rationale. Tick this off when the Notion page is updated. **Do not block on this before starting Phase 1** — it can land in parallel with the in-repo work, as long as it's complete before the PR opens.

---

## Phase 1 — Cross-stack line-length bump (commit 1)

### Task 1: Bump py + rs line length to 200

**Files:**
- Modify: `py/pyproject.toml:18` (`line-length` and `target-version` block)
- Create: `rs/rustfmt.toml`

- [ ] **Step 1.1: Update `py/pyproject.toml`**

Open `py/pyproject.toml`. Change the `[tool.ruff]` block from `line-length = 100` to `line-length = 200`:

```toml
[tool.ruff]
line-length = 200
target-version = "py312"
```

(Leave the rest of the file unchanged.)

- [ ] **Step 1.2: Create `rs/rustfmt.toml`**

Create the file with:

```toml
# rustfmt configuration for the rs/ workspace. Default max_width is 100;
# bumped to 200 in SMA-359 for cross-stack consistency with py (ruff
# line-length = 200) and ts (prettier printWidth: 200).
max_width = 200
```

(No SPDX header — TOML is a config file, per CONTRIBUTING.md.)

- [ ] **Step 1.3: Verify py gates still pass**

```bash
moon run py:lint
moon run py:fmt
moon run py:typecheck
moon run py:test
```

Expected: all four exit 0. The empty py workspace has no source lines exceeding 100, so no reformat churn shows up at this step.

- [ ] **Step 1.4: Verify rs gates still pass**

```bash
moon ci :build :test :lint :fmt --query 'language=rust'
```

Or if that's awkward, run each rs crate's gates explicitly:

```bash
cd rs && cargo fmt --check && cargo build --workspace && cargo clippy --workspace -- -D warnings
```

Expected: exit 0 on all. The rs workspace's existing code (if any) was written at default 100, but rustfmt at `max_width = 200` is permissive (longer lines are allowed, not required), so existing 100-width code remains valid.

- [ ] **Step 1.5: Commit**

```bash
git add py/pyproject.toml rs/rustfmt.toml
git commit -m "$(cat <<'EOF'
chore(repo): widen line length to 200 across rs/py/ts (SMA-359)

Bump py/pyproject.toml [tool.ruff] line-length 100 -> 200 and create
rs/rustfmt.toml with max_width = 200. The ts/ side of the bump
(ts/.prettierrc.js) lands with the ts bootstrap commit in Phase 4. This
commit lands first so subsequent ts files are formatted at 200 from the
start, avoiding later reformat churn. Polyglot stays at a single number;
rationale documented in the SMA-359 design spec.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — Moon language tasks file (commit 2)

### Task 2: Add `.moon/tasks/typescript.yml` + register implicit input

**Files:**
- Create: `.moon/tasks/typescript.yml`
- Modify: `.moon/tasks.yml` (add typescript.yml to `implicitInputs`)

- [ ] **Step 2.1: Create `.moon/tasks/typescript.yml`**

Write exactly:

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
    # No `outputs:` — tsc --noEmit produces no files; Moon's per-project output check
    # would fail (same shape as SMA-384 Correction 1 for `uv build`). Cache invalidation
    # runs off `inputs:`. Apps that produce artifacts (Next.js) override this task.
    command: 'pnpm exec tsc -p tsconfig.json --noEmit'
    inputs:
      - '@group(sources)'
      - 'tsconfig.json'
      - 'package.json'
      - '/ts/tsconfig.base.json'
      - '/ts/pnpm-lock.yaml'
  lint:
    command: 'pnpm exec eslint .'
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - 'eslint.config.js'
      - 'package.json'
      - '/ts/eslint.config.js'
      - '/ts/pnpm-lock.yaml'
  fmt:
    command: 'pnpm exec prettier --check .'
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - '.prettierrc.js'
      - '.prettierignore'
      - 'package.json'
      - '/ts/.prettierrc.js'
      - '/ts/.prettierignore'
      - '/ts/pnpm-lock.yaml'
  typecheck:
    command: 'pnpm exec tsc -p tsconfig.json --noEmit'
    inputs:
      - '@group(sources)'
      - 'tsconfig.json'
      - 'package.json'
      - '/ts/tsconfig.base.json'
      - '/ts/pnpm-lock.yaml'
  test:
    command: 'pnpm exec vitest run --passWithNoTests'
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - 'package.json'
      - 'vitest.config.ts'
      - '/ts/pnpm-lock.yaml'
```

- [ ] **Step 2.2: Update `.moon/tasks.yml`**

Edit `.moon/tasks.yml`. The current `implicitInputs` block:

```yaml
implicitInputs:
  - '/.moon/toolchain.yml'
  - '/.moon/tasks.yml'
  - '/.moon/tasks/rust.yml'
  - '/.moon/tasks/python.yml'
```

Add `'/.moon/tasks/typescript.yml'` as the new last entry:

```yaml
implicitInputs:
  - '/.moon/toolchain.yml'
  - '/.moon/tasks.yml'
  - '/.moon/tasks/rust.yml'
  - '/.moon/tasks/python.yml'
  - '/.moon/tasks/typescript.yml'
```

- [ ] **Step 2.3: Verify Moon still validates the workspace**

Run from the repo root:

```bash
moon sync projects
moon project py
moon project paigasus-kernel-rs
```

Expected: both `moon project` invocations succeed and report inherited tasks. No errors about the new typescript.yml (it has no consumers yet — there are no `language: typescript` projects until Phase 4).

- [ ] **Step 2.4: Commit**

```bash
git add .moon/tasks/typescript.yml .moon/tasks.yml
git commit -m "$(cat <<'EOF'
feat(ts): add .moon/tasks/typescript.yml + register implicit input (SMA-359)

Mirrors .moon/tasks/python.yml's shape: inheritedBy.languages: ['typescript'],
build/lint/fmt/typecheck/test tasks invoking pnpm exec <tool>, workspace-anchor
input paths for /ts/* root configs alongside local-relative paths so both
per-package overlays and root-config edits bust caches. Task name `fmt` (not
`format`) harmonizes with rust/py for cross-stack `moon ci :fmt`. No consumers
yet — ts projects land in the bootstrap commit (Phase 4). Closes the
.moon/tasks/typescript.yml deferral from SMA-384.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — Slim the TS scaffold template (commit 3)

### Task 3: Slim `.moon/templates/typescript/moon.yml` and update template.yml

**Files:**
- Modify: `.moon/templates/typescript/moon.yml`
- Modify: `.moon/templates/typescript/template.yml`

- [ ] **Step 3.1: Replace `.moon/templates/typescript/moon.yml`**

Replace the entire file contents with:

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
{%- endif %}
```

After this, `library`-archetype renders produce a 4-line `moon.yml` (header only); `app` archetype adds the `build` override (which differs from the inherited `tsc --noEmit`).

- [ ] **Step 3.2: Update `.moon/templates/typescript/template.yml`**

Edit the description block to drop the "finalize in SMA-359" hedge. Replace:

```yaml
description: |
  Scaffolds a moon.yml for a TypeScript project. `library` for a publishable
  package under ts/packages, `app` for a deployable under ts/apps (e.g. Next.js).
  Lint/format use ESLint + Prettier per ADR-0009; the test runner is a scaffold
  default (vitest) — finalize in SMA-359.
```

with:

```yaml
description: |
  Scaffolds a moon.yml for a TypeScript project. `library` for a publishable
  package under ts/packages, `app` for a deployable under ts/apps (e.g. Next.js).
  Library archetype renders a header-only moon.yml; app archetype adds a `build`
  task overriding the inherited `tsc --noEmit` with `next build`. Lint/format/test
  /typecheck come from .moon/tasks/typescript.yml (ESLint + Prettier per ADR-0009;
  Vitest).
```

- [ ] **Step 3.3: Smoke test the slimmed template (library archetype)**

```bash
moon generate typescript --to ts/_throwaway-lib --defaults --force -- --name throwaway --archetype library
cat ts/_throwaway-lib/moon.yml
moon project throwaway-ts
```

Expected: rendered `moon.yml` is exactly four lines (`$schema`, `id: 'throwaway-ts'`, `layer: 'library'`, `language: 'typescript'`). `moon project throwaway-ts` resolves with the five inherited tasks (`build`, `lint`, `fmt`, `typecheck`, `test`) and no `unknown_file_group` error.

Clean up:

```bash
rm -rf ts/_throwaway-lib
moon sync projects
```

- [ ] **Step 3.4: Smoke test the slimmed template (app archetype)**

```bash
moon generate typescript --to ts/_throwaway-app --defaults --force -- --name throwaway-app --archetype app
cat ts/_throwaway-app/moon.yml
moon project throwaway-app-ts
```

Expected: rendered `moon.yml` includes the `tasks: build:` block with `next build`. `moon project throwaway-app-ts` shows `build` overridden + the other four inherited.

Clean up:

```bash
rm -rf ts/_throwaway-app
moon sync projects
```

- [ ] **Step 3.5: Commit**

```bash
git add .moon/templates/typescript/moon.yml .moon/templates/typescript/template.yml
git commit -m "$(cat <<'EOF'
chore(ts): slim typescript scaffold template (app override only) (SMA-359)

Library archetype renders a 4-line moon.yml (header only); app archetype adds
only the `build` task override (next build + .next outputs). All other tasks
(lint/fmt/typecheck/test) come from the inherited .moon/tasks/typescript.yml
landed in the prior commit. Matches the python template's post-SMA-384 shape.
The template's description is updated to drop the "finalize in SMA-359"
hedge.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4 — Bootstrap `ts/` workspace (commit 4)

This is the big commit. Many files, all interrelated; verification at the end of the phase ensures the whole thing lands consistently. Within Phase 4, the order of tasks matters: root configs → parent project → packages → apps → install/verify → commit.

### Task 4: Workspace-root files under `ts/`

**Files:**
- Create: `ts/pnpm-workspace.yaml`
- Create: `ts/package.json`
- Create: `ts/tsconfig.base.json`
- Create: `ts/eslint.config.js`
- Create: `ts/.prettierrc.js`
- Create: `ts/.prettierignore`

- [ ] **Step 4.1: Create `ts/pnpm-workspace.yaml`**

```yaml
packages:
  - 'packages/*'
  - 'apps/*'

# Catalog — single bump-point for shared dep versions across the workspace.
# Per-package package.json references these as "<dep>": "catalog:".
# pnpm-lock.yaml pins exact resolved versions; the carets here cap the upgrade window.
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

If Step P3 surfaced a meaningfully newer version for any entry, bump the floor here before continuing.

- [ ] **Step 4.2: Create `ts/package.json`**

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

- [ ] **Step 4.3: Create `ts/tsconfig.base.json`**

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

- [ ] **Step 4.4: Create `ts/eslint.config.js`**

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

- [ ] **Step 4.5: Create `ts/.prettierrc.js`**

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

- [ ] **Step 4.6: Create `ts/.prettierignore`**

```
dist
.next
node_modules
pnpm-lock.yaml
coverage
```

### Task 5: Parent `ts/moon.yml` and `.moon/workspace.yml` registration

**Files:**
- Create: `ts/moon.yml`
- Modify: `.moon/workspace.yml` (add `'ts'` to `projects`)

- [ ] **Step 5.1: Create `ts/moon.yml`**

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
# lint/fmt/test commands work from `ts/` cwd unchanged — they walk from cwd to find their
# config and pick up the whole tree.
tasks:
  typecheck:
    command: 'pnpm -r --if-present run typecheck'
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - 'tsconfig.base.json'
      - 'package.json'
      - 'pnpm-workspace.yaml'
      - 'pnpm-lock.yaml'
  build:
    command: 'pnpm -r --if-present run build'
    inputs:
      - '@group(sources)'
      - 'tsconfig.base.json'
      - 'package.json'
      - 'pnpm-workspace.yaml'
      - 'pnpm-lock.yaml'
```

- [ ] **Step 5.2: Update `.moon/workspace.yml`**

Edit `.moon/workspace.yml`. Current `projects` block:

```yaml
projects:
  - 'contracts'
  - 'rs/crates/libs/*'
  - 'rs/crates/bindings/*'
  - 'rs/crates/services/*'
  - 'py'
  - 'py/packages/*'
  - 'ts/packages/*'
  - 'ts/apps/*'
```

Add `'ts'` between `'py/packages/*'` and `'ts/packages/*'`:

```yaml
projects:
  - 'contracts'
  - 'rs/crates/libs/*'
  - 'rs/crates/bindings/*'
  - 'rs/crates/services/*'
  - 'py'
  - 'py/packages/*'
  - 'ts'
  - 'ts/packages/*'
  - 'ts/apps/*'
```

- [ ] **Step 5.3: Verify Moon resolves the parent project**

```bash
moon sync projects
moon project ts
```

Expected: project resolves with `Layer: configuration`, `Language: typescript`, inherited tasks (5 total: build/lint/fmt/typecheck/test, with build+typecheck overridden by the local definitions). No errors.

### Task 6: Library packages — `paigasus-proto`, `paigasus-kernel`, `paigasus-sdk`, `paigasus-ui`

Each library lives under `ts/packages/paigasus-<short>/` (the `paigasus-` prefix matches the py + rs conventions for cross-stack Moon ID symmetry per spec §11). npm name stays `@paigasus/<short>` — pnpm doesn't require dir-name alignment.

**Files (per library, identical skeleton):**
- Create: `ts/packages/paigasus-<short>/moon.yml`
- Create: `ts/packages/paigasus-<short>/package.json`
- Create: `ts/packages/paigasus-<short>/tsconfig.json`
- Create: `ts/packages/paigasus-<short>/src/index.ts`

- [ ] **Step 6.1: Create `paigasus-proto`**

```bash
mkdir -p ts/packages/paigasus-proto/src
```

`ts/packages/paigasus-proto/moon.yml`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-proto-ts'
layer: 'library'
language: 'typescript'
```

`ts/packages/paigasus-proto/package.json`:

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

`ts/packages/paigasus-proto/tsconfig.json`:

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

`ts/packages/paigasus-proto/src/index.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0

export {};
```

- [ ] **Step 6.2: Create `paigasus-kernel`**

```bash
mkdir -p ts/packages/paigasus-kernel/src
```

`ts/packages/paigasus-kernel/moon.yml`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-kernel-ts'
layer: 'library'
language: 'typescript'
```

`ts/packages/paigasus-kernel/package.json`:

```json
{
  "name": "@paigasus/kernel",
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

`ts/packages/paigasus-kernel/tsconfig.json`:

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

`ts/packages/paigasus-kernel/src/index.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0

export {};
```

- [ ] **Step 6.3: Create `paigasus-sdk`**

```bash
mkdir -p ts/packages/paigasus-sdk/src
```

`ts/packages/paigasus-sdk/moon.yml`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-sdk-ts'
layer: 'library'
language: 'typescript'
```

`ts/packages/paigasus-sdk/package.json`:

```json
{
  "name": "@paigasus/sdk",
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

`ts/packages/paigasus-sdk/tsconfig.json`:

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

`ts/packages/paigasus-sdk/src/index.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0

export {};
```

- [ ] **Step 6.4: Create `paigasus-ui` (with React peers)**

```bash
mkdir -p ts/packages/paigasus-ui/src
```

`ts/packages/paigasus-ui/moon.yml`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-ui-ts'
layer: 'library'
language: 'typescript'
```

`ts/packages/paigasus-ui/package.json` — note the `peerDependencies` block (not present on the other three):

```json
{
  "name": "@paigasus/ui",
  "_comment_exports": "Source-only exports. Bundler-aware consumers only (Next/Vitest/tsc walk through TS via moduleResolution: bundler). Switch to ./dist/index.js when tsup wiring lands — must happen IN LOCKSTEP with flipping `private: false` for publishable packages.",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "license": "Apache-2.0",
  "exports": { ".": "./src/index.ts" },
  "scripts": { "typecheck": "tsc -p tsconfig.json --noEmit" },
  "peerDependencies": {
    "react": "catalog:",
    "react-dom": "catalog:"
  },
  "devDependencies": { "typescript": "catalog:" }
}
```

`ts/packages/paigasus-ui/tsconfig.json`:

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

`ts/packages/paigasus-ui/src/index.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0

export {};
```

### Task 7: The Next.js 16 `paigasus-console` app

**Files:**
- Create (via CLI then trim): `ts/apps/paigasus-console/` tree
- Final files after trim: `moon.yml`, `package.json`, `tsconfig.json`, `next.config.ts`, `next-env.d.ts`, `eslint.config.js`, `app/layout.tsx`, `app/page.tsx`

- [ ] **Step 7.1: Run `create-next-app`**

```bash
pnpm dlx create-next-app@16 ts/apps/paigasus-console \
  --ts --app --tailwind=false --src-dir=false --import-alias='@/*' \
  --use-pnpm --eslint=false --turbopack
```

If the CLI prompts despite the flags (some versions ignore one or two), accept the defaults that match the flags above. Do not let it run `pnpm install` at the workspace level — Phase 4's install runs from `ts/`.

- [ ] **Step 7.2: Trim files we don't want**

```bash
cd ts/apps/paigasus-console
rm -f README.md .gitignore
rm -rf public
rm -f app/favicon.ico app/globals.css
# Defensive — in case create-next-app emitted one despite --eslint=false:
rm -f .eslintrc* eslint.config.js eslint.config.mjs
cd ../../..
```

- [ ] **Step 7.3: Overwrite `package.json`**

Replace `ts/apps/paigasus-console/package.json` with:

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

- [ ] **Step 7.4: Overwrite `tsconfig.json`**

Replace `ts/apps/paigasus-console/tsconfig.json` with:

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

- [ ] **Step 7.5: Overwrite `next.config.ts`**

Replace `ts/apps/paigasus-console/next.config.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
import type { NextConfig } from 'next';

const nextConfig: NextConfig = {};

export default nextConfig;
```

- [ ] **Step 7.6: Create `eslint.config.js`**

Create `ts/apps/paigasus-console/eslint.config.js`:

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

- [ ] **Step 7.7: Overwrite `app/layout.tsx`**

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

- [ ] **Step 7.8: Overwrite `app/page.tsx`**

```tsx
// SPDX-License-Identifier: Apache-2.0
export default function Page() {
  return <h1>Paigasus console</h1>;
}
```

- [ ] **Step 7.9: Create `moon.yml`**

`ts/apps/paigasus-console/moon.yml`:

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
```

- [ ] **Step 7.10: Verify the trim landed cleanly**

Run from `ts/apps/paigasus-console`:

```bash
ls -A1
```

Expected output (exact, in some order): `app`, `eslint.config.js`, `moon.yml`, `next-env.d.ts`, `next.config.ts`, `package.json`, `tsconfig.json`. Nothing else. (`.next/` and `node_modules/` show up after `pnpm install` + `next dev`/`next build` later — not at this step.)

### Task 8: The `paigasus-docs` placeholder app

**Files:**
- Create: `ts/apps/paigasus-docs/moon.yml`
- Create: `ts/apps/paigasus-docs/package.json`
- Create: `ts/apps/paigasus-docs/tsconfig.json`
- Create: `ts/apps/paigasus-docs/src/index.ts`

- [ ] **Step 8.1: Create directory + files**

```bash
mkdir -p ts/apps/paigasus-docs/src
```

`ts/apps/paigasus-docs/moon.yml`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-docs-ts'
layer: 'library'
language: 'typescript'
```

(`layer: 'library'` — not `'application'` — until the framework is chosen, per spec §D.2 and N6.)

`ts/apps/paigasus-docs/package.json`:

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

`ts/apps/paigasus-docs/tsconfig.json`:

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

`ts/apps/paigasus-docs/src/index.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0

export {};
```

### Task 9: Replace `ts/README.md`

**Files:**
- Modify (overwrite): `ts/README.md`

- [ ] **Step 9.1: Overwrite `ts/README.md`**

```markdown
# ts/

TypeScript workspace for paigasus-core, managed with [pnpm](https://pnpm.io/) and orchestrated by [Moon](https://moonrepo.dev).

## Layout

- `pnpm-workspace.yaml` — declares the workspace members (`packages/*`, `apps/*`) and the dependency catalog. The `catalog:` block is the single bump-point for shared versions across the workspace; per-package `package.json` references entries as `"<dep>": "catalog:"`.
- `package.json` — workspace root. Private, ESM-only, declares the workspace-wide devDependencies (TypeScript, ESLint, Prettier, plugins, Vitest) referenced via `catalog:`.
- `tsconfig.base.json` — shared `compilerOptions` every package's `tsconfig.json` extends. Strict; `moduleResolution: bundler`; ES2022 target.
- `eslint.config.js` — flat config. Type-checked TS rules across `**/*.{ts,tsx,mts,cts}`; React rules (`@eslint-react/eslint-plugin`, `eslint-plugin-react-hooks`, `eslint-plugin-jsx-a11y`) glob-scoped to `**/*.{tsx,jsx}` only, so non-React libraries don't see them.
- `.prettierrc.js`, `.prettierignore` — formatting config. `printWidth: 200` (cross-stack with py/rs).
- `moon.yml` — workspace parent project (`layer: configuration`). Owns workspace-wide `typecheck` and `build` (recursive `pnpm -r --if-present run <task>`); inherits `lint`/`fmt`/`test` from `.moon/tasks/typescript.yml`.
- `packages/*` — publishable libraries; each is a uv-style first-class Moon project (id `paigasus-<short>-ts`):
  - `paigasus-proto` (`@paigasus/proto`) — generated proto types post-MVP (consumes `contracts/`)
  - `paigasus-kernel` (`@paigasus/kernel`) — thin wrapper over the napi-rs binding to `paigasus-kernel-rs`, post-MVP
  - `paigasus-sdk` (`@paigasus/sdk`) — public SDK placeholder
  - `paigasus-ui` (`@paigasus/ui`) — shared React components for the console
- `apps/*` — deployables (id `paigasus-<name>-ts`):
  - `paigasus-console` (`@paigasus/console`) — Next.js 16 (App Router) operator console
  - `paigasus-docs` (`@paigasus/docs`) — framework TBD; framework choice tracked in a follow-up SMA-NNN issue

## Commands

The workspace-wide gates live on the `ts` Moon project and run once over the whole workspace from `ts/`:

| Task | Command |
| --- | --- |
| Lint | `moon run ts:lint` |
| Format check | `moon run ts:fmt` |
| Type check | `moon run ts:typecheck` |
| Test | `moon run ts:test` |
| Build (libs) | `moon run ts:build` |
| Build (Next app) | `moon run paigasus-console-ts:build` |

Notes:

- For env parity, invoke pnpm via `moon run ts:<task>` so Moon's pinned Node (`.moon/toolchain.yml`) is used, not whatever's on PATH.
- Per-package install: `pnpm --filter @paigasus/<name> add <dep>`. For dev deps: `pnpm --filter @paigasus/<name> add -D <dep>`.
- The `catalog:` block in `pnpm-workspace.yaml` is the single bump-point for shared versions. To bump a tool, edit the catalog entry — every package picks up the new version on the next `pnpm install`.
- All packages currently ship `private: true` with `"exports": { ".": "./src/index.ts" }`. Before any first publish: drop `private`, add `description`/`repository`/`homepage`/`keywords`, switch `exports` to `./dist/index.js`, and wire `tsup` per package (these MUST land together; see the SMA-359 design spec §H).
- The `test` task runs `vitest run --passWithNoTests`. Drop the flag once the first real test lands.

**Status:** workspace bootstrapped in SMA-359; packages are empty stubs.
```

### Task 10: Install dependencies and run all gates

**No file changes — only verification commands.**

- [ ] **Step 10.1: Run `pnpm install` from `ts/`**

```bash
cd ts && pnpm install && cd ..
```

Expected:

- Exit 0.
- `ts/pnpm-lock.yaml` is generated (commit it; do NOT add to `.gitignore`).
- `ts/node_modules/` populated.
- Per-package `node_modules/` symlinks set up by pnpm.
- No `ERR_PNPM_*` errors. If pnpm complains about `peerDependencies: { "react": "catalog:" }` not being resolved, fall back to explicit versions for the ui peers (per spec §S6) and re-run.

If install fails because a catalog version no longer exists on npm, bump the catalog floor (`ts/pnpm-workspace.yaml`) to the latest visible on registry and re-run. Note the bump in the commit message.

- [ ] **Step 10.2: Verify `pnpm lint`**

```bash
cd ts && pnpm lint && cd ..
```

Expected: exit 0. No ESLint findings on the empty stubs.

If lint fails on the `eslint.config.js` itself (e.g., `@eslint-react/eslint-plugin`'s `configs.recommended` shape differs from what's coded), inspect the plugin's actual exported config shape with:

```bash
cd ts && node -e "import('@eslint-react/eslint-plugin').then(m => console.log(Object.keys(m.default.configs || {})))"
```

and adjust the spread accordingly. Same for `react-hooks` / `jsx-a11y` if they don't expose `configs.recommended.rules`.

- [ ] **Step 10.3: Verify `pnpm format`**

```bash
cd ts && pnpm format && cd ..
```

Expected: exit 0. All files conform to Prettier 3.4 at `printWidth: 200`.

If anything fails, run `pnpm format:write` to apply, then re-run `pnpm format` to confirm clean.

- [ ] **Step 10.4: Verify `pnpm typecheck`**

```bash
cd ts && pnpm typecheck && cd ..
```

Expected: exit 0. `pnpm -r --if-present run typecheck` runs each package's typecheck script; all the empty `export {};` modules typecheck fine. If a package complains about extending `../../tsconfig.base.json`, verify the relative path matches the directory depth.

- [ ] **Step 10.5: Verify `pnpm test`**

```bash
cd ts && pnpm test && cd ..
```

Expected: exit 0 with a "No test files found" message (vitest's `--passWithNoTests`).

- [ ] **Step 10.6: Verify Moon parity**

```bash
moon sync projects
moon project ts
moon project paigasus-proto-ts
moon project paigasus-console-ts
moon project paigasus-docs-ts
```

Expected: every project resolves cleanly. `paigasus-console-ts` shows the local `build` overriding the inherited one; the others show all five inherited tasks (`build`/`lint`/`fmt`/`typecheck`/`test`).

Then run the gates through Moon:

```bash
moon run ts:lint
moon run ts:fmt
moon run ts:typecheck
moon run ts:test
moon run ts:build
```

Expected: all five exit 0.

- [ ] **Step 10.7: Verify cross-stack `moon ci`**

```bash
moon ci :lint :fmt :typecheck :test :build
```

Expected: exit 0 across all four workspaces (contracts/rs/py/ts). With this commit, `moon ci :fmt` covers all three of rs + py + ts uniformly (py was already renamed to `fmt` in SMA-384; SMA-359's `.moon/tasks/typescript.yml` matches that name).

### Task 11: Commit Phase 4

- [ ] **Step 11.1: Stage and commit**

```bash
git add ts/ .moon/workspace.yml
git status
```

Verify only the expected files are staged (everything under `ts/` including `pnpm-lock.yaml`, plus the one-line `.moon/workspace.yml` edit). Then:

```bash
git commit -m "$(cat <<'EOF'
chore(ts): bootstrap ts/ pnpm workspace with ESLint + Prettier (SMA-359)

Scaffolds the TypeScript workspace under ts/:

- Workspace root: pnpm-workspace.yaml (with catalog), package.json (workspace
  scripts + devDeps as catalog refs), tsconfig.base.json (strict + bundler +
  ES2022), eslint.config.js (flat, ESLint 10 + @eslint-react + typescript-eslint
  v8 + react-hooks v7 + jsx-a11y; React rules glob-scoped to JSX/TSX only),
  .prettierrc.js (printWidth: 200), .prettierignore.
- Parent project: ts/moon.yml (layer: configuration; typecheck + build overrides
  via pnpm -r --if-present run). Registered in .moon/workspace.yml's projects
  list alongside the existing ts/{packages,apps}/* globs.
- Four library stubs under ts/packages/paigasus-{proto,kernel,sdk,ui}/ (Moon ids
  paigasus-<short>-ts). Source-only exports (./src/index.ts), private: true,
  ESM, catalog-driven devDeps. @paigasus/ui carries react/react-dom as peers.
- paigasus-console: Next.js 16 (App Router) via create-next-app + trim; local
  eslint.config.js layering @next/eslint-plugin-next on top of the root config;
  moon.yml overrides build to `next build`.
- paigasus-docs: framework-TBD placeholder, layer: library until framework lands.
- README rewritten from "Empty until pnpm workspace lands" to describe the real
  layout, commands, operator notes.

Verifies: pnpm install / lint / format / typecheck / test all exit 0 on the
empty workspace; moon run ts:<task> and moon ci :<task> exit 0 across stacks.
ADR-0009 deviation (modern React-tooling stack) tracked in the spec §K
amendment (Notion-side, done before PR opens).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 11.2: Confirm commit log + branch state**

```bash
git log --oneline -6
```

Expected (newest first):

```
<sha> chore(ts): bootstrap ts/ pnpm workspace with ESLint + Prettier (SMA-359)
<sha> chore(ts): slim typescript scaffold template (app override only) (SMA-359)
<sha> feat(ts): add .moon/tasks/typescript.yml + register implicit input (SMA-359)
<sha> chore(repo): widen line length to 200 across rs/py/ts (SMA-359)
9e9eb47 docs(ts): incorporate design review for SMA-359 ts workspace bootstrap
4882eb5 docs(ts): design ts/ pnpm workspace bootstrap with ESLint + Prettier (SMA-359)
```

Six commits ahead of `main`.

---

## Post-implementation

- [ ] **Step F1: Final cross-stack sanity sweep**

```bash
moon ci :build :test :lint :fmt :typecheck
```

Expected: exit 0. The full polyglot graph builds, tests, lints, formats, and typechecks across rs + py + ts (contracts is a generator project with its own task shape and may not match every target — that's expected, not a failure).

- [ ] **Step F2: Confirm ADR-0009 Notion amendment is live**

Re-tick Step P4 once Notion shows the amended ADR-0009. If the PR opens before the Notion edit, the PR description needs to call this out as a follow-up blocker on the spec's §K.

- [ ] **Step F3: Open the PR**

Use `gh pr create` (or the Linear branch link from `SMA-359`'s issue page — the integration auto-attaches when the branch matches `feature/sma-359-...`). PR template's acceptance-criteria checklist maps 1:1 to the spec's §F verification table.

PR body should reference:

- The spec at `docs/superpowers/specs/2026-05-28-sma-359-ts-pnpm-workspace-design.md`
- Spec §J (design review disposition) for what changed since the original review
- Spec §K (ADR-0009 amendment plan) and confirm it's live in Notion before merge

---

## Notes on common failure modes

If you hit any of these during execution, here are the known fixes (mostly already covered in the spec's review disposition):

- **`pnpm install` errors on `"react": "catalog:"` in peerDependencies of `@paigasus/ui`.** Fall back to explicit version strings for the peer deps; document the carve-out in a TODO comment in `paigasus-ui/package.json`. Per spec §S6.
- **`@eslint-react/eslint-plugin`'s exported config shape differs from `.configs.recommended`.** Inspect with `node -e "import('@eslint-react/eslint-plugin').then(m => console.log(Object.keys(m.default || m)))"` and adjust the spread in `ts/eslint.config.js`. The plugin's v5 line uses flat-config-native exports; the exact key may be `configs.recommended`, `configs['recommended-typescript']`, or similar.
- **Moon resolves but `moon run ts:typecheck` exits non-zero with "no projects to run."** Usually means `pnpm-workspace.yaml` didn't pick up the package, so no package has a `typecheck` script for `pnpm -r --if-present` to find. Verify `pnpm -r ls` from `ts/` lists all six projects (4 libs + 2 apps).
- **`next build` fails on first invocation with "ESLint not configured."** Next 16 may try to run its own ESLint during build despite `--eslint=false` at scaffold time. Add `eslint: { ignoreDuringBuilds: true }` to `next.config.ts` (we use our root flat config; Next's bundled rule discovery isn't needed). If this hits, update `next.config.ts` and re-verify Step 10.6.
- **`moon ci :fmt` reports a project without a `fmt` task** — typically the `contracts` project, which has a `generate` task (buf codegen) but no formatter. That's expected (contracts is the proto generator workspace, not a stack with sources to format) and Moon's behavior of skipping projects-without-the-task is correct. Not a failure.
