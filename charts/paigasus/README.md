# paigasus chart

Renders the Paigasus IAM and AI Gateway console zones, and their backends, behind one ingress
origin. See `docs/superpowers/specs/2026-09-19-sma-513-multi-zone-ingress-helm-design.md` for the
design and `docs/superpowers/plans/2026-09-19-sma-513-console-images-and-chart.md` for the plan
that built it.

## One values block, six projections

`values.yaml` holds exactly one `zones` map, plus `ingress`, `oidc`, `redis` and `postgres`. A
zone's `enabled` flag governs six things together, all derived from `zones` and from nothing
else, so a zone cannot be routable but unadvertised or advertised but unrouted:

- the ingress path rule for that zone (`templates/ingress.yaml`)
- its entry in `PAIGASUS_ZONES` (`paigasus.zoneMapJson`, `templates/_helpers.tpl`)
- its entry in `PAIGASUS_SERVICES` (`paigasus.serviceMapJson`, `templates/_helpers.tpl`)
- `PAIGASUS_IAM_GRPC_URL` (`templates/console-env-configmap.yaml`), which every enabled zone's
  console reads regardless of which zone it is
- its console Deployment and Service (`templates/console-deployment.yaml`,
  `templates/console-service.yaml`)
- its backend Deployment and Service, when `backend.deploy` is true
  (`templates/backend-deployment.yaml`, `templates/backend-service.yaml`)

A disabled zone leaves no trace in any of the six — no path, no map entry, no Deployment, no
Service. `tests/ingress.sh` and `tests/maps.sh` assert this directly.

## The refusals

`paigasus.validate` (`templates/_helpers.tpl`) runs first in every template and fails the render,
with its own message, rather than letting a bad values file produce broken Kubernetes objects:

- **No zone enabled.** A chart with no zone serves nothing, so this is refused outright rather
  than rendered as an empty, useless release.
- **The gateway zone without the iam zone.** gateway-console's `PAIGASUS_SERVICES` schema refuses
  to construct without an `iam` entry, so its pods would crash-loop on first request. Refusing at
  render time is cheaper than a crash-loop.
- **An unknown zone id.** `parseServiceMap` throws on any key outside `SERVICE_SLUGS` in both
  consoles, at first request. A zone id must be a member of `SERVICE_SLUGS`
  (`ts/packages/paigasus-discovery/src/core/state.ts`); `paigasus.serviceSlugs` in
  `_helpers.tpl` must be kept equal to it by hand.
- **A backend the chart does not deploy, with no `backend.url`.** See the next section.
- **Every value `values.yaml` marks REQUIRED, when it is empty.** `ingress.host`,
  `ingress.tlsSecretName`, `oidc.issuer`, `oidc.clientId`, `oidc.existingSecret`,
  `postgres.existingSecret` and `zones.iam.backend.apiKeysPepperSecret` each fail with their own
  named message the moment they are unset. Before SMA-513 Task 14, `paigasus.validate` refused
  only `ingress.host`; the other seven were required by comment alone, and a default
  `helm install` rendered a Deployment with an empty `secretKeyRef.name` for the pepper secret —
  a manifest the Kubernetes API server refuses. This is what stops that: every REQUIRED value is
  now refused at render time, so a values file missing one never reaches the API server at all.

`tests/refusals.sh` renders each refusal case and asserts it fails with its own message, not an
incidental template error from somewhere else — otherwise the chart could refuse by accident and
a later edit would silently make it install.

## The chart cannot observe an external Secret's contents

The chart does not own `oidc.existingSecret` and cannot see when its contents rotate. `helm
template` cannot run `lookup` either, so there is no value to hash. `templates/console-deployment.yaml`
therefore does not hash the Secret's contents — it hashes `oidc.secretVersion`
(`values.yaml`), a knob with no default and no required check. Bump it to any new value whenever
the referenced Secret's contents change, so the `checksum/secret` pod annotation changes and the
console pods roll. Nothing enforces that an operator remembers to bump it.

## No rewrite annotation

`values.yaml`'s `ingress.annotations` defaults to `{}` and `templates/ingress.yaml` adds no
rewrite annotation of its own. Do not add one. Each console compiles its `basePath` in and serves
its full path already; a rewrite that strips a prefix such as `/iam` breaks every route in that
zone. This is the change an operator is most likely to add by reflex, so it is called out in both
`values.yaml` and `templates/ingress.yaml` as well as here.

## A zone may supply its backend address instead of having it deployed

`zones.<id>.backend.deploy` defaults to `true`: the chart deploys the backend Deployment and
Service itself, and `paigasus.serviceMapJson` points `PAIGASUS_SERVICES` at that in-cluster
Service. When `deploy` is `false`, the chart deploys no backend for that zone at all, and
`zones.<id>.backend.url` becomes required — `paigasus.validate` refuses the render otherwise —
holding the full base URL of an existing backend elsewhere (the same shape as `postgres.existingSecret` and
`oidc.existingSecret`, which also name infrastructure this chart does not create).

**This applies to every zone except `iam`.** `zones.iam.backend.deploy=false` is refused: `PAIGASUS_IAM_GRPC_URL` is built from the chart-managed IAM Service, so an external IAM would leave every console pointing at a Service that does not exist. An external IAM needs its own values key, and that is not in this chart yet.

**And `zones.gateway.backend.deploy=true` is refused too**, in the other direction: `GatewayConfig::validate` hard-fails on an empty `upstream.openai.api_key`, and `iam.grpc_addr` accepts `LoopbackInsecure` only for a loopback host, so an in-cluster gateway-to-IAM link needs a TLS design this chart does not have. Rendering that Deployment would produce a pod that cannot boot.

The `gateway` zone ships with `backend.deploy: false` because the gateway backend cannot
currently be deployed safely by this chart:

- `GatewayConfig::validate` hard-fails boot on an empty `upstream.openai.api_key`
  (`rs/crates/services/paigasus-gateway/src/config.rs:278`).
- `iam.grpc_addr` accepts `LoopbackInsecure` only for a loopback host
  (`rs/crates/services/paigasus-gateway/src/config.rs:209-222`), so an in-cluster
  gateway-to-IAM gRPC link needs a TLS design this chart does not yet have.

The `iam` zone ships with `backend.deploy: true` because IAM has no such dependency and this
chart can deploy it directly.

## The golden files

`tests/golden/iam-only.yaml` and `tests/golden/iam-and-gateway.yaml` are a byte-exact pin of
`helm template` for the two valid zone subsets, rendered with a fixed `--kube-version` and a
fixed set of required values (`tests/render.sh`'s `FIXED` array) so the files do not move with
the local Helm binary's default Kubernetes version. They are pinned to **Helm v3.22.0+g144ca65**
(`docs/superpowers/specs/2026-09-19-sma-513-measurements.md`, M2) — a Helm upgrade may change
unrelated rendering details (indentation, key order) and would need a deliberate re-baseline, not
a silent regeneration.

`tests/render.sh --update` re-baselines both files from the current chart. This is a deliberate
act, done by a human reviewing the resulting diff, never a mechanical step to clear a red. A
golden-file change is the reviewable artifact of a chart change: read the diff before committing
it.

## Running the tests

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test
charts/paigasus/tests/maps.sh
charts/paigasus/tests/ingress.sh
charts/paigasus/tests/render.sh
```

`refusals.sh` needs `--set ingress.host=…` from its caller, since its own valid-render rows have
no default host; it holds the other seven REQUIRED values valid by default in its own `FIXED`
array, so each refusal row can set only the one value it names to `""`. `maps.sh`, `ingress.sh`
and `render.sh` set every REQUIRED value themselves, `ingress.host` included.
