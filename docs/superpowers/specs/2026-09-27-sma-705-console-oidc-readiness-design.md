# SMA-705: a console pod that cannot do OIDC discovery is not ready

- Linear: SMA-705
- Date: 2026-09-27
- Packages: `ts/packages/paigasus-auth`, `ts/apps/iam-console`, `ts/apps/gateway-console`,
  `charts/paigasus`
- Related: SMA-656 (the discovery 503, § 6 known limits), SMA-506 (auth design, § 12), SMA-513
  (chart design, § 7.8), SMA-688 (console versions and images), SMA-704 (discovery under the
  refresh lock), `docs/ops/RUNBOOK-containers.md` § 4 (the `/healthz` and `/readyz` contract)
- Revision 2, approved at GATE 1 (2026-09-27). One adversarial challenge is folded in (§ 11).
- Revision 3. Rebased on SMA-704 by a merge (2026-09-27). § 11 lists the changes.

## 1. The problem

The OIDC adapter (`src/adapters/oidc.ts`) runs discovery lazily, at the first call that needs it.
It keeps a successful result for the life of the process. A failure clears `configPromise`, so the
next call tries again. After SMA-656, a discovery failure gives a 503 on `/auth/login` and on
`/auth/callback`.

The console readiness probe is `GET <basePath>/healthz`. That route parses the configuration only.
It does not touch the OIDC adapter. So a pod that cannot discover (DNS, egress, TLS, a wrong
issuer) is ready, and the Service sends it traffic.

Behind a round-robin balancer, such a pod fails the callbacks of logins that other pods started.
Each retry costs the user a full sign-in at the IdP. Nothing takes the pod out of rotation.

## 2. Acceptance

- A1. A pod that has not built its auth runtime and completed one OIDC discovery does not receive
  login or callback traffic.
- A2. The Helm chart's readiness probe uses the new route, and the image tag that the chart pins
  serves that route (§ 9).
- A3. No error object, URL, secret or library `reason` gets into a probe response. The log gets
  only the closed SMA-656 `reason`, or the `name` of a runtime-build error (D10).
- A4. Tests prove the states of the route and the adapter, through the built app. A mutation table
  proves that the tests bite.

## 3. Decisions

- **D1. The gate checks the runtime build and discovery. It does not ping Redis.** Redis is shared
  by every console pod, so a Redis fault would take every pod out of rotation. SMA-653 already
  answers a store fault with a 503 page. This is a decision, not a gap: it supersedes the SMA-513
  § 7.8 text "a real /readyz is the fix" for Redis. (Decided at brainstorming.) The runtime build
  is in the gate by construction (D10): the route needs the runtime to reach the OIDC client.
- **D2. Ready is sticky.** After one successful discovery the pod stays ready for the life of the
  process. The adapter never discovers again (§ 1). The reasons:
  - An IdP outage after the first success affects every pod. Removing all pods would also remove
    the pages of signed-in users, and it would not make a sign-in work.
  - A pod-local fault after the first success (DNS or egress on one node) is a real gap. A
    re-discovery would detect it, but the adapter keeps the first result, so a re-discovery that
    does not replace the result proves nothing that the other routes use. § 6 records the gap.
- **D3. The route never waits for discovery.** It reads a synchronous status. If discovery is idle,
  it starts one without `await`, and answers at once. The next probe sees the result. The chart
  does not need to know `PAIGASUS_OIDC_HTTP_TIMEOUT_MS`. (Decided at brainstorming.) One wait
  stays: the FIRST call of a process builds the runtime (D10). That includes the Redis connect,
  bounded by `PAIGASUS_SESSION_REDIS_TIMEOUT_MS` (default 1000 ms), and the first load of the route
  chunk. So the first probe can pass the kubelet's default 1 s timeout. That is harmless: the
  kubelet counts one failure, and the build continues. The next probe finds the runtime.
- **D4. No eager discovery at process start.** The first readiness probe starts discovery a few
  seconds after the server binds. No user request reaches a pod that is not ready, so no user
  request pays for discovery. An `instrumentation.ts` hook would save at most one probe period
  (10 s). It would add a start hook to both apps, and Next also runs `register()` in the edge
  runtime. (Decided at brainstorming.)
- **D5. The logic lives once, in `@paigasus/auth`.** `@paigasus/auth/server` exports
  `readinessResponse`. Each app adds a route file of three lines. (Decided at brainstorming.)
- **D6. The route is `<basePath>/readyz`.** This follows the Rust services
  (`RUNBOOK-containers.md` § 4): `/healthz` does not touch a dependency, and `/readyz` does.
  `/healthz` does not change. The Docker `HEALTHCHECK` in `ts/Dockerfile` (asserted by
  `ci/images/run.sh`), the standalone-runtime tests and the Playwright harnesses keep using it.
- **D7. The probe body has two values and no reason.** 200 `{"status":"ready"}` or 503
  `{"status":"unready"}`, always with `cache-control: no-store`. No other status or body occurs
  (D10). The route is public, so it does not tell an anonymous caller why it is not ready. The
  operator reads the log (D8, D10).
- **D8. A discovery failure that the route started logs `oidc.discovery_failed` with
  `stage: 'readiness'`.** `OidcDiscoveryStage` gets the member `'readiness'`. SMA-656 D7 made this
  union for such a stage. The fields stay `{ zone, stage, reason }`, and `reason` passes through
  `oidcDiscoveryReason`. One helper, `logDiscoveryFailed(runtime, stage: OidcDiscoveryStage, err)`,
  emits the event for all three stages, so the typed parameter checks the literal (`AuthEventFields`
  is a free record). When a login or a callback joins an attempt that readiness started, one
  failure logs one event for each caller that needed discovery. That is intended: each event says
  "this stage needed a configuration and did not get one".
- **D9. The route is public in `proxy.ts`.** Both apps add `'/readyz'` to `publicPaths`. The
  matcher runs on `/readyz`, and `publicPaths` is an exact match on the path without the basePath.
  Without the entry, the middleware redirects a request with no session cookie to
  `<basePath>/auth/login?returnTo=…` on the request's own host. The kubelet follows a same-host
  redirect, so each probe would run the login route: a Redis login transaction and a
  `login.started` event each 10 s per pod. That is not measured; it is read from the kubelet's
  prober behavior. Either way the probe then does not test `/readyz`. A test in each app pins the
  entry (T13).
- **D10. A runtime-build failure gives the 503 `unready` body, and logs only the error's `name`.**
  The route builds the runtime (`authRuntime()` → `getAuthRuntime` → `createAuthRuntime`). That can
  fail: a configuration parse error, a cross-field rule, or the Redis client build.
  `getAuthRuntime` clears its slot on a failure, so each probe tries again. A Next 500 would print
  the raw error each 10 s from boot. MEASURED: a `TypeError` from `new URL()` prints its `input`
  property, and node-redis parses `PAIGASUS_SESSION_REDIS_URL` with `new URL()`
  (`@redis/client` 6.2.1, `dist/lib/client/index.js:176`). So a malformed Redis URL would put the
  password in the log each 10 s. `readinessResponse` therefore catches every error from the runtime
  getter, synchronous or not, and answers 503 `unready`. It logs one event,
  `readiness.runtime_failed { error }`, where `error` is the error's `name` only (the SMA-656 D3
  rule). It never logs the error object, its message or its `input`. The same leak on the user
  request path (the auth routes build the runtime too) is a separate defect; § 10 files it.
- **D11. The event emitters never throw out of a detached promise.** The `.catch` that logs a
  readiness discovery failure wraps the log call in `try`/`catch`. A throw from `logger.event`
  would otherwise make a new unhandled rejection.

## 4. Design

### 4.1 `src/adapters/oidc.ts`

The readiness route uses two members of `OidcClient`. SMA-705 adds only the first:

```ts
/** SMA-705 D3. Synchronous, no I/O: the state of this client's discovery. */
discoveryStatus(): OidcDiscoveryStatus;
```

The second member is SMA-704's `ensureDiscovered(): Promise<void>`. It resolves when discovery has
succeeded. It starts discovery when none is in flight, and it joins the attempt in flight
otherwise. It rejects with `OidcDiscoveryFailed` exactly as every other method does. SMA-705 does
not add a second member with the same behavior (§ 11, revision 2 → 3).

`export type OidcDiscoveryStatus = 'idle' | 'discovering' | 'discovered';` sits next to
`OidcClient`.

In `createOidcClient`:

- Add `let discovered = false;`. In `getConfig`, add a `.then` BEFORE the existing `.catch` that
  sets `discovered = true` and returns the configuration. The `.catch` does not change: it clears
  `configPromise` and throws `OidcDiscoveryFailed`.
- `discoveryStatus()` returns `'discovered'` when `discovered` is true, `'discovering'` when
  `configPromise` is set, and `'idle'` otherwise.
- `ensureDiscovered()` (SMA-704) is `async () => { await getConfig(); }`. SMA-705 does not change it.

The flag is set inside the promise chain, so the status is `'discovered'` before any caller of the
same promise resumes. A probe, a login and a callback that run at the same time share one
`configPromise`, so the IdP gets one discovery request.

The file header's "DISCOVERY IS LAZY" paragraph gets one sentence: the readiness route (SMA-705)
starts discovery through `ensureDiscovered()`, and it is the first caller in a normal process.
`src/runtime.ts` lines 19-22 get the same sentence.

### 4.2 `src/ports/logger.ts`

- `OidcDiscoveryStage` becomes `'login' | 'callback' | 'readiness'`. Its doc comment names the
  helper `logDiscoveryFailed`.
- `AuthEventName` gets `'readiness.runtime_failed'`.

### 4.3 `src/http/discovery-log.ts` (new) and `src/http/routes.ts`

`logDiscoveryFailed(runtime, stage: OidcDiscoveryStage, err: unknown): void` emits
`oidc.discovery_failed { zone: runtime.zone, stage, reason: oidcDiscoveryReason(err) }`.
`discoveryFailedResponse` in `routes.ts` calls it and no longer calls `runtime.logger.event`
itself. The SMA-656 tests must pass with no edit.

### 4.4 `src/http/readiness.ts` (new)

```ts
export async function readinessResponse(getRuntime: () => Promise<AuthRuntime>, logger: AuthLogger): Promise<Response> {
  let runtime: AuthRuntime;
  try {
    runtime = await getRuntime();          // a synchronous throw lands here too
  } catch (err) {
    logger.event('readiness.runtime_failed', { error: errorName(err) });
    return probeResponse(503, 'unready');
  }
  const status = runtime.oidc.discoveryStatus();
  if (status === 'discovered') return probeResponse(200, 'ready');
  if (status === 'idle') {
    runtime.oidc.ensureDiscovered().catch((err: unknown) => {
      try { logDiscoveryFailed(runtime, 'readiness', err); } catch { /* D11 */ }
    });
  }
  return probeResponse(503, 'unready');
}
```

- `probeResponse` gives `Response.json({ status }, { status: code, headers: { 'cache-control':
  'no-store' } })`.
- `errorName(err)` is `err instanceof Error ? err.name : 'unknown_error'`, the rule of
  `libraryErrorName` in `oidc.ts`. The plan decides whether to share the one helper.
- The `.catch` logs every rejection. It does not test `isOidcDiscoveryFailed`: a rejection that is
  not `OidcDiscoveryFailed` gets `reason: 'other'` from `oidcDiscoveryReason`. It never rethrows.
- The `logger` parameter is for the runtime-failure path only, where no runtime exists. The app
  passes the same logger that it gives `getAuthRuntime`.
- The file header states D2, D3, D7, D8, D10 and D11.

`src/server.ts` exports `readinessResponse` and the type `OidcDiscoveryStatus`.

### 4.5 The apps

`ts/apps/iam-console/app/readyz/route.ts` and `ts/apps/gateway-console/app/readyz/route.ts`, the
same file in each app, with the SPDX header and a short header comment:

```ts
import 'server-only';
import { connection } from 'next/server';
import { readinessResponse } from '@paigasus/auth/server';
import { logger } from '@paigasus/console-core';
import { authRuntime } from '../../lib/auth';

export async function GET(): Promise<Response> {
  await connection();
  return readinessResponse(authRuntime, logger);
}
```

`await connection()` keeps the route dynamic, as on `/healthz`. `getAuthRuntime` keeps the runtime
on `globalThis`, so the readiness route and the auth route see the same `OidcClient` and the same
`configPromise`, although Next gives them separate module graphs. Whether `import 'server-only'`
is used follows the other route files; the plan checks.

Each `proxy.ts` adds `'/readyz'` to `publicPaths` (D9).

### 4.6 `charts/paigasus/templates/console-deployment.yaml`

- `readinessProbe.httpGet.path` becomes `{{ $z.basePath }}/readyz`. `periodSeconds: 10` and
  `failureThreshold: 3` do not change. No `timeoutSeconds` is set (D3).
- The liveness probe stays a TCP check.
- The comment above the probes loses "The consoles have no /readyz; that is a recorded gap (spec
  § 7.8)". It states: readiness is `/readyz`, not ready until the auth runtime is built and one
  OIDC discovery succeeded, and ready after that for the life of the process (SMA-705). Readiness
  does not check Redis (SMA-705 D1).
- The two whole-file fixture copies,
  `ci/helm-render/fixtures/template-only-diff/templates/console-deployment.yaml` and
  `ci/helm-render/fixtures/security-context/templates/console-deployment.yaml`, get the same edit in
  the same commit (`charts/CLAUDE.md`). The plan reads each fixture before it edits it, because a
  fixture can hold an intended difference.
- The golden files `charts/paigasus/tests/golden/iam-and-gateway.yaml` and
  `charts/paigasus/tests/golden/iam-only.yaml` are regenerated. They hold the probe path, so the
  golden render test pins the chart half of A2.
- The chart applies `/readyz` to every zone through its `range`. A future third console must ship
  `app/readyz/route.ts`. Nothing enforces that; § 6 records it.

## 5. Tests

### 5.1 Adapter (`tests/adapters/oidc.test.ts`, with the real local fixture)

The rows count discovery requests with SMA-704's request log of `startOidcFixture`
(`tests/fixtures/jwks.ts`): `fixture.requests().filter((r) => r.endpoint === 'discovery').length`.
Below, "the discovery count" is that value.

- T1. A new client reports `'idle'`. During `ensureDiscovered()` it reports `'discovering'`. After
  `ensureDiscovered()` resolves it reports `'discovered'`.
- T2. Against the unreachable issuer (`http://127.0.0.1:1`), `ensureDiscovered()` rejects with an error for
  which `isOidcDiscoveryFailed` is true. After the rejection the status is `'idle'` again.
- T3. `ensureDiscovered()` and `buildAuthorizationUrl` started together give a discovery count of 1.
- T4. After `'discovered'`, a second `ensureDiscovered()` sends no request, and the status stays
  `'discovered'`.

### 5.2 The fakes

The package `tsconfig.json` includes `tests/`, so every `OidcClient` fake must get
`discoveryStatus()` or `:typecheck` reds. SMA-704 already gave each fake `ensureDiscovered()`. The five fakes:

- `tests/support/store-failure.ts` (`FakeOidc`)
- `tests/next/get-session.test.ts`
- `tests/http/logout.test.ts`
- `tests/http/callback.test.ts`
- `tests/http/route-handler.test.ts`

(`tests/http/login-returnto-table.test.ts` uses `as unknown as` and compiles without a change.)
`FakeOidc` gets a settable status. Its `ensureDiscovered()` records its calls. By default it
resolves at once, as SMA-704's rows expect. When `holdEnsureDiscovered` is true, it resolves or
rejects under test control. The other four get the smallest members that compile.

### 5.3 Handler (`tests/http/readiness.test.ts`, new, with `FakeOidc`)

A detached promise settles after the handler returns. Each row that checks a log or an unhandled
rejection waits for it: it settles the fake's promise, then awaits one `setImmediate`. Node emits
`unhandledRejection` only after the microtask queue drains.

- T5. `'discovered'` → 200, body `{"status":"ready"}`, `cache-control: no-store`, no call to
  `ensureDiscovered()`, no event.
- T6. `'discovering'` → 503, body `{"status":"unready"}`, `cache-control: no-store`, no call to
  `ensureDiscovered()`.
- T7. `'idle'` → 503 `unready`, exactly one call to `ensureDiscovered()`. The response exists before the
  fake settles its promise (the fake holds it open).
- T8. `'idle'`, and `ensureDiscovered()` rejects with `OidcDiscoveryFailed` whose message holds the SMA-656
  sentinel URL → exactly one event `oidc.discovery_failed { zone: 'iam', stage: 'readiness',
  reason }`. Neither `idp.invalid` nor `sentinel-656` gets into the body, a header or a logged field.
- T9. `ensureDiscovered()` rejects with an `OidcDiscoveryFailed` from a second module copy
  (`vi.resetModules()`) → the event has that error's `reason`. It rejects with a plain `Error` →
  one event with `reason: 'other'`. In both cases no `unhandledRejection` occurs (a listener is
  registered).
- T10. `'idle'`, and `ensureDiscovered()` resolves → no event.
- T11. The runtime getter rejects with an error whose message and `input` hold
  `redis://u:sentinel-705@h` → 503 `unready`, one event `readiness.runtime_failed { error:
  '<name>' }`, and `sentinel-705` is in no body, header or logged field. The same for a getter that
  throws synchronously.
- T12. `logger.event` throws inside the discovery `.catch` → no `unhandledRejection`.

### 5.4 Through the real adapter (`tests/http/readiness.test.ts`)

- T13a. A runtime from `createAuthRuntime` with
  `oidcClientFactory: (o) => createOidcClient({ ...o, allowInsecureRequests: true })` and the
  healthy local fixture: the first call gives 503. The test joins the attempt
  (`await runtime.oidc.ensureDiscovered()`), awaits one `setImmediate`, and the next call gives 200.
- T13b. The same factory with the unreachable issuer `http://127.0.0.1:1`: every call gives 503.
  The test joins each attempt with `.catch(() => {})` and one `setImmediate`. Each settled attempt
  logs one `readiness` event with `reason: 'network'`.

### 5.5 The apps

- T14. In each app's `tests/unit/proxy.test.ts`: add `/iam/readyz` (and `/gateway/readyz`) to the
  existing `it.each` of public paths, so a request with no session cookie is not redirected. Also
  add it to the matcher "runs" list.
- T15. The built app serves the route. In each app's `tests/standalone-runtime.test.ts`, one more
  boot of the real standalone server with ONE zone, the memory store, and the issuer
  `https://127.0.0.1:1` (it refuses at once). `GET <basePath>/readyz` gives 503, body
  `{"status":"unready"}`, `cache-control: no-store` and no `location` header. After a bounded wait,
  the server output holds `oidc.discovery_failed` with `"stage":"readiness"`. (The existing boots
  use two zones and the memory store, which `createAuthRuntime` refuses, so they cannot serve this
  row.)
- T16. The positive half through a real build. The iam-console e2e harness has a TLS fake IdP.
  `GET /iam/readyz` reaches 200 within a bound. The plan confirms that the harness can reach that
  state; if not, it stops and reports.
- T17. The helm golden render test (`charts/paigasus/tests/render.sh`) passes with the new path in
  both golden files.

### 5.6 Proof that the tests bite

Run each mutation, record the result in the PR, and restore by an edit (not `git checkout`).

| Mutation | Must fail |
|---|---|
| `discoveryStatus()` never returns `'discovered'` | T1, T4, T13a, T16 |
| The `.catch` in `getConfig` does not clear `configPromise` | T2 |
| `ensureDiscovered()` calls `client.discovery` directly, not `getConfig()` | T3, T4 |
| The handler answers 200 for `'discovering'` | T6 |
| The handler does not start `ensureDiscovered()` for `'idle'` | T7, T13b, T15 |
| The handler starts `ensureDiscovered()` for `'discovering'` too | T6 |
| The handler awaits `ensureDiscovered()` | T7 |
| The handler logs `String(err)` in the discovery event | T8 |
| The handler logs `err.reason` without `oidcDiscoveryReason` | T9 |
| The handler logs only when `err instanceof OidcDiscoveryFailed` | T9 |
| The handler rethrows in its `.catch` | T9 |
| The handler does not wrap the log call in `try` | T12 |
| The handler does not catch a runtime-getter failure | T11 |
| The runtime-failure event holds `String(err)` | T11 |
| Remove `'/readyz'` from `publicPaths` in one app | T14 and T15 for that app |
| Delete `app/readyz/route.ts` in one app | T15 for that app |
| The chart keeps `/healthz` as the readiness path | T17 |

## 6. Known limits

- **A fresh install or a full restart during an IdP outage leaves no ready console pod.** The
  ingress then answers 503 for every console page, also for a signed-in user. A rolling update
  keeps the old ready pods, so the rollout stops and the old pods keep the traffic. This is
  accepted: a pod that cannot discover cannot sign anyone in or refresh a session.
- **A configuration defect keeps the pod not ready for ever.** For example a wrong issuer, a wrong
  CA or a malformed Redis URL. The rollout then stops, and the log shows the `reason` or the error
  name. This is intended.
- **A failing pod logs one event per attempt.** The kubelet probes each 10 s, and each probe after a
  settled failure starts a new attempt. That is about one line each 10 s per pod. A login or a
  callback that joins the attempt adds its own event (D8).
- **Ready is sticky** (D2). An IdP outage after the first success does not make the pod not ready.
  SMA-656 § 6 already records that `/auth/login` then redirects to an IdP that does not answer. A
  pod-local fault after the first success (DNS or egress on one node) does not make it not ready
  either. The reason is in D2.
- **Ready proves discovery only.** It does not prove that the pod reaches `token_endpoint` or
  `jwks_uri`. The kind preflight checks `jwks_uri` separately (`ci/kind/run.sh`). A check of those
  endpoints on each probe would put IdP load on every pod each 10 s, and it is not part of this
  issue.
- **The route is not rate-limited.** The kubelet is not the only caller. The adapter joins an
  attempt in flight, so the IdP gets at most one discovery request at a time per pod. With a fast
  failure (a 404, a refused connection, NXDOMAIN), a caller that loops `/readyz` gets back-to-back
  IdP requests and one log line per attempt. SMA-656's `/auth/login` has the same exposure. A
  minimum interval after a failed attempt is not added: no measurement shows the need.
- **Nothing makes a third console app ship `app/readyz/route.ts`** (§ 4.6).
- **SMA-704.** SMA-704 is fixed on `main`: `resolveSession` runs discovery through `prepareRefresh`
  (`ensureDiscovered()`) before it takes the refresh lock. On a gated deployment the readiness
  probe usually runs discovery first, and a pod that is ready never discovers again. On a
  deployment without this probe (for example local `next start` or a plain Docker run), the first
  refresh runs discovery through `prepareRefresh`. In both cases no discovery runs under the lock.

## 7. Documentation changes

- `ts/packages/paigasus-auth/README.md`: a section on `readinessResponse` (the states, D2, D3, D7,
  D10), the `readiness` stage of `oidc.discovery_failed`, the `readiness.runtime_failed` event, and
  the limits in § 6. Correct lines 189-191 (the first OIDC call is now the readiness route) and
  lines 212-215 (the event meaning names only login and callback). Correct the SMA-656 text that
  says a pod that cannot discover keeps its traffic.
- `docs/ops/RUNBOOK-containers.md` § 6 (console images): the consoles now have `/healthz` for
  liveness-type checks and `/readyz` for readiness, and the chart uses `/readyz`.
- `docs/ops/RUNBOOK-chart.md`: an entry for "console pods stay NotReady after install": read
  `oidc.discovery_failed` and `readiness.runtime_failed` in the pod log.
- `ci/kind/README.md` "Where to look first": a row for the same failure mode.
- SMA-656 spec § 6: the third limit gets "Closed by SMA-705".
- SMA-513 spec § 7.8: mark the Redis part as superseded by SMA-705 D1.
- SMA-506 design § 12: add the `readiness` stage of `oidc.discovery_failed` and the
  `readiness.runtime_failed` event.
- The comments listed in § 4.

## 8. Kind

`ci/kind/run.sh` waits for Keycloak Ready and runs the discovery preflight before
`helm install --wait`. So the new gate lets the install finish. The `chart.yml` kind job runs on
this pull request by itself: its path filters include `charts/**` and
`ts/packages/paigasus-auth/**`. It is not a required check, so the pull request must cite a green
run of it as evidence before merge.

## 9. Rollout

The chart pins both console images at `0.1.0` (`charts/paigasus/values.yaml`). The released
`0.1.0` images have no `/readyz` route and no `/readyz` public path. A chart that probes `/readyz`
against a `0.1.0` image gets the D9 redirect: the gate is not active, and each probe may run the
login route. kind cannot see this, because it uses locally built `:dev` images.

So this pull request bumps both console versions, as the SMA-688 process requires:

- `ts/apps/iam-console/package.json` and `ts/apps/gateway-console/package.json`: `0.1.0` →
  `0.2.0` (a new feature in `0.x`).
- A `CHANGELOG.md` section for `0.2.0` in each app.
- Both image tags in `charts/paigasus/values.yaml` → `"0.2.0"`. `repo:helm-render` row 8a checks
  this.
- The chart `appVersion` does not move in this pull request (`.github/CLAUDE.md`, SMA-696).

Between the merge and the release of the two `0.2.0` images, the chart on `main` pins a tag that
does not exist yet. This window exists for every console version bump under SMA-688. It is not new.

## 10. Out of scope

- A Redis check in readiness (D1).
- An eager discovery at start (D4).
- A readiness change after the first success, for example a periodic re-discovery (D2).
- A `startupProbe`.
- A change to `/healthz`.
- A rate limit on `/readyz` (§ 6).
- The Redis DSN leak on the user request path (D10). SMA-715 tracks it: `redis-store.ts`
  must catch the `createClient` URL error and rethrow a redacted error.

## 11. Challenge changelog

### Integration with SMA-704 (revision 2 → 3)

SMA-704 merged to `main` while this branch was open. A merge of `origin/main` brought it in.

- `OidcClient.discover()` is removed. The readiness route calls SMA-704's `ensureDiscovered()`.
  Reason: both members awaited `getConfig()`, so the port had two members with one behavior (§ 4.1,
  § 4.4).
- The fixture's `discoveryRequests()` counter is removed. The tests count discovery requests in
  SMA-704's `requests()` log. Reason: the fixture had two counters for the same requests (§ 5.1).
- `FakeOidc`'s `ensureDiscovered()` resolves at once by default and holds its promise only when
  `holdEnsureDiscovered` is true. Reason: SMA-704's rows need the default, and T7-T10 and T12 need
  the control (§ 5.2).
- The SMA-704 limit in § 6 now states the fix on `main`. Reason: the old text described a defect
  that `main` no longer has.

### Challenge 1 (revision 1 → 2), verdict "approve with changes"

Folded in:

- No test proved that the built app serves `/readyz` → T15 (standalone server), T16 (e2e harness),
  the mutation "delete `app/readyz/route.ts`".
- D10 was wrong, and a malformed Redis URL would log its password each 10 s (measured) → D10
  rewritten, `readinessResponse` catches the runtime getter, `readiness.runtime_failed`, T11, SMA-715.
- No rollout plan against the released `0.1.0` images → § 9, a version bump in this pull request.
- T11 (old) could not work: `createAuthRuntime` does not pass `allowInsecureRequests` → T13a/T13b
  inject the factory and state the settle procedure.
- T3's counter did not exist → a fixed plan step in § 5.1.
- Five fakes break `:typecheck`, not one → § 5.2.
- The `'readiness'` literal was not checked against `OidcDiscoveryStage` → `logDiscoveryFailed`
  (D8, § 4.3).
- D9's rationale was wrong: the kubelet follows a same-host redirect → D9 rewritten.
- T12 (old) tested a list that is not exported → T14 uses the existing `it.each`.
- D3 said the route never waits, but the first call builds the runtime → D3.
- An unhandled rejection from a throwing logger → D11, T12.
- Two mutation rows missing → § 5.6.
- D2's reason was incomplete, and "ready" does not prove `token_endpoint` or `jwks_uri` → D2, § 6.
- "At most one request at a time" is not a rate limit → § 6.
- Documentation gaps (SMA-513 § 7.8, `runtime.ts`, the README lines, `RUNBOOK-chart.md`,
  `ci/kind/README.md`) → § 7.
- The `HEALTHCHECK` is in `ts/Dockerfile` → D6.
- The route's `WeakMap` added nothing → § 4.5 has no cache.
- A third console must ship the route → § 4.6, § 6.
- § 8 relied on a one-time kind run → the pull request's own kind job, cited.
- The questions on SMA-704 and on two events per failure → § 6, D8.

Not changed:

- An optional minimum interval after a failed attempt. No measurement shows the need (§ 6).
