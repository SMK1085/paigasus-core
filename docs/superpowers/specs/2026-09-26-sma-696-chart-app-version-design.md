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
  The tag is a sound proxy for a published image. `release.yml` makes `tag-<key>` only after
  `publish-images-<key>` (`needs: [plan, publish-images-<key>]`). A failed tag job gives a false
  red, never a false green.
- **D2a. A new chain, before its first release.** A PR that adds a key to `chains.toml` cannot
  have a release tag for it: the tag comes only after the merge. Without a rule, row 8c fails on
  that PR for every `appVersion`. A hand-pushed tag is not a workaround: `release_plan.py` decides
  on tag existence, so the pipeline would skip that chain's first release. The rule: a table
  `UNRELEASED_CHAINS` in `helm_render.py` maps a chain key to a reason. It ships EMPTY. Row 8c
  does not require a tag for a listed key. Row 8c FAILS when a listed key has one or more
  `paigasus-<key>-v*` tags, so the entry must go away when the chain first releases. Row 8c also
  fails when a listed key is not in `chains.toml`. The `chains.toml` header lists this table with
  the other steps for a new key. (Challenger option A. Option B, "skip a key with zero tags",
  was rejected: it hides a chain silently and needs no decision from a human.)
- **D3. The chart `version` stays `0.1.0`.** No workflow packages or publishes the chart, so no
  consumer reads a chart version change.
- **D4. The rule does not make `appVersion` follow each release.** A release PR changes no
  `appVersion`, and the row stays green, because the older tag still exists. With four
  independent chains, one value cannot be "in step" with all of them. D2 gives the property that
  matters: the fallback pulls a real image. To move `appVersion` forward is a manual choice, and
  it takes two PRs: the version bumps merge and release first, and a later PR moves `appVersion`
  after every chain's tag exists. A PR that does both is red until the release. The docs and the
  row's failure message say this.
- **D4a. `appVersion` is the fallback tag, not the deployed version.** The set of versions that
  every chain released can stay `{0.1.0}` for a long time. `helm list` then shows `0.1.0` while the
  pods run newer pinned images. An empty tag pulls a released image that can be older than the
  pinned images beside it. The docs say this in these words.
- **D5. No special rule for `0.0.0`.** No chain has a tag `paigasus-<key>-v0.0.0`, so D2 already
  fails on `0.0.0`. The failure message names the missing tags.

## 3. The new row: `8c chart-app-version`

The row goes into `ci/helm-render/helm_render.py`, after row 8b.

### 3.1 Inputs

- `appVersion` from `<chart>/Chart.yaml`. The row reads the chart under test (`--chart`), not
  the repository chart, so a negative-control fixture can change it.
- The chain keys from `chain_registry()`. This is the same reader that row 8a uses.
- The release tags: `git -C <REPO_ROOT> for-each-ref --format='%(refname:lstrip=2)'
  'refs/tags/paigasus-*'`. This is plumbing output, one tag on each line. `git tag --list` is
  porcelain and follows `column.ui`, so it can print several tags on one line. The tags come from
  the repository, also for a fixture chart. A fixture changes the chart, not the release history.
- The iam+gateway render with `CLEARED_TAGS` (every `image.tag` set to `""`). Row 3b already
  makes this render (`before` in `check3`). The implementation renders it once and gives it to
  both rows, or renders it once more for row 8c. The plan picks one.

### 3.2 The pure function

`check8c(app_version, registry, tags, fallback_docs, unreleased=UNRELEASED_CHAINS)` returns one
row. It takes plain values, so the self-test can call it with no git, no helm and no file. The
row fails when:

1. `app_version` is not a string, or is empty. For a non-string value (for example an unquoted
   `1.0`, which YAML reads as a float), the message says "quote appVersion in Chart.yaml". For an
   empty string, the message says that the fallback has no tag.
2. For one or more chain keys that are not in `unreleased`, `paigasus-<key>-v<app_version>` is not
   in `tags`. The message names each missing tag. It also says: "move appVersion only after every
   chain released it; run `git fetch --tags` if the tags are not local". The comparison is
   whole-string set membership. The tag `paigasus-iam-console-v0.1.0` must not satisfy the chain
   `iam`, and the tag `paigasus-iam-v0.1.0` must not satisfy `app_version` `0.1`.
3. A key in `unreleased` has one or more tags that start with `paigasus-<key>-v`, or is not a
   chain key (D2a).
4. In `fallback_docs`, one or more Deployment containers (`containers` and `initContainers`) have
   an image whose tag is not exactly `app_version`. The tag is the text after the last `:` of the
   image reference. This ties the row to the rendered pull reference, not only to the
   `Chart.yaml` value. A template edit such as `default (printf "v%s" $root.Chart.AppVersion)`
   then fails the row.

### 3.3 The readers and the infrastructure errors

- `chart_app_version(chart)` reads `Chart.yaml` with `yaml.safe_load`. An unreadable file or
  bad YAML raises `InfraError` (rc 2). A missing or non-string `appVersion` is NOT an
  infrastructure error. The reader returns the value, and the row fails on it (§ 3.2 item 1).
- `release_tags(run=subprocess.run)` runs the git command in § 3.1. It raises `InfraError` when:
  - git is not found (`FileNotFoundError`), or git exits non-zero.
  - git lists no `paigasus-*` tag at all. A clone with no tags is a checkout fault, not a chart
    defect. The message says to fetch the tags (`fetch-depth: 0`, or `git fetch --tags`).
  CI has the tags: `ci.yml` checks out with `fetch-depth: 0`, and `release_plan.py --assert`
  already fails on a checkout with no tags. `repo:actionlint` check 11 runs it on every PR.
- `run_checks()` calls the two readers at its TOP, next to the other source reads, before the
  renders. So a git fault fails fast. It appends row 8c after row 8b.
- `EXPECTED_ROW_LABELS` gets `"8c chart-app-version"` as its last entry. The self-test's arity
  check (`len(EXPECTED_ROW_LABELS) != 23`) becomes 24.

### 3.4 The Moon cache and the local tags

Git tags are not a file input of the `repo:helm-render` task. A cached PASS therefore survives a
deleted tag. This is acceptable: a release tag is not deleted in normal work, and `Chart.yaml` is
an input, so every `appVersion` change runs the row again. The task's WHY comment in `moon.yml`
records this limit.

The local run reads the local tags, not the origin's tags. A local-only tag gives a green that CI
does not give. A tag that is not fetched gives a red. CI is the authority. The README says this.

The documented Docker workaround for the small-pipe state mounts only the worktree. A worktree's
`.git` is a link to the common git directory on the host, so git fails there, and the module exits
rc 2. `repo:helm-render` has no here-string, so the README says to run it on the host under
`/bin/bash` 3.2, not in the container.

## 4. The tests

### 4.1 Self-test rows (`helm_render.py --self-test`)

Registry: the chains `iam`, `iam-console` and `gateway-console`. Tags: each chain at `v0.1.0`.
`fallback`: a synthetic iam+gateway render with every image at `:0.1.0`. Each failure row also
asserts a word of the detail text, so a row that fails for the wrong reason does not pass.

| Case | Input | Expected |
|---|---|---|
| good | `0.1.0`, all three tags, `fallback` | pass |
| one chain has no release | no `paigasus-gateway-console-v0.1.0` | fail, detail names that tag |
| prefix trap, key | `0.2.0`; tags hold `paigasus-iam-console-v0.2.0` and `paigasus-gateway-console-v0.2.0`, not `paigasus-iam-v0.2.0` | fail, detail names `paigasus-iam-v0.2.0` |
| prefix trap, version | `0.1` with tags at `0.1.0` | fail |
| the SMA-696 value | `0.0.0` | fail |
| empty | `""` | fail |
| not a string | `1.0` (a float) | fail, detail says "quote" |
| the rendered fallback differs | `0.1.0`, a render with one image at `:v0.1.0` | fail |
| unreleased key, no tags | `gateway-console` listed; no tag for it | pass |
| unreleased key that released | `gateway-console` listed; its tag exists | fail |
| unreleased key not a chain | `billing` listed | fail |
| `release_tags` with no tags | a stub `run` that prints nothing | `InfraError` |
| `release_tags` with a git error | a stub `run` that exits 128 | `InfraError` |
| `release_tags` with no git | a stub `run` that raises `FileNotFoundError` | `InfraError` |
| `release_tags` filters | a stub that prints two tags and a blank line | exactly the two tags |
| `chart_app_version` | a temporary `Chart.yaml` with `appVersion: "0.4.0"` | `"0.4.0"` |
| `chart_app_version` no key | a temporary `Chart.yaml` with no `appVersion` | `None` (not an error) |

### 4.2 Negative control

A new fixture `ci/helm-render/fixtures/app-version-unreleased/Chart.yaml` is a copy of
`Chart.yaml` with `appVersion: "0.1.0-helm-render-unreleased"`. The pipeline never tags this
value: `release_plan.py` rejects pre-release versions. The value is not `0.0.0`, so a special
rule for `0.0.0` cannot make the control pass while the tag path is broken. Its `FIXTURE_TABLE`
row in `run.sh` is `'app-version-unreleased|8c chart-app-version|'`. This proves the production
call site, not only the pure function.

The fixture is a whole-file copy of `Chart.yaml`, so an edit to `Chart.yaml` must re-sync it in
the same commit, as for the other whole-file fixtures.

The new `FIXTURE_TABLE` row is pinned. `ci/affected-graph/ci_targets.py`
`HELM_RENDER_SH_CALL_SITES` gets the line `"'app-version-unreleased|8c chart-app-version|'"`, and
its comment "the six FIXTURE_TABLE rows" becomes "the seven". Without the pin, one commit can
remove the directory and the row, and nothing goes red.

### 4.3 The deletion proof

The implementation must run these mutations, each one alone, and record the results in the
README "Delete-the-feature record". Each mutation must be valid Python that runs.

1. Remove the `check8c` call in `run_checks()`. The row inventory check must raise
   `InfraError` (rc 2) on the production run.
2. Make the body of `check8c` return no problems. The self-test must go red, and the negative
   control must report FAILED for `app-version-unreleased`.
3. Make `run_checks()` read `appVersion` from `charts/paigasus` in place of `--chart`. The
   fallback render still uses the fixture chart, so § 3.2 item 4 still fails the row, and the
   negative control reports OK. With mutation 4 also applied, the negative control must report
   FAILED. (Corrected during planning: item 4 is a second guard for this mistake.)
4. Remove the § 3.2 item 4 assertion. The "rendered fallback differs" self-test row must go red.

## 5. Files that change

| File | Change |
|---|---|
| `charts/paigasus/Chart.yaml` | `appVersion: "0.1.0"` |
| `ci/helm-render/helm_render.py` | `UNRELEASED_CHAINS`, the readers, `check8c`, the inventory label, the arity 24, the self-test rows |
| `ci/helm-render/run.sh` | a `FIXTURE_TABLE` row for the new fixture |
| `ci/helm-render/fixtures/app-version-unreleased/Chart.yaml` | new |
| `ci/affected-graph/ci_targets.py` | the new `HELM_RENDER_SH_CALL_SITES` line; "six" becomes "seven" |
| `ci/helm-render/README.md` | the checks table (row 8c); the negative-control table; the pin count 35 becomes 36; Tool resolution (git, the tags); Running it locally (local tags, no container); the Delete-the-feature record |
| `ci/images/chains.toml` | the header: `helm_render.py` also reads the keys (row 8c); a new key may need an `UNRELEASED_CHAINS` entry |
| `moon.yml` | the WHY comment of `helm-render`: the tag input and its cache limit |
| `charts/paigasus/values.yaml` | all four tag comments, including "falling back to 0.0.0" |
| `charts/paigasus/README.md` | § "The default image tags": the fallback, row 8c, D4 two PRs, D4a |
| `docs/ops/RUNBOOK-chart.md` | the `image.tag` row: the fallback tag, not the deployed version |
| `charts/CLAUDE.md` | the whole-file fixture bullet (add `Chart.yaml`); the last bullet (`0.1.0`, row 8c, two PRs) |
| `.github/CLAUDE.md` | near the release-plan notes: `appVersion` moves in a later PR |
| `ts/packages/paigasus-auth/src/core/session.ts` | lines 24-26: the comment says the zones default their tag to `.Chart.AppVersion`. That is stale since SMA-688; the tags are pinned |

The implementation plan must still grep for other text that says the fallback is `0.0.0`, that
the tags default to `appVersion`, or that counts the rows, the fixtures or the pins. It must
change each hit in the same PR.

## 6. Out of scope

- Automation that moves `appVersion` on a release (D4).
- A check of the registry itself (`ghcr.io`). The git tag is the proxy for a published image.
  A tag with no published image is a release fault. This row does not detect it.
- A chart release or a chart `version` bump (D3).
- A change to row 3b. Row 3b bumps `appVersion` in a temporary copy to
  `0.0.0-helm-render-bump`. It does not depend on the committed value.
- To refuse an empty `image.tag` at render time with `required`. That makes `appVersion` for
  display only and changes row 3b's contract. It is a different scope.

## 7. Success criteria

1. Row 8c asserts that the render with every `image.tag` set to `""` holds only `:0.1.0` images.
2. `bash ci/helm-render/run.sh --self-test`, `--negative-control` and the full run pass.
3. The new fixture makes row `8c chart-app-version` fail. The negative control reports OK for it.
4. Every mutation in § 4.3 gives the stated red.
5. `repo:affected-smoke` passes with the new pin.
6. The full gate graph in the root `CLAUDE.md` passes in CI.
