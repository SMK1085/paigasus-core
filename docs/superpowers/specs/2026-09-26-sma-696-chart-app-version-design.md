<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-696: the chart `appVersion` names a released image

- Linear: [SMA-696](https://linear.app/smaschek/issue/SMA-696)
- Branch: `feature/sma-696-chart-app-version`
- Related: SMA-688 (#311) pinned the four default image tags and added rows 8a and 8b.

## 1. The problem

SMA-696 reports this defect: `charts/paigasus/Chart.yaml` sets `appVersion: "0.0.0"`. The two
Deployment templates use `.Chart.AppVersion` when `image.tag` is empty. No published image has the
tag `0.0.0`, so a default install ends in ImagePullBackOff. The report is for commit cb77239.

SMA-688 (#311) merged after cb77239. It set all four `image.tag` values in `values.yaml` to
`"0.1.0"`, and row 8a keeps each tag equal to its chain version. So a default install on `main`
pulls `0.1.0` now. These parts of the defect remain on `main`:

1. `appVersion` is still `"0.0.0"`. `helm list` shows `0.0.0` as the APP VERSION.
2. An operator who sets `image.tag: ""` still gets the unpullable tag `0.0.0`.
3. No check prevents a future `appVersion` that names no released image.

Sven chose this scope on 2026-09-26: set `appVersion` to `0.1.0` and add a gate.

## 2. The decisions

- **D1. `appVersion` becomes `"0.1.0"`.** All four chains released `0.1.0`. The git tags
  `paigasus-iam-v0.1.0`, `paigasus-gateway-v0.1.0`, `paigasus-iam-console-v0.1.0` and
  `paigasus-gateway-console-v0.1.0` exist.
- **D2. The rule: every chain released `appVersion`.** For each chain key `<key>` in
  `ci/images/chains.toml`, the git tag `paigasus-<key>-v<appVersion>` must exist. Sven chose this
  rule over two file-only rules. "Equal to every chain version" fails when one chain releases
  alone, and then no value can pass. "Equal to the lowest chain version" does not prove that a
  chain which skipped that version has an image with that tag.
- **D3. The chart `version` stays `0.1.0`.** No workflow packages or publishes the chart, so no
  consumer reads a chart version change.
- **D4. The rule does not make `appVersion` follow each release.** A release PR changes no
  `appVersion`, and the row stays green, because the older tag still exists. The issue proposes
  "keep `appVersion` in step with each image release". With four independent chains, one value
  cannot be "in step" with all of them. D2 gives the property that matters: the fallback pulls a
  real image. To move `appVersion` forward is a manual choice, and the row checks that choice.
- **D5. No special rule for `0.0.0`.** No chain has a tag `paigasus-<key>-v0.0.0`, so D2 already
  fails on `0.0.0`. The failure message names the missing tags.

## 3. The new row: `8c chart-app-version`

The row goes into `ci/helm-render/helm_render.py`, after row 8b.

### 3.1 Inputs

- `appVersion` from `<chart>/Chart.yaml`. The row reads the chart under test (`--chart`), not
  the repository chart, so a negative-control fixture can change it.
- The chain keys from `chain_registry()`. This is the same reader that row 8a uses.
- The release tags: `git -C <REPO_ROOT> tag --list 'paigasus-*'`. The tags come from the
  repository, also for a fixture chart. A fixture changes the chart, not the release history.

### 3.2 The pure function

`check8c(app_version, registry, tags)` returns one row. It takes plain values, so the self-test
can call it with no git and no file. The row fails when:

1. `app_version` is not a non-empty string. The message says that the fallback has no tag.
2. For one or more chain keys, `paigasus-<key>-v<app_version>` is not in `tags`. The message
   names each missing tag. The comparison is whole-string set membership. The tag
   `paigasus-iam-console-v0.1.0` must not satisfy the chain `iam`.

### 3.3 The readers and the infrastructure errors

- `chart_app_version(chart)` reads `Chart.yaml` with `yaml.safe_load`. An unreadable file or
  bad YAML raises `InfraError` (rc 2). A missing or non-string `appVersion` is NOT an
  infrastructure error. The reader returns the value, and the row fails on it (§ 3.2 item 1).
- `release_tags(run=subprocess.run)` runs the git command in § 3.1. It raises `InfraError` when:
  - git is not found, or git exits non-zero.
  - git lists no `paigasus-*` tag at all. A clone with no tags is a checkout fault, not a chart
    defect. The message says to fetch the tags (`fetch-depth: 0`). `ci.yml` checks out with
    `fetch-depth: 0` today.
- `run_checks()` calls the two readers and appends the row after row 8b.
  `EXPECTED_ROW_LABELS` gets `"8c chart-app-version"` as its last entry.

### 3.4 The Moon cache

Git tags are not a file input of the `repo:helm-render` task. A cached PASS therefore survives a
deleted tag. This is acceptable: a release tag is not deleted in normal work, and `Chart.yaml` is
an input, so every `appVersion` change runs the row again. The task's WHY comment in `moon.yml`
records this limit.

## 4. The tests

### 4.1 Self-test rows (`helm_render.py --self-test`)

Registry: the chains `iam`, `iam-console` and `gateway-console`. Tags: each chain at `v0.1.0`.

| Case | Input | Expected |
|---|---|---|
| good | `0.1.0`, all three tags | pass |
| one chain has no release | `0.1.0`, no `paigasus-gateway-console-v0.1.0` | fail |
| prefix trap | tags hold `paigasus-iam-console-v0.2.0` but not `paigasus-iam-v0.2.0`, `app_version` `0.2.0`, others released | fail |
| the SMA-696 value | `0.0.0` | fail |
| empty | `""` | fail |
| not a string | `None` | fail |
| `release_tags` with no tags | a stub `run` that prints nothing | `InfraError` |
| `release_tags` with a git error | a stub `run` that exits 128 | `InfraError` |
| `release_tags` filters | a stub that prints two tags and a blank line | exactly the two tags |
| `chart_app_version` | a temporary `Chart.yaml` with `appVersion: "0.4.0"` | `"0.4.0"` |
| `chart_app_version` no key | a temporary `Chart.yaml` with no `appVersion` | `None` (not an error) |

### 4.2 Negative control

A new fixture `ci/helm-render/fixtures/app-version-unreleased/Chart.yaml` is a copy of
`Chart.yaml` with `appVersion: "0.0.0"`. Its `FIXTURE_TABLE` row in `run.sh` names
`8c chart-app-version` as the row that must fail. This proves the production call site, not only
the pure function. The fixture is a whole-file copy of `Chart.yaml`, so an edit to `Chart.yaml`
must re-sync it in the same commit, as for the other whole-file fixtures.

### 4.3 The deletion proof

The implementation must show that the negative control and the production run go red when the
`check8c` call in `run_checks()` is removed. The row inventory check must report the missing row as
an infrastructure error. This follows the rule "red-first is not proof, delete the feature".

## 5. Files that change

| File | Change |
|---|---|
| `charts/paigasus/Chart.yaml` | `appVersion: "0.1.0"` |
| `ci/helm-render/helm_render.py` | the readers, `check8c`, the inventory label, the self-test rows |
| `ci/helm-render/run.sh` | a `FIXTURE_TABLE` row for the new fixture |
| `ci/helm-render/fixtures/app-version-unreleased/Chart.yaml` | new |
| `ci/helm-render/README.md` | a row `8c chart-app-version` in the checks table; the fixture |
| `moon.yml` | the WHY comment of `helm-render`: the tag input and its cache limit |
| `charts/paigasus/values.yaml` | the tag comment: an empty tag falls back to a released `appVersion` |
| `charts/paigasus/README.md` | § "The default image tags": the new fallback and row 8c |
| `docs/ops/RUNBOOK-chart.md` | the `image.tag` row: the fallback is `appVersion`, a released version |
| `charts/CLAUDE.md` | the last bullet: `appVersion` is `0.1.0`, row 8c guards it |

The implementation plan must grep for other text that says the fallback is `0.0.0`, or that
counts the rows or the fixtures (for example a floor in `ci/affected-graph/ci_targets.py`). It must
change each hit in the same PR.

## 6. Out of scope

- Automation that moves `appVersion` on a release (D4).
- A check of the registry itself (`ghcr.io`). The git tag is the proxy for a published image.
  A tag with no published image is a release fault. This row does not detect it.
- A chart release or a chart `version` bump (D3).
- A change to row 3b. Row 3b bumps `appVersion` in a temporary copy to
  `0.0.0-helm-render-bump`. It does not depend on the committed value.

## 7. Success criteria

1. `helm template` of the chart with every `image.tag` set to `""` renders `:0.1.0` images.
2. `bash ci/helm-render/run.sh --self-test`, `--negative-control` and the full run pass.
3. The new fixture makes row `8c chart-app-version` fail. The negative control reports OK for it.
4. The deletion proof in § 4.3 holds.
5. The full gate graph in the root `CLAUDE.md` passes in CI.
