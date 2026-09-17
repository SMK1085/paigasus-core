# SMA-641 — a `dev:stack` command so `next dev` runs for the console zones

Linear: <https://linear.app/smaschek/issue/SMA-641>

Status: draft for review.
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

The developer opens one URL and logs in. An edit to app code or to a workspace package updates the
browser.

## 3. Decisions

| # | Decision | Reason |
| -- | -- | -- |
| D1 | Serve BOTH zones, always | The zones link to each other. A single-zone stack cannot show that. Chosen by Sven. |
| D2 | Redis comes from a `testcontainers` container | Same as the two-zone e2e tier. Zero setup for the developer. Docker becomes a hard requirement, and the stack fails loudly without it. Chosen by Sven. |
| D3 | `tsx` runs the stack | The fakes are TypeScript with extensionless relative imports. Node 24 strips types but resolves specifiers literally, so plain `node` cannot load them. The alternative was a third copy of the hand-rolled resolve hook. Chosen by Sven. |
| D4 | The terminator proxies WebSocket upgrades | Without it `next dev` has no hot reload. Chosen by Sven. |
| D5 | The stack lives in `ts/tooling/`, not in a package or an app | `ts/tooling/**/*` is already in `ts:lint`'s `sources` group. No new package, no new Moon project, no new gate registration. Chosen by Sven. |
| D6 | A supervisor, NOT a written `.env.local` | See § 4. The issue proposed an env file. It cannot work. |
| D7 | Fixed ports for the two browser-facing servers | A certificate exception is stored per origin. A random port forces a new exception on every restart. |
| D8 | The stack ships its own fake-IAM world | See § 8. |
| D9 | No automated test for the supervisor | See § 10. |

## 4. Why a supervisor and not an env file

The issue's scope line says the command "prints the environment block (or writes `.env.local`)".
An env file cannot work, for one reason that admits no workaround:

**The fakes use a self-signed certificate, so the Next process needs `NODE_EXTRA_CA_CERTS`. Node
reads that variable when the process starts, before Next loads `.env.local`.** A value placed in
`.env.local` has no effect. The OIDC discovery request then fails with a certificate error.

Two further reasons support the same conclusion. The fake IAM and fake gateway take random ports,
so `PAIGASUS_SERVICES` and `PAIGASUS_IAM_GRPC_URL` are not known until the stack runs. And both
zone processes must agree on one `PAIGASUS_PUBLIC_ORIGIN`, which is the terminator's origin.

So `dev:stack` starts the `next dev` children itself and injects the environment. It still PRINTS
the environment block, because that answers "what does this app need?" — but printing is a
diagnostic, not the mechanism.

## 5. Shape

```
browser --https--> TLS terminator :8443 -- /iam/*     --http--> next dev :3000 (iam-console)
                                         \- /gateway/* --http--> next dev :3001 (gateway-console)

browser --https--> fake IdP :8444            (the authorization redirect goes here directly)

both children --https--> fake IdP :8444      (discovery, token, JWKS)
both children --h2c----> fake IAM gRPC       (random port)
both children --http---> fake IAM + fake gateway service-info  (random ports)
both children ---------> Redis container     (random mapped port, one session store)
```

Start order, each step recorded for teardown in reverse:

1. `testTls({ root: <dev root>, days: 30 })` — certificate and key.
2. Redis: `GenericContainer('redis:8-alpine')`, one exposed port.
3. `startFakeIdp({ cert, port: 8444 })`.
4. `startFakeIam({ handlers: devWorld() })`, then `setServiceInfo(...)`.
5. `startFakeGateway()`, then `setServiceInfo(...)`.
6. `startTlsTerminator({ tls, port: 8443, routes: [...] })`.
7. Two `next dev` children, each with `buildDevEnv()`'s record plus `PORT`, `HOSTNAME` and
   `PAIGASUS_ZONE`.
8. Wait for `http://127.0.0.1:3000/iam/healthz` and `http://127.0.0.1:3001/gateway/healthz`.
9. Print the URL, the two certificate exceptions to accept, and the environment block.

`SIGINT` and `SIGTERM` close everything in reverse order and exit. A failure during start closes
what already started, then reports the START error — a close error goes to stderr and must not
hide it. This mirrors `ts/apps/gateway-console/tests/e2e/support/two-zone-harness.ts`.

## 6. Ports

| Port | Server | Faces the browser |
| -- | -- | -- |
| 8443 | TLS terminator | yes — the only URL to open |
| 8444 | fake IdP | yes — the authorization redirect |
| 3000 | `next dev`, iam-console | no |
| 3001 | `next dev`, gateway-console | no |
| random | fake IAM gRPC, fake IAM HTTP, fake gateway, Redis | no |

The two browser-facing ports are fixed because a browser stores a certificate exception per
origin. A random port would force a new exception on every restart. The two `next dev` ports are
fixed only so the terminator's routes can be built before the children start; the developer never
opens them.

A port already in use is a hard failure. The message names the port and the server that wanted it.

Both origins use the host `127.0.0.1`, which matches the existing code and the certificate's SAN
(`DNS:localhost,IP:127.0.0.1`).

## 7. Changes to `@paigasus/console-core/testing`

Four changes. Every one keeps its current behaviour as the default, so the three existing e2e
harnesses are unaffected.

### 7.1 `tls-terminator.ts` — WebSocket upgrades

`next dev` opens a WebSocket for hot module replacement. The terminator drops it today, twice
over: `upgrade` is in `HOP_BY_HOP`, so the header never reaches the upstream, and the server has
no `'upgrade'` listener, so Node closes the connection.

Add an `'upgrade'` listener that selects the route with the same longest-prefix rule the request
path uses, opens a plain HTTP request to the upstream carrying the original `upgrade` and
`connection` headers, and pipes the two sockets together in both directions when the upstream
answers `101`. An unroutable or failing upgrade destroys the client socket.

The e2e tier sends no WebSocket, so its behaviour does not change.

### 7.2 `tls-terminator.ts` — an optional port

`startTlsTerminator` binds `listen(0, ...)`. Add an optional `port`, default `0`, so the current
callers keep a random port.

### 7.3 `fake-idp.ts` — an optional port

`startFakeIdp` binds `listen(0, ...)` and builds `issuer` from the assigned port. Add an optional
`port`, default `0`.

### 7.4 `tls.ts` — an optional validity, and a new pure module

`testTls()` issues one-day certificates. A dev certificate that expires daily forces the developer
to accept two exceptions again every day. Add an optional validity in days, default 1.

The stack passes its own cache `root`, so the 30-day dev certificate and the 1-day e2e certificate
never share cached material.

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

It lives in the package, not in `ts/tooling/`, because `ts/tooling/` has no vitest project and this
is the part most likely to break in silence. `testing/index.ts` re-exports it.

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

Each child adds `PORT`, `HOSTNAME=127.0.0.1` and its own `PAIGASUS_ZONE`.

The stack does NOT set `PAIGASUS_DISCOVERY_NEGATIVE_MS`, `_FRESH_MS` or `_STALE_MS`. The e2e
harnesses set them to 1, 2 and 3 milliseconds to force a re-probe on every call. Dev uses the real
defaults, so the discovery cache behaves the way it does in a deployment.

### 8.2 The world

`startFakeIam()` with no handlers cannot serve the console. Its `defaults()` answers only
`serviceInfo.getServiceInfo`, `authz.isAuthorized` (allow) and `authz.listRoleGrants` (empty); every
tenancy method throws `Unimplemented`. `Introspect` throws `identity-not-provisioned` until an
enforced call provisions the token.

The e2e worlds that solve this live in `ts/apps/iam-console/tests/e2e/support/world.ts` (195 lines)
and `ts/apps/gateway-console/tests/e2e/support/world.ts` (80 lines). Neither is importable from
`ts/tooling/`: they are app test files, they import `@paigasus/console-core`'s `IamAction` type and
`@paigasus/sdk/iam/types`, and the apps' boundary rules scope them to tests.

So `ts/tooling/dev-stack-world.ts` holds a smaller world of its own: one organization, one team,
one project, every action allowed, an empty audit log.

This is a third near-copy of the same idea, accepted deliberately. A shared world would couple two
things with different lifecycles: an e2e assertion change would alter the dev experience, and a dev
convenience would alter what the tests prove. Promoting all three into the package is a separate
piece of work, and it would edit load-bearing e2e fixtures inside a dev-tooling change.

## 9. Documentation

- `ts/apps/iam-console/.env.local.example` and `ts/apps/gateway-console/.env.local.example`: point
  at `dev:stack`, and state that a copied env file alone cannot boot the app, because
  `NODE_EXTRA_CA_CERTS` must exist before Node starts.
- Both apps' README: a "Run it locally" section with the one command, the URL, and the two
  certificate exceptions.
- `ts/README.md`: one line naming the command.

**Read restriction.** The agent's tools refuse to read a `.env*` path, so I cannot open either
`.env.local.example` directly. Sven decides whether to paste the content or to accept a blind
edit. A `git show` of the same path is a way around the guard and must not be used.

## 10. Testing

In `ts/packages/paigasus-console-core/tests/unit/`:

| Test | Asserts |
| -- | -- |
| terminator upgrade | A WebSocket handshake through the terminator reaches the routed upstream, and bytes flow both ways. Performed with a raw `node:http` upgrade, so no WebSocket library is needed. |
| terminator upgrade, no route | An upgrade to an unrouted path destroys the client socket and does not crash the server. |
| terminator port | An explicit port binds that port; no port keeps a random one. |
| idp port | The same two cases, and `issuer` carries the explicit port. |
| tls validity | An explicit day count produces a certificate valid past one day; the default stays one day. |
| `buildDevEnv` | Every required key is present; a parent `PAIGASUS_*`, `__NEXT*` or `NODE_ENV` value is removed; the three discovery timing keys are ABSENT. |

**The supervisor has no automated test.** A test would need Docker, two `next dev` servers and a
browser. Verification is by hand, and the PR records the result: the stack starts, the login
completes, both zones render, and an edit to a source file updates the browser without a reload.

Stated plainly: nothing in CI runs this tool, so it can break without CI noticing. The four
package changes in § 7 ARE covered, so the most likely silent breakage — the env contract and the
upgrade tunnel — is gated. The residual is the supervisor's own orchestration.

## 11. Deliverables

1. `ts/tooling/dev-stack.ts` — the supervisor.
2. `ts/tooling/dev-stack-world.ts` — the fake-IAM world.
3. `ts/packages/paigasus-console-core/testing/tls-terminator.ts` — upgrade handler, optional port.
4. `ts/packages/paigasus-console-core/testing/fake-idp.ts` — optional port.
5. `ts/packages/paigasus-console-core/testing/tls.ts` — optional validity.
6. `ts/packages/paigasus-console-core/testing/dev-env.ts` — `buildDevEnv()`, re-exported from
   `testing/index.ts`.
7. `ts/packages/paigasus-console-core/tests/unit/` — the tests in § 10.
8. `ts/package.json` — a `dev:stack` script, and `tsx`, `testcontainers` and
   `@paigasus/console-core` as devDependencies. The root package already carries `workspace:*`
   entries, so this shape has precedent.
9. `ts/pnpm-workspace.yaml` — a catalog entry for `tsx`. `testcontainers` is already in the
   catalog.
10. Both apps' `.env.local.example` and README, and `ts/README.md`.

## 12. Out of scope

- A single-zone form. D1 chose both zones.
- A `.env.local` writer. § 4 explains why it cannot work.
- Promoting the three fake-IAM worlds into one shared world (§ 8.2).
- Promoting the duplicated `ts-esm-loader.mjs` into one shared copy. D3 removes the need to add a
  third copy; it does not remove the existing two.
- Running `dev:stack` in CI.
- A Moon task. The command is a developer tool, and a Moon task would have to join the CI target
  array or carry an exemption.

## 13. Risks

| Risk | Handling |
| -- | -- |
| Docker is required for `next dev` | Chosen in D2. The stack fails loudly and names Docker. |
| The supervisor is untested and can rot | Stated in § 10. The package-side contract IS tested. |
| The upgrade handler edits a file three e2e harnesses use | It is additive, and the e2e tier sends no WebSocket. Covered by two new tests. |
| A third fake-IAM world | Accepted in § 8.2, with the reason recorded. |
| `next dev` behind a path-routing proxy may still reveal HMR problems | Verified by hand before the PR. If HMR needs more than the upgrade tunnel, the finding lands in the plan, not in a silent workaround. |
| The scope is larger than the issue's "roughly an hour" | The missing WebSocket support and the missing IAM world are absent from the issue text. Recorded here rather than absorbed. |
