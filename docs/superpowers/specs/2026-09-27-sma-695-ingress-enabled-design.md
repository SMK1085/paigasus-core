# SMA-695 — Helm chart: allow the Ingress to be disabled (`ingress.enabled`)

- Linear: [SMA-695](https://linear.app/smaschek/issue/SMA-695)
- Related: SMA-694 (Gateway API HTTPRoute), SMA-697
- Chart: `charts/paigasus`, version 0.1.0

## 1. Problem

The chart always renders one `networking.k8s.io/v1` Ingress (`templates/ingress.yaml`).
`paigasus.validate` in `templates/_helpers.tpl` refuses a render when `ingress.tlsSecretName` or
`ingress.host` is empty.

The install target is a K3s cluster with Cilium Gateway API and no Ingress controller. No
controller processes the Ingress, so the Ingress never gets a load-balancer status. Argo CD then
reports the Ingress, and the whole Application, as Progressing forever.

## 2. Goal

An operator can set `ingress.enabled: false`. The chart then renders no Ingress and does not
require `ingress.tlsSecretName`. The operator routes traffic to the console Services with their
own mechanism (for example a Gateway API HTTPRoute that they write, until SMA-694 lands).

Success criteria:

1. `ingress.enabled` defaults to `true`. The default render is byte-identical to the render
   before this change. The golden files in `charts/paigasus/tests/golden/` do not change.
2. With `ingress.enabled: false`, the render contains no object of kind `Ingress`, and it still
   contains each enabled zone's console Service with port name `http`, port 3000.
3. With `ingress.enabled: false`, an empty `ingress.tlsSecretName` renders.
4. With `ingress.enabled: false`, an empty `ingress.host` is still refused.
5. A release that has no `ingress.enabled` key (a release made before this change, upgraded with
   `--reuse-values`) still renders the Ingress.
6. A non-boolean `ingress.enabled` (for example the string `"false"`) is refused with its own
   message.
7. An `ingress.host` that holds a scheme, a path or a port is refused with its own message.

## 3. Decisions

### D1 — `ingress.host` stays required

The issue proposes not to require `ingress.host` when the Ingress is off. This spec does not do
that. Sven chose this on 2026-09-27.

`ingress.host` is not only the Ingress host. `templates/console-env-configmap.yaml` line 8 sets
`PAIGASUS_PUBLIC_ORIGIN` to `https://<ingress.host>`. `ts/packages/paigasus-auth/src/config.ts`
line 72 parses that value as an https URL, and `runtime.ts` lines 157-158 build the OIDC redirect
URI and the post-logout redirect URI from it. An empty host renders `https://`, which fails that
parse. The consoles read the configuration at the first request, and the readiness probe
(`<basePath>/healthz`) is such a request. The liveness probe is TCP. So the pods run, but they never
become Ready and serve no route. A render-time refusal is better than that silent failure.

The alternative, a new neutral key such as `publicHost`, is out of scope. SMA-694 can decide
whether to move the host to a neutral key. Such a move must then work under `--reuse-values`.

### D2 — D6 holds only when the Ingress is enabled

The chart's decision D6 (SMA-513) says that `zones.<id>.enabled` controls every projection of a
zone together, "the ingress path rules" included, so a zone cannot be routed but not shown.

With `ingress.enabled: false`, the chart renders no routing. The operator's route becomes a
projection of `zones.<id>.enabled` that the operator keeps by hand. If the operator disables a zone
and keeps its route, the route points to a deleted Service. If the operator enables a zone and adds
no route, the `PAIGASUS_ZONES` link to it gives a 404.

This spec accepts that limit, and states it in every place that states D6 (§ 7). The operator must
change the route in the same change as any `zones.<id>.enabled` edit. SMA-694 (a chart-owned
HTTPRoute from the same `range`) restores D6 for Gateway API.

### D3 — alternatives considered

- **Cilium's own Ingress controller** (`ingressController.enabled=true`, `ingress.className:
  cilium`). It needs no chart change and gives the Ingress a load-balancer status. It is rejected
  for this issue because the target cluster uses Gateway API, and the issue asks for a chart that
  works with no Ingress controller. An operator can still use it with the default
  `ingress.enabled: true`.
- **A neutral `publicHost` key.** See D1.

## 4. Design

### 4.1 Value

`values.yaml` gets one new key, first in the `ingress` block. The comments change as follows:

- `enabled: true`: `false` renders no Ingress. The operator must then route
  `https://<host><basePath>` to each enabled zone's console Service (§ 7 lists the full contract).
  It must be a boolean: a quoted `"false"` is refused.
- `host`: REQUIRED, also when `enabled` is false. It feeds `PAIGASUS_PUBLIC_ORIGIN`. A bare host
  name, with no scheme, path or port.
- `tlsSecretName`: REQUIRED when `enabled` is true.

The header comment of `values.yaml` (lines 3-6, "Six projections … the ingress path rules") gets
one sentence: the ingress path rules are a projection only when `ingress.enabled` is true (D2).

### 4.2 Reading the value — helper `paigasus.ingressEnabled`

A new helper in `_helpers.tpl` reads the value with `dig`, with the default `true`:

```
{{- define "paigasus.ingressEnabled" -}}
{{- if (dig "enabled" true .Values.ingress) -}}true{{- end -}}
{{- end -}}
```

It yields the string `true` or an empty string, the same convention as the `paigasus.idpCa*`
helpers, so a caller tests it with `if (include "paigasus.ingressEnabled" .)`. The template and
the validator both use this helper. Neither reads `.Values.ingress.enabled` directly.

Why `dig`: a release made before this value existed has no `enabled` key under
`helm upgrade --reuse-values`, because Helm then uses the old release's values in place of the new
chart defaults. A plain `.Values.ingress.enabled` reads nil, which is false, and the upgrade then
deletes the live Ingress with no warning. `charts/CLAUDE.md` requires `dig` for an optional nested
value.

Measured on the pinned helm 3.22.0 (2026-09-27), `dig "enabled" true .Values.ingress` gives:

| Input | Value | Kind | Key exists |
|---|---|---|---|
| chart default | `true` | bool | yes |
| `--set ingress.enabled=false` | `false` | bool | yes |
| `--set ingress.enabled=null` | `true` | bool | no (Helm deletes the key) |
| `--set-string ingress.enabled=false` | `false` | string | yes |
| values file `enabled: "false"` | `false` | string | yes |

A non-empty string is true in a Go template `if`. So a string `"false"` would keep the Ingress
with no message. § 4.4 refuses it.

### 4.3 Template `templates/ingress.yaml`

- Line 2, `{{- include "paigasus.validate" . -}}`, stays first and stays outside the condition.
  Every template calls the validator (see the comment above `paigasus.validate`), so the chart
  refuses bad values also when the Ingress is off.
- The Ingress body is wrapped in `{{- if (include "paigasus.ingressEnabled" .) }}` …
  `{{- end }}`. All existing YAML comments stay inside the block. A YAML comment renders into the
  manifest, so a comment outside the block would change the golden files.
- Helm trims each document and skips a file that renders to whitespace only. So the `if` and
  `end` lines cannot change the default render. `render.sh` proves this: the goldens are a byte
  pin.

### 4.4 Validation `paigasus.validate`

- **New: the kind of `ingress.enabled`.** When `hasKey .Values.ingress "enabled"` and the value is
  not of kind `bool`, fail: `ingress.enabled must be true or false (a boolean), got <kind> <value>;
  a quoted "false" is a string and would keep the Ingress`. The chart refuses wrong kinds the same
  way in `_iam-backend.tpl`. This check runs before the `tlsSecretName` check, so a string
  `"false"` gets this message and not a misleading `tlsSecretName` message.
- **Changed: `ingress.tlsSecretName`** is refused only when `paigasus.ingressEnabled` is true. The
  message keeps the substring `ingress.tlsSecretName is required` and adds: `or set
  ingress.enabled: false when TLS ends in front of the chart's Services`.
- **Changed: `ingress.host`** is refused in both modes. The message keeps the substring
  `ingress.host is required` and adds that the host is required also when `ingress.enabled` is
  false, because it feeds `PAIGASUS_PUBLIC_ORIGIN`.
- **New: the form of `ingress.host`**, in both modes. Fail when the host contains `://`, `/` or
  `:`: `ingress.host must be a bare host name with no scheme, path or port, got <value>`. Reason:
  with the Ingress on, the API server refuses such a host in `spec.rules[].host`. With the Ingress
  off, nothing refuses it, and `https://console.example.com` renders
  `PAIGASUS_PUBLIC_ORIGIN=https://https://console.example.com`. That value passes the https-URL
  parse in `config.ts` (the host parses as `https`), and login then fails at the IdP with a
  redirect-URI mismatch. An existing working install cannot hold such a value, because the API
  server refused it, so this check breaks no install.

A port is refused also when the Ingress is off. A Gateway listener on a port other than 443 needs
the port in the origin. This spec does not support it. SMA-694 can add it with the neutral key.

### 4.5 Values that do nothing when the Ingress is off

`ingress.className` and `ingress.annotations` have no effect when `ingress.enabled` is false. The
chart does not refuse them. A refusal would break a GitOps values file that toggles only
`enabled`.

## 5. Tests

No new script under `charts/paigasus/tests/`. A new script raises the chart-script floor, which is
pinned in `ci/helm-render/run.sh` (`CHART_SCRIPT_FLOOR=7`) and in
`ci/affected-graph/ci_targets.py` (`HELM_RENDER_SH_CALL_SITES`). The new rows go into existing
scripts. The new rows use process substitution, not a pipe into `grep -q` (actionlint check 13),
and keep the `"${X[@]+...}"` guards, because the scripts also run under bash 3.2.

`tests/ingress.sh`:

- T1 `ingress disabled`: with `--set ingress.enabled=false`, the render succeeds, has no object of
  kind `Ingress`, has one Service `<…>-<zone>-console` with a port named `http` on port 3000 for
  each enabled zone, and `PAIGASUS_ZONES` and `PAIGASUS_SERVICES` still have equal key sets. It runs
  for both zone subsets (iam only; iam and gateway). The existing `coupling` function compares
  Ingress paths with the zone map, so T1 is a separate function, not a `coupling` call.
- T2 `ingress.enabled key absent`: with `--set ingress.enabled=null` (Helm deletes the key, which
  is the `--reuse-values` shape), the render has exactly one Ingress.

`tests/refusals.sh`:

- R1 `expect_render "ingress disabled, tlsSecretName empty"`:
  `--set ingress.enabled=false --set ingress.tlsSecretName=""`.
- R2 `expect_fail "ingress disabled, host empty" "ingress.host is required"`:
  `--set ingress.enabled=false --set ingress.host=""`.
- R3 `expect_fail "ingress.enabled a string" "ingress.enabled must be true or false"`:
  `--set-string ingress.enabled=false`.
- R4 `expect_fail "ingress.enabled key absent, tlsSecretName empty" "ingress.tlsSecretName is
  required"`: `--set ingress.enabled=null --set ingress.tlsSecretName=""`. It catches a validator
  that reads the value without the helper.
- R5 `expect_fail "ingress.host with a scheme" "must be a bare host name"`:
  `--set ingress.host=https://console.example.test`.
- R6 `expect_fail "ingress.host with a port, ingress disabled" "must be a bare host name"`:
  `--set ingress.enabled=false --set ingress.host=console.example.test:8443`.
- The existing row `ingress.tlsSecretName empty` (Ingress enabled by default) stays and still must
  be refused.

`tests/render.sh`: no change. The goldens pass unchanged, which proves criterion 1.

### 5.1 Mutation proof

Run manually before the PR. Each mutation must turn its row red AND leave `render.sh` green. A
mutation that breaks the template parse reds every row for the wrong reason and proves nothing. So
"remove the `if`" also removes its `{{- end }}`. Record each result in the PR description, in the
form of the "Delete-the-feature record" in `ci/helm-render/README.md`.

| Mutation | Row that must go red |
|---|---|
| Remove the `if` and its `end` around the Ingress body | T1 |
| The helper reads `.Values.ingress.enabled` without `dig` | T2 |
| The helper uses `default true .Values.ingress.enabled` | T1 (an explicit `false` becomes `true`) |
| Make the `tlsSecretName` refusal unconditional | R1 |
| Make the `host` refusal conditional on the Ingress | R2 |
| Delete the kind check | R3 |
| The validator reads `.Values.ingress.enabled` directly | R4 |
| Delete the host-form check | R5, R6 |

T2 and R4 depend on the measured fact that `--set ingress.enabled=null` deletes the key on helm
3.22.0 (§ 4.2). The "no `dig`" mutation turning T2 red is the proof of that fact inside the test.

## 6. Coupled files

- `ci/helm-render/fixtures/literal-ingress/templates/ingress.yaml` is a whole-file copy of the
  live `templates/ingress.yaml` with one mutation (both rules written literally). It must be
  re-synced in the same commit, with its mutation kept. `repo:helm-render` must still report the
  control as failing row `1.1 iam`, not INCONCLUSIVE.
- The two `_helpers.tpl` fixtures (`slug-mirror`, `zones-omits-enabled`) are whole-file copies of
  `_helpers.tpl`. They must get the new helper and the changed validation, with their mutations
  kept. A stale copy lacks `paigasus.ingressEnabled`, so its renders fail and the control reports
  INCONCLUSIVE.
- The other four fixtures copy files that this change does not touch.
- `ci/helm-render/helm_render.py` checks 1.1 and 1.2 render with the default values, so they still
  see the Ingress. No change.
- `ci/kind/values/a.yaml` does not set `enabled`, so the kind job keeps the Ingress. No change.

## 7. Docs

**The operator routing contract** (README and RUNBOOK), for `ingress.enabled: false`:

1. Route each enabled zone's `<basePath>` prefix (`PathPrefix`) on `ingress.host` to that zone's
   console Service, port name `http` (port 3000). Take the Service name from the rendered
   manifest: `paigasus.name` shortens the base for a long release name, so
   `<release>-paigasus-<zone>-console` is not always the name.
2. Do not rewrite the path. Each console serves its full `basePath`.
3. Forward the `Host` header (or `X-Forwarded-Host`) unchanged. Next's Server Action origin
   check fails otherwise (`ts/apps/iam-console/README.md` line 56). An HTTPRoute `URLRewrite`
   hostname filter breaks it.
4. Terminate TLS at the Gateway or proxy in front of the chart's Services, for `ingress.host`.
5. Route only the console Services. Never expose a `*-backend` Service.
6. Change the route in the same change as any `zones.<id>.enabled` edit (D2).

**The cut-over procedure** (RUNBOOK), for a release that has a live Ingress:

1. Create the new route and verify it.
2. Set `ingress.enabled: false`.
3. Sync with prune, or delete the Ingress by hand. `helm upgrade` deletes it. Argo CD marks it
   "requires pruning" and keeps it, unless the sync prunes. A kept Ingress can keep the
   Application Progressing.

**Texts that state D6 or the required values, and must change:**

- `charts/paigasus/values.yaml`: the header (lines 3-6) and the `ingress` block comments (§ 4.1).
- `charts/CLAUDE.md` line 9-12 ("three projections that must agree"): true only when the Ingress
  is enabled.
- `charts/paigasus/README.md`: "One values block, six projections" (the ingress rule is a
  projection only when enabled), and "The refusals" (`ingress.tlsSecretName` is required only when
  enabled; the new kind and host-form refusals). A new section "Running without an Ingress
  controller" holds the routing contract.
- `docs/ops/RUNBOOK-chart.md`: § 1 gets an `ingress.enabled` row; the `ingress.host` row says
  "also when `ingress.enabled` is false; a bare host name"; the `ingress.tlsSecretName` row says
  "when `ingress.enabled` is true". § 2 gets the two new refusals. § 4 gets the D2 limit. A new
  section holds the routing contract and the cut-over procedure.

## 8. Out of scope

- A chart-rendered Gateway API HTTPRoute (SMA-694). SMA-694 decides whether `ingress.enabled` and
  its own switch are mutually exclusive.
- A neutral `publicHost` key, and a public origin with a port (D1, § 4.4).
- The same string-kind gap in `zones.<id>.enabled`. It exists today and is not changed here. A
  follow-up issue records it.
- A bump of `Chart.yaml` `version`. The chart is not published: `.github/workflows/chart.yml`
  says "Nothing is pushed and nothing is published", and no workflow packages or pushes it.

## 9. Verification

- `charts/paigasus/tests/*.sh` pass (as run by `repo:helm-render`).
- `repo:helm-render` passes, including the three re-synced negative controls.
- The mutation table in § 5.1 is run, and each mutation reds its row and leaves `render.sh` green.
