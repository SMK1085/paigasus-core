# SMA-701: Wait for the IAM Service after the kind chart install

- **Linear:** [SMA-701](https://linear.app/smaschek/issue/SMA-701)
- **Branch:** `feature/sma-701-kind-iam-settle`
- **Related:** SMA-514 (the kind job, the summed step timeouts), PR 316 (the failing run)
- **Status:** DRAFT, waiting for approval (Gate 1). Sven approved the design in chat on
  2026-10-02 (§0). The spec-challenger reviewed it on 2026-10-02 (APPROVE WITH CHANGES, no
  BLOCKER); §10 lists what changed. An earlier draft from 2026-09-27 is lost. Only its Linear
  comment remains. This spec replaces it and agrees with that comment.

## 0. Decisions (Sven, 2026-10-02)

- **D1: no `probeService` retry in SMA-701.** The issue lists it as optional. SMA-701 changes only
  the kind job and its documents. A retry changes product behaviour on every page, so it is out of
  scope.
- **D2: the wait runs in `install a` AND in `upgrade b`.** It is one helper with two call sites.
- **D3: approach 1.** First `kubectl rollout status` for the IAM Deployment, then a poll from each
  console pod to the IAM Service through `kubectl exec` and `node`. §8 gives the rejected
  alternatives.

## 1. Problem

On PR 316, run 36258937662, attempt 1, the job "kind install + upgrade (amd64)" failed in the step
"specs a". The test `ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts:12` waited 15 s
for a click on the primary nav link "IAM". The link had `aria-disabled="true"`. Attempt 2 passed
with no code change.

The facts from the issue and the kind-evidence artifact:

| Fact | Value |
|---|---|
| `install a` duration, failing attempt | about 2.6 s (17:35:30.20 to 17:35:32.78 UTC) |
| `install a` duration, passing run 36257619590 on `main` | about 12.5 s (17:13:02.83 to 17:13:15.29 UTC) |
| First IAM request served, failing attempt | 17:35:31.259 |
| gRPC `Unavailable` from IAM recorded | 17:35:36.151 |
| Discovery event | `discovery.probe_failed`, reason `network` |

## 2. Cause

### 2.1 `helm install --wait` does not wait for IAM

`.prototools:12` pins `helm = "3.22.0"`. In Helm 3.22.0, `pkg/kube/ready.go:301-302` counts a
Deployment as ready when this is true:

```go
expectedReady := *dep.Spec.Replicas - deploymentutil.MaxUnavailable(*dep)
if !(rs.Status.ReadyReplicas >= expectedReady) { /* not ready */ }
```

The IAM backend Deployment (`charts/paigasus/templates/backend-deployment.yaml`, about lines
28-39) has `replicas: 1` and the strategy `maxSurge: 0, maxUnavailable: 1`. The chart sets this on
purpose. So `expectedReady` is `1 - 1 = 0`, and Helm counts IAM as ready with zero Ready pods.

`serviceReady` (`ready.go:253-270`) checks only that a ClusterIP exists, plus the ingress of a
LoadBalancer. It does not check endpoints.

So `install_a` (`ci/kind/run.sh:386-399`) can return before the IAM pod is Ready. Before the pod
is Ready, the IAM Service has no endpoint, and a connection to it fails.

The same arithmetic applies to `kubectl wait --for=condition=Available`. The Deployment controller
sets `Available=True` when `availableReplicas >= replicas - maxUnavailable`, which is 0 here.
`kubectl rollout status` is different: it compares `availableReplicas` with `updatedReplicas` and
has no `maxUnavailable` floor.

### 2.2 How one failed probe disables the link for the whole test

1. The gateway console resolves IAM from `PAIGASUS_SERVICES`. The chart builds the URL
   `http://paigasus-paigasus-iam-backend:8080` (`charts/paigasus/templates/_helpers.tpl:169`).
   The console container gets this variable through `envFrom`
   (`charts/paigasus/templates/console-deployment.yaml:83-85`).
2. `probeService` (`ts/packages/paigasus-discovery/src/probe.ts:80-119`) sends one
   `GET /v1/service-info` with a 1.5 s timeout. A thrown fetch error (connection refused, DNS)
   becomes the reason `network` (`src/core/reasons.ts:22-29`).
3. The descriptor cache keeps the failed record. In kind the cache is Redis:
   `charts/paigasus/templates/console-env-configmap.yaml:15` sets `PAIGASUS_SESSION_STORE: "redis"`.
   Both consoles share one key space (`src/adapters/redis-cache.ts:45`), and Redis runs in the
   namespace `paigasus-deps`. So the record outlives every console pod and the Helm release. A
   failed record is fresh for 10 s (`negativeMs`) and is kept for 600 s (`staleMs`,
   `src/core/record.ts:35-42`). After 10 s, the next render still serves the failed record as
   `degraded` and starts a re-probe in the background (`src/core/single-flight.ts:269-323`). Only
   a later render sees the new result. `ci/kind/README.md:41` already states this: "about 10 s plus
   two renders". The probe runs only at server render. No timer and no warm-up exist.
4. `navStateOf` (`ts/packages/paigasus-app-shell/src/nav/state.ts`) turns the failure into
   `degraded`. `ts/packages/paigasus-app-shell/src/nav/primary-nav.tsx:96-98` renders a `span`
   with `aria-disabled="true"` and no `href`.
5. The spec renders the page once after login and then clicks at once (`cross-zone.spec.ts:12`
   onward). It does not reload. Playwright waits for the link to be enabled until `actionTimeout`
   (15 s, `ts/apps/iam-console/tests/cluster/playwright.config.ts:38`), and `retries` is 0.
   Nothing renders the page again, so the link stays disabled until the timeout.

### 2.3 Assumption A1: nothing probes IAM before `install a` returns

The fix holds only if no process calls service discovery between the chart install and the end of
the new wait. A probe in that window would write a failed record to Redis, and the wait cannot
remove it. The evidence for A1:

- Both console `/readyz` routes call only `readinessResponse`
  (`ts/apps/gateway-console/app/readyz/route.ts:12-15`, the same in `iam-console`).
- The console liveness probe is `tcpSocket`.
- Discovery has no warm-up and no timer (§2.2 step 3).
- `cross-zone.spec.ts` (R2) is the first spec file of phase A, so the first page render is in
  `specs a`.

**Local re-run hazard.** A1 is true in CI, because each run starts a new cluster and a new Redis.
A local re-run in the same cluster (`helm uninstall`, then `install a`) can find a failed record
from the earlier attempt in Redis. The helper then passes, and R2 fails again. The remedies are:
render the page twice, wait 600 s, or delete the discovery key for `iam` in Redis. The README note
(§4.4) records this.

### 2.4 What is not proven

- The evidence agrees with §2.1, but no run recorded the IAM pod's Ready time. The first served
  request at 17:35:31.259 can be a kubelet probe, or a request on the gRPC port, which do not need
  the Service endpoint. The IAM readiness probe starts only after the startup probe passes
  (`backend-deployment.yaml:148-164`). It has `periodSeconds: 10`. One 503 at the first readiness
  check delays Ready by 10 s. This fits the failure at 17:35:36.151. The red proof R1 (§6) and the
  entry line of the helper (§4.1) measure §2.1 directly.
- A second race is possible after the pod is Ready: kube-proxy can program the Service rules a
  short time after the endpoint appears. No run has shown it, and the kind cluster has one node
  (`ci/kind/cluster.yaml`). Part B of the helper (§4.1) covers it.

## 3. Goals and non-goals

**Goals**

- G1: The step `install a` returns only after IAM answers HTTP 200 on `/readyz` through its Service,
  as seen from each console pod.
- G2: The wait polls with a deadline, as `settle_gateway_404` (`ci/kind/run.sh:423-443`) does. It
  does not use a fixed sleep.
- G3: The same helper runs at the end of `upgrade b`.
- G4: A failure gives an rc and a message in the existing classes: rc 1 (assertion) or rc 2
  (infrastructure).
- G5: Each run prints how long the helper waited, so a green run shows whether the race occurred.

**Non-goals**

- No `probeService` retry (D1).
- No change to the Helm chart. The IAM rollout strategy stays as it is.
- No change to the Playwright specs. The specs are correct when the cluster is settled.
- No new test harness for `run.sh`. None exists today (§6 gives the proof instead).

## 4. Design

### 4.1 The helper `settle_iam_service`

A new function in `ci/kind/run.sh`, next to `settle_gateway_404`. It has an entry line and two
parts.

**Entry: names and the measurement.**

1. `IAM_DEPLOY="$(chart_resource_name iam-backend)" || die_infra …`. It reuses the existing helper
   (`run.sh:111-122`), as the gateway check in `install_a` does (`run.sh:393-396`).
2. `k -n "$NS" get deployment "$IAM_DEPLOY" -o name`. A failure gives `die_infra`. This keeps a
   script fault or an API fault out of Part A's rc 1.
3. Print one line with the time (`date -u +%H:%M:%S`) and the IAM Deployment's
   `status.readyReplicas` (empty prints as 0). Example:
   `  iam settle: start 17:35:32 UTC, IAM readyReplicas=0`.
   This line records whether Helm returned before IAM was Ready (§2.4).

**Part A: the IAM pod is Ready.**

```bash
k -n "$NS" rollout status "deployment/$IAM_DEPLOY" --timeout=240s \
  || die_assert "the IAM backend Deployment did not become Ready in 240 s"
```

- Then print `  settled: IAM Ready after N s`, where N counts from the helper's start.
- The budget: the startup probe allows 5 s × 12 = 60 s. The readiness probe then answers 503 while
  IAM waits for the migration lock (`IAM_MIGRATION__LOCK_WAIT_SECS`, 120 s by default,
  `charts/paigasus/values.yaml:40`) and while the migration runs. Then one readiness period of
  10 s follows. That is about 190 s plus the migration time, so 240 s.
- The class is rc 1, the same as a failed `helm install --wait` in `install_a`: IAM did not
  become Ready, which is a fault of the chart or the product.

**Part B: the data path from each console pod.**

1. **List the console pods** with the selector
   `app.kubernetes.io/name in (gateway-console,iam-console),app.kubernetes.io/instance=$RELEASE`
   and `--field-selector=status.phase=Running`. Use `-o jsonpath` to print the pod name and its
   `app.kubernetes.io/name` label.
2. **Assert coverage, not only a non-empty list.** Each console Deployment of the release must
   have at least one listed pod. In phase A that is `iam-console` and `gateway-console`. In phase
   B it is `iam-console` only. The helper finds the expected set from the console Deployments
   that exist (`k get deployment -l …`), not from a fixed list. A console Deployment with no
   listed pod gives `die_infra`. The gateway console is the pod that failed in R2, so it must not
   be skipped.
3. **Poll each pod** with a 60 s deadline and `sleep 2`, in the shape of `settle_gateway_404`:
   - `rm -f "$STATE/settle-iam.err"` at the start of each iteration.
   - Run, with stdout captured into a variable and the capture written as `|| true`:
     `k -n "$NS" exec -c console "$pod" -- /nodejs/bin/node -e "$PROBE_JS" 2>"$STATE/settle-iam.err"`.
     Do not use `-i` or `-t`. The pod has one container, `console`
     (`console-deployment.yaml:45-46`). Then use `case` on the captured value. Do not pipe into an
     early-exit reader.
   - **Bound:** run the `k exec` in the background, start a watchdog
     `( sleep 20; kill "$pid" ) &`, run `wait "$pid"`, then kill the watchdog. Bash 3.2 has all of
     these. `run.sh` cannot use `timeout(1)`, because macOS has no `timeout`. A killed exec gives
     an empty result. The plan must write the `wait` so that `set -e` does not end the script on
     a non-zero status, and must prove that the watchdog kills a stalled exec.
   - **`PROBE_JS`** is CommonJS with a promise chain (no top-level `await`). It reads
     `process.env.PAIGASUS_SERVICES`, parses it as JSON, and takes the `iam` URL. It calls
     `fetch(iam + "/readyz", { signal: AbortSignal.timeout(5000) })`. It prints the HTTP status,
     cancels the response body, and calls `process.exit(0)`. On any thrown error (a missing
     variable, bad JSON, a fetch error) it prints `000` and calls `process.exit(0)`. So the script
     probes the exact URL that the console uses, and it holds no copy of the Service name or of
     port 8080. It always exits 0, so a fetch failure does not look like an exec failure.
   - A failed or killed `kubectl exec` gives an empty result. The loop treats it as `000` and tries
     again.
   - `200` ends the loop for that pod and prints
     `  settled: <pod> reaches IAM /readyz (HTTP 200) after N s`.
4. **At the deadline:**
   - Last result another HTTP status: `die_assert` (rc 1), "`<pod>` reaches IAM, but it answers
     HTTP `<code>`".
   - Last result `000` or empty: read the IAM Deployment's `readyReplicas` again. If it is 0, IAM
     became NotReady after Part A: `die_assert` (rc 1). Otherwise `die_infra` (rc 2), "`<pod>`
     cannot reach IAM", with the content of `settle-iam.err` in the message, as
     `settle_gateway_404` does with `settle-curl.err` (`run.sh:431,436`).

**Why `/readyz` and not `/v1/service-info`:** `/v1/service-info` needs a bearer token
(`rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:1114`), and a 401 proves only that a
socket opened. `/readyz` needs no token, and 200 also proves that migrations are complete. It uses
the same host and port as the discovery probe, so it tests the same Service and the same data
path.

**The gRPC port is not probed.** The console's IAM calls use gRPC on port 9090
(`console-env-configmap.yaml:11`). Both ports are in one EndpointSlice, and IAM opens both
listeners before the migration under one `BootSlot`
(`rs/crates/services/paigasus-iam/src/main.rs:125-142,231`). So an HTTP 200 on 8080 through the
Service is enough evidence for 9090.

**Shell rules (the `run.sh` header, lines 28-31):** the code must run on bash 3.2.57 and on bash 5.
It uses no `mapfile`, no associative array, no here-string and no here-doc. It never pipes into an
early-exit reader (actionlint check 13). Every wait has an explicit timeout.

**Why the pods must be the console pods:** the discovery probe runs inside the console process. A
check from the host or from a new pod does not test the network namespace that the probe uses.

### 4.2 Call sites

- `install_a`: after `helm install … --wait` and the gateway-console Deployment check, before
  `assert_notes_marker "install a"`.
- `upgrade_b`: after `settle_gateway_404`, before `assert_notes_marker "upgrade b"`. At this point
  settle part 1 has removed every old `iam-console` pod, so Part B checks only the new pods. On
  this upgrade IAM does not roll: its pod-template checksums come from `postgres.secretVersion`,
  `apiKeysSecretVersion` and the IdP CA, and `values/b.yaml` changes none of them. So Part A
  returns at once, and Part B costs a few seconds. It protects `specs b` if a later values change
  makes IAM roll.

### 4.3 Step and job timeouts (`.github/workflows/chart.yml`)

The workflow sizes each step timeout to the sum of the waits in `run.sh` for that mode, plus a
margin. The new waits add 4 min for Part A. Part B's worst case per pod is the 60 s deadline,
plus one last iteration (a 20 s exec bound and a 2 s sleep), which is about 82 s. The sums count
1.5 min per pod. Phase A has 2 console pods (`values/a.yaml` sets `replicas: 1` for each zone),
and phase B has 1.

| Item | Before | After | Sum |
|---|---|---|---|
| `install a` step | 12 | 19 | 10 (helm) + 4 (A) + 3 (B, 2 pods) + 2 margin |
| `upgrade b` step | 17 | 23 | 10 (helm) + 3 (settle 1) + 2 (gateway 404) + 4 (A) + 1.5 (B, 1 pod) + 2.5 margin |
| step sum | 158 | 171 | +7 +6 |
| job `timeout-minutes` | 169 | 182 | step sum + the same 11 min for the untimed steps |

The comments above both steps and above the job timeout get the new sums, in the existing style.

### 4.4 Documents and evidence

- `ci/kind/run.sh` header (lines 3-31): describe the new settle in the `install a` and
  `upgrade b` lines.
- `ci/kind/README.md`: the mode table (it must stay in sync with the header), and a short note.
  The note states:
  - the Helm 3.22.0 arithmetic from §2.1, so that nobody removes the wait as redundant with
    `--wait`;
  - that `kubectl wait --for=condition=Available` has the same defect, and why
    `rollout status` is correct;
  - that a Helm 4 bump (kstatus) can change this arithmetic;
  - assumption A1 and the local re-run hazard from §2.3, with its remedies.
- `diagnose` in `run.sh` gets `k -n "$NS" get endpointslices -o wide`. Today `get all -A` does not
  list EndpointSlices, and `diagnose` collects endpoints only for the stub (`run.sh:665-667`). An
  rc 2 "cannot reach" needs this evidence.

## 5. Failure classes

| Situation | rc | Message start |
|---|---|---|
| `chart_resource_name` fails, or the IAM Deployment is not found | 2 | `cannot find the IAM backend Deployment` |
| IAM rollout not done in 240 s | 1 | `the IAM backend Deployment did not become Ready` |
| A console Deployment has no Running pod to check | 2 | `no Running <name> pod to check the IAM data path from` |
| Last result another HTTP status at the 60 s deadline | 1 | `<pod> reaches IAM, but it answers HTTP <code>` |
| Last result `000` or empty, IAM `readyReplicas` 0 | 1 | `IAM became NotReady after the rollout` |
| Last result `000` or empty, IAM still Ready | 2 | `<pod> cannot reach IAM` + the exec stderr |

The workflow's existing `Collect evidence` step runs `run.sh diagnose` on any failure. The helper
adds no evidence collection of its own.

## 6. Verification

No test harness exists for `run.sh`. The proof uses static checks and real CI runs.

1. **Static:**
   - `bash -n ci/kind/run.sh` under `/bin/bash` 3.2.57 and under Homebrew bash 5.
   - `bash -n` accepts `mapfile` and `declare -A` under 3.2, so it does not check the 3.2 rule.
     Also grep the diff for `mapfile`, `declare -A`, `<<<` and `<<`. All four must be absent.
   - The shellcheck that the repo pins: `uv run --locked --project py shellcheck ci/kind/run.sh`.
     The result must show no new finding against `main`.
   - `repo:actionlint` for `chart.yml`.
2. **The watchdog (local):** prove that the watchdog bound kills a stalled command after 20 s and
   that the loop then continues. Use a stand-in for `k exec` (for example `sleep 60`) under both
   bash versions. No cluster is needed.
3. **The red proofs (CI).** They run on a throwaway branch `sma-701-red-proof`, made from the
   feature branch. They are never pushed to the PR branch, and the branch is deleted after the
   proofs. Each run is started with `gh workflow run chart.yml --ref sma-701-red-proof`, one after
   the other. Each stops at `install a`, so each takes about 8 minutes.
   - **R1, Part A and §2.1:** add `--set zones.iam.backend.image.tag=does-not-exist` to the
     `helm install` in `install_a`. The tag is not `latest`, so the kubelet uses `IfNotPresent`,
     the pull fails, and the IAM pod never becomes Ready. Console readiness does not depend on
     IAM (`console-deployment.yaml:93-96`), so the Helm wait does not depend on IAM either.
     Expected: `helm install --wait` exits 0, the entry line prints `readyReplicas=0`, and the
     step fails with rc 1 and the message `did not become Ready`. This measures §2.1 directly.
   - **R2, Part B rc 2:** `PROBE_JS` replaces the port `:8080` with `:8081` in the URL. Expected:
     rc 2, `cannot reach IAM`, after about 60 s of Part B.
   - **R3, Part B rc 1:** `PROBE_JS` uses the path `/nope` in place of `/readyz`. Expected: rc 1,
     `answers HTTP 404`.
4. **The green proof (CI):** with the final commit, run the job 5 times, one after the other.
   Start the next run only when the last one has finished. The concurrency group
   (`chart.yml:49-52`) keeps one pending run per group and cancels an older pending one, so
   parallel dispatches do not give 5 runs. One run takes about 13 minutes, so the 5 runs take
   about 70 minutes. All 5 must pass.
   - The PR body records, for each run: the `readyReplicas` value at the helper's start, the time
     of Part A, and the time of each pod in Part B.
   - It also records how many runs had `readyReplicas=0` when Helm returned. That count shows
     whether the race occurred in the runs.

**Limit:** the race occurred in 1 of a small number of runs. 5 green runs make the fix probable.
They do not prove it. The red proof R1 shows that Helm returns before IAM is Ready and that the
helper detects it.

## 7. Risks

- **R1: `kubectl exec` needs the console container to accept exec.** Distroless images allow
  exec of an absolute path. The image has `/nodejs/bin/node` (`ts/Dockerfile:81-82`). The first CI
  run proves it.
- **R2: the console pod runs as UID 65532.** `node -e` needs no write access, so this is not a
  problem.
- **R3: more wall time.** On a healthy run the helper costs about 1-3 s per console pod, plus
  Part A's wait when IAM is not Ready yet. The specs needed that wait anyway.
- **R4: Helm 4.** A Helm 4 bump uses kstatus and can make §2.1 false. The helper stays correct,
  but it can become redundant. The README note (§4.4) records this.
- **R5 (residual, out of scope): a cold JWKS fetch.** `/v1/service-info` needs a token, and IAM
  fetches the JWKS on the first token (`rs/crates/services/paigasus-iam/tests/http_authn.rs:375`).
  A `/readyz` probe does not start that fetch. If the first nav probe is the first token that IAM
  sees, a slow JWKS fetch can take more than the 1.5 s probe timeout. The result would be the
  reason `timeout` and the same disabled link. The failure in §1 had the reason `network`, so
  this is a different failure. It is not fixed here. If it occurs, it needs its own issue.

## 8. Rejected alternatives

- **An EndpointSlice wait** (`kubectl wait` for a ready address). It does not prove that the
  console pod can reach the Service.
- **`kubectl wait --for=condition=Available`.** It has the same `maxUnavailable` floor as Helm 3
  (§2.1).
- **A one-shot curl pod** (the shape of `manifests/stub-check.yaml`). It tests a new pod's network
  path, not the console pod's. It also needs a new manifest and pod scheduling time.
- **IAM `maxUnavailable: 0` in the chart.** The chart chose `maxSurge: 0, maxUnavailable: 1` on
  purpose, and a chart change is out of scope for a CI race.
- **A reload loop in `cross-zone.spec.ts`.** It hides a cluster that is not settled, and the
  specs must stay correct for a settled cluster.
- **A `probeService` retry.** Out of scope by D1.
- **`kubectl exec --request-timeout` as the exec bound.** It is not confirmed that it bounds an
  exec stream on kubectl 1.31 (WebSocket executor). The watchdog is a bound that the plan can
  prove locally.

## 9. Open questions

None. D1-D3 close the questions from the 2026-09-27 draft. §10 records how the challenger's
questions were answered.

## 10. Changes after the spec challenge (2026-10-02)

The challenger's verdict was APPROVE WITH CHANGES, with no BLOCKER. All findings were folded in.

| Finding | Change |
|---|---|
| MAJOR: the cache model in §2.2 was wrong (stale-while-revalidate, Redis, 600 s) | §2.2 step 3 corrected. New §2.3 with assumption A1, its evidence and the local re-run hazard. README note (§4.4). |
| MAJOR: Part B could skip the gateway console | §4.1 Part B step 2: a coverage assertion per console Deployment. The selector uses `$RELEASE`. |
| MAJOR: the Part A red proof could not prove §2.1 | §6 R1 now uses an image tag that does not exist. It measures Helm's early return directly. |
| MAJOR: green runs could not show that the helper waited | §4.1 entry line with `readyReplicas`, `settled:` lines with times, §6 step 4 records them. G5 added. |
| MAJOR: 5 dispatches conflict with the concurrency group | §6 step 4: sequential runs, about 70 minutes. |
| MINOR: no proven exec bound | §4.1: a watchdog bound. §6 step 2 proves it locally. §8 rejects `--request-timeout`. |
| MINOR: `PROBE_JS` details | §4.1: CommonJS, a promise chain, `process.exit(0)`, the body cancelled, `-c console`, no `-i`/`-t`, the URL from `PAIGASUS_SERVICES`. |
| MINOR: reuse `chart_resource_name`; check the name first | §4.1 entry steps 1-2. |
| MINOR: capture exec stderr; guard the capture | §4.1 Part B step 3. |
| MINOR: wrong reason for the 240 s budget | §4.1 Part A: derived from the startup probe, the 120 s lock wait, the migration and the readiness period. |
| MINOR: the sums omit the last iteration | §4.3: 1.5 min per pod. `install a` 19, `upgrade b` 23, job 182. |
| MINOR: the wrong class when IAM becomes NotReady after Part A | §4.1 Part B step 4 and §5: re-read `readyReplicas`. |
| MINOR: no red proof for the rc 1 branch of Part B | §6 R3. |
| MINOR: warn against `kubectl wait --for=condition=Available` | §2.1, §4.4, §8. |
| MINOR: weak static checks | §6 step 1: a grep for the four constructs and the pinned shellcheck. |
| MINOR: `diagnose` lacks EndpointSlices | §4.4. |
| MINOR: wrong citations | Fixed: `run.sh:28-31`, `ts/Dockerfile:81-82`, `playwright.config.ts:38`, `.prototools:12`, the full `primary-nav.tsx` path. |
| QUESTION: the gRPC port is not probed | Accepted. §4.1 states the reason. |
| QUESTION: a cold JWKS fetch | Out of scope. Recorded as R5. |
| QUESTION: Part A alone if the exec bound is not reliable | Not needed: the watchdog is the bound. |
| QUESTION: Helm source not read by the challenger | The coordinator read `ready.go` at `v3.22.0` through the GitHub API. R1 measures it too. |
