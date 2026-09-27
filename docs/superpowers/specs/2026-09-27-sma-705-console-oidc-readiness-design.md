# SMA-705: a console pod that cannot do OIDC discovery is not ready

- Linear: SMA-705
- Date: 2026-09-27
- Packages: `ts/packages/paigasus-auth`, `ts/apps/iam-console`, `ts/apps/gateway-console`,
  `charts/paigasus`
- Related: SMA-656 (the discovery 503, § 6 known limits), SMA-506 (auth design, § 12),
  `docs/ops/RUNBOOK-containers.md` § 4 (the `/healthz` and `/readyz` probe contract)
- Revision 1.

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

- A1. A pod that has not completed one OIDC discovery does not receive login or callback traffic.
- A2. The Helm chart's readiness probe uses the new route.
- A3. No error object, URL or `reason` from the OIDC library gets into a probe response. The log
  gets only the closed SMA-656 `reason`.
- A4. Tests prove the states of the route and the adapter, and a mutation table proves that the
  tests bite.

## 3. Decisions

- **D1. The gate checks discovery only.** The route does not ping Redis. Redis is shared by every
  console pod, so a Redis fault would take every pod out of rotation. SMA-653 already answers a
  store fault with a 503 page. (Decided at brainstorming.)
- **D2. Ready is sticky.** After one successful discovery the pod stays ready for the life of the
  process. The adapter never discovers again (§ 1), so a later IdP outage cannot change what the
  pod can do. Such an outage affects every pod the same way. Removing all pods would also remove
  the pages of signed-in users.
- **D3. The route never waits for discovery.** It reads a synchronous status. If discovery is idle,
  it starts one without `await`, and answers at once. The next probe sees the result. So the route
  works with the Kubernetes default `timeoutSeconds: 1`. The chart does not need to know
  `PAIGASUS_OIDC_HTTP_TIMEOUT_MS`. (Decided at brainstorming.)
- **D4. No eager discovery at process start.** The first readiness probe starts discovery a few
  seconds after the server binds. No user request reaches a pod that is not ready, so no user
  request pays for discovery. An `instrumentation.ts` hook would save at most one probe period
  (10 s). It would add a start hook to both apps, and Next also runs `register()` in the edge
  runtime. (Decided at brainstorming.)
- **D5. The logic lives once, in `@paigasus/auth`.** `@paigasus/auth/server` exports
  `createReadinessHandler`. Each app adds a thin route file. (Decided at brainstorming.)
- **D6. The route is `<basePath>/readyz`.** This follows the Rust services
  (`RUNBOOK-containers.md` § 4): `/healthz` does not touch a dependency, and `/readyz` does.
  `/healthz` does not change. The Docker `HEALTHCHECK` (`ci/images/run.sh`), the standalone-runtime
  tests and the Playwright harnesses keep using it.
- **D7. The probe body has two values and no reason.** 200 `{"status":"ready"}` or 503
  `{"status":"unready"}`. The route is public, so it does not tell an anonymous caller why
  discovery failed. The operator reads the `reason` in the log (D8).
- **D8. A failure that the route started logs `oidc.discovery_failed` with `stage: 'readiness'`.**
  `OidcDiscoveryStage` gets the member `'readiness'`. SMA-656 D7 made this union for such a stage.
  The fields stay `{ zone, stage, reason }`, and `reason` passes through `oidcDiscoveryReason`. A
  discovery that a login or a callback started logs as it does today. It does not also log a
  `readiness` event.
- **D9. The route is public in `proxy.ts`.** Both apps add `'/readyz'` to `publicPaths`. Without it,
  the middleware redirects a request with no session cookie to the login route. A kubelet
  `httpGet` probe counts any status from 200 to 399 as a success. So a missing entry makes the gate
  pass silently, and it then does nothing. A test in each app pins the entry (T12).
- **D10. A configuration error gives a 500, as on `/healthz`.** The route calls the app's
  `authRuntime()`. If the configuration is not valid, that call throws, and Next answers 500. The
  kubelet reads a 500 as not ready. This is the same behavior that `/healthz` has today.

## 4. Design

### 4.1 `src/adapters/oidc.ts`

Add two members to `OidcClient`:

```ts
/** SMA-705 D3. Synchronous, no I/O: the state of this client's discovery. */
discoveryStatus(): OidcDiscoveryStatus;
/**
 * SMA-705. Resolves when discovery has succeeded. Starts it when none is in flight; joins the one
 * in flight otherwise. Rejects with OidcDiscoveryFailed exactly as every other method does.
 */
discover(): Promise<void>;
```

`export type OidcDiscoveryStatus = 'idle' | 'discovering' | 'discovered';` sits next to
`OidcClient`.

In `createOidcClient`:

- Add `let discovered = false;`. In `getConfig`, add a `.then` BEFORE the existing `.catch` that
  sets `discovered = true` and returns the configuration. The `.catch` does not change: it clears
  `configPromise` and throws `OidcDiscoveryFailed`.
- `discoveryStatus()` returns `'discovered'` when `discovered` is true, `'discovering'` when
  `configPromise` is set, and `'idle'` otherwise.
- `discover()` is `async () => { await getConfig(); }`.

The flag is set inside the promise chain, so the status is `'discovered'` before any caller of the
same promise resumes. A probe, a login and a callback that run at the same time share one
`configPromise`, so the IdP gets one discovery request.

The file header's "DISCOVERY IS LAZY" paragraph gets one sentence: the readiness route (SMA-705)
starts discovery through `discover()`, and it is the first caller in a normal process.

### 4.2 `src/ports/logger.ts`

`OidcDiscoveryStage` becomes `'login' | 'callback' | 'readiness'`. Its doc comment names the second
emitter, `http/readiness.ts`.

### 4.3 `src/http/readiness.ts` (new)

```ts
export function createReadinessHandler(runtime: AuthRuntime): () => Response {
  return () => {
    const status = runtime.oidc.discoveryStatus();
    if (status === 'discovered') return probeResponse(200, 'ready');
    if (status === 'idle') {
      runtime.oidc.discover().catch((err: unknown) => {
        runtime.logger.event('oidc.discovery_failed', {
          zone: runtime.zone,
          stage: 'readiness',
          reason: oidcDiscoveryReason(err),
        });
      });
    }
    return probeResponse(503, 'unready');
  };
}
```

- `probeResponse` gives `Response.json({ status }, { status: code, headers: { 'cache-control':
  'no-store' } })`.
- The `.catch` logs every rejection. It does not test `isOidcDiscoveryFailed`: a rejection from
  `discover()` that is not `OidcDiscoveryFailed` gets `reason: 'other'` from `oidcDiscoveryReason`.
  The catch never rethrows, so no rejection is unhandled.
- The handler is synchronous. It takes no request, because the answer does not depend on the
  request.
- The file header states D2, D3, D7 and D8.

`src/server.ts` exports `createReadinessHandler` and the type `OidcDiscoveryStatus`.

### 4.4 The apps

`ts/apps/iam-console/app/readyz/route.ts` and `ts/apps/gateway-console/app/readyz/route.ts`, the
same file in each app:

```ts
import { connection } from 'next/server';
import { createReadinessHandler, type AuthRuntime } from '@paigasus/auth/server';
import { authRuntime } from '../../lib/auth';

const handlers = new WeakMap<AuthRuntime, () => Response>();

export async function GET(): Promise<Response> {
  await connection();
  const runtime = await authRuntime();
  let handler = handlers.get(runtime);
  if (handler === undefined) {
    handler = createReadinessHandler(runtime);
    handlers.set(runtime, handler);
  }
  return handler();
}
```

`await connection()` keeps the route dynamic, as on `/healthz`. `getAuthRuntime` keeps the runtime
on `globalThis`, so the readiness route and the auth route see the same `OidcClient` and the same
`configPromise`, although Next gives them separate module graphs.

Each `proxy.ts` adds `'/readyz'` to `publicPaths` (D9).

### 4.5 `charts/paigasus/templates/console-deployment.yaml`

- `readinessProbe.httpGet.path` becomes `{{ $z.basePath }}/readyz`. `periodSeconds: 10` and
  `failureThreshold: 3` do not change. No `timeoutSeconds` is set (D3).
- The liveness probe stays a TCP check.
- The comment above the probes loses "The consoles have no /readyz; that is a recorded gap". It
  states that readiness is `/readyz`, which is not ready until one OIDC discovery succeeded
  (SMA-705), and that it stays ready after that.
- The two whole-file fixture copies,
  `ci/helm-render/fixtures/template-only-diff/templates/console-deployment.yaml` and
  `ci/helm-render/fixtures/security-context/templates/console-deployment.yaml`, get the same edit in
  the same commit (`charts/CLAUDE.md`). The implementation plan reads each fixture before it edits
  it, because a fixture can hold an intended difference.
- The golden files `charts/paigasus/tests/golden/iam-and-gateway.yaml` and
  `charts/paigasus/tests/golden/iam-only.yaml` are regenerated. They hold the probe path, so the
  golden render test pins A2. No new `helm-render` check is added.

## 5. Tests (Vitest)

### 5.1 Adapter (`tests/adapters/oidc.test.ts`, with the real local fixture)

- T1. A new client reports `'idle'`. During `discover()` it reports `'discovering'`. After
  `discover()` resolves it reports `'discovered'`.
- T2. Against the unreachable issuer (`http://127.0.0.1:1`), `discover()` rejects with an error for
  which `isOidcDiscoveryFailed` is true. After the rejection the status is `'idle'` again.
- T3. `discover()` and `buildAuthorizationUrl` started together cause ONE request to the discovery
  document. The fixture counts the requests. If `startOidcFixture` has no counter, the plan adds
  one.
- T4. After `'discovered'`, a second `discover()` sends no request and the status stays
  `'discovered'`.

### 5.2 Handler (`tests/http/readiness.test.ts`, new, with `FakeOidc`)

`FakeOidc` (`tests/support/store-failure.ts`) gets a settable status and a `discover()` that
records its calls and resolves or rejects under test control.

- T5. `'discovered'` → 200, body `{"status":"ready"}`, `cache-control: no-store`, no call to
  `discover()`, no event.
- T6. `'discovering'` → 503, body `{"status":"unready"}`, `cache-control: no-store`, no call to
  `discover()`.
- T7. `'idle'` → 503 `unready`, exactly one call to `discover()`. The answer comes before the
  `discover()` promise settles (the fake holds it open).
- T8. `'idle'`, and `discover()` rejects with `OidcDiscoveryFailed` whose message holds the SMA-656
  sentinel URL → exactly one event `oidc.discovery_failed { zone: 'iam', stage: 'readiness',
  reason }`. Neither `idp.invalid` nor `sentinel-656` gets into the body, a header or a logged field.
- T9. `discover()` rejects with an `OidcDiscoveryFailed` from a second module copy
  (`vi.resetModules()`) → the event has that error's `reason`. It rejects with a plain `Error` →
  the event has `reason: 'other'`. In both cases no unhandled rejection occurs (the test registers
  an `unhandledRejection` listener).
- T10. `'idle'`, and `discover()` resolves → no event.

### 5.3 Through the real adapter (`tests/http/readiness.test.ts`)

- T11. A runtime from `createAuthRuntime` with the healthy local fixture: the first call gives 503;
  after the fixture's discovery completes, the next call gives 200. The same with the unreachable
  issuer: every call gives 503, and each settled attempt logs one `readiness` event.

### 5.4 The apps

- T12. In each app's `tests/unit/proxy.test.ts`: a request to `/readyz` with no session cookie is
  not redirected, and `publicPaths` holds `'/readyz'`. The test uses the file's existing
  `request()` helper and the `/iam` or `/gateway` basePath.
- T13. The helm golden render test (`charts/paigasus/tests/render.sh`) passes with the new path in
  both golden files.

### 5.5 Proof that the tests bite

Run each mutation, record the result in the PR, and restore by an edit (not `git checkout`).

| Mutation | Must fail |
|---|---|
| `discoveryStatus()` never returns `'discovered'` | T1, T4, T11 |
| The `.catch` in `getConfig` does not clear `configPromise` | T2 |
| `discover()` calls `client.discovery` directly, not `getConfig()` | T3, T4 |
| The handler answers 200 for `'discovering'` | T6 |
| The handler does not start `discover()` for `'idle'` | T7, T11 |
| The handler starts `discover()` for `'discovering'` too | T6 |
| The handler awaits `discover()` | T7 |
| The handler logs `String(err)` in the event | T8 |
| The handler logs `err.reason` without `oidcDiscoveryReason` | T9 |
| The handler rethrows in its `.catch` | T9 |
| Remove `'/readyz'` from `publicPaths` in one app | T12 for that app |
| The chart keeps `/healthz` as the readiness path | T13 |

## 6. Known limits

- **A fresh install or a full restart during an IdP outage leaves no ready console pod.** The
  ingress then answers 503 for every console page, also for a signed-in user. A rolling update
  keeps the old ready pods, so the rollout stops and the old pods keep the traffic. This is
  accepted: a pod that cannot discover cannot sign anyone in or refresh a session.
- **A configuration defect keeps the pod not ready for ever.** For example a wrong issuer or a
  wrong CA. The rollout then stops, and the `oidc.discovery_failed` event shows the `reason`. This is
  intended.
- **A failing pod logs one event per attempt.** The kubelet probes each 10 s, and each probe after a
  settled failure starts a new attempt. That is about one line each 10 s per pod.
- **An IdP outage after the first success does not make the pod not ready** (D2). SMA-656 § 6
  already records that `/auth/login` then redirects to an IdP that does not answer.
- **The kubelet probe is not the only caller.** An anonymous client can call `/readyz` and start a
  discovery attempt. The adapter joins an attempt in flight, so the IdP gets at most one discovery
  request at a time per pod.

## 7. Documentation changes

- `ts/packages/paigasus-auth/README.md`: a section on `createReadinessHandler` (the states, D2, D3,
  D7), the `readiness` stage of `oidc.discovery_failed`, and the limits in § 6. Correct the SMA-656
  text that says a pod that cannot discover keeps its traffic.
- `docs/ops/RUNBOOK-containers.md` § 6 (console images): the consoles now have `/healthz` for
  liveness-type checks and `/readyz` for readiness, and the chart uses `/readyz`.
- SMA-656 spec § 6: the third limit gets "Closed by SMA-705".
- SMA-506 design § 12: add the `readiness` stage to `oidc.discovery_failed`.
- The comments listed in § 4.

## 8. Kind

`ci/kind/run.sh` waits for Keycloak Ready and runs the discovery preflight before
`helm install --wait`. So the new gate lets the install finish. The implementation plan must
measure this with one kind run before it opens the PR, and record the result.

## 9. Out of scope

- A Redis check in readiness (D1).
- An eager discovery at start (D4).
- A readiness change after the first success, for example a periodic re-discovery (D2).
- A `startupProbe`.
- A change to `/healthz`.
