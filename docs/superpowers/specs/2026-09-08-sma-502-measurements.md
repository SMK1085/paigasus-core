<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-502: environment measurements

Date: 2026-09-08

Measured in worktree `feature/sma-502-next-config` at
`/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-502`, after `proto install` and
`pnpm -C ts install`.

Tool versions:

- `node --version` → `v24.16.0`
- `pnpm --version` → `11.3.0`
- `moon --version` → `moon 2.5.3`
- Next.js version, from `ts/pnpm-lock.yaml` (`next@16.3.4:` resolution entry) → `16.3.4`

All five measurements below were taken with scratch files (`ts/packages/paigasus-next-config`,
`ts/apps/paigasus-console/next.config.ts`, `ts/apps/paigasus-console/app/probe/page.tsx`, a
temporary edit to `ts/apps/paigasus-console/moon.yml`, and a temporary edit to `ts/eslint.config.js`
and `ts/package.json`). Every scratch change was reverted at the end of the task (see "Revert" below).

## Step 1: dependency check

The brief's exact commands do not match this repo's layout — Next.js is not hoisted to
`ts/node_modules/.bin/`; it resolves only under the console app's own
`node_modules/.bin/` (pnpm's per-package dependency layout). Adapted check:

```
$ ls ts/node_modules/.bin/next && ls ts/node_modules/.pnpm/node_modules/next/package.json
ls: ts/node_modules/.bin/next: No such file or directory
```

```
$ ls ts/apps/paigasus-console/node_modules/.bin/next
ts/apps/paigasus-console/node_modules/.bin/next

$ ts/apps/paigasus-console/node_modules/.bin/next --version
Next.js v16.3.4
```

Deps were installed via `proto install --reporter text` then `pnpm -C ts install` (the latter
reported "Already up to date"). Adaptation reason: the brief's path assumed a hoisted
`node_modules/.bin`, which this workspace does not have; the underlying package is present and
resolvable, so the check's intent (deps installed) is satisfied via the adapted command.

## M5a — TS_LOADABLE_FROM_NEXT_CONFIG

Command (`ts/apps/paigasus-console/next.config.ts` importing `PROBE` from the scratch workspace
package `@paigasus/next-config`, added via `pnpm --filter @paigasus/console add
'@paigasus/next-config@workspace:*'`):

```
$ cd ts/apps/paigasus-console && pnpm exec next build 2>&1 | tee /tmp/sma502-m2.log
▲ Next.js 16.3.4 (Turbopack)
[probe] loaded from workspace package: next-config-probe
✓ Running next.config.ts took 1489ms
  Creating an optimized production build ...
[probe] loaded from workspace package: next-config-probe
✓ Compiled successfully in 1586ms
...
```

The probe line printed successfully; the import and its use compiled and executed with no loader
error.

**Derived value: `TS_LOADABLE_FROM_NEXT_CONFIG = true`.** `next.config.ts` can import TypeScript
source directly from a `workspace:*` package with an `exports: { ".": "./src/index.ts" }` field —
no build step or `.d.mts` generation is required for this consumer. No fallback triggered.

## M2 — STANDALONE_ENTRY

Command:

```
$ find ts/apps/paigasus-console/.next/standalone -name 'server.js' -maxdepth 4
/Users/.../ts/apps/paigasus-console/.next/standalone/apps/paigasus-console/server.js
```

**Derived value: `STANDALONE_ENTRY = .next/standalone/apps/paigasus-console/server.js`**
(relative to `ts/apps/paigasus-console/`). This matches the spec's prediction exactly — no
correction needed, no fallback triggered.

## M2b — does `next build` clear `.next` except `cache`?

Commands:

```
$ touch ts/apps/paigasus-console/.next/standalone/STALE-MARKER
$ cd ts/apps/paigasus-console && pnpm exec next build >/dev/null 2>&1
$ ls ts/apps/paigasus-console/.next/standalone/STALE-MARKER
ls: .../STALE-MARKER: No such file or directory
```

**Derived value:** the marker did not survive a rebuild. `next build` clears `.next/standalone`
(and presumably the rest of `.next` outside `cache`) on every run, matching the spec's
expectation. No fallback triggered — Task 5 does not need an explicit `rm -rf .next/standalone`
before build.

## M3 — ASSET_PREFIX_COMPOSES

Step A: `basePath: '/iam'` only, rebuild, grep an emitted asset URL:

```
$ cd ts/apps/paigasus-console && pnpm exec next build >/dev/null 2>&1; echo rc=$?
rc=0

$ grep -ro '"/[^"]*_next/static/[^"]*"' ts/apps/paigasus-console/.next/server/app/ 2>/dev/null | head -3
.../.next/server/app/_not-found.rsc:"/iam/_next/static/chunks/3wy-nto6vj8wz.js"
.../.next/server/app/_not-found.rsc:"/iam/_next/static/chunks/3wy-nto6vj8wz.js"
.../.next/server/app/_not-found.rsc:"/iam/_next/static/chunks/3wy-nto6vj8wz.js"
```

Prefix with `basePath` alone: `/iam/_next/static/...`.

Step B: add `assetPrefix: '/iam'` alongside `basePath: '/iam'`, rebuild, re-grep:

```
$ cd ts/apps/paigasus-console && pnpm exec next build >/dev/null 2>&1; echo rc=$?
rc=0

$ grep -ro '"/[^"]*_next/static/[^"]*"' ts/apps/paigasus-console/.next/server/app/ 2>/dev/null | head -3
.../.next/server/app/_not-found.rsc:"/iam/_next/static/chunks/3wy-nto6vj8wz.js"
.../.next/server/app/_not-found.rsc:"/iam/_next/static/chunks/3wy-nto6vj8wz.js"
.../.next/server/app/_not-found.rsc:"/iam/_next/static/chunks/3wy-nto6vj8wz.js"
```

Corroborating check against the manifest (`required-server-files.json` records the raw config
value, distinct from what's emitted into HTML/RSC output):

```
$ grep 'assetPrefix' ts/apps/paigasus-console/.next/required-server-files.json
    "assetPrefix": "/iam",
```

The emitted URL is `/iam/_next/static/...` in both runs — **not** `/iam/iam/_next/...`.

**Derived value: `ASSET_PREFIX_COMPOSES = false`.** Setting both `basePath` and `assetPrefix` to
the same value does **not** concatenate them; `assetPrefix` behaves as an independent override for
where static assets are fetched from, not as a suffix appended after `basePath`. This is a
negative result relative to what the brief's Step 5 wording suggested as the composing case
(`/iam/iam/...`) — recorded as measured, not corrected toward a prediction. **Fallback triggered:
any later task (Task 4/8, per the plan) that assumed `assetPrefix` needs to be set relative to
`basePath` (e.g. `assetPrefix: basePath`) must instead set `assetPrefix` to the same absolute
value as `basePath`, or omit `assetPrefix` entirely when it should just equal `basePath`.**

## M1 — NEXT_PHASE_REACHES_WORKERS

Probe page `ts/apps/paigasus-console/app/probe/page.tsx` throws at module scope when
`process.env.NEXT_PHASE === 'phase-production-build'`.

```
$ cd ts/apps/paigasus-console && pnpm exec next build 2>&1 | tee /tmp/sma502-m1.log
▲ Next.js 16.3.4 (Turbopack)
...
  Collecting page data using 5 workers ...
Error: Failed to collect configuration for /probe
    at ignore-listed frames {
  [cause]: Error: PROBE: module scope evaluated during next build
      at module evaluation (app/probe/page.tsx:4:9)
      at module evaluation (app/probe/page.tsx:10:1)
    2 | // Throwaway probe: a MODULE-SCOPE read, which is exactly what the build-phase guard must catch.
    3 | if (process.env.NEXT_PHASE === 'phase-production-build') {
  > 4 |   throw new Error('PROBE: module scope evaluated during next build');
...
> Build error occurred
Error: Failed to collect page data for /probe

$ grep -c 'PROBE: module scope evaluated during next build' /tmp/sma502-m1.log
2
```

Separately confirmed exit status:

```
$ cd ts/apps/paigasus-console && pnpm exec next build >/tmp/sma502-m1-rc.log 2>&1; echo "rc=$?"
rc=1
```

**Derived value: `NEXT_PHASE_REACHES_WORKERS = true`.** The build FAILS (rc=1) and the module-scope
throw message appears twice in the log. `NEXT_PHASE=phase-production-build` is inherited by the
page-data-collection workers, so a module-scope `NEXT_PHASE` guard does fire there, not only in
the main process. No fallback triggered — Task 3's `getRuntimeConfig()` guard design (a
module-scope `NEXT_PHASE` throw) works as designed; Task 3's "residual not covered by the gate"
concern about workers not inheriting the variable does not apply.

## M4 — MOON_NEGATED_OUTPUT_GLOBS

`ts/apps/paigasus-console/moon.yml`'s `build` task `outputs` temporarily changed from `['.next']`
to `['.next/**/*', '!.next/cache/**/*']`.

```
$ moon run paigasus-console-ts:build --force 2>&1 | tail -30
...
✓ Compiled successfully in 183ms
...
▮▮▮▮ paigasus-console-ts:build (2s 936ms, f7d725d9)
Tasks: 1 completed
 Time: 3s 482ms

$ moon query tasks --affected 2>/dev/null >/dev/null; echo "graph load rc=$?"
graph load rc=0
```

The graph loaded (rc 0) and the build succeeded with the negated glob present — no graph-load
error naming the glob.

Cached output archive inspection (per-task tarball
`.moon/cache/outputs/f7d725d9257462f339d7fe9b954eb855a0e28686767f2a9a1506c524bd254d3f.tar.gz`,
matching the task run hash `f7d725d9` shown above):

```
$ tar -tzf .moon/cache/outputs/f7d725d9....tar.gz | grep -E '\.next/BUILD_ID|\.next/cache' | head -10
ts/apps/paigasus-console/.next/standalone/apps/paigasus-console/.next/BUILD_ID
ts/apps/paigasus-console/.next/BUILD_ID

$ tar -tzf .moon/cache/outputs/f7d725d9....tar.gz | grep -c '\.next/cache/'
0
```

**Derived value: `MOON_NEGATED_OUTPUT_GLOBS = true`.** Moon 2.5.3 honours `!`-prefixed negated
globs in task **`outputs`**: the graph loads cleanly, the build runs, dot-prefixed entries such as
`.next/BUILD_ID` ARE captured in the cached output archive, and everything under `.next/cache/`
(0 matching entries in the tarball) is correctly excluded by the negation. No fallback triggered.

`ts/apps/paigasus-console/moon.yml` was reverted to `outputs: ['.next']` after this measurement.

### M4b — negated INPUT globs (added at CodeRabbit round 1)

**Why this exists.** The measurement above changed only `outputs` and inspected the output archive.
It was nevertheless cited as evidence for the negated **input** glob in
`repo:next-public-free`'s `inputs` (`moon.yml`, spec § 6.5), which it does not support: an output
negation controls what the cache ARCHIVES, an input negation controls what the cache KEYS ON, and
Moon implements those separately. Nothing had compared task hashes across a change to an excluded
path. This probe does.

**Method.** `repo:next-public-free` declares
`inputs: ['!ts/apps/*/.next/**', 'ci/next-public/**/*', 'ts/**/*']`. The task hash is read from
Moon's own completion line:

```
$ moon run repo:next-public-free 2>&1 \
    | grep -oE 'repo:next-public-free \(.*, [0-9a-f]{8}\)' | tail -n1 \
    | grep -oE '[0-9a-f]{8}\)' | tr -d ')'
```

Three arms, because two are not enough. Arm A alone cannot tell "the negation excluded the file"
from "the probe cannot detect anything"; arm B is the positive control that rules the second out.
Arms A and B together still cannot tell "the negation excluded it" from "that path would not have
been hashed anyway", so arm C repeats arm A with the negation line deleted from `moon.yml`.

```
## arm A — excluded path, negation present
  baseline                 hash=cae69cf0
  with .next/probe file    hash=cae69cf0        (ts/apps/paigasus-console/.next/probe-excluded.txt)
  RESULT: unchanged

## arm B — positive control, an included ts/ path
  baseline                 hash=cae69cf0
  with new tracked ts file hash=8867aa6f        (ts/packages/paigasus-next-config/src/probe-included.ts)
  RESULT: CHANGED

## arm C — same excluded path, negation REMOVED from moon.yml
  baseline (no negation)   hash=acfd6118
  with .next/probe file    hash=59cd599f
  RESULT: CHANGED
```

**Derived value: `MOON_NEGATED_INPUT_GLOBS = true`.** An excluded `.next` file does not move the
task hash (A); an included `ts/` file does, so the probe is live rather than inert (B); and the
same excluded file DOES move the hash once the negation is removed, so the negation is what
excluded it (C). No fallback triggered.

**Arm C also corrects a plausible wrong reading.** The `.next` tree is gitignored, so one might
assume Moon never hashes it and the negation is decorative. It is not: arm C shows Moon hashes an
untracked, gitignored file when a declared input glob covers it. That is consistent with
`.moon/workspace.yml`'s `hasher.ignorePatterns`, which deliberately omits `'**/.next/**'` — and it
is exactly why `moon.yml`'s comment says the negation is what keeps the broad `ts/**/*` form
affordable. Both halves of that comment are now measured rather than assumed.

The probe files were removed and `moon.yml` restored; `git status --short` was empty afterwards.

## M5b — TS_LOADABLE_FROM_ESLINT_CONFIG

First attempt, following the brief literally (scratch package added only as a dependency of
`@paigasus/console`, not of the `ts/` root workspace package that owns `eslint.config.js`):

```
$ cd ts && pnpm exec eslint --no-warn-ignored packages/paigasus-ui/src/index.ts
Error [ERR_MODULE_NOT_FOUND]: Cannot find package '@paigasus/next-config' imported from
  .../ts/eslint.config.js
rc=2
```

Adaptation: `ts/eslint.config.js` lives at the `ts/` workspace root (package `@paigasus/workspace`),
so Node's ESM resolver needs the package linked into `ts/node_modules`, not only into
`ts/apps/paigasus-console/node_modules`. Added the scratch package as a `devDependency` of the
root package too (`pnpm add -D -w '@paigasus/next-config@workspace:*'` from `ts/`), then re-ran:

```
$ cd ts && pnpm exec eslint --no-warn-ignored packages/paigasus-ui/src/index.ts; echo "rc=$?"
rc=0
```

**Derived value: `TS_LOADABLE_FROM_ESLINT_CONFIG = true`**, but with a caveat that changes Task 2's
packaging requirement: it only works once the consuming `package.json` (here, `ts/package.json`,
the workspace root that owns `eslint.config.js`) declares the dependency explicitly — plain
`workspace:*` resolution does not fall back to a sibling app's `node_modules`. **This is a
measured requirement Task 2 must account for**: `@paigasus/next-config` needs to be a declared
dependency of any package whose config file imports it directly, including `ts/package.json`
itself if `ts/eslint.config.js` is the consumer (as opposed to shipping the ESLint preset as
`src/eslint.mjs`, which the plan already defaults to and which this measurement does not
invalidate).

## M6 — SERVER_ONLY_BLOCKS_MIDDLEWARE (taken in the final fix wave)

**Question.** Three places on this branch claimed `import 'server-only'` makes a **middleware or
edge-route** import of `@paigasus/next-config/runtime` a **build error**. Is that true on the
pinned Next 16.3.4?

**Answer: NO. `server-only` does not stop a middleware import. The build succeeds.**

### Method and literal output

A throwaway `ts/apps/paigasus-console/middleware.ts` was added, importing the module and calling
`getRuntimeConfig()`, then `next build` was run from the app directory. The file was deleted
afterwards.

```
$ cat ts/apps/paigasus-console/middleware.ts
// THROWAWAY probe for SMA-502 I1. Deleted after the measurement.
import { NextResponse } from 'next/server';
import { defineRuntimeConfig } from '@paigasus/next-config/runtime';

const { getRuntimeConfig } = defineRuntimeConfig();

export function middleware() {
  getRuntimeConfig();
  return NextResponse.next();
}

export const config = { matcher: '/probe' };

$ pnpm exec next build
BUILD_EXIT=0
▲ Next.js 16.3.4 (Turbopack)
✓ Running next.config.ts took 15ms

⚠ The "middleware" file convention is deprecated. Please use "proxy" instead.
  Creating an optimized production build ...
✓ Compiled successfully in 708ms
  Running TypeScript ...
  Finished TypeScript in 705ms ...
✓ Generating static pages using 4 workers (3/3) in 229ms

Route (app)
┌ ƒ /
└ ○ /_not-found

ƒ Proxy (Middleware)
```

The module was then located in the build output, to confirm it really was compiled into the edge
layer rather than tree-shaken away:

```
$ grep -rl 'getRuntimeConfig() was called during' .next/ | head
.next/standalone/apps/paigasus-console/.next/server/chunks/ssr/_1drnzxw._.js
.next/standalone/apps/paigasus-console/.next/server/edge/chunks/[root-of-the-server]__0g9l-ai._.js
.next/server/edge/chunks/[root-of-the-server]__0g9l-ai._.js
.next/server/chunks/ssr/_1drnzxw._.js
```

### Why

`server-only` throws only when the `react-server` export condition is **absent** — its own
`package.json` maps `"react-server": "./empty.js"`, `"default": "./index.js"`:

```
$ cat ts/node_modules/.pnpm/server-only@0.0.1/node_modules/server-only/package.json
  "exports": { ".": { "react-server": "./empty.js", "default": "./index.js" } }
```

Next sets that condition for the middleware layer:

```
$ sed -n '557,561p' .../next/dist/build/webpack-config.js
    const reactServerConditionNames = [
        'react-server',
        ...conditionNames
    ];
$ sed -n '1402,1409p' .../next/dist/build/webpack-config.js
                        {
                            test: codeCondition.test,
                            issuerLayer: _constants.WEBPACK_LAYERS.middleware,
                            use: middlewareLayerLoaders,
                            resolve: {
                                mainFields: (0, _resolve.getMainField)(compilerType, true),
                                conditionNames: reactServerConditionNames,
```

So the import resolves to `empty.js` in a middleware and is a **no-op** there. `server-only` is a
**client-bundle** guard, and only that. That part is genuine and is kept.

## M6b — NEXT_RUNTIME is the guard that does close the edge door

**Question.** Next's `process.env.NEXT_RUNTIME` is documented as `'edge'` in the edge runtime.
Is it, on this build path?

**Answer: YES, and it is a compile-time DEFINE, so the guard is statically resolved.**

```
$ grep -rn "'process.env.NEXT_RUNTIME'" .../next/dist/build/define-env.js
80:        'process.env.NEXT_RUNTIME': isEdgeServer ? 'edge' : isNodeServer ? 'nodejs' : '',
```

A `NEXT_RUNTIME === 'edge'` guard was added to `getRuntimeConfig()` and the same probe middleware
rebuilt. The literal compiled output shows the branch **eliminated in both directions** — an
unconditional throw in the edge chunk, and the code removed entirely from the node chunk:

```
$ pnpm exec next build
BUILD_EXIT=0

$ grep -o '.\{80\}called in the EDGE runtime.\{80\}' \
    '.next/server/edge/chunks/[root-of-the-server]__0g9l-ai._.js'
aigasus/next-config owns it.`);function t(){throw Error("getRuntimeConfig() was called in the EDGE
runtime. The edge runtime has no dynamic `process.env` — values are inlined at build ti

$ grep -c 'called in the EDGE runtime' .next/server/chunks/ssr/_1drnzxw._.js
0
```

So in an edge bundle every `getRuntimeConfig()` call is an unconditional `throw`, and in the node
bundle the guard costs nothing. It is a loud **runtime** failure on first call, not a build error
— which is the honest claim, and is what the three sites now say.

The throwaway `middleware.ts` was deleted after the measurement (`git status --short` clean of
it, confirmed).

## M6c — the zod `.refine()` message form that actually leaks

**Question.** For I5, which `.refine()` message form puts the input into the thrown error on the
pinned zod 4.5.4? The review quoted `(v) => ({ message: \`${v} …\` })`.

**Answer: that (zod 3) form does NOT leak on zod 4; the `{ error: (iss) => … }` form does.**

```
$ node probe-zod.mjs
form A: [{"code":"custom","path":["SECRET_TOKEN"],"message":"Invalid input"}]
form B: [{"code":"custom","path":["SECRET_TOKEN"],"message":"sk-live-abc123 is not a valid issuer"}]
form C: [{"code":"custom","path":["SECRET_TOKEN"],"message":"sk-live-abc123 is not a valid issuer"}]
```

(A = `.refine(fn, (v) => ({ message }))`, B = `.refine(fn, { error: (iss) => … })`, C = a literal
string message.) A test written against form A would pass with the `describeIssues` owner check
deleted, i.e. it would assert nothing. The shipped test uses form B, and the guard was verified to
bite by mutating it back to `issue.code === 'custom'` — see the fix report.

## M6d — bash 5.3.15 deadlocks on a here-string feeding a builtin

**Found while verifying the fix wave, not predicted.** `moon run repo:next-public-free` and
`bash ci/next-public/run.sh --self-test` began hanging with **no output and no failure**, after
having passed minutes earlier in the same session on the same code.

`sample` on the stuck process showed a `heredoc_write` → `write` block, and a `bash -x` trace
placed it on `mapfile -t files <<< "$corpus"` in `check_prefix` — a line this branch did not
change. bash writes a here-string into a pipe from `do_redirections` **before** the builtin that
would drain it starts, so content past the pipe's capacity blocks forever.

Threshold, measured on this host (`/opt/homebrew/bin/bash`, 5.3.15, installed 2026-06-13):

```
/opt/homebrew/bin/bash n=1   len=32:   ok
/opt/homebrew/bin/bash n=5   len=164:  ok
/opt/homebrew/bin/bash n=10  len=330:  ok
/opt/homebrew/bin/bash n=20  len=670:  HUNG
/opt/homebrew/bin/bash n=40  len=1350: HUNG
/opt/homebrew/bin/bash n=61  len=2064: HUNG
/opt/homebrew/bin/bash n=200 len=6891: HUNG
--- system bash ---
/bin/bash n=61  len=2064: ok
/bin/bash n=200 len=6891: ok
```

And the fix shape, same host, 200 lines:

```
HUNG: here-string
n=200
ok:   process substitution
```

It is **INTERMITTENT** rather than version-deterministic: macOS can hand back a 512-byte pipe
under memory pressure instead of the usual 16K, which is why the identical command passed at
06:11 and hung at 06:16 after several `next build` and `moon` runs. So a green run does not clear
this.

**Fix.** Both reads in `ci/next-public/run.sh` now use `mapfile -t … < <(printf '%s\n' "$var")`,
which forks a writer and cannot deadlock. `ci/ruff/run.sh:114` and `:239` carry the identical
`mapfile … <<<` pattern and are **NOT** changed here — that gate is outside SMA-502's scope, and
the same hang applies to it on this host.

## Revert

```
$ git checkout -- ts/eslint.config.js ts/apps/paigasus-console/next.config.ts \
    ts/apps/paigasus-console/package.json ts/pnpm-lock.yaml ts/package.json
$ rm -rf ts/apps/paigasus-console/app/probe ts/packages/paigasus-next-config ts/apps/paigasus-console/.next
$ git status --short
 M docs/superpowers/plans/2026-09-08-sma-502-next-config.md
```

`ts/package.json` was added to the `git checkout --` list beyond the brief's literal Step 9
command, because Step 8's adaptation (above) required editing it; reverting only the brief's
listed files would have left that edit in place. The one remaining modified file,
`docs/superpowers/plans/2026-09-08-sma-502-next-config.md`, predates this task's scratch work (it
already carried the controller's Step 9 pnpm-store-restore ruling before this task began) and was
left untouched.

Per the controller's ruling on Step 9, the pnpm store was then restored (not part of the brief's
original Step 9, added because `rm -rf ts/packages/paigasus-next-config` otherwise leaves the
store holding a link to a package that no longer exists):

```
$ pnpm -C ts install
Lockfile is up to date, resolution step is skipped
Packages: +2
++
devDependencies:
- @paigasus/next-config
```

Final `git status --short` after the restore install: only the pre-existing plan-doc modification
remains (confirmed no other tracked file changed).

## Summary of derived values

| Value | Result | Fallback triggered? |
|---|---|---|
| `STANDALONE_ENTRY` | `.next/standalone/apps/paigasus-console/server.js` | No — matches spec prediction |
| `ASSET_PREFIX_COMPOSES` | `false` | **Yes** — affects any task setting `assetPrefix` relative to `basePath` |
| `MOON_NEGATED_OUTPUT_GLOBS` (M4) | `true` | No |
| `MOON_NEGATED_INPUT_GLOBS` (M4b) | `true` | No |
| `TS_LOADABLE_FROM_NEXT_CONFIG` | `true` | No |
| `TS_LOADABLE_FROM_ESLINT_CONFIG` | `true`, with a packaging caveat (see M5b) | No fallback triggered, but Task 2 must declare the dependency on every direct-import consumer's `package.json` |
| `NEXT_PHASE_REACHES_WORKERS` | `true` | No — Task 3's guard design works as-is |

## Tasks affected by a fallback-triggering result

- **`ASSET_PREFIX_COMPOSES = false`**: any later task (per the plan, Task 4 and/or Task 8) that
  composes `assetPrefix` as a suffix appended after `basePath` (e.g. computing
  `assetPrefix = basePath + assetPrefix`, or assuming the two concatenate) must instead treat
  `assetPrefix` as an independent, non-composing value — typically set to the same absolute value
  as `basePath`, not appended to it.
- **M5b packaging caveat**: Task 2 should verify that wherever `@paigasus/next-config` is imported
  directly by a TypeScript config file (not only via the `src/eslint.mjs` preset path the plan
  already defaults to), the importing file's own nearest `package.json` declares the dependency —
  `workspace:*` linking is per-package, not workspace-wide.
- No other measurement triggered a fallback: `STANDALONE_ENTRY`, `MOON_NEGATED_OUTPUT_GLOBS`, `MOON_NEGATED_INPUT_GLOBS`,
  `TS_LOADABLE_FROM_NEXT_CONFIG`, and `NEXT_PHASE_REACHES_WORKERS` all matched their expected /
  predicted values, so Tasks 2, 4, 5, 7 and 8 can proceed on the spec's original assumptions for
  those four.
