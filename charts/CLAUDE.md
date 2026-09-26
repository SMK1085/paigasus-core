# paigasus-core — `charts/`

Project memory for the Helm chart. Claude Code loads this file only when it reads a file here.
Operator detail is in `docs/ops/RUNBOOK-chart.md`; developer detail in `charts/paigasus/README.md`.

- **The ingress has no rewrite annotation, on purpose.** Each console has its `basePath` compiled
  in. A rewrite that removes `/iam` breaks every route in that zone. Do not add one to
  `templates/ingress.yaml` or to `values.yaml`.
- **One values block renders three projections that must agree:** `PAIGASUS_ZONES`,
  `PAIGASUS_SERVICES` and the ingress rules. All three come from `zones.<id>.enabled` through one
  `range`. Do not write a zone literally in a template: `repo:helm-render` check 1 and check 2
  fail on it.
- **Check 1a couples the chart to `contracts/proto/paigasus/common/v1/service_info.proto`.**
  `paigasus.serviceSlugs` in `_helpers.tpl` must EQUAL the slugs of `enum Capability`. A PR that adds
  a capability for a new service must also edit `_helpers.tpl`.
- **The chart-script floor is pinned twice.** `ci/helm-render/run.sh` `CHART_SCRIPT_FLOOR=7` and
  `ci/affected-graph/ci_targets.py` `HELM_RENDER_SH_CALL_SITES`. The check matches each pinned
  line against the script text as a STRIPPED WHOLE LINE, not a substring (`ci_targets.py`, around
  lines 1903-1907). Change both pins in one commit, or `repo:affected-smoke` goes red.
- **A new chart script under `tests/` runs in CI through the `tests/*.sh` glob.** Raise the floor
  with it, and fix the counts in `ci/helm-render/README.md` and `helm_render.py`.
- **Five negative-control fixtures are whole-file copies:** of `templates/_helpers.tpl` (two),
  `templates/console-deployment.yaml` (two) and `Chart.yaml` (one, `app-version-unreleased`),
  under `ci/helm-render/fixtures/`. An edit to any of these live files must re-sync them in the
  SAME commit, keeping each mutation. A stale `_helpers.tpl` fixture has none of the new helpers,
  so its renders fail and the control reports INCONCLUSIVE.
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
  `appVersion` (`0.1.0`). Row 8c requires a git tag `paigasus-<key>-v<appVersion>` for every chain
  (SMA-696). Move `appVersion` in a later PR, after every chain released that version.
