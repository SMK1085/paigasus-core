<!-- SPDX-License-Identifier: Apache-2.0 -->

# `@paigasus/auth` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `@paigasus/auth`, a backend-for-frontend OIDC relying party with a Redis-backed session, so a Paigasus console zone can log a user in and hold their tokens server-side.

**Architecture:** Ports-and-adapters, with ports only where two implementations ship in production. `SessionStore` has Redis and memory adapters; `PrincipalResolver` has a claims-based adapter now and an Introspect-backed one in SMA-508; `AuthLogger` has a no-op default. Everything else is concrete and wired by one composition root, `createAuthRuntime`. The package exposes three subpath entries — `./server`, `./client`, `./middleware` — and no root export, which is what structurally prevents a token from reaching a client bundle.

**Tech Stack:** TypeScript 6, zod 4, `openid-client` 6, node-redis 6, `jose` 6, vitest 5, Playwright 1.63, testcontainers 12, Moon, pnpm 11.

**Spec:** `docs/superpowers/specs/2026-09-09-sma-506-auth-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Branch is `feature/sma-506-ts-paigasus-auth`. Conventional commits with a workspace scope: `feat(ts): …`.
- **Commit message body: no line over 100 characters, and no line may begin with `word:`** — a wrapped line starting `BFF:` parses as a git footer token and reds `footer-leading-blank`. This already cost one failed commit on this branch.
- Node `>=24`. Every dependency is referenced as `"catalog:"`; the version lives only in `ts/pnpm-workspace.yaml`.
- Package is `private: true`, source-only, `"type": "module"`.
- `tsconfig.json` extends `../../tsconfig.base.json`, has **no `rootDir`/`outDir`**, and includes `src/**/*`, `tests/**/*`, `vitest.config.ts`, `playwright.config.ts`. A file in no program is a fatal error under `ts:lint`.
- No file under `ts/` may contain the literal `NEXT_PUBLIC_` outside Markdown or backtick-quoted comment prose — `repo:next-public-free` scans every tracked file under `ts/`.
- Shell PATH: prefix commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- **Never `<<<` in a shell script or command on this machine** — bash 5.3.15 here deadlocks on a here-string over ~512 bytes. Use a file or process substitution.
- Pinned versions for this plan: `openid-client@^6.8.8`, `jose@^6.2.12`, `redis@^6.2.1`, `@playwright/test@^1.63.0`, `testcontainers@^12.1.0`. All cleared pnpm's 24-hour `minimumReleaseAge` on 2026-09-09.
- **node-redis is v6, not v5.** Its client API differs from v5; verify every call against the installed types rather than from memory (M2).

---

## File Structure

**New package `ts/packages/paigasus-auth/`:**

| File | Responsibility |
|---|---|
| `package.json` | three subpath exports, no `"."` |
| `moon.yml` | `paigasus-auth-ts`, `layer: library`, `test` + `test-e2e` inputs |
| `tsconfig.json` | ui/next-config shape |
| `vitest.config.ts` | node env, `ssr.resolve.conditions` |
| `playwright.config.ts` | E2E project, `globalSetup`, `webServer` |
| `src/server.ts` | `/server` entry; `import 'server-only'` first line |
| `src/client.ts` | `/client` entry; React only |
| `src/middleware.ts` | `/middleware` entry; Web APIs only |
| `src/config.ts` | the zod raw shape this package owns |
| `src/runtime.ts` | `createAuthRuntime` — composition root, cross-field validation, derivation |
| `src/core/refresh-policy.ts` | `shouldRefresh` — pure |
| `src/core/return-to.ts` | `validateReturnTo` — pure |
| `src/core/session.ts` | `SessionRecord`, `SESSION_VIEW_KEYS`, `toSessionView` |
| `src/core/ids.ts` | session id, lock token, txn id, secret + hash |
| `src/core/single-flight.ts` | the refresh algorithm |
| `src/core/errors.ts` | error taxonomy |
| `src/ports/session-store.ts` | `SessionStore` |
| `src/ports/principal-resolver.ts` | `PrincipalResolver`, `ResolvedPrincipal`, `RoleGrantRef` |
| `src/ports/logger.ts` | `AuthLogger`, `AuthEventName` |
| `src/adapters/memory-store.ts` | in-process store |
| `src/adapters/redis-store.ts` | node-redis store, Lua CAS |
| `src/adapters/claims-resolver.ts` | identity from ID-token claims |
| `src/adapters/oidc.ts` | `openid-client` wiring |
| `src/adapters/noop-logger.ts` | default logger |
| `src/http/cookies.ts` | `__Host-` cookie serialisation |
| `src/http/routes.ts` | `createAuthRoutes` |
| `src/next/get-session.ts` | `getSession`, `requireSession` |

**Modified:**

| File | Change |
|---|---|
| `ts/pnpm-workspace.yaml` | five catalog entries |
| `ts/packages/paigasus-next-config/src/eslint.mjs` | three boundary blocks, scope keys, three stale issue refs |
| `ts/packages/paigasus-next-config/tests/boundaries.test.ts` | new DENIED/ALLOWED rows |
| `.github/workflows/ci.yml` | `T=(…)` gains `:test-e2e`; Playwright browser install step |
| `CLAUDE.md` | marker-delimited command gains `:test-e2e` |
| `ci/affected-graph/run.sh` | a `run_task_case_ci` row for the new package |
| `CONTRIBUTING.md` | container + browser provisioning for the local full-graph run |

---

## Task 1: Package scaffold, config shape, and the logger port

**Files:**
- Create: `ts/packages/paigasus-auth/package.json`, `tsconfig.json`, `moon.yml`, `vitest.config.ts`
- Create: `ts/packages/paigasus-auth/src/config.ts`, `src/ports/logger.ts`, `src/adapters/noop-logger.ts`
- Modify: `ts/pnpm-workspace.yaml`
- Test: `ts/packages/paigasus-auth/tests/config.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `authEnvShape` (a `ZodRawShape`), `AuthEnv` (`z.infer` of `z.object(authEnvShape)`), `AuthLogger`, `AuthEventName`, `noopLogger`.

The logger port lands **now**, not later: adding it after the adapters exist changes every constructor in `adapters/` and `http/` (spec § 12).

- [ ] **Step 1: Add the catalog entries**

In `ts/pnpm-workspace.yaml`, inside the `catalog:` block, after the `vite-plugin-wasm` entry:

```yaml
  # OIDC relying party for @paigasus/auth (SMA-506). v6 is the line ADR-0017 decision 4 names.
  # Protocol handling — PKCE, state, nonce, discovery, JWKS, token exchange — stays in the library.
  openid-client: ^6.8.8
  # JOSE primitives. Used ONLY by tests, to mint a local JWKS and sign fixture tokens (AC 6).
  # Production token verification is openid-client's job; this is never imported from src/.
  jose: ^6.2.12
  # node-redis, the official client. v6 — its client API differs from v5, so check the installed
  # types rather than older examples. Chosen over ioredis because its redis:// DSN parsing matches
  # rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs verbatim.
  redis: ^6.2.1
  # E2E tier for @paigasus/auth. A real browser is the only thing that can prove SameSite behaviour
  # on the cross-site redirect back from the IdP, and __Host- cookie-prefix enforcement.
  '@playwright/test': ^1.63.0
  # Starts Keycloak and Redis for the E2E tier. The Rust side uses testcontainers-modules; this is
  # the TS equivalent. GitHub Actions `services:` cannot do the cert copy and realm import.
  testcontainers: ^12.1.0
```

- [ ] **Step 2: Create `package.json`**

```json
{
  "name": "@paigasus/auth",
  "_comment_exports": "THREE subpath exports and deliberately NO \".\". The app-shell boundary rule denies @paigasus/auth/server; a root export re-exporting the server surface would let `import { getSession } from '@paigasus/auth'` walk around it. tests/exports.test.ts pins this key set.",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "engines": { "node": ">=24" },
  "license": "Apache-2.0",
  "description": "OIDC relying party, Redis-backed session store, and auth route factory for Paigasus console zones.",
  "exports": {
    "./server": "./src/server.ts",
    "./client": "./src/client.ts",
    "./middleware": "./src/middleware.ts"
  },
  "scripts": { "typecheck": "tsc -p tsconfig.json --noEmit" },
  "dependencies": {
    "openid-client": "catalog:",
    "redis": "catalog:",
    "server-only": "catalog:",
    "zod": "catalog:"
  },
  "peerDependencies": { "next": "catalog:", "react": "catalog:" },
  "devDependencies": {
    "@playwright/test": "catalog:",
    "@types/node": "catalog:",
    "@types/react": "catalog:",
    "jose": "catalog:",
    "next": "catalog:",
    "react": "catalog:",
    "testcontainers": "catalog:",
    "typescript": "catalog:",
    "vitest": "catalog:"
  }
}
```

- [ ] **Step 3: Create `tsconfig.json`**

```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "lib": ["DOM", "DOM.Iterable", "ES2022"],
    "jsx": "react-jsx",
    "types": ["node"],
    "noEmit": true
  },
  "_comment_include": "tests/, vitest.config.ts and playwright.config.ts must be in the program: ts/eslint.config.js applies type-checked rules with projectService:true to every **/*.{ts,tsx}, and `ts:lint` runs `eslint .` over the whole tree — a file in no program errors there. No rootDir: it rejects files outside src/ and is inert under noEmit. Same shape and same reason as ts/packages/paigasus-ui/tsconfig.json.",
  "include": ["src/**/*", "tests/**/*", "vitest.config.ts", "playwright.config.ts"]
}
```

`lib` needs DOM because `src/client.ts` is React and `tests/` asserts on cookie strings.

- [ ] **Step 4: Create `vitest.config.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from 'vitest/config';

// BOTH condition lists are required, and the ssr one is what actually governs a Node-environment
// test. vitest 5 resolves a Node project's imports through ssr.resolve.conditions, NOT the
// top-level resolve.conditions (MEASURED on 5.0.0, SMA-502). src/server.ts opens with
// `import 'server-only'`, whose exports map is { "react-server": "./empty.js", "default":
// "./index.js" } — and index.js is an unconditional throw. Without the react-server condition
// every test that touches the server entry dies at import.
//
// The list is ADDITIVE and must keep node/import/default: a bare ['react-server'] drops them and
// breaks source-exports .ts resolution for every @paigasus/* package.
const conditions = ['react-server', 'node', 'import', 'default'];

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
    // The container-backed suites live under tests/containers/ and run only in the test-e2e task.
    exclude: ['tests/containers/**', 'tests/e2e/**', '**/node_modules/**'],
  },
  resolve: { conditions },
  ssr: { resolve: { conditions } },
});
```

- [ ] **Step 5: Create `moon.yml`**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-auth-ts'
layer: 'library'
language: 'typescript'

tasks:
  # `/ts/eslint.config.js` is added to the INHERITED test inputs, not merged away: tests/boundaries
  # asserts the workspace eslint config actually applies this package's blocks, and the inherited
  # list (.moon/tasks/typescript-project.yml) does not name that file. Without it Moon replays a
  # cached PASS on exactly the PR that deletes a block.
  test:
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - 'vitest.config.ts'
      - 'package.json'
      - '/ts/eslint.config.js'
      - '/ts/pnpm-lock.yaml'
```

The `test-e2e` task is added in Task 13, when there is something for it to run. Adding it now would put a task in `T` that resolves to a suite of zero tests.

- [ ] **Step 6: Write the failing test**

`ts/packages/paigasus-auth/tests/config.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { z } from 'zod';
import { authEnvShape } from '../src/config.js';

const schema = z.object(authEnvShape);

const VALID = {
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.com/realms/paigasus',
  PAIGASUS_OIDC_CLIENT_ID: 'paigasus-console',
  PAIGASUS_OIDC_CLIENT_SECRET: 's3cret',
  PAIGASUS_PUBLIC_ORIGIN: 'https://app.example.com',
  PAIGASUS_SESSION_STORE: 'redis',
  PAIGASUS_SESSION_REDIS_URL: 'redis://localhost:6379/0',
};

describe('authEnvShape', () => {
  it('accepts a complete valid environment', () => {
    expect(schema.parse(VALID).PAIGASUS_OIDC_CLIENT_ID).toBe('paigasus-console');
  });

  it('applies every documented default', () => {
    const parsed = schema.parse(VALID);
    expect(parsed.PAIGASUS_OIDC_SCOPES).toBe('openid profile email offline_access');
    expect(parsed.PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS).toBe(30);
    expect(parsed.PAIGASUS_OIDC_HTTP_TIMEOUT_MS).toBe(3500);
    expect(parsed.PAIGASUS_SESSION_REDIS_TIMEOUT_MS).toBe(1000);
    expect(parsed.PAIGASUS_SESSION_TTL_SECONDS).toBe(28800);
    expect(parsed.PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS).toBe(86400);
    expect(parsed.PAIGASUS_SESSION_REFRESH_SKEW_SECONDS).toBe(30);
    expect(parsed.PAIGASUS_SESSION_LOCK_TTL_MS).toBe(5000);
    expect(parsed.PAIGASUS_SESSION_LOCK_WAIT_MS).toBe(3000);
  });

  it('rejects a non-https issuer', () => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_OIDC_ISSUER: 'http://idp.example.com' })).toThrow();
  });

  it('rejects an unknown store backend', () => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_SESSION_STORE: 'postgres' })).toThrow();
  });

  it('rejects an origin with a trailing slash', () => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_PUBLIC_ORIGIN: 'https://app.example.com/' })).toThrow();
  });

  it('does NOT reject a missing redis url — that is a cross-field rule owned by createAuthRuntime', () => {
    const { PAIGASUS_SESSION_REDIS_URL: _omitted, ...withoutUrl } = VALID;
    expect(() => schema.parse(withoutUrl)).not.toThrow();
  });

  it('declares neither zone key — defineRuntimeConfig throws if an extra shape does', () => {
    expect(Object.keys(authEnvShape)).not.toContain('PAIGASUS_ZONE');
    expect(Object.keys(authEnvShape)).not.toContain('PAIGASUS_ZONES');
  });
});
```

The last two tests are the ones that matter. They pin the § 6.1 resolution: a `ZodRawShape` is flat and cannot express a cross-field rule, and `defineRuntimeConfig` throws on the zone keys (`runtime.ts:195-199`).

- [ ] **Step 7: Run it and watch it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts install
pnpm -C ts/packages/paigasus-auth exec vitest run tests/config.test.ts
```

Expected: FAIL — cannot resolve `../src/config.js`.

- [ ] **Step 8: Write `src/config.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The environment variables @paigasus/auth owns, as a zod raw shape an app composes into
// @paigasus/next-config's defineRuntimeConfig(extraShape).
//
// THIS SHAPE IS FLAT AND PER-KEY, DELIBERATELY. defineRuntimeConfig builds z.object({...core,
// ...extra}) with no refinement hook (next-config/src/runtime.ts:201), and throws if an extra
// shape declares PAIGASUS_ZONE or PAIGASUS_ZONES (:195-199). So two things CANNOT live here and
// live in createAuthRuntime instead (src/runtime.ts):
//   - "REDIS_URL is required when STORE=redis" — a cross-field rule.
//   - the derived redirect URIs — they need the zone keys this shape may not declare.
//
// Note also that next-config's describeIssues renders a message only for `custom` issues on ITS
// OWN keys; every issue raised here surfaces as `<key>: <code>`. That filter is what stops a
// package-authored message leaking a secret, so it is accepted rather than widened. The names
// below are therefore self-describing, and README.md carries the expected form of each.
import { z } from 'zod';

const seconds = (fallback: number) => z.coerce.number().int().positive().default(fallback);
const millis = (fallback: number) => z.coerce.number().int().positive().default(fallback);

/** An absolute https URL with no trailing slash and no surrounding whitespace. */
const httpsUrl = z
  .string()
  .refine((v) => v === v.trim() && v.length > 0, { error: 'must not have leading or trailing whitespace' })
  .refine((v) => URL.canParse(v) && new URL(v).protocol === 'https:', { error: 'must be an absolute https URL' })
  .refine((v) => !v.endsWith('/'), { error: 'must not end with a slash' });

export const authEnvShape = {
  PAIGASUS_OIDC_ISSUER: httpsUrl,
  PAIGASUS_OIDC_CLIENT_ID: z.string().min(1),
  PAIGASUS_OIDC_CLIENT_SECRET: z.string().min(1),
  PAIGASUS_PUBLIC_ORIGIN: httpsUrl,
  PAIGASUS_OIDC_REDIRECT_URI: z.string().optional(),
  PAIGASUS_OIDC_POST_LOGOUT_REDIRECT_URI: z.string().optional(),
  PAIGASUS_OIDC_SCOPES: z.string().min(1).default('openid profile email offline_access'),
  PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS: seconds(30),
  PAIGASUS_OIDC_HTTP_TIMEOUT_MS: millis(3500),
  PAIGASUS_SESSION_STORE: z.enum(['redis', 'memory']),
  PAIGASUS_SESSION_REDIS_URL: z.string().optional(),
  PAIGASUS_SESSION_REDIS_TIMEOUT_MS: millis(1000),
  PAIGASUS_SESSION_TTL_SECONDS: seconds(28800),
  PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS: seconds(86400),
  PAIGASUS_SESSION_REFRESH_SKEW_SECONDS: seconds(30),
  PAIGASUS_SESSION_LOCK_TTL_MS: millis(5000),
  PAIGASUS_SESSION_LOCK_WAIT_MS: millis(3000),
} as const;

export type AuthEnv = z.infer<z.ZodObject<typeof authEnvShape>>;
```

`https` is enforced on the issuer because IAM's own `IssuerConfig` validator does the same (`rs/.../config.rs:1032-1053`); a plain-http issuer is a downgrade the operator should be told about.

- [ ] **Step 9: Run the test and watch it pass**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/config.test.ts
```

Expected: PASS, 7 tests.

- [ ] **Step 10: Write the logger port and the no-op adapter**

`src/ports/logger.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Observability is a PORT, not a detail: adding a logger after the adapters exist changes every
// constructor in adapters/ and http/. It lands with the scaffold for that reason.
//
// REDACTION IS THE CALLER'S CONTRACT, and it is absolute. No event may carry an access token, a
// refresh token, an authorization code, an id_token, the client secret, the Redis DSN, or a
// transaction secret. `sid` is logged TRUNCATED TO 8 CHARACTERS — enough to correlate two lines,
// not enough to replay a session.
//
// Never pass a caught library error object into `fields`: node-redis embeds the DSN in its own
// connection errors and openid-client may include a URL. Extract `name` and a fixed message.

export type AuthEventName =
  | 'login.started'
  | 'login.callback_rejected'
  | 'session.created'
  | 'session.refreshed'
  | 'session.refresh_failed'
  | 'session.refresh_timeout'
  | 'session.refresh.persist_failed'
  | 'session.deleted'
  | 'logout.completed'
  | 'store.unavailable';

export type AuthEventFields = Readonly<Record<string, string | number | boolean>>;

export interface AuthLogger {
  event(name: AuthEventName, fields: AuthEventFields): void;
}

/** Truncate a session id for logging. See the redaction contract above. */
export function sidTag(sid: string): string {
  return sid.slice(0, 8);
}
```

`src/adapters/noop-logger.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import type { AuthLogger } from '../ports/logger.js';

/** The default. The package emits nothing unless an app opts in. */
export const noopLogger: AuthLogger = { event: () => undefined };
```

- [ ] **Step 11: Write the logger test**

`ts/packages/paigasus-auth/tests/logger.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { noopLogger, sidTag } from '../src/adapters/noop-logger.js';
import type { AuthLogger } from '../src/ports/logger.js';

describe('AuthLogger', () => {
  it('the no-op default swallows events', () => {
    expect(() => noopLogger.event('login.started', { zone: 'iam' })).not.toThrow();
  });

  it('sidTag truncates to 8 characters', () => {
    expect(sidTag('abcdefghijklmnopqrstuvwxyz')).toBe('abcdefgh');
  });

  it('a recording logger captures name and fields', () => {
    const seen: Array<[string, Record<string, unknown>]> = [];
    const rec: AuthLogger = { event: (n, f) => void seen.push([n, { ...f }]) };
    rec.event('session.created', { sid: sidTag('abcdefghijkl'), zone: 'iam' });
    expect(seen).toEqual([['session.created', { sid: 'abcdefgh', zone: 'iam' }]]);
  });
});
```

Note `sidTag` is re-exported from `noop-logger.ts` for this import to work — add
`export { sidTag } from '../ports/logger.js';` to that file, or import it from the port directly. Import it from the port directly:
change the test's import to `import { sidTag } from '../src/ports/logger.js';` and keep `noop-logger.ts` exporting only `noopLogger`.

- [ ] **Step 12: Run the full package test suite**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run
```

Expected: PASS, 10 tests.

- [ ] **Step 13: Verify typecheck and lint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-auth-ts:typecheck
moon run ts:lint
```

Expected: both PASS. If `ts:lint` reports a parse error about a file not in a program, `tsconfig.json`'s `include` is wrong — re-read Step 3.

- [ ] **Step 14: Commit**

```bash
git add ts/pnpm-workspace.yaml ts/pnpm-lock.yaml ts/packages/paigasus-auth
git commit -m "feat(ts): scaffold @paigasus/auth with its config shape and logger port" \
  -m "Adds the package with three subpath exports and no root export, the zod raw
shape for the seventeen variables it owns, and the AuthLogger port with a
no-op default.

The logger lands with the scaffold rather than later because adding it once
adapters exist would change every constructor in adapters/ and http/."
```

---

## Task 2: Core pure functions

**Files:**
- Create: `src/core/refresh-policy.ts`, `src/core/return-to.ts`, `src/core/session.ts`, `src/core/ids.ts`, `src/core/errors.ts`, `src/ports/principal-resolver.ts`
- Test: `tests/core/refresh-policy.test.ts`, `tests/core/return-to.test.ts`, `tests/core/session.test.ts`, `tests/core/ids.test.ts`

**Interfaces:**
- Consumes: `AuthLogger` (Task 1).
- Produces: `shouldRefresh(now, expiresAt, skewMs): boolean`; `validateReturnTo(raw, fallback): string`; `SessionRecord`; `SESSION_VIEW_KEYS`; `SessionView`; `toSessionView(rec): SessionView`; `newSessionId()`, `newLockToken()`, `newTransactionId()`, `newTransactionSecret()`, `hashSecret(s): string`, `secretMatchesHash(s, h): boolean`; `RoleGrantRef`, `ResolvedPrincipal`, `PrincipalResolver`; `AuthError` subclasses.

- [ ] **Step 1: Write the failing tests for `shouldRefresh`**

`tests/core/refresh-policy.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { shouldRefresh } from '../../src/core/refresh-policy.js';

// Every case varies the STORED timestamp, never the clock. That is the whole reason this package
// has no Clock port: moving expiresAt is equivalent to moving now, and it needs no machinery.
describe('shouldRefresh', () => {
  const skew = 30_000;
  const now = 1_000_000;

  it.each([
    ['far in the future', now + 600_000, false],
    ['just outside the skew window', now + skew + 1, false],
    ['exactly at the skew boundary', now + skew, true],
    ['just inside the skew window', now + skew - 1, true],
    ['exactly at expiry', now, true],
    ['already expired', now - 1, true],
    ['long expired', now - 600_000, true],
  ])('%s', (_label, expiresAt, expected) => {
    expect(shouldRefresh(now, expiresAt, skew)).toBe(expected);
  });

  it('a zero skew refreshes only at or past expiry', () => {
    expect(shouldRefresh(now, now + 1, 0)).toBe(false);
    expect(shouldRefresh(now, now, 0)).toBe(true);
  });
});
```

- [ ] **Step 2: Write the failing tests for `validateReturnTo`**

`tests/core/return-to.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { validateReturnTo } from '../../src/core/return-to.js';

const FALLBACK = '/iam';

describe('validateReturnTo', () => {
  it.each([
    ['a plain path', '/iam/orgs', '/iam/orgs'],
    ['a path with a query', '/iam/orgs?page=2', '/iam/orgs?page=2'],
    ['a path with a fragment', '/iam/orgs#top', '/iam/orgs#top'],
    ['the root', '/', '/'],
  ])('accepts %s', (_l, input, expected) => {
    expect(validateReturnTo(input, FALLBACK)).toBe(expected);
  });

  // Each of these is an open redirect if it gets through.
  it.each([
    ['a protocol-relative URL', '//evil.com'],
    ['a protocol-relative URL with a path', '//evil.com/x'],
    ['an absolute http URL', 'http://evil.com'],
    ['an absolute https URL', 'https://evil.com'],
    ['a backslash-relative URL', '/\\evil.com'],
    ['a double backslash', '\\\\evil.com'],
    ['a scheme-relative with backslash', '/\\/evil.com'],
    ['a javascript scheme', 'javascript:alert(1)'],
    ['a data scheme', 'data:text/html,x'],
    ['a percent-encoded scheme separator', '/%2f%2fevil.com'],
    ['an encoded backslash', '/%5Cevil.com'],
    ['a relative path with no leading slash', 'iam/orgs'],
    ['an empty string', ''],
    ['whitespace only', '   '],
    ['a leading-whitespace protocol-relative URL', '  //evil.com'],
    ['a tab-prefixed absolute URL', '\thttps://evil.com'],
  ])('rejects %s and falls back', (_l, input) => {
    expect(validateReturnTo(input, FALLBACK)).toBe(FALLBACK);
  });

  it('rejects undefined and falls back', () => {
    expect(validateReturnTo(undefined, FALLBACK)).toBe(FALLBACK);
  });
});
```

- [ ] **Step 3: Run both and watch them fail**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/
```

Expected: FAIL — modules not found.

- [ ] **Step 4: Implement `src/core/refresh-policy.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0

/**
 * Should the access token be refreshed now?
 *
 * Pure and total. The clock is an ARGUMENT, not a dependency — the caller that already performs
 * I/O reads it. In the single-flight loop the caller re-reads it ONCE PER ITERATION; binding it
 * once outside the loop makes the waiter's deadline unreachable and the loop never terminates.
 */
export function shouldRefresh(now: number, expiresAt: number, skewMs: number): boolean {
  return now >= expiresAt - skewMs;
}
```

- [ ] **Step 5: Implement `src/core/return-to.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0

/**
 * Accept `raw` only as a same-origin relative path; otherwise return `fallback`.
 *
 * This is an OPEN REDIRECT surface. The rules, in order:
 *   - must be a string with no leading or trailing whitespace (a leading tab or space would
 *     otherwise let "\thttps://evil.com" past a naive startsWith check);
 *   - must start with "/";
 *   - must not start with "//" or "/\" — both are protocol-relative to a browser;
 *   - must contain no backslash anywhere, because browsers normalise "\" to "/" in URLs;
 *   - must not contain a percent-encoded slash or backslash, which a downstream decode would
 *     turn back into one of the cases above.
 *
 * CR/LF is deliberately NOT checked here. The response is built with the Web `Headers` API, which
 * rejects a header value containing them — the mitigation is the API, not this validator. If
 * anyone ever writes a Location header by hand, this comment is the reason that breaks.
 */
export function validateReturnTo(raw: string | undefined | null, fallback: string): string {
  if (typeof raw !== 'string') return fallback;
  if (raw !== raw.trim() || raw.length === 0) return fallback;
  if (!raw.startsWith('/')) return fallback;
  if (raw.startsWith('//')) return fallback;
  if (raw.includes('\\')) return fallback;
  // slice(1, 4), NOT slice(0, 3). The input starts with a literal '/', so the three characters
  // that could spell an encoded slash begin at index 1. slice(0, 3) of "/%2f%2fevil.com" is "/%2",
  // which /%2f/i does NOT match — the guard misses and the open redirect passes. MEASURED.
  if (/%2f/i.test(raw.slice(1, 4)) || /%5c/i.test(raw)) return fallback;
  return raw;
}
```

- [ ] **Step 6: Run and watch those two pass**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/refresh-policy.test.ts tests/core/return-to.test.ts
```

Expected: PASS. If `'/%2f%2fevil.com'` fails, the slice bound is wrong — it must cover the three characters after the leading `/`.

- [ ] **Step 7: Write the failing test for the session view**

`tests/core/session.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, expectTypeOf, it } from 'vitest';
import { SESSION_VIEW_KEYS, toSessionView } from '../../src/core/session.js';
import type { SessionRecord, SessionView } from '../../src/core/session.js';

const RECORD: SessionRecord = {
  version: 1,
  rev: 3,
  accessToken: 'AT-secret',
  refreshToken: 'RT-secret',
  accessExpiresAt: 2_000_000,
  absoluteExpiresAt: 9_000_000,
  idTokenClaims: { iss: 'https://idp', sub: 'u1', email: 'a@b.c', name: 'Alice' },
  principal: {
    principalPrn: 'prn:pgs:iam::org1:user/8f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8',
    issuer: 'https://idp',
    subject: 'u1',
    memberships: [],
    roleGrants: [{ scopePrn: 'prn:pgs:iam::org1:org/8f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8', roleKey: 'admin' }],
    grantsAvailable: false,
  },
};

describe('toSessionView', () => {
  it('emits exactly the pinned key set', () => {
    // STRICT EQUALITY, and it is the point. Widening what reaches the browser must be a
    // deliberate edit to this list, never a side effect of adding a field to SessionRecord.
    expect(Object.keys(toSessionView(RECORD)).sort()).toEqual([...SESSION_VIEW_KEYS].sort());
  });

  it('carries no token field under any name', () => {
    const serialised = JSON.stringify(toSessionView(RECORD));
    expect(serialised).not.toContain('AT-secret');
    expect(serialised).not.toContain('RT-secret');
    for (const k of Object.keys(toSessionView(RECORD))) {
      expect(k.toLowerCase()).not.toContain('token');
    }
  });

  it('projects identity and grants', () => {
    const v = toSessionView(RECORD);
    expect(v.principalPrn).toBe(RECORD.principal.principalPrn);
    expect(v.displayName).toBe('Alice');
    expect(v.email).toBe('a@b.c');
    expect(v.grants).toEqual(RECORD.principal.roleGrants);
    expect(v.grantsAvailable).toBe(false);
  });

  it('falls back to the email local part when no name claim is present', () => {
    const { name: _dropped, ...claims } = RECORD.idTokenClaims;
    expect(toSessionView({ ...RECORD, idTokenClaims: claims }).displayName).toBe('a');
  });

  it('a SessionRecord is not assignable to a SessionView', () => {
    // Enforced by `tsc --noEmit` in the build/typecheck tasks, NOT by `vitest run` — the inherited
    // test task passes no --typecheck, so expectTypeOf is a runtime no-op there. It works only
    // because tsconfig.json includes tests/**/*. Do not "simplify" either half.
    expectTypeOf<SessionRecord>().not.toMatchTypeOf<SessionView>();
  });
});
```

- [ ] **Step 8: Implement `src/ports/principal-resolver.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The port speaks DOMAIN types, not proto types. RoleGrantRef and Membership exist as protobuf
// messages in @paigasus/proto, and importing them would make @paigasus/auth a dependent of that
// package — contradicting the § 6 dependency graph (apps → {sdk, auth/server}; sdk → proto) and
// putting paigasus-auth-ts into ci/affected-graph's strict-equality contracts->proto set.
//
// When IntrospectPrincipalResolver lands in SMA-508, the ADAPTER maps proto to these types.

/** A role grant, flattened from IAM's RoleGrantRef { scope_prn, role_key }. */
export interface RoleGrantRef {
  scopePrn: string;
  roleKey: string;
}

/** A membership, flattened from IAM's Membership { id, principal_prn, node_prn }. */
export interface Membership {
  id: string;
  principalPrn: string;
  nodePrn: string;
}

export interface ResolvedPrincipal {
  /** null until SMA-508 wires Introspect — IAM mints the PRN, the claims cannot. */
  principalPrn: string | null;
  issuer: string;
  subject: string;
  memberships: Membership[];
  roleGrants: RoleGrantRef[];
  /**
   * False while the claims-based resolver is in use. Consumers MUST treat an unavailable grant
   * set as "unknown", never as "denied" — see can() in src/client.ts.
   */
  grantsAvailable: boolean;
}

export interface PrincipalResolver {
  resolve(input: { accessToken: string; idTokenClaims: IdTokenClaims }): Promise<ResolvedPrincipal>;
}

export interface IdTokenClaims {
  iss: string;
  sub: string;
  email?: string;
  name?: string;
  [claim: string]: unknown;
}
```

- [ ] **Step 9: Implement `src/core/session.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
import type { IdTokenClaims, ResolvedPrincipal, RoleGrantRef } from '../ports/principal-resolver.js';

export interface SessionRecord {
  /**
   * A mismatch on read is treated as ABSENT and the record is deleted. There is no migration
   * path by design; the alternative is deserialising into a wrong-typed object.
   * OPERATIONAL CONSEQUENCE: the deploy that bumps this logs out every active user. Say so in
   * the release note.
   */
  version: 1;
  /**
   * Fencing counter. `SessionStore.set` is a compare-and-set on it. Without this, a refresh that
   * outlives its lock TTL lets the slow holder write its now-revoked token over a newer valid
   * one — a lock's compare-and-delete protects the LOCK, never the WRITE.
   */
  rev: number;
  accessToken: string;
  refreshToken?: string;
  accessExpiresAt: number;
  /** loginTime + ABSOLUTE_TTL. Never extended by a refresh; a refresh clamps to it. */
  absoluteExpiresAt: number;
  idTokenClaims: IdTokenClaims;
  principal: ResolvedPrincipal;
}

/**
 * Everything /client may ever see. A runtime tuple, not just a type: an interface has no runtime
 * key set, so a strict-equality assertion needs something to compare against.
 */
export const SESSION_VIEW_KEYS = ['principalPrn', 'displayName', 'email', 'grants', 'grantsAvailable'] as const;

export type SessionViewKey = (typeof SESSION_VIEW_KEYS)[number];

export interface SessionView {
  principalPrn: string | null;
  displayName: string | null;
  email: string | null;
  grants: RoleGrantRef[];
  grantsAvailable: boolean;
}

/** The ONLY function that may construct what crosses to the browser. */
export function toSessionView(rec: SessionRecord): SessionView {
  const email = typeof rec.idTokenClaims.email === 'string' ? rec.idTokenClaims.email : null;
  const name = typeof rec.idTokenClaims.name === 'string' ? rec.idTokenClaims.name : null;
  return {
    principalPrn: rec.principal.principalPrn,
    displayName: name ?? (email === null ? null : (email.split('@')[0] ?? null)),
    email,
    grants: rec.principal.roleGrants,
    grantsAvailable: rec.principal.grantsAvailable,
  };
}
```

- [ ] **Step 10: Write the failing test for ids**

`tests/core/ids.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { hashSecret, newLockToken, newSessionId, newTransactionId, newTransactionSecret, secretMatchesHash } from '../../src/core/ids.js';

describe('ids', () => {
  it('a session id is 32 bytes of base64url', () => {
    const id = newSessionId();
    expect(id).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(Buffer.from(id, 'base64url')).toHaveLength(32);
  });

  it('session ids do not repeat across 1000 draws', () => {
    expect(new Set(Array.from({ length: 1000 }, newSessionId)).size).toBe(1000);
  });

  it('lock tokens and transaction ids are distinct values', () => {
    expect(newLockToken()).not.toBe(newLockToken());
    expect(newTransactionId()).not.toBe(newTransactionId());
  });

  it('a secret matches only its own hash', () => {
    const s = newTransactionSecret();
    const h = hashSecret(s);
    expect(secretMatchesHash(s, h)).toBe(true);
    expect(secretMatchesHash(newTransactionSecret(), h)).toBe(false);
  });

  it('a hash comparison against a wrong-length input is false, not a throw', () => {
    // timingSafeEqual throws on a length mismatch; the wrapper must not propagate that.
    expect(secretMatchesHash('short', hashSecret(newTransactionSecret()))).toBe(false);
    expect(secretMatchesHash(newTransactionSecret(), 'not-a-hash')).toBe(false);
  });
});
```

- [ ] **Step 11: Implement `src/core/ids.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
import { createHash, randomBytes, timingSafeEqual } from 'node:crypto';

const bytes = (n: number): string => randomBytes(n).toString('base64url');

/** Opaque, 32 bytes. This is the only thing the browser ever holds. */
export const newSessionId = (): string => bytes(32);

/** Unique per acquisition, so compare-and-delete cannot release someone else's lock. */
export const newLockToken = (): string => bytes(16);

/** Short id for the per-transaction cookie name and the `state` parameter. */
export const newTransactionId = (): string => bytes(9);

/** The browser-bound secret. Only its SHA-256 is stored server-side. */
export const newTransactionSecret = (): string => bytes(32);

export const hashSecret = (secret: string): string => createHash('sha256').update(secret).digest('base64url');

/**
 * Constant-time comparison. The realistic exposure is low — an attacker in the login-CSRF
 * scenario already holds their own secret — but timingSafeEqual costs nothing and removes the
 * argument entirely. It THROWS on a length mismatch, so the lengths are checked first and a
 * mismatch returns false rather than propagating.
 */
export function secretMatchesHash(secret: string, expectedHash: string): boolean {
  const actual = Buffer.from(hashSecret(secret), 'base64url');
  const expected = Buffer.from(expectedHash, 'base64url');
  if (actual.length !== expected.length) return false;
  return timingSafeEqual(actual, expected);
}
```

- [ ] **Step 12: Implement `src/core/errors.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0

/** Base for every error this package raises. Carries no secret in its message, ever. */
export abstract class AuthError extends Error {
  abstract readonly code: string;
}

/** The store could not be reached. Callers treat this as "no session", never as a 500. */
export class SessionStoreUnavailable extends AuthError {
  readonly code = 'session_store_unavailable';
}

/** Configuration is internally inconsistent. Thrown by createAuthRuntime at first request. */
export class AuthConfigError extends AuthError {
  readonly code = 'auth_config_invalid';
}

/** The callback was rejected before any token exchange. `reason` is a closed vocabulary. */
export class CallbackRejected extends AuthError {
  readonly code = 'callback_rejected';
  constructor(readonly reason: 'txn_missing' | 'txn_mismatch' | 'state_unknown' | 'code_exchange_failed') {
    super(`callback rejected: ${reason}`);
  }
}
```

- [ ] **Step 13: Run the whole suite**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run
moon run paigasus-auth-ts:typecheck
```

Expected: PASS. The `expectTypeOf` assertion is checked by `typecheck`, not by `vitest run`.

- [ ] **Step 14: Commit**

```bash
git add ts/packages/paigasus-auth
git commit -m "feat(ts): add the pure core of @paigasus/auth" \
  -m "Adds shouldRefresh, validateReturnTo, the session record and its sanitized
view, id and secret generation, and the error taxonomy. All pure and total,
with the clock passed as an argument rather than injected.

SESSION_VIEW_KEYS is a runtime tuple so the key set that reaches the browser
can be pinned by strict equality; an interface alone has none."
```

---

## Task 3: The `SessionStore` port and the in-memory adapter

**Files:**
- Create: `src/ports/session-store.ts`, `src/adapters/memory-store.ts`
- Create: `tests/store-contract.ts` (a shared suite, not a `.test.ts`)
- Test: `tests/adapters/memory-store.test.ts`

**Interfaces:**
- Consumes: `SessionRecord` (Task 2).
- Produces: `SessionStore`, `LoginTransaction`, `MemorySessionStore`, and `runStoreContract(name, makeStore)` — reused verbatim by Task 4.

- [ ] **Step 1: Write the port**

`src/ports/session-store.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import type { SessionRecord } from '../core/session.js';

export interface LoginTransaction {
  codeVerifier: string;
  nonce: string;
  returnTo: string;
  /** SHA-256 of the browser-bound secret. The secret itself lives only in the cookie. */
  secretHash: string;
  createdAt: number;
}

/**
 * PRIMITIVES ONLY — no policy. The single-flight refresh algorithm lives once in
 * core/single-flight.ts and both adapters inherit it, rather than being implemented twice and
 * being right once.
 */
export interface SessionStore {
  get(sid: string): Promise<SessionRecord | null>;

  /**
   * Compare-and-set on `rev`. Returns false when the stored rev has moved, meaning another
   * writer fenced this one. A `null` expectedRev inserts only when no record exists.
   */
  set(sid: string, rec: SessionRecord, ttlMs: number, expectedRev: number | null): Promise<boolean>;

  delete(sid: string): Promise<void>;

  tryAcquireLock(sid: string, token: string, ttlMs: number): Promise<boolean>;
  /** Compare-and-delete: releases only if `token` still owns the lock. */
  releaseLock(sid: string, token: string): Promise<void>;

  putTransaction(txnId: string, tx: LoginTransaction, ttlMs: number): Promise<void>;
  /** ATOMIC get-and-delete. A separate get + delete is a race, so the port does not offer it. */
  takeTransaction(txnId: string): Promise<LoginTransaction | null>;

  close(): Promise<void>;
}
```

- [ ] **Step 2: Write the shared contract suite**

`tests/store-contract.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import type { SessionStore } from '../src/ports/session-store.js';
import type { SessionRecord } from '../src/core/session.js';

export function makeRecord(over: Partial<SessionRecord> = {}): SessionRecord {
  return {
    version: 1,
    rev: 0,
    accessToken: 'AT',
    refreshToken: 'RT',
    accessExpiresAt: Date.now() + 300_000,
    absoluteExpiresAt: Date.now() + 86_400_000,
    idTokenClaims: { iss: 'https://idp', sub: 'u1' },
    principal: { principalPrn: null, issuer: 'https://idp', subject: 'u1', memberships: [], roleGrants: [], grantsAvailable: false },
    ...over,
  };
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * ONE suite, run against EVERY adapter. This is what proves the port is honest: an adapter that
 * quietly differs on takeTransaction atomicity or compare-and-delete semantics fails here rather
 * than being discovered in production.
 */
export function runStoreContract(name: string, makeStore: () => Promise<SessionStore>): void {
  describe(`SessionStore contract: ${name}`, () => {
    it('returns null for an unknown sid', async () => {
      const s = await makeStore();
      try { expect(await s.get('nope')).toBeNull(); } finally { await s.close(); }
    });

    it('inserts with a null expectedRev and reads back', async () => {
      const s = await makeStore();
      try {
        expect(await s.set('a', makeRecord(), 60_000, null)).toBe(true);
        expect((await s.get('a'))?.accessToken).toBe('AT');
      } finally { await s.close(); }
    });

    it('refuses a second insert when one already exists', async () => {
      const s = await makeStore();
      try {
        await s.set('a', makeRecord(), 60_000, null);
        expect(await s.set('a', makeRecord({ accessToken: 'OTHER' }), 60_000, null)).toBe(false);
        expect((await s.get('a'))?.accessToken).toBe('AT');
      } finally { await s.close(); }
    });

    it('compare-and-set succeeds on a matching rev and fails on a stale one', async () => {
      const s = await makeStore();
      try {
        await s.set('a', makeRecord({ rev: 0 }), 60_000, null);
        expect(await s.set('a', makeRecord({ rev: 1, accessToken: 'AT2' }), 60_000, 0)).toBe(true);
        // This is the fencing case: a slow writer still holding rev 0 must be rejected.
        expect(await s.set('a', makeRecord({ rev: 1, accessToken: 'STALE' }), 60_000, 0)).toBe(false);
        expect((await s.get('a'))?.accessToken).toBe('AT2');
      } finally { await s.close(); }
    });

    it('deletes', async () => {
      const s = await makeStore();
      try {
        await s.set('a', makeRecord(), 60_000, null);
        await s.delete('a');
        expect(await s.get('a')).toBeNull();
      } finally { await s.close(); }
    });

    it('expires a record after its ttl', async () => {
      const s = await makeStore();
      try {
        await s.set('a', makeRecord(), 30, null);
        await sleep(80);
        expect(await s.get('a')).toBeNull();
      } finally { await s.close(); }
    });

    it('grants a lock to exactly one of two contenders', async () => {
      const s = await makeStore();
      try {
        expect(await s.tryAcquireLock('a', 'tok1', 5_000)).toBe(true);
        expect(await s.tryAcquireLock('a', 'tok2', 5_000)).toBe(false);
      } finally { await s.close(); }
    });

    it('releases a lock only to its owner', async () => {
      const s = await makeStore();
      try {
        await s.tryAcquireLock('a', 'tok1', 5_000);
        await s.releaseLock('a', 'wrong-token');
        expect(await s.tryAcquireLock('a', 'tok2', 5_000)).toBe(false);
        await s.releaseLock('a', 'tok1');
        expect(await s.tryAcquireLock('a', 'tok2', 5_000)).toBe(true);
      } finally { await s.close(); }
    });

    it('frees a lock when its ttl expires', async () => {
      const s = await makeStore();
      try {
        await s.tryAcquireLock('a', 'tok1', 30);
        await sleep(80);
        expect(await s.tryAcquireLock('a', 'tok2', 5_000)).toBe(true);
      } finally { await s.close(); }
    });

    it('takeTransaction is single-use', async () => {
      const s = await makeStore();
      try {
        await s.putTransaction('t1', { codeVerifier: 'v', nonce: 'n', returnTo: '/', secretHash: 'h', createdAt: Date.now() }, 60_000);
        expect((await s.takeTransaction('t1'))?.nonce).toBe('n');
        expect(await s.takeTransaction('t1')).toBeNull();
      } finally { await s.close(); }
    });

    it('takeTransaction is atomic under concurrency — exactly one caller wins', async () => {
      const s = await makeStore();
      try {
        await s.putTransaction('t1', { codeVerifier: 'v', nonce: 'n', returnTo: '/', secretHash: 'h', createdAt: Date.now() }, 60_000);
        const results = await Promise.all(Array.from({ length: 20 }, () => s.takeTransaction('t1')));
        expect(results.filter((r) => r !== null)).toHaveLength(1);
      } finally { await s.close(); }
    });
  });
}
```

- [ ] **Step 3: Run it and watch it fail**

Create `tests/adapters/memory-store.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { runStoreContract } from '../store-contract.js';

runStoreContract('memory', async () => new MemorySessionStore());
```

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters/memory-store.test.ts
```

Expected: FAIL — `MemorySessionStore` not found.

- [ ] **Step 4: Implement `src/adapters/memory-store.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SINGLE-PROCESS ONLY, and that is a correctness statement, not a caveat.
//
// The multi-zone topology runs one Next process per zone. Under this adapter the zones share
// nothing: a user who logs in on zone A is anonymous on zone B, and the single-flight lock is
// per-process, so AC 2's "exactly one refresh" does NOT hold across processes.
//
// createAuthRuntime therefore REFUSES this adapter when the zone map declares more than one zone.
// It is for local development and single-process tests.
import type { SessionRecord } from '../core/session.js';
import type { LoginTransaction, SessionStore } from '../ports/session-store.js';

interface Entry<T> { value: T; expiresAt: number; }

export class MemorySessionStore implements SessionStore {
  readonly #records = new Map<string, Entry<SessionRecord>>();
  readonly #locks = new Map<string, Entry<string>>();
  readonly #txns = new Map<string, Entry<LoginTransaction>>();

  #live<T>(map: Map<string, Entry<T>>, key: string): T | null {
    const hit = map.get(key);
    if (hit === undefined) return null;
    if (Date.now() >= hit.expiresAt) { map.delete(key); return null; }
    return hit.value;
  }

  get(sid: string): Promise<SessionRecord | null> {
    const rec = this.#live(this.#records, sid);
    // A version mismatch is treated as ABSENT, matching the Redis adapter.
    if (rec !== null && rec.version !== 1) { this.#records.delete(sid); return Promise.resolve(null); }
    return Promise.resolve(rec);
  }

  set(sid: string, rec: SessionRecord, ttlMs: number, expectedRev: number | null): Promise<boolean> {
    const current = this.#live(this.#records, sid);
    if (expectedRev === null) { if (current !== null) return Promise.resolve(false); }
    else if (current === null || current.rev !== expectedRev) return Promise.resolve(false);
    this.#records.set(sid, { value: rec, expiresAt: Date.now() + ttlMs });
    return Promise.resolve(true);
  }

  delete(sid: string): Promise<void> { this.#records.delete(sid); return Promise.resolve(); }

  tryAcquireLock(sid: string, token: string, ttlMs: number): Promise<boolean> {
    if (this.#live(this.#locks, sid) !== null) return Promise.resolve(false);
    this.#locks.set(sid, { value: token, expiresAt: Date.now() + ttlMs });
    return Promise.resolve(true);
  }

  releaseLock(sid: string, token: string): Promise<void> {
    if (this.#live(this.#locks, sid) === token) this.#locks.delete(sid);
    return Promise.resolve();
  }

  putTransaction(txnId: string, tx: LoginTransaction, ttlMs: number): Promise<void> {
    this.#txns.set(txnId, { value: tx, expiresAt: Date.now() + ttlMs });
    return Promise.resolve();
  }

  // Atomic by construction: JavaScript runs this synchronously to completion, so no other
  // caller can observe the entry between the read and the delete.
  takeTransaction(txnId: string): Promise<LoginTransaction | null> {
    const tx = this.#live(this.#txns, txnId);
    if (tx !== null) this.#txns.delete(txnId);
    return Promise.resolve(tx);
  }

  close(): Promise<void> {
    this.#records.clear(); this.#locks.clear(); this.#txns.clear();
    return Promise.resolve();
  }
}
```

- [ ] **Step 5: Run and watch it pass**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters/memory-store.test.ts
```

Expected: PASS, 11 tests.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-auth
git commit -m "feat(ts): add the SessionStore port and in-memory adapter" \
  -m "The port exposes primitives only — get, compare-and-set, delete, lock
acquire and compare-and-delete release, and an atomic take for the login
transaction. Refresh policy stays out of it so both adapters inherit one
tested algorithm.

The contract suite is shared and will be run verbatim against the Redis
adapter, which is what proves the port is honest."
```

---

## Task 4: The Redis adapter

**Files:**
- Create: `src/adapters/redis-store.ts`
- Test: `tests/containers/redis-store.test.ts`

**Interfaces:**
- Consumes: `SessionStore`, `runStoreContract` (Task 3).
- Produces: `createRedisSessionStore(opts): Promise<SessionStore>`.

This is the first task whose tests need Docker. They live under `tests/containers/`, which `vitest.config.ts` excludes from the default run and Task 13's `test-e2e` task includes.

- [ ] **Step 1: Measure the node-redis v6 API (M2)**

Do this before writing code — v6's client API differs from v5 and this plan must not guess.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
sed -n '1,80p' ts/node_modules/.pnpm/redis@*/node_modules/redis/dist/index.d.ts
grep -rn "GETDEL\|getDel" ts/node_modules/.pnpm/@redis+client@*/node_modules/@redis/client/dist/lib/commands/ | head
grep -rn "disableOfflineQueue\|reconnectStrategy" ts/node_modules/.pnpm/@redis+client@*/node_modules/@redis/client/dist/lib/client/index.d.ts | head
```

Record in `docs/superpowers/specs/2026-09-09-sma-506-measurements.md` under **M2**: the exact `set` options object shape, whether `getDel` exists, and the `eval` signature. Write the adapter against what you find, not against this plan's sketch.

- [ ] **Step 2: Write the failing test**

`tests/containers/redis-store.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { GenericContainer, type StartedTestContainer } from 'testcontainers';
import { afterAll, beforeAll } from 'vitest';
import { createRedisSessionStore } from '../../src/adapters/redis-store.js';
import { runStoreContract } from '../store-contract.js';

let container: StartedTestContainer;
let url: string;

// NO SKIP. If Docker is unreachable this FAILS. That is the whole reason the container suites
// live in their own Moon task: a task whose only purpose is container tests has no reason to
// skip, so there is no hatch, no canary, and no way to green a run that tested nothing.
beforeAll(async () => {
  container = await new GenericContainer('redis:8-alpine').withExposedPorts(6379).start();
  url = `redis://${container.getHost()}:${String(container.getMappedPort(6379))}/0`;
}, 120_000);

afterAll(async () => { await container?.stop(); });

runStoreContract('redis', async () =>
  createRedisSessionStore({ url, commandTimeoutMs: 1000, keyPrefix: `t${String(Date.now())}:` }),
);
```

The per-run `keyPrefix` keeps the contract suite's fixed sids (`'a'`, `'t1'`) from colliding between test files sharing one container.

- [ ] **Step 3: Run it and watch it fail**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run --config vitest.config.ts tests/containers/redis-store.test.ts
```

The default config excludes `tests/containers/`, so pass the path explicitly. Expected: FAIL — `createRedisSessionStore` not found. If it fails with a Docker connection error instead, start Docker; that is the intended fail-loud behaviour.

- [ ] **Step 4: Implement `src/adapters/redis-store.ts`**

Write it against the M2 measurement. The three Lua scripts are the substance:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Three operations need atomicity Redis commands alone do not give:
//   SET_CAS   — compare-and-set on `rev`, so a refresh that outlived its lock cannot write its
//               now-revoked token over a newer one.
//   UNLOCK    — compare-and-delete, so a holder whose TTL expired cannot release the next
//               holder's lock.
//   TAKE_TXN  — atomic get-and-delete, so `state` is single-use.
//
// TAKE_TXN could be GETDEL instead; it is a script so all three go through one code path and one
// error mode. Verify GETDEL's availability in M2 before choosing.

const SET_CAS = `
local cur = redis.call('GET', KEYS[1])
if ARGV[3] == '' then
  if cur then return 0 end
else
  if not cur then return 0 end
  local ok, parsed = pcall(cjson.decode, cur)
  -- tonumber on BOTH sides, never tostring. cjson.decode may render an integer rev as a Lua
  -- float, so tostring(3) yields "3.0" and a string compare against ARGV[3] = "3" fails --
  -- the CAS then rejects every legitimate write. MEASURED against redis:8-alpine.
  if not ok or tonumber(parsed.rev) ~= tonumber(ARGV[3]) then return 0 end
end
redis.call('SET', KEYS[1], ARGV[1], 'PX', tonumber(ARGV[2]))
return 1`;

const UNLOCK = `
if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) end
return 0`;

const TAKE_TXN = `
local v = redis.call('GET', KEYS[1])
if v then redis.call('DEL', KEYS[1]) end
return v`;
```

Requirements the implementation must meet, each with a stated reason:

- `createClient({ url, socket: { reconnectStrategy, connectTimeout }, disableOfflineQueue: true })`. **`disableOfflineQueue` is load-bearing**: node-redis queues commands while disconnected by default, so an outage becomes hung requests instead of fast failures, and every page in the console stalls rather than erroring.
- A bounded `reconnectStrategy` — return an `Error` past a retry cap so the client stops rather than retrying forever.
- Every command wrapped so a connection or timeout error becomes `SessionStoreUnavailable`. **Never** re-throw the node-redis error object: it embeds the DSN.
- `version !== 1` on read is treated as absent, and the key is deleted.
- Keys: `${keyPrefix}pgs:sess:${sid}`, `${keyPrefix}pgs:lock:${sid}`, `${keyPrefix}pgs:txn:${txnId}`.
- The DSN is held in a wrapper whose `toString` and `toJSON` return `'redis://<redacted>'`.

- [ ] **Step 5: Run and watch it pass**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run --config vitest.config.ts tests/containers/redis-store.test.ts
```

Expected: PASS, the same 11 contract tests that pass for memory. If the compare-and-set test fails, `cjson.decode` is likely reading `rev` as a float — `tostring(parsed.rev)` may render `3` as `3.0`. Compare numerically with `tonumber(...)` on both sides instead.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-auth
git commit -m "feat(ts): add the Redis session store adapter" \
  -m "Runs the same contract suite as the in-memory adapter, which is what proves
the port is honest. Compare-and-set, compare-and-delete unlock and the atomic
transaction take are Lua scripts.

disableOfflineQueue is set deliberately: node-redis queues commands while
disconnected by default, which turns an outage into hung requests instead of
fast failures."
```

---

## Task 5: Single-flight refresh — AC 2

**Files:**
- Create: `src/core/single-flight.ts`
- Test: `tests/core/single-flight.test.ts`, `tests/containers/single-flight-redis.test.ts`, `tests/containers/single-flight-multiprocess.test.ts`, `tests/fixtures/refresh-worker.ts`

**Interfaces:**
- Consumes: `SessionStore`, `shouldRefresh`, `SessionRecord`, `AuthLogger`.
- Produces: `resolveSession(deps, sid): Promise<SessionRecord | null>` where `deps` is `{ store, refresh, logger, skewMs, lockTtlMs, lockWaitMs, ttlMs }` and `refresh(refreshToken)` returns `{ accessToken, refreshToken?, expiresIn }`.

This is the headline acceptance criterion. **Step 8 is a mutation test and is not optional** — an AC 2 test that cannot fail proves nothing.

- [ ] **Step 1: Write the failing AC 2 test**

`tests/core/single-flight.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from 'vitest';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { noopLogger } from '../../src/adapters/noop-logger.js';
import { resolveSession } from '../../src/core/single-flight.js';
import { makeRecord } from '../store-contract.js';
import type { SessionStore } from '../../src/ports/session-store.js';

// REAL TIMERS THROUGHOUT. The waiter backs off with setTimeout; under vi.useFakeTimers() that
// never fires unless the test advances timers by hand, so the failure mode is a HANG rather than
// an assertion failure. Expiry is produced by writing a past accessExpiresAt, never by moving a
// clock — which is why this package needs no Clock port.
function deps(store: SessionStore, refresh: (rt: string) => Promise<{ accessToken: string; refreshToken?: string; expiresIn: number }>) {
  return { store, refresh, logger: noopLogger, skewMs: 30_000, lockTtlMs: 5_000, lockWaitMs: 3_000, ttlMs: 60_000 };
}

describe('resolveSession', () => {
  it('returns the record untouched when the token is fresh', async () => {
    const store = new MemorySessionStore();
    const refresh = vi.fn();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 600_000 }), 60_000, null);
    expect((await resolveSession(deps(store, refresh), 's'))?.accessToken).toBe('AT');
    expect(refresh).not.toHaveBeenCalled();
  });

  it('TWO CONCURRENT CALLERS ON AN EXPIRED TOKEN TRIGGER EXACTLY ONE REFRESH', async () => {
    const store = new MemorySessionStore();
    let calls = 0;
    const refresh = async (_rt: string) => {
      calls += 1;
      await new Promise((r) => setTimeout(r, 40)); // hold the lock long enough to force contention
      return { accessToken: `AT${String(calls)}`, refreshToken: `RT${String(calls)}`, expiresIn: 300 };
    };
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);

    const results = await Promise.all([resolveSession(deps(store, refresh), 's'), resolveSession(deps(store, refresh), 's')]);

    expect(calls).toBe(1);
    expect(results.map((r) => r?.accessToken)).toEqual(['AT1', 'AT1']);
  });

  it('TWENTY concurrent callers still trigger exactly one refresh', async () => {
    const store = new MemorySessionStore();
    let calls = 0;
    const refresh = async (_rt: string) => {
      calls += 1;
      await new Promise((r) => setTimeout(r, 40));
      return { accessToken: `AT${String(calls)}`, refreshToken: 'RT2', expiresIn: 300 };
    };
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);

    const results = await Promise.all(Array.from({ length: 20 }, () => resolveSession(deps(store, refresh), 's')));

    expect(calls).toBe(1);
    expect(new Set(results.map((r) => r?.accessToken))).toEqual(new Set(['AT1']));
  });

  it('the rotated refresh token replaces the old one', async () => {
    const store = new MemorySessionStore();
    const refresh = async (rt: string) => {
      expect(rt).toBe('RT');
      return { accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 };
    };
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    await resolveSession(deps(store, refresh), 's');
    expect((await store.get('s'))?.refreshToken).toBe('RT2');
  });

  it('increments rev on every refresh', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ rev: 7, accessExpiresAt: Date.now() - 1 }), 60_000, null);
    await resolveSession(deps(store, async () => ({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 })), 's');
    expect((await store.get('s'))?.rev).toBe(8);
  });

  it('returns null and deletes when the absolute cap has passed', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ absoluteExpiresAt: Date.now() - 1 }), 60_000, null);
    expect(await resolveSession(deps(store, async () => { throw new Error('must not refresh'); }), 's')).toBeNull();
    expect(await store.get('s')).toBeNull();
  });

  it('clamps a refreshed accessExpiresAt to the absolute cap', async () => {
    const store = new MemorySessionStore();
    const cap = Date.now() + 10_000;
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1, absoluteExpiresAt: cap }), 60_000, null);
    const out = await resolveSession(deps(store, async () => ({ accessToken: 'AT2', expiresIn: 3600 })), 's');
    expect(out?.accessExpiresAt).toBeLessThanOrEqual(cap);
  });

  it('returns null when the record has no refresh token', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ refreshToken: undefined, accessExpiresAt: Date.now() - 1 }), 60_000, null);
    expect(await resolveSession(deps(store, async () => { throw new Error('must not refresh'); }), 's')).toBeNull();
  });

  it('returns null when the record is deleted between the read and the lock', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const original = store.tryAcquireLock.bind(store);
    // Simulate a concurrent POST /auth/logout landing in the window.
    store.tryAcquireLock = async (sid, tok, ttl) => { await store.delete('s'); return original(sid, tok, ttl); };
    expect(await resolveSession(deps(store, async () => { throw new Error('must not refresh'); }), 's')).toBeNull();
  });

  it('returns the stale-but-live record when the lock cannot be won in time', async () => {
    const store = new MemorySessionStore();
    // Inside the skew window but NOT yet expired: still usable.
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 5_000 }), 60_000, null);
    await store.tryAcquireLock('s', 'someone-else', 60_000);
    const d = { ...deps(store, async () => { throw new Error('must not refresh'); }), lockWaitMs: 120 };
    const out = await resolveSession(d, 's');
    expect(out?.accessToken).toBe('AT');
    expect(out?.refreshPending).toBe(true);
  });

  it('returns null when the lock cannot be won and the token is genuinely expired', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    await store.tryAcquireLock('s', 'someone-else', 60_000);
    const d = { ...deps(store, async () => { throw new Error('must not refresh'); }), lockWaitMs: 120 };
    expect(await resolveSession(d, 's')).toBeNull();
  });

  it('deletes the record when the refreshed value cannot be persisted twice', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    store.set = async () => false; // every persist fails
    expect(await resolveSession(deps(store, async () => ({ accessToken: 'AT2', expiresIn: 300 })), 's')).toBeNull();
  });

  it('releases the lock when the refresh call throws', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    await expect(resolveSession(deps(store, async () => { throw new Error('idp down'); }), 's')).rejects.toThrow('idp down');
    // If the lock leaked, this would be false.
    expect(await store.tryAcquireLock('s', 'next', 5_000)).toBe(true);
  });
});
```

- [ ] **Step 2: Run and watch it fail**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight.test.ts
```

Expected: FAIL — `resolveSession` not found.

- [ ] **Step 3: Implement `src/core/single-flight.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
import { newLockToken } from './ids.js';
import { shouldRefresh } from './refresh-policy.js';
import type { SessionRecord } from './session.js';
import type { AuthLogger } from '../ports/logger.js';
import { sidTag } from '../ports/logger.js';
import type { SessionStore } from '../ports/session-store.js';

export interface RefreshedTokens {
  accessToken: string;
  refreshToken?: string;
  expiresIn: number; // seconds
}

export interface ResolveDeps {
  store: SessionStore;
  refresh: (refreshToken: string) => Promise<RefreshedTokens>;
  logger: AuthLogger;
  skewMs: number;
  lockTtlMs: number;
  lockWaitMs: number;
  ttlMs: number;
}

export type ResolvedSession = SessionRecord & { refreshPending?: boolean };

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Exponential with jitter. A fixed backoff synchronises every waiter onto the same wake-up. */
const backoff = (attempt: number): number => Math.min(25 * 2 ** attempt, 250) * (0.5 + Math.random());

/**
 * Resolve a session, refreshing under a per-session lock if the access token is at or past its
 * skew window. AT MOST ONE refresh happens per session across concurrent callers.
 *
 * FIVE INVARIANTS. Each is a defect if dropped:
 *
 *  1. The DOUBLE-CHECK after acquiring the lock. Without it two SEQUENTIAL holders both refresh,
 *     and the second presents a refresh token the IdP rotated and revoked a millisecond earlier.
 *     A lock alone does not prevent this — it is the least obvious point in the design.
 *  2. The waiter NEVER refreshes on timeout. Falling back to "refresh anyway" reinstates the race
 *     under exactly the load where it matters.
 *  3. A unique lock token with a compare-and-delete release, so a holder whose TTL expired cannot
 *     delete the next holder's lock.
 *  4. Release in `finally`, so a throwing IdP call does not hold the lock for its full TTL.
 *  5. The write is FENCED on `rev`. A refresh that outlives its lock TTL would otherwise write
 *     its now-revoked token over a newer valid one; compare-and-delete protects the LOCK, never
 *     the WRITE. The caller additionally asserts the refresh HTTP timeout is below lockTtlMs.
 *
 * `now` is re-read EVERY ITERATION. Binding it once makes the deadline unreachable and the loop
 * never terminates.
 */
export async function resolveSession(deps: ResolveDeps, sid: string): Promise<ResolvedSession | null> {
  const { store, refresh, logger, skewMs, lockTtlMs, lockWaitMs, ttlMs } = deps;

  const rec = await store.get(sid);
  if (rec === null) return null;
  if (Date.now() >= rec.absoluteExpiresAt) {
    await store.delete(sid);
    logger.event('session.deleted', { sid: sidTag(sid), reason: 'absolute_expiry' });
    return null;
  }
  if (!shouldRefresh(Date.now(), rec.accessExpiresAt, skewMs)) return rec;

  const lockToken = newLockToken();
  const deadline = Date.now() + lockWaitMs;

  for (let attempt = 0; ; attempt += 1) {
    if (await store.tryAcquireLock(sid, lockToken, lockTtlMs)) {
      try {
        const fresh = await store.get(sid);
        if (fresh === null) return null;                                   // concurrent logout
        if (!shouldRefresh(Date.now(), fresh.accessExpiresAt, skewMs)) return fresh;   // invariant 1
        if (fresh.refreshToken === undefined) {
          await store.delete(sid);
          logger.event('session.deleted', { sid: sidTag(sid), reason: 'no_refresh_token' });
          return null;
        }

        const tokens = await refresh(fresh.refreshToken);
        const next: SessionRecord = {
          ...fresh,
          rev: fresh.rev + 1,
          accessToken: tokens.accessToken,
          refreshToken: tokens.refreshToken ?? fresh.refreshToken,
          accessExpiresAt: Math.min(Date.now() + tokens.expiresIn * 1000, fresh.absoluteExpiresAt),
        };

        // Invariant 5. A false here means a newer writer fenced us; take their record.
        let ok = await store.set(sid, next, ttlMs, fresh.rev);
        if (!ok) {
          const winner = await store.get(sid);
          if (winner !== null) return winner;
          ok = await store.set(sid, next, ttlMs, null);
        }
        if (!ok) {
          // Retried once and still could not persist. A clean re-login beats a session that can
          // never refresh again, so delete rather than leave the revoked token in place.
          await store.delete(sid);
          logger.event('session.refresh.persist_failed', { sid: sidTag(sid) });
          return null;
        }

        logger.event('session.refreshed', { sid: sidTag(sid), rev: next.rev });
        return next;
      } finally {
        await store.releaseLock(sid, lockToken);                            // invariant 4
      }
    }

    if (Date.now() >= deadline) {                                           // invariant 2
      logger.event('session.refresh_timeout', { sid: sidTag(sid) });
      const last = await store.get(sid);
      if (last === null) return null;
      // Still inside the skew window means the access token is live: proceed on it and let the
      // next request refresh. Only a genuinely expired token degrades to "signed out", which has
      // a defined recovery path, rather than to a 500 from a server component.
      return Date.now() < last.accessExpiresAt ? { ...last, refreshPending: true } : null;
    }

    await sleep(backoff(attempt));
    const reread = await store.get(sid);
    if (reread === null) return null;
    if (!shouldRefresh(Date.now(), reread.accessExpiresAt, skewMs)) return reread;
  }
}
```

- [ ] **Step 4: Run and watch it pass**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight.test.ts
```

Expected: PASS, 13 tests.

- [ ] **Step 5: Add the Redis-backed run of the same tests**

`tests/containers/single-flight-redis.test.ts` — same body as Step 1's concurrency tests, but `makeStore` returns a `createRedisSessionStore(...)`. What this proves that the memory run does not is that **the Lua compare-and-set and compare-and-delete scripts are correct**; the memory adapter's synchronous checks cannot exercise them.

- [ ] **Step 6: Add the two-process test**

`tests/fixtures/refresh-worker.ts` connects to a Redis URL from `process.argv[2]`, calls `resolveSession` against a shared sid with a refresh function that increments a counter **in Redis** (`INCR refresh:count`), and prints the resulting access token as JSON.

`tests/containers/single-flight-multiprocess.test.ts` starts Redis, seeds an expired record, forks two workers with `child_process.fork`, waits for both, and asserts `GET refresh:count` is `"1"` and both workers printed the same access token.

Without this, AC 2's claim is proven only within one process — and the failure it exists to prevent is two **zones**, which are two processes.

- [ ] **Step 7: Run the container tests**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run --config vitest.config.ts tests/containers/
```

Expected: PASS.

- [ ] **Step 8: M5 — prove the test can fail (MANDATORY)**

Comment out the `if (await store.tryAcquireLock(...))` guard so every caller refreshes:

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight.test.ts
```

Expected: the "exactly one refresh" tests **FAIL** with `calls` greater than 1. Record the observed number in the measurements document under **M5**, then restore the guard.

**Restore by deleting the comment markers you added, not with `git checkout --`** — a checkout would also revert the uncommitted implementation from Step 3, and the next run would then fail against original code for an unrelated reason.

Re-run to confirm green before committing.

- [ ] **Step 9: Commit**

```bash
git add ts/packages/paigasus-auth
git commit -m "feat(ts): add single-flight token refresh under a per-session lock" \
  -m "Two concurrent requests on an expired token now trigger exactly one refresh,
proven in-process, against Redis, and across two forked processes.

The write is fenced on a rev counter. A lock's compare-and-delete protects
the lock, not the write, so a refresh that outlived its TTL could otherwise
overwrite a newer valid token with a revoked one.

Verified the test can fail: with the lock acquisition removed, the refresh
counter exceeds one."
```

---

## Task 6: The claims-based principal resolver

**Files:**
- Create: `src/adapters/claims-resolver.ts`
- Test: `tests/adapters/claims-resolver.test.ts`

**Interfaces:**
- Consumes: `PrincipalResolver`, `ResolvedPrincipal`, `IdTokenClaims`.
- Produces: `claimsPrincipalResolver: PrincipalResolver`.

- [ ] **Step 1: Write the failing test**

`tests/adapters/claims-resolver.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { claimsPrincipalResolver } from '../../src/adapters/claims-resolver.js';

describe('claimsPrincipalResolver', () => {
  it('derives issuer and subject from the claims', async () => {
    const p = await claimsPrincipalResolver.resolve({
      accessToken: 'AT',
      idTokenClaims: { iss: 'https://idp/realms/x', sub: 'user-1' },
    });
    expect(p.issuer).toBe('https://idp/realms/x');
    expect(p.subject).toBe('user-1');
  });

  it('reports no PRN — only IAM can mint one', async () => {
    const p = await claimsPrincipalResolver.resolve({ accessToken: 'AT', idTokenClaims: { iss: 'i', sub: 's' } });
    expect(p.principalPrn).toBeNull();
  });

  it('reports grants as UNAVAILABLE, not as empty-and-known', async () => {
    const p = await claimsPrincipalResolver.resolve({ accessToken: 'AT', idTokenClaims: { iss: 'i', sub: 's' } });
    expect(p.grantsAvailable).toBe(false);
    expect(p.roleGrants).toEqual([]);
    expect(p.memberships).toEqual([]);
  });

  it('never returns the access token in the principal', async () => {
    const p = await claimsPrincipalResolver.resolve({ accessToken: 'AT-secret', idTokenClaims: { iss: 'i', sub: 's' } });
    expect(JSON.stringify(p)).not.toContain('AT-secret');
  });
});
```

- [ ] **Step 2: Run and watch it fail**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters/claims-resolver.test.ts
```

- [ ] **Step 3: Implement**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The resolver that ships TODAY. It derives identity from the verified ID-token claims and
// reports the grant set as UNAVAILABLE.
//
// IAM's Introspect is what mints a principal PRN and returns memberships and role grants, and it
// is not reachable from TypeScript yet: contracts/buf.gen.yaml runs only bufbuild/es for TS
// (descriptors, no client), @paigasus/proto exports only common/v1, and @paigasus/sdk is a stub.
// SMA-508 lands the transport; IntrospectPrincipalResolver then slots in behind this same port
// with no change to the session shape or the store.
//
// grantsAvailable: false is NOT the same as "no grants". can() must treat it as unknown and fail
// OPEN, or a console gating navigation on it renders with no navigation at all.
import type { PrincipalResolver, ResolvedPrincipal } from '../ports/principal-resolver.js';

export const claimsPrincipalResolver: PrincipalResolver = {
  resolve({ idTokenClaims }): Promise<ResolvedPrincipal> {
    return Promise.resolve({
      principalPrn: null,
      issuer: idTokenClaims.iss,
      subject: idTokenClaims.sub,
      memberships: [],
      roleGrants: [],
      grantsAvailable: false,
    });
  },
};
```

- [ ] **Step 4: Run and watch it pass, then commit**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters/claims-resolver.test.ts
git add ts/packages/paigasus-auth
git commit -m "feat(ts): add the claims-based principal resolver" \
  -m "Derives issuer and subject from the verified ID token and reports the grant
set as unavailable rather than empty. IAM's Introspect mints the PRN and the
grants, and is not reachable from TypeScript until SMA-508 lands a transport."
```

---

## Task 7: The OIDC adapter and the composition root

**Files:**
- Create: `src/adapters/oidc.ts`, `src/runtime.ts`
- Test: `tests/adapters/oidc.test.ts`, `tests/runtime.test.ts`, `tests/fixtures/jwks.ts`

**Interfaces:**
- Consumes: `AuthEnv`, `SessionStore`, `PrincipalResolver`, `AuthLogger`, `SessionStoreUnavailable`, `AuthConfigError`.
- Produces: `createAuthRuntime(cfg, deps?): Promise<AuthRuntime>`; `AuthRuntime` = `{ store, resolver, logger, oidc, redirectUri, postLogoutRedirectUri, cookieDomainless: true, skewMs, lockTtlMs, lockWaitMs, ttlMs, absoluteTtlMs, zone, basePath }`.

- [ ] **Step 1: Measure the `openid-client` v6 API (M1)**

```bash
sed -n '1,120p' ts/node_modules/.pnpm/openid-client@*/node_modules/openid-client/build/index.d.ts
grep -n "export declare function discovery\|authorizationCodeGrant\|refreshTokenGrant\|buildAuthorizationUrl\|tokenRevocation\|buildEndSessionUrl\|clockTolerance\|clockSkew" ts/node_modules/.pnpm/openid-client@*/node_modules/openid-client/build/index.d.ts
```

Record under **M1**: the exact names and signatures for discovery, building the authorization URL, the code grant, the refresh grant, revocation, the end-session URL, and **where clock tolerance is set** (v6 uses symbol-keyed properties on the `Configuration` object, not an options bag — confirm this). Write `src/adapters/oidc.ts` against what you find.

- [ ] **Step 2: Write the JWKS fixture**

`tests/fixtures/jwks.ts` uses `jose` to generate an RS256 keypair, expose a JWKS document, and mint ID tokens with overridable `iss`, `aud`, `sub`, `nonce`, `exp`, `iat` and signing key. It also stands up a tiny `node:http` server serving `/.well-known/openid-configuration` and `/jwks`, so `openid-client`'s real discovery path runs against it.

- [ ] **Step 3: Write the failing verification tests**

`tests/adapters/oidc.test.ts` asserts, against the local JWKS:

- a well-formed token is accepted;
- a token with the **wrong `iss`** is rejected;
- a token with the **wrong `aud`** is rejected;
- a token whose **`nonce` does not match** the expected nonce is rejected;
- a token signed by a **different key** is rejected;
- a token with **`alg: none`** is rejected;
- an **expired** token is rejected;
- a token **45 s in the future** is accepted under a 30 s clock tolerance;
- a token **120 s in the future** is rejected.

The last two are the clock-skew cases. Without a tolerance, a 60-second skew against the IdP fails **every** login, and there is no seam to discover that through later.

These prove **our wiring passes the right expectations**, not that `openid-client`'s cryptography works — that is its own library's job. The negative cases are the point.

- [ ] **Step 4: Implement `src/adapters/oidc.ts`**

Requirements:

- Discovery runs **once per process**, lazily at first use, and is cached. JWKS rotation is handled by `openid-client`'s keystore, not by re-running discovery.
- Every outbound call carries `PAIGASUS_OIDC_HTTP_TIMEOUT_MS`.
- Clock tolerance is set from `PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS`.
- The client secret is held in a redacting wrapper.
- No caught library error is logged directly — extract `name` and a fixed message. `openid-client` may embed a URL.

- [ ] **Step 5: Write the failing `createAuthRuntime` tests**

`tests/runtime.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { createAuthRuntime } from '../src/runtime.js';
import { AuthConfigError } from '../src/core/errors.js';

const BASE = {
  PAIGASUS_ZONE: 'iam',
  PAIGASUS_ZONES: { iam: '/iam' },
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.com',
  PAIGASUS_OIDC_CLIENT_ID: 'c', PAIGASUS_OIDC_CLIENT_SECRET: 's',
  PAIGASUS_PUBLIC_ORIGIN: 'https://app.example.com',
  PAIGASUS_OIDC_SCOPES: 'openid profile email offline_access',
  PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS: 30, PAIGASUS_OIDC_HTTP_TIMEOUT_MS: 3500,
  PAIGASUS_SESSION_STORE: 'memory' as const,
  PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 1000, PAIGASUS_SESSION_TTL_SECONDS: 28800,
  PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS: 86400, PAIGASUS_SESSION_REFRESH_SKEW_SECONDS: 30,
  PAIGASUS_SESSION_LOCK_TTL_MS: 5000, PAIGASUS_SESSION_LOCK_WAIT_MS: 3000,
};

describe('createAuthRuntime', () => {
  it('derives the redirect URI from the origin and the zone base path', async () => {
    const rt = await createAuthRuntime(BASE);
    expect(rt.redirectUri).toBe('https://app.example.com/iam/auth/callback');
    expect(rt.postLogoutRedirectUri).toBe('https://app.example.com/iam/');
  });

  it('honours an explicit redirect URI override', async () => {
    const rt = await createAuthRuntime({ ...BASE, PAIGASUS_OIDC_REDIRECT_URI: 'https://proxy/cb' });
    expect(rt.redirectUri).toBe('https://proxy/cb');
  });

  // The cross-field rule that a flat ZodRawShape cannot express.
  it('requires a redis url when the store is redis', async () => {
    await expect(createAuthRuntime({ ...BASE, PAIGASUS_SESSION_STORE: 'redis' })).rejects.toBeInstanceOf(AuthConfigError);
  });

  // memory is single-process: under multi-zone the zones share nothing and AC 2 does not hold.
  it('refuses the memory store when more than one zone is declared', async () => {
    await expect(
      createAuthRuntime({ ...BASE, PAIGASUS_ZONES: { iam: '/iam', gateway: '/gateway' } }),
    ).rejects.toBeInstanceOf(AuthConfigError);
  });

  it('allows the memory store for a single zone', async () => {
    await expect(createAuthRuntime(BASE)).resolves.toBeDefined();
  });

  // Invariant 5's second control.
  it('refuses an http timeout at or above the lock ttl', async () => {
    await expect(
      createAuthRuntime({ ...BASE, PAIGASUS_OIDC_HTTP_TIMEOUT_MS: 5000, PAIGASUS_SESSION_LOCK_TTL_MS: 5000 }),
    ).rejects.toBeInstanceOf(AuthConfigError);
  });

  it('refuses a zone with no entry in the zone map', async () => {
    await expect(createAuthRuntime({ ...BASE, PAIGASUS_ZONE: 'ghost' })).rejects.toBeInstanceOf(AuthConfigError);
  });

  it('accepts injected dependencies', async () => {
    const logger = { event: () => undefined };
    expect((await createAuthRuntime(BASE, { logger })).logger).toBe(logger);
  });
});
```

- [ ] **Step 6: Implement `src/runtime.ts`**

It owns every cross-field rule and every derivation the flat shape cannot express, plus adapter selection and discovery. Each `AuthConfigError` message names the variables involved and **never** renders a value.

- [ ] **Step 7: Run, then commit**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run
moon run paigasus-auth-ts:typecheck
git add ts/packages/paigasus-auth
git commit -m "feat(ts): wire openid-client and add the auth composition root" \
  -m "createAuthRuntime owns the cross-field configuration rules and the derived
redirect URIs, which a flat zod raw shape cannot express: defineRuntimeConfig
has no refinement hook and forbids an extra shape from declaring the zone keys.

It refuses the in-memory store under a multi-zone map, because that store is
single-process and would silently break the shared session, and it refuses an
HTTP timeout at or above the lock TTL, which the refresh fencing depends on."
```

---

## Task 8: Cookies, login, and callback

**Files:**
- Create: `src/http/cookies.ts`, `src/http/routes.ts`
- Test: `tests/http/cookies.test.ts`, `tests/http/login.test.ts`, `tests/http/callback.test.ts`

**Interfaces:**
- Consumes: everything from Tasks 2, 3, 6, 7.
- Produces: `createAuthRoutes(runtime): { handle(req: Request): Promise<Response> }`; `SESSION_COOKIE = '__Host-pgs_sid'`; `txnCookieName(txnId): string`; `serializeCookie`, `clearCookie`, `readCookies`.

- [ ] **Step 1: Write the failing cookie tests**

`tests/http/cookies.test.ts` asserts:

- `SESSION_COOKIE` is exactly `'__Host-pgs_sid'`;
- `txnCookieName('abc')` is `'__Host-pgs_txn_abc'`;
- a serialised session cookie contains `HttpOnly`, `Secure`, `SameSite=Lax`, `Path=/`;
- it contains **no `Domain=`** — the `__Host-` prefix forbids one, and a browser rejects the whole cookie if present;
- it carries **no `Max-Age`** and no `Expires` — it is a browser-session cookie, so nothing needs to re-issue it from a server component, where Next forbids cookie writes;
- `clearCookie` emits `Max-Age=0`;
- `readCookies` parses multiple cookies and returns the value for a name.

The `__Host-` prefix is the point of this file. Host-only (no `Domain`) constrains what *this* server writes; it does not stop a sibling `*.example.com` host writing `pgs_sid=...; Domain=example.com`, which arrives as a second value in the same header and most parsers take first. On a multi-zone console sharing one origin a sibling host is normal, not hypothetical. Browsers refuse a `Domain`-scoped write of a `__Host-` name.

- [ ] **Step 2: Write the failing login-route tests**

`tests/http/login.test.ts` asserts that `GET /auth/login`:

- responds 302 to the issuer's authorization endpoint;
- includes `code_challenge`, `code_challenge_method=S256`, `state`, `nonce`, and the derived `redirect_uri`;
- sets a `__Host-pgs_txn_<id>` cookie whose id matches the `state`;
- stores a transaction holding only the **SHA-256** of the cookie's secret — assert the raw secret does not appear in the stored record;
- **clears `__Host-pgs_sid`** in the same response (this is what kills a stale cookie, since a server component cannot);
- keeps at most 4 outstanding txn cookies, clearing the oldest on the 5th;
- validates `returnTo` through `validateReturnTo`, falling back to the zone base path.

Two tabs starting a login concurrently must both succeed — that is why the cookie is per transaction. A single fixed name at `Path=/` means the second tab overwrites the first tab's secret and the first tab's callback then fails its own security check.

- [ ] **Step 3: Write the failing callback tests**

`tests/http/callback.test.ts` asserts that `GET /auth/callback`:

- rejects with `CallbackRejected('txn_missing')` when no txn cookie is present, **before any token exchange** — assert the token endpoint was not called;
- rejects with `'txn_mismatch'` when the cookie's secret does not hash to the stored value;
- rejects with `'state_unknown'` when the state has no stored transaction (expired, or already used);
- rejects a **replayed** callback: the same state twice, the second attempt failing because `takeTransaction` is single-use;
- on success, issues a **new** session id and **deletes** any pre-existing record for the old sid (session fixation);
- writes a record whose `absoluteExpiresAt` is `now + ABSOLUTE_TTL`;
- redirects to the validated `returnTo`;
- clears every outstanding txn cookie.

The rejection tests are the login-CSRF defence. Without the browser-bound secret, an attacker authenticates as themselves, captures the callback URL without following it, sends it to a victim, and the victim's browser is issued a session **as the attacker** — everything the victim then does lands in the attacker's account.

- [ ] **Step 4: Run all three and watch them fail**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/http/
```

- [ ] **Step 5: Implement `src/http/cookies.ts` and the login and callback halves of `src/http/routes.ts`**

Handlers are Web-standard `(Request) => Promise<Response>`. Emit `login.started`, `session.created`, and `login.callback_rejected` with its `reason`, per the redaction contract.

- [ ] **Step 6: Run and watch them pass, then commit**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/http/
git add ts/packages/paigasus-auth
git commit -m "feat(ts): add the login and callback routes with browser-bound transactions" \
  -m "The login transaction is bound to the browser by a per-transaction __Host-
cookie holding a random secret, of which only the SHA-256 is stored. Without
that binding an attacker can capture their own callback URL, send it to a
victim, and have the victim issued a session as the attacker.

Cookies carry the __Host- prefix so a sibling subdomain cannot toss a
competing value into the same header, and the session cookie is a browser
session cookie because a Next server component cannot re-issue one."
```

---

## Task 9: Logout

**Files:**
- Modify: `src/http/routes.ts`
- Test: `tests/http/logout.test.ts`

**Interfaces:**
- Consumes: Task 8's route table.
- Produces: `POST /auth/logout` and `GET /auth/logout/callback` on the same handler.

- [ ] **Step 1: Write the failing tests**

`tests/http/logout.test.ts` asserts:

- `GET /auth/logout` returns **405** — a GET logout is triggerable by an `<img src>` from any page on the internet;
- `POST /auth/logout` deletes the record **before** any network call — assert the store is empty even when the IdP revocation call rejects;
- it clears `__Host-pgs_sid` and any outstanding txn cookies;
- it redirects to `end_session_endpoint` with `id_token_hint`, `post_logout_redirect_uri` and a `state`;
- a failing revocation call does **not** fail the logout;
- `GET /auth/logout/callback` with a valid state redirects to the zone root;
- `GET /auth/logout/callback` with an unknown state still redirects to the zone root and is **not** an error — the user is already logged out, because deletion happened first, so the callback is cosmetic;
- after logout, `resolveSession` with the old sid returns `null`.

No CSRF token is added, because `SameSite=Lax` already blocks a cross-site form POST. Asserted here so the reasoning is recorded rather than assumed.

- [ ] **Step 2: Run and watch it fail, implement, run and watch it pass**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/http/logout.test.ts
```

- [ ] **Step 3: Commit**

```bash
git add ts/packages/paigasus-auth
git commit -m "feat(ts): add server-side logout with IdP end-session redirect" \
  -m "Deletion happens before any network call, so a slow or unreachable identity
provider cannot leave a live session behind. Revocation is best effort and its
failure does not fail the logout.

Logout is POST because a GET logout is forgeable by an image tag on any page."
```

---

## Task 10: The Next binding, the middleware factory, and the client entry

**Files:**
- Create: `src/next/get-session.ts`, `src/server.ts`, `src/middleware.ts`, `src/client.ts`
- Test: `tests/next/get-session.test.ts`, `tests/middleware.test.ts`, `tests/client.test.tsx`

**Interfaces:**
- Produces: from `./server` — `createAuthRuntime`, `createAuthRoutes`, `getSession`, `requireSession`, `toSessionView`, `authEnvShape`, the ports and adapters. From `./middleware` — `createAuthMiddleware`. From `./client` — `SessionProvider`, `useSession`, `can`, and the `SessionView` type.

- [ ] **Step 1: Write the failing `getSession` / `requireSession` tests**

`tests/next/get-session.test.ts` asserts:

- `getSession()` returns `null` when no cookie is present, and **never redirects**;
- `getSession()` returns `null` when the cookie names a deleted record;
- `getSession()` returns `null` — not a throw — when the store is unavailable, so a Redis blip is a redirect to login rather than a 500;
- `requireSession()` calls Next's `redirect()` to `${loginPath}?returnTo=…` when `getSession()` is null;
- `requireSession()` returns the record when one exists.

This pair closes a real trap. When a record is deleted server-side the browser keeps sending the cookie; middleware checks presence only, so it does not redirect, and **a server component cannot delete a cookie in Next**. Without `requireSession`, every page would render unauthenticated forever with no route to login. Redirecting *is* allowed from a server component; writing a cookie is not — and `/auth/login` clears the stale cookie at the one place that may.

- [ ] **Step 2: Write the failing middleware tests**

`tests/middleware.test.ts` asserts:

- a request with no `__Host-pgs_sid` to a guarded path redirects to the login path with `returnTo`;
- a request with **any** cookie value — including a forged one — passes through: presence, never validity;
- a public path passes without a cookie;
- the module's transitive import graph reaches **no** store, resolver, or `openid-client` module.

That last assertion is AC 4. Middleware cannot check a grant because it cannot import anything that knows what a grant is.

- [ ] **Step 3: Write the failing client tests**

`tests/client.test.tsx` asserts:

- `useSession()` inside a `SessionProvider` returns the view;
- `useSession()` outside a provider throws a named error rather than returning undefined;
- **`can()` returns `true` when `grantsAvailable` is false**;
- `can()` returns `true` for a matching grant and `false` for a non-matching one when grants **are** available.

The failing-open rule is deliberate. With grants deferred to SMA-508, a `can()` that returned false for everything would make `@paigasus/app-shell` render with no navigation at all — the console looks broken, and IAM never gets the chance to answer authoritatively. Failing open is the correct direction for a control documented as cosmetic.

- [ ] **Step 4: Run all three and watch them fail, then implement**

`src/server.ts` opens with `import 'server-only';` as its first statement after the SPDX header. `src/middleware.ts` imports only the cookie name and Web APIs. `src/client.ts` opens with `'use client';` and imports only `react` and the `SessionView` type.

- [ ] **Step 5: Run and watch them pass, then commit**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run
git add ts/packages/paigasus-auth
git commit -m "feat(ts): add the Next binding, middleware factory and client entry" \
  -m "getSession never redirects and requireSession does, which is what recovers a
browser holding a cookie for a record that no longer exists — middleware
checks presence only and a server component cannot clear a cookie.

can() fails OPEN while the grant set is unavailable. A cosmetic control that
returned false for everything would render a console with no navigation."
```

---

## Task 11: The eslint boundary blocks

**Files:**
- Modify: `ts/packages/paigasus-next-config/src/eslint.mjs`
- Modify: `ts/packages/paigasus-next-config/tests/boundaries.test.ts`

- [ ] **Step 1: Read the derivation before writing any glob**

```bash
sed -n '108,132p' ts/packages/paigasus-next-config/tests/boundaries.test.ts
sed -n '33,45p' ts/packages/paigasus-next-config/tests/boundaries.test.ts
```

The test derives each rule's scope as `entry.files[0].split('/**')[0]` and asserts a **two-way** pairing with `BOUNDARY_SCOPES`; `expectedPackageName` maps `packages/paigasus-<x>` to `@paigasus/<x>`.

So `files[0]` **must** begin `packages/paigasus-auth/**`. A glob like `packages/paigasus-auth/src/client/**/*` derives `packages/paigasus-auth/src/client`, fails the reverse loop, and the obvious fix makes the liveness test hunt for a package named `@paigasus/auth/src/client`, which cannot exist.

- [ ] **Step 2: Add the DENIED and ALLOWED rows first**

In `tests/boundaries.test.ts`, add to `DENIED`:

```ts
['auth/client must not import openid-client', 'packages/paigasus-auth/src/client.ts', "import * as c from 'openid-client';"],
['auth/client must not import redis', 'packages/paigasus-auth/src/client.ts', "import { createClient } from 'redis';"],
['auth/client must not import a node builtin', 'packages/paigasus-auth/src/client.ts', "import { randomBytes } from 'node:crypto';"],
['auth/client must not import a BARE node builtin', 'packages/paigasus-auth/src/client.ts', "import { randomBytes } from 'crypto';"],
// SIBLING-RELATIVE. src/client.ts reaches src/adapters as './adapters/…', never '../'. A
// ../-only group is inert on the one file this rule exists to protect.
['auth/client must not reach adapters via ./', 'packages/paigasus-auth/src/client.ts', "import { x } from './adapters/redis-store.js';"],
['auth/client must not reach core via ./', 'packages/paigasus-auth/src/client.ts', "import { x } from './core/single-flight.js';"],
['auth/middleware must not import the store', 'packages/paigasus-auth/src/middleware.ts', "import { x } from './adapters/redis-store.js';"],
['an app middleware must not import auth/server', 'apps/paigasus-console/middleware.ts', "import { getSession } from '@paigasus/auth/server';"],
['an app middleware must not import the sdk', 'apps/paigasus-console/middleware.ts', "import { x } from '@paigasus/sdk';"],
```

And to `ALLOWED`:

```ts
['auth/client may import react', 'packages/paigasus-auth/src/client.ts', "import { createContext } from 'react';"],
['auth/server may import openid-client', 'packages/paigasus-auth/src/server.ts', "import * as c from 'openid-client';"],
['auth/server may reach its own adapters', 'packages/paigasus-auth/src/server.ts', "import { x } from './adapters/redis-store.js';"],
['an app middleware may import auth/middleware', 'apps/paigasus-console/middleware.ts', "import { createAuthMiddleware } from '@paigasus/auth/middleware';"],
```

The app-middleware rows are AC 4's other half. `src/middleware.ts` having no import path to a store says nothing about an app's own `middleware.ts` importing `@paigasus/auth/server` directly, and `server-only` is a **no-op** in the middleware layer.

- [ ] **Step 3: Run and watch them fail**

```bash
pnpm -C ts/packages/paigasus-next-config exec vitest run tests/boundaries.test.ts
```

Expected: the new DENIED rows fail — no rule matches them yet.

- [ ] **Step 4: Add the three blocks to `eslint.mjs`**

Append to `boundaryRules`, with `files[0]` beginning `packages/paigasus-auth/**` in the first two:

```js
{
  name: 'paigasus/boundaries/auth-client',
  files: [
    'packages/paigasus-auth/**/client.ts',
    'packages/paigasus-auth/**/client.tsx',
    'packages/paigasus-auth/src/client/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}',
  ],
  rules: restrict([
    {
      group: [
        'openid-client', 'redis', 'server-only',
        'node:*', 'crypto', 'fs', 'net', 'tls', 'http', 'https', 'stream', 'buffer',
        './adapters/**', './core/**', './ports/**', './http/**', './next/**', './runtime', './config',
        '../adapters/**', '../core/**', '../ports/**', '../http/**', '../next/**',
      ],
      message:
        '@paigasus/auth/client is React-only and must never reach the server surface — it would put a token in a browser bundle (AC 5). Import types from ./core/session.js only.',
    },
  ]),
},
{
  name: 'paigasus/boundaries/auth-middleware',
  files: ['packages/paigasus-auth/**/middleware.ts'],
  rules: restrict([
    {
      group: [
        'openid-client', 'redis',
        './adapters/**', './core/single-flight', './ports/session-store', './next/**',
        '../adapters/**', '../ports/**',
      ],
      message:
        'Next middleware does cookie-presence checks only (ADR-0017 decision 7; CVE-2025-29927 was a middleware auth bypass). Resolve the session in a server component or route handler.',
    },
  ]),
},
{
  name: 'paigasus/boundaries/app-middleware',
  files: ['apps/*/middleware.{ts,js,mts,cts,mjs,cjs}'],
  rules: restrict([
    {
      group: ['@paigasus/auth/server', '@paigasus/auth/server/**', '@paigasus/sdk', '@paigasus/sdk/**'],
      message:
        "An app's middleware must import @paigasus/auth/middleware, never /server or the sdk. `server-only` is a NO-OP in the middleware layer, so nothing else stops a token-bearing module being bundled there.",
    },
  ]),
},
```

Add to `BOUNDARY_SCOPES`:

```js
'packages/paigasus-auth': 'exists',
'apps/*/middleware.{ts,js,mts,cts,mjs,cjs}': 'a file glob, not a package directory — see the liveness note',
```

Two rules deriving the same `packages/paigasus-auth` key is fine: the forward loop uses `toContain` and the reverse loop checks membership.

- [ ] **Step 5: Correct the three stale issue references**

| Line | Currently | Correct |
|---|---|---|
| `eslint.mjs:36` | "INERT until SMA-506 lands" | **SMA-510** |
| `eslint.mjs:39` | "gains nothing when SMA-508 lands" | **SMA-506** |
| `eslint.mjs:53` | `'SMA-506 has not landed yet…'` | **SMA-510** |

SMA-506 is `@paigasus/auth`, SMA-508 is `@paigasus/sdk`, SMA-510 is `@paigasus/app-shell`. Line 53 is the one most easily missed: it reads as data rather than prose, and it sits beside the glob it describes. Also update line 39's claim that there is no separate auth scope — Step 4 makes it untrue.

- [ ] **Step 6: Run both packages' tests**

```bash
pnpm -C ts/packages/paigasus-next-config exec vitest run
moon run ts:lint
```

Expected: PASS. If the liveness test reports a package named `@paigasus/auth/src/client`, `files[0]` is wrong — return to Step 1.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-next-config
git commit -m "feat(ts): add package boundary rules for @paigasus/auth" \
  -m "Three blocks: the client entry may not reach the server surface, this
package's middleware may not reach the store, and an app's own middleware may
not import auth/server or the sdk.

The deny groups carry ./-relative forms as well as ../ ones, because the client
entry reaches a sibling directory as './adapters/…' and no-restricted-imports
matches the specifier as written. Bare node builtins are listed alongside the
node: forms for the same reason.

Also corrects three stale issue references that attributed the app-shell scope
to SMA-506 and the auth scope to SMA-508."
```

---

## Task 12: Structural tests

**Files:**
- Test: `tests/structure/exports.test.ts`, `tests/structure/import-graph.test.ts`

- [ ] **Step 1: Write the export-surface test**

Reads `package.json` and asserts `Object.keys(exports)` equals exactly `['./server', './client', './middleware']`.

**Adding `"."` must red this test.** A root export re-exporting the server surface would let `import { getSession } from '@paigasus/auth'` bypass the app-shell boundary rule, which names only the `/server` subpath.

- [ ] **Step 2: Write the import-graph test**

A helper walks static `import`/`export … from` specifiers transitively from an entry file, resolving relative paths and stopping at bare specifiers (which it records). Then:

- from `src/client.ts`: no visited file lies under `src/adapters/`, `src/core/single-flight`, `src/ports/session-store`, `src/http/` or `src/next/`, and the bare specifier set contains neither `openid-client` nor `redis`;
- from `src/middleware.ts`: the same;
- from `src/server.ts`: `openid-client` **is** reachable — a control proving the walker actually finds things rather than silently returning an empty set.

That last assertion is the guard-the-guard. Without it, a walker with a broken resolver returns an empty set and both real assertions pass vacuously.

- [ ] **Step 3: Run, then prove they bite**

```bash
pnpm -C ts/packages/paigasus-auth exec vitest run tests/structure/
```

Temporarily add `import { createClient } from 'redis';` to `src/client.ts` and re-run: the import-graph test must FAIL. Temporarily add `".": "./src/server.ts"` to `package.json`'s exports and re-run: the export test must FAIL. **Remove each by deleting the line you added**, never with `git checkout --`, which would also discard uncommitted work from earlier tasks.

- [ ] **Step 4: Commit**

```bash
git add ts/packages/paigasus-auth
git commit -m "test(ts): pin the auth export surface and client import graph" \
  -m "Strict equality on the exports key set, so adding a root export reds a test
rather than silently reopening a boundary bypass. Transitive import-graph
walks from the client and middleware entries, with a positive control from the
server entry so a broken walker cannot pass vacuously."
```

---

## Task 13: Playwright, testcontainers, and the fixture server

**Files:**
- Create: `playwright.config.ts`, `tests/e2e/fixture-server.ts`, `tests/e2e/global-setup.ts`, `tests/e2e/keycloak-realm.json`, `tests/e2e/roundtrip.spec.ts`, `tests/e2e/logout.spec.ts`, `tests/e2e/recovery.spec.ts`
- Modify: `ts/packages/paigasus-auth/moon.yml`

- [ ] **Step 1: Build the Keycloak realm fixture (M6)**

Start from `rs/crates/services/paigasus-iam/tests/fixtures/keycloak-realm.json`, but **the client cannot be reused**: `paigasus-cli` is `publicClient: true`, `standardFlowEnabled: false` — a password-grant client. Add a **confidential** client with `standardFlowEnabled: true`, `publicClient: false`, a `secret`, a registered `redirectUris` entry, a `postLogoutRedirectUris` entry, and `offline_access` in its default client scopes. Keep the `alice` / `alice-password` user. Record the working realm JSON under **M6**.

- [ ] **Step 2: Write `global-setup.ts`**

Starts Keycloak (`quay.io/keycloak/keycloak:26.4`, self-signed HTTPS on 8443, `--import-realm`, a 240 s startup timeout) and Redis via `testcontainers`, and exports their URLs. **No skip:** if Docker is unreachable this throws, which is the whole reason the container suites have their own Moon task.

- [ ] **Step 3: Write the fixture server**

A `node:http` server mounting `createAuthRoutes(...).handle` plus a public page and a `requireSession()`-guarded page.

**It must be started with `NODE_OPTIONS=--conditions=react-server`.** `src/server.ts` opens with `import 'server-only'`, whose exports map resolves to an unconditional `throw` under every condition except `react-server`. Vitest solves this with `ssr.resolve.conditions`; a plain Node process has no such setting, so without the flag the fixture server does not start at all. Record the result under **M8**.

- [ ] **Step 4: Write `playwright.config.ts`**

`globalSetup` points at Step 2. `webServer` runs the fixture server with that `NODE_OPTIONS` and `NODE_EXTRA_CA_CERTS` pointing at the Keycloak certificate (**M3**). The browser context sets `ignoreHTTPSErrors: true`.

- [ ] **Step 5: Write the specs**

`roundtrip.spec.ts` — **AC 1**: navigate to the guarded page, get redirected to Keycloak, fill `alice` / `alice-password`, submit, land back on the guarded page authenticated. Then assert via `context.cookies()` that the session cookie is named **`__Host-pgs_sid`** and carries `httpOnly: true`, `secure: true`, `sameSite: 'Lax'`, `path: '/'`, and **no domain beyond the host**. Assert no cookie value decodes to anything containing the access token. **M11** records whether `__Host-` cookies work over the fixture server's scheme in Chromium.

`roundtrip.spec.ts` also covers § 9.2: two tabs starting a login concurrently both complete.

`logout.spec.ts` — **AC 3**: log in, capture the session cookie value, POST the logout form, then **add that captured cookie into a fresh browser context** and load the guarded page. Assert it redirects to login. This proves the stolen-cookie property directly, rather than proving the cookie was cleared in the original browser.

`recovery.spec.ts`: log in, delete the record straight out of Redis, load the guarded page, assert a redirect to login rather than a blank authenticated-looking page. This is the § 10.1 trap.

- [ ] **Step 6: Add the `test-e2e` task**

In `ts/packages/paigasus-auth/moon.yml`:

```yaml
  # Container-backed tests. A task whose ONLY purpose is container tests has no reason to skip:
  # if Docker is unreachable this FAILS. That is why there is no PAIGASUS_SKIP_DOCKER-style hatch
  # and no canary test here — there is no code path that decides whether to run.
  #
  # `inputs` are declared in full because this is a NEW TASK NAME, not an override of `test`, so
  # it inherits nothing. Task inputs are the only thing conferring affectedness in Moon 2.5.3, and
  # an undeclared input means an edit to the realm fixture or the Playwright config serves a
  # cached PASS.
  test-e2e:
    command: 'pnpm exec vitest run --config vitest.config.ts tests/containers/ && pnpm exec playwright test'
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - 'playwright.config.ts'
      - 'vitest.config.ts'
      - 'tests/e2e/keycloak-realm.json'
      - 'package.json'
      - '/ts/pnpm-lock.yaml'
    options:
      cache: false
```

`cache: false` because the outcome depends on container state the hash cannot see.

- [ ] **Step 7: Run it**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec playwright install --with-deps chromium
moon run paigasus-auth-ts:test-e2e
```

Expected: PASS. Keycloak's first pull is slow; the 240 s startup timeout is deliberate.

- [ ] **Step 8: Commit**

```bash
git add ts/packages/paigasus-auth
git commit -m "test(ts): add the containerized Keycloak round trip for @paigasus/auth" \
  -m "Drives the full authorization-code flow in a real browser against Keycloak,
which is the only way to observe SameSite behaviour on the cross-site redirect
back from the identity provider and the __Host- cookie prefix.

Logout is proven by replaying the captured cookie in a fresh browser context.
Container tests live in their own Moon task, so an unreachable Docker daemon
fails rather than silently skipping."
```

---

## Task 14: CI registration and documentation

**Files:**
- Modify: `.github/workflows/ci.yml`, `CLAUDE.md`, `ci/affected-graph/run.sh`, `CONTRIBUTING.md`
- Create: `ts/packages/paigasus-auth/README.md`, `docs/superpowers/specs/2026-09-09-sma-506-measurements.md`

- [ ] **Step 1: Add `:test-e2e` to both registries**

`ci.yml`'s `T=(…)` array — keep it a **single-line bash array**. And the marker-delimited command in `CLAUDE.md` between `<!-- ci-targets:begin -->` and `<!-- ci-targets:end -->`. `ci/affected-graph/ci_targets.py` asserts the two agree, and **there must be exactly one copy of each marker in the file** — a second copy anywhere, even inside backticks in prose, reds `repo:affected-smoke`.

**Do not add a `T_EXEMPT` entry.** `moon ci` exits **0** on a target resolving to nothing, so the resolution assertion is the only thing standing between a typo and a silently disabled E2E tier.

- [ ] **Step 2: Add the browser install step to `ci.yml`**

Place it **before** the `moon ci` step, with **no `if:`**. A path-filtered condition can be skipped on a run where `moon ci` still selects `test-e2e` as affected, and the task then fails for a missing browser. An unconditional step costs every run; a wrong one reds a green PR.

The step is a `run:` block, so `repo:actionlint` and its shellcheck pass apply.

- [ ] **Step 3: Add the affected-graph row**

In `ci/affected-graph/run.sh`, a `run_task_case_ci` row anchoring an auth source file to `paigasus-auth-ts:{build,test,test-e2e}` and `ts:lint`. Nothing today asserts the new task is reachable from an edit to the package.

**Derive the expected set with the harness's own flags rather than editing the string to match an error message.** SMA-503 added `ui->console` and `ui-components->console` cases; this issue does not wire the console, so neither needs re-baselining.

- [ ] **Step 4: Write the measurements document**

`docs/superpowers/specs/2026-09-09-sma-506-measurements.md`, with a section per M-row. Every one must be **taken**, not predicted:

M1 `openid-client` v6 API · M2 node-redis v6 API · M3 CA handling · M4 `:test-e2e` resolution · **M5 the AC 2 mutation** · M6 the realm JSON · M7 `server-only` in middleware · M8 `--conditions=react-server` · M9 the `contracts->proto` claim · M10 CI disk and container contention · M11 `__Host-` over the fixture scheme.

- [ ] **Step 5: Write the package README**

Every variable from § 6.2 with its expected form. This is load-bearing, not decoration: `next-config`'s `describeIssues` renders a message only for `custom` issues on its **own** keys, so every validation failure here surfaces as `<key>: <code>` with no guidance. The README is the guidance, and widening the trusted-message allowlist would re-open the secret-leak that filter exists to close.

Document that the in-memory store is **single-process only** and does not satisfy AC 2 across processes.

- [ ] **Step 6: Update `CONTRIBUTING.md`**

The documented full-graph pre-push run now needs a Docker daemon and an installed Chromium. State it plainly. The repository already requires Docker for that run because `paigasus-iam-rs:test`'s suites are in `T`, so this is an increment on an existing requirement, not a new class of one.

- [ ] **Step 7: Run the full graph as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci :next-public-free :test-e2e --base origin/main --include-relations
```

If `repo:actionlint` or `repo:affected-smoke` hangs at zero CPU, that is this machine's bash here-string deadlock, **not** a gate failure. Re-run without those two targets and let CI judge them.

Diagnose any other failure with `.moon/cache/ciReport.json` — copy it and the task's state directory **before** re-running, because a passing re-run overwrites every artifact.

- [ ] **Step 8: Commit**

```bash
git add .github/workflows/ci.yml CLAUDE.md ci/affected-graph/run.sh CONTRIBUTING.md ts/packages/paigasus-auth/README.md docs/superpowers/specs/2026-09-09-sma-506-measurements.md
git commit -m "ci: register the auth end-to-end task and record its measurements" \
  -m "Adds test-e2e to the CI target array and the marker-delimited command, which
the affected-graph gate asserts agree, plus an unconditional browser install
step. A conditional one can be skipped on a run that still selects the task.

The README documents every environment variable because next-config renders
this package's validation failures as bare issue codes by design."
```

---

## Self-Review

**Spec coverage.** § 3.1 layout → Tasks 1–10. § 3.2 tsconfig → Task 1 Step 3. § 4 exports → Tasks 1, 12. § 5 boundaries → Task 11. § 6 config → Tasks 1, 7. § 7 failure policy → Tasks 4, 7, 10. § 8 session and single-flight → Tasks 2, 3, 4, 5. § 9 routes and cookies → Tasks 8, 9. § 10 middleware → Task 10. § 11 testing → every task, plus 12 and 13. § 12 observability → Task 1, emitted throughout. § 13 `can()` → Task 10. § 14 no proto dependency → Task 2 Step 8. § 15 dependencies → Task 1 Step 1. § 16 registration → Task 14. § 20 measurements → Task 14 Step 4, taken where they arise.

**Gap found and closed:** § 11.4's two-process AC 2 case had no task; it is Task 5 Step 6.

**Naming consistency.** `resolveSession` (not `getSessionRecord`) throughout Tasks 5, 9, 10. `SESSION_VIEW_KEYS`, `toSessionView`, `grantsAvailable` consistent across Tasks 2, 6, 10. `createAuthRuntime` returns the shape Task 8 consumes. `set(sid, rec, ttlMs, expectedRev)` is four-argument in the port, both adapters, and the single-flight caller.

**Ordering.** Task 4 needs Task 3's contract suite. Task 5 needs both adapters. Task 8 needs Task 7's runtime. Task 11 can run any time after Task 10 creates the three entry files. Task 13 needs Tasks 8–10.
