# SMA-514 — E2E harness: auth round-trip and cross-zone navigation

- **Linear:** [SMA-514](https://linear.app/smaschek/issue/SMA-514/ts-e2e-harness-auth-round-trip-and-cross-zone-navigation)
- **Design source:** Frontend Architecture Scoping § 8 (Notion)
- **Depends on:** SMA-513, all four PRs merged (#273, #279, #290, #296). The live stack is
  `ci/kind/run.sh`, run by `.github/workflows/chart.yml`.
- **Status:** design approved in chat on 2026-09-24. Revised after the spec challenge on the
  same day (§ 12).

## 1. Goal

Add a small Playwright suite that proves two things in a real browser, against the real kind
stack:

1. **Auth round-trip.** A cold visit goes to the IdP, returns through the callback, and gets a
   session and a rendered protected page. Logout then kills the session on the server, in both
   zones, and not only in the browser.
2. **Cross-zone navigation.** A logged-in user clicks the shell's link from `/iam` to `/gateway`
   and back. Each click is a hard navigation, the user is not asked to log in again, and no RSC
   request leaves its zone.

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
  real (a failed spec is rc 1). `specs` hard-codes `phase-$phase` (`run.sh:465,477,485`), and
  the dispatch accepts only `a|b` (`run.sh:557`).
- `tests/cluster/playwright.config.ts`: `baseURL https://console.paigasus.test`, Chromium
  `--host-resolver-rules` for `console.paigasus.test` and `idp.paigasus.test`,
  `ignoreHTTPSErrors`, `workers: 1`, `retries: 0`, projects `phase-a` and `phase-b`.
- `tests/cluster/support/login.ts`: `credential()` (not exported, `:13`) throws when
  `PAIGASUS_KIND_USERNAME` or `PAIGASUS_KIND_PASSWORD` is missing; `loginAt(page, path)`;
  `sessionCookie(page)`; `redirectChain(response)`; `SESSION_COOKIE = '__Host-pgs_sid'`.
- SMA-513 specs: R1 (IAM login is honored at `/gateway/overview`, opened by URL in a second
  tab), R1-control, R2 (a click on the gateway shell's **IAM** link is a hard navigation),
  R3-control (phase A: the **Gateway** nav entry is `aria-disabled`), R3 (phase B: the entry is
  gone and `/gateway/*` is Traefik's 404).
- The chart does not deploy the gateway backend (`charts/paigasus/values.yaml:49-54`: it needs
  an OpenAI key and a TLS design for its gRPC link to IAM). `values/a.yaml:30-32` points
  `zones.gateway.backend.url` at `manifests/gateway-absent.yaml`, a Service with no endpoints.
  So in phase A the gateway zone is `degraded`, and `primary-nav.tsx:91-106` renders the
  **Gateway** entry as `<span role="link" aria-disabled="true">`, which cannot be clicked.
- **Consequence:** the IAM → gateway click, which the issue names as the part that "earns its
  keep", cannot be tested on today's stack. Logout is not tested anywhere (SMA-513 PR 3 spec
  § 11 puts it out of scope).
- Both consoles compile a `basePath` (`ts/apps/iam-console/next.config.ts:14`,
  `ts/apps/gateway-console/next.config.ts:13`). There is no root zone.

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
- **Deployment** `gateway-stub`, **`replicas: 0`**. Image: `nginxinc/nginx-unprivileged`
  (or an equivalent small non-root static server), pinned by digest like the other manifests in
  this directory, so Dependabot covers it. Non-root user, read-only root filesystem with
  `emptyDir` mounts for the server's cache, run and temp directories, resource limits, and a
  `readinessProbe` on `/v1/service-info`, so that "ready endpoint" means that the server
  answers.
- **ConfigMap** with two keys: the server configuration and the body.
  - The configuration serves `GET /v1/service-info` with status 200 and
    `Content-Type: application/json`, with no redirect. A static server takes the content type
    from the file extension, and a file named `service-info` has none, so the configuration
    must set it explicitly (for nginx: a `location = /v1/service-info` block with
    `default_type application/json`). The probe requires the essence `application/json`
    (`probe.ts:98`) and uses `redirect: 'error'` (`probe.ts:90`).
  - The body: `{"service":"gateway","version":"0.0.0-kind-stub","capabilities":[]}`.
    No code checks `service` or `version` beyond their type: a mismatch only logs
    `discovery.service_mismatch` (`single-flight.ts:174-181`), and the state takes the
    configured service key (`record.ts:76-79`). The stub ignores the bearer token.
- `values/a.yaml`: `zones.gateway.backend.url` changes from `gateway-absent` to `gateway-stub`.
  With 0 replicas the Service has no endpoints, so a connection is refused at once — the same
  condition as today. **Phase A and R1–R3 do not change behavior.** R3 (phase B) is also safe:
  phase B removes `gateway` from `PAIGASUS_ZONES` and `PAIGASUS_SERVICES`
  (`_helpers.tpl:129-151`), so the nav drops the entry whatever the discovery cache holds.
- Every reference to `gateway-absent` moves to `gateway-stub`: `run.sh:262`,
  `values/a.yaml:30-32`, `tests/cluster/phase-a/nav-degraded.spec.ts:4`, and the manifest file
  itself.

### 4.3 Why the stub starts after phase A, not before

Decision **D2** (Sven, 2026-09-24): approach A, a new step with no Helm change. The discovery
cache (`core/record.ts:35`, `single-flight.ts:269-323`) works as follows:

- A failed probe stays fresh for `negativeMs` (10 s). After that, the next render still serves
  the stale `degraded` record and revalidates in the background. Only a later render shows
  `available`. So `degraded` → `available` takes about 10 s plus two renders.
- A successful probe stays fresh for `freshMs` (60 s). So `available` → `degraded` takes about
  60 s plus one render. `staleMs` (10 min) is only the Redis TTL of the record.

R3-control needs `degraded`, so it runs first, and the stub starts after it.

**Assumption A1:** the chart sets no `PAIGASUS_DISCOVERY_*` variable (a grep of `charts/` finds
none), so the default timings apply. The 30-s deadline in § 6 step 2 depends on this.

### 4.4 `run.sh` changes

- **`specs` refactor.** `specs` takes a project name: `a` → `phase-a`, `b` → `phase-b`,
  `journeys` → `journeys`. The dispatch accepts `specs a|b|journeys`. `USAGE` (`run.sh:61`)
  lists the new modes. The Deployment-absence check after `specs b` stays specific to `b`.
- **`stub up`**: `kubectl scale deployment/gateway-stub --replicas=1`, then
  `kubectl rollout status` with a timeout that allows a slow Docker Hub pull (the stub image is
  pulled only here). Then one in-cluster GET to
  `http://gateway-stub.paigasus.svc.cluster.local:8088/v1/service-info` with the pinned curl
  image. It runs as a Pod manifest `ci/kind/manifests/stub-check.yaml`, like the IdP preflight
  (`run.sh:265-298`), but its log is parsed by the Node module `ci/kind/stub-check.mjs`, not by
  `python3`. Assert status 200, the
  content type `application/json`, and string `service` and `version`. Any failure exits rc 2
  (infrastructure error) with a message that names the check.
- **`stub down`**: scale to 0 and wait until the Service has no endpoints. It is for local
  re-runs only. Order rule: `specs a` needs the stub down and then about 60 s for the cache
  (§ 4.3). The README states this rule.
- **`specs journeys`**: the refactored `specs` function with the project `journeys`, plus the
  checks in § 7.2. It exports the same `PAIGASUS_KIND_USERNAME`, `PAIGASUS_KIND_PASSWORD` and
  `PAIGASUS_KIND_OUTPUT_DIR`.
- `diagnose` also records the stub's Deployment, pods and endpoints.
- All new code obeys `run.sh`'s header constraints: no here-string, and no pipe into a reader
  that exits early. JSON parsing uses `node`, which Playwright already needs; `specs` does not
  need `python3`.

### 4.5 `chart.yml`

- Two new steps after `specs a`: `run.sh stub up` and `run.sh specs journeys`, each with its
  own `timeout-minutes`. Both have
  `if: ${{ !cancelled() && steps.<install-a>.outcome == 'success' }}`. So the journeys run even
  when `specs a` fails. That is necessary for the M1 proof (§ 7.3), because a cross-zone
  `NextLink` also breaks R2. It also keeps journey evidence on a normal run where phase A fails.
  `upgrade b` and `specs b` keep the default `if: success()`: phase B runs only when everything
  before it passed.
- **Job budget.** `timeout-minutes: 150` (`chart.yml:48-55`) is sized to the sum of the step
  timeouts, and today that sum is 139 min. The two new step timeouts are 9 min (`stub up`) and
  10 min (`specs journeys`), so the sum is 158 min and the job budget goes from 150 to 169 min. The
  comment's arithmetic is updated in the same edit.
- Decision **D3** (Sven, 2026-09-24): widen the `pull_request` paths to the code under test.
  The check stays NOT required. The list:
  `ts/packages/paigasus-app-shell/**`, `ts/packages/paigasus-auth/**`,
  `ts/packages/paigasus-discovery/**`, `ts/packages/paigasus-console-core/**`,
  `ts/packages/paigasus-next-config/**`, `ts/packages/paigasus-ui/**`,
  `ts/apps/iam-console/**`, `ts/apps/gateway-console/**`, `ts/pnpm-lock.yaml` (a Next bump is
  the most probable cause of a prefetch or RSC change).
  **Cost, corrected:** each run on such a PR includes a cold `--release` IAM build. The comment
  at `chart.yml:22-24` states the opposite rule ("a console PR does not pay for a cold
  `--release` IAM build") and is rewritten to state D3 and its cost.
- Order after the change: `up` → `images` → `install a` → `specs a` → `stub up` →
  `specs journeys` → `upgrade b` → `specs b`.

### 4.6 Playwright config

- New project `journeys`, `testDir: './journeys'`. The name avoids a clash with the app's
  `tests/e2e/` tier and with `phase-a/cross-zone.spec.ts`.
- `forbidOnly: true` and a `json` reporter that writes into `PAIGASUS_KIND_OUTPUT_DIR` next to
  the HTML report. Both settings are config-level (`playwright.config.ts:17,22`), so phases A
  and B also get the JSON report. `CI=true` already sets `forbidOnly` in CI, so that setting
  changes only local runs.

## 5. Scenario 1 — `tests/cluster/journeys/auth-roundtrip.spec.ts` (one test)

All steps are in one `test()`. Each step is a `test.step` with a fixed title (§ 7.2 checks the
titles).

1. **Cold visit.** In a new context A with no cookies, go to `/iam/orgs`. Assert that the chain
   passes `/iam/auth/login` and reaches `https://idp.paigasus.test/…/protocol/openid-connect/auth`,
   and that the Keycloak form is visible.
2. **Login and callback.** Fill the form with `credential()` (exported from `login.ts` for
   this). Assert that the browser returns through `/iam/auth/callback` to `/iam/orgs` with
   status 200, that the page hydrates, and that `__Host-pgs_sid` exists. Keep its value as
   `sid`.
3. **Positive controls, before logout.**
   - **Replay, both zones.** For each of `/iam/orgs` and `/gateway/overview`: in a new context
     that holds only the cookie `__Host-pgs_sid=sid` (host `console.paigasus.test`, `Secure`,
     path `/`), go to the URL. Expect the protected page with status 200 and no redirect to
     `…/auth/login`. Without this control, step 5 could pass because the replay method is
     broken, not because the session is dead.
   - **IdP SSO.** In a new context that holds only context A's `KEYCLOAK_IDENTITY` and
     `KEYCLOAK_SESSION` cookies (D10) but no
     `__Host-pgs_sid`, go to `/iam/orgs`. Expect a silent login (no Keycloak form) that ends on
     `/iam/orgs`. Without this control, step 6 could pass because Keycloak never kept an SSO
     session.
4. **Logout through the shell.** In context A, open the user menu and select **Sign out**
   (`user-menu.tsx:42-45`, a form POST to `/iam/auth/logout`). Assert this chain, with
   `page.waitForRequest` for the end-session hop (the pattern in
   `docs/superpowers/specs/2026-09-09-sma-506-measurements.md:803-838`):
   POST `/iam/auth/logout` → 302; GET `https://idp.paigasus.test/…/protocol/openid-connect/logout`
   with `client_id`; a request to `https://console.paigasus.test/iam/?state=…`, and a last document with status 200
   and `data-testid="public-home"` (Next can answer `/iam/` with a 308 to `/iam` first; the test
   records the hops as an annotation and does not assert their number). The chart sets no `PAIGASUS_OIDC_POST_LOGOUT_REDIRECT_URI`, so
   the IdP returns to the default `${PUBLIC_ORIGIN}${basePath}/` (`runtime.ts:130`), and
   `/iam/auth/logout/callback` is NOT requested. Then assert that `__Host-pgs_sid` is gone. This
   step also covers the SMA-653 defect class: a CSP `form-action` that blocks the form's 302 to
   the IdP. Keycloak then shows its logout confirmation page, because the request has no
   `id_token_hint`; the test clicks the confirm button (D9, until SMA-681).
5. **The session is dead on the server, in both zones.** For each of `/iam/orgs` and
   `/gateway/overview`, use **one new context per URL** that holds only the old `sid` cookie.
   Go to the URL and assert on the redirect chain (`redirectChain(response)`): (a) the first
   hop's request carried `__Host-pgs_sid=sid` (`request.allHeaders()`); (b) the first hop's
   response is a 3xx, and the next hop is that zone's `/auth/login`. The proxy checks only that
   the cookie exists (`middleware.ts:122`), so (a) plus (b) proves that the redirect came from
   `requireSession()`'s store lookup, and that the Redis record is gone for both zones.
   The chain is NOT stopped at `/auth/login`. Measured on Playwright 1.63 (Task 4 review,
   Decision D8): a `context.route` handler never sees a server-side redirect hop, so a route
   cannot stop it. The real `handleLogin` therefore runs. It deletes the presented sid, which is
   already dead, and sends the context to the Keycloak form. That context is new and is never
   reused or closed after it shows Keycloak, so the SMA-652 rule holds. The step-3 replay
   controls have no stop either: if a control fails, `handleLogin` deletes the live sid, but the
   test has already failed at that control.
6. **The IdP session is dead too.** In a new context that holds the same `KEYCLOAK_IDENTITY` and
   `KEYCLOAK_SESSION` values as step 3's IdP control (D10) (captured before logout), go to `/iam/orgs`. Assert that the
   Keycloak login form is shown (no silent SSO login). This is the exact negative of step 3's
   control. Context A cannot serve: Keycloak's logout response clears context A's IdP cookies,
   so context A would show the form even if the server-side SSO session were alive (final
   review, 2026-09-24). The new context is never reused or closed after it shows Keycloak.

## 6. Scenario 2 — `tests/cluster/journeys/zone-round-trip.spec.ts` (one test)

1. **Record requests.** Attach `context.on('request')` before anything else. Record the URL,
   the method, `resourceType()` and the request headers.
2. **Log in** at `/iam/orgs` with `loginAt`.
3. **Wait for the stub.** Reload `/iam/orgs` until the Primary nav shows **Gateway** as a real
   link (an `<a>` with an `href`, no `aria-disabled`). Deadline 30 s (assumption A1). On
   timeout, fail with a message that includes the degraded reason text next to the entry
   (`primary-nav.tsx:102-104`). Then `network` (stub not up), `bad-response` (wrong content
   type) and `not-implemented` (wrong path) look different.
4. **RSC check after hydration, before any click** (see step 7 for the rule).
5. **IAM → gateway.** Click `getByRole('navigation', {name: 'Primary'}).getByRole('link', {name:
   'Gateway', exact: true})`. The href is `/gateway/` (`ts/apps/iam-console/lib/nav.ts:40`),
   the gateway's public page, which redirects to `/gateway/overview` when the cookie is present
   (`ts/apps/gateway-console/app/(public)/page.tsx:17`). The chain can also start with a
   trailing-slash redirect; the implementation measures it and does not assume it. Assert: a
   request with `resourceType() === 'document'` to `/gateway/…`; the page ends on
   `/gateway/overview`, and the final document response has status 200; the page hydrates; the
   `__Host-pgs_sid` value did not change; zero requests to `idp.paigasus.test`.
6. **Gateway → IAM.** Click the **IAM** link in the gateway shell's Primary nav (href
   `/iam/orgs`). Make the same assertions in the reverse direction.
7. **No RSC request leaves its zone.** An RSC request is a request with the header `rsc`, or
   the header `next-router-prefetch`, or the query parameter `_rsc`. Header names are compared
   without regard to case (Playwright gives them in lower case). The check fails on:
   - an RSC request to a path in the other zone;
   - an RSC request to a nested cross-zone path: `/iam/gateway…` or `/gateway/iam…`. A
     `NextLink` always adds its own zone's `basePath`, so a cross-zone `NextLink` to
     `/gateway/` renders and prefetches `/iam/gateway/?_rsc=…`. That path is in the IAM zone,
     so the first rule alone cannot see the `ZoneLink` defect on this topology;
   - an RSC request with a 404 response.
   The check runs at step 4 and again at the end, over the whole recording.
   **Positive control:** the recording must hold at least one same-zone RSC request. The IAM
   nav's same-zone `NextLink`s prefetch on hydration. Without this control, a wrong match
   (for example on `RSC` in upper case) would pass for any input.

R2 already covers step 6 as a separate test. Both stay: R2 is SMA-513's evidence, and step 6
completes the round trip in one flow.

### 6.1 Rules for both scenarios

- Follow the SMA-652 rule: never close a tab that Keycloak opened, and never reuse it for a
  later check. Open a new page or a new context.
- No `test.skip`, `test.fixme`, `test.fail`, `test.only` or conditional skip anywhere in
  `tests/cluster/`.
- Reuse `support/login.ts`. Add a helper there only when two tests need it.

## 7. Fail loud and scope (AC 2, AC 3)

### 7.1 A missing or broken stack

- Missing credentials: `credential()` throws.
- No stack: the first navigation fails with a network error, so the test fails.
- The stub is not up or serves a bad response: `run.sh stub up` exits rc 2, or scenario 2
  step 3 fails with the degraded reason in its message.
- Zero tests or a broken config: the existing `--list` guard exits rc 2.

### 7.2 Checks in `run.sh specs journeys`

- **Before the run.** Scan `tests/cluster/**` for `.skip(`, `.fixme(`, `.fail(` and `.only(`.
  A match exits rc 1. The scan is in `run.sh`, not in `iam-console-ts:test`: that task's
  inputs exclude `tests/cluster/**` (`moon.yml:313-315`), so Moon would return a cached PASS.
- **After `--list`.** The listed count must be **exactly 2**. Any other count exits rc 2 with
  the message "journeys must hold exactly 2 tests (SMA-514 AC 3)". Then delete any old JSON
  report, as `settle_gateway_404` does for its body (`run.sh:406-408`).
- **After the run.** Read the JSON report with `node`:
  - a missing or unparseable report exits rc 2;
  - for each of the two tests: `expectedStatus` must be `passed`, and the last
    `results[].status` must be `passed`. The `stats.expected` counter alone is not enough: a
    test with `test.fail()` that fails counts as `expected`;
  - `stats.skipped`, `stats.unexpected` and `stats.flaky` must be 0;
  - each test's result must hold all of its `test.step` titles from § 5 and § 6. An early
    `return` passes every counter, but it skips steps.
  A well-formed report that fails a check exits rc 1 (`die_assert`).
- The checks apply to the `journeys` project only. Phases A and B keep their behavior.

### 7.3 Proof that the tests find defects

kind is not installed on the development Mac, and CI is the reference (SMA-513 README). So the
stack-dependent proofs run in CI with `workflow_dispatch` on throwaway branches:

- **Run 1, M1:** `ZoneLink` renders a `NextLink` for a cross-zone target. Scenario 2 must fail
  at step 4 or step 7 (the nested path) and at step 5. The RSC rule checks use `expect.soft`,
  so a failure at step 4 does not stop the test before step 5; the positive controls stay hard.
  R2 also fails in `specs a`; § 4.5's step condition lets the journeys run anyway.
- **Run 2, M2:** `handleLogout` does not call `runtime.store.delete(sid)`. Scenario 1 must fail
  at step 5 while step 3 passes. (Decision D7: M2 runs alone. With a `test.skip` in the same
  run, the pre-run scan of § 7.2 stops the step before scenario 1 runs, so M2 cannot show.)

These checks run locally, with no stack, because `--list` and the scan never touch it:

- **M3:** one test marked `test.skip` — the pre-run scan fails.
- **M3b:** both tests marked `test.skip` — the pre-run scan fails.
- **M4:** a third test — `--list` exits rc 2.

The PR description records the two run URLs, the failing step of each mutation, and the local
output of M3b and M4. The throwaway branches are deleted afterwards and are never merged.

## 8. Documentation

- `ci/kind/README.md`: the mode table gets `stub up`, `stub down` and `specs journeys`; a
  gateway-stub section with the order rule of § 4.4 and the cache timing of § 4.3.
- `docs/ops/RUNBOOK-chart.md`: the new steps and how to re-run one scenario.
- `ts/CLAUDE.md:151`: it says the cluster specs run only from `specs a|b`; it now also names
  `specs journeys`.
- Comments that name the phases: `tests/cluster/support/login.ts:16`,
  `tests/cluster/playwright.config.ts:4`.
- `charts/CLAUDE.md`: no change (no chart file changes).

## 9. Out of scope

- Deploying the real gateway backend in kind.
- Making `chart.yml` a required check.
- Any third scenario, and tests of features.
- Changing R1–R3 beyond the `gateway-absent` → `gateway-stub` rename.
- Running the suite on the development Mac. A local run is best effort, as for SMA-513.
- A root-zone topology. § 6 step 7's nested-path rule is specific to two `basePath` zones.

## 10. Decisions

| ID | Decision | By |
|---|---|---|
| D1 | A static `service-info` stub makes the gateway zone `available`; the real gateway is not deployed | Sven, 2026-09-24 |
| D2 | The stub starts after phase A in a new `stub up` step; no Helm change | Sven, 2026-09-24 |
| D3 | Widen `chart.yml`'s `pull_request` paths to the code under test; the check stays not required | Sven, 2026-09-24; cost corrected in § 4.5, to re-confirm at GATE 1 |
| D4 | The job checks exactly 2 tests, 0 skipped, per-test status and step titles for the `journeys` project | design, 2026-09-24 |
| D5 | `stub up` and `specs journeys` run when `install a` succeeded, even if `specs a` failed; phase B still needs every earlier step to pass | spec challenge, 2026-09-24 |
| D6 | A post-run check on a well-formed report is rc 1; a missing report is rc 2 | spec challenge, 2026-09-24 |
| D7 | Mutation M2 runs alone in CI; M3 is proven locally by the pre-run scan | Sven, 2026-09-24 |
| D8 | § 5 step 5 asserts the redirect chain; no route stop at `/auth/login` (a route cannot see a server redirect hop, measured on Playwright 1.63) | controller ruling, Task 4 review, 2026-09-24 |
| D9 | § 5 step 4 clicks Keycloak's logout confirmation (A2 disproven); SMA-681 removes the click | Sven, 2026-09-24 |
| D10 | § 5 steps 3 and 6 copy only `KEYCLOAK_IDENTITY` and `KEYCLOAK_SESSION`; copying all IdP cookies failed in CI run 36046478837 (cause not reproduced locally; the two-cookie subset is measured for both the control and the negative) | controller, 2026-09-24 |

## 11. Assumptions

- **A1:** default discovery timings (§ 4.3).
- **A2 (DISPROVEN, 2026-09-24):** the assumption was that Keycloak accepts `client_id` without
  `id_token_hint` and returns with no confirmation page. A local run of the pinned Keycloak 26.4
  image with this realm measured a confirmation page ("Do you want to log out?"). Decision D9
  (Sven): § 5 step 4 clicks the confirmation, and SMA-681 makes logout send `id_token_hint` and
  removes the click. The `post.logout.redirect.uris` half holds (`paigasus-realm.json:23`).

## 12. Changes after the spec challenge (2026-09-24)

The challenger's verdict: APPROVE WITH CHANGES. All findings were checked against the code.

| Finding | Change |
|---|---|
| BLOCKER: logout never hits `/auth/logout/callback` | § 5 step 4 asserts the real chain to `/iam/?state=…` |
| BLOCKER: step 5 cleared its own cookie; the gateway half proved nothing; SMA-652 hazard | § 5 step 5: one context per zone, assert the cookie was sent and the 3xx to `/auth/login` (D8) |
| MAJOR: nested cross-zone path invisible to the RSC check | § 6 step 7: nested-path and 404 rules |
| MAJOR: recorder too late, no positive control, header case | § 6 steps 1, 4, 7 |
| MAJOR: M1 cannot reach the journeys | § 4.5 step condition, D5 |
| MAJOR: `test.fail()` passes the count check | § 7.2 per-test status, step titles, scan |
| MAJOR: stub content type and readiness | § 4.2, § 4.4 in-cluster check |
| MAJOR: job budget | § 4.5 |
| MINOR: landing page, cache timing, R3 safety, `service` check, A1, timeout message, config level, `run.sh` refactor, rc, early `return`, IdP control, `credential()` export, path list, docs list, naming, image pull, cheaper mutations | folded into §§ 4–8, 11 |
| QUESTION: "about 14 min" per PR | Wrong. A PR run includes a cold `--release` IAM build. § 4.5 corrected; D3 goes back to Sven |

No finding was rejected.
