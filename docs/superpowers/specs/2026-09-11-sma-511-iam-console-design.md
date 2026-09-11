# SMA-511 — `iam-console`: the IAM zone

**Status:** approved design, revision 1 (before adversarial challenge)
**Date:** 2026-09-11
**Issue:** [SMA-511](https://linear.app/smaschek/issue/SMA-511/ts-iam-console-app-the-iam-zone)
**ADR:** ADR-0017 — Console topology & session ownership; ADR-0019 (error model, E5/E8);
ADR-0020 (capability discovery)
**Design:** Frontend Architecture Scoping §§ 2.3, 3, 6, 8
**Depends on:** SMA-502 (`@paigasus/next-config`), SMA-503 (`@paigasus/ui`), SMA-506
(`@paigasus/auth`), SMA-508 (`@paigasus/sdk`), SMA-509 (`@paigasus/discovery`), SMA-510
(`@paigasus/app-shell`) — all Done
**Blocks:** SMA-513 (multi-zone ingress and Helm chart)

---

## 1. Problem

The console is a set of Next.js apps, one per backend service, on one origin (ADR-0017). Six
shared packages exist for it: `next-config`, `ui`, `auth`, `sdk`, `discovery` and `app-shell`. No
app uses them together yet.

`ts/apps/paigasus-console` is already partly the IAM zone. Its `next.config.ts` calls
`createNextConfig({ zone: 'iam', basePath: '/iam' })`, and it has a runtime config, a
`LinkProvider` and a standalone-server test. It has no auth, no SDK calls, no shell and no
screens.

This issue makes it the first real console zone: login, the shell, tenancy screens over the SDK,
the 403 view, and capability-gated screens.

### 1.1 Acceptance criteria (from the issue)

1. A user logs in through this zone and lands on a tenancy screen.
2. A Cedar denial renders the 403 view correctly. The UI does not pre-judge authorization:
   `can()` only hides affordances, and the server decision is authoritative.
3. The app builds to a standalone image with no `NEXT_PUBLIC_` usage (the SMA-502 gate is green).
4. Capability-gated screens (audit, dead-letters) appear only when IAM reports the matching
   capability.
5. MSW-backed app tests run without live services.

§ 2 records how this design changes AC 4 and AC 5, and why.

---

## 2. Scope and decisions

### 2.1 Decisions (Sven, 2026-09-11)

| # | Question | Decision |
|---|---|---|
| D1 | Split of the work outside the app | This PR holds the Turbopack import fix (§ 7.1) and the Introspect principal resolver (§ 4.5). A new issue holds the dead-letters capability key and its screen. |
| D2 | App name, and `paigasus-docs` | Rename to `ts/apps/iam-console` (`@paigasus/iam-console`, Moon id `iam-console-ts`). Delete the `paigasus-docs` stub. |
| D3 | Test harness for AC 5 | In-process fake servers plus MSW (§ 9). No Docker. |
| D4 | Mutations | Create organization, create team, create project, attach membership, detach membership. |

### 2.2 How the acceptance criteria change

- **AC 3 — "standalone image".** SMA-513's scope says "Console images following the conventions
  established for the services in SMA-500". So the OCI image belongs to SMA-513. This issue
  proves the `output: 'standalone'` build and the `NEXT_PUBLIC_` ban (§ 9.5).
- **AC 4 — dead-letters.** No dead-letters capability key exists. The registry in
  `contracts/proto/paigasus/common/v1/service_info.proto` has `iam.authz.cedar`, `iam.apikeys`,
  `iam.audit` and `gateway.chat.stream` only, and IAM emits only its three
  (`rs/crates/services/paigasus-iam/src/service_info.rs:27-63`). Per D1, this issue meets AC 4 for
  audit, and the follow-up issue (§ 11) meets it for dead-letters.
- **AC 5 — MSW.** MSW intercepts `fetch` and `http`/`https`. The SDK calls IAM through
  `createGrpcTransport`, which uses `node:http2`, so MSW cannot intercept it. Per D3, the gRPC
  calls go to a real in-process fake server, and MSW intercepts the `fetch` calls. After the spec
  is approved, AC 5 in Linear is reworded to "in-process fake servers and MSW", with this reason.

### 2.3 In scope

- The rename and the deletion (§ 3).
- The composition root (§ 4), the screens (§ 5), the error handling (§ 6).
- The Turbopack import fix in four packages, and the lint rule that holds it (§ 7.1).
- The boundary rule for `proxy.ts` (§ 7.2), the Tailwind sentinel (§ 7.3), the SDK
  client-boundary fixture (§ 7.4), and the Moon and affected-graph changes (§ 8).
- Three test tiers (§ 9).

### 2.4 Out of scope

- The console OCI image and the ingress (SMA-513).
- The dead-letters capability key and screen (follow-up, § 11).
- Rename, archive and restore (follow-up, § 11).
- Policy and role-grant administration (`AuthorizationService`), service accounts and API keys.
- A search in the switcher, i18n, and any visual design beyond `@paigasus/ui`.

---

## 3. Structure and the rename

### 3.1 Rename

`git mv ts/apps/paigasus-console ts/apps/iam-console`. The package name becomes
`@paigasus/iam-console` and the Moon id becomes `iam-console-ts`. Reason: SMA-512 adds a
gateway zone app, and `paigasus-console` then no longer says which zone it is.

These sites hardcode the old path, and all change in the same commit:

| Site | Change |
|---|---|
| `ts/apps/iam-console/moon.yml` | the id, and the `.next/standalone/apps/iam-console/server.js` check in `build` |
| `ci/tailwind-source/run.mjs:17` | `CONSOLE_DIR` |
| root `moon.yml`, `repo:next-env-drift` | `deps` and four `inputs` paths |
| `ts/eslint.config.js:57-64` | the Next rules `files` glob and `settings.next.rootDir` |
| `ci/affected-graph/run.sh` | every `paigasus-console-ts` target in expected sets |
| `ci/next-env/run.sh`, `ci/next-public/run.sh` | any literal app path (the plan greps for all of them) |
| `ts/README.md`, `CLAUDE.md`, `ci/tailwind-source/README.md` | path mentions |

The plan finds every site with `git grep -n paigasus-console` and records the full list before
it changes one. A missed site in a gate's `inputs` switches that gate off in silence;
`repo:input-liveness` catches a dead glob only for `repo:*` tasks.

### 3.2 Delete `ts/apps/paigasus-docs`

It holds only `export {}`, and nothing depends on it. The pnpm lockfile loses its workspace
entry. A docs app gets its own issue when a framework is chosen.

### 3.3 App layout

```
ts/apps/iam-console/
  proxy.ts                         ← Next 16 file name; createAuthMiddleware(...)
  app/
    layout.tsx                     ← html/body and globals.css only
    globals.css
    auth/[...auth]/route.ts        ← GET/POST = createAuthRouteHandler(authRuntime())
    (public)/signed-out/page.tsx   ← PublicShell
    (console)/layout.tsx           ← requireSession → providers → AppShell
    (console)/page.tsx             ← redirect('/orgs')
    (console)/forbidden.tsx        ← the 403 view (§ 6.2)
    (console)/error.tsx            ← uncaught errors
    (console)/orgs/…               ← tenancy screens (§ 5)
    (console)/audit/page.tsx       ← capability-gated
  lib/                             ← composition root, every file `import 'server-only'`
    config.ts  auth.ts  iam.ts  discovery.ts  principal-resolver.ts  logger.ts  errors.ts
    nav.ts                         ← buildNavEntries
  tests/
    unit/  integration/  e2e/  support/  fixtures/client-imports-sdk/
```

- `proxy.ts` replaces `middleware.ts`. Next 16.3.4 deprecates the old name
  (`next/dist/lib/constants.js:287-290`; the dev server prints a deprecation warning).
- The `(console)` route group holds every page that needs a session, so one `requireSession()`
  in its layout guards all of them. The route handler and `(public)` sit outside the group.
- The style is functional, to match Next and React. Dependencies enter through the `lib/`
  accessors, not through classes.

---

## 4. The composition root (`lib/`)

Every file in `lib/` starts with `import 'server-only'`, so a client component that imports one
fails the build.

### 4.1 `lib/config.ts`

The app's one `defineRuntimeConfig` call composes three parts:

- `authEnvShape` from `@paigasus/auth/server`;
- `discoveryEnvShape` from `@paigasus/discovery/server`;
- one app-owned key, `PAIGASUS_IAM_GRPC_URL`: an absolute `http:` or `https:` URL, with no
  credentials, no query and no fragment.

`PAIGASUS_IAM_GRPC_URL` is a separate key because IAM listens on two addresses: HTTP on `8080` and
gRPC on `9090` (`rs/crates/services/paigasus-iam/src/config.rs:812-813`). Discovery's
`PAIGASUS_SERVICES` holds the HTTP address, which discovery uses for `GET /v1/service-info`.

`PAIGASUS_SERVICES` must contain `iam`. The IAM zone cannot work without IAM, so a missing `iam`
key fails the parse. The parse happens at the first request. `getRuntimeConfig()` already throws
during `phase-production-build`, so a module-scope read fails the build loudly.

### 4.2 `lib/auth.ts`

`authRuntime()` returns `getAuthRuntime(getRuntimeConfig(), { resolver, logger })`. It is a
process singleton. `@paigasus/auth` builds its own Redis store from `PAIGASUS_SESSION_REDIS_URL`.

### 4.3 `lib/iam.ts`

`iamClient(Service)` calls `requireSession(authRuntime())`, then
`createIamClient(Service, { baseUrl: PAIGASUS_IAM_GRPC_URL }, { bearer: session.accessToken })`.
React `cache()` memoizes it for one request only. No client and no token lives past the request
(`ts/packages/paigasus-sdk/src/iam.ts:19-24`). Only the SDK's transport is process-scoped.

### 4.4 `lib/discovery.ts`

`discovery()` makes one `Discovery` handle per request, with React `cache()` and
`waitUntil: after`.

- When `PAIGASUS_SESSION_STORE=redis`, the descriptor cache uses the same Redis URL as the
  session store. The app makes that client with `disableOfflineQueue: true` and an error
  listener that never logs the error, because node-redis embeds the DSN in it
  (`ts/packages/paigasus-discovery/src/adapters/redis-cache.ts:67-73`). An operator configures one
  Redis, not two.
- When `PAIGASUS_SESSION_STORE=memory`, discovery uses the memory cache.

### 4.5 `lib/principal-resolver.ts` — `IntrospectPrincipalResolver`

The only resolver today is `claimsPrincipalResolver`, which always returns
`grantsAvailable: false` (`ts/packages/paigasus-auth/src/adapters/claims-resolver.ts:16-27`). So
`can()` always fails open. The adapter that SMA-508 was to supply does not exist.

- It implements auth's `PrincipalResolver` port. It calls `AuthnService.Introspect` with the new
  access token, both as the bearer and as `IntrospectRequest.token`.
- It maps `principal_prn`, `issuer`, `subject`, `memberships` and `role_grants` to the port's
  domain types, with `grantsAvailable: true`.
- If the call fails or IAM rejects it, the resolver **does not fail the login**. It returns
  `principalPrn: null`, `memberships: []`, `roleGrants: []` and `grantsAvailable: false` (so `can()`
  fails open, which is cosmetic only). It logs `principal.introspect_failed` with the
  `presentation` value. The login then continues, and the pages show IAM's own answer.
- It lives in the app because `@paigasus/auth` must not import `@paigasus/sdk` (the port's header,
  `ports/principal-resolver.ts:3-8`), and the `sdk` boundary rule bans every `@paigasus/*` import
  except `proto`. Only the app may depend on both.

**Limit.** The auth package calls the resolver once, in the login callback
(`http/routes.ts:216`). The grants are therefore a login-time snapshot. A role change reaches
`can()` at the next login only. This is acceptable because `can()` is cosmetic and IAM decides
every call. The landing screen does not use the snapshot (§ 5.1).

### 4.6 `lib/logger.ts`

One JSON-lines adapter implements both `AuthLogger` and `DiscoveryLogger`, and writes to stdout.
It writes only the fields that each port gives; the ports already redact them. This is the first
real logger adapter that SMA-626 item 6 expects.

---

## 5. Screens, URLs and navigation

### 5.1 The landing screen (AC 1)

Only `platform_admin` can call `ListOrganizations`, and only `platform_admin` can call
`ListMemberships` filtered by principal. Both check against the root PRN
(`rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs:193`, `:669-676`;
`rs/crates/libs/paigasus-iam-core/src/authz/roles.rs:95-96`). An "all organizations" list as the
landing screen would give every `org_admin` a 403.

The login ends at `/iam`, which redirects to `/iam/orgs`, **"Your organizations"**:

1. The page calls `AuthnService.Introspect` **live** on each render. It does not use the login
   snapshot, because the snapshot is stale after a membership change. Cedar does not check
   Introspect.
2. It collects the organization UUIDs from the memberships. An organization PRN gives its own
   UUID (`prn:pgs:iam:::organization/<org>`); a team or project PRN gives the UUID in its org slot
   (`prn:pgs:iam::<org>:team/<team>`).
3. It calls `GetOrganization` for each UUID. A per-row 403 (a `team_member` has no
   `GetOrganization`) shows that row as "No access to organization details". The page does not
   fail.
4. It shows an **"All organizations"** section when
   `can(session, { scopePrn: <root PRN>, roleKey: 'platform_admin' })` is true. The section calls
   `ListOrganizations`. If IAM denies it, the section shows the 403 inline.

### 5.2 URLs and data

PRNs hold no slug (`rs/crates/libs/paigasus-kernel/src/resource_name.rs:2-4`), so URLs use UUIDs.

| Path | Reads | Mutations (Server Actions) |
|---|---|---|
| `/iam/orgs` | Introspect, `GetOrganization` ×N, `ListOrganizations` | create organization |
| `/iam/orgs/[org]` | `GetOrganization`, `ListTeams`, `ListMemberships(node_prn)` | create team, attach, detach |
| `/iam/orgs/[org]/teams/[team]` | `GetTeam`, `ListProjects`, `ListMemberships(node_prn)` | create project, attach, detach |
| `/iam/orgs/[org]/teams/[team]/projects/[project]` | `GetProject`, `ListMemberships(node_prn)` | attach, detach |
| `/iam/audit` | `ListAuditEntries` | — |

- Memberships appear as a section on each node page. That is the only place where IAM lets a
  non-admin list them (`ListMemberships` by `node_prn` checks the node).
- Tenancy lists page with `?offset=`, 50 per page (`limit`/`offset`, server maximum 200). The
  responses carry no total, so "Next" appears only when a page is full. The audit list pages with
  `cursor`/`next_cursor`.
- A URL whose `[team]` does not belong to its `[org]` renders `notFound()`. The page compares the
  team's `org_prn` with the URL, and IAM remains the authority on access.
- Each page is an async server component that calls a loader function in the same route folder.
  Loaders are plain async functions, so tier-2 tests call them directly (§ 9.3).

### 5.3 Mutations

- Each form is a small client component with `useActionState`. It calls a Server Action in the
  route folder's `actions.ts`.
- The action validates its input with zod, calls IAM through `callIam`, and on success calls
  `revalidatePath` for the page.
- The action returns a plain `{ ok: true } | { ok: false, error: PaigasusError }` state.
  `PaigasusError` is a plain object, so it crosses the Flight boundary
  (`ts/packages/paigasus-sdk/src/errors/types.ts:39-44`).
- Each form's button is hidden when `can()` returns false for the role that the starter policies
  need (`org_admin` at the org for create team, `team_admin` or `org_admin` for create project,
  the node's admin role for attach and detach, `platform_admin` for create organization). If a
  user gets past the hidden button, IAM still denies the call (§ 6).

### 5.4 The shell

- `(console)/layout.tsx` calls `requireSession(authRuntime())` and `getPublicConfig()`. It renders
  `SessionProvider` (with `toSessionView` output only, never a token), then
  `ZoneProvider zone zones`, then `AppShell`. This is the first real Flight handoff of
  `getPublicConfig().zones` to `ZoneProvider` (SMA-510 spec § 10.4).
- `lib/nav.ts` exports `buildNavEntries(states)`, the one function that makes `PrimaryNav`
  entries. Every entry's `state` comes from `navStateOf()`. This closes SMA-510 § 10.4's second
  gap: the app passes `NavEntryState` and never a raw `ServiceState`. The entries:
  - **Organizations** — `navStateOf(iam)`.
  - **Audit** — `navStateOf(iam, 'iam.audit')`.
  - **Gateway** — a cross-zone entry, `navStateOf(gateway)`. It is absent unless `gateway` is in
    both `PAIGASUS_ZONES` and `PAIGASUS_SERVICES`; it is disabled with a reason when the gateway
    is degraded.
- The node pages render `Breadcrumbs`.
- An organization switcher lists the user's organizations (from § 5.1) and marks the `[org]`
  URL segment as current. A small client wrapper reads `useParams()`, because a layout does not
  receive the params of child segments. The selection lives in the URL (SMA-510 spec § 4, D1).

---

## 6. Errors, the 403 view and capability gating

### 6.1 One call wrapper

`lib/errors.ts` exports `callIam(fn)`. It runs one SDK call, catches a `ConnectError`, and returns
the `PaigasusError` from `mapError`. The app branches only on `presentation`, `domain` and
`reason`, and never on message text (ADR-0019 E8).

| `presentation` | Page read | Section read | Server Action |
|---|---|---|---|
| `forbidden` | `forbidden()` → the 403 view, HTTP 403 | inline `<Forbidden>` | inline form error |
| `not-found` | `notFound()` | inline "not found" row | inline form error |
| `relogin` | `ErrorState` + a "Sign in again" link | same | same |
| `invalid-input`, `conflict` | — | — | inline error, copy by `reason` |
| `disabled` | `EmptyState` "This feature is not enabled on this IAM" | same | same |
| `degraded`, `rate-limited`, `generic` | `ErrorState` + correlation id | same | same |

Uncaught exceptions go to `(console)/error.tsx`.

### 6.2 The 403 view (AC 2)

- The view uses Next's `forbidden()` and a `forbidden.tsx` file, so the HTTP status is a real 403.
  In Next 16.3.4, `forbidden()` throws unless `experimental.authInterrupts` is true
  (`next/dist/client/components/forbidden.js:27`). The app sets it through
  `createNextConfig({ extend: { experimental: { authInterrupts: true } } })`.
- `forbidden.tsx` sits in `(console)/`, so that the shell stays visible. The plan measures
  whether Next 16.3.4 renders a nested `forbidden.tsx`. If it does not, the file moves to the
  app root, and the spec records the measurement.
- The view shows a fixed title, the correlation id (ADR-0019 E5) and a link to `/iam/orgs`. It
  never shows IAM's message.

**Risk.** The flag is experimental. An e2e test asserts the HTTP 403 status (§ 9.4), so a Next
upgrade that changes the flag fails CI and does not change behavior in silence. The fallback is
an inline view with status 200. It is recorded here and not built.

### 6.3 The UI does not pre-judge (AC 2)

`can()` only decides whether a button or a section appears. No code skips an IAM call because
`can()` returned false. The e2e tests prove both directions (§ 9.4).

### 6.4 No login loop on `relogin`

`getSession()` refreshes a near-expiry token before each render. So when IAM still returns
"unauthenticated", the cause is a wrong configuration (for example a wrong audience) or a revoked
token. An automatic redirect to login would then loop. The app shows a "Sign in again" link to
`/iam/auth/login?returnTo=<path>` instead, so each new attempt needs a click. The login route
clears the stale cookie itself.

### 6.5 Error copy

- One table maps each `presentation` to its copy.
- A second table maps the `reason` values that the five forms can get (for example
  `slug-conflict`) to their copy. A test asserts that every `reason` in it exists in the
  `ErrorReason` registry, so a renamed code fails a test.
- An unknown reason shows generic copy and the correlation id (the version-skew rule).

### 6.6 Capability gating (AC 4)

- The Audit nav entry uses `navStateOf(iam, 'iam.audit')`: absent when IAM does not report the
  capability, available when it does, and disabled with a reason when IAM is degraded.
- The `/iam/audit` page calls `hasCapability('iam.audit', token)` on the server and returns
  `notFound()` when it is false. A degraded state is always false
  (`ts/packages/paigasus-discovery/src/server.ts:136`), so the page never calls an endpoint that
  may not exist. A typed URL does not reach the screen.

---

## 7. Changes to packages

### 7.1 The Turbopack import fix

Next 16.3.4's Turbopack does not resolve a `.js` relative specifier to a `.ts` file (CLAUDE.md,
measured in SMA-510). Four packages that this app compiles use such specifiers:

| Package | `.js` relative imports | Change |
|---|---|---|
| `@paigasus/auth` | 73 in `src/` (all three entries) | extensionless |
| `@paigasus/sdk` | 29 in `src/` (all entries except `./errors/types`) | extensionless |
| `@paigasus/discovery` | 43 in `src/` (`/server`, `/react`; `/client` is already done) | extensionless |
| `@paigasus/proto` | 34 in `src/` (hand-written and generated) | hand-written: extensionless; generated: see below |

- Only `src/` changes. The test files keep their `.js` imports; vitest resolves both forms, and
  the diff stays smaller. All packages use `moduleResolution: bundler`
  (`ts/tsconfig.base.json`), so extensionless imports type-check everywhere.
- **Generated proto code.** `contracts/buf.gen.yaml` passes `import_extension=.js` to
  `buf.build/bufbuild/es:v2.13.0`. The plan measures the plugin's other values and removes or
  changes the option so that the output is extensionless, then regenerates. The inline codegen
  drift step in `ci.yml` checks the committed output.
- **Plain-Node harness.** `@paigasus/auth`'s e2e fixture server runs the package source under
  plain Node through `tests/fixtures/ts-esm-loader.mjs`, which retries `.js` as `.ts`. Plain Node
  does not probe extensions, so the loader gains an "extensionless → `.ts`, then `/index.ts`"
  retry. The plan runs `paigasus-auth-ts:test-e2e` to prove it. The plan also finds every other
  harness that runs package source under plain Node, and extends each one in the same way.
- **Enforcement.** A new ESLint rule, `paigasus/no-js-relative-specifier`, lives in
  `@paigasus/next-config/eslint` and applies to `packages/*/src/**`, test files excluded. It
  reports an `import`, `export … from` or `import()` whose specifier starts with `./` or `../` and
  ends in `.js`. It must be a **custom rule with its own name**, not another
  `no-restricted-imports` entry: in flat config, a second `no-restricted-imports` block that
  matches the same files **replaces** the first, which would switch off the `sdk`, `auth-*` and
  `discovery` boundary rules in silence. (The `app-shell-fixture` rule copies the full allowlist
  for this reason.)
- ESLint ignores `**/generated/**`, so a proto test asserts that no file under `src/generated/`
  holds a `.js` relative specifier.
- The CLAUDE.md entry on this Turbopack limit is updated to say that the packages are fixed and
  what holds the fix.

### 7.2 The proxy boundary

`paigasus/boundaries/app-middleware` (`ts/packages/paigasus-next-config/src/eslint.mjs:288-302`)
matches `apps/**/middleware.{ts,js,mts,cts,mjs,cjs}` only. Its glob changes to
`apps/**/{middleware,proxy}.{ts,js,mts,cts,mjs,cjs}`. `apps/iam-console/proxy.ts` then makes the
rule live, and the `BOUNDARY_SCOPES` liveness test checks it.

`proxy.ts` imports only `@paigasus/auth/middleware`. Its `publicPaths` are `authRoutePaths({
basePath })` plus `/iam/signed-out`, and `loginPath` is `/iam/auth/login`. It checks cookie
presence only (ADR-0017 decision 7).

### 7.3 Tailwind

- `globals.css` gets `@source '../../../packages/paigasus-app-shell/src'` next to the existing
  `@paigasus/ui` line.
- A third sentinel, `--paigasus-app-shell-source-probe`, goes into one app-shell component, in the
  same way as the ui probe in `table.tsx`.
- `ci/tailwind-source/run.mjs` asserts it in all three modes (the real run, `--self-test` and
  `--negative-control`).

### 7.4 The SDK client-boundary fixture (SMA-508 AC 1, moved here)

- A minimal Next app at `ts/apps/iam-console/tests/fixtures/client-imports-sdk/` holds one
  `'use client'` component that imports `@paigasus/sdk/iam`.
- A test runs `next build` on it and asserts a non-zero exit and the `server-only` error text.
- A **positive control** builds the same fixture with the import in a server component, and
  asserts success. This proves that the failure comes from the boundary, not from a broken
  fixture.
- No ESLint rule can do this job, because rules select files by path and not by the
  `'use client'` directive.

---

## 8. Moon, CI and dependencies

- `iam-console-ts` adds `dependsOn` entries and task `inputs` for the `src/**` and `package.json`
  of `auth`, `sdk`, `discovery`, `app-shell` and `proto`, in `build`, `test` and the new
  `test-e2e`. The `build` and `test` tasks use `options.merge: replace`, so they list every input
  by hand.
- The new `iam-console-ts:test-e2e` task (`cache: false`) is picked up by the existing
  `:test-e2e` CI target, so `ci.yml` does not change for it.
- `ci/affected-graph/run.sh`: the `auth`, `sdk`, `discovery`, `app-shell`, `proto` and `ui` cases
  add `iam-console-ts:build`, `:test` and `:test-e2e` to their expected sets, and a new
  `app-shell->console` case anchors on the file that holds the new sentinel.
- `msw` goes into the pnpm catalog, at a version released more than 24 hours before the commit
  (pnpm 11 `minimumReleaseAge`).
- The app gets `@paigasus/auth`, `sdk`, `discovery`, `app-shell`, `redis` and `zod`. The tests use
  `@connectrpc/connect-node`, `jose` and `@playwright/test`, which are already in the catalog.
- The full CI target list runs before the push (CLAUDE.md), including `:osv`, `:deny`,
  `:next-public-free`, `:next-env-drift` and `:affected-smoke`.

---

## 9. Testing

No tier needs a live service or Docker.

### 9.1 Test doubles

- **Fake IAM (gRPC).** A real `@connectrpc/connect-node` HTTP/2 server on an ephemeral port, with
  scripted `TenancyService`, `AuthnService` and `AuditService` handlers per test. The SDK uses its
  real gRPC transport against it. A denial is a `ConnectError` with `Code.PermissionDenied` and an
  `ErrorInfo(domain: iam, reason: "forbidden", metadata: { retryable: "false" })` detail, the same
  shape as `status_to_grpc` (`rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:111-131`).
- **Fake IAM (HTTP).** `GET /v1/service-info`. In vitest, MSW serves it. In e2e, a plain
  `node:http` server serves it on a second port, because MSW cannot reach into the standalone
  server process.
- **Fake IdP.** An in-process `node:http` server with `jose`: discovery document, JWKS,
  `/authorize` (approves at once and redirects with a code), and `/token` with PKCE checks. It is
  based on `ts/packages/paigasus-auth/tests/fixtures/jwks.ts`, copied into `tests/support/`,
  because one package's tests must not import another package's tests.

### 9.2 Tier 1 — unit (vitest)

- `buildNavEntries`: all three IAM states, with and without `iam.audit`; the gateway absent,
  degraded and available; every entry went through `navStateOf()`.
- `callIam`: the § 6.1 table. `forbidden()` and `notFound()` throw special errors, so the test
  asserts their digests.
- The error copy tables against the `ErrorReason` registry (§ 6.5).
- Config: `PAIGASUS_IAM_GRPC_URL` validation, and the parse fails without `iam` in
  `PAIGASUS_SERVICES`.
- The logger adapter writes only the port's fields.
- `proxy.ts`: `publicPaths` contain `authRoutePaths()` and `/iam/signed-out`.
- The org-UUID collection of § 5.1 step 2, over org, team and project PRNs.

### 9.3 Tier 2 — integration (vitest, fake IAM + MSW)

- The loaders and Server Actions of § 5 against scripted IAM answers.
- **The ErrorInfo trailer round trip.** The fake IAM returns the denial of § 9.1, and the test
  asserts `presentation: 'forbidden'`, the reason and the correlation id. The SDK's own tests
  build the `ConnectError` in memory and state that they do not test the wire
  (`ts/packages/paigasus-sdk/tests/map-error.test.ts:10-15`); this test does.
- `IntrospectPrincipalResolver`: a scripted Introspect maps to the port's types; a failed call
  degrades to `grantsAvailable: false` and logs `principal.introspect_failed`.

### 9.4 Tier 3 — e2e (Playwright, `iam-console-ts:test-e2e`)

A **production build** runs through the standalone `server.js`, as in the app-shell e2e, because
`next dev` prefetches differently. The session store is `memory`. Short
`PAIGASUS_DISCOVERY_*_MS` values let a scenario change the IAM state.

| Scenario | AC |
|---|---|
| Unauthenticated `/iam/orgs` → proxy redirect → IdP → callback → "Your organizations" lists the orgs from the fake Introspect | 1 |
| A denied page read → **HTTP 403** and the 403 view, inside the shell | 2 |
| `can()` true (grants unavailable) and IAM denies → the 403 renders | 2 |
| `can()` false → the create button is hidden, and the reads still render | 2 |
| A denied Server Action → an inline 403 in the form | 2 |
| `iam.audit` reported → the Audit entry appears and the page works | 4 |
| `iam.audit` not reported → the entry is absent, and `/iam/audit` returns 404 | 4 |
| IAM degraded → the Audit and Organizations entries are disabled with a reason | 4 |
| `PAIGASUS_ZONES` has `gateway` → the Gateway entry is a plain `<a href="/gateway">`, and Organizations is a Next link | shell |
| Sign out → POST `/iam/auth/logout` → `/iam/orgs` redirects to login again | 1 |

### 9.5 AC 3

- The `build` script asserts `.next/standalone/apps/iam-console/server.js`.
- `tests/standalone-runtime.test.ts` stays, with the renamed paths.
- `repo:next-public-free` stays green, and the e2e tier runs against the standalone server.
- The client-boundary fixture of § 7.4 runs in `iam-console-ts:test`.

---

## 10. Things to measure in the plan

The design depends on these facts. The plan measures each one before it relies on it.

1. The auth route handler under `basePath`: `createAuthRoutes` matches `new URL(req.url).pathname`
   against `${basePath}/auth/...` (`http/routes.ts:86-92`). Does `req.url` in an App Router route
   handler keep `/iam`? If it does not, the mount changes.
2. A nested `forbidden.tsx` in a route group renders on `forbidden()` (§ 6.2).
3. The protobuf-es `import_extension` values (§ 7.1).
4. `proxy.ts` runs with the `react-server` condition like `middleware.ts`, so the
   `app-middleware` rule's premise holds (`server-only` is a no-op there).
5. Every harness that runs package source under plain Node (§ 7.1).

---

## 11. Follow-ups

Linear issues, made after the spec is approved:

1. **The dead-letters capability key and screen.** The contracts registry, IAM `Capabilities`,
   the discovery vocabulary, and `/iam/dead-letters`. AC 4's dead-letters half moves there.
2. **Rename, archive and restore** for organizations, teams and projects (7 mutations).
3. **A shared home for `IntrospectPrincipalResolver`** when SMA-512 needs it.

Recorded limits, no issue:

- The `authInterrupts` fallback (§ 6.2).
- The login-time grants snapshot (§ 4.5).
