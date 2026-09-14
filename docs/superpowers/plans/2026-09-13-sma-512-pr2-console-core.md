# SMA-512 PR 2 — `@paigasus/console-core`: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the console composition code out of `ts/apps/iam-console/lib/` into a new
server-only package both zones can use, and move `iam-console` onto it — with no behaviour change.
Closes SMA-631.

**Architecture:** One source-only package, `@paigasus/console-core`, holding the IAM client
accessors, provisioning and the principal resolver, `mayI()`, `myScopes()`, `callIam`, the
JSON-lines logger, discovery, the correlation helpers and the PRN reader. It reads no environment:
an app passes a config **thunk**, so the factory is safe at module scope and nothing is read during
`next build`. Files move bottom-up — leaves first, then the middle layer, then the cached
accessors — so every task leaves a compiling tree.

**Tech Stack:** TypeScript 6, pnpm workspaces, Moon 2.5.3, vitest, ESLint flat config.

**Spec:** `docs/superpowers/specs/2026-09-13-sma-512-gateway-console-design.md` § 5, which § 3
assigns to pull request 2.

## Global Constraints

- **No behaviour change.** `iam-console`'s existing suites must stay green throughout. That is the
  property that makes this pull request reviewable.
- Every file under the package's `src/` opens with `import 'server-only'` **and** the SPDX header
  `// SPDX-License-Identifier: Apache-2.0`.
- **The `testing/` subpath lives OUTSIDE `src/`** and is NOT `server-only` guarded — vitest and
  Playwright harnesses import it outside a Next server. That placement is what lets the `src/` rule
  hold without an exception.
- Conventional commits with a workspace scope (`refactor(ts):`, `feat(ts):`).
- `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` before any `moon`/`pnpm`.
- Do NOT amend commits. Add new ones.
- Local bash traps, all measured: `ci/affected-graph/run.sh` needs system `/bin/bash` 3.2 (5.3.15
  deadlocks); `ci/actionlint/run.sh` needs `/opt/homebrew/bin/bash`; `ci/ruff/run.sh` and
  `ci/next-public/run.sh` need bash 4+ for `mapfile`. No single local bash runs every gate.

## What the research changed, versus spec § 5

Three corrections. Follow this plan, not the spec, where they differ; Task 8 updates the spec.

1. **`lib/tenancy-path.ts` does NOT move.** It is one constant (`TENANCY_PATH = '/orgs'`) and it
   deliberately carries **no** `server-only` guard — its own comment records why: a Server Actions
   file may export only async functions, so the constant cannot live in an `actions.ts`. Moving it
   into `src/` would force an exception to the guard rule for one string. It stays per app.
2. **The factory takes `config`, not `iamGrpcUrl`.** Spec § 5.3 proposed
   `{ authRuntime, iamGrpcUrl, logger }`. `discovery.ts` needs `PAIGASUS_SESSION_STORE`,
   `PAIGASUS_SESSION_REDIS_URL` and the `PAIGASUS_DISCOVERY_*_MS` timings as well, so a single
   `config: () => ConsoleCoreConfig` thunk replaces the two. Each app's own `ConsoleConfig`
   structurally satisfies it.
3. **`SESSION_COOKIE_NAME` is solved upstream, not moved.** Spec § 5.4 already rules that
   `@paigasus/auth` exports `SESSION_COOKIE` from `./server`; Task 2 does that and deletes the
   app's re-declaration, so neither app nor the package re-states it.

## File inventory — what moves, what stays

`ts/apps/iam-console/lib/` holds 19 files, 1148 lines. All carry `import 'server-only'` except
`tenancy-path.ts`.

**Moves to `packages/paigasus-console-core/src/`** (13 files): `principal-prn.ts`,
`prn-tenancy.ts`, `correlation-header.ts`, `logger.ts`, `correlation.ts`, `errors.ts`,
`iam-clients.ts`, `principal-resolver.ts`, `iam.ts`, `principal.ts`, `authorize.ts`, `scopes.ts`,
`discovery.ts`.

**Stays in each app** (6 files): `config.ts` (the `defineRuntimeConfig` call merges three schema
sources plus an app-owned `PAIGASUS_SERVICES` refinement — inherently one per app), `auth.ts`
(composition; becomes a thin call), `nav.ts` (per-zone entries), `form.ts` and `paging.ts` (IAM
screen helpers with no gateway consumer), `tenancy-path.ts` (correction 1).

---

### Task 1: The package skeleton, its boundary rule, and the four leaf files

**Files:**
- Create: `ts/packages/paigasus-console-core/{package.json,moon.yml,tsconfig.json,vitest.config.ts}`
- Create: `ts/packages/paigasus-console-core/src/index.ts`
- Move: `lib/{principal-prn.ts,prn-tenancy.ts,correlation-header.ts,logger.ts}` → package `src/`
- Move: `tests/unit/{prn-tenancy.test.ts,logger.test.ts}` → package `tests/unit/`
- Modify: `ts/packages/paigasus-next-config/src/eslint.mjs`, `.../tests/boundaries.test.ts`,
  `ts/packages/paigasus-next-config/src/index.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: the package, importable as `@paigasus/console-core`. Exports so far:
  `principalPrnOf`, the `prn-tenancy` surface (`TenancyKind`, `TenancyRef`, `ROOT_PRN`, `isUuid`,
  `parseTenancyPrn`, `organizationPrn`, `teamPrn`, `projectPrn`), `CORRELATION_HEADER`,
  `REQUEST_PATH_HEADER`, and the logger surface (`AppEventName`, `AppEventFields`,
  `ConsoleLogger`, `createJsonLogger`, `logger`).

**Why these four first.** They are the only `lib/` files with **zero** imports from other `lib/`
files, so they move without touching anything else. Everything later builds on them.

- [ ] **Step 1: Create the package manifest**

`ts/packages/paigasus-console-core/package.json`:

```json
{
  "name": "@paigasus/console-core",
  "_comment_exports": "A root export plus a testing subpath. Root is server-only, the way @paigasus/sdk's is — @paigasus/auth and @paigasus/discovery omit a root export only because they also have a CLIENT surface, which this package does not. ./testing is deliberately outside src/ and carries no server-only guard: vitest and Playwright harnesses import it outside a Next server.",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "engines": { "node": ">=24" },
  "license": "Apache-2.0",
  "description": "Server-only console composition shared by the Paigasus zones: IAM clients, provisioning and the principal resolver, authorization self-queries, scopes, discovery, the error wrapper and the JSON-lines logger.",
  "exports": {
    ".": "./src/index.ts",
    "./testing": "./testing/index.ts"
  },
  "scripts": { "typecheck": "tsc -p tsconfig.json --noEmit" },
  "dependencies": {
    "@paigasus/auth": "workspace:*",
    "@paigasus/discovery": "workspace:*",
    "@paigasus/sdk": "workspace:*",
    "redis": "catalog:",
    "server-only": "catalog:"
  },
  "peerDependencies": { "next": "catalog:", "react": "catalog:" },
  "devDependencies": {
    "@connectrpc/connect-node": "catalog:",
    "@paigasus/proto": "workspace:*",
    "@types/node": "catalog:",
    "jose": "catalog:",
    "next": "catalog:",
    "react": "catalog:",
    "testcontainers": "catalog:",
    "typescript": "catalog:",
    "vitest": "catalog:"
  }
}
```

`@paigasus/proto`, `@connectrpc/connect-node`, `jose` and `testcontainers` are **devDependencies**:
only `testing/` uses them, so neither app carries them in its production graph.

- [ ] **Step 2: Create `moon.yml`**

Modelled on `ts/packages/paigasus-app-shell/moon.yml`. **Append inputs, never
`options.merge: replace`** — the inherited definitions supply `@group(sources)`, `@group(tests)`,
`tsconfig.json`, `package.json`, `/ts/tsconfig.base.json` and `/ts/pnpm-lock.yaml`, and replacing
silently drops them (the defect SMA-503 fixed on `iam-console-ts:build`).

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-console-core-ts'
layer: 'library'
language: 'typescript'

# BOTH are required and neither implies the other. `dependsOn` is what `moon query projects
# --affected` follows; task `inputs` are the ONLY thing that makes a DOWNSTREAM task affected
# (CLAUDE.md, Moon 2.5.3).
dependsOn:
  - 'paigasus-auth-ts'
  - 'paigasus-sdk-ts'
  - 'paigasus-discovery-ts'
  - 'paigasus-proto-ts'

tasks:
  build:
    inputs: &upstreams
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-sdk/src/**/*'
      - '/ts/packages/paigasus-sdk/package.json'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
      # testing/ is a published subpath, not test-only scaffolding: both apps import it. It is
      # outside src/, so @group(sources) does not cover it.
      - 'testing/**/*'
  typecheck:
    inputs: *upstreams
  test:
    inputs:
      - 'vitest.config.ts'
      - 'testing/**/*'
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-sdk/src/**/*'
      - '/ts/packages/paigasus-sdk/package.json'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
      # The parity corpus and the Rust constant the PRN reader is held to.
      - '/rs/crates/libs/paigasus-kernel-parity/vectors/**/*'
      - '/rs/crates/libs/paigasus-iam-core/src/authz/model.rs'
```

GitHub Actions supports YAML anchors; Moon's schema does too. If `*upstreams` fails to load, inline
the list in both tasks rather than debugging the anchor.

- [ ] **Step 3: Create `tsconfig.json` and `vitest.config.ts`**

Copy `ts/packages/paigasus-app-shell/tsconfig.json`, changing only paths. For `vitest.config.ts`,
copy `ts/apps/iam-console/vitest.config.ts`'s **aliases** — `server-only`, `next/headers`,
`next/cache` — since the moved code imports all three. Set `environment: 'node'`, and set
`ssr.resolve.conditions` **alongside** the top-level `resolve.conditions`: vitest 5 resolves a
Node-environment test's imports through the former, and setting only the latter has no effect.
List the additive defaults in full, `['react-server', 'node', 'import', 'default']`, never a
single-entry array — a bare list drops `import`/`default` and breaks source-exports `.ts`
resolution for every `@paigasus/*` package.

- [ ] **Step 4: Move the four leaf files and their tests**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
mkdir -p ts/packages/paigasus-console-core/src ts/packages/paigasus-console-core/tests/unit
git mv ts/apps/iam-console/lib/principal-prn.ts       ts/packages/paigasus-console-core/src/
git mv ts/apps/iam-console/lib/prn-tenancy.ts         ts/packages/paigasus-console-core/src/
git mv ts/apps/iam-console/lib/correlation-header.ts  ts/packages/paigasus-console-core/src/
git mv ts/apps/iam-console/lib/logger.ts              ts/packages/paigasus-console-core/src/
git mv ts/apps/iam-console/tests/unit/prn-tenancy.test.ts ts/packages/paigasus-console-core/tests/unit/
git mv ts/apps/iam-console/tests/unit/logger.test.ts      ts/packages/paigasus-console-core/tests/unit/
```

Fix the relative import depth in both moved tests (`../../lib/x` → `../../src/x`), and the parity
corpus path in `prn-tenancy.test.ts` (it walks up to the repo root — re-derive, do not guess).

- [ ] **Step 5: Write `src/index.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The package's one entry. Server-only: every module it re-exports imports 'server-only', so a
// client component that reaches this package fails the build loudly rather than shipping a
// token-bearing module into a browser bundle.
import 'server-only';

export { principalPrnOf } from './principal-prn';
export { ROOT_PRN, isUuid, organizationPrn, parseTenancyPrn, projectPrn, teamPrn, type TenancyKind, type TenancyRef } from './prn-tenancy';
export { CORRELATION_HEADER, REQUEST_PATH_HEADER } from './correlation-header';
export { createJsonLogger, logger, type AppEventFields, type AppEventName, type ConsoleLogger } from './logger';
```

- [ ] **Step 6: Re-point the app's imports of the four moved files**

`lib/auth.ts`, `lib/iam.ts`, `lib/errors.ts`, `lib/authorize.ts`, `lib/discovery.ts`,
`lib/principal.ts`, `lib/principal-resolver.ts`, `lib/correlation.ts`, `lib/iam-clients.ts`,
`lib/scopes.ts` and `proxy.ts` import one or more of them. Change each `./logger`,
`./principal-prn`, `./prn-tenancy`, `./correlation-header` import to `@paigasus/console-core`.

Find them all rather than working from this list:

```bash
grep -rn "from '\./\(logger\|principal-prn\|prn-tenancy\|correlation-header\)'" ts/apps/iam-console/
grep -rn "lib/\(logger\|principal-prn\|prn-tenancy\|correlation-header\)" ts/apps/iam-console/
```

- [ ] **Step 7: Add the package to `SOURCE_ONLY_PACKAGES`**

`ts/packages/paigasus-next-config/src/index.ts:16`. Without it Next is handed raw TypeScript and
neither app builds. Its own comment says the list grows when a new package lands.

- [ ] **Step 8: Add the boundary rule**

In `ts/packages/paigasus-next-config/src/eslint.mjs`, add a block modelled on the `sdk` one:

```js
{
  name: 'paigasus/boundaries/console-core',
  // The ONE package allowed to import both @paigasus/auth and @paigasus/sdk. It exists because
  // @paigasus/auth must not import @paigasus/sdk (ports/principal-resolver.ts) and the sdk block
  // below bans every @paigasus/* but proto — so only an app could depend on both, until now.
  //
  // @paigasus/proto stays BANNED in src/: the provisioning call reaches ServiceInfoService through
  // the sdk's re-export (SMA-511 § 7.3). The testing/ subpath is the one exception — its fake IAM
  // needs ErrorInfoSchema, which only @paigasus/proto exports.
  files: ['packages/paigasus-console-core/src/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
  rules: restrict([
    {
      group: ['@paigasus/proto', '@paigasus/proto/**'],
      message: '@paigasus/console-core reaches proto through @paigasus/sdk, never directly (§ 5.5). The testing/ subpath is the exception.',
    },
    {
      group: ['react-dom', 'react-dom/*', 'next/navigation'],
      message: '@paigasus/console-core is server-only composition. React components belong in the app or in @paigasus/ui (§ 5.2).',
    },
  ]),
},
```

Add `'packages/paigasus-console-core/src': 'exists'` to `BOUNDARY_SCOPES`. The reverse-liveness
loop derives the key as `files[0].split('/**')[0]` (`boundaries.test.ts:254`) and asserts both
directions, so the key must be exactly that string.

Add `@paigasus/console-core` and `@paigasus/console-core/**` to the **`app-middleware`** block's
first group, beside `@paigasus/auth/server` and `@paigasus/sdk`. That block's own message says why:
`server-only` is a no-op in the middleware layer, and this package transitively holds both.

**Do NOT add a `packages/**` block for the reverse rule** ("only apps may import console-core"). In
flat config a second `no-restricted-imports` block matching the same files **replaces** the first,
which would switch off the `sdk`, `auth-*` and `discovery` boundary rules in silence
(`eslint.mjs:376-379`). If the reverse rule is wanted, it needs a custom named rule in
`sourceRules`, following the `paigasus/no-js-relative-specifier` precedent — and that is out of
scope here; record it as a follow-up.

- [ ] **Step 9: Add boundary test rows**

In `ts/packages/paigasus-next-config/tests/boundaries.test.ts`, rows are
`readonly [label, filePath, source]`:

```ts
// DENIED
['console-core must not import proto directly', 'packages/paigasus-console-core/src/principal.ts', "import { x } from '@paigasus/proto';"],
['a proxy must not import console-core', 'apps/iam-console/proxy.ts', "import { x } from '@paigasus/console-core';"],
// ALLOWED
['console-core may import auth/server', 'packages/paigasus-console-core/src/iam.ts', "import { x } from '@paigasus/auth/server';"],
['console-core may import the sdk', 'packages/paigasus-console-core/src/iam.ts', "import { x } from '@paigasus/sdk/iam';"],
['console-core testing may import proto', 'packages/paigasus-console-core/testing/fake-iam.ts', "import { x } from '@paigasus/proto';"],
```

- [ ] **Step 10: Install, build, test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts install
moon run paigasus-console-core-ts:test
moon run paigasus-next-config-ts:test
moon run iam-console-ts:test
moon run ts:lint && moon run ts:fmt
```

Expected: all pass. `iam-console-ts:test` passing is the no-behaviour-change proof for this task.

- [ ] **Step 11: Commit**

```bash
git add -A ts/packages/paigasus-console-core ts/packages/paigasus-next-config ts/apps/iam-console ts/pnpm-lock.yaml
git commit -m "feat(ts): add @paigasus/console-core with the four leaf modules (SMA-512)"
```

---

### Task 2: Export `SESSION_COOKIE` from `@paigasus/auth`

**Files:**
- Modify: `ts/packages/paigasus-auth/src/server.ts`
- Modify: `ts/apps/iam-console/lib/auth.ts`, `ts/apps/iam-console/tests/unit/session-cookie.test.ts`

**Interfaces:**
- Produces: `SESSION_COOKIE` from `@paigasus/auth/server`.

`@paigasus/auth` defines `SESSION_COOKIE = '__Host-pgs_sid'` at `src/http/cookies.ts:21` but exports
it from no public entry, so `iam-console/lib/auth.ts` re-declares it as `SESSION_COOKIE_NAME`. A
second app would make a third copy of a constant that must agree across every zone or the shared
session silently stops working.

- [ ] **Step 1: Re-export it**

Add to `ts/packages/paigasus-auth/src/server.ts`:

```ts
export { SESSION_COOKIE } from './http/cookies';
```

- [ ] **Step 2: Delete the app's copy**

Remove `SESSION_COOKIE_NAME` from `lib/auth.ts` and import `SESSION_COOKIE` from
`@paigasus/auth/server` at every use. Update `tests/unit/session-cookie.test.ts` to assert the
imported constant is what the middleware treats as the session — keep the assertion, only change
where the name comes from.

- [ ] **Step 3: Verify and commit**

```bash
moon run paigasus-auth-ts:test && moon run iam-console-ts:test
git add ts/packages/paigasus-auth ts/apps/iam-console
git commit -m "refactor(ts): export SESSION_COOKIE from @paigasus/auth/server (SMA-512)"
```

---

### Task 3: The middle layer — correlation, errors, clients, resolver

**Files:**
- Move: `lib/{correlation.ts,errors.ts,iam-clients.ts,principal-resolver.ts}` → package `src/`
- Move: `tests/unit/{correlation.test.ts,call-iam.test.ts}`,
  `tests/integration/{principal.test.ts,error-info-round-trip.test.ts}` → package `tests/`
- Modify: package `src/index.ts`, and every app importer

**Interfaces:**
- Consumes: Task 1's leaf exports.
- Produces: `callIam`, `type IamResult<T>`, `type ActionState`, `requestCorrelationId`,
  `requestPath`, `FORBIDDEN_VIEW_CORRELATION`, `createIamClients`, `type IamClients`,
  `createIntrospectPrincipalResolver`.

**The one shape change in this task.** `iam-clients.ts` currently exports `iamClientsForToken`,
which calls `getRuntimeConfig().PAIGASUS_IAM_GRPC_URL` — app config the package must not read.
`createIamClients({ baseUrl, token, correlationId })` is already the pure factory underneath.
**Move `createIamClients` only; leave `iamClientsForToken` in the app** as a four-line wrapper that
supplies `baseUrl` from its own config. Task 5 removes it when the factory takes over.

- [ ] **Step 1: Move the files**

```bash
git mv ts/apps/iam-console/lib/correlation.ts        ts/packages/paigasus-console-core/src/
git mv ts/apps/iam-console/lib/errors.ts             ts/packages/paigasus-console-core/src/
git mv ts/apps/iam-console/lib/iam-clients.ts        ts/packages/paigasus-console-core/src/
git mv ts/apps/iam-console/lib/principal-resolver.ts ts/packages/paigasus-console-core/src/
git mv ts/apps/iam-console/tests/unit/correlation.test.ts ts/packages/paigasus-console-core/tests/unit/
git mv ts/apps/iam-console/tests/unit/call-iam.test.ts    ts/packages/paigasus-console-core/tests/unit/
mkdir -p ts/packages/paigasus-console-core/tests/integration
git mv ts/apps/iam-console/tests/integration/principal.test.ts             ts/packages/paigasus-console-core/tests/integration/
git mv ts/apps/iam-console/tests/integration/error-info-round-trip.test.ts ts/packages/paigasus-console-core/tests/integration/
```

- [ ] **Step 2: Split `iam-clients.ts`**

In the moved `src/iam-clients.ts`, delete `iamClientsForToken` and its `getRuntimeConfig` import.
Keep `createIamClients` and `type IamClients` exactly as they are.

Re-create `iamClientsForToken` in `ts/apps/iam-console/lib/iam-clients.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// TEMPORARY (SMA-512 PR 2, task 3). The pure factory moved to @paigasus/console-core; this wrapper
// supplies the app's own baseUrl until task 5's createConsoleRuntime takes over. Delete it there.
import 'server-only';
import { createIamClients, type IamClients } from '@paigasus/console-core';
import { getRuntimeConfig } from './config';

/** The clients for a token the caller already holds. No session lookup — used at login. */
export function iamClientsForToken(token: string, correlationId: string | null = null): IamClients {
  return createIamClients({ baseUrl: getRuntimeConfig().PAIGASUS_IAM_GRPC_URL, token, correlationId });
}
```

- [ ] **Step 3: Extend `src/index.ts`**

```ts
export { callIam, type ActionState, type IamResult } from './errors';
export { FORBIDDEN_VIEW_CORRELATION, requestCorrelationId, requestPath } from './correlation';
export { createIamClients, type IamClients } from './iam-clients';
export { createIntrospectPrincipalResolver } from './principal-resolver';
```

- [ ] **Step 4: Re-point every app importer**

```bash
grep -rn "from '\.\./\?\.\?\./\?lib/\(correlation\|errors\|iam-clients\|principal-resolver\)'" ts/apps/iam-console/
grep -rn "from '\./\(correlation\|errors\|principal-resolver\)'" ts/apps/iam-console/lib/
```

`lib/errors.ts` alone has 14 app importers. `lib/correlation.ts` has 2. Change each to
`@paigasus/console-core`, except `./iam-clients`, which still resolves to the app wrapper.

The moved tests' relative imports change from `../../lib/x` to `../../src/x`, and their
`tests/support/` imports need a path that still resolves — Task 6 moves those fakes; until then,
point them at `../../../apps/iam-console/tests/support/x`. **Record this as temporary in a comment
on each such import**, so Task 6 has a grep target:

```ts
// TEMPORARY (SMA-512 PR 2, task 3 → task 6): the fakes move to ./testing in task 6.
```

- [ ] **Step 5: Verify and commit**

```bash
moon run paigasus-console-core-ts:test && moon run iam-console-ts:test && moon run ts:lint
git add -A && git commit -m "refactor(ts): move the correlation, error and client layer to console-core (SMA-512)"
```

---

### Task 4: The cached accessors — iam, principal, authorize, scopes, discovery

**Files:**
- Move: `lib/{iam.ts,principal.ts,authorize.ts,scopes.ts,discovery.ts}` → package `src/`
- Move: the matching tests
- Modify: package `src/index.ts`

**Interfaces:**
- Consumes: Tasks 1 and 3.
- Produces: the five modules, still exporting their module-scope `cache()` accessors. Task 5
  converts them to a factory.

**This task is a straight move with one substitution.** All five read app state only through
`authRuntime()` and `getRuntimeConfig()`. Replace those two with **module-level injection points**
that Task 5 fills:

```ts
// src/runtime-ports.ts
// SPDX-License-Identifier: Apache-2.0
import 'server-only';
import type { AuthRuntime } from '@paigasus/auth/server';
import type { ConsoleCoreConfig } from './config-shape';

type Ports = { authRuntime: () => Promise<AuthRuntime>; config: () => ConsoleCoreConfig };
let ports: Ports | null = null;

/** Set once, at module scope, by the app's createConsoleRuntime() call (task 5). */
export function setConsolePorts(next: Ports): void { ports = next; }

export function consolePorts(): Ports {
  if (ports === null) {
    throw new Error('@paigasus/console-core: createConsoleRuntime() was never called. An app must call it once, at module scope, before any accessor runs.');
  }
  return ports;
}
```

`src/config-shape.ts` declares the minimum the package needs, which each app's `ConsoleConfig`
structurally satisfies:

```ts
// SPDX-License-Identifier: Apache-2.0
import 'server-only';
import type { authEnvShape } from '@paigasus/auth/server';
import type { discoveryEnvShape } from '@paigasus/discovery/server';

/**
 * The config slice this package reads. Declared structurally rather than importing an app's
 * ConsoleConfig, so both zones satisfy it without the package knowing either app exists.
 */
export type ConsoleCoreConfig = {
  readonly PAIGASUS_IAM_GRPC_URL: string;
  readonly PAIGASUS_SESSION_STORE: 'redis' | 'memory';
  readonly PAIGASUS_SESSION_REDIS_URL?: string | undefined;
  readonly PAIGASUS_SERVICES: Readonly<Record<string, string>>;
};
```

Widen that type if `timingsFromEnv` needs more keys — read `@paigasus/discovery/server`'s
`timingsFromEnv` signature and include exactly what it reads. Do not guess.

- [ ] **Step 1: Move the five files and their tests**

```bash
for f in iam principal authorize scopes discovery; do
  git mv "ts/apps/iam-console/lib/$f.ts" ts/packages/paigasus-console-core/src/
done
git mv ts/apps/iam-console/tests/unit/authorize.test.ts        ts/packages/paigasus-console-core/tests/unit/
git mv ts/apps/iam-console/tests/integration/authorize.test.ts ts/packages/paigasus-console-core/tests/integration/
git mv ts/apps/iam-console/tests/integration/discovery.test.ts ts/packages/paigasus-console-core/tests/integration/
git mv ts/apps/iam-console/tests/integration/scopes.test.ts    ts/packages/paigasus-console-core/tests/integration/
```

`tests/unit/switcher-orgs.test.ts` covers `switcherOrgs` from `scopes.ts` **and** the `(console)`
layout. Split it: the `switcherOrgs` cases move, the layout cases stay.

- [ ] **Step 2: Substitute the ports**

In the moved files, replace `import { getRuntimeConfig } from './config'` with
`import { consolePorts } from './runtime-ports'` and each `getRuntimeConfig()` call with
`consolePorts().config()`. Replace `import { authRuntime } from './auth'` with the same and
`authRuntime()` with `consolePorts().authRuntime()`.

`src/iam-clients.ts` regains a `baseUrl` source this way — but leave the app's wrapper in place
until Task 5, so this task changes no app file.

- [ ] **Step 3: Keep the app compiling with a temporary shim**

`ts/apps/iam-console/lib/iam.ts` and the other four app paths are gone, so every app importer
breaks. Rather than re-point 40 files twice, add one temporary re-export barrel per moved module:

```ts
// ts/apps/iam-console/lib/iam.ts
// SPDX-License-Identifier: Apache-2.0
// TEMPORARY (SMA-512 PR 2, task 4 → task 5). A barrel so app files keep compiling while the
// package takes ownership. Task 5 deletes these and re-points the app at lib/console.ts.
import 'server-only';
export { currentSession, iamClients, iamClientsForAction, optionalSession, sessionToken, type IamClients } from '@paigasus/console-core';
```

Do the same for `principal.ts`, `authorize.ts`, `scopes.ts`, `discovery.ts`. Each carries the same
TEMPORARY comment so Task 5 has a grep target.

- [ ] **Step 4: Call `setConsolePorts` from the app, once**

In `ts/apps/iam-console/lib/config.ts`, at the very bottom:

```ts
// TEMPORARY (SMA-512 PR 2, task 4 → task 5): task 5 replaces this with lib/console.ts's
// createConsoleRuntime() call. Module scope is correct and safe — both fields are THUNKS, so
// nothing reads the environment here, and getRuntimeConfig() throws during phase-production-build
// if it ever did.
setConsolePorts({ authRuntime: () => authRuntime(), config: () => getRuntimeConfig() });
```

Watch for an import cycle: `config.ts` → `auth.ts` → `config.ts`. If it appears, put the
`setConsolePorts` call in `lib/auth.ts` instead, which already imports both.

- [ ] **Step 5: Verify**

```bash
moon run paigasus-console-core-ts:test && moon run iam-console-ts:test && moon run iam-console-ts:build
moon run ts:lint && moon run ts:fmt
```

`iam-console-ts:build` matters here specifically: a module-scope config read would fail it during
`phase-production-build`, which is the failure this port design exists to avoid.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(ts): move the cached console accessors to console-core (SMA-512)"
```

---

### Task 5: `createConsoleRuntime`, and deleting the shims

**Files:**
- Create: `ts/packages/paigasus-console-core/src/runtime.ts`
- Create: `ts/apps/iam-console/lib/console.ts`
- Delete: the five temporary barrels and `lib/iam-clients.ts`
- Modify: every app importer, `lib/auth.ts`, `lib/config.ts`

**Interfaces:**
- Produces:

```ts
export function createConsoleRuntime(deps: {
  config: () => ConsoleCoreConfig;
  authRuntime: () => Promise<AuthRuntime>;
  logger: ConsoleLogger;
}): ConsoleRuntime;

export type ConsoleRuntime = {
  currentSession: () => Promise<ResolvedSession>;
  optionalSession: () => Promise<ResolvedSession | null>;
  sessionToken: () => Promise<string>;
  iamClients: () => Promise<IamClients>;
  iamClientsForAction: () => Promise<IamResult<IamClients>>;
  iamClientsForToken: (token: string, correlationId?: string | null) => IamClients;
  currentPrincipal: () => Promise<IamResult<Principal>>;
  mayI: () => Promise<MayI>;
  myScopes: () => Promise<IamResult<MyScopes>>;
  discovery: () => Discovery;
};
```

**Two rules the implementation depends on. Both are correctness, not style.**

1. **`createConsoleRuntime` must be called EXACTLY ONCE per app, at module scope**, in
   `lib/console.ts`, whose exports the app re-uses everywhere. Each accessor is a `cache()` wrapper,
   and a factory makes fresh wrappers on every call. Two calls means two memoization identities and
   a second `Introspect`, `ListRoleGrants` and up to 50 tenancy reads per render. **Outside a React
   server render `cache()` is a pass-through, so no vitest tier can observe the difference** — the
   only assertion that can is an e2e `Introspect` count, which PR 3 owns.
2. **`iamClientsForAction` is deliberately NOT memoized.** It returns a `relogin` failure rather
   than redirecting, so a Server Action can render an inline error. Keep it a plain async function.

`createIntrospectPrincipalResolver` is **not** on `ConsoleRuntime` and must stay a separate export:
the console runtime takes the auth runtime, and the auth runtime takes the resolver. Building it
inside would be circular.

- [ ] **Step 1: Write `src/runtime.ts`**

Move the five modules' `cache()` bodies into a factory closure. The module-level `export const
currentSession = cache(...)` in `src/iam.ts` becomes a field built inside `createConsoleRuntime`.
Keep the pure helpers (`createMayI`, `loadMyScopes`, `introspectWithProvisioning`,
`createAppDiscovery`, `descriptorCacheFor`) exported as they are — the tests drive those directly
and must keep doing so.

Delete `src/runtime-ports.ts` and its `setConsolePorts` call: the factory replaces it.

- [ ] **Step 2: Write the app's `lib/console.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The app's ONE call to createConsoleRuntime, at module scope. Called once per module graph, which
// is a correctness requirement, not a convention: each accessor is a React cache() wrapper, and a
// second call would make a second memoization identity. Outside a server render cache() is a
// pass-through, so no unit or integration test can catch a regression here — only an e2e Introspect
// count can.
import 'server-only';
import { createConsoleRuntime, logger } from '@paigasus/console-core';
import { authRuntime } from './auth';
import { getRuntimeConfig } from './config';

export const {
  currentSession, optionalSession, sessionToken,
  iamClients, iamClientsForAction, iamClientsForToken,
  currentPrincipal, mayI, myScopes, discovery,
} = createConsoleRuntime({ config: () => getRuntimeConfig(), authRuntime, logger });
```

- [ ] **Step 3: Delete the shims and re-point every importer**

```bash
git rm ts/apps/iam-console/lib/{iam,principal,authorize,scopes,discovery,iam-clients}.ts
grep -rn "TEMPORARY (SMA-512 PR 2" ts/            # every remaining marker must be gone by the end
```

Re-point every `from './iam'`, `'../lib/iam'`, `'./authorize'`, `'./scopes'`, `'./discovery'`,
`'./principal'` in `app/**` and `lib/**` to `./console` / `../lib/console` as appropriate.

**`tests/unit/actions-structure.test.ts` pins import specifier strings** (`'../lib/iam'`,
`'../lib/iam-clients'`, `'../lib/authorize'`) to prove no Server Action reaches a client except
through `iamClientsForAction()`. Update those literals to the new paths and **keep the assertion
intact** — it is the control that stops an action bypassing the session check.

- [ ] **Step 4: Re-wire `lib/auth.ts`**

It now composes the resolver from package exports:

```ts
import { createIntrospectPrincipalResolver, logger, requestCorrelationId } from '@paigasus/console-core';
import { iamClientsForToken } from './console';
```

Remove the `setConsolePorts` call added in Task 4.

- [ ] **Step 5: Verify**

```bash
moon run paigasus-console-core-ts:test && moon run iam-console-ts:test
moon run iam-console-ts:build && moon run iam-console-ts:typecheck
moon run ts:lint && moon run ts:fmt
grep -rn "TEMPORARY (SMA-512 PR 2" ts/ && echo "MARKERS REMAIN — not done" || echo "no temporary markers left"
```

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(ts): introduce createConsoleRuntime and retire the app shims (SMA-512)"
```

---

### Task 6: Move the test doubles to `./testing`

**Files:**
- Move: `ts/apps/iam-console/tests/support/{fake-iam.ts,fake-idp.ts,tls.ts,tls-terminator.ts}` →
  `ts/packages/paigasus-console-core/testing/`
- Create: `ts/packages/paigasus-console-core/testing/index.ts`
- Modify: every importer in `iam-console`'s tests

**Interfaces:**
- Produces, from `@paigasus/console-core/testing`: `startFakeIam`, `denial`, `errorInfoOf`,
  `IAM_ERROR_DOMAIN`, `FAKE_IAM_ISSUER` and the `FakeIam*` types; `startFakeIdp`, `type FakeIdp`;
  `testTls`, `type TlsMaterial`; `startTlsTerminator`.

**This is safe to do late, and that is deliberate.** The four fakes import **nothing** from `lib/`
— verified — so they are decoupled from every earlier task. `env.ts`, `msw.ts`, `next-cache.ts`,
`next-headers.ts`, `server-only-stub.ts` and `setup.ts` **stay in the app**: they are vitest
harness wiring, not doubles a second app would share.

- [ ] **Step 1: Move the four files**

```bash
mkdir -p ts/packages/paigasus-console-core/testing
for f in fake-iam fake-idp tls tls-terminator; do
  git mv "ts/apps/iam-console/tests/support/$f.ts" ts/packages/paigasus-console-core/testing/
done
```

- [ ] **Step 2: Write `testing/index.ts`**

No `import 'server-only'` — that is the point of the placement. Add a header saying so:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The testing surface. OUTSIDE src/ and deliberately NOT server-only guarded: vitest and Playwright
// harnesses import it outside a Next server. That placement is what lets the src/ rule — every file
// imports 'server-only' — hold with no exception.
export { denial, errorInfoOf, FAKE_IAM_ISSUER, IAM_ERROR_DOMAIN, startFakeIam, type FakeIam, type FakeIamCall, type FakeIamContext, type FakeIamHandlers, type FakeIamMethod, type ServiceDescriptorBody } from './fake-iam';
export { startFakeIdp, type FakeIdp } from './fake-idp';
export { testTls, type TlsMaterial } from './tls';
export { startTlsTerminator } from './tls-terminator';
```

- [ ] **Step 3: Re-point every importer**

```bash
grep -rn "tests/support/\(fake-iam\|fake-idp\|tls\|tls-terminator\)\|from '\./\(fake-iam\|fake-idp\|tls\|tls-terminator\)'" ts/
```

Both the app's tests and the package's own moved tests (Task 3's TEMPORARY comments) import these.
All become `@paigasus/console-core/testing`.

- [ ] **Step 4: Verify and commit**

```bash
moon run paigasus-console-core-ts:test && moon run iam-console-ts:test && moon run iam-console-ts:test-e2e
git add -A && git commit -m "refactor(ts): move the console test doubles into @paigasus/console-core/testing (SMA-512)"
```

`test-e2e` matters: the Playwright harness uses all four fakes.

---

### Task 7: The affected-graph re-baselines

**Files:**
- Modify: `ci/affected-graph/run.sh`

**Interfaces:** none; this task makes CI agree with the new graph.

`ci/affected-graph/run.sh` compares expected task sets with **strict equality**, so every case the
new project joins must be re-baselined in the same pull request. Spec § 3.1 lists them:

| Case (line, re-derive before editing) | Change |
|---|---|
| `auth->auth-tasks` | add `paigasus-console-core-ts` tasks |
| `proto->sdk`, `proto-iam->sdk` | add console-core tasks |
| `discovery->discovery-tasks`, `discovery-adapters->discovery-tasks` | add console-core tasks |
| `sdk->iam-console`, `sdk-errors->iam-console` | add console-core tasks |
| `iam-console-lib->iam-console-tasks` | **re-anchor** — its anchor is `ts/apps/iam-console/lib/iam.ts`, which no longer exists. Use `lib/config.ts`, and say why in the case comment. |
| new: `console-core->consumers` | two anchors inside the new package |

- [ ] **Step 1: Re-derive the real expected sets**

Do not hand-write them. For each anchor:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query tasks --affected --base origin/main   # with the anchor file touched
```

Parse the JSON and take **one target per `tasks[project][task]`**. A `grep -o '"target": "[^"]*"'`
over the raw JSON counts SCHEDULED upstreams as if they were SELECTIONS — it reported 15 tasks for
a `.prototools` edit where the real answer is 12.

- [ ] **Step 2: Add the new case**

Two anchors, not one: a single anchor leaves the other subtree narrowable while the case stays
green. Use `src/iam.ts` and `src/prn-tenancy.ts`.

- [ ] **Step 3: Run the suite**

```bash
/bin/bash ci/affected-graph/run.sh --self-test
/bin/bash ci/affected-graph/run.sh --negative-control
/bin/bash ci/affected-graph/run.sh
```

System `/bin/bash` 3.2, never 5.3.15.

- [ ] **Step 4: Commit**

```bash
git add ci/affected-graph/run.sh
git commit -m "ci(ts): re-baseline the affected-graph cases for console-core (SMA-512)"
```

---

### Task 8: Documentation and the full gate run

**Files:**
- Modify: `CLAUDE.md`, `ts/README.md`,
  `docs/superpowers/specs/2026-09-13-sma-512-gateway-console-design.md`

- [ ] **Step 1: Correct the spec**

Record the three corrections from this plan's own header: `tenancy-path.ts` stays; the factory takes
`config` rather than `iamGrpcUrl`; `SESSION_COOKIE` is solved upstream. Update § 5.2, § 5.3 and
§ 5.4 to describe what shipped.

- [ ] **Step 2: Record the package in `CLAUDE.md` and `ts/README.md`**

One paragraph: what it is, why it may import both `auth` and `sdk`, the exactly-once factory rule
and why no unit test can catch a violation, and that `testing/` is outside `src/` on purpose.

- [ ] **Step 3: Run the full gate graph**

Use the marker-delimited `moon ci` command in `CLAUDE.md` verbatim, with
`--base origin/main --include-relations`. Read it from the file; do not retype it.

Expect `repo:affected-smoke`, `repo:input-liveness` and `repo:actionlint` to be the plausible reds.
CODEOWNERS is Moon-generated and regenerates on a new project — do not hand-edit it; `ci.yml` runs
`moon sync code-owners` and diffs.

- [ ] **Step 4: Commit and push**

```bash
git add -A && git commit -m "docs(ts): record @paigasus/console-core (SMA-512)"
git push -u origin <branch>
```

## Rules that the code depends on

- `createConsoleRuntime` is called exactly once per app, at module scope. No unit or integration
  test can catch a violation.
- `iamClientsForAction` is not memoized, on purpose.
- `createIntrospectPrincipalResolver` stays a separate export; putting it on `ConsoleRuntime` is
  circular.
- `principal-prn.ts` moves with both its callers and must never be separated from them. Its header
  records the production bug that splitting the reading caused: the resolver normalised `'' → null`
  while the live path passed the empty string through, `mayI()` asked IAM about a principal it could
  not name, failed open, and every mutation control rendered.
- The reverse boundary rule is never a `packages/**` `no-restricted-imports` block.
- `testing/` is outside `src/` and carries no `server-only` guard.

## Known limits

- No test proves the exactly-once factory rule. PR 3's e2e `Introspect` count is the first control.
- The reverse boundary rule ("only apps may import console-core") is **not** implemented — it needs
  a custom named rule in `sourceRules`. Record it as a follow-up; the `app-middleware` ban added in
  Task 1 covers the case that actually leaks tokens.
- Spec § 5.5 rule 3 ("only `apps/*/tests/**` may import `@paigasus/console-core/testing`") is also
  **not** implemented, for the same reason as the reverse rule above and needing the same mechanism
  — a custom named rule in `sourceRules`, never a second `packages/**` block. `@paigasus/console-core`
  is a production `dependencies` entry of `iam-console` and the `testing` subpath carries no
  `server-only` guard by design, so today an `app/page.tsx` could import `startFakeIam` and
  `next build` would succeed, bundling an in-process gRPC fake into production code. This and the
  reverse rule above are one piece of follow-up work, not two.
