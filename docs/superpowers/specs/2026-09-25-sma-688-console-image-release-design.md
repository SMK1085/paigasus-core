<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-688: release the console images to Docker Hub and GHCR

- Linear: [SMA-688](https://linear.app/smaschek/issue/SMA-688)
- Related: SMA-658 (the service image chains), SMA-513 (the chart), SMA-665 (the service SBOM),
  SMA-670 (console image checks, open in parallel)
- Status: draft, 2026-09-25

## 1. Problem

No workflow publishes the `iam-console` and `gateway-console` images. The Helm chart refers to
`ghcr.io/smk1085/paigasus-iam-console` and `ghcr.io/smk1085/paigasus-gateway-console`. A
`helm install` outside kind cannot pull the console pods, and they stop with `ImagePullBackOff`.

The chart has a second defect of the same kind. Every image tag in `charts/paigasus/values.yaml`
is `""`, so each tag falls back to `.Chart.AppVersion`, which is `"0.0.0"`. The two service images
exist only as `0.1.0` (git tags `paigasus-iam-v0.1.0` and `paigasus-gateway-v0.1.0`). So the
default backend tags also point to images that do not exist. AC 6 needs all four defaults to
resolve, so this spec fixes the backend defaults too.

## 2. Decisions

| ID | Decision | Reason |
| -- | -- | -- |
| D1 | The console version source is the `version` field of `ts/apps/<app>/package.json`. A person bumps it by hand in a normal PR (SMA-658 V-a). | Same model as the services. Each console has an independent version. release-plz does not process private TS apps. |
| D2 | A console chain runs when the git tag `paigasus-<app>-v<version>` is missing. Version `0.0.0` never releases. | This is the existing SMA-658 D1 rule and the existing `service_skips` rule. |
| D3 | Chain keys are `iam-console` and `gateway-console`. | The key forms the job names, the plan outputs, the image names and the git tag. |
| D4 | The console jobs alias `*image-build-steps`, `*image-publish-steps` and `*image-tag-steps` with no change. The tooling learns the two console keys. | One step list for four chains. D10, adopt, cosign, attestations and floating tags are then the same code for every image. |
| D5 | The SBOM floor becomes `release_decision.py sbom-floor --service <key>`, with one floor for each image kind. | The current step accepts only an SBOM with Rust crates. A console SBOM has none. |
| D6 | `values.yaml` sets an explicit `tag` on each of the four images. | One `appVersion` cannot hold four independent versions. |
| D7 | A new `repo:helm-render` row 8 asserts that each default image tag equals its version source. | Without a gate, a version bump that forgets the chart makes the default drift again. `helm-render` already renders the chart and pins PyYAML. `release_plan.py` has no dependencies and cannot parse YAML. |
| D8 | One PR. It adds the chains, bumps both consoles to `0.1.0`, and sets the four chart tags. | The approval gate holds every push until the manual registry setup is done (§ 8). |
| D9 | The console chains share the `release-approval` and `release-images` environments with the service chains. | SMA-658 D9. One approval releases every chain that waits in the same run. |

## 3. Measurements

M1 (2026-09-25, this Mac, arm64). `ci/images/run.sh build-console iam` built the image in 37 s,
with a warm cache. syft 1.52.0 on the loaded image found 79 packages: 67 npm, 10 deb, 1 oci and
1 generic. `libc6` 2.36-9+deb12u13, `node` 24.14.0, `next` 16.3.5 and `react` 19.3.0 are in the
list.

M2 (same session). The same build with `--output type=oci,dest=… --provenance=false --sbom=false`
gave a 66 MB OCI archive. `syft oci-archive:<tar>` found the same 79 packages with the same
counts. So the release transport sees the full package set.

M3. The SMA-665 defect (no OS packages in the SBOM) does not apply to the consoles. The
distroless Node base has dpkg status data, and the chisel-cut service base has none.

M4. `ts/Dockerfile` copies only portable wasm glue and the napi loader stubs. It copies no
`.node` binary. `images.yml` already builds both consoles on native amd64 and arm64 runners.

Not measured: the amd64 SBOM counts. The first `images.yml` run on the PR measures them (§ 7).

## 4. Version source and plan

### 4.1 `ci/release-plan/release_plan.py`

`EXPECTED_SERVICES` maps a chain key to a Rust crate. Change it to a map from chain key to a
version source with a kind:

| Key | Kind | Version file | Changelog | Release name |
| -- | -- | -- | -- | -- |
| `iam` | cargo | `rs/crates/services/paigasus-iam/Cargo.toml` | `rs/crates/services/paigasus-iam/CHANGELOG.md` | `paigasus-iam` |
| `gateway` | cargo | `rs/crates/services/paigasus-gateway/Cargo.toml` | `rs/crates/services/paigasus-gateway/CHANGELOG.md` | `paigasus-gateway` |
| `iam-console` | npm | `ts/apps/iam-console/package.json` | `ts/apps/iam-console/CHANGELOG.md` | `paigasus-iam-console` |
| `gateway-console` | npm | `ts/apps/gateway-console/package.json` | `ts/apps/gateway-console/CHANGELOG.md` | `paigasus-gateway-console` |

- The cargo reader stays as it is.
- The npm reader reads the top-level `version` string with `json`. It is stdlib, so the project
  stays at zero dependencies.
- A missing file, an invalid JSON document, or a `version` that is not a string is inconclusive
  for that key only. The existing fail-safe rule applies: the key gets `skip=false` and an empty
  version, and the chain fails at the label compare before the first registry write.
- The `MAJOR.MINOR.PATCH` rule (`_SERVICE_VERSION_RE`) applies to all four keys.
- `--github-output` writes `skip_<key>` and `version_<key>` for all four keys, for example
  `skip_iam-console`. Section 6.1 records the check of the hyphen in an output name.
- `--assert` requires a `## [<version>]` section in the changelog of every key whose version is
  not `0.0.0`. This rule already exists for the two services.

### 4.2 The console changelogs

Add `ts/apps/iam-console/CHANGELOG.md` and `ts/apps/gateway-console/CHANGELOG.md`, in the same
Keep a Changelog form as the service changelogs. Each holds `## [Unreleased]` and
`## [0.1.0] - <merge date>`, with one entry: the first published image.

### 4.3 The bump

`ts/apps/iam-console/package.json` and `ts/apps/gateway-console/package.json` go from `0.0.0` to
`0.1.0`. pnpm does not record a private workspace package version in `ts/pnpm-lock.yaml`. The
plan confirms that with `pnpm install --frozen-lockfile` after the bump.

## 5. Image tooling

### 5.1 `ci/images/run.sh`

- `build-oci <key> <outdir>` accepts `iam-console` and `gateway-console`. For a console key it
  builds from `ts/Dockerfile` with the context `ts/`, the named context
  `bindings=rs/crates/bindings`, `--build-arg APP=<app>` and `--build-arg BASE_PATH=<path>`. It
  uses the same `--provenance=false --sbom=false` OCI output and the same label set as the service
  path. The archive name is `paigasus-<key>-<arch>.oci.tar`.
- `version_for <key>` reads `package.json` for a console key. The OCI label
  `org.opencontainers.image.version` carries that version.
- `smoke <key>` accepts a console key. It runs the existing console smoke (the public route, one
  `(console)` route, uid 65532) against the image that `load-oci` loaded as `paigasus-<key>:dev`.
- The existing `build-console` and `all-consoles` commands stay. `images.yml` still uses them.
- One helper maps a key to its kind. `app_for`, `base_path_for` and `console_probe_path_for` take
  a console key as input too, so no second name table appears.

### 5.2 `ci/images/release_decision.py`

- `sbom_summary` also counts `npm` packages and records whether `next` is present.
- A new `sbom-floor --service <key> <sbom>` subcommand exits 1 with a named reason when the floor
  fails. The floors:
  - cargo keys: `cargo >= 1`. This is the current rule.
  - console keys: `npm >= 1`, `libc6` present, and `next` present.
- An unknown key is an error, not a pass.
- New self-test rows cover each floor with a pass and a fail, and the unknown key.

### 5.3 `.github/workflows/images.yml`

The console step runs `sbom-floor` on each console image that it built. A PR then finds a floor
failure before a release run does. It also measures the amd64 counts that M1 did not measure.

## 6. Release workflow

### 6.1 `.github/workflows/release.yml`

- `plan` gets four new outputs: `skip_iam-console`, `version_iam-console`, `skip_gateway-console`
  and `version_gateway-console`.
- A hyphen in an output name is valid in a GitHub expression, because the expression language has
  no minus operator. The plan proves it with actionlint 1.7.12 on the changed file. If actionlint
  refuses it, the keys in outputs and in `if:` expressions use `_` (`skip_iam_console`), and
  `release_guard.py` derives that form from the chain key in one place.
- Add eight jobs, four for each console, after the gateway chain: `images-build-<key>`,
  `approve-images-<key>`, `publish-images-<key>` and `tag-<key>`. Each job has the same `needs`,
  `if`, `environment`, `concurrency`, `permissions` and `outputs` shape as the gateway job with the
  same role. Each job sets its own `SERVICE`, `VERSION`, `GHCR_IMAGE` and `HUB_IMAGE`, and aliases
  the anchor for its role.
- The `Report the SBOM contents` step inside `&image-build-steps` changes to call
  `sbom-floor --service "${SERVICE}"`. It still prints the summary line.
- Images: `ghcr.io/smk1085/paigasus-<key>` and `docker.io/smaschek/paigasus-<key>`. Git tag:
  `paigasus-<key>-v<version>`.

### 6.2 `ci/actionlint/release_guard.py`

- Add `"iam-console": "approve-images-iam-console"` and
  `"gateway-console": "approve-images-gateway-console"` to `CHAIN_APPROVALS`.
  `SERVICE_PLAN_GATE_EXPRS` and `SCOPED_SECRET_JOBS` derive from it, so V8, V9, V13 and V14 cover
  the new chains with no other change.
- `approval_for_job` compares exact job names, so `images-build-iam` and
  `images-build-iam-console` do not collide. A new fixture pins that fact.
- New fixtures on the image base template, each expected to fail with its own named message:
  - `publish-images-iam-console` without `approve-images-iam-console` in `needs`.
  - `publish-images-iam-console` behind `approve-images-iam` (the wrong chain).
  - `images-build-gateway-console` gated on `skip_gateway`, not `skip_gateway-console`.
  - `DOCKERHUB_TOKEN` in `tag-iam-console`.
  - `publish-images-gateway-console` without `attestations: write`.
- One positive fixture: the base template with both console chains added passes.
- The `--fixture-count` floor in `ci/actionlint/run.sh` check 10 goes up by the number of new
  fixtures.

## 7. Chart

### 7.1 `charts/paigasus/values.yaml`

Set `tag: "0.1.0"` on all four images. Change the comment `# defaults to .Chart.AppVersion` to
say that the tag is pinned to the image version source, that `repo:helm-render` row 8 checks it,
and that an empty tag falls back to `.Chart.AppVersion`. `Chart.yaml` stays as it is: chart
`version: 0.1.0`, `appVersion: "0.0.0"`. No workflow publishes the chart.

### 7.2 `repo:helm-render` row 8

Row `8 default-image-tags` renders the chart with the existing `STUB_VALUES` and
`zones.gateway.enabled=true`. `zones.gateway.enabled` is `false` in `values.yaml`, so without that
override the render holds only two of the four images. The stub values must set no `image` key;
the row asserts that before it renders. Every container `image:` must equal `<repository>:<version>`. The version comes from the
§ 4.1 version file of that image. The row reads the file directly, with `tomllib` or `json`, so it
does not depend on `release_plan.py`.

The row fails when:

- a rendered image has a tag that differs from its version source;
- a rendered image has no row in the image-to-source table;
- a table entry has no rendered image;
- the version source is `0.0.0`, because version `0.0.0` is never published (D2).

The table of four `repository → version file` entries lives in `helm_render.py`. A negative-control
fixture under `ci/helm-render/fixtures/` holds a `values.yaml` with one wrong tag and must make the
module exit 3 with row 8 failed.

### 7.3 Moon inputs

A console bump must select `repo:helm-render`. Its task `inputs` get the four version files
(`rs/crates/services/paigasus-{iam,gateway}/Cargo.toml`, `ts/apps/{iam,gateway}-console/package.json`).
The task that runs `release_plan.py --assert` (`repo:actionlint` check 11) gets
`ts/apps/*/package.json` and `ts/apps/*/CHANGELOG.md`. The plan checks both with
`moon query tasks --affected` on the real diff, and parses the JSON per the root CLAUDE.md rule.

### 7.4 Golden files

`charts/paigasus/tests/golden/iam-only.yaml` and `iam-and-gateway.yaml` change from `:0.0.0` to
`:0.1.0` on each image line. Re-baseline with `tests/render.sh --update`. The diff must hold only
those image lines.

## 8. Rollout

1. Merge the PR. The release run on `main` builds both console images and stops at
   `approve-images-iam-console` and `approve-images-gateway-console`.
2. Before you approve, do the manual setup:
   - Create the public Docker Hub repositories `smaschek/paigasus-iam-console` and
     `smaschek/paigasus-gateway-console`.
   - Make sure that the `DOCKERHUB_TOKEN` PAT in the `release-images` environment can push to the
     two new repositories. If the PAT has a repository scope, extend it or replace it.
   - The `release-images` and `release-approval` environments need no change.
3. Approve. The run publishes both consoles and sets the tags `paigasus-iam-console-v0.1.0` and
   `paigasus-gateway-console-v0.1.0`.
4. After the first push, set both GHCR packages to public and link them to the repository.
5. Verify AC 3 and AC 6 (§ 9).

If step 2 is not done when you approve, `publish-images-<key>` fails on the Docker Hub push.
GHCR then holds the digest and Docker Hub does not. A re-run takes the SMA-658 D10 adopt path and
copies that exact digest to Docker Hub. No digest changes.

## 9. Acceptance criteria and their proof

| AC | Proof |
| -- | -- |
| 1. A release that bumps one console publishes exactly that console image. | `release_plan.py` fixture rows: a tree that bumps only `iam-console` gives `skip_iam-console=false` and `skip=true` for the other three keys. After the rollout: the release run shows only the chains of the bumped keys. |
| 2. amd64 + arm64 index, the same digest in both registries. | The aliased publish steps. They assert the Docker Hub copy digest. After the rollout: `crane digest` on both registries. |
| 3. `cosign verify` and `gh attestation verify` pass. | The aliased verify step. After the rollout: run both commands by hand on each console digest. |
| 4. Nothing is pushed or tagged before the approval. | `release_guard.py` V8 over the new `CHAIN_APPROVALS` entries, and the § 6.2 fixtures. |
| 5. A published `:<version>` never changes digest. | The aliased D10 logic in `&image-publish-steps`. No new code. |
| 6. `helm install` with default values pulls every enabled image outside kind. | Row 8 proves that each default tag equals a version source. After the rollout: for each `image:` line in a `helm template` of the chart with the row 8 values (both zones enabled), an anonymous `crane manifest <ref>` succeeds. A real non-kind cluster install is not part of the proof; the anonymous pull of each reference is the property that `ImagePullBackOff` depends on. |

## 10. Documentation

- `.github/CLAUDE.md`: the image chain bullets name the two console chains and the console
  version source (`package.json`, bump by hand, changelog in `ts/apps/<app>/`).
- `ci/helm-render/README.md`: add row 8 to the table.
- `charts/paigasus/README.md`: a values note. The default tags track the image versions, and a
  version bump must update `values.yaml`.
- `charts/CLAUDE.md`: one line about row 8.

## 11. Out of scope

- A chart release or chart publish workflow.
- The SMA-665 OS-package gap in the service SBOM.
- The SMA-670 console image check changes. That branch also changes the console part of
  `ci/images/run.sh`. The branch that merges second resolves the conflict.
- Automatic console version bumps. release-plz does not process private TS apps.
- Moving `Chart.yaml` `appVersion` away from `0.0.0`.

## 12. Risks

- R1. The console SBOM floor is measured on arm64 only. The first `images.yml` run on the PR
  measures amd64 before the release depends on it.
- R2. A merge conflict with SMA-670 in `ci/images/run.sh`.
- R3. If the Docker Hub PAT cannot push to the new repositories, the first publish fails after
  the GHCR push. The D10 adopt path recovers it (§ 8).
