# SMA-514 — E2E harness: auth round-trip and cross-zone navigation

- **Linear:** [SMA-514](https://linear.app/smaschek/issue/SMA-514/ts-e2e-harness-auth-round-trip-and-cross-zone-navigation)
- **Design source:** Frontend Architecture Scoping § 8 (Notion)
- **Depends on:** SMA-513, all four PRs merged (#273, #279, #290, #296). The live stack is
  `ci/kind/run.sh`, run by `.github/workflows/chart.yml`.
- **Status:** design approved in chat on 2026-09-24; this document is the written spec.

## 1. Goal

Add a small Playwright suite that proves two things in a real browser, against the real kind
stack:

1. **Auth round-trip.** A cold visit goes to the IdP, returns through the callback, and gets a
   session and a rendered protected page. Logout then kills the session on the server, not only
   in the browser.
2. **Cross-zone navigation.** A logged-in user clicks the shell's link from `/iam` to `/gateway`
   and back. Each click is a hard navigation, the user is not asked to log in again, and no RSC
   request crosses a zone boundary.

The suite has **exactly two tests**. Feature coverage stays in unit tests and MSW-backed app
tests (issue AC 3).

## 2. Acceptance criteria (from the issue) and how this design meets them

| AC | Meaning | Met by |
|---|---|---|
| 1 | Both scenarios pass against a real stack, in CI | § 5, § 6 run in `chart.yml` against kind (§ 4) |
| 2 | The suite fails loud when the stack is unavailable; it never skips | § 7 |
| 3 | The suite stays at two scenarios | § 7.2: the job asserts exactly 2 tests |

## 3. What exists today (measured on `1035bf80`)

- `ci/kind/run.sh` modes: `up`, `images`, `install a`, `specs a`, `upgrade b`, `specs b`,
  `diagnose`, `down`. `specs <phase>` runs
  `pnpm --dir ts/apps/iam-console exec playwright test --config tests/cluster/playwright.config.ts --project phase-<phase>`,
  first with `--list` (zero tests or a broken config is an infrastructure error, rc 2), then for
  real (a failed spec is rc 1).
- `tests/cluster/playwright.config.ts`: `baseURL https://console.paigasus.test`, Chromium
  `--host-resolver-rules` for `console.paigasus.test` and `idp.paigasus.test`,
  `ignoreHTTPSErrors`, `workers: 1`, `retries: 0`, projects `phase-a` and `phase-b`.
- `tests/cluster/support/login.ts`: `credential()` throws when `PAIGASUS_KIND_USERNAME` or
  `PAIGASUS_KIND_PASSWORD` is missing; `loginAt(page, path)`; `sessionCookie(page)`;
  `redirectChain(response)`; `SESSION_COOKIE = '__Host-pgs_sid'`.
- SMA-513 specs: R1 (IAM login is honored at `/gateway/overview`, opened by URL in a second
  tab), R1-control, R2 (a click on the gateway shell's **IAM** link is a hard navigation),
  R3-control (phase A: the **Gateway** nav entry is `aria-disabled`), R3 (phase B: the entry is
  gone and `/gateway/*` is Traefik's 404).
- The chart does not deploy the gateway backend (`charts/paigasus/values.yaml:49-54`: it needs
  an OpenAI key and a TLS design for its gRPC link to IAM). `values/a.yaml` points
  `zones.gateway.backend.url` at `manifests/gateway-absent.yaml`, a Service with no endpoints.
  So in phase A the gateway zone is `degraded`, and `primary-nav.tsx:91-106` renders the
  **Gateway** entry as `<span role="link" aria-disabled="true">`, which cannot be clicked.
- **Consequence:** the IAM → gateway click, which the issue names as the part that "earns its
  keep", cannot be tested on today's stack. Logout is not tested anywhere (SMA-513 PR 3 spec
  § 11 puts it out of scope).

## 4. Stack change: a gateway stub

### 4.1 Why a stub, and what it proves

Decision **D1** (Sven, 2026-09-24): make the gateway zone `available` with a static stub that
answers only the discovery probe. The consoles, IAM, Keycloak, Redis and Traefik stay real.
The defects this suite looks for live in the consoles (`ZoneLink`, the auth routes, the session
store), so a stub backend does not weaken what the suite proves. The stub is NOT a gateway: it
answers one path and nothing else.

Rejected: deploying the real `paigasus-gateway` (it needs the gRPC TLS design that SMA-513 put
out of scope); navigating IAM → gateway by URL (that is R1, and it does not test `ZoneLink`).

### 4.2 `ci/kind/manifests/gateway-stub.yaml` (replaces `gateway-absent.yaml`)

- **Service** `gateway-stub`, namespace `paigasus`, port 8088 → the stub pod.
- **Deployment** `gateway-stub`, **`replicas: 0`**, a digest-pinned static HTTP server image
  (the implementation picks one small, non-root image and pins it by digest, like the other
  manifests in this directory, so Dependabot covers it). Read-only root filesystem, non-root
  user, resource limits.
- **ConfigMap** with one file, served at `GET /v1/service-info` with
  `Content-Type: application/json`:
  `{"service":"gateway","version":"0.0.0-kind-stub","capabilities":[]}`.
  The probe (`ts/packages/paigasus-discovery/src/probe.ts`) needs a 2xx status, a JSON content
  type, and string `service` and `version`. It ignores the bearer token, and so does the stub.
  The implementation must confirm any further check on `service` (for example that it equals
  the zone's service name) and adjust the JSON.
- `values/a.yaml`: `zones.gateway.backend.url` changes from `gateway-absent` to `gateway-stub`.
  With 0 replicas the Service has no endpoints, so a connection is refused at once — the same
  condition as today. **Phase A and R1–R3 do not change behavior.**
- Every reference to `gateway-absent` (the manifest comment, `run.sh`, `README.md`,
  `RUNBOOK-chart.md`, spec comments in `tests/cluster/`) moves to `gateway-stub`.

### 4.3 Why the stub starts after phase A, not before

Decision **D2** (Sven, 2026-09-24): approach A, a new step with no Helm change. The discovery
cache (`core/record.ts:35`) keeps a failed probe for 10 s (`negativeMs`) but keeps a successful
probe fresh for 60 s and stale for up to 10 min. So `degraded` → `available` takes about 10 s,
and `available` → `degraded` can take minutes. R3-control needs `degraded`, so it runs first.

### 4.4 New `run.sh` modes

- **`stub up`**: `kubectl scale deployment/gateway-stub --replicas=1`, `kubectl rollout status`
  with a timeout, then wait until the Service has a ready endpoint. A timeout exits rc 2
  (infrastructure error), with a message that names the step.
- **`specs e2e`**: the existing `specs` function with the project `e2e`, plus the checks in
  § 7.2. It exports the same `PAIGASUS_KIND_USERNAME`, `PAIGASUS_KIND_PASSWORD` and
  `PAIGASUS_KIND_OUTPUT_DIR`.
- `diagnose` also records the stub's Deployment, pods and endpoints.

### 4.5 `chart.yml`

- Two new steps between `specs a` and `upgrade b`: `run.sh stub up` and `run.sh specs e2e`,
  each with its own `timeout-minutes`.
- Decision **D3** (Sven, 2026-09-24): widen the `pull_request` paths with
  `ts/packages/paigasus-app-shell/**`, `ts/packages/paigasus-auth/**`,
  `ts/apps/iam-console/**` and `ts/apps/gateway-console/**`. The check stays NOT required.
  The cost is about 14 min of runner time on each such PR.
- Order after the change: `up` → `images` → `install a` → `specs a` → `stub up` → `specs e2e`
  → `upgrade b` → `specs b`.

### 4.6 Playwright config

- New project `e2e`, `testDir: './e2e'`.
- `forbidOnly: true`.
- A `json` reporter that writes into `PAIGASUS_KIND_OUTPUT_DIR` next to the HTML report, so
  `run.sh` can read the counts (§ 7.2). The existing phase projects keep their behavior.

## 5. Scenario 1 — `tests/cluster/e2e/auth-roundtrip.spec.ts` (one test)

All steps are in one `test()`. Each step is a `test.step` so that a failure names the step.

1. **Cold visit.** In a new context with no cookies, go to `/iam/orgs`. Assert that the chain
   passes `/iam/auth/login` and reaches `https://idp.paigasus.test/…/protocol/openid-connect/auth`,
   and that the Keycloak form is visible.
2. **Login and callback.** Fill the form with `credential()`. Assert that the browser returns
   through `/iam/auth/callback` to `/iam/orgs` with status 200, that the page hydrates, and that
   `__Host-pgs_sid` exists. Keep its value as `sid`.
3. **Positive control for the replay.** In a second new context, add only the cookie
   `__Host-pgs_sid=sid` (host `console.paigasus.test`, `Secure`, path `/`). Go to `/iam/orgs`.
   Expect 200 and the page content, with no redirect. Without this control, step 5 could pass
   because the replay method is broken, not because the session is dead.
4. **Logout through the shell.** In the first context, open the user menu and select
   **Sign out** (`user-menu.tsx:42-45`, a form POST to `/iam/auth/logout`). Assert that the
   browser goes through the IdP's `end_session` endpoint and back to the console
   (`/iam/auth/logout/callback`, then the public page), and that `__Host-pgs_sid` is gone. This
   step also covers the SMA-653 defect class: a CSP `form-action` that blocks the form's 302 to
   the IdP.
5. **The session is dead on the server.** In a third new context, add the old `sid` cookie
   again. Go to `/iam/orgs` and then to `/gateway/overview`. Each must redirect to its zone's
   `…/auth/login`. The middleware checks only that the cookie exists
   (`get-session.ts` header), so this redirect comes from `requireSession()`'s store lookup and
   proves that the Redis record is gone.
6. **The IdP session is dead too.** In the first context, go to `/iam/orgs`. Assert that the
   Keycloak login form is shown again (no silent SSO login).

## 6. Scenario 2 — `tests/cluster/e2e/cross-zone.spec.ts` (one test)

1. **Log in** at `/iam/orgs` with `loginAt`.
2. **Wait for the stub.** Reload `/iam/orgs` until the Primary nav shows **Gateway** as a real
   link (an `<a>` with an `href`, no `aria-disabled`). Deadline 30 s (the 10 s negative cache
   plus the probe). On timeout, fail with the message "gateway zone never became available: is
   `run.sh stub up` done?".
3. **Record requests.** From here to the end, record every request of the context.
4. **IAM → gateway.** Click `getByRole('navigation', {name: 'Primary'}).getByRole('link', {name:
   'Gateway', exact: true})`. Assert: a request with `resourceType() === 'document'` to
   `/gateway/…`; the final page is in the gateway zone, status 200, hydrated; the
   `__Host-pgs_sid` value did not change; zero requests to `idp.paigasus.test`.
5. **Gateway → IAM.** Click the **IAM** link in the gateway shell's Primary nav. Make the same
   assertions in the reverse direction.
6. **No cross-zone RSC request.** Over the whole recording: no request to a path in the other
   zone that has the `RSC` request header or the `_rsc` query parameter. This includes
   prefetches that happen before a click. This is the `ZoneLink` defect that the issue names.

R2 already covers step 5 as a separate test. Both stay: R2 is SMA-513's evidence, and step 5
completes the round trip in one flow.

### 6.1 Rules for both scenarios

- Follow the SMA-652 rule: never close a tab that Keycloak opened, and never reuse it for a
  later check. Open a new page or a new context.
- No `test.skip`, `test.fixme` or conditional skip anywhere in `tests/cluster/`.
- Reuse `support/login.ts`. Add a helper there only when two tests need it.

## 7. Fail loud and scope (AC 2, AC 3)

### 7.1 A missing or broken stack

- Missing credentials: `credential()` throws.
- No stack: the first navigation fails with a network error, so the test fails.
- The stub is not up: `run.sh stub up` exits rc 2, or scenario 2 step 2 fails with its message.
- Zero tests or a broken config: the existing `--list` guard exits rc 2.

### 7.2 Count checks in `run.sh specs e2e`

- After `--list`: the listed count must be **exactly 2**. Any other count exits rc 2 with the
  message "e2e must hold exactly 2 tests (SMA-514 AC 3)". This stops a third scenario and a
  deleted test.
- After the run: read the JSON report. `expected` must be 2, `skipped` must be 0, `unexpected`
  and `flaky` must be 0. A skip at runtime is a failure, not a green.
- The checks apply to the `e2e` project only. Phases A and B keep their behavior.

### 7.3 Proof that the tests find defects

kind is not installed on the development Mac, and CI is the reference (SMA-513 README). So the
proof runs in CI with `workflow_dispatch` on a throwaway branch that holds one mutation each:

- **M1:** `ZoneLink` renders a `NextLink` for a cross-zone target. Scenario 2 must fail
  (step 4 or step 6).
- **M2:** `handleLogout` does not call `runtime.store.delete(sid)`. Scenario 1 must fail at
  step 5, and step 3 must still pass.
- **M3:** `run.sh specs e2e` with one test changed to `test.skip`. The step must fail on the
  count check.

Each run costs about 14 min. The PR description records the three run URLs and the failing
step of each. The throwaway branch is deleted afterwards and is never merged.

## 8. Documentation

- `ci/kind/README.md`: the mode table gets `stub up` and `specs e2e`; the gateway-stub section
  replaces gateway-absent.
- `docs/ops/RUNBOOK-chart.md`: the new steps and how to re-run one scenario.
- `charts/CLAUDE.md`: no change unless a chart file changes (none is planned).

## 9. Out of scope

- Deploying the real gateway backend in kind.
- Making `chart.yml` a required check.
- Any third scenario, and tests of features.
- Changing R1–R3 beyond the `gateway-absent` → `gateway-stub` rename.
- Running the suite on the development Mac. A local run is best effort, as for SMA-513.

## 10. Decisions

| ID | Decision | By |
|---|---|---|
| D1 | A static `service-info` stub makes the gateway zone `available`; the real gateway is not deployed | Sven, 2026-09-24 |
| D2 | The stub starts after phase A in a new `stub up` step; no Helm change | Sven, 2026-09-24 |
| D3 | Widen `chart.yml`'s `pull_request` paths to the code under test; the check stays not required | Sven, 2026-09-24 |
| D4 | The job asserts exactly 2 tests and 0 skipped for the `e2e` project | design, approved 2026-09-24 |
