# SMA-513 — Multi-zone ingress and Helm chart for the console zones

**Linear:** [SMA-513](https://linear.app/smaschek/issue/SMA-513/ops-multi-zone-ingress-and-helm-chart-for-the-console-zones)
**ADR:** ADR-0017 (console topology and session ownership)
**Design:** Frontend Architecture Scoping §§ 1, 2b
**Date:** 2026-09-19
**Revision:** 2, after the adversarial challenge. § 13 records what changed and why.

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

Eight findings, read from the tree at `7684b8f3`. Six amend the issue's own text. Two (F7, F8)
were found by the adversarial challenge and invalidate parts of revision 1.

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

So one values block must render three projections together, not two.

### F3 — AC 3's `assetPrefix` premise conflicts with `@paigasus/next-config`

AC 3 says *"(`basePath` + `assetPrefix` verified against a real build)"*. `createNextConfig`
accepts `assetPrefix` but deliberately does not default it to `basePath`
(`ts/packages/paigasus-next-config/src/index.ts:34,47-51,88`). It is a CDN-offload axis on
purpose, and neither console app passes it.

What prevents collision is `basePath` alone: Next serves chunks at `/iam/_next/static/…` and
`/gateway/_next/static/…`. AC 3 is therefore satisfied without `assetPrefix`.

### F4 — The runtime fails closed on a mis-rendered zone map, but only for the pod's own entry

`assertCompiledAgreement()` (`ts/packages/paigasus-next-config/src/runtime.ts:174-202`)
cross-checks the image's compiled `PAIGASUS_COMPILED_ZONE` and `PAIGASUS_COMPILED_BASE_PATH`
against the deployed `PAIGASUS_ZONE` and `PAIGASUS_ZONES`, and throws on disagreement.

**It checks only the pod's own zone entry** (`runtime.ts:190-201`). A wrong entry for a *different*
zone does not throw. So the runtime is an asset, not a substitute for a gate: it catches a wrong
self-entry at run time on a live pod, and nothing else.

### F5 — The console image build needs no Rust, no napi and no wasm

Neither console app depends on `@paigasus/kernel`. Their `@paigasus/*` dependency sets are
`app-shell`, `auth`, `console-core`, `discovery`, `next-config`, `sdk` and `ui`;
`@paigasus/console-core` adds `proto` as a **devDependency**
(`ts/packages/paigasus-console-core/package.json:35`) while importing it from `src/`, and `next`
and `react` are peer dependencies there.

So SMA-634 does not touch the console image, and the build is pure TypeScript. The devDependency
placement means the builder stage needs a **dev-inclusive** install; a `--prod` install breaks it.

### F6 — The two-zone shape is proven in-process, but against fakes

`ts/apps/gateway-console/tests/e2e/support/two-zone-harness.ts` starts a `redis:8-alpine`
container, both apps' real standalone servers, and one TLS terminator that path-routes `/iam/*`
and `/gateway/*`, sharing one `PAIGASUS_ZONES` map and one Redis session store.

It also starts a **fake IAM**, a **fake IdP** and a **counting forwarder**
(`two-zone-harness.ts:34,88-95,118`), and its specs script those fakes and assert on
`iamConsole.connections()`. None of that survives a move to a real ingress with a real IAM and a
real Keycloak. § 9 plans accordingly.

### F7 — Next's standalone output contains no static assets (challenge finding)

**This invalidates revision 1's console image outright.** The standalone tree has no
`.next/static` and no `public/`. What supplies them is the Moon **`build` task's own `script:`**
(`ts/apps/iam-console/moon.yml:80-99`), which copies both into the standalone tree and then
asserts the staged `BUILD_ID` matches. The app's `package.json` `build` script is a bare
`"next build"` (`ts/apps/iam-console/package.json:11`) and stages nothing.

A Dockerfile that runs `next build` or `pnpm build` therefore produces an image that returns 404
for every JavaScript and CSS chunk. The page never hydrates, AC 1 and AC 3 both fail — and a
smoke check on `<basePath>/healthz` still passes, because that route is a handler needing no
client bundle. Revision 1's § 5.4 assertion 3 would have forbidden the fix.

### F8 — `PAIGASUS_SERVICES` keys are service slugs from a closed registry (challenge finding)

`parseServiceMap` validates every key against `SERVICE_SLUGS` and **throws** on an unknown one
(`ts/packages/paigasus-discovery/src/config.ts:31-36`), with the registry derived from the proto
capability registry (`ts/packages/paigasus-discovery/src/core/state.ts:21`). Zone id and service
slug coincide today for exactly two values, by accident.

Revision 1's residual claim — that the gate's subset matrix "grows on its own" for a third zone —
was therefore false. A third zone whose id is not a registered slug crashes **both** consoles at
first request, not just the new one.

### F9 — Measured service ports (challenge finding)

Revision 1 named two wrong ports. Measured: IAM's gRPC default is **9090**
(`rs/crates/services/paigasus-iam/src/config.rs:813`) and the gateway's HTTP default is **8088**
(`rs/crates/services/paigasus-gateway/src/config.rs:186`). `docs/ops/RUNBOOK-containers.md:76-79`
already carries the correct table.

A wrong gRPC address fails every session introspect. A wrong gateway HTTP address makes the
discovery probe fail, so the gateway renders **`degraded`** forever — exactly the state AC 2
requires be distinguishable from `absent`. The chart would look correct and the AC would be
silently broken.

---

## 3. Decisions

| # | Topic | Decision |
| -- | -- | -- |
| D1 | Scope | All four pieces in SMA-513, started after PR 270 (SMA-658) merges |
| D2 | Chart boundary | **Amended 2026-09-20.** One chart deploying both consoles, the `paigasus-iam` backend and the ingress. `paigasus-gateway` is **not** deployed — it is supplied through `zones.gateway.backend.url`, for the reasons in § 7.3. Postgres, Redis and the OIDC issuer are required values naming existing endpoints |
| D3 | Console base image | `gcr.io/distroless/nodejs24-debian12:nonroot`, digest-pinned |
| D4 | AC verification | A required static gate for AC 2 and 5; a container smoke assertion for AC 3; a non-required kind job for AC 1 and 4 |
| D5 | `reconcile_starter` | The chart keeps `replicas: 1` and `maxSurge: 0` for IAM. The concurrency question is filed separately, not asserted here |
| D6 | Zone atomicity | A zone's `enabled` flag governs its console, its ingress rule and both environment entries **together**. "Routable but unadvertised" is not expressible. See § 7.3 |
| D9 | Where a backend runs | Amended 2026-09-20. `backend.deploy` is a **separate axis** from `enabled`: the chart deploys the IAM backend and takes every other zone's backend as an address. It cannot affect AC 2. See § 7.3 |
| D7 | Zone id domain | A zone id must be a member of `SERVICE_SLUGS`. The chart fails at render time otherwise (F8) |
| D8 | `helm` | Proto-pinned like every other CLI gate, with `[ -x ]` asserted and rc 2 on absence |

---

## 4. Delivery shape

Four pull requests on `feature/sma-513-ops-multi-zone-ingress-and-helm-chart`.

| PR | Content |
| -- | -- |
| 1 | `ts/Dockerfile`, console build and smoke in `ci/images/run.sh`, `images.yml` path filter, a Dependabot `docker` block for `/ts`, RUNBOOK § 6 rewritten as-built |
| 2a | `charts/paigasus/`, the `helm` proto pin, `helm lint`, and the committed golden files |
| 2b | The `repo:helm-render` coupling and rollout checks, the negative-control fixtures, and the eight registration obligations |
| 3 | `.github/workflows/chart.yml` (kind), `docs/ops/RUNBOOK-chart.md`, CLAUDE.md gotchas |

**Why 2a and 2b are separate.** If the chart and its gate land together, the golden files are
born in the same commit that writes the chart, so no reviewer ever sees a golden *change*. Splitting
them means 2b's first act is to prove its checks fire against 2a's already-committed chart.

**Ordering constraint.** PR 270 rewrites `ci/images/run.sh`, `.github/workflows/release.yml`,
`ci/actionlint/release_guard.py` and `docs/ops/RUNBOOK-containers.md`. PR 1 touches the first and
the last. Work starts after PR 270 merges. If it slips, PR 2a does not depend on it and can go
first.

---

## 5. Console images

### 5.1 Base

`gcr.io/distroless/nodejs24-debian12:nonroot`, pinned by digest
`sha256:14d42e2511532589a7c7e01a753667a74fcc96266e137e8125006b87b0c32d0a`.

**Verified against the registry on 2026-09-19:** the tag exists and that digest is a **manifest
list**, carrying linux/amd64 and linux/arm64 (plus s390x and ppc64le, which this repository does
not build). It is not a per-platform digest, so the arm64 leg is safe.

It satisfies `docs/ops/RUNBOOK-containers.md` § 6 on every count: no shell, a numeric non-root
`USER`, and a digest pin. Its `nonroot` user is uid 65532, the same identity `rs/Dockerfile` uses,
so one `securityContext` covers all four images.

Node 24 matches the repository's pins: `.prototools` sets `node = "24.16.0"` and every console
`package.json` sets `"engines": { "node": ">=24" }`.

**Dependabot does not cover it today.** `.github/dependabot.yml:117-118` scopes the `docker`
ecosystem to `directory: /rs`. PR 1 adds a `/ts` block, with `ignore` entries mirroring lines
142-152, because § 5.5 assertion 1 couples the Node major to `.prototools` — so an automated major
bump is a red Dependabot cannot fix alone. That is the same multi-site argument the existing
comment at lines 113-116 makes for `rust` and `ubuntu`.

### 5.2 One parameterized Dockerfile

`ts/Dockerfile`, selected by `--build-arg APP=iam-console|gateway-console`, mirroring
`rs/Dockerfile`'s `BIN` argument.

**Exec-form `ENTRYPOINT` and `HEALTHCHECK` do not expand `ARG` or `ENV`.** `rs/Dockerfile` solves
this with a fixed binary path. The console image solves it the same way: the builder stage, which
has a shell, writes two fixed-path files.

Both are **`.mjs`**, and the entrypoint uses a dynamic import, not `require`. Every console
`package.json` sets `"type": "module"` (`ts/apps/iam-console/package.json:5`), Next reads that
flag to choose its template, and copies the same `package.json` into the standalone tree. A CJS
`require()` shim would depend on Node's `require(esm)` interop, and a `"type": "module"` manifest
landing at `/app` would make the shim itself parse as ESM, where `require` is undefined.

- `/app/entrypoint.mjs` — `await import('./apps/<APP>/server.js')`, `<APP>` substituted at build
  time.
- `/app/healthcheck.mjs` — probes `http://127.0.0.1:${PORT}<basePath>/healthz`, `<basePath>`
  substituted at build time.

```dockerfile
ENTRYPOINT ["/nodejs/bin/node", "/app/entrypoint.mjs"]
HEALTHCHECK --interval=30s --timeout=3s --start-period=30s --retries=3 \
  CMD ["/nodejs/bin/node", "/app/healthcheck.mjs"]
```

Next's standalone server calls `process.chdir(__dirname)` on load. That is harmless to the shim,
because both CJS and ESM specifiers resolve relative to the importing file rather than to the
working directory. It does mean the working directory becomes `/app/apps/<APP>` for everything
after load. Stated here so nobody re-litigates it.

`PORT` defaults to 3000 and `HOSTNAME` to `0.0.0.0`. The chart sets `PORT` explicitly and must
leave `HOSTNAME` unset or `0.0.0.0`, because the healthcheck probes the loopback address.

### 5.3 Build stage, and the staging the standalone tree does not do

Per F7, `next build` alone is not enough. The builder stage must reproduce what
`ts/apps/<app>/moon.yml`'s `build` script does:

```
pnpm install --frozen-lockfile --filter <app>...   # dev-inclusive; see F5
pnpm exec next build
assert .next/standalone/apps/<app>/server.js exists
cp -R .next/static  -> .next/standalone/apps/<app>/.next/static
cp -R public        -> .next/standalone/apps/<app>/public   (if present)
assert the staged BUILD_ID equals .next/BUILD_ID
```

`--frozen-lockfile` is required, not optional. An unlocked install inside a Docker build can
resolve versions the committed `ts/pnpm-lock.yaml` does not name, and the published image would
then not be built from the committed lockfile with nothing saying so. That is the same defect
class `repo:affected-smoke`'s A8 exists to prevent for cargo.

`pnpm install --filter <app>...` installs only the app's subgraph, which keeps `@paigasus/kernel`
and its `file:`-linked `@paigasus/node-bindings` out of the install. **This is a claim to verify in
the first task of PR 1, not a measured fact.** If pnpm still materializes the kernel packages, the
fallback is a pruned build context omitting `ts/packages/paigasus-kernel`.

The runtime stage installs nothing. Next traces what it needs into the standalone tree.

**This creates a second staging site, and SMA-655 centralised it deliberately.** § 5.5 assertion 4
is what keeps the two honest.

### 5.4 The smoke assertion that would have caught F7

A text allowlist over `COPY` instructions is what failed in revision 1. The smoke suite asserts
behaviour instead, against the running container:

1. `GET <basePath>/` returns 200.
2. Extract a `<basePath>/_next/static/…` URL from that HTML.
3. `GET` that URL and require **200 with a non-empty body**.
4. `GET` the other zone's prefix for the same asset and require **404**.

Step 3 fails on a missing `.next/static`, which is exactly what revision 1 shipped. Steps 2 and 4
together are AC 3's real-build proof: the emitted asset URL carries the zone's own base path, and
the other zone's prefix does not serve it. This replaces revision 1's § 7.1 check 2, which could
not work — on disk the tree is `.next/static/chunks/*.js`, and the base path is a URL prefix that
never appears in a filename.

### 5.5 New assertions in `ci/images/run.sh`

`assert_pins()` already cross-checks `rs/Dockerfile` against `rs/rust-toolchain.toml`. The console
path gets the same treatment:

1. The base image's Node major equals `.prototools`' `node` pin major.
2. No `ENV PAIGASUS_*` is baked into the image — the console analogue of the existing
   `ENV IAM_*|GATEWAY_*` assertion. The rule is **no deployment-varying configuration**, not "no
   build-time configuration": `createNextConfig` deliberately bakes `PAIGASUS_COMPILED_ZONE` and
   `PAIGASUS_COMPILED_BASE_PATH` (`next-config/src/index.ts:91-94`), and § 5.2's healthcheck bakes
   the base path.
3. `--frozen-lockfile` is present on the builder's install line.
4. **The image's staged tree matches a host build's.** Compare the file set under
   `/app/apps/<APP>/.next/static` in the image against the host's
   `.next/standalone/apps/<app>/.next/static`, which `moon.yml` produced. This is a behavioural
   comparison, not a text match, so it survives either site being reworded.
5. The container runs as uid 65532 and the image has no shell.

---

## 6. `helm`, and why it is pinned

§ 7 shells out to `helm template` and `helm lint`. `.prototools` has no `helm` entry today.

Every other CLI gate in this repository is proto-pinned, and CLAUDE.md records the reason for
shellcheck in the same words that apply here: there is deliberately no fallback to whatever binary
a host happens to have. `helm template` output varies with the binary — default
`.Capabilities.KubeVersion`, YAML emission details, the `helm lint` rule set — so an unpinned helm
makes the golden files red at random on a different host or runner.

PR 2a therefore adds a `helm` pin to `.prototools` and a `.proto/plugins/helm.toml`. The gate
resolves it with `PROTO_REPORTER=text` per the NDJSON rule, asserts `[ -x ]` on the result, and
exits **2** (infrastructure) rather than 1 when it is absent. Every `helm template` call passes
`--kube-version` explicitly, so the goldens do not move with the binary's default.

---

## 7. The chart

`charts/paigasus/`. One chart, no subcharts.

### 7.1 The single values block

```yaml
zones:
  iam:                 # the id MUST be a member of SERVICE_SLUGS (D7, F8)
    enabled: true
    basePath: /iam
    console:
      image: { repository: …, tag: "" }
    backend:
      image: { repository: …, tag: "" }
      http: { port: 8080 }
      grpc: { port: 9090 }        # measured default, F9
  gateway:
    enabled: false
    basePath: /gateway
    console:
      image: { repository: …, tag: "" }
    backend:
      image: { repository: …, tag: "" }
      http: { port: 8088 }        # measured default, F9
```

Six projections derive from that map and from nothing else:

| Projection | Derivation |
| -- | -- |
| `Ingress.spec.rules[0].http.paths` | one entry per enabled zone, `path: <basePath>`, `pathType: Prefix` |
| `PAIGASUS_ZONES` | `{id: basePath}` over enabled zones |
| `PAIGASUS_SERVICES` | `{id: <backend http in-cluster URL>}` over enabled zones |
| `PAIGASUS_IAM_GRPC_URL` | the `iam` zone's backend gRPC URL |
| Console `Deployment` set | one per enabled zone |
| Backend `Deployment` and `Service` set | one per enabled zone **whose `backend.deploy` is true** (D9) |

Only the first four are AC-2 projections. The backend row keys on a second axis, and a zone whose
backend is supplied rather than deployed is still fully routed and fully advertised.

### 7.2 Port pinning

`values.yaml`'s default ports must equal the two Rust services' compiled defaults. A service-side
port change that the chart does not follow produces a permanently `degraded` gateway tile, which
F9 explains is the failure AC 2 exists to prevent. The gate therefore asserts the four default
ports in `values.yaml` against the literals in `rs/crates/services/paigasus-{iam,gateway}/src/config.rs`.

### 7.3 Zone atomicity (D6), and where the backend runs (D9)

**Amended 2026-09-20**, after implementation found that the gateway backend cannot boot from this
chart at all: `GatewayConfig::validate` hard-fails on an empty `upstream.openai.api_key`
(`rs/crates/services/paigasus-gateway/src/config.rs:278`) and `iam.grpc_addr` accepts
`LoopbackInsecure` only for a loopback host (`:209-222`), so an in-cluster gateway→IAM link needs
a TLS design that is out of scope here.

**D6, unchanged in substance.** A zone's `enabled` flag governs its console, its ingress rule, its
`PAIGASUS_ZONES` entry and its `PAIGASUS_SERVICES` entry, together. That is what makes "routable
but unadvertised" unrepresentable rather than merely checked, and it is the strictest reading of
AC 2. The price is that **"the gateway zone is advertised but not routed", or the reverse, cannot
be expressed** — which is the point.

**D9 is a separate axis.** `zones.<id>.backend.deploy` decides whether the chart runs that zone's
backend or takes its address from `backend.url`. It cannot produce a half-present zone, because
every AC-2 projection keys on `enabled` alone. Today `iam` deploys and every other zone supplies;
`zones.iam.backend.deploy=false` is refused outright, because `PAIGASUS_IAM_GRPC_URL` would
otherwise point at a Service the chart does not render.

Revision 1's claim that a zone's `enabled` flag also governs its backend was therefore too strong.
It was written before anyone had read the gateway's own configuration validator.

### 7.4 ConfigMaps, Secrets and what rolls what

Three objects, and the spec names all of them because revision 1 left five keys unmodelled:

| Object | Holds | Checksum annotation on |
| -- | -- | -- |
| `<release>-zonemap` ConfigMap | `PAIGASUS_ZONES`, `PAIGASUS_SERVICES` | both console Deployments |
| `<release>-console-env` ConfigMap | `PAIGASUS_IAM_GRPC_URL`, `PAIGASUS_PUBLIC_ORIGIN`, `PAIGASUS_OIDC_ISSUER`, `PAIGASUS_OIDC_CLIENT_ID`, `PAIGASUS_SESSION_STORE` | both console Deployments |
| `<release>-console-secret` Secret | `PAIGASUS_OIDC_CLIENT_SECRET`, `PAIGASUS_SESSION_REDIS_URL` | both console Deployments (`checksum/secret`) |

The Secret needs its own annotation for a concrete reason: with `envFrom` and no checksum,
**rotating `PAIGASUS_SESSION_REDIS_URL` or the OIDC client secret restarts nothing**, and every pod
keeps the old value indefinitely.

**Corrected 2026-09-20, after the local review.** The first implementation annotated
`sha256sum` of `.Values.oidc.existingSecret` — the Secret's *name*, which does not change when its
contents rotate. The annotation therefore could never fire, and this section described a control
that did not exist. The chart does not own that Secret and `lookup` returns empty under
`helm template`, so hashing its contents is not available. The knob is now an explicit
`oidc.secretVersion` the operator bumps on rotation. That is weaker than hashing real contents and
the chart says so: it depends on the operator doing something, where the ConfigMap checksums
depend on nothing.

All three are shared across zones, so a change to any of them rolls both consoles. That is correct
— the value changed for both — and § 8.1 check 3 accounts for it.

`PAIGASUS_ZONE` is per zone and sits in the pod spec.

### 7.5 Required environment

Ten keys have no default. `PAIGASUS_PUBLIC_ORIGIN` is validated as https (`authEnvShape`,
`httpsUrl`), so the ingress must terminate TLS — which AC 4 needs anyway, because `__Host-pgs_sid`
requires `Secure`.

`PAIGASUS_SESSION_STORE` is forced to `redis` rather than exposed. `createAuthRuntime` refuses
`memory` once `PAIGASUS_ZONES` names more than one zone
(`ts/packages/paigasus-auth/src/runtime.ts:89-113`), and `@paigasus/console-core`'s descriptor
cache needs Redis regardless. A knob whose only other setting crashes the pod is not a knob.

### 7.6 Ingress

One `networking.k8s.io/v1` `Ingress`: one host, one TLS secret, one path rule per enabled zone,
`pathType: Prefix`, and **no rewrite annotation**.

Each app serves its full path already, because `basePath` is compiled in. A rewrite that strips
`/iam` breaks every route. That is the change an operator is most likely to add by reflex, so the
template carries a comment saying so.

`/iam/auth/*` and `/gateway/auth/*` are in-app route handlers and fall under their zone's own
prefix, so they need no rule of their own. One host is what keeps the cookie host-only and
readable by every zone, per ADR-0017 decision 2.

Keycloak must have **two** redirect URIs registered — `<origin>/iam/auth/callback` and
`<origin>/gateway/auth/callback` — derived at `ts/packages/paigasus-auth/src/runtime.ts:129`.

### 7.7 Values combinations the chart refuses

Validation lives in one `paigasus.validate` include, called at the top of every template, so the
error does not depend on Helm's template evaluation order.

| Condition | Verdict |
| -- | -- |
| no zone enabled | `fail` — a chart with no zone serves nothing |
| `gateway` enabled, `iam` disabled | `fail` — `gateway-console`'s `servicesWithIamAndGateway` refuses to construct without an `iam` entry (`ts/apps/gateway-console/lib/config.ts:52-63`) |
| a zone id outside `SERVICE_SLUGS` | `fail` — `parseServiceMap` would throw in **both** consoles at first request (F8) |
| `iam` only | valid |
| `iam` and `gateway` | valid |

### 7.8 Probes

**Liveness is a `tcpSocket` check on the console's port; readiness is `httpGet <basePath>/healthz`.**

The consoles have no `/readyz`, and `/healthz` runs the full configuration parse
(`ts/apps/iam-console/app/healthz/route.ts:3-5`) with failure deliberately not memoized
(`next-config/src/runtime.ts:244-248`). Using it for liveness would put a misconfigured pod into
CrashLoopBackOff instead of leaving it running and NotReady with a readable log — and
`docs/ops/RUNBOOK-containers.md:90` states that liveness must never touch a dependency. A TCP
check satisfies both constraints without inventing a route in this issue.

**Known gap:** `getRuntimeConfig()` parses environment only and never connects, so a console whose
Redis is down still reports Ready while every session read fails. A real `/readyz` is the fix and
is out of scope here (§ 11).

### 7.9 Service deployments and the SMA-559 handoff

The IAM Deployment carries `replicas: 1`, `strategy.rollingUpdate.maxSurge: 0`,
`IAM_MIGRATION__LOCK_WAIT_SECS` as a value defaulting to 120, a `startupProbe` sized for
configuration load plus `Database::connect` (SMA-571 removed the migration coupling), a
`readinessProbe` on `/readyz` sized for a steady-state database blip, `securityContext` with
65532/65532 and `runAsNonRoot`, **no** `readOnlyRootFilesystem`, and a
`terminationGracePeriodSeconds` above the 30s default documented as a floor to widen from.

**`maxSurge: 0` is a deliberate deviation, not the runbook's instruction.**
`docs/ops/RUNBOOK-containers.md:169-171` says the opposite — it need no longer be pinned to 0. D5
keeps 0 because the runbook's own precondition is unmet: `AppState::new`'s `reconcile_starter`
writes system policies and roles on every boot with no advisory lock and has never been tested
under concurrency. That is a question about Rust code needing a concurrency test, not a chart
default. It is filed as its own issue. Claiming safety we have not measured would be worse than
keeping 0.

The gateway Deployment keeps `readinessProbe.periodSeconds` at 30s or above, because its `/readyz`
issues a real gRPC introspect call to IAM on every poll and sits inside its metrics layer.

---

## 8. `repo:helm-render` — the static gate

`ci/helm-render/run.sh`, in the repository's standard three-mode shape: `--self-test`,
`--negative-control`, then the real run, under an explicit `set -euo pipefail`.

### 8.1 Checks

1. **Coupling (AC 2).** For each valid subset, the ingress path-prefix set, the `PAIGASUS_ZONES`
   key set and the `PAIGASUS_SERVICES` key set are equal, and every key is a member of
   `SERVICE_SLUGS`.
2. **A disabled zone leaves no trace.** A disabled zone's id appears in **no** rendered
   `PAIGASUS_SERVICES` value, in any ConfigMap or Deployment `env`, in any subset. This is a
   separate row from check 1 because check 1 compares sets that all derive from one `range` and so
   cannot catch a value written outside it.
3. **Independent rollout (AC 5).** Diff **`spec.template` only**, not whole-manifest bytes — a
   Deployment-level label or annotation change is not a restart, and diffing whole bytes gives a
   false red for it. Three cases: bumping one zone's explicit console image tag changes only that
   zone's template; bumping `.Chart.AppVersion` with both tags defaulted changes **both**, which
   is a documented consequence and not a failure; disabling a zone removes its Deployment entirely
   and changes the surviving zone's template, because the shared checksums moved.
4. **Port pin (§ 7.2)** and **security-context pin** — 65532/65532, `runAsNonRoot`, and no
   `readOnlyRootFilesystem` on any Deployment.
5. **`helm lint` plus golden output**, with `--kube-version` pinned.

AC 3 is **not** in this gate. It is § 5.4's container assertion, because the base path is a URL
prefix and does not exist on disk.

### 8.2 The negative control, named concretely

Revision 1 named `--negative-control` without saying what it mutates. That is the shape CLAUDE.md
calls "a control that actively lies rather than one that merely no-ops". Four fixture charts under
`ci/helm-render/fixtures/`, each of which must exit 1 with its own named row:

| Fixture | Mutation | Must be caught by |
| -- | -- | -- |
| `literal-ingress` | ingress rules written literally instead of ranged over `zones` | check 1 |
| `zones-omits-enabled` | `PAIGASUS_ZONES` omits an enabled zone | check 1 |
| `orphan-service` | `PAIGASUS_SERVICES` carries a disabled zone's key | check 2 |
| `template-only-diff` | both zones share one pod template fragment | check 3 |

Which mutation was measured against which check is recorded in `ci/helm-render/README.md`, as
`ci/release-parity/README.md` does for its own residual.

### 8.3 Registration — eight obligations

1. `ci.yml`'s `T=(…)` array.
2. The marker-delimited command in CLAUDE.md.
3. `SELF_SCHEDULED_GATES` — the four `moon.yml` invocation lines, `set -euo pipefail` included.
4. `SELF_TASK_EXPECTED_GLOBS` — the task's literal `inputs`.
5. A script pin, `HELM_RENDER_SH_CALL_SITES`, holding the flag parse, the `NEGATIVE` guard, the
   assertion body and both report arms as discrete lines.
6. `REQUIRED_REPO_TASKS`, because this gate carries a `--negative-control`.
7. `repo:affected-smoke`'s own `inputs` listing `ci/helm-render/**/*`, floored by
   `T_AFFECTED_SMOKE_REQUIRED_INPUTS`. **Reason corrected:** revision 1 said reachability would
   otherwise fail. It would not — `moon.yml:219` already declares a broad `ci/**/*`. The narrow
   glob is kept for convention and for check 8e's length floor, exactly as the comment at
   `moon.yml:245` explains for `ci/next-public/**/*`.
8. **`EXPECTED_PR_SUBJECTS` in `ci/workflow-credentials/workflow_credentials.py:284-291`**, for PR
   3. `chart.yml` carries a `pull_request` trigger, and that tuple is a strict-equality pin of six
   filenames. A seventh reds `repo:workflow-credentials` on arrival. `chart.yml` must also declare
   no `secrets:` and no `id-token: write`.

**CLAUDE.md is stale here.** It says `EXPECTED_PR_SUBJECTS` pins "five subject filenames"; the code
holds six. PR 3 corrects it.

`EXPECTED_FINDING_KEYS` in `ci_targets.py` needs re-baselining if this work adds a check to
`collect_findings`. That is a deliberate act, not a mechanical edit to clear a red.

### 8.4 Affectedness, and the gate's own hygiene

Check 3 and the goldens need no build. Nothing in this gate reads a `.next` tree any more — § 5.4
moved that to the container smoke — so the gate does not depend on either app's `build`, and its
`inputs` are `charts/**/*`, `ci/helm-render/**/*`, `values.yaml`'s schema and the two Rust
`config.rs` files the port pin reads.

`ci/helm-render/run.sh` must not write into any build tree, and nothing in the repository would
catch it if it did — SMA-655's read-only scan covers only `tests/e2e/**` and
`playwright.config.ts`. The rule is stated in the gate's README.

Two shell constraints, so the gate does not join the "no single local bash works" split:
`ci/actionlint/run.sh` check 13 bans piping into an early-exit reader in every tracked `*.sh`, and
`mapfile` or `declare -A` make a gate unusable under the development Mac's system
`/bin/bash` 3.2.57.

Chart template files must avoid Windows reserved device names — no `con.yaml`, `prn.yaml`,
`aux.yaml` or `nul.yaml`. The Linux-only gates pass and only a Windows matrix job catches it.

---

## 9. `chart.yml` — the cluster job

Not a required check, same posture as `images.yml`: a broken chart reds `main`, not the pull
request. Triggers: `workflow_dispatch`; `push` to `main` on `charts/**`, `ts/apps/**`,
`ci/helm-render/**` and itself; `pull_request` on the same set.

Steps: kind plus ingress-nginx; build all four images and `kind load` them; Postgres, Redis and
Keycloak in-cluster with both redirect URIs registered; a self-signed certificate as the TLS
secret; `helm install`; then the specs below.

**Revision 1 claimed this re-hosts the existing two-zone Playwright suite. It does not** (F6). That
suite scripts a fake IAM and a fake IdP and asserts on a counting forwarder, none of which exists
behind a real ingress. Revision 1 named that as a risk with a fallback; **the fallback is now the
plan**, and the existing fake-based tier stays where it is.

Three small specs against the ingress origin:

| # | Asserts | AC |
| -- | -- | -- |
| R1 | a login at one zone is honored at the other | 4 |
| R2 | one cross-zone navigation is a hard navigation and lands | 1 |
| R3 | with `zones.gateway.enabled: false`, the IAM console's nav item for the gateway is **absent from the DOM**, not merely disabled | 2 |

R3 exists because AC 2's user-visible half — `primary-nav.tsx:74`'s
`if (entry.state.state === 'absent') continue;` — was asserted by nobody in revision 1. The static
gate checks the rendered environment; only R3 checks what an operator sees. R3 needs a second
`helm install` with a different values file, so the job runs two installs.

Two known hazards from memory: Keycloak two-tab e2e flakes (SMA-652), and `form-action 'self'`
blocking a cross-origin redirect in Chromium (SMA-653).

`images.yml` already carries a "Reclaim runner disk" step for the Rust build. This job adds two
Next production builds and two more images, so it needs the same step.

**Expect more than one CI round.** `wheels.yml`'s first run failed on a maturin flag that looked
correct locally. A kind job has more moving parts. The first green is the measurement; the plan is
not the proof.

---

## 10. Documentation

- `docs/ops/RUNBOOK-chart.md` — new. Values reference, the refused combinations, the no-rewrite
  rule, the upgrade order for a zone-map change, and D6's limitation.
- `docs/ops/RUNBOOK-containers.md` § 6 — rewritten as-built.
- `CLAUDE.md` — the staging rule (F7), exec-form `ENTRYPOINT` cannot expand the app name, the
  ingress carries no rewrite annotation, the three-way projection, and the
  `EXPECTED_PR_SUBJECTS` count correction.

---

## 11. Out of scope

- **Image publishing, and what that costs AC 1.** `docs/ops/RUNBOOK-containers.md:50-53` records
  that `ci/images/run.sh` pushes nothing and `images.yml` carries no registry credentials. So AC 1
  — "a single `helm install` brings up both zones" — **is demonstrated only under `kind load`**. A
  real install into someone else's cluster needs a follow-up issue. Stated plainly rather than
  implied.
- Publishing the chart to an OCI registry.
- Routing the bare origin root `/`. A visitor gets the ingress controller's 404.
- A console `/readyz`, and with it the Redis-down readiness gap in § 7.8.
- An external IAM backend. `zones.iam.backend.deploy=false` is refused (D9, § 7.3), because
  `PAIGASUS_IAM_GRPC_URL` would point at a Service the chart does not render.
- Deploying the gateway backend from this chart at all (D9, § 7.3), and with it the in-cluster
  gateway→IAM TLS design that would be its prerequisite.
- `readOnlyRootFilesystem`; `NetworkPolicy`, `HorizontalPodAutoscaler`, `PodDisruptionBudget`;
  Gateway API; bundled Postgres, Redis or identity provider.
- The `reconcile_starter` concurrency question (§ 7.9), filed separately.

---

## 12. Residual risks

1. **Nothing gates a third console zone.** A new `ts/apps/*` directory does not have to reach the
   chart, and no control says so. D7's `SERVICE_SLUGS` check limits the blast radius — an unusable
   id fails at render rather than at first request — but it does not force a new app to be added.
2. **The golden files are a strict-equality pin.** Any legitimate chart change re-baselines them.
   That is intended and must stay a deliberate act.
3. **The kind job is not required**, so a chart regression reaches `main` before anyone sees it.
   The same trade `images.yml` already makes, taken knowingly.
4. **The distroless digest is a second pin** beside `.prototools`. § 5.5 assertion 1 holds the Node
   major equal. It cannot hold the minor equal, because distroless publishes no patch-level tags.
5. **§ 5.3's filtered install is unverified** and is task 1 of PR 1.
6. **The Next version measurement is stale.** `ts/pnpm-workspace.yaml:106` pins `next: ^16.3.5`,
   while the "no `app-build-manifest.json`" measurement in `iam-console/moon.yml`'s comment was
   taken on 16.3.4. Nothing in this design depends on it any more — § 5.4 replaced the manifest
   question with an HTTP assertion — but the comment should be re-measured when someone touches it.

---

## 13. What the adversarial challenge changed

Verdict on revision 1: **NEEDS REWORK**, seven blockers. Every blocker I spot-checked against the
tree was confirmed. Folded in:

| Finding | Change |
| -- | -- |
| Console image ships no static assets | F7; § 5.3 staging; § 5.4 replaces the text allowlist with an HTTP assertion; § 5.5 assertion 4 |
| AC 3 check cannot work on disk paths | § 5.4 steps 2–4 replace revision 1's `.next/static` walk |
| AC 2 check is tautological | § 8.1 check 2 added; § 8.2 names four fixtures |
| Service without a zone passes | D6 removes the configuration; § 7.3 |
| `helm` unpinned | D8; § 6 |
| Wrong ports (50051, 8080) | F9; § 7.1 and § 7.2's pin |
| `PAIGASUS_SERVICES` key domain | F8; D7; § 7.7 |
| `require()` shim on an ESM module | § 5.2 uses `.mjs` and a dynamic import |
| Dependabot does not cover `/ts` | § 5.1 |
| `EXPECTED_PR_SUBJECTS` missing | § 8.3 obligation 8 |
| Obligation 7's reason false | § 8.3 obligation 7, corrected |
| AC 5 diffs the wrong thing | § 8.1 check 3 diffs `spec.template`, three cases |
| Secret and five env keys unmodelled | § 7.4 |
| § 8 step 6 not a re-host | F6; § 9 adopts the fallback as the plan |
| AC 2's DOM half uncovered | § 9 row R3 |
| Image publishing blocks AC 1 | § 11, stated plainly |
| `/healthz` as liveness | § 7.8 uses `tcpSocket` for liveness |
| `maxSurge` misattributed | § 7.9 presents it as a deviation |
| Plus the minors | five-vs-six findings count, the phantom "F8" citation, the tailwind precedent, port and `HOSTNAME`, validation placement, gate read-only rule, bash constraints, Windows device names, runner disk |

**Not folded in, with reasons:** nothing. Two findings were answered by measurement rather than by
edit — the distroless digest is a manifest list (verified against the registry, § 5.1), and
`process.chdir(__dirname)` is harmless to the shim (§ 5.2).

**Scope change accepted:** PR 2 split into 2a and 2b, so a reviewer sees the golden files change
rather than be born (§ 4).
