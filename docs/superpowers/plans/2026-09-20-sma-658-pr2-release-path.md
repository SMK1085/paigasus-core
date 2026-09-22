# SMA-658 PR 2 — The service image release path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **First action of every implementer:** a subagent starts pinned to the MAIN checkout. Before
> anything else, run EnterWorktree with `path` =
> `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-658-release-path`, then confirm
> `git branch --show-current` prints `feature/sma-658-release-path`. Stop if it does not.
> **Add commits, never amend.** Run every step in the foreground.

**Goal:** Publish each service image to Docker Hub and GHCR from `release.yml`, behind its own
approval, with attestations, a cosign signature and a git tag — and let the guard prove that no
step can reach a registry before a human approves it.

**Architecture:** `plan` gains one skip flag and one version for each service. Each service gets a
four-job chain in `release.yml`: build (no push) → approve → publish → tag. The publish job copies
the push/attest/sign/verify sequence that `images-rehearsal.yml` already ran for real on `main`.
`release_guard.py` learns a per-chain approval map (V8), a credential-scope rule (V13) and a
capability rule (V14), so the guarantees do not depend on how a command is spelled. The Dockerfile
builds with `cargo auditable`, so the SBOM lists the crates.

**Tech Stack:** GitHub Actions (YAML anchors, no merge keys), Python 3.12 stdlib, bash, Docker
buildx, crane 0.22.1, cosign 3.1.3, syft 1.52.0, cargo-auditable 0.7.6, proto 0.61.1.

**Spec:** `docs/superpowers/specs/2026-09-19-sma-658-container-release-design.md` (approved at
GATE 1). This plan covers **PR 2 only** (spec § 10). PR 1 is merged (`ff926404`), so `crane`,
`cosign`, `syft`, `ci/images/release_decision.py` and `ci/images/run.sh`'s `build-oci`, `load-oci`,
`smoke` and `rehearse` are all present.

## Global Constraints

- The merge of this PR publishes the first two real images. Every step assumes that.
- Every new source file opens with the SPDX header: `# SPDX-License-Identifier: Apache-2.0`.
- Every action is pinned by a 40-character commit SHA with a `# vX.Y.Z` comment. Reuse the SHAs
  already in the repository:
  - `actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1`
  - `actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1`
  - `actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1`
  - `actions/attest-build-provenance@4d101475d8b20a2381f78447822ac1eab6504dd8  # v4.2.2`
  - `actions/attest-sbom@c604332985a26aa8cf1bdc465b92731239ec6b9e  # v4.1.0`
  - `actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1  # v3.2.0`
  - `docker/login-action@dbcb813823bdd20940b903addbd779551569679f  # v4.6.0`
  - `docker/setup-buildx-action@37fe631027851001ddb9b187196cc803df7f5f0e  # v4.3.0`
  - `moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0`
- Tool versions: crane `0.22.1`, cosign `3.1.3`, syft `1.52.0`, **cargo-auditable `0.7.6`** (read
  from crates.io on 2026-09-20: `max_stable_version` is `0.7.6`).
- Registries: `ghcr.io/smk1085/paigasus-<svc>` and `docker.io/smaschek/paigasus-<svc>`.
- cosign identity for `release.yml`:
  `https://github.com/SMK1085/paigasus-core/.github/workflows/release.yml@refs/heads/main`,
  issuer `https://token.actions.githubusercontent.com`.
- GHCR login uses `${{ github.token }}`, **never** `secrets.GITHUB_TOKEN` (V10 rule 1 pins the
  secret names, and `secrets.GITHUB_TOKEN` is a `secrets` reference).
- `DOCKERHUB_TOKEN` appears only in `publish-images-<svc>`, whose environment is `release-images`.
- `release.yml` may name only these secrets: `PAIGASUS_BOT_APP_ID`, `PAIGASUS_BOT_PRIVATE_KEY`,
  `DOCKERHUB_TOKEN`.
- Registry commands appear literally in `release.yml`, never inside a called script (spec § 7.1).
- Shell rules (`CLAUDE.md`): no pipe into an early-exit reader (`grep -q`, `grep -m`, `head`,
  `awk … exit`, a `sed` script with `q`); use `sed -n 1p` or process substitution. No `mapfile`,
  no `declare -A`, no `${var,,}` in `ci/*/run.sh`. No new `<<<` here-string.
- Every captured `proto` or shim output needs `PROTO_REPORTER: text` in the job `env:`.
- `SELF_TEST_COUNT` in `ci/actionlint/run.sh` stays **16**. Add rows to the existing tables; add no
  new `*_self_test` table.
- Commit messages: conventional, with a **non-empty** scope (`ci`, `repo`, `rs`, `docs`), ending
  with `Co-Authored-By: <your model name> <noreply@anthropic.com>`. Commitlint rejects an empty
  scope: use `docs(docs): …` for a documentation-only commit. No `#NNN` line and no `token: value`
  line in a commit body.
- Prose in documents and comments: ASD-STE100 Simplified Technical English.

## Local environment and its limits

- PATH: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` and `export PROTO_REPORTER=text`
  before any proto, uv or moon call.
- **`release.yml` cannot run locally.** Its first real test is a live release. Every task states
  what CI or the live run must prove.
- `repo:actionlint`'s full gate exits rc 2 on this Mac (the 512-byte pipe preflight). Lint one file
  with `actionlint -shellcheck= <file>`. Run `ci/actionlint/run.sh --self-test` under `/bin/bash`
  (3.2); **two `cargo-lock-step` rows fail there and are known false failures**. Any other failing
  row is real.
- Docker Desktop here uses the **containerd** image store; the runners use the **classic** store.
  PR 1 hit three CI-only failures from that difference. A step that touches an image proves itself
  in CI, not locally.
- Lint Python with `uv run --locked --project py ruff check --config py/pyproject.toml ci/`.
- `ci/release-plan` has its own uv project: run its checker with
  `uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py …`.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `rs/Dockerfile` | Modify | Build with `cargo auditable`, so the binary carries its crate list. |
| `rs/crates/services/paigasus-iam/Cargo.toml` | Modify | `version = "0.1.0"`. |
| `rs/crates/services/paigasus-gateway/Cargo.toml` | Modify | `version = "0.1.0"`. |
| `rs/crates/services/paigasus-iam/CHANGELOG.md` | Create | The 0.1.0 section that `--assert` demands. |
| `rs/crates/services/paigasus-gateway/CHANGELOG.md` | Create | The same for the gateway. |
| `rs/Cargo.lock` | Modify | The two new versions. |
| `rs/crates/services/*/src/service_info.rs` | Modify | Correct the `0.0.0` doc comments and the test comment. |
| `rs/release-plz.toml` | Modify | Say why `release = false` stays (M7), and correct the stale "0.0.0 deliberately" section header (S8). |
| `ci/release-plan/release_plan.py` | Modify | Service versions, `skip_<svc>`, the changelog assertion, fixture rows. |
| `ci/release-plan/run.sh` | Modify | Write the six new outputs, in the real path and in the fail-safe path. |
| `ci/affected-graph/ci_targets.py` | Modify | Re-pin the `run.sh` lines that change. |
| `ci/actionlint/release_guard.py` | Modify | V8 per chain, V9b/V9c for the new outputs, V13, V14, the new markers. |
| `.github/workflows/release.yml` | Modify | The two service chains, six new jobs. |
| `docs/ops/RUNBOOK-containers.md` | Modify | How to cut a service release, and how to verify one. |
| `CLAUDE.md` | Modify | The chains, D10, the two approvals, `cargo auditable`. |
| `docs/superpowers/specs/2026-09-19-sma-658-container-release-design.md` | Modify | Record the decisions this plan makes. |

**Not changed, and why (obligations checked):**

- No new `repo:*` Moon task, so `ci.yml`'s `T=(…)` array, the `CLAUDE.md` marker command,
  `SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS` and `REQUIRED_REPO_TASKS` do not change
  (spec § 7.4).
- `ci/version-lockstep/run.sh` names neither service (checked: `grep` finds no `iam` or `gateway`
  line), so the kernel family's eighteen sites are unaffected by the bump.
- `ci/publish-metadata/run.sh` Check 3 filters `publish = false` crates out before it reads a
  version (M6), so `0.1.0` reds nothing there.
- `ci/images/run.sh` and `ci/images/release_decision.py` do not change. PR 2 consumes their CLI.
- `.github/workflows/images.yml` and `images-rehearsal.yml` do not change.

---

### Task 1: Build the service binaries with `cargo auditable`

**Files:**
- Modify: `rs/Dockerfile` (the `builder` stage, the `RUN --mount=…` block)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: an image whose binary carries its crate list, so `syft` reports `cargo>0`. Task 9's
  runbook text and AC 7 depend on it. No interface changes for `ci/images/run.sh`.

- [ ] **Step 1: Read the builder stage**

Run: `sed -n '13,29p' rs/Dockerfile`
Expected: the `FROM rust:1.95.0-bookworm@sha256:…  AS builder` line, `ENV RUSTUP_TOOLCHAIN=1.95.0`,
and one `RUN --mount=type=cache,…` block whose command is
`cargo build --release --locked -p "${BIN}" --bin "${BIN}"`.

- [ ] **Step 2: Install the pinned tool and change the build command**

Replace the `RUN --mount=…` block with this. The two `--mount` lines and the `&& mkdir -p /out …`
tail stay exactly as they are; only the `cargo build` line changes, and one `RUN` line is added
above the block.

```dockerfile
# `cargo auditable` embeds the crate list in the binary, which is what makes the image SBOM list
# the Rust dependencies. MEASURED on 2026-09-20 (spec § 4.5): a plain `cargo build` gives
# `cargo=0` in the SBOM, and `cargo auditable build` gives real purls. syft's
# cargo-auditable-binary-cataloger is in its default set, so no syft flag changes.
# Version-pinned: cargo-auditable has no proto plugin, so this is the only pin it can carry.
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    cargo install cargo-auditable --locked --version 0.7.6
# The binary is copied OUT of the cache mount inside this same RUN: a cache mount is not part of
# the resulting layer, so `COPY --from=builder /src/target/...` in a later stage finds nothing.
# Distinct cache ids per binary — the default sharing mode is `shared`, so two concurrent builds
# would otherwise contend on cargo's lock.
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=target-${BIN},target=/src/target \
    cargo auditable build --release --locked -p "${BIN}" --bin "${BIN}" \
 && mkdir -p /out && cp "/src/target/release/${BIN}" /out/service
```

- [ ] **Step 3: Confirm `assert_pins` still passes**

`assert_pins` asserts the rustc channel, the builder digest, the chisel release and the final
stage's `COPY` list. It asserts nothing about the cargo invocation, so it must still pass.

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/images/run.sh build-oci bogus-service out 2>&1 | sed -n 1,6p
```
Expected: a line starting `pins OK: rustc 1.95.0,` and then a failure about the unknown service.
The pins line is the assertion under test; the failure after it is expected.

- [ ] **Step 4: Confirm the pinned version exists**

Run: `curl -sS https://crates.io/api/v1/crates/cargo-auditable/0.7.6 -o /dev/null -w '%{http_code}\n'`
Expected: `200`.

- [ ] **Step 5: Commit**

```bash
git add rs/Dockerfile
git commit -m "$(cat <<'EOF'
build(rs): build the service binaries with cargo auditable

The image SBOM listed no Rust crates. cargo auditable embeds the crate
list in the binary, and syft reads it with a cataloger that is already
in its default set. The tool has no proto plugin, so the builder stage
installs a pinned version.

Co-Authored-By: <your model name> <noreply@anthropic.com>
EOF
)"
```

**What only CI can prove:** that the real service build still succeeds and that `M8` now reports
`cargo>0`. A cold release build takes longer than 15 minutes locally. `images.yml` runs it on this
PR and prints the `M8 …` line.

---

### Task 2: Set both services to 0.1.0

**Files:**
- Modify: `rs/crates/services/paigasus-iam/Cargo.toml` (line 3), `rs/crates/services/paigasus-gateway/Cargo.toml` (line 3)
- Create: `rs/crates/services/paigasus-iam/CHANGELOG.md`, `rs/crates/services/paigasus-gateway/CHANGELOG.md`
- Modify: `rs/Cargo.lock`, `rs/release-plz.toml`, `rs/crates/services/paigasus-iam/src/service_info.rs`, `rs/crates/services/paigasus-gateway/src/service_info.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `paigasus-iam` and `paigasus-gateway` at version `0.1.0`, each with a `CHANGELOG.md`
  whose first version heading is `## [0.1.0] - 2026-09-20`. Task 3's `--assert` reads that heading
  form, and Task 3's `skip_<svc>` rule reads the version.

- [ ] **Step 1: Bump both manifests**

In `rs/crates/services/paigasus-iam/Cargo.toml` and
`rs/crates/services/paigasus-gateway/Cargo.toml`, change line 3 from `version = "0.0.0"` to:

```toml
version = "0.1.0"
```

- [ ] **Step 2: Write both changelogs**

Create `rs/crates/services/paigasus-iam/CHANGELOG.md`:

```markdown
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A maintainer writes this file by hand. release-plz does not process this crate, because its Cargo
manifest sets `publish = false` (SMA-658, spec § 3.1).

## [Unreleased]

## [0.1.0] - 2026-09-20

### Added

- The first released container image: `ghcr.io/smk1085/paigasus-iam` and
  `docker.io/smaschek/paigasus-iam` (SMA-658).
```

Create `rs/crates/services/paigasus-gateway/CHANGELOG.md` with the same text, with `paigasus-iam`
replaced by `paigasus-gateway` in the two image names.

- [ ] **Step 3: Update the lockfile**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo update --manifest-path rs/Cargo.toml --workspace --offline 2>&1 | tail -3
git diff --stat rs/Cargo.lock
```
Expected: `rs/Cargo.lock` changes, and the diff holds only the two `version = "0.1.0"` lines for
`paigasus-iam` and `paigasus-gateway`. If the diff holds any other package, stop and report: the
lockfile must not move for this change.

- [ ] **Step 4: Correct the comments that document the `0.0.0` pin**

In `rs/crates/services/paigasus-gateway/src/service_info.rs`, replace line 15:

```rust
/// `Cargo.toml` version (AC 4). Reports `0.0.0` until release-plz is activated.
```

with:

```rust
/// `Cargo.toml` version (AC 4). A maintainer sets it by hand; release-plz does not process this
/// crate, because its Cargo manifest sets `publish = false` (SMA-658, spec § 3.1).
```

In `rs/crates/services/paigasus-iam/src/service_info.rs`, find the doc comment above
`pub const VERSION` that says every crate is `0.0.0` and release-plz is dormant, and replace that
sentence with the same two lines.

Then, in `rs/crates/services/paigasus-iam/src/service_info.rs`, find this comment inside
`the_descriptor_names_this_service_and_this_crates_build_version`:

```rust
        // Still NOT proven while every crate is "0.0.0": that this is the SERVICE's version
        // rather than the shared library's. Both strings are identical today (spec § 6.4).
```

and replace it with:

```rust
        // The service crate is 0.1.0 and the shared library crates it depends on (paigasus-iam-
        // core, paigasus-logging) stay at 0.0.0 (SMA-658, spec § 3.4). This test asserts no
        // specific literal here, because a hard-pinned "0.1.0" would break on every future
        // service bump; the check above already proves the value came from THIS crate's own
        // env!("CARGO_PKG_VERSION").
```

S7 (should-fix): the plan's original replacement added `assert_eq!(info.version, "0.1.0")` under a
claim that this "proves" the version came from the right crate. It does not — the line above it
already does that, by reading `env!("CARGO_PKG_VERSION")` directly — and the new literal only pins
a value that the next service bump breaks. This corrected comment states the true, weaker claim and
adds no fragile assertion.

- [ ] **Step 5: Update the release-plz comments**

In `rs/release-plz.toml`, find the `[[package]]` entries for `paigasus-iam` and `paigasus-gateway`
and add this comment line above each `release = false`:

```toml
# release = false stays. MEASURED (SMA-658 M7, release-plz 0.3.158): release-plz never processes a
# crate whose Cargo manifest sets `publish = false`, so `release = true` would change nothing. A
# maintainer bumps this service by hand, and release.yml's tag-<svc> job makes its tag.
```

S8 (should-fix): a second, STALE comment survives above these two `[[package]]` entries, in the
shared "Not releasable" section header. Replace it too. Find this text (line 107):

```toml
# paigasus-gateway / paigasus-iam stay at 0.0.0 deliberately: their
# `env!("CARGO_PKG_VERSION")` feeds the ServiceInfo descriptor, and ADR-0020 skew
# reporting is parked on that value (SMA-505 R7).
```

and replace it with:

```toml
# paigasus-gateway / paigasus-iam are versioned BY HAND (SMA-658): each moved to 0.1.0 at the
# first image release. `packages_to_process()` filters on Cargo's own `publish` field, and
# neither crate is in a `version_group`, so `release-plz update` never sees them (M7, measured on
# 0.3.158). That is the scoped claim, not a blanket one: a `publish = false` crate INSIDE a group
# whose head is publishable still gets its `[package] version` written — the comment on the
# kernel group above records that, also measured. `env!("CARGO_PKG_VERSION")` feeds the
# ServiceInfo descriptor, and ADR-0020 skew reporting is parked on that value (SMA-505 R7).
```

- [ ] **Step 6: Run the service tests**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --locked -p paigasus-iam -p paigasus-gateway --no-tests=pass -E 'test(service_info)' 2>&1 | tail -5
```
Expected: the `service_info` tests pass, including
`the_descriptor_names_this_service_and_this_crates_build_version` with its corrected comment
(Step 4, S7). If Docker is unreachable, the Docker-gated suites skip; that does not affect these
tests.

- [ ] **Step 7: Confirm no gate asserts the old version**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/opt/homebrew/bin/bash ci/publish-metadata/run.sh 2>&1 | tail -3
```
Expected: "all checks passed". Use the Homebrew bash: this gate needs bash 4+ (`declare -A`).

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam rs/crates/services/paigasus-gateway rs/Cargo.lock rs/release-plz.toml
git commit -m "$(cat <<'EOF'
chore(rs): release paigasus-iam and paigasus-gateway v0.1.0

The two services leave 0.0.0. A maintainer sets a service version by
hand, because release-plz does not process a crate whose Cargo manifest
sets publish = false. Each service gets a changelog with a 0.1.0
section, which ci/release-plan asserts.

Co-Authored-By: <your model name> <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Teach the release plan about the services

**Files:**
- Modify: `ci/release-plan/release_plan.py`
- Test: the same file (`FIXTURES`, `COLLECTION_ROWS`, `self_test`, `_assert_repo`)

**Interfaces:**
- Consumes: Task 2's `0.1.0` versions and the `## [0.1.0]` changelog heading.
- Produces, for `main()`'s runtime path, six printed lines in addition to `nothing_to_release=…`:
  `skip_iam=true|false`, `skip_gateway=true|false`, `version_iam=<version>`,
  `version_gateway=<version>`. Task 4 copies those lines into `$GITHUB_OUTPUT`; Task 7's jobs read
  them as `needs.plan.outputs.skip_iam` and `needs.plan.outputs.version_iam`.
- New public names: `SERVICES`, `service_state()`, `changelog_names_version()`.

**Decision this task makes (the spec does not fix it):** `plan` also outputs the VERSION for each
service. Spec § 6.1 lists only the `skip_*` outputs, but § 4.3 step 2 requires `publish-images` to
compare the image label "with `plan`'s version", and no other job knows it. Four outputs, not two.

- [ ] **Step 1: Write the failing fixtures**

Add to `ci/release-plan/release_plan.py`, directly above `def self_test() -> int:`:

```python
# SMA-658. The services are invisible to `releasable_packages` (they are Cargo `publish = false`),
# so they need their own reader. A service is SKIPPED when it has no real version yet, or when its
# tag already exists. Anything else RUNS, which is the fail-safe direction: spec § 4.3 step 3 makes
# a run for an already released version a no-op.
SERVICE_FIXTURES: list[tuple[str, str, set[str], bool]] = [
    ("a version with no tag -> run", "0.1.0", set(), False),
    ("the tag already exists -> skip", "0.1.0", {"paigasus-iam-v0.1.0"}, True),
    ("still 0.0.0 -> skip", "0.0.0", set(), True),
    ("a newer version than the tag -> run", "0.2.0", {"paigasus-iam-v0.1.0"}, False),
    ("a tag that only PREFIXES the wanted one -> run", "0.1.0", {"paigasus-iam-v0.1.0-rc1"}, False),
]


def _service_fixture_rows() -> str | None:
    for label, version, tags, want in SERVICE_FIXTURES:
        got = service_skips(version, "iam", tags)
        if got != want:
            return f"{label!r}: expected {want}, got {got}"
    return None


def _changelog_reader_rows() -> str | None:
    rows: list[tuple[str, str, str, bool]] = [
        ("a keep-a-changelog heading", "## [0.1.0] - 2026-09-20\n", "0.1.0", True),
        ("a heading with a compare link", "## [0.1.0](https://x/y) - 2026-09-20\n", "0.1.0", True),
        ("a bare heading", "## 0.1.0\n", "0.1.0", True),
        ("only the unreleased section", "## [Unreleased]\n", "0.1.0", False),
        ("another version only", "## [0.2.0] - 2026-09-20\n", "0.1.0", False),
        # A prefix must not read as a hit: 0.1.0 is not named by a 0.1.01 heading.
        ("a longer version that starts with it", "## [0.1.01] - 2026-09-20\n", "0.1.0", False),
        ("the version inside prose, not a heading", "see 0.1.0 below\n", "0.1.0", False),
    ]
    for label, text, version, want in rows:
        got = changelog_names_version(text, version)
        if got != want:
            return f"{label!r}: expected {want}, got {got}"
    return None


# S9 (SMA-658). Spec § 6.1's fail-safe direction as a fixture: a service that cannot be read is
# inconclusive for THAT SERVICE alone — never a silent skip, and never cross-talk into the other
# service or into run()'s own kernel verdict. This mirrors _broken_crate_manifest_tree's shape,
# but leaves `paigasus-iam` OUT of `[workspace] members` entirely (rather than malforming its
# manifest), so `crate_manifests(rs_root)["paigasus-iam"]` fails with a plain KeyError that never
# touches the scan for the healthy `paigasus-gateway` crate or for run()'s own kernel packages.
def _mixed_service_tree(tmp: str) -> Path:
    rs_root = Path(tmp) / "rs"
    kernel_dir = rs_root / "crates" / "libs" / "paigasus-kernel"
    gateway_dir = rs_root / "crates" / "services" / "paigasus-gateway"
    kernel_dir.mkdir(parents=True)
    gateway_dir.mkdir(parents=True)
    (rs_root / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["crates/libs/*", "crates/services/paigasus-gateway"]\n')
    (rs_root / "release-plz.toml").write_text("")
    (kernel_dir / "Cargo.toml").write_text(
        '[package]\nname = "paigasus-kernel"\nversion = "0.1.0"\n')
    (gateway_dir / "Cargo.toml").write_text(
        '[package]\nname = "paigasus-gateway"\nversion = "0.1.0"\npublish = false\n')
    return rs_root


def _service_state_is_a_separate_failure_domain() -> str | None:
    tmp = tempfile.mkdtemp()
    try:
        rs_root = _mixed_service_tree(tmp)
        state = service_state(rs_root, set())
        if state.get("iam") != (False, ""):
            return f"the missing iam crate did not read as (False, ''): {state.get('iam')!r}"
        if state.get("gateway") != (False, "0.1.0"):
            return (f"the healthy gateway crate was affected by iam's failure: "
                    f"{state.get('gateway')!r}")
        try:
            run(Path(tmp), "push")
        except Exception as exc:  # deliberately broad; run() must never raise (see its docstring)
            return f"run() raised {type(exc).__name__}: {exc} — a service failure must not reach it"
        return None
    finally:
        shutil.rmtree(tmp)
```

Then register the three rows. Find the `COLLECTION_ROWS` tuple and add these three entries at its end:

```python
    ("SMA-658 service skip rows", _service_fixture_rows),
    ("SMA-658 changelog reader rows", _changelog_reader_rows),
    ("SMA-658 service_state is a separate failure domain", _service_state_is_a_separate_failure_domain),
```

- [ ] **Step 2: Run the self-test to watch it fail**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --self-test
```
Expected: FAIL for all three new rows, with `raised NameError: name 'service_skips' is not
defined`, the same for `changelog_names_version`, and the same for `service_state` (S9). Exit
code 3.

- [ ] **Step 3: Write the readers**

First, add `import re` to the module's import block, in alphabetical order (between `import io`
and `import shutil`). The module does not import `re` today, and `_CHANGELOG_HEADING` below needs
it — without this line, the module fails to import with `NameError: name 're' is not defined`.

Add to `ci/release-plan/release_plan.py`, directly below `def repo_tags(...)`:

```python
# SMA-658. Both services are Cargo `publish = false`, so `releasable_packages` filters them out by
# design and release-plz never processes them (M7). They need their own reader, and a STRICT pin:
# a service crate that is missing from the tree is inconclusive for that service, never a silent
# skip.
SERVICES: dict[str, str] = {"iam": "paigasus-iam", "gateway": "paigasus-gateway"}

_CHANGELOG_HEADING = re.compile(r"^##\s+\[?(?P<version>[0-9][^\]\s]*)\]?", re.M)


def changelog_names_version(text: str, version: str) -> bool:
    """True when a `## [<version>]` heading names exactly this version.

    The comparison is on the captured version STRING, not a prefix test: `## [0.1.01]` must not
    satisfy a 0.1.0 release, and prose that merely contains the number must not either.
    """
    return any(m.group("version") == version for m in _CHANGELOG_HEADING.finditer(text))


def service_skips(version: str, service: str, tags: set[str]) -> bool:
    """Spec § 6.1. Skip when there is no real version yet, or when the tag already exists."""
    if version == "0.0.0":
        return True
    return tag_for(f"paigasus-{service}", version) in tags


def service_state(rs_root: Path, tags: set[str]) -> dict[str, tuple[bool, str]]:
    """service -> (skip, version). A SEPARATE FAILURE DOMAIN from the kernel verdict.

    Each service is read in its own try. A failure for one service writes skip=False for that
    service — the fail-safe direction, because spec § 4.3 step 3 makes a run for an already
    released version a no-op — and leaves the other service and the kernel verdict untouched. The
    negative control's synthetic trees hold no service crate at all, and their kernel verdict must
    not change because of that.
    """
    out: dict[str, tuple[bool, str]] = {}
    for service, crate in SERVICES.items():
        try:
            manifest = crate_manifests(rs_root)[crate]
            pkg = load_toml(manifest).get("package") or {}
            version = pkg.get("version")
            if not isinstance(version, str):
                raise InconclusiveError(f"{crate} has no literal [package] version in {manifest}")
            out[service] = (service_skips(version, service, tags), version)
        except Exception as exc:  # deliberately broad; see the docstring above.
            print(f"release-plan: {service} is inconclusive ({type(exc).__name__}: {exc}) — run",
                  file=sys.stderr)
            out[service] = (False, "")
    return out
```

Confirm `sys` is already imported at the top of the file — it is; this step is what adds `re`.

- [ ] **Step 4: Run the self-test to watch it pass**

Run: the Step 2 command.
Expected: no FAIL lines, exit code 0.

- [ ] **Step 5: Print the new lines in the runtime path**

In `main()`, replace the last three lines before `return 0`:

```python
    nothing, reason = run(root, args.event_name)
    print(f"release-plan: {reason}")
    print(f"nothing_to_release={'true' if nothing else 'false'}")
    return 0
```

with:

```python
    nothing, reason = run(root, args.event_name)
    print(f"release-plan: {reason}")
    # SMA-658. The service lines are computed in their own failure domain, so a broken service
    # read cannot change the kernel verdict above. `repo_tags` is read once more here on purpose:
    # `run()` swallows its own failure, and a second failure here must land on the per-service
    # fail-safe rather than on the kernel one.
    try:
        tags = repo_tags(root)
    except Exception as exc:  # deliberately broad; the fail-safe direction is "run".
        print(f"release-plan: tags are inconclusive ({type(exc).__name__}: {exc}) — run",
              file=sys.stderr)
        tags = set()
    for service, (skip, version) in service_state(root / "rs", tags).items():
        print(f"skip_{service}={'true' if skip else 'false'}")
        print(f"version_{service}={version}")
    print(f"nothing_to_release={'true' if nothing else 'false'}")
    return 0
```

The `nothing_to_release=` line stays LAST, because `run.sh`'s `tail -n 1` takes the final match.

- [ ] **Step 6: Add the changelog assertion to `--assert`**

In `_assert_repo`, directly above `for p in problems:`, add:

```python
    # SMA-658 spec § 3.1: V-a is a bump by hand, so nothing else can catch a forgotten changelog
    # entry. `repo:actionlint` check 11 runs --assert on every pull request, which is what makes
    # this a gate rather than a convention.
    for service, (_skip, version) in service_state(repo_root / "rs", tags).items():
        if not version or version == "0.0.0":
            continue
        changelog = repo_root / "rs" / "crates" / "services" / SERVICES[service] / "CHANGELOG.md"
        try:
            text = changelog.read_text(encoding="utf-8")
        except OSError as exc:
            problems.append(f"{SERVICES[service]} is at {version} but {changelog} cannot be read "
                            f"({exc}). A hand-bumped service needs a changelog section.")
            continue
        if not changelog_names_version(text, version):
            problems.append(f"{SERVICES[service]} is at {version} but {changelog} has no "
                            f"`## [{version}]` heading. Add the section in the same PR as the "
                            f"bump: nothing else records what that release contains.")
```

- [ ] **Step 7: Prove the assertion bites**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cp rs/crates/services/paigasus-iam/CHANGELOG.md /tmp/iam-changelog.bak
python3 - <<'PY'
import pathlib
p = pathlib.Path("rs/crates/services/paigasus-iam/CHANGELOG.md")
p.write_text(p.read_text().replace("## [0.1.0] - 2026-09-20", "## [0.0.9] - 2026-09-20"))
PY
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --assert .; echo "rc=$?"
cp /tmp/iam-changelog.bak rs/crates/services/paigasus-iam/CHANGELOG.md
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --assert .; echo "rc=$?"
```
Expected: the first run prints `paigasus-iam is at 0.1.0 but … has no `## [0.1.0]` heading` and
`rc=3`. The second run prints nothing about the changelog and `rc=0`. Restore the file with the
`cp` above; do not use `git checkout --`, which would also revert this task's other edits.

- [ ] **Step 8: Lint and commit**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py ruff check --config py/pyproject.toml ci/release-plan/
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --self-test
```
Expected: `All checks passed!` and exit 0 for both.

```bash
git add ci/release-plan/release_plan.py
git commit -m "$(cat <<'EOF'
feat(ci): decide a per-service image release in ci/release-plan

The plan job now reports, for each service, whether its image release
is pending and which version it carries. A service is skipped when it
is still 0.0.0 or when its tag exists. Every other state runs, which is
the fail-safe direction. --assert also fails when a bumped service has
no changelog section.

Co-Authored-By: <your model name> <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Carry the six lines into `$GITHUB_OUTPUT`

**Files:**
- Modify: `ci/release-plan/run.sh`
- Modify: `ci/affected-graph/ci_targets.py` (`RELEASE_PLAN_SH_CALL_SITES`)

**Interfaces:**
- Consumes: Task 3's printed lines.
- Produces: `skip_iam`, `skip_gateway`, `version_iam`, `version_gateway` and `nothing_to_release`
  as step outputs. Task 7's `if:` expressions read them through `needs.plan.outputs.…`.

- [ ] **Step 1: Read the two paths that write output**

Run: `sed -n '66,100p' ci/release-plan/run.sh`
Expected: the `github_output()` function, its fail-safe branch (a `::warning::` and
`printf 'nothing_to_release=false\n' >> "${GITHUB_OUTPUT:-/dev/stdout}"`), and the real branch
(`grep -E … | tail -n 1 >> "${GITHUB_OUTPUT:-/dev/stdout}"`).

- [ ] **Step 2: Write both paths**

In `github_output()`, replace the fail-safe `printf` line with these three lines:

```bash
    printf 'nothing_to_release=false\n' >> "${GITHUB_OUTPUT:-/dev/stdout}"
    # SMA-658. The image chains read their own outputs, and an unset output makes the chain RUN
    # (spec § 4.1). Writing them here as well keeps the fail-safe explicit rather than implied.
    printf 'skip_iam=false\nskip_gateway=false\n' >> "${GITHUB_OUTPUT:-/dev/stdout}"
```

Then replace the real branch's single `grep … | tail -n 1 >> …` line with:

```bash
  printf '%s\n' "$out" | grep -E '^nothing_to_release=(true|false)$' | tail -n 1 \
    >> "${GITHUB_OUTPUT:-/dev/stdout}"
  # SMA-658. Four separate greps, one per key, each taken LAST for the same forged-line reason as
  # the verdict above: a service line inside a heredoc or an echoed error message must not be
  # read as the real decision. S6: a single combined grep over two keys cannot use `tail -n 1`
  # without dropping one of the two services, so each key gets its own grep and its own tail.
  printf '%s\n' "$out" | grep -E '^skip_iam=(true|false)$' | tail -n 1 \
    >> "${GITHUB_OUTPUT:-/dev/stdout}"
  printf '%s\n' "$out" | grep -E '^skip_gateway=(true|false)$' | tail -n 1 \
    >> "${GITHUB_OUTPUT:-/dev/stdout}"
  printf '%s\n' "$out" | grep -E '^version_iam=' | tail -n 1 \
    >> "${GITHUB_OUTPUT:-/dev/stdout}"
  printf '%s\n' "$out" | grep -E '^version_gateway=' | tail -n 1 \
    >> "${GITHUB_OUTPUT:-/dev/stdout}"
```

Every `grep` here is followed by a fixed-count `tail -n 1`, which reads its whole input before
writing — this is not a pipe into an early-exit reader (`grep -q`/`-m`, `head`, an `awk … exit`);
`tail -n 1` only trims what it already read in full.

- [ ] **Step 3: Run the negative control and the self-test**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/release-plan/run.sh --self-test; echo "rc=$?"
/bin/bash ci/release-plan/run.sh --negative-control; echo "rc=$?"
```
Expected: rc 0 for both.

- [ ] **Step 4: Prove the wiring writes the six lines**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
out="$(mktemp)"; GITHUB_OUTPUT="$out" GITHUB_EVENT_NAME=push /bin/bash ci/release-plan/run.sh --github-output > /dev/null; sort "$out"
```
Expected exactly these five lines (the order after `sort` is alphabetical):
```
nothing_to_release=false
skip_gateway=false
skip_iam=false
version_gateway=0.1.0
version_iam=0.1.0
```
`skip_*` is `false` because the tags `paigasus-iam-v0.1.0` and `paigasus-gateway-v0.1.0` do not
exist yet. If a tag already exists when you run this, the matching line reads `true`; that is
correct, and it is the state after the first release.

- [ ] **Step 5: Re-pin the changed lines**

One pinned entry in `ci/affected-graph/ci_targets.py`'s `RELEASE_PLAN_SH_CALL_SITES` names a line
this task kept unchanged (S16: it is kept verbatim, not rewritten), and two new entries are added
for the lines this task adds. Keep this entry as it is:

```python
    "printf 'nothing_to_release=false\\n' >> \"${GITHUB_OUTPUT:-/dev/stdout}\"",
```

and add these two new entries directly after it, keeping the tuple's other entries unchanged:

```python
    "printf 'skip_iam=false\\nskip_gateway=false\\n' >> \"${GITHUB_OUTPUT:-/dev/stdout}\"",
    "printf '%s\\n' \"$out\" | grep -E '^skip_iam=(true|false)$' | tail -n 1 \\",
```

One representative real-branch grep line is enough for the pin: the same reachability guard
(`T_AFFECTED_SMOKE_REQUIRED_INPUTS`, see below) covers all four new grep lines, and the other
three follow the same one-line shape.

- [ ] **Step 6: Run the affected-graph gate**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/affected-graph/run.sh --self-test 2>&1 | tail -5
```
Expected: no failing row. Use the system `/bin/bash` 3.2: this gate deadlocks under Homebrew bash
on this host.

- [ ] **Step 7: Commit**

```bash
git add ci/release-plan/run.sh ci/affected-graph/ci_targets.py
git commit -m "$(cat <<'EOF'
feat(ci): publish the per-service release decision as job outputs

The plan job now carries a skip flag and a version for each service. The
fail-safe branch writes the skip flags too, so an unreadable decision
runs the chains rather than dropping them.

Co-Authored-By: <your model name> <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Generalize the approval rule (V8) and the plan contract (V9)

**Files:**
- Modify: `ci/actionlint/release_guard.py`
- Test: the same file (`FIXTURES`, `_OK_IMAGES_MAIN`)

**Interfaces:**
- Consumes: Task 4's output names.
- Produces: `CHAIN_APPROVALS`, `approval_for_job()`, an extended `ACCEPTED_PLAN_FORMS`, and
  `_OK_IMAGES_MAIN` (a control fixture that holds both service chains). Task 6 adds V13 and V14
  rows against the same fixture.

**Decision this task makes (the spec does not fix it):** `publish-images-<svc>` names `plan` in its
`needs:`, because it reads `needs.plan.outputs.version_<svc>`. V9b's subject is every direct
consumer of `plan`, so that job must carry a gate literal too. It carries the SAME literal as its
chain's first job. The alternative — routing the version through the build job's outputs — would
hide the version's origin one hop further from the gate.

- [ ] **Step 1: Write the failing control fixture**

Add to `ci/actionlint/release_guard.py`, directly below the `_OK_MAIN` string:

```python
# SMA-658. A second control: the release file with BOTH service chains present (S11 — the row
# below is named "both chains present", so the fixture must actually hold both, not the iam chain
# alone). _OK_MAIN stays as it is, so every existing row keeps its meaning, and the per-chain
# rules must tolerate a file with no image chain at all — which is exactly what _OK_MAIN asserts
# for them.
#
# B3: `_OK_MAIN`'s `plan` job outputs only `nothing_to_release`. The new V9c loop (Step 6) checks
# every service's `skip_*`/`version_*` output, so a fixture built on `_OK_MAIN` without adding
# those four output lines would collect four V9c violations on every row below — including every
# row that expects a clean verdict or an unrelated violation. The control fixture must model the
# file the rule requires.
_OK_IMAGES_MAIN = _OK_MAIN.replace(
    "      nothing_to_release: ${{ steps.decide.outputs.nothing_to_release }}\n",
    "      nothing_to_release: ${{ steps.decide.outputs.nothing_to_release }}\n"
    "      skip_iam: ${{ steps.decide.outputs.skip_iam }}\n"
    "      version_iam: ${{ steps.decide.outputs.version_iam }}\n"
    "      skip_gateway: ${{ steps.decide.outputs.skip_gateway }}\n"
    "      version_gateway: ${{ steps.decide.outputs.version_gateway }}\n"
) + """
  images-build-iam:
    needs: [plan]
    if: needs.plan.outputs.skip_iam != 'true'
    runs-on: ubuntu-latest
    steps: [{run: ci/images/run.sh build-oci paigasus-iam out}]
  approve-images-iam:
    needs: [images-build-iam]
    environment: release-approval
    runs-on: ubuntu-latest
    steps: [{run: echo approved}]
  publish-images-iam:
    needs: [plan, images-build-iam, approve-images-iam]
    if: needs.plan.outputs.skip_iam != 'true'
    environment: release-images
    runs-on: ubuntu-latest
    permissions:
      packages: write
      id-token: write
      attestations: write
    steps: [{run: crane push layout ghcr.io/smk1085/paigasus-iam:x}]
  tag-iam:
    needs: [publish-images-iam]
    environment: release-publish
    runs-on: ubuntu-latest
    steps: [{run: gh api repos/o/r/git/refs}]
  images-build-gateway:
    needs: [plan]
    if: needs.plan.outputs.skip_gateway != 'true'
    runs-on: ubuntu-latest
    steps: [{run: ci/images/run.sh build-oci paigasus-gateway out}]
  approve-images-gateway:
    needs: [images-build-gateway]
    environment: release-approval
    runs-on: ubuntu-latest
    steps: [{run: echo approved}]
  publish-images-gateway:
    needs: [plan, images-build-gateway, approve-images-gateway]
    if: needs.plan.outputs.skip_gateway != 'true'
    environment: release-images
    runs-on: ubuntu-latest
    permissions:
      packages: write
      id-token: write
      attestations: write
    steps: [{run: crane push layout ghcr.io/smk1085/paigasus-gateway:x}]
  tag-gateway:
    needs: [publish-images-gateway]
    environment: release-publish
    runs-on: ubuntu-latest
    steps: [{run: gh api repos/o/r/git/refs}]
"""
```

Add these rows to `FIXTURES`, at the end of the list:

```python
    ("SMA-658 both chains present is clean", "main", _OK_IMAGES_MAIN, None),
    # S11: a row for each of the three graph shapes spec AC 9 asks for — kernel only (_OK_MAIN
    # itself, already covered by every pre-existing row), image only, and combined (the row
    # above). `build`, `wheels` and `release` are the only _OK_MAIN jobs an image-only file would
    # not have; `approve-release` stays, orphaned but harmless — V8a requires only that an
    # approval a chain actually uses exists and names an environment.
    ("SMA-658 only the image chains are present (no kernel chain)", "main",
     _OK_IMAGES_MAIN
     .replace("  build:\n    needs: [plan]\n    if: needs.plan.outputs.nothing_to_release != 'true'"
              "\n    runs-on: ubuntu-latest\n    steps: [{run: echo build}]\n", "")
     .replace("  wheels:\n    needs: [plan]\n    if: ${{ needs.plan.outputs.nothing_to_release != "
              "'true' }}\n    runs-on: ubuntu-latest\n    steps: [{run: echo wheels}]\n", "")
     .replace("  release:\n    needs: [build, approve-release]\n    runs-on: ubuntu-latest\n"
              "    steps: [{run: release-plz release}]\n", ""),
     None),
    ("SMA-658 a publisher without its own approval", "main",
     _OK_IMAGES_MAIN.replace("    needs: [plan, images-build-iam, approve-images-iam]",
                             "    needs: [plan, images-build-iam]"), "V8c"),
    ("SMA-658 a publisher behind the KERNEL approval only", "main",
     _OK_IMAGES_MAIN.replace("    needs: [plan, images-build-iam, approve-images-iam]",
                             "    needs: [plan, images-build-iam, approve-release]"), "V8c"),
    ("SMA-658 the service approval loses its environment", "main",
     _OK_IMAGES_MAIN.replace("    needs: [images-build-iam]\n    environment: release-approval",
                             "    needs: [images-build-iam]"), "V8a"),
    ("SMA-658 a chain job with the wrong gate literal", "main",
     _OK_IMAGES_MAIN.replace("    if: needs.plan.outputs.skip_iam != 'true'\n    runs-on: ubuntu-latest\n    steps: [{run: ci/images/run.sh build-oci",
                             "    if: needs.plan.outputs.skip_iam == 'true'\n    runs-on: ubuntu-latest\n    steps: [{run: ci/images/run.sh build-oci"),
     "V9b"),
```

- [ ] **Step 2: Run the self-test to watch it fail**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py --python '>=3.12' python3 ci/actionlint/release_guard.py --self-test
```
Expected: FAIL for every new row. `SMA-658 both chains present is clean` and `SMA-658 only the
image chains are present` both expect a clean verdict but get violations — V9b already fires,
because today's `ACCEPTED_PLAN_FORMS` does not know the per-service gate literal. The four
remaining rows each expect a specific "V8a"/"V8c"/"V9b" substring that the unmodified V8/V9 code
does not yet produce. Exit code 1.

- [ ] **Step 3: Add the per-chain approval map**

In `ci/actionlint/release_guard.py`, directly below the `APPROVAL_JOB = "approve-release"` line
and its comment, add:

```python
# SMA-658. Each service image chain carries its OWN approval job, separate from the kernel's. A
# kernel approval must not authorise an image push, and an image approval must not authorise a
# crates.io publish, so the rule is per chain rather than per file. A file with no image chain —
# every fixture built on _OK_MAIN — keeps exactly the old behaviour.
CHAIN_APPROVALS: dict[str, str] = {
    "iam": "approve-images-iam",
    "gateway": "approve-images-gateway",
}
# The jobs each service chain owns, by suffix. `publish-images-iam` and `tag-iam` must both sit
# behind `approve-images-iam`, never behind the kernel gate.
_CHAIN_JOB_PREFIXES = ("images-build-", "publish-images-", "tag-")


def approval_for_job(job_id: str) -> str:
    """The approval job that must gate this job. The kernel gate is the default."""
    for service, approval in CHAIN_APPROVALS.items():
        for prefix in _CHAIN_JOB_PREFIXES:
            if job_id == f"{prefix}{service}":
                return approval
    return APPROVAL_JOB
```

- [ ] **Step 4: Make V8 use the map**

In `approval_boundary_violations`, replace the V8a block and both loops with this. The function's
docstring stays; add one paragraph to it saying the rule is per chain.

```python
    out: list[str] = []
    # V8a. Every approval job that the file actually uses must exist and must name an environment.
    # A chain's approval job is only required when that chain has a job in this file.
    required = {APPROVAL_JOB}
    for jid in jobs:
        required.add(approval_for_job(jid))
    for approval in sorted(required):
        gate = jobs.get(approval)
        if not isinstance(gate, dict):
            if approval == APPROVAL_JOB:
                return [f"{name}: V8a: no job named '{APPROVAL_JOB}' exists. Every other clause "
                        f"of V8 is defined relative to it, so without it this verdict would pass "
                        f"vacuously."]
            out.append(f"{name}: V8a: no job named '{approval}' exists, but a job of its chain "
                       f"does. Each service chain carries its own approval.")
            continue
        if not _environment_name(gate):
            out.append(f"{name}: V8a: job '{approval}' declares no NAMED environment:. The pause "
                       f"that makes it a gate comes from that named environment's required "
                       f"reviewers; a missing environment:, or one with no name:, is an ordinary "
                       f"job that always succeeds.")

    # V8b. Nothing upstream of an approval job may publish.
    for approval in sorted(required):
        if not isinstance(jobs.get(approval), dict):
            continue
        for jid in sorted(gated_path_jobs(approval, jobs)):
            job = jobs.get(jid)
            if not isinstance(job, dict) or not job_publishes(job, f"{name}: job '{jid}'"):
                continue
            if jid == approval:
                out.append(f"{name}: V8b: job '{approval}' IS an approval gate and contains a "
                           f"step that can reach a registry. The gate itself must never publish "
                           f"— move the step to a job downstream of it.")
            else:
                out.append(f"{name}: V8b: job '{jid}' runs upstream of '{approval}' and contains "
                           f"a step that can reach a registry. That publishes before any human "
                           f"approves. Add --dry-run, or move the step downstream of the gate.")

    # V8c. Every publisher must sit downstream of ITS OWN chain's approval job.
    for jid, job in jobs.items():
        if not isinstance(job, dict) or not job_publishes(job, f"{name}: job '{jid}'"):
            continue
        approval = approval_for_job(jid)
        if approval not in gated_path_jobs(jid, jobs):
            out.append(f"{name}: V8c: job '{jid}' can reach a registry, but '{approval}' is "
                       f"not on its needs: path. It would publish without passing the gate that "
                       f"owns its chain.")
    return out
```

De-duplicate the V8b output: a job upstream of two approval jobs would otherwise be reported twice.
Replace the function's final `return out` with this three-line block:

```python
    seen: set[str] = set()
    deduped = [v for v in out if not (v in seen or seen.add(v))]
    return deduped
```

- [ ] **Step 5: Accept the new gate literals in V9b**

Replace the `ACCEPTED_PLAN_FORMS` definition with:

```python
ACCEPTED_PLAN_FORMS = frozenset({PLAN_GATE_EXPR, "${{ " + PLAN_GATE_EXPR + " }}"})
# SMA-658. Each service chain gates on its own skip output, with the same `!=` polarity and the
# same literal pinning: `== 'true'` inverts the decision, and `== 'false'` drops the chain on an
# unset output. A chain job may carry EITHER its own service's literal or the kernel one — never a
# different service's, which would tie two chains together.
SERVICE_PLAN_GATE_EXPRS: dict[str, frozenset[str]] = {
    service: frozenset({
        f"needs.{PLAN_JOB}.outputs.skip_{service} != 'true'",
        "${{ " + f"needs.{PLAN_JOB}.outputs.skip_{service} != 'true'" + " }}",
    })
    for service in ("iam", "gateway")
}


def accepted_plan_forms(job_id: str) -> frozenset[str]:
    """The `if:` literals this consumer of `plan` may carry."""
    for service, forms in SERVICE_PLAN_GATE_EXPRS.items():
        for prefix in _CHAIN_JOB_PREFIXES:
            if job_id == f"{prefix}{service}":
                return forms
    return ACCEPTED_PLAN_FORMS
```

In `plan_contract_violations`, replace the V9b loop body:

```python
    for jid in sorted(consumers):
        accepted = accepted_plan_forms(jid)
        if if_text(jobs[jid]) not in accepted:
            out.append(f"{name}: V9b: job '{jid}' needs '{PLAN_JOB}' but its if: is "
                       f"{if_text(jobs[jid])!r}, not one of {sorted(accepted)!r}. Only `!=` "
                       f"fails safe: `== 'true'` inverts the decision and `== 'false'` skips on "
                       f"an unset output. A whitespace variant of an accepted form (an extra "
                       f"space, a different quote style) also reds here — literal pinning, "
                       f"exactly as V2 pins GATE_EXPR.")
```

- [ ] **Step 6: Extend V9c to the new outputs**

In `plan_contract_violations`, directly above `return out`, add:

```python
    # V9c, SMA-658. The service outputs need the same full-match treatment as the kernel verdict:
    # `${{ steps.decide.outputs.skip_iam || 'true' }}` resolves to 'true' on an unset output and
    # silently drops that chain. The step id must be the one V9c already resolved.
    #
    # B4: the loop is guarded — it runs only when this file actually declares a chain job (one
    # whose approval differs from the kernel's). Unguarded, it would also red `_OK_MAIN` and every
    # fixture built on it, and the real release.yml before Task 7 lands: none of those declare the
    # four service outputs, and none of them needs to, because none of them has a chain job to run.
    if any(approval_for_job(jid) != APPROVAL_JOB for jid in jobs):
        for service in SERVICE_PLAN_GATE_EXPRS:
            for key in (f"skip_{service}", f"version_{service}"):
                expr = outs.get(key) if isinstance(outs, dict) else None
                if not isinstance(expr, str):
                    out.append(f"{name}: V9c: job '{PLAN_JOB}' declares no outputs.{key}. Its "
                               f"chain would read the empty string, which runs the chain but "
                               f"carries no version.")
                    continue
                want = "${{ steps." + str(decision.get("id") if decision else "") + f".outputs.{key} }}}}"
                if expr.strip() != want:
                    out.append(f"{name}: V9c: outputs.{key} is {expr!r}, not {want!r}. Anything "
                               f"else can resolve to a constant, and a `|| 'true'` tail drops the "
                               f"chain on an unset output.")
```

- [ ] **Step 7: Run the self-test to watch it pass**

Run: the Step 2 command.
Expected: no FAIL lines, exit code 0. Every pre-existing row still passes: `_OK_MAIN` has no image
chain, so `required` holds only `approve-release`, the guard's `any(...)` is `False`, and the V9c
loop above does not run — the behaviour on every `_OK_MAIN`-based row is unchanged.

- [ ] **Step 8: Run the guard against the real file**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py --python '>=3.12' python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "rc=$?"
```
Expected: rc 0. `release.yml` has no image chain yet, so the same guard keeps the new V9c loop from
running against it, exactly as it does for `_OK_MAIN`.

- [ ] **Step 9: Lint and commit**

Run: `uv run --locked --project py ruff check --config py/pyproject.toml ci/actionlint/`
Expected: `All checks passed!`

```bash
git add ci/actionlint/release_guard.py
git commit -m "$(cat <<'EOF'
feat(ci): make the approval rule per release chain

V8 knew one approval job, so a kernel approval would have authorised an
image push. Each chain now carries its own approval job, and a publisher
must sit behind the approval of its own chain. V9 accepts the per-service
gate literals and pins the new plan outputs.

Co-Authored-By: <your model name> <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Add the credential-scope rule (V13), the capability rule (V14) and the new markers

**Files:**
- Modify: `ci/actionlint/release_guard.py`
- Test: the same file (`FIXTURES`, one new `COLLECTION`-style helper for the other-workflow case,
  and one new self-test helper for the widened `PUBLISH_MARKERS` table, B7)

**Interfaces:**
- Consumes: Task 5's `_OK_IMAGES_MAIN`, `CHAIN_APPROVALS`, `approval_for_job`.
- Produces: `credential_scope_violations()`, `capability_violations()`, an extended
  `PUBLISH_MARKERS`, and an extended `OIDC_PUBLISH_JOBS` (S1). Task 7's `release.yml` must satisfy
  all of these; Task 7 ALSO adds `DOCKERHUB_TOKEN` to `EXPECTED_RELEASE_SECRETS`, in the same
  commit as the reference it pins, so that check never sees a pin with no matching reference (B5).

- [ ] **Step 1: Write the failing rows**

Add to `FIXTURES`, at the end:

```python
    ("SMA-658 the Docker Hub token outside its environment", "main",
     _OK_IMAGES_MAIN.replace("    steps: [{run: gh api repos/o/r/git/refs}]",
                             "    steps: [{run: echo x, env: {T: '${{ secrets.DOCKERHUB_TOKEN }}'}}]"),
     "V13"),
    ("SMA-658 the release-images environment on another job", "main",
     _OK_IMAGES_MAIN.replace("  tag-iam:\n    needs: [publish-images-iam]\n    environment: release-publish",
                             "  tag-iam:\n    needs: [publish-images-iam]\n    environment: release-images"),
     "V13"),
    ("SMA-658 an environment name that differs only by case", "main",
     _OK_IMAGES_MAIN.replace("    environment: release-images", "    environment: Release-Images"),
     None),
    ("SMA-658 an environment name built from an expression", "main",
     _OK_IMAGES_MAIN.replace("    environment: release-images",
                             "    environment: ${{ github.event.inputs.env }}"),
     "V13"),
    ("SMA-658 a write capability upstream of the approval", "main",
     _OK_IMAGES_MAIN.replace("  images-build-iam:\n    needs: [plan]",
                             "  images-build-iam:\n    permissions: {packages: write}\n    needs: [plan]"),
     "V14"),
    ("SMA-658 a publish hidden inside a script", "main",
     _OK_IMAGES_MAIN.replace("    steps: [{run: crane push layout ghcr.io/smk1085/paigasus-iam:x}]",
                             "    steps: [{run: ci/images/run.sh publish}]"),
     None),
    # B7: this row's name used to claim it exercises the `imagetools create` marker; it does not
    # — it reroutes tag-iam past its own chain's publish and approval jobs, and the violation it
    # gets is V8c on the `git/refs` marker. Renamed to say what it actually tests. The real
    # `imagetools\s+create` marker, and the other ten new markers, get their own fixtures in
    # Step 6 below.
    ("SMA-658 tag-iam skips its chain's publish and approval jobs", "main",
     _OK_IMAGES_MAIN.replace("    steps: [{run: crane push layout ghcr.io/smk1085/paigasus-iam:x}]\n  tag-iam:\n    needs: [publish-images-iam]",
                             "    steps: [{run: echo x}]\n  tag-iam:\n    needs: [images-build-iam]"),
     "V8c"),
```

The `Release-Images` row expects `None`: a case variant is the SAME environment to GitHub, so V13
must accept it rather than red. The "hidden inside a script" row expects `None` from the marker
list on purpose — that is what V14 exists to catch, and the row above it proves V14 fires on the
capability instead.

- [ ] **Step 2: Run the self-test to watch it fail**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py --python '>=3.12' python3 ci/actionlint/release_guard.py --self-test
```
Expected: FAIL for every row that expects `V13` or `V14`, because neither rule exists yet.

B5: `EXPECTED_RELEASE_SECRETS` does NOT get `DOCKERHUB_TOKEN` added here. `secret_reference_violations`
reds `release.yml` for any pinned name the file does not yet reference, and `release.yml` gets its
`DOCKERHUB_TOKEN` reference only in Task 7. Adding the pin here would make this task's own later
step that runs the guard against the real file fail with a real, unexpected rc 1 — Task 7 makes
this edit instead, in the same commit as the reference, so the pin and its reference always land
together.

- [ ] **Step 3: Write V13**

Add directly above `def approval_boundary_violations(`:

```python
# V13 (SMA-658). The Docker Hub token is the one long-lived publish credential in this repository.
# V10 pins WHICH secret names may appear; V13 pins WHERE one may appear. Without it, V10 accepts
# the token in any job of the release file.
SCOPED_SECRET = "DOCKERHUB_TOKEN"
SCOPED_SECRET_ENVIRONMENT = "release-images"
SCOPED_SECRET_JOBS = frozenset(f"publish-images-{service}" for service in CHAIN_APPROVALS)


def credential_scope_violations(doc: dict, name: str) -> list[str]:
    """V13. DOCKERHUB_TOKEN only in a `release-images` job, and that environment only on a
    publish job.

    Environment names are compared CASE-FOLDED, because GitHub treats them case-insensitively: a
    `Release-Images` job reaches the same secrets as `release-images`. An `environment:` built from
    an expression fails closed — this file cannot resolve it, and an unresolvable environment must
    never satisfy a scoping rule.

    This runs over EVERY workflow file, not only release.yml: any workflow with a `main` trigger
    could name the same environment and read the same secret.
    """
    out: list[str] = []
    jobs = doc.get("jobs")
    if not isinstance(jobs, dict):
        return out
    for jid, job in jobs.items():
        if not isinstance(job, dict):
            continue
        raw_env = job.get("environment")
        raw_text = raw_env if isinstance(raw_env, str) else str(
            raw_env.get("name") if isinstance(raw_env, dict) else "")
        if "${{" in raw_text:
            out.append(f"{name}: V13: job '{jid}' builds its environment: from an expression "
                       f"({raw_text!r}). This guard cannot resolve it, so the scoping rule for "
                       f"{SCOPED_SECRET} cannot be checked. Name the environment literally.")
            continue
        env_name = (_environment_name(job) or "").casefold()
        # S14 (SMA-658): the default 80-column fold can insert a newline inside a `${{ ... }}`
        # span for a long secret name, which `_EXPR_SPAN`'s `re.S` would then misparse. PyYAML
        # only folds at a space, so today's names survive — but the next one might not.
        names, _ = secret_refs(yaml.safe_dump(job, width=10**9, default_flow_style=False))
        if SCOPED_SECRET in names and env_name != SCOPED_SECRET_ENVIRONMENT:
            out.append(f"{name}: V13: job '{jid}' reads {SCOPED_SECRET} but its environment is "
                       f"{env_name or '(none)'!r}, not {SCOPED_SECRET_ENVIRONMENT!r}. That "
                       f"environment is the only thing that scopes the token to one job.")
        if env_name == SCOPED_SECRET_ENVIRONMENT and jid not in SCOPED_SECRET_JOBS:
            out.append(f"{name}: V13: job '{jid}' names the {SCOPED_SECRET_ENVIRONMENT!r} "
                       f"environment, but only {sorted(SCOPED_SECRET_JOBS)} may. Every job that "
                       f"names it can read {SCOPED_SECRET}.")
    return out
```

- [ ] **Step 4: Write V14**

Add directly below `credential_scope_violations`:

```python
# V14 (SMA-658). A CAPABILITY rule, not a spelling rule. The marker list can only ever catch a
# command someone already thought of; a job that HOLDS a write capability can publish with a tool
# nobody listed, a `with: push: true`, or a command inside a script. So the capability itself must
# sit behind an approval.
WRITE_SCOPES = ("packages", "id-token", "attestations", "contents")
CAPABILITY_ENVIRONMENTS = ("release-images", "release-publish")
_APP_TOKEN_ACTION = "actions/create-github-app-token"


def _holds_write_capability(job: dict, workflow_perms: object = None) -> str | None:
    """The capability this job holds, or None. The reason is returned for the message.

    S13 (SMA-658): a job-level `permissions:` block always wins — and, per GitHub's own semantics,
    sets every scope it does not name to `none` — so the workflow-level block is consulted only
    when the job declares none at all. This is the same fallback `id_token_violations` already
    applies; without it, a workflow-level `packages: write` would grant every job that declares no
    block of its own, invisibly to V14.
    """
    perms = job.get("permissions")
    if perms is None:
        perms = workflow_perms
    if isinstance(perms, dict):
        for scope in WRITE_SCOPES:
            if scope == "contents":
                continue  # `contents: write` alone is not a registry capability.
            if perms.get(scope) == "write":
                return f"permissions.{scope}: write"
    if isinstance(perms, str) and perms.strip() == "write-all":
        return "permissions: write-all"
    env_name = (_environment_name(job) or "").casefold()
    if env_name in CAPABILITY_ENVIRONMENTS:
        return f"environment: {env_name}"
    for step in steps_of(job, "a job"):
        if not isinstance(step, dict):
            continue
        uses = str(step.get("uses") or "")
        with_block = step.get("with")
        if _APP_TOKEN_ACTION in uses and isinstance(with_block, dict) \
                and with_block.get("permission-contents") == "write":
            return "an App token with contents: write"
    return None


def capability_violations(jobs: dict, workflow_perms: object, name: str) -> list[str]:
    """V14. Every job that holds a publish capability sits behind its chain's approval."""
    out: list[str] = []
    for jid, job in jobs.items():
        if not isinstance(job, dict) or jid in UNGATED_JOBS:
            continue
        reason = _holds_write_capability(job, workflow_perms)
        if reason is None:
            continue
        approval = approval_for_job(jid)
        if approval not in gated_path_jobs(jid, jobs):
            out.append(f"{name}: V14: job '{jid}' holds {reason}, but '{approval}' is not on its "
                       f"needs: path. A job with that capability can reach a registry with a tool "
                       f"no marker list names, so the capability itself must sit behind the gate.")
    return out
```

- [ ] **Step 5: Add the new publish and tag markers**

Replace the `PUBLISH_MARKERS` tuple's closing lines by adding these entries before the `)`:

```python
    # SMA-658. Container registries and the tag API. Bounded ends, like the release-plz marker
    # above: `crane pusher` must not match `crane push`.
    r"docker\s+push(?![-\w])",
    r"docker\s+buildx\s+build[^\n]*--push(?![-\w])",
    r"--output\s+type=registry(?![-\w])",
    r"docker\s+manifest\s+push(?![-\w])",
    r"imagetools\s+create(?![-\w])",
    r"crane\s+(push|copy|cp|tag|index|append)(?![-\w])",
    r"skopeo\s+copy(?![-\w])",
    r"regctl\s+(image\s+(copy|cp)|tag|index\s+create)(?![-\w])",
    r"oras\s+(push|cp|attach)(?![-\w])",
    r"cosign\s+(sign|attest|attach|copy)(?![-\w])",
    r"git/refs(?![-\w])",
```

- [ ] **Step 6: Give each new marker its own reding fixture (B7)**

B7: ten of these eleven markers have no fixture anywhere in `FIXTURES`, so any one of them could be
deleted later and nothing would notice. Add this directly above `self_test()`, never as a new
`*_self_test` table (Global Constraints: `SELF_TEST_COUNT` stays 16) — it registers into the
EXISTING `(check_name, fn)` list the same way `_v11_id_token_write_required` and the other
standalone helpers already do:

```python
# SMA-658 B7. One case per new PUBLISH_MARKERS entry, plus a release-pr `git push` negative
# control (S10) so the widened marker list does not red the real repository's own branch push.
# Driven straight at job_publishes(), at the granularity the markers were written for: a FIXTURES
# row would bury a deleted marker under whatever OTHER violation the same synthetic file happens
# to produce (see the renamed row in Step 1 above), where this helper reds on that one marker
# alone.
_SMA658_MARKER_CASES: tuple[tuple[str, bool], ...] = (
    ("docker push ghcr.io/smk1085/paigasus-iam:latest", True),
    ("docker buildx build --push -t ghcr.io/smk1085/paigasus-iam:latest .", True),
    ("docker buildx build --output type=registry -t ghcr.io/smk1085/paigasus-iam:latest .", True),
    ("docker manifest push ghcr.io/smk1085/paigasus-iam:latest", True),
    ("docker buildx imagetools create --tag ghcr.io/smk1085/paigasus-iam:latest "
     "x@sha256:" + "0" * 64, True),
    ("crane push /tmp/layout ghcr.io/smk1085/paigasus-iam:latest", True),
    ("crane copy ghcr.io/smk1085/paigasus-iam:latest docker.io/smaschek/paigasus-iam:latest", True),
    ("crane tag ghcr.io/smk1085/paigasus-iam@sha256:" + "0" * 64 + " latest", True),
    ("skopeo copy oci:layout docker://ghcr.io/smk1085/paigasus-iam:latest", True),
    ("regctl image copy ghcr.io/smk1085/paigasus-iam:latest docker.io/smaschek/paigasus-iam:latest",
     True),
    ("oras push ghcr.io/smk1085/paigasus-iam:latest ./file", True),
    ("cosign sign --yes ghcr.io/smk1085/paigasus-iam@sha256:" + "0" * 64, True),
    ('gh api --method POST "repos/o/r/git/refs" -f "ref=refs/tags/x"', True),
    # S10: release-pr's own branch push must stay green against the widened marker list.
    ('git push "$AUTH_REMOTE" "HEAD:$BRANCH"', False),
)


def _sma658_new_publish_markers_bite() -> str | None:
    for cmd, want in _SMA658_MARKER_CASES:
        got = job_publishes({"steps": [{"run": cmd}]}, "fixture")
        if got != want:
            return f"{cmd!r}: expected job_publishes()={want}, got {got}"
    return None
```

Register it in `self_test()`'s existing tuple list, alongside `_v12_npm_floor_pinned`:

```python
        ("sma-658 every new publish marker has a reding fixture", _sma658_new_publish_markers_bite),
```

- [ ] **Step 7: Scope `id-token: write` to the image chains (S1)**

Spec § 7.2 asks for two things: `publish-images-<svc>` joins the jobs that must HOLD
`id-token: write` (V11's existing direction), and `tag-<svc>` must NOT hold it (a direction V11
does not check today — no job outside `OIDC_PUBLISH_JOBS` is currently forbidden from holding the
grant). Extend `id_token_violations` with the second half. Replace the `OIDC_PUBLISH_JOBS` line:

```python
OIDC_PUBLISH_JOBS = ("publish-pypi", "publish-npm")
```

with:

```python
OIDC_PUBLISH_JOBS = ("publish-pypi", "publish-npm", "publish-images-iam", "publish-images-gateway")
```

In `id_token_violations`, directly above its final `return out`, add:

```python
    # SMA-658 S1. The grant is scoped: a job outside OIDC_PUBLISH_JOBS that holds
    # id-token: write can run its own OIDC exchange against a registry or a publisher this rule
    # never audited for it. tag-<svc> only calls the tag API with an App token, so it must never
    # hold this grant.
    for jid, job in jobs.items():
        if jid in OIDC_PUBLISH_JOBS or not isinstance(job, dict):
            continue
        job_grant = _grants_scope(job.get("permissions"), ID_TOKEN_SCOPE)
        if job_grant is None:
            job_grant = bool(workflow_grant)
        if job_grant:
            out.append(
                f"{name}: V11: job '{jid}' grants `{ID_TOKEN_SCOPE}: write` but is not in "
                f"OIDC_PUBLISH_JOBS. Only a job that runs a trusted-publishing exchange may hold "
                f"this scope; add it to OIDC_PUBLISH_JOBS if that is now true, or drop the grant."
            )
```

Add one FIXTURES row proving `tag-iam` reds if it ever gains the grant:

```python
    ("SMA-658 tag-iam gains id-token: write it does not need (V11 spec 7.2)", "main",
     _OK_IMAGES_MAIN.replace("  tag-iam:\n    needs: [publish-images-iam]",
                             "  tag-iam:\n    permissions: {id-token: write}\n    needs: [publish-images-iam]"),
     "V11"),
```

`_v11_id_token_write_required`'s existing test cases build a document with only `publish-pypi` and
`publish-npm` jobs, so the new loop above — which only fires for a job NOT in `OIDC_PUBLISH_JOBS`
— finds nothing there and that helper needs no change.

- [ ] **Step 8: Call both rules from `check_main` and V13 from every file**

In `check_main`, directly above its `return`, add:

```python
    out += credential_scope_violations(doc, name)
    out += capability_violations(doc["jobs"], doc.get("permissions"), name)
```

C3: `check_main`'s own accumulator is named `out`, not `violations` — that name belongs to `main()`'s
top-level accumulator (used two blocks below, in the `main()` snippet).

In `main()`, after the `callee_boundary_violations` call, add:

```python
    # V13 runs over EVERY workflow file, not only the release path: any workflow with a `main`
    # trigger could name the release-images environment and read the same secret. C6 (SMA-658):
    # glob both suffixes GitHub Actions accepts, so a future `.yaml` workflow is not invisible to
    # this sweep.
    workflow_paths = sorted(Path(".github/workflows").glob("*.yml")) \
        + sorted(Path(".github/workflows").glob("*.yaml"))
    for path in workflow_paths:
        if path.resolve() == main_path.resolve():
            continue
        violations += credential_scope_violations(load_workflow(path), path.name)
```

- [ ] **Step 9: Run the self-test to watch it pass**

Run: the Step 2 command.
Expected: no FAIL lines, exit code 0.

- [ ] **Step 10: Run the guard against every real workflow**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py --python '>=3.12' python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "rc=$?"
```
Expected: rc 0. `release.yml` has no image chain yet, and no other workflow names `release-images`.
`DOCKERHUB_TOKEN` is not yet in `EXPECTED_RELEASE_SECRETS` (B5 — Task 7 adds it), and `release.yml`
does not yet reference it either, so `secret_reference_violations` sees a matched empty set on both
sides and stays clean.

- [ ] **Step 11: Lint and commit**

Run: `uv run --locked --project py ruff check --config py/pyproject.toml ci/actionlint/`
Expected: `All checks passed!`

```bash
git add ci/actionlint/release_guard.py
git commit -m "$(cat <<'EOF'
feat(ci): scope the Docker Hub token and gate every publish capability

V13 allows the token only in a job whose environment is release-images,
and allows that environment only on a publish job. It compares
environment names case-folded and fails closed on an expression. V14
gates the capability itself, so a publish with a tool no marker names
still reds. V11 now also forbids id-token: write outside the jobs that
run a trusted-publishing exchange.

Co-Authored-By: <your model name> <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Add the two service chains to `release.yml`

**Files:**
- Modify: `.github/workflows/release.yml`
- Modify: `ci/actionlint/release_guard.py` (`EXPECTED_RELEASE_SECRETS` only — B5)

**Interfaces:**
- Consumes: Task 4's outputs (`skip_<svc>`, `version_<svc>`), PR 1's `ci/images/run.sh build-oci`,
  `load-oci`, `smoke`, and `ci/images/release_decision.py`'s `oci-digests`, `adopt`, `floating`.
- Produces: six jobs — `images-build-<svc>`, `approve-images-<svc>`, `publish-images-<svc>`,
  `tag-<svc>` for `iam` and `gateway`, plus `DOCKERHUB_TOKEN` added to `EXPECTED_RELEASE_SECRETS`
  in the same commit as the job that references it (B5). Task 8 mutates them; Task 9 documents
  them.

**Decision this task makes (the spec does not fix it):** the two chains share their step lists
through YAML anchors. MEASURED with actionlint 1.7.12 (B2): a top-level `x-image-chain:` key fails
`syntax-check` (`unexpected key "x-image-chain" for "workflow" section`) — GitHub's own workflow
schema uses the same fixed allowlist of top-level keys, and it does not include an `x-` escape
hatch the way `docker-compose` does. So each anchor (`&image-build-steps`, `&image-publish-steps`,
`&image-tag-steps`) is defined directly on the `iam` job that needs it first, as that job's own
`steps:` value, and aliased (`*image-build-steps`, …) from the matching `gateway` job's `steps:`.
YAML anchors and aliases are supported on any node, not only inside a dedicated block, and PyYAML
still expands the alias before `release_guard.py` reads the document, so every job is still
checked in full — MEASURED rc 0 with this shape.

- [ ] **Step 1: Add the plan outputs**

In the `plan` job's `outputs:` block, below the `nothing_to_release:` line, add:

```yaml
      # SMA-658. One skip flag and one version for each service chain. The version is what
      # publish-images-<svc> compares against the image's own label, so a chain can never publish
      # an archive built for a different version.
      skip_iam: ${{ steps.decide.outputs.skip_iam }}
      skip_gateway: ${{ steps.decide.outputs.skip_gateway }}
      version_iam: ${{ steps.decide.outputs.version_iam }}
      version_gateway: ${{ steps.decide.outputs.version_gateway }}
```

- [ ] **Step 2: Add the Docker Hub secret to the pin (B5)**

In `ci/actionlint/release_guard.py`, replace the `EXPECTED_RELEASE_SECRETS` tuple with:

```python
EXPECTED_RELEASE_SECRETS = (
    "PAIGASUS_BOT_APP_ID",
    "PAIGASUS_BOT_PRIVATE_KEY",
    # SMA-658 D7. Docker Hub offers OIDC connections only to organizations with a Team, Business
    # or DHI subscription, or in its Sponsored Open Source program. `smaschek` is a personal
    # account, so this token cannot be an OIDC exchange. V13 (Task 6) scopes it to one job.
    "DOCKERHUB_TOKEN",
)
```

B5: this pin and the `secrets.DOCKERHUB_TOKEN` reference this task adds below (Steps 3 and 4, the
`Fail on an empty Docker Hub token` step of each chain's publish job) land in the SAME commit.
`secret_reference_violations` reds `release.yml` for a pinned name the file does not reference, so
pinning this in Task 6 — before `release.yml` names the secret — would red Task 6's own guard run
against the real file.

- [ ] **Step 3: Add the `iam` chain**

The two chains share their step lists through YAML anchors (see the task's Decision note above).
`&image-build-steps`, `&image-publish-steps` and `&image-tag-steps` are each defined here, on the
`iam` job that needs them first, and aliased from the matching `gateway` job in Step 4. At the end
of the `jobs:` mapping, add:

```yaml
  # SMA-658. THE IMAGE CHAINS. Each service has its own four jobs and its own approval job, so a
  # kernel release and an image release never block each other in the job graph. release_guard.py
  # V8 asserts the per-chain boundary and V14 the capabilities. All approval jobs share the
  # `release-approval` environment, so one human approval releases every chain pending in the run
  # (GitHub approves by environment, not by job; measured on the first live release, run 35648073131).
  images-build-iam:
    name: build the iam image (${{ matrix.arch }})
    needs: [plan]
    # FAIL-SAFE POLARITY, exactly like the kernel gate above: anything but the literal 'true'
    # runs. An already released version is a no-op in publish-images-iam, not a red.
    if: needs.plan.outputs.skip_iam != 'true'
    runs-on: ${{ matrix.arch == 'arm64' && 'ubuntu-24.04-arm' || 'ubuntu-latest' }}
    timeout-minutes: 60
    permissions:
      contents: read
    strategy:
      fail-fast: false
      matrix:
        arch:
          - amd64
          - arm64
    # NO job outputs. A matrix job writes ONE set of outputs, so both legs would write the same
    # name and the last leg would win -- the amd64 digest could arrive under `digest_arm64`.
    # `publish-images-<svc>` reads each per-platform digest from its own archive instead, which is
    # the only source that cannot be crossed (spec Section 4.2's "job output" wording is corrected
    # here; see the self-review record).
    env:
      SERVICE: iam
      ARCH: ${{ matrix.arch }}
      PROTO_REPORTER: text
    # SMA-658 B2. This anchor is defined HERE, on the first job that needs it, and aliased
    # (*image-build-steps) from images-build-gateway. MEASURED with actionlint 1.7.12: a top-level
    # x-image-chain: key fails schema validation; an anchor on an ordinary job key does not.
    steps: &image-build-steps
      - name: Reclaim runner disk (drop unused preinstalled toolchains)
        run: |
          sudo rm -rf /usr/local/lib/android /usr/share/dotnet /opt/ghc /usr/local/.ghcup \
            /opt/hostedtoolcache/CodeQL || true
          sudo docker image prune --all --force > /dev/null 2>&1 || true

      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          persist-credentials: false

      - name: Set up Buildx
        uses: docker/setup-buildx-action@37fe631027851001ddb9b187196cc803df7f5f0e  # v4.3.0

      - name: Set up proto (pinned via .prototools)
        uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0
        with:
          cache: false

      - name: Install crane, syft and uv
        run: |
          proto install crane
          proto install syft
          proto install uv

      - name: Build the image as an OCI archive
        run: ci/images/run.sh build-oci "${SERVICE}" out

      - name: Load the archive and check its identity
        id: load
        run: |
          set -euo pipefail
          archive="out/paigasus-${SERVICE}-${ARCH}.oci.tar"
          ci/images/run.sh load-oci "$archive" "paigasus-${SERVICE}:dev"
          digests="$(uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py oci-digests "$archive")"
          manifest="$(printf '%s\n' "$digests" | sed -n 's/^manifest=//p')"
          echo "digest=${manifest}" >> "$GITHUB_OUTPUT"

      - name: Smoke this service
        run: ci/images/run.sh smoke "${SERVICE}"

      - name: SBOM for this platform
        run: syft "oci-archive:out/paigasus-${SERVICE}-${ARCH}.oci.tar" -o "spdx-json=out/sbom-paigasus-${SERVICE}-${ARCH}.spdx.json"

      - name: Report the SBOM contents (spec AC 7)
        run: |
          set -euo pipefail
          summary="$(uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-summary "out/sbom-paigasus-${SERVICE}-${ARCH}.spdx.json")"
          echo "M8 image=paigasus-${SERVICE} arch=${ARCH} ${summary}"
          cargo_count="$(printf '%s\n' "$summary" | sed -n 's/^cargo=//p')"
          if [ "${cargo_count:-0}" -lt 1 ]; then
            echo "::error::the SBOM lists no Rust crates; the binary was not built with cargo auditable (spec AC 7)." >&2
            exit 1
          fi

      - name: Upload the archive and its SBOM
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: image-${{ env.SERVICE }}-${{ matrix.arch }}
          path: |
            out/paigasus-${{ env.SERVICE }}-${{ matrix.arch }}.oci.tar
            out/sbom-paigasus-${{ env.SERVICE }}-${{ matrix.arch }}.spdx.json
          if-no-files-found: error
          retention-days: 7

  approve-images-iam:
    name: approve the iam image release
    needs: [images-build-iam]
    runs-on: ubuntu-latest
    timeout-minutes: 5
    environment: release-approval
    steps:
      - run: echo "Approved. Everything below this point is irreversible."

  publish-images-iam:
    name: publish the iam image
    needs: [plan, images-build-iam, approve-images-iam]
    if: needs.plan.outputs.skip_iam != 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    environment: release-images
    concurrency:
      group: release-images-iam
      cancel-in-progress: false
    permissions:
      contents: read
      packages: write
      id-token: write
      attestations: write
    outputs:
      digest: ${{ steps.publish.outputs.index }}
      revision: ${{ steps.publish.outputs.revision }}
    env:
      SERVICE: iam
      VERSION: ${{ needs.plan.outputs.version_iam }}
      GHCR_IMAGE: ghcr.io/smk1085/paigasus-iam
      HUB_IMAGE: docker.io/smaschek/paigasus-iam
      PROTO_REPORTER: text
    steps: &image-publish-steps
      - name: Fail on an empty Docker Hub token
        env:
          DOCKERHUB_TOKEN: ${{ secrets.DOCKERHUB_TOKEN }}
        run: |
          if [ -z "$DOCKERHUB_TOKEN" ]; then
            echo "::error::DOCKERHUB_TOKEN is empty or unset; the release-images environment secret did not reach this job." >&2
            exit 1
          fi

      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          fetch-depth: 0
          persist-credentials: false

      - name: Set up Buildx
        uses: docker/setup-buildx-action@37fe631027851001ddb9b187196cc803df7f5f0e  # v4.3.0

      - name: Set up proto (pinned via .prototools)
        uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0
        with:
          cache: false

      - name: Install crane, cosign and uv
        run: |
          proto install crane
          proto install cosign
          proto install uv

      - name: Download both archives
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          pattern: image-${{ env.SERVICE }}-*
          merge-multiple: true
          path: in

      - name: Log in to GHCR
        uses: docker/login-action@dbcb813823bdd20940b903addbd779551569679f  # v4.6.0
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ github.token }}

      - name: Check the artifact hop and read the decision
        id: decide
        run: |
          set -euo pipefail
          # S3 (SMA-658): only a registry's own "no such tag" answer may read as "the digest is
          # absent". Any other crane failure -- a timeout, an auth error, a transient 5xx -- must
          # fail the job, because reading it as absent would push a second digest under a
          # published version (D10).
          digest_or_none() {
            local image="$1" rc=0 out
            out="$(crane digest "$image" 2>&1)" || rc=$?
            if [ "$rc" -eq 0 ]; then
              printf '%s\n' "$out"
              return 0
            fi
            case "$out" in
              *MANIFEST_UNKNOWN*|*NAME_UNKNOWN*|*"unexpected status code 404"*)
                echo none
                ;;
              *)
                echo "::error::crane digest failed for ${image}: ${out}" >&2
                exit 1
                ;;
            esac
          }
          for arch in amd64 arm64; do
            archive="in/paigasus-${SERVICE}-${arch}.oci.tar"
            digests="$(uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py oci-digests "$archive")"
            printf '%s\n' "$digests" | sed -n "s/^manifest=/digest_${arch}=/p" >> "$GITHUB_OUTPUT"
          done
          # `crane config` reads only a registry reference; it cannot read a local archive
          # (SMA-658 C1, MEASURED), so read the two labels with `release_decision.py labels`.
          labels="$(uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py labels "in/paigasus-${SERVICE}-amd64.oci.tar")"
          label="$(printf '%s\n' "$labels" | sed -n 's/^version=//p')"
          if [ "$label" != "$VERSION" ]; then
            echo "::error::the archive carries version ${label}, but plan says ${VERSION}." >&2
            exit 1
          fi
          revision="$(printf '%s\n' "$labels" | sed -n 's/^revision=//p')"
          echo "revision=${revision}" >> "$GITHUB_OUTPUT"
          tag="paigasus-${SERVICE}-v${VERSION}"
          if git ls-remote --tags --exit-code origin "refs/tags/${tag}" > /dev/null 2>&1; then
            git_tag=present
          else
            git_tag=absent
          fi
          ghcr="$(digest_or_none "${GHCR_IMAGE}:${VERSION}")"
          hub="$(digest_or_none "${HUB_IMAGE}:${VERSION}")"
          echo "ghcr=${ghcr} dockerhub=${hub} git_tag=${git_tag}"
          echo "ghcr=${ghcr}" >> "$GITHUB_OUTPUT"
          echo "dockerhub=${hub}" >> "$GITHUB_OUTPUT"
          echo "git_tag=${git_tag}" >> "$GITHUB_OUTPUT"

      - name: Push, copy, tag and sign
        id: publish
        env:
          DOCKERHUB_TOKEN: ${{ secrets.DOCKERHUB_TOKEN }}
          DIGEST_AMD64: ${{ steps.decide.outputs.digest_amd64 }}
          DIGEST_ARM64: ${{ steps.decide.outputs.digest_arm64 }}
          GHCR_EXISTING: ${{ steps.decide.outputs.ghcr }}
          HUB_EXISTING: ${{ steps.decide.outputs.dockerhub }}
          GIT_TAG_STATE: ${{ steps.decide.outputs.git_tag }}
          REVISION: ${{ steps.decide.outputs.revision }}
        run: |
          set -euo pipefail
          # D10: the FIRST digest published under :<version> is final. A later run adopts it and
          # discards its own build, so a rebuild -- which never reproduces a digest, because the
          # chisel cut resolves the live archive -- can never replace a published image.
          # C5 (SMA-658): --new-digest feeds only the push-new branch below, so it must be a real
          # digest, not a placeholder -- pass the amd64 archive's own manifest digest.
          decision="$(uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py adopt \
            --new-digest "$DIGEST_AMD64" \
            --ghcr "$GHCR_EXISTING" --dockerhub "$HUB_EXISTING" --git-tag "$GIT_TAG_STATE")"
          action="$(printf '%s\n' "$decision" | sed -n 's/^action=//p')"
          if [ "$action" = "already-released" ]; then
            echo "already released: the tag paigasus-${SERVICE}-v${VERSION} exists. Nothing to do."
            echo "index=" >> "$GITHUB_OUTPUT"
            echo "revision=${REVISION}" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          if [ "$action" = "push-new" ]; then
            refs=()
            for arch in amd64 arm64; do
              layout="$(mktemp -d)"
              tar -xf "in/paigasus-${SERVICE}-${arch}.oci.tar" -C "$layout"
              crane push "$layout" "${GHCR_IMAGE}:${GITHUB_SHA}-${arch}"
              got="$(crane digest "${GHCR_IMAGE}:${GITHUB_SHA}-${arch}")"
              # S5 (SMA-658): images-rehearsal.yml carries this exact check (run 35498922565). A
              # changed per-platform digest would later name an attest-sbom subject the registry
              # does not hold.
              case "$arch" in
                amd64) want="$DIGEST_AMD64" ;;
                arm64) want="$DIGEST_ARM64" ;;
              esac
              if [ "$got" != "$want" ]; then
                echo "::error::crane push changed the ${arch} digest: ${want} -> ${got}" >&2
                exit 1
              fi
              refs+=("${GHCR_IMAGE}@${got}")
            done
            docker buildx imagetools create --tag "${GHCR_IMAGE}:${GITHUB_SHA}" "${refs[@]}"
            index="$(crane digest "${GHCR_IMAGE}:${GITHUB_SHA}")"
            echo "index=${index}" >> "$GITHUB_OUTPUT"
            echo "revision=${REVISION}" >> "$GITHUB_OUTPUT"
            echo "$DOCKERHUB_TOKEN" | docker login docker.io --username smaschek --password-stdin
            docker buildx imagetools create --tag "${HUB_IMAGE}:${GITHUB_SHA}" "${GHCR_IMAGE}@${index}"
            hub_index="$(crane digest "${HUB_IMAGE}:${GITHUB_SHA}")"
            if [ "$hub_index" != "$index" ]; then
              echo "::error::the Docker Hub copy changed the digest: ${index} -> ${hub_index}" >&2
              exit 1
            fi
          else
            index="$(printf '%s\n' "$decision" | sed -n 's/^digest=//p')"
            copy_to="$(printf '%s\n' "$decision" | sed -n 's/^copy_to=//p')"
            echo "adopting the published digest ${index} (copy_to=${copy_to})"
            echo "index=${index}" >> "$GITHUB_OUTPUT"
            echo "revision=${REVISION}" >> "$GITHUB_OUTPUT"
            # S4 (SMA-658): `adopt` names which registry is MISSING the digest. Copying from the
            # wrong registry fails outright, because that registry may not hold it at all.
            if [ "$copy_to" = "dockerhub" ]; then
              echo "$DOCKERHUB_TOKEN" | docker login docker.io --username smaschek --password-stdin
              docker buildx imagetools create --tag "${HUB_IMAGE}:${GITHUB_SHA}" "${GHCR_IMAGE}@${index}"
              hub_index="$(crane digest "${HUB_IMAGE}:${GITHUB_SHA}")"
              if [ "$hub_index" != "$index" ]; then
                echo "::error::the Docker Hub copy changed the digest: ${index} -> ${hub_index}" >&2
                exit 1
              fi
            elif [ "$copy_to" = "ghcr" ]; then
              echo "$DOCKERHUB_TOKEN" | docker login docker.io --username smaschek --password-stdin
              docker buildx imagetools create --tag "${GHCR_IMAGE}:${GITHUB_SHA}" "${HUB_IMAGE}@${index}"
              ghcr_index="$(crane digest "${GHCR_IMAGE}:${GITHUB_SHA}")"
              if [ "$ghcr_index" != "$index" ]; then
                echo "::error::the GHCR copy changed the digest: ${index} -> ${ghcr_index}" >&2
                exit 1
              fi
            else
              echo "$DOCKERHUB_TOKEN" | docker login docker.io --username smaschek --password-stdin
            fi
          fi
          cosign sign --yes "${GHCR_IMAGE}@${index}"
          cosign sign --yes "${HUB_IMAGE}@${index}"
          git tag -l > /tmp/tags.txt
          floating="$(uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py floating \
            --service "$SERVICE" --version "$VERSION" --tags-file /tmp/tags.txt)"
          move="$(printf '%s\n' "$floating" | sed -n 's/^move=//p')"
          minor="$(printf '%s\n' "$floating" | sed -n 's/^minor_tag=//p')"
          for image in "$GHCR_IMAGE" "$HUB_IMAGE"; do
            crane tag "${image}@${index}" "$VERSION"
            if [ "$move" = "true" ]; then
              crane tag "${image}@${index}" "$minor"
              crane tag "${image}@${index}" latest
            fi
          done
          # S15 (SMA-658, should-fix): spec Section 4.3 step 6 says logout happens "at once" after
          # the Hub cosign sign. It happens here instead, because the crane tag loop above ALSO
          # writes to Docker Hub and needs the same credential -- moving logout earlier would
          # break that write. This is a deliberate, recorded deviation (spec Section 16 P15);
          # verify on the first live release whether a later measurement lets it move up.
          docker logout docker.io

      - name: Attest build provenance (index)
        if: steps.publish.outputs.index != ''
        uses: actions/attest-build-provenance@4d101475d8b20a2381f78447822ac1eab6504dd8  # v4.2.2
        with:
          subject-name: ${{ env.GHCR_IMAGE }}
          subject-digest: ${{ steps.publish.outputs.index }}
          push-to-registry: true

      - name: Attest the amd64 SBOM (per-platform subject)
        if: steps.publish.outputs.index != ''
        uses: actions/attest-sbom@c604332985a26aa8cf1bdc465b92731239ec6b9e  # v4.1.0
        with:
          subject-name: ${{ env.GHCR_IMAGE }}
          subject-digest: ${{ steps.decide.outputs.digest_amd64 }}
          sbom-path: in/sbom-paigasus-${{ env.SERVICE }}-amd64.spdx.json
          push-to-registry: true

      - name: Attest the arm64 SBOM (per-platform subject)
        if: steps.publish.outputs.index != ''
        uses: actions/attest-sbom@c604332985a26aa8cf1bdc465b92731239ec6b9e  # v4.1.0
        with:
          subject-name: ${{ env.GHCR_IMAGE }}
          subject-digest: ${{ steps.decide.outputs.digest_arm64 }}
          sbom-path: in/sbom-paigasus-${{ env.SERVICE }}-arm64.spdx.json
          push-to-registry: true

      # SMA-658 B6. The plan's original single "Push, copy, tag, sign and verify" step ran
      # `gh attestation verify` BEFORE the three attest-* steps above created anything to verify
      # -- on a first release that check always failed. Verification is its own, later step.
      - name: Verify the signature and attestations
        if: steps.publish.outputs.index != ''
        env:
          GH_TOKEN: ${{ github.token }}
          INDEX: ${{ steps.publish.outputs.index }}
        run: |
          set -euo pipefail
          index="$INDEX"
          for image in "$GHCR_IMAGE" "$HUB_IMAGE"; do
            cosign verify \
              --certificate-identity "https://github.com/SMK1085/paigasus-core/.github/workflows/release.yml@refs/heads/main" \
              --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
              "${image}@${index}"
            gh attestation verify "oci://${image}@${index}" \
              --repo SMK1085/paigasus-core \
              --signer-workflow SMK1085/paigasus-core/.github/workflows/release.yml \
              --source-ref refs/heads/main
          done
          echo "PUBLISHED ${SERVICE} ${VERSION}: ${index}"

  tag-iam:
    name: tag the iam release
    needs: [plan, publish-images-iam]
    if: needs.plan.outputs.skip_iam != 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 10
    environment: release-publish
    permissions:
      contents: read
    env:
      SERVICE: iam
      VERSION: ${{ needs.plan.outputs.version_iam }}
      REVISION: ${{ needs.publish-images-iam.outputs.revision }}
    steps: &image-tag-steps
      - name: Mint the App installation token
        id: app_token
        uses: actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1  # v3.2.0
        with:
          client-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}
          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}
          permission-contents: write

      - name: Make the release tag
        env:
          GH_TOKEN: ${{ steps.app_token.outputs.token }}
        run: |
          set -euo pipefail
          tag="paigasus-${SERVICE}-v${VERSION}"
          existing="$(gh api "repos/${GITHUB_REPOSITORY}/git/ref/tags/${tag}" --jq .object.sha 2> /dev/null || echo none)"
          if [ "$existing" = "$REVISION" ]; then
            echo "the tag ${tag} already points at ${REVISION}. Nothing to do."
            exit 0
          fi
          if [ "$existing" != "none" ]; then
            echo "::error::the tag ${tag} points at ${existing}, not at the image's revision ${REVISION}." >&2
            exit 1
          fi
          gh api --method POST "repos/${GITHUB_REPOSITORY}/git/refs" \
            -f "ref=refs/tags/${tag}" -f "sha=${REVISION}"
          echo "TAGGED ${tag} at ${REVISION}"
```

- [ ] **Step 4: Add the `gateway` chain**

Copy the four `iam` jobs from Step 3, and change in the copies: every job id suffix `-iam` to
`-gateway`, `SERVICE: iam` to `SERVICE: gateway`, `skip_iam` to `skip_gateway`, `version_iam` to
`version_gateway`, `needs.publish-images-iam` to `needs.publish-images-gateway`,
`group: release-images-iam` to `group: release-images-gateway`, and both image names from
`paigasus-iam` to `paigasus-gateway`. The three `steps:` VALUES become aliases
(`steps: *image-build-steps`, `steps: *image-publish-steps`, `steps: *image-tag-steps`) instead of
repeating the step lists — the anchors read the service from `env:`, so no step text differs
between the two chains. Drop the `# SMA-658 B2. This anchor is defined HERE…` comment from the
copy; it applies only to the job that defines the anchor.

- [ ] **Step 5: Lint the workflow**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
actionlint -shellcheck= .github/workflows/release.yml; echo "rc=$?"
```
Expected: rc 0. B2 was MEASURED against this exact anchor-on-the-iam-job shape with actionlint
1.7.12: rc 0, where a top-level `x-image-chain:` key rc's 1.

- [ ] **Step 6: Run the guard against the real file**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py --python '>=3.12' python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "rc=$?"
```
Expected: rc 0. If V8c reds a chain job, its `needs:` is missing its own approval job. If V13 reds,
a job outside `publish-images-<svc>` names `release-images` or reads the token. If V11 reds
`tag-iam` or `tag-gateway`, one of them gained `id-token: write` it does not need (S1).

- [ ] **Step 7: Prove the anchors expand**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py --python '>=3.12' python3 -c "
import yaml
doc = yaml.safe_load(open('.github/workflows/release.yml'))
jobs = doc['jobs']
for jid in ('images-build-iam', 'publish-images-gateway', 'tag-iam'):
    print(jid, len(jobs[jid]['steps']), 'steps')
"
```
Expected: each job reports a step count above zero. A zero, or a `KeyError`, means an alias did not
resolve, and the guard would then check an empty job.

- [ ] **Step 8: Check the early-exit-reader ban**

Run:
```bash
grep -nE "\|[[:space:]]*(grep[[:space:]]+-[a-zA-Z]*q|grep[[:space:]]+-[a-zA-Z]*m|head|awk)" .github/workflows/release.yml | grep -v "^[[:space:]]*#"
```
Expected: no output. Every pipe in the new steps reads its whole input (`sed -n 's/…/p'`). This
verification command itself does not pipe into `head` (C4: an earlier draft did, in violation of
the Global Constraints' own ban) — it is an interactive check, not a gated script, but there is no
reason for it to break its own rule.

- [ ] **Step 9: Commit**

```bash
git add .github/workflows/release.yml ci/actionlint/release_guard.py
git commit -m "$(cat <<'EOF'
feat(ci): release each service image from its own chain

Each service gets build, approve, publish and tag jobs. The publish job
adopts a digest that is already published, copies the index to Docker
Hub, signs both, moves the floating tags only forward, and verifies the
result in its own step, after the attestations exist. The tag job runs
last, so a tag never names an image that does not exist.

Co-Authored-By: <your model name> <noreply@anthropic.com>
EOF
)"
```

**What only a live release can prove:** the Docker Hub login and copy, the cosign signature on
Docker Hub, the `gh attestation verify` result for a `release.yml` identity, and the tag API call.
`images-rehearsal.yml` has proven the GHCR half of this sequence on `main` (run 35498922565).

---

### Task 8: Mutate the new rules and confirm each one bites

**Files:**
- Modify: none, unless a mutation survives.
- Test: `ci/actionlint/release_guard.py --self-test`, `ci/release-plan/run.sh --self-test`

**Interfaces:**
- Consumes: Tasks 3 to 7.
- Produces: a recorded mutation log in the task report, and a fix for any mutation that survived.

- [ ] **Step 1: Prepare a restore point**

Run: `git rev-parse HEAD > /tmp/sma658-mutation-base.txt && cat /tmp/sma658-mutation-base.txt`
Expected: the commit from Task 7. Restore each mutation by reverting the ONE edit you made, never
with `git checkout --`, which would also discard an uncommitted fix.

- [ ] **Step 2: Mutate V8's per-chain map**

Edit `ci/actionlint/release_guard.py`: in `approval_for_job`, replace the loop body's
`return approval` with `return APPROVAL_JOB`.
Run: `uv run --locked --project py --python '>=3.12' python3 ci/actionlint/release_guard.py --self-test`
Expected: FAIL for `SMA-658 a publisher behind the KERNEL approval only`. Restore the line.

- [ ] **Step 3: Mutate V13's case-folding**

Replace `env_name = (_environment_name(job) or "").casefold()` with
`env_name = _environment_name(job) or ""`.
Run: the same self-test.
Expected: FAIL for `SMA-658 an environment name that differs only by case`, which expects a clean
verdict. Restore the line.

- [ ] **Step 4: Mutate V13's expression guard**

Delete the `if "${{" in raw_text:` block.
Run: the same self-test.
Expected: FAIL for `SMA-658 an environment name built from an expression`. Restore the block.

- [ ] **Step 5: Mutate V14's capability list**

Remove `"packages"` from `WRITE_SCOPES`.
Run: the same self-test.
Expected: FAIL for `SMA-658 a write capability upstream of the approval`. Restore the entry.

- [ ] **Step 6: Mutate the new markers**

Delete the `r"imagetools\s+create(?![-\w])",` entry from `PUBLISH_MARKERS`.
Run: the same self-test.
Expected: FAIL for `"sma-658 every new publish marker has a reding fixture"` — the B7 helper
(Task 6 Step 6) catches this directly, naming the `docker buildx imagetools create …` case. Restore
the entry.

- [ ] **Step 7: Mutate the service skip rule**

Edit `ci/release-plan/release_plan.py`: in `service_skips`, replace the body with `return False`.
Run: `uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --self-test`
Expected: FAIL for `the tag already exists -> skip` and `still 0.0.0 -> skip`. Restore the body.

- [ ] **Step 8: Mutate the changelog reader**

Replace `changelog_names_version`'s body with `return True`.
Run: the same self-test.
Expected: FAIL for `only the unreleased section`, `another version only` and
`the version inside prose, not a heading`. Restore the body.

- [ ] **Step 9: Confirm the tree is clean and re-run everything**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git status --short
uv run --locked --project py --python '>=3.12' python3 ci/actionlint/release_guard.py --self-test; echo "guard rc=$?"
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --self-test; echo "plan rc=$?"
/bin/bash ci/release-plan/run.sh --negative-control; echo "negctl rc=$?"
/bin/bash ci/actionlint/run.sh --self-test 2>&1 | grep -c "^FAIL" || true
```
Expected: `git status --short` prints nothing, the first three report rc 0, and the last prints
`2` — the two known false `cargo-lock-step` rows. A third failure is real.

- [ ] **Step 10: Commit only if a mutation survived**

If every mutation was caught, there is nothing to commit; record the log in the task report. If one
survived, add the missing fixture row or tighten the rule, then commit:

```bash
git add ci/actionlint/release_guard.py ci/release-plan/release_plan.py
git commit -m "$(cat <<'EOF'
test(ci): close a gap the mutation pass found

Co-Authored-By: <your model name> <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Document the release path

**Files:**
- Modify: `docs/ops/RUNBOOK-containers.md`, `CLAUDE.md`,
  `docs/superpowers/specs/2026-09-19-sma-658-container-release-design.md`
- Modify: `ci/actionlint/release_guard.py`, `.github/workflows/release.yml` (one reworded string
  in each, S2 — no behavior change)

**Interfaces:**
- Consumes: every earlier task.
- Produces: no code interface.

- [ ] **Step 1: Add the release procedure to the runbook**

Append to `docs/ops/RUNBOOK-containers.md`:

```markdown
## Release a service image

A maintainer sets a service version by hand. release-plz does not process these crates, because
their Cargo manifests set `publish = false` (SMA-658, spec § 3.1).

1. Open a pull request with the title `chore(rs): release paigasus-<svc> v<version>`. In it:
   - set `version` in `rs/crates/services/paigasus-<svc>/Cargo.toml`;
   - run `cargo update --manifest-path rs/Cargo.toml --workspace --offline`;
   - add a `## [<version>] - <date>` section to that crate's `CHANGELOG.md`.
   `repo:actionlint` check 11 fails the pull request when the changelog section is missing.
2. Merge it. The `plan` job selects the service, because its version has no tag.
3. Approve the `approve-images-<svc>` job. Each service chain has its own approval job, but all
   three approval jobs use the same `release-approval` environment. GitHub approves a pending
   deployment by environment, not by job. So one approval releases every chain that waits for
   approval in the same run. This comes from the shape of GitHub's approval API. It has not yet
   been observed on a live run. To release only one chain, keep only that chain pending: put only
   that service's version bump in the release, and do not combine it with a kernel release you
   want to hold back.
4. The `publish-images-<svc>` job pushes to GHCR, copies the index to Docker Hub, signs both,
   moves `:<major>.<minor>` and `:latest` only forward, and verifies the result. While the version
   is `0.x`, no `:<major>` tag is made. The `tag-<svc>` job then makes
   `paigasus-<svc>-v<version>`.
5. After the first push of a new package, set the GHCR package to public and link it to the
   repository. GitHub makes every new package private.

### Verify a published image

```bash
cosign verify \
  --certificate-identity "https://github.com/SMK1085/paigasus-core/.github/workflows/release.yml@refs/heads/main" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  ghcr.io/smk1085/paigasus-iam@<digest>

gh attestation verify "oci://ghcr.io/smk1085/paigasus-iam@<digest>" \
  --repo SMK1085/paigasus-core \
  --signer-workflow SMK1085/paigasus-core/.github/workflows/release.yml \
  --source-ref refs/heads/main
```

Both registries hold the same index digest. GHCR also stores the attestations; Docker Hub stores
only the cosign signature. `gh attestation verify` reads the GitHub API, so it works for an image
pulled from either registry.

### What a re-run does

The first digest published under `:<version>` is final. A later run reads that digest, discards
its own build and continues with the published one, so a re-run can never replace a released
image. A rebuild never reproduces a digest: the chisel cut resolves the live Ubuntu archive on
every build.

### If the release plan itself cannot be read

`ci/release-plan/run.sh`'s fail-safe branch writes `skip_iam=false` and `skip_gateway=false` (S12,
so both chains RUN — the fail-safe direction). It also writes `version_iam` and `version_gateway`
as explicit empty strings; it does not omit them. Every chain job that got past its gate then
hard-fails at the label compare (`the archive carries version , but plan says .`), because
`plan`'s version output is an empty string. Read that specific failure as "the release plan could
not be read" — check the `plan` job's own log, and inspect the empty `version_iam`/`version_gateway`
outputs there — not as a build problem in `images-build-<svc>`.
```

- [ ] **Step 2: Add the CLAUDE.md entries**

Append to the Gotchas list in `CLAUDE.md`:

```markdown
- Each service image releases through **its own chain** in `release.yml`: `images-build-<svc>` →
  `approve-images-<svc>` → `publish-images-<svc>` → `tag-<svc>`, for `iam` and `gateway`. The
  chains are independent of the kernel chain and of each other, so a kernel-only release, an
  image-only release and a combined release all work, and a failed image chain does not stop the
  kernel release. `release_guard.py` V8 asserts that a publisher's job depends, in the job graph,
  on the approval job of ITS OWN chain — a kernel approval job never gates an image push job. All
  three approval jobs (`approve-release`, `approve-images-iam`, `approve-images-gateway`) share the
  one `release-approval` environment, though. GitHub approves a pending deployment by environment,
  not by job, so one human approval releases every chain that waits for approval in the same run.
  MEASURED on the first live release (run 35648073131, 2026-09-21): the run listed one pending
  deployment for the two waiting approval jobs, and one approval released both chains.
  V14 asserts the same job-graph rule for the CAPABILITY (`packages: write`, `id-token: write`,
  `attestations: write`, the `release-images` or `release-publish` environment, an App token with
  `contents: write`), so a publish with a tool no marker names still reds. V13 allows
  `DOCKERHUB_TOKEN` only in a job whose environment is `release-images`, compares environment names
  case-folded, and fails closed on an `environment:` built from an expression.
- **The first digest published under `:<version>` is final** (D10). A later run adopts it and
  discards its own build. A rebuild never reproduces a digest, because `chisel cut` resolves the
  live Ubuntu archive on every build, so "push the same digest again" is not available as a
  recovery. `:<major>.<minor>` and `:latest` move only forward, compared as numbers. `:<major>`
  moves the same way, but only once the service leaves `0.x`.
- A service version is set **by hand**, in a normal pull request, with a `CHANGELOG.md` section.
  The two service crates are `publish = false` and sit in no `version_group`, so `release-plz
  update` never sees them, and `git_only` hard-errors on the second release because each has an
  unpublished workspace dependency (MEASURED, SMA-658 M7). See the release-plz entry above for
  the bound on that claim: a `publish = false` crate inside a group with a publishable head IS
  version-written. `ci/release-plan/release_plan.py
  --assert`, which `repo:actionlint` check 11 runs on every pull request, fails when a bumped
  service has no changelog section.
- `rs/Dockerfile` builds the services with **`cargo auditable`**, which is what makes the image
  SBOM list the Rust crates. MEASURED (SMA-658, 2026-09-20): a plain `cargo build` gives `cargo=0`.
  The OS-package half of the SBOM is still empty and is tracked as SMA-665: syft 1.52.0 reads only
  `/var/lib/dpkg/status` or `.deb` files, and a chisel cut writes neither. The `base-files_chisel`
  slice does NOT help — it writes a chisel-specific manifest that syft cannot read.
```

S8 (should-fix): a second, stale `0.0.0` statement survives in the existing `release-plz.toml`
bullet. Find this text (lines 451-452):

```markdown
one explicitly. `paigasus-gateway` / `paigasus-iam` stay at `0.0.0` deliberately: their
`env!("CARGO_PKG_VERSION")` feeds `ServiceInfo`, and ADR-0020 skew reporting is parked on that
value (SMA-505 R7).
```

and replace it with:

```markdown
one explicitly. `paigasus-gateway` / `paigasus-iam` are versioned BY HAND (SMA-658): each moved
to `0.1.0` at the first image release. `packages_to_process()` filters on Cargo's own `publish`
field, and neither crate is in a `version_group`, so `release-plz update` never sees them (M7,
measured on 0.3.158). That is the scoped claim, not a blanket one: a `publish = false` crate
inside a group whose head is publishable still gets its `[package] version` written, which the
version-lockstep entry records as measured. `env!("CARGO_PKG_VERSION")` feeds `ServiceInfo`, and
ADR-0020 skew reporting is parked on that value (SMA-505 R7).
```

- [ ] **Step 3: Reword the ADR-0011 tag-ownership statement (S2)**

Spec § 13 makes an ADR-0011 amendment a precondition of this plan and asks for two existing
"release-plz owns every tag" statements to be reworded, since `tag-<svc>` (Task 7) now cuts a tag
too. In `ci/actionlint/release_guard.py`, in `napi_violations`, replace:

```python
                        f"{name}: job '{job_id}' runs `napi prepublish` without "
                        f"--no-gh-release. release-plz owns every tag (ADR-0011 S3); napi "
                        f"must never cut one."
```

with:

```python
                        f"{name}: job '{job_id}' runs `napi prepublish` without "
                        f"--no-gh-release. release-plz owns every CRATE tag (ADR-0011 S3, "
                        f"amended); napi must never cut one."
```

In `.github/workflows/release.yml` (near line 1066), in the `publish-npm` job's comment above
`--no-gh-release`, replace these two lines:

```yaml
      # defaults ON). release-plz owns every tag — ADR-0011 S3, "the tool owns every tag",
      # singular. ci/actionlint/release_guard.py's V5 fails if this flag is ever dropped.
```

with:

```yaml
      # defaults ON). release-plz owns every CRATE tag — ADR-0011 S3, amended. tag-<svc>
      # (SMA-658) separately owns each service's IMAGE tag. ci/actionlint/release_guard.py's V5
      # fails if this flag is ever dropped.
```

The line above and the four lines below stay unchanged. Record in the task report that ADR-0011
itself needs the matching amendment in Notion — this plan cannot edit Notion directly.

- [ ] **Step 4: Record the plan's decisions in the spec**

Append to § 16 of `docs/superpowers/specs/2026-09-19-sma-658-container-release-design.md`:

```markdown
| P11 | `plan` outputs a VERSION for each service, not only a skip flag. § 6.1 lists only `skip_*`, but § 4.3 step 2 compares the image label with "plan's version", and no other job knows it. |
| P12 | `publish-images-<svc>` and `tag-<svc>` name `plan` in `needs:` and carry their chain's gate literal. V9b's subject is every direct consumer of `plan`, so a consumer without a literal would red. |
| P13 | The two chains share their step lists through YAML anchors. MEASURED (SMA-658 B2, actionlint 1.7.12): a top-level `x-image-chain:` key fails schema validation (`unexpected key`), so each anchor (`&image-build-steps`, `&image-publish-steps`, `&image-tag-steps`) is defined on the `iam` job that needs it first, as that job's own `steps:` value, and aliased from the matching `gateway` job. PyYAML still expands the alias before `release_guard.py` reads the file, so each job is checked in full. |
| P14 | `cargo-auditable` is pinned at `0.7.6` with `cargo install --locked --version`, in the builder stage. It has no proto plugin, so this is the only pin available to it. |
| P15 | `docker logout docker.io` runs after the `crane tag` loop, not immediately after the Hub `cosign sign` (§ 4.3 step 6's "at once" wording). `crane tag`'s Hub write also needs the credential `docker login` set, so logging out earlier would break it (SMA-658 S15, should-fix — not measured live; verify on the first live release whether that constraint still holds). |
```

- [ ] **Step 5: Check the markers are intact**

Run:
```bash
grep -c "ci-targets:begin\|ci-targets:end" CLAUDE.md; grep -c "moon-diagnosis:begin\|moon-diagnosis:end" CLAUDE.md
```
Expected: `2` and `2`. A third copy of either marker reds `repo:affected-smoke`.

- [ ] **Step 6: Lint and commit**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py ruff check --config py/pyproject.toml ci/actionlint/
actionlint -shellcheck= .github/workflows/release.yml; echo "rc=$?"
```
Expected: `All checks passed!` and `rc=0` — Step 3's reword touches both files.

```bash
git add docs/ops/RUNBOOK-containers.md CLAUDE.md docs/superpowers/specs/2026-09-19-sma-658-container-release-design.md ci/actionlint/release_guard.py .github/workflows/release.yml
git commit -m "$(cat <<'EOF'
docs(docs): document the service image release path

Co-Authored-By: <your model name> <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: CI evidence (the controller does this, not an implementer)

After the pull request is open, collect and record:

- [ ] `images.yml` on this PR: the `M8 …` line now reports `cargo>0` for both services (AC 7,
      Task 1). If it still reports `cargo=0`, Task 1 did not take effect in the real build.
- [ ] `repo:actionlint` passes in CI, including check 10 (the guard against the real `release.yml`)
      and check 11 (`--assert` against the real tree, which now reads both changelogs).
- [ ] `repo:affected-smoke` passes with the re-pinned `RELEASE_PLAN_SH_CALL_SITES`.
- [ ] The whole `moon ci` graph passes. SMA-663 records that the napi-stage race can red this
      branch repeatedly; a red there with a `.napi-stage-` path in the error is that known race.
- [ ] **After the merge, on the live release run:** the run pauses for approval on
      `release-approval`, and one approval releases every pending chain; the
      published index digest is identical on both registries; `cosign verify` and
      `gh attestation verify` pass; `paigasus-iam-v0.1.0` and `paigasus-gateway-v0.1.0` point at
      the image's revision label. Record the digests in the Linear issue.

---

## After PR 2 merges (not part of this plan)

- Set both GHCR packages to public and link them to the repository.
- SMA-665: make the SBOM list the OS packages.
- Watch the Docker Hub token's expiry date.

## Self-review record

- **Spec coverage.** § 3.1 V-a → Tasks 2, 3. § 3.3 the bump → Task 2. § 3.4 the comments → Task 2.
  § 4.1 the gates → Tasks 4, 7. § 4.2 the build job → Task 7. § 4.3 the publish job, now split into
  the push/adopt/copy/sign step, the three `attest-*` steps and the Verify step (B6) → Task 7.
  § 4.4 the tag job → Task 7. § 4.5 and AC 7 → Task 1. § 5 recovery (D10) → Task 7 Step 3. § 6.1 and
  § 6.2 → Tasks 3, 4. § 7.1 V8, V9b, V9c and the markers → Tasks 5, 6. § 7.2 V10, V13, V11 → Task 6.
  § 7.3 the fixtures and the mutation pass → Tasks 5, 6, 8. § 7.4 no registry change → the File
  Structure note. § 11 AC 1 to AC 9 → Tasks 7, 8, 10. § 13 ADR-0011 → Task 9. § 14 the documents →
  Task 9.
- **Gaps found in the spec, and what this plan does.** § 6.1 omits the version outputs (P11). § 4.3
  omits that a `needs: plan` job must carry a gate literal (P12). § 4.2's "the per-platform digest
  is a job output" cannot work as written for a matrix job: both matrix legs write the same output
  name, and the last leg wins. Task 7 therefore reads each per-platform digest again in the publish
  job, from the archive, which is the only source that cannot be crossed. The build job declares no
  job outputs at all. State this in the implementer's report, so the spec can be corrected.
- **What cannot be verified before the first live release.** The Docker Hub login, the copy to
  Docker Hub and its digest equality, the cosign signature on Docker Hub, `gh attestation verify`
  against a `release.yml` identity, the App token's tag API call, and each approval's pause. The
  GHCR half of the sequence is proven by `images-rehearsal.yml` on `main`.
- **Corrections from the SMA-658 PR 2 pre-flight scan (2026-09-20), applied throughout this plan.**
  The two chains' shared step lists no longer live under a top-level `x-image-chain:` key — MEASURED,
  actionlint 1.7.12 rejects that key — so each anchor sits on the `iam` job that needs it first
  (B2). `gh attestation verify` moved out of the publish step into its own, later "Verify the
  signature and attestations" step, run after the `attest-*` action steps actually create something
  to verify (B6). `EXPECTED_RELEASE_SECRETS` gains `DOCKERHUB_TOKEN` in Task 7, alongside the job
  that references it, not in Task 6 (B5). V11 now also forbids `id-token: write` outside
  `OIDC_PUBLISH_JOBS` (S1). `docker logout` stays after the `crane tag` loop rather than moving
  next to `cosign sign`, because `crane tag`'s Hub write needs the same credential (S15, P15).
