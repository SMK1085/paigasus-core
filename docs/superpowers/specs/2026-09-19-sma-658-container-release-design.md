# SMA-658 — Release the service container images to Docker Hub and GHCR

- **Linear:** [SMA-658](https://linear.app/smaschek/issue/SMA-658)
- **Related:** SMA-500 (the image build), SMA-505 R7 (the `0.0.0` version), SMA-580 (release-plz tags
  only what it publishes), SMA-602 (V10, no publish credential), SMA-603 (`ci/release-plan/`).
- **Status:** revision 2, after the adversarial challenge (verdict: NEEDS REWORK, 3 BLOCKER,
  9 MAJOR). Sven approved the three design sections of revision 1 in the brainstorm on 2026-09-19.
  Revision 2 changes the job graph (§ 4) and the recovery model (§ 5). It waits for his review
  (GATE 1).
- **Approach chosen by Sven:** A — new jobs in `release.yml` (option 1 of 3, 2026-09-19).

## 1. Problem

SMA-500 builds and smoke-tests two images, `paigasus-iam` and `paigasus-gateway`, from one
`rs/Dockerfile`. `.github/workflows/images.yml` has `contents: read` only and pushes nothing. The
SMA-500 spec deferred "push, credentials, semver tags, retention and signing" (its lines 317 and
674). A user cannot pull a Paigasus image from any registry today.

The two services also have no release version. Both crates stay at `0.0.0`, and `rs/release-plz.toml`
sets `release = false` for them. So an image release has no version to carry.

## 2. Decisions

| # | Topic | Decision |
|---|---|---|
| D1 | Trigger | A service version that has no git tag `paigasus-<svc>-v<version>` yet. |
| D2 | Registries | Docker Hub `docker.io/smaschek/paigasus-{iam,gateway}` and GHCR `ghcr.io/smk1085/paigasus-{iam,gateway}`. Both registries hold the same index digest. |
| D3 | Architectures | `linux/amd64` and `linux/arm64`, built on native runners (`ubuntu-latest`, `ubuntu-24.04-arm`), joined in one image index. |
| D4 | Metadata | GitHub build provenance and SBOM attestations, and a cosign keyless signature on the index digest. |
| D5 | Versioning | **V-a: a version bump by hand, in a normal PR** (§ 3.1). Sven chose it at GATE 1 on 2026-09-19, after M7 showed that release-plz cannot bump a Cargo `publish = false` crate. The `tag-<svc>` job makes the git tag. |
| D6 | Version relation | Independent. Each service has its own version. |
| D7 | Docker Hub credential | A personal access token with Read & Write scope, stored as an environment secret. |
| D8 | Workflow shape | New jobs in `release.yml` (approach A). |
| D9 | Approval | **One approval for each service**, separate from the kernel approval (§ 4). Revision 2. |
| D10 | Authoritative digest | The **first** digest pushed under `:<version>` is final. No later run overwrites it (§ 5). Revision 2. |

**Why D7 needs a stored secret.** Docker Hub supports OIDC for GitHub Actions through
`docker/login-action` v4.5.0 or later. But Docker Hub offers OIDC connections only to organizations
with a Team, Business or DHI subscription, or in the Docker Sponsored Open Source program
([Docker blog](https://www.docker.com/blog/docker-oidc-connections-for-github-actions-available-for-docker-orgs/),
[Docker docs](https://docs.docker.com/security/authentication/oidc-connections/create-manage),
[docker/roadmap#918](https://github.com/docker/roadmap/issues/918), read on 2026-09-19). A free or
Pro personal account, such as `smaschek`, cannot use it. So the Docker Hub path needs a token, and
the token is an exception to V10 (§ 7.2). GHCR uses `github.token` and needs no secret.

**The token's scope.** A Docker Hub PAT covers every repository under `smaschek`, not only the two
image repositories. A leak therefore exposes the whole namespace. This is the accepted cost of D7.
A move to an organization with OIDC removes it (§ 12).

**Rejected approaches.**

- **B — a separate `images-release.yml` on a tag push.** `release_guard.py` does not check that
  workflow, and a failed push leaves a tag with no image.
- **C — a push mode in `images.yml`.** Not possible. `images.yml` has a `pull_request` trigger, and
  `repo:workflow-credentials` bans every secret in such a workflow.
- **GHCR only for a first version.** This removes the only stored secret, V13 and the copy step.
  Sven chose both registries in the brainstorm; this spec does not reopen that decision.
- **A shared approval for kernel and images** (challenger B1, option a: redefine
  `nothing_to_release`). An image-only release would then also run `wheels`, `prebuild`, `release`,
  `publish-pypi` and `publish-npm` for kernel versions that are already tagged. `release.yml:132-142`
  records that a fresh run over existing tags is not a safe path for `publish-npm`. D9 keeps the
  kernel chain unchanged instead.

## 3. Service versions

### 3.1 release-plz cannot version the services (M7, measured)

M7 ran on 2026-09-19 against release-plz 0.3.158, in a fixture that copies the `[workspace]` keys
of `rs/release-plz.toml`. A `lib` crate and a binary `svc` crate that depends on it were both
Cargo `publish = false`. `svc` had `release = true` and its own `version_group`.

| Case | Result |
|---|---|
| No `git_only`: a `feat` commit on `svc`, a `fix` on `lib`, a `Cargo.lock`-only change | MEASURED: `svc` stays at `0.1.0` in every case. The log shows `version groups: {}`. |
| `git_only = true`, no tags | MEASURED: `update` sees both crates once ("initial release") and writes a changelog. |
| `git_only = true`, tag present, a new commit | MEASURED: **hard error**, exit 1: `cargo package failed … no matching package named 'pgs-m7-lib-…' found … required by package 'pgs-m7-svc-a-…'`. The same with `git_tag_name = "{{ package }}-v{{ version }}"`. |
| Does `release-plz release` tag a Cargo `publish = false` crate? | READ FROM SOURCE (`release_plz_core` 0.36.14, `release.rs:586`): never, with or without `git_only`. |

The cause (READ FROM SOURCE): `packages_to_process()` (`updater.rs:361-366`) takes
`publishable_packages()` (`project.rs:110-114`), which reads Cargo's own `publish` field. A Cargo
`publish = false` crate is outside the process. `release = true` and `version_group` do not change
that. `git_only` adds the crate back, but a second release then runs `cargo package` on it, and
`cargo package` cannot resolve an unpublished workspace dependency. Both services have one:
`paigasus-iam` depends on `paigasus-iam-core`, and both depend on `paigasus-logging`; each is
Cargo `publish = false`. So `git_only` fails for them.

**So D5 as approved in the brainstorm cannot work.** The source shows one configuration that could
reach the tag path: Cargo `publish` left at its default, with `publish = false` only in
`release-plz.toml`. It is also excluded. It makes the services Cargo-publishable, which brings them
under `repo:publish-metadata`'s `publish = true` checks, and `git_only` still fails on their
unpublished dependencies.

**One side effect on existing text.** `CLAUDE.md` and `rs/release-plz.toml` say that `version_group`
applies to the kernel family's `publish = false` binding crates (measured on 0.3.158). The fixture
had a group with **only** unpublishable members, and `version groups: {}` came out. The two results
do not contradict each other: the kernel group also contains the publishable `paigasus-kernel`.
M7 did not test that case, so this spec makes no claim about it.

**D5, decided at GATE 1: V-a, a version bump by hand, in a normal PR.**

- A maintainer changes the version in the service's `Cargo.toml`, updates `Cargo.lock`, and adds a
  section for the new version to the service's `CHANGELOG.md`. The PR title is
  `chore(rs): release paigasus-<svc> v<version>`.
- The merge makes `plan` select the service, because the new version has no git tag (§ 6.1).
- A person decides the bump level. Library changes do not bump a service by themselves (§ 3.2).
- **The changelog check.** `release_plan.py --assert` (run on every PR by `repo:actionlint`
  check 11) fails when a service's Cargo version is not `0.0.0` and its `CHANGELOG.md` has no
  heading for that version. The plan confirms the exact heading form and that `--assert` runs on
  every PR.
- `rs/release-plz.toml` keeps `release = false` for both services, because `release = true` has no
  effect on them (M7). Its comment changes to say why.

**Rejected alternatives.**

- **V-b: `git-cliff` with `--include-path` and `--bumped-version`.** It automates the bump, but it
  adds a tool, a second author of release PRs next to release-plz, and a second changelog format.
  It stays a possible follow-up (§ 12).
- **V-c: a `workflow_dispatch` bump.** This is V-a with a button. It adds a workflow that has
  `contents: write` for small value.
- **Cargo `publish = true` for the services only** (with `publish = false` in `release-plz.toml`).
  `repo:publish-metadata` selects crates by the Cargo field (`ci/publish-metadata/run.sh:184`), and
  its `cargo publish --dry-run` fails on the four unpublished dependencies (`paigasus-iam-core`,
  `paigasus-logging`, `paigasus-observability`, `paigasus-service-info`). Without `git_only`,
  release-plz still proposes no bump; with it, `cargo package` fails on the same dependencies
  (reasoned from M7, not measured for this case). If it worked, release-plz would make the tag in
  the kernel `release` job, before any image exists (`release.rs:950-987`).
- **Publish the services and their four dependencies to crates.io.** This works: release-plz then
  treats the services like the kernel family. But it makes four internal libraries a permanent
  public API, it needs a manual first publish for each new crate, and it moves the tag before the
  image, into the kernel approval. That is a product decision that is larger than this issue.

### 3.2 No dependency cascade reaches the services

`rs/release-plz.toml:17-22` records that `dependencies_update = true` bumps every transitive
dependent of a bumped crate. That is true for the crates that release-plz processes. The services
are not among them (§ 3.1), so a `paigasus-proto` or `paigasus-kernel` release does **not** bump a
service. Revision 2 of this spec said the opposite before M7 finished; that was wrong.

Under V-a, a person decides when a library change needs a new image.

### 3.3 The first version

**This PR sets both services to `0.1.0` by hand.** The merge of this PR then selects both services
for their first release (§ 10 describes the order of the setup steps). `plan` never selects a
service at version `0.0.0` (§ 6.1).

### 3.4 Consequences of a real version

- **The reported version becomes real.** `service_info.rs` reads `env!("CARGO_PKG_VERSION")`.
  SMA-505 R7 records that ADR-0020 skew reporting is inert while every crate is `0.0.0`. A real
  version is a precondition for skew reporting, but no client implements skew reporting today
  (`ts/packages/paigasus-discovery/src` has no version-skew logic). So this change turns nothing on
  by itself. ADR-0020's "N-1 minor" rule also has no defined meaning between two services with
  independent versions. That gap belongs to whoever implements skew reporting, not to this issue.
- **The service-version assertion can now fail.** The comment at
  `rs/crates/services/paigasus-iam/src/service_info.rs:210` says the assertion is "Still NOT proven
  while every crate is `0.0.0`". After the bump, the service and library versions differ. The
  implementation updates that comment and, where it can now prove it, the test.
- **Places that document the `0.0.0` pin on purpose** change: the comment in `rs/release-plz.toml`,
  the `CLAUDE.md` release-plz entry, and the doc comments in both `service_info.rs` files. M6
  confirms that no gate asserts `0.0.0`.

### 3.5 Tags

- **Git tag:** `paigasus-<svc>-v<version>`, the same form release-plz uses for the kernel family.
  It points at the commit in the image's `org.opencontainers.image.revision` label, **not** at the
  commit of the run that makes the tag. A recovery run can be on a later commit (§ 5).
- **Image tags:**
  - `:<git-sha>` — the build commit. `RUNBOOK-containers.md:43-53` fixes this tag for SMA-513's Helm
    chart, so it stays. It is also the staging tag: the index is first written under it (§ 4.3).
  - `:<version>` — never moves after the first push (D10).
  - `:<major>.<minor>` and `:latest` — move only forward (§ 4.3 step 6). While the version is
    `0.x`, no `:<major>` tag is made, because a `0.x` minor bump can break compatibility.

## 4. Job graph

```
plan ─┬─ wheels / prebuild / proto-dist ─ approve-release ─ release ─ publish-pypi / publish-npm   (unchanged)
      ├─ images-build-iam (arch) ─────── approve-images-iam ─────── publish-images-iam ─────── tag-iam
      └─ images-build-gateway (arch) ─── approve-images-gateway ─── publish-images-gateway ─── tag-gateway
```

- The kernel chain does not change.
- Each service has its own chain of four jobs. The two service chains are the same except for the
  service name. They share their `steps:` through YAML anchors (GitHub Actions supports anchors but
  not merge keys; `CLAUDE.md`). The service name is a job-level `env:` value. PyYAML expands the
  anchors, so `release_guard.py` sees every job in full.
- **Why a chain for each service, and not a matrix.** GitHub fails a job whose matrix is empty
  (challenger B1). A job-level `if:` cannot read `matrix`. One approval job that needs both builds
  is skipped when one build is skipped. A static chain for each service avoids all three problems.
- **Result:** a kernel-only, an image-only, and a combined release all work. A failure in one chain
  does not block another chain. When two chains are pending, the reviewer approves each one.

### 4.1 The gate on each chain

- `plan` writes one output for each service: `skip_iam` and `skip_gateway` (§ 6.1).
- The first job of each chain has exactly `if: needs.plan.outputs.skip_<svc> != 'true'`, in the same
  form as the kernel gate. An **unset** output therefore **runs** the chain. That is safe, because
  the checks in § 4.3 make a run for an already released version a no-op.
- `plan` keeps its single literal `if:` on `PAIGASUS_RELEASE_ENABLED` (V2), so the flag also stops
  every image chain.

### 4.2 `images-build-<svc>` — no push

- **Matrix:** arch only, {amd64 on `ubuntu-latest`, arm64 on `ubuntu-24.04-arm`}, `fail-fast: false`.
  The matrix is static and never empty.
- **Permissions:** `contents: read`. No secret, no environment. `timeout-minutes` set.
- **Build:** buildx to an **OCI archive**, with `--provenance=false --sbom=false`, and the label
  `org.opencontainers.image.version=<version>` in addition to the labels `build_one` sets today.
- **Smoke:** the job loads that same archive into Docker, asserts that the loaded image is the
  archive's image (M3 decides the exact identity check on each runner's image store), and runs the
  smoke suite for this one service.
- **SBOM:** an SPDX SBOM for this architecture (§ 4.5).
- **Outputs:** the per-platform manifest digest is a **job output**. It reaches `publish-images`
  through `needs:`, not through the artifact store, so § 4.3 step 2 can check the artifact hop.
- **Artifact:** the archive, the SBOM and the chisel manifest.

`ci/images/run.sh` changes:

- `build` gets an OCI-archive mode and the version label. The `--load` mode stays for `images.yml`.
- `smoke` gets an optional service argument. The uid loop at `:431` names both containers and
  changes with it. The stale-image guard at `:444-456` stays.
- `assert_base_intact`'s `.Size` check (`:355`) is re-checked under the containerd image store (M3).

### 4.3 `publish-images-<svc>` — after the approval

- **needs:** `plan`, `images-build-<svc>`, `approve-images-<svc>`.
- **Environment:** a **new** environment, `release-images`, with a `main`-only deployment rule and
  **no** required reviewers. Only this environment holds `DOCKERHUB_TOKEN`.
- **Permissions:** `contents: read`, `packages: write`, `id-token: write`, `attestations: write`.
- **Concurrency:** `group: release-images-<svc>`, `cancel-in-progress: false`.
- `timeout-minutes` set. Every action is pinned by SHA.
- **Steps, in order:**
  1. **Fail on an empty token.** No skip-green preflight, unlike `release-pr` (`release.yml:219-227`).
  2. **Check the artifact hop.** Each archive's per-platform digest must equal the build job's
     output. Read the version label from each archive, and compare it with `plan`'s version.
  3. **Check what already exists (D10).** Read the git tag `paigasus-<svc>-v<version>` from the
     remote, and `:<version>` from both registries.
     - The git tag exists → stop with success: "already released".
     - `:<version>` exists in either registry → that digest is the **authoritative** digest. The
       new build is discarded. Go to step 5 with the authoritative digest. Copy it to a registry
       that lacks it.
     - `:<version>` exists in both registries with different digests → fail.
     - Nothing exists → go on with step 4.
  4. **Push to GHCR.** Push the two per-platform images from their archives by digest, then write
     the index under `:<git-sha>` (the staging tag, § 3.5). No release tag is written yet.
  5. **GHCR metadata.** `actions/attest-build-provenance` on the index digest. `actions/attest-sbom`
     for each architecture, on its **per-platform** digest (the subject that the SBOM describes).
     Both with `push-to-registry: true`. `cosign sign` keyless on the index digest in GHCR.
  6. **Docker Hub.** `docker login`, copy the index by digest from GHCR to Docker Hub
     (`docker buildx imagetools create`), `cosign sign` on the Docker Hub digest, then
     `docker logout` at once. The token is on disk only for these steps.
  7. **Release tags.** Write `:<version>` in both registries. Move `:<major>.<minor>` and `:latest`
     **only when** `<version>` is greater than or equal to the highest existing
     `paigasus-<svc>-v*` git tag, compared as semver and read at this step.
  8. **Verify (AC 3).** Against both registries:
     `cosign verify --certificate-identity https://github.com/SMK1085/paigasus-core/.github/workflows/release.yml@refs/heads/main --certificate-oidc-issuer https://token.actions.githubusercontent.com <image>@<digest>`,
     and `gh attestation verify oci://<image>@<digest> --repo SMK1085/paigasus-core --signer-workflow SMK1085/paigasus-core/.github/workflows/release.yml`.
     A failure fails the job, and `tag-<svc>` does not run.
- **What differs between the registries.** The index digest is the same. The stored metadata is
  not: GHCR holds the GitHub attestations and a cosign signature; Docker Hub holds only a cosign
  signature. `gh attestation verify` reads from the GitHub API, so it works for an image from either
  registry. Registry-stored attestations exist only on GHCR.
- **Tools.** cosign, syft and the archive push tool (`crane`, `skopeo` or `regctl`, chosen by M2)
  are pinned as proto plugins, the same way as the other CLIs in this repo.
- **GHCR login** uses `github.token`, not `secrets.GITHUB_TOKEN`. The second form is a `secrets`
  reference, and V10 rule 1 (`release_guard.py:224-227`) would red it.

### 4.4 `tag-<svc>` — after the push

- **needs:** `publish-images-<svc>`. **Environment:** the existing `release-publish`.
- **Permissions:** `contents: read` and no `id-token: write`. Twelve trusted publishers trust the
  `release-publish` OIDC claim, so a job there that does not publish must not be able to mint one.
- It mints the App token with `permission-contents: write` only, and makes the tag through
  `gh api repos/{owner}/{repo}/git/refs` on the image's revision label (§ 3.5). No `git push`.
- **Idempotent:** a tag that already points at the same commit is success. A tag that points at
  another commit is a failure.
- **No GitHub Release.** A `gh release create` would take the "Latest" badge from the kernel
  release. The changelog is in the crate's `CHANGELOG.md`.
- `timeout-minutes` set. No tag ruleset exists today (the only ruleset is "Protect main", target
  `branch`, read on 2026-09-19), and the `release` job already makes tags with the same App scope.

### 4.5 The SBOM

The challenger found that the SBOM is probably almost empty. The chisel cut does not include
`base-files_chisel`, so the image has no chisel manifest and no dpkg status, and the binary is a
plain `cargo build`, not `cargo auditable`.

**Requirement (AC 7):** each SBOM lists the libc6 package and the Rust crates in the binary. M8
measures what syft finds today. If it finds too little, the plan adds, in this order of
preference: `cargo auditable build` (the crates), and the `base-files_chisel` slice (the Ubuntu
packages). `assert_pins` changes with the Dockerfile if the slice list changes.

## 5. Recovery

**D10 is the recovery model.** A rebuild does not give the same digest: `chisel cut` resolves the
live Ubuntu archive on each build (`rs/Dockerfile:36-52`, `--no-cache-filter=rootfs`), and the build
sets no `SOURCE_DATE_EPOCH`. So "push the same digest again" is not a safe assumption. Instead, the
first digest under `:<version>` is final, and every later run adopts it (§ 4.3 step 3).

| Failure | State after it | Recovery |
|---|---|---|
| Build or smoke fails | Nothing pushed, nothing tagged | Fix, merge, and the next run tries again |
| Push to GHCR fails | Maybe an untagged digest under `:<git-sha>` on GHCR | "Re-run failed jobs" in the same run (same artifacts) |
| Docker Hub copy fails | `:<git-sha>` on GHCR; no `:<version>`, no git tag | "Re-run failed jobs" |
| Verification fails | `:<version>` exists, no git tag | Investigate first. A re-run adopts the existing digest and verifies it again |
| `tag-<svc>` fails | Images public, no git tag | "Re-run failed jobs", or a new run: it adopts the existing digest and makes the tag on its revision |

Two runs for the same untagged version (a time-of-check gap between `plan` and a late approval) are
serialized by the concurrency group. The second run adopts the first run's digest in step 3.

## 6. Changes to `ci/release-plan/`

### 6.1 The service outputs

- New outputs `skip_iam` and `skip_gateway`, each `true` or `false`.
- `skip_<svc>=true` when the service's Cargo version is `0.0.0` **or** its git tag already exists on
  the remote. Otherwise `false`.
- **A separate failure domain.** Service collection runs in its own `try`. A failure there writes
  `skip_<svc>=false` for that service (the fail-safe direction, § 4.1) and does not change the
  kernel verdict. The synthetic trees in negative-control rows 3 and 4 have no service crates; their
  kernel verdict must not change.
- **A strict pin.** `EXPECTED_SERVICES = {"iam": "paigasus-iam", "gateway": "paigasus-gateway"}`.
  A service crate that is missing from the tree is an inconclusive result for that service.
- `github_output`'s fail-safe branch (`ci/release-plan/run.sh:78-98`) also writes `skip_iam=false`
  and `skip_gateway=false`.
- On `workflow_dispatch` the service outputs follow the same rule. A dispatch therefore runs a
  chain only for a version with no tag, and § 4.3 step 3 makes that run safe.

### 6.2 Tests

- `release_plan_self_test` (check 11) gets rows: no tag → run; tag exists → skip; `0.0.0` → skip;
  both services; neither service; a service-collection failure that leaves the kernel verdict
  unchanged.
- A negative-control row for the new outputs.
- `RELEASE_PLAN_SH_CALL_SITES` in `ci/affected-graph/ci_targets.py:1180-1192` pins `run.sh` lines.
  The lines that change are re-pinned there, so `repo:affected-smoke` also changes in this PR.

## 7. Guards (`ci/actionlint/release_guard.py`)

### 7.1 Approval and publish rules

- **V8, generalized.** Today V8 knows one approval job. It learns a map from each chain to its
  approval job: `approve-release` for the kernel chain, `approve-images-<svc>` for each service
  chain. Every approval job uses the `release-approval` environment (V8a). An image publisher must
  be downstream of **its own service's** approval job. The kernel approval does not count.
- **V9b.** Every direct consumer of `plan` has exactly one of the accepted gate literals: the
  kernel literal, or `needs.plan.outputs.skip_<svc> != 'true'` for a known service. The V9c
  full-match pin extends to the new `outputs:` expressions of `plan`, so a
  `${{ steps.decide.outputs.skip_iam || 'true' }}` form reds (the SMA-603 2d class).
- **New V14: capability rule.** A job that has `packages: write`, `attestations: write` or
  `id-token: write`, names `release-images` or `release-publish`, or mints an App token with
  `contents: write`, must have an approval job on its `needs:` path. `release-pr` stays the one
  `UNGATED_JOBS` member. This rule does not depend on how a command is spelled, so it covers what a
  marker list cannot see: `with: push: true`, a command inside a script, a new tool.
- **Publish markers, as a second layer.** The detector learns: `docker push`,
  `docker buildx build … --push` and `--output type=registry`, `docker manifest push`,
  `imagetools\s+create`, `crane (push|copy|cp|tag|index|append)`, `skopeo copy`,
  `regctl (image (copy|cp)|tag|index create)`, `oras (push|cp|attach)`, `cosign (sign|attest|attach|copy)`,
  and a `git/refs` API call. Each marker is a regex with bounded ends (SMA-579). A fixture keeps
  `release-pr`'s branch push (`release.yml:396`) green.
- **Registry commands are literal.** In `release.yml`, a registry command must appear in the
  workflow text, not in a called script. `run.sh rehearse` (§ 8) is used by `images.yml` only.

### 7.2 Credentials

- **V10.** `EXPECTED_RELEASE_SECRETS` gets `DOCKERHUB_TOKEN`, with a comment that states the reason
  from § 2 (D7).
- **New V13: credential scope.**
  - `DOCKERHUB_TOKEN` may appear only in a job whose environment is `release-images`, and only the
    `publish-images-<svc>` jobs may name that environment.
  - Environment names are compared **case-folded**, because GitHub treats them case-insensitively.
  - An `environment:` value that contains `${{` fails closed.
  - V13 runs over **every** file in `.github/workflows/`, not only `release.yml`, because any
    workflow with a `main` trigger could name `release-images`.
- **V11.** The `publish-images-<svc>` jobs join the jobs that must hold `id-token: write`.
- **`tag-<svc>` must not hold `id-token: write`** (§ 4.4). A new row asserts it.
- **Not a guard, a runbook read-back:** no repository or organization secret named
  `DOCKERHUB_TOKEN` exists (none exists today: the repository has no Actions secrets). A secret with
  the same name at a wider scope would defeat the environment scoping.

### 7.3 Tests for the guard

- Each change gets rows in the existing `release_guard_self_test` table: one that must red and one
  that must stay green. The V13 rows include the case-fold, the `${{` and the other-workflow shapes.
  The V14 rows include a publish inside a script and a `with: push: true`. No new `*_self_test`
  table is added, so `SELF_TEST_COUNT` (16) does not change.
- The floating-tag comparison in § 4.3 step 7 is a small script with its own fixture rows
  (`0.2.0` vs `0.10.0`, equal versions, no tag yet).
- A mutation pass: delete each new marker, each new rule line and each new gate literal, one at a
  time, and confirm that a fixture reds each time. The whole battery runs again after every fix.

### 7.4 Registry obligations

This design adds **no** new `repo:*` Moon task. The `T=(…)` array, the `CLAUDE.md` marker command,
`SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS` and `REQUIRED_REPO_TASKS` do not change. The pin
tables that do change are `RELEASE_PLAN_SH_CALL_SITES` (§ 6.2) and any `release_guard.py` pin in
`ci/actionlint/run.sh` whose line moves.

## 8. Tests for the image path

- **`images.yml` runs the release build path.** It changes from `run.sh all` (`--load`) to the same
  path as § 4.2: OCI archive, load, identity check, per-service smoke. On a pull request it runs
  amd64. On a push to `main` and on `workflow_dispatch` it also runs an arm64 leg.
- **`run.sh rehearse`** runs § 4.3 steps 3, 4, 6 and 7 against **two local `registry:2`
  containers**, digest-pinned like the other images in `ci/images/run.sh:21-30`. It asserts the same
  digest in both registries, the D10 adoption case, and the floating-tag rule. It needs no
  credential, so `images.yml` runs it on a pull request.
- **A rehearsal workflow for the parts that need OIDC.** A new workflow,
  `.github/workflows/images-rehearsal.yml`, with `workflow_dispatch` only (no `pull_request`, so
  `repo:workflow-credentials` does not apply), restricted to `main`. It pushes one image to a
  scratch GHCR package, `ghcr.io/smk1085/paigasus-rehearsal`, with `github.token`, then runs
  `cosign sign`, `attest-build-provenance`, `attest-sbom`, `cosign verify` and
  `gh attestation verify` for real. So the signing and attestation steps have run once before the
  first real release. The Docker Hub login and `tag-<svc>` still have no rehearsal; the first real
  release is their first test.

## 9. Measurements before the plan

A result that contradicts this spec changes the spec first.

| # | Question | Why it matters |
|---|---|---|
| M1 | ~~Does a `Cargo.lock`-only change bump a service?~~ Answered by M7: release-plz does not process the services at all. | — |
| M2 | Which of `crane`, `skopeo`, `regctl` pushes a buildx OCI archive by digest, and has a proto plugin? | § 4.3 step 4. |
| M3 | On the Docker version of each runner label (`ubuntu-latest`, `ubuntu-24.04-arm`): after `docker load` of a buildx OCI archive, which identity equals which archive digest (classic store: config digest; containerd store: manifest or index digest)? What does `.Size` mean? | § 4.2 identity check, `assert_base_intact`. |
| M4 | Does `docker buildx imagetools create` from GHCR to Docker Hub keep the index digest byte for byte? | AC 2. |
| M5 | Does the per-service smoke pass on `ubuntu-24.04-arm`? | The arm64 leg has never run. |
| M6 | Does any gate assert the services' `0.0.0`? | § 3.4. |
| M7 | release-plz 0.3.158 and a Cargo `publish = false` crate. | **Done** (§ 3.1): never bumped, never tagged; `git_only` fails on the second release. |
| M8 | What does syft list for the archive today? | § 4.5, AC 7. |
| M9 | Does an environment secret reach a job in `release.yml` with `environment: release-images` when the environment has no reviewers and a `main`-only rule? (Expected yes; confirm in the rehearsal.) | AC 5. |

## 10. Rollout order

`PAIGASUS_RELEASE_ENABLED` is already on, and the `0.1.0` bump (§ 3.3) selects both services on
the merge that contains it. The rehearsal workflow (§ 8) runs only from `main`. So the feature ships
in **two pull requests**:

- **PR 1 — tooling, no release.** The proto pins (cosign, syft, the push tool), the `ci/images/run.sh`
  changes, the `images.yml` change, and `images-rehearsal.yml`. Both services stay at `0.0.0`, and
  `release.yml` does not change. Nothing can publish a release image.
- **PR 2 — the release path.** The `release.yml` chains, the `ci/release-plan/` outputs, the guard
  changes, the D5 mechanism, and the `0.1.0` bump.

The order of the steps:

1. Merge PR 1.
2. Run `images-rehearsal.yml` on `main`. It must pass before PR 2 merges.
3. Make the Docker Hub repositories `smaschek/paigasus-iam` and `smaschek/paigasus-gateway`
   (public).
4. Make the `release-images` environment with a `main`-only deployment rule and no reviewers. If a
   job names an environment that does not exist, GitHub makes it with **no** branch rule, so this
   step comes before PR 2.
5. Make the Docker Hub PAT (Read & Write) and store it as `DOCKERHUB_TOKEN` in `release-images`
   only. Record its expiry date.
6. Read back: no repository or organization secret named `DOCKERHUB_TOKEN`.
7. Merge PR 2. Approve each service chain.
8. After the first GHCR push, set each GHCR package to **public** and link it to the repository.
   GitHub makes a new package private. Until this step, Docker Hub is public and GHCR is not.

## 11. Acceptance criteria

1. A merged release PR that bumps one service publishes exactly that service's image, and no other
   image.
2. The image is a multi-arch (amd64 + arm64) index, with the same digest on Docker Hub and GHCR.
3. The two § 4.3 step 8 commands pass on the published digest, against both registries.
4. Nothing is pushed or tagged before the service's own approval job, and `release_guard.py`
   asserts it (V8, V14). The `release-approval` environment has a required reviewer (`SMK1085`,
   read on 2026-09-19).
5. `DOCKERHUB_TOKEN` is readable only by the `publish-images-<svc>` jobs, and it is the only new
   secret name in `release.yml`. V13 asserts it.
6. The git tag is made only after the push to both registries and the verification pass.
7. Each SBOM lists libc6 and the Rust crates of the binary.
8. A published `:<version>` never changes digest. `run.sh rehearse` asserts the adoption case.
9. A kernel-only release, an image-only release and a combined release each complete, and a failed
   image chain does not stop the kernel chain. Guard fixture rows cover the three graph shapes.

## 12. Out of scope and follow-ups

- **Docker Hub OIDC.** A move to a Docker Hub organization with OIDC removes the stored token and
  its namespace-wide scope. The namespace then changes.
- **Images for the TS consoles.** No Dockerfile exists for `iam-console` or `gateway-console`.
- **Reproducible builds** (`SOURCE_DATE_EPOCH`, a pinned chisel snapshot). D10 makes them
  unnecessary for correctness.
- **Retention.** No cleanup of old image tags.
- **An automated service bump** (V-b, `git-cliff`), if the manual bump becomes a burden.
- **zizmor.** The GitHub-expression interpolation class in `release.yml` stays unlinted.

## 13. ADR

`release.yml:1066-1068` and V5 (`release_guard.py:704`) cite ADR-0011 S3: "release-plz owns every
tag". `tag-<svc>` makes a second tag owner, and the service chains add release families (S1). The
repo convention requires a Notion ADR change before code. **Precondition for the plan:** amend
ADR-0011 S1 and S3 in Notion, and reword the V5 message to match.

## 14. Documentation changes

- `docs/ops/RUNBOOK-containers.md`: both registry names, the tag scheme, the verification commands,
  § 10, the token rotation step and its namespace-wide scope.
- `CLAUDE.md`: update the release-plz `0.0.0` entry. Record the M7 result: release-plz does not
  process a Cargo `publish = false` crate, and `git_only` fails on its second release when it has
  an unpublished workspace dependency. Add an entry for the service chains, D10 and
  the separate approvals.
- The comments in `rs/release-plz.toml`, in both `service_info.rs` files, and in `release.yml`'s
  header.

## 15. Challenge changelog (revision 1 → 2)

| Finding | Action |
|---|---|
| B1 graph drops or reds single-family releases | Folded: § 4, D9, AC 9. Option (a) rejected (§ 2). |
| B2 re-run not idempotent | Folded: D10, § 4.3 step 3, § 5, AC 8. |
| B3 release-plz may not bump the services | Confirmed by M7, and worse: it never can. § 3.1 rewritten; Sven chose V-a at GATE 1. |
| M1 cascade already measured | Rejected after M7: the cascade does not reach the services (§ 3.2). |
| M2 floating tags move backwards | Folded: concurrency group, semver rule, fixture rows. |
| M3 ADR-0011 | Folded: § 13, precondition. |
| M4 detector gaps | Folded: V14 capability rule, extra markers, literal commands. |
| M5 V13 bypasses | Folded: case-fold, `${{`, all workflow files, `github.token`, read-back. |
| M6 SBOM | Folded: § 4.5, per-platform subject, M8, AC 7. |
| M7 rollout order | Folded: § 3.3, § 10, empty-token failure, `0.0.0` never selected. |
| M8 PR tests | Folded: § 8, `images.yml` runs the release path, rehearsal workflow. |
| M9 `services` output | Folded: § 6, per-service outputs, failure domain, pins. |
| Minor: token on disk | Folded: `docker logout` at once (§ 4.3 step 6). |
| Minor: tool pins, AC 3 commands, registry difference, staging tag, containerd, uid loop, version label, artifact hop, `:<git-sha>`, `tag` rules, PAT scope, `registry:2` pin, env reviewers, GHCR private | Folded in the named sections. |
| Minor: "Latest" badge | Folded: no GitHub Release (§ 4.4). |
| Minor: skew reporting "turns on" | Folded: § 3.4 corrected. |
| Minor: GHCR-only alternative | Rejected: Sven chose both registries (§ 2). |
| Q: reviewers on `release-approval` | Answered: `SMK1085` (read from GitHub). |
| Q: tag ruleset | Answered: none (read from GitHub). |
| Q: one approval for both families | Answered by D9: no. |
| Q: dispatch selection | Answered: § 6.1. |
| Q: D7 source | Answered: § 2 links. |
| Q: GHCR packages exist? | Open: the local `gh` token has no `read:packages` scope. Sven checks. |
