<!-- SPDX-License-Identifier: Apache-2.0 -->

# The kind job (`ci/kind/`)

`.github/workflows/chart.yml` installs `charts/paigasus` into a kind cluster behind Traefik. It uses a real Keycloak and runs the Playwright specs in `ts/apps/iam-console/tests/cluster/`. This job is NOT a required check. A broken chart makes `main` fail, not the pull request. `repo:helm-render` row 7 renders `values/a.yaml` alone, then renders `values/a.yaml` + `values/b.yaml` together on every pull request. A values break makes a required check fail.

Spec: `docs/superpowers/specs/2026-09-23-sma-513-pr3-kind-chart-job-design.md`.

## Modes

| Command | Does |
| -- | -- |
| `bash ci/kind/run.sh up` | kind v0.31.0 with a Kubernetes 1.31.14 node; Traefik 3.7.13 (chart 41.6.0, SHA-256 checked); a throwaway CA and two leaves; the CoreDNS `hosts` block; Postgres, Redis and Keycloak; the three chart Secrets; the discovery preflight |
| `bash ci/kind/run.sh images` | `ci/images/run.sh build iam` and `build-console`, then `kind load` the three images: `paigasus-iam:dev`, `iam-console:dev`, `gateway-console:dev` |
| `bash ci/kind/run.sh install a` | `helm install` with `values/a.yaml` (both zones, the CA bundle set), then the IAM settle (SMA-701) and the NOTES check (SMA-691) |
| `bash ci/kind/run.sh specs a` | Playwright project `phase-a` (tests R1, R1-control, R2, R3-control) |
| `bash ci/kind/run.sh stub up` | scale the gateway stub to 1 replica, wait for it, and GET `/v1/service-info` from inside the cluster (`manifests/stub-check.yaml`, `stub-check.mjs`). Any failure is rc 2 |
| `bash ci/kind/run.sh specs journeys` | Playwright project `journeys` (SMA-514: J1 auth round trip, J2 cross-zone round trip). Before the run: the skip scan, the step-title check and the checkers' own unit tests. After `--list`: exactly 2 tests. After the run: the JSON report check (`journeys-report.mjs`) |
| `bash ci/kind/run.sh stub down` | scale the gateway stub to 0 and wait until its Service has no endpoints. For local re-runs only |
| `bash ci/kind/run.sh upgrade b` | `helm upgrade` with `a.yaml` + `b.yaml` (the gateway zone off), then the settle step, the IAM settle (SMA-701) and the NOTES check (SMA-691) |
| `bash ci/kind/run.sh specs b` | Playwright project `phase-b` (test R3), then check that R3's Deployment does not exist |
| `bash ci/kind/run.sh diagnose` | write evidence into `<state>/diagnose` |
| `bash ci/kind/run.sh down` | delete the cluster |

The state directory is `$PAIGASUS_KIND_STATE`, or `$RUNNER_TEMP/paigasus-kind`, or `$TMPDIR/paigasus-kind`. It holds the per-run credentials. Only the `diagnose/` subdirectory is uploaded.

## Exit codes

| Code | Meaning |
| -- | -- |
| 0 | pass |
| 1 | a spec or an assertion failed. A failed `helm install` or `helm upgrade` also reads as 1: the chart is the unit under test |
| 2 | an infrastructure error: a tool missing, the cluster, a dependency, the preflight, a build or an image load |

## Gateway stub (SMA-514)

The chart does not deploy the gateway backend. `values/a.yaml` points `zones.gateway.backend.url` at the Service `gateway-stub` (`manifests/gateway-stub.yaml`). nginx answers `GET /v1/service-info` with a fixed descriptor and 404 on every other path. The stub is not a gateway.

- `up` applies the stub with **0 replicas**. In phase A the Service has no endpoints, so the gateway zone is `degraded`, and R3-control needs that.
- `stub up` runs after `specs a` and makes the zone `available` for the journeys.
- **Order rule for a local re-run.** `specs a` needs the stub down: run `stub down`, then wait about 60 s before `specs a`. The discovery cache keeps a good probe for 60 s (`freshMs`) and a failed probe for 10 s (`negativeMs`). So `available` → `degraded` takes about 60 s plus one render, and `degraded` → `available` takes about 10 s plus two renders. J2 waits up to 30 s for the Gateway link. These times assume the chart sets no `PAIGASUS_DISCOVERY_*` variable (spec assumption A1).

## The journeys checks

`specs journeys` fails with rc 1 when any of these occur. A test file in `tests/cluster/` calls `.skip(`, `.fixme(`, `.fail(` or `.only(`. A step title in `tests/cluster/journeys/` differs from `EXPECTED_STEPS` in `journeys-report.mjs`. The report shows a skipped, failed, flaky or `test.fail()` test, or a step that never ran.

It fails with rc 2 when `--list` finds any count other than 2, or when the report is missing. Change a step title in the spec file and in `EXPECTED_STEPS` in the same commit. The checkers' unit tests run with `node --test ci/kind/journeys-report.test.mjs ci/kind/stub-check.test.mjs`; no Moon task and no required check runs them, only `specs journeys`.

## The NOTES check (SMA-691)

`install a` and `upgrade b` end with `helm get notes`. The release NOTES must contain the line `WARNING (SMA-691): the IAM audience equals oidc.clientId`. `values/a.yaml` and `values/b.yaml` keep the default audience, so the chart must show this warning. This check is the end-to-end positive control of `charts/paigasus/templates/NOTES.txt`, on install and on upgrade.

- A missing line is rc 1. A failed `helm get notes` is rc 2.
- The check is inside `install a`. A NOTES failure there stops the later steps of that run. The rows N0-N6 in `charts/paigasus/tests/env.sh` test the NOTES text offline, on every pull request, so they find a text change first.
- This job is not a required check. If a change deletes the NOTES check, nothing fails (SMA-691 spec, residual R2).

## The IAM settle (SMA-701)

`install a` and `upgrade b` call `settle_iam_service` before the NOTES check. Spec: `docs/superpowers/specs/2026-10-02-sma-701-kind-iam-service-settle-design.md`.

- **Why `--wait` is not enough.** Helm 3.22.0 (`.prototools`) counts a Deployment as ready when `readyReplicas >= replicas - maxUnavailable` (`pkg/kube/ready.go`). The IAM backend Deployment has `replicas: 1` and `maxUnavailable: 1` on purpose (`charts/paigasus/templates/backend-deployment.yaml`). So the floor is 0, and `helm install --wait` can return while IAM has no Ready pod. Then the IAM Service has no endpoint, the console's discovery probe fails, and the IAM nav link stays disabled (PR 316). Do not remove the settle as a copy of `--wait`.
- **Do not use `kubectl wait --for=condition=Available`.** The Deployment controller sets `Available=True` when `availableReplicas >= replicas - maxUnavailable`, which is 0 here too. `kubectl rollout status` compares `availableReplicas` with `updatedReplicas` and has no such floor. Part A uses it, with a 240 s timeout.
- **Part B** runs `node -e` in each Running console pod (`kubectl exec -c console`). The script reads the IAM URL from `PAIGASUS_SERVICES` and asks `<url>/readyz` until it gets HTTP 200, for up to 60 s per pod. A watchdog stops each `kubectl exec` after 20 s. Each console Deployment of the release must have one Running pod to ask. A pod that is shutting down is not asked. When the probe gets an error, it writes `probe: <reason>` to stderr before it prints `000`. The `cannot reach IAM` message shows that line.
- **What a run prints.** `iam settle: start HH:MM:SS UTC, IAM readyReplicas=N`. `readyReplicas=0` means that Helm returned before IAM was Ready, so the race occurred and the settle waited. Then `settled: IAM Ready after N s`, the console Deployments it found, and one `settled: <pod> reaches IAM /readyz (HTTP 200)` line per pod.
- **Helm 4.** Helm 4 uses kstatus for `--wait`, which can change this arithmetic. After a Helm 4 bump the settle stays correct, but it can become redundant. Measure it before you remove it.
- **Assumption A1.** The settle helps only if nothing probes IAM through discovery before the settle ends. A failed probe writes a failed record to Redis, and the settle cannot remove it. Today the console `/readyz` routes do not call discovery, the console liveness probe is `tcpSocket`, discovery has no timer and no warm-up, and the first page render is in `specs a`. A change to any of these can make A1 false.
- **Local re-run hazard.** In CI, each run has a new cluster and a new Redis. A local re-run in the same cluster (`helm uninstall`, then `install a`) can find a failed `iam` record from the earlier attempt. Redis keeps it for 600 s (`staleMs`). The settle then passes, and R2 can fail again. Do one of these: render the page two times, wait 600 s, or delete the record. The key is `pgs:svcinfo:iam` (`pgs:svcinfo:<service>`, no prefix in production, `ts/packages/paigasus-discovery/src/adapters/redis-cache.ts`):
  `kubectl --context kind-paigasus-kind -n paigasus-deps exec deploy/redis -- redis-cli DEL pgs:svcinfo:iam`

| Situation | rc | Message start |
| -- | -- | -- |
| The IAM backend Deployment name or object is not found | 2 | `cannot find the IAM backend Deployment` |
| The IAM rollout is not done in 240 s | 1 | `the IAM backend Deployment did not become Ready` |
| No `iam-console` Deployment, or a console Deployment has no Running pod to check | 2 | `no iam-console Deployment` / `no Running <name> pod to check the IAM data path from` |
| At the 60 s deadline the last answer is another HTTP status | 1 | `<pod> reaches IAM, but it answers HTTP <code>` |
| At the deadline there is no answer, and IAM `readyReplicas` is 0 | 1 | `IAM became NotReady after the rollout` |
| At the deadline there is no answer, and IAM is still Ready | 2 | `<pod> cannot reach IAM` + the exec rc and stderr |
| The probe printed something that is not an HTTP status | 2 | `the IAM probe in <pod> printed` |
| A `kubectl` read fails | 2 | `cannot read the readyReplicas of` / `cannot list the Deployments in` / `cannot list the console pods in` |

## Reading the evidence

On a failure or cancel, `chart.yml` runs `diagnose` and uploads `kind-evidence` (7 days).

- `get-all.txt`, `ingress.txt`, `events.txt`, `coredns.yaml` — the cluster state
- `logs/<namespace>-<pod>.log` and `.previous.log` — every pod in `paigasus`, `paigasus-deps` and `traefik`. `describe/` holds each pod that is not Ready
- `keycloak.log` — Keycloak's own log, under this fixed name, even when the pod name changes
- `helm-manifest.yaml` — what the chart rendered
- `idp-preflight.json` — the discovery document the pods saw
- `playwright/phase-a|b|journeys/` — the HTML report, `report.json`, and the traces of failed tests. For journeys also `skip-scan.txt`, `sources.txt`, `checker-tests.txt`, `list.txt` and `report-check.txt`
- `stub-check.log` — the stub's in-cluster answer; `gateway-stub.txt` — its Deployment, pods and endpoints
- `endpointslices.txt` — every EndpointSlice in `paigasus` (SMA-701)

Secrets and the realm ConfigMap are never collected. The Playwright traces hold the per-run user password as typed. It is a throwaway that is valid only while that one cluster exists.

Where to look first:

- The login ends on the IAM console, but IAM shows as unusable: IAM refused the token. Read `logs/paigasus-*-iam-backend-*.log` for the JWKS fetch or the `aud` check (spec F3, F4). A wrong or missing `aud` is the `info` line "its aud claim holds none of the accepted audiences" (at most one line per issuer in 10 seconds). IAM logs two other refusals at `info`. A token that is not an access token shows "a verified marker shows it is not an access token" (SMA-686). A sender-constrained token shows "it is bound to a key, and IAM cannot check the binding" (SMA-690).
- The preflight fails: read `idp-preflight.json` and `keycloak.log`. A wrong `issuer` comes from Keycloak hostname options. A TLS error comes from the CA or the CoreDNS block.
- `helm install --wait` times out and a console pod is not Ready: the pod failed its `/readyz` probe (SMA-705). Read `logs/paigasus-*-iam-console-*.log` or `logs/paigasus-*-gateway-console-*.log`. Look for `oidc.discovery_failed` with `"stage":"readiness"`. Its `reason` names the fault. A `tls` reason comes from the CA or the CoreDNS block, same as for the preflight. Or look for `readiness.runtime_failed`: its `error` names the error class.
- `install a` or `upgrade b` fails in the IAM settle (SMA-701): read the `kind:` line, and see the table in "The IAM settle". For `cannot reach IAM`, read `endpointslices.txt` for the slice whose name starts with `paigasus-paigasus-iam-backend-`, and its ready addresses, then `logs/paigasus-*-iam-backend-*.log`. For `did not become Ready`, read `describe/` for the IAM pod: an image pull error or a failed probe shows there.

## Hazards

- **Keycloak tabs (SMA-652).** A Keycloak page must not be re-used or closed after a login. The login helper never does either.
- **nginx proxy buffers.** Keycloak's large `Set-Cookie` headers can exceed an nginx proxy buffer and cause a 502 at login. This job uses Traefik, so this does not apply. If the job moves to an nginx controller, set `proxy-buffer-size` on the Keycloak Ingress.
- **Here-strings in `images`.** `ci/images/run.sh` uses here-strings, so `run.sh images` can hang on a host whose new pipe holds 512 bytes (see root `CLAUDE.md`). The other modes do not use here-strings.
- **The CoreDNS `hosts` block is a kind-only device.** A real cluster needs real DNS for the IdP.

## Pins and their refresh

| Pin | Where | Refresh |
| -- | -- | -- |
| kind node image | `run.sh` `KIND_NODE_IMAGE` | the release notes of the kind version `chart.yml` pins |
| kind, kubectl | `chart.yml` (`helm/kind-action` inputs) | kind v0.31.0 is the newest release with a 1.31 node |
| Traefik chart | `run.sh` `TRAEFIK_CHART_VERSION`, `TRAEFIK_CHART_SHA256` | `helm pull traefik --repo https://traefik.github.io/charts --version <v>`; `shasum -a 256` |
| Traefik image | `traefik-values.yaml` | `docker buildx imagetools inspect docker.io/traefik:<v> --format '{{json .Manifest.Digest}}'` |
| Postgres, Redis, Keycloak, curl, nginx-unprivileged (the gateway stub) | `manifests/*.yaml` | Dependabot (`/ci/kind/manifests`); the command is in each file |

## Local run

Best effort only. Docker Desktop's containerd image store differs from the runner's classic store (see root memory: Docker Desktop store vs CI runner), so an image load can fail here and pass in CI. CI is the reference. You need Docker, kind v0.31.0, kubectl, `proto install helm node pnpm`, `pnpm --dir ts install`, the Chromium from `pnpm --dir ts/apps/iam-console exec playwright install chromium`, and free host ports 80 and 443.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/kind/run.sh up
bash ci/kind/run.sh images        # set PAIGASUS_KIND_LOAD=archive if docker-image fails
bash ci/kind/run.sh install a
bash ci/kind/run.sh specs a
bash ci/kind/run.sh stub up
bash ci/kind/run.sh specs journeys
bash ci/kind/run.sh upgrade b
bash ci/kind/run.sh specs b
bash ci/kind/run.sh down
```
