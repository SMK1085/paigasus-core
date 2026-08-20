# Container images RUNBOOK — `paigasus-iam` / `paigasus-gateway` (SMA-500)

Operator-facing reference for the two service container images: how to build them locally, what
they are named, how they take configuration, the liveness/readiness/startup probe contract, and
the operational rules that follow from how the images are built. This is the contract SMA-513's
Helm chart reads — nothing here may be vague or left implicit.

For the design rationale behind these choices (why a chiseled base over Chainguard/distroless
images, why one parameterized `Dockerfile` instead of two, why publishing is deferred but naming
is not), see
[`docs/superpowers/specs/2026-08-19-sma-500-service-container-images-design.md`](../superpowers/specs/2026-08-19-sma-500-service-container-images-design.md).
This runbook does not repeat it.

---

## 1. Build locally

The supported entry point is `ci/images/run.sh`, not a raw `docker build` — it also asserts the
toolchain/glibc pins (§6) and stamps OCI labels:

```bash
ci/images/run.sh all              # build + smoke-test both images
ci/images/run.sh build            # build both images, no smoke test
ci/images/run.sh build iam        # build only paigasus-iam
ci/images/run.sh build gateway    # build only paigasus-gateway
ci/images/run.sh smoke            # smoke-test whatever images are already built
```

The build context is `rs/` (the Cargo workspace root). This is also what
`.github/workflows/images.yml` runs in CI — `workflow_dispatch`, on every `push` to `main` that
touches `rs/**`, and on pull requests that touch the build inputs (`rs/Cargo.lock`,
`rs/Cargo.toml`, `rs/rust-toolchain.toml`, `rs/Dockerfile`, `rs/.dockerignore`,
`ci/images/**`). **The workflow is not a required check**, so a broken image build reds `main`
after merge rather than blocking the PR that broke it. Run `gh workflow run images.yml --ref
<branch>` on any branch touching `rs/Dockerfile` before merging it, so a broken build shows up
before the merge rather than after.

## 2. Image names

```
ghcr.io/paigasus/paigasus-iam:<git-sha>
ghcr.io/paigasus/paigasus-gateway:<git-sha>
```

Publishing to the registry is deferred — `ci/images/run.sh` only builds and smoke-tests, it never
pushes, and no registry credentials are wired into `images.yml`. The names and the `:<git-sha>`
tag convention are fixed now regardless, so SMA-513's Helm chart has a concrete
`image.repository`/`image.tag` to inherit rather than inventing its own image story.

## 3. Runtime configuration

Both services take configuration from runtime environment only. Nothing is baked into the image.
Precedence, lowest to highest:

```
defaults  <  optional TOML file  <  IAM_* / GATEWAY_* environment variables
```

A double underscore (`__`) maps an environment variable onto nested config, e.g.
`IAM_API_KEYS__PEPPER` sets `api_keys.pepper`. `GATEWAY_*` follows the identical scheme for
`paigasus-gateway`.

Mounting an `iam.toml` / `gateway.toml` into the container still works and still layers beneath
the environment — that is figment's ordinary merge behaviour, not a container-specific mechanism,
and the image does not need to change to support it. The image itself ships neither file.

## 4. Probe contract

### Listening ports

| Service | HTTP | gRPC |
| --- | --- | --- |
| `paigasus-iam` | `8080` | `9090` |
| `paigasus-gateway` | `8088` | — |

Both probes below ride the HTTP port. These are config defaults, not fixed values —
`IAM_HTTP_ADDR`, `IAM_GRPC_ADDR`, and `GATEWAY_HTTP_ADDR` each override the **full** `host:port`,
not just the port number. IAM also accepts an optional, separate `[metrics].addr`
(`IAM_METRICS__ADDR`; the gateway has the identical `GATEWAY_METRICS__ADDR`) that moves
`/metrics` onto its own port instead of merging it onto `http_addr` — when unset (the default),
`/metrics` shares the HTTP port with everything else.

| Probe | Endpoint | Notes |
| --- | --- | --- |
| liveness | `GET /healthz` | Never touches a dependency, by construction in both services |
| readiness | `GET /readyz` | IAM pings Postgres; gateway issues a real gRPC introspect to IAM |
| startup | `GET /healthz` | IAM migrates at boot — a `startupProbe` with a generous `failureThreshold` is required, or the kubelet kills it mid-migration |

The image has no shell, so every probe command must use an **absolute path** and the **exec
form** — no `sh -c`, no shell pipelines. Docker's own `HEALTHCHECK` only ever calls `/healthz`
(Docker has no separate readiness concept); `/readyz` is reachable through the same binary for
anyone wiring a Kubernetes `exec` readiness probe:

```
/usr/local/bin/paigasus-service healthcheck --path /readyz
```

Exit codes: `0` healthy, `1` unhealthy, `2` usage error (unrecognized arguments — this never falls
through to starting the service).

## 5. Operational rules that are NOT image properties

These follow from how the services behave once containerized, not from anything in the image
build itself — they bite the first operator who deploys without reading this section.

- **IAM runs `Migrator::up` on every process start, with no advisory lock around it.** A rolling
  update or a scale-out risks concurrent migration. Migrate with a single replica —
  `replicas: 1` with `strategy.rollingUpdate.maxSurge: 0` — or use a pre-install migration Job.
- **Gateway `/readyz` issues a real gRPC introspect call to IAM on every poll**, and both of the
  gateway's health routes sit inside its metrics and correlation layers (deliberately — SMA-504
  D10), so probe traffic is metered and load-bearing. Keep `readinessProbe.periodSeconds` at 30s
  or above, and filter `route!~"/healthz|/readyz"` on dashboards. IAM's health routes sit
  **outside** its equivalent layers, so this asymmetry between the two services is real, not an
  oversight to normalize away.
- **Neither service terminates TLS.** Both require a TLS-terminating ingress in front of them.
- **A private-CA identity provider is not supported.** IAM's `reqwest`-based path to the IdP
  (discovery/JWKS) carries compiled-in webpki roots, so mounting a CA certificate into the
  container does not make IAM trust it.

## 6. Conventions the console images must follow

Any future console-facing image in this repo should follow the same shape:

- A chiseled or otherwise distroless base with **no shell**.
- A numeric, non-root `USER`.
- A self-contained probe entrypoint (the binary probes itself) plus a `HEALTHCHECK` instruction
  that uses it.
- Runtime environment configuration only — nothing deployment-varying baked into the image.
- Digest-pinned base images, covered by Dependabot's `docker` ecosystem updater.
