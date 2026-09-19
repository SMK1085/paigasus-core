# SMA-513 — Multi-zone ingress and Helm chart for the console zones

**Linear:** [SMA-513](https://linear.app/smaschek/issue/SMA-513/ops-multi-zone-ingress-and-helm-chart-for-the-console-zones)
**ADR:** ADR-0017 (console topology and session ownership)
**Design:** Frontend Architecture Scoping §§ 1, 2b
**Date:** 2026-09-19

---

## 1. Problem

Two console zones exist in code. `iam-console` compiles with `basePath: '/iam'` and
`gateway-console` with `basePath: '/gateway'`. Nothing deploys them. The repository holds no
`Chart.yaml`, no `charts/`, no `deploy/` and no `k8s/` directory. `ops/` holds only NATS
permissions and Prometheus and Grafana assets.

ADR-0017 makes the single origin load-bearing. One host-only cookie serves every zone, which is
why no Rust service needs CORS. The ADR states the consequence directly: *"Ingress path-routing
becomes part of the deployment contract. Helm must render the ingress rules and the console's zone
map from one values block so they cannot drift."*

This design delivers the images, the chart, the ingress and the controls that make the drift
impossible.

---

## 2. As-built findings (measured 2026-09-19)

These five findings change what the issue's own text says. Each one is read from the tree at
`7684b8f3`.

### F1 — There is no session secret

The issue's scope says *"Shared session secret and Redis connection wired into every console
pod."* No session secret exists. The cookie `__Host-pgs_sid`
(`ts/packages/paigasus-auth/src/http/cookies.ts:21`) carries an opaque server-generated id,
validated by a store lookup. It is neither signed nor encrypted. `authEnvShape`
(`ts/packages/paigasus-auth/src/config.ts:39-64`) holds no key of that shape.

What must reach every console pod instead is `PAIGASUS_SESSION_REDIS_URL` and
`PAIGASUS_OIDC_CLIENT_SECRET`. The second is shared because ADR-0017 decision 5 registers one
OIDC client with one redirect URI per zone.

### F2 — AC 2's coupling is three-way, not two-way

AC 2 names two halves: the ingress rule and the zone-map entry. There is a third. The `absent`
navigation state is decided from `PAIGASUS_SERVICES` alone, with no cache read and no probe
(`ts/packages/paigasus-discovery/src/server.ts:121-124`). `PAIGASUS_ZONES` decides which zones the
`<ZoneLink>` map can reach; `PAIGASUS_SERVICES` decides which services the navigation renders at
all.

So one values block must render three projections together, not two: the ingress path rule, the
`PAIGASUS_ZONES` entry and the `PAIGASUS_SERVICES` entry.

### F3 — AC 3's `assetPrefix` premise conflicts with `@paigasus/next-config`

AC 3 says static assets must not collide, *"(`basePath` + `assetPrefix` verified against a real
build)"*. `createNextConfig` accepts `assetPrefix` but deliberately does not default it to
`basePath` (`ts/packages/paigasus-next-config/src/index.ts:34,47-51,88`). It is a CDN-offload axis
on purpose, and neither console app passes it.

What prevents collision is `basePath` alone: Next emits `/iam/_next/…` and `/gateway/_next/…`. AC 3
is therefore satisfied without `assetPrefix`, and the real-build check must assert the emitted
prefix rather than the presence of an `assetPrefix` setting.

### F4 — The runtime already fails closed on a mis-rendered zone map

`assertCompiledAgreement()` (`ts/packages/paigasus-next-config/src/runtime.ts:174-202`)
cross-checks the image's compiled `PAIGASUS_COMPILED_ZONE` and `PAIGASUS_COMPILED_BASE_PATH`
against the deployed `PAIGASUS_ZONE` and `PAIGASUS_ZONES`, and throws on any disagreement. A chart
that renders a wrong zone map therefore fails at first request. It does not navigate wrongly in
silence.

This is an asset, not a substitute for a gate: it catches a wrong map at run time, on a live pod.
The gate in § 7 catches it at render time, in CI.

### F5 — The console image build needs no Rust, no napi and no wasm

Neither console app depends on `@paigasus/kernel`. Their `@paigasus/*` dependency sets are
`app-shell`, `auth`, `console-core`, `discovery`, `next-config`, `sdk` and `ui`;
`@paigasus/console-core` adds `proto`, and `@paigasus/proto`'s generated output is committed.

So SMA-634 — `@paigasus/kernel` cannot load its napi binding inside a Next build — does not touch
the console image. The image build is a pure TypeScript build.

### F6 — The two-zone shape is already proven in-process

`ts/apps/gateway-console/tests/e2e/support/two-zone-harness.ts` starts a `redis:8-alpine`
container, both apps' real standalone servers, and one TLS terminator that path-routes `/iam/*`
and `/gateway/*` to them, sharing one `PAIGASUS_ZONES` map and one Redis session store. It covers
rows R8–R12 and R21.

That is the same shape the ingress must reproduce in Kubernetes. `iam-console`'s README already
says so: *"The e2e tier runs one zone. A two-zone test belongs to SMA-513."*

---

## 3. Decisions

| # | Topic | Decision |
| -- | -- | -- |
| D1 | Scope | All four pieces — images, chart, ingress, cluster proof — in SMA-513, started after PR 270 (SMA-658) merges |
| D2 | Chart boundary | One chart deploying the first-party stack: both consoles, `paigasus-iam`, `paigasus-gateway`, and the ingress. Postgres, Redis and the OIDC issuer are required values naming existing endpoints. The chart ships no database |
| D3 | Console base image | `gcr.io/distroless/nodejs24-debian12:nonroot`, digest-pinned |
| D4 | AC verification | A required static gate for AC 2, 3 and 5; a non-required kind job for AC 1 and 4 |
| D5 | `reconcile_starter` | The chart keeps `replicas: 1` and `maxSurge: 0` for IAM. The concurrency question is filed separately rather than asserted here |

---

## 4. Delivery shape

Three pull requests on `feature/sma-513-ops-multi-zone-ingress-and-helm-chart`.

| PR | Content |
| -- | -- |
| 1 | `ts/Dockerfile`, console build and smoke in `ci/images/run.sh`, `images.yml` path filter, `docs/ops/RUNBOOK-containers.md` § 6 rewritten from prescriptive to as-built |
| 2 | `charts/paigasus/`, the `repo:helm-render` gate, its seven registration obligations |
| 3 | `.github/workflows/chart.yml`, `docs/ops/RUNBOOK-chart.md`, CLAUDE.md gotchas |

**Ordering constraint.** PR 270 rewrites `ci/images/run.sh`, `.github/workflows/release.yml`,
`ci/actionlint/release_guard.py` and `docs/ops/RUNBOOK-containers.md`. PR 1 touches the first and
the last. Work starts after PR 270 merges, so PR 1 extends the shape that landed rather than a
shape that changed under it.

---

## 5. Console images

### 5.1 Base

`gcr.io/distroless/nodejs24-debian12:nonroot`, pinned by digest
`sha256:14d42e2511532589a7c7e01a753667a74fcc96266e137e8125006b87b0c32d0a` (resolved 2026-09-19;
the manifest list carries linux/amd64 and linux/arm64, plus s390x and ppc64le which this repository
does not build).

It satisfies `docs/ops/RUNBOOK-containers.md` § 6 on every count: no shell, a numeric non-root
`USER`, and a digest pin that Dependabot's `docker` ecosystem updater already covers. Its `nonroot`
user is uid 65532, the same identity `rs/Dockerfile` uses, so one `securityContext` covers all four
images.

Node 24 matches the repository's pins: `.prototools` sets `node = "24.16.0"` and every console
`package.json` sets `"engines": { "node": ">=24" }`.

### 5.2 One parameterized Dockerfile

`ts/Dockerfile`, selected by `--build-arg APP=iam-console|gateway-console`, mirroring
`rs/Dockerfile`'s `BIN` argument.

**Exec-form `ENTRYPOINT` and `HEALTHCHECK` do not expand `ARG` or `ENV`.** `rs/Dockerfile` solves
this by installing both binaries to the fixed path `/usr/local/bin/paigasus-service`. The console
image solves it the same way: the builder stage, which has a shell, writes two fixed-path files.

- `/app/entrypoint.js` — `require('./apps/<APP>/server.js')`, with `<APP>` substituted at build
  time. Relative resolution is from `/app`, which is where the standalone tree's root sits.
- `/app/healthcheck.mjs` — probes `http://127.0.0.1:${PORT}<basePath>/healthz` and exits non-zero
  on anything but 200, with `<basePath>` substituted at build time.

The instructions are then constant:

```dockerfile
ENTRYPOINT ["/nodejs/bin/node", "/app/entrypoint.js"]
HEALTHCHECK --interval=30s --timeout=3s --start-period=30s --retries=3 \
  CMD ["/nodejs/bin/node", "/app/healthcheck.mjs"]
```

The probe path is under the base path, because that is where the route lives:
`ts/apps/iam-console/app/healthz/route.ts` serves `GET /iam/healthz`. That route runs the full
configuration parse, so a 200 proves the environment is valid, not merely that a process is alive.

There is no `/readyz` route in either console. The chart therefore uses `/healthz` for both the
readiness and the liveness probe, and records that as a known limitation rather than inventing a
second route in this issue.

### 5.3 Build stage

The builder needs the `ts/` pnpm workspace. The console packages are source-only, so the app build
compiles their `src/` through `transpilePackages`.

`pnpm install --filter <app>...` installs only the app's own subgraph, which keeps
`@paigasus/kernel` and its `file:`-linked `@paigasus/node-bindings` out of the install. That
matters because `@paigasus/node-bindings` has no `.node` binary in a fresh tree.

**Implementation risk.** The filtered install is a claim to verify in the first task of PR 1, not a
measured fact. If pnpm still materializes the kernel packages, the fallback is a pruned build
context that omits `ts/packages/paigasus-kernel`.

### 5.4 New assertions in `ci/images/run.sh`

`assert_pins()` already cross-checks `rs/Dockerfile` against `rs/rust-toolchain.toml`. The console
path gets the same treatment:

1. The base image's Node major equals `.prototools`' `node` pin major. A distroless tag bump that
   crosses a major reds the gate rather than shipping a runtime the repository does not pin.
2. No `ENV PAIGASUS_*` is baked into the image. This is the console analogue of the existing
   `ENV IAM_*|GATEWAY_*` assertion, and it enforces decision F8 of the design document — no
   build-time configuration anywhere.
3. The final stage copies only the standalone tree and the two generated fixed-path files.

The smoke suite gains a console case: the container runs, `docker top -o pid,uid` reports 65532,
the image has no shell, and `GET <basePath>/healthz` answers 200 with the expected zone.

---

## 6. The chart

`charts/paigasus/`. One chart, no subcharts.

### 6.1 The single values block

```yaml
zones:
  iam:
    enabled: true
    basePath: /iam
    image:
      repository: ghcr.io/smk1085/paigasus-iam-console
      tag: ""            # defaults to .Chart.AppVersion
    service:             # the backend this zone fronts
      http: http://paigasus-iam:8080
      grpc: http://paigasus-iam:50051
  gateway:
    enabled: false
    basePath: /gateway
    image:
      repository: ghcr.io/smk1085/paigasus-gateway-console
      tag: ""
    service:
      http: http://paigasus-gateway:8080
```

Four projections derive from that map and from nothing else:

| Projection | Derivation |
| -- | -- |
| `Ingress.spec.rules[0].http.paths` | one entry per enabled zone, `path: <basePath>`, `pathType: Prefix` |
| `PAIGASUS_ZONES` | `{zoneId: basePath}` over enabled zones |
| `PAIGASUS_SERVICES` | `{zoneId: service.http}` over enabled zones |
| Console `Deployment` set | one per enabled zone |

### 6.2 Shared ConfigMap

`PAIGASUS_ZONES` and `PAIGASUS_SERVICES` live in one ConfigMap that both console pods read with
`envFrom`. They are byte-identical across zones by construction rather than by template
discipline. This is what F4's runtime assertion needs in order to stay quiet.

Both console Deployments carry a `checksum/config` annotation over that ConfigMap. A zone-map
change therefore rolls both zones, which is correct because the map changed for both. An image tag
sits in its own pod spec, so bumping one zone's tag rolls only that zone. § 7 check 3 proves it.

### 6.3 Required environment

Ten keys have no default and must reach every console pod.

| Key | Source | Per zone or shared |
| -- | -- | -- |
| `PAIGASUS_ZONE` | the zone's own id | per zone |
| `PAIGASUS_ZONES` | shared ConfigMap | shared |
| `PAIGASUS_SERVICES` | shared ConfigMap | shared |
| `PAIGASUS_IAM_GRPC_URL` | `zones.iam.service.grpc` | shared |
| `PAIGASUS_PUBLIC_ORIGIN` | `https://{{ .Values.ingress.host }}` | shared |
| `PAIGASUS_OIDC_ISSUER` | `oidc.issuer` | shared |
| `PAIGASUS_OIDC_CLIENT_ID` | `oidc.clientId` | shared |
| `PAIGASUS_OIDC_CLIENT_SECRET` | Secret | shared |
| `PAIGASUS_SESSION_STORE` | the constant `redis` | shared |
| `PAIGASUS_SESSION_REDIS_URL` | Secret | shared |

`PAIGASUS_PUBLIC_ORIGIN` is validated as https (`authEnvShape`, `httpsUrl`). The ingress therefore
has to terminate TLS. AC 4 needs that independently, because `__Host-pgs_sid` requires `Secure`.

`PAIGASUS_SESSION_STORE` is forced to `redis` rather than exposed. `createAuthRuntime` refuses
`memory` once `PAIGASUS_ZONES` names more than one zone
(`ts/packages/paigasus-auth/src/runtime.ts:89-113`), and `@paigasus/console-core`'s descriptor
cache needs Redis regardless. A values knob whose only other setting crashes the pod is not a
knob.

### 6.4 Ingress

One `networking.k8s.io/v1` `Ingress`: one host, one TLS secret, one path rule per enabled zone,
`pathType: Prefix`, and **no rewrite annotation**.

Each app serves its full path already, because `basePath` is compiled in. A rewrite that strips
`/iam` breaks every route. That is the change an operator is most likely to add by reflex, so the
template carries a comment saying so, and `values.yaml`'s `ingress.annotations` documents it.

`/iam/auth/*` and `/gateway/auth/*` are in-app route handlers
(`ts/apps/*/app/auth/[...auth]/route.ts`) and fall under their zone's own prefix. They need no rule
of their own. This matches the ADR's own diagram, where `/auth/*` is *"handled IN-APP by both"*.

One host is what keeps the cookie host-only and readable by every zone, per ADR-0017 decision 2.

### 6.5 Values combinations the chart refuses

Two combinations cannot work, and the chart fails at template time with a named message rather
than installing something that crash-loops.

| Enabled zones | Verdict |
| -- | -- |
| none | `fail` — a chart with no zone serves nothing |
| `gateway` only | `fail` — `gateway-console`'s `servicesWithIamAndGateway` refuses to construct without an `iam` entry (`ts/apps/gateway-console/lib/config.ts:52-63`) |
| `iam` only | valid |
| `iam` and `gateway` | valid |

### 6.6 Service deployments and the SMA-559 handoff

The IAM Deployment carries, per `docs/ops/RUNBOOK-containers.md` § 5 and § 7:

- `replicas: 1` and `strategy.rollingUpdate.maxSurge: 0` as defaults.
- `IAM_MIGRATION__LOCK_WAIT_SECS` exposed as a value, default 120.
- `startupProbe` sized for configuration load plus `Database::connect`, **not** for the migration.
  SMA-571 removed that coupling. A migrating replica binds its sockets and answers `/healthz` 200
  within a second of process start.
- `readinessProbe` on `/readyz`, sized for a database blip in steady state. A replica that has
  never been ready is simply absent from the endpoint list for as long as the migration takes, and
  the threshold neither extends nor shortens that.
- `securityContext` with `runAsUser: 65532`, `runAsGroup: 65532`, `runAsNonRoot: true`, and
  **no** `readOnlyRootFilesystem`. § 7 of the runbook states that posture is untested.
- `terminationGracePeriodSeconds` above Kubernetes' 30s default, documented as a floor to widen
  from rather than a measured value.

The gateway Deployment keeps `readinessProbe.periodSeconds` at 30s or above, because its `/readyz`
issues a real gRPC introspect call to IAM on every poll and sits inside its metrics layer.

**The `reconcile_starter` question is not answered here.** The runbook asks SMA-513 to confirm that
`AppState::new`'s `reconcile_starter` is safe under a surging rollout. It writes system policies
and roles on every boot with no advisory lock and has never been tested under concurrency. That is
a question about Rust code and needs a concurrency test, not a chart. The chart keeps the
conservative default, and the question is filed as its own issue. Claiming safety we have not
measured would be worse than leaving `maxSurge` at 0.

---

## 7. `repo:helm-render` — the static gate

`ci/helm-render/run.sh`, in the repository's standard three-mode shape: `--self-test`,
`--negative-control`, then the real run, invoked under an explicit `set -euo pipefail`.

### 7.1 Checks

1. **Coupling (AC 2).** For each of the four enabled-subsets in § 6.5, render the chart and assert
   that the ingress path-prefix set, the `PAIGASUS_ZONES` key set and the `PAIGASUS_SERVICES`
   console-zone key set are equal. The two invalid subsets must fail, and must fail with their own
   message rather than with a template error from somewhere else.
2. **Asset isolation (AC 3), against a real build.** Walk both apps' `.next/static` trees and
   assert every emitted chunk path sits under its own zone's base path, and that the two sets are
   disjoint. This follows `ci/tailwind-source/run.mjs`'s precedent of walking `.next/static`
   rather than reading a manifest, because Next 16.3.4 writes no `app-build-manifest.json`.
3. **Independent rollout (AC 5).** Render twice, changing only `zones.iam.image.tag`, and assert
   the `gateway-console` Deployment's rendered bytes are unchanged while the `iam-console`
   Deployment's are not. Then render twice changing only a zone's `enabled` flag, and assert both
   Deployments change — because the shared ConfigMap's checksum moved, which is correct.
4. **Security-context pin.** Every first-party Deployment sets 65532/65532 and `runAsNonRoot`, and
   no Deployment sets `readOnlyRootFilesystem`.
5. **`helm lint` plus golden output.** `helm template` for each valid subset, compared to committed
   golden files.

### 7.2 Registration — seven obligations

A new `repo:*` gate carries seven obligations. Missing one does not always red
`repo:affected-smoke`, as the `repo:ruff-ci` entry in CLAUDE.md records, so each is listed
explicitly:

1. `ci.yml`'s `T=(…)` array.
2. The marker-delimited command in CLAUDE.md.
3. `SELF_SCHEDULED_GATES` in `ci/affected-graph/ci_targets.py` — the four `moon.yml` invocation
   lines, `set -euo pipefail` included.
4. `SELF_TASK_EXPECTED_GLOBS` — the task's literal `inputs`.
5. A script pin, `HELM_RENDER_SH_CALL_SITES`, holding the flag parse, the `NEGATIVE` guard, the
   assertion body and both report arms as discrete lines. Pinning the span as one block leaves the
   two measured bypasses the `release-parity` entry documents.
6. `REQUIRED_REPO_TASKS`, because this gate carries a `--negative-control`.
7. `repo:affected-smoke`'s own `inputs` listing `ci/helm-render/**/*`, floored by a
   `T_AFFECTED_SMOKE_REQUIRED_INPUTS` entry in `ci/actionlint/run.sh`. Without this last one, a PR
   editing `ci/helm-render/**` does not schedule `repo:affected-smoke` at all, so none of the other
   six pins can fire on exactly the PR that breaks them.

`EXPECTED_FINDING_KEYS` in `ci_targets.py` needs re-baselining if this work adds a check to
`collect_findings`. That is a deliberate act, not a mechanical edit to clear a red.

### 7.3 Affectedness

Check 2 reads built `.next` trees, so the task declares `deps` on `iam-console-ts:build` and
`gateway-console-ts:build`. `deps` schedules a build; it does not select this task. Only `inputs`
confer affectedness on Moon 2.5.3, so the task's `inputs` must also list `charts/**/*`,
`ci/helm-render/**/*` and both apps' sources.

**Residual, inherited from the tailwind-source gate.** A Moon cache hit hydrates `.next` without
running the build's staging step, so check 2 can in principle satisfy itself from a stale chunk.
This is the limitation `ci/tailwind-source/README.md` already records, not a new one.

---

## 8. `chart.yml` — the cluster job

A new workflow, with the same posture as `images.yml`: it is **not** a required check, so a broken
chart reds `main` rather than the pull request.

Triggers: `workflow_dispatch`; `push` to `main` on `charts/**`, `ts/apps/**`, `ci/helm-render/**`
and itself; `pull_request` on the same narrower set.

Steps:

1. Bring up kind and ingress-nginx.
2. Build all four images and `kind load` them.
3. Start Postgres, Redis and Keycloak in the cluster. Keycloak is the OIDC issuer because
   `@paigasus/auth` already has an integration test against a containerized Keycloak, so the realm
   fixture exists.
4. Generate a self-signed certificate for the ingress host and install it as the TLS secret.
5. `helm install` with both zones enabled.
6. Run the existing two-zone Playwright specs against the ingress origin.

Step 6 is what makes AC 1 and AC 4 cheap. `two-zone-harness.ts` already proves cross-zone
navigation and a shared session against a TLS terminator that path-routes to two standalone
servers over one Redis. Pointing those specs at a real ingress re-hosts a passing suite instead of
writing a new one.

**Implementation risk.** That harness owns its own process startup, and it lives in the gateway
zone's test tree. Making it accept an external origin means changing a file that another tier's
tests depend on. If the change proves invasive, the fallback is a small separate spec covering the
same two rows — a login at one zone honored at the other, and one cross-zone navigation — rather
than reshaping the harness.

**A second risk worth naming:** `wheels.yml`'s first CI run failed on a maturin flag that looked
correct locally. A kind job has more moving parts than that. Expect PR 3 to need more than one CI
round, and treat the first green as the measurement rather than the plan as the proof.

---

## 9. Documentation

- `docs/ops/RUNBOOK-chart.md` — new. Values reference, the two refused combinations, the
  no-rewrite rule, and the upgrade order for a release that changes the zone map.
- `docs/ops/RUNBOOK-containers.md` § 6 — rewritten from prescriptive to as-built, naming the
  distroless base, the fixed-path entrypoint and the reason for it.
- `CLAUDE.md` — gotchas worth the space: exec-form `ENTRYPOINT` cannot expand the app name, the
  ingress must carry no rewrite annotation, and the three-way projection AC 2 rests on.

---

## 10. Out of scope

- Publishing the chart to an OCI registry. That is adjacent to SMA-658 and belongs with it.
- Routing the bare origin root `/`. A visitor to `https://<host>/` gets the ingress controller's
  404 today. Recorded as a gap, with a follow-up issue.
- `readOnlyRootFilesystem`. Untested for all four images, per RUNBOOK § 7.
- `NetworkPolicy`, `HorizontalPodAutoscaler` and `PodDisruptionBudget`.
- Gateway API. Plain `Ingress` is controller-agnostic and needs no CRDs.
- Bundled Postgres, Redis or identity provider.
- A `/readyz` route for the consoles.
- The `reconcile_starter` concurrency question (§ 6.6), filed separately.

---

## 11. Residual risks

1. **Nothing gates a third console zone.** Adding one means touching the chart, `values.yaml`, the
   golden files and the gate's subset matrix. The matrix is written over the enabled-subsets of
   whatever `zones` holds, so it grows on its own, but no control asserts that a new
   `ts/apps/*` directory reaches the chart at all. This is the same residual shape the
   `TAILWIND_GUARD_INVOCATIONS` registry exists to close for its own gate.
2. **The golden files are a strict-equality pin.** Any legitimate chart change re-baselines them.
   That is intended, and it must stay a deliberate act.
3. **Check 2 can be satisfied by a stale `.next`** on a Moon cache hit (§ 7.3).
4. **The kind job is not a required check**, so a chart regression reaches `main` before anyone
   sees it. This is the same trade `images.yml` already makes, taken knowingly.
5. **The distroless digest is a second pin** beside `.prototools`. Assertion 1 in § 5.4 holds the
   Node major equal. It does not hold the minor equal, and it cannot — distroless publishes no
   patch-level tags.

---

## 12. Open questions

None blocking. Two worth a sentence at review time:

- Whether `docs/ops/RUNBOOK-chart.md` should carry the values reference or whether `values.yaml`'s
  own comments suffice. The repository's habit is a runbook, so the spec assumes one.
- Whether the gateway Deployment belongs in this chart at all, given that `gateway-console` is the
  only console that needs it and SMA-635 has not yet given the gateway an interactive-user auth
  path. Decision D2 includes it; the alternative is an address in values, as for Postgres.
