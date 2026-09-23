# SMA-513 PR 3 — the kind job, the IdP CA value and the docs (spec addendum)

- **Issue:** SMA-513, PR 3 of four. PR 273 (2a), PR 279 (1) and PR 290 (2b) are merged.
- **Parent spec:** `2026-09-19-sma-513-multi-zone-ingress-helm-design.md`. This addendum replaces
  parent § 9 and § 10 where the two disagree. Everything else in the parent stays in force.
- **PR 2b addendum:** `2026-09-22-sma-513-pr2b-helm-render-gate-design.md`. Its § 9 hands two
  items to this PR (§ 8 below).
- **Revision:** 1, before the adversarial challenge.
- **Base:** `beeb8f43`.

---

## 1. What this PR delivers

1. `.github/workflows/chart.yml`: a kind cluster with ingress-nginx, not a required check. It
   shows AC 1 and AC 4, and the user-visible half of AC 2.
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
  (`rs/crates/services/paigasus-iam/src/config.rs:154`), read once at boot. A bad path or a file
  with no PEM certificate is a boot failure
  (`rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs:215-232`).
- Node: `NODE_EXTRA_CA_CERTS`. Node reads it once at process start. A missing file gives only a
  warning, and the process continues.

The chart has no value that mounts a file or adds env. So the chart as merged cannot log in
against any IdP whose CA is not in the public roots. This is a gap for real operators too, not
only for kind.

### F2 — The gateway console reaches IAM with a nav link, so the job needs no gateway backend

`ts/apps/gateway-console/lib/nav.ts:26-28` adds an `IAM` entry to `/iam/orgs` when the zone map
names `iam`. Its state comes from `navStateOf(input.iam)`. The chart deploys the real IAM, so the
entry is `available` and renders as a `<ZoneLink>`. A cross-zone `<ZoneLink>` is a plain `<a>`
(`ts/packages/paigasus-app-shell/src/zone/zone-link.tsx:60,77-81`), which is a hard navigation.

The reverse link, IAM → gateway, is `available` only when a gateway backend answers
`/v1/service-info` (`ts/packages/paigasus-discovery/src/probe.ts:16,85`). The chart does not
deploy the gateway backend (parent D2). So R2 goes from `/gateway` to `/iam`, and the job builds
three images, not four.

### F3 — A fresh IAM accepts a first login with no provisioning

The login callback makes one gRPC call, `authn.whoAmI`
(`ts/packages/paigasus-console-core/src/principal-resolver.ts:72`). IAM provisions the principal
just in time (`rs/crates/services/paigasus-iam/src/application/authenticate_token.rs`). The
principal has no memberships, and the console renders with no organization switcher
(`ts/apps/gateway-console/tests/e2e/login.spec.ts:30-39`). So the job needs no bootstrap admin
and no seed data.

### F4 — IAM needs no NATS and no live IdP at boot

The outbox publisher defaults to `tracing` (`config.rs:559`). JWKS is fetched lazily, on the
first token check. IAM migrates under an advisory lock after it binds its listeners, and `/readyz`
answers 503 while it migrates.

### F5 — The existing console e2e tiers use a fake IdP

Only `ts/packages/paigasus-auth`'s e2e tier starts a real Keycloak
(`quay.io/keycloak/keycloak:26.4`, `tests/e2e/global-setup.ts:49,95-110`). Its realm file
`tests/e2e/keycloak-realm.json` is the model for this PR's realm. Its selectors are `#username`,
`#password` and `#kc-login` (`tests/e2e/roundtrip.spec.ts:16-19`).

### F6 — Workflow guards

- `EXPECTED_PR_SUBJECTS` (`ci/workflow-credentials/workflow_credentials.py:285-292`) holds six
  names and is compared by strict equality against a sorted glob. `chart.yml` sorts first.
- Rules R1–R5 of the same module refuse `secrets:`, `id-token: write`, `write-all`, a read of the
  `secrets` context, and every other `write` scope.
- `ci/actionlint/run.sh` finds workflows by glob. Checks 5 and 6 need block-sequence `branches:`
  and `paths:`, and every `paths:` glob must match a file in the tree.
- No Moon `inputs` edit is needed: both gates take workflows by glob (`moon.yml:671-679`,
  `835-842`).
- No workflow in the repo uses kind or helm actions. Nothing pins `kind`, `kubectl` or
  ingress-nginx.

---

## 3. Decisions

| # | Topic | Decision |
| -- | -- | -- |
| B1 | Job structure | One script, `ci/kind/run.sh`, with modes. `chart.yml` only installs tools and calls the modes |
| B2 | IdP trust | A new optional chart value, `oidc.caBundle` (§ 5). Not a CI post-renderer: the install CI proves must be the install an operator runs |
| B3 | Hosts | Two hosts, one throwaway CA: `console.paigasus.test` (the chart's ingress) and `idp.paigasus.test` (Keycloak's own Ingress) |
| B4 | Name resolution | In pods, a CoreDNS `rewrite` sends both hosts to the ingress-nginx Service. In the browser, Chromium's `--host-resolver-rules` maps both to `127.0.0.1`. The job does not edit `/etc/hosts` |
| B5 | Images | Three: `paigasus-iam`, `iam-console`, `gateway-console`. No gateway backend (F2). Replaces parent § 9's "all four" |
| B6 | Dependencies | Postgres, Redis and Keycloak as plain manifests in `ci/kind/manifests/`, images pinned by digest. Not in the chart (parent § 11) |
| B7 | Spec location | `ts/apps/iam-console/tests/cluster/` with its own Playwright config. Not a Moon task |
| B8 | AC 2 in the cluster | `helm upgrade` to disable the gateway, not a second `helm install`. An operator disables a zone with an upgrade |
| B9 | Done | `chart.yml` is green on this PR's own run |

---

## 4. The cluster

### 4.1 Tools

`kind`, `kubectl` and the ingress-nginx manifest are pinned. The plan decides the mechanism for
each tool: a proto plugin if one works, otherwise a version plus a SHA-256 checksum in
`ci/kind/run.sh`. Each resolved binary is checked with `[ -x ]`, and an absent tool exits rc 2.
`helm` stays on its existing proto pin (`.prototools`, 3.22.0).

### 4.2 Bring-up (`run.sh up`)

1. Make a kind cluster with one node. Map host ports 80 and 443 to the node, and label the node
   `ingress-ready=true`, as the ingress-nginx kind manifest requires.
2. Apply the pinned ingress-nginx kind manifest and wait for the controller to be ready.
3. Make a throwaway CA with `openssl`. Sign a certificate for `console.paigasus.test` and one for
   `idp.paigasus.test`. Store them as two TLS Secrets. Store the CA certificate as the ConfigMap
   `paigasus-idp-ca`, key `ca.crt`.
4. Patch CoreDNS with two `rewrite name` rules that send both hosts to the ingress-nginx
   controller Service. Restart CoreDNS and wait.
5. Apply Postgres, Redis and Keycloak. Keycloak imports the realm from a ConfigMap and has its own
   Ingress on `idp.paigasus.test`. Keycloak must know that it is behind a TLS proxy, so that its
   issuer is `https://idp.paigasus.test/realms/paigasus`. The plan measures the exact Keycloak
   options.
6. Make the three Secrets that the chart refers to, with values generated per run:
   - `oidc.existingSecret`: keys `oidc-client-secret` (equal to the realm client's secret) and
     `session-redis-url`.
   - `postgres.existingSecret`: key `database-url`.
   - `zones.iam.backend.apiKeysPepperSecret`: key `pepper`, 32 random bytes in base64.

### 4.3 The realm

One realm, `paigasus`. One confidential client, `paigasus-console`, with the redirect URIs
`https://console.paigasus.test/iam/auth/callback` and
`https://console.paigasus.test/gateway/auth/callback`, the matching logout URIs, and the web
origin `https://console.paigasus.test`. The audience that IAM expects
(`IAM_AUTHN__ISSUERS`, from `oidc.issuer` and `oidc.clientId`) must be in the access token. The
plan measures which mapper does that. One test user. The client secret in the realm file is a
placeholder that `run.sh` replaces with the per-run value before it applies the file.

### 4.4 Images (`run.sh images`)

Build the three images with `ci/images/run.sh`, then load them with `kind load`. The values files
set each image's `repository` and `tag` to the loaded names, with `pullPolicy: Never` or
`IfNotPresent` so that the kubelet does not try a registry. The plan measures which build and
load path works (`build-oci` plus `kind load image-archive`, or a docker-daemon image plus
`kind load docker-image`).

### 4.5 Install and upgrade

- `run.sh install a`: `helm install --wait` with `ci/kind/values/a.yaml`. Both zones enabled.
  `zones.gateway.backend.url` points at a Service with no endpoints, so the gateway tile is
  `degraded`. `oidc.caBundle.existingConfigMap=paigasus-idp-ca`.
- `run.sh upgrade b`: `helm upgrade --wait` with `ci/kind/values/b.yaml`, which differs from `a`
  only in `zones.gateway.enabled: false`.

### 4.6 Failure evidence (`run.sh diagnose`)

On failure, write to one directory: `kubectl get all -A -o wide`, `describe` for every pod not
Ready, the logs of every pod in the release namespace and of ingress-nginx and Keycloak, the
rendered manifest (`helm get manifest`), and the Playwright traces and report. `chart.yml` uploads
that directory. `run.sh down` deletes the cluster.

### 4.7 Shell constraints

`run.sh` exports `PROTO_REPORTER=text` at the top (root CLAUDE.md). It has no `mapfile`, no
`declare -A` and no here-string, so it runs under bash 3.2 and bash 5. Exit codes: 0 pass, 1 a
spec or assertion failed, 2 an infrastructure error.

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

- Every console pod and the IAM pod get a volume from the ConfigMap, mounted read-only at
  `/etc/paigasus/idp-ca/`.
- Console pods get `NODE_EXTRA_CA_CERTS=/etc/paigasus/idp-ca/<key>`.
- The IAM pod gets `IAM_AUTHN__EXTRA_CA_BUNDLE_PATH=/etc/paigasus/idp-ca/<key>`.
- `version` goes into a pod-template annotation on all three Deployments.

### 5.3 Render when it is empty

Nothing changes. The default render stays byte-identical, so the committed golden files do not
change. If a golden file changes, that is a defect in this PR.

### 5.4 Test

A new chart script, `charts/paigasus/tests/ca-bundle.sh`, in the style of the six existing
scripts. With the value set, it asserts the volume, the mount with `readOnly: true`, and the env
key on each of the three Deployments. With the value empty, it asserts that none of them appears.
It runs every row after a failure (the PR 2a handoff's trap 4) and handles an empty array under
bash 3.2 (trap 3).

### 5.5 Effect on `repo:helm-render`

- Check 6 runs `charts/paigasus/tests/*.sh` by glob with a floor of six
  (`ci/helm-render/run.sh:129-133`). The floor goes to seven.
- Check 3 (rollout) and check 4 (security context) must stay green with the value set. The plan
  verifies both. The new keys are shared, like `oidc.secretVersion`, so a change restarts every
  pod. That is correct for a shared bundle and is not an AC 5 violation.
- If a check needs a change, that change is part of this PR, with its own row and fixture.

### 5.6 Two failure modes (for the runbook)

- IAM refuses to start on a wrong path or a file with no PEM certificate.
- A console starts with a wrong path. Node prints a warning, and the failure shows only at login,
  as a TLS error against the IdP.

---

## 6. The specs

### 6.1 Location and config

`ts/apps/iam-console/tests/cluster/`, with `ts/apps/iam-console/playwright.cluster.config.ts`.
Chromium only. `ignoreHTTPSErrors: true`. Launch argument
`--host-resolver-rules=MAP console.paigasus.test 127.0.0.1, MAP idp.paigasus.test 127.0.0.1`.
`baseURL: https://console.paigasus.test`. Retries 0. Trace and screenshot kept on failure. The
existing `test-e2e` config (`testDir: ./tests/e2e`) must not collect these files, and neither must
vitest. The plan verifies both.

### 6.2 Login helper

It goes to a zone page, fills `#username`, `#password` and `#kc-login`, and waits for the zone
page. It never re-uses or closes a Keycloak tab after a login (SMA-652). A shared session is
proved on a new page.

### 6.3 Phase A — after `install a`

| # | Asserts | AC |
| -- | -- | -- |
| R1 | Log in at `/iam/orgs`. Open a **new page in the same context** and go to `/gateway/overview`. It lands with **zero requests to `idp.paigasus.test`**, and `__Host-pgs_sid` has the same value before and after | 4 |
| R2 | From `/gateway/overview`, click `IAM` in `nav[aria-label="Primary"]`. The element is an `<a>`. A new **document** request to `/iam/orgs` occurs. The page renders its heading | 1 |
| R3-control | In the IAM console's primary nav, the `Gateway` item is present with `aria-disabled="true"` (the `degraded` state) | 2 |

**Why R1 counts IdP requests.** Keycloak keeps its own SSO cookie. A second console login would go
through with no form. So "no form appeared" does not prove that the gateway zone accepted the IAM
zone's session. Zero IdP requests does.

**Why R3 has a control.** A count of zero also passes when the selector is wrong. The control
proves that the same selector finds the item while the zone exists.

### 6.4 Phase B — after `upgrade b`

| # | Asserts | AC |
| -- | -- | -- |
| R3 | The IAM console's primary nav has no `Gateway` item in the DOM (count 0). `/gateway/overview` gets the ingress 404. The gateway console Deployment does not exist | 2 |

**One limit.** With the zone disabled, the nav omits the item at the zone-map rule
(`ts/apps/iam-console/lib/nav.ts:40-43`), before the `absent` rule at
`ts/packages/paigasus-app-shell/src/nav/primary-nav.tsx:74` runs. R3 cannot tell which rule
removed it. Both rules are part of the chart's contract (parent D6), so AC 2 holds either way.

### 6.5 Hazards

- SMA-652: Keycloak's stale-page reload can override a `goto` on the losing tab, and
  `page.close()` on that tab can hang. § 6.2 avoids both.
- SMA-653: `form-action 'self'` blocks a cross-origin redirect. No spec here submits a form to a
  console, so this does not apply. Logout is not in scope.

---

## 7. `chart.yml`

- **Triggers.** `workflow_dispatch`. `push` to `main` and `pull_request` to `main`, both with
  `paths:` `charts/**`, `ci/kind/**`, `ci/helm-render/**`, `ts/apps/**`, `ts/Dockerfile`,
  `.prototools` and `.github/workflows/chart.yml`. Block sequences only.
- **Permissions.** `contents: read`. No `secrets:`, no `id-token`, no read of the `secrets`
  context.
- **Concurrency.** The `images.yml` pattern: one group per workflow, ref and event; a new
  `pull_request` push cancels the old run.
- **Job.** `ubuntu-latest`, `timeout-minutes: 60`. Steps: "Reclaim runner disk" (as in
  `images.yml`); `actions/checkout` with `persist-credentials: false`;
  `moonrepo/setup-toolchain`; `proto install`; `pnpm install` for the Playwright specs; the
  Playwright Chromium install; then `run.sh up`, `images`, `install a`, `specs a`, `upgrade b`,
  `specs b`. On failure, `run.sh diagnose` and `actions/upload-artifact`. `run.sh down` with
  `if: always()`.
- **Pins.** Every action is pinned by commit SHA. Re-use the SHAs already in the repo where one
  exists.
- **Not required.** A broken chart reds `main`, not the pull request, the same trade that
  `images.yml` makes (parent § 12 risk 3).

---

## 8. Registrations

1. `ci/workflow-credentials/workflow_credentials.py`: insert `"chart.yml"` as the **first**
   element of `EXPECTED_PR_SUBJECTS`.
2. `ci/CLAUDE.md:172-174`: the text says "five subject filenames". After this PR the tuple holds
   seven. Correct the text.
3. `ci/helm-render/run.sh`: check 6 floor from six to seven (§ 5.5).
4. `charts/paigasus/README.md`: add `ca-bundle.sh` to the test list, and the new values.

---

## 9. Documentation

- `docs/ops/RUNBOOK-chart.md` (new): the values reference with `oidc.caBundle`, the refused
  combinations (`_helpers.tpl:55-124`), the no-rewrite rule, the upgrade order for a zone-map
  change, D6's limit, the two CA failure modes (§ 5.6), and how to run the kind job.
- `ci/kind/README.md` (new): modes, exit codes, how to read the uploaded evidence, and the local
  run. A local run on the development Mac is best effort, because Docker Desktop's containerd image
  store differs from the runner's store. CI is the reference.
- `ts/CLAUDE.md`: the Docker staging rule (parent F7: a Dockerfile that runs `next build` ships
  no `.next/static`), and "an exec-form `ENTRYPOINT` cannot expand an `ARG`".
- `charts/CLAUDE.md` (new): the ingress has no rewrite annotation; one values block renders
  three projections (`PAIGASUS_ZONES`, `PAIGASUS_SERVICES` and the ingress rule); check 1a couples
  the chart to `service_info.proto`. The root CLAUDE.md memory map gets a row for it.

---

## 10. Verification

- `chart.yml` is green on this PR's own run. That is the measurement (B9). Expect more than one
  CI round.
- **R3 control.** Before the PR is ready, show once that R3 goes red when the upgrade step is
  skipped (the gateway zone stays enabled). A red R3 there proves that its count-zero assertion
  can fail.
- **R1 control.** Show once that R1 goes red when the second page uses a new browser context, so
  it has no session cookie. That proves the zero-IdP-request assertion can fail.
- `repo:helm-render`, `repo:workflow-credentials` and `repo:actionlint` pass, per the bash split
  rules in root CLAUDE.md.
- `ca-bundle.sh` passes under `/bin/bash` 3.2.57 and Homebrew bash 5.3.15. Delete the volume
  block from the template once and show that the script goes red.
- The golden files are unchanged.

---

## 11. Out of scope

- Publishing images or the chart (parent § 11). AC 1 is shown under `kind load` only.
- The gateway backend in the cluster (parent D2).
- Logout, and the SMA-653 path.
- Making `chart.yml` a required check.
- The PR 2b open items: the zone-id floor in `run_checks()`, and the `check_self_invocation`
  refactor.

---

## 12. Residual risks

1. **The kind job is not required.** A regression reaches `main` first (parent § 12 risk 3).
2. **Four new pins** (`kind`, `kubectl`, ingress-nginx, and the Postgres, Redis and Keycloak
   digests) have no Dependabot coverage unless the plan adds it.
3. **The CoreDNS rewrite is a kind-only device.** A real cluster needs real DNS for the IdP host.
   The runbook states this.
4. **`NODE_EXTRA_CA_CERTS` fails open** (§ 5.6). The chart cannot check that the key exists in the
   ConfigMap at render time.
5. **R3 cannot tell which nav rule removed the item** (§ 6.4).
6. **The job is heavy.** A Rust image build plus two Next builds on one runner. The runner disk has
   overflowed once in `images.yml`. The reclaim step is the only control.
