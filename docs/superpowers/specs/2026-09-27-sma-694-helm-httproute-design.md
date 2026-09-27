<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-694: the Helm chart renders a Gateway API HTTPRoute as an alternative to the Ingress

- Linear: [SMA-694](https://linear.app/smaschek/issue/SMA-694)
- Branch: `feature/sma-694-helm-httproute`
- Related: SMA-695 (`ingress.enabled`, merged on `main` as `4051df5e`, PR #332), SMA-697, SMA-513
  (the chart and decision D6).
- Chart: `charts/paigasus`, version 0.1.0.
- Status: challenged once (APPROVE WITH CHANGES). The changes are folded in (§ 10). Approved by
  Sven on 2026-09-27 with the decisions in § 11. The spec is checked against `origin/main`
  `4051df5e` (§ 12).

## 1. The problem

`templates/ingress.yaml` renders only a `networking.k8s.io/v1` Ingress. The chart has no
HTTPRoute template. The install target is a K3s cluster with Cilium Gateway API and no Ingress
controller. No controller serves the Ingress, so the consoles are not reachable.

SMA-695 lets the operator turn the Ingress off. It then says that the operator writes the route by
hand (the "routing contract" in `docs/ops/RUNBOOK-chart.md` § 11), and that decision D6
("`zones.<id>.enabled` controls every projection of a zone together") does not hold for such a
route (SMA-695 spec D2). SMA-694 closes that gap: the chart renders the route from the same
`range` over `zones` as the Ingress.

Checked on `origin/main` (`4051df5e`, 2026-09-27): no commit mentions HTTPRoute or SMA-694 as
done, and `charts/paigasus/templates/` has no HTTPRoute template. SMA-695 left three forward
references to this issue: `charts/paigasus/README.md` lines 30-31, `docs/ops/RUNBOOK-chart.md`
line 549, and the `values.yaml` header lines 8-9. The issue is not done.

## 2. The goal and the acceptance criteria

The issue's two criteria are AC1 and AC6. The issue text is: "`helm template` with
`httpRoute.enabled` set renders one HTTPRoute for each enabled zone console path" and "The kind CI
job keeps passing." The other criteria follow from the design.

1. **AC1.** `helm template` with `httpRoute.enabled: true` renders exactly one
   `gateway.networking.k8s.io/v1` HTTPRoute for each enabled zone, for both valid zone subsets
   (`iam`; `iam` and `gateway`).
2. **AC2.** Each HTTPRoute has one rule. The rule matches `PathPrefix` `<zones.<id>.basePath>` and
   sends to that zone's console Service on port 3000. It has no filter, so no path rewrite. Its
   `hostnames` is exactly `[<ingress.host>]`.
3. **AC3.** A disabled zone leaves no trace in a render with `httpRoute.enabled: true`
   (the same rule as `tests/ingress.sh`). The Gateway API group name in `apiVersion` is not a
   trace (§ 5.1, H2).
4. **AC4.** The default render (no `httpRoute` key, or `httpRoute.enabled: false`) is
   byte-identical to the render before this change. The two golden files in
   `charts/paigasus/tests/golden/` do not change.
5. **AC5.** A release made before this change (no `httpRoute` map, upgraded with
   `--reuse-values`) renders no HTTPRoute and does not fail. The same release, upgraded with
   `--reuse-values --set httpRoute.enabled=true` and no `parentRefs`, fails with the
   `parentRefs` refusal. It does not render a route that attaches to no Gateway.
6. **AC6.** The kind CI job keeps passing. It uses the Traefik Ingress and does not set
   `httpRoute`, so its render does not change.
7. **AC7.** With `ingress.enabled: false` and `httpRoute.enabled: true`, an empty
   `ingress.tlsSecretName` renders (SMA-695 behaviour, checked again here for this mode).
8. **AC8.** Each refusal in § 4.4 fails the render with its own message.
9. **AC9 (Q7, added on approval).** Each rule carries a Gateway API v1 `timeouts` block
   (`request`, `backendRequest`) when a timeout is set for its zone (D13). With the default
   values, the gateway zone's rule has `timeouts.request: 10m`, and the iam zone's rule has no
   `timeouts` key. A render with neither key set has no `timeouts` key.
10. **AC10 (Q8, added on approval).** `httpRoute.annotations` renders as `metadata.annotations`
    on each HTTPRoute. The default (an empty map) renders no `annotations` key (D14).

## 3. The decisions

### D1. SMA-694 builds on SMA-695, which is merged.

SMA-695 merged on 2026-09-27 (`4051df5e`, PR #332). It added `ingress.enabled`, the helper
`paigasus.ingressEnabled` (`templates/_helpers.tpl` line 52), the host-form check (lines 143-147)
and the conditional `tlsSecretName` refusal (lines 82-84). SMA-694 needs all four. The merged code
matches the SMA-695 spec in each of these points (§ 12). A cluster without an Ingress controller
needs both issues: the HTTPRoute to reach the consoles, and `ingress.enabled: false` so that Argo
CD does not show the Application as Progressing forever.

The SMA-695 spec says: "SMA-694 decides whether `ingress.enabled` and its own switch are mutually
exclusive." D3 below decides it.

### D2. The route host is always `ingress.host`. There is no `httpRoute.hostnames` value.

`ingress.host` feeds `PAIGASUS_PUBLIC_ORIGIN` (`templates/console-env-configmap.yaml` line 8).
The consoles build the OIDC redirect URI from it, and the `__Host-pgs_sid` cookie is host-only.
SMA-695 D1 keeps `ingress.host` required also when the Ingress is off, and does not add a neutral
`publicHost` key. This spec follows that decision. The chart always renders
`hostnames: [<ingress.host>]`.

The issue proposes a `httpRoute.hostnames` value. This spec declines it, for this reason:

- An extra host in THIS route sends that host straight to the console. The console builds its
  redirects and its cookie for `ingress.host`, so login on the extra host cannot work.
- The route offers no filters (D7), so the route itself cannot redirect the extra host.
- A separate redirect route for the extra host with `PathPrefix /` does not help either. The
  chart route has the longer path match for `/iam/*`, so Gateway API gives it those requests, and
  the redirect never fires on the console paths.

So the value has no working use case. It would add refusals, rows and an operator trap. An
operator who needs an alias host writes a redirect route for that host alone, and does not list it
in the chart route. **Sven accepted this decision on 2026-09-27 (Q2).**

Gateway API hostnames must be lowercase RFC 1123 names. The SMA-695 host-form rule
(`_helpers.tpl` lines 143-147: no `/`, no `:`) accepts `Console.Example.test`, and the API server
then refuses the route at apply time. So route mode adds a lowercase refusal on `ingress.host`
(§ 4.4).

### D3. `ingress.enabled` and `httpRoute.enabled` are independent. Both can be true.

The SMA-695 cut-over procedure (`docs/ops/RUNBOOK-chart.md` § 11, "Cut-over on a release that has
a live Ingress") is: "1. Create the new route and verify it. 2. Set `ingress.enabled: false` and
sync. 3. Remove the old Ingress." Step 1 needs the Ingress and the route live at the same time. A
mutual-exclusion refusal would force a gap with no route.

Both false is also allowed. That is the SMA-695 mode "the operator writes the route by hand".
The chart shows no warning for it.

### D4. One HTTPRoute for each enabled zone, named like the zone's console Service.

The issue asks for "one HTTPRoute for each enabled zone console path" (the literal AC text). The
name is `include "paigasus.name" (list $root (printf "%s-console" $id))`, the same name as the
console Service. Kubernetes names are unique per kind, and `tests/names.sh` checks uniqueness per
kind (line 59), so the equal name is legal. It also keeps the suffix at 12 or 16 characters, which
`paigasus.name` already handles for a long release name.

The alternative, one HTTPRoute with one rule per zone, is rejected. It is not what the issue asks.
Also, per-zone objects let the Gateway report `Accepted` and `ResolvedRefs` for each zone alone.
Per-zone objects also let each rule carry its own `timeouts` (D13).

The equal name has one cost. An operator who followed the SMA-695 routing contract (RUNBOOK § 11)
copied the Service name into a hand-written route. If that route has the same name,
`helm upgrade` fails with "invalid ownership metadata". The RUNBOOK migration procedure (§ 6)
handles it.

### D5. `parentRefs` is passed through as the operator writes it.

`httpRoute.parentRefs` is a list of Gateway API `ParentReference` objects. The chart renders it
with `toYaml` and does not add `group`, `kind`, `namespace`, `sectionName` or `port`. The Gateway
API defaults apply (`group: gateway.networking.k8s.io`, `kind: Gateway`, the route's namespace).

The chart checks only what it can check without the cluster: the value is a non-empty list, each
entry is a map with a non-empty string `name`, and each entry sets `sectionName` or `port` (D11).
An absent `parentRefs` key counts as an empty list. A Gateway in another namespace must allow the
release namespace in its listener's `allowedRoutes`. The chart cannot see that. The docs state it
(§ 6).

### D6. The backend port is the number 3000, from one helper.

An HTTPRoute `backendRef` needs a port NUMBER. It cannot name the port `http`, as the Ingress does.
The Service port is written only in `templates/console-service.yaml` (line 18). The number 3000
also appears as the container port and the `PORT` env value in `console-deployment.yaml`
(lines 54 and 59), and in the `helm_render.py` fixtures (lines 1027, 1044, 1047). Those are the
container side, and the Service reaches them by the port name `http`. A second literal copy of the
SERVICE port in `httproute.yaml` could drift. So a new helper `paigasus.consoleServicePort` yields
`3000`, and both `console-service.yaml` and `httproute.yaml` use it. The rendered Service does not
change, so the goldens stay equal.

### D7. The HTTPRoute has no filters. No rewrite, no hostname rewrite.

The rule in `charts/CLAUDE.md` for the Ingress applies here too: each console has its `basePath`
compiled in, and a `URLRewrite` that removes `/iam` breaks every route in that zone. A
`URLRewrite` hostname filter breaks Next's Server Action origin check (RUNBOOK § 11, routing
contract item 3). The chart offers no `filters` value. The template comment says so, inside the
`if` block.

### D8. No `.Capabilities` gate on the Gateway API CRDs.

`helm template` in CI and in Argo CD's repo server does not always see the cluster's API versions.
A refusal on `.Capabilities.APIVersions.Has "gateway.networking.k8s.io/v1"` would fail those
renders. Without the CRD, the API server refuses the object at apply time with a clear error. The
docs state the prerequisites (§ 6).

### D9. No new REQUIRED value.

`httpRoute.enabled` defaults to `false`. So `STUB_VALUES` in `helm_render.py`, the value lists of
the seven chart scripts and `ci/kind/values/a.yaml` do not change (`charts/CLAUDE.md`, the rule
about a new REQUIRED value). The new optional values (`httpRoute.timeouts`,
`httpRoute.annotations`, `zones.<id>.console.httpRouteTimeouts`) change nothing outside route
mode.

### D10. The route checks go into the chart scripts, not into `helm_render.py`.

The `repo:helm-render` rows 1.1 and 1.2 render with `STUB_VALUES`, where `httpRoute` is off, so
they see no HTTPRoute. A new render subset in `helm_render.py` would need new rows, a new
negative-control fixture and README table rows. The route coupling is instead asserted in
`tests/ingress.sh`, which `repo:helm-render` already runs as row `6 ingress.sh`. No new chart
script, so the floor `CHART_SCRIPT_FLOOR=7` and `HELM_RENDER_SH_CALL_SITES` do not change.

The cost: no automatic negative control proves the new rows fail. § 5.2 replaces it with a manual
mutation record. **Sven accepted this on 2026-09-27 (Q3): a manual mutation record is enough, and
`helm_render.py` gets no HTTPRoute subset.**

Note for a future subset: `helm_render.py` check 2 lowercases the raw render (line 430). An
HTTPRoute subset there would hit the same `gateway` collision as H2 (the `apiVersion` group name).
It would also match any "Gateway API" comment. Such a subset must strip the `apiVersion` line
first.

### D11. Route mode keeps the TLS rule. Each parentRef must name a listener.

The Ingress path refuses a render without TLS, because `PAIGASUS_PUBLIC_ORIGIN` is `https` and
`__Host-pgs_sid` needs `Secure`. A parentRef with no `sectionName` and no `port` attaches to EVERY
listener of the Gateway, a plain HTTP listener included. The console then answers on `http://`.
Login cannot set the cookie there, and page content can be changed in transit.

So the chart refuses a parentRef that sets neither `sectionName` nor `port`. The chart cannot see
whether the named listener is HTTPS. The values comment, README and RUNBOOK tell the operator to
name the HTTPS listener (or `port: 443`), and to add an HTTP-to-HTTPS `RequestRedirect` route on
the HTTP listener. **Sven accepted the refusal on 2026-09-27 (Q6). There is no NOTES warning
instead.**

### D12. The route helpers live in a new `templates/_httproute.tpl`.

The helpers and a `paigasus.validateHttpRoute` define go into `templates/_httproute.tpl`.
`paigasus.validate` in `_helpers.tpl` calls it with one line, after
`paigasus.validateIamBackend` (today line 150, the last line before the `end` of the define).
`_iam-backend.tpl` already uses this pattern. The negative control copies the live chart and then
overlays each fixture (`ci/helm-render/run.sh` line 216). So a fixture `_helpers.tpl` without the
new line still renders: the new helpers come from the live `_httproute.tpl`.

### D13. Timeouts: a chart-wide `httpRoute.timeouts` and a per-zone override. (Q7, changed on approval.)

gateway-console relays chat as `text/event-stream`
(`ts/apps/gateway-console/lib/chat-route.ts` line 214). Its own client waits at most 35 s for the
upstream response headers (`CHAT_HEADER_TIMEOUT_MS`, line 25), and it sets no bound on the stream
length. Envoy's default route timeout is 15 s, and Cilium's Gateway API uses Envoy. Sven decided to
expose the Gateway API v1 `HTTPRouteTimeouts` (`request`, `backendRequest`) on each rule, with a
longer default for the gateway-console route, so that a chat stream longer than 15 s does not
break.

The values:

- `httpRoute.timeouts: {}`: a map with the optional keys `request` and `backendRequest`. It
  applies to the rule of EVERY enabled zone.
- `zones.<id>.console.httpRouteTimeouts`: the same shape, for one zone. A non-empty key here wins
  over the same key in `httpRoute.timeouts`. The merge is per key.
- The default in `values.yaml`: `zones.gateway.console.httpRouteTimeouts: {request: 10m}`. The iam
  zone has no default, so its rule gets the implementation default. No default exists for
  `backendRequest`.

Why the per-zone value lives under `zones` and not under `httpRoute`: the `values.yaml` header
rule is "ONE values block", and every projection derives from `zones`. A zone-keyed map under
`httpRoute` (for example `httpRoute.timeouts.gateway`) would be a second place that names a zone
id. It could name a zone that `zones` does not have, and it would need its own refusal for that.
Under `zones`, a disabled zone's value is never read, so it leaves no trace (AC3). This is an
agent decision made while unattended. Q9 lets Sven reverse it.

Why `10m` and not `0s`: Gateway API reads `0s` as "no timeout". A route with no bound keeps a hung
upstream connection open without limit. Ten minutes is far longer than a normal chat stream and
still bounds a hung connection. The operator can change it. The value is a Gateway API
`Duration` (GEP-2257), so `10m` is a valid form.

The rendering: after the merge, a rule gets `timeouts:` with only the non-empty keys. If both keys
are empty, the rule has no `timeouts` key. So a route-mode render with no timeout set is equal to
the design before this decision (AC9).

The validation is fail-closed, as for the rest of the block (§ 4.4, checks 5 to 8). It mirrors the
HTTPRoute v1 CRD rules: the value form `^([0-9]{1,5}(h|m|s|ms)){1,4}$`, and `backendRequest` not
longer than `request`, except when `request` is `0s`. The chart refuses these before the API server
does, because a refusal at apply time in Argo CD is a sync error that is easy to miss.

Support residual: `timeouts` is a Gateway API "Extended" feature, in the Standard channel since
Gateway API v1.2.0. Whether the Cilium version on the target cluster supports it is not measured.
An implementation that does not support it can refuse the route (`Accepted: False`). The manual
verification (§ 5.4) checks `Accepted` and a chat stream longer than 15 s.

### D14. `httpRoute.annotations`: one generic map for each HTTPRoute. (Q8, changed on approval.)

`httpRoute.annotations` is a map, empty by default. When it is not empty, each HTTPRoute renders
it as `metadata.annotations` with `toYaml`, as `ingress.yaml` does for `ingress.annotations`. Use
cases are external-dns and similar controllers. The chart refuses a value that is not a map, and
an annotation value that is not a string (the API server needs strings, and
`--set httpRoute.annotations.x=true` gives a boolean). It does not check annotation key form: the
API server does that.

A rendered annotation value could contain a zone id (for example `gateway`). The chart cannot
stop that, and it is the operator's own text. The H2 row uses the default (empty) annotations, and
the docs state this.

## 4. The design

### 4.1 Values

A new top-level block in `values.yaml`, after the `ingress` block (today lines 104-117):

```yaml
httpRoute:
  # NOT required. true renders one gateway.networking.k8s.io/v1 HTTPRoute for each enabled zone,
  # from the same zones range as the Ingress (SMA-694). It must be a boolean.
  # Independent of ingress.enabled: both can be on during a cut-over. See RUNBOOK-chart.md § 11.
  # The route host is always ingress.host.
  enabled: false
  # REQUIRED when enabled. Gateway API ParentReference objects, rendered as written.
  # Each entry needs a name and a sectionName or port that selects the HTTPS listener.
  # Example: [{name: cilium-gateway, namespace: kube-system, sectionName: https}]
  # The listener must end TLS for ingress.host and allow this namespace.
  parentRefs: []
  # Optional. Gateway API HTTPRouteTimeouts for the rule of every enabled zone: request and
  # backendRequest, each a duration such as 30s, 10m or 1h. A key in
  # zones.<id>.console.httpRouteTimeouts wins over the same key here.
  timeouts: {}
  # Optional. Annotations on each HTTPRoute, for example for external-dns. Values are strings.
  annotations: {}
  # DO NOT add a rewrite. Each console serves its full basePath.
```

And one new key in the gateway zone's `console` block (today lines 73-79):

```yaml
      # Used only when httpRoute.enabled is true. The chat relay streams text/event-stream for
      # longer than the 15 s default route timeout of Envoy. See RUNBOOK-chart.md § 11.
      httpRouteTimeouts:
        request: 10m
```

The iam zone's `console` block gets no such key. The `values.yaml` header (lines 3-9) changes: the
HTTPRoute path rules are a projection of `zones` when `httpRoute.enabled` is true, and the SMA-695
exception applies only when both `ingress.enabled` and `httpRoute.enabled` are false.

Measured on the pinned helm 3.22.0 (2026-09-27, prototype in a scratch copy of the chart):
`.Values.httpRoute | default dict` followed by `dig "enabled" false` renders no HTTPRoute for the
default values and for `--set httpRoute=null`. `--set-string httpRoute.enabled=false` renders
the HTTPRoutes, because a non-empty string is true in `if`. § 4.4 refuses that case. The prototype
did not have `timeouts` or `annotations`; the plan measures them.

### 4.2 Helpers in `templates/_httproute.tpl`

Every caller reads the block with the idiom `$h := .Values.httpRoute | default dict`. Reason: `dig`
needs a map, and under `--reuse-values` a release made before this change has no `httpRoute` map
(the `charts/CLAUDE.md` `dig` rule). The validator checks that `$h` is a map before any `dig`. The
per-zone value is read with `dig "console" "httpRouteTimeouts" dict $z` for the same reason: a
release made before this change has no such key.

- `paigasus.httpRouteEnabled`: yields `true` or an empty string, as
  `paigasus.ingressEnabled` does (`_helpers.tpl` lines 52-55). Nil counts as not set, which is
  `false` here.
- `paigasus.consoleServicePort`: yields `3000` (D6). `console-service.yaml` uses it too.
- `paigasus.httpRouteTimeouts`: takes `(list $root $z)` and yields the merged timeouts map (D13)
  as YAML, or an empty string when both keys are empty.
- `paigasus.durationMs`: takes a Gateway API duration string and yields its value in
  milliseconds, for the `backendRequest` check.
- `paigasus.validateHttpRoute`: the checks in § 4.4.

### 4.3 Template `templates/httproute.yaml`

- Line 1 the SPDX header, line 2 `{{- include "paigasus.validate" . -}}`, outside the condition,
  as in every template.
- `{{- if (include "paigasus.httpRouteEnabled" .) }}` wraps a `range $id, $z := .Values.zones`
  with `if $z.enabled`, as in `ingress.yaml`. Each iteration emits `---` and one HTTPRoute:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: <paigasus.name (list $root "<id>-console")>
  annotations: <toYaml httpRoute.annotations>   # only when not empty (D14)
spec:
  parentRefs: <toYaml parentRefs>
  hostnames:
    - <ingress.host>
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: <basePath>
      backendRefs:
        - name: <paigasus.name (list $root "<id>-console")>
          port: <paigasus.consoleServicePort>
      timeouts: <paigasus.httpRouteTimeouts>   # only when not empty (D13)
```

- Every YAML comment stays inside the `if` block, and a comment that names a zone is written per
  zone (the `ingress.yaml` pattern), so an iam-only render leaks no zone id.
- No comment in `httproute.yaml` contains the word "gateway" in any case. Write "the route's
  parent" or "the listener", not "Gateway API". Reason: the H2 absent-grep and a future
  `helm_render.py` check 2 subset both match that word (D10, § 5.1). The same applies to the
  timeouts comment: write "the chat relay", not "gateway-console".
- `templates/console-service.yaml` line 18 `port: 3000` becomes
  `port: {{ include "paigasus.consoleServicePort" $root }}`. The render is equal.

### 4.4 Validation in `paigasus.validateHttpRoute`

The kind checks (1 and 2) run in both modes. The other checks run only when
`paigasus.httpRouteEnabled` is true. The timeouts checks (5 to 8) run for `httpRoute.timeouts` and
for the `httpRouteTimeouts` of each ENABLED zone. An ABSENT key counts as its empty value: no
check is skipped because a key is missing. This matters under `helm upgrade --reuse-values`, where
Helm uses the OLD chart's defaults, so a release made before this change has no `parentRefs` key.
`parentRefs` is optional in the HTTPRoute v1 CRD, so without this rule the API server accepts a
route that attaches to no Gateway, and the consoles are silently not reachable.

Each message is unique and carries its key path, so a refusal row can match it without matching
another message (the trap that `tests/refusals.sh` lines 113-115 describe). `<path>` below is
`httpRoute.timeouts` or `zones.<id>.console.httpRouteTimeouts`.

1. **Kind of `httpRoute`.** When `.Values.httpRoute` is neither nil nor a map: fail
   `httpRoute must be a map, got <kind>`.
2. **Kind of `enabled`.** When `dig "enabled" false $h` is neither `bool` nor nil: fail
   `httpRoute.enabled must be true or false (a boolean), got <kind> <value>`.
3. When `paigasus.httpRouteEnabled` is true:
   - **`parentRefs` absent, null, empty, or not a list:** fail `httpRoute.parentRefs is required
     when httpRoute.enabled is true: a non-empty list that names the listener serving
     ingress.host`.
   - **An entry that is not a map, or has no non-empty string `name`:** fail
     `httpRoute.parentRefs[<i>] must be a map with a non-empty name`.
   - **An entry with neither `sectionName` nor `port`:** fail `httpRoute.parentRefs[<i>] must set
     sectionName or port to select the HTTPS listener`.
   - **`ingress.host` not lowercase:** fail `ingress.host must be lowercase when
     httpRoute.enabled is true (Gateway API hostnames are lowercase RFC 1123 names)`.
4. **`annotations` (D14), when enabled:** not nil and not a map: fail `httpRoute.annotations must
   be a map, got <kind>`. A value that is not a string: fail `httpRoute.annotations.<key> must be a
   string, got <kind>`.
5. **Kind of a timeouts map (D13), when enabled:** not nil and not a map: fail `<path> must be a
   map with the optional keys request and backendRequest, got <kind>`.
6. **An unknown key:** fail `<path> has the unknown key <key>; the allowed keys are request and
   backendRequest`. This catches `requestTimeout` and similar typos, which would render nothing.
7. **A value form:** a non-empty value that is not a string, or does not match
   `^([0-9]{1,5}(h|m|s|ms)){1,4}$`: fail `<path>.<key> must be a duration such as 30s, 10m or 1h,
   got <kind> <value>`. An empty string is "not set".
8. **The order, on the merged map of each enabled zone:** `backendRequest` longer than `request`,
   with `request` not `0s`: fail `zones.<id>: the HTTPRoute backendRequest timeout <b> is longer
   than the request timeout <r>`.

The existing `ingress.host` refusals (required, bare host form, both from SMA-695,
`_helpers.tpl` lines 140-147) run first, because `paigasus.validate` calls
`paigasus.validateHttpRoute` last. So the route always renders a valid host.

### 4.5 What does not change

- `templates/ingress.yaml` and its fixture `ci/helm-render/fixtures/literal-ingress/`.
- `templates/console-deployment.yaml` and its two fixtures. The Service's `targetPort: http`
  refers to the container port by name, so the HTTPRoute does not depend on the container port
  number. A helper for the container port is out of scope.
- `ci/helm-render/helm_render.py`, `ci/helm-render/run.sh`, `ci/affected-graph/ci_targets.py`.
- `ci/kind/` (AC6).
- `Chart.yaml` `version`: the chart is not published (the SMA-695 spec § 8, lines 296-297,
  records the evidence).

### 4.6 Fixtures that copy an edited file

`templates/_helpers.tpl` changes by one line (the `paigasus.validateHttpRoute` include). The
`charts/CLAUDE.md` fixture rule still applies: re-sync its two whole-file copies, `slug-mirror` and
`zones-omits-enabled`, in the same commit, and keep each mutation. A stale copy no longer makes
the control INCONCLUSIVE, because the helpers are in the live `_httproute.tpl` (D12). Run
`bash ci/helm-render/run.sh --negative-control` and confirm that each fixture still fails its
named row.

`templates/console-service.yaml` and `values.yaml` have no fixture copy.

## 5. The test strategy

All rows go into existing scripts. They use process substitution, not a pipe into `grep -q`
(actionlint check 13), and keep the `"${X[@]+...}"` guards (the scripts also run under bash 3.2).

### 5.1 Rows

`tests/ingress.sh`, new function `route_coupling`, called for both zone subsets with
`--set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].name=gw'
--set 'httpRoute.parentRefs[0].sectionName=https'`. The script today has three functions:
`coupling` (line 28), `no_ingress` (line 68) and `ingress_count` (line 103). `route_coupling` is
a fourth, next to them.

- H1 (AC1, AC2): the set of HTTPRoute `rules[0].matches[0].path.value` equals the set of
  `PAIGASUS_ZONES` values; there is one HTTPRoute per enabled zone; each path type is `PathPrefix`;
  each `backendRefs[0]` names a rendered Service whose selected Deployment has
  `PAIGASUS_ZONE` equal to the zone, and whose port list has that port number; no rule has
  `filters`; `hostnames` equals `[console.example.test]`.
- H2 (AC3): in the iam-only render, `gateway` does not appear. `route_coupling` first deletes
  every line that matches `^apiVersion: gateway\.networking\.k8s\.io/`, and then runs the absent
  grep with the needle `gateway`. Reason: the existing `coupling()` greps the raw render
  (`tests/ingress.sh` lines 53-58), and every HTTPRoute renders that `apiVersion`. Without the
  filter H2 is red on every run, and the mutations that depend on it prove nothing. Do NOT weaken
  the needle to make it pass. On the prototype, measure H2 green, then red with `if $z.enabled`
  removed.
- H3 (D3, AC7): with `--set ingress.enabled=false --set ingress.tlsSecretName=""` added, the
  render has no Ingress and H1 still holds.
- H4 (AC5): with `--set httpRoute=null`, the render has no HTTPRoute.
- H5 (AC9, default timeouts): in the iam-and-gateway render with the default values, the gateway
  route's rule has `timeouts` equal to `{request: 10m}`, and the iam route's rule has no `timeouts`
  key. Parse the YAML; do not grep for `10m`.
- H6 (AC9, merge): with `--set httpRoute.timeouts.request=30s --set
  httpRoute.timeouts.backendRequest=20s`, the iam rule has `{request: 30s, backendRequest: 20s}`
  and the gateway rule has `{request: 10m, backendRequest: 20s}` (the zone key wins per key).
- H7 (AC9, none): with `--set zones.gateway.console.httpRouteTimeouts=null`, no rule has a
  `timeouts` key.
- H8 (AC10): with `--set httpRoute.annotations.example\.test/owner=team`, each HTTPRoute has
  `metadata.annotations` equal to `{example.test/owner: team}`. With the default values, no
  HTTPRoute has an `annotations` key.

`tests/refusals.sh` (needles carry the key path, so each is unique). `expect_fail` is at line 34
and `expect_render` at line 45:

- R1 `expect_fail "httpRoute.enabled a string" "httpRoute.enabled must be true or false"`:
  `--set-string httpRoute.enabled=false`.
- R2 `expect_fail "httpRoute without parentRefs" "httpRoute.parentRefs is required"`:
  `--set httpRoute.enabled=true`.
- R2b `expect_fail "httpRoute parentRefs null" "httpRoute.parentRefs is required"`:
  `--set httpRoute.enabled=true --set httpRoute.parentRefs=null` (the `--reuse-values` case, AC5).
- R2c `expect_fail "httpRoute parentRefs a map" "httpRoute.parentRefs is required"`:
  `--set httpRoute.enabled=true --set httpRoute.parentRefs.name=gw`. Not measured: Helm may
  merge a map over the default list with only a warning, or refuse it earlier. Measure on the
  prototype. If `--set` cannot make a map there, pass a small values file with
  `parentRefs: {name: gw}`.
- R3 `expect_fail "httpRoute parentRef without name" "httpRoute.parentRefs[0] must be a map with a non-empty name"`:
  `--set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].sectionName=https'`.
- R4 `expect_fail "httpRoute parentRef without listener" "httpRoute.parentRefs[0] must set sectionName or port"`:
  `--set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].name=gw'`.
- R5 `expect_fail "httpRoute not a map" "httpRoute must be a map"`: `--set httpRoute=true`.
- R6 `expect_fail "httpRoute host not lowercase" "ingress.host must be lowercase when httpRoute.enabled"`:
  `--set ingress.host=Console.Example.test --set httpRoute.enabled=true` plus a valid parentRef.
- R7 `expect_render "httpRoute parentRef with port"`: a parentRef with `name` and `port=443`.

The rows below all add `--set httpRoute.enabled=true` and a valid parentRef:

- R8 `expect_fail "httpRoute annotations not a map" "httpRoute.annotations must be a map"`:
  `--set httpRoute.annotations=x`.
- R9 `expect_fail "httpRoute annotation not a string" "httpRoute.annotations.owner must be a string"`:
  `--set httpRoute.annotations.owner=true`.
- R10 `expect_fail "httpRoute timeouts not a map" "httpRoute.timeouts must be a map"`:
  `--set httpRoute.timeouts=10s`.
- R11 `expect_fail "httpRoute timeouts unknown key" "httpRoute.timeouts has the unknown key requestTimeout"`:
  `--set httpRoute.timeouts.requestTimeout=10s`.
- R12 `expect_fail "httpRoute timeout bad form" "httpRoute.timeouts.request must be a duration"`:
  `--set-string httpRoute.timeouts.request=10`. A bare number is not a Gateway API duration.
  (Plain `--set` gives an int64; the same refusal covers it through the kind part of check 7.)
- R13 `expect_fail "zone timeout bad form" "zones.gateway.console.httpRouteTimeouts.request must be a duration"`:
  `--set zones.gateway.enabled=true --set zones.gateway.console.httpRouteTimeouts.request=ten`.
- R14 `expect_fail "httpRoute backendRequest longer than request" "the HTTPRoute backendRequest timeout"`:
  `--set httpRoute.timeouts.request=10s --set httpRoute.timeouts.backendRequest=1m`.
- R15 `expect_render "httpRoute request 0s with backendRequest"`:
  `--set httpRoute.timeouts.request=0s --set httpRoute.timeouts.backendRequest=1m`.
- R16 `expect_render "zone timeout on a disabled zone is not checked"`:
  `--set zones.gateway.enabled=false --set zones.gateway.console.httpRouteTimeouts.request=ten`.
  A disabled zone leaves no trace, so its value is not read.

`tests/render.sh`: a third golden, `golden/iam-and-gateway-httproute.yaml`, rendered with
`--set zones.gateway.enabled=true --set ingress.enabled=false --set httpRoute.enabled=true`,
one parentRef with `name`, `namespace` and `sectionName`, `httpRoute.timeouts.backendRequest=30s`
and one annotation. It is a byte pin for the HTTPRoute shape, with the default gateway timeout.
The two existing goldens do not change (AC4). Create it with `render.sh --update`, then check with
`git diff` that only the new file changed.

`tests/names.sh`: add `--set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].name=gw'
--set 'httpRoute.parentRefs[0].sectionName=https'` to all four release rows (lines 72-75). The
52-character row is the case that caused the original name collision; the 53-character row is
Helm's ceiling.

### 5.2 Mutation record

Run before the PR. Each mutation must turn its row red and leave the two old goldens green, with
one exception: the `httpRouteEnabled` gate mutation turns the two old goldens red, and that IS the
proof. A mutation that breaks the template parse proves nothing, so remove an `if` together with
its `end`. Record the results in the PR description, in the form of the "Delete-the-feature
record" in `ci/helm-render/README.md`. This manual record is the accepted proof (Q3).

| Mutation | Row that must go red |
|---|---|
| Delete `templates/httproute.yaml` | H1, the new golden |
| Remove the `httpRouteEnabled` gate (always render) | the two old goldens, H4 |
| Write the `/iam` rule literally, outside the `range` | H1 or H2 |
| Drop `if $z.enabled` inside the range | H2 |
| Add a `URLRewrite` filter to the rule | H1 |
| Write a literal host in `hostnames` | H1 |
| Read `.Values.httpRoute.enabled` with no `default dict` | H4 |
| Delete the kind check of `httpRoute` | R5 |
| Delete the kind check of `enabled` | R1 |
| Skip the `parentRefs` check when the key is absent | R2b |
| Delete the `parentRefs` required check | R2, R2b, R2c |
| Delete the parentRef `name` check | R3 |
| Delete the `sectionName`/`port` check | R4 |
| Delete the lowercase check | R6 |
| Make the `sectionName`/`port` check require `sectionName` only | R7 |
| Restore the conditional `tlsSecretName` refusal to always fire | H3 |
| Replace the port helper with `3001` in `httproute.yaml` only | H1 |
| Give the HTTPRoute a fixed name (`paigasus-console`) | `names.sh` |
| Delete the `timeouts` block from the rule | H5, H6, the new golden |
| Let the chart-wide key win over the zone key | H6 |
| Render `timeouts: {}` when both keys are empty | H5 (iam rule), H7 |
| Delete the `10m` default from `values.yaml` | H5 |
| Delete the `annotations` block | H8, the new golden |
| Render `annotations: {}` when the map is empty | H8 |
| Delete the annotations kind check / string check | R8 / R9 |
| Delete the timeouts kind check / unknown-key check / form check | R10 / R11 / R12, R13 |
| Delete the `backendRequest` order check | R14 |
| Remove the `0s` exception from the order check | R15 |
| Check the timeouts of every zone, not only enabled zones | R16 |

### 5.3 Gates to run

`bash ci/helm-render/run.sh`, `--self-test` and `--negative-control` (all three, as
`moon run repo:helm-render` does), `repo:actionlint` (it scans the chart scripts), and
`repo:affected-smoke`. Per the root `CLAUDE.md`, the chart scripts run under system bash 3.2 and
under bash 5. Run the full `ci-targets` graph before the push.

### 5.4 Manual verification on the target cluster (Q1)

No new kind leg (Q1). The proof that the route works is a manual check on Sven's K3s + Cilium
cluster, done by Sven. The agent cannot reach that cluster. The PR body carries the list below as
an UNCHECKED checklist under the heading "Manual verification on the target cluster". Replace
`<ns>`, `<release>`, `<host>`, `<gw-ns>`, `<gw>` and `<gw-ip>` with the real values. Route names
come from `kubectl get httproute`, because `paigasus.name` shortens a long release name.

```bash
# 0. Prerequisites: the CRD serves v1, and the listener that the parentRef names exists.
kubectl get crd httproutes.gateway.networking.k8s.io \
  -o jsonpath='{.spec.versions[?(@.served==true)].name}{"\n"}'
kubectl -n <gw-ns> get gateway <gw> -o jsonpath='{range .spec.listeners[*]}{.name} {.protocol} {.port}{"\n"}{end}'
# 1. Server-side dry run of the chart objects with the route values.
helm template <release> charts/paigasus -n <ns> -f <values.yaml> \
  --set httpRoute.enabled=true | kubectl apply -n <ns> --dry-run=server -f -
# 2. After the sync: one HTTPRoute per enabled zone, each Accepted and ResolvedRefs.
kubectl -n <ns> get httproute
kubectl -n <ns> get httproute -o jsonpath='{range .items[*]}{.metadata.name}{" "}{range .status.parents[*].conditions[*]}{.type}={.status}{" "}{end}{"\n"}{end}'
# 3. Each zone answers through the Gateway address over HTTPS.
curl -sS -o /dev/null -w '%{http_code}\n' --resolve <host>:443:<gw-ip> https://<host>/iam/
curl -sS -o /dev/null -w '%{http_code}\n' --resolve <host>:443:<gw-ip> https://<host>/gateway/
# 4. Plain HTTP redirects to HTTPS and does not serve the console.
curl -sS -o /dev/null -w '%{http_code} %{redirect_url}\n' --resolve <host>:80:<gw-ip> http://<host>/iam/
# 5. Login works end to end in a browser at https://<host>/iam/ (the __Host-pgs_sid cookie is set).
# 6. A gateway-console chat that streams for more than 15 s completes (the 10m route timeout, D13).
#    Ask for a long answer in https://<host>/gateway/ and watch the stream to its end.
# 7. The rendered timeouts on the gateway route:
kubectl -n <ns> get httproute -o jsonpath='{range .items[*]}{.metadata.name}{" "}{.spec.rules[0].timeouts}{"\n"}{end}'
# 8. Argo CD health of the HTTPRoute objects (Q4 residual: record what Argo CD shows).
kubectl -n argocd get application <app> -o jsonpath='{range .status.resources[?(@.kind=="HTTPRoute")]}{.name} {.health.status}{"\n"}{end}'
```

The checklist items: (a) prerequisites present; (b) server-side dry run clean; (c) every route
`Accepted=True` and `ResolvedRefs=True`; (d) both zones answer over HTTPS; (e) HTTP redirects;
(f) login works; (g) a chat stream longer than 15 s completes; (h) the gateway route has
`timeouts.request: 10m`; (i) the Argo CD health status of each HTTPRoute is recorded.

## 6. The docs

- `charts/paigasus/values.yaml`: the new block and the new zone key (§ 4.1). The header
  (lines 3-9: "Six projections" and the SMA-695 exception) names the HTTPRoute path rules as a
  projection when `httpRoute.enabled` is true. The `ingress.enabled` comment (lines 105-108)
  points to `httpRoute.enabled` as the chart-owned route.
- `charts/CLAUDE.md`: the "three projections" bullet includes the HTTPRoute rules. It says today
  that "check 1 and check 2 fail on" a literal zone. That is false for `httproute.yaml` (D10): name
  `tests/ingress.sh` H1 and H2 there. Its SMA-695 sentence ("the operator keeps the route by
  hand") gets "unless `httpRoute.enabled` is true". The rewrite bullet names
  `templates/httproute.yaml` too. The fixture bullet notes that `_httproute.tpl` has no fixture
  copy. A new bullet: read `httpRoute.enabled` only through `paigasus.httpRouteEnabled` (the same
  reason as the `paigasus.ingressEnabled` bullet).
- `charts/paigasus/README.md`:
  - "One values block, six projections" (lines 8-31): the projection list gets
    `templates/httproute.yaml`. The SMA-695 exception paragraph (lines 26-31, which ends "SMA-694
    ... will restore D6 for Gateway API") says that D6 holds again with `httpRoute.enabled`.
  - "The refusals" (from line 33) lists the new refusals of § 4.4 (two kind checks, the
    `parentRefs` required check, two parentRef entry checks, the lowercase check, the two
    annotations checks and the four timeouts checks), each with its message.
  - "The golden files" (lines 235-248) names three goldens, and says that `--update` re-baselines
    all three.
  - "Running without an Ingress controller" (lines 90-99) gets a Gateway API part. It states the
    prerequisites: Gateway API v1 CRDs (standard channel, `HTTPRoute` at `v1`, v1.2.0 or later for
    `timeouts`); a Gateway with an HTTPS listener that ends TLS for `ingress.host` and allows the
    release namespace; an HTTP-to-HTTPS `RequestRedirect` route on any HTTP listener (D11); create
    permission for the deployer on `gateway.networking.k8s.io/httproutes` (the standard Gateway
    API install adds no aggregated rules to the `admin` and `edit` ClusterRoles); and an Argo CD
    AppProject resource whitelist that allows the kind. It replaces "route the traffic yourself"
    with "set `httpRoute.enabled`", and keeps the hand-written route as the other option. It
    describes `httpRoute.timeouts`, the gateway zone's `10m` default (D13), and
    `httpRoute.annotations` (D14).
- `docs/ops/RUNBOOK-chart.md`:
  - § 1 gets the four new rows: `httpRoute.enabled`, `httpRoute.parentRefs`,
    `httpRoute.timeouts`, `httpRoute.annotations`, plus `zones.<id>.console.httpRouteTimeouts`.
    The `ingress.enabled` row points to `httpRoute.enabled`.
  - § 2 "Refused combinations" gets the new refusals. Its intro names `templates/_httproute.tpl`
    as a third place.
  - § 4 "One values block, six projections": the paragraph "D6 and `ingress.enabled: false`"
    (lines 85-88) gets its route-mode case. D6 holds for the HTTPRoute. It also states the prune
    rule: disabling a zone DELETES that zone's HTTPRoute (the Ingress path only modifies one
    object). Without Argo CD prune, the zone stays routed while `PAIGASUS_ZONES` drops it, which
    is the state D6 forbids. Enable prune, or delete the route by hand.
  - § 8 "The kind job" (lines 421-427) names Gateway API as the other supported path, and says
    that the kind job does not test it (Q1).
  - § 11 "Running without an Ingress controller" (line 515): a new part "The chart-owned
    HTTPRoute (SMA-694)" before the routing contract. The routing contract stays, for an operator
    who writes the route by hand. The line "SMA-694 will add a chart-owned Gateway API HTTPRoute."
    (line 549) is removed.
  - § 11 cut-over procedure: step 1 "Create the new route and verify it" becomes "set
    `httpRoute.enabled: true` and verify the route". Keep the cert-manager text of step 1. Verify
    with `status.parents` (`Accepted` and `ResolvedRefs` true) AND with a request through the
    Gateway address, for example `curl --resolve <host>:443:<gateway-ip> https://<host>/iam/`.
  - A new procedure in § 11: migration from a hand-written route (the § 11 routing contract) to
    the chart route. 1. Check for a hand-written HTTPRoute with the D4 name; if one exists, delete
    it first or rename it, else `helm upgrade` fails with "invalid ownership metadata". 2. Set
    `httpRoute.enabled: true`. 3. Delete the hand-written route. Two routes with the same host and
    the same `PathPrefix` tie, and Gateway API gives the tie to the OLDEST route, so the chart
    route shows `Accepted` but carries no traffic while the old route exists. 4. Verify with a
    request through the Gateway address, not only with `status.parents`.
  - A verify step for chat streaming in route mode: send a gateway-console chat that streams for
    more than 15 seconds. A new part "Route timeouts" explains D13: the Envoy 15 s default, the
    `10m` gateway default, the per-key merge, `0s` as "no timeout", and the `timeouts` support
    residual (D13).
  - A residual note for Q4: the Argo CD version on the target cluster is not known. Some Argo CD
    versions have no health check for HTTPRoute. Then Argo CD shows each HTTPRoute as Healthy at
    once, and a route that the Gateway refuses does not show as degraded. Versions with a check
    can show a route that no Gateway reports on (a wrong parentRef) as Progressing forever. Check
    `status.parents` by hand in both cases (the § 11 verify step).
  - A note for D14: an annotation value is operator text, and the chart does not stop a zone id
    in it.
- The PR body: the unchecked checklist "Manual verification on the target cluster" (§ 5.4) and the
  mutation record (§ 5.2).

## 7. Files expected to change

- `charts/paigasus/values.yaml`
- `charts/paigasus/templates/httproute.yaml` (new)
- `charts/paigasus/templates/_httproute.tpl` (new)
- `charts/paigasus/templates/_helpers.tpl` (one include line)
- `charts/paigasus/templates/console-service.yaml`
- `charts/paigasus/tests/ingress.sh`, `refusals.sh`, `render.sh`, `names.sh`
- `charts/paigasus/tests/golden/iam-and-gateway-httproute.yaml` (new)
- `ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl`
- `ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl`
- `charts/paigasus/README.md`, `charts/CLAUDE.md`, `docs/ops/RUNBOOK-chart.md`

Estimated size: medium. About 250 lines of template and test code, plus docs. The timeouts and
annotations values (D13, D14) add about 100 lines to the first estimate of 150.

## 8. Out of scope

- A kind CI leg that installs Gateway API CRDs and routes through a Gateway (Q1: the proof is the
  manual check in § 5.4).
- `httpRoute.hostnames` (D2, Q2), `httpRoute.labels`, or any `filters` value.
- An `HTTPRoute` subset and a negative-control fixture in `helm_render.py` (Q3).
- Timeouts other than `request` and `backendRequest`, and a retry policy.
- A `GRPCRoute` or a route to a `*-backend` Service. Only consoles are public.
- A neutral `publicHost` key and a public origin with a port (SMA-695 D1).
- A `.Capabilities` gate (D8).

## 9. Open questions

- **Q1. ANSWERED (2026-09-27).** Must the kind job also prove the HTTPRoute end to end? The issue
  asks only that the kind job keeps passing. The job uses Traefik (`ci/kind/traefik-values.yaml`),
  and Traefik can serve Gateway API, so a second leg is possible. It adds CRD installs and CI time.
  **Answer: no new kind leg. The proof is a manual check on Sven's K3s + Cilium cluster, done by
  Sven. The agent cannot reach that cluster. The PR body carries an unchecked checklist "Manual
  verification on the target cluster" with the exact commands (§ 5.4).**
- **Q2. ANSWERED (2026-09-27).** The issue proposes `httpRoute.hostnames`. This spec drops it (D2).
  **Answer: Sven accepts the drop.**
- **Q3. ANSWERED (2026-09-27).** Is a manual mutation record enough for the route rows (D10), or
  must `helm_render.py` get an HTTPRoute render subset and a `literal-httproute` negative-control
  fixture? **Answer: the manual mutation record is enough. No `helm_render.py` subset.**
- **Q4. ANSWERED (2026-09-27), as a residual.** Does the Argo CD version on the target cluster
  have a health check for HTTPRoute? **Answer: the Argo CD version is not known. The RUNBOOK
  documents the health-check behaviour as a residual (§ 6), and the manual check records what
  Argo CD shows (§ 5.4, item i).**
- **Q5. ANSWERED (2026-09-27).** SMA-695 is merged (`4051df5e`). The helper name
  (`paigasus.ingressEnabled`) and the host-form check (no `/`, no `:`) match its spec. This spec
  is checked against the merged code (§ 12).
- **Q6. ANSWERED (2026-09-27).** D11 refuses a parentRef without `sectionName` or `port`.
  **Answer: Sven accepts the refusal. No NOTES warning.**
- **Q7. ANSWERED (2026-09-27), CHANGED.** gateway-console relays chat as `text/event-stream`, and
  Envoy's default route timeout is 15 s. **Answer: expose an optional `httpRoute.timeouts` value
  (`request`, `backendRequest`; Gateway API v1 HTTPRouteTimeouts), rendered on each route rule,
  with a longer default for the gateway-console route. Validate the values fail-closed. Document
  it in the RUNBOOK and `values.yaml`.** See D13, AC9, § 4.4 checks 5-8, rows H5-H7 and R10-R16.
  Whether Cilium sets another default for a route with no `timeouts` is still not measured; the
  gateway route now sets its own value, so it does not depend on that.
- **Q8. ANSWERED (2026-09-27), CHANGED.** Does the target cluster need annotations on the route?
  **Answer: add a generic `httpRoute.annotations` map, empty by default, rendered on each
  HTTPRoute.** See D14, AC10, § 4.4 check 4, rows H8, R8 and R9.
- **Q9. NEW, decided by the agent without Sven.** Where does the longer gateway-console default
  (Q7) live? The approval names `httpRoute.timeouts`, and a per-zone default needs a per-zone key.
  This spec keeps `httpRoute.timeouts` as the chart-wide value and puts the per-zone override and
  the `10m` default under `zones.<id>.console.httpRouteTimeouts` (D13, with the reason). The
  alternative is a zone-keyed map `httpRoute.timeouts.<zone>`. Sven can reverse this choice at plan
  review. The `10m` value is also an agent choice (D13).

## 10. Challenge changelog

Spec-challenger verdict: **APPROVE WITH CHANGES**. Each finding was checked against the repo.

Folded:

- BLOCKER H2: confirmed. `coupling()` greps the raw render for `gateway` (`tests/ingress.sh`
  lines 53-58), and `apiVersion: gateway.networking.k8s.io/v1` matches it. H2 now deletes the
  `apiVersion` lines first and keeps the needle. § 4.3 bans the word in `httproute.yaml`
  comments. The `helm_render.py` line 430 note is in D10.
- MAJOR preamble: the "only when the key exists" rule is deleted. An absent key counts as empty.
  AC5 and rows R2b and R2c are added.
- MAJOR `httpRoute.hostnames`: the value is removed (D2), with the reason. Q2 asked Sven to
  accept the drop. This also removes the `fromJsonArray` MINOR and old rows R4-R6.
- MAJOR TLS: new D11. A parentRef must set `sectionName` or `port`. Docs require an HTTPS listener
  and an HTTP-to-HTTPS redirect. Q6 recorded the refusal-or-warning choice.
- MAJOR migration from a hand-written route: new RUNBOOK procedure in § 6, with the name check, the
  oldest-route tie rule and a `curl --resolve` verify. D4 names the ownership risk.
- MINOR AC8 rows: `httpRoute` kind check and rows R2c, R5 added.
- MINOR needles: every needle now carries its key path.
- MINOR mutation gaps: rows added for the gate (with the golden exception stated), R3, R4, R5, R6,
  R7, H3 and `names.sh`.
- MINOR helper placement: new D12, `templates/_httproute.tpl`. Confirmed at `run.sh` line 216 that
  the control overlays each fixture on a copy of the live chart.
- MINOR D6: narrowed. 3000 also appears in `console-deployment.yaml` lines 54 and 59 and
  `helm_render.py` lines 1027, 1044, 1047 (confirmed).
- MINOR doc gaps: refusal count fixed, RUNBOOK § 2, README "The golden files" and the
  `charts/CLAUDE.md` check 1/2 claim added to § 6.
- MINOR Argo CD prune, RBAC and AppProject prerequisites: added to § 6.
- MINOR host form: lowercase refusal in route mode (D2, § 4.4, R6).
- MINOR `names.sh`: the route flags go on all four rows, the 52-character row included (confirmed
  at `tests/names.sh` line 74).
- MINOR § 4.2 helper list: the non-define entry is removed; the idiom is stated once.
- QUESTIONS: D4 is confirmed against the literal issue text. SSE timeout became Q7 plus a RUNBOOK
  verify step. The Argo CD version joined Q4. The Q1 proof gap now has a proposal. Annotations
  became Q8.

Rejected:

- None in full. One partial choice: for the TLS finding the spec picks a refusal over a NOTES
  warning, because the Ingress path also fails closed on missing TLS. Q6 let Sven reverse it; he
  did not.

## 11. Approval decisions (Sven, 2026-09-27)

- SMA-695 is merged on `origin/main`. The design is rebased on its real code (§ 12).
- Q1: manual check on Sven's K3s + Cilium cluster, by Sven. An unchecked checklist with the exact
  commands goes into the PR body (§ 5.4). No new kind leg.
- Q2: `httpRoute.hostnames` is dropped (accepted).
- Q3: a manual mutation record is enough. No `helm_render.py` HTTPRoute subset.
- Q4: the Argo CD version is unknown. The RUNBOOK documents the health-check behaviour as a
  residual.
- Q6: refuse a parentRef without `sectionName` or `port` (accepted).
- Q7 CHANGED: an optional `httpRoute.timeouts` value, rendered on each rule, with a longer default
  for the gateway-console route, validated fail-closed, documented in the RUNBOOK and
  `values.yaml` (D13).
- Q8 CHANGED: a generic `httpRoute.annotations` map, empty by default, rendered on each HTTPRoute
  (D14).

The scope grew with Q7 and Q8. The Linear issue SMA-694 has a "Scope extended on 2026-09-27"
section.

## 12. Check against `origin/main` `4051df5e` (2026-09-27)

The spec was first written against `ad1c44b7`, before SMA-695 merged. These points changed:

- Header, § 1, D1: SMA-695 is merged (`4051df5e`, PR #332), not In Progress. § 1 names the three
  SMA-695 forward references to SMA-694.
- "SMA-695 § 7 contract" / "SMA-695 § 7, item 3" (D4, D7, § 6): the routing contract is in
  `docs/ops/RUNBOOK-chart.md` § 11 on `main`. Section 7 of the SMA-695 spec is its docs list.
- D3: the cut-over procedure is quoted from RUNBOOK § 11, which has three steps, not two.
- D4: `tests/names.sh` checks name uniqueness at line 59, not line 57.
- D12: the README "line 174" reference to `_audience.tpl` is removed. That line no longer holds
  the claim; the `_iam-backend.tpl` precedent is enough. The include goes after line 150.
- § 4.1: `values.yaml` header is now lines 3-9 (SMA-695 added the exception at lines 8-9). The
  `ingress` block is lines 104-117.
- § 4.4: the `tests/refusals.sh` needle trap is at lines 113-115, not 87-89. The SMA-695 host
  checks are at `_helpers.tpl` lines 140-147.
- § 5.1: `tests/ingress.sh` now also has `no_ingress` and `ingress_count` (SMA-695).
- § 6: README "The golden files" is at lines 235-248, not 212-225. README already has a "Running
  without an Ingress controller" section (lines 90-99) and a D6 exception paragraph
  (lines 26-31); the spec now edits those instead of adding a new "Gateway API" section. RUNBOOK
  § 4 already has a "D6 and `ingress.enabled: false`" paragraph. The "ingress-controller section
  (around line 417)" is RUNBOOK § 8 "The kind job" (lines 421-427). RUNBOOK § 11 line 549 holds
  the SMA-694 forward reference to remove.
- Unchanged and confirmed: `console-env-configmap.yaml` line 8; `console-service.yaml` line 18;
  `console-deployment.yaml` lines 54 and 59; `helm_render.py` lines 430, 1027, 1044, 1047;
  `tests/ingress.sh` lines 53-58; `tests/names.sh` lines 72-75; `run.sh` line 216;
  `chat-route.ts` line 214; the SMA-695 spec § 8 evidence (lines 296-297).
