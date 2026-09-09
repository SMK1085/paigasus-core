<!-- SPDX-License-Identifier: Apache-2.0 -->

# `@paigasus/sdk` — package shape, Connect-ES transport and IAM clients (SMA-508) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `ts/packages/paigasus-sdk` from a one-line stub into a real server-only package that
builds typed Connect-ES clients over the seven IAM gRPC services, with a cached transport, per-call
bearer attachment, and the Moon/CI wiring that makes the `@paigasus/proto` edge real.

**Architecture:** One `import 'server-only'` site (`src/server-guard.ts`) that every guarded entry
imports first. A module-level transport cache keyed on a stable serialization of the whole options
object, holding both the `Transport` and its `Http2SessionManager` so the package can dispose them.
Authorization never enters the cached transport: an interceptor on the transport reads the token from
per-call `ContextValues`, and the client factory — not the transport factory — takes the token and
binds it. Two affected-graph cases make the `@paigasus/proto` → `@paigasus/sdk` edge a proven
selection rather than a declared one.

**Tech Stack:** TypeScript 6.0.3, `@connectrpc/connect` 2.2.0, `@connectrpc/connect-node` 2.2.0,
`@bufbuild/protobuf` 2.14.1, vitest 5.0.0, Node 24.16.0, pnpm 11.3.0, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-09-sma-508-sdk-design.md` §§ 6, 7, 11.1, 11.2
(this is **PR B of three**; § 14.1 draws the split — PR A is SMA-624, merged as `9f57d6e4`;
PR C is SMA-625).

---

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Branch `feature/sma-508-ts-paigasus-sdk-transport`; conventional commits scoped `feat(ts):` /
  `fix(ts):` / `build(ts):` / `test(ts):` / `ci(...)`.
- Work happens in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-508`.
  It is already provisioned (`proto install`, `pnpm -C ts install`, `uv sync`, `cargo fetch`).
- Every shell command that reaches `moon`, `pnpm`, `uv` or `buf` must first
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Relative imports carry the `.js` extension (`./transport.js`), matching
  `ts/packages/paigasus-proto/src/iam.ts`.
- `ts/tsconfig.base.json` sets `verbatimModuleSyntax: true`, `exactOptionalPropertyTypes: true`,
  `noUncheckedIndexedAccess: true` and `strict: true`. A type-only import must be written
  `import type`, and an optional property may not be assigned an explicit `undefined`.
- Moon task `inputs` are the **only** thing that confers affectedness. `dependsOn` schedules an
  upstream and never selects a downstream. Both are required, and inputs are **appended**, never
  `options.merge: replace`.
- Do **not** bypass the `commit-msg` hook with `--no-verify`.
- Package stays `private: true` and source-only. No `dist/`, no `tsup`.

### Two decisions this plan takes, stated plainly

**D1 — PR B ships two entry points, not five.** The Linear issue's Scope names five
(`.`, `./iam`, `./chat`, `./errors`, `./errors/types`), but `./chat` needs spec § 8 and `./errors`
needs spec § 9, and § 14.1 assigns both to **PR C**. An `exports` map entry pointing at a stub is a
public API promise the package does not keep: a consumer importing `@paigasus/sdk/errors` would get
an empty module, which is worse than a resolution error. Spec § 6.2 layer 3 settles it — the
structural test is *"driven off `package.json`, so a sixth entry point is covered the day it is
added."* PR B therefore ships `.` and `./iam`, and the AC 1 test covers PR C's three entries the day
PR C adds them, with no edit to the test.

**D2 — a caller-supplied `contextValues` is a thrown error, not a silent drop.** `ContextValues`
exposes `get`/`set`/`delete` and no iteration, so the client factory cannot copy a caller's object.
That leaves three options: drop the caller's values silently, mutate the caller's object (which
writes the bearer into something the caller holds and can reuse against another client — the
cross-request leak spec § 7.5 exists to prevent), or refuse. This plan refuses, with a message
naming the reason. Nothing in the repo passes `contextValues` today.

### One correction to the spec, measured during planning

Spec § 7.1 says *"`createGrpcTransport` also takes `nodeOptions` (TLS trust material),
`defaultTimeoutMs` and `httpVersion`."* **`httpVersion` is not a field of `GrpcTransportOptions`.**
Measured against the published `@connectrpc/connect-node@2.2.0` tarball:
`GrpcTransportOptions = NodeHttp2TransportOptions & { baseUrl, useBinaryFormat?, interceptors?,
jsonOptions?, binaryOptions?, acceptCompression?, sendCompression?, compressMinBytes?, readMaxBytes?,
writeMaxBytes?, defaultTimeoutMs? }`. `createGrpcTransport` is HTTP/2 only; `httpVersion` is a field
of `NodeTransportOptions`, which `createConnectTransport` and `createGrpcWebTransport` take.
`nodeOptions` and `defaultTimeoutMs` are real and are exactly as the spec describes. The design does
not change — the cache key is still the whole options object — only the enumeration does.

---

## File Structure

**Created:**

| File | Responsibility |
|---|---|
| `ts/packages/paigasus-sdk/vitest.config.ts` | node environment; **both** `resolve.conditions` and `ssr.resolve.conditions` |
| `ts/packages/paigasus-sdk/src/server-guard.ts` | the single `import 'server-only'` site |
| `ts/packages/paigasus-sdk/src/transport.ts` | transport cache, stable key, `Auth`, auth context key, auth interceptor, `disposeTransports` |
| `ts/packages/paigasus-sdk/src/iam.ts` | `createIamClient`, the per-client auth binding, the seven service re-exports |
| `ts/packages/paigasus-sdk/tests/server-guard.test.ts` | AC 1, structural, driven off `package.json` |
| `ts/packages/paigasus-sdk/tests/transport.test.ts` | cache identity, stable key, dispose, interceptor header |
| `ts/packages/paigasus-sdk/tests/transport-wiring.test.ts` | AC 6 — the real transport carries the interceptor and the 10 s deadline (needs `vi.mock`, so its own file) |
| `ts/packages/paigasus-sdk/tests/iam.test.ts` | AC 5 — cross-client token isolation, and D2's refusal |

**Modified:**

| File | Change |
|---|---|
| `ts/pnpm-workspace.yaml` | two catalog entries, `@connectrpc/connect` and `@connectrpc/connect-node`, both `^2.2.0` |
| `ts/packages/paigasus-sdk/package.json` | `exports` gains `./iam`; `dependencies` and `devDependencies` |
| `ts/packages/paigasus-sdk/tsconfig.json` | `types: ["node"]`, drop `rootDir`/`outDir`, widen `include` |
| `ts/packages/paigasus-sdk/src/index.ts` | guard + re-export `./iam` |
| `ts/packages/paigasus-sdk/moon.yml` | `dependsOn` + appended `inputs` on `build`/`typecheck`/`test` |
| `ci/affected-graph/run.sh` | `contracts->proto` gains `paigasus-sdk-ts`; new `proto->sdk` task case |
| `ts/pnpm-lock.yaml` | written by `pnpm install` |

---

## Task 1: Catalog entries, package manifest, and dependency install

The spec's M2 and M3 were measured outside the workspace install tree (`npm pack` plus a standalone
`tsc`), and spec § 13 requires them re-confirmed once the catalog entries land, because pnpm catalog
resolution is what CI uses. That re-confirmation is this task's test.

**Files:**
- Modify: `ts/pnpm-workspace.yaml` (catalog block, after the `@bufbuild/protobuf` entry)
- Modify: `ts/packages/paigasus-sdk/package.json`
- Modify: `ts/pnpm-lock.yaml` (generated)
- Test: `ts/packages/paigasus-sdk/tests/deps.test.ts` (temporary — deleted in Task 4, see Step 8)

**Interfaces:**
- Consumes: nothing.
- Produces: `@connectrpc/connect` and `@connectrpc/connect-node` resolvable from
  `ts/packages/paigasus-sdk`; `@paigasus/proto` resolvable as a workspace dependency. Later tasks
  import `createClient`, `createContextKey`, `createContextValues`, `type Interceptor`,
  `type Client`, `type CallOptions`, `type Transport` from `@connectrpc/connect`, and
  `createGrpcTransport`, `Http2SessionManager` from `@connectrpc/connect-node`.

- [ ] **Step 1: Add the two catalog entries**

In `ts/pnpm-workspace.yaml`, inside the `catalog:` block, directly after the two lines

```yaml
  # Protobuf-ES v2 runtime (SMA-389)
  '@bufbuild/protobuf': ^2.14.1
```

insert:

```yaml
  # Connect-ES v2 — the gRPC transport and typed clients @paigasus/sdk builds on (SMA-508,
  # ADR-0018). `@connectrpc/connect` is the protocol-independent client half (createClient,
  # ContextValues, Interceptor); `@connectrpc/connect-node` supplies createGrpcTransport, which is
  # HTTP/2 ONLY — it has no `httpVersion` option, unlike createConnectTransport and
  # createGrpcWebTransport. Both are pinned at the same minor on purpose: connect-node declares a
  # peer range on connect, and a split would let the two drift across a protocol change.
  # connect-node's `@bufbuild/protobuf` peer range is ^2.7.0, which the entry above satisfies.
  '@connectrpc/connect': ^2.2.0
  '@connectrpc/connect-node': ^2.2.0
```

- [ ] **Step 2: Rewrite the SDK manifest**

Replace `ts/packages/paigasus-sdk/package.json` entirely with:

```json
{
  "name": "@paigasus/sdk",
  "_comment_exports": "Source-only exports. Bundler-aware consumers only (Next/Vitest/tsc walk through TS via moduleResolution: bundler). Switch to ./dist/index.js when tsup wiring lands — must happen IN LOCKSTEP with flipping `private: false` for publishable packages.",
  "_comment_entries": "Two entry points when this package is finished, not the five the design spec's § 6.1 lists. `./chat` needs spec § 8 and `./errors` needs spec § 9, and § 14.1 assigns both to PR C (SMA-625). An exports entry pointing at a stub is a public API promise this package would not keep — a consumer importing @paigasus/sdk/errors would get an empty module, which is worse than a resolution error. tests/server-guard.test.ts is driven off THIS map, so PR C's three entries are covered the day they are added, with no edit to the test. `./iam` is added in Task 4, in the same commit as the file it points at.",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "engines": {
    "node": ">=24"
  },
  "license": "Apache-2.0",
  "description": "Server-only Connect-ES clients for the Paigasus IAM gRPC services.",
  "exports": {
    ".": "./src/index.ts"
  },
  "scripts": {
    "typecheck": "tsc -p tsconfig.json --noEmit"
  },
  "dependencies": {
    "@bufbuild/protobuf": "catalog:",
    "@connectrpc/connect": "catalog:",
    "@connectrpc/connect-node": "catalog:",
    "@paigasus/proto": "workspace:*",
    "server-only": "catalog:"
  },
  "devDependencies": {
    "@types/node": "catalog:",
    "typescript": "catalog:",
    "vitest": "catalog:"
  }
}
```

- [ ] **Step 3: Install**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts install
```

Expected: exit 0, and `ts/pnpm-lock.yaml` gains `@connectrpc/connect@2.2.0` and
`@connectrpc/connect-node@2.2.0`.

If pnpm reports `ERR_PNPM_...` about release age, read spec § 11.5: `@connectrpc/connect` 2.2.0
published 2026-09-07 and pnpm 11's `minimumReleaseAge` is 24 hours, which is satisfied — any
release-age failure is about a different package and waiting is the fix.

- [ ] **Step 4: Write the failing re-confirmation test (spec M2 + M3)**

Create `ts/packages/paigasus-sdk/tests/deps.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// TEMPORARY. Re-confirms spec § 13's M2 and M3 against the WORKSPACE resolution rather than the
// standalone `npm pack` tree they were first measured in — spec § 13 requires exactly that once the
// catalog entries land, because pnpm catalog resolution is what CI uses. Deleted in Task 4, once
// the real suites import the same symbols and prove the same thing by using them.
import { describe, expect, it } from 'vitest';
import { createClient, createContextKey, createContextValues } from '@connectrpc/connect';
import { createGrpcTransport, Http2SessionManager } from '@connectrpc/connect-node';
import { TenancyService } from '@paigasus/proto/iam';

describe('M2 — connect 2.2.0 context-value API resolves from the workspace', () => {
  it('exports createContextKey and createContextValues', () => {
    expect(typeof createContextKey).toBe('function');
    expect(typeof createContextValues).toBe('function');
  });

  it('round-trips a value through a context key', () => {
    const key = createContextKey<string>('default');
    expect(createContextValues().get(key)).toBe('default');
    expect(createContextValues().set(key, 'set').get(key)).toBe('set');
  });
});

describe('M3 — the node transport and the typed client resolve from the workspace', () => {
  it('exports createGrpcTransport and Http2SessionManager', () => {
    expect(typeof createGrpcTransport).toBe('function');
    expect(typeof Http2SessionManager).toBe('function');
  });

  it('builds a typed client over the TenancyService descriptor', () => {
    const transport = createGrpcTransport({ baseUrl: 'https://iam.invalid' });
    const client = createClient(TenancyService, transport);
    expect(typeof client.createOrganization).toBe('function');
  });
});
```

- [ ] **Step 5: Run it and RECORD the result**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-sdk exec vitest run
```

**This step is a measurement, not a TDD red gate.** It re-confirms the design spec's M2 and M3
against the workspace resolution, which spec § 13 requires once the catalog entries land. **PASS and
FAIL are both acceptable outcomes here** — record which you got in your report and move on.

A pass means vitest's default `include` already matched the file and `@paigasus/proto`'s
unconditional `exports` strings resolved without help; the measurement is simply complete early. A
failure means the resolution needs the conditions Task 2 adds. **Do not "fix" a passing run to make
it fail, and do not fix a failing run here** — Task 2 owns the vitest config either way.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-508
git add ts/pnpm-workspace.yaml ts/packages/paigasus-sdk/package.json ts/pnpm-lock.yaml ts/packages/paigasus-sdk/tests/deps.test.ts
git commit -m "build(ts): add Connect-ES catalog entries and @paigasus/sdk dependencies

Two catalog entries at ^2.2.0 and the SDK's own dependency set. The
temporary tests/deps.test.ts re-confirms the design spec's M2 and M3
against the workspace resolution, as spec § 13 requires; it does not
pass until the vitest config in the next commit supplies the resolution
conditions.

Refs SMA-508"
```

---

## Task 2: tsconfig, vitest config, the server guard, and the AC 1 structural test

**Files:**
- Create: `ts/packages/paigasus-sdk/vitest.config.ts`
- Create: `ts/packages/paigasus-sdk/src/server-guard.ts`
- Modify: `ts/packages/paigasus-sdk/tsconfig.json`
- Test: `ts/packages/paigasus-sdk/tests/server-guard.test.ts`

**Interfaces:**
- Consumes: Task 1's installed dependencies and the `exports` map.
- Produces: `src/server-guard.ts` — a side-effect-only module. Every guarded entry in Tasks 3 and 4
  opens with `import './server-guard.js';` as its **first import statement**.

- [ ] **Step 1: Write the vitest config**

Create `ts/packages/paigasus-sdk/vitest.config.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts'],
  },
  // `src/server-guard.ts` imports `server-only`, whose exports map is
  // { "react-server": "./empty.js", "default": "./index.js" } — and index.js is one unconditional
  // throw. Vitest does not set the `react-server` condition, so without this every test in the
  // package dies at import. The fix is the condition, NEVER deleting the import: that line is the
  // structural guard keeping this package out of client bundles.
  //
  // The other three entries are the additive module-resolution defaults. `react-server` ALONE
  // would drop `import`/`default` and break source-exports `.ts` resolution for @paigasus/proto —
  // the same trap ts/packages/paigasus-kernel/vitest.config.ts records for its browser project.
  resolve: {
    conditions: ['react-server', 'node', 'import', 'default'],
  },
  // Vitest 5 resolves a Node-environment test file's imports through `ssr.resolve.conditions`, NOT
  // the top-level `resolve.conditions` above — MEASURED on 5.0.0 in SMA-502. Setting only the
  // top-level key has no effect on an `environment: 'node'` project. Both blocks are kept: this one
  // governs resolution for this package's tests, and the top-level one is what a future
  // browser-mode project in this file would read.
  ssr: {
    resolve: {
      conditions: ['react-server', 'node', 'import', 'default'],
    },
  },
});
```

- [ ] **Step 2: Widen the tsconfig**

Replace `ts/packages/paigasus-sdk/tsconfig.json` entirely with:

```json
{
  "extends": "../../tsconfig.base.json",
  "_comment_types": "ts/tsconfig.base.json sets lib: [\"ES2022\"] and no types, under which Headers, fetch and ReadableStream do not exist. `types: [\"node\"]` supplies them WITHOUT pulling in the DOM lib — banning DOM types is spec § 6.2 layer 4, and node types are not DOM types. Matches ts/packages/paigasus-next-config/tsconfig.json.",
  "_comment_rootDir": "No rootDir/outDir: both are inert under noEmit, but rootDir enforces a containment check that would reject tests/ and vitest.config.ts, which `include` names deliberately so a type error in a test is a typecheck failure rather than a runtime surprise.",
  "compilerOptions": {
    "lib": ["ES2022"],
    "types": ["node"],
    "noEmit": true
  },
  "include": ["src/**/*", "tests/**/*", "vitest.config.ts"]
}
```

- [ ] **Step 3: Write the guard**

Create `ts/packages/paigasus-sdk/src/server-guard.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The single `import 'server-only'` site for @paigasus/sdk (spec § 6.2). One site, so an entry
// point that forgets the guard is a visible omission rather than a copied line that drifted.
//
// What this buys, and what it does not. `server-only`'s exports map is
// { "react-server": "./empty.js", "default": "./index.js" }, and index.js is one unconditional
// throw — so a CLIENT component taking a VALUE import of a guarded entry fails the build. It does
// NOT cover the middleware/edge layer: Next sets the `react-server` condition there too, so this
// resolves to server-only's own empty.js and is a no-op (MEASURED in SMA-502). This package is not
// middleware-reachable by design and reads no environment, so no NEXT_RUNTIME check is added.
//
// AC 1 is enforced STRUCTURALLY and is not proven by a build: no test in this package runs a
// client-side Next build, so no test observes the throw. tests/server-guard.test.ts proves every
// guarded entry imports this module first; a console-side failing-build fixture is SMA-510's.
import 'server-only';
```

- [ ] **Step 4: Write the failing AC 1 structural test**

Create `ts/packages/paigasus-sdk/tests/server-guard.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// AC 1, spec § 6.2 layer 3. Driven off package.json's `exports` map rather than a hand-written
// list, so an entry point added later is covered the day it is added — which is what makes it safe
// for PR B to ship two entries while the design spec's § 6.1 names five (see the plan's D1).
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const PKG_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');

// Entries that deliberately carry NO guard. `./errors/types` (PR C) holds only types: under
// `verbatimModuleSyntax` an `import type` emits nothing, so a client component can name those types
// with no runtime import and no server-only evaluation (spec § 6.3). Every OTHER entry is guarded.
const UNGUARDED_ENTRIES = new Set(['./errors/types']);

const GUARD_IMPORT = "import './server-guard.js';";

function readPackageExports(): Record<string, string> {
  const raw = readFileSync(resolve(PKG_ROOT, 'package.json'), 'utf8');
  const parsed = JSON.parse(raw) as { exports?: Record<string, string> };
  const exportsMap = parsed.exports;
  if (exportsMap === undefined) throw new Error('package.json declares no exports map');
  return exportsMap;
}

// The FIRST IMPORT STATEMENT, not the first line: every file opens with the SPDX header and most
// carry a comment block after it.
function firstImportStatement(source: string): string | undefined {
  return source.split('\n').find((line) => line.startsWith('import '));
}

describe('AC 1 — every guarded entry point imports the server guard first', () => {
  const entries = Object.entries(readPackageExports());

  it('finds at least one entry to check', () => {
    expect(entries.length).toBeGreaterThan(0);
  });

  it.each(entries.filter(([name]) => !UNGUARDED_ENTRIES.has(name)))(
    'entry %s imports the guard as its first import statement',
    (_name, target) => {
      const source = readFileSync(resolve(PKG_ROOT, target), 'utf8');
      expect(firstImportStatement(source)).toBe(GUARD_IMPORT);
    },
  );

  it.each(entries.filter(([name]) => UNGUARDED_ENTRIES.has(name)))(
    'entry %s deliberately carries no guard',
    (_name, target) => {
      const source = readFileSync(resolve(PKG_ROOT, target), 'utf8');
      expect(source).not.toContain(GUARD_IMPORT);
    },
  );
});

describe("AC 1 — 'server-only' is imported at exactly one site", () => {
  it('server-guard.ts imports it first', () => {
    const source = readFileSync(resolve(PKG_ROOT, 'src/server-guard.ts'), 'utf8');
    expect(firstImportStatement(source)).toBe("import 'server-only';");
  });

  it('no other file in src/ imports it', () => {
    const others = globSourceFiles().filter((f) => f !== resolve(PKG_ROOT, 'src/server-guard.ts'));
    const offenders = others.filter((f) => readFileSync(f, 'utf8').includes("'server-only'"));
    expect(offenders).toEqual([]);
  });
});

function globSourceFiles(): string[] {
  // readdirSync with recursive: true — no glob dependency, Node 24 supports it natively.
  return readdirSync(resolve(PKG_ROOT, 'src'), { recursive: true, encoding: 'utf8' })
    .filter((p) => p.endsWith('.ts'))
    .map((p) => resolve(PKG_ROOT, 'src', p));
}
```

- [ ] **Step 5: Run the tests and verify the state**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-sdk exec vitest run
```

Expected: `tests/deps.test.ts` **PASSES** (the resolution conditions are now in place — this is the
M2/M3 re-confirmation Task 1 could not complete). `tests/server-guard.test.ts` **FAILS** on the
entry `.`, because `src/index.ts` is still `export {};` and has no import statement at all.

- [ ] **Step 6: Make the `.` entry pass**

Replace `ts/packages/paigasus-sdk/src/index.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The root barrel. Task 4 turns this into a re-export of the ./iam surface, in the same commit that
// creates src/iam.ts and adds the "./iam" entry to package.json — the three belong together, so
// that no commit leaves the exports map pointing at a file that does not exist.
import './server-guard.js';

export {};
```

- [ ] **Step 7: Run tests and typecheck — both green**

```bash
pnpm -C ts/packages/paigasus-sdk exec vitest run
pnpm -C ts/packages/paigasus-sdk exec tsc -p tsconfig.json --noEmit
```

Expected: **PASS** for both. Every test in the package passes and the package typechecks. This task
leaves no known red behind; if something is red, it is a real defect, not staging.

- [ ] **Step 8: Commit**

```bash
git add ts/packages/paigasus-sdk/vitest.config.ts ts/packages/paigasus-sdk/tsconfig.json ts/packages/paigasus-sdk/src/server-guard.ts ts/packages/paigasus-sdk/src/index.ts ts/packages/paigasus-sdk/tests/server-guard.test.ts
git commit -m "feat(ts): add the @paigasus/sdk server-only guard and its structural test

One import 'server-only' site, and an AC 1 test driven off package.json's
exports map so a later entry point is covered the day it is added. The
vitest config sets BOTH resolve.conditions and ssr.resolve.conditions:
vitest 5 resolves a node-environment test through the latter, measured in
SMA-502.

Refs SMA-508"
```

---

## Task 3: The transport cache, the auth context key, and the interceptor

**Files:**
- Create: `ts/packages/paigasus-sdk/src/transport.ts`
- Test: `ts/packages/paigasus-sdk/tests/transport.test.ts`
- Test: `ts/packages/paigasus-sdk/tests/transport-wiring.test.ts`

**Interfaces:**
- Consumes: `src/server-guard.ts` from Task 2.
- Produces, all from `./transport.js`:
  - `type TransportOptions = { readonly baseUrl: string }`
  - `type Auth = { readonly bearer: string } | { readonly anonymous: true }`
  - `const DEFAULT_TIMEOUT_MS: 10_000`
  - `const authContextKey: ContextKey<Auth>`
  - `const authInterceptor: Interceptor`
  - `function stableTransportKey(options: TransportOptions): string`
  - `function getTransport(options: TransportOptions): Transport`
  - `function disposeTransports(): void`

- [ ] **Step 1: Write the failing transport tests**

Create `ts/packages/paigasus-sdk/tests/transport.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { afterAll, afterEach, describe, expect, it } from 'vitest';
import { createContextValues } from '@connectrpc/connect';
import type { UnaryRequest, UnaryResponse } from '@connectrpc/connect';
import {
  authContextKey,
  authInterceptor,
  disposeTransports,
  getTransport,
  stableTransportKey,
} from '../src/transport.js';

// An open HTTP/2 session keeps the Node process alive, so without this `vitest run` can hang after
// the assertions pass (spec § 7.3). Nothing here opens a socket — createGrpcTransport is lazy — but
// the hook is the contract this package exports disposeTransports() for.
afterAll(() => {
  disposeTransports();
});

afterEach(() => {
  disposeTransports();
});

describe('transport cache identity (spec § 7.1)', () => {
  it('returns the SAME object for equal options', () => {
    const a = getTransport({ baseUrl: 'https://iam.invalid' });
    const b = getTransport({ baseUrl: 'https://iam.invalid' });
    expect(a).toBe(b);
  });

  it('returns DIFFERENT objects for differing options', () => {
    const a = getTransport({ baseUrl: 'https://iam.invalid' });
    const b = getTransport({ baseUrl: 'https://gateway.invalid' });
    expect(a).not.toBe(b);
  });

  it('disposeTransports() empties the cache, so the next get rebuilds', () => {
    const before = getTransport({ baseUrl: 'https://iam.invalid' });
    disposeTransports();
    const after = getTransport({ baseUrl: 'https://iam.invalid' });
    expect(after).not.toBe(before);
  });
});

describe('stableTransportKey (spec § 7.1)', () => {
  it('is insensitive to property order', () => {
    // Cast: TransportOptions holds ONE field today. The key function is written for the whole
    // options object because spec § 7.1's rule is about the SECOND field — nodeOptions, the TLS
    // trust material — and this assertion is what keeps the rule honest before that field exists.
    const one = stableTransportKey({ baseUrl: 'https://a.invalid', z: 1, a: 2 } as never);
    const two = stableTransportKey({ a: 2, z: 1, baseUrl: 'https://a.invalid' } as never);
    expect(one).toBe(two);
  });

  it('separates two option sets that differ only in a nested field', () => {
    const one = stableTransportKey({ baseUrl: 'https://a.invalid', nodeOptions: { ca: 'X' } } as never);
    const two = stableTransportKey({ baseUrl: 'https://a.invalid', nodeOptions: { ca: 'Y' } } as never);
    expect(one).not.toBe(two);
  });

  it('does not collide two distinct base URLs', () => {
    expect(stableTransportKey({ baseUrl: 'https://a.invalid' })).not.toBe(
      stableTransportKey({ baseUrl: 'https://b.invalid' }),
    );
  });
});

function fakeUnaryRequest(): UnaryRequest {
  return {
    stream: false,
    header: new Headers(),
    contextValues: createContextValues(),
  } as unknown as UnaryRequest;
}

const noopNext = async (req: UnaryRequest): Promise<UnaryResponse> =>
  ({ stream: false, header: req.header } as unknown as UnaryResponse);

describe('authInterceptor (spec § 7.4)', () => {
  it('sets an Authorization header from a bearer Auth', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { bearer: 'token-a' });
    await authInterceptor(noopNext)(req);
    expect(req.header.get('authorization')).toBe('Bearer token-a');
  });

  it('sets NO Authorization header for an anonymous Auth', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    await authInterceptor(noopNext)(req);
    expect(req.header.get('authorization')).toBeNull();
  });

  it('defaults to anonymous when no Auth was bound', async () => {
    const req = fakeUnaryRequest();
    await authInterceptor(noopNext)(req);
    expect(req.header.get('authorization')).toBeNull();
  });
});
```

- [ ] **Step 2: Write the failing wiring test (AC 6)**

`vi.mock` is hoisted and applies to the whole module graph of its file, so this lives in its own
file rather than mixing a mocked `createGrpcTransport` into the cache-identity assertions above.

Create `ts/packages/paigasus-sdk/tests/transport-wiring.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// AC 6 and the other half of AC 5. The cache tests prove getTransport() memoizes; these prove WHAT
// it memoizes — that the real transport carries the auth interceptor and an explicit deadline, and
// that no token is among the arguments. Without this, `interceptors: [authInterceptor]` could be
// deleted and every other test in the package would still pass.
import { afterEach, describe, expect, it, vi } from 'vitest';

// `vi.hoisted`, NOT a plain `const`. vi.mock's factory is hoisted above every top-level statement
// in the file, so a factory closing over a plain `const` throws a TDZ ReferenceError at mock time.
// vi.hoisted runs its initializer in that same hoisted phase, which is what makes the spy reachable
// from the factory.
const { createGrpcTransport } = vi.hoisted(() => ({
  createGrpcTransport: vi.fn(() => ({ unary: vi.fn(), stream: vi.fn() })),
}));

vi.mock('@connectrpc/connect-node', async (importOriginal) => {
  // Spread the real module: transport.ts also imports Http2SessionManager from it, and a factory
  // that returned only the spy would break the constructor call rather than the assertion.
  const actual = await importOriginal<typeof import('@connectrpc/connect-node')>();
  return { ...actual, createGrpcTransport };
});

const { DEFAULT_TIMEOUT_MS, authInterceptor, disposeTransports, getTransport } = await import(
  '../src/transport.js'
);

afterEach(() => {
  disposeTransports();
  createGrpcTransport.mockClear();
});

describe('what getTransport builds (spec § 7.2, § 7.4)', () => {
  it('passes the auth interceptor', () => {
    getTransport({ baseUrl: 'https://iam.invalid' });
    const options = createGrpcTransport.mock.calls[0]?.[0] as { interceptors?: unknown[] };
    expect(options.interceptors).toContain(authInterceptor);
  });

  it('sets an explicit 10 s default deadline', () => {
    getTransport({ baseUrl: 'https://iam.invalid' });
    const options = createGrpcTransport.mock.calls[0]?.[0] as { defaultTimeoutMs?: number };
    expect(options.defaultTimeoutMs).toBe(DEFAULT_TIMEOUT_MS);
    expect(DEFAULT_TIMEOUT_MS).toBe(10_000);
  });

  it('passes NO token-shaped argument — the transport identity stays token-free', () => {
    getTransport({ baseUrl: 'https://iam.invalid' });
    const options = createGrpcTransport.mock.calls[0]?.[0] as Record<string, unknown>;
    expect(Object.keys(options).sort()).toEqual(
      ['baseUrl', 'defaultTimeoutMs', 'interceptors', 'sessionManager'].sort(),
    );
  });

  it('builds the transport once per distinct options object', () => {
    getTransport({ baseUrl: 'https://iam.invalid' });
    getTransport({ baseUrl: 'https://iam.invalid' });
    getTransport({ baseUrl: 'https://gateway.invalid' });
    expect(createGrpcTransport).toHaveBeenCalledTimes(2);
  });
});
```

- [ ] **Step 3: Run both and verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-sdk exec vitest run tests/transport.test.ts tests/transport-wiring.test.ts
```

Expected: FAIL — `Cannot find module '../src/transport.js'`.

- [ ] **Step 4: Implement the transport module**

Create `ts/packages/paigasus-sdk/src/transport.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import './server-guard.js';

import { createContextKey } from '@connectrpc/connect';
import type { Interceptor, Transport } from '@connectrpc/connect';
import { createGrpcTransport, Http2SessionManager } from '@connectrpc/connect-node';

/**
 * The identity of a transport.
 *
 * ONE field today, and the whole object is the cache key anyway. That is the point: spec § 7.1's
 * rule exists for the SECOND field. `createGrpcTransport` also accepts `nodeOptions` — the TLS
 * trust material — and this repo already carries four CA-bundle knobs with divergent semantics, so
 * a second option is a question of when, not whether. Keying on the base URL alone would let two
 * different trust configurations share one transport.
 *
 * When `nodeOptions` is added it must go to the `Http2SessionManager` CONSTRUCTOR (its third
 * argument), not to `createGrpcTransport`: connect-node documents that supplying `sessionManager`
 * makes a `nodeOptions` passed to the transport ineffective.
 */
export type TransportOptions = {
  readonly baseUrl: string;
};

/**
 * A union rather than an optional, so that an unauthenticated call — the health check is the real
 * case — is a written decision rather than an omission (spec § 7.4).
 */
export type Auth = { readonly bearer: string } | { readonly anonymous: true };

/**
 * An unset deadline lets a gRPC call in a Next server component hang the request forever. That is a
 * production hazard, not a default worth inheriting (spec § 7.2). Callers override per call via
 * `CallOptions.timeoutMs`.
 */
export const DEFAULT_TIMEOUT_MS = 10_000;

/**
 * The per-call authorization channel.
 *
 * The interceptor lives on the transport and the transport is cached, so the interceptor cannot
 * close over a token — it has to read one per call. `createContextKey` requires a default; the
 * default is `anonymous` so that a code path which somehow reaches the interceptor without a bound
 * Auth sends NO credential rather than a stale one. In practice the default is unreachable:
 * `createIamClient` takes `Auth` as a required parameter (spec § 7.4).
 */
export const authContextKey = createContextKey<Auth>(
  { anonymous: true },
  { description: '@paigasus/sdk per-call authorization' },
);

export const authInterceptor: Interceptor = (next) => async (req) => {
  const auth = req.contextValues.get(authContextKey);
  if ('bearer' in auth) {
    req.header.set('authorization', `Bearer ${auth.bearer}`);
  }
  return next(req);
};

/**
 * A stable serialization of the whole options object: keys sorted at every depth, `undefined`
 * dropped so an explicitly-absent option keys the same as an omitted one.
 *
 * `TransportOptions` is deliberately restricted to JSON-serializable data. A function-valued option
 * would serialize identically for two different functions and silently alias two transports, so a
 * future option that is not plain data needs its own identity contribution rather than this
 * function's default handling.
 */
export function stableTransportKey(options: TransportOptions): string {
  return serialize(options);
}

function serialize(value: unknown): string {
  if (value === undefined) return 'undefined';
  if (value === null || typeof value !== 'object') return JSON.stringify(value) ?? 'undefined';
  if (Array.isArray(value)) return `[${value.map(serialize).join(',')}]`;
  const entries = Object.entries(value as Record<string, unknown>)
    .filter(([, v]) => v !== undefined)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  return `{${entries.map(([k, v]) => `${JSON.stringify(k)}:${serialize(v)}`).join(',')}}`;
}

type CacheEntry = {
  readonly transport: Transport;
  readonly sessionManager: Http2SessionManager;
};

/**
 * Nothing evicts, and that is deliberate (spec § 7.3). Under a Next server a handful of long-lived
 * transports to a fixed set of in-cluster services is the intended shape.
 */
const cache = new Map<string, CacheEntry>();

export function getTransport(options: TransportOptions): Transport {
  const key = stableTransportKey(options);
  const hit = cache.get(key);
  if (hit !== undefined) return hit.transport;

  // The session manager is held so disposeTransports() can close the connection. Without a handle
  // there is no way to close an HTTP/2 session, and an open session keeps the Node process alive.
  const sessionManager = new Http2SessionManager(options.baseUrl);
  const transport = createGrpcTransport({
    baseUrl: options.baseUrl,
    sessionManager,
    defaultTimeoutMs: DEFAULT_TIMEOUT_MS,
    interceptors: [authInterceptor],
  });

  cache.set(key, { transport, sessionManager });
  return transport;
}

/**
 * Close every cached transport's HTTP/2 session and empty the cache.
 *
 * An open HTTP/2 session keeps a Node process alive, so `vitest run` can hang after the assertions
 * pass without this. Call it from `afterAll`, and from a server's shutdown path.
 */
export function disposeTransports(): void {
  for (const { sessionManager } of cache.values()) {
    sessionManager.abort();
  }
  cache.clear();
}
```

- [ ] **Step 5: Run the tests and verify they pass**

```bash
pnpm -C ts/packages/paigasus-sdk exec vitest run tests/transport.test.ts tests/transport-wiring.test.ts
```

Expected: PASS, both files.

If the `passes NO token-shaped argument` assertion fails listing a different key set, read what the
implementation actually passes, confirm it is correct, and re-pin the constant in the test — never
loosen the comparison to a subset check. The assertion exists to make a future added option a
reviewed decision.

- [ ] **Step 6: Typecheck**

```bash
pnpm -C ts/packages/paigasus-sdk exec tsc -p tsconfig.json --noEmit
```

Expected: exit 0, with no errors at all. This task leaves no known red behind.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-sdk/src/transport.ts ts/packages/paigasus-sdk/tests/transport.test.ts ts/packages/paigasus-sdk/tests/transport-wiring.test.ts
git commit -m "feat(ts): cache Connect-ES transports on full options identity

The cache key is a stable serialization of the whole options object, not
the base URL: createGrpcTransport also takes nodeOptions (TLS trust
material), and a base-URL key would let two trust configurations share
one transport. Authorization rides per call through ContextValues, so no
token ever enters the cached object. The transport carries an explicit
10 s deadline; an unset one lets a call in a server component hang the
request forever.

Refs SMA-508"
```

---

## Task 4: The IAM client factories and the token-isolation proof

**Files:**
- Create: `ts/packages/paigasus-sdk/src/iam.ts`
- Modify: `ts/packages/paigasus-sdk/package.json` (add the `./iam` entry — same commit as the file)
- Modify: `ts/packages/paigasus-sdk/src/index.ts` (re-export `./iam.js`)
- Test: `ts/packages/paigasus-sdk/tests/iam.test.ts`
- Delete: `ts/packages/paigasus-sdk/tests/deps.test.ts`

**Interfaces:**
- Consumes: `./transport.js`'s `Auth`, `TransportOptions`, `authContextKey`, `getTransport`.
- Produces, from `./iam.js` (and re-exported by `./index.js`):
  - `function createIamClient<S extends DescService>(service: S, options: TransportOptions, auth: Auth): Client<S>`
  - re-exports of the seven service descriptors: `TenancyService`, `AuthnService`,
    `AuthorizationService`, `ServiceAccountService`, `AuditService`, `UserService`, `OutboxService`
  - re-exports of `type Auth`, `type TransportOptions`, and `disposeTransports`

- [ ] **Step 1: Write the failing client tests**

Create `ts/packages/paigasus-sdk/tests/iam.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// AC 5. There is no live IAM service in CI (spec § 10 states this gap), so the proof runs against a
// recording Transport: it captures the ContextValues each call carries, and then drives the REAL
// authInterceptor over a synthetic request to turn that into the header a server would see.
import { afterEach, describe, expect, it } from 'vitest';
import { createClient, createContextValues } from '@connectrpc/connect';
import type { ContextValues, Transport, UnaryRequest, UnaryResponse } from '@connectrpc/connect';
import { TenancyService } from '@paigasus/proto/iam';
import { authContextKey, authInterceptor, disposeTransports } from '../src/transport.js';
import { bindAuth, createIamClient } from '../src/iam.js';

afterEach(() => {
  disposeTransports();
});

type Recorder = { readonly transport: Transport; readonly seen: (ContextValues | undefined)[] };

function recordingTransport(): Recorder {
  const seen: (ContextValues | undefined)[] = [];
  const transport: Transport = {
    async unary(_method, _signal, _timeoutMs, _header, _input, contextValues) {
      seen.push(contextValues);
      return { stream: false, message: {}, header: new Headers(), trailer: new Headers() } as unknown as UnaryResponse;
    },
    async stream(_method, _signal, _timeoutMs, _header, _input, contextValues) {
      seen.push(contextValues);
      throw new Error('not used');
    },
  };
  return { transport, seen };
}

// Turn a captured ContextValues into the header the real interceptor would produce.
async function headerFor(contextValues: ContextValues | undefined): Promise<Headers> {
  const header = new Headers();
  const req = { stream: false, header, contextValues: contextValues ?? createContextValues() } as unknown as UnaryRequest;
  const next = async (r: UnaryRequest): Promise<UnaryResponse> =>
    ({ stream: false, header: r.header } as unknown as UnaryResponse);
  await authInterceptor(next)(req);
  return header;
}

describe('AC 5 — two clients over ONE transport do not share a token (spec § 7.5)', () => {
  it('produces two different Authorization headers, neither carrying the other token', async () => {
    const { transport, seen } = recordingTransport();

    const alice = bindAuth(createClient(TenancyService, transport), { bearer: 'alice-token' });
    const bob = bindAuth(createClient(TenancyService, transport), { bearer: 'bob-token' });

    await alice.getOrganization({});
    await bob.getOrganization({});

    expect(seen).toHaveLength(2);
    const [first, second] = await Promise.all([headerFor(seen[0]), headerFor(seen[1])]);

    expect(first.get('authorization')).toBe('Bearer alice-token');
    expect(second.get('authorization')).toBe('Bearer bob-token');
    expect(first.get('authorization')).not.toBe(second.get('authorization'));
    expect(first.get('authorization')).not.toContain('bob-token');
    expect(second.get('authorization')).not.toContain('alice-token');
  });

  it('sends no Authorization header for an anonymous client', async () => {
    const { transport, seen } = recordingTransport();
    const anon = bindAuth(createClient(TenancyService, transport), { anonymous: true });

    await anon.getOrganization({});

    const header = await headerFor(seen[0]);
    expect(header.get('authorization')).toBeNull();
  });

  it('binds the Auth on EVERY call, not only the first', async () => {
    const { transport, seen } = recordingTransport();
    const alice = bindAuth(createClient(TenancyService, transport), { bearer: 'alice-token' });

    await alice.getOrganization({});
    await alice.getOrganization({});

    expect(seen).toHaveLength(2);
    for (const captured of seen) {
      expect(captured?.get(authContextKey)).toEqual({ bearer: 'alice-token' });
    }
  });
});

describe('D2 — a caller-supplied contextValues is refused, not silently dropped', () => {
  it('throws, naming the reason', async () => {
    const { transport } = recordingTransport();
    const client = bindAuth(createClient(TenancyService, transport), { bearer: 'alice-token' });

    await expect(client.getOrganization({}, { contextValues: createContextValues() })).rejects.toThrow(
      /contextValues/,
    );
  });
});

describe('createIamClient wires the cached transport', () => {
  it('returns a client whose methods are callable', () => {
    const client = createIamClient(TenancyService, { baseUrl: 'https://iam.invalid' }, { anonymous: true });
    expect(typeof client.getOrganization).toBe('function');
  });
});
```

**`getOrganization` is a VERIFIED method name, not a placeholder.** Read off
`ts/packages/paigasus-proto/src/generated/paigasus/iam/v1/iam_pb.ts:2728`, `TenancyService`'s unary
methods are `createOrganization`, `getOrganization`, `listOrganizations`, `renameOrganization`,
`archiveOrganization` and `restoreOrganization`. Use `getOrganization` exactly as written above.

There is no `createTenant` and no `getTenant` on this service — an earlier revision of this plan
guessed those names, and Task 1's review caught it. If a method name in this file does not resolve,
re-read the descriptor rather than inventing one.

- [ ] **Step 2: Run it and verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-sdk exec vitest run tests/iam.test.ts
```

Expected: FAIL — `Cannot find module '../src/iam.js'`.

- [ ] **Step 3: Implement the IAM surface**

Create `ts/packages/paigasus-sdk/src/iam.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import './server-guard.js';

import { createClient, createContextValues } from '@connectrpc/connect';
import type { CallOptions, Client } from '@connectrpc/connect';
import type { DescService } from '@bufbuild/protobuf';
import {
  AuditService,
  AuthnService,
  AuthorizationService,
  OutboxService,
  ServiceAccountService,
  TenancyService,
  UserService,
} from '@paigasus/proto/iam';

import { authContextKey, getTransport } from './transport.js';
import type { Auth, TransportOptions } from './transport.js';

export {
  AuditService,
  AuthnService,
  AuthorizationService,
  OutboxService,
  ServiceAccountService,
  TenancyService,
  UserService,
};
export { disposeTransports } from './transport.js';
export type { Auth, TransportOptions } from './transport.js';

/**
 * Bind an `Auth` to every call a client makes.
 *
 * The transport is cached and shared; the CLIENT is not. It holds a bearer, so its lifetime is one
 * request. A module-scope `const client = createIamClient(...)` is a cross-request token leak — the
 * same leak the transport cache exists to prevent, moved one level up. Nothing about `Client<S>`'s
 * shape marks it request-scoped, so this is a rule the type system cannot express (spec § 7.5).
 *
 * Exported for the test suite, which proves two clients over ONE transport keep their tokens apart.
 */
export function bindAuth<S extends DescService>(client: Client<S>, auth: Auth): Client<S> {
  const contextValues = createContextValues().set(authContextKey, auth);

  return new Proxy(client, {
    get(target, property, receiver) {
      const value: unknown = Reflect.get(target, property, receiver);
      if (typeof value !== 'function') return value;

      return (request: unknown, options?: CallOptions) => {
        if (options?.contextValues !== undefined) {
          // Refused rather than dropped or merged. ContextValues exposes get/set/delete and no
          // iteration, so this object cannot be copied: merging would mean writing the bearer into
          // an object the CALLER holds, which they could then reuse against another client — the
          // cross-request leak spec § 7.5 exists to prevent. Dropping it silently is worse still.
          throw new Error(
            '@paigasus/sdk: a caller-supplied `contextValues` is not supported, because the ' +
              'client binds its own to carry the bearer token. Remove it from the CallOptions.',
          );
        }
        return (value as (r: unknown, o: CallOptions) => unknown)(request, {
          ...options,
          contextValues,
        });
      };
    },
  });
}

/**
 * Build a typed, request-scoped client for one IAM service.
 *
 * `auth` is a REQUIRED parameter, so a forgotten token is a compile error rather than a runtime 401
 * — the most likely mistake in a package whose stated purpose is attaching bearer tokens. It is a
 * parameter of the CLIENT and never of `getTransport`, so it stays absent from the cached transport
 * and out of its cache key (spec § 7.4).
 */
export function createIamClient<S extends DescService>(
  service: S,
  options: TransportOptions,
  auth: Auth,
): Client<S> {
  return bindAuth(createClient(service, getTransport(options)), auth);
}
```

- [ ] **Step 3b: Publish the `./iam` entry point and re-export it from the root**

These two edits belong in the **same commit** as `src/iam.ts` above. No commit may leave the
`exports` map naming a file that does not exist — `tests/server-guard.test.ts` iterates that map and
`readFileSync`s every target, so a map entry without its file is a failing suite, and the AC 1 test
would be reporting a staging artefact instead of a guard violation.

In `ts/packages/paigasus-sdk/package.json`, extend the `exports` map to:

```json
  "exports": {
    ".": "./src/index.ts",
    "./iam": "./src/iam.ts"
  },
```

Replace `ts/packages/paigasus-sdk/src/index.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The root barrel. It re-exports the ./iam surface so `@paigasus/sdk` and `@paigasus/sdk/iam` are
// interchangeable for a consumer that wants everything; the subpath exists so a caller that only
// needs one surface does not pull the rest into its module graph (spec § 6.1).
import './server-guard.js';

export * from './iam.js';
```

- [ ] **Step 4: Run the whole suite**

```bash
pnpm -C ts/packages/paigasus-sdk exec vitest run
```

Expected: PASS, every file — `deps`, `server-guard` (now including the `.` and `./iam` entries),
`transport`, `transport-wiring`, `iam`.

- [ ] **Step 5: Delete the temporary dependency test**

`tests/deps.test.ts` has done its job: M2 and M3 are re-confirmed against the workspace resolution,
and the real suites now import and USE the same symbols, which proves the same thing by exercise.

```bash
rm ts/packages/paigasus-sdk/tests/deps.test.ts
pnpm -C ts/packages/paigasus-sdk exec vitest run
```

Expected: PASS, four files.

- [ ] **Step 6: Typecheck, lint and format**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-sdk exec tsc -p tsconfig.json --noEmit
pnpm -C ts exec prettier --write 'packages/paigasus-sdk/**/*.{ts,json}'
pnpm -C ts exec eslint packages/paigasus-sdk
```

Expected: all three exit 0.

The eslint boundary rule `paigasus/boundaries/sdk`
(`ts/packages/paigasus-next-config/src/eslint.mjs:91-104`) allows `@paigasus/proto` and its
subpaths and bans `react`/`react-dom`/`next`. `@paigasus/proto/iam` is allowed by the `!@paigasus/proto/**`
negation. If eslint reports a boundary violation, the import is wrong — do not widen the rule.

- [ ] **Step 7: Commit**

```bash
git add -A ts/packages/paigasus-sdk
git commit -m "feat(ts): add typed IAM client factories with per-call bearer binding

createIamClient takes Auth as a REQUIRED parameter, so a forgotten token
is a compile error rather than a runtime 401, and the token is a
parameter of the client and never of the transport factory — it stays
out of the cached object and its key. Two clients over one cached
transport keep their tokens apart, proven against a recording Transport
since CI has no live IAM service.

Refs SMA-508"
```

---

## Task 5: Moon inputs and the two affected-graph cases

This is the task that makes the `@paigasus/proto` edge real. Spec § 11.1 measured it (M11): editing
`error_pb.ts` selects **no** `paigasus-sdk-ts` task today, so without this task SMA-625's AC 2 would
be vacuous — its table test would run on a later, unrelated PR.

**Files:**
- Modify: `ts/packages/paigasus-sdk/moon.yml`
- Modify: `ci/affected-graph/run.sh` (the `contracts->proto` case, and a new `proto->sdk` case)

**Interfaces:**
- Consumes: the package from Tasks 1–4.
- Produces: `paigasus-sdk-ts` in the `contracts->proto` expected set, and a `proto->sdk` task case
  that is the only control on the input list.

- [ ] **Step 1: Declare the project edge and the inputs**

Replace `ts/packages/paigasus-sdk/moon.yml` entirely with:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-sdk-ts'
layer: 'library'
language: 'typescript'

# BOTH are required and neither implies the other. `dependsOn` is what `moon query projects
# --affected` follows and what orders the upstream's build; task `inputs` are the ONLY thing that
# confers affectedness on a DOWNSTREAM. MEASURED (spec § 11.1, M11): without the inputs below, an
# edit to paigasus-proto's generated error_pb.ts selects no paigasus-sdk-ts task at all.
dependsOn: ['paigasus-proto-ts']

tasks:
  # APPEND, never `options.merge: replace`. The inherited definitions in
  # .moon/tasks/typescript-project.yml supply @group(sources), tsconfig.json, package.json,
  # /ts/tsconfig.base.json and /ts/pnpm-lock.yaml; replacing would silently drop all of them —
  # the defect SMA-503 fixed on paigasus-console-ts:build.
  build:
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
  typecheck:
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
  test:
    # vitest.config.ts matches neither src/**/* nor tests/**/*, so without this line an edit to the
    # resolution conditions serves a cached PASS — and those conditions are the only thing keeping
    # `server-only`'s unconditional throw from failing the whole suite.
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
      - 'vitest.config.ts'
```

- [ ] **Step 2: Measure the new `contracts->proto` expected set**

Do **not** transcribe the set from the spec. SMA-624 merged since the spec's M4 was taken, so
re-measure it exactly as the harness does:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-508
sed -n '250,262p' ci/affected-graph/run.sh
```

Read how `run_case` derives its set, then run the same query against
`contracts/proto/paigasus/gateway/v1/health.proto` and record the ids. The expectation from the
spec is the seven existing ids plus `paigasus-sdk-ts`, and no app — but the measurement decides.

- [ ] **Step 3: Update the `contracts->proto` case**

In `ci/affected-graph/run.sh`, the `contracts->proto` case currently reads:

```bash
  run_case "contracts->proto" "contracts/proto/paigasus/gateway/v1/health.proto" \
    "contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs,paigasus-iam-rs,paigasus-service-info-rs"
```

Append `paigasus-sdk-ts` to the CSV, and extend the comment above it to say why the id is there:

```bash
  # contracts proto edit -> proto packages in all three languages + the gateway rebuild + the
  # IAM service crate that consumes paigasus-proto-rs for its gRPC surface (SMA-442) + the
  # shared descriptor crate that consumes the generated ServiceInfo/Capability types (SMA-505)
  # + @paigasus/sdk, whose build/typecheck/test key on paigasus-proto's sources (SMA-508). That
  # last edge is what makes a generated-code change re-run the SDK's suite; `dependsOn` alone
  # schedules the upstream and never selects the downstream.
  run_case "contracts->proto" "contracts/proto/paigasus/gateway/v1/health.proto" \
    "contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs,paigasus-iam-rs,paigasus-service-info-rs,paigasus-sdk-ts"
```

Keep the CSV on one line. If Step 2's measurement produced a different set, use the measured one and
say so in the commit message.

- [ ] **Step 4: Measure the `proto->sdk` task set**

The `ui->console` pair is the precedent (`ci/affected-graph/run.sh:354-377`). `run_task_case_ci`
filters `moon query tasks` to `build`/`test`/`lint` by name, so `typecheck` is structurally
invisible to it — the case cannot cover `typecheck` even though the input is declared there.

Derive the set with the same no-flag traversal `_assert_task_case_impl` uses, anchored on
`ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts`. Record what it returns.

**Cross-check, so a wrong measurement is recognisable as wrong.** The set should hold
`paigasus-sdk-ts:build` and `paigasus-sdk-ts:test` — spec § 11.1's M11b measured all three SDK tasks
selected, and `typecheck` is the one `run_task_case_ci` cannot see — plus `paigasus-proto-ts:build`
and `paigasus-proto-ts:test` (the edited package's own tasks) and `ts:lint`, which is what the
`ui->console` case shows for the same shape of edit. If the measured set holds **no**
`paigasus-sdk-ts` row, Step 1's `inputs` did not take effect — fix that before recording anything.
If it holds an app, read why: nothing in `ts/` depends on `@paigasus/sdk` today, so an app appearing
is a finding, not a baseline.

Use the measured set. The cross-check is there to catch a broken measurement, not to replace one.

- [ ] **Step 5: Add the `proto->sdk` case**

Insert directly after the `ui-components->console` case in `run_suite`, before
`assert_cargo_moon_parity`:

```bash
  # SMA-508 — a @paigasus/proto SOURCE edit must select the SDK's build and test.
  # This is the ONLY control on ts/packages/paigasus-sdk/moon.yml's `inputs` list. Remove that
  # list and the SDK's suite stops running on the PR that changes the generated code it consumes,
  # which is exactly the PR that can break it — and nothing else in the repo notices, because
  # `repo:input-liveness` scans `repo:*` tasks only and proves DECLARED inputs are live, never
  # that NEEDED ones are declared. MEASURED before the input existed (spec § 11.1, M11): the same
  # edit selected no paigasus-sdk-ts task at all.
  # Anchored on the generated error_pb.ts deliberately: SMA-625's error-mapping table test keys on
  # that file's descriptor, so this is the path whose selection that issue's AC depends on.
  # Strict equality: re-baseline deliberately when the set legitimately changes.
  run_task_case_ci "proto->sdk" "ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts" \
    "<MEASURED SET FROM STEP 4>"
```

Replace `<MEASURED SET FROM STEP 4>` with the measured CSV. Do not invent it.

- [ ] **Step 6: Run the gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke --force
```

Expected: PASS.

Two failure modes to read correctly before re-running anything:

- A **sub-3-second** failure is the known `affected-smoke` abort. Capture the full task output
  **before** re-running — a passing re-run overwrites `stdout.log`, truncates `stderr.log` and flips
  the `ciReport.json` row to `passed`. Grep the captured output for `proto-shim`: if that line is
  present the failure is infrastructure, not the affected graph, and a re-run passes.
- On this machine, `repo:affected-smoke` and `repo:actionlint` can **hang forever** on a
  `while read` fed by a here-string over about 512 bytes (bash 5.3.15). A hang is not a gate
  failure. CI is unaffected.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-sdk/moon.yml ci/affected-graph/run.sh
git commit -m "ci(affected-graph): make the proto -> sdk edge a proven selection

The SDK's build/typecheck/test now key on paigasus-proto's sources, and
a new proto->sdk case is the only control on that list. Measured before
the input existed: an error_pb.ts edit selected no paigasus-sdk-ts task
at all, which would have left SMA-625's table test running on a later,
unrelated PR. dependsOn schedules an upstream and never selects a
downstream, so both declarations are required.

Refs SMA-508"
```

---

## Task 6: Full-graph verification

Per-project Moon tasks do not run the repo-level gates. Before pushing, run the graph the way CI
does.

**Files:** none created or modified unless a gate reds.

- [ ] **Step 1: Run the full CI target graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-508
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free \
  --base origin/main \
  --include-relations
```

Expected: exit 0.

- [ ] **Step 2: Diagnose any failure with the recorded procedure**

Do not re-run first. A re-run overwrites every artifact holding the evidence, and a **passing**
re-run is just as destructive as another failing one.

```bash
cp .moon/cache/ciReport.json /tmp/sma508-ciReport.json
jq '.actions[] | select(.status=="failed")
    | {label, error,
       exec: (.operations[] | select(.meta.type=="task-execution") | {command, exitCode})}' \
   .moon/cache/ciReport.json
cat .moon/cache/states/<project>/<task>/stdout.log
cat .moon/cache/states/<project>/<task>/stderr.log
```

There is no action-level `exitCode` key — the real exit code and the full command live in
`operations[]`, on the entry whose `meta.type` is `task-execution`. Before pairing a log with a
command, compare the action's `finishedAt` against `lastRun.json`'s `lastRunTime`; if they disagree
the logs are from a different run.

Three gates are expected to be unaffected by this change and are worth naming so a red there is read
as a real finding rather than noise: `repo:release-parity*` abort **INCONCLUSIVE at rc=2** inside an
agent session because `proto` emits NDJSON on stdout — that is not a pass. `repo:input-liveness`
scans `repo:*` tasks only and cannot see the SDK's inputs. `repo:osv` may red on a fresh advisory
against a Connect-ES transitive dependency; the fix is a version bump or a justified
`osv-scanner.toml` waiver, never removing the dependency.

- [ ] **Step 3: Verify the diff matches the plan**

```bash
git diff origin/main --stat
```

Expected exactly these files:

```
ci/affected-graph/run.sh
ts/packages/paigasus-sdk/moon.yml
ts/packages/paigasus-sdk/package.json
ts/packages/paigasus-sdk/src/iam.ts
ts/packages/paigasus-sdk/src/index.ts
ts/packages/paigasus-sdk/src/server-guard.ts
ts/packages/paigasus-sdk/src/transport.ts
ts/packages/paigasus-sdk/tests/iam.test.ts
ts/packages/paigasus-sdk/tests/server-guard.test.ts
ts/packages/paigasus-sdk/tests/transport-wiring.test.ts
ts/packages/paigasus-sdk/tests/transport.test.ts
ts/packages/paigasus-sdk/tsconfig.json
ts/packages/paigasus-sdk/vitest.config.ts
ts/pnpm-lock.yaml
ts/pnpm-workspace.yaml
docs/superpowers/plans/2026-09-09-sma-508-sdk-package-transport-iam.md
```

Anything else is either a stray edit or a generated file that should not be committed. `.releaserc.json`
is deliberately unchanged.

- [ ] **Step 4: Confirm no debug residue**

```bash
grep -rnE 'console\.(log|debug)|\.only\(|\.skip\(|TODO|FIXME|XXX' ts/packages/paigasus-sdk/src ts/packages/paigasus-sdk/tests
```

Expected: no output.

- [ ] **Step 5: Commit the plan document**

```bash
git add docs/superpowers/plans/2026-09-09-sma-508-sdk-package-transport-iam.md
git commit -m "docs(superpowers): add the SMA-508 implementation plan

Refs SMA-508"
```

---

## Acceptance criteria mapping

| AC (Linear SMA-508) | Where it lands | How it is proven |
|---|---|---|
| 1 — `server-only` at the entry point | Task 2 | `tests/server-guard.test.ts`, driven off `package.json`. **Structural, not proven by a build** — no test here runs a client-side Next build; the fixture is SMA-510's. |
| 2 — `contracts->proto` updated in this change | Task 5 Step 3 | The case itself, strict equality, re-measured not transcribed |
| 3 — SDK inputs name `/ts/packages/paigasus-proto/src/**/*` | Task 5 Step 1 | `moon.yml`; the control is AC 4 |
| 4 — a new `run_task_case_ci "proto->sdk"` | Task 5 Step 5 | The case, anchored on the generated `error_pb.ts` |
| 5 — a forgotten token is a compile error | Task 4 | `auth` is a required parameter of `createIamClient`; `tests/iam.test.ts` proves two clients over one transport keep their tokens apart, and `tests/transport-wiring.test.ts` proves no token-shaped key reaches `createGrpcTransport` |
| 6 — explicit 10 s default deadline | Task 3 | `tests/transport-wiring.test.ts` asserts `defaultTimeoutMs === 10_000` |

## What this plan deliberately does not do

- **No `./chat`, `./errors` or `./errors/types` entry points.** PR C (SMA-625). See D1.
- **No live-service tier.** The SDK has no server to talk to in CI. This suite proves the SDK forms
  correct requests and binds credentials correctly; it does not prove IAM accepts them. Standing a
  service up is SMA-509/SMA-510's integration surface (spec § 10).
- **No retry or backoff policy.** Spec § 14.
- **No `IntrospectPrincipalResolver` adapter.** The SDK's boundary rule bans importing
  `@paigasus/auth`; the app wires the two. Spec § 14.
- **No eviction from the transport cache.** Stated and deliberate (spec § 7.3).
