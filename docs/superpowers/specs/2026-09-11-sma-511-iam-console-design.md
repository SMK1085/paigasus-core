# SMA-511 — `iam-console`: the IAM zone

**Status:** approved design, revision 2 (after adversarial challenge)
**Date:** 2026-09-11
**Issue:** [SMA-511](https://linear.app/smaschek/issue/SMA-511/ts-iam-console-app-the-iam-zone)
**ADR:** ADR-0017 — Console topology & session ownership; ADR-0005 (kernel owns PRN logic);
ADR-0019 (error model, E5/E8); ADR-0020 (capability discovery)
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
the 403 view, and capability-gated screens. It is also the first time that the shared packages
run inside a Next app with a `basePath`, and § 7 fixes what that exposes.

### 1.1 Acceptance criteria (from the issue)

1. A user logs in through this zone and lands on a tenancy screen.
2. A Cedar denial renders the 403 view correctly. The UI does not pre-judge authorization:
   `can()` only hides affordances, and the server decision is authoritative.
3. The app builds to a standalone image with no `NEXT_PUBLIC_` usage (the SMA-502 gate is green).
4. Capability-gated screens (audit, dead-letters) appear only when IAM reports the matching
   capability.
5. MSW-backed app tests run without live services.

§ 2.2 records how this design changes AC 2, AC 3, AC 4 and AC 5, and why.

---

## 2. Scope and decisions

### 2.1 Decisions (Sven, 2026-09-11)

| # | Question | Decision |
|---|---|---|
| D1 | Work outside the app | This PR holds the Turbopack import fix (§ 7.1) and the Introspect principal resolver (§ 4.5). A new issue holds the dead-letters capability key and its screen. |
| D2 | App name, and `paigasus-docs` | Rename to `ts/apps/iam-console` (`@paigasus/iam-console`, Moon id `iam-console-ts`). Delete the `paigasus-docs` stub. |
| D3 | Test harness for AC 5 | In-process fake servers plus MSW (§ 9). No Docker. |
| D4 | Mutations | Create organization, create team, create project, attach membership, detach membership. |
| D5 | How the UI decides which affordances to show | `IsAuthorized` self-queries at render time (§ 6.3), not role grants in the session. |
| D6 | PRN parsing, after the kernel napi binding failed to load in a Next build (§ 4.7) | Spike the kernel's wasm binding on the server first. If it fails, use a small app PRN reader checked against the kernel parity corpus, without asking again. |

### 2.2 How the acceptance criteria change

- **AC 2 — `can()`.** IAM's `Introspect` always returns an empty `role_grants`
  (`rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:161-165`;
  `adapters/http/dto.rs:242`: "empty until a later M3 task populates it"). A `can()` over a session
  grants snapshot therefore has no data. Per D5, the app's affordance check is `mayI()`, which asks
  IAM's `IsAuthorized` about the current principal (§ 6.3). It is cosmetic in the same way as
  `can()`. This departs from ADR-0017 decision 8's wording ("`can()` reads role grants from the
  session"); § 12 records the departure.
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
  calls go to a real in-process fake server, and MSW intercepts the `fetch` calls.

After the spec is approved, the AC text in Linear is updated for AC 2, 3, 4 and 5, each with its
reason from this section.

### 2.3 In scope

- The rename and the deletion (§ 3).
- The composition root (§ 4), the screens (§ 5), the error handling and affordances (§ 6).
- `@paigasus/auth` under a `basePath` (§ 7.1), the Turbopack import fix (§ 7.2), and small
  changes to `sdk`, `next-config` and `app-shell` (§ 7.3–7.6).
- The Moon and affected-graph changes (§ 8), three test tiers (§ 9), and the deployment contract
  for SMA-513 (§ 10).

### 2.4 Out of scope

- The console OCI image and the ingress (SMA-513).
- A real two-zone test (SMA-513's harness; see § 9.4).
- The dead-letters capability key and screen, and rename/archive/restore (follow-ups, § 11).
- Policy and role-grant administration, service accounts and API keys.
- A user search (IAM has no user lookup RPC), a search in the switcher, and i18n.

---

## 3. Structure and the rename

### 3.1 Rename

`git mv ts/apps/paigasus-console ts/apps/iam-console`. The package name becomes
`@paigasus/iam-console` and the Moon id becomes `iam-console-ts`. Reason: SMA-512 adds a gateway
zone app, and `paigasus-console` then no longer says which zone it is.

Every site that names the old path or id changes in the same commit. The list below comes from
`git grep -n paigasus-console` on this branch's base; the plan re-runs the grep and treats any new
hit as a site to change:

| Site | Change |
|---|---|
| `ts/apps/iam-console/moon.yml` | id, the `server.js` path check, the `sources` group (§ 8) |
| `ts/apps/iam-console/tests/standalone-runtime.test.ts` | paths, and the new target (§ 9.5) |
| root `moon.yml`, `repo:next-env-drift` | `deps` and four `inputs` paths |
| `ci/tailwind-source/run.mjs:17`, `ci/tailwind-source/README.md` | `CONSOLE_DIR`, prose |
| `ci/affected-graph/run.sh` | every `paigasus-console-ts` target |
| `ts/eslint.config.js:57-64` | the Next rules `files` glob and `settings.next.rootDir` |
| `ts/packages/paigasus-next-config/tests/boundaries.test.ts` | rows at `:75-76`, `:109-110`, `:150-152`, `:165`, `:281` |
| `ts/packages/paigasus-auth/tests/config.test.ts` | the app path it names |
| `ts/packages/{discovery,sdk,ui,app-shell}/moon.yml` | comments or inputs that name the app |
| `ts/packages/paigasus-ui/{tsconfig.json,README.md,src/components/table.tsx}` | path mentions |
| `ts/README.md`, `CONTRIBUTING.md:212`, `CLAUDE.md` | prose |

`docs/superpowers/**` keeps the old name, because those files record history.

### 3.2 Delete `ts/apps/paigasus-docs`

It holds only `export {}`, and nothing depends on it. The pnpm lockfile loses its workspace entry,
and `ts/README.md` loses its row. A docs app gets its own issue when a framework is chosen.

### 3.3 App layout

```
ts/apps/iam-console/
  proxy.ts                         ← Next 16 file name; createAuthMiddleware + config.matcher
  app/
    layout.tsx                     ← html/body and globals.css only
    globals.css
    error.tsx, global-error.tsx    ← boundaries for the root and for errors in child layouts
    healthz/route.ts               ← public: { zone, zones } from getPublicConfig()
    auth/[...auth]/route.ts        ← async GET/POST → createAuthRouteHandler(await authRuntime())
    (public)/layout.tsx            ← ZoneProvider, `await connection()`
    (public)/page.tsx              ← `/iam/`: PublicShell, or redirect('/orgs') when the cookie is present
    (console)/layout.tsx           ← requireSession → providers → AppShell
    (console)/forbidden.tsx        ← the 403 view (§ 6.2)
    (console)/error.tsx
    (console)/orgs/…               ← tenancy screens (§ 5)
    (console)/audit/page.tsx       ← capability-gated
  lib/                             ← composition root; every file `import 'server-only'`
    config.ts  auth.ts  iam.ts  discovery.ts  principal.ts  principal-resolver.ts
    authorize.ts  errors.ts  nav.ts  prn.ts  logger.ts
  tests/
    unit/  integration/  e2e/  support/  fixtures/client-imports-sdk/
```

- `proxy.ts` replaces `middleware.ts`, because Next 16.3.4 deprecates the old name
  (`next/dist/esm/server/lib/router-utils/setup-dev-bundler.js:288` prints the warning).
- `app/page.tsx` is deleted. `(public)/page.tsx` owns `/iam/`. The Tailwind proof does not need a
  page that renders `Table`, because `@source` scans the package source.
- The `(console)` route group holds every page that needs a session. Its layout calls
  `requireSession()`. **This does not guard Server Actions**: each action gets its token through
  `iamClient()`, which calls `requireSession()` itself (§ 5.3).
- `(public)/page.tsx` checks only that the session cookie exists, and does not resolve the
  session. So it needs no IdP, and a stale cookie reaches `requireSession()` in the console layout,
  which handles it. `@paigasus/auth` redirects `idp_error` and the default post-logout URI to
  `/iam/` (`server.ts:68-69`, `runtime.ts:123`), so both land on this public page.
- The style is functional, to match Next and React. Dependencies enter through small ports that
  loaders and actions receive as arguments (§ 5.2); only the page and action shells read `lib/`.

---

## 4. The composition root (`lib/`)

Every file in `lib/` starts with `import 'server-only'`, so a client component that imports one
fails the build. No accessor reads the config at module scope.

### 4.1 `lib/config.ts`

The app's one `defineRuntimeConfig` call composes three parts:

- `authEnvShape` from `@paigasus/auth/server`;
- `discoveryEnvShape` from `@paigasus/discovery/server`;
- one app-owned key, `PAIGASUS_IAM_GRPC_URL`: an absolute `http:` or `https:` URL, with no
  credentials, no query and no fragment.

`PAIGASUS_IAM_GRPC_URL` is a separate key because IAM listens on two addresses: HTTP on `8080` and
gRPC on `9090` (`rs/crates/services/paigasus-iam/src/config.rs:812-813`). Discovery's
`PAIGASUS_SERVICES` holds the HTTP address, which discovery uses for `GET /v1/service-info`.

`PAIGASUS_SERVICES` must contain `iam`, so a missing `iam` key fails the parse at the first
request. `getRuntimeConfig()` throws during `phase-production-build`, so a module-scope read fails
the build loudly.

### 4.2 `lib/auth.ts`

`authRuntime()` returns `getAuthRuntime(getRuntimeConfig(), { resolver, logger })`, a process
singleton that returns a Promise (`runtime.ts:178`). The route handler is an async function that
awaits it on each request, and never builds the runtime at module scope.

### 4.3 `lib/iam.ts`

`iamClient(Service)` calls `requireSession(await authRuntime())`, then
`createIamClient(Service, { baseUrl: PAIGASUS_IAM_GRPC_URL }, { bearer: session.accessToken })`.
React `cache()` memoizes it for one request only. No client and no token lives past the request
(`ts/packages/paigasus-sdk/src/iam.ts:19-24`). Only the SDK's transport is process-scoped.

### 4.4 `lib/discovery.ts`

`discovery()` makes one `Discovery` handle per request, with React `cache()` and
`waitUntil: after`.

- When `PAIGASUS_SESSION_STORE=redis`, the descriptor cache uses the same Redis URL as the session
  store, so an operator configures one Redis. The node-redis client is a **lazy process
  singleton**. It is made with all four preconditions that `createRedisDescriptorCache` asserts
  (`redis-cache.ts:67-90`): `disableOfflineQueue: true`, an `error` listener,
  `commandOptions.timeout` and `socket.socketTimeout`. The listener never logs the error object,
  because node-redis embeds the DSN in it.
- `connect()` runs once, on first use. Its rejection is caught and logged as the app event
  `discovery.redis_connect_failed`, with no DSN. The cache then fails fast, and discovery reports
  its own `cache-unavailable` degraded reason. The app does not fall back to a memory cache in
  silence.
- When `PAIGASUS_SESSION_STORE=memory`, discovery uses the memory cache.

### 4.5 `lib/principal.ts` and `lib/principal-resolver.ts`

**Provisioning.** IAM's `Introspect` is exempt from bearer enforcement
(`adapters/grpc/authn.rs:139-141`) and runs with `Provisioning::Disabled`
(`application/authenticate_token.rs:104-105,146-147`). For an identity that IAM has never seen, it
returns `PermissionDenied` with reason `identity-not-provisioned` (`convert.rs:142`). IAM
provisions a principal (JIT), and seeds the bootstrap `platform_admin` grant, only inside a
bearer-enforced RPC (`authn.rs:182-190`). So the first call must be a bearer-enforced one.

`ServiceInfoService.GetServiceInfo` is the provisioning call: it is bearer-enforced and checks no
Cedar action (`adapters/grpc/service_info.rs:1-44`). No `WhoAmI` RPC exists; § 11 records one as a
follow-up.

**`IntrospectPrincipalResolver`** (`lib/principal-resolver.ts`) implements auth's
`PrincipalResolver` port, which the login callback calls once (`http/routes.ts:216`):

1. `GetServiceInfo` with the new access token as the bearer (this provisions the principal).
2. `Introspect` with the token in `IntrospectRequest.token`, the field that IAM reads
   (`authn.rs:59`).
3. It maps `principal_prn`, `issuer`, `subject` and `memberships` to the port's domain types. It
   sets `roleGrants: []` and `grantsAvailable: false`, because IAM reports no grants here, and
   the port says to treat that as "unknown" (`ports/principal-resolver.ts:32-36`).

Each call has a short `timeoutMs` (3 s), not the SDK's 10 s default (`transport.ts:42`), because
the user waits on the login callback. The whole sequence, the mapping included, runs inside one
`try`. On any failure the resolver **does not fail the login**: it returns `principalPrn: null`,
empty lists and `grantsAvailable: false`, and logs the app event `principal.resolve_failed` with the
`presentation` value. That event goes through the app logger's own API, because `AuthEventName` is
a closed union (`ports/logger.ts:14-25`).

The resolver lives in the app because `@paigasus/auth` must not import `@paigasus/sdk`
(`ports/principal-resolver.ts:3-8`), and the `sdk` boundary rule bans every `@paigasus/*` import
except `proto`. SMA-512 will need the same code; § 11 records that.

**`currentPrincipal()`** (`lib/principal.ts`) is what the pages use. It is a per-request
`cache()` of a **live** `Introspect`. If `Introspect` returns `identity-not-provisioned`, it calls
`GetServiceInfo` and retries `Introspect` once. The pages never use the login snapshot, so a
degraded login does not stay degraded for the session, and a membership change appears on the
next render.

### 4.6 `lib/authorize.ts` — `mayI()`

`mayI(action, resourcePrn)` calls `AuthorizationService.IsAuthorized` with
`principal_prn = currentPrincipal().prn`. A principal may always ask about itself
(`application/authorize.rs:75-80`), and `IsAuthorized` has no capability gate
(`adapters/grpc/authz.rs:81-91`). Cedar evaluates the request, so the result follows the resource
hierarchy (`resource in ?resource`, `roles.rs:324-338`) and any custom policy exactly. The app
holds **no table of which role grants which action**. That table would copy the starter policies,
and it would be wrong for policies added with `PutPolicy`.

- Results are memoized per `(action, resource)` for one request.
- A failed query returns `true` (fail open) and logs `authorize.query_failed`. A button that should
  be hidden then shows, and IAM still denies the action.
- The plan measures the `action` string format that `Action::parse` accepts.

### 4.7 `lib/prn.ts`

PRN parsing and building should use `@paigasus/kernel` (`prnBuild`, `prnOrg`, `prnResourceType`,
`prnResourceId`), because ADR-0005 keeps cross-language behavior in the kernel.

**Measured (2026-09-11): the napi binding cannot load in a Next build.** Under the `node`
condition the kernel imports `@paigasus/node-bindings`, a pnpm `file:` dependency whose `files`
allowlist is `["index.js", "index.d.ts"]`. pnpm therefore never copies the `.node` binary into
`node_modules`, and `next build` fails at "Collecting page data" with `Cannot find native binding`
(`Cannot find module '@paigasus/node-bindings-darwin-arm64'`). The wasm entry
(`src/wasm.ts`, over the committed `--target bundler` output in `rs/crates/bindings/paigasus-wasm/`)
has no subpath export, so a server cannot reach it.

**Decision D6.**
1. **Spike first.** `@paigasus/kernel` gains a `./wasm` subpath export for `src/wasm.ts`. The spike
   measures that Turbopack bundles the wasm into the standalone server and that `prnBuild`,
   `prnOrg`, `prnResourceType` and `prnResourceId` return correct values from a route handler.
   Wasm is platform-neutral, so SMA-513's image then needs no native binary.
2. **Fallback C, if the spike fails.** `lib/prn.ts` holds a small reader for the IAM tenancy shapes
   only (resource type, org UUID, resource UUID, and building an organization, team or project
   PRN). A test runs it against every IAM-tenancy vector in the kernel parity corpus
   (`rs/crates/libs/paigasus-kernel-parity/vectors/`), so a divergence from the kernel fails CI.
   This is a recorded ADR-0005 exception (§ 12), and the `./wasm` export is not added.

`lib/prn.ts` has the same interface in both cases, so no caller changes between them.

### 4.8 `lib/logger.ts`

One JSON-lines adapter writes to stdout. It implements `AuthLogger` and `DiscoveryLogger`, and
writes only the fields that each port gives; the ports already redact them. It also exposes
`appEvent(name, fields)` for the app's own events (`principal.resolve_failed`,
`authorize.query_failed`, `discovery.redis_connect_failed`). This is the first real logger adapter
that SMA-626 item 6 expects.

---

## 5. Screens, URLs and navigation

### 5.1 The landing screen (AC 1)

Only `platform_admin` can call `ListOrganizations`, and only `platform_admin` can call
`ListMemberships` filtered by principal. Both check against the root PRN
(`adapters/grpc/tenancy.rs:193`, `:669-676`; `roles.rs:95-96`). Also, memberships are not grants:
`CreateOrganization` gives the creator an `org_admin` grant but no membership
(`application/organizations.rs:129-131,156-164`), and a user can hold a team or project role with
no organization access.

The login ends at `/iam/`, which redirects a signed-in user to `/iam/orgs`, **"Your
organizations"**. One per-request loader, `myScopes()`, gives both this page and the switcher their
data:

1. The **scopes** are the union of `currentPrincipal().memberships[].nodePrn` and, when IAM reports
   the `iam.authz.cedar` capability, the `scope_prn` of `ListRoleGrants(principal_prn = me)`. A
   principal may always list its own grants (`application/roles.rs:307-318`), but the RPC needs the
   capability (`adapters/grpc/authz.rs:85-91,227-228`). Without it, the page lists memberships only
   and says so in one line.
2. Scopes are deduplicated and grouped by organization with `prnOrg()`/`prnResourceType()`. The
   page shows at most 50 scopes and states how many more exist.
3. Each organization row calls `GetOrganization`. A team scope links straight to
   `/iam/orgs/<org>/teams/<team>`, because both UUIDs are in its PRN, and takes its label from
   `GetTeam`. A project scope calls `GetProject`, which carries `team_prn` and `org_prn`
   (`convert.rs:310-326`), and links straight to the project.
4. A denied row shows "No access to details" with the PRN, and the page does not fail. A user with
   only a team role therefore sees the team as a direct link, not a dead organization row.
5. An **"All organizations"** section appears when `mayI('ListOrganizations', <root PRN>)` is
   true. It calls `ListOrganizations`. If IAM denies it anyway, the section shows the 403 inline.

### 5.2 URLs and data

PRNs hold no slug (`rs/crates/libs/paigasus-kernel/src/resource_name.rs:2-4`), so URLs use UUIDs.
A URL segment that is not a UUID renders `notFound()`.

| Path | Reads | Mutations (Server Actions) |
|---|---|---|
| `/iam/orgs` | `myScopes()`, `ListOrganizations` | create organization |
| `/iam/orgs/[org]` | `GetOrganization`, `ListTeams`, `ListMemberships(node_prn)` | create team, attach, detach |
| `/iam/orgs/[org]/teams/[team]` | `GetTeam`, `ListProjects`, `ListMemberships(node_prn)` | create project, attach, detach |
| `/iam/orgs/[org]/teams/[team]/projects/[project]` | `GetProject`, `ListMemberships(node_prn)` | attach, detach |
| `/iam/audit` | `ListAuditEntries` | — |

- Memberships appear as a section on each node page. That is the only place where IAM lets a
  non-admin list them.
- **Consistency.** A team page compares `GetTeam.org_prn` with `[org]`. A project page compares
  `GetProject.team_prn` and `.org_prn` with `[team]` and `[org]`. A mismatch renders `notFound()`.
  IAM remains the authority on access.
- Tenancy lists page with `?offset=`, 50 per page (`limit`/`offset`, server maximum 200). The
  responses carry no total, so "Next" appears only when a page is full. The audit list pages with
  `cursor`/`next_cursor`.
- **Loaders take ports.** Each page is an async server component. It builds the clients from
  `lib/` and passes them to a loader function in the same route folder, for example
  `loadTeamPage({ tenancy, mayI }, params)`. Loaders are plain async functions that tier-2 tests
  call with fakes (§ 9.3).
- The attach form takes a principal PRN as text, because IAM has no user lookup RPC. The member
  lists show PRNs. This is a known limit of D4 (§ 12).

### 5.3 Mutations

- Each form is a small client component with `useActionState`. It calls a Server Action in the
  route folder's `actions.ts`.
- The action shell gets its client through `iamClient()` (so `requireSession()` runs for every
  action), validates the input with zod, and calls a command function with the client as a port.
  On success it calls `revalidatePath` for the page.
- The action returns `{ ok: true } | { ok: false, error: PaigasusError }`. `PaigasusError` is a
  plain object, so it crosses the Flight boundary (`ts/packages/paigasus-sdk/src/errors/types.ts:39-44`).
- A structure test asserts that every exported function in every `actions.ts` obtains its client
  through `iamClient()`.
- Server Actions keep Next's built-in origin check. § 10 states what the ingress must forward for
  that check to pass.

### 5.4 The shell

- `(console)/layout.tsx` calls `requireSession(await authRuntime())` and `getPublicConfig()`. It
  renders `SessionProvider` (with `toSessionView` output only, never a token), then
  `ZoneProvider zone zones`, then `AppShell`. This is the first real Flight handoff of
  `getPublicConfig().zones` to `ZoneProvider` (SMA-510 spec § 10.4).
- `lib/nav.ts` exports `buildNavEntries(states, allowed)`, the one function that makes
  `PrimaryNav` entries. Every entry's `state` comes from `navStateOf()`, which closes SMA-510
  § 10.4's second gap. The entries:
  - **Organizations** — `navStateOf(iam)`.
  - **Audit** — `navStateOf(iam, 'iam.audit')`. The layout also omits it when
    `mayI('ListAuditLog', <root PRN>)` is false, because `ListAuditEntries` is Root-only
    (`application/audit.rs:36-38`).
  - **Gateway** — a cross-zone entry, `navStateOf(gateway)`. It is absent unless `gateway` is in
    both `PAIGASUS_ZONES` and `PAIGASUS_SERVICES`, and disabled with a reason when the gateway is
    degraded.
- The node pages render `Breadcrumbs`.
- An organization switcher lists the organizations from `myScopes()` and marks the `[org]` URL
  segment as current. A small client wrapper reads `useParams()`, because a layout does not
  receive the params of child segments. The selection lives in the URL (SMA-510 spec § 4, D1).

---

## 6. Errors, the 403 view and affordances

### 6.1 One call wrapper

`lib/errors.ts` exports `callIam(fn)`. It runs one SDK call. It catches a `ConnectError` and
returns the `PaigasusError` from `mapError`. **It rethrows every other error unchanged**, so that
Next's `redirect`, `notFound` and `forbidden` errors and real bugs are not swallowed. The app
branches only on `presentation`, `domain` and `reason`, never on message text (ADR-0019 E8).

| `presentation` | Page read | Section read | Server Action |
|---|---|---|---|
| `forbidden` | `forbidden()` → the 403 view, HTTP 403 | inline `<Forbidden>` | inline form error |
| `not-found` | `notFound()` | inline "not found" row | inline form error |
| `relogin` | `ErrorState` + "Sign in again" | same | same |
| `invalid-input`, `conflict` | `ErrorState` + correlation id | same | inline error, copy by `reason` |
| `disabled` | `EmptyState` "This feature is not enabled on this IAM" | same | same |
| `degraded`, `rate-limited`, `generic` | `ErrorState` + correlation id | same | same |

`(console)/error.tsx` catches errors in the console pages, and `app/error.tsx` and
`app/global-error.tsx` catch errors in `(console)/layout.tsx` and the root layout, which a
segment's own `error.tsx` does not catch.

### 6.2 The 403 view (AC 2)

- The view uses Next's `forbidden()` and `(console)/forbidden.tsx`, so the HTTP status is a real
  403. `forbidden()` throws unless `experimental.authInterrupts` is true
  (`next/dist/client/components/forbidden.js:27`). The app sets it through
  `createNextConfig({ extend: { experimental: { authInterrupts: true } } })`.
- A nested `forbidden.tsx` is a per-segment boundary like `not-found.tsx`
  (`next/dist/server/app-render/create-component-tree.js:56,113-116,313-333`). The plan measures it
  in a real build.
- `forbidden()` takes no argument, so the view cannot receive the correlation id as a prop.
  **Measured (2026-09-11): a React `cache()` holder set before `forbidden()` is not visible in the
  `forbidden.tsx` render.** The plan tries one other way: `proxy.ts` mints a per-request id into a
  request header, `lib/iam.ts` sends it to IAM as `paigasus-correlation-id`, and the view reads it
  with `headers()`. This works only if IAM adopts an incoming id; the plan checks that in
  `paigasus-observability` first. If either step fails, the view shows no id, and `callIam` logs the
  id with the path (the fallback). The section and action 403s always show the id, because they
  render the `PaigasusError` directly.
- The view shows a fixed title, the correlation id and a link to `/iam/orgs`. It never shows IAM's
  message.

**Risk.** `authInterrupts` is experimental. An e2e test asserts the HTTP 403 status (§ 9.4), so a
Next upgrade that changes the flag fails CI. The fallback, an inline view with status 200, is
recorded here and not built.

### 6.3 Affordances, and "the UI does not pre-judge" (AC 2, D5)

- **An affordance** is a button, a form, a page section or a nav entry that exists only to start
  an action. `mayI()` may hide an affordance. A hidden affordance issues no call.
- **A user action** is a navigation to a page or a Server Action. **Every user action calls IAM.**
  No page and no action refuses anything because `mayI()` returned false. A user who types a URL,
  or posts to an action whose button is hidden, gets IAM's real answer.
- The tests prove both directions and count the calls (§ 9.3, § 9.4).

### 6.4 No login loop

- **`relogin`.** `getSession()` refreshes a near-expiry token before a render. So when IAM still
  returns "unauthenticated", the cause is a wrong configuration (for example the wrong audience;
  IAM checks `aud` per issuer, `adapters/oidc/validator.rs:198`) or a revoked token. An automatic
  redirect would loop. The app shows a "Sign in again" link to `/iam/auth/login?returnTo=<path>`,
  so each new attempt needs a click.
- **`returnTo`.** `validateReturnTo` accepts `/iam/auth/login` (`core/return-to.ts:19-26`), so a
  crafted link can loop. § 7.1 makes the login route reject a `returnTo` under the zone's `/auth/`
  paths.

### 6.5 Error copy

- One table maps each `presentation` to its copy.
- A second table maps the `reason` values that the five forms can get (for example
  `slug-conflict`) to their copy. A test asserts that every `reason` in it exists in the
  `ErrorReason` registry.
- An unknown reason shows generic copy and the correlation id (the version-skew rule).

### 6.6 Capability gating (AC 4)

- The Audit nav entry uses `navStateOf(iam, 'iam.audit')`: absent when IAM does not report the
  capability, available when it does, and disabled with a reason when IAM is degraded.
- The `/iam/audit` page calls `discovery().getServiceState('iam', token)`. When the state is
  available without `iam.audit`, or absent, it returns `notFound()`. When IAM is degraded, it shows
  the degraded `ErrorState`, not a 404. Only an available state with the capability reaches
  `ListAuditEntries`.

---

## 7. Changes to packages

### 7.1 `@paigasus/auth` under a `basePath`

The package's tests never ran inside a Next app with a `basePath`. Three parts break under
`basePath: '/iam'` (all confirmed from the Next 16.3.4 source; the plan measures them in a real
build first):

1. **Proxy.** Next strips the basePath from `req.nextUrl.pathname`
   (`next/dist/server/web/next-url.js:41-57`; `get-next-pathname-info.js:20-23`), and the proxy's
   `NextURL` gets `nextConfig` (`web/adapter.js:123-125`). So `publicPaths` built from
   `authRoutePaths({ basePath })` never match (`middleware.ts:67-78`), the login route redirects to
   itself, and `returnTo` loses `/iam` (`middleware.ts:89`).
2. **`requireSession`.** It calls `redirect('/iam/auth/login?…')` (`next/get-session.ts:103`), and
   app-render adds the basePath again without a duplicate check (`app-render.js:2390`, `:5901`;
   `add-path-prefix.js:12-18`). The result is `/iam/iam/auth/login`.
3. **Route handler.** Next removes the basePath from `req.url` before the handler sees it
   (`base-server.js:621-629`), so `createAuthRoutes` finds no route (`http/routes.ts:83-89`). Also,
   the handler's `req.url` uses the server's bind address, not the public host
   (`next-server.js:1275-1281`), and `handleCallback` passes it to `authorizationCodeGrant` as
   `currentUrl` (`http/routes.ts:206-211`). openid-client then sends it as `redirect_uri`
   (`openid-client/build/index.js:909`), which does not match the one in the authorization request.
   This second part happens without a basePath too.

The fixes stay in the Next adapter files, and one core change fixes the `redirect_uri`:

| Where | Fix |
|---|---|
| `middleware.ts` | Match `req.nextUrl.pathname` against basePath-relative public paths. Build `returnTo` as `req.nextUrl.basePath + pathname + search`. |
| `next/get-session.ts` | `requireSession` redirects to the basePath-relative `/auth/login`, with a `returnTo` that keeps the basePath. |
| `server.ts` `createAuthRouteHandler` | Rebuild the request URL as `PAIGASUS_PUBLIC_ORIGIN + basePath + pathname + search` before it calls `createAuthRoutes`, so the core route table stays keyed by the full path. |
| `http/routes.ts` `handleCallback` | Build `currentUrl` from `runtime.redirectUri` plus the incoming query string, so `redirect_uri` always equals the value sent to `/authorize`, override or not. |
| `http/routes.ts` `handleLogin` | Replace a `returnTo` under `${basePath}/auth/` with the fallback. |

The package's own tests gain cases with a `NextRequest` built with `nextConfig: { basePath }`.
The plain-Node auth e2e harness does not use Next and keeps passing full paths; the plan runs
`paigasus-auth-ts:test-e2e` to prove it.

### 7.2 The Turbopack import fix

Next 16.3.4's Turbopack does not resolve a `.js` relative specifier to a `.ts` file (CLAUDE.md,
measured in SMA-510). Four packages that this app compiles use such specifiers:

| Package | `.js` relative imports in `src/` | Change |
|---|---|---|
| `@paigasus/auth` | 73 (all three entries) | extensionless |
| `@paigasus/sdk` | 29 (all entries except `./errors/types`) | extensionless |
| `@paigasus/discovery` | 43 (`/server`, `/react`; `/client` is already done) | extensionless |
| `@paigasus/proto` | 34 (hand-written and generated) | hand-written: extensionless; generated: below |

- Only `src/` changes; test files keep their `.js` imports, which vitest resolves. All packages use
  `moduleResolution: bundler` (`ts/tsconfig.base.json`), so extensionless imports type-check.
- **Generated proto code.** `contracts/buf.gen.yaml` passes `import_extension=.js` to
  `buf.build/bufbuild/es:v2.13.0`. The plan measures the plugin's values and changes the option so
  that the output is extensionless, then regenerates. The inline codegen-drift step in `ci.yml`
  checks the committed output.
- **Plain-Node harnesses.** `@paigasus/auth`'s e2e fixture server runs package source under plain
  Node through `tests/fixtures/ts-esm-loader.mjs`, which retries `.js` as `.ts`. Plain Node does not
  probe extensions, so the loader gains an "extensionless → `.ts`, then `/index.ts`" retry. The plan
  finds every other harness that runs package source under plain Node and extends it the same way.
- **Enforcement.** A new custom rule, `paigasus/no-js-relative-specifier`, reports an `import`,
  `export … from` or `import()` whose specifier starts with `./` or `../` and ends in `.js`.
  - It is a **custom rule with its own name**, not another `no-restricted-imports` entry. In flat
    config, a second `no-restricted-imports` block that matches the same files **replaces** the
    first, and would switch off the `sdk`, `auth-*` and `discovery` boundary rules in silence.
  - It ships as a **separate export**, `sourceRules`, from `@paigasus/next-config/eslint`, not
    inside `boundaryRules`. A `packages/*/src` scope in `boundaryRules` would fail the reverse
    liveness loop (`boundaries.test.ts:236-243`). `ts/eslint.config.js` spreads `sourceRules`, and
    a test pins that spread.
  - It applies to `packages/*/src/**`, test files excluded. ESLint ignores `**/generated/**`, so a
    proto test asserts that no file under `src/generated/` holds a `.js` relative specifier.
- The CLAUDE.md entry on this Turbopack limit is updated to say what is fixed and what holds it.

### 7.3 `@paigasus/sdk`

`/iam` re-exports `ServiceInfoService` (from `@paigasus/proto`), so the app can make the
provisioning call (§ 4.5). Apps must not import `@paigasus/proto`.

### 7.4 Boundary rules

- **Proxy.** `paigasus/boundaries/app-middleware` (`eslint.mjs:288-302`) matches
  `apps/**/middleware.*` only. Its glob becomes `apps/**/{middleware,proxy}.{ts,js,mts,cts,mjs,cjs}`.
  Because that block **replaces** the `apps` block's `no-restricted-imports` for those files, it
  also restates the `@paigasus/proto` ban. New `boundaries.test.ts` rows lint a `proxy.ts` path
  for each banned import.
- **Test doubles.** The fake IAM builds an `ErrorInfo` detail, and only `@paigasus/proto` exports
  `ErrorInfoSchema`. The `apps` rule gains `ignores: ['apps/*/tests/support/**']`. Only the test
  doubles live there.

### 7.5 `proxy.ts`

- It imports only `@paigasus/auth/middleware`. Its public paths are the auth routes, `/` and
  `/healthz` (all basePath-relative after § 7.1), and it checks cookie presence only
  (ADR-0017 decision 7).
- It exports a `config.matcher` that excludes `_next/static`, `_next/image` and `favicon.ico`.
  Without one, the default matcher is a catch-all (`middleware-plugin.js:138-147`), and a visitor
  with no cookie gets a login redirect for every CSS and JS file. The plan measures how the matcher
  interacts with the basePath, and the e2e tier loads the public page with no cookie.

### 7.6 Tailwind

- `globals.css` gets `@source '../../../packages/paigasus-app-shell/src'` next to the
  `@paigasus/ui` line.
- A third sentinel, `--paigasus-app-shell-source-probe`, goes into one app-shell component, in the
  same way as the ui probe in `table.tsx`.
- `ci/tailwind-source/run.mjs` asserts it in all three modes.

### 7.7 The SDK client-boundary fixture (SMA-508 AC 1, moved here)

- A minimal Next app at `ts/apps/iam-console/tests/fixtures/client-imports-sdk/` holds one
  `'use client'` component that imports `@paigasus/sdk/iam`.
- A test runs `next build` on it and asserts a non-zero exit and the `server-only` error text.
- A **positive control** builds the same fixture with the import in a server component, and
  asserts success.
- No ESLint rule can do this job, because rules select files by path, not by the `'use client'`
  directive.
- The app `tsconfig.json` (`include: ["**/*.ts", …]`) excludes `tests/fixtures/**`, so the fixture
  and its `.next/types` are not part of the app's type-check.

---

## 8. Moon, CI and dependencies

- **Sources.** `fileGroups.sources` becomes `app/**/*`, `lib/**/*` and `proxy.ts`. Without that,
  an edit in `lib/` or `proxy.ts` serves a cached pass.
- **Inputs.** `build`, `test` and the new `test-e2e` list the `src/**` and `package.json` of `auth`,
  `sdk`, `discovery`, `app-shell`, `proto`, `kernel`, `ui` and `next-config`, plus
  `next-config/tsconfig.app.json`, `/ts/pnpm-lock.yaml` and `/ts/tsconfig.base.json`. `build` and
  `test` use `options.merge: replace`, so they list every input by hand. Every fixture `.next`
  directory is negated (`!tests/fixtures/**/.next/**`).
- `dependsOn` gains the same projects.
- `iam-console-ts:test-e2e` (`cache: false`) is picked up by the existing `:test-e2e` CI target.
- **`ci/affected-graph/run.sh`.**
  - The `auth`, `discovery`, `app-shell`, `proto` and `ui` cases add `iam-console-ts:build`,
    `:test` and `:test-e2e` to their expected sets.
  - A new two-anchor `sdk->iam-console` case (the file has none today).
  - A new `app-shell->console` case anchored on the file that holds the new sentinel.
  - A new app-local case with two anchors, `lib/iam.ts` and `proxy.ts`.
  - `contracts->proto` gains `iam-console-ts`, because the app now depends on `proto`.
- **Dependencies.** `msw` goes into the pnpm catalog at a version released more than 24 hours
  before the commit. If `msw` declares an install script, `allowBuilds` gets `msw: false` (the
  script copies a browser service worker, which node tests do not need). The app gets
  `@paigasus/auth`, `sdk`, `discovery`, `app-shell`, `kernel`, `redis` and `zod`; the tests use
  `@connectrpc/connect-node`, `jose` and `@playwright/test`.
- The full CI target list runs before the push (CLAUDE.md).

---

## 9. Testing

No tier needs a live service or Docker.

### 9.1 Test doubles (`tests/support/`)

- **Fake IAM (gRPC).** A real `@connectrpc/connect-node` HTTP/2 server on an ephemeral port, with
  scripted `ServiceInfoService`, `AuthnService`, `TenancyService`, `AuthorizationService` and
  `AuditService` handlers. It **behaves like IAM in the ways this design depends on**:
  - `Introspect` returns `identity-not-provisioned` until the principal has made one
    bearer-enforced call;
  - `Introspect` returns an empty `role_grants`;
  - a denial is `Code.PermissionDenied` with an `ErrorInfo(reason: "forbidden", metadata:
    { retryable: "false" })` detail, as `status_to_grpc` makes it (`convert.rs:111-131`), and the
    correlation id is carried where IAM carries it (the plan reads that from IAM's code);
  - every handler counts its calls, so a test can assert that a call happened.
- **Fake IAM (HTTP).** `GET /v1/service-info`. In vitest, MSW serves it. In e2e, a `node:http`
  server serves it on a second port, because MSW cannot reach into the standalone server process.
- **Fake IdP.** An in-process **HTTPS** server (`node:https`, `jose`), because `authEnvShape`
  requires `https:` for `PAIGASUS_OIDC_ISSUER` and `PAIGASUS_PUBLIC_ORIGIN` (`config.ts:22-27,40,43`).
  It serves discovery, JWKS, `/authorize` (it approves at once) and `/token`. `/token` checks PKCE
  and asserts that `redirect_uri` equals the value sent to `/authorize`. It is based on
  `ts/packages/paigasus-auth/tests/fixtures/jwks.ts` and `tests/e2e/tls-fixture.ts`, copied into
  `tests/support/`, because one package's tests must not import another package's tests.

### 9.2 Tier 1 — unit (vitest)

- `buildNavEntries`: all three IAM states, with and without `iam.audit`, with `mayI` allowed and
  denied; the gateway absent, degraded and available; every entry went through `navStateOf()`.
- `callIam`: the § 6.1 table, and that a non-`ConnectError` is rethrown unchanged.
- The error copy tables against the `ErrorReason` registry (§ 6.5).
- Config: `PAIGASUS_IAM_GRPC_URL` validation, and a failed parse without `iam` in
  `PAIGASUS_SERVICES`.
- The logger writes only the port's fields, and never a DSN.
- The `actions.ts` structure test (§ 5.3).

The vitest setup sets `__NEXT_EXPERIMENTAL_AUTH_INTERRUPTS=true` (without it, `forbidden()` throws
E488, `forbidden.js:26`), aliases `server-only` to a stub as the other packages do, mocks
`next/headers` and `next/cache`, and sets `ssr.resolve.conditions` (CLAUDE.md, vitest 5).

### 9.3 Tier 2 — integration (vitest, fake IAM + MSW)

- The loaders and command functions of § 5 against scripted IAM answers.
- **The ErrorInfo trailer round trip.** The fake IAM returns the denial of § 9.1; the test asserts
  `presentation: 'forbidden'`, the reason and the correlation id. The SDK's own tests build the
  `ConnectError` in memory and say they do not test the wire (`sdk/tests/map-error.test.ts:10-15`).
- **Not pre-judged, direction 1:** `mayI` returns false, the fake IAM allows the call, and the
  command function returns `{ ok: true }`; the fake counts one call.
- **Provisioning:** the resolver calls `GetServiceInfo` before `Introspect`; a failed call degrades
  to `principalPrn: null` and logs `principal.resolve_failed`; `currentPrincipal()` retries once
  after `identity-not-provisioned`.
- `myScopes()`: memberships only, memberships plus grants, team-only and project-only scopes,
  deduplication, and the 50 cap.

### 9.4 Tier 3 — e2e (Playwright, `iam-console-ts:test-e2e`)

- A **production build** runs through the standalone `server.js`, as in the app-shell e2e.
- The browser reaches it through an in-process **TLS terminator** that forwards `Host` and sets
  `X-Forwarded-Proto`, so `PAIGASUS_PUBLIC_ORIGIN` is a real `https:` origin and Next's Server
  Action origin check sees the forwarded host. The server trusts the test CA through
  `NODE_EXTRA_CA_CERTS`.
- The session store is `memory`, so `PAIGASUS_ZONES` holds one zone: `createAuthRuntime` rejects
  `memory` with more than one zone (`runtime.ts:99-105`). A real two-zone test belongs to SMA-513.
  The single-zone map still proves the Flight handoff, because `ZoneLink` renders nothing when the
  zone map is missing.
- Short `PAIGASUS_DISCOVERY_*_MS` values let a scenario change the IAM state.

| Scenario | AC |
|---|---|
| The public page `/iam/` loads with no cookie, with its CSS and JS | shell |
| Unauthenticated `/iam/orgs` → proxy redirect → IdP → callback → "Your organizations" lists the scopes from the fake Introspect; the fake saw `GetServiceInfo` first | 1 |
| A first-time user (not yet provisioned) lands on the same screen | 1 |
| A denied page read → **HTTP 403** and the 403 view inside the shell, with the correlation id; the fake counted the call | 2 |
| `mayI` true and IAM denies → the 403 renders | 2 |
| `mayI` false → the create button is hidden, the reads still render, and a direct POST to the action reaches IAM | 2 |
| A denied Server Action → an inline 403 in the form | 2 |
| `iam.audit` reported and allowed → the Audit entry appears and the page works | 4 |
| `iam.audit` not reported → the entry is absent, and `/iam/audit` returns 404 | 4 |
| IAM degraded → the entries are disabled with a reason, and `/iam/audit` shows the degraded view | 4 |
| No response body (HTML, RSC payload or action result) contains the fake access token or refresh token | ADR-0017 |
| Sign out → POST `/iam/auth/logout` → `/iam/orgs` redirects to login again | 1 |

### 9.5 AC 3

- The `build` script asserts `.next/standalone/apps/iam-console/server.js`.
- `tests/standalone-runtime.test.ts` boots the server twice with a complete, valid environment and
  targets `GET /iam/healthz`, which is public and returns `{ zone, zones }`. The mismatch case
  asserts the mismatch message in the server output, not only a 500, so a missing unrelated
  variable cannot pass it.
- `repo:next-public-free` stays green, and the e2e tier runs against the standalone server.
- The client-boundary fixture of § 7.7 runs in `iam-console-ts:test`.

---

## 10. The deployment contract (for SMA-513)

The console image needs these environment variables:

| Group | Variables |
|---|---|
| Zone | `PAIGASUS_ZONE=iam`, `PAIGASUS_ZONES` (JSON, shared with every zone) |
| Auth | the `authEnvShape` keys (`ts/packages/paigasus-auth/README.md:29-47`), with `PAIGASUS_SESSION_STORE=redis` in any multi-zone deployment |
| Discovery | `PAIGASUS_SERVICES` (JSON; must contain `iam`), optional `PAIGASUS_DISCOVERY_*_MS` |
| IAM | `PAIGASUS_IAM_GRPC_URL` |

Deployment assumptions:

- IAM's `authn.issuers[].audiences` (`rs/crates/services/paigasus-iam/src/config.rs:178`) accepts
  the audience of the console client's access tokens.
- The ingress forwards `Host` (or `X-Forwarded-Host`) unchanged, so Next's Server Action origin
  check passes. Otherwise `serverActions.allowedOrigins` is needed.
- The console reaches IAM's gRPC port over h2c inside the cluster, or over TLS with a CA that Node
  trusts (a public CA or `NODE_EXTRA_CA_CERTS`). The SDK's `TransportOptions` has no CA option
  (`transport.ts:21-23`).
- The OIDC client registers `https://<origin>/iam/auth/callback` as a redirect URI, and
  `https://<origin>/iam/` as the post-logout URI.

---

## 11. Follow-ups

Linear issues, made on 2026-09-11 after the spec was approved:

1. **SMA-629 — the dead-letters capability key and screen:** the contracts registry, IAM
   `Capabilities`, the discovery vocabulary, and `/iam/dead-letters`.
2. **SMA-630 — rename, archive and restore** for organizations, teams and projects (9 of
   `TenancyService`'s 14 mutations; this issue delivers 5).
3. **SMA-631 — a shared home for the provisioning and principal code** (§ 4.5) when SMA-512 needs
   it.
4. **SMA-632 — an IAM `WhoAmI` RPC**, so that provisioning does not depend on `GetServiceInfo`
   being bearer-enforced.
5. **SMA-633 — IAM `Introspect` role grants.** `role_grants` is always empty
   (`authenticate_token.rs:161-165`), so `SessionView.grants` and `can()` have no data anywhere.

---

## 12. Recorded limits and departures

- **ADR-0017 decision 8** says "`can()` reads role grants from the session". This app uses
  `IsAuthorized` self-queries instead (D5), because IAM reports no grants. The rule that matters —
  cosmetic only, IAM authoritative — is unchanged. The ADR gets a note in Notion.
- The attach form takes a raw principal PRN (§ 5.2).
- The `authInterrupts` fallback (§ 6.2) and the correlation-id holder fallback (§ 6.2).
- `mayI()` fails open, so an IAM outage shows buttons that IAM then denies.
- The e2e tier runs one zone (§ 9.4).
- If the wasm spike fails (D6), `lib/prn.ts` is an ADR-0005 exception, held to the kernel by the
  parity corpus. The napi packaging defect (`files` excludes `*.node`) stays open for any Node
  consumer of `@paigasus/kernel`, as SMA-634.

---

## 13. Things to measure

**Measured on 2026-09-11, before the plan was written** (throwaway Next 16.3.4 apps in the session
scratchpad; nothing in the repo changed):

| # | Result |
|---|---|
| 1 | Confirmed all three § 7.1 failures. In the proxy, `req.nextUrl.pathname` is `/auth/login` (basePath removed), `req.nextUrl.basePath` is `/iam`, and `req.url` keeps `/iam`. In a route handler, `req.url` is `http://0.0.0.0:<port>/auth/callback?…`: no basePath, the bind address, and only the scheme follows `X-Forwarded-Proto`. A page `redirect('/auth/login?…')` gives `Location: /iam/auth/login?…`; `redirect('/iam/auth/login')` gives `/iam/iam/auth/login`. A raw `Response` `Location` from a route handler passes through unchanged. A proxy `NextResponse.redirect(new URL('/iam/auth/login?…', req.url))` and a `req.nextUrl.clone()` with `pathname = '/auth/login'` both give `/iam/auth/login` once. |
| 2 | `forbidden()` → HTTP 403, the nested `(console)/forbidden.tsx` renders inside the `(console)` layout. A `cache()` holder does not reach the view (§ 6.2). |
| 3 | protobuf-es `import_extension` accepts `none` (the default when omitted), `.js` and `.ts`. Omitting it gives extensionless imports. `buf generate` works in this environment. |
| 4 | The matcher `'/((?!_next/static|_next/image|favicon.ico).*)'`, written WITHOUT `/iam`, skips static assets under the basePath. Without a matcher, the proxy runs for `/iam/_next/static/*`. A module that imports `server-only` loads in `proxy.ts`. |
| 5 | The napi kernel binding fails in a Next build (§ 4.7, D6). |
| 6 | `Action::parse` takes PascalCase names (`CreateTeam`, `ListOrganizations`, `ListAuditLog`; `rs/crates/libs/paigasus-iam-core/src/authz/action.rs:114-163`). IAM puts the correlation id in `ErrorInfo.metadata["correlation_id"]` (`convert.rs:59-74`) with domain `iam.paigasus.io`, and also sends the `paigasus-correlation-id` header; the SDK reads the metadata first (`map-error.ts:179`). |
| 7 | Two plain-Node loaders import package source: `paigasus-auth/tests/fixtures/ts-esm-loader.mjs` (the e2e fixture server and the multi-process single-flight test) and `paigasus-discovery/tests/containers/support/ts-esm-loader.mjs`. Both need the extensionless retry. |
| 8 | `msw` 2.15.0 (released 2026-07-08) declares a `postinstall` script, so `allowBuilds` needs `msw: false`. |
| 9 | `@paigasus/proto`'s root entry exports `ServiceInfoService`; the SDK does not re-export it yet. |

The plan still measures these in the repo itself, before it builds on them:

1. The § 7.1 fixes, in the real app's e2e tier (the failures are measured; the fixes are not).
2. The kernel wasm spike (D6, § 4.7).
3. Whether IAM adopts an incoming `paigasus-correlation-id`, and whether `headers()` works in
   `forbidden.tsx` (§ 6.2).
4. A Server Action POST under the basePath through the TLS terminator (§ 9.4). The scratch
   measurement skipped it, because a raw POST needs Flight-encoded arguments.

---

## 14. Challenge log (revision 1 → 2)

The spec-challenger (Opus) returned **NEEDS REWORK**. Each finding was checked against the code
before it was folded in.

| Finding | Verdict | Where |
|---|---|---|
| BLOCKER — auth breaks under `basePath` in three places | confirmed from Next source | § 7.1 |
| BLOCKER — first login 403s: Introspect never provisions | confirmed | § 4.5 |
| BLOCKER — Introspect grants always empty, `can()` hides everything | confirmed; owner chose D5 | § 2.2, § 4.6, § 6.3 |
| MAJOR — landing data model (memberships ≠ grants, dead org rows, N calls) | confirmed | § 5.1 |
| MAJOR — § 5.1 contradicts § 6.3; AC 2 tests can pass vacuously | justified | § 6.3, § 9.3, § 9.4 |
| MAJOR — e2e: memory store + two zones throws; https-only issuer | confirmed | § 9.1, § 9.4 |
| MAJOR — standalone test premise breaks | justified | § 3.3, § 9.5 |
| MAJOR — Moon inputs miss `lib/`, `proxy.ts`; affected-graph gaps | confirmed | § 8 |
| MAJOR — proxy has no matcher | confirmed from Next source | § 7.5 |
| MAJOR — 403 view cannot receive the correlation id | justified | § 6.2 |
| MAJOR — loaders use a service locator | justified | § 5.2, § 9.2 |
| MAJOR — discovery Redis client lifetime; 4 preconditions, not 2 | confirmed | § 4.4 |
| MAJOR — no test that the browser never receives a token | justified | § 9.4 |
| MINOR — route handler gets a Promise | justified | § 4.2 |
| MINOR — `AuthEventName` is closed | confirmed | § 4.5, § 4.8 |
| MINOR — login-path timeouts | justified | § 4.5 |
| MINOR — lint rule placement; app-middleware replaces the proto ban | confirmed | § 7.2, § 7.4 |
| MINOR — test doubles need `@paigasus/proto` | justified | § 7.4 |
| MINOR — Audit is Root-only; degraded view, not 404 | confirmed | § 5.4, § 6.6 |
| MINOR — PRN parsing belongs to the kernel (ADR-0005) | justified | § 4.7 |
| MINOR — `msw` install script | unknown; measured in the plan | § 8 |
| MINOR — public group; `idp_error` and post-logout go to `/iam/` | confirmed | § 3.3 |
| MINOR — root error boundary | justified | § 6.1 |
| MINOR — the layout does not guard Server Actions | justified | § 3.3, § 5.3 |
| MINOR — `callIam` must rethrow | justified | § 6.1 |
| MINOR — unspecified cells; malformed UUIDs; project consistency | justified | § 5.2, § 6.1 |
| MINOR — rename table incomplete; tsconfig includes the fixture | confirmed | § 3.1, § 7.7 |
| MINOR — `returnTo` loop | confirmed | § 6.4, § 7.1 |
| MINOR — weak deprecation citation | confirmed | § 3.3 |
| QUESTION — AC 3 sign-off | the Linear AC update covers AC 2–5 | § 2.2 |
| QUESTION — attach by raw PRN | recorded limit; owner to confirm | § 12 |
| QUESTION — TLS to IAM, token audience, Host forwarding, env contract | recorded | § 10 |
