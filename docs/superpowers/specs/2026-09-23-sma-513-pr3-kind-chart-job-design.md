# SMA-513 PR 3 — the kind job, the IdP CA value and the docs (spec addendum)

- **Issue:** SMA-513, PR 3 of four. PR 273 (2a), PR 279 (1) and PR 290 (2b) are merged.
- **Parent spec:** `2026-09-19-sma-513-multi-zone-ingress-helm-design.md`. This addendum replaces
  parent § 9 and § 10 where the two disagree. Everything else in the parent stays in force.
- **PR 2b addendum:** `2026-09-22-sma-513-pr2b-helm-render-gate-design.md`. Its § 9 hands two
  items to this PR (§ 8 below).
- **Revision:** 2, after the adversarial challenge. § 13 records what the challenge changed.
- **Base:** `beeb8f43`.

---

## 1. What this PR delivers

1. `.github/workflows/chart.yml`: a kind cluster with an ingress controller, not a required
   check. It shows AC 1 and AC 4, the user-visible half of AC 2, and AC 3 through a real ingress.
2. One optional chart value, `oidc.caBundle`, that lets the pods trust an IdP with a private CA.
   Without it, the chart cannot log in against the kind job's Keycloak (§ 2 F1).
3. `docs/ops/RUNBOOK-chart.md`, and the CLAUDE.md gotchas from parent § 10.
4. The two items from the PR 2b addendum § 9: the `EXPECTED_PR_SUBJECTS` entry and the stale
   count in `ci/CLAUDE.md`.

---

## 2. As-built findings (measured 2026-09-23, at `beeb8f43`)

### F1 — The chart cannot trust an IdP with a private CA

Both consumers of the issuer refuse a non-`https` URL:

- The consoles: `ts/packages/paigasus-auth/src/config.ts:26` (`must be an absolute https URL`).
- IAM: `rs/crates/libs/paigasus-iam-core/src/authn.rs:29` (`strip_prefix("https://")`).

In kind, Keycloak can only have a self-signed certificate. Each side has a way to trust an extra
CA, and each way needs a file in the container:

- IAM: `authn.extra_ca_bundle_path`
  (`rs/crates/services/paigasus-iam/src/config.rs:154`), read once at boot, used only for JWKS
  fetches (`config.rs:141-143`). A bad path or a file with no PEM certificate is a boot failure
  (`rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs:215-232`).
- Node: `NODE_EXTRA_CA_CERTS`. Node reads it once at process start and trusts it for **every** TLS
  connection of the process. A missing file gives only a warning. The console's OIDC client
  honours it: the in-tree e2e harness depends on it
  (`ts/apps/iam-console/tests/e2e/support/harness.ts:155`).

The chart has no value that mounts a file or adds env. So the chart as merged cannot log in
against any IdP whose CA is not in the public roots. This is a gap for real operators too.

### F2 — The gateway console reaches IAM with a nav link, so the job needs no gateway backend

`ts/apps/gateway-console/lib/nav.ts:26-28` adds an `IAM` entry to `/iam/orgs` when the zone map
names `iam`. Its state comes from `navStateOf(input.iam)`. The chart deploys the real IAM, so the
entry is `available` and renders as a `<ZoneLink>`. A cross-zone `<ZoneLink>` is a plain `<a>`
with no client router (`ts/packages/paigasus-app-shell/src/zone/zone-link.tsx:60,77-81`).

The reverse link, IAM → gateway, is `available` only when a gateway backend answers
`/v1/service-info` (`ts/packages/paigasus-discovery/src/probe.ts:16,85`). The chart does not
deploy the gateway backend (parent D2). So R2 goes from `/gateway` to `/iam`, and the job builds
three images, not four.

### F3 — A fresh IAM accepts a first login with no provisioning

The login callback makes one gRPC call, `authn.whoAmI`, with the **access token**
(`ts/packages/paigasus-console-core/src/principal-resolver.ts:71-72`). IAM provisions the
principal just in time (`rs/crates/services/paigasus-iam/src/application/authenticate_token.rs`).
The principal has no memberships. So the job needs no bootstrap admin and no seed data.

A refused token does **not** fail the login (`principal-resolver.ts:12`). The console then shows
IAM as unusable. So a wrong realm or a wrong certificate shows up late and unclearly. § 4.2
step 7 adds a preflight for this reason.

### F4 — IAM's token contract

IAM validates the access token (`rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs`).
It needs:

1. `aud` contains `oidc.clientId` (`charts/paigasus/templates/backend-deployment.yaml:95-96`).
2. An `email` claim, for just-in-time provisioning (`authenticate_token.rs:193-204`).
3. Algorithm RS256 or ES256, and a `kid` in the header (`validator.rs:26,183`).
4. A discovery document whose `issuer` equals the configured issuer and whose `jwks_uri` is
   `https` (`jwks.rs:182-184`).

The console requests the scopes `openid profile email offline_access` by default
(`ts/packages/paigasus-auth/src/config.ts:46`). The in-tree Keycloak realm
(`ts/packages/paigasus-auth/tests/e2e/keycloak-realm.json`) already meets all of this: an
`oidc-audience-mapper` (`:14-26`), `defaultClientScopes` (`:59`), and a user with `email` and the
`offline_access` role (`:67-71`).

The chart comment at `backend-deployment.yaml:92-94` says IAM validates "ID tokens". That is
false. This PR corrects it.

### F5 — IAM needs no NATS and no live IdP at boot

The outbox publisher defaults to `tracing` (`config.rs:559`). JWKS is fetched lazily, on the
first token check. IAM migrates under an advisory lock after it binds its listeners, and `/readyz`
answers 503 while it migrates.

### F6 — Workflow guards

- `EXPECTED_PR_SUBJECTS` (`ci/workflow-credentials/workflow_credentials.py:284-291`) holds six
  names and is compared by strict equality against a sorted glob. `chart.yml` sorts first.
- Rules R1–R5 of the same module refuse `secrets:`, `id-token: write`, `write-all`, a read of the
  `secrets` context, and every other `write` scope.
- `ci/actionlint/run.sh` finds workflows by glob. Checks 5 and 6 need block-sequence `branches:`
  and `paths:`, and every `paths:` glob must match a file in the tree. Check 13 bans a pipe into
  an early-exit reader in every tracked `*.sh` and every workflow (`ci/CLAUDE.md`).
- No Moon `inputs` edit is needed for the workflow file: both gates take workflows by glob.
- No workflow in the repo uses kind or helm actions. Nothing pins `kind`, `kubectl` or an ingress
  controller.

### F7 — The chart-script floor is pinned in two places

`ci/helm-render/run.sh:33` holds `CHART_SCRIPT_FLOOR=6`, and
`ci/affected-graph/ci_targets.py:1407` pins that literal line (`HELM_RENDER_SH_CALL_SITES`,
substring match). A change to one without the other reds `repo:affected-smoke`.

---

## 3. Decisions

| # | Topic | Decision |
| -- | -- | -- |
| B1 | Job structure | One script, `ci/kind/run.sh`, with modes. `chart.yml` only installs tools and calls the modes |
| B2 | IdP trust | A new optional chart value, `oidc.caBundle` (§ 5). Not a CI post-renderer: the install CI proves must be the install an operator runs |
| B3 | Hosts | Two hosts, one throwaway CA: `console.paigasus.test` (the chart's ingress) and `idp.paigasus.test` (Keycloak's own Ingress) |
| B4 | Name resolution | In pods, a CoreDNS `hosts` block maps both names to the ClusterIP of the ingress controller Service. In the browser, Chromium's `--host-resolver-rules` maps both to `127.0.0.1`. The job does not edit `/etc/hosts` |
| B5 | Images | Three: `paigasus-iam`, `iam-console`, `gateway-console`. No gateway backend (F2). Replaces parent § 9's "all four" |
| B6 | Dependencies | Postgres, Redis and Keycloak as plain manifests in `ci/kind/manifests/`, images pinned by digest. Not in the chart (parent § 11) |
| B7 | Spec location | `ts/apps/iam-console/tests/cluster/`, config inside that directory. Not a Moon task |
| B8 | AC 2 in the cluster | `helm upgrade` to disable the gateway, not a second `helm install` |
| B9 | Done | `chart.yml` is green on this PR's own run |
| B10 | Ingress controller | Decided at Gate 1 by a rule: the plan first verifies the ingress-nginx retirement. If it is confirmed, use Traefik. If not, use ingress-nginx (§ 4.1) |
| B11 | Pull policy | No chart change. Kubernetes gives `IfNotPresent` to every tag that is not `latest`, and the loaded images carry non-`latest` tags |

---

## 4. The cluster

### 4.1 Tools and the ingress controller

`kind` and `kubectl` come from `helm/kind-action`, pinned by commit SHA, so the existing
`github-actions` Dependabot entry covers them. The kind node image is pinned by digest to a
Kubernetes 1.31 patch, because the golden files are rendered for 1.31.0
(`ci/helm-render/helm_render.py:47`). `helm` stays on its proto pin (`.prototools`, 3.22.0).

**The controller is decision B10.** Gate 1 approved a rule, not a name: verify the status, then take Traefik if the retirement is confirmed, otherwise ingress-nginx. Upstream announced the retirement of ingress-nginx in
November 2025. The plan verifies the status first. Two options:

- **ingress-nginx, last release, manifest committed** under `ci/kind/manifests/`. It is the kind
  documentation's reference. It gets no more fixes, which matters little for a CI-only harness.
- **A maintained controller** (for example Traefik) that serves `networking.k8s.io/v1` Ingress.
  The chart renders a plain Ingress with no controller annotations, so the chart does not change.
  The runbook can then name a maintained controller for operators.

Whichever is chosen, its manifest or chart is committed or checked by SHA-256, and
`ingress.className` is set to its class in both values files.

### 4.2 Bring-up (`run.sh up`)

Namespaces: `paigasus` (the release, the three Secrets, the CA ConfigMap, and the Service with no
endpoints for the gateway URL) and `paigasus-deps` (Postgres, Redis, Keycloak).

1. Make a kind cluster with one node and the pinned node image. Map host ports 80 and 443 to the
   node, as the controller's kind recipe requires.
2. Install the controller and wait for it with an explicit `--timeout`.
3. Make a throwaway CA and two leaf certificates with `openssl`, with this profile. The CA:
   `basicConstraints=critical,CA:TRUE`, `keyUsage=critical,keyCertSign`. Each leaf: X.509 v3,
   `subjectAltName=DNS:<host>`, `extendedKeyUsage=serverAuth`, `basicConstraints=CA:FALSE`, a CN
   different from the CA's. IAM's rustls and webpki refuse a v1 leaf, a leaf with no SAN, and a CA
   used as a leaf, while Node and Chromium accept them. The precedent is
   `rs/crates/services/paigasus-iam/tests/support/mod.rs:300-341`. Store the leaves as two TLS
   Secrets and the CA as the ConfigMap `paigasus-idp-ca`, key `ca.crt`.
4. Add a `hosts` block to CoreDNS that maps `console.paigasus.test` and `idp.paigasus.test` to
   the controller Service's ClusterIP, with `fallthrough`. A `hosts` answer has the question name
   as its owner, so glibc accepts it (a `rewrite` without `answer auto` may not). Restart CoreDNS
   and wait.
5. Apply Postgres, Redis and Keycloak. Postgres re-uses the digest pin in `ci/images/run.sh:50`.
   Keycloak imports the realm (§ 4.3) and has its own Ingress on `idp.paigasus.test`. It runs
   behind TLS termination, so its hostname and proxy options must make the issuer
   `https://idp.paigasus.test/realms/paigasus`. The plan measures the exact options.
6. Make the three Secrets that the chart refers to, with values generated per run:
   - `oidc.existingSecret`: keys `oidc-client-secret` (equal to the realm client's secret) and
     `session-redis-url`.
   - `postgres.existingSecret`: key `database-url`.
   - `zones.iam.backend.apiKeysPepperSecret`: key `pepper`, 32 random bytes in base64.
7. **Preflight.** A pod that mounts the CA ConfigMap fetches
   `https://idp.paigasus.test/realms/paigasus/.well-known/openid-configuration` through the
   in-cluster name. It asserts `issuer` equals that URL and `jwks_uri` starts with `https://`. A
   failure is rc 2 with the response in the evidence. This turns F3's late, unclear failure into
   an early, named one.

### 4.3 The realm and the credentials

One realm, `paigasus`, modelled on `ts/packages/paigasus-auth/tests/e2e/keycloak-realm.json`. It
meets F4 in full:

- One confidential client, `paigasus-console`, with the redirect URIs
  `https://console.paigasus.test/iam/auth/callback` and
  `https://console.paigasus.test/gateway/auth/callback`, the matching post-logout URIs, and the
  web origin `https://console.paigasus.test`.
- An `oidc-audience-mapper` that puts `paigasus-console` into the access token's `aud`.
- Default client scopes that cover `openid profile email offline_access`.
- One test user with an `email` and the `offline_access` role.

**Credentials.** `run.sh` generates the client secret and the user's password per run. It writes
them into the realm ConfigMap and the Secrets. It passes the password to Playwright in an
environment variable. The committed realm file holds placeholders only. Every credential is a
throwaway that lives for one run.

### 4.4 Images (`run.sh images`)

Build the three images with `ci/images/run.sh`, then load them with `kind load`. The values files
set each image's `repository` and `tag` to the loaded names. The tags are not `latest`, so the
kubelet does not pull (B11). The plan measures which build and load path works (`build-oci` plus
`kind load image-archive`, or a docker-daemon image plus `kind load docker-image`).

### 4.5 Install and upgrade

- `run.sh install a`: `helm install --wait --timeout <n>` with `ci/kind/values/a.yaml`. Both
  zones enabled. `ingress.className` set (B10). `zones.gateway.backend.url` points at the Service
  with no endpoints, so the gateway tile is `degraded`.
  `oidc.caBundle.existingConfigMap=paigasus-idp-ca`.
- `run.sh upgrade b`: `helm upgrade --wait --timeout <n>` with `-f a.yaml -f b.yaml`. `b.yaml` is
  an overlay that holds only `zones.gateway.enabled: false`.
- **Settle before phase B.** After the upgrade, `run.sh` waits until no `iam-console` pod from the
  old ReplicaSet exists. Then it polls `https://console.paigasus.test/gateway/overview` until it
  answers 404, with a time limit. Only then do the phase-B specs start. This removes the race
  between old pods, the Service delete and the controller reload.

### 4.6 Failure evidence (`run.sh diagnose`)

It writes to one directory: `kubectl get all -A -o wide`, `kubectl get events -A`, `describe`
for every pod not Ready, the logs (and `--previous` logs) of every pod in both namespaces and of
the controller, `helm get manifest`, and the Playwright report and traces. It **excludes** Secrets
and the realm ConfigMap. The Playwright traces hold the per-run password, which is a throwaway.
`chart.yml` uploads the directory with a short `retention-days`. `run.sh down` deletes the
cluster.

### 4.7 Shell constraints

`run.sh` exports `PROTO_REPORTER=text` at the top. It has no `mapfile`, no `declare -A`, no
here-string, and no pipe into an early-exit reader (actionlint check 13). So `run.sh`'s own lines
run under bash 3.2 and bash 5. `run.sh images` calls `ci/images/run.sh`, which uses here-strings
(`ci/images/run.sh:121,134,135`), so that mode can hang on a host whose pipe holds 512 bytes (root
CLAUDE.md). The README states this. Every `kubectl wait` and `helm --wait` has an explicit
`--timeout`. Exit codes: 0 pass, 1 a spec or assertion failed, 2 an infrastructure error.

---

## 5. The chart value `oidc.caBundle`

### 5.1 Values

| Key | Default | Meaning |
| -- | -- | -- |
| `oidc.caBundle.existingConfigMap` | `""` | A ConfigMap with PEM root certificates for the IdP. Empty means no change |
| `oidc.caBundle.key` | `ca.crt` | The key in that ConfigMap |
| `oidc.caBundle.version` | `""` | A rotation signal. An operator changes it to restart the pods, as with `oidc.secretVersion` |

A CA certificate is public, so it is a ConfigMap, not a Secret.

### 5.2 Render when `existingConfigMap` is set

- Every console pod and the IAM pod get a `configMap` volume with
  `items: [{key: <key>, path: <key>}]` and no `optional`, mounted read-only at
  `/etc/paigasus/idp-ca/`. With `items`, a missing key or a missing ConfigMap stops the pod at
  volume setup, before the process starts. So the Node-side "warning only" case cannot happen for
  a missing key.
- Console pods get `NODE_EXTRA_CA_CERTS=/etc/paigasus/idp-ca/<key>` in the pod `env`, not in the
  shared `console-env` ConfigMap.
- The IAM pod gets `IAM_AUTHN__EXTRA_CA_BUNDLE_PATH=/etc/paigasus/idp-ca/<key>`.
- `version` goes into a pod-template annotation on all three Deployments, only when it is set.
- `existingConfigMap` set with an empty `key` is refused at render time in `paigasus.validate`.

### 5.3 Render when it is empty

Nothing changes. The default render stays byte-identical, so the committed golden files do not
change. If a golden file changes, that is a defect in this PR.

### 5.4 Tests

- A new chart script, `charts/paigasus/tests/ca-bundle.sh`, in the style of the six existing
  scripts. Rows:
  1. With the value set: on each of the three Deployments, the volume with `items`, the mount with
     `readOnly: true`, and the env key.
  2. With the value empty: none of them appears, and no `version` annotation appears.
  3. With `version` set to two different values: all three `spec.template` values differ (the
     check-3 style).
- A new `refusals.sh` row: `existingConfigMap` set, `key` empty.
- The script runs every row after a failure, and handles an empty array under bash 3.2 (the PR 2a
  handoff's traps 3 and 4).

### 5.5 Effect on `repo:helm-render` and its pins

- Check 6's floor goes from six to seven: `ci/helm-render/run.sh:33` **and** the pin at
  `ci/affected-graph/ci_targets.py:1407`, in the same commit (F7).
- Stale counts after this PR: `ci/helm-render/README.md:55` ("fewer than six chart scripts"),
  `ci/helm-render/README.md:122-123` and `ci/helm-render/helm_render.py:52-53` ("seventh copy",
  "six chart scripts"). `ca-bundle.sh` becomes the eighth copy of the required stub values.
  Update all three.
- Two negative-control fixtures are whole-file copies of `console-deployment.yaml`
  (`ci/helm-render/fixtures/security-context/templates/` and
  `ci/helm-render/fixtures/template-only-diff/templates/`). Re-sync both from the new template,
  keeping each fixture's own mutation, as `ci/helm-render/README.md:85-87` describes. Then run the
  negative control and show each fixture still fails its own row.
- Check 3 (rollout) and check 4 (security context) must stay green with the value set. The plan
  verifies both. A shared bundle change restarts every pod, as `oidc.secretVersion` does. That is
  not an AC 5 violation.
- **The kind values are rendered by the required gate.** A new row in `repo:helm-render` renders
  the chart with `ci/kind/values/a.yaml` and with `a.yaml` plus `b.yaml`, and requires rc 0. So a
  new required chart value that breaks the kind values reds a required check, not only the kind
  job. The row is added to `EXPECTED_ROW_LABELS`. `ci/kind/values/**` is added to the gate's Moon
  inputs.

### 5.6 Failure modes (for the runbook)

- IAM refuses to start on a file with no PEM certificate.
- A missing ConfigMap or key stops every pod that mounts it, at volume setup.
- One fail-open case stays: the key exists, but its content is not a valid PEM certificate. Node
  prints a warning and starts. The failure shows only at login, as a TLS error against the IdP.
- The consoles trust the bundle for every TLS connection, Redis included. IAM trusts it only for
  JWKS fetches. So the bundle should hold only the roots the IdP needs.

---

## 6. The specs

### 6.1 Location and config

- Specs: `ts/apps/iam-console/tests/cluster/phase-a/*.spec.ts` and
  `ts/apps/iam-console/tests/cluster/phase-b/*.spec.ts`. `run.sh specs a` and `run.sh specs b`
  select a phase by directory.
- Config: `ts/apps/iam-console/tests/cluster/playwright.config.ts`, inside `tests/`, so the app's
  existing `tests/**/*` inputs and the `ts` project's `tests` group reach it. A config at the app
  root would get a cached typecheck and lint pass (`ts/apps/iam-console/moon.yml:182-187`,
  `ts/moon.yml:10-26`).
- `outputDir` is under the `diagnose` directory, outside the app directory, because the app
  directory is the Tailwind scan root and Moon hashes it.
- Chromium only. `ignoreHTTPSErrors: true`. Launch argument
  `--host-resolver-rules=MAP console.paigasus.test 127.0.0.1, MAP idp.paigasus.test 127.0.0.1`.
  `baseURL: https://console.paigasus.test`. Retries 0. Trace and screenshot kept on failure.
- Explicit `timeout`, `expect.timeout`, `use.actionTimeout` and `use.navigationTimeout`
  (`ts/CLAUDE.md`: these have no default limit in Playwright 1.63).
- Every spec file is `*.spec.ts`, which the vitest `include` (`vitest.config.ts:34`) does not
  collect. The `test-e2e` config (`testDir: ./tests/e2e`) does not reach `tests/cluster/`.
- `!tests/cluster/**` is added to the `inputs` of `iam-console-ts:test`,
  `iam-console-ts:test-e2e` and the gateway-console two-zone tier
  (`ts/apps/iam-console/moon.yml:263,336`, `ts/apps/gateway-console/moon.yml:353`), so an edit to
  a cluster spec does not start three heavy tiers that never run it. The path stays in
  `typecheck` and `lint`.
- Each test logs in with its own browser context. No state is shared between tests.

### 6.2 Login helper

It goes to a zone page, fills `#username`, `#password` (from the environment) and `#kc-login`,
and waits for the zone page. It never re-uses or closes a Keycloak tab after a login (SMA-652). A
shared session is proved on a new page.

### 6.3 Phase A — after `install a`

| # | Asserts | AC |
| -- | -- | -- |
| R1 | Log in at `/iam/orgs`. Open a **new page in the same context** and go to `/gateway/overview`. It lands with **zero requests to `idp.paigasus.test`**, the document response has `redirectedFrom() === null` (one hop), and `__Host-pgs_sid` has the same value before and after | 4 |
| R1-control | From a **new context** with no cookie, `request.get('/gateway/overview', { maxRedirects: 0 })` redirects to `/gateway/auth/login` (the precedent is `ts/apps/gateway-console/tests/e2e/login.spec.ts:63-65`). This proves that the gateway zone does not admit a request without the session | 4 |
| R2 | Log in at `/gateway/overview` and wait for hydration. Click `IAM` in `nav[aria-label="Primary"]`. A new **document** request to `/iam/orgs` occurs (a hard navigation; a same-zone link would not make one). The page shows the heading "Your organizations" (`ts/apps/iam-console/app/(console)/orgs/page.tsx:86,94-96`), which renders only when IAM accepted the token. Wait for hydration there too | 1, 3 |
| R3-control | In the IAM console's primary nav, `getByRole('link', { name: 'Gateway' })` finds one element, and it has `aria-disabled="true"` (the `degraded` state) | 2 |

**Why R1 counts IdP requests.** Keycloak keeps its own SSO cookie. A second console login would go
through with no form. So "no form appeared" does not prove that the gateway zone accepted the IAM
zone's session. Zero IdP requests does, and R1-control proves that the gateway zone would
otherwise redirect.

**Why R2 waits for hydration.** A hydrated page proves that each zone's `_next` assets load
through the real ingress under its own base path. `docs/ops/RUNBOOK-containers.md:370-374`
records this proof as open and assigns it to the ingress work. So R2 also covers AC 3 through a
real ingress. The wait uses the shape of `ts/apps/iam-console/tests/e2e/support/hydration.ts`.

### 6.4 Phase B — after `upgrade b` and the settle step

| # | Asserts | AC |
| -- | -- | -- |
| R3 | First, the IAM console's primary nav renders and shows `Organizations` (the in-phase control). Then `getByRole('link', { name: 'Gateway' })` in that nav has count 0. The locator ignores the state, so R3 also fails if the item comes back as an available `<a>`. `/gateway/overview` answers 404 from the controller's default backend. The gateway console Deployment does not exist | 2 |

The degraded item is a `span` with `role="link"` (`primary-nav.tsx:98`), so the same role locator
finds it in phase A and in phase B.

**One limit.** With the zone disabled, the nav omits the item at the zone-map rule
(`ts/apps/iam-console/lib/nav.ts:38-41`), before the `absent` rule at
`ts/packages/paigasus-app-shell/src/nav/primary-nav.tsx:74` runs. R3 cannot tell which rule
removed it. Both rules are part of the chart's contract (parent D6), so AC 2 holds either way.

### 6.5 Hazards

- SMA-652: Keycloak's stale-page reload can override a `goto` on the losing tab, and
  `page.close()` on that tab can hang. § 6.2 avoids both.
- SMA-653: `form-action 'self'` blocks a cross-origin redirect. No spec here submits a form to a
  console, so it does not apply. Logout is not in scope.
- Keycloak's large `Set-Cookie` headers can exceed an nginx proxy buffer and give a 502 at login.
  `diagnose` captures the controller logs. If it occurs with ingress-nginx, the fix is the
  `proxy-buffer-size` annotation on the Keycloak Ingress. The README records it.

---

## 7. `chart.yml`

- **Triggers**, split as in `images.yml`:
  - `push` to `main`: broad paths, so a regression anywhere in the tested code starts a run:
    `rs/**`, `ts/**`, `charts/**`, `ci/kind/**`, `ci/images/**`, `ci/helm-render/**`,
    `.prototools`, `.proto/plugins/helm.toml` and `.github/workflows/chart.yml`.
  - `pull_request` to `main`: only the job's own inputs, so a console PR does not pay for a cold
    `--release` IAM build (the SMA-520 reason in `images.yml:21-28`): `charts/**`, `ci/kind/**`,
    `ts/apps/iam-console/tests/cluster/**` and `.github/workflows/chart.yml`.
  - `workflow_dispatch`, for any other PR that wants the job.
  - Block sequences only. Every glob matches a file in the tree after this PR.
- **Permissions.** `contents: read`. No `secrets:`, no `id-token`, no read of the `secrets`
  context.
- **Concurrency.** The `images.yml` pattern: one group per workflow, ref and event; a new
  `pull_request` push cancels the old run.
- **Job.** `ubuntu-latest`, `timeout-minutes: 60`. Steps:
  1. "Reclaim runner disk", as in `images.yml`.
  2. `actions/checkout` with `persist-credentials: false`.
  3. `moonrepo/setup-toolchain`, then `proto install` of only the tools the job needs (`helm`,
     `node`, `pnpm`, and what `ci/images/run.sh` needs), as `images.yml:106-111` does.
  4. The pnpm store cache, as in `ci.yml:155-161`. `pnpm install`. The Playwright Chromium install
     with `pnpm --dir ts/apps/iam-console exec`, as in `ci.yml:206-210`.
  5. `helm/kind-action` for kind and kubectl, without making a cluster (`run.sh up` makes it).
  6. `run.sh up`, `images`, `install a`, `specs a`, `upgrade b`, `specs b`. **Each step has its own
     `timeout-minutes`**, so a hang fails its step inside the job budget.
  7. `run.sh diagnose` and `actions/upload-artifact` with `if: failure() || cancelled()`.
  8. `run.sh down` with `if: always()`.
- **Pins.** Every action is pinned by commit SHA. Re-use the SHAs already in the repo where one
  exists.
- **Not required.** A broken chart reds `main`, not the pull request, the same trade that
  `images.yml` makes (parent § 12 risk 3).
- **Time.** The plan records the step times of a green `images.yml` amd64 leg before it fixes the
  step timeouts. A BuildKit cache is out of scope.

---

## 8. Registrations

1. `ci/workflow-credentials/workflow_credentials.py`: insert `"chart.yml"` as the **first**
   element of `EXPECTED_PR_SUBJECTS`.
2. `ci/CLAUDE.md:172-174`: the text says "five subject filenames". After this PR the tuple holds
   seven. Correct the text.
3. `ci/helm-render/run.sh:33` and `ci/affected-graph/ci_targets.py:1407`: the floor from six to
   seven, in one commit (F7).
4. `ci/helm-render/README.md:55,122-123` and `ci/helm-render/helm_render.py:52-53`: the stale
   counts (§ 5.5).
5. The two fixture re-syncs (§ 5.5).
6. The new `repo:helm-render` row for the kind values, its `EXPECTED_ROW_LABELS` entry and its
   Moon input (§ 5.5).
7. The `!tests/cluster/**` input exclusions (§ 6.1).
8. `charts/paigasus/README.md`: add `ca-bundle.sh` to the test list, and the new values.
9. Dependabot: the plan checks in the dependabot-core source whether the `docker` ecosystem reads
   `image:` lines in plain Kubernetes manifests. If it does, add a `/ci/kind` entry. If it does
   not, record the refresh command for each digest in the manifest, as `ci/images/run.sh:47-49`
   does, and keep residual 2.

---

## 9. Documentation

- `docs/ops/RUNBOOK-chart.md` (new): the values reference with `oidc.caBundle`; the refused
  combinations (`_helpers.tpl:55-124`); the no-rewrite rule; the upgrade order for a zone-map
  change; D6's limit; the failure modes in § 5.6; **the IdP contract in F4**, with the Keycloak
  audience mapper as the example, because a real operator meets the same `aud` rule; the
  controller the kind job uses; and how to run the kind job.
- `charts/paigasus/templates/backend-deployment.yaml:92-94`: correct "ID tokens" to "access
  tokens" (F4).
- `ci/kind/README.md` (new): modes, exit codes, how to read the uploaded evidence, the Keycloak
  proxy-buffer hazard, the here-string limit of `run.sh images`, and the local run. A local run on
  the development Mac is best effort, because Docker Desktop's containerd image store differs from
  the runner's store. CI is the reference.
- `ts/CLAUDE.md`: one line that links the Docker staging rule and the exec-form `ENTRYPOINT` rule
  to where they already are (`rs/CLAUDE.md`, `docs/ops/RUNBOOK-containers.md:317-322,364-369`).
  No third copy.
- `charts/CLAUDE.md` (new): the ingress has no rewrite annotation; one values block renders three
  projections (`PAIGASUS_ZONES`, `PAIGASUS_SERVICES` and the ingress rule); check 1a couples the
  chart to `service_info.proto`; the floor pin in F7. The root CLAUDE.md memory map gets a row for
  it.

---

## 10. Verification

- `chart.yml` is green on this PR's own run. That is the measurement (B9). Expect more than one
  CI round.
- **R3 delete-the-feature proof.** Once, run phase B with the upgrade step skipped. R3 must go red
  at the count assertion. Record the result.
- **R1 delete-the-feature proof.** R1-control is a standing assertion. In addition, once, point
  R1's second page at a new context. R1 must go red. Record the result.
- `ca-bundle.sh` passes under `/bin/bash` 3.2.57 and Homebrew bash 5.3.15. Delete the volume
  block from the template once and show that the script goes red.
- The negative control of `repo:helm-render` passes after the fixture re-sync, and each fixture
  fails its own row.
- `repo:helm-render`, `repo:affected-smoke`, `repo:workflow-credentials` and `repo:actionlint`
  pass, per the bash split rules in root CLAUDE.md.
- The golden files are unchanged.

---

## 11. Out of scope

- Publishing images or the chart (parent § 11). AC 1 is shown under `kind load` only.
- The gateway backend in the cluster (parent D2).
- Logout, and the SMA-653 path.
- Making `chart.yml` a required check.
- A BuildKit cache for the job.
- **An `oidc.audience` value.** The chart uses `oidc.clientId` as the audience. An IdP whose
  access token carries a different `aud` (for example Okta's `api://default`) cannot use the
  chart today without an IdP-side change. File a follow-up issue.
- The PR 2b open items: the zone-id floor in `run_checks()`, and the `check_self_invocation`
  refactor.

---

## 12. Residual risks

1. **The kind job is not required.** A regression reaches `main` first (parent § 12 risk 3). The
   render row in § 5.5 moves one class of break (the values files) into a required gate.
2. **The dependency digests** (Postgres, Redis, Keycloak, and the controller if not
   action-managed) may have no Dependabot coverage (§ 8 item 9).
3. **The CoreDNS `hosts` block is a kind-only device.** A real cluster needs real DNS for the IdP
   host. The runbook states this.
4. **One CA case fails open** (§ 5.6): a key with content that is not PEM.
5. **R3 cannot tell which nav rule removed the item** (§ 6.4).
6. **The job is heavy.** A Rust image build plus two Next builds on one runner. The runner disk has
   overflowed once in `images.yml`. The reclaim step is the only control.
7. **The ingress controller** (B10). If it is ingress-nginx, it gets no more upstream fixes.

---

## 13. What the adversarial challenge changed

Verdict on revision 1: **APPROVE WITH CHANGES**, no blocker, eight major. I checked the
`CHART_SCRIPT_FLOOR=6` pin, the "ID tokens" comment, the `email` requirement, the `images.yml`
pull-request filter reason and the Postgres digest pin against the tree. All five were as the
challenge said.

| Finding | Change |
| -- | -- |
| Floor pin in `ci_targets.py`, stale counts, stale fixtures | F7; § 5.5; § 8 items 3–5 |
| IAM's token contract left to the plan | F4; § 4.3; § 9 runbook; chart comment fixed |
| Certificate profile unspecified | § 4.2 step 3; preflight in step 7 |
| Trigger paths miss tested code; PR filter costs a cold build | § 7 split triggers |
| `pullPolicy` value does not exist | B11; § 4.4 |
| R3 vacuous in phase B; rollout race | § 4.5 settle step; § 6.4 in-phase control and state-free locator |
| A hang gives no evidence | § 4.7 timeouts; § 6.1 Playwright timeouts; § 7 per-step timeouts and `failure() \|\| cancelled()` |
| Config location meets three traps | § 6.1 location, `outputDir`, input exclusions |
| CA volume can fail closed | § 5.2 `items`; § 5.4 rows; § 5.6 |
| Minors | R1-control and one-hop check; R2 heading and no `<a>` assertion; `ingress.className`; CoreDNS `hosts` block; proxy-buffer hazard; check 13; narrow `proto install` and `pnpm --dir`; credentials and evidence exclusions; `NODE_EXTRA_CA_CERTS` scope; namespaces and overlay; re-used pins and `helm/kind-action`; hydration for AC 3; the kind-values render row; citations; no duplicate docs |
| Questions | Controller retirement → B10, decided at Gate 1; `oidc.audience` → out of scope, follow-up; Dependabot for manifests → § 8 item 9; context per test and phase by directory → § 6.1; step times → § 7 |

**Not folded in:** a BuildKit `type=gha` cache. It is an optimisation, and the first green run
must come first. If the job does not fit in 60 minutes, it comes back as a separate change.
