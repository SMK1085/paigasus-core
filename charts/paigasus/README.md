# paigasus chart

Renders the Paigasus IAM and AI Gateway console zones, and their backends, behind one ingress
origin. See `docs/superpowers/specs/2026-09-19-sma-513-multi-zone-ingress-helm-design.md` for the
design and `docs/superpowers/plans/2026-09-19-sma-513-console-images-and-chart.md` for the plan
that built it.

## One values block, six projections

`values.yaml` holds exactly one `zones` map, plus `ingress`, `oidc`, `redis` and `postgres`. A
zone's `enabled` flag governs six things together, all derived from `zones` and from nothing
else, so a zone cannot be routable but unadvertised or advertised but unrouted:

- the route path rule for that zone: the Ingress rule (`templates/ingress.yaml`), and the
  HTTPRoute (`templates/httproute.yaml`) when `httpRoute.enabled` is true
- its entry in `PAIGASUS_ZONES` (`paigasus.zoneMapJson`, `templates/_helpers.tpl`)
- its entry in `PAIGASUS_SERVICES` (`paigasus.serviceMapJson`, `templates/_helpers.tpl`)
- `PAIGASUS_IAM_GRPC_URL` (`templates/console-env-configmap.yaml`), which every enabled zone's
  console reads regardless of which zone it is
- its console Deployment and Service (`templates/console-deployment.yaml`,
  `templates/console-service.yaml`)
- its backend Deployment and Service, when `backend.deploy` is true
  (`templates/backend-deployment.yaml`, `templates/backend-service.yaml`)

A disabled zone leaves no trace in any of the six — no path, no map entry, no Deployment, no
Service. `tests/ingress.sh` (also for the HTTPRoute) and `tests/maps.sh` assert this directly.

**Exception: `ingress.enabled: false` and `httpRoute.enabled: false` (SMA-695).** The chart then
renders no route, so the first projection is gone. The operator's own route is a projection of
`zones` that the operator keeps by hand. A route to a disabled zone points to a deleted Service.
An enabled zone with no route gives a 404 for its `PAIGASUS_ZONES` link. With
`httpRoute.enabled: true` (SMA-694) the chart renders the route again, and D6 holds.

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
  `ingress.tlsSecretName` (only when `ingress.enabled` is true), `oidc.issuer`, `oidc.clientId`,
  `oidc.existingSecret`, `postgres.existingSecret` and `zones.iam.backend.apiKeysPepperSecret` each
  fail with their own named message the moment they are unset. Before SMA-513 Task 14, `paigasus.validate` refused
  only `ingress.host`; the other seven were required by comment alone, and a default
  `helm install` rendered a Deployment with an empty `secretKeyRef.name` for the pepper secret —
  a manifest the Kubernetes API server refuses. This is what stops that: every REQUIRED value is
  now refused at render time, so a values file missing one never reaches the API server at all.

- **A non-boolean `ingress.enabled`, and an `ingress.host` with a scheme, path or port
  (SMA-695).** A quoted `"false"` is a string, and a string is true in a template `if`, so the
  Ingress would stay. A host such as `https://console.example.com` renders
  `PAIGASUS_PUBLIC_ORIGIN=https://https://console.example.com`. With the Ingress on, the API server
  refuses such a host. With it off, only this refusal does.

- **A bad bootstrap admin or `extraEnv` entry (SMA-697).** `paigasus.validateIamBackend` in
  `templates/_iam-backend.tpl` refuses a bootstrap admin that IAM would refuse at boot (an empty
  subject, an issuer that is not `https`), a subject that is not a string, and an issuer that is
  not `oidc.issuer`, which IAM never matches. It refuses an `extraEnv` name that the chart sets
  itself. See `docs/ops/RUNBOOK-chart.md` § 9.

- **A bad `httpRoute` block (SMA-694).** `paigasus.validateHttpRoute` in
  `templates/_httproute.tpl` refuses:
  - `httpRoute` that is not a map: `httpRoute must be a map`;
  - `httpRoute.enabled` that is not a boolean: `httpRoute.enabled must be true or false`;
  - in route mode, an absent, null, empty or non-list `parentRefs`:
    `httpRoute.parentRefs is required when httpRoute.enabled is true`. An absent key counts as
    empty, so a `--reuse-values` upgrade that turns the route on without it fails;
  - a parentRef that is not a map or has no string `name`:
    `httpRoute.parentRefs[<i>] must be a map with a non-empty name`;
  - a parentRef with neither `sectionName` nor `port`:
    `httpRoute.parentRefs[<i>] must set sectionName or port`. Such a parentRef can attach to every
    compatible listener whose `allowedRoutes` admits the route, a plain HTTP one included, and the
    console then answers on `http://`;
  - an `ingress.host` with an upper-case letter: `ingress.host must be lowercase when
    httpRoute.enabled is true`. The API server refuses such an HTTPRoute hostname;
  - `httpRoute.annotations` that is not a map, or a value that is not a string:
    `httpRoute.annotations must be a map`, `httpRoute.annotations.<key> must be a string`;
  - a timeouts map (`httpRoute.timeouts` or an enabled zone's
    `zones.<id>.console.httpRouteTimeouts`) that is not a map, has a key other than `request`
    and `backendRequest`, or has a value that is not a Gateway API duration:
    `<path> must be a map`, `<path> has the unknown key <key>`, `<path>.<key> must be a
    duration`;
  - a merged `backendRequest` longer than the merged `request`, when `request` is not `0s`:
    `zones.<id>: the HTTPRoute backendRequest timeout <b> is longer than the request timeout <r>`.

  The HTTPRoute CRD and the API server refuse some of these values too (an upper-case hostname,
  a bad duration, a `backendRequest` longer than `request`, a non-string annotation), but only at
  apply time. In Argo CD that is a sync error that is easy to miss. The CRD accepts the others:
  an absent `parentRefs` gives a route that attaches to no Gateway, and a parentRef without
  `sectionName` or `port` attaches to every listener. Only the chart refuses those two.

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

## Running without an Ingress controller

Set `ingress.enabled: false` when the cluster has no Ingress controller, for example a cluster
that uses Gateway API. The chart then renders no Ingress, and `ingress.tlsSecretName` is not
required. `ingress.host` stays required, because it feeds `PAIGASUS_PUBLIC_ORIGIN` and the OIDC
redirect URIs. `ingress.className` and `ingress.annotations` then do nothing.

`templates/ingress.yaml` wraps its body in `paigasus.ingressEnabled` (`templates/_helpers.tpl`).
`tests/ingress.sh` and `tests/refusals.sh` hold the SMA-695 rows.

**Gateway API: the chart-owned HTTPRoute (SMA-694).** Set `httpRoute.enabled: true` and
`httpRoute.parentRefs`. The chart then renders one `gateway.networking.k8s.io/v1` HTTPRoute for
each enabled zone, with the same name as the zone's console Service. Each route has one rule: a
`PathPrefix` match on the zone's `basePath`, sent to the console Service on port 3000, with no
filter. Its `hostnames` is `[<ingress.host>]`. The two switches are independent: both can be on
during a cut-over. The prerequisites:

- the Gateway API CRDs, standard channel, with `HTTPRoute` served at `v1`. `timeouts` needs
  Gateway API v1.2.0 or later;
- a Gateway with an HTTPS listener that ends TLS for `ingress.host` and allows the release
  namespace in `allowedRoutes`. Each parentRef names that listener with `sectionName` or `port`;
- an HTTP-to-HTTPS `RequestRedirect` route on any plain HTTP listener of that Gateway;
- create permission for the deployer on `gateway.networking.k8s.io/httproutes`. The standard
  Gateway API install adds no aggregated rules to the `admin` and `edit` ClusterRoles;
- an Argo CD AppProject resource whitelist that allows the `HTTPRoute` kind.

`httpRoute.timeouts` sets `request` and `backendRequest` on the rule of every enabled zone.
`zones.<id>.console.httpRouteTimeouts` sets them for one zone, and a key there wins. The gateway
zone has the default `request: 10m`, because its chat relay streams for longer than the 15 s
default route timeout of Envoy. `httpRoute.annotations` goes on each HTTPRoute, for example for
external-dns. `tests/ingress.sh` (rows H1-H8), `tests/refusals.sh`, `tests/names.sh` and the
third golden hold the SMA-694 rows. The chart does not check that the cluster has the CRDs: a
`.Capabilities` gate would fail `helm template` in CI and in the Argo CD repo server.

You can also keep both switches off and write the route yourself. The routing contract is in
`docs/ops/RUNBOOK-chart.md` § 11.

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

## Trusting an IdP with a private CA (`oidc.caBundle`)

Both consumers of the issuer refuse a non-`https` URL, so an IdP with a private CA needs its root
in the pods. Set `oidc.caBundle.existingConfigMap` to a ConfigMap that holds PEM root certificates
under `oidc.caBundle.key` (default `ca.crt`). Every console pod and the IAM pod then mount that one
key read-only under `/etc/paigasus/idp-ca/`. The consoles get `NODE_EXTRA_CA_CERTS`, in the pod
env and not in the shared `console-env` ConfigMap. IAM gets `IAM_AUTHN__EXTRA_CA_BUNDLE_PATH`.

- The volume uses `items`, so a missing ConfigMap or key stops the pod at volume setup.
- `existingConfigMap` with an empty `key` is refused at render time.
- Bump `oidc.caBundle.version` after the ConfigMap changes. Node and IAM read the bundle once, at
  process start. The version feeds a `checksum/idp-ca` annotation on all three Deployments.
- The consoles trust the bundle for every TLS connection, Redis included. IAM trusts it only for
  JWKS fetches. Put only the roots the IdP needs in it.

With the value empty, the render is byte-identical to a chart without it. See
`docs/ops/RUNBOOK-chart.md` for the failure modes.

## The access-token audience (`oidc.audience`)

IAM accepts an access token only if its `aud` claim contains a configured audience. The chart
renders that list, with one element, into `IAM_AUTHN__ISSUERS` in
`templates/backend-deployment.yaml`.

- **Empty or absent (the default).** The audience is `oidc.clientId`. The render is byte-identical
  to a chart without the value. Under `helm upgrade --reuse-values` from an older release, the key
  is absent. The template then reads nil. The result is the same.
- **Set.** The value replaces `oidc.clientId`. It does not add to it. IAM then refuses a token
  whose `aud` holds only the client id. This includes an ID token.
- **A number** (`--set oidc.audience=12345`, or an unquoted number in a values file) renders as the
  string `"12345"`. The template applies `toString` before `%q`. Quote the value in a values file.
  A large number can change to an exponent form.
- When the value is set, one more YAML comment line renders above `IAM_AUTHN__ISSUERS`:
  `# oidc.audience is set: IAM accepts that audience, not the client id.`
- A change of the value restarts the IAM pod and no console pod.
- **The warning (SMA-691).** When the IAM audience equals `oidc.clientId`, an ID token passes
  IAM's audience check. The chart then shows a warning in two places: the release NOTES
  (`templates/NOTES.txt`) and the annotation `paigasus.io/iam-audience-warning` in the IAM backend
  Deployment's `metadata`. `oidc.acknowledgeClientIdAudience`, set to the value of
  `oidc.clientId`, removes both. It does not change what IAM accepts.
- The helpers are in `templates/_audience.tpl`. Both signals call `paigasus.iamAudienceWarns`. The
  recommended setup is a dedicated API audience in `oidc.audience`.
- **NOTES has no offline render.** `helm template` executes `NOTES.txt` but does not print it, and
  `helm install --dry-run` needs a cluster. So `tests/env.sh` wraps the bytes of `NOTES.txt` in a
  named template in a copy of the chart and renders it through a probe ConfigMap. The kind job
  checks the NOTES of the real release (`ci/kind/README.md`).

`tests/env.sh` holds the rows: `A1 unset`, `A2 reuse-values-no-key` (`--set oidc.audience=null`),
`A3 set`, `A4 number`, `A5 number-in-file` and `A6 restart-scope`. For the warning annotation it
holds `W1 default` to `W14 no-restart`. For the NOTES text it holds `N0 pin` (the bytes of
`NOTES.txt`) and `N1 default` to `N6 other-client`. Three row counters red the script when a row call
line is deleted. See `docs/ops/RUNBOOK-chart.md` § 6 for the recommended setup and the migration
order.

## The ID-token marker claims (`oidc.idTokenMarkerClaims`)

IAM can refuse a token that carries a claim which the IdP puts into its ID token only (SMA-703).
Zitadel needs this, because no audience setting separates its two tokens. The chart renders the
list into the one issuer entry of `IAM_AUTHN__ISSUERS` in `templates/backend-deployment.yaml`.

- **Empty or absent (the default).** The chart adds nothing. The render is byte-identical to a
  chart without the value. Under `helm upgrade --reuse-values` from an older release, the key is
  absent. The template then reads the default. The result is the same. If that release also gets
  `--set oidc.idTokenMarkerClaims=null`, Helm keeps a nil value, and the template counts it as
  an empty list.
- **Set.** The entry gets `,id_token_marker_claims=["at_hash","azp"]` after `audiences`. Each name
  is quoted with `%q`. One more YAML comment line renders above `IAM_AUTHN__ISSUERS`:
  `# oidc.idTokenMarkerClaims is set: IAM refuses a token that carries one of these claims.`
- **The render fails** when the value is not a list, when a name is not a string or is empty,
  when a name has a character outside printable ASCII, a space, `"` or `\`, when a name is `iss`,
  `sub`, `aud` or `exp`, and when a name occurs twice. IAM refuses an empty, padded, reserved or
  repeated name at boot, and the IAM Deployment has one replica with `maxSurge: 0`, so the chart
  refuses these first. The character rule is a chart rule only: it keeps the rendered env value
  readable for IAM. The checks
  are in `paigasus.validateIdTokenMarkerClaims` in `templates/_iam-backend.tpl`. They are not in
  `_helpers.tpl`, so its fixture copies do not change.
- **To remove the value,** set `[]` in a values file, or use
  `--set-json 'oidc.idTokenMarkerClaims=[]'`. With `helm upgrade --reuse-values`, deleting the key
  from a values file does not remove the old list. Do not use
  `--set oidc.idTokenMarkerClaims={}`: Helm makes it a list with one empty name, and the render
  fails.
- A change of the value restarts the IAM pod and no console pod.

`tests/env.sh` holds the rows `M1 default`, `M2 reuse-values-no-key`, `M3 set`,
`M4 empty-list-in-file`, `M5 with-audience`, `M6 restart-scope`, `M7 empty-list-set-json` and
`M8 reuse-values-nil-guard`. A sixth row counter checks them. `tests/refusals.sh` holds one row for each refusal and one valid render. See
`docs/ops/RUNBOOK-chart.md` § 6 for the IdP setup.

## The console authorization request (`oidc.scopes`, `oidc.authorizationAudience`)

Two values change what both consoles request from the IdP (SMA-692). Both values are empty by
default. An empty value renders no key. The render is then byte-identical to a chart without
them.

- `oidc.scopes` renders `PAIGASUS_OIDC_SCOPES` into the `console-env` ConfigMap. When empty, the
  console uses its default scopes: `openid profile email offline_access`. When set, the consoles
  also send this list as the `scope` of each refresh request. Entra ID needs one scope of its
  API.
- `oidc.authorizationAudience` renders `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE`. The consoles send
  it as the `audience` parameter of the authorization request. They do not send it on a refresh.
  Auth0 needs it.
- The helpers are `paigasus.consoleScopes` and `paigasus.consoleAuthorizationAudience` in
  `templates/_audience.tpl`. They read the values with `dig`. An absent key under
  `--reuse-values` then gives `""`. `console-env-configmap.yaml` calls them. That file renders on
  every install. Their refusals fire on every render. They are not part of `paigasus.validate`.
  Because of this, `_helpers.tpl` and its two fixture copies do not change.
- Both values are normalized before they render, on the explicit whitespace class
  `[\t\n\f\r ]` (final fix I1, not the wider `\s`). `oidc.scopes` splits on that class, drops
  empty tokens, and re-joins the rest with one space; a whitespace-only value normalizes to
  empty and renders no key, the same as an absent value. `@paigasus/auth` normalizes on the same
  class, so both sides agree.
- The render fails in four cases:
  - `oidc.scopes` is set, normalizes to a non-empty list, and that list does not hold the token
    `openid`. The token `openidx` does not count.
  - `oidc.authorizationAudience` is set with leading or trailing whitespace (final fix M3).
    `@paigasus/auth` refuses the same value at pod start, on the same rule.
  - `oidc.authorizationAudience` is set and `oidc.audience` is empty.
  - `oidc.authorizationAudience` is set and does not equal `oidc.audience`.

  The compare is exact, as strings.
- A change of either value changes the `console-env` ConfigMap. `checksum/console-env` then
  restarts both consoles. IAM does not restart.

`tests/env.sh` holds the rows `O1 unset`, `O2 both-set`, `O3 reuse-values-no-key`, `O4 number`,
`O5 restart-scopes`, `O6 restart-audience`, `O7 tab-scopes`, `O8 newline-scopes` and
`O9 whitespace-only-scopes`. A fourth row counter checks them.
`tests/refusals.sh` holds the five refusals and three valid renders. See
`docs/ops/RUNBOOK-chart.md` § 6 for the IdP setup.

## The default image tags

Each `image.tag` in `values.yaml` is pinned to the published version of that image. The version
lives in the file that `ci/images/chains.toml` names for the image: a service's `Cargo.toml` or a
console's `package.json`. `repo:helm-render` row 8a fails when a tag and its version differ, so a
version bump must update the tag in the same pull request. The tags no longer default to
`appVersion`. An empty tag falls back to `.Chart.AppVersion`.
`repo:helm-render` row 8c requires `appVersion` to be a version that every image released. The git
tag `paigasus-<key>-v<appVersion>` must exist for each chain. So `appVersion` is the fallback tag,
not the deployed version. It can be older than the pinned tags, and `helm list` then shows that
older value. To move `appVersion`, use a later pull request, after every image released the new
version. A pull request that bumps the versions and `appVersion` together fails row 8c. It cannot
merge, because the release runs only after the merge. So split it into two pull requests.

## The golden files

`tests/golden/iam-only.yaml` and `tests/golden/iam-and-gateway.yaml` are a byte-exact pin of
`helm template` for the two valid zone subsets, and `tests/golden/iam-and-gateway-httproute.yaml`
pins the HTTPRoute mode (SMA-694), rendered with a fixed `--kube-version` and a
fixed set of required values (`tests/render.sh`'s `FIXED` array) so the files do not move with
the local Helm binary's default Kubernetes version. They are pinned to **Helm v3.22.0+g144ca65**
(`docs/superpowers/specs/2026-09-19-sma-513-measurements.md`, M2) — a Helm upgrade may change
unrelated rendering details (indentation, key order) and would need a deliberate re-baseline, not
a silent regeneration.

`tests/render.sh --update` re-baselines all three files from the current chart. This is a deliberate
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
charts/paigasus/tests/names.sh
charts/paigasus/tests/env.sh
charts/paigasus/tests/ca-bundle.sh
```

`refusals.sh` needs `--set ingress.host=…` from its caller, since its own valid-render rows have
no default host; it holds the other seven REQUIRED values valid by default in its own `FIXED`
array, so each refusal row can set only the one value it names to `""`. `maps.sh`, `ingress.sh`,
`render.sh`, `names.sh`, `env.sh` and `ca-bundle.sh` set every REQUIRED value themselves,
`ingress.host` included.

`repo:helm-render` runs all seven in CI, each with `--set ingress.host=console.example.test`, and
adds checks the scripts do not make. See `ci/helm-render/README.md`.
