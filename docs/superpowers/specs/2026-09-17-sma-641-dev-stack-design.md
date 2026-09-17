# SMA-641 — a `dev:stack` command so `next dev` runs for the console zones

Linear: <https://linear.app/smaschek/issue/SMA-641>

Status: revised after the adversarial challenge.
Related: SMA-512 (gateway-console, which raised this), SMA-511 (iam-console).

## 1. The problem

Neither `ts/apps/iam-console` nor `ts/apps/gateway-console` can run `next dev` today. Both
`package.json` files declare `"dev": "next dev"`, and both fail at the first request, because
`getRuntimeConfig()` rejects the environment.

Eight keys have no default:

| Source | Keys with no default |
| -- | -- |
| `@paigasus/next-config` `coreEnvShape` | `PAIGASUS_ZONE`, `PAIGASUS_ZONES` |
| `@paigasus/auth` `authEnvShape` | `PAIGASUS_OIDC_ISSUER`, `PAIGASUS_OIDC_CLIENT_ID`, `PAIGASUS_OIDC_CLIENT_SECRET`, `PAIGASUS_PUBLIC_ORIGIN`, `PAIGASUS_SESSION_STORE` |
| `@paigasus/discovery` `discoveryEnvShape` | `PAIGASUS_SERVICES` |
| each app's `lib/config.ts` | `PAIGASUS_IAM_GRPC_URL` |

A list of keys does not solve it. Three facts make the problem infrastructural.

1. `PAIGASUS_OIDC_ISSUER` and `PAIGASUS_PUBLIC_ORIGIN` use the `httpsUrl` validator
   (`ts/packages/paigasus-auth/src/config.ts:22-27`). Each must be an absolute `https:` URL with
   no trailing slash. A plain `next dev` on `http://localhost:3000` cannot satisfy
   `PAIGASUS_PUBLIC_ORIGIN`. The `__Host-` cookie prefix needs a secure origin too.
2. The app calls real endpoints: an OIDC provider that issues for the client, an IAM gRPC
   endpoint, and a `GET /v1/service-info` endpoint per discovered service.
3. `@paigasus/auth`'s `createAuthRuntime` refuses the memory session store when `PAIGASUS_ZONES`
   names more than one zone (`ts/packages/paigasus-auth/src/runtime.ts:105-113`). A two-zone dev
   stack therefore needs Redis.

The parts already exist. `@paigasus/console-core/testing` holds a fake IdP over HTTPS, a fake IAM
with a gRPC face and an HTTP service-info face, a fake gateway, and a TLS terminator. Only
Playwright can reach them today.

## 2. Goal

One command starts a working two-zone console on the developer's machine, with hot reload:

```
pnpm --dir ts dev:stack
```

The stack prints `https://127.0.0.1:8443/iam`. The developer opens it, accepts two certificate
exceptions once, and logs in. An edit to app code or to a workspace package updates the browser.

## 3. Decisions

| # | Decision | Reason |
| -- | -- | -- |
| D1 | Serve BOTH zones, always | The zones link to each other. A single-zone stack cannot show that. Chosen by Sven. |
| D2 | Redis comes from a `testcontainers` container | Same as the two-zone e2e tier. Zero setup for the developer. Docker becomes a hard requirement, and the stack fails loudly without it. Chosen by Sven. |
| D3 | `tsx` runs the stack | The fakes are TypeScript with extensionless relative imports. Node 24 strips types but resolves specifiers literally, so plain `node` cannot load them. The alternative was a third copy of the hand-rolled resolve hook. Chosen by Sven. |
| D4 | The terminator proxies WebSocket upgrades | Without it `next dev` has no hot reload. Chosen by Sven. |
| D5 | The stack lives in `ts/tooling/` | `ts/tooling/**/*` is already in `ts:lint`'s `sources` group. It needs its OWN tsconfig and an ESLint glob change — see § 12. |
| D6 | A supervisor, NOT a written `.env.local` | See § 4. |
| D7 | Fixed ports for the two browser-facing servers | A certificate exception is stored per origin. A random port forces a new exception on every restart. |
| D8 | The stack ships its own fake-IAM world | See § 8.2. |
| D9 | No automated test for the supervisor | See § 10. |
| D10 | Move `@paigasus/console-core`'s discovery singletons to `globalThis` | See § 11. This is production code, and the dev stack is the first thing that exposes the defect. |

## 4. Why a supervisor and not an env file

The issue's scope line says the command "prints the environment block (or writes `.env.local`)".
An env file cannot carry this environment, for three reasons. The first two are structural.

1. **The fake IAM, the fake gateway and Redis take random ports.** `PAIGASUS_SERVICES`,
   `PAIGASUS_IAM_GRPC_URL` and `PAIGASUS_SESSION_REDIS_URL` are not known until the stack runs.
2. **Both zone processes must agree on one `PAIGASUS_PUBLIC_ORIGIN`**, which is the terminator's
   origin. One shared value has to reach two separately started processes.
3. **`NODE_EXTRA_CA_CERTS` cannot come from `.env.local`.** Node captures that variable while it
   initializes, before any user code runs, so a value Next later writes into `process.env` has no
   effect. `ts/packages/paigasus-console-core/testing/tls.ts:23` records the same fact. A
   workaround exists outside the env file — the `dev` script could carry the assignment inline —
   so this reason supports the first two rather than standing alone.

So `dev:stack` starts the `next dev` children itself and injects the environment. It still PRINTS
the environment block, because that answers "what does this app need?" — but printing is a
diagnostic, not the mechanism.

## 5. Shape

```
browser --https--> TLS terminator :8443 -- /iam/*     --http--> next dev :3000 (iam-console)
                                         |- /gateway/* --http--> next dev :3001 (gateway-console)
                                         \- /*         --http--> default-zone handler
                                                                  ("/" -> 302 /iam, else -> :3000)

browser --https--> fake IdP :8444            (the authorization redirect goes here directly)

both children --https--> fake IdP :8444      (discovery, token, JWKS)
both children --h2c----> fake IAM gRPC       (random port)
both children --http---> fake IAM + fake gateway service-info  (random ports)
both children ---------> Redis container     (random mapped port, one session store)
```

Start order. Each step is recorded for teardown in reverse:

1. Preflight: bind and release 8443, 8444, 3000 and 3001. This is ADVISORY — it races anything
   that takes a port in between. The authoritative check is each server's own listen error (§ 6).
2. `testTls({ root: <dev root>, days: 30 })` — certificate and key.
3. Redis: `GenericContainer('redis:8-alpine')`, one exposed port.
4. `startFakeIdp({ cert, port: 8444 })`.
5. `startFakeIam({ handlers: devWorld() })`, then `setServiceInfo(IAM_DESCRIPTOR)`.
6. `startFakeGateway()`, then `setServiceInfo(GATEWAY_DESCRIPTOR)`.
7. The default-zone handler on a random port.
8. `startTlsTerminator({ tls, port: 8443, routes: [...] })`.
9. Two `next dev` children (§ 7.5), each with `buildDevEnv()`'s record plus `PORT`, `HOSTNAME` and
   `PAIGASUS_ZONE`.
10. Wait for `http://127.0.0.1:3000/iam/healthz` and `http://127.0.0.1:3001/gateway/healthz`
    (§ 7.6).
11. Print `https://127.0.0.1:8443/iam`, the two origins whose certificate must be accepted, and
    the environment block. The stack does NOT open a browser.

`SIGINT` and `SIGTERM` close everything in reverse order and exit. A failure during start closes
what already started, then reports the START error — a close error goes to stderr and must not
hide it. This mirrors `ts/apps/gateway-console/tests/e2e/support/two-zone-harness.ts:75-212`.

## 6. Ports

| Port | Server | Faces the browser |
| -- | -- | -- |
| 8443 | TLS terminator | yes — the only URL to open |
| 8444 | fake IdP | yes — the authorization redirect |
| 3000 | `next dev`, iam-console | no |
| 3001 | `next dev`, gateway-console | no |
| random | fake IAM gRPC, fake IAM HTTP, fake gateway, default-zone handler, Redis | no |

The two browser-facing ports are fixed because a browser stores a certificate exception per
origin. A random port would force a new exception on every restart. The two `next dev` ports are
fixed only so the terminator's routes can be built before the children start.

**A busy port must fail with a named message, and today it would not.** `tls-terminator.ts:117`
and `fake-idp.ts:216` both call `server.listen(...)` inside a promise that has no `reject` and no
`'error'` listener. With `listen(0)` that never mattered. A fixed port makes `EADDRINUSE`
reachable, and Node then raises an uncaught exception that kills the supervisor with a raw stack.
So the optional-port change (§ 7.2, § 7.3) MUST also add `server.once('error', reject)`.

**MEASURED, and it removes a hazard class:** `next dev` does NOT silently move to another port
when `PORT` is set. `next/dist/cli/next-dev.js:212-213` sets `allowRetry` only when the port came
from the default, and `start-server.js:262-269` increments the port only under `allowRetry`.
Otherwise it logs `Failed to start server`. Re-measure this on a Next bump.

Both origins use the host `127.0.0.1`, which matches the existing code and the certificate's SAN
(`DNS:localhost,IP:127.0.0.1`).

## 7. Changes to `@paigasus/console-core/testing`, and the child processes

Every change keeps its current behaviour as the default, so the three existing e2e harnesses are
unaffected.

### 7.1 `tls-terminator.ts` — WebSocket upgrades

`next dev` opens a WebSocket for hot module replacement. The terminator drops it today, twice
over: `upgrade` is in `HOP_BY_HOP` (`tls-terminator.ts:19`), so the header never reaches the
upstream, and the server has no `'upgrade'` listener, so Node closes the connection.

**MEASURED on Next 16.3.5, the routing works under the existing prefix rule.** The server expects
`${basePath}/_next/hmr` (`next/dist/server/lib/router-server.js:672-684`). The client builds
`getSocketUrl(assetPrefix) + '/_next/hmr'` (`client/dev/hot-reloader/app/web-socket.js:60`), and
`assetPrefix` comes from the running script's own path prefix (`client/asset-prefix.js:12-31`),
which is `/iam` under `basePath: '/iam'`. So the URL is `wss://127.0.0.1:8443/iam/_next/hmr?id=…`,
the longest-prefix rule routes it, and the two zones stay distinguishable. The path is
`/_next/hmr`, not the older `/_next/webpack-hmr`.

Add an `'upgrade'` listener that selects the route with the same longest-prefix rule, then:

1. opens a plain HTTP request to the upstream carrying the original `upgrade` and `connection`
   headers (they must bypass `forwardable()`);
2. on the upstream's own `'upgrade'` event, writes the `HTTP/1.1 101 Switching Protocols` status
   line and the upstream's headers to the client socket BY HAND — there is no `ServerResponse` on
   this path;
3. writes the upstream's `head` buffer to the client and the server-side `head` buffer to the
   upstream BEFORE piping, then pipes both directions.

Dropping either `head` buffer loses the first frame, which shows up as intermittent HMR failure
rather than a hard one. An unroutable or failing upgrade destroys the client socket.

The e2e tier sends no WebSocket, so its behaviour does not change.

### 7.2 `tls-terminator.ts` — an optional port

`startTlsTerminator` binds `listen(0, ...)`. Add an optional `port`, default `0`, and the
`'error'` listener § 6 requires.

### 7.3 `fake-idp.ts` — an optional port

`startFakeIdp` binds `listen(0, ...)` and builds `issuer` from the assigned port. Add an optional
`port`, default `0`, and the same `'error'` listener.

### 7.4 `tls.ts` — an optional validity, and a new pure module

`testTls()` issues one-day certificates. A dev certificate that expires daily forces the developer
to accept two exceptions again every day. Add an optional validity in days, default 1. Correct
`tls.ts:37`'s comment, which says only the self-test passes its own root.

The stack passes its own cache `root`, so the 30-day dev certificate and the 1-day e2e certificate
never share cached material. Every path in `tls.ts` derives from `root`, so the isolation holds —
but nothing tests it today, so § 10 adds a row.

A new `testing/dev-env.ts` exports a PURE function:

```ts
export function buildDevEnv(input: {
  parentEnv: NodeJS.ProcessEnv;
  certPath: string;
  idp: { issuer: string; clientId: string; clientSecret: string };
  iam: { grpcUrl: string; httpUrl: string };
  gatewayUrl: string;
  redisUrl: string;
  publicOrigin: string;
}): Record<string, string>;
```

It lives in the package, not in `ts/tooling/`, because this is the part most likely to break in
silence and the package has a vitest project. `testing/index.ts` re-exports it.

### 7.5 How the children start and stop

`next dev` forks a child of its own and installs its own signal handlers
(`next/dist/cli/next-dev.js:162-168`). A `pnpm run dev` wrapper can swallow `SIGTERM` and leave an
orphan holding 3000 or 3001 — after which the next run fails on a port that looks free.

So the supervisor resolves `next`'s bin path and spawns `process.execPath` on it directly, with
`cwd` set to the app directory — the shape `two-zone-harness.ts:151-155` already uses for
`server.js`. Stop follows `harness.ts:61-77`'s `stop()`: `SIGTERM`, wait 5 seconds, then
`SIGKILL`. Both children use `stdio: ['ignore', 'pipe', 'pipe']`, and every line is printed with
its zone as a prefix.

### 7.6 Readiness

In dev the first request COMPILES the route, so the e2e budget is too short. The supervisor waits
up to 180 seconds per zone. A connection refusal or a timeout means "still starting". A 5xx answer
is fatal, the same rule as `harness.ts:80-96` — that is what distinguishes a rejected
configuration from a slow compile. A fatal answer prints the child's captured output.

## 8. The environment, and the fake-IAM world

### 8.1 The environment

`buildDevEnv()` follows `two-zone-harness.ts`'s `serverEnv` exactly: it starts from the parent
environment, then REMOVES `NODE_ENV`, every key that starts with `PAIGASUS_`, and every key that
starts with `__NEXT`. A developer's own `.env.local` or exported shell variable must not reach the
child. It then sets:

| Key | Value |
| -- | -- |
| `NEXT_TELEMETRY_DISABLED` | `1` |
| `NODE_EXTRA_CA_CERTS` | the certificate path |
| `PAIGASUS_ZONES` | `{"iam":"/iam","gateway":"/gateway"}` |
| `PAIGASUS_OIDC_ISSUER` / `_CLIENT_ID` / `_CLIENT_SECRET` | from the fake IdP |
| `PAIGASUS_PUBLIC_ORIGIN` | the terminator's origin |
| `PAIGASUS_SESSION_STORE` | `redis` |
| `PAIGASUS_SESSION_REDIS_URL` | the container's URL |
| `PAIGASUS_SERVICES` | `{"iam":<iam.httpUrl>,"gateway":<gateway.url>}` |
| `PAIGASUS_IAM_GRPC_URL` | `iam.grpcUrl` |

Each child adds `PORT`, its own `PAIGASUS_ZONE`, and `HOSTNAME=127.0.0.1`.

**`HOSTNAME` is load-bearing, not cosmetic.** `router-server.js:669` calls `blockCrossSiteDEV`
with the bind hostname, and `block-cross-site-dev.js:77-85` builds the allowlist as
`['**.localhost', 'localhost', ...allowedDevOrigins, hostname]`, then compares the `Origin`
header's hostname against it (`:102-107`). The browser sends `Origin: https://127.0.0.1:8443`, so
`127.0.0.1` is allowed only because `HOSTNAME` puts it in that list. Dropping `HOSTNAME` produces
a 403 that talks about `allowedDevOrigins` and never mentions the bind host.

The stack does NOT set `PAIGASUS_DISCOVERY_NEGATIVE_MS`, `_FRESH_MS` or `_STALE_MS`. The e2e
harnesses set them to 1, 2 and 3 milliseconds to force a re-probe on every call. Dev uses the real
defaults, so the discovery cache behaves the way it does in a deployment.

### 8.2 The world

`startFakeIam()` with no handlers cannot serve the console. Its `defaults()`
(`fake-iam.ts:245-258`) answers only `serviceInfo.getServiceInfo`, `authz.isAuthorized` (allow)
and `authz.listRoleGrants` (empty). **Every `tenancy.*` and `audit.*` method throws
`Unimplemented`**, which the console surfaces as an error page. `setHandlers()` REPLACES the whole
map (`fake-iam.ts:126-129`), so a partial handler set is not possible.

The ordering is not a problem: `introspect` checks `provisioned.has(token)` before consulting any
scripted handler (`fake-iam.ts:228-231`), but the principal resolver calls
`serviceInfo.getServiceInfo` first (`principal-resolver.ts:62-65`), which provisions the token
(`fake-iam.ts:268-271`), and `introspectWithProvisioning` (`principal.ts:41-48`) carries a retry.
Login works. Note also that `fake-iam.ts:241` forces `roleGrants: []` after the spread, so a
scripted `authn.introspect` cannot return grants.

`ts/tooling/dev-stack-world.ts` therefore supplies the FULL handler map:

- `authn.introspect`
- `tenancy.listOrganizations`, `listTeams`, `listProjects`, `listMemberships`
- `tenancy.getOrganization`, `getTeam`, `getProject`
- `tenancy.createOrganization`, `createTeam`, `createProject`
- `tenancy.renameOrganization`, `renameTeam`, `renameProject`
- `tenancy.archiveOrganization`, `archiveTeam`, `archiveProject`
- `tenancy.restoreOrganization`, `restoreTeam`, `restoreProject`
- `tenancy.attachMembership`, `detachMembership`
- `audit.listAuditEntries`
- `authz.isAuthorized` (allow every action), `authz.listRoleGrants`
- `serviceInfo.getServiceInfo`

The exact key set is taken from `ts/apps/iam-console/tests/e2e/support/world.ts:126-194`; the
gateway zone additionally needs `authn.introspect`, `authz.listRoleGrants` and the three
`tenancy.get*` (`ts/apps/gateway-console/tests/e2e/support/world.ts:50-77`).

Two further requirements, each measured by the e2e worlds:

- **Every tenancy node carries `status` and `effectiveStatus` set to `NodeStatus.ACTIVE`**
  (`iam-console/.../world.ts:82-86`). Without them every row and header reads "Status unknown".
- **The descriptors are pinned.** IAM must report `['iam.authz.cedar', 'iam.audit']`: `myScopes()`
  lists role grants only when discovery reports `iam.authz.cedar`
  (`ts/packages/paigasus-console-core/src/scopes.ts:116-118`), and the audit page needs
  `iam.audit`. The gateway reports `['gateway.chat.stream']`.

The world holds one organization, one team, one project, and one audit entry.

This is a third near-copy of the same idea, accepted deliberately. A shared world would couple two
things with different lifecycles: an e2e assertion change would alter the dev experience, and a
dev convenience would alter what the tests prove. Promoting all three into the package is separate
work, and it would edit load-bearing e2e fixtures inside a dev-tooling change.

`NodeStatus` comes from `@paigasus/sdk/iam/types`, so `@paigasus/sdk` joins the root
devDependencies in § 12.

## 9. The origin root and Next's dev-overlay endpoints

Two related problems, both absent from the issue.

**The root has no route.** `routes: [{prefix:'/iam'}, {prefix:'/gateway'}]` matches only those two
prefixes (`tls-terminator.ts:33-36`), so `https://127.0.0.1:8443/` and `/favicon.ico` answer
`502 no route matches`.

**Next's dev-overlay endpoints are ROOT-relative and cannot be routed by prefix.** The overlay
fetches `/__nextjs_original-stack-frames` with no basePath
(`next/dist/next-devtools/shared/stack-frame.js:76`), and the server matches the bare pathname
(`next/dist/server/dev/middleware-turbopack.js:282`, `:312`). The same family holds
`/__nextjs_font/…`, `/__nextjs_source-map`, `/__nextjs_launch-editor`,
`/__nextjs_attach-nodejs-inspector` and `/__nextjs_error_feedback`
(`next-dev-server.js:341`). A prefix route cannot catch them: the matcher requires
`pathname === prefix` or `pathname.startsWith(prefix + '/')`, and `/__nextjs_original-stack-frames`
satisfies neither for a `/__nextjs` prefix.

One mechanism solves both, and it needs no change to the terminator's matcher. The supervisor adds
a **default-zone handler** on its own port and gives the terminator a third route, `{prefix: '/',
target: <handler>}`. The longest-prefix sort already puts it last. The handler answers:

- `/` exactly → `302` to `/iam`;
- everything else → proxy to the iam zone's dev server.

**Known limitation, stated rather than hidden:** two zones on one origin cannot both own
`/__nextjs_*`. The iam zone is the default, so the gateway zone's error overlay loses its
source-mapped frames, its fonts and "open in editor". The overlay still reports the error.

## 10. Testing

In `ts/packages/paigasus-console-core/tests/unit/`:

| Test | Asserts |
| -- | -- |
| terminator upgrade | A handshake through the terminator reaches the routed upstream; the client receives a `101` status line and the echoed `sec-websocket-accept`; bytes flow both ways. Performed with a raw `node:http` upgrade, so no WebSocket library is needed. |
| terminator upgrade, head buffers | A client that sends its first frame in the same packet as the handshake still delivers it, and an upstream that answers with a trailing buffer delivers that too. |
| terminator upgrade, no route | An upgrade to an unrouted path destroys the client socket and does not crash the server. |
| terminator port | An explicit port binds that port; no port keeps a random one. |
| terminator port in use | An explicit port already bound REJECTS with a message naming the port, rather than raising an uncaught exception. |
| idp port | The same three cases, and `issuer` carries the explicit port. |
| tls validity | An explicit day count produces a certificate valid past one day; the default stays one day. |
| tls root isolation | Two roots with different day counts produce independent pairs; neither sees the other's pointer file. |
| `buildDevEnv` | Every required key is present; a parent `PAIGASUS_*`, `__NEXT*` or `NODE_ENV` value is removed; the three discovery timing keys are ABSENT. |

**The supervisor has no automated test.** A test would need Docker, two `next dev` servers and a
browser. Verification is by hand, and the PR records the result: the stack starts, the login
completes, both zones render, an edit to a source file updates the browser without a reload, and
`Ctrl-C` leaves no process holding 3000, 3001, 8443 or 8444.

Stated plainly: nothing in CI runs this tool, so it can break without CI noticing. The package
changes in § 7 ARE covered, so the most likely silent breakage — the env contract and the upgrade
tunnel — is gated. The residual is the supervisor's own orchestration.

## 11. A defect the dev stack exposes (D10)

`ts/packages/paigasus-console-core/src/discovery.ts:22-23` holds `processCache` and `redisClient`
in MODULE scope, and `descriptorCacheFor` (`:98-109`) creates a node-redis client on first use.
Under `next dev` each recompiled module graph gets a fresh copy, so a long dev session accumulates
open Redis connections. `resetDiscoveryForTest` (`:112-116`) has no production caller.

SMA-511 fixed exactly this class for `getAuthRuntime` by moving the memo onto `globalThis` under a
`Symbol.for` key (`ts/packages/paigasus-auth/src/runtime.ts:171-230`). This spec applies the same
fix to `discovery.ts`.

Two honest notes. No existing tier can catch this — a production build does not recompile — so the
dev stack is the first thing that exposes it. And this is production code changed inside a
dev-tooling change; the alternative is to record the leak as a known limitation and open a
follow-up issue instead. The fix is preferred because the precedent is exact and the symptom lands
on the feature being built.

## 12. Deliverables

1. `ts/tooling/dev-stack.ts` — the supervisor, including the default-zone handler.
2. `ts/tooling/dev-stack-world.ts` — the fake-IAM world.
3. `ts/tooling/tsconfig.json` — extends `../tsconfig.base.json`, includes `**/*.ts`, types
   `["node"]`. **Required, not optional:** `ts/eslint.config.js:29-38` applies `projectService`
   with `tsconfigRootDir: ts/` to every `**/*.{ts,tsx,mts,cts}`, `ts/` has no root `tsconfig.json`
   by design (`ts/moon.yml:49-58` enforces that), and a file in no program is an ESLint ERROR —
   `ts/packages/paigasus-console-core/tsconfig.json:4` records it. The existing `ts/tooling/*.mjs`
   files escape only because `.mjs` is outside that glob.
4. `ts/eslint.config.js` — extend the `tooling/**/*.{js,mjs,cjs}` block (`:23`) to cover `.ts`.
5. `ts/packages/paigasus-console-core/testing/tls-terminator.ts` — upgrade handler, optional port,
   listen-error listener.
6. `ts/packages/paigasus-console-core/testing/fake-idp.ts` — optional port, listen-error listener.
7. `ts/packages/paigasus-console-core/testing/tls.ts` — optional validity, corrected comment.
8. `ts/packages/paigasus-console-core/testing/dev-env.ts` — `buildDevEnv()`, re-exported from
   `testing/index.ts`.
9. `ts/packages/paigasus-console-core/src/discovery.ts` — the `globalThis` memo (§ 11).
10. `ts/packages/paigasus-console-core/tests/unit/` — the tests in § 10.
11. `ts/package.json` — a `dev:stack` script, and `tsx`, `testcontainers`,
    `@paigasus/console-core` and `@paigasus/sdk` as devDependencies. The root package already
    carries `workspace:*` entries, so this shape has precedent.
12. `ts/pnpm-workspace.yaml` — a catalog entry for `tsx`. `testcontainers` is already there.
13. Both apps' `.env.local.example` and README, and `ts/README.md`.

## 13. Out of scope

- A single-zone form. D1 chose both zones.
- A `.env.local` writer. § 4 explains why.
- Promoting the three fake-IAM worlds into one shared world (§ 8.2).
- Promoting the duplicated `ts-esm-loader.mjs` into one shared copy. D3 removes the need for a
  third copy; it does not remove the existing two.
- Running `dev:stack` in CI.
- A Moon task. A Moon task would have to join the CI target array or carry an exemption.

## 14. Risks

| Risk | Handling |
| -- | -- |
| Docker is required for `next dev` | Chosen in D2. The stack fails loudly and names Docker. |
| The supervisor is untested and can rot | Stated in § 10. The package-side contract IS tested. |
| The upgrade handler edits a file three e2e harnesses use | Additive, and the e2e tier sends no WebSocket. Covered by three new tests. |
| A third fake-IAM world | Accepted in § 8.2, with the reason recorded. |
| The gateway zone's dev overlay is degraded | Known limitation, § 9. |
| A restart logs the developer out | A fresh Redis holds the session store and the descriptor cache. One line in the README. |
| `next dev` writes into the same `.next` the Tailwind guard reads | Local only — CI never runs `dev:stack`. `ci/tailwind-source/README.md` already records a stale-chunk residual for a Moon cache hit; a dev run widens it. Recorded, not fixed. |
| `next dev` may rewrite the tracked `next-env.d.ts` | `repo:next-env-drift` gates that file. Verify by hand during implementation; if a dev run dirties it, record the fact and the remedy before the PR. |
| New root devDependencies change `ts/pnpm-lock.yaml` | `tsx` brings an esbuild tree, and `@paigasus/console-core`'s peer dependencies (`next`, `react`) resolve at the root. Run the full graph, including `repo:osv` and the licence gates, before pushing. pnpm's 24-hour minimum release age applies to a same-day `tsx` release. |
| The scope is larger than the issue's "roughly an hour" | The missing WebSocket support, the missing IAM world, the unrouted origin root and the overlay endpoints are all absent from the issue text. Recorded rather than absorbed. |

## 15. Findings from the adversarial challenge that were REJECTED

| Finding | Why it was rejected |
| -- | -- |
| "`.env.local.example` does not exist in either app; the deliverable should be *create*, and the read-restriction note is moot" | REFUTED by measurement. `git ls-files ts/apps` lists `ts/apps/iam-console/.env.local.example` and `ts/apps/gateway-console/.env.local.example`. The challenger's glob missed them because they are dotfiles. The deliverable stays "edit", and the note below stands. |

**Read restriction, unchanged.** The agent's tools refuse to read a `.env*` path, so the two
`.env.local.example` files cannot be opened directly. A `git show` of the same path is a way around
that guard and must not be used. Sven decides whether to paste the content or to accept a blind
edit.
