# SMA-658 PR 1 — Image release tooling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **First action of every implementer:** a subagent starts pinned to the MAIN checkout. Before
> anything else, run EnterWorktree with `path` =
> `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-658-container-release`, then
> confirm `git branch --show-current` prints `feature/sma-658-container-release`. Stop if it does not.
> **Add commits, never amend.** Run every step in the foreground.

**Goal:** Add the tools, the scripts and the two workflows that the image release path needs, and prove them in CI, without publishing any release image.

**Architecture:** Three CLIs (crane, cosign, syft) are pinned through vendored proto plugins. The release decisions (which digest is final, when the floating tags move, what an OCI archive holds) live in one stdlib-only Python script with a self-test. `ci/images/run.sh` gets an OCI-archive build, an identity check after `docker load`, a per-service smoke, and a `rehearse` mode that runs the publish sequence against two local registries. `images.yml` runs that path on every relevant PR (amd64) and on `main`/dispatch (amd64 + arm64). A dispatch-only `images-rehearsal.yml` runs cosign and the GitHub attestations for real against a scratch GHCR package.

**Tech Stack:** bash (must run under macOS `/bin/bash` 3.2 and Linux bash 5), Python 3.12 stdlib, Docker buildx, crane 0.22.1, cosign 3.1.3, syft 1.52.0, proto 0.61.1, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-19-sma-658-container-release-design.md` (approved at GATE 1). This plan covers **PR 1 only** (spec § 10). PR 2 gets its own plan after the rehearsal has run on `main`.

## Global Constraints

- PR 1 publishes **no** release image. Both services stay at `version = "0.0.0"`. `.github/workflows/release.yml` does **not** change.
- Every new source file opens with the SPDX header: `# SPDX-License-Identifier: Apache-2.0`.
- Every action is pinned by a 40-character commit SHA with a `# vX.Y.Z` comment.
- Tool versions: crane (go-containerregistry) `0.22.1`, cosign `3.1.3`, syft `1.52.0`.
- Pinned action SHAs (read from GitHub on 2026-09-19):
  - `actions/attest-build-provenance@4d101475d8b20a2381f78447822ac1eab6504dd8  # v4.2.2`
  - `actions/attest-sbom@c604332985a26aa8cf1bdc465b92731239ec6b9e  # v4.1.0`
  - `docker/login-action@dbcb813823bdd20940b903addbd779551569679f  # v4.6.0`
  - Already in the repo: `actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1`, `docker/setup-buildx-action@37fe631027851001ddb9b187196cc803df7f5f0e  # v4.3.0`, `moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0`, `actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1`, `actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1`.
- Scratch GHCR package for the rehearsal: `ghcr.io/smk1085/paigasus-rehearsal`.
- cosign identity for the rehearsal: `https://github.com/SMK1085/paigasus-core/.github/workflows/images-rehearsal.yml@refs/heads/main`, issuer `https://token.actions.githubusercontent.com`.
- GHCR login uses `${{ github.token }}`, **never** `secrets.GITHUB_TOKEN`.
- `images.yml` keeps `permissions: contents: read` and reads no secret (it has a `pull_request` trigger; `repo:workflow-credentials`).
- `images-rehearsal.yml` has **only** `workflow_dispatch`, and it fails on any ref but `refs/heads/main`.
- Shell rules (`CLAUDE.md`): no pipe into an early-exit reader (`grep -q`, `grep -m`, `head`, `awk … exit`, a `sed` script with `q`); use `sed -n 1p` or process substitution. No `mapfile`, no `declare -A`, no `${var,,}` in `ci/images/run.sh` (it must run under `/bin/bash` 3.2). No new `<<<` here-string.
- Every captured `proto` or shim output: `export PROTO_REPORTER=text` at the top of the script (`CLAUDE.md`, SMA-609).
- Commit messages: conventional, with the scope `ci`, `repo` or `docs`, ending with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`. No `#NNN` line and no `token: value` line in a commit body (commitlint `footer-leading-blank`).
- Prose in docs and comments: ASD-STE100 Simplified Technical English.

## Local environment and its limits

- PATH: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` before any proto, uv or moon call.
- Docker Desktop 29.8 with the **containerd image store**, arm64. A cold release build of a service takes longer than 15 minutes, so local checks use a tiny fixture image. CI is the verification for the real images and for the runner-specific values (M3, M5, M8).
- Run `ci/images/run.sh` locally with `/bin/bash` (3.2). Homebrew bash 5.3 can deadlock on this Mac (512-byte pipes).
- `repo:actionlint`'s full gate exits rc 2 on this Mac (512-byte pipe preflight). Locally, lint the two workflows with `actionlint -shellcheck= <file>` (no shellcheck, which is the part that hangs) and rely on CI for the shellcheck pass. `ci/actionlint/run.sh --self-test` under `/bin/bash` shows two known false `cargo-lock-step` rows; any OTHER failing row is real.
- Lint new Python with `uv run --locked --project py ruff check --config py/pyproject.toml ci/images/`.
- Shellcheck the script by file argument (no pipe): `uv run --locked --project py shellcheck ci/images/run.sh`.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `.proto/plugins/crane.toml` | Create | Vendored proto plugin: crane from go-containerregistry release tarballs. |
| `.proto/plugins/cosign.toml` | Create | Vendored proto plugin: cosign bare release binaries. |
| `.proto/plugins/syft.toml` | Create | Vendored proto plugin: syft release tarballs. |
| `.prototools` | Modify | Pin the three versions and register the three plugins. |
| `ci/images/release_decision.py` | Create | Pure decisions: OCI archive digests, D10 adoption, floating tags, SBOM summary. `--self-test`. |
| `ci/images/run.sh` | Modify | `build-oci`, `load-oci` (identity check, M3 line), per-service `smoke`, `rehearse`. |
| `.github/workflows/images.yml` | Modify | Run the release build path: amd64 on PR, amd64 + arm64 on `main` and dispatch. |
| `.github/workflows/images-rehearsal.yml` | Create | Dispatch-only: push, attest, sign and verify a scratch image on GHCR. |
| `docs/ops/RUNBOOK-containers.md` | Modify | Tool pins, the new `run.sh` commands, the rehearsal how-to. |
| `docs/superpowers/specs/2026-09-19-sma-658-container-release-design.md` | Modify | Record the PR 1 decisions (§ "Decisions this plan made") and, after CI, M3/M5/M8. |

**Not changed, and why (obligations checked):**

- No new `repo:*` Moon task, so the `T=(…)` array, the `CLAUDE.md` marker command and the `ci_targets.py` registries do not change. `ci/images/` is not a Moon task (`ci/images/run.sh:6-9`).
- `.prototools` is an input of several tasks (the three `repo:release-parity*` tasks, `contracts:generate`, the FFI wrapper tasks and others). The edit **selects** them in CI; it breaks no pin. The only `.prototools` line pin is `actionlint = "1.7.12"` (`ci/affected-graph/ci_targets.py:782`), which does not change.
- The new plugin files are no Moon task's input (only `osv-scanner.toml`, `release-plz.toml` and `promtool.toml` are, in `moon.yml`), so `repo:input-liveness` needs nothing.
- `ci/images/release_decision.py` is linted by `repo:ruff-ci` (it covers `ci/**/*.py`). It must pass `ruff check` with the `py/pyproject.toml` rule set.
- `images.yml` stays in `workflow_credentials.py`'s `EXPECTED_PR_SUBJECTS` (`:287`) and reads no secret. `images-rehearsal.yml` has no `pull_request` trigger, so it is not a subject.
- `release_guard.py` checks only `release.yml` and the workflows it calls. The rehearsal workflow is neither.
- `repo:actionlint` applies to both workflows: actionlint and shellcheck on every `run:` block (check 1), `paths:` globs must match the tree and filters must be block sequences (checks 5, 6), no early-exit reader (check 13, which also scans `ci/images/run.sh`).
- `ci.yml`'s bare `proto install` (`ci.yml:76`) now also downloads crane, cosign and syft on every CI run. This plan accepts that cost (three release downloads). Task 9 records the added time from the PR's own CI run.

---

### Task 1: Pin crane, cosign and syft through proto

**Files:**
- Create: `.proto/plugins/crane.toml`, `.proto/plugins/cosign.toml`, `.proto/plugins/syft.toml`
- Modify: `.prototools`

**Interfaces:**
- Produces: the commands `crane`, `cosign`, `syft` through `~/.proto/shims` after `proto install <tool>`. Later tasks call `proto install crane`, `proto install cosign`, `proto install syft` in workflows.

- [ ] **Step 1: Confirm the tools are absent (the failing check)**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
proto install crane; echo "rc=$?"
```
Expected: a non-zero rc; proto does not know the tool `crane`.

- [ ] **Step 2: Write `.proto/plugins/crane.toml`**

```toml
# SPDX-License-Identifier: Apache-2.0
#
# Vendored proto TOML plugin for crane (SMA-658).
#
# Resolves official google/go-containerregistry GitHub release tarballs. Same vendoring rationale
# as cargo-deny/promtool: a static schema over official release assets. There is no proto plugin
# for crane upstream (`proto plugin search crane`, proto 0.61.1: no results).
#
# The asset names use a CAPITALISED OS (Linux/Darwin/Windows) and x86_64/arm64 for the arch.
# proto's default {arch} tokens are x86_64/aarch64, so only aarch64 needs a remap, and it is the
# same on every OS, so one global [install.arch] covers all three platforms.
#
# Each tarball holds crane, gcrane and krane at its root, so exe-path names the crane binary.
# exe-path is kept explicit here (unlike actionlint.toml/maturin.toml), although the binary sits
# at the archive root.
# All platforms share one checksums.txt. Tags are "v"-prefixed; asset names carry no version.

name = "crane"
type = "cli"

[platform.linux]
download-file = "go-containerregistry_Linux_{arch}.tar.gz"
checksum-file = "checksums.txt"
exe-path = "crane"

[platform.macos]
download-file = "go-containerregistry_Darwin_{arch}.tar.gz"
checksum-file = "checksums.txt"
exe-path = "crane"

[platform.windows]
download-file = "go-containerregistry_Windows_{arch}.tar.gz"
checksum-file = "checksums.txt"
exe-path = "crane.exe"

[install]
download-url = "https://github.com/google/go-containerregistry/releases/download/v{version}/{download_file}"
checksum-url = "https://github.com/google/go-containerregistry/releases/download/v{version}/{checksum_file}"

[install.arch]
aarch64 = "arm64"

[resolve]
git-url = "https://github.com/google/go-containerregistry"
```

- [ ] **Step 3: Write `.proto/plugins/cosign.toml`**

```toml
# SPDX-License-Identifier: Apache-2.0
#
# Vendored proto TOML plugin for cosign (SMA-658).
#
# Resolves official sigstore/cosign GitHub release binaries. No proto plugin exists upstream
# (`proto plugin search cosign`, proto 0.61.1: no results).
#
# Assets are BARE binaries (cosign-{os}-{arch}), like osv-scanner, so no exe-path is needed.
# They use Go's GOARCH names (amd64/arm64) on every OS, so one global [install.arch] remaps both.
# Windows ships amd64 only. All platforms share one cosign_checksums.txt.

name = "cosign"
type = "cli"

[platform.linux]
download-file = "cosign-linux-{arch}"
checksum-file = "cosign_checksums.txt"

[platform.macos]
download-file = "cosign-darwin-{arch}"
checksum-file = "cosign_checksums.txt"

[platform.windows]
download-file = "cosign-windows-{arch}.exe"
checksum-file = "cosign_checksums.txt"

[install]
download-url = "https://github.com/sigstore/cosign/releases/download/v{version}/{download_file}"
checksum-url = "https://github.com/sigstore/cosign/releases/download/v{version}/{checksum_file}"

[install.arch]
x86_64 = "amd64"
aarch64 = "arm64"

[resolve]
git-url = "https://github.com/sigstore/cosign"
```

- [ ] **Step 4: Write `.proto/plugins/syft.toml`**

```toml
# SPDX-License-Identifier: Apache-2.0
#
# Vendored proto TOML plugin for syft (SMA-658).
#
# Resolves official anchore/syft GitHub release archives. No proto plugin exists upstream
# (`proto plugin search syft`, proto 0.61.1: no results).
#
# Asset names embed the bare version (syft_1.52.0_linux_amd64.tar.gz) and use Go's GOARCH names
# on every OS, so one global [install.arch] remaps both arches. The binary sits at the archive
# root; exe-path is kept explicit here (unlike actionlint.toml/maturin.toml). Windows ships a
# .zip, not a .tar.gz. One syft_{version}_checksums.txt covers all assets.

name = "syft"
type = "cli"

[platform.linux]
download-file = "syft_{version}_linux_{arch}.tar.gz"
checksum-file = "syft_{version}_checksums.txt"
exe-path = "syft"

[platform.macos]
download-file = "syft_{version}_darwin_{arch}.tar.gz"
checksum-file = "syft_{version}_checksums.txt"
exe-path = "syft"

[platform.windows]
download-file = "syft_{version}_windows_{arch}.zip"
checksum-file = "syft_{version}_checksums.txt"
exe-path = "syft.exe"

[install]
download-url = "https://github.com/anchore/syft/releases/download/v{version}/{download_file}"
checksum-url = "https://github.com/anchore/syft/releases/download/v{version}/{checksum_file}"

[install.arch]
x86_64 = "amd64"
aarch64 = "arm64"

[resolve]
git-url = "https://github.com/anchore/syft"
```

- [ ] **Step 5: Pin them in `.prototools`**

Add three version lines to the first block, in alphabetical order. Put these two directly after `cargo-nextest = "0.9.136"`:

```toml
cosign = "3.1.3"
crane = "0.22.1"
```

Put this one directly after `release-plz = "0.3.158"`:

```toml
syft = "1.52.0"
```

Add three lines to `[plugins]`, in alphabetical order:

```toml
cosign = "file://./.proto/plugins/cosign.toml"
crane = "file://./.proto/plugins/crane.toml"
syft = "file://./.proto/plugins/syft.toml"
```

- [ ] **Step 6: Install and run each tool**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
proto install crane && crane version
proto install cosign && cosign version
proto install syft && syft version
```
Expected: rc 0; `crane version` prints `0.22.1`, `cosign version` shows `v3.1.3`, `syft version` shows `1.52.0`. A checksum mismatch or a 404 means an asset name in the plugin is wrong: compare it with `gh release view --repo <owner/repo> v<version> --json assets --jq '.assets[].name'`.

- [ ] **Step 7: Confirm the checksum files list the exact asset names**

Run:
```bash
gh release download v3.1.3 --repo sigstore/cosign -p cosign_checksums.txt -O - | grep -E ' cosign-(linux|darwin)-(amd64|arm64)$'
gh release download v1.52.0 --repo anchore/syft -p syft_1.52.0_checksums.txt -O - | grep -E 'syft_1.52.0_(linux|darwin)_(amd64|arm64)\.tar\.gz$'
gh release download v0.22.1 --repo google/go-containerregistry -p checksums.txt -O - | grep -E 'go-containerregistry_(Linux|Darwin)_(x86_64|arm64)\.tar\.gz$'
```
Expected: four lines each. (The Linux lines are the ones CI installs; this is the only local proof of them.)

- [ ] **Step 8: Commit**

```bash
git add .proto/plugins/crane.toml .proto/plugins/cosign.toml .proto/plugins/syft.toml .prototools
git commit -m "chore(repo): pin crane, cosign and syft through proto (SMA-658)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: The release decision script

**Files:**
- Create: `ci/images/release_decision.py`

**Interfaces:**
- Produces (CLI, `key=value` lines on stdout, exit 0 decided / 2 usage / 3 conflict or failed self-test):
  - `release_decision.py oci-digests ARCHIVE` → `manifest=sha256:…`, `config=sha256:…`, `platform=linux/<arch>`
  - `release_decision.py adopt --new-digest D --ghcr D|none --dockerhub D|none --git-tag present|absent` → `action=already-released|push-new|adopt`, `digest=…`, `copy_to=none|ghcr|dockerhub`; on a conflict prints `action=conflict` and exits 3
  - `release_decision.py floating --service S --version X.Y.Z --tags-file FILE` → `move=true|false`, `minor_tag=X.Y`, `major_tag=` (empty while X is 0)
  - `release_decision.py sbom-summary SPDX_JSON` → `packages=N`, `libc6=true|false`, `cargo=N`
  - `release_decision.py --self-test`
- Invocation everywhere: `uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py …`

- [ ] **Step 1: Write the script with the self-test table and stub decision functions**

Write `ci/images/release_decision.py` with the full content below, EXCEPT that the bodies of `adopt`, `floating`, `oci_digests_from` and `sbom_summary` are `raise NotImplementedError` for now:

```python
#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""SMA-658: the decisions behind a service image release, kept out of shell and YAML so that a
self-test can prove them.

Subcommands. Each one prints `key=value` lines on stdout, in a fixed order:

  oci-digests ARCHIVE
      manifest=, config=, platform= of a single-image buildx OCI archive.
  adopt --new-digest D --ghcr D|none --dockerhub D|none --git-tag present|absent
      action=already-released | push-new | adopt, then digest= and copy_to=.
      The FIRST digest published under :<version> is final (spec D10). A new build never
      overwrites it; it adopts it.
  floating --service S --version X.Y.Z --tags-file FILE
      move=true|false, minor_tag=, major_tag= (empty while the major version is 0).
      FILE holds bare tag names or `git ls-remote --tags` lines.
  sbom-summary SPDX_JSON
      packages=, libc6=true|false, cargo= (a measurement for spec M8, not an assertion).

Exit codes: 0 decided | 2 usage or unreadable input | 3 a conflict, or a failed self-test row.
It never exits 1: `uv` exits 1 on its own failures, so 1 would be ambiguous.

Standard library only, so it runs with `uv run --no-project --python '>=3.12' python3`.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import re
import sys
import tarfile
from pathlib import Path
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from collections.abc import Callable

DIGEST_RE = re.compile(r"sha256:[0-9a-f]{64}")
VERSION_RE = re.compile(r"(\d+)\.(\d+)\.(\d+)")
SERVICE_RE = re.compile(r"[a-z][a-z0-9-]*")
NONE = "none"
IMAGE_MANIFEST_TYPES = frozenset(
    {
        "application/vnd.oci.image.manifest.v1+json",
        "application/vnd.docker.distribution.manifest.v2+json",
    }
)

Version = tuple[int, int, int]


class UsageError(Exception):
    """Bad arguments or an unreadable input: exit 2."""


class ConflictError(Exception):
    """The two registries hold different digests under one version: exit 3."""


def parse_version(text: str) -> Version:
    match = VERSION_RE.fullmatch(text)
    if match is None:
        raise UsageError(f"not a MAJOR.MINOR.PATCH version: {text!r}")
    return (int(match[1]), int(match[2]), int(match[3]))


def require_digest(text: str) -> str:
    if DIGEST_RE.fullmatch(text) is None:
        raise UsageError(f"not a sha256 digest: {text!r}")
    return text


def optional_digest(text: str) -> str | None:
    return None if text == NONE else require_digest(text)


def adopt(new_digest: str, ghcr: str | None, dockerhub: str | None, *, git_tag_present: bool) -> dict[str, str]:
    raise NotImplementedError


def tag_names(lines: list[str]) -> list[str]:
    """Accept bare tag names and `git ls-remote --tags` lines (`<sha>\\trefs/tags/<name>`)."""
    names: list[str] = []
    for line in lines:
        fields = line.split()
        if fields:
            names.append(fields[-1].removeprefix("refs/tags/"))
    return names


def floating(service: str, version: str, lines: list[str]) -> dict[str, str]:
    raise NotImplementedError


def _member(tar: tarfile.TarFile, name: str) -> bytes:
    try:
        handle = tar.extractfile(name)
    except KeyError as exc:
        raise UsageError(f"the archive has no {name}") from exc
    if handle is None:
        raise UsageError(f"{name} in the archive is not a regular file")
    return handle.read()


def _blob(tar: tarfile.TarFile, digest: str) -> bytes:
    data = _member(tar, "blobs/sha256/" + digest.removeprefix("sha256:"))
    if "sha256:" + hashlib.sha256(data).hexdigest() != digest:
        raise UsageError(f"blob {digest} does not match its own digest")
    return data


def oci_digests_from(tar: tarfile.TarFile) -> dict[str, str]:
    raise NotImplementedError


def oci_digests(path: Path) -> dict[str, str]:
    try:
        with tarfile.open(path) as tar:
            return oci_digests_from(tar)
    except (OSError, tarfile.TarError, json.JSONDecodeError) as exc:
        raise UsageError(f"cannot read {path}: {exc}") from exc


def sbom_summary(doc: dict[str, Any]) -> dict[str, str]:
    raise NotImplementedError


# --- self-test -------------------------------------------------------------------------------

D1 = "sha256:" + "1" * 64
D2 = "sha256:" + "2" * 64
D3 = "sha256:" + "3" * 64


def _fixture_archive(
    *, media_type: str = "application/vnd.oci.image.manifest.v1+json", manifests: int = 1
) -> tuple[tarfile.TarFile, str, str]:
    """An in-memory OCI archive shaped like a buildx single-image export."""
    config = json.dumps({"os": "linux", "architecture": "arm64"}).encode()
    config_digest = "sha256:" + hashlib.sha256(config).hexdigest()
    manifest = json.dumps({"config": {"digest": config_digest}}).encode()
    manifest_digest = "sha256:" + hashlib.sha256(manifest).hexdigest()
    index = json.dumps({"manifests": [{"mediaType": media_type, "digest": manifest_digest}] * manifests}).encode()
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        for name, data in (
            ("index.json", index),
            ("blobs/sha256/" + manifest_digest.removeprefix("sha256:"), manifest),
            ("blobs/sha256/" + config_digest.removeprefix("sha256:"), config),
        ):
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    buf.seek(0)
    return tarfile.open(fileobj=buf, mode="r"), manifest_digest, config_digest


def _outcome(fn: Callable[[], object]) -> object:
    try:
        return fn()
    except UsageError:
        return "UsageError"
    except ConflictError:
        return "ConflictError"


def self_test() -> int:
    tar, manifest, config = _fixture_archive()
    gw = "gateway"
    rows: list[tuple[str, Callable[[], object], object]] = [
        # adopt (spec D10, § 4.3 step 3)
        ("adopt: the git tag exists", lambda: adopt(D1, D2, D2, git_tag_present=True)["action"], "already-released"),
        ("adopt: nothing published", lambda: adopt(D1, None, None, git_tag_present=False), {"action": "push-new", "digest": D1, "copy_to": "none"}),
        ("adopt: both registries agree", lambda: adopt(D1, D2, D2, git_tag_present=False), {"action": "adopt", "digest": D2, "copy_to": "none"}),
        ("adopt: only GHCR has it", lambda: adopt(D1, D2, None, git_tag_present=False), {"action": "adopt", "digest": D2, "copy_to": "dockerhub"}),
        ("adopt: only Docker Hub has it", lambda: adopt(D1, None, D3, git_tag_present=False), {"action": "adopt", "digest": D3, "copy_to": "ghcr"}),
        ("adopt: the registries disagree", lambda: adopt(D1, D2, D3, git_tag_present=False), "ConflictError"),
        ("digest: 'none' where a digest is required", lambda: require_digest(NONE), "UsageError"),
        ("digest: a short digest", lambda: require_digest("sha256:abc"), "UsageError"),
        ("digest: 'none' is absent where allowed", lambda: optional_digest(NONE), None),
        # floating (spec § 4.3 step 7, § 7.3)
        ("floating: no tag yet", lambda: floating(gw, "0.1.0", [])["move"], "true"),
        ("floating: an equal version moves", lambda: floating(gw, "0.2.0", ["paigasus-gateway-v0.2.0"])["move"], "true"),
        ("floating: an older version holds", lambda: floating(gw, "0.1.1", ["paigasus-gateway-v0.2.0"])["move"], "false"),
        ("floating: 0.10.0 beats 0.9.0 (numeric order)", lambda: floating(gw, "0.10.0", ["paigasus-gateway-v0.9.0"])["move"], "true"),
        ("floating: 0.2.0 holds against 0.10.0", lambda: floating(gw, "0.2.0", ["paigasus-gateway-v0.10.0"])["move"], "false"),
        ("floating: other services' tags are ignored", lambda: floating(gw, "0.1.0", ["paigasus-iam-v9.0.0"])["move"], "true"),
        (
            "floating: ls-remote lines and peeled refs",
            lambda: floating(gw, "0.1.0", ["abc\trefs/tags/paigasus-gateway-v0.3.0", "abc\trefs/tags/paigasus-gateway-v0.3.0^{}"])["move"],
            "false",
        ),
        ("floating: no :major tag while 0.x", lambda: floating(gw, "0.4.2", []), {"move": "true", "minor_tag": "0.4", "major_tag": ""}),
        ("floating: a :major tag from 1.0", lambda: floating(gw, "1.2.3", [])["major_tag"], "1"),
        ("floating: a malformed version", lambda: floating(gw, "v1.2", []), "UsageError"),
        ("floating: a malformed service", lambda: floating("Gate way", "0.1.0", []), "UsageError"),
        # oci-digests (spec § 4.2 identity check)
        ("oci: a single-image archive", lambda: oci_digests_from(tar), {"manifest": manifest, "config": config, "platform": "linux/arm64"}),
        ("oci: an index is refused", lambda: oci_digests_from(_fixture_archive(media_type="application/vnd.oci.image.index.v1+json")[0]), "UsageError"),
        ("oci: two manifests are refused", lambda: oci_digests_from(_fixture_archive(manifests=2)[0]), "UsageError"),
        # sbom-summary (spec M8)
        (
            "sbom: counts libc6 and cargo packages",
            lambda: sbom_summary({"packages": [{"name": "libc6"}, {"name": "serde", "externalRefs": [{"referenceLocator": "pkg:cargo/serde@1.0.228"}]}]}),
            {"packages": "2", "libc6": "true", "cargo": "1"},
        ),
        ("sbom: an empty document", lambda: sbom_summary({}), {"packages": "0", "libc6": "false", "cargo": "0"}),
    ]
    failed = 0
    for label, fn, want in rows:
        got = _outcome(fn)
        if got != want:
            failed += 1
            print(f"FAIL {label}: expected {want!r}, got {got!r}", file=sys.stderr)
    if failed:
        print(f"release_decision self-test: {failed} of {len(rows)} rows failed", file=sys.stderr)
        return 3
    print(f"release_decision self-test: {len(rows)} rows OK")
    return 0


# --- CLI ---------------------------------------------------------------------------------------


def _emit(result: dict[str, str]) -> None:
    for key, value in result.items():
        print(f"{key}={value}")


def _read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        raise UsageError(f"cannot read {path}: {exc}") from exc


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="release_decision.py", description="SMA-658 image release decisions")
    parser.add_argument("--self-test", action="store_true", help="run the fixture table and exit")
    sub = parser.add_subparsers(dest="command")
    p_oci = sub.add_parser("oci-digests")
    p_oci.add_argument("archive", type=Path)
    p_adopt = sub.add_parser("adopt")
    p_adopt.add_argument("--new-digest", required=True)
    p_adopt.add_argument("--ghcr", required=True)
    p_adopt.add_argument("--dockerhub", required=True)
    p_adopt.add_argument("--git-tag", required=True, choices=("present", "absent"))
    p_float = sub.add_parser("floating")
    p_float.add_argument("--service", required=True)
    p_float.add_argument("--version", required=True)
    p_float.add_argument("--tags-file", required=True, type=Path)
    p_sbom = sub.add_parser("sbom-summary")
    p_sbom.add_argument("sbom", type=Path)
    try:
        args = parser.parse_args(argv)
    except SystemExit as exc:  # argparse exits 2 on a usage error and 0 on --help
        return int(exc.code or 0)

    if args.self_test:
        return self_test()
    try:
        if args.command == "oci-digests":
            _emit(oci_digests(args.archive))
        elif args.command == "adopt":
            _emit(
                adopt(
                    require_digest(args.new_digest),
                    optional_digest(args.ghcr),
                    optional_digest(args.dockerhub),
                    git_tag_present=args.git_tag == "present",
                )
            )
        elif args.command == "floating":
            _emit(floating(args.service, args.version, _read_text(args.tags_file).splitlines()))
        elif args.command == "sbom-summary":
            try:
                doc = json.loads(_read_text(args.sbom))
            except json.JSONDecodeError as exc:
                raise UsageError(f"{args.sbom} is not JSON: {exc}") from exc
            if not isinstance(doc, dict):
                raise UsageError(f"{args.sbom} is not an SPDX JSON object")
            _emit(sbom_summary(doc))
        else:
            parser.print_usage(sys.stderr)
            return 2
    except UsageError as exc:
        print(f"release_decision: {exc}", file=sys.stderr)
        return 2
    except ConflictError as exc:
        print("action=conflict")
        print(f"release_decision: {exc}", file=sys.stderr)
        return 3
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
```

Note: `_outcome` catches only `UsageError` and `ConflictError`, so a stub's `NotImplementedError` escapes and crashes the self-test in Step 2. That is the expected red.

- [ ] **Step 2: Run the self-test and see it fail**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py --self-test; echo "rc=$?"
```
Expected: a `NotImplementedError` traceback from the first `adopt` row, rc 1.

- [ ] **Step 3: Implement the four functions**

Replace the four stubs with:

```python
def adopt(new_digest: str, ghcr: str | None, dockerhub: str | None, *, git_tag_present: bool) -> dict[str, str]:
    """Spec D10: the first digest published under :<version> is final."""
    if git_tag_present:
        return {"action": "already-released", "digest": "", "copy_to": "none"}
    if ghcr is None and dockerhub is None:
        return {"action": "push-new", "digest": new_digest, "copy_to": "none"}
    if ghcr is not None and dockerhub is not None:
        if ghcr != dockerhub:
            raise ConflictError(f":<version> points at two digests: ghcr={ghcr} dockerhub={dockerhub}")
        return {"action": "adopt", "digest": ghcr, "copy_to": "none"}
    if ghcr is not None:
        return {"action": "adopt", "digest": ghcr, "copy_to": "dockerhub"}
    assert dockerhub is not None
    return {"action": "adopt", "digest": dockerhub, "copy_to": "ghcr"}
```

```python
def floating(service: str, version: str, lines: list[str]) -> dict[str, str]:
    """Spec § 4.3 step 7: move :<major>.<minor> and :latest only forward, compared as numbers."""
    if SERVICE_RE.fullmatch(service) is None:
        raise UsageError(f"not a service name: {service!r}")
    mine = parse_version(version)
    pattern = re.compile(rf"paigasus-{re.escape(service)}-v(\d+\.\d+\.\d+)")
    existing = [parse_version(m[1]) for name in tag_names(lines) if (m := pattern.fullmatch(name))]
    move = not existing or mine >= max(existing)
    return {
        "move": "true" if move else "false",
        "minor_tag": f"{mine[0]}.{mine[1]}",
        "major_tag": "" if mine[0] == 0 else str(mine[0]),
    }
```

```python
def oci_digests_from(tar: tarfile.TarFile) -> dict[str, str]:
    """The manifest and config digests of a single-image buildx OCI export."""
    index = json.loads(_member(tar, "index.json"))
    manifests = index.get("manifests") or []
    if len(manifests) != 1:
        raise UsageError(f"index.json lists {len(manifests)} manifests; expected one image (build with --provenance=false --sbom=false)")
    entry = manifests[0]
    if entry.get("mediaType") not in IMAGE_MANIFEST_TYPES:
        raise UsageError(f"index.json names {entry.get('mediaType')!r}, not one image manifest")
    manifest_digest = require_digest(str(entry.get("digest", "")))
    manifest = json.loads(_blob(tar, manifest_digest))
    config_digest = require_digest(str((manifest.get("config") or {}).get("digest", "")))
    config = json.loads(_blob(tar, config_digest))
    platform = f"{config.get('os', 'unknown')}/{config.get('architecture', 'unknown')}"
    return {"manifest": manifest_digest, "config": config_digest, "platform": platform}
```

```python
def sbom_summary(doc: dict[str, Any]) -> dict[str, str]:
    """Spec M8: does the SBOM see the Ubuntu packages and the Rust crates at all?"""
    packages = doc.get("packages") or []
    names = {str(p.get("name", "")) for p in packages}
    cargo = sum(
        1 for p in packages if any(str(ref.get("referenceLocator", "")).startswith("pkg:cargo/") for ref in p.get("externalRefs") or [])
    )
    return {"packages": str(len(packages)), "libc6": "true" if "libc6" in names else "false", "cargo": str(cargo)}
```

- [ ] **Step 4: Run the self-test and the CLI**

Run:
```bash
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py --self-test; echo "rc=$?"
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py adopt --new-digest "sha256:$(printf '1%.0s' $(seq 64))" --ghcr none --dockerhub none --git-tag absent
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py adopt --new-digest none --ghcr none --dockerhub none --git-tag absent; echo "rc=$?"
```
Expected: `release_decision self-test: 25 rows OK`, rc 0; then `action=push-new`, `digest=sha256:111…`, `copy_to=none`; then a `not a sha256 digest` message and rc 2.

- [ ] **Step 5: Prove the self-test bites (delete a feature, see it red)**

Temporarily change `move = not existing or mine >= max(existing)` to `move = True`. Run the self-test. Expected: rc 3 and at least three `FAIL floating:` rows. Revert the line with the Edit tool (not `git checkout`, which would also discard the uncommitted file). Run the self-test again: rc 0.

- [ ] **Step 6: Lint**

Run: `uv run --locked --project py ruff check --config py/pyproject.toml ci/images/release_decision.py`
Expected: `All checks passed!`. Fix any finding in place.

- [ ] **Step 7: Commit**

```bash
git add ci/images/release_decision.py
git commit -m "feat(ci): add the image release decision script with a self-test (SMA-658)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Build an OCI archive and check the loaded identity

**Files:**
- Modify: `ci/images/run.sh` (header usage block `:11-13`, near the top for `PROTO_REPORTER`, `build_one` `:133-184`, dispatch `:439-469`)

**Interfaces:**
- Consumes: `release_decision.py oci-digests` (Task 2).
- Produces:
  - `ci/images/run.sh build-oci <iam|gateway> <outdir>` → writes `<outdir>/paigasus-<svc>-<arch>.oci.tar` (arch = Docker server arch: `amd64`/`arm64`) and `chisel-manifest-<svc>-<arch>.txt` at the repo root. The image inside is named `paigasus-<svc>:dev` and carries the label `org.opencontainers.image.version=<Cargo version>`.
  - `ci/images/run.sh load-oci <archive> <image-name>` → loads the archive, prints one `M3 …` line, exits 1 when the loaded image is not the archive's image.
  - Shell helpers used by Tasks 4 and 5: `decide` (runs the Python script), `kv "<output>" <key>` (reads one value).

- [ ] **Step 1: Make a tiny local fixture archive (not committed)**

Run:
```bash
FIX="$(mktemp -d "${TMPDIR:-/tmp}/sma658-fixture.XXXXXX")"; echo "$FIX"
printf 'FROM busybox:1.36\nLABEL org.opencontainers.image.revision=%s\n' "$(git rev-parse HEAD)" > "$FIX/Dockerfile"
docker build --provenance=false --sbom=false \
  --output "type=oci,dest=$FIX/paigasus-gateway-$(docker version --format '{{.Server.Arch}}').oci.tar,name=paigasus-gateway:dev" "$FIX"
ls -l "$FIX"
```
Expected: one `paigasus-gateway-arm64.oci.tar` file. Keep `$FIX` for the next tasks.

- [ ] **Step 2: Run the new command before it exists (the failing check)**

Run: `/bin/bash ci/images/run.sh load-oci "$FIX"/paigasus-gateway-*.oci.tar paigasus-gateway:dev; echo "rc=$?"`
Expected: `unknown command: load-oci`, rc 1.

- [ ] **Step 3: Add `PROTO_REPORTER`, `decide` and `kv` near the top**

Insert after the `REVISION=…` line (`:19`):

```bash
# proto prints an NDJSON preamble on STDOUT inside an agent session, and that poisons every
# `$(...)` capture of a shimmed tool such as `uv` (CLAUDE.md, SMA-609). Exported once here so every
# capture below inherits it.
export PROTO_REPORTER=text

# The release decisions live in release_decision.py so a self-test can prove them (SMA-658).
# Standard library only: no project, no lock, any Python >= 3.12 that uv can find.
decide() {
  uv run --no-project --python '>=3.12' python3 "$HERE/release_decision.py" "$@"
}

# kv "<key=value lines>" <key> — the value of one key, or an empty string.
kv() {
  printf '%s\n' "$1" | sed -n "s/^$2=//p"
}
```

- [ ] **Step 4: Split the chisel-manifest extraction out of `build_one` and add `version_for`**

Replace the lines of `build_one` from `grep -oE 'Fetching pool/[^ ]+\.deb' "$build_log" | sort -u > "$ROOT/chisel-manifest-${service}.txt" || true` through the `fi` that closes the empty-manifest check, with a call `extract_chisel_manifest "$build_log" "$ROOT/chisel-manifest-${service}.txt"`, and add this function above `build_one`:

```bash
# Writes the chisel package list that a build log names into $2, and fails when it is empty
# (SMA-500 fix-round 1: an empty manifest answers nothing when someone asks which libc shipped).
extract_chisel_manifest() {
  local build_log="$1" out="$2"
  grep -oE 'Fetching pool/[^ ]+\.deb' "$build_log" | sort -u > "$out" || true
  if [ ! -s "$out" ]; then
    echo "::error::$(basename "$out") is empty; the package-fetch log format may have changed — update the grep pattern in ci/images/run.sh." >&2
    return 1
  fi
}

# The version line of a service crate's own Cargo.toml. Both services carry a literal version
# (not `version.workspace = true`), which is what the image's version label must equal.
version_for() {
  local crate="$1" v
  v="$(sed -n 's/^version = "\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)"$/\1/p' "$ROOT/rs/crates/services/${crate}/Cargo.toml" | sed -n 1p)"
  if [ -z "$v" ]; then
    echo "::error::no literal version line in rs/crates/services/${crate}/Cargo.toml" >&2
    return 1
  fi
  echo "$v"
}
```

- [ ] **Step 5: Add `build_oci` after `build_one`**

```bash
# SMA-658: the release build. The same image as build_one, exported as an OCI ARCHIVE instead of
# being loaded, so its bytes (and so its digest) are fixed before anything is pushed.
# --provenance=false --sbom=false: buildx would otherwise wrap the image in an index with its own
# attestation manifests; the release path attests through GitHub instead (spec D4).
# name=<crate>:dev: `docker load` of the archive then restores that name for load_oci and smoke.
build_oci() {
  local service="$1" outdir="$2" crate version arch archive build_log
  crate="$(crate_for "$service")"
  version="$(version_for "$crate")"
  arch="$(docker version --format '{{.Server.Arch}}')"
  mkdir -p "$outdir"
  archive="${outdir}/${crate}-${arch}.oci.tar"
  build_log="$(mktemp "${TMPDIR:-/tmp}/paigasus-build-${service}.XXXXXX")"
  trap 'rm -f "$build_log"' RETURN
  echo "== build-oci ${crate} ${version} (${arch}) =="
  docker build \
    --progress=plain \
    --no-cache-filter=rootfs \
    --provenance=false --sbom=false \
    --output "type=oci,dest=${archive},name=${crate}:dev" \
    -f "$ROOT/rs/Dockerfile" \
    --build-arg "BIN=${crate}" \
    --label "org.opencontainers.image.title=${crate}" \
    --label "org.opencontainers.image.description=Paigasus ${service} service" \
    --label "org.opencontainers.image.source=https://github.com/SMK1085/paigasus-core" \
    --label "org.opencontainers.image.revision=${REVISION}" \
    --label "org.opencontainers.image.version=${version}" \
    --label "org.opencontainers.image.licenses=Apache-2.0" \
    "$ROOT/rs" 2>&1 | tee "$build_log"
  extract_chisel_manifest "$build_log" "$ROOT/chisel-manifest-${service}-${arch}.txt"
  echo "  built ${archive}"
}
```

- [ ] **Step 6: Add `load_oci` after `build_oci`**

```bash
# SMA-658 spec § 4.2: the image the smoke suite tests must be the image in the archive. What
# `docker load` reports as the image ID depends on the daemon's image store (measured M3, local
# Docker 29.8): the containerd store gives the MANIFEST digest, the classic store gives the CONFIG
# digest. So the check accepts the one value that matches this daemon's store, and prints every
# value once per runner so CI records M3 for each runner label.
load_oci() {
  local archive="$1" name="$2" digests manifest config store driver loaded size expected
  digests="$(decide oci-digests "$archive")"
  manifest="$(kv "$digests" manifest)"
  config="$(kv "$digests" config)"
  docker load -i "$archive" >/dev/null
  loaded="$(docker image inspect --format '{{.Id}}' "$name")"
  size="$(docker image inspect --format '{{.Size}}' "$name")"
  driver="$(docker info --format '{{json .DriverStatus}}')"
  store="classic"
  case "$driver" in *io.containerd.snapshotter*) store="containerd" ;; esac
  expected="$config"
  [ "$store" = "containerd" ] && expected="$manifest"
  echo "M3 arch=$(docker version --format '{{.Server.Arch}}') docker=$(docker version --format '{{.Server.Version}}') store=${store} loaded_id=${loaded} manifest=${manifest} config=${config} size=${size}"
  if [ "$loaded" != "$expected" ]; then
    echo "::error::${name} loaded as ${loaded}, but the ${store} store should report ${expected}: the loaded image is not the archive's image." >&2
    return 1
  fi
  echo "  ${name} is the archive's image (${store} store)"
}
```

- [ ] **Step 7: Wire the two commands into the dispatch and the usage header**

In the header usage block (`:11-13`) add:

```bash
#        ci/images/run.sh build-oci <iam|gateway> <outdir>   # SMA-658: OCI archive, no --load
#        ci/images/run.sh load-oci <archive> <image-name>     # load + identity check (prints M3)
```

In the `case "$cmd"` dispatch add, before the `*)` arm:

```bash
  build-oci)
    if [ -z "$target" ] || [ -z "${3:-}" ]; then
      echo "usage: ci/images/run.sh build-oci <iam|gateway> <outdir>" >&2
      exit 1
    fi
    assert_pins
    build_oci "$target" "$3"
    ;;
  load-oci)
    if [ -z "$target" ] || [ -z "${3:-}" ]; then
      echo "usage: ci/images/run.sh load-oci <archive> <image-name>" >&2
      exit 1
    fi
    load_oci "$target" "$3"
    ;;
```

Note: for `load-oci`, `$target` is the archive path, not a service. The `services=("$target")` line above the dispatch does not matter for this arm.

- [ ] **Step 8: Run `load-oci` on the fixture**

Run: `/bin/bash ci/images/run.sh load-oci "$FIX"/paigasus-gateway-*.oci.tar paigasus-gateway:dev; echo "rc=$?"`
Expected: one `M3 arch=arm64 docker=29.8.0 store=containerd loaded_id=sha256:X manifest=sha256:X config=sha256:Y size=…` line where `loaded_id` equals `manifest`, then `is the archive's image`, rc 0.

- [ ] **Step 9: Prove the identity check bites**

Temporarily change `expected="$config"` to `expected="$manifest"` and `[ "$store" = "containerd" ] && expected="$manifest"` to `[ "$store" = "containerd" ] && expected="$config"`. Run Step 8 again. Expected: the `::error::… is not the archive's image` line and rc 1. Revert both lines with the Edit tool and run Step 8 again: rc 0.

- [ ] **Step 10: Shellcheck and the early-exit ban**

The second command below uses the `EARLY_EXIT_ERE` from `ci/actionlint/run.sh:5005`. It runs
against the file with full-line comments stripped first. The real gate joins logical lines before
it matches. This plain grep needs only the comment strip, to avoid a false hit inside a comment.
Run:
```bash
uv run --locked --project py shellcheck ci/images/run.sh
grep -v '^[[:space:]]*#' ci/images/run.sh | grep -nE '(^|[^|])[|][[:space:]]*([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+)*(command[[:space:]]+)?(grep[[:space:]]([^|]*[[:space:]])?(-[[:alpha:]]*[qm]|--(quiet|silent|max-count))|head([[:space:];)]|$)|awk[[:space:]]([^|]*[^[:alnum:]_])?exit([^[:alnum:]_]|$))' || echo "no early-exit reader"
```
Expected: no shellcheck finding for the new lines, and `no early-exit reader`.

- [ ] **Step 11: Commit**

```bash
git add ci/images/run.sh
git commit -m "feat(ci): build service images as OCI archives and check the loaded identity (SMA-658)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Smoke one service at a time

**Files:**
- Modify: `ci/images/run.sh` (the `smoke` function, from `smoke() {` through the `}` above the
  `cmd=` line; the dispatch, from the comment beginning `` `smoke` and `all` always exercise BOTH
  images `` through `esac`)

**Interfaces:**
- Consumes: nothing new.
- Produces: `ci/images/run.sh smoke [iam|gateway]...` — with no argument it smokes both (old behaviour, gateway first); with arguments it smokes exactly those. Before it smokes a service, it asserts that `paigasus-<svc>:dev` carries this checkout's revision label. `ci/images/run.sh all` still builds with `--load` and smokes both.

- [ ] **Step 1: Run the new form before it exists (the failing check)**

Run: `/bin/bash ci/images/run.sh smoke gateway; echo "rc=$?"`
Expected: `usage: ci/images/run.sh smoke takes no service argument — it always smokes both images`, rc 1.

- [ ] **Step 2: Split `smoke` into per-service functions**

Replace the whole `smoke()` function (from `smoke() {` through the `}` above the `cmd=` line) with the functions below. The body text of `smoke_gateway` is the old lines from `echo "== gateway: standalone =="` through `assert_base_intact paigasus-gateway:dev`, unchanged, and the body of `smoke_iam` is the old lines from `echo "== iam: with postgres, reached BY HOSTNAME =="` through `assert_base_intact paigasus-iam:dev`, unchanged; the uid loop becomes `assert_uid`.

```bash
# The image under test must be the one THIS checkout built. This replaces the old rule that
# `smoke` took no service argument (SMA-500): the danger was that `smoke` would test a stale or
# absent image of the OTHER service and still report SMOKE OK. A per-service smoke never touches
# the other service's image, and this check refuses a stale image of the service it does test.
assert_fresh() {
  local image="$1" rev
  rev="$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image" 2>/dev/null || true)"
  if [ "$rev" != "$REVISION" ]; then
    echo "::error::${image} carries revision '${rev:-<none>}', expected ${REVISION}: a stale or absent image would be smoke-tested." >&2
    return 1
  fi
}

smoke_gateway() {
  # (old lines, unchanged: from `echo "== gateway: standalone =="` through `assert_base_intact paigasus-gateway:dev`)
}

smoke_iam() {
  # (old lines, unchanged: from `echo "== iam: with postgres, reached BY HOSTNAME =="` through `assert_base_intact paigasus-iam:dev`)
}

# `docker top`, not `docker inspect .Config.User`: the latter reads IMAGE config, so a `--user 0`
# invocation would still pass it. (Keep the full `-o pid,uid` rationale comment from the old
# uid loop here, unchanged.)
assert_uid() {
  local c="$1" uid
  uid="$(docker top "$c" -o pid,uid 2>/dev/null | tail -1 | awk '{print $NF}')"
  [ "$uid" = "65532" ] || { echo "::error::$c runs as ${uid}, expected 65532" >&2; return 1; }
  echo "  $c runs as uid ${uid}"
}

smoke() {
  local s
  trap cleanup EXIT
  cleanup
  docker network create "$NET" >/dev/null
  for s in "$@"; do
    case "$s" in
      gateway) assert_fresh paigasus-gateway:dev; smoke_gateway; assert_uid "$GW_NAME" ;;
      iam)     assert_fresh paigasus-iam:dev;     smoke_iam;     assert_uid "$IAM_NAME" ;;
      *) echo "unknown service: $s" >&2; return 1 ;;
    esac
  done
  echo "SMOKE OK ($*)"
}
```

When you paste the two old bodies, copy them exactly from the current file, including every comment and the `local readyz_rc=0` line in the gateway body. Do not re-type them.

- [ ] **Step 3: Update the dispatch**

Replace everything from the comment block (beginning `` # `smoke` and `all` always exercise BOTH
images ``, the block that precedes `case "$cmd" in`) through the `;;` that ends the `all)` arm,
with:

```bash
# `smoke` with no argument smokes both images, gateway first (the old behaviour). With service
# arguments it smokes exactly those; assert_fresh (above) is what stops a stale image from being
# tested. `all` still takes no argument: it builds with --load and smokes both.
case "$cmd" in
  build) assert_pins; for s in "${services[@]}"; do build_one "$s"; done ;;
  smoke)
    shift
    if [ "$#" -eq 0 ]; then smoke gateway iam; else smoke "$@"; fi
    ;;
  all)
    if [ -n "$target" ]; then
      echo "usage: ci/images/run.sh all takes no service argument — it builds and smokes both images; use 'build [iam|gateway]' to build one" >&2
      exit 1
    fi
    assert_pins
    for s in "${services[@]}"; do build_one "$s"; done
    smoke gateway iam
    ;;
```

Keep Task 3's `build-oci)`/`load-oci)` arms between `all)` and `*)`.

Update the header usage line for `smoke`:

```bash
#        ci/images/run.sh smoke [iam|gateway]...   # no argument: both images; else exactly those
```

- [ ] **Step 4: Check the stale-image guard on the fixture**

Re-make the fixture with Task 3 Step 1 and re-load it with Task 3 Step 8. HEAD moved when Task 3
was committed, so a fixture from before that commit fails `assert_fresh` for the wrong reason.

The fixture image is not a real service, so only the guard can be checked locally. The fixture that Task 3 Step 8 loaded is `paigasus-gateway:dev`. Run:
```bash
/bin/bash ci/images/run.sh smoke gateway; echo "rc=$?"
```
Expected: the fixture carries this checkout's revision, so `assert_fresh` passes and the smoke then FAILS in `wait_healthy` (busybox has no healthcheck and exits), rc 1. That failure is expected; what matters is that the output does NOT contain `carries revision`.

Then re-label to a stale revision and run again:
```bash
printf 'FROM busybox:1.36\nLABEL org.opencontainers.image.revision=stale\n' | docker build -q -t paigasus-gateway:dev - >/dev/null
/bin/bash ci/images/run.sh smoke gateway; echo "rc=$?"
```
Expected: `::error::paigasus-gateway:dev carries revision 'stale'`, rc 1, and no container was started.

- [ ] **Step 5: Shellcheck and the early-exit ban**

Run the two commands of Task 3 Step 10. Expected: the same clean result.

- [ ] **Step 6: Commit**

```bash
git add ci/images/run.sh
git commit -m "feat(ci): smoke one service image at a time (SMA-658)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

The real smoke of both services runs in CI (Task 6). Say so in the task report.

---

### Task 5: Rehearse the publish steps against two local registries

**Files:**
- Modify: `ci/images/run.sh` (the pins block that ends at `CURL_8_11_1_DIGEST=…`; a new section placed directly above the `cmd=` dispatch line, after `smoke()`; the dispatch)

**Interfaces:**
- Consumes: `decide`, `kv` (Task 3); `release_decision.py adopt|floating|oci-digests` (Task 2); `crane` on PATH (Task 1).
- Produces: `ci/images/run.sh rehearse <archive.oci.tar>...` — prints `REHEARSE OK` and exits 0, or exits 1 on the first failed assertion, or 2 when crane is missing. PR 2's `publish-images` job copies the command sequence of case 1–3; `tag_digest`'s "missing tag vs. failure" rule is the rule PR 2 must keep.

- [ ] **Step 1: Run the command before it exists (the failing check)**

Run: `/bin/bash ci/images/run.sh rehearse "$FIX"/paigasus-gateway-*.oci.tar; echo "rc=$?"`
Expected: `unknown command: rehearse`, rc 1. (Re-make `$FIX` with Task 3 Step 1 if the shell is new.)

- [ ] **Step 2: Pin `registry:2` by digest**

Run: `docker buildx imagetools inspect registry:2 --format '{{.Manifest.Digest}}'`
Add to the pins block, after `CURL_8_11_1_DIGEST=…`, with the printed 64-hex digest:

```bash
# SMA-658: the two throwaway registries that `rehearse` pushes to. Refresh with:
#   docker buildx imagetools inspect registry:2 --format '{{.Manifest.Digest}}'
REGISTRY_2_DIGEST="registry:2@sha256:<the 64 hex characters that command printed>"
```

(The `<…>` is a value you paste from the command output, not text to keep.)

- [ ] **Step 3: Add the rehearse helpers and `rehearse`**

Add directly above the `cmd="${1:?…}"` line (orig `:439`), after `smoke()`'s closing `}`. `RUN_ID`
(orig `:189`) must be set before this point. `REH_A_NAME` and `REH_B_NAME` below read `RUN_ID`. An
earlier position triggers `set -u` and fails every command, including `nosuchcmd` and `smoke`:

```bash
# --- rehearse (SMA-658 spec § 8) -----------------------------------------------------------------
# Runs the publish sequence of the future release path against two LOCAL registries: A stands in
# for GHCR and B for Docker Hub. It needs no credential, so images.yml runs it on a pull request.
# It proves the parts that do not need OIDC or a secret: push by digest, the index, the
# digest-preserving copy, the D10 adoption rule, the conflict refusal and the floating-tag rule.
# The decisions come from release_decision.py, so PR 2 runs the SAME decision code; the registry
# commands here are a copy of PR 2's sequence, which is the residual (spec § 7.1 wants them
# literal in release.yml).
REH_A_NAME="rehearse-a-${RUN_ID}"
REH_B_NAME="rehearse-b-${RUN_ID}"
REH_TMP=""

rehearse_cleanup() {
  docker rm -f "$REH_A_NAME" "$REH_B_NAME" >/dev/null 2>&1 || true
  if [ -n "$REH_TMP" ]; then rm -rf "$REH_TMP"; fi
}

rh_fail() {
  echo "::error::rehearse: $*" >&2
  return 1
}

# Starts a registry:2 on a free localhost port and prints `localhost:<port>`. `localhost`, not
# 127.0.0.1, because crane and buildx both treat a `localhost` registry as plain HTTP.
start_registry() {
  local name="$1" hostport port i
  docker run -d --name "$name" -p 127.0.0.1::5000 "$REGISTRY_2_DIGEST" >/dev/null
  hostport="$(docker port "$name" 5000/tcp | sed -n 1p)"
  port="${hostport##*:}"
  for i in $(seq 1 30); do
    if crane catalog --insecure "localhost:${port}" >/dev/null 2>&1; then
      echo "localhost:${port}"
      return 0
    fi
    sleep 1
  done
  rh_fail "registry ${name} never answered on localhost:${port} (${i}s)"
}

# The digest that `$1:$2` resolves to, or `none` when the tag does not exist. Any OTHER failure
# (a network error, a registry 5xx, auth) is fatal: reading it as "absent" would let a real
# release push a second digest under a published version (spec D10). PR 2 keeps this rule.
tag_digest() {
  local ref="$1:$2" out rc=0
  out="$(crane digest --insecure "$ref" 2>&1)" || rc=$?
  if [ "$rc" -eq 0 ]; then
    printf '%s\n' "$out"
    return 0
  fi
  case "$out" in
    *MANIFEST_UNKNOWN*|*NAME_UNKNOWN*) echo "none" ;;
    *) rh_fail "cannot read ${ref}: ${out}" ;;
  esac
}

expect_kv() {
  local got
  got="$(kv "$1" "$2")"
  [ "$got" = "$3" ] || rh_fail "expected $2=$3, got $2=${got:-<empty>}"
}

expect_tag() {
  local got
  got="$(tag_digest "$1" "$2")"
  [ "$got" = "$3" ] || rh_fail "$1:$2 is ${got}, expected $3"
}

rehearse() {
  if [ "$#" -lt 1 ]; then
    echo "usage: ci/images/run.sh rehearse <archive.oci.tar>..." >&2
    return 1
  fi
  if ! command -v crane >/dev/null 2>&1; then
    echo "::error::crane is not on PATH; run 'proto install crane'" >&2
    return 2
  fi
  trap rehearse_cleanup EXIT
  REH_TMP="$(mktemp -d "${TMPDIR:-/tmp}/paigasus-rehearse.XXXXXX")"
  local a b repo_a repo_b archive digests manifest platform arch refs first index index2 rebuilt ga gb out rc r t
  refs=()
  a="$(start_registry "$REH_A_NAME")"
  b="$(start_registry "$REH_B_NAME")"
  repo_a="${a}/paigasus-rehearse"
  repo_b="${b}/paigasus-rehearse"

  echo "== rehearse: push each platform to A and keep its digest =="
  for archive in "$@"; do
    digests="$(decide oci-digests "$archive")"
    manifest="$(kv "$digests" manifest)"
    platform="$(kv "$digests" platform)"
    arch="${platform#*/}"
    mkdir -p "$REH_TMP/$arch"
    tar -xf "$archive" -C "$REH_TMP/$arch"
    crane push --insecure "$REH_TMP/$arch" "${repo_a}:${REVISION}-${arch}" >/dev/null
    expect_tag "$repo_a" "${REVISION}-${arch}" "$manifest"
    refs+=("${repo_a}@${manifest}")
    echo "  ${arch}: ${manifest}"
  done
  first="${refs[0]#*@}"
  docker buildx imagetools create --tag "${repo_a}:${REVISION}" "${refs[@]}"
  index="$(tag_digest "$repo_a" "$REVISION")"
  echo "  index: ${index}"

  echo "== case 1: nothing published -> push-new, copy to B, tag both =="
  ga="$(tag_digest "$repo_a" 0.1.0)"
  gb="$(tag_digest "$repo_b" 0.1.0)"
  out="$(decide adopt --new-digest "$index" --ghcr "$ga" --dockerhub "$gb" --git-tag absent)"
  expect_kv "$out" action push-new
  docker buildx imagetools create --tag "${repo_b}:${REVISION}" "${repo_a}@${index}"
  expect_tag "$repo_b" "$REVISION" "$index"
  : > "$REH_TMP/tags"
  out="$(decide floating --service gateway --version 0.1.0 --tags-file "$REH_TMP/tags")"
  expect_kv "$out" move true
  for r in "$repo_a" "$repo_b"; do
    crane tag --insecure "${r}@${index}" 0.1.0
    crane tag --insecure "${r}@${index}" "$(kv "$out" minor_tag)"
    crane tag --insecure "${r}@${index}" latest
    for t in 0.1.0 0.1 latest; do expect_tag "$r" "$t" "$index"; done
  done

  echo "== case 2: a rebuild of a published version -> adopt the first digest, overwrite nothing =="
  crane mutate --insecure "${repo_a}@${first}" --label org.opencontainers.image.description=rehearsal-rebuild -t "${repo_a}:rebuild" >/dev/null
  rebuilt="$(tag_digest "$repo_a" rebuild)"
  [ "$rebuilt" != "$first" ] || rh_fail "the rebuild kept the first digest; the case proves nothing"
  docker buildx imagetools create --tag "${repo_a}:rebuild-index" "${repo_a}@${rebuilt}"
  index2="$(tag_digest "$repo_a" rebuild-index)"
  ga="$(tag_digest "$repo_a" 0.1.0)"
  gb="$(tag_digest "$repo_b" 0.1.0)"
  out="$(decide adopt --new-digest "$index2" --ghcr "$ga" --dockerhub "$gb" --git-tag absent)"
  expect_kv "$out" action adopt
  expect_kv "$out" digest "$index"
  expect_kv "$out" copy_to none
  # A real "overwrite nothing" claim needs a write that COULD have happened. Gate a tag move to
  # the rebuild's index behind the same condition PR 2 will use, so a branch that pushed
  # unconditionally would move the tag to $index2 and the assertion below would catch it.
  if [ "$(kv "$out" action)" = push-new ]; then
    for r in "$repo_a" "$repo_b"; do crane tag --insecure "${r}@${index2}" 0.1.0; done
  fi
  expect_tag "$repo_a" 0.1.0 "$index"
  expect_tag "$repo_b" 0.1.0 "$index"

  echo "== case 3: only A holds the version -> adopt A's digest and copy it to B =="
  crane tag --insecure "${repo_a}@${index2}" 0.1.1
  ga="$(tag_digest "$repo_a" 0.1.1)"
  gb="$(tag_digest "$repo_b" 0.1.1)"
  out="$(decide adopt --new-digest "$index" --ghcr "$ga" --dockerhub "$gb" --git-tag absent)"
  expect_kv "$out" action adopt
  expect_kv "$out" digest "$index2"
  expect_kv "$out" copy_to dockerhub
  docker buildx imagetools create --tag "${repo_b}:0.1.1" "${repo_a}@${index2}"
  expect_tag "$repo_b" 0.1.1 "$index2"

  echo "== case 4: the registries disagree -> refuse (exit 3) =="
  crane tag --insecure "${repo_a}@${index}" 0.1.2
  docker buildx imagetools create --tag "${repo_b}:0.1.2" "${repo_a}@${index2}"
  ga="$(tag_digest "$repo_a" 0.1.2)"
  gb="$(tag_digest "$repo_b" 0.1.2)"
  rc=0
  out="$(decide adopt --new-digest "$index" --ghcr "$ga" --dockerhub "$gb" --git-tag absent)" || rc=$?
  [ "$rc" -eq 3 ] || rh_fail "a digest conflict must exit 3, got ${rc}"
  expect_kv "$out" action conflict

  echo "== case 5: an older version than a released one -> the floating tags hold =="
  printf 'abc123\trefs/tags/paigasus-gateway-v0.2.0\n' > "$REH_TMP/tags"
  out="$(decide floating --service gateway --version 0.1.1 --tags-file "$REH_TMP/tags")"
  expect_kv "$out" move false
  # Same reasoning as case 2: gate the case-1 tag-move loop behind the real condition, using
  # $index2 (already known to differ from $index) as the would-be new value, so a branch that
  # moved the tags unconditionally would be caught below.
  if [ "$(kv "$out" move)" = true ]; then
    for r in "$repo_a" "$repo_b"; do
      crane tag --insecure "${r}@${index2}" 0.1.1
      crane tag --insecure "${r}@${index2}" "$(kv "$out" minor_tag)"
      crane tag --insecure "${r}@${index2}" latest
    done
  fi
  expect_tag "$repo_a" latest "$index"
  expect_tag "$repo_b" latest "$index"

  echo "REHEARSE OK"
}
```

Add to the dispatch, before `*)`:

```bash
  rehearse) shift; rehearse "$@" ;;
```

And to the header usage block:

```bash
#        ci/images/run.sh rehearse <archive.oci.tar>...      # SMA-658: publish steps vs two local registries
```

- [ ] **Step 4: Run it on the fixture**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
/bin/bash ci/images/run.sh rehearse "$FIX"/paigasus-gateway-*.oci.tar; echo "rc=$?"
docker ps -a --filter name=rehearse- --format '{{.Names}}'
```
Expected: the five case headers, `REHEARSE OK`, rc 0; the `docker ps` prints nothing (cleanup ran).

If `tag_digest` reports `cannot read …` for a tag that does not exist, run `crane digest --insecure localhost:<port>/paigasus-rehearse:nope` by hand, and add the error word that crane 0.22.1 prints for a missing tag to the `case` pattern. Record the word in the task report.

- [ ] **Step 5: Prove the rehearse bites**

Temporarily change `expect_kv "$out" copy_to dockerhub` in case 3 to `expect_kv "$out" copy_to none`. Run Step 4. Expected: `::error::rehearse: expected copy_to=none, got copy_to=dockerhub`, rc 1, and no `rehearse-` container left. Revert with the Edit tool and run Step 4 again: rc 0.

- [ ] **Step 6: Shellcheck and the early-exit ban**

Run the two commands of Task 3 Step 10. Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add ci/images/run.sh
git commit -m "feat(ci): rehearse the image publish steps against local registries (SMA-658)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: Run the release build path in `images.yml`

**Files:**
- Modify: `.github/workflows/images.yml` (whole file)

**Interfaces:**
- Consumes: `run.sh build-oci|load-oci|smoke|rehearse` (Tasks 3–5), `release_decision.py --self-test|sbom-summary` (Task 2), `proto install crane|syft|uv` (Task 1).
- Produces: per-runner log lines `M3 …`, `M8 …`, and artifacts `chisel-manifests-<arch>`, `sboms-<arch>` (90-day retention). Task 8 reads them.

- [ ] **Step 1: Replace the file**

```yaml
# SPDX-License-Identifier: Apache-2.0
name: images

on:
  workflow_dispatch:

  # POST-merge verification. `rs/**` because any service or dependency change can break an
  # image build, and this is the only place that is checked.
  push:
    branches:
      - main
    paths:
      - 'rs/**'
      - 'ci/images/**'
      - '.github/workflows/images.yml'
      - '.prototools'
      - '.proto/plugins/crane.toml'
      - '.proto/plugins/syft.toml'

  # PRE-merge verification of the BUILD INPUTS only. `rs/**` is deliberately absent: most PRs
  # here touch it, and two cold --release builds on every one of them would raise the bill
  # (SMA-520). These are the inputs that can actually break an image build — a new dependency,
  # a toolchain bump, a tool pin, or the build machinery itself.
  pull_request:
    branches:
      - main
    paths:
      - 'rs/Cargo.lock'
      - 'rs/Cargo.toml'
      - 'rs/rust-toolchain.toml'
      - 'rs/Dockerfile'
      - 'rs/.dockerignore'
      - 'ci/images/**'
      - '.github/workflows/images.yml'
      - '.prototools'
      - '.proto/plugins/crane.toml'
      - '.proto/plugins/syft.toml'

# Build-and-verify only: no registry, no credentials, nothing is pushed (SMA-500 D1). The
# `rehearse` step pushes only to two registry:2 containers on the runner itself.
permissions:
  contents: read

concurrency:
  # `event_name` is in the GROUP so a manual dispatch cannot cancel a running push job.
  group: images-${{ github.workflow }}-${{ github.ref }}-${{ github.event_name }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

jobs:
  images:
    name: build + smoke (${{ matrix.arch }})
    # SMA-658: the RELEASE build path (OCI archive -> load -> identity check -> per-service
    # smoke -> rehearse), one leg per architecture on a NATIVE runner. A pull request runs amd64
    # only; `main` and a dispatch also run arm64 (spec M5), so a PR can get the arm64 evidence
    # with `gh workflow run images.yml --ref <branch>`.
    runs-on: ${{ matrix.arch == 'arm64' && 'ubuntu-24.04-arm' || 'ubuntu-latest' }}
    # Two cold --release builds of a tree that has already overflowed the runner disk once.
    # Both services build in ONE leg: the smoke suite needs both images on one daemon.
    timeout-minutes: 60
    strategy:
      fail-fast: false
      matrix:
        arch: ${{ fromJSON(github.event_name == 'pull_request' && '["amd64"]' || '["amd64", "arm64"]') }}
    env:
      ARCH: ${{ matrix.arch }}
      # SMA-609: every captured shim output must set this, so `$(uv run …)` never captures proto's
      # NDJSON preamble.
      PROTO_REPORTER: text
    steps:
      # Same reclaim as ci.yml: this builds the cedar-policy tree in --release on a ~14 GB disk.
      - name: Reclaim runner disk (drop unused preinstalled toolchains)
        run: |
          df -h /
          sudo rm -rf /usr/local/lib/android /usr/share/dotnet /opt/ghc /usr/local/.ghcup \
            /opt/hostedtoolcache/CodeQL || true
          sudo docker image prune --all --force > /dev/null 2>&1 || true
          df -h /

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

      # Narrow installs, not a bare `proto install`: this job needs only these three.
      - name: Install crane, syft and uv
        run: |
          proto install crane
          proto install syft
          proto install uv

      - name: Decision script self-test
        run: uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py --self-test

      - name: Build both images as OCI archives
        run: |
          ci/images/run.sh build-oci iam out
          ci/images/run.sh build-oci gateway out

      # Prints one `M3 …` line per image: this runner's image store and what `docker load`
      # reports as the image ID (spec M3).
      - name: Load each archive and check its identity
        run: |
          for crate in paigasus-iam paigasus-gateway; do
            ci/images/run.sh load-oci "out/${crate}-${ARCH}.oci.tar" "${crate}:dev"
          done

      - name: Smoke each service on its own
        run: |
          ci/images/run.sh smoke gateway
          ci/images/run.sh smoke iam

      - name: Rehearse the publish steps against local registries
        run: ci/images/run.sh rehearse "out/paigasus-gateway-${ARCH}.oci.tar"

      # A MEASUREMENT for spec M8, not an assertion: does syft see libc6 and the Rust crates?
      - name: SBOM for each archive
        run: |
          for crate in paigasus-iam paigasus-gateway; do
            syft "oci-archive:out/${crate}-${ARCH}.oci.tar" -o "spdx-json=sbom-${crate}-${ARCH}.spdx.json"
            summary="$(uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-summary "sbom-${crate}-${ARCH}.spdx.json")"
            echo "M8 image=${crate} arch=${ARCH} $(printf '%s' "$summary" | tr '\n' ' ')"
          done

      - name: Upload chisel package manifests
        if: always()
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: chisel-manifests-${{ matrix.arch }}
          path: chisel-manifest-*.txt
          if-no-files-found: ignore
          # 90, not the 14-day default: any image built from this manifest can and does outlive
          # a 14-day artifact expiry, and the manifest is the only record of which chisel-cut
          # package versions (including libc6) that image actually shipped.
          retention-days: 90

      - name: Upload SBOMs
        if: always()
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: sboms-${{ matrix.arch }}
          path: sbom-*.spdx.json
          if-no-files-found: ignore
          retention-days: 90
```

- [ ] **Step 2: Lint the workflow locally (no shellcheck)**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
actionlint -shellcheck= .github/workflows/images.yml; echo "rc=$?"
```
Expected: rc 0. (The shellcheck pass runs in CI; locally it can hang on this Mac's 512-byte pipes.)

- [ ] **Step 3: Check the new `paths:` globs match the tree (check 5)**

Run: `git ls-files -- .prototools .proto/plugins/crane.toml .proto/plugins/syft.toml 'ci/images/**'`
Expected: every listed path prints (the plugin files after Task 1 is committed).

- [ ] **Step 4: Check the early-exit ban on the workflow**

Run (the `EARLY_EXIT_ERE` from `ci/actionlint/run.sh:5005`, comments stripped first — see Task 3
Step 10):
```bash
grep -v '^[[:space:]]*#' .github/workflows/images.yml | grep -nE '(^|[^|])[|][[:space:]]*([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+)*(command[[:space:]]+)?(grep[[:space:]]([^|]*[[:space:]])?(-[[:alpha:]]*[qm]|--(quiet|silent|max-count))|head([[:space:];)]|$)|awk[[:space:]]([^|]*[^[:alnum:]_])?exit([^[:alnum:]_]|$))' || echo "no early-exit reader"
```
Expected: `no early-exit reader`.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/images.yml
git commit -m "ci(repo): run the release build path in images.yml, with an arm64 leg off PRs (SMA-658)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: The rehearsal workflow for the OIDC steps

**Files:**
- Create: `.github/workflows/images-rehearsal.yml`

**Interfaces:**
- Consumes: `run.sh build-oci` (Task 3), `release_decision.py oci-digests` (Task 2), `proto install crane|cosign|syft|uv` (Task 1).
- Produces: a dispatch-only workflow. After PR 1 merges, `gh workflow run images-rehearsal.yml --ref main` pushes `ghcr.io/smk1085/paigasus-rehearsal:<sha>` (a two-platform index of the gateway image), attests it, signs it, and verifies both. PR 2's `publish-images` job copies its attest/sign/verify steps.

- [ ] **Step 1: Write the file**

```yaml
# SPDX-License-Identifier: Apache-2.0
name: images-rehearsal

# SMA-658 PR 1 (spec § 8). Runs the signing and attestation steps of the future release path FOR
# REAL, against a scratch GHCR package, so they have run once before the first real release:
# cosign keyless, attest-build-provenance, attest-sbom (per-platform subject), cosign verify and
# gh attestation verify. It publishes no release image and reads no secret.
#
# workflow_dispatch ONLY. A pull_request trigger would let a PR mint an OIDC token
# (repo:workflow-credentials bans that shape), and both jobs refuse any ref but main.
# Run it with: gh workflow run images-rehearsal.yml --ref main
on:
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: images-rehearsal
  cancel-in-progress: false

env:
  IMAGE: ghcr.io/smk1085/paigasus-rehearsal

jobs:
  build:
    name: build gateway (${{ matrix.arch }})
    runs-on: ${{ matrix.arch == 'arm64' && 'ubuntu-24.04-arm' || 'ubuntu-latest' }}
    timeout-minutes: 60
    strategy:
      fail-fast: false
      matrix:
        arch:
          - amd64
          - arm64
    env:
      ARCH: ${{ matrix.arch }}
      # SMA-609: every captured shim output must set this (Global Constraint `:36`).
      PROTO_REPORTER: text
    steps:
      - name: Refuse any ref but main
        run: |
          if [ "$GITHUB_REF" != "refs/heads/main" ]; then
            echo "::error::images-rehearsal runs only from main (got ${GITHUB_REF})." >&2
            exit 1
          fi

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

      - name: Install syft
        run: proto install syft

      - name: Build the gateway image as an OCI archive
        run: ci/images/run.sh build-oci gateway out

      - name: SBOM for this platform
        run: syft "oci-archive:out/paigasus-gateway-${ARCH}.oci.tar" -o "spdx-json=out/sbom-paigasus-gateway-${ARCH}.spdx.json"

      - name: Upload the archive and its SBOM
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: rehearsal-${{ matrix.arch }}
          path: |
            out/paigasus-gateway-${{ matrix.arch }}.oci.tar
            out/sbom-paigasus-gateway-${{ matrix.arch }}.spdx.json
          if-no-files-found: error
          retention-days: 7

  publish:
    name: push, attest, sign and verify on GHCR
    needs: build
    runs-on: ubuntu-latest
    timeout-minutes: 30
    permissions:
      contents: read
      packages: write       # push to ghcr.io/smk1085/paigasus-rehearsal
      id-token: write       # cosign keyless and the attestations
      attestations: write   # actions/attest-*
    steps:
      - name: Refuse any ref but main
        run: |
          if [ "$GITHUB_REF" != "refs/heads/main" ]; then
            echo "::error::images-rehearsal runs only from main (got ${GITHUB_REF})." >&2
            exit 1
          fi

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

      - name: Install crane, cosign and uv
        run: |
          proto install crane
          proto install cosign
          proto install uv

      - name: Download both archives and SBOMs
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          pattern: rehearsal-*
          merge-multiple: true
          path: in

      # github.token, never secrets.GITHUB_TOKEN: the latter is a `secrets` reference, and PR 2
      # copies these steps into release.yml, where V10 rule 1 pins the secret names.
      - name: Log in to GHCR
        uses: docker/login-action@dbcb813823bdd20940b903addbd779551569679f  # v4.6.0
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ github.token }}

      - name: Push each platform by digest, then the index
        id: push
        run: |
          set -euo pipefail
          export PROTO_REPORTER=text
          refs=()
          for arch in amd64 arm64; do
            archive="in/paigasus-gateway-${arch}.oci.tar"
            digests="$(uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py oci-digests "$archive")"
            manifest="$(printf '%s\n' "$digests" | sed -n 's/^manifest=//p')"
            layout="$(mktemp -d)"
            tar -xf "$archive" -C "$layout"
            crane push "$layout" "${IMAGE}:${GITHUB_SHA}-${arch}"
            got="$(crane digest "${IMAGE}:${GITHUB_SHA}-${arch}")"
            if [ "$got" != "$manifest" ]; then
              echo "::error::crane push changed the ${arch} digest: ${manifest} -> ${got}" >&2
              exit 1
            fi
            echo "digest_${arch}=${manifest}" >> "$GITHUB_OUTPUT"
            refs+=("${IMAGE}@${manifest}")
          done
          docker buildx imagetools create --tag "${IMAGE}:${GITHUB_SHA}" "${refs[@]}"
          index="$(crane digest "${IMAGE}:${GITHUB_SHA}")"
          echo "index=${index}" >> "$GITHUB_OUTPUT"

      - name: Attest build provenance (index)
        uses: actions/attest-build-provenance@4d101475d8b20a2381f78447822ac1eab6504dd8  # v4.2.2
        with:
          subject-name: ${{ env.IMAGE }}
          subject-digest: ${{ steps.push.outputs.index }}
          push-to-registry: true

      - name: Attest the amd64 SBOM (per-platform subject)
        uses: actions/attest-sbom@c604332985a26aa8cf1bdc465b92731239ec6b9e  # v4.1.0
        with:
          subject-name: ${{ env.IMAGE }}
          subject-digest: ${{ steps.push.outputs.digest_amd64 }}
          sbom-path: in/sbom-paigasus-gateway-amd64.spdx.json
          push-to-registry: true

      - name: Attest the arm64 SBOM (per-platform subject)
        uses: actions/attest-sbom@c604332985a26aa8cf1bdc465b92731239ec6b9e  # v4.1.0
        with:
          subject-name: ${{ env.IMAGE }}
          subject-digest: ${{ steps.push.outputs.digest_arm64 }}
          sbom-path: in/sbom-paigasus-gateway-arm64.spdx.json
          push-to-registry: true

      - name: Sign the index (cosign keyless)
        env:
          INDEX: ${{ steps.push.outputs.index }}
        run: cosign sign --yes "${IMAGE}@${INDEX}"

      - name: Verify the signature and the attestations
        env:
          INDEX: ${{ steps.push.outputs.index }}
          DIGEST_AMD64: ${{ steps.push.outputs.digest_amd64 }}
          DIGEST_ARM64: ${{ steps.push.outputs.digest_arm64 }}
          GH_TOKEN: ${{ github.token }}
        run: |
          set -euo pipefail
          cosign verify \
            --certificate-identity "https://github.com/SMK1085/paigasus-core/.github/workflows/images-rehearsal.yml@refs/heads/main" \
            --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
            "${IMAGE}@${INDEX}"
          gh attestation verify "oci://${IMAGE}@${INDEX}" \
            --repo SMK1085/paigasus-core \
            --signer-workflow SMK1085/paigasus-core/.github/workflows/images-rehearsal.yml
          for d in "$DIGEST_AMD64" "$DIGEST_ARM64"; do
            gh attestation verify "oci://${IMAGE}@${d}" \
              --repo SMK1085/paigasus-core \
              --signer-workflow SMK1085/paigasus-core/.github/workflows/images-rehearsal.yml \
              --predicate-type https://spdx.dev/Document/v2.3
          done
          echo "REHEARSAL OK: ${IMAGE}@${INDEX}"
```

- [ ] **Step 2: Lint locally (no shellcheck) and check the early-exit ban**

Run:
```bash
actionlint -shellcheck= .github/workflows/images-rehearsal.yml; echo "rc=$?"
# The EARLY_EXIT_ERE from ci/actionlint/run.sh:5005, comments stripped first — see Task 3 Step 10.
grep -v '^[[:space:]]*#' .github/workflows/images-rehearsal.yml | grep -nE '(^|[^|])[|][[:space:]]*([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+)*(command[[:space:]]+)?(grep[[:space:]]([^|]*[[:space:]])?(-[[:alpha:]]*[qm]|--(quiet|silent|max-count))|head([[:space:];)]|$)|awk[[:space:]]([^|]*[^[:alnum:]_])?exit([^[:alnum:]_]|$))' || echo "no early-exit reader"
```
Expected: rc 0 and `no early-exit reader`.

- [ ] **Step 3: Confirm `repo:workflow-credentials` does not treat it as a subject**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
/opt/homebrew/bin/bash ci/workflow-credentials/run.sh; echo "rc=$?"
```
Expected: rc 0. If it hangs for more than two minutes (this Mac's pipe issue), stop it and record "not run locally; CI runs it" in the task report.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/images-rehearsal.yml
git commit -m "ci(repo): add a dispatch-only rehearsal for image signing and attestation (SMA-658)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 8: Documentation and the spec record

**Files:**
- Modify: `docs/ops/RUNBOOK-containers.md`
- Modify: `docs/superpowers/specs/2026-09-19-sma-658-container-release-design.md`
- Modify: `CLAUDE.md`

**Interfaces:**
- Consumes: everything above.
- Produces: the operator how-to for PR 1, and the spec's record of the decisions this plan made.

- [ ] **Step 1: Add a runbook section**

Append to `docs/ops/RUNBOOK-containers.md` a section `## Release tooling (SMA-658, PR 1)` with these subsections, in STE:

- **Tools.** crane 0.22.1, cosign 3.1.3 and syft 1.52.0, pinned in `.prototools` through vendored plugins in `.proto/plugins/`. Install one with `proto install <tool>`. `ci.yml`'s bare `proto install` now also installs them.
- **New `ci/images/run.sh` commands.** One line each for `build-oci`, `load-oci`, `smoke [svc]…` and `rehearse`, copied from the script's usage header. Say that `load-oci` prints an `M3 …` line and why the expected ID depends on the image store.
- **The rehearsal workflow.** When to run it (once after PR 1 merges, and again after any change to the attest or sign steps), the command `gh workflow run images-rehearsal.yml --ref main`, what `REHEARSAL OK` proves, and what it does NOT prove: the Docker Hub login, the Docker Hub copy and the git tag (PR 2's first real release is their first test).
- **The scratch package.** `ghcr.io/smk1085/paigasus-rehearsal` is private after its first push. It holds only rehearsal images. Delete old versions in the package settings when they are not needed.

Also edit two existing runbook lines, since `smoke` and the chisel-manifest artifact changed shape
in Tasks 3, 4 and 6:

- Runbook `:26` (`ci/images/run.sh smoke            # smoke-test whatever images are already built`)
  becomes: `ci/images/run.sh smoke [iam|gateway]...  # smoke-test images built at this HEAD`.
- Runbook `:326-328` (the "Which libc is in the image I am running?" section) names the artifact
  `chisel-manifests` and the file `chisel-manifest-<service>.txt`. Rewrite it to say: the artifact
  is `chisel-manifests-<arch>`; `build-oci` writes `chisel-manifest-<service>-<arch>.txt`; the
  plain `build` command still writes the un-arched `chisel-manifest-<service>.txt`.

Also edit `CLAUDE.md`, since it is now incomplete rather than false:

- `CLAUDE.md:312` (`` Container images (SMA-500) live behind `ci/images/run.sh {build,smoke,all}`
  and ``) — add the new commands: `ci/images/run.sh {build,smoke,all,build-oci,load-oci,rehearse}`.
- `CLAUDE.md:316-318` (the `pull_request` trigger's path list: `rs/Dockerfile`,
  `rs/Cargo.{lock,toml}`, `rs/rust-toolchain.toml`, `rs/.dockerignore`, `ci/images/**` and the
  workflow itself) — add `.prototools`, `.proto/plugins/crane.toml` and `.proto/plugins/syft.toml`
  (not `cosign.toml`: `images.yml` does not use cosign).

- [ ] **Step 2: Record the plan's decisions in the spec**

Add a section `## 16. Decisions made in the PR 1 plan` to the spec, with one row each:

| # | Decision | Reason |
|---|---|---|
| P1 | The per-platform images are pushed under `:<git-sha>-<arch>` and their digest is asserted, instead of a push "by digest" with no tag. | `crane push` writes to a tag reference. The tag also makes each platform image findable. |
| P2 | The release decisions live in `ci/images/release_decision.py` (stdlib, self-test), used by both `rehearse` and PR 2. | § 7.1 wants registry commands literal in `release.yml`, so the rehearse can share only the decision code. The command sequence in `rehearse` is a copy: that is the residual, and the `images-rehearsal.yml` push step is a second copy of the same `decide`/`kv` and push-then-assert sequence. |
| P3 | `tag_digest` reads only a missing-tag error as "absent"; every other error is fatal. | D10: an error read as "absent" would push a second digest under a published version. |
| P4 | `ci.yml`'s bare `proto install` now also downloads crane, cosign and syft. | One pin source. The cost is measured on PR 1's CI run (Task 9). |
| P5 | M9 (an environment secret reaching a `release.yml` job) is not measured in PR 1. | The rehearsal has no environment and no secret. PR 2's first real run is the first test. |
| P6 | `smoke` takes service arguments. The reject-argument guard (orig `:444-456`) is replaced by `assert_fresh`, which refuses an image whose revision label is not HEAD. | A per-service chain (§ 4.2) needs a one-service smoke. `assert_fresh` closes the same stale-image risk, and it is stricter. |

- [ ] **Step 3: Commit**

```bash
git add docs/ops/RUNBOOK-containers.md docs/superpowers/specs/2026-09-19-sma-658-container-release-design.md CLAUDE.md
git commit -m "docs(repo): document the SMA-658 image release tooling and the PR 1 decisions

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 9: CI evidence (after the PR is open — the controller does this, not an implementer)

**Files:**
- Modify: `docs/superpowers/specs/2026-09-19-sma-658-container-release-design.md` (§ 9 rows M3, M5, M8; § 16 row P4)
- Modify: `CLAUDE.md` only if a measurement contradicts a statement in it or in the spec.

- [ ] **Step 1: Read the PR's `images` run (amd64)**

Run: `gh run list --workflow images.yml --branch feature/sma-658-container-release --limit 1`, then `gh run view <id> --log > images-run.log` and `grep -E ' (M3|M8|REHEARSE OK|SMOKE OK)' images-run.log`.
Expected: two `M3` lines and two `M8` lines for amd64, and `REHEARSE OK`, `SMOKE OK (gateway)`, `SMOKE OK (iam)`.

- [ ] **Step 2: Get the arm64 evidence before the merge**

Run: `gh workflow run images.yml --ref feature/sma-658-container-release`, wait for it, and read its arm64 leg the same way (spec M5).

- [ ] **Step 3: Measure P4's cost**

From the PR's `ci.yml` run, read the duration of the step "Install pinned CLIs from .prototools" and compare it with the same step on the last `main` run.

- [ ] **Step 4: Record the results**

Update the spec's § 9 rows M3 (both runner labels), M5 (arm64 smoke) and M8 (what syft found for each image), and § 16 row P4 (the measured seconds). If M8 shows `libc6=false` or `cargo=0`, write that down plainly: PR 2 then needs the § 4.5 remedy (`cargo auditable`, `base-files_chisel`), and AC 7 cannot pass without it.

- [ ] **Step 5: Commit and push**

```bash
git add docs/superpowers/specs/2026-09-19-sma-658-container-release-design.md
git commit -m "docs(repo): record the SMA-658 CI measurements M3, M5 and M8

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
git push
```

---

## After PR 1 merges (not part of this plan)

1. Run `gh workflow run images-rehearsal.yml --ref main` and wait for `REHEARSAL OK`.
2. Write the PR 2 plan from the spec and the measured values.

## Self-review record

- **Spec coverage (PR 1 scope, § 10):** proto pins → Task 1; `run.sh` OCI mode, version label, identity check, per-service smoke, `.Size` reporting → Tasks 3–4; `rehearse` with D10 adoption, conflict and floating rule → Tasks 2 and 5; `images.yml` release path with arm64 off PRs → Task 6; rehearsal workflow → Task 7; runbook → Task 8; M3/M5/M8 → Tasks 6 and 9. Out of PR 1 by design: `release.yml`, `ci/release-plan/`, the guards, the `0.1.0` bump, the changelog check.
- **`.Size` under the containerd store:** the plan reports it in the `M3` line and keeps the existing 200 MB ceiling in `assert_base_intact`, which reads the same field. Local M3 measured `.Size` as the unpacked size, which is at least the compressed size, so the ceiling does not become weaker.
- **Names used across tasks:** `decide`, `kv`, `version_for`, `extract_chisel_manifest`, `build_oci`, `load_oci`, `assert_fresh`, `smoke_gateway`, `smoke_iam`, `assert_uid`, `tag_digest`, `expect_kv`, `expect_tag`, `start_registry`, `rehearse`; archive path `out/paigasus-<svc>-<arch>.oci.tar`; SBOM path `sbom-paigasus-<svc>-<arch>.spdx.json` in `images.yml`, `out/sbom-paigasus-<svc>-<arch>.spdx.json` in `images-rehearsal.yml` (a deliberate difference: the rehearsal uploads its whole `out/` directory). Checked for consistency.
