# paigasus-core — `charts/`

Project memory for the Helm chart. Claude Code loads this file only when it reads a file here.
Operator detail is in `docs/ops/RUNBOOK-chart.md`; developer detail in `charts/paigasus/README.md`.

- **The ingress and the HTTPRoute have no rewrite, on purpose.** Each console has its `basePath`
  compiled in. A rewrite that removes `/iam` breaks every route in that zone. Do not add one to
  `templates/ingress.yaml`, `templates/httproute.yaml` or `values.yaml`. The HTTPRoute has no
  `filters` at all: a hostname rewrite breaks Next's Server Action origin check.
- **One values block renders three projections that must agree:** `PAIGASUS_ZONES`,
  `PAIGASUS_SERVICES` and the route rules (the Ingress rules, and the HTTPRoute rules when
  `httpRoute.enabled` is true). All three come from `zones.<id>.enabled` through one `range`. Do
  not write a zone literally in a template. For the Ingress, `repo:helm-render` check 1 and
  check 2 fail on it. `repo:helm-render` renders with `httpRoute` off and sees no HTTPRoute
  (SMA-694 D10): for `templates/httproute.yaml`, `tests/ingress.sh` rows H1 and H2 fail on it
  instead. With `ingress.enabled: false` and `httpRoute.enabled: false` (SMA-695) the chart
  renders no route rules, so only two projections remain, and the operator keeps the route by
  hand.
- **Read `ingress.enabled` only through `paigasus.ingressEnabled`.** It reads the value with `dig`
  and a default of `true`, and counts nil as not set. A plain `.Values.ingress.enabled` reads nil
  under `--reuse-values` on a release from before the value, and deletes the live Ingress.
- **Read `httpRoute.enabled` only through `paigasus.httpRouteEnabled`** (`templates/_httproute.tpl`).
  A release made before SMA-694 has no `httpRoute` map under `--reuse-values`, and a plain
  `.Values.httpRoute.enabled` is then a nil-pointer error. No comment in `httproute.yaml` may
  contain the word "gateway": the H2 row greps the iam-only render for it (after it removes the
  `apiVersion: gateway.networking.k8s.io/v1` lines).
- **Check 1a couples the chart to `contracts/proto/paigasus/common/v1/service_info.proto`.**
  `paigasus.serviceSlugs` in `_helpers.tpl` must EQUAL the slugs of `enum Capability`. A PR that adds
  a capability for a new service must also edit `_helpers.tpl`.
- **The chart-script floor is pinned twice.** `ci/helm-render/run.sh` `CHART_SCRIPT_FLOOR=7` and
  `ci/affected-graph/ci_targets.py` `HELM_RENDER_SH_CALL_SITES`. The check matches each pinned
  line against the script text as a STRIPPED WHOLE LINE, not a substring (`ci_targets.py`, around
  lines 1903-1907). Change both pins in one commit, or `repo:affected-smoke` goes red.
- **A new chart script under `tests/` runs in CI through the `tests/*.sh` glob.** Raise the floor
  with it, and fix the counts in `ci/helm-render/README.md` and `helm_render.py`.
- **All seven negative-control fixtures are whole-file copies of a live file**, under
  `ci/helm-render/fixtures/`. `templates/_helpers.tpl` holds two: `slug-mirror` and
  `zones-omits-enabled`. `templates/console-deployment.yaml` holds two: `security-context` and
  `template-only-diff`. `templates/console-env-configmap.yaml` holds one: `leaked-value`.
  `templates/ingress.yaml` holds one: `literal-ingress`. `Chart.yaml` holds one:
  `app-version-unreleased`. `templates/_httproute.tpl` has no copy: a fixture chart gets it from
  the live chart. An edit to one of these live files must re-sync its copies in the same
  commit. Keep each mutation when you re-sync. A stale `_helpers.tpl` fixture has none of the new
  helpers. Its renders then fail. The control reports INCONCLUSIVE.
- **A YAML comment in a template renders into the manifest,** so it is part of the golden files.
  Put a comment for a conditional block INSIDE the `{{- if }}` block, or the default render and the
  goldens change.
- **A new REQUIRED value must also go into** the seven chart scripts' value lists,
  `helm_render.py` `STUB_VALUES`, and `ci/kind/values/a.yaml` (row 7).
- **Read optional nested values with `dig`**, as the `paigasus.idpCa*` helpers do. A release made
  before the value existed has no map under `--reuse-values`.
- **Each default image tag is pinned to its image version.** `repo:helm-render` row 8a compares
  every `image.tag` in `values.yaml` with the version file that `ci/images/chains.toml` names
  (SMA-688). A version bump updates the tag in the same PR. An empty tag falls back to
  `appVersion` (`0.2.0`). Row 8c requires a git tag `paigasus-<key>-v<appVersion>` for every chain
  (SMA-696). Move `appVersion` in a later PR, after every chain released that version.
