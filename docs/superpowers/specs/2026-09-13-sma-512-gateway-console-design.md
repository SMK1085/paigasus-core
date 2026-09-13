# SMA-512 — `gateway-console`: the AI Gateway zone

**Status:** draft, revision 1 (before the adversarial challenge)
**Date:** 2026-09-13
**Issue:** [SMA-512](https://linear.app/smaschek/issue/SMA-512/ts-gateway-console-app-the-ai-gateway-zone)
**ADR:** ADR-0017 — Console topology & session ownership; ADR-0005 (kernel owns PRN logic);
ADR-0018 (Connect-ES transport); ADR-0019 (error model); ADR-0020 (capability discovery)
**Design:** Frontend Architecture Scoping §§ 1, 2, 3, 4.4, 6
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

This issue adds the second zone and proves both. It also gives the gateway its operator surface: a
chat playground that streams through a Next route handler against the gateway's
OpenAI-compatible endpoint.

### 1.1 Acceptance criteria (from the issue)

1. A user already logged in through the IAM zone reaches this zone with no re-authentication.
2. With the IAM zone stopped, a cold user can still log in here.
3. Streaming responses arrive incrementally. Nothing in middleware consumes or buffers the body.
4. The app builds to a standalone image, and the `NEXT_PUBLIC_` gate stays green.

§ 2.2 records how this design reads AC 2 and AC 4, and why.

---

## 2. Scope and decisions

### 2.1 Decisions (Sven, 2026-09-13)

| # | Question | Decision |
|---|---|---|
| D1 | Where the two-zone proof lives | A full two-zone e2e tier in this issue. Both standalone servers run behind one TLS terminator, over one shared session store. |
| D2 | SMA-631 — the shared home | Make it now: a new server-only package that may depend on both `@paigasus/auth` and `@paigasus/sdk`. `iam-console` moves onto it in the same change, so a second copy never exists. |
| D3 | The playground's product surface | One chat page. Streaming output, a stop control, conversation state in React only. No server-side history. |
| D4 | The app name | `ts/apps/gateway-console`, `@paigasus/gateway-console`, Moon id `gateway-console-ts`. Symmetric with `iam-console`. The issue text predates SMA-511's rename. |
| D5 | The three app-scoped gates | Parameterize them over `ts/apps/*`, and add a liveness assertion so an uncovered app fails CI instead of being skipped. |
| D6 | The model id | An operator-configured default pre-fills the field. The user may type any other id. |
| D7 | The shell | Full parity with `iam-console`, the organization switcher included. Reason: the gateway gets organization and project settings later, so the switcher must exist before those screens do. |
| D8 | What the switcher does today | Selection writes the scope into the URL (`/gateway/orgs/<org>`). The playground reads it and sends nothing extra to the gateway. The route shape the later settings screens hang off exists from day one. |
| D9 | The shared package's shape, and the e2e tier's home | One package taking its dependencies as arguments (§ 5). The two-zone tier lives inside `gateway-console-ts:test-e2e` (§ 10.5). |
| D10 | The test doubles | Move the fake IAM, the fake IdP and the TLS helpers out of `iam-console` into the shared package's testing subpath, and add the new fake gateway beside them. One copy, not three. |

### 2.2 How the acceptance criteria are read

- **AC 2 — "with the IAM zone stopped".** This means the `iam-console` **app** is stopped. It does
  not mean the IAM **service** is stopped. D7 puts the organization switcher in this zone, so the
  app calls IAM for `myScopes()`. Provisioning and `Introspect` need IAM as well
  (§ 6.2). A cold login at `/gateway` therefore needs the IAM service, and needs no other console
  app. That is the property ADR-0017 claims, and it is what the test asserts.
- **AC 4 — "a standalone image".** SMA-513's scope says "Console images following the conventions
  established for the services in SMA-500", so the OCI image belongs there. This issue proves the
  `output: 'standalone'` build and the `NEXT_PUBLIC_` ban. SMA-511 read the identical wording the
  same way (its spec § 2.2).

After this spec is approved, the AC text in Linear is updated for AC 2 and AC 4, each with its
reason.

### 2.3 In scope

- The new app (§ 4), the playground and the streaming passthrough (§ 6), the shell and the scope
  route (§ 7).
- `@paigasus/console-core` (§ 5), and the refactor of `iam-console` onto it.
- Parameterizing the three app-scoped gates (§ 8).
- Moon, CI and affected-graph registration (§ 9).
- Four test tiers (§ 10), including the two-zone tier.
- The deployment contract for SMA-513 (§ 11).

### 2.4 Out of scope

- The console OCI image and the ingress (SMA-513).
- Server-side prompt history, a model picker, and usage analytics (D3; the Frontend Architecture
  Scoping document does not scope the playground's product surface).
- Organization and project **settings screens** in the gateway zone. D8 delivers the route shape
  only.
- A `/v1/models` route in `paigasus-gateway`. The gateway has no model list and no allowlist
  today; `model` passes through to the upstream unchanged.
- Any change to the gateway service itself.

---

## 3. The change is delivered as four pull requests

The work splits at four clean seams. Each pull request is independently green, and each closes
something on its own. The order is fixed by dependency, smallest first.

| # | Pull request | What it delivers | What proves it |
|---|---|---|---|
| 1 | **Parameterize the app-scoped gates** (§ 8) | `ci/next-env`, `ci/tailwind-source` and `ts/eslint.config.js` stop naming one app. Liveness assertions added. | The three gates stay green on one app, and their self-test and negative-control arms cover a synthetic second app. |
| 2 | **`@paigasus/console-core`** (§ 5) | The package, its boundary rule, the testing subpath, and `iam-console` refactored onto both. Closes SMA-631. | **No behaviour change.** `iam-console`'s existing unit, integration and e2e suites stay green unchanged in intent. |
| 3 | **The `gateway-console` app** (§ 4, 6, 7) | The app, the shell, the playground, the passthrough, and a single-zone e2e tier. | AC 3 and AC 4 in full. A cold login at `/gateway` in a single-zone configuration. |
| 4 | **The two-zone tier** (§ 10.4) | The Redis container, the path-routing terminator, both servers, and the `iam-console -> gateway-console` affected-graph edge. | AC 1 and AC 2 in their real form. |

**Why split.** Pull request 2 touches every test file in `iam-console` while changing no
behaviour. Mixed into a new app, a reviewer cannot tell a refactor defect from a new-app defect.
Pull request 1 is CI-only and orthogonal to both. Pull request 4 is the first time the TypeScript
end-to-end path needs Docker, so an isolated failure there is unambiguous. Each pull request also
re-baselines a smaller set of the strict-equality cases in `ci/affected-graph/run.sh`.

**The risk of splitting, stated plainly.** Pull request 2 designs a shared package with only one
consumer, so it can bake in an `iam-console`-shaped interface. Two things hold against that: this
spec fixes the interface before the code is written (§ 5), and pull request 3 is the first
consumer that would expose a wrong shape. If pull request 3 has to change the package's
interface, that is a signal the split hid something, and it is recorded rather than absorbed
quietly.

**The cost.** `main` requires strict up-to-date branches, so the four merges are sequential and
each blocks the next. Four full CI runs and four review loops replace one.

---

## 4. The app

`ts/apps/gateway-console`, package `@paigasus/gateway-console`, Moon id `gateway-console-ts`,
`layer: application`.

`next.config.ts` calls `createNextConfig({ zone: 'gateway', basePath: '/gateway',
outputFileTracingRoot, extend: { experimental: { authInterrupts: true } } })`. The factory forces
`output: 'standalone'` and adds every source-only `@paigasus/*` package to `transpilePackages`.
`authInterrupts` is needed for the same reason as in `iam-console`: without it `forbidden()`
throws instead of rendering the 403 boundary.

```
ts/apps/gateway-console/
  proxy.ts                        cookie presence and the correlation id; never reads a body
  next.config.ts  postcss.config.mjs  tsconfig.json  vitest.config.ts  playwright.config.ts
  next-env.d.ts                   tracked, and now covered by repo:next-env-drift (§ 8)
  app/
    layout.tsx  globals.css  error.tsx  global-error.tsx  providers.tsx
    healthz/route.ts              public; returns { zone, zones }
    auth/[...auth]/route.ts       createAuthRouteHandler, mounted at /gateway/auth/*
    api/chat/route.ts             POST; the streaming passthrough (§ 6.3)
    (public)/layout.tsx  page.tsx
    (console)/layout.tsx  error.tsx  forbidden.tsx
    (console)/page.tsx            the playground
    (console)/orgs/[org]/page.tsx the scope route (§ 7.2)
  lib/
    config.ts                     the app's one defineRuntimeConfig call
    console.ts                    the app's one createConsoleRuntime call
    nav.ts                        the zone navigation entries
  tests/  unit/  integration/  e2e/  support/
```

`proxy.ts` imports only `@paigasus/auth/middleware`. Its public paths are the auth routes, `/` and
`/healthz`, all base-path relative. It exports a `config.matcher` that excludes `_next/static`,
`_next/image` and `favicon.ico`, for the reason SMA-511 measured: without a matcher the default is
a catch-all, and a visitor with no cookie gets a login redirect for every CSS and JS file.

### 4.1 `lib/config.ts`

One `defineRuntimeConfig` call composes four parts:

- `authEnvShape` from `@paigasus/auth/server`;
- `discoveryEnvShape` from `@paigasus/discovery/server`, refined so `PAIGASUS_SERVICES` must
  contain **both** `iam` and `gateway`;
- `PAIGASUS_IAM_GRPC_URL`, the same app-owned key `iam-console` declares, validated as an absolute
  `http:` or `https:` URL with no credentials, query or fragment;
- two new app-owned keys, `PAIGASUS_GATEWAY_URL` and `PAIGASUS_GATEWAY_DEFAULT_MODEL`.

`PAIGASUS_GATEWAY_URL` is separate from the `gateway` entry in `PAIGASUS_SERVICES` for the same
reason `PAIGASUS_IAM_GRPC_URL` is separate from the `iam` entry: discovery's map holds the address
that answers `GET /v1/service-info`. For the gateway those two happen to be the same HTTP port
today, because the gateway runs no gRPC server. The keys stay separate so a future split needs no
migration. The plan states this explicitly in the key's doc comment, so it does not read as an
oversight.

`PAIGASUS_GATEWAY_DEFAULT_MODEL` is a non-empty string with no surrounding whitespace. It is not
validated against any list, because no list exists.

---

## 5. `@paigasus/console-core`

`ts/packages/paigasus-console-core`. Source-only, `private: true`, `exports` map with two entries.
Every file under `src/` begins with `import 'server-only'`.

### 5.1 Why it exists

`iam-console` holds the provisioning call, `IntrospectPrincipalResolver`, `currentPrincipal()`,
`mayI()`, `myScopes()`, the IAM client accessors and the PRN reader in `lib/`. They live in the app
because `@paigasus/auth` must not import `@paigasus/sdk`
(`ts/packages/paigasus-auth/src/ports/principal-resolver.ts:3-8`), and the `sdk` boundary rule
bans every `@paigasus/*` import except `proto`. Only an app may depend on both. SMA-631 recorded
that this stops being true the moment a second zone needs the same code.

A subpath on either existing package cannot work: it would break one of those two rules. A new
package is the only shape that holds them.

### 5.2 What moves

| From `ts/apps/iam-console/lib/` | Note |
|---|---|
| `iam.ts`, `iam-clients.ts` | `currentSession`, `optionalSession`, `sessionToken`, `iamClients`, `iamClientsForAction` |
| `principal.ts`, `principal-resolver.ts` | provisioning, the resolver, `currentPrincipal()` |
| `authorize.ts` | `mayI()` |
| `scopes.ts` | `myScopes()` |
| `errors.ts` | the `callIam` wrapper and the presentation mapping |
| `logger.ts` | the JSON-lines adapter |
| `discovery.ts` | the discovery handle and the Redis descriptor cache |
| `correlation.ts`, `correlation-header.ts` | the correlation header names and the reader |
| `prn-tenancy.ts`, `tenancy-path.ts` | the PRN reader and the path helpers |

`discovery.ts` moves although it does not touch the `auth`/`sdk` conflict, because both zones need
byte-identical code and the code is subtle: `createRedisDescriptorCache` asserts four
preconditions on the client (`disableOfflineQueue`, an `error` listener, `commandOptions.timeout`
and `socket.socketTimeout`), the listener must never log the error object because node-redis
embeds the DSN in it, and a failed `connect()` must degrade to the `cache-unavailable` reason
rather than fall back to a memory cache in silence. A second hand-written copy would get one of
those wrong.

`tests/unit/prn-tenancy.test.ts` moves with the reader. The reader is a recorded ADR-0005
exception, held to the Rust kernel by the parity corpus. One exception with one parity test is
defensible; two copies of an ADR exception are not.

The logger has two halves and only one moves. The JSON-lines **adapter** moves, because both zones
write the same shape to stdout. The app still constructs it and passes it to
`createConsoleRuntime`, because the app owns composition. `appEvent(name, fields)` stays on the
adapter's own interface, since `AuthEventName` is a closed union and an app event cannot go
through it.

**What stays in each app:** `config.ts` (the key sets differ per zone), `nav.ts` (the entries
differ per zone), `form.ts` and `paging.ts` (IAM screen helpers with no gateway consumer), and
every React component. Pulling React in would make the package
client-reachable and would compromise the `server-only` guard that is the whole point of it.

### 5.3 The interface

The package reads no environment. An app passes **thunks**, so the factory is safe to call at
module scope and no configuration is read during `next build`:

```ts
createConsoleRuntime({
  authRuntime: () => Promise<AuthRuntime>,
  iamGrpcUrl: () => string,
  logger: ConsoleLogger,
}): ConsoleRuntime
```

`ConsoleRuntime` exposes `currentSession`, `optionalSession`, `sessionToken`, `iamClients`,
`iamClientsForAction`, `currentPrincipal`, `mayI`, `myScopes` and `callIam`. Each per-request
accessor keeps the React `cache()` memoization it has today.

`createIntrospectPrincipalResolver` is exported **separately**, not from that factory. It cannot
come from the console runtime: the console runtime takes the auth runtime, and the auth runtime
takes the resolver. Building it inside would be circular.

The PRN reader and the path helpers are pure functions and are exported directly.

### 5.4 One small change to `@paigasus/auth`

`iam-console/lib/auth.ts` re-declares the session cookie name as `SESSION_COOKIE_NAME =
'__Host-pgs_sid'`, because `@paigasus/auth` defines it at `src/http/cookies.ts:21` but exports it
from no public entry — the package's `exports` map has only `./server`, `./client` and
`./middleware`. A second app would make a third copy of a security-relevant constant that must
agree across every zone or the shared session silently stops working.

`@paigasus/auth` therefore exports `SESSION_COOKIE` from `./server`, and both apps use it. This is
the only change this issue makes to `@paigasus/auth`. It belongs in pull request 2 with the rest of
the deduplication.

### 5.5 The boundary rule

`@paigasus/next-config/eslint`'s `boundaryRules` gains one block: files under
`packages/paigasus-console-core/src/**` may import `@paigasus/auth/*`, `@paigasus/sdk/*` and
`@paigasus/proto`. Every other package keeps its current bans, so this is the one hole and it is
named.

The reverse direction matters as much: only `apps/*` may import `@paigasus/console-core`. A
package that imported it would inherit the hole.

`ts/packages/paigasus-next-config/tests/boundaries.test.ts` gains rows for both directions, and
the file's reverse-liveness loop must keep passing — the loop is why the rule goes in
`boundaryRules` and not in `sourceRules`.

### 5.6 The testing subpath (D10)

`@paigasus/console-core/testing` exports the fake IAM (gRPC and the `GET /v1/service-info`
handler), the fake IdP, the TLS certificate helper, the TLS terminator, and the new fake gateway
(§ 10.1). Both apps import it.

This is not the pattern SMA-511 refused. That rule was "one package's tests must not import
another package's tests". This is a package exporting a testing surface, which has a stable name
and a stable interface.

`testcontainers`, `jose` and `@connectrpc/connect-node` are **devDependencies** of the package, so
neither app carries them in its production dependency graph. § 14 item 5 measures that pnpm
resolves them for a consuming app.

The testing subpath is not guarded by `server-only`, because vitest and Playwright harnesses
import it outside a Next server. Nothing in `src/` imports it.

---

## 6. The playground and the streaming passthrough

### 6.1 The page

`(console)/page.tsx` renders the playground: a model field pre-filled from
`PAIGASUS_GATEWAY_DEFAULT_MODEL`, a message composer, the transcript, and a stop control. The
transcript lives in React state. Nothing is persisted anywhere (D3).

The page is a server component that passes the default model and the current scope to a client
component. The client component owns the stream.

### 6.2 Why the gateway needs a provisioned principal

`POST /v1/chat/completions` runs `require_iam_auth`
(`rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs:93`), which authenticates the
bearer against IAM and then performs a D9 self-query authorization for `InvokeModel`
(`adapters/http/auth.rs:94-124`). IAM returns `identity-not-provisioned` for an identity it has
never seen, and it provisions a principal only inside a bearer-enforced RPC.

So a cold user who logs in at the gateway zone must be provisioned before the first chat call, and
the provisioning call is `ServiceInfoService.GetServiceInfo` — bearer-enforced, and checking no
Cedar action. That is the same sequence `iam-console` runs, and it is exactly the code D2 moves
into the shared package. AC 2 fails without it.

### 6.3 The route handler

`app/api/chat/route.ts` is the only place a token approaches the wire. It:

1. resolves the session with `optionalSession`, **not** `currentSession`;
2. on no session, returns HTTP 401 carrying the gateway's own OpenAI error envelope;
3. otherwise calls `createChatClient` from `@paigasus/sdk/chat` with the bearer and
   `PAIGASUS_GATEWAY_URL`;
4. on a `text/event-stream` answer, returns the **unread** `ReadableStream` in a `Response` with
   `content-type: text/event-stream` and `cache-control: no-cache, no-transform`;
5. on a JSON answer, returns it unchanged.

It sets `export const runtime = 'nodejs'` and `export const dynamic = 'force-dynamic'`.

Two points are deliberate and easy to get wrong.

**It never redirects.** A `fetch` POST that receives a 307 to an HTML login page produces a
confusing failure in the browser and a nonsense entry in the transcript. A 401 with the error
envelope lets the page show a sign-in link. This is why the handler uses `optionalSession`.

**It never reads the body.** The SDK returns the raw `ReadableStream` for exactly this purpose
(`ts/packages/paigasus-sdk/src/chat.ts:275`) and bounds only the wait for response headers, never
the body. The handler passes the same object through. `proxy.ts` reads no body either. Section 14
item 2 measures whether Next's standalone server preserves that.

The gateway's one terminal error frame (`code: "upstream-error"`) is not parsed by the handler.
The SDK exposes `createTerminalFrameParser` for a caller that wants it, deliberately not
auto-wired, so that the SDK never buffers the passthrough. The **client component** parses it as
it renders, which is the only place the frames are already being read.

---

## 7. The shell, the switcher and the scope route

### 7.1 The shell (D7)

`(console)/layout.tsx` mirrors `iam-console`: `currentSession()`, `getPublicConfig()`,
`discovery()`, `myScopes()`, then `AppShell` with the primary navigation, the organization
switcher and the user menu, wrapped in `ZoneProvider` and a `LinkProvider` bound to `ZoneLink`.

`lib/nav.ts` builds two zone entries. The IAM entry is present only when `PAIGASUS_ZONES` has an
`iam` key, and it carries `navStateOf(iamServiceState)`. The gateway entry is this zone's own. An
IAM-only deployment has no gateway zone and an gateway-only deployment has no IAM zone entry; the
absent state renders nothing, and the degraded state renders a disabled entry with a reason
(Frontend Architecture Scoping § 3.3).

`iam-console`'s `lib/nav.ts` already has the mirror-image wiring for the gateway
(`ts/apps/iam-console/lib/nav.ts:28-33`), so the cross-zone link appears in both directions as
soon as the zone map names both.

### 7.2 The scope route (D8)

Selecting an organization in the switcher navigates to `/gateway/orgs/<org>`, a same-zone
navigation. `(console)/orgs/[org]/page.tsx` renders the playground with that scope shown in the
breadcrumbs. It sends nothing extra to the gateway, because the gateway's chat surface takes no
tenancy parameter.

The route exists now so the later settings screens hang off a shape that already works, and so
the switcher is not a dead control. This is recorded as a deliberate near-term limit in § 13.

### 7.3 The 403 view

`(console)/forbidden.tsx` renders the 403 boundary with the correlation id read from the request
header, in the same way `iam-console` does. The gateway's own denial for `InvokeModel` arrives as
an OpenAI-envelope error from the chat route, not as a `forbidden()` interrupt, so the two paths
are distinct and both are tested (§ 10.2, § 10.3).

---

## 8. Parameterizing the three gates (D5)

Three gates name one app and therefore skip a second one in silence. Each becomes app-agnostic
and each gains a liveness assertion. The liveness half is the load-bearing half: without it, this
work buys coverage for one more app and re-creates the same trap for the third zone.

| Gate | Change |
|---|---|
| `ci/next-env/run.sh` | `APP='ts/apps/iam-console'` becomes discovery over `ts/apps/*/next.config.*`, the glob `ci/next-public/run.sh:184` already uses. The check loops over the discovered set. It fails when the set is empty, and when it does not equal the set of `ts/apps/*` directories. The `--self-test` and `--negative-control` arms gain a second-app case. |
| root `moon.yml`, `repo:next-env-drift` | `deps` gains `gateway-console-ts:build`. The four `ts/apps/iam-console/**` inputs widen to `ts/apps/*/**`. |
| `ci/tailwind-source/run.mjs` | `CONSOLE_DIR` becomes the discovered set. Every app must carry both sentinels. Assertion 3's full-directory scan runs once per app. |
| `ts/eslint.config.js:53-64` | The Next rules become one block per discovered app, so `settings.next.rootDir` stays correct for each. A test asserts every `ts/apps/*` directory has a block. |

`repo:next-public-free` needs no change: its `APP_CONFIG_GLOB` is already generic, which is why AC
4's gate passes for a new app with no registry work.

`ci/tailwind-source/run.mjs` must stay outside every app directory, because an app directory is
Tailwind's scan root and a script holding the sentinel literal would make Tailwind generate the
utility the guard asserts on.

---

## 9. Moon, CI and the affected graph

- **`fileGroups.sources`** for the new app is `app/**/*`, `lib/**/*` and `proxy.ts`. The inherited
  group is `src/**/*`, which the app does not have, so without this an edit in `lib/` serves a
  cached pass.
- **`build`, `test` and `test-e2e`** use `options.merge: replace` and list every input by hand:
  the app's own files, and the `src/**` and `package.json` of `console-core`, `auth`, `sdk`,
  `discovery`, `app-shell`, `proto`, `ui` and `next-config`, plus
  `next-config/tsconfig.app.json`, `/ts/pnpm-lock.yaml` and `/ts/tsconfig.base.json`.
- `build` asserts `.next/standalone/apps/gateway-console/server.js` exists, and removes
  `.next/static` first, as `iam-console-ts:build` does.
- `test-e2e` carries `options.cache: false` and depends on `~:build`, `iam-console-ts:build`,
  `repo:next-env-drift` and `contracts:generate`.
- **`ci/affected-graph/run.sh`.** `gateway-console-ts:{build,test,test-e2e}` joins the `auth`,
  `sdk`, `discovery`, `app-shell`, `proto`, `ui` and `next-config` cases. New cases: a two-anchor
  `console-core -> both apps` case, an app-local gateway case with two anchors, and an
  `iam-console -> gateway-console` edge, which exists only because of the `test-e2e` build
  dependency. `contracts->proto` gains `gateway-console-ts`.
- `.github/workflows/ci.yml`'s Playwright Chromium install step names the new app.
- `ts/pnpm-workspace.yaml` needs no change: `apps/*` and `packages/*` are globs.
- `ts/README.md`, `CONTRIBUTING.md` and `CLAUDE.md` record the new app, the new package, and the
  Docker requirement of the two-zone tier.
- No new catalog entry: `testcontainers` is already used by `@paigasus/auth` and
  `@paigasus/discovery`.
- The full repository-gate run happens before every push, as CLAUDE.md requires. Per-project tasks
  do not run the repository gates.

---

## 10. Testing

### 10.1 Test doubles

All live in `@paigasus/console-core/testing` (D10).

- **Fake IAM (gRPC and HTTP).** Carried over from `iam-console` unchanged: `Introspect` returns
  `identity-not-provisioned` until the principal has made one bearer-enforced call; `role_grants`
  is always empty; a denial is `Code.PermissionDenied` with an `ErrorInfo` detail carrying the
  correlation id; every handler counts its calls.
- **Fake IdP.** An in-process HTTPS server, because `authEnvShape` requires `https:` for
  `PAIGASUS_OIDC_ISSUER` and `PAIGASUS_PUBLIC_ORIGIN`. It counts its `/authorize` calls, which is
  what makes AC 1 assertable.
- **Fake gateway (new).** A `node:http` server serving `GET /v1/service-info` and
  `POST /v1/chat/completions`. The chat handler emits SSE frames spaced by a configurable delay,
  and can be scripted to emit the terminal `upstream-error` frame, to answer a non-stream JSON
  body, or to deny.
- **TLS certificate helper and TLS terminator.** The terminator gains a **path router**: it maps a
  path prefix to an upstream, so one terminator fronts both zones. It forwards `Host` unchanged
  and sets `X-Forwarded-Proto`.

### 10.2 Tier 1 — unit (vitest)

The config shape, including a `PAIGASUS_SERVICES` that lacks `gateway`, and an empty or
whitespace-only default model. The navigation entries in every zone-map and service-state
combination. The switcher's scope-in-URL behaviour. The terminal-frame parsing in the client
component. The error copy tables against the `ErrorReason` registry. That the logger writes only
the port's fields and never a DSN.

The vitest setup mirrors `iam-console`'s: `__NEXT_EXPERIMENTAL_AUTH_INTERRUPTS=true`, a
`server-only` stub, mocked `next/headers` and `next/cache`, and `ssr.resolve.conditions` set
alongside the top-level `resolve.conditions` — vitest 5 resolves a Node-environment test's imports
through the former, and setting only the latter has no effect.

### 10.3 Tier 2 — integration (vitest, fake gateway and fake IAM)

- The chat route handler returns the **same stream object** it received from the SDK, so the
  passthrough is structural rather than asserted by timing alone.
- A stale session yields HTTP 401 with the OpenAI envelope, and no redirect.
- The gateway's `upstream-error` terminal frame is surfaced to the caller.
- A gateway denial for `InvokeModel` renders as an inline error, not as the 403 boundary.
- Provisioning: the resolver calls `GetServiceInfo` before `Introspect`; a failed call degrades to
  `principalPrn: null` and logs `principal.resolve_failed`; `currentPrincipal()` retries once
  after `identity-not-provisioned`.

### 10.4 Tier 3 — e2e, single zone (`gateway-console-ts:test-e2e`, part 1)

A production build behind the standalone `server.js`, reached through the TLS terminator.
`PAIGASUS_SESSION_STORE=memory`, one zone in `PAIGASUS_ZONES`.

| Scenario | AC |
|---|---|
| `/gateway/` loads with no cookie, with its CSS and JS | shell |
| Unauthenticated `/gateway/` to the IdP and back, landing on the playground; the fake IAM saw `GetServiceInfo` before `Introspect` | 2 (partial) |
| A streamed completion renders incrementally: at least two read events separated by the fake's delay, and the rendered text grows between them | 3 |
| A **negative control** that buffers the same stream fails that timing assertion | 3 |
| The stop control cancels the stream, and the fake gateway observes the disconnect | 3 |
| No response body — HTML, RSC payload or route-handler output — contains the access or refresh token | ADR-0017 |
| Sign out, then `/gateway/` redirects to login again | shell |

### 10.5 Tier 4 — e2e, two zones (`gateway-console-ts:test-e2e`, part 2)

A Redis container, both standalone servers, one shared cookie secret,
`PAIGASUS_ZONES={"iam":"/iam","gateway":"/gateway"}`, `PAIGASUS_SESSION_STORE=redis`, and one
terminator path-routing both.

| Scenario | AC |
|---|---|
| Log in at `/iam`, navigate to `/gateway`, the playground renders, and the fake IdP counted exactly one `/authorize` call | 1 |
| The cross-zone navigation is a hard navigation, and the same-zone one is not | ADR-0017 |
| Stop the `iam-console` process, clear cookies, log in cold at `/gateway`, land on the playground | 2 |
| With `iam-console` stopped, the IAM navigation entry renders degraded, not absent | ADR-0020 |
| Static chunks do not collide: each zone loads its own `_next` assets under its own base path | SMA-513 AC 3 rehearsal |

**Why the `/authorize` count is the assertion for AC 1.** A test that only checks the page renders
would pass if the browser silently completed a second, invisible IdP round trip. Counting the
authorize calls is what distinguishes "the session was shared" from "the user logged in twice
quickly".

### 10.6 AC 4

`build` asserts the standalone entry point. `tests/standalone-runtime.test.ts` boots the server
with a complete, valid environment and targets `GET /gateway/healthz`, and boots it again with a
zone and base-path mismatch, asserting the mismatch message in the server output rather than only
a 500.

---

## 11. The deployment contract (for SMA-513)

| Group | Variables |
|---|---|
| Zone | `PAIGASUS_ZONE=gateway`, `PAIGASUS_ZONES` (JSON, identical in every zone) |
| Auth | the `authEnvShape` keys, with `PAIGASUS_SESSION_STORE=redis` **mandatory**, and the same cookie and crypto secret as every other zone |
| Discovery | `PAIGASUS_SERVICES` (JSON; must contain `iam` **and** `gateway`), optional `PAIGASUS_DISCOVERY_*_MS` |
| IAM | `PAIGASUS_IAM_GRPC_URL` |
| Gateway | `PAIGASUS_GATEWAY_URL`, `PAIGASUS_GATEWAY_DEFAULT_MODEL` |

`redis` is mandatory and not a recommendation: `createAuthRuntime` throws when
`PAIGASUS_SESSION_STORE=memory` and `PAIGASUS_ZONES` names more than one zone
(`ts/packages/paigasus-auth/src/runtime.ts:105-113`). A two-zone deployment on the memory store
cannot start, which is the intended behaviour — under it, a user signed in on one zone is
anonymous on the other.

Further assumptions:

- The ingress forwards `Host` or `X-Forwarded-Host` unchanged, so Next's Server Action origin
  check passes.
- The OIDC client registers `https://<origin>/gateway/auth/callback` as a redirect URI and
  `https://<origin>/gateway/` as a post-logout URI, in addition to the IAM zone's pair. ADR-0017
  decision 5 is one client registration with one redirect URI per zone.
- The console reaches the gateway's HTTP port and IAM's gRPC port from inside the cluster.
- IAM's `authn.issuers[].audiences` accepts the audience of this console client's access tokens.

---

## 12. Follow-ups

Linear issues to create after this spec is approved:

1. **Organization and project settings screens in the gateway zone** — the screens D8's route
   shape exists for.
2. **A `/v1/models` route in `paigasus-gateway`**, with its own capability key, so the playground
   can offer a model list instead of a text field (D6's real fix).
3. **Consolidating `lib/nav.ts`** across the two zones, if a third zone shows the two copies have
   converged.

---

## 13. Recorded limits and departures

- AC 4 is the standalone build and the `NEXT_PUBLIC_` gate, not an OCI image (§ 2.2). SMA-513 owns
  the image.
- AC 2 concerns the `iam-console` **app**, not the IAM **service**, which must be reachable
  (§ 2.2, § 6.2).
- `gateway-console-ts:test-e2e` **requires Docker**. `iam-console`'s tier does not. There is no
  alternative: the memory store is refused with two zones.
- The organization switcher runs `myScopes()` on every gateway page render — one `Introspect`, one
  `ListRoleGrants` and up to 50 tenancy reads. SMA-511 § 12 already records that cost as unmeasured
  for the IAM zone. This design pays it twice, on Sven's explicit decision (D7), because the
  gateway gets organization and project settings later.
- The switcher's scope changes the URL and the breadcrumbs and nothing else (D8). It is not a dead
  control, but it is not yet a useful one.
- `@paigasus/console-core` exports a testing surface from a package that is not a test package
  (§ 5.6).
- The PRN reader stays an ADR-0005 exception. This design moves it to one site instead of letting
  it become two. SMA-634 still owns the underlying napi packaging defect.
- The playground keeps no history. A page reload loses the transcript (D3).

---

## 14. Things the plan must measure before building on them

1. **Whether one TLS terminator can path-route to two Next standalone servers** with `Host` and
   `X-Forwarded-Proto` intact, and whether the two apps' static chunks collide under one origin.
   This rehearses SMA-513 AC 3.
2. **Whether Next's standalone server preserves incremental delivery of a `ReadableStream`
   returned from a route handler.** This is the real AC 3 risk. Nothing in this repository has
   exercised it, and a buffering layer anywhere between the handler and the socket defeats the
   whole design.
3. Whether `proxy.ts` running on the chat route disturbs a streaming POST.
4. Whether `testcontainers` starts Redis inside a Moon task, locally and in CI.
5. Whether pnpm resolves `@paigasus/console-core/testing`'s devDependencies for a consuming app.
6. Whether the fake gateway's SSE delays survive the terminator, so the AC 3 timing assertion
   measures the server and not the test harness.
7. What `createChatClient` does when the caller aborts, and whether the abort reaches the fake
   gateway — the stop control depends on it.
