# SMA-512 pull request 3 — the `gateway-console` app

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `ts/apps/gateway-console`, the AI Gateway zone — its shell, its zone overview, its organization scope route and a single-zone end-to-end tier — and re-baseline every gate the second app changes.

**Architecture:** A second Next 16 App Router zone behind the same origin, at `basePath: '/gateway'`. It composes `@paigasus/console-core` exactly as `iam-console` does: one `defineRuntimeConfig` call, one `createConsoleRuntime` call, one auth composition root. It adds no new shared package. The two zones share one session through the `__Host-pgs_sid` cookie, one OIDC client and one Redis record; this pull request runs the zone alone, and pull request 4 proves the cross-zone behaviour.

**Tech Stack:** Next 16.3.4 (Turbopack, `output: 'standalone'`), React 19.2.8, TypeScript, vitest 5, Playwright, Moon 2.5.3, Tailwind v4.

**Spec:** `docs/superpowers/specs/2026-09-13-sma-512-gateway-console-design.md` (revision 2). Read § 4, § 6, § 7, § 8.4, § 9, § 10.1–10.4, § 10.6, § 11 and § 13 before Task 1.

**Base branch:** `main` at `f774ae6d`. **Branch:** `feature/sma-512-gateway-console`.

---

## Global Constraints

Every task's requirements implicitly include this section.

- **SPDX header on every source file:** `// SPDX-License-Identifier: Apache-2.0` as the first line.
- **Conventional commits with a workspace scope**, for example `feat(ts): …`. The subject must not start with an upper-case token, and **no body line may start with `word:`** — commitlint reads such a line as a footer token and fails `footer-leading-blank`.
- **Extensionless relative value imports.** Turbopack does not resolve `'./a.js'` to `a.ts`. Write `from './load'`, never `from './load.js'`.
- **`import 'server-only'`** is the first import of every file under a package's `src/` and of every app `lib/` file. It is **banned** from `proxy.ts` and from anything the middleware layer compiles.
- **`createConsoleRuntime` is called exactly once per app, at module scope, in `lib/console.ts`.** A second call makes a second `cache()` memoization identity: a second `Introspect`, a second `ListRoleGrants` walk, a second discovery probe per render. Outside a React server render `cache()` is a pass-through, so **no unit or integration test can observe a violation** — only the e2e `Introspect` count of Task 5.
- **vitest 5 resolves a Node-environment test's imports through `ssr.resolve.conditions`.** Set both that and the top-level `resolve.conditions`, each to the additive list `['node', 'import', 'default']`. A single-entry list breaks source-exports resolution for every `@paigasus/*` package.
- **`forbidden()` needs `experimental.authInterrupts`.** `next.config.ts` sets it; the vitest env needs `__NEXT_EXPERIMENTAL_AUTH_INTERRUPTS: 'true'` or the call throws E488.
- **Next strips the basePath** before app code sees a path. `redirect('/overview')` becomes `/gateway/overview`; `redirect('/gateway/overview')` becomes `/gateway/gateway/overview`. `proxy.ts`'s public paths are basePath-relative.
- **No file base name may be a Windows reserved device name** (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`), in any extension.
- **No file under `ts/apps/` may contain a Tailwind sentinel literal.** An app directory is Tailwind's scan root, and a second copy of a sentinel makes Tailwind generate the utility the guard asserts on, silently disarming it. Never write the literal, not even in a comment.
- **`ci/affected-graph/run.sh` compares expected task sets with strict equality.** A new project that becomes affected by an anchor and is absent from that case's expected list fails the case as "unexpected".
- **Never run `git add -A`.** `scratchpad-types-orig.ts` is untracked at the repository root and must stay untracked. Stage named paths only.
- **Local bash is split** and no single build runs every gate: `ci/affected-graph/run.sh` needs system `/bin/bash` 3.2; `ci/ruff/run.sh` and `ci/next-public/run.sh` need bash 4+ (`/opt/homebrew/bin/bash`); `ci/actionlint/run.sh` has **no working local bash** and its verdict must come from CI.
- **Moon needs the proto shims on `PATH`:** prefix a shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- **macOS has no `timeout`.** Never write `timeout N <cmd>`, and never chain a destructive command behind `&&` after a pipeline whose exit status you have not isolated.

---

## Corrections this plan makes to the spec

The spec is the binding authority; these are defects in it that the plan resolves, recorded rather than absorbed quietly (spec § 3.2).

**D14 — the spec's app tree declares two routes at the same path.** § 4 lists both `app/(public)/page.tsx` and `app/(console)/page.tsx`. Both resolve to `/gateway/`, and Next fails the build with a duplicate-route error. § 4 also lists `/` among `proxy.ts`'s public paths, which contradicts § 6's console overview owning it.

*Ruling:* mirror `iam-console`. `(public)/page.tsx` stays the public landing at `/gateway/` and redirects to `/overview` when the session cookie exists. The zone overview moves to **`(console)/overview/page.tsx`** → `/gateway/overview`. The scope route is unchanged at `(console)/orgs/[org]/page.tsx`. `proxy.ts`'s public paths are unchanged. `iam-console`'s cross-zone nav entry already points at `${gatewayBase}/`, which lands on the public page and redirects — no change there.

**D15 — the fake gateway does not exist.** § 10.1 lists it among the doubles in `@paigasus/console-core/testing`, but pull request 2 shipped only `fake-iam`, `fake-idp`, `tls` and `tls-terminator`. Task 4 adds `testing/fake-gateway.ts`.

**D16 — the presentation-copy table stays per app.** § 4's tree omits `app/_components/` entirely, yet both error boundaries, the 403 view and every deniable page need it. This plan does **not** hoist it into `@paigasus/console-core`: `iam-console`'s copy is IAM-worded throughout ("IAM refused this request", "This feature is not enabled on this IAM"), the two zones are separate products whose wording may legitimately differ, and the package's own boundary rule `paigasus/boundaries/console-core` bans React components from its `src/`. `gateway-console` gets its own `PRESENTATION_COPY` and its own three thin view files, whose hrefs are zone-local. This is per-zone content, not a duplicated logic block.

**D17 — the gateway's self nav entry is not gated on the zone map.** § 7.1 says both entries are present only when `PAIGASUS_ZONES` names that zone. This plan mirrors `iam-console/lib/nav.ts` instead: the app's own prefix is compiled in (`next.config.ts`'s `basePath`), `@paigasus/next-config/runtime` already fails closed at first request when the zone map disagrees, and only a genuinely cross-zone entry needs a runtime lookup. The **cross-zone `iam` entry is** gated on `zones['iam']`.

**D18 — the path-routing terminator belongs to pull request 4.** § 10.1 describes it among the doubles, but only the two-zone tier needs it. The single-zone tier fronts one server with the terminator exactly as shipped.

---

## What this pull request does NOT change

Measured against the tree at `f774ae6d`, so no task wastes a cycle on it:

- **`ts/eslint.config.js`** derives `appNames` from `readdirSync(appsDir)` (`:62-65`). A new app directory is picked up with no edit. Spec § 8.3's "a test asserts every `ts/apps/*` directory has a block" shipped in pull request 1.
- **`BOUNDARY_SCOPES`** (`ts/packages/paigasus-next-config/src/eslint.mjs:54-64`) keys on packages plus one generic `apps` entry. No new key.
- **`paigasus/boundaries/app-middleware`** globs `apps/**/{middleware,proxy}.*` (`:348`) and already covers the new `proxy.ts`.
- **`SOURCE_ONLY_PACKAGES`** (`ts/packages/paigasus-next-config/src/index.ts:16-26`) already lists `@paigasus/console-core` and every package this app compiles. Spec § 9's instruction was discharged by pull request 2.
- **`.moon/workspace.yml`** discovers projects by the glob `ts/apps/*`. No project registration.
- **`ts/pnpm-workspace.yaml`**, **`ts/moon.yml`'s `fileGroups.sources`** (already `apps/*/app/**/*`, `apps/*/lib/**/*`, `apps/*/proxy.ts`) and **`ci/next-public/run.sh`'s `APP_CONFIG_GLOB`** need no edit.
- **`repo:next-env-drift`'s `inputs`** are already `ts/apps/*` globs. Only its `deps` changes. Do **not** widen `ts/apps/*/next.config.ts` to other extensions: `repo:input-liveness` fails a glob matching zero tracked files.

## Known-red window

From Task 1, which adds `ts/apps/gateway-console/moon.yml`, until Task 6, the branch deliberately reds two repository gates:

- `repo:affected-smoke` — every strict-equality case the new project joins, and `TAILWIND_GUARD_INVOCATIONS`, which has no entry for the new app.
- `repo:next-env-drift` — no `deps` edge on the new app's build, so `next typegen` races its `.next`.

Verify a task with the **project-scoped** commands each task names, not with a full `moon ci`. Task 6 closes the window, and only after Task 6 is a full-graph run meaningful.

---

## File structure

```
ts/apps/gateway-console/
  moon.yml  package.json  next.config.ts  tsconfig.json  postcss.config.mjs
  vitest.config.ts  playwright.config.ts  next-env.d.ts  README.md
  .gitignore  .prettierignore  .env.local.example
  proxy.ts                            cookie presence + the correlation id
  lib/
    config.ts                         the one defineRuntimeConfig call
    auth.ts                           the auth runtime and the principal resolver
    console.ts                        the one createConsoleRuntime call
    nav.ts                            the zone navigation entries
  app/
    layout.tsx  globals.css  providers.tsx  error.tsx  global-error.tsx
    healthz/route.ts                  public; { zone, zones }
    auth/[...auth]/route.ts           the four auth routes
    (public)/layout.tsx  page.tsx     the signed-out landing at /gateway/
    (console)/layout.tsx  error.tsx  forbidden.tsx
    (console)/overview/page.tsx       the zone overview (spec § 6)   [D14]
    (console)/orgs/[org]/page.tsx     the scope route (spec § 7.2)
    _components/
      error-copy.ts  error-reference.tsx  error-tail.tsx  page-error.tsx
      org-switcher.tsx                the client shell that reads [org]
      zone-overview.tsx               the overview body, shared by both routes
      gateway-state.ts                the pure reader over ServiceState
  tests/
    tsconfig.json  standalone-runtime.test.ts
    support/  unit/  integration/  e2e/

ts/packages/paigasus-console-core/
  testing/fake-gateway.ts             new (D15)
  testing/index.ts                    re-exports it
```

---

### Task 1: The project skeleton, `lib/config.ts` and the public shell

The deliverable is a Next app that builds, serves `/gateway/` and `/gateway/healthz`, and whose Moon `build`, `typecheck` and `test` tasks are green — including the Tailwind guard, which needs a real production build with real CSS.

**Files:**
- Create: `ts/apps/gateway-console/moon.yml`
- Create: `ts/apps/gateway-console/package.json`
- Create: `ts/apps/gateway-console/next.config.ts`
- Create: `ts/apps/gateway-console/tsconfig.json`
- Create: `ts/apps/gateway-console/tests/tsconfig.json`
- Create: `ts/apps/gateway-console/postcss.config.mjs`
- Create: `ts/apps/gateway-console/vitest.config.ts`
- Create: `ts/apps/gateway-console/.prettierignore`
- Create: `ts/apps/gateway-console/.env.local.example`
- Create: `ts/apps/gateway-console/README.md`
- Create: `ts/apps/gateway-console/next-env.d.ts` (written by the first build, then tracked)
- Create: `ts/apps/gateway-console/lib/config.ts`
- Create: `ts/apps/gateway-console/app/globals.css`
- Create: `ts/apps/gateway-console/app/layout.tsx`
- Create: `ts/apps/gateway-console/app/providers.tsx`
- Create: `ts/apps/gateway-console/app/error.tsx`
- Create: `ts/apps/gateway-console/app/global-error.tsx`
- Create: `ts/apps/gateway-console/app/_components/error-copy.ts`
- Create: `ts/apps/gateway-console/app/(public)/layout.tsx`
- Create: `ts/apps/gateway-console/app/(public)/page.tsx`
- Create: `ts/apps/gateway-console/app/healthz/route.ts`
- Create: `ts/apps/gateway-console/tests/support/setup.ts`
- Create: `ts/apps/gateway-console/tests/support/env.ts`
- Copy verbatim (they are app-agnostic): `ts/apps/iam-console/tests/support/server-only-stub.ts` and `ts/apps/iam-console/tests/support/next-headers.ts` → `ts/apps/gateway-console/tests/support/`
- Test: `ts/apps/gateway-console/tests/unit/config.test.ts`
- Test: `ts/apps/gateway-console/tests/unit/error-copy.test.ts`

**Interfaces:**
- Produces: `getRuntimeConfig`, `getPublicConfig` and `type ConsoleConfig` from `lib/config.ts`; `Providers` from `app/providers.tsx`; `PRESENTATION_COPY` from `app/_components/error-copy.ts`. Tasks 2 and 3 consume all three.
- Consumes: `authEnvShape` from `@paigasus/auth/server`, `discoveryEnvShape` from `@paigasus/discovery/server`, `defineRuntimeConfig` from `@paigasus/next-config/runtime`, `createNextConfig` from `@paigasus/next-config`, `PublicShell`/`ZoneProvider`/`ZoneLink` from `@paigasus/app-shell`, `SESSION_COOKIE`/`SessionProvider` from `@paigasus/auth`, `LinkProvider`/`ErrorState` from `@paigasus/ui`.

- [ ] **Step 1: Confirm the branch**

The branch already exists — this plan was committed on it. Verify before writing anything:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git rev-parse --abbrev-ref HEAD   # must print: feature/sma-512-gateway-console
```

If it prints `main`, stop and check out the branch. Never implement on `main`.

- [ ] **Step 2: Write `package.json`**

`ts/apps/gateway-console/package.json`:

```json
{
  "name": "@paigasus/gateway-console",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "engines": {
    "node": ">=24"
  },
  "scripts": {
    "dev": "next dev",
    "build": "next build",
    "start": "next start",
    "typecheck": "tsc -p tsconfig.json --noEmit"
  },
  "dependencies": {
    "@paigasus/app-shell": "workspace:*",
    "@paigasus/auth": "workspace:*",
    "@paigasus/console-core": "workspace:*",
    "@paigasus/discovery": "workspace:*",
    "@paigasus/next-config": "workspace:*",
    "@paigasus/sdk": "workspace:*",
    "@paigasus/ui": "workspace:*",
    "@tailwindcss/postcss": "catalog:",
    "next": "catalog:",
    "react": "catalog:",
    "react-dom": "catalog:",
    "server-only": "catalog:",
    "tailwindcss": "catalog:",
    "zod": "catalog:"
  },
  "devDependencies": {
    "@types/node": "catalog:",
    "@types/react": "catalog:",
    "@types/react-dom": "catalog:",
    "typescript": "catalog:",
    "vitest": "catalog:"
  }
}
```

`@playwright/test` and `@connectrpc/connect` are deliberately absent: Task 5 adds the first when the e2e tier lands, and Task 4 adds the second only if the integration tier actually imports `Code`. **Before the final commit of every task, grep the app for each declared dependency and remove any nothing imports** — an unused dependency in a public repository is a review finding, and `pnpm` will not tell you.

- [ ] **Step 3: Write `next.config.ts`, `postcss.config.mjs`, `tsconfig.json` and `tests/tsconfig.json`**

`next.config.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { createNextConfig } from '@paigasus/next-config';

// The AI Gateway zone. `zone` and `basePath` describe the IMAGE, not the deployment: this image IS
// the gateway zone, and Next 16 has no runtime basePath, so the prefix is necessarily compiled in.
// Every deployment-varying value is read at request time instead (spec § 4, § 11).
//
// @paigasus/next-config/runtime cross-checks these two values against PAIGASUS_ZONE and
// PAIGASUS_ZONES at first request and fails closed if they disagree.
export default createNextConfig({
  zone: 'gateway',
  basePath: '/gateway',
  // ts/ — the pnpm workspace root. The inferred value decides where the standalone entry point
  // lands, so it is pinned rather than left to change under a lockfile move.
  outputFileTracingRoot: fileURLToPath(new URL('../..', import.meta.url)),
  // forbidden() and (console)/forbidden.tsx give the 403 view a real HTTP 403 (spec § 7.3).
  // EXPERIMENTAL in Next 16.3.4: the e2e tier asserts the 403 status, so an upgrade that changes
  // the flag fails CI instead of silently rendering the view with status 200.
  extend: { experimental: { authInterrupts: true } },
});
```

`postcss.config.mjs` and `tsconfig.json` are byte-identical to `ts/apps/iam-console/`'s. `tests/tsconfig.json` is byte-identical to `ts/apps/iam-console/tests/tsconfig.json` except that its `_comment` names this app. Copy all three.

- [ ] **Step 4: Write `vitest.config.ts`**

Copy `ts/apps/iam-console/vitest.config.ts` and make exactly these changes:

1. `PAIGASUS_COMPILED_ZONE: 'gateway'` and `PAIGASUS_COMPILED_BASE_PATH: '/gateway'`.
2. Drop the `next/cache` alias and the `nextCacheDouble` constant. This zone runs no Server Action, so nothing calls `revalidatePath`. Keep the `server-only` and `next/headers` aliases.
3. Keep `testTimeout` and `hookTimeout` at `120_000`: Task 5 adds `tests/standalone-runtime.test.ts`, which boots a real standalone server twice.
4. Keep the `oxc` block, the `conditions` list and **both** the `resolve.conditions` and `ssr.resolve.conditions` blocks, verbatim and with their comments.

- [ ] **Step 5: Write `lib/config.ts`**

Identical to `ts/apps/iam-console/lib/config.ts` except for the services refinement, which must demand **both** entries (spec § 4.1). Replace the `servicesWithIam` constant with:

```ts
/**
 * discovery's service map, plus two rules: this zone cannot run without an `iam` entry (it calls
 * IAM's gRPC port for every session) and it is pointless without a `gateway` entry (the overview
 * reports the gateway's own discovery state). Both are checked in ONE refine so a map missing both
 * reports both problems rather than only the first (spec § 4.1).
 */
const REQUIRED_SERVICES = ['iam', 'gateway'] as const;

const servicesWithIamAndGateway = discoveryEnvShape.PAIGASUS_SERVICES.superRefine((services, ctx) => {
  for (const name of REQUIRED_SERVICES) {
    if (!Object.hasOwn(services, name)) ctx.addIssue({ code: 'custom', message: `must contain a "${name}" entry` });
  }
});
```

and pass it as `PAIGASUS_SERVICES: servicesWithIamAndGateway`. Copy `grpcUrlProblem` and the `iamGrpcUrl` transform verbatim, comments included — **there is no gateway-specific key** (spec § 4.1: decision D11 removed `PAIGASUS_GATEWAY_URL` and `PAIGASUS_GATEWAY_DEFAULT_MODEL`; the gateway's address reaches this app through `PAIGASUS_SERVICES`).

- [ ] **Step 6: Write the failing config test**

`ts/apps/gateway-console/tests/unit/config.test.ts`. Use `ts/apps/iam-console/tests/unit/config.test.ts` as the shape, and cover at least:

```ts
it('rejects a PAIGASUS_SERVICES map with no gateway entry', () => {
  stubConsoleEnv({ PAIGASUS_SERVICES: '{"iam":"http://iam.internal:8080"}' });
  expect(() => getRuntimeConfig()).toThrow(/gateway/);
});

it('rejects a PAIGASUS_SERVICES map with no iam entry', () => {
  stubConsoleEnv({ PAIGASUS_SERVICES: '{"gateway":"http://gateway.internal:8080"}' });
  expect(() => getRuntimeConfig()).toThrow(/iam/);
});

it('reports BOTH missing entries, not only the first', () => {
  stubConsoleEnv({ PAIGASUS_SERVICES: '{"other":"http://other.internal:8080"}' });
  let message = '';
  try {
    getRuntimeConfig();
  } catch (error) {
    message = String(error);
  }
  expect(message).toMatch(/iam/);
  expect(message).toMatch(/gateway/);
});

it('accepts a map with both entries', () => {
  stubConsoleEnv();
  expect(getRuntimeConfig().PAIGASUS_SERVICES).toHaveProperty('gateway');
});
```

Also carry over `iam-console`'s `PAIGASUS_IAM_GRPC_URL` cases — an absolute-URL rejection, a non-`http(s)` scheme, credentials, a query, a fragment, and the trailing-slash canonicalization — because this app re-declares that key rather than importing it.

`tests/support/env.ts` is `iam-console`'s with `PAIGASUS_ZONE: 'gateway'`, `PAIGASUS_ZONES: '{"gateway":"/gateway"}'` and `PAIGASUS_SERVICES: '{"iam":"http://iam.internal:8080","gateway":"http://gateway.internal:8080"}'`.

`tests/support/setup.ts` is `iam-console`'s minus the `resetNextCache` import and call (Step 4 dropped that double).

- [ ] **Step 7: Run the config test and watch it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts install
pnpm --dir ts/apps/gateway-console exec vitest run tests/unit/config.test.ts
```

Expected: FAIL, because `lib/config.ts` does not exist yet if you wrote the test first, or PASS on the "accepts" case and FAIL on the two rejection cases if you wrote the single-rule version. Write the test before the refinement.

- [ ] **Step 8: Write the presentation copy**

`app/_components/error-copy.ts`. Start from `ts/apps/iam-console/app/_components/error-copy.ts` and keep **only** `PRESENTATION_COPY`, typed `Record<Presentation, { title: string; body: string }>` so a tenth presentation fails the type-check. Drop `FORM_REASON_COPY` and `formMessage`: this zone has no forms.

Re-word the rows whose IAM phrasing is wrong for this zone, and leave the rows whose errors genuinely come from IAM. The table must stay total over `Presentation`:

```ts
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
```

Head the file with a comment stating that this zone's authorization errors all come from IAM, which is why the IAM wording is correct here, and that `iam-console` keeps its own table (plan D16).

`tests/unit/error-copy.test.ts` asserts the table is total: iterate the SDK's `Presentation` union members through a literal list and assert each has a non-empty `title` and `body`. Do **not** write a test that only reads keys off the object — that passes with the table replaced by `{}` widened to `any`.

- [ ] **Step 9: Write the shell files**

`app/layout.tsx`, `app/providers.tsx`, `app/error.tsx`, `app/global-error.tsx`, `app/(public)/layout.tsx`, `app/(public)/page.tsx` and `app/healthz/route.ts` mirror `iam-console`'s, with these changes and no others:

| File | Change |
|---|---|
| `app/layout.tsx` | `metadata = { title: 'Paigasus AI Gateway' }` |
| `app/providers.tsx` | verbatim copy, comments included |
| `app/error.tsx` | the fallback anchor is `href="/gateway/"` |
| `app/global-error.tsx` | the anchor is `href="/gateway/"` |
| `app/(public)/layout.tsx` | verbatim copy |
| `app/(public)/page.tsx` | `redirect('/overview')` (basePath-relative — Next adds `/gateway` exactly once); `PublicShell brand={{ label: 'Paigasus AI Gateway', href: '/gateway' }}`; the heading and the one-line description name the gateway zone; `data-testid="public-home"` stays |
| `app/healthz/route.ts` | verbatim copy |

`app/globals.css` is `iam-console`'s verbatim, **including both `@source` lines**. Both are load-bearing: Tailwind's scan root is this app's own directory, so without them `@paigasus/ui`'s and `@paigasus/app-shell`'s classes are silently absent from a production build. Keep the comments and **do not write any sentinel literal**.

- [ ] **Step 10: Write `moon.yml`**

Copy `ts/apps/iam-console/moon.yml` and make exactly these changes:

1. `id: 'gateway-console-ts'`.
2. `dependsOn`: keep all eight entries unchanged.
3. `fileGroups.sources`: keep `app/**/*`, `lib/**/*`, `proxy.ts` unchanged.
4. `build`'s `script`: the asserted path becomes `.next/standalone/apps/gateway-console/server.js` and the failure message names `gateway-console`.
5. `test`'s `script`: the third guard line becomes `node ../../../ci/tailwind-source/run.mjs --app ts/apps/gateway-console`. The `--self-test` and `--negative-control` lines are unchanged — `ci/affected-graph/ci_targets.py`'s `_expected_tailwind_lines` derives all three from the app name, so they must match character for character.
6. Delete the whole `test-e2e:` task. Task 5 adds it.
7. In `typecheck`'s `inputs`, delete the `playwright.config.ts` line and the three `tests/fixtures` lines. Task 5 restores the first; this app has no fixtures, so the other three would match nothing.
8. In `test`'s `inputs`, delete the two `!tests/fixtures/...` negations, for the same reason.
9. Rewrite every `SMA-511`/`SMA-502`/`SMA-503` provenance comment that describes a *past* fix in `iam-console` so it states the *rule* for this app instead. Do not copy a comment that claims a measurement this app never made.

Every `/ts/packages/...` input line stays, verbatim: this app compiles the same eight packages.

- [ ] **Step 11: Build, then commit the generated `next-env.d.ts`**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run gateway-console-ts:build
```

Expected: PASS, and `ts/apps/gateway-console/next-env.d.ts` now exists. It is generated but **tracked** — `repo:next-env-drift` regenerates it and diffs, so a missing or stale copy reds that gate. Add it. Write `.prettierignore` as `iam-console`'s (`.next`, `node_modules`, `next-env.d.ts`).

- [ ] **Step 12: Run the app's own tasks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run gateway-console-ts:typecheck
moon run gateway-console-ts:test
```

Expected: both PASS. `test` runs vitest and then all three Tailwind guard modes; the third asserts the three sentinels reach this app's built CSS. If the real run fails on a missing sentinel, the cause is a dropped `@source` line in `globals.css`, not the guard.

Do **not** run `moon ci` here. `repo:affected-smoke` and `repo:next-env-drift` are red by construction until Task 6 (see "Known-red window").

- [ ] **Step 13: Write `README.md` and `.env.local.example`**

`README.md`: what the zone is, its base path, the environment groups from spec § 11, that `PAIGASUS_SESSION_STORE=redis` is mandatory in a multi-zone deployment (`@paigasus/auth`'s `createAuthRuntime` throws on `memory` when `PAIGASUS_ZONES` names more than one zone), and that `PAIGASUS_SERVICES` must carry both `iam` and `gateway`.

`.env.local.example`: `iam-console`'s with `PAIGASUS_ZONE=gateway`, the same `PAIGASUS_ZONES` JSON, and a `PAIGASUS_SERVICES` line carrying both entries.

- [ ] **Step 14: Commit**

```bash
git add ts/apps/gateway-console
git commit -m "feat(ts): scaffold the gateway-console app and its public shell (SMA-512)"
```

---

### Task 2: The composition root, the navigation and the proxy

**Files:**
- Create: `ts/apps/gateway-console/lib/auth.ts`
- Create: `ts/apps/gateway-console/lib/console.ts`
- Create: `ts/apps/gateway-console/lib/nav.ts`
- Create: `ts/apps/gateway-console/proxy.ts`
- Create: `ts/apps/gateway-console/app/auth/[...auth]/route.ts`
- Test: `ts/apps/gateway-console/tests/unit/nav.test.ts`
- Test: `ts/apps/gateway-console/tests/unit/proxy.test.ts`
- Test: `ts/apps/gateway-console/tests/unit/lib-console-composition.test.ts`
- Test: `ts/apps/gateway-console/tests/unit/session-cookie.test.ts`

**Interfaces:**
- Consumes: `getRuntimeConfig` from `./config` (Task 1).
- Produces: `authRuntime` from `lib/auth.ts`; the ten accessors `currentSession`, `optionalSession`, `sessionToken`, `iamClients`, `iamClientsForAction`, `iamClientsForToken`, `currentPrincipal`, `mayI`, `myScopes` and `discovery` from `lib/console.ts`; `GATEWAY_BASE_PATH` and `buildNavEntries` from `lib/nav.ts`. Task 3 consumes all of them.

- [ ] **Step 1: Write `lib/console.ts` and `lib/auth.ts`**

Both are **verbatim copies** of `ts/apps/iam-console/lib/console.ts` and `ts/apps/iam-console/lib/auth.ts`, comments included. Nothing in either file names a zone. Two properties of those files are load-bearing and must survive the copy unchanged:

1. `lib/console.ts` passes `authRuntime: () => authRuntime()` — a **fresh arrow**, not the imported binding. The two modules import from each other, and Next gives a route handler and a page separate module graphs, so both evaluation orders occur in production. The arrow resolves the identifier at call time.
2. `lib/auth.ts` builds `clientsForToken` as `async (token) => iamClientsForToken(token, await requestCorrelationId())`, so the login callback's `GetServiceInfo` and `Introspect` carry the request's correlation id.

There is exactly ONE `createConsoleRuntime(` call in this app, at module scope in `lib/console.ts`. This is a correctness rule (Global Constraints); no test in this task can catch a violation, and Task 5's `Introspect` count is the only control that can.

- [ ] **Step 2: Write the failing composition test**

`tests/unit/lib-console-composition.test.ts`. Use `ts/apps/iam-console/tests/unit/lib-console-composition.test.ts` as the shape. It reads `lib/console.ts` as TEXT and asserts:

```ts
const source = readFileSync(new URL('../../lib/console.ts', import.meta.url), 'utf8');

it('calls createConsoleRuntime exactly once', () => {
  expect(source.match(/createConsoleRuntime\s*\(/g) ?? []).toHaveLength(1);
});

it('calls it at module scope, not inside a function', () => {
  expect(source).toMatch(/^export const \{[^}]*\} = createConsoleRuntime\(/m);
});

it('re-exports every accessor the app uses', () => {
  for (const name of ['currentSession', 'optionalSession', 'sessionToken', 'iamClients', 'iamClientsForAction', 'iamClientsForToken', 'currentPrincipal', 'mayI', 'myScopes', 'discovery']) {
    expect(source).toContain(name);
  }
});

it('passes the auth runtime as a fresh arrow, not the imported binding', () => {
  expect(source).toMatch(/authRuntime:\s*\(\)\s*=>\s*authRuntime\(\)/);
});
```

Run it and watch it fail before writing the file, or — if you wrote `lib/console.ts` first — delete the arrow wrapper, watch the fourth case fail, and restore it. **A test that has not been observed failing has not been tested.**

**Not covered here, deliberately.** Spec § 10.2 also asks that the logger write only the port's fields and never a DSN. That is `@paigasus/console-core`'s logger, and `paigasus-console-core-ts:test`'s `tests/unit/logger.test.ts` already asserts it. Do not duplicate that suite in this app: the app imports the logger, it does not own it.

- [ ] **Step 3: Write `lib/nav.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The ONE function that builds PrimaryNav's entries. Every entry's `state` comes from
// @paigasus/app-shell's navStateOf(): no entry here decides its own visibility from a capability
// list.
//
// Hrefs are FULL paths, as the ingress sees them (SMA-510 spec § 6.2).
import 'server-only';
import { navStateOf, type NavEntry, type ZoneMap } from '@paigasus/app-shell';
import type { ServiceState } from '@paigasus/discovery/types';

/**
 * This image IS the gateway zone (next.config.ts's `basePath: '/gateway'`), so this app's own
 * prefix is compiled in, not read from the zone map: it can never be anything else, and
 * @paigasus/next-config/runtime already fails closed at first request if the deployment's zone map
 * disagrees. Only a genuinely cross-zone base path (`iam`, below) needs a runtime lookup. This
 * mirrors iam-console/lib/nav.ts and departs from spec § 7.1's literal wording (plan D17).
 */
export const GATEWAY_BASE_PATH = '/gateway';

export function buildNavEntries(input: { iam: ServiceState; gateway: ServiceState; zones: ZoneMap }): NavEntry[] {
  const entries: NavEntry[] = [{ zone: 'gateway', href: `${GATEWAY_BASE_PATH}/overview`, label: 'Overview', state: navStateOf(input.gateway) }];
  // A cross-zone entry. PrimaryNav drops it when `iam` is not a zone (its rule 1), and navStateOf
  // answers `absent` when `iam` is not a configured service. The mirror image of iam-console's own
  // gateway entry, so the link appears in both directions as soon as the zone map names both.
  const iamBase = input.zones['iam'];
  if (iamBase !== undefined) {
    entries.push({ zone: 'iam', href: `${iamBase}/orgs`, label: 'IAM', state: navStateOf(input.iam) });
  }
  return entries;
}
```

- [ ] **Step 4: Write the failing navigation test**

`tests/unit/nav.test.ts` must cover **every zone-map and service-state combination** (spec § 10.2). Build the states with small helpers so a case reads as data:

```ts
const available = (service: string, capabilities: string[] = []): ServiceState => ({ state: 'available', service, descriptor: { service, version: '1.0.0', capabilities }, capabilities });
const degraded = (service: string): ServiceState => ({ state: 'degraded', service, reason: 'timeout', descriptor: null, capabilities: [] });
const absent = (service: string): ServiceState => ({ state: 'absent', service });
```

Assert at least:
- the Overview entry is always present, whatever the zone map says, and its `state` tracks `navStateOf(gateway)` across all three service states;
- the IAM entry is present when `zones` has an `iam` key and absent when it does not — including when the IAM **service** is available but the IAM **zone** is not deployed;
- the IAM entry's `href` is `${zones['iam']}/orgs`, so a deployment that mounts IAM at a different prefix gets the right link;
- the Overview entry's `href` is `/gateway/overview` and does **not** depend on `zones['gateway']`;
- the order is Overview first, IAM second.

Run it, watch the failures, then implement.

- [ ] **Step 5: Write `proxy.ts`**

A **verbatim copy** of `ts/apps/iam-console/proxy.ts`, with only the header comment's zone name changed and the matcher comment restated for this app. Keep all of:

- the two inlined header constants `CORRELATION_HEADER = 'paigasus-correlation-id'` and `REQUEST_PATH_HEADER = 'x-paigasus-request-path'`, **inlined and not imported** from `@paigasus/console-core`: the `paigasus/boundaries/app-middleware` rule bans that package here, because `server-only` is a no-op in the middleware layer. The values must match `ts/packages/paigasus-console-core/src/correlation-header.ts`;
- `publicPaths: [...authRoutePaths(), '/', '/healthz']` and `loginPath: '/auth/login'`, both basePath-relative;
- the `decision.headers.has('location')` early return;
- the carry-over loop for response headers and the separate cookie pass;
- `config.matcher = ['/((?!_next/static|_next/image|favicon.ico).*)']`, written **without** the `/gateway` prefix.

- [ ] **Step 6: Write the failing proxy test**

`tests/unit/proxy.test.ts`, modelled on `ts/apps/iam-console/tests/unit/proxy.test.ts` and `proxy-carry-headers.test.ts`. Construct requests with the basePath, or the assertions are meaningless:

```ts
const req = new NextRequest('https://console.example.test/gateway/overview', { nextConfig: { basePath: '/gateway' } });
```

Cover:
- no cookie on `/gateway/overview` → a redirect whose `location` ends in `/gateway/auth/login`;
- no cookie on `/gateway/`, `/gateway/healthz` and each of `authRoutePaths()` → no redirect;
- a request that proceeds carries both headers, and `CORRELATION_HEADER` is a **fresh** value: send the request with `paigasus-correlation-id: attacker-supplied` and assert the outgoing value differs;
- `REQUEST_PATH_HEADER` equals `/gateway/overview` — the full path including the basePath;
- the two header names equal the exported constants in `ts/packages/paigasus-console-core/src/correlation-header.ts`, read as **text** (that module imports `server-only`). This is the only thing stopping the inlined copies drifting.

- [ ] **Step 7: Write the session-cookie test**

`tests/unit/session-cookie.test.ts`: read every file under `lib/` and `proxy.ts` as text and assert **zero** occurrences of a `__Host-` literal and zero of `SESSION_COOKIE_NAME`. `@paigasus/auth` exports `SESSION_COOKIE` from `./server`; a second copy of that constant must agree across every zone or the shared session silently stops working (spec § 5.4).

- [ ] **Step 8: Write the auth route**

`app/auth/[...auth]/route.ts` is a **verbatim copy** of `iam-console`'s, with the path names in its header comment changed to `/gateway/auth/*`. Keep the `WeakMap<AuthRuntime, …>` handler cache and the async `handle`: nothing may build the runtime at module scope, where `next build` would evaluate it.

- [ ] **Step 9: Run the tests and the app's tasks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts/apps/gateway-console exec vitest run
moon run gateway-console-ts:typecheck
moon run gateway-console-ts:build
```

Expected: all PASS.

- [ ] **Step 10: Commit**

```bash
git add ts/apps/gateway-console
git commit -m "feat(ts): gateway-console composition root, navigation and proxy (SMA-512)"
```

---

### Task 3: The console group — the shell, the zone overview and the scope route

**Files:**
- Create: `ts/apps/gateway-console/app/_components/gateway-state.ts`
- Create: `ts/apps/gateway-console/app/_components/zone-overview.tsx`
- Create: `ts/apps/gateway-console/app/_components/error-reference.tsx`
- Create: `ts/apps/gateway-console/app/_components/error-tail.tsx`
- Create: `ts/apps/gateway-console/app/_components/page-error.tsx`
- Create: `ts/apps/gateway-console/app/_components/org-switcher.tsx`
- Create: `ts/apps/gateway-console/app/(console)/layout.tsx`
- Create: `ts/apps/gateway-console/app/(console)/error.tsx`
- Create: `ts/apps/gateway-console/app/(console)/forbidden.tsx`
- Create: `ts/apps/gateway-console/app/(console)/overview/page.tsx`
- Create: `ts/apps/gateway-console/app/(console)/orgs/[org]/page.tsx`
- Test: `ts/apps/gateway-console/tests/unit/gateway-state.test.ts`
- Test: `ts/apps/gateway-console/tests/unit/zone-overview.test.tsx`
- Test: `ts/apps/gateway-console/tests/unit/error-views.test.tsx`
- Test: `ts/apps/gateway-console/tests/unit/org-switcher.test.tsx`

**Interfaces:**
- Consumes: `currentSession`, `discovery`, `myScopes`, `iamClients` from `../../lib/console`; `buildNavEntries`, `GATEWAY_BASE_PATH` from `../../lib/nav`; `getPublicConfig` from `../../lib/config`; `PRESENTATION_COPY` from `../_components/error-copy` (Task 1); `switcherOrgs`, `isUuid`, `organizationPrn`, `callIam`, `FORBIDDEN_VIEW_CORRELATION`, `requestCorrelationId`, `requestPath` from `@paigasus/console-core`.
- Produces: `gatewayView` and `STREAM_CAPABILITY` from `_components/gateway-state.ts`, and `ZoneOverview` from `_components/zone-overview.tsx`. Task 4's integration tier and Task 5's e2e specs both assert on the test ids this task defines.

- [ ] **Step 1: Write the failing gateway-state test**

`tests/unit/gateway-state.test.ts`. The reader is pure, so it carries the whole capability rule and every branch is testable here. Assert:

```ts
it('reports streaming when the gateway is available and lists the capability', () => {
  expect(gatewayView(available('gateway', ['gateway.chat.stream'])).streaming).toBe(true);
});

it('does NOT report streaming from a DEGRADED service, even when its last descriptor listed it', () => {
  const state: ServiceState = { state: 'degraded', service: 'gateway', reason: 'timeout', descriptor: { service: 'gateway', version: '1.2.3', capabilities: ['gateway.chat.stream'] }, capabilities: ['gateway.chat.stream'] };
  expect(gatewayView(state).streaming).toBe(false);
  expect(gatewayView(state).version).toBe('1.2.3');
  expect(gatewayView(state).reason).toBe('timeout');
});

it('reports nothing at all for an absent service', () => {
  expect(gatewayView(absent('gateway'))).toEqual({ state: 'absent', reason: null, version: null, capabilities: [], streaming: false });
});
```

The degraded case is the one that matters: a stale descriptor must not read as a live capability, which is the rule `@paigasus/console-core`'s `cedarCapabilityOf` already applies to `iam.authz.cedar`. Run the file and watch all three fail.

- [ ] **Step 2: Write `_components/gateway-state.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The pure reader over the gateway's ServiceState (spec § 6). It holds the ONE capability rule this
// zone has: `gateway.chat.stream` counts only when the service is AVAILABLE. A degraded service may
// still carry the descriptor of its last good probe, and treating that as a live capability would
// ship a control that reports a feature the gateway cannot currently serve. This is the same rule
// @paigasus/console-core's cedarCapabilityOf applies to iam.authz.cedar.
//
// No 'server-only': this is a pure function over plain data and the view renders it on the server.
import type { ServiceState } from '@paigasus/discovery/types';

/** The gateway's only capability (rs/crates/services/paigasus-gateway/src/service_info.rs:19-33). */
export const STREAM_CAPABILITY = 'gateway.chat.stream';

export type GatewayView = {
  readonly state: ServiceState['state'];
  /** Why it is degraded, or null in every other state. */
  readonly reason: string | null;
  readonly version: string | null;
  readonly capabilities: readonly string[];
  readonly streaming: boolean;
};

export function gatewayView(state: ServiceState): GatewayView {
  if (state.state === 'absent') return { state: 'absent', reason: null, version: null, capabilities: [], streaming: false };
  return {
    state: state.state,
    reason: state.state === 'degraded' ? state.reason : null,
    version: state.descriptor?.version ?? null,
    capabilities: state.capabilities,
    streaming: state.state === 'available' && state.capabilities.includes(STREAM_CAPABILITY),
  };
}
```

- [ ] **Step 3: Write `_components/zone-overview.tsx`**

A server component. It renders what the zone can honestly report today and states plainly that the playground is not here yet (spec § 6, § 12 item 1). Give every assertable fact a `data-testid`, because Tasks 4 and 5 assert on them:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The zone overview (spec § 6). It is not a placeholder: it is the first exercise of ADR-0020
// discovery from a SECOND zone, and it is the control the follow-up issue's chat playground will
// gate on — the gateway answers a streaming request with a 400 carrying `param: "stream"` when the
// capability is off, so a console that could not see the capability would ship a control that
// always fails.
import type { ReactElement } from 'react';
import { EmptyState } from '@paigasus/ui';
import { STREAM_CAPABILITY, type GatewayView } from './gateway-state';

const STATE_COPY: Record<GatewayView['state'], { title: string; body: string }> = {
  available: { title: 'The gateway is available', body: 'It answered the capability probe.' },
  degraded: { title: 'The gateway is not available', body: 'It did not answer the capability probe. Try again in a moment.' },
  absent: { title: 'The gateway is not configured', body: 'No gateway entry is present in this deployment’s service map.' },
};

export function ZoneOverview({ view, scope }: { readonly view: GatewayView; readonly scope: { readonly orgId: string; readonly name: string } | null }): ReactElement {
  const copy = STATE_COPY[view.state];
  return (
    <section className="flex flex-col gap-6 p-8" data-testid="zone-overview">
      <div>
        <h1 className="text-2xl font-semibold">AI Gateway</h1>
        <p className="text-muted-foreground text-sm" data-testid="gateway-scope">
          {scope === null ? 'All organizations' : scope.name}
        </p>
      </div>

      <div data-testid="gateway-state" data-state={view.state}>
        <h2 className="text-lg font-semibold">{copy.title}</h2>
        <p className="text-muted-foreground text-sm">{copy.body}</p>
        {view.reason === null ? null : (
          <p className="text-muted-foreground mt-1 text-xs" data-testid="gateway-reason">
            Reason: <code>{view.reason}</code>
          </p>
        )}
      </div>

      {view.version === null ? null : (
        <p className="text-sm" data-testid="gateway-version">
          Version: <code>{view.version}</code>
        </p>
      )}

      <div>
        <h2 className="text-lg font-semibold">Capabilities</h2>
        {view.capabilities.length === 0 ? (
          <EmptyState title="No capabilities reported" />
        ) : (
          <ul className="text-sm" data-testid="gateway-capabilities">
            {view.capabilities.map((capability) => (
              <li key={capability}>
                <code>{capability}</code>
              </li>
            ))}
          </ul>
        )}
        <p className="text-muted-foreground mt-2 text-sm" data-testid="gateway-streaming" data-streaming={String(view.streaming)}>
          {view.streaming ? `Streaming chat (${STREAM_CAPABILITY}) is available.` : `Streaming chat (${STREAM_CAPABILITY}) is not available.`}
        </p>
      </div>

      <p className="text-muted-foreground text-sm" data-testid="playground-note">
        The chat playground is not part of this zone yet. It needs the gateway to authenticate an interactive user, which is tracked separately.
      </p>
    </section>
  );
}
```

- [ ] **Step 4: Write the failing overview-view test**

`tests/unit/zone-overview.test.tsx`, rendering with `renderToStaticMarkup` from `react-dom/server` (the pattern `iam-console`'s `error-views.test.tsx` uses). Cover the full matrix spec § 10.2 asks for: each of the three discovery states, and the capability present and absent. Assert on the rendered markup, not on the props you passed:

- `available` with the capability → `data-state="available"`, `data-streaming="true"`, the capability key appears, the version appears;
- `available` without it → `data-streaming="false"` and the "is not available" sentence;
- `degraded` → `data-state="degraded"` and a visible `data-testid="gateway-reason"`;
- `absent` → `data-state="absent"`, no version element, no capability list;
- `scope === null` renders "All organizations"; a scope renders the organization's name.

- [ ] **Step 5: Write the three error views and the switcher shell**

| File | Source | Change |
|---|---|---|
| `_components/error-reference.tsx` | `iam-console`'s | `SignInAgain`'s href becomes `/gateway/auth/login` (and the `returnTo` form of it) |
| `_components/error-tail.tsx` | `iam-console`'s | verbatim; it imports `requestPath` from `@paigasus/console-core` and `./error-reference` |
| `_components/page-error.tsx` | `iam-console`'s | verbatim except it imports `./error-copy` and `./error-tail` from this app |
| `_components/org-switcher.tsx` | `iam-console`'s | verbatim — it derives its hrefs from `useZone().basePath`, so it is already zone-agnostic |

Keep `page-error.tsx`'s two load-bearing properties: `forbidden()` and `notFound()` are called **first, before any await**, so they throw before the page has rendered anything; and **do not add a `loading.tsx` under `(console)`** — its Suspense boundary would let Next commit a 200 before the throw and the 403 status would be lost.

`tests/unit/error-views.test.tsx` asserts, with `renderToStaticMarkup`: a `forbidden` presentation calls `forbidden()` (mock `next/navigation`), a `not-found` one calls `notFound()`, a `disabled` one renders the EmptyState copy, a `relogin` one renders the "Sign in again" link pointing at `/gateway/auth/login`, and every other presentation renders the correlation id. `tests/unit/org-switcher.test.tsx` mirrors `iam-console`'s: `orgSwitcherItems` builds `${basePath}/orgs/${orgId}` hrefs, and `currentOrgId` lower-cases an upper-case `[org]` segment and answers null off an organization page.

- [ ] **Step 6: Write `(console)/layout.tsx`**

Mirrors `iam-console`'s with one difference: this zone has no audit entry, so it asks no `mayI()` and makes **three** request-scoped reads, not five.

```tsx
export default async function ConsoleLayout({ children }: { children: ReactNode }): Promise<ReactElement> {
  // No prerender, as in app/(public)/layout.tsx: the runtime config does not exist during `next build`.
  await connection();
  const session = await currentSession();
  const { zone, zones } = getPublicConfig();
  const probe = discovery();
  const [iam, gateway, scopes] = await Promise.all([probe.getServiceState('iam', session.accessToken), probe.getServiceState('gateway', session.accessToken), myScopes()]);
  const nav = buildNavEntries({ iam, gateway, zones });
  return (
    <Providers zone={zone} zones={zones} session={toSessionView(session)}>
      <OrgSwitcherShell brand={{ label: 'Paigasus AI Gateway', href: `${GATEWAY_BASE_PATH}/overview` }} nav={nav} orgs={switcherOrgs(scopes)}>
        {children}
      </OrgSwitcherShell>
    </Providers>
  );
}
```

`SessionProvider` receives `toSessionView(session)` only — never a token (ADR-0017). Carry `iam-console`'s header comment explaining that this layout does not guard Server Actions and that `myScopes()` is a React `cache()`, so the layout and a page share one call.

**Cost, recorded deliberately (spec § 13):** `myScopes()` runs on every render in this zone — one `Introspect`, one `ListRoleGrants` walk and up to 50 tenancy reads. This design pays that twice across the two zones, on decision D7, because the gateway gets organization and project settings later.

- [ ] **Step 7: Write `(console)/error.tsx` and `(console)/forbidden.tsx`**

`error.tsx` is `iam-console`'s verbatim (it shows Next's digest, never the error message).

`forbidden.tsx` is `iam-console`'s with the back link changed to `/gateway/overview` and its label to "Back to the gateway overview". Keep the `FORBIDDEN_VIEW_CORRELATION === 'header'` guard and `data-testid="forbidden-view"`: Task 5's e2e row asserts the real HTTP 403 and the correlation id, and it reads that constant from `@paigasus/console-core`'s source as text.

- [ ] **Step 8: Write `(console)/overview/page.tsx`**

```tsx
export default async function OverviewPage(): Promise<ReactElement> {
  const session = await currentSession();
  const state = await discovery().getServiceState('gateway', session.accessToken);
  return <ZoneOverview view={gatewayView(state)} scope={null} />;
}
```

`currentSession()` and `discovery()` are both React `cache()` wrappers built by the one `createConsoleRuntime` call, and `Discovery` memoizes `getServiceState` per handle — so this page and the layout above it share **one** session resolution and **one** gateway probe per request. That sharing is exactly what a second `createConsoleRuntime()` call would break, invisibly (Global Constraints).

This route is `/gateway/overview`, **not** `/gateway/`: `(public)/page.tsx` already owns the latter, and two `page.tsx` at one path fail the Next build (plan D14).

- [ ] **Step 9: Write `(console)/orgs/[org]/page.tsx`**

```tsx
export default async function ScopePage({ params }: { params: Promise<{ org: string }> }): Promise<ReactElement> {
  const { org } = await params;
  if (!isUuid(org)) notFound();
  const orgId = org.toLowerCase();
  const [session, clients] = await Promise.all([currentSession(), iamClients()]);
  // The organization read comes FIRST: when IAM denies it, the page is the 403 view and nothing
  // else runs. The PRN comes from the URL, so IAM's invalid-input answer means the URL names no
  // such node — notFound(), not an error view.
  const got = await callIam(() => clients.tenancy.getOrganization({ prn: organizationPrn(orgId) }));
  if (!got.ok) return got.error.presentation === 'invalid-input' ? notFound() : <PageError error={got.error} />;
  const organization = got.value.organization;
  if (organization === undefined) notFound();
  const state = await discovery().getServiceState('gateway', session.accessToken);
  return (
    <div className="flex flex-col gap-8 p-6">
      <Breadcrumbs items={[{ label: 'Overview', href: `${GATEWAY_BASE_PATH}/overview` }, { label: organization.name }]} />
      <ZoneOverview view={gatewayView(state)} scope={{ orgId, name: organization.name }} />
    </div>
  );
}
```

This route is why `forbidden.tsx` is reachable in this zone (spec § 7.3): a denied `GetOrganization` produces the `forbidden` presentation, `PageError` calls `forbidden()`, and Next renders the 403 boundary inside the console layout.

**Recorded limit (spec § 13):** the scope changes the URL and the breadcrumbs and nothing else. The route exists now so that the later settings screens hang off a shape that already works, and so the organization switcher is not a dead control.

- [ ] **Step 10: Run the tests and build**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts/apps/gateway-console exec vitest run
moon run gateway-console-ts:typecheck
moon run gateway-console-ts:build
moon run gateway-console-ts:test
```

Expected: all PASS. If `next build` reports a duplicate route, a `page.tsx` landed under `(console)` at the group root — see plan D14.

- [ ] **Step 11: Commit**

```bash
git add ts/apps/gateway-console
git commit -m "feat(ts): gateway-console shell, zone overview and scope route (SMA-512)"
```

---

### Task 4: The fake gateway and the integration tier

Spec § 10.1 lists a fake gateway among the shared doubles, but pull request 2 shipped only four (`fake-iam`, `fake-idp`, `tls`, `tls-terminator`). This task adds the fifth (plan D15) and builds the app's integration tier on it.

**Files:**
- Create: `ts/packages/paigasus-console-core/testing/fake-gateway.ts`
- Modify: `ts/packages/paigasus-console-core/testing/index.ts` (re-export it)
- Create: `ts/apps/gateway-console/tests/integration/support.ts`
- Create: `ts/apps/gateway-console/tests/integration/doubles/fake-gateway.test.ts`
- Create: `ts/apps/gateway-console/tests/integration/overview-page.test.ts`
- Create: `ts/apps/gateway-console/tests/integration/scope-page.test.ts`
- Create: `ts/apps/gateway-console/tests/integration/provisioning.test.ts`
- Modify: `ts/apps/gateway-console/package.json` (add `@connectrpc/connect` if and only if a test imports `Code`)

**Interfaces:**
- Produces: `startFakeGateway`, `type FakeGateway`, `type FakeGatewayCall` from `@paigasus/console-core/testing`. Task 5's e2e harness consumes them.
- Consumes: `startFakeIam`, `denial`, `type FakeIamHandlers` from the same subpath; `gatewayView` and the `data-testid`s from Task 3.

- [ ] **Step 1: Write the failing fake-gateway test**

`ts/apps/gateway-console/tests/integration/doubles/fake-gateway.test.ts`, modelled on `ts/apps/iam-console/tests/integration/doubles/fake-iam.test.ts`. Assert:

- `GET /v1/service-info` with a bearer token answers 200 and the current descriptor;
- any other method or path answers 404;
- a request with no `Authorization` header answers 401;
- `setServiceInfo({ status: 503 })` makes the next probe answer 503;
- `setReachable(false)` makes the next request fail at the transport, not with an HTTP status — assert the `fetch` **rejects**;
- `setReachable(true)` restores it **on the same port**, so a caller holding the URL works again;
- every request is recorded in `calls`, with its bearer token and its correlation-id header.

The reachability case is the one that is easy to get wrong: closing the server frees the port and the restore then binds a different one, which silently invalidates any URL a test already handed to the app. Run the file and watch every case fail.

- [ ] **Step 2: Write `testing/fake-gateway.ts`**

HTTP only. It serves **no** chat route: decision D11 removed the only consumer, and a fake endpoint with no product behind it is the vacuous-green shape spec § 2.3 refuses.

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The fake gateway (spec § 10.1). It serves `GET /v1/service-info` and NOTHING else — in
// particular no chat route, because decision D11 removed the only consumer and a fake endpoint
// with no product behind it would let a whole tier pass while the product could not make one call.
//
// Reachability is toggled by DESTROYING inbound sockets, not by closing the server: closing frees
// the port, and a later restore would bind a different one, silently invalidating the URL a test
// already handed to the app under test.
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';

export const GATEWAY_CORRELATION_HEADER = 'paigasus-correlation-id';

export type FakeGatewayCall = { readonly method: string; readonly path: string; readonly token: string | null; readonly correlationId: string | null };
export type GatewayDescriptorBody = { service: string; version: string; capabilities: string[] };

export type FakeGateway = {
  /** `http://127.0.0.1:<port>` — the value that belongs in PAIGASUS_SERVICES under `gateway`. */
  readonly url: string;
  readonly calls: readonly FakeGatewayCall[];
  /** What `GET /v1/service-info` answers: a descriptor, or an HTTP status to fail with. */
  setServiceInfo(next: GatewayDescriptorBody | { status: number }): void;
  /** false: inbound connections are destroyed, so a probe fails at the transport. */
  setReachable(reachable: boolean): void;
  close(): Promise<void>;
};

const DEFAULT_DESCRIPTOR: GatewayDescriptorBody = { service: 'gateway', version: '0.0.0-fake', capabilities: ['gateway.chat.stream'] };
```

Write `startFakeGateway(opts: { descriptor?: GatewayDescriptorBody } = {}): Promise<FakeGateway>` to follow `fake-iam.ts`'s HTTP half exactly: a `bearerOf` read of `authorization`, a `calls.push` before any branch so a 401 is still recorded, a `json(status, body)` helper, the 404 for anything but `GET /v1/service-info`, and a `listen(0, '127.0.0.1')` that rejects when `address()` is null or a string. Attach the reachability switch as a `connection` listener that calls `socket.destroy()` while `reachable` is false, and have `close()` destroy any held socket before `server.close()`.

Re-export from `testing/index.ts` alongside the other four, keeping the file's alphabetical grouping and its header comment intact.

- [ ] **Step 3: Write `tests/integration/support.ts`**

Model it on `ts/apps/iam-console/tests/integration/support.ts`. It must start a fake IAM **and** a fake gateway, stub the environment so `lib/config.ts` parses (`PAIGASUS_SERVICES` carrying both `iam` and the fake gateway's URL, `PAIGASUS_IAM_GRPC_URL` the fake IAM's gRPC URL, and the three `PAIGASUS_DISCOVERY_*_MS` values at 1/2/3 ms so every render re-probes), install a session in the `next/headers` double, and reset `@paigasus/console-core`'s discovery cache between tests with `resetDiscoveryForTest`.

- [ ] **Step 4: Write the failing overview-page test**

`tests/integration/overview-page.test.ts` drives `(console)/overview/page.tsx` against the real `discovery()` and the scripted fake gateway (spec § 10.3). Render the returned element with `renderToStaticMarkup` and assert on the markup:

| Fake gateway | Expected |
|---|---|
| descriptor listing `gateway.chat.stream` | `data-state="available"`, `data-streaming="true"`, the version rendered |
| descriptor without it | `data-state="available"`, `data-streaming="false"` |
| `setServiceInfo({ status: 503 })` | `data-state="degraded"` and a rendered reason |
| `setReachable(false)` | `data-state="degraded"` and a rendered reason |
| `PAIGASUS_SERVICES` with no `gateway` key | **not reachable from this app** — `lib/config.ts` refuses to parse. Assert that instead, in `tests/unit/config.test.ts` (Task 1), and record here that the `absent` branch of the view is covered only by the unit tier |

That last row matters: the `absent` state is unreachable at the integration tier **by construction**, because this app's config demands a `gateway` entry. Do not fake it by reaching into `@paigasus/discovery`'s internals; state the limit in the test file's header.

- [ ] **Step 5: Write the failing scope-page test**

`tests/integration/scope-page.test.ts`:

- a permitted `GetOrganization` renders the breadcrumbs with the organization's name and the overview beneath it, with `data-testid="gateway-scope"` holding that name;
- a **denied** `GetOrganization` (a `FakeIamHandlers` override throwing `denial({ code: Code.PermissionDenied, reason: 'forbidden' })` with an `ErrorInfo` detail carrying the correlation id) makes the page call `forbidden()` — assert against a mocked `next/navigation` — and the correlation id survives into the error the view would render;
- a non-UUID `[org]` segment calls `notFound()` and makes **no** IAM call: assert `iam.callsTo('tenancy.getOrganization')` is unchanged;
- an `invalid-input` answer calls `notFound()`, not the error view.

- [ ] **Step 6: Write the failing provisioning test**

`tests/integration/provisioning.test.ts` covers what is specific to **this app's** wiring rather than to `@paigasus/console-core` (which has its own tier for the resolver itself):

- the resolver this app builds in `lib/auth.ts` calls `GetServiceInfo` **before** `Introspect` — assert on the ORDER of `iam.calls`, not merely that both happened;
- both of those calls carry the request's correlation id, which is what `lib/auth.ts`'s `await requestCorrelationId()` exists for;
- `myScopes()` through this app's runtime returns memberships only when the fake IAM reports no `iam.authz.cedar`, and memberships plus grants when it does, deduplicated, capped at `SCOPE_CAP`.

- [ ] **Step 7: Run the tier**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts/apps/gateway-console exec vitest run
moon run paigasus-console-core-ts:test
moon run gateway-console-ts:test
```

Expected: all PASS. `paigasus-console-core-ts:test` must stay green — this task adds a file to `testing/`, which is one of that task's inputs.

- [ ] **Step 8: Prune and commit**

Grep the app for every dependency declared in `package.json` and delete any nothing imports. Then:

```bash
git add ts/packages/paigasus-console-core/testing ts/apps/gateway-console
git commit -m "test(ts): fake gateway double and the gateway-console integration tier (SMA-512)"
```

---

### Task 5: The single-zone end-to-end tier and the standalone-runtime proof

This is the tier that proves the zone actually works: a production build behind the standalone `server.js`, reached through the TLS terminator, with `PAIGASUS_SESSION_STORE=memory` and **one** zone in `PAIGASUS_ZONES` (spec § 10.4). The two-zone tier that proves acceptance criteria 1 and 2 is pull request 4.

**Files:**
- Create: `ts/apps/gateway-console/playwright.config.ts`
- Create: `ts/apps/gateway-console/tests/e2e/global-setup.ts`
- Create: `ts/apps/gateway-console/tests/e2e/support/paths.ts`
- Create: `ts/apps/gateway-console/tests/e2e/support/world.ts`
- Create: `ts/apps/gateway-console/tests/e2e/support/harness.ts`
- Create: `ts/apps/gateway-console/tests/e2e/support/login.ts`
- Create: `ts/apps/gateway-console/tests/e2e/support/correlation.ts`
- Create: `ts/apps/gateway-console/tests/e2e/public.spec.ts` (R1)
- Create: `ts/apps/gateway-console/tests/e2e/login.spec.ts` (R2, R3, R6)
- Create: `ts/apps/gateway-console/tests/e2e/capabilities.spec.ts` (R4)
- Create: `ts/apps/gateway-console/tests/e2e/token-leak.spec.ts` (R5)
- Create: `ts/apps/gateway-console/tests/e2e/forbidden.spec.ts` (R7 — an addition, see Step 8)
- Create: `ts/apps/gateway-console/tests/standalone-runtime.test.ts`
- Modify: `ts/apps/gateway-console/moon.yml` (add the `test-e2e` task; restore `playwright.config.ts` to `typecheck`'s inputs)
- Modify: `ts/apps/gateway-console/package.json` (add `@playwright/test`)

**Interfaces:**
- Consumes: `startFakeIam`, `startFakeIdp`, `startFakeGateway`, `startTlsTerminator`, `testTls` from `@paigasus/console-core/testing`; the `data-testid`s from Task 3.
- Produces: `Harness` (with `iam`, `idp`, `gateway`, `url()`, `serverOutput()`, `useWorld()`), `signIn()` and `waitForHydration()` — pull request 4's two-zone tier starts from these.

- [ ] **Step 1: Write `playwright.config.ts` and `tests/e2e/support/paths.ts`**

`playwright.config.ts` is `ts/apps/iam-console/playwright.config.ts` with one change: `outputDir` becomes `path.join(os.tmpdir(), 'gateway-console-e2e-results')`. Keep everything else verbatim, including the reasons in the comments:

- `workers: 1` and `fullyParallel: false` — every spec shares one server and one fake IAM, and the specs count calls;
- `retries: isCI ? 2 : 0` and `timeout: isCI ? 120_000 : 60_000` and `expect.timeout: isCI ? 15_000 : 5_000` — **CI only**. Raising the local values would hide a real local regression behind a retry;
- `outputDir` **outside** the app directory: the app directory is Tailwind's scan root, the guard walks all of it, and the Moon `test` inputs hash `tests/**`;
- `ignoreHTTPSErrors: true` — the terminator and the fake IdP use a self-signed test certificate;
- `forbidOnly: !!process.env.CI` — a committed `test.only` would drop the proofs from CI in silence.

`tests/e2e/support/paths.ts` is `iam-console`'s with `STANDALONE_APP_DIR` pointing at `apps/gateway-console`.

- [ ] **Step 2: Write `tests/e2e/global-setup.ts`**

`iam-console`'s verbatim, with the app name in its error message. It does **not** start servers — a Playwright `globalSetup` runs in a different process from the tests, so anything a test must script or count belongs in the worker fixture. Keep the `.next/static` copy into the standalone tree: the standalone output has none, and without the copy every client chunk is a 404 and nothing hydrates.

- [ ] **Step 3: Write `tests/e2e/support/world.ts`**

Much smaller than `iam-console`'s: this zone calls five IAM methods, and `setHandlers()` **replaces** the whole map, so the default set must carry every key an override can use. Declare exactly the methods this zone reaches — `authn.introspect`, `authz.listRoleGrants`, `tenancy.getOrganization`, `tenancy.getTeam`, `tenancy.getProject` — and nothing else. An unused handler is dead code a reviewer will flag.

```ts
export const PRINCIPAL_PRN = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0';
export const ORG_ID = '0190a100-0000-7000-8000-0000000000e1';
export const ORG_PRN = `prn:pgs:iam:::organization/${ORG_ID}`;
export const ORG_NAME = 'Acme Research';

export type Descriptor = { service: string; version: string; capabilities: string[] } | { status: number };
export const DEFAULT_IAM_DESCRIPTOR: Descriptor = { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar'] };
export const DEFAULT_GATEWAY_DESCRIPTOR: Descriptor = { service: 'gateway', version: '0.0.0-e2e', capabilities: ['gateway.chat.stream'] };

export type WorldOptions = {
  /** false: a first-time identity with no membership and no grant. Default: true. */
  readonly memberships?: boolean;
  /** What the fake IAM's `GET /v1/service-info` answers. Default: DEFAULT_IAM_DESCRIPTOR. */
  readonly iamDescriptor?: Descriptor;
  /** What the fake gateway's `GET /v1/service-info` answers. Default: DEFAULT_GATEWAY_DESCRIPTOR. */
  readonly gatewayDescriptor?: Descriptor;
  /** Replace single handlers, for example with one that throws denial(). */
  readonly overrides?: FakeIamHandlers;
};
```

Both descriptor keys are what Step 4's `useWorld` resets, and what the R4 and R7 specs script. A `WorldOptions` without them makes `harness.useWorld({ gatewayDescriptor: … })` a type error.

PRNs are **literal strings**: `@paigasus/console-core`'s `prn-tenancy.ts` imports `server-only`, which throws under Playwright.

- [ ] **Step 4: Write `tests/e2e/support/harness.ts`**

Start from `ts/apps/iam-console/tests/e2e/support/harness.ts` and make exactly these changes. Everything else — `freePort`, `stop`, `waitForHealth`, `serverEnv`, `closeInOrder`, the `MAX_START_ATTEMPTS` retry loop and the worker-scoped fixture with its CI timeout — is copied verbatim, comments included.

1. Start a fake gateway alongside the fake IdP and the fake IAM, and `unshift` its close onto `started` so a start that throws still closes it.
2. `Harness` gains `readonly gateway: FakeGateway`.
3. The server environment becomes:
   - `PAIGASUS_ZONE: 'gateway'`
   - `PAIGASUS_ZONES: JSON.stringify({ gateway: '/gateway' })`
   - `PAIGASUS_SERVICES: JSON.stringify({ iam: iam.httpUrl, gateway: gateway.url })`
   - everything else unchanged, `PAIGASUS_SESSION_STORE: 'memory'` and the 1/2/3 ms discovery timings included.
4. The health probe becomes `http://127.0.0.1:${port}/gateway/healthz`, and both failure messages name `gateway-console`.
5. `useWorld(options)` resets **three** things, not two: the IAM handlers, the IAM descriptor (`options.iamDescriptor ?? DEFAULT_IAM_DESCRIPTOR`) and the gateway descriptor (`options.gatewayDescriptor ?? DEFAULT_GATEWAY_DESCRIPTOR`). It must also restore `gateway.setReachable(true)`, or a test that made the gateway unreachable leaks that state into every later test in the worker.

The auto `world` fixture that calls `harness.useWorld()` before every test is what makes point 5 safe. Keep it.

- [ ] **Step 5: Write `tests/e2e/support/login.ts` and `correlation.ts`**

`login.ts` is `iam-console`'s with the default path changed to `/gateway/overview`. Keep `waitForHydration` — `Providers` sets `html[data-hydrated="true"]` in an effect, and a click before that is a document navigation, not a client navigation. Keep the `harness.idp.issued` delta read: it snapshots before the login and slices after, which is what makes the assertion independent of every login that ran earlier in the worker.

`correlation.ts` is `iam-console`'s verbatim: it reads `FORBIDDEN_VIEW_CORRELATION` out of `@paigasus/console-core`'s `src/correlation.ts` as **text**, because that module imports `server-only` and would throw under Playwright.

- [ ] **Step 6: Write the six specs from spec § 10.4**

| Row | Spec | Assertions |
|---|---|---|
| R1 | `/gateway/` loads with no cookie, with its CSS and JS | every `/gateway/_next/static/` response is 200, at least one stylesheet and one script, the body's computed `background-color` is not transparent (so `globals.css` and the token layer actually applied), and no `__Host-pgs_sid` cookie was set |
| R2 | cold login lands on the overview | `signIn(page, harness, '/gateway/overview')`; the URL is `/gateway/overview`, `data-testid="zone-overview"` is visible, and the fake IAM's call list shows `http.getServiceInfo` **before** `authn.introspect` — assert on the ORDER of `harness.iam.calls`, not on both being present |
| R3 | a first-time, unprovisioned user reaches the same screen | `harness.useWorld({ memberships: false })`, then the same login; the overview renders and the organization switcher is absent rather than the page failing |
| R4 | the gateway reported degraded | `harness.useWorld({ gatewayDescriptor: { status: 503 } })`; `data-state="degraded"`, a visible `data-testid="gateway-reason"`, and the **Overview nav entry is disabled with a reason** — `aria-disabled="true"` plus an `aria-describedby` target with non-empty text — rather than hidden |
| R5 | no response body carries a token | collect every response body during a signed-in navigation — HTML documents, RSC payloads and route-handler output alike — and assert neither the access nor the refresh token from `signIn`'s return value appears in any of them (ADR-0017). Use `ts/apps/iam-console/tests/e2e/token-leak.spec.ts` as the template; it already handles the RSC content types |
| R6 | sign out, then `/gateway/` redirects to login again | after `signIn`, navigate to `/gateway/auth/logout`, then `page.goto('/gateway/overview')` and assert the final URL is the IdP or the login route, and that the session cookie is gone |

R4 also proves that the capability gate reads a **live** state: pair it with a second case that leaves the gateway available but drops `gateway.chat.stream` from its descriptor, and assert `data-streaming="false"` while `data-state` stays `"available"`.

- [ ] **Step 7: Write `tests/standalone-runtime.test.ts` (acceptance criterion 4)**

`ts/apps/iam-console/tests/standalone-runtime.test.ts` with these changes: `SERVER_ENTRY` points at `../.next/standalone/apps/gateway-console/server.js`; `COMPLETE_ENV` uses `PAIGASUS_ZONE: 'gateway'` and a `PAIGASUS_SERVICES` carrying both `iam` and `gateway`; the probe path is `/gateway/healthz`; the two zone maps differ in their non-gateway entry; and the mismatch case uses `{ gateway: '/admin/gateway' }` with the message `Base path mismatch for zone "gateway"`.

Keep the shape of the mismatch assertion: it asserts the mismatch **message in the server output**, not only a 500. A missing unrelated variable also answers 500 and must not pass this test.

- [ ] **Step 8: Write `forbidden.spec.ts` (R7 — an addition beyond spec § 10.4)**

Spec § 10.4's table has no 403 row, but Task 3 builds `(console)/forbidden.tsx` and nothing else proves it renders with a **real** HTTP 403 in this zone. `experimental.authInterrupts` is experimental, so without this row a Next upgrade that changes it would silently render the view with status 200. Record the addition in the plan review and in the pull request body.

Drive it through the scope route, with a `tenancy.getOrganization` override that throws `denial({ code: Code.PermissionDenied, reason: 'forbidden' })`, and assert:

- the response status is **403**, not 200;
- `data-testid="forbidden-view"` is visible **and** the console shell is still rendered around it (the boundary is per-segment and renders inside `(console)/layout.tsx`);
- when `forbiddenViewCorrelation()` answers `'header'`, `data-testid="correlation-id"` holds the same id the fake IAM recorded for that request — read it off `harness.iam.calls`.

- [ ] **Step 9: Add the `test-e2e` task to `moon.yml`**

Copy `ts/apps/iam-console/moon.yml`'s `test-e2e` block and make exactly these changes: drop the two `!tests/fixtures/...` negations (this app has none), and keep everything else — `options.cache: false` (a real build and a real Chromium; a cached PASS would replay a green that tested nothing), `deps: ['~:build', 'repo:next-env-drift', 'contracts:generate']`, and every `/ts/packages/...` input line.

Also restore `playwright.config.ts` to the `typecheck` task's `inputs` (Task 1 Step 10 removed it): `tsconfig.json` includes `**/*.ts`, so `tsc` type-checks that file, and without the input a type error there serves a cached PASS.

Add `"@playwright/test": "catalog:"` to `devDependencies` and run `pnpm -C ts install`.

- [ ] **Step 10: Install Chromium and run the tier**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts/apps/gateway-console exec playwright install chromium
moon run gateway-console-ts:build
moon run gateway-console-ts:test
moon run gateway-console-ts:test-e2e
```

Expected: all PASS. If a spec fails on a missing client chunk, `global-setup.ts`'s `.next/static` copy did not run or the build was stale — rebuild before re-running. Report the tier's wall-clock time in your report: spec § 14 item 5 wants it on record before pull request 4 adds a second server and a container.

- [ ] **Step 11: Commit**

```bash
git add ts/apps/gateway-console
git commit -m "test(ts): single-zone e2e tier and standalone-runtime proof for gateway-console (SMA-512)"
```

---

### Task 6: The gate registries and the strict-equality re-baselines

This task closes the known-red window. It is the highest-risk task in the plan: every edit here is a **pin**, and a pin that is re-baselined without understanding what moved converts a gate into decoration. Nothing in this task may be guessed — every expected set is **measured** and every addition is justified in the commit.

**Files:**
- Modify: `ci/next-public/run.sh` (`APP_CONFIG_FLOOR`)
- Modify: `ci/affected-graph/ci_targets.py` (`NEXT_PUBLIC_FREE_SH_CALL_SITES`, `TAILWIND_GUARD_INVOCATIONS`)
- Modify: `moon.yml` (root — `repo:next-env-drift`'s `deps`)
- Modify: `ci/affected-graph/run.sh` (every strict-equality case the new project joins, plus two new cases)
- Modify: `.github/workflows/ci.yml` (the Playwright Chromium install step)
- Modify: `.github/CODEOWNERS` (regenerated, never hand-edited)

- [ ] **Step 1: Raise the app-config floor and its pin, in ONE edit**

`ci/next-public/run.sh:100` carries the instruction in its own comment: *"raise this when a second console zone app lands, and lower it only for a real removal."*

```
APP_CONFIG_FLOOR=1   ->   APP_CONFIG_FLOOR=2
```

That literal line is pinned **as a whole line** in `ci/affected-graph/ci_targets.py:1251`:

```python
    "APP_CONFIG_FLOOR=1",   ->   "APP_CONFIG_FLOOR=2",
```

Both edits go in the same commit. Leaving the floor at 1 means the gate stays green if this app's `next.config.ts` later disappears — the collapse the floor exists to detect. `APP_CONFIG_GLOB` needs no change: it already globs `ts/apps/*`.

Verify with the bash this gate needs (it calls `mapfile`, a bash-4+ builtin that system `/bin/bash` 3.2 does not have):

```bash
/opt/homebrew/bin/bash ci/next-public/run.sh --self-test
/opt/homebrew/bin/bash ci/next-public/run.sh --negative-control
/opt/homebrew/bin/bash ci/next-public/run.sh
```

Expected: all three PASS. A wall of "expected rc 0" self-test failures across unrelated rows means the wrong bash ran the gate, not a real finding.

- [ ] **Step 2: Register the new app in `TAILWIND_GUARD_INVOCATIONS`**

`ci/affected-graph/ci_targets.py:702`:

```python
TAILWIND_GUARD_INVOCATIONS = {"iam-console"}   ->   TAILWIND_GUARD_INVOCATIONS = {"gateway-console", "iam-console"}
```

This is a **set of app names**, not stored lines: `_expected_tailwind_lines(app)` derives the three invocation lines from the name, so there is nowhere left to write another app's `--app` directory. The check matches against moon's **resolved** `test` script, so a line parked in another task or in a task that never runs does not count. Task 1 Step 10 already wrote the three lines into `gateway-console-ts:test`; this entry is what makes their absence fail.

**Verify the check can fail before trusting that it passes.** Temporarily change the third line in `ts/apps/gateway-console/moon.yml` to `--app ts/apps/iam-console`, run the suite, confirm it reports the mismatch, then restore. Record the observation in your report.

- [ ] **Step 3: Add the drift gate's ordering edge**

Root `moon.yml`, `repo:next-env-drift`'s `deps` (`:142-143`):

```yaml
    deps:
      - 'iam-console-ts:build'
      - 'gateway-console-ts:build'
```

The comment above it states the reason: `next typegen` writes into each app's own `.next`, which is that app's `build` output, so without one edge per app the gate and the build race on the same directory. **Do not touch the `inputs` block** — every entry is already a `ts/apps/*` glob, and widening `ts/apps/*/next.config.ts` to other extensions would red `repo:input-liveness`, which fails a declared glob matching zero tracked files.

Spec § 8.1 and § 13 record that this gate still has **no negative control**; its correctness rests on the discovery loop and the subset assertion. That is a follow-up issue, not this pull request.

- [ ] **Step 4: Re-baseline the affected-graph cases — MEASURED, never guessed**

Run the suite first and read what it reports. This gate needs **system `/bin/bash` 3.2**: Homebrew's bash 5.3.15 deadlocks on a `while read` fed by a here-string over roughly 512 bytes on this class of machine.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/affected-graph/run.sh 2>&1 | tee /tmp/affected-before.txt
```

Every case the new project joins fails with an **unexpected** list naming `gateway-console-ts:build`, `:test` and/or `:test-e2e`. For each one:

1. Read the case's anchor file and its comment. Ask *why* the new app's task keys on that anchor.
2. Confirm the answer against `ts/apps/gateway-console/moon.yml`: the anchor's package must appear in that task's `inputs`.
3. Only then append the reported tasks to that case's expected CSV.

**Never append a task the run did not report, and never delete one it did.** An expected set edited to match a guess is a gate that no longer measures anything.

From the inventory taken at `f774ae6d`, expect to touch these cases — but let the run, not this table, decide the final membership:

| Case | Line | Anchor |
|---|---|---|
| `contracts->proto` (a project-level `run_case`) | 281 | the contracts tree |
| `ui->console` | 390 | `paigasus-ui/src/styles/tokens.css` |
| `ui-components->console` | 407 | `paigasus-ui/src/components/table.tsx` |
| `auth->auth-tasks` | 422 | `paigasus-auth/src/config.ts` |
| `proto->sdk` | 440 | `paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts` |
| `proto-iam->sdk` | 464 | `paigasus-proto/src/generated/paigasus/iam/v1/iam_pb.ts` |
| `discovery->discovery-tasks` | 478 | `paigasus-discovery/src/core/state.ts` |
| `discovery-adapters->discovery-tasks` | 492 | `paigasus-discovery/src/adapters/memory-cache.ts` |
| `app-shell->app-shell-tasks` | 502 | `paigasus-app-shell/src/…` |
| `app-shell-shell->app-shell-tasks` | 510 | `paigasus-app-shell/src/…` |
| `sdk->iam-console` | 520 | `paigasus-sdk/src/iam.ts` |
| `sdk-errors->iam-console` | 522 | `paigasus-sdk/src/errors/map-error.ts` |
| `app-shell->console` | 528 | `paigasus-app-shell/src/shell/app-shell.tsx` |
| `console-core->consumers` | 558 | `paigasus-console-core/src/runtime.ts` |
| `console-core-prn-tenancy->consumers` | 560 | `paigasus-console-core/src/prn-tenancy.ts` |
| `console-core-testing->consumers` | 577 | `paigasus-console-core/testing/…` |

The two cases named after `iam-console`'s own files — `iam-console-lib->iam-console-tasks` (:542) and `iam-console-proxy->iam-console-tasks` (:544) — must **not** gain the new app: their anchors are inside `ts/apps/iam-console/`, which is not an input of any `gateway-console-ts` task in this pull request. (Pull request 4 adds `/ts/apps/iam-console/**/*` to `gateway-console-ts:test-e2e`'s inputs and re-baselines them then. That edge is what makes the two-zone tier re-run on an `iam-console` change, and it comes from `inputs`, never from `dependsOn`.)

The names `sdk->iam-console` and `sdk-errors->iam-console` become inaccurate once a second app joins them. **Do not rename them in this pull request** — a renamed case is invisible in a diff that also re-baselines it. Note the mismatch in the case comment instead.

- [ ] **Step 5: Add the two app-local cases**

Mirror `iam-console`'s pair, with a comment giving each anchor's reason:

```bash
  # SMA-512 — the new zone's composition root. Its build, test and e2e tier all key on lib/**/*
  # through fileGroups.sources; without this case, narrowing that group would silently stop
  # selecting them and every later change to the zone's configuration would serve a cached pass.
  run_task_case_ci "gateway-console-lib->gateway-console-tasks" "ts/apps/gateway-console/lib/config.ts" \
    "<measured>"

  # SMA-512 — the zone's proxy. It sits at the project root, so only the explicit `proxy.ts` entry
  # in fileGroups.sources reaches it.
  run_task_case_ci "gateway-console-proxy->gateway-console-tasks" "ts/apps/gateway-console/proxy.ts" \
    "<measured>"
```

Take each expected CSV from a real run. Anchor the first on `lib/config.ts` — a file that stays, unlike `iam-console`'s original anchor, which had to be re-anchored in pull request 2 when the file it named moved.

Also update the doc comment at `ci/affected-graph/run.sh:89-92`, which hardcodes the number of projects declaring a `test-e2e` task. It is prose, asserted nowhere, and read by reviewers — four becomes five.

- [ ] **Step 6: Re-run the suite until it is clean**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/affected-graph/run.sh 2>&1 | tee /tmp/affected-after.txt
```

Expected: every case PASSES. Then run the whole gate through Moon, which also runs `ci_targets.py`'s registry checks and the negative control:

```bash
moon run repo:affected-smoke --force
```

Expected: PASS. If it aborts in under three seconds, grep the output for `proto-shim` — a `Permission denied (os error 13)` from the shim under a concurrent `moon ci` is an infrastructure abort (rc 2), not a verdict. Capture the full output **before** re-running; a passing re-run overwrites `stdout.log`, truncates `stderr.log` and flips the `ciReport.json` row, destroying the evidence.

- [ ] **Step 7: Name the app in the CI workflow**

`.github/workflows/ci.yml`, the Playwright Chromium install step (`:194-203`). Append one line to the `run:` block:

```bash
          pnpm --dir ts/apps/gateway-console exec playwright install --with-deps chromium
```

and add the app to the step's `name:`. Run it from the **app**, not from `ts/`: `@playwright/test` is a devDependency of each package, so the binary is only on that package's `node_modules/.bin`, and from `ts/` it fails with `ERR_PNPM_RECURSIVE_EXEC_FIRST_FAIL`.

The step is deliberately **unconditional**, with no `if:`. A path-filtered condition can be skipped on a run where `moon ci` still selects `test-e2e`, and the task then fails for a missing browser. Do not add one.

`ci.yml`'s `T=(…)` array needs no change: it lists task names, not projects.

- [ ] **Step 8: Regenerate CODEOWNERS**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon sync code-owners
git diff --stat .github/CODEOWNERS
```

`.github/CODEOWNERS` is Moon-generated and must never be hand-edited. `ci.yml:350-353` runs this command and then `git diff --exit-code` on the file, unconditionally, so an unregenerated file reds CI.

- [ ] **Step 9: Run the repository gates that have a working local bash**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:next-env-drift --force
moon run repo:input-liveness --force
/opt/homebrew/bin/bash ci/ruff/run.sh
moon run ts:lint
moon run ts:fmt
```

`repo:actionlint` has **no working local bash** on this class of machine: system 3.2 runs past an hour without finishing, and Homebrew 5.3.15 deadlocks at 0% CPU. Its verdict must come from CI. Say so plainly in your report rather than claiming a local pass.

`ts:fmt` is its own whole-tree Prettier gate, decoupled from `ts:lint` and from `tsc`. Run it **and gate the commit on it** — do not chain it behind `&&` with the commit.

- [ ] **Step 10: Commit**

```bash
git add ci/next-public/run.sh ci/affected-graph/ci_targets.py ci/affected-graph/run.sh moon.yml .github/workflows/ci.yml .github/CODEOWNERS
git commit -m "ci(ts): register gateway-console in the app-scoped gates and re-baseline the affected graph (SMA-512)"
```

The body must name each re-baselined case and say, in one line each, why the new app's task legitimately keys on that anchor. A re-baseline commit whose body does not is unreviewable.

---

### Task 7: Documentation, the recorded measurements and the full-graph run

**Files:**
- Modify: `ts/README.md`
- Modify: `CONTRIBUTING.md`
- Modify: `CLAUDE.md`
- Modify: `docs/superpowers/specs/2026-09-13-sma-512-gateway-console-design.md` (§ 14 measurement rows this pull request answered)

- [ ] **Step 1: `ts/README.md` and `CONTRIBUTING.md`**

Add `gateway-console` beside `iam-console` in whatever list each file already keeps: the app's name, its base path, its Moon id, and one line on what the zone does. Do not restructure either document.

- [ ] **Step 2: `CLAUDE.md`**

Three existing entries speak about the second app in the **future** tense and are now wrong. Correct them rather than appending a new entry beside them:

1. The Tailwind entry says *"Since SMA-512 the guard is per app"* and describes `TAILWIND_GUARD_INVOCATIONS`. Record that the registry now holds **two** apps, and that each app's own `test` task invokes the guard for itself.
2. The `repo:next-public-free` material: `APP_CONFIG_FLOOR` is 2, and its pin in `ci_targets.py` moved with it.
3. The `repo:next-env-drift` material: its `deps` now names one build per app, and the gate still has no negative control.

Then add one new bullet for the app itself, in the style of the existing `@paigasus/console-core` entry: what `ts/apps/gateway-console` is, that its `lib/config.ts` demands **both** an `iam` and a `gateway` entry in `PAIGASUS_SERVICES` and declares no gateway-specific key, that the zone overview lives at `/gateway/overview` and not at `/gateway/` (plan D14), and that `gateway.chat.stream` counts only when the service is `available`.

**Two gates constrain this file. Both are easy to trip:**

- `ci/actionlint/run.sh` check 12 requires a `<!-- moon-diagnosis:ok -->` marker on any file that names `ciReport.json`, unless the file is in `CIREPORT_MENTIONS_ALLOWED`. This plan carries the marker for exactly that reason.
- The `<!-- ci-targets:begin -->` / `<!-- ci-targets:end -->` markers must stay unique in `CLAUDE.md`. A second copy of either marker anywhere in the file — **including inside backticks in prose** — makes the count 2 and reds `repo:affected-smoke`. Do not quote the delimited command.

- [ ] **Step 3: Record the measurements spec § 14 asked for**

Update the spec in place. This pull request answers item 3 and part of item 5; items 1, 2 and 4 belong to pull request 4 and stay open.

- **Item 3 — does pnpm resolve `@paigasus/console-core/testing`'s devDependencies for a consuming app?** Tasks 4 and 5 import that subpath from an app that does not declare `testcontainers`, `jose` or `@connectrpc/connect-node` itself. State what you measured: whether the tier ran, and if anything had to be added to the app's `devDependencies` to make it run. If something did, that is the answer, and it must be written down rather than quietly added.
- **Item 5 — the wall-clock cost of the tier.** Record Task 5's `test-e2e` duration, local and, once the pull request is open, in CI. Pull request 4 adds a second server and a Redis container on top of it, and `iam-console`'s tier already needs a 420 s worker timeout in CI.

- [ ] **Step 4: The full-graph run, with the bash split stated honestly**

No single local bash runs every gate on this machine. Run each gate under the build it needs and report the three results separately:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"

# The main graph. Pick one bash for the whole invocation; the gates below get their own runs.
moon ci :build :test :lint :fmt :typecheck :test-e2e --base origin/main --include-relations

# Needs system bash 3.2 (Homebrew 5.3.15 deadlocks on its here-string reader).
/bin/bash ci/affected-graph/run.sh

# Need bash 4+ (they call `mapfile`, which bash 3.2 does not have).
/opt/homebrew/bin/bash ci/ruff/run.sh
/opt/homebrew/bin/bash ci/next-public/run.sh
```

`repo:actionlint` has no working local bash. Its verdict comes from CI, and your report must say that rather than imply a local pass.

If a gate fails, capture `.moon/cache/ciReport.json` and `.moon/cache/states/<project>/<task>/` **before** re-running anything: a passing re-run rewrites `stdout.log`, truncates `stderr.log`, rewrites `lastRun.json` and flips the report row, destroying the evidence. <!-- moon-diagnosis:ok -->

- [ ] **Step 5: Commit**

```bash
git add ts/README.md CONTRIBUTING.md CLAUDE.md docs/superpowers/specs/2026-09-13-sma-512-gateway-console-design.md
git commit -m "docs(ts): record the gateway-console zone and the gates it re-baselines (SMA-512)"
```

---

## Known limits this pull request ships

State these in the pull request body. They are deliberate, and each is either recorded in the spec or ruled on here.

- **Acceptance criteria 1 and 2 are not proved here.** The single-zone tier cannot prove a cross-zone property. Pull request 4 owns them.
- **Acceptance criterion 3 is not delivered at all** (decision D11, spec § 2.3): the gateway's chat route authenticates Paigasus API keys and never an OIDC token, so a playground built on a console session could not make one real call. The follow-up issue owns widening `require_iam_auth` and deciding the authorization resource for a user principal.
- **The scope route changes the URL and the breadcrumbs and nothing else** (decision D8). It exists so the later settings screens hang off a working shape and so the switcher is not a dead control.
- **The organization switcher runs `myScopes()` on every render in this zone** — one `Introspect`, one `ListRoleGrants` walk and up to 50 tenancy reads. This design now pays that cost twice across the two zones, on decision D7. It is unmeasured, in both zones.
- **A lost `cache()` memoization is observable only in the e2e tier.** Outside a React server render `cache()` is a pass-through, so no unit or integration test can see a second `createConsoleRuntime()` call. Pull request 4's `Introspect` count is the only control that can, and it does not exist yet — so **this pull request ships the rule with no automated control behind it**.
- **`repo:next-env-drift` still has no negative control.** Its correctness rests on the discovery loop and the subset assertion (spec § 8.1, § 12 item 3).
- **A stopped zone app is invisible to ADR-0020 discovery.** Discovery probes services, not zone apps, so a zone whose app is down still renders an *available* nav entry pointing at a dead route. SMA-513's ingress is where that would be caught.
- **The `absent` gateway state is covered only at the unit tier.** This app's `lib/config.ts` refuses to parse a `PAIGASUS_SERVICES` map with no `gateway` entry, so the integration tier cannot reach that branch of the view (Task 4 Step 4).
- **The presentation-copy table is per zone** (plan D16), so the two zones can word the same presentation differently. That is intended — the tables are content, not shared logic — and nothing gates them against each other.
- **`sdk->iam-console` and `sdk-errors->iam-console` are now misnamed**, because both cases cover two apps. Renaming them was deliberately deferred: a renamed case is invisible in the same diff that re-baselines it (Task 6 Step 4).
