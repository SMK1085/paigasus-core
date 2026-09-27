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
2. With `ingress.enabled: false`, the render contains no object of kind `Ingress`.
3. With `ingress.enabled: false`, an empty `ingress.tlsSecretName` renders.
4. With `ingress.enabled: false`, an empty `ingress.host` is still refused.
5. A release that has no `ingress.enabled` key (a release made before this change, upgraded with
   `--reuse-values`) still renders the Ingress.

## 3. Decision D1 — `ingress.host` stays required

The issue proposes not to require `ingress.host` when the Ingress is off. This spec does not do
that. Sven chose this on 2026-09-27.

`ingress.host` is not only the Ingress host. `templates/console-env-configmap.yaml` line 8 sets
`PAIGASUS_PUBLIC_ORIGIN` to `https://<ingress.host>`. `ts/packages/paigasus-auth/src/config.ts`
line 72 parses that value as an https URL, and `runtime.ts` lines 157-158 build the OIDC redirect
URI and the post-logout redirect URI from it. An empty host renders `https://`, which fails that
parse, so every console pod fails at start.

The alternative, a new neutral key such as `publicHost`, is out of scope. SMA-694 can decide
whether to move the host to a neutral key.

## 4. Design

### 4.1 Value

`values.yaml` gets one new key, first in the `ingress` block:

```yaml
ingress:
  enabled: true          # false: the chart renders no Ingress. host stays REQUIRED ...
  host: ""               # REQUIRED, also when enabled is false. Feeds PAIGASUS_PUBLIC_ORIGIN.
```

The comment on `enabled` says what the operator must supply when it is `false`: a route from
`https://<host><basePath>` to each enabled zone's console Service (`<release>-paigasus-<zone>-console`,
port name `http`, port 3000), with no rewrite, and TLS terminated upstream. The comment on `host`
says that it is required in both modes.

### 4.2 Reading the value — helper `paigasus.ingressEnabled`

A new helper in `_helpers.tpl` reads the value with `dig`, with the default `true`:

```
{{- define "paigasus.ingressEnabled" -}}
{{- if (dig "enabled" true .Values.ingress) -}}true{{- end -}}
{{- end -}}
```

It yields the string `true` or an empty string, the same convention as the `paigasus.idpCa*`
helpers, so a caller tests it with `if (include "paigasus.ingressEnabled" .)`.

Why `dig`: a release made before this value existed has no `enabled` key under
`helm upgrade --reuse-values`. A plain `.Values.ingress.enabled` reads nil, which is false, and the
upgrade then deletes the live Ingress with no warning. `dig` returns the default `true` for a
missing key and returns a set `false` as `false`. `charts/CLAUDE.md` requires `dig` for an optional
nested value.

### 4.3 Template `templates/ingress.yaml`

- Line 2, `{{- include "paigasus.validate" . -}}`, stays first and stays outside the condition.
  Every template calls the validator (see the comment above `paigasus.validate`), so the chart
  refuses bad values also when the Ingress is off.
- The Ingress body is wrapped in `{{- if (include "paigasus.ingressEnabled" .) }}` …
  `{{- end }}`. All existing YAML comments stay inside the block. A YAML comment renders into the
  manifest, so a comment outside the block would change the golden files.
- The whitespace control is chosen so that the default render is byte-identical. `render.sh`
  proves this: the goldens are a byte pin.

### 4.4 Validation `paigasus.validate`

- The `ingress.tlsSecretName` refusal fires only when `paigasus.ingressEnabled` is true. The
  message text does not change, so the `refusals.sh` needle still matches.
- The `ingress.host` refusal fires in both modes. Its message keeps the existing substring
  `ingress.host is required` and adds that the host is required also when `ingress.enabled` is
  false, because it feeds `PAIGASUS_PUBLIC_ORIGIN`.

### 4.5 Values that do nothing when the Ingress is off

`ingress.className` and `ingress.annotations` have no effect when `ingress.enabled` is false. The
chart does not refuse them. A refusal would break a GitOps values file that toggles only
`enabled`.

## 5. Tests

No new script under `charts/paigasus/tests/`. A new script raises the chart-script floor, which is
pinned in `ci/helm-render/run.sh` (`CHART_SCRIPT_FLOOR=7`) and in
`ci/affected-graph/ci_targets.py` (`HELM_RENDER_SH_CALL_SITES`). The new rows go into existing
scripts.

`tests/ingress.sh`:

- T1 `ingress disabled`: with `--set ingress.enabled=false`, the render succeeds, has no object of
  kind `Ingress`, and `PAIGASUS_ZONES` and `PAIGASUS_SERVICES` still have equal key sets. The
  existing `coupling` function compares Ingress paths with the zone map, so T1 is a separate
  function, not a `coupling` call.
- T2 `ingress.enabled key absent`: with `--set ingress.enabled=null` (Helm deletes the key, which
  is the `--reuse-values` shape), the render has exactly one Ingress.
- T3 `ingress enabled explicitly`: with `--set ingress.enabled=true`, the render has exactly one
  Ingress.

`tests/refusals.sh`:

- R1 `expect_render "ingress disabled, tlsSecretName empty"`:
  `--set ingress.enabled=false --set ingress.tlsSecretName=""`.
- R2 `expect_fail "ingress disabled, host empty" "ingress.host is required"`:
  `--set ingress.enabled=false --set ingress.host=""`.
- The existing row `ingress.tlsSecretName empty` (Ingress enabled by default) stays and still must
  be refused.

`tests/render.sh`: no change. The goldens pass unchanged, which proves criterion 1.

Mutation proof (manual, before the PR): each mutation below must turn at least one row red.

| Mutation | Row that must go red |
|---|---|
| Remove the `if` around the Ingress body | T1 |
| Read `.Values.ingress.enabled` without `dig` | T2 |
| Make the `tlsSecretName` refusal unconditional | R1 |
| Make the `host` refusal conditional on the Ingress | R2 |

## 6. Coupled files

- `ci/helm-render/fixtures/literal-ingress/templates/ingress.yaml` is a whole-file copy of the
  live `templates/ingress.yaml` with one mutation (both rules written literally). It must be
  re-synced in the same commit, with its mutation kept. `repo:helm-render` must still report the
  control as failing row `1.1 iam`, not INCONCLUSIVE.
- The two `_helpers.tpl` fixtures (`slug-mirror`, `zones-omits-enabled`) are whole-file copies of
  `_helpers.tpl`. They must get the new helper and the changed validation, with their mutations
  kept. A stale copy lacks `paigasus.ingressEnabled`, so its renders fail and the control reports
  INCONCLUSIVE.
- `ci/helm-render/helm_render.py` renders with the default values, so check 1 still sees the
  Ingress. No change.
- `ci/kind/values/a.yaml` does not set `enabled`, so the kind job keeps the Ingress. No change.

## 7. Docs

- `charts/paigasus/README.md`: a short section "Running without an Ingress controller".
- `docs/ops/RUNBOOK-chart.md`: the same facts for the operator: set `ingress.enabled: false`, keep
  `ingress.host`, route each `<basePath>` prefix to `<release>-paigasus-<zone>-console:http`
  with no rewrite, terminate TLS upstream, and see SMA-694 for a chart-owned HTTPRoute.

## 8. Out of scope

- A chart-rendered Gateway API HTTPRoute (SMA-694).
- A neutral `publicHost` key (see D1).
- A bump of `Chart.yaml` `version`. No commit has changed it since the chart was added.

## 9. Verification

- `charts/paigasus/tests/*.sh` pass (as run by `repo:helm-render`).
- `repo:helm-render` passes, including the three re-synced negative controls.
- The manual mutation table in § 5 is run, and each mutation reds its row.
