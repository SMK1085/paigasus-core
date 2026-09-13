# SMA-511 `iam-console` — the IAM zone: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `ts/apps/iam-console` the first real console zone: login under `/iam`, the shared shell, tenancy screens over `@paigasus/sdk`, a real HTTP 403 view, and capability-gated audit — on shared packages that now work inside a Next app with a `basePath`.

**Architecture:** Server components first. Each page builds request-scoped SDK clients in `lib/` and passes them as ports to a plain loader function; mutations are Server Actions over plain command functions. The browser receives rendered HTML and a sanitized `SessionView` only. Affordances use `IsAuthorized` self-queries (`mayI()`), and IAM decides every call.

**Tech Stack:** Next 16.3.4 (App Router, Turbopack, `output: 'standalone'`, `proxy.ts`), React 19.2, TypeScript 6, Connect-ES v2 over gRPC (`@connectrpc/connect-node`), openid-client v6, node-redis 6, zod 4, vitest 5, MSW 2.15.0, Playwright 1.63, Moon 2.5.3, pnpm 11.

**Spec:** `docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md` (revision 2, with the § 13 measurements). Read the spec before each task; this plan argues from it.

## Global Constraints

- Every source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0` (`#` for shell, `/* … */` for CSS).
- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console`, on branch `feature/sma-511-iam-console`. A subagent's first action is `EnterWorktree {"path": "<that path>"}`, then `git branch --show-current` must print `feature/sma-511-iam-console`; stop if it does not.
- Before any tool command: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Moon 2.x needs explicit targets. Run the affected-graph suite as `/bin/bash ci/affected-graph/run.sh` (the system bash 3.2; bash 5.3.15 deadlocks on this machine). Do not use a perl alarm wrapper. macOS has no `timeout`: start a server with `&` and kill it in the same shell command.
- Conventional commits, workspace scope (`ts`, `contracts`, `ci`, `docs`). The subject starts with a lowercase word. No body line starts with `#NNN` or `token: value`. Every message ends with `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`. Never use `--no-verify`.
- Relative imports in any file that Next compiles (`ts/packages/*/src/**`, `ts/apps/iam-console/{app,lib}/**`, `proxy.ts`) are extensionless. From Task 3, the rule `paigasus/no-js-relative-specifier` enforces this for packages.
- No `NEXT_PUBLIC_` anywhere in `ts/`. No `env:` in the Next config (the `createNextConfig` factory owns it).
- `redirect()` in a page or layout takes a basePath-relative path (`'/orgs'`, never `'/iam/orgs'`); a raw `Response` `Location` from a route handler is passed through unchanged, so it must carry the basePath.
- The app never imports `@paigasus/proto` outside `tests/support/**`. `proxy.ts` has value imports only from `next/server` (`NextResponse`), `@paigasus/auth/middleware` and `lib/correlation-header.ts`; a type import from `next/server` is also allowed. `lib/correlation-header.ts` imports only `server-only`. Task 7's lint rule is a deny list and cannot enforce "only", so the reviewer checks these imports.
- Do not add a `loading.tsx` under `app/(console)/`: it starts streaming before `forbidden()` runs, and the HTTP 403 status is then lost.
- `msw` is pinned at `2.15.0` (released 2026-07-08; pnpm 11 `minimumReleaseAge` is 24 hours), and `allowBuilds` has `msw: false`.
- Every task that creates or edits files under `ts/` runs `pnpm -C ts exec prettier --write <those files>` (paths relative to `ts/`) before its fmt check and before its commit.
- Functional style in Next and React code; no classes except `Error` subclasses.
- A per-project Moon task does not run the repo gates. Task 23 runs the full CI target list from `CLAUDE.md` before the push.
- Prose in code comments, READMEs and commit messages follows ASD-STE100 Simplified Technical English.

---

### Task 1: Rename `ts/apps/paigasus-console` to `ts/apps/iam-console` and delete `ts/apps/paigasus-docs`

Spec § 3.1, § 3.2. This task changes names only. It adds no affected-graph case.

Run every command from the worktree root. Set the tool path first in each shell:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
```

**Files:**
- Move: `ts/apps/paigasus-console/` → `ts/apps/iam-console/` (all 16 tracked files, with `git mv`)
- Delete: `ts/apps/paigasus-docs/` (`moon.yml`, `package.json`, `tsconfig.json`, `src/index.ts`)
- Modify (string replacement, lines verified on the branch base `9707d932`):
  - `.moon/tasks/typescript-project.yml:7`
  - `.moon/workspace.yml:59-60`
  - `CLAUDE.md:887,891,894,899,904,905`
  - `CONTRIBUTING.md:212`
  - `ci/affected-graph/run.sh:370,381,396`
  - `ci/next-env/run.sh:11,26,58`
  - `ci/tailwind-source/README.md:3,16,26,31,37,44`
  - `ci/tailwind-source/run.mjs:6,17,124`
  - `moon.yml:138,143,147,150-153,923`
  - `ts/README.md:18-20,33`
  - `ts/apps/iam-console/app/globals.css:21`
  - `ts/apps/iam-console/moon.yml:3,17,18,55,56,107`
  - `ts/apps/iam-console/package.json:2`
  - `ts/apps/iam-console/tests/standalone-runtime.test.ts:13`
  - `ts/eslint.config.js:53,54,58,60`
  - `ts/packages/paigasus-app-shell/moon.yml:21`
  - `ts/packages/paigasus-auth/tests/config.test.ts:10,19`
  - `ts/packages/paigasus-discovery/moon.yml:16`
  - `ts/packages/paigasus-next-config/tests/boundaries.test.ts:75,76,109,110,150,151,152,165,281`
  - `ts/packages/paigasus-sdk/moon.yml:17`
  - `ts/packages/paigasus-ui/README.md:33,43`
  - `ts/packages/paigasus-ui/moon.yml:12`
  - `ts/packages/paigasus-ui/src/components/table.tsx:15`
  - `ts/packages/paigasus-ui/tsconfig.json:6`
  - `ts/packages/paigasus-ui/vitest.config.ts:8`
  - `ts/pnpm-lock.yaml:210,250` (regenerated by `pnpm install`, never edited by hand)
- Test: the existing suites of `iam-console-ts`, `paigasus-next-config-ts`, `paigasus-auth-ts`, the gates `repo:next-env-drift`, `repo:input-liveness`, and `ci/affected-graph/run.sh`.

**Interfaces:**
- Consumes: nothing.
- Produces: the app directory `ts/apps/iam-console`, the package name `@paigasus/iam-console`, the Moon id `iam-console-ts`. Every later task uses these three names.

- [ ] **Step 1: Record the baseline hits**

Run:

```bash
git grep -n paigasus-console -- ':!docs/superpowers'
git grep -n -e paigasus-docs -e '@paigasus/docs' -e '@paigasus/console' -- ':!docs/superpowers'
```

Expected: the hits below and no others. If a new hit appears, treat it as a site to change with the same rule (`paigasus-console` → `iam-console`, `@paigasus/console` → `@paigasus/iam-console`, and remove any `paigasus-docs` mention).

| Site | Old text (excerpt) | New text |
|---|---|---|
| `.moon/tasks/typescript-project.yml:7` | `(see paigasus-console)` | `(see iam-console)` |
| `.moon/workspace.yml:59` | `outputs:` of paigasus-console-ts:build | `…of iam-console-ts:build` |
| `.moon/workspace.yml:60` | `(ts/apps/paigasus-console/moon.yml)` | `(ts/apps/iam-console/moon.yml)` |
| `CLAUDE.md:887` | `` `ts/apps/paigasus-console/app/globals.css:23` `` | `` `ts/apps/iam-console/app/globals.css:23` `` |
| `CLAUDE.md:891,899` | `` `paigasus-console-ts:build` `` | `` `iam-console-ts:build` `` |
| `CLAUDE.md:894` | `` `ts/apps/paigasus-console/` `` | `` `ts/apps/iam-console/` `` |
| `CLAUDE.md:904` | `` `paigasus-console-ts:build` `` | `` `iam-console-ts:build` `` |
| `CLAUDE.md:905` | `` `paigasus-console-ts:test` `` | `` `iam-console-ts:test` `` |
| `CONTRIBUTING.md:212` | `` `paigasus-console` does `` | `` `iam-console` does `` |
| `ci/affected-graph/run.sh:370` | comment `paigasus-console-ts:test` | `iam-console-ts:test` |
| `ci/affected-graph/run.sh:381` (case `ui->console`) | `…,paigasus-console-ts:build,paigasus-console-ts:test,…` | `…,iam-console-ts:build,iam-console-ts:test,…` |
| `ci/affected-graph/run.sh:396` (case `ui-components->console`) | same | same |
| `ci/next-env/run.sh:11,58` | comments `paigasus-console-ts:build` | `iam-console-ts:build` |
| `ci/next-env/run.sh:26` | `APP='ts/apps/paigasus-console'` | `APP='ts/apps/iam-console'` |
| `ci/tailwind-source/README.md:3` | `` `@paigasus/console` `` | `` `@paigasus/iam-console` `` |
| `ci/tailwind-source/README.md:16,31` | `ts/apps/paigasus-console/` | `ts/apps/iam-console/` |
| `ci/tailwind-source/README.md:26,37,44` | `paigasus-console-ts:…` | `iam-console-ts:…` |
| `ci/tailwind-source/run.mjs:6,124` | comment / message `ts/apps/paigasus-console` | `ts/apps/iam-console` |
| `ci/tailwind-source/run.mjs:17` | `join(REPO_ROOT, 'ts', 'apps', 'paigasus-console')` | `join(REPO_ROOT, 'ts', 'apps', 'iam-console')` |
| `moon.yml:138,147,923` | comments | `iam-console` / `iam-console-ts:build` |
| `moon.yml:143` | `- 'paigasus-console-ts:build'` | `- 'iam-console-ts:build'` |
| `moon.yml:150-153` | `- 'ts/apps/paigasus-console/{next-env.d.ts,next.config.ts,tsconfig.json,app/**/*}'` | `- 'ts/apps/iam-console/…'` (same four entries) |
| `ts/README.md:18-20,33` | see Step 4 | see Step 4 |
| `ts/apps/iam-console/app/globals.css:21` | `reds paigasus-console-ts:test` | `reds iam-console-ts:test` |
| `ts/apps/iam-console/moon.yml:3` | `id: 'paigasus-console-ts'` | `id: 'iam-console-ts'` |
| `ts/apps/iam-console/moon.yml:17,18,107` | comments | `iam-console-ts`, `ts/apps/iam-console/src/**/*` |
| `ts/apps/iam-console/moon.yml:55` | `.next/standalone/apps/paigasus-console/server.js` | `.next/standalone/apps/iam-console/server.js` |
| `ts/apps/iam-console/moon.yml:56` | `echo "paigasus-console: standalone…` | `echo "iam-console: standalone…` |
| `ts/apps/iam-console/package.json:2` | `"name": "@paigasus/console"` | `"name": "@paigasus/iam-console"` |
| `ts/apps/iam-console/tests/standalone-runtime.test.ts:13` | `'../.next/standalone/apps/paigasus-console/server.js'` | `'../.next/standalone/apps/iam-console/server.js'` |
| `ts/eslint.config.js:53,54` | comments `apps/paigasus-console/` | `apps/iam-console/` |
| `ts/eslint.config.js:58` | `files: ['apps/paigasus-console/**/*.{ts,tsx}']` | `files: ['apps/iam-console/**/*.{ts,tsx}']` |
| `ts/eslint.config.js:60` | `path.join(import.meta.dirname, 'apps/paigasus-console')` | `path.join(import.meta.dirname, 'apps/iam-console')` |
| `ts/packages/paigasus-app-shell/moon.yml:21`, `ts/packages/paigasus-discovery/moon.yml:16`, `ts/packages/paigasus-sdk/moon.yml:17`, `ts/packages/paigasus-ui/moon.yml:12` | comment `paigasus-console-ts:build` | `iam-console-ts:build` |
| `ts/packages/paigasus-auth/tests/config.test.ts:10` | `PAIGASUS_OIDC_CLIENT_ID: 'paigasus-console',` | `PAIGASUS_OIDC_CLIENT_ID: 'iam-console',` |
| `ts/packages/paigasus-auth/tests/config.test.ts:19` | `.toBe('paigasus-console')` | `.toBe('iam-console')` |
| `ts/packages/paigasus-next-config/tests/boundaries.test.ts:75,76,150,151,152` | `'apps/paigasus-console/app/page.tsx'` | `'apps/iam-console/app/page.tsx'` |
| `…/boundaries.test.ts:109,110,165` | `'apps/paigasus-console/middleware.ts'` | `'apps/iam-console/middleware.ts'` |
| `…/boundaries.test.ts:281` | `'apps/paigasus-console/app/probe.mjs'` | `'apps/iam-console/app/probe.mjs'` |
| `ts/packages/paigasus-ui/README.md:33` | `` `paigasus-console-ts:test` `` | `` `iam-console-ts:test` `` |
| `ts/packages/paigasus-ui/README.md:43` | `` `ts/apps/paigasus-console/app/globals.css` `` | `` `ts/apps/iam-console/app/globals.css` `` |
| `ts/packages/paigasus-ui/src/components/table.tsx:15` | comment `ts/apps/paigasus-console/` | `ts/apps/iam-console/` |
| `ts/packages/paigasus-ui/tsconfig.json:6` | comment `ts/apps/paigasus-console/tsconfig.json` | `ts/apps/iam-console/tsconfig.json` |
| `ts/packages/paigasus-ui/vitest.config.ts:8` | `@paigasus/console` | `@paigasus/iam-console` |
| `ts/apps/paigasus-docs/moon.yml:3,9`, `ts/apps/paigasus-docs/package.json:2`, `ts/apps/paigasus-docs/tsconfig.json:2` | — | deleted with the directory |
| `ts/pnpm-lock.yaml:210` | `apps/paigasus-console:` | `apps/iam-console:` (by `pnpm install`) |
| `ts/pnpm-lock.yaml:250-254` | the `apps/paigasus-docs:` importer | removed (by `pnpm install`) |

- [ ] **Step 2: Move the app and delete the docs stub**

`git mv` renames the whole directory on disk, so the untracked `node_modules/` and `.next/` move with it. Delete the old `.next/`, because its standalone tree still holds the old path.

```bash
git mv ts/apps/paigasus-console ts/apps/iam-console
[ -e ts/apps/paigasus-console ] && rm -rf ts/apps/paigasus-console
rm -rf ts/apps/iam-console/.next
git rm -r -q ts/apps/paigasus-docs
rm -rf ts/apps/paigasus-docs
ls ts/apps
```

Expected: `ls ts/apps` prints exactly `iam-console`.

- [ ] **Step 3: Replace the names in every listed file**

```bash
perl -pi -e 's/paigasus-console/iam-console/g; s{\@paigasus/console\b}{\@paigasus/iam-console}g' \
  .moon/tasks/typescript-project.yml .moon/workspace.yml CLAUDE.md CONTRIBUTING.md \
  ci/affected-graph/run.sh ci/next-env/run.sh ci/tailwind-source/README.md ci/tailwind-source/run.mjs \
  moon.yml ts/README.md ts/apps/iam-console/app/globals.css ts/apps/iam-console/moon.yml \
  ts/apps/iam-console/package.json ts/apps/iam-console/tests/standalone-runtime.test.ts \
  ts/eslint.config.js ts/packages/paigasus-app-shell/moon.yml ts/packages/paigasus-auth/tests/config.test.ts \
  ts/packages/paigasus-discovery/moon.yml ts/packages/paigasus-next-config/tests/boundaries.test.ts \
  ts/packages/paigasus-sdk/moon.yml ts/packages/paigasus-ui/README.md ts/packages/paigasus-ui/moon.yml \
  ts/packages/paigasus-ui/src/components/table.tsx ts/packages/paigasus-ui/tsconfig.json \
  ts/packages/paigasus-ui/vitest.config.ts
```

The two affected-graph cases keep their labels `ui->console` and `ui-components->console`. Only the task ids in their expected sets change. The comparison sorts both sets, so the position of the ids in the CSV does not matter.

- [ ] **Step 4: Edit the workspace README by hand**

In `ts/README.md`, replace lines 18–20:

```markdown
- `apps/*` — deployables (id `paigasus-<name>-ts`):
  - `iam-console` (`@paigasus/iam-console`) — Next.js 16 (App Router) operator console
  - `paigasus-docs` (`@paigasus/docs`) — framework TBD; framework choice tracked in a follow-up SMA-NNN issue
```

with:

```markdown
- `apps/*` — deployables, one Next.js app per console zone (id `<name>-ts`):
  - `iam-console` (`@paigasus/iam-console`) — Next.js 16 (App Router) console zone for IAM, mounted at `/iam`
```

Line 33 already reads `moon run iam-console-ts:build` after Step 3. Prettier aligns Markdown tables, and the shorter name can change how Prettier wraps a line. So format every `ts/` file that Step 3 and this step edit (the Global Constraints):

```bash
pnpm -C ts exec prettier --write README.md apps/iam-console/app/globals.css apps/iam-console/moon.yml \
  apps/iam-console/package.json apps/iam-console/tests/standalone-runtime.test.ts eslint.config.js \
  packages/paigasus-app-shell/moon.yml packages/paigasus-auth/tests/config.test.ts packages/paigasus-discovery/moon.yml \
  packages/paigasus-next-config/tests/boundaries.test.ts packages/paigasus-sdk/moon.yml packages/paigasus-ui/README.md \
  packages/paigasus-ui/moon.yml packages/paigasus-ui/src/components/table.tsx packages/paigasus-ui/tsconfig.json \
  packages/paigasus-ui/vitest.config.ts
```

`ts/pnpm-lock.yaml` is not in the list: `ts/.prettierignore` excludes it, and `pnpm install` writes it in Step 5.

- [ ] **Step 5: Update the lockfile**

```bash
pnpm -C ts install --no-frozen-lockfile
grep -n '^  apps/' ts/pnpm-lock.yaml
```

Expected: the grep prints exactly one line, `210:  apps/iam-console:` (`-n` adds the line number). `git diff --stat ts/pnpm-lock.yaml` shows only the importer rename and the removed `apps/paigasus-docs` block (about 10 lines).

- [ ] **Step 6: Prove that no old name remains**

```bash
git grep -n paigasus-console -- ':!docs/superpowers'; echo "exit $?"
git grep -n -e paigasus-docs -e '@paigasus/docs' -e '@paigasus/console' -- ':!docs/superpowers'; echo "exit $?"
moon project iam-console-ts > /dev/null; echo "new id exit $?"
moon project paigasus-console-ts > /dev/null 2>&1; echo "old id exit $?"
```

Expected: both greps print nothing and `exit 1`. `new id exit 0`. `old id exit` is not 0.

- [ ] **Step 7: Run the suites that name the app**

```bash
moon run iam-console-ts:build iam-console-ts:test iam-console-ts:typecheck
moon run repo:next-env-drift repo:input-liveness --force
moon run paigasus-next-config-ts:test paigasus-auth-ts:test paigasus-ui-ts:test
moon run ts:lint ts:fmt --force
```

Expected: every target passes. `iam-console-ts:build` prints no `standalone entry point missing` line. `iam-console-ts:test` runs `tests/standalone-runtime.test.ts` (2 passed) and the three `ci/tailwind-source/run.mjs` modes. `repo:input-liveness` passes, which proves the four `ts/apps/iam-console/…` inputs of `repo:next-env-drift` match tracked files.

- [ ] **Step 8: Run the affected-graph suite**

Use the system bash. Bash 5.3.15 on this machine deadlocks on this script.

```bash
/bin/bash ci/affected-graph/run.sh
```

Expected: a line that starts `PASS  ui->console            -> ` and a line that starts `PASS  ui-components->console -> `, each followed by the sorted task ids, and the last line `== affected-graph cascade intact ==`. `run.sh:113` prints a PASS line as `printf 'PASS  %-22s -> %s\n'`: the label has no brackets and is padded to 22 characters. Only a `FAIL  [label]` line has brackets.

- [ ] **Step 9: Commit**

```bash
# `git rm` in Step 2 already staged the paigasus-docs deletion; a pathspec for a removed path fails.
git add -A ts/apps/iam-console ts/pnpm-lock.yaml \
  .moon/tasks/typescript-project.yml .moon/workspace.yml CLAUDE.md CONTRIBUTING.md \
  ci/affected-graph/run.sh ci/next-env/run.sh ci/tailwind-source/README.md ci/tailwind-source/run.mjs \
  moon.yml ts/README.md ts/eslint.config.js ts/packages/paigasus-app-shell/moon.yml \
  ts/packages/paigasus-auth/tests/config.test.ts ts/packages/paigasus-discovery/moon.yml \
  ts/packages/paigasus-next-config/tests/boundaries.test.ts ts/packages/paigasus-sdk/moon.yml \
  ts/packages/paigasus-ui/README.md ts/packages/paigasus-ui/moon.yml \
  ts/packages/paigasus-ui/src/components/table.tsx ts/packages/paigasus-ui/tsconfig.json \
  ts/packages/paigasus-ui/vitest.config.ts
git status --short
git commit -F - <<'EOF'
refactor(ts): rename the console app to iam-console and delete paigasus-docs (SMA-511)

SMA-512 adds a gateway zone app, so the name paigasus-console no longer says which zone this app
is. The package becomes @paigasus/iam-console and the Moon id becomes iam-console-ts. Every site
that named the old path or id changes here, including the two affected-graph cases that list the
app's tasks. docs/superpowers keeps the old name, because those files record history.

paigasus-docs held only an empty module and nothing depended on it.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

Expected: `git status --short` before the commit shows `R` rows for the 16 app files, `D` rows for the 4 docs files and `M` rows for the other files. The commit-msg hook passes.

### Task 2: The `paigasus/no-js-relative-specifier` rule and the `sourceRules` export

Spec § 7.2 "Enforcement". This task adds the rule and its tests. It does NOT spread `sourceRules` into `ts/eslint.config.js`; Task 3 does that, after the sources are clean.

Why a custom rule and a separate export: in flat config, a second `no-restricted-imports` block that matches the same files REPLACES the first one, so it would switch off the `sdk`, `auth-*` and `discovery` boundary blocks without a warning. A `packages/*/src` scope inside `boundaryRules` would also fail the reverse liveness loop in `tests/boundaries.test.ts:236-243`, because its derived scope key `packages/*/src` is not a `BOUNDARY_SCOPES` entry.

**Files:**
- Modify: `ts/packages/paigasus-next-config/src/eslint.mjs` — header comment lines 33–40; insert the rule, the plugin and `sourceRules` between line 303 (`];` that closes `boundaryRules`) and line 305 (`export default boundaryRules;`).
- Create: `ts/packages/paigasus-next-config/tests/no-js-relative-specifier.test.ts`

**Interfaces:**
- Consumes: `eslint` 10.9.1 (`RuleTester`, `ESLint`, `Linter`, `Rule`), `typescript-eslint` 8.69.0 (`tseslint.parser`) — both are already devDependencies of `@paigasus/next-config`.
- Produces (from `@paigasus/next-config/eslint`):
  - `export const noJsRelativeSpecifier: import('eslint').Rule.RuleModule` — message id `jsSpecifier`, data `{ specifier }`.
  - `export const sourceRules: import('eslint').Linter.Config[]` — one block named `paigasus/source/no-js-relative-specifier`, `files: ['packages/*/src/**/*.{ts,tsx,mts,cts}']`, `ignores: ['**/*.test.*', '**/tests/**']`, plugin key `paigasus`, rule `'paigasus/no-js-relative-specifier': 'error'`.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-next-config/tests/no-js-relative-specifier.test.ts`:

```ts
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
```

- [ ] **Step 2: Run the test and see it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-next-config exec vitest run tests/no-js-relative-specifier.test.ts
```

Expected: FAIL at collection. `noJsRelativeSpecifier` and `sourceRules` are not exported yet, so `noJsRelativeSpecifier` is `undefined`. `ruleTester.run` runs at the top level of the module, and RuleTester's `assertRule` throws an `AssertionError`: `` Rule paigasus/no-js-relative-specifier must be an object with a `create` method ``. vitest fails the whole file, and no case runs.

- [ ] **Step 3: Write the rule and the export**

In `ts/packages/paigasus-next-config/src/eslint.mjs`, replace the header paragraph at lines 33–34:

```js
// Written as plain ESM rather than TypeScript: ts/eslint.config.js is loaded by ESLint's own
// resolver, and configuration data gains little from types (spec § 13 M5).
```

with:

```js
// Written as plain ESM rather than TypeScript: ts/eslint.config.js is loaded by ESLint's own
// resolver, and configuration data gains little from types (spec § 13 M5).
//
// A SECOND EXPORT, `sourceRules`, carries the custom rule `paigasus/no-js-relative-specifier`
// (SMA-511 spec § 7.2). It is not a boundaryRules block, on purpose: see its own doc comment.
```

Then insert this block between line 303 (`];`) and line 305 (`export default boundaryRules;`):

```js

/**
 * `paigasus/no-js-relative-specifier` (SMA-511 spec § 7.2).
 *
 * MEASURED (SMA-510): Turbopack in Next 16.3.4 does not resolve a `.js` relative specifier to a
 * `.ts` file. A package whose src/ writes `from './x.js'` therefore breaks `next build` in every
 * zone that compiles it, and vitest, tsc and ESLint all accept the specifier. This rule reports an
 * import declaration, an `export … from` and an `import()` with a string literal, whose specifier
 * starts with `./` or `../` and ends in `.js`. Bare package specifiers are not reported.
 *
 * @type {import('eslint').Rule.RuleModule}
 */
export const noJsRelativeSpecifier = {
  meta: {
    type: 'problem',
    docs: {
      description: 'Disallow a relative module specifier that ends in .js (Turbopack does not resolve it to a .ts file)',
    },
    schema: [],
    messages: {
      jsSpecifier:
        "Relative specifier '{{specifier}}' ends in .js. Turbopack (Next 16.3.4) does not resolve a .js relative specifier to a .ts file, so every Next zone that compiles this file fails to build. Remove the extension (SMA-511 spec § 7.2).",
    },
  },
  create(context) {
    const check = (source) => {
      if (source === null || source === undefined || source.type !== 'Literal' || typeof source.value !== 'string') return;
      const specifier = source.value;
      if (/^\.\.?\//.test(specifier) && specifier.endsWith('.js')) {
        context.report({ node: source, messageId: 'jsSpecifier', data: { specifier } });
      }
    };
    return {
      ImportDeclaration: (node) => check(node.source),
      ExportNamedDeclaration: (node) => check(node.source),
      ExportAllDeclaration: (node) => check(node.source),
      ImportExpression: (node) => check(node.source),
    };
  },
};

/** @type {import('eslint').ESLint.Plugin} */
const paigasusPlugin = {
  meta: { name: '@paigasus/next-config/eslint' },
  rules: { 'no-js-relative-specifier': noJsRelativeSpecifier },
};

/**
 * Source-hygiene blocks, as an ESLint flat-config array. `ts/eslint.config.js` spreads it next to
 * `boundaryRules`, and `tests/boundaries.test.ts` pins that spread.
 *
 * WHY NOT INSIDE `boundaryRules`. The liveness test there derives a `BOUNDARY_SCOPES` key from every
 * block's `files[0]`; `packages/*` is not a package directory, so the reverse loop would fail.
 *
 * WHY A CUSTOM RULE. A second `no-restricted-imports` block that matches the same files REPLACES
 * the first one's options in flat config. It would switch off the sdk, auth-* and discovery
 * boundary blocks for every file under packages/*\/src, with no error anywhere.
 *
 * Scope: package sources only. Test files are excluded (vitest resolves `.js` to `.ts`). Apps are
 * excluded, because `next build` itself fails on such a specifier in app code. Generated proto code
 * is excluded by the workspace's global `**\/generated/**` ignore; a proto test holds it instead.
 *
 * @type {import('eslint').Linter.Config[]}
 */
export const sourceRules = [
  {
    name: 'paigasus/source/no-js-relative-specifier',
    files: ['packages/*/src/**/*.{ts,tsx,mts,cts}'],
    ignores: ['**/*.test.*', '**/tests/**'],
    plugins: { paigasus: paigasusPlugin },
    rules: { 'paigasus/no-js-relative-specifier': 'error' },
  },
];
```

Note the escaped `*\/` inside the two doc-comment globs: an unescaped `*/` ends the JSDoc block.

- [ ] **Step 4: Run the test and see it pass**

```bash
pnpm -C ts/packages/paigasus-next-config exec vitest run tests/no-js-relative-specifier.test.ts
```

Expected: PASS — 24 RuleTester cases (14 valid, 10 invalid) and 11 scope cases. If RuleTester reports "detected duplicate test case", a case string repeats; make it unique.

- [ ] **Step 5: Run the package and workspace checks**

```bash
pnpm -C ts exec prettier --write packages/paigasus-next-config/src/eslint.mjs packages/paigasus-next-config/tests/no-js-relative-specifier.test.ts
moon run paigasus-next-config-ts:test paigasus-next-config-ts:typecheck
moon run ts:lint ts:fmt --force
```

Expected: all pass. `tests/boundaries.test.ts` stays green, because `boundaryRules` did not change.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-next-config/src/eslint.mjs ts/packages/paigasus-next-config/tests/no-js-relative-specifier.test.ts
git commit -F - <<'EOF'
feat(ts): add the paigasus/no-js-relative-specifier eslint rule (SMA-511)

Turbopack in Next 16.3.4 does not resolve a relative specifier that ends in .js to a .ts file, so
a package whose src/ uses one breaks the build of every Next zone that compiles it. vitest, tsc and
ESLint all accept such a specifier, so nothing else notices.

The rule reports an import, an export-from and an import() with a string literal. It ships as the
separate sourceRules export of @paigasus/next-config/eslint, scoped to packages/*/src without the
tests. It is a custom rule with its own name, because a second no-restricted-imports block would
replace the boundary blocks for the same files. ts/eslint.config.js does not spread it yet.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 3: Extensionless relative imports in package sources, the loader retry, and the rule spread

Spec § 7.2. Four packages that a Next zone compiles use `.js` relative specifiers in `src/`: `@paigasus/auth` (74), `@paigasus/sdk` (29), `@paigasus/discovery` (43) and the hand-written part of `@paigasus/proto` (16). The proto files are in this task, not in Task 4, because this task spreads `sourceRules`, which covers `packages/*/src`: with `.js` left in them, `ts:lint` fails at the end of this task. This task removes the extension from all 162, teaches the two plain-Node loaders to resolve an extensionless specifier, and turns the Task 2 rule on for the whole workspace.

All packages use `moduleResolution: bundler` (`ts/tsconfig.base.json:4`), so tsc accepts extensionless specifiers. Test files keep their `.js` specifiers; vitest resolves them.

**Files:**
- Modify: `ts/packages/paigasus-auth/tests/fixtures/ts-esm-loader.mjs` (whole file, 27 lines)
- Modify: `ts/packages/paigasus-discovery/tests/containers/support/ts-esm-loader.mjs` (whole file, 27 lines)
- Create: `ts/packages/paigasus-auth/tests/ts-esm-loader.test.ts`
- Create: `ts/packages/paigasus-discovery/tests/ts-esm-loader.test.ts`
- Modify by script (specifier text only), 41 files:
  - auth (15): `src/adapters/{claims-resolver,memory-store,noop-logger,oidc,redis-store}.ts`, `src/client.ts`, `src/core/{session,single-flight}.ts`, `src/http/routes.ts`, `src/middleware.ts`, `src/next/get-session.ts`, `src/ports/{principal-resolver,session-store}.ts`, `src/runtime.ts`, `src/server.ts`
  - sdk (8): `src/chat.ts`, `src/errors.ts`, `src/errors/{map-error,presentation,transport-status}.ts`, `src/iam.ts`, `src/index.ts`, `src/transport.ts`
  - discovery (13): `src/adapters/{memory-cache,noop-logger,redis-cache}.ts`, `src/config.ts`, `src/core/{reasons,record,single-flight,state}.ts`, `src/disabled.tsx`, `src/ports/cache.ts`, `src/probe.ts`, `src/react.tsx`, `src/server.ts`
  - proto (5): `src/{audit,capability,error,iam,index}.ts`
- Modify: `ts/packages/paigasus-sdk/tests/server-guard.test.ts:19-25,36,76-77`
- Modify: `ts/packages/paigasus-sdk/src/errors.ts:3-5` and `ts/packages/paigasus-sdk/src/errors/types.ts:3-5` (comments that quote the guard import)
- Modify: `ts/packages/paigasus-discovery/tests/structure/exports.test.ts:46`
- Modify: `ts/packages/paigasus-next-config/tests/boundaries.test.ts` — line 8 (import), the comments at lines 92–96 and 102–104, new DENIED rows after line 110, new tests inside the describe at lines 270–294
- Modify: `ts/packages/paigasus-next-config/src/eslint.mjs:236` (the `auth-client` message) and `:256-260` (a comment). These are lines 233 and 253–257 before Task 2 added three header lines.
- Modify: `ts/packages/paigasus-auth/tests/structure/import-graph.test.ts:5` and `ts/packages/paigasus-auth/tests/support/import-graph.ts:42` (comments)
- Modify: `ts/eslint.config.js:9` and `:65-71`

**Interfaces:**
- Consumes: `sourceRules` from Task 2 (`@paigasus/next-config/eslint`).
- Produces: every relative specifier in `packages/*/src` (tests excluded) is extensionless; `ts:lint` enforces it. Both loaders resolve `./x` to `./x.ts`, then `./x/index.ts`, and still resolve `./x.js` to `./x.ts`.

- [ ] **Step 1: Write the failing loader tests**

Create `ts/packages/paigasus-auth/tests/ts-esm-loader.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-511 spec § 7.2. Two plain-`node` harnesses run this package's source through
// tests/fixtures/ts-esm-loader.mjs: the e2e fixture server and the multi-process single-flight
// worker. Both run only in `test-e2e`, behind Docker. This suite proves the loader itself on every
// `test` run, against a throwaway module tree, so a broken retry fails here and not only in e2e.
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const LOADER = pathToFileURL(fileURLToPath(new URL('./fixtures/ts-esm-loader.mjs', import.meta.url))).href;

let root: string;

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'auth-ts-esm-loader-'));
  mkdirSync(join(root, 'dir'));
  writeFileSync(join(root, 'leaf.ts'), 'export const leaf: number = 40;\n');
  writeFileSync(join(root, 'dir', 'index.ts'), 'export const fromDir: number = 2;\n');
  writeFileSync(join(root, 'extensionless.ts'), "import { leaf } from './leaf';\nimport { fromDir } from './dir';\nconsole.log(String(leaf + fromDir));\n");
  writeFileSync(join(root, 'js-suffixed.ts'), "import { leaf } from './leaf.js';\nconsole.log(String(leaf));\n");
  writeFileSync(join(root, 'missing.ts'), "import { nothing } from './does-not-exist';\nconsole.log(String(nothing));\n");
});

afterAll(() => {
  rmSync(root, { recursive: true, force: true });
});

function run(entry: string, withLoader = true) {
  const args = withLoader ? ['--import', LOADER, join(root, entry)] : [join(root, entry)];
  return spawnSync(process.execPath, args, { encoding: 'utf8', timeout: 30_000 });
}

describe('tests/fixtures/ts-esm-loader.mjs', () => {
  it('resolves an extensionless specifier to ./x.ts, and a directory to ./x/index.ts', () => {
    const result = run('extensionless.ts');
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout.trim()).toBe('42');
  });

  it('still resolves a .js specifier to its .ts sibling (the test files keep that form)', () => {
    const result = run('js-suffixed.ts');
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout.trim()).toBe('40');
  });

  it('still fails on a specifier that names no file, instead of hiding it', () => {
    const result = run('missing.ts');
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('ERR_MODULE_NOT_FOUND');
    expect(result.stderr).toContain('does-not-exist');
  });

  it('control: plain node without the loader cannot resolve the extensionless tree', () => {
    const result = run('extensionless.ts', false);
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('ERR_MODULE_NOT_FOUND');
  });
});
```

Create `ts/packages/paigasus-discovery/tests/ts-esm-loader.test.ts` with the same content, except for four places. The header comment's first paragraph becomes:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-511 spec § 7.2. tests/containers/single-flight-multiprocess.test.ts runs this package's source
// through tests/containers/support/ts-esm-loader.mjs in plain `node` workers, only in `test-e2e`,
// behind Docker. This suite proves the loader itself on every `test` run, against a throwaway module
// tree, so a broken retry fails here and not only in e2e.
```

The loader URL and the temp-dir prefix become:

```ts
const LOADER = pathToFileURL(fileURLToPath(new URL('./containers/support/ts-esm-loader.mjs', import.meta.url))).href;
```

```ts
  root = mkdtempSync(join(tmpdir(), 'discovery-ts-esm-loader-'));
```

and the describe title `'tests/containers/support/ts-esm-loader.mjs'`. Write the whole file out; do not import one package's test from the other.

- [ ] **Step 2: Run the loader tests and see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-auth exec vitest run tests/ts-esm-loader.test.ts
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/ts-esm-loader.test.ts
```

Expected: in each package, FAIL on the first case (status 1, `ERR_MODULE_NOT_FOUND` for `./leaf`); the other three cases pass.

- [ ] **Step 3: Add the extensionless retry to both loaders**

Replace the whole of `ts/packages/paigasus-auth/tests/fixtures/ts-esm-loader.mjs` with:

```js
// SPDX-License-Identifier: Apache-2.0
//
// tests/containers/single-flight-multiprocess.test.ts forks tests/fixtures/refresh-worker.ts as a
// standalone `node` process, and playwright.config.ts starts tests/e2e/fixture-server.ts the same
// way — not through vitest/Vite. Node's built-in TypeScript support strips types but resolves
// module specifiers LITERALLY: it maps no `.js` specifier onto a `.ts` file and it probes no
// extension at all. vitest's Vite-based resolver does both. This hook restores them for a plain
// `node` process, in two retries after a failed resolution:
//
//   1. `./x.js` → `./x.ts` — the test files, which keep `.js` specifiers.
//   2. `./x` → `./x.ts`, then `./x/index.ts` — the package sources, which are EXTENSIONLESS since
//      SMA-511 because Turbopack (Next 16.3.4) does not resolve `./x.js` to `./x.ts` (spec § 7.2).
//
// A specifier that still names no file fails with its ORIGINAL error. tests/ts-esm-loader.test.ts
// proves all three outcomes. tests/containers/support/ts-esm-loader.mjs in @paigasus/discovery is
// a copy of this file; change both together.
import { register } from 'node:module';

register(import.meta.url);

const NOT_FOUND = new Set(['ERR_MODULE_NOT_FOUND', 'ERR_UNSUPPORTED_DIR_IMPORT']);
const HAS_MODULE_EXTENSION = /\.(?:[cm]?[jt]sx?|json)$/;

/** @param {unknown} err */
function codeOf(err) {
  return err && typeof err === 'object' && 'code' in err ? err.code : undefined;
}

/** @type {import('node:module').ResolveHook} */
export async function resolve(specifier, context, nextResolve) {
  try {
    return await nextResolve(specifier, context);
  } catch (err) {
    const code = codeOf(err);
    if (specifier.endsWith('.js') && code === 'ERR_MODULE_NOT_FOUND') {
      return nextResolve(`${specifier.slice(0, -3)}.ts`, context);
    }
    if (/^\.\.?\//.test(specifier) && !HAS_MODULE_EXTENSION.test(specifier) && NOT_FOUND.has(code)) {
      for (const candidate of [`${specifier}.ts`, `${specifier}/index.ts`]) {
        try {
          return await nextResolve(candidate, context);
        } catch (retryErr) {
          if (!NOT_FOUND.has(codeOf(retryErr))) throw retryErr;
        }
      }
    }
    throw err;
  }
}
```

Replace the whole of `ts/packages/paigasus-discovery/tests/containers/support/ts-esm-loader.mjs` with the same code. Use this header instead of the auth one:

```js
// SPDX-License-Identifier: Apache-2.0
//
// single-flight-multiprocess.test.ts spawns tests/containers/support/worker.ts as a standalone
// `node` process — not through vitest/Vite. Node's built-in TypeScript support strips types but
// resolves module specifiers LITERALLY: it maps no `.js` specifier onto a `.ts` file and it probes
// no extension at all. vitest's Vite-based resolver does both. This hook restores them for a plain
// `node` process, in two retries after a failed resolution:
//
//   1. `./x.js` → `./x.ts` — the test files, which keep `.js` specifiers.
//   2. `./x` → `./x.ts`, then `./x/index.ts` — the package sources, which are EXTENSIONLESS since
//      SMA-511 because Turbopack (Next 16.3.4) does not resolve `./x.js` to `./x.ts` (spec § 7.2).
//
// A specifier that still names no file fails with its ORIGINAL error. tests/ts-esm-loader.test.ts
// proves all three outcomes. This file is a copy of
// ts/packages/paigasus-auth/tests/fixtures/ts-esm-loader.mjs; change both together.
```

- [ ] **Step 4: Run the loader tests and see them pass**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/ts-esm-loader.test.ts
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/ts-esm-loader.test.ts
```

Expected: PASS, 4 of 4 in each package.

- [ ] **Step 5: Commit the loaders**

```bash
pnpm -C ts exec prettier --write packages/paigasus-auth/tests/fixtures/ts-esm-loader.mjs packages/paigasus-auth/tests/ts-esm-loader.test.ts packages/paigasus-discovery/tests/containers/support/ts-esm-loader.mjs packages/paigasus-discovery/tests/ts-esm-loader.test.ts
git add ts/packages/paigasus-auth/tests/fixtures/ts-esm-loader.mjs ts/packages/paigasus-auth/tests/ts-esm-loader.test.ts ts/packages/paigasus-discovery/tests/containers/support/ts-esm-loader.mjs ts/packages/paigasus-discovery/tests/ts-esm-loader.test.ts
git commit -F - <<'EOF'
test(ts): retry extensionless specifiers in the plain-node test loaders (SMA-511)

The package sources lose their .js relative specifiers in the next commit, because Turbopack does
not resolve ./x.js to ./x.ts. Plain Node probes no extension, so the two loaders that run package
source in plain node processes now retry ./x as ./x.ts and then ./x/index.ts. A new unit test per
package proves the retry, the old .js retry, and that a missing file still fails.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

- [ ] **Step 6: Write the specifier script**

Write this script to your scratchpad directory as `strip-js-specifiers.mjs` (it is a tool, not a repo file). It parses each file with the TypeScript compiler, so a `.js` inside a comment or a string is never touched. It edits import and export declarations, clause-level `import type`/`export type`, and `import()` with a string literal. It skips `*.test.*`, `*.d.ts`, and any `tests/`, `generated/` or `node_modules/` directory.

```js
// SPDX-License-Identifier: Apache-2.0
// Usage (cwd = ts/): node <this file> [--write] <dir> [<dir> ...]
// Without --write it only reports. With --write it removes the `.js` suffix from every RELATIVE
// module specifier (starts with ./ or ../) in *.ts/*.tsx/*.mts/*.cts files under the given dirs.
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { join, relative } from 'node:path';

const ts = createRequire(join(process.cwd(), 'package.json'))('typescript');
const args = process.argv.slice(2);
const write = args.includes('--write');
const dirs = args.filter((a) => a !== '--write');

function walk(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === 'tests' || entry.name === 'generated' || entry.name === 'node_modules') continue;
      out.push(...walk(full));
    } else if (/\.(ts|tsx|mts|cts)$/.test(entry.name) && !/\.test\./.test(entry.name) && !entry.name.endsWith('.d.ts')) {
      out.push(full);
    }
  }
  return out;
}

const isTarget = (s) => /^\.\.?\//.test(s) && s.endsWith('.js');
let total = 0;
for (const dir of dirs) {
  for (const file of walk(dir)) {
    const text = readFileSync(file, 'utf8');
    const kind = file.endsWith('x') ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
    const sf = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, kind);
    const edits = [];
    const visit = (node) => {
      let lit;
      if ((ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) && node.moduleSpecifier && ts.isStringLiteral(node.moduleSpecifier)) lit = node.moduleSpecifier;
      else if (ts.isCallExpression(node) && node.expression.kind === ts.SyntaxKind.ImportKeyword && node.arguments[0] && ts.isStringLiteral(node.arguments[0])) lit = node.arguments[0];
      if (lit && isTarget(lit.text)) edits.push(lit.getEnd() - 1);
      ts.forEachChild(node, visit);
    };
    visit(sf);
    if (edits.length === 0) continue;
    total += edits.length;
    console.log(`${relative(process.cwd(), file)}: ${edits.length}`);
    if (write) {
      let next = text;
      // Right to left, so earlier offsets stay valid. Each offset is the closing quote; the three
      // characters before it are `.js`.
      for (const end of edits.sort((a, b) => b - a)) next = next.slice(0, end - 3) + next.slice(end);
      writeFileSync(file, next);
    }
  }
}
console.log(`total: ${total}${write ? ' (written)' : ' (dry run)'}`);
```

- [ ] **Step 7: Dry-run the script and check the counts**

```bash
cd ts
node "$SCRATCH/strip-js-specifiers.mjs" packages/paigasus-auth/src packages/paigasus-sdk/src packages/paigasus-discovery/src packages/paigasus-proto/src
cd ..
```

(`$SCRATCH` is your scratchpad directory.) Expected: 41 file lines and `total: 162 (dry run)`. Per package: auth 74 in 15 files (`server.ts: 24`, `runtime.ts: 10`, `http/routes.ts: 9`, `core/single-flight.ts: 7`, `next/get-session.ts: 6`, `adapters/redis-store.ts: 4`, `adapters/memory-store.ts: 3`, `adapters/oidc.ts: 2`, `core/session.ts: 2`, `middleware.ts: 2`, one each in `adapters/claims-resolver.ts`, `adapters/noop-logger.ts`, `client.ts`, `ports/principal-resolver.ts`, `ports/session-store.ts`); sdk 29 in 8 files; discovery 43 in 13 files (`server.ts: 18`); proto 16 in 5 files (`index.ts: 12`). If a count differs, a file changed after this plan was written: read the new hits before you continue.

- [ ] **Step 8: Apply the edit and check that nothing is left**

```bash
cd ts
node "$SCRATCH/strip-js-specifiers.mjs" --write packages/paigasus-auth/src packages/paigasus-sdk/src packages/paigasus-discovery/src packages/paigasus-proto/src
node "$SCRATCH/strip-js-specifiers.mjs" packages/*/src
pnpm exec prettier --write packages/paigasus-auth/src packages/paigasus-sdk/src packages/paigasus-discovery/src packages/paigasus-proto/src
cd ..
git diff --stat -- ts/packages/*/src
```

Expected: the second run prints `total: 0 (dry run)` over every package. `git diff --stat` lists the 41 files and no others. Each changed line differs only by a removed `.js`, except where prettier joins a wrapped import that now fits in 200 columns.

- [ ] **Step 9: Update the places that pin or prescribe the old form**

1. `ts/packages/paigasus-sdk/tests/server-guard.test.ts`. Replace lines 19–25:

```ts
// The guard's expected specifier depends on where the entry FILE sits, not on one fixed literal.
// `src/index.ts` needs './server-guard.js'; a nested entry such as `src/errors/map-error.ts` needs
// '../server-guard.js'. Spec § 6.2 layer 3 always said this test "resolves the path relative to
// each entry" — the original literal did not, so a nested guarded entry could not satisfy it and
// the file layout had to be bent around the test. Ported from PR #231, which fixed the test
// instead. (SMA-625)
const GUARD_MODULE = 'src/server-guard.js';
```

with:

```ts
// The guard's expected specifier depends on where the entry FILE sits, not on one fixed literal.
// `src/index.ts` needs './server-guard'; a nested entry such as `src/errors/map-error.ts` needs
// '../server-guard'. Spec § 6.2 layer 3 always said this test "resolves the path relative to
// each entry" — the original literal did not, so a nested guarded entry could not satisfy it and
// the file layout had to be bent around the test. Ported from PR #231, which fixed the test
// instead. (SMA-625)
//
// EXTENSIONLESS since SMA-511: Turbopack does not resolve './x.js' to './x.ts' (spec § 7.2), and
// `paigasus/no-js-relative-specifier` now rejects the `.js` form in src/.
const GUARD_MODULE = 'src/server-guard';
```

Replace the regular expression on line 36, `/(?:^|\/)server-guard\.js$/`, with `/(?:^|\/)server-guard(?:\.js)?$/`, so the "deliberately carries no guard" check still rejects both spellings. On line 77, replace `a stray './server-guard.js' in a nested file` with `a stray './server-guard' (or './server-guard.js') in a nested file`.

2. The comments that quote the guard import. In `ts/packages/paigasus-sdk/src/errors.ts:3-5`, replace:

```ts
// The `./errors` entry. It is GUARDED and lives at src/ root: tests/server-guard.test.ts:19 pins
// the literal "import './server-guard.js';" and compares it with toBe at :52, so an entry one
// directory deep would need '../server-guard.js' and fail.
```

with:

```ts
// The `./errors` entry. It is GUARDED and lives at src/ root. tests/server-guard.test.ts derives
// the guard import each entry must open with from the entry's own depth ("import
// './server-guard';" here).
```

In `ts/packages/paigasus-sdk/src/errors/types.ts:3-4`, replace `` `import './server-guard.js'` `` with `` `import '../server-guard'` ``.

3. `ts/packages/paigasus-discovery/tests/structure/exports.test.ts:46`. Replace:

```ts
  'src/disabled.tsx': ['./types.js', 'react'],
```

with:

```ts
  'src/disabled.tsx': ['./types', 'react'],
```

4. One lint message and five comments still tell a developer that the code base always writes `.js`. The new rule rejects that form in `src/`, so change them. Each replacement keeps the line count of the old text, so the line numbers that Step 10, Step 11 and later tasks cite stay correct.

In `ts/packages/paigasus-next-config/src/eslint.mjs:236` (the `paigasus/boundaries/auth-client` message), replace:

```js
          '@paigasus/auth/client is React-only and must never reach the server surface — it would put a token in a browser bundle (AC 5). Import the shared vocabulary from ./session-view.js only.',
```

with:

```js
          '@paigasus/auth/client is React-only and must never reach the server surface — it would put a token in a browser bundle (AC 5). Import the shared vocabulary from ./session-view only.',
```

In the same file, lines 256–260 (the `paigasus/boundaries/auth-middleware` block), replace:

```js
          // NOT './http/**' — src/middleware.ts legitimately imports './http/cookies.js' for
          // the cookie-presence check ADR-0017 decision 7 actually authorizes. What must stay
          // banned is the composition-root surface, './http/routes.js', which pulls in the full
          // session-resolution machinery (openid-client, the store) that middleware must never
          // reach.
```

with:

```js
          // NOT './http/**' — src/middleware.ts legitimately imports './http/cookies' (it wrote
          // './http/cookies.js' until SMA-511; extensionless since) for the cookie-presence check
          // ADR-0017 decision 7 actually authorizes. What must stay banned is the composition-root
          // surface, './http/routes' (both spellings are listed), which pulls in the full
          // session-resolution machinery (openid-client, the store) that middleware must never reach.
```

In `ts/packages/paigasus-next-config/tests/boundaries.test.ts:92-96`, replace:

```ts
  // EXTENSION-BEARING. This codebase always suffixes relative imports with `.js` (a real file
  // never writes `from './runtime'` — it writes `from './runtime.js'`), and no-restricted-imports
  // matches the specifier AS WRITTEN. A bare `'./runtime'` pattern with no `.js` sibling and no
  // glob matches nothing a real file would ever import — these four rows are what proved that
  // (fix round 2).
```

with:

```ts
  // EXTENSION-BEARING. This codebase wrote `.js` on every relative import until SMA-511 (a real
  // file wrote `from './runtime.js'`, never `from './runtime'`); package src/ is extensionless since.
  // no-restricted-imports matches the specifier AS WRITTEN. A bare `'./runtime'` pattern with no `.js`
  // sibling and no glob matched nothing a real file imported then — these four rows are what proved
  // that (fix round 2). The SMA-511 rows below prove that the groups also match the extensionless form.
```

In the same file, lines 102–104, replace:

```ts
  // Four dead entries survived earlier in this branch because a bare './runtime' does not match
  // the '.js'-suffixed specifier a real file would write — these use the `.js` form a real file
  // in this codebase always writes, the same lesson the auth/client rows above already record.
```

with:

```ts
  // Four dead entries survived earlier in this branch because a bare './runtime' did not match
  // the '.js'-suffixed specifier a real file wrote then — these use the `.js` form that real files
  // wrote until SMA-511 (extensionless since), the same lesson the auth/client rows above record.
```

In `ts/packages/paigasus-auth/tests/structure/import-graph.test.ts:5`, replace:

```ts
// because this codebase suffixes relative imports with `.js` (`./runtime.js`). Those two entries
```

with:

```ts
// because this codebase then wrote relative imports with `.js` (`./runtime.js`; src/ is extensionless since SMA-511). Those two entries
```

In `ts/packages/paigasus-auth/tests/support/import-graph.ts:42`, replace:

```ts
/** Resolve a relative import specifier (repo convention: `.js` extension, real file is `.ts`/`.tsx`) to a file on disk. */
```

with:

```ts
/** Resolve a relative import specifier to a file on disk. src/ wrote `.js` until SMA-511 and is extensionless since; tests keep `.js`. The real file is `.ts`/`.tsx`. */
```

- [ ] **Step 10: Run the package suites**

```bash
moon run paigasus-auth-ts:test paigasus-auth-ts:typecheck paigasus-sdk-ts:test paigasus-sdk-ts:typecheck \
  paigasus-discovery-ts:test paigasus-discovery-ts:typecheck paigasus-proto-ts:test paigasus-proto-ts:typecheck \
  paigasus-app-shell-ts:test paigasus-next-config-ts:test
```

Expected: all pass. The auth import-graph tests stay green: `tests/support/import-graph.ts:43-51` strips an optional `.js` and then tries `.ts`/`.tsx`, so it resolves an extensionless specifier too.

- [ ] **Step 11: Write the failing tests that pin the spread**

In `ts/packages/paigasus-next-config/tests/boundaries.test.ts`, change line 8 to:

```ts
import { BOUNDARY_SCOPES, boundaryRules, sourceRules } from '../src/eslint.mjs';
```

After line 110 (the last `an app middleware must not …` row), insert these DENIED rows. They prove that the boundary groups still match the new, extensionless form that real files now write:

```ts
  // SMA-511: package sources are EXTENSIONLESS now (spec § 7.2). The `.js` rows above prove the
  // groups match the old spelling; these prove they match what real files write today.
  ['auth/client must not reach runtime.ts, extensionless', 'packages/paigasus-auth/src/client.ts', "import { x } from './runtime';"],
  ['auth/client must not reach core, extensionless', 'packages/paigasus-auth/src/client.ts', "import { x } from './core/single-flight';"],
  ['auth/middleware must not reach the http composition root, extensionless', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './http/routes';"],
  ['auth/middleware must not reach the session store port, extensionless', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './ports/session-store';"],
  ['auth/middleware must not reach config.ts, extensionless', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './config';"],
  ['auth/server must not reach the client-only surface, extensionless', 'packages/paigasus-auth/src/server.ts', "import { x } from './client';"],
```

Inside `describe('the workspace eslint config actually applies the preset', …)` (lines 270–294), after the last `it(…)`, add:

```ts
  // SMA-511 spec § 7.2. The source rule is a SEPARATE export, so the boundary-entry check above does
  // not see it. Deleting only the spread from ts/eslint.config.js would leave every other test green.
  it('carries every sourceRules entry in its EXPORTED array', async () => {
    const shipped = (await import('../../../eslint.config.js')).default as Array<{ files?: string[]; ignores?: string[] }>;
    for (const entry of sourceRules) {
      expect(shipped, `ts/eslint.config.js dropped the ${entry.name} block`).toContainEqual(expect.objectContaining({ files: entry.files, ignores: entry.ignores }));
    }
  });

  // Lints through the REAL config, so a global `ignores` entry that silences packages/*/src fails
  // here. The path is a REAL, tracked file: the shipped config lints every .ts path with
  // projectService, and a path no tsconfig includes gives one fatal parse error and runs no rule.
  // lintText uses the source given here, not the file on disk.
  it('lints a .js relative specifier in package src through the REAL config', async () => {
    const eslint = new ESLint({ cwd: TS_ROOT });
    const [result] = await eslint.lintText("import { SESSION_VIEW_KEYS } from './core/session.js';\nexport const keys = SESSION_VIEW_KEYS;\n", {
      filePath: 'packages/paigasus-auth/src/session-view.ts',
      warnIgnored: false,
    });
    const messages = result?.messages ?? [];
    expect(messages.filter((m) => m.fatal === true)).toEqual([]);
    expect(messages.filter((m) => m.ruleId === 'paigasus/no-js-relative-specifier')).toHaveLength(1);
  }, 120_000);
```

Run:

```bash
pnpm -C ts/packages/paigasus-next-config exec vitest run tests/boundaries.test.ts
```

Expected: FAIL on the two new `it` cases only (`ts/eslint.config.js dropped the paigasus/source/no-js-relative-specifier block`, and 0 rule messages instead of 1). The six new DENIED rows pass already.

- [ ] **Step 12: Spread `sourceRules` in the workspace config**

In `ts/eslint.config.js`, change line 9 to:

```js
import { boundaryRules, sourceRules } from '@paigasus/next-config/eslint';
```

Replace lines 65–71 (the boundary comment and `...boundaryRules,`) with:

```js
  // Package dependency direction (Frontend Architecture Scoping § 6, SMA-502). The rules live in
  // @paigasus/next-config/eslint so they ship with the package that owns the boundary, and are
  // unit-tested there against synthetic paths — including the app-shell and auth scopes, which do
  // not exist on disk yet. paigasus-next-config-ts:test asserts this spread is still here, and
  // that package's moon.yml lists /ts/eslint.config.js among its test inputs so the assertion is
  // reachable on the PR that removes it.
  ...boundaryRules,
  // Source hygiene (SMA-511 spec § 7.2): no `.js` relative specifier in packages/*/src, because
  // Turbopack does not resolve it to a `.ts` file. A SEPARATE export, never a boundaryRules block —
  // see its doc comment. paigasus-next-config-ts:test asserts this spread too.
  ...sourceRules,
```

- [ ] **Step 13: Run the boundary tests and the whole lint**

```bash
pnpm -C ts/packages/paigasus-next-config exec vitest run tests/boundaries.test.ts
pnpm -C ts exec prettier --write eslint.config.js packages/paigasus-next-config/tests/boundaries.test.ts packages/paigasus-sdk/tests/server-guard.test.ts packages/paigasus-sdk/src/errors.ts packages/paigasus-sdk/src/errors/types.ts packages/paigasus-discovery/tests/structure/exports.test.ts \
  packages/paigasus-next-config/src/eslint.mjs packages/paigasus-auth/tests/structure/import-graph.test.ts packages/paigasus-auth/tests/support/import-graph.ts
moon run paigasus-next-config-ts:test paigasus-auth-ts:test ts:lint ts:fmt --force
```

Expected: all pass. `ts:lint` reports no `paigasus/no-js-relative-specifier` message. If it reports one, the file is under `packages/*/src` and the Step 8 script missed it (for example a new file); remove the extension there.

- [ ] **Step 14: Run the plain-Node harnesses (Docker required)**

These are the two consumers of the loaders. Both need a running Docker daemon.

```bash
docker info > /dev/null && echo "docker ok"
moon run paigasus-auth-ts:test-e2e paigasus-discovery-ts:test-e2e
```

Expected: `docker ok`, then both tasks pass. The auth task starts `tests/e2e/fixture-server.ts` through both loaders and drives the Keycloak login, so a failed extensionless resolution in `src/server.ts`'s graph shows as a web-server start failure. The discovery task forks `tests/containers/support/worker.ts`, whose graph (`src/adapters/redis-cache.ts`, `src/core/record.ts`, `src/core/single-flight.ts`) is now extensionless.

- [ ] **Step 15: Commit**

```bash
git add ts/eslint.config.js ts/packages/paigasus-next-config/tests/boundaries.test.ts \
  ts/packages/paigasus-auth/src ts/packages/paigasus-sdk/src ts/packages/paigasus-discovery/src ts/packages/paigasus-proto/src \
  ts/packages/paigasus-sdk/tests/server-guard.test.ts ts/packages/paigasus-discovery/tests/structure/exports.test.ts \
  ts/packages/paigasus-next-config/src/eslint.mjs ts/packages/paigasus-auth/tests/structure/import-graph.test.ts \
  ts/packages/paigasus-auth/tests/support/import-graph.ts
git commit -F - <<'EOF'
fix(ts): use extensionless relative imports in package sources (SMA-511)

Turbopack in Next 16.3.4 does not resolve ./x.js to ./x.ts, so @paigasus/auth, @paigasus/sdk,
@paigasus/discovery and the hand-written part of @paigasus/proto could not be built by a Next app.
All 162 relative specifiers in their src/ lose the .js suffix. Test files keep it.

ts/eslint.config.js now spreads sourceRules, so paigasus/no-js-relative-specifier holds the rule
for packages/*/src, and a boundaries test pins the spread. The server-guard and client-graph tests
now expect the extensionless form. The auth-client lint message and five comments no longer tell a
developer to write .js.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 4: Extensionless generated protobuf-es code

Spec § 7.2 "Generated proto code", § 13 row 3 (measured: protobuf-es `import_extension` accepts `none` — the default when the option is omitted — `.js` and `.ts`; omitting it gives extensionless imports; `buf generate` works here). ESLint ignores `**/generated/**` (`ts/eslint.config.js:14`), so a proto test holds this tree instead of the rule.

**Files:**
- Create: `ts/packages/paigasus-proto/src/generated-specifiers.test.ts`
- Modify: `ts/packages/paigasus-proto/package.json` (devDependencies: add `@types/node`)
- Modify: `contracts/buf.gen.yaml:51-56` (the TypeScript plugin block)
- Modify: `contracts/buf.gen.googleapis.yaml:28-32` (its only plugin block)
- Regenerate (by `moon run contracts:generate --force`, never by hand), 8 files under `ts/packages/paigasus-proto/src/generated/`: `google/rpc/error_details_pb.ts`, `paigasus/common/v1/{actor,audit,auditable_example,error,service_info}_pb.ts`, `paigasus/gateway/v1/health_pb.ts`, `paigasus/iam/v1/iam_pb.ts`. All 8 change their `@generated … with parameter` header line. Three of them (`audit_pb.ts`, `auditable_example_pb.ts`, `iam_pb.ts`) also lose `.js` on 6 import lines.
- Modify: `ts/pnpm-lock.yaml` (by `pnpm install`)

**Interfaces:**
- Consumes: the Task 3 loaders (a plain-Node process that reaches proto's generated code now needs the extensionless retry).
- Produces: `@paigasus/proto`'s whole `src/` is free of `.js` relative specifiers, so Turbopack can compile `@paigasus/sdk` → `@paigasus/proto`.

- [ ] **Step 1: Give the package Node types**

`@paigasus/proto` has no `@types/node` today, and the new test reads files with `node:fs`. In `ts/packages/paigasus-proto/package.json`, change the `devDependencies` block to:

```json
  "devDependencies": {
    "@types/node": "catalog:",
    "typescript": "catalog:"
  }
```

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts install --no-frozen-lockfile
```

Expected: the lockfile gains one `'@types/node'` entry under `packages/paigasus-proto`, and nothing else changes.

- [ ] **Step 2: Write the failing test**

Create `ts/packages/paigasus-proto/src/generated-specifiers.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-511 spec § 7.2. Turbopack in Next 16.3.4 does not resolve './x.js' to './x.ts', and every Next
// zone compiles this generated code through @paigasus/sdk. ts/eslint.config.js ignores
// **/generated/**, so `paigasus/no-js-relative-specifier` never sees this tree: this test is the
// control. contracts/buf.gen.yaml and contracts/buf.gen.googleapis.yaml OMIT protobuf-es's
// `import_extension` option for that reason (the default, `none`, writes extensionless imports).
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const GENERATED = fileURLToPath(new URL('./generated', import.meta.url));

function generatedFiles(): string[] {
  return readdirSync(GENERATED, { recursive: true, encoding: 'utf8' })
    .filter((p) => p.endsWith('.ts'))
    .map((p) => join(GENERATED, p));
}

/** Every RELATIVE specifier of an import or export declaration, read with the TypeScript parser. */
function relativeSpecifiers(file: string): string[] {
  const source = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true);
  const found: string[] = [];
  for (const statement of source.statements) {
    if ((ts.isImportDeclaration(statement) || ts.isExportDeclaration(statement)) && statement.moduleSpecifier !== undefined && ts.isStringLiteral(statement.moduleSpecifier)) {
      if (statement.moduleSpecifier.text.startsWith('.')) found.push(statement.moduleSpecifier.text);
    }
  }
  return found;
}

describe('generated protobuf-es output (SMA-511 spec § 7.2)', () => {
  const files = generatedFiles();

  it('finds the generated tree', () => {
    expect(files.length).toBeGreaterThanOrEqual(8);
  });

  // Without this, a tree whose relative imports all vanished would pass the check below vacuously.
  it('carries relative imports at all', () => {
    expect(files.flatMap(relativeSpecifiers).length).toBeGreaterThan(0);
  });

  it.each(files)('%s has no relative specifier that ends in .js', (file) => {
    expect(relativeSpecifiers(file).filter((specifier) => specifier.endsWith('.js'))).toEqual([]);
  });
});
```

- [ ] **Step 3: Run the test and see it fail**

```bash
pnpm -C ts/packages/paigasus-proto exec vitest run src/generated-specifiers.test.ts
```

Expected: FAIL on 3 of the per-file cases: `paigasus/common/v1/audit_pb.ts` (`["./actor_pb.js","./actor_pb.js"]`), `paigasus/common/v1/auditable_example_pb.ts` (`["./audit_pb.js","./audit_pb.js"]`) and `paigasus/iam/v1/iam_pb.ts` (`["../../common/v1/audit_pb.js","../../common/v1/audit_pb.js"]`). The other cases pass.

- [ ] **Step 4: Remove the option from both templates**

In `contracts/buf.gen.yaml`, replace lines 51–56:

```yaml
  # ─── TypeScript: protobuf-es v2, pinned ───────────────────────────────────
  - remote: buf.build/bufbuild/es:v2.13.0
    out: ../ts/packages/paigasus-proto/src/generated
    opt:
      - target=ts
      - import_extension=.js
```

with:

```yaml
  # ─── TypeScript: protobuf-es v2, pinned ───────────────────────────────────
  # `import_extension` is deliberately OMITTED (SMA-511 spec § 7.2). Its default, `none`, writes
  # EXTENSIONLESS relative imports. Turbopack (Next 16.3.4) does not resolve './x.js' to './x.ts',
  # and every Next zone compiles this output through @paigasus/sdk. Held by
  # ts/packages/paigasus-proto/src/generated-specifiers.test.ts. Keep buf.gen.googleapis.yaml's es
  # options identical: both templates write into the same tree.
  - remote: buf.build/bufbuild/es:v2.13.0
    out: ../ts/packages/paigasus-proto/src/generated
    opt:
      - target=ts
```

In `contracts/buf.gen.googleapis.yaml`, replace lines 28–32:

```yaml
  - remote: buf.build/bufbuild/es:v2.13.0
    out: ../ts/packages/paigasus-proto/src/generated
    opt:
      - target=ts
      - import_extension=.js
```

with:

```yaml
  # `import_extension` is OMITTED here too, for the reason buf.gen.yaml gives (SMA-511).
  - remote: buf.build/bufbuild/es:v2.13.0
    out: ../ts/packages/paigasus-proto/src/generated
    opt:
      - target=ts
```

- [ ] **Step 5: Regenerate**

Always through Moon: a bare `buf generate` runs only the first template and deletes `google/rpc/error_details_pb.ts` (`contracts/moon.yml`, RECOVERY note).

```bash
moon run contracts:generate --force
git status --porcelain -- contracts rs py ts
```

Expected: `git status` lists `contracts/buf.gen.yaml`, `contracts/buf.gen.googleapis.yaml`, `ts/packages/paigasus-proto/package.json`, `ts/pnpm-lock.yaml`, the new test, and the 8 generated TS files. It lists NO file under `rs/` or `py/`: the Rust and Python generators did not change. Check one header:

```bash
sed -n '3p' ts/packages/paigasus-proto/src/generated/paigasus/common/v1/audit_pb.ts
```

Expected: `// @generated by protoc-gen-es v2.13.0 with parameter "target=ts"`.

- [ ] **Step 6: Run the test and see it pass**

```bash
pnpm -C ts/packages/paigasus-proto exec vitest run src/generated-specifiers.test.ts
```

Expected: PASS, 10 cases (2 + 8 files).

- [ ] **Step 7: Run the consumers**

```bash
pnpm -C ts exec prettier --write packages/paigasus-proto/src/generated-specifiers.test.ts packages/paigasus-proto/package.json
moon run paigasus-proto-ts:test paigasus-proto-ts:typecheck paigasus-sdk-ts:test paigasus-sdk-ts:typecheck \
  paigasus-discovery-ts:test paigasus-app-shell-ts:test contracts:lint contracts:fmt
moon run ts:lint ts:fmt --force
```

Expected: every target passes. The drift check runs after the commit, in Step 9.

- [ ] **Step 8: Commit**

```bash
git add contracts/buf.gen.yaml contracts/buf.gen.googleapis.yaml ts/packages/paigasus-proto/package.json ts/pnpm-lock.yaml \
  ts/packages/paigasus-proto/src/generated-specifiers.test.ts ts/packages/paigasus-proto/src/generated
git commit -F - <<'EOF'
fix(contracts): generate extensionless protobuf-es imports (SMA-511)

Turbopack in Next 16.3.4 does not resolve ./x.js to ./x.ts, and every Next zone compiles the
generated TypeScript through @paigasus/sdk. Both buf templates now omit import_extension, whose
default writes extensionless imports, and the generated tree is regenerated. ESLint ignores
generated code, so a proto test now asserts that no generated file holds a .js relative specifier.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

- [ ] **Step 9: Prove the committed tree has no drift**

```bash
moon run contracts:generate --force
git status --porcelain -- contracts rs py ts/packages/paigasus-proto; echo "status printed above; expect nothing"
```

Expected: no output from `git status`. This is the same check as the codegen-drift step in `.github/workflows/ci.yml`.

### Task 5: `@paigasus/auth` under a Next `basePath`

Spec § 7.1. Measured facts this task relies on (spec § 13 row 1, Next 16.3.4):

- In the proxy, `req.nextUrl.pathname` has the basePath REMOVED (`/auth/login`), and `req.nextUrl.basePath` is `/iam`. A `req.nextUrl.clone()` with `pathname = '/auth/login'` redirects to `/iam/auth/login` exactly once.
- In a page, `redirect('/auth/login?…')` gives `Location: /iam/auth/login?…`; `redirect('/iam/auth/login')` gives `/iam/iam/auth/login`.
- In a route handler, `req.url` is `http://0.0.0.0:<port>/auth/…`: no basePath, and the bind address as the host. A raw `Response` `Location` from a route handler passes through unchanged.

The core route table stays keyed by the FULL path. The public origin is not on `AuthRuntime` today (only the derived `redirectUri` and `postLogoutRedirectUri` are), so this task ADDS `publicOrigin` to `AuthRuntime`.

The plain-Node e2e fixture (`tests/e2e/fixture-server.ts`) does not use Next. It builds each request on `FIXTURE_SERVER_ORIGIN` (equal to `PAIGASUS_PUBLIC_ORIGIN`) with the FULL path `/e2e/auth/…` and passes it to `createAuthRouteHandler`. The new URL rebuild keeps a path that is already one of the zone's auth routes, so the fixture keeps working without a change. Step 16 runs the e2e task to prove it.

**Files:**
- Modify: `ts/packages/paigasus-auth/src/runtime.ts:61-79` (interface), `:144-160` (return value)
- Modify: `ts/packages/paigasus-auth/src/middleware.ts:25-92`
- Modify: `ts/packages/paigasus-auth/src/next/get-session.ts:93-106`
- Modify: `ts/packages/paigasus-auth/src/server.ts:18-81`
- Modify: `ts/packages/paigasus-auth/src/http/routes.ts:120-122` (`handleLogin` returnTo, and the two line citations in the I3 comment above it) and `:204-207` (`handleCallback` currentUrl)
- Modify: `ts/packages/paigasus-auth/README.md:105-157`
- Modify tests: `tests/runtime.test.ts:30-34`, `tests/middleware.test.ts:22-90,176-188`, `tests/next/get-session.test.ts:66-84,164-178`, `tests/server.test.ts:28-46` + new describe, `tests/http/login.test.ts:35-58` + new cases, `tests/http/callback.test.ts:43-105` + new describe, `tests/http/logout.test.ts:130-146`
- Create: `ts/packages/paigasus-auth/tests/http/route-handler.test.ts`

(All paths below are relative to `ts/packages/paigasus-auth/`. Source imports are extensionless after Task 3.)

**Interfaces:**
- Consumes: `AUTH_ROUTE_SUFFIXES` (`src/http/route-table.ts`), `validateReturnTo` (`src/core/return-to.ts`).
- Produces:
  - `AuthRuntime.publicOrigin: string` — `PAIGASUS_PUBLIC_ORIGIN` as parsed.
  - `authRoutePaths(): readonly string[]` — NO argument; returns `['/auth/login', '/auth/callback', '/auth/logout', '/auth/logout/callback']` (basePath-relative).
  - `createAuthMiddleware({ publicPaths, loginPath })` — both basePath-relative; `returnTo` = `nextUrl.basePath + pathname + search`.
  - `requireSession(runtime, { returnTo? })` — calls `redirect('/auth/login?returnTo=<encoded full path>')`.
  - `createAuthRouteHandler(runtime)` — accepts a basePath-stripped bind-address URL (Next) or a full-path URL (plain Node).
  - `handleCallback` sends `redirect_uri` = `runtime.redirectUri`; `handleLogin` replaces a `returnTo` under `${basePath}/auth/` with `${basePath}/`.

- [ ] **Step 1: Write the failing runtime test**

In `tests/runtime.test.ts`, replace lines 30–34:

```ts
  it('derives the redirect URI from the origin and the zone base path', async () => {
    const rt = await createAuthRuntime(BASE);
    expect(rt.redirectUri).toBe('https://app.example.com/iam/auth/callback');
    expect(rt.postLogoutRedirectUri).toBe('https://app.example.com/iam/');
  });
```

with:

```ts
  it('derives the redirect URI from the origin and the zone base path', async () => {
    const rt = await createAuthRuntime(BASE);
    expect(rt.redirectUri).toBe('https://app.example.com/iam/auth/callback');
    expect(rt.postLogoutRedirectUri).toBe('https://app.example.com/iam/');
  });

  // SMA-511 spec § 7.1: createAuthRouteHandler rebuilds a route handler's request URL on this value.
  it('carries PAIGASUS_PUBLIC_ORIGIN as publicOrigin', async () => {
    const rt = await createAuthRuntime(BASE);
    expect(rt.publicOrigin).toBe('https://app.example.com');
  });
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-auth exec vitest run tests/runtime.test.ts
```

Expected: FAIL — `expected undefined to be 'https://app.example.com'`.

- [ ] **Step 2: Add `publicOrigin` to the runtime**

In `src/runtime.ts`, in `interface AuthRuntime`, after `oidc: OidcClient;` (line 65) insert:

```ts
  /**
   * `PAIGASUS_PUBLIC_ORIGIN` as parsed: an https origin with no trailing slash (config.ts
   * `httpsUrl`). `createAuthRouteHandler` (server.ts) rebuilds a route handler's request URL on it,
   * because Next gives a route handler the server's BIND address instead (SMA-511 spec § 7.1).
   */
  publicOrigin: string;
```

In the object returned by `createAuthRuntime` (line 154 after the insert above, 148 before it; after `oidc,`), insert:

```ts
    publicOrigin: cfg.PAIGASUS_PUBLIC_ORIGIN,
```

Every hand-built `AuthRuntime` in the tests now misses a required field. Add the line to each:

- `tests/next/get-session.test.ts`, in `baseRuntime` after `oidc: unusedOidc(),` (line 71): `publicOrigin: 'https://app.example.com',`
- `tests/server.test.ts`, in `runtime()` after `oidc: {} as AuthRuntime['oidc'],` (line 33): `publicOrigin: 'https://app.example.com',`
- `tests/http/login.test.ts`, in `beforeEach` after the `oidc: createOidcClient({…}),` entry (line 46): `publicOrigin: 'https://rp.example.com',`
- `tests/http/callback.test.ts`, in `beforeEach` after the `oidc: countingOidc(…),` entry (line 92): `publicOrigin: 'https://rp.example.com',`
- `tests/http/logout.test.ts`, in `baseRuntime` after `oidc,` (line 134): `publicOrigin: 'https://rp.example.com',`

Run:

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/runtime.test.ts
moon run paigasus-auth-ts:typecheck
```

Expected: PASS, and typecheck passes (no `Property 'publicOrigin' is missing` error).

- [ ] **Step 3: Write the failing middleware tests**

In `tests/middleware.test.ts`, replace lines 22–90 (from `const OPTIONS = {` to the end of `describe('authRoutePaths (I5)', …)`) with:

```ts
// basePath-RELATIVE since SMA-511 (spec § 7.1): the middleware compares `req.nextUrl.pathname`,
// which Next gives WITHOUT the zone's basePath.
const OPTIONS = { publicPaths: authRoutePaths(), loginPath: '/auth/login' };

/** What Next hands a zone's proxy: a NextRequest whose NextURL knows the compiled basePath. */
function request(path: string, cookie?: string, basePath = '/iam'): NextRequest {
  const headers = cookie !== undefined ? { cookie } : {};
  return new NextRequest(`https://app.example.com${path}`, { headers, nextConfig: { basePath } });
}

describe('createAuthMiddleware', () => {
  it('redirects a guarded path with no session cookie to the login path, with a returnTo that keeps the basePath', () => {
    const res = createAuthMiddleware(OPTIONS)(request('/iam/dashboard?tab=1'));

    expect(res.status).toBe(307); // NextResponse.redirect's default status
    const location = new URL(res.headers.get('location') ?? '');
    // Exactly one `/iam`: a basePath-relative loginPath on a clone of nextUrl (spec § 13 row 1).
    expect(location.pathname).toBe('/iam/auth/login');
    expect(location.searchParams.get('returnTo')).toBe('/iam/dashboard?tab=1');
    expect([...location.searchParams.keys()]).toEqual(['returnTo']);
  });

  it('passes through a guarded path carrying ANY cookie value, including a forged one', () => {
    const res = createAuthMiddleware(OPTIONS)(request('/iam/dashboard', `${SESSION_COOKIE}=totally-forged-not-a-real-sid`));

    expect(res.headers.get('location')).toBeNull();
    // NextResponse.next() sets this marker header — proof this is a "continue" response, not a
    // freshly constructed 200 that merely looks like one.
    expect(res.headers.get('x-middleware-next')).toBe('1');
  });

  it.each(AUTH_ROUTE_SUFFIXES)('passes through the auth route /iam%s with no cookie at all', (suffix) => {
    const res = createAuthMiddleware(OPTIONS)(request(`/iam${suffix}`));

    expect(res.headers.get('location')).toBeNull();
    expect(res.headers.get('x-middleware-next')).toBe('1');
  });

  // The SMA-511 failure, kept as a test: a basePath-PREFIXED public path never matches, because
  // Next never hands the proxy a prefixed pathname — so the zone's own login route redirected to itself.
  it('does not match a basePath-PREFIXED public path', () => {
    const res = createAuthMiddleware({ publicPaths: ['/iam/auth/login'], loginPath: '/auth/login' })(request('/iam/auth/login'));

    expect(res.status).toBe(307);
  });

  it('works for a root-mounted zone, where basePath is the empty string', () => {
    const res = createAuthMiddleware(OPTIONS)(request('/dashboard', undefined, ''));

    const location = new URL(res.headers.get('location') ?? '');
    expect(location.pathname).toBe('/auth/login');
    expect(location.searchParams.get('returnTo')).toBe('/dashboard');
  });
});

// I5 (final fix wave). `publicPaths` used to be the caller's own hand-copied list, bound to
// `http/routes.ts`'s route table by nothing — omit one path there (the callback path is the easy
// one to miss) and a signed-out visitor loops between `/auth/login` and `/auth/callback` forever,
// with no error anywhere. `authRoutePaths()` replaces the hand copy with a derivation; this
// cross-checks it against the REAL route table in `http/routes.ts`.
describe('authRoutePaths (I5)', () => {
  it('returns the four basePath-RELATIVE paths createAuthRoutes dispatches on', () => {
    expect(authRoutePaths()).toEqual(['/auth/login', '/auth/callback', '/auth/logout', '/auth/logout/callback']);
  });

  // Each path, under the zone's basePath, must be recognised by createAuthRoutes (a 405 on the
  // wrong method, never a 404), and a lookalike path must still 404. Only `runtime.basePath` is
  // read before routes.ts's method dispatch, so a partial fixture is enough here.
  it('every path, under the basePath, is recognised by createAuthRoutes, and a lookalike is not', async () => {
    const fakeRuntime = { basePath: '/iam' } as unknown as AuthRuntime;
    const routes = createAuthRoutes(fakeRuntime);

    for (const path of authRoutePaths()) {
      const wrongMethod = path.endsWith('/logout') ? 'GET' : 'POST';
      const res = await routes.handle(new Request(`https://rp.example.com/iam${path}`, { method: wrongMethod }));
      expect(res.status).toBe(405);
    }

    const res = await routes.handle(new Request('https://rp.example.com/iam/auth/not-a-real-route'));
    expect(res.status).toBe(404);
  });
});
```

In the same file, replace the first and third `it` of `describe('the shared route table (SMA-626 § 3)', …)` (original lines 177–188; they move after the edit above, so match the text):

```ts
  it('authRoutePaths is exactly the table mapped over basePath', () => {
    expect(authRoutePaths({ basePath: '/iam' })).toEqual(AUTH_ROUTE_SUFFIXES.map((s) => `/iam${s}`));
  });
```

and

```ts
  it('works for a root-mounted zone, where basePath is the empty string', () => {
    expect(authRoutePaths({ basePath: '' })).toEqual(['/auth/login', '/auth/callback', '/auth/logout', '/auth/logout/callback']);
  });
```

with one test:

```ts
  it('authRoutePaths is exactly the table, basePath-relative', () => {
    expect(authRoutePaths()).toEqual([...AUTH_ROUTE_SUFFIXES]);
  });
```

Run:

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/middleware.test.ts
```

Expected: FAIL. The file fails at collection with `TypeError: Cannot read properties of undefined (reading 'basePath')`, because the current `authRoutePaths` needs an argument.

- [ ] **Step 4: Rewrite the middleware**

In `src/middleware.ts`, replace lines 25–92 (from `export interface AuthMiddlewareOptions {` to the end of the file) with:

```ts
export interface AuthMiddlewareOptions {
  /**
   * BasePath-RELATIVE pathnames that never require a session cookie, matched exactly against
   * `req.nextUrl.pathname`. Next removes the zone's basePath from that value before the proxy sees
   * it (measured, SMA-511 spec § 13 row 1: under `basePath: '/iam'` a request for `/iam/auth/login`
   * has `nextUrl.pathname === '/auth/login'`). So list `/auth/login`, never `/iam/auth/login`: a
   * prefixed entry never matches, and the login route then redirects to itself.
   *
   * MUST include every route `createAuthRoutes` serves — `/auth/login`, `/auth/callback`,
   * `/auth/logout`, `/auth/logout/callback` — or a signed-out visitor redirected to `loginPath` is
   * redirected right back to itself: `loginPath` has no cookie either, and this file has no way to
   * recognise it is already the login route without being told so explicitly.
   *
   * Use `authRoutePaths()` below to build this list instead of hand-copying it — a hand-copied list
   * drifts from `createAuthRoutes`'s own route table with no error anywhere: an app that forgets the
   * callback path here still builds and deploys, and the failure is a silent infinite redirect loop
   * (I5, final fix wave) rather than a build-time or lint-time signal.
   */
  publicPaths: readonly string[];
  /**
   * Where a signed-out visitor is sent, basePath-RELATIVE: usually `/auth/login`. The redirect
   * keeps the zone's basePath, so the browser lands on `/iam/auth/login`.
   */
  loginPath: string;
}

/**
 * The four basePath-RELATIVE pathnames `createAuthRoutes` dispatches on, for use as
 * `AuthMiddlewareOptions.publicPaths` — I5, final fix wave.
 *
 * THE FAILURE THIS CLOSES. `publicPaths` used to be the caller's own hand-copied list, with nothing
 * binding it to `http/routes.ts`'s actual route table. Omit one path there — the callback path is
 * the easy one to miss — and `/auth/login` clears the session cookie, the IdP's redirect back to
 * `/auth/callback` arrives with no cookie, middleware (seeing a non-public path with no cookie)
 * bounces it BACK to `/auth/login`, which clears the cookie again and redirects again: the user
 * loops forever, with no error anywhere.
 *
 * BASEPATH-RELATIVE SINCE SMA-511 (spec § 7.1). The middleware compares `req.nextUrl.pathname`,
 * which Next gives without the basePath, so this list carries no basePath and takes no argument.
 * The core route table (`http/routes.ts`) is still keyed by the FULL path; `createAuthRouteHandler`
 * puts the basePath back for a route handler.
 *
 * Derived from `route-table.ts`'s shared `AUTH_ROUTE_SUFFIXES` tuple (SMA-626 § 3) — the same table
 * `http/routes.ts` builds its `Record<AuthRouteSuffix, …>` dispatch from. That module imports
 * nothing, so importing it here adds no new edge into this entry point's module graph, and AC 4's
 * import-graph guarantee is unchanged (see the file header and `tests/middleware.test.ts`).
 */
export function authRoutePaths(): readonly string[] {
  return [...AUTH_ROUTE_SUFFIXES];
}

/** Build this zone's middleware (Next 16: the zone's `proxy.ts`). */
export function createAuthMiddleware(options: AuthMiddlewareOptions): (req: NextRequest) => NextResponse {
  const publicPaths = new Set(options.publicPaths);

  return function authMiddleware(req: NextRequest): NextResponse {
    // basePath-RELATIVE: Next removes the zone's basePath from `nextUrl.pathname` (see publicPaths).
    const { pathname, basePath, search } = req.nextUrl;

    if (publicPaths.has(pathname)) {
      return NextResponse.next();
    }

    // PRESENCE ONLY (AC 4) — see the file header. Never call anything that would tell this cookie
    // apart from a forged one.
    if (req.cookies.has(SESSION_COOKIE)) {
      return NextResponse.next();
    }

    // A clone of nextUrl keeps Next's basePath configuration, so a basePath-relative loginPath
    // redirects to `/iam/auth/login` exactly once (measured, spec § 13 row 1). `returnTo` is a FULL
    // path: the callback sends it back as a raw Location from a route handler, and Next does not
    // prefix that.
    const loginUrl = req.nextUrl.clone();
    loginUrl.pathname = options.loginPath;
    loginUrl.search = '';
    loginUrl.hash = '';
    loginUrl.searchParams.set('returnTo', `${basePath}${pathname}${search}`);
    return NextResponse.redirect(loginUrl);
  };
}
```

Run:

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/middleware.test.ts
```

Expected: PASS. The import-graph cases in the same file stay green (the middleware imports did not change).

- [ ] **Step 5: Write the failing `requireSession` tests**

In `tests/next/get-session.test.ts`, replace lines 166–179 (165–178 before Step 2's insert). These are the first two `it` of `describe('requireSession', …)`: from `it("calls next/navigation's redirect() to the login path…` to the end of `it('falls back to the zone root when returnTo is not a valid same-origin path', …)`. Line 165, `describe('requireSession', () => {`, stays. Replace the two `it` with:

```ts
  it("calls next/navigation's redirect() with the basePath-RELATIVE login path and a returnTo that keeps the basePath", async () => {
    cookiesMock.mockResolvedValue(cookieJar());
    const runtime = baseRuntime(new MemorySessionStore());

    await expect(requireSession(runtime, { returnTo: '/iam/dashboard' })).rejects.toThrow('NEXT_REDIRECT:/auth/login?returnTo=%2Fiam%2Fdashboard');
    expect(redirectMock).toHaveBeenCalledWith('/auth/login?returnTo=%2Fiam%2Fdashboard');
  });

  // SMA-511 spec § 13 row 1: Next's redirect() adds the basePath itself, with no duplicate check, so
  // `redirect('/iam/auth/login')` lands on `/iam/iam/auth/login`.
  it('never passes the basePath to redirect()', async () => {
    cookiesMock.mockResolvedValue(cookieJar());

    await expect(requireSession(baseRuntime(new MemorySessionStore()))).rejects.toThrow();

    const [target] = redirectMock.mock.lastCall ?? [];
    expect(target).toBe('/auth/login?returnTo=%2Fiam%2F');
  });

  it('falls back to the zone root when returnTo is not a valid same-origin path', async () => {
    cookiesMock.mockResolvedValue(cookieJar());
    const runtime = baseRuntime(new MemorySessionStore());

    await expect(requireSession(runtime, { returnTo: 'https://evil.example.com' })).rejects.toThrow('NEXT_REDIRECT:/auth/login?returnTo=%2Fiam%2F');
  });

  it('falls back to "/" on a root-mounted zone', async () => {
    cookiesMock.mockResolvedValue(cookieJar());
    const runtime = { ...baseRuntime(new MemorySessionStore()), basePath: '' };

    await expect(requireSession(runtime)).rejects.toThrow('NEXT_REDIRECT:/auth/login?returnTo=%2F');
  });
```

Run:

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/next/get-session.test.ts
```

Expected: FAIL on the first three new cases — the current code redirects to `/iam/auth/login?…`.

- [ ] **Step 6: Change `requireSession`**

In `src/next/get-session.ts`, replace lines 93–106 (the doc comment and body of `requireSession`) with:

```ts
/**
 * Read the current session, redirecting to this zone's login path when there is none. Safe to
 * call from a server component or a Server Action: `redirect()` is the one recovery a server
 * component may perform, and it is what turns a stale `__Host-pgs_sid` cookie into a working
 * "sign in again" prompt instead of a permanently blank page.
 *
 * THE REDIRECT TARGET IS BASEPATH-RELATIVE (SMA-511 spec § 7.1). Next's redirect() adds the
 * basePath itself, with no duplicate check: `redirect('/auth/login?…')` gives `Location:
 * /iam/auth/login?…`, and `redirect('/iam/auth/login')` gives `/iam/iam/auth/login` (measured, spec
 * § 13 row 1). `returnTo` keeps the basePath, because the callback sends it back as a raw Location
 * header from a route handler, which Next passes through unchanged. Do not call this from a route
 * handler; a route handler returns its own redirect Response.
 */
export async function requireSession(runtime: AuthRuntime, options: RequireSessionOptions = {}): Promise<ResolvedSession> {
  const session = await getSession(runtime);
  if (session !== null) return session;

  const returnTo = validateReturnTo(options.returnTo, `${runtime.basePath}/`);
  redirect(`/auth/login?returnTo=${encodeURIComponent(returnTo)}`);
}
```

Run:

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/next/get-session.test.ts
```

Expected: PASS.

- [ ] **Step 7: Write the failing route-handler tests (routes mocked)**

In `tests/server.test.ts`, replace the `runtime()` helper (lines 28–47 after Step 2's `publicOrigin` insert; 28–46 before it) with a version that takes overrides:

```ts
function runtime(overrides: Partial<AuthRuntime> = {}): AuthRuntime {
  return {
    store: {} as AuthRuntime['store'],
    resolver: {} as AuthRuntime['resolver'],
    logger: { event: () => undefined },
    oidc: {} as AuthRuntime['oidc'],
    publicOrigin: 'https://app.example.com',
    redirectUri: 'https://app.example.com/iam/auth/callback',
    postLogoutRedirectUri: 'https://app.example.com/iam/',
    cookieDomainless: true,
    skewMs: 30_000,
    lockTtlMs: 10_000,
    lockWaitMs: 3_000,
    ttlMs: 28_800_000,
    absoluteTtlMs: 86_400_000,
    zone: 'iam',
    basePath: '/iam',
    scopes: 'openid',
    ...overrides,
  };
}
```

At the end of the file, add:

```ts
// SMA-511 spec § 7.1. Next removes the basePath from a route handler's `req.url` and puts the
// server's bind address in it (measured: `http://0.0.0.0:<port>/auth/callback?…`). The core route
// table is keyed by the full path on the public origin, so the handler rebuilds the URL first.
describe('createAuthRouteHandler rebuilds the request URL (SMA-511 spec § 7.1)', () => {
  async function requestSeenBy(rt: AuthRuntime, input: string, init?: RequestInit): Promise<Request> {
    handleMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
    await createAuthRouteHandler(rt)(new Request(input, init));
    const seen = handleMock.mock.lastCall?.[0];
    if (seen === undefined) throw new Error('createAuthRoutes().handle was not called');
    return seen;
  }

  it('adds the basePath back and uses PAIGASUS_PUBLIC_ORIGIN for what Next hands a route handler', async () => {
    const seen = await requestSeenBy(runtime(), 'http://0.0.0.0:3000/auth/callback?code=c&state=s');
    expect(seen.url).toBe('https://app.example.com/iam/auth/callback?code=c&state=s');
  });

  it('keeps a full path, as a plain node:http server passes it (tests/e2e/fixture-server.ts)', async () => {
    const seen = await requestSeenBy(runtime(), 'http://127.0.0.1:4000/iam/auth/login?returnTo=%2Fiam%2Fx');
    expect(seen.url).toBe('https://app.example.com/iam/auth/login?returnTo=%2Fiam%2Fx');
  });

  // The route table decides, not a prefix test: with basePath '/auth', the stripped path '/auth/login'
  // STARTS with the basePath and is still not a full path.
  it('is not confused by a zone whose basePath is /auth', async () => {
    const rt = runtime({ basePath: '/auth' });
    expect((await requestSeenBy(rt, 'http://0.0.0.0:3000/auth/login')).url).toBe('https://app.example.com/auth/auth/login');
    expect((await requestSeenBy(rt, 'http://127.0.0.1:4000/auth/auth/login')).url).toBe('https://app.example.com/auth/auth/login');
  });

  it('works for a root-mounted zone', async () => {
    const seen = await requestSeenBy(runtime({ basePath: '' }), 'http://0.0.0.0:3000/auth/login');
    expect(seen.url).toBe('https://app.example.com/auth/login');
  });

  it('passes a non-auth path on with only the origin changed, so the route table still 404s it', async () => {
    const seen = await requestSeenBy(runtime(), 'http://0.0.0.0:3000/elsewhere');
    expect(seen.url).toBe('https://app.example.com/elsewhere');
  });

  it('keeps the method, the headers and the body', async () => {
    const seen = await requestSeenBy(runtime(), 'http://0.0.0.0:3000/auth/logout', { method: 'POST', headers: { cookie: 'a=b' }, body: 'x=1' });
    expect(seen.method).toBe('POST');
    expect(seen.headers.get('cookie')).toBe('a=b');
    expect(await seen.text()).toBe('x=1');
  });
});
```

Run:

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/server.test.ts
```

Expected: FAIL on five new cases, because the handler still passes the request on unchanged — for example `expected 'http://0.0.0.0:3000/auth/callback?code=c&state=s' to be 'https://app.example.com/iam/auth/callback?code=c&state=s'`. `keeps the method, the headers and the body` passes already. The seven existing cases pass (one `it`, four `it.each` rows, then two more `it`). Totals: 5 failed, 8 passed.

- [ ] **Step 8: Rebuild the URL in `createAuthRouteHandler`**

In `src/server.ts`, replace lines 19–21:

```ts
import { CallbackRejected } from './core/errors';
import { createAuthRoutes, type AuthRoutes } from './http/routes';
import type { AuthRuntime } from './runtime';
```

with:

```ts
import { CallbackRejected } from './core/errors';
import { AUTH_ROUTE_SUFFIXES } from './http/route-table';
import { createAuthRoutes, type AuthRoutes } from './http/routes';
import type { AuthRuntime } from './runtime';
```

Replace lines 33–82 (32–81 before the import edit above; the doc comment and the whole `createAuthRouteHandler` function, from its `/**` to its closing `}`) with:

```ts
/** A `RequestInit` that can carry a streamed body: Node's `Request` needs `duplex: 'half'` for one. */
interface RequestInitWithDuplex extends RequestInit {
  duplex?: 'half';
}

/**
 * The URL `createAuthRoutes` must see: this zone's PUBLIC origin, the FULL path (basePath
 * included) and the incoming query string (SMA-511 spec § 7.1).
 *
 * A Next route handler gets a `req.url` with the basePath REMOVED and the server's BIND address as
 * its origin — measured on Next 16.3.4: `http://0.0.0.0:<port>/auth/callback?…` under
 * `basePath: '/iam'` (spec § 13 row 1). The core route table is keyed by the full path, so this
 * puts the basePath back. A plain `node:http` caller (tests/e2e/fixture-server.ts) passes the full
 * path already. The route table tells the two apart, not a prefix test: a path that is already one
 * of this zone's auth routes is kept, and a path that becomes one with the basePath added gets it.
 * So a zone whose basePath is `/auth` is not ambiguous.
 */
function publicRequestUrl(runtime: AuthRuntime, routePaths: ReadonlySet<string>, raw: string): string {
  const incoming = new URL(raw);
  const prefixed = `${runtime.basePath}${incoming.pathname}`;
  const pathname = !routePaths.has(incoming.pathname) && routePaths.has(prefixed) ? prefixed : incoming.pathname;
  // Concatenated onto the absolute origin, never `new URL(path, origin)`: a path such as
  // `//evil.example/auth/login` would otherwise resolve as a protocol-relative URL on another host.
  return `${runtime.publicOrigin}${pathname}${incoming.search}`;
}

/** The same request at another URL. Method, headers, body and abort signal carry over. */
function withUrl(req: Request, url: string): Request {
  const init: RequestInitWithDuplex = { method: req.method, headers: req.headers, signal: req.signal };
  if (req.method !== 'GET' && req.method !== 'HEAD' && req.body !== null) {
    init.body = req.body;
    init.duplex = 'half';
  }
  return new Request(url, init);
}

/**
 * Builds the same four routes as `createAuthRoutes`, mounted as a single Next route handler
 * (`app/auth/[...auth]/route.ts`'s `GET`/`POST`), with two differences.
 *
 * 1. The request URL is rebuilt on `PAIGASUS_PUBLIC_ORIGIN` with the basePath put back (see
 *    `publicRequestUrl` above), so the core route table, keyed by the full path, finds the route.
 *
 * 2. `CallbackRejected` is mapped to a `Response` instead of left to propagate. `http/routes.ts`
 *    deliberately lets `CallbackRejected` reject `handle()`'s promise rather than deciding an HTTP
 *    response itself (see that file's header) — this is the Next boundary that makes that decision,
 *    so a stale tab hitting `/auth/callback` a second time becomes a redirect, not an unhandled
 *    rejection a Next route handler turns into a 500.
 *
 * The five reasons split into two outcomes. `txn_missing`, `txn_mismatch`, and `state_unknown` all
 * mean "this callback cannot be completed with what the server has" — a stale tab, an expired
 * (10-minute) transaction, or a replayed request — and the safe, unsurprising recovery is the same
 * for all three: send the browser back to start a fresh login. `idp_error` is the user themselves
 * declining at the identity provider (e.g. clicking Cancel), so it returns them to the zone root
 * rather than straight back into another login attempt. `code_exchange_failed` is the one reason
 * that is a genuine failure (an unreachable or erroring token endpoint) rather than an expected
 * outcome, so it surfaces as a 502 rather than a silent redirect — an operator must be able to
 * tell it apart from ordinary traffic.
 *
 * The redirect Locations below are FULL paths. A raw `Response` Location from a route handler
 * passes through Next unchanged (measured, spec § 13 row 1).
 */
export function createAuthRouteHandler(runtime: AuthRuntime): AuthRoutes['handle'] {
  const routes = createAuthRoutes(runtime);
  const routePaths: ReadonlySet<string> = new Set(AUTH_ROUTE_SUFFIXES.map((suffix) => `${runtime.basePath}${suffix}`));

  const redirectTo = (path: string): Response => new Response(null, { status: 302, headers: new Headers({ Location: path }) });

  return async function handle(req: Request): Promise<Response> {
    try {
      return await routes.handle(withUrl(req, publicRequestUrl(runtime, routePaths, req.url)));
    } catch (err) {
      if (!(err instanceof CallbackRejected)) throw err;

      switch (err.reason) {
        case 'txn_missing':
        case 'txn_mismatch':
        case 'state_unknown':
          return redirectTo(`${runtime.basePath}/auth/login`);
        case 'idp_error':
          return redirectTo(`${runtime.basePath}/`);
        case 'code_exchange_failed':
          return new Response('login failed', { status: 502 });
        default: {
          // Exhaustiveness guard: a future sixth CallbackRejected reason fails typecheck here
          // rather than silently falling through to an unhandled rejection again.
          const exhaustive: never = err.reason;
          throw new Error(`unreachable: unmapped CallbackRejected reason ${String(exhaustive)}`, { cause: err });
        }
      }
    }
  };
}
```

Run:

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/server.test.ts
```

Expected: PASS, 13 cases (the seven existing cases and the six new ones).

- [ ] **Step 9: Write the failing `redirect_uri` tests**

In `tests/http/callback.test.ts` (the line numbers below are for the file after Step 2 and after each earlier item of this step):

1. After line 43 (`let grantCalls: number;`) add `let grantUrls: string[];`.
2. In `countingOidc` (lines 61–64 after item 1; 60–63 before it), replace the four-line `authorizationCodeGrant` entry with:

```ts
    authorizationCodeGrant: (params) => {
      grantCalls += 1;
      grantUrls.push(params.currentUrl.href);
      return inner.authorizationCodeGrant(params);
    },
```

3. In `beforeEach`, after `grantCalls = 0;` (line 76 after items 1 and 2; 74 before them) add `grantUrls = [];`.
4. Before `describe('route dispatch', …)` (line 440 after Step 2 and items 1–3; 436 in the unchanged file) add:

```ts
// SMA-511 spec § 7.1. openid-client sends the token request's `redirect_uri` as `currentUrl` with the
// query removed (openid-client build/index.js `stripParams(currentUrl)`). A Next route handler's
// `req.url` carries the server's BIND address, so a `currentUrl` built from the request would not
// equal the redirect_uri sent to /authorize, and the IdP rejects the exchange.
describe('GET /auth/callback — redirect_uri (SMA-511 spec § 7.1)', () => {
  it('builds currentUrl from runtime.redirectUri, not from a bind-address request URL', async () => {
    const state = 'state-bind-address';
    await seedTransaction(state);
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE }));

    const req = new Request(`http://0.0.0.0:3000/iam/auth/callback?code=test-code&state=${state}`, { headers: { cookie: cookieHeaderFor(state, CORRECT_SECRET) } });
    const res = await createAuthRoutes(runtime).handle(req);

    expect(res.status).toBe(302);
    expect(grantUrls).toEqual([`${CALLBACK_URL}?code=test-code&state=${state}`]);
  });

  it('uses an overridden redirect URI as it is, so redirect_uri always equals the /authorize value', async () => {
    const state = 'state-override';
    await seedTransaction(state);
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE }));

    const routes = createAuthRoutes({ ...runtime, redirectUri: 'https://proxy.example.com/cb' });
    const res = await routes.handle(callbackRequest(state, cookieHeaderFor(state, CORRECT_SECRET)));

    expect(res.status).toBe(302);
    expect(grantUrls).toEqual([`https://proxy.example.com/cb?code=test-code&state=${state}`]);
  });
});
```

Run:

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/http/callback.test.ts
```

Expected: FAIL on the two new cases — `grantUrls` holds `http://0.0.0.0:3000/iam/auth/callback?…` and `https://rp.example.com/iam/auth/callback?…`.

- [ ] **Step 10: Build `currentUrl` from the redirect URI**

In `src/http/routes.ts`, replace lines 204–207 (only these four lines; `codeVerifier`, `expectedState`, `expectedNonce` and `});` below them stay):

```ts
  let tokens: OidcTokens;
  try {
    tokens = await runtime.oidc.authorizationCodeGrant({
      currentUrl: url,
```

with:

```ts
  // SMA-511 spec § 7.1: `currentUrl` is runtime.redirectUri plus the incoming query string, NEVER
  // the request URL. openid-client derives the token request's `redirect_uri` from `currentUrl`
  // (`stripParams(currentUrl)`), and a Next route handler's `req.url` carries the server's bind
  // address, so the value would not equal the one sent to /authorize. Built from the same
  // redirectUri `handleLogin` sends, the two are equal with or without an override.
  const currentUrl = new URL(runtime.redirectUri);
  currentUrl.search = url.search;

  let tokens: OidcTokens;
  try {
    tokens = await runtime.oidc.authorizationCodeGrant({
      currentUrl,
```

Run:

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/http/callback.test.ts
```

Expected: PASS, all cases.

- [ ] **Step 11: Write the failing `returnTo` loop tests**

In `tests/http/login.test.ts`, after the test `falls back to "/" (not "") on a root-mounted zone, i.e. basePath === ""` (ends at line 193 after Step 2's insert; 192 before it), add:

```ts
  // SMA-511 spec § 6.4. validateReturnTo accepts any same-origin path, including this zone's own
  // auth routes. A crafted link with returnTo=/iam/auth/login would send the browser back into the
  // login after a successful callback — one loop per click.
  it.each([
    '/iam/auth/login',
    '/iam/auth/login?returnTo=%2Fiam%2F',
    '/iam/auth/callback?code=x&state=y',
    '/iam/auth/logout',
    '/iam/auth/logout/callback',
    // Dot segments. validateReturnTo keeps these two values unchanged, and the browser resolves the
    // callback's Location to /iam/auth/login for both. A guard on the raw string misses them.
    '/iam/./auth/login',
    '/iam/x/../auth/login',
  ])('replaces a returnTo under the zone auth routes (%s) with the zone root', async (raw) => {
    const res = await createAuthRoutes(runtime).handle(loginRequest(`?returnTo=${encodeURIComponent(raw)}`));
    const state = new URL(res.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const tx = await store.takeTransaction(state);
    expect(tx?.returnTo).toBe('/iam/');
  });

  it('keeps a returnTo that only starts like an auth route', async () => {
    const res = await createAuthRoutes(runtime).handle(loginRequest('?returnTo=%2Fiam%2Fauthors'));
    const state = new URL(res.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const tx = await store.takeTransaction(state);
    expect(tx?.returnTo).toBe('/iam/authors');
  });

  it('replaces an /auth/ returnTo with "/" on a root-mounted zone', async () => {
    const rootRuntime: AuthRuntime = { ...runtime, basePath: '' };
    const res = await createAuthRoutes(rootRuntime).handle(new Request('https://rp.example.com/auth/login?returnTo=%2Fauth%2Flogin'));
    const state = new URL(res.headers.get('location') ?? '').searchParams.get('state') ?? '';
    const tx = await store.takeTransaction(state);
    expect(tx?.returnTo).toBe('/');
  });
```

Run:

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/http/login.test.ts
```

Expected: FAIL on the seven `replaces a returnTo …` cases (the five plain paths and the two dot-segment paths) and on the root-mounted case, 8 failures in total: the stored value is the raw path. `keeps a returnTo …` passes.

- [ ] **Step 12: Refuse a `returnTo` under the zone's auth routes**

In `src/http/routes.ts`, replace lines 120–122 (the last two lines of the I3 comment and the `validateReturnTo` call). Step 10 edited lines 204–207, below these lines, so these lines did not move:

```ts
  // completes on a root-mounted zone. `src/next/get-session.ts:104` and `src/runtime.ts:123` both
  // already use the trailing-slash form; this call is the one place that had drifted from it.
  const returnTo = validateReturnTo(url.searchParams.get('returnTo'), `${runtime.basePath}/`);
```

with:

```ts
  // completes on a root-mounted zone. `src/next/get-session.ts:110` and `src/runtime.ts:129` both
  // already use the trailing-slash form; this call is the one place that had drifted from it.
  const fallback = `${runtime.basePath}/`;
  const requested = validateReturnTo(url.searchParams.get('returnTo'), fallback);
  // SMA-511 spec § 6.4: a returnTo under this zone's own auth routes would send the browser back into
  // /auth/login (or /auth/callback) after a successful login, so a crafted link loops, one click per
  // round. validateReturnTo accepts such a path, because it is same-origin; it is refused here.
  //
  // The check reads the path with its dot segments resolved, because the browser resolves them in
  // the callback's Location: `/iam/./auth/login` and `/iam/x/../auth/login` both land on
  // `/iam/auth/login`. The placeholder origin only lets `new URL` parse a path. validateReturnTo has
  // already refused every value that is not a same-origin path (`//`, a backslash), so the parse
  // cannot move to another host. The stored value stays `requested`, so its query string is kept.
  const resolvedPath = new URL(requested, 'http://placeholder').pathname;
  const returnTo = resolvedPath.startsWith(`${runtime.basePath}/auth/`) ? fallback : requested;
```

The two new line citations are the positions after this task: Step 6 moves the `validateReturnTo` call in `src/next/get-session.ts` from line 104 to line 110, and Step 2 moves the `postLogoutRedirectUri` line in `src/runtime.ts` from line 123 to line 129.

Run:

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/http/login.test.ts tests/middleware.test.ts
```

Expected: PASS. `tests/middleware.test.ts`'s `routes.ts contains no direct pathname comparison` stays green: its pattern matches only `pathname ==` / `pathname ===` and the reverse, and the new code calls `.startsWith` on `resolvedPath`.

To see that the dot-segment rows are not vacuous, change `resolvedPath.startsWith` to `requested.startsWith` in a scratch edit: the `/iam/./auth/login` and `/iam/x/../auth/login` cases then fail with `expected '/iam/./auth/login' to be '/iam/'` and `expected '/iam/x/../auth/login' to be '/iam/'`. Put the line back with Edit, not with `git checkout`.

- [ ] **Step 13: Write the end-to-end route-handler test (real routes, real OIDC fixture)**

Create `tests/http/route-handler.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-511 spec § 7.1 — createAuthRouteHandler under a Next basePath, with the REAL routes and the
// real OIDC fixture. A Next route handler receives `req.url` with the basePath REMOVED and the
// server's BIND address as the host (measured, spec § 13 row 1). This drives a login, a callback and
// a logout with exactly that URL shape and asserts the redirect_uri the token request carries.
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { claimsPrincipalResolver } from '../../src/adapters/claims-resolver.js';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { createOidcClient } from '../../src/adapters/oidc.js';
import { SESSION_COOKIE, TXN_COOKIE_PREFIX } from '../../src/http/cookies.js';
import type { AuthRuntime } from '../../src/runtime.js';
import { createAuthRouteHandler } from '../../src/server.js';
import { startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';

const BIND = 'http://0.0.0.0:3000';
const PUBLIC_ORIGIN = 'https://console.example.com';

let fixture: OidcFixture;
let runtime: AuthRuntime;
let grantUrls: string[];

beforeEach(async () => {
  fixture = await startOidcFixture();
  grantUrls = [];
  const inner = createOidcClient({
    issuer: fixture.issuer,
    clientId: fixture.clientId,
    clientSecret: fixture.clientSecret,
    httpTimeoutMs: 5000,
    clockToleranceSeconds: 30,
    allowInsecureRequests: true, // the fixture is plain http on localhost — never set in production
  });
  runtime = {
    store: new MemorySessionStore(),
    resolver: claimsPrincipalResolver,
    logger: { event: () => undefined },
    oidc: {
      buildAuthorizationUrl: (params) => inner.buildAuthorizationUrl(params),
      authorizationCodeGrant: (params) => {
        grantUrls.push(params.currentUrl.href);
        return inner.authorizationCodeGrant(params);
      },
      refresh: (token) => inner.refresh(token),
      revoke: (token) => inner.revoke(token),
      buildEndSessionUrl: (params) => inner.buildEndSessionUrl(params),
    },
    publicOrigin: PUBLIC_ORIGIN,
    redirectUri: `${PUBLIC_ORIGIN}/iam/auth/callback`,
    postLogoutRedirectUri: `${PUBLIC_ORIGIN}/iam/`,
    cookieDomainless: true,
    skewMs: 30_000,
    lockTtlMs: 10_000,
    lockWaitMs: 3_000,
    ttlMs: 28_800_000,
    absoluteTtlMs: 86_400_000,
    zone: 'iam',
    basePath: '/iam',
    scopes: 'openid profile email',
  };
});

afterEach(async () => {
  await fixture.close();
});

describe('createAuthRouteHandler under basePath /iam (SMA-511 spec § 7.1)', () => {
  it('completes a login and a callback from basePath-stripped, bind-address URLs', async () => {
    const handle = createAuthRouteHandler(runtime);

    const login = await handle(new Request(`${BIND}/auth/login?returnTo=%2Fiam%2Forgs`));
    expect(login.status).toBe(302);
    const authorize = new URL(login.headers.get('location') ?? '');
    expect(authorize.origin + authorize.pathname).toBe(`${fixture.issuer}/authorize`);
    expect(authorize.searchParams.get('redirect_uri')).toBe(`${PUBLIC_ORIGIN}/iam/auth/callback`);
    const state = authorize.searchParams.get('state') ?? '';
    const nonce = authorize.searchParams.get('nonce') ?? '';
    const txnCookie = login.headers
      .getSetCookie()
      .find((c) => c.startsWith(TXN_COOKIE_PREFIX))
      ?.split(';')[0];
    expect(txnCookie).toBeDefined();

    fixture.setNextIdToken(await fixture.mintIdToken({ nonce }));
    const callback = await handle(new Request(`${BIND}/auth/callback?code=test-code&state=${state}`, { headers: { cookie: txnCookie ?? '' } }));

    expect(callback.status).toBe(302);
    expect(callback.headers.get('location')).toBe('/iam/orgs');
    expect(callback.headers.getSetCookie().some((c) => c.startsWith(`${SESSION_COOKIE}=`))).toBe(true);
    // The token request's redirect_uri equals the /authorize one, not the bind address.
    expect(grantUrls).toEqual([`${PUBLIC_ORIGIN}/iam/auth/callback?code=test-code&state=${state}`]);
  });

  it('logs out with a POST to the basePath-stripped path', async () => {
    const res = await createAuthRouteHandler(runtime)(new Request(`${BIND}/auth/logout`, { method: 'POST' }));
    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toMatch(new RegExp(`^${fixture.issuer}/logout\\?`));
  });

  it('still 404s a path that is not an auth route', async () => {
    const res = await createAuthRouteHandler(runtime)(new Request(`${BIND}/orgs`));
    expect(res.status).toBe(404);
  });
});
```

Run:

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/http/route-handler.test.ts
```

Expected: PASS (Steps 8 and 10 are already in place). To see that the file is not vacuous, revert Step 8's `withUrl(…)` call to `routes.handle(req)` in a scratch edit: the first case then fails with status 404. Put the call back with Edit, not with `git checkout` (that would discard the rest of this task).

- [ ] **Step 14: Update the README**

In `ts/packages/paigasus-auth/README.md`, after the route table (after line 115), add:

```markdown

Under Next, a zone mounts these routes at `app/auth/[...auth]/route.ts` and exports
`createAuthRouteHandler(runtime)` as `GET` and `POST`. Next removes the basePath from a route
handler's `req.url` and puts the server's bind address in it. So `createAuthRouteHandler` rebuilds
the URL as `PAIGASUS_PUBLIC_ORIGIN` + basePath + path + query before it dispatches. A plain server
that passes the full path (this package's e2e fixture) works too. The callback sends `redirect_uri`
from `runtime.redirectUri`, never from the request URL, so it always equals the value sent to the
IdP's `/authorize`. `/auth/login` resolves the dot segments of a `returnTo` path first. It then
replaces a path under the zone's own `/auth/` routes with the zone root, so a crafted link cannot
loop.
```

Replace lines 153–159 (143–149 before the insert above; the heading and first paragraph of "`publicPaths` must list every route this package serves"):

```markdown
### `publicPaths` must list every route this package serves

`createAuthMiddleware({ publicPaths, loginPath })` requires `publicPaths` to name every route
`createAuthRoutes` dispatches on for this zone: the login, callback, logout, and logout-callback
paths. **Do not hand-copy this list.** Build it with `authRoutePaths(runtime)`
(`@paigasus/auth/middleware`) instead — a pure derivation from `runtime.basePath` that always
matches the real route table.
```

with (the outer fence here has four backticks, because the block holds a code fence of its own):

````markdown
### `publicPaths` must list every route this package serves

The middleware compares `req.nextUrl.pathname`, which Next gives WITHOUT the zone's basePath. So
`publicPaths` and `loginPath` are basePath-relative: `/auth/login`, not `/iam/auth/login`. The
middleware puts the basePath back when it builds the login redirect and its `returnTo` value.

`createAuthMiddleware({ publicPaths, loginPath })` requires `publicPaths` to name every route
`createAuthRoutes` dispatches on: the login, callback, logout, and logout-callback paths. **Do not
hand-copy this list.** Build it with `authRoutePaths()` (`@paigasus/auth/middleware`), which
returns the four basePath-relative paths from the shared route table:

```ts
// proxy.ts
import { authRoutePaths, createAuthMiddleware } from '@paigasus/auth/middleware';

export default createAuthMiddleware({ publicPaths: [...authRoutePaths(), '/'], loginPath: '/auth/login' });
```

`requireSession()` redirects to the basePath-relative `/auth/login` for the same reason: Next's
`redirect()` adds the basePath itself.
````

Format it:

```bash
pnpm -C ts exec prettier --write packages/paigasus-auth/README.md
```

- [ ] **Step 15: Run the whole package**

Format every file this task creates or edits (the Global Constraints). This runs before the fmt check and before the commit:

```bash
pnpm -C ts exec prettier --write \
  packages/paigasus-auth/src/runtime.ts packages/paigasus-auth/src/middleware.ts \
  packages/paigasus-auth/src/next/get-session.ts packages/paigasus-auth/src/server.ts \
  packages/paigasus-auth/src/http/routes.ts packages/paigasus-auth/README.md \
  packages/paigasus-auth/tests/runtime.test.ts packages/paigasus-auth/tests/middleware.test.ts \
  packages/paigasus-auth/tests/next/get-session.test.ts packages/paigasus-auth/tests/server.test.ts \
  packages/paigasus-auth/tests/http/login.test.ts packages/paigasus-auth/tests/http/callback.test.ts \
  packages/paigasus-auth/tests/http/logout.test.ts packages/paigasus-auth/tests/http/route-handler.test.ts
moon run paigasus-auth-ts:test paigasus-auth-ts:typecheck paigasus-app-shell-ts:test
moon run ts:lint ts:fmt --force
```

Expected: all pass.

- [ ] **Step 16: Run the plain-Node e2e tier (Docker required)**

The fixture server passes FULL paths (`/e2e/auth/…`) on `FIXTURE_SERVER_ORIGIN`, which equals its `PAIGASUS_PUBLIC_ORIGIN`. `publicRequestUrl` keeps a path that is already an auth route, so the fixture needs no change.

```bash
docker info > /dev/null && echo "docker ok"
moon run paigasus-auth-ts:test-e2e
```

Expected: `docker ok`, then the containers suite and the Playwright suite (`roundtrip`, `recovery`, `logout`) pass.

- [ ] **Step 17: Commit**

```bash
git add ts/packages/paigasus-auth/src ts/packages/paigasus-auth/tests ts/packages/paigasus-auth/README.md
git commit -F - <<'EOF'
fix(ts): make @paigasus/auth work under a next base path (SMA-511)

Next removes the basePath from the proxy's pathname and from a route handler's URL, and it adds
the basePath to a redirect() target on its own. The middleware now matches basePath-relative public
paths and keeps the basePath in returnTo, requireSession redirects to the relative /auth/login, and
createAuthRouteHandler rebuilds the request URL on PAIGASUS_PUBLIC_ORIGIN with the basePath put
back. authRoutePaths() now takes no argument, and AuthRuntime carries publicOrigin.

The callback builds its currentUrl from runtime.redirectUri, so redirect_uri always equals the
value sent to /authorize; a route handler's URL carries the bind address. The login route refuses
a returnTo whose path, with dot segments resolved, is under the zone's own /auth/ routes, so a
crafted link cannot loop.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 6: `@paigasus/sdk/iam` re-exports `ServiceInfoService`

Spec § 7.3, § 4.5. The app provisions a principal with `ServiceInfoService.GetServiceInfo`, and apps must not import `@paigasus/proto`. `@paigasus/proto`'s ROOT entry exports the live `paigasus.common.v1.ServiceInfoService` (`ts/packages/paigasus-proto/src/index.ts:9`); the `./iam` subpath must not be used for it, because `iam.proto` keeps a deprecated `ServiceInfo` message (see `ts/packages/paigasus-proto/src/index.test.ts:44-59`).

**Files:**
- Create: `ts/packages/paigasus-sdk/tests/iam-service-info.test.ts`
- Modify: `ts/packages/paigasus-sdk/src/iam.ts:7,12` (after Task 3 the line numbers are the same; the specifiers are extensionless)
- Modify: `ts/packages/paigasus-sdk/src/index.ts:12`

**Interfaces:**
- Consumes: `ServiceInfoService` from `@paigasus/proto` (root).
- Produces: `ServiceInfoService` exported from `@paigasus/sdk/iam` and from the `@paigasus/sdk` root barrel, identical (`===`) to the proto root export. `createIamClient(ServiceInfoService, { baseUrl }, { bearer })` returns a `Client<typeof ServiceInfoService>` with `getServiceInfo`.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-sdk/tests/iam-service-info.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-511 spec § 7.3. The iam console provisions its principal with
// ServiceInfoService.GetServiceInfo (spec § 4.5), and apps must not import @paigasus/proto, so the
// SDK's ./iam entry re-exports the service. It must be the paigasus.common.v1 service from the proto
// ROOT entry — never anything from @paigasus/proto/iam, which keeps a deprecated ServiceInfo.
import { afterEach, describe, expect, it } from 'vitest';
import { ServiceInfoService as ProtoServiceInfoService } from '@paigasus/proto';
import { ServiceInfoService, createIamClient, disposeTransports } from '../src/iam.js';
import * as barrel from '../src/index.js';

afterEach(() => {
  disposeTransports();
});

describe('@paigasus/sdk/iam re-exports ServiceInfoService (SMA-511 spec § 7.3)', () => {
  it('is the SAME object as the @paigasus/proto root export', () => {
    expect(ServiceInfoService).toBe(ProtoServiceInfoService);
  });

  it('is the common.v1 service with its one rpc', () => {
    expect(ServiceInfoService.typeName).toBe('paigasus.common.v1.ServiceInfoService');
    expect(Object.keys(ServiceInfoService.method)).toEqual(['getServiceInfo']);
  });

  it('builds a request-scoped client through createIamClient', () => {
    const client = createIamClient(ServiceInfoService, { baseUrl: 'http://127.0.0.1:9' }, { bearer: 'token' });
    expect(typeof client.getServiceInfo).toBe('function');
  });

  // Both sides are checked for a value first. Before the re-export both are `undefined`, and `toBe`
  // alone passes on `undefined === undefined` (measured, pre-flight T6.a).
  it('the root barrel serves the same object', () => {
    expect(barrel.ServiceInfoService).toBeDefined();
    expect(ServiceInfoService).toBeDefined();
    expect(barrel.ServiceInfoService).toBe(ServiceInfoService);
  });
});
```

- [ ] **Step 2: Run the test and see it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-sdk exec vitest run tests/iam-service-info.test.ts
```

Expected: FAIL on all four cases:

- `is the SAME object …` — `expected undefined to be …`.
- `is the common.v1 service …` — `Cannot read properties of undefined (reading 'typeName')`.
- `builds a request-scoped client …` — `Cannot read properties of undefined (reading 'methods')`, because `createIamClient` gets `undefined`.
- `the root barrel serves the same object` — `expected undefined to be defined`. Without its two `toBeDefined()` lines this case passes here, because both sides are `undefined`.

If you delete only the `ServiceInfoService` entry from `src/index.ts` after Step 3, the fourth case fails on its first line. That is the mutation it exists to catch.

- [ ] **Step 3: Add the re-export**

In `ts/packages/paigasus-sdk/src/iam.ts`, replace line 7:

```ts
import { AuditService, AuthnService, AuthorizationService, OutboxService, ServiceAccountService, TenancyService, UserService } from '@paigasus/proto/iam';
```

with:

```ts
// ServiceInfoService comes from the proto ROOT entry: it is paigasus.common.v1, served by every
// Paigasus service. The iam console calls GetServiceInfo as its bearer-enforced provisioning call
// (SMA-511 spec § 4.5). Never take it from @paigasus/proto/iam (see that entry's ServiceInfo note).
import { ServiceInfoService } from '@paigasus/proto';
import { AuditService, AuthnService, AuthorizationService, OutboxService, ServiceAccountService, TenancyService, UserService } from '@paigasus/proto/iam';
```

Replace line 12:

```ts
export { AuditService, AuthnService, AuthorizationService, OutboxService, ServiceAccountService, TenancyService, UserService };
```

with:

```ts
export { AuditService, AuthnService, AuthorizationService, OutboxService, ServiceAccountService, ServiceInfoService, TenancyService, UserService };
```

In `ts/packages/paigasus-sdk/src/index.ts`, replace line 12:

```ts
export { AuditService, AuthnService, AuthorizationService, OutboxService, ServiceAccountService, TenancyService, UserService, bindAuth, createIamClient, disposeTransports } from './iam';
```

with:

```ts
export { AuditService, AuthnService, AuthorizationService, OutboxService, ServiceAccountService, ServiceInfoService, TenancyService, UserService, bindAuth, createIamClient, disposeTransports } from './iam';
```

- [ ] **Step 4: Run the tests and see them pass**

```bash
pnpm -C ts/packages/paigasus-sdk exec vitest run tests/iam-service-info.test.ts tests/index-barrel.test.ts tests/server-guard.test.ts
```

Expected: PASS. `tests/index-barrel.test.ts` gains the row `the barrel re-exports ./iam key ServiceInfoService` and it passes.

- [ ] **Step 5: Run the package and the lint**

```bash
pnpm -C ts exec prettier --write packages/paigasus-sdk/src/iam.ts packages/paigasus-sdk/src/index.ts packages/paigasus-sdk/tests/iam-service-info.test.ts
moon run paigasus-sdk-ts:test paigasus-sdk-ts:typecheck
moon run ts:lint ts:fmt --force
```

Expected: all pass. The `paigasus/boundaries/sdk` block allows `@paigasus/proto` (`eslint.mjs:136` after Task 2's three header lines; 133 on the branch base).

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-sdk/src/iam.ts ts/packages/paigasus-sdk/src/index.ts ts/packages/paigasus-sdk/tests/iam-service-info.test.ts
git commit -F - <<'EOF'
feat(ts): re-export ServiceInfoService from @paigasus/sdk/iam (SMA-511)

The iam console provisions its principal with ServiceInfoService.GetServiceInfo, and apps must not
import @paigasus/proto. The ./iam entry and the root barrel now re-export the paigasus.common.v1
service from the proto root entry, and a test pins that it is the same object.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 7: Boundary rules for `proxy.ts` and the app test doubles

Spec § 7.4. Two changes in `@paigasus/next-config/eslint`:

1. `paigasus/boundaries/app-middleware` matches `apps/**/middleware.*` only. Next 16 names the file `proxy.ts`. The glob becomes `apps/**/{middleware,proxy}.{ts,js,mts,cts,mjs,cjs}`. In flat config this block REPLACES the `apps` block's `no-restricted-imports` options for those files, so it must restate the `@paigasus/proto` ban.
2. The app's fake IAM (Task 11) builds an `ErrorInfo` detail, and only `@paigasus/proto` exports `ErrorInfoSchema`. The `apps` block gains `ignores: ['apps/*/tests/support/**']`. Only the test doubles live there.

**Files:**
- Modify: `ts/packages/paigasus-next-config/src/eslint.mjs:186-195` (the `apps` block) and `:287-302` (the `app-middleware` block). These are the line numbers on the branch base. Task 2 adds three header lines at line 35, so after Task 2 add 3 to every line number in this task; the steps quote the old text, so match the text.
- Modify: `ts/packages/paigasus-next-config/tests/boundaries.test.ts` — new DENIED rows (before the `];` that closes `DENIED`), one new ALLOWED row (before the `];` that closes `ALLOWED`), and three new real-config cases at the end of `describe('the workspace eslint config actually applies the preset', …)`

**Interfaces:**
- Consumes: nothing new.
- Produces: `boundaryRules` entry `paigasus/boundaries/apps` with `ignores: ['apps/*/tests/support/**']`; entry `paigasus/boundaries/app-middleware` with `files: ['apps/**/{middleware,proxy}.{ts,js,mts,cts,mjs,cjs}']` and two pattern groups (auth/server + sdk; proto). Both scopes still derive the key `apps`, which `BOUNDARY_SCOPES` already holds.

- [ ] **Step 1: Write the failing rows**

In `ts/packages/paigasus-next-config/tests/boundaries.test.ts`, add these rows at the end of `DENIED` (before its closing `];`):

```ts
  // SMA-511 spec § 7.4. Next 16 names the middleware file `proxy.ts`. The app-middleware block
  // REPLACES the apps block's options for these files, so it restates the proto ban; these rows
  // prove both halves on both file names.
  ['an app proxy must not import auth/server', 'apps/iam-console/proxy.ts', "import { getSession } from '@paigasus/auth/server';"],
  ['an app proxy must not import the sdk', 'apps/iam-console/proxy.ts', "import { x } from '@paigasus/sdk';"],
  ['an app proxy must not import a sdk SUBPATH', 'apps/iam-console/proxy.ts', "import { x } from '@paigasus/sdk/iam';"],
  ['an app proxy must not import proto — the restated ban', 'apps/iam-console/proxy.ts', "import { x } from '@paigasus/proto';"],
  ['an app proxy must not import a proto SUBPATH', 'apps/iam-console/proxy.ts', "import { x } from '@paigasus/proto/iam';"],
  ['an app middleware must not import proto — the restated ban', 'apps/iam-console/middleware.ts', "import { x } from '@paigasus/proto';"],
  // The test-double exemption is NARROW: only apps/*/tests/support/**.
  ['app lib code must not import proto', 'apps/iam-console/lib/iam.ts', "import { x } from '@paigasus/proto';"],
  ['an app test outside tests/support must not import proto', 'apps/iam-console/tests/unit/errors.test.ts', "import { ErrorInfoSchema } from '@paigasus/proto';"],
```

Add this row at the end of `ALLOWED` (before its closing `];`):

```ts
  // SMA-511 spec § 7.4.
  ['an app proxy may import auth/middleware', 'apps/iam-console/proxy.ts', "import { authRoutePaths, createAuthMiddleware } from '@paigasus/auth/middleware';"],
```

The test-double exemption does NOT go into `ALLOWED`. `restrictedImportsFor` lints through `boundaryRules` only. After Step 2's `ignores`, no block's `files` matches `apps/iam-console/tests/support/fake-iam.ts`, so ESLint does not lint that path at all. With `warnIgnored: false` the result is empty, and an ALLOWED row passes for that reason alone (measured, pre-flight T7.a). So the exemption is tested through the REAL config, on a `.mjs` path that the real config lints, with a DENIED twin one directory over.

At the end of `describe('the workspace eslint config actually applies the preset', …)` (after the `carries every boundary entry in its EXPORTED array …` case, before the `});` that closes the `describe`), add:

```ts
  // SMA-511 spec § 7.4 — the app test-double exemption (`ignores: ['apps/*/tests/support/**']`),
  // through the REAL config. Not an ALLOWED row: through `boundaryRules` alone no block matches a
  // tests/support `.ts` path after the `ignores`, so ESLint does not lint it, and an empty result
  // would prove nothing (measured, pre-flight T7.a). The real config lints every `.mjs` path
  // (`js.configs.recommended` has no `files` key). The `isPathIgnored` check proves that the path
  // is linted, so an empty list here means that no rule bans the import.
  const TEST_DOUBLE_PATH = 'apps/iam-console/tests/support/fake-iam.mjs';
  const TEST_DOUBLE_IMPORTS: ReadonlyArray<readonly [string, string]> = [
    ['proto (it builds ErrorInfo details)', "import { ErrorInfoSchema } from '@paigasus/proto';\nexport const y = ErrorInfoSchema;\n"],
    ['a proto SUBPATH', "import { TenancyService } from '@paigasus/proto/iam';\nexport const y = TenancyService;\n"],
  ];

  it.each(TEST_DOUBLE_IMPORTS)('an app test double under tests/support may import %s, through the REAL config', async (_label, source) => {
    const ignored = await new ESLint({ cwd: TS_ROOT }).isPathIgnored(TEST_DOUBLE_PATH);
    expect(ignored, 'the real config does not lint this path, so an empty result would prove nothing').toBe(false);
    expect(await realConfigRestrictedImportsFor(TEST_DOUBLE_PATH, source)).toEqual([]);
  });

  // The DENIED twin: the same import one directory over, through the same config. If the exemption
  // is widened (for example to `apps/*/tests/**`), this case fails.
  it('an app test OUTSIDE tests/support still may not import proto, through the REAL config', async () => {
    const messages = await realConfigRestrictedImportsFor('apps/iam-console/tests/unit/errors.mjs', "import { ErrorInfoSchema } from '@paigasus/proto';\nexport const y = ErrorInfoSchema;\n");
    expect(messages, 'the test-double exemption covers more than apps/*/tests/support/**').not.toHaveLength(0);
  });
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-next-config exec vitest run tests/boundaries.test.ts
```

Expected: FAIL on exactly 6 cases:

- `an app proxy must not import auth/server`, `… the sdk`, `… a sdk SUBPATH` — no block bans those for `proxy.ts` yet.
- `an app middleware must not import proto — the restated ban` — the app-middleware block replaces the apps block for `middleware.ts` and has no proto group. This is a real gap today.
- the two `an app test double under tests/support may import …, through the REAL config` cases — the apps block still bans proto under `tests/support/`. Their `isPathIgnored` line passes; the `toEqual([])` line fails with the apps block's proto message.

The two `an app proxy must not import proto…` rows, the `lib` row, the `tests/unit` row and the real-config DENIED twin (`an app test OUTSIDE tests/support …`) pass already, because the apps block covers those files.

- [ ] **Step 2: Change the two blocks**

In `ts/packages/paigasus-next-config/src/eslint.mjs`, replace lines 186–195:

```js
  {
    name: 'paigasus/boundaries/apps',
    files: ['apps/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    rules: restrict([
      {
        group: ['@paigasus/proto', '@paigasus/proto/**'],
        message: 'Apps reach the contract through @paigasus/sdk, never @paigasus/proto directly (§ 6).',
      },
    ]),
  },
```

with:

```js
  {
    name: 'paigasus/boundaries/apps',
    files: ['apps/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    // SMA-511 spec § 7.4. An app's in-process fake IAM builds a google.rpc.ErrorInfo detail, and only
    // @paigasus/proto exports ErrorInfoSchema. Only the test doubles live under tests/support/, so
    // the exemption is that directory and nothing wider — tests/boundaries.test.ts proves an app
    // test outside it is still denied.
    ignores: ['apps/*/tests/support/**'],
    rules: restrict([APP_PROTO_BAN]),
  },
```

Above `export const boundaryRules = [` (line 112), after the `APP_SHELL_BARE_ROOTS` constant (ends at line 94), add:

```js

/**
 * The apps' @paigasus/proto ban. Two blocks carry it: `paigasus/boundaries/apps` and
 * `paigasus/boundaries/app-middleware`. The second REPLACES the first's `no-restricted-imports`
 * options for middleware/proxy files, so it must restate this group or the ban is lost there.
 */
const APP_PROTO_BAN = {
  group: ['@paigasus/proto', '@paigasus/proto/**'],
  message: 'Apps reach the contract through @paigasus/sdk, never @paigasus/proto directly (§ 6).',
};
```

Replace lines 287–302 (the whole `app-middleware` block):

```js
  {
    name: 'paigasus/boundaries/app-middleware',
    // 'apps/**/middleware…' rather than 'apps/*/middleware…': the latter derives the scope key
    // 'apps/*/middleware.{ts,js,mts,cts,mjs,cjs}' (a file glob, not a directory), which the
    // liveness test's `existsSync` check can never resolve. This form derives 'apps' instead —
    // the SAME key the `paigasus/boundaries/apps` block above already owns in BOUNDARY_SCOPES —
    // so it needs no scope entry of its own.
    files: ['apps/**/middleware.{ts,js,mts,cts,mjs,cjs}'],
    rules: restrict([
      {
        group: ['@paigasus/auth/server', '@paigasus/auth/server/**', '@paigasus/sdk', '@paigasus/sdk/**'],
        message:
          "An app's middleware must import @paigasus/auth/middleware, never /server or the sdk. `server-only` is a NO-OP in the middleware layer, so nothing else stops a token-bearing module being bundled there.",
      },
    ]),
  },
```

with:

```js
  {
    name: 'paigasus/boundaries/app-middleware',
    // 'apps/**/…' rather than 'apps/*/…': the latter derives the scope key
    // 'apps/*/{middleware,proxy}.{…}' (a file glob, not a directory), which the liveness test's
    // `existsSync` check can never resolve. This form derives 'apps' instead — the SAME key the
    // `paigasus/boundaries/apps` block above already owns in BOUNDARY_SCOPES — so it needs no scope
    // entry of its own.
    //
    // `proxy` since SMA-511 (spec § 7.4): Next 16.3.4 deprecates `middleware.ts` in favour of
    // `proxy.ts`. This block REPLACES the apps block's options for these files, so it restates
    // APP_PROTO_BAN as its second group.
    files: ['apps/**/{middleware,proxy}.{ts,js,mts,cts,mjs,cjs}'],
    rules: restrict([
      {
        group: ['@paigasus/auth/server', '@paigasus/auth/server/**', '@paigasus/sdk', '@paigasus/sdk/**'],
        message:
          "An app's proxy (middleware) must import @paigasus/auth/middleware, never /server or the sdk. `server-only` is a NO-OP in the middleware layer, so nothing else stops a token-bearing module being bundled there.",
      },
      APP_PROTO_BAN,
    ]),
  },
```

- [ ] **Step 3: Run the tests and see them pass**

```bash
pnpm -C ts/packages/paigasus-next-config exec vitest run tests/boundaries.test.ts tests/no-js-relative-specifier.test.ts
```

Expected: PASS. The liveness tests stay green: both blocks still derive the scope key `apps`. The `carries every boundary entry in its EXPORTED array` test still passes: it compares `files` only, and `ts/eslint.config.js` spreads the same array.

To see that the three real-config cases are not vacuous, make two scratch edits in `src/eslint.mjs`, one at a time. Put each line back with Edit, not with `git checkout`:

- Change `ignores: ['apps/*/tests/support/**']` to `ignores: ['apps/*/tests/**']`. The DENIED twin `an app test OUTSIDE tests/support …` fails.
- Delete the `ignores:` line. The two `an app test double under tests/support may import …` cases fail.

- [ ] **Step 4: Run the package and the lint**

```bash
pnpm -C ts exec prettier --write packages/paigasus-next-config/src/eslint.mjs packages/paigasus-next-config/tests/boundaries.test.ts
moon run paigasus-next-config-ts:test paigasus-next-config-ts:typecheck
moon run ts:lint ts:fmt --force
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-next-config/src/eslint.mjs ts/packages/paigasus-next-config/tests/boundaries.test.ts
git commit -F - <<'EOF'
feat(ts): extend the app boundary rules to proxy.ts and the test doubles (SMA-511)

Next 16 names the middleware file proxy.ts, so the app-middleware block now matches both names.
That block replaces the apps block for those files in flat config, so it restates the
@paigasus/proto ban. The apps block ignores apps/*/tests/support, where the fake IAM builds an
ErrorInfo detail that only @paigasus/proto exports. New rows prove both halves on both names.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 8: The kernel wasm spike (D6) and `lib/prn.ts`

Spec § 4.7, decision D6. PRN logic belongs to `paigasus-kernel` (ADR-0005). The napi binding cannot load in a Next build (spec § 13 row 5). This task first tries the kernel's wasm binding on the server (the spike). If the spike fails, it writes the fallback C reader, held to the kernel by the parity corpus, without asking again. `lib/prn.ts` has the same interface either way, and ONE test file tests that interface for both implementations.

Facts this task relies on (all read from the repo):

- PRN grammar: `prn:pgs:<service>:<region>:<org>:<resource-type>/<resource-id>` (`rs/crates/libs/paigasus-kernel/src/resource_name.rs:2-3`). Max 512 bytes; 6 colon fields; `prn`, `pgs`; service a lowercase label; region empty or `^[a-z0-9]+(-[a-z0-9]+)*$`; org empty or a 36-character hyphenated UUID; exactly one `/`; resource id a 36-character UUID. Canonical form lower-cases the UUIDs (`:101-107`, `:161-166`).
- IAM tenancy rule (`rs/crates/libs/paigasus-iam-core/src/tenancy.rs:66-69,72-127`): service `iam`; `organization` has NO org field (its org UUID is its resource id); `team` and `project` have an org field.
- `ROOT_PRN`: `root_prn()` is `Prn::build("iam", "", None, "root", Uuid::nil())` (`rs/crates/libs/paigasus-iam-core/src/authz/model.rs:30-32`), which canonicalises to `prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000` (the same literal as `rs/crates/services/paigasus-iam/tests/authz_schema.rs:24`).
- Corpus: `rs/crates/libs/paigasus-kernel-parity/vectors/prn_fields.json` is an array of `{ prn, service, region, org, resource_type, resource_id }` (6 rows: organization, team, project, service-account, user, gateway api-key). `prn_canonical.json` is an array of `{ input, error_kind, canonical }` (23 rows; `error_kind` is `""` for a valid input, and then `canonical` is a string, else `null`).
- The wasm entry `ts/packages/paigasus-kernel/src/wasm.ts` re-exports `prnBuild, prnErrorKind, prnOrg, prnResourceType, prnResourceId, prnService, …` from `@paigasus/wasm` (the committed `--target bundler` glue in `rs/crates/bindings/paigasus-wasm/`). Each accessor throws the error kind on an invalid PRN; `prnErrorKind` returns `""` for a valid one; `prnOrg` returns `""` when the org field is empty.
- `@paigasus/wasm` is a pnpm `file:` dependency. pnpm HARD-LINKS the crate files into `ts/node_modules/.pnpm/@paigasus+wasm@file+..+rs+crates+bindings+paigasus-wasm/node_modules/@paigasus/wasm/` at install time (measured in this worktree: same inode). `paigasus_wasm_bg.wasm` is gitignored and made by `paigasus-kernel-ts:build`. In CI, `pnpm install` runs BEFORE that build, so the installed copy has NO `.wasm` there. The spike must measure this CI state (Step 10), not only a warm developer tree.
- The app's `vitest.config.ts` exists (Task 1 moved it): `environment: 'node'`, `include: ['tests/**/*.test.ts']`, 120 s timeouts, no alias.

**Files:**
- Create: `ts/apps/iam-console/tests/unit/prn.test.ts`
- Create: `ts/apps/iam-console/tests/support/server-only-stub.ts`
- Create: `ts/apps/iam-console/lib/prn.ts` (wasm version in Step 5; replaced by the fallback in Step 14 if the spike fails)
- Modify: `ts/apps/iam-console/vitest.config.ts` (whole file, 12 lines)
- Modify: `ts/apps/iam-console/package.json` (`dependencies`, `devDependencies`)
- Modify: `ts/apps/iam-console/moon.yml` — `fileGroups.sources` (lines 27–29), `test.inputs` (lines 139–155); wasm branch only: `dependsOn` (lines 11–13), `build.deps` and `build.inputs` (lines 51–94)
- Modify: `ts/moon.yml:11-15` (`fileGroups.sources`)
- Modify (wasm branch only): `ci/affected-graph/run.sh` — five strict-equality cases: `kernel->bindings` (lines 295–296), `binding-oneway-node` (304–305), `binding-oneway-wasm` (308–309), `lockfile->all-lint` (354–355), `kernel->consumer-tasks` (367–368)
- Modify (spike; kept only in the wasm branch): `ts/packages/paigasus-kernel/package.json` (`exports`, `_comment_exports`)
- Create then delete (spike only): `ts/apps/iam-console/app/prn-probe/route.ts`
- Modify: `docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md` § 13 (record the spike result)
- Modify: `ts/pnpm-lock.yaml` (by `pnpm install`)

**Interfaces:**
- Consumes: the kernel parity vectors and `model.rs` (test only); in the wasm branch, `@paigasus/kernel/wasm`.
- Produces (`ts/apps/iam-console/lib/prn.ts`, first line after the header `import 'server-only';`):

```ts
export type TenancyKind = 'organization' | 'team' | 'project';
export type TenancyRef = { kind: TenancyKind; orgId: string; id: string };
export function parseTenancyPrn(prn: string): TenancyRef | null; // null for non-tenancy or invalid; ids lower-case
export function organizationPrn(orgId: string): string;          // throws TypeError if orgId is not a UUID
export function teamPrn(orgId: string, teamId: string): string;  // throws TypeError on a non-UUID
export function projectPrn(orgId: string, projectId: string): string; // throws TypeError on a non-UUID
export function isUuid(value: string): boolean;                  // 36-char hyphenated, case-insensitive
export const ROOT_PRN: string; // 'prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000'
```

For an organization, `orgId` and `id` are both the organization's UUID.

- [ ] **Step 1: Write the interface test**

Create `ts/apps/iam-console/tests/unit/prn.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// lib/prn.ts against the kernel (SMA-511 spec § 4.7, decision D6). The corpus rows are the SAME
// vectors every kernel binding replays (rs/crates/libs/paigasus-kernel-parity/vectors/), so a reader
// that drifts from the Rust grammar fails here. This suite tests the INTERFACE, so it is the same for
// both implementations of lib/prn.ts (the kernel wasm binding, or the fallback reader).
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { ROOT_PRN, isUuid, organizationPrn, parseTenancyPrn, projectPrn, teamPrn, type TenancyRef } from '../../lib/prn';

// tests/unit -> tests -> iam-console -> apps -> ts -> repo root: five `../`.
const REPO_ROOT = new URL('../../../../../', import.meta.url);

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, REPO_ROOT)), 'utf8');
}

function vectors<T>(name: string): T[] {
  return JSON.parse(read(`rs/crates/libs/paigasus-kernel-parity/vectors/${name}.json`)) as T[];
}

type FieldsCase = { prn: string; service: string; region: string; org: string; resource_type: string; resource_id: string };
type CanonicalCase = { input: string; error_kind: string; canonical: string | null };

const fieldsCases = vectors<FieldsCase>('prn_fields');
const canonicalCases = vectors<CanonicalCase>('prn_canonical');
const TENANCY = new Set(['organization', 'team', 'project']);

const ORG = '0190a100-0000-7000-8000-0000000000aa';
const TEAM = '0190a1b2-0000-7000-8000-000000000001';

/** IAM's tenancy rule (paigasus-iam-core tenancy.rs `check`): organization has no org field; team and project have one. */
function expectedRef(c: FieldsCase): TenancyRef | null {
  if (c.service !== 'iam' || !TENANCY.has(c.resource_type)) return null;
  if (c.resource_type === 'organization') return c.org === '' ? { kind: 'organization', orgId: c.resource_id, id: c.resource_id } : null;
  return c.org === '' ? null : { kind: c.resource_type as 'team' | 'project', orgId: c.org, id: c.resource_id };
}

function build(ref: TenancyRef): string {
  switch (ref.kind) {
    case 'organization':
      return organizationPrn(ref.id);
    case 'team':
      return teamPrn(ref.orgId, ref.id);
    case 'project':
      return projectPrn(ref.orgId, ref.id);
  }
}

describe('lib/prn.ts against the kernel parity corpus', () => {
  // Without this, an emptied or reshaped corpus would make every case below pass vacuously.
  it('holds a vector of each tenancy kind, a non-tenancy vector and an invalid vector', () => {
    const kinds = new Set(
      fieldsCases
        .map(expectedRef)
        .filter((ref) => ref !== null)
        .map((ref) => ref.kind),
    );
    expect([...kinds].sort()).toEqual(['organization', 'project', 'team']);
    expect(fieldsCases.some((c) => expectedRef(c) === null)).toBe(true);
    expect(canonicalCases.some((c) => c.error_kind !== '')).toBe(true);
  });

  it.each(fieldsCases)('prn_fields: $prn', (c) => {
    const expected = expectedRef(c);
    expect(parseTenancyPrn(c.prn)).toEqual(expected);
    if (expected !== null) expect(build(expected)).toBe(c.prn);
  });

  it.each(canonicalCases.map((c, index) => [index, c] as const))('prn_canonical vector %i', (_index, c) => {
    if (c.error_kind !== '') {
      expect(parseTenancyPrn(c.input)).toBeNull();
      return;
    }
    const canonical = c.canonical ?? '';
    const ref = parseTenancyPrn(canonical);
    expect(parseTenancyPrn(c.input)).toEqual(ref);
    if (ref !== null) expect(build(ref)).toBe(canonical);
  });
});

describe('ROOT_PRN', () => {
  it('is the canonical form of IAM root_prn()', () => {
    expect(ROOT_PRN).toBe('prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000');
  });

  // A change to root_prn()'s arguments must re-derive ROOT_PRN. The app's test task lists model.rs
  // among its inputs, so an edit there re-runs this assertion.
  it('still matches the Rust builder call it is derived from', () => {
    expect(read('rs/crates/libs/paigasus-iam-core/src/authz/model.rs')).toMatch(/pub fn root_prn\(\) -> Prn \{\s*Prn::build\("iam", "", None, "root", Uuid::nil\(\)\)/);
  });

  it('is not a tenancy PRN', () => {
    expect(parseTenancyPrn(ROOT_PRN)).toBeNull();
  });
});

describe('parseTenancyPrn', () => {
  it('lower-cases the ids of an upper-case PRN, as the kernel canonicalises', () => {
    expect(parseTenancyPrn(`prn:pgs:iam::${ORG.toUpperCase()}:team/${TEAM.toUpperCase()}`)).toEqual({ kind: 'team', orgId: ORG, id: TEAM });
  });

  it.each([
    ['a team without an org field', `prn:pgs:iam:::team/${TEAM}`],
    ['an organization WITH an org field', `prn:pgs:iam::${ORG}:organization/${TEAM}`],
    ['another service', `prn:pgs:gateway::${ORG}:team/${TEAM}`],
    ['a non-tenancy IAM type', `prn:pgs:iam:::user/${TEAM}`],
    ['a malformed org UUID', `prn:pgs:iam::not-a-uuid:team/${TEAM}`],
    ['an upper-case region', `prn:pgs:iam:US-EAST:${ORG}:team/${TEAM}`],
    ['two slashes in the resource path', `prn:pgs:iam::${ORG}:team/${TEAM}/x`],
    ['the empty string', ''],
    // Valid in every field except its length: 12 + 480 + 1 + 36 + 1 + 5 + 36 = 571 characters. The
    // kernel sets no region length limit other than MAX_LEN (512), so ONLY the length rule rejects it.
    ['a valid team PRN of 571 characters (a 480-character region)', `prn:pgs:iam:${'a'.repeat(480)}:${ORG}:team/${TEAM}`],
  ])('returns null for %s', (_label, prn) => {
    expect(parseTenancyPrn(prn)).toBeNull();
  });
});

describe('isUuid', () => {
  it.each(['0190a1b2-0000-7000-8000-000000000001', '0190A1B2-0000-7000-8000-00000000ABCD', '00000000-0000-0000-0000-000000000000'])('accepts %s', (value) => {
    expect(isUuid(value)).toBe(true);
  });

  it.each(['', 'not-a-uuid', '0190a1b2000070008000000000000001', '{0190a1b2-0000-7000-8000-000000000001}', '0190a1b2-0000-7000-8000-00000000000g', ' 0190a1b2-0000-7000-8000-000000000001'])(
    'rejects %j',
    (value) => {
      expect(isUuid(value)).toBe(false);
    },
  );
});

describe('the builders', () => {
  it('build canonical, lower-case PRNs', () => {
    expect(organizationPrn(ORG.toUpperCase())).toBe(`prn:pgs:iam:::organization/${ORG}`);
    expect(teamPrn(ORG, TEAM)).toBe(`prn:pgs:iam::${ORG}:team/${TEAM}`);
    expect(projectPrn(ORG, TEAM)).toBe(`prn:pgs:iam::${ORG}:project/${TEAM}`);
  });

  it('throw a TypeError for an id that is not a UUID, so a bad URL segment cannot become a PRN', () => {
    expect(() => organizationPrn('x')).toThrow(TypeError);
    expect(() => teamPrn(ORG, '../x')).toThrow(TypeError);
    expect(() => projectPrn('x', TEAM)).toThrow(TypeError);
  });
});
```

- [ ] **Step 2: Give the app a `server-only` stub, alias and dependency, and the `oxc` setting**

`lib/*` opens with `import 'server-only'`, whose default export throws. Create `ts/apps/iam-console/tests/support/server-only-stub.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// A test-only stand-in for `server-only` (vitest.config.ts `resolve.alias`). The real package's
// default export is an unconditional throw. The `react-server` condition that switches it off also
// switches `react` to its server build, which breaks next/navigation (measured in @paigasus/auth;
// see its vitest.config.ts). Same pattern as ts/packages/paigasus-auth/tests/support/.
export {};
```

The `oxc` constant below is needed before any test imports `lib/`. vite:oxc loads the NEAREST `tsconfig.json` of each file it transforms. For `lib/prn.ts` that is the app's `tsconfig.json`, which extends the preset through a pnpm symlink that oxc cannot follow. Without the setting, the import of `lib/prn.ts` fails with `[TSCONFIG_ERROR]` (`Tsconfig not found`), and Steps 6, 14C and 15 cannot pass. Task 9 keeps the constant, its comment and the `oxc,` key word for word, in the same places.

Replace the whole of `ts/apps/iam-console/vitest.config.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// `lib/*` opens with `import 'server-only'`, which throws under every condition except
// `react-server` — and that condition breaks next/navigation. So `server-only` is ALIASED to an
// empty stub, as in every @paigasus/* package with a server entry.
//
// The condition list stays ADDITIVE (not just ['node']): dropping `import`/`default` breaks the
// source-exports `.ts` resolution of the @paigasus/* packages. vitest 5 resolves a node test's
// imports through `ssr.resolve.conditions`, so both blocks carry the list (CLAUDE.md, SMA-502).
const conditions = ['node', 'import', 'default'];

const serverOnlyStub = fileURLToPath(new URL('./tests/support/server-only-stub.ts', import.meta.url));

// MEASURED (SMA-511 plan, Task 9): vite:oxc loads the NEAREST tsconfig.json for every file it
// transforms. This app's tsconfig.json extends '@paigasus/next-config/tsconfig-app' through pnpm's
// symlink, which oxc's resolver cannot follow — every import of lib/ or app/ failed with
// "[TSCONFIG_ERROR] Failed to load tsconfig … Tsconfig not found" — and the preset sets
// `jsx: preserve`, which Node cannot run. `tsconfig: false` stops the lookup, and the JSX runtime
// is stated here instead. Vite's OxcOptions type omits `tsconfig`, but the plugin spreads every key
// into rolldown's transformSync (vite 8.0.16), so a non-literal object carries it.
const oxc = { tsconfig: false, jsx: { runtime: 'automatic', importSource: 'react' } } as const;

export default defineConfig({
  oxc,
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts'],
    // Each standalone-runtime case boots a real Next standalone server twice; the default 5s is not enough.
    testTimeout: 120_000,
    hookTimeout: 120_000,
  },
  resolve: { conditions, alias: { 'server-only': serverOnlyStub } },
  ssr: { resolve: { conditions } },
});
```

In `ts/apps/iam-console/package.json`, add `"server-only": "catalog:"` to `dependencies`, after `"react-dom": "catalog:",`. `next build` type-checks every file the app `tsconfig.json` includes, and it bundles `server-only` once a route imports `lib/`.

- [ ] **Step 3: Run the test and see it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts install --no-frozen-lockfile
pnpm -C ts/apps/iam-console exec vitest run tests/unit/prn.test.ts
```

Expected: FAIL — `Failed to resolve import "../../lib/prn"` (the file does not exist).

- [ ] **Step 4: Spike prerequisite — a `./wasm` subpath on the kernel**

In `ts/packages/paigasus-kernel/package.json`, change `exports` to:

```json
  "exports": {
    ".": {
      "node": "./src/index.ts",
      "browser": "./src/wasm.ts",
      "default": "./src/wasm.ts"
    },
    "./wasm": "./src/wasm.ts"
  },
```

and append this sentence to the end of the `_comment_exports` string: ` ./wasm → src/wasm.ts for a Node server that must not load the napi binary (SMA-511 D6).`

In `ts/apps/iam-console/package.json`, add `"@paigasus/kernel": "workspace:*"` to `dependencies` (first entry, alphabetical) and `"vite-plugin-wasm": "catalog:"` to `devDependencies`. Then:

```bash
moon run paigasus-kernel-ts:build
pnpm -C ts install --no-frozen-lockfile
```

Expected: the kernel build passes and writes `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm`; the install adds the two dependencies to `ts/pnpm-lock.yaml`.

- [ ] **Step 5: Write `lib/prn.ts` on the kernel wasm binding**

Create `ts/apps/iam-console/lib/prn.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// IAM tenancy PRNs for the console (SMA-511 spec § 4.7, decision D6). PRN logic belongs to
// paigasus-kernel (ADR-0005). The kernel's napi binding cannot load in a Next build (spec § 13 row 5),
// so this module reads PRNs through the kernel's WASM binding (`@paigasus/kernel/wasm`), which is
// platform-neutral and needs no native file in the image. tests/unit/prn.test.ts replays the kernel
// parity corpus through it.
//
// The tenancy rule is IAM's (rs/crates/libs/paigasus-iam-core/src/tenancy.rs `check`): service
// `iam`; an organization has NO org field; a team and a project have one.
import 'server-only';
import { prnBuild, prnErrorKind, prnOrg, prnResourceId, prnResourceType, prnService } from '@paigasus/kernel/wasm';

export type TenancyKind = 'organization' | 'team' | 'project';
export type TenancyRef = { kind: TenancyKind; orgId: string; id: string };

/** The canonical PRN of IAM's synthetic Cedar Root: `root_prn()` in paigasus-iam-core authz/model.rs. */
export const ROOT_PRN = 'prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000';

/** The kernel's UUID field form: 36 characters, hyphenated, either case (resource_name.rs `parse_uuid_field`). */
const UUID = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

export function isUuid(value: string): boolean {
  return UUID.test(value);
}

function requireUuid(label: string, value: string): string {
  if (!isUuid(value)) throw new TypeError(`${label} must be a UUID`);
  return value;
}

/** The tenancy node a PRN names, or null for any other resource or an invalid PRN. Ids are lower-case. */
export function parseTenancyPrn(prn: string): TenancyRef | null {
  if (prnErrorKind(prn) !== '') return null;
  if (prnService(prn) !== 'iam') return null;
  const type = prnResourceType(prn);
  const org = prnOrg(prn);
  const id = prnResourceId(prn);
  if (type === 'organization') return org === '' ? { kind: 'organization', orgId: id, id } : null;
  if (type === 'team' || type === 'project') return org === '' ? null : { kind: type, orgId: org, id };
  return null;
}

export function organizationPrn(orgId: string): string {
  return prnBuild('iam', '', '', 'organization', requireUuid('orgId', orgId));
}

export function teamPrn(orgId: string, teamId: string): string {
  return prnBuild('iam', '', requireUuid('orgId', orgId), 'team', requireUuid('teamId', teamId));
}

export function projectPrn(orgId: string, projectId: string): string {
  return prnBuild('iam', '', requireUuid('orgId', orgId), 'project', requireUuid('projectId', projectId));
}
```

Make vitest load the `.wasm`, the same way the kernel's own browser project does (`ts/packages/paigasus-kernel/vitest.config.ts`). In `ts/apps/iam-console/vitest.config.ts`, change the imports to:

```ts
import { fileURLToPath } from 'node:url';
import wasm from 'vite-plugin-wasm';
import { defineConfig, type Plugin } from 'vitest/config';
```

add, after the `serverOnlyStub` constant (before the `oxc` comment):

```ts
// WASM BRANCH (SMA-511 D6). @paigasus/wasm is aliased to the crate-dir glue, which
// paigasus-kernel-ts:build writes, NOT to pnpm's installed `file:` copy, which a fresh install
// leaves without the gitignored .wasm. vite-plugin-wasm instantiates the bundler-target .wasm.
const wasmGlue = fileURLToPath(new URL('../../../rs/crates/bindings/paigasus-wasm/paigasus_wasm.js', import.meta.url));
```

and change the config object's `resolve` and `ssr` lines (below `test`; the `oxc,` key stays first) to:

```ts
  // vite-plugin-wasm's factory is typed `() => any`; cast to vite's Plugin as the kernel does.
  plugins: [wasm() as Plugin],
  resolve: { conditions, alias: { 'server-only': serverOnlyStub, '@paigasus/wasm': wasmGlue } },
  ssr: { resolve: { conditions } },
```

- [ ] **Step 6: Run the test against the wasm implementation**

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/prn.test.ts
```

Expected: PASS (54 cases). This proves the WASM implementation is correct in vitest. It does not prove that Turbopack can bundle it; Steps 7–10 measure that.

- [ ] **Step 7: Spike B1 — add a temporary probe route**

Create `ts/apps/iam-console/app/prn-probe/route.ts`. It is TEMPORARY: Step 11 deletes it in both branches.

```ts
// SPDX-License-Identifier: Apache-2.0
//
// TEMPORARY — SMA-511 Task 8 spike (D6). Proves that Turbopack bundles the kernel wasm into the
// standalone server and that the functions return correct values at REQUEST time. Deleted by the
// decision step of the same task.
import { prnBuild, prnOrg, prnResourceId, prnResourceType } from '@paigasus/kernel/wasm';

// Request time, not build time: a static route handler would run once in `next build`.
export const dynamic = 'force-dynamic';

export function GET(): Response {
  const team = prnBuild('iam', '', '0190a100-0000-7000-8000-0000000000aa', 'team', '0190a1b2-0000-7000-8000-000000000001');
  return Response.json({
    team,
    org: prnOrg(team),
    type: prnResourceType(team),
    id: prnResourceId(team),
    root: prnBuild('iam', '', '', 'root', '00000000-0000-0000-0000-000000000000'),
  });
}
```

**The pass criterion (spec D6, in its own words).** "The spike measures that Turbopack bundles the wasm into the standalone server and that `prnBuild`, `prnOrg`, `prnResourceType` and `prnResourceId` return correct values from a route handler." The probe of Step 7 calls the four functions in a route handler, and the standalone server answers it. So a correct answer from the standalone server proves both halves, and the probe result decides. B1 measures this in a warm developer tree. B2 measures the same thing in the CI state, because CI and the image build from that state. The `.wasm` files under `.next/standalone` are recorded for the spec, but they do not decide: Turbopack can put the module into a JS chunk, and then no `.wasm` file exists.

**Run Steps 8–10 as scripts.** These steps use command substitution and shell variables (`W=$(…)`, `$SERVER_PID`, `$SCRATCH`). The worktree sandbox refuses such commands when you type them. Write each step's commands into a script file in your scratchpad, and run it with `/bin/bash <script>`. Start every script with this header:

```bash
# The script lives in the scratchpad, so its own directory is the scratchpad.
SCRATCH="$(cd "$(dirname "$0")" && pwd)"
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console
```

- [ ] **Step 8: Spike B1 — make sure the installed copy has the `.wasm` (a warm developer tree)**

Script `$SCRATCH/spike-step8.sh` (the header, then):

```bash
W=$(ls -d ts/node_modules/.pnpm/@paigasus+wasm@file*/node_modules/@paigasus/wasm)
echo "$W"
ls -li rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm "$W/paigasus_wasm_bg.wasm"
```

Expected: one directory path; both files exist, with the SAME inode number (pnpm hard-linked them at install). If the second file is missing or has another inode, add `ln -f rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm "$W/paigasus_wasm_bg.wasm"` above the `ls -li` line and run the script again. Do not use `cp`: a copy gets its own inode, so a later `paigasus-kernel-ts:build` does not update it, and the installed copy goes stale.

- [ ] **Step 9: Spike B1 — build, start the standalone server, and call the probe**

Script `$SCRATCH/spike-probe.sh` (the header, then the lines below). It takes the label `B1` or `B2`, because Step 10 runs it again. The server runs in the background, and the same script stops it (macOS has no `timeout`; curl's own retry waits for the port).

```bash
LABEL="${1:?usage: /bin/bash spike-probe.sh B1|B2}"
cd ts/apps/iam-console
rm -rf .next
pnpm exec next build > "$SCRATCH/spike-$LABEL-build.log" 2>&1
echo "$LABEL build exit $?"
test -f .next/standalone/apps/iam-console/server.js && echo "$LABEL standalone entry ok"
echo "$LABEL .wasm files under .next/standalone (recorded, not a criterion):"
find .next/standalone -name '*.wasm' | head -5
PORT=3917 HOSTNAME=127.0.0.1 PAIGASUS_ZONE=iam PAIGASUS_ZONES='{"iam":"/iam"}' node .next/standalone/apps/iam-console/server.js > "$SCRATCH/spike-$LABEL-server.log" 2>&1 &
SERVER_PID=$!
curl -fsS --retry 60 --retry-delay 1 --retry-connrefused http://127.0.0.1:3917/iam/prn-probe > "$SCRATCH/spike-$LABEL.json"
echo "$LABEL curl exit $?"
kill "$SERVER_PID"
cd ../../..
node -e '
const got = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
const label = process.argv[2];
const want = {
  team: "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:team/0190a1b2-0000-7000-8000-000000000001",
  org: "0190a100-0000-7000-8000-0000000000aa",
  type: "team",
  id: "0190a1b2-0000-7000-8000-000000000001",
  root: "prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000",
};
const ok = JSON.stringify(got) === JSON.stringify(want);
console.log(ok ? `${label} MATCH` : `${label} MISMATCH`, JSON.stringify(got));
process.exit(ok ? 0 : 1);
' "$SCRATCH/spike-$LABEL.json" "$LABEL"
```

Run `/bin/bash "$SCRATCH/spike-probe.sh" B1` (type the literal scratchpad path in place of `$SCRATCH`).

B1 PASSES only if all of these hold: `B1 build exit 0`, `B1 standalone entry ok`, `B1 curl exit 0`, and `B1 MATCH`. Copy the `find` output (the `.wasm` paths, or none) for Step 12. On any other result B1 FAILS: keep the first error line of `spike-B1-build.log` or `spike-B1-server.log` for Step 12, and go to Step 11 (skip Step 10).

- [ ] **Step 10: Spike B2 — the CI state (only if B1 passed)**

A CI runner installs before it builds the kernel, so its installed `@paigasus/wasm` copy has no `.wasm`. Reproduce that: remove the hard link in the installed copy only (the crate-dir file stays), then run the same probe again. Script `$SCRATCH/spike-step10.sh` (the header, then):

```bash
W=$(ls -d ts/node_modules/.pnpm/@paigasus+wasm@file*/node_modules/@paigasus/wasm)
rm "$W/paigasus_wasm_bg.wasm"
ls rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm
test -e "$W/paigasus_wasm_bg.wasm" || echo "B2 installed copy has no .wasm (the CI state)"
/bin/bash "$SCRATCH/spike-probe.sh" B2
# Restore the HARD LINK, not a copy: a copy gets its own inode, and a later kernel build would not
# update it. The ls -li must show one inode number for both paths.
ln -f rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm "$W/paigasus_wasm_bg.wasm"
ls -li rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm "$W/paigasus_wasm_bg.wasm"
```

Run `/bin/bash "$SCRATCH/spike-step10.sh"` (with the literal scratchpad path). B2 PASSES only if all of these hold: `B2 build exit 0`, `B2 standalone entry ok`, `B2 curl exit 0`, and `B2 MATCH`. Copy the `find` output for Step 12. If B2 fails, keep the first error line of `spike-B2-build.log` or `spike-B2-server.log` for Step 12 (the expected build failure is a `Module not found` for `./paigasus_wasm_bg.wasm`). The `ln -f` line restores the local tree in both cases; check that the last `ls -li` shows the same inode twice.

- [ ] **Step 11: Delete the probe (both branches)**

```bash
rm -r ts/apps/iam-console/app/prn-probe
rm -rf ts/apps/iam-console/.next
git status --short -- ts/apps/iam-console/app
```

Expected: `git status` prints nothing for `app/`.

- [ ] **Step 12: Decide, and record the result in the spec**

The spike PASSES only if B1 AND B2 passed. Write the result into `docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md`. After the numbered list that ends with item 4 (`A Server Action POST under the basePath through the TLS terminator …`, end of § 13, before `## 14. Challenge log`), insert:

```markdown

**Measured during implementation:**

| # | Result |
|---|---|
| 10 | D6 wasm spike (plan Task 8). Criterion (D6): the standalone server answers a route handler that calls `prnBuild`, `prnOrg`, `prnResourceType` and `prnResourceId` with the correct values. B1, a warm tree: <PASS, or FAIL with the first error line>; `.wasm` files under `.next/standalone`: <the B1 `find` output, or "none">. B2, the CI state (the installed `file:` copy of `@paigasus/wasm` without the gitignored `.wasm`): <PASS, FAIL with the first error line, or "not run, B1 failed">; `.wasm` files under `.next/standalone`: <the B2 `find` output, "none", or "not run">. Decision: <the kernel wasm binding, or fallback C>. |
```

Fill the five angle-bracket slots with what Steps 9 and 10 printed. Then:

- If the spike PASSED, do Steps 13W–14W and skip Steps 13C–15C.
- If the spike FAILED, do Steps 13C–15C and skip Steps 13W–14W. Do not ask again (decision D6).

- [ ] **Step 13W (wasm branch): Moon wiring for the kernel**

In `ts/apps/iam-console/moon.yml`, change `dependsOn` (lines 11–13) to:

```yaml
dependsOn:
  - 'paigasus-next-config-ts'
  - 'paigasus-ui-ts'
  # SMA-511 D6: lib/prn.ts reads PRNs through @paigasus/kernel/wasm. The kernel's build writes the
  # gitignored paigasus_wasm_bg.wasm that next build bundles and vitest loads.
  - 'paigasus-kernel-ts'
```

In the `build` task, add `deps: ['^:build']` directly under `script: |…` (before `inputs:`), and add these lines to `build.inputs` after `'/ts/packages/paigasus-ui/package.json'`:

```yaml
      # SMA-511 D6: next build bundles the kernel's wasm entry and the committed glue.
      - '/ts/packages/paigasus-kernel/src/**/*'
      - '/ts/packages/paigasus-kernel/package.json'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm.js'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js'
      - '/rs/crates/bindings/paigasus-wasm/src/**/*'
      - '/rs/crates/libs/paigasus-kernel/src/**/*'
```

Add the same six lines to `test.inputs`.

Then re-baseline the five strict-equality cases in `ci/affected-graph/run.sh` that these edges change. Without this, Step 15's suite reds five cases in this branch. The reasons:

- `run_case` follows `dependsOn` with `--downstream deep` (`run.sh:31-35`). The app now `dependsOn` `paigasus-kernel-ts`, which depends on both binding crates. So `kernel->bindings`, `binding-oneway-node` and `binding-oneway-wasm` each gain the project `iam-console-ts`.
- `kernel->consumer-tasks` (no flags) anchors on `rs/crates/libs/paigasus-kernel/src/lib.rs`. The new `build` and `test` input `/rs/crates/libs/paigasus-kernel/src/**/*` matches it, so the case gains `iam-console-ts:build,iam-console-ts:test`.
- `lockfile->all-lint` (`--downstream deep`, which follows task `deps`) selects `paigasus-kernel-ts:build`. The app's `build` reaches it through `^:build`, and the app's `test` through `~:build`, so the case gains `iam-console-ts:build,iam-console-ts:test`.

Make these edits in `ci/affected-graph/run.sh`. Each one inserts ONE comment line directly above the case's `run_case` or `run_task_case*` line, keeps that line, and appends to the expected CSV. The line numbers are from before the edits; match on the text. Do not reorder any other line.

`kernel->bindings` (lines 295–296) becomes:

```bash
  # + iam-console-ts (SMA-511, D6 wasm branch), through paigasus-kernel-ts.
  run_case "kernel->bindings" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs,paigasus-kernel-py,paigasus-node-bindings-rs,paigasus-kernel-ts,paigasus-wasm-rs,paigasus-kernel-parity-rs,paigasus-iam-core-rs,paigasus-iam-rs,paigasus-observability-rs,iam-console-ts"
```

`binding-oneway-node` (lines 304–305) becomes:

```bash
  # + iam-console-ts (SMA-511, D6 wasm branch), through paigasus-kernel-ts.
  run_case "binding-oneway-node" "rs/crates/bindings/paigasus-node-bindings/src/lib.rs" \
    "paigasus-node-bindings-rs,paigasus-kernel-ts,iam-console-ts"
```

`binding-oneway-wasm` (lines 308–309) becomes:

```bash
  # + iam-console-ts (SMA-511, D6 wasm branch), through paigasus-kernel-ts.
  run_case "binding-oneway-wasm" "rs/crates/bindings/paigasus-wasm/src/lib.rs" \
    "paigasus-wasm-rs,paigasus-kernel-ts,iam-console-ts"
```

`lockfile->all-lint` (lines 354–355) becomes:

```bash
  # + iam-console-ts:{build,test} (SMA-511, D6 wasm branch), through task deps ('^:build', '~:build') on paigasus-kernel-ts:build, not through inputs.
  run_task_case "lockfile->all-lint" "rs/Cargo.lock" \
    "paigasus-gateway-rs:lint,paigasus-iam-core-rs:lint,paigasus-iam-rs:lint,paigasus-kernel-parity-rs:lint,paigasus-kernel-py:test,paigasus-kernel-rs:lint,paigasus-kernel-ts:build,paigasus-kernel-ts:test,paigasus-logging-rs:lint,paigasus-node-bindings-rs:lint,paigasus-observability-rs:lint,paigasus-proto-derive-rs:lint,paigasus-proto-rs:lint,paigasus-py-bindings-rs:lint,paigasus-service-info-rs:lint,paigasus-wasm-rs:lint,iam-console-ts:build,iam-console-ts:test"
```

Its `-ci` twin (`lockfile->all-lint-ci`, lines 359–360) keeps its CSV: the two app tasks do not key on `rs/Cargo.lock`. Its comment says the twin equals the deep set, which is now false in this branch. Insert this line directly above `run_task_case_ci "lockfile->all-lint-ci"`:

```bash
  # SMA-511 (D6 wasm branch): no longer equal — the deep set also holds iam-console-ts:{build,test}, which arrive through task deps.
```

`kernel->consumer-tasks` (lines 367–368) becomes:

```bash
  # + iam-console-ts:{build,test} (SMA-511, D6 wasm branch): both tasks list '/rs/crates/libs/paigasus-kernel/src/**/*' as an input.
  run_task_case_ci "kernel->consumer-tasks" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-gateway-rs:build,paigasus-gateway-rs:test,paigasus-gateway-rs:lint,paigasus-iam-core-rs:build,paigasus-iam-core-rs:test,paigasus-iam-core-rs:lint,paigasus-iam-rs:build,paigasus-iam-rs:test,paigasus-iam-rs:lint,paigasus-kernel-parity-rs:build,paigasus-kernel-parity-rs:test,paigasus-kernel-parity-rs:lint,paigasus-node-bindings-rs:build,paigasus-node-bindings-rs:test,paigasus-node-bindings-rs:lint,paigasus-observability-rs:build,paigasus-observability-rs:test,paigasus-observability-rs:lint,paigasus-py-bindings-rs:build,paigasus-py-bindings-rs:test,paigasus-py-bindings-rs:lint,paigasus-wasm-rs:build,paigasus-wasm-rs:test,paigasus-wasm-rs:lint,paigasus-kernel-rs:build,paigasus-kernel-rs:test,paigasus-kernel-rs:lint,paigasus-kernel-ts:build,paigasus-kernel-ts:test,paigasus-kernel-py:test,iam-console-ts:build,iam-console-ts:test"
```

Step 15 runs the suite. If a case still FAILs, do not loosen its expected set. Re-derive each `missing` or `unexpected` row from the real inputs with the two `moon query` commands of Task 23 Step 6, and fix the input or the set with a reason in the case's comment.

Then do Step 15 (shared) and commit with the wasm message in Step 16.

- [ ] **Step 14W (wasm branch): nothing else**

The kernel `./wasm` export, the two app dependencies, the wasm `vitest.config.ts` and the wasm `lib/prn.ts` stay as Steps 4–5 wrote them. Go to Step 15.

- [ ] **Step 13C (fallback branch): remove the spike's wiring**

In `ts/packages/paigasus-kernel/package.json`, remove the `"./wasm": "./src/wasm.ts"` line (and the comma before it) and the appended ` ./wasm → …` sentence, with Edit. Do not use `git checkout --` on a file that may hold other work. Check:

```bash
git diff --exit-code -- ts/packages/paigasus-kernel/package.json; echo "kernel package.json unchanged: exit $?"
```

Expected: `exit 0`. The spec says the `./wasm` export is NOT added on this branch.

In `ts/apps/iam-console/package.json`, remove `"@paigasus/kernel": "workspace:*"` from `dependencies` and `"vite-plugin-wasm": "catalog:"` from `devDependencies`. Keep `"server-only": "catalog:"`.

In `ts/apps/iam-console/vitest.config.ts`, remove the `vite-plugin-wasm` import, the `type Plugin` import, the `wasmGlue` constant with its comment, the `plugins` line with its comment, and the `'@paigasus/wasm': wasmGlue` alias entry. The file is then exactly the Step 2 version.

```bash
pnpm -C ts install --no-frozen-lockfile
```

- [ ] **Step 14C (fallback branch): write the fallback reader**

Replace the whole of `ts/apps/iam-console/lib/prn.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// IAM tenancy PRNs for the console (SMA-511 spec § 4.7, decision D6, fallback C).
//
// A RECORDED ADR-0005 EXCEPTION (spec § 12). PRN logic belongs to paigasus-kernel. The kernel's napi
// binding cannot load in a Next build (spec § 13 row 5), and the wasm spike did not pass (spec § 13
// row 10). So this module holds a small reader for the THREE tenancy shapes only. It is held to the
// kernel by tests/unit/prn.test.ts, which runs every vector of the kernel parity corpus
// (rs/crates/libs/paigasus-kernel-parity/vectors/) through it: a divergence from the kernel fails CI.
//
// The grammar is the kernel's (rs/crates/libs/paigasus-kernel/src/resource_name.rs):
//   prn:pgs:<service>:<region>:<org>:<resource-type>/<resource-id>
// The tenancy rule is IAM's (rs/crates/libs/paigasus-iam-core/src/tenancy.rs `check`): service
// `iam`; an organization has NO org field (its org id is its own id); a team and a project have one.
import 'server-only';

export type TenancyKind = 'organization' | 'team' | 'project';
export type TenancyRef = { kind: TenancyKind; orgId: string; id: string };

/** The canonical PRN of IAM's synthetic Cedar Root: `root_prn()` in paigasus-iam-core authz/model.rs. */
export const ROOT_PRN = 'prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000';

/**
 * The kernel's MAX_LEN is 512 BYTES. This compares UTF-16 units; the two differ only for non-ASCII
 * input, which no valid field can hold, so such a string is rejected either way.
 */
const MAX_LEN = 512;

/** The kernel's UUID field form: 36 characters, hyphenated, either case (resource_name.rs `parse_uuid_field`). */
const UUID = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

/** resource_name.rs `is_valid_region`: lower-case alphanumeric segments joined by single hyphens. */
const REGION = /^[a-z0-9]+(-[a-z0-9]+)*$/;

const TENANCY_KINDS: ReadonlySet<string> = new Set<TenancyKind>(['organization', 'team', 'project']);

export function isUuid(value: string): boolean {
  return UUID.test(value);
}

function requireUuid(label: string, value: string): string {
  if (!isUuid(value)) throw new TypeError(`${label} must be a UUID`);
  return value.toLowerCase();
}

/** The tenancy node a PRN names, or null for any other resource or an invalid PRN. Ids are lower-case. */
export function parseTenancyPrn(prn: string): TenancyRef | null {
  if (prn.length === 0 || prn.length > MAX_LEN) return null;
  const parts = prn.split(':');
  if (parts.length !== 6) return null;
  const [scheme, partition, service, region, org, path] = parts as [string, string, string, string, string, string];
  // `iam` is a valid kernel service label, so checking equality also checks the label grammar.
  if (scheme !== 'prn' || partition !== 'pgs' || service !== 'iam') return null;
  if (region !== '' && !REGION.test(region)) return null;
  if (org !== '' && !isUuid(org)) return null;
  const slash = path.indexOf('/');
  if (slash === -1 || path.includes('/', slash + 1)) return null;
  const type = path.slice(0, slash);
  const id = path.slice(slash + 1);
  // The three tenancy names are valid kernel resource-type labels, so membership also checks the grammar.
  if (!TENANCY_KINDS.has(type) || !isUuid(id)) return null;
  const kind = type as TenancyKind;
  const resourceId = id.toLowerCase();
  if (kind === 'organization') return org === '' ? { kind, orgId: resourceId, id: resourceId } : null;
  return org === '' ? null : { kind, orgId: org.toLowerCase(), id: resourceId };
}

export function organizationPrn(orgId: string): string {
  return `prn:pgs:iam:::organization/${requireUuid('orgId', orgId)}`;
}

export function teamPrn(orgId: string, teamId: string): string {
  return `prn:pgs:iam::${requireUuid('orgId', orgId)}:team/${requireUuid('teamId', teamId)}`;
}

export function projectPrn(orgId: string, projectId: string): string {
  return `prn:pgs:iam::${requireUuid('orgId', orgId)}:project/${requireUuid('projectId', projectId)}`;
}
```

Run:

```bash
pnpm -C ts/apps/iam-console exec vitest run tests/unit/prn.test.ts
```

Expected: PASS (54 cases) — the same suite that passed against the kernel wasm binding in Step 6.

- [ ] **Step 15C (fallback branch): nothing else in this branch**

Go to Step 15.

- [ ] **Step 15: Moon inputs (both branches)**

In `ts/apps/iam-console/moon.yml`, change `fileGroups.sources` (lines 27–29) to:

```yaml
fileGroups:
  sources:
    - 'app/**/*'
    # SMA-511: the composition root. Without this, an edit in lib/ serves a cached build and test.
    - 'lib/**/*'
```

In `test.inputs` (the list at lines 139–155), after `'/ts/tsconfig.base.json'`, add:

```yaml
      # SMA-511 D6: tests/unit/prn.test.ts replays the kernel parity corpus and pins ROOT_PRN to
      # root_prn()'s builder call. Without these, a corpus or model.rs edit serves a cached PASS.
      - '/rs/crates/libs/paigasus-kernel-parity/vectors/**/*'
      - '/rs/crates/libs/paigasus-iam-core/src/authz/model.rs'
```

In `ts/moon.yml`, change `fileGroups.sources` (lines 11–15) to:

```yaml
  sources:
    - 'packages/*/src/**/*'
    - 'apps/*/src/**/*'
    - 'apps/*/app/**/*'
    # SMA-511: an app's composition root. Without this, ts:lint does not key on apps/*/lib and an edit
    # there serves a cached lint pass.
    - 'apps/*/lib/**/*'
    - 'tooling/**/*' # SMA-406 parity helpers; lint/fmt lint the whole tree, so the cache must track these too
```

Then run everything. The `prettier --write` line comes first and lists every `ts/` file this task creates or edits, in both branches (the Global Constraints). Prettier skips `pnpm-lock.yaml` through `.prettierignore`, and an unchanged file stays unchanged.

```bash
pnpm -C ts exec prettier --write apps/iam-console/lib/prn.ts apps/iam-console/tests/unit/prn.test.ts apps/iam-console/tests/support/server-only-stub.ts apps/iam-console/vitest.config.ts apps/iam-console/package.json apps/iam-console/moon.yml moon.yml packages/paigasus-kernel/package.json pnpm-lock.yaml
moon run iam-console-ts:typecheck iam-console-ts:build iam-console-ts:test
moon run ts:lint ts:fmt --force
/bin/bash ci/affected-graph/run.sh
```

Expected: all pass; `iam-console-ts:test` runs the prn suite (54), the standalone-runtime suite (2) and the three Tailwind guard modes; the affected-graph suite ends with `== affected-graph cascade intact ==` in both branches. In the fallback branch no case changes. In the wasm branch the suite is green only because Step 13W re-baselined the five cases, and `iam-console-ts:build` now also runs `paigasus-kernel-ts:build` first (`^:build`).

- [ ] **Step 16: Commit**

Fallback branch:

```bash
git add ts/apps/iam-console/lib/prn.ts ts/apps/iam-console/tests/unit/prn.test.ts ts/apps/iam-console/tests/support/server-only-stub.ts \
  ts/apps/iam-console/vitest.config.ts ts/apps/iam-console/package.json ts/apps/iam-console/moon.yml ts/moon.yml ts/pnpm-lock.yaml \
  docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md
git status --short
git commit -F - <<'EOF'
feat(ts): add the tenancy prn reader to iam-console (SMA-511)

Decision D6: the kernel's wasm binding was spiked on the server first and did not pass (spec § 13
row 10 records why), so lib/prn.ts holds a small reader for the three IAM tenancy shapes only. It
is a recorded ADR-0005 exception, held to the kernel by a test that replays every vector of the
kernel parity corpus. ROOT_PRN is pinned to root_prn()'s builder call in paigasus-iam-core.

The app gains a server-only stub alias for vitest, and lib/ joins the app's and the ts workspace's
Moon sources, so an edit there no longer serves a cached build, test or lint.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

Expected: `git status --short` before the commit shows no change under `ts/packages/paigasus-kernel/`, no change to `ci/affected-graph/run.sh`, and no `app/prn-probe/`.

Wasm branch: add `ts/packages/paigasus-kernel/package.json` and `ci/affected-graph/run.sh` to the `git add` list above, and use this message instead:

```text
feat(ts): read prns through the kernel wasm binding in iam-console (SMA-511)

Decision D6: @paigasus/kernel gains a ./wasm subpath, and lib/prn.ts reads IAM tenancy PRNs
through it. The spike measured that Turbopack bundles the wasm into the standalone server, and
that the values are correct at request time, also from a fresh install (spec § 13 row 10). The
app depends on the kernel's build, and a test replays the kernel parity corpus. Five
affected-graph cases gain the app, which now depends on paigasus-kernel-ts.

The app gains a server-only stub alias for vitest, and lib/ joins the app's and the ts workspace's
Moon sources, so an edit there no longer serves a cached build, test or lint.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
```

> Handoff to Task 9: After Task 8 these exist in `ts/apps/iam-console/`: `vitest.config.ts` with `environment: 'node'`, `include: ['tests/**/*.test.ts']`, 120 s timeouts, `resolve.conditions` and `ssr.resolve.conditions` = `['node', 'import', 'default']`, `resolve.alias['server-only']` → `tests/support/server-only-stub.ts`, and the `oxc` constant (`{ tsconfig: false, jsx: { runtime: 'automatic', importSource: 'react' } }`, with its MEASURED comment) plus the `oxc,` key before `test:`, in the places Task 9's final file has them (wasm branch only: `vite-plugin-wasm` and an `@paigasus/wasm` alias to the crate glue). Task 9 keeps the `oxc` lines and does not add them again. Task 9 ADDS `__NEXT_EXPERIMENTAL_AUTH_INTERRUPTS=true` (in `test.env`), the `next/headers`/`next/cache` aliases to test doubles, a `setupFiles` entry and the `msw` dependency (Task 11 writes the MSW handlers; each test file starts its own MSW server); it keeps the alias and the conditions. `tests/support/server-only-stub.ts`, `tests/unit/prn.test.ts` and `lib/prn.ts` exist. `package.json` has `server-only` in `dependencies` (wasm branch: also `@paigasus/kernel` and dev `vite-plugin-wasm`). `moon.yml` has `lib/**/*` in `fileGroups.sources` and, in `test.inputs`, the kernel parity vectors and `paigasus-iam-core/src/authz/model.rs`; Task 9 must not add `lib/**/*` again. Task 10 (not Task 9) adds `proxy.ts` to both `sources` groups when it creates the file: the app's, and `ts/moon.yml`'s as `'apps/*/proxy.ts'`. `ts/moon.yml` already lists `apps/*/lib/**/*`. From Task 5, `authRoutePaths()` takes no argument and `proxy.ts` must pass basePath-RELATIVE paths: `createAuthMiddleware({ publicPaths: [...authRoutePaths(), '/', '/healthz'], loginPath: '/auth/login' })`; any hand-built `AuthRuntime` needs `publicOrigin`. From Task 3, no `next build` has yet compiled `@paigasus/auth/server`, `@paigasus/sdk` or `@paigasus/discovery/server`: the first build that imports them (Task 10) is the real Turbopack proof. For Task 23: a corpus or `model.rs` edit now selects `iam-console-ts:test`; no existing affected-graph case anchors there, so these inputs change no expected set, but a new case could pin it. In the fallback branch, Task 8 changed no case in `ci/affected-graph/run.sh`. In the wasm branch, Step 13W appended `,iam-console-ts` to `kernel->bindings`, `binding-oneway-node` and `binding-oneway-wasm`, and `,iam-console-ts:build,iam-console-ts:test` to `kernel->consumer-tasks` and `lockfile->all-lint`; Task 23 adds only `,iam-console-ts:test-e2e` to those two task cases. For Task 24: Task 1 already renamed the paths in `CLAUDE.md`.


---

### Task 9: App dependencies, runtime config, logger and the vitest setup

This task gives the app its dependencies, its one runtime configuration (spec § 4.1), its logger adapter (spec § 4.8) and a vitest setup that can import `lib/` and `app/`. It starts from what Task 8 left: `vitest.config.ts` with the `server-only` alias, the additive conditions and the `oxc` constant (`tsconfig: false`), `tests/support/server-only-stub.ts`, `lib/prn.ts`, `server-only` in the app's dependencies, and `lib/**/*` in both Moon `sources` groups. The app still serves the old page after this task; Task 10 replaces it.

**Files:**
- Modify: `ts/pnpm-workspace.yaml` (catalog entry `msw`, `allowBuilds.msw: false`)
- Modify: `ts/apps/iam-console/package.json`
- Modify: `ts/pnpm-lock.yaml` (written by `pnpm install`)
- Modify: `ts/apps/iam-console/vitest.config.ts` (Task 8's file, extended)
- Modify: `ts/apps/iam-console/tests/tsconfig.json`
- Create: `ts/apps/iam-console/tests/support/next-headers.ts`
- Create: `ts/apps/iam-console/tests/support/next-cache.ts`
- Create: `ts/apps/iam-console/tests/support/setup.ts`
- Create: `ts/apps/iam-console/tests/support/env.ts`
- Create: `ts/apps/iam-console/lib/config.ts`
- Create: `ts/apps/iam-console/lib/logger.ts`
- Test: `ts/apps/iam-console/tests/unit/config.test.ts`
- Test: `ts/apps/iam-console/tests/unit/logger.test.ts`
- Modify: `ts/apps/iam-console/moon.yml` (`build`, `typecheck`, `test` inputs)
- Modify: `ts/apps/iam-console/.env.local.example`

**Interfaces:**
- Consumes: `authEnvShape` (`@paigasus/auth/server`, `ts/packages/paigasus-auth/src/config.ts:39`), `discoveryEnvShape` (`@paigasus/discovery/server`, `src/config.ts:79`), `defineRuntimeConfig` (`@paigasus/next-config/runtime`, `src/runtime.ts:211`), types `AuthLogger`/`AuthEventName`/`AuthEventFields` and `DiscoveryLogger`/`DiscoveryEventName`/`DiscoveryEventFields`.
- Consumes (Task 8): `ts/apps/iam-console/vitest.config.ts`, `tests/support/server-only-stub.ts`.
- Produces: `lib/config.ts` → `getRuntimeConfig`, `getPublicConfig`, `type ConsoleConfig`. `lib/logger.ts` → `type AppEventName`, `type AppEventFields`, `interface ConsoleLogger`, `createJsonLogger(write?)`, `logger`. Test support → `stubConsoleEnv(overrides)`, `VALID_ENV`, `setRequestHeaders`, `setRequestCookies`, `resetNextHeaders`, `revalidatedPaths`, `resetNextCache`.

- [ ] **Step 1: Confirm the msw release and its install script**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
npm view msw@2.15.0 version scripts.postinstall
npm view msw time --json | grep '"2.15.0"'
```

Expected: `version = '2.15.0'`, `scripts.postinstall = node -e "import('./config/scripts/postinstall.js').catch(() => void 0)"`, and `"2.15.0": "2026-07-08T01:43:07.502Z"`. The date is more than 24 hours before today, so pnpm's `minimumReleaseAge` accepts it. The postinstall script copies a browser service worker, which the node tests do not use, so `allowBuilds` gets `msw: false`.

- [ ] **Step 2: Add msw to the pnpm catalog and to `allowBuilds`**

In `ts/pnpm-workspace.yaml`, in the `allowBuilds:` map, replace:

```yaml
  cpu-features: false
  protobufjs: false
```

with:

```yaml
  cpu-features: false
  # msw (SMA-511) declares a postinstall that copies its BROWSER service worker into a public
  # directory. The iam-console tests use msw/node only, so the script has nothing to do.
  msw: false
  protobufjs: false
```

At the end of the `catalog:` map, after the `'axe-core': ^4.13.0` line, add:

```yaml
  # Request interception for the iam-console vitest tier (SMA-511 spec § 9, AC 5). It intercepts
  # `fetch` — @paigasus/discovery's `GET /v1/service-info` probe. It cannot intercept the SDK's gRPC
  # calls over node:http2; an in-process fake server answers those. 2.15.0 was released 2026-07-08.
  # Pinned EXACTLY, not a caret range (the Global Constraints): a later 2.x must not install silently.
  msw: 2.15.0
```

- [ ] **Step 3: Add the app's dependencies**

In `ts/apps/iam-console/package.json`, make `dependencies` and `devDependencies` hold exactly these keys, in this order. Keep every other key of the file. Task 8 already added `server-only`. In Task 8's wasm branch it also added `@paigasus/kernel` (dependencies) and `vite-plugin-wasm` (devDependencies): keep both, in alphabetical order.

```json
  "dependencies": {
    "@bufbuild/protobuf": "catalog:",
    "@connectrpc/connect": "catalog:",
    "@paigasus/app-shell": "workspace:*",
    "@paigasus/auth": "workspace:*",
    "@paigasus/discovery": "workspace:*",
    "@paigasus/next-config": "workspace:*",
    "@paigasus/sdk": "workspace:*",
    "@paigasus/ui": "workspace:*",
    "@tailwindcss/postcss": "catalog:",
    "next": "catalog:",
    "react": "catalog:",
    "react-dom": "catalog:",
    "redis": "catalog:",
    "server-only": "catalog:",
    "tailwindcss": "catalog:",
    "zod": "catalog:"
  },
  "devDependencies": {
    "@connectrpc/connect-node": "catalog:",
    "@paigasus/proto": "workspace:*",
    "@playwright/test": "catalog:",
    "@types/node": "catalog:",
    "@types/react": "catalog:",
    "@types/react-dom": "catalog:",
    "jose": "catalog:",
    "msw": "catalog:",
    "typescript": "catalog:",
    "vitest": "catalog:"
  }
```

`@paigasus/proto` and `@connectrpc/connect-node` are dev dependencies: only the test doubles in `tests/support/` import them (Task 11). The `apps` boundary rule still bans `@paigasus/proto` everywhere else in the app (Task 7). `@bufbuild/protobuf` is a runtime dependency. The test doubles import it, and `lib/iam-clients.ts` (Task 12) imports a type from it. A type that `lib/` imports must come from a runtime dependency.

- [ ] **Step 4: Install and check the lockfile**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts install --no-frozen-lockfile
grep -n "^  msw@" ts/pnpm-lock.yaml
pnpm -C ts install --frozen-lockfile
```

Expected: the first install ends without `ERR_PNPM_IGNORED_BUILDS`. `grep` prints the lockfile's `msw@2.15.0` entries (one under `packages:`, one or more under `snapshots:`). No other `msw@` version appears. The frozen install exits 0 and changes nothing.

- [ ] **Step 5: Let the tests' tsconfig cover `.tsx` files**

Replace `ts/apps/iam-console/tests/tsconfig.json` with:

```json
{
  "_comment": "The nearest tsconfig.json for every file under tests/, for ESLint's projectService (ts/eslint.config.js). It extends the workspace base by a plain relative path, the pattern every package uses. SMA-511 adds `**/*.tsx` and the DOM libs and react-jsx, because tests now render components; without `**/*.tsx` a .test.tsx file belongs to no program and `ts:lint` fails on it. vitest no longer reads any tsconfig for its transform (vitest.config.ts sets `oxc.tsconfig: false`), and `tsc -p tsconfig.json --noEmit` reads the parent config, never this file.",
  "extends": "../../../tsconfig.base.json",
  "compilerOptions": {
    "lib": ["DOM", "DOM.Iterable", "ES2022"],
    "jsx": "react-jsx",
    "types": ["node"]
  },
  "include": ["**/*.ts", "**/*.tsx"]
}
```

- [ ] **Step 6: Extend the vitest config and write the test support files**

Task 8 wrote `ts/apps/iam-console/vitest.config.ts`, with the `oxc` constant, its MEASURED comment and the `oxc,` key already in place. Task 9 keeps those lines as they are and does not add them again. Make three edits to the file; they apply in both of Task 8's branches.

Replace:

```ts
const serverOnlyStub = fileURLToPath(new URL('./tests/support/server-only-stub.ts', import.meta.url));
```

with:

```ts
const serverOnlyStub = fileURLToPath(new URL('./tests/support/server-only-stub.ts', import.meta.url));

// Both real modules throw outside a Next request scope. These doubles let a test set the request
// headers and cookies, and record revalidatePath() calls (SMA-511, spec § 9.2).
const nextHeadersDouble = fileURLToPath(new URL('./tests/support/next-headers.ts', import.meta.url));
const nextCacheDouble = fileURLToPath(new URL('./tests/support/next-cache.ts', import.meta.url));
```

Replace:

```ts
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts'],
```

with:

```ts
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
    // The browser tier runs under Playwright; the client-boundary fixture is a Next app of its own.
    exclude: ['tests/e2e/**', 'tests/fixtures/**', '**/node_modules/**'],
    setupFiles: ['./tests/support/setup.ts'],
    env: {
      // Without it forbidden() throws E488 (next/dist/client/components/forbidden.js:26).
      __NEXT_EXPERIMENTAL_AUTH_INTERRUPTS: 'true',
      // The two values next.config.ts compiles into a real build. getRuntimeConfig() fails closed
      // without them.
      PAIGASUS_COMPILED_ZONE: 'iam',
      PAIGASUS_COMPILED_BASE_PATH: '/iam',
    },
```

Replace:

```ts
'server-only': serverOnlyStub
```

with:

```ts
'server-only': serverOnlyStub, 'next/headers': nextHeadersDouble, 'next/cache': nextCacheDouble
```

In Task 8's fallback branch the file then reads as below. In the wasm branch it also keeps Task 8's `vite-plugin-wasm` import, the `wasmGlue` constant, the `plugins` line and the `'@paigasus/wasm': wasmGlue` alias entry.

```ts
// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// `lib/*` opens with `import 'server-only'`, which throws under every condition except
// `react-server` — and that condition breaks next/navigation. So `server-only` is ALIASED to an
// empty stub, as in every @paigasus/* package with a server entry.
//
// The condition list stays ADDITIVE (not just ['node']): dropping `import`/`default` breaks the
// source-exports `.ts` resolution of the @paigasus/* packages. vitest 5 resolves a node test's
// imports through `ssr.resolve.conditions`, so both blocks carry the list (CLAUDE.md, SMA-502).
const conditions = ['node', 'import', 'default'];

const serverOnlyStub = fileURLToPath(new URL('./tests/support/server-only-stub.ts', import.meta.url));

// Both real modules throw outside a Next request scope. These doubles let a test set the request
// headers and cookies, and record revalidatePath() calls (SMA-511, spec § 9.2).
const nextHeadersDouble = fileURLToPath(new URL('./tests/support/next-headers.ts', import.meta.url));
const nextCacheDouble = fileURLToPath(new URL('./tests/support/next-cache.ts', import.meta.url));

// MEASURED (SMA-511 plan, Task 9): vite:oxc loads the NEAREST tsconfig.json for every file it
// transforms. This app's tsconfig.json extends '@paigasus/next-config/tsconfig-app' through pnpm's
// symlink, which oxc's resolver cannot follow — every import of lib/ or app/ failed with
// "[TSCONFIG_ERROR] Failed to load tsconfig … Tsconfig not found" — and the preset sets
// `jsx: preserve`, which Node cannot run. `tsconfig: false` stops the lookup, and the JSX runtime
// is stated here instead. Vite's OxcOptions type omits `tsconfig`, but the plugin spreads every key
// into rolldown's transformSync (vite 8.0.16), so a non-literal object carries it.
const oxc = { tsconfig: false, jsx: { runtime: 'automatic', importSource: 'react' } } as const;

export default defineConfig({
  oxc,
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
    // The browser tier runs under Playwright; the client-boundary fixture is a Next app of its own.
    exclude: ['tests/e2e/**', 'tests/fixtures/**', '**/node_modules/**'],
    setupFiles: ['./tests/support/setup.ts'],
    env: {
      // Without it forbidden() throws E488 (next/dist/client/components/forbidden.js:26).
      __NEXT_EXPERIMENTAL_AUTH_INTERRUPTS: 'true',
      // The two values next.config.ts compiles into a real build. getRuntimeConfig() fails closed
      // without them.
      PAIGASUS_COMPILED_ZONE: 'iam',
      PAIGASUS_COMPILED_BASE_PATH: '/iam',
    },
    // Each standalone-runtime case boots a real Next standalone server twice; the default 5s is not enough.
    testTimeout: 120_000,
    hookTimeout: 120_000,
  },
  resolve: { conditions, alias: { 'server-only': serverOnlyStub, 'next/headers': nextHeadersDouble, 'next/cache': nextCacheDouble } },
  ssr: { resolve: { conditions } },
});
```

Create `ts/apps/iam-console/tests/support/next-headers.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Stands in for `next/headers` in the vitest tier (vitest.config.ts aliases it here). The real
// module throws outside a Next request scope. A test sets the request headers and cookies it needs;
// tests/support/setup.ts clears both before every test.
let requestHeaders = new Headers();
let requestCookies = new Map<string, string>();

export function setRequestHeaders(init: Record<string, string>): void {
  requestHeaders = new Headers(init);
}

export function setRequestCookies(values: Record<string, string>): void {
  requestCookies = new Map(Object.entries(values));
}

export function resetNextHeaders(): void {
  requestHeaders = new Headers();
  requestCookies = new Map();
}

export function headers(): Promise<Headers> {
  return Promise.resolve(new Headers(requestHeaders));
}

type CookieValue = { name: string; value: string };

export function cookies(): Promise<{ get(name: string): CookieValue | undefined; has(name: string): boolean; getAll(): CookieValue[] }> {
  const snapshot = new Map(requestCookies);
  return Promise.resolve({
    get: (name) => {
      const value = snapshot.get(name);
      return value === undefined ? undefined : { name, value };
    },
    has: (name) => snapshot.has(name),
    getAll: () => [...snapshot].map(([name, value]) => ({ name, value })),
  });
}
```

Create `ts/apps/iam-console/tests/support/next-cache.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Stands in for `next/cache` in the vitest tier (vitest.config.ts aliases it here). The real
// revalidatePath() throws outside a Next request scope. This one records each call, so a command
// test can assert what an action revalidates.
export const revalidatedPaths: string[] = [];

export function revalidatePath(path: string): void {
  revalidatedPaths.push(path);
}

export function resetNextCache(): void {
  revalidatedPaths.length = 0;
}
```

Create `ts/apps/iam-console/tests/support/setup.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Runs before every test file (vitest.config.ts `setupFiles`). It resets the two Next doubles, so
// no test sees another test's request headers, cookies or revalidations.
import { beforeEach } from 'vitest';
import { resetNextCache } from './next-cache';
import { resetNextHeaders } from './next-headers';

beforeEach(() => {
  resetNextHeaders();
  resetNextCache();
});
```

Create `ts/apps/iam-console/tests/support/env.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// A complete, valid environment for lib/config.ts, and a helper that installs it with vi.stubEnv.
// vitest.config.ts already sets PAIGASUS_COMPILED_ZONE/BASE_PATH, the two values next.config.ts
// compiles into a real build.
import { vi } from 'vitest';

export const VALID_ENV: Readonly<Record<string, string>> = {
  PAIGASUS_ZONE: 'iam',
  PAIGASUS_ZONES: '{"iam":"/iam"}',
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.test',
  PAIGASUS_OIDC_CLIENT_ID: 'console',
  PAIGASUS_OIDC_CLIENT_SECRET: 'console-secret',
  PAIGASUS_PUBLIC_ORIGIN: 'https://console.example.test',
  PAIGASUS_SESSION_STORE: 'memory',
  PAIGASUS_SERVICES: '{"iam":"http://iam.internal:8080"}',
  PAIGASUS_IAM_GRPC_URL: 'http://iam.internal:9090',
};

/** Stubs VALID_ENV plus `overrides`; an `undefined` override removes the variable. Undo with vi.unstubAllEnvs(). */
export function stubConsoleEnv(overrides: Record<string, string | undefined> = {}): void {
  for (const [key, value] of Object.entries({ ...VALID_ENV, ...overrides })) {
    vi.stubEnv(key, value);
  }
}
```

- [ ] **Step 7: Write the failing tests for the config and the logger**

Create `ts/apps/iam-console/tests/unit/config.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// lib/config.ts (spec § 4.1, § 9.2). defineRuntimeConfig memoizes a SUCCESSFUL parse for the life
// of the module, so each case imports a fresh copy after vi.resetModules().
import { afterEach, describe, expect, it, vi } from 'vitest';
import { stubConsoleEnv } from '../support/env';

async function load(overrides: Record<string, string | undefined>) {
  vi.resetModules();
  stubConsoleEnv(overrides);
  return import('../../lib/config');
}

afterEach(() => {
  vi.unstubAllEnvs();
});

describe('getRuntimeConfig', () => {
  it('parses a complete environment and canonicalizes the IAM gRPC address', async () => {
    const { getRuntimeConfig } = await load({ PAIGASUS_IAM_GRPC_URL: 'http://iam.internal:9090/' });
    const config = getRuntimeConfig();
    expect(config.PAIGASUS_IAM_GRPC_URL).toBe('http://iam.internal:9090');
    expect(config.PAIGASUS_SERVICES['iam']).toBe('http://iam.internal:8080');
    expect(config.PAIGASUS_OIDC_ISSUER).toBe('https://idp.example.test');
  });

  // 'not a url' is the value that reaches the `URL.canParse` branch. 'iam.internal:9090' does not:
  // it IS a URL, with the scheme `iam.internal:` (MEASURED), so the scheme rule refuses it instead.
  // Without the canParse guard, `new URL('not a url')` throws a TypeError ("Invalid URL") out of
  // the zod parse (MEASURED, zod 4.5.4). It names no key, so this case fails its first assertion.
  it.each([
    ['not a URL', 'not a url'],
    ['a non-http scheme', 'grpc://iam.internal:9090'],
    ['credentials', 'http://user:pass@iam.internal:9090'],
    ['a query', 'http://iam.internal:9090?x=1'],
    ['a fragment', 'http://iam.internal:9090#x'],
  ])('refuses PAIGASUS_IAM_GRPC_URL with %s, and never echoes the value', async (_label, value) => {
    const { getRuntimeConfig } = await load({ PAIGASUS_IAM_GRPC_URL: value });
    let message = '';
    try {
      getRuntimeConfig();
    } catch (err) {
      message = err instanceof Error ? err.message : String(err);
    }
    expect(message).toMatch(/PAIGASUS_IAM_GRPC_URL/);
    expect(message).not.toContain(value);
  });

  it('refuses a missing PAIGASUS_IAM_GRPC_URL', async () => {
    const { getRuntimeConfig } = await load({ PAIGASUS_IAM_GRPC_URL: undefined });
    expect(() => getRuntimeConfig()).toThrow(/PAIGASUS_IAM_GRPC_URL/);
  });

  it('refuses a PAIGASUS_SERVICES map without an iam entry', async () => {
    const { getRuntimeConfig } = await load({ PAIGASUS_SERVICES: '{"gateway":"http://gateway.internal:8080"}' });
    expect(() => getRuntimeConfig()).toThrow(/PAIGASUS_SERVICES/);
  });

  it('still applies discovery’s own service-map rules', async () => {
    const { getRuntimeConfig } = await load({ PAIGASUS_SERVICES: '{"iam":"http://iam.internal:8080","unknown":"http://x.internal"}' });
    expect(() => getRuntimeConfig()).toThrow(/PAIGASUS_SERVICES/);
  });

  it('projects only the zone and the zone map into the public config', async () => {
    const { getPublicConfig } = await load({});
    expect(getPublicConfig()).toEqual({ zone: 'iam', zones: { iam: '/iam' } });
  });
});
```

Create `ts/apps/iam-console/tests/unit/logger.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// lib/logger.ts (spec § 4.8, § 9.2): it writes the event name, a time and EXACTLY the fields the
// port gave it — nothing it could have picked up elsewhere, so never a DSN or a token.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createJsonLogger } from '../../lib/logger';

afterEach(() => {
  vi.unstubAllEnvs();
});

function capture() {
  const lines: string[] = [];
  const logger = createJsonLogger((line) => lines.push(line));
  const parsed = () => lines.map((line) => JSON.parse(line) as { time: string; event: string; fields: Record<string, unknown> });
  return { logger, lines, parsed };
}

describe('createJsonLogger', () => {
  it('writes one JSON line per auth event, with the fields under their own key', () => {
    const { logger, lines, parsed } = capture();
    logger.event('session.created', { sid: 'abcd1234', zone: 'iam' });
    expect(lines).toHaveLength(1);
    const [line] = parsed();
    expect(Object.keys(line ?? {}).sort()).toEqual(['event', 'fields', 'time']);
    expect(line?.event).toBe('session.created');
    expect(line?.fields).toEqual({ sid: 'abcd1234', zone: 'iam' });
    expect(Number.isNaN(Date.parse(line?.time ?? ''))).toBe(false);
  });

  it('writes discovery events and app events the same way', () => {
    const { logger, parsed } = capture();
    logger.event('discovery.probe_failed', { service: 'iam', reason: 'timeout' });
    logger.appEvent('principal.resolve_failed', { presentation: 'degraded', correlation_id: null });
    expect(parsed().map((line) => [line.event, line.fields])).toEqual([
      ['discovery.probe_failed', { service: 'iam', reason: 'timeout' }],
      ['principal.resolve_failed', { presentation: 'degraded', correlation_id: null }],
    ]);
  });

  // The DSN is really in the environment, in the variable the Redis connect path reads. So a logger
  // that appends the env (or any key beyond time/event/fields) fails both assertions.
  it('writes exactly the given fields, and never the Redis DSN that is in its environment', () => {
    const dsn = 'redis://console:s3cret-password@redis.internal:6379/0';
    vi.stubEnv('PAIGASUS_SESSION_REDIS_URL', dsn);
    const { logger, lines } = capture();
    logger.appEvent('discovery.redis_connect_failed', { stage: 'connect' });
    expect(lines).toHaveLength(1);
    // The FULL record, by toEqual: any key the logger adds beyond these three fails it. The time
    // is taken from the record itself, and checked on its own below.
    const record = JSON.parse(lines[0] ?? '{}') as Record<string, unknown>;
    expect(record).toEqual({ time: record['time'], event: 'discovery.redis_connect_failed', fields: { stage: 'connect' } });
    expect(typeof record['time']).toBe('string');
    const output = lines.join('\n');
    expect(output).not.toContain(dsn);
    expect(output).not.toContain('s3cret-password');
  });
});
```

- [ ] **Step 8: Run the tests and see them fail**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/unit/config.test.ts tests/unit/logger.test.ts
```

Expected: FAIL. Both files report that `../../lib/config` and `../../lib/logger` cannot be resolved.

- [ ] **Step 9: Write `lib/config.ts`**

Create `ts/apps/iam-console/lib/config.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// This app's ONE call to defineRuntimeConfig (spec § 4.1). It composes three parts into one
// schema, one parse and one place: @paigasus/auth's shape, @paigasus/discovery's shape, and the one
// key this app owns.
//
// No accessor runs at module scope. getRuntimeConfig() throws during `next build`
// (phase-production-build), so a module-scope read fails the build loudly instead of baking the
// builder's environment into the image.
import 'server-only';
import { z } from 'zod';
import { authEnvShape } from '@paigasus/auth/server';
import { discoveryEnvShape } from '@paigasus/discovery/server';
import { defineRuntimeConfig } from '@paigasus/next-config/runtime';

/** Why `value` is not an acceptable IAM gRPC address, or null when it is. */
function grpcUrlProblem(value: string): string | null {
  if (!URL.canParse(value)) return 'must be an absolute URL';
  const url = new URL(value);
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return 'must use http: or https:';
  if (url.username !== '' || url.password !== '') return 'must not carry credentials';
  if (url.search !== '' || url.hash !== '') return 'must not carry a query or a fragment';
  return null;
}

/**
 * IAM's gRPC address. A separate key from PAIGASUS_SERVICES.iam because IAM listens on two
 * addresses: HTTP on 8080, which discovery probes, and gRPC on 9090, which the SDK calls
 * (rs/crates/services/paigasus-iam/src/config.rs:811-812).
 *
 * One transform, not a chain of refines: a refine that calls `new URL` after a failed
 * `URL.canParse` refine would THROW, because zod 4 runs every refine. The value is canonicalized
 * to `<origin><path>` with no trailing slash, the form @paigasus/discovery gives its addresses.
 * The message never contains the value (next-config renders only the issue code for this key).
 */
const iamGrpcUrl = z.string().transform((value, ctx) => {
  const problem = grpcUrlProblem(value);
  if (problem !== null) {
    ctx.addIssue({ code: 'custom', message: problem });
    return z.NEVER;
  }
  const url = new URL(value);
  return `${url.origin}${url.pathname.replace(/\/+$/, '')}`;
});

/** discovery's service map, plus one rule: the console cannot run without an `iam` entry. */
const servicesWithIam = discoveryEnvShape.PAIGASUS_SERVICES.refine((services) => Object.hasOwn(services, 'iam'), { error: 'must contain an "iam" entry' });

export const { getRuntimeConfig, getPublicConfig } = defineRuntimeConfig({
  ...authEnvShape,
  ...discoveryEnvShape,
  PAIGASUS_SERVICES: servicesWithIam,
  PAIGASUS_IAM_GRPC_URL: iamGrpcUrl,
});

export type ConsoleConfig = ReturnType<typeof getRuntimeConfig>;
```

- [ ] **Step 10: Write `lib/logger.ts`**

Create `ts/apps/iam-console/lib/logger.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The app's one logger adapter (spec § 4.8): JSON lines on stdout. It implements @paigasus/auth's
// AuthLogger and @paigasus/discovery's DiscoveryLogger, and adds `appEvent` for the app's own
// events, because AuthEventName is a closed union (ts/packages/paigasus-auth/src/ports/logger.ts:14-25).
//
// REDACTION IS THE CALLER'S CONTRACT (both ports say so). This adapter writes the event name, a
// timestamp and EXACTLY the fields it receives, under their own key, and adds nothing else. It
// never serializes an error object: the field types admit only scalars.
import 'server-only';
import type { AuthEventFields, AuthEventName, AuthLogger } from '@paigasus/auth/server';
import type { DiscoveryEventFields, DiscoveryEventName, DiscoveryLogger } from '@paigasus/discovery/server';

export type AppEventName = 'principal.resolve_failed' | 'authorize.query_failed' | 'discovery.redis_connect_failed' | 'iam.call_failed';

export type AppEventFields = Readonly<Record<string, string | number | boolean | null>>;

/**
 * `event` is declared again here, as the union of both ports' event names. Without it TypeScript
 * rejects the interface: AuthLogger.event and DiscoveryLogger.event have different parameter types,
 * and an interface cannot extend two bases whose same-named members are not identical.
 */
export interface ConsoleLogger extends AuthLogger, DiscoveryLogger {
  event(name: AuthEventName | DiscoveryEventName, fields: AuthEventFields | DiscoveryEventFields): void;
  appEvent(name: AppEventName, fields: AppEventFields): void;
}

const stdout = (line: string): void => {
  process.stdout.write(`${line}\n`);
};

export function createJsonLogger(write: (line: string) => void = stdout): ConsoleLogger {
  const emit = (event: string, fields: Readonly<Record<string, unknown>>): void => {
    write(JSON.stringify({ time: new Date().toISOString(), event, fields }));
  };
  return {
    event: (name, fields) => emit(name, fields),
    appEvent: (name, fields) => emit(name, fields),
  };
}

/** The process-wide instance. Stateless, so one per process is correct. */
export const logger: ConsoleLogger = createJsonLogger();
```

- [ ] **Step 11: Run the tests and see them pass**

Run the command of Step 8 again.

Expected: `Test Files  2 passed (2)` and `Tests  13 passed (13)`.

- [ ] **Step 12: Add the packages that `lib/config.ts` compiles to the Moon inputs**

`lib/config.ts` composes `@paigasus/auth`'s and `@paigasus/discovery`'s env shapes, and discovery imports `@paigasus/proto`. `build` and `test` use `options.merge: replace` and inherit nothing, so each package they compile is listed by hand, sources AND manifest (the manifest holds the `exports` map). Task 8 already put `lib/**/*` into `fileGroups.sources`, which `@group(sources)` carries into every task. Task 23 finalizes these lists and `dependsOn`.

In `ts/apps/iam-console/moon.yml`, replace every occurrence (two: `build` and `test`) of:

```yaml
      - '/ts/tsconfig.base.json'
```

with:

```yaml
      - '/ts/tsconfig.base.json'
      # SMA-511: packages the app compiles (Task 9: auth, discovery, proto; later tasks add more).
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
```

(With the Edit tool, use `replace_all: true`. The `typecheck` task has no such line: it merges, and inherits `/ts/tsconfig.base.json`.)

In the `typecheck` task's `inputs`, replace:

```yaml
      - '/ts/packages/paigasus-ui/package.json'

  test:
```

with:

```yaml
      - '/ts/packages/paigasus-ui/package.json'
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'

  test:
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
grep -c "paigasus-proto/package.json" ts/apps/iam-console/moon.yml
moon run iam-console-ts:typecheck
```

Expected: `3`, then `moon run` exits 0.

- [ ] **Step 13: Document the new variables for `next dev`**

Replace `ts/apps/iam-console/.env.local.example` with:

```bash
# SPDX-License-Identifier: Apache-2.0
#
# Copy to .env.local for `next dev`. No variable below has a default in code unless its comment
# says so: a default would hide a misconfiguration. lib/config.ts is the one schema (spec § 4.1).
#
# In a real deployment Helm renders PAIGASUS_ZONES from the SAME values block as the ingress
# rules, so the routing and the console's zone map cannot drift (ADR-0017).

# This app's zone id. Must equal the `zone` passed to createNextConfig() in next.config.ts.
PAIGASUS_ZONE=iam

# Every deployed zone, mapped to its path prefix behind the single origin. The entry for
# PAIGASUS_ZONE must equal this image's compiled basePath, or the first request fails loudly.
# With PAIGASUS_SESSION_STORE=memory the map may hold ONE zone only (@paigasus/auth runtime.ts:105-111).
PAIGASUS_ZONES={"iam":"/iam"}

# @paigasus/auth (ts/packages/paigasus-auth/README.md). Issuer and public origin must be https.
PAIGASUS_OIDC_ISSUER=https://idp.example.test/realms/paigasus
PAIGASUS_OIDC_CLIENT_ID=iam-console
PAIGASUS_OIDC_CLIENT_SECRET=change-me
PAIGASUS_PUBLIC_ORIGIN=https://console.example.test
PAIGASUS_SESSION_STORE=memory
# PAIGASUS_SESSION_REDIS_URL=redis://localhost:6379   (required when PAIGASUS_SESSION_STORE=redis)

# @paigasus/discovery. IAM's HTTP address, which serves GET /v1/service-info. It must contain `iam`.
PAIGASUS_SERVICES={"iam":"http://localhost:8080"}

# IAM's gRPC address (h2c inside a cluster). A separate key: IAM listens on 8080 (HTTP) and 9090 (gRPC).
PAIGASUS_IAM_GRPC_URL=http://localhost:9090
```

- [ ] **Step 14: Run the app's gates**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write pnpm-workspace.yaml \
  apps/iam-console/package.json apps/iam-console/moon.yml apps/iam-console/vitest.config.ts \
  apps/iam-console/tests/tsconfig.json apps/iam-console/tests/support/next-headers.ts \
  apps/iam-console/tests/support/next-cache.ts apps/iam-console/tests/support/setup.ts \
  apps/iam-console/tests/support/env.ts apps/iam-console/lib/config.ts apps/iam-console/lib/logger.ts \
  apps/iam-console/tests/unit/config.test.ts apps/iam-console/tests/unit/logger.test.ts
moon run iam-console-ts:test
moon run ts:lint ts:fmt --force
```

Expected: exit 0. Prettier formats every `ts/` file this task creates or edits (the Global Constraints). Two files are not in the list. `ts/pnpm-lock.yaml` is in `ts/.prettierignore`. Prettier has no parser for `.env.local.example`, and an explicit path to it exits 2 (measured). `iam-console-ts:test` builds the app, runs vitest (Task 8's `prn` suite, the two new unit files, and the standalone test, which still targets the old page), and runs the Tailwind guard in its three modes. `--force` makes the whole-tree lint and format run even when Moon's cache key did not move.

- [ ] **Step 15: Commit**

```bash
git add ts/pnpm-workspace.yaml ts/pnpm-lock.yaml \
  ts/apps/iam-console/package.json ts/apps/iam-console/moon.yml ts/apps/iam-console/.env.local.example \
  ts/apps/iam-console/vitest.config.ts ts/apps/iam-console/tests/tsconfig.json \
  ts/apps/iam-console/tests/support/next-headers.ts \
  ts/apps/iam-console/tests/support/next-cache.ts ts/apps/iam-console/tests/support/setup.ts \
  ts/apps/iam-console/tests/support/env.ts ts/apps/iam-console/lib/config.ts ts/apps/iam-console/lib/logger.ts \
  ts/apps/iam-console/tests/unit/config.test.ts ts/apps/iam-console/tests/unit/logger.test.ts
git commit -F - <<'EOF'
feat(ts): iam-console runtime config, logger and vitest setup (SMA-511)

The app's one defineRuntimeConfig call composes the auth and discovery
shapes with PAIGASUS_IAM_GRPC_URL, and refuses a service map without
iam. The JSON-lines logger implements both package logger ports and
adds appEvent for the app's own events.

vitest gets the next/headers and next/cache test doubles and a setup
file that resets them before every test.

msw joins the catalog, pinned at 2.15.0, with its browser postinstall
blocked. @bufbuild/protobuf is a runtime dependency. The app's
Moon tasks now key on the packages lib/config.ts compiles.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 10: Auth wiring, the proxy, the public page, `/iam/healthz` and the root layout

This task mounts `@paigasus/auth` in the app (spec § 4.2, § 7.5), adds the signed-out landing page `/iam/` and the public `/iam/healthz` (spec § 3.3, § 9.5), and deletes the old demo page. The standalone-runtime test moves to `/iam/healthz`.

**Files:**
- Create: `ts/apps/iam-console/lib/auth.ts`
- Create: `ts/apps/iam-console/proxy.ts`
- Create: `ts/apps/iam-console/app/auth/[...auth]/route.ts`
- Create: `ts/apps/iam-console/app/(public)/layout.tsx`
- Create: `ts/apps/iam-console/app/(public)/page.tsx`
- Create: `ts/apps/iam-console/app/healthz/route.ts`
- Modify: `ts/apps/iam-console/app/layout.tsx`
- Modify: `ts/apps/iam-console/app/providers.tsx`
- Delete: `ts/apps/iam-console/app/page.tsx`
- Delete: `ts/apps/iam-console/app/runtime-config.ts`
- Modify: `ts/apps/iam-console/moon.yml` (`proxy.ts` in `fileGroups.sources`; app-shell inputs)
- Modify: `ts/moon.yml` (`apps/*/proxy.ts` in `fileGroups.sources`)
- Test: `ts/apps/iam-console/tests/unit/session-cookie.test.ts`
- Test: `ts/apps/iam-console/tests/unit/proxy.test.ts`
- Test: `ts/apps/iam-console/tests/standalone-runtime.test.ts`

**Interfaces:**
- Consumes: `getAuthRuntime`, `createAuthRouteHandler`, `type AuthRuntime` (`@paigasus/auth/server`); `authRoutePaths()` (no argument, basePath-relative paths) and `createAuthMiddleware({ publicPaths, loginPath })` with basePath-relative paths (`@paigasus/auth/middleware`, as Task 5 leaves them); `SessionProvider`, `type SessionView` (`@paigasus/auth/client`); `ZoneProvider`, `ZoneLink`, `PublicShell`, `type ZoneMap` (`@paigasus/app-shell`); `LinkProvider` (`@paigasus/ui`); `getRuntimeConfig`, `getPublicConfig` (Task 9); `logger` (Task 9).
- Produces: `lib/auth.ts` → `authRuntime(): Promise<AuthRuntime>`, `SESSION_COOKIE_NAME`. `proxy.ts` → `proxy(req)`, `config.matcher`. `app/providers.tsx` → `Providers({ zone, zones, session, children })`. Routes `GET /iam/healthz`, `GET|POST /iam/auth/*`, page `/iam/`.

- [ ] **Step 1: Check the middleware API that Task 5 left**

Run:

```bash
grep -n "export function authRoutePaths\|basePath" ts/packages/paigasus-auth/src/middleware.ts
```

Expected: `export function authRoutePaths(): readonly string[]` (no parameter), and the redirect and `returnTo` built with `req.nextUrl.basePath` (spec § 7.1). This task therefore writes `createAuthMiddleware({ publicPaths: [...authRoutePaths(), '/', '/healthz'], loginPath: '/auth/login' })`, with no `/iam` anywhere in `proxy.ts`. The test in Step 2 pins the observable result: the login redirect is `/iam/auth/login` once, and `returnTo` keeps `/iam`.

- [ ] **Step 2: Write the failing tests for the session cookie and the proxy**

Create `ts/apps/iam-console/tests/unit/session-cookie.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SESSION_COOKIE_NAME is written in lib/auth.ts because @paigasus/auth exports its cookie name
// from no entry an app may import. This test binds the literal to the package's BEHAVIOUR: its own
// middleware must treat exactly this name as the session, and no other.
import { NextRequest } from 'next/server';
import { describe, expect, it } from 'vitest';
import { createAuthMiddleware } from '@paigasus/auth/middleware';
import { SESSION_COOKIE_NAME } from '../../lib/auth';

const middleware = createAuthMiddleware({ publicPaths: ['/auth/login'], loginPath: '/auth/login' });

function withCookie(name: string): NextRequest {
  return new NextRequest('https://console.example.test/iam/orgs', { headers: { cookie: `${name}=x` }, nextConfig: { basePath: '/iam' } });
}

describe('SESSION_COOKIE_NAME', () => {
  it('is the name @paigasus/auth/middleware accepts as a session', () => {
    expect(middleware(withCookie(SESSION_COOKIE_NAME)).headers.get('location')).toBeNull();
  });

  it('is not accepted in any other spelling (the control)', () => {
    expect(middleware(withCookie(`${SESSION_COOKIE_NAME}x`)).headers.get('location')).not.toBeNull();
  });
});
```

Create `ts/apps/iam-console/tests/unit/proxy.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// proxy.ts under the real basePath (spec § 7.5, § 13 #1). A NextRequest built with
// `nextConfig: { basePath: '/iam' }` strips the basePath from nextUrl.pathname exactly as Next does.
import { unstable_doesMiddlewareMatch } from 'next/experimental/testing/server';
import { NextRequest } from 'next/server';
import { describe, expect, it, vi } from 'vitest';
import { SESSION_COOKIE_NAME } from '../../lib/auth';
import { config, proxy } from '../../proxy';

// Next's server installs globalThis.AsyncLocalStorage before any other module
// (next/dist/server/node-environment-baseline.js); vitest does not. Loading
// next/experimental/testing/server patches `console`, and without the global every later console
// call in this file throws "AsyncLocalStorage accessed in runtime where it is not available"
// (MEASURED, Next 16.3.4 under vitest 5.0.0). vi.hoisted runs before the imports above.
vi.hoisted(() => {
  const scope = globalThis as { AsyncLocalStorage?: unknown };
  scope.AsyncLocalStorage ??= process.getBuiltinModule('node:async_hooks').AsyncLocalStorage;
});

const ORIGIN = 'https://console.example.test';

/** The basePath next.config.ts compiles in. unstable_doesMiddlewareMatch prefixes each matcher with it, as Next does. */
const NEXT_CONFIG = { basePath: '/iam' };

function request(path: string, init: { cookie?: boolean; headers?: Record<string, string> } = {}): NextRequest {
  const headers = new Headers(init.headers);
  if (init.cookie === true) headers.set('cookie', `${SESSION_COOKIE_NAME}=opaque-session-id`);
  return new NextRequest(`${ORIGIN}${path}`, { headers, nextConfig: { basePath: '/iam' } });
}

describe('proxy', () => {
  it.each(['/iam', '/iam/healthz', '/iam/auth/login', '/iam/auth/callback', '/iam/auth/logout', '/iam/auth/logout/callback'])('lets %s through with no cookie', (path) => {
    const res = proxy(request(path));
    expect(res.headers.get('location')).toBeNull();
  });

  it('sends a visitor with no cookie to login ONCE under the basePath, and keeps the basePath in returnTo', () => {
    const res = proxy(request('/iam/orgs?offset=50'));
    const location = new URL(res.headers.get('location') ?? '', ORIGIN);
    expect(location.pathname).toBe('/iam/auth/login');
    expect(location.searchParams.get('returnTo')).toBe('/iam/orgs?offset=50');
  });

  it('lets a request with the session cookie through, and checks presence only', () => {
    const res = proxy(request('/iam/orgs', { cookie: true }));
    expect(res.headers.get('location')).toBeNull();
  });
});

// Next's own matcher evaluation (next/experimental/testing/server), run against the exported
// config under the real basePath. MEASURED: a matcher written WITH the /iam prefix matches no page
// here (Next prefixes the basePath again), so the "runs" cases fail; a matcher that drops one of
// the three exclusions fails the matching "skips" case.
describe('config.matcher (spec § 7.5, § 13 #4)', () => {
  it('is written WITHOUT the basePath', () => {
    expect(config.matcher).toEqual(['/((?!_next/static|_next/image|favicon.ico).*)']);
  });

  it.each(['/iam/', '/iam/orgs', '/iam/orgs?offset=50', '/iam/healthz', '/iam/auth/login'])('runs the proxy for the page %s', (url) => {
    expect(unstable_doesMiddlewareMatch({ config, url, nextConfig: NEXT_CONFIG })).toBe(true);
  });

  it.each(['/iam/_next/static/chunks/main.js', '/iam/_next/image?url=%2Flogo.png&w=64&q=75', '/iam/favicon.ico'])('skips the static asset %s', (url) => {
    expect(unstable_doesMiddlewareMatch({ config, url, nextConfig: NEXT_CONFIG })).toBe(false);
  });
});
```

- [ ] **Step 3: Run the tests and see them fail**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/unit/session-cookie.test.ts tests/unit/proxy.test.ts
```

Expected: FAIL. Both files report that `../../lib/auth` (and `../../proxy`) cannot be resolved.

- [ ] **Step 4: Write `lib/auth.ts`**

Create `ts/apps/iam-console/lib/auth.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The auth composition root (spec § 4.2). getAuthRuntime is a PROCESS singleton that returns a
// Promise (ts/packages/paigasus-auth/src/runtime.ts:185), and its first call fixes the resolver and
// the logger for the life of the process. Nothing here runs at module scope.
//
// Task 13 adds the Introspect principal resolver. Until then the package's claims resolver runs,
// which reports principalPrn: null (ts/packages/paigasus-auth/src/adapters/claims-resolver.ts).
import 'server-only';
import { getAuthRuntime, type AuthRuntime } from '@paigasus/auth/server';
import { getRuntimeConfig } from './config';
import { logger } from './logger';

/**
 * The session cookie's name. @paigasus/auth does not export it from any entry an app may import,
 * so it is written here once; tests/unit/session-cookie.test.ts proves that @paigasus/auth's own
 * middleware treats exactly this name as the session.
 */
export const SESSION_COOKIE_NAME = '__Host-pgs_sid';

export function authRuntime(): Promise<AuthRuntime> {
  return getAuthRuntime(getRuntimeConfig(), { logger });
}
```

- [ ] **Step 5: Write `proxy.ts`**

Create `ts/apps/iam-console/proxy.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The zone's proxy (Next 16's name for middleware; spec § 3.3, § 7.5).
//
// COOKIE PRESENCE ONLY (ADR-0017 decision 7). It imports @paigasus/auth/middleware, never /server
// and never the sdk (paigasus/boundaries/app-middleware). A forged or expired cookie passes here and
// is rejected by requireSession() in the (console) layout.
//
// Public paths are basePath-RELATIVE: Next strips the basePath from req.nextUrl.pathname before the
// proxy sees it (spec § 13 #1), and Task 5 makes createAuthMiddleware add it back to the login
// redirect and to returnTo.
import type { NextRequest, NextResponse } from 'next/server';
import { authRoutePaths, createAuthMiddleware } from '@paigasus/auth/middleware';

const authMiddleware = createAuthMiddleware({
  publicPaths: [...authRoutePaths(), '/', '/healthz'],
  loginPath: '/auth/login',
});

export function proxy(req: NextRequest): NextResponse {
  return authMiddleware(req);
}

/*
 * Without a matcher the proxy runs for every CSS and JS file too, and a visitor with no cookie gets
 * a login redirect for each one (spec § 7.5). MEASURED (spec § 13 #4): this pattern, written
 * WITHOUT the /iam prefix, skips the static assets under the basePath.
 */
export const config = {
  matcher: ['/((?!_next/static|_next/image|favicon.ico).*)'],
};
```

- [ ] **Step 6: Run the tests and see them pass**

Run the command of Step 3 again.

Expected: `Test Files  2 passed (2)` and `Tests  19 passed (19)` (2 session-cookie cases; 8 proxy cases and 9 `config.matcher` cases). If only the case `sends a visitor with no cookie to login ONCE under the basePath …` fails with `expected '/auth/login' to be '/iam/auth/login'`, Task 5's middleware fix is missing: stop and complete Task 5 first. Do not change the test. (Measured in the session scratchpad: this is exactly how the case fails against the pre-Task 5 middleware.)

- [ ] **Step 7: Rewrite the standalone-runtime test for `/iam/healthz`**

Replace `ts/apps/iam-console/tests/standalone-runtime.test.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// AC 3's runtime half (spec § 9.5). The build asserts the standalone ARTIFACT exists; this asserts
// the SERVER reads deployment configuration at runtime. Two boots of the SAME binary with different
// PAIGASUS_ZONES must answer different zone maps on `GET /iam/healthz`, which is public and runs the
// FULL configuration parse. If Next had inlined a value, or the standalone trace had dropped a
// package, both boots would agree — and every unit test in this repo would still pass.
//
// Each boot gets a COMPLETE, valid environment, so a failure can only come from what a case
// changes. The mismatch case asserts the mismatch MESSAGE in the server output, not only a 500: a
// missing unrelated variable also answers 500, and must not pass it.
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:net';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const SERVER_ENTRY = fileURLToPath(new URL('../.next/standalone/apps/iam-console/server.js', import.meta.url));

const COMPLETE_ENV: Readonly<Record<string, string>> = {
  PAIGASUS_ZONE: 'iam',
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.test',
  PAIGASUS_OIDC_CLIENT_ID: 'console',
  PAIGASUS_OIDC_CLIENT_SECRET: 'console-secret',
  PAIGASUS_PUBLIC_ORIGIN: 'https://console.example.test',
  PAIGASUS_SESSION_STORE: 'memory',
  PAIGASUS_SERVICES: '{"iam":"http://127.0.0.1:9"}',
  PAIGASUS_IAM_GRPC_URL: 'http://127.0.0.1:9',
};

async function freePort(): Promise<number> {
  return await new Promise((resolve, reject) => {
    const srv = createServer();
    srv.once('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const address = srv.address();
      if (address === null || typeof address === 'string') {
        reject(new Error('could not acquire a port'));
        return;
      }
      const { port } = address;
      srv.close(() => resolve(port));
    });
  });
}

type Answer = { status: number; body: string; output: string };

// The child is OBSERVED, not merely polled (the reasons are recorded in this file's history): an
// early death is reported as a death, both streams are drained so a chatty child cannot block, and
// a deadline still covers a process that starts and never listens. `waitForOutput` keeps the server
// alive until its log contains that text, because Next writes a route error to stderr around the
// time it sends the 500.
async function healthzWith(zones: Record<string, string>, waitForOutput?: string): Promise<Answer> {
  const port = await freePort();
  const child: ChildProcess = spawn(process.execPath, [SERVER_ENTRY], {
    env: { ...process.env, ...COMPLETE_ENV, PORT: String(port), HOSTNAME: '127.0.0.1', PAIGASUS_ZONES: JSON.stringify(zones) },
    stdio: 'pipe',
  });

  let output = '';
  const capture = (chunk: Buffer | string): void => {
    output += String(chunk);
  };
  child.stdout?.on('data', capture);
  child.stderr?.on('data', capture);

  let died: string | undefined;
  child.once('error', (err: Error) => {
    died = `the server process failed to start: ${err.message}`;
  });
  child.once('exit', (code, signal) => {
    died = `the server process exited before answering (code ${String(code)}, signal ${String(signal)})`;
  });

  const withOutput = (message: string): Error => new Error(`${message}\n--- server output ---\n${output.trim() === '' ? '(none captured)' : output}`);

  try {
    const deadline = Date.now() + 60_000;
    for (;;) {
      if (died !== undefined) throw withOutput(died);
      if (Date.now() > deadline) throw withOutput('standalone server did not become ready within 60s');
      let res: Response | undefined;
      try {
        res = await fetch(`http://127.0.0.1:${String(port)}/iam/healthz`);
      } catch {
        // not listening yet
      }
      if (res !== undefined) {
        const answer = { status: res.status, body: await res.text() };
        const logDeadline = Date.now() + 5_000;
        while (waitForOutput !== undefined && !output.includes(waitForOutput) && Date.now() < logDeadline) {
          await new Promise((r) => setTimeout(r, 100));
        }
        return { ...answer, output };
      }
      await new Promise((r) => setTimeout(r, 250));
    }
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  }
}

describe('the standalone server reads configuration at runtime', () => {
  it('serves different zone maps from the SAME binary', async () => {
    const first = await healthzWith({ iam: '/iam', gateway: '/gateway' });
    const second = await healthzWith({ iam: '/iam', reports: '/reports' });

    expect(first.status).toBe(200);
    expect(second.status).toBe(200);
    expect(JSON.parse(first.body)).toEqual({ zone: 'iam', zones: { iam: '/iam', gateway: '/gateway' } });
    expect(JSON.parse(second.body)).toEqual({ zone: 'iam', zones: { iam: '/iam', reports: '/reports' } });
  });

  it('fails loudly, with the mismatch message, when the deployed prefix disagrees with the compiled one', async () => {
    const message = 'Base path mismatch for zone "iam"';
    const result = await healthzWith({ iam: '/admin/iam' }, message);
    expect(result.status).toBe(500);
    expect(result.output).toContain(message);
  });
});
```

- [ ] **Step 8: Run the app test task and see the new test fail**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:test
```

Expected: FAIL in `tests/standalone-runtime.test.ts`. `/iam/healthz` does not exist yet, so `first.status` is `404`, not `200`.

- [ ] **Step 9: Replace the demo page with the public group and `/iam/healthz`**

Delete the demo page and the old config module (lib/config.ts replaced it in Task 9):

```bash
git rm ts/apps/iam-console/app/page.tsx ts/apps/iam-console/app/runtime-config.ts
```

Replace `ts/apps/iam-console/app/providers.tsx` with:

```tsx
// SPDX-License-Identifier: Apache-2.0
'use client';

import { useEffect, type ReactElement, type ReactNode } from 'react';
import { SessionProvider, type SessionView } from '@paigasus/auth/client';
import { ZoneLink, ZoneProvider, type ZoneMap } from '@paigasus/app-shell';
import { LinkProvider } from '@paigasus/ui';

/*
 * The client-side context for every page: the zone map, the session view (console pages only), and
 * the Link implementation @paigasus/ui renders (ADR-0021 decision 3).
 *
 * A client boundary because ZoneLink is a function and cannot cross from a server layout as a prop.
 * `session` is `toSessionView()` output — never a token (ADR-0017). It is null on the public page,
 * which renders PublicShell and needs no SessionProvider.
 */
export function Providers({ zone, zones, session, children }: { zone: string; zones: ZoneMap; session: SessionView | null; children: ReactNode }): ReactElement {
  useEffect(() => {
    // The hydration signal the e2e tier waits for before a click (the @paigasus/app-shell fixture's
    // pattern): a click before hydration is a document navigation for every link.
    document.documentElement.dataset['hydrated'] = 'true';
  }, []);
  const linked = <LinkProvider link={ZoneLink}>{children}</LinkProvider>;
  return (
    <ZoneProvider zone={zone} zones={zones}>
      {session === null ? linked : <SessionProvider value={session}>{linked}</SessionProvider>}
    </ZoneProvider>
  );
}
```

Replace `ts/apps/iam-console/app/layout.tsx` with:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The root layout: html, body and the stylesheet ONLY (spec § 3.3). Each route group adds its own
// providers — the public group has no session, the console group has one.
import type { Metadata } from 'next';
import type { ReactElement, ReactNode } from 'react';
import './globals.css';

export const metadata: Metadata = { title: 'Paigasus IAM' };

export default function RootLayout({ children }: { children: ReactNode }): ReactElement {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
```

Create `ts/apps/iam-console/app/(public)/layout.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The public group: the signed-out landing page. A ZoneProvider and NO SessionProvider (PublicShell
// never calls useSession(); SMA-510 spec § 8.2).
//
// `await connection()` opts the group out of prerendering. getPublicConfig() reads the deployment's
// environment, which does not exist during `next build`.
import type { ReactElement, ReactNode } from 'react';
import { connection } from 'next/server';
import { getPublicConfig } from '../../lib/config';
import { Providers } from '../providers';

export default async function PublicLayout({ children }: { children: ReactNode }): Promise<ReactElement> {
  await connection();
  const { zone, zones } = getPublicConfig();
  return (
    <Providers zone={zone} zones={zones} session={null}>
      {children}
    </Providers>
  );
}
```

Create `ts/apps/iam-console/app/(public)/page.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// `/iam/` — the landing page (spec § 3.3). It checks only that the session cookie EXISTS and never
// resolves the session, so it needs no identity provider. A stale cookie therefore reaches
// requireSession() in the (console) layout, which sends the browser to login. @paigasus/auth sends
// `idp_error` and the default post-logout redirect here (server.ts:112-113, runtime.ts:129).
//
// redirect('/orgs') is basePath-relative: Next adds /iam exactly once (spec § 13 #1).
import type { ReactElement } from 'react';
import { cookies } from 'next/headers';
import { redirect } from 'next/navigation';
import { connection } from 'next/server';
import { PublicShell } from '@paigasus/app-shell';
import { SESSION_COOKIE_NAME } from '../../lib/auth';

export default async function PublicHome(): Promise<ReactElement> {
  await connection();
  if ((await cookies()).has(SESSION_COOKIE_NAME)) redirect('/orgs');
  return (
    <PublicShell brand={{ label: 'Paigasus IAM', href: '/iam' }}>
      <section className="p-8" data-testid="public-home">
        <h1 className="mb-2 text-2xl font-semibold">Paigasus IAM</h1>
        <p className="text-muted-foreground text-sm">Sign in to manage your organizations, teams and projects.</p>
      </section>
    </PublicShell>
  );
}
```

Create `ts/apps/iam-console/app/healthz/route.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// `GET /iam/healthz` — public (proxy.ts lists it), and the target of the standalone-runtime test
// (spec § 9.5). It answers the client-safe slice only: the zone and the zone map. It still runs the
// FULL configuration parse, so a server that answers 200 here has a valid environment.
import { connection } from 'next/server';
import { getPublicConfig } from '../../lib/config';

export async function GET(): Promise<Response> {
  await connection();
  const { zone, zones } = getPublicConfig();
  return Response.json({ zone, zones }, { headers: { 'cache-control': 'no-store' } });
}
```

Create `ts/apps/iam-console/app/auth/[...auth]/route.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The four auth routes (/iam/auth/login, /callback, /logout, /logout/callback), mounted as one
// catch-all route handler (spec § 4.2). The handler is an ASYNC function that awaits the runtime on
// each request; nothing builds the runtime at module scope, where `next build` would evaluate it.
//
// Task 5 makes createAuthRouteHandler rebuild the request URL from PAIGASUS_PUBLIC_ORIGIN and the
// basePath, because Next hands a route handler a URL with the bind address and no basePath
// (spec § 7.1, § 13 #1).
import { createAuthRouteHandler, type AuthRuntime } from '@paigasus/auth/server';
import { authRuntime } from '../../../lib/auth';

// One handler per runtime. The runtime is a process singleton, so this holds one entry.
const handlers = new WeakMap<AuthRuntime, (req: Request) => Promise<Response>>();

async function handle(req: Request): Promise<Response> {
  const runtime = await authRuntime();
  let handler = handlers.get(runtime);
  if (handler === undefined) {
    handler = createAuthRouteHandler(runtime);
    handlers.set(runtime, handler);
  }
  return handler(req);
}

export async function GET(req: Request): Promise<Response> {
  return handle(req);
}

export async function POST(req: Request): Promise<Response> {
  return handle(req);
}
```

- [ ] **Step 10: Make Moon see `proxy.ts` and the app-shell package**

In `ts/apps/iam-console/moon.yml`, replace:

```yaml
    - 'lib/**/*'
```

with:

```yaml
    - 'lib/**/*'
    # SMA-511: the zone's proxy (spec § 8). Without it an edit to proxy.ts serves a cached build.
    - 'proxy.ts'
```

In `ts/moon.yml`, replace:

```yaml
    - 'apps/*/lib/**/*'
```

with:

```yaml
    - 'apps/*/lib/**/*'
    # SMA-511: a zone app's proxy, for the same reason.
    - 'apps/*/proxy.ts'
```

The public page and the providers compile `@paigasus/app-shell` now. In `ts/apps/iam-console/moon.yml`, replace every occurrence (three: `build`, `typecheck`, `test`) of:

```yaml
      - '/ts/packages/paigasus-proto/package.json'
```

with:

```yaml
      - '/ts/packages/paigasus-proto/package.json'
      - '/ts/packages/paigasus-app-shell/src/**/*'
      - '/ts/packages/paigasus-app-shell/package.json'
```

(With the Edit tool, use `replace_all: true`.)

- [ ] **Step 11: Run the app test task and see it pass**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write apps/iam-console/lib/auth.ts apps/iam-console/proxy.ts \
  'apps/iam-console/app/auth/[...auth]/route.ts' 'apps/iam-console/app/(public)/layout.tsx' \
  'apps/iam-console/app/(public)/page.tsx' apps/iam-console/app/healthz/route.ts \
  apps/iam-console/app/layout.tsx apps/iam-console/app/providers.tsx apps/iam-console/moon.yml moon.yml \
  apps/iam-console/tests/unit/session-cookie.test.ts apps/iam-console/tests/unit/proxy.test.ts \
  apps/iam-console/tests/standalone-runtime.test.ts
moon run iam-console-ts:test iam-console-ts:typecheck
moon run ts:lint ts:fmt --force
```

Expected: exit 0. Prettier first formats every `ts/` file this task creates or edits (the Global Constraints). It takes the bracket and parenthesis paths literally, because each path exists (measured with Prettier 3.9.6). This is the FIRST `next build` that compiles `@paigasus/auth/server` and `@paigasus/discovery/server` (Task 3 made their relative imports extensionless). A `Module not found: Can't resolve './….js'` here means Task 3 missed a specifier: fix it in that package, not here. `next build` lists the routes `/`, `/auth/[...auth]` and `/healthz` as dynamic (`ƒ`). The standalone test prints `2 passed`. The Tailwind guard still finds both of its sentinels: `@source` scans the `@paigasus/ui` source, so the deleted `Table` page does not matter (spec § 3.3).

- [ ] **Step 12: Commit**

```bash
git add ts/apps/iam-console/lib/auth.ts ts/apps/iam-console/proxy.ts \
  'ts/apps/iam-console/app/auth/[...auth]/route.ts' 'ts/apps/iam-console/app/(public)/layout.tsx' \
  'ts/apps/iam-console/app/(public)/page.tsx' ts/apps/iam-console/app/healthz/route.ts \
  ts/apps/iam-console/app/layout.tsx ts/apps/iam-console/app/providers.tsx ts/apps/iam-console/moon.yml ts/moon.yml \
  ts/apps/iam-console/tests/unit/session-cookie.test.ts ts/apps/iam-console/tests/unit/proxy.test.ts \
  ts/apps/iam-console/tests/standalone-runtime.test.ts
git commit -F - <<'EOF'
feat(ts): iam-console auth routes, proxy and public landing page (SMA-511)

The zone mounts @paigasus/auth: one catch-all route handler that awaits
the process-wide runtime per request, and proxy.ts, which checks cookie
presence only. Its matcher skips static assets under the basePath.

The demo page is gone. /iam/ is the signed-out landing page and sends a
visitor who has the session cookie to /iam/orgs. The public
/iam/healthz answers the zone and the zone map after a full config
parse, and the standalone-runtime test now targets it with a complete
environment and asserts the mismatch message itself.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 11: Test doubles — fake IAM, fake IdP, TLS certificate, TLS terminator, MSW handlers

The integration tier (Tasks 12–19) and the e2e tier (Tasks 21–22) need IAM, an identity provider and an ingress without Docker (spec § 9.1, decision D3). This task writes the five doubles in `tests/support/` and a short self-test for each. Every double imports only Node built-ins, `@connectrpc/*`, `@bufbuild/protobuf`, `@paigasus/proto`, `jose` or `msw` — never a module that imports `server-only` — so the Playwright e2e harness (Task 21's worker-scoped fixture) can load them under plain Node.

**Files:**
- Create: `ts/apps/iam-console/tests/support/fake-iam.ts`
- Create: `ts/apps/iam-console/tests/support/tls.ts`
- Create: `ts/apps/iam-console/tests/support/https-client.ts`
- Create: `ts/apps/iam-console/tests/support/fake-idp.ts`
- Create: `ts/apps/iam-console/tests/support/tls-terminator.ts`
- Create: `ts/apps/iam-console/tests/support/msw.ts`
- Test: `ts/apps/iam-console/tests/integration/doubles/fake-iam.test.ts`
- Test: `ts/apps/iam-console/tests/integration/doubles/fake-idp.test.ts`
- Test: `ts/apps/iam-console/tests/integration/doubles/tls-terminator.test.ts`
- Test: `ts/apps/iam-console/tests/integration/doubles/tls.test.ts`
- Test: `ts/apps/iam-console/tests/integration/doubles/msw.test.ts`

**Interfaces:**
- Consumes: `TenancyService`, `AuthnService`, `AuthorizationService`, `AuditService` (`@paigasus/proto/iam`); `ServiceInfoService`, `ErrorInfoSchema` (`@paigasus/proto`); `connectNodeAdapter` (`@connectrpc/connect-node` 2.2.0); `ConnectError`, `Code`, `type ConnectRouter`, `type HandlerContext` (`@connectrpc/connect`); `SignJWT`, `generateKeyPair`, `exportJWK` (`jose`); `http`, `HttpResponse`, `type HttpHandler` (`msw`). The self-tests also use `createIamClient`, `disposeTransports`, `ServiceInfoService` (`@paigasus/sdk/iam`, after Task 6) and `mapError`, `ErrorReason`, `ErrorDomain` (`@paigasus/sdk/errors`).
- Produces (the contract, plus the per-method handler types, `FakeIamCall.correlationId`, `principalPrnFor(token)` and the `https://127.0.0.1:<port>` origins that this task adds):
  - `fake-iam.ts` → `startFakeIam(opts?)`, `denial(opts?)`, `type FakeIam`, `type FakeIamCall`, `type FakeIamHandlers`, `type FakeIamMethod`, `type FakeIamContext`, `type ServiceDescriptorBody`, `IAM_ERROR_DOMAIN`, `FAKE_IAM_ISSUER`.
  - `fake-idp.ts` → `startFakeIdp({ cert, subject? })`, `type FakeIdp`.
  - `tls.ts` → `testTls(opts?)`, `type TlsMaterial`. Every caller except `tls.test.ts` calls `testTls()` with no argument.
  - `tls-terminator.ts` → `startTlsTerminator({ target, tls })`.
  - `msw.ts` → `serviceInfoHandlers(httpUrl, body)`.
  - `https-client.ts` → `httpsRequest(url, tls, init?)`, `type HttpsResponse` (for self-tests and e2e helpers).

> Handoff to Task 21: the e2e harness gets its environment from these doubles: `PAIGASUS_OIDC_ISSUER=idp.issuer`, `PAIGASUS_OIDC_CLIENT_ID=idp.clientId`, `PAIGASUS_OIDC_CLIENT_SECRET=idp.clientSecret`, `PAIGASUS_PUBLIC_ORIGIN=terminator.origin`, `PAIGASUS_SERVICES={"iam":"<fake.httpUrl>"}`, `PAIGASUS_IAM_GRPC_URL=fake.grpcUrl`, `NODE_EXTRA_CA_CERTS=testTls().certPath` (set before the server process starts), `PAIGASUS_SESSION_STORE=memory`, `PAIGASUS_ZONES={"iam":"/iam"}`. The issuer and the origin are `https://127.0.0.1:<port>`. The browser needs `ignoreHTTPSErrors: true`. Wait for `html[data-hydrated="true"]` before a click (Task 10's `Providers` sets it).

- [ ] **Step 1: Write the failing self-test for the fake IAM**

Create `ts/apps/iam-console/tests/integration/doubles/fake-iam.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The fake IAM's self-test: each IAM behaviour the console depends on (tests/support/fake-iam.ts's
// header lists them) is observable through the REAL SDK transport, over h2c.
import { afterAll, afterEach, beforeEach, describe, expect, it } from 'vitest';
import { ConnectError } from '@connectrpc/connect';
import { ErrorDomain, ErrorReason, mapError } from '@paigasus/sdk/errors';
import { AuthnService, createIamClient, disposeTransports, ServiceInfoService, TenancyService } from '@paigasus/sdk/iam';
import { denial, startFakeIam, type FakeIam } from '../../support/fake-iam';

const DENIED = 'prn:pgs:iam::0192f1c0-0000-7000-8000-000000000002:organization/0192f1c0-0000-7000-8000-000000000002';
const ALLOWED = 'prn:pgs:iam::0192f1c0-0000-7000-8000-000000000009:organization/0192f1c0-0000-7000-8000-000000000009';
const SENT_ID = '0198f2c1-8888-7000-8000-000000000042';

async function rejection(promise: Promise<unknown>): Promise<ConnectError> {
  try {
    await promise;
  } catch (err) {
    if (err instanceof ConnectError) return err;
    throw err;
  }
  throw new Error('expected the call to fail');
}

describe('the fake IAM', () => {
  let fake: FakeIam;

  beforeEach(async () => {
    fake = await startFakeIam();
  });
  afterEach(() => fake.close());
  afterAll(() => disposeTransports());

  it('serves a scripted answer and records the call with its token', async () => {
    fake.setHandlers({ 'tenancy.getOrganization': (req) => ({ organization: { prn: req.prn, slug: 'acme', name: 'Acme' } }) });
    const tenancy = createIamClient(TenancyService, { baseUrl: fake.grpcUrl }, { bearer: 'token-a' });
    const answer = await tenancy.getOrganization({ prn: ALLOWED });
    expect(answer.organization?.name).toBe('Acme');
    expect(fake.callsTo('tenancy.getOrganization')).toEqual([expect.objectContaining({ token: 'token-a', correlationId: null })]);
  });

  it('builds a denial the SDK maps to forbidden, carrying the correlation id it ADOPTED', async () => {
    fake.setHandlers({
      'tenancy.getOrganization': (req) => {
        if (req.prn === DENIED) throw denial();
        return { organization: { prn: req.prn } };
      },
    });
    const tenancy = createIamClient(TenancyService, { baseUrl: fake.grpcUrl }, { bearer: 'token-a' });
    const error = mapError({ kind: 'grpc', error: await rejection(tenancy.getOrganization({ prn: DENIED }, { headers: { 'paigasus-correlation-id': SENT_ID } })) });
    expect(error).toMatchObject({ presentation: 'forbidden', reason: ErrorReason.FORBIDDEN, rawReason: 'forbidden', domain: ErrorDomain.IAM, retryable: false, correlationId: SENT_ID });
    expect(fake.callsTo('tenancy.getOrganization')[0]?.correlationId).toBe(SENT_ID);
  });

  it('mints a correlation id when the incoming one is not a UUID', async () => {
    fake.setHandlers({
      'tenancy.getOrganization': () => {
        throw denial();
      },
    });
    const tenancy = createIamClient(TenancyService, { baseUrl: fake.grpcUrl }, { bearer: 'token-a' });
    const error = mapError({ kind: 'grpc', error: await rejection(tenancy.getOrganization({ prn: DENIED }, { headers: { 'paigasus-correlation-id': 'not-a-uuid' } })) });
    expect(error.correlationId).toMatch(/^[0-9a-f-]{36}$/);
    expect(error.correlationId).not.toBe('not-a-uuid');
  });

  it('answers identity-not-provisioned until the token makes a bearer-enforced call', async () => {
    const authn = createIamClient(AuthnService, { baseUrl: fake.grpcUrl }, { anonymous: true });
    const before = mapError({ kind: 'grpc', error: await rejection(authn.introspect({ token: 'token-b' })) });
    expect(before).toMatchObject({ presentation: 'forbidden', reason: ErrorReason.IDENTITY_NOT_PROVISIONED });

    const info = createIamClient(ServiceInfoService, { baseUrl: fake.grpcUrl }, { bearer: 'token-b' });
    expect((await info.getServiceInfo({})).serviceInfo?.service).toBe('iam');
    expect(fake.provisioned.has('token-b')).toBe(true);

    const after = await authn.introspect({ token: 'token-b' });
    expect(after.principalPrn).toBe(fake.principalPrnFor('token-b'));
    expect(after.roleGrants).toEqual([]);
  });

  it('keeps role_grants empty even when a script returns some', async () => {
    fake.provisioned.add('token-c');
    fake.setHandlers({ 'authn.introspect': () => ({ roleGrants: [{ scopePrn: ALLOWED, roleKey: 'org_admin' }], memberships: [{ id: 'm1', principalPrn: 'p', nodePrn: ALLOWED }] }) });
    const authn = createIamClient(AuthnService, { baseUrl: fake.grpcUrl }, { anonymous: true });
    const answer = await authn.introspect({ token: 'token-c' });
    expect(answer.roleGrants).toEqual([]);
    expect(answer.memberships.map((m) => m.nodePrn)).toEqual([ALLOWED]);
  });

  it('refuses a bearer-enforced call that carries no token', async () => {
    const tenancy = createIamClient(TenancyService, { baseUrl: fake.grpcUrl }, { anonymous: true });
    const error = mapError({ kind: 'grpc', error: await rejection(tenancy.listOrganizations({})) });
    expect(error.presentation).toBe('relogin');
  });

  it('serves GET /v1/service-info over HTTP, and a status override only there', async () => {
    const probe = (token: string | null) => fetch(`${fake.httpUrl}/v1/service-info`, { headers: token === null ? {} : { authorization: `Bearer ${token}` } });
    expect((await probe(null)).status).toBe(401);
    const ok = await probe('token-d');
    expect(ok.headers.get('content-type')).toBe('application/json');
    expect(await ok.json()).toEqual({ service: 'iam', version: '0.0.0-fake', capabilities: ['iam.authz.cedar', 'iam.audit'] });
    expect(fake.provisioned.has('token-d')).toBe(true);

    fake.setServiceInfo({ status: 503 });
    expect((await probe('token-d')).status).toBe(503);
    const info = createIamClient(ServiceInfoService, { baseUrl: fake.grpcUrl }, { bearer: 'token-d' });
    expect((await info.getServiceInfo({})).serviceInfo?.capabilities).toEqual(['iam.authz.cedar', 'iam.audit']);

    fake.setServiceInfo({ service: 'iam', version: '9.9.9', capabilities: [] });
    expect(await (await probe('token-d')).json()).toEqual({ service: 'iam', version: '9.9.9', capabilities: [] });
    expect(fake.callsTo('http.getServiceInfo')).toHaveLength(4);
  });
});
```

- [ ] **Step 2: Run it and see it fail**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/integration/doubles/fake-iam.test.ts
```

Expected: FAIL. `../../support/fake-iam` cannot be resolved.

- [ ] **Step 3: Write the fake IAM**

Create `ts/apps/iam-console/tests/support/fake-iam.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// An in-process fake of IAM for the integration and e2e tiers (spec § 9.1): a real gRPC server over
// h2c for the five services the console calls, and a plain HTTP server for `GET /v1/service-info`,
// which @paigasus/discovery probes.
//
// It copies these IAM behaviours, which the console depends on. Each one names its source:
//   - Introspect is exempt from bearer enforcement and never provisions. It answers
//     `identity-not-provisioned` until the token has made one bearer-enforced call
//     (rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:139-141, :178;
//     application/authenticate_token.rs:104-105).
//   - EVERY other RPC is bearer-enforced, and provisions the token (authn.rs:182-190). So does the
//     HTTP route (adapters/http/service_info.rs:10, auth_middleware.rs:54).
//   - Introspect always returns an empty `role_grants` (authenticate_token.rs:161-165).
//   - On a gRPC call, an incoming `paigasus-correlation-id` in the HYPHENATED UUID form (8-4-4-4-12
//     hex digits, any case) is adopted and echoed in lower case. Any other value, and a missing
//     header, gets a minted id. Every gRPC error carries the id in
//     `ErrorInfo.metadata["correlation_id"]` (adapters/grpc/convert.rs:59-74).
//   - A denial is `PermissionDenied` + `ErrorInfo(domain "iam.paigasus.io", reason "forbidden",
//     metadata { retryable: "false" })` (convert.rs:111-131).
//
// Where the fake does LESS than IAM. No current test depends on these differences. A new test that
// needs one of them must extend the fake first, or it tests the fake and not IAM:
//   - IAM adopts every form that `Uuid::parse_str` accepts: hyphenated, simple (32 hex digits),
//     braced and `urn:uuid:` (rs/crates/libs/paigasus-observability/src/correlation.rs:103-119). It
//     echoes the hyphenated lower-case form. The fake adopts only the hyphenated form, so it mints a
//     new id where IAM keeps a simple, braced or `urn:uuid:` id.
//   - IAM mints a UUIDv7 (correlation.rs:94-101). The fake mints a UUIDv4 (`randomUUID()`).
//   - IAM also puts `request_id` in `ErrorInfo.metadata` (convert.rs:69-72) and sets a
//     `paigasus-request-id` response header (correlation.rs:174). The fake sets no `request_id`.
//   - IAM's HTTP routes run the same correlation layer. The fake's HTTP route only records the
//     incoming header; it adopts, mints and echoes no id.
//
// It imports NO `server-only` module. The Playwright e2e harness (a worker-scoped fixture) loads it
// under plain Node, where `server-only` resolves to its throwing default export. That is why the service descriptors come
// from @paigasus/proto (allowed under tests/support/ by the `apps` boundary rule's ignore) and not
// from @paigasus/sdk.
import { randomUUID } from 'node:crypto';
import { createServer as createHttpServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { createServer as createHttp2Server, type ServerHttp2Session } from 'node:http2';
import type { AddressInfo } from 'node:net';
import type { DescMessage, DescService, MessageInitShape, MessageShape } from '@bufbuild/protobuf';
import { Code, ConnectError, type ConnectRouter, type HandlerContext } from '@connectrpc/connect';
import { connectNodeAdapter } from '@connectrpc/connect-node';
import { ErrorInfoSchema, ServiceInfoService } from '@paigasus/proto';
import { AuditService, AuthnService, AuthorizationService, TenancyService } from '@paigasus/proto/iam';

/** IAM's error domain on the wire (ErrorDomain.IAM through the registry's mapping rule). */
export const IAM_ERROR_DOMAIN = 'iam.paigasus.io';
/** The issuer the default Introspect answer reports. */
export const FAKE_IAM_ISSUER = 'https://idp.fake-iam.test';

const CORRELATION_HEADER = 'paigasus-correlation-id';
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const SERVICES = {
  tenancy: TenancyService,
  authn: AuthnService,
  authz: AuthorizationService,
  audit: AuditService,
  serviceInfo: ServiceInfoService,
} as const;

type ServiceMap = typeof SERVICES;
type ServiceKey = keyof ServiceMap;
type MethodsOf<S extends ServiceKey> = ServiceMap[S]['method'];

type Entry = {
  [S in ServiceKey]: {
    [M in Extract<keyof MethodsOf<S>, string>]: { key: `${S}.${M}`; desc: MethodsOf<S>[M] };
  }[Extract<keyof MethodsOf<S>, string>];
}[ServiceKey];

/** Every RPC the fake serves, as `<client key>.<method localName>` — the same keys as `IamClients`. */
export type FakeIamMethod = Entry['key'];

/** What a scripted handler receives besides the request. */
export type FakeIamContext = {
  /** The bearer token of the call, or null when the call carried none. */
  readonly token: string | null;
  /** The correlation id IAM adopted or minted for this call. */
  readonly correlationId: string;
};

type HandlerFor<D> = D extends { input: infer I extends DescMessage; output: infer O extends DescMessage }
  ? (request: MessageShape<I>, context: FakeIamContext) => MessageInitShape<O> | Promise<MessageInitShape<O>>
  : never;

/** Scripted answers, keyed by method. A handler may throw — use `denial()` for an IAM denial. */
export type FakeIamHandlers = { [E in Entry as E['key']]?: HandlerFor<E['desc']> };

export type FakeIamCall = {
  /** A `FakeIamMethod`, or `http.getServiceInfo` for the HTTP route. */
  readonly method: string;
  readonly token: string | null;
  /** The `paigasus-correlation-id` request header as it ARRIVED, or null. */
  readonly correlationId: string | null;
  readonly request: unknown;
};

export type ServiceDescriptorBody = { service: string; version: string; capabilities: string[] };

export type FakeIam = {
  readonly grpcUrl: string;
  readonly httpUrl: string;
  readonly calls: FakeIamCall[];
  callsTo(method: string): FakeIamCall[];
  /** Tokens that have made a bearer-enforced call, gRPC or HTTP. */
  readonly provisioned: Set<string>;
  /** The principal PRN the default Introspect answer reports for this token. Stable per token. */
  principalPrnFor(token: string): string;
  /**
   * REPLACES the whole handler map; it does not merge. A method missing from the new map falls back
   * to the built-in behaviour. Each fake is its own instance with no module state, so a test worker
   * can start, script and close one per test or per worker.
   */
  setHandlers(handlers: FakeIamHandlers): void;
  /**
   * A descriptor changes both the gRPC and the HTTP answer. `{ status }` changes ONLY the HTTP
   * route (the discovery probe): the gRPC GetServiceInfo keeps the last descriptor, because it is
   * also the provisioning call and a degraded-discovery scenario must not fail every first login.
   */
  setServiceInfo(descriptor: ServiceDescriptorBody | { status: number }): void;
  close(): Promise<void>;
};

const DEFAULT_DESCRIPTOR: ServiceDescriptorBody = { service: 'iam', version: '0.0.0-fake', capabilities: ['iam.authz.cedar', 'iam.audit'] };

/** Calls IAM serves WITHOUT bearer enforcement (authn.rs:139-141). */
const UNENFORCED: ReadonlySet<string> = new Set(['authn.introspect', 'authn.introspectApiKey']);

/** A ConnectError shaped the way IAM's `iam_status` shapes one (convert.rs:76-80). */
function iamError(code: Code, reason: string, message: string, correlationId?: string): ConnectError {
  const metadata: Record<string, string> = { retryable: 'false' };
  if (correlationId !== undefined) metadata['correlation_id'] = correlationId;
  return new ConnectError(message, code, undefined, [{ desc: ErrorInfoSchema, value: { reason, domain: IAM_ERROR_DOMAIN, metadata } }]);
}

/**
 * An IAM denial: `PermissionDenied` + ErrorInfo(domain `iam.paigasus.io`, reason `forbidden`,
 * metadata `{ retryable: "false" }`). When `correlationId` is omitted, the fake fills in the id of
 * the call that throws it, exactly as IAM does inside a request scope.
 */
export function denial(opts: { reason?: string; correlationId?: string; code?: Code } = {}): ConnectError {
  return iamError(opts.code ?? Code.PermissionDenied, opts.reason ?? 'forbidden', 'the principal is not allowed to perform this action', opts.correlationId);
}

function bearerOf(value: string | null | undefined): string | null {
  if (value === null || value === undefined) return null;
  const match = /^Bearer (.+)$/.exec(value);
  return match?.[1] ?? null;
}

/** Adds `correlation_id` to every ErrorInfo detail that lacks one. */
function stampCorrelation(err: unknown, correlationId: string): unknown {
  if (!(err instanceof ConnectError)) return err;
  for (const detail of err.details) {
    if (!('desc' in detail) || detail.desc.typeName !== ErrorInfoSchema.typeName) continue;
    const value = detail.value as { metadata?: Record<string, string> };
    const metadata = { ...(value.metadata ?? {}) };
    metadata['correlation_id'] ??= correlationId;
    detail.value = { ...detail.value, metadata };
  }
  return err;
}

function listen(server: { listen(port: number, host: string, cb: () => void): unknown; address(): AddressInfo | string | null }): Promise<string> {
  return new Promise((resolve, reject) => {
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (address === null || typeof address === 'string') {
        reject(new Error('fake IAM: could not read the listening port'));
        return;
      }
      resolve(`http://127.0.0.1:${String(address.port)}`);
    });
  });
}

export async function startFakeIam(opts: { handlers?: FakeIamHandlers } = {}): Promise<FakeIam> {
  let handlers: FakeIamHandlers = opts.handlers ?? {};
  let grpcDescriptor: ServiceDescriptorBody = DEFAULT_DESCRIPTOR;
  let httpAnswer: ServiceDescriptorBody | { status: number } = DEFAULT_DESCRIPTOR;
  const calls: FakeIamCall[] = [];
  const provisioned = new Set<string>();
  const principals = new Map<string, string>();

  const principalPrnFor = (token: string): string => {
    let prn = principals.get(token);
    if (prn === undefined) {
      prn = `prn:pgs:iam:::principal/${randomUUID()}`;
      principals.set(token, prn);
    }
    return prn;
  };

  const scripted = (method: string): ((request: unknown, context: FakeIamContext) => unknown) | undefined =>
    (handlers as Record<string, ((request: unknown, context: FakeIamContext) => unknown) | undefined>)[method];

  async function introspect(request: { token: string }, context: FakeIamContext): Promise<unknown> {
    if (!provisioned.has(request.token)) {
      throw iamError(Code.PermissionDenied, 'identity-not-provisioned', 'the identity is not provisioned');
    }
    const handler = scripted('authn.introspect');
    const extra = handler === undefined ? {} : ((await handler(request, context)) as object);
    return {
      principalPrn: principalPrnFor(request.token),
      status: 'active',
      issuer: FAKE_IAM_ISSUER,
      subject: `subject-of-${principalPrnFor(request.token).slice(-12)}`,
      memberships: [],
      ...extra,
      roleGrants: [],
    };
  }

  function defaults(method: string): unknown {
    switch (method) {
      case 'serviceInfo.getServiceInfo':
        return { serviceInfo: { ...grpcDescriptor } };
      case 'authz.isAuthorized':
        return { allowed: true, determiningPolicies: [], reason: '' };
      case 'authz.listRoleGrants':
        return { grants: [] };
      default:
        throw new ConnectError(`fake IAM: no handler is scripted for ${method}`, Code.Unimplemented);
    }
  }

  async function dispatch(method: string, request: unknown, ctx: HandlerContext): Promise<unknown> {
    const token = bearerOf(ctx.requestHeader.get('authorization'));
    const incoming = ctx.requestHeader.get(CORRELATION_HEADER);
    const correlationId = incoming !== null && UUID_RE.test(incoming) ? incoming.toLowerCase() : randomUUID();
    ctx.responseHeader.set(CORRELATION_HEADER, correlationId);
    calls.push({ method, token, correlationId: incoming, request });
    const context: FakeIamContext = { token, correlationId };
    try {
      if (!UNENFORCED.has(method)) {
        if (token === null) throw iamError(Code.Unauthenticated, 'missing-authorization', 'a bearer token is required');
        provisioned.add(token);
      }
      if (method === 'authn.introspect') return await introspect(request as { token: string }, context);
      const handler = scripted(method);
      return handler === undefined ? defaults(method) : await handler(request, context);
    } catch (err) {
      throw stampCorrelation(err, correlationId);
    }
  }

  function routes(router: ConnectRouter): void {
    for (const [serviceKey, service] of Object.entries(SERVICES) as [ServiceKey, DescService][]) {
      const impl: Record<string, (request: unknown, ctx: HandlerContext) => Promise<unknown>> = {};
      for (const method of service.methods) {
        impl[method.localName] = (request, ctx) => dispatch(`${serviceKey}.${method.localName}`, request, ctx);
      }
      // The map is built from the descriptor itself, so every key is a real method of `service`;
      // TypeScript cannot follow that through Object.entries, hence the cast.
      router.service(service, impl as never);
    }
  }

  const sessions = new Set<ServerHttp2Session>();
  const grpcServer = createHttp2Server(connectNodeAdapter({ routes }));
  grpcServer.on('session', (session) => {
    sessions.add(session);
    session.once('close', () => sessions.delete(session));
  });

  function handleHttp(req: IncomingMessage, res: ServerResponse): void {
    const url = new URL(req.url ?? '/', 'http://fake-iam.invalid');
    const json = (status: number, body: unknown): void => {
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(body));
    };
    if (req.method !== 'GET' || url.pathname !== '/v1/service-info') {
      json(404, { error: { code: 'not-found', message: 'no such route' } });
      return;
    }
    const token = bearerOf(req.headers.authorization);
    const header = req.headers[CORRELATION_HEADER];
    calls.push({ method: 'http.getServiceInfo', token, correlationId: typeof header === 'string' ? header : null, request: null });
    if (token === null) {
      json(401, { error: { code: 'missing-authorization', message: 'a bearer token is required' } });
      return;
    }
    provisioned.add(token);
    if ('status' in httpAnswer) {
      json(httpAnswer.status, { error: { code: 'internal', message: 'the fake IAM is configured to fail' } });
      return;
    }
    json(200, httpAnswer);
  }
  const httpServer = createHttpServer(handleHttp);

  const [grpcUrl, httpUrl] = await Promise.all([listen(grpcServer), listen(httpServer)]);

  return {
    grpcUrl,
    httpUrl,
    calls,
    callsTo: (method) => calls.filter((call) => call.method === method),
    provisioned,
    principalPrnFor,
    setHandlers(next) {
      handlers = next;
    },
    setServiceInfo(next) {
      httpAnswer = next;
      if (!('status' in next)) grpcDescriptor = next;
    },
    async close() {
      for (const session of sessions) session.destroy();
      await Promise.all([new Promise<void>((resolve) => grpcServer.close(() => resolve())), new Promise<void>((resolve) => httpServer.close(() => resolve()))]);
    },
  };
}
```

- [ ] **Step 4: Run the self-test and see it pass**

Run the command of Step 2 again.

Expected: `Tests  7 passed (7)`. The denial case proves the whole trailer path: `mapError` reads `presentation: 'forbidden'`, `reason: ErrorReason.FORBIDDEN`, `domain: ErrorDomain.IAM`, `retryable: false` and the correlation id that the call SENT, which the fake adopted.

- [ ] **Step 5: Write the failing self-tests for the IdP, the terminator and the certificate helper**

Create `ts/apps/iam-console/tests/integration/doubles/fake-idp.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { createHash, randomBytes } from 'node:crypto';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createLocalJWKSet, jwtVerify, type JSONWebKeySet } from 'jose';
import { startFakeIdp, type FakeIdp } from '../../support/fake-idp';
import { httpsRequest } from '../../support/https-client';
import { testTls, type TlsMaterial } from '../../support/tls';

const REDIRECT = 'https://127.0.0.1:9/iam/auth/callback';

describe('the fake IdP', () => {
  let tls: TlsMaterial;
  let idp: FakeIdp;

  beforeAll(async () => {
    tls = testTls();
    idp = await startFakeIdp({ cert: tls });
  });
  afterAll(() => idp.close());

  async function authorize(redirectUri: string, verifier: string): Promise<string> {
    const url = new URL(`${idp.issuer}/authorize`);
    url.search = new URLSearchParams({
      response_type: 'code',
      client_id: idp.clientId,
      redirect_uri: redirectUri,
      scope: 'openid',
      state: 'state-1',
      nonce: 'nonce-1',
      code_challenge: createHash('sha256').update(verifier).digest('base64url'),
      code_challenge_method: 'S256',
    }).toString();
    const res = await httpsRequest(url.toString(), tls);
    expect(res.status).toBe(302);
    const location = new URL(String(res.headers.location));
    expect(location.searchParams.get('state')).toBe('state-1');
    return location.searchParams.get('code') ?? '';
  }

  function exchange(code: string, redirectUri: string, verifier: string) {
    return httpsRequest(`${idp.issuer}/token`, tls, {
      method: 'POST',
      headers: { 'content-type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams({ grant_type: 'authorization_code', code, redirect_uri: redirectUri, code_verifier: verifier, client_id: idp.clientId, client_secret: idp.clientSecret }).toString(),
    });
  }

  it('serves a discovery document whose issuer is its own https origin', async () => {
    const res = await httpsRequest(`${idp.issuer}/.well-known/openid-configuration`, tls);
    expect(res.status).toBe(200);
    expect((JSON.parse(res.body) as { issuer: string }).issuer).toBe(idp.issuer);
    expect(idp.issuer.startsWith('https://')).toBe(true);
  });

  it('issues a signed id_token for a PKCE exchange with the same redirect_uri', async () => {
    const verifier = randomBytes(32).toString('base64url');
    const code = await authorize(REDIRECT, verifier);
    const res = await exchange(code, REDIRECT, verifier);
    expect(res.status).toBe(200);
    const body = JSON.parse(res.body) as { access_token: string; refresh_token: string; id_token: string; expires_in: number };
    const jwks = JSON.parse((await httpsRequest(`${idp.issuer}/jwks`, tls)).body) as JSONWebKeySet;
    const { payload } = await jwtVerify(body.id_token, createLocalJWKSet(jwks), { issuer: idp.issuer, audience: idp.clientId });
    expect(payload.nonce).toBe('nonce-1');
    expect(body.expires_in).toBe(3600);
    expect(idp.issued.at(-1)).toEqual({ accessToken: body.access_token, refreshToken: body.refresh_token });
    expect(idp.authorizeRedirectUris.at(-1)).toBe(REDIRECT);
    expect(idp.tokenRedirectUris.at(-1)).toBe(REDIRECT);
  });

  it('rejects an exchange whose redirect_uri differs from the authorization request', async () => {
    const verifier = randomBytes(32).toString('base64url');
    const code = await authorize(REDIRECT, verifier);
    const res = await exchange(code, 'https://127.0.0.1:9/auth/callback', verifier);
    expect(res.status).toBe(400);
    expect(JSON.parse(res.body)).toMatchObject({ error: 'invalid_grant' });
  });

  it('rejects a wrong PKCE verifier', async () => {
    const code = await authorize(REDIRECT, randomBytes(32).toString('base64url'));
    const res = await exchange(code, REDIRECT, randomBytes(32).toString('base64url'));
    expect(res.status).toBe(400);
  });

  it('rotates a refresh token and refuses the old one', async () => {
    const verifier = randomBytes(32).toString('base64url');
    const first = JSON.parse((await exchange(await authorize(REDIRECT, verifier), REDIRECT, verifier)).body) as { refresh_token: string };
    const refresh = (token: string) =>
      httpsRequest(`${idp.issuer}/token`, tls, {
        method: 'POST',
        headers: { 'content-type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({ grant_type: 'refresh_token', refresh_token: token, client_id: idp.clientId, client_secret: idp.clientSecret }).toString(),
      });
    expect((await refresh(first.refresh_token)).status).toBe(200);
    expect((await refresh(first.refresh_token)).status).toBe(400);
  });
});
```

Create `ts/apps/iam-console/tests/integration/doubles/tls-terminator.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { httpsRequest } from '../../support/https-client';
import { startTlsTerminator } from '../../support/tls-terminator';
import { testTls, type TlsMaterial } from '../../support/tls';

describe('the TLS terminator', () => {
  let tls: TlsMaterial;
  let upstream: Server;
  let terminator: { origin: string; close(): Promise<void> };

  beforeAll(async () => {
    tls = testTls();
    upstream = createServer((req, res) => {
      const chunks: Buffer[] = [];
      req.on('data', (chunk: Buffer) => chunks.push(chunk));
      req.on('end', () => {
        res.setHeader('set-cookie', ['a=1; Path=/', 'b=2; Path=/']);
        res.writeHead(201, { 'content-type': 'application/json' });
        res.end(
          JSON.stringify({
            method: req.method,
            url: req.url,
            host: req.headers.host,
            proto: req.headers['x-forwarded-proto'],
            forwardedHost: req.headers['x-forwarded-host'],
            body: Buffer.concat(chunks).toString('utf8'),
          }),
        );
      });
    });
    await new Promise<void>((resolve) => upstream.listen(0, '127.0.0.1', resolve));
    const { port } = upstream.address() as AddressInfo;
    terminator = await startTlsTerminator({ target: `http://127.0.0.1:${String(port)}`, tls });
  });
  afterAll(async () => {
    await terminator.close();
    await new Promise<void>((resolve) => upstream.close(() => resolve()));
  });

  it('keeps Host, sets the forwarded headers, and forwards the method, path and body', async () => {
    const res = await httpsRequest(`${terminator.origin}/iam/orgs?x=1`, tls, { method: 'POST', headers: { 'content-type': 'text/plain' }, body: 'hello' });
    expect(res.status).toBe(201);
    const seen = JSON.parse(res.body) as Record<string, string>;
    const publicHost = new URL(terminator.origin).host;
    expect(seen).toEqual({ method: 'POST', url: '/iam/orgs?x=1', host: publicHost, proto: 'https', forwardedHost: publicHost, body: 'hello' });
  });

  it('passes every Set-Cookie header through', async () => {
    const res = await httpsRequest(`${terminator.origin}/`, tls);
    expect(res.headers['set-cookie']).toEqual(['a=1; Path=/', 'b=2; Path=/']);
  });
});
```

Create `ts/apps/iam-console/tests/integration/doubles/tls.test.ts`. It proves the property that the parallel test workers rely on: a process that replaces an expiring pair never deletes or changes the pair that another process already holds. Two single-line mutations of Step 6's `tls.ts` turn it red (measured on a scratch copy of this code, vitest 5.0.0). A `rmSync` of the published pair before `generate()` fails the second case with `ENOENT` on the held `cert.pem`. A missing `publish()` fails its last `toEqual`.

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The certificate helper's self-test. It uses its OWN root, so it never touches the pair that the
// other doubles use. The rotation case is the parallel-worker race in sequential form: a process
// that finds the published pair expiring must publish a new pair, and must not delete or change the
// pair that another process already holds.
import { createPrivateKey, X509Certificate } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import { testTls, type TlsMaterial } from '../../support/tls';

const root = mkdtempSync(path.join(os.tmpdir(), 'paigasus-iam-console-tls-selftest-'));
afterAll(() => rmSync(root, { recursive: true, force: true }));

/** The files on disk are the pair in memory, and the key belongs to the cert. */
function expectIntact(tls: TlsMaterial): void {
  expect(readFileSync(tls.certPath, 'utf8')).toBe(tls.cert);
  expect(readFileSync(tls.keyPath, 'utf8')).toBe(tls.key);
  expect(new X509Certificate(tls.cert).checkPrivateKey(createPrivateKey(tls.key))).toBe(true);
}

describe('testTls', () => {
  it('reuses the published pair while it stays valid', () => {
    const first = testTls({ root });
    const second = testTls({ root });
    expect(second).toEqual(first);
    expectIntact(second);
  });

  it('publishes a new pair when the current one expires, and leaves the held pair intact', () => {
    const held = testTls({ root });
    // Two days is longer than the one-day cert lives, so the published pair counts as expiring.
    const rotated = testTls({ root, minValidSeconds: 2 * 86_400 });
    expect(rotated.certPath).not.toBe(held.certPath);
    expect(rotated.cert).not.toBe(held.cert);
    expectIntact(held);
    expectIntact(rotated);
    // The pointer moved: a call with the default threshold now reuses the new pair.
    expect(testTls({ root })).toEqual(rotated);
  });
});
```

The test is sequential. It does not start parallel processes, so it does not prove the race-free publication by itself. That part comes from the design: a pair directory is complete before its name is published, and no code deletes or rewrites a published directory. A one-off stress run of this `tls.ts` (10 parallel processes, 30 calls, half of them forcing rotation) returned 30 intact pairs, and all 30 were still on disk at the end.

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/integration/doubles/fake-idp.test.ts tests/integration/doubles/tls-terminator.test.ts tests/integration/doubles/tls.test.ts
```

Expected: FAIL. `../../support/fake-idp`, `../../support/tls`, `../../support/https-client` and `../../support/tls-terminator` cannot be resolved.

- [ ] **Step 6: Write the certificate helper and the HTTPS client**

Create `ts/apps/iam-console/tests/support/tls.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// A self-signed certificate for the fake IdP and the TLS terminator. Based on
// ts/packages/paigasus-auth/tests/e2e/tls-fixture.ts, and COPIED rather than imported: one
// package's tests must not import another package's tests (spec § 9.1).
//
// Vitest runs test files in parallel processes, and Playwright runs parallel workers, so two
// processes can call testTls() at the same moment. The layout under the root is:
//   - `pair-<pid>-<random>/cert.pem` and `key.pem`: one pair per directory. A process generates
//     into a NEW directory that no other process knows about, and publishes it only when openssl
//     has written both files.
//   - `current`: a pointer file that holds the NAME of the published pair directory. A process
//     publishes by writing a scratch pointer and then one `renameSync` over `current`. The rename
//     replaces the file atomically, so a reader sees the old name or the new name, never a mix.
//
// What the code guarantees:
//   - testTls() never returns a half-written pair. It reads the pointer ONCE and then reads both
//     files from the directory that the pointer named.
//   - No code deletes, moves or rewrites a pair directory after it is published. An expired pair
//     stays where it is; only the pointer moves to the new pair. So `certPath` and `keyPath` hold
//     the same pair as the returned `cert` and `key`, even after a peer publishes a newer pair.
//     This matters for the e2e tier: it passes `certPath` to the standalone server as
//     NODE_EXTRA_CA_CERTS, which Node reads once, at process start.
//   - When two processes generate at the same moment, both publish, and the LAST rename wins the
//     pointer. Each process still returns its own complete pair. Each current caller uses one
//     TlsMaterial for both the server and the client that trusts it, so two pairs do no harm.
// What it does not guarantee: one pair per day (a race costs one extra pair), and a clean root.
// Old pair directories stay under os.tmpdir() until the OS cleans that directory.
import { execFileSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export type TlsMaterial = { certPath: string; keyPath: string; cert: string; key: string };

/** The root every caller uses. Only the self-test passes its own root. */
const DEFAULT_TLS_ROOT = path.join(os.tmpdir(), 'paigasus-iam-console-tls');

/** A published pair is reused only while it stays valid for this many more seconds. */
const MIN_VALID_SECONDS = 300;

const PAIR_PREFIX = 'pair-';

function pointerPath(root: string): string {
  return path.join(root, 'current');
}

/** The pair directory that the pointer names, or null when there is no usable pointer. */
function publishedPair(root: string): string | null {
  let name: string;
  try {
    name = readFileSync(pointerPath(root), 'utf8');
  } catch {
    return null;
  }
  // This module writes a bare directory name. Any other content is not a pointer it wrote.
  if (!name.startsWith(PAIR_PREFIX) || path.basename(name) !== name) return null;
  return path.join(root, name);
}

/**
 * `-checkend`: reuse a pair only while it stays valid for `minValidSeconds` more seconds. The cert
 * lives one day, so a plain existence check would reuse an expired cert on day two and fail with
 * an opaque TLS error.
 */
function stillValid(dir: string, minValidSeconds: number): boolean {
  const certPath = path.join(dir, 'cert.pem');
  if (!existsSync(certPath) || !existsSync(path.join(dir, 'key.pem'))) return false;
  try {
    execFileSync('openssl', ['x509', '-in', certPath, '-checkend', String(minValidSeconds), '-noout']);
    return true;
  } catch {
    return false;
  }
}

/** Writes a complete pair into a NEW directory and returns it. The directory is not published yet. */
function generate(root: string): string {
  const dir = mkdtempSync(path.join(root, `${PAIR_PREFIX}${String(process.pid)}-`));
  try {
    execFileSync(
      'openssl',
      [
        'req',
        '-x509',
        '-newkey',
        'rsa:2048',
        '-nodes',
        '-keyout',
        path.join(dir, 'key.pem'),
        '-out',
        path.join(dir, 'cert.pem'),
        '-days',
        '1',
        '-subj',
        '/CN=localhost',
        '-addext',
        'subjectAltName=DNS:localhost,IP:127.0.0.1',
      ],
      { stdio: 'ignore' },
    );
    if (!stillValid(dir, MIN_VALID_SECONDS)) throw new Error(`testTls: openssl wrote no valid certificate in ${dir}`);
    return dir;
  } catch (error) {
    // The directory is not published, so no other process knows its name. Removing it is safe.
    rmSync(dir, { recursive: true, force: true });
    throw error;
  }
}

/** Points `current` at `dir`: a scratch pointer, then ONE atomic rename over the real one. */
function publish(root: string, dir: string): void {
  const scratch = path.join(root, `current-${String(process.pid)}-${randomBytes(8).toString('hex')}.tmp`);
  writeFileSync(scratch, path.basename(dir));
  renameSync(scratch, pointerPath(root));
}

function load(dir: string): TlsMaterial {
  const certPath = path.join(dir, 'cert.pem');
  const keyPath = path.join(dir, 'key.pem');
  return { certPath, keyPath, cert: readFileSync(certPath, 'utf8'), key: readFileSync(keyPath, 'utf8') };
}

/**
 * The test certificate pair. It reuses the published pair while that pair stays valid for
 * `minValidSeconds` more seconds (default 300). Otherwise it generates a pair, publishes it and
 * returns it. `root` and `minValidSeconds` are for the self-test; other callers pass nothing.
 */
export function testTls(opts: { root?: string; minValidSeconds?: number } = {}): TlsMaterial {
  const root = opts.root ?? DEFAULT_TLS_ROOT;
  const minValidSeconds = opts.minValidSeconds ?? MIN_VALID_SECONDS;
  mkdirSync(root, { recursive: true });
  const current = publishedPair(root);
  if (current !== null && stillValid(current, minValidSeconds)) return load(current);
  const fresh = generate(root);
  publish(root, fresh);
  return load(fresh);
}
```

The `rmSync` in `generate()` is the only delete in the file. It removes a directory that failed before publication, so no other process can hold its name.

Create `ts/apps/iam-console/tests/support/https-client.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// A minimal HTTPS client that trusts the test certificate. Vitest workers start before any test
// could set NODE_EXTRA_CA_CERTS, so the global `fetch` cannot reach the fake IdP or the TLS
// terminator; `node:https` with an explicit `ca` can.
import { request } from 'node:https';
import type { TlsMaterial } from './tls';

export type HttpsResponse = { status: number; headers: Record<string, string | string[] | undefined>; body: string };

export function httpsRequest(url: string, tls: TlsMaterial, init: { method?: string; headers?: Record<string, string>; body?: string } = {}): Promise<HttpsResponse> {
  return new Promise((resolve, reject) => {
    const req = request(url, { method: init.method ?? 'GET', headers: init.headers ?? {}, ca: tls.cert }, (res) => {
      const chunks: Buffer[] = [];
      res.on('data', (chunk: Buffer) => chunks.push(chunk));
      res.on('end', () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks).toString('utf8') }));
      res.on('error', reject);
    });
    req.on('error', reject);
    if (init.body !== undefined) req.write(init.body);
    req.end();
  });
}
```

- [ ] **Step 7: Write the fake IdP**

Create `ts/apps/iam-console/tests/support/fake-idp.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// An in-process OIDC provider over HTTPS for the e2e tier (spec § 9.1). HTTPS because
// `authEnvShape` requires an `https:` PAIGASUS_OIDC_ISSUER (ts/packages/paigasus-auth/src/config.ts:22-27).
// Based on ts/packages/paigasus-auth/tests/fixtures/jwks.ts, copied rather than imported.
//
// It approves every authorization request at once, and it CHECKS what a real provider checks and
// what this app must get right under a basePath:
//   - /token recomputes the S256 PKCE challenge from `code_verifier`;
//   - /token requires `redirect_uri` to equal the value sent to /authorize (RFC 6749 § 4.1.3).
//     spec § 7.1 fixes the app side of exactly this, so a mismatch fails the exchange here.
import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { createServer, type Server } from 'node:https';
import type { IncomingMessage, ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { exportJWK, generateKeyPair, SignJWT } from 'jose';
import type { TlsMaterial } from './tls';

export type FakeIdp = {
  readonly issuer: string;
  readonly clientId: string;
  readonly clientSecret: string;
  /** Every `redirect_uri` /authorize received, in order. */
  readonly authorizeRedirectUris: string[];
  /** Every `redirect_uri` an authorization_code grant at /token received, in order. */
  readonly tokenRedirectUris: string[];
  /** Every token pair /token issued, in order (authorization_code and refresh_token grants). */
  readonly issued: { accessToken: string; refreshToken: string }[];
  close(): Promise<void>;
};

const CLIENT_ID = 'iam-console-e2e';
const CLIENT_SECRET = 'iam-console-e2e-secret';
const KID = 'fake-idp-key';

type PendingCode = { redirectUri: string; codeChallenge: string; nonce: string | null };

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on('data', (chunk: Buffer) => chunks.push(chunk));
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    req.on('error', reject);
  });
}

function json(res: ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  res.end(JSON.stringify(body));
}

function s256(verifier: string): string {
  return createHash('sha256').update(verifier).digest('base64url');
}

/** client_secret_post or client_secret_basic — openid-client picks one from the discovery document. */
function clientAuthenticated(req: IncomingMessage, form: URLSearchParams): boolean {
  const basic = req.headers.authorization;
  if (typeof basic === 'string' && basic.startsWith('Basic ')) {
    const [id, secret] = Buffer.from(basic.slice('Basic '.length), 'base64').toString('utf8').split(':');
    return decodeURIComponent(id ?? '') === CLIENT_ID && decodeURIComponent(secret ?? '') === CLIENT_SECRET;
  }
  return form.get('client_id') === CLIENT_ID && form.get('client_secret') === CLIENT_SECRET;
}

export async function startFakeIdp(opts: { cert: TlsMaterial; subject?: string }): Promise<FakeIdp> {
  const subject = opts.subject ?? 'fake-user-1';
  const keys = await generateKeyPair('RS256', { extractable: true });
  const publicJwk = { ...(await exportJWK(keys.publicKey)), kid: KID, use: 'sig', alg: 'RS256' };

  const codes = new Map<string, PendingCode>();
  const refreshTokens = new Set<string>();
  const authorizeRedirectUris: string[] = [];
  const tokenRedirectUris: string[] = [];
  const issued: { accessToken: string; refreshToken: string }[] = [];
  let issuer = '';

  const mint = (): { accessToken: string; refreshToken: string } => {
    const pair = { accessToken: `fake-at-${randomBytes(16).toString('hex')}`, refreshToken: `fake-rt-${randomBytes(16).toString('hex')}` };
    refreshTokens.add(pair.refreshToken);
    issued.push(pair);
    return pair;
  };

  const idToken = (nonce: string | null): Promise<string> => {
    const jwt = new SignJWT({ email: 'ada@example.test', name: 'Ada Lovelace', ...(nonce === null ? {} : { nonce }) })
      .setProtectedHeader({ alg: 'RS256', kid: KID })
      .setIssuer(issuer)
      .setAudience(CLIENT_ID)
      .setSubject(subject)
      .setIssuedAt()
      .setExpirationTime('10m');
    return jwt.sign(keys.privateKey);
  };

  async function token(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const form = new URLSearchParams(await readBody(req));
    if (!clientAuthenticated(req, form)) {
      json(res, 401, { error: 'invalid_client' });
      return;
    }
    const grant = form.get('grant_type');
    if (grant === 'authorization_code') {
      const code = form.get('code') ?? '';
      const pending = codes.get(code);
      codes.delete(code); // single use
      const redirectUri = form.get('redirect_uri') ?? '';
      tokenRedirectUris.push(redirectUri);
      if (pending === undefined) {
        json(res, 400, { error: 'invalid_grant', error_description: 'unknown or used code' });
        return;
      }
      if (redirectUri !== pending.redirectUri) {
        json(res, 400, { error: 'invalid_grant', error_description: 'redirect_uri does not match the authorization request' });
        return;
      }
      const verifier = form.get('code_verifier');
      if (verifier === null || s256(verifier) !== pending.codeChallenge) {
        json(res, 400, { error: 'invalid_grant', error_description: 'PKCE verification failed' });
        return;
      }
      const pair = mint();
      json(res, 200, { access_token: pair.accessToken, refresh_token: pair.refreshToken, id_token: await idToken(pending.nonce), token_type: 'Bearer', expires_in: 3600 });
      return;
    }
    if (grant === 'refresh_token') {
      const presented = form.get('refresh_token') ?? '';
      if (!refreshTokens.has(presented)) {
        json(res, 400, { error: 'invalid_grant', error_description: 'unknown refresh token' });
        return;
      }
      refreshTokens.delete(presented); // rotation, as most providers do
      const pair = mint();
      // No id_token on refresh: openid-client then has nothing to cross-check against the first one.
      json(res, 200, { access_token: pair.accessToken, refresh_token: pair.refreshToken, token_type: 'Bearer', expires_in: 3600 });
      return;
    }
    json(res, 400, { error: 'unsupported_grant_type' });
  }

  async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const url = new URL(req.url ?? '/', issuer);
    if (req.method === 'GET' && url.pathname === '/.well-known/openid-configuration') {
      json(res, 200, {
        issuer,
        authorization_endpoint: `${issuer}/authorize`,
        token_endpoint: `${issuer}/token`,
        jwks_uri: `${issuer}/jwks`,
        revocation_endpoint: `${issuer}/revoke`,
        end_session_endpoint: `${issuer}/logout`,
        response_types_supported: ['code'],
        subject_types_supported: ['public'],
        id_token_signing_alg_values_supported: ['RS256'],
        code_challenge_methods_supported: ['S256'],
        token_endpoint_auth_methods_supported: ['client_secret_post', 'client_secret_basic'],
        grant_types_supported: ['authorization_code', 'refresh_token'],
      });
      return;
    }
    if (req.method === 'GET' && url.pathname === '/jwks') {
      json(res, 200, { keys: [publicJwk] });
      return;
    }
    if (req.method === 'GET' && url.pathname === '/authorize') {
      const redirectUri = url.searchParams.get('redirect_uri');
      const state = url.searchParams.get('state');
      const challenge = url.searchParams.get('code_challenge');
      if (url.searchParams.get('client_id') !== CLIENT_ID || redirectUri === null || state === null || challenge === null || url.searchParams.get('code_challenge_method') !== 'S256') {
        json(res, 400, { error: 'invalid_request' });
        return;
      }
      authorizeRedirectUris.push(redirectUri);
      const code = randomUUID();
      codes.set(code, { redirectUri, codeChallenge: challenge, nonce: url.searchParams.get('nonce') });
      const back = new URL(redirectUri);
      back.searchParams.set('code', code);
      back.searchParams.set('state', state);
      res.writeHead(302, { location: back.toString() });
      res.end();
      return;
    }
    if (req.method === 'POST' && url.pathname === '/token') {
      await token(req, res);
      return;
    }
    if (req.method === 'POST' && url.pathname === '/revoke') {
      const form = new URLSearchParams(await readBody(req));
      refreshTokens.delete(form.get('token') ?? '');
      res.writeHead(200);
      res.end();
      return;
    }
    if (req.method === 'GET' && url.pathname === '/logout') {
      const target = url.searchParams.get('post_logout_redirect_uri');
      if (target === null) {
        res.writeHead(200, { 'content-type': 'text/plain' });
        res.end('signed out');
        return;
      }
      const back = new URL(target);
      const state = url.searchParams.get('state');
      if (state !== null) back.searchParams.set('state', state);
      res.writeHead(302, { location: back.toString() });
      res.end();
      return;
    }
    json(res, 404, { error: 'not_found' });
  }

  const server: Server = createServer({ cert: opts.cert.cert, key: opts.cert.key }, (req, res) => {
    handle(req, res).catch(() => {
      if (!res.headersSent) res.writeHead(500);
      res.end();
    });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  // 127.0.0.1, not `localhost`: the certificate names both, the server listens on IPv4 only, and
  // `localhost` can resolve to ::1 first. The issuer string must equal the `iss` claim byte for
  // byte, so this one spelling is used everywhere.
  issuer = `https://127.0.0.1:${String(port)}`;

  return {
    issuer,
    clientId: CLIENT_ID,
    clientSecret: CLIENT_SECRET,
    authorizeRedirectUris,
    tokenRedirectUris,
    issued,
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.closeAllConnections();
        server.close((err) => (err ? reject(err) : resolve()));
      }),
  };
}
```

- [ ] **Step 8: Write the TLS terminator**

Create `ts/apps/iam-console/tests/support/tls-terminator.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The ingress stand-in for the e2e tier (spec § 9.4). The browser speaks HTTPS to this server; it
// forwards each request over plain HTTP to the standalone Next server.
//
// It keeps `Host` UNCHANGED and adds `X-Forwarded-Proto: https` and `X-Forwarded-Host`. That is
// the deployment contract of spec § 10: Next's Server Action origin check compares `Origin` with
// the forwarded host, and the auth routes build absolute URLs from the public origin.
import { request as httpRequest, type IncomingHttpHeaders, type IncomingMessage, type ServerResponse } from 'node:http';
import { createServer } from 'node:https';
import type { AddressInfo } from 'node:net';
import type { TlsMaterial } from './tls';

/** Hop-by-hop headers (RFC 9110 § 7.6.1). A proxy must not forward them. */
const HOP_BY_HOP = new Set(['connection', 'keep-alive', 'proxy-connection', 'te', 'trailer', 'upgrade']);

function forwardable(headers: IncomingHttpHeaders): IncomingHttpHeaders {
  const out: IncomingHttpHeaders = {};
  for (const [name, value] of Object.entries(headers)) {
    if (!HOP_BY_HOP.has(name) && value !== undefined) out[name] = value;
  }
  return out;
}

export async function startTlsTerminator(opts: { target: string; tls: TlsMaterial }): Promise<{ origin: string; close(): Promise<void> }> {
  const target = new URL(opts.target);

  function forward(req: IncomingMessage, res: ServerResponse): void {
    const host = req.headers.host ?? '';
    const upstream = httpRequest(
      {
        hostname: target.hostname,
        port: target.port,
        method: req.method,
        path: req.url,
        headers: { ...forwardable(req.headers), host, 'x-forwarded-proto': 'https', 'x-forwarded-host': host, 'x-forwarded-port': host.split(':')[1] ?? '443' },
      },
      (upstreamRes) => {
        res.writeHead(upstreamRes.statusCode ?? 502, forwardable(upstreamRes.headers));
        upstreamRes.pipe(res);
      },
    );
    upstream.on('error', () => {
      if (!res.headersSent) res.writeHead(502, { 'content-type': 'text/plain' });
      res.end('tls-terminator: the upstream did not answer');
    });
    req.pipe(upstream);
  }

  const server = createServer({ cert: opts.tls.cert, key: opts.tls.key }, forward);
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  return {
    origin: `https://127.0.0.1:${String(port)}`,
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.closeAllConnections();
        server.close((err) => (err ? reject(err) : resolve()));
      }),
  };
}
```

- [ ] **Step 9: Run the three self-tests and see them pass**

Run the command of Step 5 again.

Expected: `Test Files  3 passed (3)` and `Tests  9 passed (9)`. On a fresh machine, `openssl` writes a pair into `$TMPDIR/paigasus-iam-console-tls/pair-<pid>-<random>/`, and the pointer file `$TMPDIR/paigasus-iam-console-tls/current` names that directory. If the two IdP and terminator files start at the same moment, each can write its own pair. `tls.test.ts` uses its own root under `$TMPDIR` and removes it at the end.

- [ ] **Step 10: Write the failing MSW self-test**

Create `ts/apps/iam-console/tests/integration/doubles/msw.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The MSW handler's self-test: it answers only an authenticated probe, with the bare descriptor.
import { setupServer } from 'msw/node';
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest';
import { serviceInfoHandlers } from '../../support/msw';

const IAM_HTTP = 'http://iam.msw.test';
const server = setupServer();

beforeAll(() => server.listen({ onUnhandledRequest: 'error' }));
afterEach(() => server.resetHandlers());
afterAll(() => server.close());

describe('serviceInfoHandlers', () => {
  it('answers the descriptor as JSON to a request with a bearer token', async () => {
    server.use(...serviceInfoHandlers(IAM_HTTP, { service: 'iam', version: '1.2.3', capabilities: ['iam.audit'] }));
    const res = await fetch(`${IAM_HTTP}/v1/service-info`, { headers: { authorization: 'Bearer t' } });
    expect(res.status).toBe(200);
    expect(res.headers.get('content-type')).toContain('application/json');
    expect(await res.json()).toEqual({ service: 'iam', version: '1.2.3', capabilities: ['iam.audit'] });
  });

  it('answers 401 without a bearer token', async () => {
    server.use(...serviceInfoHandlers(IAM_HTTP, { service: 'iam', version: '1.2.3', capabilities: [] }));
    expect((await fetch(`${IAM_HTTP}/v1/service-info`)).status).toBe(401);
  });

  it('answers the configured status', async () => {
    server.use(...serviceInfoHandlers(IAM_HTTP, { status: 503 }));
    expect((await fetch(`${IAM_HTTP}/v1/service-info`, { headers: { authorization: 'Bearer t' } })).status).toBe(503);
  });
});
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/integration/doubles/msw.test.ts
```

Expected: FAIL. `../../support/msw` cannot be resolved.

- [ ] **Step 11: Write the MSW handlers**

Create `ts/apps/iam-console/tests/support/msw.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// MSW handlers for IAM's HTTP surface in the vitest tier (spec § 9.1, AC 5). MSW intercepts
// `fetch`, which is how @paigasus/discovery probes `GET /v1/service-info`. It cannot intercept the
// SDK's gRPC calls, which run over node:http2 — the fake IAM serves those.
//
// The handler requires a bearer token, as IAM's route does (adapters/http/service_info.rs:10), and
// answers the BARE descriptor as JSON (contracts/proto/paigasus/common/v1/service_info.proto:23-33).
import { http, HttpResponse, type HttpHandler } from 'msw';

export function serviceInfoHandlers(httpUrl: string, body: { service: string; version: string; capabilities: string[] } | { status: number }): HttpHandler[] {
  return [
    http.get(`${httpUrl}/v1/service-info`, ({ request }) => {
      if (!/^Bearer .+/.test(request.headers.get('authorization') ?? '')) {
        return HttpResponse.json({ error: { code: 'missing-authorization', message: 'a bearer token is required' } }, { status: 401 });
      }
      if ('status' in body) {
        return HttpResponse.json({ error: { code: 'internal', message: 'IAM is configured to fail' } }, { status: body.status });
      }
      return HttpResponse.json(body);
    }),
  ];
}
```

Run the command of Step 10 again.

Expected: `Tests  3 passed (3)`.

- [ ] **Step 12: Format the files, then run every double's self-test, the type-check and the lint**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write apps/iam-console/tests/support/fake-iam.ts apps/iam-console/tests/support/tls.ts \
  apps/iam-console/tests/support/https-client.ts apps/iam-console/tests/support/fake-idp.ts \
  apps/iam-console/tests/support/tls-terminator.ts apps/iam-console/tests/support/msw.ts \
  apps/iam-console/tests/integration/doubles/fake-iam.test.ts apps/iam-console/tests/integration/doubles/fake-idp.test.ts \
  apps/iam-console/tests/integration/doubles/tls-terminator.test.ts apps/iam-console/tests/integration/doubles/tls.test.ts \
  apps/iam-console/tests/integration/doubles/msw.test.ts
pnpm -C ts/apps/iam-console exec vitest run tests/integration/doubles
moon run iam-console-ts:typecheck
moon run ts:lint ts:fmt --force
```

Expected: `prettier --write` lists the eleven files; then `Test Files  5 passed (5)`, `Tests  19 passed (19)`; then exit 0. `ts:lint` accepts the `@paigasus/proto` imports in `tests/support/` because Task 7 exempts that directory from the `apps` boundary rule. It would refuse the same import in any other app file.

- [ ] **Step 13: Commit**

```bash
git add ts/apps/iam-console/tests/support/fake-iam.ts ts/apps/iam-console/tests/support/tls.ts \
  ts/apps/iam-console/tests/support/https-client.ts ts/apps/iam-console/tests/support/fake-idp.ts \
  ts/apps/iam-console/tests/support/tls-terminator.ts ts/apps/iam-console/tests/support/msw.ts \
  ts/apps/iam-console/tests/integration/doubles
git commit -F - <<'EOF'
test(ts): in-process fake IAM, IdP, TLS terminator and MSW handlers (SMA-511)

The fake IAM serves the five services over h2c and GET /v1/service-info
over HTTP. It behaves like IAM where the console depends on it:
Introspect answers identity-not-provisioned until a bearer-enforced
call, role_grants stay empty, a denial carries ErrorInfo with the
adopted correlation id, and every call is recorded.

The fake IdP is HTTPS, checks PKCE and requires the token-request
redirect_uri to equal the authorize one. The TLS terminator keeps Host
and adds the forwarded headers. Each double has its own self-test.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 12: IAM clients, the error model, the 403 view and the correlation id

This task writes the request-scoped IAM clients (spec § 4.3), the one call wrapper `callIam` (spec § 6.1), the error copy (spec § 6.5), the three error views, the 403 view and the three error boundaries (spec § 6.1, § 6.2). It also carries the correlation id from the proxy through IAM to the 403 view (spec § 6.2). Step 1 checks the IAM half in the Rust source; Step 19 measures the Next half on a real build and sets `FORBIDDEN_VIEW_CORRELATION` to `'header'` or `'fallback'` from the result.

**Files:**
- Modify: `ts/apps/iam-console/next.config.ts` (`experimental.authInterrupts`)
- Create: `ts/apps/iam-console/lib/correlation-header.ts`
- Create: `ts/apps/iam-console/lib/correlation.ts`
- Create: `ts/apps/iam-console/lib/errors.ts`
- Create: `ts/apps/iam-console/lib/iam-clients.ts`
- Create: `ts/apps/iam-console/lib/iam.ts`
- Modify: `ts/apps/iam-console/proxy.ts`
- Create: `ts/apps/iam-console/app/_components/error-copy.ts`
- Create: `ts/apps/iam-console/app/_components/error-reference.tsx`
- Create: `ts/apps/iam-console/app/_components/page-error.tsx`
- Create: `ts/apps/iam-console/app/_components/section-error.tsx`
- Create: `ts/apps/iam-console/app/_components/form-error.tsx`
- Create: `ts/apps/iam-console/app/(console)/forbidden.tsx`
- Create: `ts/apps/iam-console/app/(console)/error.tsx`
- Create: `ts/apps/iam-console/app/error.tsx`
- Create: `ts/apps/iam-console/app/global-error.tsx`
- Modify: `ts/apps/iam-console/moon.yml` (sdk inputs)
- Create, then delete in Step 20: `ts/apps/iam-console/app/(console)/layout.tsx` and `ts/apps/iam-console/app/(console)/correlation-probe/page.tsx` (the Step 19 measurement; Task 15 writes the real layout)
- Test: `ts/apps/iam-console/tests/unit/error-copy.test.ts`
- Test: `ts/apps/iam-console/tests/unit/correlation.test.ts`
- Test: `ts/apps/iam-console/tests/unit/call-iam.test.ts`
- Test: `ts/apps/iam-console/tests/unit/proxy.test.ts` (three more cases)
- Test: `ts/apps/iam-console/tests/unit/error-views.test.tsx`
- Test: `ts/apps/iam-console/tests/unit/error-boundaries.test.tsx`
- Test: `ts/apps/iam-console/tests/integration/error-info-round-trip.test.ts`

**Interfaces:**
- Consumes: `createIamClient`, `TenancyService`, `AuthnService`, `AuthorizationService`, `AuditService`, `ServiceInfoService` (`@paigasus/sdk/iam`; `ServiceInfoService` after Task 6); `mapError`, `type PaigasusError` (`@paigasus/sdk/errors`); `ErrorReason`, `type Presentation` (`@paigasus/sdk/errors/types`, the client-safe entry); `requireSession`, `type ResolvedSession` (`@paigasus/auth/server`); `type DescService` (`@bufbuild/protobuf`, a type-only import in `lib/iam-clients.ts`; Task 9 lists the package under `dependencies` because this `lib/` file imports it); `authRuntime` (Task 10); `getRuntimeConfig`, `logger` (Task 9); `startFakeIam`, `denial` (Task 11).
- Produces: `lib/correlation.ts` → `CORRELATION_HEADER`, `REQUEST_PATH_HEADER`, `requestCorrelationId()`, `requestPath()`, `FORBIDDEN_VIEW_CORRELATION: 'header' | 'fallback'` (set by the Step 19 measurement). `lib/errors.ts` → `type IamResult<T>`, `type ActionState`, `callIam(fn)`. `lib/iam.ts` → `type IamClients`, `createIamClients(opts)`, `iamClientsForToken(token, correlationId?)`, `currentSession()`, `iamClients()`, `sessionToken()`. `app/_components/error-copy.ts` → `PRESENTATION_COPY`, `FORM_REASON_COPY`, `formMessage(error)`. `PageError`, `SectionError` (async server components), `FormError` (client), `CorrelationReference`, `SignInAgain`. The 403 view carries `data-testid="forbidden-view"` and the id `data-testid="correlation-id"`.

> Handoff to Tasks 16–19: a page renders `if (!result.ok) return <PageError error={result.error} />;` for a page read and `<SectionError error={…} />` for a section read. A form renders `<FormError error={state?.ok === false ? state.error : null} />`. Do NOT add a `loading.tsx` under `app/(console)/`: its Suspense boundary would let Next commit status 200 before `forbidden()` throws, and the 403 status would be lost. In vitest, `revalidatePath` is the recorder in `tests/support/next-cache.ts` (`revalidatedPaths`), and React `cache()` does not memoize.

> Handoff to Task 22: read `FORBIDDEN_VIEW_CORRELATION` from `lib/correlation.ts` as text (the module imports `server-only`). In `'header'` mode, assert for a denied page read: (a) the document status is 403, (b) `[data-testid="forbidden-view"]` is visible inside the shell, and (c) `[data-testid="correlation-id"]` has exactly the text of the `correlationId` the fake IAM recorded for the denied call (`harness.iam.callsTo('tenancy.getOrganization').at(-1)?.correlationId`, or the RPC the page calls) — the id `proxy.ts` minted, which IAM adopted. In `'fallback'` mode, replace (c) with: `[data-testid="correlation-id"]` is absent, and the standalone server's stdout holds one `"event":"iam.call_failed"` line with `"correlation_id":"<that id>"` and `"path":"<the page path>"`. The section and form 403s show `[data-testid="correlation-id"]` in both modes.

- [ ] **Step 1: Confirm that IAM adopts an incoming correlation id**

Run:

```bash
sed -n '19,31p;103,119p' rs/crates/libs/paigasus-observability/src/correlation.rs
grep -n "CorrelationLayer" rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs
```

Expected: `CORRELATION_ID_HEADER: &str = "paigasus-correlation-id"` with the doc line "Adopted from the caller when it parses as a UUID, else minted" (lines 21–31), and `adopt_or_mint` (lines 103–119) returning the parsed UUID. `grpc/mod.rs` prints `.layer(CorrelationLayer)` (line 139). So IAM adopts the id the console sends, in its canonical hyphenated form, and puts it into `ErrorInfo.metadata["correlation_id"]` (`adapters/grpc/convert.rs:59-74`). `crypto.randomUUID()` gives that canonical lowercase form, so the id comes back unchanged. The header route of spec § 6.2 goes ahead.

- [ ] **Step 2: Switch on `authInterrupts`**

`forbidden()` throws unless `experimental.authInterrupts` is true (`next/dist/client/components/forbidden.js:27`). In `ts/apps/iam-console/next.config.ts`, replace:

```ts
  outputFileTracingRoot: fileURLToPath(new URL('../..', import.meta.url)),
});
```

with:

```ts
  outputFileTracingRoot: fileURLToPath(new URL('../..', import.meta.url)),
  // forbidden() and (console)/forbidden.tsx give the 403 view a real HTTP 403 (spec § 6.2, AC 2).
  // EXPERIMENTAL in Next 16.3.4: the e2e tier asserts the 403 status, so an upgrade that changes the
  // flag fails CI instead of silently rendering the view with status 200.
  extend: { experimental: { authInterrupts: true } },
});
```

If Task 8 already passes an `extend` object, add `experimental: { authInterrupts: true }` to that object instead.

- [ ] **Step 3: Write the failing error-copy test**

Create `ts/apps/iam-console/tests/unit/error-copy.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The error copy tables (spec § 6.5, § 9.2).
import { describe, expect, it } from 'vitest';
import { ErrorReason, type PaigasusError, type Presentation } from '@paigasus/sdk/errors/types';
import { FORM_REASON_COPY, formMessage, PRESENTATION_COPY } from '../../app/_components/error-copy';

const PRESENTATIONS: readonly Presentation[] = ['relogin', 'forbidden', 'not-found', 'degraded', 'rate-limited', 'invalid-input', 'conflict', 'disabled', 'generic'];

function errorWith(presentation: Presentation, reason: ErrorReason | null): PaigasusError {
  return {
    presentation,
    domain: null,
    reason,
    rawReason: null,
    rawDomain: null,
    message: 'IAM text that must never show',
    correlationId: 'cid-1',
    requestId: null,
    retryable: null,
    metadata: {},
    transport: { kind: 'transport', cause: 'network' },
  };
}

describe('the error copy', () => {
  it('has a title and a body for every presentation', () => {
    expect(Object.keys(PRESENTATION_COPY).sort()).toEqual([...PRESENTATIONS].sort());
    for (const presentation of PRESENTATIONS) {
      expect(PRESENTATION_COPY[presentation].title.length).toBeGreaterThan(0);
      expect(PRESENTATION_COPY[presentation].body.length).toBeGreaterThan(0);
    }
  });

  it('keys the form table only by real, non-sentinel ErrorReason values', () => {
    const keys = Object.keys(FORM_REASON_COPY).map(Number);
    expect(keys.length).toBeGreaterThan(0);
    for (const key of keys) {
      expect(ErrorReason[key]).toBeDefined();
      expect(key).not.toBe(ErrorReason.UNSPECIFIED);
    }
  });

  it('prefers the reason copy, and falls back to the presentation copy for an unknown reason', () => {
    expect(formMessage(errorWith('conflict', ErrorReason.SLUG_CONFLICT))).toBe(FORM_REASON_COPY[ErrorReason.SLUG_CONFLICT]);
    expect(formMessage(errorWith('conflict', null))).toBe(PRESENTATION_COPY.conflict.body);
    expect(formMessage(errorWith('generic', ErrorReason.INTERNAL))).toBe(PRESENTATION_COPY.generic.body);
  });

  it('never repeats the message IAM sent', () => {
    for (const presentation of PRESENTATIONS) {
      expect(formMessage(errorWith(presentation, null))).not.toContain('IAM text');
    }
  });

  it('gives a form the spec § 6.1 "not enabled" copy for a disabled capability, and other copy for an inactive principal', () => {
    expect(formMessage(errorWith('disabled', ErrorReason.CAPABILITY_DISABLED))).toBe('This feature is not enabled on this IAM.');
    const inactive = formMessage(errorWith('disabled', ErrorReason.PRINCIPAL_INACTIVE));
    expect(inactive).toBe('Your account is not active in IAM.');
    expect(inactive).not.toContain('not enabled');
  });
});
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/unit/error-copy.test.ts
```

Expected: FAIL. `../../app/_components/error-copy` cannot be resolved.

- [ ] **Step 4: Write the error copy**

Create `ts/apps/iam-console/app/_components/error-copy.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The user-facing words for every error (spec § 6.5). CLIENT-SAFE: no `server-only`, and the only
// import is @paigasus/sdk's guard-free ./errors/types entry (ts/packages/paigasus-sdk/src/errors/types.ts),
// so form-error.tsx (a client component) can use it.
//
// Two tables. PRESENTATION_COPY is total over Presentation, so a tenth presentation fails the
// type-check. FORM_REASON_COPY covers the reasons the five forms can get; a test asserts every key
// is a real ErrorReason. A reason with no entry — including one this build does not know, which
// the SDK reports as reason null — falls back to the presentation's copy (the version-skew rule).
// No copy ever repeats IAM's message.
import { ErrorReason, type PaigasusError, type Presentation } from '@paigasus/sdk/errors/types';

export const PRESENTATION_COPY: Record<Presentation, { title: string; body: string }> = {
  relogin: { title: 'Your session has ended', body: 'Sign in again to continue.' },
  forbidden: { title: 'You do not have access', body: 'IAM refused this request for your account.' },
  'not-found': { title: 'Not found', body: 'This item does not exist, or it was removed.' },
  degraded: { title: 'IAM is not available', body: 'IAM did not answer in time. Try again in a moment.' },
  'rate-limited': { title: 'Too many requests', body: 'Wait a moment, then try again.' },
  'invalid-input': { title: 'The request was not valid', body: 'Check the values and try again.' },
  conflict: { title: 'The request conflicts with the current state', body: 'Reload the page and try again.' },
  disabled: { title: 'This feature is not enabled on this IAM', body: 'An operator can enable it in the IAM configuration.' },
  generic: { title: 'Something went wrong', body: 'The request failed. Try again, and give the reference below to support if it fails again.' },
};

export const FORM_REASON_COPY: Partial<Record<ErrorReason, string>> = {
  [ErrorReason.SLUG_CONFLICT]: 'This slug is already in use. Choose another slug.',
  [ErrorReason.INVALID_SLUG]: 'Use lowercase letters, digits and single hyphens for the slug.',
  [ErrorReason.INVALID_NAME]: 'Enter a name.',
  [ErrorReason.INVALID_PRN]: 'Enter a valid principal PRN, for example prn:pgs:iam:::principal/<uuid>.',
  [ErrorReason.INVALID_UUID]: 'The identifier is not a valid UUID.',
  [ErrorReason.DUPLICATE_MEMBERSHIP]: 'This principal is already a member here.',
  [ErrorReason.MISSING_ORG_MEMBERSHIP]: 'Add this principal to the organization first.',
  [ErrorReason.PARENT_ARCHIVED]: 'The parent of this item is archived.',
  [ErrorReason.NODE_ARCHIVED]: 'This item is archived.',
  [ErrorReason.NOT_FOUND]: 'The item was not found. It may have been removed.',
  [ErrorReason.MISSING_REQUIRED_FIELD]: 'Fill in every required field.',
  [ErrorReason.FORBIDDEN]: 'You do not have permission to do this.',
  // The SDK maps TWO reasons to the `disabled` presentation (ts/packages/paigasus-sdk/src/errors/presentation.ts:49,74).
  // Spec § 6.1 gives a Server Action the same "not enabled" copy as a page, so a disabled capability
  // shows it. An inactive principal is not a disabled feature, so it gets its own sentence.
  [ErrorReason.CAPABILITY_DISABLED]: 'This feature is not enabled on this IAM.',
  [ErrorReason.PRINCIPAL_INACTIVE]: 'Your account is not active in IAM.',
};

/** The one sentence a form shows for an error. */
export function formMessage(error: PaigasusError): string {
  const byReason = error.reason === null ? undefined : FORM_REASON_COPY[error.reason];
  return byReason ?? PRESENTATION_COPY[error.presentation].body;
}
```

Run the command of Step 3 again. Expected: `Tests  5 passed (5)`.

- [ ] **Step 5: Write the failing correlation test**

Create `ts/apps/iam-console/tests/unit/correlation.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// lib/correlation.ts: the two request headers proxy.ts sets, read back defensively.
import { describe, expect, it } from 'vitest';
import { CORRELATION_HEADER, REQUEST_PATH_HEADER, requestCorrelationId, requestPath } from '../../lib/correlation';
import { setRequestHeaders } from '../support/next-headers';

describe('the request correlation helpers', () => {
  it('uses the header name IAM adopts', () => {
    expect(CORRELATION_HEADER).toBe('paigasus-correlation-id');
  });

  it('returns the id proxy.ts set', async () => {
    setRequestHeaders({ [CORRELATION_HEADER]: '0198f2c1-8888-7000-8000-000000000042' });
    expect(await requestCorrelationId()).toBe('0198f2c1-8888-7000-8000-000000000042');
  });

  it('treats an absent or non-UUID value as no id', async () => {
    expect(await requestCorrelationId()).toBeNull();
    setRequestHeaders({ [CORRELATION_HEADER]: '<script>' });
    expect(await requestCorrelationId()).toBeNull();
  });

  it('returns the request path only when it is an absolute path', async () => {
    setRequestHeaders({ [REQUEST_PATH_HEADER]: '/iam/orgs' });
    expect(await requestPath()).toBe('/iam/orgs');
    setRequestHeaders({ [REQUEST_PATH_HEADER]: 'https://elsewhere.test/' });
    expect(await requestPath()).toBeNull();
  });
});
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/unit/correlation.test.ts
```

Expected: FAIL. `../../lib/correlation` cannot be resolved.

- [ ] **Step 6: Write the two correlation modules**

Create `ts/apps/iam-console/lib/correlation-header.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The two request-header names proxy.ts writes, in a module that imports nothing but the
// `server-only` guard. proxy.ts imports THIS file and not ./correlation, so the proxy bundle never
// reaches `next/headers`. spec § 13 measured that `server-only` itself loads in proxy.ts.
import 'server-only';

/** IAM adopts an incoming value that parses as a UUID (paigasus-observability correlation.rs:103-119). */
export const CORRELATION_HEADER = 'paigasus-correlation-id';

/** The public path of the console request, with the basePath and without the query. */
export const REQUEST_PATH_HEADER = 'x-paigasus-request-path';
```

Create `ts/apps/iam-console/lib/correlation.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The per-request correlation id (spec § 6.2). proxy.ts mints a UUID into the REQUEST headers of
// every request it lets through; lib/iam.ts sends it to IAM as `paigasus-correlation-id`; IAM adopts
// it (rs/crates/libs/paigasus-observability/src/correlation.rs:103-119, applied to the gRPC server
// at rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs:139) and puts it into every error's
// ErrorInfo metadata. So the id a user reads on the 403 view is the id in IAM's logs.
//
// `forbidden()` takes no argument and a React cache() holder does not reach forbidden.tsx
// (measured, spec § 13 #2). A request header does: forbidden.tsx reads it with headers().
import 'server-only';
import { headers } from 'next/headers';
import { unstable_rethrow } from 'next/navigation';
import { CORRELATION_HEADER, REQUEST_PATH_HEADER } from './correlation-header';

export { CORRELATION_HEADER, REQUEST_PATH_HEADER };

/**
 * How the 403 view gets its correlation id (spec § 6.2). DECIDED BY MEASUREMENT, not by taste: the
 * SMA-511 plan's Task 12 builds the app and requests a page that calls forbidden(), and reads the
 * 403 body.
 *
 *   'header'   — the view read the id proxy.ts set, with headers(), and shows it.
 *   'fallback' — it could not; the view shows no id, and callIam's `iam.call_failed` log line
 *                (lib/errors.ts) carries IAM's correlation id and the request path instead.
 *
 * The e2e tier (Task 22) reads this constant to choose its assertion.
 */
export const FORBIDDEN_VIEW_CORRELATION: 'header' | 'fallback' = 'header';

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/**
 * One request header, or null. Null outside a request scope too: headers() throws there, and a
 * caller such as callIam must still work in a test or a background task. unstable_rethrow first,
 * so Next's own control-flow errors (dynamic-usage bailouts during a build) are never swallowed.
 */
async function requestHeader(name: string): Promise<string | null> {
  try {
    const value = (await headers()).get(name);
    return value === null || value === '' ? null : value;
  } catch (err) {
    unstable_rethrow(err);
    return null;
  }
}

/** The id proxy.ts set for this request, or null. A value that is not a UUID is treated as absent. */
export async function requestCorrelationId(): Promise<string | null> {
  const value = await requestHeader(CORRELATION_HEADER);
  return value !== null && UUID_RE.test(value) ? value : null;
}

/** The public path of this request (for example `/iam/orgs/<uuid>`), or null. */
export async function requestPath(): Promise<string | null> {
  const value = await requestHeader(REQUEST_PATH_HEADER);
  return value !== null && value.startsWith('/') ? value : null;
}
```

Run the command of Step 5 again. Expected: `Tests  4 passed (4)`.

- [ ] **Step 7: Write the failing `callIam` test**

Create `ts/apps/iam-console/tests/unit/call-iam.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// callIam (spec § 6.1, § 9.2): a ConnectError becomes data, EVERYTHING else is rethrown unchanged.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { notFound, redirect } from 'next/navigation';
import { callIam } from '../../lib/errors';
import { logger } from '../../lib/logger';
import { setRequestHeaders } from '../support/next-headers';

afterEach(() => {
  vi.restoreAllMocks();
});

describe('callIam', () => {
  it('returns the value of a successful call', async () => {
    expect(await callIam(() => Promise.resolve(42))).toEqual({ ok: true, value: 42 });
  });

  it.each([
    [Code.PermissionDenied, 'forbidden'],
    [Code.NotFound, 'not-found'],
    [Code.Unauthenticated, 'relogin'],
    [Code.InvalidArgument, 'invalid-input'],
    [Code.AlreadyExists, 'conflict'],
    [Code.Unavailable, 'degraded'],
    [Code.ResourceExhausted, 'rate-limited'],
    [Code.Internal, 'generic'],
  ] as const)('maps a ConnectError with code %s to the %s presentation', async (code, presentation) => {
    const result = await callIam(() => Promise.reject(new ConnectError('boom', code)));
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error.presentation).toBe(presentation);
  });

  it('rethrows a plain Error unchanged — the same object', async () => {
    const bug = new TypeError('a real bug');
    await expect(callIam(() => Promise.reject(bug))).rejects.toBe(bug);
  });

  it('rethrows Next’s control-flow errors, so redirect() and notFound() still navigate', async () => {
    await expect(callIam(() => Promise.resolve().then(() => redirect('/somewhere')))).rejects.toMatchObject({ digest: expect.stringContaining('NEXT_REDIRECT') as string });
    await expect(callIam(() => Promise.resolve().then(() => notFound()))).rejects.toMatchObject({ digest: 'NEXT_HTTP_ERROR_FALLBACK;404' });
  });

  it('logs one iam.call_failed line with both correlation ids and the path, and never the message', async () => {
    const spy = vi.spyOn(logger, 'appEvent').mockImplementation(() => undefined);
    setRequestHeaders({ 'paigasus-correlation-id': '0198f2c1-8888-7000-8000-000000000001', 'x-paigasus-request-path': '/iam/orgs' });
    await callIam(() => Promise.reject(new ConnectError('secret internal detail', Code.PermissionDenied)));
    expect(spy).toHaveBeenCalledTimes(1);
    const [name, fields] = spy.mock.calls[0] ?? [];
    expect(name).toBe('iam.call_failed');
    expect(fields).toEqual({
      presentation: 'forbidden',
      reason: null,
      code: 'PermissionDenied',
      correlation_id: null,
      request_correlation_id: '0198f2c1-8888-7000-8000-000000000001',
      path: '/iam/orgs',
    });
    expect(JSON.stringify(fields)).not.toContain('secret internal detail');
  });
});
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/unit/call-iam.test.ts
```

Expected: FAIL. `../../lib/errors` cannot be resolved.

- [ ] **Step 8: Write `lib/errors.ts`**

Create `ts/apps/iam-console/lib/errors.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The ONE wrapper around an SDK call (spec § 6.1). It turns a ConnectError into the SDK's
// PaigasusError and returns it as data. It RETHROWS EVERY OTHER ERROR UNCHANGED: Next's redirect(),
// notFound() and forbidden() are thrown errors, and so is a real bug; swallowing either would turn
// a navigation into an error page or hide the bug. The app branches on presentation, domain and
// reason only, never on message text (ADR-0019 E8).
import 'server-only';
import { ConnectError } from '@connectrpc/connect';
import { mapError, type PaigasusError } from '@paigasus/sdk/errors';
import { requestCorrelationId, requestPath } from './correlation';
import { logger } from './logger';

export type IamResult<T> = { ok: true; value: T } | { ok: false; error: PaigasusError };

/** What a Server Action returns to its form. PaigasusError is a plain object, so it crosses Flight. */
export type ActionState = { ok: true } | { ok: false; error: PaigasusError } | null;

export async function callIam<T>(fn: () => Promise<T>): Promise<IamResult<T>> {
  try {
    return { ok: true, value: await fn() };
  } catch (err) {
    if (!(err instanceof ConnectError)) throw err;
    const error = mapError({ kind: 'grpc', error: err });
    // One line per failed call. It carries the correlation id IAM put into the error AND the one
    // proxy.ts minted for this request, so an operator can join the console log to IAM's even when
    // the 403 view shows no id (the spec § 6.2 fallback).
    logger.appEvent('iam.call_failed', {
      presentation: error.presentation,
      reason: error.rawReason,
      code: error.transport.kind === 'grpc' ? error.transport.codeName : null,
      correlation_id: error.correlationId,
      request_correlation_id: await requestCorrelationId(),
      path: await requestPath(),
    });
    return { ok: false, error };
  }
}
```

Run the command of Step 7 again. Expected: `Tests  12 passed (12)`.

- [ ] **Step 9: Write the failing wire round-trip test**

Create `ts/apps/iam-console/tests/integration/error-info-round-trip.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The ErrorInfo trailer round trip over the WIRE (spec § 9.3). The SDK's own tests build the
// ConnectError in memory and say they do not test the wire (ts/packages/paigasus-sdk/tests/map-error.test.ts:10-15).
// Here a real h2c call reaches the fake IAM, which denies it the way IAM does, and callIam maps
// what came back. It also proves the correlation-id header route of spec § 6.2: the id the console
// sends is the id IAM puts into the error.
import { afterAll, afterEach, beforeEach, describe, expect, it } from 'vitest';
import { ErrorDomain, ErrorReason } from '@paigasus/sdk/errors';
import { disposeTransports } from '@paigasus/sdk/iam';
import { callIam } from '../../lib/errors';
import { createIamClients } from '../../lib/iam-clients';
import { denial, startFakeIam, type FakeIam } from '../support/fake-iam';

const ORG = 'prn:pgs:iam::0192f1c0-0000-7000-8000-000000000002:organization/0192f1c0-0000-7000-8000-000000000002';
const REQUEST_ID = '0198f2c1-8888-7000-8000-00000000beef';

describe('the ErrorInfo round trip', () => {
  let fake: FakeIam;

  beforeEach(async () => {
    fake = await startFakeIam({
      handlers: {
        'tenancy.getOrganization': () => {
          throw denial();
        },
      },
    });
  });
  afterEach(() => fake.close());
  afterAll(() => disposeTransports());

  it('maps a wire denial to forbidden, with the reason, the domain and the id the console sent', async () => {
    const clients = createIamClients({ baseUrl: fake.grpcUrl, token: 'token-a', correlationId: REQUEST_ID });
    const result = await callIam(() => clients.tenancy.getOrganization({ prn: ORG }));
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error).toMatchObject({ presentation: 'forbidden', reason: ErrorReason.FORBIDDEN, domain: ErrorDomain.IAM, retryable: false, correlationId: REQUEST_ID });
    expect(fake.callsTo('tenancy.getOrganization')).toEqual([expect.objectContaining({ token: 'token-a', correlationId: REQUEST_ID })]);
  });

  it('sends the correlation header from every one of the five clients', async () => {
    const clients = createIamClients({ baseUrl: fake.grpcUrl, token: 'token-a', correlationId: REQUEST_ID });
    // Some calls fail (the beforeEach handler denies getOrganization, and the fake has no default
    // for listAuditEntries). That does not matter here: the fake records each call as it ARRIVED,
    // before it runs a handler, so the header of every call is in fake.calls.
    await callIam(() => clients.tenancy.getOrganization({ prn: ORG }));
    await callIam(() => clients.serviceInfo.getServiceInfo({}));
    await callIam(() => clients.authn.introspect({ token: 'token-a' }));
    await callIam(() => clients.authz.isAuthorized({ principalPrn: fake.principalPrnFor('token-a'), action: 'ListOrganizations', resourcePrn: ORG }));
    await callIam(() => clients.audit.listAuditEntries({}));
    const sent = fake.calls.map((call) => [call.method, call.correlationId]);
    expect(sent).toEqual([
      ['tenancy.getOrganization', REQUEST_ID],
      ['serviceInfo.getServiceInfo', REQUEST_ID],
      ['authn.introspect', REQUEST_ID],
      ['authz.isAuthorized', REQUEST_ID],
      ['audit.listAuditEntries', REQUEST_ID],
    ]);
  });

  it('sends no correlation header when the request has none, and IAM then mints one', async () => {
    const clients = createIamClients({ baseUrl: fake.grpcUrl, token: 'token-a' });
    const result = await callIam(() => clients.tenancy.getOrganization({ prn: ORG }));
    expect(fake.calls[0]?.correlationId).toBeNull();
    expect(result.ok ? null : result.error.correlationId).toMatch(/^[0-9a-f-]{36}$/);
  });
});
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/integration/error-info-round-trip.test.ts
```

Expected: FAIL. `../../lib/iam-clients` cannot be resolved.

- [ ] **Step 10: Write the IAM clients**

Create `ts/apps/iam-console/lib/iam-clients.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The request-scoped IAM clients, with NO session lookup (spec § 4.3). lib/iam.ts adds the session;
// lib/auth.ts uses iamClientsForToken for the login callback, where no session exists yet. The two
// files are separate so lib/auth.ts and lib/iam.ts do not import each other.
//
// No client and no token lives past the request (ts/packages/paigasus-sdk/src/iam.ts:23-28). Only
// the SDK's transport is process-scoped.
import 'server-only';
import type { DescService } from '@bufbuild/protobuf';
import type { CallOptions, Client } from '@connectrpc/connect';
import { AuditService, AuthnService, AuthorizationService, createIamClient, ServiceInfoService, TenancyService } from '@paigasus/sdk/iam';
import { getRuntimeConfig } from './config';
import { CORRELATION_HEADER } from './correlation-header';

export type IamClients = {
  tenancy: Client<typeof TenancyService>;
  authn: Client<typeof AuthnService>;
  authz: Client<typeof AuthorizationService>;
  audit: Client<typeof AuditService>;
  serviceInfo: Client<typeof ServiceInfoService>;
};

/**
 * Adds `paigasus-correlation-id` to every call a client makes. A Proxy over the SDK's own Proxy:
 * it touches only `headers`, never `contextValues`, which the SDK refuses from a caller. A header
 * the caller set itself wins.
 */
function withCorrelation<S extends DescService>(client: Client<S>, correlationId: string | null): Client<S> {
  if (correlationId === null) return client;
  return new Proxy(client, {
    get(target, property, receiver) {
      const value: unknown = Reflect.get(target, property, receiver);
      if (typeof value !== 'function') return value;
      return (request: unknown, options?: CallOptions) => {
        const headers = new Headers(options?.headers);
        if (!headers.has(CORRELATION_HEADER)) headers.set(CORRELATION_HEADER, correlationId);
        return (value as (r: unknown, o: CallOptions) => unknown)(request, { ...options, headers });
      };
    },
  });
}

/** The five clients for one bearer token, over one base URL. Pure: tests call it with a fake IAM. */
export function createIamClients(opts: { baseUrl: string; token: string; correlationId?: string | null }): IamClients {
  const transport = { baseUrl: opts.baseUrl };
  const auth = { bearer: opts.token };
  const id = opts.correlationId ?? null;
  return {
    tenancy: withCorrelation(createIamClient(TenancyService, transport, auth), id),
    authn: withCorrelation(createIamClient(AuthnService, transport, auth), id),
    authz: withCorrelation(createIamClient(AuthorizationService, transport, auth), id),
    audit: withCorrelation(createIamClient(AuditService, transport, auth), id),
    serviceInfo: withCorrelation(createIamClient(ServiceInfoService, transport, auth), id),
  };
}

/** The clients for a token the caller already holds. No session lookup — used at login. */
export function iamClientsForToken(token: string, correlationId: string | null = null): IamClients {
  return createIamClients({ baseUrl: getRuntimeConfig().PAIGASUS_IAM_GRPC_URL, token, correlationId });
}
```

Create `ts/apps/iam-console/lib/iam.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The session-bound IAM clients (spec § 4.3). Every accessor is a React cache(), so it runs once
// per request and never longer: a client holds a bearer token, and a token must not outlive its
// request (ts/packages/paigasus-sdk/src/iam.ts:23-28).
//
// NOTE: outside a React server render (vitest, a plain script) cache() does NOT memoize — React's
// default build exports a pass-through (measured on react 19.2.8). Tests must not rely on it.
import 'server-only';
import { cache } from 'react';
import { requireSession, type ResolvedSession } from '@paigasus/auth/server';
import { authRuntime } from './auth';
import { requestCorrelationId } from './correlation';
import { iamClientsForToken, type IamClients } from './iam-clients';

export { createIamClients, iamClientsForToken } from './iam-clients';
export type { IamClients } from './iam-clients';

/** The session for this request. Redirects to login when there is none. */
export const currentSession: () => Promise<ResolvedSession> = cache(async () => requireSession(await authRuntime()));

/** The five IAM clients for this request, bound to the session's access token and correlation id. */
export const iamClients: () => Promise<IamClients> = cache(async () => {
  const session = await currentSession();
  return iamClientsForToken(session.accessToken, await requestCorrelationId());
});

/** The session's access token — the bearer that @paigasus/discovery probes with. */
export const sessionToken: () => Promise<string> = cache(async () => (await currentSession()).accessToken);
```

Run the command of Step 9 again. Expected: `Tests  3 passed (3)`. The first case is the ErrorInfo trailer round trip of spec § 9.3 over a real h2c connection; the second proves that all five clients send the correlation header.

- [ ] **Step 11: Extend the proxy test for the two request headers and the import list**

Replace `ts/apps/iam-console/tests/unit/proxy.test.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// proxy.ts under the real basePath (spec § 7.5, § 13 #1). A NextRequest built with
// `nextConfig: { basePath: '/iam' }` strips the basePath from nextUrl.pathname exactly as Next does.
import { readFileSync } from 'node:fs';
import { NextRequest } from 'next/server';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';
import { SESSION_COOKIE_NAME } from '../../lib/auth';
import { config, proxy } from '../../proxy';

const ORIGIN = 'https://console.example.test';
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

/**
 * Every module specifier a source file imports, sorted and without duplicates. TypeScript's own
 * pre-processor reads them: static, type-only, side-effect, re-export, dynamic and require forms,
 * and never an import inside a comment.
 */
function importsOf(relative: string): string[] {
  const source = readFileSync(new URL(relative, import.meta.url), 'utf8');
  return [...new Set(ts.preProcessFile(source, true, true).importedFiles.map((file) => file.fileName))].sort();
}

function request(path: string, init: { cookie?: boolean; headers?: Record<string, string> } = {}): NextRequest {
  const headers = new Headers(init.headers);
  if (init.cookie === true) headers.set('cookie', `${SESSION_COOKIE_NAME}=opaque-session-id`);
  return new NextRequest(`${ORIGIN}${path}`, { headers, nextConfig: { basePath: '/iam' } });
}

/** NextResponse.next({ request: { headers } }) carries each overridden request header as x-middleware-request-<name>. */
function forwarded(res: Response, name: string): string | null {
  return res.headers.get(`x-middleware-request-${name}`);
}

describe('proxy', () => {
  it.each(['/iam', '/iam/healthz', '/iam/auth/login', '/iam/auth/callback', '/iam/auth/logout', '/iam/auth/logout/callback'])('lets %s through with no cookie', (path) => {
    const res = proxy(request(path));
    expect(res.headers.get('location')).toBeNull();
  });

  it('sends a visitor with no cookie to login ONCE under the basePath, and keeps the basePath in returnTo', () => {
    const res = proxy(request('/iam/orgs?offset=50'));
    const location = new URL(res.headers.get('location') ?? '', ORIGIN);
    expect(location.pathname).toBe('/iam/auth/login');
    expect(location.searchParams.get('returnTo')).toBe('/iam/orgs?offset=50');
  });

  it('lets a request with the session cookie through, and checks presence only', () => {
    const res = proxy(request('/iam/orgs', { cookie: true }));
    expect(res.headers.get('location')).toBeNull();
  });

  it('mints a fresh UUID correlation id into the REQUEST headers, overwriting one the browser sent', () => {
    const first = proxy(request('/iam/orgs', { cookie: true, headers: { 'paigasus-correlation-id': 'chosen-by-the-browser' } }));
    const second = proxy(request('/iam/orgs', { cookie: true }));
    const a = forwarded(first, 'paigasus-correlation-id');
    const b = forwarded(second, 'paigasus-correlation-id');
    expect(a).toMatch(UUID_RE);
    expect(b).toMatch(UUID_RE);
    expect(a).not.toBe(b);
  });

  it('records the public path, with the basePath and without the query', () => {
    const res = proxy(request('/iam/orgs/abc?offset=50', { cookie: true }));
    expect(forwarded(res, 'x-paigasus-request-path')).toBe('/iam/orgs/abc');
  });

  it('excludes static assets through a matcher written WITHOUT the basePath (spec § 13 #4)', () => {
    expect(config.matcher).toEqual(['/((?!_next/static|_next/image|favicon.ico).*)']);
  });

  // The proxy's allowed imports, as a strict-equality list. `server-only` is a no-op in the proxy layer,
  // and paigasus/boundaries/app-middleware is a DENY list of direct specifiers. So one import added to
  // lib/correlation-header.ts (for example ./iam-clients, which reaches the sdk) would enter the
  // proxy bundle with no lint error. Here it fails.
  it('imports only the allowed modules in proxy.ts and lib/correlation-header.ts', () => {
    expect(importsOf('../../proxy.ts')).toEqual(['./lib/correlation-header', '@paigasus/auth/middleware', 'next/server']);
    expect(importsOf('../../lib/correlation-header.ts')).toEqual(['server-only']);
  });
});
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/unit/proxy.test.ts
```

Expected: FAIL in the three new cases. `x-middleware-request-paigasus-correlation-id` and `x-middleware-request-x-paigasus-request-path` are `null`. The import list of `proxy.ts` is `['@paigasus/auth/middleware', 'next/server']`: `./lib/correlation-header` is missing.

- [ ] **Step 12: Mint the correlation id in the proxy**

Replace `ts/apps/iam-console/proxy.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The zone's proxy (Next 16's name for middleware; spec § 3.3, § 7.5). Two jobs, and no more:
//
// 1. COOKIE PRESENCE ONLY (ADR-0017 decision 7). It imports @paigasus/auth/middleware, never
//    /server and never the sdk (paigasus/boundaries/app-middleware). A forged or expired cookie
//    passes here and is rejected by requireSession() in the (console) layout.
// 2. A per-request correlation id (spec § 6.2), minted into the REQUEST headers so that
//    lib/iam.ts can send it to IAM and forbidden.tsx can read it with headers().
//
// Public paths are basePath-RELATIVE: Next strips the basePath from req.nextUrl.pathname before
// the proxy sees it (spec § 13 #1).
import { NextResponse, type NextRequest } from 'next/server';
import { authRoutePaths, createAuthMiddleware } from '@paigasus/auth/middleware';
import { CORRELATION_HEADER, REQUEST_PATH_HEADER } from './lib/correlation-header';

const authMiddleware = createAuthMiddleware({
  publicPaths: [...authRoutePaths(), '/', '/healthz'],
  loginPath: '/auth/login',
});

export function proxy(req: NextRequest): NextResponse {
  const decision = authMiddleware(req);
  // A redirect to login is final. Anything else continues, with the two request headers set.
  if (decision.headers.has('location')) return decision;
  const headers = new Headers(req.headers);
  // ALWAYS a fresh id: a value the browser sent is overwritten, so the id in IAM's logs is one this
  // server chose.
  headers.set(CORRELATION_HEADER, crypto.randomUUID());
  headers.set(REQUEST_PATH_HEADER, `${req.nextUrl.basePath}${req.nextUrl.pathname}`);
  return NextResponse.next({ request: { headers } });
}

/*
 * Without a matcher the proxy runs for every CSS and JS file too, and a visitor with no cookie gets
 * a login redirect for each one (spec § 7.5). MEASURED (spec § 13 #4): this pattern, written
 * WITHOUT the /iam prefix, skips the static assets under the basePath.
 */
export const config = {
  matcher: ['/((?!_next/static|_next/image|favicon.ico).*)'],
};
```

Run the command of Step 11 again. Expected: `Tests  12 passed (12)`.

- [ ] **Step 13: Write the failing test for the error views and the 403 view**

Create `ts/apps/iam-console/tests/unit/error-views.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The three error views and the 403 view (spec § 6.1, § 6.2). Server components are async
// functions here, so a test awaits them and renders the element they return.
import type { ReactElement, ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { PaigasusError, Presentation } from '@paigasus/sdk/errors/types';
import Forbidden from '../../app/(console)/forbidden';
import { FORBIDDEN_VIEW_CORRELATION } from '../../lib/correlation';
import { FormError } from '../../app/_components/form-error';
import { PageError } from '../../app/_components/page-error';
import { SectionError } from '../../app/_components/section-error';
import { setRequestHeaders } from '../support/next-headers';

// next/link needs the App Router's context, which a static render does not have.
vi.mock('next/link', () => ({
  default: ({ href, children, ...rest }: { href: string; children?: ReactNode }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs/abc' }));

const CID = '0198f2c1-8888-7000-8000-00000000abcd';

function errorWith(presentation: Presentation): PaigasusError {
  return {
    presentation,
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: 'IAM text that must never show',
    correlationId: CID,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'grpc', code: 7, codeName: 'PermissionDenied' },
  };
}

function render(element: ReactElement): string {
  return renderToStaticMarkup(
    <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>
      {element}
    </ZoneProvider>,
  );
}

describe('PageError', () => {
  it('calls forbidden() for a forbidden read, so Next answers 403', async () => {
    await expect(PageError({ error: errorWith('forbidden') })).rejects.toMatchObject({ digest: 'NEXT_HTTP_ERROR_FALLBACK;403' });
  });

  it('calls notFound() for a not-found read', async () => {
    await expect(PageError({ error: errorWith('not-found') })).rejects.toMatchObject({ digest: 'NEXT_HTTP_ERROR_FALLBACK;404' });
  });

  it.each(['degraded', 'generic', 'conflict', 'invalid-input', 'rate-limited'] as const)('renders ErrorState with the correlation id for %s', async (presentation) => {
    const html = render(await PageError({ error: errorWith(presentation) }));
    expect(html).toContain(CID);
    expect(html).not.toContain('IAM text');
  });

  it('renders the "not enabled" EmptyState for disabled', async () => {
    const html = render(await PageError({ error: errorWith('disabled') }));
    expect(html).toContain('This feature is not enabled on this IAM');
  });

  it('renders a "Sign in again" LINK with a returnTo for relogin, never a redirect', async () => {
    setRequestHeaders({ 'x-paigasus-request-path': '/iam/orgs/abc' });
    const html = render(await PageError({ error: errorWith('relogin') }));
    expect(html).toContain('href="/iam/auth/login?returnTo=%2Fiam%2Forgs%2Fabc"');
  });
});

describe('SectionError', () => {
  it('renders a forbidden section inline, with the correlation id, and does not throw', async () => {
    const html = render(await SectionError({ error: errorWith('forbidden') }));
    expect(html).toContain('data-presentation="forbidden"');
    expect(html).toContain(CID);
  });

  it('renders a not-found section inline', async () => {
    const html = render(await SectionError({ error: errorWith('not-found') }));
    expect(html).toContain('data-presentation="not-found"');
  });
});

describe('FormError', () => {
  it('renders nothing without an error', () => {
    expect(render(<FormError error={null} />)).toBe('');
  });

  it('renders an alert with the copy and the correlation id, never IAM’s message', () => {
    const html = render(<FormError error={errorWith('forbidden')} />);
    expect(html).toContain('role="alert"');
    expect(html).toContain(CID);
    expect(html).not.toContain('IAM text');
  });

  it('keeps the basePath in the returnTo of its "Sign in again" link', () => {
    const html = render(<FormError error={errorWith('relogin')} />);
    expect(html).toContain('href="/iam/auth/login?returnTo=%2Fiam%2Forgs%2Fabc"');
  });
});

describe('the 403 view', () => {
  it('always shows the fixed copy and a way back', async () => {
    const html = render(await Forbidden());
    expect(html).toContain('data-testid="forbidden-view"');
    expect(html).toContain('href="/orgs"');
    expect(html).not.toContain('data-testid="correlation-id"');
  });

  // ONE case for both values of the constant, so no case is ever skipped. In 'header' mode it fails
  // when the view stops reading the header; in 'fallback' mode it fails when the view shows the id.
  it('shows the correlation id proxy.ts set in header mode, and no id in fallback mode (spec § 6.2)', async () => {
    setRequestHeaders({ 'paigasus-correlation-id': CID });
    const html = render(await Forbidden());
    if (FORBIDDEN_VIEW_CORRELATION === 'header') {
      expect(html).toContain(`data-testid="correlation-id">${CID}<`);
    } else {
      expect(html).not.toContain(CID);
    }
  });
});
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/unit/error-views.test.tsx
```

Expected: FAIL. The four component modules cannot be resolved.

- [ ] **Step 14: Write the shared pieces and the three error views**

Create `ts/apps/iam-console/app/_components/error-reference.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The small pieces every error view shares: the correlation id a user gives to support, and the
// "Sign in again" link. Server-safe and client-safe: it imports only @paigasus/app-shell's ZoneLink.
import type { ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';

export function CorrelationReference({ id }: { id: string | null }): ReactElement | null {
  if (id === null) return null;
  return (
    <p className="text-muted-foreground mt-2 text-xs">
      Reference: <code data-testid="correlation-id">{id}</code>
    </p>
  );
}

/**
 * A LINK, never an automatic redirect (spec § 6.4): IAM answering "unauthenticated" after
 * getSession() refreshed the token means a wrong configuration or a revoked token, and a redirect
 * would loop. The login route rejects a returnTo under the zone's own /auth/ paths (spec § 7.1).
 */
export function SignInAgain({ returnTo }: { returnTo: string | null }): ReactElement {
  const href = returnTo === null ? '/iam/auth/login' : `/iam/auth/login?returnTo=${encodeURIComponent(returnTo)}`;
  return (
    <ZoneLink href={href} className="mt-2 text-sm underline">
      Sign in again
    </ZoneLink>
  );
}
```

Create `ts/apps/iam-console/app/_components/page-error.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// A PAGE read failed (spec § 6.1, first column). SERVER component.
//
//   forbidden  -> forbidden(): Next renders (console)/forbidden.tsx with HTTP 403 (AC 2)
//   not-found  -> notFound()
//   disabled   -> EmptyState "This feature is not enabled on this IAM"
//   relogin    -> ErrorState + "Sign in again" (a link, never a redirect: spec § 6.4)
//   the rest   -> ErrorState + the correlation id
//
// forbidden() and notFound() are called FIRST, before any await, so they throw before the page has
// rendered anything. Do not add a loading.tsx under (console): its Suspense boundary would let
// Next commit a 200 before the throw, and the 403 status would be lost.
import type { ReactElement } from 'react';
import { forbidden, notFound } from 'next/navigation';
import type { PaigasusError } from '@paigasus/sdk/errors';
import { EmptyState, ErrorState } from '@paigasus/ui';
import { requestPath } from '../../lib/correlation';
import { PRESENTATION_COPY } from './error-copy';
import { CorrelationReference, SignInAgain } from './error-reference';

export async function PageError({ error }: { error: PaigasusError }): Promise<ReactElement> {
  if (error.presentation === 'forbidden') forbidden();
  if (error.presentation === 'not-found') notFound();
  const copy = PRESENTATION_COPY[error.presentation];
  if (error.presentation === 'disabled') {
    return (
      <section className="p-8" data-testid="page-error" data-presentation={error.presentation}>
        <EmptyState title={copy.title} description={copy.body} />
      </section>
    );
  }
  return (
    <section className="p-8" data-testid="page-error" data-presentation={error.presentation}>
      <ErrorState title={copy.title} description={copy.body} />
      {error.presentation === 'relogin' ? <SignInAgain returnTo={await requestPath()} /> : <CorrelationReference id={error.correlationId} />}
    </section>
  );
}
```

Create `ts/apps/iam-console/app/_components/section-error.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// A SECTION read failed (spec § 6.1, second column): one part of a page, for example "All
// organizations" or a member list. SERVER component. It NEVER throws: the rest of the page still
// renders. A forbidden section shows the 403 inline, WITH the correlation id, because the
// PaigasusError is right here (unlike the page-level view, spec § 6.2).
import type { ReactElement } from 'react';
import type { PaigasusError } from '@paigasus/sdk/errors';
import { EmptyState, ErrorState } from '@paigasus/ui';
import { requestPath } from '../../lib/correlation';
import { PRESENTATION_COPY } from './error-copy';
import { CorrelationReference, SignInAgain } from './error-reference';

export async function SectionError({ error }: { error: PaigasusError }): Promise<ReactElement> {
  const copy = PRESENTATION_COPY[error.presentation];
  if (error.presentation === 'disabled' || error.presentation === 'not-found') {
    return (
      <div data-testid="section-error" data-presentation={error.presentation}>
        <EmptyState title={copy.title} description={copy.body} />
      </div>
    );
  }
  return (
    <div data-testid="section-error" data-presentation={error.presentation}>
      <ErrorState title={copy.title} description={copy.body} />
      {error.presentation === 'relogin' ? <SignInAgain returnTo={await requestPath()} /> : <CorrelationReference id={error.correlationId} />}
    </div>
  );
}
```

Create `ts/apps/iam-console/app/_components/form-error.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// A Server Action failed (spec § 6.1, third column). CLIENT component: a form renders it from its
// useActionState result. It shows the reason's copy when the form knows the reason, else the
// presentation's copy, and always the correlation id — never IAM's message.
'use client';

import type { ReactElement } from 'react';
import { usePathname } from 'next/navigation';
import { useZone } from '@paigasus/app-shell';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { formMessage } from './error-copy';
import { CorrelationReference, SignInAgain } from './error-reference';

export function FormError({ error }: { error: PaigasusError | null }): ReactElement | null {
  const pathname = usePathname();
  const { basePath } = useZone();
  if (error === null) return null;
  return (
    <div role="alert" data-testid="form-error" data-presentation={error.presentation} className="text-destructive mt-2 text-sm">
      <p>{formMessage(error)}</p>
      {/* usePathname() has no basePath (SMA-510 spec F19), and returnTo must keep it. */}
      {error.presentation === 'relogin' ? <SignInAgain returnTo={`${basePath}${pathname}`} /> : <CorrelationReference id={error.correlationId} />}
    </div>
  );
}
```

- [ ] **Step 15: Write the 403 view**

Create `ts/apps/iam-console/app/(console)/forbidden.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The 403 view (spec § 6.2, AC 2). Next renders it, with HTTP 403, when a page under (console)
// calls forbidden() — PageError does that for a `forbidden` presentation. It renders INSIDE the
// (console) layout, so the shell stays (measured, spec § 13 #2). It needs
// experimental.authInterrupts (next.config.ts).
//
// It shows a fixed title, the request's correlation id and a way back. It never shows IAM's
// message. forbidden() takes no argument, so the id comes from the request header proxy.ts set;
// IAM adopted the same id, so it is the one in IAM's logs. FORBIDDEN_VIEW_CORRELATION records
// whether that route works (lib/correlation.ts); under 'fallback' the view shows no id.
import type { ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import { ErrorState } from '@paigasus/ui';
import { FORBIDDEN_VIEW_CORRELATION, requestCorrelationId } from '../../lib/correlation';
import { PRESENTATION_COPY } from '../_components/error-copy';
import { CorrelationReference } from '../_components/error-reference';

export default async function Forbidden(): Promise<ReactElement> {
  const correlationId = FORBIDDEN_VIEW_CORRELATION === 'header' ? await requestCorrelationId() : null;
  return (
    <section className="p-8" data-testid="forbidden-view">
      <ErrorState title={PRESENTATION_COPY.forbidden.title} description={PRESENTATION_COPY.forbidden.body} />
      <CorrelationReference id={correlationId} />
      <ZoneLink href="/iam/orgs" className="mt-4 inline-block text-sm underline">
        Back to your organizations
      </ZoneLink>
    </section>
  );
}
```

Run the command of Step 13 again. Expected: `Tests  16 passed (16)`, with no skipped test. `FORBIDDEN_VIEW_CORRELATION` starts as `'header'`, so the id case asserts that the view shows the id. Step 19 measures whether `'header'` is true.

- [ ] **Step 16: Write the failing test for the error boundaries, then the boundaries**

Create `ts/apps/iam-console/tests/unit/error-boundaries.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The three error boundaries (spec § 6.1): each shows fixed copy and Next's digest, never the
// error's message.
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import ConsoleError from '../../app/(console)/error';
import RootError from '../../app/error';
import GlobalError from '../../app/global-error';

const error = Object.assign(new Error('internal detail: db password rejected'), { digest: 'digest-123' });
const reset = (): void => undefined;

describe('the error boundaries', () => {
  it.each([
    ['(console)/error.tsx', () => renderToStaticMarkup(<ConsoleError error={error} reset={reset} />)],
    ['app/error.tsx', () => renderToStaticMarkup(<RootError error={error} reset={reset} />)],
    ['app/global-error.tsx', () => renderToStaticMarkup(<GlobalError error={error} />)],
  ])('%s shows the digest and hides the message', (_name, render) => {
    const html = render();
    expect(html).toContain('digest-123');
    expect(html).not.toContain('db password');
  });
});
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/unit/error-boundaries.test.tsx
```

Expected: FAIL. The three boundary modules cannot be resolved.

Create `ts/apps/iam-console/app/(console)/error.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The error boundary for the console PAGES (spec § 6.1). An error in (console)/layout.tsx itself is
// caught one level up, by app/error.tsx. Next requires an error boundary to be a client component.
// It shows Next's digest as the reference, never the error message: a message can carry internal
// detail, and in production Next replaces it anyway.
'use client';

import type { ReactElement } from 'react';
import { ErrorState } from '@paigasus/ui';
import { PRESENTATION_COPY } from '../_components/error-copy';

export default function ConsoleError({ error, reset }: { error: Error & { digest?: string }; reset: () => void }): ReactElement {
  return (
    <section className="p-8" data-testid="console-error">
      <ErrorState title={PRESENTATION_COPY.generic.title} description={PRESENTATION_COPY.generic.body} retry={reset} />
      {error.digest === undefined ? null : (
        <p className="text-muted-foreground mt-2 text-xs">
          Reference: <code>{error.digest}</code>
        </p>
      )}
    </section>
  );
}
```

Create `ts/apps/iam-console/app/error.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The error boundary for errors in the ROUTE-GROUP LAYOUTS ((console)/layout.tsx, (public)/layout.tsx),
// which their own segment's error.tsx does not catch (spec § 6.1). It renders outside every
// provider, so it uses a plain <a>, not ZoneLink.
'use client';

import type { ReactElement } from 'react';
import { ErrorState } from '@paigasus/ui';
import { PRESENTATION_COPY } from './_components/error-copy';

export default function RootError({ error, reset }: { error: Error & { digest?: string }; reset: () => void }): ReactElement {
  return (
    <main className="p-8" data-testid="root-error">
      <ErrorState title={PRESENTATION_COPY.generic.title} description={PRESENTATION_COPY.generic.body} retry={reset} />
      {error.digest === undefined ? null : (
        <p className="text-muted-foreground mt-2 text-xs">
          Reference: <code>{error.digest}</code>
        </p>
      )}
      <a href="/iam/" className="mt-4 inline-block text-sm underline">
        Go to the start page
      </a>
    </main>
  );
}
```

Create `ts/apps/iam-console/app/global-error.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The last boundary: an error in the ROOT layout (spec § 6.1). It replaces the root layout, so it
// renders its own <html> and <body>, and globals.css may not have loaded — hence inline styles.
'use client';

import type { ReactElement } from 'react';

export default function GlobalError({ error }: { error: Error & { digest?: string } }): ReactElement {
  return (
    <html lang="en">
      <body style={{ fontFamily: 'system-ui, sans-serif', padding: '2rem' }}>
        <h1>Something went wrong</h1>
        <p>The console could not load. Try again in a moment.</p>
        {error.digest === undefined ? null : (
          <p>
            Reference: <code>{error.digest}</code>
          </p>
        )}
        <a href="/iam/">Go to the start page</a>
      </body>
    </html>
  );
}
```

Run the test command again. Expected: `Tests  3 passed (3)`.

- [ ] **Step 17: Add the sdk inputs to Moon**

`lib/iam-clients.ts` and `lib/errors.ts` compile `@paigasus/sdk` now. In `ts/apps/iam-console/moon.yml`, replace every occurrence (three: `build`, `typecheck`, `test`) of:

```yaml
      - '/ts/packages/paigasus-app-shell/package.json'
```

with:

```yaml
      - '/ts/packages/paigasus-app-shell/package.json'
      - '/ts/packages/paigasus-sdk/src/**/*'
      - '/ts/packages/paigasus-sdk/package.json'
```

(With the Edit tool, use `replace_all: true`.)

- [ ] **Step 18: Run the app's gates**

Run Prettier on every file this task creates or edits, then the gates:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write apps/iam-console/next.config.ts apps/iam-console/proxy.ts apps/iam-console/moon.yml \
  apps/iam-console/lib/correlation-header.ts apps/iam-console/lib/correlation.ts \
  apps/iam-console/lib/errors.ts apps/iam-console/lib/iam-clients.ts apps/iam-console/lib/iam.ts \
  apps/iam-console/app/_components/error-copy.ts apps/iam-console/app/_components/error-reference.tsx \
  apps/iam-console/app/_components/page-error.tsx apps/iam-console/app/_components/section-error.tsx \
  apps/iam-console/app/_components/form-error.tsx 'apps/iam-console/app/(console)/forbidden.tsx' \
  'apps/iam-console/app/(console)/error.tsx' apps/iam-console/app/error.tsx apps/iam-console/app/global-error.tsx \
  apps/iam-console/tests/unit/error-copy.test.ts apps/iam-console/tests/unit/correlation.test.ts \
  apps/iam-console/tests/unit/call-iam.test.ts apps/iam-console/tests/unit/proxy.test.ts \
  apps/iam-console/tests/unit/error-views.test.tsx apps/iam-console/tests/unit/error-boundaries.test.tsx \
  apps/iam-console/tests/integration/error-info-round-trip.test.ts
moon run iam-console-ts:test iam-console-ts:typecheck
moon run ts:lint ts:fmt --force
```

Expected: exit 0. `next build` accepts `experimental.authInterrupts` and compiles `app/(console)/forbidden.tsx`. The boundary rule for the proxy (Task 7) accepts `proxy.ts`. It takes values only from `next/server` (`NextResponse`), `@paigasus/auth/middleware` and a relative `lib/correlation-header`, and `lib/correlation-header.ts` imports only `server-only` (the Global Constraints). The last case of `tests/unit/proxy.test.ts` asserts both import lists by strict equality, because the boundary rule is a deny list and cannot say "only".

- [ ] **Step 19: Measure whether the 403 view can read the header (the objective check for `FORBIDDEN_VIEW_CORRELATION`)**

Step 1 proved the IAM half of spec § 6.2. This step measures the Next half: does `headers()` inside `(console)/forbidden.tsx` see the id `proxy.ts` set? It uses a TEMPORARY page that calls `forbidden()` and a TEMPORARY `(console)` layout that gives the view its providers (Task 15 writes the real layout). Step 20 deletes both.

Create `ts/apps/iam-console/app/(console)/layout.tsx` (temporary):

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// TEMPORARY — SMA-511 Task 12 Step 19 measurement. Deleted in Step 20; Task 15 writes the real layout.
import type { ReactElement, ReactNode } from 'react';
import { connection } from 'next/server';
import { getPublicConfig } from '../../lib/config';
import { Providers } from '../providers';

export default async function ProbeLayout({ children }: { children: ReactNode }): Promise<ReactElement> {
  await connection();
  const { zone, zones } = getPublicConfig();
  return (
    <Providers zone={zone} zones={zones} session={null}>
      {children}
    </Providers>
  );
}
```

Create `ts/apps/iam-console/app/(console)/correlation-probe/page.tsx` (temporary):

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// TEMPORARY — SMA-511 Task 12 Step 19 measurement. Deleted in Step 20.
import { forbidden } from 'next/navigation';
import { connection } from 'next/server';

export default async function CorrelationProbe(): Promise<never> {
  await connection();
  forbidden();
}
```

Build, start the standalone server, and request the probe with a session cookie (the proxy checks presence only):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:build
node --input-type=module - <<'EOF'
import { spawn } from 'node:child_process';
const env = {
  ...process.env, PORT: '3917', HOSTNAME: '127.0.0.1',
  PAIGASUS_ZONE: 'iam', PAIGASUS_ZONES: '{"iam":"/iam"}',
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.test', PAIGASUS_OIDC_CLIENT_ID: 'probe', PAIGASUS_OIDC_CLIENT_SECRET: 'probe',
  PAIGASUS_PUBLIC_ORIGIN: 'https://console.example.test', PAIGASUS_SESSION_STORE: 'memory',
  PAIGASUS_SERVICES: '{"iam":"http://127.0.0.1:9"}', PAIGASUS_IAM_GRPC_URL: 'http://127.0.0.1:9',
};
const child = spawn(process.execPath, ['ts/apps/iam-console/.next/standalone/apps/iam-console/server.js'], { env, stdio: 'ignore' });
try {
  let res;
  for (let i = 0; i < 240 && res === undefined; i += 1) {
    try {
      res = await fetch('http://127.0.0.1:3917/iam/correlation-probe', { headers: { cookie: '__Host-pgs_sid=probe' } });
    } catch {
      await new Promise((r) => setTimeout(r, 250));
    }
  }
  if (res === undefined) throw new Error('the standalone server did not answer within 60 s');
  const html = await res.text();
  const id = /data-testid="correlation-id">([0-9a-f-]{36})</.exec(html)?.[1] ?? null;
  console.log(JSON.stringify({ status: res.status, forbiddenView: html.includes('data-testid="forbidden-view"'), correlationId: id }));
} finally {
  child.kill('SIGTERM');
}
EOF
```

Expected: one JSON line. `status` must be `403` and `forbiddenView` must be `true`; anything else means `authInterrupts` or the view is broken — stop and fix that first, because it is not an input to the decision. Then:

- `correlationId` is a UUID → the header route works. `FORBIDDEN_VIEW_CORRELATION` stays `'header'`.
- `correlationId` is `null` → `headers()` does not reach the view. In `ts/apps/iam-console/lib/correlation.ts`, change the line to `export const FORBIDDEN_VIEW_CORRELATION: 'header' | 'fallback' = 'fallback';`. The view then shows no id, and `callIam`'s `iam.call_failed` line carries the id and the path (spec § 6.2 fallback).

Keep the constant on ONE line in exactly this form: Task 22 reads it as text with `/export const FORBIDDEN_VIEW_CORRELATION(?::[^=]+)?= '(header|fallback)'/`, because the module imports `server-only`, which throws under Playwright.

- [ ] **Step 20: Remove the probe and run the gates again**

```bash
rm 'ts/apps/iam-console/app/(console)/correlation-probe/page.tsx' 'ts/apps/iam-console/app/(console)/layout.tsx'
rmdir 'ts/apps/iam-console/app/(console)/correlation-probe'
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write apps/iam-console/lib/correlation.ts
pnpm -C ts/apps/iam-console exec vitest run tests/unit/error-views.test.tsx
moon run iam-console-ts:test
moon run ts:fmt --force
```

Step 19 can edit `lib/correlation.ts`, so Prettier runs on it again before the commit. Expected: `Tests  16 passed (16)` in both modes, with no skipped test: the one 403-view id case branches on the constant. Then `iam-console-ts:test` and `ts:fmt` exit 0. `ls 'ts/apps/iam-console/app/(console)'` prints only `error.tsx` and `forbidden.tsx`.

- [ ] **Step 21: Commit**

```bash
git add ts/apps/iam-console/next.config.ts ts/apps/iam-console/proxy.ts ts/apps/iam-console/moon.yml \
  ts/apps/iam-console/lib/correlation-header.ts ts/apps/iam-console/lib/correlation.ts \
  ts/apps/iam-console/lib/errors.ts ts/apps/iam-console/lib/iam-clients.ts ts/apps/iam-console/lib/iam.ts \
  ts/apps/iam-console/app/_components/error-copy.ts ts/apps/iam-console/app/_components/error-reference.tsx \
  ts/apps/iam-console/app/_components/page-error.tsx ts/apps/iam-console/app/_components/section-error.tsx \
  ts/apps/iam-console/app/_components/form-error.tsx 'ts/apps/iam-console/app/(console)/forbidden.tsx' \
  'ts/apps/iam-console/app/(console)/error.tsx' ts/apps/iam-console/app/error.tsx ts/apps/iam-console/app/global-error.tsx \
  ts/apps/iam-console/tests/unit/error-copy.test.ts ts/apps/iam-console/tests/unit/correlation.test.ts \
  ts/apps/iam-console/tests/unit/call-iam.test.ts ts/apps/iam-console/tests/unit/proxy.test.ts \
  ts/apps/iam-console/tests/unit/error-views.test.tsx ts/apps/iam-console/tests/unit/error-boundaries.test.tsx \
  ts/apps/iam-console/tests/integration/error-info-round-trip.test.ts
git commit -F - <<'EOF'
feat(ts): iam-console IAM clients, error views and the 403 view (SMA-511)

callIam turns a ConnectError into the SDK's PaigasusError and rethrows
every other error unchanged, so redirect, notFound and forbidden still
navigate. PageError calls forbidden() for a denied page read, which
renders (console)/forbidden.tsx with a real HTTP 403.

proxy.ts mints a UUID correlation id into the request headers, the IAM
clients send it, and IAM adopts it (paigasus-observability
correlation.rs). FORBIDDEN_VIEW_CORRELATION records what a probe page
measured: whether the 403 view can read that id with headers(). callIam
also logs every failed call with both ids and the path.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 13: The principal — provisioning, `currentPrincipal()` and the login resolver

IAM's `Introspect` never provisions a principal, so the first call must be a bearer-enforced one (spec § 4.5). This task writes `introspectWithProvisioning`, the per-request `currentPrincipal()`, and the `PrincipalResolver` that the login callback calls. The resolver never fails the login.

**Files:**
- Create: `ts/apps/iam-console/lib/principal.ts`
- Create: `ts/apps/iam-console/lib/principal-resolver.ts`
- Modify: `ts/apps/iam-console/lib/auth.ts` (pass the resolver to `getAuthRuntime`)
- Test: `ts/apps/iam-console/tests/integration/principal.test.ts`

**Interfaces:**
- Consumes: `type PrincipalResolver`, `type ResolvedPrincipal` (`@paigasus/auth/server`, `ports/principal-resolver.ts:25-41`); `ErrorReason` (`@paigasus/sdk/errors`); `callIam`, `type IamResult` (Task 12); `iamClients`, `sessionToken`, `iamClientsForToken`, `createIamClients`, `type IamClients` (Task 12); `type ConsoleLogger`, `createJsonLogger` (Task 9); `startFakeIam`, `denial`, `FAKE_IAM_ISSUER` (Task 11).
- Produces: `lib/principal.ts` → `type Principal = { prn: string; memberships: readonly { nodePrn: string }[] }`, `introspectWithProvisioning(clients, token, opts?)`, `currentPrincipal()`. `lib/principal-resolver.ts` → `createIntrospectPrincipalResolver({ clientsForToken, logger, timeoutMs? })`.

- [ ] **Step 1: Write the failing provisioning tests**

Create `ts/apps/iam-console/tests/integration/principal.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Provisioning (spec § 4.5, § 9.3), against the fake IAM over the real SDK transport:
//   - the login resolver calls GetServiceInfo BEFORE Introspect, and never fails the login;
//   - introspectWithProvisioning retries ONCE after identity-not-provisioned.
import { afterAll, afterEach, beforeEach, describe, expect, it } from 'vitest';
import { ErrorReason } from '@paigasus/sdk/errors';
import { disposeTransports } from '@paigasus/sdk/iam';
import { createIamClients } from '../../lib/iam-clients';
import { createJsonLogger } from '../../lib/logger';
import { introspectWithProvisioning } from '../../lib/principal';
import { createIntrospectPrincipalResolver } from '../../lib/principal-resolver';
import { denial, FAKE_IAM_ISSUER, startFakeIam, type FakeIam } from '../support/fake-iam';

const ORG = 'prn:pgs:iam::0192f1c0-0000-7000-8000-000000000002:organization/0192f1c0-0000-7000-8000-000000000002';
const CLAIMS = { iss: 'https://idp.example.test', sub: 'user-1' };

function captureLogger() {
  const lines: string[] = [];
  return { logger: createJsonLogger((line) => lines.push(line)), events: () => lines.map((line) => JSON.parse(line) as { event: string; fields: Record<string, unknown> }) };
}

describe('provisioning', () => {
  let fake: FakeIam;
  const clientsFor = (token: string) => createIamClients({ baseUrl: fake.grpcUrl, token });

  beforeEach(async () => {
    fake = await startFakeIam();
  });
  afterEach(() => fake.close());
  afterAll(() => disposeTransports());

  describe('the login resolver', () => {
    it('provisions with GetServiceInfo first, then maps Introspect, reporting grants as unknown', async () => {
      fake.setHandlers({ 'authn.introspect': (req) => ({ memberships: [{ id: 'm-1', principalPrn: fake.principalPrnFor(req.token), nodePrn: ORG }] }) });
      const { logger, events } = captureLogger();
      const resolver = createIntrospectPrincipalResolver({ clientsForToken: clientsFor, logger });
      const principal = await resolver.resolve({ accessToken: 'first-login', idTokenClaims: CLAIMS });
      expect(fake.calls.map((call) => call.method)).toEqual(['serviceInfo.getServiceInfo', 'authn.introspect']);
      expect(principal).toEqual({
        principalPrn: fake.principalPrnFor('first-login'),
        issuer: FAKE_IAM_ISSUER,
        subject: expect.any(String) as string,
        memberships: [{ id: 'm-1', principalPrn: fake.principalPrnFor('first-login'), nodePrn: ORG }],
        roleGrants: [],
        grantsAvailable: false,
      });
      expect(events()).toEqual([]);
    });

    it('degrades to a null principal and logs principal.resolve_failed when IAM refuses', async () => {
      fake.setHandlers({
        'serviceInfo.getServiceInfo': () => {
          throw denial();
        },
      });
      const { logger, events } = captureLogger();
      const principal = await createIntrospectPrincipalResolver({ clientsForToken: clientsFor, logger }).resolve({ accessToken: 't', idTokenClaims: CLAIMS });
      expect(principal).toEqual({ principalPrn: null, issuer: CLAIMS.iss, subject: CLAIMS.sub, memberships: [], roleGrants: [], grantsAvailable: false });
      expect(events()).toContainEqual({ event: 'principal.resolve_failed', fields: { presentation: 'forbidden' }, time: expect.any(String) as string });
    });

    it('degrades within its short timeout when IAM does not answer', async () => {
      fake.setHandlers({ 'serviceInfo.getServiceInfo': () => new Promise(() => undefined) });
      const { logger, events } = captureLogger();
      const started = Date.now();
      const principal = await createIntrospectPrincipalResolver({ clientsForToken: clientsFor, logger, timeoutMs: 200 }).resolve({ accessToken: 't', idTokenClaims: CLAIMS });
      expect(Date.now() - started).toBeLessThan(2_000);
      expect(principal.principalPrn).toBeNull();
      expect(events().map((e) => e.fields['presentation'])).toEqual(['degraded']);
    });

    it('degrades when the client factory itself throws', async () => {
      const { logger } = captureLogger();
      const resolver = createIntrospectPrincipalResolver({
        clientsForToken: () => {
          throw new Error('no config');
        },
        logger,
      });
      expect((await resolver.resolve({ accessToken: 't', idTokenClaims: CLAIMS })).principalPrn).toBeNull();
    });
  });

  describe('introspectWithProvisioning', () => {
    it('retries ONCE after identity-not-provisioned, provisioning in between', async () => {
      const result = await introspectWithProvisioning(clientsFor('new-user'), 'new-user', { provisionFirst: false });
      expect(result).toEqual({ ok: true, value: { prn: fake.principalPrnFor('new-user'), memberships: [] } });
      expect(fake.calls.map((call) => call.method)).toEqual(['authn.introspect', 'serviceInfo.getServiceInfo', 'authn.introspect']);
    });

    it('makes one Introspect call for a provisioned user', async () => {
      fake.provisioned.add('known-user');
      await introspectWithProvisioning(clientsFor('known-user'), 'known-user');
      expect(fake.calls.map((call) => call.method)).toEqual(['authn.introspect']);
    });

    it('returns the second identity-not-provisioned as an error instead of looping', async () => {
      // The fake provisions a token BEFORE it runs the handler, so this handler undoes it: the
      // provisioning call succeeds and the retry still finds no principal.
      fake.setHandlers({
        'serviceInfo.getServiceInfo': (_req, ctx) => {
          if (ctx.token !== null) fake.provisioned.delete(ctx.token);
          return { serviceInfo: { service: 'iam', version: 'x', capabilities: [] } };
        },
      });
      const result = await introspectWithProvisioning(clientsFor('ghost'), 'ghost');
      expect(result.ok ? null : result.error.reason).toBe(ErrorReason.IDENTITY_NOT_PROVISIONED);
      expect(fake.callsTo('authn.introspect')).toHaveLength(2);
    });
  });
});
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/integration/principal.test.ts
```

Expected: FAIL. `../../lib/principal` and `../../lib/principal-resolver` cannot be resolved.

- [ ] **Step 2: Write `lib/principal.ts`**

Create `ts/apps/iam-console/lib/principal.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Who the current user is, according to IAM, NOW (spec § 4.5). The pages use this, never the
// login snapshot in the session record: a degraded login must not stay degraded for the session,
// and a membership change must appear on the next render.
//
// PROVISIONING. Introspect is exempt from bearer enforcement and runs with Provisioning::Disabled,
// so for an identity IAM has never seen it answers PermissionDenied `identity-not-provisioned`
// (rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:139-141;
// application/authenticate_token.rs:104-105). IAM provisions only inside a bearer-enforced RPC
// (authn.rs:182-190). GetServiceInfo is bearer-enforced and checks no Cedar action
// (adapters/grpc/service_info.rs:1-44), so it is the provisioning call.
import 'server-only';
import { cache } from 'react';
import { ErrorReason } from '@paigasus/sdk/errors';
import { callIam, type IamResult } from './errors';
import { iamClients, sessionToken, type IamClients } from './iam';

export type Principal = { prn: string; memberships: readonly { nodePrn: string }[] };

export async function introspectWithProvisioning(
  clients: Pick<IamClients, 'authn' | 'serviceInfo'>,
  token: string,
  opts: { timeoutMs?: number; provisionFirst?: boolean } = {},
): Promise<IamResult<Principal>> {
  const callOptions = opts.timeoutMs === undefined ? {} : { timeoutMs: opts.timeoutMs };
  const provision = () => callIam(() => clients.serviceInfo.getServiceInfo({}, callOptions));
  const introspect = () => callIam(() => clients.authn.introspect({ token }, callOptions));

  if (opts.provisionFirst === true) {
    const provisioned = await provision();
    if (!provisioned.ok) return provisioned;
  }
  let answer = await introspect();
  if (!answer.ok && opts.provisionFirst !== true && answer.error.reason === ErrorReason.IDENTITY_NOT_PROVISIONED) {
    // ONE retry, after one provisioning call. A second `identity-not-provisioned` is returned as
    // the error it is; looping would hide a provisioning failure behind a hang.
    const provisioned = await provision();
    if (!provisioned.ok) return provisioned;
    answer = await introspect();
  }
  if (!answer.ok) return answer;
  return { ok: true, value: { prn: answer.value.principalPrn, memberships: answer.value.memberships.map((m) => ({ nodePrn: m.nodePrn })) } };
}

/** Per request: a LIVE Introspect, with one provisioning retry on `identity-not-provisioned`. */
export const currentPrincipal: () => Promise<IamResult<Principal>> = cache(async () => introspectWithProvisioning(await iamClients(), await sessionToken(), { provisionFirst: false }));
```

- [ ] **Step 3: Write `lib/principal-resolver.ts`**

Create `ts/apps/iam-console/lib/principal-resolver.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// @paigasus/auth's PrincipalResolver port, implemented over IAM (spec § 4.5). The login callback
// calls it once (ts/packages/paigasus-auth/src/http/routes.ts:236), with the new access token.
//
// It lives in the APP because @paigasus/auth must not import @paigasus/sdk
// (ts/packages/paigasus-auth/src/ports/principal-resolver.ts:3-8), and the sdk boundary rule bans
// every @paigasus/* import except proto. SMA-631 records a shared home for SMA-512.
//
// IT NEVER FAILS THE LOGIN. On any failure it returns principalPrn: null, empty lists and
// grantsAvailable: false, and logs `principal.resolve_failed`. The pages do not read this snapshot
// (they call currentPrincipal()), so a degraded login costs nothing after the first render.
import 'server-only';
import type { PrincipalResolver, ResolvedPrincipal } from '@paigasus/auth/server';
import { callIam } from './errors';
import type { IamClients } from './iam-clients';
import type { ConsoleLogger } from './logger';

/** The user waits on the login callback, so each call gets 3 s, not the SDK's 10 s (transport.ts:42). */
const DEFAULT_TIMEOUT_MS = 3_000;

export function createIntrospectPrincipalResolver(deps: {
  clientsForToken: (token: string) => Pick<IamClients, 'authn' | 'serviceInfo'>;
  logger: ConsoleLogger;
  timeoutMs?: number;
}): PrincipalResolver {
  const timeoutMs = deps.timeoutMs ?? DEFAULT_TIMEOUT_MS;

  return {
    async resolve({ accessToken, idTokenClaims }): Promise<ResolvedPrincipal> {
      const degraded = (presentation: string): ResolvedPrincipal => {
        deps.logger.appEvent('principal.resolve_failed', { presentation });
        return { principalPrn: null, issuer: idTokenClaims.iss, subject: idTokenClaims.sub, memberships: [], roleGrants: [], grantsAvailable: false };
      };
      // The whole sequence, the mapping included, runs inside one try (spec § 4.5).
      try {
        const clients = deps.clientsForToken(accessToken);
        // 1. The provisioning call, with the NEW token as the bearer.
        const provisioned = await callIam(() => clients.serviceInfo.getServiceInfo({}, { timeoutMs }));
        if (!provisioned.ok) return degraded(provisioned.error.presentation);
        // 2. Introspect reads the token from the REQUEST field (authn.rs:59), not from the header.
        const answer = await callIam(() => clients.authn.introspect({ token: accessToken }, { timeoutMs }));
        if (!answer.ok) return degraded(answer.error.presentation);
        const me = answer.value;
        // 3. IAM reports no role grants here (authenticate_token.rs:161-165), and the port says
        //    to treat that as UNKNOWN (ports/principal-resolver.ts:32-36).
        return {
          principalPrn: me.principalPrn === '' ? null : me.principalPrn,
          issuer: me.issuer,
          subject: me.subject,
          memberships: me.memberships.map((m) => ({ id: m.id, principalPrn: m.principalPrn, nodePrn: m.nodePrn })),
          roleGrants: [],
          grantsAvailable: false,
        };
      } catch {
        // callIam rethrows everything that is not a ConnectError. Here nothing may escape.
        return degraded('generic');
      }
    },
  };
}
```

- [ ] **Step 4: Run the tests and see them pass**

Run the command of Step 1 again.

Expected: `Tests  7 passed (7)`. The call log proves the order `serviceInfo.getServiceInfo` → `authn.introspect` for a login, and `authn.introspect` → `serviceInfo.getServiceInfo` → `authn.introspect` for a first render of a user IAM has never seen. The timeout case ends in well under 2 s.

- [ ] **Step 5: Give the login callback the resolver**

Replace `ts/apps/iam-console/lib/auth.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The auth composition root (spec § 4.2). getAuthRuntime is a PROCESS singleton that returns a
// Promise (ts/packages/paigasus-auth/src/runtime.ts:185), and its first call fixes the resolver and
// the logger for the life of the process. Nothing here runs at module scope.
import 'server-only';
import { getAuthRuntime, type AuthRuntime } from '@paigasus/auth/server';
import { getRuntimeConfig } from './config';
import { iamClientsForToken } from './iam-clients';
import { logger } from './logger';
import { createIntrospectPrincipalResolver } from './principal-resolver';

/**
 * The session cookie's name. @paigasus/auth does not export it from any entry an app may import,
 * so it is written here once; tests/unit/session-cookie.test.ts proves that @paigasus/auth's own
 * middleware treats exactly this name as the session.
 */
export const SESSION_COOKIE_NAME = '__Host-pgs_sid';

export function authRuntime(): Promise<AuthRuntime> {
  return getAuthRuntime(getRuntimeConfig(), {
    logger,
    resolver: createIntrospectPrincipalResolver({ clientsForToken: (token) => iamClientsForToken(token), logger }),
  });
}
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/unit tests/integration
```

Expected: every test file passes. `tests/unit/proxy.test.ts` and `tests/unit/session-cookie.test.ts` import `lib/auth.ts` and still pass: the resolver is built only when `authRuntime()` runs, never at module scope.

- [ ] **Step 6: Run the app's gates**

Run Prettier on every file this task creates or edits, then the gates:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write apps/iam-console/lib/principal.ts apps/iam-console/lib/principal-resolver.ts \
  apps/iam-console/lib/auth.ts apps/iam-console/tests/integration/principal.test.ts
moon run iam-console-ts:test iam-console-ts:typecheck
moon run ts:lint ts:fmt --force
```

Expected: exit 0.

- [ ] **Step 7: Commit**

```bash
git add ts/apps/iam-console/lib/principal.ts ts/apps/iam-console/lib/principal-resolver.ts \
  ts/apps/iam-console/lib/auth.ts ts/apps/iam-console/tests/integration/principal.test.ts
git commit -F - <<'EOF'
feat(ts): iam-console principal provisioning and login resolver (SMA-511)

IAM's Introspect never provisions a principal, and only a
bearer-enforced call does. The login resolver therefore calls
GetServiceInfo with the new token first, then Introspect, each with a
3 s timeout. On any failure it returns a null principal and logs
principal.resolve_failed instead of failing the login.

currentPrincipal() is a live Introspect per request. On
identity-not-provisioned it provisions once and retries once, so a
degraded login does not stay degraded for the session.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 14: `mayI()` and the app's discovery

This task writes the affordance check `mayI()` over IAM's `IsAuthorized` (spec § 4.6, § 6.3, decision D5) and the app's composition of `@paigasus/discovery` with the descriptor cache of spec § 4.4.

**Files:**
- Create: `ts/apps/iam-console/lib/authorize.ts`
- Create: `ts/apps/iam-console/lib/discovery.ts`
- Test: `ts/apps/iam-console/tests/unit/authorize.test.ts`
- Test: `ts/apps/iam-console/tests/integration/authorize.test.ts`
- Test: `ts/apps/iam-console/tests/integration/discovery.test.ts`

**Interfaces:**
- Consumes: `type AuthorizationService` (`@paigasus/sdk/iam`); `type Client` (`@connectrpc/connect`); `createDiscovery`, `createMemoryDescriptorCache`, `createRedisDescriptorCache`, `timingsFromEnv`, `type DescriptorCache`, `type Discovery` (`@paigasus/discovery/server`); `createClient`, `type RedisClientType` (`redis` 6.2.1); `after` (`next/server`); `callIam` (Task 12); `iamClients` (Task 12); `currentPrincipal` (Task 13); `getRuntimeConfig`, `type ConsoleConfig`, `logger`, `type ConsoleLogger` (Task 9); `serviceInfoHandlers` (Task 11).
- Produces: `lib/authorize.ts` → `type IamAction`, `type MayI`, `createMayI({ authz, principalPrn, logger })`, `mayI()`. `lib/discovery.ts` → `discovery()`, `createAppDiscovery({ config, log?, fetch?, waitUntil? })`, `descriptorCacheFor(config, log?)`, `resetDiscoveryForTest()`.

> Handoff to Tasks 16–19: `const may = await mayI();` then `await may('CreateTeam', organizationPrn(orgId))`. A `false` may HIDE an affordance only; no loader and no command refuses anything because of it (spec § 6.3). The action names are PascalCase (`Action::parse`, `rs/crates/libs/paigasus-iam-core/src/authz/action.rs:114-163`). `IamAction` lists the seven this app asks about; add a name there when a screen needs another. For the audit page: `discovery().getServiceState('iam', await sessionToken())`.

- [ ] **Step 1: Write the failing `mayI` tests**

Create `ts/apps/iam-console/tests/unit/authorize.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// createMayI (spec § 4.6, § 6.3): it asks IsAuthorized about the current principal, memoizes per
// (action, resource), and FAILS OPEN.
import { describe, expect, it, vi } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { createMayI } from '../../lib/authorize';

const ME = 'prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000001';
const ORG = 'prn:pgs:iam::0192f1c0-0000-7000-8000-000000000002:organization/0192f1c0-0000-7000-8000-000000000002';

function fakeAuthz(answer: (req: { principalPrn: string; action: string; resourcePrn: string }) => boolean | Error) {
  const calls: { principalPrn: string; action: string; resourcePrn: string }[] = [];
  const isAuthorized = vi.fn((req: { principalPrn: string; action: string; resourcePrn: string }) => {
    calls.push(req);
    const result = answer(req);
    return result instanceof Error ? Promise.reject(result) : Promise.resolve({ allowed: result, determiningPolicies: [], reason: '' });
  });
  return { authz: { isAuthorized } as never, calls };
}

describe('createMayI', () => {
  it('asks IsAuthorized about the current principal with the PascalCase action', async () => {
    const { authz, calls } = fakeAuthz(() => false);
    const mayI = createMayI({ authz, principalPrn: ME, logger: { appEvent: vi.fn() } });
    expect(await mayI('CreateTeam', ORG)).toBe(false);
    expect(calls).toEqual([{ principalPrn: ME, action: 'CreateTeam', resourcePrn: ORG }]);
  });

  it('asks once per (action, resource) for the instance’s lifetime', async () => {
    const { authz, calls } = fakeAuthz(() => true);
    const mayI = createMayI({ authz, principalPrn: ME, logger: { appEvent: vi.fn() } });
    await Promise.all([mayI('CreateTeam', ORG), mayI('CreateTeam', ORG), mayI('AttachMembership', ORG)]);
    await mayI('CreateTeam', ORG);
    expect(calls.map((c) => c.action)).toEqual(['CreateTeam', 'AttachMembership']);
  });

  it('fails OPEN on a failed query, and logs authorize.query_failed', async () => {
    const { authz } = fakeAuthz(() => new ConnectError('down', Code.Unavailable));
    const appEvent = vi.fn();
    const mayI = createMayI({ authz, principalPrn: ME, logger: { appEvent } });
    expect(await mayI('ListAuditLog', ORG)).toBe(true);
    expect(appEvent).toHaveBeenCalledWith('authorize.query_failed', { action: 'ListAuditLog', presentation: 'degraded' });
  });

  it('answers true without a call when IAM could not say who the principal is', async () => {
    const { authz, calls } = fakeAuthz(() => false);
    const mayI = createMayI({ authz, principalPrn: null, logger: { appEvent: vi.fn() } });
    expect(await mayI('CreateOrganization', ORG)).toBe(true);
    expect(calls).toEqual([]);
  });
});
```

Create `ts/apps/iam-console/tests/integration/authorize.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// mayI against the fake IAM over the wire: the question IAM receives is exactly
// (me, PascalCase action, resource), and a denial of the QUESTION itself fails open.
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { disposeTransports } from '@paigasus/sdk/iam';
import { createMayI } from '../../lib/authorize';
import { createIamClients } from '../../lib/iam-clients';
import { denial, startFakeIam, type FakeIam } from '../support/fake-iam';

const ORG = 'prn:pgs:iam::0192f1c0-0000-7000-8000-000000000002:organization/0192f1c0-0000-7000-8000-000000000002';

describe('mayI over the wire', () => {
  let fake: FakeIam;
  beforeEach(async () => {
    fake = await startFakeIam();
  });
  afterEach(() => fake.close());
  afterAll(() => disposeTransports());

  it('sends IAM the principal, the action and the resource, and returns its answer', async () => {
    fake.setHandlers({ 'authz.isAuthorized': (req) => ({ allowed: req.action === 'CreateTeam' }) });
    const me = fake.principalPrnFor('token-a');
    const mayI = createMayI({ authz: createIamClients({ baseUrl: fake.grpcUrl, token: 'token-a' }).authz, principalPrn: me, logger: { appEvent: vi.fn() } });
    expect(await mayI('CreateTeam', ORG)).toBe(true);
    expect(await mayI('DetachMembership', ORG)).toBe(false);
    expect(fake.callsTo('authz.isAuthorized').map((call) => call.request)).toEqual([
      expect.objectContaining({ principalPrn: me, action: 'CreateTeam', resourcePrn: ORG }),
      expect.objectContaining({ principalPrn: me, action: 'DetachMembership', resourcePrn: ORG }),
    ]);
  });

  it('fails open when IAM refuses the question itself', async () => {
    fake.setHandlers({
      'authz.isAuthorized': () => {
        throw denial();
      },
    });
    const appEvent = vi.fn();
    const mayI = createMayI({ authz: createIamClients({ baseUrl: fake.grpcUrl, token: 'token-a' }).authz, principalPrn: fake.principalPrnFor('token-a'), logger: { appEvent } });
    expect(await mayI('CreateOrganization', ORG)).toBe(true);
    expect(appEvent).toHaveBeenCalledWith('authorize.query_failed', { action: 'CreateOrganization', presentation: 'forbidden' });
  });
});
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/unit/authorize.test.ts tests/integration/authorize.test.ts
```

Expected: FAIL. `../../lib/authorize` cannot be resolved.

- [ ] **Step 2: Write `lib/authorize.ts`**

Create `ts/apps/iam-console/lib/authorize.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// mayI() — the affordance check (spec § 4.6, § 6.3, decision D5). It asks IAM's IsAuthorized about
// the CURRENT principal. A principal may always ask about itself
// (rs/crates/services/paigasus-iam/src/application/authorize.rs:75-80), and IsAuthorized has no
// capability gate (adapters/grpc/authz.rs:81-91). Cedar evaluates the request, so the answer
// follows the resource hierarchy and every custom policy. The app holds NO table of which role
// grants which action.
//
// COSMETIC ONLY. mayI() may hide an affordance. No page and no Server Action refuses anything
// because mayI() said no: every user action calls IAM, and IAM decides.
//
// IT FAILS OPEN. A failed query answers true and logs `authorize.query_failed`: a button that
// should be hidden then shows, and IAM still denies the action. A null principal (IAM could not
// say who this is) also answers true, for the same reason.
import 'server-only';
import { cache } from 'react';
import type { Client } from '@connectrpc/connect';
import type { AuthorizationService } from '@paigasus/sdk/iam';
import { callIam } from './errors';
import { iamClients } from './iam';
import { logger, type ConsoleLogger } from './logger';
import { currentPrincipal } from './principal';

/** The PascalCase names IAM's Action::parse accepts (rs/crates/libs/paigasus-iam-core/src/authz/action.rs:114-163). */
export type IamAction = 'ListOrganizations' | 'CreateOrganization' | 'CreateTeam' | 'CreateProject' | 'AttachMembership' | 'DetachMembership' | 'ListAuditLog';

export type MayI = (action: IamAction, resourcePrn: string) => Promise<boolean>;

export function createMayI(deps: { authz: Pick<Client<typeof AuthorizationService>, 'isAuthorized'>; principalPrn: string | null; logger: Pick<ConsoleLogger, 'appEvent'> }): MayI {
  const memo = new Map<string, Promise<boolean>>();

  const ask = async (principalPrn: string, action: IamAction, resourcePrn: string): Promise<boolean> => {
    const answer = await callIam(() => deps.authz.isAuthorized({ principalPrn, action, resourcePrn }));
    if (answer.ok) return answer.value.allowed;
    deps.logger.appEvent('authorize.query_failed', { action, presentation: answer.error.presentation });
    return true;
  };

  return (action, resourcePrn) => {
    const principalPrn = deps.principalPrn;
    if (principalPrn === null) return Promise.resolve(true);
    // The NUL separator cannot occur in an action name or a PRN, so two pairs never share a key.
    const key = `${action}\u0000${resourcePrn}`;
    let pending = memo.get(key);
    if (pending === undefined) {
      pending = ask(principalPrn, action, resourcePrn);
      memo.set(key, pending);
    }
    return pending;
  };
}

/** One MayI per request, so the memo lives exactly one request. */
export const mayI: () => Promise<MayI> = cache(async () => {
  const [principal, clients] = await Promise.all([currentPrincipal(), iamClients()]);
  return createMayI({ authz: clients.authz, principalPrn: principal.ok ? principal.value.prn : null, logger });
});
```

Run the command of Step 1 again. Expected: `Test Files  2 passed (2)`, `Tests  6 passed (6)`.

- [ ] **Step 3: Write the failing discovery tests**

Create `ts/apps/iam-console/tests/integration/discovery.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// lib/discovery.ts (spec § 4.4, § 6.6): the app's composition of @paigasus/discovery, with MSW
// serving IAM's `GET /v1/service-info` (AC 5). No live service and no Docker.
//
// The process-wide descriptor cache is module state, so each case imports fresh modules and resets
// the cache afterwards.
import { setupServer } from 'msw/node';
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { stubConsoleEnv } from '../support/env';
import { serviceInfoHandlers } from '../support/msw';

const IAM_HTTP = 'http://iam.msw.test';
const server = setupServer();

beforeAll(() => server.listen({ onUnhandledRequest: 'error' }));
afterEach(() => {
  server.resetHandlers();
  vi.unstubAllEnvs();
});
afterAll(() => server.close());

async function load(overrides: Record<string, string>) {
  vi.resetModules();
  stubConsoleEnv({ PAIGASUS_SERVICES: JSON.stringify({ iam: IAM_HTTP }), ...overrides });
  const { getRuntimeConfig } = await import('../../lib/config');
  const { createJsonLogger } = await import('../../lib/logger');
  const discovery = await import('../../lib/discovery');
  const lines: string[] = [];
  const handle = discovery.createAppDiscovery({ config: getRuntimeConfig(), log: createJsonLogger((line) => lines.push(line)), waitUntil: () => undefined });
  return { handle, lines, reset: discovery.resetDiscoveryForTest };
}

describe('the app’s discovery', () => {
  it('reports IAM available with the capabilities it serves (memory store)', async () => {
    server.use(...serviceInfoHandlers(IAM_HTTP, { service: 'iam', version: '1.0.0', capabilities: ['iam.authz.cedar', 'iam.audit'] }));
    const { handle, reset } = await load({});
    try {
      expect(await handle.getServiceState('iam', 'token-a')).toMatchObject({ state: 'available', capabilities: ['iam.authz.cedar', 'iam.audit'] });
      expect(await handle.hasCapability('iam.audit', 'token-a')).toBe(true);
    } finally {
      reset();
    }
  });

  it('reports a service that is not configured as absent, without a request', async () => {
    const { handle, reset } = await load({});
    try {
      expect(await handle.getServiceState('gateway', 'token-a')).toEqual({ state: 'absent', service: 'gateway' });
    } finally {
      reset();
    }
  });

  it('reports IAM degraded when its descriptor route fails', async () => {
    server.use(...serviceInfoHandlers(IAM_HTTP, { status: 503 }));
    const { handle, reset } = await load({});
    try {
      expect(await handle.getServiceState('iam', 'token-a')).toMatchObject({ state: 'degraded', reason: 'server-error' });
    } finally {
      reset();
    }
  });

  it('with an unreachable Redis: degrades to cache-unavailable, and logs the failure once without the DSN', async () => {
    server.use(...serviceInfoHandlers(IAM_HTTP, { service: 'iam', version: '1.0.0', capabilities: [] }));
    const { handle, lines, reset } = await load({ PAIGASUS_SESSION_STORE: 'redis', PAIGASUS_SESSION_REDIS_URL: 'redis://:hunter2@127.0.0.1:1', PAIGASUS_SESSION_REDIS_TIMEOUT_MS: '200' });
    try {
      expect(await handle.getServiceState('iam', 'token-a')).toMatchObject({ state: 'degraded', reason: 'cache-unavailable' });
      const events = lines.map((line) => (JSON.parse(line) as { event: string }).event);
      expect(events.filter((event) => event === 'discovery.redis_connect_failed')).toHaveLength(1);
      expect(lines.join('\n')).not.toContain('hunter2');
    } finally {
      reset();
    }
  });
});
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/integration/discovery.test.ts
```

Expected: FAIL. `../../lib/discovery` cannot be resolved.

- [ ] **Step 4: Write `lib/discovery.ts`**

Create `ts/apps/iam-console/lib/discovery.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Capability discovery for this app (spec § 4.4). ONE Discovery handle PER REQUEST (React cache()),
// as @paigasus/discovery requires (ts/packages/paigasus-discovery/src/server.ts:63-82), over a
// descriptor cache that is a PROCESS singleton.
//
//   PAIGASUS_SESSION_STORE=redis  -> the SAME Redis URL as the session store, so an operator
//                                    configures one Redis. A lazy node-redis client, made on first
//                                    use with the four preconditions createRedisDescriptorCache
//                                    asserts (src/adapters/redis-cache.ts:67-90).
//   PAIGASUS_SESSION_STORE=memory -> the memory cache.
//
// There is NO silent fallback from Redis to memory. When Redis cannot connect, the cache fails
// fast, discovery reports its own `cache-unavailable` degraded reason, and this file logs
// `discovery.redis_connect_failed` once, with no DSN.
import 'server-only';
import { after } from 'next/server';
import { cache } from 'react';
import { createClient, type RedisClientType } from 'redis';
import { createDiscovery, createMemoryDescriptorCache, createRedisDescriptorCache, timingsFromEnv, type DescriptorCache, type Discovery } from '@paigasus/discovery/server';
import { getRuntimeConfig, type ConsoleConfig } from './config';
import { logger, type ConsoleLogger } from './logger';

let processCache: DescriptorCache | undefined;
let redisClient: RedisClientType | undefined;

/**
 * Waits for the first connect, but never longer than one command timeout. node-redis's connect()
 * keeps retrying while its reconnect strategy returns a delay (@redis/client socket.js #connect),
 * so an unreachable Redis would otherwise hold every render. The connect keeps running in the
 * background, so the cache recovers when Redis does.
 */
function connectOnce(client: RedisClientType, timeoutMs: number, log: ConsoleLogger): Promise<void> {
  let settled = false;
  const connected = client.connect().then(
    () => {
      settled = true;
    },
    () => {
      settled = true;
      log.appEvent('discovery.redis_connect_failed', { stage: 'connect' });
    },
  );
  const bounded = new Promise<void>((resolve) => {
    setTimeout(() => {
      if (!settled) log.appEvent('discovery.redis_connect_failed', { stage: 'connect_timeout' });
      resolve();
    }, timeoutMs).unref();
  });
  return Promise.race([connected, bounded]);
}

/** Wraps a cache so that every operation first waits (boundedly) for the first connect. */
function afterConnect(inner: DescriptorCache, ready: Promise<void>): DescriptorCache {
  return {
    get: async (service) => {
      await ready;
      return inner.get(service);
    },
    set: async (service, rec, ttlMs, expectedRev) => {
      await ready;
      return inner.set(service, rec, ttlMs, expectedRev);
    },
    delete: async (service) => {
      await ready;
      return inner.delete(service);
    },
    tryAcquireLock: async (service, token, ttlMs) => {
      await ready;
      return inner.tryAcquireLock(service, token, ttlMs);
    },
    releaseLock: async (service, token) => {
      await ready;
      return inner.releaseLock(service, token);
    },
    close: () => inner.close(),
  };
}

function redisDescriptorCache(url: string, timeoutMs: number, log: ConsoleLogger): DescriptorCache {
  const client: RedisClientType = createClient({ url, disableOfflineQueue: true, commandOptions: { timeout: timeoutMs }, socket: { socketTimeout: timeoutMs * 2, connectTimeout: timeoutMs } });
  // REQUIRED by createRedisDescriptorCache, and it must NEVER log the error: node-redis embeds the
  // DSN in its connection errors. Reconnects fire it repeatedly, so it logs nothing at all.
  client.on('error', () => undefined);
  redisClient = client;
  const inner = createRedisDescriptorCache(client);
  return afterConnect(inner, connectOnce(client, timeoutMs, log));
}

/** The process-wide descriptor cache for this configuration. */
export function descriptorCacheFor(config: ConsoleConfig, log: ConsoleLogger = logger): DescriptorCache {
  if (processCache !== undefined) return processCache;
  if (config.PAIGASUS_SESSION_STORE === 'redis' && config.PAIGASUS_SESSION_REDIS_URL !== undefined) {
    processCache = redisDescriptorCache(config.PAIGASUS_SESSION_REDIS_URL, config.PAIGASUS_SESSION_REDIS_TIMEOUT_MS, log);
  } else {
    processCache = createMemoryDescriptorCache();
  }
  return processCache;
}

/** Test and shutdown seam: forget the process cache and close the Redis client, if any. */
export function resetDiscoveryForTest(): void {
  redisClient?.destroy();
  redisClient = undefined;
  processCache = undefined;
}

/** A Discovery handle for one request. Pure apart from the process cache: tests call it directly. */
export function createAppDiscovery(deps: { config: ConsoleConfig; log?: ConsoleLogger; fetch?: typeof globalThis.fetch; waitUntil?: (p: Promise<unknown>) => void }): Discovery {
  const log = deps.log ?? logger;
  return createDiscovery({
    services: deps.config.PAIGASUS_SERVICES,
    cache: descriptorCacheFor(deps.config, log),
    logger: log,
    timings: timingsFromEnv(deps.config),
    ...(deps.fetch === undefined ? {} : { fetch: deps.fetch }),
    ...(deps.waitUntil === undefined ? {} : { waitUntil: deps.waitUntil }),
  });
}

/** One handle per request; background revalidation runs after the response, through after(). */
export const discovery: () => Discovery = cache(() => createAppDiscovery({ config: getRuntimeConfig(), waitUntil: (p) => after(p) }));
```

Run the command of Step 3 again. Expected: `Tests  4 passed (4)`. The Redis case needs no Redis: it points at `127.0.0.1:1`, which refuses the connection. It proves the three things spec § 4.4 requires of that path: discovery reports `cache-unavailable` (no silent memory fallback), the app logs `discovery.redis_connect_failed` once, and the password in the DSN appears in no log line.

- [ ] **Step 5: Run the app's gates**

Run Prettier on every file this task creates or edits, then the gates:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write apps/iam-console/lib/authorize.ts apps/iam-console/lib/discovery.ts \
  apps/iam-console/tests/unit/authorize.test.ts apps/iam-console/tests/integration/authorize.test.ts \
  apps/iam-console/tests/integration/discovery.test.ts
moon run iam-console-ts:test iam-console-ts:typecheck
moon run ts:lint ts:fmt --force
```

Expected: exit 0.

- [ ] **Step 6: Commit**

```bash
git add ts/apps/iam-console/lib/authorize.ts ts/apps/iam-console/lib/discovery.ts \
  ts/apps/iam-console/tests/unit/authorize.test.ts ts/apps/iam-console/tests/integration/authorize.test.ts \
  ts/apps/iam-console/tests/integration/discovery.test.ts
git commit -F - <<'EOF'
feat(ts): iam-console mayI affordance check and capability discovery (SMA-511)

mayI asks IAM's IsAuthorized about the current principal, memoizes per
action and resource for one request, and fails open with a logged
authorize.query_failed. It only hides affordances; IAM still decides
every user action.

discovery() builds one handle per request over a process-wide
descriptor cache: the session store's Redis, or memory. The lazy
node-redis client carries all four preconditions, and an unreachable
Redis degrades to cache-unavailable with one log line and no DSN.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 15: The navigation, the console shell, the organization switcher and the app-shell Tailwind sentinel

This task builds the shell of every signed-in page (spec § 5.4): the navigation entries through `navStateOf()` and the `(console)` layout. It also writes the organization switcher, which reads the `[org]` segment, with its unit test — but the layout does not use it yet: its data comes from `myScopes()`, which Task 16 creates, and Task 16 wires it in. Last, the task makes Tailwind scan `@paigasus/app-shell` and proves that with a third sentinel in all three modes of the guard (spec § 7.6).

**Files:**
- Create: `ts/apps/iam-console/lib/nav.ts`
- Create: `ts/apps/iam-console/app/_components/org-switcher.tsx`
- Create: `ts/apps/iam-console/app/(console)/layout.tsx`
- Modify: `ts/apps/iam-console/app/globals.css`
- Modify: `ts/packages/paigasus-app-shell/src/shell/app-shell.tsx`
- Modify: `ci/tailwind-source/run.mjs`
- Modify: `ci/tailwind-source/README.md`
- Test: `ts/apps/iam-console/tests/unit/nav.test.ts`
- Test: `ts/apps/iam-console/tests/unit/org-switcher.test.tsx`

**Interfaces:**
- Consumes: `navStateOf`, `AppShell`, `useZone`, `type Brand`, `type NavEntry`, `type SwitcherItem`, `type ZoneMap` (`@paigasus/app-shell`); `type ServiceState` (`@paigasus/discovery/types`); `useParams` (`next/navigation`); `connection` (`next/server`); `toSessionView` (`@paigasus/auth/server`); `currentSession` (Task 12); `discovery` (Task 14); `mayI` (Task 14); `ROOT_PRN` (Task 8); `getPublicConfig` (Task 9); `Providers` (Task 10).
- Produces: `lib/nav.ts` → `buildNavEntries({ iam, gateway, zones, auditAllowed }): NavEntry[]`. The sentinel `--paigasus-app-shell-source-probe` in `src/shell/app-shell.tsx`. `app/(console)/layout.tsx` starts with `await connection();` (as `app/(public)/layout.tsx` does) and renders `<AppShell brand={{ label: 'Paigasus IAM', href: '/iam/orgs' }} nav={nav}>` with NO switcher.
- Produces: `app/_components/org-switcher.tsx` ('use client'), exactly:
  - `export type OrgSwitcherOrg = { readonly orgId: string; readonly label: string };`
  - `export function OrgSwitcherShell(props: { brand: Brand; nav: readonly NavEntry[]; orgs: readonly OrgSwitcherOrg[]; children: ReactNode }): ReactElement` — `AppShell` plus one switcher labelled "Organization". The props are `AppShellProps` with `orgs` in place of `switchers`. It must render `AppShell` itself, because `AppShell` renders a switcher only from its `switchers` prop and only a client component can read `[org]` (a layout does not receive child segments' params). It builds each item as `{ id: orgId, label, href: '<basePath>/orgs/<orgId>' }` (full path, as `@paigasus/app-shell`'s `Switcher` requires) and passes `currentId` from `useParams().org` in lower case (the item ids are lower case). An empty `orgs` renders no switcher.
  - For its unit test only: `orgSwitcherItems(orgs, basePath): SwitcherItem[]` and `currentOrgId(params): string | null` (the `[org]` segment in lower case). They live in a `'use client'` module, so a server file must not call them.

> Handoff to Task 16: in `app/(console)/layout.tsx`, replace `<AppShell brand={…} nav={nav}>…</AppShell>` with `<OrgSwitcherShell brand={…} nav={nav} orgs={orgs}>…</OrgSwitcherShell>` (same `brand`, same `nav`), import it from `'../_components/org-switcher'`, and delete the two-line comment above it. Build `orgs: OrgSwitcherOrg[]` on the server from `myScopes()`: one `{ orgId, label }` per organization scope, `label` = the organization name, or the `orgId` for a denied row; on an `IamResult` error pass `[]`. Pass plain data only: `OrgSwitcherShell` is a client component and builds the hrefs itself.

> Handoff to Task 23: the new affected-graph case `app-shell->console` anchors on `ts/packages/paigasus-app-shell/src/shell/app-shell.tsx`, the file that holds sentinel C. The app-local case anchors on `ts/apps/iam-console/lib/iam.ts` and `ts/apps/iam-console/proxy.ts`; both exist after Task 12. Task 8 added `apps/*/lib/**/*` and Task 10 added `apps/*/proxy.ts` to `ts/moon.yml`'s `sources` group, so an edit there now selects `ts:lint`; re-baseline any expected set that lists that task.

- [ ] **Step 1: Write the failing navigation test**

Create `ts/apps/iam-console/tests/unit/nav.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// buildNavEntries (spec § 5.4, § 6.6, § 9.2): every IAM state, with and without `iam.audit`, with
// the audit question allowed and denied; the gateway absent, degraded and available; and every
// entry's state went through navStateOf().
import { describe, expect, it, vi } from 'vitest';
import type { ServiceState } from '@paigasus/discovery/types';

const navStateOf = vi.hoisted(() => vi.fn());
vi.mock('@paigasus/app-shell', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@paigasus/app-shell')>();
  navStateOf.mockImplementation(actual.navStateOf);
  return { ...actual, navStateOf };
});

const { buildNavEntries } = await import('../../lib/nav');

const ZONES = { iam: '/iam', gateway: '/gateway' };
const iamUp = (capabilities: string[]): ServiceState => ({ state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1', capabilities }, capabilities });
const IAM_DOWN: ServiceState = { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: { service: 'iam', version: '1', capabilities: ['iam.audit'] }, capabilities: ['iam.audit'] };
const IAM_ABSENT: ServiceState = { state: 'absent', service: 'iam' };
const GATEWAY_ABSENT: ServiceState = { state: 'absent', service: 'gateway' };
const GATEWAY_DOWN: ServiceState = { state: 'degraded', service: 'gateway', reason: 'network', descriptor: null, capabilities: [] };
const GATEWAY_UP: ServiceState = { state: 'available', service: 'gateway', descriptor: { service: 'gateway', version: '1', capabilities: [] }, capabilities: [] };

const byLabel = (entries: ReturnType<typeof buildNavEntries>) => Object.fromEntries(entries.map((entry) => [entry.label, entry]));

describe('buildNavEntries', () => {
  it('lists Organizations, Audit and Gateway with full-path hrefs when everything is available', () => {
    const entries = buildNavEntries({ iam: iamUp(['iam.audit']), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true });
    expect(entries.map((e) => [e.label, e.zone, e.href, e.state.state])).toEqual([
      ['Organizations', 'iam', '/iam/orgs', 'available'],
      ['Audit', 'iam', '/iam/audit', 'available'],
      ['Gateway', 'gateway', '/gateway/', 'available'],
    ]);
  });

  it('makes Audit absent when IAM does not report iam.audit', () => {
    const entries = byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true }));
    expect(entries['Audit']?.state).toEqual({ state: 'absent' });
    expect(entries['Organizations']?.state).toEqual({ state: 'available' });
  });

  it('omits Audit when mayI says no', () => {
    const entries = byLabel(buildNavEntries({ iam: iamUp(['iam.audit']), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: false }));
    expect(entries['Audit']).toBeUndefined();
  });

  it('disables the IAM entries with a reason when IAM is degraded', () => {
    const entries = byLabel(buildNavEntries({ iam: IAM_DOWN, gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true }));
    expect(entries['Organizations']?.state).toEqual({ state: 'degraded', service: 'iam', reason: 'timeout' });
    expect(entries['Audit']?.state).toEqual({ state: 'degraded', service: 'iam', reason: 'timeout' });
  });

  it('makes the IAM entries absent when IAM is absent', () => {
    const entries = byLabel(buildNavEntries({ iam: IAM_ABSENT, gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true }));
    expect(entries['Organizations']?.state).toEqual({ state: 'absent' });
    expect(entries['Audit']?.state).toEqual({ state: 'absent' });
  });

  it('gives the Gateway entry the gateway’s own state: absent, or degraded with a reason', () => {
    expect(byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_ABSENT, zones: ZONES, auditAllowed: false }))['Gateway']?.state).toEqual({ state: 'absent' });
    expect(byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_DOWN, zones: ZONES, auditAllowed: false }))['Gateway']?.state).toEqual({
      state: 'degraded',
      service: 'gateway',
      reason: 'network',
    });
  });

  it('leaves out the Gateway entry when gateway is not a zone', () => {
    const entries = byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_UP, zones: { iam: '/iam' }, auditAllowed: false }));
    expect(entries['Gateway']).toBeUndefined();
  });

  it('takes EVERY entry’s state from navStateOf()', () => {
    navStateOf.mockClear();
    const entries = buildNavEntries({ iam: iamUp(['iam.audit']), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true });
    expect(navStateOf).toHaveBeenCalledTimes(entries.length);
    expect(navStateOf.mock.results.map((r) => r.value as unknown)).toEqual(entries.map((e) => e.state));
  });
});
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/unit/nav.test.ts
```

Expected: FAIL. `../../lib/nav` cannot be resolved.

- [ ] **Step 2: Write `lib/nav.ts`**

Create `ts/apps/iam-console/lib/nav.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The ONE function that builds PrimaryNav's entries (spec § 5.4). Every entry's `state` comes from
// @paigasus/app-shell's navStateOf(), which closes SMA-510 spec § 10.4's second gap: no entry here
// decides its own visibility from a capability list.
//
// Hrefs are FULL paths, as the ingress sees them (SMA-510 spec § 6.2), built from the zone map, so a
// zone mounted at another prefix gets the right link without a code change.
import 'server-only';
import { navStateOf, type NavEntry, type ZoneMap } from '@paigasus/app-shell';
import type { ServiceState } from '@paigasus/discovery/types';

export function buildNavEntries(input: { iam: ServiceState; gateway: ServiceState; zones: ZoneMap; auditAllowed: boolean }): NavEntry[] {
  const iamBase = input.zones['iam'];
  if (iamBase === undefined || !Object.hasOwn(input.zones, 'iam')) {
    // getRuntimeConfig() already refuses a zone map without this app's own zone, so this is a
    // programming error, not a deployment one.
    throw new Error('buildNavEntries: the zone map has no "iam" entry');
  }
  const entries: NavEntry[] = [{ zone: 'iam', href: `${iamBase}/orgs`, label: 'Organizations', state: navStateOf(input.iam) }];
  // ListAuditEntries is Root-only (rs/crates/services/paigasus-iam/src/application/audit.rs:36-38),
  // so the layout asks mayI('ListAuditLog', ROOT_PRN) and omits the entry on a clear "no". The
  // capability decides the rest: absent without `iam.audit`, disabled when IAM is degraded.
  if (input.auditAllowed) {
    entries.push({ zone: 'iam', href: `${iamBase}/audit`, label: 'Audit', state: navStateOf(input.iam, 'iam.audit') });
  }
  // A cross-zone entry. PrimaryNav drops it when `gateway` is not a zone (its rule 1), and
  // navStateOf answers `absent` when `gateway` is not a configured service.
  const gatewayBase = input.zones['gateway'];
  if (gatewayBase !== undefined && Object.hasOwn(input.zones, 'gateway')) {
    entries.push({ zone: 'gateway', href: `${gatewayBase}/`, label: 'Gateway', state: navStateOf(input.gateway) });
  }
  return entries;
}
```

Run the command of Step 1 again. Expected: `Tests  8 passed (8)`. The last case proves that every entry's `state` is exactly what `navStateOf()` returned: the spy counts one call per entry.

- [ ] **Step 3: Write the failing organization-switcher test**

Create `ts/apps/iam-console/tests/unit/org-switcher.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The organization switcher wrapper (spec § 5.4): it reads the `[org]` segment with useParams()
// and marks that organization as current. The selection lives in the URL only.
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { SessionProvider } from '@paigasus/auth/client';
import { ZoneProvider } from '@paigasus/app-shell';

const params: { current: Record<string, string | string[]> } = vi.hoisted(() => ({ current: {} }));
vi.mock('next/navigation', async (importOriginal) => ({
  ...(await importOriginal<typeof import('next/navigation')>()),
  useParams: () => params.current,
  usePathname: () => '/orgs',
}));
vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));

const { currentOrgId, orgSwitcherItems, OrgSwitcherShell } = await import('../../app/_components/org-switcher');

const ACME = '0192f1c0-0000-7000-8000-00000000000a';
const GLOBEX = '0192f1c0-0000-7000-8000-00000000000b';
const ORGS = [
  { orgId: ACME, label: 'Acme' },
  { orgId: GLOBEX, label: 'Globex' },
];

function renderShell(): string {
  return renderToStaticMarkup(
    <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>
      <SessionProvider value={{ principalPrn: null, displayName: 'Ada', email: null, grants: [], grantsAvailable: false }}>
        <OrgSwitcherShell brand={{ label: 'Paigasus IAM', href: '/iam/orgs' }} nav={[]} orgs={ORGS}>
          <p>page</p>
        </OrgSwitcherShell>
      </SessionProvider>
    </ZoneProvider>,
  );
}

describe('the organization switcher', () => {
  it('builds full-path hrefs under the basePath', () => {
    expect(orgSwitcherItems(ORGS, '/iam')).toEqual([
      { id: ACME, label: 'Acme', href: `/iam/orgs/${ACME}` },
      { id: GLOBEX, label: 'Globex', href: `/iam/orgs/${GLOBEX}` },
    ]);
  });

  it('reads the current organization from the [org] segment only', () => {
    expect(currentOrgId({ org: ACME })).toBe(ACME);
    expect(currentOrgId({ org: [ACME] })).toBeNull();
    expect(currentOrgId({})).toBeNull();
    expect(currentOrgId(null)).toBeNull();
  });

  it('marks the organization in the URL as current', () => {
    params.current = { org: GLOBEX };
    expect(renderShell()).toContain('Organization: Globex');
  });

  it('reads an upper-case [org] segment as the lower-case id of its item, and marks it as current', () => {
    expect(currentOrgId({ org: GLOBEX.toUpperCase() })).toBe(GLOBEX);
    params.current = { org: GLOBEX.toUpperCase() };
    expect(renderShell()).toContain('Organization: Globex');
  });

  it('marks nothing on a page outside one organization', () => {
    params.current = {};
    expect(renderShell()).toContain('Organization: none selected');
  });
});
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/apps/iam-console exec vitest run tests/unit/org-switcher.test.tsx
```

Expected: FAIL. `../../app/_components/org-switcher` cannot be resolved.

- [ ] **Step 4: Write the organization switcher**

Create `ts/apps/iam-console/app/_components/org-switcher.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The authenticated shell with its organization switcher (spec § 5.4). CLIENT component, for one
// reason: a layout does not receive the params of its child segments, so only a client component
// can read the `[org]` segment, through useParams(). The selection lives in the URL (SMA-510
// spec § 4, D1); nothing here stores it.
//
// The two helpers below are exported for the unit test only. They live in a 'use client' module,
// so a SERVER component that imported them would get client references, not functions.
'use client';

import type { ReactElement, ReactNode } from 'react';
import { useParams } from 'next/navigation';
import { AppShell, useZone, type Brand, type NavEntry, type SwitcherItem } from '@paigasus/app-shell';

/** One organization the switcher can list. Built on the server from myScopes(). */
export type OrgSwitcherOrg = { readonly orgId: string; readonly label: string };

/** Switcher items with FULL-path hrefs, as @paigasus/app-shell requires (SMA-510 spec § 6.2). */
export function orgSwitcherItems(orgs: readonly OrgSwitcherOrg[], basePath: string): SwitcherItem[] {
  return orgs.map((org) => ({ id: org.orgId, label: org.label, href: `${basePath}/orgs/${org.orgId}` }));
}

/**
 * The `[org]` segment of the current URL in lower case, or null on a page outside one organization.
 * The page accepts an upper-case UUID, and the switcher's item ids are lower case (myScopes()).
 */
export function currentOrgId(params: Readonly<Record<string, string | string[] | undefined>> | null): string | null {
  const org = params?.['org'];
  return typeof org === 'string' ? org.toLowerCase() : null;
}

/**
 * AppShell plus one "Organization" switcher. The same props as AppShell, with `orgs` in place of
 * `switchers`: AppShell renders a switcher only from its `switchers` prop, so the component that
 * knows the current `[org]` (a client, through useParams()) must be the one that renders AppShell.
 */
export function OrgSwitcherShell({ brand, nav, orgs, children }: { brand: Brand; nav: readonly NavEntry[]; orgs: readonly OrgSwitcherOrg[]; children: ReactNode }): ReactElement {
  const params = useParams();
  const { basePath } = useZone();
  const items = orgSwitcherItems(orgs, basePath);
  return (
    <AppShell brand={brand} nav={nav} switchers={items.length === 0 ? [] : [{ label: 'Organization', items, currentId: currentOrgId(params) }]}>
      {children}
    </AppShell>
  );
}
```

Run the command of Step 3 again. Expected: `Tests  5 passed (5)`.

- [ ] **Step 5: Write the console layout**

Create `ts/apps/iam-console/app/(console)/layout.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// Every page that needs a session (spec § 5.4). It resolves the session FIRST — requireSession()
// redirects to login when there is none — then builds the shell from four request-scoped reads.
//
// This layout does NOT guard Server Actions: an action is its own request, and each one gets its
// token through iamClients(), which calls requireSession() itself (spec § 3.3, § 5.3).
//
// SessionProvider receives toSessionView() output only — never a token (ADR-0017). This is the
// first real Flight handoff of getPublicConfig().zones to ZoneProvider (SMA-510 spec § 10.4).
import type { ReactElement, ReactNode } from 'react';
import { connection } from 'next/server';
import { AppShell } from '@paigasus/app-shell';
import { toSessionView } from '@paigasus/auth/server';
import { buildNavEntries } from '../../lib/nav';
import { getPublicConfig } from '../../lib/config';
import { discovery } from '../../lib/discovery';
import { currentSession } from '../../lib/iam';
import { mayI } from '../../lib/authorize';
import { ROOT_PRN } from '../../lib/prn';
import { Providers } from '../providers';

export default async function ConsoleLayout({ children }: { children: ReactNode }): Promise<ReactElement> {
  // No prerender, as in app/(public)/layout.tsx: the runtime config does not exist during `next build`.
  await connection();
  const session = await currentSession();
  const { zone, zones } = getPublicConfig();
  const probe = discovery();
  const [iam, gateway, may] = await Promise.all([probe.getServiceState('iam', session.accessToken), probe.getServiceState('gateway', session.accessToken), mayI()]);
  const nav = buildNavEntries({ iam, gateway, zones, auditAllowed: await may('ListAuditLog', ROOT_PRN) });
  const iamBase = zones['iam'] ?? '/iam';
  // No organization switcher yet: it needs myScopes() (the SMA-511 plan's Task 16), which then
  // replaces AppShell with OrgSwitcherShell (app/_components/org-switcher.tsx) and the same props.
  return (
    <Providers zone={zone} zones={zones} session={toSessionView(session)}>
      <AppShell brand={{ label: 'Paigasus IAM', href: `${iamBase}/orgs` }} nav={nav}>
        {children}
      </AppShell>
    </Providers>
  );
}
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:typecheck
```

Expected: exit 0. The e2e tier (Task 22) renders this layout; no unit test does, because it only composes functions that other tests cover. It renders `AppShell` with no switcher until Task 16 swaps in `OrgSwitcherShell`.

- [ ] **Step 6: Teach the Tailwind guard sentinel C — the tests first**

In `ci/tailwind-source/run.mjs`, replace:

```js
const PROBE_TOKEN = '--paigasus' + '-token-probe';
```

with:

```js
const PROBE_TOKEN = '--paigasus' + '-token-probe';
// Sentinel C (SMA-511): declared in ts/packages/paigasus-app-shell/src/shell/app-shell.tsx. It
// proves the console's second `@source` line, the one that covers @paigasus/app-shell.
const PROBE_APP_SHELL = '--paigasus' + '-app-shell-source-probe';
```

In `selfTest()`, replace:

```js
    const good = join(dir, 'good.css');
    writeFileSync(good, `:root{${PROBE_TOKEN}:1}\n.x{${PROBE_SOURCE}:1}\n`);
    const noSource = join(dir, 'no-source.css');
    writeFileSync(noSource, `:root{${PROBE_TOKEN}:1}\n`);
    const noToken = join(dir, 'no-token.css');
    writeFileSync(noToken, `.x{${PROBE_SOURCE}:1}\n`);
    const cleanApp = join(dir, 'page.tsx');
    writeFileSync(cleanApp, 'export default function Page() { return null; }\n');
    const dirtyApp = join(dir, 'dirty.tsx');
    writeFileSync(dirtyApp, `const leak = '${PROBE_SOURCE}';\n`);

    const read = (f) => readFileSync(f, 'utf8');
    expect('a good build passes', verdict({ cssFiles: [good], appFiles: [cleanApp], readFile: read }).length, 0);
    expect('a missing sentinel A fails', verdict({ cssFiles: [noSource], appFiles: [cleanApp], readFile: read }).length, 1);
    expect('a missing sentinel B fails', verdict({ cssFiles: [noToken], appFiles: [cleanApp], readFile: read }).length, 1);
    expect('an empty CSS set fails', verdict({ cssFiles: [], appFiles: [cleanApp], readFile: read }).length, 1);
    expect('a sentinel in the app fails', verdict({ cssFiles: [good], appFiles: [dirtyApp], readFile: read }).length, 1);
    expect('both sentinels missing fails twice', verdict({ cssFiles: [cleanApp], appFiles: [cleanApp], readFile: read }).length, 2);
    expect('an empty appFiles set fails', verdict({ cssFiles: [good], appFiles: [], readFile: read }).length, 1);
```

with:

```js
    const good = join(dir, 'good.css');
    writeFileSync(good, `:root{${PROBE_TOKEN}:1}\n.x{${PROBE_SOURCE}:1}\n.y{${PROBE_APP_SHELL}:1}\n`);
    const noSource = join(dir, 'no-source.css');
    writeFileSync(noSource, `:root{${PROBE_TOKEN}:1}\n.y{${PROBE_APP_SHELL}:1}\n`);
    const noToken = join(dir, 'no-token.css');
    writeFileSync(noToken, `.x{${PROBE_SOURCE}:1}\n.y{${PROBE_APP_SHELL}:1}\n`);
    const noAppShell = join(dir, 'no-app-shell.css');
    writeFileSync(noAppShell, `:root{${PROBE_TOKEN}:1}\n.x{${PROBE_SOURCE}:1}\n`);
    const cleanApp = join(dir, 'page.tsx');
    writeFileSync(cleanApp, 'export default function Page() { return null; }\n');
    const dirtyApp = join(dir, 'dirty.tsx');
    writeFileSync(dirtyApp, `const leak = '${PROBE_SOURCE}';\n`);
    const dirtyAppShell = join(dir, 'dirty-app-shell.tsx');
    writeFileSync(dirtyAppShell, `const leak = '${PROBE_APP_SHELL}';\n`);

    const read = (f) => readFileSync(f, 'utf8');
    expect('a good build passes', verdict({ cssFiles: [good], appFiles: [cleanApp], readFile: read }).length, 0);
    expect('a missing sentinel A fails', verdict({ cssFiles: [noSource], appFiles: [cleanApp], readFile: read }).length, 1);
    expect('a missing sentinel B fails', verdict({ cssFiles: [noToken], appFiles: [cleanApp], readFile: read }).length, 1);
    expect('a missing sentinel C fails', verdict({ cssFiles: [noAppShell], appFiles: [cleanApp], readFile: read }).length, 1);
    expect('an empty CSS set fails', verdict({ cssFiles: [], appFiles: [cleanApp], readFile: read }).length, 1);
    expect('a sentinel in the app fails', verdict({ cssFiles: [good], appFiles: [dirtyApp], readFile: read }).length, 1);
    expect('an app-shell sentinel in the app fails', verdict({ cssFiles: [good], appFiles: [dirtyAppShell], readFile: read }).length, 1);
    expect('all three sentinels missing fails three times', verdict({ cssFiles: [cleanApp], appFiles: [cleanApp], readFile: read }).length, 3);
    expect('an empty appFiles set fails', verdict({ cssFiles: [good], appFiles: [], readFile: read }).length, 1);
```

In `negativeControl()`, replace:

```js
    // (see the guard in verdict()), which would inflate this count past the 2 sentinel failures
    // this control targets.
    const cleanApp = join(dir, 'page.tsx');
    writeFileSync(cleanApp, 'export default function Page() { return null; }\n');
    const failures = verdict({ cssFiles, appFiles: [cleanApp], readFile: (f) => readFileSync(f, 'utf8') });
    if (failures.length !== 2) {
      console.error(`NEGATIVE CONTROL FAIL: expected 2 failures on probe-free CSS, got ${String(failures.length)}`);
```

with:

```js
    // (see the guard in verdict()), which would inflate this count past the 3 sentinel failures
    // this control targets.
    const cleanApp = join(dir, 'page.tsx');
    writeFileSync(cleanApp, 'export default function Page() { return null; }\n');
    const failures = verdict({ cssFiles, appFiles: [cleanApp], readFile: (f) => readFileSync(f, 'utf8') });
    if (failures.length !== 3) {
      console.error(`NEGATIVE CONTROL FAIL: expected 3 failures on probe-free CSS, got ${String(failures.length)}`);
```

Run:

```bash
node ci/tailwind-source/run.mjs --self-test; echo "rc=$?"
node ci/tailwind-source/run.mjs --negative-control; echo "rc=$?"
```

Expected: `SELF-TEST FAIL: a missing sentinel C fails — expected 1 failure(s), got 0` and `rc=2`; then `NEGATIVE CONTROL FAIL: expected 3 failures on probe-free CSS, got 2` and `rc=2`. The assertion does not exist yet, and both modes see that.

- [ ] **Step 7: Add the assertion to the verdict**

In `verdict()`, replace:

```js
    failures.push(`sentinel B (${PROBE_TOKEN}) is absent from the built CSS — the app's \`@import '@paigasus/ui/styles.css'\` did not resolve`);
  }
```

with:

```js
    failures.push(`sentinel B (${PROBE_TOKEN}) is absent from the built CSS — the app's \`@import '@paigasus/ui/styles.css'\` did not resolve`);
  }
  if (!css.includes(PROBE_APP_SHELL)) {
    failures.push(`sentinel C (${PROBE_APP_SHELL}) is absent from the built CSS — the app's Tailwind \`@source\` line no longer covers ts/packages/paigasus-app-shell/src`);
  }
```

In `verdict()`, replace:

```js
    if (text.includes(PROBE_SOURCE) || text.includes(PROBE_TOKEN)) {
```

with:

```js
    if (text.includes(PROBE_SOURCE) || text.includes(PROBE_TOKEN) || text.includes(PROBE_APP_SHELL)) {
```

In `realRun()`, replace:

```js
  console.log(`tailwind-source guard: both sentinels present across ${String(cssFiles.length)} CSS file(s)`);
```

with:

```js
  console.log(`tailwind-source guard: all three sentinels present across ${String(cssFiles.length)} CSS file(s)`);
```

In the header comment at the top of the file, replace:

```js
 * SMA-503 AC 3 — assert a production console build still emits @paigasus/ui's CSS.
```

with:

```js
 * SMA-503 AC 3 — assert a production console build still emits @paigasus/ui's CSS, and (SMA-511)
 * @paigasus/app-shell's.
```

Run the two commands of Step 6 again.

Expected: `tailwind-source self-test: 12 checks passed` and `rc=0`; then `tailwind-source negative control: reported red as expected` and `rc=0`.

- [ ] **Step 8: Put sentinel C into app-shell and watch the real run fail without `@source`**

Replace `ts/packages/paigasus-app-shell/src/shell/app-shell.tsx` with:

```tsx
// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement, ReactNode } from 'react';
import { cn } from '@paigasus/ui';
import { PrimaryNav, type NavEntry } from '../nav/primary-nav';
import { ZoneLink } from '../zone/zone-link';
import { MAIN_ID, SkipLink } from './skip-link';
import { Switcher, type SwitcherProps } from './switcher';
import { UserMenu } from './user-menu';

/*
 * Sentinel C (SMA-511 spec § 7.6). SOURCE_PROBE's value, below, is an arbitrary custom-property
 * utility that exists ONLY there, and it compiles to a single declaration in the built CSS.
 * ci/tailwind-source/run.mjs asserts it reaches the iam-console production build, which is what
 * proves the app's Tailwind `@source` line still covers this package.
 *
 * It follows @paigasus/ui's sentinel A (src/components/table.tsx). Do not remove it, do not rename
 * it, and do not write its literal value anywhere else in this file or under ts/apps/ — Tailwind's
 * scanner reads raw file text, comments included, so a second copy (even in prose) could generate
 * the utility independently and silently disarm the gate. That is why this comment does not spell
 * out the literal itself; see the assignment below, or run.mjs's PROBE_APP_SHELL.
 */
const SOURCE_PROBE = '[--paigasus-app-shell-source-probe:1]';

export type Brand = {
  readonly label: string;
  /** A full path (spec § 6.2). */
  readonly href: string;
};

export type AppShellProps = {
  readonly brand: Brand;
  readonly nav: readonly NavEntry[];
  /** One Switcher per level (org, team). */
  readonly switchers?: readonly SwitcherProps[];
  readonly children: ReactNode;
};

/**
 * The authenticated shell (spec § 8.1). It must be rendered under BOTH ZoneProvider and
 * SessionProvider. UserMenu and PrimaryNav call useSession(), so without a SessionProvider this
 * throws, which makes a missing provider loud. It takes no breadcrumbs: a page renders its own.
 */
export function AppShell({ brand, nav, switchers, children }: AppShellProps): ReactElement {
  return (
    <>
      <SkipLink />
      <header className={cn(SOURCE_PROBE, 'flex flex-wrap items-center gap-4 border-b border-border bg-background px-4 py-2')}>
        <ZoneLink href={brand.href} className="font-semibold">
          {brand.label}
        </ZoneLink>
        <PrimaryNav entries={nav} />
        <div className="ml-auto flex items-center gap-2">
          {switchers?.map((switcher) => (
            <Switcher key={switcher.label} label={switcher.label} items={switcher.items} currentId={switcher.currentId} />
          ))}
          <UserMenu />
        </div>
      </header>
      <main id={MAIN_ID} tabIndex={-1} className="outline-none">
        {children}
      </main>
    </>
  );
}
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:test
```

Expected: FAIL in the guard's real run: `FAIL: sentinel C (--paigasus-app-shell-source-probe) is absent from the built CSS — the app's Tailwind \`@source\` line no longer covers ts/packages/paigasus-app-shell/src`, then `== tailwind-source guard FAILED ==`. Tailwind's scan root is the app directory, and pnpm links `@paigasus/app-shell` through `node_modules`, which Tailwind skips. This is the red that proves the gate can fail.

- [ ] **Step 9: Add the `@source` line and watch the real run pass**

In `ts/apps/iam-console/app/globals.css`, replace:

```css
@source '../../../packages/paigasus-ui/src';
```

with:

```css
@source '../../../packages/paigasus-ui/src';

/*
 * LOAD-BEARING for the same reason (SMA-511 spec § 7.6): the shell's classes live in
 * @paigasus/app-shell, which pnpm also resolves through node_modules. Sentinel C in
 * ci/tailwind-source/run.mjs proves this line; the literal is not written here, because this file
 * is itself a Tailwind scan root.
 */
@source '../../../packages/paigasus-app-shell/src';
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write apps/iam-console/lib/nav.ts apps/iam-console/app/_components/org-switcher.tsx \
  'apps/iam-console/app/(console)/layout.tsx' apps/iam-console/app/globals.css \
  packages/paigasus-app-shell/src/shell/app-shell.tsx \
  apps/iam-console/tests/unit/nav.test.ts apps/iam-console/tests/unit/org-switcher.test.tsx
moon run iam-console-ts:test paigasus-app-shell-ts:test iam-console-ts:typecheck
moon run ts:lint ts:fmt --force
```

The `prettier --write` line formats every `ts/` file that this task creates or edits, before the fmt check and the commit (the Global Constraints).

Expected: exit 0. The guard prints `tailwind-source guard: all three sentinels present across 1 CSS file(s)`. `paigasus-app-shell-ts:test` still passes: the header gained one class name and nothing else changed.

- [ ] **Step 10: Document sentinel C**

In `ci/tailwind-source/README.md`, replace:

```markdown
`@paigasus/ui` contributes. Two independent sentinels:
```

with:

```markdown
`@paigasus/ui` contributes, and (SMA-511) the CSS that `@paigasus/app-shell` contributes. Three
independent sentinels:
```

Then replace:

```markdown
| `--paigasus-token-probe` | `ts/packages/paigasus-ui/src/styles/tokens.css` | the app's `@import '@paigasus/ui/styles.css'` RESOLVED, i.e. the token layer reached the output |
```

with:

```markdown
| `--paigasus-token-probe` | `ts/packages/paigasus-ui/src/styles/tokens.css` | the app's `@import '@paigasus/ui/styles.css'` RESOLVED, i.e. the token layer reached the output |
| `--paigasus-app-shell-source-probe` | `ts/packages/paigasus-app-shell/src/shell/app-shell.tsx` | Tailwind SCANNED `@paigasus/app-shell`'s source, i.e. the app's second `@source` line still covers it |
```

Then, in the Limitations section (line 47), replace:

```markdown
  chunks that hydrated tree contains. A chunk from an earlier build can carry both sentinels
```

with:

```markdown
  chunks that hydrated tree contains. A chunk from an earlier build can carry every sentinel
```

Do not change the quoted *"both sentinels present …"* line below it: it quotes an SMA-503 measurement word for word.

- [ ] **Step 11: Commit**

```bash
git add ts/apps/iam-console/lib/nav.ts ts/apps/iam-console/app/_components/org-switcher.tsx \
  'ts/apps/iam-console/app/(console)/layout.tsx' ts/apps/iam-console/app/globals.css \
  ts/packages/paigasus-app-shell/src/shell/app-shell.tsx ci/tailwind-source/run.mjs ci/tailwind-source/README.md \
  ts/apps/iam-console/tests/unit/nav.test.ts ts/apps/iam-console/tests/unit/org-switcher.test.tsx
git commit -F - <<'EOF'
feat(ts): iam-console shell, navigation and organization switcher (SMA-511)

The (console) layout resolves the session, then renders the session
view, the zone map and AppShell. buildNavEntries takes every entry's
state from navStateOf(): Organizations, Audit (omitted when mayI says
no, absent without iam.audit) and the cross-zone Gateway entry. The
organization switcher reads the [org] segment with useParams(), so the
selection lives in the URL; the layout adopts it once myScopes exists.

globals.css now scans @paigasus/app-shell. A third sentinel in AppShell
proves it in the Tailwind guard's self-test, negative control and real
run; the real run failed with the probe present and the line absent.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```


---

### Task 16: "Your organizations" — `myScopes()`, the `/iam/orgs` page, create organization, and the `actions.ts` structure test

This task implements spec § 5.1 exactly, plus the first mutation (§ 5.3), the structure test that holds every later Server Action, and the organization switcher in the `(console)` layout (spec § 5.4, the Task 15 handoff).

**Files:**
- Create: `ts/apps/iam-console/lib/paging.ts`
- Create: `ts/apps/iam-console/lib/form.ts`
- Create: `ts/apps/iam-console/lib/scopes.ts`
- Create: `ts/apps/iam-console/app/_components/create-form.tsx`
- Create: `ts/apps/iam-console/app/_components/pager.tsx`
- Create: `ts/apps/iam-console/app/(console)/orgs/load.ts`
- Create: `ts/apps/iam-console/app/(console)/orgs/commands.ts`
- Create: `ts/apps/iam-console/app/(console)/orgs/actions.ts`
- Create: `ts/apps/iam-console/app/(console)/orgs/page.tsx`
- Modify: `ts/apps/iam-console/app/(console)/layout.tsx` (Task 15's layout: `AppShell` → `OrgSwitcherShell`)
- Create: `ts/apps/iam-console/tests/integration/support.ts`
- Test: `ts/apps/iam-console/tests/unit/paging.test.ts`
- Test: `ts/apps/iam-console/tests/unit/form.test.ts`
- Test: `ts/apps/iam-console/tests/integration/scopes.test.ts`
- Test: `ts/apps/iam-console/tests/integration/orgs-page.test.ts`
- Test: `ts/apps/iam-console/tests/integration/orgs-commands.test.ts`
- Test: `ts/apps/iam-console/tests/unit/actions-structure.test.ts`
- Test: `ts/apps/iam-console/tests/unit/switcher-orgs.test.ts`

**Interfaces:**
- Consumes: `type ServiceState` (`@paigasus/discovery/types`); `connection` (`next/server`); `callIam`, `IamResult`, `ActionState` (`lib/errors.ts`); `iamClients`, `sessionToken`, `currentSession`, `IamClients` (`lib/iam.ts`); `currentPrincipal`, `Principal` (`lib/principal.ts`); `mayI`, `MayI`, `IamAction` (`lib/authorize.ts`); `discovery` (`lib/discovery.ts`); `buildNavEntries` (`lib/nav.ts`); `getPublicConfig` (`lib/config.ts`); `parseTenancyPrn`, `organizationPrn`, `teamPrn`, `projectPrn`, `ROOT_PRN`, `TenancyRef` (`lib/prn.ts`); `PageError`, `SectionError`, `FormError`; `OrgSwitcherShell` (`app/_components/org-switcher.tsx`, Task 15; `switcherOrgs` returns its `OrgSwitcherOrg` shape); `Providers` (`app/providers.tsx`, Task 10); `startFakeIam`, `denial`, `FakeIam`, `FakeIamHandlers`, `FakeIamCall` (`tests/support/fake-iam.ts`); `createIamClient`, `TenancyService`, `AuthorizationService`, `AuditService`, `disposeTransports` (`@paigasus/sdk/iam`); `Breadcrumbs`, `ZoneLink` (`@paigasus/app-shell`); `EmptyState`, `Table*`, `Field`, `Input` (`@paigasus/ui`).
- Produces: `ScopeEntry`, `MyScopes`, `SCOPE_CAP`, `loadMyScopes`, `myScopes` (`lib/scopes.ts`, exactly as the contract states), `cedarCapabilityOf(state: ServiceState): boolean` (`lib/scopes.ts`, the one check in `myScopes()` that selects "memberships only"), and `switcherOrgs` (`lib/scopes.ts`, the switcher's data); `PAGE_SIZE`, `parseOffset`, `nextOffset`, `parseCursor`, `pageHref(path: string, param: string, offset: number, keep: Readonly<Record<string, number>> = {}): string` (`lib/paging.ts`); `ActionResult`, `toActionResult`, `invalidFormInput`, `formFields` (`lib/form.ts`); `CreateForm`, `CreateFormProps` (`app/_components/create-form.tsx`); `Pager`, `PagerProps` (`app/_components/pager.tsx`; props `label`, `path`, `param`, `offset`, `nextOffset`, and the optional `keep?: Readonly<Record<string, number>>`, which Tasks 17 and 18 use on a page with two lists); `loadOrganizationsPage`, `OrganizationsPageData`, `OrganizationList`, `OrganizationRow` (`orgs/load.ts`); `createOrganizationForm`, `createOrganization` (`orgs/commands.ts`); `createOrganizationAction` (`orgs/actions.ts`); `clientsFor`, `scriptedMayI`, `callsSince`, `IDS` (`tests/integration/support.ts`).

Rules for this task and for Tasks 17–19:
- Every `lib/*.ts` file starts with `import 'server-only';`. Every `commands.ts`, `load.ts` and `members.ts` does too.
- Relative imports in `app/` and `lib/` are extensionless (Turbopack).
- A hidden affordance issues no call. A user action (a page read or a Server Action) always calls IAM. No loader and no command refuses anything because `mayI()` said no (spec § 6.3). Commands take no `mayI` argument at all.
- The fake IAM's handlers are typed per method (Task 11, `FakeIamHandlers`): a handler gets the typed request message, and its return value must fit the response message. A parameter annotation is optional. When a test writes one (for example `(req: { prn: string }) => …`), the request message must be assignable to it. Do not annotate a `oneof` field such as `ListMembershipsRequest.filter`: its type is a union that includes `{ case: undefined; value?: undefined }`, so narrow on `req.filter.case` instead. An array in a response must not hold `undefined`.
- `setHandlers()` REPLACES the whole handler map (Task 11). A method that the new map does not name falls back to the fake's built-in behaviour, so each test scripts every method its code path calls.
- A function that is `async` must contain an `await` (`@typescript-eslint/require-await`). Use `Promise.resolve(…)` in a non-async function instead.
- Put a number into a template literal only through `String(n)` (`@typescript-eslint/restrict-template-expressions`).

- [ ] **Step 1: Check the prerequisites from Tasks 9–15**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console
grep -n '"@connectrpc/connect"\|"zod"\|"typescript"' ts/apps/iam-console/package.json
grep -n "export async function startFakeIam\|export function denial\|export type FakeIamHandlers\|export type FakeIamCall" ts/apps/iam-console/tests/support/fake-iam.ts
grep -n "include" ts/apps/iam-console/vitest.config.ts
grep -n "export function OrgSwitcherShell\|export type OrgSwitcherOrg" ts/apps/iam-console/app/_components/org-switcher.tsx
grep -n "<AppShell" 'ts/apps/iam-console/app/(console)/layout.tsx'
```
Expected: `@connectrpc/connect`, `zod` and `typescript` are listed (Task 9 adds all three); the four fake-IAM exports exist; the vitest `include` matches `tests/**/*.test.ts`; Task 15's `OrgSwitcherShell` and `OrgSwitcherOrg` exist; and Task 15's layout still renders `AppShell` (Step 26 swaps it).
If a line is missing, the task that makes it is not complete (Task 9, 11 or 15). Stop and complete that task first. Every test path in this part depends on them.

- [ ] **Step 2: Write the failing unit tests for `lib/paging.ts` and `lib/form.ts`**

`ts/apps/iam-console/tests/unit/paging.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { PAGE_SIZE, nextOffset, pageHref, parseCursor, parseOffset } from '../../lib/paging';

describe('parseOffset', () => {
  it('reads a plain non-negative integer', () => {
    expect(parseOffset('0')).toBe(0);
    expect(parseOffset('50')).toBe(50);
    expect(parseOffset(['100', '150'])).toBe(100);
  });

  it('reads anything else as 0, because a hand-edited query string is not worth an error page', () => {
    for (const raw of [undefined, '', '-50', '5.5', '1e3', 'abc', ' 50', '9999999999']) {
      expect(parseOffset(raw)).toBe(0);
    }
  });
});

describe('nextOffset', () => {
  it('offers a next page only when the page came back full, because IAM reports no total', () => {
    expect(nextOffset(0, PAGE_SIZE)).toBe(PAGE_SIZE);
    expect(nextOffset(50, PAGE_SIZE)).toBe(100);
    expect(nextOffset(0, PAGE_SIZE - 1)).toBeNull();
    expect(nextOffset(0, 0)).toBeNull();
  });
});

describe('parseCursor', () => {
  it('passes an opaque cursor through and drops an absent or oversized one', () => {
    expect(parseCursor('abc')).toBe('abc');
    expect(parseCursor(['abc', 'def'])).toBe('abc');
    expect(parseCursor(undefined)).toBe('');
    expect(parseCursor('x'.repeat(1025))).toBe('');
  });
});

describe('pageHref', () => {
  it('puts the offset into the one named parameter', () => {
    expect(pageHref('/iam/orgs', 'offset', 50)).toBe('/iam/orgs?offset=50');
    expect(pageHref('/iam/orgs', 'offset', 0)).toBe('/iam/orgs?offset=0');
  });

  it("keeps the other list's offset, so paging one list does not reset the other", () => {
    expect(pageHref('/iam/orgs/x', 'moffset', 50, { offset: 100 })).toBe('/iam/orgs/x?offset=100&moffset=50');
  });

  it('drops a kept offset of 0, because 0 is the default page', () => {
    expect(pageHref('/iam/orgs/x', 'moffset', 50, { offset: 0 })).toBe('/iam/orgs/x?moffset=50');
  });

  it('lets the parameter override a kept entry of the same name', () => {
    expect(pageHref('/iam/orgs', 'offset', 50, { offset: 100 })).toBe('/iam/orgs?offset=50');
  });
});
```

`ts/apps/iam-console/tests/unit/form.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { formFields, invalidFormInput, toActionResult } from '../../lib/form';

describe('lib/form', () => {
  it('reads the named fields and nothing else', () => {
    const form = new FormData();
    form.set('slug', 'acme');
    form.set('extra', 'ignored');
    expect(formFields(form, ['slug', 'name'])).toEqual({ slug: 'acme', name: null });
  });

  it('builds a local invalid-input error that claims nothing about IAM', () => {
    const error = invalidFormInput();
    expect(error.presentation).toBe('invalid-input');
    expect(error.domain).toBeNull();
    expect(error.reason).toBeNull();
    expect(error.correlationId).toBeNull();
    expect(error.retryable).toBe(false);
    // A plain object, so it crosses the Flight boundary as an action result (spec § 5.3).
    expect(structuredClone(error)).toEqual(error);
  });

  it('maps an IamResult to an action result and drops the value', () => {
    expect(toActionResult({ ok: true, value: { anything: 1 } })).toEqual({ ok: true });
    const error = invalidFormInput();
    expect(toActionResult({ ok: false, error })).toEqual({ ok: false, error });
  });
});
```

- [ ] **Step 3: Run the two tests and see them fail**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/paging.test.ts tests/unit/form.test.ts
```
Expected: FAIL. The imports `../../lib/paging` and `../../lib/form` do not resolve.

- [ ] **Step 4: Write `lib/paging.ts` and `lib/form.ts`**

`ts/apps/iam-console/lib/paging.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// Offset paging for the tenancy lists and cursor paging for the audit list (spec § 5.2). IAM's
// tenancy list responses carry no total, so "Next" appears only when a page came back full. A
// missing or malformed offset reads as 0: a hand-edited query string is not an error page.
import 'server-only';

/** IAM's server maximum is 200; the console asks for 50. */
export const PAGE_SIZE = 50;

const MAX_OFFSET_DIGITS = 9;
const MAX_CURSOR_LENGTH = 1024;

function first(raw: string | readonly string[] | undefined): string | undefined {
  return typeof raw === 'string' ? raw : raw?.[0];
}

export function parseOffset(raw: string | readonly string[] | undefined): number {
  const value = first(raw);
  if (value === undefined || value.length > MAX_OFFSET_DIGITS || !/^\d+$/.test(value)) return 0;
  return Number(value);
}

export function nextOffset(offset: number, received: number): number | null {
  return received >= PAGE_SIZE ? offset + PAGE_SIZE : null;
}

/**
 * The href of one page link. `keep` holds the offsets of the OTHER lists on the same page, so paging
 * one list does not reset the others. A kept offset of 0 is the default page and is left out. `param`
 * is set last, so it overrides a kept entry of the same name.
 */
export function pageHref(path: string, param: string, offset: number, keep: Readonly<Record<string, number>> = {}): string {
  const query = new URLSearchParams();
  for (const [name, value] of Object.entries(keep)) {
    if (value > 0) query.set(name, String(value));
  }
  query.set(param, String(offset));
  return `${path}?${query.toString()}`;
}

/** The audit cursor is opaque. IAM validates it and answers `invalid-cursor` for a bad one. */
export function parseCursor(raw: string | readonly string[] | undefined): string {
  const value = first(raw);
  return value === undefined || value.length > MAX_CURSOR_LENGTH ? '' : value;
}
```

`ts/apps/iam-console/lib/form.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// Shared pieces of the Server Action shells (spec § 5.3). zod checks the SHAPE of a form only
// (present, trimmed, bounded). IAM owns every business rule — the slug grammar, the PRN grammar,
// who may act — and answers with a reason the form copy knows (spec § 6.5).
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import type { ActionState, IamResult } from './errors';

/** What a command returns. `null` is only the initial state of `useActionState`. */
export type ActionResult = Exclude<ActionState, null>;

export function toActionResult(result: IamResult<unknown>): ActionResult {
  return result.ok ? { ok: true } : { ok: false, error: result.error };
}

export function formFields(form: FormData, names: readonly string[]): Record<string, FormDataEntryValue | null> {
  return Object.fromEntries(names.map((name) => [name, form.get(name)]));
}

/**
 * A form that zod refused never reached IAM, so this error carries no IAM data: no domain, no
 * reason, no correlation id. `transport` says HTTP 400 because the BFF itself refused the request;
 * the field is for logging only, and nothing branches on it (ADR-0019 E8).
 */
export function invalidFormInput(): PaigasusError {
  return {
    presentation: 'invalid-input',
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: 'Fill in every field of the form.',
    correlationId: null,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'http', status: 400 },
  };
}
```

- [ ] **Step 5: Run the two tests and see them pass**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/paging.test.ts tests/unit/form.test.ts
```
Expected: PASS, 11 tests.

- [ ] **Step 6: Write the shared integration-test support file**

`ts/apps/iam-console/tests/integration/support.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// Helpers for the tier-2 tests (spec § 9.3). The clients talk real gRPC to the fake IAM, so a test
// exercises the SDK's transport and error map, not a hand-built object.
import { AuditService, AuthorizationService, TenancyService, createIamClient } from '@paigasus/sdk/iam';
import type { IamAction, MayI } from '../../lib/authorize';
import type { FakeIam, FakeIamCall } from '../support/fake-iam';

/** Fixed UUIDs. `a` sorts before `b`, so orgA's scopes come first in every list. */
export const IDS = {
  // IAM names a principal `prn:pgs:iam:::principal/<uuid>` (paigasus-iam-core authn.rs), as the fake does.
  principalPrn: 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0',
  orgA: '0190a100-0000-7000-8000-00000000000a',
  orgB: '0190a100-0000-7000-8000-00000000000b',
  teamA1: '0190a1b2-0000-7000-8000-0000000000a1',
  teamB1: '0190a1b2-0000-7000-8000-0000000000b1',
  projectA1: '0190a1c3-0000-7000-8000-0000000000a1',
  projectB1: '0190a1c3-0000-7000-8000-0000000000b1',
  membership: '0190a1d4-0000-7000-8000-0000000000c1',
} as const;

export function clientsFor(iam: FakeIam, token = 'tok-integration') {
  const auth = { bearer: token };
  const options = { baseUrl: iam.grpcUrl };
  return {
    tenancy: createIamClient(TenancyService, options, auth),
    authz: createIamClient(AuthorizationService, options, auth),
    audit: createIamClient(AuditService, options, auth),
  };
}

export type ScriptedMayI = MayI & { readonly asked: readonly (readonly [IamAction, string])[] };

/** A MayI that answers from a table (absent = false) and records every question. */
export function scriptedMayI(allowed: Partial<Record<IamAction, boolean>>): ScriptedMayI {
  const asked: (readonly [IamAction, string])[] = [];
  const mayI = (action: IamAction, resourcePrn: string): Promise<boolean> => {
    asked.push([action, resourcePrn]);
    return Promise.resolve(allowed[action] ?? false);
  };
  return Object.assign(mayI, { asked });
}

/** The calls the fake saw from now on, by method. The fake's log is shared by every test in a file. */
export function callsSince(iam: FakeIam): (method: string) => FakeIamCall[] {
  const start = iam.calls.length;
  return (method) => iam.calls.slice(start).filter((call) => call.method === method);
}
```

- [ ] **Step 7: Write the failing `loadMyScopes` integration test**

`ts/apps/iam-console/tests/integration/scopes.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// Tier 2 (spec § 9.3): loadMyScopes (spec § 5.1) against the fake IAM over real gRPC. Every case
// scripts the answers, and most cases count the calls. The "ListRoleGrants fails" case counts its
// call, because its result is the same when the call never happens. The cedarCapabilityOf cases at
// the bottom are pure: they cover the one check in myScopes() that selects "memberships only".
import { Code } from '@connectrpc/connect';
import type { ServiceState } from '@paigasus/discovery/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { ROOT_PRN, organizationPrn, projectPrn, teamPrn } from '../../lib/prn';
import { SCOPE_CAP, cedarCapabilityOf, loadMyScopes } from '../../lib/scopes';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers } from '../support/fake-iam';
import { IDS, callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const ORG_A = organizationPrn(IDS.orgA);
const ORG_B = organizationPrn(IDS.orgB);
const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);
const TEAM_B1 = teamPrn(IDS.orgB, IDS.teamB1);
const PROJECT_B1 = projectPrn(IDS.orgB, IDS.projectB1);

function principal(...nodePrns: string[]) {
  return { prn: IDS.principalPrn, memberships: nodePrns.map((nodePrn) => ({ nodePrn })) };
}

function ports() {
  const { tenancy, authz } = clientsFor(iam);
  return { tenancy, authz };
}

/** GetOrganization, GetTeam and GetProject from one name table. An unknown PRN is NotFound. */
function tenancyTable(names: Readonly<Record<string, string>>): FakeIamHandlers {
  const name = (prn: string): string => {
    const value = names[prn];
    if (value === undefined) throw denial({ code: Code.NotFound, reason: 'not-found' });
    return value;
  };
  return {
    'tenancy.getOrganization': (req: { prn: string }) => ({ organization: { prn: req.prn, slug: 'slug', name: name(req.prn) } }),
    'tenancy.getTeam': (req: { prn: string }) => ({ team: { prn: req.prn, orgPrn: ORG_A, slug: 'slug', name: name(req.prn) } }),
    'tenancy.getProject': (req: { prn: string }) => ({ project: { prn: req.prn, teamPrn: TEAM_B1, orgPrn: ORG_B, slug: 'slug', name: name(req.prn) } }),
  };
}

function grants(...scopePrns: string[]): FakeIamHandlers {
  return {
    'authz.listRoleGrants': () => ({
      grants: scopePrns.map((scopePrn, index) => ({ id: `grant-${String(index)}`, principalPrn: IDS.principalPrn, roleKey: 'viewer', scopePrn })),
    }),
  };
}

describe('loadMyScopes', () => {
  it('lists the memberships only, and never calls ListRoleGrants, when IAM does not report iam.authz.cedar', async () => {
    iam.setHandlers({ ...tenancyTable({ [ORG_A]: 'Acme', [TEAM_A1]: 'Platform' }), ...grants(PROJECT_B1) });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(ORG_A, TEAM_A1), cedarCapability: false });

    expect(result).toEqual({
      grantsListed: false,
      hiddenCount: 0,
      entries: [
        { kind: 'organization', prn: ORG_A, orgId: IDS.orgA, label: 'Acme', denied: false },
        { kind: 'team', prn: TEAM_A1, orgId: IDS.orgA, teamId: IDS.teamA1, label: 'Platform', denied: false },
      ],
    });
    expect(calls('authz.listRoleGrants')).toHaveLength(0);
  });

  it("adds the scopes of the principal's own role grants when IAM reports iam.authz.cedar", async () => {
    iam.setHandlers({ ...tenancyTable({ [ORG_A]: 'Acme', [PROJECT_B1]: 'Models' }), ...grants(PROJECT_B1) });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(ORG_A), cedarCapability: true });

    expect(result.grantsListed).toBe(true);
    expect(result.entries).toEqual([
      { kind: 'organization', prn: ORG_A, orgId: IDS.orgA, label: 'Acme', denied: false },
      { kind: 'project', prn: PROJECT_B1, orgId: IDS.orgB, teamId: IDS.teamB1, projectId: IDS.projectB1, label: 'Models', denied: false },
    ]);
    const [listed] = calls('authz.listRoleGrants');
    expect(listed?.request).toMatchObject({ principalPrn: IDS.principalPrn, limit: 200, offset: 0n });
  });

  it('gives a team-only scope a direct entry and never asks for its organization', async () => {
    iam.setHandlers({ ...tenancyTable({ [TEAM_A1]: 'Platform' }), ...grants(TEAM_A1) });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(), cedarCapability: true });

    expect(result.entries).toEqual([{ kind: 'team', prn: TEAM_A1, orgId: IDS.orgA, teamId: IDS.teamA1, label: 'Platform', denied: false }]);
    expect(calls('tenancy.getOrganization')).toHaveLength(0);
    expect(calls('tenancy.getTeam')).toHaveLength(1);
  });

  it("takes a project-only scope's team from GetProject", async () => {
    iam.setHandlers({ ...tenancyTable({ [PROJECT_B1]: 'Models' }), ...grants(PROJECT_B1) });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(), cedarCapability: true });

    expect(result.entries).toEqual([{ kind: 'project', prn: PROJECT_B1, orgId: IDS.orgB, teamId: IDS.teamB1, projectId: IDS.projectB1, label: 'Models', denied: false }]);
    expect(calls('tenancy.getProject')).toHaveLength(1);
  });

  it('deduplicates a scope that is both a membership and a grant, in any UUID case', async () => {
    iam.setHandlers({ ...tenancyTable({ [ORG_A]: 'Acme' }), ...grants(ORG_A) });
    const calls = callsSince(iam);
    const upper = `prn:pgs:iam:::organization/${IDS.orgA.toUpperCase()}`;

    const result = await loadMyScopes({ ...ports(), principal: principal(ORG_A, upper), cedarCapability: true });

    expect(result.entries.map((entry) => entry.prn)).toEqual([ORG_A]);
    expect(calls('tenancy.getOrganization')).toHaveLength(1);
  });

  it('skips a grant whose scope is not a tenancy node, such as the root', async () => {
    iam.setHandlers({ ...tenancyTable({ [ORG_A]: 'Acme' }), ...grants(ROOT_PRN, ORG_A) });

    const result = await loadMyScopes({ ...ports(), principal: principal(), cedarCapability: true });

    expect(result.entries.map((entry) => entry.prn)).toEqual([ORG_A]);
  });

  it('marks a denied row, keeps its PRN, and does not fail the page', async () => {
    iam.setHandlers({
      ...tenancyTable({ [TEAM_A1]: 'Platform' }),
      'tenancy.getOrganization': () => {
        throw denial({ correlationId: 'corr-scope-row' });
      },
    });

    const result = await loadMyScopes({ ...ports(), principal: principal(ORG_A, TEAM_A1), cedarCapability: false });

    expect(result.entries).toEqual([
      { kind: 'organization', prn: ORG_A, orgId: IDS.orgA, label: null, denied: true },
      { kind: 'team', prn: TEAM_A1, orgId: IDS.orgA, teamId: IDS.teamA1, label: 'Platform', denied: false },
    ]);
  });

  it('gives a row that failed for another reason no label, but does not call it denied', async () => {
    iam.setHandlers(tenancyTable({}));

    const result = await loadMyScopes({ ...ports(), principal: principal(ORG_A), cedarCapability: false });

    expect(result.entries).toEqual([{ kind: 'organization', prn: ORG_A, orgId: IDS.orgA, label: null, denied: false }]);
  });

  it('shows at most SCOPE_CAP scopes, asks IAM about those only, and counts the rest', async () => {
    const teams = Array.from({ length: SCOPE_CAP + 10 }, (_, index) => teamPrn(IDS.orgA, `0190a1b2-0000-7000-8000-${String(index).padStart(12, '0')}`));
    iam.setHandlers({ 'tenancy.getTeam': (req: { prn: string }) => ({ team: { prn: req.prn, orgPrn: ORG_A, slug: 's', name: 'Team' } }) });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(...teams), cedarCapability: false });

    expect(result.entries).toHaveLength(SCOPE_CAP);
    expect(result.hiddenCount).toBe(10);
    expect(calls('tenancy.getTeam')).toHaveLength(SCOPE_CAP);
  });

  it('lists the memberships only when ListRoleGrants fails', async () => {
    iam.setHandlers({
      ...tenancyTable({ [ORG_A]: 'Acme' }),
      'authz.listRoleGrants': () => {
        throw denial({ code: Code.Unavailable });
      },
    });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(ORG_A), cedarCapability: true });

    expect(result.grantsListed).toBe(false);
    expect(result.entries.map((entry) => entry.prn)).toEqual([ORG_A]);
    // grantsListed is false on the no-call path too, so only the count proves that the call happened.
    expect(calls('authz.listRoleGrants')).toHaveLength(1);
  });

  it('pages ListRoleGrants until a page comes back short', async () => {
    const fullPage = Array.from({ length: 200 }, () => ROOT_PRN);
    iam.setHandlers({
      ...tenancyTable({ [ORG_A]: 'Acme' }),
      'authz.listRoleGrants': (req: { offset: bigint }) => ({
        grants: (req.offset === 0n ? fullPage : [ORG_A]).map((scopePrn, index) => ({ id: `g-${String(index)}`, principalPrn: IDS.principalPrn, roleKey: 'viewer', scopePrn })),
      }),
    });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(), cedarCapability: true });

    expect(calls('authz.listRoleGrants').map((call) => (call.request as { offset: bigint }).offset)).toEqual([0n, 200n]);
    expect(result.entries.map((entry) => entry.prn)).toEqual([ORG_A]);
  });
});

describe('cedarCapabilityOf', () => {
  const CEDAR = ['iam.authz.cedar'];
  const descriptor = (capabilities: string[]) => ({ service: 'iam', version: '1', capabilities });

  it('lists the role grants only when IAM is available and reports iam.authz.cedar', () => {
    expect(cedarCapabilityOf({ state: 'available', service: 'iam', descriptor: descriptor(CEDAR), capabilities: CEDAR })).toBe(true);
  });

  const MEMBERSHIPS_ONLY: readonly (readonly [string, ServiceState])[] = [
    ['IAM is available without iam.authz.cedar', { state: 'available', service: 'iam', descriptor: descriptor(['iam.audit']), capabilities: ['iam.audit'] }],
    ['IAM is degraded, even with iam.authz.cedar in its last descriptor', { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: descriptor(CEDAR), capabilities: CEDAR }],
    ['IAM is absent', { state: 'absent', service: 'iam' }],
  ];

  it.each(MEMBERSHIPS_ONLY)('selects "memberships only" when %s', (_label, state) => {
    expect(cedarCapabilityOf(state)).toBe(false);
  });
});
```

- [ ] **Step 8: Run the test and see it fail**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/integration/scopes.test.ts
```
Expected: FAIL. The import `../../lib/scopes` does not resolve.

- [ ] **Step 9: Write `lib/scopes.ts`**

`ts/apps/iam-console/lib/scopes.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// "Your organizations" (spec § 5.1). Only platform_admin can call ListOrganizations or list
// memberships by principal, and a membership is not a grant: CreateOrganization gives the creator
// an org_admin grant and no membership, and a user can hold a team or project role with no
// organization access. So the scopes are the union of the principal's memberships and, when IAM
// reports `iam.authz.cedar`, the scopes of its OWN role grants (a principal may always list its own
// grants; the RPC needs the capability).
import 'server-only';
import { cache } from 'react';
import type { ServiceState } from '@paigasus/discovery/types';
import { discovery } from './discovery';
import { callIam, type IamResult } from './errors';
import { iamClients, sessionToken, type IamClients } from './iam';
import { currentPrincipal, type Principal } from './principal';
import { organizationPrn, parseTenancyPrn, projectPrn, teamPrn, type TenancyRef } from './prn';

export type ScopeEntry =
  | { kind: 'organization'; prn: string; orgId: string; label: string | null; denied: boolean }
  | { kind: 'team'; prn: string; orgId: string; teamId: string; label: string | null; denied: boolean }
  | { kind: 'project'; prn: string; orgId: string; teamId: string | null; projectId: string; label: string | null; denied: boolean };

export type MyScopes = { entries: ScopeEntry[]; hiddenCount: number; grantsListed: boolean };

/** The page shows at most this many scopes and states how many more exist (spec § 5.1 step 2). */
export const SCOPE_CAP = 50;

const GRANT_PAGE_SIZE = 200;
/** A bound on the grant walk: 2000 grants. A principal with more sees the first 2000 only. */
const GRANT_MAX_PAGES = 10;
const KIND_ORDER: Readonly<Record<TenancyRef['kind'], number>> = { organization: 0, team: 1, project: 2 };

type Scope = { readonly prn: string; readonly ref: TenancyRef; readonly orgId: string; readonly id: string };

function compare(a: string, b: string): number {
  if (a < b) return -1;
  return a > b ? 1 : 0;
}

/** Null when any page fails: the page then says "memberships only" (spec § 5.1 step 1). */
async function listOwnGrantScopes(authz: Pick<IamClients['authz'], 'listRoleGrants'>, principalPrn: string): Promise<string[] | null> {
  const scopes: string[] = [];
  for (let page = 0; page < GRANT_MAX_PAGES; page += 1) {
    const result = await callIam(() => authz.listRoleGrants({ principalPrn, limit: GRANT_PAGE_SIZE, offset: BigInt(page * GRANT_PAGE_SIZE) }));
    if (!result.ok) return null;
    for (const grant of result.value.grants) scopes.push(grant.scopePrn);
    if (result.value.grants.length < GRANT_PAGE_SIZE) break;
  }
  return scopes;
}

/** Parse, drop non-tenancy PRNs (the root, a service account), deduplicate, group by organization. */
function collectScopes(prns: readonly string[]): Scope[] {
  const byKey = new Map<string, Scope>();
  for (const raw of prns) {
    const ref = parseTenancyPrn(raw);
    if (ref === null) continue;
    const id = ref.id.toLowerCase();
    const orgId = (ref.kind === 'organization' ? ref.id : ref.orgId).toLowerCase();
    const key = `${ref.kind}:${id}`;
    if (byKey.has(key)) continue;
    const prn = ref.kind === 'organization' ? organizationPrn(id) : ref.kind === 'team' ? teamPrn(orgId, id) : projectPrn(orgId, id);
    byKey.set(key, { prn, ref, orgId, id });
  }
  return [...byKey.values()].sort((a, b) => compare(a.orgId, b.orgId) || KIND_ORDER[a.ref.kind] - KIND_ORDER[b.ref.kind] || compare(a.id, b.id));
}

/** One call per shown scope. A failed call never fails the page: the row loses its label. */
async function describeScope(tenancy: Pick<IamClients['tenancy'], 'getOrganization' | 'getTeam' | 'getProject'>, scope: Scope): Promise<ScopeEntry> {
  const { prn, orgId, id } = scope;
  if (scope.ref.kind === 'organization') {
    const result = await callIam(() => tenancy.getOrganization({ prn }));
    return { kind: 'organization', prn, orgId, label: result.ok ? (result.value.organization?.name ?? null) : null, denied: !result.ok && result.error.presentation === 'forbidden' };
  }
  if (scope.ref.kind === 'team') {
    // Both UUIDs are in a team PRN, so the row links straight to the team (spec § 5.1 step 3).
    const result = await callIam(() => tenancy.getTeam({ prn }));
    return { kind: 'team', prn, orgId, teamId: id, label: result.ok ? (result.value.team?.name ?? null) : null, denied: !result.ok && result.error.presentation === 'forbidden' };
  }
  // A project PRN holds the org and the project, not the team. GetProject carries team_prn.
  const result = await callIam(() => tenancy.getProject({ prn }));
  const project = result.ok ? result.value.project : undefined;
  const teamRef = project === undefined ? null : parseTenancyPrn(project.teamPrn);
  return {
    kind: 'project',
    prn,
    orgId,
    teamId: teamRef?.kind === 'team' ? teamRef.id.toLowerCase() : null,
    projectId: id,
    label: project?.name ?? null,
    denied: !result.ok && result.error.presentation === 'forbidden',
  };
}

export async function loadMyScopes(deps: {
  tenancy: Pick<IamClients['tenancy'], 'getOrganization' | 'getTeam' | 'getProject'>;
  authz: Pick<IamClients['authz'], 'listRoleGrants'>;
  principal: Principal;
  cedarCapability: boolean;
}): Promise<MyScopes> {
  const grantScopes = deps.cedarCapability ? await listOwnGrantScopes(deps.authz, deps.principal.prn) : null;
  const scopes = collectScopes([...deps.principal.memberships.map((membership) => membership.nodePrn), ...(grantScopes ?? [])]);
  const shown = scopes.slice(0, SCOPE_CAP);
  const entries = await Promise.all(shown.map((scope) => describeScope(deps.tenancy, scope)));
  return { entries, hiddenCount: scopes.length - shown.length, grantsListed: grantScopes !== null };
}

/**
 * True when myScopes() may list the principal's own role grants: IAM is available and reports
 * `iam.authz.cedar`. Any other state selects "memberships only" (spec § 5.1 step 1). A degraded IAM
 * selects it too, even when its last descriptor listed the capability.
 */
export function cedarCapabilityOf(state: ServiceState): boolean {
  return state.state === 'available' && state.capabilities.includes('iam.authz.cedar');
}

/** One per request. The page and the switcher share it (spec § 5.1). */
export const myScopes = cache(async (): Promise<IamResult<MyScopes>> => {
  const principal = await currentPrincipal();
  if (!principal.ok) return principal;
  const [clients, token] = await Promise.all([iamClients(), sessionToken()]);
  const iam = await discovery().getServiceState('iam', token);
  return { ok: true, value: await loadMyScopes({ tenancy: clients.tenancy, authz: clients.authz, principal: principal.value, cedarCapability: cedarCapabilityOf(iam) }) };
});
```

- [ ] **Step 10: Run the test and see it pass**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/integration/scopes.test.ts
```
Expected: PASS, 15 tests (11 `loadMyScopes` cases and 4 `cedarCapabilityOf` cases). If the dedup case fails because `parseTenancyPrn` refuses an upper-case UUID, that is a defect in Task 8's reader: the kernel canonicalizes upper case (`prn_canonical.json` has that vector). Fix the reader, not this test.

- [ ] **Step 11: Write the failing loader test for `/iam/orgs`**

`ts/apps/iam-console/tests/integration/orgs-page.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The "All organizations" section (spec § 5.1 step 5) and the create affordance. The section is
// an affordance: when mayI() says no, it is hidden and issues NO call. When mayI() says yes and
// IAM says no, the section shows IAM's 403 inline and the page lives (AC 2).
import { ErrorReason } from '@paigasus/sdk/errors/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { loadOrganizationsPage } from '../../app/(console)/orgs/load';
import { PAGE_SIZE } from '../../lib/paging';
import { ROOT_PRN, organizationPrn } from '../../lib/prn';
import { denial, startFakeIam, type FakeIam } from '../support/fake-iam';
import { callsSince, clientsFor, scriptedMayI } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

function organizations(count: number) {
  return Array.from({ length: count }, (_, index) => ({
    prn: organizationPrn(`0190a100-0000-7000-8000-${String(index).padStart(12, '0')}`),
    slug: `org-${String(index)}`,
    name: `Org ${String(index)}`,
  }));
}

describe('loadOrganizationsPage', () => {
  it('hides "All organizations" and makes no ListOrganizations call when mayI says no', async () => {
    iam.setHandlers({ 'tenancy.listOrganizations': () => ({ organizations: organizations(1) }) });
    const calls = callsSince(iam);
    const mayI = scriptedMayI({ CreateOrganization: true });

    const data = await loadOrganizationsPage({ tenancy: clientsFor(iam).tenancy, mayI }, { offset: 0 });

    expect(data).toEqual({ canCreateOrganization: true, all: null });
    expect(calls('tenancy.listOrganizations')).toHaveLength(0);
    expect(mayI.asked).toEqual(
      expect.arrayContaining([
        ['ListOrganizations', ROOT_PRN],
        ['CreateOrganization', ROOT_PRN],
      ]),
    );
  });

  it('lists one page, links each row by UUID, and offers "Next" only when the page is full', async () => {
    iam.setHandlers({ 'tenancy.listOrganizations': (req: { offset: bigint }) => ({ organizations: organizations(req.offset === 0n ? PAGE_SIZE : 3) }) });
    const calls = callsSince(iam);
    const deps = { tenancy: clientsFor(iam).tenancy, mayI: scriptedMayI({ ListOrganizations: true }) };

    const first = await loadOrganizationsPage(deps, { offset: 0 });
    const second = await loadOrganizationsPage(deps, { offset: PAGE_SIZE });

    if (first.all?.ok !== true || second.all?.ok !== true) throw new Error('expected two listed pages');
    expect(first.all.value.rows).toHaveLength(PAGE_SIZE);
    expect(first.all.value.rows[0]).toEqual({ prn: organizationPrn('0190a100-0000-7000-8000-000000000000'), orgId: '0190a100-0000-7000-8000-000000000000', slug: 'org-0', name: 'Org 0' });
    expect(first.all.value.nextOffset).toBe(PAGE_SIZE);
    expect(second.all.value.nextOffset).toBeNull();
    expect(calls('tenancy.listOrganizations').map((call) => call.request)).toEqual([
      expect.objectContaining({ limit: PAGE_SIZE, offset: 0n }),
      expect.objectContaining({ limit: PAGE_SIZE, offset: BigInt(PAGE_SIZE) }),
    ]);
  });

  it("shows IAM's 403 inline when mayI says yes and IAM denies (AC 2)", async () => {
    iam.setHandlers({
      'tenancy.listOrganizations': () => {
        throw denial({ correlationId: 'corr-all-orgs' });
      },
    });
    const calls = callsSince(iam);

    const data = await loadOrganizationsPage({ tenancy: clientsFor(iam).tenancy, mayI: scriptedMayI({ ListOrganizations: true }) }, { offset: 0 });

    if (data.all === null || data.all.ok) throw new Error('expected an inline error');
    expect(data.all.error.presentation).toBe('forbidden');
    expect(data.all.error.reason).toBe(ErrorReason.FORBIDDEN);
    expect(data.all.error.correlationId).toBe('corr-all-orgs');
    expect(calls('tenancy.listOrganizations')).toHaveLength(1);
  });
});
```

- [ ] **Step 12: Run it and see it fail**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/integration/orgs-page.test.ts
```
Expected: FAIL. The import `../../app/(console)/orgs/load` does not resolve.

- [ ] **Step 13: Write `app/(console)/orgs/load.ts`**

`ts/apps/iam-console/app/(console)/orgs/load.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The loader of /iam/orgs (spec § 5.2). It takes ports, so the tier-2 tests call it with a client
// to the fake IAM and a scripted mayI. The "Your organizations" data is lib/scopes.ts' myScopes().
import 'server-only';
import type { MayI } from '../../../lib/authorize';
import { callIam, type IamResult } from '../../../lib/errors';
import type { IamClients } from '../../../lib/iam';
import { PAGE_SIZE, nextOffset } from '../../../lib/paging';
import { ROOT_PRN, parseTenancyPrn } from '../../../lib/prn';

export type OrganizationRow = { readonly prn: string; readonly orgId: string | null; readonly slug: string; readonly name: string };
export type OrganizationList = { readonly rows: readonly OrganizationRow[]; readonly offset: number; readonly nextOffset: number | null };
export type OrganizationsPageData = {
  readonly canCreateOrganization: boolean;
  /** `null`: the section is hidden, because mayI('ListOrganizations', root) said no. */
  readonly all: IamResult<OrganizationList> | null;
};
export type OrganizationsPageDeps = {
  readonly tenancy: Pick<IamClients['tenancy'], 'listOrganizations'>;
  readonly mayI: MayI;
};

export async function loadOrganizationsPage(deps: OrganizationsPageDeps, params: { readonly offset: number }): Promise<OrganizationsPageData> {
  // Both RPCs check against the root PRN (adapters/grpc/tenancy.rs:135 and :193).
  const [canList, canCreateOrganization] = await Promise.all([deps.mayI('ListOrganizations', ROOT_PRN), deps.mayI('CreateOrganization', ROOT_PRN)]);
  if (!canList) return { canCreateOrganization, all: null };

  const result = await callIam(() => deps.tenancy.listOrganizations({ limit: PAGE_SIZE, offset: BigInt(params.offset) }));
  if (!result.ok) return { canCreateOrganization, all: result };

  const rows = result.value.organizations.map((organization): OrganizationRow => {
    const ref = parseTenancyPrn(organization.prn);
    return { prn: organization.prn, orgId: ref?.kind === 'organization' ? ref.id.toLowerCase() : null, slug: organization.slug, name: organization.name };
  });
  return { canCreateOrganization, all: { ok: true, value: { rows, offset: params.offset, nextOffset: nextOffset(params.offset, rows.length) } } };
}
```

- [ ] **Step 14: Run it and see it pass**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/integration/orgs-page.test.ts
```
Expected: PASS, 3 tests.

- [ ] **Step 15: Write the failing command test**

`ts/apps/iam-console/tests/integration/orgs-commands.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The create-organization command (spec § 5.3) against the fake IAM. A command takes NO mayI, so
// it cannot pre-judge: the "direction 1" case sends what a hidden button would send, and the call
// reaches IAM. The other half of the rule, that no actions.ts names mayI, lives in
// tests/unit/actions-structure.test.ts.
import { Code } from '@connectrpc/connect';
import { ErrorReason } from '@paigasus/sdk/errors/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createOrganization, createOrganizationForm } from '../../app/(console)/orgs/commands';
import { denial, startFakeIam, type FakeIam } from '../support/fake-iam';
import { callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const created = (req: { slug: string; name: string }) => ({ organization: { prn: 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000f1', slug: req.slug, name: req.name } });

describe('createOrganizationForm', () => {
  it('trims the fields and refuses an empty, missing or non-text one', () => {
    expect(createOrganizationForm.safeParse({ slug: ' acme ', name: 'Acme' }).data).toEqual({ slug: 'acme', name: 'Acme' });
    expect(createOrganizationForm.safeParse({ slug: '', name: 'Acme' }).success).toBe(false);
    expect(createOrganizationForm.safeParse({ slug: 'acme', name: null }).success).toBe(false);
    expect(createOrganizationForm.safeParse({ slug: new File(['x'], 'x.txt'), name: 'Acme' }).success).toBe(false);
  });
});

describe('createOrganization', () => {
  it('sends the slug and the name and returns ok', async () => {
    iam.setHandlers({ 'tenancy.createOrganization': created });
    const calls = callsSince(iam);

    const result = await createOrganization({ tenancy: clientsFor(iam).tenancy }, { slug: 'acme', name: 'Acme' });

    expect(result).toEqual({ ok: true });
    expect(calls('tenancy.createOrganization').map((call) => call.request)).toEqual([expect.objectContaining({ slug: 'acme', name: 'Acme' })]);
  });

  it('is not pre-judged: the command takes no mayI, so a hidden button does not stop the call (AC 2, direction 1)', async () => {
    iam.setHandlers({ 'tenancy.createOrganization': created });
    const calls = callsSince(iam);

    const result = await createOrganization({ tenancy: clientsFor(iam).tenancy }, { slug: 'hidden', name: 'Hidden' });

    expect(result).toEqual({ ok: true });
    expect(calls('tenancy.createOrganization')).toHaveLength(1);
  });

  it("returns IAM's 403 as a form error with the correlation id", async () => {
    iam.setHandlers({
      'tenancy.createOrganization': () => {
        throw denial({ correlationId: 'corr-create-org' });
      },
    });

    const result = await createOrganization({ tenancy: clientsFor(iam).tenancy }, { slug: 'acme', name: 'Acme' });

    if (result.ok) throw new Error('expected a denial');
    expect(result.error.presentation).toBe('forbidden');
    expect(result.error.correlationId).toBe('corr-create-org');
  });

  it('returns a slug conflict with its reason, so the form can show the conflict copy', async () => {
    iam.setHandlers({
      'tenancy.createOrganization': () => {
        throw denial({ code: Code.AlreadyExists, reason: 'slug-conflict', correlationId: 'corr-slug' });
      },
    });

    const result = await createOrganization({ tenancy: clientsFor(iam).tenancy }, { slug: 'taken', name: 'Taken' });

    if (result.ok) throw new Error('expected a conflict');
    expect(result.error.presentation).toBe('conflict');
    expect(result.error.reason).toBe(ErrorReason.SLUG_CONFLICT);
  });
});
```

- [ ] **Step 16: Run it and see it fail**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/integration/orgs-commands.test.ts
```
Expected: FAIL. The import `../../app/(console)/orgs/commands` does not resolve.

- [ ] **Step 17: Write `app/(console)/orgs/commands.ts`**

`ts/apps/iam-console/app/(console)/orgs/commands.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The commands behind this folder's Server Actions (spec § 5.3). A command takes its IAM client as
// a port, so the tier-2 tests call it against the fake IAM with no session and no Next runtime.
// It takes NO mayI: a command cannot refuse an action on the UI's guess. IAM decides (spec § 6.3).
import 'server-only';
import { z } from 'zod';
import { callIam } from '../../../lib/errors';
import { toActionResult, type ActionResult } from '../../../lib/form';
import type { IamClients } from '../../../lib/iam';

const text = z.string().trim().min(1).max(200);

/** The shape of the form, not IAM's rules. IAM validates the slug grammar and answers with a reason. */
export const createOrganizationForm = z.object({ slug: text, name: text });
export type CreateOrganizationInput = z.infer<typeof createOrganizationForm>;

export async function createOrganization(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'createOrganization'> }, input: CreateOrganizationInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.createOrganization({ slug: input.slug, name: input.name })));
}
```

- [ ] **Step 18: Run it and see it pass**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/integration/orgs-commands.test.ts
```
Expected: PASS, 5 tests.

- [ ] **Step 19: Write the failing `actions.ts` structure test**

`ts/apps/iam-console/tests/unit/actions-structure.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// Spec § 5.3: every exported Server Action obtains its client through iamClients(), so
// requireSession() runs for EVERY action. The (console) layout does not guard Server Actions —
// an action is a POST to the page URL, and Next does not render the layout for it (spec § 3.3).
// No actions.ts names mayI (spec § 6.3): a hidden button is cosmetic, and IAM decides. That check
// reads identifiers, not text, so a comment that names mayI() is not a violation.
//
// EXPECTED is a strict-equality list. A new action is a review point: add it here. The negative
// cases at the bottom prove the checker can fail, so a green here is not vacuous.
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const APP_DIR = fileURLToPath(new URL('../../app', import.meta.url));

const EXPECTED: Readonly<Record<string, readonly string[]>> = {
  '(console)/orgs/actions.ts': ['createOrganizationAction'],
};

function findActionFiles(dir: string, prefix = ''): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name === 'node_modules' || entry.name === '.next') continue;
    const relative = prefix === '' ? entry.name : `${prefix}/${entry.name}`;
    if (entry.isDirectory()) found.push(...findActionFiles(path.join(dir, entry.name), relative));
    else if (entry.name === 'actions.ts') found.push(relative);
  }
  return found.sort();
}

type Exported = { readonly name: string; readonly body: ts.Node | undefined };

function isExported(node: ts.Node): boolean {
  return ts.canHaveModifiers(node) && (ts.getModifiers(node) ?? []).some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword);
}

function exportedValues(source: ts.SourceFile): Exported[] {
  const out: Exported[] = [];
  for (const statement of source.statements) {
    if (ts.isFunctionDeclaration(statement) && isExported(statement)) {
      out.push({ name: statement.name?.text ?? 'default', body: statement.body });
    } else if (ts.isVariableStatement(statement) && isExported(statement)) {
      for (const declaration of statement.declarationList.declarations) {
        const init = declaration.initializer;
        const body = init !== undefined && (ts.isArrowFunction(init) || ts.isFunctionExpression(init)) ? init.body : undefined;
        out.push({ name: declaration.name.getText(source), body });
      }
    } else if (ts.isExportAssignment(statement)) {
      out.push({ name: 'default', body: undefined });
    } else if (ts.isExportDeclaration(statement) && !statement.isTypeOnly) {
      // `export { x } from './y'` hides the body where this test cannot read it.
      out.push({ name: `re-export ${statement.getText(source)}`, body: undefined });
    }
  }
  return out;
}

function callsIamClients(node: ts.Node): boolean {
  let found = false;
  const visit = (child: ts.Node): void => {
    if (found) return;
    if (ts.isCallExpression(child) && ts.isIdentifier(child.expression) && child.expression.text === 'iamClients') {
      found = true;
      return;
    }
    ts.forEachChild(child, visit);
  };
  visit(node);
  return found;
}

/** True when any identifier in the tree is `name`: an import, a call, a reference. Comments are not nodes. */
function namesIdentifier(node: ts.Node, name: string): boolean {
  let found = false;
  const visit = (child: ts.Node): void => {
    if (found) return;
    if (ts.isIdentifier(child) && child.text === name) {
      found = true;
      return;
    }
    ts.forEachChild(child, visit);
  };
  visit(node);
  return found;
}

function checkActionsSource(file: string, text: string): { names: string[]; violations: string[] } {
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.ES2022, true, ts.ScriptKind.TS);
  const violations: string[] = [];
  const [first] = source.statements;
  if (first === undefined || !ts.isExpressionStatement(first) || !ts.isStringLiteral(first.expression) || first.expression.text !== 'use server') {
    violations.push(`${file}: the first statement is not 'use server'`);
  }
  const values = exportedValues(source);
  for (const value of values) {
    if (value.body === undefined) violations.push(`${file}: export ${value.name} is not a function whose body this test can read`);
    else if (!callsIamClients(value.body)) violations.push(`${file}: export ${value.name} does not call iamClients()`);
  }
  // createIamClients? covers both lib/iam.ts' createIamClients and the SDK's createIamClient.
  if (/\b(?:iamClientsForToken|createIamClients?)\b/.test(text)) violations.push(`${file}: builds an IAM client without a session`);
  if (namesIdentifier(source, 'mayI')) violations.push(`${file}: consults mayI()`);
  return { names: values.map((value) => value.name).sort(), violations };
}

describe('every Server Action gets its client through iamClients() (spec § 5.3)', () => {
  const files = findActionFiles(APP_DIR);

  it('finds exactly the expected actions.ts files and exports', () => {
    expect(files).toEqual(Object.keys(EXPECTED).sort());
    for (const file of files) {
      expect(checkActionsSource(file, readFileSync(path.join(APP_DIR, file), 'utf8')).names).toEqual([...(EXPECTED[file] ?? [])].sort());
    }
  });

  it('reports no violation in any actions.ts', () => {
    const violations = files.flatMap((file) => checkActionsSource(file, readFileSync(path.join(APP_DIR, file), 'utf8')).violations);
    expect(violations).toEqual([]);
  });

  describe('the checker fails on each broken shape (negative controls)', () => {
    const header = "'use server';\nimport { iamClients } from '../lib/iam';\n";
    it.each([
      ['a function without the call', `${header}export async function a() { return 1; }`, 'does not call iamClients()'],
      ['an arrow without the call', `${header}export const a = async () => 1;`, 'does not call iamClients()'],
      ['a re-export', `${header}export { a } from './other';`, 'is not a function whose body this test can read'],
      ['a default export of a value', `${header}export default iamClients;`, 'is not a function whose body this test can read'],
      ['a missing directive', 'export async function a() { await iamClients(); }', "the first statement is not 'use server'"],
      ['a session-less client', `${header}import { iamClientsForToken } from '../lib/iam';\nexport async function a() { await iamClients(); iamClientsForToken('t'); }`, 'without a session'],
      // No import line: the check reads the text, so the call `createIamClients(` alone must trip it.
      ['a session-less client builder', `${header}export async function a() { await iamClients(); createIamClients({ baseUrl: 'x', token: 't' }); }`, 'without a session'],
      ['an action that consults mayI', `${header}import { mayI } from '../lib/authorize';\nexport async function a() { await iamClients(); const may = await mayI(); return may; }`, 'consults mayI()'],
    ])('%s', (_label, source, message) => {
      expect(checkActionsSource('probe.ts', source).violations.join('\n')).toContain(message);
    });

    it('accepts a correct action, and a comment that names mayI()', () => {
      expect(
        checkActionsSource('probe.ts', `${header}// No action consults mayI().\nexport async function a(_p: unknown, f: FormData) { const c = await iamClients(); return c; }`).violations,
      ).toEqual([]);
    });
  });
});
```

- [ ] **Step 20: Run it and see it fail**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/actions-structure.test.ts
```
Expected: FAIL in "finds exactly the expected actions.ts files and exports": `files` is `[]`, not `['(console)/orgs/actions.ts']`. The negative controls pass.

- [ ] **Step 21: Write `app/(console)/orgs/actions.ts`**

`ts/apps/iam-console/app/(console)/orgs/actions.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
'use server';

// Server Action shells (spec § 5.3). Each one gets its client through iamClients(), so
// requireSession() runs for every action. tests/unit/actions-structure.test.ts checks that every
// export calls it. No action consults mayI(): a hidden button is cosmetic, and IAM decides
// (spec § 6.3). The same test checks that no code in this file names it.
import { revalidatePath } from 'next/cache';
import type { ActionState } from '../../../lib/errors';
import { formFields, invalidFormInput } from '../../../lib/form';
import { iamClients } from '../../../lib/iam';
import { createOrganization, createOrganizationForm } from './commands';

/**
 * basePath-relative. Route groups such as (console) are not part of the URL path, so '/orgs' with
 * 'layout' covers every page under /orgs. Task 22's e2e test asserts that a created organization appears.
 */
const TENANCY_PATH = '/orgs';

export async function createOrganizationAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClients();
  const parsed = createOrganizationForm.safeParse(formFields(form, ['slug', 'name']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await createOrganization({ tenancy: clients.tenancy }, parsed.data);
  if (result.ok) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}
```

- [ ] **Step 22: Run the structure test and see it pass**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/actions-structure.test.ts
```
Expected: PASS, 11 tests.

- [ ] **Step 23: Write the create form (client) and the pager (server)**

`ts/apps/iam-console/app/_components/create-form.tsx`:
```tsx
// SPDX-License-Identifier: Apache-2.0
'use client';

import { useActionState, useId, type ReactElement } from 'react';
import { Field, Input } from '@paigasus/ui';
// A TYPE import: verbatimModuleSyntax erases it, so no server-only module reaches the client bundle.
import type { ActionState } from '../../lib/errors';
import { FormError } from './form-error';

/**
 * The one "create" form of the tenancy screens: a slug, a name and a submit button (spec § 5.3).
 * A page renders it only when mayI() allowed it. The Server Action it posts to never asks mayI():
 * IAM decides (spec § 6.3). `hidden` carries the parent PRN.
 */
export type CreateFormProps = {
  readonly testId: string;
  readonly title: string;
  readonly submitLabel: string;
  readonly action: (previous: ActionState, form: FormData) => Promise<ActionState>;
  readonly hidden?: Readonly<Record<string, string>>;
};

export function CreateForm({ testId, title, submitLabel, action, hidden = {} }: CreateFormProps): ReactElement {
  const [state, formAction, pending] = useActionState(action, null);
  const id = useId();
  return (
    <form action={formAction} aria-label={title} data-testid={testId} className="flex max-w-md flex-col gap-3">
      <h3 className="text-sm font-medium">{title}</h3>
      {Object.entries(hidden).map(([name, value]) => (
        <input key={name} type="hidden" name={name} value={value} />
      ))}
      <Field label="Slug" htmlFor={`${id}-slug`}>
        <Input name="slug" required autoComplete="off" />
      </Field>
      <Field label="Name" htmlFor={`${id}-name`}>
        <Input name="name" required autoComplete="off" />
      </Field>
      <button type="submit" disabled={pending} className="bg-primary text-primary-foreground rounded-pgs self-start px-3 py-1.5 text-sm font-medium disabled:opacity-50">
        {submitLabel}
      </button>
      {state?.ok === true ? (
        <p role="status" className="text-sm">
          Created.
        </p>
      ) : null}
      <div data-testid={`${testId}-error`}>
        <FormError error={state?.ok === false ? state.error : null} />
      </div>
    </form>
  );
}
```

`ts/apps/iam-console/app/_components/pager.tsx`:
```tsx
// SPDX-License-Identifier: Apache-2.0
//
// Offset paging links (spec § 5.2). A SERVER component: ZoneLink is a client component, and a
// server component may render it with string props. Hrefs are full paths (/iam/…), as ZoneLink needs.
// On a page with two lists, `keep` carries the other list's offset, so a link keeps that list's page.
import type { ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import { PAGE_SIZE, pageHref } from '../../lib/paging';

export type PagerProps = {
  readonly label: string;
  readonly path: string;
  readonly param: string;
  readonly offset: number;
  readonly nextOffset: number | null;
  /** The offsets of the other lists on the page, by query parameter. */
  readonly keep?: Readonly<Record<string, number>>;
};

export function Pager({ label, path, param, offset, nextOffset, keep }: PagerProps): ReactElement | null {
  if (offset === 0 && nextOffset === null) return null;
  const previous = Math.max(0, offset - PAGE_SIZE);
  return (
    <nav aria-label={label} className="flex gap-4 text-sm">
      {offset > 0 ? (
        <ZoneLink href={pageHref(path, param, previous, keep)} className="hover:underline">
          Previous
        </ZoneLink>
      ) : null}
      {nextOffset === null ? null : (
        <ZoneLink href={pageHref(path, param, nextOffset, keep)} className="hover:underline">
          Next
        </ZoneLink>
      )}
    </nav>
  );
}
```

- [ ] **Step 24: Write the `/iam/orgs` page**

`ts/apps/iam-console/app/(console)/orgs/page.tsx`:
```tsx
// SPDX-License-Identifier: Apache-2.0
//
// "Your organizations" (spec § 5.1, AC 1). The login ends at /iam/, which sends a signed-in user
// here. The page never uses the login snapshot: myScopes() runs a live Introspect per request.
import type { ReactElement } from 'react';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { CreateForm } from '../../_components/create-form';
import { PageError } from '../../_components/page-error';
import { Pager } from '../../_components/pager';
import { SectionError } from '../../_components/section-error';
import { mayI } from '../../../lib/authorize';
import { iamClients } from '../../../lib/iam';
import { parseOffset } from '../../../lib/paging';
import { myScopes, type ScopeEntry } from '../../../lib/scopes';
import { createOrganizationAction } from './actions';
import { loadOrganizationsPage, type OrganizationList } from './load';

type SearchParams = Promise<Record<string, string | string[] | undefined>>;

const KIND_LABEL: Readonly<Record<ScopeEntry['kind'], string>> = { organization: 'Organization', team: 'Team', project: 'Project' };

/** A denied row does not link: the link would only lead to a 403. */
function scopeHref(entry: ScopeEntry): string | null {
  if (entry.denied) return null;
  if (entry.kind === 'organization') return `/iam/orgs/${entry.orgId}`;
  if (entry.kind === 'team') return `/iam/orgs/${entry.orgId}/teams/${entry.teamId}`;
  return entry.teamId === null ? null : `/iam/orgs/${entry.orgId}/teams/${entry.teamId}/projects/${entry.projectId}`;
}

function scopeLabel(entry: ScopeEntry): string {
  if (entry.label !== null) return entry.label;
  return entry.denied ? 'No access to details' : 'Details not available';
}

function groupByOrganization(entries: readonly ScopeEntry[]): { orgId: string; title: string; entries: ScopeEntry[] }[] {
  const groups = new Map<string, ScopeEntry[]>();
  for (const entry of entries) {
    const list = groups.get(entry.orgId) ?? [];
    list.push(entry);
    groups.set(entry.orgId, list);
  }
  return [...groups.entries()].map(([orgId, list]) => {
    const organization = list.find((entry) => entry.kind === 'organization' && entry.label !== null);
    return { orgId, title: organization?.label ?? `Organization ${orgId}`, entries: list };
  });
}

function OrganizationTable({ list }: { readonly list: OrganizationList }): ReactElement {
  if (list.rows.length === 0 && list.offset === 0) return <EmptyState title="No organizations" />;
  return (
    <>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Slug</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {list.rows.map((row) => (
            <TableRow key={row.prn}>
              <TableCell>
                {row.orgId === null ? (
                  row.name
                ) : (
                  <ZoneLink href={`/iam/orgs/${row.orgId}`} className="hover:underline">
                    {row.name}
                  </ZoneLink>
                )}
              </TableCell>
              <TableCell>{row.slug}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <Pager label="Organization pages" path="/iam/orgs" param="offset" offset={list.offset} nextOffset={list.nextOffset} />
    </>
  );
}

export default async function OrganizationsPage({ searchParams }: { searchParams: SearchParams }): Promise<ReactElement> {
  const [query, scopes, clients, may] = await Promise.all([searchParams, myScopes(), iamClients(), mayI()]);
  if (!scopes.ok) return <PageError error={scopes.error} />;
  const data = await loadOrganizationsPage({ tenancy: clients.tenancy, mayI: may }, { offset: parseOffset(query.offset) });
  const groups = groupByOrganization(scopes.value.entries);

  return (
    <div className="flex flex-col gap-8 p-6">
      <Breadcrumbs items={[{ label: 'Organizations' }]} />
      <section aria-labelledby="scopes-heading" className="flex flex-col gap-3">
        <h1 id="scopes-heading" className="text-2xl font-semibold">
          Your organizations
        </h1>
        {scopes.value.grantsListed ? null : <p className="text-muted-foreground text-sm">Only your memberships are shown. Your role grants are not listed.</p>}
        {groups.length === 0 ? (
          <EmptyState title="You have no organizations yet" description="Ask an administrator to add you to an organization." />
        ) : (
          groups.map((group) => (
            <div key={group.orgId} className="flex flex-col gap-1">
              <h2 className="text-lg font-medium">{group.title}</h2>
              <ul className="flex flex-col gap-1">
                {group.entries.map((entry) => {
                  const href = scopeHref(entry);
                  const label = scopeLabel(entry);
                  return (
                    <li key={entry.prn} className="flex flex-wrap items-baseline gap-2 text-sm">
                      <span className="text-muted-foreground">{KIND_LABEL[entry.kind]}</span>
                      {href === null ? (
                        <span>{label}</span>
                      ) : (
                        <ZoneLink href={href} className="hover:underline">
                          {label}
                        </ZoneLink>
                      )}
                      {entry.denied ? <code className="text-muted-foreground text-xs">{entry.prn}</code> : null}
                    </li>
                  );
                })}
              </ul>
            </div>
          ))
        )}
        {scopes.value.hiddenCount > 0 ? <p className="text-muted-foreground text-sm">{`${String(scopes.value.hiddenCount)} more scopes are not shown.`}</p> : null}
      </section>
      {data.all === null ? null : (
        <section aria-labelledby="all-orgs-heading" className="flex flex-col gap-3">
          <h2 id="all-orgs-heading" className="text-lg font-semibold">
            All organizations
          </h2>
          {data.all.ok ? <OrganizationTable list={data.all.value} /> : <SectionError error={data.all.error} />}
        </section>
      )}
      {data.canCreateOrganization ? <CreateForm testId="create-organization" title="Create organization" submitLabel="Create" action={createOrganizationAction} /> : null}
    </div>
  );
}
```

- [ ] **Step 25: Write the failing test for the organization switcher's data and the layout swap**

Task 15 wrote `OrgSwitcherShell` and its unit test, and left `(console)/layout.tsx` on a bare `AppShell`, because the switcher's data is `myScopes()` (spec § 5.4). This step and the next one wire it in. The data is a pure function of the `myScopes()` result, so a unit test covers it. No unit test renders the async layout (Task 15), so the same file holds the layout to the swap by reading its source, as `actions-structure.test.ts` does.

`ts/apps/iam-console/tests/unit/switcher-orgs.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The organization switcher (spec § 5.4) lists the organizations from myScopes(). The (console)
// layout passes switcherOrgs(await myScopes()) to OrgSwitcherShell (Task 15), which builds the
// hrefs itself. One entry per ORGANIZATION scope, in myScopes() order. A row with no name (a denied
// row, or one whose GetOrganization failed) shows its UUID. An IAM error gives an empty switcher,
// never a failed layout.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { invalidFormInput } from '../../lib/form';
import { switcherOrgs, type MyScopes, type ScopeEntry } from '../../lib/scopes';

const ORG_A = '0190a100-0000-7000-8000-00000000000a';
const ORG_B = '0190a100-0000-7000-8000-00000000000b';
const ORG_C = '0190a100-0000-7000-8000-00000000000c';
const TEAM = '0190a1b2-0000-7000-8000-0000000000a1';
const PROJECT = '0190a1c3-0000-7000-8000-0000000000c1';

const TEAM_ENTRY: ScopeEntry = { kind: 'team', prn: `prn:pgs:iam::${ORG_A}:team/${TEAM}`, orgId: ORG_A, teamId: TEAM, label: 'Platform', denied: false };

const SCOPES: MyScopes = {
  grantsListed: true,
  hiddenCount: 0,
  entries: [
    { kind: 'organization', prn: `prn:pgs:iam:::organization/${ORG_A}`, orgId: ORG_A, label: 'Acme', denied: false },
    TEAM_ENTRY,
    { kind: 'organization', prn: `prn:pgs:iam:::organization/${ORG_B}`, orgId: ORG_B, label: null, denied: true },
    { kind: 'organization', prn: `prn:pgs:iam:::organization/${ORG_C}`, orgId: ORG_C, label: null, denied: false },
    { kind: 'project', prn: `prn:pgs:iam::${ORG_C}:project/${PROJECT}`, orgId: ORG_C, teamId: null, projectId: PROJECT, label: 'Models', denied: false },
  ],
};

const LAYOUT = fileURLToPath(new URL('../../app/(console)/layout.tsx', import.meta.url));

describe('switcherOrgs', () => {
  it('lists one entry per organization scope, in order, and skips the team and project scopes', () => {
    expect(switcherOrgs({ ok: true, value: SCOPES })).toEqual([
      { orgId: ORG_A, label: 'Acme' },
      { orgId: ORG_B, label: ORG_B },
      { orgId: ORG_C, label: ORG_C },
    ]);
  });

  it('gives an empty list when the principal has no organization scope', () => {
    expect(switcherOrgs({ ok: true, value: { grantsListed: false, hiddenCount: 0, entries: [TEAM_ENTRY] } })).toEqual([]);
  });

  it('gives an empty list when myScopes() failed, so the layout still renders', () => {
    expect(switcherOrgs({ ok: false, error: invalidFormInput() })).toEqual([]);
  });
});

describe('the (console) layout', () => {
  it('renders OrgSwitcherShell with switcherOrgs(myScopes()), and no bare AppShell', () => {
    // Strip the line comments first: the layout's comments name myScopes(), and so did Task 15's
    // layout, which had no myScopes() call at all.
    const source = readFileSync(LAYOUT, 'utf8').replace(/^\s*\/\/.*$/gm, '');
    // The call must sit inside the layout's Promise.all([…]), the one that yields `scopes`.
    expect(source).toMatch(/Promise\.all\(\[[^\]]*\bmyScopes\(\)/);
    expect(source).toMatch(/<OrgSwitcherShell\b[^>]*\borgs=\{switcherOrgs\(scopes\)\}/);
    expect(source).not.toMatch(/<AppShell\b/);
  });
});
```

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/switcher-orgs.test.ts
```
Expected: FAIL. `switcherOrgs` is not exported from `../../lib/scopes`, and the layout still renders `<AppShell`.

- [ ] **Step 26: Add `switcherOrgs` and swap `OrgSwitcherShell` into the layout**

Append to `ts/apps/iam-console/lib/scopes.ts` (after `myScopes`):
```ts

/**
 * The organization switcher's entries (spec § 5.4): one per ORGANIZATION scope of the same
 * myScopes() result the page shows. A row with no name shows its UUID. A failed myScopes() gives
 * no switcher, not a failed layout. Plain data: OrgSwitcherShell (a client component) builds the
 * hrefs. The shape is app/_components/org-switcher.tsx's OrgSwitcherOrg; lib/ does not import app/.
 */
export function switcherOrgs(scopes: IamResult<MyScopes>): { orgId: string; label: string }[] {
  if (!scopes.ok) return [];
  return scopes.value.entries.flatMap((entry) => (entry.kind === 'organization' ? [{ orgId: entry.orgId, label: entry.label ?? entry.orgId }] : []));
}
```

Replace the whole of `ts/apps/iam-console/app/(console)/layout.tsx` (Task 15's file) with the code below. It keeps Task 15's `await connection()` as the first statement, and the session, zone map, discovery, `mayI`, nav and `brand`. It adds `myScopes()` to the reads, and it renders `OrgSwitcherShell` in place of `AppShell`. Task 15's two-line "No organization switcher yet" comment goes.
```tsx
// SPDX-License-Identifier: Apache-2.0
//
// Every page that needs a session (spec § 5.4). It resolves the session FIRST — requireSession()
// redirects to login when there is none — then builds the shell from five request-scoped reads.
//
// This layout does NOT guard Server Actions: an action is its own request, and each one gets its
// token through iamClients(), which calls requireSession() itself (spec § 3.3, § 5.3).
//
// SessionProvider receives toSessionView() output only — never a token (ADR-0017). This is the
// first real Flight handoff of getPublicConfig().zones to ZoneProvider (SMA-510 spec § 10.4).
//
// The organization switcher lists the organizations from myScopes() (spec § 5.4). myScopes() is a
// React cache(), so in a Next request the /orgs page and this layout share ONE call. OrgSwitcherShell
// is a client component: it gets plain data only, and reads the [org] segment with useParams().
import type { ReactElement, ReactNode } from 'react';
import { connection } from 'next/server';
import { toSessionView } from '@paigasus/auth/server';
import { buildNavEntries } from '../../lib/nav';
import { getPublicConfig } from '../../lib/config';
import { discovery } from '../../lib/discovery';
import { currentSession } from '../../lib/iam';
import { mayI } from '../../lib/authorize';
import { ROOT_PRN } from '../../lib/prn';
import { myScopes, switcherOrgs } from '../../lib/scopes';
import { OrgSwitcherShell } from '../_components/org-switcher';
import { Providers } from '../providers';

export default async function ConsoleLayout({ children }: { children: ReactNode }): Promise<ReactElement> {
  // No prerender, as in app/(public)/layout.tsx: the runtime config does not exist during `next build`.
  await connection();
  const session = await currentSession();
  const { zone, zones } = getPublicConfig();
  const probe = discovery();
  const [iam, gateway, may, scopes] = await Promise.all([probe.getServiceState('iam', session.accessToken), probe.getServiceState('gateway', session.accessToken), mayI(), myScopes()]);
  const nav = buildNavEntries({ iam, gateway, zones, auditAllowed: await may('ListAuditLog', ROOT_PRN) });
  const iamBase = zones['iam'] ?? '/iam';
  return (
    <Providers zone={zone} zones={zones} session={toSessionView(session)}>
      <OrgSwitcherShell brand={{ label: 'Paigasus IAM', href: `${iamBase}/orgs` }} nav={nav} orgs={switcherOrgs(scopes)}>
        {children}
      </OrgSwitcherShell>
    </Providers>
  );
}
```
Do NOT add a `loading.tsx` under `app/(console)/` (Task 12): its Suspense boundary would let Next commit status 200 before a page's `forbidden()` throws, and the 403 status would be lost.

- [ ] **Step 27: Run the switcher tests and see them pass**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/switcher-orgs.test.ts tests/unit/org-switcher.test.tsx
```
Expected: PASS, 9 tests (4 new, and Task 15's 5 `OrgSwitcherShell` cases).

- [ ] **Step 28: Run the app's checks**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console
pnpm -C ts exec prettier --write apps/iam-console/lib/paging.ts apps/iam-console/lib/form.ts apps/iam-console/lib/scopes.ts \
  apps/iam-console/app/_components/create-form.tsx apps/iam-console/app/_components/pager.tsx \
  'apps/iam-console/app/(console)/orgs/load.ts' 'apps/iam-console/app/(console)/orgs/commands.ts' \
  'apps/iam-console/app/(console)/orgs/actions.ts' 'apps/iam-console/app/(console)/orgs/page.tsx' \
  'apps/iam-console/app/(console)/layout.tsx' \
  apps/iam-console/tests/integration/support.ts apps/iam-console/tests/integration/scopes.test.ts \
  apps/iam-console/tests/integration/orgs-page.test.ts apps/iam-console/tests/integration/orgs-commands.test.ts \
  apps/iam-console/tests/unit/paging.test.ts apps/iam-console/tests/unit/form.test.ts \
  apps/iam-console/tests/unit/actions-structure.test.ts apps/iam-console/tests/unit/switcher-orgs.test.ts
pnpm --dir ts/apps/iam-console exec vitest run tests/unit tests/integration
moon run iam-console-ts:typecheck ts:lint ts:fmt --force
moon run iam-console-ts:build
```
The `prettier --write` line is the normal path, not a fallback. It formats every `ts/` file that this task creates or edits, before the first fmt check and before the commit (the Global Constraints). It can rewrite line breaks in a code block of this task; that changes no behaviour.

Expected: every vitest file passes; typecheck, lint and fmt pass; the build prints the route list with `ƒ /orgs` and ends with exit 0. If the build reports a prerender error on `/orgs`, add `await connection();` (from `next/server`) as the first statement of `OrganizationsPage` too.

- [ ] **Step 29: Commit**

```bash
git add ts/apps/iam-console/lib/paging.ts ts/apps/iam-console/lib/form.ts ts/apps/iam-console/lib/scopes.ts \
  ts/apps/iam-console/app/_components/create-form.tsx ts/apps/iam-console/app/_components/pager.tsx \
  'ts/apps/iam-console/app/(console)/orgs/load.ts' 'ts/apps/iam-console/app/(console)/orgs/commands.ts' \
  'ts/apps/iam-console/app/(console)/orgs/actions.ts' 'ts/apps/iam-console/app/(console)/orgs/page.tsx' \
  'ts/apps/iam-console/app/(console)/layout.tsx' \
  ts/apps/iam-console/tests/integration/support.ts ts/apps/iam-console/tests/integration/scopes.test.ts \
  ts/apps/iam-console/tests/integration/orgs-page.test.ts ts/apps/iam-console/tests/integration/orgs-commands.test.ts \
  ts/apps/iam-console/tests/unit/paging.test.ts ts/apps/iam-console/tests/unit/form.test.ts \
  ts/apps/iam-console/tests/unit/actions-structure.test.ts ts/apps/iam-console/tests/unit/switcher-orgs.test.ts
git commit -F - <<'EOF'
feat(ts): add the your-organizations landing screen to the iam console (SMA-511)

myScopes() unions the principal's memberships with the scopes of its own
role grants when IAM reports iam.authz.cedar, deduplicates them, caps the
list at 50 and describes each row with one tenancy call. A denied row keeps
its PRN and the page lives. The All organizations section appears only when
mayI allows ListOrganizations and shows IAM's 403 inline.

The create-organization Server Action gets its client through iamClients(),
and a structure test holds that rule for every actions.ts in the app.

The console layout now renders the organization switcher. It lists the
organization scopes of the same myScopes() result, and an IAM error gives
an empty switcher.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 17: The organization page — `/iam/orgs/[org]`, create team, attach and detach membership

**Files:**
- Create: `ts/apps/iam-console/app/(console)/orgs/members.ts`
- Create: `ts/apps/iam-console/app/(console)/orgs/members-section.tsx`
- Create: `ts/apps/iam-console/app/_components/membership-form.tsx`
- Create: `ts/apps/iam-console/app/(console)/orgs/[org]/load.ts`
- Create: `ts/apps/iam-console/app/(console)/orgs/[org]/commands.ts`
- Create: `ts/apps/iam-console/app/(console)/orgs/[org]/actions.ts`
- Create: `ts/apps/iam-console/app/(console)/orgs/[org]/page.tsx`
- Modify: `ts/apps/iam-console/app/(console)/orgs/commands.ts` (attach and detach commands)
- Modify: `ts/apps/iam-console/app/(console)/orgs/actions.ts` (attach and detach actions)
- Modify: `ts/apps/iam-console/tests/unit/actions-structure.test.ts` (`EXPECTED`)
- Test: `ts/apps/iam-console/tests/integration/org-page.test.ts`
- Test: `ts/apps/iam-console/tests/integration/membership-commands.test.ts`

**Interfaces:**
- Consumes: everything Task 16 consumes; `isUuid`, `organizationPrn`, `parseTenancyPrn` (`lib/prn.ts`); `PAGE_SIZE`, `nextOffset`, `parseOffset` (`lib/paging.ts`); `toActionResult`, `formFields`, `invalidFormInput`, `ActionResult` (`lib/form.ts`); `CreateForm`, `Pager` with its optional `keep` prop (Task 16).
- Produces: `MemberRow`, `MemberList`, `MembersData`, `loadMembers` (`orgs/members.ts`); `MembersSection`, `MembersSectionProps` (an optional `keep` prop that it passes to its member `Pager`) (`orgs/members-section.tsx`); `AttachMembershipForm`, `DetachMembershipButton` (`app/_components/membership-form.tsx`); `attachMembershipForm`, `attachMembership`, `detachMembershipForm`, `detachMembership` (`orgs/commands.ts`); `attachMembershipAction`, `detachMembershipAction` (`orgs/actions.ts`); `TeamRow`, `TeamList`, `OrganizationPageData`, `loadOrganizationPage` (`orgs/[org]/load.ts`); `createTeamForm`, `createTeam` (`orgs/[org]/commands.ts`); `createTeamAction` (`orgs/[org]/actions.ts`).

IAM checks these actions (`adapters/grpc/tenancy.rs`): `GetOrganization` and `ListTeams` against the org (`:174`, `:351`), `CreateTeam` against the org (`:310`), `AttachMembership` against the node (`:615`), `DetachMembership` against the membership's node (`:652`), `ListMemberships(node_prn)` against the node (`:669-676`). The page therefore asks `mayI('CreateTeam' | 'AttachMembership' | 'DetachMembership', orgPrn)`.

- [ ] **Step 1: Write the failing loader test**

`ts/apps/iam-console/tests/integration/org-page.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The organization page loader (spec § 5.2). A non-UUID segment is notFound() with no IAM call. A
// denied GetOrganization is a PAGE error (the 403 view). An invalid-input answer to GetOrganization
// (IAM's prn-mismatch for a URL-built PRN) is notFound(). A denied list is a SECTION error.
import { Code } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { loadOrganizationPage } from '../../app/(console)/orgs/[org]/load';
import { PAGE_SIZE } from '../../lib/paging';
import { organizationPrn, teamPrn } from '../../lib/prn';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers } from '../support/fake-iam';
import { IDS, callsSince, clientsFor, scriptedMayI } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const ORG_A = organizationPrn(IDS.orgA);

function teams(count: number) {
  return Array.from({ length: count }, (_, index) => ({ prn: teamPrn(IDS.orgA, `0190a1b2-0000-7000-8000-${String(index).padStart(12, '0')}`), orgPrn: ORG_A, slug: `t-${String(index)}`, name: `Team ${String(index)}` }));
}

function world(teamCount: number): FakeIamHandlers {
  return {
    'tenancy.getOrganization': (req: { prn: string }) => ({ organization: { prn: req.prn, slug: 'acme', name: 'Acme' } }),
    'tenancy.listTeams': () => ({ teams: teams(teamCount) }),
    // `filter` is a oneof: its type includes `{ case: undefined }`, so narrow instead of annotating.
    'tenancy.listMemberships': (req) => ({ memberships: [{ id: IDS.membership, principalPrn: IDS.principalPrn, nodePrn: req.filter.case === 'nodePrn' ? req.filter.value : '' }] }),
  };
}

const deps = (allowed: Parameters<typeof scriptedMayI>[0] = {}) => ({ tenancy: clientsFor(iam).tenancy, mayI: scriptedMayI(allowed) });

describe('loadOrganizationPage', () => {
  it('answers not-found for a segment that is not a UUID, and calls IAM not at all', async () => {
    iam.setHandlers(world(1));
    const start = iam.calls.length;

    expect(await loadOrganizationPage(deps(), { org: 'acme', offset: 0, membersOffset: 0 })).toEqual({ kind: 'not-found' });
    expect(iam.calls.length).toBe(start);
  });

  it('returns the page data, with teams, members and the three affordances', async () => {
    iam.setHandlers(world(2));
    const calls = callsSince(iam);
    const d = deps({ CreateTeam: true, AttachMembership: true, DetachMembership: false });

    const data = await loadOrganizationPage(d, { org: IDS.orgA.toUpperCase(), offset: 0, membersOffset: 0 });

    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect(data.orgId).toBe(IDS.orgA);
    expect(data.orgPrn).toBe(ORG_A);
    expect(data.organization).toEqual({ name: 'Acme', slug: 'acme' });
    expect(data.teams).toEqual({
      ok: true,
      value: { offset: 0, nextOffset: null, rows: teams(2).map((team) => ({ prn: team.prn, teamId: team.prn.slice(-36), slug: team.slug, name: team.name })) },
    });
    expect(data.canCreateTeam).toBe(true);
    expect(data.members.canAttach).toBe(true);
    expect(data.members.canDetach).toBe(false);
    expect(data.members.list).toEqual({ ok: true, value: { offset: 0, nextOffset: null, rows: [{ id: IDS.membership, principalPrn: IDS.principalPrn }] } });
    expect(d.mayI.asked).toEqual(expect.arrayContaining([['CreateTeam', ORG_A], ['AttachMembership', ORG_A], ['DetachMembership', ORG_A]]));
    expect(calls('tenancy.listMemberships').map((call) => call.request)).toEqual([expect.objectContaining({ filter: { case: 'nodePrn', value: ORG_A }, limit: PAGE_SIZE, offset: 0n })]);
  });

  it('pages the teams with the offset and offers "Next" only when the page is full', async () => {
    iam.setHandlers(world(PAGE_SIZE));
    const calls = callsSince(iam);

    const data = await loadOrganizationPage(deps(), { org: IDS.orgA, offset: 100, membersOffset: 50 });

    if (data.kind !== 'ok' || !data.teams.ok) throw new Error('expected a team list');
    expect(data.teams.value.nextOffset).toBe(150);
    expect(calls('tenancy.listTeams')[0]?.request).toMatchObject({ orgPrn: ORG_A, limit: PAGE_SIZE, offset: 100n });
    expect(calls('tenancy.listMemberships')[0]?.request).toMatchObject({ offset: 50n });
  });

  it('turns a denied GetOrganization into a page error and asks for nothing else', async () => {
    iam.setHandlers({
      ...world(1),
      'tenancy.getOrganization': () => {
        throw denial({ correlationId: 'corr-org-page' });
      },
    });
    const calls = callsSince(iam);

    const data = await loadOrganizationPage(deps({ CreateTeam: true }), { org: IDS.orgA, offset: 0, membersOffset: 0 });

    if (data.kind !== 'error') throw new Error(`expected an error, got ${data.kind}`);
    expect(data.error.presentation).toBe('forbidden');
    expect(data.error.correlationId).toBe('corr-org-page');
    expect(calls('tenancy.listTeams')).toHaveLength(0);
    expect(calls('tenancy.listMemberships')).toHaveLength(0);
  });

  it('answers not-found when IAM refuses the URL-built PRN as invalid input (prn-mismatch), and asks for nothing else', async () => {
    // Real IAM answers a PRN whose canonical form differs from the stored node with PrnMismatch,
    // which is InvalidArgument (adapters/grpc/tenancy.rs:176-178). Spec § 5.2: a mismatched URL is a 404.
    iam.setHandlers({
      ...world(1),
      'tenancy.getOrganization': () => {
        throw denial({ code: Code.InvalidArgument, reason: 'prn-mismatch', correlationId: 'corr-org-mismatch' });
      },
    });
    const calls = callsSince(iam);

    expect(await loadOrganizationPage(deps({ CreateTeam: true }), { org: IDS.orgA, offset: 0, membersOffset: 0 })).toEqual({ kind: 'not-found' });
    expect(calls('tenancy.getOrganization')).toHaveLength(1);
    expect(calls('tenancy.listTeams')).toHaveLength(0);
    expect(calls('tenancy.listMemberships')).toHaveLength(0);
  });

  it('keeps the page when a list is denied, and puts the error in that section', async () => {
    iam.setHandlers({
      ...world(1),
      'tenancy.listMemberships': () => {
        throw denial({ code: Code.PermissionDenied, correlationId: 'corr-members' });
      },
    });

    const data = await loadOrganizationPage(deps(), { org: IDS.orgA, offset: 0, membersOffset: 0 });

    if (data.kind !== 'ok' || data.members.list.ok) throw new Error('expected a members section error');
    expect(data.members.list.error.correlationId).toBe('corr-members');
    expect(data.teams.ok).toBe(true);
  });
});
```

- [ ] **Step 2: Run it and see it fail**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/integration/org-page.test.ts
```
Expected: FAIL. The import `../../app/(console)/orgs/[org]/load` does not resolve.

- [ ] **Step 3: Write the shared members loader**

`ts/apps/iam-console/app/(console)/orgs/members.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The Members section of every node page (spec § 5.2). A node page is the only place where IAM
// lets a non-admin list memberships: ListMemberships(node_prn) checks against the node.
import 'server-only';
import type { MayI } from '../../../lib/authorize';
import { callIam, type IamResult } from '../../../lib/errors';
import type { IamClients } from '../../../lib/iam';
import { PAGE_SIZE, nextOffset } from '../../../lib/paging';

export type MemberRow = { readonly id: string; readonly principalPrn: string };
export type MemberList = { readonly rows: readonly MemberRow[]; readonly offset: number; readonly nextOffset: number | null };
export type MembersData = { readonly list: IamResult<MemberList>; readonly canAttach: boolean; readonly canDetach: boolean };
export type MembersDeps = { readonly tenancy: Pick<IamClients['tenancy'], 'listMemberships'>; readonly mayI: MayI };

export async function loadMembers(deps: MembersDeps, nodePrn: string, offset: number): Promise<MembersData> {
  const [result, canAttach, canDetach] = await Promise.all([
    callIam(() => deps.tenancy.listMemberships({ filter: { case: 'nodePrn', value: nodePrn }, limit: PAGE_SIZE, offset: BigInt(offset) })),
    deps.mayI('AttachMembership', nodePrn),
    deps.mayI('DetachMembership', nodePrn),
  ]);
  const list: IamResult<MemberList> = result.ok
    ? { ok: true, value: { rows: result.value.memberships.map((membership) => ({ id: membership.id, principalPrn: membership.principalPrn })), offset, nextOffset: nextOffset(offset, result.value.memberships.length) } }
    : result;
  return { list, canAttach, canDetach };
}
```

- [ ] **Step 4: Write the organization page loader**

`ts/apps/iam-console/app/(console)/orgs/[org]/load.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The loader of /iam/orgs/[org] (spec § 5.2). URLs use UUIDs because a PRN holds no slug. The
// organization comes first: when IAM denies it, the page is the 403 view and nothing else runs.
// The PRN comes from the URL, so an invalid-input answer (IAM's prn-mismatch, InvalidArgument,
// adapters/grpc/tenancy.rs:176-178) means that the URL names no such node: notFound().
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import type { MayI } from '../../../../lib/authorize';
import { callIam, type IamResult } from '../../../../lib/errors';
import type { IamClients } from '../../../../lib/iam';
import { PAGE_SIZE, nextOffset } from '../../../../lib/paging';
import { isUuid, organizationPrn, parseTenancyPrn } from '../../../../lib/prn';
import { loadMembers, type MembersData } from '../members';

export type TeamRow = { readonly prn: string; readonly teamId: string | null; readonly slug: string; readonly name: string };
export type TeamList = { readonly rows: readonly TeamRow[]; readonly offset: number; readonly nextOffset: number | null };
export type OrganizationPageData =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly orgPrn: string;
      readonly organization: { readonly name: string; readonly slug: string };
      readonly teams: IamResult<TeamList>;
      readonly canCreateTeam: boolean;
      readonly members: MembersData;
    };
export type OrganizationPageDeps = {
  readonly tenancy: Pick<IamClients['tenancy'], 'getOrganization' | 'listTeams' | 'listMemberships'>;
  readonly mayI: MayI;
};

export async function loadOrganizationPage(deps: OrganizationPageDeps, params: { readonly org: string; readonly offset: number; readonly membersOffset: number }): Promise<OrganizationPageData> {
  if (!isUuid(params.org)) return { kind: 'not-found' };
  const orgId = params.org.toLowerCase();
  const orgPrn = organizationPrn(orgId);

  const got = await callIam(() => deps.tenancy.getOrganization({ prn: orgPrn }));
  if (!got.ok) return got.error.presentation === 'invalid-input' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const organization = got.value.organization;
  if (organization === undefined) return { kind: 'not-found' };

  const [teams, canCreateTeam, members] = await Promise.all([
    callIam(() => deps.tenancy.listTeams({ orgPrn, limit: PAGE_SIZE, offset: BigInt(params.offset) })),
    deps.mayI('CreateTeam', orgPrn),
    loadMembers(deps, orgPrn, params.membersOffset),
  ]);
  const teamList: IamResult<TeamList> = teams.ok
    ? {
        ok: true,
        value: {
          rows: teams.value.teams.map((team): TeamRow => {
            const ref = parseTenancyPrn(team.prn);
            return { prn: team.prn, teamId: ref?.kind === 'team' ? ref.id.toLowerCase() : null, slug: team.slug, name: team.name };
          }),
          offset: params.offset,
          nextOffset: nextOffset(params.offset, teams.value.teams.length),
        },
      }
    : teams;

  return { kind: 'ok', orgId, orgPrn, organization: { name: organization.name, slug: organization.slug }, teams: teamList, canCreateTeam, members };
}
```

- [ ] **Step 5: Run the loader test and see it pass**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/integration/org-page.test.ts
```
Expected: PASS, 6 tests.

- [ ] **Step 6: Write the failing command tests for create team, attach and detach**

`ts/apps/iam-console/tests/integration/membership-commands.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// Create team (spec § 5.3) and the two membership commands every node page shares. The node PRN
// and the membership id arrive as form fields; IAM checks them, so no command pre-judges.
import { ErrorReason } from '@paigasus/sdk/errors/types';
import { Code } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createTeam, createTeamForm } from '../../app/(console)/orgs/[org]/commands';
import { attachMembership, attachMembershipForm, detachMembership, detachMembershipForm } from '../../app/(console)/orgs/commands';
import { organizationPrn, teamPrn } from '../../lib/prn';
import { denial, startFakeIam, type FakeIam } from '../support/fake-iam';
import { IDS, callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const ORG_A = organizationPrn(IDS.orgA);
const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);

describe('createTeam', () => {
  it('creates a team under the organization from the hidden field', async () => {
    iam.setHandlers({ 'tenancy.createTeam': (req: { orgPrn: string; slug: string; name: string }) => ({ team: { prn: TEAM_A1, orgPrn: req.orgPrn, slug: req.slug, name: req.name } }) });
    const calls = callsSince(iam);

    expect(await createTeam({ tenancy: clientsFor(iam).tenancy }, { orgPrn: ORG_A, slug: 'platform', name: 'Platform' })).toEqual({ ok: true });
    expect(calls('tenancy.createTeam').map((call) => call.request)).toEqual([expect.objectContaining({ orgPrn: ORG_A, slug: 'platform', name: 'Platform' })]);
  });

  it("returns IAM's 403 with the correlation id", async () => {
    iam.setHandlers({
      'tenancy.createTeam': () => {
        throw denial({ correlationId: 'corr-create-team' });
      },
    });

    const result = await createTeam({ tenancy: clientsFor(iam).tenancy }, { orgPrn: ORG_A, slug: 'platform', name: 'Platform' });

    if (result.ok) throw new Error('expected a denial');
    expect(result.error.correlationId).toBe('corr-create-team');
  });

  it('refuses a form without the parent PRN', () => {
    expect(createTeamForm.safeParse({ orgPrn: null, slug: 'a', name: 'A' }).success).toBe(false);
  });
});

describe('attachMembership', () => {
  it('sends the typed principal PRN and the node PRN', async () => {
    iam.setHandlers({ 'tenancy.attachMembership': (req: { principalPrn: string; nodePrn: string }) => ({ membership: { id: IDS.membership, principalPrn: req.principalPrn, nodePrn: req.nodePrn } }) });
    const calls = callsSince(iam);

    expect(await attachMembership({ tenancy: clientsFor(iam).tenancy }, { principalPrn: IDS.principalPrn, nodePrn: ORG_A })).toEqual({ ok: true });
    expect(calls('tenancy.attachMembership').map((call) => call.request)).toEqual([expect.objectContaining({ principalPrn: IDS.principalPrn, nodePrn: ORG_A })]);
  });

  it('returns a duplicate membership as a conflict with its reason', async () => {
    iam.setHandlers({
      'tenancy.attachMembership': () => {
        throw denial({ code: Code.AlreadyExists, reason: 'duplicate-membership', correlationId: 'corr-dup' });
      },
    });

    const result = await attachMembership({ tenancy: clientsFor(iam).tenancy }, { principalPrn: IDS.principalPrn, nodePrn: ORG_A });

    if (result.ok) throw new Error('expected a conflict');
    expect(result.error.presentation).toBe('conflict');
    expect(result.error.reason).toBe(ErrorReason.DUPLICATE_MEMBERSHIP);
  });

  it('trims the principal PRN and refuses an empty one', () => {
    expect(attachMembershipForm.safeParse({ principalPrn: `  ${IDS.principalPrn} `, nodePrn: ORG_A }).data).toEqual({ principalPrn: IDS.principalPrn, nodePrn: ORG_A });
    expect(attachMembershipForm.safeParse({ principalPrn: ' ', nodePrn: ORG_A }).success).toBe(false);
  });
});

describe('detachMembership', () => {
  it('sends the membership id', async () => {
    iam.setHandlers({ 'tenancy.detachMembership': () => ({}) });
    const calls = callsSince(iam);

    expect(await detachMembership({ tenancy: clientsFor(iam).tenancy }, { id: IDS.membership })).toEqual({ ok: true });
    expect(calls('tenancy.detachMembership').map((call) => call.request)).toEqual([expect.objectContaining({ id: IDS.membership })]);
  });

  it("returns IAM's 403 with the correlation id", async () => {
    iam.setHandlers({
      'tenancy.detachMembership': () => {
        throw denial({ correlationId: 'corr-detach' });
      },
    });

    const result = await detachMembership({ tenancy: clientsFor(iam).tenancy }, { id: IDS.membership });

    if (result.ok) throw new Error('expected a denial');
    expect(result.error.presentation).toBe('forbidden');
    expect(result.error.correlationId).toBe('corr-detach');
  });

  it('refuses an empty id', () => {
    expect(detachMembershipForm.safeParse({ id: '' }).success).toBe(false);
  });
});
```

- [ ] **Step 7: Run it and see it fail**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/integration/membership-commands.test.ts
```
Expected: FAIL. `../../app/(console)/orgs/[org]/commands` does not resolve, and `orgs/commands` has no `attachMembership` export.

- [ ] **Step 8: Add the membership commands and the create-team command**

Append to `ts/apps/iam-console/app/(console)/orgs/commands.ts` (after `createOrganization`):
```ts

/** A PRN field: bounded text. IAM parses it and answers `invalid-prn` for a bad one. */
const prn = z.string().trim().min(1).max(512);

export const attachMembershipForm = z.object({ principalPrn: prn, nodePrn: prn });
export type AttachMembershipInput = z.infer<typeof attachMembershipForm>;

/** The same for every node: the node PRN is a hidden field of the page that rendered the form. */
export async function attachMembership(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'attachMembership'> }, input: AttachMembershipInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.attachMembership({ principalPrn: input.principalPrn, nodePrn: input.nodePrn })));
}

export const detachMembershipForm = z.object({ id: z.string().trim().min(1).max(64) });
export type DetachMembershipInput = z.infer<typeof detachMembershipForm>;

/** IAM finds the membership's node itself and checks DetachMembership against it. */
export async function detachMembership(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'detachMembership'> }, input: DetachMembershipInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.detachMembership({ id: input.id })));
}
```

`ts/apps/iam-console/app/(console)/orgs/[org]/commands.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The create-team command (spec § 5.3). It takes NO mayI: IAM decides (spec § 6.3).
import 'server-only';
import { z } from 'zod';
import { callIam } from '../../../../lib/errors';
import { toActionResult, type ActionResult } from '../../../../lib/form';
import type { IamClients } from '../../../../lib/iam';

const text = z.string().trim().min(1).max(200);

export const createTeamForm = z.object({ orgPrn: z.string().min(1).max(512), slug: text, name: text });
export type CreateTeamInput = z.infer<typeof createTeamForm>;

export async function createTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'createTeam'> }, input: CreateTeamInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.createTeam({ orgPrn: input.orgPrn, slug: input.slug, name: input.name })));
}
```

- [ ] **Step 9: Run the command tests and see them pass**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/integration/membership-commands.test.ts tests/integration/orgs-commands.test.ts
```
Expected: PASS, 14 tests.

- [ ] **Step 10: Extend the structure test first, and see it fail**

In `ts/apps/iam-console/tests/unit/actions-structure.test.ts`, replace the `EXPECTED` constant with:
```ts
const EXPECTED: Readonly<Record<string, readonly string[]>> = {
  '(console)/orgs/actions.ts': ['attachMembershipAction', 'createOrganizationAction', 'detachMembershipAction'],
  '(console)/orgs/[org]/actions.ts': ['createTeamAction'],
};
```
Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/actions-structure.test.ts
```
Expected: FAIL in "finds exactly the expected actions.ts files and exports": the `[org]` file is missing, and `orgs/actions.ts` exports only `createOrganizationAction`.

- [ ] **Step 11: Write the actions**

Append to `ts/apps/iam-console/app/(console)/orgs/actions.ts`, and change its commands import line to `import { attachMembership, attachMembershipForm, createOrganization, createOrganizationForm, detachMembership, detachMembershipForm } from './commands';`:
```ts

export async function attachMembershipAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClients();
  const parsed = attachMembershipForm.safeParse(formFields(form, ['principalPrn', 'nodePrn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await attachMembership({ tenancy: clients.tenancy }, parsed.data);
  if (result.ok) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function detachMembershipAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClients();
  const parsed = detachMembershipForm.safeParse(formFields(form, ['id']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await detachMembership({ tenancy: clients.tenancy }, parsed.data);
  if (result.ok) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}
```

`ts/apps/iam-console/app/(console)/orgs/[org]/actions.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
'use server';

// See ../actions.ts for the rules every action here follows. tests/unit/actions-structure.test.ts
// holds the iamClients() rule.
import { revalidatePath } from 'next/cache';
import type { ActionState } from '../../../../lib/errors';
import { formFields, invalidFormInput } from '../../../../lib/form';
import { iamClients } from '../../../../lib/iam';
import { createTeam, createTeamForm } from './commands';

export async function createTeamAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClients();
  const parsed = createTeamForm.safeParse(formFields(form, ['orgPrn', 'slug', 'name']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await createTeam({ tenancy: clients.tenancy }, parsed.data);
  if (result.ok) revalidatePath('/orgs', 'layout');
  return result;
}
```

- [ ] **Step 12: Run the structure test and see it pass**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/actions-structure.test.ts
```
Expected: PASS.

- [ ] **Step 13: Write the membership forms**

`ts/apps/iam-console/app/_components/membership-form.tsx`:
```tsx
// SPDX-License-Identifier: Apache-2.0
'use client';

import { useActionState, useId, type ReactElement } from 'react';
import { Field, Input } from '@paigasus/ui';
import type { ActionState } from '../../lib/errors';
import { FormError } from './form-error';

export type MembershipAction = (previous: ActionState, form: FormData) => Promise<ActionState>;

/**
 * Attach by raw principal PRN: IAM has no user lookup RPC (spec § 5.2, a recorded limit, § 12).
 * The page renders this only when mayI('AttachMembership', node) said yes. The action still asks IAM.
 */
export function AttachMembershipForm({ nodePrn, action }: { readonly nodePrn: string; readonly action: MembershipAction }): ReactElement {
  const [state, formAction, pending] = useActionState(action, null);
  const id = useId();
  return (
    <form action={formAction} aria-label="Add member" data-testid="attach-membership" className="flex max-w-md flex-col gap-3">
      <input type="hidden" name="nodePrn" value={nodePrn} />
      <Field label="Principal PRN" htmlFor={`${id}-principal`} description="IAM has no user search yet. Enter the PRN of the principal.">
        <Input name="principalPrn" required autoComplete="off" />
      </Field>
      <button type="submit" disabled={pending} className="bg-primary text-primary-foreground rounded-pgs self-start px-3 py-1.5 text-sm font-medium disabled:opacity-50">
        Add member
      </button>
      {state?.ok === true ? (
        <p role="status" className="text-sm">
          Added.
        </p>
      ) : null}
      <div data-testid="attach-membership-error">
        <FormError error={state?.ok === false ? state.error : null} />
      </div>
    </form>
  );
}

export function DetachMembershipButton({ membershipId, principalPrn, action }: { readonly membershipId: string; readonly principalPrn: string; readonly action: MembershipAction }): ReactElement {
  const [state, formAction, pending] = useActionState(action, null);
  return (
    <form action={formAction} aria-label={`Remove ${principalPrn}`} className="flex flex-col gap-1">
      <input type="hidden" name="id" value={membershipId} />
      <button type="submit" disabled={pending} className="text-destructive self-start text-sm underline disabled:opacity-50">
        Remove
      </button>
      <FormError error={state?.ok === false ? state.error : null} />
    </form>
  );
}
```

- [ ] **Step 14: Write the Members section**

`ts/apps/iam-console/app/(console)/orgs/members-section.tsx`:
```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The Members section of the organization, team and project pages (spec § 5.2). A SERVER
// component: it passes the Server Actions to the client forms as props. `keep` holds the offset
// of the page's other list (for example `{ offset: 50 }`), so a member page link keeps it.
import type { ReactElement } from 'react';
import { EmptyState, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { AttachMembershipForm, DetachMembershipButton } from '../../_components/membership-form';
import { Pager } from '../../_components/pager';
import { SectionError } from '../../_components/section-error';
import { attachMembershipAction, detachMembershipAction } from './actions';
import type { MembersData } from './members';

export type MembersSectionProps = {
  readonly nodePrn: string;
  readonly path: string;
  readonly data: MembersData;
  readonly keep?: Readonly<Record<string, number>>;
};

export function MembersSection({ nodePrn, path, data, keep = {} }: MembersSectionProps): ReactElement {
  return (
    <section aria-labelledby="members-heading" className="flex flex-col gap-3">
      <h2 id="members-heading" className="text-lg font-semibold">
        Members
      </h2>
      {data.list.ok ? (
        data.list.value.rows.length === 0 && data.list.value.offset === 0 ? (
          <EmptyState title="No members" />
        ) : (
          <>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Principal</TableHead>
                  {data.canDetach ? <TableHead>Action</TableHead> : null}
                </TableRow>
              </TableHeader>
              <TableBody>
                {data.list.value.rows.map((row) => (
                  <TableRow key={row.id}>
                    <TableCell>
                      <code className="text-xs">{row.principalPrn}</code>
                    </TableCell>
                    {data.canDetach ? (
                      <TableCell>
                        <DetachMembershipButton membershipId={row.id} principalPrn={row.principalPrn} action={detachMembershipAction} />
                      </TableCell>
                    ) : null}
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            <Pager label="Member pages" path={path} param="moffset" offset={data.list.value.offset} nextOffset={data.list.value.nextOffset} keep={keep} />
          </>
        )
      ) : (
        <SectionError error={data.list.error} />
      )}
      {data.canAttach ? <AttachMembershipForm nodePrn={nodePrn} action={attachMembershipAction} /> : null}
    </section>
  );
}
```

- [ ] **Step 15: Write the organization page**

`ts/apps/iam-console/app/(console)/orgs/[org]/page.tsx`:
```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /iam/orgs/[org] (spec § 5.2). A navigation is a user action, so this page always asks IAM; the
// only things mayI() hides are the create and membership forms (spec § 6.3).
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { CreateForm } from '../../../_components/create-form';
import { PageError } from '../../../_components/page-error';
import { Pager } from '../../../_components/pager';
import { SectionError } from '../../../_components/section-error';
import { mayI } from '../../../../lib/authorize';
import { iamClients } from '../../../../lib/iam';
import { parseOffset } from '../../../../lib/paging';
import { isUuid } from '../../../../lib/prn';
import { MembersSection } from '../members-section';
import { createTeamAction } from './actions';
import { loadOrganizationPage, type TeamList } from './load';

type Props = {
  params: Promise<{ org: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};

function TeamTable({ orgId, list, membersOffset }: { readonly orgId: string; readonly list: TeamList; readonly membersOffset: number }): ReactElement {
  if (list.rows.length === 0 && list.offset === 0) return <EmptyState title="No teams yet" />;
  return (
    <>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Slug</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {list.rows.map((row) => (
            <TableRow key={row.prn}>
              <TableCell>
                {row.teamId === null ? (
                  row.name
                ) : (
                  <ZoneLink href={`/iam/orgs/${orgId}/teams/${row.teamId}`} className="hover:underline">
                    {row.name}
                  </ZoneLink>
                )}
              </TableCell>
              <TableCell>{row.slug}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <Pager label="Team pages" path={`/iam/orgs/${orgId}`} param="offset" offset={list.offset} nextOffset={list.nextOffset} keep={{ moffset: membersOffset }} />
    </>
  );
}

export default async function OrganizationPage({ params, searchParams }: Props): Promise<ReactElement> {
  const [{ org }, query] = await Promise.all([params, searchParams]);
  if (!isUuid(org)) notFound();
  // Each list's pager keeps the other list's offset in its links (pageHref, lib/paging.ts).
  const offset = parseOffset(query.offset);
  const membersOffset = parseOffset(query.moffset);
  const [clients, may] = await Promise.all([iamClients(), mayI()]);
  const data = await loadOrganizationPage({ tenancy: clients.tenancy, mayI: may }, { org, offset, membersOffset });
  if (data.kind === 'not-found') notFound();
  if (data.kind === 'error') return <PageError error={data.error} />;

  return (
    <div className="flex flex-col gap-8 p-6">
      <Breadcrumbs items={[{ label: 'Organizations', href: '/iam/orgs' }, { label: data.organization.name }]} />
      <div>
        <h1 className="text-2xl font-semibold">{data.organization.name}</h1>
        <p className="text-muted-foreground text-sm">{data.organization.slug}</p>
      </div>
      <section aria-labelledby="teams-heading" className="flex flex-col gap-3">
        <h2 id="teams-heading" className="text-lg font-semibold">
          Teams
        </h2>
        {data.teams.ok ? <TeamTable orgId={data.orgId} list={data.teams.value} membersOffset={membersOffset} /> : <SectionError error={data.teams.error} />}
        {data.canCreateTeam ? <CreateForm testId="create-team" title="Create team" submitLabel="Create" action={createTeamAction} hidden={{ orgPrn: data.orgPrn }} /> : null}
      </section>
      <MembersSection nodePrn={data.orgPrn} path={`/iam/orgs/${data.orgId}`} data={data.members} keep={{ offset }} />
    </div>
  );
}
```

- [ ] **Step 16: Run the checks**

First format every file of this task (the Global Constraints). Then run the checks:
```bash
pnpm -C ts exec prettier --write \
  'apps/iam-console/app/(console)/orgs/members.ts' 'apps/iam-console/app/(console)/orgs/members-section.tsx' \
  apps/iam-console/app/_components/membership-form.tsx \
  'apps/iam-console/app/(console)/orgs/commands.ts' 'apps/iam-console/app/(console)/orgs/actions.ts' \
  'apps/iam-console/app/(console)/orgs/[org]/load.ts' 'apps/iam-console/app/(console)/orgs/[org]/commands.ts' \
  'apps/iam-console/app/(console)/orgs/[org]/actions.ts' 'apps/iam-console/app/(console)/orgs/[org]/page.tsx' \
  apps/iam-console/tests/integration/org-page.test.ts apps/iam-console/tests/integration/membership-commands.test.ts \
  apps/iam-console/tests/unit/actions-structure.test.ts
pnpm --dir ts/apps/iam-console exec vitest run tests/unit tests/integration
moon run iam-console-ts:typecheck ts:lint ts:fmt --force
moon run iam-console-ts:build
```
Expected: all pass. The build's route list shows `ƒ /orgs/[org]`.

- [ ] **Step 17: Commit**

```bash
git add 'ts/apps/iam-console/app/(console)/orgs/members.ts' 'ts/apps/iam-console/app/(console)/orgs/members-section.tsx' \
  ts/apps/iam-console/app/_components/membership-form.tsx \
  'ts/apps/iam-console/app/(console)/orgs/commands.ts' 'ts/apps/iam-console/app/(console)/orgs/actions.ts' \
  'ts/apps/iam-console/app/(console)/orgs/[org]/load.ts' 'ts/apps/iam-console/app/(console)/orgs/[org]/commands.ts' \
  'ts/apps/iam-console/app/(console)/orgs/[org]/actions.ts' 'ts/apps/iam-console/app/(console)/orgs/[org]/page.tsx' \
  ts/apps/iam-console/tests/integration/org-page.test.ts ts/apps/iam-console/tests/integration/membership-commands.test.ts \
  ts/apps/iam-console/tests/unit/actions-structure.test.ts
git commit -F - <<'EOF'
feat(ts): add the organization page with team and membership actions (SMA-511)

The page reads the organization, its teams and its memberships, and pages
both lists with an offset. A segment that is not a UUID is a 404 with no
IAM call. A denied organization is the 403 view; a denied list stays in
its own section. An invalid-input answer to the organization's PRN (IAM's
prn-mismatch) is a 404. Each list's pager keeps the offset of the other
list.

Create team, attach membership and detach membership are Server Actions
that always call IAM. Attach and detach live once in orgs/actions.ts, so
the team and project pages reuse them.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 18: The team page with create project, the project page, and URL consistency

**Files:**
- Create: `ts/apps/iam-console/app/(console)/orgs/node-ref.ts`
- Create: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/load.ts`
- Create: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts`
- Create: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/actions.ts`
- Create: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/page.tsx`
- Create: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/load.ts`
- Create: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/page.tsx`
- Modify: `ts/apps/iam-console/tests/unit/actions-structure.test.ts` (`EXPECTED`)
- Test: `ts/apps/iam-console/tests/unit/node-ref.test.ts`
- Test: `ts/apps/iam-console/tests/integration/team-project-pages.test.ts`

**Interfaces:**
- Consumes: Task 16 and Task 17 products (`loadMembers`, `MembersData`, `MembersSection` and its optional `keep` prop, `CreateForm`, `Pager` and its optional `keep` prop, `toActionResult`, `formFields`, `invalidFormInput`); `isUuid`, `teamPrn`, `projectPrn`, `parseTenancyPrn`, `TenancyKind` (`lib/prn.ts`).
- Produces: `sameNode` (`orgs/node-ref.ts`); `ProjectRow`, `ProjectList`, `TeamPageData`, `loadTeamPage` (`[team]/load.ts`); `createProjectForm`, `createProject` (`[team]/commands.ts`); `createProjectAction` (`[team]/actions.ts`); `ProjectPageData`, `loadProjectPage` (`[project]/load.ts`).

Spec § 5.2 "Consistency": a team page compares `GetTeam.org_prn` with `[org]`; a project page compares `GetProject.team_prn` and `.org_prn` with `[team]` and `[org]`. A mismatch renders `notFound()`. IAM stays the authority on access. Note that a project PRN holds the ORG and the project, not the team (`prn:pgs:iam::<org>:project/<id>`), so `[team]` is checked only through `team_prn`.

Real IAM finds a wrong `[org]` segment BEFORE it answers. `GetTeam` and `GetProject` compare the stored node's canonical PRN with the request PRN. On a difference they return `PrnMismatch` (`rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs:331-333`, `:480-482`). IAM runs its access check first (`:328-330`, `:477-479`), so a caller with no access to the node still gets the 403 view. The mismatch error is `InvalidArgument`, and the SDK presents it as `invalid-input`. So both loaders map an `invalid-input` answer of their primary Get to `{ kind: 'not-found' }` (ruling T18.a). The cost: a real invalid input on that call shows the 404, not an error page. The `sameNode` checks stay. For `[org]` they are a second guard. For the project page's `[team]` they are the only guard, because IAM cannot see the team in a project PRN.

- [ ] **Step 1: Write the failing `sameNode` unit test**

`ts/apps/iam-console/tests/unit/node-ref.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { sameNode } from '../../app/(console)/orgs/node-ref';
import { organizationPrn, projectPrn, teamPrn } from '../../lib/prn';

const ORG = '0190a100-0000-7000-8000-00000000000a';
const OTHER = '0190a100-0000-7000-8000-00000000000b';
const TEAM = '0190a1b2-0000-7000-8000-0000000000a1';

describe('sameNode', () => {
  it('matches the kind and the id, in any UUID case', () => {
    expect(sameNode(organizationPrn(ORG), 'organization', ORG)).toBe(true);
    expect(sameNode(organizationPrn(ORG), 'organization', ORG.toUpperCase())).toBe(true);
    expect(sameNode(teamPrn(ORG, TEAM), 'team', TEAM)).toBe(true);
  });

  it('refuses another id, another kind, and a PRN that does not parse', () => {
    expect(sameNode(organizationPrn(OTHER), 'organization', ORG)).toBe(false);
    expect(sameNode(teamPrn(ORG, TEAM), 'organization', TEAM)).toBe(false);
    expect(sameNode(projectPrn(ORG, TEAM), 'team', TEAM)).toBe(false);
    expect(sameNode('', 'team', TEAM)).toBe(false);
    expect(sameNode('not a prn', 'organization', ORG)).toBe(false);
  });
});
```

- [ ] **Step 2: Run it and see it fail**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/node-ref.test.ts
```
Expected: FAIL. The import `../../app/(console)/orgs/node-ref` does not resolve.

- [ ] **Step 3: Write `node-ref.ts`**

`ts/apps/iam-console/app/(console)/orgs/node-ref.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The URL-consistency check of the node pages (spec § 5.2). A team or project PRN holds the org, so
// for /orgs/<wrong org>/teams/<team> IAM itself answers prn-mismatch (InvalidArgument,
// adapters/grpc/tenancy.rs:331-333 and :480-482), and the loaders map that invalid-input answer to
// notFound(). For [org] this check is a second guard. The project page's [team] check is the one
// IAM cannot make: a project PRN holds no team, so only GetProject's team_prn shows a wrong [team].
// On a mismatch the page renders notFound(). This is not an access check: IAM is that.
import 'server-only';
import { parseTenancyPrn, type TenancyKind } from '../../../lib/prn';

export function sameNode(prn: string, kind: TenancyKind, id: string): boolean {
  const ref = parseTenancyPrn(prn);
  return ref !== null && ref.kind === kind && ref.id.toLowerCase() === id.toLowerCase();
}
```

- [ ] **Step 4: Run it and see it pass**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/node-ref.test.ts
```
Expected: PASS, 2 tests.

- [ ] **Step 5: Write the failing loader and command test for the team and project pages**

`ts/apps/iam-console/tests/integration/team-project-pages.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The team and project page loaders and the create-project command (spec § 5.2, § 5.3), against the
// fake IAM. The consistency cases prove that a URL whose segments disagree with IAM's answer is a
// 404, and the non-UUID cases prove that IAM is not called for a malformed URL. The prn-mismatch
// cases script the answer real IAM gives for a wrong [org] (tenancy.rs:331-333, :480-482); the
// other-organization cases script an answer real IAM never gives, and hold the sameNode guard.
import { Code } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createProject } from '../../app/(console)/orgs/[org]/teams/[team]/commands';
import { loadTeamPage } from '../../app/(console)/orgs/[org]/teams/[team]/load';
import { loadProjectPage } from '../../app/(console)/orgs/[org]/teams/[team]/projects/[project]/load';
import { PAGE_SIZE } from '../../lib/paging';
import { organizationPrn, projectPrn, teamPrn } from '../../lib/prn';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers } from '../support/fake-iam';
import { IDS, callsSince, clientsFor, scriptedMayI } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const ORG_A = organizationPrn(IDS.orgA);
const ORG_B = organizationPrn(IDS.orgB);
const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);
const PROJECT_A1 = projectPrn(IDS.orgA, IDS.projectA1);

function world(overrides: { teamOrgPrn?: string; projectTeamPrn?: string; projectOrgPrn?: string } = {}): FakeIamHandlers {
  return {
    'tenancy.getTeam': (req: { prn: string }) => ({ team: { prn: req.prn, orgPrn: overrides.teamOrgPrn ?? ORG_A, slug: 'platform', name: 'Platform' } }),
    'tenancy.getProject': (req: { prn: string }) => ({
      project: { prn: req.prn, teamPrn: overrides.projectTeamPrn ?? TEAM_A1, orgPrn: overrides.projectOrgPrn ?? ORG_A, slug: 'models', name: 'Models' },
    }),
    'tenancy.listProjects': () => ({ projects: [{ prn: PROJECT_A1, teamPrn: TEAM_A1, orgPrn: ORG_A, slug: 'models', name: 'Models' }] }),
    'tenancy.listMemberships': () => ({ memberships: [] }),
  };
}

const deps = (allowed: Parameters<typeof scriptedMayI>[0] = {}) => ({ tenancy: clientsFor(iam).tenancy, mayI: scriptedMayI(allowed) });

describe('loadTeamPage', () => {
  it('answers not-found for a non-UUID segment, and calls IAM not at all', async () => {
    iam.setHandlers(world());
    const start = iam.calls.length;
    expect(await loadTeamPage(deps(), { org: IDS.orgA, team: 'platform', offset: 0, membersOffset: 0 })).toEqual({ kind: 'not-found' });
    expect(await loadTeamPage(deps(), { org: 'acme', team: IDS.teamA1, offset: 0, membersOffset: 0 })).toEqual({ kind: 'not-found' });
    expect(iam.calls.length).toBe(start);
  });

  it('answers not-found when the team belongs to another organization than the URL says', async () => {
    iam.setHandlers(world({ teamOrgPrn: ORG_B }));
    const calls = callsSince(iam);

    expect(await loadTeamPage(deps(), { org: IDS.orgA, team: IDS.teamA1, offset: 0, membersOffset: 0 })).toEqual({ kind: 'not-found' });
    expect(calls('tenancy.listProjects')).toHaveLength(0);
  });

  it('answers not-found when IAM refuses the URL-built team PRN with prn-mismatch (a wrong [org])', async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.getTeam': () => {
        throw denial({ code: Code.InvalidArgument, reason: 'prn-mismatch', correlationId: 'corr-team-mismatch' });
      },
    });
    const calls = callsSince(iam);

    expect(await loadTeamPage(deps({ CreateProject: true }), { org: IDS.orgB, team: IDS.teamA1, offset: 0, membersOffset: 0 })).toEqual({ kind: 'not-found' });
    expect(calls('tenancy.getTeam').map((call) => call.request)).toEqual([expect.objectContaining({ prn: teamPrn(IDS.orgB, IDS.teamA1) })]);
    expect(calls('tenancy.listProjects')).toHaveLength(0);
    expect(calls('tenancy.listMemberships')).toHaveLength(0);
  });

  it('returns the team, its projects, its members and the create-project affordance', async () => {
    iam.setHandlers(world());
    const calls = callsSince(iam);
    const d = deps({ CreateProject: true });

    const data = await loadTeamPage(d, { org: IDS.orgA, team: IDS.teamA1, offset: 0, membersOffset: 0 });

    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect(data).toMatchObject({ orgId: IDS.orgA, teamId: IDS.teamA1, teamPrn: TEAM_A1, team: { name: 'Platform', slug: 'platform' }, canCreateProject: true });
    expect(data.projects).toEqual({ ok: true, value: { offset: 0, nextOffset: null, rows: [{ prn: PROJECT_A1, projectId: IDS.projectA1, slug: 'models', name: 'Models' }] } });
    expect(d.mayI.asked).toEqual(expect.arrayContaining([['CreateProject', TEAM_A1], ['AttachMembership', TEAM_A1], ['DetachMembership', TEAM_A1]]));
    expect(calls('tenancy.getTeam')[0]?.request).toMatchObject({ prn: TEAM_A1 });
    expect(calls('tenancy.listProjects')[0]?.request).toMatchObject({ teamPrn: TEAM_A1, limit: PAGE_SIZE, offset: 0n });
  });

  it('turns a denied GetTeam into a page error', async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.getTeam': () => {
        throw denial({ correlationId: 'corr-team' });
      },
    });

    const data = await loadTeamPage(deps(), { org: IDS.orgA, team: IDS.teamA1, offset: 0, membersOffset: 0 });

    if (data.kind !== 'error') throw new Error(`expected an error, got ${data.kind}`);
    expect(data.error.presentation).toBe('forbidden');
    expect(data.error.correlationId).toBe('corr-team');
  });
});

describe('loadProjectPage', () => {
  const params = { org: IDS.orgA, team: IDS.teamA1, project: IDS.projectA1, membersOffset: 0 };

  it('answers not-found for a non-UUID segment, and calls IAM not at all', async () => {
    iam.setHandlers(world());
    const start = iam.calls.length;
    expect(await loadProjectPage(deps(), { ...params, project: 'models' })).toEqual({ kind: 'not-found' });
    expect(iam.calls.length).toBe(start);
  });

  it('answers not-found when the project belongs to another team or another organization', async () => {
    iam.setHandlers(world({ projectTeamPrn: teamPrn(IDS.orgA, IDS.teamB1) }));
    expect(await loadProjectPage(deps(), params)).toEqual({ kind: 'not-found' });

    iam.setHandlers(world({ projectOrgPrn: ORG_B }));
    expect(await loadProjectPage(deps(), params)).toEqual({ kind: 'not-found' });
  });

  it("answers not-found for IAM's prn-mismatch on the URL-built project PRN (a wrong [org]), but a page error for a denial", async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.getProject': () => {
        throw denial({ code: Code.InvalidArgument, reason: 'prn-mismatch', correlationId: 'corr-project-mismatch' });
      },
    });
    const calls = callsSince(iam);

    expect(await loadProjectPage(deps({ AttachMembership: true }), { ...params, org: IDS.orgB })).toEqual({ kind: 'not-found' });
    expect(calls('tenancy.getProject').map((call) => call.request)).toEqual([expect.objectContaining({ prn: projectPrn(IDS.orgB, IDS.projectA1) })]);
    expect(calls('tenancy.listMemberships')).toHaveLength(0);

    // Only invalid-input becomes a 404: a denied GetProject stays the 403 view.
    iam.setHandlers({
      ...world(),
      'tenancy.getProject': () => {
        throw denial({ correlationId: 'corr-project' });
      },
    });
    const denied = await loadProjectPage(deps(), params);
    if (denied.kind !== 'error') throw new Error(`expected an error, got ${denied.kind}`);
    expect(denied.error.presentation).toBe('forbidden');
    expect(denied.error.correlationId).toBe('corr-project');
  });

  it('returns the project and its members', async () => {
    iam.setHandlers(world());
    const calls = callsSince(iam);
    const d = deps({ AttachMembership: true });

    const data = await loadProjectPage(d, params);

    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect(data).toMatchObject({ orgId: IDS.orgA, teamId: IDS.teamA1, projectId: IDS.projectA1, projectPrn: PROJECT_A1, project: { name: 'Models', slug: 'models' } });
    expect(data.members.canAttach).toBe(true);
    expect(calls('tenancy.getProject')[0]?.request).toMatchObject({ prn: PROJECT_A1 });
    expect(calls('tenancy.listMemberships')[0]?.request).toMatchObject({ filter: { case: 'nodePrn', value: PROJECT_A1 } });
  });
});

describe('createProject', () => {
  it('creates a project under the team from the hidden field', async () => {
    iam.setHandlers({ 'tenancy.createProject': (req: { teamPrn: string; slug: string; name: string }) => ({ project: { prn: PROJECT_A1, teamPrn: req.teamPrn, orgPrn: ORG_A, slug: req.slug, name: req.name } }) });
    const calls = callsSince(iam);

    expect(await createProject({ tenancy: clientsFor(iam).tenancy }, { teamPrn: TEAM_A1, slug: 'models', name: 'Models' })).toEqual({ ok: true });
    expect(calls('tenancy.createProject').map((call) => call.request)).toEqual([expect.objectContaining({ teamPrn: TEAM_A1, slug: 'models', name: 'Models' })]);
  });

  it("returns IAM's 403 with the correlation id", async () => {
    iam.setHandlers({
      'tenancy.createProject': () => {
        throw denial({ correlationId: 'corr-create-project' });
      },
    });

    const result = await createProject({ tenancy: clientsFor(iam).tenancy }, { teamPrn: TEAM_A1, slug: 'models', name: 'Models' });

    if (result.ok) throw new Error('expected a denial');
    expect(result.error.correlationId).toBe('corr-create-project');
  });
});
```

- [ ] **Step 6: Run it and see it fail**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/integration/team-project-pages.test.ts
```
Expected: FAIL. The three imports under `[team]` do not resolve.

- [ ] **Step 7: Write the team page loader and the create-project command**

`ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/load.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The loader of /iam/orgs/[org]/teams/[team] (spec § 5.2).
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import type { MayI } from '../../../../../../lib/authorize';
import { callIam, type IamResult } from '../../../../../../lib/errors';
import type { IamClients } from '../../../../../../lib/iam';
import { PAGE_SIZE, nextOffset } from '../../../../../../lib/paging';
import { isUuid, parseTenancyPrn, teamPrn } from '../../../../../../lib/prn';
import { loadMembers, type MembersData } from '../../../members';
import { sameNode } from '../../../node-ref';

export type ProjectRow = { readonly prn: string; readonly projectId: string | null; readonly slug: string; readonly name: string };
export type ProjectList = { readonly rows: readonly ProjectRow[]; readonly offset: number; readonly nextOffset: number | null };
export type TeamPageData =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly teamId: string;
      readonly teamPrn: string;
      readonly team: { readonly name: string; readonly slug: string };
      readonly projects: IamResult<ProjectList>;
      readonly canCreateProject: boolean;
      readonly members: MembersData;
    };
export type TeamPageDeps = {
  readonly tenancy: Pick<IamClients['tenancy'], 'getTeam' | 'listProjects' | 'listMemberships'>;
  readonly mayI: MayI;
};

export async function loadTeamPage(
  deps: TeamPageDeps,
  params: { readonly org: string; readonly team: string; readonly offset: number; readonly membersOffset: number },
): Promise<TeamPageData> {
  if (!isUuid(params.org) || !isUuid(params.team)) return { kind: 'not-found' };
  const orgId = params.org.toLowerCase();
  const teamId = params.team.toLowerCase();
  const prn = teamPrn(orgId, teamId);

  const got = await callIam(() => deps.tenancy.getTeam({ prn }));
  // The PRN comes from the URL: IAM answers a wrong [org] with prn-mismatch, which is invalid-input
  // (tenancy.rs:331-333). Spec § 5.2 makes a mismatched URL a 404 (ruling T18.a).
  if (!got.ok) return got.error.presentation === 'invalid-input' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const team = got.value.team;
  if (team === undefined || !sameNode(team.orgPrn, 'organization', orgId)) return { kind: 'not-found' };

  const [projects, canCreateProject, members] = await Promise.all([
    callIam(() => deps.tenancy.listProjects({ teamPrn: prn, limit: PAGE_SIZE, offset: BigInt(params.offset) })),
    deps.mayI('CreateProject', prn),
    loadMembers(deps, prn, params.membersOffset),
  ]);
  const projectList: IamResult<ProjectList> = projects.ok
    ? {
        ok: true,
        value: {
          rows: projects.value.projects.map((project): ProjectRow => {
            const ref = parseTenancyPrn(project.prn);
            return { prn: project.prn, projectId: ref?.kind === 'project' ? ref.id.toLowerCase() : null, slug: project.slug, name: project.name };
          }),
          offset: params.offset,
          nextOffset: nextOffset(params.offset, projects.value.projects.length),
        },
      }
    : projects;

  return { kind: 'ok', orgId, teamId, teamPrn: prn, team: { name: team.name, slug: team.slug }, projects: projectList, canCreateProject, members };
}
```

`ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The create-project command (spec § 5.3). It takes NO mayI: IAM decides (spec § 6.3).
import 'server-only';
import { z } from 'zod';
import { callIam } from '../../../../../../lib/errors';
import { toActionResult, type ActionResult } from '../../../../../../lib/form';
import type { IamClients } from '../../../../../../lib/iam';

const text = z.string().trim().min(1).max(200);

export const createProjectForm = z.object({ teamPrn: z.string().min(1).max(512), slug: text, name: text });
export type CreateProjectInput = z.infer<typeof createProjectForm>;

export async function createProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'createProject'> }, input: CreateProjectInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.createProject({ teamPrn: input.teamPrn, slug: input.slug, name: input.name })));
}
```

- [ ] **Step 8: Write the project page loader**

`ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/load.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The loader of /iam/orgs/[org]/teams/[team]/projects/[project] (spec § 5.2). A project PRN holds
// the org and the project, not the team, so [team] is checked through GetProject's team_prn.
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import type { MayI } from '../../../../../../../../lib/authorize';
import { callIam } from '../../../../../../../../lib/errors';
import type { IamClients } from '../../../../../../../../lib/iam';
import { isUuid, projectPrn } from '../../../../../../../../lib/prn';
import { loadMembers, type MembersData } from '../../../../../members';
import { sameNode } from '../../../../../node-ref';

export type ProjectPageData =
  | { readonly kind: 'not-found' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | {
      readonly kind: 'ok';
      readonly orgId: string;
      readonly teamId: string;
      readonly projectId: string;
      readonly projectPrn: string;
      readonly project: { readonly name: string; readonly slug: string };
      readonly members: MembersData;
    };
export type ProjectPageDeps = {
  readonly tenancy: Pick<IamClients['tenancy'], 'getProject' | 'listMemberships'>;
  readonly mayI: MayI;
};

export async function loadProjectPage(
  deps: ProjectPageDeps,
  params: { readonly org: string; readonly team: string; readonly project: string; readonly membersOffset: number },
): Promise<ProjectPageData> {
  if (!isUuid(params.org) || !isUuid(params.team) || !isUuid(params.project)) return { kind: 'not-found' };
  const orgId = params.org.toLowerCase();
  const teamId = params.team.toLowerCase();
  const projectId = params.project.toLowerCase();
  const prn = projectPrn(orgId, projectId);

  const got = await callIam(() => deps.tenancy.getProject({ prn }));
  // The PRN comes from the URL: IAM answers a wrong [org] with prn-mismatch, which is invalid-input
  // (tenancy.rs:480-482). Spec § 5.2 makes a mismatched URL a 404 (ruling T18.a).
  if (!got.ok) return got.error.presentation === 'invalid-input' ? { kind: 'not-found' } : { kind: 'error', error: got.error };
  const project = got.value.project;
  if (project === undefined || !sameNode(project.teamPrn, 'team', teamId) || !sameNode(project.orgPrn, 'organization', orgId)) return { kind: 'not-found' };

  const members = await loadMembers(deps, prn, params.membersOffset);
  return { kind: 'ok', orgId, teamId, projectId, projectPrn: prn, project: { name: project.name, slug: project.slug }, members };
}
```

- [ ] **Step 9: Run the test and see it pass**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/integration/team-project-pages.test.ts
```
Expected: PASS, 11 tests (`loadTeamPage` 5, `loadProjectPage` 4, `createProject` 2).

- [ ] **Step 10: Extend the structure test first, and see it fail**

In `ts/apps/iam-console/tests/unit/actions-structure.test.ts`, replace the `EXPECTED` constant with:
```ts
const EXPECTED: Readonly<Record<string, readonly string[]>> = {
  '(console)/orgs/actions.ts': ['attachMembershipAction', 'createOrganizationAction', 'detachMembershipAction'],
  '(console)/orgs/[org]/actions.ts': ['createTeamAction'],
  '(console)/orgs/[org]/teams/[team]/actions.ts': ['createProjectAction'],
};
```
Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/actions-structure.test.ts
```
Expected: FAIL. The `[team]` file is missing.

- [ ] **Step 11: Write the create-project action**

`ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/actions.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
'use server';

// See ../../../actions.ts for the rules every action here follows. tests/unit/actions-structure.test.ts
// holds the iamClients() rule.
import { revalidatePath } from 'next/cache';
import type { ActionState } from '../../../../../../lib/errors';
import { formFields, invalidFormInput } from '../../../../../../lib/form';
import { iamClients } from '../../../../../../lib/iam';
import { createProject, createProjectForm } from './commands';

export async function createProjectAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClients();
  const parsed = createProjectForm.safeParse(formFields(form, ['teamPrn', 'slug', 'name']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await createProject({ tenancy: clients.tenancy }, parsed.data);
  if (result.ok) revalidatePath('/orgs', 'layout');
  return result;
}
```
Run the structure test again. Expected: PASS.

- [ ] **Step 12: Write the team page**

`ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/page.tsx`:
```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /iam/orgs/[org]/teams/[team] (spec § 5.2). The breadcrumb does not name the organization: a user
// with a team role only can have no access to GetOrganization, and this page must not need it.
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { CreateForm } from '../../../../../_components/create-form';
import { PageError } from '../../../../../_components/page-error';
import { Pager } from '../../../../../_components/pager';
import { SectionError } from '../../../../../_components/section-error';
import { mayI } from '../../../../../../lib/authorize';
import { iamClients } from '../../../../../../lib/iam';
import { parseOffset } from '../../../../../../lib/paging';
import { isUuid } from '../../../../../../lib/prn';
import { MembersSection } from '../../../members-section';
import { createProjectAction } from './actions';
import { loadTeamPage, type ProjectList } from './load';

type Props = {
  params: Promise<{ org: string; team: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};

function ProjectTable({ base, list, membersOffset }: { readonly base: string; readonly list: ProjectList; readonly membersOffset: number }): ReactElement {
  if (list.rows.length === 0 && list.offset === 0) return <EmptyState title="No projects yet" />;
  return (
    <>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Slug</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {list.rows.map((row) => (
            <TableRow key={row.prn}>
              <TableCell>
                {row.projectId === null ? (
                  row.name
                ) : (
                  <ZoneLink href={`${base}/projects/${row.projectId}`} className="hover:underline">
                    {row.name}
                  </ZoneLink>
                )}
              </TableCell>
              <TableCell>{row.slug}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <Pager label="Project pages" path={base} param="offset" offset={list.offset} nextOffset={list.nextOffset} keep={{ moffset: membersOffset }} />
    </>
  );
}

export default async function TeamPage({ params, searchParams }: Props): Promise<ReactElement> {
  const [{ org, team }, query] = await Promise.all([params, searchParams]);
  if (!isUuid(org) || !isUuid(team)) notFound();
  // Each list's pager keeps the other list's offset in its links (pageHref, lib/paging.ts).
  const offset = parseOffset(query.offset);
  const membersOffset = parseOffset(query.moffset);
  const [clients, may] = await Promise.all([iamClients(), mayI()]);
  const data = await loadTeamPage({ tenancy: clients.tenancy, mayI: may }, { org, team, offset, membersOffset });
  if (data.kind === 'not-found') notFound();
  if (data.kind === 'error') return <PageError error={data.error} />;
  const base = `/iam/orgs/${data.orgId}/teams/${data.teamId}`;

  return (
    <div className="flex flex-col gap-8 p-6">
      <Breadcrumbs items={[{ label: 'Organizations', href: '/iam/orgs' }, { label: 'Organization', href: `/iam/orgs/${data.orgId}` }, { label: data.team.name }]} />
      <div>
        <h1 className="text-2xl font-semibold">{data.team.name}</h1>
        <p className="text-muted-foreground text-sm">{data.team.slug}</p>
      </div>
      <section aria-labelledby="projects-heading" className="flex flex-col gap-3">
        <h2 id="projects-heading" className="text-lg font-semibold">
          Projects
        </h2>
        {data.projects.ok ? <ProjectTable base={base} list={data.projects.value} membersOffset={membersOffset} /> : <SectionError error={data.projects.error} />}
        {data.canCreateProject ? <CreateForm testId="create-project" title="Create project" submitLabel="Create" action={createProjectAction} hidden={{ teamPrn: data.teamPrn }} /> : null}
      </section>
      <MembersSection nodePrn={data.teamPrn} path={base} data={data.members} keep={{ offset }} />
    </div>
  );
}
```

- [ ] **Step 13: Write the project page**

`ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/page.tsx`:
```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /iam/orgs/[org]/teams/[team]/projects/[project] (spec § 5.2). Mutations: attach and detach only.
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs } from '@paigasus/app-shell';
import { PageError } from '../../../../../../../_components/page-error';
import { mayI } from '../../../../../../../../lib/authorize';
import { iamClients } from '../../../../../../../../lib/iam';
import { parseOffset } from '../../../../../../../../lib/paging';
import { isUuid } from '../../../../../../../../lib/prn';
import { MembersSection } from '../../../../../members-section';
import { loadProjectPage } from './load';

type Props = {
  params: Promise<{ org: string; team: string; project: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};

export default async function ProjectPage({ params, searchParams }: Props): Promise<ReactElement> {
  const [{ org, team, project }, query] = await Promise.all([params, searchParams]);
  if (!isUuid(org) || !isUuid(team) || !isUuid(project)) notFound();
  const [clients, may] = await Promise.all([iamClients(), mayI()]);
  const data = await loadProjectPage({ tenancy: clients.tenancy, mayI: may }, { org, team, project, membersOffset: parseOffset(query.moffset) });
  if (data.kind === 'not-found') notFound();
  if (data.kind === 'error') return <PageError error={data.error} />;
  const teamPath = `/iam/orgs/${data.orgId}/teams/${data.teamId}`;

  return (
    <div className="flex flex-col gap-8 p-6">
      <Breadcrumbs
        items={[
          { label: 'Organizations', href: '/iam/orgs' },
          { label: 'Organization', href: `/iam/orgs/${data.orgId}` },
          { label: 'Team', href: teamPath },
          { label: data.project.name },
        ]}
      />
      <div>
        <h1 className="text-2xl font-semibold">{data.project.name}</h1>
        <p className="text-muted-foreground text-sm">{data.project.slug}</p>
      </div>
      <MembersSection nodePrn={data.projectPrn} path={`${teamPath}/projects/${data.projectId}`} data={data.members} />
    </div>
  );
}
```

- [ ] **Step 14: Run the checks**

First format every file of this task (the Global Constraints). Then run the checks:
```bash
pnpm -C ts exec prettier --write \
  'apps/iam-console/app/(console)/orgs/node-ref.ts' \
  'apps/iam-console/app/(console)/orgs/[org]/teams/[team]/load.ts' \
  'apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts' \
  'apps/iam-console/app/(console)/orgs/[org]/teams/[team]/actions.ts' \
  'apps/iam-console/app/(console)/orgs/[org]/teams/[team]/page.tsx' \
  'apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/load.ts' \
  'apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/page.tsx' \
  apps/iam-console/tests/unit/node-ref.test.ts apps/iam-console/tests/integration/team-project-pages.test.ts \
  apps/iam-console/tests/unit/actions-structure.test.ts
pnpm --dir ts/apps/iam-console exec vitest run tests/unit tests/integration
moon run iam-console-ts:typecheck ts:lint ts:fmt --force
moon run iam-console-ts:build
```
Expected: all pass. The build's route list shows `ƒ /orgs/[org]/teams/[team]` and `ƒ /orgs/[org]/teams/[team]/projects/[project]`.

- [ ] **Step 15: Commit**

```bash
git add 'ts/apps/iam-console/app/(console)/orgs/node-ref.ts' \
  'ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/load.ts' \
  'ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/commands.ts' \
  'ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/actions.ts' \
  'ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/page.tsx' \
  'ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/load.ts' \
  'ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/projects/[project]/page.tsx' \
  ts/apps/iam-console/tests/unit/node-ref.test.ts ts/apps/iam-console/tests/integration/team-project-pages.test.ts \
  ts/apps/iam-console/tests/unit/actions-structure.test.ts
git commit -F - <<'EOF'
feat(ts): add the team and project pages to the iam console (SMA-511)

The team page reads the team, its projects and its members, and offers
create project. The project page reads the project and its members. Both
compare IAM's answer with the URL: a team of another organization, or a
project of another team or organization, is a 404. IAM itself answers a
wrong organization segment with prn-mismatch (invalid input), and both
loaders map that answer to a 404 too. A segment that is not a UUID is a
404 with no IAM call.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 19: `/iam/audit` — capability gating, the degraded view, and cursor paging

**Files:**
- Create: `ts/apps/iam-console/app/(console)/audit/load.ts`
- Create: `ts/apps/iam-console/app/(console)/audit/page.tsx`
- Test: `ts/apps/iam-console/tests/unit/audit-gate.test.ts`
- Test: `ts/apps/iam-console/tests/integration/audit-page.test.ts`

**Interfaces:**
- Consumes: `discovery` (`lib/discovery.ts`); `sessionToken`, `iamClients`, `IamClients` (`lib/iam.ts`); `callIam`, `IamResult` (`lib/errors.ts`); `parseCursor` (`lib/paging.ts`); `PageError`, `PRESENTATION_COPY`; `ServiceState` (`@paigasus/discovery/types`); `capabilityOutcome` (`@paigasus/discovery/client`, `ts/packages/paigasus-discovery/src/core/outcome.ts:25`, returns `'hidden' | 'shown' | 'degraded'`); `ErrorState`, `Table*` (`@paigasus/ui`); `Breadcrumbs`, `ZoneLink` (`@paigasus/app-shell`).
- Produces: `AuditGate`, `auditGate`, `AUDIT_PAGE_SIZE`, `AuditRow`, `AuditPageData`, `loadAuditPage` (`app/(console)/audit/load.ts`).

Spec § 6.6: absent, or available without `iam.audit` → `notFound()`. Degraded → the degraded `ErrorState`, not a 404. Only an available state WITH `iam.audit` reaches `ListAuditEntries`. The page does NOT ask `mayI('ListAuditLog', …)`: a navigation is a user action, so IAM answers it (spec § 6.3). Only the nav entry is hidden by `mayI` (Task 15).

- [ ] **Step 1: Write the failing gate test**

`ts/apps/iam-console/tests/unit/audit-gate.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
import type { ServiceState } from '@paigasus/discovery/types';
import { describe, expect, it } from 'vitest';
import { auditGate } from '../../app/(console)/audit/load';

const descriptor = (capabilities: string[]) => ({ service: 'iam', version: '1.0.0', capabilities });

describe('auditGate (spec § 6.6)', () => {
  it.each<[string, ServiceState, ReturnType<typeof auditGate>]>([
    ['absent', { state: 'absent', service: 'iam' }, 'not-found'],
    ['available without iam.audit', { state: 'available', service: 'iam', descriptor: descriptor(['iam.authz.cedar']), capabilities: ['iam.authz.cedar'] }, 'not-found'],
    ['available with iam.audit', { state: 'available', service: 'iam', descriptor: descriptor(['iam.audit']), capabilities: ['iam.audit'] }, 'available'],
    ['degraded with the last known capability', { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: descriptor(['iam.audit']), capabilities: ['iam.audit'] }, 'degraded'],
    ['degraded with no descriptor', { state: 'degraded', service: 'iam', reason: 'network', descriptor: null, capabilities: [] }, 'degraded'],
  ])('%s', (_label, state, gate) => {
    expect(auditGate(state)).toBe(gate);
  });
});
```

- [ ] **Step 2: Write the failing loader test**

`ts/apps/iam-console/tests/integration/audit-page.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// ListAuditEntries (spec § 5.2) through the fake IAM: cursor paging with next_cursor, and the 403 of
// a principal that is not root (application/audit.rs:36-38) as a page error.
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { AUDIT_PAGE_SIZE, loadAuditPage } from '../../app/(console)/audit/load';
import { denial, startFakeIam, type FakeIam } from '../support/fake-iam';
import { IDS, callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const entry = {
  id: 'audit-1',
  occurredAt: { seconds: 1_788_000_000n, nanos: 250_000_000 },
  actorPrn: IDS.principalPrn,
  action: 'CreateTeam',
  resourcePrn: 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a',
  outcome: 'allow',
  determiningPolicies: [],
  detailJson: '{}',
  correlationId: 'corr-audit-1',
};

describe('loadAuditPage', () => {
  it('maps the entries, forwards the cursor, and returns the next cursor', async () => {
    iam.setHandlers({ 'audit.listAuditEntries': () => ({ entries: [entry], nextCursor: 'cursor-2' }) });
    const calls = callsSince(iam);

    const data = await loadAuditPage({ audit: clientsFor(iam).audit }, { cursor: 'cursor-1' });

    expect(data).toEqual({
      ok: true,
      value: {
        cursor: 'cursor-1',
        nextCursor: 'cursor-2',
        rows: [
          {
            id: 'audit-1',
            occurredAt: new Date(1_788_000_000_250).toISOString(),
            actorPrn: IDS.principalPrn,
            action: 'CreateTeam',
            resourcePrn: entry.resourcePrn,
            outcome: 'allow',
            correlationId: 'corr-audit-1',
          },
        ],
      },
    });
    expect(calls('audit.listAuditEntries')[0]?.request).toMatchObject({ cursor: 'cursor-1', limit: AUDIT_PAGE_SIZE });
  });

  it('reads an empty next_cursor as the last page, and an absent timestamp as null', async () => {
    iam.setHandlers({ 'audit.listAuditEntries': () => ({ entries: [{ ...entry, occurredAt: undefined }], nextCursor: '' }) });

    const data = await loadAuditPage({ audit: clientsFor(iam).audit }, { cursor: '' });

    if (!data.ok) throw new Error('expected entries');
    expect(data.value.nextCursor).toBeNull();
    expect(data.value.rows[0]?.occurredAt).toBeNull();
  });

  it('returns a denial as a page error with the correlation id', async () => {
    iam.setHandlers({
      'audit.listAuditEntries': () => {
        throw denial({ correlationId: 'corr-audit-403' });
      },
    });

    const data = await loadAuditPage({ audit: clientsFor(iam).audit }, { cursor: '' });

    if (data.ok) throw new Error('expected a denial');
    expect(data.error.presentation).toBe('forbidden');
    expect(data.error.correlationId).toBe('corr-audit-403');
  });
});
```

- [ ] **Step 3: Run both and see them fail**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/audit-gate.test.ts tests/integration/audit-page.test.ts
```
Expected: FAIL. The import `../../app/(console)/audit/load` does not resolve.

- [ ] **Step 4: Write `app/(console)/audit/load.ts`**

`ts/apps/iam-console/app/(console)/audit/load.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// /iam/audit (spec § 6.6, AC 4). The screen exists only when IAM reports `iam.audit`. The gate is
// a pure function of the ServiceState, so a unit test covers every branch.
import 'server-only';
import { capabilityOutcome } from '@paigasus/discovery/client';
import type { ServiceState } from '@paigasus/discovery/types';
import { callIam, type IamResult } from '../../../lib/errors';
import type { IamClients } from '../../../lib/iam';

export type AuditGate = 'not-found' | 'degraded' | 'available';

/**
 * The branch table is @paigasus/discovery's capabilityOutcome: there is one copy. This maps its
 * answer to the page. Absent or available-without-the-capability is `hidden`, which is a 404.
 * Degraded is NOT a 404: the feature may exist.
 */
const GATE = { hidden: 'not-found', shown: 'available', degraded: 'degraded' } as const;

export function auditGate(state: ServiceState): AuditGate {
  return GATE[capabilityOutcome(state, 'iam.audit')];
}

export const AUDIT_PAGE_SIZE = 50;

export type AuditRow = {
  readonly id: string;
  /** ISO 8601, or null when IAM sent no timestamp. */
  readonly occurredAt: string | null;
  readonly actorPrn: string;
  readonly action: string;
  readonly resourcePrn: string;
  readonly outcome: string;
  readonly correlationId: string;
};

export type AuditPageData = IamResult<{ readonly rows: readonly AuditRow[]; readonly cursor: string; readonly nextCursor: string | null }>;

export async function loadAuditPage(deps: { readonly audit: Pick<IamClients['audit'], 'listAuditEntries'> }, params: { readonly cursor: string }): Promise<AuditPageData> {
  const result = await callIam(() => deps.audit.listAuditEntries({ cursor: params.cursor, limit: AUDIT_PAGE_SIZE }));
  if (!result.ok) return result;
  const rows = result.value.entries.map(
    (entry): AuditRow => ({
      id: entry.id,
      occurredAt: entry.occurredAt === undefined ? null : new Date(Number(entry.occurredAt.seconds) * 1000 + Math.floor(entry.occurredAt.nanos / 1_000_000)).toISOString(),
      actorPrn: entry.actorPrn,
      action: entry.action,
      resourcePrn: entry.resourcePrn,
      outcome: entry.outcome,
      correlationId: entry.correlationId,
    }),
  );
  return { ok: true, value: { rows, cursor: params.cursor, nextCursor: result.value.nextCursor === '' ? null : result.value.nextCursor } };
}
```

- [ ] **Step 5: Run both and see them pass**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/audit-gate.test.ts tests/integration/audit-page.test.ts
```
Expected: PASS, 8 tests.

- [ ] **Step 6: Write the page**

`ts/apps/iam-console/app/(console)/audit/page.tsx`:
```tsx
// SPDX-License-Identifier: Apache-2.0
//
// /iam/audit (spec § 6.6, AC 4). The page asks discovery, not mayI(): the Audit nav entry is the
// affordance and mayI() hides it (Task 15); a typed URL is a user action, and IAM answers it.
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs, ZoneLink } from '@paigasus/app-shell';
import { EmptyState, ErrorState, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { PRESENTATION_COPY } from '../../_components/error-copy';
import { PageError } from '../../_components/page-error';
import { discovery } from '../../../lib/discovery';
import { iamClients, sessionToken } from '../../../lib/iam';
import { parseCursor } from '../../../lib/paging';
import { auditGate, loadAuditPage } from './load';

type Props = { searchParams: Promise<Record<string, string | string[] | undefined>> };

export default async function AuditPage({ searchParams }: Props): Promise<ReactElement> {
  const [query, token] = await Promise.all([searchParams, sessionToken()]);
  const gate = auditGate(await discovery().getServiceState('iam', token));
  if (gate === 'not-found') notFound();
  if (gate === 'degraded') {
    return (
      <div className="flex flex-col gap-6 p-6" data-testid="audit-degraded">
        <Breadcrumbs items={[{ label: 'Audit' }]} />
        <ErrorState title={PRESENTATION_COPY.degraded.title} description={PRESENTATION_COPY.degraded.body} />
      </div>
    );
  }

  const clients = await iamClients();
  const data = await loadAuditPage({ audit: clients.audit }, { cursor: parseCursor(query.cursor) });
  if (!data.ok) return <PageError error={data.error} />;
  const { rows, cursor, nextCursor } = data.value;

  return (
    <div className="flex flex-col gap-6 p-6">
      <Breadcrumbs items={[{ label: 'Audit' }]} />
      <h1 className="text-2xl font-semibold">Audit log</h1>
      {rows.length === 0 ? (
        <EmptyState title="No audit entries" />
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Time</TableHead>
              <TableHead>Actor</TableHead>
              <TableHead>Action</TableHead>
              <TableHead>Resource</TableHead>
              <TableHead>Outcome</TableHead>
              <TableHead>Correlation id</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((row) => (
              <TableRow key={row.id}>
                <TableCell>{row.occurredAt ?? '—'}</TableCell>
                <TableCell>
                  <code className="text-xs">{row.actorPrn}</code>
                </TableCell>
                <TableCell>{row.action}</TableCell>
                <TableCell>
                  <code className="text-xs">{row.resourcePrn}</code>
                </TableCell>
                <TableCell>{row.outcome}</TableCell>
                <TableCell>
                  <code className="text-xs">{row.correlationId}</code>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      <nav aria-label="Audit pages" className="flex gap-4 text-sm">
        {cursor === '' ? null : (
          <ZoneLink href="/iam/audit" className="hover:underline">
            First page
          </ZoneLink>
        )}
        {nextCursor === null ? null : (
          <ZoneLink href={`/iam/audit?cursor=${encodeURIComponent(nextCursor)}`} className="hover:underline">
            Next
          </ZoneLink>
        )}
      </nav>
    </div>
  );
}
```

- [ ] **Step 7: Run the checks**

First format every file of this task (the Global Constraints). Then run the checks:
```bash
pnpm -C ts exec prettier --write \
  'apps/iam-console/app/(console)/audit/load.ts' 'apps/iam-console/app/(console)/audit/page.tsx' \
  apps/iam-console/tests/unit/audit-gate.test.ts apps/iam-console/tests/integration/audit-page.test.ts
pnpm --dir ts/apps/iam-console exec vitest run tests/unit tests/integration
moon run iam-console-ts:typecheck ts:lint ts:fmt --force
moon run iam-console-ts:build
```
Expected: all pass. The build's route list shows `ƒ /audit`.

- [ ] **Step 8: Commit**

```bash
git add 'ts/apps/iam-console/app/(console)/audit/load.ts' 'ts/apps/iam-console/app/(console)/audit/page.tsx' \
  ts/apps/iam-console/tests/unit/audit-gate.test.ts ts/apps/iam-console/tests/integration/audit-page.test.ts
git commit -F - <<'EOF'
feat(ts): add the capability-gated audit screen to the iam console (SMA-511)

The page exists only when discovery reports iam.audit. Absent, or available
without the capability, is a 404. A degraded IAM shows the degraded view,
not a 404, and never reaches ListAuditEntries. The list pages with IAM's
next_cursor.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 20: The SDK client-boundary fixture (SMA-508 AC 1, spec § 7.7)

**Files:**
- Create: `ts/apps/iam-console/tests/fixtures/client-imports-sdk/next.config.ts`
- Create: `ts/apps/iam-console/tests/fixtures/client-imports-sdk/tsconfig.json`
- Create: `ts/apps/iam-console/tests/fixtures/client-imports-sdk/app/layout.tsx`
- Create: `ts/apps/iam-console/tests/fixtures/client-imports-sdk/app/page.tsx`
- Create: `ts/apps/iam-console/tests/fixtures/client-imports-sdk/app/sdk-user.tsx`
- Create: the same five files under `ts/apps/iam-console/tests/fixtures/server-imports-sdk/`
- Create: `ts/apps/iam-console/.gitignore` (no earlier task creates it)
- Modify: `ts/apps/iam-console/tsconfig.json` (`exclude`)
- Modify: `ts/apps/iam-console/moon.yml` (`test` inputs: two negations)
- Modify: `moon.yml` (root, `next-public-free` inputs)
- Modify: `ci/affected-graph/ci_targets.py` (`SELF_TASK_EXPECTED_GLOBS["next-public-free"]`)
- Test: `ts/apps/iam-console/tests/build/client-boundary.test.ts`

**Interfaces:**
- Consumes: `createNextConfig` (`@paigasus/next-config`); `createIamClient`, `TenancyService` (`@paigasus/sdk/iam`).
- Produces: the vitest file `tests/build/client-boundary.test.ts`, which runs inside `iam-console-ts:test`.

Facts this task relies on. `server-only`'s exports map is `{ "react-server": "./empty.js", "default": "./index.js" }`, and `index.js` throws. So a client component that takes a VALUE import of a guarded SDK entry fails `next build`. No ESLint rule can do this job: rules select files by path, not by the `'use client'` directive. The fixture builds through the real factory, so the build uses `SOURCE_ONLY_PACKAGES` exactly as the app does. `createNextConfig` pins `outputFileTracingRoot` to `ts/` (as in the app-shell e2e fixture), which is five levels up from a fixture directory.

- [ ] **Step 1: Write the failing test**

`ts/apps/iam-console/tests/build/client-boundary.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-508 AC 1, moved here (spec § 7.7). A 'use client' component that imports @paigasus/sdk/iam
// must FAIL `next build`, with the server-only error. The POSITIVE CONTROL builds the same code
// from a server component and must succeed; without it, a build that failed for any other reason
// (a missing module, a bad config) would pass the negative case.
//
// The two fixtures are two directories because a Next build compiles every file of its project.
// The last case pins that their sdk-user.tsx files differ ONLY in the directive, so the control
// stays "the same fixture with the import in a server component".
//
// The negative case needs exit code 1 exactly: `null` (a spawn error, or the kill at the timeout)
// is not a build failure. The message regex matches Next 16.3.4's own error, "'server-only' cannot
// be imported from a Client Component module. It should only be used from a Server Component."
// (next/dist/build/webpack-config.js:1183; the swc binary carries the same text for Turbopack), and
// server-only's own throw, "This module cannot be imported from a Client Component module.". A
// looser /client component/i also matches Turbopack's layer labels ("Client Component Browser"),
// so a build that fails for another client-side reason would pass it.
import { spawnSync } from 'node:child_process';
import { readFileSync, rmSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const APP_DIR = fileURLToPath(new URL('../..', import.meta.url));
const FIXTURES_DIR = path.join(APP_DIR, 'tests', 'fixtures');
const BUILD_TIMEOUT_MS = 300_000;

type Fixture = 'client-imports-sdk' | 'server-imports-sdk';

function buildEnv(): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const [key, value] of Object.entries(process.env)) {
    // vitest sets NODE_ENV=test; `next build` must run as production. The fixture reads no config.
    if (key === 'NODE_ENV' || key.startsWith('PAIGASUS_') || key.startsWith('__NEXT')) continue;
    env[key] = value;
  }
  return { ...env, NODE_ENV: 'production', NEXT_TELEMETRY_DISABLED: '1' };
}

function build(fixture: Fixture): { status: number | null; output: string } {
  rmSync(path.join(FIXTURES_DIR, fixture, '.next'), { recursive: true, force: true });
  // `pnpm exec next build <dir>` from the app directory: the command app-shell's e2e fixture uses.
  const result = spawnSync('pnpm', ['exec', 'next', 'build', `tests/fixtures/${fixture}`], { cwd: APP_DIR, env: buildEnv(), encoding: 'utf8', timeout: BUILD_TIMEOUT_MS });
  return { status: result.status, output: `${result.stdout}\n${result.stderr}` };
}

describe('a client component cannot import @paigasus/sdk (spec § 7.7)', () => {
  it('fails `next build` with the server-only error when a client component imports the SDK', { timeout: BUILD_TIMEOUT_MS + 10_000 }, () => {
    const { status, output } = build('client-imports-sdk');
    expect(status, output).toBe(1);
    expect(output).toMatch(/server-only/);
    expect(output).toMatch(/cannot be imported from a Client Component module/);
  });

  it('builds the same code when a server component imports the SDK (positive control)', { timeout: BUILD_TIMEOUT_MS + 10_000 }, () => {
    const { status, output } = build('server-imports-sdk');
    expect(status, output).toBe(0);
  });

  it('keeps the two fixtures identical except for the directive', () => {
    const read = (fixture: Fixture, file: string) => readFileSync(path.join(FIXTURES_DIR, fixture, file), 'utf8');
    expect(read('client-imports-sdk', 'app/sdk-user.tsx').replace("'use client';\n\n", '')).toBe(read('server-imports-sdk', 'app/sdk-user.tsx'));
    for (const file of ['next.config.ts', 'tsconfig.json', 'app/layout.tsx', 'app/page.tsx']) {
      expect(read('client-imports-sdk', file), file).toBe(read('server-imports-sdk', file));
    }
  });
});
```

- [ ] **Step 2: Run it and see it fail**

Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/build/client-boundary.test.ts
```
Expected: FAIL in all three cases. `next build` reports that the project directory does not exist, and `readFileSync` throws `ENOENT`.

- [ ] **Step 3: Write the negative fixture**

`ts/apps/iam-console/tests/fixtures/client-imports-sdk/next.config.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// A minimal Next app for tests/build/client-boundary.test.ts. It builds through the REAL factory,
// so the SDK is transpiled exactly as in the iam-console. ts/ is five levels up.
import path from 'node:path';
import { createNextConfig } from '@paigasus/next-config';

export default createNextConfig({ zone: 'iam', basePath: '/iam', outputFileTracingRoot: path.resolve(import.meta.dirname, '../../../../..') });
```

`ts/apps/iam-console/tests/fixtures/client-imports-sdk/tsconfig.json`:
```json
{
  "extends": "@paigasus/next-config/tsconfig-app",
  "compilerOptions": {
    "paths": { "@/*": ["./*"] }
  },
  "include": ["next-env.d.ts", "**/*.ts", "**/*.tsx", ".next/types/**/*.ts"],
  "exclude": ["node_modules"]
}
```

`ts/apps/iam-console/tests/fixtures/client-imports-sdk/app/layout.tsx`:
```tsx
// SPDX-License-Identifier: Apache-2.0
import type { ReactElement, ReactNode } from 'react';

export default function RootLayout({ children }: { children: ReactNode }): ReactElement {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
```

`ts/apps/iam-console/tests/fixtures/client-imports-sdk/app/page.tsx`:
```tsx
// SPDX-License-Identifier: Apache-2.0
import type { ReactElement } from 'react';
import { SdkUser } from './sdk-user';

export default function Page(): ReactElement {
  return <SdkUser />;
}
```

`ts/apps/iam-console/tests/fixtures/client-imports-sdk/app/sdk-user.tsx`:
```tsx
// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement } from 'react';
import { TenancyService, createIamClient } from '@paigasus/sdk/iam';

// A VALUE import of a guarded SDK entry. In a client component this must fail the build.
export function SdkUser(): ReactElement {
  return <p>{`${typeof createIamClient} ${TenancyService.typeName}`}</p>;
}
```

- [ ] **Step 4: Write the positive-control fixture**

Create `ts/apps/iam-console/tests/fixtures/server-imports-sdk/` with the SAME `next.config.ts`, `tsconfig.json`, `app/layout.tsx` and `app/page.tsx` as Step 3, byte for byte:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console/ts/apps/iam-console/tests/fixtures
mkdir -p server-imports-sdk/app
cp client-imports-sdk/next.config.ts client-imports-sdk/tsconfig.json server-imports-sdk/
cp client-imports-sdk/app/layout.tsx client-imports-sdk/app/page.tsx server-imports-sdk/app/
```
Then write `ts/apps/iam-console/tests/fixtures/server-imports-sdk/app/sdk-user.tsx` (the Step 3 file without the directive and its blank line):
```tsx
// SPDX-License-Identifier: Apache-2.0
import type { ReactElement } from 'react';
import { TenancyService, createIamClient } from '@paigasus/sdk/iam';

// A VALUE import of a guarded SDK entry. In a client component this must fail the build.
export function SdkUser(): ReactElement {
  return <p>{`${typeof createIamClient} ${TenancyService.typeName}`}</p>;
}
```

- [ ] **Step 5: Keep the generated files out of git, the app's type-check and the Moon hashes**

`ts/apps/iam-console/.gitignore` (create it; if it exists, append these lines):
```gitignore
# `next build` writes each fixture's next-env.d.ts. It is generated, so it is not tracked, and
# repo:next-env-drift (which checks only the app's tracked copy) needs no change. The fixtures'
# .next/ directories are already covered by the root .gitignore.
tests/fixtures/*/next-env.d.ts
```

In `ts/apps/iam-console/tsconfig.json`, change the `exclude` line to:
```json
  "exclude": ["node_modules", "tests/fixtures/**"]
```
Each fixture has its own `tsconfig.json`, so ESLint's project service still finds a project for every fixture file.

In `ts/apps/iam-console/moon.yml`, in the `test` task's `inputs`, add these two lines directly after `'tests/**/*'`:
```yaml
      # The fixtures' `next build` output and generated next-env.d.ts. The Moon hasher does NOT skip
      # `.next` (.moon/workspace.yml omits it on purpose), so without these, every run of this task
      # would re-key the task itself (the same measurement as app-shell's fixture, SMA-510).
      - '!tests/fixtures/**/.next/**'
      - '!tests/fixtures/*/next-env.d.ts'
```

In the root `moon.yml`, in `next-public-free`'s `inputs`, add a line directly after `'!ts/apps/*/.next/**'`:
```yaml
      # SMA-511: the iam-console's client-boundary fixtures build under ts/apps/*/tests/fixtures/*/.next,
      # which the broad 'ts/**/*' entry below also hashes. Same reason as the app-shell entry below.
      - '!ts/apps/*/tests/fixtures/*/.next/**'
```

In `ci/affected-graph/ci_targets.py`, replace the `"next-public-free"` entry of `SELF_TASK_EXPECTED_GLOBS` with (globs sorted, as `check_gate_inputs` compares them):
```python
    "next-public-free": (
        "!ts/apps/*/.next/**",
        "!ts/apps/*/tests/fixtures/*/.next/**",
        "!ts/packages/*/tests/e2e/fixture/.next/**",
        "ci/next-public/**/*",
        "ts/**/*",
    ),
```
and add one line to the comment block above that entry:
```python
    # SMA-511 added the '!ts/apps/*/tests/fixtures/*/.next/**' entry, for the iam-console's
    # client-boundary fixture builds, which run inside iam-console-ts:test.
```

- [ ] **Step 6: Run the test and see it pass**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console
pnpm --dir ts/apps/iam-console exec vitest run tests/build/client-boundary.test.ts
```
Expected: PASS, 3 tests (about one minute per build).
Then record the build-output line that the message regex matched in the test's header comment, under the two quoted texts.
If the negative case fails on `toBe(1)` or on a `toMatch`, read the printed build output. The build failed for another reason, or it did not fail. Fix the fixture. Do not loosen or delete an assertion: a build that fails for another reason proves nothing.
If the positive control fails, the fixture itself is broken (for example a missing dependency). Fix the fixture; the negative case proves nothing until the control is green.

- [ ] **Step 7: Prove the fixture files are clean for lint, format and type-check**

First format every `ts/` file that this task creates or edits (the Global Constraints). The two fixtures get the same input, so they stay byte-identical. `.gitignore` is not in the list: Prettier has no parser for it.
```bash
pnpm -C ts exec prettier --write apps/iam-console/tests/build/client-boundary.test.ts \
  apps/iam-console/tests/fixtures/client-imports-sdk/next.config.ts apps/iam-console/tests/fixtures/client-imports-sdk/tsconfig.json \
  apps/iam-console/tests/fixtures/client-imports-sdk/app/layout.tsx apps/iam-console/tests/fixtures/client-imports-sdk/app/page.tsx \
  apps/iam-console/tests/fixtures/client-imports-sdk/app/sdk-user.tsx \
  apps/iam-console/tests/fixtures/server-imports-sdk/next.config.ts apps/iam-console/tests/fixtures/server-imports-sdk/tsconfig.json \
  apps/iam-console/tests/fixtures/server-imports-sdk/app/layout.tsx apps/iam-console/tests/fixtures/server-imports-sdk/app/page.tsx \
  apps/iam-console/tests/fixtures/server-imports-sdk/app/sdk-user.tsx \
  apps/iam-console/tsconfig.json apps/iam-console/moon.yml
```

Then run:
```bash
git status --short --untracked-files=all ts/apps/iam-console/tests/fixtures
moon run ts:lint ts:fmt iam-console-ts:typecheck --force
```
Expected: `git status` prints exactly ten `??` lines, one for each source file, and nothing else (no `next-env.d.ts`, no `.next`). `--untracked-files=all` is necessary: without it, git prints one collapsed `?? ts/apps/iam-console/tests/fixtures/` line for the untracked directory. The ten lines:
```text
?? ts/apps/iam-console/tests/fixtures/client-imports-sdk/app/layout.tsx
?? ts/apps/iam-console/tests/fixtures/client-imports-sdk/app/page.tsx
?? ts/apps/iam-console/tests/fixtures/client-imports-sdk/app/sdk-user.tsx
?? ts/apps/iam-console/tests/fixtures/client-imports-sdk/next.config.ts
?? ts/apps/iam-console/tests/fixtures/client-imports-sdk/tsconfig.json
?? ts/apps/iam-console/tests/fixtures/server-imports-sdk/app/layout.tsx
?? ts/apps/iam-console/tests/fixtures/server-imports-sdk/app/page.tsx
?? ts/apps/iam-console/tests/fixtures/server-imports-sdk/app/sdk-user.tsx
?? ts/apps/iam-console/tests/fixtures/server-imports-sdk/next.config.ts
?? ts/apps/iam-console/tests/fixtures/server-imports-sdk/tsconfig.json
```
Lint, fmt and typecheck pass.

- [ ] **Step 8: Run the gates that key on the changed inputs**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:next-public-free repo:input-liveness --force
python3 ci/affected-graph/ci_targets.py
/bin/bash ci/affected-graph/run.sh --negative-control
```
Expected: both Moon tasks pass.
`python3 ci/affected-graph/ci_targets.py` (no flag) exits 0 and prints one line that starts with `PASS  ci-targets`. This is the check that compares the pin with Moon. It runs `moon query tasks`, and `check_gate_inputs` compares Moon's resolved `next-public-free` inputs with `SELF_TASK_EXPECTED_GLOBS["next-public-free"]` by exact ordered equality. So a typo in the `moon.yml` glob or in the pin fails here.
The negative control ends with `negative-control OK: harness reported red on all wrong expectations`. It runs `ci_targets.py --self-test`, which checks the table's own shape only: it builds its payload from the table and never reads Moon. It cannot find a `moon.yml`/pin mismatch. Use `/bin/bash`: bash 5.3.15 on this machine deadlocks in this script.

- [ ] **Step 9: Commit**

```bash
git add ts/apps/iam-console/tests/build/client-boundary.test.ts ts/apps/iam-console/tests/fixtures \
  ts/apps/iam-console/.gitignore ts/apps/iam-console/tsconfig.json ts/apps/iam-console/moon.yml \
  moon.yml ci/affected-graph/ci_targets.py
git commit -F - <<'EOF'
test(ts): prove a client component cannot import the sdk (SMA-511)

A minimal Next app whose 'use client' component imports @paigasus/sdk/iam
must fail next build with the server-only error. A sibling fixture with the
same code in a server component must build, so the negative case cannot
pass on an unrelated failure. A third case pins that the two fixtures differ
only in the directive. This is SMA-508 AC 1, which no ESLint rule can hold.

The fixture build output is negated in the test task's inputs and in
repo:next-public-free, and the ci_targets pin follows.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 21: The e2e harness — Playwright config, global setup, the worker-scoped stack, and the `iam-console-ts:test-e2e` task

**Files:**
- Create: `ts/apps/iam-console/playwright.config.ts`
- Create: `ts/apps/iam-console/tests/e2e/global-setup.ts`
- Create: `ts/apps/iam-console/tests/e2e/support/paths.ts`
- Create: `ts/apps/iam-console/tests/e2e/support/world.ts`
- Create: `ts/apps/iam-console/tests/e2e/support/harness.ts`
- Test: `ts/apps/iam-console/tests/e2e/harness.spec.ts`
- Modify: `ts/apps/iam-console/moon.yml` (new `test-e2e` task)
- Modify: `.github/workflows/ci.yml` (Chromium install line)

**Interfaces:**
- Consumes: `startFakeIam`, `denial`, `FakeIam`, `FakeIamHandlers` (`tests/support/fake-iam.ts`); `startFakeIdp`, `FakeIdp` (`tests/support/fake-idp.ts`); `testTls`, `TlsMaterial` (`tests/support/tls.ts`); `startTlsTerminator` (`tests/support/tls-terminator.ts`); the standalone build from `iam-console-ts:build`; `/iam/healthz` (Task 10).
- Produces: `test`, `expect`, `Harness` (`tests/e2e/support/harness.ts`); `worldHandlers`, `WorldOptions`, `Descriptor`, `DEFAULT_DESCRIPTOR`, `ALL_ACTIONS` and the world constants (`tests/e2e/support/world.ts`); `APP_DIR`, `STANDALONE_APP_DIR` (`tests/e2e/support/paths.ts`); the Moon task `iam-console-ts:test-e2e`.

Design (spec § 9.4; this plan starts the fakes in a worker-scoped fixture, not in `globalSetup`, as the contract first said):
- A PRODUCTION build runs through the standalone `server.js`, as in the app-shell e2e. The build comes from `iam-console-ts:build` (a `deps` edge), so CI builds once.
- The browser reaches the server through the in-process TLS terminator. `PAIGASUS_PUBLIC_ORIGIN` is the terminator's `https:` origin, and Next's Server Action origin check sees the forwarded `Host`.
- The server trusts the test certificate through `NODE_EXTRA_CA_CERTS`, because the fake IdP is HTTPS (`authEnvShape` requires `https:`).
- The session store is `memory`, so `PAIGASUS_ZONES` holds ONE zone (`createAuthRuntime` refuses `memory` with more than one).
- `PAIGASUS_DISCOVERY_NEGATIVE_MS=1`, `_FRESH_MS=2`, `_STALE_MS=3`. The memory cache then drops a record after 3 ms, so each request probes the fake again and a test can change the IAM state between two page loads. The lock timings keep their defaults (`probeTimeoutMs < lockWaitMs < lockTtlMs` must hold).
- Everything that a test scripts or counts runs in the WORKER process: a Playwright `globalSetup` runs in another process.

- [ ] **Step 1: Check what the harness needs from the Task 11 doubles**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console/ts/apps/iam-console
grep -n '"@playwright/test"\|"@connectrpc/connect"' package.json
grep -n "@paigasus/sdk\|/lib/\|server-only" tests/support/fake-iam.ts tests/support/fake-idp.ts tests/support/tls.ts tests/support/tls-terminator.ts
grep -n "provisioned\|introspect" tests/support/fake-iam.ts | head -20
grep -n "target" tests/support/tls-terminator.ts | head -5
```
Expected, and what to do when a line differs:
1. `@playwright/test` and `@connectrpc/connect` are listed. If one is missing: `pnpm --dir ts --filter @paigasus/iam-console add -D '<name>@catalog:'`.
2. The second grep prints NOTHING that imports a guarded `@paigasus/sdk` entry (`/iam`, `/errors`, `/chat`, the root) or a `lib/` file. Playwright loads these files WITHOUT the vitest `server-only` stub, so such an import throws at load. `@paigasus/sdk/errors/types` is unguarded and is fine. If a double imports a service descriptor from `@paigasus/sdk/iam`, change that import to `@paigasus/proto/iam` (the `apps/*/tests/support/**` path is exempt from the proto ban, spec § 7.4).
3. The fake applies its provisioning gate to `authn.introspect` BEFORE it calls a scripted `authn.introspect` handler, and it forces `roleGrants` to `[]`. If the fake instead lets a scripted handler replace the gate, add the gate to `worldHandlers` in Step 4: throw `denial({ reason: 'identity-not-provisioned' })` when `!iam.provisioned.has(ctx.token ?? '')`.
4. `startTlsTerminator`'s `target` is an origin such as `http://127.0.0.1:<port>`. If it takes a port number or `host:port`, adapt the one call in Step 5.

- [ ] **Step 2: Write the Playwright config**

`ts/apps/iam-console/playwright.config.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The e2e tier (spec § 9.4). tests/e2e/global-setup.ts checks the build; tests/e2e/support/harness.ts
// starts the fakes, the TLS terminator and the standalone server in the WORKER, because a test
// scripts the fake IAM and counts its calls, and globalSetup runs in another process.
import os from 'node:os';
import path from 'node:path';
import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e',
  testMatch: '**/*.spec.ts',
  // A committed test.only would drop the AC proofs from CI in silence.
  forbidOnly: !!process.env.CI,
  globalSetup: './tests/e2e/global-setup.ts',
  // One worker: every spec shares ONE server and ONE fake IAM, and the specs count calls.
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: 'list',
  timeout: 60_000,
  // OUTSIDE the app directory. The app directory is Tailwind's scan root, the tailwind-source guard
  // walks all of it, and the Moon `test` inputs hash tests/**: none of them may see run artifacts.
  outputDir: path.join(os.tmpdir(), 'iam-console-e2e-results'),
  use: {
    trace: 'retain-on-failure',
    // The terminator and the fake IdP use a self-signed test certificate.
    ignoreHTTPSErrors: true,
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
```

- [ ] **Step 3: Write the paths module and the global setup**

`ts/apps/iam-console/tests/e2e/support/paths.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const APP_DIR = fileURLToPath(new URL('../../..', import.meta.url));
// createNextConfig pins outputFileTracingRoot to ts/, so the entry point lands under the app's path
// relative to ts/ (the same path the `build` task asserts).
export const STANDALONE_APP_DIR = path.join(APP_DIR, '.next', 'standalone', 'apps', 'iam-console');
```

`ts/apps/iam-console/tests/e2e/global-setup.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// Runs ONCE, in the Playwright runner process, before any worker starts. It does not start servers:
// see playwright.config.ts. The build comes from `iam-console-ts:build` (the Moon task's deps).
import { cpSync, existsSync, rmSync } from 'node:fs';
import path from 'node:path';
import { APP_DIR, STANDALONE_APP_DIR } from './support/paths';

export default function globalSetup(): void {
  const serverJs = path.join(STANDALONE_APP_DIR, 'server.js');
  if (!existsSync(serverJs)) {
    throw new Error(`the standalone server is missing at ${serverJs}. Run \`moon run iam-console-ts:build\` first (iam-console-ts:test-e2e depends on it).`);
  }
  // The standalone tree has NO .next/static (measured, SMA-510). Without this copy every client
  // chunk is a 404, nothing hydrates, and no Server Action can run.
  const staticTarget = path.join(STANDALONE_APP_DIR, '.next', 'static');
  rmSync(staticTarget, { recursive: true, force: true });
  cpSync(path.join(APP_DIR, '.next', 'static'), staticTarget, { recursive: true });
  const publicDir = path.join(APP_DIR, 'public');
  if (existsSync(publicDir)) cpSync(publicDir, path.join(STANDALONE_APP_DIR, 'public'), { recursive: true });
}
```

- [ ] **Step 4: Write the scripted IAM world**

`ts/apps/iam-console/tests/e2e/support/world.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The IAM that the e2e tier talks to: one organization with a team and a project, a grant on a
// project in a second organization, and an audit entry. `worldHandlers()` always returns the FULL
// handler set, because the fake's setHandlers() REPLACES the whole map (Task 11). Every key an
// override uses must therefore exist in the default set below.
//
// One piece of state: an organization that CreateOrganization makes is in every later
// ListOrganizations answer of the same world. Each worldHandlers() call starts with none, so a
// test cannot see the organizations of an earlier test. R6 uses it to prove that the action
// refreshes the page (P5b-16).
//
// PRNs are literal strings: lib/prn.ts imports server-only, which throws under Playwright.
import { Code } from '@connectrpc/connect';
import { denial, type FakeIamHandlers } from '../../support/fake-iam';

export const PRINCIPAL_PRN = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0';
export const ORG_ID = '0190a100-0000-7000-8000-0000000000e1';
export const TEAM_ID = '0190a1b2-0000-7000-8000-0000000000e2';
export const PROJECT_ID = '0190a1c3-0000-7000-8000-0000000000e3';
export const OTHER_ORG_ID = '0190a100-0000-7000-8000-0000000000e4';
export const OTHER_TEAM_ID = '0190a1b2-0000-7000-8000-0000000000e5';
export const OTHER_PROJECT_ID = '0190a1c3-0000-7000-8000-0000000000e6';
export const NEW_ORG_ID = '0190a100-0000-7000-8000-0000000000e7';

export const ORG_PRN = `prn:pgs:iam:::organization/${ORG_ID}`;
export const TEAM_PRN = `prn:pgs:iam::${ORG_ID}:team/${TEAM_ID}`;
export const PROJECT_PRN = `prn:pgs:iam::${ORG_ID}:project/${PROJECT_ID}`;
export const OTHER_TEAM_PRN = `prn:pgs:iam::${OTHER_ORG_ID}:team/${OTHER_TEAM_ID}`;
export const OTHER_PROJECT_PRN = `prn:pgs:iam::${OTHER_ORG_ID}:project/${OTHER_PROJECT_ID}`;

export const ORG_NAME = 'Acme Research';
export const TEAM_NAME = 'Platform Team';
export const PROJECT_NAME = 'Inference Gateway';
export const OTHER_PROJECT_NAME = 'Shared Models';
export const AUDIT_ACTION = 'CreateTeam';

/** The Cedar action names the app asks IsAuthorized about (lib/authorize.ts IamAction). */
export const ALL_ACTIONS = ['ListOrganizations', 'CreateOrganization', 'CreateTeam', 'CreateProject', 'AttachMembership', 'DetachMembership', 'ListAuditLog'] as const;

export type Descriptor = { service: string; version: string; capabilities: string[] } | { status: number };
export const DEFAULT_DESCRIPTOR: Descriptor = { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar', 'iam.audit'] };

export type WorldOptions = {
  /** The actions IsAuthorized allows. Default: all of them. */
  readonly allow?: readonly string[];
  /** false: a first-time identity with no membership and no grant. Default: true. */
  readonly memberships?: boolean;
  /** What GET /v1/service-info answers. Default: DEFAULT_DESCRIPTOR. */
  readonly descriptor?: Descriptor;
  /** Replace single handlers, for example with one that throws denial(). */
  readonly overrides?: FakeIamHandlers;
};

const notFound = () => denial({ code: Code.NotFound, reason: 'not-found' });

// Named constants, not `Map.get()` results, where a response lists them: the handlers are typed per
// method (Task 11), and a repeated field must not hold `undefined`.
const ORGANIZATION = { prn: ORG_PRN, slug: 'acme', name: ORG_NAME };
const TEAM = { prn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'platform', name: TEAM_NAME };
const PROJECT = { prn: PROJECT_PRN, teamPrn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'gateway', name: PROJECT_NAME };

const ORGANIZATIONS = new Map([[ORG_PRN, ORGANIZATION]]);
const TEAMS = new Map([
  [TEAM_PRN, TEAM],
  [OTHER_TEAM_PRN, { prn: OTHER_TEAM_PRN, orgPrn: `prn:pgs:iam:::organization/${OTHER_ORG_ID}`, slug: 'shared', name: 'Shared Team' }],
]);
const PROJECTS = new Map([
  [PROJECT_PRN, PROJECT],
  [OTHER_PROJECT_PRN, { prn: OTHER_PROJECT_PRN, teamPrn: OTHER_TEAM_PRN, orgPrn: `prn:pgs:iam:::organization/${OTHER_ORG_ID}`, slug: 'models', name: OTHER_PROJECT_NAME }],
]);

export function worldHandlers(options: WorldOptions = {}): FakeIamHandlers {
  const allow = new Set<string>(options.allow ?? ALL_ACTIONS);
  const withScopes = options.memberships ?? true;
  const created: { prn: string; slug: string; name: string }[] = [];
  return {
    'authn.introspect': () => ({
      principalPrn: PRINCIPAL_PRN,
      status: 'active',
      issuer: 'fake-idp',
      subject: 'e2e-user',
      memberships: withScopes
        ? [
            { id: '0190a1d4-0000-7000-8000-0000000000f1', principalPrn: PRINCIPAL_PRN, nodePrn: ORG_PRN },
            { id: '0190a1d4-0000-7000-8000-0000000000f2', principalPrn: PRINCIPAL_PRN, nodePrn: TEAM_PRN },
          ]
        : [],
    }),
    'authz.isAuthorized': (req: { action: string }) => ({ allowed: allow.has(req.action), determiningPolicies: [], reason: '' }),
    'authz.listRoleGrants': () => ({
      grants: withScopes ? [{ id: '0190a1d4-0000-7000-8000-0000000000f3', principalPrn: PRINCIPAL_PRN, roleKey: 'project_viewer', scopePrn: OTHER_PROJECT_PRN }] : [],
    }),
    'tenancy.getOrganization': (req: { prn: string }) => {
      const organization = ORGANIZATIONS.get(req.prn);
      if (organization === undefined) throw notFound();
      return { organization };
    },
    'tenancy.getTeam': (req: { prn: string }) => {
      const team = TEAMS.get(req.prn);
      if (team === undefined) throw notFound();
      return { team };
    },
    'tenancy.getProject': (req: { prn: string }) => {
      const project = PROJECTS.get(req.prn);
      if (project === undefined) throw notFound();
      return { project };
    },
    'tenancy.listOrganizations': () => ({ organizations: [...ORGANIZATIONS.values(), ...created] }),
    'tenancy.listTeams': () => ({ teams: [TEAM] }),
    'tenancy.listProjects': () => ({ projects: [PROJECT] }),
    // `filter` is a oneof: its type includes `{ case: undefined }`, so narrow instead of annotating.
    'tenancy.listMemberships': (req) => ({
      memberships: [{ id: '0190a1d4-0000-7000-8000-0000000000f4', principalPrn: PRINCIPAL_PRN, nodePrn: req.filter.case === 'nodePrn' ? req.filter.value : '' }],
    }),
    'tenancy.createOrganization': (req: { slug: string; name: string }) => {
      const organization = { prn: `prn:pgs:iam:::organization/${NEW_ORG_ID}`, slug: req.slug, name: req.name };
      created.push(organization);
      return { organization };
    },
    'tenancy.createTeam': (req: { orgPrn: string; slug: string; name: string }) => ({ team: { prn: TEAM_PRN, orgPrn: req.orgPrn, slug: req.slug, name: req.name } }),
    'tenancy.createProject': (req: { teamPrn: string; slug: string; name: string }) => ({ project: { prn: PROJECT_PRN, teamPrn: req.teamPrn, orgPrn: ORG_PRN, slug: req.slug, name: req.name } }),
    'tenancy.attachMembership': (req: { principalPrn: string; nodePrn: string }) => ({
      membership: { id: '0190a1d4-0000-7000-8000-0000000000f5', principalPrn: req.principalPrn, nodePrn: req.nodePrn },
    }),
    'tenancy.detachMembership': () => ({}),
    'audit.listAuditEntries': () => ({
      entries: [
        {
          id: 'audit-e2e-1',
          occurredAt: { seconds: 1_788_000_000n, nanos: 0 },
          actorPrn: PRINCIPAL_PRN,
          action: AUDIT_ACTION,
          resourcePrn: ORG_PRN,
          outcome: 'allow',
          determiningPolicies: [],
          detailJson: '{}',
          correlationId: 'corr-audit-e2e',
        },
      ],
      nextCursor: '',
    }),
    ...options.overrides,
  };
}
```

- [ ] **Step 5: Write the failing harness smoke test**

`ts/apps/iam-console/tests/e2e/harness.spec.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The harness itself: the standalone server answers through the TLS terminator on an https origin.
// Every scenario in this directory depends on this, so it fails first and loudly.
import { expect, test } from './support/harness';

test('harness: /iam/healthz answers through the TLS terminator with the one configured zone', async ({ request, harness }) => {
  expect(new URL(harness.origin).protocol).toBe('https:');
  const response = await request.get(harness.url('/iam/healthz'));
  expect(response.status()).toBe(200);
  expect(await response.json()).toMatchObject({ zone: 'iam', zones: { iam: '/iam' } });
});
```

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console
moon run iam-console-ts:build
pnpm --dir ts/apps/iam-console exec playwright test tests/e2e/harness.spec.ts
```
Expected: FAIL. `./support/harness` does not resolve.

- [ ] **Step 6: Write the harness**

`ts/apps/iam-console/tests/e2e/support/harness.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The e2e stack, ONE per worker (workers: 1, so one per run; Playwright starts a new worker, and
// with it a new stack, after a failed test):
//
//   browser --https--> TLS terminator --http--> standalone server.js --h2c--> fake IAM (gRPC)
//                                                    |  \--http--> fake IAM (GET /v1/service-info)
//                                                    \--https--> fake IdP (NODE_EXTRA_CA_CERTS)
//
// The terminator forwards Host and sets X-Forwarded-Proto: https, so PAIGASUS_PUBLIC_ORIGIN is a
// real https origin, the __Host- cookies work, and Next's Server Action origin check passes.
import { spawn, type ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:net';
import path from 'node:path';
import { test as base } from '@playwright/test';
import { startFakeIam, type FakeIam } from '../../support/fake-iam';
import { startFakeIdp, type FakeIdp } from '../../support/fake-idp';
import { testTls } from '../../support/tls';
import { startTlsTerminator } from '../../support/tls-terminator';
import { STANDALONE_APP_DIR } from './paths';
import { DEFAULT_DESCRIPTOR, worldHandlers, type WorldOptions } from './world';

const READY_TIMEOUT_MS = 60_000;
const PROBE_TIMEOUT_MS = 2_000;
const STOP_TIMEOUT_MS = 5_000;
const MAX_START_ATTEMPTS = 3;

export type Harness = {
  readonly origin: string;
  readonly iam: FakeIam;
  readonly idp: FakeIdp;
  /** `origin + path`. `path` is the full path the browser sees, for example `/iam/orgs`. */
  url(path: string): string;
  /** Everything the server wrote to stdout and stderr since it started. */
  serverOutput(): string;
  /** Script the fake IAM for the current test. The auto fixture below resets it before each test. */
  useWorld(options?: WorldOptions): void;
};

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const probe = createServer();
    probe.once('error', reject);
    probe.listen(0, '127.0.0.1', () => {
      const address = probe.address();
      if (address === null || typeof address === 'string') {
        probe.close();
        reject(new Error('could not read the probe port'));
        return;
      }
      probe.close(() => resolve(address.port));
    });
  });
}

async function stop(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, 'exit').then(() => 'exited' as const);
  child.kill('SIGTERM');
  const timeout = new Promise<'timeout'>((resolve) => setTimeout(() => resolve('timeout'), STOP_TIMEOUT_MS));
  if ((await Promise.race([exited, timeout])) === 'timeout') child.kill('SIGKILL');
}

/** 'ready', or 'exited' when the process died first (a port race: the caller retries). */
async function waitForHealth(url: string, child: ChildProcess, output: () => string): Promise<'ready' | 'exited'> {
  const deadline = Date.now() + READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) return 'exited';
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(PROBE_TIMEOUT_MS) });
      if (response.ok) return 'ready';
      // A 500 here is a config parse failure at first request. It never recovers: fail now.
      if (response.status >= 500) throw new Error(`GET ${url} answered ${String(response.status)}\n--- server output ---\n${output()}`);
    } catch (error) {
      if (error instanceof Error && error.message.startsWith('GET ')) throw error;
      // Not listening yet, or the probe timed out.
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`the iam-console server did not answer ${url} within ${String(READY_TIMEOUT_MS)} ms\n--- server output ---\n${output()}`);
}

/** The parent's env minus anything that could leak into or mis-configure the server. */
function serverEnv(values: Readonly<Record<string, string>>): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (key === 'NODE_ENV' || key.startsWith('PAIGASUS_') || key.startsWith('__NEXT')) continue;
    env[key] = value;
  }
  return { ...env, ...values };
}

/** Runs every close step in order, also after one throws, so one failed step cannot leak the rest. */
async function closeInOrder(steps: readonly (() => Promise<void>)[]): Promise<void> {
  const failures: unknown[] = [];
  for (const step of steps) {
    try {
      await step();
    } catch (error) {
      failures.push(error);
    }
  }
  if (failures.length > 0) throw new AggregateError(failures, `${String(failures.length)} of ${String(steps.length)} close steps failed`);
}

async function startStack(): Promise<{ harness: Harness; close: () => Promise<void> }> {
  const tls = testTls();
  // Everything that started so far, NEWEST FIRST: the order to close it in. A start step that
  // throws closes all of it before the error goes up. waitForHealth throws on a 5xx answer and at
  // its timeout, and the worker fixture gets no `close` from a start that threw. Without this, the
  // server.js child, the terminator and both fakes stay alive after a failed start.
  const started: (() => Promise<void>)[] = [];
  try {
    const idp = await startFakeIdp({ cert: tls });
    started.unshift(() => idp.close());
    const iam = await startFakeIam({ handlers: worldHandlers() });
    started.unshift(() => iam.close());
    iam.setServiceInfo(DEFAULT_DESCRIPTOR);

    let output = '';
    const failures: string[] = [];
    for (let attempt = 1; attempt <= MAX_START_ATTEMPTS; attempt += 1) {
      const port = await freePort();
      const terminator = await startTlsTerminator({ target: `http://127.0.0.1:${String(port)}`, tls });
      const closeTerminator = (): Promise<void> => terminator.close();
      started.unshift(closeTerminator);
      output = '';
      const child = spawn(process.execPath, [path.join(STANDALONE_APP_DIR, 'server.js')], {
        cwd: STANDALONE_APP_DIR,
        stdio: ['ignore', 'pipe', 'pipe'],
        env: serverEnv({
          PORT: String(port),
          HOSTNAME: '127.0.0.1',
          NEXT_TELEMETRY_DISABLED: '1',
          NODE_EXTRA_CA_CERTS: tls.certPath,
          PAIGASUS_ZONE: 'iam',
          PAIGASUS_ZONES: JSON.stringify({ iam: '/iam' }),
          PAIGASUS_OIDC_ISSUER: idp.issuer,
          PAIGASUS_OIDC_CLIENT_ID: idp.clientId,
          PAIGASUS_OIDC_CLIENT_SECRET: idp.clientSecret,
          PAIGASUS_PUBLIC_ORIGIN: terminator.origin,
          PAIGASUS_SESSION_STORE: 'memory',
          PAIGASUS_SERVICES: JSON.stringify({ iam: iam.httpUrl }),
          PAIGASUS_IAM_GRPC_URL: iam.grpcUrl,
          PAIGASUS_DISCOVERY_NEGATIVE_MS: '1',
          PAIGASUS_DISCOVERY_FRESH_MS: '2',
          PAIGASUS_DISCOVERY_STALE_MS: '3',
        }),
      });
      const stopChild = (): Promise<void> => stop(child);
      started.unshift(stopChild);
      child.stdout?.on('data', (chunk: Buffer) => {
        output += chunk.toString('utf8');
      });
      child.stderr?.on('data', (chunk: Buffer) => {
        output += chunk.toString('utf8');
      });

      const state = await waitForHealth(`http://127.0.0.1:${String(port)}/iam/healthz`, child, () => output);
      if (state === 'ready') {
        const harness: Harness = {
          origin: terminator.origin,
          iam,
          idp,
          url: (fullPath) => `${terminator.origin}${fullPath}`,
          serverOutput: () => output,
          useWorld: (options = {}) => {
            iam.setHandlers(worldHandlers(options));
            iam.setServiceInfo(options.descriptor ?? DEFAULT_DESCRIPTOR);
          },
        };
        // The server, then the terminator in front of it, then the fake IAM and the fake IdP.
        return { harness, close: () => closeInOrder(started.splice(0)) };
      }
      failures.push(`attempt ${String(attempt)}: the server exited before it answered\n${output}`);
      // This attempt's server exited first (a port race). Remove its two entries (the newest two)
      // and close them. The fakes stay up for the next attempt.
      started.splice(0, 2);
      await closeInOrder([stopChild, closeTerminator]);
    }
    throw new Error(`the iam-console server failed to start after ${String(MAX_START_ATTEMPTS)} attempts:\n${failures.join('\n')}`);
  } catch (error) {
    // Close what started, then rethrow the START error: it tells why the stack is not up. A close
    // failure goes to stderr, because it must not hide the start error.
    await closeInOrder(started.splice(0)).catch((closeError: unknown) => {
      console.error('the e2e stack failed to start, and closing what had started failed too:', closeError);
    });
    throw error;
  }
}

export const test = base.extend<{ world: undefined }, { harness: Harness }>({
  harness: [
    // eslint-disable-next-line no-empty-pattern -- Playwright requires an object pattern as the first argument.
    async ({}, use) => {
      const { harness, close } = await startStack();
      try {
        await use(harness);
      } finally {
        await close();
      }
    },
    { scope: 'worker', timeout: 180_000 },
  ],
  // Before EVERY test: the default world and the default descriptor. A test that needs another
  // world calls harness.useWorld(...) itself, after this reset.
  world: [
    async ({ harness }, use) => {
      harness.useWorld();
      await use(undefined);
    },
    { auto: true },
  ],
});

export { expect } from '@playwright/test';
```

- [ ] **Step 7: Run the smoke test and see it pass**

Run:
```bash
pnpm --dir ts/apps/iam-console exec playwright test tests/e2e/harness.spec.ts
```
Expected: `1 passed`. If the server dies at start, the error prints its output. The most likely causes: a config key that the parse refuses (fix the value in `startStack`), or a missing Chromium (`pnpm --dir ts/apps/iam-console exec playwright install chromium`).

- [ ] **Step 8: Add the Moon task**

In `ts/apps/iam-console/moon.yml`, add this task after `test` (Task 23 writes the final file; this block is part of it):
```yaml
  test-e2e:
    # The e2e tier (spec § 9.4): Playwright against the standalone server.js behind a TLS
    # terminator, with an in-process fake IAM and fake IdP. No Docker. `script` (not `command`):
    # Moon's `command` refuses shell syntax. `set -euo pipefail` because Moon does not enable
    # errexit for `script:` blocks. The server's lifecycle belongs to tests/e2e/support/harness.ts;
    # a shell `&` here left an orphan server in the SMA-510 spike.
    script: |
      set -euo pipefail
      pnpm exec playwright test
    deps:
      # The production build. On a cache hit Moon restores `.next`, standalone tree included.
      - '~:build'
      # repo:next-env-drift writes into .next too; the same serialization `test` has.
      - 'repo:next-env-drift'
    # A NEW task name, so it inherits NOTHING: every input is listed.
    inputs:
      - '@group(sources)'
      - 'app/**/*'
      - 'lib/**/*'
      - 'proxy.ts'
      - 'tests/**/*'
      - '!tests/fixtures/**/.next/**'
      - '!tests/fixtures/*/next-env.d.ts'
      - 'package.json'
      - 'tsconfig.json'
      - 'next.config.ts'
      - 'postcss.config.mjs'
      - 'playwright.config.ts'
      - '/ts/pnpm-lock.yaml'
      - '/ts/tsconfig.base.json'
      - '/ts/packages/paigasus-next-config/src/**/*'
      - '/ts/packages/paigasus-next-config/package.json'
      - '/ts/packages/paigasus-next-config/tsconfig.app.json'
      - '/ts/packages/paigasus-ui/src/**/*'
      - '/ts/packages/paigasus-ui/package.json'
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-sdk/src/**/*'
      - '/ts/packages/paigasus-sdk/package.json'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
      - '/ts/packages/paigasus-app-shell/src/**/*'
      - '/ts/packages/paigasus-app-shell/package.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
    options:
      # A real build and a real Chromium. A cached PASS would replay a green that tested nothing.
      cache: false
```
`:test-e2e` is already in `ci.yml`'s `T=(…)` array and in the CLAUDE.md target list, so CI selects this task with no change there.

- [ ] **Step 9: Install Chromium for this app in CI**

In `.github/workflows/ci.yml`, in the step `Install Playwright Chromium (…)`, change the step name to:
```yaml
      - name: Install Playwright Chromium (for the test-e2e tasks of @paigasus/auth, @paigasus/discovery, @paigasus/app-shell and @paigasus/iam-console)
```
and add one line at the end of its `run:` block:
```yaml
          pnpm --dir ts/apps/iam-console exec playwright install --with-deps chromium
```
Add this sentence to the comment above the step: `@paigasus/iam-console needs it too (SMA-511): its test-e2e task drives the standalone server through a TLS terminator.` All four packages resolve the same catalog `@playwright/test`, so the line is explicit, not a second download.

- [ ] **Step 10: Format, then run the task through Moon and the workflow gate**

First format every `ts/` file that this task creates or edits (the Global Constraints). The code above is already in Prettier's form (checked with `printWidth: 200`), so the write is a guard against a typo. If Step 1 changed `package.json` or a file in `tests/support/`, add those paths to the command.
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console
pnpm -C ts exec prettier --write apps/iam-console/playwright.config.ts apps/iam-console/tests/e2e/global-setup.ts \
  apps/iam-console/tests/e2e/support/paths.ts apps/iam-console/tests/e2e/support/world.ts apps/iam-console/tests/e2e/support/harness.ts \
  apps/iam-console/tests/e2e/harness.spec.ts apps/iam-console/moon.yml
pnpm -C ts exec prettier --check apps/iam-console/playwright.config.ts apps/iam-console/tests/e2e apps/iam-console/moon.yml
```
Expected: the check prints `All matched files use Prettier code style!`.

Then run:
```bash
moon run iam-console-ts:test-e2e
moon run repo:actionlint
```
Expected: `1 passed` in the task output; `repo:actionlint` passes (it lints the edited workflow step).
If `repo:actionlint` runs for more than 10 minutes, it is the bash 5.3.15 here-string deadlock on this host, not a gate failure. Stop it. Then run the gate script with the system bash, `/bin/bash ci/actionlint/run.sh`, and record both facts (the hang and the `/bin/bash` result) in the task report.

- [ ] **Step 11: Commit**

```bash
git add ts/apps/iam-console/playwright.config.ts ts/apps/iam-console/tests/e2e ts/apps/iam-console/moon.yml .github/workflows/ci.yml
git add ts/apps/iam-console/package.json ts/pnpm-lock.yaml ts/apps/iam-console/tests/support   # only if Step 1 changed them
git commit -F - <<'EOF'
test(ts): add the iam-console e2e harness (SMA-511)

Playwright drives the production standalone server through an in-process
TLS terminator, with a fake IAM over gRPC and HTTP and a fake HTTPS IdP.
The fakes and the server start in a worker-scoped fixture, because a test
scripts the fake IAM and counts its calls, and a Playwright globalSetup runs
in another process. The session store is memory, so the zone map holds one
zone. The new iam-console-ts:test-e2e task depends on the app build, and CI
installs Chromium for it.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 22: The e2e scenarios — every row of spec § 9.4

**Files:**
- Read only: `ts/apps/iam-console/lib/correlation.ts` (`FORBIDDEN_VIEW_CORRELATION`, which Task 12 Step 19 sets) and `ts/apps/iam-console/app/_components/error-reference.tsx` (`data-testid="correlation-id"`, Task 12)
- Create: `ts/apps/iam-console/tests/e2e/support/login.ts`
- Create: `ts/apps/iam-console/tests/e2e/support/correlation.ts`
- Test: `ts/apps/iam-console/tests/e2e/public.spec.ts` (row R1)
- Test: `ts/apps/iam-console/tests/e2e/login.spec.ts` (rows R2, R3, R12)
- Test: `ts/apps/iam-console/tests/e2e/forbidden.spec.ts` (rows R4, R5, R6, R7)
- Test: `ts/apps/iam-console/tests/e2e/capabilities.spec.ts` (rows R8, R9, R10)
- Test: `ts/apps/iam-console/tests/e2e/token-leak.spec.ts` (row R11)
- Test: `ts/apps/iam-console/tests/e2e/session-expiry.spec.ts` (row R13, added by the final fix wave)
- Test: `ts/apps/iam-console/tests/unit/e2e-rows.test.ts`

**Interfaces:**
- Consumes: `test`, `expect`, `Harness` (Task 21); the world constants and `ALL_ACTIONS` (`tests/e2e/support/world.ts`); `denial` (`tests/support/fake-iam.ts`); `PRESENTATION_COPY` (`app/_components/error-copy.ts`, client-safe); the page structure of Tasks 16–19 (region names "Your organizations" and "All organizations", the form named "Create organization", the test id `create-organization-error`, the degraded test id `audit-degraded`); the nav labels of Task 15 ("Organizations", "Audit").
- Produces: `signIn`, `waitForHydration`, `redirectChain` (`tests/e2e/support/login.ts`); `forbiddenViewCorrelation`, `CorrelationMode` (`tests/e2e/support/correlation.ts`).

The rows, as spec § 9.4 numbers them (each test title starts with its row id; `tests/unit/e2e-rows.test.ts` holds that every row has exactly one test):

| Row | Scenario | AC |
|---|---|---|
| R1 | The public page `/iam/` loads with no cookie, with its CSS and JS | shell |
| R2 | Unauthenticated `/iam/orgs` → proxy redirect → IdP → callback → "Your organizations" lists the scopes; the fake saw `GetServiceInfo` first | 1 |
| R3 | A first-time user (not yet provisioned) lands on the same screen | 1 |
| R4 | A denied page read → HTTP 403 and the 403 view inside the shell, with the correlation id; the fake counted the call | 2 |
| R5 | `mayI` true and IAM denies → the 403 renders | 2 |
| R6 | `mayI` false → the create button is hidden, the reads still render, and a direct POST to the action reaches IAM | 2 |
| R7 | A denied Server Action → an inline 403 in the form | 2 |
| R8 | `iam.audit` reported and allowed → the Audit entry appears and the page works | 4 |
| R9 | `iam.audit` not reported → the entry is absent, and `/iam/audit` returns 404 | 4 |
| R10 | IAM degraded → the entries are disabled with a reason, and `/iam/audit` shows the degraded view | 4 |
| R11 | No response body (HTML, RSC payload or action result) contains the fake access token or refresh token | ADR-0017 |
| R12 | Sign out → POST `/iam/auth/logout` → `/iam/orgs` redirects to login again | 1 |
| R13 | A Server Action whose session ended shows the relogin link and keeps the browser inside `/iam` | 1 |

These specs test code that Tasks 10–19 already built, so most of them pass at once. A failure here is a defect in the app, not in the spec: fix the app. Change a spec only when this plan names a selector that the app renders in another way (Step 1 lists those).

- [ ] **Step 1: Confirm the correlation-id mode and the selectors that Task 12 wrote**

Spec § 6.2: the 403 view shows the correlation id only if the header route works (proxy mints an id, `lib/iam.ts` sends it to IAM as `paigasus-correlation-id`, and `forbidden.tsx` reads it with `headers()`). Otherwise the view shows no id, and `callIam` logs the id with the path. Task 12 Step 19 measured which one holds on a real standalone build and wrote the result into `FORBIDDEN_VIEW_CORRELATION`. The e2e tier must assert the mode that ships, and must fail when the code and the mode disagree. This step changes no file.

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console/ts/apps/iam-console
grep -n "export const FORBIDDEN_VIEW_CORRELATION" lib/correlation.ts
grep -c "requestCorrelationId\|CorrelationReference" 'app/(console)/forbidden.tsx'
grep -n 'data-testid="correlation-id"' app/_components/error-reference.tsx
grep -n 'data-testid="forbidden-view"' 'app/(console)/forbidden.tsx'
grep -n "PRESENTATION_COPY.forbidden.title" 'app/(console)/forbidden.tsx'
```
Expected:
1. The first grep prints ONE line, `export const FORBIDDEN_VIEW_CORRELATION: 'header' | 'fallback' = 'header';` or the same line with `'fallback'`. `tests/e2e/support/correlation.ts` (Step 2) parses exactly this form.
2. The count is above 0: the 403 view reads the minted id and renders it through `CorrelationReference`.
3. The third grep prints one line. `CorrelationReference` (Task 12, `error-reference.tsx`) is the ONE element that carries `data-testid="correlation-id"`, and the 403 view, `SectionError` and `FormError` all render it.
4. The fourth and fifth greps each print one line. R4 finds the 403 view by `data-testid="forbidden-view"` and by its heading, `PRESENTATION_COPY.forbidden.title` (spec § 6.2: "a fixed title"), which `ErrorState` renders as an `h2`.

If a grep prints nothing, Task 12 is not complete: stop and complete Task 12 (Steps 14, 15 and 19). Do not add a second constant or a second test id here.

- [ ] **Step 2: Write the two support modules**

`ts/apps/iam-console/tests/e2e/support/correlation.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The e2e tier reads FORBIDDEN_VIEW_CORRELATION from lib/correlation.ts as TEXT: the module imports
// server-only, which throws under Playwright. A missing literal is a loud error, not a default.
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { APP_DIR } from './paths';

export type CorrelationMode = 'header' | 'fallback';

export function forbiddenViewCorrelation(): CorrelationMode {
  const source = readFileSync(path.join(APP_DIR, 'lib', 'correlation.ts'), 'utf8');
  const match = /export const FORBIDDEN_VIEW_CORRELATION(?::[^=]+)?= '(header|fallback)'/.exec(source);
  if (match?.[1] === undefined) {
    throw new Error("lib/correlation.ts does not export FORBIDDEN_VIEW_CORRELATION = 'header' | 'fallback' on one line (Task 12 Step 19 sets it)");
  }
  return match[1] as CorrelationMode;
}
```

`ts/apps/iam-console/tests/e2e/support/login.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
import { expect, type Page, type Request, type Response } from '@playwright/test';
import type { Harness } from './harness';

export type SignedIn = { readonly accessToken: string; readonly refreshToken: string; readonly response: Response };

/**
 * Waits until React hydrated the page: Task 10's `Providers` sets `html[data-hydrated="true"]` in an
 * effect. A click before that is a plain document request, not a client navigation and not a
 * Server Action call (no `Next-Action` header), so every test waits for it before a click.
 */
export async function waitForHydration(page: Page): Promise<void> {
  await page.locator('html[data-hydrated="true"]').waitFor({ state: 'attached' });
}

/**
 * Open `path` with no session and follow the whole login: proxy -> /iam/auth/login -> the fake IdP
 * (it approves at once) -> /iam/auth/callback -> `path`. Every hop is an HTTP redirect, so one
 * page.goto() follows all of them. Returns the tokens the IdP issued for THIS login, after the page
 * hydrated.
 */
export async function signIn(page: Page, harness: Harness, path = '/iam/orgs'): Promise<SignedIn> {
  const before = harness.idp.issued.length;
  const response = await page.goto(harness.url(path));
  if (response === null) throw new Error(`page.goto(${path}) returned no response`);
  expect(new URL(page.url()).pathname).toBe(path);
  await expect(page.getByRole('navigation', { name: 'Primary' })).toBeVisible();
  await waitForHydration(page);
  const issued = harness.idp.issued.slice(before);
  expect(issued).toHaveLength(1);
  const [tokens] = issued;
  if (tokens === undefined) throw new Error('the fake IdP issued no token for this login');
  return { accessToken: tokens.accessToken, refreshToken: tokens.refreshToken, response };
}

/** Every hop of the navigation that produced `response`, first hop first. */
export async function redirectChain(response: Response): Promise<{ readonly url: URL; readonly status: number }[]> {
  const chain: { url: URL; status: number }[] = [];
  let request: Request | null = response.request();
  while (request !== null) {
    const hop = await request.response();
    chain.unshift({ url: new URL(request.url()), status: hop?.status() ?? 0 });
    request = request.redirectedFrom();
  }
  return chain;
}
```

- [ ] **Step 3: Write R1 — the public page**

`ts/apps/iam-console/tests/e2e/public.spec.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// The proxy's matcher (spec § 7.5) must let static assets through with NO cookie. Without it, the
// default matcher is a catch-all and every CSS and JS file of the public page is a login redirect.
import { expect, test } from './support/harness';

test('R1: the public page /iam/ loads with no cookie, with its CSS and JS (shell)', async ({ page, harness }) => {
  const assets: { pathname: string; status: number; type: string }[] = [];
  page.on('response', (response) => {
    const url = new URL(response.url());
    if (url.pathname.startsWith('/iam/_next/static/')) assets.push({ pathname: url.pathname, status: response.status(), type: response.request().resourceType() });
  });

  const response = await page.goto(harness.url('/iam/'));

  expect(response?.status()).toBe(200);
  // Next may normalise /iam/ to /iam (trailingSlash is off). Either way the proxy did not redirect to login.
  expect(new URL(page.url()).pathname).toMatch(/^\/iam\/?$/);
  expect(assets.filter((asset) => asset.type === 'stylesheet').length).toBeGreaterThan(0);
  expect(assets.filter((asset) => asset.type === 'script').length).toBeGreaterThan(0);
  expect(assets.filter((asset) => asset.status !== 200)).toEqual([]);
  // The CSS applied: globals.css paints the body with the design token.
  expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).not.toBe('rgba(0, 0, 0, 0)');
  expect((await page.context().cookies()).filter((cookie) => cookie.name === '__Host-pgs_sid')).toEqual([]);
});
```
Run:
```bash
pnpm --dir ts/apps/iam-console exec playwright test tests/e2e/public.spec.ts
```
Expected: `1 passed`.

- [ ] **Step 4: Write R2, R3 and R12 — login, the first-time user, sign out**

`ts/apps/iam-console/tests/e2e/login.spec.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// AC 1. The login flows under the /iam basePath (spec § 7.1): the proxy, /auth/login, the callback's
// redirect_uri, and requireSession(). And the provisioning order (spec § 4.5): IAM provisions a
// principal only inside a bearer-enforced call, so GetServiceInfo must come before Introspect.
import { ORG_NAME, OTHER_PROJECT_NAME, TEAM_NAME } from './support/world';
import { redirectChain, signIn } from './support/login';
import { expect, test } from './support/harness';

test('R2: an unauthenticated /iam/orgs logs in through the IdP and lands on "Your organizations" (AC 1)', async ({ page, harness }) => {
  const { accessToken, response } = await signIn(page, harness);

  const chain = await redirectChain(response);
  const idpOrigin = new URL(harness.idp.issuer).origin;
  const hops = chain.map(({ url }) => (url.origin === idpOrigin ? 'idp' : url.pathname)).filter((hop, index, all) => hop !== 'idp' || all[index - 1] !== 'idp');
  expect(hops).toEqual(['/iam/orgs', '/iam/auth/login', 'idp', '/iam/auth/callback', '/iam/orgs']);
  expect([302, 303, 307]).toContain(chain[0]?.status);
  expect(chain[1]?.url.searchParams.get('returnTo')).toBe('/iam/orgs');

  // Spec § 7.1 part 3: the token request's redirect_uri equals the authorization request's.
  const callback = harness.url('/iam/auth/callback');
  expect(harness.idp.authorizeRedirectUris.at(-1)).toBe(callback);
  expect(harness.idp.tokenRedirectUris.at(-1)).toBe(callback);

  const scopes = page.getByRole('region', { name: 'Your organizations' });
  await expect(scopes.getByRole('link', { name: ORG_NAME })).toBeVisible();
  await expect(scopes.getByRole('link', { name: TEAM_NAME })).toBeVisible();
  // From ListRoleGrants: the default descriptor reports iam.authz.cedar.
  await expect(scopes.getByRole('link', { name: OTHER_PROJECT_NAME })).toBeVisible();

  const mine = harness.iam.calls.filter((call) => call.token === accessToken);
  expect(mine[0]?.method).toBe('serviceInfo.getServiceInfo');
  expect(mine.some((call) => call.method === 'authn.introspect')).toBe(true);
});

test('R3: a first-time identity that IAM has never seen lands on the same screen (AC 1)', async ({ page, harness }) => {
  harness.useWorld({ memberships: false, allow: [] });

  const { accessToken } = await signIn(page, harness);

  expect(harness.iam.provisioned.has(accessToken)).toBe(true);
  const mine = harness.iam.calls.filter((call) => call.token === accessToken);
  const firstProvisioning = mine.findIndex((call) => call.method === 'serviceInfo.getServiceInfo');
  const firstIntrospect = mine.findIndex((call) => call.method === 'authn.introspect');
  expect(firstProvisioning).toBe(0);
  expect(firstIntrospect).toBeGreaterThan(firstProvisioning);

  const scopes = page.getByRole('region', { name: 'Your organizations' });
  await expect(scopes.getByText('You have no organizations yet')).toBeVisible();
  await expect(page.getByRole('region', { name: 'All organizations' })).toHaveCount(0);
  await expect(page.getByRole('form', { name: 'Create organization' })).toHaveCount(0);
});

test('R12: sign out posts /iam/auth/logout, and /iam/orgs then redirects to login again (AC 1)', async ({ page, harness }) => {
  await signIn(page, harness);
  const logout = page.waitForRequest((request) => request.method() === 'POST' && new URL(request.url()).pathname === '/iam/auth/logout');

  // The user menu is the LAST menu trigger in the header (the org switcher comes before it).
  await page.getByRole('banner').locator('button[aria-haspopup="menu"]').last().click();
  await page.getByRole('menuitem', { name: 'Sign out' }).click();
  const request = await logout;
  // waitForRequest resolves when the POST is SENT, and waitForLoadState() would resolve at once (the
  // old page is loaded). So wait for the post-logout page instead: handleLogout clears
  // __Host-pgs_sid in its 302 to the IdP's end_session_endpoint (after it revokes at the IdP), and
  // the fake IdP sends the browser back to `${PAIGASUS_PUBLIC_ORIGIN}/iam/`. When that URL is
  // current, the clearing response has arrived.
  await page.waitForURL((url) => /^\/iam\/?$/.test(url.pathname));
  expect((await request.response())?.status()).toBe(302);

  expect((await page.context().cookies()).filter((cookie) => cookie.name === '__Host-pgs_sid')).toEqual([]);
  const again = await page.request.get(harness.url('/iam/orgs'), { maxRedirects: 0 });
  expect([302, 303, 307]).toContain(again.status());
  expect(new URL(again.headers()['location'] ?? '', harness.origin).pathname).toBe('/iam/auth/login');
});
```
Run:
```bash
pnpm --dir ts/apps/iam-console exec playwright test tests/e2e/login.spec.ts
```
Expected: `3 passed`. If the chain shows `/iam/iam/auth/login` or a `redirect_uri` with `0.0.0.0`, a § 7.1 fix of Task 5 is missing: fix `@paigasus/auth`, not this test.

- [ ] **Step 5: Write R4–R7 — the 403 view and "the UI does not pre-judge"**

`ts/apps/iam-console/tests/e2e/forbidden.spec.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// AC 2 in both directions (spec § 6.3). IAM is authoritative: a denial renders the 403 (page: the
// forbidden() boundary with a REAL HTTP 403; section and form: inline, with the correlation id).
// And mayI() only hides: a Server Action whose button is hidden still reaches IAM. R6 posts through
// the REAL browser form, rendered while mayI() said yes, after IsAuthorized has flipped to no.
import { PRESENTATION_COPY } from '../../app/_components/error-copy';
import { denial } from '../support/fake-iam';
import { forbiddenViewCorrelation } from './support/correlation';
import { signIn } from './support/login';
import { ALL_ACTIONS, ORG_ID, ORG_NAME, ORG_PRN } from './support/world';
import { expect, test } from './support/harness';

test('R4: a denied page read is an HTTP 403 with the 403 view inside the shell (AC 2)', async ({ page, harness }) => {
  await signIn(page, harness);
  harness.useWorld({
    overrides: {
      // No explicit correlation id: the fake stamps the id of the call, as IAM does. That is the id
      // proxy.ts minted for this request, because IAM adopts a UUID it receives (Task 12). So the
      // view, IAM and the console log must all show ONE id.
      'tenancy.getOrganization': () => {
        throw denial();
      },
    },
  });
  const before = harness.iam.callsTo('tenancy.getOrganization').length;
  const pagePath = `/iam/orgs/${ORG_ID}`;

  const response = await page.goto(harness.url(pagePath));

  expect(response?.status()).toBe(403);
  await expect(page.getByTestId('forbidden-view')).toBeVisible();
  await expect(page.getByRole('heading', { name: PRESENTATION_COPY.forbidden.title })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Primary' })).toBeVisible();
  const denied = harness.iam
    .callsTo('tenancy.getOrganization')
    .slice(before)
    .filter((call) => (call.request as { prn: string }).prn === ORG_PRN);
  expect(denied.length).toBeGreaterThan(0);
  // The paigasus-correlation-id header as it ARRIVED at the fake: the id proxy.ts minted. Every call
  // of one request carries the same id (lib/iam.ts reads it once per request).
  const correlationId = denied.at(-1)?.correlationId ?? '';
  expect(correlationId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);

  if (forbiddenViewCorrelation() === 'header') {
    // The view shows exactly the id IAM adopted for the denied call.
    await expect(page.getByTestId('correlation-id')).toHaveText(correlationId);
  } else {
    // The fallback: no id in the view, and callIam logged one iam.call_failed line with that id
    // and the page path.
    await expect(page.getByTestId('correlation-id')).toHaveCount(0);
    await expect
      .poll(() =>
        harness
          .serverOutput()
          .split('\n')
          .some((line) => line.includes('"event":"iam.call_failed"') && line.includes(`"correlation_id":"${correlationId}"`) && line.includes(`"path":"${pagePath}"`)),
      )
      .toBe(true);
  }
});

test('R5: mayI says yes and IAM denies, so the 403 renders inline in the section (AC 2)', async ({ page, harness }) => {
  await signIn(page, harness);
  harness.useWorld({
    overrides: {
      'tenancy.listOrganizations': () => {
        throw denial({ correlationId: 'corr-e2e-section-403' });
      },
    },
  });
  const before = harness.iam.callsTo('tenancy.listOrganizations').length;

  const response = await page.reload();

  expect(response?.status()).toBe(200);
  const section = page.getByRole('region', { name: 'All organizations' });
  await expect(section).toContainText(PRESENTATION_COPY.forbidden.title);
  // A section 403 shows the id in BOTH correlation modes: the PaigasusError is right there (Task 12).
  await expect(section.getByTestId('correlation-id')).toHaveText('corr-e2e-section-403');
  expect(harness.iam.callsTo('tenancy.listOrganizations').length - before).toBe(1);
});

test('R6: mayI says no, so the button hides, the reads render, and a POST from the form still reaches IAM (AC 2)', async ({ page, harness }) => {
  await signIn(page, harness);
  const form = page.getByRole('form', { name: 'Create organization' });
  await expect(form).toBeVisible();

  // IsAuthorized now refuses CreateOrganization. The form above was rendered before this change.
  harness.useWorld({ allow: ALL_ACTIONS.filter((action) => action !== 'CreateOrganization') });
  const before = harness.iam.callsTo('tenancy.createOrganization').length;
  await form.getByLabel('Slug').fill('e2e-org');
  await form.getByLabel('Name').fill('E2E Org');
  const post = page.waitForResponse((candidate) => candidate.request().method() === 'POST' && new URL(candidate.url()).pathname === '/iam/orgs');
  await form.getByRole('button', { name: 'Create' }).click();
  const actionResponse = await post;

  // A Server Action POST under the basePath, through the TLS terminator, passed Next's origin check.
  expect(actionResponse.status()).toBe(200);
  expect((await actionResponse.request().allHeaders())['next-action']).toBeTruthy();
  const created = harness.iam.callsTo('tenancy.createOrganization').slice(before);
  expect(created).toHaveLength(1);
  expect(created[0]?.request).toMatchObject({ slug: 'e2e-org', name: 'E2E Org' });

  // The action's revalidatePath refreshed the page, BEFORE any reload (P5b-16): the new
  // organization is in "All organizations" (the world's ListOrganizations now returns it). Delete
  // the revalidatePath call and the old list stays, so this fails. A WRONG path fails here only
  // when Next matches the path to the page: Next 16.3.4 still refreshes the page for any path.
  await expect(page.getByRole('region', { name: 'All organizations' }).getByRole('link', { name: 'E2E Org' })).toBeVisible();

  await page.reload();
  await expect(page.getByRole('form', { name: 'Create organization' })).toHaveCount(0);
  await expect(page.getByRole('region', { name: 'Your organizations' }).getByRole('link', { name: ORG_NAME })).toBeVisible();
});

test('R7: a denied Server Action shows an inline 403 in the form (AC 2)', async ({ page, harness }) => {
  await signIn(page, harness);
  harness.useWorld({
    overrides: {
      'tenancy.createOrganization': () => {
        throw denial({ correlationId: 'corr-e2e-action-403' });
      },
    },
  });
  const form = page.getByRole('form', { name: 'Create organization' });
  await form.getByLabel('Slug').fill('denied-org');
  await form.getByLabel('Name').fill('Denied Org');

  await form.getByRole('button', { name: 'Create' }).click();

  const error = page.getByTestId('create-organization-error');
  // A form 403 shows the id in BOTH correlation modes (Task 12's FormError).
  await expect(error.getByTestId('correlation-id')).toHaveText('corr-e2e-action-403');
  await expect(error).not.toBeEmpty();
  await expect(form).toBeVisible();
});
```
Run:
```bash
pnpm --dir ts/apps/iam-console exec playwright test tests/e2e/forbidden.spec.ts
```
Expected: `4 passed`.
If R6 fails with a 500 and `Invalid Server Actions request` in `harness.serverOutput()`, the origin check failed: the terminator does not forward `Host`, or `X-Forwarded-Host` differs from the browser's origin. Fix the terminator (Task 11). Do not add `serverActions.allowedOrigins`: spec § 10 records `Host` forwarding as the deployment contract.

- [ ] **Step 6: Write R8–R10 — capability gating**

`ts/apps/iam-console/tests/e2e/capabilities.spec.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// AC 4 (spec § 6.6). PAIGASUS_DISCOVERY_*_MS are 1/2/3 ms in the harness, so each page load probes
// the fake's GET /v1/service-info again, and a test changes the IAM state between two loads.
import { AUDIT_ACTION } from './support/world';
import { signIn } from './support/login';
import { expect, test } from './support/harness';

test('R8: iam.audit reported and allowed, so the Audit entry appears and the page works (AC 4)', async ({ page, harness }) => {
  await signIn(page, harness);
  const nav = page.getByRole('navigation', { name: 'Primary' });
  const before = harness.iam.callsTo('audit.listAuditEntries').length;

  await nav.getByRole('link', { name: 'Audit', exact: true }).click();
  await page.waitForURL((url) => url.pathname === '/iam/audit');

  await expect(page.getByRole('heading', { name: 'Audit log' })).toBeVisible();
  await expect(page.getByRole('cell', { name: AUDIT_ACTION })).toBeVisible();
  expect(harness.iam.callsTo('audit.listAuditEntries').length).toBeGreaterThan(before);
});

test('R9: iam.audit not reported, so the entry is absent and /iam/audit is a 404 (AC 4)', async ({ page, harness }) => {
  harness.useWorld({ descriptor: { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar'] } });
  await signIn(page, harness);
  const nav = page.getByRole('navigation', { name: 'Primary' });

  await expect(nav.getByRole('link', { name: 'Organizations', exact: true })).toBeVisible();
  await expect(nav.getByRole('link', { name: 'Audit', exact: true })).toHaveCount(0);
  const before = harness.iam.callsTo('audit.listAuditEntries').length;
  const response = await page.goto(harness.url('/iam/audit'));
  expect(response?.status()).toBe(404);
  expect(harness.iam.callsTo('audit.listAuditEntries').length).toBe(before);
});

test('R10: IAM degraded, so the entries are disabled with a reason and /iam/audit shows the degraded view (AC 4)', async ({ page, harness }) => {
  await signIn(page, harness);
  harness.useWorld({ descriptor: { status: 503 } });

  await page.goto(harness.url('/iam/orgs'));

  const nav = page.getByRole('navigation', { name: 'Primary' });
  for (const label of ['Organizations', 'Audit']) {
    const entry = nav.getByRole('link', { name: label, exact: true });
    await expect(entry).toHaveAttribute('aria-disabled', 'true');
    const reasonId = await entry.getAttribute('aria-describedby');
    expect(reasonId).toBeTruthy();
    await expect(page.locator(`[id="${String(reasonId)}"]`)).toHaveText(/\S/);
  }

  const before = harness.iam.callsTo('audit.listAuditEntries').length;
  const response = await page.goto(harness.url('/iam/audit'));
  expect(response?.status()).toBe(200);
  await expect(page.getByTestId('audit-degraded')).toBeVisible();
  // A degraded IAM never reaches ListAuditEntries.
  expect(harness.iam.callsTo('audit.listAuditEntries').length).toBe(before);
});
```
Run:
```bash
pnpm --dir ts/apps/iam-console exec playwright test tests/e2e/capabilities.spec.ts
```
Expected: `3 passed`.

- [ ] **Step 7: Write R11 — no token in any response**

`ts/apps/iam-console/tests/e2e/token-leak.spec.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// ADR-0017: the browser never receives a token. Every response the page receives in a full session
// is collected: HTML documents, RSC payloads (navigations and prefetches), static assets, and a
// Server Action result. None may contain the access or the refresh token the fake IdP issued. The
// vacuity guards at the end prove that a non-empty BODY of each kind was in fact read and scanned:
// a response whose body read failed counts for no guard.
import type { Response } from '@playwright/test';
import { ORG_ID, ORG_NAME, PROJECT_ID, TEAM_ID } from './support/world';
import { signIn, waitForHydration } from './support/login';
import { expect, test } from './support/harness';

type Seen = {
  readonly url: string;
  readonly method: string;
  readonly contentType: string;
  readonly action: boolean;
  /** true only after response.body() succeeded. A redirect has no body, so it stays false. */
  readonly bodyRead: boolean;
  readonly body: string;
  /** The headers and the body: what the leak scan searches. */
  readonly text: string;
};

async function capture(response: Response): Promise<Seen> {
  const request = response.request();
  const headers = await response.allHeaders();
  const requestHeaders = await request.allHeaders();
  let body = '';
  let bodyRead = false;
  const status = response.status();
  if (status < 300 || status >= 400) {
    try {
      body = (await response.body()).toString('utf8');
      bodyRead = true;
    } catch {
      // The page navigated away before the body was read. bodyRead stays false, so no vacuity
      // guard counts this response as scanned.
    }
  }
  return {
    url: response.url(),
    method: request.method(),
    contentType: headers['content-type'] ?? '',
    action: requestHeaders['next-action'] !== undefined,
    bodyRead,
    body,
    text: `${JSON.stringify(headers)}\n${body}`,
  };
}

/** true when the body was read and is not empty, so the leak scan searched it. */
function scanned(response: Seen): boolean {
  return response.bodyRead && response.body.length > 0;
}

test('R11: no response body, header, RSC payload or action result contains a fake token (ADR-0017)', async ({ page, harness }) => {
  const pending: Promise<Seen>[] = [];
  page.on('response', (response) => {
    pending.push(capture(response));
  });
  const issuedBefore = harness.idp.issued.length;

  await signIn(page, harness);
  for (const path of [`/iam/orgs/${ORG_ID}`, `/iam/orgs/${ORG_ID}/teams/${TEAM_ID}`, `/iam/orgs/${ORG_ID}/teams/${TEAM_ID}/projects/${PROJECT_ID}`, '/iam/audit', '/iam/orgs']) {
    await page.goto(harness.url(path));
  }
  // A client-side navigation, so the RSC payload path is exercised, not only full documents. It
  // needs a hydrated page: before hydration the click is a plain document request. Wait for the
  // navigation's RSC response and for the new URL. waitForLoadState('networkidle') would resolve at
  // once, because the document reached that state at its first load, and the page.goto below could
  // then abort the RSC fetch.
  await waitForHydration(page);
  const rsc = page.waitForResponse((response) => (response.headers()['content-type'] ?? '').startsWith('text/x-component') && response.request().method() === 'GET');
  await page.getByRole('region', { name: 'Your organizations' }).getByRole('link', { name: ORG_NAME }).click();
  await rsc;
  await page.waitForURL((url) => url.pathname.startsWith('/iam/orgs/'));
  await page.goto(harness.url('/iam/orgs'));
  await waitForHydration(page);
  const form = page.getByRole('form', { name: 'Create organization' });
  await form.getByLabel('Slug').fill('leak-check');
  await form.getByLabel('Name').fill('Leak Check');
  await form.getByRole('button', { name: 'Create' }).click();
  await expect(form.getByRole('status')).toHaveText('Created.');
  await page.waitForLoadState('networkidle');

  const seen = await Promise.all(pending);
  const tokens = harness.idp.issued.slice(issuedBefore).flatMap((issued) => [issued.accessToken, issued.refreshToken]);
  expect(tokens.length).toBeGreaterThanOrEqual(2);
  for (const token of tokens) expect(token.length).toBeGreaterThanOrEqual(16);

  const leaks = seen.filter((response) => tokens.some((token) => response.text.includes(token))).map((response) => `${response.method} ${response.url}`);
  expect(leaks).toEqual([]);

  // Vacuity guards: for each kind of response the row names, at least one non-empty body was READ
  // and scanned. A response is not enough: if every body read of a kind failed, the scan above
  // searched only headers for that kind, and the guard fails.
  expect(seen.some((response) => response.contentType.startsWith('text/html') && scanned(response))).toBe(true);
  expect(seen.some((response) => response.contentType.startsWith('text/x-component') && !response.action && scanned(response))).toBe(true);
  expect(seen.some((response) => response.method === 'POST' && response.action && scanned(response))).toBe(true);
});
```
Run:
```bash
pnpm --dir ts/apps/iam-console exec playwright test tests/e2e/token-leak.spec.ts
```
Expected: `1 passed`. If the test times out at `await rsc` or at `waitForURL`, the click was not a client navigation (for example the page was not hydrated, or the link is a cross-zone link). Fix the app, not the wait.
If a vacuity guard fails, no body of that kind was read. Read the `seen` entries of that kind: a `bodyRead: false` entry means the page left before the read. Add a wait for that response before the next `page.goto`. Do not remove `scanned(...)` from a guard.
Keep the `toHaveText('Created.')` assertion. Only if revalidation hides the `Created.` text, change it to a check of the action result. Start the wait before the click, and assert the status and the header after it:
```ts
  const post = page.waitForResponse((candidate) => candidate.request().method() === 'POST' && new URL(candidate.url()).pathname === '/iam/orgs');
  await form.getByRole('button', { name: 'Create' }).click();
  const actionResponse = await post;
  expect(actionResponse.status()).toBe(200);
  expect((await actionResponse.request().allHeaders())['next-action']).toBeTruthy();
```
These five lines replace the `click()` line and the `toHaveText('Created.')` line. Do not use a bare `waitForResponse` with no assertion: that is a wait, not a result check.

- [ ] **Step 8: Write the row-coverage unit test**

`ts/apps/iam-console/tests/unit/e2e-rows.test.ts`:
```ts
// SPDX-License-Identifier: Apache-2.0
//
// Spec § 9.4 has twelve rows. Each must have exactly one Playwright test whose title starts with
// its row id. A deleted or renamed scenario then fails this vitest suite, which runs in
// iam-console-ts:test on every PR that touches the app, even when the e2e task does not run.
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const E2E_DIR = fileURLToPath(new URL('../e2e', import.meta.url));
const ROWS = Array.from({ length: 12 }, (_, index) => `R${String(index + 1)}`);

describe('the e2e tier covers every row of spec § 9.4', () => {
  const titles = readdirSync(E2E_DIR)
    .filter((file) => file.endsWith('.spec.ts'))
    .flatMap((file) => [...readFileSync(path.join(E2E_DIR, file), 'utf8').matchAll(/^test\('(R\d+):/gm)].map((match) => match[1]));

  it.each(ROWS)('%s has exactly one test', (row) => {
    expect(titles.filter((title) => title === row)).toHaveLength(1);
  });

  it('has no test for a row the spec does not have', () => {
    expect(titles.filter((title) => title === undefined || !ROWS.includes(title))).toEqual([]);
  });
});
```
Run:
```bash
pnpm --dir ts/apps/iam-console exec vitest run tests/unit/e2e-rows.test.ts
```
Expected: PASS, 13 tests.

- [ ] **Step 9: Run the whole tier through Moon, and the app checks**

First format every `ts/` file that this task creates (the Global Constraints). The code above is already in Prettier's form, so the write is a guard against a typo. It runs before `ts:fmt` and before the commit. Then run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console
pnpm -C ts exec prettier --write apps/iam-console/tests/e2e/support/login.ts apps/iam-console/tests/e2e/support/correlation.ts \
  apps/iam-console/tests/e2e/public.spec.ts apps/iam-console/tests/e2e/login.spec.ts apps/iam-console/tests/e2e/forbidden.spec.ts \
  apps/iam-console/tests/e2e/capabilities.spec.ts apps/iam-console/tests/e2e/token-leak.spec.ts apps/iam-console/tests/unit/e2e-rows.test.ts
moon run iam-console-ts:test-e2e
moon run iam-console-ts:test iam-console-ts:typecheck ts:lint ts:fmt --force
```
Expected: `13 passed` (the twelve rows and the harness test); the other tasks pass.

- [ ] **Step 10: Commit**

```bash
git add ts/apps/iam-console/tests/e2e ts/apps/iam-console/tests/unit/e2e-rows.test.ts
git commit -F - <<'EOF'
test(ts): cover every iam-console e2e scenario of the spec (SMA-511)

Twelve Playwright scenarios run against the standalone server: the public
page with no cookie, the full login under the basePath with the provisioning
order, a first-time identity, the HTTP 403 view, inline 403s in a section
and in a form, a Server Action posted from a real form after IsAuthorized
flipped to no, the three audit capability states, no token in any response,
and sign out. A vitest suite pins one test per row.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

### Task 23: Moon `inputs`/`dependsOn` final, the affected-graph cases, and the full CI run

**Files:**
- Modify (replace): `ts/apps/iam-console/moon.yml`
- Modify: `ts/moon.yml` (the `sources` group)
- Modify: `ci/affected-graph/run.sh`

**Interfaces:**
- Consumes: every app input that Tasks 8–22 created; Task 8's D6 outcome. **Branch K**: Task 8's wasm spike passed, and `ts/apps/iam-console/package.json` lists `@paigasus/kernel`. **Branch C** (the likely outcome): the spike failed, and `lib/prn.ts` is the fallback reader. Step 1 finds which. In BOTH branches Task 8 put the kernel parity vectors and `paigasus-iam-core/src/authz/model.rs` into `iam-console-ts:test`'s inputs, and this task keeps them.
- Produces: the final Moon graph of `iam-console-ts`; strict-equality cases in `ci/affected-graph/run.sh` that hold it.

Why each change (spec § 8, CLAUDE.md "Moon 2.5.3"): task `inputs` are the ONLY thing that makes a task affected. `dependsOn` is what `moon query projects --affected` follows, and it schedules an upstream but never selects a downstream. `build` and `test` use `options.merge: replace`, so they inherit nothing and list every input by hand. `typecheck` merges and inherits `@group(sources)`, `tsconfig.json`, `package.json`, `/ts/tsconfig.base.json` and `/ts/pnpm-lock.yaml`.

- [ ] **Step 1: Find the D6 branch and record the current input lines**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console
grep -c '"@paigasus/kernel"' ts/apps/iam-console/package.json
cp ts/apps/iam-console/moon.yml "$TMPDIR/iam-console-moon.before.yml"
```
Expected: `1` (Branch K) or `0` (Branch C). The copy lets Step 3 compare.

- [ ] **Step 2: Replace `ts/apps/iam-console/moon.yml`**

Write this content (Branch C). For Branch K, apply the three Branch K additions listed after the file.
```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'iam-console-ts'
layer: 'application'
language: 'typescript'

# Graph documentation, and what `moon query projects --affected` follows. This confers NO
# affectedness on its own: dependsOn schedules an upstream and never selects a downstream
# (CLAUDE.md, Moon 2.5.3). The task `inputs` below are what make these tasks affected by a change
# inside a package. Every package this app compiles is listed, so the `contracts->proto` case in
# ci/affected-graph/run.sh reaches this project (spec § 8).
dependsOn:
  - 'paigasus-next-config-ts'
  - 'paigasus-ui-ts'
  - 'paigasus-auth-ts'
  - 'paigasus-sdk-ts'
  - 'paigasus-discovery-ts'
  - 'paigasus-app-shell-ts'
  - 'paigasus-proto-ts'

# The inherited `sources` group from .moon/tasks/typescript-project.yml is `src/**/*`, and this app
# has no src/. Moon MERGES a project's file groups with the inherited ones (measured, SMA-503 M3), so
# `@group(sources)` resolves to src/**/* plus these. Without lib/**/* and proxy.ts, an edit there
# serves a cached pass (SMA-511 spec § 8). repo:input-liveness cannot see this: it scans `repo:*`.
fileGroups:
  sources:
    - 'app/**/*'
    - 'lib/**/*'
    - 'proxy.ts'

tasks:
  build:
    # `set -euo pipefail` is REQUIRED. Moon does not enable errexit for `script:` blocks and takes
    # a block's status from its LAST command, so without it a failed `next build` followed by a
    # satisfied file check exits 0 — a false green on SMA-502's headline acceptance criterion.
    #
    # The standalone assertion is what proves SMA-502 AC1 and SMA-511 AC 3: a Next version that
    # ignores `output: 'standalone'` would pass every unit test and ship an image carrying all of
    # node_modules. With outputFileTracingRoot pinned to ts/, Next writes the entry point under the
    # app's own subpath (docs/superpowers/specs/2026-09-08-sma-502-measurements.md).
    #
    # `rm -rf .next/static` guards the Tailwind guard's cache-MISS path (SMA-503 M6): the guard walks
    # all of .next/static and cannot tell a file this build wrote from one an earlier build left.
    # .next/cache must survive; deleting all of .next would make every build cold.
    script: |
      set -euo pipefail
      rm -rf .next/static
      pnpm exec next build
      if [ ! -f .next/standalone/apps/iam-console/server.js ]; then
        echo "iam-console: standalone entry point missing — output: 'standalone' did not take effect" >&2
        exit 1
      fi
    # `merge: replace` below: this task inherits NOTHING, so every input it needs is listed here.
    inputs:
      - '@group(sources)'
      - 'app/**/*'
      - 'lib/**/*'
      - 'proxy.ts'
      - 'tsconfig.json'
      - 'package.json'
      - 'next.config.ts'
      # At the project root, so no source group reaches it. It drives the whole CSS pipeline.
      - 'postcss.config.mjs'
      # Every package this app compiles: its sources AND its manifest (the `exports` map decides what
      # an import resolves to). Without these, a package edit serves a cached .next (SMA-519).
      - '/ts/packages/paigasus-next-config/src/**/*'
      - '/ts/packages/paigasus-next-config/package.json'
      # tsconfig.json extends @paigasus/next-config/tsconfig-app.
      - '/ts/packages/paigasus-next-config/tsconfig.app.json'
      # SMA-503 AC 3: the Tailwind @source proof reads this build's CSS.
      - '/ts/packages/paigasus-ui/src/**/*'
      - '/ts/packages/paigasus-ui/package.json'
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-sdk/src/**/*'
      - '/ts/packages/paigasus-sdk/package.json'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
      # Also the third Tailwind sentinel (spec § 7.6).
      - '/ts/packages/paigasus-app-shell/src/**/*'
      - '/ts/packages/paigasus-app-shell/package.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
      # The Tailwind, PostCSS and Next versions come from here (SMA-503 kept it on purpose).
      - '/ts/pnpm-lock.yaml'
      # `merge: replace` drops this inherited input. It sets exactOptionalPropertyTypes,
      # verbatimModuleSyntax and noUncheckedIndexedAccess.
      - '/ts/tsconfig.base.json'
    outputs:
      # SMA-502 AC4 — the cache archive must not carry .next/cache, which grows without bound.
      - '.next/**/*'
      - '!.next/cache/**/*'
    options:
      merge: replace

  typecheck:
    # APPENDED to the inherited inputs (typecheck merges). A type error introduced in a package would
    # otherwise leave this task's cache key unchanged.
    inputs:
      - 'app/**/*'
      - 'lib/**/*'
      - 'proxy.ts'
      # The root-level config files. tsconfig.json includes **/*.ts, so tsc type-checks them, but no
      # inherited input or source group reaches them. Without these, a type error there serves a
      # cached typecheck PASS.
      - 'next.config.ts'
      - 'vitest.config.ts'
      - 'playwright.config.ts'
      - 'tests/**/*'
      - '!tests/fixtures/**/.next/**'
      - '!tests/fixtures/*/next-env.d.ts'
      - '/ts/packages/paigasus-next-config/src/**/*'
      - '/ts/packages/paigasus-next-config/package.json'
      - '/ts/packages/paigasus-next-config/tsconfig.app.json'
      - '/ts/packages/paigasus-ui/src/**/*'
      - '/ts/packages/paigasus-ui/package.json'
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-sdk/src/**/*'
      - '/ts/packages/paigasus-sdk/package.json'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
      - '/ts/packages/paigasus-app-shell/src/**/*'
      - '/ts/packages/paigasus-app-shell/package.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'

  test:
    # Four jobs in one task, all against `~:build`'s output: the vitest tiers 1 and 2 (unit and
    # integration, fake IAM + MSW, no Docker), the standalone runtime smoke, the client-boundary
    # fixture builds (spec § 7.7), and the Tailwind @source guard. The guard runs three times: the
    # self-test and the negative control prove it CAN fire before the real run is trusted.
    # The guard lives at the REPOSITORY ROOT and must stay there: this directory is Tailwind's scan
    # root (ci/tailwind-source/README.md).
    script: |
      set -euo pipefail
      pnpm exec vitest run --passWithNoTests
      node ../../../ci/tailwind-source/run.mjs --self-test
      node ../../../ci/tailwind-source/run.mjs --negative-control
      node ../../../ci/tailwind-source/run.mjs
    deps:
      - '~:build'
      # repo:next-env-drift ALSO depends on this project's build, because `next typegen` writes into
      # .next. Without this edge, that gate and this task would race over the same directory.
      - 'repo:next-env-drift'
    inputs:
      - '@group(sources)'
      - 'app/**/*'
      - 'lib/**/*'
      - 'proxy.ts'
      - 'tests/**/*'
      # The fixtures' `next build` output and generated next-env.d.ts. The Moon hasher does NOT skip
      # `.next`, so without these every run would re-key this task (the app-shell fixture measured it).
      - '!tests/fixtures/**/.next/**'
      - '!tests/fixtures/*/next-env.d.ts'
      - 'vitest.config.ts'
      - 'tsconfig.json'
      - 'package.json'
      - 'next.config.ts'
      - 'postcss.config.mjs'
      # The guard itself. Without this, deleting an assertion from it serves a cached PASS.
      - '/ci/tailwind-source/**/*'
      - '/ts/packages/paigasus-next-config/src/**/*'
      - '/ts/packages/paigasus-next-config/package.json'
      - '/ts/packages/paigasus-next-config/tsconfig.app.json'
      - '/ts/packages/paigasus-ui/src/**/*'
      - '/ts/packages/paigasus-ui/package.json'
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-sdk/src/**/*'
      - '/ts/packages/paigasus-sdk/package.json'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
      - '/ts/packages/paigasus-app-shell/src/**/*'
      - '/ts/packages/paigasus-app-shell/package.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
      # SMA-511 D6 (Task 8, both branches): tests/unit/prn.test.ts replays the kernel parity corpus and
      # pins ROOT_PRN to root_prn()'s builder call. Without these, a corpus or model.rs edit serves a
      # cached PASS.
      - '/rs/crates/libs/paigasus-kernel-parity/vectors/**/*'
      - '/rs/crates/libs/paigasus-iam-core/src/authz/model.rs'
      - '/ts/pnpm-lock.yaml'
      - '/ts/tsconfig.base.json'
    options:
      merge: replace

  test-e2e:
    # The e2e tier (spec § 9.4): Playwright against the standalone server.js behind a TLS
    # terminator, with an in-process fake IAM and fake IdP. No Docker. `set -euo pipefail` because
    # Moon does not enable errexit for `script:` blocks. The server's lifecycle belongs to
    # tests/e2e/support/harness.ts; a shell `&` here left an orphan server in the SMA-510 spike.
    script: |
      set -euo pipefail
      pnpm exec playwright test
    deps:
      - '~:build'
      - 'repo:next-env-drift'
    # A NEW task name, so it inherits NOTHING: every input is listed.
    inputs:
      - '@group(sources)'
      - 'app/**/*'
      - 'lib/**/*'
      - 'proxy.ts'
      - 'tests/**/*'
      - '!tests/fixtures/**/.next/**'
      - '!tests/fixtures/*/next-env.d.ts'
      - 'package.json'
      - 'tsconfig.json'
      - 'next.config.ts'
      - 'postcss.config.mjs'
      - 'playwright.config.ts'
      - '/ts/pnpm-lock.yaml'
      - '/ts/tsconfig.base.json'
      - '/ts/packages/paigasus-next-config/src/**/*'
      - '/ts/packages/paigasus-next-config/package.json'
      - '/ts/packages/paigasus-next-config/tsconfig.app.json'
      - '/ts/packages/paigasus-ui/src/**/*'
      - '/ts/packages/paigasus-ui/package.json'
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-sdk/src/**/*'
      - '/ts/packages/paigasus-sdk/package.json'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
      - '/ts/packages/paigasus-app-shell/src/**/*'
      - '/ts/packages/paigasus-app-shell/package.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
    options:
      # A real build and a real Chromium. A cached PASS would replay a green that tested nothing.
      cache: false
```

**Branch K additions** (the app imports `@paigasus/kernel/wasm`; Task 8's `test` inputs — the parity vectors and `model.rs` — stay in BOTH branches). These are Task 8 Step 13W's lines, word for word:
1. `dependsOn` gains `- 'paigasus-kernel-ts'`, with Task 8's two-line comment above it.
2. `build`, `typecheck`, `test` and `test-e2e` each gain these inputs, after the `paigasus-proto` pair (Task 8 put them into `build` and `test`; `typecheck` and `test-e2e` compile the same code):
```yaml
      # SMA-511 D6: next build bundles the kernel's wasm entry and the committed glue.
      - '/ts/packages/paigasus-kernel/src/**/*'
      - '/ts/packages/paigasus-kernel/package.json'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm.js'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js'
      - '/rs/crates/bindings/paigasus-wasm/src/**/*'
      - '/rs/crates/libs/paigasus-kernel/src/**/*'
```
3. `build` keeps Task 8's `deps: ['^:build']` line, directly under its `script: |` block and before `inputs:`. It makes `paigasus-kernel-ts:build` write the gitignored `.wasm` before `next build` runs. It is an inline list; the Step 3 comparison parses the YAML, so it reports this entry as `tasks.build.deps` if it is lost.

- [ ] **Step 3: Carry over any input that an earlier task added and this file lacks**

Compare the two files PER TASK, not as one sorted set of lines. A whole-file set cannot see an input that Step 2 dropped from one task while another task keeps the same line (for example a `typecheck` line that `build` also lists). The command parses both files with PyYAML from `py/uv.lock`, and prints each entry of the old file that the new file does not have in the SAME section (`dependsOn`, `fileGroups.sources`, or one task's `inputs`, `deps` or `outputs`).

Run:
```bash
uv run --locked --project py python -c '
import sys

import yaml


def rows(path):
    with open(path, encoding="utf-8") as handle:
        doc = yaml.safe_load(handle)
    out = set()
    for entry in doc.get("dependsOn") or []:
        out.add(("dependsOn", str(entry)))
    for entry in (doc.get("fileGroups") or {}).get("sources") or []:
        out.add(("fileGroups.sources", str(entry)))
    for name, task in (doc.get("tasks") or {}).items():
        for key in ("inputs", "deps", "outputs"):
            for entry in task.get(key) or []:
                out.add((f"tasks.{name}.{key}", str(entry)))
    return out


dropped = sorted(rows(sys.argv[1]) - rows(sys.argv[2]))
for section, entry in dropped:
    print(f"< {section}: {entry}")
print("nothing dropped" if not dropped else f"{len(dropped)} entries dropped")
' "$TMPDIR/iam-console-moon.before.yml" ts/apps/iam-console/moon.yml
```
Expected: `nothing dropped`. A `<` line is an entry that an earlier task added to that section and Step 2 does not have (for example a root-level `vitest.setup.ts` in `tasks.test.inputs`). Add each one to the same section in `moon.yml`, and run the command again. Never drop an input.

- [ ] **Step 4: Make `ts:lint` key on the app's new code directories**

`ts:lint` runs `eslint .` over the whole tree, but Moon re-runs it only when a file of the `ts` project's `sources` or `tests` group changes. Before SMA-511 that group listed `apps/*/app/**/*` and not `lib/` or `proxy.ts`, so an edit there served a cached lint PASS. Task 8 added `apps/*/lib/**/*`, and Task 10 (Step 10) added `apps/*/proxy.ts`. Make the group EQUAL to the block below, whichever lines already exist. In `ts/moon.yml`, replace the `sources` group with:
```yaml
  sources:
    - 'packages/*/src/**/*'
    - 'apps/*/src/**/*'
    - 'apps/*/app/**/*'
    # SMA-511: the iam-console keeps its composition root in lib/ and its Next 16 proxy at the app
    # root. `eslint .` lints both; without these lines an edit there serves a cached lint PASS. The
    # `iam-console-lib->iam-console-tasks` and `iam-console-proxy->iam-console-tasks` cases in
    # ci/affected-graph/run.sh hold them.
    - 'apps/*/lib/**/*'
    - 'apps/*/proxy.ts'
    - 'tooling/**/*' # SMA-406 parity helpers; lint/fmt lint the whole tree, so the cache must track these too
```

- [ ] **Step 5: Update the affected-graph cases in `ci/affected-graph/run.sh`**

Each set below is derived from the inputs of Step 2 and Step 4. The rule that derives them: the no-flag `moon query tasks --affected` traversal (`_assert_task_case_impl`) selects a task when the touched file matches one of that task's `inputs`, filtered to the task names `build`, `test`, `lint` and `test-e2e`. The project cases (`run_case`) follow `dependsOn` with `--downstream deep`. Task 1 already renamed `paigasus-console-ts` to `iam-console-ts` in this file; the "old" text below is the text after Task 1 (and, in Branch K, after Task 8 Step 13W).

**A. Changed project case (both branches).** `contracts->proto`: the app now depends on `paigasus-proto-ts` (and on `sdk`, `discovery` and `app-shell`, which depend on it). Replace its expected CSV with:
```
"contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs,paigasus-iam-rs,paigasus-service-info-rs,paigasus-sdk-ts,paigasus-discovery-ts,paigasus-app-shell-ts,iam-console-ts"
```
and add this line at the end of its comment block: `# + iam-console-ts (SMA-511), which dependsOn paigasus-proto-ts and the three packages above.`

**B. Kernel project cases (both branches: no change in this task).** In Branch K the app `dependsOn` `paigasus-kernel-ts`, which depends on both binding crates. Task 8 Step 13W already appended `,iam-console-ts` to the expected CSV of `kernel->bindings`, `binding-oneway-node` and `binding-oneway-wasm`, each with the comment line `# + iam-console-ts (SMA-511, D6 wasm branch), through paigasus-kernel-ts.`. Do not append it again. In Branch C these three cases do NOT change.

**C. Changed task cases (both branches).** Every package case gains `iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e`, because all three app tasks list that package's `src/**/*`. Replace the expected CSVs with these exact strings:

| Case label | New expected CSV |
|---|---|
| `ui->console` | `"paigasus-app-shell-ts:build,paigasus-app-shell-ts:test,paigasus-app-shell-ts:test-e2e,iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,paigasus-ui-ts:build,paigasus-ui-ts:test,ts:lint"` |
| `ui-components->console` | same as `ui->console` |
| `auth->auth-tasks` | `"paigasus-app-shell-ts:build,paigasus-app-shell-ts:test,paigasus-app-shell-ts:test-e2e,paigasus-auth-ts:build,paigasus-auth-ts:test,paigasus-auth-ts:test-e2e,iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,ts:lint"` |
| `proto->sdk` | `"paigasus-proto-ts:build,paigasus-proto-ts:test,paigasus-sdk-ts:build,paigasus-sdk-ts:test,ts:lint,paigasus-discovery-ts:build,paigasus-discovery-ts:test,iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e"` |
| `proto-iam->sdk` | same as `proto->sdk` |
| `discovery->discovery-tasks` | `"paigasus-app-shell-ts:build,paigasus-app-shell-ts:test,paigasus-app-shell-ts:test-e2e,paigasus-discovery-ts:build,paigasus-discovery-ts:test,paigasus-discovery-ts:test-e2e,iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,ts:lint"` |
| `discovery-adapters->discovery-tasks` | same as `discovery->discovery-tasks` |
| `app-shell->app-shell-tasks` | `"paigasus-app-shell-ts:build,paigasus-app-shell-ts:test,paigasus-app-shell-ts:test-e2e,iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,ts:lint"` |
| `app-shell-shell->app-shell-tasks` | same as `app-shell->app-shell-tasks` |

Add one comment line above each changed case: `# SMA-511: iam-console-ts:{build,test,test-e2e} join this set — the app's inputs name this package's sources.`

**D. Changed task cases (Branch K only).** Task 8 Step 13W already appended `,iam-console-ts:build,iam-console-ts:test` to `kernel->consumer-tasks` and `lockfile->all-lint`. This task adds ONLY `,iam-console-ts:test-e2e` to each of the two CSVs, because the new `test-e2e` task (Task 21) joins both:
- `kernel->consumer-tasks` (no flags) anchors on `rs/crates/libs/paigasus-kernel/src/lib.rs`. Branch K addition 2 puts `/rs/crates/libs/paigasus-kernel/src/**/*` into `test-e2e`'s inputs, and that glob matches the anchor.
- `lockfile->all-lint` runs with `--downstream deep`, which follows task `deps`. `rs/Cargo.lock` selects `paigasus-kernel-ts:build`, the app's `build` depends on it through `^:build`, and `test-e2e` depends on the app's `build` through `~:build`.

Also widen Task 8's comment line above each of the two cases, so that it names the third task. The comment line above `lockfile->all-lint` becomes:
```bash
  # + iam-console-ts:{build,test,test-e2e} (SMA-511, D6 wasm branch), through task deps ('^:build', '~:build') on paigasus-kernel-ts:build, not through inputs.
```
The comment line above `kernel->consumer-tasks` becomes:
```bash
  # + iam-console-ts:{build,test,test-e2e} (SMA-511, D6 wasm branch): all three tasks list '/rs/crates/libs/paigasus-kernel/src/**/*' as an input.
```
The Task 8 comment line above `lockfile->all-lint-ci` becomes:
```bash
  # SMA-511 (D6 wasm branch): no longer equal — the deep set also holds iam-console-ts:{build,test,test-e2e}, which arrive through task deps.
```
The CSV of the `-ci` twin (`lockfile->all-lint-ci`, no flags) does NOT change: no app task keys on `rs/Cargo.lock`.

**E. Unchanged cases.** `proto-derive->proto`, `service-info->services`, `binding-oneway`, `parity-oneway`, `proto->svc-info-deep`, `proto->svc-info-ci`, `lockfile->all-lint-ci` and `gateway->sdk` in both branches, and in Branch C also `kernel->consumer-tasks` and `lockfile->all-lint`: the app's inputs match none of their anchors, no app task keys on `rs/Cargo.lock`, and (Branch C) the app has no project edge into the Rust graph. The parity vectors and `model.rs` are task inputs of `iam-console-ts:test`, not project edges, so `parity-oneway` stays as it is.

**F. New cases.** Add these after the `app-shell-shell->app-shell-tasks` case and before `gateway->sdk`. Find the file that holds the third Tailwind sentinel first:
```bash
git grep -l -- '--paigasus-app-shell-source-probe' -- ts/packages/paigasus-app-shell/src
```
Expected: exactly one path. The block below uses `ts/packages/paigasus-app-shell/src/shell/app-shell.tsx`; if the command printed another path, use that path in the `app-shell->console` line.
```bash
  # SMA-511 — a @paigasus/sdk SOURCE edit must select the iam-console's build, test and test-e2e,
  # plus the SDK's own build/test and ts:lint. This is the ONLY control on the three
  # '/ts/packages/paigasus-sdk/src/**/*' entries in ts/apps/iam-console/moon.yml: without them, an SDK
  # change serves a cached .next and a cached e2e verdict. Two anchors on opposite sides of the glob
  # (src/ root and src/errors/) prove its WIDTH, the precedent being the ui->console pair. The
  # expected set is DERIVED with the no-flag `moon query tasks --affected` traversal.
  run_task_case_ci "sdk->iam-console" "ts/packages/paigasus-sdk/src/iam.ts" \
    "paigasus-sdk-ts:build,paigasus-sdk-ts:test,iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,ts:lint"
  run_task_case_ci "sdk-errors->iam-console" "ts/packages/paigasus-sdk/src/errors/map-error.ts" \
    "paigasus-sdk-ts:build,paigasus-sdk-ts:test,iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,ts:lint"
  # SMA-511 — the file that holds the THIRD Tailwind sentinel (--paigasus-app-shell-source-probe,
  # spec § 7.6). ci/tailwind-source/run.mjs asserts it against the iam-console build, so an edit to
  # this file MUST rebuild the app. If the app's app-shell input narrowed away from this file, the
  # guard would read a cached .next and pass against stale CSS; this case is the control.
  run_task_case_ci "app-shell->console" "ts/packages/paigasus-app-shell/src/shell/app-shell.tsx" \
    "paigasus-app-shell-ts:build,paigasus-app-shell-ts:test,paigasus-app-shell-ts:test-e2e,iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,ts:lint"
  # SMA-511 — the app's OWN code outside app/: lib/ (the composition root) and proxy.ts. Without
  # `lib/**/*` and `proxy.ts` in the app's `sources` group, and `apps/*/lib/**/*` and
  # `apps/*/proxy.ts` in the ts project's, an edit there serves a cached build, test, e2e and lint
  # (spec § 8). Two anchors, one per path, because the two are separate input lines.
  run_task_case_ci "iam-console-lib->iam-console-tasks" "ts/apps/iam-console/lib/iam.ts" \
    "iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,ts:lint"
  run_task_case_ci "iam-console-proxy->iam-console-tasks" "ts/apps/iam-console/proxy.ts" \
    "iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,ts:lint"
```

**G. One comment.** In the `_assert_task_case_impl` header comment, replace the sentence that starts `Measured (SMA-510): three projects now declare \`test-e2e\`` up to `…include it.` with:
```bash
#   Measured (SMA-510, SMA-511): four projects now declare `test-e2e` — `paigasus-auth-ts`,
#   `paigasus-discovery-ts`, `paigasus-app-shell-ts` and `iam-console-ts`. app-shell's `test-e2e` keys
#   on the ui, auth, discovery and next-config sources; iam-console's keys on every package the app
#   compiles, so the ui, auth, discovery, app-shell, proto and sdk cases below include it.
```

- [ ] **Step 6: Run the affected-graph guard, read every mismatch, and re-derive**

Run:
```bash
/bin/bash ci/affected-graph/run.sh --negative-control
/bin/bash ci/affected-graph/run.sh
```
Expected: the control ends with `negative-control OK`, and the suite prints `PASS` for every case and ends with `== affected-graph cascade intact ==`.
Use `/bin/bash` (3.2): bash 5.3.15 on this machine deadlocks in this script's here-strings. CI runs it on Linux.

If a case FAILs, the output lists `missing` and `unexpected` rows. Do NOT loosen the expected set to make it pass. For each row, re-derive the answer from the real inputs:
```bash
printf '%s\n' '<the case anchor file>' | moon query tasks --affected \
  | python3 -c 'import sys,json; d=json.load(sys.stdin); print(",".join(sorted(f"{p}:{t}" for p,ts in (d.get("tasks") or {}).items() for t in ts if t in ("build","test","lint","test-e2e"))))'
printf '%s\n' '<the case anchor file>' | moon query projects --affected --downstream deep \
  | python3 -c 'import sys,json; print(",".join(sorted(p["id"] for p in json.load(sys.stdin)["projects"] if p["id"] != "repo")))'
```
Then find which input line or `dependsOn` entry produces each row (`moon query projects --id iam-console-ts` prints the resolved `inputFiles` and `inputGlobs` of every task). Two outcomes only:
1. The row follows from an input this plan meant to add (a Branch K row, or an input an earlier task added): change the expected set, and write the reason in the case's comment.
2. The row shows a MISSING input (an app task that a package edit does not select): add the input to `moon.yml`. That is the defect these cases exist to catch.

- [ ] **Step 7: Check the other gates that read Moon config, then commit**

Run (the `prettier --write` line lists the two `ts/` files this task edits, before the fmt check and the commit, as the Global Constraints require):
```bash
pnpm -C ts exec prettier --write apps/iam-console/moon.yml moon.yml
moon run repo:input-liveness repo:next-public-free repo:actionlint --force
moon run ts:lint ts:fmt --force
git diff --stat
```
Expected: all pass. `git diff --stat` lists only `ts/apps/iam-console/moon.yml`, `ts/moon.yml` and `ci/affected-graph/run.sh`.

Stop rule for a local hang: if `repo:actionlint` or `repo:affected-smoke` runs for more than 10 minutes, it is the bash 5.3.15 here-string deadlock on this machine, not a gate failure. Stop it. Run the gate script with the system bash instead: `/bin/bash ci/actionlint/run.sh`, or `/bin/bash ci/affected-graph/run.sh --negative-control && /bin/bash ci/affected-graph/run.sh`. Record the hang and that result in your report. Under `/bin/bash` 3.2, the `cargo-lock-step` self-test rows of `ci/actionlint/run.sh` can fail with `expected rc 0` (SMA-628). Record such rows too; CI (Linux, bash 5) judges them.

```bash
git add ts/apps/iam-console/moon.yml ts/moon.yml ci/affected-graph/run.sh
git commit -F - <<'EOF'
ci(ts): hold the iam-console's moon inputs with affected-graph cases (SMA-511)

The app's build, test and test-e2e now key on every package it compiles,
on lib/ and on proxy.ts, and the ts project's lint keys on apps/*/lib and
apps/*/proxy.ts. The contracts->proto case gains iam-console-ts, every
package case gains the three app tasks, and five new cases pin the SDK
input glob from two sides, the third Tailwind sentinel, and the app's own
lib/ and proxy.ts.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

- [ ] **Step 8: Confirm that CI selects the new task with no workflow change**

Run:
```bash
grep -n 'T=(' .github/workflows/ci.yml | grep -c ':test-e2e'
grep -c ':next-public-free :test-e2e' CLAUDE.md
```
Expected: `1` and `1`. `:test-e2e` is already in `ci.yml`'s `T=(…)` array and in the CLAUDE.md target list, so `iam-console-ts:test-e2e` needs no new registration. Do not add a target, and do not edit either list: `repo:affected-smoke` asserts that the two agree.

- [ ] **Step 9: Run the full CI graph before the push**

Fetch the base first, then run the command from CLAUDE.md's target list:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console
git fetch origin main
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :test-e2e \
  --base origin/main \
  --include-relations
```
Expected: exit 0. Docker must run (`paigasus-auth-ts:test-e2e` and the `paigasus-iam-rs` suites need it).
If a task fails, follow CLAUDE.md "Diagnosing an unattributed `moon ci` failure" BEFORE any re-run: copy `.moon/cache/ciReport.json` and `.moon/cache/states/<project>/<task>/` out of the repo first, because a re-run overwrites them.
<!-- moon-diagnosis:ok -->
(The marker above satisfies `ci/actionlint/run.sh` check 12: this file points at CLAUDE.md's procedure rather than restating it, so it cannot go stale against that procedure.)
Two known local traps. (1) The stop rule of Step 7: if `repo:actionlint` or `repo:affected-smoke` runs for more than 10 minutes, it is the bash 5.3.15 here-string deadlock, not a gate failure. Stop the run, run that gate's script with `/bin/bash` (`ci/actionlint/run.sh`, or `ci/affected-graph/run.sh --negative-control` and then `ci/affected-graph/run.sh`), and record the hang and that result. (2) A wall of "expected rc 0" self-test failures across unrelated gates means `/bin/bash` 3.2 ran a gate that needs bash 4+; check `which -a bash` before you read it as a finding. A Docker-backed suite that fails only under this parallel load: run the same suite on unmodified `origin/main` before you blame this branch.

### Task 24: Documentation — CLAUDE.md, `ts/README.md`, the app README, and the PR-ready checklist

**Files:**
- Modify: `CLAUDE.md` (the Turbopack entry; five new Gotchas entries)
- Modify: `ts/README.md`
- Create: `ts/apps/iam-console/README.md`

**Interfaces:**
- Consumes: the facts that Tasks 1–23 measured or built. Task 8's D6 outcome (Branch K or C) and the `FORBIDDEN_VIEW_CORRELATION` value that Task 12 Step 19 measured choose between two given sentences in two entries.
- Produces: documentation only.

Task 1 already renamed every `paigasus-console` mention in `CLAUDE.md`. This task adds new content only. Do not add or quote the `ci-targets` markers anywhere: a second copy of either marker reds `repo:affected-smoke`. Do not touch the `moon-diagnosis` block: `repo:actionlint` check 12 pins it.

- [ ] **Step 1: Confirm the rename is complete**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/feature+sma-511-iam-console
git grep -n paigasus-console -- ':!docs/superpowers' || echo "no hits"
grep -c 'moon-diagnosis:begin' CLAUDE.md
grep -c 'ci-targets:begin' CLAUDE.md
```
Expected: `no hits`, `1`, `1`. Task 12's request header is `x-paigasus-request-path`, so this grep has no hit on it. If the first command prints a hit, Task 1 missed a site: fix it with Task 1's rule (`paigasus-console` → `iam-console`), in a separate commit.

- [ ] **Step 2: Replace the Turbopack entry in `CLAUDE.md`**

Replace the whole bullet that starts `- **Turbopack (Next 16.3.4) does NOT resolve a \`.js\` relative specifier to a \`.ts\` file**` and ends `…so nothing but a Next build notices.` with:
```markdown
- **Turbopack (Next 16.3.4) does NOT resolve a `.js` relative specifier to a `.ts` file** (MEASURED,
  SMA-510): `import { x } from './a.js'` with only `a.ts` on disk fails `next build` with `Module not
  found`, in app code and in a workspace package's source alike. A clause-level `import type … from
  './a.js'` is erased first and builds (measured). `import { type A } from './a.js'` is not erased
  under `verbatimModuleSyntax` (reasoned from the flag's rules, not separately measured). So every
  file a Next app compiles uses EXTENSIONLESS relative value imports. SMA-511 made that true for every
  package the IAM console compiles: the `src/` of `@paigasus/auth`, `@paigasus/sdk`,
  `@paigasus/discovery` and the hand-written `@paigasus/proto` files, and BOTH buf templates
  (`contracts/buf.gen.yaml`, `contracts/buf.gen.googleapis.yaml`) no longer pass
  `import_extension=.js`, so the generated protobuf-es code is extensionless too. Two controls hold
  it. The ESLint rule `paigasus/no-js-relative-specifier` reports an `import`, `export … from` or
  `import()` whose `./`/`../` specifier ends in `.js`, under `packages/*/src/**`, test files
  excluded. It ships as `sourceRules` from `@paigasus/next-config/eslint`, NOT inside
  `boundaryRules` (a `packages/*/src` scope there fails the reverse liveness loop), and
  `ts/eslint.config.js` spreads it, which a test pins. It is a rule with its OWN name on purpose: in
  flat config a second `no-restricted-imports` block that matches the same files REPLACES the first
  and switches the boundary rules off without a word. ESLint ignores `**/generated/**`, so a
  `@paigasus/proto` test asserts the same thing for `src/generated/`. Test files keep their `.js`
  imports (vitest resolves both). The two plain-Node loaders that run package source
  (`paigasus-auth/tests/fixtures/ts-esm-loader.mjs`,
  `paigasus-discovery/tests/containers/support/ts-esm-loader.mjs`) retry an extensionless specifier
  as `.ts`, then `/index.ts`, because plain Node does not probe extensions. Vite, vitest, tsc and
  Playwright accept both forms, so only a Next build or these two controls notices a regression.
```

- [ ] **Step 3: Add five new Gotchas entries after the Turbopack entry (before `## Workflow`)**

Insert this text directly after the bullet of Step 2. It contains two choices; keep one sentence of each pair and delete the other, as the bracketed rule says.
```markdown
- **`@paigasus/auth` under a Next `basePath`** (MEASURED on Next 16.3.4, SMA-511 spec § 13 row 1).
  Next removes the basePath before app code sees a path, in three different places. In `proxy.ts`,
  `req.nextUrl.pathname` has no `/iam`, while `req.nextUrl.basePath` is `/iam` and `req.url` keeps
  it. In a route handler, `req.url` has no basePath AND carries the server's bind address
  (`http://0.0.0.0:<port>`); only its scheme follows `X-Forwarded-Proto`. A page
  `redirect('/auth/login')` gets the basePath added once, and `redirect('/iam/auth/login')` becomes
  `/iam/iam/auth/login`. So `authRoutePaths()` takes no argument and returns basePath-RELATIVE
  paths, `requireSession` redirects to the relative login path, `createAuthRouteHandler` rebuilds the
  URL from `AuthRuntime.publicOrigin` + basePath, and `handleCallback` builds openid-client's
  `currentUrl` from `runtime.redirectUri`. That last part broke `redirect_uri` equality even with NO
  basePath. A unit test sees none of this without `new NextRequest(url, { nextConfig: { basePath:
  '/iam' } })`, and the plain-Node auth e2e harness passes full paths, so the iam-console e2e tier
  (`iam-console-ts:test-e2e`, row R2) is the only end-to-end control.
- **`@paigasus/kernel` cannot load its napi binding inside a Next build** (MEASURED 2026-09-11,
  SMA-634 open). `@paigasus/node-bindings` is a pnpm `file:` dependency whose `files` allowlist is
  `["index.js", "index.d.ts"]`, so pnpm never copies the `.node` binary into `node_modules`, and
  `next build` fails at "Collecting page data" with `Cannot find native binding`. Every Node consumer
  of `@paigasus/kernel` has the same defect.
  [Branch K — keep this sentence:] The iam-console reads PRNs through the kernel's `./wasm` subpath
  export instead (decision D6); wasm is platform-neutral, so the console image needs no native binary.
  [Branch C — keep this sentence:] The iam-console's `lib/prn.ts` is a small reader for the IAM
  tenancy PRN shapes (decision D6, fallback C). It is a recorded ADR-0005 exception, and
  `tests/unit/prn.test.ts` replays the kernel parity corpus through it, so a divergence from the
  kernel reds `iam-console-ts:test`.
- **`forbidden()` needs `experimental.authInterrupts`, and a React `cache()` value does not reach
  `forbidden.tsx`** (MEASURED on Next 16.3.4, SMA-511). Without the flag, `forbidden()` throws
  instead of rendering the 403 boundary. The iam-console sets it through
  `createNextConfig({ extend: { experimental: { authInterrupts: true } } })`, and its vitest env needs
  `__NEXT_EXPERIMENTAL_AUTH_INTERRUPTS=true` or the call throws E488. A nested `forbidden.tsx` is a
  per-segment boundary: `(console)/forbidden.tsx` renders inside the `(console)` layout with a real
  HTTP 403. `forbidden()` takes no argument, and a `cache()` holder set before the call is EMPTY in
  the `forbidden.tsx` render, so the view cannot receive request data that way.
  [`FORBIDDEN_VIEW_CORRELATION = 'header'` — keep this sentence:] The view gets the correlation id
  from a request header instead: `proxy.ts` mints it, `lib/iam.ts` sends it to IAM as
  `paigasus-correlation-id`, IAM adopts it, and `forbidden.tsx` reads it with `headers()`.
  [`FORBIDDEN_VIEW_CORRELATION = 'fallback'` — keep this sentence:] The view therefore shows no
  correlation id; `callIam` logs the id with the path, and the section and form 403s show it.
  `lib/correlation.ts`'s `FORBIDDEN_VIEW_CORRELATION` records which of the two ships, and e2e row R4
  fails if the view does the other. The flag is experimental: R4 asserts the real HTTP 403, so a Next
  upgrade that changes it reds CI.
- **A Playwright `globalSetup` runs in another process than the tests.** A fake server that a test
  must script, or whose calls a test must count, cannot start there. The iam-console e2e tier only
  checks the build and copies `.next/static` in `tests/e2e/global-setup.ts`, and starts the fake IAM,
  the fake IdP, the TLS terminator and the standalone server in a WORKER-scoped fixture
  (`tests/e2e/support/harness.ts`, `workers: 1`). Playwright starts a new worker after a failed test,
  and the fixture then starts the whole stack again. Anything that fixture imports runs WITHOUT the
  vitest `server-only` stub, so `tests/support/` must not import a guarded `@paigasus/sdk` entry or a
  `lib/` file (use `@paigasus/proto/iam`, which the `apps/*/tests/support/**` boundary exemption allows).
- The `ts` project's `sources` group names app code directories BY HAND (`apps/*/app/**/*`,
  `apps/*/lib/**/*`, `apps/*/proxy.ts`). `ts:lint` runs `eslint .` over the whole tree, but Moon
  re-runs it only for a file in its `sources` or `tests` group (or one of its config inputs), so a
  new top-level app directory needs a line in `sources`, or an edit to it serves a cached lint PASS.
  The same holds for a top-level app file such as `playwright.config.ts`.
```
Run:
```bash
grep -n '\[Branch K\|\[Branch C\|\[`FORBIDDEN_VIEW_CORRELATION' CLAUDE.md || echo "choices resolved"
grep -c 'ci-targets:begin' CLAUDE.md
```
Expected: `choices resolved` and `1`.

- [ ] **Step 4: Update `ts/README.md`**

Replace the four lines that list the packages:
```markdown
  - `paigasus-proto` (`@paigasus/proto`) — generated proto types post-MVP (consumes `contracts/`)
  - `paigasus-kernel` (`@paigasus/kernel`) — thin wrapper over the napi-rs binding to `paigasus-kernel-rs`, post-MVP
  - `paigasus-sdk` (`@paigasus/sdk`) — public SDK placeholder
  - `paigasus-ui` (`@paigasus/ui`) — shared React components for the console
```
with:
```markdown
  - `paigasus-proto` (`@paigasus/proto`) — protobuf-es types generated from `contracts/` (extensionless imports; see CLAUDE.md, Turbopack)
  - `paigasus-kernel` (`@paigasus/kernel`) — the Rust kernel for Node (napi) and the browser (wasm); a Next build cannot load the napi binding (SMA-634)
  - `paigasus-sdk` (`@paigasus/sdk`) — server-only Connect clients for the IAM gRPC services, and the error map
  - `paigasus-ui` (`@paigasus/ui`) — shared React components for the console
  - `paigasus-next-config` (`@paigasus/next-config`) — the Next config factory, runtime config and the ESLint boundary and source rules
  - `paigasus-auth` (`@paigasus/auth`) — the BFF session: OIDC login, the session store and the proxy
  - `paigasus-discovery` (`@paigasus/discovery`) — capability discovery with three service states
  - `paigasus-app-shell` (`@paigasus/app-shell`) — the console chrome: header, navigation, switchers, cross-zone links
```
Replace the line that Task 1 wrote:
```markdown
  - `iam-console` (`@paigasus/iam-console`) — Next.js 16 (App Router) console zone for IAM, mounted at `/iam`
```
with:
```markdown
  - `iam-console` (`@paigasus/iam-console`) — Next.js 16 (App Router) console zone for IAM, mounted at `/iam`; see its [README](apps/iam-console/README.md) for the environment and the test tiers
```
In the Commands table, add this row directly after the `Build (one app)` row:
```markdown
| E2E (one app)   | `moon run iam-console-ts:test-e2e`                  |
```
Then run:
```bash
pnpm -C ts exec prettier --write README.md
```

- [ ] **Step 5: Write the app README**

`ts/apps/iam-console/README.md`:
```markdown
# `@paigasus/iam-console`

The IAM zone of the Paigasus console (ADR-0017). It is a Next.js 16 app, mounted at `/iam`, that
runs as a standalone server. It has the login, the console shell, the tenancy screens, the 403
view and the capability-gated audit screen. Design: `docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md`.

## Rules that the code depends on

- Every file in `lib/` starts with `import 'server-only'`. A client component that imports one fails the build.
- Relative imports in `app/`, `lib/` and `proxy.ts` have no file extension (Turbopack, see `CLAUDE.md`).
- No page and no Server Action skips its IAM call because `mayI()` said no. `mayI()` (`lib/authorize.ts`) only hides a button, a form, a section or a nav entry. IAM decides.
- Every Server Action gets its client through `iamClients()`, so `requireSession()` runs for each action. `tests/unit/actions-structure.test.ts` holds this rule, and a new action must be added to its list.
- URLs use UUIDs. A segment that is not a UUID is a 404 with no IAM call. A team or project whose IAM answer disagrees with the URL is a 404.
- There is no `loading.tsx` under `app/(console)/`, and none may be added: its Suspense boundary lets Next send status 200 before a page's `forbidden()` throws, so the 403 status is lost (e2e R4 asserts it).
- No `NEXT_PUBLIC_` anywhere (`repo:next-public-free`). Every deployment value is read at request time.

## Environment

The image reads these variables at the first request. A parse failure stops every request.

| Group | Variables |
|---|---|
| Zone | `PAIGASUS_ZONE=iam`, `PAIGASUS_ZONES` (JSON, the same in every zone) |
| Auth | the `authEnvShape` keys (`ts/packages/paigasus-auth/README.md`). Use `PAIGASUS_SESSION_STORE=redis` in a deployment with more than one zone. |
| Discovery | `PAIGASUS_SERVICES` (JSON; it must contain `iam`, IAM's HTTP address), optional `PAIGASUS_DISCOVERY_*_MS` |
| IAM | `PAIGASUS_IAM_GRPC_URL` (IAM's gRPC address: absolute `http:` or `https:`, no credentials, no query, no fragment) |

Deployment assumptions (spec § 10):

- IAM's `authn.issuers[].audiences` accepts the audience of the console client's access tokens.
- The ingress forwards `Host` (or `X-Forwarded-Host`) unchanged. Otherwise Next's Server Action origin check fails.
- The console reaches IAM's gRPC port over h2c in the cluster, or over TLS with a CA that Node trusts (`NODE_EXTRA_CA_CERTS`).
- The OIDC client registers `https://<origin>/iam/auth/callback` as a redirect URI and `https://<origin>/iam/` as the post-logout URI.

## Commands

| Task | Command |
|---|---|
| Build (asserts the standalone `server.js`) | `moon run iam-console-ts:build` |
| Unit, integration, standalone smoke, client-boundary fixture, Tailwind guard | `moon run iam-console-ts:test` |
| Browser tier | `moon run iam-console-ts:test-e2e` |
| Type check | `moon run iam-console-ts:typecheck` |

No tier needs Docker or a live service.

## Test tiers

- **Unit and integration** (`tests/unit`, `tests/integration`, vitest). The integration tests talk real gRPC to an in-process fake IAM (`tests/support/fake-iam.ts`), and MSW serves `GET /v1/service-info`. Loaders (`load.ts`) take their IAM client and `mayI` as arguments, and commands (`commands.ts`) take their IAM client only, so the tests call them with no session and no Next runtime.
- **Client boundary** (`tests/build/client-boundary.test.ts`). `next build` on `tests/fixtures/client-imports-sdk` must fail with the `server-only` error; the same code in a server component (`tests/fixtures/server-imports-sdk`) must build.
- **Browser** (`tests/e2e`, Playwright). A production build runs through the standalone server behind an in-process TLS terminator, with the fake IAM and a fake HTTPS IdP. The session store is `memory`, so the zone map holds one zone. Each row of the spec's § 9.4 table is one test (`R1`–`R12`); `tests/unit/e2e-rows.test.ts` holds that.

## Known limits

- The attach form takes a raw principal PRN: IAM has no user lookup RPC.
- `mayI()` fails open: when `IsAuthorized` fails, a button shows, and IAM then denies the action.
- The e2e tier runs one zone. A two-zone test belongs to SMA-513.
- `Introspect` reports no role grants (SMA-633), so the app asks `IsAuthorized` about itself instead of reading grants from the session (a recorded departure from ADR-0017 decision 8).
- `forbidden()` needs the experimental `authInterrupts` flag. E2e row R4 asserts the real HTTP 403, so a Next upgrade that changes the flag reds CI. The fallback, an inline view with status 200, is recorded in spec § 6.2 and not built.
- The 403 view shows the correlation id only when `FORBIDDEN_VIEW_CORRELATION` (`lib/correlation.ts`) is `'header'`. When it is `'fallback'`, `callIam` logs the id with the path, and the section and form 403s show it.
- The console layout calls `myScopes()` on every console page for the organization switcher. That is one `Introspect`, up to ten `ListRoleGrants` pages and up to 50 tenancy reads per render. The cost is not measured (spec § 12). If it is too slow, a short-lived per-session cache is the next step.
- [Branch C only — delete this bullet in Branch K:] `lib/prn.ts` is a recorded ADR-0005 exception (decision D6, fallback C). `tests/unit/prn.test.ts` holds it to the kernel: it replays every vector of the kernel parity corpus.
- The kernel's napi binding cannot load in a Next build, because `@paigasus/node-bindings` ships no `.node` binary. The defect stays open for every Node consumer of `@paigasus/kernel` (SMA-634).
```
The Known limits hold one choice. In Branch C, remove only the bracketed prefix and keep the bullet. In Branch K, delete the whole bullet. Then run:
```bash
pnpm -C ts exec prettier --write apps/iam-console/README.md
grep -n '\[Branch C only' ts/apps/iam-console/README.md || echo "choice resolved"
```
Expected: `choice resolved`.

- [ ] **Step 6: Check and commit**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:fmt --force
/bin/bash ci/affected-graph/run.sh
moon run repo:actionlint --force
```
Expected: all pass. `ci/affected-graph/run.sh` reads the CLAUDE.md target list between its markers, and `repo:actionlint` check 12 reads the diagnosis block; both prove this task did not disturb them.
If `repo:actionlint` runs for more than 10 minutes, apply the stop rule of Task 23 Step 7: it is the bash 5.3.15 here-string deadlock, not a gate failure. Stop it, run `/bin/bash ci/actionlint/run.sh`, and record the hang and that result.

```bash
git add CLAUDE.md ts/README.md ts/apps/iam-console/README.md
git commit -F - <<'EOF'
docs(ts): record what the iam console measured and how to run it (SMA-511)

CLAUDE.md now says the Turbopack limit is fixed for every package the app
compiles and names the lint rule and the proto test that hold it. It gains
entries on the auth adapters under a basePath, the napi kernel packaging
defect (SMA-634), forbidden() with the authInterrupts flag and the cache()
holder, the Playwright globalSetup process, and the ts lint sources group.
The workspace README lists every package, and the app gets a README with
its environment, commands and test tiers.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

- [ ] **Step 7: The SMA-511 PR-ready checklist**

Do each item and tick it. Do not push until every implementer item is done.

Implementer:
1. `git status` is clean, and `git log --oneline origin/main..HEAD` shows one commit per task (plus any Task 1 fix-up commit from Task 24 Step 1), each with the `Co-Authored-By` line.
2. Every commit message passes commitlint: `pnpm -C ts exec commitlint --from origin/main --to HEAD`. No body line starts with `#<number>` or with `word: value`.
3. Task 23 Step 9's full `moon ci` run exited 0 on the final `HEAD`. If a commit came after it, run it again.
4. `/bin/bash ci/affected-graph/run.sh --negative-control && /bin/bash ci/affected-graph/run.sh` passes.
5. `moon run iam-console-ts:test-e2e` reports 13 passed.
6. `git grep -n 'NEXT_PUBLIC_' -- ts/apps/iam-console` finds only the documentation mentions of the ban.
7. `git grep -nE "from '\.\.?/[^']+\.js'" -- 'ts/apps/iam-console/app' 'ts/apps/iam-console/lib' ts/apps/iam-console/proxy.ts` finds nothing.
8. The spec's § 13 "still to measure" items each have a recorded result: the § 7.1 fixes (e2e R2, R12), the D6 spike (Task 8's decision), the correlation header (the `FORBIDDEN_VIEW_CORRELATION` constant that Task 12 Step 19 measured, which e2e R4 asserts), and the Server Action POST under the basePath (e2e R6).

Owner (Sven) — the plan cannot do these:
9. Update the Linear acceptance criteria of SMA-511 for AC 2, 3, 4 and 5, each with its reason from spec § 2.2.
10. Add the ADR-0017 decision 8 note in Notion (spec § 12: `IsAuthorized` self-queries instead of session grants).
11. Confirm the recorded limit "attach by raw principal PRN" (spec § 12, § 14 open question).
