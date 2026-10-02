# SMA-701: Wait for the IAM Service after the kind chart install

- **Linear:** [SMA-701](https://linear.app/smaschek/issue/SMA-701)
- **Branch:** `feature/sma-701-kind-iam-settle`
- **Related:** SMA-514 (the kind job, the summed step timeouts), PR 316 (the failing run)
- **Status:** DRAFT. Sven approved the design in chat on 2026-10-02 (§0). The spec-challenger has
  not reviewed it yet. An earlier draft from 2026-09-27 is lost; only its Linear comment remains.
  This spec replaces it and agrees with that comment.

## 0. Decisions (Sven, 2026-10-02)

- **D1: no `probeService` retry in SMA-701.** The issue lists it as optional. SMA-701 changes only
  the kind job and its documents. A retry changes product behaviour on every page, so it is out of
  scope.
- **D2: the wait runs in `install a` AND in `upgrade b`.** It is one helper with two call sites.
- **D3: approach 1.** First `kubectl rollout status` for the IAM Deployment, then a poll from each
  console pod to the IAM Service through `kubectl exec` and `node`. The alternatives (an
  EndpointSlice wait, or a one-shot curl pod) are rejected in §8.

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

`.prototools:11` pins `helm = "3.22.0"`. In Helm 3.22.0, `pkg/kube/ready.go:301-302` counts a
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

### 2.2 How one failed probe disables the link for the whole test

1. The gateway console resolves IAM from `PAIGASUS_SERVICES`. The chart builds the URL
   `http://paigasus-paigasus-iam-backend:8080` (`charts/paigasus/templates/_helpers.tpl:169`).
2. `probeService` (`ts/packages/paigasus-discovery/src/probe.ts:80-119`) sends one
   `GET /v1/service-info` with a 1.5 s timeout. A thrown fetch error (connection refused, DNS)
   becomes the reason `network` (`src/core/reasons.ts:22-29`).
3. A failed record stays fresh for 10 s (`negativeMs`, `src/core/record.ts:35-42`). The probe runs
   only at server render. No timer and no startup probe exist.
4. `navStateOf` turns the failure into `degraded`. `primary-nav.tsx:96-98` renders a `span` with
   `aria-disabled="true"` and no `href`.
5. The spec renders the page once after login and then clicks at once
   (`cross-zone.spec.ts:12` onward). It does not reload. Playwright waits 15 s for the link to be
   enabled (`playwright.config.ts:33`), and `retries` is 0. Nothing renders the page again, so the
   link stays disabled until the timeout.

### 2.3 What is not proven

- The evidence agrees with §2.1, but no run recorded the IAM pod's Ready time. The first served
  request at 17:35:31.259 can be a kubelet probe or a request on the gRPC port, which do not need
  the Service endpoint. The IAM `readinessProbe` has `periodSeconds: 10` and no initial delay. A
  503 during migration at the first check delays Ready by 10 s. This fits the 17:35:36.151 failure.
- A second race is possible after the pod is Ready: kube-proxy can program the Service rules a
  short time after the endpoint appears. Part B of the helper (§4.1) covers this race too.

## 3. Goals and non-goals

**Goals**

- G1: The step `install a` returns only after IAM answers HTTP 200 on `/readyz` through its Service,
  as seen from each console pod.
- G2: The wait polls with a deadline, as `settle_gateway_404` (`ci/kind/run.sh:423-443`) does. It
  does not use a fixed sleep.
- G3: The same helper runs at the end of `upgrade b`.
- G4: A failure gives an rc and a message in the existing classes: rc 1 (assertion) or rc 2
  (infrastructure).

**Non-goals**

- No `probeService` retry (D1).
- No change to the Helm chart. The IAM rollout strategy stays as it is.
- No change to the Playwright specs. The specs are correct when the cluster is settled.
- No new test harness for `run.sh`. None exists today (§6 explains the proof instead).

## 4. Design

### 4.1 The helper `settle_iam_service`

A new function in `ci/kind/run.sh`, next to `settle_gateway_404`. It has two parts.

**Part A: the IAM pod is Ready.**

```bash
k -n "$NS" rollout status "deployment/$IAM_DEPLOY" --timeout=240s \
  || die_assert "the IAM backend Deployment did not become Ready in 240 s"
```

- `IAM_DEPLOY` is `${RELEASE}-paigasus-iam-backend`. A comment names the chart helper
  `paigasus.name` (`_helpers.tpl:34-40`) that builds the name.
- The budget: the startup probe allows 5 s × 12 = 60 s. After that, `/readyz` answers 503 while
  migrations run, and the readiness probe checks every 10 s. 240 s is 4 times the startup window.
  The passing run on `main` needed about 12.5 s for the whole install.
- The class is rc 1, the same as a failed `helm install --wait` in `install_a`: IAM did not
  become Ready, which is a fault of the chart or the product.

**Part B: the data path from each console pod.**

1. List the console pods once:
   `k -n "$NS" get pods -l 'app.kubernetes.io/name in (gateway-console,iam-console),app.kubernetes.io/instance=paigasus' --field-selector=status.phase=Running -o jsonpath=…`.
   An empty list gives `die_infra`. In phase B the gateway zone is off, so the list holds only
   `iam-console` pods. That is correct.
2. For each pod, poll with a 60 s deadline and `sleep 2`, in the shape of `settle_gateway_404`:
   - `rm -f` the per-iteration state file first.
   - Run `k -n "$NS" exec "$pod" -- /nodejs/bin/node -e "$PROBE_JS" "$IAM_READYZ"` and capture its
     stdout. Then use `case` on the captured value. Do not pipe into an early-exit reader.
   - `PROBE_JS` calls `fetch(url, { signal: AbortSignal.timeout(5000) })`. It prints the HTTP
     status, or `000` on any thrown error. It always exits 0, so a fetch failure does not look
     like an exec failure.
   - `IAM_READYZ` is `http://${IAM_DEPLOY}:8080/readyz`. Port 8080 is the chart default
     `zones.iam.backend.httpPort`. The kind values do not change it. A comment states this.
   - A failed `kubectl exec` gives an empty result. The loop treats it as `000` and tries again.
   - `200` ends the loop for that pod and prints `  settled: <pod> reaches IAM (HTTP 200)`.
3. At the deadline, the class follows `settle_gateway_404`: an empty result or `000` gives
   `die_infra` (rc 2); any other status gives `die_assert` (rc 1). The message names the pod, the
   URL and the last status.

**Why `/readyz` and not `/v1/service-info`:** `/v1/service-info` needs a bearer token, and a 401
proves only that a socket opened. `/readyz` needs no token, and 200 also proves that migrations
are complete. It uses the same host and port as the discovery probe, so it tests the same Service
and the same data path.

**Bounds on each poll:** the node fetch aborts after 5 s. `kubectl exec` also gets
`--request-timeout=20s`. The plan must measure whether that flag bounds a stalled exec stream. If
it does not, the plan must name a different bound. `run.sh` cannot use `timeout(1)`, because macOS
has no `timeout`.

**Shell rules (the `run.sh` header, lines 20-25):** the code must run on bash 3.2.57 and on bash 5.
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
margin. The new waits add 4 min (Part A) and 1 min (Part B) to both modes. Part B's 60 s is per
pod, so with N console pods the worst case is N minutes. Phase A has 2 console pods
(`values/a.yaml` sets `replicas: 1` for each zone), and phase B has 1.

| Item | Before | After | Sum |
|---|---|---|---|
| `install a` step | 12 | 18 | 10 (helm) + 4 (A) + 2 (B, 2 pods) + 2 margin |
| `upgrade b` step | 17 | 22 | 10 (helm) + 3 (settle 1) + 2 (gateway 404) + 4 (A) + 1 (B, 1 pod) + 2 margin |
| step sum | 158 | 169 | +6 +5 |
| job `timeout-minutes` | 169 | 180 | step sum + the same 11 min for the untimed steps |

The comments above both steps and above the job timeout get the new sums, in the existing style.

### 4.4 Documents

- `ci/kind/run.sh` header (lines 3-25): describe the new settle in the `install a` and
  `upgrade b` lines.
- `ci/kind/README.md`: the mode table (it must stay in sync with the header), and a short note
  with the Helm 3.22.0 arithmetic from §2.1. The note exists so that nobody removes the wait as
  redundant with `--wait`. It also says that a Helm 4 bump (kstatus) changes this arithmetic.

## 5. Failure classes

| Situation | rc | Message start |
|---|---|---|
| IAM rollout not done in 240 s | 1 | `the IAM backend Deployment did not become Ready` |
| No Running console pod | 2 | `no Running console pod to check the IAM data path from` |
| Last result `000` or empty at the 60 s deadline | 2 | `<pod> cannot reach <url>` |
| Last result another HTTP status at the deadline | 1 | `<pod> reaches <url>, but it answers HTTP <code>` |

The workflow's existing `Collect evidence` step runs `run.sh diagnose` on any failure. The new
helper adds no evidence collection of its own.

## 6. Verification

No test harness exists for `run.sh`. The proof uses static checks and real CI runs.

1. **Static:** `bash -n ci/kind/run.sh` under `/bin/bash` 3.2.57 and under Homebrew bash 5.
   `shellcheck ci/kind/run.sh` if it is installed. `repo:actionlint` for `chart.yml`.
2. **Red proof for Part B:** a temporary commit sets the port in `IAM_READYZ` to 8081. Start the
   job with `gh workflow run chart.yml --ref feature/sma-701-kind-iam-settle`. The step
   `install a` must fail with rc 2 and the message `cannot reach`, after about 60 s of Part B.
   Revert the commit.
3. **Red proof for Part A:** a temporary commit runs
   `k -n "$NS" scale deployment/"$IAM_DEPLOY" --replicas=0` just before Part A, and changes Part A's
   timeout to 30 s for this run only. `install a` must fail with rc 1 and the rollout message.
   Note: `rollout status` on a Deployment with 0 replicas can report success at once. If it does,
   the red proof must use a different mutation (for example, an image tag that does not exist,
   set by `kubectl set image`). The plan decides after one measurement.
4. **Green proof:** with the final commit, start the job 5 times. All 5 runs must pass. Record
   for each run the time that Part A and Part B used, from the `settled:` lines, in the PR body.
5. **The two red-proof commits must not reach `main`.** The PR history keeps them as reverted
   commits, or the branch drops them before the PR opens. The plan decides which.

**Limit:** the race occurred in 1 of a small number of runs. 5 green runs make the fix probable.
They do not prove it. The Part A red proof shows that the helper detects the cause from §2.1.

## 7. Risks

- **R1: `kubectl exec` needs the console container to accept exec.** Distroless images allow
  exec of an absolute path. The image has `/nodejs/bin/node` (`ts/Dockerfile:74`). The first CI
  run proves it. If a console pod has more than one container, the exec needs `-c`. The plan
  checks the pod spec.
- **R2: the console pod runs as UID 65532.** `node -e` needs no write access, so this is not a
  problem.
- **R3: more wall time.** On a healthy run the helper costs about 1-3 s per console pod, plus
  Part A's wait when IAM is not Ready yet. That wait is time the specs needed anyway.
- **R4: Helm 4.** A Helm 4 bump uses kstatus and can make §2.1 false. The helper stays correct,
  but it can become redundant. The README note (§4.4) records this.

## 8. Rejected alternatives

- **An EndpointSlice wait** (`kubectl wait` for a ready address). It does not prove that the
  console pod can reach the Service.
- **A one-shot curl pod** (the shape of `manifests/stub-check.yaml`). It tests a new pod's network
  path, not the console pod's. It also needs a new manifest and pod scheduling time.
- **IAM `maxUnavailable: 0` in the chart.** The chart chose `maxSurge: 0, maxUnavailable: 1` on
  purpose, and a chart change is out of scope for a CI race.
- **A reload loop in `cross-zone.spec.ts`.** It hides a cluster that is not settled, and the
  specs must stay correct for a settled cluster.
- **A `probeService` retry.** Out of scope by D1.

## 9. Open questions

None. D1-D3 close the questions from the 2026-09-27 draft.
