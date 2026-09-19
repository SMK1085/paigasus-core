# SMA-658 — Release the service container images to Docker Hub and GHCR

- **Linear:** [SMA-658](https://linear.app/smaschek/issue/SMA-658)
- **Related:** SMA-500 (the image build), SMA-505 R7 (the `0.0.0` version), SMA-580 (release-plz tags
  only what it publishes), SMA-602 (V10, no publish credential), SMA-603 (`ci/release-plan/`).
- **Status:** draft. Sven approved the three design sections in the brainstorm on 2026-09-19. The
  written spec waits for his review.
- **Approach chosen by Sven:** A — new jobs in `release.yml` (option 1 of 3, 2026-09-19).

## 1. Problem

SMA-500 builds and smoke-tests two images, `paigasus-iam` and `paigasus-gateway`, from one
`rs/Dockerfile`. `.github/workflows/images.yml` has `contents: read` only and pushes nothing. The
SMA-500 spec deferred "push, credentials, semver tags, retention and signing" (its lines 317 and
674). A user cannot pull a Paigasus image from any registry today.

The two services also have no release version. Both crates stay at `0.0.0`, and `rs/release-plz.toml`
sets `release = false` for them. So an image release has no version to carry.

## 2. Decisions (brainstorm, 2026-09-19)

| # | Topic | Decision |
|---|---|---|
| D1 | Trigger | A service version that has no tag `paigasus-<svc>-v<version>` yet. |
| D2 | Registries | Docker Hub `docker.io/smaschek/paigasus-{iam,gateway}` and GHCR `ghcr.io/smk1085/paigasus-{iam,gateway}`. Both registries get the same digest. |
| D3 | Architectures | `linux/amd64` and `linux/arm64`, built on native runners (`ubuntu-latest`, `ubuntu-24.04-arm`), joined in one image index. |
| D4 | Metadata | GitHub build provenance and SBOM attestations, and a cosign keyless signature on the index digest. |
| D5 | Versioning | release-plz bumps each service's Cargo version and changelog. A workflow job makes the tag. |
| D6 | Version relation | Independent. Each service has its own `version_group`. An IAM change does not publish a gateway image. |
| D7 | Docker Hub credential | A personal access token with Read & Write scope, stored as an environment secret. |
| D8 | Workflow shape | New jobs in `release.yml` (approach A). |

**Why D7 needs a stored secret.** Docker Hub supports OIDC for GitHub Actions through
`docker/login-action` v4.5.0 or later. But Docker Hub offers OIDC connections only to organizations
with a Team, Business or DHI subscription, or in the Docker Sponsored Open Source program. A free or
Pro personal account, such as `smaschek`, cannot use it. So the Docker Hub path needs a token, and the
token is an exception to V10 (§ 6.2). GHCR uses the workflow's `GITHUB_TOKEN` and needs no secret.
A move to an organization with OIDC is a follow-up (§ 10).

**Rejected approaches.**

- **B — a separate `images-release.yml` on a tag push.** `release_guard.py` does not check that
  workflow, so it needs its own approval environment and its own enable switch. The tag also exists
  before the push, so a failed push leaves a tag with no image. This is the opposite of the order in
  `release.yml`: reversible steps first, irreversible steps last.
- **C — a push mode in `images.yml`.** Not possible. `images.yml` has a `pull_request` trigger, and
  `repo:workflow-credentials` bans every secret in such a workflow.

## 3. Service versions and the release decision

### 3.1 release-plz

In `rs/release-plz.toml`, `paigasus-iam` and `paigasus-gateway` change from `release = false` to
`release = true`. Each gets its own `version_group`: `iam` and `gateway`. `publish = false` stays,
because the services do not go to crates.io. `release-plz release` needs that key to agree with
the Cargo manifest (SMA-579).

The release PR then bumps each service's Cargo version and writes its `CHANGELOG.md` from the
conventional commits that touch the crate.

**Both services start at `0.1.0`**, the same first version as the kernel family.

### 3.2 Consequences of a real version

- **ADR-0020 skew reporting turns on.** `service_info.rs` reads `env!("CARGO_PKG_VERSION")`. SMA-505
  R7 records that skew reporting is "inert" while every crate is `0.0.0`. A real version is what the
  design intended, so this is a wanted change, not a side effect.
- **The service-version assertion can now fail.** The comment at
  `rs/crates/services/paigasus-iam/src/service_info.rs:210` says the assertion is "Still NOT proven
  while every crate is `0.0.0`". After the bump, the service version and the library version can
  differ. The implementation updates that comment and, where the test can now prove it, the test.
- **Three places document the `0.0.0` pin on purpose** and must change: the comment in
  `rs/release-plz.toml`, the `CLAUDE.md` entry on `rs/release-plz.toml`, and the doc comments in both
  `service_info.rs` files. `repo:version-lockstep` covers the kernel family only; the plan confirms
  that it asserts nothing about the services.

### 3.3 The release decision

`ci/release-plan/` decides a kernel release today by tag existence (SMA-603). It gets a second
output, `services`: a JSON list such as `["iam"]` or `[]`.

- A service is in the list when its Cargo version has no `paigasus-<svc>-v<version>` tag on the
  remote.
- An empty list skips every image job. A skipped job is green.
- The kernel release and the image release are independent. One run can release only the kernel,
  only one image, or both.
- The `plan` job keeps its single literal `if:` gate (V2). `PAIGASUS_RELEASE_ENABLED` therefore
  also switches the image path off.

### 3.4 Tags

- **Git tag:** `paigasus-<svc>-v<version>`, the same form release-plz uses for the kernel family.
- **Image tags:** `:<version>`, `:<major>.<minor>` and `:latest`. While the version is `0.x`, no
  `:<major>` tag is made, because a `0.x` minor bump can break compatibility.
- The git tag is made **last**, after the push to both registries and the verification pass (AC 6).
  A re-run after a partial failure is therefore safe: the tag does not exist, so `plan` selects the
  service again, and a push of the same digest again changes nothing.

## 4. Job graph

```
plan ─┬─ wheels / prebuild / proto-dist ──┐
      └─ images-build (svc × arch) ───────┴─ approve-release ─┬─ release → publish-pypi / publish-npm
                                                              └─ publish-images (svc) → tag-services (svc)
```

`images-build`, `publish-images` and `tag-services` are new.

### 4.1 `images-build` — before the gate, no push

- **Matrix:** `fromJSON(needs.plan.outputs.services)` × {amd64 on `ubuntu-latest`, arm64 on
  `ubuntu-24.04-arm`}. `fail-fast: false`. An empty list skips the job.
- **Permissions:** `contents: read`. No secret, no environment.
- **Build:** buildx to an **OCI archive**, with `--provenance=false --sbom=false`. The layer bytes
  are fixed at build time, so the digest is known before any push. Buildx's own attestations are off
  because D4 uses GitHub attestations; two sets of attestations on one index is not useful.
- **Smoke:** the job loads that same archive into Docker and runs the smoke suite on it. It asserts
  that the loaded image ID equals the config digest in the archive. So the tested image is the
  published image.
- **SBOM:** an SPDX SBOM from the archive, made by a pinned syft.
- **Artifact:** the archive, the SBOM and the chisel manifest, one artifact for each (service, arch).

`ci/images/run.sh` changes:

- `build` gets an OCI-archive output mode. The current `--load` mode stays for `images.yml`.
- `smoke` gets an optional service argument. Today it always smokes both services, and a release
  of IAM alone has no gateway image to smoke.
- `assert_pins` and the chisel-manifest extraction run unchanged in both modes.
- The `PAIGASUS_IMAGE_REGISTRY` default stays `ghcr.io/smk1085`. The release path names both
  registries explicitly; it does not read that default.

### 4.2 `publish-images` — after the gate, one job for each service

- **needs:** `plan`, `images-build`, `approve-release`.
- **Environment:** a **new** environment, `release-images`, with a `main`-only deployment rule. Only
  this environment holds `DOCKERHUB_TOKEN`, so the `release` job cannot read it (AC 5).
- **Permissions:** `contents: read`, `packages: write`, `id-token: write`, `attestations: write`.
- **Steps, in order:**
  1. Push the two architecture images from their archives to GHCR by digest. Make the image index
     there.
  2. **Copy** the index by digest from GHCR to Docker Hub (`docker buildx imagetools create`). A
     copy keeps the bytes, so the digest is the same in both registries (AC 2). A second
     `docker push` does not guarantee that, because Docker compresses the layers again on each push.
  3. Add the § 3.4 image tags to the index in both registries.
  4. `actions/attest-build-provenance` and `actions/attest-sbom` on the index digest, with
     `push-to-registry: true` on GHCR. `cosign sign` keyless on the digest in both registries.
  5. Verify: `cosign verify` and `gh attestation verify` on the pushed digest (AC 3). A failure
     fails the job, and `tag-services` does not run.
- **The archive push tool** (step 1) is `crane`, `skopeo` or `regctl`, pinned. Measurement M2 (§ 8)
  picks the one that pushes an OCI archive by digest with the least new pinning.

### 4.3 `tag-services` — after the push

- **needs:** `publish-images`.
- **Environment:** the existing `release-publish`, which holds the App secrets.
- It mints the App token with `permission-contents: write` only. It makes
  `paigasus-<svc>-v<version>` on the release commit, and a GitHub Release with the changelog
  section and the image digest.
- Two jobs, not one, keep the Docker Hub token and the App key in different environments. `needs:`
  also enforces AC 6.

## 5. Failure modes

| Failure | State after it | Recovery |
|---|---|---|
| Build or smoke fails | Nothing pushed, nothing tagged | Fix, merge, run again |
| GHCR push passes, Docker Hub copy fails | An untagged digest on GHCR, no git tag | Run again: `plan` selects the service again, the push is idempotent |
| Verification fails | Images exist, no git tag | Investigate first. A new run pushes the same digest again |
| `tag-services` fails | Images are public, no git tag | `workflow_dispatch`: `plan` selects the service, the push is idempotent, the tag follows |

## 6. Guards

### 6.1 Publish detector (`ci/actionlint/release_guard.py`)

The detector learns the container publish markers: `docker push`, `docker buildx imagetools create`,
`crane push|copy`, `skopeo copy`, `regctl image copy`, `oras push`, `cosign sign`, and
`docker/build-push-action` with `push: true`. It also learns the tag markers that `tag-services`
uses: a `git push` of a tag ref, `gh release create`, and a `git/refs` API call.

V8b and V8c then require each of these to be downstream of `approve-release` (AC 4). The markers
are regexes with bounded ends, the same as the `release-plz release` marker (SMA-579), so a longer
command name that starts with a marker does not match it.

### 6.2 V10 and a new V13

- **V10:** `EXPECTED_RELEASE_SECRETS` gets `DOCKERHUB_TOKEN`. A code comment records the reason
  from § 2 (D7).
- **V13 — credential scope (new):** `DOCKERHUB_TOKEN` may appear only in a job whose environment is
  `release-images`, and only `publish-images` may name that environment. Without V13, V10 accepts
  the token in any job, and AC 5 has no control.
- **V11:** `publish-images` joins the jobs that must hold `id-token: write`. cosign keyless and the
  attestations need it.

### 6.3 Tests for the guard

- Each change gets rows in the existing `release_guard_self_test` table: one row that must red and
  one row that must stay green. No new `*_self_test` table is added, so `SELF_TEST_COUNT` (16) does
  not change.
- A mutation pass: delete each new marker and each new rule line, one at a time, and confirm that a
  fixture reds each time. The whole battery runs again after every fix.

### 6.4 `ci/release-plan/`

The `services` output gets rows in `release_plan_self_test` (check 11): no tag → selected; a tag
exists → not selected; both services; neither service. It also gets a negative-control row.

### 6.5 Registry obligations

This design adds **no** new `repo:*` Moon task. The `T=(…)` array, the `CLAUDE.md` marker command,
`SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS` and `REQUIRED_REPO_TASKS` do not change. The new
checks live in two existing gates, `repo:actionlint` (checks 10 and 11), and in `images.yml`.

## 7. Tests for the image path

- `images.yml` keeps `run.sh all`, so the current PR coverage does not change.
- A new mode, `run.sh rehearse`, runs steps 1–3 of § 4.2 against **two local `registry:2`
  containers**: OCI archive → registry A by digest → index → copy to registry B. It asserts the same
  digest in both registries and all three image tags. It needs no credential, so `images.yml` can
  run it on a PR without a conflict with `repo:workflow-credentials`.
- **Limit.** cosign keyless, the attestations, the Docker Hub login and `tag-services` need a real
  OIDC token or a real secret. No PR test covers them. The first live release is their first real
  test, the same as for SMA-580. The acceptance evidence for AC 1, AC 3, AC 5 and AC 6 is therefore
  that first live run.

## 8. Measurements before the plan

Each one is taken before the plan is written. A result that contradicts this spec changes the spec
first.

| # | Question | Why it matters |
|---|---|---|
| M1 | With `dependencies_update = true`, does a change in a library crate that a service depends on bump the service in the release PR (release-plz 0.3.158, local fixture)? | If yes, a library change publishes a new image. The spec must then say whether that is wanted or how to stop it. |
| M2 | Which of `crane`, `skopeo`, `regctl` pushes a buildx OCI archive by digest, and which is already pinnable through proto or a digest-pinned action? | § 4.2 step 1. |
| M3 | Does `docker load` of a buildx OCI archive give an image ID equal to the archive's config digest, on the runner's Docker version? | § 4.1's "tested image = published image" assertion depends on it. |
| M4 | Does `docker buildx imagetools create` from GHCR to Docker Hub keep the index digest byte for byte? | AC 2. |
| M5 | Does the smoke suite pass on `ubuntu-24.04-arm` (Postgres image, chisel arm64 checksum, the size limit)? | The arm64 leg has never run. |
| M6 | Does `repo:version-lockstep` or any other gate assert the services' `0.0.0`? | § 3.2. |

## 9. Acceptance criteria

1. A merged release PR that bumps one service publishes exactly that service's image, and no other
   image.
2. The image is a multi-arch (amd64 + arm64) index, with the same digest on Docker Hub and GHCR.
3. `cosign verify` and `gh attestation verify` pass on the published digest.
4. Nothing is pushed or tagged before `approve-release`, and `release_guard.py` asserts this.
5. `DOCKERHUB_TOKEN` is readable only by `publish-images`, and it is the only new secret name in
   `release.yml`. V13 asserts this.
6. The git tag is made only after the push to both registries and the verification pass succeed.

## 10. Out of scope and follow-ups

- **Docker Hub OIDC.** A move to a Docker Hub organization (for example through the Docker Sponsored
  Open Source program) with OIDC removes the stored token. The namespace then changes.
- **Images for the TS consoles.** No Dockerfile exists for `iam-console` or `gateway-console`, and
  neither sets `output: 'standalone'`.
- **Retention.** No cleanup of old image tags.
- **zizmor.** The GitHub-expression interpolation class in `release.yml` stays unlinted (see the
  `repo:actionlint` shellcheck entry in `CLAUDE.md`).

## 11. One-time setup (runbook, not code)

1. Make the Docker Hub repositories `smaschek/paigasus-iam` and `smaschek/paigasus-gateway` as
   public repositories.
2. Make a Docker Hub personal access token with Read & Write scope. Store it as `DOCKERHUB_TOKEN` in
   the `release-images` environment only.
3. Make the `release-images` environment with a `main`-only deployment rule.
4. After the first GHCR push, set each GHCR package to **public** and link it to the repository.
   GitHub makes a new package private, even when the source repository is public.
5. Record the token's expiry date and the rotation step in `docs/ops/RUNBOOK-containers.md`.

## 12. Documentation changes

- `docs/ops/RUNBOOK-containers.md`: both registry names, the tag scheme, the `cosign verify` and
  `gh attestation verify` commands, § 11, and the token rotation step.
- `CLAUDE.md`: update the entry that says gateway and IAM stay at `0.0.0`. Add an entry: release-plz
  does not tag the services, and `tag-services` makes their tags.
- The comments in `rs/release-plz.toml` and in both `service_info.rs` files.
