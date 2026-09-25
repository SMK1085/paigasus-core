<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-688: release the console images to Docker Hub and GHCR

- Linear: [SMA-688](https://linear.app/smaschek/issue/SMA-688)
- Related: SMA-658 (the service image chains), SMA-513 (the chart), SMA-665 (the service SBOM),
  SMA-670 (console image checks, open in parallel)
- Status: draft, revision 2 (after the adversarial challenge), 2026-09-25

## 1. Problem

No workflow publishes the `iam-console` and `gateway-console` images. The Helm chart refers to
`ghcr.io/smk1085/paigasus-iam-console` and `ghcr.io/smk1085/paigasus-gateway-console`. A
`helm install` outside kind cannot pull the console pods, and they stop with `ImagePullBackOff`.

The chart has a second defect of the same kind. Every image tag in `charts/paigasus/values.yaml`
is `""`, so each tag falls back to `.Chart.AppVersion`, which is `"0.0.0"`. The two service images
exist only as `0.1.0` (git tags `paigasus-iam-v0.1.0` and `paigasus-gateway-v0.1.0`). So the
default iam backend tag also points to an image that does not exist.

The chart renders at most three images. The gateway backend is never rendered:
`backend-deployment.yaml:5` renders a backend only when `backend.deploy` is true, `values.yaml`
sets `zones.gateway.backend.deploy: false`, and `_helpers.tpl:81-82` fails the render when it is
true. So AC 6 covers three images: the iam console, the iam backend and the gateway console.

## 2. Decisions

| ID | Decision | Reason |
| -- | -- | -- |
| D1 | The console version source is the `version` field of `ts/apps/<app>/package.json`. A person bumps it by hand in a normal PR (SMA-658 V-a). | Same model as the services. Each console has an independent version. release-plz does not process private TS apps. |
| D2 | A chain runs when the git tag `paigasus-<key>-v<version>` is missing. Version `0.0.0` never releases. | This is the existing SMA-658 D1 rule and the existing `service_skips` rule. |
| D3 | Chain keys are `iam-console` and `gateway-console`. The plan outputs are `skip_<key>` and `version_<key>`, with the hyphen. | GitHub identifiers allow `-`; `release.yml` already uses `needs.images-build-iam`. Note: `iam` is a string prefix of `iam-console`, and `gateway` of `gateway-console`. No chain may select another chain's jobs or artifacts by a prefix or a glob (D11). |
| D4 | The console jobs alias `*image-build-steps`, `*image-publish-steps` and `*image-tag-steps`. The anchors change so that they are generic in `SERVICE` (§ 6.1). | One step list for four chains. D10, adopt, cosign, attestations and floating tags stay one code path. |
| D5 | The SBOM floor becomes `release_decision.py sbom-floor --service <key>`, with one floor for each image kind. | The current step accepts only an SBOM with Rust crates. A console SBOM has none. |
| D6 | `values.yaml` sets an explicit `tag` on each of the four images. | One `appVersion` cannot hold four independent versions. |
| D7 | `repo:helm-render` row 8 asserts that each `values.yaml` default tag equals its version source, and that the rendered images use those tags. | Without a gate, a version bump that forgets the chart makes the default drift again. `helm-render` pins PyYAML; `release_plan.py` has no dependencies. |
| D8 | One PR. It adds the chains, bumps both consoles to `0.1.0`, and sets the four chart tags. `images.yml` runs the full console release sequence on the PR (§ 5.3). | The approval gate holds every push until the manual setup is done (§ 8). The PR run tests the new console tooling before the merge, which answers the reason SMA-658 had for two PRs. |
| D9 | The console chains share the `release-approval` and `release-images` environments with the service chains. | SMA-658 D9. One approval releases every chain that waits in the same run. |
| D10 | One chain registry, `ci/images/chains.toml`, holds each key's kind, version file, changelog and image names. Every Python consumer reads it. `release_guard.py` asserts that `release.yml` agrees with it. | Without it, the key list lives in six or more hand-written places. One of them, `ci/release-plan/run.sh`, drops unknown outputs silently. |
| D11 | A chain job downloads its artifacts by exact name, never by a `pattern:` glob. The per-arch digest file name carries the key. | `pattern: image-iam-*` also matches `image-iam-console-*`, and both artifacts hold a file with the same name. |

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

M5 (2026-09-25, anonymous `crane ls`). Both console repositories refuse anonymous access on GHCR
and on Docker Hub; the issue recorded `404 Package not found` with a token. The GHCR service
packages are public: an anonymous list of `ghcr.io/smk1085/paigasus-iam` and
`…/paigasus-gateway` works. Docker Hub holds `0.1`, `0.1.0` and `latest` for both services.

M6 (2026-09-25, `gh api …/rulesets`). The only ruleset is `Protect main`, target `branch`. No
ruleset restricts the new tag names.

Not measured: the amd64 SBOM counts. The first `images.yml` run on the PR measures them.

## 4. The chain registry and the plan

### 4.1 `ci/images/chains.toml`

```toml
[chain.iam]
kind = "cargo"
version_file = "rs/crates/services/paigasus-iam/Cargo.toml"
changelog = "rs/crates/services/paigasus-iam/CHANGELOG.md"
ghcr = "ghcr.io/smk1085/paigasus-iam"
hub = "docker.io/smaschek/paigasus-iam"

[chain.gateway]      # the same shape, kind = "cargo"
[chain.iam-console]  # kind = "npm", version_file = "ts/apps/iam-console/package.json"
[chain.gateway-console]
```

The release name of every chain is `paigasus-<key>`. It forms the git tag and the image title
label. The file is read with `tomllib`, which is stdlib, so no consumer gets a new dependency.

Consumers:

| Consumer | Uses |
| -- | -- |
| `ci/release-plan/release_plan.py` | keys, kind, version file, changelog |
| `ci/release-plan/run.sh` | the key list, through `release_plan.py --keys` |
| `ci/images/release_decision.py` | kind, for `sbom-floor` |
| `ci/helm-render/helm_render.py` | `ghcr` → version file, for row 8 |
| `ci/actionlint/release_guard.py` | keys and image names, for V16 (§ 6.2) |

`ci/images/run.sh` stays bash and keeps its own `case` tables. It fails on an unknown key.
`images.yml` runs `build-oci` for all four keys on the PR (§ 5.3), so a key that `run.sh` does
not know reds the PR.

### 4.2 `ci/release-plan/release_plan.py`

- `EXPECTED_SERVICES` goes away. The keys come from `chains.toml`.
- The cargo reader stays as it is. The npm reader reads the top-level `version` string with `json`.
- A missing file, invalid JSON, or a `version` that is not a string is inconclusive for that key
  only. The existing fail-safe rule applies: `skip=false` and an empty version for that key.
- The `MAJOR.MINOR.PATCH` rule (`_SERVICE_VERSION_RE`) applies to all four keys.
- The module prints `skip_<key>` and `version_<key>` for every key.
- `--keys` prints the key list, one key on each line, and exits 0.
- `--assert` requires a `## [<version>]` section in the changelog of every key whose version is
  not `0.0.0`.
- A missing or malformed `chains.toml` is inconclusive for every key. It never produces an empty
  key list, because an empty list would write no chain outputs.

Synthetic fixture trees. Every tree that reaches `_assert_repo` or `service_state` must hold a
`chains.toml` and both console `package.json` files at `0.0.0`. Without them, each console adds a
"could not be read" problem, and a fixture such as `_service_unparsable_version_asserts_three`
passes for the wrong reason. Each such fixture's docstring names the mutation that it proves:
with the fix under test removed, the fixture must fail.

New fixture rows:
- A tree that bumps only `iam-console`: `skip_iam-console=false`; `skip=true` for the other three.
- A console `package.json` with no `version`, with `"version": 1`, and with invalid JSON: each is
  inconclusive for that key only, and the other keys keep their values.
- A console at `0.1.0` with no changelog section fails `--assert`.
- `--keys` prints exactly the four keys.

### 4.3 `ci/release-plan/run.sh`

The wrapper hard-codes the four SMA-658 keys in three places: the presence check (`:92-97`), the
fail-safe `printf` lines (`:102`, `:108`) and the per-key extraction (`:124-131`). A console output
from `release_plan.py` is therefore dropped, and an unset `skip_<key>` makes that chain run on
every push to `main`.

The wrapper gets the key list once, from `release_plan.py --keys`. It then loops over the keys for
all three places. Each key keeps its own `grep … | tail -n 1`, for the forged-line reason in the
existing comment. If `--keys` fails or prints no key, the wrapper takes the fail-safe branch and
writes `nothing_to_release=false`, and it exits non-zero, because it cannot name the chain
outputs it must write. A non-zero exit fails the `plan` job, and a failed `plan` skips every
chain (their `needs:` fail). That is the fail-closed direction for the chains, and the kernel
release waits for a fix. This is a deliberate exception to "always build": a chain output that
nobody writes runs the chain with an empty version.

Negative-control rows 5 and 9 cover all nine outputs. `RELEASE_PLAN_SH_CALL_SITES` in
`ci/affected-graph/ci_targets.py` (`:1211-1234`) is re-pinned in the same commit, because those
pins match whole lines.

### 4.4 The console changelogs and the bump

- Add `ts/apps/iam-console/CHANGELOG.md` and `ts/apps/gateway-console/CHANGELOG.md` in the Keep a
  Changelog form of the service changelogs. Each holds `## [Unreleased]` and
  `## [0.1.0] - <merge date>`, with one entry: the first published image. Run `ts:fmt`
  (Prettier) on them.
- `ts/apps/iam-console/package.json` and `ts/apps/gateway-console/package.json` go from `0.0.0` to
  `0.1.0`. The plan confirms with `pnpm install --frozen-lockfile` that `ts/pnpm-lock.yaml` does
  not change.

## 5. Image tooling

### 5.1 `ci/images/run.sh`

- The dispatcher maps a key to a kind and, for a console, to a zone (`iam-console` → `iam`) once.
  The helpers below get the zone, not the key.
- `build-oci <key> <outdir>` accepts the two console keys. The console path:
  - calls `assert_console_pins`, not `assert_pins`;
  - builds from `ts/Dockerfile`, context `ts/`, `--build-context bindings=rs/crates/bindings`,
    `--build-arg APP=<app>` and `--build-arg BASE_PATH=<path>`;
  - uses the same `--provenance=false --sbom=false` OCI output and the same label set as the
    service path, with `org.opencontainers.image.title=paigasus-<key>`;
  - does not use `--no-cache-filter=rootfs` or `extract_chisel_manifest`, which are for the
    chisel-cut service base only;
  - writes `paigasus-<key>-<arch>.oci.tar`.
- `version_for <key>` reads `package.json` for a console key.
- `smoke <key>` accepts a console key. `smoke_consoles` takes the image name as a parameter, so
  it can test `paigasus-<key>:dev` (the `load-oci` name) as well as `<app>:dev` (the
  `build-console` name). Console smoke also calls `assert_fresh` on the image it tests.
- `build-console` and `all-consoles` stay.

### 5.2 `ci/images/release_decision.py`

- `sbom_summary` also counts `npm` packages and records whether `next` is present.
- `sbom-floor --service <key> <sbom>` reads the kind from `chains.toml`. It exits 3 with a named
  reason when the floor fails, because the module contract (`:24-25`) keeps rc 1 for `uv`. The
  floors:
  - `cargo`: `cargo >= 1`. This is the current rule.
  - `npm`: `npm >= 1`, `libc6` present, and `next` present.
- An unknown key exits 3.
- `labels` also emits `title`, so the publish step can check the image identity (§ 6.1).
- New self-test rows cover each floor with a pass and a fail, the unknown key, and `title`.

### 5.3 `.github/workflows/images.yml`

The PR runs the exact release sequence for each console key, as it already does for the
services: `build-oci <key> out`, `load-oci`, `smoke <key>`, `syft oci-archive:…` and
`sbom-floor --service <key>`. Every new console path then runs on the PR, on amd64 and arm64,
before the merge. The existing `all-consoles` step stays.

## 6. Release workflow

### 6.1 `.github/workflows/release.yml`

Anchor changes (they apply to all four chains):

- `&image-build-steps`: the digest file is `out/digest-${SERVICE}-${ARCH}.txt`. The SBOM step runs
  `sbom-floor --service "${SERVICE}"` and still prints the summary line.
- `&image-publish-steps`: two download steps, one for each exact artifact name
  (`image-${SERVICE}-amd64`, `image-${SERVICE}-arm64`), with no `pattern:`. The digest compare
  reads `in/digest-${SERVICE}-${ARCH}.txt`. The decide step also asserts that the archive's
  `title` label equals `paigasus-${SERVICE}`, before the first registry write.

New jobs:

- `plan` gets four new outputs: `skip_iam-console`, `version_iam-console`,
  `skip_gateway-console` and `version_gateway-console`.
- Eight jobs after the gateway chain, four for each console key: `images-build-<key>`,
  `approve-images-<key>`, `publish-images-<key>` and `tag-<key>`. Each job has the same `needs`,
  `if`, `environment`, `concurrency`, `permissions` and `outputs` shape as the gateway job with the
  same role. Each job sets `SERVICE`, `VERSION`, `GHCR_IMAGE` and `HUB_IMAGE`, and aliases the
  anchor for its role.

### 6.2 `ci/actionlint/release_guard.py`

- `CHAIN_APPROVALS` gets the two console keys. `SERVICE_PLAN_GATE_EXPRS` and `SCOPED_SECRET_JOBS`
  derive from it, so V8, V9 and V13 cover the new chains.
- V11: `OIDC_PUBLISH_JOBS` (`:409-411`) is a literal tuple. Its image members change to derive
  from `CHAIN_APPROVALS`. The `_v11_id_token_write_required` document builder (`:3466-3468`)
  changes in the same commit.
- New V15: every `publish-images-<key>` holds `packages: write`, `id-token: write` and
  `attestations: write`. Today no rule requires these grants; V14 only forbids them upstream of an
  approval. A missing grant fails `attest-build-provenance` after the GHCR push.
- New V16, the registry agreement:
  - the `CHAIN_APPROVALS` keys equal the `chains.toml` keys;
  - the `plan` job's outputs include `skip_<key>` and `version_<key>` for every key;
  - each chain job's `env.SERVICE` equals the key in its job id;
  - each `publish-images-<key>` job's `GHCR_IMAGE` and `HUB_IMAGE` equal the `chains.toml` entries.
- New V17 (D11): a `download-artifact` step in a chain job has no `pattern:` key.
- The new fixtures use a separate console template, so the `.replace` edits of the existing
  `_OK_IMAGES_MAIN` rows keep their meaning. Each fixture expects its own named message:
  - `publish-images-iam-console` without `approve-images-iam-console` in `needs` (V8).
  - `publish-images-iam-console` behind `approve-images-iam` (V8, the wrong chain).
  - `images-build-gateway-console` gated on `skip_gateway` (V9).
  - `DOCKERHUB_TOKEN` in `tag-iam-console` (V13).
  - `publish-images-iam-console` without `id-token: write` (V11), and without
    `attestations: write` (V15).
  - `SERVICE: iam` in `publish-images-iam-console` (V16).
  - a key in `CHAIN_APPROVALS` and not in `chains.toml` (V16).
  - `pattern: image-iam-*` in `publish-images-iam` (V17).
  - `approval_for_job("images-build-iam-console")` returns `approve-images-iam-console`, not
    `approve-images-iam`.
- One positive fixture: the console template passes.
- The `--fixture-count` floor in `ci/actionlint/run.sh` check 10 goes up by the number of new
  fixtures.

## 7. Chart

### 7.1 `charts/paigasus/values.yaml`

Set `tag: "0.1.0"` on all four images. The gateway backend tag is never rendered today; it is
pinned for consistency, so that a later chart that deploys it does not fall back to `0.0.0`.
Change the comment `# defaults to .Chart.AppVersion`: the tag is pinned to the image version
source, row 8 checks it, and an empty tag falls back to `.Chart.AppVersion`. `Chart.yaml` stays as
it is. No workflow publishes the chart.

### 7.2 `repo:helm-render` row 8

Row `8 default-image-tags` has two parts:

- **8a, values.** For each `chains.toml` entry, `values.yaml` holds exactly one `image` block whose
  `repository` equals the `ghcr` value, and its `tag` equals the version in the entry's version
  file. The row fails on a tag that differs, on an empty tag, on a version of `0.0.0`, on a chain
  with no `image` block, and on an `image` block whose repository no chain names.
- **8b, render.** The `iam+gateway` render (the existing `STUB_VALUES` with both zones enabled)
  holds three images. Each rendered `image:` equals `<repository>:<tag>` of its `values.yaml` block.
  The row first asserts that `STUB_VALUES` sets no `image` key.

The row is proved by in-process self-test rows over inline YAML and a synthetic version table: a
wrong tag, an empty tag, a `0.0.0` version, an unknown repository, and a rendered image with the
wrong tag. It adds no whole-file fixture copy. `EXPECTED_ROW_LABELS` and its arity check
(`helm_render.py:1002`) grow by the new labels.

### 7.3 Row 3b

`CHECK3_EXPECT["3b"]` requires that an `appVersion` bump changes the pod template of all three
rendered Deployments. With explicit tags, the bump changes nothing, and row 3b goes red. Row 3b
changes to render both sides of the bump with every `image.tag` cleared (`--set …image.tag=`). It
then still proves the fallback path. `ci/helm-render/README.md` states that the "bump
`appVersion` to roll every pod" contract now holds only when the tags are empty.

### 7.4 Moon inputs

- `repo:helm-render` gets four literal input paths:
  `rs/crates/services/paigasus-iam/Cargo.toml`, `rs/crates/services/paigasus-gateway/Cargo.toml`,
  `ts/apps/iam-console/package.json`, `ts/apps/gateway-console/package.json`, and
  `ci/images/chains.toml`. `SELF_TASK_EXPECTED_GLOBS["helm-render"]` in
  `ci/affected-graph/ci_targets.py` (`:390-400`) changes in the same commit.
- `repo:actionlint` already has `inputs: ['**/*']`. It needs no change.
- Any other task that reads `chains.toml` (the release-plan and images tasks, if they declare
  narrow inputs) gets it as an input, with its `SELF_TASK_EXPECTED_GLOBS` twin.
- The plan checks the result with `moon query tasks --affected` on the real diff, and parses the
  JSON per the root CLAUDE.md rule.

### 7.5 Golden files

`charts/paigasus/tests/golden/iam-only.yaml` and `iam-and-gateway.yaml` change from `:0.0.0` to
`:0.1.0` on each image line (two lines and three lines). Re-baseline with
`tests/render.sh --update`. The diff must hold only those image lines.

## 8. Rollout

1. Merge the PR. The release run on `main` builds both console images and waits at
   `approve-images-iam-console` and `approve-images-gateway-console`. From the merge until the
   publish, the chart default console tags name `0.1.0`, which is not yet published.
2. Before you approve, create the public Docker Hub repositories `smaschek/paigasus-iam-console`
   and `smaschek/paigasus-gateway-console`. The `release-images` and `release-approval`
   environments and the `DOCKERHUB_TOKEN` need no change.
3. Approve the newest pending run. Reject older pending runs, if any.
4. The run publishes both consoles and sets the tags `paigasus-iam-console-v0.1.0` and
   `paigasus-gateway-console-v0.1.0`.
5. After the first push, set both GHCR packages to public and link them to the repository.
6. Verify AC 2, AC 3 and AC 6 (§ 9).

If step 2 is not done when you approve, the Docker Hub part of `publish-images-<key>` fails. The
GHCR `:<version>` tag is written only in the last publish step (`release.yml:1611`), so no
`:<version>` tag exists yet in either registry. Before the failure, GHCR holds only the
`:<sha>` tags. After step 2, a re-run of the failed job takes the `push-new` path with the same
build artifacts and so with the same digest. The plan measures which step fails first on a
missing Docker Hub repository, and the runbook text follows that measurement.

## 9. Acceptance criteria and their proof

| AC | Proof |
| -- | -- |
| 1. A release that bumps one console publishes exactly that console image. | The § 4.2 fixture row that bumps only `iam-console`, and the § 4.3 negative-control rows that prove the wrapper writes each output. The rollout bumps both consoles, so the live run cannot show this. |
| 2. amd64 + arm64 index, the same digest in both registries. | The aliased publish steps, which assert the Docker Hub copy digest. After the rollout: `crane digest` on both registries. |
| 3. `cosign verify` and `gh attestation verify` pass. | The aliased verify step, and V11 and V15 for the grants it needs. After the rollout: both commands by hand on each console digest. |
| 4. Nothing is pushed or tagged before the approval. | V8 over the new `CHAIN_APPROVALS` entries, V16, and the § 6.2 fixtures. |
| 5. A published `:<version>` never changes digest. | The aliased D10 logic in `&image-publish-steps`, and the title check in the decide step, which stops a wrong image before its first `:<version>` tag. |
| 6. `helm install` with default values pulls every enabled image outside kind. | Row 8a and 8b. After the rollout: for each of the three `image:` lines in the row 8b render, an anonymous `crane manifest <ref>` succeeds. A real install on a cluster other than kind is not part of the proof; the anonymous pull of each reference is the property that `ImagePullBackOff` depends on. |

## 10. Documentation

- `.github/CLAUDE.md`: the image chain bullets name the console chains, `chains.toml`, and the
  console version source. The service bump rule adds: "and update the tag in
  `charts/paigasus/values.yaml`; row 8 fails otherwise".
- `ci/release-plan/README.md`: the outputs list and `--keys`.
- `ci/helm-render/README.md`: rows 8a and 8b, and the new form of row 3b.
- `charts/paigasus/README.md` and `docs/ops/RUNBOOK-chart.md:14`: the default tags track the image
  versions; they no longer default to `appVersion`.
- `charts/CLAUDE.md`: one line about row 8.

## 11. Out of scope

- A chart release or chart publish workflow.
- The SMA-665 OS-package gap in the service SBOM.
- The SMA-670 console image check changes. That branch also changes the console part of
  `ci/images/run.sh`. The branch that merges second resolves the conflict. If SMA-670 adds a
  runtime dependency to the console smoke (for example a session store), the release build job
  must supply it; the plan checks the state of SMA-670 before it changes `smoke_consoles`.
- Automatic console version bumps.
- Moving `Chart.yaml` `appVersion` away from `0.0.0`.
- A chart that deploys the gateway backend.

## 12. Risks

- R1. The console SBOM floor is measured on arm64 only. The PR's `images.yml` run measures amd64
  (§ 5.3) before the release depends on it.
- R2. A merge conflict with SMA-670 in `ci/images/run.sh`.
- R3. The anchor changes of § 6.1 touch the live service chains. The next service release is the
  first live run of the changed steps. `images.yml` does not run the publish steps, so only
  `release_guard.py` and actionlint check them before that run.
