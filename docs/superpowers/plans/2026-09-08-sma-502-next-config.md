<!-- SPDX-License-Identifier: Apache-2.0 -->

# `@paigasus/next-config` Implementation Plan (SMA-502)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `@paigasus/next-config` — a Next config factory, a request-time env validator, a client-safe config projection, an eslint package-boundary preset — plus a CI gate that bans `NEXT_PUBLIC_` across `ts/`, so one OCI image can serve many self-hosted deployments.

**Architecture:** A source-only pnpm workspace package (`private: true`, `exports` point at `src/*.ts`), consumed by `ts/apps/paigasus-console`. Build-time inlining is allowed only for values that describe the **image** (the zone id and its path prefix, written by the factory into `env:`); every deployment-varying value is read and zod-validated at first request. A runtime cross-check compares the compiled zone and prefix against the deployed zone map and fails closed on disagreement.

**Tech Stack:** TypeScript 6.0.3, Next 16.3.4 (Turbopack is the default `next build` bundler), React 19.2.8, zod 4.5.4, vitest 5, ESLint 10 flat config, Moon 2.5.3, pnpm 11, bash for the CI gate.

**Spec:** `docs/superpowers/specs/2026-09-08-sma-502-next-config-design.md` — read it before Task 1. This plan argues from it; section references below (`spec § 4.2`) point into it.

## Global Constraints

- **SPDX header on every new source file.** `// SPDX-License-Identifier: Apache-2.0` for TS/JS, `# SPDX-License-Identifier: Apache-2.0` for bash. This task adds roughly fourteen files.
- **Branch:** `feature/sma-502-next-config` (already checked out, in the worktree `.claude/worktrees/sma-502`). Never `cd` out of the worktree.
- **Commits:** conventional, with a workspace scope — `feat(ts): …`, `feat(ci): …`, `test(ts): …`. Subject starts lowercase, ≤100 chars. Keep `#NNN` out of the body: a `#NNN` ref or a stray `token: value` line in the body fails `footer-leading-blank`. Every commit message ends with the line `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>` as its own trailer.
- **PATH:** prefix every tool command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `moon`, `pnpm`, `node` and `uv` resolve to the repo-pinned versions. Shims first.
- **`PROTO_REPORTER=text`** must be exported at the top of any bash script that captures `proto` or a proto-shimmed tool's stdout. Intermittent NDJSON leakage is measured and real.
- **No error message may print the value of an env var that came from `extraShape`.** Values in the public slice (`zone`, `zones`) are exempt — they ship to the browser by design, so naming them in an error leaks nothing. This is the one refinement to spec § 5.5's absolute wording, and Task 3 tests it.
- **`exactOptionalPropertyTypes: true`** is on in `ts/tsconfig.base.json`. `{ basePath: undefined }` is NOT assignable to `basePath?: string`. Use conditional spread (`...(x === '' ? {} : { basePath: x })`), never an explicit `undefined`.
- **`noUncheckedIndexedAccess: true`** is on. Indexing a `Record<string, string>` yields `string | undefined`. Handle it; do not cast.
- **Prettier:** `printWidth: 200`, `semi: true`, `singleQuote: true`, `trailingComma: 'all'`, `arrowParens: 'always'`. Run `moon run ts:fmt` before every commit that touches `ts/` — it is a separate whole-tree CI gate from `ts:lint`.
- **The console's `next dev`/`next build` need `PAIGASUS_ZONE` and `PAIGASUS_ZONES` set.** Task 5 adds `.env.local.example`; `.gitignore:2-4` already carries `!.env*.example`, so no `.gitignore` change is needed.

---

## File Structure

**Created:**

| Path | Responsibility |
|---|---|
| `ts/packages/paigasus-next-config/package.json` | Package manifest; four subpath exports |
| `ts/packages/paigasus-next-config/moon.yml` | Moon project; `test.inputs` adds `/ts/eslint.config.js` |
| `ts/packages/paigasus-next-config/tsconfig.json` | Package typecheck config |
| `ts/packages/paigasus-next-config/tsconfig.app.json` | The shared Next-app TS preset |
| `ts/packages/paigasus-next-config/vitest.config.ts` | Node-environment vitest config |
| `ts/packages/paigasus-next-config/src/base-path.ts` | `canonicalBasePath()` — one canonical prefix form |
| `ts/packages/paigasus-next-config/src/index.ts` | `createNextConfig()` |
| `ts/packages/paigasus-next-config/src/runtime.ts` | `defineRuntimeConfig()`, the public projection |
| `ts/packages/paigasus-next-config/src/eslint.mjs` | The boundary preset (plain ESM) |
| `ts/packages/paigasus-next-config/tests/*.test.ts` | Four test files, one per source unit |
| `ts/apps/paigasus-console/app/runtime-config.ts` | The app's single `defineRuntimeConfig()` call |
| `ts/apps/paigasus-console/.env.local.example` | Documents the two variables |
| `ci/next-public/run.sh` | The gate: two checks, self-test, negative control |
| `ci/next-public/README.md` | Gate rationale and Limitations |

**Modified:**

| Path | Change |
|---|---|
| `ts/pnpm-workspace.yaml` | `zod` and `server-only` catalog entries |
| `ts/eslint.config.js` | Consume the boundary preset |
| `ts/apps/paigasus-console/next.config.ts` | Call `createNextConfig()` |
| `ts/apps/paigasus-console/app/page.tsx` | Render the zone map at request time |
| `ts/apps/paigasus-console/package.json` | Depend on `@paigasus/next-config` |
| `ts/apps/paigasus-console/tsconfig.json` | Extend the shared preset |
| `ts/apps/paigasus-console/moon.yml` | `script:` build with the standalone assertion; inputs; outputs |
| `moon.yml` | `repo:next-public-free`; `ci/next-public/**/*` in `affected-smoke` inputs |
| `.github/workflows/ci.yml` | `:next-public-free` in the `T=(…)` array |
| `CLAUDE.md` | `:next-public-free` in the marker-delimited command |
| `ci/affected-graph/ci_targets.py` | Four registries + the eighth `check_self_invocation` haystack |
| `ci/actionlint/run.sh` | `ci/next-public/**/*` in `T_AFFECTED_SMOKE_REQUIRED_INPUTS` |

---

## Task 1: Measure the five environmental unknowns

The spec's § 13 lists five facts the design depends on that are not knowable from documentation. Measure them first so no later task guesses. Each has a stated fallback.

**Files:**
- Create: `docs/superpowers/specs/2026-09-08-sma-502-measurements.md`
- Scratch only (reverted at the end of the task): `ts/apps/paigasus-console/next.config.ts`, `ts/apps/paigasus-console/app/probe/page.tsx`

**Interfaces:**
- Produces: a committed measurements document. Tasks 2, 4, 5 and 8 read these values:
  - `STANDALONE_ENTRY` — the path, relative to the console dir, of the standalone server entry point.
  - `ASSET_PREFIX_COMPOSES` — `true` if Next concatenates `basePath` and `assetPrefix`.
  - `MOON_NEGATED_GLOBS` — `true` if Moon 2.5.3 honours `!`-prefixed `inputs`/`outputs` globs.
  - `TS_LOADABLE_FROM_NEXT_CONFIG` / `TS_LOADABLE_FROM_ESLINT_CONFIG` — booleans.
  - `NEXT_PHASE_REACHES_WORKERS` — `true` if a module-scope throw fails the build.

- [ ] **Step 1: Confirm the worktree has installed deps**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ls ts/node_modules/.bin/next && ls ts/node_modules/.pnpm/node_modules/next/package.json
```

Expected: both exist. If not, run `proto install && pnpm -C ts install` first.

- [ ] **Step 2: M5a — can `next.config.ts` import TypeScript source from a workspace package?**

Create a throwaway package entry and import it. Write `ts/packages/paigasus-next-config/package.json` and `src/index.ts` with the minimum needed (Task 2 rewrites both):

```bash
mkdir -p ts/packages/paigasus-next-config/src
```

`ts/packages/paigasus-next-config/package.json`:

```json
{
  "name": "@paigasus/next-config",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "engines": { "node": ">=24" },
  "license": "Apache-2.0",
  "exports": { ".": "./src/index.ts" }
}
```

`ts/packages/paigasus-next-config/src/index.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
export const PROBE = 'next-config-probe';
```

Add the dependency and install:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm --filter @paigasus/console add '@paigasus/next-config@workspace:*' && cd ..
```

Replace `ts/apps/paigasus-console/next.config.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
import type { NextConfig } from 'next';
import { PROBE } from '@paigasus/next-config';

console.log('[probe] loaded from workspace package:', PROBE);

const nextConfig: NextConfig = { output: 'standalone' };
export default nextConfig;
```

- [ ] **Step 3: M5a + M2 — run the build and read both answers at once**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/paigasus-console && pnpm exec next build 2>&1 | tee /tmp/sma502-m2.log; cd ../../..
```

Expected: the log contains `[probe] loaded from workspace package: next-config-probe` — that answers `TS_LOADABLE_FROM_NEXT_CONFIG`. If instead it errors on the import, record `false` and note that Task 2 must ship `src/index.mjs` with a generated `src/index.d.mts` (see the § 13 M5 fallback — **generated with `tsc --emitDeclarationOnly`, never hand-written**).

Then find the entry point:

```bash
find ts/apps/paigasus-console/.next/standalone -name 'server.js' -maxdepth 4
```

Record the path relative to `ts/apps/paigasus-console/` as `STANDALONE_ENTRY`. The spec predicts `.next/standalone/apps/paigasus-console/server.js`; if it is `.next/standalone/server.js`, record that instead. **Do not assume — record what the command printed.**

- [ ] **Step 4: M2b — confirm `next build` clears `.next` except `cache`**

```bash
touch ts/apps/paigasus-console/.next/standalone/STALE-MARKER
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/paigasus-console && pnpm exec next build >/dev/null 2>&1; cd ../../..
ls ts/apps/paigasus-console/.next/standalone/STALE-MARKER
```

Expected: `No such file or directory` — the marker is gone, so a stale artifact cannot satisfy Task 5's assertion. If the marker survives, record it and Task 5 adds `rm -rf .next/standalone` before the build.

- [ ] **Step 5: M3 — how do `basePath` and `assetPrefix` compose?**

Set `basePath: '/iam'` only in `next.config.ts`, build, and read an emitted asset URL:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/paigasus-console && pnpm exec next build >/dev/null 2>&1; cd ../../..
grep -ro '"/[^"]*_next/static/[^"]*"' ts/apps/paigasus-console/.next/server/app/ 2>/dev/null | head -3
```

Record the prefix. Then set **both** `basePath: '/iam'` and `assetPrefix: '/iam'`, rebuild, and re-read. If the second run yields `/iam/iam/_next/…`, record `ASSET_PREFIX_COMPOSES=true`.

- [ ] **Step 6: M1 — does the build-phase throw actually fire, including in prerender workers?**

```bash
mkdir -p ts/apps/paigasus-console/app/probe
```

`ts/apps/paigasus-console/app/probe/page.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
// Throwaway probe: a MODULE-SCOPE read, which is exactly what the build-phase guard must catch.
if (process.env.NEXT_PHASE === 'phase-production-build') {
  throw new Error('PROBE: module scope evaluated during next build');
}
const evaluatedAt = process.env.NEXT_PHASE ?? '(unset)';

export default function ProbePage() {
  return <p>{evaluatedAt}</p>;
}
```

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/paigasus-console && pnpm exec next build 2>&1 | tee /tmp/sma502-m1.log; cd ../../..
grep -c 'PROBE: module scope evaluated during next build' /tmp/sma502-m1.log
```

Expected: the build FAILS and the message appears. Record `NEXT_PHASE_REACHES_WORKERS=true`.

If the build succeeds and the probe page renders `(unset)`, the workers do **not** inherit the variable. Record `false`, and Task 3's guard changes: `getRuntimeConfig()` keeps the `NEXT_PHASE` throw (it still catches a module-scope read in the main process) but the design's enforcement moves to requiring `await connection()` in every page that reads config, documented in `ci/next-public/README.md` as a residual the gate does not cover.

- [ ] **Step 7: M4 — does Moon 2.5.3 honour negated `inputs`/`outputs` globs?**

Temporarily set the console's `moon.yml` `outputs` to `['.next/**/*', '!.next/cache/**/*']`, then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-console-ts:build --force 2>&1 | tail -20
moon query tasks --affected 2>/dev/null >/dev/null; echo "graph load rc=$?"
```

Expected: the graph loads (rc 0) and the build succeeds. A negation Moon rejects surfaces as a graph-load error naming the glob. Record `MOON_NEGATED_GLOBS`.

Also record whether `.next`'s dot-prefixed entries are captured: `moon run paigasus-console-ts:build --force` then inspect the task's cached output archive under `.moon/cache/outputs/` for a `.next/BUILD_ID`-style entry.

- [ ] **Step 8: M5b — can `ts/eslint.config.js` import TypeScript from the package?**

Add to `ts/packages/paigasus-next-config/src/index.ts`:

```ts
export const ESLINT_PROBE = [];
```

Add to the top of `ts/eslint.config.js`: `import { ESLINT_PROBE } from '@paigasus/next-config';` and spread `...ESLINT_PROBE,` as the first element of the exported array.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm exec eslint --no-warn-ignored packages/paigasus-ui/src/index.ts; echo "rc=$?"; cd ..
```

Expected: rc 0 and no loader error. Record `TS_LOADABLE_FROM_ESLINT_CONFIG`. If it errors, the preset ships as `src/eslint.mjs` — **which is the plan's default anyway**, so a `false` here costs nothing.

- [ ] **Step 9: Revert every scratch change**

```bash
git checkout -- ts/eslint.config.js ts/apps/paigasus-console/next.config.ts ts/apps/paigasus-console/package.json ts/pnpm-lock.yaml
rm -rf ts/apps/paigasus-console/app/probe ts/packages/paigasus-next-config ts/apps/paigasus-console/.next
# Restore the pnpm store. The rm -rf above leaves it holding a link to a workspace package that no
# longer exists, and Task 2's install is the next thing to touch it.
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts install
git status --short
```

Expected: clean, apart from the measurements file you are about to write. **Restore by deleting the files you added and `git checkout`-ing the files you edited — never by a blanket revert, which would also discard the measurements document.**

- [ ] **Step 10: Write the measurements document**

Create `docs/superpowers/specs/2026-09-08-sma-502-measurements.md` with an SPDX header, the date, the exact tool versions (`node --version`, `pnpm --version`, `moon --version`, and Next's version from `ts/pnpm-lock.yaml`), and one section per measurement recording **the command run and its literal output**, then the derived value. Where a fallback was triggered, say so and name the task it changes.

- [ ] **Step 11: Commit**

```bash
git add docs/superpowers/specs/2026-09-08-sma-502-measurements.md
git commit -m "docs(ts): record SMA-502 environment measurements" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 2: Package scaffold, catalog entries, and the config factory

**Files:**
- Create: `ts/packages/paigasus-next-config/{package.json,moon.yml,tsconfig.json,vitest.config.ts}`
- Create: `ts/packages/paigasus-next-config/src/{base-path.ts,index.ts}`
- Create: `ts/packages/paigasus-next-config/tests/{base-path.test.ts,next-config.test.ts}`
- Modify: `ts/pnpm-workspace.yaml`

**Interfaces:**
- Produces:
  - `canonicalBasePath(value: string, label: string): string` from `./base-path.js`
  - `createNextConfig(options: CreateNextConfigOptions): NextConfig` from the package root
  - `interface CreateNextConfigOptions { zone: string; basePath: string; assetPrefix?: string; outputFileTracingRoot: string; extend?: NextConfig }`

- [ ] **Step 1: Resolve the two new dependency versions**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
npm view zod version && npm view server-only version
npm view zod time --json | python3 -c "import sys,json;d=json.load(sys.stdin);import datetime;v=json.loads(open('/dev/null').read() or '{}');print()" 2>/dev/null || true
npm view zod time --json | python3 -c "import sys,json; d=json.load(sys.stdin); print([(k,v) for k,v in d.items() if k[0].isdigit()][-1])"
```

Expected: `zod` at 4.5.x. Confirm the published date is more than 24 hours old — pnpm 11's `minimumReleaseAge` defaults to 24h and a same-day release reds CI. If the newest is same-day, pin the previous patch.

- [ ] **Step 2: Add the catalog entries**

In `ts/pnpm-workspace.yaml`, inside the `catalog:` block, after the `vitest` entry:

```yaml
  # Runtime env validation for @paigasus/next-config (SMA-502). zod 4 semantics are assumed
  # throughout that package: unknown object keys are stripped by default, z.record takes
  # (keyType, valueType), and errors surface as error.issues[]. The runtime formats messages from
  # .path and .code and never renders an input value, so a validation failure cannot log a secret.
  zod: ^4.5.4
  # Build-time guard that @paigasus/next-config/runtime never reaches a client or edge bundle.
  # The edge runtime has no dynamic process.env, so an edge import would read inlined or
  # undefined values rather than deployment values (spec § 5).
  'server-only': ^0.0.1
```

Replace both version numbers with what Step 1 printed.

- [ ] **Step 3: Write the package manifest**

`ts/packages/paigasus-next-config/package.json`:

```json
{
  "name": "@paigasus/next-config",
  "_comment_exports": "Source-only exports. Bundler-aware consumers only (Next/Vitest/tsc walk through TS via moduleResolution: bundler). Switch to ./dist/index.js when tsup wiring lands — must happen IN LOCKSTEP with flipping `private: false` for publishable packages.",
  "_comment_eslint": "./eslint is plain ESM, not TypeScript: ts/eslint.config.js is loaded by ESLint's own resolver and configuration data gains little from types. See the spec § 13 M5.",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "engines": {
    "node": ">=24"
  },
  "license": "Apache-2.0",
  "description": "Next.js config factory, request-time runtime configuration, and shared eslint/tsconfig presets for Paigasus console zones.",
  "exports": {
    ".": "./src/index.ts",
    "./runtime": "./src/runtime.ts",
    "./eslint": "./src/eslint.mjs",
    "./tsconfig-app": "./tsconfig.app.json"
  },
  "scripts": {
    "typecheck": "tsc -p tsconfig.json --noEmit"
  },
  "dependencies": {
    "server-only": "catalog:",
    "zod": "catalog:"
  },
  "peerDependencies": {
    "next": "catalog:"
  },
  "devDependencies": {
    "@types/node": "catalog:",
    "eslint": "catalog:",
    "next": "catalog:",
    "typescript": "catalog:",
    "vitest": "catalog:"
  }
}
```

- [ ] **Step 4: Write the Moon project, tsconfig and vitest config**

`ts/packages/paigasus-next-config/moon.yml`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-next-config-ts'
layer: 'library'
language: 'typescript'

tasks:
  # `/ts/eslint.config.js` is added to the INHERITED test inputs, not merged away: tests/boundaries
  # asserts that the workspace eslint config actually applies this package's preset, and the
  # inherited input list (.moon/tasks/typescript-project.yml) does not name that file. Without it
  # Moon replays a cached PASS on exactly the PR that deletes the import — the guard-the-guard
  # hole the assertion exists to close (spec § 9.4).
  test:
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - 'vitest.config.ts'
      - 'package.json'
      - '/ts/eslint.config.js'
      - '/ts/pnpm-lock.yaml'
```

`ts/packages/paigasus-next-config/tsconfig.json`:

```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "lib": ["ES2022"],
    "types": ["node"],
    "outDir": "./dist",
    "rootDir": ".",
    "noEmit": true,
    "allowJs": true
  },
  "include": ["src/**/*", "tests/**/*"]
}
```

`ts/packages/paigasus-next-config/vitest.config.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts'],
  },
});
```

- [ ] **Step 5: Write the failing tests for `canonicalBasePath`**

`ts/packages/paigasus-next-config/tests/base-path.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { canonicalBasePath } from '../src/base-path.js';

describe('canonicalBasePath', () => {
  it('accepts an already-canonical prefix unchanged', () => {
    expect(canonicalBasePath('/iam', 'test')).toBe('/iam');
  });

  it('treats a trailing slash as equivalent', () => {
    expect(canonicalBasePath('/iam/', 'test')).toBe('/iam');
  });

  it('adds a missing leading slash', () => {
    expect(canonicalBasePath('iam', 'test')).toBe('/iam');
  });

  it('canonicalises a nested prefix', () => {
    expect(canonicalBasePath('/admin/iam/', 'test')).toBe('/admin/iam');
  });

  it("maps the origin root to '' because Next rejects basePath '/'", () => {
    expect(canonicalBasePath('/', 'test')).toBe('');
    expect(canonicalBasePath('', 'test')).toBe('');
  });

  it('rejects a value that is not a path prefix', () => {
    expect(() => canonicalBasePath('https://evil.example/x', 'test')).toThrow(/test/);
  });

  it('never echoes the rejected value, because the same validator reads operator JSON', () => {
    const secret = 'sk-live-should-never-appear';
    expect(() => canonicalBasePath(`https://${secret}.example`, 'PAIGASUS_ZONES["iam"]')).toThrow();
    try {
      canonicalBasePath(`https://${secret}.example`, 'PAIGASUS_ZONES["iam"]');
    } catch (err) {
      expect(String(err)).not.toContain(secret);
      expect(String(err)).toContain('PAIGASUS_ZONES["iam"]');
    }
  });
});
```

- [ ] **Step 6: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm install && pnpm --filter @paigasus/next-config exec vitest run tests/base-path.test.ts; cd ..
```

Expected: FAIL — `Failed to resolve import "../src/base-path.js"`.

- [ ] **Step 7: Implement `canonicalBasePath`**

`ts/packages/paigasus-next-config/src/base-path.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0

/** Matches one or more `/`-separated path segments of URL-safe unreserved characters. */
const PATH_PREFIX = /^(?:\/[A-Za-z0-9._~-]+)+$/;

/**
 * One canonical form for a zone prefix: a leading `/`, no trailing `/`, and `''` for a zone
 * mounted at the origin root — Next rejects `basePath: '/'`.
 *
 * Both sides of the compiled-versus-deployed cross-check run through this, so `/iam`, `/iam/`
 * and `iam` compare equal. Without it, a trailing slash in an operator's `PAIGASUS_ZONES` JSON
 * hard-fails a correctly configured deployment (spec § 4.2).
 *
 * The rejected value is NEVER included in the error. This validator also reads operator-supplied
 * JSON, so echoing it could put a mis-pasted secret into a log. `label` carries the location,
 * which is what a reader actually needs.
 */
export function canonicalBasePath(value: string, label: string): string {
  if (typeof value !== 'string') {
    throw new Error(`${label}: expected a string base path`);
  }
  const trimmed = value.trim();
  if (trimmed === '' || trimmed === '/') {
    return '';
  }
  const withLeading = trimmed.startsWith('/') ? trimmed : `/${trimmed}`;
  const withoutTrailing = withLeading.replace(/\/+$/, '');
  if (!PATH_PREFIX.test(withoutTrailing)) {
    throw new Error(`${label}: not a valid base path prefix (value withheld — it may be operator input)`);
  }
  return withoutTrailing;
}
```

- [ ] **Step 8: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm --filter @paigasus/next-config exec vitest run tests/base-path.test.ts; cd ..
```

Expected: 7 passed.

- [ ] **Step 9: Write the failing tests for `createNextConfig`**

`ts/packages/paigasus-next-config/tests/next-config.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { createNextConfig } from '../src/index.js';

const base = { zone: 'iam', basePath: '/iam', outputFileTracingRoot: '/repo/ts' } as const;

describe('createNextConfig', () => {
  it("forces output: 'standalone'", () => {
    expect(createNextConfig(base).output).toBe('standalone');
  });

  it('does not let extend override the standalone rule', () => {
    expect(createNextConfig({ ...base, extend: { output: 'export' } }).output).toBe('standalone');
  });

  it('rejects an extend.env key, because env inlines at build time', () => {
    expect(() => createNextConfig({ ...base, extend: { env: { ISSUER: 'https://idp.example' } } })).toThrow(/extend\.env/);
  });

  it('writes the compiled zone and base path into env', () => {
    const cfg = createNextConfig(base);
    expect(cfg.env).toEqual({ PAIGASUS_COMPILED_ZONE: 'iam', PAIGASUS_COMPILED_BASE_PATH: '/iam' });
  });

  it('canonicalises the base path on both the config and the compiled env value', () => {
    const cfg = createNextConfig({ ...base, basePath: 'iam/' });
    expect(cfg.basePath).toBe('/iam');
    expect(cfg.env?.PAIGASUS_COMPILED_BASE_PATH).toBe('/iam');
  });

  it("omits basePath entirely for a root-mounted zone, because Next rejects '/'", () => {
    const cfg = createNextConfig({ ...base, basePath: '/' });
    expect('basePath' in cfg).toBe(false);
    expect(cfg.env?.PAIGASUS_COMPILED_BASE_PATH).toBe('');
  });

  it('does NOT default assetPrefix — basePath already namespaces chunks', () => {
    expect('assetPrefix' in createNextConfig(base)).toBe(false);
  });

  it('passes assetPrefix through when the caller supplies one', () => {
    expect(createNextConfig({ ...base, assetPrefix: 'https://cdn.example' }).assetPrefix).toBe('https://cdn.example');
  });

  it('sets outputFileTracingRoot so the standalone entry point is predictable', () => {
    expect(createNextConfig(base).outputFileTracingRoot).toBe('/repo/ts');
  });

  it('transpiles the source-only workspace packages and unions the caller list', () => {
    const cfg = createNextConfig({ ...base, extend: { transpilePackages: ['@acme/x'] } });
    expect(cfg.transpilePackages).toContain('@paigasus/next-config');
    expect(cfg.transpilePackages).toContain('@acme/x');
  });

  it('preserves other extend keys', () => {
    expect(createNextConfig({ ...base, extend: { poweredByHeader: false } }).poweredByHeader).toBe(false);
  });
});
```

- [ ] **Step 10: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm --filter @paigasus/next-config exec vitest run tests/next-config.test.ts; cd ..
```

Expected: FAIL — `createNextConfig` is not exported.

- [ ] **Step 11: Implement `createNextConfig`**

`ts/packages/paigasus-next-config/src/index.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import type { NextConfig } from 'next';
import { canonicalBasePath } from './base-path.js';

export { canonicalBasePath } from './base-path.js';

/**
 * Workspace packages that export TypeScript source rather than built JS. Next must transpile
 * them. Every `@paigasus/*` package is `private: true` with `exports` pointing at `./src/*.ts`;
 * this list grows when a new one lands.
 */
const SOURCE_ONLY_PACKAGES = ['@paigasus/kernel', '@paigasus/next-config', '@paigasus/proto', '@paigasus/sdk', '@paigasus/ui'];

export interface CreateNextConfigOptions {
  /** This app's zone id, e.g. `iam`. `PAIGASUS_ZONE` must equal it at runtime. */
  zone: string;
  /** The zone's path prefix. `'/'` or `''` for a zone mounted at the origin root. */
  basePath: string;
  /** CDN offload only, and deliberately NOT defaulted — see the note below. */
  assetPrefix?: string;
  /** Absolute path Next traces the standalone output from. Pin it; the inferred value moves. */
  outputFileTracingRoot: string;
  /** Any other Next options. `env` is refused. */
  extend?: NextConfig;
}

/**
 * Build one console zone's Next configuration.
 *
 * `output: 'standalone'` is forced LAST and cannot be overridden by `extend`. Without it the
 * image ships all of `node_modules`, which defeats the self-hosting story outright.
 *
 * `assetPrefix` is NOT defaulted to `basePath`. With `basePath: '/iam'` Next already serves
 * chunks under `/iam/_next/`, so the default would buy nothing — and if Next composes the two
 * keys it produces `/iam/iam/_next/…` and every chunk 404s at runtime (spec § 4.4).
 *
 * `env` is refused from `extend` because `env` inlines values at build time exactly as
 * `NEXT_PUBLIC_` does. The factory owns it, and writes only the two values that describe the
 * IMAGE: which zone this image is, and where it is mounted. Everything that varies per
 * DEPLOYMENT belongs in `@paigasus/next-config/runtime`.
 */
export function createNextConfig(options: CreateNextConfigOptions): NextConfig {
  const { zone, basePath, assetPrefix, outputFileTracingRoot, extend } = options;

  if (extend && Object.prototype.hasOwnProperty.call(extend, 'env')) {
    throw new Error(
      'createNextConfig: `extend.env` is not allowed. `env` inlines values at build time, so it is the same single-image hazard as NEXT_PUBLIC_. ' +
        'The factory owns `env`; deployment-varying values belong in @paigasus/next-config/runtime.',
    );
  }
  if (zone.trim() === '') {
    throw new Error('createNextConfig: `zone` must be a non-empty zone id');
  }

  const canonical = canonicalBasePath(basePath, 'createNextConfig: basePath');
  const transpilePackages = [...new Set([...SOURCE_ONLY_PACKAGES, ...(extend?.transpilePackages ?? [])])];

  return {
    ...extend,
    // `exactOptionalPropertyTypes` is on, so an optional key is omitted by conditional spread
    // rather than set to `undefined`.
    ...(canonical === '' ? {} : { basePath: canonical }),
    ...(assetPrefix === undefined ? {} : { assetPrefix }),
    outputFileTracingRoot,
    transpilePackages,
    env: {
      PAIGASUS_COMPILED_ZONE: zone,
      PAIGASUS_COMPILED_BASE_PATH: canonical,
    },
    output: 'standalone',
  };
}
```

- [ ] **Step 12: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm --filter @paigasus/next-config exec vitest run && pnpm --filter @paigasus/next-config run typecheck; cd ..
```

Expected: 18 passed, and `tsc` clean.

- [ ] **Step 13: Format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:fmt ts:lint
git add ts/packages/paigasus-next-config ts/pnpm-workspace.yaml ts/pnpm-lock.yaml
git commit -m "feat(ts): add @paigasus/next-config with the Next config factory" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

If `ts:fmt` reports files it would rewrite, run `cd ts && pnpm exec prettier --write .` and re-stage.

---

## Task 3: The runtime entry

**Files:**
- Create: `ts/packages/paigasus-next-config/src/runtime.ts`
- Create: `ts/packages/paigasus-next-config/tests/runtime.test.ts`
- Modify: `ts/packages/paigasus-next-config/vitest.config.ts` — see Step 0, which is mandatory

**Interfaces:**
- Consumes: `canonicalBasePath` from Task 2.
- Produces, from `@paigasus/next-config/runtime`:
  - `coreEnvShape` — a `ZodRawShape` with `PAIGASUS_ZONE` and `PAIGASUS_ZONES`
  - `defineRuntimeConfig<T extends ZodRawShape>(extraShape?: T): { getRuntimeConfig(): RuntimeConfig<T>; getPublicConfig(): PublicConfig }`
  - `PUBLIC_CONFIG_KEYS: readonly ['zone', 'zones']`
  - `interface PublicConfig { zone: string; zones: Record<string, string> }`

- [ ] **Step 0: Set the `react-server` resolve condition — mandatory, not a contingency**

`src/runtime.ts` opens with `import 'server-only'`. **Measured** on the installed `server-only@0.0.1`: its `exports` map is `{ "react-server": "./empty.js", "default": "./index.js" }`, and `index.js` is nothing but an unconditional `throw new Error("This module cannot be imported from a Client Component module. …")`. Vitest does not set the `react-server` condition, so without this step **every test in this task fails at import time**.

Edit `ts/packages/paigasus-next-config/vitest.config.ts` to read:

```ts
// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts'],
  },
  // `src/runtime.ts` imports `server-only`, whose exports map resolves to a module that throws
  // unconditionally under every condition except `react-server` (measured on server-only@0.0.1:
  // "." -> { "react-server": "./empty.js", "default": "./index.js" }, and index.js is one throw).
  // Vitest does not set that condition, so without this the whole suite fails at import.
  //
  // The remaining three conditions are the additive module-resolution defaults. Listing
  // `react-server` ALONE would drop `import`/`default` and break source-exports `.ts` resolution
  // for the workspace packages — the same trap ts/packages/paigasus-kernel/vitest.config.ts
  // records for its browser project.
  resolve: {
    conditions: ['react-server', 'node', 'import', 'default'],
  },
});
```

Verify the guard still does its real job — it must throw for a client-condition import:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && node --input-type=module -e "import('server-only').then(() => console.log('RESOLVED (wrong)'), (e) => console.log('THREW (correct):', e.message.slice(0, 40)))"; cd ..
```

Expected: `THREW (correct): …`. This proves the condition change makes the module testable without disarming the protection it provides in a real client bundle.

- [ ] **Step 1: Write the failing tests**

`ts/packages/paigasus-next-config/tests/runtime.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { z } from 'zod';
import { defineRuntimeConfig, PUBLIC_CONFIG_KEYS } from '../src/runtime.js';

const ORIGINAL = { ...process.env };

function setEnv(overrides: Record<string, string | undefined>) {
  for (const [key, value] of Object.entries(overrides)) {
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
}

beforeEach(() => {
  for (const key of Object.keys(process.env)) {
    if (key.startsWith('PAIGASUS_') || key === 'NEXT_PHASE' || key === 'SECRET_TOKEN') delete process.env[key];
  }
  setEnv({
    PAIGASUS_ZONE: 'iam',
    PAIGASUS_ZONES: JSON.stringify({ iam: '/iam', gateway: '/gateway' }),
    PAIGASUS_COMPILED_ZONE: 'iam',
    PAIGASUS_COMPILED_BASE_PATH: '/iam',
  });
});

afterEach(() => {
  process.env = { ...ORIGINAL };
});

describe('defineRuntimeConfig', () => {
  it('parses a valid environment', () => {
    const { getRuntimeConfig } = defineRuntimeConfig();
    const cfg = getRuntimeConfig();
    expect(cfg.PAIGASUS_ZONE).toBe('iam');
    expect(cfg.PAIGASUS_ZONES).toEqual({ iam: '/iam', gateway: '/gateway' });
  });

  it('names the offending variable when the environment is invalid', () => {
    setEnv({ PAIGASUS_ZONES: 'not-json' });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/PAIGASUS_ZONES/);
  });

  it('memoizes on first success — a later process.env mutation is not observed', () => {
    const { getRuntimeConfig } = defineRuntimeConfig();
    expect(getRuntimeConfig().PAIGASUS_ZONE).toBe('iam');
    setEnv({ PAIGASUS_ZONE: 'gateway', PAIGASUS_COMPILED_ZONE: 'gateway' });
    expect(getRuntimeConfig().PAIGASUS_ZONE).toBe('iam');
  });

  it('does NOT memoize a failure — a misconfigured container fails every request', () => {
    setEnv({ PAIGASUS_ZONES: 'not-json' });
    const { getRuntimeConfig } = defineRuntimeConfig();
    expect(() => getRuntimeConfig()).toThrow();
    expect(() => getRuntimeConfig()).toThrow();
  });

  it('throws during next build, so a module-scope read cannot bake in a value', () => {
    setEnv({ NEXT_PHASE: 'phase-production-build' });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/next build/);
  });

  it('fails CLOSED when the compiled zone is absent', () => {
    setEnv({ PAIGASUS_COMPILED_ZONE: undefined });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/PAIGASUS_COMPILED_ZONE/);
  });

  it('fails closed when the compiled base path is absent', () => {
    setEnv({ PAIGASUS_COMPILED_BASE_PATH: undefined });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/PAIGASUS_COMPILED_BASE_PATH/);
  });

  it('detects the image deployed as the wrong zone, naming both values', () => {
    setEnv({ PAIGASUS_ZONE: 'gateway', PAIGASUS_ZONES: JSON.stringify({ gateway: '/gateway' }) });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/iam[\s\S]*gateway|gateway[\s\S]*iam/);
  });

  it('detects an ingress remap, naming both base paths', () => {
    setEnv({ PAIGASUS_ZONES: JSON.stringify({ iam: '/admin/iam' }) });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/\/admin\/iam/);
  });

  it('treats a trailing slash in the operator JSON as equivalent', () => {
    setEnv({ PAIGASUS_ZONES: JSON.stringify({ iam: '/iam/' }) });
    expect(defineRuntimeConfig().getRuntimeConfig().PAIGASUS_ZONES.iam).toBe('/iam');
  });

  it('accepts a zone mounted at the origin root', () => {
    setEnv({ PAIGASUS_ZONES: JSON.stringify({ iam: '/' }), PAIGASUS_COMPILED_BASE_PATH: '' });
    expect(defineRuntimeConfig().getRuntimeConfig().PAIGASUS_ZONES.iam).toBe('');
  });

  it('throws when the zone map has no entry for this app zone', () => {
    setEnv({ PAIGASUS_ZONES: JSON.stringify({ gateway: '/gateway' }) });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/no entry/i);
  });

  it('rejects a non-path value inside the zone map at parse time', () => {
    setEnv({ PAIGASUS_ZONES: JSON.stringify({ iam: 'https://idp.example/secret' }) });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/PAIGASUS_ZONES/);
  });

  it('throws when an extra shape declares a key this package owns', () => {
    expect(() => defineRuntimeConfig({ PAIGASUS_ZONE: z.string() })).toThrow(/PAIGASUS_ZONE/);
  });

  it('validates an extra shape alongside the core one', () => {
    setEnv({ SECRET_TOKEN: 'sk-live-abc123' });
    const cfg = defineRuntimeConfig({ SECRET_TOKEN: z.string().min(1) }).getRuntimeConfig();
    expect(cfg.SECRET_TOKEN).toBe('sk-live-abc123');
  });

  it('keeps an extra-shape secret out of the client slice', () => {
    setEnv({ SECRET_TOKEN: 'sk-live-abc123' });
    const { getPublicConfig } = defineRuntimeConfig({ SECRET_TOKEN: z.string().min(1) });
    expect(JSON.stringify(getPublicConfig())).not.toContain('sk-live-abc123');
  });

  it('projects exactly the pinned public keys', () => {
    expect(Object.keys(defineRuntimeConfig().getPublicConfig()).sort()).toEqual([...PUBLIC_CONFIG_KEYS].sort());
  });

  it('pins the public key list by strict equality, so widening the slice is deliberate', () => {
    expect(PUBLIC_CONFIG_KEYS).toEqual(['zone', 'zones']);
  });

  it('never puts an extra-shape value into an error message', () => {
    setEnv({ SECRET_TOKEN: 'sk-live-abc123', PAIGASUS_ZONE: '' });
    const { getRuntimeConfig } = defineRuntimeConfig({ SECRET_TOKEN: z.string().min(1) });
    try {
      getRuntimeConfig();
      throw new Error('expected a validation failure');
    } catch (err) {
      expect(String(err)).not.toContain('sk-live-abc123');
      expect(String(err)).toContain('PAIGASUS_ZONE');
    }
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm --filter @paigasus/next-config exec vitest run tests/runtime.test.ts; cd ..
```

Expected: FAIL — `../src/runtime.js` does not resolve.

- [ ] **Step 3: Implement the runtime entry**

`ts/packages/paigasus-next-config/src/runtime.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Deployment configuration, read and validated at FIRST REQUEST — never at module scope.
//
// `server-only` is the structural guard. In the edge runtime Next does not provide a dynamic
// `process.env`: values must be known at build time, so an edge route or middleware importing
// this module would read inlined or undefined values rather than deployment values. That is the
// same silent-wrong-value class the compiled-versus-deployed cross-check below exists to catch,
// arriving through a different door (spec § 5). This module is Node-runtime only.
import 'server-only';
import { z, type ZodRawShape } from 'zod';
import { canonicalBasePath } from './base-path.js';

/** Keys this package owns. An extra shape declaring one of them is a hard error. */
const OWNED_KEYS = ['PAIGASUS_ZONE', 'PAIGASUS_ZONES'] as const;

/** The client-safe projection's key set, pinned by a strict-equality test. */
export const PUBLIC_CONFIG_KEYS = ['zone', 'zones'] as const;

export interface PublicConfig {
  zone: string;
  zones: Record<string, string>;
}

/**
 * `PAIGASUS_ZONES` arrives as a JSON string. Each value is validated as a canonical base path,
 * which is what closes the one hole in "zod strips unknown keys": zod constrains an OBJECT's key
 * set, never a RECORD's, so without this a secret pasted into the operator's JSON would ride
 * through the public projection inside an allowed key (spec § 5.4).
 *
 * Issue messages name the zone id but never the value. A zone id is a public routing label; a
 * value might be a mis-pasted secret.
 */
const zoneMapFromJson = z.string().transform((raw, ctx): Record<string, string> => {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    ctx.addIssue({ code: 'custom', message: 'is not valid JSON' });
    return z.NEVER;
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
    ctx.addIssue({ code: 'custom', message: 'must be a JSON object mapping zone id to base path' });
    return z.NEVER;
  }
  const out: Record<string, string> = {};
  for (const [id, value] of Object.entries(parsed as Record<string, unknown>)) {
    if (typeof value !== 'string') {
      ctx.addIssue({ code: 'custom', message: `entry ${JSON.stringify(id)} must be a string` });
      return z.NEVER;
    }
    try {
      out[id] = canonicalBasePath(value, `entry ${JSON.stringify(id)}`);
    } catch {
      ctx.addIssue({ code: 'custom', message: `entry ${JSON.stringify(id)} is not a valid base path` });
      return z.NEVER;
    }
  }
  return out;
});

/** The variables this package owns. Neither has a default — a default hides a misconfiguration. */
export const coreEnvShape = {
  PAIGASUS_ZONE: z.string().min(1),
  PAIGASUS_ZONES: zoneMapFromJson,
};

/**
 * Format a validation failure without ever rendering an input value.
 *
 * Custom issues carry messages authored in this file, which are provably value-free. Built-in
 * issues are rendered as their `code` alone rather than their message, because a future zod
 * version could start echoing the input into a built-in message and nothing would notice.
 */
function describeIssues(error: z.ZodError): string {
  const parts = error.issues.map((issue) => {
    const where = issue.path.length > 0 ? issue.path.join('.') : '(root)';
    return issue.code === 'custom' ? `${where}: ${issue.message}` : `${where}: ${issue.code}`;
  });
  return [...new Set(parts)].join('; ');
}

/**
 * Compare what the IMAGE was built as against what the DEPLOYMENT declares.
 *
 * Fail-closed by construction: an absent compiled value throws rather than skipping the check. A
 * check that silently does nothing is worse than no check, because the design document then
 * records that the condition is detected.
 *
 * Zone ids and base paths ARE the public slice, so naming them in an error leaks nothing.
 */
function assertCompiledAgreement(zone: string, zones: Record<string, string>): void {
  const compiledZone = process.env.PAIGASUS_COMPILED_ZONE;
  const compiledBasePath = process.env.PAIGASUS_COMPILED_BASE_PATH;

  if (compiledZone === undefined || compiledZone === '') {
    throw new Error(
      'PAIGASUS_COMPILED_ZONE is absent. createNextConfig() writes it, so this image was not built with the factory. ' +
        'Failing closed rather than skipping the zone cross-check.',
    );
  }
  if (compiledBasePath === undefined) {
    throw new Error(
      'PAIGASUS_COMPILED_BASE_PATH is absent. createNextConfig() writes it, so this image was not built with the factory. ' +
        'Failing closed rather than skipping the base-path cross-check.',
    );
  }
  if (zone !== compiledZone) {
    throw new Error(`Zone mismatch: this image was built as zone "${compiledZone}" but PAIGASUS_ZONE is "${zone}". Deploy the right image, or fix PAIGASUS_ZONE.`);
  }

  const declared = zones[zone];
  if (declared === undefined) {
    throw new Error(`PAIGASUS_ZONES has no entry for this app's own zone "${zone}". Every deployed zone must appear in the map.`);
  }

  const compiledCanonical = canonicalBasePath(compiledBasePath, 'PAIGASUS_COMPILED_BASE_PATH');
  if (declared !== compiledCanonical) {
    throw new Error(
      `Base path mismatch for zone "${zone}": the image serves "${compiledCanonical}" but PAIGASUS_ZONES declares "${declared}". ` +
        'Next has no runtime basePath, so rebuild the image at the new prefix or fix the ingress.',
    );
  }
}

/**
 * Build this app's single runtime-configuration accessor.
 *
 * Call it ONCE per app. `@paigasus/auth` and `@paigasus/sdk` each export a zod shape for the
 * variables they own; the app composes them here. One schema, one parse, one place — a registry
 * would let two packages parse the same environment twice and disagree.
 */
export function defineRuntimeConfig<T extends ZodRawShape>(extraShape?: T) {
  const extra = (extraShape ?? {}) as T;
  for (const key of OWNED_KEYS) {
    if (Object.prototype.hasOwnProperty.call(extra, key)) {
      throw new Error(`defineRuntimeConfig: extraShape must not declare ${key} — @paigasus/next-config owns it.`);
    }
  }

  const schema = z.object({ ...coreEnvShape, ...extra });
  type Parsed = z.infer<typeof schema>;
  let cached: Parsed | undefined;

  function getRuntimeConfig(): Parsed {
    // A module-scope read from a prerendered page would bake the BUILDER's values into the image
    // and hand them to every self-hoster. Failing the build with an actionable message is the
    // only outcome that cannot ship silently.
    if (process.env.NEXT_PHASE === 'phase-production-build') {
      throw new Error(
        'getRuntimeConfig() was called during `next build`. Deployment configuration must be read at request time, not module scope — ' +
          'a build-time read bakes the builder\'s values into the image. Move the call inside a request-scoped function, or `await connection()` from `next/server` first.',
      );
    }
    // Success is memoized; failure is not. A misconfigured container must fail EVERY request,
    // not just the first.
    if (cached !== undefined) {
      return cached;
    }
    const result = schema.safeParse(process.env);
    if (!result.success) {
      throw new Error(`Invalid runtime configuration — ${describeIssues(result.error)}`);
    }
    const parsed = result.data as Parsed & { PAIGASUS_ZONE: string; PAIGASUS_ZONES: Record<string, string> };
    assertCompiledAgreement(parsed.PAIGASUS_ZONE, parsed.PAIGASUS_ZONES);
    cached = parsed;
    return cached;
  }

  /**
   * The client-safe slice. An ALLOWLIST BY CONSTRUCTION: the projection names two fields, so a
   * secret added to an extra shape cannot reach the browser unless someone edits this function
   * and `PUBLIC_CONFIG_KEYS` together — and a strict-equality test on that constant makes the
   * edit deliberate.
   *
   * Internal service URLs are cluster-internal DNS and must never reach the browser.
   *
   * TRANSPORT: pass the result as a PROP from a server component. This package ships no inline
   * `<script>` serialization; that path is a `</script>`-injection hazard when the JSON is not
   * escaped, and whichever issue first needs it owns the escaping rule (spec § 5.4).
   */
  function getPublicConfig(): PublicConfig {
    const full = getRuntimeConfig() as Parsed & { PAIGASUS_ZONE: string; PAIGASUS_ZONES: Record<string, string> };
    return { zone: full.PAIGASUS_ZONE, zones: full.PAIGASUS_ZONES };
  }

  return { getRuntimeConfig, getPublicConfig };
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm --filter @paigasus/next-config exec vitest run tests/runtime.test.ts; cd ..
```

Expected: 19 passed.

If you see "This module cannot be imported from a Client Component", Step 0 was skipped or its `resolve.conditions` list is wrong. Fix Step 0 — do **not** remove `import 'server-only'` from `src/runtime.ts` to make the suite pass. That line is the structural guard that keeps this module out of a client or edge bundle, where `process.env` is not dynamic and the whole runtime-config contract silently breaks.

- [ ] **Step 5: Typecheck and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm --filter @paigasus/next-config run typecheck; cd ..
moon run ts:fmt ts:lint
git add ts/packages/paigasus-next-config
git commit -m "feat(ts): add request-time runtime config to @paigasus/next-config" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 4: The eslint boundary preset, its wiring, and the guard-the-guard test

**Files:**
- Create: `ts/packages/paigasus-next-config/src/eslint.mjs`
- Create: `ts/packages/paigasus-next-config/tests/boundaries.test.ts`
- Modify: `ts/eslint.config.js`
- Modify: `ts/packages/paigasus-next-config/package.json` (already exports `./eslint` from Task 2)

**Interfaces:**
- Produces: `boundaryRules` (a named export, also the default) from `@paigasus/next-config/eslint` — an ESLint flat-config array whose entries each carry `files` and a `no-restricted-imports` rule.

- [ ] **Step 1: Write the boundary preset**

`ts/packages/paigasus-next-config/src/eslint.mjs`:

```js
// SPDX-License-Identifier: Apache-2.0
//
// Package-boundary rules for the ts workspace (Frontend Architecture Scoping § 6, SMA-502).
//
//   apps → app-shell → { ui, auth/client }
//   apps → { sdk, auth/server, next-config }
//   sdk  → proto
//   ui   → React only
//
// DELIBERATE DEVIATION: an app may import @paigasus/ui directly. The diagram shows layering, not
// exclusivity, and forcing every component through an app-shell re-export barrel buys no safety.
// The four rules below are the ones that carry real safety.
//
// EVERY GROUP CARRIES BOTH FORMS. `no-restricted-imports` matches `patterns[].group` with
// gitignore-style globs, where `*` does not cross `/`. So '@paigasus/*' alone matches
// '@paigasus/sdk' and NOT '@paigasus/sdk/client' — which would silently permit every subpath
// import these rules exist to ban. Negations are doubled for the same reason.
//
// Type imports are banned alongside value imports: an `import type` of @paigasus/sdk from
// @paigasus/ui still couples the packages. That is the core rule's default behaviour, which is
// why @typescript-eslint/no-restricted-imports (whose added feature here is `allowTypeImports`)
// is not used.
//
// Written as plain ESM rather than TypeScript: ts/eslint.config.js is loaded by ESLint's own
// resolver, and configuration data gains little from types (spec § 13 M5).
//
// The app-shell and auth entries are INERT until SMA-506 and SMA-508 land. They are written now,
// tested against synthetic paths, and covered by a liveness assertion so a package landing under
// a different directory name reds instead of silently disabling its rule.

/**
 * Package directories these rules expect, mapped to a status string. `'exists'` means the
 * directory is on disk today; anything else is the stated reason it is not, which the liveness
 * test requires so an inert rule cannot go unnoticed.
 *
 * @type {Record<string, string>}
 */
export const BOUNDARY_SCOPES = {
  'packages/paigasus-ui': 'exists',
  'packages/paigasus-sdk': 'exists',
  'packages/paigasus-app-shell': 'SMA-506 has not landed yet; the rule is inert until it does',
  apps: 'exists',
};

const restrict = (patterns) => ({ 'no-restricted-imports': ['error', { patterns }] });

/**
 * The boundary blocks, as an ESLint flat-config array.
 *
 * The `@type` annotation is load-bearing for the consumer, not decoration: `tests/boundaries.test.ts`
 * is TypeScript with `noUncheckedIndexedAccess` and the typed-ESLint `no-unsafe-*` rules on, and an
 * unannotated export from a `.mjs` gives it loosely-inferred types that trip those rules. Fix the
 * typing here rather than adding an eslint-disable in the one test that proves these rules are wired.
 *
 * @type {Array<{ name: string, files: string[], rules: Record<string, unknown> }>}
 */
export const boundaryRules = [
  {
    name: 'paigasus/boundaries/ui',
    files: ['packages/paigasus-ui/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    rules: restrict([
      {
        group: ['next', 'next/*', 'next/**'],
        message: '@paigasus/ui is plain React and must not import next/* — it has to test in jsdom with no Next runtime and stay usable from the docs app (§ 6 rule 1). Inject navigation instead of reaching for next/link.',
      },
      {
        group: ['@paigasus/*', '@paigasus/*/**'],
        message: '@paigasus/ui depends on React only. Nothing in the workspace may be imported here (§ 6 rule 1).',
      },
    ]),
  },
  {
    name: 'paigasus/boundaries/sdk',
    files: ['packages/paigasus-sdk/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    rules: restrict([
      {
        group: ['@paigasus/*', '@paigasus/*/**', '!@paigasus/proto', '!@paigasus/proto/**'],
        message: '@paigasus/sdk depends on @paigasus/proto only (§ 6: sdk → proto).',
      },
      {
        group: ['react', 'react-dom', 'react-dom/*', 'next', 'next/*', 'next/**'],
        message: '@paigasus/sdk is server-only. It attaches bearer tokens, so it must never be reachable from a client bundle (§ 6 rule 2).',
      },
    ]),
  },
  {
    name: 'paigasus/boundaries/app-shell',
    files: ['packages/paigasus-app-shell/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    rules: restrict([
      {
        group: ['@paigasus/sdk', '@paigasus/sdk/**', '@paigasus/auth/server', '@paigasus/auth/server/**'],
        message: '@paigasus/app-shell is client-reachable. It may use @paigasus/auth/client, never /server, and never the sdk (§ 6 rule 3).',
      },
    ]),
  },
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
];

export default boundaryRules;
```

- [ ] **Step 2: Write the failing boundary tests**

`ts/packages/paigasus-next-config/tests/boundaries.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { ESLint } from 'eslint';
import { describe, expect, it } from 'vitest';
import { BOUNDARY_SCOPES, boundaryRules } from '../src/eslint.mjs';

/** The ts workspace root — `files` globs in the preset are relative to it. */
const TS_ROOT = fileURLToPath(new URL('../../..', import.meta.url));

async function restrictedImportsFor(filePath: string, source: string): Promise<string[]> {
  const eslint = new ESLint({ cwd: TS_ROOT, overrideConfigFile: true, overrideConfig: boundaryRules });
  const [result] = await eslint.lintText(source, { filePath, warnIgnored: false });
  return (result?.messages ?? []).filter((m) => m.ruleId === 'no-restricted-imports').map((m) => m.message);
}

const DENIED: ReadonlyArray<readonly [string, string, string]> = [
  ['ui must not import next', 'packages/paigasus-ui/src/button.tsx', "import Link from 'next/link';"],
  ['ui must not import a workspace package', 'packages/paigasus-ui/src/button.tsx', "import { x } from '@paigasus/sdk';"],
  ['ui must not import a workspace SUBPATH', 'packages/paigasus-ui/src/button.tsx', "import { x } from '@paigasus/sdk/client';"],
  ['sdk must not import ui', 'packages/paigasus-sdk/src/iam.ts', "import { x } from '@paigasus/ui';"],
  ['sdk must not import a ui SUBPATH', 'packages/paigasus-sdk/src/iam.ts', "import { x } from '@paigasus/ui/button';"],
  ['sdk must not import react', 'packages/paigasus-sdk/src/iam.ts', "import { useState } from 'react';"],
  ['app-shell must not import the sdk', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/sdk';"],
  ['app-shell must not import auth/server', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/auth/server';"],
  ['apps must not import proto', 'apps/paigasus-console/app/page.tsx', "import { x } from '@paigasus/proto';"],
  ['apps must not import a proto SUBPATH', 'apps/paigasus-console/app/page.tsx', "import { x } from '@paigasus/proto/gen/iam';"],
];

const ALLOWED: ReadonlyArray<readonly [string, string, string]> = [
  ['ui may import react', 'packages/paigasus-ui/src/button.tsx', "import { useState } from 'react';"],
  ['sdk may import proto', 'packages/paigasus-sdk/src/iam.ts', "import { x } from '@paigasus/proto';"],
  ['sdk may import a proto SUBPATH', 'packages/paigasus-sdk/src/iam.ts', "import { x } from '@paigasus/proto/gen/iam';"],
  ['app-shell may import auth/client', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/auth/client';"],
  ['app-shell may import ui', 'packages/paigasus-app-shell/src/header.tsx', "import { x } from '@paigasus/ui';"],
  ['apps may import the sdk', 'apps/paigasus-console/app/page.tsx', "import { x } from '@paigasus/sdk';"],
  ['apps may import ui directly — the deliberate § 7.3 deviation', 'apps/paigasus-console/app/page.tsx', "import { x } from '@paigasus/ui';"],
  ['apps may import next', 'apps/paigasus-console/app/page.tsx', "import Link from 'next/link';"],
];

describe('boundary preset', () => {
  it.each(DENIED)('reports: %s', async (_label, filePath, source) => {
    expect(await restrictedImportsFor(filePath, source)).not.toHaveLength(0);
  });

  it.each(ALLOWED)('permits: %s', async (_label, filePath, source) => {
    expect(await restrictedImportsFor(filePath, source)).toHaveLength(0);
  });

  it('every scope either exists on disk or states why it does not yet', () => {
    for (const [dir, note] of Object.entries(BOUNDARY_SCOPES)) {
      const present = existsSync(new URL(`../../../${dir}`, import.meta.url));
      if (!present) {
        expect(note, `${dir} is absent, so BOUNDARY_SCOPES must state why`).not.toBe('exists');
        expect(note.trim().length, `${dir} needs a real reason, not a blank one`).toBeGreaterThan(0);
      }
    }
  });

  it('every rule scope has a matching entry in the preset', () => {
    // Indexed through a typed local rather than a chained member access: `noUncheckedIndexedAccess`
    // makes `entry.files[0]` `string | undefined`, and chaining `.split()` straight off it is both
    // a type error and an unsafe-member-access finding under the typed ESLint rules.
    const scoped = boundaryRules.map((entry) => {
      const first: string | undefined = entry.files[0];
      expect(first, `${entry.name} declares no files glob`).toBeDefined();
      return (first ?? '').split('/**')[0];
    });
    for (const dir of Object.keys(BOUNDARY_SCOPES)) {
      expect(scoped).toContain(dir);
    }
  });
});
```

- [ ] **Step 3: Write the failing guard-the-guard test**

Append to `ts/packages/paigasus-next-config/tests/boundaries.test.ts`:

```ts
describe('the workspace eslint config actually applies the preset', () => {
  it('carries every boundary entry in its EXPORTED array, not merely as an import', async () => {
    // Importing the real config is what makes this an assertion rather than a text scan: a dead
    // `import` statement satisfies a grep, and deleting only the spread would leave every test
    // above green while no app is actually governed by these rules.
    const shipped = (await import('../../../eslint.config.js')).default as Array<{ files?: string[] }>;
    for (const entry of boundaryRules) {
      expect(shipped, `ts/eslint.config.js dropped the ${entry.name} boundary block`).toContainEqual(expect.objectContaining({ files: entry.files }));
    }
  });
});
```

- [ ] **Step 4: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm --filter @paigasus/next-config exec vitest run tests/boundaries.test.ts; cd ..
```

Expected: the DENIED rows FAIL (no rules are wired into `ts/eslint.config.js` yet, but the preset tests use `overrideConfig` so they should PASS once Step 1 landed) and the guard-the-guard test FAILS with "dropped the paigasus/boundaries/ui boundary block".

**If a DENIED row unexpectedly passes with zero messages**, the glob semantics differ from the assumption — re-read the `no-restricted-imports` docs for the installed ESLint version before changing the test. Do not weaken the assertion to make it pass.

- [ ] **Step 5: Wire the preset into the workspace config**

In `ts/eslint.config.js`, add the import after the existing plugin imports:

```js
import { boundaryRules } from '@paigasus/next-config/eslint';
```

and add, as the LAST element of the `tseslint.config(...)` argument list (after the Next.js block):

```js
  // Package dependency direction (Frontend Architecture Scoping § 6, SMA-502). The rules live in
  // @paigasus/next-config/eslint so they ship with the package that owns the boundary, and are
  // unit-tested there against synthetic paths — including the app-shell and auth scopes, which do
  // not exist on disk yet. paigasus-next-config-ts:test asserts this spread is still here, and
  // that package's moon.yml lists /ts/eslint.config.js among its test inputs so the assertion is
  // reachable on the PR that removes it.
  ...boundaryRules,
```

Add the dependency:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm add -D -w '@paigasus/next-config@workspace:*' && cd ..
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm --filter @paigasus/next-config exec vitest run; cd ..
moon run ts:lint
```

Expected: all tests pass, and `ts:lint` stays green — no existing file violates a boundary. If `ts:lint` reports a violation in existing code, that is a real finding: fix the import, do not relax the rule.

- [ ] **Step 7: Prove the guard bites (mutation check)**

```bash
python3 - <<'PY'
import pathlib
p = pathlib.Path('ts/eslint.config.js')
s = p.read_text()
p.with_suffix('.js.mutation-backup').write_text(s)
p.write_text(s.replace('  ...boundaryRules,\n', ''))
PY
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm --filter @paigasus/next-config exec vitest run tests/boundaries.test.ts; echo "rc=$?"; cd ..
```

Expected: rc non-zero, failing on "dropped the paigasus/boundaries/ui boundary block".

Restore by moving the backup back — **not** with `git checkout`, which would also discard uncommitted work in this task:

```bash
mv ts/eslint.config.js.mutation-backup ts/eslint.config.js
touch ts/eslint.config.js
cd ts && pnpm --filter @paigasus/next-config exec vitest run tests/boundaries.test.ts; cd ..
```

Expected: passing again. The `touch` matters — a restored file with an older mtime can serve a stale cached result.

- [ ] **Step 8: Format and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:fmt
git add ts/packages/paigasus-next-config ts/eslint.config.js ts/package.json ts/pnpm-lock.yaml
git commit -m "feat(ts): add the package-boundary eslint preset" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 5: The tsconfig preset and the console's adoption

**Files:**
- Create: `ts/packages/paigasus-next-config/tsconfig.app.json`
- Create: `ts/apps/paigasus-console/app/runtime-config.ts`
- Create: `ts/apps/paigasus-console/.env.local.example`
- Modify: `ts/apps/paigasus-console/{next.config.ts,package.json,tsconfig.json,moon.yml}`
- Modify: `ts/apps/paigasus-console/app/page.tsx`

**Interfaces:**
- Consumes: `createNextConfig` (Task 2), `defineRuntimeConfig` (Task 3), `STANDALONE_ENTRY` and `MOON_NEGATED_GLOBS` (Task 1).
- Produces: `getRuntimeConfig` and `getPublicConfig` from `app/runtime-config.ts`, used by Task 6's smoke.

- [ ] **Step 1: Write the shared Next-app tsconfig preset**

`ts/packages/paigasus-next-config/tsconfig.app.json`:

```json
{
  "_comment": "Shared Next-app TypeScript preset (SMA-502). It extends the workspace base by relative path: pnpm symlinks this package into node_modules, and `extends` resolves through the symlink's real path, so ../../tsconfig.base.json lands on ts/tsconfig.base.json. An app extends '@paigasus/next-config/tsconfig-app' and adds only what is app-specific.",
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "lib": ["DOM", "DOM.Iterable", "ES2022"],
    "jsx": "preserve",
    "incremental": true,
    "plugins": [{ "name": "next" }],
    "noEmit": true
  }
}
```

- [ ] **Step 2: Point the console at the preset and verify the resolution works**

`ts/apps/paigasus-console/tsconfig.json`:

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

Add the dependency:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm --filter @paigasus/console add '@paigasus/next-config@workspace:*' && cd ..
cd ts && pnpm --filter @paigasus/console run typecheck; echo "rc=$?"; cd ..
```

Expected: rc 0. If `tsc` cannot resolve the `extends` through the package `exports`, fall back to `"extends": "../../packages/paigasus-next-config/tsconfig.app.json"` and record the reason in a `_comment` key — the preset still lives in one place, which is the point.

- [ ] **Step 3: Write the console's single runtime-config module**

`ts/apps/paigasus-console/app/runtime-config.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// This app's ONE call to defineRuntimeConfig. When @paigasus/auth (SMA-506) and @paigasus/sdk
// (SMA-508) land, each exports a zod shape for the variables it owns and they are composed here —
// one schema, one parse, one place.
import { defineRuntimeConfig } from '@paigasus/next-config/runtime';

export const { getRuntimeConfig, getPublicConfig } = defineRuntimeConfig();
```

- [ ] **Step 4: Point `next.config.ts` at the factory**

`ts/apps/paigasus-console/next.config.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { createNextConfig } from '@paigasus/next-config';

// The IAM zone. `zone` and `basePath` describe the IMAGE, not the deployment: this image IS the
// IAM zone, and Next 16 has no runtime basePath, so the prefix is necessarily compiled in. Every
// deployment-varying value is read at request time instead (spec § 2, § 4.1).
//
// @paigasus/next-config/runtime cross-checks these two values against PAIGASUS_ZONE and
// PAIGASUS_ZONES at first request and fails closed if they disagree, so an ingress remap surfaces
// as a startup error rather than an intermittently broken nav item.
export default createNextConfig({
  zone: 'iam',
  basePath: '/iam',
  // ts/ — the pnpm workspace root. Next infers this from the nearest lockfile, and the inferred
  // value decides where the standalone entry point lands, so it is pinned rather than left to
  // change under a lockfile move.
  outputFileTracingRoot: fileURLToPath(new URL('../..', import.meta.url)),
});
```

- [ ] **Step 5: Make the page read config at request time**

`ts/apps/paigasus-console/app/page.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
import { connection } from 'next/server';
import { getRuntimeConfig } from './runtime-config';

// `await connection()` opts this page out of prerendering. Without it, Next would evaluate the
// page during `next build`, getRuntimeConfig() would throw, and the build would fail — which is
// the guard working as designed. This is the pattern every config-reading page follows.
export default async function Page() {
  await connection();
  const config = getRuntimeConfig();
  return (
    <main>
      <h1>Paigasus console</h1>
      <p data-testid="zone">{config.PAIGASUS_ZONE}</p>
      <pre data-testid="zones">{JSON.stringify(config.PAIGASUS_ZONES)}</pre>
    </main>
  );
}
```

- [ ] **Step 6: Document the two variables**

`ts/apps/paigasus-console/.env.local.example`:

```bash
# SPDX-License-Identifier: Apache-2.0
#
# Copy to .env.local for `next dev`. Neither variable has a default: a default would hide a
# misconfiguration, and these two are exactly what makes one image serve many deployments.
#
# In a real deployment Helm renders PAIGASUS_ZONES from the SAME values block as the ingress
# rules, so the routing and the console's zone map cannot drift (ADR-0017).

# This app's zone id. Must equal the `zone` passed to createNextConfig() in next.config.ts.
PAIGASUS_ZONE=iam

# Every deployed zone, mapped to its path prefix behind the single origin. The entry for
# PAIGASUS_ZONE must equal this image's compiled basePath, or the first request fails loudly.
PAIGASUS_ZONES={"iam":"/iam","gateway":"/gateway"}
```

- [ ] **Step 7: Update the console's Moon task**

Replace the `tasks:` block in `ts/apps/paigasus-console/moon.yml`:

```yaml
tasks:
  build:
    # `set -euo pipefail` is REQUIRED. Moon does not enable errexit for `script:` blocks and takes
    # a block's status from its LAST command, so without it a failed `next build` followed by a
    # satisfied file check exits 0 — a false green on this issue's headline acceptance criterion.
    #
    # The assertion is what proves AC1. A unit test on the object createNextConfig() returns shows
    # `output: 'standalone'` is SET; it cannot show Next HONOURED it. A Next version that renames
    # or ignores the key would pass that unit test and ship an image carrying all of node_modules.
    #
    # The asserted path is measured, not assumed: with outputFileTracingRoot pinned to ts/, Next
    # writes the entry point under the app's own subpath. See
    # docs/superpowers/specs/2026-09-08-sma-502-measurements.md.
    script: |
      set -euo pipefail
      pnpm exec next build
      if [ ! -f .next/standalone/apps/paigasus-console/server.js ]; then
        echo "paigasus-console: standalone entry point missing — output: 'standalone' did not take effect" >&2
        exit 1
      fi
    inputs:
      # `@group(sources)` resolves to src/**/* for an application-layer project, and this app has
      # NO src/ — its pages live in app/. So the inherited group matches nothing and the group
      # alone would never re-key this task. `app/**/*` is what actually confers affectedness.
      # repo:next-env-drift already works around the same hole from its own side (moon.yml).
      - '@group(sources)'
      - 'app/**/*'
      - 'tsconfig.json'
      - 'package.json'
      - 'next.config.ts'
      # next.config.ts now imports the factory, so this task READS the package's sources. Without
      # this input, editing the factory serves a cached build — the staleness class SMA-519 paid
      # for. ts/pnpm-lock.yaml is deliberately NOT added: repo:next-env-drift keys on the lockfile
      # precisely because this task does not, and its comment records why.
      - '/ts/packages/paigasus-next-config/src/**/*'
    outputs:
      # AC4 — the cache archive must not carry .next/cache, which grows without bound.
      - '.next/**/*'
      - '!.next/cache/**/*'
    options:
      merge: replace
```

**If Task 1 recorded `MOON_NEGATED_GLOBS=false`**, replace the `outputs` list with the explicit kept subpaths Task 1's Step 7 enumerated, and add a comment naming the measurement.

**If Task 1 recorded a different `STANDALONE_ENTRY`**, use that path in the `if [ ! -f … ]` test.

- [ ] **Step 8: Verify the build and the assertion**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/paigasus-console && cp .env.local.example .env.local && cd ../../..
moon run paigasus-console-ts:build --force
```

Expected: PASS. Confirm the cache archive excludes the cache dir:

```bash
ls ts/apps/paigasus-console/.next/cache >/dev/null && echo "cache dir exists on disk (expected)"
```

- [ ] **Step 9: Prove the assertion bites**

```bash
rm -f ts/apps/paigasus-console/.next/standalone/apps/paigasus-console/server.js
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
```

Temporarily change `next.config.ts`'s export to a plain object without `output`, then:

```bash
moon run paigasus-console-ts:build --force; echo "rc=$?"
```

Expected: rc non-zero, with `standalone entry point missing` in the output. Restore `next.config.ts` from Step 4 by re-writing it (not by `git checkout`, which would discard this task's other edits), then re-run to confirm PASS.

- [ ] **Step 10: Confirm the build-phase guard fires for real**

Temporarily remove `await connection();` from `app/page.tsx` and move `getRuntimeConfig()` to module scope:

```tsx
const config = getRuntimeConfig();
export default function Page() { return <p>{config.PAIGASUS_ZONE}</p>; }
```

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-console-ts:build --force 2>&1 | grep -c 'called during `next build`'; echo "rc=$?"
```

Expected: the build FAILS with the guard's message. This is the end-to-end proof of spec § 5.3 that a unit test cannot give. Restore `app/page.tsx` from Step 5 by re-writing it.

**If the build SUCCEEDS**, Task 1's `NEXT_PHASE_REACHES_WORKERS` was `false`: record it in `ci/next-public/README.md`'s Limitations (Task 7) as an uncovered residual, and keep `await connection()` as the documented requirement.

- [ ] **Step 11: Format, lint, commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
rm -f ts/apps/paigasus-console/.env.local
moon run ts:fmt ts:lint
moon run paigasus-console-ts:build repo:next-env-drift
git add ts/apps/paigasus-console ts/packages/paigasus-next-config ts/pnpm-lock.yaml
git commit -m "feat(ts): adopt the next-config factory in the console app" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

`repo:next-env-drift` keys on `next.config.ts` and `tsconfig.json`, both changed here, so it will run. If it reds, commit the regenerated `next-env.d.ts` it produces.

---

## Task 6: The standalone runtime smoke

The artifact check in Task 5 proves the file exists. It does not prove the server reads configuration at runtime, which is what AC1 actually claims. The two failure modes most likely to ship — Next inlining a value, and the standalone bundle omitting the transpiled package — are invisible to both the artifact check and every unit test.

**Files:**
- Create: `ts/apps/paigasus-console/tests/standalone-runtime.test.ts`
- Create: `ts/apps/paigasus-console/vitest.config.ts`
- Modify: `ts/apps/paigasus-console/{package.json,moon.yml}`

**Interfaces:**
- Consumes: the built standalone server from Task 5, and `STANDALONE_ENTRY` from Task 1.

- [ ] **Step 1: Add vitest to the console app**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm --filter @paigasus/console add -D vitest@catalog: && cd ..
```

`ts/apps/paigasus-console/vitest.config.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts'],
    // Each case boots a real Next standalone server twice; the default 5s is not enough.
    testTimeout: 120_000,
    hookTimeout: 120_000,
  },
});
```

- [ ] **Step 2: Write the failing smoke test**

`ts/apps/paigasus-console/tests/standalone-runtime.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// AC1's second half. Task 5's build asserts the standalone ARTIFACT exists; this asserts the
// SERVER reads deployment configuration at runtime. Two boots of the SAME binary with different
// PAIGASUS_ZONES must produce different responses. If Next had inlined the value, or the
// standalone trace had dropped @paigasus/next-config, both boots would agree — and every unit
// test in this repo would still pass.
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:net';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const SERVER_ENTRY = fileURLToPath(new URL('../.next/standalone/apps/paigasus-console/server.js', import.meta.url));

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

async function fetchHomeWith(zones: Record<string, string>): Promise<string> {
  const port = await freePort();
  const child: ChildProcess = spawn(process.execPath, [SERVER_ENTRY], {
    env: {
      ...process.env,
      PORT: String(port),
      HOSTNAME: '127.0.0.1',
      PAIGASUS_ZONE: 'iam',
      PAIGASUS_ZONES: JSON.stringify(zones),
    },
    stdio: 'pipe',
  });
  try {
    const deadline = Date.now() + 60_000;
    for (;;) {
      if (Date.now() > deadline) throw new Error('standalone server did not become ready within 60s');
      try {
        const res = await fetch(`http://127.0.0.1:${port}/iam`);
        if (res.ok) return await res.text();
      } catch {
        // not listening yet
      }
      await new Promise((r) => setTimeout(r, 250));
    }
  } finally {
    child.kill('SIGTERM');
  }
}

describe('the standalone server reads configuration at runtime', () => {
  it('serves different zone maps from the SAME binary', async () => {
    const first = await fetchHomeWith({ iam: '/iam', gateway: '/gateway' });
    const second = await fetchHomeWith({ iam: '/iam', reports: '/reports' });

    expect(first).toContain('/gateway');
    expect(first).not.toContain('/reports');
    expect(second).toContain('/reports');
    expect(second).not.toContain('/gateway');
    expect(first).not.toBe(second);
  });

  it('fails loudly when the deployed prefix disagrees with the compiled one', async () => {
    await expect(fetchHomeWith({ iam: '/admin/iam' })).rejects.toThrow();
  });
});
```

The second case relies on the page returning a 500 — `res.ok` stays false, so the loop times out and rejects. If the timeout makes the suite slow, assert on the response status instead by returning `{ status, body }` from the helper.

- [ ] **Step 3: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
rm -rf ts/apps/paigasus-console/.next
cd ts && pnpm --filter @paigasus/console exec vitest run; cd ..
```

Expected: FAIL — the standalone entry point does not exist because the build has not run.

- [ ] **Step 4: Wire the test task so it depends on the build**

Add to `ts/apps/paigasus-console/moon.yml` under `tasks:`:

```yaml
  test:
    # The smoke boots the BUILT standalone server, so the build must have run. `~:build` is this
    # project's own build, not an upstream's.
    deps:
      - '~:build'
    inputs:
      - 'app/**/*'
      - 'tests/**/*'
      - 'vitest.config.ts'
      - 'next.config.ts'
      - 'package.json'
      - '/ts/packages/paigasus-next-config/src/**/*'
      - '/ts/pnpm-lock.yaml'
    options:
      merge: replace
```

- [ ] **Step 5: Run the test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-console-ts:test --force
```

Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:fmt ts:lint
git add ts/apps/paigasus-console ts/pnpm-lock.yaml
git commit -m "test(ts): assert the standalone console reads config at runtime" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 7: The `NEXT_PUBLIC_` gate script and its README

**Files:**
- Create: `ci/next-public/run.sh` (executable)
- Create: `ci/next-public/README.md`

**Interfaces:**
- Produces: `bash ci/next-public/run.sh [--self-test|--negative-control]`, exit codes 0 clean / 1 the repo is wrong / 2 infrastructure failed.
- Produces the exact lines Task 9 pins. Keep them byte-stable once written.

- [ ] **Step 1: Write the gate script**

`ci/next-public/run.sh`:

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:next-public-free — ban NEXT_PUBLIC_ across ts/, and require every app to build its Next
# config through @paigasus/next-config's factory (SMA-502).
#
# WHY. Next inlines every NEXT_PUBLIC_* value at `next build` time, so one OCI image cannot serve
# two self-hosters with different IdP issuers or API URLs. That single fact is the whole reason
# @paigasus/next-config exists. A grep is enough to enforce it mechanically, which beats relying
# on review forever.
#
# Exit codes: 0 pass | 1 the repo is wrong | 2 infrastructure failed.
#
# CHECK 2 EXISTS BECAUSE CHECK 1 IS NOT SUFFICIENT. `env:` in a next.config is a build-time
# inlining channel exactly equivalent to NEXT_PUBLIC_. createNextConfig() refuses an `extend.env`
# key — but a hand-written next.config.ts that never calls the factory contains no NEXT_PUBLIC_
# literal, passes check 1, and bakes a deployment value into the image anyway.
set -euo pipefail

# proto prints an NDJSON preamble on STDOUT inside an agent session, which poisons a `$(...)`
# capture. Exported once so any future capture in this file inherits it (SMA-609).
export PROTO_REPORTER=text

# Computed from BASH_SOURCE with NO env override, matching every other ci/*/run.sh. An override
# would let `REPO_ROOT=<anywhere> bash ci/next-public/run.sh` — the ordinary check path, no flag —
# scan an empty tree and report a clean pass. self_test and negative_control instead point a COPY
# of this file at their fixture directories, so BASH_SOURCE resolves there naturally.
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

BANNED='NEXT_PUBLIC_'
# The floor is what stops a moved or renamed ts/ silently emptying the gate — the SMA-553 class,
# which repo:input-liveness cannot reach here because it proves a DECLARED glob is live, never
# that the scan still sees anything.
#
# MEASURED: the real corpus is 72 tracked ts/ files after the lockfile and Markdown exclusions.
# 48 is two thirds of that, the same proportion ci/ruff/run.sh's floor of 10 uses against its own
# corpus. A floor set far below the real count would let ts/ collapse most of the way before the
# gate noticed, which defeats the point of having one.
CORPUS_FLOOR=48

# GLOBAL, not function-local: negative_control's EXIT trap fires after the function that sets this
# has already returned (including via the `exit 1` on its own failure path), and a `local` binding
# is gone by then — referencing it under `set -u` would itself be an unbound-variable error. This
# is MEASURED in this repo; see ci/ruff/run.sh:40-44.
tmp=""

die_infra()  { printf 'next-public-free: %s\n' "$*" >&2; exit 2; }
die_assert() { printf 'next-public-free: %s\n' "$*" >&2; exit 1; }

# Corpus: TRACKED files under ts/, minus the lockfile and Markdown.
#
# Markdown is excluded so a document can name the string it bans; a document compiles into
# nothing. The lockfile is excluded because a dependency NAME can contain the prefix and a
# lockfile is not compiled either. Untracked paths are out of scope entirely, so node_modules
# needs no rule and a third-party dependency using the prefix is unaffected.
next_public_corpus() {
  local root="${1:-$REPO_ROOT}"
  git -C "$root" ls-files -- 'ts/' | grep -v -e '^ts/pnpm-lock\.yaml$' -e '\.md$' | sort
}

# An allowlist entry is a RECORDED DECISION: path<TAB>reason. Ships empty.
allowed_path() {
  case "$1" in
    *) return 1 ;;
  esac
}

check_prefix() {
  local root="${1:-$REPO_ROOT}" corpus rc=0
  local -a files=()
  corpus="$(next_public_corpus "$root")" || rc=$?
  [ "$rc" -eq 0 ] || die_infra "git ls-files failed while deriving the ts/ corpus (rc $rc)"
  if [ -n "$corpus" ]; then
    mapfile -t files <<< "$corpus"
  fi
  [ "${#files[@]}" -ge "$CORPUS_FLOOR" ] \
    || die_assert "corpus collapsed to ${#files[@]} files (floor $CORPUS_FLOOR) — did ts/ move? If \
this is a legitimate shrink, lower CORPUS_FLOOR in ci/next-public/run.sh to match the new corpus \
size instead of raising it back later."
  local hits=0 f
  for f in "${files[@]}"; do
    allowed_path "$f" && continue
    if grep -q -- "$BANNED" "$root/$f" 2>/dev/null; then
      printf '  %s uses %s\n' "$f" "$BANNED" >&2
      hits=$((hits + 1))
    fi
  done
  if [ "$hits" -gt 0 ]; then
    die_assert "$hits file(s) use $BANNED. It is inlined at build time, so one image cannot serve two deployments — read it at request time via @paigasus/next-config/runtime instead."
  fi
  printf 'next-public-free: %d tracked ts/ files free of %s\n' "${#files[@]}" "$BANNED"
}

check_factory() {
  local root="${1:-$REPO_ROOT}" missing=0 cfg
  while IFS= read -r cfg; do
    [ -n "$cfg" ] || continue
    if ! grep -q 'createNextConfig' "$root/$cfg" 2>/dev/null; then
      printf '  %s does not call createNextConfig()\n' "$cfg" >&2
      missing=$((missing + 1))
    fi
  done <<< "$(git -C "$root" ls-files -- 'ts/apps/*/next.config.ts')"
  if [ "$missing" -gt 0 ]; then
    die_assert "$missing app config(s) bypass @paigasus/next-config. A hand-written \`env:\` block inlines exactly as $BANNED does, so the prefix scan alone cannot see it."
  fi
  printf 'next-public-free: every ts/apps/*/next.config.ts uses the factory\n'
}

# Build a git fixture tree. Each fixture IS a git repository, because the scan reads git ls-files:
# a bare mktemp -d would exercise a different code path and prove nothing about the tracked-file
# filter or the exclusion list. Fixtures live OUTSIDE the working tree — repo:actionlint and
# repo:input-liveness carry inputs: ['**/*'] and hash-walk the whole tree concurrently.
make_fixture() {
  local dir="$1" n=0
  git -C "$dir" init -q
  mkdir -p "$dir/ts/apps/probe" "$dir/ts/packages/probe/src"
  while [ "$n" -lt 25 ]; do
    printf 'export const filler%s = %s;\n' "$n" "$n" >"$dir/ts/packages/probe/src/filler$n.ts"
    n=$((n + 1))
  done
  printf 'import { createNextConfig } from "@paigasus/next-config";\nexport default createNextConfig({ zone: "probe", basePath: "/probe", outputFileTracingRoot: "/x" });\n' \
    >"$dir/ts/apps/probe/next.config.ts"
  git -C "$dir" add -A >/dev/null 2>&1
}

self_test() {
  local failures=0 clean violating markdown lockfile shrunk nofactory rc

  clean="$(mktemp -d)"; make_fixture "$clean"
  rc=0; ( cd "$clean" && check_prefix "$clean" && check_factory "$clean" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 0 ]; then
    printf '  FAIL clean fixture: expected rc 0, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  violating="$(mktemp -d)"; make_fixture "$violating"
  printf 'export const url = process.env.NEXT_PUBLIC_API_URL;\n' >"$violating/ts/packages/probe/src/bad.ts"
  git -C "$violating" add -A >/dev/null 2>&1
  rc=0; ( cd "$violating" && check_prefix "$violating" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 1 ]; then
    printf '  FAIL violating fixture: expected rc 1, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  markdown="$(mktemp -d)"; make_fixture "$markdown"
  printf 'Never use NEXT_PUBLIC_ in this workspace.\n' >"$markdown/ts/packages/probe/NOTES.md"
  git -C "$markdown" add -A >/dev/null 2>&1
  rc=0; ( cd "$markdown" && check_prefix "$markdown" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 0 ]; then
    printf '  FAIL markdown-only fixture: expected rc 0, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  lockfile="$(mktemp -d)"; make_fixture "$lockfile"
  printf 'packages:\n  next-public-shim: 1.0.0 # NEXT_PUBLIC_ in a dep name\n' >"$lockfile/ts/pnpm-lock.yaml"
  git -C "$lockfile" add -A >/dev/null 2>&1
  rc=0; ( cd "$lockfile" && check_prefix "$lockfile" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 0 ]; then
    printf '  FAIL lockfile-only fixture: expected rc 0, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  shrunk="$(mktemp -d)"; git -C "$shrunk" init -q
  mkdir -p "$shrunk/ts"; printf 'x\n' >"$shrunk/ts/only.ts"; git -C "$shrunk" add -A >/dev/null 2>&1
  rc=0; ( cd "$shrunk" && check_prefix "$shrunk" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 1 ]; then
    printf '  FAIL collapsed corpus: expected rc 1 from the floor, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  nofactory="$(mktemp -d)"; make_fixture "$nofactory"
  printf 'export default { env: { ISSUER: process.env.ISSUER } };\n' >"$nofactory/ts/apps/probe/next.config.ts"
  git -C "$nofactory" add -A >/dev/null 2>&1
  rc=0; ( cd "$nofactory" && check_factory "$nofactory" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 1 ]; then
    printf '  FAIL hand-written app config: expected rc 1, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  rm -rf "$clean" "$violating" "$markdown" "$lockfile" "$shrunk" "$nofactory"
  if [ "$failures" -gt 0 ]; then
    printf 'next-public-free self-test: %d row(s) failed\n' "$failures" >&2
    exit 1
  fi
  printf '== next-public-free self-test passed ==\n'
}

negative_control() {
  local negctl_rc=0
  # `tmp` is the FILE-SCOPE global declared above — deliberately not `local`. See its comment.
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  make_fixture "$tmp"
  printf 'export const url = process.env.NEXT_PUBLIC_API_URL;\n' >"$tmp/ts/packages/probe/src/planted.ts"
  git -C "$tmp" add -A >/dev/null 2>&1
  ( cd "$tmp" && check_prefix "$tmp" ) >/dev/null 2>&1 || negctl_rc=$?
  if [ "$negctl_rc" != 1 ]; then
    printf '  FAIL a planted NEXT_PUBLIC_ did not red the gate: expected rc 1, got %s\n' "$negctl_rc" >&2
    exit 1
  fi
  printf '== next-public-free negative control passed ==\n'
}

MODE=check
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)        MODE=selftest; shift ;;
    --negative-control) MODE=negctl;   shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

case "$MODE" in
  selftest) self_test ;;
  check)    check_prefix; check_factory ;;
  negctl)   negative_control ;;
esac
```

- [ ] **Step 2: Make it executable and run all three modes**

```bash
chmod +x ci/next-public/run.sh
bash ci/next-public/run.sh --self-test; echo "selftest rc=$?"
bash ci/next-public/run.sh --negative-control; echo "negctl rc=$?"
bash ci/next-public/run.sh; echo "check rc=$?"
```

Expected: all three rc 0. The real check must report at least `CORPUS_FLOOR` files.

**If the real check reds on the floor**, re-measure the corpus and set `CORPUS_FLOOR` to two thirds of the real count:

```bash
git ls-files -- 'ts/' | grep -v -e '^ts/pnpm-lock\.yaml$' -e '\.md$' | wc -l
```

At the time this plan was written that printed **72**, which is why the floor is 48. The count grows as this issue adds files, so it should only ever be comfortably above the floor.

**If the real check reds on a prefix hit**, an existing `ts/` file uses `NEXT_PUBLIC_` — fix the file, do not add an allowlist entry. Measured at plan time: zero files use it.

- [ ] **Step 3: Prove each mode bites**

```bash
printf 'export const x = process.env.NEXT_PUBLIC_OOPS;\n' > ts/packages/paigasus-ui/src/mutation-probe.ts
git add ts/packages/paigasus-ui/src/mutation-probe.ts
bash ci/next-public/run.sh; echo "rc=$? (expect 1)"
git rm -f --cached ts/packages/paigasus-ui/src/mutation-probe.ts >/dev/null && rm -f ts/packages/paigasus-ui/src/mutation-probe.ts
bash ci/next-public/run.sh; echo "rc=$? (expect 0)"
bash ci/next-public/run.sh --nonsense-flag; echo "rc=$? (expect 2)"
```

Expected: 1, then 0, then 2. The third confirms the infrastructure/assertion split — an unknown flag must not read as "the repo is wrong".

- [ ] **Step 4: Write the README**

`ci/next-public/README.md`, following `ci/ruff/README.md`'s shape — a title, `## Why`, `## What it asserts`, `## Exit codes, and why 1 and 2 must not collapse into each other`, `## Why check 2 exists`, `## Corpus derivation`, and `## Limitations`.

The Limitations section carries at least these, in `ci/ruff/README.md`'s lettered-paragraph style, each stating the residual, what does not close it, and why:

- **L1** — the ban is scoped to `ts/`. A `NEXT_PUBLIC_` in `rs/Dockerfile`, `ops/`, or a Helm template is equally harmful and unscanned. That scope is a decision: `ts/` is where a Next build reads the prefix from.
- **L2** — a split literal (`'NEXT_' + 'PUBLIC_'`) evades any text scan. No text gate closes this.
- **L3** — the `**/*.md` exclusion is safe only while no app compiles Markdown. An MDX app would make it a bypass.
- **L4** — `repo:input-liveness` catches a **dead** input glob, not a **too-narrow** one. A future narrowing of this gate's `inputs` would switch it off, and only `CORPUS_FLOOR` would notice, and only for the `ts/` rename case. The alternative — an unconditional `ci.yml` step — closes that residual but loses local `moon ci` coverage. The trade was made deliberately (spec § 6.5).
- **L5** — check 2 asserts an app config *mentions* `createNextConfig`; it does not prove the call reaches the default export. A config that imports the factory and exports something else passes.
- **L6** — if Task 1 recorded `NEXT_PHASE_REACHES_WORKERS=false`, record here that a module-scope config read inside a prerender worker is NOT caught by the build-phase guard, and that `await connection()` is the required discipline.

- [ ] **Step 5: Commit**

```bash
git add ci/next-public
git commit -m "feat(ci): add the next-public-free gate script" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 8: Gate registration — six of seven obligations

CLAUDE.md records that a brand-new `repo:*` gate added to `ci.yml`'s `T` array with **none** of the other obligations done still passes `repo:affected-smoke`. So each obligation is an explicit step, and Step 8 proves the ones that are self-enforcing actually fire.

**Files:**
- Modify: `moon.yml`, `.github/workflows/ci.yml`, `CLAUDE.md`, `ci/actionlint/run.sh`, `ci/affected-graph/ci_targets.py`

**Interfaces:**
- Consumes: `ci/next-public/run.sh` from Task 7 and `MOON_NEGATED_GLOBS` from Task 1.
- Produces: a scheduled, registered `repo:next-public-free`. Task 9 adds the seventh obligation.

- [ ] **Step 1: Add the Moon task**

In `moon.yml`, append after the `ruff-ci` block (which ends at `- 'py/uv.lock'`). Task keys sit at two spaces, fields at four — `ci/actionlint/run.sh`'s check 8e parses this file with a hand-rolled extractor held to that style:

```yaml
  next-public-free:
    description: 'Ban NEXT_PUBLIC_ across ts/ and require every app to use the next-config factory (SMA-502).'
    # WHY THIS EXISTS — Next inlines every NEXT_PUBLIC_* value at `next build` time, so one OCI
    # image cannot serve two self-hosters with different IdP issuers or API URLs. That is the
    # whole premise of @paigasus/next-config; a grep enforces it mechanically instead of by review.
    #
    # THE BAN COVERS PACKAGES, NOT ONLY APPS. A source-only package is transpiled by the app and
    # inlines the same way, so an apps-only ban leaves a hole.
    #
    # INPUTS ARE NOT ts/**/*. .moon/workspace.yml deliberately omits '**/.next/**' from
    # hasher.ignorePatterns, so a ts/**/* glob would hash the whole built .next tree — output this
    # gate never scans — and re-key on every console build.
    #
    # `--self-test` and `--negative-control` run FIRST and in the SAME block. `set -euo pipefail`
    # is REQUIRED: Moon does not enable errexit for `script:` blocks and takes the block's status
    # from its LAST command, so without it a failing control is masked by the passing real run.
    # These four lines are pinned by SELF_SCHEDULED_GATES.
    script: |
      set -euo pipefail
      bash ci/next-public/run.sh --self-test
      bash ci/next-public/run.sh --negative-control
      bash ci/next-public/run.sh
    toolchain: 'system'
    inputs:
      - '!ts/apps/*/.next/**'
      - 'ci/next-public/**/*'
      - 'ts/apps/**/*'
      - 'ts/packages/**/*'
```

**If Task 1 recorded `MOON_NEGATED_GLOBS=false`**, drop the `!` entry and instead list the app and package subdirectories explicitly, with a comment naming the measurement.

- [ ] **Step 2: Add the gate's own reachability input to `affected-smoke`**

In `moon.yml`, in `repo:affected-smoke`'s `inputs:`, after the `ci/ruff/**/*` entry (line ~211) and before `CLAUDE.md`:

```yaml
      # SMA-502 — this task pins load-bearing lines inside ci/next-public/run.sh
      # (NEXT_PUBLIC_FREE_SH_CALL_SITES), so a change under ci/next-public/ MUST re-key it.
      # Without this the pin is real but unreachable: the PR that deletes the pinned lines does
      # not schedule this task. Same reasoning as the ci/ruff/**/* entry above. Note the broad
      # 'ci/**/*' entry already covers it for SCHEDULING; the narrow entry is kept because
      # check 8e in ci/actionlint/run.sh floors this array's length and the file's own policy is
      # to keep narrow globs rather than collapse them into the broad one.
      - 'ci/next-public/**/*'
```

- [ ] **Step 3: Add the target to `ci.yml`'s `T` array**

At `.github/workflows/ci.yml:235`, append `:next-public-free` inside the existing parentheses, on the **same physical line**. `T` must stay a single-line bash array.

```bash
grep -c 'next-public-free' .github/workflows/ci.yml
awk 'NR==235 { print (NF>0 && $0 !~ /\n/) ? "single line: ok" : "WRAPPED" }' .github/workflows/ci.yml
```

Expected: `1`, and `single line: ok`.

- [ ] **Step 4: Mirror it in CLAUDE.md's marker-delimited command**

Between `<!-- ci-targets:begin -->` and `<!-- ci-targets:end -->` in `CLAUDE.md`, add `:next-public-free` so the target set matches `T` exactly. Keep it before `--base origin/main`. **Do not add a second copy of either marker anywhere in the file, not even inside backticks in prose** — a count of 2 reds the gate.

```bash
grep -c 'ci-targets:begin' CLAUDE.md && grep -c 'ci-targets:end' CLAUDE.md
```

Expected: `1` and `1`.

- [ ] **Step 5: Add the three `ci_targets.py` registry entries**

In `ci/affected-graph/ci_targets.py`:

`REQUIRED_REPO_TASKS`, after `"ruff-ci",` at line 202:

```python
    # SMA-502. Same reasoning as the release-parity*, workflow-credentials and ruff-ci entries:
    # this gate carries a --negative-control, and check_forward's `want`/`got` shrink CONSISTENTLY
    # when a task is dropped from `T` and made CI-ineligible in the same edit — so without a floor
    # entry the whole gate, control included, could be switched off with every check green.
    "next-public-free",
```

`SELF_TASK_EXPECTED_GLOBS`, after the `ruff-ci` entry closing at line 348. **`check_gate_inputs` compares globs sorted first, then literal files sorted** (`:1659-1674`), and all four of this gate's inputs are globs, so write them sorted:

```python
    # SMA-502. Four globs, no literal files, in check_gate_inputs' comparison order (globs sorted
    # first, then files sorted — there are none here). The negated entry keeps the built .next
    # tree out of the hash walk: .moon/workspace.yml deliberately omits '**/.next/**' from
    # hasher.ignorePatterns, so a ts/**/* glob alone would re-key this gate on every console build
    # while scanning nothing it hashed.
    "next-public-free": (
        "!ts/apps/*/.next/**",
        "ci/next-public/**/*",
        "ts/apps/**/*",
        "ts/packages/**/*",
    ),
```

`SELF_SCHEDULED_GATES`, after the `ruff-ci` entry closing at line 590:

```python
    # SMA-502. Four lines, like the other self-scheduled gates: `set -euo pipefail` is what makes
    # a failing control propagate, since Moon takes a `script:` block's status from its LAST
    # command.
    "next-public-free": (
        "set -euo pipefail",
        "bash ci/next-public/run.sh --self-test",
        "bash ci/next-public/run.sh --negative-control",
        "bash ci/next-public/run.sh",
    ),
```

- [ ] **Step 6: Add the reachability floor in `ci/actionlint/run.sh`**

In `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (`:2119-2150`), after the `'ci/ruff/**/*'` entry at line 2147:

```bash
  # SMA-502 — floors the input that makes NEXT_PUBLIC_FREE_SH_CALL_SITES reachable. Without it, a
  # PR editing ci/next-public/** does not schedule repo:affected-smoke, and neither the
  # SELF_SCHEDULED_GATES nor the SELF_TASK_EXPECTED_GLOBS pin for that gate can fire.
  'ci/next-public/**/*'
```

Confirm the array's length still clears check 8e's floor:

```bash
grep -c "^  '" ci/actionlint/run.sh | head -1
sed -n '5310p' ci/actionlint/run.sh
```

Expected: the check-8e line reads `-ge 20`, and the array now has 24 entries — comfortably above.

- [ ] **Step 7: Verify the graph loads, and write `SELF_TASK_EXPECTED_GLOBS` from what Moon REPORTS**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query tasks --affected >/dev/null; echo "graph rc=$?"
moon run repo:next-public-free --force
```

Expected: graph rc 0, and the task runs all three modes green. A rejected input glob surfaces here as a graph-load error naming the glob.

Now read the task's **resolved** inputs, because `check_gate_inputs` compares against what Moon reports, not what `moon.yml` authored:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects | python3 -c "
import json, sys
p = json.load(sys.stdin)['projects']
repo = next(x for x in p if x['id'] == 'repo')
t = repo['tasks']['next-public-free']
print('inputGlobs:', sorted(g for g in (t.get('inputGlobs') or {}) if g != '.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}'))
print('inputFiles:', sorted(t.get('inputFiles') or {}))
"
```

**Write the `SELF_TASK_EXPECTED_GLOBS["next-public-free"]` entry to match this output exactly** — globs sorted first, then files sorted, which is `check_gate_inputs`' comparison order (`ci_targets.py:1659-1674`). Moon may drop, rewrite, or re-sort the negated glob. If what it reports differs from what `moon.yml` authored, record the entry in **Moon's** form and add a comment stating the divergence. Guessing this entry is how it silently mismatches.

Note `moon query projects --json` **errors** on Moon 2.5.3 — bare `moon query projects` already emits JSON. Measure any exit status UNPIPED: `jq` and `python3` both return 0 on empty input, so a piped failure reads as "the reader found nothing" rather than "the command was invalid".

- [ ] **Step 8: Prove the self-enforcing registrations actually fire**

`check_self_scheduled_coverage` derives the must-register set from Moon's own resolved script text, so deleting only the `SELF_SCHEDULED_GATES` entry must red. Prove it:

```bash
python3 - <<'PY'
import pathlib, re
p = pathlib.Path('ci/affected-graph/ci_targets.py')
s = p.read_text()
p.with_suffix('.py.mutation-backup').write_text(s)
mutated = re.sub(r'\n    "next-public-free": \(\n        "set -euo pipefail",.*?\n    \),', '', s, flags=re.S)
assert mutated != s, 'mutation did not apply — check the entry text'
p.write_text(mutated)
PY
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke --force; echo "rc=$? (expect non-zero)"
mv ci/affected-graph/ci_targets.py.mutation-backup ci/affected-graph/ci_targets.py
touch ci/affected-graph/ci_targets.py
```

Expected: non-zero, naming `next-public-free` as unregistered. Restore by moving the backup back — **never** `git checkout`, which would discard this task's other edits to the same file.

Then confirm the T-array mirror fires: temporarily delete `:next-public-free` from `CLAUDE.md`'s marker block only, run `moon run repo:affected-smoke --force`, expect non-zero, and restore.

- [ ] **Step 9: Run the full registration check and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke repo:actionlint repo:input-liveness repo:ruff-ci --force
git add moon.yml .github/workflows/ci.yml CLAUDE.md ci/actionlint/run.sh ci/affected-graph/ci_targets.py
git commit -m "feat(ci): register the next-public-free gate" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

`repo:ruff-ci` is included because `ci_targets.py` is a `ci/**/*.py` file and must pass the same rule set.

---

## Task 9: The script pin — the seventh obligation

This is the expensive one, and the plan states its true size up front so nobody sizes it as "add a tuple". Measured: `check_self_invocation` takes **seven required positional parameters** (`ci_targets.py:1516-1519`) and has **47 call sites** — one production call at `:3147`, one f-string field access at `:2357`, and 45 self-test assertions. Every one gains an eighth argument.

**Files:**
- Modify: `ci/affected-graph/ci_targets.py`

**Interfaces:**
- Consumes: the exact lines written in Task 7. Verify each occurs **exactly once** in `run.sh` before pinning it.
- Produces: `NEXT_PUBLIC_FREE_SH_CALL_SITES`, and `check_self_invocation`'s eighth parameter `next_public_free_sh_text`.

- [ ] **Step 1: Verify every candidate pin line is unique in the script**

```bash
for line in \
  '--negative-control) MODE=negctl;   shift ;;' \
  'negctl)   negative_control ;;' \
  'check)    check_prefix; check_factory ;;' \
  'CORPUS_FLOOR=48' \
  'if [ "$negctl_rc" != 1 ]; then' ; do
  printf '%s -> %s\n' "$line" "$(grep -cxF "  $line" ci/next-public/run.sh 2>/dev/null || grep -cF "$line" ci/next-public/run.sh)"
done
```

Expected: each `1`. A count above 1 means the line cannot be whole-line pinned — pick a longer, unique variant before continuing.

- [ ] **Step 2: Add the pin tuple**

In `ci/affected-graph/ci_targets.py`, after `RUFF_SH_CALL_SITES`'s closing paren at line 1146:

```python
# SMA-502 — ci/next-public/run.sh's load-bearing lines. REACHABILITY IS NOT AUTOMATIC: this check
# only runs when repo:affected-smoke is scheduled, so moon.yml lists `ci/next-public/**/*` among
# its inputs and ci/actionlint/run.sh's T_AFFECTED_SMOKE_REQUIRED_INPUTS floors that entry.
#
# Matched as stripped WHOLE LINES, like the ruff haystack and for both of its reasons: the `case`
# arms and `if` bodies are indented, so a column-0 rule would reject the real executing lines,
# while a substring rule would let a COMMENTED-OUT copy satisfy the pin. Every entry was verified
# to occur EXACTLY ONCE in run.sh before this tuple was written.
#
# The set spans BOTH functions, not the control alone. A self-test over an extracted helper cannot
# see its own production call site being deleted, so the dispatch arms are pinned alongside the
# assertion bodies: the flag parse and both mode arms (a neutered parse falls through to the real
# suite, which then proves nothing), the corpus floor (lowering it makes the collapsed-corpus row
# pass vacuously), the two check invocations that ARE the production call site, and the control's
# guard comparison — whose VALUE, not merely its presence, carries the fix.
NEXT_PUBLIC_FREE_SH_CALL_SITES = (
    "--self-test)        MODE=selftest; shift ;;",
    "--negative-control) MODE=negctl;   shift ;;",
    "selftest) self_test ;;",
    "check)    check_prefix; check_factory ;;",
    "negctl)   negative_control ;;",
    "CORPUS_FLOOR=48",
    '( cd "$tmp" && check_prefix "$tmp" ) >/dev/null 2>&1 || negctl_rc=$?',
    'if [ "$negctl_rc" != 1 ]; then',
    "printf '  FAIL a planted NEXT_PUBLIC_ did not red the gate: expected rc 1, got %s\\n' \"$negctl_rc\" >&2",
    'if [ "$failures" -gt 0 ]; then',
)
```

Adjust each string to match Task 7's file byte-for-byte. Re-run Step 1's uniqueness check for every entry.

- [ ] **Step 3: Add the eighth parameter and haystack block**

Change the signature at `:1516-1519`:

```python
def check_self_invocation(
    run_sh_text, scripts, actionlint_sh_text, release_parity_sh_text,
    workflow_credentials_sh_text, release_plan_sh_text, ruff_sh_text,
    next_public_free_sh_text,
):
```

Update the three docstring counts: `:1523` "Eight haystacks" → "Nine haystacks"; `:1548` "The seven texts" → "The eight texts"; `:1560` "alone of the seven" → "alone of the eight".

Add to `:1552`'s required-parameter sentence: `next_public_free_sh_text`.

Add the haystack block immediately before `return missing` at `:1616`, mirroring `:1607-1615`:

```python
    # SMA-502 — stripped whole lines, like the ruff haystack above and for the same two reasons.
    next_public_free_lines = {line.strip() for line in next_public_free_sh_text.splitlines()}
    missing.extend(
        f"ci/next-public/run.sh: {site}"
        for site in NEXT_PUBLIC_FREE_SH_CALL_SITES
        if site not in next_public_free_lines
    )
    return missing
```

- [ ] **Step 4: Add it to the required-parameter loop**

At `:2605-2608`:

```python
    for _param_name in (
        "actionlint_sh_text", "release_parity_sh_text", "workflow_credentials_sh_text",
        "release_plan_sh_text", "ruff_sh_text", "next_public_free_sh_text",
    ):
```

- [ ] **Step 5: Wire `main()`**

After the `ruff_sh` read at `:3123-3125`:

```python
        next_public_free_sh = read_input(
            root / "ci" / "next-public" / "run.sh", "ci/next-public/run.sh"
        )
```

And at `:3147-3150`:

```python
    missing_sites = check_self_invocation(
        run_sh, scripts, actionlint_sh, release_parity_sh, workflow_credentials_sh,
        release_plan_sh, ruff_sh, next_public_free_sh,
    )
```

- [ ] **Step 6: Update all 45 self-test call sites mechanically**

Every self-test call currently ends its argument list with `wired_ruff` (or a ruff fixture variable). First define the wired fixture next to where `wired_ruff` is defined:

```python
    wired_next_public_free = (root / "ci" / "next-public" / "run.sh").read_text()
```

Match how `wired_ruff` is built — read the surrounding lines and copy that idiom exactly.

Then extend each call. Do it with a script, not by hand:

```bash
python3 - <<'PY'
import pathlib, re
p = pathlib.Path('ci/affected-graph/ci_targets.py')
s = p.read_text()
# Every self-test call ends with a ruff argument; append the new one after it.
s2 = re.sub(r'(\bwired_release_plan,\s*)(_ruff_[a-z_]+|wired_ruff)(\s*,?\s*\))',
            r'\1\2, wired_next_public_free\3', s)
p.write_text(s2)
print('rewrote', len(re.findall(r'wired_next_public_free', s2)), 'call sites')
PY
```

The regex must be adapted to the file's real formatting — read a handful of call sites first and confirm the shape. **The four ruff-battery calls at `:2790`, `:2798`, `:2811` and `:2829` pass a mutated ruff text and must still receive the UNMUTATED `wired_next_public_free`**, or those rows would fail for the wrong reason.

```bash
grep -c 'check_self_invocation(' ci/affected-graph/ci_targets.py
python3 -c "import ast,pathlib; ast.parse(pathlib.Path('ci/affected-graph/ci_targets.py').read_text()); print('syntax ok')"
```

- [ ] **Step 7: Add the four-fixture self-test battery**

After the ruff battery's end at `:2835`, add the same four rows for this gate. The fourth is **not optional**: with only the deletion, contamination and commented-out rows, rewriting the control's guard to something always-false leaves every other pinned line byte-identical and the control reports "passed" while asserting nothing.

```python
    # SMA-502 — the ruff battery above, repeated for the next-public-free haystack.
    for _npf_site in NEXT_PUBLIC_FREE_SH_CALL_SITES:
        _npf_broken = "".join(
            line for line in wired_next_public_free.splitlines(keepends=True)
            if line.strip() != _npf_site
        )
        if not check_self_invocation(
            wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
            wired_release_plan, wired_ruff, _npf_broken,
        ):
            failures.append(
                f"check_self_invocation: missed {_npf_site!r} deleted from ci/next-public/run.sh"
            )
    # Contamination: a next-public-free site must not be satisfiable from another haystack.
    if not check_self_invocation(
        wired + wired_next_public_free, scripts, wired_actionlint, wired_release_parity,
        wired_workflow_credentials, wired_release_plan, wired_ruff,
        "".join(line for line in wired_next_public_free.splitlines(keepends=True)
                if line.strip() != NEXT_PUBLIC_FREE_SH_CALL_SITES[0]),
    ):
        failures.append(
            "check_self_invocation: a next-public-free site was satisfied by run.sh text"
        )
    # Whole-LINE, not substring: a commented-out copy of a pinned line must report missing.
    _npf_commented = wired_next_public_free.replace(
        "negctl)   negative_control ;;\n", "# negctl)   negative_control ;;\n"
    )
    if not check_self_invocation(
        wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
        wired_release_plan, wired_ruff, _npf_commented,
    ):
        failures.append(
            "check_self_invocation: a COMMENTED-OUT next-public-free line satisfied the pin "
            "(widened to substring matching)"
        )
    # The entry whose VALUE, not merely its presence, carries the fix. Neutering the control's
    # comparison to something always-false leaves every OTHER pinned line — the diagnostic printf
    # included — byte-identical, so negative_control() would report "passed" unconditionally.
    _npf_guard_neutered = wired_next_public_free.replace(
        'if [ "$negctl_rc" != 1 ]; then\n', 'if [ "$negctl_rc" != "$negctl_rc" ]; then\n'
    )
    if not check_self_invocation(
        wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
        wired_release_plan, wired_ruff, _npf_guard_neutered,
    ):
        failures.append(
            "check_self_invocation: a NEUTERED comparison (always-false guard) satisfied the pin"
        )
```

- [ ] **Step 8: Run the self-test and the real gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py --self-test; echo "selftest rc=$?"
moon run repo:affected-smoke --force
```

Expected: both green. A self-test failure names the exact row.

- [ ] **Step 9: Prove the pin bites**

```bash
python3 - <<'PY'
import pathlib
p = pathlib.Path('ci/next-public/run.sh')
s = p.read_text()
p.with_suffix('.sh.mutation-backup').write_text(s)
p.write_text(s.replace('  negctl)   negative_control ;;\n', ''))
PY
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke --force; echo "rc=$? (expect non-zero)"
mv ci/next-public/run.sh.mutation-backup ci/next-public/run.sh
chmod +x ci/next-public/run.sh
touch ci/next-public/run.sh
moon run repo:affected-smoke --force; echo "rc=$? (expect 0)"
```

Expected: non-zero, naming `ci/next-public/run.sh: negctl)   negative_control ;;`, then 0 after restore.

**Re-run the WHOLE battery after any fix, not only the row you changed.** A fixture can go silently inert when a later edit changes how the text is processed, and the mutation then passes while asserting nothing.

- [ ] **Step 10: Lint the Python and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:ruff-ci --force
git add ci/affected-graph/ci_targets.py
git commit -m "feat(ci): pin the next-public-free gate call sites" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Task 10: Full-graph verification

Per-project Moon tasks do not run the repo-level gates. Before pushing, run the graph the way CI does.

**Files:** none — verification only.

- [ ] **Step 1: Confirm the working tree is clean**

```bash
git status --short
```

Expected: empty. Any leftover `.mutation-backup` file is a bug from an earlier task — remove it.

```bash
find . -name '*.mutation-backup' -not -path './.git/*'
```

Expected: no output.

- [ ] **Step 2: Run the full affected graph, exactly as `ci.yml` does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free --base origin/main --include-relations
```

Expected: all green.

**Two known local-only failure shapes, neither a real red:**

- The three `repo:release-parity*` gates abort **INCONCLUSIVE at rc 2** inside an agent session, because `proto` emits NDJSON that breaks a captured `$(proto bin …)`. `unset AI_AGENT CLAUDECODE CLAUDE_CODE_ENTRYPOINT` before re-running just those, and do not read the abort as a pass.
- A sub-3s `repo:affected-smoke` failure under a concurrent `moon ci` is a known infrastructure abort. **Capture the full task output before re-running**, because a passing re-run overwrites `stdout.log`, truncates `stderr.log` and flips the `ciReport.json` row. Grep the captured output for `proto-shim` — if that line is present, the failure is not about the affected graph.

- [ ] **Step 3: Diagnose any genuine failure before re-running**

<!-- moon-diagnosis:ok -->

The procedure below is copied from CLAUDE.md's `moon-diagnosis` block and reproduces it faithfully: there is **no** action-level `exitCode` key, the real exit code and command live in `operations[]` on the `task-execution` entry, and the logs must be proved to belong to this run before they are trusted. `repo:actionlint`'s check 12 requires any document mentioning `ciReport.json` to carry `<!-- moon-diagnosis:ok -->` (a correct reference) or `<!-- moon-diagnosis:superseded -->` (a historical one), so that the widely-copied broken advice cannot spread unmarked.

Copy the evidence out of the repo first — a re-run destroys it, and a **passing** re-run is just as destructive as a failing one:

```bash
mkdir -p /tmp/sma502-moon-evidence
cp .moon/cache/ciReport.json /tmp/sma502-moon-evidence/
jq '.actions[] | select(.status=="failed") | {label, error, exec: (.operations[] | select(.meta.type=="task-execution") | {command, exitCode})}' .moon/cache/ciReport.json
```

There is no action-level `exitCode` key — the real code and command are in `operations[]` on the `task-execution` entry. Then read `.moon/cache/states/<project>/<task>/stdout.log` and `stderr.log`, and **compare the action's `finishedAt` against `lastRun.json`'s `lastRunTime` before trusting them** — if they disagree the logs are from a different run.

- [ ] **Step 4: Confirm the four acceptance criteria, each against its own evidence**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
# AC1 — a real build produces the standalone server, and it reads config at runtime
moon run paigasus-console-ts:build paigasus-console-ts:test --force
# AC2 — the gate runs all three modes
moon run repo:next-public-free --force
# AC3 — the boundary rule reports a deliberately-wrong import
moon run paigasus-next-config-ts:test --force
# AC4 — the build outputs exclude .next/cache
grep -A4 'outputs:' ts/apps/paigasus-console/moon.yml
```

- [ ] **Step 5: Push and report**

```bash
git log --oneline origin/main..HEAD
git push -u origin feature/sma-502-next-config
```

Report: the commit list, which acceptance criterion each verification command proved, and any measurement from Task 1 that triggered a fallback.

---

## Self-Review

**Spec coverage.** § 2 principle → Tasks 2, 5 (comments) and 7 (gate). § 4 factory → Task 2. § 4.1 build-time prefix → Task 5 Step 4. § 4.2 cross-check, fail-closed, canonical form → Task 3. § 4.3 `outputFileTracingRoot` → Tasks 1, 2, 5. § 4.4 no `assetPrefix` default → Tasks 1 (M3), 2. § 5 `server-only`, edge → Task 3. § 5.1 variables and `basePathSchema` → Task 3. § 5.2 composition and key collisions → Task 3. § 5.3 memo and build-phase throw → Tasks 1 (M1), 3, 5 (Step 10). § 5.4 public slice and transport → Task 3. § 5.5 error messages → Tasks 2, 3. § 6.1 prefix scan and floor → Task 7. § 6.2 factory check → Task 7. § 6.3 fixtures, exit codes, pin → Tasks 7, 9. § 6.4 seven obligations → Tasks 8, 9. § 6.5 inputs and residuals → Tasks 7 (README), 8. § 6.6 the real cost → Task 9. § 7 boundary preset → Task 4. § 7.1 subpath forms → Task 4. § 7.2 liveness → Task 4. § 8.1 `.next/cache` → Tasks 1 (M4), 5. § 8.2 console inputs → Task 5. § 8.3 standalone assertion → Task 5. § 8.4 runtime smoke → Task 6. § 8.5 affected graph → Task 10. § 8.6 dev ergonomics → Task 5. § 9 testing → Tasks 2, 3, 4. § 10 dependencies → Task 2. § 13 measurements → Task 1.

**Gap found and closed:** § 8.5's claim that no `ci/affected-graph/run.sh` case changes was not verified anywhere. Task 10 Step 2 runs `repo:affected-smoke` on the full graph, which executes every case — if the claim is wrong, it reds there with the offending case named.

**Placeholder scan:** clean. Every "measure this" is Task 1, which has literal commands and stated fallbacks, and every later task names the fallback branch explicitly.

**Type consistency:** `canonicalBasePath(value, label)` — same two-argument shape in Tasks 2 and 3. `createNextConfig(options)` returns `NextConfig`; `outputFileTracingRoot` is required in the interface and supplied in Task 5. `defineRuntimeConfig` returns `{ getRuntimeConfig, getPublicConfig }` — destructured that way in Task 5. `PUBLIC_CONFIG_KEYS` is `['zone', 'zones']` and `getPublicConfig()` returns `{ zone, zones }`. `boundaryRules` entries carry `name` and `files`; the guard-the-guard test matches on `files`, which `tseslint.config()` preserves.
