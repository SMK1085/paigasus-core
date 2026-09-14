# SMA-512 — `gateway-console`: the AI Gateway zone

**Status:** draft, revision 2 (after the adversarial challenge)
**Date:** 2026-09-13
**Issue:** [SMA-512](https://linear.app/smaschek/issue/SMA-512/ts-gateway-console-app-the-ai-gateway-zone)
**ADR:** ADR-0017 — Console topology & session ownership; ADR-0005 (kernel owns PRN logic);
ADR-0019 (error model); ADR-0020 (capability discovery)
**Design:** Frontend Architecture Scoping §§ 1, 2, 3, 6
**Depends on:** SMA-510 (`@paigasus/app-shell`), SMA-511 (`iam-console`) — both Done
**Closes:** SMA-631 (a shared home for the console principal code)
**Blocks:** SMA-513 (multi-zone ingress and Helm chart)

---

## 1. Problem

ADR-0017 chose a multi-zone topology: one Next.js app per backend service, each with its own
image, all behind one origin, all sharing one session cookie. One zone exists today —
`ts/apps/iam-console`, from SMA-511.

One zone cannot prove that topology. Two properties separate multi-zone from both "one big app"
and "separate origins", and neither is observable from a single app:

1. A user who logged in at one zone reaches another zone with no re-authentication.
2. No zone is a login single point of failure for another.

This issue adds the second zone and proves both.

### 1.1 Acceptance criteria (from the issue)

1. A user already logged in through the IAM zone reaches this zone with no re-authentication.
2. With the IAM zone stopped, a cold user can still log in here.
3. Streaming responses arrive incrementally. Nothing in middleware consumes or buffers the body.
4. The app builds to a standalone image, and the `NEXT_PUBLIC_` gate stays green.

§ 2.2 records how this design reads AC 2 and AC 4, and why **AC 3 moves to a follow-up issue**.

---

## 2. Scope and decisions

### 2.1 Decisions (Sven, 2026-09-13)

| # | Question | Decision |
|---|---|---|
| D1 | Where the two-zone proof lives | A full two-zone e2e tier in this issue. Both standalone servers run behind one TLS terminator, over one shared session store. |
| D2 | SMA-631 — the shared home | Make it now: a new server-only package that may depend on both `@paigasus/auth` and `@paigasus/sdk`. `iam-console` moves onto it in the same change, so a second copy never exists. |
| D3 | The playground's product surface | One chat page, streaming, no persistence. **Superseded by D11.** |
| D4 | The app name | `ts/apps/gateway-console`, `@paigasus/gateway-console`, Moon id `gateway-console-ts`. Symmetric with `iam-console`. The issue text predates SMA-511's rename. |
| D5 | The three app-scoped gates | Parameterize them over `ts/apps/*`, and add a liveness assertion so an uncovered app fails CI instead of being skipped. |
| D6 | The model id | An operator-configured default. **Superseded by D11**; no gateway configuration key survives. |
| D7 | The shell | Full parity with `iam-console`, the organization switcher included. Reason: the gateway gets organization and project settings later, so the switcher must exist before those screens do. |
| D8 | What the switcher does today | Selection writes the scope into the URL (`/gateway/orgs/<org>`). The route shape the later settings screens hang off exists from day one. |
| D9 | The shared package's shape, and the e2e tier's home | One package taking its dependencies as arguments (§ 5). The two-zone tier lives inside `gateway-console-ts:test-e2e` (§ 10.5). |
| D10 | The test doubles | Move the fake IAM, the fake IdP and the TLS helpers out of `iam-console` into the shared package's testing subpath. One copy, not three. |
| D11 | AC 3, after the challenge found the gateway rejects an OIDC token (§ 2.3) | **Descope the chat playground.** This issue delivers ACs 1, 2 and 4. A follow-up issue owns the gateway's interactive-user authentication path and the playground together. |

### 2.2 How the acceptance criteria are read

- **AC 2 — "with the IAM zone stopped".** This means the `iam-console` **app** is stopped. It does
  not mean the IAM **service** is stopped. D7 puts the organization switcher in this zone, so the
  app calls IAM for `myScopes()`, and provisioning and `Introspect` need IAM as well. A cold login
  at `/gateway` therefore needs the IAM service and no other console app. That is the property
  ADR-0017 claims. § 10.5 explains why the test asserts it by observation rather than by stopping
  a process, which on its own would be tautological.
- **AC 4 — "a standalone image".** SMA-513's scope says "Console images following the conventions
  established for the services in SMA-500", so the OCI image belongs there. This issue proves the
  `output: 'standalone'` build and the `NEXT_PUBLIC_` ban. SMA-511 read the identical wording the
  same way (its spec § 2.2).
- **AC 3 — descoped (D11).** See § 2.3.

The AC text in Linear is updated for AC 2, AC 3 and AC 4 after this spec is approved, each with
its reason.

### 2.3 Why AC 3 cannot be built here

`POST /v1/chat/completions` is wrapped by `require_iam_auth`
(`rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs:93`). That middleware:

1. takes the bearer and calls **only** `iam.introspect_api_key(&key)` (`.../http/auth.rs:64`);
2. rejects a non-`active` status, and rejects an empty `scope_prn` (`auth.rs:75-89`);
3. authorizes `InvokeModel` **against that `scope_prn`** (`auth.rs:98`).

The OIDC leg, `introspect_token`, is reached only from `require_authenticated`, whose own doc says
so: *"This relaxation is scoped to THIS middleware; `require_iam_auth` is unchanged"*
(`auth.rs:146`). The test `require_iam_auth_still_rejects_an_unprovisioned_identity`
(`auth.rs:872-879`) pins the behaviour.

So a console session bearer — an OIDC access token — receives `401 invalid-api-key`
(`.../http/error.rs:129`). The gateway's chat surface is built for service accounts holding API
keys, and it has no interactive-user path. A user principal also has no `scope_prn`, so even a
widened introspection would leave the authorization resource undecided.

The two apparent shortcuts are both closed:

- **Give the console a service-account key.** ADR-0020 D4 forbids exactly that
  (`auth.rs:128-129`), and one shared key would destroy per-user attribution in the gateway's
  request log, which records `principal` and `key_id` per call (`.../http/chat.rs:162-170`).
- **Test it against the fake gateway anyway.** Revision 1 of this spec did precisely that, and
  every tier would have gone green while the shipped product could not make one chat call. That is
  the vacuous-green shape this repository's gate rules exist to prevent.

The follow-up issue (§ 12 item 1) therefore owns the gateway change and the playground together,
because neither is useful without the other.

### 2.4 In scope

- The new app (§ 4), the shell, the switcher and the scope route (§ 7), and the zone overview
  page (§ 6).
- `@paigasus/console-core` (§ 5), and the refactor of `iam-console` onto it.
- One small export added to `@paigasus/auth` (§ 5.4).
- Parameterizing the three app-scoped gates (§ 8).
- Moon, CI and affected-graph registration (§ 9).
- Four test tiers (§ 10), including the two-zone tier.
- The deployment contract for SMA-513 (§ 11).

### 2.5 Out of scope

- **The chat playground and the streaming passthrough** (D11, § 2.3, follow-up § 12 item 1).
- Any change to `paigasus-gateway`.
- The console OCI image and the ingress (SMA-513).
- Organization and project **settings screens** in the gateway zone. D8 delivers the route shape
  only.

---

## 3. The change is delivered as four pull requests

Each is independently green, and each closes something on its own. The order is fixed by
dependency, smallest first.

| # | Pull request | What it delivers | What proves it |
|---|---|---|---|
| 1 | **Parameterize the app-scoped gates** (§ 8) | `ci/next-env`, `ci/tailwind-source` and `ts/eslint.config.js` stop naming one app. Liveness assertions added. | The three gates stay green on one app, and `check_tailwind_guard_invocations`'s own self-test in `ci/affected-graph/ci_targets.py` proves a second app directory is covered — the `run.mjs` self-test rows only cover its `appLabel` parameter, and the negative control is unchanged. |
| 2 | **`@paigasus/console-core`** (§ 5) | The package, its boundary rules, the testing subpath, the `@paigasus/auth` export, and `iam-console` refactored onto all of it. Closes SMA-631. | **No behaviour change.** `iam-console`'s existing suites stay green. |
| 3 | **The `gateway-console` app** (§ 4, 6, 7) | The app, the shell, the scope route, the overview page, and a single-zone e2e tier. | AC 4 in full, and a cold login at `/gateway` in a single-zone configuration. |
| 4 | **The two-zone tier** (§ 10.5) | The Redis container, the path-routing terminator, both servers, and the affected-graph inputs edge. | AC 1 and AC 2 in their real form. |

### 3.1 Which strict-equality cases each pull request re-baselines

`ci/affected-graph/run.sh` compares expected task sets with **strict equality**, so every case
touched must be re-baselined in the same commit. Enumerating them per pull request is what keeps
the split's benefit — a reviewer who cannot see which re-baselines belong to a refactor cannot
tell a refactor defect from a new-app defect.

**Pull request 2** adds the `paigasus-console-core-ts` project, whose tasks then join every case
whose anchor lies in its declared inputs:

| Case (line) | Change |
|---|---|
| `auth->auth-tasks` (:413) | add console-core tasks |
| `proto->sdk` (:429), `proto-iam->sdk` (:451) | add console-core tasks |
| `discovery->discovery-tasks` (:463), `discovery-adapters->discovery-tasks` (:475) | add console-core tasks |
| `sdk->iam-console` (:501), `sdk-errors->iam-console` (:503) | add console-core tasks |
| `iam-console-lib->iam-console-tasks` (:515) | **re-anchor.** Its anchor is `ts/apps/iam-console/lib/iam.ts`, and that file moves. Re-anchor on a file that stays — `lib/config.ts` — and state why in the case comment. |
| new: `console-core->consumers` | two anchors inside the new package |

`iam-console-ts` must **not** drop out of any case: the app still compiles every package it
compiled before, now one level deeper.

**Pull request 3** adds `gateway-console-ts:{build,test,typecheck,test-e2e}` to `ui->console`
(:383), `ui-components->console` (:400), `app-shell->console` (:509), `auth->auth-tasks`,
`proto->sdk`, `proto-iam->sdk`, both `discovery->*` cases, `sdk->iam-console`,
`sdk-errors->iam-console` and `console-core->consumers`, and adds a new app-local
`gateway-console-lib->gateway-console-tasks` case with two anchors.

**Pull request 4** adds the `iam-console -> gateway-console` case (§ 9).

### 3.2 The risk of splitting

Pull request 2 designs a shared package with only one consumer, so it can bake in an
`iam-console`-shaped interface. Two things hold against that: § 5.3 fixes the interface before the
code is written, and pull request 3 is the first consumer that would expose a wrong shape. If
pull request 3 has to change the interface, that is a signal the split hid something, and it is
recorded rather than absorbed quietly.

**The cost.** `main` requires strict up-to-date branches, so the four merges are sequential and
each blocks the next.

---

## 4. The app

`ts/apps/gateway-console`, package `@paigasus/gateway-console`, Moon id `gateway-console-ts`,
`layer: application`.

`next.config.ts` calls `createNextConfig({ zone: 'gateway', basePath: '/gateway',
outputFileTracingRoot, extend: { experimental: { authInterrupts: true } } })`. The factory forces
`output: 'standalone'`. `authInterrupts` is needed for the same reason as in `iam-console`:
without it `forbidden()` throws instead of rendering the 403 boundary.

```
ts/apps/gateway-console/
  proxy.ts                        cookie presence and the correlation id
  next.config.ts  postcss.config.mjs  tsconfig.json  vitest.config.ts  playwright.config.ts
  next-env.d.ts                   tracked, and now covered by repo:next-env-drift (§ 8)
  app/
    layout.tsx  globals.css  error.tsx  global-error.tsx  providers.tsx
    healthz/route.ts              public; returns { zone, zones }
    auth/[...auth]/route.ts       createAuthRouteHandler, mounted at /gateway/auth/*
    (public)/layout.tsx  page.tsx
    (console)/layout.tsx  error.tsx  forbidden.tsx
    (console)/page.tsx            the zone overview (§ 6)
    (console)/orgs/[org]/page.tsx the scope route (§ 7.2)
  lib/
    config.ts                     the app's one defineRuntimeConfig call
    auth.ts                       composition: the auth runtime and the resolver
    console.ts                    the app's one createConsoleRuntime call (§ 5.3)
    nav.ts                        the zone navigation entries
  tests/  unit/  integration/  e2e/  support/
```

`proxy.ts` imports only `@paigasus/auth/middleware`. Its public paths are the auth routes, `/` and
`/healthz`, all base-path relative, and it exports a `config.matcher` excluding `_next/static`,
`_next/image` and `favicon.ico`. Without a matcher the default is a catch-all, and a visitor with
no cookie gets a login redirect for every CSS and JS file.

`createAuthMiddleware` matches `publicPaths` as an **exact-name `Set`**
(`ts/packages/paigasus-auth/src/middleware.ts:107,117`) and redirects anything else that carries
no session cookie (`:122-135`). Revision 1 of this spec assumed a route handler could answer 401
for a cookie-less request; it cannot, because the proxy redirects first. Any future route handler
that must answer rather than redirect has to be listed in `publicPaths` or excluded from the
matcher, and the handler then becomes the authority. No such route exists in this issue, now that
D11 removed the chat route, but the rule is recorded here because the follow-up issue will hit it.

### 4.1 `lib/config.ts`

One `defineRuntimeConfig` call composes three parts:

- `authEnvShape` from `@paigasus/auth/server`;
- `discoveryEnvShape` from `@paigasus/discovery/server`, refined so `PAIGASUS_SERVICES` must
  contain **both** `iam` and `gateway`;
- `PAIGASUS_IAM_GRPC_URL`, the same app-owned key `iam-console` declares, validated as an absolute
  `http:` or `https:` URL with no credentials, query or fragment.

There is **no gateway-specific key**. Revision 1 declared `PAIGASUS_GATEWAY_URL` and
`PAIGASUS_GATEWAY_DEFAULT_MODEL`; D11 removed the only consumer of both. The gateway's address
reaches the app through discovery's `PAIGASUS_SERVICES`, which is where the `GET /v1/service-info`
address belongs.

---

## 5. `@paigasus/console-core`

`ts/packages/paigasus-console-core`. Source-only, `private: true`. Every file under `src/` begins
with `import 'server-only'`.

**The testing surface lives outside `src/`**, at `testing/`, exported as `./testing` →
`./testing/index.ts`. That placement is what lets § 5.6's rule hold without an exception: every
file under `src/` is guarded, and the testing subpath — which vitest and Playwright harnesses
import outside a Next server — is not. The package's Moon `inputs` and the boundary globs of
§ 5.5 both name `testing/**` separately from `src/**` for the same reason.

### 5.1 Why it exists

`iam-console` holds the provisioning call, `IntrospectPrincipalResolver`, `currentPrincipal()`,
`mayI()`, `myScopes()`, the IAM client accessors and the PRN reader in `lib/`. They live in the app
because `@paigasus/auth` must not import `@paigasus/sdk`
(`ts/packages/paigasus-auth/src/ports/principal-resolver.ts:3-8`), and the `sdk` boundary rule
bans every `@paigasus/*` import except `proto` (`ts/packages/paigasus-next-config/src/eslint.mjs:146`).
Only an app may depend on both. SMA-631 recorded that this stops being true when a second zone
needs the same code.

A subpath on either existing package would break one of those two rules. A third package is the
only shape that holds them.

### 5.2 What moved

**Corrected at Task 8, against the shipped tree.** Revision 1 of this table paired `prn-tenancy.ts`
with `tenancy-path.ts` and proposed both move. Only `prn-tenancy.ts` did.

| From `ts/apps/iam-console/lib/` | To `packages/paigasus-console-core/src/` | Note |
|---|---|---|
| `iam-clients.ts` | `iam-clients.ts` | `createIamClients`, the five-client factory. The request-scoped accessors that used to live in `iam.ts` (`currentSession`, `optionalSession`, `sessionToken`, `iamClients`, `iamClientsForAction`) are not carried over as a standalone file — they are built by `runtime.ts`'s `createConsoleRuntime` factory instead (§ 5.3) |
| `principal.ts`, `principal-resolver.ts`, `principal-prn.ts` | `principal.ts`, `principal-resolver.ts`, `principal-prn.ts` | provisioning, the resolver, `introspectWithProvisioning()`, and the one reading of `principal_prn` |
| `authorize.ts` | `authorize.ts` | `createMayI()` / `mayI()` |
| `scopes.ts` | `scopes.ts` | `loadMyScopes()` / `myScopes()`, `switcherOrgs`, `cedarCapabilityOf` |
| `errors.ts` | `errors.ts` | the `callIam` wrapper and the presentation mapping |
| `logger.ts` | `logger.ts` | the JSON-lines adapter |
| `discovery.ts` | `discovery.ts` | the discovery handle and the Redis descriptor cache |
| `correlation.ts`, `correlation-header.ts` | `correlation.ts`, `correlation-header.ts` | the correlation header names and the reader |
| `prn-tenancy.ts` | `prn-tenancy.ts` | the PRN reader |

**`tenancy-path.ts` does not move.** It holds one constant, `TENANCY_PATH = '/orgs'`, and
deliberately carries no `server-only` guard: a Server Actions file may export only async functions,
so the constant cannot live in an `actions.ts`. Moving it into `src/` would force an exception to
the guard rule for one string. It stays per app (`ts/apps/iam-console/lib/tenancy-path.ts`).

Two files exist in the package with no `lib/` predecessor: `runtime.ts`, holding the
`createConsoleRuntime` factory (§ 5.3), and `config-shape.ts`, the structural `ConsoleCoreConfig`
type § 5.3 also corrects. Neither is a move.

`principal-prn.ts` moved **with its two callers, and was not separated from them.** Its header
records the drift that splitting the reading caused once already: the login resolver normalised
`'' -> null` while the live path passed the empty string through, so `mayI()` asked IAM
`isAuthorized({ principalPrn: '' })`, IAM refused with `InvalidArgument`, `mayI()` failed open, and
every mutation control rendered for a principal IAM could not name
(`ts/packages/paigasus-console-core/src/principal-prn.ts:5-12`). Leaving it behind while
`principal.ts` and `principal-resolver.ts` moved would have reinstated a measured production bug.

`discovery.ts` moved although it does not touch the `auth`/`sdk` conflict, because both zones need
byte-identical code and the code is subtle: the Redis descriptor cache it wraps asserts
preconditions on the client, the error listener must never log the error object because node-redis
embeds the DSN in it, and a failed `connect()` must degrade to the `cache-unavailable` reason
rather than fall back to a memory cache in silence. A second hand-written copy would get one of
those wrong.

`tests/unit/prn-tenancy.test.ts` moved with the reader, to
`ts/packages/paigasus-console-core/tests/unit/prn-tenancy.test.ts`. The reader is a recorded
ADR-0005 exception, held to the Rust kernel by the parity corpus. One exception with one parity
test is defensible; two copies of an ADR exception are not.

The logger has two halves and only one moved. The JSON-lines **adapter** moved. The app still
constructs it and passes it in, because the app owns composition.

**What stays in each app:** `config.ts` (the key sets differ per zone), `auth.ts` (composition —
it builds the auth runtime from the moved resolver, logger and clients, and its shape depends on
which keys the zone declares), `console.ts` (the app's one `createConsoleRuntime` call, § 5.3),
`nav.ts` (the entries differ per zone), `form.ts` and `paging.ts` (IAM screen helpers with no
gateway consumer), `tenancy-path.ts` (above), and every React component. Pulling React in would
make the package client-reachable and compromise the `server-only` guard that is its point.

### 5.3 The interface (corrected at Task 8, against the shipped code)

The package reads no environment. An app passes a **config thunk**, not the `iamGrpcUrl` thunk
revision 1 proposed, because `discovery.ts` also needs the session-store and discovery-timing
settings — one thunk replaces the two:

```ts
createConsoleRuntime(deps: {
  config: () => ConsoleCoreConfig,
  authRuntime: () => Promise<AuthRuntime>,
  logger: ConsoleLogger,
}): ConsoleRuntime
```

**Correction to revision 1.** § 5.3 there proposed `{ authRuntime, iamGrpcUrl, logger }`.
`ConsoleCoreConfig` (`ts/packages/paigasus-console-core/src/config-shape.ts`) is a structural type
with eleven keys — `PAIGASUS_IAM_GRPC_URL`, the three `PAIGASUS_SESSION_*` keys `discovery.ts`
needs, `PAIGASUS_SERVICES`, and the six `PAIGASUS_DISCOVERY_*_MS` timings — declared structurally
rather than imported from either app's own `ConsoleConfig`, so both zones satisfy it without the
package knowing either app exists. Each app's own config structurally satisfies it.

`ConsoleRuntime` exposes `currentSession`, `optionalSession`, `sessionToken`, `iamClients`,
`iamClientsForAction`, `iamClientsForToken`, `currentPrincipal`, `mayI`, `myScopes` and `discovery`
(`ts/packages/paigasus-console-core/src/runtime.ts:34-45`). There is no `callIam` on
`ConsoleRuntime`: `callIam` is exported directly from the package's root (§ 5.2), not built by the
factory.

**The factory must be called exactly once per app, at module scope in `lib/console.ts`, and the
app re-exports the returned accessors.** This is a correctness rule, not a style preference. Every
accessor except `iamClientsForToken` and `iamClientsForAction` is a module-scope `cache(...)`
wrapper built inside the factory (`runtime.ts:57,60,63,69,99,102,108,111`), and a second
`createConsoleRuntime()` call makes fresh `cache()` wrappers with a new memoization identity. Two
calls therefore means a second `Introspect`, a second `ListRoleGrants` walk (up to 50 tenancy
reads) and a second discovery probe per render. Outside a React server render `cache()` is a
pass-through (measured on react 19.2.8, `runtime.ts:17`), so **no unit or integration test can
observe the difference** — this is the single most important thing a future reader needs to know
about this package. Only an e2e `Introspect` count can observe it; § 10.5 carries the only
assertion that can, an e2e row counting fake-IAM `Introspect` calls for one page render, and PR 3
owns it.

`iamClientsForAction` is deliberately **not** memoized (`runtime.ts:92`): it returns a `relogin`
failure rather than redirecting, so a Server Action can render an inline error rather than a
mid-action redirect Next cannot resolve correctly inside the zone.

`iamClientsForToken` is also not memoized (`runtime.ts:54`): it is parameterized by `token`, and
every call site — `iamClients`/`iamClientsForAction` inside the factory, and the login callback's
resolver factory in the app's `lib/auth.ts` — already runs behind its own per-request
memoization.

`createIntrospectPrincipalResolver` is exported **separately** (`principal-resolver.ts`), not from
that factory. It cannot come from the console runtime: the console runtime takes the auth runtime,
and the auth runtime takes the resolver, so building it inside would be circular. Its signature is
`createIntrospectPrincipalResolver({ clientsForToken, logger, timeoutMs })`, where `clientsForToken`
takes a bearer token and returns `Pick<IamClients, 'authn' | 'serviceInfo'>` — the two clients
provisioning needs, not the full five. `iam-console/lib/auth.ts` wires it to `iamClientsForToken`
from `./console`, reading the request's correlation id first. Stating the signature here stops two
implementers picking two shapes.

The PRN reader (`principalPrnOf`) and the path helpers (`prn-tenancy.ts`'s exports) are pure
functions and are exported directly.

### 5.4 One small change to `@paigasus/auth` (as shipped)

`iam-console/lib/auth.ts` used to re-declare the session cookie name as `SESSION_COOKIE_NAME =
'__Host-pgs_sid'`, because `@paigasus/auth` defined it at `src/http/cookies.ts:21` but exported it
from no public entry — its `exports` map had only `./server`, `./client` and `./middleware`. A
second app would have made a third copy of a constant that must agree across every zone or the
shared session silently stops working.

**Correction to revision 1: this is solved, not merely proposed.** `@paigasus/auth` now exports
`SESSION_COOKIE` from `./server` (`src/server.ts:34`, re-exporting `src/http/cookies.ts:21`), and
`iam-console/lib/auth.ts` no longer declares its own copy — confirmed by grep: there are zero
occurrences of `SESSION_COOKIE_NAME`, or a re-declared `SESSION_COOKIE`, anywhere under
`ts/apps/iam-console/lib/`. A second app will use the same export once it exists (pull request 3).
This is the only change this issue makes to `@paigasus/auth`, and it shipped in pull request 2.

### 5.5 Boundary rules

Three rules, not one. The first is the hole; the other two keep it from widening.

1. **The hole.** Files under `packages/paigasus-console-core/src/**` may import `@paigasus/auth/*`
   and `@paigasus/sdk/*`. They may **not** import `@paigasus/proto`: the provisioning call reaches
   `ServiceInfoService` through the sdk's re-export, which SMA-511 § 7.3 added for exactly this
   reason. The `testing` subpath is the one exception — its fake IAM needs `ErrorInfoSchema`, and
   `@paigasus/proto` is the only package that exports it, the same exemption SMA-511 § 7.4 granted
   `apps/*/tests/support/**`.
2. **The middleware layer.** `paigasus/boundaries/app-middleware` (`eslint.mjs:301-320`) bans
   `@paigasus/auth/server` and `@paigasus/sdk` from `proxy.ts`, and its own message says why:
   *"`server-only` is a NO-OP in the middleware layer, so nothing else stops a token-bearing
   module being bundled there."* `@paigasus/console-core` transitively holds both, so it joins that
   ban. Without this, the new package silently reopens the hole the rule exists to close.
3. **The testing subpath — DID NOT SHIP.** The design intent was that only `apps/*/tests/**` may
   import `@paigasus/console-core/testing`, and that nothing in an app's `lib/` or `app/` may. This
   was never built. There is no `no-restricted-imports` group for `@paigasus/console-core/testing`
   in `eslint.mjs`, the subpath carries no `server-only` guard by design (§ 5.6), and
   `@paigasus/console-core` is a production `dependencies` entry of `iam-console`, so the subpath
   resolves from production code today. Building it needs the same mechanism the reverse rule below
   needs — a custom named rule in `sourceRules`, not a second `packages/**` block, for the reason the
   next paragraph gives. Residual exposure until it ships: an `app/page.tsx` can import, for example,
   `startFakeIam` today, and `next build` succeeds, pulling an in-process gRPC fake into the app
   bundle. See the plan's "Known limits" section for the follow-up.

**Do not express the reverse rule ("only apps may import console-core") as a new `packages/**`
block.** In flat config a second `no-restricted-imports` block matching the same files
**replaces** the first, which is the trap documented at `eslint.mjs:376-379` — it would switch off
the `sdk`, `auth-*` and `discovery` boundary rules in silence. Several packages
(`paigasus-next-config`, `paigasus-proto`, `paigasus-kernel`) have no block at all, so there is
nothing to extend for them either. Use a custom named rule in `sourceRules`, following the
`paigasus/no-js-relative-specifier` precedent, which exists for this exact reason.

`boundaries.test.ts` gains ALLOWED and DENIED rows for rules 1 and 2 — rule 3 shipped no rule, so it
gained none. Its reverse-liveness loop derives a scope key as `files[0].split('/**')[0]`
(`boundaries.test.ts:254`), so rule 1's glob must yield the key `packages/paigasus-console-core/src`
and that key must appear in `BOUNDARY_SCOPES`.

### 5.6 The testing subpath (D10)

`@paigasus/console-core/testing` exports the fake IAM (gRPC and the `GET /v1/service-info`
handler), the fake IdP, the TLS certificate helper and the TLS terminator. Both apps import it.

This is not the pattern SMA-511 refused. That rule was "one package's tests must not import
another package's tests". This is a package exporting a testing surface, with a stable name and a
stable interface, fenced by boundary rule 3.

`testcontainers`, `jose` and `@connectrpc/connect-node` are **devDependencies** of the package, so
neither app carries them in its production dependency graph. § 14 item 3 measures that pnpm
resolves them for a consuming app.

The testing subpath is not guarded by `server-only`, because vitest and Playwright harnesses import
it outside a Next server. Nothing in `src/` imports it.

---

## 6. The zone overview page

With D11 removing the playground, `(console)/page.tsx` is the zone's landing page. It renders what
this zone can honestly report today, and it is not a placeholder:

- the gateway's own discovery state — available, degraded or absent — from
  `discovery().getServiceState('gateway', token)`;
- its reported version and capability keys;
- whether `gateway.chat.stream` is among them;
- the current scope from the switcher (§ 7.2);
- a stated note that the playground arrives with the follow-up issue (§ 12 item 1).

This is worth building rather than stubbing. It is the first exercise of ADR-0020 discovery **from
a second zone**, and `gateway.chat.stream` is the gateway's only capability
(`rs/crates/services/paigasus-gateway/src/service_info.rs:19-33`), reported only when
`stream_enabled` is set. The overview is therefore also the control that the follow-up issue's
playground will gate on: the gateway answers a streaming request with a 400 carrying
`param: "stream"` when the capability is off (`.../http/chat.rs:104-106`), and a console that
cannot see the capability would ship a control that always fails.

§ 10 tests all three discovery states, mirroring the `iam.audit` gating in SMA-511 § 6.6.

---

## 7. The shell, the switcher and the scope route

### 7.1 The shell (D7)

`(console)/layout.tsx` mirrors `iam-console`: `currentSession()`, `getPublicConfig()`,
`discovery()`, `myScopes()`, then `AppShell` with the primary navigation, the organization switcher
and the user menu, wrapped in `ZoneProvider` and a `LinkProvider` bound to `ZoneLink`.

`lib/nav.ts` builds two zone entries. Each is present only when `PAIGASUS_ZONES` names that zone,
and each carries `navStateOf(serviceState)` for its **service**. `iam-console`'s `lib/nav.ts`
already has the mirror-image wiring for the gateway (`ts/apps/iam-console/lib/nav.ts:28-33`), so
the cross-zone link appears in both directions as soon as the zone map names both.

### 7.2 The scope route (D8)

Selecting an organization navigates to `/gateway/orgs/<org>`, a same-zone navigation.
`(console)/orgs/[org]/page.tsx` renders the overview scoped to that organization, with the scope in
the breadcrumbs. The route exists now so the later settings screens hang off a shape that already
works, and so the switcher is not a dead control. § 13 records that the scope changes the URL and
the breadcrumbs and nothing else.

### 7.3 The 403 view

`(console)/forbidden.tsx` renders the 403 boundary with the correlation id read from the request
header, as `iam-console` does. It is reachable in this zone: a denied `Introspect`, `myScopes()` or
tenancy read raises it.

---

## 8. Parameterizing the three gates (D5)

Three gates name one app and therefore skip a second one in silence. The liveness half of each fix
is the load-bearing half: without it, this work buys coverage for one more app and re-creates the
same trap for the third zone.

### 8.1 `ci/next-env/run.sh` and `repo:next-env-drift`

Replace `APP='ts/apps/iam-console'` with discovery over `ts/apps/*/next.config.*`, the glob
`ci/next-public/run.sh:184` already uses, and loop. Fail when the discovered set is empty, and when
it does not include every `ts/apps/*` directory that has a `package.json` — a subset assertion,
not set-equality: a discovered app need not itself be checked against anything else.

**Correction to revision 1.** That revision said the gate's `--self-test` and `--negative-control`
arms "gain a second-app case". The script has neither: it is 82 lines with no flag parsing, and
`repo:next-env-drift` is a bare `script: 'ci/next-env/run.sh'` (root `moon.yml:135`). Adding those
arms is not free — `check_self_scheduled_coverage` in `ci/affected-graph/ci_targets.py` then
demands a `SELF_SCHEDULED_GATES["next-env-drift"]` entry, and the pairing rule demands a
`SELF_TASK_EXPECTED_GLOBS` entry or a reasoned `SELF_TASK_GLOBS_EXEMPT` one. **This design does not
add them.** The loop plus the subset assertion is the control, and § 13 records that this
gate has no negative control.

The task's `deps` gains `gateway-console-ts:build`. Its inputs widen **per path, not per tree**:
`ts/apps/*/next-env.d.ts`, `ts/apps/*/next.config.ts`, `ts/apps/*/tsconfig.json`,
`ts/apps/*/app/**/*`. A blanket `ts/apps/*/**` would sweep in `.next`, which
`.moon/workspace.yml`'s `hasher.ignorePatterns` deliberately does not ignore because `.next` is a
declared build output — `repo:next-public-free` negates it by hand for that reason
(root `moon.yml:921-926,939`).

### 8.2 `ci/tailwind-source/run.mjs`

Three corrections to revision 1, each of which would otherwise leave a gate that reports green
while checking nothing:

1. There are **three** sentinels, not two: `PROBE_SOURCE`, `PROBE_TOKEN` and `PROBE_APP_SHELL`
   (`run.mjs:20-24`, asserted at `:118-126`).
2. `verdict()` joins every CSS file into one string (`:117`). Run it **once per app**, with that
   app's own `cssFiles` and `appFiles`. A single call over the union would let a sentinel in
   `iam-console`'s CSS satisfy the assertion for `gateway-console`.
3. The guard takes the app directory as an argument, and **each app's own `test` task invokes it
   for itself**. Revision 1 implied one invocation looping over every app, which would give
   `iam-console-ts:test` a dependency on `gateway-console-ts:build` — a cross-app edge in the
   opposite direction from § 9's, and one that reads a missing `.next` (`currentBuildCssFiles`
   throws when `.next/static` is absent, `run.mjs:99`). Per-app invocation keeps every dependency
   local to the app that already builds.

**Correction found while planning.** `verdict()` is already app-agnostic: it takes `cssFiles` and
`appFiles` as arguments, and `selfTest()` and `negativeControl()` drive it over temporary fixtures
they build themselves. Their expected failure counts therefore do **not** change — the control's
3 stays 3. Only `realRun()` and the module-level `CONSOLE_DIR` are app-scoped, plus one failure
message that names `ts/apps/iam-console` in prose, which becomes a caller-supplied label.

**The liveness control for this one is new.** Per-app invocation means an app could simply never
invoke the guard. `ci/affected-graph/ci_targets.py` gains a check, run by `repo:affected-smoke`,
asserting that every `ts/apps/*` project's `test` script contains the guard's three invocation
lines, pinned as whole lines in the way `SELF_SCHEDULED_GATES` pins its members.

`ci/tailwind-source/run.mjs` must stay outside every app directory: an app directory is Tailwind's
scan root, and a script holding the sentinel literal would make Tailwind generate the utility the
guard asserts on.

### 8.3 `ts/eslint.config.js`

The Next rules become one block per app, so `settings.next.rootDir` stays correct for each. A test
asserts every `ts/apps/*` directory has a block.

### 8.4 `repo:next-public-free`

Revision 1 said this gate needs no change. Its `APP_CONFIG_GLOB` indeed needs none, but
`APP_CONFIG_FLOOR=1` carries the instruction "raise this when a second console zone app lands"
(`ci/next-public/run.sh:98-100`). Leaving it at 1 means the gate stays green if the gateway app's
config later disappears — the collapse the floor exists to detect. That literal line is pinned as a
whole line in `NEXT_PUBLIC_FREE_SH_CALL_SITES` (`ci/affected-graph/ci_targets.py:1219`), so raising
it to 2 and updating the pin must happen in the same commit, in pull request 3.

---

## 9. Moon, CI and the affected graph

- **`fileGroups.sources`** for the new app is `app/**/*`, `lib/**/*` and `proxy.ts`. The inherited
  group is `src/**/*`, which the app does not have.
- **`build`, `test`, `typecheck` and `test-e2e`** use `options.merge: replace` and list every input
  by hand: the app's own files, and the `src/**` and `package.json` of `console-core`, `auth`,
  `sdk`, `discovery`, `app-shell`, `proto`, `ui` and `next-config`, plus
  `next-config/tsconfig.app.json`, `/ts/pnpm-lock.yaml` and `/ts/tsconfig.base.json`.
- `build` asserts `.next/standalone/apps/gateway-console/server.js` and removes `.next/static`
  first, as `iam-console-ts:build` does.
- **`@paigasus/console-core` must be added to `SOURCE_ONLY_PACKAGES`**
  (`ts/packages/paigasus-next-config/src/index.ts:16`, whose comment says "this list grows when a
  new one lands"). A source-only package absent from `transpilePackages` hands raw TypeScript to
  Next and neither app builds. Editing `next-config/src/**` re-keys both apps' `build`, `test`,
  `typecheck` and `test-e2e`, so this reaches the affected-graph cases too.
- **`gateway-console-ts:test-e2e` lists `/ts/apps/iam-console/**/*` in its `inputs`**, with
  `!/ts/apps/iam-console/.next/**` and `!/ts/apps/iam-console/tests/fixtures/**/.next/**`.
  Revision 1 claimed the `iam-console -> gateway-console` edge came from the `deps` relation. It
  does not: CLAUDE.md records as measured that task `inputs` are the only thing conferring
  affectedness in Moon 2.5.3, and `dependsOn` and `^:build` schedule an upstream's build without
  ever selecting a downstream. `_assert_task_case_impl` runs a plain `moon query tasks --affected`
  with no graph flags (`ci/affected-graph/run.sh:142-163`), so the case would have reported an
  empty set and the two-zone tier — the only proof of ACs 1 and 2 — would never re-run on an
  `iam-console` change. **The cost is real and accepted: every `iam-console` edit now runs a
  Docker-backed two-zone Playwright tier.**
- `test-e2e` carries `options.cache: false` and depends on `~:build`, `iam-console-ts:build`,
  `repo:next-env-drift` and `contracts:generate`.
- `.github/workflows/ci.yml`'s Playwright Chromium install step names the new app.
- **CODEOWNERS.** `ci.yml:350-353` runs `moon sync code-owners` and `git diff --exit-code
  .github/CODEOWNERS` unconditionally. A new Moon project and a new package both change the
  generated file, which must not be hand-edited.
- `ts/pnpm-workspace.yaml` needs no change: `apps/*` and `packages/*` are globs.
- `ts/README.md`, `CONTRIBUTING.md` and `CLAUDE.md` record the new app, the new package, and the
  Docker requirement of the two-zone tier.
- No new catalog entry: `testcontainers` is already used by `@paigasus/auth` and
  `@paigasus/discovery`.
- The full repository-gate run happens before every push. Per-project tasks do not run the
  repository gates.

---

## 10. Testing

### 10.1 Test doubles

All live in `@paigasus/console-core/testing` (D10).

- **Fake IAM (gRPC and HTTP).** Carried over from `iam-console` unchanged: `Introspect` returns
  `identity-not-provisioned` until the principal has made one bearer-enforced call; `role_grants`
  is always empty; a denial is `Code.PermissionDenied` with an `ErrorInfo` detail carrying the
  correlation id; every handler counts its calls. Its `GET /v1/service-info` descriptor is
  scriptable, so a scenario can change the reported capabilities mid-test.
- **Fake gateway (HTTP only).** `GET /v1/service-info`, with a scriptable descriptor and a
  scriptable reachability failure. It serves **no** chat route: D11 removed the only consumer, and
  a fake endpoint with no product behind it is the vacuous-green shape § 2.3 refuses.
- **Fake IdP.** An in-process HTTPS server, because `authEnvShape` requires `https:` for
  `PAIGASUS_OIDC_ISSUER` and `PAIGASUS_PUBLIC_ORIGIN`. It counts its `/authorize` calls, and the
  counter is **readable as a snapshot**, so a test asserts a delta rather than an absolute (§ 10.5).
- **TLS certificate helper and TLS terminator.** The terminator gains a **path router**: it maps a
  path prefix to an upstream, so one terminator fronts both zones. It forwards `Host` unchanged and
  sets `X-Forwarded-Proto`.

### 10.2 Tier 1 — unit (vitest)

The config shape, including a `PAIGASUS_SERVICES` that lacks `gateway` or `iam`. The navigation
entries in every zone-map and service-state combination. The switcher's scope-in-URL behaviour. The
overview's rendering for each of the three discovery states and for the capability present and
absent. The error copy tables against the `ErrorReason` registry. That the logger writes only the
port's fields and never a DSN.

The vitest setup mirrors `iam-console`'s: `__NEXT_EXPERIMENTAL_AUTH_INTERRUPTS=true`, a
`server-only` stub, mocked `next/headers` and `next/cache`, and `ssr.resolve.conditions` set
alongside the top-level `resolve.conditions` — vitest 5 resolves a Node-environment test's imports
through the former, and setting only the latter has no effect.

### 10.3 Tier 2 — integration (vitest, fake IAM and fake gateway)

- Provisioning: the resolver calls `GetServiceInfo` before `Introspect`; a failed call degrades to
  `principalPrn: null` and logs `principal.resolve_failed`; `currentPrincipal()` retries once after
  `identity-not-provisioned`.
- `myScopes()`: memberships only, memberships plus grants, deduplication, and the cap.
- The overview against a scripted gateway descriptor: available, degraded and absent, and with
  `gateway.chat.stream` present and absent.
- A denied tenancy read raises the 403 boundary with the correlation id intact.

### 10.4 Tier 3 — e2e, single zone (`gateway-console-ts:test-e2e`, part 1)

A production build behind the standalone `server.js`, reached through the TLS terminator.
`PAIGASUS_SESSION_STORE=memory`, one zone in `PAIGASUS_ZONES`.

| Scenario | AC |
|---|---|
| `/gateway/` loads with no cookie, with its CSS and JS | shell |
| Unauthenticated `/gateway/` to the IdP and back, landing on the overview; the fake IAM saw `GetServiceInfo` before `Introspect` | 2 (partial) |
| A first-time, unprovisioned user reaches the same screen | 2 (partial) |
| The gateway reported degraded: the overview says so, and the nav entry is disabled with a reason rather than hidden | ADR-0020 |
| No response body — HTML, RSC payload or route-handler output — contains the access or refresh token | ADR-0017 |
| Sign out, then `/gateway/` redirects to login again | shell |

### 10.5 Tier 4 — e2e, two zones (`gateway-console-ts:test-e2e`, part 2)

A Redis container, both standalone servers, `PAIGASUS_ZONES={"iam":"/iam","gateway":"/gateway"}`,
`PAIGASUS_SESSION_STORE=redis`, and one terminator path-routing both.

The Redis container starts in the **worker-scoped fixture**, not in `globalSetup`: a Playwright
`globalSetup` runs in a different process from the tests, and Playwright starts a new worker after
a failed test, so the fixture must be able to bring the whole stack up again.

| Scenario | AC |
|---|---|
| Log in at `/iam`, navigate to `/gateway`, the overview renders, and the fake IdP's `/authorize` count is **unchanged across that navigation** | 1 |
| The cross-zone navigation is a hard navigation, and the same-zone one is not | ADR-0017 |
| Cold login at `/gateway` reaches the overview while **an instrumented listener in front of the iam-console server records zero inbound connections** | 2 |
| One page render makes exactly one fake-IAM `Introspect` call | § 5.3 |
| Static chunks do not collide: each zone loads its own `_next` assets under its own base path | SMA-513 AC 3 rehearsal |

**Two corrections to revision 1, both of which made a test unable to fail.**

*AC 1.* Revision 1 asserted the `/authorize` count was "exactly one". The harness fixture is
worker-scoped with `workers: 1`, and Playwright restarts the worker and the whole stack after a
failed test (`ts/apps/iam-console/tests/e2e/support/harness.ts:3-4,215-229`), so the counter is the
sum over every login that ran before in that worker. "Exactly one" is therefore flaky, and relaxing
it to "at least one" asserts nothing. The test snapshots the count before the cross-zone navigation
and asserts the delta is zero.

*AC 2.* Revision 1 stopped the `iam-console` process and asserted the login worked. The gateway
zone's cold-login path is proxy → `/gateway/auth/login` → IdP → `/gateway/auth/callback` → IAM
gRPC. The `iam-console` process is on none of it, running or stopped, so that test passes whether
or not the design has the property — and would keep passing against a design that grew a cross-zone
dependency. Asserting **zero inbound connections to the iam-console zone** fails if such a
dependency ever appears, which is what AC 2 actually claims.

**The spy cannot simply bind that port.** Both standalone servers run in this tier, so the
iam-console server already owns it. The listener is therefore a counting **forwarder**: the
terminator routes `/iam/*` to it, it records each connection and proxies to the real server. That
keeps the AC 1 scenario — which needs a working IAM zone — and the AC 2 scenario, which needs the
count, in one fixture. A plain bind-and-drop spy would work only if the IAM app were stopped, and
stopping it is the tautological test § 10.5 already rejected.

Revision 1 also carried a row asserting the IAM nav entry renders "degraded, not absent" when the
`iam-console` app is stopped. That row is deleted. `navStateOf` reads the discovery state of the
IAM **service**, which AC 2 requires to stay up, so the entry renders *available* and points at a
dead zone. A stopped zone app is invisible to ADR-0020 discovery; § 13 records it as a limit, and
§ 10.4 tests the degraded state by degrading a **service** instead.

**Docker.** The tier fails loudly when Docker is unreachable. There is no skip hatch, following the
precedent stated in `ts/packages/paigasus-discovery/vitest.containers.config.ts:11-12`. The
decision lives in the harness fixture, named in § 13.

### 10.6 AC 4

`build` asserts the standalone entry point. `tests/standalone-runtime.test.ts` boots the server with
a complete, valid environment and targets `GET /gateway/healthz`, and boots it again with a zone and
base-path mismatch, asserting the mismatch message in the server output rather than only a 500.

---

## 11. The deployment contract (for SMA-513)

| Group | Variables |
|---|---|
| Zone | `PAIGASUS_ZONE=gateway`, and a `PAIGASUS_ZONES` JSON **byte-identical in every zone** |
| Auth | the `authEnvShape` keys, with `PAIGASUS_SESSION_STORE=redis` **mandatory** |
| Discovery | `PAIGASUS_SERVICES` (JSON; must contain `iam` **and** `gateway`), optional `PAIGASUS_DISCOVERY_*_MS` |
| IAM | `PAIGASUS_IAM_GRPC_URL` |

**The cross-zone invariants, corrected.** Revision 1 named "the same cookie and crypto secret".
No such key exists: `authEnvShape` declares `PAIGASUS_ZONE`, `PAIGASUS_ZONES`, six
`PAIGASUS_OIDC_*` and six `PAIGASUS_SESSION_*` keys and nothing else
(`ts/packages/paigasus-auth/src/config.ts:40-61`). The session cookie holds an opaque id and the
record lives in the store; nothing is signed or encrypted client-side
(`src/http/cookies.ts:1-21`). SMA-513's Helm chart would have generated two secrets nothing reads.

What must actually match across zones:

- one `PAIGASUS_SESSION_REDIS_URL`, with the key prefix hardcoded empty so every zone reads the
  same records (`runtime.ts:146-147`);
- one `PAIGASUS_PUBLIC_ORIGIN`, because the cookie is host-only on that origin;
- one OIDC client, with one redirect URI per zone;
- an identical `PAIGASUS_ZONES` JSON.

`redis` is mandatory, not a recommendation: `createAuthRuntime` throws
`'PAIGASUS_SESSION_STORE cannot be "memory" when PAIGASUS_ZONES declares more than one zone'`
(`ts/packages/paigasus-auth/src/runtime.ts:112`). A two-zone deployment on the memory store cannot
start, which is intended — under it a user signed in on one zone is anonymous on the other.

Further assumptions: the ingress forwards `Host` or `X-Forwarded-Host` unchanged; the OIDC client
registers `https://<origin>/gateway/auth/callback` and `https://<origin>/gateway/` alongside the
IAM zone's pair; the console reaches IAM's gRPC port from inside the cluster; and IAM's
`authn.issuers[].audiences` accepts this client's token audience.

---

## 12. Follow-ups

Linear issues to create after this spec is approved:

1. **The gateway's interactive-user authentication path, and the chat playground** (D11, § 2.3).
   It owns widening `require_iam_auth` past API keys, deciding the authorization resource for a
   user principal — an API key has a `scope_prn`, a user has none, and D8's switcher scope is the
   natural candidate — and only then the console playground with its streaming passthrough. It
   likely needs an ADR. This issue does not block on it.
2. **Organization and project settings screens in the gateway zone** — what D8's route shape exists
   for.
3. **A negative control for `repo:next-env-drift`** (§ 8.1), with the three registry obligations
   that come with it.

---

## 13. Recorded limits and departures

- AC 3 is not delivered here (D11, § 2.3). AC 4 is the standalone build and the `NEXT_PUBLIC_`
  gate, not an OCI image (§ 2.2).
- AC 2 concerns the `iam-console` **app**, not the IAM **service**, which must be reachable.
- **A stopped zone app is invisible to ADR-0020 discovery.** Discovery probes services, not zone
  apps, so a zone whose app is down still renders an *available* nav entry pointing at a dead
  route. Nothing in this design detects that, and SMA-513's ingress is where it would be caught.
- `gateway-console-ts:test-e2e` **requires Docker**, and fails loudly without it. `iam-console`'s
  tier does not need Docker. The memory store is refused with two zones, so there is no
  alternative.
- **Every `iam-console` edit now runs the Docker-backed two-zone tier** (§ 9). That is the price of
  making the tier re-run when the property it tests can break.
- The organization switcher runs `myScopes()` on every gateway page render — one `Introspect`, one
  `ListRoleGrants` and up to 50 tenancy reads. SMA-511 § 12 records that cost as unmeasured for the
  IAM zone. This design pays it twice, on decision D7, because the gateway gets organization and
  project settings later.
- The switcher's scope changes the URL and the breadcrumbs and nothing else (D8).
- `repo:next-env-drift` still has **no negative control** (§ 8.1). Its correctness rests on the
  loop and the subset assertion. Follow-up § 12 item 3.
- A lost `cache()` memoization is observable only in the e2e tier (§ 5.3).
- `@paigasus/console-core` exports a testing surface from a package that is not a test package.
- The PRN reader stays an ADR-0005 exception, now at one site instead of two. SMA-634 owns the
  underlying napi packaging defect.

---

## 14. Things the plan must measure before building on them

1. **Whether one TLS terminator can path-route to two Next standalone servers** with `Host` and
   `X-Forwarded-Proto` intact, and whether the two apps' static chunks collide under one origin.
   This rehearses SMA-513 AC 3.
2. Whether `testcontainers` starts Redis from a **Playwright worker fixture** — not from a vitest
   config, which `@paigasus/auth` and `@paigasus/discovery` already prove works — and what a worker
   restart does to a container that is already running.
3. Whether pnpm resolves `@paigasus/console-core/testing`'s devDependencies for a consuming app.
4. Whether adding `/ts/apps/iam-console/**/*` to `gateway-console-ts:test-e2e`'s `inputs` actually
   makes `moon query tasks --affected` select the tier from an `iam-console` edit, measured with the
   unpiped exit status. This is the fix for revision 1's wrong `dependsOn` claim, and it must be
   confirmed rather than assumed.
5. The wall-clock cost of the two-zone tier on a loaded CI runner. The `iam-console` tier already
   needs a 420 s worker timeout in CI (`tests/e2e/support/harness.ts:226-228`), and this tier runs
   two servers and a container.
6. Whether `boundaries.test.ts`'s reverse-liveness loop accepts the derived scope key
   `packages/paigasus-console-core/src` (§ 5.5).

---

## 15. Challenge log (revision 1 → 2)

The spec-challenger (Opus) returned **NEEDS REWORK**: 5 BLOCKER, 14 MAJOR, 8 MINOR, 6 QUESTION.
Seven claims were verified against the code by hand before any was folded in; all seven held.

| Finding | Verdict | Where |
|---|---|---|
| BLOCKER — the gateway chat route rejects an OIDC token, so AC 3 is unbuildable and every tier would pass vacuously | confirmed at `auth.rs:64,146,872-879` | D11, § 2.3 |
| BLOCKER — `proxy.ts` redirects the cookie-less chat POST, so "it never redirects" was false | confirmed at `middleware.ts:107,117,122-135` | § 4 |
| BLOCKER — `ChatResult`'s third arm, which carries every gateway error, was omitted | confirmed | removed with D11 |
| BLOCKER — the reverse boundary rule cannot be a `packages/**` block, and console-core reopens the middleware hole | confirmed at `eslint.mjs:301-320,376-379` | § 5.5 |
| BLOCKER — `dependsOn` does not confer affectedness, so the two-zone tier would never re-run | confirmed against CLAUDE.md and `run.sh:142-163` | § 9 |
| MAJOR — `ci/next-env/run.sh` has no self-test arms, and adding them costs three registry entries | confirmed: 0 occurrences, 82 lines | § 8.1, § 12 |
| MAJOR — `APP_CONFIG_FLOOR=1` must be raised, and it is pinned | confirmed at `run.sh:100`, `ci_targets.py:1219` | § 8.4 |
| MAJOR — three sentinels not two; `verdict()` joins the CSS; the invoking task | confirmed at `run.mjs:20-24,117-126,99` | § 8.2 |
| MAJOR — widening next-env inputs to `ts/apps/*/**` hashes `.next` | confirmed | § 8.1 |
| MAJOR — the "degraded not absent" row cannot pass | confirmed | § 10.5, § 13 |
| MAJOR — the AC 2 test was vacuous by construction | confirmed | § 10.5 |
| MAJOR — the AC 1 `/authorize` count is worker-scoped | confirmed at `harness.ts:3-4,215-229` | § 10.1, § 10.5 |
| MAJOR — § 11 named a cookie and crypto secret that do not exist | confirmed at `config.ts:40-61` | § 11 |
| MAJOR — no capability gate on `gateway.chat.stream` | confirmed | § 6 |
| MAJOR — no abort plumbing for the stop control | confirmed | removed with D11 |
| MAJOR — pull request 2 re-baselines six pinned expected sets | confirmed | § 3.1 |
| MAJOR — `principal-prn.ts` and `lib/auth.ts` missing from the move table | confirmed at `principal-prn.ts:5-12` | § 5.2 |
| MAJOR — `cache()` identity is load-bearing, unpinned and untestable at the unit tier | confirmed at `lib/iam.ts:7-8,21,52` | § 5.3, § 10.5 |
| MAJOR — console-core must join `SOURCE_ONLY_PACKAGES` | confirmed at `index.ts:16` | § 9 |
| MINOR — the resolver's signature was unstated | justified | § 5.3 |
| MINOR — measurement 4 was already answered for vitest | justified | § 14 item 2 |
| MINOR — Docker-absent behaviour unstated | justified | § 10.5, § 13 |
| MINOR — CODEOWNERS regeneration missing | confirmed at `ci.yml:350-353` | § 9 |
| MINOR — the stream-identity assertion may not hold | moot | removed with D11 |
| MINOR — the AC 3 negative control was not located | moot | removed with D11 |
| MINOR — SSE compression unaddressed | moot | removed with D11 |
| MINOR — line reference `runtime.ts:105-113` had drifted | confirmed | § 11 cites `:112` |
| MINOR — drop `@paigasus/proto` from console-core's allowlist | justified, with the testing-subpath exception | § 5.5 |
| QUESTION — how the playground authenticates against a real gateway | answered by D11 | § 2.3 |
| QUESTION — a mandatory default model with no model list | moot: the key is gone | § 4.1 |
| QUESTION — does `iam-console-ts:test` gain a dep on the gateway build | yes under revision 1's shape; per-app invocation removes it | § 8.2 |
| QUESTION — where the Redis container starts, and the tier's cost | answered; cost is measured | § 10.5, § 14 item 5 |
| QUESTION — what stops an app's `lib/` importing `./testing` | nothing did; boundary rule 3 added | § 5.5 |
| QUESTION — who signs off the AC 2 reading | Sven, at the spec gate; the test no longer relies on it being tautological | § 2.2, § 10.5 |
